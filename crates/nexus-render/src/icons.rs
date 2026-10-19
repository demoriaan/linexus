//! Icons: the embedded Lucide set (stroke icons, recoloured on the fly) and
//! user images given as `data:` URLs.

use crate::canvas::{Bounds, Canvas};
use crate::Rgb;
use base64::Engine as _;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{Pixmap, Transform};

const LUCIDE: &str = include_str!("../assets/icons/lucide.txt");

pub(crate) fn catalog_text() -> &'static str {
    LUCIDE
}

pub struct IconInfo {
    pub name: &'static str,
    pub tags: &'static str,
    body: &'static str,
}

fn catalog() -> &'static HashMap<&'static str, IconInfo> {
    static CATALOG: OnceLock<HashMap<&'static str, IconInfo>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        LUCIDE
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let name = parts.next()?;
                let tags = parts.next()?;
                let body = parts.next()?;
                Some((name, IconInfo { name, tags, body }))
            })
            .collect()
    })
}

/// All embedded icon names with their search tags, sorted by name.
pub fn list() -> Vec<(&'static str, &'static str)> {
    let mut all: Vec<_> = catalog()
        .values()
        .map(|info| (info.name, info.tags))
        .collect();
    all.sort_unstable();
    all
}

pub fn exists(name: &str) -> bool {
    catalog().contains_key(lucide_name(name))
}

fn lucide_name(name: &str) -> &str {
    name.strip_prefix("lucide:").unwrap_or(name)
}

/// Stroke width in icon units so the drawn line stays about 1.6-2 px.
fn stroke_units(size: f32) -> f32 {
    let target_px = (size / 11.0).clamp(1.5, 2.6);
    (target_px * 24.0 / size).clamp(1.25, 2.75)
}

type IconKey = (String, u32, Rgb, bool);

fn rasterize(name: &str, size: f32, color: Rgb, filled: bool) -> Option<Arc<Pixmap>> {
    static CACHE: OnceLock<Mutex<HashMap<IconKey, Arc<Pixmap>>>> = OnceLock::new();
    let key = (name.to_string(), (size * 4.0) as u32, color, filled);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Some(hit);
    }
    let info = catalog().get(lucide_name(name))?;
    let hex = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
    let fill = if filled { hex.as_str() } else { "none" };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="{fill}" stroke="{hex}" stroke-width="{:.2}" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
        stroke_units(size),
        info.body
    );
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()?;
    let px = size.ceil().max(1.0) as u32 + 2;
    let mut pixmap = Pixmap::new(px, px)?;
    let scale = size / 24.0;
    resvg::render(
        &tree,
        Transform::from_row(scale, 0.0, 0.0, scale, 1.0, 1.0),
        &mut pixmap.as_mut(),
    );
    let pixmap = Arc::new(pixmap);
    if let Ok(mut cache) = cache.lock() {
        if cache.len() > 512 {
            cache.clear();
        }
        cache.insert(key, pixmap.clone());
    }
    Some(pixmap)
}

/// Draws a Lucide icon with its top-left corner at (`x`, `y`).
pub fn draw(canvas: &mut Canvas, name: &str, x: f32, y: f32, size: f32, color: Rgb, alpha: f32) {
    if let Some(pixmap) = rasterize(name, size, color, false) {
        canvas.draw_pixmap(
            x.round() as i32 - 1,
            y.round() as i32 - 1,
            pixmap.as_ref().as_ref(),
            alpha,
        );
    }
}

/// Like [`draw`] but fills the outline too (play, pause, skip glyphs).
pub fn draw_filled(
    canvas: &mut Canvas,
    name: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Rgb,
    alpha: f32,
) {
    if let Some(pixmap) = rasterize(name, size, color, true) {
        canvas.draw_pixmap(
            x.round() as i32 - 1,
            y.round() as i32 - 1,
            pixmap.as_ref().as_ref(),
            alpha,
        );
    }
}

