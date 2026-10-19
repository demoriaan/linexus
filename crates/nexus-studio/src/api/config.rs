//! The strip configuration: load, validate and save it, the page templates
//! and theme presets the editor offers, and `.nexuspack` backups.
//!
//! Every write names the revision of the file it started from (`If-Match`
//! for saves). A save from a stale view (a second tab, or a tab opened
//! before `linexusctl` or a text editor changed the file) gets 409 instead of
//! silently overwriting the newer config.

use super::reload_daemon;
use crate::http::{Request, Response};
use crate::state::AppState;
use nexus_config::{MigrationReport, NexusConfig, PageTemplate};
use serde_json::{json, Value};
use std::fs;
use std::hash::{DefaultHasher, Hasher};
use std::path::PathBuf;
use std::sync::{MutexGuard, PoisonError};

/// Revision of a config file that does not exist yet.
const NO_REVISION: &str = "none";

// Any config a pack may install must fit one save request again.
const _: () = assert!(nexus_pack::MAX_SAVEABLE_CONFIG_BYTES < crate::http::MAX_BODY_BYTES as u64);

pub fn route(request: &Request, state: &AppState) -> Option<Response> {
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/config") => get(state),
        ("GET", "/api/config/default") => default(),
        ("POST", "/api/config") => save(state, request),
        ("POST", "/api/config/reset") => reset(state),
        ("POST", "/api/config/validate") => validate(&request.body),
        ("GET", "/api/templates") => templates(),
        ("GET", "/api/themes") => themes(),
        ("GET", "/api/pack/export") => export_pack(state),
        ("POST", "/api/pack/inspect") => review_upload(&request.body),
        ("POST", "/api/pack/import") => import_pack(state, request),
        _ => return None,
    };
    Some(response)
}

/// Short hash of `bytes`: config revisions and reviewed pack uploads.
fn digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    format!("{:016x}", hasher.finish())
}

/// The saved config's revision: a hash of the file, or `none` while there
/// is no file yet.
fn revision(state: &AppState) -> String {
    fs::read(&state.config_path).map_or_else(|_| NO_REVISION.to_string(), |bytes| digest(&bytes))
}

fn lock(state: &AppState) -> MutexGuard<'_, ()> {
    state
        .config_lock
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The saved config (upgraded in memory when it uses an older schema, with
/// the migration notes) or the built-in default when there is no file yet,
/// plus the revision a save must name.
fn get(state: &AppState) -> Response {
    let path = state.config_path.display().to_string();
    // Revision first: should the file change in between, the editor holds
    // an older revision than its content, so its next save gets a 409
    // rather than overwriting the change.
    let revision = revision(state);
    if !state.config_path.exists() {
        let config = nexus_config::default_config();
        let body = json!({
            "ok": true,
            "path": path,
            "exists": false,
            "config": config,
            "migrated": null,
            "revision": revision,
        });
        return Response::json(200, &body);
    }
    match nexus_config::load_with_report(&state.config_path) {
        Ok((config, report)) => Response::json(
            200,
            &json!({
                "ok": true,
                "path": path,
                "exists": true,
                "config": config,
                "migrated": report.as_ref().map(report_json),
                "revision": revision,
            }),
        ),
        Err(error) => Response::json(
            500,
            &json!({ "ok": false, "message": error.to_string(), "path": path, "exists": true }),
        ),
    }
}

/// The built-in config for "Reset to defaults"; the editor saves it through
/// `POST /api/config` like any other draft.
fn default() -> Response {
    let config = nexus_config::default_config();
    Response::json(200, &json!({ "ok": true, "config": config }))
}

fn report_json(report: &MigrationReport) -> Value {
    json!({
        "from_version": report.from_version,
        "pages": report.pages,
        "buttons": report.buttons,
        "notes": report.notes,
    })
}

/// Validates, checks that the file is still at the revision the edit
/// started from (`If-Match`), writes atomically and asks the daemon to
/// reload. Answers the new revision.
fn save(state: &AppState, request: &Request) -> Response {
    let config = match parse_config(&request.body) {
        Ok(config) => config,
        Err(errors) => return invalid(errors),
    };
    if let Err(error) = nexus_config::validate(&config) {
        return invalid(vec![error.to_string()]);
    }
    // The limit packs are held to, so every saved config can be exported
    // and imported again.
    if let Err(error) = nexus_pack::check_config_size(&config) {
        return Response::error(413, format!("{error:#}"));
    }
    let expected = request
        .header("if-match")
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty());
    let Some(expected) = expected else {
        return Response::error(
            428,
            "a save must name the config revision it started from (If-Match); reload Studio",
        );
    };
    let guard = lock(state);
    let current = revision(state);
    if expected != current {
        return Response::json(
            409,
            &json!({
                "ok": false,
                "conflict": true,
                "revision": current,
                "message": "the configuration was changed elsewhere after this page loaded it",
            }),
        );
    }
    if let Err(error) = nexus_config::save_to_path(&state.config_path, &config, true) {
        return Response::error(500, format!("failed to save the config: {error}"));
    }
    // The revision of what we wrote, not of what is on disk now: another
    // program writing in between must still cause a conflict next time.
    let revision = nexus_config::to_pretty_json(&config)
        .map_or_else(|_| revision(state), |json| digest(json.as_bytes()));
    drop(guard);
    let daemon = reload_daemon(state);
    Response::json(
        200,
        &json!({
            "ok": true,
            "path": state.config_path.display().to_string(),
            "message": "config saved",
            "revision": revision,
            "daemon": daemon,
        }),
    )
}

