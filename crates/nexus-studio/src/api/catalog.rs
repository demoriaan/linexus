//! Picker data: the Lucide icon set, installed applications with their
//! icons, place search for the weather widget and a weather probe.

use crate::http::{percent_encode, Request, Response};
use crate::providers::{self, AppEntry};
use crate::state::AppState;
use nexus_render::tiny_skia::Pixmap;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Installed applications change rarely; rescanning .desktop files for
/// every icon request would be wasteful.
const APPS_TTL: Duration = Duration::from_secs(60);
/// Rendered icons kept in memory before the cache starts over.
const ICON_CACHE_LIMIT: usize = 1024;
const MAX_QUERY_CHARS: usize = 200;

/// Launch targets the action planner resolves to the desktop's defaults.
const DEFAULT_APPS: [(&str, &str); 4] = [
    (nexus_config::DEFAULT_BROWSER, "Default browser"),
    (nexus_config::DEFAULT_FILES, "File manager"),
    (nexus_config::DEFAULT_TERMINAL, "Terminal"),
    (nexus_config::DEFAULT_MAIL, "Mail"),
];

pub fn route(request: &Request, state: &AppState) -> Option<Response> {
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/icons") => icons(),
        ("GET", "/api/apps") => apps(state),
        ("GET", "/api/app-icon") => app_icon(state, request),
        ("GET", "/api/geocode") => geocode(request),
        ("GET", "/api/weather") => weather(request),
        _ => return None,
    };
    Some(response)
}

type IconKey = (String, u32);

#[derive(Default)]
pub struct CatalogCache {
    apps: Mutex<Option<(Instant, Arc<Vec<AppEntry>>)>>,
    /// Encoded PNGs, including misses (`None`) so unknown ids stay cheap.
    icons: Mutex<HashMap<IconKey, Option<Arc<Vec<u8>>>>>,
}

impl CatalogCache {
    fn apps(&self) -> Arc<Vec<AppEntry>> {
        let mut slot = self.apps.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.as_ref() {
            Some((at, apps)) if at.elapsed() < APPS_TTL => apps.clone(),
            _ => {
                let apps = Arc::new(providers::list_applications());
                *slot = Some((Instant::now(), apps.clone()));
                apps
            }
        }
    }

    fn icon(&self, key: IconKey, render: impl FnOnce() -> Option<Vec<u8>>) -> Option<Arc<Vec<u8>>> {
        if let Some(hit) = self
            .icons
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return hit.clone();
        }
        let png = render().map(Arc::new);
        let mut icons = self.icons.lock().unwrap_or_else(PoisonError::into_inner);
        if icons.len() >= ICON_CACHE_LIMIT {
            icons.clear();
        }
        icons.insert(key, png.clone());
        png
    }
}

/// Every Lucide icon as `[name, tags, svg inner markup]`, built once.
fn icons() -> Response {
    static BODY: OnceLock<Vec<u8>> = OnceLock::new();
    let body = BODY.get_or_init(|| {
        let icons: Vec<[&str; 3]> = nexus_render::icon_catalog()
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                Some([parts.next()?, parts.next()?, parts.next()?])
            })
            .collect();
        serde_json::to_vec(&json!({ "ok": true, "icons": icons })).unwrap_or_default()
    });
    Response::bytes(200, "application/json; charset=utf-8", body.clone())
        .with_cache("max-age=86400")
}

fn apps(state: &AppState) -> Response {
    let apps = state.catalog.apps();
    let mut sorted: Vec<&AppEntry> = apps.iter().collect();
    sorted.sort_by_cached_key(|app| app.name.to_lowercase());
    let apps: Vec<Value> = sorted
        .into_iter()
        .map(|app| {
            let icon_url = app
                .icon
                .as_ref()
                .map(|_| format!("/api/app-icon?id={}&size=48", percent_encode(&app.id)));
            json!({ "id": app.id, "name": app.name, "icon_url": icon_url, "categories": app.categories })
        })
        .collect();
    let defaults: Vec<Value> = DEFAULT_APPS
        .iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    Response::json(
        200,
        &json!({ "ok": true, "apps": apps, "defaults": defaults }),
    )
}

fn app_icon(state: &AppState, request: &Request) -> Response {
    let Some(id) = request.query_param("id").filter(|id| !id.is_empty()) else {
        return Response::error(400, "id is required");
    };
    let size = match request.query_param("size").as_deref() {
        None | Some("48") => 48,
        Some("32") => 32,
        Some("64") => 64,
        Some(_) => return Response::error(400, "size must be 32, 48 or 64"),
    };
    let png = state.catalog.icon((id.clone(), size), || {
        render_app_icon(&state.catalog.apps(), &id, size)
    });
    match png {
        Some(png) => Response::bytes(200, "image/png", png.to_vec()),
        None => Response::error(404, format!("no icon found for application '{id}'")),
    }
}

