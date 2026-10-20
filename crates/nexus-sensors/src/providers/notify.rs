//! Mirrors desktop notifications onto the panel by following the session
//! bus with `busctl --user monitor`, so no D-Bus library is needed. busctl
//! prints every message as one line of JSON with typed values.
//!
//! Apps hand a notification to the desktop in one of three calls:
//! - `org.freedesktop.Notifications.Notify`: libnotify, KDE and most apps;
//! - `org.gtk.Notifications.AddNotification`: GApplication apps on GNOME;
//! - `org.freedesktop.impl.portal.Notification.AddNotification`: the
//!   notification portal (Flatpak apps) passing one on to the desktop.
//!
//! One notification can pass through two of them (the portal hands it on to
//! GNOME Shell or Plasma), so the same app sending the same text again
//! within a few seconds is dropped.

mod policy;
#[cfg(test)]
mod tests;

pub use policy::PopupPolicy;

use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MATCH_RULES: [&str; 3] = [
    "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
    "type='method_call',interface='org.gtk.Notifications',member='AddNotification'",
    "type='method_call',interface='org.freedesktop.impl.portal.Notification',member='AddNotification'",
];
/// Restart delays after busctl exits; reset once a run lasted a minute.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Longer lines are skipped without being parsed.
const MAX_LINE_BYTES: u64 = 16 << 20;
/// Byte arrays longer than this (inline images, about 256 x 256 RGBA) are
/// emptied before a line is parsed: as JSON values every byte would take 32
/// bytes of memory. The notification itself still shows, just without them.
const MAX_BYTE_ARRAY: usize = 256 * 256 * 4;
/// Lines with more JSON values than this (outside byte arrays) are dropped
/// unparsed: a hint of a million empty strings or tiny arrays would take
/// hundreds of megabytes as JSON values. Real notifications have a few
/// dozen.
const MAX_JSON_VALUES: usize = 65_536;
/// The same text again within this window is one notification passed on.
const REPEAT_WINDOW: Duration = Duration::from_secs(3);
/// Remembered GApplication notification ids, to spot updates.
const MAX_REMEMBERED_IDS: usize = 512;
/// Text kept per notification; the strip shows one line of it. Markup is
/// removed from at most four times as much.
const MAX_NAME_CHARS: usize = 128;
const MAX_PATH_CHARS: usize = 4096;
const MAX_SUMMARY_CHARS: usize = 256;
const MAX_BODY_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationIcon {
    /// Icon theme name, absolute path or `file://` URL.
    Named(String),
    /// Contents of an image file (PNG, JPEG, SVG).
    Encoded(Vec<u8>),
    /// Straight RGBA pixels of an inline image.
    Pixels {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notification {
    /// What the sender calls itself: the `app_name` of a `Notify` call or
    /// the application id.
    pub app_name: String,
    /// Desktop file id without `.desktop`, when the sender names one.
    pub desktop_entry: Option<String>,
    /// KDE notification service (`x-kde-appname`), whose settings Plasma
    /// keeps apart from the app's.
    pub kde_service: Option<String>,
    pub summary: String,
    /// Plain text; the markup notifications may use is removed.
    pub body: String,
    /// A picture of the notification's own, such as a contact photo.
    pub image: Option<NotificationIcon>,
    /// Icons of the sending app, best first.
    pub app_icons: Vec<NotificationIcon>,
    pub urgency: Urgency,
    /// How long the sender wants it shown, when it sets a limit.
    pub timeout: Option<Duration>,
    /// It updates an earlier notification of the same app.
    pub replaces: bool,
}

/// Turns `busctl monitor --json=short` lines into notifications.
#[derive(Debug, Default)]
struct MonitorParser {
    /// (app id, notification id) of GApplication and portal notifications.
    seen_ids: HashSet<(String, String)>,
    /// App and text of recent notifications, to drop one passed on.
    recent: VecDeque<(Instant, String)>,
}

impl MonitorParser {
    fn push_line(&mut self, line: &str, now: Instant) -> Option<Notification> {
        let message: Value = serde_json::from_str(&shrink_line(line)?).ok()?;
        if message.get("type")?.as_str()? != "method_call" {
            return None;
        }
        let interface = message.get("interface")?.as_str()?;
        let member = message.get("member")?.as_str()?;
        let data = message.get("payload")?.get("data")?.as_array()?;
        let notification = match (interface, member) {
            ("org.freedesktop.Notifications", "Notify") => from_notify(data)?,
            (
                "org.gtk.Notifications" | "org.freedesktop.impl.portal.Notification",
                "AddNotification",
            ) => {
                let (mut notification, key) = from_add_notification(data)?;
                if self.seen_ids.len() >= MAX_REMEMBERED_IDS {
                    self.seen_ids.clear();
                }
                notification.replaces = !self.seen_ids.insert(key);
                notification
            }
            _ => return None,
        };
        if notification.summary.is_empty() && notification.body.is_empty() {
            return None;
        }
        while self
            .recent
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > REPEAT_WINDOW)
        {
            self.recent.pop_front();
        }
        // The portal and the desktop call the app by its id on both legs.
        let app = notification
            .desktop_entry
            .as_deref()
            .unwrap_or(&notification.app_name)
            .to_lowercase();
        let key = format!("{app}\n{}\n{}", notification.summary, notification.body);
        if self.recent.iter().any(|(_, seen)| *seen == key) {
            return None;
        }
        self.recent.push_back((now, key));
        Some(notification)
    }
}

/// `Notify(app_name, replaces_id, app_icon, summary, body, actions, hints,
/// expire_timeout)`.
fn from_notify(data: &[Value]) -> Option<Notification> {
    let [app_name, replaces_id, app_icon, summary, body, _actions, hints, timeout] = data else {
        return None;
    };
    let hint = |key: &str| hints.get(key).and_then(|hint| hint.get("data"));
    let hint_text = |key: &str, max: usize| {
        hint(key)
            .and_then(Value::as_str)
            .map(|text| clip(text, max))
            .and_then(non_empty)
    };
    // An icon name stands for the app. A file can be anything, a contact
    // photo included (browsers pass a web notification's icon as a file in
    // `app_icon`), so it counts as the notification's own picture.
    let image_path = ["image-path", "image_path"]
        .iter()
        .find_map(|key| hint_text(key, MAX_PATH_CHARS));
    let app_icon = app_icon
        .as_str()
        .map(|icon| clip(icon, MAX_PATH_CHARS))
        .and_then(non_empty);
    let (files, names): (Vec<String>, Vec<String>) = image_path
        .into_iter()
        .chain(app_icon)
        .partition(|icon| icon.contains('/'));
    let image = ["image-data", "image_data", "icon_data"]
        .iter()
        .find_map(|key| hint(key).and_then(inline_image))
        .or_else(|| files.into_iter().next().map(NotificationIcon::Named));
    let urgency = match hint("urgency").and_then(Value::as_u64) {
        Some(0) => Urgency::Low,
        Some(2) => Urgency::Critical,
        _ => Urgency::Normal,
    };
    Some(Notification {
        app_name: clip(app_name.as_str()?, MAX_NAME_CHARS).trim().to_string(),
        desktop_entry: hint_text("desktop-entry", MAX_NAME_CHARS)
            .map(|entry| entry.trim_end_matches(".desktop").to_string()),
        kde_service: hint_text("x-kde-appname", MAX_NAME_CHARS),
        summary: clip(summary.as_str()?, MAX_SUMMARY_CHARS)
            .trim()
            .to_string(),
        body: plain_body(body.as_str()?),
        image,
        app_icons: names.into_iter().map(NotificationIcon::Named).collect(),
        urgency,
        timeout: timeout
            .as_i64()
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms.unsigned_abs())),
        replaces: replaces_id.as_u64().is_some_and(|id| id != 0),
    })
}

