//! Text on the strip: Barlow Semi Condensed (vector, anti-aliased) for the
//! interface and Terminus Bold 10x20 (bitmap) for the pixel headline style.

mod compose;

use crate::canvas::Canvas;
use crate::font_headline;
use crate::Rgb;
use fontdue::{Font, FontSettings, Metrics};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The strip's UI font (Barlow Semi Condensed, SIL OFL 1.1), also served to
/// Studio so the editor and the device share one typeface.
pub const MEDIUM_TTF: &[u8] = include_bytes!("../assets/fonts/BarlowSemiCondensed-Medium.ttf");
pub const SEMIBOLD_TTF: &[u8] = include_bytes!("../assets/fonts/BarlowSemiCondensed-SemiBold.ttf");
pub const BOLD_TTF: &[u8] = include_bytes!("../assets/fonts/BarlowSemiCondensed-Bold.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFace {
    Medium,
    SemiBold,
    Bold,
    /// Terminus Bold 10x20 bitmap font; `size` is ignored.
    Pixel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub face: FontFace,
    pub size: f32,
    pub color: Rgb,
    pub alpha: f32,
    /// Extra space between letters in pixels.
    pub tracking: f32,
    /// Give every digit the advance of the widest one so changing numbers do
    /// not jitter.
    pub tabular: bool,
}

impl TextStyle {
    pub const fn new(face: FontFace, size: f32, color: Rgb) -> Self {
        Self {
            face,
            size,
            color,
            alpha: 1.0,
            tracking: 0.0,
            tabular: false,
        }
    }

    pub const fn tracking(mut self, tracking: f32) -> Self {
        self.tracking = tracking;
        self
    }

    pub const fn tabular(mut self) -> Self {
        self.tabular = true;
        self
    }

    pub const fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }

    pub const fn color(mut self, color: Rgb) -> Self {
        self.color = color;
        self
    }
}

fn font(face: FontFace) -> &'static Font {
    static FONTS: OnceLock<[Font; 3]> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let load = |bytes: &[u8]| {
            Font::from_bytes(
                bytes,
                FontSettings {
                    scale: 40.0,
                    ..FontSettings::default()
                },
            )
            .expect("embedded font parses")
        };
        [load(MEDIUM_TTF), load(SEMIBOLD_TTF), load(BOLD_TTF)]
    });
    match face {
        FontFace::Medium => &fonts[0],
        FontFace::SemiBold | FontFace::Pixel => &fonts[1],
        FontFace::Bold => &fonts[2],
    }
}

type GlyphKey = (FontFace, char, u32);
type Glyph = Arc<(Metrics, Vec<u8>)>;

fn glyph(face: FontFace, ch: char, size: f32) -> Glyph {
    static CACHE: OnceLock<Mutex<HashMap<GlyphKey, Glyph>>> = OnceLock::new();
    let key = (face, ch, (size * 4.0).round() as u32);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return hit;
    }
    let raster = Arc::new(font(face).rasterize(ch, size));
    if let Ok(mut cache) = cache.lock() {
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(key, raster.clone());
    }
    raster
}

/// What `face` draws for `ch`:
/// - the character itself when the font has it;
/// - a plain space for whitespace it lacks (thin or narrow no-break spaces);
/// - an ASCII stand-in for common arrows, flats and sharps, stars,
///   triangles and crosses (`→` as `->`, `E♭` as `Eb`);
/// - nothing for emoji, pictographs, control characters and the invisible
///   characters that join, vary or decorate them;
/// - otherwise the character anyway, which shows the font's placeholder
///   box, so a missing letter still shows that one is there.
fn shown(face: FontFace, ch: char) -> Shown {
    if has_glyph(face, ch) {
        Shown::One(Some(ch))
    } else if ch.is_whitespace() {
        Shown::One(Some(' '))
    } else if let Some(text) = stand_in(ch) {
        Shown::Text(text.chars())
    } else if ch.is_control() || is_emoji_or_format(ch) {
        Shown::One(None)
    } else {
        Shown::One(Some(ch))
    }
}

enum Shown {
    One(Option<char>),
    Text(std::str::Chars<'static>),
}

impl Iterator for Shown {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Shown::One(ch) => ch.take(),
            Shown::Text(chars) => chars.next(),
        }
    }
}

fn has_glyph(face: FontFace, ch: char) -> bool {
    font(face).lookup_glyph_index(ch) != 0
}

