use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::icons;
use crate::live::TimerPhase;
use crate::text::{FontFace, TextStyle};
use nexus_config::{TimerMode, TimerProps, Widget};

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &TimerProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let locale = ctx.live.locale;
    let timer = ctx.live.timers.get(&widget.id).cloned().unwrap_or_default();
    let stage_color = match timer.stage {
        "break" | "long_break" => palette.good,
        _ => ctx.accent(widget, palette.series[4]),
    };
    let pressed = ctx.pressed(widget);
    if pressed > 0.0 {
        canvas.round_rect(b.inset(3.0, 3.0), 8.0, stage_color, 0.2 * pressed);
    }

    let (cx, cy, r) = (b.x + 26.0, 24.0, 16.0);
    canvas.arc(cx, cy, r, 0.0, 360.0, palette.track, 1.0, 3.5);
    if timer.progress > 0.0 {
        canvas.arc(
            cx,
            cy,
            r,
            -90.0,
            360.0 * timer.progress.clamp(0.0, 1.0),
            stage_color,
            1.0,
            3.5,
        );
    }
    let glyph = match timer.phase {
        TimerPhase::Running => "pause",
        TimerPhase::Done => "rotate-ccw",
        _ => "play",
    };
    let nudge = if glyph == "play" { 1.0 } else { 0.0 };
    if glyph == "rotate-ccw" {
        icons::draw(canvas, glyph, cx - 7.0, cy - 7.0, 14.0, palette.text, 1.0);
    } else {
        icons::draw_filled(
            canvas,
            glyph,
            cx - 7.0 + nudge,
            cy - 7.0,
            14.0,
            palette.text,
            1.0,
        );
    }

    let text_x = b.x + 52.0;
    let room = b.right() - text_x - 6.0;
    let mut caption = match (props.mode, timer.stage) {
        (TimerMode::Stopwatch, _) => format::word(Word::Stopwatch, locale).to_string(),
        (TimerMode::Countdown, _) => props
            .label
            .clone()
            .unwrap_or_else(|| format::word(Word::Timer, locale).to_string()),
        (TimerMode::Pomodoro, "break") => format::word(Word::Break, locale).to_string(),
        (TimerMode::Pomodoro, "long_break") => format::word(Word::LongBreak, locale).to_string(),
        (TimerMode::Pomodoro, _) => props
            .label
            .clone()
            .unwrap_or_else(|| format::word(Word::Focus, locale).to_string()),
    };
    if props.mode == TimerMode::Pomodoro && timer.rounds > 0 {
        caption = format!(
            "{caption} · {} {}/{}",
            format::word(Word::Round, locale),
            timer.round.max(1),
            timer.rounds
        );
    }
    canvas.text_fit(
        text_x,
        17.0,
        &caption.to_uppercase(),
        &ctx.caption().color(stage_color),
        room,
    );
    let (time, dim) = match timer.phase {
        TimerPhase::Idle => {
            let ms = match props.mode {
                TimerMode::Stopwatch => 0,
                _ => u64::from(props.minutes) * 60_000,
            };
            (format::duration(ms), true)
        }
        TimerPhase::Done => (format::word(Word::Done, locale).to_string(), false),
        _ => (
            format::duration(timer.display_ms),
            timer.phase == TimerPhase::Paused,
        ),
    };
    let color = if dim { palette.text_dim } else { palette.text };
    canvas.text_fit(
        text_x,
        41.0,
        &time,
        &TextStyle::new(FontFace::Bold, 24.0, color).tabular(),
        room,
    );
}
