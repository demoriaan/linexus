//! `.nexuspack` files: a zip with a manifest and the config, for sharing and
//! backing up profiles.
//!
//! Images (page backgrounds, image widgets, button icons) live inside the
//! config as `data:` URLs, so the config entry is the whole payload. Format 1
//! packs carried a schema-1 config plus an empty `resources/` folder; they
//! still import and are upgraded to the current schema on the way in.

use anyhow::{anyhow, bail, Context, Result};
use nexus_config::{MigrationReport, NexusConfig, Profile};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

mod review;
pub use review::{outside_reach, review_pack, visible_text, PackReview, Reach, ReachKind};

pub const PACK_FORMAT_VERSION: u32 = 2;
/// Oldest pack format that still imports.
pub const MIN_PACK_FORMAT_VERSION: u32 = 1;
pub const MANIFEST_PATH: &str = "manifest.json";
pub const CONFIG_PATH: &str = "config.json";
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Most bytes read from the config entry, a guard against zip bombs.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
/// Largest config a pack may install, measured as compact JSON. Nexus
/// Studio saves the whole config in one request of at most 16 MiB, so a
/// bigger config would import but could never be saved from Studio again.
pub const MAX_SAVEABLE_CONFIG_BYTES: u64 = 15 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackManifest {
    pub format_version: u32,
    pub created_by: String,
    pub created_at_unix_ms: u128,
    /// Schema of the config stored in the pack (older schemas are upgraded
    /// on import).
    pub config_schema_version: u32,
    pub active_profile: String,
    pub profile_count: usize,
    pub profiles: Vec<PackProfileSummary>,
    /// Images embedded in the config as `data:` URLs.
    #[serde(default)]
    pub embedded_images: usize,
    /// Format 1 listed files under `resources/`; always empty now that
    /// images are embedded in the config.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<PackResource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackProfileSummary {
    pub id: String,
    pub name: String,
    pub page_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackResource {
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub target_path: PathBuf,
    pub active_profile: String,
    pub profile_count: usize,
    pub page_count: usize,
    pub dry_run: bool,
    /// Schema the pack's config was upgraded from, if it was older.
    pub migrated_from: Option<u32>,
    /// What the upgrade could not carry over.
    pub notes: Vec<String>,
    /// Copy of the config the import replaced.
    pub backup_path: Option<PathBuf>,
}

pub fn build_manifest(config: &NexusConfig, created_by: impl Into<String>) -> PackManifest {
    PackManifest {
        format_version: PACK_FORMAT_VERSION,
        created_by: created_by.into(),
        created_at_unix_ms: now_unix_ms(),
        config_schema_version: config.schema_version,
        active_profile: config.active_profile.clone(),
        profile_count: config.profiles.len(),
        profiles: config.profiles.iter().map(profile_summary).collect(),
        embedded_images: count_embedded_images(config),
        resources: Vec::new(),
    }
}

/// Number of `data:image/...` URLs anywhere in the config.
pub fn count_embedded_images(config: &NexusConfig) -> usize {
    fn walk(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::String(s) => usize::from(s.starts_with("data:image/")),
            serde_json::Value::Array(items) => items.iter().map(walk).sum(),
            serde_json::Value::Object(map) => map.values().map(walk).sum(),
            _ => 0,
        }
    }
    serde_json::to_value(config).map(|v| walk(&v)).unwrap_or(0)
}

/// Exports the config file at `config_path`. An older schema is upgraded in
/// memory first, so the pack always carries the current schema.
pub fn export_pack(config_path: &Path, out_path: &Path, overwrite: bool) -> Result<PackManifest> {
    let config = nexus_config::load_from_path(config_path)
        .with_context(|| format!("failed to load config {}", config_path.display()))?;
    export_config(&config, out_path, overwrite, "linexusctl")
}

