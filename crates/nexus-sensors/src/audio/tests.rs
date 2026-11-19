use super::*;
use serde_json::{json, Value};

fn node(id: u32, state: &str, props: Value, volume: f64, mute: bool) -> Value {
    json!({
        "id": id,
        "type": "PipeWire:Interface:Node",
        "info": {
            "state": state,
            "props": props,
            "params": { "Props": [ { "volume": 1.0, "mute": mute, "channelVolumes": [volume, volume] } ] }
        }
    })
}

fn object(id: u32, kind: &str, props: Value) -> Value {
    json!({ "id": id, "type": format!("PipeWire:Interface:{kind}"), "info": { "props": props } })
}

fn link(id: u32, out: u32, input: u32) -> Value {
    json!({
        "id": id,
        "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": out, "input-node-id": input, "state": "active" }
    })
}

/// A desk like the one this was written on, with umlauts where German
/// PipeWire puts them.
fn fixture() -> Vec<Value> {
    vec![
        object(
            50,
            "Device",
            json!({
                "device.bus": "usb", "device.description": "Astro A50",
                "device.product.name": "Astro A50", "device.profile-set": "usb-gaming-headset.conf"
            }),
        ),
        object(
            48,
            "Device",
            json!({
                "device.bus": "pci", "device.description": "Navi 10 HDMI Audio",
                "device.product.name": "Navi 10 HDMI Audio"
            }),
        ),
        object(
            51,
            "Device",
            json!({
                "device.bus": "pci", "device.description": "Starship/Matisse HD Audio Controller",
                "device.product.name": "Starship/Matisse HD Audio Controller"
            }),
        ),
        object(
            52,
            "Device",
            json!({
                "device.description": "Schleifenschaltung", "device.form-factor": "internal"
            }),
        ),
        object(
            53,
            "Device",
            json!({
                "device.api": "bluez5", "device.bus": "bluetooth",
                "device.description": "Kopfhörer Küche", "device.form-factor": "headphone"
            }),
        ),
        node(
            91,
            "running",
            json!({
                "media.class": "Audio/Sink", "device.id": 50, "device.bus": "usb",
                "node.name": "alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game",
                "node.description": "Astro A50 Spiel", "device.profile.description": "Spiel",
                "priority.session": 828
            }),
            0.216,
            false,
        ),
        node(
            68,
            "suspended",
            json!({
                "media.class": "Audio/Sink", "device.id": 50, "device.bus": "usb",
                "node.name": "alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-chat",
                "node.description": "Astro A50 Unterhaltung", "priority.session": 1100
            }),
            0.091,
            false,
        ),
        node(
            62,
            "suspended",
            json!({
                "media.class": "Audio/Sink", "device.id": 48, "device.bus": "pci",
                "node.name": "alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2",
                "node.description": "Navi 10 HDMI Audio Digital Stereo (HDMI 3)",
                "device.profile.description": "Digital Stereo (HDMI 3)",
                "device.profile.name": "hdmi-stereo-extra2", "priority.session": 632
            }),
            0.512,
            false,
        ),
        node(
            70,
            "suspended",
            json!({
                "media.class": "Audio/Sink", "device.id": 51, "device.bus": "pci",
                "node.name": "alsa_output.pci-0000_13_00.4.iec958-stereo",
                "node.description": "Starship/Matisse HD Audio Controller Digitales Stereo (IEC958)",
                "device.profile.description": "Digitales Stereo (IEC958)",
                "device.profile.name": "iec958-stereo", "priority.session": 736
            }),
            0.042875,
            true,
        ),
        node(
            72,
            "suspended",
            json!({
                "media.class": "Audio/Sink", "device.id": 52,
                "node.name": "alsa_output.platform-snd_aloop.0.analog-stereo",
                "node.description": "Schleifenschaltung Analoges Stereo",
                "alsa.driver_name": "snd_aloop", "api.alsa.card.name": "Loopback",
                "priority.session": 1009
            }),
            1.0,
            false,
        ),
        node(
            95,
            "suspended",
            json!({
                "media.class": "Audio/Sink", "device.id": 53, "device.api": "bluez5",
                "node.name": "bluez_output.AA_BB.1", "node.description": "Kopfhörer Küche",
                "api.bluez5.profile": "a2dp-sink", "priority.session": 1010
            }),
            0.125,
            false,
        ),
        node(
            84,
            "running",
            json!({
                "media.class": "Audio/Source", "device.id": 50, "device.bus": "usb",
                "node.name": "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat",
                "node.description": "Astro A50 Unterhaltung", "priority.session": 2100
            }),
            0.729,
            false,
        ),
        node(
            73,
            "suspended",
            json!({
                "media.class": "Audio/Source", "device.id": 52,
                "node.name": "alsa_input.platform-snd_aloop.0.analog-stereo",
                "node.description": "Schleifenschaltung Analoges Stereo",
                "alsa.driver_name": "snd_aloop", "priority.session": 2009
            }),
            1.0,
            false,
        ),
        node(
            96,
            "suspended",
            json!({
                "media.class": "Audio/Sink", "node.name": "Sink.Übung",
                "node.description": "Nullsenke Übung", "factory.name": "support.null-audio-sink",
                "priority.session": 2000
            }),
            1.0,
            false,
        ),
        object(
            78,
            "Client",
            json!({
                "application.name": "WEBRTC VoiceEngine", "application.process.binary": "Discord",
                "application.process.id": 3648411
            }),
        ),
        object(
            79,
            "Client",
            json!({
                "application.name": "Firefox", "application.process.binary": "firefox",
                "application.process.id": 4100
            }),
        ),
        object(
            81,
            "Client",
            json!({
                "application.name": "ALSA plug-in [mpv]", "application.process.binary": "mpv",
                "application.process.id": 4200
            }),
        ),
        object(
            41,
            "Client",
            json!({
                "application.name": "pipewire", "application.process.binary": "pipewire",
                "application.process.id": 2931
            }),
        ),
        object(
            90,
            "Client",
            json!({
                "application.name": "pw-cat", "application.process.binary": "pw-cat",
                "application.process.id": 5000
            }),
        ),
        object(
            120,
            "Client",
            json!({
                "application.name": "OBS", "application.process.binary": "obs",
                "application.process.id": 6000
            }),
        ),
        // Firefox plays two tabs, one of them paused.
        node(
            200,
            "running",
            json!({
                "media.class": "Stream/Output/Audio", "client.id": 79,
                "application.name": "Firefox", "node.name": "Firefox", "media.name": "Spätzle – YouTube"
            }),
            0.343,
            false,
        ),
        node(
            201,
            "running",
            json!({
                "media.class": "Stream/Output/Audio", "client.id": 79,
                "application.name": "Firefox", "node.name": "Firefox", "pulse.corked": true
            }),
            0.125,
            false,
        ),
        node(
            202,
            "running",
            json!({
                "media.class": "Stream/Output/Audio", "client.id": 78, "node.name": "Discord"
            }),
            1.0,
            true,
        ),
        node(
            203,
            "running",
            json!({
                "media.class": "Stream/Output/Audio", "client.id": 81, "node.name": "mpv"
            }),
            1.0,
            false,
        ),
        // A loopback PipeWire runs for itself.
        node(
            204,
            "running",
            json!({
                "media.class": "Stream/Output/Audio", "client.id": 41, "node.name": "output.loopback"
            }),
            1.0,
            false,
        ),
        // Discord records the headset; our own meter does too.
        node(
            210,
            "running",
            json!({
                "media.class": "Stream/Input/Audio", "client.id": 78, "node.name": "Discord"
            }),
            1.0,
            false,
        ),
        node(
            211,
            "running",
            json!({
                "media.class": "Stream/Input/Audio", "client.id": 90,
                "application.name": "Linexus level meter", "node.name": "linexus-level-meter-777"
            }),
            1.0,
            false,
        ),
        // OBS records what the headset plays, from its monitor.
        node(
            212,
            "running",
            json!({
                "media.class": "Stream/Input/Audio", "client.id": 120,
                "application.name": "OBS", "node.name": "OBS desktop audio"
            }),
            1.0,
            false,
        ),
        // A volume applet peeks at the microphone.
        node(
            213,
            "running",
            json!({
                "media.class": "Stream/Input/Audio", "client.id": 79,
                "node.name": "peak", "stream.monitor": true
            }),
            1.0,
            false,
        ),
        node(
            105,
            "running",
            json!({
                "media.class": "Video/Source", "device.id": 58, "node.nick": "Droidcam",
                "node.description": "Droidcam (V4L2)", "api.v4l2.path": "/dev/video0",
                "node.name": "v4l2_input._sys_devices_virtual_video4linux_video0"
            }),
            1.0,
            false,
        ),
        node(
            130,
            "running",
            json!({
                "media.class": "Stream/Input/Video", "client.id": 120, "node.name": "OBS camera"
            }),
            1.0,
            false,
        ),
        link(300, 200, 91),
        link(301, 201, 91),
        link(302, 202, 68),
        link(303, 203, 91),
        link(304, 84, 210),
        link(305, 84, 211),
        link(306, 91, 212),
        link(307, 84, 213),
        link(308, 105, 130),
        json!({
            "id": 38,
            "type": "PipeWire:Interface:Metadata",
            "props": { "metadata.name": "default" },
            "metadata": [
                { "subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON",
                  "value": { "name": "alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game" } },
                { "subject": 0, "key": "default.audio.source", "type": "Spa:String:JSON",
                  "value": { "name": "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat" } }
            ]
        }),
    ]
}

