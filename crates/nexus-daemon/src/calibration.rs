//! Touch calibration: the strip shows marks one after the other, the user
//! touches each, and the panel's raw readings give the correction to store
//! in `settings.touch`.
//!
//! Each mark is touched twice (two rounds across the strip), since a finger
//! lands within about a dozen pixels of where it aims. A straight line fit
//! through all touches gives the correction at both edges.

use nexus_ipc::{CalibrationState, CalibrationStatus, TouchPhase};
use std::time::{Duration, Instant};

/// Where the marks stand, left to right.
pub(crate) const MARKS: [u16; 5] = [40, 180, 320, 460, 600];
const ROUNDS: usize = 2;
/// The calibration gives up after this long without a touch.
const IDLE_LIMIT: Duration = Duration::from_secs(30);
/// A fit that stretches the strip more than this is not a correction but a
/// series of stray touches.
const MAX_STRETCH: f32 = 0.25;
/// Largest correction stored at either edge, as the config allows.
const MAX_CORRECTION: f32 = nexus_config::MAX_TOUCH_CORRECTION as f32;
/// Marks stand 140 px apart: a touch further than half of that from its mark
/// was not aimed at it, so it does not count and the mark stays.
const STRAY_PX: i32 = 70;
/// A touch this far from the fitted line says the touches do not describe a
/// panel offset, whatever the line through them looks like.
const MAX_RESIDUAL: f32 = 30.0;
/// A finger lands within about a dozen pixels of where it aims: less than
/// this at both edges is the finger, not the panel.
const NOISE_PX: f32 = 10.0;

pub(crate) struct Calibration {
    /// (mark, raw X the panel read).
    samples: Vec<(u16, u16)>,
    /// X where the finger came down, until it lifts.
    down: Option<u16>,
    /// When the last mark was hit, or the calibration started.
    last_input: Instant,
    /// A touch was too far from its mark since the last one that counted.
    missed: bool,
}

impl Calibration {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            samples: Vec::new(),
            down: None,
            last_input: now,
            missed: false,
        }
    }

    pub(crate) fn steps() -> usize {
        MARKS.len() * ROUNDS
    }

    /// The mark to touch now, or None once all were touched.
    pub(crate) fn mark(&self) -> Option<u16> {
        (self.samples.len() < Self::steps()).then(|| MARKS[self.samples.len() % MARKS.len()])
    }

    pub(crate) fn step(&self) -> usize {
        self.samples.len()
    }

    /// A touch on the panel, at its raw X. The X where the finger came
    /// down counts: the panel reports a lift where the touch was last seen.
    pub(crate) fn touch(&mut self, phase: TouchPhase, raw_x: u16, now: Instant) {
        match phase {
            TouchPhase::Began => self.down = Some(raw_x),
            TouchPhase::Moved => {}
            TouchPhase::Ended => {
                let (Some(x), Some(mark)) = (self.down.take(), self.mark()) else {
                    return;
                };
                if (i32::from(x) - i32::from(mark)).abs() > STRAY_PX {
                    self.missed = true;
                } else {
                    self.samples.push((mark, x));
                    self.missed = false;
                    self.last_input = now;
                }
            }
        }
    }

    /// No mark was hit for a while.
    pub(crate) fn timed_out(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_input) >= IDLE_LIMIT
    }

    /// The touches since the last mark that counted were all off the mark.
    pub(crate) fn missed(&self) -> bool {
        self.missed
    }

    /// The correction at the left and right edges, once every mark was
    /// touched; None when the touches do not fit a correction.
    pub(crate) fn result(&self) -> Option<(i16, i16)> {
        if self.mark().is_some() {
            return None;
        }
        fit(&self.samples)
    }

    pub(crate) fn status(&self) -> CalibrationStatus {
        CalibrationStatus {
            state: CalibrationState::Running,
            step: self.step(),
            steps: Self::steps(),
            left: None,
            right: None,
        }
    }
}

