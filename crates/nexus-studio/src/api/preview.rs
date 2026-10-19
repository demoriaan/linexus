//! Pixels: draft pages rendered by the strip's own renderer, the daemon's
//! current frame, and the metric catalog the editor offers.

use super::{daemon_call, parse_json};
use crate::http::{Request, Response};
use crate::state::AppState;
use base64::Engine as _;
use nexus_config::{Page, Settings, TemperatureUnit, Theme};
use nexus_ipc::MetricReading;
use nexus_render::{LiveData, Locale, Metric, UiState, Unit};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Readings are reused this long; the daemon samples once per second.
const LIVE_TTL: Duration = Duration::from_secs(1);
/// After a failed connect we stop trying for a moment, so a stopped daemon
/// does not cost a connect attempt on every preview.
const OFFLINE_TTL: Duration = Duration::from_secs(2);
/// Points in a synthesized sparkline history.
const HISTORY_POINTS: usize = 40;

pub fn route(request: &Request, state: &AppState) -> Option<Response> {
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/api/preview") => render(state, &request.body),
        ("GET", "/api/frame.png") => frame(state),
        ("GET", "/api/metrics") => metrics(state),
        _ => return None,
    };
    Some(response)
}

/// Daemon metric readings shared by previews and `/api/metrics`. The editor
/// re-renders on every change, so readings are cached briefly instead of
/// asking the daemon each time.
#[derive(Default)]
pub struct MetricsCache {
    slot: Mutex<Option<(Instant, Option<Vec<MetricReading>>)>>,
}

impl MetricsCache {
    /// Current readings, or `None` while the daemon is unreachable.
    pub fn readings(&self, socket: &Path) -> Option<Vec<MetricReading>> {
        {
            let slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((at, readings)) = slot.as_ref() {
                let ttl = if readings.is_some() {
                    LIVE_TTL
                } else {
                    OFFLINE_TTL
                };
                if at.elapsed() < ttl {
                    return readings.clone();
                }
            }
        }
        // Not holding the lock during IPC: a slow daemon must not stall
        // every other request; a duplicate fetch is harmless.
        let fresh = nexus_ipc::send_request(socket, nexus_ipc::Command::Metrics)
            .ok()
            .filter(|reply| reply.ok)
            .and_then(|reply| reply.metrics);
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Instant::now(), fresh.clone()));
        fresh
    }
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub page: Page,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub settings: Option<Settings>,
    /// `en` or `de`; overrides `settings.locale`.
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub ui: PreviewUi,
}

#[derive(Deserialize, Default)]
pub struct PreviewUi {
    /// Widget id drawn in its pressed state.
    #[serde(default)]
    pub pressed: Option<String>,
}

fn render(state: &AppState, body: &[u8]) -> Response {
    let request: PreviewRequest = match parse_json(body, "preview") {
        Ok(request) => request,
        Err(response) => return response,
    };
    let readings = state.metrics.readings(&state.socket_path);
    match render_png(&request, readings.as_deref()) {
        Some(png) => Response::bytes(200, "image/png", png),
        None => Response::error(500, "failed to encode the preview PNG"),
    }
}

/// Renders one page over sample data (with live readings laid over it when
/// the daemon is running) exactly as the strip would show it.
pub fn render_png(request: &PreviewRequest, readings: Option<&[MetricReading]>) -> Option<Vec<u8>> {
    let mut live = nexus_render::preview::sample_live(locale(request));
    let now = nexus_sensors::local_clock();
    live.time = nexus_render::LocalTime {
        year: now.year,
        month: now.month,
        day: now.day,
        weekday: now.weekday,
        hour: now.hour,
        minute: now.minute,
        second: now.second,
    };
    if let Some(settings) = &request.settings {
        live.hour12 = settings.hour12;
        live.fahrenheit = settings.temperature_unit == TemperatureUnit::Fahrenheit;
    }
    if let Some(readings) = readings {
        overlay_readings(&mut live, readings);
    }
    let ui = UiState {
        pressed: request.ui.pressed.clone().map(|id| (id, 1.0)),
        page_index: 0,
        page_count: 1,
        ..UiState::default()
    };
    let canvas = nexus_render::render_page(&request.page, &request.theme, &live, &ui);
    nexus_render::encode_png(canvas.rgba())
}