/// `AddNotification(app_id, id, fields)` of GApplication and the portal;
/// also returns the key that identifies the notification for updates.
fn from_add_notification(data: &[Value]) -> Option<(Notification, (String, String))> {
    let [app_id, id, fields] = data else {
        return None;
    };
    let app_id = clip(app_id.as_str()?, MAX_NAME_CHARS).trim();
    let id = clip(id.as_str()?, MAX_NAME_CHARS);
    let field = |key: &str| fields.get(key).and_then(|field| field.get("data"));
    let body = match field("markup-body").and_then(Value::as_str) {
        Some(markup) => plain_body(markup),
        None => {
            let body = field("body").and_then(Value::as_str).unwrap_or_default();
            clip(body, MAX_BODY_CHARS).trim().to_string()
        }
    };
    let urgency = match field("priority").and_then(Value::as_str) {
        Some("low") => Urgency::Low,
        Some("urgent") => Urgency::Critical,
        Some(_) => Urgency::Normal,
        None if field("urgent").and_then(Value::as_bool) == Some(true) => Urgency::Critical,
        None => Urgency::Normal,
    };
    // A themed icon names the app's icon; a file or image data is a picture
    // of the notification's own.
    let (image, app_icons) = match field("icon").map(serialized_icon) {
        Some(icons) if matches!(icons.first(), Some(NotificationIcon::Named(name)) if !name.contains('/')) => {
            (None, icons)
        }
        Some(mut icons) if !icons.is_empty() => (Some(icons.remove(0)), Vec::new()),
        _ => (None, Vec::new()),
    };
    let notification = Notification {
        app_name: app_id.to_string(),
        desktop_entry: non_empty(app_id),
        kde_service: None,
        summary: field("title")
            .and_then(Value::as_str)
            .map(|title| clip(title, MAX_SUMMARY_CHARS).trim().to_string())
            .unwrap_or_default(),
        body,
        image,
        app_icons,
        urgency,
        timeout: None,
        replaces: false,
    };
    Some((notification, (app_id.to_string(), id.to_string())))
}

