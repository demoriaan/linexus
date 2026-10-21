//! System view: the setup health checks, the event log, version and the
//! legacy session endpoint.

use super::daemon::{systemctl_user, Systemctl, DAEMON_UNIT};
use super::daemon_call;
use crate::http::{Request, Response};
use crate::state::AppState;
use nexus_ipc::{Command, DaemonMode, DaemonStatus};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const UDEV_RULE_PATHS: [&str; 2] = [
    "/etc/udev/rules.d/99-linexus.rules",
    "/usr/lib/udev/rules.d/99-linexus.rules",
];
const UDEV_FIX: &str = "linexusctl print-udev-rule | sudo tee /etc/udev/rules.d/99-linexus.rules \
>/dev/null && sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=hidraw";
/// `(program, what it is used for, package that usually ships it)`; each
/// entry lists alternatives, the first one is suggested when all are missing.
type Backend = (&'static str, &'static str, &'static str);
const BACKENDS: [&[Backend]; 4] = [
    &[(
        "xdg-open",
        "opening links and launching applications",
        "xdg-utils",
    )],
    &[
        ("wpctl", "volume control through PipeWire", "wireplumber"),
        (
            "pactl",
            "volume control through PulseAudio",
            "pulseaudio-utils",
        ),
    ],
    &[(
        "busctl",
        "media controls over MPRIS and desktop notifications",
        "systemd",
    )],
    &[("curl", "feeds, weather and Home Assistant", "curl")],
];

pub fn route(request: &Request, state: &AppState) -> Option<Response> {
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/health") => health(state),
        ("GET", "/api/events") => events(request),
        ("GET", "/api/version") => version(),
        ("GET", "/api/session") => {
            Response::json(200, &json!({ "ok": true, "token": state.token }))
        }
        _ => return None,
    };
    Some(response)
}

fn version() -> Response {
    let mut body = json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") });
    // Packaging may stamp the build; a guessed commit would be worse than none.
    if let Some(commit) = option_env!("NEXUS_BUILD_COMMIT") {
        body["commit"] = commit.into();
    }
    Response::json(200, &body)
}

