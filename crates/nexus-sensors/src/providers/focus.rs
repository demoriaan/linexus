//! Which application has keyboard focus, for profiles that switch on by
//! themselves (a game, a video editor, a browser).
//!
//! There is no desktop-neutral way to ask. Each compositor family gets its
//! own watcher:
//! - KDE Plasma: a tiny KWin script prints focus changes; KWin's output
//!   lands in the user journal, which is followed. No polling.
//! - Hyprland and Sway: their IPC tools are polled.
//! - Other X11 sessions: `xprop` on the root window is polled.
//!
//! GNOME on Wayland exposes focus only to shell extensions and is not
//! supported.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MARKER: &str = "nexus-focus\t";
const SCRIPT_NAME: &str = "nexus-focus";
const POLL: Duration = Duration::from_millis(700);

// Window properties are chosen by the application, so they are printed JSON
// encoded: a newline or marker inside an app id or class cannot start a fake
// line. The title is left out; matching never uses it.
const KWIN_SCRIPT: &str = r#"function report(w) {
    if (!w) return;
    print("nexus-focus\t" + JSON.stringify([String(w.desktopFileName || ""), String(w.resourceClass || "")]));
}
var changed = workspace.windowActivated || workspace.clientActivated;
changed.connect(report);
report(workspace.activeWindow || workspace.activeClient);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FocusedWindow {
    /// Desktop entry or Wayland app id, e.g. `org.kde.dolphin` or `steam`.
    pub app_id: String,
    /// X11/KWin window class, e.g. `dolphin`.
    pub class: String,
    pub title: String,
}

impl FocusedWindow {
    /// Whether `pattern` (an app id, class or desktop file name, any case)
    /// names this window.
    pub fn matches(&self, pattern: &str) -> bool {
        let pattern = pattern
            .trim()
            .trim_end_matches(".desktop")
            .to_ascii_lowercase();
        if pattern.is_empty() {
            return false;
        }
        let candidates = [
            self.app_id.to_ascii_lowercase(),
            self.class.to_ascii_lowercase(),
        ];
        candidates.iter().any(|value| {
            !value.is_empty()
                && (*value == pattern
                    // `org.kde.dolphin` should match a plain `dolphin`.
                    || value.rsplit('.').next() == Some(pattern.as_str())
                    || pattern.rsplit('.').next() == Some(value.as_str()))
        })
    }
}

pub struct FocusWatcher {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    backend: &'static str,
    session_env: Vec<(String, String)>,
}

impl FocusWatcher {
    /// Starts the watcher that fits the running desktop, or `None` when
    /// there is none (GNOME Wayland, no session). `session_env` are the
    /// graphical-session variables (DISPLAY, XDG_CURRENT_DESKTOP, ...).
    pub fn start(
        session_env: Vec<(String, String)>,
    ) -> Option<(FocusWatcher, Receiver<FocusedWindow>)> {
        let var = |key: &str| {
            session_env
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var(key).ok())
                .unwrap_or_default()
        };
        let desktop = var("XDG_CURRENT_DESKTOP").to_ascii_uppercase();
        let backend: &'static str =
            if desktop.split(':').any(|d| d == "KDE") && crate::which("journalctl").is_some() {
                "kwin"
            } else if !var("HYPRLAND_INSTANCE_SIGNATURE").is_empty()
                && crate::which("hyprctl").is_some()
            {
                "hyprland"
            } else if !var("SWAYSOCK").is_empty() && crate::which("swaymsg").is_some() {
                "sway"
            } else if var("XDG_SESSION_TYPE") == "x11"
                && !var("DISPLAY").is_empty()
                && crate::which("xprop").is_some()
            {
                "x11"
            } else {
                return None;
            };
        // KWin loads the script from a file; without a private place for it
        // there is no watcher.
        let script = match backend {
            "kwin" => Some(write_kwin_script()?),
            _ => None,
        };
        let watcher = FocusWatcher {
            stop: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            backend,
            session_env,
        };
        let (tx, rx) = mpsc::channel();
        let stop = watcher.stop.clone();
        let child = watcher.child.clone();
        let env = watcher.session_env.clone();
        thread::Builder::new()
            .name("focus-watch".into())
            .spawn(move || match script {
                Some(script) => watch_kwin(&env, &script, &stop, &child, &tx),
                None => poll(backend, &env, &stop, &tx),
            })
            .ok()?;
        Some((watcher, rx))
    }

    pub fn backend(&self) -> &'static str {
        self.backend
    }
}

