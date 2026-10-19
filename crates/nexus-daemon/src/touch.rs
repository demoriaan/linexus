//! Turns raw strip touches (X only) into interactions.
//!
//! A finger that stays put is a press on the widget under it: released
//! quickly it is a tap, held past the hold threshold it is a hold (fired
//! while the finger is still down, so the strip reacts without waiting for
//! the release). A finger that travels drags the page along and changes it
//! on release when it went far or fast enough. Sliders capture the finger
//! instead and follow it.
//!
//! Taps fire on release unless the widget also has a double-tap meaning;
//! only then is the tap held back for the double-tap window. Widgets that
//! repeat keep firing holds while the finger stays down.
//!
//! Widgets are referred to by index on the page that showed at touch-down,
//! stamped with the runtime's generation so a page, profile or config change
//! in between cannot redirect the touch to another widget.

use std::time::{Duration, Instant};

/// Travel after which a press becomes a page drag. The panel's X jitters by
/// a few pixels during a plain tap.
const DRAG_START_PX: i32 = 24;
/// Release distance that commits a page change.
const COMMIT_PX: i32 = 140;
/// Or a quick flick: at least this far within `FLICK_WINDOW`.
const FLICK_PX: i32 = 60;
const FLICK_WINDOW: Duration = Duration::from_millis(280);
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(300);
const DOUBLE_TAP_DISTANCE: u16 = 80;
/// Pause between repeated holds, like a held key.
const REPEAT_INTERVAL: Duration = Duration::from_millis(160);
/// A finger down without a single report for this long has lifted and the
/// release got lost; it is released where it was last seen.
const LOST_RELEASE: Duration = Duration::from_secs(10);

/// A widget of the page that showed when the finger went down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WidgetRef {
    pub(crate) index: usize,
    /// `Runtime::generation` at touch-down.
    pub(crate) generation: u64,
}

