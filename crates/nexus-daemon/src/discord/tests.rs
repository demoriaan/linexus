use super::images::Images;
use super::request::request;
use super::tracked::{Channel, Tracked};
use super::*;
use nexus_config::{ButtonProps, DiscordVoiceProps, Widget};
use nexus_render::DiscordLink;
use nexus_sensors::{DiscordEvent, DiscordVoiceMember};

fn member(id: &str) -> Value {
    json!({ "user": { "id": id, "username": format!("u{id}") }, "voice_state": {} })
}

fn tracked() -> Tracked {
    let mut tracked = Tracked {
        channel: Some("10".into()),
        guild: Some("1".into()),
        ..Tracked::default()
    };
    tracked.guilds.insert(
        "1".into(),
        vec![
            Channel {
                id: "10".into(),
                name: "Gaming".into(),
                members: vec![DiscordVoiceMember::from_voice_state(&member("100")).unwrap()],
            },
            Channel {
                id: "11".into(),
                name: "Chill".into(),
                members: Vec::new(),
            },
        ],
    );
    tracked
}

fn event(evt: &str, data: Value) -> DiscordEvent {
    DiscordEvent {
        evt: evt.into(),
        data,
    }
}

#[test]
fn follows_members_moving_speaking_and_leaving() {
    let mut tracked = tracked();
    let mut join = member("200");
    join["channel_id"] = "11".into();
    tracked.apply(&event("VOICE_STATE_CREATE", join));
    tracked.apply(&event("SPEAKING_START", json!({ "user_id": "100" })));
    let info = tracked.info(&Images::with_fetch(Box::new(|_| None)));
    let channels = info.channels_for(None);
    assert_eq!(channels[1].members[0].user_id, "200");
    assert!(channels[0].members[0].speaking);

    let mut muted = member("200");
    muted["voice_state"]["self_mute"] = true.into();
    tracked.apply(&event("VOICE_STATE_UPDATE", muted));
    assert!(tracked.guilds["1"][1].members[0].muted);

    tracked.apply(&event("VOICE_STATE_DELETE", member("100")));
    assert!(tracked.guilds["1"][0].members.is_empty());
    assert!(tracked.speaking.is_empty());

    tracked.apply(&event(
        "VOICE_SETTINGS_UPDATE",
        json!({ "mute": true, "deaf": false }),
    ));
    assert!(tracked.self_muted);
    tracked.apply(&event(
        "VOICE_CHANNEL_SELECT",
        json!({ "channel_id": null, "guild_id": null }),
    ));
    assert_eq!(tracked.channel, None);
}

fn command(command: DiscordCommand, channel: Option<&str>) -> Request {
    Request::Command {
        command,
        channel: channel.map(Into::into),
        step: None,
    }
}

#[test]
fn commands_become_rpc_requests() {
    let mut tracked = tracked();
    assert_eq!(
        request(&command(DiscordCommand::ToggleMute, None), &tracked),
        Some(("SET_VOICE_SETTINGS", json!({ "mute": true })))
    );
    assert_eq!(
        request(&command(DiscordCommand::JoinVoice, Some("10")), &tracked),
        None
    );
    assert_eq!(
        request(&command(DiscordCommand::JoinVoice, Some("11")), &tracked),
        Some((
            "SELECT_VOICE_CHANNEL",
            json!({ "channel_id": "11", "force": true })
        ))
    );
    assert_eq!(
        request(&command(DiscordCommand::LeaveVoice, None), &tracked)
            .unwrap()
            .1,
        json!({ "channel_id": null })
    );
    assert_eq!(
        request(&command(DiscordCommand::PushToTalk, None), &tracked),
        None
    );

    // Levels step from where they are and stay in range.
    tracked.input_volume = 95;
    tracked.output_volume = 100;
    assert_eq!(
        request(&command(DiscordCommand::InputVolumeUp, None), &tracked)
            .unwrap()
            .1,
        json!({ "input": { "volume": 100 } })
    );
    let louder = Request::Command {
        command: DiscordCommand::OutputVolumeUp,
        channel: None,
        step: Some(25),
    };
    assert_eq!(
        request(&louder, &tracked).unwrap().1,
        json!({ "output": { "volume": 125 } })
    );
    assert_eq!(
        request(&command(DiscordCommand::ToggleVoiceMode, None), &tracked)
            .unwrap()
            .1,
        json!({ "mode": { "type": "PUSH_TO_TALK" } })
    );
    assert_eq!(
        request(
            &Request::UserVolume {
                user: "7".into(),
                volume: 250
            },
            &tracked
        ),
        Some((
            "SET_USER_VOICE_SETTINGS",
            json!({ "user_id": "7", "volume": 200 })
        ))
    );
}