/// `text` as `face` draws it. A letter followed by a combining accent
/// becomes the composed letter when the font has that.
fn drawn(text: &str, face: FontFace) -> impl Iterator<Item = char> + '_ {
    let mut chars = text.chars().peekable();
    std::iter::from_fn(move || {
        let ch = chars.next()?;
        let composed = chars
            .peek()
            .and_then(|&mark| compose::compose(ch, mark))
            .filter(|&letter| has_glyph(face, letter));
        if composed.is_some() {
            chars.next();
        }
        Some(composed.unwrap_or(ch))
    })
    .flat_map(move |ch| shown(face, ch))
}

fn stand_in(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{2192}' | '\u{27F6}' | '\u{2794}' | '\u{279C}' | '\u{27A1}' => "->",
        '\u{2190}' | '\u{27F5}' | '\u{2B05}' => "<-",
        '\u{2194}' => "<->",
        '\u{21D2}' => "=>",
        '\u{21D0}' => "<=",
        '\u{2191}' | '\u{2B06}' | '\u{25B2}' | '\u{25B4}' | '\u{25B3}' => "^",
        '\u{2193}' | '\u{2B07}' | '\u{25BC}' | '\u{25BE}' | '\u{25BD}' => "v",
        '\u{25B6}' | '\u{25BA}' | '\u{25B8}' => ">",
        '\u{25C0}' | '\u{25C4}' | '\u{25C2}' => "<",
        '\u{266D}' => "b",
        '\u{266F}' => "#",
        '\u{2605}' | '\u{272D}' | '\u{2B50}' => "*",
        '\u{2606}' => "\u{B7}",
        '\u{2713}' | '\u{2714}' | '\u{2705}' => "+",
        '\u{2717}' | '\u{2718}' | '\u{2715}' | '\u{2716}' | '\u{274C}' => "x",
        _ => return None,
    })
}

/// Emoji and pictographs (left out rather than drawn as boxes), and the
/// invisible characters that join, vary or decorate letters and emoji.
fn is_emoji_or_format(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0xAD
            | 0x300..=0x36F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x20D0..=0x20FF
            | 0x2300..=0x23FF
            | 0x25A0..=0x27BF
            | 0x2B00..=0x2BFF
            | 0x3030
            | 0x303D
            | 0x3297
            | 0x3299
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0x1F000..=0x1FBFF
            | 0xE0000..=0xE01EF
    )
}

/// `text` as `face` draws it, with the runs of whitespace that left-out
/// characters can leave behind collapsed to one space.
pub fn printable(text: &str, face: FontFace) -> String {
    let kept: String = drawn(text, face).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn digit_advance(face: FontFace, size: f32) -> f32 {
    ('0'..='9')
        .map(|d| font(face).metrics(d, size).advance_width)
        .fold(0.0, f32::max)
}

/// (ascent, descent) in pixels for the face at `size`.
pub fn line_metrics(face: FontFace, size: f32) -> (f32, f32) {
    if face == FontFace::Pixel {
        return (15.0, 5.0);
    }
    font(face)
        .horizontal_line_metrics(size)
        .map(|m| (m.ascent, -m.descent))
        .unwrap_or((size * 0.8, size * 0.2))
}

/// Height of a capital letter, for vertically centring labels.
pub fn cap_height(face: FontFace, size: f32) -> f32 {
    if face == FontFace::Pixel {
        return 13.0;
    }
    let m = font(face).metrics('H', size);
    m.height as f32
}

pub fn measure(text: &str, style: &TextStyle) -> f32 {
    if style.face == FontFace::Pixel {
        return pixel_text_width(text) as f32;
    }
    let f = font(style.face);
    let digit = style.tabular.then(|| digit_advance(style.face, style.size));
    let mut width = 0.0;
    let mut prev: Option<char> = None;
    for ch in drawn(text, style.face) {
        if let (Some(p), None) = (prev, digit) {
            width += f.horizontal_kern(p, ch, style.size).unwrap_or(0.0);
        }
        width += match digit {
            Some(d) if ch.is_ascii_digit() => d,
            _ => f.metrics(ch, style.size).advance_width,
        };
        width += style.tracking;
        prev = Some(ch);
    }
    if prev.is_some() {
        width -= style.tracking;
    }
    width
}

/// Draws `text` with its baseline at `baseline`; returns the advance.
pub fn draw(canvas: &mut Canvas, x: f32, baseline: f32, text: &str, style: &TextStyle) -> f32 {
    if style.face == FontFace::Pixel {
        // The bitmap cell's baseline sits 15 px below its top.
        let top = baseline.round() as i32 - 15;
        draw_pixel(
            canvas,
            x.round() as i32,
            top,
            text,
            style.color,
            style.alpha,
        );
        return pixel_text_width(text) as f32;
    }
    let f = font(style.face);
    let digit = style.tabular.then(|| digit_advance(style.face, style.size));
    let mut pen = x;
    let mut prev: Option<char> = None;
    let base = baseline.round() as i32;
    for ch in drawn(text, style.face) {
        if let (Some(p), None) = (prev, digit) {
            pen += f.horizontal_kern(p, ch, style.size).unwrap_or(0.0);
        }
        let raster = glyph(style.face, ch, style.size);
        let (metrics, coverage) = (&raster.0, &raster.1);
        let advance = match digit {
            Some(d) if ch.is_ascii_digit() => d,
            _ => metrics.advance_width,
        };
        let centre = match digit {
            Some(d) if ch.is_ascii_digit() => (d - metrics.advance_width) / 2.0,
            _ => 0.0,
        };
        let gx = (pen + centre).round() as i32 + metrics.xmin;
        let gy = base - metrics.height as i32 - metrics.ymin;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let c = coverage[row * metrics.width + col];
                if c == 0 {
                    continue;
                }
                // A slight gamma lift keeps thin strokes from washing out on
                // the dark, low-density panel.
                let a = (c as f32 / 255.0).powf(0.85) * style.alpha;
                canvas.blend_pixel(gx + col as i32, gy + row as i32, style.color, a);
            }
        }
        pen += advance + style.tracking;
        prev = Some(ch);
    }
    (pen - x - if prev.is_some() { style.tracking } else { 0.0 }).max(0.0)
}

