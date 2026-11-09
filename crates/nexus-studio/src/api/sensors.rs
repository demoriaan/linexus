//! Sensors and integrations: sensors.json (feeds, webhooks, IFTTT, MQTT),
//! RSS read state, webhook ingest, integration tests and the Home Assistant
//! server.

use super::parse_json;
use crate::http::{self, Request, Response};
use crate::providers::{self, HomeAssistantServer};
use serde::Deserialize;
use serde_json::{json, Value};

/// Fields Studio edits through dedicated endpoints. The browser only ever
/// has their redacted copy, so a full sensors.json save keeps what is stored.
const PRESERVED_FIELDS: [&str; 3] = ["discord", "home_assistant_server", "obs_server"];

/// Secrets inside the lists the browser edits: `(section, field, fields that
/// say where the secret is sent)`.
const LIST_SECRETS: [(&str, &str, &[&str]); 3] = [
    ("home_assistant", "token", &["base_url"]),
    ("ifttt", "key", &[]),
    ("mqtt", "password", &["host", "port"]),
];

pub fn route(request: &Request) -> Option<Response> {
    let path = request.path.as_str();
    let response = match (request.method.as_str(), path) {
        ("GET", "/api/sensors/config") => config(),
        ("POST", "/api/sensors/config") => save_config(&request.body),
        ("POST", "/api/sensors/news") => save_news(&request.body),
        ("POST", "/api/sensors/rss/inspect") => inspect_feed(&request.body),
        ("POST", "/api/sensors/rss/mark-read") => mark_rss_read(),
        ("POST", "/api/sensors/rss/reset-read") => reset_rss_read(),
        ("POST", "/api/home-assistant/test") => test_home_assistant(&request.body),
        ("GET", "/api/home-assistant/entities") => home_assistant_entities(),
        ("POST", "/api/home-assistant/server") => save_home_assistant_server(&request.body),
        ("DELETE", _) => delete_webhook(path.strip_prefix("/api/sensors/webhook/")?),
        ("POST", _) => test_integration(integration_target(path)?),
        _ => return None,
    };
    Some(response)
}

/// Largest webhook delivery: a card is a title, a link and a little text.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 64 * 1024;

/// Id of a webhook delivery (`POST`/`PUT /api/sensors/webhook/<id>`), the
/// one write open to local programs without the Studio token.
pub fn webhook_ingest_id(request: &Request) -> Option<&str> {
    if !matches!(request.method.as_str(), "POST" | "PUT") {
        return None;
    }
    request.path.strip_prefix("/api/sensors/webhook/")
}

/// Stores a delivery for a webhook set up in sensors.json. Programs such as
/// curl send neither `Origin` nor `Sec-Fetch-Site`; browsers send them with
/// every POST, so any web page the user visits could otherwise post here.
/// Page requests are refused unless they come from Studio itself with its
/// token (the "Send a test" button).
pub fn ingest_webhook(id: &str, request: &Request, token: &str) -> Response {
    let site = request
        .header("sec-fetch-site")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let from_page = request.header("origin").is_some() && !http::token_valid(request, token);
    if from_page || matches!(site.as_str(), "cross-site" | "same-site") {
        return Response::error(
            403,
            "webhooks accept requests from programs on this computer, not from web pages",
        );
    }
    if request.body.len() > MAX_WEBHOOK_BODY_BYTES {
        return Response::error(413, "webhook payloads are limited to 64 KiB");
    }
    // Names are stored normalised (lower case, `-` for other characters), so
    // `/webhook/Build` reaches the webhook listed as `build`.
    let id = nexus_sensors::sanitize_sensor_id(&String::from_utf8_lossy(&http::percent_decode(
        id, false,
    )));
    let config = nexus_sensors::load_sensor_config();
    if !config.webhooks.iter().any(|rule| rule.id == id) {
        return Response::error(
            404,
            format!("there is no webhook named '{id}'; add it under Connections in Linexus Studio"),
        );
    }
    match nexus_sensors::record_webhook_payload(&id, request.header("content-type"), &request.body)
    {
        Ok(record) => Response::json(
            201,
            &json!({ "ok": true, "message": "webhook sensor stored", "record": record }),
        ),
        Err(error) => Response::error(
            500,
            format!("failed to store webhook sensor '{id}': {error}"),
        ),
    }
}

