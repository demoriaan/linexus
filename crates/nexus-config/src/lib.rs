//! Profiles, pages, widgets and actions for the Nexus strip.

mod actions;
mod defaults;
mod migrate;
mod model;
mod validate;

pub use defaults::{
    button, default_config, template_page, PageTemplate, DEFAULT_BROWSER, DEFAULT_FILES,
    DEFAULT_MAIL, DEFAULT_PROFILE_ID, DEFAULT_TERMINAL,
};
pub use migrate::{migrate, MigrationReport};
pub use model::*;
pub use validate::{
    validate, ConfigError, Result, MAX_IMAGE_DATA_URL_BYTES, MAX_SEQUENCE_DELAY_MS,
    MAX_SEQUENCE_STEPS,
};

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub const SCHEMA_VERSION: u32 = 2;

/// Widget whose X range contains `x`.
pub fn widget_at_x(page: &Page, x: u16) -> Option<&Widget> {
    page.widgets.iter().find(|widget| widget.contains(x))
}

pub fn active_profile(config: &NexusConfig) -> Option<&Profile> {
    config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile)
}

#[derive(Debug, thiserror::Error)]
pub enum IoConfigError {
    #[error("config IO error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config JSON error for {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot upgrade config {path}: {message}")]
    Migrate { path: PathBuf, message: String },
    #[error(transparent)]
    Validate(#[from] ConfigError),
}

pub type IoResult<T> = std::result::Result<T, IoConfigError>;

pub fn config_dir() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("linexus");
    }
    let home = env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".config/linexus")
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Parses a config document of any supported schema version.
pub fn parse(content: &str) -> std::result::Result<(NexusConfig, Option<MigrationReport>), String> {
    let raw: serde_json::Value = serde_json::from_str(content).map_err(|e| e.to_string())?;
    let (mut config, report) = match migrate(&raw)? {
        Some((config, report)) => (config, Some(report)),
        None => (
            serde_json::from_value(raw).map_err(|e| e.to_string())?,
            None,
        ),
    };
    migrate::clamp_legacy_values(&mut config);
    Ok((config, report))
}

/// Loads and validates a config file. Older schemas are upgraded in memory;
/// call [`load_and_upgrade`] to also rewrite the file.
pub fn load_from_path(path: &Path) -> IoResult<NexusConfig> {
    load_with_report(path).map(|(config, _)| config)
}

pub fn load_with_report(path: &Path) -> IoResult<(NexusConfig, Option<MigrationReport>)> {
    let content = fs::read_to_string(path).map_err(|source| IoConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let raw: serde_json::Value =
        serde_json::from_str(&content).map_err(|source| IoConfigError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    let migrated = migrate(&raw).map_err(|message| IoConfigError::Migrate {
        path: path.to_path_buf(),
        message,
    })?;
    let (mut config, report) = match migrated {
        Some((config, report)) => (config, Some(report)),
        None => (
            serde_json::from_value(raw).map_err(|source| IoConfigError::Json {
                path: path.to_path_buf(),
                source,
            })?,
            None,
        ),
    };
    migrate::clamp_legacy_values(&mut config);
    validate(&config)?;
    Ok((config, report))
}

/// Loads a config and, when it used an older schema, keeps a copy of the
/// original next to it (`config.json.v1.bak`) and writes the upgraded file.
pub fn load_and_upgrade(path: &Path) -> IoResult<(NexusConfig, Option<MigrationReport>)> {
    let (config, report) = load_with_report(path)?;
    if let Some(report) = &report {
        let backup = path.with_extension(format!("json.v{}.bak", report.from_version));
        if !backup.exists() {
            fs::copy(path, &backup).map_err(|source| IoConfigError::Io {
                path: backup.clone(),
                source,
            })?;
        }
        save_to_path(path, &config, true)?;
    }
    Ok((config, report))
}

/// Numbers the temp files of [`save_to_path`] within this process.
static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Validates and writes `config` atomically (temp file + rename).
pub fn save_to_path(path: &Path, config: &NexusConfig, overwrite: bool) -> IoResult<()> {
    validate(config)?;
    if !overwrite && path.exists() {
        return Err(IoConfigError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, "config exists"),
        });
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|source| IoConfigError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let json = to_pretty_json(config).map_err(|source| IoConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    // A temp file of its own per save, so concurrent writers (two Studio
    // tabs, Studio and linexusctl) never write into each other's file.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let (tmp, mut file) = loop {
        let tmp = parent.join(format!(
            ".{name}.{}-{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(IoConfigError::Io { path: tmp, source }),
        }
    };
    let written = file
        .write_all(json.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| IoConfigError::Io {
            path: tmp.clone(),
            source,
        })
        .and_then(|()| {
            fs::rename(&tmp, path).map_err(|source| IoConfigError::Io {
                path: path.to_path_buf(),
                source,
            })
        });
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

pub fn to_pretty_json(config: &NexusConfig) -> serde_json::Result<String> {
    serde_json::to_string_pretty(config).map(|mut s| {
        s.push('\n');
        s
    })
}

#[cfg(test)]
mod tests;
