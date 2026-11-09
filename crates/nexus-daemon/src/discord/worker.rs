//! The thread that stays signed in to Discord.

use super::images::Images;
use super::request::request;
use super::tracked::{Channel, Tracked};
use super::*;
use nexus_sensors::DiscordVoiceMember;

const CHANNEL_EVENTS: [&str; 5] = [
    "VOICE_STATE_CREATE",
    "VOICE_STATE_UPDATE",
    "VOICE_STATE_DELETE",
    "SPEAKING_START",
    "SPEAKING_STOP",
];

pub(super) struct Worker {
    pub(super) shared: Arc<Shared>,
    pub(super) commands: Receiver<Command>,
    pub(super) feedback: Sender<Feedback>,
    pub(super) logger: nexus_events::EventLogger,
    pub(super) load: LoadApp,
    pub(super) images: Images,
    pub(super) policy: Policy,
}

/// A notification waiting for its picture and for the desktop's answer on
/// whether it may show.
struct Pending {
    notification: DiscordNotification,
    icon: Option<String>,
    /// Which question to the desktop this waits for.
    asked: u64,
    decided: Option<bool>,
    /// The picture is left out after this.
    picture_by: Instant,
    /// Without an answer by then, the message is dropped.
    decide_by: Instant,
}

impl Pending {
    /// True once this can leave the queue: everything is in, or waiting
    /// longer would make it late.
    fn settled(&self, images: &Images, now: Instant) -> bool {
        let picture = self.icon.as_deref().is_none_or(|url| images.answered(url));
        (self.decided.is_some() && (picture || now >= self.picture_by)) || now >= self.decide_by
    }

    /// Shows only on a yes: no answer means the desktop could not say
    /// whether do not disturb is on, and message text must not show then.
    fn into_shown(mut self, images: &Images) -> Option<DiscordNotification> {
        if self.decided != Some(true) {
            return None;
        }
        self.notification.icon = self.icon.as_deref().and_then(|url| images.get(url));
        Some(self.notification)
    }
}

/// Asks the desktop on one long-lived thread whether a notification from
/// Discord could pop up now: a bus that hangs must not hold up voice, nor
/// pile up threads.
pub(super) struct Policy {
    /// Dropping this ends the thread.
    jobs: Sender<(u64, String)>,
    answers: Receiver<(u64, bool)>,
    next: u64,
}

impl Policy {
    pub(super) fn start() -> Self {
        let (jobs, asked) = mpsc::channel::<(u64, String)>();
        let (answer, answers) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("discord-policy".into())
            .spawn(move || {
                while let Ok(first) = asked.recv() {
                    // Questions that piled up get the same answer.
                    let mut batch = vec![first];
                    batch.extend(asked.try_iter());
                    let policy = nexus_sensors::PopupPolicy::read(
                        batch.last().map(|(_, app)| app.as_str()),
                        None,
                    );
                    let shows = policy.shows(nexus_sensors::Urgency::Normal);
                    for (id, _) in batch {
                        if answer.send((id, shows)).is_err() {
                            return;
                        }
                    }
                }
            });
        Self {
            jobs,
            answers,
            next: 0,
        }
    }

    fn ask(&mut self, app: &str) -> u64 {
        self.next += 1;
        let _ = self.jobs.send((self.next, app.to_string()));
        self.next
    }

    fn answer(&self, pending: &mut [Pending]) {
        for (id, shows) in self.answers.try_iter() {
            if let Some(item) = pending.iter_mut().find(|item| item.asked == id) {
                item.decided = Some(shows);
            }
        }
    }
}

impl Worker {
    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn wanted(&self) -> Option<Option<Wanted>> {
        let status = self.lock();
        (!status.stop).then(|| status.wanted.clone())
    }

    pub(super) fn run(mut self) {
        self.serve();
        self.lock().finished = true;
        self.shared.wake.notify_all();
    }