fn delete_webhook(id: &str) -> Response {
    match nexus_sensors::delete_webhook_record(id) {
        Ok(deleted) => Response::json(200, &json!({ "ok": true, "id": id, "deleted": deleted })),
        Err(error) => Response::error(
            500,
            format!("failed to delete webhook sensor '{id}': {error}"),
        ),
    }
}

fn sensor_config_path() -> String {
    nexus_sensors::sensor_config_path().display().to_string()
}

fn config() -> Response {
    let config = nexus_sensors::load_sensor_config();
    let mut body = json!({
        "ok": true,
        "path": sensor_config_path(),
        "feed_titles": nexus_sensors::cached_feed_titles(&config.rss),
        "config": public_config(&config),
        "webhook_records": nexus_sensors::list_webhook_records(),
        "rss_seen_count": nexus_sensors::load_rss_seen_state().seen.len(),
    });
    // Set when sensors.json exists but does not parse.
    if let Some(problem) = nexus_sensors::sensor_config_problem() {
        body["config_error"] = problem.into();
    }
    Response::json(200, &body)
}

fn save_config(body: &[u8]) -> Response {
    let incoming: Value = match parse_json(body, "sensor config") {
        Ok(value) => value,
        Err(response) => return response,
    };
    // Read, merge and write under one lock: the daemon may store a renewed
    // Discord token at any moment. A file that does not parse is refused,
    // since the browser's copy would replace everything in it.
    let updated = nexus_sensors::update_sensor_config(|stored| {
        let mut incoming = incoming;
        let current = serde_json::to_value(&*stored).unwrap_or(Value::Null);
        preserve_fields(&mut incoming, &current);
        *stored = serde_json::from_value(incoming)
            .map_err(|error| format!("invalid sensor config: {error}"))?;
        Ok::<_, String>(())
    });
    match updated {
        Ok(Ok(())) => saved(
            nexus_sensors::load_sensor_config(),
            "Sensor configuration saved",
        ),
        Ok(Err(problem)) => Response::error(400, problem),
        Err(error) => Response::error(500, format!("failed to save sensor config: {error}")),
    }
}

/// Replaces a secret with `has_<field>`: the page can say one is saved.
fn hide_secret(object: &mut serde_json::Map<String, Value>, field: &str) {
    let saved = object
        .remove(field)
        .is_some_and(|value| value.as_str().is_some_and(|text| !text.is_empty()));
    object.insert(format!("has_{field}"), saved.into());
}

/// sensors.json as the browser sees it: no passwords, keys or tokens, only
/// whether one is saved, and for Discord who authorized and until when.
pub(crate) fn public_config(config: &nexus_sensors::SensorConfig) -> Value {
    let mut value = serde_json::to_value(config).unwrap_or(Value::Null);
    for (section, field) in [
        ("home_assistant_server", "token"),
        ("obs_server", "password"),
    ] {
        if let Some(server) = value.get_mut(section).and_then(Value::as_object_mut) {
            hide_secret(server, field);
        }
    }
    for (section, field, _) in LIST_SECRETS {
        let items = value.get_mut(section).and_then(Value::as_array_mut);
        for item in items.into_iter().flatten().filter_map(Value::as_object_mut) {
            hide_secret(item, field);
        }
    }
    if let Some(discord) = value.get_mut("discord").and_then(Value::as_object_mut) {
        discord.remove("client_secret");
        if let Some(token) = discord.get_mut("token").and_then(Value::as_object_mut) {
            token.remove("access_token");
            token.remove("refresh_token");
        }
    }
    value
}

fn preserve_fields(incoming: &mut Value, stored: &Value) {
    let (Some(incoming), Some(stored)) = (incoming.as_object_mut(), stored.as_object()) else {
        return;
    };
    for key in PRESERVED_FIELDS {
        match stored.get(key) {
            Some(value) => {
                incoming.insert(key.to_string(), value.clone());
            }
            None => {
                incoming.remove(key);
            }
        }
    }
    for (section, field, target) in LIST_SECRETS {
        let empty = Vec::new();
        let saved = stored
            .get(section)
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let items = incoming.get_mut(section).and_then(Value::as_array_mut);
        for item in items.into_iter().flatten().filter_map(Value::as_object_mut) {
            restore_secret(item, saved, field, target);
        }
    }
}