pub fn export_config(
    config: &NexusConfig,
    out_path: &Path,
    overwrite: bool,
    created_by: impl Into<String>,
) -> Result<PackManifest> {
    nexus_config::validate(config).context("config failed validation")?;
    check_config_size(config)?;
    let manifest = build_manifest(config, created_by);
    if out_path.exists() && !overwrite {
        bail!(
            "pack already exists: {} (use --force to overwrite)",
            out_path.display()
        );
    }
    ensure_parent_dir(out_path)?;

    let tmp_path = temp_path_for(out_path);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .with_context(|| format!("failed to create temporary pack {}", tmp_path.display()))?;
    let result = write_pack(file, &manifest, config).and_then(|()| {
        publish_temp(&tmp_path, out_path, overwrite)
            .with_context(|| format!("failed to publish pack {}", out_path.display()))
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result.map(|()| manifest)
}

fn write_pack(file: File, manifest: &PackManifest, config: &NexusConfig) -> Result<()> {
    let mut zip = ZipWriter::new(file);
    // Base64 image data shrinks by about a quarter under deflate.
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(MANIFEST_PATH, options)
        .context("failed to start manifest entry")?;
    zip.write_all(&serde_json::to_vec_pretty(manifest)?)
        .context("failed to write manifest entry")?;
    zip.write_all(b"\n")
        .context("failed to terminate manifest entry")?;

    zip.start_file(CONFIG_PATH, options)
        .context("failed to start config entry")?;
    zip.write_all(nexus_config::to_pretty_json(config)?.as_bytes())
        .context("failed to write config entry")?;

    let file = zip.finish().context("failed to finish pack")?;
    file.sync_all().context("failed to sync temporary pack")?;
    Ok(())
}

/// Reads and checks a pack without importing it. The manifest describes the
/// config as stored; `config_schema_version` below the current schema means
/// the config is upgraded on import.
pub fn inspect_pack(path: &Path) -> Result<PackManifest> {
    load_checked(path).map(|(manifest, _, _)| manifest)
}

/// Opens a pack and runs every check an import needs: manifest, config
/// schema and validation, manifest/config agreement and size.
pub(crate) fn load_checked(
    path: &Path,
) -> Result<(PackManifest, NexusConfig, Option<MigrationReport>)> {
    let mut archive = open_archive(path)?;
    let manifest = read_manifest(&mut archive)?;
    validate_manifest(&manifest)?;
    let (config, migration) = read_config(&mut archive)?;
    nexus_config::validate(&config).context("pack config failed validation")?;
    validate_manifest_matches_config(&manifest, &config, migration.as_ref())?;
    check_config_size(&config)?;
    Ok((manifest, config, migration))
}

/// Refuses configs above [`MAX_SAVEABLE_CONFIG_BYTES`] of compact JSON.
/// Export, import and Studio's save all apply it, so a config that can be
/// saved can also be exported and imported again.
pub fn check_config_size(config: &NexusConfig) -> Result<()> {
    let size = serde_json::to_vec(config)?.len() as u64;
    if size > MAX_SAVEABLE_CONFIG_BYTES {
        bail!(
            "the config is {:.1} MiB, more than the {} MiB Linexus Studio can save; \
             remove or shrink some images",
            size as f64 / (1024.0 * 1024.0),
            MAX_SAVEABLE_CONFIG_BYTES / (1024 * 1024)
        );
    }
    Ok(())
}

/// Imports a pack as the config at `config_path`. An existing config is
/// first copied to `config.json.bak` next to it.
pub fn import_pack(
    pack_path: &Path,
    config_path: &Path,
    overwrite: bool,
    dry_run: bool,
) -> Result<ImportReport> {
    if config_path.exists() && !overwrite {
        bail!(
            "config already exists: {} (use --force to overwrite)",
            config_path.display()
        );
    }

    let (_, config, migration) = load_checked(pack_path)?;
    let mut report = ImportReport {
        target_path: config_path.to_path_buf(),
        active_profile: config.active_profile.clone(),
        profile_count: config.profiles.len(),
        page_count: config.profiles.iter().map(|p| p.pages.len()).sum(),
        dry_run,
        migrated_from: migration.as_ref().map(|m| m.from_version),
        notes: migration.map(|m| m.notes).unwrap_or_default(),
        backup_path: None,
    };

    if !dry_run {
        report.backup_path = backup_config(config_path)?;
        write_config_atomically(config_path, &config, overwrite)
            .with_context(|| format!("failed to write config {}", config_path.display()))?;
    }

    Ok(report)
}

/// Copies the config at `path` to `<name>.bak` next to it (replacing an
/// older backup) before something replaces it. `None` when there is no
/// config yet.
pub fn backup_config(path: &Path) -> Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    let backup = path.with_file_name(format!("{name}.bak"));
    let tmp_path = temp_path_for(&backup);
    let result = fs::copy(path, &tmp_path)
        .map_err(anyhow::Error::from)
        .and_then(|_| publish_temp(&tmp_path, &backup, true));
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result.with_context(|| format!("failed to back up {}", path.display()))?;
    Ok(Some(backup))
}

fn open_archive(path: &Path) -> Result<ZipArchive<File>> {
    let file =
        File::open(path).with_context(|| format!("failed to open pack {}", path.display()))?;
    ZipArchive::new(file).with_context(|| format!("failed to read zip pack {}", path.display()))
}

fn read_manifest(archive: &mut ZipArchive<File>) -> Result<PackManifest> {
    let entry = archive
        .by_name(MANIFEST_PATH)
        .context("pack is missing manifest.json")?;
    let json = read_limited_string(entry, MAX_MANIFEST_BYTES, MANIFEST_PATH)?;
    serde_json::from_str(&json).context("failed to parse manifest.json")
}

/// The pack's config in the current schema, plus the upgrade report when it
/// was stored in an older one.
fn read_config(archive: &mut ZipArchive<File>) -> Result<(NexusConfig, Option<MigrationReport>)> {
    let entry = archive
        .by_name(CONFIG_PATH)
        .context("pack is missing config.json")?;
    let json = read_limited_string(entry, MAX_CONFIG_BYTES, CONFIG_PATH)?;
    nexus_config::parse(&json)
        .map_err(|e| anyhow!(e))
        .context("failed to parse config.json")
}

fn read_limited_string<R: Read>(mut reader: R, limit: u64, label: &str) -> Result<String> {
    let mut json = String::new();
    let bytes_read = reader
        .by_ref()
        .take(limit + 1)
        .read_to_string(&mut json)
        .with_context(|| format!("failed to read {label}"))?;
    if bytes_read as u64 > limit {
        bail!("{label} exceeds maximum size of {limit} bytes");
    }
    Ok(json)
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    Ok(())
}

fn write_config_atomically(path: &Path, config: &NexusConfig, overwrite: bool) -> Result<()> {
    if overwrite {
        // save_to_path already writes a temp file and renames it into place.
        return nexus_config::save_to_path(path, config, true).map_err(Into::into);
    }
    // Without --force the target must not appear between the existence check
    // and the write, so publish via hard link, which fails if it exists.
    ensure_parent_dir(path)?;
    let tmp_path = temp_path_for(path);
    nexus_config::save_to_path(&tmp_path, config, false)
        .with_context(|| format!("failed to write temporary config {}", tmp_path.display()))?;
    let result = publish_temp(&tmp_path, path, false);
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn temp_path_for(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nexuspack");
    parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        now_unix_ms()
    ))
}

