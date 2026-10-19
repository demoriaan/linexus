//! Pure renderer for the 640x48 Nexus strip: configuration plus live data in,
//! RGBA frame out. No file, device or network access happens here.

pub mod canvas;
mod font_headline;
pub mod format;
pub mod icons;
pub mod live;
pub mod preview;
pub mod text;
pub mod theme;
pub mod widgets;

pub use canvas::{Bounds, Canvas};
pub use live::*;
pub use text::{FontFace, TextStyle};
pub use theme::{Palette, PRESETS};
pub use widgets::{media_zone, news, volume_fraction_at, weather_key, MediaZone};

use nexus_config::{HexColor, ImageFit, Page, Theme};

pub use tiny_skia;

/// Raw Lucide catalog (`name<TAB>tags<TAB>svg body` per line) for Studio's
/// icon picker.
pub fn icon_catalog() -> &'static str {
    icons::catalog_text()
}

pub const WIDTH: usize = nexus_core::WIDTH;
pub const HEIGHT: usize = nexus_core::HEIGHT;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn hex(v: u32) -> Self {
        Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn skia(self, alpha: f32) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(
            self.r,
            self.g,
            self.b,
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }

    /// `#rrggbb`
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Linear blend towards `other` by `t` (0..=1).
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb::new(
            lerp(self.r, other.r),
            lerp(self.g, other.g),
            lerp(self.b, other.b),
        )
    }
}

impl From<HexColor> for Rgb {
    fn from(color: HexColor) -> Self {
        let [r, g, b] = color.0;
        Self::new(r, g, b)
    }
}

/// Renders one page with its widgets, the page indicator and any overlay.
pub fn render_page(page: &Page, theme: &Theme, live: &LiveData, ui: &UiState) -> Canvas {
    let palette = Palette::for_theme(theme);
    let mut canvas = Canvas::new();
    draw_background(&mut canvas, page, &palette);
    let ctx = widgets::Ctx {
        palette: &palette,
        live,
        ui,
    };
    for widget in &page.widgets {
        widgets::draw(&mut canvas, widget, &ctx);
    }
    draw_page_indicator(&mut canvas, ui, &palette);
    canvas
}

fn draw_background(canvas: &mut Canvas, page: &Page, palette: &Palette) {
    canvas.vertical_gradient(Bounds::strip(), palette.bg_top, palette.bg_bottom);
    let Some(background) = &page.background else {
        return;
    };
    if let Some(color) = background.color {
        canvas.fill(Bounds::strip(), Rgb::from(color), 1.0);
    }
    if let Some(image) = background.image.as_deref().and_then(icons::data_url_image) {
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        let (w, h) = (WIDTH as f32, HEIGHT as f32);
        let rect = match background.fit {
            ImageFit::Stretch => Bounds::strip(),
            ImageFit::Fit | ImageFit::Fill => {
                let s = if background.fit == ImageFit::Fit {
                    (w / iw).min(h / ih)
                } else {
                    (w / iw).max(h / ih)
                };
                Bounds::new((w - iw * s) / 2.0, (h - ih * s) / 2.0, iw * s, ih * s)
            }
        };
        canvas.draw_image(rect, image.as_ref().as_ref(), 0.0, 1.0);
    }
    if background.dim_percent > 0 {
        canvas.fill(
            Bounds::strip(),
            Rgb::new(0, 0, 0),
            f32::from(background.dim_percent.min(90)) / 100.0,
        );
    }
}

/// Pill-shaped page dots along the bottom edge, faded by
/// `ui.indicator_alpha`.
pub fn draw_page_indicator(canvas: &mut Canvas, ui: &UiState, palette: &Palette) {
    if ui.indicator_alpha <= 0.0 || ui.page_count < 2 {
        return;
    }
    let alpha = ui.indicator_alpha.clamp(0.0, 1.0);
    let n = ui.page_count.min(12);
    let (dot, active, gap) = (5.0, 14.0, 4.0);
    let total = (n - 1) as f32 * (dot + gap) + active;
    let mut x = (WIDTH as f32 - total) / 2.0;
    let back = Bounds::new(x - 6.0, 41.0, total + 12.0, 7.0);
    canvas.round_rect(back, 3.5, palette.bg_bottom, 0.85 * alpha);
    for i in 0..n {
        let current = i == ui.page_index.min(n - 1);
        let w = if current { active } else { dot };
        let color = if current {
            palette.text
        } else {
            palette.text_dim
        };
        canvas.round_rect(Bounds::new(x, 42.5, w, 4.0), 2.0, color, alpha);
        x += w + gap;
    }
}

