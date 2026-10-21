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
fn characters_the_font_lacks_are_left_out() {
    assert_eq!(
        text::printable(
            "\u{1F399} Aufnahme \u{1F600}\u{FE0F}  l\u{e4}uft",
            FontFace::Medium
        ),
        "Aufnahme l\u{e4}uft"
    );
    let style = TextStyle::new(FontFace::Bold, 15.0, Rgb::new(255, 255, 255));
    assert_eq!(
        text::measure("A\u{1F399}V", &style),
        text::measure("AV", &style)
    );
    assert_eq!(
        text::measure("A\u{26A0}\u{1F600}V", &style),
        text::measure("AV", &style)
    );
    // Common symbols get ASCII stand-ins; decomposed accents lose the mark.
    assert_eq!(
        text::printable(
            "E\u{266D} \u{2192} F\u{266F} \u{2605}\u{2605}\u{2606} \u{25B2}2 \u{2717}",
            FontFace::Medium
        ),
        "Eb -> F# **\u{B7} ^2 x"
    );
    // Accents written as separate marks join their letter.
    assert_eq!(
        text::printable("Mu\u{308}ller Cafe\u{301} scho\u{308}n", FontFace::Medium),
        "Müller Café schön"
    );
    assert_eq!(
        text::measure("Mu\u{308}ller", &style),
        text::measure("Müller", &style)
    );
    assert_eq!(
        text::printable("\u{2713} ok \u{2717} no", FontFace::Medium),
        "+ ok x no"
    );
    assert_eq!(
        text::measure("A\u{2192}V", &style),
        text::measure("A->V", &style)
    );
    // Missing letters keep a placeholder; missing spaces become spaces.
    assert!(text::measure("A\u{416}V", &style) > text::measure("AV", &style));
    assert_eq!(
        text::printable("mul\u{21B}umesc", FontFace::Medium),
        "mul\u{21B}umesc"
    );
    assert_eq!(
        text::measure("1\u{202F}000", &style),
        text::measure("1 000", &style)
    );
    let mut with = Canvas::new();
    let mut without = Canvas::new();
    with.text(10.0, 30.0, "A\u{1F399}V", &style);
    without.text(10.0, 30.0, "AV", &style);
    assert_eq!(with.rgba(), without.rgba());
}

#[test]
fn svg_images_never_read_files() {
    // A red square in a file the SVG points at must not be loaded.
    let dir = std::env::temp_dir().join(format!("nexus-svg-href-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let inner = dir.join("red.svg");
    std::fs::write(
        &inner,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#,
    )
    .unwrap();
    let outer = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image width="8" height="8" xlink:href="{}"/></svg>"#,
        inner.display()
    );
    let pixmap = icons::rasterize_svg(outer.as_bytes(), 8).unwrap();
    assert!(pixmap.data().iter().all(|&byte| byte == 0));
    let _ = std::fs::remove_dir_all(&dir);
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

#[test]
fn obs_buttons_show_what_obs_does() {
    let page = template_page(PageTemplate::Streaming, "stream");
    let theme = Theme::default();
    let mut live = preview::sample_live(Locale::En);
    // Red (not amber) pixels in the record button (the first 107 px).
    let red = |canvas: &Canvas| {
        canvas
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, px)| {
                let [r, g, b] = [px[0], px[1], px[2]].map(u16::from);
                i % WIDTH < 107 && r > 2 * g + 20 && r > b + 40
            })
            .count()
    };
    let recording = render_page(&page, &theme, &live, &ui());
    snapshot("obs-recording", &recording);
    live.obs.as_mut().unwrap().recording = false;
    let stopped = render_page(&page, &theme, &live, &ui());
    live.obs = None;
    let offline = render_page(&page, &theme, &live, &ui());
    snapshot("obs-offline", &offline);
    assert!(red(&recording) > 30, "{}", red(&recording));
    assert!(red(&stopped) < red(&recording) / 4, "{}", red(&stopped));
    assert_ne!(stopped.rgba(), offline.rgba());
    // Paused: the recording turns amber and its time stands still.
    let mut paused_live = preview::sample_live(Locale::En);
    paused_live.obs.as_mut().unwrap().record_paused = true;
    let paused = render_page(&page, &theme, &paused_live, &ui());
    snapshot("obs-paused", &paused);
    assert!(red(&paused) < red(&recording) / 4, "{}", red(&paused));
    assert_ne!(paused.rgba(), stopped.rgba());

    let mut live = preview::sample_live(Locale::En);
    let palette = Palette::for_theme(&theme);
    let ctx = widgets::Ctx {
        palette: &palette,
        live: &live,
        ui: &ui(),
    };
    assert_eq!(
        widgets::text::expand("{obs.scene} {obs.record} {obs.stream}", &ctx),
        "Scene 12:34 –"
    );
    live.obs = None;
    let ctx = widgets::Ctx {
        palette: &palette,
        live: &live,
        ui: &ui(),
    };
    assert_eq!(widgets::text::expand("{obs.scene}", &ctx), "–");
}