fn locale(request: &PreviewRequest) -> Locale {
    match (&request.locale, &request.settings) {
        (Some(tag), _) => Locale::resolve(tag),
        (None, Some(settings)) => Locale::resolve(&settings.locale),
        (None, None) => Locale::En,
    }
}

/// Puts live readings over the sample data so metric widgets show real
/// values. The sample sparkline keeps its shape but is rescaled to the live
/// value; metrics without a sample (hwmon sensors) get a flat line.
fn overlay_readings(live: &mut LiveData, readings: &[MetricReading]) {
    for reading in readings {
        let sample = live
            .metrics
            .get(&reading.id)
            .map(|metric| metric.value)
            .filter(|value| *value > 0.0);
        let history = match (sample, live.history.get(&reading.id)) {
            (Some(sample), Some(history)) => {
                history.iter().map(|v| v * reading.value / sample).collect()
            }
            _ => vec![reading.value; HISTORY_POINTS],
        };
        live.history.insert(reading.id.clone(), history);
        let metric = Metric {
            value: reading.value,
            unit: unit_from_name(&reading.unit),
            label: reading.label.clone(),
            max: reading.max,
        };
        live.metrics.insert(reading.id.clone(), metric);
    }
}

/// Unit names as `nexus_ipc::MetricReading` documents them.
fn unit_from_name(name: &str) -> Unit {
    match name {
        "percent" => Unit::Percent,
        "celsius" => Unit::Celsius,
        "watts" => Unit::Watts,
        "rpm" => Unit::Rpm,
        "bytes" => Unit::Bytes,
        "bytes_per_sec" => Unit::BytesPerSec,
        "megahertz" => Unit::Megahertz,
        _ => Unit::Plain,
    }
}

fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Percent => "percent",
        Unit::Celsius => "celsius",
        Unit::Watts => "watts",
        Unit::Rpm => "rpm",
        Unit::Bytes => "bytes",
        Unit::BytesPerSec => "bytes_per_sec",
        Unit::Megahertz => "megahertz",
        Unit::Plain => "plain",
    }
}

/// The frame currently on the strip, for Studio's live mirror.
fn frame(state: &AppState) -> Response {
    let reply = match daemon_call(state, nexus_ipc::Command::Frame) {
        Ok(reply) => reply,
        Err(error) => return Response::error(503, format!("daemon is not reachable: {error}")),
    };
    let Some(encoded) = reply.frame.filter(|_| reply.ok) else {
        let message = if reply.message.is_empty() {
            "daemon sent no frame".into()
        } else {
            reply.message
        };
        return Response::error(503, message);
    };
    match base64::engine::general_purpose::STANDARD.decode(encoded.trim()) {
        Ok(png) => Response::bytes(200, "image/png", png),
        Err(error) => Response::error(502, format!("daemon sent an invalid frame: {error}")),
    }
}

/// Live catalog from the daemon, or the sample catalog when it is not
/// running, so the metric picker always has something to offer.
fn metrics(state: &AppState) -> Response {
    let (live, metrics): (bool, Vec<Value>) = match state.metrics.readings(&state.socket_path) {
        Some(readings) => (true, readings.iter().map(reading_json).collect()),
        None => (false, sample_catalog()),
    };
    Response::json(
        200,
        &json!({ "ok": true, "live": live, "metrics": metrics }),
    )
}

fn reading_json(reading: &MetricReading) -> Value {
    json!({
        "id": reading.id,
        "label": reading.label,
        "value": reading.value,
        "unit": reading.unit,
        "max": reading.max,
    })
}

