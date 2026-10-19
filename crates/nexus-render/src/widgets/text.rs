use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format;
use crate::icons;
use crate::text::{FontFace, TextStyle};
use nexus_config::{CommandProps, ImageFit, ImageProps, TextAlign, TextProps, TextSize, Widget};

pub fn draw_text(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &TextProps, ctx: &Ctx) {
    let color = ctx.accent(widget, ctx.palette.text);
    press_glow(canvas, b, color, ctx.pressed(widget));
    let text = expand(&props.text, ctx);
    let size = match props.size {
        TextSize::Small => 14.0,
        TextSize::Medium => 19.0,
        TextSize::Large => 28.0,
    };
    let style = TextStyle::new(FontFace::Bold, size, color).tabular();
    let baseline = if props.caption.is_some() {
        39.0
    } else {
        24.0 + size * 0.36
    };
    if let Some(caption) = &props.caption {
        place(
            canvas,
            b,
            16.0,
            &caption.to_uppercase(),
            &ctx.caption(),
            props.align,
        );
    }
    place(canvas, b, baseline, &text, &style, props.align);
}

pub fn draw_command(
    canvas: &mut Canvas,
    b: Bounds,
    widget: &Widget,
    props: &CommandProps,
    ctx: &Ctx,
) {
    let color = ctx.accent(widget, ctx.palette.text);
    press_glow(canvas, b, color, ctx.pressed(widget));
    let output = ctx
        .live
        .commands
        .get(&widget.id)
        .map(|s| s.lines().next().unwrap_or_default().trim().to_string())
        .unwrap_or_else(|| "…".into());
    let style = TextStyle::new(FontFace::Bold, 19.0, color).tabular();
    match &props.label {
        Some(label) => {
            place(
                canvas,
                b,
                16.0,
                &label.to_uppercase(),
                &ctx.caption(),
                TextAlign::Left,
            );
            place(canvas, b, 39.0, &output, &style, TextAlign::Left);
        }
        None => place(canvas, b, 31.0, &output, &style, TextAlign::Left),
    }
}

pub fn draw_image(canvas: &mut Canvas, b: Bounds, props: &ImageProps) {
    let Some(image) = icons::data_url_image(&props.src) else {
        return;
    };
    let target = b.inset(2.0, 2.0);
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let rect = match props.fit {
        ImageFit::Stretch => target,
        ImageFit::Fit => {
            let s = (target.w / iw).min(target.h / ih);
            Bounds::new(
                target.center_x() - iw * s / 2.0,
                24.0 - ih * s / 2.0,
                iw * s,
                ih * s,
            )
        }
        ImageFit::Fill => {
            let s = (target.w / iw).max(target.h / ih);
            Bounds::new(
                target.center_x() - iw * s / 2.0,
                24.0 - ih * s / 2.0,
                iw * s,
                ih * s,
            )
        }
    };
    let previous = canvas.clip_x(target.x, target.right());
    canvas.draw_image(rect, image.as_ref().as_ref(), 4.0, 1.0);
    canvas.set_clip(previous);
}

fn place(
    canvas: &mut Canvas,
    b: Bounds,
    baseline: f32,
    text: &str,
    style: &TextStyle,
    align: TextAlign,
) {
    let fitted = crate::text::ellipsize(text, style, b.w - 16.0);
    match align {
        TextAlign::Left => {
            canvas.text(b.x + 8.0, baseline, &fitted, style);
        }
        TextAlign::Center => {
            canvas.text_centered(b.center_x(), baseline, &fitted, style);
        }
        TextAlign::Right => {
            canvas.text_right(b.right() - 8.0, baseline, &fitted, style);
        }
    }
}

/// Replaces `{metric.id}` tokens with the current reading.
pub fn expand(template: &str, ctx: &Ctx) -> String {
    format::expand_tokens(template, |key| {
        let metric = ctx.live.metric(key)?;
        Some(format::short(
            metric.value,
            metric.unit,
            ctx.live.fahrenheit,
        ))
    })
}
