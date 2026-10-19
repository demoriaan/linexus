//! Frontend files: embedded at build time (see `build.rs`), or read from
//! disk when `NEXUS_STUDIO_ASSETS` points at `apps/nexus-studio`, so
//! frontend edits show up on reload without rebuilding the binary.

use crate::http::{percent_decode, Request, Response};
use crate::mime;
use crate::state::AppState;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub bytes: &'static [u8],
}

// `ASSETS: &[Asset]`, sorted by path, and `ASSETS_ETAG`, which changes
// whenever any of them does.
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub const ENV_OVERRIDE: &str = "NEXUS_STUDIO_ASSETS";
/// Browsers revalidate every file. Embedded files answer with 304 while the
/// build is the same, so this costs a round trip but never serves modules
/// from an older Studio after an update.
const REVALIDATE: &str = "no-cache";

pub enum Source {
    Embedded,
    /// Canonical root directory of the frontend checkout.
    Disk(PathBuf),
}

impl Source {
    /// Disk mode when `NEXUS_STUDIO_ASSETS` names a directory; anything else
    /// falls back to the embedded files with a warning.
    pub fn from_env() -> Self {
        let Some(dir) = std::env::var_os(ENV_OVERRIDE).filter(|dir| !dir.is_empty()) else {
            return Self::Embedded;
        };
        match Path::new(&dir).canonicalize() {
            Ok(root) if root.is_dir() => Self::Disk(root),
            _ => {
                eprintln!(
                    "{ENV_OVERRIDE}={} is not a directory; serving the embedded UI",
                    Path::new(&dir).display()
                );
                Self::Embedded
            }
        }
    }

    fn load(&self, rel: &str) -> Option<(Cow<'static, [u8]>, &'static str)> {
        match self {
            Self::Embedded => embedded(rel).map(|asset| (Cow::Borrowed(asset.bytes), asset.mime)),
            Self::Disk(root) => {
                read_below(root, rel).map(|bytes| (Cow::Owned(bytes), mime::for_path(rel)))
            }
        }
    }

    /// Validator for conditional requests; disk files have none.
    fn etag(&self) -> Option<&'static str> {
        match self {
            Self::Embedded => Some(ASSETS_ETAG),
            Self::Disk(_) => None,
        }
    }
}

/// Embedded file by its path relative to `apps/nexus-studio/`.
pub fn embedded(rel: &str) -> Option<&'static Asset> {
    ASSETS
        .binary_search_by(|asset| asset.path.cmp(rel))
        .ok()
        .map(|index| &ASSETS[index])
}

/// Answers a GET for anything outside `/api/`.
pub fn serve(state: &AppState, request: &Request) -> Response {
    let path = request.path.as_str();
    if path == "/" || path == "/index.html" {
        return index(state);
    }
    let etag = state.assets.etag();
    let fresh = |etag: &str| request.header("if-none-match") == Some(etag);
    if let Some(font) = font(path) {
        // Fonts come from the renderer and change only with the binary.
        if fresh(ASSETS_ETAG) {
            return not_modified(ASSETS_ETAG);
        }
        return Response::bytes(200, "font/ttf", font.to_vec())
            .with_cache(REVALIDATE)
            .with_header("ETag", ASSETS_ETAG);
    }
    let Some(rel) = sanitize(path) else {
        return Response::not_found();
    };
    if let Some(etag) = etag.filter(|etag| fresh(etag)) {
        if state.assets.load(&rel).is_some() {
            return not_modified(etag);
        }
    }
    match state.assets.load(&rel) {
        Some((bytes, mime)) => {
            let response = Response::bytes(200, mime, bytes.into_owned()).with_cache(REVALIDATE);
            match etag {
                Some(etag) => response.with_header("ETag", etag),
                None => response,
            }
        }
        None => Response::not_found(),
    }
}

fn not_modified(etag: &str) -> Response {
    Response::bytes(304, "text/plain", Vec::new())
        .with_cache(REVALIDATE)
        .with_header("ETag", etag)
}

/// `index.html` with the write token. Never cached: the token changes with
/// every server start and a stale page would get 403 on every save.
fn index(state: &AppState) -> Response {
    match state.assets.load("index.html") {
        Some((html, _)) => Response::bytes(
            200,
            "text/html; charset=utf-8",
            inject_token(&html, &state.token),
        ),
        None => Response::error(404, "index.html is missing from the Studio frontend"),
    }
}

