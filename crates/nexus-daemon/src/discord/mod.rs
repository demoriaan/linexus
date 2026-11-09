//! Discord on the strip.
//!
//! While something needs Discord (Discord buttons or widgets in the active
//! profile, a profile that follows voice, its notifications or the muted
//! badge), a worker thread stays signed in to the Discord desktop app over
//! its local RPC socket. It follows your own voice state (channel, mute,
//! deafen, audio levels, the connection's ping), who sits and speaks in the
//! voice channels the widgets show, and the notifications Discord shows,
//! and it sends the buttons' commands. Without Discord set up in Studio it
//! stays idle and keeps no connection.

use crate::dispatch::Feedback;
use crate::log::{log_fields, Level};
use nexus_config::{ActionSpec, DiscordCommand, NexusConfig, Profile, WidgetKind};
use nexus_render::tiny_skia::Pixmap;
use nexus_render::DiscordInfo;
use nexus_sensors::{DiscordApp, DiscordConnection, DiscordError};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

mod images;
mod request;
mod tracked;
mod worker;

use worker::Worker;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// First wait before connecting again; doubles while Discord stays away.
const RETRY_MIN: Duration = Duration::from_secs(3);
const RETRY_MAX: Duration = Duration::from_secs(30);
/// A missing authorization or an unsupported client does not fix itself
/// soon; sensors.json is still watched for a new token.
const RETRY_SLOW: Duration = Duration::from_secs(60);
const EVENT_WAIT: Duration = Duration::from_millis(100);
/// sensors.json is read this often, to follow changes from Studio.
const CONFIG_EVERY: Duration = Duration::from_secs(5);
/// Who sits where is asked again this often, in case an event was missed
/// (Discord does not send every channel's events to every client).
const RESYNC_EVERY: Duration = Duration::from_secs(20);
/// The channel list of a server is read again this often.
const CHANNELS_EVERY: Duration = Duration::from_secs(300);
/// Voice channels followed per server.
const MAX_CHANNELS: usize = 30;
/// How long a notification waits for its picture and for the desktop's
/// do-not-disturb answer before it goes out without them.
const NOTIFICATION_WAIT: Duration = Duration::from_millis(1_500);
/// How long after a message over RPC its desktop copy can still arrive.
const COPY_WINDOW: Duration = Duration::from_secs(5);
/// A session shorter than this counts as a failed attempt to connect.
const STEADY_SESSION: Duration = Duration::from_secs(60);
/// Push to talk released while Discord was out of reach: the microphone is
/// muted again when Discord comes back within this time.
const REMUTE_WITHIN: Duration = Duration::from_secs(600);
/// Notifications waiting for the engine; older ones go when it falls behind.
const MAX_NOTIFICATIONS: usize = 8;
/// Percent per press of the volume buttons.
const VOLUME_STEP: u8 = 10;
const NOT_CONNECTED: &str = "Discord is not connected";

type LoadApp = Box<dyn Fn() -> Option<DiscordApp> + Send>;

/// What the strip needs from Discord.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Wanted {
    /// Servers named by voice widgets.
    guilds: BTreeSet<String>,
    /// A voice or call widget follows the server of the channel you are in.
    current_guild: bool,
    /// Discord's notifications are shown.
    notifications: bool,
}

/// What a button, slider or tap asks of Discord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    Command {
        command: DiscordCommand,
        channel: Option<String>,
        step: Option<u8>,
    },
    /// Mutes or opens your microphone (push to talk).
    SetMute(bool),
    InputVolume(u8),
    OutputVolume(u16),
    /// How loud you hear someone, 0..=200 %.
    UserVolume {
        user: String,
        volume: u16,
    },
    /// Mutes someone for you.
    UserMute {
        user: String,
        mute: bool,
    },
}

struct Command {
    label: String,
    request: Request,
}

/// A message notification from Discord, ready for the strip.
pub(crate) struct DiscordNotification {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) icon: Option<Arc<Pixmap>>,
    /// Opens the conversation in Discord.
    pub(crate) link: Option<String>,
    /// A direct or group message, not one in a server.
    pub(crate) direct: bool,
}

/// Message texts Discord sent over RPC a moment ago. Only a desktop
/// notification that repeats one of them is a copy: the rest (calls, friend
/// requests, a Discord that was not asked) must still show.
#[derive(Default)]
pub(crate) struct RecentMessages(VecDeque<(Instant, String)>);

impl RecentMessages {
    pub(crate) fn remember(&mut self, body: &str, now: Instant) {
        self.forget_old(now);
        if self.0.len() >= MAX_NOTIFICATIONS {
            self.0.pop_front();
        }
        self.0.push_back((now, body.trim().to_string()));
    }

    /// Whether a desktop notification with this text is a copy of a message
    /// that already showed.
    pub(crate) fn repeats(&mut self, body: &str, now: Instant) -> bool {
        self.forget_old(now);
        let body = body.trim();
        !body.is_empty() && self.0.iter().any(|(_, text)| text == body)
    }

    fn forget_old(&mut self, now: Instant) {
        self.0
            .retain(|(at, _)| now.saturating_duration_since(*at) < COPY_WINDOW);
    }
}

#[derive(Default)]
struct Shared {
    status: Mutex<Status>,
    wake: Condvar,
}

#[derive(Default)]
struct Status {
    wanted: Option<Wanted>,
    stop: bool,
    configured: bool,
    /// Set while signed in.
    info: Option<DiscordInfo>,
    problem: Option<String>,
    notifications: VecDeque<DiscordNotification>,
    /// Discord sends its notifications here; the desktop copies of them
    /// can be left out.
    notifying: bool,
    /// Mute the microphone on the next connection, until this time.
    remute_until: Option<Instant>,
}