fn publish_temp(tmp_path: &Path, final_path: &Path, overwrite: bool) -> Result<()> {
    if overwrite {
        fs::rename(tmp_path, final_path).with_context(|| {
            format!(
                "failed to rename {} to {}",
                tmp_path.display(),
                final_path.display()
            )
        })?;
    } else {
        fs::hard_link(tmp_path, final_path).with_context(|| {
            format!(
                "failed to create {} without overwriting an existing file",
                final_path.display()
            )
        })?;
        fs::remove_file(tmp_path)
            .with_context(|| format!("failed to remove temporary file {}", tmp_path.display()))?;
    }
    Ok(())
}

fn validate_manifest(manifest: &PackManifest) -> Result<()> {
    if !(MIN_PACK_FORMAT_VERSION..=PACK_FORMAT_VERSION).contains(&manifest.format_version) {
        bail!(
            "unsupported nexuspack format_version {}, expected {}..={}",
            manifest.format_version,
            MIN_PACK_FORMAT_VERSION,
            PACK_FORMAT_VERSION
        );
    }
    if !(1..=nexus_config::SCHEMA_VERSION).contains(&manifest.config_schema_version) {
        bail!(
            "unsupported config schema_version {}, expected 1..={}",
            manifest.config_schema_version,
            nexus_config::SCHEMA_VERSION
        );
    }
    if manifest.profile_count != manifest.profiles.len() {
        bail!(
            "manifest profile_count {} does not match profiles length {}",
            manifest.profile_count,
            manifest.profiles.len()
        );
    }
    Ok(())
}

/// Cross-checks the manifest against the config it ships with. For an
/// upgraded config only what the upgrade keeps (profile ids, names, active
/// profile) is compared; page counts may change in the upgrade.
fn validate_manifest_matches_config(
    manifest: &PackManifest,
    config: &NexusConfig,
    migration: Option<&MigrationReport>,
) -> Result<()> {
    let stored_schema = migration.map_or(config.schema_version, |m| m.from_version);
    if manifest.config_schema_version != stored_schema {
        bail!(
            "manifest config_schema_version {} does not match config schema_version {}",
            manifest.config_schema_version,
            stored_schema
        );
    }
    if manifest.active_profile != config.active_profile {
        bail!(
            "manifest active_profile '{}' does not match config active_profile '{}'",
            manifest.active_profile,
            config.active_profile
        );
    }
    if manifest.profile_count != config.profiles.len() {
        bail!(
            "manifest profile_count {} does not match config profile count {}",
            manifest.profile_count,
            config.profiles.len()
        );
    }
    for (manifest_profile, config_profile) in manifest.profiles.iter().zip(&config.profiles) {
        if manifest_profile.id != config_profile.id
            || manifest_profile.name != config_profile.name
            || (migration.is_none() && manifest_profile.page_count != config_profile.pages.len())
        {
            bail!(
                "manifest profile '{}' does not match config profile '{}'",
                manifest_profile.id,
                config_profile.id
            );
        }
    }
    if manifest.format_version >= 2 && manifest.embedded_images != count_embedded_images(config) {
        bail!(
            "manifest embedded_images {} does not match the {} images in the config",
            manifest.embedded_images,
            count_embedded_images(config)
        );
    }
    Ok(())
}

pub(crate) fn profile_summary(profile: &Profile) -> PackProfileSummary {
    PackProfileSummary {
        id: profile.id.clone(),
        name: profile.name.clone(),
        page_count: profile.pages.len(),
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests;
