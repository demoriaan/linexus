//! Every hwmon sensor the kernel exposes, not just the CPU/GPU picks that
//! `discover_hwmon` makes, so users can put any fan or temperature on a page.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HwmonKind {
    Temp,
    Fan,
    Power,
    Voltage,
}

/// One sensor value in display units (°C, RPM, W, V).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HwmonReading {
    /// Chip name from `name` (`k10temp`, `amdgpu`, `nct6798`). When several
    /// chips share a name (two NVMe drives), the device they belong to tells
    /// them apart (`nvme0`, `nvme1`), which stays stable across reboots while
    /// hwmonN numbering does not.
    pub chip: String,
    /// The driver's `*_label` (`Tctl`, `edge`, `PPT`) or the input name
    /// (`temp1`, `fan2`).
    pub label: String,
    pub kind: HwmonKind,
    pub value: f32,
}

/// Reads all temperature, fan, power and voltage inputs below `root`
/// (normally /sys/class/hwmon), sorted by chip, kind and input number.
/// Unreadable files and values no working sensor reports (unconnected inputs
/// reading 0 °C or 127 °C, 65535 RPM) are skipped.
pub fn read_hwmon(root: &Path) -> Vec<HwmonReading> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut chips: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            let name = fs::read_to_string(path.join("name"))
                .map(|name| name.trim().to_string())
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "unknown".into());
            (name, path)
        })
        .collect();
    chips.sort();
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for (name, _) in &chips {
        *name_counts.entry(name.as_str()).or_default() += 1;
    }

    let mut readings = Vec::new();
    for (name, path) in &chips {
        let chip = if name_counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
            distinct_chip_name(name, path)
        } else {
            name.clone()
        };
        let mut chip_readings = read_chip(path, &chip);
        chip_readings.sort_by_key(|(index, reading)| (reading.kind, *index));
        readings.extend(chip_readings.into_iter().map(|(_, reading)| reading));
    }
    readings.sort_by(|a, b| a.chip.cmp(&b.chip));
    readings
}

/// `nvme0` for an NVMe drive's chip, `amdgpu-0000:03:00.0` for a second GPU,
/// the hwmonN directory name when there is no device link.
fn distinct_chip_name(name: &str, path: &Path) -> String {
    let device = fs::read_link(path.join("device"))
        .ok()
        .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()));
    match device {
        Some(device) if device.starts_with(name) => device,
        Some(device) => format!("{name}-{device}"),
        None => format!(
            "{name}-{}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
    }
}

fn read_chip(path: &Path, chip: &str) -> Vec<(u32, HwmonReading)> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut readings = Vec::new();
    let mut seen_power = std::collections::HashSet::new();
    let mut files: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    // `power1_average` sorts before `power1_input`, which makes the average
    // (what amdgpu reports) win when a chip offers both.
    files.sort();
    for file in files {
        let Some((prefix, kind, index)) = parse_input_name(&file) else {
            continue;
        };
        if kind == HwmonKind::Power && !seen_power.insert(index) {
            continue;
        }
        let Some(raw) = fs::read_to_string(path.join(&file))
            .ok()
            .and_then(|text| text.trim().parse::<f64>().ok())
        else {
            if kind == HwmonKind::Power {
                seen_power.remove(&index);
            }
            continue;
        };
        let Some(value) = scale_value(kind, raw) else {
            continue;
        };
        let label = fs::read_to_string(path.join(format!("{prefix}{index}_label")))
            .map(|label| label.trim().to_string())
            .ok()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| format!("{prefix}{index}"));
        readings.push((
            index,
            HwmonReading {
                chip: chip.to_string(),
                label,
                kind,
                value,
            },
        ));
    }
    readings
}

/// Splits `temp3_input` into ("temp", Temp, 3). Power also accepts
/// `power1_average`, which `read_chip` prefers over the instantaneous input.
fn parse_input_name(file: &str) -> Option<(&'static str, HwmonKind, u32)> {
    let (stem, suffix) = file.split_once('_')?;
    if !matches!(suffix, "input" | "average") {
        return None;
    }
    let (prefix, kind): (&'static str, HwmonKind) = [
        ("temp", HwmonKind::Temp),
        ("fan", HwmonKind::Fan),
        ("power", HwmonKind::Power),
        ("in", HwmonKind::Voltage),
    ]
    .into_iter()
    .find(|(prefix, _)| {
        stem.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    })?;
    if suffix == "average" && kind != HwmonKind::Power {
        return None;
    }
    let index = stem[prefix.len()..].parse().ok()?;
    Some((prefix, kind, index))
}

