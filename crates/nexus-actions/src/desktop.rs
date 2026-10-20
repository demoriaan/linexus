//! Desktop-session plumbing for launching GUI programs from a background
//! daemon: the graphical environment it may have missed at startup, and the
//! user's default browser so URLs open in a new, focused window.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Graphical-session variables a user service started before the desktop
/// (`WantedBy=default.target`) does not inherit. Plasma/GNOME import them into
/// the systemd user manager once the session is up, so they are re-read from
/// there for every launch instead of trusting the daemon's own environment.
/// Without them `xdg-open` falls back to `$BROWSER`, which fails with
/// "no DISPLAY environment variable specified".
const SESSION_ENV_KEYS: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
    "KDE_FULL_SESSION",
    "KDE_SESSION_VERSION",
    // The focus watcher finds Hyprland and Sway through these.
    "HYPRLAND_INSTANCE_SIGNATURE",
    "SWAYSOCK",
];

/// Browsers whose launcher accepts `--new-window <url>` (Chromium and Firefox
/// families), matched against the executable's file name.
const NEW_WINDOW_BROWSERS: &[&str] = &[
    "brave",
    "brave-browser",
    "brave-browser-stable",
    "brave-browser-beta",
    "brave-browser-nightly",
    "chromium",
    "chromium-browser",
    "ungoogled-chromium",
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "google-chrome-unstable",
    "vivaldi",
    "vivaldi-stable",
    "microsoft-edge",
    "microsoft-edge-stable",
    "microsoft-edge-beta",
    "opera",
    "thorium-browser",
    "cromite",
    "firefox",
    "firefox-esr",
    "firefox-developer-edition",
    "firefox-nightly",
    "librewolf",
    "firedragon",
    "waterfox",
    "floorp",
    "zen",
    "zen-browser",
    "mullvad-browser",
];

const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// The default `https` handler, resolved to an argv that opens a new window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserLauncher {
    pub desktop_id: String,
    /// Program plus fixed arguments from the desktop entry, field codes
    /// removed; callers append `--new-window <url>`.
    pub argv: Vec<String>,
}

/// Graphical-session variables from `systemctl --user show-environment`.
pub fn session_environment() -> Vec<(String, String)> {
    let Some(systemctl) = crate::which("systemctl") else {
        return Vec::new();
    };
    let mut command = Command::new(systemctl);
    command.args(["--user", "show-environment"]);
    capture_stdout(command, QUERY_TIMEOUT)
        .map(|text| parse_show_environment(&text))
        .unwrap_or_default()
}