/// What the widget under the finger does with touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Target {
    pub(crate) widget: Option<WidgetRef>,
    /// The widget follows the finger (volume slider).
    pub(crate) slider: bool,
    /// The widget distinguishes double taps, so single taps wait briefly.
    pub(crate) double_tap: bool,
    /// A hold keeps firing until the finger lifts.
    pub(crate) repeat: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Interaction {
    /// Finger down on a widget: show press feedback.
    Press {
        widget: Option<WidgetRef>,
        x: u16,
    },
    /// Finger lifted or turned into a drag without triggering anything.
    Release,
    Tap {
        widget: Option<WidgetRef>,
        x: u16,
    },
    DoubleTap {
        widget: Option<WidgetRef>,
        x: u16,
    },
    Hold {
        widget: Option<WidgetRef>,
        x: u16,
    },
    /// Slider position while captured; `done` on release.
    Slide {
        widget: WidgetRef,
        x: u16,
        done: bool,
    },
    /// Page follows the finger; positive `dx` reveals the next page.
    Drag {
        dx: i32,
    },
    /// Release of a page drag: change page (`forward`) or snap back.
    DragEnd {
        commit: Option<bool>,
        dx: i32,
    },
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Idle,
    Pressing {
        start_x: u16,
        last_x: u16,
        since: Instant,
        target: Target,
        held: bool,
        /// When the last hold fired, for repeats.
        fired: Instant,
    },
    Sliding {
        widget: WidgetRef,
    },
    Dragging {
        start_x: u16,
        last_x: u16,
        since: Instant,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct TouchMachine {
    pub(crate) hold: Duration,
    mode: Mode,
    pending_tap: Option<(Instant, u16, Option<WidgetRef>)>,
    /// When and where the finger was last reported while down.
    last_input: (Instant, u16),
}

impl Default for TouchMachine {
    fn default() -> Self {
        Self {
            hold: Duration::from_millis(600),
            mode: Mode::Idle,
            pending_tap: None,
            last_input: (Instant::now(), 0),
        }
    }
}

impl TouchMachine {
    /// Forgets the touch in progress and any held-back tap, without firing
    /// anything.
    pub(crate) fn reset(&mut self) {
        self.mode = Mode::Idle;
        self.pending_tap = None;
    }

    pub(crate) fn began(&mut self, x: u16, target: Target, now: Instant) -> Vec<Interaction> {
        self.last_input = (now, x);
        if target.slider {
            if let Some(widget) = target.widget {
                self.mode = Mode::Sliding { widget };
                return vec![Interaction::Slide {
                    widget,
                    x,
                    done: false,
                }];
            }
        }
        self.mode = Mode::Pressing {
            start_x: x,
            last_x: x,
            since: now,
            target,
            held: false,
            fired: now,
        };
        vec![Interaction::Press {
            widget: target.widget,
            x,
        }]
    }

    pub(crate) fn moved(&mut self, x: u16, now: Instant) -> Vec<Interaction> {
        self.last_input = (now, x);
        match &mut self.mode {
            Mode::Sliding { widget } => vec![Interaction::Slide {
                widget: *widget,
                x,
                done: false,
            }],
            Mode::Pressing {
                start_x,
                last_x,
                held,
                ..
            } => {
                *last_x = x;
                let dx = i32::from(*start_x) - i32::from(x);
                if !*held && dx.abs() >= DRAG_START_PX {
                    let start_x = *start_x;
                    self.mode = Mode::Dragging {
                        start_x,
                        last_x: x,
                        since: now,
                    };
                    self.pending_tap = None;
                    vec![Interaction::Release, Interaction::Drag { dx }]
                } else {
                    Vec::new()
                }
            }
            Mode::Dragging {
                start_x, last_x, ..
            } => {
                *last_x = x;
                vec![Interaction::Drag {
                    dx: i32::from(*start_x) - i32::from(x),
                }]
            }
            Mode::Idle => Vec::new(),
        }
    }

    pub(crate) fn ended(&mut self, x: u16, now: Instant) -> Vec<Interaction> {
        let mode = std::mem::replace(&mut self.mode, Mode::Idle);
        match mode {
            Mode::Idle => Vec::new(),
            Mode::Sliding { widget } => vec![Interaction::Slide {
                widget,
                x,
                done: true,
            }],
            Mode::Dragging { start_x, since, .. } => {
                let dx = i32::from(start_x) - i32::from(x);
                let quick =
                    now.saturating_duration_since(since) <= FLICK_WINDOW && dx.abs() >= FLICK_PX;
                let commit = (dx.abs() >= COMMIT_PX || quick).then_some(dx > 0);
                vec![Interaction::DragEnd { commit, dx }]
            }
            Mode::Pressing {
                start_x,
                target,
                held,
                ..
            } => {
                let mut out = vec![Interaction::Release];
                if held {
                    return out;
                }
                if !target.double_tap {
                    self.pending_tap = None;
                    out.push(Interaction::Tap {
                        widget: target.widget,
                        x: start_x,
                    });
                    return out;
                }
                match self.pending_tap.take() {
                    Some((at, prev_x, widget))
                        if now.saturating_duration_since(at) <= DOUBLE_TAP_WINDOW
                            && prev_x.abs_diff(start_x) <= DOUBLE_TAP_DISTANCE
                            && widget == target.widget =>
                    {
                        out.push(Interaction::DoubleTap { widget, x: start_x });
                    }
                    Some((_, prev_x, widget)) => {
                        out.push(Interaction::Tap { widget, x: prev_x });
                        self.pending_tap = Some((now, start_x, target.widget));
                    }
                    None => self.pending_tap = Some((now, start_x, target.widget)),
                }
                out
            }
        }
    }

    /// Fires holds while the finger is down, flushes a held-back tap once
    /// the double-tap window has passed and releases a finger that went
    /// silent (`LOST_RELEASE`). Call every loop iteration.
    pub(crate) fn poll(&mut self, now: Instant) -> Vec<Interaction> {
        let (last_at, last_x) = self.last_input;
        if !matches!(self.mode, Mode::Idle)
            && now.saturating_duration_since(last_at) >= LOST_RELEASE
        {
            return self.ended(last_x, now);
        }
        let mut out = Vec::new();
        if let Mode::Pressing {
            start_x,
            since,
            target,
            held,
            fired,
            ..
        } = &mut self.mode
        {
            let first = !*held && now.saturating_duration_since(*since) >= self.hold;
            let again =
                *held && target.repeat && now.saturating_duration_since(*fired) >= REPEAT_INTERVAL;
            if first || again {
                *held = true;
                *fired = now;
                self.pending_tap = None;
                out.push(Interaction::Hold {
                    widget: target.widget,
                    x: *start_x,
                });
            }
        }
        if let Some((at, x, widget)) = self.pending_tap {
            if now.saturating_duration_since(at) > DOUBLE_TAP_WINDOW {
                self.pending_tap = None;
                out.push(Interaction::Tap { widget, x });
            }
        }
        out
    }

    /// Next instant `poll` may produce something, for loop pacing.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let hold = match self.mode {
            Mode::Pressing {
                since, held: false, ..
            } => Some(since + self.hold),
            Mode::Pressing {
                held: true,
                target: Target { repeat: true, .. },
                fired,
                ..
            } => Some(fired + REPEAT_INTERVAL),
            _ => None,
        };
        let tap = self
            .pending_tap
            .map(|(at, ..)| at + DOUBLE_TAP_WINDOW + Duration::from_millis(1));
        let lost = (!matches!(self.mode, Mode::Idle)).then_some(self.last_input.0 + LOST_RELEASE);
        [hold, tap, lost].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(index: usize) -> WidgetRef {
        WidgetRef {
            index,
            generation: 0,
        }
    }

    fn button(idx: usize) -> Target {
        Target {
            widget: Some(w(idx)),
            slider: false,
            double_tap: false,
            repeat: false,
        }
    }

    #[test]
    fn quick_press_is_an_immediate_tap() {
        let mut m = TouchMachine::default();
        let t = Instant::now();
        assert_eq!(
            m.began(100, button(1), t),
            vec![Interaction::Press {
                widget: Some(w(1)),
                x: 100
            }]
        );
        assert!(m.moved(104, t + Duration::from_millis(20)).is_empty());
        let out = m.ended(104, t + Duration::from_millis(80));
        assert_eq!(
            out,
            vec![
                Interaction::Release,
                Interaction::Tap {
                    widget: Some(w(1)),
                    x: 100
                }
            ]
        );
    }

    #[test]
    fn hold_fires_while_pressed_and_suppresses_the_tap() {
        let mut m = TouchMachine::default();
        let t = Instant::now();
        m.began(300, button(2), t);
        assert!(m.poll(t + Duration::from_millis(500)).is_empty());
        assert_eq!(
            m.poll(t + Duration::from_millis(620)),
            vec![Interaction::Hold {
                widget: Some(w(2)),
                x: 300
            }]
        );
        assert_eq!(
            m.ended(300, t + Duration::from_millis(900)),
            vec![Interaction::Release]
        );
    }

    #[test]
    fn repeating_widgets_keep_firing_while_held() {
        let mut m = TouchMachine::default();
        let t = Instant::now();
        let target = Target {
            repeat: true,
            ..button(1)
        };
        m.began(200, target, t);
        assert_eq!(m.poll(t + Duration::from_millis(620)).len(), 1);
        assert_eq!(
            m.next_deadline(),
            Some(t + Duration::from_millis(620) + REPEAT_INTERVAL)
        );
        assert!(m.poll(t + Duration::from_millis(700)).is_empty());
        assert_eq!(m.poll(t + Duration::from_millis(790)).len(), 1);
        assert_eq!(m.poll(t + Duration::from_millis(960)).len(), 1);
        assert_eq!(
            m.ended(200, t + Duration::from_millis(1000)),
            vec![Interaction::Release]
        );
        assert!(m.poll(t + Duration::from_millis(1400)).is_empty());
    }

    #[test]
    fn travel_turns_into_a_page_drag_that_commits_or_snaps_back() {
        let mut m = TouchMachine::default();
        let t = Instant::now();
        m.began(500, button(0), t);
        let out = m.moved(470, t + Duration::from_millis(100));
        assert_eq!(
            out,
            vec![Interaction::Release, Interaction::Drag { dx: 30 }]
        );
        assert_eq!(
            m.moved(300, t + Duration::from_millis(400)),
            vec![Interaction::Drag { dx: 200 }]
        );
        assert_eq!(
            m.ended(300, t + Duration::from_millis(500)),
            vec![Interaction::DragEnd {
                commit: Some(true),
                dx: 200
            }]
        );
        // Short slow drag snaps back.
        m.began(200, button(0), t);
        m.moved(240, t + Duration::from_millis(100));
        assert_eq!(
            m.ended(250, t + Duration::from_millis(900)),
            vec![Interaction::DragEnd {
                commit: None,
                dx: -50
            }]
        );
        // A quick flick commits even when short.
        m.began(200, button(0), t);
        m.moved(240, t + Duration::from_millis(60));
        assert_eq!(
            m.ended(270, t + Duration::from_millis(150)),
            vec![Interaction::DragEnd {
                commit: Some(false),
                dx: -70
            }]
        );
    }

    #[test]
    fn double_tap_only_waits_for_widgets_that_use_it() {
        let mut m = TouchMachine::default();
        let news = Target {
            widget: Some(w(0)),
            slider: false,
            double_tap: true,
            repeat: false,
        };
        let t = Instant::now();
        m.began(100, news, t);
        assert_eq!(
            m.ended(100, t + Duration::from_millis(60)),
            vec![Interaction::Release]
        );
        m.began(110, news, t + Duration::from_millis(200));
        assert_eq!(
            m.ended(110, t + Duration::from_millis(260)),
            vec![
                Interaction::Release,
                Interaction::DoubleTap {
                    widget: Some(w(0)),
                    x: 110
                }
            ]
        );
        // A lone tap is flushed after the window.
        m.began(100, news, t + Duration::from_secs(2));
        m.ended(100, t + Duration::from_millis(2_060));
        assert!(m.poll(t + Duration::from_millis(2_200)).is_empty());
        assert_eq!(
            m.poll(t + Duration::from_millis(2_400)),
            vec![Interaction::Tap {
                widget: Some(w(0)),
                x: 100
            }]
        );
    }

    #[test]
    fn sliders_capture_the_finger() {
        let mut m = TouchMachine::default();
        let slider = Target {
            widget: Some(w(3)),
            slider: true,
            double_tap: false,
            repeat: false,
        };
        let t = Instant::now();
        assert_eq!(
            m.began(500, slider, t),
            vec![Interaction::Slide {
                widget: w(3),
                x: 500,
                done: false
            }]
        );
        assert_eq!(
            m.moved(300, t),
            vec![Interaction::Slide {
                widget: w(3),
                x: 300,
                done: false
            }]
        );
        assert_eq!(
            m.ended(310, t),
            vec![Interaction::Slide {
                widget: w(3),
                x: 310,
                done: true
            }]
        );
    }

    #[test]
    fn reset_drops_the_press_and_the_held_back_tap() {
        let mut m = TouchMachine::default();
        let t = Instant::now();
        let news = Target {
            double_tap: true,
            ..button(0)
        };
        m.began(100, news, t);
        m.ended(100, t + Duration::from_millis(50));
        m.began(300, button(1), t + Duration::from_millis(100));
        m.reset();
        assert_eq!(m.next_deadline(), None);
        assert!(m.poll(t + Duration::from_secs(2)).is_empty());
        assert!(m.ended(300, t + Duration::from_secs(2)).is_empty());
    }

    #[test]
    fn a_press_whose_release_got_lost_ends_by_itself() {
        let mut m = TouchMachine::default();
        let t = Instant::now();
        let target = Target {
            repeat: true,
            ..button(1)
        };
        m.began(200, target, t);
        assert_eq!(m.poll(t + Duration::from_millis(620)).len(), 1);
        // Reports keep the press alive, repeats go on.
        m.moved(204, t + Duration::from_secs(5));
        assert_eq!(m.poll(t + Duration::from_secs(14)).len(), 1);
        // Ten seconds of silence: released at the last position.
        let silent = t + Duration::from_secs(5) + LOST_RELEASE;
        assert!(m.next_deadline().is_some_and(|at| at <= silent));
        assert_eq!(m.poll(silent), vec![Interaction::Release]);
        assert_eq!(m.next_deadline(), None);
        assert!(m.poll(silent + Duration::from_secs(1)).is_empty());
    }
}
