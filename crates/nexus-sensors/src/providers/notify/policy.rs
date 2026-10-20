//! Whether the desktop would pop a notification up right now. The strip
//! shows a notification only when the screen does.

use super::Urgency;
use crate::providers::apps::ini_value;
use crate::providers::command::run_output;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupPolicy {
    /// The app may pop up at all (GNOME's per-app `enable`, Plasma's "Show
    /// popups").
    pub app_popups: bool,
    /// Do not disturb is on: for everything, or for this app (GNOME's
    /// per-app banners switch).
    pub do_not_disturb: bool,
    /// Critical notifications still pop up during do not disturb.
    pub critical_in_dnd: bool,
    /// This app still pops up during do not disturb (Plasma's "Show in Do
    /// Not Disturb mode").
    pub app_in_dnd: bool,
    /// Low-urgency notifications pop up.
    pub low_urgency: bool,
}

impl Default for PopupPolicy {
    fn default() -> Self {
        Self {
            app_popups: true,
            do_not_disturb: false,
            critical_in_dnd: true,
            app_in_dnd: false,
            low_urgency: true,
        }
    }
}

impl PopupPolicy {
    /// Asks the running notification server and reads its settings for the
    /// sender. `app` is the desktop file id (without `.desktop`) the desktop
    /// knows the sender by, `kde_service` its KDE notification service.
    ///
    /// - GNOME Shell: do not disturb is `show-banners` turned off. The app's
    ///   own `enable` and `show-banners` settings apply too, and low-urgency
    ///   notifications never pop up.
    /// - KDE Plasma: do not disturb is the server's `Inhibited` property.
    ///   The app's (or service's) "Show popups" and "Show in Do Not Disturb
    ///   mode" settings apply, as do the low-priority and critical settings.
    /// - Other servers: the `Inhibited` property, where they have one.
    pub fn read(app: Option<&str>, kde_service: Option<&str>) -> Self {
        let Some(busctl) = crate::which("busctl") else {
            return Self::default();
        };
        match server_name(&busctl).as_deref() {
            Some("gnome-shell") => gnome(app),
            Some("Plasma") => Self {
                do_not_disturb: inhibited(&busctl),
                ..plasma(&plasma_config(), app, kde_service)
            },
            _ => Self {
                do_not_disturb: inhibited(&busctl),
                ..Self::default()
            },
        }
    }

    pub fn shows(&self, urgency: Urgency) -> bool {
        if !self.app_popups || (urgency == Urgency::Low && !self.low_urgency) {
            return false;
        }
        if self.do_not_disturb {
            return (urgency == Urgency::Critical && self.critical_in_dnd) || self.app_in_dnd;
        }
        true
    }
}

fn notifications_call(busctl: &Path, args: &[&str]) -> Option<Value> {
    let mut full = vec![
        "--user",
        "--timeout=1",
        "--json=short",
        args[0],
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    ];
    full.extend_from_slice(&args[1..]);
    let output = run_output(busctl, &full, Duration::from_secs(2)).ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Value>(&output.stdout)
        .ok()?
        .get("data")
        .cloned()
}

/// The notification server's name (`gnome-shell`, `Plasma`, …). When the
/// server does not answer in time, the last answer stands.
fn server_name(busctl: &Path) -> Option<String> {
    static LAST: Mutex<Option<String>> = Mutex::new(None);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(name) = notifications_call(busctl, &["call", "GetServerInformation"])
        .and_then(|data| data.get(0)?.as_str().map(str::to_string))
    {
        *last = Some(name);
    }
    last.clone()
}

fn inhibited(busctl: &Path) -> bool {
    notifications_call(busctl, &["get-property", "Inhibited"])
        .and_then(|data| data.as_bool())
        .unwrap_or(false)
}

fn gnome(app: Option<&str>) -> PopupPolicy {
    let gsettings = crate::which("gsettings");
    let get = |args: &[&str]| -> Option<String> {
        let output = run_output(gsettings.as_ref()?, args, Duration::from_secs(2)).ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let banners = get(&["get", "org.gnome.desktop.notifications", "show-banners"])
        .is_none_or(|value| value.trim() != "false");
    let app_settings = app
        .map(gnome_app_path)
        .and_then(|path| {
            get(&[
                "list-recursively",
                &format!("org.gnome.desktop.notifications.application:{path}"),
            ])
        })
        .unwrap_or_default();
    PopupPolicy {
        app_popups: gnome_flag(&app_settings, "enable"),
        do_not_disturb: !banners || !gnome_flag(&app_settings, "show-banners"),
        critical_in_dnd: true,
        app_in_dnd: false,
        low_urgency: false,
    }
}

/// A switch in `gsettings list-recursively` output (`schema key value`
/// lines); on unless it says `false`.
fn gnome_flag(settings: &str, key: &str) -> bool {
    !settings.lines().any(|line| {
        let mut parts = line.split_whitespace().skip(1);
        parts.next() == Some(key) && parts.next() == Some("false")
    })
}

/// GNOME's settings path for an app: its id in lowercase, with anything but
/// letters, digits and single dashes turned into a dash.
fn gnome_app_path(app: &str) -> String {
    let mut id = String::with_capacity(app.len());
    for ch in app.to_lowercase().chars() {
        let ch = if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            ch
        } else {
            '-'
        };
        if !(ch == '-' && id.ends_with('-')) {
            id.push(ch);
        }
    }
    format!("/org/gnome/desktop/notifications/application/{id}/")
}

