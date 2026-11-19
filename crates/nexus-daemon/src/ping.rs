//! Pinging for the strip. While the active profile shows a ping widget or a
//! `net.ping*` reading (in a metric widget, its corner or a text token),
//! the hosts they name are probed in the background; with none of them,
//! while the strip is off or while it is unplugged, nothing is sent.

use nexus_config::{Profile, Settings, Widget, WidgetKind, MAX_PING_THRESHOLD_MS};
use nexus_render::{LiveData, Metric, PingInfo};
use nexus_sensors::{
    probe_timeout, MetricReading, MetricUnit, PingMethod, PingMonitor, PingReading,
};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// Full scale of `net.ping` and `net.ping.jitter` for bars and rings.
const PING_FULL_SCALE_MS: f32 = 200.0;
const JITTER_FULL_SCALE_MS: f32 = 50.0;

/// The hosts `profile` needs pinged, each once in the order they appear,
/// with the probe timeout their widgets' red thresholds need.
pub(crate) fn wanted_hosts(profile: &Profile, settings: &Settings) -> Vec<(String, Duration)> {
    let mut hosts: Vec<(String, Option<u32>)> = Vec::new();
    for widget in profile.pages.iter().flat_map(|page| &page.widgets) {
        let Some(host) = widget.ping_host(&settings.ping) else {
            continue;
        };
        let red = red_threshold_ms(widget);
        match hosts.iter_mut().find(|(known, _)| known == host) {
            Some((_, known)) => *known = (*known).max(red),
            None => hosts.push((host.to_string(), red)),
        }
    }
    hosts
        .into_iter()
        .map(|(host, red)| (host, probe_timeout(red)))
        .collect()
}

