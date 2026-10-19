use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventRecord {
    pub ts_ms: u128,
    pub level: EventLevel,
    pub source: String,
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct EventLogger {
    path: PathBuf,
    source: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("failed to create event log directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to open event log {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write event log {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize event record: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to read event log {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, EventLogError>;

impl EventLogger {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            path: default_event_log_path(),
            source: source.into(),
        }
    }

    pub fn with_path(source: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn emit(
        &self,
        level: EventLevel,
        kind: impl Into<String>,
        message: impl Into<String>,
        fields: BTreeMap<String, String>,
    ) -> Result<()> {
        let record = EventRecord {
            ts_ms: unix_ms(),
            level,
            source: self.source.clone(),
            kind: kind.into(),
            message: message.into(),
            fields,
        };
        append_record(&self.path, &record)
    }

    pub fn info(&self, kind: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.emit(EventLevel::Info, kind, message, BTreeMap::new())
    }

    pub fn warn(&self, kind: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.emit(EventLevel::Warn, kind, message, BTreeMap::new())
    }

    pub fn error(&self, kind: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.emit(EventLevel::Error, kind, message, BTreeMap::new())
    }
}

pub fn default_event_log_path() -> PathBuf {
    let state_home = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    state_home.join("linexus/events.jsonl")
}

/// Size at which the log is rotated to `<name>.1` (replacing the previous
/// rotation), so a device that stays unplugged for weeks cannot grow it
/// without bound and `read_recent` stays fast.
pub const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

pub fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

pub fn append_record(path: &Path, record: &EventRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EventLogError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    // Serialize the full record + newline into one buffer, then issue a single
    // write() syscall. Concurrent loggers from the touch loop, dispatcher,
    // and IPC thread previously interleaved their JSON across multiple writes
    // and corrupted lines, which `read_recent` silently drops. POSIX
    // guarantees atomicity for an append+write under PIPE_BUF (4096 bytes),
    // and event records comfortably fit in that window.
    let mut buf = serde_json::to_vec(record)?;
    buf.push(b'\n');
    if fs::metadata(path).is_ok_and(|meta| meta.len() >= MAX_LOG_BYTES) {
        // Several processes log here; if two rotate at once the loser renames
        // a fresh, nearly empty file over the first rotation, which only costs
        // history, never a write.
        let _ = fs::rename(path, rotated_path(path));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| EventLogError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&buf)
        .map_err(|source| EventLogError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

pub fn read_recent(path: &Path, lines: usize) -> Result<Vec<EventRecord>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(EventLogError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let reader = std::io::BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|source| EventLogError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<EventRecord>(&line) {
            records.push(record);
        }
    }
    if records.len() > lines {
        Ok(records.split_off(records.len() - lines))
    } else {
        Ok(records)
    }
}

pub fn clear(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EventLogError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::File::create(path).map_err(|source| EventLogError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    match fs::remove_file(rotated_path(path)) {
        Err(source) if source.kind() != std::io::ErrorKind::NotFound => Err(EventLogError::Write {
            path: rotated_path(path),
            source,
        }),
        _ => Ok(()),
    }
}

pub fn field_map<const N: usize>(pairs: [(&str, String); N]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "linexus-events-{}-{name}.jsonl",
            std::process::id()
        ))
    }

    #[test]
    fn writes_and_reads_recent_events() {
        let path = temp_path("recent");
        let _ = fs::remove_file(&path);
        let logger = EventLogger::with_path("test", &path);
        logger.info("start", "one").unwrap();
        logger.warn("warn", "two").unwrap();
        let records = read_recent(&path, 1).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "warn");
        assert_eq!(records[0].source, "test");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rotates_once_the_log_reaches_its_size_cap() {
        let path = temp_path("rotate");
        let rotated = rotated_path(&path);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&rotated);
        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        let logger = EventLogger::with_path("test", &path);
        logger.info("after", "rotation").unwrap();
        assert_eq!(fs::metadata(&rotated).unwrap().len(), MAX_LOG_BYTES);
        let records = read_recent(&path, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "after");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&rotated);
    }

    #[test]
    fn clear_truncates_log() {
        let path = temp_path("clear");
        let _ = fs::remove_file(&path);
        let logger = EventLogger::with_path("test", &path);
        logger.info("start", "one").unwrap();
        fs::write(rotated_path(&path), b"old\n").unwrap();
        clear(&path).unwrap();
        assert!(read_recent(&path, 10).unwrap().is_empty());
        assert!(!rotated_path(&path).exists());
        let _ = fs::remove_file(&path);
    }
}
