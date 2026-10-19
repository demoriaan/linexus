//! `linexusctl doctor`: a short health report with a fix for each problem.

use crate::fix::{INPUT_HELPER_BINARY, UDEV_INSTALL_COMMAND, UDEV_RULE_PATHS};
use crate::util::{
    command_stdout, home_dir, is_executable, os_pretty_name, processes_named, read_trimmed,
    systemctl_query, tilde, which,
};
use nexus_core::{enumerate_hidapi, enumerate_hidraw_sysfs, is_probably_permission_error};
use nexus_ipc::{DaemonMode, DeviceState};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Desktop tools the actions and the daemon call, and what breaks without
/// them.
const TOOLS: [(&str, &str); 6] = [
    ("xdg-open", "opening links and files"),
    ("wpctl", "volume and microphone controls"),
    ("busctl", "media controls"),
    ("curl", "HTTP request actions"),
    ("dbus-monitor", "desktop notifications on the strip"),
    ("loginctl", "the lock-screen action"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Default)]
struct Report {
    problems: usize,
    warnings: usize,
    fixable: bool,
}

impl Report {
    fn check(&mut self, level: Level, text: impl AsRef<str>) {
        let tag = match level {
            Level::Ok => "ok  ",
            Level::Warn => {
                self.warnings += 1;
                "warn"
            }
            Level::Fail => {
                self.problems += 1;
                "FAIL"
            }
        };
        println!("  {tag}  {}", text.as_ref());
    }

    fn hint(&self, text: impl AsRef<str>) {
        println!("        {}", text.as_ref());
    }
}

/// Whether this user can open the strip, as far as doctor could tell.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Missing,
    Denied,
    Granted,
}

pub(crate) fn run(verbose: bool) {
    let mut report = Report::default();
    println!("iCUE Nexus doctor");
    println!("  {}", system_line());

    println!("\nDevice");
    let access = check_device(&mut report);
    check_udev(&mut report, access);
    check_competing_software(&mut report);

    println!("\nServices");
    check_daemon(&mut report);
    check_input_helper(&mut report);

    println!("\nConfig");
    check_config(&mut report);

    println!("\nDesktop tools");
    check_tools(&mut report);

    if verbose {
        print_details();
    }

    println!();
    if report.problems == 0 && report.warnings == 0 {
        println!("Everything looks good.");
        return;
    }
    let mut summary = format!(
        "{} problem(s), {} warning(s).",
        report.problems, report.warnings
    );
    if report.fixable {
        summary.push_str(" `linexusctl doctor --fix` repairs what it can.");
    }
    if !verbose {
        summary.push_str(" `--verbose` shows details.");
    }
    println!("{summary}");
}

fn system_line() -> String {
    let os = os_pretty_name().unwrap_or_else(|| "Linux".into());
    let kernel = read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_else(|| "?".into());
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown session".into());
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown desktop".into());
    format!("{os}, kernel {kernel}, {session}, {desktop}")
}

fn check_device(report: &mut Report) -> Access {
    let nodes: Vec<_> = enumerate_hidraw_sysfs()
        .into_iter()
        .filter(|node| node.is_nexus_vid_pid())
        .collect();
    if nodes.is_empty() {
        report.check(Level::Fail, "Nexus not found (USB 1b1c:1b8e)");
        report.hint("Check the USB cable; the strip appears as a USB HID device.");
        return Access::Missing;
    }
    let interfaces = match enumerate_hidapi() {
        Ok(devices) => devices,
        Err(error) => {
            report.check(Level::Fail, format!("cannot list HID devices: {error}"));
            return Access::Denied;
        }
    };
    let Some(display) = interfaces
        .iter()
        .filter(|d| d.is_operational_candidate())
        .min_by(|a, b| a.path.cmp(&b.path))
    else {
        let found: Vec<String> = nodes
            .iter()
            .map(|n| n.devnode.display().to_string())
            .collect();
        report.check(
            Level::Fail,
            format!(
                "Nexus found ({}) but not its display interface",
                found.join(", ")
            ),
        );
        report.hint("Unplug and replug the strip.");
        return Access::Denied;
    };
    match crate::device::try_open(display) {
        Ok(()) => {
            report.check(Level::Ok, format!("Nexus connected at {}", display.path));
            Access::Granted
        }
        Err(error) if is_probably_permission_error(&error) => {
            report.check(
                Level::Fail,
                format!("Nexus at {} cannot be opened by this user", display.path),
            );
            Access::Denied
        }
        Err(error) => {
            report.check(
                Level::Fail,
                format!("Nexus at {} failed to open: {error}", display.path),
            );
            Access::Denied
        }
    }
}

