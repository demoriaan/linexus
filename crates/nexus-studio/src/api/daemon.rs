//! Daemon control: status and commands proxied over nexus-ipc, plus the
//! fixed `systemctl --user` operations on the daemon's unit. No request can
//! choose the program, the unit or arbitrary arguments.

use super::{daemon_call, daemon_reply, parse_json};
use crate::http::{Request, Response};
use crate::state::AppState;
use nexus_ipc::{Command, TouchPhase};
use serde::Deserialize;
use serde_json::json;
use std::process::Command as Process;

pub const DAEMON_UNIT: &str = "linexusd.service";
const STRIP_WIDTH: u16 = nexus_config::STRIP_WIDTH;
/// Longest test notification; the strip shows one line anyway.
const MAX_NOTIFY_CHARS: usize = 500;

pub fn route(request: &Request, state: &AppState) -> Option<Response> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/daemon/status") => Some(status(state)),
        ("POST", path) => {
            let operation = path.strip_prefix("/api/daemon/")?;
            command(state, operation, &request.body)
        }
        _ => None,
    }
}

fn command(state: &AppState, operation: &str, body: &[u8]) -> Option<Response> {
    let response = match operation {
        "touch" => touch(state, body),
        "tap" => tap(state, body),
        "page" => page(state, body),
        "profile" => profile(state, body),
        "brightness" => brightness(state, body),
        "blank" => daemon_reply(state, Command::Blank),
        "restore" => daemon_reply(state, Command::Restore),
        "notify" => notify(state, body),
        "start" | "restart" | "stop" => service(state, operation),
        "autostart" => autostart(state, body),
        _ => return None,
    };
    Some(response)
}

/// Always 200: "not running" is a valid answer, not a failed request.
fn status(state: &AppState) -> Response {
    let body = match daemon_call(state, Command::Status) {
        Ok(reply) => json!({
            "ok": true,
            "reachable": true,
            "status": reply.status,
            "message": reply.message,
        }),
        Err(error) => json!({ "ok": true, "reachable": false, "status": null, "message": error }),
    };
    Response::json(200, &body)
}

/// Strip X coordinate, rejected before any IPC when outside 0..640.
fn checked_x(x: u16) -> Result<u16, Response> {
    if x < STRIP_WIDTH {
        Ok(x)
    } else {
        Err(Response::error(
            400,
            format!("x must be within 0..{}", STRIP_WIDTH - 1),
        ))
    }
}

#[derive(Deserialize)]
struct TouchRequest {
    phase: TouchPhase,
    x: u16,
}

