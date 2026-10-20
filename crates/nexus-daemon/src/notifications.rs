//! Desktop notifications on the strip.
//!
//! The session bus is only watched while the setting is on. A worker thread
//! prepares each notification away from the render loop: it names the app,
//! asks the desktop whether it would pop the notification up right now (do
//! not disturb, the app's own settings, low urgency) and loads the icons.

use crate::log::{log, Level};
use nexus_render::tiny_skia::Pixmap;
use nexus_sensors::{AppEntry, Notification, NotificationIcon, NotificationMonitor, PopupPolicy};
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const ICON_SIZE: u32 = 48;
/// Icon files larger than this are not read.
const MAX_ICON_FILE_BYTES: u64 = 16 << 20;
/// How long the list of installed apps is reused.
const APPS_FRESH: Duration = Duration::from_secs(60);
/// Wait before trying again when watching could not start (no busctl) or
/// stopped.
const START_RETRY: Duration = Duration::from_secs(30);

/// A notification ready for the strip.
pub(crate) struct Prepared {
    /// The name shown: what the app calls itself, or its menu name.
    pub(crate) app: String,
    /// Every name the app goes by, for muting.
    pub(crate) names: Vec<String>,
    pub(crate) title: String,
    pub(crate) body: String,
    /// A picture of the notification's own, such as a contact photo.
    pub(crate) picture: Option<Arc<Pixmap>>,
    pub(crate) app_icon: Option<Arc<Pixmap>>,
    /// How long the app wants it shown, when it sets a limit.
    pub(crate) timeout: Option<Duration>,
    /// It updates an earlier notification of the same app.
    pub(crate) replaces: bool,
}

pub(crate) struct Notifications {
    allowed: bool,
    running: Option<(NotificationMonitor, Receiver<Prepared>)>,
    retry_at: Option<Instant>,
}

impl Notifications {
    /// Never watches: one-shot renders and tests.
    pub(crate) fn off() -> Self {
        Self {
            allowed: false,
            running: None,
            retry_at: None,
        }
    }

    /// Watches while the setting is on.
    pub(crate) fn on() -> Self {
        Self {
            allowed: true,
            ..Self::off()
        }
    }

    /// Starts or stops watching to match `enabled`, and returns what was
    /// prepared since the last call.
    pub(crate) fn poll(
        &mut self,
        enabled: bool,
        now: Instant,
        logger: &nexus_events::EventLogger,
    ) -> Vec<Prepared> {
        if !(self.allowed && enabled) {
            self.running = None;
            self.retry_at = None;
            return Vec::new();
        }
        if self.running.is_none() && self.retry_at.is_none_or(|at| now >= at) {
            self.running = start();
            self.retry_at = self.running.is_none().then_some(now + START_RETRY);
        }
        let Some((_, prepared)) = &self.running else {
            return Vec::new();
        };
        let mut out = Vec::new();
        loop {
            match prepared.try_recv() {
                Ok(notification) => out.push(notification),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    log(
                        logger,
                        Level::Warn,
                        "notifications_stopped",
                        "desktop notifications stopped; watching again shortly",
                    );
                    self.running = None;
                    self.retry_at = Some(now + START_RETRY);
                    break;
                }
            }
        }
        out
    }
}

fn start() -> Option<(NotificationMonitor, Receiver<Prepared>)> {
    let (monitor, raw) = NotificationMonitor::start()?;
    let (tx, rx) = mpsc::channel();
    // Ends when the monitor is dropped (its sender goes away) or the
    // receiver is.
    thread::Builder::new()
        .name("notify-prepare".into())
        .spawn(move || {
            let mut apps = Apps::default();
            for notification in raw {
                // A notification that trips a decoder is skipped, not the
                // ones after it.
                let prepared = catch_unwind(AssertUnwindSafe(|| {
                    let entry = apps.find(&notification).cloned();
                    let policy = PopupPolicy::read(
                        entry
                            .as_ref()
                            .map(desktop_id)
                            .or(notification.desktop_entry.as_deref()),
                        notification.kde_service.as_deref(),
                    );
                    policy
                        .shows(notification.urgency)
                        .then(|| prepare(notification, entry))
                }));
                if let Ok(Some(prepared)) = prepared {
                    if tx.send(prepared).is_err() {
                        return;
                    }
                }
            }
        })
        .ok()?;
    Some((monitor, rx))
}