fn check_udev(report: &mut Report, access: Access) {
    let installed = UDEV_RULE_PATHS
        .iter()
        .find_map(|path| fs::read_to_string(path).ok().map(|text| (*path, text)));
    match installed {
        Some((path, text)) if nexus_core::udev_rule_matches(&text) => {
            report.check(Level::Ok, format!("udev rule installed ({path})"));
            if access == Access::Denied {
                report.hint("Access is still denied: replug the strip. If that does not help,");
                report.hint(
                    "join the plugdev group (sudo usermod -aG plugdev $USER) and log in again.",
                );
            }
        }
        Some((path, _)) => {
            report.fixable = true;
            report.check(
                Level::Warn,
                format!("udev rule {path} differs from this version's rule"),
            );
            report.hint(format!("Update it: {UDEV_INSTALL_COMMAND}"));
        }
        None => {
            report.fixable = true;
            if access == Access::Granted {
                report.check(
                    Level::Warn,
                    "udev rule not installed; access works now but may stop after a replug",
                );
            } else {
                report.check(Level::Fail, "udev rule not installed");
            }
            report.hint(format!("Install it: {UDEV_INSTALL_COMMAND}"));
            report.hint("Then unplug and replug the strip.");
        }
    }
}

fn check_competing_software(report: &mut Report) {
    let pids = processes_named("OpenLinkHub");
    if pids.is_empty() {
        report.check(
            Level::Ok,
            "no other software driving the Nexus (OpenLinkHub)",
        );
        return;
    }
    let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
    report.check(
        Level::Warn,
        format!(
            "OpenLinkHub is running (pid {}) and may fight over the strip",
            pids.join(", ")
        ),
    );
    report.hint("Stop it while using this app, e.g. `sudo systemctl stop OpenLinkHub`,");
    report.hint("or turn off its Nexus support.");
}

fn check_daemon(report: &mut Report) {
    let socket = nexus_ipc::default_socket_path();
    let response = match nexus_ipc::send_request(&socket, nexus_ipc::Command::Status) {
        Ok(response) => response,
        Err(_) => {
            report.check(Level::Fail, "daemon (linexusd) is not running");
            if user_unit_installed() {
                report.hint("Start it: systemctl --user enable --now linexusd.service");
                report.hint("Logs: journalctl --user -u linexusd");
            } else {
                report.fixable = true;
                report.hint("`linexusctl doctor --fix` installs and enables its user service.");
            }
            return;
        }
    };
    let Some(status) = response.status.filter(|_| response.ok) else {
        report.check(
            Level::Warn,
            format!("daemon answers but reports: {}", response.message),
        );
        return;
    };
    let place = format!(
        "profile '{}', page '{}'",
        status.profile_name, status.page_name
    );
    let screen = if status.blanked { ", screen off" } else { "" };
    match (status.mode, status.device.state) {
        (DaemonMode::Emulator, _) | (_, DeviceState::Emulator) => report.check(
            Level::Warn,
            format!("daemon runs in emulator mode, not on the strip ({place})"),
        ),
        (_, DeviceState::Unavailable) => {
            report.check(
                Level::Warn,
                format!(
                    "daemon runs but cannot use the strip: {}",
                    status
                        .device
                        .error
                        .as_deref()
                        .unwrap_or("device unavailable")
                ),
            );
        }
        (DaemonMode::Starting, _) => report.check(Level::Ok, "daemon is starting"),
        _ => report.check(Level::Ok, format!("daemon running ({place}{screen})")),
    }
}

fn user_unit_installed() -> bool {
    let mut candidates = vec![
        PathBuf::from("/usr/lib/systemd/user/linexusd.service"),
        PathBuf::from("/etc/systemd/user/linexusd.service"),
    ];
    if let Some(home) = home_dir() {
        candidates.push(home.join(".config/systemd/user/linexusd.service"));
    }
    candidates.iter().any(|path| path.is_file())
}

fn check_input_helper(report: &mut Report) {
    let status = nexus_actions::BackendStatus::probe();
    let answered =
        nexus_input::send_request(&status.input_helper_socket, nexus_input::InputCommand::Ping)
            .is_ok_and(|response| response.ok);
    if answered {
        report.check(
            Level::Ok,
            "input helper running (shortcut and text actions work)",
        );
        return;
    }
    report.check(
        Level::Warn,
        "input helper not running; keyboard-shortcut and text actions will fail",
    );
    if !status.uinput_present {
        report.hint("/dev/uinput is missing: sudo modprobe uinput");
    }
    if is_executable(Path::new(INPUT_HELPER_BINARY)) {
        report.hint("Start it: sudo systemctl enable --now linexus-inputd.service");
    } else {
        report.fixable = true;
        report.hint("`linexusctl doctor --fix` installs it (needs root).");
    }
}

