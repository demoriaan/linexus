//! Two-row headline ticker. The top row belongs to the left half of the
//! widget and the bottom row to the right half (the strip reports X only),
//! which the accent bar on each row's tap side makes visible.

use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::{MarqueeStyle, NewsRow};
use crate::text::{self, FontFace, TextStyle};
use nexus_config::{NewsFont, NewsProps};

/// Widest source label; longer feed names are cut.
const LABEL_MAX_W: f32 = 64.0;
const ROW_H: f32 = 22.0;
const BAR_W: f32 = 3.0;
const MARQUEE_END_PAD: u32 = 12;
/// Floor for headlines that fit, so short titles don't fly past.
const STATIC_MIN_DWELL_MS: u64 = 4_000;
/// 5 characters per second is comfortable reading at arm's length.
const STATIC_PER_CHAR_MS: u64 = 200;

fn headline_style(font: NewsFont, color: crate::Rgb) -> TextStyle {
    match font {
        NewsFont::Pixel => TextStyle::new(FontFace::Pixel, 20.0, color),
        NewsFont::Smooth => TextStyle::new(FontFace::SemiBold, 17.0, color),
    }
}

/// Pixel width of a headline in the widget's font.
pub fn headline_width(text: &str, font: NewsFont) -> u32 {
    text::measure(text, &headline_style(font, crate::Rgb::new(0, 0, 0))).ceil() as u32
}

/// Conservative headline viewport for a news widget `widget_w` pixels wide
/// (assumes the widest source label). The daemon paces the rotation with it.
pub fn viewport_estimate(widget_w: u16) -> u32 {
    (f32::from(widget_w) - 4.0 - BAR_W - 6.0 - LABEL_MAX_W - 8.0 - 8.0).max(40.0) as u32
}

/// Whether `text` overflows the viewport and scrolls.
pub fn headline_scrolls(text: &str, font: NewsFont, viewport: u32) -> bool {
    headline_width(text, font) > viewport
}

/// How long a headline needs on screen: a reading dwell when it fits, or a
/// full marquee run (start pause + scroll + end pause) when it overflows.
pub fn headline_dwell_ms(text: &str, font: NewsFont, viewport: u32, style: &MarqueeStyle) -> u64 {
    let text_w = headline_width(text, font);
    if text_w <= viewport {
        let chars = text.chars().count() as u64;
        (chars * STATIC_PER_CHAR_MS).max(STATIC_MIN_DWELL_MS)
    } else {
        let scroll_px = text_w - viewport + MARQUEE_END_PAD;
        let scroll_ms = u64::from(scroll_px) * 1_000 / u64::from(style.speed_px_per_sec.max(1));
        style.start_pause_ms + scroll_ms + style.end_pause_ms
    }
}

fn marquee_offset(text_w: u32, viewport: u32, phase_ms: u64, style: &MarqueeStyle) -> u32 {
    if text_w <= viewport {
        return 0;
    }
    let scroll_px = text_w - viewport + MARQUEE_END_PAD;
    let elapsed = phase_ms.saturating_sub(style.start_pause_ms);
    let px = elapsed.saturating_mul(u64::from(style.speed_px_per_sec)) / 1_000;
    px.min(u64::from(scroll_px)) as u32
}

pub fn draw(canvas: &mut Canvas, b: Bounds, props: &NewsProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let news = &ctx.live.news;
    if news.rows.is_empty() {
        canvas.icon("newspaper", b.x + 12.0, 14.0, 20.0, palette.text_dim, 1.0);
        let title = TextStyle::new(FontFace::SemiBold, 15.0, palette.text_soft);
        canvas.text_fit(
            b.x + 42.0,
            22.0,
            format::word(Word::NoHeadlines, ctx.live.locale),
            &title,
            b.w - 50.0,
        );
        canvas.text_fit(
            b.x + 42.0,
            38.0,
            format::word(Word::AddFeedsInStudio, ctx.live.locale),
            &TextStyle::new(FontFace::Medium, 12.0, palette.text_dim),
            b.w - 50.0,
        );
        return;
    }
    for (idx, row) in news.rows.iter().take(2).enumerate() {
        let y = if idx == 0 { 2.0 } else { 25.0 };
        draw_row(
            canvas,
            Bounds::new(b.x + 2.0, y, b.w - 4.0, ROW_H),
            row,
            idx == 0,
            news.highlight == Some(idx),
            props.font,
            ctx,
        );
    }
}

