//! One renderer per widget type. Each draws into its own X range of the
//! strip and reads only configuration, live data and interaction state.

mod button;
mod clock;
mod entity;
mod media;
mod metric;
mod network;
pub mod news;
mod text;
mod timer;
mod volume;
mod weather;

use crate::canvas::{Bounds, Canvas};
use crate::live::{LiveData, UiState};
use crate::text::{FontFace, TextStyle};
use crate::theme::Palette;
use crate::Rgb;
use nexus_config::{Widget, WidgetKind};

pub use media::{media_zone, MediaZone};
pub use volume::volume_fraction_at;
pub use weather::weather_key;

/// Shared inputs for every widget renderer.
pub struct Ctx<'a> {
    pub palette: &'a Palette,
    pub live: &'a LiveData,
    pub ui: &'a UiState,
}

impl Ctx<'_> {
    /// Press animation strength (0..=1) for `widget`.
    pub fn pressed(&self, widget: &Widget) -> f32 {
        match &self.ui.pressed {
            Some((id, strength)) if *id == widget.id => *strength,
            _ => 0.0,
        }
    }

    /// The widget's own accent, or `fallback`.
    pub fn accent(&self, widget: &Widget, fallback: Rgb) -> Rgb {
        widget.accent.map(Rgb::from).unwrap_or(fallback)
    }

    /// Small uppercase caption style.
    pub fn caption(&self) -> TextStyle {
        TextStyle::new(FontFace::SemiBold, 11.0, self.palette.text_dim).tracking(0.6)
    }

    /// Colourful default for tiles without an explicit accent: a stable
    /// pick from the palette's series colours.
    pub fn series_for(&self, key: &str) -> Rgb {
        let hash = key
            .bytes()
            .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
        self.palette.series[(hash as usize) % self.palette.series.len()]
    }
}

pub fn draw(canvas: &mut Canvas, widget: &Widget, ctx: &Ctx) {
    let bounds = Bounds::new(
        f32::from(widget.x),
        0.0,
        f32::from(widget.w),
        f32::from(nexus_config::STRIP_HEIGHT),
    );
    canvas.clip_x(bounds.x, bounds.right());
    match &widget.kind {
        WidgetKind::Clock(props) => clock::draw(canvas, bounds, props, ctx),
        WidgetKind::Metric(props) => metric::draw(canvas, bounds, widget, props, ctx),
        WidgetKind::Network(_) => network::draw(canvas, bounds, ctx),
        WidgetKind::Media(props) => media::draw(canvas, bounds, widget, props, ctx),
        WidgetKind::Volume(props) => volume::draw(canvas, bounds, widget, props, ctx),
        WidgetKind::Button(props) => button::draw(canvas, bounds, widget, props, ctx),
        WidgetKind::News(props) => news::draw(canvas, bounds, props, ctx),
        WidgetKind::Weather(props) => weather::draw(canvas, bounds, props, ctx),
        WidgetKind::Entity(props) => entity::draw(canvas, bounds, widget, props, ctx),
        WidgetKind::Timer(props) => timer::draw(canvas, bounds, widget, props, ctx),
        WidgetKind::Text(props) => text::draw_text(canvas, bounds, widget, props, ctx),
        WidgetKind::Image(props) => text::draw_image(canvas, bounds, props),
        WidgetKind::Command(props) => text::draw_command(canvas, bounds, widget, props, ctx),
        WidgetKind::Spacer => {}
    }
    canvas.unclip();
}

/// Tinted rounded highlight behind a pressed widget.
pub(crate) fn press_glow(canvas: &mut Canvas, bounds: Bounds, color: Rgb, strength: f32) {
    if strength > 0.0 {
        canvas.round_rect(bounds.inset(3.0, 3.0), 7.0, color, 0.22 * strength);
    }
}

/// Icon for a metric id.
pub(crate) fn metric_icon(source: &str) -> &'static str {
    if let Some(rest) = source.strip_prefix("battery:") {
        return match rest.split('/').next().unwrap_or_default() {
            "mouse" => "mouse",
            "keyboard" => "keyboard",
            "headset" | "headphones" => "headphones",
            "gaming-input" => "gamepad-2",
            "phone" => "smartphone",
            "tablet" => "tablet",
            _ => "battery-medium",
        };
    }
    let (family, rest) = source.split_once('.').unwrap_or((source, ""));
    match (family, rest) {
        (_, "temp") => "thermometer",
        (_, "power") => "zap",
        (_, "fan") => "fan",
        (_, "clock") => "gauge",
        ("cpu", _) => "cpu",
        ("gpu", _) => "gpu",
        ("ram" | "swap", _) => "memory-stick",
        ("net", "down") => "arrow-down",
        ("net", "up") => "arrow-up",
        ("disk", _) => "hard-drive",
        ("volume", _) => "volume-2",
        _ if family.starts_with("hwmon") => "thermometer",
        _ => "activity",
    }
}
