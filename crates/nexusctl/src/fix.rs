//! `linexusctl doctor --fix`: applies the fixes doctor can make safely, and
//! prints the commands for the ones that need root.

use crate::util::{
    current_uid, find_sibling_or_path_binary, home_dir, is_executable, run_best_effort,
    shell_quote, sudo_noninteractive_available, which,
};
use anyhow::{Context, Result};
use nexus_core::udev_rule_text;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(crate) const INPUT_HELPER_BINARY: &str = "/usr/local/lib/linexus/linexus-inputd";
const INPUT_HELPER_SERVICE: &str = "/etc/systemd/system/linexus-inputd.service";
pub(crate) const UDEV_RULE_PATHS: [&str; 2] = [
    "/etc/udev/rules.d/99-linexus.rules",
    "/usr/lib/udev/rules.d/99-linexus.rules",
];
pub(crate) const UDEV_INSTALL_COMMAND: &str = "linexusctl print-udev-rule | sudo tee /etc/udev/rules.d/99-linexus.rules >/dev/null && sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=hidraw";

pub(crate) fn run(dry_run: bool) -> Result<()> {
    println!("Applying fixes");
    if dry_run {
        println!("mode: dry-run (no files or services changed)");
    }
    fix_config(dry_run)?;
    fix_user_service(dry_run)?;
    fix_input_helper(dry_run)?;
    fix_udev_rule(dry_run)?;
    println!("After udev changes, unplug and replug the Nexus.");
    println!();
    Ok(())
}

fn fix_config(dry_run: bool) -> Result<()> {
    let path = nexus_config::default_config_path();
    if !path.is_file() {
        if dry_run {
            println!("config: would create default config at {}", path.display());
        } else {
            nexus_config::save_to_path(&path, &nexus_config::default_config(), false)
                .with_context(|| format!("failed to create config {}", path.display()))?;
            println!("config: created {}", path.display());
        }
        return Ok(());
    }
    match nexus_config::load_with_report(&path) {
        Ok((_, None)) => println!("config: OK ({})", path.display()),
        Ok((_, Some(report))) if dry_run => println!(
            "config: would upgrade {} from schema {} and keep a backup",
            path.display(),
            report.from_version
        ),
        Ok((_, Some(report))) => {
            nexus_config::load_and_upgrade(&path)
                .with_context(|| format!("failed to upgrade config {}", path.display()))?;
            println!(
                "config: upgraded {} from schema {} (backup: {})",
                path.display(),
                report.from_version,
                path.with_extension(format!("json.v{}.bak", report.from_version))
                    .display()
            );
            for note in &report.notes {
                println!("  - {note}");
            }
        }
        Err(error) => println!(
            "config: left untouched because it is invalid ({error}); fix it, or move it away and run `linexusctl config init`"
        ),
    }
    Ok(())
}

fn fix_user_service(dry_run: bool) -> Result<()> {
    let home = home_dir().context("HOME is not set")?;
    let service_dir = home.join(".config/systemd/user");
    let service_path = service_dir.join("linexusd.service");
    let Some(daemon) = find_sibling_or_path_binary("linexusd") else {
        println!("user service: cannot fix, linexusd not found next to linexusctl or in PATH");
        return Ok(());
    };
    let content = user_service_text(&daemon);
    let needs_write = fs::read_to_string(&service_path)
        .map(|existing| existing != content)
        .unwrap_or(true);
    if !needs_write {
        println!("user service: OK ({})", service_path.display());
    } else if dry_run {
        println!(
            "user service: would write {} with ExecStart={}",
            service_path.display(),
            daemon.display()
        );
    } else {
        fs::create_dir_all(&service_dir)
            .with_context(|| format!("failed to create {}", service_dir.display()))?;
        fs::write(&service_path, content)
            .with_context(|| format!("failed to write {}", service_path.display()))?;
        println!("user service: wrote {}", service_path.display());
    }

    if dry_run {
        println!("user service: would run `systemctl --user daemon-reload` and `systemctl --user enable linexusd.service`");
    } else if std::env::var_os("NEXUSCTL_DOCTOR_FIX_SKIP_SYSTEMD").is_some() {
        println!("user service: skipped systemctl (NEXUSCTL_DOCTOR_FIX_SKIP_SYSTEMD)");
    } else if let Some(systemctl) = which("systemctl") {
        run_best_effort(
            &systemctl,
            &["--user", "daemon-reload"],
            "user service: daemon-reload",
        );
        run_best_effort(
            &systemctl,
            &["--user", "enable", "linexusd.service"],
            "user service: enable linexusd.service",
        );
    } else {
        println!("user service: systemctl missing; enable the service manually");
    }
    Ok(())
}