/// Draws a transient overlay. `visibility` (0..=1) drives a slide-in from
/// the top edge.
pub fn draw_overlay(
    canvas: &mut Canvas,
    overlay: &Overlay,
    theme: &Theme,
    live: &LiveData,
    visibility: f32,
) {
    let palette = Palette::for_theme(theme);
    let t = ease_out_cubic(visibility.clamp(0.0, 1.0));
    if t <= 0.0 {
        return;
    }
    let offset = (1.0 - t) * -(HEIGHT as f32);
    let panel = Bounds::new(0.0, offset, WIDTH as f32, HEIGHT as f32);
    canvas.vertical_gradient(panel, palette.bg_top, palette.bg_bottom);
    canvas.fill(
        Bounds::new(0.0, offset + HEIGHT as f32 - 2.0, WIDTH as f32, 2.0),
        palette.accent,
        0.9,
    );
    let y = offset;
    match overlay {
        Overlay::Notification {
            app,
            title,
            body,
            icon,
        } => {
            match icon {
                Some(image) => icons::draw_image_icon(canvas, image, 12.0, y + 8.0, 30.0, 1.0),
                None => canvas.icon("bell", 15.0, y + 11.0, 24.0, palette.accent, 1.0),
            }
            let caption = TextStyle::new(FontFace::SemiBold, 11.0, palette.accent).tracking(0.6);
            let app_w = canvas.text(54.0, y + 16.0, &app.to_uppercase(), &caption);
            let title_style = TextStyle::new(FontFace::Bold, 15.0, palette.text);
            canvas.text_fit(
                54.0 + app_w + 10.0,
                y + 17.0,
                title,
                &title_style,
                WIDTH as f32 - 70.0 - app_w,
            );
            let body_style = TextStyle::new(FontFace::Medium, 14.0, palette.text_soft);
            let line = body.split_whitespace().collect::<Vec<_>>().join(" ");
            canvas.text_fit(54.0, y + 37.0, &line, &body_style, WIDTH as f32 - 66.0);
        }
        Overlay::Volume(volume) => {
            let icon = if volume.muted { "volume-x" } else { "volume-2" };
            canvas.icon(icon, 16.0, y + 13.0, 22.0, palette.text, 1.0);
            let track = Bounds::new(56.0, y + 21.0, 500.0, 6.0);
            canvas.round_rect(track, 3.0, palette.track, 1.0);
            let fraction = f32::from(volume.percent.min(100)) / 100.0;
            let color = if volume.muted {
                palette.text_dim
            } else {
                palette.accent
            };
            canvas.round_rect(
                Bounds::new(track.x, track.y, track.w * fraction, track.h),
                3.0,
                color,
                1.0,
            );
            let value = TextStyle::new(FontFace::Bold, 20.0, palette.text).tabular();
            let label = if volume.muted {
                format::word(format::Word::Muted, live.locale).to_string()
            } else {
                format!("{}%", volume.percent)
            };
            canvas.text_right(WIDTH as f32 - 14.0, y + 31.0, &label, &value);
        }
        Overlay::NowPlaying(media) => {
            let art = Bounds::new(6.0, y + 5.0, 38.0, 38.0);
            match &media.art {
                Some(image) => canvas.draw_image(art, image.as_ref().as_ref(), 6.0, 1.0),
                None => {
                    canvas.round_rect(art, 6.0, media.art_color.unwrap_or(palette.accent), 0.6);
                    canvas.icon("music", art.x + 9.0, art.y + 9.0, 20.0, palette.text, 1.0);
                }
            }
            let title = TextStyle::new(FontFace::Bold, 17.0, palette.text);
            canvas.text_fit(54.0, y + 20.0, &media.title, &title, WIDTH as f32 - 66.0);
            let sub = TextStyle::new(FontFace::Medium, 13.0, palette.text_dim);
            canvas.text_fit(54.0, y + 37.0, &media.artist, &sub, WIDTH as f32 - 66.0);
        }
        Overlay::Toast { icon, text } => {
            canvas.icon(icon, 16.0, y + 13.0, 22.0, palette.accent, 1.0);
            let style = TextStyle::new(FontFace::SemiBold, 17.0, palette.text);
            canvas.text_fit(52.0, y + 30.0, text, &style, WIDTH as f32 - 64.0);
        }
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Slides from `from` to `to`. `offset` is how far (in pixels) the new page
/// has moved in; positive values bring it in from the right.
pub fn slide(from: &Canvas, to: &Canvas, offset: f32) -> Canvas {
    let mut out = Canvas::new();
    let shift = offset.round() as i32;
    let w = WIDTH as i32;
    let (from_x, to_x) = if shift >= 0 {
        (-shift, w - shift)
    } else {
        (-shift, -w - shift)
    };
    out.draw_pixmap(from_x, 0, from.pixmap().as_ref(), 1.0);
    out.draw_pixmap(to_x, 0, to.pixmap().as_ref(), 1.0);
    out
}

/// Cross-fades `from` into `to`.
pub fn crossfade(from: &Canvas, to: &Canvas, t: f32) -> Canvas {
    let mut out = Canvas::new();
    out.draw_pixmap(0, 0, from.pixmap().as_ref(), 1.0);
    out.draw_pixmap(0, 0, to.pixmap().as_ref(), t.clamp(0.0, 1.0));
    out
}

/// PNG bytes for an RGBA frame (previews, Studio mirror).
pub fn encode_png(rgba: &[u8]) -> Option<Vec<u8>> {
    let image = image::RgbaImage::from_raw(WIDTH as u32, HEIGHT as u32, rgba.to_vec())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Red, green and blue thirds for checking the panel's colour order.
pub fn rgb_bars_frame() -> Vec<u8> {
    let mut canvas = Canvas::new();
    let third = WIDTH as f32 / 3.0;
    canvas.fill(
        Bounds::new(0.0, 0.0, third, HEIGHT as f32),
        Rgb::new(255, 0, 0),
        1.0,
    );
    canvas.fill(
        Bounds::new(third, 0.0, third, HEIGHT as f32),
        Rgb::new(0, 255, 0),
        1.0,
    );
    canvas.fill(
        Bounds::new(third * 2.0, 0.0, third, HEIGHT as f32),
        Rgb::new(0, 0, 255),
        1.0,
    );
    let label = TextStyle::new(FontFace::Bold, 22.0, Rgb::new(255, 255, 255));
    for (i, name) in ["RED", "GREEN", "BLUE"].iter().enumerate() {
        canvas.text_centered(third * (i as f32 + 0.5), 32.0, name, &label);
    }
    canvas.into_rgba()
}

/// 1 px border and a grid every 100 x 12 px for checking geometry.
pub fn border_grid_frame() -> Vec<u8> {
    let mut canvas = Canvas::new();
    canvas.clear(Rgb::new(5, 5, 8));
    let orange = Rgb::new(255, 180, 0);
    for x in (0..WIDTH).step_by(100) {
        canvas.fill(Bounds::new(x as f32, 0.0, 1.0, HEIGHT as f32), orange, 1.0);
    }
    for y in (0..HEIGHT).step_by(12) {
        canvas.fill(Bounds::new(0.0, y as f32, WIDTH as f32, 1.0), orange, 1.0);
    }
    let white = Rgb::new(255, 255, 255);
    canvas.fill(Bounds::new(0.0, 0.0, WIDTH as f32, 1.0), white, 1.0);
    canvas.fill(
        Bounds::new(0.0, HEIGHT as f32 - 1.0, WIDTH as f32, 1.0),
        white,
        1.0,
    );
    canvas.fill(Bounds::new(0.0, 0.0, 1.0, HEIGHT as f32), white, 1.0);
    canvas.fill(
        Bounds::new(WIDTH as f32 - 1.0, 0.0, 1.0, HEIGHT as f32),
        white,
        1.0,
    );
    canvas.into_rgba()
}

#[cfg(test)]
mod tests;