/// A serialized GIcon: `("themed", ["name", …])`, `("file", "file:///…")`,
/// `("bytes", [..])` or a bare icon name. Symbolic names come last, since
/// they draw as a flat silhouette.
fn serialized_icon(value: &Value) -> Vec<NotificationIcon> {
    if let Some(name) = value.as_str().and_then(non_empty) {
        return vec![NotificationIcon::Named(name)];
    }
    let Some([kind, inner]) = value.as_array().map(Vec::as_slice) else {
        return Vec::new();
    };
    let Some(inner) = inner.get("data") else {
        return Vec::new();
    };
    match kind.as_str() {
        Some("themed") => {
            let mut names: Vec<String> = inner
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| non_empty(clip(name.as_str()?, MAX_PATH_CHARS)))
                .filter(|name| !name.contains('/'))
                .collect();
            names.sort_by_key(|name| name.ends_with("-symbolic"));
            names.into_iter().map(NotificationIcon::Named).collect()
        }
        Some("file") => inner
            .as_str()
            .and_then(non_empty)
            .filter(|path| path.contains('/'))
            .map(NotificationIcon::Named)
            .into_iter()
            .collect(),
        Some("bytes") => bytes(inner)
            .filter(|bytes| !bytes.is_empty())
            .map(NotificationIcon::Encoded)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The `image-data` hint: `(width, height, rowstride, has_alpha,
/// bits_per_sample, channels, data)`, 8-bit RGB or RGBA rows.
fn inline_image(value: &Value) -> Option<NotificationIcon> {
    let [width, height, stride, _alpha, bits, channels, data] = value.as_array()?.as_slice() else {
        return None;
    };
    let width = usize::try_from(width.as_u64()?).ok()?;
    let height = usize::try_from(height.as_u64()?).ok()?;
    let stride = usize::try_from(stride.as_u64()?).ok()?;
    let channels = usize::try_from(channels.as_u64()?).ok()?;
    let row = width.checked_mul(channels)?;
    if bits.as_u64()? != 8 || !(channels == 3 || channels == 4) || width == 0 || height == 0 {
        return None;
    }
    let data = bytes(data)?;
    let needed = stride.checked_mul(height - 1)?.checked_add(row)?;
    if stride < row || data.len() < needed {
        return None;
    }
    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for line in 0..height {
        for px in data[line * stride..line * stride + row].chunks_exact(channels) {
            rgba.extend_from_slice(&px[..3]);
            rgba.push(if channels == 4 { px[3] } else { 255 });
        }
    }
    Some(NotificationIcon::Pixels {
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
        rgba,
    })
}

fn bytes(value: &Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|byte| u8::try_from(byte.as_u64()?).ok())
        .collect()
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// At most the first `max` characters of `text`.
fn clip(text: &str, max: usize) -> &str {
    text.char_indices()
        .nth(max)
        .map_or(text, |(end, _)| &text[..end])
}

/// A body as plain text: markup removed from a bounded prefix, then cut to
/// what is kept.
fn plain_body(markup: &str) -> String {
    let plain = strip_markup(clip(markup, MAX_BODY_CHARS * 4));
    clip(plain.trim(), MAX_BODY_CHARS).trim().to_string()
}

/// `line` ready to parse: every JSON array of more than [`MAX_BYTE_ARRAY`]
/// plain numbers (outside strings) replaced by `[]`. None when the rest
/// holds more than [`MAX_JSON_VALUES`] values.
fn shrink_line(line: &str) -> Option<std::borrow::Cow<'_, str>> {
    let bytes = line.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut in_string = false;
    // Commas and openings outside strings; a kept byte array counts once.
    let mut values = 0usize;
    // Bytes in kept byte arrays: one picture's worth per line, so many
    // arrays just under the limit cannot add up.
    let mut kept_bytes = 0usize;
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\\' if in_string => idx += 1,
            b'"' => in_string = !in_string,
            b',' | b'{' if !in_string => values += 1,
            b'[' if !in_string => {
                values += 1;
                let mut end = idx + 1;
                let mut commas = 0;
                while end < bytes.len() && matches!(bytes[end], b'0'..=b'9' | b',' | b' ' | b'-') {
                    commas += usize::from(bytes[end] == b',');
                    end += 1;
                }
                let numbers = end < bytes.len() && bytes[end] == b']';
                if numbers && kept_bytes + commas >= MAX_BYTE_ARRAY {
                    let out = out.get_or_insert_with(|| String::with_capacity(line.len() / 4));
                    out.push_str(&line[copied..idx]);
                    out.push_str("[]");
                    copied = end + 1;
                    idx = end;
                } else if numbers {
                    kept_bytes += commas + 1;
                    idx = end;
                } else {
                    // Not a byte array: its commas count one by one.
                    idx = end - 1;
                    values += commas;
                }
            }
            _ => {}
        }
        if values > MAX_JSON_VALUES {
            return None;
        }
        idx += 1;
    }
    Some(match out {
        Some(mut out) => {
            out.push_str(&line[copied..]);
            out.into()
        }
        None => line.into(),
    })
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
        // Only a known tag looks for its end, so text full of `<` stays
        // linear.
        match is_tag.then(|| after.find('>')).flatten() {
            Some(end) => {
                if name == "br" || (name == "p" && !after.starts_with('/') && !out.is_empty()) {
                    out.push('\n');
                }
                rest = &after[end + 1..];
            }
            None => {
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
        // Entities are short; looking further would make text full of `&`
        // quadratic.
        let end = after.bytes().take(11).position(|byte| byte == b';');
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

/// Keeps a `busctl monitor` child running and turns its output into
/// [`Notification`]s. Dropping the monitor stops the child and the reader.
pub struct NotificationMonitor {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl NotificationMonitor {
    /// Starts watching, or None when busctl (systemd) is not installed. If
    /// the child dies (session bus restart), it is restarted after 1 s,
    /// doubling up to 60 s while it keeps failing.
    pub fn start() -> Option<(NotificationMonitor, Receiver<Notification>)> {
        Self::start_with(crate::which("busctl")?)
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
    let mut parser = MonitorParser::default();
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
        let receiver_gone = stdout.is_some_and(|stdout| !forward(stdout, &mut parser, tx));
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
    let mut command = Command::new(program);
    command.args(["--user", "monitor", "--json=short"]);
    for rule in MATCH_RULES {
        command.arg(format!("--match={rule}"));
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// Reads until the child's output ends. False when the receiver was dropped.
fn forward(
    stdout: std::process::ChildStdout,
    parser: &mut MonitorParser,
    tx: &Sender<Notification>,
) -> bool {
    let mut reader = BufReader::new(stdout);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader
            .by_ref()
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut raw)
        {
            Ok(0) | Err(_) => return true,
            Ok(_) => {}
        }
        if raw.last() != Some(&b'\n') && raw.len() as u64 >= MAX_LINE_BYTES {
            // Too long to parse: skip the rest of the line.
            if reader.skip_until(b'\n').is_err() {
                return true;
            }
            continue;
        }
        let line = String::from_utf8_lossy(&raw);
        if let Some(notification) = parser.push_line(line.trim_end(), Instant::now()) {
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