impl Drop for FocusWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if self.backend == "kwin" {
            let _ = kwin_call(
                &self.session_env,
                "/Scripting",
                "unloadScript",
                &[&format!("string:{SCRIPT_NAME}")],
            );
        }
    }
}

fn command(program: &str, env: &[(String, String)]) -> Command {
    let mut command = Command::new(program);
    command.envs(env.iter().map(|(k, v)| (k, v)));
    command
}

fn kwin_call(env: &[(String, String)], path: &str, method: &str, args: &[&str]) -> Option<String> {
    let output = command("dbus-send", env)
        .args([
            "--session",
            "--print-reply=literal",
            "--dest=org.kde.KWin",
            path,
        ])
        .arg(match path {
            "/Scripting" => format!("org.kde.kwin.Scripting.{method}"),
            _ => format!("org.kde.kwin.Script.{method}"),
        })
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Writes the KWin script to `$XDG_RUNTIME_DIR/linexus/focus.js`. KWin
/// runs whatever that file holds (scripts can call D-Bus), so it is only
/// written into a runtime directory and subdirectory that belong to this
/// user and that nobody else can write to. There is no fallback to the
/// shared /tmp.
fn write_kwin_script() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let dir = runtime.join("linexus");
    if !runtime.is_absolute() || !private_dir(&runtime) {
        return None;
    }
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    if !private_dir(&dir) {
        return None;
    }
    let script = dir.join("focus.js");
    std::fs::write(&script, KWIN_SCRIPT).ok()?;
    Some(script)
}

/// A real directory (not a symlink) owned by this user, writable by nobody
/// else.
fn private_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o022 == 0
    })
}

/// Loads the focus script into KWin and follows KWin's journal output.
fn watch_kwin(
    env: &[(String, String)],
    script: &Path,
    stop: &AtomicBool,
    child: &Mutex<Option<Child>>,
    tx: &Sender<FocusedWindow>,
) {
    // Follow first, so the report the script prints on load is not missed.
    let mut journal = match command("journalctl", env)
        .args([
            "--user",
            "--follow",
            "--lines=0",
            "--output=cat",
            "_COMM=kwin_wayland",
            "_COMM=kwin_x11",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(journal) => journal,
        Err(_) => return,
    };
    let Some(stdout) = journal.stdout.take() else {
        return;
    };
    if let Ok(mut slot) = child.lock() {
        *slot = Some(journal);
    }
    load_kwin_script(env, script);
    let mut last = None;
    for line in BufReader::new(stdout).lines() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(line) = line else { break };
        if let Some(window) = parse_marker_line(&line) {
            if last.as_ref() != Some(&window) {
                last = Some(window.clone());
                if tx.send(window).is_err() {
                    return;
                }
            }
        }
    }
}

fn load_kwin_script(env: &[(String, String)], script: &Path) {
    let _ = kwin_call(
        env,
        "/Scripting",
        "unloadScript",
        &[&format!("string:{SCRIPT_NAME}")],
    );
    let Some(reply) = kwin_call(
        env,
        "/Scripting",
        "loadScript",
        &[
            &format!("string:{}", script.display()),
            &format!("string:{SCRIPT_NAME}"),
        ],
    ) else {
        return;
    };
    let Some(id) = reply
        .split_whitespace()
        .last()
        .and_then(|n| n.parse::<i64>().ok())
    else {
        return;
    };
    // Plasma 6 exposes scripts under /Scripting/ScriptN, Plasma 5 under /N.
    if kwin_call(env, &format!("/Scripting/Script{id}"), "run", &[]).is_none() {
        let _ = kwin_call(env, &format!("/{id}"), "run", &[]);
    }
}

