//! A flat catalog of readings with stable ids, so widgets can bind to
//! "cpu.temp" or "hwmon:nct6798/fan2" without knowing where a value comes
//! from or how `SystemMetrics` stores it.

use super::hwmon::{HwmonKind, HwmonReading};
use crate::SystemMetrics;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricUnit {
    Percent,
    Celsius,
    Watts,
    Rpm,
    Bytes,
    BytesPerSec,
    Megahertz,
    /// No unit; also used for hwmon voltages (volts).
    Plain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricReading {
    /// Stable binding key, e.g. `cpu.usage` or `hwmon:k10temp/tctl`.
    pub id: String,
    /// Short human name for a gauge caption.
    pub label: String,
    pub value: f32,
    pub unit: MetricUnit,
    /// Full scale for bars and gauges.
    pub max: f32,
}

const GIB: f32 = 1_073_741_824.0;
const TEMP_FULL_SCALE: f32 = 100.0;
const CLOCK_FULL_SCALE_MHZ: f32 = 6_000.0;
const POWER_FULL_SCALE_W: f32 = 400.0;
const FAN_FULL_SCALE_RPM: f32 = 3_000.0;
/// Gigabit Ethernet, in bytes per second.
const NET_FULL_SCALE_BPS: f32 = 125_000_000.0;
/// Roughly what a SATA SSD sustains; NVMe bursts simply peg the bar.
const DISK_FULL_SCALE_BPS: f32 = 500_000_000.0;
/// Board rails top out at 12 V.
const VOLTAGE_FULL_SCALE: f32 = 12.0;

/// Maps the collected metrics to catalog entries. Values missing on this
/// machine (no GPU, no PipeWire) are left out rather than reported as zero.
/// The hwmon sensors follow, one per input, with ids
/// `hwmon:<chip>/<label>` (lowercase, punctuation replaced by `_`).
pub fn metric_catalog(m: &SystemMetrics, hwmon: &[HwmonReading]) -> Vec<MetricReading> {
    let mut out = Vec::new();
    let mut push = |id: &str, label: &str, value: Option<f32>, unit: MetricUnit, max: f32| {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            out.push(MetricReading {
                id: id.into(),
                label: label.into(),
                value,
                unit,
                max,
            });
        }
    };
    // SystemMetrics keeps usage as 0..1 fractions; the catalog speaks percent.
    let percent = |fraction: Option<f32>| fraction.map(|f| f * 100.0);
    use MetricUnit::*;

    push("cpu.usage", "CPU", percent(m.cpu_usage), Percent, 100.0);
    push(
        "cpu.temp",
        "CPU temp",
        m.cpu_temp_c,
        Celsius,
        TEMP_FULL_SCALE,
    );
    push(
        "cpu.clock",
        "CPU clock",
        m.cpu_clock_mhz,
        Megahertz,
        CLOCK_FULL_SCALE_MHZ,
    );
    push("gpu.usage", "GPU", percent(m.gpu_usage), Percent, 100.0);
    push(
        "gpu.temp",
        "GPU temp",
        m.gpu_temp_c,
        Celsius,
        TEMP_FULL_SCALE,
    );
    push(
        "gpu.mem_temp",
        "VRAM temp",
        m.gpu_mem_temp_c,
        Celsius,
        TEMP_FULL_SCALE,
    );
    push(
        "gpu.power",
        "GPU power",
        m.gpu_power_w,
        Watts,
        POWER_FULL_SCALE_W,
    );
    push(
        "gpu.fan",
        "GPU fan",
        m.gpu_fan_rpm.map(|rpm| rpm as f32),
        Rpm,
        FAN_FULL_SCALE_RPM,
    );
    push("gpu.vram", "VRAM", percent(m.vram_usage), Percent, 100.0);
    push("ram.usage", "RAM", percent(m.ram_usage), Percent, 100.0);
    let ram_total = m.ram_total_gib.map(|gib| gib * GIB);
    let ram_used = m.ram_used_gib.map(|gib| gib * GIB);
    push(
        "ram.used",
        "RAM used",
        ram_used,
        Bytes,
        ram_total.or(ram_used).unwrap_or(0.0),
    );
    push(
        "ram.total",
        "RAM total",
        ram_total,
        Bytes,
        ram_total.unwrap_or(0.0),
    );
    push(
        "disk.usage",
        "Disk",
        percent(m.disk_root_usage),
        Percent,
        100.0,
    );
    push(
        "disk.read",
        "Disk read",
        m.disk_read_bps,
        BytesPerSec,
        DISK_FULL_SCALE_BPS,
    );
    push(
        "disk.write",
        "Disk write",
        m.disk_write_bps,
        BytesPerSec,
        DISK_FULL_SCALE_BPS,
    );
    // network_*_bps are bytes per second (byte counter deltas from
    // /proc/net/dev), not bits.
    push(
        "net.down",
        "Down",
        m.network_rx_bps,
        BytesPerSec,
        NET_FULL_SCALE_BPS,
    );
    push(
        "net.up",
        "Up",
        m.network_tx_bps,
        BytesPerSec,
        NET_FULL_SCALE_BPS,
    );
    push(
        "volume.level",
        "Volume",
        m.volume_percent.map(f32::from),
        Percent,
        100.0,
    );

    let mut ids: HashSet<String> = out.iter().map(|reading| reading.id.clone()).collect();
    for reading in hwmon {
        let base = format!(
            "hwmon:{}/{}",
            sanitize_id_part(&reading.chip),
            sanitize_id_part(&reading.label)
        );
        let mut id = base.clone();
        let mut n = 2;
        while !ids.insert(id.clone()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let (unit, max) = match reading.kind {
            HwmonKind::Temp => (Celsius, TEMP_FULL_SCALE),
            HwmonKind::Fan => (Rpm, FAN_FULL_SCALE_RPM),
            HwmonKind::Power => (Watts, POWER_FULL_SCALE_W),
            HwmonKind::Voltage => (Plain, VOLTAGE_FULL_SCALE),
        };
        out.push(MetricReading {
            id,
            label: format!("{} {}", reading.chip, reading.label),
            value: reading.value,
            unit,
            max,
        });
    }
    out
}

/// Lowercase ASCII letters, digits, `-` and `.`; everything else becomes `_`
/// (runs collapsed), so ids survive URLs, JSON keys and config files.
fn sanitize_id_part(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.') {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };
        if mapped == '_' && out.ends_with('_') {
            continue;
        }
        out.push(mapped);
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".into()
    } else {
        trimmed.to_string()
    }
}