/// "Reset all pages": keeps the current file as `config.json.bak`, then
/// writes the built-in config.
fn reset(state: &AppState) -> Response {
    let config = nexus_config::default_config();
    let guard = lock(state);
    let backup = match nexus_pack::backup_config(&state.config_path) {
        Ok(backup) => backup,
        Err(error) => return Response::error(500, format!("{error:#}")),
    };
    if let Err(error) = nexus_config::save_to_path(&state.config_path, &config, true) {
        return Response::error(500, format!("failed to save the config: {error}"));
    }
    let revision = revision(state);
    drop(guard);
    let daemon = reload_daemon(state);
    Response::json(
        200,
        &json!({
            "ok": true,
            "config": config,
            "revision": revision,
            "backup": backup.map(|path| path.display().to_string()),
            "daemon": daemon,
        }),
    )
}

fn validate(body: &[u8]) -> Response {
    let errors = match parse_config(body) {
        Ok(config) => nexus_config::validate(&config)
            .err()
            .map(|error| vec![error.to_string()])
            .unwrap_or_default(),
        Err(errors) => errors,
    };
    Response::json(
        200,
        &json!({ "ok": true, "valid": errors.is_empty(), "errors": errors }),
    )
}

/// Parses a schema-2 config. The version is checked first so an old or
/// foreign document gets "unsupported schema" instead of a confusing
/// "missing field" error from deep inside the model.
fn parse_config(body: &[u8]) -> Result<NexusConfig, Vec<String>> {
    let raw: Value = serde_json::from_slice(body)
        .map_err(|error| vec![format!("invalid config JSON: {error}")])?;
    let version = raw.get("schema_version").and_then(Value::as_u64);
    if version != Some(u64::from(nexus_config::SCHEMA_VERSION)) {
        let found = version.map_or("none".to_string(), |v| v.to_string());
        return Err(vec![format!(
            "unsupported config schema version {found}; expected {}",
            nexus_config::SCHEMA_VERSION
        )]);
    }
    serde_json::from_value(raw).map_err(|error| vec![format!("invalid config: {error}")])
}

fn invalid(errors: Vec<String>) -> Response {
    let message = format!("config is invalid: {}", errors.join("; "));
    Response::json(
        400,
        &json!({ "ok": false, "message": message, "errors": errors }),
    )
}