    fn serve(&mut self) {
        let mut retry = RETRY_MIN;
        let mut retry_at = Instant::now();
        loop {
            let Some(wanted) = self.wanted() else {
                return;
            };
            while let Ok(command) = self.commands.try_recv() {
                self.not_sent(command, NOT_CONNECTED);
            }
            let app = if wanted.is_some() && Instant::now() >= retry_at {
                let app = (self.load)();
                self.lock().configured = app.is_some();
                if app.is_none() {
                    // Not set up: look again now and then.
                    retry_at = Instant::now() + CONFIG_EVERY;
                }
                app
            } else {
                None
            };
            let Some(app) = app else {
                if wanted.is_none() {
                    self.lock().problem = None;
                }
                self.wait(Duration::from_secs(1));
                continue;
            };
            match nexus_sensors::discord_sign_in(&app, CONNECT_TIMEOUT) {
                Ok(signed) => {
                    if let Some(error) = signed.unsaved {
                        log_fields(
                            &self.logger,
                            Level::Warn,
                            "discord_token_not_saved",
                            "could not save the renewed Discord token",
                            [("error", error)],
                        );
                    }
                    let mut app = app;
                    app.token = Some(signed.token);
                    let started = Instant::now();
                    self.session(signed.connection, &app, &signed.user);
                    // A Discord that accepts and then drops every connection
                    // backs off like one that refuses.
                    if started.elapsed() >= STEADY_SESSION {
                        retry = RETRY_MIN;
                    }
                    retry_at = Instant::now() + retry;
                    retry = (retry * 2).min(RETRY_MAX);
                }
                Err(error) => {
                    let wait = match error {
                        DiscordError::NotAuthorized
                        | DiscordError::Unsupported
                        | DiscordError::UnknownApplication => RETRY_SLOW,
                        _ => retry,
                    };
                    retry = (retry * 2).min(RETRY_MAX);
                    retry_at = Instant::now() + wait;
                    let problem = error.to_string();
                    let changed = self.lock().problem.as_deref() != Some(problem.as_str());
                    if changed {
                        log_fields(
                            &self.logger,
                            Level::Warn,
                            "discord_unavailable",
                            "could not connect to Discord",
                            [("error", problem.clone())],
                        );
                    }
                    self.lock().problem = Some(problem);
                }
            }
        }
    }

    /// Sleeps up to `wait`, or until wanted or stop change.
    fn wait(&self, wait: Duration) {
        let status = self.lock();
        let _ = self.shared.wake.wait_timeout(status, wait);
    }

    fn session(&mut self, mut discord: DiscordConnection, app: &DiscordApp, user: &str) {
        let mut tracked = Tracked {
            self_id: discord.user_id().map(str::to_string),
            ..Tracked::default()
        };
        let started = Self::sync_self(&mut discord, &mut tracked).and_then(|()| {
            discord.subscribe("VOICE_SETTINGS_UPDATE", json!({}))?;
            discord.subscribe("VOICE_CHANNEL_SELECT", json!({}))?;
            // Older Discord versions may not know it; the ping just stays
            // unknown then.
            match discord.subscribe("VOICE_CONNECTION_STATUS", json!({})) {
                Ok(()) | Err(DiscordError::Command { .. }) => Ok(()),
                Err(error) => Err(error),
            }
        });
        if let Err(error) = started {
            self.lock().problem = Some(error.to_string());
            return;
        }
        if let Err(error) = self.remute(&mut discord) {
            self.lock().problem = Some(error.to_string());
            return;
        }
        log_fields(
            &self.logger,
            Level::Info,
            "discord_connected",
            "connected to Discord",
            [("user", user.to_string())],
        );
        self.lock().problem = None;
        let may_notify = app
            .token
            .as_ref()
            .is_some_and(|token| token.has_scope(nexus_sensors::DISCORD_NOTIFICATIONS_SCOPE));
        let mut notifying = false;
        let mut notify_refused = false;
        let mut pending: Vec<Pending> = Vec::new();
        let mut config_checked = Instant::now();
        let mut resync_at = Instant::now();
        let ended = loop {
            let Some(Some(wanted)) = self.wanted() else {
                break None;
            };
            let notify = wanted.notifications && may_notify && !notify_refused;
            if notify != notifying {
                let result = if notify {
                    discord.subscribe("NOTIFICATION_CREATE", json!({}))
                } else {
                    discord.unsubscribe("NOTIFICATION_CREATE", json!({}))
                };
                match result {
                    Ok(()) => notifying = notify,
                    Err(DiscordError::Command { message, .. }) => {
                        if notify {
                            notify_refused = true;
                            log_fields(
                                &self.logger,
                                Level::Warn,
                                "discord_notifications_refused",
                                "Discord does not send its notifications",
                                [("error", message)],
                            );
                        }
                        notifying = false;
                    }
                    Err(error) => break Some(error),
                }
                self.lock().notifying = notifying;
            }
            if config_checked.elapsed() >= CONFIG_EVERY {
                config_checked = Instant::now();
                let stored = (self.load)();
                let same = stored.as_ref().is_some_and(|stored| {
                    stored.client_id == app.client_id
                        && stored.token.as_ref().map(|t| &t.access_token)
                            == app.token.as_ref().map(|t| &t.access_token)
                });
                if !same {
                    break None;
                }
            }
            if let Err(error) = self.run_commands(&mut discord, &mut tracked) {
                break Some(error);
            }
            if let Err(error) = self.remute(&mut discord) {
                break Some(error);
            }
            if Instant::now() >= resync_at {
                resync_at = Instant::now() + RESYNC_EVERY;
                if let Err(error) = Self::sync_self(&mut discord, &mut tracked)
                    .and_then(|()| Self::follow(&mut discord, &mut tracked, &wanted, true))
                {
                    break Some(error);
                }
            } else if let Err(error) = Self::follow(&mut discord, &mut tracked, &wanted, false) {
                break Some(error);
            }
            let mut resync_soon = false;
            let mut messages = Vec::new();
            match discord.next_event(EVENT_WAIT) {
                Ok(Some(event)) => {
                    let mut event = Some(event);
                    // Take what else arrived before redrawing.
                    while let Some(next) = event.take() {
                        if next.evt == "NOTIFICATION_CREATE" {
                            messages.push(next.data);
                        } else {
                            resync_soon |= next.evt == "VOICE_CHANNEL_SELECT";
                            resync_soon |= tracked.apply(&next);
                        }
                        // The next read reports a broken connection.
                        event = discord.next_event(Duration::ZERO).ok().flatten();
                    }
                }
                Ok(None) => {}
                Err(error) => break Some(error),
            }
            let mut lost = None;
            for message in messages.into_iter().filter(|_| notifying) {
                match self.notification(&mut discord, &message) {
                    Ok(notification) => pending.push(notification),
                    Err(error) => {
                        lost = Some(error);
                        break;
                    }
                }
            }
            if lost.is_some() {
                break lost;
            }
            self.images.poll();
            self.deliver(&mut pending);
            if resync_soon {
                // Members of a channel you joined, or of one someone
                // joined, show within a moment.
                resync_at = resync_at.min(Instant::now() + Duration::from_millis(500));
            }
            self.fetch_avatars(&tracked);
            self.lock().info = Some(tracked.info(&self.images));
        };
        // Nothing wants Discord any more; what was queued still goes out, a
        // microphone to mute above all.
        if ended.is_none() {
            let _ = self.run_commands(&mut discord, &mut tracked);
        }
        {
            let mut status = self.lock();
            status.info = None;
            status.notifying = false;
        }
        self.ended(ended);
    }

