//! `linexusctl gnome-extension`: the GNOME Shell extension that tells the
//! daemon which app has focus. GNOME shows the focused window only to shell
//! extensions, so app-aware profiles need it there.

use crate::util::home_dir;
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use nexus_sensors::{
    gnome_focus_path, gnome_shell_major, gnome_shell_on_bus, gsettings_get, is_gnome,
    parse_gsettings_strings, GnomeExtensionStatus, GNOME_EXTENSION_UUID,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// For GNOME Shell 45 and newer (ES modules).
const EXTENSION_JS: &str = include_str!("gnome/extension.js");
/// For GNOME Shell 40 to 44.
const EXTENSION_LEGACY_JS: &str = include_str!("gnome/extension-legacy.js");
/// The newest GNOME Shell the extension was tested with.
const TESTED_UP_TO: u32 = 50;
/// Newer GNOME Shell versions the metadata accepts in advance: GNOME refuses
/// an extension whose metadata does not list the running version, and the
/// few calls it makes have stayed the same since 45.
const AHEAD: u32 = 4;

#[derive(Debug, Subcommand)]
pub(crate) enum GnomeExtensionCommand {
    /// Install or update the extension for this user and enable it.
    Install,
    /// Disable the extension and remove it from this user's extensions.
    Remove,
    /// Show whether the extension is installed, enabled and running.
    Status,
}

pub(crate) fn run(command: GnomeExtensionCommand) -> Result<()> {
    match command {
        GnomeExtensionCommand::Install => install(),
        GnomeExtensionCommand::Remove => remove(),
        GnomeExtensionCommand::Status => {
            println!("GNOME Shell session: {}", yes_no(gnome_session()));
            let status = GnomeExtensionStatus::read();
            match status.shell {
                Some(major) => println!("GNOME Shell: {major}"),
                None => println!("GNOME Shell: not found"),
            }
            match &status.dir {
                Some(dir) => println!("installed: {}", dir.display()),
                None => println!("installed: no"),
            }
            println!(
                "supports this GNOME Shell: {}",
                yes_no(status.supports_shell)
            );
            println!("enabled: {}", yes_no(status.enabled));
            if status.user_extensions_off {
                println!("GNOME extensions are turned off (disable-user-extensions)");
            }
            println!("running: {}", yes_no(status.running));
            Ok(())
        }
    }
}

/// Whether the desktop session runs GNOME Shell, as the systemd user manager
/// (or, without one, this process) sees it.
pub(crate) fn gnome_session() -> bool {
    let env = nexus_actions::session_environment();
    let desktop = env
        .iter()
        .find(|(key, _)| key == "XDG_CURRENT_DESKTOP")
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var("XDG_CURRENT_DESKTOP").ok())
        .unwrap_or_default();
    is_gnome(&desktop.to_ascii_uppercase()) && gnome_shell_on_bus(&env)
}

pub(crate) fn install() -> Result<()> {
    let Some(major) = gnome_shell_major() else {
        bail!("GNOME Shell not found (gnome-shell --version)");
    };
    let (script, versions): (&str, Vec<u32>) = match major {
        40..=44 => (EXTENSION_LEGACY_JS, (40..=44).collect()),
        45.. => (
            EXTENSION_JS,
            (45..=major.max(TESTED_UP_TO) + AHEAD).collect(),
        ),
        _ => bail!("GNOME Shell {major} is too old; the extension needs 40 or newer"),
    };
    let dir = user_extension_dir().context("HOME is not set")?;
    let metadata = serde_json::json!({
        "uuid": GNOME_EXTENSION_UUID,
        "name": "Linexus focus",
        "description": "Tells Linexus which app has focus, so that profiles of the Corsair iCUE Nexus can follow it.",
        "url": "https://github.com/demoriaan/linexus",
        "shell-version": versions.iter().map(u32::to_string).collect::<Vec<_>>(),
    });
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    fs::write(dir.join("extension.js"), script)
        .with_context(|| format!("failed to write {}", dir.join("extension.js").display()))?;
    fs::write(
        dir.join("metadata.json"),
        format!("{}\n", serde_json::to_string_pretty(&metadata)?),
    )
    .with_context(|| format!("failed to write {}", dir.join("metadata.json").display()))?;
    println!("Installed the Linexus focus extension to {}", dir.display());

    if gsettings_get("disable-user-extensions").as_deref() == Some("true") {
        println!("GNOME has extensions turned off. Turn them on in the Extensions app, or app-aware profiles stay off.");
    }
    let mut enabled = gsettings_list("enabled-extensions")?;
    if !enabled.iter().any(|uuid| uuid == GNOME_EXTENSION_UUID) {
        enabled.push(GNOME_EXTENSION_UUID.to_string());
        gsettings_set_list("enabled-extensions", &enabled)?;
    }
    let mut disabled = gsettings_list("disabled-extensions")?;
    if disabled.iter().any(|uuid| uuid == GNOME_EXTENSION_UUID) {
        disabled.retain(|uuid| uuid != GNOME_EXTENSION_UUID);
        gsettings_set_list("disabled-extensions", &disabled)?;
    }
    println!("Enabled it in the GNOME settings.");
    if gnome_focus_path().is_some_and(|path| path.is_file()) {
        println!("GNOME Shell runs it.");
    } else {
        println!("GNOME Shell loads new extensions at login: log out and back in once.");
    }
    Ok(())
}

