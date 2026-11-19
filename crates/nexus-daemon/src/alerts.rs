//! Breaking news on the strip. [`nexus_sensors::AlertState`] decides which
//! items of the alert feeds fire; this keeps that state on disk, the banners
//! waiting for their turn, and the label the news widget shows for an hour.
//! Waits and labels count wall-clock seconds, so a suspended machine does
//! not show hour-old news as breaking when it wakes up.
//!
//! Priority: a banner goes above everything else. A notification shown when
//! one arrives waits behind it and comes back in full afterwards; a banner
//! never waits for a notification (see `Scene::show_alert`).

use crate::metrics_cache::MetricsSnapshot;
use nexus_sensors::{AlertState, BreakingNews, SensorCard};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Banners waiting for their turn; beyond this the oldest is dropped.
pub(crate) const MAX_PENDING: usize = 5;
/// A banner that could not show for this long (strip off) is dropped.
const PENDING_MAX_AGE_SECS: u64 = 30 * 60;
/// How long the news widget lists breaking news first, labelled as such,
/// and how long the same story from another feed stays quiet.
const LABEL_FOR_SECS: u64 = 60 * 60;
const MAX_LABELLED: usize = 10;
const MAX_RECENT: usize = 100;
/// Card status of a labelled headline.
pub(crate) const ALERT_STATUS: &str = "ALERT";

/// Where the seen items are kept: next to the daemon's runtime state.
pub(crate) fn state_path(runtime_state: &Path) -> PathBuf {
    runtime_state.with_file_name("alert-seen.json")
}

/// The wall clock in unix seconds.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Whether something stamped `at` is younger than `secs` at `unix`.
fn within(at: u64, unix: u64, secs: u64) -> bool {
    unix.saturating_sub(at) < secs
}

pub(crate) struct Alerts {
    state: AlertState,
    /// None keeps the state in memory only (one-shot renders, tests).
    path: Option<PathBuf>,
    dirty: bool,
    /// The snapshot looked at last; each one is looked at once.
    observed: Option<Arc<MetricsSnapshot>>,
    /// Stamped with the wall clock when they fired, like the lists below.
    pending: VecDeque<(u64, BreakingNews)>,
    labelled: Vec<(u64, BreakingNews)>,
    /// Everything that fired in the last hour, to keep the same story from
    /// a second feed quiet.
    recent: Vec<(u64, BreakingNews)>,
}

