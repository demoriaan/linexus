//! The microphone: a tile naming the input in use (a tap takes the next),
//! its live level as 25 segments, mute for all apps, and the input volume.

use super::sound::{self, Slider, SoundHit, TILE_H, TILE_TOP};
use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::AudioInfo;
use crate::text::{self, FontFace, TextStyle};
use crate::theme::Palette;
use crate::Rgb;
use nexus_config::{MicrophoneProps, Widget};

const PAD: f32 = 12.0;
const GAP: f32 = 11.0;
const TILE_W: f32 = 125.0;
const MUTE_W: f32 = 75.0;
const METER_MIN: f32 = 70.0;
const SLIDER_W: f32 = 133.0;
const SLIDER_MIN: f32 = 70.0;
pub const SEGMENTS: usize = 25;
/// First segment drawn in the warning colour, and in the critical one.
const WARN_FROM: usize = 17;
const CRITICAL_FROM: usize = 22;
/// Above this level the state reads "voice coming through".
const VOICE_LEVEL: u8 = 12;

struct Layout {
    tile: Bounds,
    meter: Option<Bounds>,
    mute: Bounds,
    slider: Option<Slider>,
}

fn layout(b: Bounds, props: &MicrophoneProps) -> Layout {
    let inner = b.w - 2.0 * PAD;
    let base = TILE_W + GAP + MUTE_W;
    let mut rest = inner - base;
    let mut meter_w = None;
    let mut slider_w = None;
    if props.show_volume && rest >= GAP + SLIDER_MIN {
        let w = SLIDER_W.min(rest - GAP);
        slider_w = Some(w);
        rest -= GAP + w;
    }
    if props.show_level && rest >= GAP + METER_MIN {
        meter_w = Some(rest - GAP);
    } else if let (true, Some(w)) = (props.show_level, slider_w) {
        // Room for the meter by taking from the slider.
        let both = rest + GAP + w;
        if both >= 2.0 * GAP + METER_MIN + SLIDER_MIN {
            let slider = SLIDER_MIN.max(both - 2.0 * GAP - METER_MIN).min(SLIDER_W);
            slider_w = Some(slider);
            meter_w = Some(both - 2.0 * GAP - slider);
        }
    }
    let tile_w = TILE_W.min((inner - GAP - MUTE_W).max(40.0));
    let tile = Bounds::new(b.x + PAD, TILE_TOP, tile_w, TILE_H);
    let mut x = tile.right() + GAP;
    let meter = meter_w.map(|w| {
        let meter = Bounds::new(x, 9.9, w, 13.0);
        x += w + GAP;
        meter
    });
    let mute = Bounds::new(x, TILE_TOP, MUTE_W, TILE_H);
    x += MUTE_W + GAP;
    let slider = slider_w.map(|_| Slider {
        left: x,
        right: b.right() - PAD,
        y: 16.5,
    });
    Layout {
        tile,
        meter,
        mute,
        slider,
    }
}

pub(crate) fn hit(
    b: Bounds,
    props: &MicrophoneProps,
    info: &AudioInfo,
    x: f32,
) -> Option<(SoundHit, Option<Slider>)> {
    // "No microphone" takes no touches.
    info.input()?;
    let layout = layout(b, props);
    if x < layout.tile.right() + GAP / 2.0 {
        return Some((SoundHit::NextInput, None));
    }
    if x >= layout.mute.x - GAP / 2.0 && x < layout.mute.right() + GAP / 2.0 {
        return Some((SoundHit::InputMute, None));
    }
    match layout.slider {
        Some(slider) if x >= slider.left - GAP / 2.0 => Some((SoundHit::InputVolume, Some(slider))),
        _ => None,
    }
}

/// Colour of segment `index` with `lit` segments lit.
pub fn segment_color(palette: &Palette, index: usize, lit: usize) -> Rgb {
    if index >= lit {
        palette.track
    } else if index >= CRITICAL_FROM {
        palette.critical
    } else if index >= WARN_FROM {
        palette.warn
    } else {
        palette.good
    }
}