pub fn parse_show_environment(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            // systemd prints values with special characters as $'…' C-style
            // strings; none of the whitelisted variables legitimately needs
            // that, so skip rather than mis-decode them.
            if !SESSION_ENV_KEYS.contains(&key) || value.is_empty() || value.starts_with("$'") {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Desktop id of the default application for `mime` (`xdg-mime query
/// default`), e.g. `org.kde.dolphin.desktop` for `inode/directory`.
pub fn default_application(mime: &str, session_env: &[(String, String)]) -> Option<String> {
    let xdg_mime = crate::which("xdg-mime")?;
    let mut command = Command::new(xdg_mime);
    command
        .args(["query", "default", mime])
        .envs(session_env.iter().map(|(key, value)| (key, value)));
    let desktop_id = capture_stdout(command, QUERY_TIMEOUT)?.trim().to_string();
    (desktop_id.ends_with(".desktop") && !desktop_id.contains('/')).then_some(desktop_id)
}

/// Terminal emulators by preference: the cross-desktop `xdg-terminal-exec`
/// launcher first, then each desktop's own terminal, then common ones.
const TERMINALS: &[(&str, &str)] = &[
    ("", "xdg-terminal-exec"),
    ("KDE", "konsole"),
    ("GNOME", "ptyxis"),
    ("GNOME", "kgx"),
    ("GNOME", "gnome-terminal"),
    ("XFCE", "xfce4-terminal"),
    ("", "kitty"),
    ("", "alacritty"),
    ("", "wezterm"),
    ("", "foot"),
    ("", "konsole"),
    ("", "gnome-terminal"),
    ("", "xterm"),
];

/// Argv that opens a new terminal window on this desktop.
pub fn default_terminal(session_env: &[(String, String)]) -> Option<Vec<String>> {
    let desktop = session_env
        .iter()
        .find(|(key, _)| key == "XDG_CURRENT_DESKTOP")
        .map(|(_, value)| value.to_ascii_uppercase())
        .or_else(|| {
            env::var("XDG_CURRENT_DESKTOP")
                .ok()
                .map(|v| v.to_ascii_uppercase())
        })
        .unwrap_or_default();
    TERMINALS
        .iter()
        .filter(|(only_on, _)| only_on.is_empty() || desktop.split(':').any(|d| d == *only_on))
        .find(|(_, program)| crate::which(program).is_some())
        .map(|(_, program)| vec![program.to_string()])
}

/// Looks up the default `x-scheme-handler/https` application and returns its
/// launcher when it is a browser known to support `--new-window`.
pub fn default_browser(session_env: &[(String, String)]) -> Option<BrowserLauncher> {
    let desktop_id = default_application("x-scheme-handler/https", session_env)?;
    let path = application_dirs()
        .into_iter()
        .map(|dir| dir.join("applications").join(&desktop_id))
        .find(|path| path.is_file())?;
    let exec = desktop_entry_exec(&fs::read_to_string(path).ok()?)?;
    let argv = new_window_browser_argv(&exec)?;
    Some(BrowserLauncher { desktop_id, argv })
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let data_dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// `Exec=` value of the `[Desktop Entry]` group (not of desktop actions).
pub fn desktop_entry_exec(contents: &str) -> Option<String> {
    let mut in_entry = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some(value) = line.strip_prefix("Exec=") {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Splits an `Exec=` command line, drops field codes (`%U`, flatpak's `@@u`
/// markers, …) and returns the argv when the program — or a flatpak/snap
/// wrapper's `--command=` target — is a browser in [`NEW_WINDOW_BROWSERS`].
pub fn new_window_browser_argv(exec: &str) -> Option<Vec<String>> {
    let argv: Vec<String> = split_exec(exec)
        .into_iter()
        .filter(|arg| !is_field_code(arg))
        .collect();
    let is_browser = argv.iter().any(|arg| {
        let name = arg.strip_prefix("--command=").unwrap_or(arg);
        Path::new(name)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| NEW_WINDOW_BROWSERS.contains(&name))
    });
    (is_browser && !argv.is_empty()).then_some(argv)
}

fn is_field_code(arg: &str) -> bool {
    matches!(arg, "@@" | "@@u" | "@@f") || (arg.len() == 2 && arg.starts_with('%') && arg != "%%")
}

/// Desktop-entry quoting: whitespace separates arguments, double quotes group
/// them and a backslash escapes the next character inside quotes.
fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_arg = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                quoted = !quoted;
                in_arg = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ch if ch.is_whitespace() && !quoted => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            ch => {
                current.push(ch);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(current);
    }
    args
}

/// Runs `command` and returns its stdout when it exits successfully within
/// `timeout`; a command that overruns is killed.
fn capture_stdout(mut command: Command, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Drain stdout concurrently so a chatty command cannot block on a full
    // pipe while it is being waited on.
    let reader = thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).ok().map(|_| text)
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reader.join().ok().flatten(),
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_graphical_session_variables() {
        let env = parse_show_environment(
            "HOME=/home/u\nWAYLAND_DISPLAY=wayland-0\nDISPLAY=:0\nXDG_CURRENT_DESKTOP=KDE\nXAUTHORITY=$'/run/user/1000/x a'\nBROWSER=firedragon\n",
        );
        assert_eq!(
            env,
            vec![
                ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                ("DISPLAY".to_string(), ":0".to_string()),
                ("XDG_CURRENT_DESKTOP".to_string(), "KDE".to_string()),
            ]
        );
    }

    #[test]
    fn reads_exec_from_main_desktop_entry_only() {
        let entry = "[Desktop Entry]\nName=Brave\nExec=brave %U\n\n[Desktop Action new-window]\nExec=brave\n";
        assert_eq!(desktop_entry_exec(entry).as_deref(), Some("brave %U"));
        let action_first = "[Desktop Action x]\nExec=other\n[Desktop Entry]\nExec=firefox %u\n";
        assert_eq!(
            desktop_entry_exec(action_first).as_deref(),
            Some("firefox %u")
        );
    }

    #[test]
    fn recognizes_native_flatpak_and_snap_browsers() {
        assert_eq!(
            new_window_browser_argv("brave %U"),
            Some(vec!["brave".to_string()])
        );
        assert_eq!(
            new_window_browser_argv("/usr/lib/firefox/firefox %u"),
            Some(vec!["/usr/lib/firefox/firefox".to_string()])
        );
        assert_eq!(
            new_window_browser_argv(
                "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=brave --file-forwarding com.brave.Browser @@u %U @@"
            ),
            Some(
                [
                    "/usr/bin/flatpak",
                    "run",
                    "--branch=stable",
                    "--arch=x86_64",
                    "--command=brave",
                    "--file-forwarding",
                    "com.brave.Browser",
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert_eq!(
            new_window_browser_argv("env BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop /snap/bin/firefox %u"),
            Some(
                [
                    "env",
                    "BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop",
                    "/snap/bin/firefox",
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert_eq!(new_window_browser_argv("kfmclient openURL %u"), None);
    }

    #[test]
    fn capture_stdout_returns_output_and_kills_overruns() {
        let mut echo = Command::new("/bin/sh");
        echo.args(["-c", "echo brave-browser.desktop"]);
        assert_eq!(
            capture_stdout(echo, Duration::from_secs(2)).as_deref(),
            Some("brave-browser.desktop\n")
        );
        let mut hang = Command::new("/bin/sh");
        hang.args(["-c", "sleep 5"]);
        let started = Instant::now();
        assert_eq!(capture_stdout(hang, Duration::from_millis(100)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn splits_quoted_exec_arguments() {
        assert_eq!(
            split_exec(r#""/opt/My Browser/browser" --flag "a \"b\"" %U"#),
            vec!["/opt/My Browser/browser", "--flag", "a \"b\"", "%U"]
        );
    }
}
