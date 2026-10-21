use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format;
use crate::icons;
use crate::text::{self, FontFace, TextStyle};
use crate::{ObsInfo, Rgb};
use nexus_config::{ActionSpec, ButtonProps, ButtonStyle, ObsCommand, Widget};

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &ButtonProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let mut accent = ctx.accent(widget, ctx.series_for(&widget.id));
    let pressed = ctx.pressed(widget);
    let running = props.running_indicator
        && launch_target(widget).is_some_and(|t| ctx.live.running.contains(t));
    let tile = b.inset(3.0, 3.0);
    let look = obs_look(widget, ctx);
    let lit = look.as_ref().and_then(|look| look.lit);
    if let Some(color) = lit {
        accent = color;
    }
    let alpha = if look.as_ref().is_some_and(|look| look.idle) {
        0.4
    } else {
        1.0
    };
    let label = look
        .as_ref()
        .and_then(|look| look.label.clone())
        .unwrap_or_else(|| props.label.clone());
    let icon = look
        .as_ref()
        .and_then(|look| look.icon)
        .or(props.icon.as_deref());

    match props.style {
        ButtonStyle::Tile => {
            canvas.round_rect(tile, 7.0, palette.surface, 1.0);
            if let Some(color) = lit {
                canvas.round_rect(tile, 7.0, color, 0.2);
            }
            if pressed > 0.0 {
                canvas.round_rect(tile, 7.0, accent, 0.28 * pressed);
            }
            let show_label = props.show_label && !label.is_empty() && tile.w >= 40.0;
            let size = if show_label { 20.0 } else { 24.0 };
            let icon_y = if show_label { 7.0 } else { 24.0 - size / 2.0 };
            let at = (b.center_x() - size / 2.0, icon_y);
            draw_icon(canvas, icon, ctx, at, size, accent, alpha);
            if show_label {
                let style = TextStyle::new(
                    FontFace::SemiBold,
                    12.0,
                    if pressed > 0.0 || lit.is_some() {
                        palette.text
                    } else {
                        palette.text_soft
                    },
                )
                .tracking(0.2)
                .alpha(alpha);
                let fitted = text::ellipsize(&label, &style, tile.w - 10.0);
                canvas.text_centered(b.center_x(), 40.0, &fitted, &style);
            }
            if running {
                canvas.circle(tile.right() - 7.0, tile.y + 7.0, 2.5, palette.good, 1.0);
            }
        }
        ButtonStyle::Inline => {
            if let Some(color) = lit {
                canvas.round_rect(tile, 7.0, color, 0.18);
            }
            if pressed > 0.0 {
                canvas.round_rect(tile, 7.0, accent, 0.25 * pressed);
            }
            let style = TextStyle::new(FontFace::SemiBold, 15.0, palette.text).alpha(alpha);
            let label_w = if props.show_label {
                text::measure(&label, &style)
            } else {
                0.0
            };
            let has_icon = icon.is_some();
            let total = if has_icon { 22.0 } else { 0.0 }
                + if has_icon && props.show_label {
                    8.0
                } else {
                    0.0
                }
                + label_w;
            let mut x = (b.center_x() - total.min(b.w - 16.0) / 2.0).max(b.x + 8.0);
            if has_icon {
                draw_icon(canvas, icon, ctx, (x, 13.0), 22.0, accent, alpha);
                x += 30.0;
            }
            if props.show_label {
                canvas.text_fit(x, 29.0, &label, &style, b.right() - 8.0 - x);
            }
            if running {
                canvas.circle(b.center_x(), 44.0, 2.0, palette.good, 1.0);
            }
        }
        ButtonStyle::Icon => {
            if let Some(color) = lit {
                canvas.circle(b.center_x(), 24.0, 20.0, color, 0.2);
            }
            if pressed > 0.0 {
                canvas.circle(b.center_x(), 24.0, 20.0, accent, 0.25 * pressed);
            }
            let at = (b.center_x() - 13.0, 11.0);
            draw_icon(canvas, icon, ctx, at, 26.0, accent, alpha);
            if running {
                canvas.circle(b.center_x(), 43.0, 2.0, palette.good, 1.0);
            }
        }
    }
}