/// Converts sysfs units (m°C, RPM, µW, mV) and rejects values that only
/// unconnected or broken inputs report.
fn scale_value(kind: HwmonKind, raw: f64) -> Option<f32> {
    let value = match kind {
        HwmonKind::Temp => raw / 1000.0,
        HwmonKind::Fan => raw,
        HwmonKind::Power => raw / 1_000_000.0,
        HwmonKind::Voltage => raw / 1000.0,
    };
    let plausible = match kind {
        HwmonKind::Temp => value > 0.0 && value < 127.0,
        HwmonKind::Fan => (0.0..30_000.0).contains(&value),
        HwmonKind::Power => (0.0..5_000.0).contains(&value),
        HwmonKind::Voltage => (0.0..1_000.0).contains(&value),
    };
    (value.is_finite() && plausible).then_some(value as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(file), text).unwrap();
    }

    #[test]
    fn reads_and_scales_all_sensor_kinds() {
        let root = std::env::temp_dir().join(format!("nexus-hwmon-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cpu = root.join("hwmon2");
        write(&cpu, "name", "k10temp\n");
        write(&cpu, "temp1_input", "45250\n");
        write(&cpu, "temp1_label", "Tctl\n");
        write(&cpu, "temp3_input", "garbage\n");
        let gpu = root.join("hwmon5");
        write(&gpu, "name", "amdgpu\n");
        write(&gpu, "power1_average", "33000000\n");
        write(&gpu, "power1_input", "99000000\n");
        write(&gpu, "power1_label", "PPT\n");
        write(&gpu, "in0_input", "725\n");
        write(&gpu, "in0_label", "vddgfx\n");
        write(&gpu, "fan1_input", "1059\n");
        write(&gpu, "temp1_input", "0\n");
        write(&gpu, "temp2_input", "127000\n");
        write(&gpu, "temp1_crit", "100000\n");
        for (dir, device) in [("hwmon0", "nvme0"), ("hwmon1", "nvme1")] {
            let nvme = root.join(dir);
            write(&nvme, "name", "nvme\n");
            write(&nvme, "temp1_input", "38850\n");
            write(&nvme, "temp1_label", "Composite\n");
            std::os::unix::fs::symlink(format!("../../{device}"), nvme.join("device")).unwrap();
        }

        let readings = read_hwmon(&root);
        let _ = fs::remove_dir_all(&root);

        let summary: Vec<_> = readings
            .iter()
            .map(|r| (r.chip.as_str(), r.label.as_str(), r.kind, r.value))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("amdgpu", "fan1", HwmonKind::Fan, 1059.0),
                ("amdgpu", "PPT", HwmonKind::Power, 33.0),
                ("amdgpu", "vddgfx", HwmonKind::Voltage, 0.725),
                ("k10temp", "Tctl", HwmonKind::Temp, 45.25),
                ("nvme0", "Composite", HwmonKind::Temp, 38.85),
                ("nvme1", "Composite", HwmonKind::Temp, 38.85),
            ]
        );
    }

    #[test]
    fn parses_sysfs_input_names() {
        assert_eq!(
            parse_input_name("temp12_input"),
            Some(("temp", HwmonKind::Temp, 12))
        );
        assert_eq!(
            parse_input_name("in0_input"),
            Some(("in", HwmonKind::Voltage, 0))
        );
        assert_eq!(
            parse_input_name("power1_average"),
            Some(("power", HwmonKind::Power, 1))
        );
        assert_eq!(parse_input_name("temp1_average"), None);
        assert_eq!(parse_input_name("temp1_label"), None);
        assert_eq!(parse_input_name("intrusion0_alarm"), None);
        assert_eq!(parse_input_name("temp_input"), None);
    }

    #[test]
    fn missing_root_yields_nothing() {
        assert!(read_hwmon(Path::new("/nonexistent/hwmon")).is_empty());
    }
}
