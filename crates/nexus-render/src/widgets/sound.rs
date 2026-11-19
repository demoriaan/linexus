//! What the sound widgets share: device tiles, thin sliders, page dots,
//! and one answer to "what is under the finger" for all three.

use super::{audio_output, microphone, mixer, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::live::{AudioDevice, AudioInfo, AudioKind};
use crate::text::{FontFace, TextStyle};
use crate::theme::Palette;
use crate::Rgb;
use nexus_config::{Widget, WidgetKind};

pub(crate) const TILE_TOP: f32 = 6.0;
pub(crate) const TILE_H: f32 = 36.0;
/// The last pixels at each end of a track already count as the end.
const END_REACH: f32 = 6.0;

/// What a finger on a sound widget touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundHit {
    /// An output's tile: send the sound there.
    Output(String),
    /// The volume slider of the output in use.
    OutputVolume,
    /// An app's letter: mute or unmute it.
    App(String),
    /// An app's slider.
    AppVolume(String),
    /// The microphone's tile: the next input.
    NextInput,
    /// Mute the microphone for all apps.
    InputMute,
    InputVolume,
}

impl SoundHit {
    pub fn is_slider(&self) -> bool {
        matches!(
            self,
            Self::OutputVolume | Self::AppVolume(_) | Self::InputVolume
        )
    }
}

/// What a finger at `x` (absolute strip X) touches, and for sliders the
/// slider's X range.
pub fn sound_hit(widget: &Widget, info: &AudioInfo, x: u16) -> Option<(SoundHit, Option<Slider>)> {
    let b = widget_bounds(widget);
    let x = f32::from(x);
    match &widget.kind {
        WidgetKind::AudioOutput(props) => audio_output::hit(b, &widget.id, props, info, x),
        WidgetKind::Mixer(props) => mixer::hit(b, &widget.id, props, info, x),
        WidgetKind::Microphone(props) => microphone::hit(b, props, info, x),
        _ => None,
    }
}

/// How many pages of tiles the widget has, and which shows.
pub fn sound_pages(widget: &Widget, info: &AudioInfo) -> (usize, usize) {
    let b = widget_bounds(widget);
    let pages = match &widget.kind {
        WidgetKind::AudioOutput(props) => audio_output::pages(b, props, info.outputs.len()),
        WidgetKind::Mixer(_) => mixer::pages(b, info.apps.len()),
        _ => 1,
    };
    (pages, info.page(&widget.id).min(pages.saturating_sub(1)))
}

fn widget_bounds(widget: &Widget) -> Bounds {
    Bounds::new(
        f32::from(widget.x),
        0.0,
        f32::from(widget.w),
        f32::from(nexus_config::STRIP_HEIGHT),
    )
}

/// A thin track with a round knob.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub left: f32,
    pub right: f32,
    /// Centre line of the track.
    pub y: f32,
}

impl Slider {
    /// Position 0..=1 for a finger at `x`; past the ends is the end.
    pub fn fraction_at(&self, x: f32) -> f32 {
        let reach = END_REACH.min((self.right - self.left) / 4.0);
        let travel = (self.right - self.left - 2.0 * reach).max(1.0);
        ((x - self.left - reach) / travel).clamp(0.0, 1.0)
    }

    /// The X range a finger may move in and still mean the slider.
    pub fn span(&self) -> (u16, u16) {
        (
            self.left.floor().max(0.0) as u16,
            self.right.ceil().max(0.0) as u16,
        )
    }

    pub(crate) fn draw(
        &self,
        canvas: &mut Canvas,
        palette: &Palette,
        fraction: f32,
        fill: Rgb,
        dragging: bool,
    ) {
        let track = Bounds::new(self.left, self.y - 1.5, self.right - self.left, 3.0);
        canvas.round_rect(track, 1.5, palette.track, 1.0);
        let knob_x = self.left + track.w * fraction.clamp(0.0, 1.0);
        if knob_x > self.left {
            canvas.round_rect(
                Bounds::new(self.left, track.y, knob_x - self.left, track.h),
                1.5,
                fill,
                1.0,
            );
        }
        let r = if dragging { 7.0 } else { 5.5 };
        canvas.circle(knob_x, self.y, r, palette.text, 1.0);
    }
}

/// The slider value under the finger, while `key` of `widget` is dragged.
pub(crate) fn dragged(ctx: &Ctx, widget: &Widget, info: &AudioInfo, key: &str) -> Option<f32> {
    let (id, fraction) = ctx.ui.dragging.as_ref()?;
    (*id == widget.id && info.grabbed.as_deref() == Some(key)).then_some(*fraction)
}

