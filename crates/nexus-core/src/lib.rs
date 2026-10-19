use std::ffi::CStr;
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const VENDOR_ID: u16 = 0x1b1c;
pub const PRODUCT_ID: u16 = 0x1b8e;
pub const OPERATIONAL_USAGE_PAGE: u16 = 0x000c;
pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 48;
pub const PIXEL_STRIDE: usize = 4;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * PIXEL_STRIDE;
pub const CONTROL_REPORT_ID: u8 = 0x03;
pub const INFO_REPORT_ID: u8 = 0x05;
pub const IMAGE_REPORT_ID: u8 = 0x02;
pub const TOUCH_REPORT_ID: u8 = 0x01;
pub const IMAGE_REPORT_TOTAL_BYTES: usize = 1024;
pub const IMAGE_HEADER_BYTES: usize = 8;
pub const IMAGE_CHUNK_PAYLOAD_BYTES: usize = IMAGE_REPORT_TOTAL_BYTES - IMAGE_HEADER_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    SetBacklight = 0x01,
    BlankScreen = 0x04,
    PlayAnimation = 0x0d,
    StopAnimation = 0x0f,
}

pub fn brightness_report(percent: u8) -> [u8; 3] {
    [
        CONTROL_REPORT_ID,
        ControlCommand::SetBacklight as u8,
        percent.min(100),
    ]
}

pub fn blank_report() -> [u8; 2] {
    [CONTROL_REPORT_ID, ControlCommand::BlankScreen as u8]
}

pub fn play_animation_report(index: u8, looped: bool) -> [u8; 4] {
    [
        CONTROL_REPORT_ID,
        ControlCommand::PlayAnimation as u8,
        index,
        u8::from(looped),
    ]
}

pub fn stop_animation_report() -> [u8; 2] {
    [CONTROL_REPORT_ID, ControlCommand::StopAnimation as u8]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReport {
    pub block_number: u16,
    pub is_last: bool,
    pub payload_len: usize,
    pub bytes: [u8; IMAGE_REPORT_TOTAL_BYTES],
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("frame must be {expected} bytes, got {actual}")]
    InvalidFrameSize { expected: usize, actual: usize },
}

/// 4x4 ordered-dither thresholds.
const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Converts an RGBA frame into the panel's wire format.
///
/// The firmware reads pixels as B, G, R, X (verified on hardware by
/// isolating one byte at a time; Corsair's own SDK writes BGR(X) too). The
/// panel shows 18-bit colour and drops the two low bits of each channel, so
/// an ordered dither is added first: gradients and anti-aliased edges keep
/// their in-between shades instead of banding.
pub fn panel_frame(rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (i, (src, dst)) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0.iter_mut())
        .enumerate()
    {
        let (x, y) = (i % WIDTH, i / WIDTH);
        let bump = BAYER_4X4[y & 3][x & 3] >> 2;
        let dither = |v: u8| v.saturating_add(bump);
        dst[0] = dither(src[2]);
        dst[1] = dither(src[1]);
        dst[2] = dither(src[0]);
        dst[3] = 0xff;
    }
    out
}

