//! Drives the real panel (hidraw) or the PNG emulator with the engine.

use crate::engine::Engine;
use crate::ipc::ControlRequest;
use crate::log::{log, log_fields, Level};
use crate::session::SessionMonitor;
use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};
use nexus_core::{
    blank_report, brightness_report, image_reports, play_animation_report, TouchParser,
    OPERATIONAL_USAGE_PAGE, PRODUCT_ID, VENDOR_ID,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub(crate) fn install_signal_handlers() {
    extern "C" fn on_signal(_: libc::c_int) {
        SHUTDOWN.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
    }
}

fn shutdown() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Unchanged frames are re-sent at most this often.
const RESEND_UNCHANGED: Duration = Duration::from_secs(2);
/// Touch reports handled per loop pass, so a finger resting on the strip
/// cannot starve rendering.
const MAX_REPORTS: usize = 64;
/// Repeat an identical "device unavailable" log line at most this often.
const UNAVAILABLE_LOG_EVERY: Duration = Duration::from_secs(600);

pub(crate) struct LoopConfig<'a> {
    pub(crate) requests: &'a Receiver<ControlRequest>,
    pub(crate) logger: &'a nexus_events::EventLogger,
    pub(crate) session_blank: bool,
    pub(crate) touch_poll: Duration,
}

