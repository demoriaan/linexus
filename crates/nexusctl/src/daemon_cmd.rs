//! `linexusctl daemon`: talk to the running linexusd over its Unix socket.

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use clap::Subcommand;
use nexus_ipc::{Command, DaemonStatus, MetricReading, Response};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub(crate) enum DaemonCommand {
    /// Print the default daemon socket path.
    SocketPath,
    /// Check whether the daemon answers.
    Ping,
    /// Print mode, device, active profile and page.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Simulate a tap at X (0..=639).
    Tap {
        x: u16,
        #[arg(long)]
        json: bool,
    },
    /// Switch to the next page.
    PageNext {
        #[arg(long)]
        json: bool,
    },
    /// Switch to the previous page.
    PagePrevious {
        #[arg(long)]
        json: bool,
    },
    /// Open a page of the active profile by id (hidden pages too).
    GoToPage {
        page_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Switch to another configured profile.
    SwitchProfile {
        profile_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Set the strip brightness (0..=100).
    Brightness {
        percent: u8,
        #[arg(long)]
        json: bool,
    },
    /// Turn the strip off.
    Blank {
        #[arg(long)]
        json: bool,
    },
    /// Turn the strip back on.
    Restore {
        #[arg(long)]
        json: bool,
    },
    /// Reload the config from disk.
    ReloadConfig {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Save the frame currently on the strip as PNG.
    Frame {
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// Enlarge by this integer factor (nearest neighbour).
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=16))]
        scale: u32,
    },
    /// Show a short message on the strip.
    Push {
        /// Message text; several words need no quotes.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// Lucide icon name, e.g. `bell` or `check`.
        #[arg(long)]
        icon: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the current reading of every metric.
    Metrics {
        /// Only show metrics whose id or label contains this text.
        filter: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(command: DaemonCommand, socket_path: Option<PathBuf>) -> Result<()> {
    let path = socket_path.unwrap_or_else(nexus_ipc::default_socket_path);
    let simple = |command: Command, json: bool| -> Result<()> {
        print_response(send(&path, command)?, json)
    };
    match command {
        DaemonCommand::SocketPath => {
            println!("{}", path.display());
            Ok(())
        }
        DaemonCommand::Ping => {
            let response = send(&path, Command::Ping)?;
            if !response.ok {
                bail!("daemon ping failed: {}", response.message);
            }
            println!("{}", response.message);
            Ok(())
        }
        DaemonCommand::Status { json } => simple(Command::Status, json),
        DaemonCommand::Tap { x, json } => simple(Command::Tap { x }, json),
        DaemonCommand::PageNext { json } => simple(Command::PageNext, json),
        DaemonCommand::PagePrevious { json } => simple(Command::PagePrevious, json),
        DaemonCommand::GoToPage { page_id, json } => simple(Command::GoToPage { page_id }, json),
        DaemonCommand::SwitchProfile { profile_id, json } => {
            simple(Command::SwitchProfile { profile_id }, json)
        }
        DaemonCommand::Brightness { percent, json } => simple(
            Command::SetBrightness {
                percent: percent.min(100),
            },
            json,
        ),
        DaemonCommand::Blank { json } => simple(Command::Blank, json),
        DaemonCommand::Restore { json } => simple(Command::Restore, json),
        DaemonCommand::ReloadConfig { config, json } => simple(
            Command::ReloadConfig {
                path: config.map(|path| path.display().to_string()),
            },
            json,
        ),
        DaemonCommand::Frame { out, scale } => save_frame(&path, &out, scale),
        DaemonCommand::Push { text, icon, json } => {
            let icon = icon.map(|name| checked_icon(&name)).transpose()?;
            simple(
                Command::Notify {
                    text: text.join(" "),
                    icon,
                },
                json,
            )
        }
        DaemonCommand::Metrics { filter, json } => metrics(&path, filter.as_deref(), json),
    }
}

pub(crate) fn send(path: &Path, command: Command) -> Result<Response> {
    nexus_ipc::send_request(path, command).with_context(|| {
        format!(
            "cannot reach linexusd at {}; is it running? (systemctl --user status linexusd)",
            path.display()
        )
    })
}

fn print_response(response: Response, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        println!("{}", response.message);
        if let Some(status) = &response.status {
            print_status(status);
        }
    }
    if response.ok {
        Ok(())
    } else {
        bail!("daemon command failed")
    }
}

fn print_status(status: &DaemonStatus) {
    println!("mode: {:?}", status.mode);
    println!("device: {:?}", status.device.state);
    if let Some(path) = &status.device.path {
        println!("device path: {path}");
    }
    if let Some(error) = &status.device.error {
        println!("device error: {error}");
    }
    println!("profile: {} ({})", status.profile_name, status.profile_id);
    println!(
        "page: {} ({}) [{}/{}]",
        status.page_name,
        status.page_id,
        status.page_index + 1,
        status.page_count
    );
    println!("brightness: {}%", status.brightness_percent);
    println!("blanked: {}", status.blanked);
    println!("manual blanked: {}", status.manual_blanked);
    println!("session blanked: {}", status.session_blanked);
}

fn save_frame(socket: &Path, out: &Path, scale: u32) -> Result<()> {
    let response = send(socket, Command::Frame)?;
    if !response.ok {
        bail!("daemon could not provide a frame: {}", response.message);
    }
    let encoded = response
        .frame
        .context("daemon sent no frame data (is it older than this linexusctl?)")?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("daemon sent a frame that is not valid base64")?;
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .context("daemon sent a frame that is not a valid PNG")?
        .to_rgba8();
    let (w, h) = image.dimensions();
    crate::preview::save_scaled(out, image, scale)?;
    println!(
        "Frame written: {} ({}x{})",
        out.display(),
        w * scale,
        h * scale
    );
    Ok(())
}

/// Lucide name without the `lucide:` prefix, or an error that suggests
/// similar names.
fn checked_icon(name: &str) -> Result<String> {
    let bare = name.strip_prefix("lucide:").unwrap_or(name);
    if nexus_render::icons::exists(bare) {
        return Ok(bare.to_string());
    }
    let needle = bare.to_ascii_lowercase();
    let similar: Vec<&str> = nexus_render::icons::list()
        .into_iter()
        .filter(|(icon, tags)| icon.contains(&needle) || tags.contains(&needle))
        .map(|(icon, _)| icon)
        .take(10)
        .collect();
    if similar.is_empty() {
        bail!("unknown icon '{bare}' (icons are Lucide names such as bell, check or info)");
    }
    bail!("unknown icon '{bare}'; similar: {}", similar.join(", "))
}

fn metrics(socket: &Path, filter: Option<&str>, json: bool) -> Result<()> {
    let response = send(socket, Command::Metrics)?;
    if !response.ok {
        bail!("daemon could not list metrics: {}", response.message);
    }
    let mut readings = response
        .metrics
        .context("daemon sent no metrics (is it older than this linexusctl?)")?;
    if let Some(filter) = filter {
        let filter = filter.to_ascii_lowercase();
        readings.retain(|r| {
            r.id.to_ascii_lowercase().contains(&filter)
                || r.label.to_ascii_lowercase().contains(&filter)
        });
    }
    readings.sort_by(|a, b| a.id.cmp(&b.id));
    if json {
        println!("{}", serde_json::to_string_pretty(&readings)?);
    } else if readings.is_empty() {
        println!("No metrics");
    } else {
        print!("{}", metrics_table(&readings));
    }
    Ok(())
}

fn metrics_table(readings: &[MetricReading]) -> String {
    let rows: Vec<[String; 4]> = readings
        .iter()
        .map(|r| {
            let (value, unit) = format_reading(r.value, &r.unit);
            [r.id.clone(), r.label.clone(), value, unit]
        })
        .collect();
    let header = ["ID", "LABEL", "VALUE", "UNIT"].map(String::from);
    let width = |col: usize| {
        rows.iter()
            .chain(std::iter::once(&header))
            .map(|row| row[col].chars().count())
            .max()
            .unwrap_or(0)
    };
    let (w0, w1, w2) = (width(0), width(1), width(2));
    std::iter::once(&header)
        .chain(&rows)
        .map(|[id, label, value, unit]| {
            format!("{id:<w0$}  {label:<w1$}  {value:>w2$}  {unit}")
                .trim_end()
                .to_string()
                + "\n"
        })
        .collect()
}

/// Human-readable number and display unit for a reading in the daemon's
/// unit vocabulary.
fn format_reading(value: f32, unit: &str) -> (String, String) {
    match unit {
        "percent" => (format!("{value:.0}"), "%".into()),
        "celsius" => (format!("{value:.1}"), "°C".into()),
        "watts" => (format!("{value:.0}"), "W".into()),
        "rpm" => (format!("{value:.0}"), "rpm".into()),
        "megahertz" => (format!("{value:.0}"), "MHz".into()),
        "bytes" => scale_bytes(value, ""),
        "bytes_per_sec" => scale_bytes(value, "/s"),
        "plain" | "" => (trim_number(value), String::new()),
        other => (trim_number(value), other.to_string()),
    }
}

fn scale_bytes(value: f32, suffix: &str) -> (String, String) {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut scaled = f64::from(value);
    let mut unit = 0;
    while scaled.abs() >= 1024.0 && unit < UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    let number = if unit == 0 {
        format!("{scaled:.0}")
    } else {
        format!("{scaled:.1}")
    };
    (number, format!("{}{suffix}", UNITS[unit]))
}

fn trim_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(id: &str, label: &str, value: f32, unit: &str) -> MetricReading {
        MetricReading {
            id: id.into(),
            label: label.into(),
            value,
            unit: unit.into(),
            max: 100.0,
        }
    }

    #[test]
    fn formats_units_for_humans() {
        assert_eq!(format_reading(23.4, "percent"), ("23".into(), "%".into()));
        assert_eq!(
            format_reading(64.0, "celsius"),
            ("64.0".into(), "°C".into())
        );
        assert_eq!(
            format_reading(18.4 * 1024.0 * 1024.0 * 1024.0, "bytes"),
            ("18.4".into(), "GiB".into())
        );
        assert_eq!(
            format_reading(2.5 * 1024.0 * 1024.0, "bytes_per_sec"),
            ("2.5".into(), "MiB/s".into())
        );
        assert_eq!(format_reading(512.0, "bytes"), ("512".into(), "B".into()));
        assert_eq!(format_reading(3.0, "plain"), ("3".into(), String::new()));
    }

    #[test]
    fn table_aligns_columns() {
        let table = metrics_table(&[
            reading("cpu.usage", "CPU", 23.0, "percent"),
            reading("gpu.temp", "GPU temp", 70.0, "celsius"),
        ]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "ID         LABEL     VALUE  UNIT");
        assert_eq!(lines[1], "cpu.usage  CPU          23  %");
        assert_eq!(lines[2], "gpu.temp   GPU temp   70.0  °C");
    }

    #[test]
    fn icon_names_are_checked() {
        assert_eq!(checked_icon("lucide:bell").unwrap(), "bell");
        let err = checked_icon("bel").unwrap_err().to_string();
        assert!(err.contains("similar") && err.contains("bell"), "{err}");
    }
}
