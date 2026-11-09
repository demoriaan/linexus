use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::text::{FontFace, TextStyle};
use nexus_config::{VolumeProps, VolumeSource, Widget};

const ICON_W: f32 = 34.0;
const RIGHT_PAD: f32 = 14.0;

fn track_range(widget_x: f32, widget_w: f32) -> (f32, f32) {
    (widget_x + ICON_W + 6.0, widget_x + widget_w - RIGHT_PAD)
}

/// Slider position (0..=1) for a finger at absolute strip X, or `None` when
/// the finger is on the speaker icon (which toggles mute).
pub fn volume_fraction_at(widget_x: u16, widget_w: u16, x: u16) -> Option<f32> {
    let (left, right) = track_range(f32::from(widget_x), f32::from(widget_w));
    let x = f32::from(x);
    if x < left - 6.0 {
        return None;
    }
    Some(((x - left) / (right - left).max(1.0)).clamp(0.0, 1.0))
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &VolumeProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let accent = ctx.accent(widget, palette.series[0]);
    press_glow(canvas, b, accent, ctx.pressed(widget) * 0.5);
    let level = level(props.source, ctx);
    let known = level.is_some();
    let level = level.unwrap_or_default();
    let dragging = ctx
        .ui
        .dragging
        .as_ref()
        .filter(|(id, _)| *id == widget.id)
        .map(|(_, v)| *v);
    let fraction =
        dragging.unwrap_or(f32::from(level.percent.min(level.max)) / f32::from(level.max));
    let volume = level;
    let icon = match props.source {
        VolumeSource::DiscordInput if !known || volume.muted => "mic-off",
        VolumeSource::DiscordInput => "mic",
        VolumeSource::DiscordOutput if !known || volume.muted => "headphone-off",
        VolumeSource::DiscordOutput => "headphones",
        VolumeSource::System if !known || volume.muted || fraction <= 0.0 => "volume-x",
        VolumeSource::System if fraction < 0.5 => "volume-1",
        VolumeSource::System => "volume-2",
    };
    let icon_color = if volume.muted {
        palette.text_dim
    } else {
        palette.text
    };
    canvas.icon(icon, b.x + 10.0, 14.0, 20.0, icon_color, 1.0);

    let (left, right) = track_range(b.x, b.w);
    let y = if props.show_value { 20.0 } else { 24.0 };
    let track = Bounds::new(left, y - 2.0, right - left, 4.0);
    canvas.round_rect(track, 2.0, palette.track, 1.0);
    let fill_color = if volume.muted {
        palette.text_dim
    } else {
        accent
    };
    let knob_x = left + track.w * fraction;
    canvas.round_rect(
        Bounds::new(left, track.y, knob_x - left, track.h),
        2.0,
        fill_color,
        1.0,
    );
    if known || dragging.is_some() {
        let knob_r = if dragging.is_some() { 7.5 } else { 6.0 };
        canvas.circle(knob_x, y, knob_r, palette.text, 1.0);
    }

    if props.show_value {
        let caption = ctx.caption();
        let label = match props.source {
            _ if volume.muted => format::word(Word::Muted, ctx.live.locale).to_uppercase(),
            VolumeSource::System => format::word(Word::Volume, ctx.live.locale).to_uppercase(),
            VolumeSource::DiscordInput => {
                format::word(Word::Microphone, ctx.live.locale).to_uppercase()
            }
            VolumeSource::DiscordOutput => "DISCORD".to_string(),
        };
        canvas.text(left, 41.0, &label, &caption);
        let value = TextStyle::new(FontFace::Bold, 13.0, palette.text_soft).tabular();
        let shown = if known || dragging.is_some() {
            format!("{}%", (fraction * f32::from(volume.max)).round())
        } else {
            "–".into()
        };
        canvas.text_right(right, 41.0, &shown, &value);
    }
}

/// A slider's reading: its value, its top (100, or 200 for Discord's
/// output) and whether it is muted.
#[derive(Debug, Clone, Copy)]
struct Level {
    percent: u16,
    max: u16,
    muted: bool,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            percent: 0,
            max: 100,
            muted: false,
        }
    }
}

fn level(source: VolumeSource, ctx: &Ctx) -> Option<Level> {
    match source {
        VolumeSource::System => ctx.live.volume.map(|v| Level {
            percent: u16::from(v.percent),
            max: 100,
            muted: v.muted,
        }),
        VolumeSource::DiscordInput => ctx.live.discord.as_ref().map(|d| Level {
            percent: u16::from(d.input_volume),
            max: 100,
            muted: d.self_muted,
        }),
        VolumeSource::DiscordOutput => ctx.live.discord.as_ref().map(|d| Level {
            percent: d.output_volume,
            max: 200,
            muted: d.self_deafened,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_finger_position_to_volume() {
        assert_eq!(volume_fraction_at(480, 160, 485), None);
        let (left, right) = track_range(480.0, 160.0);
        assert_eq!(volume_fraction_at(480, 160, left as u16), Some(0.0));
        assert_eq!(volume_fraction_at(480, 160, right as u16), Some(1.0));
        assert_eq!(volume_fraction_at(480, 160, 639), Some(1.0));
    }
}
