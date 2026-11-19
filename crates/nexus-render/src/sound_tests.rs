//! The sound widgets and the privacy pill: layout, hits and previews.

use super::*;
use nexus_config::{
    AudioOutputProps, MicrophoneProps, MixerProps, Page, Theme, Widget, WidgetKind,
};

fn page(widgets: Vec<Widget>) -> Page {
    Page {
        id: "sound".into(),
        name: "Sound".into(),
        hidden: false,
        auto_return_seconds: None,
        background: None,
        widgets,
    }
}

fn widget(x: u16, w: u16, kind: WidgetKind) -> Widget {
    Widget::new(format!("w{x}"), x, w, kind)
}

fn live(locale: Locale) -> LiveData {
    LiveData {
        locale,
        audio: Some(preview::sample_audio(locale)),
        ..LiveData::default()
    }
}

fn render(widgets: Vec<Widget>, live: &LiveData, ui: &UiState) -> Canvas {
    render_page(&page(widgets), &Theme::default(), live, ui)
}

fn snapshot(name: &str, canvas: &Canvas) {
    if let Some(dir) = std::env::var_os("NEXUS_RENDER_SNAPSHOTS") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        std::fs::write(path, encode_png(canvas.rgba()).unwrap()).unwrap();
    }
}

fn pixel(canvas: &Canvas, x: usize, y: usize) -> Rgb {
    let i = (y * WIDTH + x) * 4;
    let px = &canvas.rgba()[i..i + 4];
    Rgb::new(px[0], px[1], px[2])
}

fn outputs() -> WidgetKind {
    WidgetKind::AudioOutput(AudioOutputProps::default())
}

fn mixer(pinned: &[&str]) -> WidgetKind {
    WidgetKind::Mixer(MixerProps {
        pinned: pinned.iter().map(|p| p.to_string()).collect(),
    })
}

fn microphone() -> WidgetKind {
    WidgetKind::Microphone(MicrophoneProps::default())
}

#[test]
fn output_tiles_match_the_mockup() {
    let live = live(Locale::De);
    let full = widget(0, 640, outputs());
    let info = live.audio.as_ref().unwrap();
    // Four tiles of 114 from x 11, 7 apart; the slider from 522 to 629.
    let hits: Vec<_> = [20, 140, 260, 380, 600]
        .iter()
        .map(|x| sound_hit(&full, info, *x).map(|(hit, _)| hit))
        .collect();
    assert_eq!(
        hits,
        vec![
            Some(SoundHit::Output("a50.game".into())),
            Some(SoundHit::Output("a50.chat".into())),
            Some(SoundHit::Output("hdmi".into())),
            Some(SoundHit::Output("spdif".into())),
            Some(SoundHit::OutputVolume),
        ]
    );
    let (_, slider) = sound_hit(&full, info, 600).unwrap();
    let slider = slider.unwrap();
    assert_eq!((slider.left, slider.right), (522.0, 629.0));
    assert_eq!(slider.fraction_at(520.0), 0.0);
    assert_eq!(slider.fraction_at(640.0), 1.0);
    assert_eq!(sound_pages(&full, info), (1, 0));

    let canvas = render(vec![full.clone()], &live, &UiState::default());
    // The device in use has an accent border, the others none.
    assert_eq!(pixel(&canvas, 60, 6), Palette::preset("midnight").accent);
    assert_ne!(pixel(&canvas, 180, 6), Palette::preset("midnight").accent);
    snapshot("audio_output", &canvas);

    let mut dragging = live.clone();
    dragging.audio.as_mut().unwrap().grabbed = Some("a50.game".into());
    let ui = UiState {
        dragging: Some((full.id.clone(), 0.85)),
        ..UiState::default()
    };
    snapshot(
        "audio_output-dragging",
        &render(vec![full.clone()], &dragging, &ui),
    );

    let mut muted = live.clone();
    muted.audio.as_mut().unwrap().outputs[0].muted = true;
    snapshot(
        "audio_output-muted",
        &render(vec![full], &muted, &UiState::default()),
    );
}

