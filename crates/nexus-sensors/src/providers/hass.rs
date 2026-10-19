//! Home Assistant through one server connection: list every entity once and
//! let the panel toggle lights, run scenes or press buttons, instead of
//! configuring URL and token per entity.

use super::http::{http_request, HttpResponse};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const HASS_TIMEOUT: Duration = Duration::from_secs(5);

/// The Home Assistant instance (`http://homeassistant.local:8123`) and a
/// long-lived access token from the user's profile page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeAssistantServer {
    pub url: String,
    pub token: String,
}

impl HomeAssistantServer {
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.url.trim().trim_end_matches('/'))
    }

    fn headers(&self) -> Vec<String> {
        vec![
            format!("Authorization: Bearer {}", self.token.trim()),
            "Accept: application/json".into(),
            "Content-Type: application/json".into(),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity_id: String,
    pub state: String,
    pub friendly_name: Option<String>,
    pub unit: Option<String>,
}

/// All entity states (`GET /api/states`). The token reaches curl through
/// stdin, never its command line, so other users cannot read it from `ps`.
pub fn fetch_states(server: &HomeAssistantServer) -> Result<Vec<EntityState>, String> {
    let response = request(server, "GET", "/api/states", None)?;
    parse_states(&String::from_utf8_lossy(&response.body))
}

pub fn parse_states(json: &str) -> Result<Vec<EntityState>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("Home Assistant sent unreadable states: {error}"))?;
    let items = value
        .as_array()
        .ok_or("Home Assistant states are not a list")?;
    let text = |item: &serde_json::Value, key: &str| {
        item.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
    };
    let mut states: Vec<EntityState> = items
        .iter()
        .filter_map(|item| {
            let attributes = item.get("attributes").unwrap_or(&serde_json::Value::Null);
            Some(EntityState {
                entity_id: text(item, "entity_id")?,
                state: text(item, "state").unwrap_or_default(),
                friendly_name: text(attributes, "friendly_name"),
                unit: text(attributes, "unit_of_measurement"),
            })
        })
        .collect();
    states.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
    Ok(states)
}

/// Calls a service (`POST /api/services/<domain>/<service>`), optionally
/// targeting one entity.
pub fn call_service(
    server: &HomeAssistantServer,
    domain: &str,
    service: &str,
    entity_id: Option<&str>,
) -> Result<(), String> {
    // Both end up in the URL path; HA names are lowercase snake_case.
    for name in [domain, service] {
        if !is_service_name(name) {
            return Err(format!("\"{name}\" is not a Home Assistant service name"));
        }
    }
    let body = match entity_id {
        Some(entity_id) => serde_json::json!({ "entity_id": entity_id }),
        None => serde_json::json!({}),
    };
    request(
        server,
        "POST",
        &format!("/api/services/{domain}/{service}"),
        Some(&body.to_string()),
    )
    .map(|_| ())
}

/// The service a touch on `entity_id` should call, or None when a tap
/// should not change anything. Locks are deliberately left out: unlocking a
/// door by brushing the strip is not a toggle anyone wants.
pub fn toggle_service(entity_id: &str) -> Option<(&'static str, &'static str)> {
    let domain = entity_id.split_once('.')?.0;
    Some(match domain {
        "light" => ("light", "toggle"),
        "switch" => ("switch", "toggle"),
        "fan" => ("fan", "toggle"),
        "input_boolean" => ("input_boolean", "toggle"),
        "automation" => ("automation", "toggle"),
        "siren" => ("siren", "toggle"),
        "cover" => ("cover", "toggle"),
        "scene" => ("scene", "turn_on"),
        "script" => ("script", "turn_on"),
        "button" => ("button", "press"),
        "input_button" => ("input_button", "press"),
        "media_player" => ("media_player", "media_play_pause"),
        _ => return None,
    })
}

/// Whether an on/off-style state reads as "on" (lit, open, playing).
/// None for states that are not a switch position, such as `unavailable`,
/// `unknown` or a sensor value.
pub fn entity_is_on(state: &str) -> Option<bool> {
    match state.trim().to_ascii_lowercase().as_str() {
        "on" | "open" | "opening" | "playing" => Some(true),
        "off" | "closed" | "closing" | "paused" | "idle" | "standby" => Some(false),
        _ => None,
    }
}

