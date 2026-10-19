//! Network refreshes outside the collection tick.
//!
//! The daemon collects every second. Feeds and Home Assistant answer within
//! seconds at best and not at all while offline, so a collection never waits
//! for them: it reads the last good result and, when that is due, starts one
//! background refresh per source. A failed refresh is retried after a backoff
//! (30 s, doubling up to 10 min) instead of on every tick.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const FIRST_RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(600);

/// Wait before the next attempt after `failures` failed refreshes in a row.
pub(crate) fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(8);
    (FIRST_RETRY * (1 << doublings)).min(MAX_RETRY)
}

#[derive(Debug, Default)]
struct Schedule {
    running: bool,
    failures: u32,
    retry_at: Option<Instant>,
}

impl Schedule {
    /// Claims the refresh unless one is running or the backoff has not passed.
    fn try_start(&mut self, now: Instant) -> bool {
        if self.running || self.retry_at.is_some_and(|at| now < at) {
            return false;
        }
        self.running = true;
        true
    }

    fn finish(&mut self, now: Instant, ok: bool) {
        self.running = false;
        if ok {
            self.failures = 0;
            self.retry_at = None;
        } else {
            self.failures = self.failures.saturating_add(1);
            self.retry_at = Some(now + retry_delay(self.failures));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// At most one background refresh per key, with retry backoff.
pub(crate) struct Refresher {
    schedules: Mutex<BTreeMap<String, Schedule>>,
}

impl Refresher {
    pub(crate) const fn new() -> Self {
        Self {
            schedules: Mutex::new(BTreeMap::new()),
        }
    }

    /// Runs `fetch` on its own thread unless a refresh for `key` is running
    /// or backing off after a failure. `fetch` reports whether it succeeded.
    /// Returns whether a refresh was started; never waits for it.
    pub(crate) fn start(
        &'static self,
        key: &str,
        now: Instant,
        fetch: impl FnOnce() -> bool + Send + 'static,
    ) -> bool {
        if !self.with_schedule(key, |schedule| schedule.try_start(now)) {
            return false;
        }
        let owned = key.to_string();
        let spawned = thread::Builder::new()
            .name("sensor-refresh".into())
            .spawn(move || {
                // A panicking fetch counts as a failure; it must not leave
                // the key marked as running forever.
                let ok = panic::catch_unwind(AssertUnwindSafe(fetch)).unwrap_or(false);
                self.with_schedule(&owned, |schedule| schedule.finish(Instant::now(), ok));
            });
        if spawned.is_err() {
            self.with_schedule(key, |schedule| schedule.finish(now, false));
            return false;
        }
        true
    }

    fn with_schedule<R>(&self, key: &str, edit: impl FnOnce(&mut Schedule) -> R) -> R {
        edit(lock(&self.schedules).entry(key.to_string()).or_default())
    }

    #[cfg(test)]
    fn running(&self, key: &str) -> bool {
        lock(&self.schedules)
            .get(key)
            .is_some_and(|schedule| schedule.running)
    }
}

/// The last good value per key, kept in memory and refreshed through a
/// [`Refresher`].
pub(crate) struct ValueCache<T> {
    values: Mutex<BTreeMap<String, (T, Instant)>>,
    refresher: Refresher,
}

impl<T: Clone + Send + 'static> ValueCache<T> {
    pub(crate) const fn new() -> Self {
        Self {
            values: Mutex::new(BTreeMap::new()),
            refresher: Refresher::new(),
        }
    }

    /// The value for `key` unless it is older than `max_stale` (or there is
    /// none yet). Starts a background `fetch` when the value is older than
    /// `refresh`; its result shows up in a later call.
    pub(crate) fn get(
        &'static self,
        key: &str,
        now: Instant,
        refresh: Duration,
        max_stale: Duration,
        fetch: impl FnOnce() -> Option<T> + Send + 'static,
    ) -> Option<T> {
        let cached = lock(&self.values).get(key).cloned();
        let age = |at: Instant| now.saturating_duration_since(at);
        if cached.as_ref().is_none_or(|(_, at)| age(*at) >= refresh) {
            let owned = key.to_string();
            self.refresher.start(key, now, move || match fetch() {
                Some(value) => {
                    lock(&self.values).insert(owned, (value, Instant::now()));
                    true
                }
                None => false,
            });
        }
        cached
            .filter(|(_, at)| age(*at) < max_stale)
            .map(|(value, _)| value)
    }
}

#[cfg(test)]
pub(crate) fn wait_until_idle(refresher: &Refresher, key: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while refresher.running(key) {
        assert!(Instant::now() < deadline, "refresh of {key} never finished");
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};

    #[test]
    fn retry_delay_doubles_up_to_ten_minutes() {
        let secs: Vec<u64> = (1..=8).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(secs, [30, 60, 120, 240, 480, 600, 600, 600]);
        assert_eq!(retry_delay(u32::MAX), MAX_RETRY);
    }

    #[test]
    fn schedule_backs_off_after_failures_and_resets_on_success() {
        let t0 = Instant::now();
        let mut schedule = Schedule::default();
        assert!(schedule.try_start(t0));
        assert!(!schedule.try_start(t0), "only one refresh at a time");
        schedule.finish(t0, false);
        assert!(!schedule.try_start(t0 + Duration::from_secs(29)));
        assert!(schedule.try_start(t0 + Duration::from_secs(30)));
        schedule.finish(t0 + Duration::from_secs(30), false);
        assert!(!schedule.try_start(t0 + Duration::from_secs(89)));
        assert!(schedule.try_start(t0 + Duration::from_secs(90)));
        schedule.finish(t0 + Duration::from_secs(90), true);
        assert!(schedule.try_start(t0 + Duration::from_secs(90)));
    }

    #[test]
    fn refresher_runs_in_the_background_and_backs_off() {
        let refresher: &'static Refresher = Box::leak(Box::new(Refresher::new()));
        let (release, wait) = mpsc::channel::<bool>();
        let started = Instant::now();
        assert!(refresher.start("feed", started, move || wait.recv().unwrap()));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "start blocked"
        );
        assert!(!refresher.start("feed", Instant::now(), || true));
        assert!(refresher.start("other", Instant::now(), || true));

        release.send(false).unwrap();
        wait_until_idle(refresher, "feed");
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let fetch = move || {
            counter.fetch_add(1, Ordering::SeqCst);
            true
        };
        assert!(!refresher.start("feed", Instant::now(), fetch.clone()));
        let later = Instant::now() + FIRST_RETRY;
        assert!(refresher.start("feed", later, fetch.clone()));
        wait_until_idle(refresher, "feed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Success clears the backoff; freshness is the caller's business.
        assert!(refresher.start("feed", Instant::now(), fetch));
        wait_until_idle(refresher, "feed");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        assert!(refresher.start("panics", Instant::now(), || panic!("fetch failed")));
        wait_until_idle(refresher, "panics");
        assert!(!refresher.start("panics", Instant::now(), || true));
    }

    #[test]
    fn value_cache_serves_the_last_value_while_refreshing_and_offline() {
        let cache: &'static ValueCache<u32> = Box::leak(Box::new(ValueCache::new()));
        let refresh = Duration::from_secs(5);
        let max_stale = Duration::from_secs(300);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let offline = move || {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        };
        let t0 = Instant::now();
        assert_eq!(cache.get("light", t0, refresh, max_stale, || Some(1)), None);
        wait_until_idle(&cache.refresher, "light");
        assert_eq!(
            cache.get("light", t0, refresh, max_stale, offline.clone()),
            Some(1)
        );
        wait_until_idle(&cache.refresher, "light");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "still fresh");

        // Due again; the refresh fails (offline), the old value stays.
        let due = Instant::now() + refresh;
        assert_eq!(
            cache.get("light", due, refresh, max_stale, || None),
            Some(1)
        );
        wait_until_idle(&cache.refresher, "light");
        assert_eq!(
            cache.get("light", due, refresh, max_stale, offline.clone()),
            Some(1)
        );
        wait_until_idle(&cache.refresher, "light");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "backing off");
        // Too old to show.
        let gone = Instant::now() + max_stale;
        assert_eq!(cache.get("light", gone, refresh, max_stale, offline), None);
        wait_until_idle(&cache.refresher, "light");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