/// How a button that controls OBS shows OBS's state.
#[derive(Debug, Default, PartialEq)]
struct ObsLook {
    /// Tint while what the button controls is on (recording, live, the
    /// scene on air, a muted source).
    lit: Option<Rgb>,
    /// Shown instead of the label: how long the recording or stream runs.
    label: Option<String>,
    icon: Option<&'static str>,
    /// Faded: OBS is not connected, or the button has nothing to act on.
    idle: bool,
}

fn obs_look(widget: &Widget, ctx: &Ctx) -> Option<ObsLook> {
    let Some(ActionSpec::Obs { command, target }) = widget.tap.as_ref() else {
        return None;
    };
    let Some(obs) = ctx.live.obs.as_ref() else {
        return Some(ObsLook {
            idle: true,
            ..ObsLook::default()
        });
    };
    let palette = ctx.palette;
    let on = |active: bool, color: Rgb| ObsLook {
        lit: active.then_some(color),
        ..ObsLook::default()
    };
    Some(match command {
        ObsCommand::ToggleRecord | ObsCommand::StartRecord | ObsCommand::StopRecord => {
            recording_look(obs, palette.critical, palette.warn)
        }
        ObsCommand::PauseRecord => ObsLook {
            idle: !obs.recording,
            ..on(obs.record_paused, palette.warn)
        },
        ObsCommand::ToggleStream | ObsCommand::StartStream | ObsCommand::StopStream => ObsLook {
            label: obs.streaming.then(|| format::duration(obs.stream_ms)),
            ..on(
                obs.streaming,
                if obs.stream_reconnecting {
                    palette.warn
                } else {
                    palette.critical
                },
            )
        },
        ObsCommand::SetScene => on(
            target.as_deref() == Some(obs.scene.as_str()),
            ctx.accent(widget, palette.accent),
        ),
        ObsCommand::ToggleMute => match target.as_deref().and_then(|name| obs.muted.get(name)) {
            Some(&muted) => ObsLook {
                icon: muted.then(|| muted_icon(widget)).flatten(),
                ..on(muted, palette.critical)
            },
            None => ObsLook {
                idle: true,
                ..ObsLook::default()
            },
        },
        ObsCommand::ToggleReplayBuffer => on(obs.replay_buffer, palette.accent),
        ObsCommand::SaveReplay => ObsLook {
            idle: !obs.replay_buffer,
            ..ObsLook::default()
        },
        ObsCommand::ToggleVirtualCam => on(obs.virtual_cam, palette.accent),
    })
}

fn recording_look(obs: &ObsInfo, recording: Rgb, paused: Rgb) -> ObsLook {
    if !obs.recording {
        return ObsLook::default();
    }
    ObsLook {
        lit: Some(if obs.record_paused { paused } else { recording }),
        label: Some(format::duration(obs.record_ms)),
        ..ObsLook::default()
    }
}

/// The crossed-out twin of a microphone or speaker icon.
fn muted_icon(widget: &Widget) -> Option<&'static str> {
    let nexus_config::WidgetKind::Button(props) = &widget.kind else {
        return None;
    };
    match props.icon.as_deref()?.trim_start_matches("lucide:") {
        "mic" => Some("lucide:mic-off"),
        "volume" | "volume-1" | "volume-2" => Some("lucide:volume-x"),
        "headphones" => Some("lucide:headphone-off"),
        _ => None,
    }
}

fn draw_icon(
    canvas: &mut Canvas,
    icon: Option<&str>,
    ctx: &Ctx,
    (x, y): (f32, f32),
    size: f32,
    color: Rgb,
    alpha: f32,
) {
    let Some(icon) = icon else {
        return;
    };
    if icon.starts_with("data:") {
        if let Some(image) = icons::data_url_image(icon) {
            icons::draw_image_icon(canvas, &image, x, y, size, alpha);
        }
    } else if icon.starts_with("app:") {
        match ctx.live.images.get(icon) {
            Some(image) => icons::draw_image_icon(canvas, image, x, y, size, alpha),
            None => canvas.icon("app-window", x, y, size, color, alpha),
        }
    } else {
        canvas.icon(icon, x, y, size, color, alpha);
    }
}

fn launch_target(widget: &Widget) -> Option<&str> {
    match widget.tap.as_ref()? {
        ActionSpec::LaunchApp { target } => Some(target),
        _ => None,
    }
}
