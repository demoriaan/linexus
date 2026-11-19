//! One column per app that plays sound: a letter in the app's colour, its
//! name and volume, and a slider. A tap on the letter mutes the app; a
//! muted app's letter fades and gets a red badge. Pinned apps come first;
//! more apps than fit take pages.

use super::sound::{self, Slider, SoundHit};
use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::{AudioApp, AudioInfo};
use crate::text::{self, FontFace, TextStyle};
use crate::Rgb;
use nexus_config::{MixerProps, Widget};

const PAD: f32 = 12.0;
const GAP: f32 = 13.0;
/// Narrowest column; at most four columns show.
const COLUMN_MIN: f32 = 110.0;
const MAX_COLUMNS: usize = 4;
const AVATAR: f32 = 27.0;
/// The letter and the room right of it.
const AVATAR_ZONE: f32 = AVATAR + 7.0;
const AVATAR_TOP: f32 = 10.5;

fn columns(b: Bounds) -> (usize, f32) {
    let inner = (b.w - 2.0 * PAD).max(1.0);
    let per = (((inner + GAP) / (COLUMN_MIN + GAP)).floor() as usize).clamp(1, MAX_COLUMNS);
    let width = (inner - (per - 1) as f32 * GAP) / per as f32;
    (per, width)
}

pub(crate) fn pages(b: Bounds, count: usize) -> usize {
    count.div_ceil(columns(b).0).max(1)
}

/// The apps in the order the widget shows them: pinned ones first, in the
/// order they are pinned, then the rest as they started.
pub fn mixer_apps<'a>(props: &MixerProps, info: &'a AudioInfo) -> Vec<&'a AudioApp> {
    let pin = |app: &AudioApp| {
        props
            .pinned
            .iter()
            .position(|name| name.trim().eq_ignore_ascii_case(app.name.trim()))
    };
    let mut apps: Vec<(usize, &AudioApp)> = info.apps.iter().enumerate().collect();
    apps.sort_by_key(|(order, app)| (pin(app).unwrap_or(usize::MAX), *order));
    apps.into_iter().map(|(_, app)| app).collect()
}

fn column(b: Bounds, width: f32, slot: usize) -> Bounds {
    Bounds::new(b.x + PAD + slot as f32 * (width + GAP), 0.0, width, b.h)
}

fn slider(column: Bounds) -> Slider {
    Slider {
        left: column.x + AVATAR_ZONE,
        right: column.right(),
        y: 32.3,
    }
}

pub(crate) fn hit(
    b: Bounds,
    widget_id: &str,
    props: &MixerProps,
    info: &AudioInfo,
    x: f32,
) -> Option<(SoundHit, Option<Slider>)> {
    let apps = mixer_apps(props, info);
    let (per, width) = columns(b);
    let range = sound::page_range(apps.len(), per, info.page(widget_id));
    range.enumerate().find_map(|(slot, index)| {
        let column = column(b, width, slot);
        if x < column.x - GAP / 2.0 || x >= column.right() + GAP / 2.0 {
            return None;
        }
        let name = apps[index].name.clone();
        Some(if x < column.x + AVATAR_ZONE {
            (SoundHit::App(name), None)
        } else {
            (SoundHit::AppVolume(name), Some(slider(column)))
        })
    })
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &MixerProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let locale = ctx.live.locale;
    let Some(info) = ctx.live.audio.as_ref() else {
        let text = format::word(Word::SoundUnavailable, locale);
        sound::draw_message(canvas, b, "volume-x", text, palette);
        return;
    };
    let apps = mixer_apps(props, info);
    if apps.is_empty() {
        let text = format::word(Word::NoAppSound, locale);
        sound::draw_message(canvas, b, "audio-lines", text, palette);
        return;
    }
    let accent = ctx.accent(widget, palette.series[0]);
    press_glow(canvas, b, accent, ctx.pressed(widget) * 0.4);
    let (per, width) = columns(b);
    let pages = apps.len().div_ceil(per).max(1);
    let page = info.page(&widget.id).min(pages - 1);
    let range = sound::page_range(apps.len(), per, page);
    for (slot, index) in range.enumerate() {
        draw_app(
            canvas,
            column(b, width, slot),
            apps[index],
            widget,
            info,
            ctx,
            accent,
        );
    }
    sound::draw_dots(canvas, b.center_x(), page, pages, palette, accent);
}

fn draw_app(
    canvas: &mut Canvas,
    column: Bounds,
    app: &AudioApp,
    widget: &Widget,
    info: &AudioInfo,
    ctx: &Ctx,
    accent: Rgb,
) {
    let palette = ctx.palette;
    let color = ctx.series_for(&app.name);
    let cx = column.x + AVATAR / 2.0;
    let cy = AVATAR_TOP + AVATAR / 2.0;
    let alpha = if app.muted { 0.45 } else { 1.0 };
    canvas.circle(cx, cy, AVATAR / 2.0, color, alpha);
    let letter: String = app
        .name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let ink = TextStyle::new(FontFace::Bold, 13.0, palette.bg_top).alpha(alpha);
    canvas.text_centered(cx, cy + 4.6, &letter, &ink);
    if app.muted {
        let (bx, by) = (column.x + AVATAR - 4.0, AVATAR_TOP + AVATAR - 4.0);
        canvas.circle(bx, by, 6.0, palette.critical, 1.0);
        canvas.icon("volume-x", bx - 3.5, by - 3.5, 7.0, Rgb::hex(0xffffff), 1.0);
    }

    let slider = slider(column);
    let dragging = sound::dragged(ctx, widget, info, &app.name);
    let moved = dragging.or_else(|| info.held(&app.name));
    let fraction = moved.unwrap_or(f32::from(app.volume.min(100)) / 100.0);
    let (value, value_color) = if app.muted && moved.is_none() {
        (
            format::word(Word::Muted, ctx.live.locale).to_lowercase(),
            sound::muted_text(palette),
        )
    } else {
        (format!("{}%", (fraction * 100.0).round()), palette.text)
    };
    let value_style = TextStyle::new(FontFace::SemiBold, 9.5, value_color).tabular();
    let value_w = canvas.text_right(slider.right, 20.7, &value, &value_style);
    let name_style = TextStyle::new(FontFace::SemiBold, 10.5, palette.text);
    let room = (slider.right - slider.left - value_w - 5.0).max(0.0);
    let name = text::ellipsize(&app.name, &name_style, room);
    canvas.text(slider.left, 20.7, &name, &name_style);
    let fill = if app.muted {
        sound::muted_fill(palette)
    } else {
        accent
    };
    slider.draw(canvas, palette, fraction, fill, dragging.is_some());
}
