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
    Milliseconds,
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

/// A host the daemon pings, for ping widgets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PingInfo {
    pub host: String,
    /// Round trip of each recent probe in milliseconds, oldest first;
    /// `None` for a lost probe.
    pub history: Vec<Option<f32>>,
    /// The latest answered probe.
    pub last_ms: Option<f32>,
    pub avg_ms: Option<f32>,
    pub min_ms: Option<f32>,
    pub max_ms: Option<f32>,
    pub jitter_ms: Option<f32>,
    /// Share of lost probes; `None` before the first probe.
    pub loss_percent: Option<f32>,
    /// The last few probes all went unanswered.
    pub down: bool,
    /// The host name does not resolve.
    pub unresolved: bool,
    /// Measured by connecting to TCP port 443 instead of ICMP echo.
    pub tcp: bool,
    /// The host is this computer's default gateway.
    pub gateway: bool,
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
    /// Breaking news of the last hour: labelled as such instead of with its
    /// source.
    pub alert: bool,
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
    /// A breaking-news banner.
    Alert {
        /// Feed name.
        source: String,
        headline: String,
        /// Opened when the banner is tapped.
        link: Option<String>,
        /// The headline's card id.
        id: String,
        /// How a headline too long for the banner scrolls.
        marquee: MarqueeStyle,
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
    /// Draw the microphone badge in the corner of the page; set by the daemon.
    pub mic_badge: bool,
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

/// Sound outputs, inputs and playing apps; set by the daemon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioInfo {
    /// Shown outputs in the order the widgets show them.
    pub outputs: Vec<AudioDevice>,
    pub inputs: Vec<AudioDevice>,
    /// `node` of the output and input in use.
    pub default_output: Option<String>,
    pub default_input: Option<String>,
    /// Apps that play sound, in the order they started.
    pub apps: Vec<AudioApp>,
    /// Live microphone level 0..=100 while the meter records.
    pub level: Option<u8>,
    /// Page of tiles per widget id, for widgets with more than fit.
    pub pages: HashMap<String, usize>,
    /// The device or app under the finger while a slider is dragged.
    pub grabbed: Option<String>,
    /// A device or app whose slider was let go, and where: shown until
    /// PipeWire reports the new volume, so the knob does not jump back.
    pub held: Option<(String, f32)>,
}

impl AudioInfo {
    /// Where a slider let go of shows while PipeWire catches up.
    pub fn held(&self, key: &str) -> Option<f32> {
        self.held
            .as_ref()
            .filter(|(held, _)| held == key)
            .map(|(_, fraction)| *fraction)
    }

    pub fn output(&self) -> Option<&AudioDevice> {
        let node = self.default_output.as_deref()?;
        self.outputs.iter().find(|device| device.node == node)
    }

    pub fn input(&self) -> Option<&AudioDevice> {
        let node = self.default_input.as_deref()?;
        self.inputs.iter().find(|device| device.node == node)
    }

    pub fn page(&self, widget: &str) -> usize {
        self.pages.get(widget).copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioKind {
    Headset,
    Monitor,
    #[default]
    Speaker,
    Microphone,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioDevice {
    /// PipeWire's `node.name`.
    pub node: String,
    pub name: String,
    /// Smaller line below the name.
    pub detail: String,
    pub kind: AudioKind,
    pub volume: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioApp {
    pub name: String,
    pub volume: u8,
    pub muted: bool,
}

/// Apps that have a microphone or camera open, for the red pill.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrivacyInfo {
    pub uses: Vec<PrivacyEntry>,
    /// The list of them fills the strip.
    pub open: bool,
    /// The pill shows its icons only, a while after the apps last changed.
    pub compact: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrivacyEntry {
    pub app: String,
    pub camera: bool,
    /// Which microphone or camera.
    pub device: String,
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
    /// Pinged hosts by name; `""` is `settings.ping.host`.
    pub ping: HashMap<String, PingInfo>,
    /// None while PipeWire is not followed (no sound widget shows).
    pub audio: Option<AudioInfo>,
    /// None or empty while nobody records or films.
    pub privacy: Option<PrivacyInfo>,
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