fn is_service_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn request(
    server: &HomeAssistantServer,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<HttpResponse, String> {
    if server.url.trim().is_empty() || server.token.trim().is_empty() {
        return Err("Home Assistant needs an address and an access token".into());
    }
    let response = http_request(
        method,
        &server.endpoint(path),
        &server.headers(),
        body,
        HASS_TIMEOUT,
    )?;
    match response.status {
        _ if response.is_success() => Ok(response),
        401 | 403 => Err("Home Assistant rejected the access token".into()),
        404 => Err(format!("Home Assistant does not know {path}")),
        status => Err(format!("Home Assistant answered HTTP {status}")),
    }
}

/// The per-entity sensor's single-state fetch, with the token kept out of
/// argv like the server calls above. None on any failure.
pub(crate) fn fetch_entity_state_json(
    base_url: &str,
    entity_id: &str,
    token: &str,
) -> Option<serde_json::Value> {
    let server = HomeAssistantServer {
        url: base_url.into(),
        token: token.into(),
    };
    let headers = server.headers();
    let response = http_request(
        "GET",
        &server.endpoint(&format!("/api/states/{entity_id}")),
        &headers,
        None,
        HASS_TIMEOUT,
    )
    .ok()
    .filter(HttpResponse::is_success)?;
    serde_json::from_slice(&response.body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_list() {
        let json = r#"[
            {"entity_id":"sensor.living_room_temperature","state":"21.5",
             "attributes":{"friendly_name":"Living room","unit_of_measurement":"°C"},
             "last_changed":"2026-10-19T01:00:00+00:00"},
            {"entity_id":"light.desk","state":"on","attributes":{"friendly_name":"Desk lamp","brightness":180}},
            {"entity_id":"scene.movie","state":"unknown","attributes":{}},
            {"state":"broken entry without id"}
        ]"#;
        let states = parse_states(json).unwrap();
        assert_eq!(states.len(), 3);
        assert_eq!(
            states[0],
            EntityState {
                entity_id: "light.desk".into(),
                state: "on".into(),
                friendly_name: Some("Desk lamp".into()),
                unit: None,
            }
        );
        assert_eq!(states[2].unit.as_deref(), Some("°C"));
        assert_eq!(states[1].friendly_name, None);
        assert!(parse_states("{\"message\":\"401: Unauthorized\"}").is_err());
        assert!(parse_states("<html>").is_err());
    }

    #[test]
    fn maps_entities_to_touch_services() {
        assert_eq!(toggle_service("light.desk"), Some(("light", "toggle")));
        assert_eq!(
            toggle_service("input_boolean.guest"),
            Some(("input_boolean", "toggle"))
        );
        assert_eq!(toggle_service("cover.garage"), Some(("cover", "toggle")));
        assert_eq!(toggle_service("scene.movie"), Some(("scene", "turn_on")));
        assert_eq!(
            toggle_service("script.goodnight"),
            Some(("script", "turn_on"))
        );
        assert_eq!(
            toggle_service("input_button.ring"),
            Some(("input_button", "press"))
        );
        assert_eq!(
            toggle_service("media_player.kitchen"),
            Some(("media_player", "media_play_pause"))
        );
        assert_eq!(toggle_service("lock.front_door"), None);
        assert_eq!(toggle_service("sensor.temperature"), None);
        assert_eq!(toggle_service("nodomain"), None);
    }

    #[test]
    fn reads_on_off_states() {
        assert_eq!(entity_is_on("on"), Some(true));
        assert_eq!(entity_is_on("OFF"), Some(false));
        assert_eq!(entity_is_on("open"), Some(true));
        assert_eq!(entity_is_on("closed"), Some(false));
        assert_eq!(entity_is_on("playing"), Some(true));
        assert_eq!(entity_is_on("unavailable"), None);
        assert_eq!(entity_is_on("21.5"), None);
    }

    #[test]
    fn refuses_bad_service_names_and_missing_credentials() {
        let server = HomeAssistantServer {
            url: "http://127.0.0.1:1".into(),
            token: "token".into(),
        };
        assert!(call_service(&server, "light/../../api", "toggle", None).is_err());
        assert!(call_service(&server, "light", "Toggle", None).is_err());
        let empty = HomeAssistantServer {
            url: String::new(),
            token: String::new(),
        };
        assert!(fetch_states(&empty).is_err());
        assert_eq!(
            server.endpoint("/api/states"),
            "http://127.0.0.1:1/api/states"
        );
    }
}