    /// Hands over the notifications that are ready.
    fn deliver(&self, pending: &mut Vec<Pending>) {
        self.policy.answer(pending);
        let now = Instant::now();
        let mut waiting = Vec::with_capacity(pending.len());
        for item in pending.drain(..) {
            if !item.settled(&self.images, now) {
                waiting.push(item);
                continue;
            }
            if let Some(notification) = item.into_shown(&self.images) {
                let mut status = self.lock();
                if status.notifications.len() >= MAX_NOTIFICATIONS {
                    status.notifications.pop_front();
                }
                status.notifications.push_back(notification);
            }
        }
        *pending = waiting;
    }

    /// Mutes the microphone that push to talk left open when Discord went
    /// away.
    fn remute(&self, discord: &mut DiscordConnection) -> Result<(), DiscordError> {
        let due = self
            .lock()
            .remute_until
            .take()
            .is_some_and(|until| Instant::now() < until);
        if !due {
            return Ok(());
        }
        match discord.command("SET_VOICE_SETTINGS", json!({ "mute": true })) {
            Ok(_) | Err(DiscordError::Command { .. }) => Ok(()),
            Err(error) => {
                self.lock().remute_until = Some(Instant::now() + REMUTE_WITHIN);
                Err(error)
            }
        }
    }

