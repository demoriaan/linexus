//! Believable sample data for previews: Studio's widget gallery before the
//! daemon delivers real readings, documentation screenshots and tests.

use crate::live::*;
use crate::Rgb;
use std::sync::Arc;

pub fn sample_live(locale: Locale) -> LiveData {
    let mut live = LiveData {
        time: LocalTime {
            year: 2026,
            month: 10,
            day: 19,
            weekday: 6,
            hour: 21,
            minute: 47,
            second: 12,
        },
        locale,
        ..LiveData::default()
    };
    let metrics: [(&str, f32, Unit, &str, f32); 17] = [
        ("cpu.usage", 23.0, Unit::Percent, "CPU", 100.0),
        ("cpu.temp", 64.0, Unit::Celsius, "CPU temp", 100.0),
        ("cpu.clock", 4650.0, Unit::Megahertz, "CPU clock", 6000.0),
        ("gpu.usage", 71.0, Unit::Percent, "GPU", 100.0),
        ("gpu.temp", 70.0, Unit::Celsius, "GPU temp", 100.0),
        ("gpu.power", 214.0, Unit::Watts, "GPU power", 350.0),
        ("gpu.fan", 1450.0, Unit::Rpm, "GPU fan", 3500.0),
        ("gpu.vram", 62.0, Unit::Percent, "VRAM", 100.0),
        ("gpu.mem_temp", 78.0, Unit::Celsius, "VRAM temp", 100.0),
        ("ram.usage", 58.0, Unit::Percent, "RAM", 100.0),
        (
            "ram.used",
            18.4 * 1024.0 * 1024.0 * 1024.0,
            Unit::Bytes,
            "RAM",
            32.0 * 1024.0 * 1024.0 * 1024.0,
        ),
        ("disk.usage", 47.0, Unit::Percent, "Disk", 100.0),
        (
            "disk.read",
            2.8 * 1024.0 * 1024.0,
            Unit::BytesPerSec,
            "Disk read",
            500.0 * 1024.0 * 1024.0,
        ),
        (
            "disk.write",
            640.0 * 1024.0,
            Unit::BytesPerSec,
            "Disk write",
            500.0 * 1024.0 * 1024.0,
        ),
        (
            "net.down",
            12.4 * 1024.0 * 1024.0,
            Unit::BytesPerSec,
            "Down",
            100.0 * 1024.0 * 1024.0,
        ),
        (
            "net.up",
            320.0 * 1024.0,
            Unit::BytesPerSec,
            "Up",
            100.0 * 1024.0 * 1024.0,
        ),
        ("volume.level", 60.0, Unit::Percent, "Volume", 100.0),
    ];
    for (id, value, unit, label, max) in metrics {
        live.metrics.insert(
            id.into(),
            Metric {
                value,
                unit,
                label: label.into(),
                max,
            },
        );
        let seed = id.len() as f32;
        let history = (0..40)
            .map(|i| {
                let t = i as f32;
                let wave = (t * 0.35 + seed).sin() * 0.55 + (t * 0.9 + seed * 2.0).sin() * 0.3;
                (value * (1.0 + 0.25 * wave)).max(0.0)
            })
            .collect();
        live.history.insert(id.into(), history);
    }
    live.media = Some(MediaInfo {
        player: "Spotify".into(),
        title: "Midnight City".into(),
        artist: "M83".into(),
        album: "Hurry Up, We're Dreaming".into(),
        playing: true,
        position_s: Some(161.0),
        length_s: Some(243.0),
        art: Some(Arc::new(sample_art())),
        art_color: Some(Rgb::hex(0x9a3fd0)),
    });
    live.volume = Some(VolumeInfo {
        percent: 60,
        muted: false,
    });
    live.weather.insert(
        String::new(),
        WeatherInfo {
            location: "Berlin".into(),
            temp_c: 14.0,
            code: 2,
            is_day: true,
            high_c: Some(16.0),
            low_c: Some(9.0),
        },
    );
    for (id, name, on) in [
        ("light.living_room", "Living room", true),
        ("light.desk", "Desk lamp", false),
        ("switch.fan", "Fan", true),
    ] {
        live.entities.insert(
            id.into(),
            EntityInfo {
                name: name.into(),
                state: if on { "on" } else { "off" }.into(),
                unit: None,
                on: Some(on),
            },
        );
    }
    // Matches the Streaming template: recording, mic open, default scene.
    live.obs = Some(ObsInfo {
        scene: "Scene".into(),
        recording: true,
        record_ms: 754_000,
        replay_buffer: true,
        muted: [("Mic/Aux".to_string(), false)].into(),
        ..ObsInfo::default()
    });
    // A few people in voice; you sit in "Gaming" with someone speaking.
    let member = |id: &str, name: &str, speaking: bool, muted: bool| DiscordMember {
        user_id: id.into(),
        name: name.into(),
        speaking,
        muted,
        volume: 100,
        ..DiscordMember::default()
    };
    live.discord = Some(DiscordInfo {
        channel: Some("2".into()),
        guild: Some("1".into()),
        guilds: [(
            "1".to_string(),
            vec![
                DiscordVoiceChannel {
                    id: "2".into(),
                    name: "Gaming".into(),
                    members: vec![
                        member("11", "Axel", false, false),
                        member("12", "Nina", true, false),
                        member("13", "Sam", false, true),
                    ],
                },
                DiscordVoiceChannel {
                    id: "3".into(),
                    name: "Chill".into(),
                    members: vec![
                        member("14", "Léa", false, false),
                        member("15", "Max", false, false),
                    ],
                },
                DiscordVoiceChannel {
                    id: "4".into(),
                    name: "AFK".into(),
                    members: Vec::new(),
                },
            ],
        )]
        .into(),
        input_volume: 80,
        output_volume: 100,
        noise_suppression: true,
        echo_cancellation: true,
        link: Some(DiscordLink {
            connected: true,
            ping_ms: Some(42),
        }),
        ..DiscordInfo::default()
    });
    live.audio = Some(sample_audio(locale));
    live.timers.insert(
        "timer".into(),
        TimerInfo {
            phase: TimerPhase::Running,
            display_ms: 17 * 60_000 + 23_000,
            progress: 0.31,
            stage: "focus",
            round: 2,
            rounds: 4,
        },
    );
    let ping = sample_ping(nexus_config::DEFAULT_PING_HOST);
    // The readings the daemon reports for the settings host, with its
    // full scales.
    for (id, value, unit, label, max) in [
        ("net.ping", ping.last_ms, Unit::Milliseconds, "Ping", 200.0),
        (
            "net.ping.jitter",
            ping.jitter_ms,
            Unit::Milliseconds,
            "Jitter",
            50.0,
        ),
        (
            "net.ping.loss",
            ping.loss_percent,
            Unit::Percent,
            "Packet loss",
            100.0,
        ),
    ] {
        let value = value.unwrap_or_default();
        let history = if id == "net.ping" {
            ping.history.iter().flatten().copied().collect()
        } else {
            vec![value; 40]
        };
        live.history.insert(id.into(), history);
        live.metrics.insert(
            id.into(),
            Metric {
                value,
                unit,
                label: label.into(),
                max,
            },
        );
    }
    live.ping.insert(String::new(), ping);
    live.news = NewsInfo {
        rows: vec![
            NewsRow {
                source: "Linexus".into(),
                headline: "Tap a headline to open it, hold it to hide it".into(),
                seen: false,
                alert: false,
            },
            NewsRow {
                source: "Weather".into(),
                headline: "Clear skies and mild temperatures for the weekend".into(),
                seen: true,
                alert: false,
            },
        ],
        ..NewsInfo::default()
    };
    live
}