fn templates() -> Response {
    let templates: Vec<Value> = PageTemplate::ALL
        .into_iter()
        .map(|template| {
            let id = template_id(template);
            let page = nexus_config::template_page(template, &id);
            json!({ "id": id, "name": template.name(), "page": page })
        })
        .collect();
    Response::json(200, &json!({ "ok": true, "templates": templates }))
}

/// The template's serde name (`overview`, `blank`, ...), which is also the
/// id the frontend sends back.
fn template_id(template: PageTemplate) -> String {
    serde_json::to_value(template)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| template.name().to_ascii_lowercase())
}

fn themes() -> Response {
    let presets: Vec<Value> = nexus_render::PRESETS
        .iter()
        .map(|id| {
            let p = nexus_render::Palette::preset(id);
            let series: Vec<String> = p.series.iter().map(|color| color.to_hex()).collect();
            json!({
                "id": id,
                "palette": {
                    "bg_top": p.bg_top.to_hex(),
                    "bg_bottom": p.bg_bottom.to_hex(),
                    "surface": p.surface.to_hex(),
                    "text": p.text.to_hex(),
                    "text_dim": p.text_dim.to_hex(),
                    "accent": p.accent.to_hex(),
                    "series": series,
                },
            })
        })
        .collect();
    Response::json(200, &json!({ "ok": true, "presets": presets }))
}

/// The saved config, or the default when none exists (a fresh install can
/// still export what the strip shows).
fn current_config(state: &AppState) -> Result<NexusConfig, Response> {
    if !state.config_path.exists() {
        return Ok(nexus_config::default_config());
    }
    nexus_config::load_from_path(&state.config_path)
        .map_err(|error| Response::error(500, error.to_string()))
}

fn export_pack(state: &AppState) -> Response {
    let config = match current_config(state) {
        Ok(config) => config,
        Err(response) => return response,
    };
    let scratch = match ScratchDir::new("export") {
        Ok(scratch) => scratch,
        Err(error) => return Response::error(500, format!("no scratch directory: {error}")),
    };
    let pack = scratch.0.join("backup.nexuspack");
    let bytes = nexus_pack::export_config(&config, &pack, false, "linexus-studio")
        .and_then(|_| fs::read(&pack).map_err(anyhow::Error::from));
    match bytes {
        Ok(bytes) => Response::bytes(200, "application/zip", bytes).with_header(
            "Content-Disposition",
            "attachment; filename=\"linexus-backup.nexuspack\"",
        ),
        Err(error) => Response::error(500, format!("pack export failed: {error:#}")),
    }
}

/// Writes an uploaded pack into a private scratch directory, since
/// nexus-pack works on paths. The directory goes away with the guard.
fn stage_upload(body: &[u8]) -> Result<(ScratchDir, PathBuf), Response> {
    if body.is_empty() {
        return Err(Response::error(400, "empty .nexuspack upload"));
    }
    let scratch = ScratchDir::new("import")
        .map_err(|error| Response::error(500, format!("no scratch directory: {error}")))?;
    let pack = scratch.0.join("upload.nexuspack");
    fs::write(&pack, body)
        .map_err(|error| Response::error(500, format!("failed to stage the upload: {error}")))?;
    Ok((scratch, pack))
}

/// First step of an import: checks the uploaded pack and describes what it
/// would install, including every action that acts outside the strip.
/// Nothing is written; `digest` names this upload for the second step.
fn review_upload(body: &[u8]) -> Response {
    let (_scratch, pack) = match stage_upload(body) {
        Ok(staged) => staged,
        Err(response) => return response,
    };
    match nexus_pack::review_pack(&pack) {
        Ok(review) => {
            let mut value = json!(review);
            value["ok"] = true.into();
            value["digest"] = digest(body).into();
            Response::json(200, &value)
        }
        Err(error) => Response::error(400, format!("this pack cannot be imported: {error:#}")),
    }
}

