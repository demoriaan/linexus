//! Anti-aliased drawing surface for one 640x48 frame.

use crate::text::{self, FontFace, TextStyle};
use crate::{Rgb, HEIGHT, WIDTH};
use tiny_skia::{
    FillRule, GradientStop, LinearGradient, Mask, Paint, PathBuilder, Pixmap, Point, Rect, Shader,
    SpreadMode, Stroke, Transform,
};

pub use tiny_skia::Path;

/// Axis-aligned rectangle in strip pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Bounds {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn strip() -> Self {
        Self::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }

    /// Splits off `w` pixels on the left; returns (left, rest).
    pub fn split_left(&self, w: f32) -> (Self, Self) {
        let w = w.min(self.w);
        (
            Self::new(self.x, self.y, w, self.h),
            Self::new(self.x + w, self.y, self.w - w, self.h),
        )
    }

    /// Splits off `w` pixels on the right; returns (rest, right).
    pub fn split_right(&self, w: f32) -> (Self, Self) {
        let w = w.min(self.w);
        (
            Self::new(self.x, self.y, self.w - w, self.h),
            Self::new(self.right() - w, self.y, w, self.h),
        )
    }
}

pub struct Canvas {
    pixmap: Pixmap,
    /// Optional horizontal clip (left, right) applied to text and fills.
    clip: Option<(f32, f32)>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    pub fn new() -> Self {
        Self {
            pixmap: Pixmap::new(WIDTH as u32, HEIGHT as u32).expect("non-zero strip size"),
            clip: None,
        }
    }

    pub fn from_rgba(rgba: &[u8]) -> Option<Self> {
        let mut canvas = Self::new();
        if rgba.len() != canvas.pixmap.data().len() {
            return None;
        }
        canvas.pixmap.data_mut().copy_from_slice(rgba);
        Some(canvas)
    }

    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    pub fn pixmap_mut(&mut self) -> &mut Pixmap {
        &mut self.pixmap
    }

    /// Straight RGBA bytes. Every pixel of a finished frame is opaque, so
    /// tiny-skia's premultiplied storage is already straight alpha.
    pub fn rgba(&self) -> &[u8] {
        self.pixmap.data()
    }

    pub fn into_rgba(self) -> Vec<u8> {
        self.pixmap.take()
    }

    /// Restricts subsequent drawing to `left..right` (intersected with any
    /// active clip) and returns the previous clip for [`Self::set_clip`].
    pub fn clip_x(&mut self, left: f32, right: f32) -> Option<(f32, f32)> {
        let previous = self.clip;
        self.clip = Some(match previous {
            Some((l, r)) => (left.max(l), right.min(r)),
            None => (left, right),
        });
        previous
    }

    pub fn set_clip(&mut self, clip: Option<(f32, f32)>) {
        self.clip = clip;
    }

    pub fn unclip(&mut self) {
        self.clip = None;
    }

    fn mask(&self) -> Option<Mask> {
        let (left, right) = self.clip?;
        let mut mask = Mask::new(WIDTH as u32, HEIGHT as u32)?;
        let rect = Rect::from_ltrb(left.max(0.0), 0.0, right.min(WIDTH as f32), HEIGHT as f32)?;
        mask.fill_path(
            &PathBuilder::from_rect(rect),
            FillRule::Winding,
            false,
            Transform::identity(),
        );
        Some(mask)
    }

    pub fn clear(&mut self, color: Rgb) {
        self.pixmap.fill(color.skia(1.0));
    }

