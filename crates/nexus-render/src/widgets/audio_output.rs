//! Output devices as tiles. The one in use has an accent border; a tap
//! sends the sound to another. Right of the tiles, a slider sets the
//! volume of the device in use. More devices than fit take pages, with
//! dots below.

use super::sound::{self, Slider, SoundHit, TILE_H, TILE_TOP};
use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::AudioInfo;
use crate::text;
use nexus_config::{AudioOutputProps, Widget};

const PAD: f32 = 11.0;
const GAP: f32 = 7.0;
const TILE_W: f32 = 114.0;
/// Room the volume needs: a little space, the speaker icon, the track.
const SLIDER_MIN: f32 = 118.0;
const ICON: f32 = 15.0;

struct Layout {
    /// Tiles per page.
    per: usize,
    tile_w: f32,
    /// Left edge of the volume part, when it shows.
    volume_x: Option<f32>,
}

fn layout(b: Bounds, props: &AudioOutputProps, count: usize) -> Layout {
    let inner = b.w - 2.0 * PAD;
    let fit = |room: f32| (((room + GAP) / (TILE_W + GAP)).floor().max(0.0)) as usize;
    let mut volume = props.show_volume;
    let mut room = if volume {
        inner - SLIDER_MIN - GAP
    } else {
        inner
    };
    if volume && room < 70.0 {
        // Too narrow for both: the tiles win.
        volume = false;
        room = inner;
    }
    let per = fit(room).max(1);
    let shown = count.clamp(1, per);
    let tile_w = TILE_W.min(room.max(0.0));
    let tiles_w = shown as f32 * tile_w + (shown - 1) as f32 * GAP;
    Layout {
        per,
        tile_w,
        volume_x: volume.then_some(b.x + PAD + tiles_w + GAP),
    }
}

pub(crate) fn pages(b: Bounds, props: &AudioOutputProps, count: usize) -> usize {
    count.div_ceil(layout(b, props, count).per).max(1)
}

fn slider(b: Bounds, volume_x: f32) -> Slider {
    Slider {
        left: volume_x + 5.0 + ICON + 7.0,
        right: b.right() - PAD,
        y: 16.5,
    }
}

fn tile_bounds(b: Bounds, layout: &Layout, slot: usize) -> Bounds {
    Bounds::new(
        b.x + PAD + slot as f32 * (layout.tile_w + GAP),
        TILE_TOP,
        layout.tile_w,
        TILE_H,
    )
}

pub(crate) fn hit(
    b: Bounds,
    widget_id: &str,
    props: &AudioOutputProps,
    info: &AudioInfo,
    x: f32,
) -> Option<(SoundHit, Option<Slider>)> {
    // "No outputs" takes no touches.
    if info.outputs.is_empty() {
        return None;
    }
    let layout = layout(b, props, info.outputs.len());
    if let Some(volume_x) = layout.volume_x {
        if x >= volume_x - GAP / 2.0 {
            let slider = slider(b, volume_x);
            return info
                .output()
                .map(|_| (SoundHit::OutputVolume, Some(slider)));
        }
    }
    let page = info.page(widget_id);
    let range = sound::page_range(info.outputs.len(), layout.per, page);
    range.enumerate().find_map(|(slot, index)| {
        let tile = tile_bounds(b, &layout, slot);
        (x >= tile.x - GAP / 2.0 && x < tile.right() + GAP / 2.0)
            .then(|| (SoundHit::Output(info.outputs[index].node.clone()), None))
    })
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &AudioOutputProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let locale = ctx.live.locale;
    let Some(info) = ctx.live.audio.as_ref() else {
        let text = format::word(Word::SoundUnavailable, locale);
        sound::draw_message(canvas, b, "volume-x", text, palette);
        return;
    };
    if info.outputs.is_empty() {
        let text = format::word(Word::NoOutputs, locale);
        sound::draw_message(canvas, b, "volume-x", text, palette);
        return;
    }
    let accent = ctx.accent(widget, palette.accent);
    press_glow(canvas, b, accent, ctx.pressed(widget) * 0.4);
    let layout = layout(b, props, info.outputs.len());
    let pages = info.outputs.len().div_ceil(layout.per).max(1);
    let page = info.page(&widget.id).min(pages - 1);
    let range = sound::page_range(info.outputs.len(), layout.per, page);
    let in_use = info.default_output.as_deref();
    for (slot, index) in range.enumerate() {
        let device = &info.outputs[index];
        sound::draw_tile(
            canvas,
            tile_bounds(b, &layout, slot),
            device,
            in_use == Some(device.node.as_str()),
            palette,
            accent,
        );
    }
    let shown = layout.per.min(info.outputs.len());
    let tiles_w = shown as f32 * layout.tile_w + (shown - 1) as f32 * GAP;
    sound::draw_dots(
        canvas,
        b.x + PAD + tiles_w / 2.0,
        page,
        pages,
        palette,
        accent,
    );

    let Some(volume_x) = layout.volume_x else {
        return;
    };
    let slider = slider(b, volume_x);
    let device = info.output();
    let dragging = device.and_then(|d| sound::dragged(ctx, widget, info, &d.node));
    let held = device.and_then(|d| info.held(&d.node));
    let (percent, muted) = device.map_or((0, false), |d| (d.volume.min(100), d.muted));
    let fraction = dragging.or(held).unwrap_or(f32::from(percent) / 100.0);
    let icon = match device {
        None => "volume-x",
        Some(_) if muted || fraction <= 0.0 => "volume-x",
        Some(_) if fraction < 0.5 => "volume-1",
        Some(_) => "volume-2",
    };
    let icon_color = if muted {
        palette.text_dim
    } else {
        palette.text
    };
    canvas.icon(icon, volume_x + 5.0, 16.5, ICON, icon_color, 1.0);
    let fill = if muted {
        sound::muted_fill(palette)
    } else {
        ctx.accent(widget, palette.series[0])
    };
    if device.is_some() {
        slider.draw(canvas, palette, fraction, fill, dragging.is_some());
    } else {
        canvas.round_rect(
            Bounds::new(slider.left, slider.y - 1.5, slider.right - slider.left, 3.0),
            1.5,
            palette.track,
            1.0,
        );
    }
    let value_style = sound::value_style(palette);
    let value = match device {
        Some(_) if muted && dragging.or(held).is_none() => {
            format::word(Word::Muted, locale).to_lowercase()
        }
        Some(_) => format!("{}%", (fraction * 100.0).round()),
        None => "–".to_string(),
    };
    let value_w = canvas.text_right(slider.right, 35.5, &value, &value_style);
    let label = device.map(|d| d.name.to_uppercase()).unwrap_or_default();
    let label_style = sound::label_style(palette);
    let room = (slider.right - slider.left - value_w - 6.0).max(0.0);
    let label = text::ellipsize(&label, &label_style, room);
    canvas.text(slider.left, 35.5, &label, &label_style);
}
