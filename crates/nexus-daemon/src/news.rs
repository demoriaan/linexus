//! Headline rotation for the news widget: which two cards are on screen,
//! for how long, and what a touch on them refers to.

use nexus_config::NewsFont;
use nexus_render::{MarqueeStyle, NewsInfo, NewsRow};
use nexus_sensors::SensorCard;
use std::time::{Duration, Instant};

/// Default floor dwell for a card pair so very short pairs don't auto-rotate
/// too fast to be read. Marquee-driven dwells already include their own scroll
/// time, so this only kicks in for static (short) headlines.
const MIN_PAIR_DWELL: Duration = Duration::from_millis(5_000);
/// Fastest frame interval while a headline scrolls (~33 fps).
const MIN_SCROLL_FRAME: Duration = Duration::from_millis(30);

/// Headline pacing from the sensor config, as the rotation and renderer use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pacing {
    marquee: MarqueeStyle,
    min_pair: Duration,
    /// Press duration that counts as "hold"; a touch freezes the rotation at
    /// least this long.
    pub(crate) hold: Duration,
    /// Headline font and viewport width of the news widget, which decide
    /// whether a headline scrolls and how long its marquee takes.
    font: NewsFont,
    viewport: u32,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            marquee: MarqueeStyle::default(),
            min_pair: MIN_PAIR_DWELL,
            hold: Duration::from_millis(600),
            font: NewsFont::Pixel,
            viewport: nexus_render::news::viewport_estimate(640),
        }
    }
}

impl Pacing {
    pub(crate) fn new(
        display: &nexus_sensors::NewsDisplayConfig,
        font: NewsFont,
        widget_w: u16,
    ) -> Self {
        Self {
            marquee: MarqueeStyle {
                speed_px_per_sec: display.scroll_speed_px_per_sec.max(1),
                start_pause_ms: u64::from(display.start_pause_ms),
                end_pause_ms: u64::from(display.end_pause_ms),
            },
            min_pair: Duration::from_millis(u64::from(display.min_pair_ms)),
            hold: Duration::from_millis(u64::from(display.hold_to_dismiss_ms)),
            font,
            viewport: nexus_render::news::viewport_estimate(widget_w),
        }
    }

    fn dwell(&self, title: &str) -> Duration {
        Duration::from_millis(nexus_render::news::headline_dwell_ms(
            title,
            self.font,
            self.viewport,
            &self.marquee,
        ))
    }

    fn scrolls(&self, title: &str) -> bool {
        nexus_render::news::headline_scrolls(title, self.font, self.viewport)
    }

    /// Frame interval that moves the marquee by a whole number of pixels per
    /// frame (even motion) at no more than ~33 fps: 50 px/s → 2 px every
    /// 40 ms, 25 px/s → 1 px every 40 ms, 100 px/s → 3 px every 30 ms.
    pub(crate) fn scroll_frame_interval(&self) -> Duration {
        let speed = u64::from(self.marquee.speed_px_per_sec.max(1));
        let min_ms = MIN_SCROLL_FRAME.as_millis() as u64;
        let step_px = (speed * min_ms).div_ceil(1_000).max(1);
        Duration::from_millis(step_px * 1_000 / speed)
    }
}

/// After a tap the pair stays at least this long, so the opened headline can
/// be seen dimming in place instead of rotating away under the finger.
const TAP_HOLD: Duration = Duration::from_millis(4_000);
/// How long the tapped row stays outlined.
const TAP_HIGHLIGHT: Duration = Duration::from_millis(700);
/// A touch freezes the rotation at least this long, covering the press plus
/// the double-tap window, so the pair cannot change before the tap resolves;
/// longer when the hold threshold plus that margin needs more.
const TOUCH_FREEZE: Duration = Duration::from_millis(1_500);
const TOUCH_FREEZE_AFTER_HOLD: Duration = Duration::from_millis(900);

/// Which two sensor cards are on screen and since when.
///
/// The visible pair is state, not a function of the wall clock: it keeps its
/// cards (looked up by id) until its dwell — the longer marquee of the two —
/// has run out, then continues with the card that followed it. Feed
/// refreshes, read headlines sorting to the back, or a dismissed card
/// therefore never swap a headline out mid-scroll; only a vanished card forces
/// an early re-pick, anchored on the card that is still there.
#[derive(Debug, Clone, Default)]
struct SensorRotation {
    /// Pacing from the latest config; applied when the next pair is picked.
    pacing: Pacing,
    /// Marquee pacing the visible pair was picked (and its dwell computed)
    /// with, so a config change never makes a scrolling headline jump.
    pair_marquee: MarqueeStyle,
    ids: Vec<String>,
    /// Card that followed the pair when it was picked. Rotation continues
    /// there even if the pair's own cards were read and sorted to the back.
    next_id: Option<String>,
    shown_at: Option<Instant>,
    dwell: Duration,
    hold_until: Option<Instant>,
}