    pub fn vertical_gradient(&mut self, bounds: Bounds, top: Rgb, bottom: Rgb) {
        let Some(shader) = LinearGradient::new(
            Point::from_xy(0.0, bounds.y),
            Point::from_xy(0.0, bounds.bottom()),
            vec![
                GradientStop::new(0.0, top.skia(1.0)),
                GradientStop::new(1.0, bottom.skia(1.0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return self.fill(bounds, top, 1.0);
        };
        self.fill_shader(&rect_path(bounds), shader);
    }

    /// Fills `bounds` with `color` fading from `from_alpha` to `to_alpha`
    /// left to right.
    pub fn horizontal_fade(&mut self, bounds: Bounds, color: Rgb, from_alpha: f32, to_alpha: f32) {
        let Some(shader) = LinearGradient::new(
            Point::from_xy(bounds.x, 0.0),
            Point::from_xy(bounds.right(), 0.0),
            vec![
                GradientStop::new(0.0, color.skia(from_alpha)),
                GradientStop::new(1.0, color.skia(to_alpha)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        self.fill_shader(&rect_path(bounds), shader);
    }

    pub fn fill(&mut self, bounds: Bounds, color: Rgb, alpha: f32) {
        self.fill_path(&rect_path(bounds), color, alpha);
    }

    pub fn round_rect(&mut self, bounds: Bounds, radius: f32, color: Rgb, alpha: f32) {
        if let Some(path) = round_rect_path(bounds, radius) {
            self.fill_path(&path, color, alpha);
        }
    }

    pub fn round_rect_stroke(
        &mut self,
        bounds: Bounds,
        radius: f32,
        color: Rgb,
        alpha: f32,
        width: f32,
    ) {
        let half = width / 2.0;
        if let Some(path) = round_rect_path(bounds.inset(half, half), (radius - half).max(0.0)) {
            self.stroke_path(&path, color, alpha, width);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgb, alpha: f32) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            self.fill_path(&path, color, alpha);
        }
    }

    pub fn line(&mut self, points: &[(f32, f32)], color: Rgb, alpha: f32, width: f32) {
        if let Some(path) = polyline(points) {
            self.stroke_path(&path, color, alpha, width);
        }
    }

    /// Arc around (`cx`, `cy`), angles in degrees clockwise from 3 o'clock.
    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        start: f32,
        sweep: f32,
        color: Rgb,
        alpha: f32,
        width: f32,
    ) {
        if sweep.abs() < 0.01 {
            return;
        }
        let steps = ((sweep.abs() / 6.0).ceil() as usize).max(2);
        let points: Vec<(f32, f32)> = (0..=steps)
            .map(|i| {
                let angle = (start + sweep * i as f32 / steps as f32).to_radians();
                (cx + r * angle.cos(), cy + r * angle.sin())
            })
            .collect();
        self.line(&points, color, alpha, width);
    }

    /// History graph: a line over a vertical fade. `values` are 0..=1.
    pub fn sparkline(
        &mut self,
        bounds: Bounds,
        values: &[f32],
        color: Rgb,
        fill_alpha: f32,
        line_width: f32,
    ) {
        if values.len() < 2 {
            return;
        }
        let step = bounds.w / (values.len() - 1) as f32;
        let points: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    bounds.x + step * i as f32,
                    bounds.bottom() - v.clamp(0.0, 1.0) * bounds.h,
                )
            })
            .collect();
        let mut pb = PathBuilder::new();
        pb.move_to(bounds.x, bounds.bottom());
        for (x, y) in &points {
            pb.line_to(*x, *y);
        }
        pb.line_to(bounds.right(), bounds.bottom());
        pb.close();
        if let (Some(path), Some(shader)) = (
            pb.finish(),
            LinearGradient::new(
                Point::from_xy(0.0, bounds.y),
                Point::from_xy(0.0, bounds.bottom()),
                vec![
                    GradientStop::new(0.0, color.skia(fill_alpha)),
                    GradientStop::new(1.0, color.skia(0.0)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ),
        ) {
            self.fill_shader(&path, shader);
        }
        self.line(&points, color, 1.0, line_width);
    }

    pub fn fill_path(&mut self, path: &Path, color: Rgb, alpha: f32) {
        let mut paint = Paint::default();
        paint.set_color(color.skia(alpha));
        paint.anti_alias = true;
        let mask = self.mask();
        self.pixmap.fill_path(
            path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            mask.as_ref(),
        );
    }

    pub fn fill_shader(&mut self, path: &Path, shader: Shader) {
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let mask = self.mask();
        self.pixmap.fill_path(
            path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            mask.as_ref(),
        );
    }

    pub fn stroke_path(&mut self, path: &Path, color: Rgb, alpha: f32, width: f32) {
        let mut paint = Paint::default();
        paint.set_color(color.skia(alpha));
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        let mask = self.mask();
        self.pixmap
            .stroke_path(path, &paint, &stroke, Transform::identity(), mask.as_ref());
    }

    /// Composites a premultiplied RGBA pixmap at (`x`, `y`) with `alpha`.
    pub fn draw_pixmap(&mut self, x: i32, y: i32, pixmap: tiny_skia::PixmapRef, alpha: f32) {
        let paint = tiny_skia::PixmapPaint {
            opacity: alpha.clamp(0.0, 1.0),
            quality: tiny_skia::FilterQuality::Nearest,
            ..tiny_skia::PixmapPaint::default()
        };
        let mask = self.mask();
        self.pixmap
            .draw_pixmap(x, y, pixmap, &paint, Transform::identity(), mask.as_ref());
    }

    /// Draws a scaled pixmap into `bounds` with rounded corners.
    pub fn draw_image(
        &mut self,
        bounds: Bounds,
        image: tiny_skia::PixmapRef,
        radius: f32,
        alpha: f32,
    ) {
        let sx = bounds.w / image.width() as f32;
        let sy = bounds.h / image.height() as f32;
        let shader = tiny_skia::Pattern::new(
            image,
            SpreadMode::Pad,
            tiny_skia::FilterQuality::Bicubic,
            alpha.clamp(0.0, 1.0),
            Transform::from_row(sx, 0.0, 0.0, sy, bounds.x, bounds.y),
        );
        let path = if radius > 0.0 {
            round_rect_path(bounds, radius)
        } else {
            Some(rect_path(bounds))
        };
        if let Some(path) = path {
            self.fill_shader(&path, shader);
        }
    }

    /// Blends one pixel; coverage-based text and bitmap glyphs use this.
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, color: Rgb, alpha: f32) {
        if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 || alpha <= 0.0 {
            return;
        }
        if let Some((left, right)) = self.clip {
            if (x as f32) < left || (x as f32) >= right {
                return;
            }
        }
        let i = (y as usize * WIDTH + x as usize) * 4;
        let data = self.pixmap.data_mut();
        let a = alpha.min(1.0);
        let inv = 1.0 - a;
        data[i] = (color.r as f32 * a + data[i] as f32 * inv + 0.5) as u8;
        data[i + 1] = (color.g as f32 * a + data[i + 1] as f32 * inv + 0.5) as u8;
        data[i + 2] = (color.b as f32 * a + data[i + 2] as f32 * inv + 0.5) as u8;
        data[i + 3] = 255;
    }

    pub fn text(&mut self, x: f32, baseline: f32, text: &str, style: &TextStyle) -> f32 {
        text::draw(self, x, baseline, text, style)
    }

    /// Draws `text` right-aligned so it ends at `right`.
    pub fn text_right(&mut self, right: f32, baseline: f32, text: &str, style: &TextStyle) -> f32 {
        let w = text::measure(text, style);
        text::draw(self, right - w, baseline, text, style);
        w
    }

    pub fn text_centered(
        &mut self,
        center: f32,
        baseline: f32,
        text: &str,
        style: &TextStyle,
    ) -> f32 {
        let w = text::measure(text, style);
        text::draw(self, (center - w / 2.0).round(), baseline, text, style);
        w
    }

    /// Draws `text` within `max_width`, ending with an ellipsis if it does not
    /// fit.
    pub fn text_fit(
        &mut self,
        x: f32,
        baseline: f32,
        text: &str,
        style: &TextStyle,
        max_width: f32,
    ) -> f32 {
        let fitted = text::ellipsize(text, style, max_width);
        text::draw(self, x, baseline, &fitted, style)
    }

    pub fn icon(&mut self, name: &str, x: f32, y: f32, size: f32, color: Rgb, alpha: f32) {
        crate::icons::draw(self, name, x, y, size, color, alpha);
    }

    pub fn font_metrics(face: FontFace, size: f32) -> (f32, f32) {
        text::line_metrics(face, size)
    }
}

pub fn rect_path(bounds: Bounds) -> Path {
    let rect = Rect::from_xywh(bounds.x, bounds.y, bounds.w.max(0.01), bounds.h.max(0.01))
        .unwrap_or_else(|| Rect::from_xywh(0.0, 0.0, 0.01, 0.01).expect("valid rect"));
    PathBuilder::from_rect(rect)
}

pub fn round_rect_path(bounds: Bounds, radius: f32) -> Option<Path> {
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return None;
    }
    let r = radius.min(bounds.w / 2.0).min(bounds.h / 2.0).max(0.0);
    let (x, y, w, h) = (bounds.x, bounds.y, bounds.w, bounds.h);
    // Circle-approximating cubic control distance.
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn polyline(points: &[(f32, f32)]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for (x, y) in rest {
        pb.line_to(*x, *y);
    }
    pb.finish()
}