/// `text` shortened with an ellipsis so it fits `max_width`.
pub fn ellipsize(text: &str, style: &TextStyle, max_width: f32) -> String {
    if measure(text, style) <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let budget = max_width - measure(ellipsis, style);
    let mut out = String::new();
    for ch in text.chars() {
        out.push(ch);
        if measure(&out, style) > budget {
            out.pop();
            break;
        }
    }
    let trimmed = out.trim_end();
    format!("{trimmed}{ellipsis}")
}

/// Gap between two inked glyphs of the pixel font. Terminus Bold strokes
/// are 2 px wide, so a 2 px gap keeps letters from touching.
const PIXEL_LETTER_SPACING: i32 = 2;
/// Advance for a space and for characters the pixel font lacks.
const PIXEL_SPACE_ADVANCE: i32 = 6;

/// `(first inked column, inked width, rows)` for `ch`, or `None` for spaces
/// and characters outside the embedded subset.
fn pixel_glyph(ch: char) -> Option<(i32, i32, &'static [u16; font_headline::GLYPH_HEIGHT])> {
    let ch = match ch {
        // The Terminus subset has no capital sharp s; the lowercase form reads
        // fine inside a headline.
        'ẞ' => 'ß',
        _ => ch,
    };
    let code = ch as u32;
    let idx = font_headline::GLYPHS
        .binary_search_by_key(&code, |(cp, ..)| *cp)
        .ok()?;
    let (_, left, inked, rows) = &font_headline::GLYPHS[idx];
    Some((i32::from(*left), i32::from(*inked), rows))
}

/// Pixel width of `text` in the bitmap headline font.
pub fn pixel_text_width(text: &str) -> usize {
    let width: i32 = text
        .chars()
        .map(|ch| match pixel_glyph(ch) {
            Some((_, inked, _)) => inked + PIXEL_LETTER_SPACING,
            None => PIXEL_SPACE_ADVANCE,
        })
        .sum();
    (width - PIXEL_LETTER_SPACING).max(0) as usize
}

/// Draws bitmap text whose 20 px cell starts at `top`. The origin may sit
/// left of the visible area; the canvas clip bounds the output.
pub fn draw_pixel(canvas: &mut Canvas, x: i32, top: i32, text: &str, color: Rgb, alpha: f32) {
    let mut cursor = x;
    for ch in text.chars() {
        if cursor >= crate::WIDTH as i32 {
            break;
        }
        let Some((left, inked, rows)) = pixel_glyph(ch) else {
            cursor += PIXEL_SPACE_ADVANCE;
            continue;
        };
        if cursor + inked > 0 {
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..inked {
                    let bit = font_headline::GLYPH_CELL_WIDTH as i32 - 1 - (left + col);
                    if bits & (1 << bit) != 0 {
                        canvas.blend_pixel(cursor + col, top + row as i32, color, alpha);
                    }
                }
            }
        }
        cursor += inked + PIXEL_LETTER_SPACING;
    }
}