/// Segments lit at `level` (0..=100).
pub fn lit_segments(level: u8) -> usize {
    ((f32::from(level.min(100)) / 4.0).round() as usize).min(SEGMENTS)
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &MicrophoneProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let locale = ctx.live.locale;
    let Some(info) = ctx.live.audio.as_ref() else {
        let text = format::word(Word::SoundUnavailable, locale);
        sound::draw_message(canvas, b, "mic-off", text, palette);
        return;
    };
    let Some(input) = info.input() else {
        let text = format::word(Word::NoInputs, locale);
        sound::draw_message(canvas, b, "mic-off", text, palette);
        return;
    };
    let accent = ctx.accent(widget, palette.series[0]);
    press_glow(canvas, b, accent, ctx.pressed(widget) * 0.4);
    let layout = layout(b, props);
    sound::draw_tile(canvas, layout.tile, input, false, palette, accent);

    let muted = input.muted;
    if let Some(meter) = layout.meter {
        let level = if muted { 0 } else { info.level.unwrap_or(0) };
        let lit = lit_segments(level);
        let gap = 2.0;
        let w = (meter.w - gap * (SEGMENTS - 1) as f32) / SEGMENTS as f32;
        for index in 0..SEGMENTS {
            let x = meter.x + index as f32 * (w + gap);
            let color = segment_color(palette, index, lit);
            canvas.round_rect(Bounds::new(x, meter.y, w, meter.h), 1.5, color, 1.0);
        }
        let label = sound::label_style(palette);
        canvas.text(
            meter.x,
            36.4,
            &format::word(Word::Level, locale).to_uppercase(),
            &label,
        );
        let (state, color) = if muted {
            (Word::MutedForAllApps, sound::muted_text(palette))
        } else if level > VOICE_LEVEL {
            (Word::VoiceArriving, palette.good)
        } else {
            (Word::Silent, sound::soft(palette))
        };
        let style = TextStyle::new(FontFace::SemiBold, 8.5, color);
        let label_w = text::measure(&format::word(Word::Level, locale).to_uppercase(), &label);
        let state = text::ellipsize(format::word(state, locale), &style, meter.w - label_w - 6.0);
        canvas.text_right(meter.right(), 36.4, &state, &style);
    }

    let mute = layout.mute;
    let fill = if muted {
        sound::alarm_fill(palette)
    } else {
        palette.surface
    };
    canvas.round_rect(mute, 6.0, fill, 1.0);
    let (icon, icon_color, word) = if muted {
        ("mic-off", palette.critical, Word::Muted)
    } else {
        ("mic", palette.good, Word::MicrophoneOn)
    };
    canvas.icon(icon, mute.center_x() - 7.0, 10.0, 14.0, icon_color, 1.0);
    let style = TextStyle::new(FontFace::SemiBold, 10.0, palette.text);
    let word = text::ellipsize(format::word(word, locale), &style, mute.w - 6.0);
    canvas.text_centered(mute.center_x(), 36.0, &word, &style);

    if let Some(slider) = layout.slider {
        let dragging = sound::dragged(ctx, widget, info, &input.node);
        let held = info.held(&input.node);
        let fraction = dragging
            .or(held)
            .unwrap_or(f32::from(input.volume.min(100)) / 100.0);
        let fill = if muted {
            sound::muted_fill(palette)
        } else {
            accent
        };
        slider.draw(canvas, palette, fraction, fill, dragging.is_some());
        let value = format!("{}%", (fraction * 100.0).round());
        canvas.text_right(slider.right, 35.5, &value, &sound::value_style(palette));
        let label = format::word(Word::Input, locale).to_uppercase();
        canvas.text(slider.left, 35.5, &label, &sound::label_style(palette));
    }
}
