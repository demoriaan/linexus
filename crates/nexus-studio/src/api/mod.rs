//! Request dispatch: security gates first, then the `/api` routes of each
//! area, then static files.

pub mod catalog;
pub mod config;
pub mod daemon;
pub mod preview;
pub mod sensors;
pub mod system;

use crate::assets;
use crate::http::{self, Request, Response};
use crate::state::AppState;
use serde::de::DeserializeOwned;

/// Answers one request. Order matters: every request must name our own
/// loopback host. Webhook ingest is the one write that local programs
/// (curl, scripts) may send without the token, so it has its own gate
/// against browser pages; every other write must carry the token.
pub fn handle(request: &Request, state: &AppState) -> Response {
    if !http::host_allowed(request.header("host"), state.port) {
        return Response::error(
            421,
            "unexpected Host header; open Studio via 127.0.0.1 or localhost",
        );
    }
    if let Some(id) = sensors::webhook_ingest_id(request) {
        return sensors::ingest_webhook(id, request, &state.token);
    }
    if request.is_mutating() && !http::token_valid(request, &state.token) {
        return Response::error(403, "missing or invalid Linexus Studio write token");
    }
    if !matches!(request.method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
        return Response::error(405, "method not allowed");
    }
    if request.path.starts_with("/api/") {
        return route_api(request, state).unwrap_or_else(Response::not_found);
    }
    match request.method.as_str() {
        "GET" => assets::serve(state, request),
        _ => Response::not_found(),
    }
}

/// Largest body accepted for a request, decided from its head before the
/// body is read: webhook deliveries are small cards, the rest may carry
/// whole configs and packs.
pub fn body_limit(request: &Request) -> usize {
    if sensors::webhook_ingest_id(request).is_some() {
        sensors::MAX_WEBHOOK_BODY_BYTES
    } else {
        http::MAX_BODY_BYTES
    }
}

fn route_api(request: &Request, state: &AppState) -> Option<Response> {
    config::route(request, state)
        .or_else(|| preview::route(request, state))
        .or_else(|| daemon::route(request, state))
        .or_else(|| catalog::route(request, state))
        .or_else(|| sensors::route(request))
        .or_else(|| system::route(request, state))
}

/// Parses a JSON body, or answers 400 with serde's description of what is
/// wrong (field name, line and column).
pub(crate) fn parse_json<T: DeserializeOwned>(body: &[u8], what: &str) -> Result<T, Response> {
    serde_json::from_slice(body)
        .map_err(|error| Response::error(400, format!("invalid {what} JSON: {error}")))
}

/// One daemon IPC round trip with a readable error.
pub(crate) fn daemon_call(
    state: &AppState,
    command: nexus_ipc::Command,
) -> Result<nexus_ipc::Response, String> {
    nexus_ipc::send_request(&state.socket_path, command).map_err(|error| error.to_string())
}

/// Sends a command and maps the outcome: unreachable daemon → 503, daemon
/// refused → 502 with its message, success → `{ok:true, daemon}`.
pub(crate) fn daemon_reply(state: &AppState, command: nexus_ipc::Command) -> Response {
    match daemon_call(state, command) {
        Err(error) => Response::error(503, format!("daemon is not reachable: {error}")),
        Ok(reply) if !reply.ok => Response::json(
            502,
            &serde_json::json!({ "ok": false, "message": reply.message, "daemon": reply }),
        ),
        Ok(reply) => Response::json(200, &serde_json::json!({ "ok": true, "daemon": reply })),
    }
}

