use super::*;
use nexus_config::{AudioDeviceSettings, AudioOutputProps, MixerProps};
use nexus_sensors::audio::Stream;

fn device(id: u32, node: &str, name: &str, kind: DeviceKind, virtual_device: bool) -> Device {
    Device {
        id,
        node: node.into(),
        name: name.into(),
        kind,
        volume: 50,
        virtual_device,
        serial: Some(1000 + u64::from(id)),
        ..Device::default()
    }
}

fn stream(id: u32, app: &str, volume: u8, active: bool, peers: &[u32]) -> Stream {
    Stream {
        id,
        app: app.into(),
        volume,
        active,
        peers: peers.to_vec(),
        ..Stream::default()
    }
}

pub(crate) fn desk() -> AudioSnapshot {
    AudioSnapshot {
        outputs: vec![
            device(91, "game", "A50 Spiel", DeviceKind::Headset, false),
            device(68, "chat", "A50 Unterhaltung", DeviceKind::Headset, false),
            device(72, "loop", "Schleife", DeviceKind::Speaker, true),
            device(62, "hdmi", "HDMI 3", DeviceKind::Monitor, false),
        ],
        inputs: vec![
            device(84, "mic", "A50 Mikro", DeviceKind::Microphone, false),
            device(73, "loop.in", "Schleife", DeviceKind::Microphone, true),
            device(71, "analog", "Analog", DeviceKind::Microphone, false),
        ],
        default_output: Some("game".into()),
        default_input: Some("mic".into()),
        // Firefox's paused tab and Discord were sent to their outputs by
        // name; the others follow the default.
        playback: vec![
            stream(200, "Firefox", 70, true, &[91]),
            Stream {
                target: Some(91),
                ..stream(201, "Firefox", 90, false, &[91])
            },
            Stream {
                target: Some(68),
                ..stream(202, "Discord", 100, true, &[68])
            },
            stream(203, "Spotify", 40, true, &[91]),
        ],
        ..AudioSnapshot::default()
    }
}

#[test]
fn devices_follow_the_settings() {
    let snapshot = desk();
    let names = |settings: &AudioSettings| -> Vec<String> {
        shown_devices(&snapshot.outputs, settings, Some("game"))
            .into_iter()
            .map(|d| display_name(d, settings))
            .collect()
    };
    // Loopbacks hide by themselves.
    assert_eq!(
        names(&AudioSettings::default()),
        ["A50 Spiel", "A50 Unterhaltung", "HDMI 3"]
    );
    let settings = AudioSettings {
        devices: vec![
            AudioDeviceSettings {
                node: "hdmi".into(),
                name: Some("Monitor".into()),
                hidden: None,
            },
            AudioDeviceSettings {
                node: "chat".into(),
                name: None,
                hidden: Some(true),
            },
            AudioDeviceSettings {
                node: "loop".into(),
                name: Some("Aufnahme".into()),
                hidden: Some(false),
            },
        ],
    };
    assert_eq!(names(&settings), ["Monitor", "Aufnahme", "A50 Spiel"]);
}

#[test]
fn the_device_in_use_shows_unless_hidden_by_name() {
    let mut snapshot = desk();
    snapshot.default_output = Some("loop".into());
    snapshot.default_input = Some("loop.in".into());
    let mut ui = SoundUi::default();
    let now = Instant::now();
    let info = ui
        .info(Some(&snapshot), &AudioSettings::default(), "p", None, now)
        .unwrap();
    // The loopback in use shows, and is the one in use.
    assert_eq!(info.output().map(|d| d.name.as_str()), Some("Schleife"));
    assert_eq!(info.input().map(|d| d.node.as_str()), Some("loop.in"));
    assert_eq!(
        meter_source(&snapshot, &AudioSettings::default()).as_deref(),
        Some("loop.in")
    );
    let outputs = Widget::new(
        "out",
        0,
        640,
        WidgetKind::AudioOutput(AudioOutputProps::default()),
    );
    // Its tile and the slider work.
    assert_eq!(ui.began(&outputs, 600), Some((522, 629)));
    assert_eq!(
        ui.slide_done(Some(&snapshot), 0.3, now),
        [AudioCommand::SetVolume {
            id: 72,
            percent: 30
        }]
    );

    // Hidden by name, it is gone, and so is what touched it.
    let hidden = AudioSettings {
        devices: ["loop", "loop.in"]
            .iter()
            .map(|node| AudioDeviceSettings {
                node: node.to_string(),
                name: None,
                hidden: Some(true),
            })
            .collect(),
    };
    let info = ui.info(Some(&snapshot), &hidden, "p", None, now).unwrap();
    assert!(info.output().is_none());
    assert!(info.input().is_none());
    assert_eq!(meter_source(&snapshot, &hidden), None);
    assert_eq!(ui.began(&outputs, 600), None);
    let mic = Widget::new(
        "mic",
        0,
        640,
        WidgetKind::Microphone(nexus_config::MicrophoneProps::default()),
    );
    for x in [30, 300, 440, 600] {
        assert!(ui.tap(&mic, x, Some(&snapshot), &hidden).is_empty());
    }
}