    /// Turns a NOTIFICATION_CREATE into what the strip shows: the author's
    /// picture, and a link to the message (asking Discord which server the
    /// channel belongs to; none for direct messages).
    fn notification(
        &mut self,
        discord: &mut DiscordConnection,
        data: &Value,
    ) -> Result<Pending, DiscordError> {
        let text = |value: &Value| value.as_str().unwrap_or_default().trim().to_string();
        let channel = data["channel_id"].as_str().map(str::to_string);
        let answer = match &channel {
            Some(channel) => match discord.command("GET_CHANNEL", json!({ "channel_id": channel }))
            {
                Ok(answer) => answer,
                Err(DiscordError::Command { .. }) => Value::Null,
                Err(error) => return Err(error),
            },
            None => Value::Null,
        };
        let guild = answer["guild_id"].as_str().map(str::to_string);
        // 1 is a direct message, 3 a group; without an answer, a message
        // outside any server.
        let direct = match answer["type"].as_u64() {
            Some(kind) => matches!(kind, 1 | 3),
            None => guild.is_none(),
        };
        let link = channel.and_then(|channel| {
            let place = guild.unwrap_or_else(|| "@me".to_string());
            let mut link = format!("discord://-/channels/{place}/{channel}");
            if let Some(message) = data["message"]["id"].as_str() {
                link.push('/');
                link.push_str(message);
            }
            nexus_actions::is_discord_link(&link).then_some(link)
        });
        let icon = data["icon_url"]
            .as_str()
            .and_then(nexus_sensors::discord_trusted_image_url)
            .map(str::to_string);
        if let Some(url) = &icon {
            self.images.ask(url);
        }
        Ok(Pending {
            notification: DiscordNotification {
                title: text(&data["title"]),
                body: text(&data["body"]),
                icon: None,
                link,
                direct,
            },
            icon,
            asked: self.policy.ask(discord.desktop_id()),
            decided: None,
            picture_by: Instant::now() + PICTURE_WAIT,
            decide_by: Instant::now() + DESKTOP_ANSWER_WAIT,
        })
    }

    fn ended(&self, error: Option<DiscordError>) {
        self.lock().problem = error.as_ref().map(ToString::to_string);
        log_fields(
            &self.logger,
            Level::Info,
            "discord_disconnected",
            "disconnected from Discord",
            [(
                "reason",
                error.map_or_else(|| "no longer needed".into(), |error| error.to_string()),
            )],
        );
    }

    /// Your own channel, mute and deafen.
    fn sync_self(
        discord: &mut DiscordConnection,
        tracked: &mut Tracked,
    ) -> Result<(), DiscordError> {
        let settings = discord.command("GET_VOICE_SETTINGS", json!({}))?;
        tracked.apply_settings(&settings);
        let selected = discord.command("GET_SELECTED_VOICE_CHANNEL", json!({}))?;
        let channel = selected["id"].as_str().map(str::to_string);
        if channel != tracked.channel {
            tracked.speaking.clear();
        }
        tracked.channel = channel;
        tracked.guild = selected["guild_id"].as_str().map(str::to_string);
        Ok(())
    }