fn desktop_id(entry: &AppEntry) -> &str {
    entry.id.trim_end_matches(".desktop")
}

fn prepare(notification: Notification, entry: Option<AppEntry>) -> Prepared {
    let mut names = vec![notification.app_name.clone()];
    names.extend(notification.desktop_entry.clone());
    if let Some(entry) = &entry {
        names.push(entry.name.clone());
        names.push(desktop_id(entry).to_string());
    }
    let mut seen = HashSet::new();
    names.retain(|name| !name.is_empty() && seen.insert(name.to_lowercase()));
    // An application id (org.gnome.Calendar) reads worse than the menu name.
    let app = match &entry {
        Some(entry)
            if notification.app_name.is_empty()
                || notification.desktop_entry.as_deref() == Some(&notification.app_name) =>
        {
            entry.name.clone()
        }
        _ => notification.app_name.clone(),
    };
    let entry_icon = entry
        .and_then(|entry| entry.icon)
        .map(NotificationIcon::Named);
    let app_icon = notification
        .app_icons
        .into_iter()
        .chain(entry_icon)
        .find_map(load_icon);
    Prepared {
        app,
        names,
        title: notification.summary,
        body: notification.body,
        picture: notification.image.and_then(load_icon),
        app_icon,
        timeout: notification.timeout,
        replaces: notification.replaces,
    }
}

fn load_icon(icon: NotificationIcon) -> Option<Arc<Pixmap>> {
    let pixmap = match icon {
        NotificationIcon::Named(name) => {
            let path = nexus_sensors::resolve_icon(&name, ICON_SIZE)?;
            nexus_render::icons::decode_image(&read_icon_file(&path)?, ICON_SIZE)
        }
        NotificationIcon::Encoded(bytes) => nexus_render::icons::decode_image(&bytes, ICON_SIZE),
        NotificationIcon::Pixels {
            width,
            height,
            rgba,
        } => nexus_render::icons::pixmap_from_rgba(width, height, rgba, ICON_SIZE),
    }?;
    Some(Arc::new(pixmap))
}

/// The file's bytes if it is a regular file of a sane size. The checks run
/// on the opened file, so a path swapped for a pipe or device in between
/// cannot slip through, and opening never waits on a pipe.
fn read_icon_file(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_ICON_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_ICON_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_ICON_FILE_BYTES).then_some(bytes)
}

/// The installed apps, listed again once a minute. Entries kept out of the
/// menu count too: notifications often come from helpers like those.
#[derive(Default)]
struct Apps {
    list: Vec<AppEntry>,
    listed: Option<Instant>,
}