#[test]
fn apps_group_their_streams_and_linger_five_seconds() {
    let mut snapshot = desk();
    let grouped = group_apps(&snapshot);
    let apps: Vec<(&str, u8, bool)> = grouped
        .iter()
        .map(|(app, active)| (app.name.as_str(), app.volume, *active))
        .collect();
    // Firefox's paused tab does not count towards its volume.
    assert_eq!(
        apps,
        [
            ("Firefox", 70, true),
            ("Discord", 100, true),
            ("Spotify", 40, true)
        ]
    );

    let mut ui = SoundUi::default();
    let start = Instant::now();
    let names = |apps: Vec<AudioApp>| apps.into_iter().map(|a| a.name).collect::<Vec<_>>();
    assert_eq!(
        names(ui.update_apps(group_apps(&snapshot), start)),
        ["Firefox", "Discord", "Spotify"]
    );
    // Discord's stream ends and Spotify pauses.
    snapshot.playback.retain(|s| s.app != "Discord");
    snapshot.playback[2].active = false;
    let later = start + Duration::from_secs(4);
    assert_eq!(
        names(ui.update_apps(group_apps(&snapshot), later)),
        ["Firefox", "Discord", "Spotify"]
    );
    let gone = start + APP_LINGER;
    assert_eq!(
        names(ui.update_apps(group_apps(&snapshot), gone)),
        ["Firefox"]
    );
    // A paused app that was never heard does not appear.
    snapshot
        .playback
        .push(stream(210, "Steam", 50, false, &[91]));
    assert_eq!(
        names(ui.update_apps(group_apps(&snapshot), gone)),
        ["Firefox"]
    );
    // Back again: at the end, where new apps go.
    snapshot
        .playback
        .push(stream(211, "Discord", 80, true, &[68]));
    assert_eq!(
        names(ui.update_apps(group_apps(&snapshot), gone)),
        ["Firefox", "Discord"]
    );
}

#[test]
fn picking_an_output_takes_along_what_was_sent_to_the_old_one() {
    let snapshot = desk();
    // WirePlumber moves Firefox's playing tab and Spotify with the
    // default; the tab sent to the headset by name goes along by serial;
    // Discord, sent to the chat output, stays.
    assert_eq!(
        select_output(&snapshot, "hdmi"),
        [
            AudioCommand::SetDefault { id: 62 },
            AudioCommand::MoveStream {
                stream: 201,
                serial: Some(1062)
            },
        ]
    );
    let mut no_serial = snapshot.clone();
    no_serial.outputs[3].serial = None;
    assert_eq!(
        select_output(&no_serial, "hdmi")[1],
        AudioCommand::MoveStream {
            stream: 201,
            serial: None
        }
    );
    assert!(select_output(&snapshot, "game").is_empty());
    assert!(select_output(&snapshot, "gone").is_empty());

    let settings = AudioSettings::default();
    let next = |devices: &[&str], snapshot: &AudioSnapshot| {
        let devices: Vec<String> = devices.iter().map(|d| d.to_string()).collect();
        output_action(&devices, snapshot, &settings).map(|c| c.first().cloned())
    };
    // One device: that one. Several: the next. None: all shown in turn.
    assert_eq!(
        next(&["hdmi"], &snapshot),
        Ok(Some(AudioCommand::SetDefault { id: 62 }))
    );
    assert_eq!(
        next(&["game", "hdmi"], &snapshot),
        Ok(Some(AudioCommand::SetDefault { id: 62 }))
    );
    assert_eq!(
        next(&["hdmi", "chat"], &snapshot),
        Ok(Some(AudioCommand::SetDefault { id: 62 }))
    );
    assert_eq!(
        next(&[], &snapshot),
        Ok(Some(AudioCommand::SetDefault { id: 68 }))
    );
    let mut on_hdmi = snapshot.clone();
    on_hdmi.default_output = Some("hdmi".into());
    // The loopback is hidden, so the turn wraps to the headset.
    assert_eq!(
        next(&[], &on_hdmi),
        Ok(Some(AudioCommand::SetDefault { id: 91 }))
    );
    assert!(next(&["gone"], &snapshot).is_err());
}

