//! The daemon's brain: turns touches, API requests and fresh data into page
//! changes, actions and frames. Hardware and emulator loops drive it.

use crate::dispatch::{Dispatcher, Feedback, Job};
use crate::ipc::ControlRequest;
use crate::live::{LiveBuilder, ProviderHandle, Wants};
use crate::log::{log_fields, Level};
use crate::metrics_cache::MetricsHandle;
use crate::news::{self, Pacing, SensorState};
use crate::runtime::{Direction, PageChange, Runtime, RuntimeState};
use crate::scene::Scene;
use crate::timers::{self, Timers};
use crate::touch::{Interaction, Target, TouchMachine, WidgetRef};
use base64::Engine as _;
use nexus_config::{
    ActionSpec, BrightnessAction, DiscordCommand, MediaKey, NotificationScroll, VolumeSource,
    Widget, WidgetKind,
};
use nexus_render::{CallHit, Canvas, DiscordInfo, MediaZone, Overlay};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Frame interval while something moves (~50 fps; one frame of HID writes
/// takes ~15 ms).
const ANIMATION_FRAME: Duration = Duration::from_millis(20);
/// Upper bound between frames when nothing moves; readings change at 1 Hz.
const IDLE_FRAME: Duration = Duration::from_millis(500);
const VOLUME_OVERLAY: Duration = Duration::from_millis(1_500);
const TRACK_OVERLAY: Duration = Duration::from_millis(4_000);
const TOAST: Duration = Duration::from_millis(2_500);
/// Our own slider changes the volume; that change should not pop up the
/// volume overlay.
const OWN_VOLUME_QUIET: Duration = Duration::from_secs(2);
/// Shortest time a notification stays, so it can be read.
const NOTIFICATION_MIN: Duration = Duration::from_millis(1500);
/// How long a desktop notification from Discord waits for the same message
/// over RPC, which can arrive just after it.
const DESKTOP_COPY_HOLD: Duration = Duration::from_millis(1_800);
/// A Discord call widget closes someone's controls after this long
/// without a touch.
const DISCORD_FOCUS: Duration = Duration::from_secs(8);
/// Voice widgets turn to their next page of channels this often...
const ROTATION_EVERY: Duration = Duration::from_secs(5);
/// ...unless the strip was touched this recently.
const ROTATION_PAUSE: Duration = Duration::from_secs(10);
/// The longest a scrolling notification stays up.
const SCROLL_MAX: Duration = Duration::from_secs(30);
/// Volume change per tap in the call widget's controls.
const DISCORD_USER_STEP: u16 = 10;

pub(crate) struct Engine {
    pub(crate) runtime: Runtime,
    scene: Scene,
    touch: TouchMachine,
    news: SensorState,
    timers: Timers,
    live: LiveBuilder,
    metrics: MetricsHandle,
    providers: ProviderHandle,
    dispatcher: Dispatcher,
    feedback: Receiver<Feedback>,
    notifications: crate::notifications::Notifications,
    obs: crate::obs::Obs,
    discord: crate::discord::Discord,
    focus: crate::focus::Focus,
    /// Profile to return to when the app that switched profiles loses focus.
    auto_return_profile: Option<String>,
    /// Profile to return to when you leave Discord voice.
    voice_return_profile: Option<String>,
    /// Whether you sat in a Discord voice channel at the last tick.
    in_voice: bool,
    /// The member a Discord call widget shows controls for, and when they
    /// were last touched.
    discord_focus: Option<(String, Instant)>,
    /// A push-to-talk button opened the microphone; it mutes again on
    /// release.
    talking: bool,
    /// Messages Discord sent lately, to recognize their desktop copies.
    recent_discord: crate::discord::RecentMessages,
    /// Discord's desktop notifications waiting for the original.
    held_desktop: Vec<(Instant, crate::notifications::Prepared)>,
    /// Which page voice widgets show, and when it last turned.
    voice_rotation: u64,
    voice_rotated: Instant,
    /// A touch calibration in progress: the strip shows its marks and the
    /// panel's touches go to it instead of the widgets.
    calibration: Option<crate::calibration::Calibration>,
    /// How the last calibration ended, for Studio.
    calibration_ended: nexus_ipc::CalibrationStatus,
    logger: nexus_events::EventLogger,
    state_path: Option<PathBuf>,
    /// Actions of a sequence waiting for their delay: (due, sequence,
    /// widget, action).
    scheduled: Vec<(Instant, u64, String, ActionSpec)>,
    /// Numbers sequences, so a step can keep the rest of its own.
    sequences: u64,
    last_frame: Option<Vec<u8>>,
    last_volume: Option<(u8, bool)>,
    own_volume_until: Instant,
    last_track: Option<String>,
    wants_checked: Option<Instant>,
    /// Touch began while the strip was asleep: swallow it until release.
    waking: bool,
    pub(crate) mode: nexus_ipc::DaemonMode,
    pub(crate) device: nexus_ipc::DeviceStatus,
}

pub(crate) struct EngineParts {
    pub(crate) runtime: Runtime,
    pub(crate) metrics: MetricsHandle,
    pub(crate) providers: ProviderHandle,
    pub(crate) dispatcher: Dispatcher,
    pub(crate) feedback: Receiver<Feedback>,
    pub(crate) notifications: crate::notifications::Notifications,
    pub(crate) obs: crate::obs::Obs,
    pub(crate) discord: crate::discord::Discord,
    pub(crate) focus: crate::focus::Focus,
    pub(crate) logger: nexus_events::EventLogger,
    pub(crate) state_path: Option<PathBuf>,
    pub(crate) mode: nexus_ipc::DaemonMode,
}

impl Engine {
    pub(crate) fn new(parts: EngineParts) -> Self {
        Self {
            runtime: parts.runtime,
            scene: Scene::default(),
            touch: TouchMachine::default(),
            news: SensorState::default(),
            timers: Timers::default(),
            live: LiveBuilder::default(),
            metrics: parts.metrics,
            providers: parts.providers,
            dispatcher: parts.dispatcher,
            feedback: parts.feedback,
            notifications: parts.notifications,
            obs: parts.obs,
            discord: parts.discord,
            focus: parts.focus,
            auto_return_profile: None,
            voice_return_profile: None,
            in_voice: false,
            discord_focus: None,
            talking: false,
            recent_discord: crate::discord::RecentMessages::default(),
            held_desktop: Vec::new(),
            voice_rotation: 0,
            voice_rotated: Instant::now(),
            calibration: None,
            calibration_ended: nexus_ipc::CalibrationStatus::default(),
            logger: parts.logger,
            state_path: parts.state_path,
            scheduled: Vec::new(),
            sequences: 0,
            last_frame: None,
            last_volume: None,
            own_volume_until: Instant::now(),
            last_track: None,
            wants_checked: None,
            waking: false,
            mode: parts.mode,
            device: nexus_ipc::DeviceStatus::default(),
        }
    }

    // ---- touch -----------------------------------------------------------

    pub(crate) fn touch_event(&mut self, phase: nexus_ipc::TouchPhase, x: u16, now: Instant) {
        use nexus_ipc::TouchPhase::*;
        if phase == Began {
            // A new touch means the last one lifted, even if its release
            // never arrived.
            self.waking = false;
            let asleep = self.runtime.touch_wake_blank
                || (self.runtime.idle_dimmed(now) && self.runtime.settings().idle.wake_touch_only);
            self.runtime.last_touch = now;
            if asleep {
                // The first touch only wakes the strip.
                self.runtime.touch_wake_blank = false;
                self.waking = true;
                return;
            }
            let link = self.scene.overlay_link(now);
            if self.scene.dismiss_overlay(now) {
                if let Some(link) = link {
                    self.queue("discord-notification", nexus_actions::Action::OpenUrl(link));
                }
                self.waking = true;
                return;
            }
        }
        if self.waking {
            if phase == Ended {
                self.waking = false;
            }
            return;
        }
        self.runtime.last_touch = now;
        let interactions = match phase {
            Began => {
                let target = self.target_at(x);
                if self.news_widget_at(x).is_some() {
                    self.news.touch_began(now);
                }
                self.touch.hold = self.news.hold_threshold();
                self.touch.began(x, target, now)
            }
            Moved => self.touch.moved(x, now),
            Ended => self.touch.ended(x, now),
        };
        for interaction in interactions {
            self.interact(interaction, now);
        }
    }

    fn target_at(&self, x: u16) -> Target {
        let page = self.runtime.page();
        let Some((idx, widget)) = page.widgets.iter().enumerate().find(|(_, w)| w.contains(x))
        else {
            return Target::default();
        };
        let slider = (matches!(widget.kind, WidgetKind::Volume(_))
            && widget.tap.is_none()
            && nexus_render::volume_fraction_at(widget.x, widget.w, x).is_some())
        .then_some((widget.x, widget.x.saturating_add(widget.w)));
        let double_tap = widget.double_tap.is_some()
            || (widget.tap.is_none()
                && matches!(widget.kind, WidgetKind::News(_) | WidgetKind::Timer(_)));
        Target {
            widget: Some(WidgetRef {
                index: idx,
                generation: self.runtime.generation,
            }),
            slider,
            double_tap,
            repeat: widget.repeat && (widget.hold.is_some() || widget.tap.is_some()),
        }
    }

    fn news_widget_at(&self, x: u16) -> Option<&Widget> {
        self.runtime
            .page()
            .widgets
            .iter()
            .find(|w| w.contains(x) && matches!(w.kind, WidgetKind::News(_)))
    }

    /// The widget a touch refers to, unless the page, profile or config
    /// changed since the finger went down.
    fn widget(&self, widget: Option<WidgetRef>) -> Option<Widget> {
        let widget = widget.filter(|w| w.generation == self.runtime.generation)?;
        self.runtime.page().widgets.get(widget.index).cloned()
    }

    /// Drops the touch in progress without firing anything: after a page,
    /// profile or config change it would hit other widgets, and a lost
    /// device never sends the release.
    pub(crate) fn cancel_touch(&mut self, now: Instant) {
        self.stop_talking();
        self.touch.reset();
        self.waking = false;
        self.news.end_touch();
        self.scene.release(now);
        self.scene.slide(None);
    }

    /// Timer state is kept per profile and page: widget ids are only unique
    /// per page.
    fn timer_key(&self, widget: &Widget) -> String {
        timers::key(
            &self.runtime.profile().id,
            &self.runtime.page().id,
            &widget.id,
        )
    }

    fn interact(&mut self, interaction: Interaction, now: Instant) {
        match interaction {
            Interaction::Press { widget, .. } => {
                let widget = self.widget(widget);
                if widget.as_ref().is_some_and(is_push_to_talk) {
                    self.start_talking(now);
                }
                let id = widget.filter(reacts_to_press).map(|w| w.id);
                self.scene.press(id, now);
            }
            Interaction::Release => {
                self.stop_talking();
                self.scene.release(now);
            }
            Interaction::Tap { widget, x } => {
                if let Some(widget) = self.widget(widget) {
                    self.tap(&widget, x, now);
                }
                self.news.end_touch();
            }
            Interaction::DoubleTap { widget, x } => {
                if let Some(widget) = self.widget(widget) {
                    self.double_tap(&widget, x, now);
                }
                self.news.end_touch();
            }
            Interaction::Hold { widget, x } => {
                if let Some(widget) = self.widget(widget) {
                    self.hold(&widget, x, now);
                }
                self.news.end_touch();
            }
            Interaction::Slide { widget, x, done } => {
                let Some(widget) = self.widget(Some(widget)) else {
                    self.scene.slide(None);
                    return;
                };
                // The finger is captured: past the left end of the track is
                // the bottom of the slider, not a press on the speaker icon.
                let fraction =
                    nexus_render::volume_fraction_at(widget.x, widget.w, x).unwrap_or(0.0);
                if done {
                    self.scene.slide(None);
                    let source = match &widget.kind {
                        WidgetKind::Volume(props) => props.source,
                        _ => VolumeSource::System,
                    };
                    let request = match source {
                        VolumeSource::System => None,
                        VolumeSource::DiscordInput => Some(crate::discord::Request::InputVolume(
                            (fraction * 100.0).round() as u8,
                        )),
                        VolumeSource::DiscordOutput => Some(crate::discord::Request::OutputVolume(
                            (fraction * 200.0).round() as u16,
                        )),
                    };
                    if let Some(request) = request {
                        self.discord_request(&widget.id, request, now);
                        return;
                    }
                    let percent = (fraction * 100.0).round() as u8;
                    self.own_volume_until = now + OWN_VOLUME_QUIET;
                    self.queue(
                        &widget.id,
                        nexus_actions::Action::Volume(nexus_actions::VolumeCommand::Set(percent)),
                    );
                    self.metrics.request_refresh();
                } else {
                    self.scene.slide(Some((widget.id.clone(), fraction)));
                }
            }
            Interaction::SlideCancel => self.scene.slide(None),
            Interaction::Drag { dx } => self.scene.drag(dx, now),
            Interaction::DragEnd { commit, .. } => match commit {
                Some(forward) => {
                    let direction = if forward {
                        Direction::Forward
                    } else {
                        Direction::Backward
                    };
                    match self.runtime.step(direction) {
                        Some(change) => self.page_changed(change, "swipe", now),
                        None => self.scene.snap_back(now),
                    }
                }
                None => self.scene.snap_back(now),
            },
        }
    }

    fn tap(&mut self, widget: &Widget, x: u16, now: Instant) {
        if let Some(action) = widget.tap.clone() {
            self.perform(&widget.id, &action, now);
            return;
        }
        match &widget.kind {
            WidgetKind::Media(props) => {
                let command = match nexus_render::media_zone(widget.x, widget.w, props, x) {
                    MediaZone::Previous => MediaKey::Previous,
                    MediaZone::Next => MediaKey::Next,
                    MediaZone::Info | MediaZone::PlayPause => MediaKey::PlayPause,
                };
                let player = self
                    .metrics
                    .latest()
                    .media
                    .as_ref()
                    .map(|m| m.bus_name.clone())
                    .or_else(|| props.player.clone());
                self.perform(&widget.id, &ActionSpec::Media { command, player }, now);
                self.metrics.request_refresh();
            }
            WidgetKind::Volume(props) if props.source.is_discord() => {
                // The icon mutes the microphone, or deafens.
                let command = if props.source == VolumeSource::DiscordInput {
                    DiscordCommand::ToggleMute
                } else {
                    DiscordCommand::ToggleDeafen
                };
                self.discord_request(
                    &widget.id,
                    crate::discord::Request::Command {
                        command,
                        channel: None,
                        step: None,
                    },
                    now,
                );
            }
            WidgetKind::Volume(_) => {
                // Taps on the speaker icon (outside the slider) toggle mute.
                self.queue(
                    &widget.id,
                    nexus_actions::Action::Volume(nexus_actions::VolumeCommand::ToggleMute),
                );
                self.own_volume_until = now + OWN_VOLUME_QUIET;
                self.metrics.request_refresh();
            }
            WidgetKind::News(_) => self.open_headline(widget, x, now),
            WidgetKind::Timer(_) => {
                let key = self.timer_key(widget);
                self.timers.tap(&key, now);
            }
            WidgetKind::Entity(props) => self.toggle_entity(&widget.id, &props.entity_id),
            WidgetKind::DiscordVoice(props) => {
                let Some(mut info) = self.discord.info() else {
                    return;
                };
                info.rotation = self.voice_rotation;
                let channel = nexus_render::discord_channel_at(widget.x, widget.w, props, &info, x);
                if channel.is_some() && channel == info.channel {
                    let text = nexus_render::format::word(
                        nexus_render::format::Word::AlreadyInChannel,
                        self.locale(),
                    );
                    self.toast("check", text.to_string(), now);
                } else if let Some(channel) = channel {
                    let join = ActionSpec::Discord {
                        command: DiscordCommand::JoinVoice,
                        channel: Some(channel),
                        step: None,
                    };
                    self.perform(&widget.id, &join, now);
                }
            }
            WidgetKind::DiscordCall(props) => self.tap_call(widget, props, x, now),
            _ => {}
        }
    }

    /// A tap in the call widget: open someone's controls, or use them.
    fn tap_call(
        &mut self,
        widget: &Widget,
        props: &nexus_config::DiscordCallProps,
        x: u16,
        now: Instant,
    ) {
        let Some(mut info) = self.discord.info() else {
            return;
        };
        info.focus = self.discord_focus(now);
        let Some(hit) = nexus_render::discord_call_hit(widget.x, widget.w, props, &info, x) else {
            return;
        };
        let member = |id: &str| {
            info.current_channel()
                .and_then(|channel| channel.members.iter().find(|m| m.user_id == id))
                .cloned()
        };
        // Discord sets no volume of your own for you: your controls are
        // your microphone's.
        let yourself = |id: &str| info.self_id.as_deref() == Some(id);
        let microphone = |delta: i16| {
            let volume = (i16::from(info.input_volume) + delta).clamp(0, 100);
            crate::discord::Request::InputVolume(volume as u8)
        };
        let toggle_mute = || crate::discord::Request::Command {
            command: DiscordCommand::ToggleMute,
            channel: None,
            step: None,
        };
        let step = DISCORD_USER_STEP as i16;
        let request = match hit {
            // Deafened, the red button means Discord has you muted anyway.
            CallHit::Microphone if info.self_deafened => Some(crate::discord::Request::Command {
                command: DiscordCommand::ToggleDeafen,
                channel: None,
                step: None,
            }),
            CallHit::Microphone => Some(toggle_mute()),
            CallHit::Mute(id) if yourself(&id) => Some(toggle_mute()),
            CallHit::Quieter(id) if yourself(&id) => Some(microphone(-step)),
            CallHit::Louder(id) if yourself(&id) => Some(microphone(step)),
            CallHit::Member(id) => {
                self.discord_focus = Some((id, now));
                return;
            }
            CallHit::Close => {
                self.discord_focus = None;
                return;
            }
            CallHit::Mute(id) => member(&id).map(|m| crate::discord::Request::UserMute {
                mute: !m.local_muted,
                user: id,
            }),
            CallHit::Quieter(id) => member(&id).map(|m| crate::discord::Request::UserVolume {
                volume: m.volume.saturating_sub(DISCORD_USER_STEP),
                user: id,
            }),
            CallHit::Louder(id) => member(&id).map(|m| crate::discord::Request::UserVolume {
                volume: (m.volume + DISCORD_USER_STEP).min(200),
                user: id,
            }),
        };
        if let Some((_, touched)) = self.discord_focus.as_mut() {
            *touched = now;
        }
        if let Some(request) = request {
            self.discord_request(&widget.id, request, now);
        }
    }

    /// The member whose controls are open, unless they timed out.
    fn discord_focus(&mut self, now: Instant) -> Option<String> {
        if self
            .discord_focus
            .as_ref()
            .is_some_and(|(_, touched)| now.saturating_duration_since(*touched) >= DISCORD_FOCUS)
        {
            self.discord_focus = None;
        }
        self.discord_focus.as_ref().map(|(id, _)| id.clone())
    }

    fn discord_request(&mut self, label: &str, request: crate::discord::Request, now: Instant) {
        if let Err(error) = self.discord.send(label, request) {
            self.toast("alert-triangle", error, now);
        }
    }

    /// A push-to-talk button went down: open the microphone if it is muted.
    fn start_talking(&mut self, now: Instant) {
        let muted = self
            .discord
            .info()
            .is_some_and(|info| info.channel.is_some() && info.self_muted);
        if muted && !self.talking {
            self.talking = true;
            self.discord_request("push-to-talk", crate::discord::Request::SetMute(false), now);
        }
    }

    /// The finger lifted: mute again what push to talk opened.
    fn stop_talking(&mut self) {
        if std::mem::take(&mut self.talking) {
            self.discord.mute_again();
        }
    }

    fn double_tap(&mut self, widget: &Widget, x: u16, now: Instant) {
        if let Some(action) = widget.double_tap.clone() {
            self.perform(&widget.id, &action, now);
            return;
        }
        match &widget.kind {
            WidgetKind::News(_) => {
                // Left half steps back a pair, right half forward.
                let pairs = if x < widget.x + widget.w / 2 { -1 } else { 1 };
                let cards = self.metrics.latest().cards.clone();
                self.news.scrub(&cards, pairs, now);
            }
            WidgetKind::Timer(props) => {
                let key = self.timer_key(widget);
                self.timers.skip(&key, props);
            }
            _ => self.tap(widget, x, now),
        }
    }

    fn hold(&mut self, widget: &Widget, x: u16, now: Instant) {
        if let Some(action) = widget.hold.clone() {
            self.perform(&widget.id, &action, now);
            return;
        }
        if widget.repeat {
            if let Some(action) = widget.tap.clone() {
                self.perform(&widget.id, &action, now);
            }
            return;
        }
        match &widget.kind {
            WidgetKind::News(_) => self.dismiss_headline(widget, x, now),
            WidgetKind::Timer(_) => {
                let key = self.timer_key(widget);
                self.timers.reset(&key);
            }
            _ => {}
        }
    }

    fn open_headline(&mut self, widget: &Widget, x: u16, now: Instant) {
        let snapshot = self.metrics.latest();
        let cards = self.news.touch_targets(&snapshot.cards, now);
        let Some(slot) = news::slot_for_x(&cards, widget.x, widget.w, x) else {
            return;
        };
        let card = cards[slot].clone();
        let Some(url) = card.url.clone().filter(|_| card.is_actionable()) else {
            return;
        };
        self.queue(
            &format!("news-{}", card.source),
            nexus_actions::Action::OpenUrl(url.clone()),
        );
        if card.module == "rss" && nexus_sensors::mark_rss_item_seen(&card.id).is_ok() {
            self.metrics.update_cards(|cards| {
                if let Some(seen) = cards.iter_mut().find(|other| other.id == card.id) {
                    seen.status = Some("SEEN".into());
                }
            });
            self.metrics.request_refresh();
        }
        self.news.acknowledge_tap(&cards, slot, now);
        log_fields(
            &self.logger,
            Level::Info,
            "news_opened",
            "tap opened a headline",
            [
                ("source", card.source.clone()),
                ("title", card.title.clone()),
                ("url", url),
            ],
        );
    }

    fn dismiss_headline(&mut self, widget: &Widget, x: u16, now: Instant) {
        let snapshot = self.metrics.latest();
        let cards = self.news.touch_targets(&snapshot.cards, now);
        let Some(slot) = news::slot_for_x(&cards, widget.x, widget.w, x) else {
            return;
        };
        let card = cards[slot].clone();
        let removed = match card.module.as_str() {
            "rss" => nexus_sensors::dismiss_rss_item(&card.id).is_ok(),
            "webhook" => nexus_sensors::delete_webhook_record(&card.source).is_ok(),
            _ => false,
        };
        if removed {
            self.metrics
                .update_cards(|cards| cards.retain(|other| other.id != card.id));
            self.metrics.request_refresh();
            log_fields(
                &self.logger,
                Level::Info,
                "news_dismissed",
                "hold hid a headline",
                [
                    ("source", card.source.clone()),
                    ("title", card.title.clone()),
                ],
            );
        }
    }

    fn toggle_entity(&mut self, label: &str, entity_id: &str) {
        let Some((domain, service)) = nexus_sensors::toggle_service(entity_id) else {
            return;
        };
        let job = Job::HomeAssistant {
            label: label.to_string(),
            domain: domain.into(),
            service: service.into(),
            entity_id: Some(entity_id.to_string()),
        };
        if self.dispatcher.dispatch(job).is_ok() && service == "toggle" {
            self.providers.toggled(entity_id);
        }
    }

    // ---- actions ---------------------------------------------------------

    fn queue(&mut self, label: &str, action: nexus_actions::Action) {
        let job = Job::Action {
            label: label.to_string(),
            action,
        };
        if let Err(error) = self.dispatcher.dispatch(job) {
            self.toast("alert-triangle", error.to_string(), Instant::now());
        }
    }

    /// Runs `action` for widget `label`: page and device actions here, the
    /// rest on the action worker.
    pub(crate) fn perform(&mut self, label: &str, action: &ActionSpec, now: Instant) {
        match action {
            ActionSpec::NextPage => {
                if let Some(change) = self.runtime.step(Direction::Forward) {
                    self.page_changed(change, label, now);
                }
            }
            ActionSpec::PreviousPage => {
                if let Some(change) = self.runtime.step(Direction::Backward) {
                    self.page_changed(change, label, now);
                }
            }
            ActionSpec::GoToPage { page_id } => match self.runtime.go_to_page(page_id) {
                Ok(Some(change)) => self.page_changed(change, label, now),
                Ok(None) => {}
                Err(error) => self.toast("alert-triangle", error.to_string(), now),
            },
            ActionSpec::GoBack => {
                if let Some(change) = self.runtime.go_back() {
                    self.page_changed(change, label, now);
                }
            }
            ActionSpec::SwitchProfile { profile_id } => {
                if let Err(error) = self.switch_profile(profile_id, now) {
                    self.toast("alert-triangle", error.to_string(), now);
                }
            }
            ActionSpec::NextProfile | ActionSpec::PreviousProfile => {
                let direction = if matches!(action, ActionSpec::NextProfile) {
                    Direction::Forward
                } else {
                    Direction::Backward
                };
                let id = self.runtime.neighbour_profile(direction).to_string();
                if id != self.runtime.profile().id {
                    if let Err(error) = self.switch_profile(&id, now) {
                        self.toast("alert-triangle", error.to_string(), now);
                    }
                }
            }
            ActionSpec::Brightness { command, percent } => {
                let current = self.runtime.brightness;
                self.runtime.brightness = match command {
                    BrightnessAction::Up => current.saturating_add(10).min(100),
                    BrightnessAction::Down => current.saturating_sub(10).max(5),
                    BrightnessAction::Set => percent.unwrap_or(current).min(100),
                    BrightnessAction::Cycle => match current {
                        0..=24 => 25,
                        25..=49 => 50,
                        50..=74 => 75,
                        75..=99 => 100,
                        _ => 25,
                    },
                };
                self.toast("sun", format!("{} %", self.runtime.brightness), now);
                self.persist();
            }
            ActionSpec::DisplayOff => self.runtime.touch_wake_blank = true,
            ActionSpec::HomeAssistant { service, entity_id } => {
                let (domain, service) = service
                    .split_once('.')
                    .unwrap_or(("homeassistant", service));
                let job = Job::HomeAssistant {
                    label: label.to_string(),
                    domain: domain.into(),
                    service: service.into(),
                    entity_id: entity_id.clone(),
                };
                if self.dispatcher.dispatch(job).is_ok() && service == "toggle" {
                    if let Some(entity) = entity_id {
                        self.providers.toggled(entity);
                    }
                }
            }
            ActionSpec::Obs { command, target } => {
                if let Err(error) = self.obs.send(label, *command, target.clone()) {
                    self.toast("alert-triangle", error, now);
                }
            }
            // Push to talk works on press and release, not on the tap.
            ActionSpec::Discord {
                command: DiscordCommand::PushToTalk,
                ..
            } => {}
            ActionSpec::Discord {
                command,
                channel,
                step,
            } => {
                let request = crate::discord::Request::Command {
                    command: *command,
                    channel: channel.clone(),
                    step: *step,
                };
                self.discord_request(label, request, now);
            }
            ActionSpec::Sequence { steps, delay_ms } => {
                self.sequences += 1;
                let sequence = self.sequences;
                for (i, step) in steps.iter().enumerate() {
                    let at = now + Duration::from_millis(u64::from(*delay_ms) * i as u64);
                    if i == 0 || *delay_ms == 0 {
                        self.perform(label, step, now);
                    } else {
                        self.scheduled
                            .push((at, sequence, label.to_string(), step.clone()));
                    }
                }
            }
            external => {
                if let Some(action) = external.to_external_action() {
                    if matches!(action, nexus_actions::Action::Volume(_)) {
                        self.own_volume_until = now + OWN_VOLUME_QUIET;
                    }
                    self.queue(label, action);
                    self.metrics.request_refresh();
                }
            }
        }
    }

    fn switch_profile(&mut self, profile_id: &str, now: Instant) -> anyhow::Result<()> {
        // A deliberate switch ends any app- or voice-driven one.
        self.auto_return_profile = None;
        self.voice_return_profile = None;
        self.change_profile(profile_id, now, true)
    }

    fn change_profile(
        &mut self,
        profile_id: &str,
        now: Instant,
        persist: bool,
    ) -> anyhow::Result<()> {
        let from = self.runtime.page_key();
        self.runtime.switch_profile(profile_id)?;
        // Touches and motion belong to the old profile, and so do the pages
        // that waiting page steps refer to. Other waiting steps keep
        // running: a macro that launches an app may well switch to that
        // app's profile on the way. A sequence that switches profiles
        // itself keeps all of its own steps, see `tick`.
        self.cancel_touch(now);
        self.scene.reset();
        self.scheduled.retain(|(.., action)| !is_page_step(action));
        self.scene.page_changed(
            PageChange {
                from,
                direction: Direction::Forward,
            },
            nexus_config::PageTransition::Fade,
            now,
        );
        self.toast("layers", self.runtime.profile().name.clone(), now);
        if persist {
            self.persist();
        }
        Ok(())
    }

    /// Switches to the profile whose apps include the focused window, and
    /// back to the previous one when focus moves on.
    fn follow_focus(&mut self, window: &nexus_sensors::FocusedWindow, now: Instant) {
        let current = self.runtime.profile().id.clone();
        let target = self
            .runtime
            .config
            .profiles
            .iter()
            .find(|p| p.apps.iter().any(|app| window.matches(app)))
            .map(|p| p.id.clone());
        match target {
            Some(id) if id != current => {
                if self.auto_return_profile.is_none() {
                    self.auto_return_profile = Some(current);
                }
                let _ = self.change_profile(&id, now, false);
                log_fields(
                    &self.logger,
                    Level::Info,
                    "profile_auto",
                    "focused app switched the profile",
                    [("profile", id), ("app", window.app_id.clone())],
                );
            }
            Some(_) => {}
            None => {
                if let Some(previous) = self.auto_return_profile.take() {
                    if previous != current {
                        let _ = self.change_profile(&previous, now, false);
                    }
                }
            }
        }
    }

    /// Switches to the profile that follows Discord voice when you join a
    /// voice channel, and back when you leave.
    fn follow_voice(&mut self, in_voice: bool, now: Instant) {
        if in_voice == std::mem::replace(&mut self.in_voice, in_voice) {
            return;
        }
        let current = self.runtime.profile().id.clone();
        let voice = self
            .runtime
            .config
            .profiles
            .iter()
            .find(|p| p.discord_voice)
            .map(|p| p.id.clone());
        let target = if in_voice {
            match voice {
                Some(id) if id != current => {
                    self.voice_return_profile = Some(current);
                    id
                }
                _ => return,
            }
        } else {
            match self.voice_return_profile.take() {
                // Only when the voice profile still shows: a switch since
                // then was the user's.
                Some(previous) if voice.as_deref() == Some(current.as_str()) => previous,
                _ => return,
            }
        };
        if self.change_profile(&target, now, false).is_ok() {
            log_fields(
                &self.logger,
                Level::Info,
                "profile_auto",
                "Discord voice switched the profile",
                [("profile", target), ("in_voice", in_voice.to_string())],
            );
        }
    }

    fn page_changed(&mut self, change: PageChange, cause: &str, now: Instant) {
        let style = self.runtime.settings().transition;
        self.cancel_touch(now);
        self.scene.page_changed(change, style, now);
        log_fields(
            &self.logger,
            Level::Info,
            "page_changed",
            "page changed",
            [
                ("page", self.runtime.page().id.clone()),
                ("cause", cause.to_string()),
            ],
        );
        self.persist();
    }

    fn toast(&mut self, icon: &str, text: String, now: Instant) {
        self.scene.show_overlay(
            Overlay::Toast {
                icon: icon.to_string(),
                text,
            },
            TOAST,
            now,
        );
    }

    fn persist(&self) {
        if let Some(path) = &self.state_path {
            if let Err(error) = RuntimeState::capture(&self.runtime).save(path) {
                eprintln!("failed to save runtime state: {error:#}");
            }
        }
    }

    // ---- periodic work ---------------------------------------------------

    /// Puts new notifications on the strip. Discord's come first, so a
    /// desktop copy of one can be recognized.
    fn poll_notifications(&mut self, now: Instant) {
        let settings = self.runtime.settings().clone();
        let shown = &settings.notifications;
        let muted = |name: &str| {
            shown
                .muted_apps
                .iter()
                .any(|muted| name.eq_ignore_ascii_case(muted.trim()))
        };
        let longest = Duration::from_millis(u64::from(shown.duration_ms)).max(NOTIFICATION_MIN);
        let show_discord = shown.enabled
            && settings.discord.notifications
            && !crate::discord::DISCORD_NAMES.iter().any(|name| muted(name));
        for n in self.discord.take_notifications() {
            if !show_discord {
                continue;
            }
            self.recent_discord.remember(&n.body, now);
            // Private: the app's name, nothing of the message or its author.
            let (title, body, icon) = if shown.private {
                ("Discord".to_string(), String::new(), None)
            } else {
                (n.title, n.body, n.icon)
            };
            let scroll = match settings.discord.scroll {
                NotificationScroll::All => true,
                NotificationScroll::Direct => n.direct,
                NotificationScroll::Never => false,
            };
            // A long message stays until it scrolled through and was read.
            let duration = nexus_render::notification_scroll_time(&body)
                .filter(|_| scroll)
                .map_or(longest, |time| time.clamp(longest, SCROLL_MAX));
            self.scene.show_notification(
                Overlay::Notification {
                    app: "Discord".into(),
                    title,
                    body,
                    icon,
                    link: n.link,
                    scroll,
                },
                duration,
                now,
                false,
            );
        }

        // While Discord notifies here, its desktop copies wait a moment for
        // the original, which can arrive just after them.
        let from_discord = show_discord && self.discord.notifying();
        for n in self.notifications.poll(shown.enabled, now, &self.logger) {
            if n.names.iter().any(|name| muted(name)) {
                continue;
            }
            let discord = n
                .names
                .iter()
                .any(|name| crate::discord::DISCORD_NAMES.contains(&name.to_lowercase().as_str()));
            if discord && from_discord {
                self.held_desktop.push((now + DESKTOP_COPY_HOLD, n));
            } else {
                self.show_desktop_notification(n, now);
            }
        }
        let (due, waiting) = std::mem::take(&mut self.held_desktop)
            .into_iter()
            .partition(|(at, _)| now >= *at);
        self.held_desktop = waiting;
        for (_, n) in due {
            if !self.recent_discord.repeats(&n.body, now) {
                self.show_desktop_notification(n, now);
            }
        }
    }

    fn show_desktop_notification(&mut self, n: crate::notifications::Prepared, now: Instant) {
        let shown = &self.runtime.settings().notifications;
        // Private: the app's name and icon, nothing of the message.
        let (title, body, icon) = if shown.private {
            (n.app.clone(), String::new(), n.app_icon)
        } else {
            (n.title, n.body, n.picture.or(n.app_icon))
        };
        // Up to the configured time; shorter when the app asks for less.
        let longest = Duration::from_millis(u64::from(shown.duration_ms)).max(NOTIFICATION_MIN);
        let duration = n
            .timeout
            .map_or(longest, |asked| asked.clamp(NOTIFICATION_MIN, longest));
        self.scene.show_notification(
            Overlay::Notification {
                app: n.app,
                title,
                body,
                icon,
                link: None,
                scroll: false,
            },
            duration,
            now,
            n.replaces,
        );
    }

    /// Housekeeping between frames: scheduled actions, timers, overlays for
    /// external events, auto-return, provider wishes.
    pub(crate) fn tick(&mut self, now: Instant) {
        if let Some(calibration) = self.calibration.as_ref().filter(|c| c.timed_out(now)) {
            let state = if calibration.missed() {
                nexus_ipc::CalibrationState::Failed
            } else {
                nexus_ipc::CalibrationState::Cancelled
            };
            self.calibration = None;
            self.calibration_ended = crate::calibration::ended(state, None);
        }
        if !self.runtime.blanked() && self.runtime.idle_off_due(now) {
            self.runtime.touch_wake_blank = true;
        }
        // Nothing to show while blanked: let the collectors rest.
        self.metrics.set_paused(self.runtime.blanked());
        for interaction in self.touch.poll(now) {
            self.interact(interaction, now);
        }
        // One step at a time: a step that switches profiles drops the
        // waiting page steps of other sequences, but not the rest of its own.
        while let Some(pos) = self.scheduled.iter().position(|(at, ..)| *at <= now) {
            let (_, sequence, label, action) = self.scheduled.remove(pos);
            let (own, others): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scheduled)
                .into_iter()
                .partition(|(_, s, ..)| *s == sequence);
            self.scheduled = others;
            self.perform(&label, &action, now);
            self.scheduled.extend(own);
        }

        // Timers of every profile keep running while another one shows.
        let timer_widgets: Vec<(String, nexus_config::TimerProps)> = self
            .runtime
            .config
            .profiles
            .iter()
            .flat_map(|profile| {
                profile.pages.iter().flat_map(move |page| {
                    page.widgets.iter().filter_map(move |w| match &w.kind {
                        WidgetKind::Timer(props) => {
                            Some((timers::key(&profile.id, &page.id, &w.id), props.clone()))
                        }
                        _ => None,
                    })
                })
            })
            .collect();
        let refs: Vec<(&str, &nexus_config::TimerProps)> = timer_widgets
            .iter()
            .map(|(id, props)| (id.as_str(), props))
            .collect();
        for event in self.timers.tick(&refs, now) {
            let (icon, text) = match (self.locale(), event.kind) {
                (nexus_render::Locale::De, "focus_done") => {
                    ("coffee", "Fokus geschafft – Zeit für eine Pause")
                }
                (nexus_render::Locale::De, "break_done") => {
                    ("brain", "Pause vorbei – weiter geht's")
                }
                (nexus_render::Locale::De, _) => ("alarm-clock", "Timer abgelaufen"),
                (nexus_render::Locale::Fr, "focus_done") => {
                    ("coffee", "Focus terminé – place à la pause")
                }
                (nexus_render::Locale::Fr, "break_done") => {
                    ("brain", "Pause terminée – on s'y remet")
                }
                (nexus_render::Locale::Fr, _) => ("alarm-clock", "Minuteur terminé"),
                (_, "focus_done") => ("coffee", "Focus done – time for a break"),
                (_, "break_done") => ("brain", "Break is over – back to it"),
                _ => ("alarm-clock", "Timer finished"),
            };
            self.scene.show_overlay(
                Overlay::Toast {
                    icon: icon.into(),
                    text: text.into(),
                },
                Duration::from_secs(6),
                now,
            );
        }

        while let Ok(feedback) = self.feedback.try_recv() {
            let text = format!("{}: {}", feedback.label, feedback.error);
            self.toast("alert-triangle", text, now);
        }

        if let Some(window) = self.focus.poll(&self.logger) {
            self.follow_focus(&window, now);
        }

        self.poll_notifications(now);
        let settings = self.runtime.settings().clone();
        let in_voice = self
            .discord
            .info()
            .is_some_and(|info| info.channel.is_some());
        self.follow_voice(in_voice, now);

        let snapshot = self.metrics.latest();
        if let Some(percent) = snapshot.metrics.volume_percent {
            let volume = (percent, snapshot.volume_muted);
            if self.last_volume.is_some_and(|last| last != volume)
                && now >= self.own_volume_until
                && settings.overlays.volume
                && !self.page_shows(|k| matches!(k, WidgetKind::Volume(_)))
            {
                self.scene.show_overlay(
                    Overlay::Volume(nexus_render::VolumeInfo {
                        percent,
                        muted: snapshot.volume_muted,
                    }),
                    VOLUME_OVERLAY,
                    now,
                );
            }
            self.last_volume = Some(volume);
        }
        let track = snapshot
            .media
            .as_ref()
            .filter(|m| m.playing && !m.title.is_empty())
            .map(|m| format!("{}\u{1f}{}", m.title, m.artist));
        if track.is_some() && track != self.last_track {
            if self.last_track.is_some()
                && settings.overlays.now_playing
                && !self.page_shows(|k| matches!(k, WidgetKind::Media(_)))
            {
                // The live data otherwise takes the snapshot in only at the
                // next frame and would still name the previous track.
                self.live.absorb(&snapshot);
                let live = self.live.build(&settings, &self.providers.provided());
                if let Some(media) = live.media {
                    self.scene.show_overlay(
                        Overlay::NowPlaying(Box::new(media)),
                        TRACK_OVERLAY,
                        now,
                    );
                }
            }
            self.last_track = track;
        }

        if self.runtime.page().hidden && self.runtime.auto_return_due(now) {
            if let Some(change) = self.runtime.go_back() {
                self.page_changed(change, "auto_return", now);
            }
        }

        if self
            .wants_checked
            .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1))
        {
            self.wants_checked = Some(now);
            let art = snapshot.media.as_ref().and_then(|m| m.art_url.clone());
            let wants = Wants::for_profile(
                self.runtime.profile(),
                &settings,
                art,
                self.dispatcher.shell_allowed(),
            );
            self.providers.set_wants(wants);
            self.obs
                .set_wanted(crate::obs::profile_uses_obs(self.runtime.profile()));
            self.discord.set_wanted(crate::discord::wanted(
                &self.runtime.config,
                self.runtime.profile(),
            ));
        }
    }

    fn page_shows(&self, kind: impl Fn(&WidgetKind) -> bool) -> bool {
        self.runtime.page().widgets.iter().any(|w| kind(&w.kind))
    }

    fn locale(&self) -> nexus_render::Locale {
        nexus_render::Locale::resolve(&self.runtime.settings().locale)
    }

    // ---- frames ----------------------------------------------------------

    pub(crate) fn frame(&mut self, now: Instant) -> Canvas {
        if let Some(calibration) = &self.calibration {
            let canvas = nexus_render::calibration_frame(
                &self.runtime.profile().theme,
                self.locale(),
                calibration.mark(),
                calibration.step(),
                crate::calibration::Calibration::steps(),
            );
            self.last_frame = Some(canvas.rgba().to_vec());
            return canvas;
        }
        let snapshot = self.metrics.latest();
        self.live.absorb(&snapshot);
        let settings = self.runtime.settings().clone();
        let mut live = self.live.build(&settings, &self.providers.provided());
        live.obs = self.obs.info(now);
        live.discord = self.discord.info();
        let focus = self.discord_focus(now);
        // Voice widgets with more channels than room turn their pages, but
        // not under a finger about to tap.
        if now.saturating_duration_since(self.runtime.last_touch) >= ROTATION_PAUSE
            && now.saturating_duration_since(self.voice_rotated) >= ROTATION_EVERY
        {
            self.voice_rotation = self.voice_rotation.wrapping_add(1);
            self.voice_rotated = now;
        }
        let page = self.runtime.page();
        if let Some(discord) = live.discord.as_mut() {
            discord.focus = focus;
            discord.rotation = self.voice_rotation;
            discord.mic_badge = settings.discord.mute_indicator && wants_mic_badge(discord, page);
        }
        if let Some(widget) = self
            .runtime
            .page()
            .widgets
            .iter()
            .find(|w| matches!(w.kind, WidgetKind::News(_)))
        {
            if let WidgetKind::News(props) = &widget.kind {
                self.news
                    .set_pacing(Pacing::new(&snapshot.display, props.font, widget.w));
            }
            live.news = self.news.frame(&snapshot.cards, now);
        }
        // The renderer looks timers up by widget id, which is only unique
        // per page: the current page goes in last so its timers win.
        let profile = self.runtime.profile();
        let current = self.runtime.page();
        let pages = profile.pages.iter().filter(|p| p.id != current.id);
        for page in pages.chain([current]) {
            for widget in &page.widgets {
                if let WidgetKind::Timer(props) = &widget.kind {
                    let key = timers::key(&profile.id, &page.id, &widget.id);
                    live.timers
                        .insert(widget.id.clone(), self.timers.info(&key, props, now));
                }
            }
        }
        if let Some(media) = &live.media {
            // The cover of a new track usually arrives after its overlay.
            self.scene.refresh_now_playing(media);
        }
        let canvas = self.scene.compose(&self.runtime, &live, now);
        self.last_frame = Some(canvas.rgba().to_vec());
        canvas
    }

    /// When the next frame is due after one rendered at `now`.
    pub(crate) fn next_frame_in(&mut self, now: Instant) -> Duration {
        if self.calibration.is_some() {
            return Duration::from_millis(100);
        }
        if self.runtime.blanked() {
            return Duration::from_millis(250);
        }
        if self.scene.animating(now) {
            return ANIMATION_FRAME;
        }
        let page = self.runtime.page();
        let mut interval = IDLE_FRAME;
        if page
            .widgets
            .iter()
            .any(|w| matches!(w.kind, WidgetKind::News(_)))
            && self.news.animating(now)
        {
            interval = interval.min(self.news.scroll_frame_interval());
        }
        let seconds_visible = self.timers.running()
            || page
                .widgets
                .iter()
                .any(|w| matches!(&w.kind, WidgetKind::Clock(p) if p.show_seconds));
        // OBS buttons follow OBS (and count their running time) promptly.
        let shows_obs = page.widgets.iter().any(|w| {
            matches!(w.tap, Some(ActionSpec::Obs { .. }))
                || matches!(&w.kind, WidgetKind::Text(p) if p.text.contains("{obs."))
        });
        if seconds_visible || shows_obs {
            interval = interval.min(Duration::from_millis(250));
        }
        // Discord buttons and widgets follow who speaks.
        let shows_discord = page.widgets.iter().any(|w| {
            matches!(w.tap, Some(ActionSpec::Discord { .. }))
                || matches!(
                    &w.kind,
                    WidgetKind::DiscordVoice(_) | WidgetKind::DiscordCall(_)
                )
                || matches!(&w.kind, WidgetKind::Volume(p) if p.source.is_discord())
        });
        if shows_discord {
            interval = interval.min(Duration::from_millis(150));
        }
        if let Some(deadline) = self.touch.next_deadline() {
            interval = interval.min(
                deadline
                    .saturating_duration_since(now)
                    .max(Duration::from_millis(5)),
            );
        }
        interval
    }

    // ---- touch calibration -----------------------------------------------

    /// A touch from the strip's own panel, at the X the panel reports.
    /// Studio's mirror and the emulator send exact positions and go straight
    /// to `touch_event`.
    pub(crate) fn panel_touch(&mut self, phase: nexus_ipc::TouchPhase, raw_x: u16, now: Instant) {
        if self.calibration.is_some() {
            self.calibration_touch(phase, raw_x, now);
        } else {
            let x = self.runtime.settings().touch.apply(raw_x);
            self.touch_event(phase, x, now);
        }
    }

    /// Starts, reports on or cancels a calibration; true when the strip
    /// changes.
    fn calibrate(&mut self, action: nexus_ipc::CalibrateAction, now: Instant) -> bool {
        use nexus_ipc::{CalibrateAction, CalibrationState};
        match action {
            CalibrateAction::Start => {
                self.cancel_touch(now);
                // The marks must show, whatever the strip was doing.
                self.runtime.manual_blank = false;
                self.runtime.touch_wake_blank = false;
                self.runtime.last_touch = now;
                self.calibration = Some(crate::calibration::Calibration::new(now));
                true
            }
            CalibrateAction::Status => false,
            CalibrateAction::Cancel => {
                let running = self.calibration.take().is_some();
                if running {
                    self.calibration_ended =
                        crate::calibration::ended(CalibrationState::Cancelled, None);
                }
                running
            }
        }
    }

    fn calibration_status(&self) -> nexus_ipc::CalibrationStatus {
        self.calibration
            .as_ref()
            .map_or_else(|| self.calibration_ended.clone(), |c| c.status())
    }

    /// A touch on the panel during calibration, at the panel's own X.
    fn calibration_touch(&mut self, phase: nexus_ipc::TouchPhase, raw_x: u16, now: Instant) {
        use nexus_ipc::CalibrationState;
        let Some(calibration) = self.calibration.as_mut() else {
            return;
        };
        self.runtime.last_touch = now;
        calibration.touch(phase, raw_x, now);
        if calibration.mark().is_none() {
            let result = calibration.result();
            let state = if result.is_some() {
                CalibrationState::Done
            } else {
                CalibrationState::Failed
            };
            self.calibration = None;
            self.calibration_ended = crate::calibration::ended(state, result);
            log_fields(
                &self.logger,
                Level::Info,
                "touch_calibrated",
                "touch calibration finished",
                [("result", format!("{result:?}"))],
            );
        }
    }

    // ---- API -------------------------------------------------------------

    pub(crate) fn drain_requests(
        &mut self,
        requests: &Receiver<ControlRequest>,
        now: Instant,
    ) -> bool {
        let mut changed = false;
        while let Ok(request) = requests.try_recv() {
            let (response, did_change) = self.handle_request(request.command, now);
            changed |= did_change;
            let _ = request.reply.send(response);
        }
        changed
    }

    fn status(&self) -> nexus_ipc::DaemonStatus {
        let (position, count) = self.runtime.visible_position();
        nexus_ipc::DaemonStatus {
            mode: self.mode,
            device: self.device.clone(),
            profile_id: self.runtime.profile().id.clone(),
            profile_name: self.runtime.profile().name.clone(),
            page_id: self.runtime.page().id.clone(),
            page_name: self.runtime.page().name.clone(),
            page_index: position,
            page_count: count,
            brightness_percent: self.runtime.brightness,
            blanked: self.runtime.blanked(),
            manual_blanked: self.runtime.manual_blank,
            session_blanked: self.runtime.session_blank,
        }
    }

    fn ok(&self, message: &str) -> nexus_ipc::Response {
        nexus_ipc::Response::with_status(message, self.status())
    }

    fn handle_request(
        &mut self,
        command: nexus_ipc::Command,
        now: Instant,
    ) -> (nexus_ipc::Response, bool) {
        use nexus_ipc::Command as C;
        match command {
            C::Ping => (nexus_ipc::Response::ok("pong"), false),
            C::Status => (self.ok("status"), false),
            C::ReloadConfig { path } => {
                let path = path.map(PathBuf::from);
                match self.runtime.reload(path.as_deref()) {
                    Ok(()) => {
                        // Indices, motion and waiting sequence steps refer
                        // to the old config.
                        self.cancel_touch(now);
                        self.scene.reset();
                        self.scheduled.clear();
                        log_fields(
                            &self.logger,
                            Level::Info,
                            "config_reloaded",
                            "configuration reloaded",
                            [("profile", self.runtime.profile().id.clone())],
                        );
                        (self.ok("config reloaded"), true)
                    }
                    Err(error) => (
                        nexus_ipc::Response::error(format!("reload failed: {error:#}")),
                        false,
                    ),
                }
            }
            C::Tap { x } if self.calibration.is_some() => {
                if self.mode == nexus_ipc::DaemonMode::Emulator {
                    let x = x.min(639);
                    self.calibration_touch(nexus_ipc::TouchPhase::Began, x, now);
                    self.calibration_touch(nexus_ipc::TouchPhase::Ended, x, now);
                }
                (self.ok("tap handled"), true)
            }
            C::Tap { x } => {
                self.touch_event(nexus_ipc::TouchPhase::Began, x, now);
                self.touch_event(
                    nexus_ipc::TouchPhase::Ended,
                    x,
                    now + Duration::from_millis(40),
                );
                (self.ok("tap handled"), true)
            }
            // Studio's mirror sends exact positions: they would spoil a
            // calibration of the panel. The emulator has only those.
            C::Touch { phase, x } if self.calibration.is_some() => {
                if self.mode == nexus_ipc::DaemonMode::Emulator {
                    self.calibration_touch(phase, x.min(639), now);
                }
                (nexus_ipc::Response::ok("touch"), true)
            }
            C::Touch { phase, x } => {
                self.touch_event(phase, x.min(639), now);
                (nexus_ipc::Response::ok("touch"), true)
            }
            C::Calibrate {
                action: nexus_ipc::CalibrateAction::Start,
            } if self.mode == nexus_ipc::DaemonMode::Hardware
                && self.device.state != nexus_ipc::DeviceState::Connected =>
            {
                (
                    nexus_ipc::Response::error("the strip is not connected"),
                    false,
                )
            }
            C::Calibrate { action } => {
                let changed = self.calibrate(action, now);
                (
                    nexus_ipc::Response {
                        ok: true,
                        message: "calibration".into(),
                        calibration: Some(self.calibration_status()),
                        ..nexus_ipc::Response::default()
                    },
                    changed,
                )
            }
            C::PageNext | C::PagePrevious => {
                let direction = if matches!(command, C::PageNext) {
                    Direction::Forward
                } else {
                    Direction::Backward
                };
                if let Some(change) = self.runtime.step(direction) {
                    self.page_changed(change, "api", now);
                }
                (self.ok("page changed"), true)
            }
            C::GoToPage { page_id } => match self.runtime.go_to_page(&page_id) {
                Ok(change) => {
                    if let Some(change) = change {
                        self.page_changed(change, "api", now);
                    }
                    (self.ok("page changed"), true)
                }
                Err(error) => (nexus_ipc::Response::error(format!("{error:#}")), false),
            },
            C::SwitchProfile { profile_id } => match self.switch_profile(&profile_id, now) {
                Ok(()) => (self.ok("profile changed"), true),
                Err(error) => (nexus_ipc::Response::error(format!("{error:#}")), false),
            },
            C::SetBrightness { percent } => {
                self.runtime.brightness = percent.min(100);
                self.persist();
                (self.ok("brightness updated"), true)
            }
            C::Blank => {
                // The marks cannot be seen any more.
                self.calibrate(nexus_ipc::CalibrateAction::Cancel, now);
                self.runtime.manual_blank = true;
                (self.ok("screen blanked"), true)
            }
            C::Restore => {
                self.runtime.manual_blank = false;
                self.runtime.touch_wake_blank = false;
                (self.ok("screen restored"), true)
            }
            C::Frame => {
                let png = self
                    .last_frame
                    .as_deref()
                    .and_then(nexus_render::encode_png)
                    .map(|png| base64::engine::general_purpose::STANDARD.encode(png));
                (
                    nexus_ipc::Response {
                        ok: png.is_some(),
                        message: if png.is_some() {
                            "frame".into()
                        } else {
                            "no frame yet".into()
                        },
                        frame: png,
                        ..nexus_ipc::Response::default()
                    },
                    false,
                )
            }
            C::Notify { text, icon } => {
                let icon = icon
                    .filter(|name| nexus_render::icons::exists(name))
                    .unwrap_or_else(|| "bell".into());
                self.scene
                    .show_overlay(Overlay::Toast { icon, text }, Duration::from_secs(5), now);
                (nexus_ipc::Response::ok("shown"), true)
            }
            C::Metrics => (
                nexus_ipc::Response {
                    ok: true,
                    message: "metrics".into(),
                    metrics: Some(crate::live::metric_readings(&self.metrics.latest())),
                    ..nexus_ipc::Response::default()
                },
                false,
            ),
        }
    }
}