pub(crate) fn device_icon(kind: AudioKind) -> &'static str {
    match kind {
        AudioKind::Headset => "headphones",
        AudioKind::Monitor => "monitor",
        AudioKind::Speaker => "speaker",
        AudioKind::Microphone => "mic",
    }
}

/// Small grey text under names and in labels (#8f9ab0 in Midnight).
pub(crate) fn soft(palette: &Palette) -> Rgb {
    palette.text_dim.mix(palette.text_soft, 0.25)
}

/// The word "muted" next to a muted app (#ff8a96 in Midnight, #aa2233
/// in Daylight): at least 4.5:1 against the strip at 9 px.
pub(crate) fn muted_text(palette: &Palette) -> Rgb {
    if palette.light {
        palette.critical.mix(Rgb::hex(0x000000), 0.15)
    } else {
        palette.critical.mix(Rgb::hex(0xffffff), 0.3)
    }
}

/// Fill of a muted slider (#4b5568 in Midnight).
pub(crate) fn muted_fill(palette: &Palette) -> Rgb {
    palette.text_dim.mix(palette.track, 0.55)
}

/// Dark red behind the muted microphone and the privacy pill.
pub(crate) fn alarm_fill(palette: &Palette) -> Rgb {
    palette.bg_top.mix(palette.critical, 0.11)
}

pub(crate) fn label_style(palette: &Palette) -> TextStyle {
    TextStyle::new(FontFace::SemiBold, 8.5, soft(palette)).tracking(0.85)
}

pub(crate) fn value_style(palette: &Palette) -> TextStyle {
    TextStyle::new(FontFace::SemiBold, 8.5, palette.text).tabular()
}

/// A device as a tile: icon, name and the line below. The active one has
/// an accent border.
pub(crate) fn draw_tile(
    canvas: &mut Canvas,
    tile: Bounds,
    device: &AudioDevice,
    active: bool,
    palette: &Palette,
    accent: Rgb,
) {
    let (name, detail) = (device.name.as_str(), device.detail.as_str());
    let fill = if active {
        palette.surface.mix(accent, 0.08)
    } else {
        palette.surface
    };
    canvas.round_rect(tile, 6.0, fill, 1.0);
    if active {
        canvas.round_rect_stroke(tile, 6.0, accent, 1.0, 1.0);
    }
    let icon_color = if active { accent } else { palette.text_soft };
    let icon = device_icon(device.kind);
    canvas.icon(icon, tile.x + 7.0, tile.y + 10.5, 15.0, icon_color, 1.0);
    let text_x = tile.x + 28.0;
    let room = (tile.right() - 6.0 - text_x).max(0.0);
    let name_style = TextStyle::new(FontFace::SemiBold, 10.5, palette.text);
    if detail.is_empty() {
        canvas.text_fit(text_x, tile.y + 21.5, name, &name_style, room);
        return;
    }
    canvas.text_fit(text_x, tile.y + 17.3, name, &name_style, room);
    let detail_style = TextStyle::new(FontFace::Medium, 8.0, soft(palette));
    canvas.text_fit(text_x, tile.y + 27.5, detail, &detail_style, room);
}

/// Which page shows, as dots below the tiles.
pub(crate) fn draw_dots(
    canvas: &mut Canvas,
    center_x: f32,
    page: usize,
    pages: usize,
    palette: &Palette,
    accent: Rgb,
) {
    if pages < 2 {
        return;
    }
    let step = 7.0;
    let start = center_x - step * (pages - 1) as f32 / 2.0;
    for n in 0..pages {
        let (color, alpha) = if n == page {
            (accent, 1.0)
        } else {
            (palette.text_dim, 0.6)
        };
        canvas.circle(start + n as f32 * step, 45.5, 1.6, color, alpha);
    }
}

/// A grey line in place of the widget when there is nothing to show. A
/// widget showing it takes no touches.
pub(crate) fn draw_message(
    canvas: &mut Canvas,
    b: Bounds,
    icon: &str,
    text: &str,
    palette: &Palette,
) {
    canvas.icon(icon, b.x + 12.0, 15.0, 18.0, palette.text_dim, 1.0);
    let style = TextStyle::new(FontFace::SemiBold, 13.0, palette.text_dim);
    canvas.text_fit(b.x + 38.0, 28.5, text, &style, (b.w - 44.0).max(0.0));
}

/// Index range of the items on `page`, with `per` items per page.
pub(crate) fn page_range(count: usize, per: usize, page: usize) -> std::ops::Range<usize> {
    let per = per.max(1);
    let pages = count.div_ceil(per).max(1);
    let start = page.min(pages - 1) * per;
    start..(start + per).min(count)
}