/// Asks the daemon to reload the config file; `None` when it is not
/// running (it reads the file on its next start anyway).
pub(crate) fn reload_daemon(state: &AppState) -> Option<nexus_ipc::Response> {
    let path = Some(state.config_path.display().to_string());
    daemon_call(state, nexus_ipc::Command::ReloadConfig { path }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::{get, json, post, request, state, temp_dir, PORT};

    #[test]
    fn unknown_api_paths_are_404_json() {
        let state = state();
        for req in [
            get("/api/nope"),
            post("/api/nope", b"{}"),
            get("/api/preview.png"),
        ] {
            let response = handle(&req, &state);
            assert_eq!(response.status, 404, "{}", req.path);
            assert_eq!(json(&response)["ok"], false);
        }
        assert_eq!(handle(&get("/missing.css"), &state).status, 404);
        assert_eq!(handle(&post("/index.html", b""), &state).status, 404);
    }

    #[test]
    fn foreign_host_is_rejected_before_routing() {
        let state = state();
        let mut req = get("/api/session");
        req.headers[0].1 = "evil.example:19777".into();
        assert_eq!(handle(&req, &state).status, 421);
        req.headers.remove(0);
        assert_eq!(handle(&req, &state).status, 421, "missing Host");
    }

    #[test]
    fn writes_without_token_are_forbidden() {
        let state = state();
        let mut req = post("/api/config", b"{}");
        req.headers.retain(|(name, _)| name == "host");
        let response = handle(&req, &state);
        assert_eq!(response.status, 403);
        assert_eq!(json(&response)["ok"], false);
        let mut wrong = post("/api/daemon/blank", b"");
        wrong.headers[2].1 = "guess".into();
        assert_eq!(handle(&wrong, &state).status, 403);
        assert!(!state.config_path.exists());
    }

    #[test]
    fn other_methods_are_not_allowed() {
        let state = state();
        assert_eq!(handle(&request("HEAD", "/", b""), &state).status, 405);
        assert_eq!(
            handle(&request("OPTIONS", "/api/config", b""), &state).status,
            405
        );
    }

    #[test]
    fn webhook_ingest_accepts_local_programs_for_configured_ids() {
        let dir = temp_dir("webhooks");
        let records = dir.join("records");
        let sensors_json = dir.join("sensors.json");
        std::fs::write(&sensors_json, r#"{"webhooks":[{"id":"door"}]}"#).unwrap();
        // Only this test stores webhooks or sets up sensors.json, so the
        // process-wide overrides are safe.
        std::env::set_var("NEXUS_SENSOR_WEBHOOK_DIR", &records);
        std::env::set_var("NEXUS_SENSOR_CONFIG", &sensors_json);
        let state = state();
        // What `curl -X POST http://127.0.0.1:<port>/api/sensors/webhook/<id>`
        // sends: our Host, no Origin, no token.
        let curl = |id: &str| {
            let mut req = post(
                &format!("/api/sensors/webhook/{id}"),
                br#"{"state":"open"}"#,
            );
            req.headers = vec![
                ("host".into(), format!("127.0.0.1:{PORT}")),
                ("content-type".into(), "application/json".into()),
            ];
            req
        };
        let response = handle(&curl("door"), &state);
        let text = String::from_utf8_lossy(&response.body).into_owned();
        assert_eq!(response.status, 201, "{text}");
        assert_eq!(json(&response)["ok"], true);
        assert!(records.join("door.json").is_file());

        // Names are matched the way they are stored.
        assert_eq!(handle(&curl("Door"), &state).status, 201);

        // Only webhooks set up in sensors.json store anything.
        assert_eq!(handle(&curl("x1"), &state).status, 404);
        assert!(!records.join("x1.json").exists());

        // DNS rebinding: a page on a foreign name that resolves to us.
        let mut foreign = curl("door");
        foreign.headers[0].1 = "evil.example:19777".into();
        assert_eq!(handle(&foreign, &state).status, 421);

        // Requests from web pages.
        for (name, value) in [
            ("origin", "https://evil.example"),
            ("origin", "null"),
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-site", "same-site"),
        ] {
            let mut req = curl("door");
            req.headers.push((name.into(), value.into()));
            assert_eq!(handle(&req, &state).status, 403, "{name}: {value}");
        }
        // Studio's own "Send a test" carries the token.
        let mut studio = post("/api/sensors/webhook/door", br#"{"title":"Test"}"#);
        studio
            .headers
            .push(("origin".into(), format!("http://127.0.0.1:{PORT}")));
        studio
            .headers
            .push(("sec-fetch-site".into(), "same-origin".into()));
        assert_eq!(handle(&studio, &state).status, 201);

        let mut big = curl("door");
        big.body = vec![b'a'; sensors::MAX_WEBHOOK_BODY_BYTES + 1];
        assert_eq!(handle(&big, &state).status, 413);
        assert_eq!(body_limit(&curl("door")), sensors::MAX_WEBHOOK_BODY_BYTES);
        assert_eq!(body_limit(&post("/api/config", b"")), http::MAX_BODY_BYTES);
        let delete = request("DELETE", "/api/sensors/webhook/door", b"");
        assert_eq!(body_limit(&delete), http::MAX_BODY_BYTES);

        // Deleting is a Studio action and stays protected.
        let mut delete = delete;
        assert_eq!(handle(&delete, &state).status, 200);
        assert!(!records.join("door.json").exists());
        delete.headers.truncate(1);
        assert_eq!(handle(&delete, &state).status, 403);
    }

    #[test]
    fn session_and_version_answer() {
        let state = state();
        assert_eq!(
            json(&handle(&get("/api/session"), &state))["token"],
            "test-token"
        );
        let version = json(&handle(&get("/api/version"), &state));
        assert_eq!(version["version"], env!("CARGO_PKG_VERSION"));
    }
}