/// Widgets that show press feedback; plain readouts without an action do
/// not light up.
fn reacts_to_press(widget: &Widget) -> bool {
    widget.tap.is_some()
        || widget.hold.is_some()
        || widget.double_tap.is_some()
        || matches!(
            widget.kind,
            WidgetKind::Button(_)
                | WidgetKind::Media(_)
                | WidgetKind::Entity(_)
                | WidgetKind::Timer(_)
                | WidgetKind::DiscordVoice(_)
                | WidgetKind::DiscordCall(_)
        )
}

/// A button that opens the microphone while it is held.
fn is_push_to_talk(widget: &Widget) -> bool {
    matches!(
        widget.tap,
        Some(ActionSpec::Discord {
            command: DiscordCommand::PushToTalk,
            ..
        })
    )
}

/// The microphone badge is for a voice channel you are in, and only where
/// nothing else on the page already shows your microphone.
fn wants_mic_badge(discord: &DiscordInfo, page: &nexus_config::Page) -> bool {
    discord.channel.is_some() && !page.widgets.iter().any(shows_own_mute)
}

/// Widgets that already show whether you are muted on Discord, so the
/// corner badge is not needed on their page.
fn shows_own_mute(widget: &Widget) -> bool {
    let mute_button = matches!(
        widget.tap,
        Some(ActionSpec::Discord {
            command: DiscordCommand::ToggleMute
                | DiscordCommand::ToggleDeafen
                | DiscordCommand::PushToTalk,
            ..
        })
    );
    mute_button
        || matches!(widget.kind, WidgetKind::DiscordCall(_))
        || matches!(&widget.kind, WidgetKind::Volume(p) if p.source.is_discord())
}

/// Steps that move between pages of the profile they were started in, also
/// inside a nested sequence.
fn is_page_step(action: &ActionSpec) -> bool {
    match action {
        ActionSpec::NextPage
        | ActionSpec::PreviousPage
        | ActionSpec::GoToPage { .. }
        | ActionSpec::GoBack => true,
        ActionSpec::Sequence { steps, .. } => steps.iter().any(is_page_step),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
