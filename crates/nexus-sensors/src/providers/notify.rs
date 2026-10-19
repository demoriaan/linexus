//! Mirrors desktop notifications onto the panel by watching the session
//! bus with `dbus-monitor`, so no D-Bus library is needed.
//!
//! dbus-monitor prints strings raw: quotes inside them are not escaped and
//! line breaks are literal. A `Notify` call always has the same eight
//! arguments, so the parser anchors on the structure around the strings
//! (the `uint32` id after the app name; the two arrays and the closing
//! `int32` timeout after the body) and lets the body absorb anything odd.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MATCH_RULE: &str =
    "type='method_call',interface='org.freedesktop.Notifications',member='Notify'";
/// Restart delays after dbus-monitor exits; reset once a run lasted a minute.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Text kept per message; beyond this only structural lines are stored
/// (inline images in hints print as huge hex dumps).
const MAX_BLOCK_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub app_name: String,
    pub summary: String,
    /// Plain text; the markup notifications may use is removed.
    pub body: String,
    /// Icon name or `file://` URL as sent; may be empty.
    pub icon: String,
}

/// Every `Notify` call in a chunk of dbus-monitor output.
pub fn parse_dbus_monitor(text: &str) -> Vec<Notification> {
    let mut parser = NotifyParser::default();
    let mut out: Vec<Notification> = text
        .split('\n')
        .filter_map(|line| parser.push_line(line))
        .collect();
    out.extend(parser.finish());
    out
}

/// Line-by-line parser that emits a notification as soon as its final
/// argument arrives, rather than waiting for the next bus message.
#[derive(Debug, Default)]
struct NotifyParser {
    block: Option<Vec<String>>,
    block_bytes: usize,
}

impl NotifyParser {
    fn push_line(&mut self, line: &str) -> Option<Notification> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if is_message_header(line) {
            let previous = self.finish();
            if line.starts_with("method call ")
                && line.contains("interface=org.freedesktop.Notifications;")
                && line.ends_with("member=Notify")
            {
                self.block = Some(Vec::new());
                self.block_bytes = 0;
            }
            return previous;
        }
        let block = self.block.as_mut()?;
        if self.block_bytes < MAX_BLOCK_BYTES || is_top_level(line) {
            self.block_bytes += line.len() + 1;
            block.push(line.to_string());
        }
        if is_top_level(line) && line.starts_with("   int32 ") {
            if let Some(notification) = parse_notify_args(block) {
                self.block = None;
                return Some(notification);
            }
        }
        None
    }

    /// Ends the pending message (stream closed or next header seen).
    fn finish(&mut self) -> Option<Notification> {
        self.block
            .take()
            .and_then(|block| parse_notify_args(&block))
    }
}

fn is_message_header(line: &str) -> bool {
    ["method call ", "method return ", "signal ", "error "]
        .iter()
        .any(|prefix| line.starts_with(prefix) && line.contains(" time="))
}

/// Arguments sit at three spaces; nested values are indented further.
fn is_top_level(line: &str) -> bool {
    line.len() > 3 && line.starts_with("   ") && !line[3..].starts_with(' ')
}

/// `lines` are everything after the header: string app_name, uint32
/// replaces_id, string icon, string summary, string body, array actions,
/// array hints, int32 timeout.
fn parse_notify_args(lines: &[String]) -> Option<Notification> {
    let last = lines.len().checked_sub(1)?;
    if !(is_top_level(&lines[last]) && lines[last].starts_with("   int32 ")) {
        return None;
    }
    // From the end: hints array, then actions array.
    let hints_end = last.checked_sub(1).filter(|&idx| lines[idx] == "   ]")?;
    let hints_start = (0..hints_end)
        .rev()
        .find(|&idx| lines[idx] == "   array [")?;
    let actions_end = hints_start
        .checked_sub(1)
        .filter(|&idx| lines[idx] == "   ]")?;
    let actions_start = (0..actions_end)
        .rev()
        .find(|&idx| lines[idx] == "   array [")?;
    // From the start: app name up to the replaces_id.
    let id_line = (1..actions_start).find(|&idx| {
        lines[idx]
            .strip_prefix("   uint32 ")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
    })?;
    let app_name = string_value(&lines[..id_line])?;
    // Icon, summary, body: split where a new top-level string starts; extra
    // splits (a body line that looks like one) are folded back into the body.
    let middle = &lines[id_line + 1..actions_start];
    let starts: Vec<usize> = (0..middle.len())
        .filter(|&idx| middle[idx].starts_with("   string \""))
        .collect();
    if starts.len() < 3 || starts[0] != 0 {
        return None;
    }
    let icon = string_value(&middle[starts[0]..starts[1]])?;
    let summary = string_value(&middle[starts[1]..starts[2]])?;
    let body = string_value(&middle[starts[2]..])?;
    Some(Notification {
        app_name: app_name.trim().to_string(),
        summary: summary.trim().to_string(),
        body: strip_markup(&body).trim().to_string(),
        icon: icon.trim().to_string(),
    })
}