#[test]
fn follows_settings_the_connection_and_volumes_for_you() {
    let mut tracked = tracked();
    tracked.apply(&event(
        "VOICE_SETTINGS_UPDATE",
        json!({
            "input": { "volume": 64.4 },
            "output": { "volume": 150 },
            "mode": { "type": "PUSH_TO_TALK" },
            "noise_suppression": true,
            "automatic_gain_control": false,
        }),
    ));
    assert_eq!((tracked.input_volume, tracked.output_volume), (64, 150));
    assert!(tracked.push_to_talk && tracked.noise_suppression && !tracked.automatic_gain);

    tracked.apply(&event(
        "VOICE_CONNECTION_STATUS",
        json!({ "state": "VOICE_CONNECTED", "last_ping": 41.6, "average_ping": 50 }),
    ));
    let info = tracked.info(&Images::with_fetch(Box::new(|_| None)));
    assert_eq!(
        info.link,
        Some(DiscordLink {
            connected: true,
            ping_ms: Some(42)
        })
    );

    tracked.apply_user(&json!({ "user_id": "100", "volume": 40, "mute": true }));
    let member = &tracked.guilds["1"][0].members[0];
    assert_eq!((member.volume, member.local_muted), (40, true));

    tracked.apply(&event(
        "VOICE_CHANNEL_SELECT",
        json!({ "channel_id": null, "guild_id": null }),
    ));
    assert_eq!(
        tracked.info(&Images::with_fetch(Box::new(|_| None))).link,
        None
    );
}

#[test]
fn the_profile_says_which_servers_to_follow() {
    let mut profile = nexus_config::default_config().profiles.remove(0);
    assert_eq!(profile_wants_discord(&profile), None);
    let mut voice = Widget::new(
        "voice",
        0,
        300,
        WidgetKind::DiscordVoice(DiscordVoiceProps {
            guild_id: Some("5".into()),
            ..DiscordVoiceProps::default()
        }),
    );
    profile.pages[0].widgets = vec![voice.clone()];
    let wanted = profile_wants_discord(&profile).unwrap();
    assert_eq!(wanted.guilds, BTreeSet::from(["5".to_string()]));
    assert!(!wanted.current_guild);

    voice.kind = WidgetKind::Button(ButtonProps::default());
    voice.tap = Some(ActionSpec::Sequence {
        steps: vec![ActionSpec::Discord {
            command: DiscordCommand::ToggleMute,
            channel: None,
            step: None,
        }],
        delay_ms: 0,
    });
    profile.pages[0].widgets = vec![voice.clone()];
    assert_eq!(profile_wants_discord(&profile), Some(Wanted::default()));

    voice.tap = None;
    voice.kind = WidgetKind::DiscordCall(nexus_config::DiscordCallProps::default());
    profile.pages[0].widgets = vec![voice];
    assert!(profile_wants_discord(&profile).unwrap().current_guild);
}

#[test]
fn notifications_badge_and_voice_profiles_keep_discord_connected() {
    let mut config = nexus_config::default_config();
    config.settings.notifications.enabled = true;
    let profile = config.profiles[0].clone();
    assert_eq!(
        wanted(&config, &profile),
        Some(Wanted {
            notifications: true,
            ..Wanted::default()
        })
    );
    config.settings.discord.notifications = false;
    config.settings.discord.mute_indicator = false;
    assert_eq!(wanted(&config, &profile), None);
    config.profiles[0].discord_voice = true;
    assert_eq!(wanted(&config, &profile), Some(Wanted::default()));
}

#[test]
fn notifications_need_the_desktop_switch_too() {
    let mut config = nexus_config::default_config();
    config.settings.discord.mute_indicator = false;
    config.settings.discord.notifications = true;
    let profile = config.profiles[0].clone();
    assert_eq!(wanted(&config, &profile), None);
    config.settings.notifications.enabled = true;
    assert!(wanted(&config, &profile).unwrap().notifications);
    config.settings.notifications.enabled = false;
    config.settings.discord.mute_indicator = true;
    assert!(!wanted(&config, &profile).unwrap().notifications);
}

#[test]
fn a_mic_opened_by_push_to_talk_is_muted_again_after_reconnecting() {
    let discord = Discord::off();
    discord.mute_again();
    assert!(discord.lock().remute_until.is_some());
}

#[test]
fn only_a_repeat_of_a_message_counts_as_its_desktop_copy() {
    let mut recent = RecentMessages::default();
    let t = Instant::now();
    recent.remember(" see you at eight ", t);
    assert!(recent.repeats("see you at eight", t + Duration::from_secs(1)));
    assert!(!recent.repeats("Incoming call", t + Duration::from_secs(1)));
    assert!(!recent.repeats("", t + Duration::from_secs(1)));
    assert!(!recent.repeats("see you at eight", t + Duration::from_secs(6)));
}