fn sample_catalog() -> Vec<Value> {
    let live = nexus_render::preview::sample_live(Locale::En);
    let mut ids: Vec<&String> = live.metrics.keys().collect();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let metric = &live.metrics[id];
            json!({
                "id": id,
                "label": metric.label,
                "value": metric.value,
                "unit": unit_name(metric.unit),
                "max": metric.max,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handle;
    use crate::state::testing::{get, json, post, state};
    use nexus_config::PageTemplate;

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).expect("valid PNG").to_rgba8()
    }

    #[test]
    fn every_template_previews_as_a_640x48_png() {
        let state = state();
        for template in PageTemplate::ALL {
            let page = nexus_config::template_page(template, "p");
            let body =
                serde_json::to_vec(&json!({ "page": page, "theme": Theme::default() })).unwrap();
            let response = handle(&post("/api/preview", &body), &state);
            assert_eq!(response.status, 200, "{template:?}");
            assert_eq!(response.content_type, "image/png");
            assert_eq!(response.cache_control, "no-store");
            let image = decode(&response.body);
            assert_eq!(image.dimensions(), (640, 48), "{template:?}");
        }
    }

    #[test]
    fn preview_honours_settings_locale_and_pressed_widget() {
        let state = state();
        let page = nexus_config::template_page(PageTemplate::Launcher, "launcher");
        let plain = json!({ "page": page, "theme": { "preset": "ember" } });
        let mut pressed = plain.clone();
        pressed["ui"] = json!({ "pressed": "browser" });
        pressed["locale"] = json!("de");
        pressed["settings"] = serde_json::to_value(Settings::default()).unwrap();
        let render = |body: &Value| {
            decode(&handle(&post("/api/preview", body.to_string().as_bytes()), &state).body)
        };
        assert_ne!(
            render(&plain),
            render(&pressed),
            "pressed state must be visible"
        );
    }

    #[test]
    fn preview_rejects_malformed_requests() {
        let state = state();
        for body in [&b"nope"[..], br#"{"theme":{}}"#, br#"{"page":{"id":"x"}}"#] {
            let response = handle(&post("/api/preview", body), &state);
            assert_eq!(response.status, 400);
            assert!(json(&response)["message"]
                .as_str()
                .unwrap()
                .starts_with("invalid preview JSON"));
        }
    }

    #[test]
    fn live_readings_replace_sample_values() {
        let mut live = nexus_render::preview::sample_live(Locale::En);
        let sample_history = live.history["cpu.usage"].clone();
        let reading = |id: &str, value: f32, unit: &str| MetricReading {
            id: id.into(),
            label: "X".into(),
            value,
            unit: unit.into(),
            max: 100.0,
        };
        overlay_readings(
            &mut live,
            &[
                reading("cpu.usage", 46.0, "percent"),
                reading("hwmon.fan1", 900.0, "rpm"),
            ],
        );
        assert_eq!(live.metrics["cpu.usage"].value, 46.0);
        let scaled = &live.history["cpu.usage"];
        assert!(
            (scaled[0] - sample_history[0] * 2.0).abs() < 1e-3,
            "sample value is 23"
        );
        assert_eq!(live.metrics["hwmon.fan1"].unit, Unit::Rpm);
        assert_eq!(live.history["hwmon.fan1"], vec![900.0; HISTORY_POINTS]);
    }

    #[test]
    fn metrics_fall_back_to_the_sample_catalog() {
        let body = json(&handle(&get("/api/metrics"), &state()));
        assert_eq!(body["live"], false);
        let cpu = body["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|metric| metric["id"] == "cpu.usage")
            .expect("cpu.usage in the sample catalog");
        assert_eq!(cpu["unit"], "percent");
    }

    #[test]
    fn frame_without_daemon_is_503() {
        let response = handle(&get("/api/frame.png"), &state());
        assert_eq!(response.status, 503);
        assert_eq!(json(&response)["ok"], false);
    }
}