/// Splits an RGBA frame into image output reports (wire format applied).
pub fn image_reports(frame: &[u8]) -> Result<Vec<ImageReport>, ProtocolError> {
    if frame.len() != FRAME_BYTES {
        return Err(ProtocolError::InvalidFrameSize {
            expected: FRAME_BYTES,
            actual: frame.len(),
        });
    }
    let frame = panel_frame(frame);

    let mut reports = Vec::with_capacity(FRAME_BYTES.div_ceil(IMAGE_CHUNK_PAYLOAD_BYTES));
    for (block_number, chunk) in frame.chunks(IMAGE_CHUNK_PAYLOAD_BYTES).enumerate() {
        let mut bytes = [0u8; IMAGE_REPORT_TOTAL_BYTES];
        let is_last = (block_number + 1) * IMAGE_CHUNK_PAYLOAD_BYTES >= FRAME_BYTES;
        let block_number = block_number as u16;
        bytes[0] = IMAGE_REPORT_ID;
        bytes[1] = 0x05;
        bytes[2] = 0x40;
        bytes[3] = u8::from(is_last);
        bytes[4] = (block_number & 0xff) as u8;
        bytes[5] = (block_number >> 8) as u8;
        bytes[6] = (chunk.len() & 0xff) as u8;
        bytes[7] = (chunk.len() >> 8) as u8;
        bytes[IMAGE_HEADER_BYTES..IMAGE_HEADER_BYTES + chunk.len()].copy_from_slice(chunk);
        reports.push(ImageReport {
            block_number,
            is_last,
            payload_len: chunk.len(),
            bytes,
        });
    }
    Ok(reports)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchEvent {
    pub phase: TouchPhase,
    pub x: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Tap { x: u16 },
    DoubleTap { x: u16 },
    Hold { x: u16 },
    Jitter { x: u16 },
    SwipeLeft { x: u16 },
    SwipeRight { x: u16 },
}

#[derive(Debug, Default)]
pub struct TouchParser {
    /// A `Began` went out and its `Ended` has not.
    active: bool,
    /// Last position on the panel while active.
    last_x: u16,
}

impl TouchParser {
    pub fn parse_report(&mut self, bytes: &[u8]) -> Option<TouchEvent> {
        if bytes.len() < 8 || bytes[0] != TOUCH_REPORT_ID || bytes[1] != 0x02 || bytes[2] != 0x21 {
            return None;
        }
        let next_active = bytes[5] != 0;
        let raw_x = u16::from_le_bytes([bytes[6], bytes[7]]);
        if raw_x as usize >= WIDTH {
            // Positions off the panel are dropped, but a lift still ends the
            // touch where it was last seen: a lost `Ended` leaves the press
            // (and any repeating hold) running.
            if self.active && !next_active {
                self.active = false;
                return Some(TouchEvent {
                    phase: TouchPhase::Ended,
                    x: self.last_x,
                });
            }
            return None;
        }

        let phase = match (self.active, next_active) {
            (false, true) => Some(TouchPhase::Began),
            (true, true) => Some(TouchPhase::Moved),
            (true, false) => Some(TouchPhase::Ended),
            (false, false) => None,
        }?;
        self.active = next_active;
        self.last_x = raw_x;
        Some(TouchEvent { phase, x: raw_x })
    }
}

#[derive(Debug, Clone)]
pub struct GestureRecognizer {
    pub swipe_threshold: i32,
    pub hold_duration: Duration,
    pub double_tap_window: Duration,
    pub double_tap_distance: u16,
    first_x: Option<u16>,
    last_x: u16,
    press_start: Option<Instant>,
    pending_tap: Option<(Instant, u16)>,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self {
            swipe_threshold: 80,
            hold_duration: Duration::from_millis(600),
            double_tap_window: Duration::from_millis(300),
            double_tap_distance: 80,
            first_x: None,
            last_x: 0,
            press_start: None,
            pending_tap: None,
        }
    }
}

impl GestureRecognizer {
    pub fn feed(&mut self, event: TouchEvent) -> Option<Gesture> {
        self.feed_at(event, Instant::now())
    }

    /// Classify a touch by press duration, not by x-movement: the Nexus strip
    /// only reports X, so quick contacts wobble and used to misfire as holds or
    /// jitters. A short press is a `Tap` (activate), a press held past
    /// `hold_duration` is a `Hold` (dismiss), and a fast horizontal drag past
    /// `swipe_threshold` is a swipe (page navigation).
    ///
    /// Single `Tap`s are deferred by [`Self::double_tap_window`] so a second
    /// tap that arrives within that window collapses both into a single
    /// `DoubleTap`. The deferred tap is flushed either by a follow-up call to
    /// [`Self::poll`] from the touch loop, or by the next gesture that
    /// happens to land within the window.
    pub fn feed_at(&mut self, event: TouchEvent, now: Instant) -> Option<Gesture> {
        match event.phase {
            TouchPhase::Began => {
                self.first_x = Some(event.x);
                self.last_x = event.x;
                self.press_start = Some(now);
                None
            }
            TouchPhase::Moved => {
                self.last_x = event.x;
                None
            }
            TouchPhase::Ended => {
                let first = self.first_x.take().unwrap_or(event.x);
                let held = self
                    .press_start
                    .take()
                    .map(|start| now.saturating_duration_since(start))
                    .unwrap_or_default();
                let diff = self.last_x as i32 - first as i32;
                let raw = if held >= self.hold_duration {
                    Gesture::Hold { x: first }
                } else if diff > self.swipe_threshold {
                    Gesture::SwipeRight { x: first }
                } else if diff < -self.swipe_threshold {
                    Gesture::SwipeLeft { x: first }
                } else {
                    Gesture::Tap { x: first }
                };
                self.collapse_with_pending(raw, now)
            }
        }
    }