#[test]
fn output_tiles_page_when_narrow_or_many() {
    let mut live = live(Locale::En);
    let info = live.audio.as_mut().unwrap();
    info.outputs.push(AudioDevice {
        node: "bt".into(),
        name: "Kopfhörer Küche".into(),
        detail: "Bluetooth".into(),
        kind: AudioKind::Headset,
        volume: 50,
        muted: false,
    });
    let full = widget(0, 640, outputs());
    assert_eq!(sound_pages(&full, live.audio.as_ref().unwrap()), (2, 0));
    snapshot(
        "audio_output-page1",
        &render(vec![full.clone()], &live, &UiState::default()),
    );
    live.audio
        .as_mut()
        .unwrap()
        .pages
        .insert(full.id.clone(), 1);
    let info = live.audio.as_ref().unwrap();
    assert_eq!(
        sound_hit(&full, info, 20).map(|(hit, _)| hit),
        Some(SoundHit::Output("bt".into()))
    );
    snapshot(
        "audio_output-page2",
        &render(vec![full], &live, &UiState::default()),
    );

    // Half the strip: one tile a page next to the slider.
    let half = widget(320, 320, outputs());
    let info = live.audio.as_ref().unwrap();
    assert_eq!(sound_pages(&half, info), (5, 0));
    snapshot(
        "audio_output-320",
        &render(vec![half], &live, &UiState::default()),
    );
    let narrow = widget(440, 200, outputs());
    let canvas = render(vec![narrow.clone()], &live, &UiState::default());
    assert!(sound_hit(&narrow, info, 450).is_some());
    snapshot("audio_output-200", &canvas);
}

#[test]
fn mixer_shows_apps_with_mute_and_pins() {
    let live = live(Locale::De);
    let full = widget(0, 640, mixer(&[]));
    let info = live.audio.as_ref().unwrap();
    // Four columns of 144.25 from x 12, 13 apart.
    assert_eq!(
        sound_hit(&full, info, 20).map(|(hit, _)| hit),
        Some(SoundHit::App("Firefox".into()))
    );
    let (hit, slider) = sound_hit(&full, info, 100).unwrap();
    assert_eq!(hit, SoundHit::AppVolume("Firefox".into()));
    assert_eq!(slider.unwrap().span(), (46, 157));
    assert_eq!(
        sound_hit(&full, info, 480).map(|(hit, _)| hit),
        Some(SoundHit::App("Valheim".into()))
    );
    let canvas = render(vec![full.clone()], &live, &UiState::default());
    snapshot("mixer", &canvas);

    let pinned = widget(0, 640, mixer(&["spotify", "Nothing"]));
    let WidgetKind::Mixer(props) = &pinned.kind else {
        unreachable!()
    };
    let order: Vec<&str> = mixer_apps(props, info)
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(order, ["Spotify", "Firefox", "Discord", "Valheim"]);
    snapshot(
        "mixer-pinned",
        &render(vec![pinned], &live, &UiState::default()),
    );

    // Narrower: fewer columns, the rest on pages.
    let half = widget(320, 320, mixer(&[]));
    assert_eq!(sound_pages(&half, info), (2, 0));
    snapshot("mixer-320", &render(vec![half], &live, &UiState::default()));
    let narrow = widget(480, 160, mixer(&[]));
    assert_eq!(sound_pages(&narrow, info), (4, 0));
    snapshot(
        "mixer-160",
        &render(vec![narrow], &live, &UiState::default()),
    );

    let mut empty = live.clone();
    empty.audio.as_mut().unwrap().apps.clear();
    snapshot(
        "mixer-empty",
        &render(vec![full], &empty, &UiState::default()),
    );
}