fn remove() -> Result<()> {
    if let Ok(mut enabled) = gsettings_list("enabled-extensions") {
        if enabled.iter().any(|uuid| uuid == GNOME_EXTENSION_UUID) {
            enabled.retain(|uuid| uuid != GNOME_EXTENSION_UUID);
            gsettings_set_list("enabled-extensions", &enabled)?;
            println!("Disabled the Linexus focus extension.");
        }
    }
    let dir = user_extension_dir().context("HOME is not set")?;
    if dir.is_dir() {
        fs::remove_dir_all(&dir).with_context(|| format!("failed to remove {}", dir.display()))?;
        println!("Removed {}", dir.display());
    }
    Ok(())
}

fn user_extension_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")))?;
    Some(
        data.join("gnome-shell/extensions")
            .join(GNOME_EXTENSION_UUID),
    )
}

/// The string list under `org.gnome.shell <key>`. A list with a quote inside
/// a value is refused: it is not split reliably, and writing it back could
/// drop or mangle another extension.
fn gsettings_list(key: &str) -> Result<Vec<String>> {
    let text = gsettings_get(key).context(
        "cannot read the GNOME settings (gsettings); run this inside your desktop session",
    )?;
    let list = parse_gsettings_strings(&text);
    if text.matches('\'').count() != list.len() * 2 || text.contains('"') {
        bail!("org.gnome.shell {key} holds values this tool cannot rewrite safely; enable {GNOME_EXTENSION_UUID} with the Extensions app instead");
    }
    Ok(list)
}

fn gsettings_set_list(key: &str, values: &[String]) -> Result<()> {
    let status = Command::new("gsettings")
        .args(["set", "org.gnome.shell", key, &format_string_list(values)])
        .status()
        .context("failed to run gsettings")?;
    if !status.success() {
        bail!("gsettings could not change org.gnome.shell {key}");
    }
    Ok(())
}

pub(crate) fn format_string_list(values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();
    format!("[{}]", quoted.join(", "))
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_gsettings_string_lists() {
        assert_eq!(parse_gsettings_strings("@as []"), Vec::<String>::new());
        let list = parse_gsettings_strings("['ding@rastersoft.com', 'ubuntu-dock@ubuntu.com']");
        assert_eq!(list, ["ding@rastersoft.com", "ubuntu-dock@ubuntu.com"]);
        assert_eq!(
            format_string_list(&list),
            "['ding@rastersoft.com', 'ubuntu-dock@ubuntu.com']"
        );
        assert_eq!(format_string_list(&[]), "[]");
    }

    #[test]
    fn reads_the_gnome_shell_version() {
        assert_eq!(
            nexus_sensors::parse_gnome_shell_major("GNOME Shell 50.6\n"),
            Some(50)
        );
        assert_eq!(
            nexus_sensors::parse_gnome_shell_major("GNOME Shell 42.9"),
            Some(42)
        );
        assert_eq!(
            nexus_sensors::parse_gnome_shell_major("gnome-shell: not found"),
            None
        );
    }
}
