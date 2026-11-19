use super::{snapshot, ui};
use crate::*;
use nexus_config::{Page, PingProps, Theme, Widget, WidgetKind};

/// Three ping widgets side by side: narrow, medium and wide.
const WIDTHS: [u16; 3] = [96, 200, 344];

fn page(host: Option<&str>) -> Page {
    let mut x = 0;
    let widgets = WIDTHS
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            let widget = Widget::new(
                format!("ping{i}"),
                x,
                w,
                WidgetKind::Ping(PingProps {
                    host: host.map(Into::into),
                    ..PingProps::default()
                }),
            );
            x += w;
            widget
        })
        .collect();
    Page {
        id: "ping".into(),
        name: "Ping".into(),
        hidden: false,
        auto_return_seconds: None,
        background: None,
        widgets,
    }
}

/// Pixels of exactly `color` within the widget at `index`.
fn count(canvas: &Canvas, index: usize, color: Rgb) -> usize {
    let left: usize = WIDTHS[..index].iter().map(|&w| usize::from(w)).sum();
    let right = left + usize::from(WIDTHS[index]);
    canvas
        .rgba()
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(i, px)| {
            (left..right).contains(&(i % WIDTH)) && px[..3] == [color.r, color.g, color.b]
        })
        .count()
}

fn render(live: &LiveData, page: &Page) -> Canvas {
    render_page(page, &Theme::default(), live, &ui())
}

#[test]
fn ping_widgets_show_the_round_trip_coloured_by_threshold() {
    let palette = Palette::for_theme(&Theme::default());
    let mut live = preview::sample_live(Locale::En);
    let page = page(None);
    let normal = render(&live, &page);
    snapshot("ping", &normal);
    for i in 0..WIDTHS.len() {
        assert!(
            count(&normal, i, palette.good) > 20,
            "widget {i} shows a green reading"
        );
        assert_eq!(
            count(&normal, i, palette.critical),
            0,
            "widget {i}: 1 of 60 lost is fine"
        );
    }

    let sample = live.ping.get_mut("").unwrap();
    sample.last_ms = Some(75.0);
    let slow = render(&live, &page);
    snapshot("ping-slow", &slow);
    assert!(count(&slow, 1, palette.warn) > 20);
    assert_eq!(count(&slow, 1, palette.good), 0);

    // More than 2 % lost turns it red, however quick the answers are.
    let sample = live.ping.get_mut("").unwrap();
    sample.last_ms = Some(14.0);
    for i in [10, 20, 30] {
        sample.history[i] = None;
    }
    sample.loss_percent = Some(4.0 * 100.0 / 60.0);
    let lossy = render(&live, &page);
    snapshot("ping-lossy", &lossy);
    for i in 0..WIDTHS.len() {
        assert!(count(&lossy, i, palette.critical) > 20, "widget {i}");
        assert_eq!(count(&lossy, i, palette.good), 0, "widget {i}");
    }
}

#[test]
fn lost_probes_are_marked_in_the_graph() {
    let mut live = preview::sample_live(Locale::En);
    let page = page(None);
    let sample = live.ping.get_mut("").unwrap();
    sample.history[41] = Some(14.0);
    let all_answered = render(&live, &page);
    live.ping.get_mut("").unwrap().history[41] = None;
    let one_lost = render(&live, &page);
    let graph_differs = |index: usize| {
        let left: usize = WIDTHS[..index].iter().map(|&w| usize::from(w)).sum();
        let right = left + usize::from(WIDTHS[index]);
        (0..WIDTH * HEIGHT).any(|i| {
            (left..right).contains(&(i % WIDTH))
                && all_answered.rgba()[i * 4..i * 4 + 3] != one_lost.rgba()[i * 4..i * 4 + 3]
        })
    };
    assert!(!graph_differs(0), "the narrow widget has no graph");
    assert!(graph_differs(1) && graph_differs(2));
}

#[test]
fn ping_widgets_say_why_there_is_no_reading() {
    let palette = Palette::for_theme(&Theme::default());
    let mut live = preview::sample_live(Locale::De);
    let page = page(Some("router.lan"));
    // Nothing measured yet for this host.
    let waiting = render(&live, &page);
    snapshot("ping-measuring", &waiting);
    for i in 0..WIDTHS.len() {
        assert_eq!(count(&waiting, i, palette.good), 0);
    }

    live.ping.insert(
        "router.lan".into(),
        PingInfo {
            host: "router.lan".into(),
            unresolved: true,
            ..PingInfo::default()
        },
    );
    let unknown = render(&live, &page);
    snapshot("ping-unknown-host", &unknown);
    assert_ne!(unknown.rgba(), waiting.rgba());
    assert!(count(&unknown, 2, palette.warn) > 20);

    let mut down = preview::sample_ping("router.lan");
    for probe in down.history.iter_mut().rev().take(5) {
        *probe = None;
    }
    down.down = true;
    down.loss_percent = Some(6.0 * 100.0 / 60.0);
    down.tcp = true;
    live.ping.insert("router.lan".into(), down);
    let silent = render(&live, &page);
    snapshot("ping-no-reply-tcp", &silent);
    for i in 0..WIDTHS.len() {
        assert!(count(&silent, i, palette.critical) > 20, "widget {i}");
        assert_eq!(count(&silent, i, palette.good), 0, "widget {i}");
    }
}

#[test]
fn the_gateway_is_called_router_in_every_language() {
    let mut live = preview::sample_live(Locale::Fr);
    let page = page(Some("192.168.0.1"));
    preview::fill_ping_samples(&mut live, &page);
    let by_address = render(&live, &page);
    live.ping.get_mut("192.168.0.1").unwrap().gateway = true;
    let router = render(&live, &page);
    snapshot("ping-router-fr", &router);
    assert_ne!(by_address.rgba(), router.rgba());
    assert_eq!(format::word(format::Word::Router, Locale::Fr), "Routeur");
    for locale in [Locale::En, Locale::De, Locale::Fr] {
        for word in [
            format::Word::Ping,
            format::Word::Jitter,
            format::Word::Loss,
            format::Word::Average,
            format::Word::Measuring,
            format::Word::NoReply,
            format::Word::UnknownHost,
            format::Word::Unknown,
        ] {
            assert!(!format::word(word, locale).is_empty());
        }
    }
    assert_eq!(
        format::value(4.26, Unit::Milliseconds, false),
        ("4.3".into(), "ms".into())
    );
    assert_eq!(format::short(23.6, Unit::Milliseconds, false), "24 ms");
    assert_eq!(format::milliseconds(0.02), "0.1");
}

#[test]
fn wide_ping_widgets_add_the_spread_in_every_theme() {
    let live = preview::sample_live(Locale::En);
    let mut page = page(None);
    page.widgets.truncate(1);
    page.widgets[0].w = 640;
    for preset in PRESETS {
        let theme = Theme {
            preset: preset.into(),
            accent: None,
        };
        let canvas = render_page(&page, &theme, &live, &ui());
        assert!(canvas
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| px[3] == 255));
        snapshot(&format!("ping-wide-{preset}"), &canvas);
    }
}