/// A minute of probes around 14 ms with one lost, as the daemon reports
/// them for `host`.
pub fn sample_ping(host: &str) -> PingInfo {
    let history: Vec<Option<f32>> = (0..60)
        .map(|i| {
            let t = i as f32;
            let rtt = 14.0
                + (t * 0.45).sin() * 1.6
                + (t * 1.7).sin() * 0.9
                + if i == 33 { 9.0 } else { 0.0 };
            (i != 41).then_some(rtt)
        })
        .collect();
    let answered: Vec<f32> = history.iter().flatten().copied().collect();
    let steps: f32 = answered.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    PingInfo {
        host: host.into(),
        last_ms: answered.last().copied(),
        avg_ms: Some(answered.iter().sum::<f32>() / answered.len() as f32),
        min_ms: answered.iter().copied().reduce(f32::min),
        max_ms: answered.iter().copied().reduce(f32::max),
        jitter_ms: Some(steps / (answered.len() - 1) as f32),
        loss_percent: Some(100.0 / history.len() as f32),
        history,
        ..PingInfo::default()
    }
}

/// Gives ping widgets with a host of their own the sample probes too; the
/// sample data only has the default host.
pub fn fill_ping_samples(live: &mut LiveData, page: &nexus_config::Page) {
    for widget in &page.widgets {
        if let nexus_config::WidgetKind::Ping(props) = &widget.kind {
            let key = crate::ping_key(props);
            if !live.ping.contains_key(key) {
                live.ping.insert(key.to_string(), sample_ping(key));
            }
        }
    }
}

