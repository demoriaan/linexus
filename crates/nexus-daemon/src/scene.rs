//! Frame composition: the current page plus whatever is in motion — a page
//! following the finger, a page change sliding in, press feedback, the page
//! indicator and transient overlays.

use crate::runtime::{Direction, PageChange, Runtime};
use nexus_config::{Page, PageTransition, Theme};
use nexus_render::{Canvas, LiveData, MediaInfo, Overlay, UiState};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const TRANSITION: Duration = Duration::from_millis(260);
const SNAP_BACK: Duration = Duration::from_millis(200);
const PRESS_IN: Duration = Duration::from_millis(70);
const PRESS_OUT: Duration = Duration::from_millis(220);
const INDICATOR: Duration = Duration::from_millis(1_400);
const INDICATOR_FADE: Duration = Duration::from_millis(400);
const OVERLAY_SLIDE: Duration = Duration::from_millis(180);

const WIDTH: f32 = nexus_render::WIDTH as f32;

struct Transition {
    /// Profile and page id of the page moving out, looked up per frame.
    from: (String, String),
    started: Instant,
    direction: Direction,
    /// Pixels already travelled when the change started (released drag).
    start_offset: f32,
    style: PageTransition,
}

struct Press {
    widget: String,
    started: Instant,
    released: Option<(Instant, f32)>,
}

struct ShownOverlay {
    overlay: Overlay,
    shown: Instant,
    duration: Duration,
}

#[derive(Default)]
pub(crate) struct Scene {
    transition: Option<Transition>,
    /// Finger-follow offset: positive reveals the next page.
    drag: Option<i32>,
    snap_back: Option<(i32, Instant)>,
    press: Option<Press>,
    slide: Option<(String, f32)>,
    indicator_since: Option<Instant>,
    overlay: Option<ShownOverlay>,
    queue: VecDeque<(Overlay, Duration)>,
}

impl Scene {
    pub(crate) fn page_changed(&mut self, change: PageChange, style: PageTransition, now: Instant) {
        let start_offset = self
            .drag
            .take()
            .map(|dx| dx.unsigned_abs() as f32)
            .unwrap_or(0.0);
        self.snap_back = None;
        self.press = None;
        self.indicator_since = Some(now);
        self.transition = (style != PageTransition::None).then_some(Transition {
            from: change.from,
            started: now,
            direction: change.direction,
            start_offset,
            style,
        });
    }

    /// Stops everything in motion that refers to pages or widgets: a page
    /// change animation, a page drag, press feedback, a captured slider.
    pub(crate) fn reset(&mut self) {
        self.transition = None;
        self.drag = None;
        self.snap_back = None;
        self.press = None;
        self.slide = None;
    }

    pub(crate) fn drag(&mut self, dx: i32, now: Instant) {
        self.drag = Some(dx);
        self.transition = None;
        self.snap_back = None;
        self.indicator_since = Some(now);
    }

    /// Release without a page change: the page springs back.
    pub(crate) fn snap_back(&mut self, now: Instant) {
        if let Some(dx) = self.drag.take() {
            self.snap_back = Some((dx, now));
        }
    }

    pub(crate) fn press(&mut self, widget: Option<String>, now: Instant) {
        self.press = widget.map(|widget| Press {
            widget,
            started: now,
            released: None,
        });
    }

    pub(crate) fn release(&mut self, now: Instant) {
        if let Some(press) = &mut self.press {
            if press.released.is_none() {
                let strength = press_strength(press.started, None, now);
                press.released = Some((now, strength));
            }
        }
    }

    pub(crate) fn slide(&mut self, widget: Option<(String, f32)>) {
        self.slide = widget;
    }

    pub(crate) fn show_overlay(&mut self, overlay: Overlay, duration: Duration, now: Instant) {
        // A newer volume or track overlay replaces one of the same kind in
        // place instead of queueing behind it.
        if let Some(current) = &mut self.overlay {
            if std::mem::discriminant(&current.overlay) == std::mem::discriminant(&overlay)
                && !matches!(overlay, Overlay::Notification { .. })
            {
                current.overlay = overlay;
                current.duration =
                    duration.max(now.saturating_duration_since(current.shown) + duration / 2);
                return;
            }
            self.queue.push_back((overlay, duration));
            self.queue.truncate(4);
            return;
        }
        self.overlay = Some(ShownOverlay {
            overlay,
            shown: now,
            duration,
        });
    }

