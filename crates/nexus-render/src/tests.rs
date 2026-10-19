use super::*;
use nexus_config::{default_config, template_page, PageTemplate, Theme};

fn ui() -> UiState {
    UiState {
        page_count: 5,
        ..UiState::default()
    }
}

/// Writes previews when NEXUS_RENDER_SNAPSHOTS points at a directory.
fn snapshot(name: &str, canvas: &Canvas) {
    if let Some(dir) = std::env::var_os("NEXUS_RENDER_SNAPSHOTS") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        std::fs::write(path, encode_png(canvas.rgba()).unwrap()).unwrap();
    }
}

#[test]
fn every_template_renders_with_sample_and_empty_data() {
    let sample = preview::sample_live(Locale::En);
    let empty = LiveData::default();
    for preset in PRESETS {
        let theme = Theme {
            preset: preset.into(),
            accent: None,
        };
        for template in PageTemplate::ALL {
            let page = template_page(template, "page");
            let full = render_page(&page, &theme, &sample, &ui());
            assert_eq!(full.rgba().len(), WIDTH * HEIGHT * 4);
            assert!(full.rgba().as_chunks::<4>().0.iter().all(|px| px[3] == 255));
            if preset == "midnight" {
                snapshot(&format!("{template:?}").to_lowercase(), &full);
                let blank = render_page(&page, &theme, &empty, &ui());
                snapshot(&format!("{template:?}-empty").to_lowercase(), &blank);
            }
        }
    }
}

#[test]
fn every_icon_the_renderer_and_templates_use_exists() {
    let mut names = vec![
        "music",
        "skip-back",
        "skip-forward",
        "play",
        "pause",
        "volume-x",
        "volume-1",
        "volume-2",
        "newspaper",
        "cloud-sun",
        "cloud-moon",
        "sun",
        "moon",
        "cloud",
        "cloud-fog",
        "cloud-drizzle",
        "cloud-rain",
        "cloud-snow",
        "cloud-rain-wind",
        "cloud-lightning",
        "bell",
        "rotate-ccw",
        "app-window",
        "thermometer",
        "zap",
        "fan",
        "gauge",
        "cpu",
        "gpu",
        "memory-stick",
        "arrow-down",
        "arrow-up",
        "hard-drive",
        "activity",
        "lightbulb",
        "toggle-right",
        "blinds",
        "lock",
        "speaker",
        "radio",
        "sparkles",
        "scroll-text",
        "house",
    ];
    let config = default_config();
    for page in &config.profiles[0].pages {
        for widget in &page.widgets {
            if let nexus_config::WidgetKind::Button(props) = &widget.kind {
                names.push(props.icon.as_deref().unwrap().trim_start_matches("lucide:"));
            }
        }
    }
    let home = template_page(PageTemplate::Home, "home");
    for widget in &home.widgets {
        if let nexus_config::WidgetKind::Entity(props) = &widget.kind {
            names.push(props.icon.as_deref().unwrap().trim_start_matches("lucide:"));
        }
    }
    for name in names {
        assert!(icons::exists(name), "missing icon {name}");
    }
    assert!(icons::list().len() > 1500);
}

#[test]
fn page_indicator_and_overlays_render() {
    let live = preview::sample_live(Locale::De);
    let page = template_page(PageTemplate::Overview, "o");
    let mut canvas = render_page(
        &page,
        &Theme::default(),
        &live,
        &UiState {
            page_index: 1,
            page_count: 5,
            indicator_alpha: 1.0,
            ..UiState::default()
        },
    );
    draw_overlay(
        &mut canvas,
        &Overlay::Notification {
            app: "Thunderbird".into(),
            title: "Neue Nachricht".into(),
            body: "Das Meeting beginnt in 10 Minuten".into(),
            icon: None,
        },
        &Theme::default(),
        &live,
        1.0,
    );
    snapshot("overlay-notification", &canvas);
    let a = render_page(&page, &Theme::default(), &live, &ui());
    let b = render_page(
        &template_page(PageTemplate::Media, "m"),
        &Theme::default(),
        &live,
        &ui(),
    );
    let mid = slide(&a, &b, 320.0);
    snapshot("transition-slide", &mid);
    // Halfway through, the left half shows the old page's right half.
    assert_eq!(&mid.rgba()[..4], &a.rgba()[320 * 4..320 * 4 + 4]);
}

#[test]
fn text_measures_match_drawn_width() {
    let style = TextStyle::new(FontFace::SemiBold, 13.0, Rgb::new(255, 255, 255));
    let w = text::measure("Midnight City", &style);
    assert!(w > 50.0 && w < 120.0, "{w}");
    let fitted = text::ellipsize("A very long title that cannot possibly fit", &style, 80.0);
    assert!(fitted.ends_with('…'));
    assert!(text::measure(&fitted, &style) <= 80.0);
    let tab = style.tabular();
    assert_eq!(text::measure("11", &tab), text::measure("88", &tab));
}

#[test]
fn format_helpers() {
    assert_eq!(
        format::value(12.4 * 1024.0 * 1024.0, Unit::BytesPerSec, false),
        ("12.4".into(), "MB/s".into())
    );
    assert_eq!(
        format::value(64.0, Unit::Celsius, true),
        ("147".into(), "°F".into())
    );
    assert_eq!(format::duration(17 * 60_000 + 23_000), "17:23");
    assert_eq!(format::duration(3_723_000), "1:02:03");
    let t = LocalTime {
        hour: 0,
        minute: 5,
        ..LocalTime::default()
    };
    assert_eq!(format::clock(&t, true, false), ("12:05".into(), "AM"));
    assert_eq!(
        format::expand_tokens("CPU {cpu} / {x}", |k| (k == "cpu").then(|| "5%".into())),
        "CPU 5% / –"
    );
}