/// Second step: replaces the config with a pack the user reviewed, keeping
/// the old file as `config.json.bak`, and reloads the daemon.
/// `?reviewed=<digest>` must name this exact upload, so no client can skip
/// the review.
fn import_pack(state: &AppState, request: &Request) -> Response {
    let body = &request.body;
    if !body.is_empty() && request.query_param("reviewed") != Some(digest(body)) {
        return Response::error(
            428,
            "review the pack first: send it to /api/pack/inspect and confirm the import",
        );
    }
    let (_scratch, pack) = match stage_upload(body) {
        Ok(staged) => staged,
        Err(response) => return response,
    };
    let guard = lock(state);
    let report = match nexus_pack::import_pack(&pack, &state.config_path, true, false) {
        Ok(report) => report,
        Err(error) => return Response::error(400, format!("pack import failed: {error:#}")),
    };
    let config = nexus_config::load_from_path(&state.config_path).ok();
    let revision = revision(state);
    drop(guard);
    let daemon = reload_daemon(state);
    Response::json(
        200,
        &json!({
            "ok": true,
            "path": report.target_path.display().to_string(),
            "active_profile": report.active_profile,
            "profile_count": report.profile_count,
            "page_count": report.page_count,
            "config": config,
            "revision": revision,
            "backup": report.backup_path.map(|path| path.display().to_string()),
            "daemon": daemon,
        }),
    )
}

