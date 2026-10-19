//! Serializable configuration model (schema version 2).
//!
//! A profile holds pages; a page is one row of widgets laid out along the
//! 640 px strip. The touch panel only reports X, so widgets never overlap:
//! every X position belongs to at most one widget, and that widget decides
//! what a tap, double tap or hold at that position means.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

pub const STRIP_WIDTH: u16 = 640;
pub const STRIP_HEIGHT: u16 = 48;
/// Narrowest widget the editor and the validator accept.
pub const MIN_WIDGET_WIDTH: u16 = 24;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NexusConfig {
    pub schema_version: u32,
    pub active_profile: String,
    #[serde(default)]
    pub settings: Settings,
    pub profiles: Vec<Profile>,
}

/// Device-wide preferences that do not depend on the active profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// `auto`, `en` or `de`; drives weekday names and the few words the
    /// widgets print on the strip.
    pub locale: String,
    pub hour12: bool,
    pub temperature_unit: TemperatureUnit,
    pub transition: PageTransition,
    pub idle: IdleSettings,
    pub weather: Option<GeoLocation>,
    pub notifications: NotificationSettings,
    pub overlays: OverlaySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: "auto".into(),
            hour12: false,
            temperature_unit: TemperatureUnit::Celsius,
            transition: PageTransition::Slide,
            idle: IdleSettings::default(),
            weather: None,
            notifications: NotificationSettings::default(),
            overlays: OverlaySettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageTransition {
    #[default]
    Slide,
    Fade,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IdleSettings {
    /// Dim the strip after this many minutes without a touch; 0 disables.
    pub dim_after_minutes: u16,
    /// Brightness (percent of the profile brightness) while dimmed.
    pub dim_percent: u8,
    /// While dimmed, the first touch only wakes the strip and does not
    /// trigger the widget under the finger.
    pub wake_touch_only: bool,
    /// Blank the strip while the session is locked or the system sleeps.
    pub blank_when_locked: bool,
    /// Turn the strip off after this many minutes without a touch; 0 keeps
    /// it on. The first touch afterwards only wakes it.
    pub off_after_minutes: u16,
}

impl Default for IdleSettings {
    fn default() -> Self {
        Self {
            dim_after_minutes: 0,
            dim_percent: 30,
            wake_touch_only: true,
            blank_when_locked: true,
            off_after_minutes: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeoLocation {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NotificationSettings {
    /// Mirror desktop notifications onto the strip.
    pub enabled: bool,
    /// How long a notification stays on the strip.
    pub duration_ms: u32,
    /// Application names that never show up on the strip.
    pub muted_apps: Vec<String>,
    /// Hide the notification body and show only the application name.
    pub private: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            duration_ms: 6000,
            muted_apps: Vec::new(),
            private: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OverlaySettings {
    /// Briefly show a volume bar when the system volume changes.
    pub volume: bool,
    /// Briefly show the new track when the playing track changes.
    pub now_playing: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            volume: true,
            now_playing: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub brightness: u8,
    #[serde(default)]
    pub theme: Theme,
    pub pages: Vec<Page>,
    /// Activate this profile while one of these applications has focus.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Theme {
    /// Name of a built-in palette (`midnight`, `aurora`, `ember`, `mono`,
    /// `daylight`).
    pub preset: String,
    /// Overrides the palette's accent colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<HexColor>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            preset: "midnight".into(),
            accent: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Page {
    pub id: String,
    pub name: String,
    /// Hidden pages are skipped by swipes and reached only through a
    /// `go_to_page` action, like folders on a stream deck.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Return to the previous page after this many seconds without a touch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_return_seconds: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Background>,
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Background {
    /// `data:image/...;base64,...`; when absent the theme background shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<HexColor>,
    #[serde(default)]
    pub fit: ImageFit,
    /// Darkens the image so widgets stay readable, 0..=90.
    #[serde(default)]
    pub dim_percent: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    #[default]
    Fill,
    Fit,
    Stretch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Widget {
    pub id: String,
    pub x: u16,
    pub w: u16,
    #[serde(flatten)]
    pub kind: WidgetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<HexColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<ActionSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_tap: Option<ActionSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ActionSpec>,
    /// Holding the widget repeats its hold action (or its tap action when
    /// it has none) until the finger lifts, like a held key.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
}

impl Widget {
    pub fn new(id: impl Into<String>, x: u16, w: u16, kind: WidgetKind) -> Self {
        Self {
            id: id.into(),
            x,
            w,
            kind,
            accent: None,
            tap: None,
            double_tap: None,
            hold: None,
            repeat: false,
        }
    }

    pub fn with_tap(mut self, action: ActionSpec) -> Self {
        self.tap = Some(action);
        self
    }

    pub fn with_accent(mut self, accent: HexColor) -> Self {
        self.accent = Some(accent);
        self
    }

    pub fn end(&self) -> u16 {
        self.x.saturating_add(self.w)
    }

    pub fn contains(&self, x: u16) -> bool {
        x >= self.x && x < self.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WidgetKind {
    Clock(ClockProps),
    Metric(MetricProps),
    Network(NetworkProps),
    Media(MediaProps),
    Volume(VolumeProps),
    Button(ButtonProps),
    News(NewsProps),
    Weather(WeatherProps),
    Entity(EntityProps),
    Timer(TimerProps),
    Text(TextProps),
    Image(ImageProps),
    Command(CommandProps),
    Spacer,
}

impl WidgetKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Clock(_) => "clock",
            Self::Metric(_) => "metric",
            Self::Network(_) => "network",
            Self::Media(_) => "media",
            Self::Volume(_) => "volume",
            Self::Button(_) => "button",
            Self::News(_) => "news",
            Self::Weather(_) => "weather",
            Self::Entity(_) => "entity",
            Self::Timer(_) => "timer",
            Self::Text(_) => "text",
            Self::Image(_) => "image",
            Self::Command(_) => "command",
            Self::Spacer => "spacer",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ClockProps {
    pub show_date: bool,
    pub show_seconds: bool,
    /// Overrides `settings.hour12` for this clock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hour12: Option<bool>,
    /// Small caption, e.g. a city name for a second clock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// IANA zone such as `America/New_York`; local time when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl Default for ClockProps {
    fn default() -> Self {
        Self {
            show_date: true,
            show_seconds: false,
            hour12: None,
            label: None,
            timezone: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricProps {
    /// Metric id from the daemon's metric catalog, e.g. `cpu.usage`,
    /// `gpu.temp`, `ram.used`, `net.down` or `hwmon:k10temp/Tctl`.
    pub source: String,
    pub style: MetricStyle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Second, smaller reading in the corner (e.g. `cpu.temp` next to CPU
    /// load).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<String>,
    /// Value (in the metric's unit) from which the reading turns amber.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn: Option<f32>,
    /// Value from which the reading turns red.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical: Option<f32>,
}

impl Default for MetricProps {
    fn default() -> Self {
        Self {
            source: "cpu.usage".into(),
            style: MetricStyle::Sparkline,
            label: None,
            secondary: None,
            warn: None,
            critical: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MetricStyle {
    /// Big number only.
    Number,
    /// Number with a horizontal fill bar.
    Bar,
    /// Number over a history graph of the last minute.
    #[default]
    Sparkline,
    /// Number inside a ring gauge.
    Ring,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct NetworkProps {
    /// Interface name; all non-loopback interfaces are summed when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MediaProps {
    /// Preferred MPRIS player (bus name suffix such as `spotify`); the most
    /// recently active player is used when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<String>,
    pub show_art: bool,
    pub show_progress: bool,
    pub show_controls: bool,
    /// Tint the widget background with the album art's dominant colour.
    pub tint: bool,
}

impl Default for MediaProps {
    fn default() -> Self {
        Self {
            player: None,
            show_art: true,
            show_progress: true,
            show_controls: true,
            tint: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VolumeProps {
    pub show_value: bool,
    /// Volume change per notch when the slider is not dragged but tapped at
    /// its ends.
    pub step: u8,
}

impl Default for VolumeProps {
    fn default() -> Self {
        Self {
            show_value: true,
            step: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ButtonProps {
    pub label: String,
    /// `lucide:<name>`, `app:<desktop id>` or a `data:image/...` URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub style: ButtonStyle,
    pub show_label: bool,
    /// For launcher buttons: show a dot while the application runs.
    pub running_indicator: bool,
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            icon: None,
            style: ButtonStyle::Tile,
            show_label: true,
            running_indicator: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    /// Rounded tile with icon above the label.
    #[default]
    Tile,
    /// Icon and label side by side without a tile.
    Inline,
    /// Icon only.
    Icon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NewsProps {
    /// Feed ids to show; all enabled feeds when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub feeds: Vec<String>,
    /// `pixel` keeps the crisp bitmap headline font, `smooth` uses the UI
    /// font.
    pub font: NewsFont,
}

impl Default for NewsProps {
    fn default() -> Self {
        Self {
            feeds: Vec::new(),
            font: NewsFont::Pixel,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NewsFont {
    #[default]
    Pixel,
    Smooth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct WeatherProps {
    /// Location for this widget; `settings.weather` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<GeoLocation>,
    /// Show today's high and low next to the current temperature.
    pub show_range: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct EntityProps {
    /// Home Assistant entity id, e.g. `light.desk`.
    pub entity_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TimerProps {
    pub mode: TimerMode,
    /// Countdown length, or the focus phase of a pomodoro.
    pub minutes: u16,
    pub break_minutes: u16,
    pub long_break_minutes: u16,
    /// Focus phases before a long break.
    pub rounds: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Default for TimerProps {
    fn default() -> Self {
        Self {
            mode: TimerMode::Pomodoro,
            minutes: 25,
            break_minutes: 5,
            long_break_minutes: 15,
            rounds: 4,
            label: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimerMode {
    Countdown,
    #[default]
    Pomodoro,
    Stopwatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TextProps {
    /// May contain `{metric.id}` tokens, e.g. `CPU {cpu.temp}`.
    pub text: String,
    pub size: TextSize,
    pub align: TextAlign,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

impl Default for TextProps {
    fn default() -> Self {
        Self {
            text: "Hello".into(),
            size: TextSize::Medium,
            align: TextAlign::Center,
            caption: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ImageProps {
    /// `data:image/...;base64,...`
    pub src: String,
    pub fit: ImageFit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CommandProps {
    /// Program and arguments (no shell); runs only when the daemon allows
    /// shell actions.
    pub program: String,
    pub args: Vec<String>,
    pub interval_seconds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Default for CommandProps {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
            interval_seconds: 30,
            label: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionSpec {
    OpenUrl {
        url: String,
    },
    /// Opens a file or folder with its default application.
    OpenPath {
        path: String,
    },
    LaunchApp {
        target: String,
    },
    #[serde(alias = "media_key")]
    Media {
        command: MediaKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        player: Option<String>,
    },
    Volume {
        command: VolumeAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
    },
    MicrophoneMute,
    KeyboardShortcut {
        keys: Vec<String>,
    },
    TextInput {
        text: String,
    },
    ShellCommand {
        program: String,
        args: Vec<String>,
    },
    HttpRequest {
        method: HttpMethod,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    /// Calls a Home Assistant service, e.g. `light.toggle`.
    HomeAssistant {
        service: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity_id: Option<String>,
    },
    #[serde(alias = "screen_next")]
    NextPage,
    #[serde(alias = "screen_previous")]
    PreviousPage,
    GoToPage {
        page_id: String,
    },
    /// Returns from a page opened with `go_to_page`.
    GoBack,
    #[serde(alias = "profile_switch")]
    SwitchProfile {
        profile_id: String,
    },
    NextProfile,
    PreviousProfile,
    Brightness {
        command: BrightnessAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
    },
    /// Turns the strip off until the next touch.
    DisplayOff,
    LockScreen,
    /// Runs several actions in order.
    Sequence {
        steps: Vec<ActionSpec>,
        #[serde(default)]
        delay_ms: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
    Stop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeAction {
    Up,
    Down,
    Set,
    ToggleMute,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    #[default]
    Post,
    Put,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrightnessAction {
    Up,
    Down,
    Set,
    /// Steps through 25, 50, 75 and 100 percent.
    Cycle,
}

/// `#rrggbb` colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HexColor(pub [u8; 3]);

impl HexColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b])
    }
}

impl fmt::Display for HexColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b] = self.0;
        write!(f, "#{r:02x}{g:02x}{b:02x}")
    }
}

impl FromStr for HexColor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.trim().trim_start_matches('#');
        // Also keeps the byte slicing below on char boundaries.
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("expected #rrggbb, got '{s}'"));
        }
        let expanded: String = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 => hex.to_string(),
            _ => return Err(format!("expected #rrggbb, got '{s}'")),
        };
        let byte = |i: usize| {
            u8::from_str_radix(&expanded[i..i + 2], 16)
                .map_err(|_| format!("expected #rrggbb, got '{s}'"))
        };
        Ok(Self([byte(0)?, byte(2)?, byte(4)?]))
    }
}

impl Serialize for HexColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}
