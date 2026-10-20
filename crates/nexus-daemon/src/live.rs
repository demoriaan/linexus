//! Gathers everything a frame shows besides configuration into
//! `nexus_render::LiveData`.
//!
//! Fast readings come from the metrics worker every second. Slow or remote
//! data (weather, Home Assistant, album art, application icons, command
//! widgets) comes from a second worker that only fetches what the active
//! profile actually shows, each on its own schedule.

use crate::metrics_cache::MetricsSnapshot;
use nexus_config::{Profile, WidgetKind};
use nexus_render::tiny_skia::Pixmap;
use nexus_render::{EntityInfo, LiveData, LocalTime, Metric, Rgb, Unit, WeatherInfo};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Samples kept per metric for sparklines (one per second).
const HISTORY: usize = 60;
const WEATHER_EVERY: Duration = Duration::from_secs(15 * 60);
const WEATHER_RETRY: Duration = Duration::from_secs(60);
const ENTITIES_EVERY: Duration = Duration::from_secs(5);

pub(crate) fn local_time() -> LocalTime {
    let c = nexus_sensors::local_clock();
    LocalTime {
        year: c.year,
        month: c.month,
        day: c.day,
        weekday: c.weekday,
        hour: c.hour,
        minute: c.minute,
        second: c.second,
    }
}

fn unit(unit: nexus_sensors::MetricUnit) -> Unit {
    use nexus_sensors::MetricUnit as M;
    match unit {
        M::Percent => Unit::Percent,
        M::Celsius => Unit::Celsius,
        M::Watts => Unit::Watts,
        M::Rpm => Unit::Rpm,
        M::Bytes => Unit::Bytes,
        M::BytesPerSec => Unit::BytesPerSec,
        M::Megahertz => Unit::Megahertz,
        M::Plain => Unit::Plain,
    }
}

/// Keeps metric history between frames and assembles `LiveData`.
#[derive(Default)]
pub(crate) struct LiveBuilder {
    history: HashMap<String, VecDeque<f32>>,
    last_snapshot: Option<Arc<MetricsSnapshot>>,
    metrics: HashMap<String, Metric>,
}

impl LiveBuilder {
    /// Folds a new metrics snapshot into the history (once per snapshot).
    pub(crate) fn absorb(&mut self, snapshot: &Arc<MetricsSnapshot>) {
        if self
            .last_snapshot
            .as_ref()
            .is_some_and(|last| Arc::ptr_eq(last, snapshot))
        {
            return;
        }
        self.last_snapshot = Some(snapshot.clone());
        self.metrics.clear();
        for reading in &snapshot.catalog {
            let history = self.history.entry(reading.id.clone()).or_default();
            history.push_back(reading.value);
            while history.len() > HISTORY {
                history.pop_front();
            }
            self.metrics.insert(
                reading.id.clone(),
                Metric {
                    value: reading.value,
                    unit: unit(reading.unit),
                    label: reading.label.clone(),
                    max: reading.max,
                },
            );
        }
    }

    pub(crate) fn build(&self, settings: &nexus_config::Settings, provided: &Provided) -> LiveData {
        let mut live = LiveData {
            time: local_time(),
            locale: nexus_render::Locale::resolve(&settings.locale),
            hour12: settings.hour12,
            fahrenheit: settings.temperature_unit == nexus_config::TemperatureUnit::Fahrenheit,
            metrics: self.metrics.clone(),
            history: self
                .history
                .iter()
                .map(|(id, values)| (id.clone(), values.iter().copied().collect()))
                .collect(),
            weather: provided.weather.clone(),
            entities: provided.entities.clone(),
            commands: provided.commands.clone(),
            images: provided.icons.clone(),
            ..LiveData::default()
        };
        if let Some(snapshot) = &self.last_snapshot {
            live.running = snapshot.running.clone();
            if let Some(volume) = snapshot.metrics.volume_percent {
                live.volume = Some(nexus_render::VolumeInfo {
                    percent: volume,
                    muted: snapshot.volume_muted,
                });
            }
            if let Some(media) = &snapshot.media {
                let art = provided
                    .art
                    .as_ref()
                    .filter(|(url, ..)| Some(url) == media.art_url.as_ref());
                live.media = Some(nexus_render::MediaInfo {
                    player: media.identity.clone(),
                    title: media.title.clone(),
                    artist: media.artist.clone(),
                    album: media.album.clone(),
                    playing: media.playing,
                    position_s: media.position_s,
                    length_s: media.length_s,
                    art: art.map(|(_, image, _)| image.clone()),
                    art_color: art.map(|(.., color)| *color),
                });
            }
        }
        live
    }
}

