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
    live.news = NewsInfo {
        rows: vec![
            NewsRow {
                source: "Linexus".into(),
                headline: "Version 1.0 brings app-aware profiles to the strip".into(),
                seen: false,
            },
            NewsRow {
                source: "Weather".into(),
                headline: "Clear skies and mild temperatures for the weekend".into(),
                seen: true,
            },
        ],
        ..NewsInfo::default()
    };
    live
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
