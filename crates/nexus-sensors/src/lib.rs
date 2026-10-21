mod feeds;
mod integrations;
mod obs;
mod providers;
mod refresh;
pub use providers::battery::{read_batteries, BatteryReading};
pub use providers::clock::{local_clock, LocalClock};

pub use feeds::{cached_feed_titles, inspect_feed_url, FeedInspection};

pub use obs::{
    input_names as obs_input_names, obs_overview, ObsConnection, ObsError, ObsEvent, ObsOverview,
    ObsServer, DEFAULT_PORT as OBS_DEFAULT_PORT, MAX_INPUTS as OBS_MAX_INPUTS,
};
pub use providers::apps::{
    list_applications, list_desktop_entries, resolve_icon, running_programs, running_targets,
    AppEntry,
};
pub use providers::command::run_command_capture;
pub use providers::focus::{
    gnome_extension_dir, gnome_focus_path, gnome_shell_major, gnome_shell_on_bus, gsettings_get,
    is_gnome, parse_gnome_shell_major, parse_gsettings_strings, FocusWatcher, FocusedWindow,
    GnomeExtensionStatus, GNOME_EXTENSION_UUID,
};
pub use providers::hass::{
    call_service, entity_is_on, fetch_states, parse_states, toggle_service, EntityState,
    HomeAssistantServer,
};
pub use providers::hwmon::{read_hwmon, HwmonKind, HwmonReading};
pub use providers::media::{current_media, current_media_and_players, fetch_art_bytes, MediaState};
pub use providers::metrics::{metric_catalog, read_cpu_clock_mhz, MetricReading, MetricUnit};
pub use providers::notify::{
    Notification, NotificationIcon, NotificationMonitor, PopupPolicy, Urgency,
};
pub use providers::nvidia::present as nvidia_gpu_present;
pub use providers::weather::{
    fetch_weather, geocode, parse_forecast, parse_geocoding, GeoPlace, WeatherReport,
    WEATHER_ATTRIBUTION,
};

pub use integrations::{
    build_home_assistant_mqtt_discovery, build_ifttt_request, build_mqtt_message,
    publish_mqtt_message, send_ifttt_webhook, IftttRequest, MqttDiscoveryMessage, MqttMessage,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default)]