fn snapshot() -> AudioSnapshot {
    let text = serde_json::to_string(&Value::Array(fixture())).unwrap();
    Graph::from_dump(&text).unwrap().snapshot()
}

#[test]
fn reads_devices_with_names_kinds_and_volumes() {
    let snap = snapshot();
    let names: Vec<(&str, &str, DeviceKind, u8, bool)> = snap
        .outputs
        .iter()
        .map(|d| {
            (
                d.name.as_str(),
                d.detail.as_str(),
                d.kind,
                d.volume,
                d.virtual_device,
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("Nullsenke Übung", "", DeviceKind::Speaker, 100, true),
            (
                "A50 Unterhaltung",
                "Astro A50",
                DeviceKind::Headset,
                45,
                false
            ),
            (
                "Kopfhörer Küche",
                "Bluetooth",
                DeviceKind::Headset,
                50,
                false
            ),
            (
                "Schleifenschaltung Analoges Stereo",
                "Loopback",
                DeviceKind::Speaker,
                100,
                true
            ),
            ("A50 Spiel", "Astro A50", DeviceKind::Headset, 60, false),
            ("S/PDIF", "Starship/Matisse", DeviceKind::Speaker, 35, false),
            ("HDMI 3", "Navi 10 HDMI", DeviceKind::Monitor, 80, false),
        ]
    );
    assert!(
        snap.output("alsa_output.pci-0000_13_00.4.iec958-stereo")
            .unwrap()
            .muted
    );
    assert_eq!(
        snap.default_output.as_deref(),
        Some("alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game")
    );
    assert_eq!(
        snap.default_input.as_deref(),
        Some("alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat")
    );
    let inputs: Vec<(&str, bool, u8)> = snap
        .inputs
        .iter()
        .map(|d| (d.name.as_str(), d.virtual_device, d.volume))
        .collect();
    assert_eq!(
        inputs,
        vec![
            ("A50 Unterhaltung", false, 90),
            ("Schleifenschaltung Analoges Stereo", true, 100),
        ]
    );
    assert!(snap.inputs.iter().all(|d| d.kind == DeviceKind::Microphone));
}

#[test]
fn names_apps_by_what_they_are() {
    let snap = snapshot();
    let apps: Vec<(u32, &str, bool, bool, u8)> = snap
        .playback
        .iter()
        .map(|s| (s.id, s.app.as_str(), s.active, s.muted, s.volume))
        .collect();
    assert_eq!(
        apps,
        vec![
            (200, "Firefox", true, false, 70),
            (201, "Firefox", false, false, 50),
            (202, "Discord", true, true, 100),
            (203, "MPV", true, false, 100),
        ]
    );
    assert_eq!(snap.playback[0].peers, vec![91]);
    assert_eq!(pretty_app_name("obs"), "OBS");
    assert_eq!(pretty_app_name("valheim.exe"), "Valheim");
    assert_eq!(pretty_app_name("/usr/bin/dolphin (deleted)"), "Dolphin");
    assert_eq!(pretty_app_name("steamwebhelper"), "Steam");
    assert_eq!(pretty_app_name("wählscheibe"), "Wählscheibe");
}

#[test]
fn privacy_sees_real_recordings_and_cameras_only() {
    let snap = snapshot();
    let uses = privacy_uses(&snap, &[], 777, Some(5000));
    assert_eq!(
        uses,
        vec![
            PrivacyUse {
                app: "Discord".into(),
                camera: false,
                device: "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat".into(),
            },
            PrivacyUse {
                app: "OBS".into(),
                camera: true,
                device: "Droidcam".into(),
            },
        ]
    );
    // Another Linexus's meter is someone listening.
    let uses = privacy_uses(&snap, &[], 778, None);
    assert!(uses.iter().any(|u| u.app == "Linexus level meter"));

    // A camera found among open files: named by PipeWire when it knows
    // the process, by its program otherwise.
    let opens = [
        CameraOpen {
            pid: 6000,
            program: "obs".into(),
            path: "/dev/video0".into(),
            name: "Loopback video device".into(),
        },
        CameraOpen {
            pid: 7000,
            program: "zoom".into(),
            path: "/dev/video2".into(),
            name: "Integrated Camera".into(),
        },
        CameraOpen {
            pid: 777,
            program: "linexusd".into(),
            path: "/dev/video2".into(),
            name: "Integrated Camera".into(),
        },
    ];
    let uses = privacy_uses(&snap, &opens, 777, Some(5000));
    let cameras: Vec<(&str, &str)> = uses
        .iter()
        .filter(|u| u.camera)
        .map(|u| (u.app.as_str(), u.device.as_str()))
        .collect();
    assert_eq!(
        cameras,
        vec![("OBS", "Droidcam"), ("Zoom", "Integrated Camera")]
    );
}

#[test]
fn monitor_changes_and_removals_fold_in() {
    let mut graph =
        Graph::from_dump(&serde_json::to_string(&Value::Array(fixture())).unwrap()).unwrap();
    // The default output changes, Firefox's tab is gone, the headset louder.
    graph.apply(&json!([
        { "id": 38, "type": "PipeWire:Interface:Metadata", "props": { "metadata.name": "default" },
          "metadata": [ { "subject": 0, "key": "default.audio.sink", "type": "Spa:String:JSON",
                          "value": { "name": "alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2" } } ] },
        { "id": 200, "info": null },
        { "id": 91, "type": "PipeWire:Interface:Node",
          "info": { "change-mask": ["params"], "params": { "Props": [ { "mute": false, "channelVolumes": [0.729, 0.5] } ] } } }
    ]));
    let snap = graph.snapshot();
    assert_eq!(
        snap.default_output.as_deref(),
        Some("alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2")
    );
    // The source stays: only the changed entry came.
    assert!(snap.default_input.is_some());
    assert!(snap.playback.iter().all(|s| s.id != 200));
    let a50 = snap
        .output("alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game")
        .unwrap();
    assert_eq!((a50.volume, a50.name.as_str()), (90, "A50 Spiel"));

    // A metadata entry without value is removed.
    graph.apply(&json!([
        { "id": 38, "type": "PipeWire:Interface:Metadata", "props": { "metadata.name": "default" },
          "metadata": [ { "subject": 0, "key": "default.audio.source", "type": null, "value": null } ] }
    ]));
    assert_eq!(graph.snapshot().default_input, None);
}

#[test]
fn follows_a_stream_of_documents() {
    let first = serde_json::to_string(&Value::Array(fixture())).unwrap();
    let text = format!(
        "{first}\n[\n  {{ \"id\": 202, \"info\": null }}\n]\n[{{\"id\": 999, \"info\": null}}]\n"
    );
    let mut seen = Vec::new();
    let mut skipped = Vec::new();
    follow_graph(
        text.as_bytes(),
        |snap| seen.push(snap.playback.len()),
        |error| skipped.push(error),
    )
    .unwrap();
    // The last document changed nothing and is not reported.
    assert_eq!(seen, vec![4, 3]);
    assert!(skipped.is_empty());
}

#[test]
fn builds_commands() {
    assert_eq!(
        AudioCommand::SetVolume {
            id: 91,
            percent: 42
        }
        .describe(),
        "wpctl set-volume 91 42%"
    );
    assert_eq!(
        AudioCommand::SetMute {
            id: 84,
            muted: None
        }
        .describe(),
        "wpctl set-mute 84 toggle"
    );
    assert_eq!(
        AudioCommand::SetMute {
            id: 202,
            muted: Some(true)
        }
        .describe(),
        "wpctl set-mute 202 1"
    );
    assert_eq!(
        AudioCommand::SetDefault { id: 62 }.describe(),
        "wpctl set-default 62"
    );
    // Like `pactl move-sink-input`: the serial, typed as an id.
    assert_eq!(
        AudioCommand::MoveStream {
            stream: 200,
            serial: Some(192759)
        }
        .describe(),
        "pw-metadata -n default 200 target.object 192759 Spa:Id"
    );
    assert_eq!(
        AudioCommand::MoveStream {
            stream: 200,
            serial: None
        }
        .describe(),
        "pw-metadata -n default -d 200 target.object"
    );
    let meter = meter_args("alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat", 4242);
    assert_eq!(
        &meter[..2],
        [
            "--target",
            "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat"
        ]
    );
    assert!(meter
        .iter()
        .any(|a| a.contains("node.name = \"linexus-level-meter-4242\"")));
    // A peak meter: no Bluetooth headset switches to its call profile.
    assert!(meter.iter().any(|a| a.contains("stream.monitor = true")));
    assert!(meter.windows(2).any(|w| w == ["--rate", "16000"]));
    assert!(meter.windows(2).any(|w| w == ["--channels", "1"]));
}

#[test]
fn maps_peaks_to_the_meter() {
    assert_eq!(level_from_peak(0.0), 0);
    assert_eq!(level_from_peak(1.0), 100);
    // -60 dBFS and below are silence, -30 dBFS is half way.
    assert_eq!(level_from_peak(0.001), 0);
    assert_eq!(level_from_peak(0.0001), 0);
    assert_eq!(level_from_peak(0.031_62), 50);
    assert_eq!(level_from_peak(0.5), 90);
    assert_eq!(peak_of(&[0, -16_384, 1_000]), 0.5);
    assert_eq!(peak_of(&[i16::MIN]), 1.0);

    let mut samples = Vec::new();
    for window in [0i16, 16_384] {
        for _ in 0..METER_WINDOW {
            samples.extend_from_slice(&window.to_le_bytes());
        }
    }
    // Half a window more: incomplete, not reported.
    samples.extend(vec![0u8; METER_WINDOW]);
    let mut levels = Vec::new();
    read_levels(samples.as_slice(), |level| levels.push(level)).unwrap();
    assert_eq!(levels, vec![0, 90]);
}

#[test]
fn finds_cameras_among_open_files() {
    let root = std::env::temp_dir().join(format!("linexus-camera-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let proc_root = root.join("proc");
    let sys_root = root.join("sys");
    let process = |pid: u32, comm: &str, targets: &[&str]| {
        let fd = proc_root.join(pid.to_string()).join("fd");
        std::fs::create_dir_all(&fd).unwrap();
        std::fs::write(
            proc_root.join(pid.to_string()).join("comm"),
            format!("{comm}\n"),
        )
        .unwrap();
        for (n, target) in targets.iter().enumerate() {
            std::os::unix::fs::symlink(target, fd.join(n.to_string())).unwrap();
        }
    };
    process(
        100,
        "droidcam",
        &["/dev/null", "/dev/video0", "socket:[123]"],
    );
    process(200, "obs", &["/dev/video0", "/dev/video0", "/dev/video10"]);
    process(300, "pipewire", &["/dev/video0"]);
    process(400, "linexusd", &["/dev/video0"]);
    process(500, "bash", &["/dev/videofoo", "/dev/snd/pcmC0D0c"]);
    std::fs::create_dir_all(proc_root.join("self")).unwrap();
    let name = sys_root.join("class/video4linux/video0");
    std::fs::create_dir_all(&name).unwrap();
    std::fs::write(name.join("name"), "Loopback video device\n").unwrap();

    let found = camera_users(&proc_root, &sys_root, &[400]);
    let short: Vec<(u32, &str, &str, &str)> = found
        .iter()
        .map(|o| (o.pid, o.program.as_str(), o.path.as_str(), o.name.as_str()))
        .collect();
    assert_eq!(
        short,
        vec![
            (100, "droidcam", "/dev/video0", "Loopback video device"),
            (200, "obs", "/dev/video0", "Loopback video device"),
            (200, "obs", "/dev/video10", "/dev/video10"),
        ]
    );
    let dev = root.join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    assert!(!camera_present(&dev));
    std::fs::write(dev.join("video0"), "").unwrap();
    assert!(camera_present(&dev));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_camera_watch_sees_cameras_opened_closed_and_added() {
    let dev = std::env::temp_dir().join(format!("linexus-camera-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dev);
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("video0"), "").unwrap();
    std::fs::write(dev.join("null"), "").unwrap();
    let mut watch = CameraWatch::new(&dev).unwrap();
    assert!(watch.complete());
    assert!(!watch.changed());
    // Other devices do not count.
    std::fs::write(dev.join("hidraw0"), "").unwrap();
    drop(std::fs::File::open(dev.join("null")).unwrap());
    assert!(!watch.changed());
    // A camera opened and closed does.
    drop(std::fs::File::open(dev.join("video0")).unwrap());
    assert!(watch.changed());
    assert!(!watch.changed());
    // A new camera counts, and is watched from then on.
    std::fs::write(dev.join("video2"), "").unwrap();
    assert!(watch.changed());
    drop(std::fs::File::open(dev.join("video2")).unwrap());
    assert!(watch.changed());
    let _ = std::fs::remove_dir_all(&dev);
}

/// The desk with more objects.
fn desk_with(extra: Vec<Value>) -> AudioSnapshot {
    let mut objects = fixture();
    objects.extend(extra);
    let text = serde_json::to_string(&Value::Array(objects)).unwrap();
    Graph::from_dump(&text).unwrap().snapshot()
}

fn recorder(id: u32, client: u32, name: &str) -> Value {
    node(
        id,
        "running",
        json!({ "media.class": "Stream/Input/Audio", "client.id": client, "node.name": name }),
        1.0,
        false,
    )
}

#[test]
fn privacy_follows_virtual_microphones_to_what_feeds_them() {
    let snap = desk_with(vec![
        // A noise filter PipeWire runs: its capture records the headset,
        // Firefox records its source.
        node(
            400,
            "running",
            json!({
                "media.class": "Stream/Input/Audio", "client.id": 41,
                "node.name": "input.rnnoise", "node.link-group": "filter-chain-1"
            }),
            1.0,
            false,
        ),
        node(
            401,
            "running",
            json!({
                "media.class": "Audio/Source", "client.id": 41, "node.name": "rnnoise_source",
                "node.description": "Rauschfilter", "node.link-group": "filter-chain-1"
            }),
            1.0,
            false,
        ),
        link(410, 84, 400),
        recorder(402, 79, "Firefox"),
        link(411, 401, 402),
        // A loopback of what the headset plays: OBS records that.
        node(
            430,
            "running",
            json!({
                "media.class": "Stream/Input/Audio", "client.id": 41,
                "node.name": "input.loopback-1", "node.link-group": "loopback-1",
                "stream.capture.sink": true
            }),
            1.0,
            false,
        ),
        node(
            431,
            "running",
            json!({
                "media.class": "Audio/Source", "client.id": 41,
                "node.name": "output.loopback-1", "node.link-group": "loopback-1"
            }),
            1.0,
            false,
        ),
        link(433, 91, 430),
        recorder(432, 120, "OBS loopback"),
        link(434, 431, 432),
        // A virtual microphone nothing tells about: it counts.
        node(
            440,
            "running",
            json!({
                "media.class": "Audio/Source/Virtual", "node.name": "virtual_mic",
                "factory.name": "support.null-audio-sink"
            }),
            1.0,
            false,
        ),
        recorder(441, 81, "mpv"),
        link(442, 440, 441),
        // The loopback card's capture side gets what its playback plays.
        recorder(450, 120, "OBS aloop"),
        link(451, 73, 450),
        // EasyEffects: its own stream records the headset, its filters
        // feed its virtual source, and Discord records that.
        object(
            140,
            "Client",
            json!({
                "application.name": "EasyEffects", "application.process.binary": "easyeffects",
                "application.process.id": 8000
            }),
        ),
        recorder(460, 140, "ee_soe_input"),
        node(
            461,
            "running",
            json!({ "client.id": 140, "node.name": "ee_soe_rnnoise" }),
            1.0,
            false,
        ),
        node(
            462,
            "running",
            json!({
                "media.class": "Audio/Source/Virtual", "node.name": "easyeffects_source",
                "factory.name": "support.null-audio-sink"
            }),
            1.0,
            false,
        ),
        link(463, 84, 460),
        link(464, 460, 461),
        link(465, 461, 462),
        recorder(466, 78, "Discord through EasyEffects"),
        link(467, 462, 466),
    ]);
    let monitor_only: Vec<(&str, bool, bool)> = snap
        .inputs
        .iter()
        .map(|d| (d.node.as_str(), d.virtual_device, d.monitor_only))
        .collect();
    for expected in [
        ("rnnoise_source", true, false),
        ("output.loopback-1", true, true),
        ("virtual_mic", true, false),
        ("alsa_input.platform-snd_aloop.0.analog-stereo", true, true),
        ("easyeffects_source", true, false),
        (
            "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat",
            false,
            false,
        ),
    ] {
        assert!(
            monitor_only.contains(&expected),
            "{expected:?} in {monitor_only:?}"
        );
    }
    let uses: Vec<(String, String)> = privacy_uses(&snap, &[], 777, Some(5000))
        .into_iter()
        .filter(|u| !u.camera)
        .map(|u| (u.app, u.device))
        .collect();
    let headset = "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat".to_string();
    assert_eq!(
        uses,
        vec![
            ("Discord".to_string(), headset.clone()),
            ("Firefox".to_string(), "rnnoise_source".to_string()),
            ("MPV".to_string(), "virtual_mic".to_string()),
            ("EasyEffects".to_string(), headset),
            ("Discord".to_string(), "easyeffects_source".to_string()),
        ]
    );
}

fn set_prop(objects: &mut [Value], id: u32, key: &str, value: Value) {
    let object = objects.iter_mut().find(|o| o["id"] == id).unwrap();
    object["info"]["props"][key] = value;
}

#[test]
fn streams_sent_to_an_output_know_which() {
    let mut objects = fixture();
    set_prop(&mut objects, 91, "object.serial", json!(1091));
    set_prop(&mut objects, 62, "object.serial", json!("1062"));
    // Discord's stream names its output itself; mpv's names HDMI, but
    // WirePlumber said to follow the default instead.
    set_prop(&mut objects, 202, "node.target", json!("68"));
    set_prop(
        &mut objects,
        203,
        "target.object",
        json!("alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2"),
    );
    let metadata = objects.iter_mut().find(|o| o["id"] == 38).unwrap();
    let entries = metadata["metadata"].as_array_mut().unwrap();
    entries.extend([
        json!({ "subject": 200, "key": "target.object", "type": "Spa:Id", "value": "1062" }),
        json!({ "subject": 201, "key": "target.node", "type": "Spa:Id", "value": "68" }),
        json!({ "subject": 201, "key": "target.object", "type": "Spa:Id", "value": 1091 }),
        json!({ "subject": 203, "key": "target.node", "type": "Spa:Id", "value": "-1" }),
    ]);
    let text = serde_json::to_string(&Value::Array(objects)).unwrap();
    let snap = Graph::from_dump(&text).unwrap().snapshot();
    let targets: Vec<(u32, Option<u32>)> = snap.playback.iter().map(|s| (s.id, s.target)).collect();
    // target.object wins over target.node.
    assert_eq!(
        targets,
        vec![
            (200, Some(62)),
            (201, Some(91)),
            (202, Some(68)),
            (203, None)
        ]
    );
    let hdmi = snap
        .output("alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2")
        .unwrap();
    assert_eq!(hdmi.serial, Some(1062));
}

#[test]
fn a_bad_byte_or_a_broken_document_does_not_end_following() {
    let mut bytes = serde_json::to_string(&Value::Array(fixture()))
        .unwrap()
        .into_bytes();
    // A Latin-1 "é" in a tab's title, and brackets and quotes in it.
    bytes.extend_from_slice(
        b"\n[\n  { \"id\": 205, \"type\": \"PipeWire:Interface:Node\", \"info\": { \"state\": \
          \"running\", \"props\": { \"media.class\": \"Stream/Output/Audio\", \"client.id\": 79, \
          \"application.name\": \"Firefox\", \"media.name\": \"Caf",
    );
    bytes.push(0xe9);
    bytes.extend_from_slice(b" \\\"Zur Post\\\" ]\" } } }\n]\n");
    // A broken document, then two on one line.
    bytes.extend_from_slice(b"[\n  { \"id\": 202, \"info\": nul }\n]\n");
    bytes.extend_from_slice(
        b"[ { \"id\": 202, \"info\": null } ] [ { \"id\": 203, \"info\": null } ]\n",
    );
    let mut seen = Vec::new();
    let mut skipped = Vec::new();
    follow_graph(
        &bytes[..],
        |snap| seen.push(snap.playback.len()),
        |error| skipped.push(error),
    )
    .unwrap();
    assert_eq!(seen, vec![4, 5, 4, 3]);
    assert_eq!(skipped.len(), 1, "{skipped:?}");
}

#[test]
fn the_mixer_and_the_pill_name_programs_alike() {
    let snap = desk_with(vec![node(
        470,
        "running",
        json!({ "media.class": "Stream/Output/Audio", "client.id": 90, "node.name": "pw-cat" }),
        1.0,
        false,
    )]);
    let stream = snap.playback.iter().find(|s| s.id == 470).unwrap();
    assert_eq!(stream.app, pretty_app_name("pw-cat"));
    assert_eq!(snap.app_of_pid(5000), Some(stream.app.as_str()));
}

fn process_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?.1.chars().next()
}

#[test]
fn a_helper_ends_with_its_handle_and_with_its_thread() {
    let (helper, _out) = Helper::spawn("sleep", &["30".into()]).unwrap();
    let pid = helper.pid();
    assert!(process_state(pid).is_some());
    drop(helper);
    assert_eq!(process_state(pid), None, "killed and reaped");

    // A handle that is never dropped (the daemon was killed): the helper
    // gets SIGTERM when the thread that started it ends.
    let pid = std::thread::spawn(|| {
        let (helper, _out) = Helper::spawn("sleep", &["30".into()]).unwrap();
        let pid = helper.pid();
        std::mem::forget(helper);
        pid
    })
    .join()
    .unwrap();
    let start = std::time::Instant::now();
    while process_state(pid) != Some('Z') && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(process_state(pid), Some('Z'), "ended, waiting to be reaped");
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: waits for our own child, which no one else waits for.
    unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
}
