//! "Who is listening?": a red pill at the right edge of every page while
//! an app records from a microphone or uses a camera, naming it. A while
//! later it shrinks to its icons, out of the way of the sliders below,
//! until the apps change. A tap fills the strip with the list of them and
//! a close button.

use super::sound;
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::{Locale, PrivacyEntry, PrivacyInfo};
use crate::text::{self, FontFace, TextStyle};
use crate::theme::Palette;
use crate::WIDTH;

const PILL_TOP: f32 = 9.5;
const PILL_H: f32 = 29.0;
const PILL_PAD: f32 = 10.0;
/// Padding of the pill with icons only: one icon makes a circle.
const COMPACT_PAD: f32 = 8.0;
const ICON: f32 = 13.0;
const ICON_GAP: f32 = 6.0;
const NAME_MAX: f32 = 90.0;
/// Right edge, and left of Discord's microphone badge while it shows.
const RIGHT: f32 = WIDTH as f32 - 12.0;
const RIGHT_OF_BADGE: f32 = WIDTH as f32 - 37.0;
const PAD: f32 = 12.0;
const ENTRY_TOP: f32 = 8.0;
const ENTRY_H: f32 = 32.0;
const ENTRY_GAP: f32 = 7.0;
const TITLE_W: f32 = 95.0;
const CLOSE_W: f32 = 70.0;

/// What a finger touches while the pill or the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyHit {
    /// The pill: open the list.
    Pill,
    /// The list's close button.
    Close,
    /// Elsewhere on the open list.
    List,
}

fn name_style(palette: &Palette) -> TextStyle {
    TextStyle::new(FontFace::SemiBold, 10.5, palette.text)
}

/// The pill's parts: one per kind of use, with the first app's name and
/// how many more there are.
fn parts(info: &PrivacyInfo) -> Vec<(&'static str, String)> {
    let mut parts = Vec::new();
    for (camera, icon) in [(false, "mic"), (true, "video")] {
        let mut apps: Vec<&str> = Vec::new();
        for entry in info.uses.iter().filter(|e| e.camera == camera) {
            if !apps.contains(&entry.app.as_str()) {
                apps.push(&entry.app);
            }
        }
        if let Some(first) = apps.first() {
            let text = match apps.len() {
                1 => first.to_string(),
                n => format!("{first} +{}", n - 1),
            };
            parts.push((icon, text));
        }
    }
    parts
}

/// Where the pill sits, if anything is in use.
pub fn privacy_pill(info: &PrivacyInfo, mic_badge: bool, palette: &Palette) -> Option<Bounds> {
    let parts = parts(info);
    if parts.is_empty() {
        return None;
    }
    let style = name_style(palette);
    let gaps = ICON_GAP * (parts.len() - 1) as f32;
    let w = if info.compact {
        ICON * parts.len() as f32 + gaps + 2.0 * COMPACT_PAD
    } else {
        let content: f32 = parts
            .iter()
            .map(|(_, name)| ICON + ICON_GAP + text::measure(name, &style).min(NAME_MAX))
            .sum();
        content + gaps + 2.0 * PILL_PAD
    };
    let right = if mic_badge { RIGHT_OF_BADGE } else { RIGHT };
    Some(Bounds::new(right - w, PILL_TOP, w, PILL_H))
}

fn close_button() -> Bounds {
    Bounds::new(WIDTH as f32 - PAD - CLOSE_W, ENTRY_TOP, CLOSE_W, ENTRY_H)
}

/// What a finger at `x` touches: the pill as drawn, or while the list is
/// open the list. Elsewhere the page has the touch.
pub fn privacy_hit(
    info: &PrivacyInfo,
    mic_badge: bool,
    palette: &Palette,
    x: u16,
) -> Option<PrivacyHit> {
    let x = f32::from(x);
    if info.open {
        let close = close_button();
        return Some(if x >= close.x - ENTRY_GAP / 2.0 {
            PrivacyHit::Close
        } else {
            PrivacyHit::List
        });
    }
    let pill = privacy_pill(info, mic_badge, palette)?;
    (x >= pill.x && x < pill.right()).then_some(PrivacyHit::Pill)
}

pub(crate) fn draw_pill(
    canvas: &mut Canvas,
    info: &PrivacyInfo,
    mic_badge: bool,
    palette: &Palette,
) {
    let Some(pill) = privacy_pill(info, mic_badge, palette) else {
        return;
    };
    let radius = PILL_H / 2.0;
    canvas.round_rect(pill.inset(-1.0, -1.0), radius + 1.0, palette.bg_bottom, 0.6);
    canvas.round_rect(pill, radius, sound::alarm_fill(palette), 1.0);
    canvas.round_rect_stroke(pill, radius, palette.critical, 1.0, 1.0);
    let style = name_style(palette);
    let mut x = pill.x + if info.compact { COMPACT_PAD } else { PILL_PAD };
    for (n, (icon, name)) in parts(info).iter().enumerate() {
        if n > 0 {
            x += ICON_GAP;
        }
        canvas.icon(icon, x, 24.0 - ICON / 2.0, ICON, palette.critical, 1.0);
        x += ICON;
        if !info.compact {
            x += ICON_GAP;
            let w = canvas.text_fit(x, 28.2, name, &style, NAME_MAX);
            x += w.min(NAME_MAX);
        }
    }
}

/// The list over the whole strip: who uses what, and a close button.
pub(crate) fn draw_list(
    canvas: &mut Canvas,
    info: &PrivacyInfo,
    palette: &Palette,
    locale: Locale,
) {
    canvas.vertical_gradient(Bounds::strip(), palette.bg_top, palette.bg_bottom);
    let title = sound::label_style(palette);
    let title_text = text::ellipsize(format::word(Word::InUseNow, locale), &title, TITLE_W);
    canvas.text(PAD, 27.0, &title_text, &title);

    let close = close_button();
    canvas.round_rect(close, 6.0, palette.surface, 1.0);
    let close_style = TextStyle::new(FontFace::SemiBold, 10.0, palette.text);
    canvas.text_centered(
        close.center_x(),
        27.5,
        format::word(Word::Close, locale),
        &close_style,
    );

    let name = name_style(palette);
    let detail = TextStyle::new(FontFace::Medium, 8.0, sound::soft(palette));
    let mut x = PAD + TITLE_W + 20.0;
    let end = close.x - 20.0;
    let entries: Vec<&PrivacyEntry> = info.uses.iter().collect();
    for (n, entry) in entries.iter().enumerate() {
        let what = format::word(
            if entry.camera {
                Word::Camera
            } else {
                Word::Microphone
            },
            locale,
        );
        let line = format!("{what}, {}", entry.device);
        let text_w = text::measure(&entry.app, &name)
            .max(text::measure(&line, &detail))
            .min(150.0);
        let w = 8.0 + ICON + ICON_GAP + text_w + 8.0;
        if x + w > end {
            let more = format!("+{}", entries.len() - n);
            canvas.text(x, 27.5, &more, &name);
            break;
        }
        let entry_box = Bounds::new(x, ENTRY_TOP, w, ENTRY_H);
        canvas.round_rect(entry_box, 6.0, palette.surface, 1.0);
        let icon = if entry.camera { "video" } else { "mic" };
        canvas.icon(
            icon,
            x + 8.0,
            24.0 - ICON / 2.0,
            ICON,
            palette.critical,
            1.0,
        );
        let text_x = x + 8.0 + ICON + ICON_GAP;
        canvas.text_fit(text_x, 23.8, &entry.app, &name, text_w);
        canvas.text_fit(text_x, 33.0, &line, &detail, text_w);
        x += w + ENTRY_GAP;
    }
}