impl Apps {
    /// The desktop entry of the sender: the one it names, else one whose
    /// file id or name matches what it calls itself.
    fn find(&mut self, notification: &Notification) -> Option<&AppEntry> {
        if self.listed.is_none_or(|at| at.elapsed() > APPS_FRESH) {
            self.list = nexus_sensors::list_desktop_entries();
            self.listed = Some(Instant::now());
        }
        let by_entry = notification.desktop_entry.as_deref().and_then(|wanted| {
            self.list
                .iter()
                .position(|entry| desktop_id(entry).eq_ignore_ascii_case(wanted))
        });
        let name = notification.app_name.as_str();
        let by_name = || {
            (!name.is_empty())
                .then(|| {
                    self.list.iter().position(|entry| {
                        desktop_id(entry).eq_ignore_ascii_case(name)
                            || entry.name.eq_ignore_ascii_case(name)
                    })
                })
                .flatten()
        };
        by_entry.or_else(by_name).map(|idx| &self.list[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str) -> AppEntry {
        AppEntry {
            id: format!("{id}.desktop"),
            name: name.into(),
            icon: None,
            exec_program: None,
            categories: Vec::new(),
        }
    }

    fn apps() -> Apps {
        Apps {
            list: vec![
                entry("org.gnome.Calendar", "Kalender"),
                entry("org.kde.konsole", "Konsole"),
                entry("discord", "Discord"),
            ],
            listed: Some(Instant::now()),
        }
    }

    fn prepare_with(notification: Notification) -> Prepared {
        let entry = apps().find(&notification).cloned();
        prepare(notification, entry)
    }

    #[test]
    fn names_the_app_by_its_desktop_entry() {
        let gtk = Notification {
            app_name: "org.gnome.Calendar".into(),
            desktop_entry: Some("org.gnome.Calendar".into()),
            summary: "Meeting".into(),
            ..Notification::default()
        };
        let prepared = prepare_with(gtk);
        assert_eq!(prepared.app, "Kalender");
        assert_eq!(prepared.names, vec!["org.gnome.Calendar", "Kalender"]);

        let kde = Notification {
            app_name: "Konsole".into(),
            desktop_entry: Some("org.kde.konsole".into()),
            ..Notification::default()
        };
        let prepared = prepare_with(kde);
        assert_eq!(prepared.app, "Konsole");
        assert_eq!(prepared.names, vec!["Konsole", "org.kde.konsole"]);

        let by_name = Notification {
            app_name: "discord".into(),
            ..Notification::default()
        };
        let prepared = prepare_with(by_name);
        assert_eq!(
            (prepared.app.as_str(), prepared.names),
            ("discord", vec!["discord".to_string()])
        );
        // A desktop entry hint that matches nothing falls back to the name.
        let stale_hint = Notification {
            app_name: "Discord".into(),
            desktop_entry: Some("com.discordapp.Discord".into()),
            ..Notification::default()
        };
        assert!(apps().find(&stale_hint).is_some());
        let unknown = Notification {
            app_name: "dictate".into(),
            ..Notification::default()
        };
        let prepared = prepare_with(unknown);
        assert_eq!(
            (prepared.app.as_str(), prepared.names.len()),
            ("dictate", 1)
        );
        assert!(prepared.picture.is_none() && prepared.app_icon.is_none());
    }

    #[test]
    fn keeps_the_picture_apart_from_the_app_icon() {
        let notification = Notification {
            app_name: "chat".into(),
            image: Some(NotificationIcon::Pixels {
                width: 2,
                height: 2,
                rgba: vec![255; 16],
            }),
            ..Notification::default()
        };
        let prepared = prepare_with(notification);
        let picture = prepared.picture.expect("picture");
        assert_eq!((picture.width(), picture.height()), (2, 2));
        assert!(prepared.app_icon.is_none());
    }

    #[test]
    fn icon_files_must_be_regular_and_small() {
        let dir = std::env::temp_dir().join(format!("nexus-icon-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("icon.png");
        std::fs::write(&file, b"png").unwrap();
        assert_eq!(read_icon_file(&file).as_deref(), Some(&b"png"[..]));
        let fifo = dir.join("pipe");
        let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // Returns at once instead of waiting for a writer.
        assert_eq!(read_icon_file(&fifo), None);
        assert_eq!(read_icon_file(Path::new("/dev/zero")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watches_only_while_enabled() {
        let dir = std::env::temp_dir().join(format!("nexus-notify-poll-{}", std::process::id()));
        let logger = nexus_events::EventLogger::with_path("test", dir.join("events.jsonl"));
        let mut off = Notifications::off();
        assert!(off.poll(true, Instant::now(), &logger).is_empty());
        assert!(off.running.is_none() && off.retry_at.is_none());
        let mut on = Notifications::on();
        assert!(on.poll(false, Instant::now(), &logger).is_empty());
        assert!(on.running.is_none() && on.retry_at.is_none());
    }
}