/// Private directory for nexus-pack, which works on paths. `create` fails
/// on an existing name and the mode is 0700, so nobody can plant a symlink
/// in a shared temp dir; the directory is removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "linexus-studio-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use crate::api::handle;
    use crate::http::Request;
    use crate::state::testing::{get, json, post, state};
    use crate::state::AppState;
    use nexus_config::{PageTemplate, Widget, WidgetKind};

    fn config_with_overlap() -> nexus_config::NexusConfig {
        let mut config = nexus_config::default_config();
        let page = &mut config.profiles[0].pages[0];
        page.widgets = vec![
            Widget::new("a", 0, 200, WidgetKind::Clock(Default::default())),
            Widget::new("b", 100, 200, WidgetKind::Clock(Default::default())),
        ];
        config
    }

    #[test]
    fn missing_file_yields_default_config() {
        let response = handle(&get("/api/config"), &state());
        let body = json(&response);
        assert_eq!(response.status, 200);
        assert_eq!(body["exists"], false);
        assert_eq!(body["config"]["schema_version"], 2);
        assert!(body["migrated"].is_null());
        let default = json(&handle(&get("/api/config/default"), &state()));
        assert_eq!(default["ok"], true);
        assert_eq!(default["config"], body["config"]);
    }

    #[test]
    fn save_rejects_invalid_configs_with_readable_errors() {
        let state = state();
        let overlap = serde_json::to_vec(&config_with_overlap()).unwrap();
        let mut old = serde_json::to_value(nexus_config::default_config()).unwrap();
        old["schema_version"] = 1.into();
        let old = old.to_string();
        let cases: [(&[u8], &str); 4] = [
            (b"{not json", "invalid config JSON"),
            (&overlap, "widgets 'a' and 'b' overlap"),
            (old.as_bytes(), "unsupported config schema version 1"),
            (br#"{"schema_version":2}"#, "missing field"),
        ];
        for (body, expected) in cases {
            let response = handle(&post("/api/config", body), &state);
            let json = json(&response);
            assert_eq!(response.status, 400, "{expected}");
            assert_eq!(json["ok"], false);
            let first = json["errors"][0].as_str().unwrap();
            assert!(
                first.contains(expected),
                "{first:?} should mention {expected:?}"
            );
            assert!(json["message"].as_str().unwrap().contains(expected));
        }
        assert!(!state.config_path.exists(), "nothing may be written");
    }

    /// `POST /api/config` naming `revision` in `If-Match`.
    fn save_request(config: &nexus_config::NexusConfig, revision: &str) -> Request {
        let mut request = post("/api/config", &serde_json::to_vec(config).unwrap());
        request
            .headers
            .push(("if-match".into(), format!("\"{revision}\"")));
        request
    }

    fn revision(state: &AppState) -> String {
        let body = json(&handle(&get("/api/config"), state));
        body["revision"].as_str().unwrap().to_string()
    }

    #[test]
    fn save_writes_valid_config_and_get_reads_it_back() {
        let state = state();
        let mut config = nexus_config::default_config();
        config.profiles[0].name = "Desk".into();
        let response = handle(&save_request(&config, &revision(&state)), &state);
        let body = json(&response);
        assert_eq!(response.status, 200, "{body}");
        assert!(body["daemon"].is_null(), "no daemon in tests");
        let loaded = json(&handle(&get("/api/config"), &state));
        assert_eq!(loaded["exists"], true);
        assert_eq!(loaded["config"]["profiles"][0]["name"], "Desk");
        assert_eq!(loaded["revision"], body["revision"]);
    }

    #[test]
    fn saves_from_a_stale_view_are_refused() {
        let state = state();
        assert_eq!(revision(&state), "none", "no file yet");
        let config = nexus_config::default_config();
        let unconditional = post("/api/config", &serde_json::to_vec(&config).unwrap());
        assert_eq!(handle(&unconditional, &state).status, 428);
        assert!(!state.config_path.exists());

        let first = json(&handle(&save_request(&config, "none"), &state));
        let saved = first["revision"].as_str().unwrap().to_string();
        assert_ne!(saved, "none");
        // A second tab that loaded before the first save.
        let stale = handle(&save_request(&config, "none"), &state);
        assert_eq!(stale.status, 409);
        assert_eq!(json(&stale)["revision"], saved.as_str());

        // linexusctl or a text editor changes the file behind Studio's back.
        let mut edited = config.clone();
        edited.profiles[0].name = "Edited".into();
        nexus_config::save_to_path(&state.config_path, &edited, true).unwrap();
        assert_eq!(handle(&save_request(&config, &saved), &state).status, 409);
        let loaded = json(&handle(&get("/api/config"), &state));
        assert_eq!(loaded["config"]["profiles"][0]["name"], "Edited");

        // "Overwrite" sends the revision the 409 reported.
        let current = loaded["revision"].as_str().unwrap();
        assert_eq!(handle(&save_request(&config, current), &state).status, 200);
    }

    #[test]
    fn saves_too_large_to_export_are_refused() {
        let state = state();
        let mut config = nexus_config::default_config();
        let src = format!("data:image/png;base64,{}", "A".repeat(2_900_000));
        let mut page = nexus_config::template_page(PageTemplate::Blank, "photos");
        page.widgets = (0..6)
            .map(|i| {
                let image = nexus_config::ImageProps {
                    src: src.clone(),
                    fit: nexus_config::ImageFit::Fit,
                };
                Widget::new(format!("photo-{i}"), i * 100, 100, WidgetKind::Image(image))
            })
            .collect();
        config.profiles[0].pages.push(page);
        let response = handle(&save_request(&config, "none"), &state);
        assert_eq!(response.status, 413);
        let message = json(&response)["message"].as_str().unwrap().to_string();
        assert!(message.contains("Linexus Studio can save"), "{message}");
        assert!(!state.config_path.exists());
    }

    #[test]
    fn reset_keeps_the_old_config_as_backup() {
        let state = state();
        let mut config = nexus_config::default_config();
        config.profiles[0].name = "Desk".into();
        assert_eq!(handle(&save_request(&config, "none"), &state).status, 200);

        let response = handle(&post("/api/config/reset", b""), &state);
        let body = json(&response);
        assert_eq!(response.status, 200, "{body}");
        let backup = state.config_path.with_file_name("config.json.bak");
        assert_eq!(body["backup"], backup.display().to_string());
        assert_eq!(nexus_config::load_from_path(&backup).unwrap(), config);
        assert_eq!(
            nexus_config::load_from_path(&state.config_path).unwrap(),
            nexus_config::default_config()
        );
        assert_eq!(body["revision"], revision(&state).as_str());
    }

    #[test]
    fn validate_reports_without_saving() {
        let state = state();
        let good = serde_json::to_vec(&nexus_config::default_config()).unwrap();
        let bad = serde_json::to_vec(&config_with_overlap()).unwrap();
        let ok = json(&handle(&post("/api/config/validate", &good), &state));
        assert_eq!(
            (ok["valid"].clone(), ok["errors"].clone()),
            (true.into(), serde_json::json!([]))
        );
        let not_ok = json(&handle(&post("/api/config/validate", &bad), &state));
        assert_eq!(not_ok["valid"], false);
        assert_eq!(not_ok["ok"], true);
        assert!(!state.config_path.exists());
    }

    #[test]
    fn templates_and_themes_cover_the_library() {
        let state = state();
        let templates = json(&handle(&get("/api/templates"), &state));
        let list = templates["templates"].as_array().unwrap();
        assert_eq!(list.len(), PageTemplate::ALL.len());
        assert_eq!(list[0]["id"], "overview");
        for item in list {
            let page: nexus_config::Page = serde_json::from_value(item["page"].clone()).unwrap();
            assert_eq!(page.id, item["id"].as_str().unwrap());
        }
        let themes = json(&handle(&get("/api/themes"), &state));
        let presets = themes["presets"].as_array().unwrap();
        assert_eq!(presets.len(), nexus_render::PRESETS.len());
        let accent = presets[0]["palette"]["accent"].as_str().unwrap();
        assert!(accent.starts_with('#') && accent.len() == 7);
        assert_eq!(presets[0]["palette"]["series"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn pack_import_is_reviewed_first_and_keeps_a_backup() {
        let state = state();
        let export = handle(&get("/api/pack/export"), &state);
        assert_eq!(export.status, 200);
        assert!(export.body.starts_with(b"PK"), "zip archive");

        let review = handle(&post("/api/pack/inspect", &export.body), &state);
        let review = json(&review);
        assert_eq!(review["ok"], true, "{review}");
        assert_eq!(review["page_count"], 5);
        let reach = review["reach"].as_array().unwrap();
        assert!(
            reach.iter().any(|item| item["kind"] == "launch_app"),
            "the default launcher buttons start apps: {review}"
        );
        assert!(!state.config_path.exists(), "inspecting writes nothing");

        let digest = review["digest"].as_str().unwrap();
        assert_eq!(
            handle(&post("/api/pack/import", &export.body), &state).status,
            428
        );
        let wrong = post("/api/pack/import?reviewed=0000", &export.body);
        assert_eq!(handle(&wrong, &state).status, 428);

        let mut config = nexus_config::default_config();
        config.profiles[0].name = "Desk".into();
        assert_eq!(handle(&save_request(&config, "none"), &state).status, 200);
        let target = format!("/api/pack/import?reviewed={digest}");
        let import = handle(&post(&target, &export.body), &state);
        let body = json(&import);
        assert_eq!(import.status, 200, "{body}");
        assert_eq!(body["config"]["schema_version"], 2);
        assert_eq!(body["revision"], revision(&state).as_str());
        let backup = state.config_path.with_file_name("config.json.bak");
        assert_eq!(body["backup"], backup.display().to_string());
        assert_eq!(nexus_config::load_from_path(&backup).unwrap(), config);

        let junk = b"not a zip";
        assert_eq!(handle(&post("/api/pack/inspect", junk), &state).status, 400);
        let target = format!("/api/pack/import?reviewed={}", super::digest(junk));
        assert_eq!(handle(&post(&target, junk), &state).status, 400);
        assert_eq!(handle(&post("/api/pack/import", b""), &state).status, 400);
    }
}