pub(crate) struct Discord {
    shared: Arc<Shared>,
    commands: Option<Sender<Command>>,
    /// `--dry-run-actions`: commands are logged, not sent.
    dry_run: Option<nexus_events::EventLogger>,
}

impl Discord {
    /// No worker: Discord buttons report that Discord is not connected.
    pub(crate) fn off() -> Self {
        Self {
            shared: Arc::default(),
            commands: None,
            dry_run: None,
        }
    }

    pub(crate) fn on(
        feedback: Sender<Feedback>,
        logger: nexus_events::EventLogger,
        dry_run: bool,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let worker = Worker {
            shared: shared.clone(),
            commands: rx,
            feedback,
            logger: logger.clone(),
            load: Box::new(|| nexus_sensors::load_sensor_config().discord),
            images: images::Images::start(),
        };
        let started = thread::Builder::new()
            .name("discord".into())
            .spawn(move || worker.run());
        Self {
            shared,
            commands: started.is_ok().then_some(tx),
            dry_run: dry_run.then_some(logger),
        }
    }

    /// Connects while something is wanted, and lets go of Discord otherwise.
    pub(crate) fn set_wanted(&self, wanted: Option<Wanted>) {
        let mut status = self.lock();
        if status.wanted != wanted {
            status.wanted = wanted;
            self.shared.wake.notify_all();
        }
    }

    /// Queues a request; refused while Discord is not connected.
    pub(crate) fn send(&self, label: &str, request: Request) -> Result<(), String> {
        let (connected, configured, problem) = {
            let status = self.lock();
            let configured = status.configured && status.wanted.is_some();
            (status.info.is_some(), configured, status.problem.clone())
        };
        if !connected {
            return Err(match problem {
                Some(problem) => problem,
                None if configured => "Connecting to Discord".to_string(),
                None => format!("{NOT_CONNECTED}; set it up in Studio under Connections"),
            });
        }
        if let Some(logger) = &self.dry_run {
            log_fields(
                logger,
                Level::Info,
                "action_dry_run",
                "action executed",
                [
                    ("widget", label.to_string()),
                    ("command", format!("discord {request:?}")),
                ],
            );
            return Ok(());
        }
        let sender = self.commands.as_ref().ok_or(NOT_CONNECTED)?;
        sender
            .send(Command {
                label: label.to_string(),
                request,
            })
            .map_err(|_| NOT_CONNECTED.to_string())
    }

    /// Push to talk ended: mutes the microphone it opened. Discord keeps the
    /// call when only this connection is lost, so an unreachable Discord
    /// must not leave the microphone open once it is back.
    pub(crate) fn mute_again(&self) {
        if self.send("push-to-talk", Request::SetMute(true)).is_err() {
            self.lock().remute_until = Some(Instant::now() + REMUTE_WITHIN);
        }
    }

    /// Discord's state for the strip.
    pub(crate) fn info(&self) -> Option<DiscordInfo> {
        self.lock().info.clone()
    }

    /// Notifications that arrived since the last call.
    pub(crate) fn take_notifications(&self) -> Vec<DiscordNotification> {
        self.lock().notifications.drain(..).collect()
    }

    #[cfg(test)]
    pub(crate) fn inject_notification(&self, notification: DiscordNotification) {
        self.lock().notifications.push_back(notification);
    }

    /// Whether Discord's notifications arrive here right now.
    pub(crate) fn notifying(&self) -> bool {
        let status = self.lock();
        status.notifying && status.info.is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Discord {
    fn drop(&mut self) {
        self.lock().stop = true;
        self.shared.wake.notify_all();
    }
}

/// What the strip needs from Discord with `profile` active; None when
/// nothing.
pub(crate) fn wanted(config: &NexusConfig, profile: &Profile) -> Option<Wanted> {
    let settings = &config.settings.discord;
    let follows_voice = config.profiles.iter().any(|p| p.discord_voice);
    let notifications = settings.notifications && config.settings.notifications.enabled;
    let mut wanted = profile_wants_discord(profile);
    if follows_voice || settings.mute_indicator || notifications {
        wanted.get_or_insert_with(Wanted::default);
    }
    if let Some(wanted) = wanted.as_mut() {
        wanted.notifications = notifications;
    }
    wanted
}

/// What the profile shows of Discord or does with it; None when nothing.
pub(crate) fn profile_wants_discord(profile: &Profile) -> Option<Wanted> {
    fn controls_discord(action: &ActionSpec) -> bool {
        match action {
            ActionSpec::Discord { .. } => true,
            ActionSpec::Sequence { steps, .. } => steps.iter().any(controls_discord),
            _ => false,
        }
    }
    let mut wanted = Wanted::default();
    let mut used = false;
    for widget in profile.pages.iter().flat_map(|page| &page.widgets) {
        match &widget.kind {
            WidgetKind::DiscordVoice(props) => {
                used = true;
                match &props.guild_id {
                    Some(guild) => {
                        wanted.guilds.insert(guild.clone());
                    }
                    None => wanted.current_guild = true,
                }
            }
            WidgetKind::DiscordCall(_) => {
                used = true;
                wanted.current_guild = true;
            }
            WidgetKind::Volume(props) if props.source.is_discord() => used = true,
            _ => {}
        }
        used |= [&widget.tap, &widget.double_tap, &widget.hold]
            .into_iter()
            .flatten()
            .any(controls_discord);
    }
    used.then_some(wanted)
}

#[cfg(test)]
mod tests;
