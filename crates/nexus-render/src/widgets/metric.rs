use super::{metric_icon, press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format;
use crate::live::Metric;
use crate::text::{self, FontFace, TextStyle};
use crate::Rgb;
use nexus_config::{MetricProps, MetricStyle, Widget};

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &MetricProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let color = ctx.accent(widget, palette.metric_color(&props.source));
    press_glow(canvas, b, color, ctx.pressed(widget));
    let metric = ctx.live.metric(&props.source);
    let label = props
        .label
        .clone()
        .or_else(|| metric.map(|m| m.label.clone()))
        .unwrap_or_else(|| fallback_label(&props.source));
    let value_color = value_color(props, metric, ctx);
    let secondary = props
        .secondary
        .as_deref()
        .and_then(|id| ctx.live.metric(id))
        .map(|m| format::short(m.value, m.unit, ctx.live.fahrenheit));

    match props.style {
        MetricStyle::Ring => ring(
            canvas,
            b,
            &label,
            metric,
            secondary,
            color,
            value_color,
            ctx,
        ),
        style => {
            // Header: icon + label left, secondary reading right.
            let caption = ctx.caption();
            canvas.icon(metric_icon(&props.source), b.x + 8.0, 5.0, 12.0, color, 1.0);
            let label_room = b.w
                - 32.0
                - secondary
                    .as_ref()
                    .map_or(0.0, |s| text::measure(s, &caption) + 6.0);
            canvas.text_fit(
                b.x + 24.0,
                15.0,
                &label.to_uppercase(),
                &caption,
                label_room,
            );
            if let Some(secondary) = &secondary {
                canvas.text_right(b.right() - 8.0, 15.0, secondary, &caption.color(color));
            }
            let (number, unit) = number_and_unit(metric, ctx);
            let big = TextStyle::new(FontFace::Bold, 24.0, value_color).tabular();
            let unit_style = TextStyle::new(FontFace::SemiBold, 12.0, palette.text_dim);
            let number_w = text::measure(&number, &big);
            let unit_w = if unit.is_empty() {
                0.0
            } else {
                text::measure(&unit, &unit_style) + 2.0
            };
            let fraction = metric.map(|m| (m.value / m.max.max(f32::EPSILON)).clamp(0.0, 1.0));
            match style {
                MetricStyle::Sparkline => {
                    let value_x = b.right() - 8.0 - unit_w - number_w;
                    let history = history(ctx, &props.source, metric);
                    let graph_right = value_x - 6.0;
                    if graph_right - (b.x + 8.0) >= 28.0 {
                        canvas.sparkline(
                            Bounds::new(b.x + 8.0, 22.0, graph_right - b.x - 8.0, 20.0),
                            &history,
                            color,
                            0.32,
                            1.4,
                        );
                    } else {
                        canvas.sparkline(
                            Bounds::new(b.x + 4.0, 24.0, b.w - 8.0, 20.0),
                            &history,
                            color,
                            0.18,
                            1.0,
                        );
                    }
                    canvas.text(value_x, 39.0, &number, &big);
                    if !unit.is_empty() {
                        canvas.text(value_x + number_w + 2.0, 39.0, &unit, &unit_style);
                    }
                }
                MetricStyle::Bar => {
                    canvas.text(b.x + 8.0, 36.0, &number, &big);
                    if !unit.is_empty() {
                        canvas.text(b.x + 10.0 + number_w, 36.0, &unit, &unit_style);
                    }
                    let track = Bounds::new(b.x + 8.0, 40.0, b.w - 16.0, 4.0);
                    canvas.round_rect(track, 2.0, palette.track, 1.0);
                    if let Some(fraction) = fraction {
                        canvas.round_rect(
                            Bounds::new(track.x, track.y, (track.w * fraction).max(4.0), track.h),
                            2.0,
                            color,
                            1.0,
                        );
                    }
                }
                _ => {
                    let total = number_w + unit_w;
                    let x = (b.center_x() - total / 2.0).max(b.x + 8.0);
                    canvas.text(x, 39.0, &number, &big);
                    if !unit.is_empty() {
                        canvas.text(x + number_w + 2.0, 39.0, &unit, &unit_style);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn ring(
    canvas: &mut Canvas,
    b: Bounds,
    label: &str,
    metric: Option<&Metric>,
    secondary: Option<String>,
    color: Rgb,
    value_color: Rgb,
    ctx: &Ctx,
) {
    let palette = ctx.palette;
    let (cx, cy, r) = (b.x + 26.0, 24.0, 17.0);
    canvas.arc(cx, cy, r, 135.0, 270.0, palette.track, 1.0, 4.0);
    let fraction = metric.map(|m| (m.value / m.max.max(f32::EPSILON)).clamp(0.0, 1.0));
    if let Some(fraction) = fraction {
        canvas.arc(cx, cy, r, 135.0, 270.0 * fraction, color, 1.0, 4.0);
    }
    let (number, unit) = number_and_unit(metric, ctx);
    let inner = TextStyle::new(FontFace::Bold, 14.0, value_color).tabular();
    canvas.text_centered(cx, cy + 5.0, &number, &inner);
    let text_x = b.x + 52.0;
    let room = b.right() - text_x - 6.0;
    canvas.text_fit(text_x, 18.0, &label.to_uppercase(), &ctx.caption(), room);
    let detail = match (&secondary, unit.as_str()) {
        (Some(s), _) => s.clone(),
        (None, "") => String::new(),
        (None, unit) => format!("{number} {unit}"),
    };
    if !detail.is_empty() {
        let style = TextStyle::new(FontFace::Bold, 17.0, palette.text_soft).tabular();
        canvas.text_fit(text_x, 37.0, &detail, &style, room);
    }
}

fn number_and_unit(metric: Option<&Metric>, ctx: &Ctx) -> (String, String) {
    match metric {
        Some(m) => format::value(m.value, m.unit, ctx.live.fahrenheit),
        None => ("–".into(), String::new()),
    }
}

fn value_color(props: &MetricProps, metric: Option<&Metric>, ctx: &Ctx) -> Rgb {
    let palette = ctx.palette;
    let Some(metric) = metric else {
        return palette.text_dim;
    };
    match (props.critical, props.warn) {
        (Some(critical), _) if metric.value >= critical => palette.critical,
        (_, Some(warn)) if metric.value >= warn => palette.warn,
        _ => palette.text,
    }
}

/// History scaled to 0..=1 against the metric's full scale, padded on the
/// left so a fresh start draws a flat line rather than nothing.
fn history(ctx: &Ctx, source: &str, metric: Option<&Metric>) -> Vec<f32> {
    const POINTS: usize = 40;
    let max = metric.map(|m| m.max).unwrap_or(100.0).max(f32::EPSILON);
    let raw = ctx
        .live
        .history
        .get(source)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let tail = &raw[raw.len().saturating_sub(POINTS)..];
    // Throughput has no natural ceiling: scale to the window's peak.
    let scale = match metric.map(|m| m.unit) {
        Some(crate::live::Unit::BytesPerSec) => tail.iter().copied().fold(1.0, f32::max) * 1.15,
        _ => max,
    };
    let mut values: Vec<f32> = tail.iter().map(|v| v / scale).collect();
    if values.is_empty() {
        values.push(metric.map(|m| m.value / scale).unwrap_or(0.0));
    }
    while values.len() < POINTS {
        values.insert(0, values[0]);
    }
    values
}

/// Label for a metric the daemon has not reported yet: the source family
/// (`cpu.usage` -> `CPU`, `hwmon:k10temp/Tctl` -> `Tctl`).
fn fallback_label(source: &str) -> String {
    if let Some(rest) = source.strip_prefix("hwmon:") {
        return rest.rsplit('/').next().unwrap_or(rest).to_string();
    }
    if source.starts_with("battery:") {
        return "Battery".into();
    }
    source.split('.').next().unwrap_or(source).to_uppercase()
}
