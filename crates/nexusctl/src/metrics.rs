//! `linexusctl sensors`: the readings collected locally, without the daemon.

pub(crate) fn print_metrics(metrics: &nexus_sensors::SystemMetrics) {
    println!("CPU usage: {}", fmt_percent(metrics.cpu_usage));
    println!("CPU temp: {}", fmt_temp(metrics.cpu_temp_c));
    println!("RAM usage: {}", fmt_percent(metrics.ram_usage));
    if let (Some(used), Some(total)) = (metrics.ram_used_gib, metrics.ram_total_gib) {
        println!("RAM used: {used:.1} / {total:.1} GiB");
    }
    println!("GPU temp: {}", fmt_temp(metrics.gpu_temp_c));
    println!("GPU usage: {}", fmt_percent(metrics.gpu_usage));
    println!("VRAM usage: {}", fmt_percent(metrics.vram_usage));
    println!("GPU mem temp: {}", fmt_temp(metrics.gpu_mem_temp_c));
    println!(
        "GPU power: {}",
        metrics
            .gpu_power_w
            .map(|v| format!("{v:.1} W"))
            .unwrap_or_else(|| "—".into())
    );
    println!(
        "GPU fan: {}",
        metrics
            .gpu_fan_rpm
            .map(|v| format!("{v} RPM"))
            .unwrap_or_else(|| "—".into())
    );
    println!(
        "Volume: {}",
        metrics
            .volume_percent
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "—".into())
    );
    println!("Network RX: {}", fmt_bps(metrics.network_rx_bps));
    println!("Network TX: {}", fmt_bps(metrics.network_tx_bps));
    println!("Disk read: {}", fmt_bps(metrics.disk_read_bps));
    println!("Disk write: {}", fmt_bps(metrics.disk_write_bps));
    println!("Disk / usage: {}", fmt_percent(metrics.disk_root_usage));
    println!(
        "Media: {}",
        match (
            &metrics.media_player,
            &metrics.media_playback_status,
            &metrics.media_title
        ) {
            (Some(player), status, Some(title)) => format!(
                "{} [{}] — {}",
                player,
                status.as_deref().unwrap_or("unknown"),
                title
            ),
            (Some(player), status, None) => format!(
                "{} [{}] — no metadata",
                player,
                status.as_deref().unwrap_or("unknown")
            ),
            _ => "—".into(),
        }
    );
    if !metrics.app_metrics.is_empty() {
        println!("App metrics:");
        for app in metrics.app_metrics.iter().take(6) {
            println!(
                "- {}: CPU {:.1}% RAM {:.1} MiB procs {}{}",
                app.name,
                app.cpu_percent,
                app.rss_mb,
                app.process_count,
                app.top_pid
                    .map(|pid| format!(" top_pid={pid}"))
                    .unwrap_or_default()
            );
        }
    }
    if let Some(artist) = &metrics.media_artist {
        if !artist.is_empty() {
            println!("Media artist: {artist}");
        }
    }
    if let Some(url) = &metrics.media_url {
        println!("Media URL: {url}");
    }
    if let Some(pid) = metrics.media_pid {
        println!("Media PID: {pid}");
    }
    if !metrics.sensor_records.is_empty() {
        println!("Sensor records:");
        for record in metrics.sensor_records.iter().take(10) {
            let value = serde_json::to_string(&record.value).unwrap_or_else(|_| "{}".into());
            println!(
                "- {}:{} {} {}",
                record.module, record.kind, record.label, value
            );
        }
    }
    println!(
        "MPRIS players: {}",
        if metrics.mpris_players.is_empty() {
            "none".into()
        } else {
            metrics.mpris_players.join(", ")
        }
    );
}

fn fmt_bps(value: Option<f32>) -> String {
    let Some(value) = value else {
        return "—".into();
    };
    if value >= 1_000_000_000.0 {
        format!("{:.2} GB/s", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.2} MB/s", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1} KB/s", value / 1_000.0)
    } else {
        format!("{value:.0} B/s")
    }
}

fn fmt_percent(value: Option<f32>) -> String {
    value
        .map(|v| format!("{:.0}%", v * 100.0))
        .unwrap_or_else(|| "—".into())
}

fn fmt_temp(value: Option<f32>) -> String {
    value
        .map(|v| format!("{v:.1} °C"))
        .unwrap_or_else(|| "—".into())
}