#[test]
fn meter_segments_and_states() {
    let palette = Palette::preset("midnight");
    assert_eq!(lit_segments(0), 0);
    assert_eq!(lit_segments(46), 12);
    assert_eq!(lit_segments(100), SEGMENTS);
    let colors: Vec<Rgb> = (0..SEGMENTS)
        .map(|i| segment_color(&palette, i, 25))
        .collect();
    assert_eq!(colors[16], palette.good);
    assert_eq!(colors[17], palette.warn);
    assert_eq!(colors[21], palette.warn);
    assert_eq!(colors[22], palette.critical);
    assert_eq!(segment_color(&palette, 12, 12), palette.track);

    let full = widget(0, 640, microphone());
    let live = live(Locale::De);
    let info = live.audio.as_ref().unwrap();
    let hits: Vec<_> = [30, 200, 440, 560]
        .iter()
        .map(|x| sound_hit(&full, info, *x).map(|(hit, _)| hit))
        .collect();
    assert_eq!(
        hits,
        vec![
            Some(SoundHit::NextInput),
            None,
            Some(SoundHit::InputMute),
            Some(SoundHit::InputVolume),
        ]
    );
    let canvas = render(vec![full.clone()], &live, &UiState::default());
    // Segment 1 lit green at the meter's top row, segment 20 dark.
    assert_eq!(pixel(&canvas, 152, 15), palette.good);
    assert_eq!(pixel(&canvas, 152 + 20 * 10, 15), palette.track);
    snapshot("microphone", &canvas);

    let mut loud = live.clone();
    loud.audio.as_mut().unwrap().level = Some(96);
    snapshot(
        "microphone-loud",
        &render(vec![full.clone()], &loud, &UiState::default()),
    );
    let mut quiet = live.clone();
    quiet.audio.as_mut().unwrap().level = Some(3);
    snapshot(
        "microphone-silent",
        &render(vec![full.clone()], &quiet, &UiState::default()),
    );
    let mut muted = live.clone();
    muted.audio.as_mut().unwrap().inputs[0].muted = true;
    let canvas = render(vec![full.clone()], &muted, &UiState::default());
    assert_eq!(pixel(&canvas, 152, 15), palette.track);
    snapshot("microphone-muted", &canvas);
    for locale in [Locale::En, Locale::Fr] {
        let name = format!("microphone-{locale:?}").to_lowercase();
        snapshot(
            &name,
            &render(vec![full.clone()], &live_with(locale), &UiState::default()),
        );
    }

    let half = widget(320, 320, microphone());
    let canvas = render(vec![half.clone()], &live, &UiState::default());
    assert_eq!(
        sound_hit(&half, info, 630).map(|(hit, _)| hit),
        Some(SoundHit::InputVolume)
    );
    snapshot("microphone-320", &canvas);
    let narrow = widget(440, 200, microphone());
    assert_eq!(
        sound_hit(&narrow, info, 630).map(|(hit, _)| hit),
        Some(SoundHit::InputMute)
    );
    snapshot(
        "microphone-200",
        &render(vec![narrow], &live, &UiState::default()),
    );
}

fn live_with(locale: Locale) -> LiveData {
    live(locale)
}

#[test]
fn without_pipewire_the_widgets_say_so() {
    let live = LiveData::default();
    for (name, kind) in [
        ("audio_output", outputs()),
        ("mixer", mixer(&[])),
        ("microphone", microphone()),
    ] {
        let canvas = render(vec![widget(0, 640, kind)], &live, &UiState::default());
        snapshot(&format!("{name}-unavailable"), &canvas);
    }
}

#[test]
fn privacy_pill_sits_right_and_avoids_the_discord_badge() {
    let palette = Palette::preset("midnight");
    let mut info = preview::sample_privacy(Locale::De);
    let pill = privacy_pill(&info, false, &palette).unwrap();
    assert_eq!((pill.right(), pill.y, pill.h), (628.0, 9.5, 29.0));
    let shifted = privacy_pill(&info, true, &palette).unwrap();
    assert_eq!(shifted.right(), 603.0);
    assert_eq!(
        privacy_hit(&info, false, &palette, 620),
        Some(PrivacyHit::Pill)
    );
    assert_eq!(privacy_hit(&info, false, &palette, 100), None);
    // Only what is drawn takes the touch.
    let left = pill.x.ceil() as u16;
    assert_eq!(
        privacy_hit(&info, false, &palette, left),
        Some(PrivacyHit::Pill)
    );
    assert_eq!(privacy_hit(&info, false, &palette, left - 2), None);
    assert_eq!(privacy_hit(&info, false, &palette, 629), None);

    // Shrunk: the two icons at the right edge, the slider free again.
    let mut compact = info.clone();
    compact.compact = true;
    let small = privacy_pill(&compact, false, &palette).unwrap();
    assert_eq!((small.x, small.right()), (580.0, 628.0));
    assert_eq!(
        privacy_hit(&compact, false, &palette, 600),
        Some(PrivacyHit::Pill)
    );
    assert_eq!(privacy_hit(&compact, false, &palette, 575), None);
    compact.uses.truncate(1);
    let one = privacy_pill(&compact, false, &palette).unwrap();
    assert_eq!((one.w, one.h), (29.0, 29.0));

    let mut overview = preview::sample_live(Locale::De);
    overview.privacy = Some(info.clone());
    let page = nexus_config::template_page(nexus_config::PageTemplate::Overview, "o");
    let canvas = render_page(&page, &Theme::default(), &overview, &UiState::default());
    let border = pixel(&canvas, 627, 24);
    assert!(border.r > 200 && border.g < 120, "{border:?}");
    snapshot("privacy-pill", &canvas);
    let discord = overview.discord.as_mut().unwrap();
    discord.mic_badge = true;
    snapshot(
        "privacy-pill-discord",
        &render_page(&page, &Theme::default(), &overview, &UiState::default()),
    );
    info.uses.truncate(1);
    overview.privacy = Some(info.clone());
    snapshot(
        "privacy-pill-mic",
        &render_page(&page, &Theme::default(), &overview, &UiState::default()),
    );

    let mut open = preview::sample_privacy(Locale::De);
    open.open = true;
    assert_eq!(
        privacy_hit(&open, false, &palette, 600),
        Some(PrivacyHit::Close)
    );
    assert_eq!(
        privacy_hit(&open, false, &palette, 200),
        Some(PrivacyHit::List)
    );
    overview.privacy = Some(open);
    for locale in [Locale::De, Locale::En, Locale::Fr] {
        overview.locale = locale;
        let canvas = render_page(&page, &Theme::default(), &overview, &UiState::default());
        snapshot(&format!("privacy-list-{locale:?}").to_lowercase(), &canvas);
    }
}

