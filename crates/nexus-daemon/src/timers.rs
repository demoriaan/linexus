//! Countdown, pomodoro and stopwatch state for timer widgets.
//!
//! Tap starts or pauses, hold resets, double tap skips to the next pomodoro
//! stage. A finished stage waits for a tap before the next one starts, so a
//! break never begins while nobody is looking.

use nexus_config::{TimerMode, TimerProps};
use nexus_render::{TimerInfo, TimerPhase};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Focus,
    Break,
    LongBreak,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Focus => "focus",
            Stage::Break => "break",
            Stage::LongBreak => "long_break",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Idle,
    Running { since: Instant, before: Duration },
    Paused { elapsed: Duration },
    Done,
}

#[derive(Debug, Clone)]
struct TimerState {
    run: Run,
    stage: Stage,
    round: u8,
}

impl Default for TimerState {
    fn default() -> Self {
        Self {
            run: Run::Idle,
            stage: Stage::Focus,
            round: 1,
        }
    }
}

/// Something worth telling the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimerEvent {
    pub(crate) widget: String,
    /// `focus_done`, `break_done` or `countdown_done`.
    pub(crate) kind: &'static str,
}

#[derive(Debug, Default)]
pub(crate) struct Timers {
    /// By `key`.
    states: HashMap<String, TimerState>,
}

/// Key of a timer widget's state. Widget ids are only unique per page, and
/// each profile keeps its own timers.
pub(crate) fn key(profile: &str, page: &str, widget: &str) -> String {
    format!("{profile}\u{1f}{page}\u{1f}{widget}")
}

fn stage_length(props: &TimerProps, stage: Stage) -> Duration {
    let minutes = match (props.mode, stage) {
        (TimerMode::Pomodoro, Stage::Break) => props.break_minutes,
        (TimerMode::Pomodoro, Stage::LongBreak) => props.long_break_minutes,
        _ => props.minutes,
    };
    Duration::from_secs(u64::from(minutes.max(1)) * 60)
}

fn elapsed(run: Run, now: Instant) -> Duration {
    match run {
        Run::Running { since, before } => before + now.saturating_duration_since(since),
        Run::Paused { elapsed } => elapsed,
        Run::Idle | Run::Done => Duration::ZERO,
    }
}

impl Timers {
    pub(crate) fn tap(&mut self, id: &str, now: Instant) {
        let state = self.states.entry(id.to_string()).or_default();
        state.run = match state.run {
            Run::Idle => Run::Running {
                since: now,
                before: Duration::ZERO,
            },
            Run::Running { .. } => Run::Paused {
                elapsed: elapsed(state.run, now),
            },
            Run::Paused { elapsed } => Run::Running {
                since: now,
                before: elapsed,
            },
            Run::Done => Run::Idle,
        };
    }

    pub(crate) fn reset(&mut self, id: &str) {
        self.states.remove(id);
    }

    pub(crate) fn skip(&mut self, id: &str, props: &TimerProps) {
        let state = self.states.entry(id.to_string()).or_default();
        advance(state, props);
    }

    /// Ends stages whose time is up; returns what finished.
    pub(crate) fn tick(
        &mut self,
        widgets: &[(&str, &TimerProps)],
        now: Instant,
    ) -> Vec<TimerEvent> {
        let mut events = Vec::new();
        for (id, props) in widgets {
            let Some(state) = self.states.get_mut(*id) else {
                continue;
            };
            if props.mode == TimerMode::Stopwatch || !matches!(state.run, Run::Running { .. }) {
                continue;
            }
            if elapsed(state.run, now) >= stage_length(props, state.stage) {
                let kind = match (props.mode, state.stage) {
                    (TimerMode::Countdown, _) => "countdown_done",
                    (_, Stage::Focus) => "focus_done",
                    _ => "break_done",
                };
                if props.mode == TimerMode::Countdown {
                    state.run = Run::Done;
                } else {
                    advance(state, props);
                }
                events.push(TimerEvent {
                    widget: id.to_string(),
                    kind,
                });
            }
        }
        // Forget timers whose widget disappeared from the config.
        self.states
            .retain(|id, _| widgets.iter().any(|(w, _)| w == id));
        events
    }

    pub(crate) fn running(&self) -> bool {
        self.states
            .values()
            .any(|state| matches!(state.run, Run::Running { .. }))
    }

