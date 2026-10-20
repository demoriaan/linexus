//! Background sensor collection.
//!
//! `nexus_sensors::collect_metrics()` samples CPU over 120 ms and spawns
//! helpers (ps, rocm-smi, busctl, wpctl), roughly half a second in total. Run
//! inline for every frame it capped the sensors page at ~2 fps, made the
//! headline marquee jump ~25 px at a time, and stalled touch processing long
//! enough to stretch quick taps into holds. A worker thread refreshes a shared
//! snapshot instead; frames and taps read the latest one without blocking.

use std::panic;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

#[derive(Debug, Default)]
pub(crate) struct MetricsSnapshot {
    pub(crate) metrics: nexus_sensors::SystemMetrics,
    /// Every reading under its stable metric id (`cpu.usage`, `hwmon:…`).
    pub(crate) catalog: Vec<nexus_sensors::MetricReading>,
    pub(crate) media: Option<nexus_sensors::MediaState>,
    pub(crate) volume_muted: bool,
    /// Launch targets of running applications (running-indicator dots).
    pub(crate) running: std::collections::HashSet<String>,
    /// Configured news / webhook / Home Assistant cards in display order.
    pub(crate) cards: Vec<nexus_sensors::SensorCard>,
    /// Headline pacing and touch timing from the sensor config (Studio).
    pub(crate) display: nexus_sensors::NewsDisplayConfig,
}

/// Installed applications change rarely; rescanning desktop files every
/// second would be wasted work.
const APPS_REFRESH: Duration = Duration::from_secs(60);

fn installed_apps() -> Arc<Vec<nexus_sensors::AppEntry>> {
    static CACHE: Mutex<Option<(std::time::Instant, Arc<Vec<nexus_sensors::AppEntry>>)>> =
        Mutex::new(None);
    let mut cache = lock(&CACHE);
    match &*cache {
        Some((at, apps)) if at.elapsed() < APPS_REFRESH => apps.clone(),
        _ => {
            let apps = Arc::new(nexus_sensors::list_applications());
            *cache = Some((std::time::Instant::now(), apps.clone()));
            apps
        }
    }
}

/// Values that change slowly but cost real work to read (some hwmon chips
/// answer each sysfs read in milliseconds; running apps need a /proc scan).
const SLOW_REFRESH: Duration = Duration::from_secs(5);

fn cached<T: Clone>(
    slot: &Mutex<Option<(std::time::Instant, T)>>,
    max_age: Duration,
    read: impl FnOnce() -> T,
) -> T {
    let mut slot = lock(slot);
    match &*slot {
        Some((at, value)) if at.elapsed() < max_age => value.clone(),
        _ => {
            let value = read();
            *slot = Some((std::time::Instant::now(), value.clone()));
            value
        }
    }
}

fn hwmon_readings() -> Vec<nexus_sensors::HwmonReading> {
    static CACHE: Mutex<Option<(std::time::Instant, Vec<nexus_sensors::HwmonReading>)>> =
        Mutex::new(None);
    cached(&CACHE, SLOW_REFRESH, || {
        nexus_sensors::read_hwmon(std::path::Path::new("/sys/class/hwmon"))
    })
}

/// Battery levels move slowly and UPower is a process spawn away.
fn batteries() -> Vec<nexus_sensors::MetricReading> {
    static CACHE: Mutex<Option<(std::time::Instant, Vec<nexus_sensors::MetricReading>)>> =
        Mutex::new(None);
    cached(&CACHE, Duration::from_secs(30), || {
        nexus_sensors::read_batteries()
            .iter()
            .map(|b| b.metric())
            .collect()
    })
}

fn running_apps() -> std::collections::HashSet<String> {
    static CACHE: Mutex<Option<(std::time::Instant, std::collections::HashSet<String>)>> =
        Mutex::new(None);
    cached(&CACHE, SLOW_REFRESH, || {
        nexus_sensors::running_targets(&installed_apps())
    })
}

impl MetricsSnapshot {
    fn collect() -> Self {
        let metrics = nexus_sensors::collect_metrics();
        let config = nexus_sensors::load_sensor_config();
        let cards = nexus_sensors::sensor_cards_with_config(&metrics.sensor_records, &config)
            .into_iter()
            .filter(|card| matches!(card.module.as_str(), "rss" | "webhook" | "home_assistant"))
            .collect();
        let mut catalog = nexus_sensors::metric_catalog(&metrics, &hwmon_readings());
        catalog.extend(batteries());
        let running = running_apps();
        Self {
            catalog,
            media: metrics.media.clone(),
            volume_muted: metrics.volume_muted.unwrap_or(false),
            running,
            metrics,
            cards,
            display: config.display,
        }
    }
}