/// Longest side a decoded config image is kept at. The strip is 640 x 48,
/// so this leaves room for cropping without holding full-size photos in the
/// cache.
const MAX_IMAGE_SIDE: u32 = 1280;
/// Raster images larger than this are refused before decoding: either side
/// above 8192 px, or more pixels than a 6000 x 4000 photo (about 96 MiB
/// once decoded).
const MAX_SOURCE_SIDE: u32 = 8192;
const MAX_SOURCE_PIXELS: u64 = 24 * 1024 * 1024;
/// The least recently used images leave the cache once it holds more than
/// this many pixmap bytes or images.
const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CACHE_IMAGES: usize = 64;

/// Decoded images by content hash, with the tick they were last used.
#[derive(Default)]
struct ImageCache {
    tick: u64,
    images: HashMap<u64, (Arc<Pixmap>, u64)>,
}

impl ImageCache {
    fn get(&mut self, key: u64) -> Option<Arc<Pixmap>> {
        self.tick += 1;
        let tick = self.tick;
        self.images.get_mut(&key).map(|(pixmap, used)| {
            *used = tick;
            pixmap.clone()
        })
    }

    fn insert(&mut self, key: u64, pixmap: Arc<Pixmap>) {
        self.tick += 1;
        let size = pixmap.data().len();
        let mut total: usize = self.images.values().map(|(p, _)| p.data().len()).sum();
        while !self.images.is_empty()
            && (self.images.len() >= MAX_CACHE_IMAGES || total + size > MAX_CACHE_BYTES)
        {
            let oldest = self
                .images
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| *key);
            if let Some((evicted, _)) = oldest.and_then(|key| self.images.remove(&key)) {
                total -= evicted.data().len();
            }
        }
        self.images.insert(key, (pixmap, self.tick));
    }
}

/// Decodes a `data:image/...;base64,` URL (PNG, JPEG, GIF first frame, or
/// SVG) into a premultiplied pixmap, cached by content. Raster images are
/// scaled down to 1280 px on the longer side; oversized ones are refused.
pub fn data_url_image(data_url: &str) -> Option<Arc<Pixmap>> {
    static CACHE: OnceLock<Mutex<ImageCache>> = OnceLock::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data_url.hash(&mut hasher);
    let key = hasher.finish();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(key)) {
        return Some(hit);
    }
    let (meta, payload) = data_url.strip_prefix("data:")?.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()?;
    let pixmap = if meta.starts_with("image/svg") {
        let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default()).ok()?;
        let size = tree.size();
        // Rasterize vector images at a size that suits a 48 px strip.
        let scale = (96.0 / size.width().max(size.height())).min(4.0);
        let mut pixmap = Pixmap::new(
            (size.width() * scale).ceil() as u32,
            (size.height() * scale).ceil() as u32,
        )?;
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap
    } else {
        decode_raster(&bytes, MAX_IMAGE_SIDE)?
    };
    let pixmap = Arc::new(pixmap);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, pixmap.clone());
    }
    Some(pixmap)
}

/// Whether a raster image of this size may be decoded at all.
fn within_decode_budget(width: u32, height: u32) -> bool {
    width.max(height) <= MAX_SOURCE_SIDE
        && u64::from(width) * u64::from(height) <= MAX_SOURCE_PIXELS
}

/// Decodes PNG, JPEG or GIF bytes with explicit limits: the header is read
/// first and oversized images are refused before any pixel is decoded; the
/// result is scaled so its longer side is at most `max`.
fn decode_raster(bytes: &[u8], max: u32) -> Option<Pixmap> {
    let reader = || image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format();
    let (width, height) = reader().ok()?.into_dimensions().ok()?;
    if !within_decode_budget(width, height) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_SOURCE_PIXELS * 4 + 16 * 1024 * 1024);
    let mut reader = reader().ok()?;
    reader.limits(limits);
    let image = reader.decode().ok()?;
    // `thumbnail` averages straight into the output size; `resize` would
    // allocate a float buffer of source width x target height first.
    let image = if image.width().max(image.height()) > max {
        image.thumbnail(max, max)
    } else {
        image
    };
    rgba_to_pixmap(image.to_rgba8())
}