    /// Drains a deferred tap once its double-tap window has elapsed. The
    /// daemon's touch loop should call this every iteration so a real
    /// single-tap still fires when no follow-up second tap arrives.
    pub fn poll(&mut self, now: Instant) -> Option<Gesture> {
        let &(time, x) = self.pending_tap.as_ref()?;
        if now.saturating_duration_since(time) >= self.double_tap_window {
            self.pending_tap = None;
            Some(Gesture::Tap { x })
        } else {
            None
        }
    }

    fn collapse_with_pending(&mut self, gesture: Gesture, now: Instant) -> Option<Gesture> {
        if let Gesture::Tap { x } = gesture {
            if let Some((prev_time, prev_x)) = self.pending_tap.take() {
                if now.saturating_duration_since(prev_time) <= self.double_tap_window
                    && x.abs_diff(prev_x) <= self.double_tap_distance
                {
                    return Some(Gesture::DoubleTap { x });
                }
                // The earlier tap aged out before its flush; emit it now and
                // hold the new tap so it can still pair with a following one.
                self.pending_tap = Some((now, x));
                return Some(Gesture::Tap { x: prev_x });
            }
            self.pending_tap = Some((now, x));
            None
        } else {
            self.pending_tap = None;
            Some(gesture)
        }
    }
}

#[derive(Debug, Clone)]
pub struct HidrawCandidate {
    pub devnode: PathBuf,
    pub hid_id: Option<String>,
    pub hid_name: Option<String>,
    pub hid_uniq: Option<String>,
    pub id_vendor: Option<String>,
    pub id_product: Option<String>,
    pub permissions_octal: Option<u32>,
    pub can_read_write: bool,
}

impl HidrawCandidate {
    pub fn is_nexus_vid_pid(&self) -> bool {
        let from_hid_id = self
            .hid_id
            .as_deref()
            .map(|id| id.to_ascii_lowercase().contains("00001b1c:00001b8e"))
            .unwrap_or(false);
        let from_props =
            self.id_vendor.as_deref() == Some("1b1c") && self.id_product.as_deref() == Some("1b8e");
        from_hid_id || from_props
    }
}

#[derive(Debug, Clone)]
pub struct HidapiDeviceSummary {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface_number: i32,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

impl HidapiDeviceSummary {
    pub fn is_nexus(&self) -> bool {
        self.vendor_id == VENDOR_ID && self.product_id == PRODUCT_ID
    }

    pub fn is_operational_candidate(&self) -> bool {
        self.is_nexus() && self.usage_page == OPERATIONAL_USAGE_PAGE
    }
}

pub fn enumerate_hidapi() -> Result<Vec<HidapiDeviceSummary>, hidapi::HidError> {
    let api = hidapi::HidApi::new()?;
    Ok(api
        .device_list()
        .map(|d| HidapiDeviceSummary {
            path: cstr_to_string(d.path()),
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            interface_number: d.interface_number(),
            manufacturer: d.manufacturer_string().map(ToOwned::to_owned),
            product: d.product_string().map(ToOwned::to_owned),
            serial: d.serial_number().map(ToOwned::to_owned),
        })
        .collect())
}

fn cstr_to_string(s: &CStr) -> String {
    s.to_string_lossy().into_owned()
}

pub fn enumerate_hidraw_sysfs() -> Vec<HidrawCandidate> {
    let mut candidates = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return candidates;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let devnode = PathBuf::from("/dev").join(name.as_ref());
        let device_dir = entry.path().join("device");
        let uevent = fs::read_to_string(device_dir.join("uevent")).unwrap_or_default();
        let mut candidate = HidrawCandidate {
            devnode: devnode.clone(),
            hid_id: None,
            hid_name: None,
            hid_uniq: None,
            id_vendor: None,
            id_product: None,
            permissions_octal: fs::metadata(&devnode)
                .ok()
                .map(|m| m.permissions().mode() & 0o7777),
            can_read_write: fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&devnode)
                .is_ok(),
        };
        for line in uevent.lines() {
            if let Some((key, value)) = line.split_once('=') {
                match key {
                    "HID_ID" => candidate.hid_id = Some(value.to_string()),
                    "HID_NAME" => candidate.hid_name = Some(value.to_string()),
                    "HID_UNIQ" => candidate.hid_uniq = Some(value.to_string()),
                    _ => {}
                }
            }
        }
        // Some systems expose USB attrs higher in sysfs; keep best-effort empty for now.
        candidates.push(candidate);
    }
    candidates.sort_by(|a, b| a.devnode.cmp(&b.devnode));
    candidates
}