pub struct SystemMetrics {
    pub cpu_usage: Option<f32>,
    pub ram_usage: Option<f32>,
    pub ram_used_gib: Option<f32>,
    pub ram_total_gib: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    /// Average current core clock; see [`read_cpu_clock_mhz`].
    pub cpu_clock_mhz: Option<f32>,
    pub gpu_usage: Option<f32>,
    pub vram_usage: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub gpu_mem_temp_c: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub gpu_fan_rpm: Option<u32>,
    /// Fan speed in percent where the GPU reports no RPM (NVIDIA).
    pub gpu_fan_percent: Option<f32>,
    pub volume_percent: Option<u8>,
    /// Default sink muted; `volume_percent` keeps the level while muted.
    pub volume_muted: Option<bool>,
    pub network_rx_bps: Option<f32>,
    pub network_tx_bps: Option<f32>,
    pub disk_read_bps: Option<f32>,
    pub disk_write_bps: Option<f32>,
    pub disk_root_usage: Option<f32>,
    pub media_player: Option<String>,
    pub media_title: Option<String>,
    pub media_artist: Option<String>,
    pub media_url: Option<String>,
    pub media_pid: Option<u32>,
    pub media_playback_status: Option<String>,
    pub mpris_players: Vec<String>,
    /// Now-playing state of the most relevant MPRIS player.
    pub media: Option<MediaState>,
    pub app_metrics: Vec<AppMetric>,
    pub top_app: Option<AppMetric>,
    pub sensor_records: Vec<SensorRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AppMetric {
    pub id: String,
    pub name: String,
    pub process_count: usize,
    pub rss_mb: f32,
    pub cpu_percent: f32,
    pub top_pid: Option<u32>,
    pub top_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorRecord {
    pub module: String,
    pub kind: String,
    pub label: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorCard {
    pub id: String,
    pub module: String,
    pub kind: String,
    pub source: String,
    pub title: String,
    pub url: Option<String>,
    pub status: Option<String>,
}

impl SensorCard {
    pub fn is_actionable(&self) -> bool {
        self.url.as_deref().is_some_and(|url| {
            url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")
        })
    }
}

pub fn rss_item_key(feed_id: &str, title: &str, link: Option<&str>) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in feed_id
        .as_bytes()
        .iter()
        .chain(title.as_bytes())
        .chain(link.unwrap_or_default().as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("rss:{feed_id}:{hash:016x}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssSeenState {
    #[serde(default)]
    pub seen: BTreeSet<String>,
    #[serde(default)]
    pub dismissed: BTreeSet<String>,
}

pub fn load_rss_seen_state() -> RssSeenState {
    let Ok(text) = fs::read_to_string(rss_seen_state_path()) else {
        return RssSeenState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_rss_seen_state(state: &RssSeenState) -> std::io::Result<()> {
    let path = rss_seen_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(state).unwrap_or_default())
}

pub fn mark_rss_item_seen(id: &str) -> std::io::Result<()> {
    let mut state = load_rss_seen_state();
    state.seen.insert(id.to_string());
    save_rss_seen_state(&state)
}

pub fn dismiss_rss_item(id: &str) -> std::io::Result<()> {
    let mut state = load_rss_seen_state();
    state.seen.insert(id.to_string());
    state.dismissed.insert(id.to_string());
    save_rss_seen_state(&state)
}

pub fn mark_rss_items_seen<'a>(ids: impl IntoIterator<Item = &'a str>) -> std::io::Result<usize> {
    let mut state = load_rss_seen_state();
    let before = state.seen.len();
    for id in ids {
        state.seen.insert(id.to_string());
    }
    let added = state.seen.len().saturating_sub(before);
    save_rss_seen_state(&state)?;
    Ok(added)
}

pub fn reset_rss_seen_state() -> std::io::Result<()> {
    let path = rss_seen_state_path();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn sensor_cards(records: &[SensorRecord]) -> Vec<SensorCard> {
    let mut cards = Vec::new();
    for record in records {
        if record.module == "rss" && record.kind == "rss_feed" {
            if let Some(items) = record.value.get("items").and_then(|items| items.as_array()) {
                let mut ordered_items: Vec<_> = items.iter().collect();
                ordered_items.sort_by_key(|item| {
                    item.get("seen")
                        .and_then(|seen| seen.as_bool())
                        .unwrap_or(false)
                });
                for (idx, item) in ordered_items.into_iter().enumerate() {
                    if item
                        .get("dismissed")
                        .and_then(|dismissed| dismissed.as_bool())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let title = item
                        .get("title")
                        .and_then(|title| title.as_str())
                        .unwrap_or("RSS item")
                        .trim();
                    if title.is_empty() {
                        continue;
                    }
                    let url = item
                        .get("link")
                        .and_then(|link| link.as_str())
                        .map(ToOwned::to_owned);
                    let id = item
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| rss_item_key(&record.label, title, url.as_deref()));
                    let seen = item
                        .get("seen")
                        .and_then(|seen| seen.as_bool())
                        .unwrap_or(false);
                    cards.push(SensorCard {
                        id,
                        module: record.module.clone(),
                        kind: "rss_item".into(),
                        source: record.label.clone(),
                        title: title.to_string(),
                        url,
                        status: Some(if seen { "SEEN" } else { "NEW" }.into()),
                    });
                    if idx >= 9 {
                        break;
                    }
                }
                continue;
            }
        }

        let title = record.display_value();
        if title.trim().is_empty() {
            continue;
        }
        cards.push(SensorCard {
            id: format!("{}:{}:{}", record.module, record.kind, record.label),
            module: record.module.clone(),
            kind: record.kind.clone(),
            source: record.label.clone(),
            title,
            url: record.primary_url(),
            status: None,
        });
    }
    cards
}

pub fn sensor_cards_with_config(
    records: &[SensorRecord],
    config: &SensorConfig,
) -> Vec<SensorCard> {
    let now = unix_now_secs();
    let mut cards = sensor_cards(records);
    cards.retain_mut(|card| match card.module.as_str() {
        "rss" => {
            let feed_id = rss_feed_id_from_card_id(&card.id);
            let Some(rule) = feed_id.and_then(|id| config.rss.iter().find(|feed| feed.id == id))
            else {
                return true;
            };
            if rule.show_mode == "hidden" {
                return false;
            }
            if rule.tap_behavior == "none" {
                card.url = None;
            }
            true
        }
        "webhook" => {
            let id = sanitize_sensor_id(&card.source);
            let Some(rule) = config.webhooks.iter().find(|rule| rule.id == id) else {
                return true;
            };
            if rule.show_mode == "hidden" {
                return false;
            }
            if rule.max_age_seconds.is_some_and(|max_age| {
                webhook_age_seconds(card, records, now).is_some_and(|age| age > max_age)
            }) {
                return false;
            }
            if rule.tap_behavior == "none" {
                card.url = None;
            }
            true
        }
        "home_assistant" => {
            let id = sanitize_sensor_id(&card.source);
            let Some(rule) = config
                .home_assistant
                .iter()
                .find(|entity| entity.id == id || sanitize_sensor_id(&entity.label) == id)
            else {
                return true;
            };
            if rule.show_mode == "hidden" {
                return false;
            }
            if rule.tap_behavior == "none" {
                card.url = None;
            }
            true
        }
        _ => true,
    });
    cards.sort_by_key(|card| {
        (
            -sensor_card_priority(card, config),
            card.status.as_deref() == Some("SEEN"),
        )
    });
    cards
}

pub fn configured_sensor_cards(records: &[SensorRecord]) -> Vec<SensorCard> {
    sensor_cards_with_config(records, &load_sensor_config())
}

pub fn sensor_cards_for_display(
    records: &[SensorRecord],
    animation_tick: u64,
    slots: usize,
) -> Vec<SensorCard> {
    let cards: Vec<_> = configured_sensor_cards(records)
        .into_iter()
        .filter(|card| matches!(card.module.as_str(), "rss" | "webhook" | "home_assistant"))
        .collect();
    rotate_sensor_cards(cards, animation_tick, slots)
}

pub fn rotate_sensor_cards(
    cards: Vec<SensorCard>,
    animation_tick: u64,
    slots: usize,
) -> Vec<SensorCard> {
    if slots == 0 || cards.is_empty() {
        return Vec::new();
    }
    if cards.len() <= slots {
        return cards;
    }
    let start = ((animation_tick / 12) as usize) % cards.len();
    (0..slots)
        .map(|offset| cards[(start + offset) % cards.len()].clone())
        .collect()
}

pub fn sensor_display_card_for_x(cards: &[SensorCard], x: u16) -> Option<&SensorCard> {
    match cards {
        [] => None,
        [only] => Some(only),
        many if x < 320 => many.first(),
        many => many.get(1).or_else(|| many.first()),
    }
}

pub fn sample_integration_card(records: &[SensorRecord]) -> SensorCard {
    configured_sensor_cards(records)
        .into_iter()
        .find(|card| matches!(card.module.as_str(), "rss" | "webhook" | "home_assistant"))
        .unwrap_or_else(|| SensorCard {
            id: "sample".into(),
            module: "sample".into(),
            kind: "message".into(),
            source: "Linexus".into(),
            title: "Hello from Linexus".into(),
            url: Some("https://example.com".into()),
            status: None,
        })
}

pub fn sensor_card_for_x(cards: &[SensorCard], x: u16) -> Option<&SensorCard> {
    let actionable: Vec<_> = cards.iter().filter(|card| card.is_actionable()).collect();
    match actionable.as_slice() {
        [] => None,
        [only] => Some(*only),
        many if x < 320 => many.first().copied(),
        many => many.get(1).copied().or_else(|| many.first().copied()),
    }
}

fn rss_feed_id_from_card_id(id: &str) -> Option<&str> {
    let mut parts = id.split(':');
    (parts.next() == Some("rss"))
        .then(|| parts.next())
        .flatten()
}

fn sensor_card_priority(card: &SensorCard, config: &SensorConfig) -> i32 {
    match card.module.as_str() {
        "rss" => rss_feed_id_from_card_id(&card.id)
            .and_then(|id| config.rss.iter().find(|feed| feed.id == id))
            .map(|feed| feed.priority)
            .unwrap_or_default(),
        "webhook" => {
            let id = sanitize_sensor_id(&card.source);
            config
                .webhooks
                .iter()
                .find(|rule| rule.id == id)
                .map(|rule| rule.priority)
                .unwrap_or_default()
        }
        "home_assistant" => {
            let id = sanitize_sensor_id(&card.source);
            config
                .home_assistant
                .iter()
                .find(|entity| entity.id == id || sanitize_sensor_id(&entity.label) == id)
                .map(|entity| entity.priority)
                .unwrap_or_default()
        }
        _ => 0,
    }
}

fn webhook_age_seconds(card: &SensorCard, records: &[SensorRecord], now: u64) -> Option<u64> {
    records
        .iter()
        .find(|record| record.module == "webhook" && record.label == card.source)
        .and_then(|record| record.value.get("received_at"))
        .and_then(|value| value.as_u64())
        .map(|received_at| now.saturating_sub(received_at))
}

impl SensorRecord {
    pub fn primary_url(&self) -> Option<String> {
        if let Some(link) = self
            .value
            .get("items")
            .and_then(|items| items.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.get("link"))
            .and_then(|link| link.as_str())
        {
            return Some(link.to_string());
        }
        if let Some(link) = self
            .value
            .get("feed")
            .and_then(|feed| feed.get("items"))
            .and_then(|items| items.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.get("link"))
            .and_then(|link| link.as_str())
        {
            return Some(link.to_string());
        }
        let payload = self.value.get("payload").unwrap_or(&self.value);
        for key in ["url", "link", "href"] {
            if let Some(url) = payload.get(key).and_then(|value| value.as_str()) {
                return Some(url.to_string());
            }
        }
        None
    }

    pub fn display_value(&self) -> String {
        if let Some(title) = self
            .value
            .get("items")
            .and_then(|items| items.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.get("title"))
            .and_then(|title| title.as_str())
        {
            return title.to_string();
        }
        if let Some(feed) = self.value.get("feed") {
            if let Some(title) = feed
                .get("items")
                .and_then(|items| items.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("title"))
                .and_then(|title| title.as_str())
            {
                return title.to_string();
            }
        }
        if let Some(payload) = self.value.get("payload") {
            for key in ["message", "status", "text"] {
                if let Some(value) = payload.get(key).and_then(|value| value.as_str()) {
                    return value.to_string();
                }
            }
            if !payload.is_object() && !payload.is_array() {
                return payload.to_string();
            }
        }
        if let Some(text) = self.value.get("text").and_then(|value| value.as_str()) {
            return text.to_string();
        }
        if let Some(state) = self.value.get("state").and_then(|value| value.as_str()) {
            let unit = self
                .value
                .get("unit")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            return format!("{state}{unit}");
        }
        if let Some(fields) = self.value.get("fields").and_then(|value| value.as_object()) {
            if let Some((key, value)) = fields.iter().next() {
                return format!(
                    "{key}: {}",
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| value.to_string())
                );
            }
        }
        serde_json::to_string(&self.value).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SensorSnapshot {
    pub metrics: SystemMetrics,
}

pub trait SensorModule: Send + Sync {
    fn id(&self) -> &'static str;
    fn collect(&self, snapshot: &mut SensorSnapshot);
}

#[derive(Default)]
pub struct SensorHub {
    modules: Vec<Box<dyn SensorModule>>,
}

impl SensorHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_modules() -> Self {
        let mut hub = Self::new();
        hub.register(CoreSystemSensor);
        hub.register(AppProcessSensor::default());
        hub.register(WebhookSensor);
        hub.register(RssSensor::from_config());
        hub.register(HomeAssistantSensor::from_config());
        hub
    }

    pub fn register<M>(&mut self, module: M)
    where
        M: SensorModule + 'static,
    {
        self.modules.push(Box::new(module));
    }

    pub fn collect(&self) -> SensorSnapshot {
        let mut snapshot = SensorSnapshot::default();
        for module in &self.modules {
            module.collect(&mut snapshot);
        }
        snapshot
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CoreSystemSensor;

impl SensorModule for CoreSystemSensor {
    fn id(&self) -> &'static str {
        "core-system"
    }

    fn collect(&self, snapshot: &mut SensorSnapshot) {
        collect_core_metrics_into(&mut snapshot.metrics);
    }
}

#[derive(Debug, Clone)]
pub struct AppProcessSensor {
    rules: Vec<AppRule>,
}

impl Default for AppProcessSensor {
    fn default() -> Self {
        Self {
            rules: vec![
                AppRule::new("brave", "Brave", &["brave"]),
                AppRule::new("firefox", "Firefox", &["firefox"]),
                AppRule::new("chromium", "Chromium", &["chromium"]),
                AppRule::new("chrome", "Chrome", &["chrome"]),
                AppRule::new("konsole", "Konsole", &["konsole"]),
                AppRule::new("dolphin", "Dolphin", &["dolphin"]),
                AppRule::new("plasmashell", "Plasma", &["plasmashell"]),
            ],
        }
    }
}

impl SensorModule for AppProcessSensor {
    fn id(&self) -> &'static str {
        "app-process"
    }

    fn collect(&self, snapshot: &mut SensorSnapshot) {
        let Some(metrics) = collect_app_process_metrics(&self.rules) else {
            return;
        };
        snapshot.metrics.top_app = metrics.first().cloned();
        snapshot.metrics.app_metrics = metrics.clone();
        for metric in metrics {
            snapshot.metrics.sensor_records.push(SensorRecord {
                module: self.id().into(),
                kind: "app_metric".into(),
                label: metric.name.clone(),
                value: serde_json::json!({
                    "id": metric.id,
                    "process_count": metric.process_count,
                    "rss_mb": metric.rss_mb,
                    "cpu_percent": metric.cpu_percent,
                    "top_pid": metric.top_pid,
                    "top_command": metric.top_command,
                }),
            });
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WebhookSensor;

impl SensorModule for WebhookSensor {
    fn id(&self) -> &'static str {
        "webhook"
    }

    fn collect(&self, snapshot: &mut SensorSnapshot) {
        for record in read_webhook_records() {
            snapshot.metrics.sensor_records.push(record);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorConfig {
    #[serde(default)]
    pub display: NewsDisplayConfig,
    #[serde(default)]
    pub rss: Vec<RssFeedConfig>,
    #[serde(default)]
    pub webhooks: Vec<WebhookRuleConfig>,
    #[serde(default)]
    pub home_assistant: Vec<HomeAssistantEntityConfig>,
    #[serde(default)]
    pub ifttt: Vec<IftttWebhookConfig>,
    #[serde(default)]
    pub mqtt: Vec<MqttPublishConfig>,
    /// One Home Assistant connection for entity widgets and service calls,
    /// alongside the older per-entity `home_assistant` list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_assistant_server: Option<HomeAssistantServer>,
    /// OBS Studio's websocket server, for OBS buttons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obs_server: Option<ObsServer>,
}

/// How the sensors page presents headlines. Read by the daemon on every
/// collection, so Studio changes reach the panel within about a second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsDisplayConfig {
    /// Marquee speed for headlines longer than their row.
    pub scroll_speed_px_per_sec: u32,
    /// Rest at the start of a long headline before it scrolls.
    pub start_pause_ms: u32,
    /// Rest at the end of a long headline before the next pair.
    pub end_pause_ms: u32,
    /// Minimum time a pair stays on screen (short headlines).
    pub min_pair_ms: u32,
    /// Press duration after which a touch counts as "hold" (hide headline).
    pub hold_to_dismiss_ms: u32,
}

impl Default for NewsDisplayConfig {
    fn default() -> Self {
        Self {
            scroll_speed_px_per_sec: 50,
            start_pause_ms: 2_000,
            end_pause_ms: 1_500,
            min_pair_ms: 5_000,
            hold_to_dismiss_ms: 600,
        }
    }
}

impl NewsDisplayConfig {
    fn normalize(&mut self) {
        // Same ranges as the Studio sliders, so the UI never shows a value
        // the backend would clamp.
        self.scroll_speed_px_per_sec = self.scroll_speed_px_per_sec.clamp(20, 150);
        self.start_pause_ms = self.start_pause_ms.min(6_000);
        self.end_pause_ms = self.end_pause_ms.min(6_000);
        self.min_pair_ms = self.min_pair_ms.clamp(2_000, 30_000);
        self.hold_to_dismiss_ms = self.hold_to_dismiss_ms.clamp(300, 2_000);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssFeedConfig {
    pub id: String,
    pub url: String,
    #[serde(default = "default_rss_limit")]
    pub limit: usize,
    #[serde(default = "default_rss_refresh_seconds")]
    pub refresh_seconds: u64,
    #[serde(default = "default_show_mode")]
    pub show_mode: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub max_age_seconds: Option<u64>,
    #[serde(default = "default_tap_behavior")]
    pub tap_behavior: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookRuleConfig {
    pub id: String,
    #[serde(default = "default_show_mode")]
    pub show_mode: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub max_age_seconds: Option<u64>,
    #[serde(default = "default_tap_behavior")]
    pub tap_behavior: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeAssistantEntityConfig {
    pub id: String,
    pub base_url: String,
    pub entity_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_show_mode")]
    pub show_mode: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_tap_behavior")]
    pub tap_behavior: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IftttWebhookConfig {
    pub id: String,
    pub event: String,
    pub key: String,
    #[serde(default = "default_value1_template")]
    pub value1_template: String,
    #[serde(default = "default_value2_template")]
    pub value2_template: String,
    #[serde(default = "default_value3_template")]
    pub value3_template: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MqttPublishConfig {
    pub id: String,
    pub host: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    pub topic: String,
    #[serde(default = "default_mqtt_client_id")]
    pub client_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_mqtt_payload_template")]
    pub payload_template: String,
    #[serde(default)]
    pub retain: bool,
    #[serde(default)]
    pub qos: u8,
    #[serde(default)]
    pub home_assistant_discovery: bool,
    #[serde(default = "default_mqtt_discovery_prefix")]
    pub discovery_prefix: String,
    #[serde(default)]
    pub discovery_object_id: String,
    #[serde(default)]
    pub discovery_name: String,
    #[serde(default = "default_mqtt_discovery_device_id")]
    pub discovery_device_id: String,
    #[serde(default = "default_mqtt_discovery_value_template")]
    pub discovery_value_template: String,
}

fn default_rss_limit() -> usize {
    3
}

fn default_rss_refresh_seconds() -> u64 {
    900
}

fn default_show_mode() -> String {
    "unread_first".into()
}

fn default_tap_behavior() -> String {
    "open_url".into()
}

fn default_value1_template() -> String {
    "{title}".into()
}

fn default_value2_template() -> String {
    "{source}".into()
}

fn default_value3_template() -> String {
    "{url}".into()
}

fn default_mqtt_port() -> u16 {
    1883
}

fn default_mqtt_client_id() -> String {
    "linexus".into()
}

fn default_mqtt_payload_template() -> String {
    r#"{"title":"{title}","source":"{source}","url":"{url}"}"#.into()
}

fn default_mqtt_discovery_prefix() -> String {
    "homeassistant".into()
}

fn default_mqtt_discovery_device_id() -> String {
    "linexus".into()
}

fn default_mqtt_discovery_value_template() -> String {
    "{{ value_json.title }}".into()
}

#[derive(Debug, Clone, Default)]
pub struct RssSensor {
    feeds: Vec<RssFeedConfig>,
}

impl RssSensor {
    pub fn from_config() -> Self {
        Self {
            feeds: read_rss_feed_config(),
        }
    }

    pub fn new(feeds: Vec<RssFeedConfig>) -> Self {
        Self { feeds }
    }
}

impl SensorModule for RssSensor {
    fn id(&self) -> &'static str {
        "rss"
    }

    fn collect(&self, snapshot: &mut SensorSnapshot) {
        let seen_state = load_rss_seen_state();
        for feed in &self.feeds {
            let Some(mut channel) = collect_rss_feed(feed) else {
                continue;
            };
            for item in &mut channel.items {
                let title = item.title.as_deref().unwrap_or("RSS item");
                item.id = rss_item_key(&feed.id, title, item.link.as_deref());
                item.seen = seen_state.seen.contains(&item.id);
                item.dismissed = seen_state.dismissed.contains(&item.id);
            }
            snapshot.metrics.sensor_records.push(SensorRecord {
                module: self.id().into(),
                kind: "rss_feed".into(),
                label: channel.title.clone().unwrap_or_else(|| feed.id.clone()),
                value: serde_json::json!({
                    "id": feed.id,
                    "url": feed.url,
                    "title": channel.title,
                    "items": channel.items,
                }),
            });
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HomeAssistantSensor {
    entities: Vec<HomeAssistantEntityConfig>,
}

impl HomeAssistantSensor {
    pub fn from_config() -> Self {
        Self {
            entities: load_sensor_config().home_assistant,
        }
    }

    pub fn new(entities: Vec<HomeAssistantEntityConfig>) -> Self {
        Self { entities }
    }
}

impl SensorModule for HomeAssistantSensor {
    fn id(&self) -> &'static str {
        "home_assistant"
    }

    fn collect(&self, snapshot: &mut SensorSnapshot) {
        for entity in &self.entities {
            let Some(value) = home_assistant_state(entity) else {
                continue;
            };
            let friendly = value
                .get("attributes")
                .and_then(|attributes| attributes.get("friendly_name"))
                .and_then(|name| name.as_str())
                .unwrap_or(&entity.label);
            let state = value
                .get("state")
                .and_then(|state| state.as_str())
                .unwrap_or("unknown");
            let unit = value
                .get("attributes")
                .and_then(|attributes| attributes.get("unit_of_measurement"))
                .and_then(|unit| unit.as_str())
                .unwrap_or("");
            snapshot.metrics.sensor_records.push(SensorRecord {
                module: self.id().into(),
                kind: "entity_state".into(),
                label: friendly.to_string(),
                value: serde_json::json!({
                    "id": entity.id,
                    "entity_id": entity.entity_id,
                    "state": state,
                    "unit": unit,
                    "url": format!("{}/lovelace", entity.base_url.trim_end_matches('/')),
                }),
            });
        }
    }
}

#[derive(Debug, Clone)]
struct AppRule {
    id: &'static str,
    name: &'static str,
    needles: &'static [&'static str],
}

impl AppRule {
    fn new(id: &'static str, name: &'static str, needles: &'static [&'static str]) -> Self {
        Self { id, name, needles }
    }

    fn matches(&self, comm: &str, args: &str) -> bool {
        let text = format!("{comm} {args}").to_ascii_lowercase();
        if text.contains(".agent-browser") {
            return false;
        }
        self.needles.iter().all(|needle| text.contains(needle))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RssChannel {
    pub title: Option<String>,
    pub items: Vec<RssItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RssItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub seen: bool,
    #[serde(default)]
    pub dismissed: bool,
    pub title: Option<String>,
    pub link: Option<String>,
    pub summary: Option<String>,
}

/// Largest webhook body that is stored; a status update is a few hundred
/// bytes, and every stored record is read back by the collector.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 64 * 1024;

/// Stores a webhook delivery as `<id>.json`. Bodies over
/// [`MAX_WEBHOOK_BODY_BYTES`] are refused with `ErrorKind::InvalidInput`.
pub fn record_webhook_payload(
    id: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> std::io::Result<SensorRecord> {
    if body.len() > MAX_WEBHOOK_BODY_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "webhook body is larger than {} KiB",
                MAX_WEBHOOK_BODY_BYTES / 1024
            ),
        ));
    }
    let id = sanitize_sensor_id(id);
    let (kind, value) = parse_sensor_payload(content_type, body);
    let record = SensorRecord {
        module: "webhook".into(),
        kind: kind.into(),
        label: id.clone(),
        value: serde_json::json!({
            "id": id,
            "content_type": content_type.unwrap_or(""),
            "received_at": unix_now_secs(),
            "payload": value,
        }),
    };
    store_webhook_record(&webhook_state_dir(), &record)?;
    Ok(record)
}

fn store_webhook_record(dir: &Path, record: &SensorRecord) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    write_private_file(
        &dir.join(format!("{}.json", record.label)),
        &serde_json::to_vec_pretty(record).unwrap_or_default(),
    )
}

/// Replaces `path` atomically with an owner-only (0600) file: the bytes go to
/// a temp file named after this process and a counter, which is renamed over
/// the target. Readers never see half a file, concurrent writers never share
/// a temp file, and a failed write removes its temp file.
fn write_private_file(path: &Path, body: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_file_name(tmp_name);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut file| file.write_all(body))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

pub fn delete_webhook_record(id: &str) -> std::io::Result<bool> {
    let path = webhook_state_dir().join(format!("{}.json", sanitize_sensor_id(id)));
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn list_webhook_records() -> Vec<SensorRecord> {
    read_webhook_records()
}

pub fn load_sensor_config() -> SensorConfig {
    let Ok(text) = read_sensor_config_text(&sensor_config_path()) else {
        return SensorConfig::default();
    };
    let Ok(mut config) = serde_json::from_str::<SensorConfig>(&text) else {
        return SensorConfig::default();
    };
    normalize_sensor_config(&mut config);
    config
}

/// Why sensors.json cannot be used, when it exists but does not parse
/// (`load_sensor_config` then silently falls back to defaults).
pub fn sensor_config_problem() -> Option<String> {
    let text = fs::read_to_string(sensor_config_path()).ok()?;
    serde_json::from_str::<SensorConfig>(&text)
        .err()
        .map(|error| {
            format!(
                "{} could not be read: {error}",
                sensor_config_path().display()
            )
        })
}

/// Serializes sensor-config writers within one process (Studio handles each
/// request on its own thread).
static SENSOR_CONFIG_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn save_sensor_config(config: SensorConfig) -> std::io::Result<SensorConfig> {
    let _guard = SENSOR_CONFIG_WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    write_sensor_config(config)
}

/// Replaces only the feeds and the headline display settings, keeping every
/// other module (webhooks, Home Assistant, IFTTT, MQTT — including tokens) as
/// stored on disk. Refuses to touch a sensors.json that does not parse, so a
/// hand-edited file with a typo is never replaced by defaults.
pub fn update_news_config(
    rss: Vec<RssFeedConfig>,
    display: NewsDisplayConfig,
) -> std::io::Result<SensorConfig> {
    let _guard = SENSOR_CONFIG_WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut config = match fs::read_to_string(sensor_config_path()) {
        Ok(text) => serde_json::from_str::<SensorConfig>(&text).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} could not be read ({error}); fix or remove it before saving feeds",
                    sensor_config_path().display()
                ),
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SensorConfig::default(),
        Err(error) => return Err(error),
    };
    config.rss = rss;
    config.display = display;
    write_sensor_config(config)
}

/// Normalizes and writes atomically (temp file + rename), so the daemon's
/// once-a-second reader never sees a half-written file. The file holds the
/// Home Assistant token, the IFTTT key and MQTT passwords, so it is 0600.
fn write_sensor_config(mut config: SensorConfig) -> std::io::Result<SensorConfig> {
    normalize_sensor_config(&mut config);
    let path = sensor_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private_file(
        &path,
        &serde_json::to_vec_pretty(&config).unwrap_or_default(),
    )?;
    Ok(config)
}

/// Reads sensors.json, first taking group and other permissions away from a
/// file written by an older version or by hand (it holds secrets).
fn read_sensor_config_text(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    if let Ok(metadata) = file.metadata() {
        if metadata.permissions().mode() & 0o077 != 0 {
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut file, &mut text)?;
    Ok(text)
}

fn normalize_sensor_config(config: &mut SensorConfig) {
    config.display.normalize();
    for feed in &mut config.rss {
        feed.id = sanitize_sensor_id(&feed.id);
        if feed.id == "default" {
            feed.id = url_to_sensor_id(&feed.url);
        }
        feed.limit = feed.limit.clamp(1, 10);
        feed.refresh_seconds = feed.refresh_seconds.max(60);
        feed.show_mode = normalize_show_mode(&feed.show_mode);
        feed.tap_behavior = normalize_tap_behavior(&feed.tap_behavior);
        feed.max_age_seconds = feed.max_age_seconds.map(normalize_max_age_seconds);
    }
    config.rss = dedupe_feeds(std::mem::take(&mut config.rss));
    for webhook in &mut config.webhooks {
        webhook.id = sanitize_sensor_id(&webhook.id);
        webhook.show_mode = normalize_show_mode(&webhook.show_mode);
        webhook.tap_behavior = normalize_tap_behavior(&webhook.tap_behavior);
        webhook.max_age_seconds = webhook.max_age_seconds.map(normalize_max_age_seconds);
    }
    config.webhooks = dedupe_webhook_rules(std::mem::take(&mut config.webhooks));
    for entity in &mut config.home_assistant {
        entity.id = sanitize_sensor_id(&entity.id);
        if entity.id == "default" {
            entity.id = sanitize_sensor_id(&entity.entity_id);
        }
        entity.base_url = entity.base_url.trim_end_matches('/').to_string();
        entity.entity_id = entity.entity_id.trim().to_string();
        if entity.label.trim().is_empty() {
            entity.label = entity.entity_id.clone();
        }
        entity.show_mode = normalize_show_mode(&entity.show_mode);
        entity.tap_behavior = normalize_tap_behavior(&entity.tap_behavior);
    }
    config.home_assistant =
        dedupe_home_assistant_entities(std::mem::take(&mut config.home_assistant));
    if let Some(server) = config.home_assistant_server.as_mut() {
        server.url = server.url.trim().trim_end_matches('/').to_string();
        server.token = server.token.trim().to_string();
    }
    if config
        .home_assistant_server
        .as_ref()
        .is_some_and(|server| server.url.is_empty())
    {
        config.home_assistant_server = None;
    }
    if let Some(server) = config.obs_server.as_mut() {
        let host = server.host.trim();
        server.host = host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(host)
            .to_string();
    }
    if config
        .obs_server
        .as_ref()
        .is_some_and(|server| server.host.is_empty() || server.port == 0)
    {
        config.obs_server = None;
    }
    for ifttt in &mut config.ifttt {
        ifttt.id = sanitize_sensor_id(&ifttt.id);
        ifttt.event = sanitize_ifttt_event(&ifttt.event);
    }
    config.ifttt = dedupe_ifttt_configs(std::mem::take(&mut config.ifttt));
    for mqtt in &mut config.mqtt {
        mqtt.id = sanitize_sensor_id(&mqtt.id);
        mqtt.host = mqtt.host.trim().to_string();
        mqtt.topic = mqtt.topic.trim().to_string();
        mqtt.discovery_prefix = mqtt.discovery_prefix.trim_matches('/').trim().to_string();
        mqtt.discovery_object_id = sanitize_sensor_id(&mqtt.discovery_object_id);
        mqtt.discovery_device_id = sanitize_sensor_id(&mqtt.discovery_device_id);
        mqtt.port = mqtt.port.max(1);
        if mqtt.client_id.trim().is_empty() {
            mqtt.client_id = default_mqtt_client_id();
        }
        if mqtt.discovery_prefix.is_empty() {
            mqtt.discovery_prefix = default_mqtt_discovery_prefix();
        }
        if mqtt.discovery_object_id.is_empty() || mqtt.discovery_object_id == "default" {
            mqtt.discovery_object_id = mqtt.id.clone();
        }
        if mqtt.discovery_name.trim().is_empty() {
            mqtt.discovery_name = format!("Linexus {}", mqtt.id);
        }
        if mqtt.discovery_device_id.is_empty() || mqtt.discovery_device_id == "default" {
            mqtt.discovery_device_id = default_mqtt_discovery_device_id();
        }
        if mqtt.discovery_value_template.trim().is_empty() {
            mqtt.discovery_value_template = default_mqtt_discovery_value_template();
        }
        mqtt.qos = mqtt.qos.min(2);
    }
    config.mqtt = dedupe_mqtt_configs(std::mem::take(&mut config.mqtt));
}

fn normalize_show_mode(value: &str) -> String {
    match value {
        "hidden" | "latest_first" | "unread_first" => value.into(),
        _ => default_show_mode(),
    }
}

fn normalize_tap_behavior(value: &str) -> String {
    match value {
        "none" | "open_url" => value.into(),
        _ => default_tap_behavior(),
    }
}

fn normalize_max_age_seconds(value: u64) -> u64 {
    value.clamp(60, 31 * 24 * 60 * 60)
}

pub fn parse_sensor_payload(
    content_type: Option<&str>,
    body: &[u8],
) -> (&'static str, serde_json::Value) {
    let text = String::from_utf8_lossy(body).trim().to_string();
    let content_type = content_type.unwrap_or("").to_ascii_lowercase();
    if content_type.contains("json") || text.starts_with('{') || text.starts_with('[') {
        return (
            "json",
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "text": text })),
        );
    }
    if content_type.contains("xml")
        || content_type.contains("rss")
        || content_type.contains("atom")
        || text.starts_with('<')
    {
        return ("xml", xml_summary_value(&text));
    }
    ("text", serde_json::json!({ "text": text }))
}

fn read_webhook_records() -> Vec<SensorRecord> {
    static CACHE: Mutex<Option<WebhookRecords>> = Mutex::new(None);
    cached_webhook_records(&webhook_state_dir(), &CACHE)
}

/// Records as parsed at the last read of `dir`.
struct WebhookRecords {
    dir: PathBuf,
    modified: SystemTime,
    records: Vec<SensorRecord>,
}

/// The collector asks every second; the files are only read again when the
/// directory's mtime moved. Every store and delete renames or removes an
/// entry, which updates it.
fn cached_webhook_records(dir: &Path, cache: &Mutex<Option<WebhookRecords>>) -> Vec<SensorRecord> {
    let Ok(modified) = fs::metadata(dir).and_then(|metadata| metadata.modified()) else {
        return Vec::new();
    };
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.dir == dir && cached.modified == modified)
    {
        return cached.records.clone();
    }
    let records = read_webhook_records_in(dir);
    // A change within the same timestamp tick as this read would leave the
    // mtime as it is; only remember reads of a directory that has been quiet
    // for a moment.
    let settled = SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= Duration::from_secs(2));
    *cache = settled.then(|| WebhookRecords {
        dir: dir.to_path_buf(),
        modified,
        records: records.clone(),
    });
    records
}

fn read_webhook_records_in(dir: &Path) -> Vec<SensorRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if let Ok(record) = serde_json::from_str::<SensorRecord>(&text) {
            records.push(record);
        }
    }
    records.sort_by(|a, b| a.label.cmp(&b.label));
    records
}

fn read_rss_feed_config() -> Vec<RssFeedConfig> {
    let mut feeds = Vec::new();
    if let Ok(urls) = std::env::var("NEXUS_SENSOR_RSS_URLS") {
        for url in urls.split(',').map(str::trim).filter(|url| !url.is_empty()) {
            feeds.push(RssFeedConfig {
                id: url_to_sensor_id(url),
                url: url.to_string(),
                limit: 3,
                refresh_seconds: 900,
                show_mode: default_show_mode(),
                priority: 0,
                max_age_seconds: None,
                tap_behavior: default_tap_behavior(),
            });
        }
    }
    feeds.extend(load_sensor_config().rss);
    dedupe_feeds(feeds)
}

fn dedupe_feeds(feeds: Vec<RssFeedConfig>) -> Vec<RssFeedConfig> {
    let mut seen = std::collections::HashSet::new();
    feeds
        .into_iter()
        .filter(|feed| !feed.url.is_empty() && seen.insert(feed.id.clone()))
        .collect()
}

fn dedupe_webhook_rules(rules: Vec<WebhookRuleConfig>) -> Vec<WebhookRuleConfig> {
    let mut seen = std::collections::HashSet::new();
    rules
        .into_iter()
        .filter(|rule| !rule.id.is_empty() && seen.insert(rule.id.clone()))
        .collect()
}

fn dedupe_home_assistant_entities(
    entities: Vec<HomeAssistantEntityConfig>,
) -> Vec<HomeAssistantEntityConfig> {
    let mut seen = std::collections::HashSet::new();
    entities
        .into_iter()
        .filter(|entity| {
            !entity.base_url.is_empty()
                && !entity.entity_id.is_empty()
                && seen.insert(entity.id.clone())
        })
        .collect()
}

fn dedupe_ifttt_configs(configs: Vec<IftttWebhookConfig>) -> Vec<IftttWebhookConfig> {
    let mut seen = std::collections::HashSet::new();
    configs
        .into_iter()
        .filter(|config| {
            !config.event.is_empty() && !config.key.is_empty() && seen.insert(config.id.clone())
        })
        .collect()
}

fn dedupe_mqtt_configs(configs: Vec<MqttPublishConfig>) -> Vec<MqttPublishConfig> {
    let mut seen = std::collections::HashSet::new();
    configs
        .into_iter()
        .filter(|config| {
            !config.host.is_empty() && !config.topic.is_empty() && seen.insert(config.id.clone())
        })
        .collect()
}

fn sanitize_ifttt_event(event: &str) -> String {
    event
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>()
}

const FEED_TIMEOUT: Duration = Duration::from_secs(5);
static FEED_REFRESH: refresh::Refresher = refresh::Refresher::new();

fn collect_rss_feed(feed: &RssFeedConfig) -> Option<RssChannel> {
    rss_channel_from_cache(
        feed,
        &rss_cache_path(feed),
        unix_now_secs(),
        &FEED_REFRESH,
        fetch_text_url,
    )
}

/// Headlines from the on-disk cache. A cache older than the feed's refresh
/// interval is replaced by a background download; until that succeeds
/// (offline, feed down) the old headlines stay up.
fn rss_channel_from_cache<F>(
    feed: &RssFeedConfig,
    path: &Path,
    now: u64,
    refresher: &'static refresh::Refresher,
    fetch: F,
) -> Option<RssChannel>
where
    F: FnOnce(&str, Duration) -> Option<String> + Send + 'static,
{
    let cached = read_rss_cache(path, &feed.url);
    let fresh = cached
        .as_ref()
        .is_some_and(|(_, fetched_at)| now.saturating_sub(*fetched_at) <= feed.refresh_seconds);
    if !fresh {
        let url = feed.url.clone();
        let path = path.to_path_buf();
        let key = format!("{}\n{url}", path.display());
        refresher.start(&key, Instant::now(), move || {
            fetch(&url, FEED_TIMEOUT)
                .is_some_and(|body| write_rss_cache(&path, &url, &body).is_ok())
        });
    }
    parse_rss_channel(&cached?.0, feed.limit)
}

/// The cached body of the feed at `url` and when it was downloaded (unix
/// seconds), however old.
fn read_rss_cache(path: &Path, url: &str) -> Option<(String, u64)> {
    let text = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    // A feed whose address changed under the same id starts over.
    if value.get("url")?.as_str()? != url {
        return None;
    }
    let fetched_at = value.get("fetched_at")?.as_u64()?;
    Some((value.get("body")?.as_str()?.to_owned(), fetched_at))
}

fn write_rss_cache(path: &Path, url: &str, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_private_file(
        path,
        &serde_json::to_vec(&serde_json::json!({
            "fetched_at": unix_now_secs(),
            "url": url,
            "body": body,
        }))
        .unwrap_or_default(),
    )
}

fn fetch_text_url(url: &str, timeout: Duration) -> Option<String> {
    fetch_document(url, timeout).map(|(body, _)| body)
}

/// Whether `url` is an http(s) address (scheme in any case).
fn is_http_url(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

/// Downloads `url` and returns the decoded body plus the address it ended up
/// at after redirects (relative links in the page resolve against that).
/// Anything but http(s) is refused without running curl.
fn fetch_document(url: &str, timeout: Duration) -> Option<(String, String)> {
    if !is_http_url(url) {
        return None;
    }
    let max_time = timeout.as_secs().max(1).to_string();
    let output = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            // Feed addresses come from config and from Studio input; never
            // let them (or a redirect) reach file://, scp:// and friends.
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--max-time",
            &max_time,
            "--user-agent",
            concat!("linexus/", env!("CARGO_PKG_VERSION")),
            // stdout carries the body; the final URL goes to stderr, which
            // holds nothing else on success.
            "--write-out",
            "%{stderr}%{url_effective}",
            // An address can never be read as an option.
            "--",
            url,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let effective = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let effective = if effective.is_empty() {
        url.to_string()
    } else {
        effective
    };
    Some((feeds::decode_text(&output.stdout), effective))
}

/// Entity states are fetched in the background at most every few seconds; the
/// last answer stays up for a while when Home Assistant stops responding.
static HASS_STATES: refresh::ValueCache<serde_json::Value> = refresh::ValueCache::new();
const HASS_REFRESH: Duration = Duration::from_secs(5);
const HASS_MAX_STALE: Duration = Duration::from_secs(300);

fn home_assistant_state(entity: &HomeAssistantEntityConfig) -> Option<serde_json::Value> {
    // The token is part of the key, so a corrected token is tried right away
    // instead of after the old one's backoff.
    let key = format!(
        "{}\n{}\n{}",
        entity.base_url, entity.entity_id, entity.token
    );
    let entity = entity.clone();
    HASS_STATES.get(
        &key,
        Instant::now(),
        HASS_REFRESH,
        HASS_MAX_STALE,
        move || {
            providers::hass::fetch_entity_state_json(
                &entity.base_url,
                &entity.entity_id,
                &entity.token,
            )
        },
    )
}

pub fn parse_rss_channel(input: &str, limit: usize) -> Option<RssChannel> {
    let title = tag_text(input, "channel")
        .and_then(|channel| tag_text(&channel, "title"))
        .or_else(|| tag_text(input, "feed").and_then(|feed| tag_text(&feed, "title")))
        .or_else(|| tag_text(input, "title"));
    let mut items = Vec::new();
    for item in tag_blocks(input, "item")
        .into_iter()
        .chain(tag_blocks(input, "entry"))
        .take(limit)
    {
        items.push(RssItem {
            id: String::new(),
            seen: false,
            dismissed: false,
            title: tag_text(&item, "title"),
            link: tag_text(&item, "link").or_else(|| atom_link_href(&item)),
            summary: tag_text(&item, "description")
                .or_else(|| tag_text(&item, "summary"))
                .or_else(|| tag_text(&item, "content")),
        });
    }
    if title.is_none() && items.is_empty() {
        return None;
    }
    Some(RssChannel { title, items })
}

fn xml_summary_value(input: &str) -> serde_json::Value {
    if let Some(feed) = parse_rss_channel(input, 3) {
        if !feed.items.is_empty() {
            return serde_json::json!({
                "root": root_tag(input),
                "feed": feed,
            });
        }
    }
    serde_json::json!({
        "root": root_tag(input),
        "text": strip_xml_tags(input),
        "fields": simple_xml_fields(input),
    })
}

fn root_tag(input: &str) -> Option<String> {
    let start = input.find('<')? + 1;
    let rest = input[start..].trim_start();
    if rest.starts_with('?') || rest.starts_with('!') {
        return root_tag(&rest[1..]);
    }
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':'))
        .collect();
    (!name.is_empty()).then_some(name)
}

fn simple_xml_fields(input: &str) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    let Some(root) = root_tag(input) else {
        return serde_json::Value::Object(fields);
    };
    let Some(block) = tag_text(input, &root) else {
        return serde_json::Value::Object(fields);
    };
    for tag in child_tag_names(&block).into_iter().take(20) {
        if fields.contains_key(&tag) {
            continue;
        }
        if let Some(value) = tag_text(&block, &tag) {
            if !value.is_empty() && !value.contains('<') {
                fields.insert(tag, serde_json::Value::String(value));
            }
        }
    }
    serde_json::Value::Object(fields)
}

fn child_tag_names(input: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = input;
    while let Some(idx) = rest.find('<') {
        rest = &rest[idx + 1..];
        if rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('!') {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':'))
            .collect();
        if !name.is_empty() {
            tags.push(name);
        }
    }
    tags
}

fn tag_blocks(input: &str, tag: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = input;
    while let Some(start_tag) = find_open_tag(rest, tag) {
        let after_open = &rest[start_tag.1..];
        let close = format!("</{tag}>");
        let Some(end) = after_open.find(&close) else {
            break;
        };
        blocks.push(after_open[..end].to_string());
        rest = &after_open[end + close.len()..];
    }
    blocks
}

fn tag_text(input: &str, tag: &str) -> Option<String> {
    let block = tag_blocks(input, tag).into_iter().next()?;
    Some(xml_unescape(strip_cdata(block.trim())).trim().to_string())
}

fn find_open_tag(input: &str, tag: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    while let Some(pos) = input[offset..].find('<') {
        let start = offset + pos;
        let rest = &input[start + 1..];
        if rest.starts_with(tag)
            && rest
                .chars()
                .nth(tag.len())
                .is_some_and(|ch| ch == '>' || ch.is_whitespace() || ch == '/')
        {
            let end = input[start..].find('>')? + start + 1;
            return Some((start, end));
        }
        offset = start + 1;
    }
    None
}

fn atom_link_href(input: &str) -> Option<String> {
    let start = find_open_tag(input, "link")?.0;
    let tag_end = input[start..].find('>')? + start;
    let tag = &input[start..tag_end];
    let marker = "href=\"";
    let value_start = tag.find(marker)? + marker.len();
    let rest = &tag[value_start..];
    let end = rest.find('"')?;
    Some(xml_unescape(&rest[..end]))
}

fn strip_xml_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    xml_unescape(strip_cdata(&out).trim())
}