impl SensorRotation {
    fn visible(&mut self, cards: &[SensorCard], now: Instant) -> (Vec<SensorCard>, u64) {
        if cards.is_empty() {
            *self = Self {
                pacing: self.pacing,
                ..Self::default()
            };
            return (Vec::new(), 0);
        }
        let positions: Vec<Option<usize>> = self
            .ids
            .iter()
            .map(|id| cards.iter().position(|card| &card.id == id))
            .collect();
        let complete = !self.ids.is_empty()
            && self.ids.len() == cards.len().min(2)
            && positions.iter().all(Option::is_some);
        if !complete {
            let anchor = positions.iter().flatten().next().copied().unwrap_or(0);
            self.show_from(cards, anchor, now);
        } else if self.expired(now) {
            let next = self
                .next_id
                .as_ref()
                .and_then(|id| cards.iter().position(|card| &card.id == id))
                .unwrap_or_else(|| positions.last().copied().flatten().unwrap_or(0) + 1);
            self.show_from(cards, next % cards.len(), now);
        }
        let visible = self
            .ids
            .iter()
            .filter_map(|id| cards.iter().find(|card| &card.id == id).cloned())
            .collect();
        let phase = self
            .shown_at
            .map(|shown_at| now.saturating_duration_since(shown_at).as_millis() as u64)
            .unwrap_or_default();
        (visible, phase)
    }

    fn expired(&self, now: Instant) -> bool {
        let Some(shown_at) = self.shown_at else {
            return true;
        };
        now >= shown_at + self.dwell && self.hold_until.is_none_or(|until| now >= until)
    }

    /// Shows the pair starting at `start` (wrapping) and restarts its marquee.
    fn show_from(&mut self, cards: &[SensorCard], start: usize, now: Instant) {
        let picked: Vec<&SensorCard> = if cards.len() >= 2 {
            vec![
                &cards[start % cards.len()],
                &cards[(start + 1) % cards.len()],
            ]
        } else {
            vec![&cards[0]]
        };
        let next_id = (cards.len() > 2).then(|| cards[(start + 2) % cards.len()].id.clone());
        self.show(&picked, next_id, now);
    }

    fn show(&mut self, picked: &[&SensorCard], next_id: Option<String>, now: Instant) {
        self.dwell = picked
            .iter()
            .map(|card| self.pacing.dwell(&card.title))
            .max()
            .unwrap_or_default()
            .max(self.pacing.min_pair);
        self.ids = picked.iter().map(|card| card.id.clone()).collect();
        self.pair_marquee = self.pacing.marquee;
        self.next_id = next_id;
        self.shown_at = Some(now);
        self.hold_until = None;
    }

    /// Keeps the current pair on screen at least until `until`.
    fn hold(&mut self, until: Instant) {
        self.hold_until = Some(self.hold_until.map_or(until, |held| held.max(until)));
    }

    /// Jumps `pairs` pairs forward (or back) from the current one.
    fn scrub(&mut self, cards: &[SensorCard], pairs: i64, now: Instant) {
        if cards.is_empty() {
            return;
        }
        let current = self
            .ids
            .first()
            .and_then(|id| cards.iter().position(|card| &card.id == id))
            .unwrap_or(0) as i64;
        let start = (current + 2 * pairs).rem_euclid(cards.len() as i64) as usize;
        self.show_from(cards, start, now);
    }
}

/// Sensors-page state the daemon keeps between frames and touches.
#[derive(Debug, Clone, Default)]
pub(crate) struct SensorState {
    rotation: SensorRotation,
    /// Cards on screen in the last rendered frame.
    shown: Vec<SensorCard>,
    /// Cards that were on screen when the current touch began. A tap resolves
    /// against these rather than against whatever is visible once the
    /// double-tap window has passed and the pair may have rotated.
    touched: Option<Vec<SensorCard>>,
    highlight: Option<(usize, Instant)>,
}

impl SensorState {
    /// Advances the rotation and returns the cards to draw plus the marquee
    /// phase (ms since the pair came on screen).
    pub(crate) fn frame_cards(
        &mut self,
        cards: &[SensorCard],
        now: Instant,
    ) -> (Vec<SensorCard>, u64) {
        let (visible, phase) = self.rotation.visible(cards, now);
        self.shown = visible.clone();
        (visible, phase)
    }