/// The udev rule shipped in `packaging/udev/99-linexus.rules`.
pub fn udev_rule_text() -> &'static str {
    include_str!("../../../packaging/udev/99-linexus.rules")
}

/// Whether an installed rule file grants the same access as ours. Comments
/// and blank lines are ignored, so reworded comments do not flag an
/// otherwise current rule as outdated.
pub fn udev_rule_matches(installed: &str) -> bool {
    fn rules(text: &str) -> Vec<&str> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect()
    }
    rules(installed) == rules(udev_rule_text())
}

pub fn is_probably_permission_error(error: &hidapi::HidError) -> bool {
    error
        .to_string()
        .to_ascii_lowercase()
        .contains("permission")
        || error.to_string().to_ascii_lowercase().contains("access")
}

impl fmt::Display for Gesture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Gesture::Tap { x } => write!(f, "tap @ {x}"),
            Gesture::DoubleTap { x } => write!(f, "double-tap @ {x}"),
            Gesture::Hold { x } => write!(f, "hold @ {x}"),
            Gesture::Jitter { x } => write!(f, "jitter @ {x}"),
            Gesture::SwipeLeft { x } => write!(f, "swipe-left @ {x}"),
            Gesture::SwipeRight { x } => write!(f, "swipe-right @ {x}"),
        }
    }
}

#[cfg(test)]
mod protocol_tests {
    #[test]
    fn udev_rule_comparison_ignores_comments() {
        let reworded = udev_rule_text().replace("# uaccess", "# The uaccess tag");
        assert!(udev_rule_matches(&reworded));
        assert!(udev_rule_matches(&format!("\n{}\n\n", udev_rule_text())));
        let other = udev_rule_text().replace("MODE=\"0660\"", "MODE=\"0666\"");
        assert!(!udev_rule_matches(&other));
        assert!(!udev_rule_matches("# only a comment\n"));
    }

    #[test]
    fn panel_frames_are_bgrx_with_a_small_dither() {
        let mut rgba = vec![0u8; super::FRAME_BYTES];
        rgba[..4].copy_from_slice(&[200, 100, 10, 255]);
        let wire = super::panel_frame(&rgba);
        // Pixel (0, 0) has dither threshold 0: channels are swapped as is.
        assert_eq!(&wire[..4], &[10, 100, 200, 0xff]);
        // Black pixels move by at most 3 per channel and stay opaque.
        assert!(wire[4..]
            .chunks(4)
            .all(|px| px[..3].iter().all(|v| *v <= 3) && px[3] == 0xff));
    }

    use super::*;

    #[test]
    fn builds_control_feature_reports() {
        assert_eq!(brightness_report(80), [0x03, 0x01, 80]);
        assert_eq!(brightness_report(250), [0x03, 0x01, 100]);
        assert_eq!(blank_report(), [0x03, 0x04]);
        assert_eq!(play_animation_report(2, true), [0x03, 0x0d, 2, 1]);
        assert_eq!(stop_animation_report(), [0x03, 0x0f]);
    }

    #[test]
    fn chunks_full_frame_like_reference_implementations() {
        let frame = vec![0xa5; FRAME_BYTES];
        let reports = image_reports(&frame).unwrap();
        assert_eq!(FRAME_BYTES, 122_880);
        assert_eq!(reports.len(), 121);
        for (i, report) in reports.iter().enumerate() {
            assert_eq!(report.bytes[0], 0x02);
            assert_eq!(report.bytes[1], 0x05);
            assert_eq!(report.bytes[2], 0x40);
            assert_eq!(report.block_number as usize, i);
            assert_eq!(report.bytes[4], (i & 0xff) as u8);
            assert_eq!(report.bytes[5], (i >> 8) as u8);
            if i == 120 {
                assert!(report.is_last);
                assert_eq!(report.bytes[3], 1);
                assert_eq!(report.payload_len, 960);
            } else {
                assert!(!report.is_last);
                assert_eq!(report.bytes[3], 0);
                assert_eq!(report.payload_len, 1016);
            }
            assert_eq!(report.bytes[6], (report.payload_len & 0xff) as u8);
            assert_eq!(report.bytes[7], (report.payload_len >> 8) as u8);
        }
    }

