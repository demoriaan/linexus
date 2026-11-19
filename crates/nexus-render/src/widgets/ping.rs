//! Round trips to a host: the latest one large, a graph of the last probes
//! with the lost ones marked, and jitter and packet loss in the corner.

use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::PingInfo;
use crate::text::{self, FontFace, TextStyle};
use crate::Rgb;
use nexus_config::{PingProps, Widget};

/// Probes the graph has room for, as many as the daemon keeps.
const GRAPH_POINTS: usize = 60;
/// More lost probes than this turn the reading red.
const LOSS_CRITICAL_PERCENT: f32 = 2.0;
/// Narrower widgets show the reading and the loss only.
const NARROW: f32 = 120.0;

/// The `LiveData::ping` key of a widget's host.
pub fn ping_key(props: &PingProps) -> &str {
    props.host.as_deref().unwrap_or("")
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &PingProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let locale = ctx.live.locale;
    let accent = ctx.accent(widget, palette.metric_color("net"));
    press_glow(canvas, b, accent, ctx.pressed(widget));
    let info = ctx.live.ping.get(ping_key(props));
    let narrow = b.w < NARROW;

    // Header: icon, "PING" and the host on the left; on the right the loss,
    // and where there is room the jitter and the minute's spread.
    let gateway = info.is_some_and(|i| i.gateway);
    let icon = if gateway { "router" } else { "radar" };
    canvas.icon(icon, b.x + 8.0, 5.0, 12.0, accent, 1.0);
    let caption = ctx.caption();
    let word = format::word(Word::Ping, locale).to_uppercase();
    let word_x = b.x + 24.0;
    let host_x = word_x + text::measure(&word, &caption) + 5.0;
    let right = b.right() - 8.0;
    let details = details(info, narrow, locale);
    let pick = |room: f32| {
        details
            .iter()
            .find(|parts| detail_width(parts, &caption) <= room)
    };
    let detail = if narrow {
        // No host; the loss takes the word's place when both do not fit.
        let detail = pick(right - host_x - 3.0);
        if detail.is_some() || pick(right - word_x).is_none() {
            canvas.text(word_x, 15.0, &word, &caption);
        }
        detail.or_else(|| pick(right - word_x)).cloned()
    } else {
        canvas.text(word_x, 15.0, &word, &caption);
        let host = match info {
            Some(_) if gateway => format::word(Word::Router, locale).to_string(),
            Some(info) => info.host.clone(),
            None => props.host.clone().unwrap_or_default(),
        };
        let host_style = caption.color(palette.text_soft).tracking(0.2);
        let tag = if info.is_some_and(|i| i.tcp) {
            "TCP"
        } else {
            ""
        };
        let tag_style = caption.alpha(0.75);
        let tag_w = if tag.is_empty() {
            0.0
        } else {
            text::measure(tag, &tag_style) + 5.0
        };
        // As much detail as fits beside the whole host; when not even the
        // loss fits there, the host is shortened instead.
        let full_host_w = text::measure(&host, &host_style) + tag_w;
        let detail = pick(right - host_x - full_host_w - 14.0)
            .or_else(|| pick(right - host_x - 8.0))
            .cloned();
        let detail_w = detail
            .as_ref()
            .map_or(0.0, |parts| detail_width(parts, &caption) + 8.0);
        let room = right - host_x - detail_w - tag_w;
        if room > 0.0 {
            let host_w = canvas.text_fit(host_x, 15.0, &host, &host_style, room);
            if !tag.is_empty() {
                canvas.text(host_x + host_w + 5.0, 15.0, tag, &tag_style);
            }
        }
        detail
    };
    if let Some(parts) = &detail {
        let lossy = info
            .and_then(|i| i.loss_percent)
            .is_some_and(|l| l > LOSS_CRITICAL_PERCENT);
        let loss_color = if lossy {
            palette.critical
        } else {
            palette.text_dim
        };
        let mut x = right;
        // The loss comes last and is red when too many probes are lost.
        for (i, part) in parts.iter().rev().enumerate() {
            let style = if i == 0 {
                caption.color(loss_color)
            } else {
                caption
            };
            x -= canvas.text_right(x, 15.0, part, &style) + DETAIL_GAP;
        }
    }

    // The reading, or what keeps it from showing.
    let state = match info {
        None => Some((Word::Measuring, palette.text_dim)),
        Some(i) if i.unresolved => Some((Word::UnknownHost, palette.warn)),
        Some(i) if i.down => Some((Word::NoReply, palette.critical)),
        Some(i) if i.last_ms.is_none() => Some((Word::Measuring, palette.text_dim)),
        Some(_) => None,
    };
    let big = TextStyle::new(FontFace::Bold, 24.0, palette.text).tabular();
    let unit_style = TextStyle::new(FontFace::SemiBold, 12.0, palette.text_dim);
    let value_left = match (state, info.and_then(|i| i.last_ms)) {
        (None, Some(last)) => {
            let info = info.expect("a reading");
            let number = format::milliseconds(last);
            let style = big.color(reading_color(info, last, props, ctx));
            let number_w = text::measure(&number, &style);
            let unit_w = text::measure("ms", &unit_style) + 2.0;
            let x = if narrow {
                (b.center_x() - (number_w + unit_w) / 2.0).max(b.x + 6.0)
            } else {
                right - number_w - unit_w
            };
            canvas.text(x, 39.0, &number, &style);
            canvas.text(x + number_w + 2.0, 39.0, "ms", &unit_style);
            x
        }
        (state, _) => {
            let (word, color) = state.unwrap_or((Word::Measuring, palette.text_dim));
            let style = TextStyle::new(FontFace::SemiBold, 14.0, color);
            let small = TextStyle {
                size: 12.0,
                ..style
            };
            // Narrow widgets try a smaller size, then a shorter word, before
            // the text gets cut.
            let short = match word {
                Word::UnknownHost => Word::Unknown,
                other => other,
            };
            let (label, style) = if narrow {
                [(word, style), (word, small), (short, style), (short, small)]
                    .into_iter()
                    .map(|(word, style)| (format::word(word, locale), style))
                    .find(|(label, style)| text::measure(label, style) <= b.w - 16.0)
                    .unwrap_or((format::word(short, locale), small))
            } else {
                (format::word(word, locale), style)
            };
            if narrow || info.is_none_or(|i| i.history.is_empty()) {
                let fitted = text::ellipsize(label, &style, b.w - 16.0);
                let x = if narrow {
                    b.center_x() - text::measure(&fitted, &style) / 2.0
                } else {
                    b.x + 24.0
                };
                canvas.text(x, 36.0, &fitted, &style);
                right
            } else {
                // Down: the graph keeps showing what came before.
                let width = text::measure(label, &style).min(b.w * 0.45);
                canvas.text_fit(right - width, 36.0, label, &style, width);
                right - width
            }
        }
    };

    if let (false, Some(info)) = (narrow, info) {
        let graph = Bounds::new(b.x + 8.0, 21.0, value_left - 10.0 - (b.x + 8.0), 22.0);
        if graph.w >= 28.0 {
            draw_graph(canvas, graph, info, accent, palette.critical);
        }
    }
}