impl Alerts {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        Self {
            state: path.as_deref().map(AlertState::load).unwrap_or_default(),
            path,
            dirty: false,
            observed: None,
            pending: VecDeque::new(),
            labelled: Vec::new(),
            recent: Vec::new(),
        }
    }

    /// Looks at a new snapshot: returns what fired and, with `banners` on,
    /// queues a banner for each. `unix` is the wall clock in seconds. A
    /// story that already fired in the last hour (the same link or title,
    /// from any feed) does not fire again.
    pub(crate) fn observe(
        &mut self,
        snapshot: &Arc<MetricsSnapshot>,
        unix: u64,
        banners: bool,
    ) -> Vec<BreakingNews> {
        if self
            .observed
            .as_ref()
            .is_some_and(|last| Arc::ptr_eq(last, snapshot))
        {
            return Vec::new();
        }
        self.observed = Some(snapshot.clone());
        self.labelled
            .retain(|(at, _)| within(*at, unix, LABEL_FOR_SECS));
        self.recent
            .retain(|(at, _)| within(*at, unix, LABEL_FOR_SECS));
        let sources = nexus_sensors::alert_sources(&snapshot.metrics.sensor_records);
        if sources.is_empty() {
            return Vec::new();
        }
        let (fired, changed) = self.state.observe(&sources, unix);
        self.dirty |= changed;
        let mut kept = Vec::new();
        for news in fired {
            if self
                .recent
                .iter()
                .any(|(_, other)| nexus_sensors::same_story(other, &news))
            {
                continue;
            }
            self.recent.push((unix, news.clone()));
            if banners {
                self.pending.push_back((unix, news.clone()));
            }
            self.labelled.retain(|(_, other)| other.id != news.id);
            self.labelled.push((unix, news.clone()));
            kept.push(news);
        }
        while self.pending.len() > MAX_PENDING {
            self.pending.pop_front();
        }
        while self.labelled.len() > MAX_LABELLED {
            self.labelled.remove(0);
        }
        if self.recent.len() > MAX_RECENT {
            self.recent.drain(..self.recent.len() - MAX_RECENT);
        }
        kept
    }

    /// Writes the state when it changed since the last call.
    pub(crate) fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.as_deref().filter(|_| self.dirty) else {
            return Ok(());
        };
        // A failed write is tried again with the next change, not every frame.
        self.dirty = false;
        self.state.save(path)
    }

    /// The banner to show next, oldest first; `unix` is the wall clock.
    pub(crate) fn next(&mut self, unix: u64) -> Option<BreakingNews> {
        while let Some((at, news)) = self.pending.pop_front() {
            if unix.saturating_sub(at) <= PENDING_MAX_AGE_SECS {
                return Some(news);
            }
        }
        None
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Takes a hidden headline out of the label list.
    pub(crate) fn forget(&mut self, id: &str) {
        self.labelled.retain(|(_, news)| news.id != id);
    }

    /// `cards` with the breaking news of the last hour first, newest first
    /// and labelled. An item its feed no longer lists among the first few
    /// still shows.
    pub(crate) fn decorate(&self, cards: &[SensorCard], unix: u64) -> Vec<SensorCard> {
        let fresh: Vec<&BreakingNews> = self
            .labelled
            .iter()
            .rev()
            .filter(|(at, _)| within(*at, unix, LABEL_FOR_SECS))
            .map(|(_, news)| news)
            .collect();
        if fresh.is_empty() {
            return cards.to_vec();
        }
        let mut out: Vec<SensorCard> = fresh
            .iter()
            .map(|news| {
                let card = cards
                    .iter()
                    .find(|card| card.id == news.id)
                    .cloned()
                    .unwrap_or_else(|| SensorCard {
                        id: news.id.clone(),
                        module: "rss".into(),
                        kind: "rss_item".into(),
                        source: news.source.clone(),
                        title: news.title.clone(),
                        url: news.link.clone(),
                        status: None,
                    });
                SensorCard {
                    status: Some(ALERT_STATUS.into()),
                    ..card
                }
            })
            .collect();
        out.extend(
            cards
                .iter()
                .filter(|card| fresh.iter().all(|news| news.id != card.id))
                .cloned(),
        );
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nexus_sensors::SensorRecord;

    pub(crate) const NOW: u64 = 1_795_093_380;

    /// An alert feed's record with these (title, published) items,
    /// downloaded at `NOW`.
    pub(crate) fn feed(items: &[(&str, Option<u64>)]) -> SensorRecord {
        source_feed("news", items, NOW)
    }

    /// The alert feed `id` with these (title, published) items, downloaded
    /// at `fetched`; an item links to its title on example.test.
    pub(crate) fn source_feed(
        id: &str,
        items: &[(&str, Option<u64>)],
        fetched: u64,
    ) -> SensorRecord {
        let items: Vec<serde_json::Value> = items
            .iter()
            .map(|(title, published)| {
                serde_json::json!({
                    "id": nexus_sensors::rss_item_key(id, title, None),
                    "title": title,
                    "link": format!("https://example.test/{}", title.replace(' ', "-")),
                    "published": published,
                })
            })
            .collect();
        SensorRecord {
            module: "rss".into(),
            kind: "rss_feed".into(),
            label: id.into(),
            value: serde_json::json!({
                "id": id, "url": format!("https://example.test/{id}.xml"), "title": id,
                "items": [],
                "alert": {"keywords": [], "opens": true, "fetched": fetched, "items": items},
            }),
        }
    }

    pub(crate) fn snapshot(records: Vec<SensorRecord>) -> Arc<MetricsSnapshot> {
        let cards = nexus_sensors::sensor_cards(&records);
        let mut snapshot = MetricsSnapshot {
            cards,
            ..MetricsSnapshot::default()
        };
        snapshot.metrics.sensor_records = records;
        Arc::new(snapshot)
    }

    fn titles(fired: &[BreakingNews]) -> Vec<&str> {
        fired.iter().map(|news| news.title.as_str()).collect()
    }

    #[test]
    fn each_snapshot_is_looked_at_once_and_banners_queue_up_to_five() {
        let mut alerts = Alerts::new(None);
        assert!(alerts
            .observe(&snapshot(vec![feed(&[("Old", Some(NOW))])]), NOW, true)
            .is_empty());
        let names: Vec<String> = (1..=7).map(|i| format!("News {i}")).collect();
        let items: Vec<(&str, Option<u64>)> = names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), Some(NOW + i as u64)))
            .collect();
        let fresh = snapshot(vec![feed(&items)]);
        let fired = alerts.observe(&fresh, NOW + 60, true);
        assert_eq!(fired.len(), 7);
        assert!(alerts.observe(&fresh, NOW + 61, true).is_empty());
        // The two oldest gave way; the rest show oldest first.
        let shown: Vec<String> = std::iter::from_fn(|| alerts.next(NOW + 62))
            .map(|news| news.title)
            .collect();
        assert_eq!(shown, ["News 3", "News 4", "News 5", "News 6", "News 7"]);
    }

    #[test]
    fn banners_that_waited_too_long_are_dropped_and_switched_off_ones_never_queue() {
        let mut alerts = Alerts::new(None);
        alerts.observe(&snapshot(vec![feed(&[])]), NOW, true);
        let fired = alerts.observe(&snapshot(vec![feed(&[("Quiet", Some(NOW))])]), NOW, false);
        assert_eq!(titles(&fired), ["Quiet"]);
        assert!(alerts.next(NOW).is_none(), "banners off");
        alerts.observe(&snapshot(vec![feed(&[("Late", Some(NOW))])]), NOW, true);
        // Half an hour by the wall clock, however long the process ran: a
        // suspended machine counts the time it slept.
        assert!(alerts.next(NOW + PENDING_MAX_AGE_SECS + 1).is_none());
        alerts.observe(&snapshot(vec![feed(&[("On time", Some(NOW))])]), NOW, true);
        assert_eq!(
            alerts
                .next(NOW + PENDING_MAX_AGE_SECS)
                .map(|news| news.title)
                .as_deref(),
            Some("On time")
        );
    }

    #[test]
    fn the_news_widget_lists_breaking_news_first_for_an_hour() {
        let mut alerts = Alerts::new(None);
        alerts.observe(&snapshot(vec![feed(&[("Old", Some(NOW))])]), NOW, true);
        let record = feed(&[("Fresh", Some(NOW)), ("Old", Some(NOW))]);
        let fresh = snapshot(vec![record]);
        let fired_at = NOW + 60;
        alerts.observe(&fresh, fired_at, true);
        let card = |id: &str, title: &str| SensorCard {
            id: id.into(),
            module: "rss".into(),
            kind: "rss_item".into(),
            source: "Example".into(),
            title: title.into(),
            url: None,
            status: Some("NEW".into()),
        };
        let fresh_id = nexus_sensors::rss_item_key("news", "Fresh", None);
        let cards = vec![card("other", "Other"), card(&fresh_id, "Fresh")];
        let decorated = alerts.decorate(&cards, fired_at);
        let order: Vec<(&str, Option<&str>)> = decorated
            .iter()
            .map(|card| (card.title.as_str(), card.status.as_deref()))
            .collect();
        assert_eq!(
            order,
            [("Fresh", Some(ALERT_STATUS)), ("Other", Some("NEW"))]
        );
        // Pushed out of the feed's first few, it still shows.
        let decorated = alerts.decorate(&cards[..1], fired_at);
        assert_eq!(decorated[0].title, "Fresh");
        assert_eq!(
            decorated[0].url.as_deref(),
            Some("https://example.test/Fresh")
        );
        assert_eq!(decorated.len(), 2);
        // An hour later by the wall clock it is a normal headline again.
        let later = fired_at + LABEL_FOR_SECS - 1;
        assert_eq!(alerts.decorate(&cards, later)[0].title, "Fresh");
        assert_eq!(alerts.decorate(&cards, fired_at + LABEL_FOR_SECS), cards);
        alerts.forget(&fresh_id);
        assert_eq!(alerts.decorate(&cards, fired_at), cards);
    }

    #[test]
    fn the_same_story_from_a_second_feed_fires_once() {
        let mut alerts = Alerts::new(None);
        let start = snapshot(vec![source_feed("a", &[], NOW), source_feed("b", &[], NOW)]);
        alerts.observe(&start, NOW, true);
        // Both feeds bring the same link at once, and a third story only
        // one of them has.
        let both = snapshot(vec![
            source_feed("a", &[("Storm warning", Some(NOW))], NOW + 60),
            source_feed(
                "b",
                &[("Storm warning", Some(NOW)), ("Bridge closed", Some(NOW))],
                NOW + 60,
            ),
        ]);
        let fired = alerts.observe(&both, NOW + 60, true);
        assert_eq!(titles(&fired), ["Bridge closed", "Storm warning"]);
        // Later the second feed corrects a title under the same link, and
        // the first brings the same title under another link: both quiet.
        let mut corrected = source_feed("b", &[("Bridge now closed", Some(NOW))], NOW + 120);
        corrected.value["alert"]["items"][0]["link"] =
            "https://www.example.test/Bridge-closed/".into();
        let mut moved = source_feed("a", &[("Bridge closed", Some(NOW))], NOW + 120);
        moved.value["alert"]["items"][0]["link"] = "https://example.test/other".into();
        moved.value["alert"]["items"][0]["id"] = "a:moved".into();
        assert!(alerts
            .observe(&snapshot(vec![corrected, moved]), NOW + 120, true)
            .is_empty());
        assert_eq!(std::iter::from_fn(|| alerts.next(NOW + 130)).count(), 2);
        // An hour on, the same title is news again.
        let again = source_feed("a", &[("Storm warning", Some(NOW + 3700))], NOW + 3700);
        let mut again = again;
        again.value["alert"]["items"][0]["id"] = "a:again".into();
        again.value["alert"]["items"][0]["link"] = "https://example.test/storm-2".into();
        assert_eq!(
            titles(&alerts.observe(&snapshot(vec![again]), NOW + 3700, true)),
            ["Storm warning"]
        );
    }

    #[test]
    fn seen_items_stay_seen_across_a_restart() {
        let dir = std::env::temp_dir().join(format!("nexus-daemon-alerts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = state_path(&dir.join("runtime.json"));
        assert_eq!(path, dir.join("alert-seen.json"));
        let mut alerts = Alerts::new(Some(path.clone()));
        alerts.observe(&snapshot(vec![feed(&[("Old", Some(NOW))])]), NOW, true);
        alerts.save().unwrap();
        assert!(path.exists());

        // After a restart the old item is still known and the new one fires;
        // without the file it would be a baseline again.
        let both = snapshot(vec![source_feed(
            "news",
            &[("New", Some(NOW + 30)), ("Old", Some(NOW))],
            NOW + 60,
        )]);
        let mut restarted = Alerts::new(Some(path.clone()));
        assert_eq!(titles(&restarted.observe(&both, NOW + 60, true)), ["New"]);
        let mut fresh_start = Alerts::new(None);
        assert!(fresh_start.observe(&both, NOW + 60, true).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