    #[test]
    fn rejects_wrong_frame_size() {
        let err = image_reports(&[0u8; 4]).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::InvalidFrameSize {
                expected: FRAME_BYTES,
                actual: 4
            }
        ));
    }

    #[test]
    fn parses_touch_reports_and_gestures() {
        let mut parser = TouchParser::default();
        let mut recognizer = GestureRecognizer::default();
        let down = [0x01, 0x02, 0x21, 0, 0, 1, 100, 0];
        let move_ = [0x01, 0x02, 0x21, 0, 0, 1, 190, 0];
        let up = [0x01, 0x02, 0x21, 0, 0, 0, 190, 0];
        let down_event = parser.parse_report(&down).unwrap();
        assert_eq!(down_event.phase, TouchPhase::Began);
        assert!(recognizer.feed(down_event).is_none());
        assert!(recognizer
            .feed(parser.parse_report(&move_).unwrap())
            .is_none());
        let gesture = recognizer.feed(parser.parse_report(&up).unwrap()).unwrap();
        assert_eq!(gesture, Gesture::SwipeRight { x: 100 });
    }

    #[test]
    fn quick_stationary_press_is_tap_even_with_wobble() {
        let mut parser = TouchParser::default();
        let mut recognizer = GestureRecognizer::default();
        let start = Instant::now();
        let down = [0x01, 0x02, 0x21, 0, 0, 1, 142, 0];
        let move_ = [0x01, 0x02, 0x21, 0, 0, 1, 142, 0];
        let up = [0x01, 0x02, 0x21, 0, 0, 0, 142, 0];

        assert!(recognizer
            .feed_at(parser.parse_report(&down).unwrap(), start)
            .is_none());
        for _ in 0..3 {
            assert!(recognizer
                .feed_at(parser.parse_report(&move_).unwrap(), start)
                .is_none());
        }

        // A single tap is held back so it can pair with a follow-up; the touch
        // loop's `poll` is what flushes it once the double-tap window passes.
        let up_at = start + Duration::from_millis(50);
        assert!(recognizer
            .feed_at(parser.parse_report(&up).unwrap(), up_at)
            .is_none());
        let flush_at = up_at + Duration::from_millis(400);
        assert_eq!(recognizer.poll(flush_at), Some(Gesture::Tap { x: 142 }));
    }

    #[test]
    fn long_press_is_hold_despite_small_drift() {
        let mut parser = TouchParser::default();
        let mut recognizer = GestureRecognizer::default();
        let start = Instant::now();
        let down = [0x01, 0x02, 0x21, 0, 0, 1, 142, 0];
        let up = [0x01, 0x02, 0x21, 0, 0, 0, 150, 0];

        assert!(recognizer
            .feed_at(parser.parse_report(&down).unwrap(), start)
            .is_none());
        let later = start + Duration::from_millis(1_100);
        let gesture = recognizer
            .feed_at(parser.parse_report(&up).unwrap(), later)
            .unwrap();
        assert_eq!(gesture, Gesture::Hold { x: 142 });
    }

    #[test]
    fn two_taps_inside_window_collapse_into_double_tap() {
        let mut parser = TouchParser::default();
        let mut recognizer = GestureRecognizer::default();
        let start = Instant::now();
        let down = [0x01, 0x02, 0x21, 0, 0, 1, 100, 0];
        let up = [0x01, 0x02, 0x21, 0, 0, 0, 100, 0];

        // First tap is deferred.
        assert!(recognizer
            .feed_at(parser.parse_report(&down).unwrap(), start)
            .is_none());
        assert!(recognizer
            .feed_at(
                parser.parse_report(&up).unwrap(),
                start + Duration::from_millis(30)
            )
            .is_none());

        // Second tap inside the double-tap window collapses both into a
        // `DoubleTap` and clears the pending tap.
        let second_down_at = start + Duration::from_millis(150);
        let second_up_at = second_down_at + Duration::from_millis(40);
        assert!(recognizer
            .feed_at(parser.parse_report(&down).unwrap(), second_down_at)
            .is_none());
        assert_eq!(
            recognizer.feed_at(parser.parse_report(&up).unwrap(), second_up_at),
            Some(Gesture::DoubleTap { x: 100 })
        );
        assert_eq!(
            recognizer.poll(second_up_at + Duration::from_millis(500)),
            None
        );
    }

    #[test]
    fn ignores_bogus_touch_prefix_and_out_of_range_x() {
        let mut parser = TouchParser::default();
        assert!(parser
            .parse_report(&[0x02, 0x02, 0x21, 0, 0, 1, 1, 0])
            .is_none());
        let x640 = 640u16.to_le_bytes();
        assert!(parser
            .parse_report(&[0x01, 0x02, 0x21, 0, 0, 1, x640[0], x640[1]])
            .is_none());
        // The touch starts with the first position on the panel.
        assert_eq!(
            parser.parse_report(&[0x01, 0x02, 0x21, 0, 0, 1, 50, 0]),
            Some(TouchEvent {
                phase: TouchPhase::Began,
                x: 50
            })
        );
    }

    #[test]
    fn lift_off_the_panel_still_ends_the_touch() {
        let mut parser = TouchParser::default();
        parser.parse_report(&[0x01, 0x02, 0x21, 0, 0, 1, 100, 0]);
        parser.parse_report(&[0x01, 0x02, 0x21, 0, 0, 1, 120, 0]);
        let off = 0xffffu16.to_le_bytes();
        // A move off the panel is dropped, the lift ends at the last X.
        assert!(parser
            .parse_report(&[0x01, 0x02, 0x21, 0, 0, 1, off[0], off[1]])
            .is_none());
        assert_eq!(
            parser.parse_report(&[0x01, 0x02, 0x21, 0, 0, 0, off[0], off[1]]),
            Some(TouchEvent {
                phase: TouchPhase::Ended,
                x: 120
            })
        );
        assert!(parser
            .parse_report(&[0x01, 0x02, 0x21, 0, 0, 0, off[0], off[1]])
            .is_none());
    }
}