fn draw_row(
    canvas: &mut Canvas,
    r: Bounds,
    row: &NewsRow,
    bar_left: bool,
    highlighted: bool,
    font: NewsFont,
    ctx: &Ctx,
) {
    let palette = ctx.palette;
    let news = &ctx.live.news;
    canvas.round_rect(r, 4.0, palette.surface, 1.0);
    if highlighted {
        canvas.round_rect(r, 4.0, palette.accent, 0.22);
        canvas.round_rect_stroke(r, 4.0, palette.text, 0.9, 1.0);
    }
    let bar_color = if highlighted {
        palette.text
    } else {
        palette.accent
    };
    let bar_x = if bar_left {
        r.x + 1.0
    } else {
        r.right() - 1.0 - BAR_W
    };
    canvas.round_rect(
        Bounds::new(bar_x, r.y + 3.0, BAR_W, r.h - 6.0),
        1.5,
        bar_color,
        1.0,
    );

    let inner_left = if bar_left {
        r.x + BAR_W + 7.0
    } else {
        r.x + 8.0
    };
    let inner_right = if bar_left {
        r.right() - 8.0
    } else {
        r.right() - BAR_W - 7.0
    };
    let (source_color, headline_color) = if row.seen {
        (palette.text_dim, palette.text_dim)
    } else {
        (palette.accent, palette.text)
    };

    let label_style = TextStyle::new(FontFace::SemiBold, 11.0, source_color).tracking(0.5);
    let mut label = short_source(&row.source).to_uppercase();
    while text::measure(&label, &label_style) > LABEL_MAX_W && label.pop().is_some() {}
    let baseline = r.y + 15.0;
    let label_w = canvas.text(inner_left, baseline, &label, &label_style);

    let headline_x = inner_left + label_w + 8.0;
    let viewport = (inner_right - headline_x).max(0.0) as u32;
    let text_w = headline_width(&row.headline, font);
    let offset = marquee_offset(text_w, viewport, news.marquee_phase_ms, &news.marquee);
    let previous_clip = canvas.clip_x(headline_x, inner_right);
    let style = headline_style(font, headline_color);
    let x = headline_x - offset as f32;
    match font {
        NewsFont::Pixel => text::draw_pixel(
            canvas,
            x as i32,
            r.y as i32 + 1,
            &row.headline,
            headline_color,
            1.0,
        ),
        NewsFont::Smooth => {
            canvas.text(x, r.y + 16.0, &row.headline, &style);
        }
    }
    canvas.set_clip(previous_clip);
}

/// Feed titles often read "Site - Section"; the site name is enough here.
fn short_source(source: &str) -> &str {
    [" - ", " – ", " | ", ": "]
        .iter()
        .filter_map(|sep| source.find(sep))
        .min()
        .map_or(source, |end| &source[..end])
        .trim()
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_labels_drop_the_section() {
        assert_eq!(super::short_source("n-tv.de - Politik"), "n-tv.de");
        assert_eq!(super::short_source("heise online: News"), "heise online");
        assert_eq!(super::short_source("tagesschau"), "tagesschau");
    }

    use super::*;

    #[test]
    fn short_headlines_dwell_for_reading_time() {
        let style = MarqueeStyle::default();
        let viewport = viewport_estimate(640);
        assert_eq!(
            headline_dwell_ms("Kurz", NewsFont::Pixel, viewport, &style),
            STATIC_MIN_DWELL_MS
        );
        let long = "Eine sehr lange Schlagzeile, die garantiert nicht in eine einzige Zeile des Streifens passt und scrollen muss";
        assert!(headline_scrolls(long, NewsFont::Pixel, viewport));
        let dwell = headline_dwell_ms(long, NewsFont::Pixel, viewport, &style);
        assert!(dwell > style.start_pause_ms + style.end_pause_ms);
    }

    #[test]
    fn marquee_parks_at_the_end() {
        let style = MarqueeStyle::default();
        assert_eq!(marquee_offset(500, 400, 0, &style), 0);
        assert_eq!(
            marquee_offset(500, 400, 1_000_000, &style),
            100 + MARQUEE_END_PAD
        );
        assert_eq!(marquee_offset(300, 400, 1_000_000, &style), 0);
    }
}
