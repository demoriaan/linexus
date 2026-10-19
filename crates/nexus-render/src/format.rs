//! Number formatting and the handful of words the strip prints.

use crate::live::{LocalTime, Locale, Unit};

/// Splits a reading into the number and its unit, e.g. `("12.4", "MB/s")`.
pub fn value(value: f32, unit: Unit, fahrenheit: bool) -> (String, String) {
    match unit {
        Unit::Percent => (format!("{:.0}", value.clamp(0.0, 999.0)), "%".into()),
        Unit::Celsius if fahrenheit => (format!("{:.0}", value * 9.0 / 5.0 + 32.0), "°F".into()),
        Unit::Celsius => (format!("{value:.0}"), "°C".into()),
        Unit::Watts => (format!("{value:.0}"), "W".into()),
        Unit::Rpm => (format!("{value:.0}"), "RPM".into()),
        Unit::Megahertz if value >= 1000.0 => (format!("{:.2}", value / 1000.0), "GHz".into()),
        Unit::Megahertz => (format!("{value:.0}"), "MHz".into()),
        Unit::Bytes => {
            let (n, u) = scale_bytes(value);
            (n, u.into())
        }
        Unit::BytesPerSec => {
            let (n, u) = scale_bytes(value);
            (n, format!("{u}/s"))
        }
        Unit::Plain => (compact(value), String::new()),
    }
}

/// Compact text for a reading, e.g. `64°` for temperatures in corners.
pub fn short(value: f32, unit: Unit, fahrenheit: bool) -> String {
    let (number, unit_text) = self::value(value, unit, fahrenheit);
    match unit {
        Unit::Celsius => format!("{number}°"),
        Unit::Percent => format!("{number}%"),
        _ => format!("{number} {unit_text}").trim().to_string(),
    }
}

fn scale_bytes(value: f32) -> (String, &'static str) {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = value.max(0.0);
    let mut idx = 0;
    while v >= 1000.0 && idx < UNITS.len() - 1 {
        v /= 1024.0;
        idx += 1;
    }
    let text = if idx == 0 || v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    (text, UNITS[idx])
}

fn compact(value: f32) -> String {
    if value.fract().abs() < 0.05 || value.abs() >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

pub fn clock(time: &LocalTime, hour12: bool, seconds: bool) -> (String, &'static str) {
    let (hour, suffix) = if hour12 {
        let h = match time.hour % 12 {
            0 => 12,
            h => h,
        };
        (h, if time.hour < 12 { "AM" } else { "PM" })
    } else {
        (time.hour, "")
    };
    let mut text = if hour12 {
        format!("{hour}:{:02}", time.minute)
    } else {
        format!("{hour:02}:{:02}", time.minute)
    };
    if seconds {
        text.push_str(&format!(":{:02}", time.second));
    }
    (text, suffix)
}

pub fn date(time: &LocalTime, locale: Locale) -> String {
    let weekday = weekday_short(time.weekday, locale);
    let month = month_short(time.month, locale);
    match locale {
        Locale::De => format!("{weekday} {}. {month}", time.day),
        Locale::En => format!("{weekday} {month} {}", time.day),
    }
}

fn weekday_short(weekday: u8, locale: Locale) -> &'static str {
    const EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const DE: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];
    let i = usize::from(weekday.min(6));
    match locale {
        Locale::En => EN[i],
        Locale::De => DE[i],
    }
}

fn month_short(month: u8, locale: Locale) -> &'static str {
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const DE: [&str; 12] = [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ];
    let i = usize::from(month.clamp(1, 12) - 1);
    match locale {
        Locale::En => EN[i],
        Locale::De => DE[i],
    }
}

/// `mm:ss`, or `h:mm:ss` from one hour.
pub fn duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Words printed on the strip.
#[derive(Debug, Clone, Copy)]
pub enum Word {
    NothingPlaying,
    Paused,
    Volume,
    Muted,
    On,
    Off,
    Unavailable,
    Focus,
    Break,
    LongBreak,
    Stopwatch,
    Timer,
    TapToStart,
    Done,
    NoHeadlines,
    AddFeedsInStudio,
    SetLocation,
    Loading,
    Round,
    Down,
    Up,
}

