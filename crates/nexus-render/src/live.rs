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
    Fr,
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
        let tag = tag.to_ascii_lowercase();
        if tag.starts_with("de") {
            Self::De
        } else if tag.starts_with("fr") {
            Self::Fr
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

/// OBS Studio's state while the daemon is connected to it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObsInfo {
    /// Scene on the program output.
    pub scene: String,
    pub streaming: bool,
    /// The stream lost its connection and OBS is trying again.
    pub stream_reconnecting: bool,
    pub stream_ms: u64,
    pub recording: bool,
    pub record_paused: bool,
    pub record_ms: u64,
    pub replay_buffer: bool,
    pub virtual_cam: bool,
    /// Mute state per audio source name.
    pub muted: HashMap<String, bool>,
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
        /// Opened when the notification is tapped (a Discord message).
        link: Option<String>,
        /// A body too long for one line scrolls to its end instead of
        /// being cut.
        scroll: bool,
    },
    Volume(VolumeInfo),
    NowPlaying(Box<MediaInfo>),
    Toast {
        /// Lucide icon name.
        icon: String,
        text: String,
    },
}

/// The Discord app's voice state while the daemon is signed in to it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscordInfo {
    /// The voice channel you are in, and its server.
    pub channel: Option<String>,
    pub guild: Option<String>,
    pub self_muted: bool,
    pub self_deafened: bool,
    /// Voice channels of the servers voice widgets show, keyed by server id.
    pub guilds: HashMap<String, Vec<DiscordVoiceChannel>>,
    /// Microphone level, 0..=100 %.
    pub input_volume: u8,
    /// What you hear, 0..=200 %.
    pub output_volume: u16,
    /// Push to talk instead of voice activity.
    pub push_to_talk: bool,
    pub noise_suppression: bool,
    pub echo_cancellation: bool,
    pub automatic_gain: bool,
    /// The voice connection, while you are in a channel.
    pub link: Option<DiscordLink>,
    /// The member whose volume the call widget shows; set by the daemon.
    pub focus: Option<String>,
    /// Draw the muted badge in the corner of the page; set by the daemon.
    pub mute_badge: bool,
    /// Counts up every few seconds; voice widgets with more channels than
    /// room show their next page. Set by the daemon.
    pub rotation: u64,
    /// Your own user id, for the call widget's controls.
    pub self_id: Option<String>,
}

/// How the voice connection is doing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscordLink {
    pub connected: bool,
    /// Last round trip to the voice server.
    pub ping_ms: Option<u32>,
}

impl DiscordLink {
    /// 3 good, 2 fair, 1 poor, 0 not connected.
    pub fn bars(&self) -> u8 {
        match (self.connected, self.ping_ms) {
            (false, _) => 0,
            (true, None) => 2,
            (true, Some(0..=99)) => 3,
            (true, Some(100..=249)) => 2,
            (true, Some(_)) => 1,
        }
    }
}

impl DiscordInfo {
    /// The voice channel you sit in, when its server is followed.
    pub fn current_channel(&self) -> Option<&DiscordVoiceChannel> {
        let id = self.channel.as_deref()?;
        self.guilds
            .values()
            .flatten()
            .find(|channel| channel.id == id)
    }

    /// The channels a voice widget shows: its own server's, or the one of
    /// the channel you are in.
    pub fn channels_for(&self, guild: Option<&str>) -> &[DiscordVoiceChannel] {
        guild
            .or(self.guild.as_deref())
            .and_then(|guild| self.guilds.get(guild))
            .map_or(&[], Vec::as_slice)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscordVoiceChannel {
    pub id: String,
    pub name: String,
    pub members: Vec<DiscordMember>,
}

#[derive(Debug, Clone, Default)]
pub struct DiscordMember {
    pub user_id: String,
    pub name: String,
    pub avatar: Option<Arc<Pixmap>>,
    pub speaking: bool,
    pub muted: bool,
    pub deafened: bool,
    /// How loud you hear them, 0..=200 %.
    pub volume: u16,
    /// Muted by you, for you only.
    pub local_muted: bool,
}

/// Pictures compare by identity: the same avatar is the same `Arc`.
impl PartialEq for DiscordMember {
    fn eq(&self, other: &Self) -> bool {
        self.user_id == other.user_id
            && self.name == other.name
            && self.speaking == other.speaking
            && self.muted == other.muted
            && self.deafened == other.deafened
            && self.volume == other.volume
            && self.local_muted == other.local_muted
            && match (&self.avatar, &other.avatar) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
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
    /// None while OBS is not connected (or not set up).
    pub obs: Option<ObsInfo>,
    /// None while Discord is not connected (or not set up).
    pub discord: Option<DiscordInfo>,
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
