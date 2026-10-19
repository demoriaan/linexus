use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format;
use crate::text::{self, FontFace, TextStyle};
use nexus_config::ClockProps;

pub fn draw(canvas: &mut Canvas, b: Bounds, props: &ClockProps, ctx: &Ctx) {
    let live = ctx.live;
    let hour12 = props.hour12.unwrap_or(live.hour12);
    let (time, suffix) = format::clock(&live.time, hour12, false);
    let caption = match (&props.label, props.show_date) {
        (Some(label), true) => Some(format!(
            "{} · {}",
            label,
            format::date(&live.time, live.locale)
        )),
        (Some(label), false) => Some(label.clone()),
        (None, true) => Some(format::date(&live.time, live.locale)),
        (None, false) => None,
    };

    let size = if b.w >= 150.0 {
        34.0
    } else if b.w >= 96.0 {
        30.0
    } else {
        24.0
    };
    let big = TextStyle::new(FontFace::Bold, size, ctx.palette.text).tabular();
    let small = TextStyle::new(FontFace::SemiBold, size * 0.5, ctx.palette.text_dim).tabular();
    let seconds = props
        .show_seconds
        .then(|| format!(":{:02}", live.time.second));

    let mut width = text::measure(&time, &big);
    if let Some(sec) = &seconds {
        width += text::measure(sec, &small) + 1.0;
    }
    if !suffix.is_empty() {
        width += text::measure(suffix, &small) + 3.0;
    }
    let baseline = if caption.is_some() { 31.0 } else { 36.0 };
    let mut x = (b.center_x() - width / 2.0).round().max(b.x + 4.0);
    x += canvas.text(x, baseline, &time, &big);
    if let Some(sec) = &seconds {
        x += 1.0 + canvas.text(x + 1.0, baseline, sec, &small);
    }
    if !suffix.is_empty() {
        canvas.text(x + 3.0, baseline, suffix, &small);
    }
    if let Some(caption) = caption {
        let style = ctx.caption().tracking(0.8);
        let fitted = text::ellipsize(&caption.to_uppercase(), &style, b.w - 12.0);
        canvas.text_centered(b.center_x(), 44.0, &fitted, &style);
    }
}
