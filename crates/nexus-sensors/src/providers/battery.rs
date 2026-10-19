//! Battery levels of the laptop and of wireless peripherals (mice,
//! keyboards, headsets, game controllers), from UPower when it is installed
//! (it also knows Bluetooth devices) and from /sys/class/power_supply
//! otherwise.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::metrics::{MetricReading, MetricUnit};

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryReading {
    /// Stable key of the device, e.g. `hidpp_battery_0`.
    pub key: String,
    /// What it powers: `battery` (the computer itself), `mouse`, `keyboard`,
    /// `headset`, `gaming-input`, …
    pub kind: String,
    /// Model name, or empty when the device does not report one.
    pub model: String,
    pub percent: f32,
    pub charging: bool,
}

impl BatteryReading {
    /// Catalog entry with id `battery:<kind>/<key>`.
    pub fn metric(&self) -> MetricReading {
        let label = if !self.model.is_empty() {
            self.model.clone()
        } else if self.kind == "battery" {
            "Battery".into()
        } else {
            let mut kind = self.kind.replace('-', " ");
            if let Some(first) = kind.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            kind
        };
        MetricReading {
            id: format!("battery:{}/{}", self.kind, self.key),
            label,
            value: self.percent,
            unit: MetricUnit::Percent,
            max: 100.0,
        }
    }
}

/// All present batteries, UPower first.
pub fn read_batteries() -> Vec<BatteryReading> {
    let dump = Command::new("upower")
        .arg("--dump")
        .stderr(std::process::Stdio::null())
        .output();
    match dump {
        Ok(output) if output.status.success() => {
            parse_upower_dump(&String::from_utf8_lossy(&output.stdout))
        }
        _ => read_power_supply(Path::new("/sys/class/power_supply")),
    }
}

/// Parses `upower --dump`: one block per device, starting with `Device:`,
/// with the device type on its own line before the battery properties.
pub fn parse_upower_dump(text: &str) -> Vec<BatteryReading> {
    let mut out = Vec::new();
    for block in text.split("Device: ").skip(1) {
        let mut lines = block.lines();
        let object = lines.next().unwrap_or_default().trim();
        if object.ends_with("/DisplayDevice") {
            continue;
        }
        let mut reading = BatteryReading {
            key: object.rsplit('/').next().unwrap_or(object).to_string(),
            kind: String::new(),
            model: String::new(),
            percent: f32::NAN,
            charging: false,
        };
        let mut present = true;
        for line in lines {
            let trimmed = line.trim();
            let Some((name, value)) = trimmed.split_once(':') else {
                // The device type is the only bare word in the block.
                if !trimmed.is_empty() && !trimmed.contains(' ') && reading.kind.is_empty() {
                    reading.kind = trimmed.to_string();
                }
                continue;
            };
            let value = value.trim();
            match name {
                "native-path" if !value.is_empty() && value != "(null)" => {
                    reading.key = value.rsplit('/').next().unwrap_or(value).to_string();
                }
                "model" => reading.model = value.to_string(),
                "present" => present = value == "yes",
                "state" => reading.charging = matches!(value, "charging" | "fully-charged"),
                "percentage" => {
                    let number = value.split('%').next().unwrap_or_default().trim();
                    reading.percent = number.replace(',', ".").parse().unwrap_or(f32::NAN);
                }
                _ => {}
            }
        }
        if present
            && reading.percent.is_finite()
            && !matches!(reading.kind.as_str(), "" | "line-power")
        {
            out.push(reading);
        }
    }
    out
}

/// Fallback without UPower: batteries the kernel exposes directly (laptop
/// batteries, Logitech HID++ and game controller drivers, …).
pub fn read_power_supply(root: &Path) -> Vec<BatteryReading> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let read = |dir: &Path, name: &str| {
        fs::read_to_string(dir.join(name))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut out: Vec<BatteryReading> = entries
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            if read(&dir, "type") != "Battery" || read(&dir, "present") == "0" {
                return None;
            }
            let percent: f32 = read(&dir, "capacity").parse().ok()?;
            let key = entry.file_name().to_string_lossy().into_owned();
            // Peripherals report scope "Device"; the computer's own battery
            // "System" or nothing.
            let kind = if read(&dir, "scope") == "Device" {
                "device"
            } else {
                "battery"
            };
            Some(BatteryReading {
                key,
                kind: kind.into(),
                model: read(&dir, "model_name"),
                percent,
                charging: matches!(read(&dir, "status").as_str(), "Charging" | "Full"),
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "\
Device: /org/freedesktop/UPower/devices/line_power_AC
  native-path:          AC
  power supply:         yes
  line-power
    online:              yes

Device: /org/freedesktop/UPower/devices/battery_hidpp_battery_0
  native-path:          hidpp_battery_0
  model:                G502 LIGHTSPEED Wireless Gaming Mouse
  power supply:         no
  mouse
    present:             yes
    rechargeable:        yes
    state:               discharging
    percentage:          55% (should be ignored)

Device: /org/freedesktop/UPower/devices/headset_dev_00_11_22
  native-path:          /org/bluez/hci0/dev_00_11_22
  model:                WH-1000XM4
  headset
    present:             yes
    state:               charging
    percentage:          80%

Device: /org/freedesktop/UPower/devices/DisplayDevice
  power supply:         yes
  updated:              Mon Oct 19 10:00:00 2026
  battery
    present:             yes
    percentage:          55%

Daemon:
  daemon-version:  1.90.4
  on-battery:      no
";

    #[test]
    fn parses_peripherals_and_skips_line_power_and_the_display_device() {
        let batteries = parse_upower_dump(DUMP);
        assert_eq!(batteries.len(), 2);
        assert_eq!(batteries[0].key, "hidpp_battery_0");
        assert_eq!(batteries[0].kind, "mouse");
        assert_eq!(batteries[0].percent, 55.0);
        assert!(!batteries[0].charging);
        assert_eq!(batteries[1].key, "dev_00_11_22");
        assert_eq!(batteries[1].kind, "headset");
        assert!(batteries[1].charging);
        let metric = batteries[1].metric();
        assert_eq!(metric.id, "battery:headset/dev_00_11_22");
        assert_eq!(metric.label, "WH-1000XM4");
    }

    #[test]
    fn reads_kernel_power_supplies() {
        let root = std::env::temp_dir().join(format!("nexus-battery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mouse = root.join("hidpp_battery_0");
        let ac = root.join("AC");
        fs::create_dir_all(&mouse).unwrap();
        fs::create_dir_all(&ac).unwrap();
        for (name, value) in [
            ("type", "Battery"),
            ("scope", "Device"),
            ("capacity", "42"),
            ("status", "Discharging"),
            ("model_name", "MX Master 3"),
        ] {
            fs::write(mouse.join(name), format!("{value}\n")).unwrap();
        }
        fs::write(ac.join("type"), "Mains\n").unwrap();
        let batteries = read_power_supply(&root);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(batteries.len(), 1);
        assert_eq!(batteries[0].kind, "device");
        assert_eq!(batteries[0].percent, 42.0);
        assert_eq!(batteries[0].metric().label, "MX Master 3");
    }
}
