//! `linexusctl config`: create, check, upgrade and inspect the config file.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use nexus_config::{ActionSpec, MigrationReport, NexusConfig, SCHEMA_VERSION};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the config file path.
    Path,
    /// Write the built-in default config.
    Init {
        /// Config file; the default path when omitted.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Replace an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Check a config file and summarize it.
    Validate {
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Rewrite a config from an older schema in the current one, keeping a
    /// backup of the original next to it.
    Upgrade {
        #[arg(long)]
        path: Option<PathBuf>,
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the config as pretty JSON (older schemas are shown upgraded).
    Show {
        #[arg(long)]
        path: Option<PathBuf>,
        /// Print the built-in default config instead.
        #[arg(long, conflicts_with = "path")]
        default: bool,
    },
    /// Show which widget and actions sit at an X position of a page.
    HitTest {
        #[arg(long)]
        path: Option<PathBuf>,
        /// Profile; the active profile when omitted.
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        page: String,
        /// Position on the strip, 0..=639.
        #[arg(long)]
        x: u16,
    },
}

pub(crate) fn run(command: ConfigCommand) -> Result<()> {
    let or_default = |path: Option<PathBuf>| path.unwrap_or_else(nexus_config::default_config_path);
    match command {
        ConfigCommand::Path => {
            println!("{}", nexus_config::default_config_path().display());
            Ok(())
        }
        ConfigCommand::Init { path, force } => init(&or_default(path), force),
        ConfigCommand::Validate { path } => validate(&or_default(path)),
        ConfigCommand::Upgrade { path, dry_run } => upgrade(&or_default(path), dry_run),
        ConfigCommand::Show { path, default } => {
            let config = if default {
                nexus_config::default_config()
            } else {
                load(&or_default(path))?.0
            };
            print!("{}", nexus_config::to_pretty_json(&config)?);
            Ok(())
        }
        ConfigCommand::HitTest {
            path,
            profile,
            page,
            x,
        } => hit_test(&load(&or_default(path))?.0, profile.as_deref(), &page, x),
    }
}

fn load(path: &Path) -> Result<(NexusConfig, Option<MigrationReport>)> {
    if !path.exists() {
        bail!(
            "no config at {} (create one with `linexusctl config init`)",
            path.display()
        );
    }
    nexus_config::load_with_report(path)
        .with_context(|| format!("config {} is not valid", path.display()))
}

fn init(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            path.display()
        );
    }
    nexus_config::save_to_path(path, &nexus_config::default_config(), true)
        .with_context(|| format!("failed to write config {}", path.display()))?;
    println!("Config written: {}", path.display());
    Ok(())
}

fn validate(path: &Path) -> Result<()> {
    let (config, migration) = load(path)?;
    let active = nexus_config::active_profile(&config)
        .context("active profile disappeared after validation")?;
    let pages: usize = config.profiles.iter().map(|p| p.pages.len()).sum();
    let widgets: usize = config
        .profiles
        .iter()
        .flat_map(|p| &p.pages)
        .map(|page| page.widgets.len())
        .sum();
    println!("Config OK: {}", path.display());
    println!(
        "  {} profile(s), {pages} page(s), {widgets} widget(s); active profile '{}' ({})",
        config.profiles.len(),
        active.name,
        active.id
    );
    if let Some(report) = migration {
        println!(
            "  note: the file uses schema {}; it works, but `linexusctl config upgrade` rewrites it in schema {SCHEMA_VERSION}",
            report.from_version
        );
        print_notes(&report);
    }
    Ok(())
}

