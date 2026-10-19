//! Everything a request handler needs, shared by all connection threads.

use crate::api::{catalog::CatalogCache, preview::MetricsCache};
use crate::assets::Source;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct AppState {
    pub config_path: PathBuf,
    /// Held while a write checks the config revision and replaces the file,
    /// so two tabs cannot both pass the check and overwrite each other.
    pub config_lock: Mutex<()>,
    pub socket_path: PathBuf,
    /// Per-run write token, injected into `index.html`.
    pub token: String,
    /// Port we listen on; the Host check compares against it.
    pub port: u16,
    pub assets: Source,
    pub metrics: MetricsCache,
    pub catalog: CatalogCache,
}

impl AppState {
    pub fn new(config_path: PathBuf, socket_path: PathBuf, port: u16, assets: Source) -> Self {
        Self {
            config_path,
            config_lock: Mutex::new(()),
            socket_path,
            token: new_session_token(),
            port,
            assets,
            metrics: MetricsCache::default(),
            catalog: CatalogCache::default(),
        }
    }
}

/// A fresh random token per server run. The kernel's UUID generator is
/// random (v4); the time-based fallback only exists for exotic systems
/// without procfs and is still unguessable for a cross-site page that
/// cannot read our responses.
fn new_session_token() -> String {
    let from_kernel = std::fs::read_to_string("/proc/sys/kernel/random/uuid").ok();
    let token: String = from_kernel
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit() || *ch == '-')
        .collect();
    if token.len() >= 32 {
        return token;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:x}-{nanos:x}", std::process::id())
}

#[cfg(test)]
pub mod testing {
    //! Builders for handler tests: an isolated state and well-formed
    //! requests that pass the Host and token checks.

    use super::*;
    use crate::http::Request;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub const PORT: u16 = 19777;

    /// A new empty directory below the system temp dir.
    pub fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nexus-studio-test-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// State with a config path in a fresh temp dir and no daemon socket.
    pub fn state() -> AppState {
        let dir = temp_dir("state");
        let mut state = AppState::new(
            dir.join("config.json"),
            dir.join("no-daemon.sock"),
            PORT,
            Source::Embedded,
        );
        state.token = "test-token".into();
        state
    }

    pub fn request(method: &str, target: &str, body: &[u8]) -> Request {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        Request {
            method: method.into(),
            path: path.into(),
            query: query.into(),
            headers: vec![
                ("host".into(), format!("127.0.0.1:{PORT}")),
                ("x-nexus-studio".into(), "1".into()),
                ("x-nexus-studio-token".into(), "test-token".into()),
            ],
            body: body.to_vec(),
        }
    }

    pub fn get(target: &str) -> Request {
        request("GET", target, b"")
    }

    pub fn post(target: &str, body: &[u8]) -> Request {
        request("POST", target, body)
    }

    pub fn json(response: &crate::http::Response) -> serde_json::Value {
        serde_json::from_slice(&response.body).expect("response body is JSON")
    }
}
