//! Blanks the strip while the desktop session is locked or the system is
//! about to sleep.
//!
//! logind is asked once a second on a thread of its own: a slow logind or
//! D-Bus must not stall frames and touches.

use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) struct SessionMonitor {
    states: Option<Receiver<bool>>,
}

impl SessionMonitor {
    pub(crate) fn new(enabled: bool) -> Self {
        if !enabled {
            return Self { states: None };
        }
        let mut session = None;
        Self::start(move || session_locked(&mut session) || preparing_for_sleep())
    }

    /// Runs `check` once a second until the monitor is dropped.
    fn start(mut check: impl FnMut() -> bool + Send + 'static) -> Self {
        let (sender, states) = mpsc::channel();
        thread::spawn(move || {
            while sender.send(check()).is_ok() {
                thread::sleep(Duration::from_secs(1));
            }
        });
        Self {
            states: Some(states),
        }
    }

    /// `Some(should_blank)` when a check finished since the last call,
    /// `None` otherwise. Never waits for one.
    pub(crate) fn poll(&mut self) -> Option<bool> {
        self.states.as_ref()?.try_iter().last()
    }
}

/// `session` caches the id of the session to watch; it is looked up again
/// when asking about it fails (logged out, or logind was slow).
fn session_locked(session: &mut Option<String>) -> bool {
    if session.is_none() {
        *session = std::env::var("XDG_SESSION_ID")
            .ok()
            .filter(|id| !id.is_empty())
            .or_else(display_session);
    }
    let Some(id) = session.as_deref() else {
        return false;
    };
    match run_with_timeout(
        "loginctl",
        &["show-session", id, "-p", "LockedHint", "--value"],
    ) {
        Some(out) => truthy(&out),
        None => {
            *session = None;
            false
        }
    }
}

/// The user's graphical session, for a daemon started before the desktop
/// imported `XDG_SESSION_ID` into the user manager.
fn display_session() -> Option<String> {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() }.to_string();
    run_with_timeout("loginctl", &["show-user", &uid, "-p", "Display", "--value"])
        .map(|out| out.trim().to_string())
        .filter(|id| !id.is_empty())
}

fn preparing_for_sleep() -> bool {
    run_with_timeout(
        "busctl",
        &[
            "get-property",
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "PreparingForSleep",
        ],
    )
    .is_some_and(|out| truthy(&out))
}

fn truthy(value: &str) -> bool {
    matches!(value.trim(), "yes" | "true" | "1" | "b true")
}

fn run_with_timeout(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => {
                let output = child.wait_with_output().ok()?;
                return Some(String::from_utf8_lossy(&output.stdout).into_owned());
            }
            Some(_) => return None,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loginctl_and_busctl_booleans() {
        assert!(truthy("yes\n"));
        assert!(truthy("b true"));
        assert!(!truthy("no"));
        assert!(!truthy("b false"));
    }

    #[test]
    fn polling_never_waits_for_a_slow_check() {
        let mut monitor = SessionMonitor::start(|| {
            thread::sleep(Duration::from_millis(400));
            true
        });
        let started = Instant::now();
        assert_eq!(monitor.poll(), None);
        assert!(started.elapsed() < Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(5);
        let state = loop {
            if let Some(state) = monitor.poll() {
                break state;
            }
            assert!(Instant::now() < deadline, "the check never reported");
            thread::sleep(Duration::from_millis(10));
        };
        assert!(state);
        assert_eq!(SessionMonitor::new(false).poll(), None);
    }
}
