//! `linexusctl pack` (shareable `.nexuspack` backups) and `linexusctl events`
//! (the daemon's event log).

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub(crate) enum PackCommand {
    /// Export a config to a .nexuspack file.
    Export {
        /// Source config; the default config path when omitted.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Destination .nexuspack file.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// Overwrite an existing .nexuspack file.
        #[arg(long)]
        force: bool,
    },
    /// Show what a .nexuspack contains without importing it.
    Inspect {
        #[arg(value_name = "FILE")]
        path: PathBuf,
        /// Print the manifest as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Import a .nexuspack as the config (older packs are upgraded). Shows
    /// what the pack's buttons can do outside the strip and asks first.
    Import {
        #[arg(value_name = "FILE")]
        path: PathBuf,
        /// Target config; the default config path when omitted.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Replace an existing config (it is kept as config.json.bak).
        #[arg(long)]
        force: bool,
        /// Check the pack and the target without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Import without asking for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum EventsCommand {
    /// Print the event log path.
    Path,
    /// Show recent events.
    List {
        #[arg(long, default_value_t = 50)]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
    /// Clear the event log.
    Clear,
}

pub(crate) fn run_pack(command: PackCommand) -> Result<()> {
    match command {
        PackCommand::Export { config, out, force } => {
            let config = config.unwrap_or_else(nexus_config::default_config_path);
            let manifest = nexus_pack::export_pack(&config, &out, force)
                .with_context(|| format!("failed to export nexuspack {}", out.display()))?;
            println!(
                "Nexuspack exported: {} ({} profile(s), {} image(s), active '{}')",
                out.display(),
                manifest.profile_count,
                manifest.embedded_images,
                manifest.active_profile
            );
            if may_carry_secrets(&config) {
                println!(
                    "Note: some buttons send web requests, open links, type text or pass \
                     arguments to programs. Keys or tokens in them are part of the pack; \
                     check them before you share it."
                );
            }
        }
        PackCommand::Inspect { path, json } => {
            let manifest = nexus_pack::inspect_pack(&path)
                .with_context(|| format!("failed to inspect nexuspack {}", path.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
                return Ok(());
            }
            println!("Nexuspack: {}", path.display());
            println!("format_version: {}", manifest.format_version);
            print!("config_schema_version: {}", manifest.config_schema_version);
            if manifest.config_schema_version < nexus_config::SCHEMA_VERSION {
                print!(" (upgraded to {} on import)", nexus_config::SCHEMA_VERSION);
            }
            println!();
            // Manifest text comes from the pack; escape it before printing.
            let text = nexus_pack::visible_text;
            println!("created_by: {}", text(&manifest.created_by));
            println!("active_profile: {}", text(&manifest.active_profile));
            println!("embedded images: {}", manifest.embedded_images);
            println!("profiles: {}", manifest.profile_count);
            for profile in &manifest.profiles {
                println!(
                    "- {} ({}) pages={}",
                    text(&profile.name),
                    text(&profile.id),
                    profile.page_count
                );
            }
            let review = nexus_pack::review_pack(&path)
                .with_context(|| format!("failed to inspect nexuspack {}", path.display()))?;
            println!();
            print_reach(&review.reach);
        }
        PackCommand::Import {
            path,
            config,
            force,
            dry_run,
            yes,
        } => {
            let config = config.unwrap_or_else(nexus_config::default_config_path);
            let review = nexus_pack::review_pack(&path)
                .with_context(|| format!("failed to inspect nexuspack {}", path.display()))?;
            print_review(&path, &review);
            // Check the target before asking, so a refused import does not
            // ask first.
            nexus_pack::import_pack(&path, &config, force, true)
                .with_context(|| format!("failed to import nexuspack {}", path.display()))?;
            if !dry_run && !confirm_import(yes)? {
                bail!("import cancelled; nothing was changed");
            }
            let report = nexus_pack::import_pack(&path, &config, force, dry_run)
                .with_context(|| format!("failed to import nexuspack {}", path.display()))?;
            // Names and upgrade notes were printed (escaped) with the review.
            println!(
                "Nexuspack {}: {} -> {} ({} profile(s), {} page(s))",
                if report.dry_run {
                    "import dry-run OK"
                } else {
                    "imported"
                },
                path.display(),
                report.target_path.display(),
                report.profile_count,
                report.page_count,
            );
            if let Some(backup) = &report.backup_path {
                println!("  previous config kept as {}", backup.display());
            }
        }
    }
    Ok(())
}

fn print_review(path: &Path, review: &nexus_pack::PackReview) {
    println!("Nexuspack: {}", path.display());
    println!(
        "  {} profile(s), {} page(s), {} image(s), active '{}'",
        review.profiles.len(),
        review.page_count,
        review.embedded_images,
        review.active_profile
    );
    if let Some(from) = review.migrated_from {
        println!(
            "  upgraded from config schema {from} to {}",
            nexus_config::SCHEMA_VERSION
        );
    }
    for note in &review.notes {
        println!("  - {note}");
    }
    println!();
    print_reach(&review.reach);
}

/// Lists, grouped by kind, every action in the pack that acts outside the
/// strip, with where it sits and what exactly it does.
fn print_reach(reach: &[nexus_pack::Reach]) {
    if reach.is_empty() {
        println!("Nothing in this pack acts outside the strip.");
        return;
    }
    println!("This pack has buttons that act outside the strip. Only import it if you trust where it came from.");
    let mut kind = None;
    for item in reach {
        if kind != Some(item.kind) {
            kind = Some(item.kind);
            println!("{}:", item.kind.describe());
        }
        let gesture = item.gesture.map(|g| format!(" ({})", g.replace('_', " ")));
        let step = if item.in_sequence {
            ", in a sequence"
        } else {
            ""
        };
        println!(
            "  - {} > {} > {}{}{}: {}",
            item.profile,
            item.page,
            item.widget,
            gesture.unwrap_or_default(),
            step,
            item.detail
        );
    }
}

/// `--yes`, or a "y" typed at the prompt. Without a terminal there is
/// nobody to ask, so the import needs `--yes`.
fn confirm_import(yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        bail!("pass --yes to import without a confirmation prompt");
    }
    print!("Replace the config with this pack? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub(crate) fn run_events(command: EventsCommand) -> Result<()> {
    let path = nexus_events::default_event_log_path();
    match command {
        EventsCommand::Path => println!("{}", path.display()),
        EventsCommand::List { lines, json } => {
            let records = nexus_events::read_recent(&path, lines)
                .with_context(|| format!("failed to read event log {}", path.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else if records.is_empty() {
                println!("No events in {}", path.display());
            } else {
                for record in &records {
                    print_event_record(record);
                }
            }
        }
        EventsCommand::Clear => {
            nexus_events::clear(&path)
                .with_context(|| format!("failed to clear event log {}", path.display()))?;
            println!("Event log cleared: {}", path.display());
        }
    }
    Ok(())
}

fn print_event_record(record: &nexus_events::EventRecord) {
    let fields: String = record
        .fields
        .iter()
        .map(|(key, value)| format!(" {key}={value}"))
        .collect();
    println!(
        "{} {:?} {} {} - {}{}",
        record.ts_ms, record.level, record.source, record.kind, record.message, fields
    );
}

/// Whether exported buttons may hold keys or tokens: web requests and links
/// (keys in URLs or bodies), typed text, and program arguments.
fn may_carry_secrets(path: &std::path::Path) -> bool {
    use nexus_pack::ReachKind::*;
    nexus_config::load_from_path(path).is_ok_and(|config| {
        nexus_pack::outside_reach(&config).iter().any(|reach| {
            matches!(reach.kind, HttpRequest | OpenUrl | TextInput)
                || (matches!(reach.kind, ShellCommand | CommandWidget)
                    && reach.detail.contains(' '))
        })
    })
}