/// Space between two parts of the details.
const DETAIL_GAP: f32 = 8.0;

/// The details from the most to the least spelled out, each ending in the
/// loss.
fn details(info: Option<&PingInfo>, narrow: bool, locale: crate::Locale) -> Vec<Vec<String>> {
    let Some(info) = info else {
        return Vec::new();
    };
    let Some(loss) = info.loss_percent else {
        return Vec::new();
    };
    // One decimal where it tells 1.7 % from 2 %.
    let loss_number = if loss < 9.95 && (loss * 10.0).round() % 10.0 != 0.0 {
        format!("{loss:.1}%")
    } else {
        format!("{loss:.0}%")
    };
    let spelled = format!("{} {loss_number}", format::word(Word::Loss, locale));
    let ms = |value: f32| format!("{} ms", format::milliseconds(value));
    let mut out = Vec::new();
    if let (false, Some(jitter)) = (narrow, info.jitter_ms) {
        let jitter_word = format!("{} {}", format::word(Word::Jitter, locale), ms(jitter));
        if let (Some(min), Some(avg), Some(max)) = (info.min_ms, info.avg_ms, info.max_ms) {
            let average = format::word(Word::Average, locale);
            let spread = format!(
                "Min {} · {average} {} · Max {} ms",
                format::milliseconds(min),
                format::milliseconds(avg),
                format::milliseconds(max)
            );
            out.push(vec![spread, jitter_word.clone(), spelled.clone()]);
            out.push(vec![
                format!("{average} {}", ms(avg)),
                jitter_word.clone(),
                spelled.clone(),
            ]);
        }
        out.push(vec![jitter_word, spelled.clone()]);
        out.push(vec![format!("±{}", ms(jitter)), spelled.clone()]);
    }
    out.push(vec![spelled]);
    out.push(vec![loss_number]);
    out
}

fn detail_width(parts: &[String], style: &TextStyle) -> f32 {
    let gaps = parts.len().saturating_sub(1) as f32 * DETAIL_GAP;
    parts
        .iter()
        .map(|part| text::measure(part, style))
        .sum::<f32>()
        + gaps
}

fn reading_color(info: &PingInfo, last: f32, props: &PingProps, ctx: &Ctx) -> Rgb {
    let palette = ctx.palette;
    let lossy = info.loss_percent.is_some_and(|l| l > LOSS_CRITICAL_PERCENT);
    if lossy || last >= props.critical_ms as f32 {
        palette.critical
    } else if last >= props.warn_ms as f32 {
        palette.warn
    } else {
        palette.good
    }
}

/// The last probes right-aligned in `graph`, scaled to the highest round
/// trip (at least 10 ms, so a quiet line does not fill the height), with a
/// red line where a probe was lost.
fn draw_graph(canvas: &mut Canvas, graph: Bounds, info: &PingInfo, color: Rgb, lost: Rgb) {
    let probes = &info.history[info.history.len().saturating_sub(GRAPH_POINTS)..];
    if probes.is_empty() {
        return;
    }
    let peak = probes.iter().flatten().copied().fold(0.0, f32::max);
    let scale = (peak * 1.25).max(10.0);
    // A lost probe has no round trip; the line carries on at the one before
    // (or after) and the red line marks the gap.
    let mut carried = probes.iter().flatten().next().copied().unwrap_or(0.0);
    let values: Vec<f32> = probes
        .iter()
        .map(|probe| {
            if let Some(rtt) = probe {
                carried = *rtt;
            }
            carried / scale
        })
        .collect();
    let step = graph.w / (GRAPH_POINTS - 1) as f32;
    let span = step * (values.len() - 1) as f32;
    let line = Bounds::new(graph.right() - span, graph.y, span, graph.h);
    if probes.iter().any(Option::is_some) {
        canvas.sparkline(line, &values, color, 0.3, 1.4);
    }
    for (i, probe) in probes.iter().enumerate() {
        if probe.is_none() {
            let x = line.x + step * i as f32;
            canvas.fill(Bounds::new(x - 0.75, graph.y, 1.5, graph.h), lost, 0.9);
        }
    }
}