fn strip_cdata(input: &str) -> &str {
    input
        .strip_prefix("<![CDATA[")
        .and_then(|value| value.strip_suffix("]]>"))
        .unwrap_or(input)
}

fn xml_unescape(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Sensor and webhook ids as stored: lower-case ASCII letters, digits, `-`
/// and `_`; anything else becomes `-`.
pub fn sanitize_sensor_id(id: &str) -> String {
    let id: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "default".into()
    } else {
        id
    }
}

fn url_to_sensor_id(url: &str) -> String {
    sanitize_sensor_id(
        url.trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/'),
    )
}

pub fn sensor_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("NEXUS_SENSOR_CONFIG") {
        return PathBuf::from(path);
    }
    config_home().join("linexus").join("sensors.json")
}

fn webhook_state_dir() -> PathBuf {
    if let Ok(path) = std::env::var("NEXUS_SENSOR_WEBHOOK_DIR") {
        return PathBuf::from(path);
    }
    state_home().join("linexus").join("webhooks")
}

fn rss_cache_dir() -> PathBuf {
    state_home().join("linexus").join("rss-cache")
}

fn rss_cache_path(feed: &RssFeedConfig) -> PathBuf {
    rss_cache_dir().join(format!("{}.json", sanitize_sensor_id(&feed.id)))
}

