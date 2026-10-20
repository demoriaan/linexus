//! Watching the focused app for app-aware profiles.
//!
//! The user service starts with `default.target`, often before the desktop
//! has exported `XDG_CURRENT_DESKTOP` and friends to the systemd user
//! manager. A first attempt then finds no desktop, so a background thread
//! keeps looking (every 5 seconds for two minutes, then every 30) and hands
//! the watcher over once it starts. The render loop never waits for it.

use crate::log::{log_fields, Level};
use nexus_sensors::{FocusWatcher, FocusedWindow};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const RETRY_FAST: Duration = Duration::from_secs(5);
const RETRY_SLOW: Duration = Duration::from_secs(30);
const FAST_FOR: Duration = Duration::from_secs(120);

struct Found {
    watcher: FocusWatcher,
    windows: Receiver<FocusedWindow>,
    /// A problem worth logging, such as a GNOME extension that cannot run.
    problem: Option<String>,
}

pub(crate) struct Focus {
    watcher: Option<(FocusWatcher, Receiver<FocusedWindow>)>,
    search: Option<Receiver<Found>>,
}

impl Focus {
    /// No watching: one-shot renders, the emulator, tests.
    pub(crate) fn off() -> Self {
        Self {
            watcher: None,
            search: None,
        }
    }

    /// Starts looking for a desktop to watch.
    pub(crate) fn on() -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("focus-search".into())
            .spawn(move || {
                let started = Instant::now();
                loop {
                    if let Some((watcher, windows)) =
                        FocusWatcher::start(nexus_actions::session_environment())
                    {
                        let problem = (watcher.backend() == "gnome").then(gnome_problem).flatten();
                        let _ = tx.send(Found {
                            watcher,
                            windows,
                            problem,
                        });
                        return;
                    }
                    thread::sleep(if started.elapsed() < FAST_FOR {
                        RETRY_FAST
                    } else {
                        RETRY_SLOW
                    });
                }
            });
        Self {
            watcher: None,
            search: spawned.is_ok().then_some(rx),
        }
    }

    /// The newest focus change since the last call.
    pub(crate) fn poll(&mut self, logger: &nexus_events::EventLogger) -> Option<FocusedWindow> {
        if let Some(found) = self.search.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.search = None;
            log_fields(
                logger,
                Level::Info,
                "focus_watch",
                "watching the focused app",
                [("backend", found.watcher.backend().to_string())],
            );
            if let Some(problem) = found.problem {
                log_fields(
                    logger,
                    Level::Warn,
                    "focus_watch_problem",
                    &problem,
                    [("backend", found.watcher.backend().to_string())],
                );
            }
            self.watcher = Some((found.watcher, found.windows));
        }
        let (_, windows) = self.watcher.as_ref()?;
        let mut newest = None;
        while let Ok(window) = windows.try_recv() {
            newest = Some(window);
        }
        newest
    }
}

/// Why the GNOME focus extension would not report anything, if it would not.
fn gnome_problem() -> Option<String> {
    let status = nexus_sensors::GnomeExtensionStatus::read();
    if status.complete() {
        return None;
    }
    Some(if status.user_extensions_off {
        "GNOME has extensions turned off, so the Linexus focus extension cannot run".into()
    } else if status.dir.is_none() {
        "the Linexus focus extension is not installed; run `linexusctl gnome-extension install`"
            .into()
    } else {
        "the Linexus focus extension is off or does not list this GNOME Shell version; run `linexusctl doctor --fix`".into()
    })
}