/// Contents of `plasmanotifyrc`: the user's file first, then the system
/// defaults (`$XDG_CONFIG_DIRS`, usually `/etc/xdg`).
fn plasma_config() -> Vec<String> {
    let system_dirs = std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/etc/xdg".into());
    std::iter::once(crate::config_home())
        .chain(system_dirs.split(':').map(PathBuf::from))
        .filter_map(|dir| std::fs::read_to_string(dir.join("plasmanotifyrc")).ok())
        .collect()
}

fn plasma(config: &[String], app: Option<&str>, kde_service: Option<&str>) -> PopupPolicy {
    let setting = |group: &str, key: &str, default: bool| {
        config
            .iter()
            .find_map(|text| ini_value(text, group, key))
            .map_or(default, |value| value != "false")
    };
    // Plasma keeps settings per KDE notification service, else per app;
    // apps without a desktop file share the `@other` group.
    let group = match (kde_service, app) {
        (Some(service), _) => format!("Services][{service}"),
        (None, Some(app)) => format!("Applications][{app}"),
        (None, None) => "Applications][@other".to_string(),
    };
    PopupPolicy {
        app_popups: setting(&group, "ShowPopups", true),
        do_not_disturb: false,
        critical_in_dnd: setting("Notifications", "CriticalInDndMode", true),
        app_in_dnd: setting(&group, "ShowPopupsInDndMode", false),
        low_urgency: setting("Notifications", "LowPriorityPopups", true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popups_follow_the_desktop_rules() {
        use Urgency::*;
        let shown = |policy: PopupPolicy| [Low, Normal, Critical].map(|u| policy.shows(u));
        let normal = PopupPolicy::default();
        assert_eq!(shown(normal), [true, true, true]);
        let gnome = PopupPolicy {
            low_urgency: false,
            ..normal
        };
        assert_eq!(shown(gnome), [false, true, true]);
        let quiet = PopupPolicy {
            do_not_disturb: true,
            ..normal
        };
        assert_eq!(shown(quiet), [false, false, true]);
        let quiet_no_critical = PopupPolicy {
            critical_in_dnd: false,
            ..quiet
        };
        assert_eq!(shown(quiet_no_critical), [false, false, false]);
        let allowed_in_dnd = PopupPolicy {
            app_in_dnd: true,
            ..quiet
        };
        assert_eq!(shown(allowed_in_dnd), [true, true, true]);
        let muted_app = PopupPolicy {
            app_popups: false,
            ..normal
        };
        assert_eq!(shown(muted_app), [false, false, false]);
    }

    #[test]
    fn reads_plasma_settings_per_app_and_service() {
        let user = "[Notifications]\nLowPriorityPopups=false\n\n[Applications][org.kde.konsole]\nShowPopups=false\n\n[Services][kdeconnect]\nShowPopupsInDndMode=true\n".to_string();
        let system = "[Applications][org.kde.spectacle]\nShowPopupsInDndMode=true\n\n[Notifications]\nCriticalInDndMode=false\n".to_string();
        let config = [user, system];
        let konsole = plasma(&config, Some("org.kde.konsole"), None);
        assert!(!konsole.app_popups && !konsole.low_urgency && !konsole.critical_in_dnd);
        assert!(plasma(&config, Some("org.kde.spectacle"), None).app_in_dnd);
        assert!(plasma(&config, Some("org.kde.dolphin"), Some("kdeconnect")).app_in_dnd);
        let other = plasma(&config, None, None);
        assert!(other.app_popups && !other.app_in_dnd);
        assert_eq!(plasma(&[], None, None), PopupPolicy::default());
    }

    #[test]
    fn reads_gnome_app_switches() {
        let settings = "org.gnome.desktop.notifications.application application-id 'org.gnome.Calendar.desktop'\norg.gnome.desktop.notifications.application enable true\norg.gnome.desktop.notifications.application show-banners false\n";
        assert!(gnome_flag(settings, "enable"));
        assert!(!gnome_flag(settings, "show-banners"));
        assert!(gnome_flag("", "enable"));
    }

    #[test]
    fn gnome_settings_paths() {
        assert_eq!(
            gnome_app_path("org.gnome.Evolution-alarm-notify"),
            "/org/gnome/desktop/notifications/application/org-gnome-evolution-alarm-notify/"
        );
        assert_eq!(
            gnome_app_path("org.mozilla.firefox"),
            "/org/gnome/desktop/notifications/application/org-mozilla-firefox/"
        );
    }
}
