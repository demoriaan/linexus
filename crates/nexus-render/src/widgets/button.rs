use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::icons;
use crate::text::{self, FontFace, TextStyle};
use crate::Rgb;
use nexus_config::{ActionSpec, ButtonProps, ButtonStyle, Widget};

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &ButtonProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let accent = ctx.accent(widget, ctx.series_for(&widget.id));
    let pressed = ctx.pressed(widget);
    let running = props.running_indicator
        && launch_target(widget).is_some_and(|t| ctx.live.running.contains(t));
    let tile = b.inset(3.0, 3.0);

    match props.style {
        ButtonStyle::Tile => {
            canvas.round_rect(tile, 7.0, palette.surface, 1.0);
            if pressed > 0.0 {
                canvas.round_rect(tile, 7.0, accent, 0.28 * pressed);
            }
            let label = props.show_label && !props.label.is_empty() && tile.w >= 40.0;
            let size = if label { 20.0 } else { 24.0 };
            let icon_y = if label { 7.0 } else { 24.0 - size / 2.0 };
            draw_icon(
                canvas,
                props,
                ctx,
                b.center_x() - size / 2.0,
                icon_y,
                size,
                accent,
            );
            if label {
                let style = TextStyle::new(
                    FontFace::SemiBold,
                    12.0,
                    if pressed > 0.0 {
                        palette.text
                    } else {
                        palette.text_soft
                    },
                )
                .tracking(0.2);
                let fitted = text::ellipsize(&props.label, &style, tile.w - 10.0);
                canvas.text_centered(b.center_x(), 40.0, &fitted, &style);
            }
            if running {
                canvas.circle(tile.right() - 7.0, tile.y + 7.0, 2.5, palette.good, 1.0);
            }
        }
        ButtonStyle::Inline => {
            if pressed > 0.0 {
                canvas.round_rect(tile, 7.0, accent, 0.25 * pressed);
            }
            let style = TextStyle::new(FontFace::SemiBold, 15.0, palette.text);
            let label_w = if props.show_label {
                text::measure(&props.label, &style)
            } else {
                0.0
            };
            let has_icon = props.icon.is_some();
            let total = if has_icon { 22.0 } else { 0.0 }
                + if has_icon && props.show_label {
                    8.0
                } else {
                    0.0
                }
                + label_w;
            let mut x = (b.center_x() - total.min(b.w - 16.0) / 2.0).max(b.x + 8.0);
            if has_icon {
                draw_icon(canvas, props, ctx, x, 13.0, 22.0, accent);
                x += 30.0;
            }
            if props.show_label {
                canvas.text_fit(x, 29.0, &props.label, &style, b.right() - 8.0 - x);
            }
            if running {
                canvas.circle(b.center_x(), 44.0, 2.0, palette.good, 1.0);
            }
        }
        ButtonStyle::Icon => {
            if pressed > 0.0 {
                canvas.circle(b.center_x(), 24.0, 20.0, accent, 0.25 * pressed);
            }
            draw_icon(canvas, props, ctx, b.center_x() - 13.0, 11.0, 26.0, accent);
            if running {
                canvas.circle(b.center_x(), 43.0, 2.0, palette.good, 1.0);
            }
        }
    }
}

fn draw_icon(
    canvas: &mut Canvas,
    props: &ButtonProps,
    ctx: &Ctx,
    x: f32,
    y: f32,
    size: f32,
    color: Rgb,
) {
    let Some(icon) = props.icon.as_deref() else {
        return;
    };
    if icon.starts_with("data:") {
        if let Some(image) = icons::data_url_image(icon) {
            icons::draw_image_icon(canvas, &image, x, y, size, 1.0);
        }
    } else if icon.starts_with("app:") {
        match ctx.live.images.get(icon) {
            Some(image) => icons::draw_image_icon(canvas, image, x, y, size, 1.0),
            None => canvas.icon("app-window", x, y, size, color, 1.0),
        }
    } else {
        canvas.icon(icon, x, y, size, color, 1.0);
    }
}

fn launch_target(widget: &Widget) -> Option<&str> {
    match widget.tap.as_ref()? {
        ActionSpec::LaunchApp { target } => Some(target),
        _ => None,
    }
}