    /// Applies the current sensor config; takes effect from the next pair.
    pub(crate) fn set_pacing(&mut self, pacing: Pacing) {
        self.rotation.pacing = pacing;
    }

    pub(crate) fn scroll_frame_interval(&self) -> Duration {
        Pacing {
            marquee: self.rotation.pair_marquee,
            ..self.rotation.pacing
        }
        .scroll_frame_interval()
    }

    /// Marquee pacing of the pair on screen.
    pub(crate) fn pair_marquee(&self) -> MarqueeStyle {
        self.rotation.pair_marquee
    }

    pub(crate) fn touch_began(&mut self, now: Instant) {
        self.touched = Some(self.shown.clone());
        let freeze = TOUCH_FREEZE.max(self.rotation.pacing.hold + TOUCH_FREEZE_AFTER_HOLD);
        self.rotation.hold(now + freeze);
    }

    /// Forgets the touch-down snapshot once its gesture has been handled, so
    /// a later tap without a touch-down (daemon API) cannot reuse it.
    pub(crate) fn end_touch(&mut self) {
        self.touched = None;
    }

    /// The cards a tap or hold at the end of the current touch refers to.
    pub(crate) fn touch_targets(&mut self, cards: &[SensorCard], now: Instant) -> Vec<SensorCard> {
        match self.touched.take() {
            Some(touched) if !touched.is_empty() => touched,
            _ if !self.shown.is_empty() => self.shown.clone(),
            _ => self.frame_cards(cards, now).0,
        }
    }

    /// Tap feedback: outline `slot` of the tapped pair briefly and keep that
    /// pair on screen — bringing it back if it rotated away meanwhile.
    pub(crate) fn acknowledge_tap(&mut self, targets: &[SensorCard], slot: usize, now: Instant) {
        let on_screen = targets.len() == self.rotation.ids.len()
            && targets
                .iter()
                .zip(&self.rotation.ids)
                .all(|(card, id)| &card.id == id);
        if !on_screen {
            let picked: Vec<&SensorCard> = targets.iter().collect();
            self.rotation.show(&picked, None, now);
        }
        self.highlight = Some((slot, now));
        self.rotation.hold(now + TAP_HOLD);
    }

    pub(crate) fn scrub(&mut self, cards: &[SensorCard], pairs: i64, now: Instant) {
        self.rotation.scrub(cards, pairs, now);
    }

    fn highlight_slot(&self, now: Instant) -> Option<usize> {
        self.highlight
            .filter(|(_, at)| now.saturating_duration_since(*at) < TAP_HIGHLIGHT)
            .map(|(slot, _)| slot)
    }

    /// Whether the sensors page needs smooth frames right now: a visible
    /// headline is long enough to scroll, or tap feedback is showing.
    pub(crate) fn animating(&self, now: Instant) -> bool {
        self.highlight_slot(now).is_some()
            || self
                .shown
                .iter()
                .any(|card| self.rotation.pacing.scrolls(&card.title))
    }

    pub(crate) fn hold_threshold(&self) -> Duration {
        self.rotation.pacing.hold
    }

    /// Advances the rotation and describes the visible pair for the renderer.
    pub(crate) fn frame(&mut self, cards: &[SensorCard], now: Instant) -> NewsInfo {
        let (visible, phase) = self.frame_cards(cards, now);
        NewsInfo {
            rows: visible
                .iter()
                .map(|card| NewsRow {
                    source: card.source.clone(),
                    headline: card.title.clone(),
                    seen: card.status.as_deref() == Some("SEEN"),
                })
                .collect(),
            marquee_phase_ms: phase,
            marquee: self.pair_marquee(),
            highlight: self.highlight_slot(now),
        }
    }
}