fn rss_seen_state_path() -> PathBuf {
    if let Ok(path) = std::env::var("NEXUS_SENSOR_RSS_SEEN_PATH") {
        return PathBuf::from(path);
    }
    state_home().join("linexus").join("rss-seen.json")
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn state_home() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    idle: u64,
    total: u64,
}

impl CpuTimes {
    pub fn usage_since(self, previous: CpuTimes) -> Option<f32> {
        let total = self.total.checked_sub(previous.total)?;
        let idle = self.idle.checked_sub(previous.idle)?;
        if total == 0 {
            return None;
        }
        Some(((total - idle) as f32 / total as f32).clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone)]
pub struct CpuSampler {
    previous: CpuTimes,
}

impl CpuSampler {
    pub fn new() -> Option<Self> {
        Some(Self {
            previous: read_cpu_times_from(Path::new("/proc/stat"))?,
        })
    }

    pub fn sample(&mut self) -> Option<f32> {
        let current = read_cpu_times_from(Path::new("/proc/stat"))?;
        let usage = current.usage_since(self.previous);
        self.previous = current;
        usage
    }
}

pub fn collect_metrics() -> SystemMetrics {
    SensorHub::with_default_modules().collect().metrics
}

fn collect_core_metrics_into(metrics: &mut SystemMetrics) {
    let mut sampler = CpuSampler::new();
    let net_before = read_network_totals_from(Path::new("/proc/net/dev"));
    let disk_before = read_disk_io_totals_from(Path::new("/proc/diskstats"));
    let started = Instant::now();
    thread::sleep(Duration::from_millis(120));
    let elapsed = started.elapsed().as_secs_f32().max(0.001);
    let cpu_usage = sampler.as_mut().and_then(CpuSampler::sample);
    let net_after = read_network_totals_from(Path::new("/proc/net/dev"));
    let disk_after = read_disk_io_totals_from(Path::new("/proc/diskstats"));
    let network = net_before
        .zip(net_after)
        .and_then(|(before, after)| after.rate_since(before, elapsed));
    let disk_io = disk_before
        .zip(disk_after)
        .and_then(|(before, after)| after.rate_since(before, elapsed));
    let memory = read_memory_from(Path::new("/proc/meminfo"));
    let hwmon = discover_hwmon(Path::new("/sys/class/hwmon"));
    // amdgpu publishes load and VRAM in sysfs; rocm-smi is a Python program
    // costing ~0.2 s of CPU per call, so it is only a throttled fallback.
    let rocm = read_amdgpu_sysfs(Path::new("/sys/class/drm")).or_else(read_rocm_smi_throttled);
    let nvidia = providers::nvidia::read();
    let (volume_percent, volume_muted) = read_volume();
    let (media, mpris_players) = providers::media::current_media_and_players(None);

    metrics.cpu_usage = cpu_usage;
    metrics.ram_usage = memory.as_ref().and_then(|m| m.usage);
    metrics.ram_used_gib = memory.as_ref().map(|m| bytes_to_gib(m.used_bytes));
    metrics.ram_total_gib = memory.as_ref().map(|m| bytes_to_gib(m.total_bytes));
    metrics.cpu_temp_c = hwmon.cpu_temp_c;
    metrics.cpu_clock_mhz = read_cpu_clock_mhz(Path::new("/sys/devices/system/cpu"));
    metrics.gpu_usage = rocm.as_ref().and_then(|r| r.gpu_usage);
    metrics.vram_usage = rocm.as_ref().and_then(|r| r.vram_usage);
    metrics.gpu_temp_c = hwmon.gpu_temp_c;
    metrics.gpu_mem_temp_c = hwmon.gpu_mem_temp_c;
    metrics.gpu_power_w = hwmon.gpu_power_w;
    metrics.gpu_fan_rpm = hwmon.gpu_fan_rpm;
    if let Some(gpu) = nvidia.filter(|gpu| nvidia_is_main_gpu(gpu, rocm.as_ref())) {
        metrics.gpu_usage = gpu.usage;
        metrics.vram_usage = gpu.vram_usage();
        metrics.gpu_temp_c = gpu.temp_c;
        metrics.gpu_mem_temp_c = None;
        metrics.gpu_power_w = gpu.power_w;
        metrics.gpu_fan_rpm = None;
        metrics.gpu_fan_percent = gpu.fan_percent;
    }
    metrics.volume_percent = volume_percent;
    metrics.volume_muted = volume_muted;
    metrics.network_rx_bps = network.map(|net| net.rx_bps);
    metrics.network_tx_bps = network.map(|net| net.tx_bps);
    metrics.disk_read_bps = disk_io.map(|disk| disk.read_bps);
    metrics.disk_write_bps = disk_io.map(|disk| disk.write_bps);
    metrics.disk_root_usage = read_disk_usage_percent(Path::new("/"));
    let non_empty = |text: &str| (!text.is_empty()).then(|| text.to_string());
    metrics.media_player = media.as_ref().map(|m| m.bus_name.clone());
    metrics.media_title = media.as_ref().and_then(|m| non_empty(&m.title));
    metrics.media_artist = media.as_ref().and_then(|m| non_empty(&m.artist));
    metrics.media_url = None;
    metrics.media_pid = None;
    metrics.media_playback_status = media
        .as_ref()
        .map(|m| if m.playing { "Playing" } else { "Paused" }.to_string());
    metrics.mpris_players = mpris_players;
    metrics.media = media;
}

fn read_cpu_times_from(path: &Path) -> Option<CpuTimes> {
    parse_cpu_times(&fs::read_to_string(path).ok()?)
}

pub fn parse_cpu_times(input: &str) -> Option<CpuTimes> {
    let line = input.lines().find(|line| line.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|part| part.parse().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().sum();
    Some(CpuTimes { idle, total })
}

#[derive(Debug, Clone)]
struct MemoryInfo {
    total_bytes: u64,
    used_bytes: u64,
    usage: Option<f32>,
}

fn read_memory_from(path: &Path) -> Option<MemoryInfo> {
    parse_memory(&fs::read_to_string(path).ok()?)
}

fn parse_memory(input: &str) -> Option<MemoryInfo> {
    let mut total_kib = None;
    let mut available_kib = None;
    for line in input.lines() {
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "MemTotal:" => total_kib = parts.next().and_then(|v| v.parse::<u64>().ok()),
            "MemAvailable:" => available_kib = parts.next().and_then(|v| v.parse::<u64>().ok()),
            _ => {}
        }
    }
    let total = total_kib? * 1024;
    let available = available_kib? * 1024;
    let used = total.saturating_sub(available);
    Some(MemoryInfo {
        total_bytes: total,
        used_bytes: used,
        usage: (total > 0).then_some(used as f32 / total as f32),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkTotals {
    rx_bytes: u64,
    tx_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkRate {
    pub rx_bps: f32,
    pub tx_bps: f32,
}

impl NetworkTotals {
    pub fn rate_since(self, previous: NetworkTotals, seconds: f32) -> Option<NetworkRate> {
        Some(NetworkRate {
            rx_bps: self.rx_bytes.checked_sub(previous.rx_bytes)? as f32 / seconds,
            tx_bps: self.tx_bytes.checked_sub(previous.tx_bytes)? as f32 / seconds,
        })
    }
}

fn read_network_totals_from(path: &Path) -> Option<NetworkTotals> {
    let text = fs::read_to_string(path).ok()?;
    // Count only network hardware: VPN tunnels, bridges and container links
    // carry the same bytes again (a WireGuard tunnel doubles everything that
    // also crosses the physical NIC). Fall back to every interface on
    // systems where sysfs shows no device-backed one.
    let physical = |iface: &str| {
        Path::new("/sys/class/net")
            .join(iface)
            .join("device")
            .exists()
    };
    parse_proc_net_dev_where(&text, physical).or_else(|| parse_proc_net_dev(&text))
}

pub fn parse_proc_net_dev(input: &str) -> Option<NetworkTotals> {
    parse_proc_net_dev_where(input, |_| true)
}

/// Sums the counters of every non-loopback interface accepted by `keep`.
pub fn parse_proc_net_dev_where(input: &str, keep: impl Fn(&str) -> bool) -> Option<NetworkTotals> {
    let mut rx_bytes = 0_u64;
    let mut tx_bytes = 0_u64;
    let mut seen = false;
    for line in input.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let iface = name.trim();
        if iface == "lo" || iface.is_empty() || !keep(iface) {
            continue;
        }
        let fields: Vec<&str> = values.split_whitespace().collect();
        if fields.len() < 16 {
            continue;
        }
        rx_bytes = rx_bytes.saturating_add(fields[0].parse::<u64>().ok()?);
        tx_bytes = tx_bytes.saturating_add(fields[8].parse::<u64>().ok()?);
        seen = true;
    }
    seen.then_some(NetworkTotals { rx_bytes, tx_bytes })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskIoTotals {
    read_bytes: u64,
    write_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskIoRate {
    pub read_bps: f32,
    pub write_bps: f32,
}

impl DiskIoTotals {
    pub fn rate_since(self, previous: DiskIoTotals, seconds: f32) -> Option<DiskIoRate> {
        Some(DiskIoRate {
            read_bps: self.read_bytes.checked_sub(previous.read_bytes)? as f32 / seconds,
            write_bps: self.write_bytes.checked_sub(previous.write_bytes)? as f32 / seconds,
        })
    }
}

fn read_disk_io_totals_from(path: &Path) -> Option<DiskIoTotals> {
    parse_proc_diskstats(&fs::read_to_string(path).ok()?)
}

pub fn parse_proc_diskstats(input: &str) -> Option<DiskIoTotals> {
    let mut read_bytes = 0_u64;
    let mut write_bytes = 0_u64;
    let mut seen = false;
    for line in input.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 14 {
            continue;
        }
        let name = fields[2];
        if !is_whole_disk_device(name) {
            continue;
        }
        let sectors_read = fields[5].parse::<u64>().ok()?;
        let sectors_written = fields[9].parse::<u64>().ok()?;
        read_bytes = read_bytes.saturating_add(sectors_read.saturating_mul(512));
        write_bytes = write_bytes.saturating_add(sectors_written.saturating_mul(512));
        seen = true;
    }
    seen.then_some(DiskIoTotals {
        read_bytes,
        write_bytes,
    })
}

fn is_whole_disk_device(name: &str) -> bool {
    if name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("zram")
        || name.starts_with("dm-")
    {
        return false;
    }
    if name.starts_with("nvme") {
        return name.contains('n') && !name.contains('p');
    }
    if name.starts_with("mmcblk") {
        return !name.contains('p');
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some('s' | 'v'), Some('d')) => chars.all(|ch| ch.is_ascii_lowercase()),
        (Some('x'), Some('v')) if name.starts_with("xvd") => {
            name[3..].chars().all(|ch| ch.is_ascii_lowercase())
        }
        _ => false,
    }
}

fn read_disk_usage_percent(path: &Path) -> Option<f32> {
    let output = run_command_timeout(
        Path::new("df"),
        &["-P", path.to_str()?],
        Duration::from_millis(750),
    )?;
    if !output.status.success() {
        return None;
    }
    parse_df_percent(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_df_percent(input: &str) -> Option<f32> {
    let line = input.lines().nth(1)?;
    let percent = line
        .split_whitespace()
        .nth(4)?
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()?;
    Some((percent / 100.0).clamp(0.0, 1.0))
}

#[derive(Debug, Clone, Default)]
pub struct HwmonSummary {
    pub devices: Vec<HwmonDevice>,
    pub cpu_temp_c: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub gpu_mem_temp_c: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub gpu_fan_rpm: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct HwmonDevice {
    pub name: String,
    pub path: PathBuf,
}

pub fn discover_hwmon(root: &Path) -> HwmonSummary {
    let mut summary = HwmonSummary::default();
    let Ok(entries) = fs::read_dir(root) else {
        return summary;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = fs::read_to_string(path.join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".into());
        let device = HwmonDevice {
            name: name.clone(),
            path: path.clone(),
        };
        match name.as_str() {
            // AMD (k10temp, zenpower) and Intel (coretemp: package first).
            "k10temp" | "zenpower" | "coretemp" => {
                summary.cpu_temp_c = summary
                    .cpu_temp_c
                    .or_else(|| temp_with_label_or_first(&path, &["Tctl", "Tdie", "Package id 0"]))
            }
            "amdgpu" => {
                summary.gpu_temp_c = summary
                    .gpu_temp_c
                    .or_else(|| temp_with_label_or_first(&path, &["edge", "junction"]));
                summary.gpu_mem_temp_c = summary
                    .gpu_mem_temp_c
                    .or_else(|| temp_with_label_or_first(&path, &["mem", "memory"]));
                summary.gpu_power_w = summary.gpu_power_w.or_else(|| first_power_w(&path));
                summary.gpu_fan_rpm = summary.gpu_fan_rpm.or_else(|| first_fan_rpm(&path));
            }
            _ => {}
        }
        summary.devices.push(device);
    }
    summary
        .devices
        .sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    summary
}

fn first_temp_c(path: &Path) -> Option<f32> {
    numbered_input(path, "temp", "input").map(|milli| milli as f32 / 1000.0)
}

fn temp_with_label_or_first(path: &Path, preferred_labels: &[&str]) -> Option<f32> {
    for idx in 1..=20 {
        let label = fs::read_to_string(path.join(format!("temp{idx}_label"))).ok()?;
        if preferred_labels
            .iter()
            .any(|wanted| label.trim().eq_ignore_ascii_case(wanted))
        {
            let milli = read_number(path.join(format!("temp{idx}_input")))?;
            return Some(milli as f32 / 1000.0);
        }
    }
    first_temp_c(path)
}

fn first_power_w(path: &Path) -> Option<f32> {
    numbered_input(path, "power", "average")
        .or_else(|| numbered_input(path, "power", "input"))
        .map(|microwatts| microwatts as f32 / 1_000_000.0)
}

fn first_fan_rpm(path: &Path) -> Option<u32> {
    numbered_input(path, "fan", "input").and_then(|v| u32::try_from(v).ok())
}

#[derive(Debug, Clone, Default)]
struct RocmMetrics {
    gpu_usage: Option<f32>,
    vram_usage: Option<f32>,
    vram_total_bytes: Option<u64>,
}

/// With an AMD and an NVIDIA GPU, the `gpu.*` readings follow the one with
/// more video memory: usually the graphics card next to a processor's
/// integrated GPU.
fn nvidia_is_main_gpu(nvidia: &providers::nvidia::NvidiaGpu, amd: Option<&RocmMetrics>) -> bool {
    let Some(amd_total) = amd.and_then(|amd| amd.vram_total_bytes) else {
        return true;
    };
    let nvidia_total = nvidia.vram_total_mib.unwrap_or(0.0) as f64 * 1024.0 * 1024.0;
    nvidia_total >= amd_total as f64
}

/// GPU load and VRAM use of the first amdgpu card, straight from sysfs.
fn read_amdgpu_sysfs(drm: &Path) -> Option<RocmMetrics> {
    let mut cards: Vec<PathBuf> = fs::read_dir(drm)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card") && !n.contains('-'))
        })
        .collect();
    cards.sort();
    cards.into_iter().find_map(|card| {
        let device = card.join("device");
        let busy = read_number(device.join("gpu_busy_percent"))?;
        let used = read_number(device.join("mem_info_vram_used"));
        let total = read_number(device.join("mem_info_vram_total")).filter(|t| *t > 0);
        Some(RocmMetrics {
            gpu_usage: Some(percent_to_fraction(busy as f32)),
            vram_usage: used.zip(total).map(|(u, t)| u as f32 / t as f32),
            vram_total_bytes: total.and_then(|t| u64::try_from(t).ok()),
        })
    })
}

/// rocm-smi at most every 5 s; the last reading is reused in between.
fn read_rocm_smi_throttled() -> Option<RocmMetrics> {
    static LAST: std::sync::Mutex<Option<(Instant, Option<RocmMetrics>)>> =
        std::sync::Mutex::new(None);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, value)) = last.as_ref() {
        if at.elapsed() < Duration::from_secs(5) {
            return value.clone();
        }
    }
    let value = read_rocm_smi();
    *last = Some((Instant::now(), value.clone()));
    value
}

fn read_rocm_smi() -> Option<RocmMetrics> {
    let rocm_smi = find_rocm_smi()?;
    let output = run_command_timeout(
        &rocm_smi,
        &["--showuse", "--showmemuse", "--json"],
        Duration::from_millis(750),
    )?;
    if !output.status.success() {
        return None;
    }
    parse_rocm_smi_json(&String::from_utf8_lossy(&output.stdout))
}

fn parse_rocm_smi_json(input: &str) -> Option<RocmMetrics> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let first_card = value.as_object()?.values().next()?;
    let gpu_usage = json_percent_fraction(first_card.get("GPU use (%)"));
    let vram_usage = json_percent_fraction(first_card.get("GPU Memory Allocated (VRAM%)"));
    Some(RocmMetrics {
        gpu_usage,
        vram_usage,
        vram_total_bytes: None,
    })
}

fn json_percent_fraction(value: Option<&serde_json::Value>) -> Option<f32> {
    let text = match value? {
        serde_json::Value::String(text) => text.as_str(),
        serde_json::Value::Number(number) => {
            return number.as_f64().map(|v| percent_to_fraction(v as f32))
        }
        _ => return None,
    };
    let numeric = text
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())?
        .parse::<f32>()
        .ok()?;
    Some(percent_to_fraction(numeric))
}

fn percent_to_fraction(value: f32) -> f32 {
    (value / 100.0).clamp(0.0, 1.0)
}

fn find_rocm_smi() -> Option<PathBuf> {
    let explicit = PathBuf::from("/opt/rocm/bin/rocm-smi");
    if explicit.is_file() {
        return Some(explicit);
    }
    which("rocm-smi")
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run_command_timeout(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<std::process::Output> {
    providers::command::run_output(program, args, timeout).ok()
}

fn numbered_input(path: &Path, prefix: &str, suffix: &str) -> Option<i64> {
    for idx in 1..=20 {
        if let Some(value) = read_number(path.join(format!("{prefix}{idx}_{suffix}"))) {
            return Some(value);
        }
    }
    None
}

fn read_number(path: PathBuf) -> Option<i64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Level and mute state of the default output: through `wpctl` on
/// PipeWire, otherwise through PulseAudio's `pactl`.
fn read_volume() -> (Option<u8>, Option<bool>) {
    match read_pipewire_volume() {
        (None, _) => read_pulseaudio_volume(),
        found => found,
    }
}

/// Level and mute state of the default sink from one `wpctl` call.
fn read_pipewire_volume() -> (Option<u8>, Option<bool>) {
    let Some(output) = run_command_timeout(
        Path::new("wpctl"),
        &["get-volume", "@DEFAULT_AUDIO_SINK@"],
        Duration::from_millis(750),
    )
    .filter(|output| output.status.success()) else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&output.stdout);
    (parse_wpctl_volume(&text), parse_wpctl_muted(&text))
}

fn read_pulseaudio_volume() -> (Option<u8>, Option<bool>) {
    // pactl translates its output; "Mute: yes" needs the C locale.
    let pactl = |args: &[&str]| {
        let mut full = vec!["LC_ALL=C", "pactl"];
        full.extend_from_slice(args);
        run_command_timeout(Path::new("env"), &full, Duration::from_millis(750))
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let Some(volume) =
        pactl(&["get-sink-volume", "@DEFAULT_SINK@"]).and_then(|text| parse_pactl_volume(&text))
    else {
        return (None, None);
    };
    let muted =
        pactl(&["get-sink-mute", "@DEFAULT_SINK@"]).and_then(|text| parse_pactl_muted(&text));
    (Some(volume), muted)
}

/// `Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: …` → 40;
/// the loudest channel counts, and levels above 100 % are kept as with wpctl.
pub fn parse_pactl_volume(input: &str) -> Option<u8> {
    input
        .split_whitespace()
        .filter_map(|part| part.strip_suffix('%')?.parse::<u32>().ok())
        .max()
        .map(|percent| percent.min(u32::from(u8::MAX)) as u8)
}

/// `Mute: yes` → true, `Mute: no` → false.
pub fn parse_pactl_muted(input: &str) -> Option<bool> {
    match input.trim().strip_prefix("Mute:")?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// `Volume: 0.42` → 42. Levels above 100 % (PipeWire allows 150 % and more)
/// are kept so over-amplification shows; bars cap their own drawing.
pub fn parse_wpctl_volume(input: &str) -> Option<u8> {
    let volume = input
        .split_whitespace()
        .find_map(|part| part.parse::<f32>().ok())?;
    Some((volume * 100.0).round().clamp(0.0, f32::from(u8::MAX)) as u8)
}

/// `Volume: 0.60 [MUTED]` → true, `Volume: 0.60` → false; None when the
/// output carries no volume at all.
pub fn parse_wpctl_muted(input: &str) -> Option<bool> {
    parse_wpctl_volume(input)?;
    Some(input.contains("[MUTED]"))
}

pub fn parse_busctl_playback_status(input: &str) -> Option<String> {
    quoted_values(input)
        .into_iter()
        .find(|value| !value.is_empty())
}

pub fn parse_busctl_metadata(input: &str) -> (Option<String>, Option<String>) {
    let metadata = parse_busctl_metadata_info(input);
    (metadata.title, metadata.artist)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MprisMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub url: Option<String>,
    pub pid: Option<u32>,
}

pub fn parse_busctl_metadata_info(input: &str) -> MprisMetadata {
    MprisMetadata {
        title: metadata_value(input, "xesam:title"),
        artist: metadata_value(input, "xesam:artist"),
        url: metadata_value(input, "xesam:url"),
        pid: metadata_u32(input, "kde:pid"),
    }
}

fn metadata_u32(input: &str, key: &str) -> Option<u32> {
    let start = input.find(key)? + key.len();
    input[start..]
        .split(|ch: char| !(ch.is_ascii_digit()))
        .filter(|part| !part.is_empty())
        .find_map(|part| part.parse::<u32>().ok())
}

fn metadata_value(input: &str, key: &str) -> Option<String> {
    if let Some(value) = string_property_after(input, &format!(r#""{key}" s ""#)) {
        return Some(value);
    }
    if let Some(value) = string_array_property_after(input, &format!(r#""{key}" as "#)) {
        return Some(value);
    }
    let start = input.find(key)?;
    quoted_values(&input[start + key.len()..])
        .into_iter()
        .map(|value| value.trim().to_string())
        .find(|value| {
            !value.is_empty()
                && !value.starts_with('s')
                && !value.starts_with("as ")
                && !value.starts_with('"')
        })
}

fn string_property_after(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(unescape_busctl_string(&rest[..end]))
}

fn string_array_property_after(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    let quote = rest.find('"')? + 1;
    let value = &rest[quote..];
    let end = value.find('"')?;
    Some(unescape_busctl_string(&value[..end]))
}

fn unescape_busctl_string(input: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }

        let mut octal = String::new();
        while octal.len() < 3 {
            let Some(next) = chars.peek().copied() else {
                break;
            };
            if !matches!(next, '0'..='7') {
                break;
            }
            octal.push(next);
            chars.next();
        }
        if !octal.is_empty() {
            if let Ok(value) = u8::from_str_radix(&octal, 8) {
                bytes.push(value);
            }
            continue;
        }

        match chars.next() {
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('r') => bytes.push(b'\r'),
            Some('"') => bytes.push(b'"'),
            Some('\\') => bytes.push(b'\\'),
            Some(other) => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn quoted_values(input: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if in_quote && ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }
        if ch == '"' {
            if in_quote {
                values.push(unescape_busctl_string(&current));
                current.clear();
            }
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            current.push(ch);
        }
    }
    values
}

pub fn parse_mpris_players(input: &str) -> Vec<String> {
    input
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
        .map(ToOwned::to_owned)
        .collect()
}

/// `ps` at most every 5 s; its %CPU is a lifetime average that barely moves
/// from one second to the next.
fn collect_app_process_metrics(rules: &[AppRule]) -> Option<Vec<AppMetric>> {
    static LAST: Mutex<Option<(Instant, Option<Vec<AppMetric>>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, value)) = last.as_ref() {
        if at.elapsed() < Duration::from_secs(5) {
            return value.clone();
        }
    }
    let value = read_app_process_metrics(rules);
    *last = Some((Instant::now(), value.clone()));
    value
}

fn read_app_process_metrics(rules: &[AppRule]) -> Option<Vec<AppMetric>> {
    // Bounded: a process stuck in the kernel can block `ps` reading its
    // command line. The output (~150 bytes per process) stays far below the
    // 4 MiB capture cap even on browser-heavy desktops.
    let output = run_command_timeout(
        Path::new("ps"),
        &["-eo", "pid,ppid,comm,rss,%cpu,args"],
        Duration::from_secs(2),
    )?;
    if !output.status.success() {
        return None;
    }
    Some(parse_ps_app_metrics(
        &String::from_utf8_lossy(&output.stdout),
        rules,
    ))
}

fn parse_ps_app_metrics(input: &str, rules: &[AppRule]) -> Vec<AppMetric> {
    #[derive(Default)]
    struct Acc {
        rss_kib: u64,
        cpu_percent: f32,
        process_count: usize,
        top_pid: Option<u32>,
        top_command: Option<String>,
        top_rss_kib: u64,
    }

    let mut accumulators: Vec<Acc> = rules.iter().map(|_| Acc::default()).collect();
    for line in input.lines().skip(1) {
        let Some(row) = parse_ps_row(line) else {
            continue;
        };
        for (idx, rule) in rules.iter().enumerate() {
            if !rule.matches(&row.comm, &row.args) {
                continue;
            }
            let acc = &mut accumulators[idx];
            acc.rss_kib = acc.rss_kib.saturating_add(row.rss_kib);
            acc.cpu_percent += row.cpu_percent;
            acc.process_count += 1;
            if row.rss_kib >= acc.top_rss_kib {
                acc.top_rss_kib = row.rss_kib;
                acc.top_pid = Some(row.pid);
                acc.top_command = Some(short_command(&row.args));
            }
            break;
        }
    }

    let mut metrics: Vec<_> = rules
        .iter()
        .zip(accumulators)
        .filter(|(_, acc)| acc.process_count > 0)
        .map(|(rule, acc)| AppMetric {
            id: rule.id.into(),
            name: rule.name.into(),
            process_count: acc.process_count,
            rss_mb: acc.rss_kib as f32 / 1024.0,
            cpu_percent: acc.cpu_percent,
            top_pid: acc.top_pid,
            top_command: acc.top_command,
        })
        .collect();
    metrics.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.rss_mb
                    .partial_cmp(&a.rss_mb)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    metrics
}

#[derive(Debug, Clone)]
struct PsRow {
    pid: u32,
    comm: String,
    rss_kib: u64,
    cpu_percent: f32,
    args: String,
}

fn parse_ps_row(line: &str) -> Option<PsRow> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let _ppid = parts.next()?;
    let comm = parts.next()?.to_string();
    let rss_kib = parts.next()?.parse().ok()?;
    let cpu_percent = parts.next()?.parse().ok()?;
    let args = parts.collect::<Vec<_>>().join(" ");
    Some(PsRow {
        pid,
        comm,
        rss_kib,
        cpu_percent,
        args,
    })
}

fn short_command(args: &str) -> String {
    args.split_whitespace()
        .next()
        .unwrap_or(args)
        .rsplit('/')
        .next()
        .unwrap_or(args)
        .to_string()
}

fn bytes_to_gib(bytes: u64) -> f32 {
    bytes as f32 / 1_073_741_824.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static SENSOR_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn news_update_keeps_other_modules_and_refuses_broken_files() {
        let _guard = SENSOR_ENV_LOCK.lock().unwrap();
        let path =
            std::env::temp_dir().join(format!("linexus-sensors-news-{}.json", std::process::id()));
        let old = std::env::var_os("NEXUS_SENSOR_CONFIG");
        std::env::set_var("NEXUS_SENSOR_CONFIG", &path);

        fs::write(
            &path,
            r#"{"rss":[],"mqtt":[{"id":"broker","host":"10.0.0.2","topic":"nexus","password":"secret"}],
                "home_assistant_server":{"url":"http://ha.local:8123/","token":"ha-token"}}"#,
        )
        .unwrap();
        let feed = RssFeedConfig {
            id: "news".into(),
            url: "https://example.test/feed.xml".into(),
            limit: 3,
            refresh_seconds: 900,
            show_mode: default_show_mode(),
            priority: 0,
            max_age_seconds: None,
            tap_behavior: default_tap_behavior(),
        };
        let display = NewsDisplayConfig {
            scroll_speed_px_per_sec: 999,
            ..NewsDisplayConfig::default()
        };
        let saved = update_news_config(vec![feed], display).unwrap();
        assert_eq!(saved.rss.len(), 1);
        assert_eq!(saved.display.scroll_speed_px_per_sec, 150);
        let on_disk = load_sensor_config();
        assert_eq!(on_disk.mqtt[0].password, "secret");
        assert_eq!(on_disk.rss[0].id, "news");
        assert_eq!(
            on_disk.home_assistant_server,
            Some(HomeAssistantServer {
                url: "http://ha.local:8123".into(),
                token: "ha-token".into(),
            })
        );

        // Without a server the key stays out of the file entirely.
        save_sensor_config(SensorConfig::default()).unwrap();
        assert!(!fs::read_to_string(&path)
            .unwrap()
            .contains("home_assistant_server"));
        let mut blank = SensorConfig {
            home_assistant_server: Some(HomeAssistantServer {
                url: "  ".into(),
                token: "x".into(),
            }),
            ..SensorConfig::default()
        };
        normalize_sensor_config(&mut blank);
        assert_eq!(blank.home_assistant_server, None);

        fs::write(&path, "{ not json").unwrap();
        assert!(update_news_config(Vec::new(), NewsDisplayConfig::default()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");

        match old {
            Some(value) => std::env::set_var("NEXUS_SENSOR_CONFIG", value),
            None => std::env::remove_var("NEXUS_SENSOR_CONFIG"),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn parses_proc_stat_cpu_line() {
        let times = parse_cpu_times("cpu  100 0 50 850 0 0 0 0 0 0\n").unwrap();
        assert_eq!(times.total, 1000);
        assert_eq!(times.idle, 850);
    }

    #[test]
    fn computes_cpu_usage_delta() {
        let a = CpuTimes {
            idle: 850,
            total: 1000,
        };
        let b = CpuTimes {
            idle: 900,
            total: 1100,
        };
        assert_eq!(b.usage_since(a).unwrap(), 0.5);
    }

    #[test]
    fn parses_memory_info() {
        let mem = parse_memory("MemTotal:       1000 kB\nMemAvailable:    250 kB\n").unwrap();
        assert_eq!(mem.total_bytes, 1_024_000);
        assert_eq!(mem.used_bytes, 768_000);
        assert!((mem.usage.unwrap() - 0.75).abs() < 0.001);
    }

    #[test]
    fn parses_proc_net_dev_totals_without_loopback() {
        let input = "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n    lo: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\n  enp5s0: 1000 0 0 0 0 0 0 0 4000 0 0 0 0 0 0 0\n wlan0: 2000 0 0 0 0 0 0 0 8000 0 0 0 0 0 0 0\n";
        let totals = parse_proc_net_dev(input).unwrap();
        assert_eq!(totals.rx_bytes, 3000);
        assert_eq!(totals.tx_bytes, 12000);
        let rate = NetworkTotals {
            rx_bytes: 3600,
            tx_bytes: 13200,
        }
        .rate_since(totals, 2.0)
        .unwrap();
        assert_eq!(rate.rx_bps, 300.0);
        assert_eq!(rate.tx_bps, 600.0);
    }

    #[test]
    fn network_totals_can_skip_virtual_interfaces() {
        let input = "Inter-|   Receive\n face |bytes\n  eth0: 100 0 0 0 0 0 0 0 40 0 0 0 0 0 0 0\n  wg0: 90 0 0 0 0 0 0 0 30 0 0 0 0 0 0 0\n";
        let totals = parse_proc_net_dev_where(input, |iface| iface == "eth0").unwrap();
        assert_eq!((totals.rx_bytes, totals.tx_bytes), (100, 40));
        let all = parse_proc_net_dev(input).unwrap();
        assert_eq!(all.rx_bytes, 190);
    }

    #[test]
    fn parses_proc_diskstats_for_whole_disks_only() {
        let input = "259 0 nvme0n1 1 0 10 0 1 0 20 0 0 0 0 0 0 0 0\n259 1 nvme0n1p1 1 0 1000 0 1 0 1000 0 0 0 0 0 0 0 0\n8 0 sda 1 0 4 0 1 0 8 0 0 0 0 0 0 0 0\n7 0 loop0 1 0 1000 0 1 0 1000 0 0 0 0 0 0 0 0\n";
        let totals = parse_proc_diskstats(input).unwrap();
        assert_eq!(totals.read_bytes, (10 + 4) * 512);
        assert_eq!(totals.write_bytes, (20 + 8) * 512);
        let rate = DiskIoTotals {
            read_bytes: totals.read_bytes + 1024,
            write_bytes: totals.write_bytes + 2048,
        }
        .rate_since(totals, 2.0)
        .unwrap();
        assert_eq!(rate.read_bps, 512.0);
        assert_eq!(rate.write_bps, 1024.0);
    }

    #[test]
    fn parses_df_percent() {
        assert_eq!(parse_df_percent("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/nvme0n1p2 1000 420 580 42% /\n"), Some(0.42));
    }

    #[test]
    fn parses_wpctl_volume() {
        assert_eq!(parse_wpctl_volume("Volume: 0.42\n"), Some(42));
        assert_eq!(parse_wpctl_volume("Volume: 1.20 [MUTED]\n"), Some(120));
        assert_eq!(parse_wpctl_volume("Volume: 1.50\n"), Some(150));
        assert_eq!(parse_wpctl_volume("Volume: 9.00\n"), Some(255));
    }

    #[test]
    fn parses_pactl_volume_and_mute_state() {
        let stereo = "Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: 28180 /  43% / -21.99 dB\n        balance 0.07\n";
        assert_eq!(parse_pactl_volume(stereo), Some(43));
        assert_eq!(
            parse_pactl_volume("Volume: mono: 98304 / 150% / 10.57 dB\n"),
            Some(150)
        );
        assert_eq!(
            parse_pactl_volume("Connection failure: Connection refused\n"),
            None
        );
        assert_eq!(parse_pactl_muted("Mute: yes\n"), Some(true));
        assert_eq!(parse_pactl_muted("Mute: no\n"), Some(false));
        assert_eq!(parse_pactl_muted("Stumm: ja\n"), None);
    }

    #[test]
    fn parses_wpctl_mute_state() {
        assert_eq!(parse_wpctl_muted("Volume: 0.60 [MUTED]\n"), Some(true));
        assert_eq!(parse_wpctl_volume("Volume: 0.60 [MUTED]\n"), Some(60));
        assert_eq!(parse_wpctl_muted("Volume: 0.60\n"), Some(false));
        assert_eq!(parse_wpctl_muted("Could not connect to PipeWire\n"), None);
    }

    #[test]
    fn parses_busctl_mpris_metadata() {
        let input = r#"a{sv} 5 "mpris:length" x 989161000 "xesam:artist" as 1 "Creator" "xesam:title" s "All Machine Learning algorithms explained in 17 min - YouTube""#;
        let (title, artist) = parse_busctl_metadata(input);
        assert_eq!(
            title.as_deref(),
            Some("All Machine Learning algorithms explained in 17 min - YouTube")
        );
        assert_eq!(artist.as_deref(), Some("Creator"));
        assert_eq!(
            parse_busctl_playback_status(
                r#"s "Paused"
"#
            )
            .as_deref(),
            Some("Paused")
        );
    }

    #[test]
    fn parses_mpris_url_and_pid_metadata() {
        let input = r#"a{sv} 4 "xesam:title" s "F\303\274rchten Example" "xesam:artist" as 1 "Example Artist" "xesam:url" s "https://example.test/watch?v=1" "kde:pid" u 4242"#;
        let metadata = parse_busctl_metadata_info(input);
        assert_eq!(metadata.title.as_deref(), Some("Fürchten Example"));
        assert_eq!(metadata.artist.as_deref(), Some("Example Artist"));
        assert_eq!(
            metadata.url.as_deref(),
            Some("https://example.test/watch?v=1")
        );
        assert_eq!(metadata.pid, Some(4242));
    }

    #[test]
    fn parses_mpris_players() {
        let players = parse_mpris_players("org.mpris.MediaPlayer2.brave.instance123 123 brave user :1.2 - -\norg.foo.Bar 1 bar user :1.3 - -\n");
        assert_eq!(players, vec!["org.mpris.MediaPlayer2.brave.instance123"]);
    }

    #[test]
    fn parses_app_process_metrics_and_excludes_agent_browser() {
        let rules = [
            AppRule::new("brave", "Brave", &["brave"]),
            AppRule::new("konsole", "Konsole", &["konsole"]),
        ];
        let input = "\
PID PPID COMMAND RSS %CPU COMMAND
100 1 brave 204800 12.5 /usr/lib/brave/brave --type=browser
101 100 brave 102400 3.5 /usr/lib/brave/brave --type=renderer
102 1 brave 999999 99.0 /tmp/.agent-browser/brave --type=browser
200 1 konsole 51200 1.0 /usr/bin/konsole
";
        let metrics = parse_ps_app_metrics(input, &rules);
        assert_eq!(metrics.len(), 2);
        let brave = metrics.iter().find(|metric| metric.id == "brave").unwrap();
        assert_eq!(brave.process_count, 2);
        assert!((brave.rss_mb - 300.0).abs() < 0.001);
        assert!((brave.cpu_percent - 16.0).abs() < 0.001);
        assert_eq!(brave.top_pid, Some(100));
        assert_eq!(brave.top_command.as_deref(), Some("brave"));
    }

    #[test]
    fn sensor_hub_accepts_pluggable_modules() {
        struct DummySensor;

        impl SensorModule for DummySensor {
            fn id(&self) -> &'static str {
                "dummy"
            }

            fn collect(&self, snapshot: &mut SensorSnapshot) {
                snapshot.metrics.sensor_records.push(SensorRecord {
                    module: self.id().into(),
                    kind: "webhook".into(),
                    label: "build".into(),
                    value: serde_json::json!({"status": "ok"}),
                });
            }
        }

        let mut hub = SensorHub::new();
        hub.register(DummySensor);
        let snapshot = hub.collect();
        assert_eq!(snapshot.metrics.sensor_records.len(), 1);
        assert_eq!(snapshot.metrics.sensor_records[0].module, "dummy");
        assert_eq!(snapshot.metrics.sensor_records[0].kind, "webhook");
        assert_eq!(snapshot.metrics.sensor_records[0].value["status"], "ok");
    }

    #[test]
    fn parses_webhook_json_text_and_xml_payloads() {
        let (kind, json) = parse_sensor_payload(Some("application/json"), br#"{"ok":true}"#);
        assert_eq!(kind, "json");
        assert_eq!(json["ok"], true);

        let (kind, text) = parse_sensor_payload(Some("text/plain"), b"hello nexus");
        assert_eq!(kind, "text");
        assert_eq!(text["text"], "hello nexus");

        let (kind, xml) = parse_sensor_payload(
            Some("application/xml"),
            b"<sensor><status>green</status></sensor>",
        );
        assert_eq!(kind, "xml");
        assert_eq!(xml["root"], "sensor");
        assert_eq!(xml["fields"]["status"], "green");
    }

    #[test]
    fn sensor_record_extracts_display_value_and_primary_url() {
        let record = SensorRecord {
            module: "rss".into(),
            kind: "rss_feed".into(),
            label: "News".into(),
            value: serde_json::json!({
                "items": [{
                    "title": "Auffälligkeiten",
                    "link": "https://example.test/news"
                }]
            }),
        };
        assert_eq!(record.display_value(), "Auffälligkeiten");
        assert_eq!(
            record.primary_url().as_deref(),
            Some("https://example.test/news")
        );
    }

    #[test]
    fn sensor_cards_expand_rss_items_and_select_by_x() {
        let records = vec![SensorRecord {
            module: "rss".into(),
            kind: "rss_feed".into(),
            label: "News".into(),
            value: serde_json::json!({
                "items": [
                    {"title": "Erste Nachricht", "link": "https://example.test/1"},
                    {"title": "Zweite Nachricht", "link": "https://example.test/2"}
                ]
            }),
        }];

        let cards = sensor_cards(&records);

        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].source, "News");
        assert_eq!(cards[0].title, "Erste Nachricht");
        assert_eq!(cards[0].status.as_deref(), Some("NEW"));
        assert_eq!(cards[0].url.as_deref(), Some("https://example.test/1"));
        assert_eq!(
            sensor_card_for_x(&cards, 200)
                .expect("left card")
                .url
                .as_deref(),
            Some("https://example.test/1")
        );
        assert_eq!(
            sensor_card_for_x(&cards, 500)
                .expect("right card")
                .url
                .as_deref(),
            Some("https://example.test/2")
        );
    }

    #[test]
    fn sensor_cards_prioritize_unread_rss_items() {
        let records = vec![SensorRecord {
            module: "rss".into(),
            kind: "rss_feed".into(),
            label: "News".into(),
            value: serde_json::json!({
                "items": [
                    {"id": "seen-item", "title": "Schon gelesen", "link": "https://example.test/seen", "seen": true},
                    {"id": "dismissed-item", "title": "Weggewischt", "link": "https://example.test/dismissed", "seen": true, "dismissed": true},
                    {"id": "new-item", "title": "Neu", "link": "https://example.test/new", "seen": false}
                ]
            }),
        }];

        let cards = sensor_cards(&records);

        assert_eq!(cards[0].id, "new-item");
        assert_eq!(cards[0].status.as_deref(), Some("NEW"));
        assert_eq!(cards[1].id, "seen-item");
        assert_eq!(cards[1].status.as_deref(), Some("SEEN"));
        assert!(!cards.iter().any(|card| card.id == "dismissed-item"));
    }

    #[test]
    fn rotate_sensor_cards_cycles_visible_window() {
        let cards: Vec<_> = ["one", "two", "three"]
            .into_iter()
            .map(|title| SensorCard {
                id: title.into(),
                module: "rss".into(),
                kind: "rss_item".into(),
                source: "News".into(),
                title: title.into(),
                url: Some(format!("https://example.test/{title}")),
                status: Some("NEW".into()),
            })
            .collect();

        let first = rotate_sensor_cards(cards.clone(), 0, 2);
        let second = rotate_sensor_cards(cards.clone(), 12, 2);
        let wrapped = rotate_sensor_cards(cards, 24, 2);

        assert_eq!(
            first
                .iter()
                .map(|card| card.id.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(
            second
                .iter()
                .map(|card| card.id.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
        assert_eq!(
            wrapped
                .iter()
                .map(|card| card.id.as_str())
                .collect::<Vec<_>>(),
            vec!["three", "one"]
        );
        assert_eq!(
            sensor_display_card_for_x(&wrapped, 500)
                .expect("right card")
                .id,
            "one"
        );
    }

    #[test]
    fn rss_seen_state_persists_and_resets() {
        let _guard = SENSOR_ENV_LOCK.lock().unwrap();
        let path =
            std::env::temp_dir().join(format!("linexus-rss-seen-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let old_seen_path = std::env::var_os("NEXUS_SENSOR_RSS_SEEN_PATH");
        std::env::set_var("NEXUS_SENSOR_RSS_SEEN_PATH", &path);

        let id = rss_item_key("feed", "Titel", Some("https://example.test/item"));
        assert!(!load_rss_seen_state().seen.contains(&id));
        mark_rss_item_seen(&id).unwrap();
        assert!(load_rss_seen_state().seen.contains(&id));
        assert!(!load_rss_seen_state().dismissed.contains(&id));
        dismiss_rss_item(&id).unwrap();
        let state = load_rss_seen_state();
        assert!(state.seen.contains(&id));
        assert!(state.dismissed.contains(&id));
        reset_rss_seen_state().unwrap();
        assert!(!load_rss_seen_state().seen.contains(&id));
        assert!(!load_rss_seen_state().dismissed.contains(&id));

        if let Some(value) = old_seen_path {
            std::env::set_var("NEXUS_SENSOR_RSS_SEEN_PATH", value);
        } else {
            std::env::remove_var("NEXUS_SENSOR_RSS_SEEN_PATH");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sensor_cards_include_actionable_webhooks() {
        let records = vec![SensorRecord {
            module: "webhook".into(),
            kind: "json".into(),
            label: "Build".into(),
            value: serde_json::json!({
                "payload": {
                    "message": "Build grün",
                    "url": "https://ci.example.test/build/1"
                }
            }),
        }];

        let cards = sensor_cards(&records);

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Build grün");
        assert!(cards[0].is_actionable());
        assert_eq!(
            sensor_card_for_x(&cards, 500)
                .expect("single card")
                .url
                .as_deref(),
            Some("https://ci.example.test/build/1")
        );
    }

    #[test]
    fn normalizes_home_assistant_config_and_displays_state_card() {
        let mut config = SensorConfig {
            home_assistant: vec![HomeAssistantEntityConfig {
                id: "default".into(),
                base_url: "http://homeassistant.local:8123/".into(),
                entity_id: "sensor.living_room_temperature".into(),
                label: "".into(),
                token: "secret".into(),
                show_mode: "bad".into(),
                priority: 7,
                tap_behavior: "bad".into(),
            }],
            ..SensorConfig::default()
        };
        normalize_sensor_config(&mut config);
        let entity = &config.home_assistant[0];
        assert_eq!(entity.id, "sensor-living_room_temperature");
        assert_eq!(entity.base_url, "http://homeassistant.local:8123");
        assert_eq!(entity.label, "sensor.living_room_temperature");
        assert_eq!(entity.show_mode, "unread_first");
        assert_eq!(entity.tap_behavior, "open_url");

        let records = vec![SensorRecord {
            module: "home_assistant".into(),
            kind: "entity_state".into(),
            label: "Living room".into(),
            value: serde_json::json!({
                "state": "21.5",
                "unit": "°C",
                "url": "http://homeassistant.local:8123/lovelace"
            }),
        }];

        let cards = sensor_cards_with_config(&records, &config);
        assert_eq!(cards[0].title, "21.5°C");
        assert_eq!(
            cards[0].url.as_deref(),
            Some("http://homeassistant.local:8123/lovelace")
        );
    }

    #[test]
    fn normalizes_ifttt_and_renders_maker_webhook_request() {
        let mut config = SensorConfig {
            ifttt: vec![IftttWebhookConfig {
                id: " Phone Alert! ".into(),
                event: "nexus-alert !".into(),
                key: "maker-key".into(),
                value1_template: "{title}".into(),
                value2_template: "{source}:{status}".into(),
                value3_template: "{url}".into(),
            }],
            ..SensorConfig::default()
        };
        normalize_sensor_config(&mut config);
        let integration = &config.ifttt[0];
        assert_eq!(integration.id, "phone-alert");
        assert_eq!(integration.event, "nexusalert");

        let card = SensorCard {
            id: "rss-1".into(),
            module: "rss".into(),
            kind: "rss_item".into(),
            source: "heise".into(),
            title: "Neue Nachricht".into(),
            url: Some("https://example.test/item".into()),
            status: Some("NEW".into()),
        };
        let request = build_ifttt_request(integration, &card);
        assert_eq!(
            request.url,
            "https://maker.ifttt.com/trigger/nexusalert/with/key/maker-key"
        );
        assert_eq!(request.body["value1"], "Neue Nachricht");
        assert_eq!(request.body["value2"], "heise:NEW");
        assert_eq!(request.body["value3"], "https://example.test/item");
    }

    #[test]
    fn normalizes_mqtt_and_renders_publish_message() {
        let mut config = SensorConfig {
            mqtt: vec![MqttPublishConfig {
                id: " Local Broker ".into(),
                host: " localhost ".into(),
                port: 0,
                topic: "linexus/{module}/{id}".into(),
                client_id: "".into(),
                username: "".into(),
                password: "".into(),
                payload_template: r#"{"title":"{title}","url":"{url}"}"#.into(),
                retain: true,
                qos: 9,
                home_assistant_discovery: true,
                discovery_prefix: "homeassistant/".into(),
                discovery_object_id: "".into(),
                discovery_name: "".into(),
                discovery_device_id: "".into(),
                discovery_value_template: "".into(),
            }],
            ..SensorConfig::default()
        };
        normalize_sensor_config(&mut config);
        let integration = &config.mqtt[0];
        assert_eq!(integration.id, "local-broker");
        assert_eq!(integration.host, "localhost");
        assert_eq!(integration.port, 1);
        assert_eq!(integration.client_id, "linexus");
        assert_eq!(integration.qos, 2);
        assert_eq!(integration.discovery_prefix, "homeassistant");
        assert_eq!(integration.discovery_object_id, "local-broker");
        assert_eq!(integration.discovery_name, "Linexus local-broker");
        assert_eq!(integration.discovery_device_id, "linexus");

        let card = SensorCard {
            id: "rss-1".into(),
            module: "rss".into(),
            kind: "rss_item".into(),
            source: "heise".into(),
            title: "MQTT Nachricht".into(),
            url: Some("https://example.test/item".into()),
            status: Some("NEW".into()),
        };
        let message = build_mqtt_message(integration, &card);
        assert_eq!(message.host, "localhost");
        assert_eq!(message.port, 1);
        assert_eq!(message.topic, "linexus/rss/rss-1");
        assert_eq!(
            message.payload,
            r#"{"title":"MQTT Nachricht","url":"https://example.test/item"}"#
        );
        assert!(message.retain);
        assert_eq!(message.qos, 2);
        let discovery = build_home_assistant_mqtt_discovery(integration).expect("discovery");
        assert_eq!(
            discovery.topic,
            "homeassistant/sensor/linexus/local-broker/config"
        );
        assert!(discovery.retain);
        assert_eq!(discovery.payload["state_topic"], "linexus/{module}/{id}");
        assert_eq!(
            discovery.payload["value_template"],
            "{{ value_json.title }}"
        );
        assert_eq!(discovery.payload["device"]["name"], "Linexus");
    }

    #[test]
    fn parses_rss_and_atom_feeds() {
        let rss = r#"
<rss><channel><title>News</title>
<item><title>One &amp; Two</title><link>https://example.test/1</link><description><![CDATA[Summary]]></description></item>
</channel></rss>"#;
        let channel = parse_rss_channel(rss, 2).unwrap();
        assert_eq!(channel.title.as_deref(), Some("News"));
        assert_eq!(channel.items.len(), 1);
        assert_eq!(channel.items[0].title.as_deref(), Some("One & Two"));
        assert_eq!(
            channel.items[0].link.as_deref(),
            Some("https://example.test/1")
        );

        let atom = r#"<feed><title>Atom</title><entry><title>Entry</title><link href="https://example.test/a"/></entry></feed>"#;
        let channel = parse_rss_channel(atom, 1).unwrap();
        assert_eq!(channel.title.as_deref(), Some("Atom"));
        assert_eq!(
            channel.items[0].link.as_deref(),
            Some("https://example.test/a")
        );
    }

    #[test]
    fn the_gpu_with_more_video_memory_is_the_gpu() {
        let nvidia = providers::nvidia::NvidiaGpu {
            address: "0000:01:00.0".into(),
            usage: Some(0.5),
            vram_used_mib: None,
            vram_total_mib: Some(8192.0),
            temp_c: None,
            power_w: None,
            fan_percent: None,
        };
        let amd = |total: Option<u64>| RocmMetrics {
            vram_total_bytes: total,
            ..RocmMetrics::default()
        };
        // A processor's integrated GPU next to a graphics card.
        assert!(nvidia_is_main_gpu(&nvidia, Some(&amd(Some(512 << 20)))));
        assert!(!nvidia_is_main_gpu(&nvidia, Some(&amd(Some(24 << 30)))));
        assert!(nvidia_is_main_gpu(&nvidia, Some(&amd(None))));
        assert!(nvidia_is_main_gpu(&nvidia, None));
    }

    #[test]
    fn parses_rocm_smi_json_usage_fields() {
        let metrics = parse_rocm_smi_json(
            r#"{"card0":{"GPU use (%)":"42","GPU Memory Allocated (VRAM%)":"39"}}"#,
        )
        .unwrap();
        assert!((metrics.gpu_usage.unwrap() - 0.42).abs() < 0.001);
        assert!((metrics.vram_usage.unwrap() - 0.39).abs() < 0.001);
    }

    /// A fresh, empty directory below the system temp dir.
    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("linexus-sensors-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn webhook_record(label: &str, text: &str) -> SensorRecord {
        SensorRecord {
            module: "webhook".into(),
            kind: "text".into(),
            label: label.into(),
            value: serde_json::json!({ "id": label, "payload": { "text": text } }),
        }
    }

    #[test]
    fn webhook_records_survive_concurrent_deliveries() {
        let dir = test_dir("webhooks");
        let writers: Vec<_> = (0..8)
            .map(|writer| {
                let dir = dir.clone();
                thread::spawn(move || {
                    for round in 0..20 {
                        let text = format!("{writer}/{round} {}", "x".repeat(4096));
                        store_webhook_record(&dir, &webhook_record("door", &text)).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(file_names(&dir), ["door.json"]);
        let stored: SensorRecord =
            serde_json::from_str(&fs::read_to_string(dir.join("door.json")).unwrap()).unwrap();
        assert_eq!(stored.label, "door");
        let mode = fs::metadata(dir.join("door.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        // A write that fails (the target is a directory) leaves no temp file.
        fs::create_dir(dir.join("blocked.json")).unwrap();
        assert!(store_webhook_record(&dir, &webhook_record("blocked", "x")).is_err());
        assert_eq!(file_names(&dir), ["blocked.json", "door.json"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_webhook_bodies_are_refused_before_touching_disk() {
        let body = vec![b'x'; MAX_WEBHOOK_BODY_BYTES + 1];
        let error = record_webhook_payload("big", Some("text/plain"), &body).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn webhook_records_are_reread_only_when_the_directory_changes() {
        let dir = test_dir("webhook-cache");
        let cache = Mutex::new(None);
        let text = |records: &[SensorRecord], index: usize| {
            records[index].value["payload"]["text"]
                .as_str()
                .unwrap()
                .to_string()
        };
        store_webhook_record(&dir, &webhook_record("a", "one")).unwrap();
        // Just changed: read, but not remembered.
        assert_eq!(cached_webhook_records(&dir, &cache).len(), 1);
        assert!(cache.lock().unwrap().is_none());

        // Quiet for a minute: remembered, and a file rewritten in place
        // (which leaves the directory alone) is not read again.
        fs::File::open(&dir)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(60))
            .unwrap();
        assert_eq!(text(&cached_webhook_records(&dir, &cache), 0), "one");
        assert!(cache.lock().unwrap().is_some());
        fs::write(
            dir.join("a.json"),
            serde_json::to_vec(&webhook_record("a", "edited")).unwrap(),
        )
        .unwrap();
        assert_eq!(text(&cached_webhook_records(&dir, &cache), 0), "one");

        // A delivery renames a file into the directory and is seen at once.
        store_webhook_record(&dir, &webhook_record("b", "two")).unwrap();
        let records = cached_webhook_records(&dir, &cache);
        assert_eq!(records.len(), 2);
        assert_eq!(
            (text(&records, 0), text(&records, 1)),
            ("edited".into(), "two".into())
        );
        fs::remove_file(dir.join("b.json")).unwrap();
        assert_eq!(cached_webhook_records(&dir, &cache).len(), 1);
        let _ = fs::remove_dir_all(&dir);
        assert!(cached_webhook_records(&dir, &cache).is_empty());
    }

    #[test]
    fn sensor_config_is_written_private_and_loose_files_are_tightened() {
        let _guard = SENSOR_ENV_LOCK.lock().unwrap();
        let dir = test_dir("config-mode");
        let path = dir.join("sensors.json");
        let old = std::env::var_os("NEXUS_SENSOR_CONFIG");
        std::env::set_var("NEXUS_SENSOR_CONFIG", &path);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

        save_sensor_config(SensorConfig {
            home_assistant_server: Some(HomeAssistantServer {
                url: "http://ha.local:8123".into(),
                token: "secret".into(),
            }),
            ..SensorConfig::default()
        })
        .unwrap();
        assert_eq!(mode(&path), 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = load_sensor_config();
        assert_eq!(loaded.home_assistant_server.unwrap().token, "secret");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(file_names(&dir), ["sensors.json"]);

        match old {
            Some(value) => std::env::set_var("NEXUS_SENSOR_CONFIG", value),
            None => std::env::remove_var("NEXUS_SENSOR_CONFIG"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_feed_cache_stays_up_while_a_background_refresh_runs_or_fails() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::{mpsc, Arc};

        let dir = test_dir("rss-cache");
        let path = dir.join("news.json");
        let feed = RssFeedConfig {
            id: "news".into(),
            url: "https://example.test/feed.xml".into(),
            limit: 3,
            refresh_seconds: 900,
            show_mode: default_show_mode(),
            priority: 0,
            max_age_seconds: None,
            tap_behavior: default_tap_behavior(),
        };
        let refresher: &'static refresh::Refresher = Box::leak(Box::new(refresh::Refresher::new()));
        let key = format!("{}\n{}", path.display(), feed.url);
        let headline = |channel: Option<RssChannel>| channel.unwrap().items[0].title.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let offline = move |_: &str, _: Duration| {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        };

        // Nothing cached: nothing shown, and the download does not block.
        let (release, gate) = mpsc::channel::<Option<String>>();
        let started = Instant::now();
        let now = unix_now_secs();
        let first = rss_channel_from_cache(&feed, &path, now, refresher, move |url, timeout| {
            assert_eq!(
                (url, timeout),
                ("https://example.test/feed.xml", FEED_TIMEOUT)
            );
            gate.recv().unwrap()
        });
        assert!(first.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "collection waited"
        );
        release
            .send(Some(
                "<rss><channel><title>News</title><item><title>Old</title></item></channel></rss>"
                    .into(),
            ))
            .unwrap();
        refresh::wait_until_idle(refresher, &key);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Fresh: served without a download.
        let fresh = rss_channel_from_cache(&feed, &path, now, refresher, offline.clone());
        assert_eq!(headline(fresh).as_deref(), Some("Old"));
        refresh::wait_until_idle(refresher, &key);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // Expired and offline: the old headlines stay while the download
        // runs and after it failed; the next attempt waits for the backoff.
        let later = now + 1000;
        let (release, gate) = mpsc::channel::<Option<String>>();
        let started = Instant::now();
        let stale = rss_channel_from_cache(&feed, &path, later, refresher, move |_, _| {
            gate.recv().unwrap()
        });
        assert_eq!(headline(stale).as_deref(), Some("Old"));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "collection waited"
        );
        release.send(None).unwrap();
        refresh::wait_until_idle(refresher, &key);
        let stale = rss_channel_from_cache(&feed, &path, later, refresher, offline.clone());
        assert_eq!(headline(stale).as_deref(), Some("Old"));
        refresh::wait_until_idle(refresher, &key);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "retried during the backoff"
        );

        // A new address under the same id does not show the old feed.
        let moved = RssFeedConfig {
            url: "https://example.test/other.xml".into(),
            ..feed.clone()
        };
        assert!(rss_channel_from_cache(&moved, &path, now, refresher, offline).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn feed_downloads_refuse_other_schemes_without_running_curl() {
        assert!(is_http_url("https://example.test/feed"));
        assert!(is_http_url("HTTP://example.test/feed"));
        assert!(!is_http_url("file:///etc/passwd"));
        assert!(!is_http_url("-K/tmp/curlrc"));
        assert!(!is_http_url("example.test/feed"));
        assert!(fetch_document("-K/tmp/curlrc", Duration::from_secs(1)).is_none());
        assert!(fetch_document("file:///etc/passwd", Duration::from_secs(1)).is_none());
    }
}