/// Looks the desktop id up (with or without `.desktop`), resolves its icon
/// through the icon theme and rasterizes it to `size` pixels.
fn render_app_icon(apps: &[AppEntry], id: &str, size: u32) -> Option<Vec<u8>> {
    let app = apps
        .iter()
        .find(|app| app.id == id || app.id.trim_end_matches(".desktop") == id)?;
    let path = providers::resolve_icon(app.icon.as_deref()?, size)?;
    let bytes = std::fs::read(path).ok()?;
    pixmap_png(&nexus_render::icons::decode_image(&bytes, size)?)
}

/// PNG of a tiny-skia pixmap, whose pixels are premultiplied while PNG
/// stores straight alpha.
fn pixmap_png(pixmap: &Pixmap) -> Option<Vec<u8>> {
    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|px| {
            let c = px.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    let image = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

fn geocode(request: &Request) -> Response {
    let query = request.query_param("q").unwrap_or_default();
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
        return Response::error(400, format!("q must have 1..{MAX_QUERY_CHARS} characters"));
    }
    let lang = match request.query_param("lang").as_deref() {
        None | Some("en") => "en",
        Some("de") => "de",
        Some(_) => return Response::error(400, "lang must be en or de"),
    };
    match providers::geocode(query, lang) {
        Ok(places) => {
            let places: Vec<Value> = places
                .iter()
                .map(|place| {
                    json!({
                        "name": place.name,
                        "admin1": place.admin1,
                        "country": place.country,
                        "latitude": place.latitude,
                        "longitude": place.longitude,
                    })
                })
                .collect();
            Response::json(200, &json!({ "ok": true, "places": places }))
        }
        Err(error) => Response::error(502, format!("place search failed: {error}")),
    }
}

fn weather(request: &Request) -> Response {
    let coordinate = |key: &str, limit: f64| {
        request
            .query_param(key)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && value.abs() <= limit)
    };
    let (Some(lat), Some(lon)) = (coordinate("lat", 90.0), coordinate("lon", 180.0)) else {
        return Response::error(400, "lat must be within ±90 and lon within ±180");
    };
    match providers::fetch_weather(lat, lon) {
        Ok(report) => Response::json(
            200,
            &json!({
                "ok": true,
                "weather": {
                    "temp_c": report.temp_c,
                    "code": report.code,
                    "is_day": report.is_day,
                    "high_c": report.high_c,
                    "low_c": report.low_c,
                },
            }),
        ),
        Err(error) => Response::error(502, format!("weather lookup failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handle;
    use crate::state::testing::{get, json, state};

    #[test]
    fn icon_catalog_lists_lucide_with_markup() {
        let response = handle(&get("/api/icons"), &state());
        assert_eq!(response.cache_control, "max-age=86400");
        let body = json(&response);
        let icons = body["icons"].as_array().unwrap();
        assert!(icons.len() > 500, "{} icons", icons.len());
        let globe = icons
            .iter()
            .find(|icon| icon[0] == "globe")
            .expect("globe icon");
        assert!(globe[2].as_str().unwrap().contains('<'), "svg markup");
    }

    #[test]
    fn apps_always_offer_the_desktop_defaults() {
        let body = json(&handle(&get("/api/apps"), &state()));
        assert_eq!(body["ok"], true);
        assert!(body["apps"].is_array());
        assert_eq!(body["defaults"][0]["id"], "default:browser");
    }

    #[test]
    fn app_icon_validates_and_404s_unknown_ids() {
        let state = state();
        assert_eq!(handle(&get("/api/app-icon"), &state).status, 400);
        assert_eq!(
            handle(&get("/api/app-icon?id=x&size=50"), &state).status,
            400
        );
        assert_eq!(
            handle(&get("/api/app-icon?id=no.such.app&size=32"), &state).status,
            404
        );
        // The miss is cached and still answers 404.
        assert_eq!(
            handle(&get("/api/app-icon?id=no.such.app&size=32"), &state).status,
            404
        );
    }

    #[test]
    fn pixmap_png_stores_straight_alpha() {
        let mut pixmap = Pixmap::new(2, 1).unwrap();
        pixmap.pixels_mut()[0] =
            nexus_render::tiny_skia::ColorU8::from_rgba(200, 100, 50, 128).premultiply();
        let png = pixmap_png(&pixmap).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        let px = image.get_pixel(0, 0).0;
        assert_eq!(px[3], 128);
        assert!((i16::from(px[0]) - 200).abs() <= 2, "{px:?}");
    }

    #[test]
    fn place_and_weather_queries_are_validated() {
        let state = state();
        for path in [
            "/api/geocode",
            "/api/geocode?q=%20",
            "/api/geocode?q=Berlin&lang=fr",
        ] {
            assert_eq!(handle(&get(path), &state).status, 400, "{path}");
        }
        for path in [
            "/api/weather?lat=91&lon=0",
            "/api/weather?lat=1",
            "/api/weather?lat=NaN&lon=1",
        ] {
            assert_eq!(handle(&get(path), &state).status, 400, "{path}");
        }
    }
}