#[test]
fn touches_on_the_widgets_become_commands() {
    let snapshot = desk();
    let settings = AudioSettings::default();
    let mut ui = SoundUi::default();
    let now = Instant::now();
    ui.info(Some(&snapshot), &settings, "p", Some(40), now)
        .unwrap();

    let outputs = Widget::new(
        "out",
        0,
        640,
        WidgetKind::AudioOutput(AudioOutputProps::default()),
    );
    assert_eq!(
        ui.tap(&outputs, 140, Some(&snapshot), &settings).first(),
        Some(&AudioCommand::SetDefault { id: 68 })
    );
    // The slider grabs the finger and sets the volume of the device in use.
    let span = ui.began(&outputs, 600).unwrap();
    assert_eq!(span, (401, 629));
    let fraction = ui.slide(&outputs, 629).unwrap();
    assert_eq!(fraction, 1.0);
    assert_eq!(
        ui.slide_done(Some(&snapshot), 0.42, now),
        [AudioCommand::SetVolume {
            id: 91,
            percent: 42
        }]
    );
    // The knob stays where it was let go until PipeWire reports the new
    // volume, or one and a half seconds pass.
    let held = |ui: &mut SoundUi, snapshot: &AudioSnapshot, at| {
        ui.info(Some(snapshot), &settings, "p", None, at)
            .unwrap()
            .held
    };
    assert_eq!(
        held(&mut ui, &snapshot, now + Duration::from_millis(500)),
        Some(("game".to_string(), 0.42))
    );
    assert!(ui.next_change(now).is_some());
    assert_eq!(held(&mut ui, &snapshot, now + SLIDER_HOLD), None);
    ui.began(&outputs, 600).unwrap();
    ui.slide_done(Some(&snapshot), 0.42, now);
    let mut reported = snapshot.clone();
    reported.outputs[0].volume = 42;
    assert_eq!(held(&mut ui, &reported, now), None);

    let mixer = Widget::new("mix", 0, 640, WidgetKind::Mixer(MixerProps::default()));
    ui.info(Some(&snapshot), &settings, "p", None, now);
    // The letter mutes every stream of the app.
    assert_eq!(
        ui.tap(&mixer, 20, Some(&snapshot), &settings),
        [
            AudioCommand::SetMute {
                id: 200,
                muted: Some(true)
            },
            AudioCommand::SetMute {
                id: 201,
                muted: Some(true)
            },
        ]
    );
    assert!(ui.began(&mixer, 20).is_none());
    assert!(ui.began(&mixer, 100).is_some());
    assert_eq!(
        ui.slide_done(Some(&snapshot), 0.5, now),
        [
            AudioCommand::SetVolume {
                id: 200,
                percent: 50
            },
            AudioCommand::SetVolume {
                id: 201,
                percent: 50
            },
        ]
    );

    let mic = Widget::new(
        "mic",
        0,
        640,
        WidgetKind::Microphone(nexus_config::MicrophoneProps::default()),
    );
    // The tile takes the next real input, past the hidden loopback.
    assert_eq!(
        ui.tap(&mic, 30, Some(&snapshot), &settings),
        [AudioCommand::SetDefault { id: 71 }]
    );
    assert_eq!(
        ui.tap(&mic, 440, Some(&snapshot), &settings),
        [AudioCommand::SetMute {
            id: 84,
            muted: None
        }]
    );
    assert_eq!(meter_source(&snapshot, &settings).as_deref(), Some("mic"));
    let mut muted = snapshot.clone();
    muted.inputs[0].muted = true;
    assert_eq!(meter_source(&muted, &settings), None);
}