#[derive(Debug, Default)]
struct Latest {
    /// Bumped by every local card edit, so a collection that started before
    /// the edit (and may have read the old read/dismissed state from disk) is
    /// discarded instead of undoing it.
    edits: u64,
    snapshot: Arc<MetricsSnapshot>,
}

#[derive(Debug, Default)]
struct Control {
    refresh: bool,
    paused: bool,
    stop: bool,
}

#[derive(Debug, Default)]
struct Shared {
    latest: Mutex<Latest>,
    control: Mutex<Control>,
    wake: Condvar,
}

/// Cheap, clonable handle to the latest snapshot. Without a running worker
/// it only changes through [`MetricsHandle::refresh_blocking`].
#[derive(Debug, Clone, Default)]
pub(crate) struct MetricsHandle {
    shared: Arc<Shared>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MetricsHandle {
    pub(crate) fn latest(&self) -> Arc<MetricsSnapshot> {
        lock(&self.shared.latest).snapshot.clone()
    }

    /// Collects on the calling thread (one-shot renders and tests).
    pub(crate) fn refresh_blocking(&self) {
        lock(&self.shared.latest).snapshot = Arc::new(MetricsSnapshot::collect());
    }

    #[cfg(test)]
    pub(crate) fn set_media(&self, media: Option<nexus_sensors::MediaState>) {
        lock(&self.shared.latest).snapshot = Arc::new(MetricsSnapshot {
            media,
            ..MetricsSnapshot::default()
        });
    }

    /// Asks the worker for a fresh snapshot now instead of at its next tick.
    pub(crate) fn request_refresh(&self) {
        lock(&self.shared.control).refresh = true;
        self.shared.wake.notify_all();
    }

    /// Skips periodic collection while the panel is blanked; resuming
    /// collects right away so the first frames after unblanking are fresh.
    pub(crate) fn set_paused(&self, paused: bool) {
        let mut control = lock(&self.shared.control);
        if control.paused && !paused {
            control.refresh = true;
            self.shared.wake.notify_all();
        }
        control.paused = paused;
    }

    /// Applies a local card edit (headline marked read, card dismissed) to the
    /// current snapshot so the next frame shows it immediately, before the
    /// worker's next collection picks the change up from disk.
    pub(crate) fn update_cards(&self, edit: impl FnOnce(&mut Vec<nexus_sensors::SensorCard>)) {
        let mut latest = lock(&self.shared.latest);
        let mut cards = latest.snapshot.cards.clone();
        edit(&mut cards);
        latest.snapshot = Arc::new(MetricsSnapshot {
            metrics: latest.snapshot.metrics.clone(),
            catalog: latest.snapshot.catalog.clone(),
            media: latest.snapshot.media.clone(),
            volume_muted: latest.snapshot.volume_muted,
            running: latest.snapshot.running.clone(),
            cards,
            display: latest.snapshot.display.clone(),
        });
        latest.edits += 1;
    }

    /// Collects in the background every `period`, starting right away. The
    /// first collection can take seconds (feeds, Home Assistant), so the
    /// strip starts with the clock and fills in readings as they arrive.
    pub(crate) fn spawn_worker(&self, period: Duration) -> MetricsWorker {
        lock(&self.shared.control).refresh = true;
        let handle = self.clone();
        thread::spawn(move || handle.run(period));
        MetricsWorker {
            handle: self.clone(),
        }
    }

    fn run(&self, period: Duration) {
        let mut immediate = false;
        loop {
            {
                let control = lock(&self.shared.control);
                let mut control = if std::mem::take(&mut immediate) {
                    control
                } else {
                    self.shared
                        .wake
                        .wait_timeout_while(control, period, |c| !c.refresh && !c.stop)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                };
                if control.stop {
                    return;
                }
                let skip = control.paused && !control.refresh;
                control.refresh = false;
                if skip {
                    continue;
                }
            }
            let edits_before = lock(&self.shared.latest).edits;
            // A panicking collector must not silently freeze the panel on its
            // last snapshot; skip this round and try again next tick.
            let Ok(snapshot) = panic::catch_unwind(MetricsSnapshot::collect) else {
                eprintln!("sensor collection panicked; keeping the previous snapshot");
                continue;
            };
            let mut latest = lock(&self.shared.latest);
            immediate = latest.edits != edits_before;
            if !immediate {
                latest.snapshot = Arc::new(snapshot);
            }
        }
    }
}

/// Stops the worker thread on drop. It is not joined: a collection can sit in
/// a 5 s feed fetch while offline, and shutdown should not wait for that.
pub(crate) struct MetricsWorker {
    handle: MetricsHandle,
}

impl Drop for MetricsWorker {
    fn drop(&mut self) {
        lock(&self.handle.shared.control).stop = true;
        self.handle.shared.wake.notify_all();
    }
}