/// Recent event log records, newest last (`limit`, default 100, at most
/// 1000; `lines` is accepted as an older spelling).
fn events(request: &Request) -> Response {
    let limit = request
        .query_param("limit")
        .or_else(|| request.query_param("lines"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 1000);
    let path = nexus_events::default_event_log_path();
    match nexus_events::read_recent(&path, limit) {
        Ok(records) => {
            let events: Vec<serde_json::Value> = records
                .iter()
                .map(|record| {
                    json!({
                        "ts": record.ts_ms as f64 / 1000.0,
                        "level": record.level,
                        "source": record.source,
                        "kind": record.kind,
                        "message": record.message,
                        "fields": record.fields,
                    })
                })
                .collect();
            Response::json(200, &json!({ "ok": true, "events": events }))
        }
        Err(error) => Response::error(500, error.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Serialize)]
struct Check {
    id: String,
    state: Level,
    title: String,
    detail: String,
    /// A copy-pasteable shell command, when one fixes the problem.
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
    /// Message key and its values, so the UI can show `title` and `detail`
    /// in its own language.
    key: &'static str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    args: BTreeMap<&'static str, String>,
}

fn check(
    id: impl Into<String>,
    state: Level,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> Check {
    Check {
        id: id.into(),
        state,
        title: title.into(),
        detail: detail.into(),
        fix: None,
        key: "",
        args: BTreeMap::new(),
    }
}

impl Check {
    fn fix(mut self, command: impl Into<String>) -> Self {
        self.fix = Some(command.into());
        self
    }

    fn key(mut self, key: &'static str) -> Self {
        self.key = key;
        self
    }

    fn arg(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.args.insert(name, value.into());
        self
    }
}

/// Facts the UI needs for its daemon buttons, beside the checks.
#[derive(Serialize)]
struct DaemonFacts {
    reachable: bool,
    autostart: Option<bool>,
    unit_active: bool,
}

/// Everything setup can get wrong, as one ordered list: device access,
/// udev rule, daemon, autostart, input helper, backends, conflicts.
fn health(state: &AppState) -> Response {
    let status = daemon_call(state, Command::Status);
    let unit_active = systemctl_user(&["is-active", DAEMON_UNIT]).success;
    let autostart = autostart_state(&systemctl_user(&["is-enabled", DAEMON_UNIT]));
    let mut checks = Vec::from(device_checks());
    checks.push(daemon_check(state, &status, unit_active));
    checks.push(autostart_check(autostart));
    checks.push(input_helper_check());
    checks.extend(BACKENDS.iter().map(|alternatives| {
        let &(program, purpose, package) = alternatives
            .iter()
            .find(|(program, _, _)| find_in_path(program).is_some())
            .unwrap_or(&alternatives[0]);
        backend_check(program, purpose, package)
    }));
    if nexus_sensors::nvidia_gpu_present() {
        checks.push(backend_check(
            "nvidia-smi",
            "NVIDIA GPU readings",
            "nvidia-utils",
        ));
    }
    checks.extend(gnome_focus_check());
    checks.push(conflict_check());
    let daemon = DaemonFacts {
        reachable: status.is_ok(),
        autostart,
        unit_active,
    };
    Response::json(
        200,
        &json!({ "ok": true, "checks": checks, "daemon": daemon }),
    )
}

fn device_checks() -> [Check; 2] {
    let candidates: Vec<_> = nexus_core::enumerate_hidraw_sysfs()
        .into_iter()
        .filter(|candidate| candidate.is_nexus_vid_pid())
        .collect();
    let operational = nexus_core::enumerate_hidapi()
        .map(|devices| {
            devices
                .iter()
                .any(|device| device.is_operational_candidate())
        })
        .unwrap_or(false);
    let found = !candidates.is_empty() || operational;
    let accessible = candidates.iter().find(|candidate| candidate.can_read_write);
    let rule_installed = UDEV_RULE_PATHS
        .iter()
        .any(|path| matching_udev_rule(Path::new(path)));
    let rule_present = UDEV_RULE_PATHS.iter().any(|path| Path::new(path).is_file());

    let device = match (accessible, found) {
        (Some(node), _) => check("device", Level::Ok, "The Nexus is connected and accessible.", format!("Using {}.", node.devnode.display())).key("device.ok").arg("path", node.devnode.display().to_string()),
        (None, true) if rule_installed => check("device", Level::Error, "The Nexus is connected but not accessible.", "The udev rule is installed but not applied to the device yet. Unplug and replug the Nexus.")
            .fix("sudo udevadm trigger --subsystem-match=hidraw").key("device.replug"),
        (None, true) => check("device", Level::Error, "The Nexus is connected but not accessible.", "Your user cannot open its hidraw device. Install the udev rule, then replug the Nexus.")
            .fix(UDEV_FIX).key("device.denied"),
        (None, false) => check("device", Level::Warn, "The Nexus is not detected.", "Connect the strip over USB. Studio and the emulator work without it.").key("device.missing"),
    };
    let udev = if rule_installed {
        check(
            "udev",
            Level::Ok,
            "The udev rule is installed.",
            "Members of the seat get access to the Nexus.",
        )
        .key("udev.ok")
    } else {
        let level = if found && accessible.is_none() {
            Level::Error
        } else {
            Level::Warn
        };
        let (key, title, detail) = if rule_present {
            (
                "udev.outdated",
                "The udev rule is outdated.",
                "The installed rule differs from the current one.",
            )
        } else {
            (
                "udev.missing",
                "The udev rule is missing.",
                "Without it only root can open the Nexus.",
            )
        };
        check("udev", level, title, detail).fix(UDEV_FIX).key(key)
    };
    [device, udev]
}

fn matching_udev_rule(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|content| nexus_core::udev_rule_matches(&content))
        .unwrap_or(false)
}

fn daemon_check(
    state: &AppState,
    status: &Result<nexus_ipc::Response, String>,
    unit_active: bool,
) -> Check {
    let start = format!("systemctl --user start {DAEMON_UNIT}");
    let restart = format!("systemctl --user restart {DAEMON_UNIT}");
    match status.as_ref().map(|reply| reply.status.as_ref()) {
        Ok(Some(status)) => running_check(status),
        Ok(None) => check(
            "daemon",
            Level::Warn,
            "The daemon answers without a status.",
            "It may be an older version; restart it.",
        )
        .fix(restart)
        .key("daemon.no_status"),
        Err(error) if unit_active => check(
            "daemon",
            Level::Error,
            "The daemon unit is active but does not answer.",
            format!("No answer on {}: {error}", state.socket_path.display()),
        )
        .fix(restart)
        .key("daemon.silent")
        .arg("socket", state.socket_path.display().to_string())
        .arg("error", error.clone()),
        Err(_) => check(
            "daemon",
            Level::Error,
            "The daemon is not running.",
            "Nothing will appear on the strip until it runs.",
        )
        .fix(start)
        .key("daemon.stopped"),
    }
}

fn running_check(status: &DaemonStatus) -> Check {
    let page = format!(
        "Profile {}, page {}.",
        status.profile_name, status.page_name
    );
    match status.mode {
        DaemonMode::Hardware => check(
            "daemon",
            Level::Ok,
            "The daemon is running and drives the Nexus.",
            page,
        )
        .key("daemon.hardware")
        .arg("profile", status.profile_name.clone())
        .arg("page", status.page_name.clone()),
        DaemonMode::Emulator => {
            let reason = status
                .device
                .error
                .as_deref()
                .unwrap_or("the device is not available");
            check(
                "daemon",
                Level::Warn,
                "The daemon is running without the Nexus.",
                format!("It renders to its emulator because {reason}."),
            )
            .key("daemon.emulator")
            .arg("reason", reason)
        }
        DaemonMode::Starting => check("daemon", Level::Warn, "The daemon is starting.", page)
            .key("daemon.starting")
            .arg("profile", status.profile_name.clone())
            .arg("page", status.page_name.clone()),
    }
}

/// `systemctl is-enabled`: exit 0 means it starts at login; "disabled" and
/// friends mean it does not; a missing unit or systemctl means unknown.
fn autostart_state(result: &Systemctl) -> Option<bool> {
    if result.success {
        return Some(true);
    }
    match result.stdout.as_str() {
        "disabled" | "masked" | "linked" | "linked-runtime" | "masked-runtime" => Some(false),
        _ => None,
    }
}

fn autostart_check(autostart: Option<bool>) -> Check {
    match autostart {
        Some(true) => check(
            "autostart",
            Level::Ok,
            "The daemon starts at login.",
            "Enabled as a systemd user unit.",
        )
        .key("autostart.on"),
        Some(false) => check(
            "autostart",
            Level::Warn,
            "The daemon does not start at login.",
            "Enable the user unit to start it automatically.",
        )
        .fix(format!("systemctl --user enable {DAEMON_UNIT}"))
        .key("autostart.off"),
        None => check(
            "autostart",
            Level::Warn,
            "The daemon's autostart state is unknown.",
            format!("The user unit {DAEMON_UNIT} is not installed or systemd is not available."),
        )
        .key("autostart.unknown")
        .arg("unit", DAEMON_UNIT),
    }
}

fn input_helper_check() -> Check {
    let backends = nexus_actions::BackendStatus::probe();
    if backends.input_helper_available {
        let detail = format!(
            "Keyboard shortcuts and typed text go through {}.",
            backends.input_helper_socket.display()
        );
        return check(
            "input_helper",
            Level::Ok,
            "The input helper is running.",
            detail,
        )
        .key("input.ok")
        .arg("socket", backends.input_helper_socket.display().to_string());
    }
    let (key, detail) = if backends.uinput_present {
        (
            "input.off",
            "Keyboard shortcuts and typed text do not work on Wayland without it.",
        )
    } else {
        (
            "input.no_uinput",
            "/dev/uinput is missing, so keyboard shortcuts and typed text cannot work.",
        )
    };
    check(
        "input_helper",
        Level::Warn,
        "The input helper is not running.",
        detail,
    )
    .fix("systemctl --user enable --now linexus-inputd.service")
    .key(key)
}

fn backend_check(program: &str, purpose: &str, package: &str) -> Check {
    let id = format!("backend.{program}");
    let found = match find_in_path(program) {
        Some(path) => check(
            id,
            Level::Ok,
            format!("{program} is available."),
            format!("Used for {purpose} ({}).", path.display()),
        )
        .key("backend.ok")
        .arg("path", path.display().to_string()),
        None => check(
            id,
            Level::Warn,
            format!("{program} is missing."),
            format!("Needed for {purpose}. Install the {package} package."),
        )
        .key("backend.missing")
        .arg("package", package),
    };
    found.arg("program", program)
}

/// First executable named `program` on `PATH`.
fn find_in_path(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// On GNOME, app-aware profiles need the Linexus focus extension.
fn gnome_focus_check() -> Option<Check> {
    let env = nexus_actions::session_environment();
    let desktop = env
        .iter()
        .find(|(key, _)| key == "XDG_CURRENT_DESKTOP")
        .map(|(_, value)| value.to_ascii_uppercase())?;
    if !nexus_sensors::is_gnome(&desktop) || !nexus_sensors::gnome_shell_on_bus(&env) {
        return None;
    }
    let status = nexus_sensors::GnomeExtensionStatus::read();
    let install = "linexusctl gnome-extension install";
    Some(if status.complete() && status.running {
        check(
            "gnome.focus",
            Level::Ok,
            "The GNOME focus extension runs.",
            "Profiles can switch with the focused app.",
        )
        .key("gnome.focus.ok")
    } else if status.user_extensions_off {
        check(
            "gnome.focus",
            Level::Warn,
            "GNOME extensions are turned off.",
            "Profiles can switch with the focused app only through the Linexus focus extension. Turn extensions on in the Extensions app.",
        )
        .fix("gsettings set org.gnome.shell disable-user-extensions false")
        .key("gnome.focus.extensions_off")
    } else if status.complete() {
        check(
            "gnome.focus",
            Level::Warn,
            "The GNOME focus extension does not run yet.",
            "GNOME Shell loads new extensions at login. Log out and back in.",
        )
        .key("gnome.focus.waiting")
    } else if status.dir.is_none() {
        check(
            "gnome.focus",
            Level::Warn,
            "The GNOME focus extension is not installed.",
            "On GNOME, profiles can switch with the focused app only through this small extension. Install it, then log out and back in.",
        )
        .fix(install)
        .key("gnome.focus.missing")
    } else {
        check(
            "gnome.focus",
            Level::Warn,
            "The GNOME focus extension is off or does not fit this GNOME version.",
            "Set it up again, then log out and back in.",
        )
        .fix(install)
        .key("gnome.focus.outdated")
    })
}

fn conflict_check() -> Check {
    if process_running("openlinkhub") {
        check(
            "conflict.openlinkhub",
            Level::Warn,
            "OpenLinkHub is running.",
            "It also drives Corsair devices and can fight over the Nexus. Stop it or exclude the Nexus in its settings.",
        )
        .fix("sudo systemctl stop OpenLinkHub.service")
        .key("conflict.openlinkhub")
    } else {
        check(
            "conflict.openlinkhub",
            Level::Ok,
            "No conflicting software is running.",
            "OpenLinkHub, which can also claim the Nexus, is not running.",
        )
        .key("conflict.none")
    }
}

/// Whether any process name (`/proc/<pid>/comm`) contains `needle`,
/// ignoring case.
fn process_running(needle: &str) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
        })
        .any(|entry| {
            fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim().to_ascii_lowercase().contains(needle))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handle;
    use crate::state::testing::{get, json, state};

    #[test]
    fn health_is_a_flat_ordered_list_with_daemon_facts() {
        let body = json(&handle(&get("/api/health"), &state()));
        assert_eq!(body["ok"], true);
        let checks = body["checks"].as_array().unwrap();
        let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(
            &ids[..5],
            ["device", "udev", "daemon", "autostart", "input_helper"]
        );
        assert_eq!(ids[5], "backend.xdg-open");
        assert_eq!(ids.last(), Some(&"conflict.openlinkhub"));
        for check in checks {
            assert!(["ok", "warn", "error"].contains(&check["state"].as_str().unwrap()));
            assert!(!check["title"].as_str().unwrap().is_empty());
            assert!(check.get("fix").is_none_or(|fix| fix.is_string()));
        }
        assert_eq!(body["daemon"]["reachable"], false, "no daemon in tests");
        assert_eq!(checks[2]["state"], "error");
        assert!(body["daemon"]["unit_active"].is_boolean());
    }

    #[test]
    fn autostart_state_reads_systemctl_answers() {
        let answer = |success, stdout: &str| Systemctl {
            success,
            code: Some(i32::from(!success)),
            stdout: stdout.into(),
            stderr: String::new(),
        };
        assert_eq!(autostart_state(&answer(true, "enabled")), Some(true));
        assert_eq!(autostart_state(&answer(false, "disabled")), Some(false));
        assert_eq!(autostart_state(&answer(false, "")), None);
    }

    #[test]
    fn events_answer_with_a_list() {
        let body = json(&handle(&get("/api/events?limit=5"), &state()));
        assert_eq!(body["ok"], true);
        assert!(body["events"].as_array().unwrap().len() <= 5);
    }

    #[test]
    fn finds_programs_on_path() {
        assert!(find_in_path("sh").is_some());
        assert!(find_in_path("definitely-not-a-program-nexus").is_none());
    }
}