/// A row without its secret keeps the stored one, unless it now points
/// somewhere else: a stored password must not follow a changed host.
/// `previous_id` names the row when the page renamed it.
fn restore_secret(
    item: &mut serde_json::Map<String, Value>,
    saved: &[Value],
    field: &str,
    target: &[&str],
) {
    item.remove(&format!("has_{field}"));
    let previous = item.remove("previous_id");
    if item.contains_key(field) {
        return;
    }
    // A page that still carries an old `previous_id` falls back to the id.
    let id = item.get("id");
    let secret = saved
        .iter()
        .find(|row| previous.is_some() && row.get("id") == previous.as_ref())
        .or_else(|| saved.iter().find(|row| row.get("id") == id))
        .filter(|row| target.iter().all(|key| row.get(*key) == item.get(*key)))
        .and_then(|row| row.get(field))
        .cloned();
    item.insert(field.to_string(), secret.unwrap_or_else(|| "".into()));
}

fn saved(config: nexus_sensors::SensorConfig, message: &str) -> Response {
    Response::json(
        200,
        &json!({ "ok": true, "path": sensor_config_path(), "config": public_config(&config), "message": message }),
    )
}

#[derive(Deserialize)]
struct NewsRequest {
    #[serde(default)]
    rss: Vec<nexus_sensors::RssFeedConfig>,
    #[serde(default)]
    display: nexus_sensors::NewsDisplayConfig,
}

/// Saves only feeds and headline display settings; every other module in
/// sensors.json stays exactly as stored.
fn save_news(body: &[u8]) -> Response {
    let request: NewsRequest = match parse_json(body, "news settings") {
        Ok(request) => request,
        Err(response) => return response,
    };
    match nexus_sensors::update_news_config(request.rss, request.display) {
        Ok(config) => saved(config, "News settings saved"),
        Err(error) => Response::error(500, format!("failed to save news settings: {error}")),
    }
}

#[derive(Deserialize)]
struct InspectRequest {
    url: String,
}

/// Downloads an address the user typed and reports the feed behind it, or
/// a readable reason why there is none (422).
fn inspect_feed(body: &[u8]) -> Response {
    let request: InspectRequest = match parse_json(body, "feed inspect") {
        Ok(request) => request,
        Err(response) => return response,
    };
    match nexus_sensors::inspect_feed_url(&request.url) {
        Ok(feed) => Response::json(200, &json!({ "ok": true, "feed": feed })),
        Err(error) => Response::json(
            422,
            &json!({ "ok": false, "message": error, "error": error }),
        ),
    }
}

fn mark_rss_read() -> Response {
    let metrics = nexus_sensors::collect_metrics();
    let cards = nexus_sensors::configured_sensor_cards(&metrics.sensor_records);
    let ids = cards
        .iter()
        .filter(|card| card.module == "rss")
        .map(|card| card.id.as_str());
    if let Err(error) = nexus_sensors::mark_rss_items_seen(ids) {
        return Response::error(500, format!("failed to persist RSS read state: {error}"));
    }
    let seen_count = nexus_sensors::load_rss_seen_state().seen.len();
    let body =
        json!({ "ok": true, "message": "Current RSS items marked read", "seen_count": seen_count });
    Response::json(200, &body)
}

fn reset_rss_read() -> Response {
    match nexus_sensors::reset_rss_seen_state() {
        Ok(()) => Response::json(
            200,
            &json!({ "ok": true, "message": "RSS read state reset", "seen_count": 0 }),
        ),
        Err(error) => Response::error(500, format!("failed to reset RSS read state: {error}")),
    }
}

/// `(kind, id)` from `/api/sensors/integrations/<kind>/<id>` with an
/// optional trailing `/test` (the older frontend's spelling).
fn integration_target(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/api/sensors/integrations/")?;
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [kind, id] | [kind, id, "test"] if !kind.is_empty() && !id.is_empty() => Some((kind, id)),
        _ => None,
    }
}