#[derive(Debug)]
pub struct DeviceLock {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceLockError {
    #[error("failed to create lock directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("device busy: lock exists at {path}. Stop linexusd or remove stale lock after verifying no process owns the device")]
    Busy { path: PathBuf },
    #[error("failed to create device lock {path}: {source}")]
    CreateLock {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write device lock {path}: {source}")]
    WriteLock {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl DeviceLock {
    pub fn acquire() -> Result<Self, DeviceLockError> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::acquire_in(base.join("linexus"))
    }

    pub fn acquire_in(dir: PathBuf) -> Result<Self, DeviceLockError> {
        fs::create_dir_all(&dir).map_err(|source| DeviceLockError::CreateDir {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join("device.lock");
        match create_lock_file(&path) {
            Ok(mut file) => {
                write_lock_pid(&path, &mut file)?;
                Ok(Self { path })
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_owner_is_stale(&path) {
                    let _ = fs::remove_file(&path);
                    match create_lock_file(&path) {
                        Ok(mut file) => {
                            write_lock_pid(&path, &mut file)?;
                            Ok(Self { path })
                        }
                        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                            Err(DeviceLockError::Busy { path })
                        }
                        Err(source) => Err(DeviceLockError::CreateLock { path, source }),
                    }
                } else {
                    Err(DeviceLockError::Busy { path })
                }
            }
            Err(source) => Err(DeviceLockError::CreateLock { path, source }),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

fn create_lock_file(path: &std::path::Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

fn write_lock_pid(path: &std::path::Path, file: &mut fs::File) -> Result<(), DeviceLockError> {
    use std::io::Write as _;
    writeln!(file, "pid={}", std::process::id()).map_err(|source| DeviceLockError::WriteLock {
        path: path.to_path_buf(),
        source,
    })
}

fn lock_owner_is_stale(path: &std::path::Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Some(pid_text) = content.trim().strip_prefix("pid=") else {
        return false;
    };
    let Ok(pid) = pid_text.parse::<u32>() else {
        return false;
    };
    !PathBuf::from(format!("/proc/{pid}")).exists()
}

impl Drop for DeviceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