/// `   string "…"` possibly spanning several lines.
fn string_value(lines: &[String]) -> Option<String> {
    let joined = lines.join("\n");
    let value = joined.strip_prefix("   string \"")?;
    Some(value.strip_suffix('"').unwrap_or(value).to_string())
}

/// Removes the tags the notification spec allows (`b`, `i`, `u`, `a`, `img`)
/// plus common HTML ones senders use anyway, turns `<br>` into a line break
/// and decodes entities. Other `<` characters ("a < b") are left alone.
fn strip_markup(text: &str) -> String {
    const TAGS: &[&str] = &[
        "b", "i", "u", "a", "img", "br", "p", "span", "s", "em", "strong", "font", "tt", "big",
        "small", "sub", "sup",
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_start = after.strip_prefix('/').unwrap_or(after);
        let name: String = name_start
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let next = name_start[name.len()..].chars().next();
        let is_tag =
            TAGS.contains(&name.as_str()) && matches!(next, Some('>' | '/' | ' ' | '\t' | '\n'));
        match (is_tag, after.find('>')) {
            (true, Some(end)) => {
                if name == "br" || (name == "p" && !after.starts_with('/') && !out.is_empty()) {
                    out.push('\n');
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find(';').filter(|&end| end <= 10);
        let decoded = end.and_then(|end| {
            let entity = &after[1..end];
            match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                    .and_then(char::from_u32),
            }
            .map(|ch| (ch, end))
        });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Keeps a `dbus-monitor` child running and turns its output into
/// [`Notification`]s. Dropping the monitor stops the child and the reader.
pub struct NotificationMonitor {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl NotificationMonitor {
    /// Starts watching, or None when dbus-monitor is not installed. If the
    /// child dies (session bus restart), it is restarted after 1 s, doubling
    /// up to 60 s while it keeps failing.
    pub fn start() -> Option<(NotificationMonitor, Receiver<Notification>)> {
        Self::start_with(crate::which("dbus-monitor")?)
    }

    fn start_with(program: PathBuf) -> Option<(NotificationMonitor, Receiver<Notification>)> {
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let monitor = NotificationMonitor {
            stop: stop.clone(),
            child: child.clone(),
        };
        thread::Builder::new()
            .name("notify-monitor".into())
            .spawn(move || supervise(&program, &stop, &child, &tx))
            .ok()?;
        Some((monitor, rx))
    }
}

impl Drop for NotificationMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let mut slot = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(child) = slot.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *slot = None;
    }
}

fn supervise(
    program: &Path,
    stop: &AtomicBool,
    slot: &Mutex<Option<Child>>,
    tx: &Sender<Notification>,
) {
    let mut backoff = BACKOFF_START;
    loop {
        let started = Instant::now();
        // Spawning under the lock pairs with Drop: either Drop sees this
        // child and kills it, or this sees the stop flag first.
        let stdout = {
            let mut guard = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match spawn_monitor(program) {
                Some(mut child) => {
                    let stdout = child.stdout.take();
                    *guard = Some(child);
                    stdout
                }
                None => None,
            }
        };
        let receiver_gone = stdout.is_some_and(|stdout| !forward(stdout, tx));
        if let Some(mut child) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        if receiver_gone || stop.load(Ordering::SeqCst) {
            return;
        }
        if started.elapsed() > BACKOFF_MAX {
            backoff = BACKOFF_START;
        }
        if !sleep_unless_stopped(backoff, stop) {
            return;
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

fn spawn_monitor(program: &Path) -> Option<Child> {
    Command::new(program)
        .args(["--session", "--monitor", MATCH_RULE])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// Reads until the child's output ends. False when the receiver was dropped.
fn forward(stdout: std::process::ChildStdout, tx: &Sender<Notification>) -> bool {
    let mut reader = BufReader::new(stdout);
    let mut parser = NotifyParser::default();
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return true,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.strip_suffix('\n').unwrap_or(&line);
        if let Some(notification) = parser.push_line(line) {
            if tx.send(notification).is_err() {
                return false;
            }
        }
    }
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    !stop.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from dbus-monitor on a private bus, with a notify-send style
    /// hint block.
    const CAPTURE: &str = r#"signal time=1792371935.587466 sender=org.freedesktop.DBus -> destination=:1.0 serial=2 path=/org/freedesktop/DBus; interface=org.freedesktop.DBus; member=NameAcquired
   string ":1.0"
method call time=1792371936.097278 sender=:1.1 -> destination=org.example.NoSuchNotifier serial=2 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify
   string "my "app""
   uint32 0
   string "dialog-information"
   string "Sum "quoted" mary"
   string "Line one <b>bold</b> &amp; more
   string "fake"
Line three \" back"
   array [
      string "default"
      string "Open"
   ]
   array [
      dict entry(
         string "urgency"
         variant             byte 1
      )
      dict entry(
         string "sender-pid"
         variant             int64 4242
      )
   ]
   int32 -1
method call time=1792371936.099978 sender=:1.2 -> destination=org.example.NoSuchNotifier serial=2 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify
   string "second"
   uint32 7
   string ""
   string "S2"
   string ""
   array [
   ]
   array [
   ]
   int32 5000
"#;

    #[test]
    fn parses_notify_calls_with_raw_quotes_and_newlines() {
        let notifications = parse_dbus_monitor(CAPTURE);
        assert_eq!(notifications.len(), 2);
        assert_eq!(
            notifications[0],
            Notification {
                app_name: "my \"app\"".into(),
                summary: "Sum \"quoted\" mary".into(),
                body: "Line one bold & more\n   string \"fake\"\nLine three \\\" back".into(),
                icon: "dialog-information".into(),
            }
        );
        assert_eq!(
            notifications[1],
            Notification {
                app_name: "second".into(),
                summary: "S2".into(),
                body: String::new(),
                icon: String::new(),
            }
        );
    }

    #[test]
    fn emits_as_soon_as_the_timeout_argument_arrives() {
        let mut parser = NotifyParser::default();
        let mut emitted = Vec::new();
        for line in CAPTURE.lines() {
            if let Some(notification) = parser.push_line(line) {
                emitted.push(notification.app_name);
            }
        }
        // Both arrive on their `int32` line; nothing is left pending.
        assert_eq!(emitted, vec!["my \"app\"", "second"]);
        assert_eq!(parser.finish(), None);
    }

    #[test]
    fn a_body_line_that_looks_like_the_timeout_does_not_end_the_message() {
        let text = "method call time=1.0 sender=:1.5 -> destination=:1.6 serial=3 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify
   string \"app\"
   uint32 0
   string \"\"
   string \"Title\"
   string \"first
   int32 5
last\"
   array [
   ]
   array [
   ]
   int32 -1
";
        let notifications = parse_dbus_monitor(text);
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].body, "first\n   int32 5\nlast");
    }

    #[test]
    fn ignores_other_messages_and_truncated_input() {
        assert!(parse_dbus_monitor("signal time=1.0 sender=x -> destination=y serial=1 path=/; interface=a; member=b\n   string \"x\"\n").is_empty());
        let truncated = CAPTURE.split("   array [").next().unwrap();
        assert!(parse_dbus_monitor(truncated)
            .iter()
            .all(|n| n.app_name != "my \"app\""));
    }

    #[test]
    fn strips_notification_markup() {
        assert_eq!(
            strip_markup("<b>Bold</b> <i>it</i> <a href=\"https://x.test\">link</a><br/>next"),
            "Bold it link\nnext"
        );
        assert_eq!(strip_markup("a < b and c > d"), "a < b and c > d");
        assert_eq!(strip_markup("<img src=\"x.png\" alt=\"\"/>text"), "text");
        assert_eq!(
            strip_markup("&lt;tag&gt; &#228;&#xE4; &bogus; AT&T"),
            "<tag> ää &bogus; AT&T"
        );
        assert_eq!(
            strip_markup("<unknown>kept</unknown>"),
            "<unknown>kept</unknown>"
        );
    }

    #[test]
    fn monitor_forwards_restarts_and_stops_on_drop() {
        // A stand-in for dbus-monitor that prints the capture and exits, so
        // no session bus is involved.
        let dir = std::env::temp_dir().join(format!("nexus-notify-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let capture = dir.join("capture.txt");
        std::fs::write(&capture, CAPTURE).unwrap();
        let script = dir.join("fake-dbus-monitor");
        std::fs::write(&script, format!("#!/bin/sh\ncat '{}'\n", capture.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (monitor, rx) = NotificationMonitor::start_with(script).unwrap();
        let wait = Duration::from_secs(10);
        let names: Vec<String> = (0..3)
            .map(|_| rx.recv_timeout(wait).expect("notification").app_name)
            .collect();
        // The third one comes from the restarted child.
        assert_eq!(names, vec!["my \"app\"", "second", "my \"app\""]);

        drop(monitor);
        let started = Instant::now();
        while rx.recv_timeout(Duration::from_millis(50)).is_ok() {}
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
