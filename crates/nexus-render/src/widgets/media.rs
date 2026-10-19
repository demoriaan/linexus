use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::icons;
use crate::text::{self, FontFace, TextStyle};
use nexus_config::{MediaProps, Widget};

/// Width of the transport buttons block on the right.
const CONTROLS_W: f32 = 132.0;

/// Part of the media widget a tap lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaZone {
    /// Album art and titles: play/pause.
    Info,
    Previous,
    PlayPause,
    Next,
}

fn has_controls(width: f32, props: &MediaProps) -> bool {
    props.show_controls && width >= 300.0
}

/// Which zone of a media widget `x` (absolute strip X) falls into.
pub fn media_zone(widget_x: u16, widget_w: u16, props: &MediaProps, x: u16) -> MediaZone {
    let (left, width) = (f32::from(widget_x), f32::from(widget_w));
    if !has_controls(width, props) {
        return MediaZone::Info;
    }
    let controls_left = left + width - CONTROLS_W;
    let rel = f32::from(x) - controls_left;
    if rel < 0.0 {
        MediaZone::Info
    } else if rel < CONTROLS_W / 3.0 {
        MediaZone::Previous
    } else if rel < CONTROLS_W * 2.0 / 3.0 {
        MediaZone::PlayPause
    } else {
        MediaZone::Next
    }
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &MediaProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let accent = ctx.accent(widget, palette.accent);
    let locale = ctx.live.locale;
    let Some(media) = ctx.live.media.as_ref() else {
        canvas.icon("music", b.x + 14.0, 14.0, 20.0, palette.text_dim, 1.0);
        let style = TextStyle::new(FontFace::SemiBold, 15.0, palette.text_dim);
        canvas.text_fit(
            b.x + 44.0,
            30.0,
            format::word(Word::NothingPlaying, locale),
            &style,
            b.w - 52.0,
        );
        return;
    };

    let tint = media.art_color.unwrap_or(accent);
    if props.tint {
        canvas.horizontal_fade(b, tint, 0.32, 0.0);
    }
    press_glow(canvas, b, accent, ctx.pressed(widget));

    let controls = has_controls(b.w, props);
    let info_right = if controls {
        b.right() - CONTROLS_W
    } else {
        b.right() - 8.0
    };
    let mut text_x = b.x + 8.0;
    if props.show_art {
        let art = Bounds::new(b.x + 5.0, 5.0, 38.0, 38.0);
        match &media.art {
            Some(image) => canvas.draw_image(art, image.as_ref().as_ref(), 6.0, 1.0),
            None => {
                canvas.round_rect(art, 6.0, tint, 0.55);
                canvas.icon("music", art.x + 9.0, art.y + 9.0, 20.0, palette.text, 0.9);
            }
        }
        text_x = art.right() + 9.0;
    }
    let room = info_right - text_x - 6.0;
    let title_style = TextStyle::new(FontFace::Bold, 17.0, palette.text);
    let title = if media.title.is_empty() {
        media.player.as_str()
    } else {
        media.title.as_str()
    };
    canvas.text_fit(text_x, 20.0, title, &title_style, room);
    let mut subtitle = media.artist.clone();
    if !media.album.is_empty() && !media.artist.is_empty() {
        subtitle = format!("{} · {}", media.artist, media.album);
    } else if subtitle.is_empty() {
        subtitle = media.player.clone();
    }
    if !media.playing {
        subtitle = format!("{} · {subtitle}", format::word(Word::Paused, locale));
    }
    let sub_style = TextStyle::new(FontFace::Medium, 13.0, palette.text_dim);
    canvas.text_fit(text_x, 34.0, &subtitle, &sub_style, room);

    if props.show_progress {
        if let (Some(pos), Some(len)) = (media.position_s, media.length_s.filter(|l| *l > 0.0)) {
            let time_style = TextStyle::new(FontFace::SemiBold, 10.0, palette.text_dim).tabular();
            let secs = pos.max(0.0) as u64;
            let time = format!("{}:{:02}", secs / 60, secs % 60);
            let time_w = text::measure(&time, &time_style);
            let track = Bounds::new(text_x, 40.0, (room - time_w - 6.0).max(10.0), 2.0);
            let fraction = (pos / len).clamp(0.0, 1.0);
            canvas.round_rect(track, 1.0, palette.track, 1.0);
            canvas.round_rect(
                Bounds::new(track.x, track.y, track.w * fraction, track.h),
                1.0,
                palette.text,
                0.9,
            );
            canvas.text(track.right() + 6.0, 44.0, &time, &time_style);
        }
    }

    if controls {
        let cx = b.right() - CONTROLS_W / 2.0 - 4.0;
        icons::draw_filled(
            canvas,
            "skip-back",
            cx - 50.0,
            13.0,
            22.0,
            palette.text,
            0.95,
        );
        canvas.circle(cx, 24.0, 18.0, palette.text, 1.0);
        let glyph = if media.playing { "pause" } else { "play" };
        let nudge = if media.playing { 0.0 } else { 1.5 };
        icons::draw_filled(
            canvas,
            glyph,
            cx - 10.0 + nudge,
            14.0,
            20.0,
            palette.bg_bottom,
            1.0,
        );
        icons::draw_filled(
            canvas,
            "skip-forward",
            cx + 28.0,
            13.0,
            22.0,
            palette.text,
            0.95,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_split_the_controls_in_thirds() {
        let props = MediaProps::default();
        assert_eq!(media_zone(0, 480, &props, 100), MediaZone::Info);
        assert_eq!(media_zone(0, 480, &props, 480 - 120), MediaZone::Previous);
        assert_eq!(media_zone(0, 480, &props, 480 - 66), MediaZone::PlayPause);
        assert_eq!(media_zone(0, 480, &props, 479), MediaZone::Next);
        // Narrow widgets have no controls; the whole widget plays/pauses.
        assert_eq!(media_zone(0, 200, &props, 199), MediaZone::Info);
    }
}