/// Inserts `<meta name="nexus-token">` before `</head>` (or before `<body`
/// when a page has no head end tag), so the UI needs no extra round trip.
pub fn inject_token(html: &[u8], token: &str) -> Vec<u8> {
    let escaped = token
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let meta = format!("<meta name=\"nexus-token\" content=\"{escaped}\">");
    let lower = html.to_ascii_lowercase();
    let find = |needle: &[u8]| lower.windows(needle.len()).position(|w| w == needle);
    let at = find(b"</head>").or_else(|| find(b"<body")).unwrap_or(0);
    let mut out = Vec::with_capacity(html.len() + meta.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(meta.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

/// The strip's own typeface, served from the renderer's bytes so the editor
/// and the device can never drift apart.
fn font(path: &str) -> Option<&'static [u8]> {
    use nexus_render::text::{BOLD_TTF, MEDIUM_TTF, SEMIBOLD_TTF};
    match path {
        "/fonts/barlow-medium.ttf" => Some(MEDIUM_TTF),
        "/fonts/barlow-semibold.ttf" => Some(SEMIBOLD_TTF),
        "/fonts/barlow-bold.ttf" => Some(BOLD_TTF),
        _ => None,
    }
}

/// Turns a URL path into a relative file path, or `None` when it could
/// climb out of the asset root or names a hidden file: after decoding,
/// every segment must be non-empty, must not start with `.` (which also
/// rules out `.` and `..`) and must not contain a backslash or NUL.
fn sanitize(path: &str) -> Option<String> {
    let decoded = String::from_utf8(percent_decode(path, false)).ok()?;
    let rel = decoded.strip_prefix('/')?;
    let safe = !rel.is_empty()
        && rel.split('/').all(|segment| {
            !segment.is_empty() && !segment.starts_with('.') && !segment.contains(['\\', '\0'])
        });
    safe.then(|| rel.to_string())
}

/// Reads `rel` below the canonical `root`, refusing anything whose real
/// path ends up outside it (a symlink pointing elsewhere, for example).
fn read_below(root: &Path, rel: &str) -> Option<Vec<u8>> {
    let path = root.join(rel).canonicalize().ok()?;
    if !path.starts_with(root) || !path.is_file() {
        return None;
    }
    std::fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::{get, state, temp_dir};

    #[test]
    fn embedded_table_contains_the_frontend_index() {
        let index = embedded("index.html").expect("apps/nexus-studio/index.html is embedded");
        assert_eq!(index.mime, "text/html; charset=utf-8");
        assert!(!index.bytes.is_empty());
        assert!(embedded("does-not-exist.js").is_none());
        assert!(ASSETS.windows(2).all(|pair| pair[0].path < pair[1].path));
    }

    #[test]
    fn root_serves_index_with_token_and_no_cache() {
        let state = state();
        let response = serve(&state, &get("/"));
        assert_eq!(response.status, 200);
        assert_eq!(response.cache_control, "no-store");
        let html = String::from_utf8(response.body).unwrap();
        assert!(html.contains(r#"<meta name="nexus-token" content="test-token">"#));
        assert_eq!(serve(&state, &get("/index.html")).status, 200);
    }

    #[test]
    fn token_lands_before_head_end_or_body() {
        let with_head = inject_token(b"<html><HEAD><title>x</title></HEAD><body></body>", "t");
        assert_eq!(
            String::from_utf8(with_head).unwrap(),
            r#"<html><HEAD><title>x</title><meta name="nexus-token" content="t"></HEAD><body></body>"#
        );
        let body_only = inject_token(b"<!doctype html><body>hi</body>", "t");
        assert!(String::from_utf8(body_only)
            .unwrap()
            .starts_with(r#"<!doctype html><meta name="nexus-token""#));
    }

    #[test]
    fn unknown_static_paths_are_404() {
        let state = state();
        assert_eq!(serve(&state, &get("/nope.js")).status, 404);
        assert_eq!(serve(&state, &get("/sub/")).status, 404);
    }

    #[test]
    fn embedded_files_revalidate_with_the_build_etag() {
        let state = state();
        let first = serve(&state, &get("/index.html"));
        assert_eq!(first.status, 200);
        let path = ASSETS
            .iter()
            .find(|asset| asset.path.ends_with(".js"))
            .map(|asset| format!("/{}", asset.path))
            .expect("the UI ships scripts");
        let full = serve(&state, &get(&path));
        assert_eq!(full.status, 200);
        assert!(full
            .extra_headers
            .contains(&("ETag", ASSETS_ETAG.to_string())));
        let mut again = get(&path);
        again
            .headers
            .push(("if-none-match".into(), ASSETS_ETAG.into()));
        let cached = serve(&state, &again);
        assert_eq!(cached.status, 304);
        assert!(cached.body.is_empty());
        let mut stale = get(&path);
        stale
            .headers
            .push(("if-none-match".into(), "\"old\"".into()));
        assert_eq!(serve(&state, &stale).status, 200);
    }

    #[test]
    fn fonts_come_from_the_renderer() {
        let response = serve(&state(), &get("/fonts/barlow-bold.ttf"));
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "font/ttf");
        assert_eq!(response.body, nexus_render::text::BOLD_TTF);
        assert_eq!(response.cache_control, REVALIDATE);
    }

    #[test]
    fn sanitize_rejects_traversal_and_hidden_files() {
        assert_eq!(sanitize("/app.js").as_deref(), Some("app.js"));
        assert_eq!(sanitize("/img/a%20b.png").as_deref(), Some("img/a b.png"));
        for bad in [
            "/../Cargo.toml",
            "/%2e%2e/secret",
            "/img/../../secret",
            "/./app.js",
            "/.git/config",
            "//etc/passwd",
            "/a%5c..%5csecret",
            "/a%00b",
            "/%ff",
            "/",
        ] {
            assert_eq!(sanitize(bad), None, "{bad}");
        }
    }

    #[test]
    fn disk_override_serves_files_but_never_leaves_its_root() {
        let base = temp_dir("assets");
        let root = base.join("app");
        std::fs::create_dir_all(root.join("css")).unwrap();
        std::fs::write(root.join("index.html"), "<head></head>disk").unwrap();
        std::fs::write(root.join("css/site.css"), "body{}").unwrap();
        std::fs::write(base.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), root.join("leak.txt")).unwrap();

        let mut state = state();
        state.assets = Source::Disk(root.canonicalize().unwrap());
        let css = serve(&state, &get("/css/site.css"));
        assert_eq!(css.status, 200);
        assert_eq!(css.content_type, "text/css; charset=utf-8");
        assert_eq!(css.cache_control, REVALIDATE);
        assert_eq!(css.body, b"body{}");
        let index = String::from_utf8(serve(&state, &get("/")).body).unwrap();
        assert!(index.contains("disk") && index.contains("nexus-token"));
        for bad in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/css/%2E%2E/../secret.txt",
            "/leak.txt",
        ] {
            assert_eq!(serve(&state, &get(bad)).status, 404, "{bad}");
        }
    }
}