/// Sends a sample card through a configured IFTTT or MQTT integration.
fn test_integration((kind, id): (&str, &str)) -> Response {
    let config = nexus_sensors::load_sensor_config();
    let metrics = nexus_sensors::collect_metrics();
    let card = nexus_sensors::sample_integration_card(&metrics.sensor_records);
    let result = match kind {
        "ifttt" => {
            let Some(integration) = config.ifttt.iter().find(|item| item.id == id) else {
                return Response::error(404, format!("IFTTT integration '{id}' not found"));
            };
            nexus_sensors::send_ifttt_webhook(integration, &card).map(|request| {
                (
                    "IFTTT webhook sent",
                    json!(redacted_ifttt_request(&request)),
                )
            })
        }
        "mqtt" => {
            let Some(integration) = config.mqtt.iter().find(|item| item.id == id) else {
                return Response::error(404, format!("MQTT integration '{id}' not found"));
            };
            nexus_sensors::publish_mqtt_message(integration, &card)
                .map(|message| ("MQTT message published", json!(message)))
        }
        _ => return Response::not_found(),
    };
    match result {
        Ok((message, integration)) => Response::json(
            200,
            &json!({ "ok": true, "message": message, "card": card.title, "integration": integration }),
        ),
        Err(error) => Response::error(502, format!("{kind} test failed: {error}")),
    }
}

/// The IFTTT key is a secret; it never goes back to the browser.
fn redacted_ifttt_request(request: &nexus_sensors::IftttRequest) -> nexus_sensors::IftttRequest {
    let mut redacted = request.clone();
    if let Some((prefix, _)) = redacted.url.rsplit_once("/with/key/") {
        redacted.url = format!("{prefix}/with/key/<redacted>");
    }
    redacted
}

#[derive(Deserialize)]
struct ServerRequest {
    url: String,
    #[serde(default)]
    token: String,
}

/// Normalized server: http(s) URL without trailing slash, non-empty token.
/// An empty token means the saved one, for the address it was saved for.
fn checked_server(
    request: ServerRequest,
    stored: Option<&HomeAssistantServer>,
) -> Result<HomeAssistantServer, Response> {
    let url = request.url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Response::error(
            400,
            "url must start with http:// or https://",
        ));
    }
    let mut token = request.token.trim().to_string();
    if token.is_empty() {
        match stored.filter(|server| server.url == url) {
            Some(server) => token.clone_from(&server.token),
            None => return Err(Response::error(400, "enter the access token")),
        }
    }
    Ok(HomeAssistantServer { url, token })
}

fn test_home_assistant(body: &[u8]) -> Response {
    let stored = providers::home_assistant_server();
    let parsed = parse_json(body, "Home Assistant");
    let server = match parsed.and_then(|request| checked_server(request, stored.as_ref())) {
        Ok(server) => server,
        Err(response) => return response,
    };
    match providers::fetch_states(&server) {
        Ok(states) => {
            let n = states.len();
            let message = format!("Connected to Home Assistant: {n} entities");
            Response::json(
                200,
                &json!({ "ok": true, "entities": n, "message": message }),
            )
        }
        Err(error) => Response::error(502, format!("Home Assistant did not answer: {error}")),
    }
}

/// Entities of the saved server, sorted by id, for the entity picker.
fn home_assistant_entities() -> Response {
    let Some(server) = providers::home_assistant_server() else {
        return Response::error(404, "Home Assistant is not connected; save a server first");
    };
    let mut states = match providers::fetch_states(&server) {
        Ok(states) => states,
        Err(error) => {
            return Response::error(502, format!("Home Assistant did not answer: {error}"))
        }
    };
    states.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
    let entities: Vec<Value> = states
        .iter()
        .map(|state| {
            let domain = state.entity_id.split('.').next().unwrap_or_default();
            let name = state.friendly_name.as_deref().unwrap_or(&state.entity_id);
            json!({
                "entity_id": state.entity_id,
                "name": name,
                "state": state.state,
                "unit": state.unit,
                "domain": domain,
            })
        })
        .collect();
    Response::json(200, &json!({ "ok": true, "entities": entities }))
}