    /// Reads the channels of the servers in `wanted` (when new, stale or
    /// `resync`), who sits in them, and subscribes to their events. Servers
    /// no longer wanted are dropped.
    fn follow(
        discord: &mut DiscordConnection,
        tracked: &mut Tracked,
        wanted: &Wanted,
        resync: bool,
    ) -> Result<(), DiscordError> {
        let guilds = tracked.guilds_for(wanted);
        let dropped: Vec<String> = tracked
            .guilds
            .keys()
            .filter(|guild| !guilds.contains(*guild))
            .cloned()
            .collect();
        for guild in dropped {
            for channel in tracked.guilds.remove(&guild).unwrap_or_default() {
                if tracked.subscribed.remove(&channel.id) {
                    for evt in CHANNEL_EVENTS {
                        // An error here only means it was not subscribed.
                        let _ = discord.unsubscribe(evt, json!({ "channel_id": channel.id }));
                    }
                }
            }
            tracked.listed.remove(&guild);
        }
        for guild in guilds {
            let stale = tracked
                .listed
                .get(&guild)
                .is_none_or(|at| at.elapsed() >= CHANNELS_EVERY);
            if !stale && !resync {
                continue;
            }
            if stale {
                let answer = match discord.command("GET_CHANNELS", json!({ "guild_id": guild })) {
                    Ok(answer) => answer,
                    // Not a member any more, or not allowed: show nothing.
                    Err(DiscordError::Command { .. }) => Value::Null,
                    Err(error) => return Err(error),
                };
                let listed = nexus_sensors::discord_voice_channels(&answer);
                let old = tracked.guilds.remove(&guild).unwrap_or_default();
                let channels = listed
                    .into_iter()
                    .take(MAX_CHANNELS)
                    .map(|channel| Channel {
                        members: old
                            .iter()
                            .find(|o| o.id == channel.id)
                            .map(|o| o.members.clone())
                            .unwrap_or_default(),
                        id: channel.id,
                        name: channel.name,
                    })
                    .collect();
                tracked.guilds.insert(guild.clone(), channels);
                tracked.listed.insert(guild.clone(), Instant::now());
            }
            let ids: Vec<String> = tracked.guilds[&guild]
                .iter()
                .map(|c| c.id.clone())
                .collect();
            for id in ids {
                let members = match discord.command("GET_CHANNEL", json!({ "channel_id": id })) {
                    Ok(answer) => answer["voice_states"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(DiscordVoiceMember::from_voice_state)
                        .collect(),
                    Err(DiscordError::Command { .. }) => Vec::new(),
                    Err(error) => return Err(error),
                };
                if let Some(channel) = tracked.channel_mut(&id) {
                    channel.members = members;
                }
                if tracked.subscribed.insert(id.clone()) {
                    for evt in CHANNEL_EVENTS {
                        match discord.subscribe(evt, json!({ "channel_id": id })) {
                            Ok(()) | Err(DiscordError::Command { .. }) => {}
                            Err(error) => return Err(error),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Asks for the pictures of whoever sits in the followed channels.
    fn fetch_avatars(&mut self, tracked: &Tracked) {
        for member in tracked
            .guilds
            .values()
            .flatten()
            .flat_map(|channel| &channel.members)
        {
            self.images.ask(&member.avatar_url);
        }
    }

    /// Reports on the strip that a command did not reach Discord.
    fn not_sent(&self, command: Command, why: &str) {
        if command.request == Request::SetMute(true) {
            self.lock().remute_until = Some(Instant::now() + REMUTE_WITHIN);
        }
        log_fields(
            &self.logger,
            Level::Error,
            "action_failed",
            "action failed",
            [
                ("widget", command.label.clone()),
                ("error", why.to_string()),
            ],
        );
        let _ = self.feedback.send(Feedback {
            label: command.label,
            error: why.to_string(),
        });
    }

    /// Sends the queued commands. Discord refusing one is reported on the
    /// strip; a broken connection ends the session.
    fn run_commands(
        &self,
        discord: &mut DiscordConnection,
        tracked: &mut Tracked,
    ) -> Result<(), DiscordError> {
        while let Ok(command) = self.commands.try_recv() {
            let Some((cmd, args)) = request(&command.request, tracked) else {
                continue;
            };
            match discord.command(cmd, args) {
                Ok(answer) => {
                    match cmd {
                        "SET_VOICE_SETTINGS" => tracked.apply_settings(&answer),
                        "SET_USER_VOICE_SETTINGS" => tracked.apply_user(&answer),
                        "SELECT_VOICE_CHANNEL" => {
                            tracked.channel = answer["id"].as_str().map(str::to_string);
                            tracked.guild = answer["guild_id"].as_str().map(str::to_string);
                            tracked.speaking.clear();
                            if tracked.channel.is_none() {
                                tracked.link = None;
                            }
                        }
                        _ => {}
                    }
                    log_fields(
                        &self.logger,
                        Level::Info,
                        "action_executed",
                        "action executed",
                        [
                            ("widget", command.label),
                            ("command", format!("discord {cmd}")),
                        ],
                    );
                }
                Err(error @ DiscordError::Command { .. }) => {
                    self.not_sent(command, &error.to_string())
                }
                Err(error) => {
                    self.not_sent(command, &error.to_string());
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(icon: Option<&str>, decided: Option<bool>, t: Instant) -> Pending {
        Pending {
            notification: DiscordNotification {
                title: "Ada".into(),
                body: "hello".into(),
                icon: None,
                link: None,
                direct: true,
            },
            icon: icon.map(str::to_string),
            asked: 1,
            decided,
            picture_by: t + PICTURE_WAIT,
            decide_by: t + DESKTOP_ANSWER_WAIT,
        }
    }

    #[test]
    fn a_message_shows_only_when_the_desktop_says_it_may() {
        let images = Images::with_fetch(Box::new(|_| None));
        let t = Instant::now();
        assert!(pending(None, Some(true), t).into_shown(&images).is_some());
        assert!(pending(None, Some(false), t).into_shown(&images).is_none());
        // No answer: it waits, and when time is up nothing shows.
        let silent = pending(None, None, t);
        assert!(!silent.settled(&images, t));
        assert!(silent.settled(&images, t + DESKTOP_ANSWER_WAIT));
        assert!(silent.into_shown(&images).is_none());
    }

    #[test]
    fn a_yes_waits_for_the_picture_for_a_moment_then_goes_without() {
        let images = Images::with_fetch(Box::new(|_| None));
        let t = Instant::now();
        let picture = pending(Some("slow"), Some(true), t);
        assert!(!picture.settled(&images, t));
        assert!(picture.settled(&images, t + PICTURE_WAIT));
    }
}