/// The round trip from which a widget turns red, in milliseconds.
fn red_threshold_ms(widget: &Widget) -> Option<u32> {
    match &widget.kind {
        WidgetKind::Ping(props) => Some(props.critical_ms),
        WidgetKind::Metric(props) if props.source == "net.ping" => props
            .critical
            .map(|ms| ms.clamp(0.0, MAX_PING_THRESHOLD_MS as f32) as u32),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct Ping {
    monitor: Option<PingMonitor>,
    /// `net.ping.jitter` and `net.ping.loss` once a second, for metric
    /// graphs, and the host they were measured for.
    history: HashMap<String, VecDeque<f32>>,
    history_host: String,
}

impl Ping {
    /// Never pings: ping widgets keep saying they are measuring.
    pub(crate) fn off() -> Self {
        Self::default()
    }

    pub(crate) fn on() -> Self {
        Self {
            monitor: Some(PingMonitor::new()),
            ..Self::default()
        }
    }

    /// Follows the active profile, once a second; `paused` (the strip is
    /// off or unplugged) stops all probes.
    pub(crate) fn set_wanted(&mut self, profile: &Profile, settings: &Settings, paused: bool) {
        let Some(monitor) = &self.monitor else {
            return;
        };
        let hosts = if paused {
            Vec::new()
        } else {
            wanted_hosts(profile, settings)
        };
        let interval = Duration::from_millis(u64::from(settings.ping.interval_ms));
        monitor.set_hosts(&hosts, interval);
        let readings = self.readings(settings);
        // Another host's graph would continue where this one's left off.
        if readings.is_empty() || self.history_host != settings.ping.host {
            self.history.clear();
            self.history_host = settings.ping.host.clone();
        }
        // `net.ping` draws its graph from the probes themselves.
        for reading in readings.into_iter().filter(|r| r.id != "net.ping") {
            let history = self.history.entry(reading.id).or_default();
            history.push_back(reading.value);
            while history.len() > crate::live::HISTORY {
                history.pop_front();
            }
        }
    }

    /// Ping widgets' data and the `net.ping*` readings for a frame.
    pub(crate) fn fill(&self, live: &mut LiveData, settings: &Settings) {
        let Some(monitor) = &self.monitor else {
            return;
        };
        for reading in monitor.readings() {
            let info = info(&reading);
            if reading.host == settings.ping.host {
                for metric in metrics(&reading) {
                    live.metrics.insert(
                        metric.id.clone(),
                        Metric {
                            value: metric.value,
                            unit: crate::live::unit(metric.unit),
                            label: metric.label,
                            max: metric.max,
                        },
                    );
                }
                // One point per probe, so the graph keeps moving while the
                // host does not answer. A lost probe repeats the round trip
                // before it: zero would read as an excellent one.
                let mut last = reading.history.iter().flatten().next().copied();
                let probes = reading.history.iter().map(|probe| {
                    if probe.is_some() {
                        last = *probe;
                    }
                    last.unwrap_or(0.0)
                });
                live.history.insert("net.ping".into(), probes.collect());
                live.ping.insert(String::new(), info.clone());
            }
            live.ping.insert(reading.host, info);
        }
        for (id, values) in &self.history {
            live.history
                .insert(id.clone(), values.iter().copied().collect());
        }
    }

    /// The `net.ping*` readings of `settings.ping.host` while it is pinged.
    pub(crate) fn readings(&self, settings: &Settings) -> Vec<MetricReading> {
        self.monitor
            .as_ref()
            .and_then(|monitor| monitor.reading(&settings.ping.host))
            .map(|reading| metrics(&reading))
            .unwrap_or_default()
    }
}

pub(crate) fn info(reading: &PingReading) -> PingInfo {
    let stats = reading.stats();
    PingInfo {
        host: reading.host.clone(),
        history: reading.history.clone(),
        last_ms: stats.last_ms,
        avg_ms: stats.avg_ms,
        min_ms: stats.min_ms,
        max_ms: stats.max_ms,
        jitter_ms: stats.jitter_ms,
        loss_percent: stats.loss_percent,
        down: stats.down(),
        unresolved: reading.unresolved,
        tcp: reading.method == PingMethod::Tcp,
        gateway: reading.gateway,
    }
}

/// `net.ping` (left out while the host does not answer), `net.ping.jitter`
/// and `net.ping.loss`, each once there is something to report.
pub(crate) fn metrics(reading: &PingReading) -> Vec<MetricReading> {
    let stats = reading.stats();
    let reading = |id: &str, label: &str, value: Option<f32>, unit, max| {
        value.map(|value| MetricReading {
            id: id.into(),
            label: label.into(),
            value,
            unit,
            max,
        })
    };
    let last = stats.last_ms.filter(|_| !stats.down());
    [
        reading(
            "net.ping",
            "Ping",
            last,
            MetricUnit::Milliseconds,
            PING_FULL_SCALE_MS,
        ),
        reading(
            "net.ping.jitter",
            "Jitter",
            stats.jitter_ms,
            MetricUnit::Milliseconds,
            JITTER_FULL_SCALE_MS,
        ),
        reading(
            "net.ping.loss",
            "Packet loss",
            stats.loss_percent,
            MetricUnit::Percent,
            100.0,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_config::{MetricProps, PingProps, TextProps, Widget, WidgetKind};

    fn profile(kinds: Vec<WidgetKind>) -> Profile {
        let mut profile = nexus_config::default_config().profiles.remove(0);
        for page in &mut profile.pages {
            page.widgets.clear();
        }
        for (i, kind) in kinds.into_iter().enumerate() {
            // Spread over two pages: a hidden page counts too.
            let page = &mut profile.pages[i % 2];
            page.widgets
                .push(Widget::new(format!("w{i}"), 0, 100, kind));
        }
        profile
    }

    fn hosts(profile: &Profile, settings: &Settings) -> Vec<String> {
        wanted_hosts(profile, settings)
            .into_iter()
            .map(|(host, _)| host)
            .collect()
    }

    #[test]
    fn only_profiles_that_show_ping_want_hosts() {
        let settings = Settings::default();
        let plain = nexus_config::default_config();
        assert!(hosts(&plain.profiles[0], &settings).is_empty());

        let own = |host: &str| {
            WidgetKind::Ping(PingProps {
                host: Some(host.into()),
                ..PingProps::default()
            })
        };
        let metric = WidgetKind::Metric(MetricProps {
            source: "cpu.usage".into(),
            secondary: Some("net.ping.loss".into()),
            ..MetricProps::default()
        });
        let text = WidgetKind::Text(TextProps {
            text: "{net.ping}".into(),
            ..TextProps::default()
        });
        let profile = profile(vec![
            own("9.9.9.9"),
            WidgetKind::Ping(PingProps::default()),
            metric,
            own("9.9.9.9"),
            text,
        ]);
        assert_eq!(hosts(&profile, &settings), ["9.9.9.9", "1.1.1.1"]);
    }

    #[test]
    fn red_thresholds_lengthen_the_probe_timeout() {
        let settings = Settings::default();
        let ping = |host: Option<&str>, critical_ms| {
            WidgetKind::Ping(PingProps {
                host: host.map(Into::into),
                warn_ms: 50,
                critical_ms,
            })
        };
        let metric = |critical| {
            WidgetKind::Metric(MetricProps {
                source: "net.ping".into(),
                critical: Some(critical),
                ..MetricProps::default()
            })
        };
        let profile = profile(vec![
            ping(Some("9.9.9.9"), 120),
            ping(Some("9.9.9.9"), 2000),
            ping(None, 120),
            metric(3000.0),
            ping(Some("10.0.0.1"), 120),
            metric(1e9),
        ]);
        let ms = Duration::from_millis;
        assert_eq!(
            wanted_hosts(&profile, &settings),
            [
                ("9.9.9.9".to_string(), ms(2500)),
                // A metric's red threshold counts, within the widget limit.
                ("1.1.1.1".to_string(), ms(10_500)),
                ("10.0.0.1".to_string(), ms(1000)),
            ]
        );
    }

    #[test]
    fn blanking_or_an_off_monitor_sends_nothing() {
        // Loopback: a test sends nothing out to the network.
        let mut settings = Settings::default();
        settings.ping.host = "127.0.0.1".into();
        let profile = profile(vec![WidgetKind::Ping(PingProps::default())]);
        let mut off = Ping::off();
        off.set_wanted(&profile, &settings, false);
        let mut live = LiveData::default();
        off.fill(&mut live, &settings);
        assert!(live.ping.is_empty() && off.readings(&settings).is_empty());

        let mut on = Ping::on();
        let readings = |ping: &Ping| ping.monitor.as_ref().unwrap().readings();
        on.set_wanted(&profile, &settings, true);
        assert!(readings(&on).is_empty(), "the strip is off or unplugged");
        on.set_wanted(&profile, &settings, false);
        assert_eq!(readings(&on).len(), 1);
        // The profile no longer pings: its host's thread and reading go.
        on.set_wanted(
            &nexus_config::default_config().profiles[0],
            &settings,
            false,
        );
        assert!(readings(&on).is_empty());
    }

    #[test]
    fn the_metric_graphs_start_over_for_another_host() {
        let mut settings = Settings::default();
        settings.ping.host = "127.0.0.1".into();
        let profile = profile(vec![WidgetKind::Ping(PingProps::default())]);
        let mut ping = Ping::on();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while ping.history.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            ping.set_wanted(&profile, &settings, false);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(ping.history_host, "127.0.0.1");
        let mut live = LiveData::default();
        ping.fill(&mut live, &settings);
        assert!(live.history.contains_key("net.ping.loss"));
        assert!(!live.history["net.ping"].is_empty());

        settings.ping.host = "::1".into();
        ping.set_wanted(&profile, &settings, false);
        assert!(ping.history.is_empty(), "no reading for ::1 yet");
        assert_eq!(ping.history_host, "::1");
    }

    #[test]
    fn readings_follow_the_statistics() {
        let reading = PingReading {
            host: "1.1.1.1".into(),
            history: vec![Some(10.0), None, Some(14.0)],
            ..PingReading::default()
        };
        let ids: Vec<(String, f32)> = metrics(&reading)
            .into_iter()
            .map(|m| (m.id, m.value))
            .collect();
        assert_eq!(ids[0], ("net.ping".to_string(), 14.0));
        assert_eq!(ids[1], ("net.ping.jitter".to_string(), 4.0));
        assert_eq!(ids[2].0, "net.ping.loss");
        assert!((ids[2].1 - 100.0 / 3.0).abs() < 1e-3);

        // A host that stopped answering has no current round trip.
        let down = PingReading {
            history: vec![Some(10.0), None, None, None],
            method: PingMethod::Tcp,
            ..reading
        };
        assert!(metrics(&down).iter().all(|m| m.id != "net.ping"));
        let info = info(&down);
        assert!(info.down && info.tcp && !info.unresolved);
        assert_eq!(info.last_ms, Some(10.0));
    }

    #[test]
    fn the_settings_host_fills_the_default_key_and_the_metrics() {
        let settings = Settings::default();
        let mut ping = Ping::on();
        ping.set_wanted(
            &profile(vec![WidgetKind::Ping(PingProps {
                host: Some("127.0.0.1".into()),
                ..PingProps::default()
            })]),
            &settings,
            false,
        );
        let mut live = LiveData::default();
        ping.fill(&mut live, &settings);
        assert!(live.ping.contains_key("127.0.0.1"));
        assert!(
            !live.ping.contains_key(""),
            "the settings host is not pinged"
        );
        assert!(live.metrics.is_empty() && live.history.is_empty());
    }
}
