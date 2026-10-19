//! Direct hardware commands. Each one takes the device lock so it never
//! interleaves reports with a running daemon.

use anyhow::{bail, Context, Result};
use hidapi::HidApi;
use nexus_core::{
    blank_report, brightness_report, image_reports, DeviceLock, HidapiDeviceSummary, HEIGHT,
    PRODUCT_ID, VENDOR_ID, WIDTH,
};
use std::path::Path;
use std::time::{Duration, Instant};

/// How `linexusctl image` maps a picture of any size onto the 640x48 strip.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum ImageFitArg {
    /// Scale to cover the strip and crop the overflow.
    Fill,
    /// Scale to fit inside the strip with black bars.
    Fit,
    /// Stretch to 640x48, ignoring the aspect ratio.
    Stretch,
}

pub(crate) fn info() -> Result<()> {
    let _lock = DeviceLock::acquire()?;
    let device = open_operational()?;
    let mut buf = [0u8; 64];
    buf[0] = nexus_core::INFO_REPORT_ID;
    let len = device.get_feature_report(&mut buf)?;
    println!("Firmware: {}", decode_firmware(&buf[..len]));
    Ok(())
}

pub(crate) fn set_brightness(percent: u8) -> Result<()> {
    let _lock = DeviceLock::acquire()?;
    let device = open_operational()?;
    device.send_feature_report(&brightness_report(percent))?;
    println!("Brightness set to {}%", percent.min(100));
    Ok(())
}

pub(crate) fn blank() -> Result<()> {
    let _lock = DeviceLock::acquire()?;
    let device = open_operational()?;
    device.send_feature_report(&blank_report())?;
    println!("Screen blanked");
    Ok(())
}

/// Sends one RGBA frame; the core library converts it to the panel's wire
/// format.
pub(crate) fn push_frame(rgba: &[u8]) -> Result<()> {
    let _lock = DeviceLock::acquire()?;
    let reports = image_reports(rgba)?;
    let device = open_operational()?;
    for report in reports {
        device.write(&report.bytes)?;
    }
    Ok(())
}

pub(crate) fn push_image(path: &Path, fit: ImageFitArg) -> Result<()> {
    let frame = load_image_frame(path, fit)?;
    push_frame(&frame)?;
    println!("Image pushed: {}", path.display());
    Ok(())
}

pub(crate) fn touch(seconds: u64) -> Result<()> {
    let _lock = DeviceLock::acquire()?;
    let device = open_operational()?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut parser = nexus_core::TouchParser::default();
    let mut recognizer = nexus_core::GestureRecognizer::default();
    let mut buf = [0u8; 64];
    println!("Listening for touch events for {seconds}s...");
    while Instant::now() < deadline {
        match device.read_timeout(&mut buf, 200) {
            Ok(0) => {}
            Ok(len) => {
                if let Some(event) = parser.parse_report(&buf[..len]) {
                    println!("event: {:?} @ {}", event.phase, event.x);
                    if let Some(gesture) = recognizer.feed(event) {
                        println!("gesture: {gesture}");
                    }
                }
            }
            Err(e) => bail!("touch read failed: {e}"),
        }
        if let Some(gesture) = recognizer.poll(Instant::now()) {
            println!("gesture: {gesture}");
        }
    }
    Ok(())
}

fn open_operational() -> Result<hidapi::HidDevice> {
    let (api, selected) = select_operational()?;
    open_summary(&api, &selected)
        .with_context(|| format!("failed to open Nexus HID path {}", selected.path))
}

/// The Nexus interface that takes images and reports touches (usage page
/// 0x000c), chosen deterministically when several match.
fn select_operational() -> Result<(HidApi, HidapiDeviceSummary)> {
    let api = HidApi::new()?;
    let mut candidates: Vec<_> = api
        .device_list()
        .filter(|d| {
            d.vendor_id() == VENDOR_ID
                && d.product_id() == PRODUCT_ID
                && d.usage_page() == nexus_core::OPERATIONAL_USAGE_PAGE
        })
        .map(|d| HidapiDeviceSummary {
            path: d.path().to_string_lossy().into_owned(),
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            interface_number: d.interface_number(),
            manufacturer: d.manufacturer_string().map(ToOwned::to_owned),
            product: d.product_string().map(ToOwned::to_owned),
            serial: d.serial_number().map(ToOwned::to_owned),
        })
        .collect();
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    let selected = candidates.into_iter().next().context(
        "No iCUE Nexus found (USB 1b1c:1b8e). Check the cable and run `linexusctl doctor`.",
    )?;
    Ok((api, selected))
}

fn open_summary(api: &HidApi, selected: &HidapiDeviceSummary) -> Result<hidapi::HidDevice> {
    let Some(info) = api
        .device_list()
        .find(|info| info.path().to_string_lossy() == selected.path)
    else {
        bail!(
            "selected HID path disappeared before open: {} (refusing VID/PID fallback)",
            selected.path
        );
    };
    Ok(info.open_device(api)?)
}

/// Opens and closes `device` to check access (used by `doctor`).
pub(crate) fn try_open(device: &HidapiDeviceSummary) -> Result<(), hidapi::HidError> {
    let api = HidApi::new()?;
    let Some(info) = api
        .device_list()
        .find(|info| info.path().to_string_lossy() == device.path)
    else {
        return Ok(());
    };
    info.open_device(&api).map(drop)
}

fn decode_firmware(bytes: &[u8]) -> String {
    if bytes.len() <= 6 {
        return "unknown".into();
    }
    let tail = &bytes[6..];
    let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).trim().to_string()
}

fn load_image_frame(path: &Path, fit: ImageFitArg) -> Result<Vec<u8>> {
    use image::imageops::FilterType;
    let img =
        image::open(path).with_context(|| format!("failed to decode image {}", path.display()))?;
    let (w, h) = (WIDTH as u32, HEIGHT as u32);
    let frame = match fit {
        ImageFitArg::Fill => img.resize_to_fill(w, h, FilterType::Lanczos3).to_rgba8(),
        ImageFitArg::Stretch => img.resize_exact(w, h, FilterType::Lanczos3).to_rgba8(),
        ImageFitArg::Fit => {
            let scaled = img.resize(w, h, FilterType::Lanczos3).to_rgba8();
            let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
            let x = (w - scaled.width()) / 2;
            let y = (h - scaled.height()) / 2;
            image::imageops::overlay(&mut canvas, &scaled, i64::from(x), i64::from(y));
            canvas
        }
    };
    Ok(frame.into_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_firmware_string_after_header() {
        let mut report = vec![0x05, 0, 0, 0, 0, 0];
        report.extend_from_slice(b"2.4.438\0\0");
        assert_eq!(decode_firmware(&report), "2.4.438");
        assert_eq!(decode_firmware(&[1, 2, 3]), "unknown");
    }
}