pub(crate) fn run_hardware(engine: &mut Engine, cfg: &LoopConfig) -> Result<()> {
    let mut session = SessionMonitor::new(cfg.session_blank);
    let mut last_error: Option<(String, Instant)> = None;
    while !shutdown() {
        match open_device() {
            Ok((device, path)) => {
                last_error = None;
                engine.device = nexus_ipc::DeviceStatus::connected(path.clone());
                log_fields(
                    cfg.logger,
                    Level::Info,
                    "hardware_connected",
                    "Nexus connected",
                    [("path", path)],
                );
                match drive(engine, &device, cfg, &mut session) {
                    Ok(()) => {
                        // Hand the panel back to its firmware animation so it
                        // does not freeze on our last frame.
                        let _ = device.send_feature_report(&play_animation_report(1, true));
                        let _ = device.send_feature_report(&brightness_report(100));
                        return Ok(());
                    }
                    Err(error) => {
                        // A finger down when the panel went away never
                        // reports its release; holds would keep firing.
                        engine.cancel_touch(Instant::now());
                        log_fields(
                            cfg.logger,
                            Level::Warn,
                            "hardware_lost",
                            "Nexus connection lost",
                            [("error", format!("{error:#}"))],
                        );
                    }
                }
            }
            Err(error) => {
                let text = format!("{error:#}");
                engine.device = nexus_ipc::DeviceStatus::unavailable(text.clone());
                let repeat = last_error.as_ref().is_some_and(|(prev, at)| {
                    *prev == text && at.elapsed() < UNAVAILABLE_LOG_EVERY
                });
                if !repeat {
                    eprintln!("Nexus unavailable, retrying: {text}");
                    log_fields(
                        cfg.logger,
                        Level::Warn,
                        "hardware_unavailable",
                        "Nexus unavailable",
                        [("error", text.clone())],
                    );
                    last_error = Some((text, Instant::now()));
                }
            }
        }
        // Keep answering the API while waiting for the device.
        let retry_at = Instant::now() + Duration::from_secs(2);
        while !shutdown() && Instant::now() < retry_at {
            let now = Instant::now();
            if let Some(blank) = session.poll() {
                engine.runtime.set_session_blank(blank, now);
            }
            engine.drain_requests(cfg.requests, now);
            engine.tick(now);
            thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(())
}

fn drive(
    engine: &mut Engine,
    device: &HidDevice,
    cfg: &LoopConfig,
    session: &mut SessionMonitor,
) -> Result<()> {
    let mut parser = TouchParser::default();
    let mut buf = [0u8; 64];
    let mut next_frame = Instant::now();
    let mut sent_brightness: Option<u8> = None;
    let mut sent_blank: Option<bool> = None;
    let mut last_push: Option<(Vec<u8>, Instant)> = None;
    while !shutdown() {
        let now = Instant::now();
        if let Some(blank) = session.poll() {
            if blank != engine.runtime.session_blank {
                engine.runtime.set_session_blank(blank, now);
                log(
                    cfg.logger,
                    Level::Info,
                    if blank {
                        "session_blank"
                    } else {
                        "session_restore"
                    },
                    "session lock state changed",
                );
                next_frame = now;
            }
        }
        if engine.drain_requests(cfg.requests, now) {
            next_frame = now;
        }
        engine.tick(now);

        if now >= next_frame {
            let blanked = engine.runtime.blanked();
            let brightness = engine.runtime.effective_brightness(now);
            if sent_brightness != Some(brightness) && !blanked {
                device.send_feature_report(&brightness_report(brightness))?;
                sent_brightness = Some(brightness);
            }
            if sent_blank != Some(blanked) {
                sent_blank = Some(blanked);
                if blanked {
                    device.send_feature_report(&blank_report())?;
                    last_push = None;
                }
            }
            if !blanked {
                let frame = engine.frame(now).into_rgba();
                let unchanged = last_push
                    .as_ref()
                    .is_some_and(|(prev, at)| *prev == frame && at.elapsed() < RESEND_UNCHANGED);
                if !unchanged {
                    for report in image_reports(&frame)? {
                        device.write(&report.bytes)?;
                    }
                    last_push = Some((frame, Instant::now()));
                }
            }
            // Pace from the start of the frame so HID writes do not stretch
            // every interval.
            next_frame = now + engine.next_frame_in(now);
        }

        let wait = next_frame.saturating_duration_since(Instant::now());
        let mut timeout = cfg
            .touch_poll
            .min(wait)
            .max(Duration::from_millis(1))
            .as_millis() as i32;
        for _ in 0..MAX_REPORTS {
            let len = match device.read_timeout(&mut buf, timeout) {
                Ok(0) => break,
                Ok(len) => len,
                // SIGTERM interrupts the blocking read: a clean stop, not a
                // lost device.
                Err(_) if shutdown() => return Ok(()),
                Err(error) => bail!("touch read failed: {error}"),
            };
            timeout = 0;
            if let Some(event) = parser.parse_report(&buf[..len]) {
                let phase = match event.phase {
                    nexus_core::TouchPhase::Began => nexus_ipc::TouchPhase::Began,
                    nexus_core::TouchPhase::Moved => nexus_ipc::TouchPhase::Moved,
                    nexus_core::TouchPhase::Ended => nexus_ipc::TouchPhase::Ended,
                };
                engine.touch_event(phase, event.x, Instant::now());
                // Answer the finger within one frame.
                next_frame = next_frame.min(Instant::now() + Duration::from_millis(8));
            }
        }
    }
    Ok(())
}

pub(crate) fn run_emulator(engine: &mut Engine, path: &Path, cfg: &LoopConfig) -> Result<()> {
    let mut session = SessionMonitor::new(cfg.session_blank);
    let mut next_frame = Instant::now();
    while !shutdown() {
        let now = Instant::now();
        if let Some(blank) = session.poll() {
            engine.runtime.set_session_blank(blank, now);
        }
        if engine.drain_requests(cfg.requests, now) {
            next_frame = now;
        }
        engine.tick(now);
        if now >= next_frame {
            write_png(engine, path, now)?;
            // PNG encoding is the emulator's cost; stay at <= 10 fps.
            next_frame = now + engine.next_frame_in(now).max(Duration::from_millis(100));
        }
        thread::sleep(
            next_frame
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(5), Duration::from_millis(50)),
        );
    }
    Ok(())
}

pub(crate) fn write_png(engine: &mut Engine, path: &Path, now: Instant) -> Result<()> {
    let frame = if engine.runtime.blanked() {
        vec![0; nexus_render::WIDTH * nexus_render::HEIGHT * 4]
    } else {
        engine.frame(now).into_rgba()
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let png = nexus_render::encode_png(&frame).context("PNG encoding failed")?;
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, png)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn push_once(engine: &mut Engine) -> Result<()> {
    let (device, _) = open_device()?;
    let now = Instant::now();
    device.send_feature_report(&brightness_report(engine.runtime.effective_brightness(now)))?;
    for report in image_reports(&engine.frame(now).into_rgba())? {
        device.write(&report.bytes)?;
    }
    Ok(())
}

fn open_device() -> Result<(HidDevice, String)> {
    let api = HidApi::new()?;
    let mut candidates: Vec<_> = api
        .device_list()
        .filter(|d| {
            d.vendor_id() == VENDOR_ID
                && d.product_id() == PRODUCT_ID
                && d.usage_page() == OPERATIONAL_USAGE_PAGE
        })
        .collect();
    candidates.sort_by_key(|d| d.path().to_owned());
    let info = candidates
        .first()
        .context("no iCUE Nexus found (USB 1b1c:1b8e, usage page 0x0c); run `linexusctl doctor`")?;
    let path = info.path().to_string_lossy().into_owned();
    let device = info.open_device(&api).with_context(|| {
        format!("cannot open {path}; is the udev rule installed? (`linexusctl doctor`)")
    })?;
    Ok((device, path))
}