/// Average current clock over all cores in MHz, from
/// `<root>/cpu*/cpufreq/scaling_cur_freq` (kHz); root is normally
/// /sys/devices/system/cpu. The average rather than the maximum: a single
/// core waking up for a moment would otherwise make the gauge jump on every
/// sample.
pub fn read_cpu_clock_mhz(root: &Path) -> Option<f32> {
    let entries = fs::read_dir(root).ok()?;
    let mut sum = 0.0_f64;
    let mut count = 0_u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(index) = name.to_str().and_then(|name| name.strip_prefix("cpu")) else {
            continue;
        };
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Some(khz) = fs::read_to_string(entry.path().join("cpufreq/scaling_cur_freq"))
            .ok()
            .and_then(|text| text.trim().parse::<f64>().ok())
        else {
            continue;
        };
        if khz > 0.0 {
            sum += khz;
            count += 1;
        }
    }
    (count > 0).then(|| (sum / f64::from(count) / 1000.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(catalog: &'a [MetricReading], id: &str) -> &'a MetricReading {
        catalog
            .iter()
            .find(|reading| reading.id == id)
            .unwrap_or_else(|| panic!("{id} missing"))
    }

    #[test]
    fn maps_system_metrics_to_stable_ids() {
        let metrics = SystemMetrics {
            cpu_usage: Some(0.25),
            cpu_temp_c: Some(55.0),
            cpu_clock_mhz: Some(3_600.0),
            ram_usage: Some(0.5),
            ram_used_gib: Some(8.0),
            ram_total_gib: Some(16.0),
            vram_usage: Some(0.125),
            gpu_fan_rpm: Some(1200),
            network_rx_bps: Some(1_000.0),
            disk_root_usage: Some(0.42),
            volume_percent: Some(60),
            volume_muted: Some(true),
            ..SystemMetrics::default()
        };
        let catalog = metric_catalog(&metrics, &[]);

        let cpu = find(&catalog, "cpu.usage");
        assert_eq!(
            (cpu.label.as_str(), cpu.value, cpu.unit),
            ("CPU", 25.0, MetricUnit::Percent)
        );
        assert_eq!(cpu.max, 100.0);
        assert_eq!(find(&catalog, "cpu.clock").unit, MetricUnit::Megahertz);
        assert_eq!(find(&catalog, "cpu.clock").max, 6_000.0);
        assert_eq!(find(&catalog, "gpu.vram").value, 12.5);
        assert_eq!(find(&catalog, "gpu.fan").value, 1200.0);
        assert_eq!(find(&catalog, "disk.usage").value, 42.0);
        let used = find(&catalog, "ram.used");
        assert_eq!(used.value, 8.0 * GIB);
        assert_eq!(used.max, 16.0 * GIB);
        assert_eq!(find(&catalog, "net.down").unit, MetricUnit::BytesPerSec);
        assert_eq!(find(&catalog, "net.down").max, 125_000_000.0);
        // Volume level is the level, muted or not.
        assert_eq!(find(&catalog, "volume.level").value, 60.0);

        for absent in ["gpu.usage", "gpu.temp", "gpu.power", "net.up", "disk.read"] {
            assert!(
                catalog.iter().all(|reading| reading.id != absent),
                "{absent}"
            );
        }
    }

    #[test]
    fn appends_hwmon_sensors_with_sanitized_unique_ids() {
        let hwmon = [
            HwmonReading {
                chip: "nct6798".into(),
                label: "PECI Agent 0 Calibration".into(),
                kind: HwmonKind::Temp,
                value: 63.0,
            },
            HwmonReading {
                chip: "nct6798".into(),
                label: "fan2".into(),
                kind: HwmonKind::Fan,
                value: 942.0,
            },
            HwmonReading {
                chip: "amdgpu".into(),
                label: "vddgfx".into(),
                kind: HwmonKind::Voltage,
                value: 0.725,
            },
            HwmonReading {
                chip: "xe".into(),
                label: "vram".into(),
                kind: HwmonKind::Temp,
                value: 50.0,
            },
            HwmonReading {
                chip: "xe".into(),
                label: "VRAM".into(),
                kind: HwmonKind::Temp,
                value: 51.0,
            },
        ];
        let catalog = metric_catalog(&SystemMetrics::default(), &hwmon);
        let ids: Vec<_> = catalog.iter().map(|reading| reading.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "hwmon:nct6798/peci_agent_0_calibration",
                "hwmon:nct6798/fan2",
                "hwmon:amdgpu/vddgfx",
                "hwmon:xe/vram",
                "hwmon:xe/vram-2",
            ]
        );
        assert_eq!(catalog[0].label, "nct6798 PECI Agent 0 Calibration");
        assert_eq!(catalog[1].unit, MetricUnit::Rpm);
        assert_eq!(catalog[2].unit, MetricUnit::Plain);
    }

    #[test]
    fn averages_cpu_clock_from_cpufreq() {
        let root = std::env::temp_dir().join(format!("nexus-cpufreq-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (cpu, khz) in [
            ("cpu0", "3000000\n"),
            ("cpu1", "4000000\n"),
            ("cpu2", "0\n"),
        ] {
            let dir = root.join(cpu).join("cpufreq");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("scaling_cur_freq"), khz).unwrap();
        }
        fs::create_dir_all(root.join("cpufreq")).unwrap();
        fs::create_dir_all(root.join("cpuidle")).unwrap();
        let clock = read_cpu_clock_mhz(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(clock, Some(3_500.0));
        assert_eq!(read_cpu_clock_mhz(Path::new("/nonexistent/cpu")), None);
    }
}