    pub(crate) fn info(&self, id: &str, props: &TimerProps, now: Instant) -> TimerInfo {
        let state = self.states.get(id).cloned().unwrap_or_default();
        let length = stage_length(props, state.stage);
        let spent = elapsed(state.run, now);
        let phase = match state.run {
            Run::Idle => TimerPhase::Idle,
            Run::Running { .. } => TimerPhase::Running,
            Run::Paused { .. } => TimerPhase::Paused,
            Run::Done => TimerPhase::Done,
        };
        let (display_ms, progress) = match props.mode {
            TimerMode::Stopwatch => (
                spent.as_millis() as u64,
                (spent.as_secs_f32() % 60.0) / 60.0,
            ),
            _ => {
                let left = length.saturating_sub(spent);
                // Round up so the display reaches 00:00 exactly when it ends.
                let ms = left.as_millis() as u64;
                (
                    ms.div_ceil(1000) * 1000,
                    spent.as_secs_f32() / length.as_secs_f32(),
                )
            }
        };
        let display_ms = match state.run {
            Run::Idle if props.mode != TimerMode::Stopwatch => length.as_millis() as u64,
            _ => display_ms,
        };
        TimerInfo {
            phase,
            display_ms,
            progress: if phase == TimerPhase::Idle {
                0.0
            } else {
                progress.clamp(0.0, 1.0)
            },
            stage: state.stage.name(),
            round: state.round,
            rounds: if props.mode == TimerMode::Pomodoro {
                props.rounds.max(1)
            } else {
                0
            },
        }
    }
}

fn advance(state: &mut TimerState, props: &TimerProps) {
    if props.mode != TimerMode::Pomodoro {
        state.run = Run::Idle;
        return;
    }
    let rounds = props.rounds.max(1);
    state.stage = match state.stage {
        Stage::Focus if state.round >= rounds => Stage::LongBreak,
        Stage::Focus => Stage::Break,
        Stage::Break => {
            state.round += 1;
            Stage::Focus
        }
        Stage::LongBreak => {
            state.round = 1;
            Stage::Focus
        }
    };
    state.run = Run::Idle;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pomodoro() -> TimerProps {
        TimerProps {
            minutes: 1,
            break_minutes: 1,
            long_break_minutes: 2,
            rounds: 2,
            ..TimerProps::default()
        }
    }

    #[test]
    fn pomodoro_cycles_focus_break_and_long_break() {
        let props = pomodoro();
        let mut timers = Timers::default();
        let t = Instant::now();
        timers.tap("t", t);
        assert_eq!(
            timers
                .info("t", &props, t + Duration::from_secs(15))
                .display_ms,
            45_000
        );
        let events = timers.tick(&[("t", &props)], t + Duration::from_secs(60));
        assert_eq!(events[0].kind, "focus_done");
        let info = timers.info("t", &props, t + Duration::from_secs(61));
        assert_eq!(
            (info.phase, info.stage, info.round),
            (TimerPhase::Idle, "break", 1)
        );
        timers.skip("t", &props);
        assert_eq!(timers.info("t", &props, t).round, 2);
        timers.skip("t", &props);
        assert_eq!(timers.info("t", &props, t).stage, "long_break");
        timers.skip("t", &props);
        let info = timers.info("t", &props, t);
        assert_eq!((info.stage, info.round), ("focus", 1));
    }

    #[test]
    fn pause_resume_and_reset() {
        let props = TimerProps {
            mode: TimerMode::Countdown,
            minutes: 1,
            ..TimerProps::default()
        };
        let mut timers = Timers::default();
        let t = Instant::now();
        timers.tap("c", t);
        timers.tap("c", t + Duration::from_secs(10));
        let paused = timers.info("c", &props, t + Duration::from_secs(40));
        assert_eq!(
            (paused.phase, paused.display_ms),
            (TimerPhase::Paused, 50_000)
        );
        timers.tap("c", t + Duration::from_secs(40));
        let events = timers.tick(&[("c", &props)], t + Duration::from_secs(90));
        assert_eq!(events[0].kind, "countdown_done");
        assert_eq!(timers.info("c", &props, t).phase, TimerPhase::Done);
        timers.reset("c");
        assert_eq!(timers.info("c", &props, t).phase, TimerPhase::Idle);
    }

    #[test]
    fn stopwatch_counts_up() {
        let props = TimerProps {
            mode: TimerMode::Stopwatch,
            ..TimerProps::default()
        };
        let mut timers = Timers::default();
        let t = Instant::now();
        timers.tap("s", t);
        assert_eq!(
            timers
                .info("s", &props, t + Duration::from_secs(75))
                .display_ms,
            75_000
        );
        assert!(timers
            .tick(&[("s", &props)], t + Duration::from_secs(9_999))
            .is_empty());
    }
}
