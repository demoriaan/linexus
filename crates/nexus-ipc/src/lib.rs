use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Ping,
    Status,
    Tap {
        x: u16,
    },
    ReloadConfig {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    PageNext,
    PagePrevious,
    SwitchProfile {
        profile_id: String,
    },
    SetBrightness {
        percent: u8,
    },
    Blank,
    Restore,
    /// Opens a page of the active profile by id (hidden pages included).
    GoToPage {
        page_id: String,
    },
    /// The frame currently on the strip, as base64 PNG in `Response::frame`.
    Frame,
    /// Raw touch input forwarded from Studio's mirror, so swipes and holds
    /// behave exactly as on the panel.
    Touch {
        phase: TouchPhase,
        x: u16,
    },
    /// Shows a short message over the current page (`linexusctl push`).
    Notify {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    /// Current readings of every metric, in `Response::metrics`.
    Metrics,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
}

/// One reading from the daemon's metric catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricReading {
    pub id: String,
    pub label: String,
    pub value: f32,
    /// `percent`, `celsius`, `watts`, `rpm`, `bytes`, `bytes_per_sec`,
    /// `megahertz` or `plain`.
    pub unit: String,
    pub max: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DaemonStatus>,
    /// Base64 PNG of the current frame (`Command::Frame`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Vec<MetricReading>>,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            ..Self::default()
        }
    }

    pub fn with_status(message: impl Into<String>, status: DaemonStatus) -> Self {
        Self {
            ok: true,
            message: message.into(),
            status: Some(status),
            ..Self::default()
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DaemonStatus {
    pub mode: DaemonMode,
    pub device: DeviceStatus,
    pub profile_id: String,
    pub profile_name: String,
    pub page_id: String,
    pub page_name: String,
    pub page_index: usize,
    pub page_count: usize,
    pub brightness_percent: u8,
    pub blanked: bool,
    pub manual_blanked: bool,
    pub session_blanked: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMode {
    Hardware,
    Emulator,
    #[default]
    Starting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeviceStatus {
    pub state: DeviceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DeviceStatus {
    pub fn emulator() -> Self {
        Self {
            state: DeviceState::Emulator,
            path: None,
            error: None,
        }
    }

    pub fn connected(path: impl Into<String>) -> Self {
        Self {
            state: DeviceState::Connected,
            path: Some(path.into()),
            error: None,
        }
    }

    pub fn unavailable(error: impl Into<String>) -> Self {
        Self {
            state: DeviceState::Unavailable,
            path: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Connected,
    #[default]
    Unavailable,
    Emulator,
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("failed to connect to daemon socket {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write daemon request to {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read daemon response from {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize daemon request: {0}")]
    SerializeRequest(serde_json::Error),
    #[error("failed to deserialize daemon response: {0}")]
    DeserializeResponse(serde_json::Error),
}

pub type Result<T> = std::result::Result<T, IpcError>;

pub fn default_socket_path() -> PathBuf {
    runtime_dir().join("linexus/daemon.sock")
}

/// `XDG_RUNTIME_DIR`, or else `/tmp/linexus-<uid>` when that is a private
/// directory of this user. Anyone can create that name in the shared /tmp
/// first, so a directory owned by someone else or writable by others is not
/// used; the sockets then live under `~/.cache/linexus-runtime` instead
/// (unless `HOME` is unset too).
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    let uid = current_uid();
    let shared = env::temp_dir().join(format!("linexus-{uid}"));
    if private_dir(&shared, uid) {
        return shared;
    }
    match env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".cache/linexus-runtime"),
        None => shared,
    }
}

/// Creates `path` as a 0700 directory, or checks that the existing one is a
/// real directory owned by `uid` that nobody else can write to. Read access
/// for others, as older versions left it, is taken away.
fn private_dir(path: &Path, uid: u32) -> bool {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => return true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return false,
    }
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o022 != 0 {
        return false;
    }
    meta.mode() & 0o077 == 0 || fs::set_permissions(path, fs::Permissions::from_mode(0o700)).is_ok()
}

pub fn send_request(path: &Path, command: Command) -> Result<Response> {
    let mut stream = UnixStream::connect(path).map_err(|source| IpcError::Connect {
        path: path.to_path_buf(),
        source,
    })?;
    let timeout = Some(Duration::from_secs(3));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = Request { command };
    serde_json::to_writer(&mut stream, &request).map_err(IpcError::SerializeRequest)?;
    stream.write_all(b"\n").map_err(|source| IpcError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut line = String::new();
    let mut reader = std::io::BufReader::new(stream);
    reader
        .read_line(&mut line)
        .map_err(|source| IpcError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let response = serde_json::from_str(&line).map_err(IpcError::DeserializeResponse)?;
    Ok(response)
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrip_json() {
        let json = serde_json::to_string(&Request {
            command: Command::Tap { x: 570 },
        })
        .unwrap();
        assert!(json.contains("tap"));
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.command, Command::Tap { x: 570 });

        let reload = serde_json::to_string(&Request {
            command: Command::ReloadConfig {
                path: Some("/tmp/config.json".into()),
            },
        })
        .unwrap();
        assert!(reload.contains("reload_config"));

        let brightness = serde_json::to_string(&Request {
            command: Command::SetBrightness { percent: 42 },
        })
        .unwrap();
        assert!(brightness.contains("set_brightness"));
        let decoded: Request = serde_json::from_str(&brightness).unwrap();
        assert_eq!(decoded.command, Command::SetBrightness { percent: 42 });

        let switch = serde_json::to_string(&Request {
            command: Command::SwitchProfile {
                profile_id: "media".into(),
            },
        })
        .unwrap();
        assert!(switch.contains("switch_profile"));
        let decoded: Request = serde_json::from_str(&switch).unwrap();
        assert_eq!(
            decoded.command,
            Command::SwitchProfile {
                profile_id: "media".into()
            }
        );
    }

    #[test]
    fn shared_runtime_dir_must_be_private_and_owned() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let chmod = |path: &Path, mode| {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap()
        };
        let base = env::temp_dir().join(format!("nexus-ipc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let uid = current_uid();

        let dir = base.join("runtime");
        assert!(private_dir(&dir, uid));
        assert_eq!(mode(&dir), 0o700);
        // Older versions left the directory readable for others.
        chmod(&dir, 0o755);
        assert!(private_dir(&dir, uid));
        assert_eq!(mode(&dir), 0o700);
        // Writable for others, someone else's, or a symlink: not used.
        chmod(&dir, 0o777);
        assert!(!private_dir(&dir, uid));
        chmod(&dir, 0o700);
        assert!(!private_dir(&dir, uid.wrapping_add(1)));
        let link = base.join("link");
        symlink(&dir, &link).unwrap();
        assert!(!private_dir(&link, uid));
        let file = base.join("file");
        fs::write(&file, b"").unwrap();
        assert!(!private_dir(&file, uid));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn status_response_serializes_snake_case_mode() {
        let status = DaemonStatus {
            mode: DaemonMode::Emulator,
            device: DeviceStatus::emulator(),
            profile_id: "default".into(),
            profile_name: "Default".into(),
            page_id: "status".into(),
            page_name: "Status".into(),
            page_index: 0,
            page_count: 1,
            brightness_percent: 80,
            blanked: false,
            manual_blanked: false,
            session_blanked: false,
        };
        let json = serde_json::to_string(&Response::with_status("ok", status)).unwrap();
        assert!(json.contains("emulator"));
        assert!(json.contains("page_id"));
        assert!(json.contains("brightness_percent"));
    }
}
