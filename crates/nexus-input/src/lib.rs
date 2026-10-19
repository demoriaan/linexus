use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub command: InputCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum InputCommand {
    KeyCombo { keys: Vec<String> },
    Text { text: String },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub ok: bool,
    pub message: String,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InputIpcError {
    #[error("failed to connect to input helper socket {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write input helper request to {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read input helper response from {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize input helper request: {0}")]
    SerializeRequest(serde_json::Error),
    #[error("failed to deserialize input helper response: {0}")]
    DeserializeResponse(serde_json::Error),
    #[error("input helper rejected request: {0}")]
    Rejected(String),
}

pub type Result<T> = std::result::Result<T, InputIpcError>;

pub fn default_socket_path() -> PathBuf {
    user_socket_path().unwrap_or_else(system_socket_path)
}

pub fn system_socket_path() -> PathBuf {
    PathBuf::from("/run/linexus/inputd.sock")
}

pub fn user_socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|runtime| runtime.join("linexus/inputd.sock"))
}

pub fn candidate_socket_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = user_socket_path() {
        paths.push(path);
    }
    let system = system_socket_path();
    if !paths.iter().any(|path| path == &system) {
        paths.push(system);
    }
    paths
}

pub fn send_request(path: &Path, command: InputCommand) -> Result<Response> {
    let mut stream = UnixStream::connect(path).map_err(|source| InputIpcError::Connect {
        path: path.to_path_buf(),
        source,
    })?;
    let timeout = Some(Duration::from_secs(3));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = Request { command };
    serde_json::to_writer(&mut stream, &request).map_err(InputIpcError::SerializeRequest)?;
    stream
        .write_all(b"\n")
        .map_err(|source| InputIpcError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut line = String::new();
    let mut reader = std::io::BufReader::new(stream);
    reader
        .read_line(&mut line)
        .map_err(|source| InputIpcError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let response: Response =
        serde_json::from_str(&line).map_err(InputIpcError::DeserializeResponse)?;
    if response.ok {
        Ok(response)
    } else {
        Err(InputIpcError::Rejected(response.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_key_combo_request() {
        let json = serde_json::to_string(&Request {
            command: InputCommand::KeyCombo {
                keys: vec!["ctrl".into(), "l".into()],
            },
        })
        .unwrap();
        assert!(json.contains("key_combo"));
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(
            decoded.command,
            InputCommand::KeyCombo {
                keys: vec!["ctrl".into(), "l".into()]
            }
        );
    }
}