#[test]
fn swipes_turn_pages_while_there_are_more() {
    let mut snapshot = desk();
    for n in 0..4 {
        snapshot.outputs.push(device(
            100 + n,
            &format!("extra{n}"),
            "Extra",
            DeviceKind::Speaker,
            false,
        ));
    }
    let settings = AudioSettings::default();
    let mut ui = SoundUi::default();
    let now = Instant::now();
    ui.info(Some(&snapshot), &settings, "p", None, now);
    let outputs = Widget::new(
        "out",
        0,
        640,
        WidgetKind::AudioOutput(AudioOutputProps::default()),
    );
    // Seven devices, four a page: forward turns the widget's page...
    ui.began(&outputs, 100);
    assert!(ui.drag_pages(60));
    assert!(ui.drag_pages(-10), "decided by the first move");
    assert!(ui.drag_end(Some(true)));
    assert_eq!(
        ui.info(Some(&snapshot), &settings, "p", None, now)
            .unwrap()
            .page("out"),
        1
    );
    // A widget of the same id on another page keeps its own page.
    assert_eq!(
        ui.info(Some(&snapshot), &settings, "q", None, now)
            .unwrap()
            .page("out"),
        0
    );
    ui.info(Some(&snapshot), &settings, "p", None, now);
    // ...and on the last page forward is the strip's again.
    ui.began(&outputs, 100);
    assert!(!ui.drag_pages(60));
    assert!(!ui.drag_end(Some(true)));
    ui.began(&outputs, 100);
    assert!(ui.drag_pages(-60));
    assert!(ui.drag_end(Some(false)));
    assert_eq!(
        ui.info(Some(&snapshot), &settings, "p", None, now)
            .unwrap()
            .page("out"),
        0
    );
}

#[test]
fn the_privacy_pill_opens_and_closes() {
    let palette = nexus_render::Palette::preset("midnight");
    let snapshot = desk();
    let settings = AudioSettings {
        devices: vec![AudioDeviceSettings {
            node: "mic".into(),
            name: Some("Headset-Mikro".into()),
            hidden: None,
        }],
    };
    let uses = || {
        vec![PrivacyUse {
            app: "Discord".into(),
            camera: false,
            device: "mic".into(),
        }]
    };
    let mut ui = SoundUi::default();
    let now = Instant::now();
    let info = ui
        .privacy(uses(), Some(&snapshot), &settings, false, now)
        .unwrap();
    assert_eq!(info.uses[0].device, "Headset-Mikro");
    assert!(!info.open);
    // A touch beside the pill is the page's.
    assert!(!ui.privacy_began(&palette, 100));
    assert!(ui.privacy_began(&palette, 620));
    assert!(ui.privacy_touching());
    assert!(ui.privacy_ended(now));
    assert!(
        ui.privacy(uses(), Some(&snapshot), &settings, false, now)
            .unwrap()
            .open
    );
    // The open list takes every touch; its button closes it.
    assert!(ui.privacy_began(&palette, 100));
    assert!(!ui.privacy_ended(now));
    assert!(ui.privacy_began(&palette, 600));
    assert!(ui.privacy_ended(now));
    assert!(
        !ui.privacy(uses(), Some(&snapshot), &settings, false, now)
            .unwrap()
            .open
    );
    // Named for five seconds after the apps change, then icons only.
    let shrunk = now + PILL_NAMES;
    let info = ui
        .privacy(uses(), Some(&snapshot), &settings, false, shrunk)
        .unwrap();
    assert!(info.compact);
    assert_eq!(ui.next_change(shrunk), None);
    // The shrunk pill is smaller: beside it is the page's again.
    assert!(!ui.privacy_began(&palette, 560));
    assert!(ui.privacy_began(&palette, 610));
    ui.reset_touch();
    let mut more = uses();
    more.push(PrivacyUse {
        app: "OBS".into(),
        camera: true,
        device: "Droidcam".into(),
    });
    let info = ui
        .privacy(more, Some(&snapshot), &settings, false, shrunk)
        .unwrap();
    assert!(!info.compact, "a new app is named again");
    assert_eq!(ui.next_change(shrunk), Some(PILL_NAMES));

    // Left open, it closes by itself; when nobody listens, it goes.
    ui.privacy_began(&palette, 620);
    ui.privacy_ended(now);
    let late = now + PRIVACY_LIST;
    assert!(
        !ui.privacy(uses(), Some(&snapshot), &settings, false, late)
            .unwrap()
            .open
    );
    assert!(ui
        .privacy(Vec::new(), Some(&snapshot), &settings, false, late)
        .is_none());
    assert!(!ui.privacy_began(&palette, 620));
}
