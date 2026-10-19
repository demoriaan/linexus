//! Everything a frame shows that is not configuration: readings, media,
//! weather, timers and interaction feedback. The daemon and Studio fill it;
//! the renderer only reads it, so rendering stays free of I/O.

use crate::Rgb;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tiny_skia::Pixmap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    #[default]
    En,
    De,
}

impl Locale {
    /// `auto` follows `LC_ALL`/`LC_MESSAGES`/`LANG`.
    pub fn resolve(setting: &str) -> Self {
        let tag = match setting {
            "auto" | "" => std::env::var("LC_ALL")
                .ok()
                .filter(|v| !v.is_empty())
                .or_else(|| std::env::var("LC_MESSAGES").ok().filter(|v| !v.is_empty()))
                .or_else(|| std::env::var("LANG").ok())
                .unwrap_or_default(),
            other => other.to_string(),
        };
        if tag.to_ascii_lowercase().starts_with("de") {
            Self::De
        } else {
            Self::En
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    /// 0 = Monday.
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Percent,
    Celsius,
    Watts,
    Rpm,
    Bytes,
    BytesPerSec,
    Megahertz,
    Plain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub value: f32,
    pub unit: Unit,
    /// Short label such as `CPU`.
    pub label: String,
    /// Full scale for bars, rings and graphs (100 for percentages).
    pub max: f32,
}

#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub player: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub position_s: Option<f32>,
    pub length_s: Option<f32>,
    pub art: Option<Arc<Pixmap>>,
    pub art_color: Option<Rgb>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VolumeInfo {
    pub percent: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeatherInfo {
    pub location: String,
    pub temp_c: f32,
    /// WMO weather interpretation code as reported by Open-Meteo.
    pub code: u16,
    pub is_day: bool,
    pub high_c: Option<f32>,
    pub low_c: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityInfo {
    pub name: String,
    pub state: String,
    pub unit: Option<String>,
    /// `Some` for entities that can be switched.
    pub on: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimerPhase {
    #[default]
    Idle,
    Running,
    Paused,
    Done,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TimerInfo {
    pub phase: TimerPhase,
    /// Time to show: remaining for countdowns, elapsed for stopwatches.
    pub display_ms: u64,
    /// 0..=1 progress of the current phase.
    pub progress: f32,
    /// `focus`, `break`, `long_break` or empty.
    pub stage: &'static str,
    pub round: u8,
    pub rounds: u8,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewsRow {
    pub source: String,
    pub headline: String,
    pub seen: bool,
}

/// Pacing of the headline marquee: a long headline rests at its start,
/// scrolls right to left at `speed_px_per_sec`, then parks at its end. It
/// never wraps by itself; the daemon restarts the phase when it rotates to
/// the next pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarqueeStyle {
    pub speed_px_per_sec: u32,
    pub start_pause_ms: u64,
    pub end_pause_ms: u64,
}

impl Default for MarqueeStyle {
    fn default() -> Self {
        Self {
            speed_px_per_sec: 50,
            start_pause_ms: 2_000,
            end_pause_ms: 1_500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewsInfo {
    /// Top and bottom headline; fewer when the queue is short.
    pub rows: Vec<NewsRow>,
    /// Milliseconds since the visible pair came on screen.
    pub marquee_phase_ms: u64,
    pub marquee: MarqueeStyle,
    /// Row to outline as tap feedback.
    pub highlight: Option<usize>,
}

/// A transient message drawn over the page (desktop notification, volume
/// change, new track).
#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Notification {
        app: String,
        title: String,
        body: String,
        icon: Option<Arc<Pixmap>>,
    },
    Volume(VolumeInfo),
    NowPlaying(Box<MediaInfo>),
    Toast {
        /// Lucide icon name.
        icon: String,
        text: String,
    },
}

impl PartialEq for MediaInfo {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title && self.artist == other.artist && self.playing == other.playing
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveData {
    pub time: LocalTime,
    pub locale: Locale,
    pub hour12: bool,
    pub fahrenheit: bool,
    pub metrics: HashMap<String, Metric>,
    /// Recent raw values per metric id, oldest first.
    pub history: HashMap<String, Vec<f32>>,
    pub media: Option<MediaInfo>,
    pub volume: Option<VolumeInfo>,
    /// Weather per location name; a widget looks up its own location.
    pub weather: HashMap<String, WeatherInfo>,
    pub entities: HashMap<String, EntityInfo>,
    /// Per timer widget id.
    pub timers: HashMap<String, TimerInfo>,
    pub news: NewsInfo,
    /// Latest output per command widget id.
    pub commands: HashMap<String, String>,
    /// Launch targets (desktop ids, executables) that are running.
    pub running: HashSet<String>,
    /// Resolved images for `app:` icon references, keyed by the reference.
    pub images: HashMap<String, Arc<Pixmap>>,
}

impl LiveData {
    pub fn metric(&self, id: &str) -> Option<&Metric> {
        self.metrics.get(id)
    }
}

/// Interaction feedback for the current frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiState {
    /// Widget under the finger and how far the press animation has run.
    pub pressed: Option<(String, f32)>,
    /// Slider value (0..=1) shown while a finger drags it.
    pub dragging: Option<(String, f32)>,
    pub page_index: usize,
    pub page_count: usize,
    /// Opacity of the page indicator, shown briefly after a page change.
    pub indicator_alpha: f32,
    /// Free-running animation clock in milliseconds.
    pub clock_ms: u64,
}