/// `nexus-focus\t["<app id>","<class>"]` as KWin logs the script's output
/// (Plasma 6; older versions put `js: ` in front). Only a line that starts
/// with the marker counts, and the rest must be exactly that JSON array, so
/// other KWin messages that quote window properties cannot fake a focus
/// change. The title is not reported.
pub fn parse_marker_line(line: &str) -> Option<FocusedWindow> {
    let rest = line
        .strip_prefix("js: ")
        .unwrap_or(line)
        .strip_prefix(MARKER)?;
    let [app_id, class]: [String; 2] = serde_json::from_str(rest.trim_end()).ok()?;
    Some(FocusedWindow {
        app_id: app_id.trim().to_string(),
        class: class.trim().to_string(),
        title: String::new(),
    })
}

fn poll(backend: &str, env: &[(String, String)], stop: &AtomicBool, tx: &Sender<FocusedWindow>) {
    let mut last = None;
    while !stop.load(Ordering::SeqCst) {
        let window = match backend {
            "hyprland" => {
                output(env, "hyprctl", &["activewindow", "-j"]).and_then(|j| parse_hyprland(&j))
            }
            "sway" => {
                output(env, "swaymsg", &["-t", "get_tree", "-r"]).and_then(|j| parse_sway(&j))
            }
            _ => x11_focus(env),
        };
        if let Some(window) = window {
            if last.as_ref() != Some(&window) {
                last = Some(window.clone());
                if tx.send(window).is_err() {
                    return;
                }
            }
        }
        thread::sleep(POLL);
    }
}