/// What the active profile needs from slow providers.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Wants {
    /// (key, latitude, longitude); the key matches `weather_key`.
    pub(crate) weather: Vec<(String, f64, f64)>,
    pub(crate) entities: bool,
    /// (widget id, program, args, interval)
    pub(crate) commands: Vec<(String, String, Vec<String>, Duration)>,
    pub(crate) art_url: Option<String>,
    /// `app:<desktop id>` references.
    pub(crate) app_icons: Vec<String>,
}

impl Wants {
    pub(crate) fn for_profile(
        profile: &Profile,
        settings: &nexus_config::Settings,
        art_url: Option<String>,
        commands_allowed: bool,
    ) -> Self {
        let mut wants = Wants {
            art_url,
            ..Wants::default()
        };
        for widget in profile.pages.iter().flat_map(|page| &page.widgets) {
            match &widget.kind {
                WidgetKind::Weather(props) => {
                    let location = props.location.as_ref().or(settings.weather.as_ref());
                    if let Some(location) = location {
                        let key = nexus_render::weather_key(props);
                        if !wants.weather.iter().any(|(k, ..)| *k == key) {
                            wants
                                .weather
                                .push((key, location.latitude, location.longitude));
                        }
                    }
                }
                WidgetKind::Entity(_) => wants.entities = true,
                WidgetKind::Command(props) if commands_allowed && !props.program.is_empty() => {
                    wants.commands.push((
                        widget.id.clone(),
                        props.program.clone(),
                        props.args.clone(),
                        Duration::from_secs(u64::from(props.interval_seconds.max(2))),
                    ));
                }
                WidgetKind::Button(props) => {
                    if let Some(icon) = props.icon.as_ref().filter(|i| i.starts_with("app:")) {
                        wants.app_icons.push(icon.clone());
                    }
                }
                _ => {}
            }
        }
        wants
    }
}

/// Latest results of the slow providers.
#[derive(Debug, Clone, Default)]
pub(crate) struct Provided {
    pub(crate) weather: HashMap<String, WeatherInfo>,
    pub(crate) entities: HashMap<String, EntityInfo>,
    pub(crate) commands: HashMap<String, String>,
    /// (art URL, image, dominant colour)
    pub(crate) art: Option<(String, Arc<Pixmap>, Rgb)>,
    pub(crate) icons: HashMap<String, Arc<Pixmap>>,
}

#[derive(Default)]
struct Shared {
    wants: Mutex<Wants>,
    provided: Mutex<Provided>,
    wake: Condvar,
    control: Mutex<(bool /*stop*/, bool /*refresh entities*/)>,
}

#[derive(Clone, Default)]
pub(crate) struct ProviderHandle {
    shared: Arc<Shared>,
}

pub(crate) struct ProviderWorker {
    shared: Arc<Shared>,
}

impl Drop for ProviderWorker {
    fn drop(&mut self) {
        self.shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0 = true;
        self.shared.wake.notify_all();
    }
}

impl ProviderHandle {
    pub(crate) fn set_wants(&self, wants: Wants) {
        let mut current = self
            .shared
            .wants
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *current != wants {
            *current = wants;
            self.shared.wake.notify_all();
        }
    }