fn check_config(report: &mut Report) {
    let path = nexus_config::default_config_path();
    let shown = tilde(&path);
    if !path.exists() {
        report.check(
            Level::Ok,
            format!(
                "no {shown} yet; the built-in default is used (`linexusctl config init` writes it)"
            ),
        );
        return;
    }
    match nexus_config::load_with_report(&path) {
        Ok((config, None)) => {
            let active = nexus_config::active_profile(&config)
                .map_or(config.active_profile.as_str(), |p| p.name.as_str());
            report.check(
                Level::Ok,
                format!(
                    "{shown}: {} profile(s), active '{active}'",
                    config.profiles.len()
                ),
            );
        }
        Ok((_, Some(migration))) => {
            report.fixable = true;
            report.check(
                Level::Warn,
                format!("{shown} uses the old schema {}", migration.from_version),
            );
            report.hint("Upgrade it (a backup is kept): linexusctl config upgrade");
        }
        Err(error) => {
            report.check(Level::Fail, format!("{shown} is invalid: {error}"));
            report.hint("Fix the file, or move it away and run `linexusctl config init`.");
        }
    }
}

fn check_tools(report: &mut Report) {
    let (present, missing): (Vec<_>, Vec<_>) =
        TOOLS.iter().partition(|(bin, _)| which(bin).is_some());
    if !present.is_empty() {
        let names: Vec<&str> = present.iter().map(|(bin, _)| *bin).collect();
        report.check(Level::Ok, names.join(", "));
    }
    for (bin, purpose) in missing {
        report.check(Level::Warn, format!("{bin} missing; needed for {purpose}"));
    }
}

fn print_details() {
    println!("\nDetails");
    for node in enumerate_hidraw_sysfs()
        .iter()
        .filter(|n| n.is_nexus_vid_pid())
    {
        println!(
            "  hidraw {} mode={} rw={} name={}",
            node.devnode.display(),
            node.permissions_octal
                .map(|m| format!("{m:04o}"))
                .unwrap_or_else(|| "?".into()),
            if node.can_read_write { "yes" } else { "no" },
            node.hid_name.as_deref().unwrap_or("?")
        );
    }
    for d in enumerate_hidapi()
        .unwrap_or_default()
        .iter()
        .filter(|d| d.is_nexus())
    {
        println!(
            "  hid interface {} usage_page=0x{:04x} usage=0x{:04x} if={}",
            d.path, d.usage_page, d.usage, d.interface_number
        );
    }
    for bin in ["linexusctl", "linexusd", "linexus-inputd", "linexus-studio"] {
        let found = which(bin).map_or_else(|| "not in PATH".into(), |p| p.display().to_string());
        println!("  {bin}: {found}");
    }
    println!(
        "  user service linexusd: enabled={}, active={}",
        systemctl_query(true, &["is-enabled", "linexusd.service"]),
        systemctl_query(true, &["is-active", "linexusd.service"])
    );
    println!(
        "  system service linexus-inputd: active={}",
        systemctl_query(false, &["is-active", "linexus-inputd.service"])
    );
    println!(
        "  input helper socket: {}",
        nexus_actions::BackendStatus::probe()
            .input_helper_socket
            .display()
    );
    println!(
        "  daemon socket: {}",
        nexus_ipc::default_socket_path().display()
    );
    println!(
        "  event log: {}",
        nexus_events::default_event_log_path().display()
    );
    println!("  /dev/uinput: {}", uinput_status());
    println!(
        "  groups: {}",
        command_stdout("id", &["-nG"]).unwrap_or_default().trim()
    );
    println!("  hwmon sensors: {}", hwmon_names());
    let players = nexus_actions::list_mpris_players();
    println!(
        "  MPRIS players: {}",
        if players.is_empty() {
            "none".into()
        } else {
            players.join(", ")
        }
    );
}

fn uinput_status() -> String {
    match fs::metadata("/dev/uinput") {
        Ok(meta) => format!("present, mode {:04o}", meta.permissions().mode() & 0o7777),
        Err(_) => "missing".into(),
    }
}

fn hwmon_names() -> String {
    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else {
        return "none".into();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join("name")).ok())
        .map(|name| name.trim().to_string())
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}