fn output(env: &[(String, String)], program: &str, args: &[&str]) -> Option<String> {
    let out = command(program, env)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn parse_hyprland(json: &str) -> Option<FocusedWindow> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let class = value.get("class")?.as_str()?.to_string();
    Some(FocusedWindow {
        app_id: class.clone(),
        class,
        title: value
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

pub fn parse_sway(json: &str) -> Option<FocusedWindow> {
    fn find(node: &serde_json::Value) -> Option<&serde_json::Value> {
        if node.get("focused").and_then(|f| f.as_bool()) == Some(true)
            && node.get("type").and_then(|t| t.as_str()) == Some("con")
        {
            return Some(node);
        }
        for key in ["nodes", "floating_nodes"] {
            for child in node
                .get(key)
                .and_then(|n| n.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(found) = find(child) {
                    return Some(found);
                }
            }
        }
        None
    }
    let tree: serde_json::Value = serde_json::from_str(json).ok()?;
    let node = find(&tree)?;
    let app_id = node
        .get("app_id")
        .and_then(|a| a.as_str())
        .unwrap_or_default()
        .to_string();
    let class = node
        .pointer("/window_properties/class")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    Some(FocusedWindow {
        app_id: if app_id.is_empty() {
            class.clone()
        } else {
            app_id
        },
        class,
        title: node
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

fn x11_focus(env: &[(String, String)]) -> Option<FocusedWindow> {
    let root = output(env, "xprop", &["-root", "_NET_ACTIVE_WINDOW"])?;
    let id = root
        .split_whitespace()
        .last()?
        .trim_end_matches(',')
        .to_string();
    if id == "0x0" {
        return None;
    }
    let props = output(env, "xprop", &["-id", &id, "WM_CLASS", "_NET_WM_NAME"])?;
    parse_xprop(&props)
}

pub fn parse_xprop(text: &str) -> Option<FocusedWindow> {
    let mut window = FocusedWindow::default();
    for line in text.lines() {
        let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        if line.starts_with("WM_CLASS") {
            // WM_CLASS(STRING) = "instance", "Class"
            window.app_id = quoted.first()?.to_string();
            window.class = quoted.get(1).unwrap_or(&"").to_string();
        } else if line.starts_with("_NET_WM_NAME") {
            window.title = quoted.first().unwrap_or(&"").to_string();
        }
    }
    (!window.app_id.is_empty() || !window.class.is_empty()).then_some(window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_kwin_journal_lines() {
        let window = parse_marker_line("nexus-focus\t[\"org.kde.dolphin\",\"dolphin\"]").unwrap();
        assert_eq!(window.app_id, "org.kde.dolphin");
        assert_eq!(window.class, "dolphin");
        assert_eq!(window.title, "");
        let older = parse_marker_line("js: nexus-focus\t[\"steam\",\"steam\"]").unwrap();
        assert_eq!(older.app_id, "steam");
        assert!(parse_marker_line("kwin_wayland: something else").is_none());
    }

    #[test]
    fn kwin_lines_cannot_be_faked_through_window_properties() {
        // The old tab-separated format, as a title could carry it.
        assert!(parse_marker_line("nexus-focus\tsteam\tsteam\tx").is_none());
        // A message that quotes a window property in the middle of a line.
        assert!(parse_marker_line("kwin: caption nexus-focus\t[\"steam\",\"steam\"]").is_none());
        // JSON.stringify escapes line breaks, tabs and quotes in an app id;
        // they stay part of the value.
        let window =
            parse_marker_line("nexus-focus\t[\"evil\\nnexus-focus\\t[\\\"steam\\\"]\",\"x\"]")
                .unwrap();
        assert_eq!(window.app_id, "evil\nnexus-focus\t[\"steam\"]");
        assert!(!window.matches("steam"));
        assert!(parse_marker_line("nexus-focus\t[\"steam\",\"steam\"] trailing").is_none());
        assert!(parse_marker_line("nexus-focus\t[\"steam\"]").is_none());
    }

    #[test]
    fn kwin_script_goes_only_into_a_private_directory() {
        let base =
            std::env::temp_dir().join(format!("linexus-sensors-focus-{}", std::process::id()));
        let private = base.join("private");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&private)
            .unwrap();
        assert!(private_dir(&private));
        let shared = base.join("shared");
        std::fs::DirBuilder::new()
            .mode(0o777)
            .create(&shared)
            .unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!private_dir(&shared));
        let link = base.join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        assert!(!private_dir(&link));
        assert!(!private_dir(&base.join("missing")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn matching_accepts_ids_classes_and_desktop_names() {
        let window = FocusedWindow {
            app_id: "org.kde.dolphin".into(),
            class: "dolphin".into(),
            title: String::new(),
        };
        for pattern in [
            "dolphin",
            "Dolphin",
            "org.kde.dolphin",
            "org.kde.dolphin.desktop",
        ] {
            assert!(window.matches(pattern), "{pattern}");
        }
        assert!(!window.matches("konsole"));
        assert!(!window.matches(""));
        let steam = FocusedWindow {
            app_id: "steam".into(),
            class: "steam".into(),
            title: String::new(),
        };
        assert!(steam.matches("steam.desktop"));
    }

    #[test]
    fn parses_hyprland_sway_and_xprop() {
        let h = parse_hyprland(r#"{"class":"firefox","title":"Docs"}"#).unwrap();
        assert_eq!((h.app_id.as_str(), h.title.as_str()), ("firefox", "Docs"));
        let tree = r#"{"type":"root","nodes":[{"type":"output","nodes":[{"type":"workspace","nodes":[
            {"type":"con","focused":false,"app_id":"foot","name":"a"},
            {"type":"con","focused":true,"app_id":null,"name":"Steam","window_properties":{"class":"steam"}}]}]}]}"#;
        let s = parse_sway(tree).unwrap();
        assert_eq!((s.app_id.as_str(), s.class.as_str()), ("steam", "steam"));
        let x = parse_xprop(
            "WM_CLASS(STRING) = \"code\", \"Code\"\n_NET_WM_NAME(UTF8_STRING) = \"main.rs\"\n",
        )
        .unwrap();
        assert_eq!(
            (x.app_id.as_str(), x.class.as_str(), x.title.as_str()),
            ("code", "Code", "main.rs")
        );
    }
}