/// A desk with a headset, a screen and speakers, four apps playing and the
/// headset's microphone in use, named in `locale`.
pub fn sample_audio(locale: Locale) -> AudioInfo {
    let device = |node: &str, name: &str, detail: &str, kind: AudioKind, volume: u8| AudioDevice {
        node: node.into(),
        name: name.into(),
        detail: detail.into(),
        kind,
        volume,
        muted: false,
    };
    let app = |name: &str, volume: u8, muted: bool| AudioApp {
        name: name.into(),
        volume,
        muted,
    };
    let [game, chat, screen, speakers, mic, line_in, board] = sample_devices(locale);
    AudioInfo {
        outputs: vec![
            device("a50.game", game, "Astro A50", AudioKind::Headset, 60),
            device("a50.chat", chat, "Astro A50", AudioKind::Headset, 45),
            device("hdmi", screen, "Navi 10 HDMI", AudioKind::Monitor, 80),
            device("spdif", speakers, "S/PDIF", AudioKind::Speaker, 35),
        ],
        inputs: vec![
            device("a50.mic", mic, "Astro A50", AudioKind::Microphone, 80),
            device("analog", line_in, board, AudioKind::Microphone, 100),
        ],
        default_output: Some("a50.game".into()),
        default_input: Some("a50.mic".into()),
        apps: vec![
            app("Firefox", 70, true),
            app("Discord", 100, false),
            app("Spotify", 55, false),
            app("Valheim", 80, false),
        ],
        level: Some(46),
        ..AudioInfo::default()
    }
}

/// The sample's device names: the headset's game and chat outputs, the
/// screen, the speakers, the headset's microphone, the line in and the
/// board it is on.
fn sample_devices(locale: Locale) -> [&'static str; 7] {
    match locale {
        Locale::En => [
            "A50 Game",
            "A50 Chat",
            "Monitor",
            "Speakers",
            "A50 Mic",
            "Line in",
            "Motherboard",
        ],
        Locale::De => [
            "A50 Spiel",
            "A50 Unterhaltung",
            "Monitor",
            "Lautsprecher",
            "A50 Mikro",
            "Analog-Eingang",
            "Mainboard",
        ],
        Locale::Fr => [
            "A50 Jeu",
            "A50 Discussion",
            "Écran",
            "Enceintes",
            "Micro A50",
            "Entrée ligne",
            "Carte mère",
        ],
    }
}

/// Discord on the microphone and OBS on the camera, for the privacy pill.
pub fn sample_privacy(locale: Locale) -> PrivacyInfo {
    PrivacyInfo {
        uses: vec![
            PrivacyEntry {
                app: "Discord".into(),
                camera: false,
                device: sample_devices(locale)[4].into(),
            },
            PrivacyEntry {
                app: "OBS Studio".into(),
                camera: true,
                device: "Droidcam".into(),
            },
        ],
        ..PrivacyInfo::default()
    }
}

/// A desktop notification as the strip shows it, for documentation
/// screenshots.
pub fn sample_notification(locale: Locale) -> Overlay {
    let (app, title, body) = match locale {
        Locale::De => (
            "Kalender",
            "Teamcall in 10 Minuten",
            "Wochenplanung mit dem Designteam",
        ),
        Locale::Fr => (
            "Agenda",
            "Réunion d'équipe dans 10 minutes",
            "Planning hebdo avec l'équipe design",
        ),
        Locale::En => (
            "Calendar",
            "Team call in 10 minutes",
            "Weekly planning with the design team",
        ),
    };
    Overlay::Notification {
        app: app.into(),
        title: title.into(),
        body: body.into(),
        icon: None,
        link: None,
        scroll: false,
    }
}

/// A breaking-news banner with a made-up headline that fits (`long` false)
/// or scrolls, for previews and documentation screenshots.
pub fn sample_alert(locale: Locale, long: bool) -> Overlay {
    let (source, short, long_headline) = match locale {
        Locale::De => (
            "Stadtinfo",
            "Brücke nach Sturmschäden wieder frei",
            "Stromausfall in mehreren Stadtteilen: Der Versorger rechnet mit einer Reparatur \
             bis zum späten Abend und bittet um Geduld",
        ),
        Locale::Fr => (
            "Info Ville",
            "Le pont rouvre après la tempête",
            "Coupure de courant dans plusieurs quartiers : le fournisseur prévoit des \
             réparations jusqu'en fin de soirée et appelle à la patience",
        ),
        Locale::En => (
            "City News",
            "Bridge reopens after storm damage",
            "Power cut across several districts: the utility expects repairs to last into \
             the late evening and asks residents for patience",
        ),
    };
    Overlay::Alert {
        source: source.into(),
        headline: if long { long_headline } else { short }.into(),
        link: None,
        id: "rss:sample:alert".into(),
        marquee: MarqueeStyle::default(),
    }
}

/// Generated gradient "album art" so previews need no image files.
fn sample_art() -> tiny_skia::Pixmap {
    let size = 64;
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    let top = Rgb::hex(0xff7a59);
    let bottom = Rgb::hex(0x6a2ff2);
    for y in 0..size {
        for x in 0..size {
            let t = (x + y) as f32 / (2 * size) as f32;
            let c = top.mix(bottom, t);
            let ring =
                (((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt() - 14.0).abs() < 2.0;
            let c = if ring {
                c.mix(Rgb::new(255, 255, 255), 0.6)
            } else {
                c
            };
            pixmap.pixels_mut()[(y * size + x) as usize] =
                tiny_skia::ColorU8::from_rgba(c.r, c.g, c.b, 255).premultiply();
        }
    }
    pixmap
}