pub fn word(word: Word, locale: Locale) -> &'static str {
    use Word::*;
    match (locale, word) {
        (Locale::En, NothingPlaying) => "Nothing playing",
        (Locale::De, NothingPlaying) => "Keine Wiedergabe",
        (Locale::En, Paused) => "Paused",
        (Locale::De, Paused) => "Pausiert",
        (Locale::En, Volume) => "Volume",
        (Locale::De, Volume) => "Lautstärke",
        (Locale::En, Muted) => "Muted",
        (Locale::De, Muted) => "Stumm",
        (Locale::En, On) => "On",
        (Locale::De, On) => "An",
        (Locale::En, Off) => "Off",
        (Locale::De, Off) => "Aus",
        (Locale::En, Unavailable) => "Unavailable",
        (Locale::De, Unavailable) => "Nicht erreichbar",
        (Locale::En, Focus) => "Focus",
        (Locale::De, Focus) => "Fokus",
        (Locale::En, Break) => "Break",
        (Locale::De, Break) => "Pause",
        (Locale::En, LongBreak) => "Long break",
        (Locale::De, LongBreak) => "Lange Pause",
        (Locale::En, Stopwatch) => "Stopwatch",
        (Locale::De, Stopwatch) => "Stoppuhr",
        (Locale::En, Timer) => "Timer",
        (Locale::De, Timer) => "Timer",
        (Locale::En, TapToStart) => "Tap to start",
        (Locale::De, TapToStart) => "Tippen zum Start",
        (Locale::En, Done) => "Done",
        (Locale::De, Done) => "Fertig",
        (Locale::En, NoHeadlines) => "No headlines yet",
        (Locale::De, NoHeadlines) => "Noch keine Schlagzeilen",
        (Locale::En, AddFeedsInStudio) => "Add news feeds in Linexus Studio",
        (Locale::De, AddFeedsInStudio) => "Nachrichtenquellen in Linexus Studio hinzufügen",
        (Locale::En, SetLocation) => "Set a location in Studio",
        (Locale::De, SetLocation) => "Ort in Studio festlegen",
        (Locale::En, Loading) => "Loading…",
        (Locale::De, Loading) => "Lädt…",
        (Locale::En, Round) => "Round",
        (Locale::De, Round) => "Runde",
        (Locale::En, Down) => "Down",
        (Locale::De, Down) => "Runter",
        (Locale::En, Up) => "Up",
        (Locale::De, Up) => "Hoch",
    }
}

/// Lucide icon and short description for a WMO weather code.
pub fn weather(code: u16, is_day: bool, locale: Locale) -> (&'static str, &'static str) {
    let de = locale == Locale::De;
    match code {
        0 => (
            if is_day { "sun" } else { "moon" },
            if de { "Klar" } else { "Clear" },
        ),
        1 | 2 => (
            if is_day { "cloud-sun" } else { "cloud-moon" },
            if de {
                "Leicht bewölkt"
            } else {
                "Partly cloudy"
            },
        ),
        3 => ("cloud", if de { "Bedeckt" } else { "Overcast" }),
        45 | 48 => ("cloud-fog", if de { "Nebel" } else { "Fog" }),
        51..=57 => ("cloud-drizzle", if de { "Nieselregen" } else { "Drizzle" }),
        61..=67 => ("cloud-rain", if de { "Regen" } else { "Rain" }),
        71..=77 => ("cloud-snow", if de { "Schnee" } else { "Snow" }),
        80..=82 => ("cloud-rain-wind", if de { "Schauer" } else { "Showers" }),
        85 | 86 => (
            "cloud-snow",
            if de { "Schneeschauer" } else { "Snow showers" },
        ),
        95..=99 => (
            "cloud-lightning",
            if de { "Gewitter" } else { "Thunderstorm" },
        ),
        _ => ("cloud", ""),
    }
}

/// Fills `{metric.id}` tokens in `template` with formatted readings.
pub fn expand_tokens(template: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let key = after[..end].trim();
                out.push_str(&lookup(key).unwrap_or_else(|| "–".into()));
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