/// Least squares line `mark = a * raw + b`, as the correction it means at
/// raw X 0 and 639.
fn fit(samples: &[(u16, u16)]) -> Option<(i16, i16)> {
    let n = samples.len() as f32;
    if n < 2.0 {
        return None;
    }
    let mean =
        |pick: fn(&(u16, u16)) -> u16| samples.iter().map(|s| f32::from(pick(s))).sum::<f32>() / n;
    let (mark_mean, raw_mean) = (mean(|s| s.0), mean(|s| s.1));
    let spread: f32 = samples
        .iter()
        .map(|&(_, raw)| (f32::from(raw) - raw_mean).powi(2))
        .sum();
    if spread < 1.0 {
        return None;
    }
    let a = samples
        .iter()
        .map(|&(mark, raw)| (f32::from(raw) - raw_mean) * (f32::from(mark) - mark_mean))
        .sum::<f32>()
        / spread;
    if (a - 1.0).abs() > MAX_STRETCH {
        return None;
    }
    let b = mark_mean - a * raw_mean;
    let residual = samples
        .iter()
        .map(|&(mark, raw)| (a * f32::from(raw) + b - f32::from(mark)).abs())
        .fold(0.0, f32::max);
    if residual > MAX_RESIDUAL {
        return None;
    }
    let last = f32::from(nexus_config::STRIP_WIDTH - 1);
    let left = b;
    let right = a * last + b - last;
    if left.abs() > MAX_CORRECTION || right.abs() > MAX_CORRECTION {
        return None;
    }
    if left.abs() < NOISE_PX && right.abs() < NOISE_PX {
        return Some((0, 0));
    }
    Some((left.round() as i16, right.round() as i16))
}

/// Status of a calibration that ended.
pub(crate) fn ended(state: CalibrationState, result: Option<(i16, i16)>) -> CalibrationStatus {
    CalibrationStatus {
        state,
        step: if state == CalibrationState::Done {
            Calibration::steps()
        } else {
            0
        },
        steps: Calibration::steps(),
        left: result.map(|r| r.0),
        right: result.map(|r| r.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(calibration: &mut Calibration, x: u16, now: Instant) {
        calibration.touch(TouchPhase::Began, x, now);
        calibration.touch(TouchPhase::Moved, x + 3, now);
        calibration.touch(TouchPhase::Ended, x + 5, now);
    }

    #[test]
    fn walks_the_marks_twice_and_measures_the_correction() {
        let now = Instant::now();
        let mut calibration = Calibration::new(now);
        assert_eq!(calibration.mark(), Some(40));
        // A panel like the one measured by hand: 28 px short at the left,
        // 11 px at the right, and a finger a few pixels off either way.
        let wobble = [3i32, -4, 2, -1, 4, -3, 1, -2, 3, -3];
        for (step, jitter) in wobble.iter().enumerate() {
            let mark = calibration.mark().unwrap();
            let actual = f32::from(mark);
            // Solve x + 28 - 17 x / 639 = actual for the reported x.
            let reported = (actual - 28.0) / (1.0 - 17.0 / 639.0);
            tap(&mut calibration, (reported as i32 + jitter) as u16, now);
            assert_eq!(calibration.step(), step + 1);
        }
        assert_eq!(calibration.mark(), None);
        let (left, right) = calibration.result().unwrap();
        assert!((left - 28).abs() <= 3, "{left}");
        assert!((right - 11).abs() <= 3, "{right}");
    }

    #[test]
    fn a_touch_far_from_its_mark_does_not_count() {
        let now = Instant::now();
        let mut calibration = Calibration::new(now);
        tap(&mut calibration, 300, now);
        assert_eq!((calibration.step(), calibration.mark()), (0, Some(40)));
        assert!(calibration.missed());
        tap(&mut calibration, 45, now);
        assert_eq!((calibration.step(), calibration.mark()), (1, Some(180)));
        assert!(!calibration.missed());
    }

    #[test]
    fn one_wild_touch_fails_the_calibration() {
        let now = Instant::now();
        let mut calibration = Calibration::new(now);
        // Close enough to count, far enough to bend any line.
        for offset in [0, 0, 0, 65, 0, 0, 0, 0, 0, 0] {
            let mark = calibration.mark().unwrap();
            tap(&mut calibration, mark - offset, now);
        }
        assert_eq!(calibration.mark(), None);
        assert_eq!(calibration.result(), None);
    }

    #[test]
    fn finger_accuracy_alone_is_no_correction() {
        let now = Instant::now();
        let mut calibration = Calibration::new(now);
        for wobble in [8i32, -6, 7, -5, 6, -8, 5, -7, 8, -6] {
            let mark = calibration.mark().unwrap();
            tap(&mut calibration, (i32::from(mark) + wobble) as u16, now);
        }
        assert_eq!(calibration.result(), Some((0, 0)));
    }

    #[test]
    fn only_touching_a_mark_keeps_a_calibration_alive() {
        let now = Instant::now();
        let mut calibration = Calibration::new(now);
        tap(&mut calibration, 300, now + Duration::from_secs(20));
        assert!(!calibration.timed_out(now + Duration::from_secs(29)));
        assert!(calibration.timed_out(now + Duration::from_secs(30)));
        // A lift without a touch down does not count.
        let mut calibration = Calibration::new(now);
        calibration.touch(TouchPhase::Ended, 40, now);
        assert_eq!(calibration.step(), 0);
        assert!(!calibration.missed());
    }
}