fn touch(state: &AppState, body: &[u8]) -> Response {
    let request: TouchRequest = match parse_json(body, "touch") {
        Ok(request) => request,
        Err(response) => return response,
    };
    match checked_x(request.x) {
        Ok(x) => daemon_reply(
            state,
            Command::Touch {
                phase: request.phase,
                x,
            },
        ),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
struct TapRequest {
    x: u16,
}

fn tap(state: &AppState, body: &[u8]) -> Response {
    let request: TapRequest = match parse_json(body, "tap") {
        Ok(request) => request,
        Err(response) => return response,
    };
    match checked_x(request.x) {
        Ok(x) => daemon_reply(state, Command::Tap { x }),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
struct PageRequest {
    #[serde(default)]
    page_id: Option<String>,
    #[serde(default)]
    direction: Option<String>,
}

/// Either a page id or a direction, never both.
fn page_command(request: PageRequest) -> Result<Command, String> {
    match (request.page_id, request.direction.as_deref()) {
        (Some(page_id), None) if !page_id.trim().is_empty() => Ok(Command::GoToPage { page_id }),
        (None, Some("next")) => Ok(Command::PageNext),
        (None, Some("previous")) => Ok(Command::PagePrevious),
        _ => Err("send either a non-empty page_id or direction \"next\"/\"previous\"".into()),
    }
}

fn page(state: &AppState, body: &[u8]) -> Response {
    let request: PageRequest = match parse_json(body, "page") {
        Ok(request) => request,
        Err(response) => return response,
    };
    match page_command(request) {
        Ok(command) => daemon_reply(state, command),
        Err(message) => Response::error(400, message),
    }
}

#[derive(Deserialize)]
struct ProfileRequest {
    profile_id: String,
}

fn profile(state: &AppState, body: &[u8]) -> Response {
    let request: ProfileRequest = match parse_json(body, "profile") {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.profile_id.trim().is_empty() {
        return Response::error(400, "profile_id must not be empty");
    }
    daemon_reply(
        state,
        Command::SwitchProfile {
            profile_id: request.profile_id,
        },
    )
}

#[derive(Deserialize)]
struct BrightnessRequest {
    percent: u16,
}

fn brightness(state: &AppState, body: &[u8]) -> Response {
    let request: BrightnessRequest = match parse_json(body, "brightness") {
        Ok(request) => request,
        Err(response) => return response,
    };
    match u8::try_from(request.percent) {
        Ok(percent) if percent <= 100 => daemon_reply(state, Command::SetBrightness { percent }),
        _ => Response::error(400, "brightness percent must be within 0..100"),
    }
}

#[derive(Deserialize)]
struct NotifyRequest {
    text: String,
    #[serde(default)]
    icon: Option<String>,
}

fn notify(state: &AppState, body: &[u8]) -> Response {
    let request: NotifyRequest = match parse_json(body, "notify") {
        Ok(request) => request,
        Err(response) => return response,
    };
    let text = request.text.trim();
    if text.is_empty() || text.chars().count() > MAX_NOTIFY_CHARS {
        let message = format!("text must have 1..{MAX_NOTIFY_CHARS} characters");
        return Response::error(400, message);
    }
    let icon = request.icon.filter(|icon| !icon.trim().is_empty());
    daemon_reply(
        state,
        Command::Notify {
            text: text.to_string(),
            icon,
        },
    )
}

/// Outcome of one `systemctl --user` call.
pub struct Systemctl {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `systemctl --user <args>`; a missing systemctl counts as failure
/// with the spawn error in `stderr`.
pub fn systemctl_user(args: &[&str]) -> Systemctl {
    match Process::new("systemctl").arg("--user").args(args).output() {
        Ok(output) => Systemctl {
            success: output.status.success(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(error) => Systemctl {
            success: false,
            code: None,
            stdout: String::new(),
            stderr: format!("failed to run systemctl: {error}"),
        },
    }
}

/// start, restart or stop the daemon unit, then report the daemon state.
fn service(state: &AppState, operation: &str) -> Response {
    let result = systemctl_user(&[operation, DAEMON_UNIT]);
    let daemon = daemon_call(state, Command::Status).ok();
    service_response(operation, &result, json!({ "daemon": daemon }))
}

#[derive(Deserialize)]
struct AutostartRequest {
    enabled: bool,
}

/// Enables or disables the unit for future logins (does not start it).
fn autostart(_state: &AppState, body: &[u8]) -> Response {
    let request: AutostartRequest = match parse_json(body, "autostart") {
        Ok(request) => request,
        Err(response) => return response,
    };
    let operation = if request.enabled { "enable" } else { "disable" };
    let result = systemctl_user(&[operation, DAEMON_UNIT]);
    service_response(operation, &result, json!({ "enabled": request.enabled }))
}

/// Common body for systemctl operations: 200 on success, 500 with the
/// command's output otherwise. `extra` fields are merged in.
fn service_response(operation: &str, result: &Systemctl, extra: serde_json::Value) -> Response {
    let message = if result.success {
        format!("systemctl --user {operation} {DAEMON_UNIT} succeeded")
    } else if result.stderr.is_empty() {
        format!("systemctl --user {operation} {DAEMON_UNIT} failed")
    } else {
        format!(
            "systemctl --user {operation} {DAEMON_UNIT} failed: {}",
            result.stderr
        )
    };
    let mut body = json!({
        "ok": result.success,
        "operation": operation,
        "message": message,
        "status": result.code,
        "stdout": result.stdout,
        "stderr": result.stderr,
    });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    Response::json(if result.success { 200 } else { 500 }, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handle;
    use crate::state::testing::{get, json, post, state};

    #[test]
    fn invalid_input_is_rejected_before_any_ipc() {
        let state = state();
        let cases: [(&str, &[u8]); 9] = [
            ("/api/daemon/brightness", br#"{"percent":101}"#),
            ("/api/daemon/brightness", br#"{"percent":-1}"#),
            ("/api/daemon/tap", br#"{"x":640}"#),
            ("/api/daemon/touch", br#"{"phase":"began","x":700}"#),
            ("/api/daemon/touch", br#"{"phase":"wiggle","x":7}"#),
            ("/api/daemon/page", br#"{}"#),
            ("/api/daemon/page", br#"{"page_id":"a","direction":"next"}"#),
            ("/api/daemon/profile", br#"{"profile_id":" "}"#),
            ("/api/daemon/notify", br#"{"text":""}"#),
        ];
        for (path, body) in cases {
            let response = handle(&post(path, body), &state);
            assert_eq!(
                response.status,
                400,
                "{path} {}",
                String::from_utf8_lossy(body)
            );
            assert_eq!(json(&response)["ok"], false);
        }
    }

    #[test]
    fn valid_commands_report_an_unreachable_daemon_as_503() {
        let state = state();
        for (path, body) in [
            ("/api/daemon/tap", &br#"{"x":12}"#[..]),
            ("/api/daemon/page", br#"{"direction":"next"}"#),
            ("/api/daemon/blank", b""),
            ("/api/daemon/notify", br#"{"text":"hello","icon":"bell"}"#),
        ] {
            let response = handle(&post(path, body), &state);
            assert_eq!(response.status, 503, "{path}");
            assert!(json(&response)["message"]
                .as_str()
                .unwrap()
                .contains("not reachable"));
        }
        assert_eq!(
            handle(&post("/api/daemon/explode", b""), &state).status,
            404
        );
    }

    #[test]
    fn status_reports_unreachable_daemon_without_failing() {
        let response = handle(&get("/api/daemon/status"), &state());
        let body = json(&response);
        assert_eq!(response.status, 200);
        assert_eq!(body["reachable"], false);
        assert!(body["status"].is_null());
    }

    #[test]
    fn page_targets_map_to_ipc_commands() {
        let request = |page_id: Option<&str>, direction: Option<&str>| PageRequest {
            page_id: page_id.map(Into::into),
            direction: direction.map(Into::into),
        };
        assert_eq!(
            page_command(request(None, Some("previous"))),
            Ok(Command::PagePrevious)
        );
        assert_eq!(
            page_command(request(Some("media"), None)),
            Ok(Command::GoToPage {
                page_id: "media".into()
            })
        );
        assert!(page_command(request(None, Some("sideways"))).is_err());
    }

    #[test]
    fn failed_systemctl_becomes_500_with_output() {
        let result = Systemctl {
            success: false,
            code: Some(5),
            stdout: String::new(),
            stderr: "Unit linexusd.service not found.".into(),
        };
        let response = service_response("start", &result, json!({ "daemon": null }));
        let body = json(&response);
        assert_eq!(response.status, 500);
        assert_eq!(body["ok"], false);
        assert!(body["message"].as_str().unwrap().contains("not found"));
        assert!(body["daemon"].is_null());
        assert_eq!(body["status"], 5);
    }
}