fn fix_input_helper(dry_run: bool) -> Result<()> {
    let root_binary = Path::new(INPUT_HELPER_BINARY);
    let service_path = Path::new(INPUT_HELPER_SERVICE);
    let source_binary = find_sibling_or_path_binary("linexus-inputd");
    let allow_uid = input_helper_allow_uid();
    let service_text = input_helper_service_text(root_binary, allow_uid);
    let service_ok = fs::read_to_string(service_path)
        .map(|existing| existing == service_text)
        .unwrap_or(false);

    if is_executable(root_binary) && service_ok {
        println!(
            "input helper: OK ({} for uid {allow_uid})",
            service_path.display()
        );
        return Ok(());
    }
    if dry_run {
        let source = source_binary
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "linexus-inputd (not found)".into());
        println!(
            "input helper: would install {} from {source} and write {} for uid {allow_uid}, then enable it (needs root)",
            root_binary.display(),
            service_path.display()
        );
        return Ok(());
    }
    let Some(source_binary) = source_binary else {
        println!(
            "input helper: cannot fix, linexus-inputd not found next to linexusctl or in PATH"
        );
        return Ok(());
    };

    if current_uid() == 0 {
        if let Some(parent) = root_binary.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(&source_binary, root_binary).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_binary.display(),
                root_binary.display()
            )
        })?;
        fs::set_permissions(root_binary, fs::Permissions::from_mode(0o755))?;
        fs::write(service_path, service_text)
            .with_context(|| format!("failed to write {}", service_path.display()))?;
        println!(
            "input helper: installed {} and {}",
            root_binary.display(),
            service_path.display()
        );
        if let Some(systemctl) = which("systemctl") {
            run_best_effort(
                &systemctl,
                &["daemon-reload"],
                "input helper: daemon-reload",
            );
            run_best_effort(
                &systemctl,
                &["enable", "--now", "linexus-inputd.service"],
                "input helper: enable --now",
            );
        }
    } else if sudo_noninteractive_available() {
        let shell = format!(
            "install -d -m 0755 {dir} && install -m 0755 {src} {dst} && cat > {svc} <<'EOF'\n{service_text}EOF\nsystemctl daemon-reload && systemctl enable --now linexus-inputd.service",
            dir = shell_quote(&root_binary.parent().unwrap_or(Path::new("/")).display().to_string()),
            src = shell_quote(&source_binary.display().to_string()),
            dst = shell_quote(&root_binary.display().to_string()),
            svc = shell_quote(&service_path.display().to_string()),
        );
        run_best_effort(
            Path::new("sudo"),
            &["sh", "-c", &shell],
            "input helper: sudo install",
        );
    } else {
        println!("input helper: needs root privileges; run:");
        println!(
            "  sudo install -Dm 0755 {} {}",
            source_binary.display(),
            root_binary.display()
        );
        println!(
            "  linexusctl print-input-helper-service --allow-uid {allow_uid} | sudo tee {} >/dev/null",
            service_path.display()
        );
        println!("  sudo systemctl daemon-reload");
        println!("  sudo systemctl enable --now linexus-inputd.service");
    }
    Ok(())
}

fn fix_udev_rule(dry_run: bool) -> Result<()> {
    let installed = UDEV_RULE_PATHS.iter().any(|path| {
        fs::read_to_string(path)
            .map(|s| nexus_core::udev_rule_matches(&s))
            .unwrap_or(false)
    });
    if installed {
        println!("udev: OK (matching rule installed)");
        return Ok(());
    }
    let etc_path = Path::new(UDEV_RULE_PATHS[0]);
    if dry_run {
        println!(
            "udev: would install matching rule to {} (needs root)",
            etc_path.display()
        );
        return Ok(());
    }
    if current_uid() == 0 {
        if let Some(parent) = etc_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(etc_path, udev_rule_text())
            .with_context(|| format!("failed to write {}", etc_path.display()))?;
        println!("udev: installed {}", etc_path.display());
        if let Some(udevadm) = which("udevadm") {
            run_best_effort(
                &udevadm,
                &["control", "--reload-rules"],
                "udev: reload rules",
            );
            run_best_effort(
                &udevadm,
                &["trigger", "--subsystem-match=hidraw"],
                "udev: trigger hidraw",
            );
        }
    } else if sudo_noninteractive_available() {
        let shell = format!(
            "install -d -m 0755 /etc/udev/rules.d && cat > {} <<'EOF'\n{}EOF\nudevadm control --reload-rules && udevadm trigger --subsystem-match=hidraw",
            shell_quote(&etc_path.display().to_string()),
            udev_rule_text()
        );
        run_best_effort(
            Path::new("sudo"),
            &["sh", "-c", &shell],
            "udev: sudo install",
        );
    } else {
        println!("udev: needs root privileges; run:");
        println!("  {UDEV_INSTALL_COMMAND}");
    }
    Ok(())
}

pub(crate) fn user_service_text(daemon: &Path) -> String {
    format!(
        "[Unit]\nDescription=iCUE Nexus user daemon\nAfter=graphical-session.target dbus.service\nPartOf=graphical-session.target\n\n[Service]\nType=simple\nExecStart={}\nRestart=on-failure\nRestartSec=2s\n\n[Install]\nWantedBy=default.target\n",
        daemon.display()
    )
}

pub(crate) fn input_helper_service_text(binary: &Path, allow_uid: u32) -> String {
    format!(
        "[Unit]\nDescription=iCUE Nexus privileged input helper\nDocumentation=file:/usr/share/doc/linexus/input-helper.md\nAfter=systemd-logind.service\n\n[Service]\nType=simple\nExecStart={} --allow-uid {allow_uid}\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=true\nProtectSystem=strict\nProtectHome=read-only\nPrivateTmp=true\nDevicePolicy=closed\nDeviceAllow=/dev/uinput rw\nRestrictAddressFamilies=AF_UNIX\nLockPersonality=true\nMemoryDenyWriteExecute=true\n\n[Install]\nWantedBy=multi-user.target\n",
        binary.display()
    )
}

/// The user who should own the helper: the sudo caller when run via sudo.
fn input_helper_allow_uid() -> u32 {
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
        .unwrap_or_else(current_uid)
}