/// `{url, token}` stores the server, `null` removes it.
fn save_home_assistant_server(body: &[u8]) -> Response {
    let request: Option<ServerRequest> = match parse_json(body, "Home Assistant server") {
        Ok(request) => request,
        Err(response) => return response,
    };
    let stored = providers::home_assistant_server();
    let server = match request
        .map(|request| checked_server(request, stored.as_ref()))
        .transpose()
    {
        Ok(server) => server,
        Err(response) => return response,
    };
    let message = if server.is_some() {
        "Home Assistant server saved"
    } else {
        "Home Assistant server removed"
    };
    match providers::save_home_assistant_server(server) {
        Ok(()) => Response::json(200, &json!({ "ok": true, "message": message })),
        Err(error) => Response::error(500, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handle;
    use crate::state::testing::{post, state};

    #[test]
    fn redacts_ifttt_key_from_test_response() {
        let request = nexus_sensors::IftttRequest {
            url: "https://maker.ifttt.com/trigger/nexus/with/key/secret-key".into(),
            body: serde_json::json!({ "value1": "hello" }),
        };
        let redacted = redacted_ifttt_request(&request);
        assert_eq!(
            redacted.url,
            "https://maker.ifttt.com/trigger/nexus/with/key/<redacted>"
        );
        assert_eq!(redacted.body, request.body);
    }

    #[test]
    fn integration_paths_accept_the_optional_test_suffix() {
        let target = |path| integration_target(path);
        assert_eq!(
            target("/api/sensors/integrations/mqtt/office"),
            Some(("mqtt", "office"))
        );
        assert_eq!(
            target("/api/sensors/integrations/ifttt/door/test"),
            Some(("ifttt", "door"))
        );
        assert_eq!(
            target("/api/sensors/integrations/mqtt/test"),
            Some(("mqtt", "test"))
        );
        assert_eq!(target("/api/sensors/integrations/mqtt"), None);
        assert_eq!(target("/api/sensors/integrations/mqtt/a/b"), None);
        let state = state();
        let unknown = handle(&post("/api/sensors/integrations/smoke/x/test", b""), &state);
        assert_eq!(unknown.status, 404);
    }

    #[test]
    fn full_saves_keep_fields_managed_elsewhere() {
        let stored =
            json!({ "rss": [], "home_assistant_server": { "url": "http://ha", "token": "t" } });
        let mut without = json!({ "rss": [{ "id": "a" }] });
        preserve_fields(&mut without, &stored);
        assert_eq!(without["home_assistant_server"]["url"], "http://ha");
        // The page holds only a redacted copy; that must not replace it.
        let mut redacted =
            json!({ "home_assistant_server": { "url": "http://evil", "has_token": true } });
        preserve_fields(&mut redacted, &stored);
        assert_eq!(
            redacted["home_assistant_server"],
            stored["home_assistant_server"]
        );
    }

    fn with_secrets() -> nexus_sensors::SensorConfig {
        serde_json::from_value(json!({
            "home_assistant_server": { "url": "http://ha", "token": "ha-token" },
            "obs_server": { "host": "localhost", "port": 4455, "password": "obs-password" },
            "home_assistant": [{ "id": "desk", "base_url": "http://ha", "entity_id": "light.desk", "token": "entity-token" }],
            "ifttt": [{ "id": "phone", "event": "e", "key": "ifttt-key" }],
            "mqtt": [{ "id": "broker", "host": "a", "port": 1883, "topic": "t", "password": "mqtt-password" }],
        }))
        .unwrap()
    }

    #[test]
    fn no_secret_reaches_the_page() {
        let public = public_config(&with_secrets());
        let text = public.to_string();
        for secret in [
            "ha-token",
            "entity-token",
            "obs-password",
            "ifttt-key",
            "mqtt-password",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert_eq!(public["home_assistant_server"]["has_token"], true);
        assert_eq!(public["obs_server"]["has_password"], true);
        assert_eq!(public["ifttt"][0]["has_key"], true);
        assert_eq!(public["mqtt"][0]["has_password"], true);
    }

    #[test]
    fn saving_the_redacted_copy_keeps_every_secret() {
        let config = with_secrets();
        let stored = serde_json::to_value(&config).unwrap();
        let mut incoming = public_config(&config);
        preserve_fields(&mut incoming, &stored);
        let back: nexus_sensors::SensorConfig = serde_json::from_value(incoming).unwrap();
        assert_eq!(back.home_assistant[0].token, "entity-token");
        assert_eq!(back.ifttt[0].key, "ifttt-key");
        assert_eq!(back.mqtt[0].password, "mqtt-password");
        assert_eq!(back.home_assistant_server.unwrap().token, "ha-token");
        assert_eq!(back.obs_server.unwrap().password, "obs-password");
    }

    #[test]
    fn a_stored_password_follows_a_rename_but_not_a_new_host() {
        let config = with_secrets();
        let stored = serde_json::to_value(&config).unwrap();
        let mut incoming = public_config(&config);
        incoming["mqtt"][0]["id"] = "renamed".into();
        incoming["mqtt"][0]["previous_id"] = "broker".into();
        preserve_fields(&mut incoming, &stored);
        assert_eq!(incoming["mqtt"][0]["password"], "mqtt-password");
        assert!(incoming["mqtt"][0].get("previous_id").is_none());
        // A second save of the renamed row, with its old name still attached.
        let after_rename =
            json!({ "mqtt": [{ "id": "renamed", "host": "a", "port": 1883, "password": "kept" }] });
        let mut again = json!({ "mqtt": [{ "id": "renamed", "host": "a", "port": 1883, "previous_id": "broker" }] });
        preserve_fields(&mut again, &after_rename);
        assert_eq!(again["mqtt"][0]["password"], "kept");
        let mut moved = public_config(&config);
        moved["mqtt"][0]["host"] = "evil.example".into();
        preserve_fields(&mut moved, &stored);
        assert_eq!(moved["mqtt"][0]["password"], "");
        // A typed value replaces the stored one.
        let mut typed = public_config(&config);
        typed["mqtt"][0]["password"] = "new".into();
        preserve_fields(&mut typed, &stored);
        assert_eq!(typed["mqtt"][0]["password"], "new");
    }

    #[test]
    fn an_empty_token_means_the_saved_one_for_its_address_only() {
        let stored = HomeAssistantServer {
            url: "http://ha".into(),
            token: "saved".into(),
        };
        let request = |url: &str| ServerRequest {
            url: url.into(),
            token: String::new(),
        };
        let same = checked_server(request("http://ha/"), Some(&stored))
            .ok()
            .unwrap();
        assert_eq!(same.token, "saved");
        let other = checked_server(request("http://evil.example"), Some(&stored));
        assert_eq!(other.err().unwrap().status, 400);
        assert_eq!(
            checked_server(request("http://ha"), None)
                .err()
                .unwrap()
                .status,
            400
        );
    }

    #[test]
    fn home_assistant_input_is_validated() {
        let state = state();
        for (path, body) in [
            (
                "/api/home-assistant/test",
                &br#"{"url":"ftp://ha","token":"t"}"#[..],
            ),
            (
                "/api/home-assistant/test",
                br#"{"url":"http://ha","token":" "}"#,
            ),
            (
                "/api/home-assistant/server",
                br#"{"url":"ha.local","token":"t"}"#,
            ),
            ("/api/home-assistant/server", b""),
        ] {
            let response = handle(&post(path, body), &state);
            assert_eq!(
                response.status,
                400,
                "{path} {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn discord_tokens_never_reach_the_page_or_get_replaced_by_it() {
        let config = nexus_sensors::SensorConfig {
            discord: Some(nexus_sensors::DiscordApp {
                client_id: "42".into(),
                client_secret: "secret".into(),
                token: Some(nexus_sensors::DiscordToken {
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    expires_at: 1,
                    user: "Axel".into(),
                    scopes: Vec::new(),
                }),
            }),
            ..nexus_sensors::SensorConfig::default()
        };
        let public = public_config(&config);
        assert_eq!(
            public["discord"]["token"],
            json!({ "expires_at": 1, "user": "Axel" })
        );
        assert!(public["discord"].get("client_secret").is_none());
        let stored = serde_json::to_value(&config).unwrap();
        // The page sends back its redacted copy, or a forged one.
        let mut incoming = public.clone();
        incoming["discord"]["client_id"] = "666".into();
        preserve_fields(&mut incoming, &stored);
        assert_eq!(incoming["discord"], stored["discord"]);
        let mut without = json!({ "discord": { "client_id": "1" } });
        preserve_fields(&mut without, &json!({}));
        assert!(without.get("discord").is_none());
    }
}