/// Rasterizes SVG bytes into a `size` square (application icons).
pub fn rasterize_svg(bytes: &[u8], size: u32) -> Option<Pixmap> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let (w, h) = (tree.size().width(), tree.size().height());
    let scale = size as f32 / w.max(h);
    let mut pixmap = Pixmap::new(size, size)?;
    let dx = (size as f32 - w * scale) / 2.0;
    let dy = (size as f32 - h * scale) / 2.0;
    resvg::render(
        &tree,
        Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// Decodes PNG, JPEG or GIF bytes (or SVG) into a pixmap whose longer side
/// is at most `max` pixels.
pub fn decode_image(bytes: &[u8], max: u32) -> Option<Pixmap> {
    if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") {
        return rasterize_svg(bytes, max);
    }
    decode_raster(bytes, max)
}

/// Converts straight RGBA into a premultiplied pixmap.
pub fn rgba_to_pixmap(image: image::RgbaImage) -> Option<Pixmap> {
    let (w, h) = image.dimensions();
    let mut data = image.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u16;
        px[0] = ((px[0] as u16 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u16 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u16 * a + 127) / 255) as u8;
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// Draws an image icon scaled to fit a `size` square at (`x`, `y`).
pub fn draw_image_icon(canvas: &mut Canvas, image: &Pixmap, x: f32, y: f32, size: f32, alpha: f32) {
    let (w, h) = (image.width() as f32, image.height() as f32);
    let scale = size / w.max(h);
    let (dw, dh) = (w * scale, h * scale);
    canvas.draw_image(
        Bounds::new(x + (size - dw) / 2.0, y + (size - dh) / 2.0, dw, dh),
        image.as_ref(),
        0.0,
        alpha,
    );
}

/// Average colour of an image, weighted towards saturated pixels; used to
/// tint the media widget after the album art.
pub fn dominant_color(image: &Pixmap) -> Rgb {
    let (mut r, mut g, mut b, mut weight) = (0f32, 0f32, 0f32, 0f32);
    for px in image.pixels().iter().step_by(7) {
        let c = px.demultiply();
        let (pr, pg, pb) = (c.red() as f32, c.green() as f32, c.blue() as f32);
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let w = 0.05 + (max - min) / 255.0;
        r += pr * w;
        g += pg * w;
        b += pb * w;
        weight += w;
    }
    if weight == 0.0 {
        return Rgb::new(90, 100, 120);
    }
    Rgb::new((r / weight) as u8, (g / weight) as u8, (b / weight) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_cache_drops_the_least_recently_used_image() {
        let mut cache = ImageCache::default();
        let pixmap = || Arc::new(Pixmap::new(4, 4).unwrap());
        for key in 0..MAX_CACHE_IMAGES as u64 {
            cache.insert(key, pixmap());
        }
        // Image 0 is in use every frame; image 1 is not.
        assert!(cache.get(0).is_some());
        cache.insert(1_000, pixmap());
        assert!(cache.get(0).is_some());
        assert!(cache.get(1).is_none());
        assert!(cache.get(1_000).is_some());
        assert_eq!(cache.images.len(), MAX_CACHE_IMAGES);
    }

    fn png_data_url(width: u32, height: u32) -> String {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(width, height, image::Rgba([200, 40, 40, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let payload = base64::engine::general_purpose::STANDARD.encode(png);
        format!("data:image/png;base64,{payload}")
    }

    #[test]
    fn large_images_are_scaled_down_before_caching() {
        let wide = data_url_image(&png_data_url(3000, 300)).unwrap();
        assert_eq!(wide.width(), MAX_IMAGE_SIDE);
        assert!((127..=128).contains(&wide.height()), "{}", wide.height());
        let small = data_url_image(&png_data_url(64, 32)).unwrap();
        assert_eq!((small.width(), small.height()), (64, 32));
    }

    #[test]
    fn oversized_images_are_refused_before_decoding() {
        assert!(data_url_image(&png_data_url(MAX_SOURCE_SIDE + 1, 1)).is_none());
        assert!(
            within_decode_budget(6000, 4000),
            "a camera photo still decodes"
        );
        assert!(!within_decode_budget(6000, 6000));
        assert!(!within_decode_budget(1, MAX_SOURCE_SIDE + 1));
    }
}