/// WCAG contrast of two colours.
fn contrast(a: Rgb, b: Rgb) -> f32 {
    let luminance = |c: Rgb| {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn the_muted_word_reads_in_every_theme() {
    for preset in PRESETS {
        let palette = Palette::preset(preset);
        let red = crate::widgets::sound::muted_text(&palette);
        for background in [palette.bg_top, palette.bg_bottom] {
            let ratio = contrast(red, background);
            assert!(ratio >= 4.5, "{preset}: {ratio:.2} against {background:?}");
        }
    }
}

#[test]
fn widgets_showing_a_message_take_no_touches() {
    let mut live = live(Locale::En);
    let info = live.audio.as_mut().unwrap();
    info.outputs.clear();
    info.apps.clear();
    info.default_input = Some("hidden".into());
    let info = live.audio.as_ref().unwrap();
    for kind in [outputs(), mixer(&[]), microphone()] {
        let full = widget(0, 640, kind);
        for x in (0..640).step_by(10) {
            assert_eq!(sound_hit(&full, info, x), None, "{:?} at {x}", full.kind);
        }
        snapshot(
            &format!("message-{}", full.kind.type_name()),
            &render(vec![full], &live, &UiState::default()),
        );
    }
}

#[test]
fn a_slider_let_go_keeps_its_place_until_pipewire_follows() {
    let mut live = live(Locale::En);
    let full = widget(0, 640, outputs());
    let knob = |live: &LiveData| {
        pixel(
            &render(vec![full.clone()], live, &UiState::default()),
            544,
            16,
        )
    };
    let text = Palette::preset("midnight").text;
    // At 60 % the knob is far right of 20 %.
    assert_ne!(knob(&live), text);
    live.audio.as_mut().unwrap().held = Some(("a50.game".into(), 0.2));
    assert_eq!(knob(&live), text);
    // Another device's value does not move this slider.
    live.audio.as_mut().unwrap().held = Some(("hdmi".into(), 0.2));
    assert_ne!(knob(&live), text);
}

/// The sound widgets and the pill in English and German, dark and light,
/// for review (written when `NEXUS_RENDER_SNAPSHOTS` is set).
#[test]
fn review_snapshots() {
    let page_of = |widgets: Vec<Widget>| page(widgets);
    for locale in [Locale::En, Locale::De] {
        for preset in ["midnight", "daylight"] {
            let theme = Theme {
                preset: preset.into(),
                accent: None,
            };
            let tag = format!("{locale:?}-{preset}").to_lowercase();
            let mut live = preview::sample_live(locale);
            live.audio = Some(preview::sample_audio(locale));
            for (name, kind) in [
                ("audio_output", outputs()),
                ("mixer", mixer(&[])),
                ("microphone", microphone()),
            ] {
                let canvas = render_page(
                    &page_of(vec![widget(0, 640, kind)]),
                    &theme,
                    &live,
                    &UiState::default(),
                );
                snapshot(&format!("review-{tag}-{name}"), &canvas);
            }
            let mut muted = live.clone();
            muted.audio.as_mut().unwrap().inputs[0].muted = true;
            snapshot(
                &format!("review-{tag}-microphone-muted"),
                &render_page(
                    &page_of(vec![widget(0, 640, microphone())]),
                    &theme,
                    &muted,
                    &UiState::default(),
                ),
            );
            let outputs_page = page_of(vec![widget(0, 640, outputs())]);
            let mut privacy = preview::sample_privacy(locale);
            for (name, compact, open) in [
                ("pill", false, false),
                ("pill-shrunk", true, false),
                ("list", false, true),
            ] {
                privacy.compact = compact;
                privacy.open = open;
                live.privacy = Some(privacy.clone());
                snapshot(
                    &format!("review-{tag}-{name}"),
                    &render_page(&outputs_page, &theme, &live, &UiState::default()),
                );
            }
        }
    }
}