/// Row a touch at `x` refers to: the left half of the widget is the top
/// row, the right half the bottom row (the strip reports X only).
pub(crate) fn slot_for_x(
    cards: &[SensorCard],
    widget_x: u16,
    widget_w: u16,
    x: u16,
) -> Option<usize> {
    match cards.len() {
        0 => None,
        1 => Some(0),
        _ => Some(usize::from(x >= widget_x + widget_w / 2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, title: &str) -> SensorCard {
        SensorCard {
            id: id.into(),
            module: "rss".into(),
            kind: "rss_item".into(),
            source: "feed".into(),
            title: title.into(),
            url: Some(format!("https://example.test/{id}")),
            status: Some("NEW".into()),
        }
    }

    fn ids(cards: &[SensorCard]) -> Vec<&str> {
        cards.iter().map(|card| card.id.as_str()).collect()
    }

    #[test]
    fn pair_survives_reordering_and_rotates_after_its_dwell() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards = vec![
            card("a", "A"),
            card("b", "B"),
            card("c", "C"),
            card("d", "D"),
        ];
        let (visible, phase) = state.frame_cards(&cards, t0);
        assert_eq!(ids(&visible), ["a", "b"]);
        assert_eq!(phase, 0);

        // "a" was read and sorted to the back: the pair stays, phase runs on.
        let reordered = vec![
            card("b", "B"),
            card("c", "C"),
            card("d", "D"),
            card("a", "A"),
        ];
        let (visible, phase) = state.frame_cards(&reordered, t0 + Duration::from_secs(1));
        assert_eq!(ids(&visible), ["a", "b"]);
        assert_eq!(phase, 1_000);

        // Once the dwell is over the next pair follows the last card shown.
        let (visible, phase) = state.frame_cards(&reordered, t0 + MIN_PAIR_DWELL);
        assert_eq!(ids(&visible), ["c", "d"]);
        assert_eq!(phase, 0);
    }

    #[test]
    fn dismissed_card_is_replaced_keeping_its_partner() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards = vec![card("a", "A"), card("b", "B"), card("c", "C")];
        state.frame_cards(&cards, t0);
        let without_a = vec![card("b", "B"), card("c", "C")];
        let (visible, _) = state.frame_cards(&without_a, t0 + Duration::from_millis(10));
        assert_eq!(ids(&visible), ["b", "c"]);
    }

    #[test]
    fn touch_targets_are_the_cards_visible_at_touch_down() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards: Vec<_> = ["a", "b", "c", "d"].map(|id| card(id, id)).to_vec();
        state.frame_cards(&cards, t0);
        let down = t0 + MIN_PAIR_DWELL - Duration::from_millis(100);
        state.touch_began(down);
        // The touch freezes the rotation past the pair's dwell...
        state.frame_cards(&cards, t0 + MIN_PAIR_DWELL);
        assert_eq!(ids(&state.shown), ["a", "b"]);
        // ...and even once it rotates, the tap targets the touch-down pair.
        state.frame_cards(&cards, down + TOUCH_FREEZE);
        assert_eq!(ids(&state.shown), ["c", "d"]);
        let targets = state.touch_targets(&cards, down + TOUCH_FREEZE);
        assert_eq!(ids(&targets), ["a", "b"]);
        // Acknowledging brings the tapped pair back and holds it.
        state.acknowledge_tap(&targets, 1, down + TOUCH_FREEZE);
        let after = down + TOUCH_FREEZE + Duration::from_millis(10);
        assert_eq!(ids(&state.frame_cards(&cards, after).0), ["a", "b"]);
        // Without a fresh touch-down the last rendered pair is the target.
        assert_eq!(ids(&state.touch_targets(&cards, after)), ["a", "b"]);
    }

    #[test]
    fn finished_gestures_drop_the_touch_snapshot() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards: Vec<_> = ["a", "b", "c", "d"].map(|id| card(id, id)).to_vec();
        state.frame_cards(&cards, t0);
        state.touch_began(t0);
        state.end_touch();
        state.scrub(&cards, 1, t0);
        state.frame_cards(&cards, t0);
        assert_eq!(ids(&state.touch_targets(&cards, t0)), ["c", "d"]);
    }

    #[test]
    fn reading_the_bottom_card_does_not_repeat_the_top_one() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards: Vec<_> = ["a", "b", "c", "d", "e"].map(|id| card(id, id)).to_vec();
        state.frame_cards(&cards, t0);
        // "b" was opened and sorted behind the unread cards.
        let reordered: Vec<_> = ["a", "c", "d", "e", "b"].map(|id| card(id, id)).to_vec();
        let (visible, _) = state.frame_cards(&reordered, t0 + MIN_PAIR_DWELL);
        assert_eq!(ids(&visible), ["c", "d"]);
    }

    #[test]
    fn tap_holds_the_pair_and_highlights_the_row() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards = vec![
            card("a", "A"),
            card("b", "B"),
            card("c", "C"),
            card("d", "D"),
        ];
        state.frame_cards(&cards, t0);
        let tap_at = t0 + MIN_PAIR_DWELL - Duration::from_millis(100);
        state.acknowledge_tap(&cards[..2], 1, tap_at);
        assert_eq!(state.highlight_slot(tap_at), Some(1));
        assert!(state.animating(tap_at));
        assert_eq!(state.highlight_slot(tap_at + TAP_HIGHLIGHT), None);
        let (visible, _) = state.frame_cards(&cards, t0 + MIN_PAIR_DWELL);
        assert_eq!(ids(&visible), ["a", "b"]);
        let (visible, _) = state.frame_cards(&cards, tap_at + TAP_HOLD);
        assert_eq!(ids(&visible), ["c", "d"]);
    }

    #[test]
    fn configured_pacing_sets_dwell_and_even_frame_steps() {
        let display = nexus_sensors::NewsDisplayConfig {
            min_pair_ms: 9_000,
            ..Default::default()
        };
        let mut state = SensorState::default();
        state.set_pacing(Pacing::new(&display, NewsFont::Pixel, 640));
        let t0 = Instant::now();
        let cards: Vec<_> = ["a", "b", "c", "d"].map(|id| card(id, id)).to_vec();
        state.frame_cards(&cards, t0);
        assert_eq!(
            ids(&state.frame_cards(&cards, t0 + MIN_PAIR_DWELL).0),
            ["a", "b"]
        );
        assert_eq!(
            ids(&state
                .frame_cards(&cards, t0 + Duration::from_millis(9_000))
                .0),
            ["c", "d"]
        );

        let interval = |speed| {
            Pacing::new(
                &nexus_sensors::NewsDisplayConfig {
                    scroll_speed_px_per_sec: speed,
                    ..Default::default()
                },
                NewsFont::Pixel,
                640,
            )
            .scroll_frame_interval()
        };
        assert_eq!(interval(50), Duration::from_millis(40));
        assert_eq!(interval(25), Duration::from_millis(40));
        assert_eq!(interval(100), Duration::from_millis(30));
        assert_eq!(interval(20), Duration::from_millis(50));
    }

    #[test]
    fn speed_changes_wait_for_the_next_pair_and_long_holds_freeze_longer() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards: Vec<_> = ["a", "b", "c", "d"].map(|id| card(id, id)).to_vec();
        state.frame_cards(&cards, t0);
        let fast = nexus_sensors::NewsDisplayConfig {
            scroll_speed_px_per_sec: 100,
            hold_to_dismiss_ms: 2_000,
            ..Default::default()
        };
        state.set_pacing(Pacing::new(&fast, NewsFont::Pixel, 640));
        state.frame_cards(&cards, t0 + Duration::from_millis(10));
        assert_eq!(state.pair_marquee().speed_px_per_sec, 50);
        state.frame_cards(&cards, t0 + MIN_PAIR_DWELL);
        assert_eq!(state.pair_marquee().speed_px_per_sec, 100);

        // Hold threshold 2 s: a touch keeps the pair for 2.9 s, not just 1.5 s.
        let down = t0 + MIN_PAIR_DWELL;
        state.touch_began(down);
        assert!(state.rotation.hold_until.unwrap() >= down + Duration::from_millis(2_900));
    }

    #[test]
    fn scrub_moves_whole_pairs_and_wraps() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards = vec![
            card("a", "A"),
            card("b", "B"),
            card("c", "C"),
            card("d", "D"),
            card("e", "E"),
        ];
        state.frame_cards(&cards, t0);
        state.scrub(&cards, 1, t0);
        assert_eq!(ids(&state.frame_cards(&cards, t0).0), ["c", "d"]);
        state.scrub(&cards, -2, t0);
        assert_eq!(ids(&state.frame_cards(&cards, t0).0), ["d", "e"]);
    }

    #[test]
    fn taps_map_to_rows_by_widget_half() {
        let cards = vec![card("a", "A"), card("b", "B")];
        assert_eq!(slot_for_x(&cards, 100, 400, 299), Some(0));
        assert_eq!(slot_for_x(&cards, 100, 400, 300), Some(1));
        assert_eq!(slot_for_x(&cards[..1], 100, 400, 450), Some(0));
        assert_eq!(slot_for_x(&[], 0, 640, 10), None);
    }

    #[test]
    fn short_lists_restart_their_marquee_each_dwell() {
        let mut state = SensorState::default();
        let t0 = Instant::now();
        let cards = vec![card("a", "A")];
        assert_eq!(ids(&state.frame_cards(&cards, t0).0), ["a"]);
        let (visible, phase) = state.frame_cards(&cards, t0 + MIN_PAIR_DWELL);
        assert_eq!(ids(&visible), ["a"]);
        assert_eq!(phase, 0);
        // A second card arriving joins the pair right away.
        let two = vec![card("a", "A"), card("b", "B")];
        assert_eq!(
            ids(&state.frame_cards(&two, t0 + MIN_PAIR_DWELL).0),
            ["a", "b"]
        );
    }
}