    /// Updates a shown track overlay with newer data of the same track: its
    /// cover, which is fetched after the track change, and the position.
    pub(crate) fn refresh_now_playing(&mut self, media: &MediaInfo) {
        if let Some(ShownOverlay {
            overlay: Overlay::NowPlaying(shown),
            ..
        }) = &mut self.overlay
        {
            if shown.title == media.title && shown.artist == media.artist {
                **shown = media.clone();
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref().map(|shown| &shown.overlay)
    }

    /// Hides the overlay on touch. Returns whether the touch is used up:
    /// notifications and media overlays swallow it, while feedback toasts
    /// (answers to the user's own taps) let it through, so quick repeated
    /// taps all count.
    pub(crate) fn dismiss_overlay(&mut self, now: Instant) -> bool {
        match &mut self.overlay {
            Some(shown) if now < shown.shown + shown.duration => {
                shown.duration = now.saturating_duration_since(shown.shown);
                !matches!(shown.overlay, Overlay::Toast { .. })
            }
            _ => false,
        }
    }

    /// Whether frames change on their own right now (for frame pacing).
    pub(crate) fn animating(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.drag.is_some()
            || self.slide.is_some()
            || self.transition.is_some()
            || self.snap_back.is_some()
            || self.press.is_some()
            || self.overlay.is_some()
            || self
                .indicator_since
                .is_some_and(|since| now < since + INDICATOR)
    }

    fn expire(&mut self, now: Instant) {
        if self
            .transition
            .as_ref()
            .is_some_and(|t| now >= t.started + TRANSITION)
        {
            self.transition = None;
        }
        if self.snap_back.is_some_and(|(_, at)| now >= at + SNAP_BACK) {
            self.snap_back = None;
        }
        if self
            .press
            .as_ref()
            .and_then(|p| p.released)
            .is_some_and(|(at, _)| now >= at + PRESS_OUT)
        {
            self.press = None;
        }
        if self
            .overlay
            .as_ref()
            .is_some_and(|shown| now >= shown.shown + shown.duration + OVERLAY_SLIDE)
        {
            self.overlay = self
                .queue
                .pop_front()
                .map(|(overlay, duration)| ShownOverlay {
                    overlay,
                    shown: now,
                    duration,
                });
        }
    }

    pub(crate) fn compose(&mut self, runtime: &Runtime, live: &LiveData, now: Instant) -> Canvas {
        self.expire(now);
        let theme = &runtime.profile().theme;
        let (position, count) = runtime.visible_position();
        let ui = UiState {
            pressed: self
                .press
                .as_ref()
                .map(|p| (p.widget.clone(), press_strength(p.started, p.released, now))),
            dragging: self.slide.clone(),
            page_index: position,
            page_count: count,
            indicator_alpha: self.indicator_alpha(now),
            clock_ms: 0,
        };
        let pages = &runtime.profile().pages;
        let render = |index: usize, ui: &UiState| {
            let page = pages.get(index).unwrap_or_else(|| runtime.page());
            nexus_render::render_page(page, theme, live, ui)
        };
        // The page moving out may be gone after a reload.
        let from = self
            .transition
            .as_ref()
            .and_then(|transition| find_page(runtime, &transition.from));
        if from.is_none() {
            self.transition = None;
        }

        let mut canvas = if let Some(dx) = self.drag.or_else(|| self.snap_back_offset(now)) {
            let direction = if dx >= 0 {
                Direction::Forward
            } else {
                Direction::Backward
            };
            let neighbour = runtime.neighbour(direction);
            let current = render(runtime.page_index, &ui);
            if neighbour == runtime.page_index {
                current
            } else {
                // Rubber band: the page follows the finger, then resists.
                let travel = rubber_band(dx as f32);
                let other = render(neighbour, &UiState::default());
                nexus_render::slide(&current, &other, travel)
            }
        } else if let (Some(transition), Some((from_page, from_theme))) = (&self.transition, from) {
            let t = now
                .saturating_duration_since(transition.started)
                .as_secs_f32()
                / TRANSITION.as_secs_f32();
            let eased = nexus_render::ease_out_cubic(t);
            let from = nexus_render::render_page(from_page, from_theme, live, &UiState::default());
            let to = render(runtime.page_index, &ui);
            match transition.style {
                PageTransition::Fade => nexus_render::crossfade(&from, &to, eased),
                _ => {
                    let offset =
                        transition.start_offset + (WIDTH - transition.start_offset) * eased;
                    let signed = match transition.direction {
                        Direction::Forward => offset,
                        Direction::Backward => -offset,
                    };
                    nexus_render::slide(&from, &to, signed)
                }
            }
        } else {
            render(runtime.page_index, &ui)
        };

        if let Some(shown) = &self.overlay {
            let elapsed = now.saturating_duration_since(shown.shown);
            let visibility = if elapsed < OVERLAY_SLIDE {
                elapsed.as_secs_f32() / OVERLAY_SLIDE.as_secs_f32()
            } else if elapsed < shown.duration {
                1.0
            } else {
                1.0 - (elapsed - shown.duration).as_secs_f32() / OVERLAY_SLIDE.as_secs_f32()
            };
            nexus_render::draw_overlay(&mut canvas, &shown.overlay, theme, live, visibility);
        }
        canvas
    }

    fn snap_back_offset(&self, now: Instant) -> Option<i32> {
        let (dx, at) = self.snap_back?;
        let t = now.saturating_duration_since(at).as_secs_f32() / SNAP_BACK.as_secs_f32();
        Some((dx as f32 * (1.0 - nexus_render::ease_out_cubic(t))).round() as i32)
    }

    fn indicator_alpha(&self, now: Instant) -> f32 {
        let Some(since) = self.indicator_since else {
            return 0.0;
        };
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= INDICATOR {
            0.0
        } else if elapsed > INDICATOR - INDICATOR_FADE {
            (INDICATOR - elapsed).as_secs_f32() / INDICATOR_FADE.as_secs_f32()
        } else {
            1.0
        }
    }
}

/// A page by profile and page id, with the theme of its profile.
fn find_page<'a>(
    runtime: &'a Runtime,
    (profile, page): &(String, String),
) -> Option<(&'a Page, &'a Theme)> {
    let profile = runtime.config.profiles.iter().find(|p| p.id == *profile)?;
    let page = profile.pages.iter().find(|p| p.id == *page)?;
    Some((page, &profile.theme))
}

fn press_strength(started: Instant, released: Option<(Instant, f32)>, now: Instant) -> f32 {
    match released {
        None => {
            (now.saturating_duration_since(started).as_secs_f32() / PRESS_IN.as_secs_f32()).min(1.0)
        }
        Some((at, from)) => {
            from * (1.0 - now.saturating_duration_since(at).as_secs_f32() / PRESS_OUT.as_secs_f32())
                .max(0.0)
        }
    }
}

/// Finger travel mapped to page offset: 1:1 at first, then damped so a long
/// drag never shows a torn edge.
fn rubber_band(dx: f32) -> f32 {
    let limit = WIDTH * 0.9;
    let sign = dx.signum();
    let d = dx.abs();
    let shown = if d < 200.0 {
        d
    } else {
        200.0 + (d - 200.0) * 0.6
    };
    sign * shown.min(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_feedback_ramps_and_fades() {
        let t = Instant::now();
        assert!(press_strength(t, None, t) < 0.01);
        assert_eq!(press_strength(t, None, t + PRESS_IN), 1.0);
        let released = Some((t + PRESS_IN, 1.0));
        assert!((press_strength(t, released, t + PRESS_IN + PRESS_OUT / 2) - 0.5).abs() < 0.01);
        assert_eq!(press_strength(t, released, t + PRESS_IN + PRESS_OUT), 0.0);
    }

    #[test]
    fn a_track_overlay_takes_the_cover_that_arrives_later() {
        let track = |title: &str, art: bool| MediaInfo {
            title: title.into(),
            artist: "Artist".into(),
            art: art
                .then(|| std::sync::Arc::new(nexus_render::tiny_skia::Pixmap::new(1, 1).unwrap())),
            ..MediaInfo::default()
        };
        let mut scene = Scene::default();
        let t = Instant::now();
        scene.show_overlay(
            Overlay::NowPlaying(Box::new(track("Second", false))),
            Duration::from_secs(4),
            t,
        );
        // Data of another track does not replace it.
        scene.refresh_now_playing(&track("Third", true));
        assert!(matches!(scene.overlay(), Some(Overlay::NowPlaying(m)) if m.art.is_none()));
        scene.refresh_now_playing(&track("Second", true));
        assert!(matches!(scene.overlay(), Some(Overlay::NowPlaying(m)) if m.art.is_some()));
    }

    #[test]
    fn rubber_band_is_linear_then_damped() {
        assert_eq!(rubber_band(120.0), 120.0);
        assert_eq!(rubber_band(-120.0), -120.0);
        assert!(rubber_band(600.0) < 600.0);
        assert!(rubber_band(5000.0) <= WIDTH * 0.9);
    }

    #[test]
    fn a_transition_from_a_removed_page_falls_back_to_the_current_one() {
        let mut runtime = Runtime::from_config(nexus_config::default_config(), None);
        let mut scene = Scene::default();
        let t = Instant::now();
        let change = runtime.step(Direction::Forward).unwrap();
        scene.page_changed(change, PageTransition::Slide, t);
        // The page sliding away disappears without the scene being told.
        let from = runtime.config.profiles[0]
            .pages
            .iter()
            .position(|p| p.id == "overview")
            .unwrap();
        runtime.config.profiles[0].pages.remove(from);
        runtime.page_index = 0;
        let live = LiveData::default();
        scene.compose(&runtime, &live, t + Duration::from_millis(100));
        assert!(scene.transition.is_none());
    }

    #[test]
    fn overlays_of_the_same_kind_replace_each_other() {
        let mut scene = Scene::default();
        let t = Instant::now();
        let volume = |percent| {
            Overlay::Volume(nexus_render::VolumeInfo {
                percent,
                muted: false,
            })
        };
        scene.show_overlay(volume(10), Duration::from_millis(1500), t);
        scene.show_overlay(
            volume(20),
            Duration::from_millis(1500),
            t + Duration::from_millis(100),
        );
        assert!(scene.queue.is_empty());
        assert_eq!(scene.overlay.as_ref().unwrap().overlay, volume(20));
        let toast = Overlay::Toast {
            icon: "bell".into(),
            text: "hi".into(),
        };
        scene.show_overlay(toast.clone(), Duration::from_secs(1), t);
        assert_eq!(scene.queue.len(), 1);
        // After the volume overlay ends, the queued toast takes its place.
        assert!(scene.animating(t + Duration::from_secs(5)));
        assert_eq!(scene.overlay.as_ref().unwrap().overlay, toast);
    }
}