fn upgrade(path: &Path, dry_run: bool) -> Result<()> {
    let (_, migration) = load(path)?;
    let Some(report) = migration else {
        println!(
            "Config already uses schema {SCHEMA_VERSION}: {}",
            path.display()
        );
        return Ok(());
    };
    let backup = backup_path(path, report.from_version);
    if dry_run {
        println!(
            "Would upgrade {} from schema {} to {SCHEMA_VERSION} ({} page(s), {} button(s)); backup: {}",
            path.display(),
            report.from_version,
            report.pages,
            report.buttons,
            backup.display()
        );
        print_notes(&report);
        return Ok(());
    }
    let backup_existed = backup.exists();
    nexus_config::load_and_upgrade(path)
        .with_context(|| format!("failed to upgrade config {}", path.display()))?;
    println!(
        "Config upgraded: {} (schema {} -> {SCHEMA_VERSION}, {} page(s), {} button(s))",
        path.display(),
        report.from_version,
        report.pages,
        report.buttons
    );
    if backup_existed {
        println!("  kept the existing backup {}", backup.display());
    } else {
        println!("  backup of the original: {}", backup.display());
    }
    print_notes(&report);
    Ok(())
}

/// Where [`nexus_config::load_and_upgrade`] keeps the original file.
fn backup_path(path: &Path, from_version: u32) -> PathBuf {
    path.with_extension(format!("json.v{from_version}.bak"))
}

fn print_notes(report: &MigrationReport) {
    for note in &report.notes {
        println!("  - {note}");
    }
}

fn hit_test(config: &NexusConfig, profile: Option<&str>, page_id: &str, x: u16) -> Result<()> {
    let profile_id = profile.unwrap_or(&config.active_profile);
    let profile = config
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .with_context(|| format!("profile '{profile_id}' not found"))?;
    let page = profile
        .pages
        .iter()
        .find(|p| p.id == page_id)
        .with_context(|| format!("page '{page_id}' not found in profile '{}'", profile.id))?;
    let Some(widget) = nexus_config::widget_at_x(page, x) else {
        println!(
            "No widget at x={x} on page '{}' (profile '{}')",
            page.id, profile.id
        );
        return Ok(());
    };
    println!(
        "x={x} on page '{}' (profile '{}'): widget '{}' ({}, x {}..{})",
        page.id,
        profile.id,
        widget.id,
        widget.kind.type_name(),
        widget.x,
        widget.end()
    );
    for (gesture, action) in [
        ("tap", &widget.tap),
        ("double tap", &widget.double_tap),
        ("hold", &widget.hold),
    ] {
        println!("  {gesture}: {}", describe_action(action.as_ref()));
    }
    Ok(())
}

fn describe_action(action: Option<&ActionSpec>) -> String {
    match action {
        Some(action) => serde_json::to_string(action).unwrap_or_else(|_| format!("{action:?}")),
        None => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_path_matches_the_library_convention() {
        assert_eq!(
            backup_path(Path::new("/x/config.json"), 1),
            PathBuf::from("/x/config.json.v1.bak")
        );
    }

    #[test]
    fn upgrade_rewrites_a_schema1_file_and_keeps_a_backup() {
        let dir = std::env::temp_dir().join(format!("linexusctl-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let v1 = r#"{"schema_version":1,"active_profile":"p","profiles":[{"id":"p","name":"P",
            "brightness":60,"refresh_ms":1000,"pages":[{"id":"s","name":"Status","kind":"status","elements":[]}]}]}"#;
        std::fs::write(&path, v1).unwrap();

        upgrade(&path, true).unwrap();
        assert!(!backup_path(&path, 1).exists(), "dry run writes nothing");

        upgrade(&path, false).unwrap();
        assert_eq!(std::fs::read_to_string(backup_path(&path, 1)).unwrap(), v1);
        let (config, migration) = nexus_config::load_with_report(&path).unwrap();
        assert!(migration.is_none());
        assert_eq!(config.profiles[0].brightness, 60);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let dir = std::env::temp_dir().join(format!("linexusctl-init-{}", std::process::id()));
        let path = dir.join("config.json");
        init(&path, false).unwrap();
        assert!(init(&path, false)
            .unwrap_err()
            .to_string()
            .contains("--force"));
        init(&path, true).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