    pub(crate) fn provided(&self) -> Provided {
        self.shared
            .provided
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Optimistically flips an entity after a toggle and refetches soon.
    pub(crate) fn toggled(&self, entity_id: &str) {
        if let Some(entity) = self
            .shared
            .provided
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entities
            .get_mut(entity_id)
        {
            entity.on = entity.on.map(|on| !on);
        }
        self.shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1 = true;
        self.shared.wake.notify_all();
    }

    pub(crate) fn spawn(&self) -> ProviderWorker {
        let shared = self.shared.clone();
        thread::spawn(move || run_worker(&shared));
        ProviderWorker {
            shared: self.shared.clone(),
        }
    }
}

fn run_worker(shared: &Shared) {
    let mut weather_due: HashMap<String, Instant> = HashMap::new();
    let mut entities_due = Instant::now();
    let mut commands_due: HashMap<String, Instant> = HashMap::new();
    let mut art_failed: HashSet<String> = HashSet::new();
    let mut icons_failed: HashSet<String> = HashSet::new();
    loop {
        let (stop, refresh) = {
            let mut control = shared
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let state = *control;
            control.1 = false;
            state
        };
        if stop {
            return;
        }
        if refresh {
            // Give Home Assistant a moment to apply the change first.
            thread::sleep(Duration::from_millis(600));
            entities_due = Instant::now();
        }
        let wants = shared
            .wants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let now = Instant::now();
        let update = |f: &mut dyn FnMut(&mut Provided)| {
            f(&mut shared
                .provided
                .lock()
                .unwrap_or_else(PoisonError::into_inner));
        };

        for (key, lat, lon) in &wants.weather {
            if weather_due.get(key).is_some_and(|due| now < *due) {
                continue;
            }
            match nexus_sensors::fetch_weather(*lat, *lon) {
                Ok(report) => {
                    let info = WeatherInfo {
                        location: if key.is_empty() {
                            String::new()
                        } else {
                            key.clone()
                        },
                        temp_c: report.temp_c,
                        code: report.code,
                        is_day: report.is_day,
                        high_c: report.high_c,
                        low_c: report.low_c,
                    };
                    update(&mut |p| {
                        p.weather.insert(key.clone(), info.clone());
                    });
                    weather_due.insert(key.clone(), now + WEATHER_EVERY);
                }
                Err(_) => {
                    weather_due.insert(key.clone(), now + WEATHER_RETRY);
                }
            }
        }

        if wants.entities && now >= entities_due {
            entities_due = now + ENTITIES_EVERY;
            let config = nexus_sensors::load_sensor_config();
            if let Some(server) = config.home_assistant_server {
                if let Ok(states) = nexus_sensors::fetch_states(&server) {
                    let entities: HashMap<String, EntityInfo> = states
                        .into_iter()
                        .map(|state| {
                            let info = EntityInfo {
                                name: state.friendly_name.clone().unwrap_or_default(),
                                on: nexus_sensors::entity_is_on(&state.state),
                                unit: state.unit.clone(),
                                state: state.state,
                            };
                            (state.entity_id, info)
                        })
                        .collect();
                    update(&mut |p| p.entities = entities.clone());
                }
            }
        }

        for (id, program, args, every) in &wants.commands {
            if commands_due.get(id).is_some_and(|due| now < *due) {
                continue;
            }
            commands_due.insert(id.clone(), now + *every);
            let output = nexus_sensors::run_command_capture(program, args, Duration::from_secs(5))
                .unwrap_or_else(|error| format!("! {error}"));
            update(&mut |p| {
                p.commands.insert(id.clone(), output.clone());
            });
        }

        if let Some(url) = &wants.art_url {
            let have = shared
                .provided
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .art
                .as_ref()
                .is_some_and(|(cached, ..)| cached == url);
            if !have && !art_failed.contains(url) {
                match nexus_sensors::fetch_art_bytes(url)
                    .and_then(|bytes| nexus_render::icons::decode_image(&bytes, 96))
                {
                    Some(image) => {
                        let color = nexus_render::icons::dominant_color(&image);
                        let art = (url.clone(), Arc::new(image), color);
                        update(&mut |p| p.art = Some(art.clone()));
                    }
                    None => {
                        art_failed.insert(url.clone());
                    }
                }
            }
        }

        let missing: Vec<String> = {
            let provided = shared
                .provided
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            wants
                .app_icons
                .iter()
                .filter(|icon| !provided.icons.contains_key(*icon) && !icons_failed.contains(*icon))
                .cloned()
                .collect()
        };
        if !missing.is_empty() {
            let apps = nexus_sensors::list_applications();
            for reference in missing {
                let id = reference.trim_start_matches("app:");
                let image = apps
                    .iter()
                    .find(|app| app.id == id || app.id.trim_end_matches(".desktop") == id)
                    .and_then(|app| app.icon.as_deref())
                    .and_then(|icon| nexus_sensors::resolve_icon(icon, 48))
                    .and_then(|path| std::fs::read(path).ok())
                    .and_then(|bytes| nexus_render::icons::decode_image(&bytes, 48));
                match image {
                    Some(image) => update(&mut |p| {
                        p.icons.insert(reference.clone(), Arc::new(image.clone()));
                    }),
                    None => {
                        icons_failed.insert(reference);
                    }
                }
            }
        }

        let mut next = Instant::now() + Duration::from_secs(1);
        for due in weather_due.values().chain(commands_due.values()) {
            next = next.min(*due);
        }
        let control = shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if control.0 || control.1 {
            continue;
        }
        let wait = next
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(200));
        let _ = shared.wake.wait_timeout(control, wait);
    }
}

/// The metric catalog for the API. Read from the collector rather than from
/// rendered frames, so it is complete while the strip is off.
pub(crate) fn metric_readings(snapshot: &MetricsSnapshot) -> Vec<nexus_ipc::MetricReading> {
    let mut readings: Vec<_> = snapshot
        .catalog
        .iter()
        .map(|reading| nexus_ipc::MetricReading {
            id: reading.id.clone(),
            label: reading.label.clone(),
            value: reading.value,
            unit: serde_json::to_value(reading.unit)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            max: reading.max,
        })
        .collect();
    readings.sort_by(|a, b| a.id.cmp(&b.id));
    readings
}
