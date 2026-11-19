//! Cameras held open: any process with `/dev/videoN` among its open
//! files, which also finds apps reading a v4l2loopback camera such as
//! Droidcam without PipeWire in between.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// PipeWire's own processes open cameras for the apps they serve; those
/// apps show through PipeWire's links instead.
const SKIP_PROGRAMS: [&str; 3] = ["pipewire", "wireplumber", "pipewire-pulse"];

/// A process with a camera open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraOpen {
    pub pid: u32,
    /// Its program name from `comm`.
    pub program: String,
    /// `/dev/videoN`.
    pub path: String,
    /// The camera's name from sysfs, or the device path.
    pub name: String,
}

/// Whether there is a camera at all; without one the scan is skipped.
pub fn camera_present(dev: &Path) -> bool {
    fs::read_dir(dev).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("video"))
    })
}

/// Every process under `proc_root` that has a camera open, except those in
/// `skip`. Processes of other users cannot be looked into and are missed.
pub fn camera_users(proc_root: &Path, sys_root: &Path, skip: &[u32]) -> Vec<CameraOpen> {
    let Ok(entries) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if skip.contains(&pid) {
            continue;
        }
        let dir = entry.path();
        let Ok(fds) = fs::read_dir(dir.join("fd")) else {
            continue;
        };
        let mut paths: Vec<String> = fds
            .flatten()
            .filter_map(|fd| fs::read_link(fd.path()).ok())
            .filter_map(|target| target.to_str().map(str::to_string))
            .filter(|target| is_camera(target))
            .collect();
        if paths.is_empty() {
            continue;
        }
        let program = fs::read_to_string(dir.join("comm"))
            .map(|comm| comm.trim().to_string())
            .unwrap_or_default();
        if SKIP_PROGRAMS.contains(&program.as_str()) {
            continue;
        }
        paths.sort();
        paths.dedup();
        for path in paths {
            let node = path.trim_start_matches("/dev/");
            let name =
                fs::read_to_string(sys_root.join("class/video4linux").join(node).join("name"))
                    .map(|name| name.trim().to_string())
                    .ok()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| path.clone());
            found.push(CameraOpen {
                pid,
                program: program.clone(),
                path,
                name,
            });
        }
    }
    found.sort_by_key(|open| open.pid);
    found
}

fn is_camera(target: &str) -> bool {
    target.strip_prefix("/dev/").is_some_and(is_camera_name)
}

fn is_camera_name(name: &str) -> bool {
    name.strip_prefix("video")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Tells when a camera is opened or closed, or one comes or goes, so the
/// scan of every process's open files runs only then. Uses inotify on the
/// camera device files and their directory.
pub struct CameraWatch {
    fd: OwnedFd,
    dev: PathBuf,
    dev_watch: i32,
    /// Every camera could be watched.
    complete: bool,
}

impl CameraWatch {
    pub fn new(dev: &Path) -> io::Result<Self> {
        // SAFETY: takes flags only; the result is checked below.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a new descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut watch = Self {
            fd,
            dev: dev.to_path_buf(),
            dev_watch: -1,
            complete: false,
        };
        watch.dev_watch = watch.add(dev, libc::IN_CREATE | libc::IN_DELETE)?;
        watch.watch_cameras();
        Ok(watch)
    }

    fn add(&self, path: &Path, mask: u32) -> io::Result<i32> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: the descriptor is open and `path` is NUL-terminated.
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), path.as_ptr(), mask) };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(wd)
    }

    /// Watches every camera there is now. Watching one again keeps its
    /// watch, so this runs after every change.
    fn watch_cameras(&mut self) {
        let mut complete = true;
        if let Ok(entries) = fs::read_dir(&self.dev) {
            for entry in entries.flatten() {
                if is_camera_name(&entry.file_name().to_string_lossy()) {
                    complete &= self
                        .add(&entry.path(), libc::IN_OPEN | libc::IN_CLOSE)
                        .is_ok();
                }
            }
        }
        self.complete = complete;
    }

    /// Whether every camera could be watched. A camera the user may not
    /// read cannot be; then a timer has to look for it.
    pub fn complete(&self) -> bool {
        self.complete
    }

    /// Whether a camera was opened or closed, or one came or went, since
    /// the last call.
    pub fn changed(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let mut changed = false;
        loop {
            // SAFETY: reads at most `buf.len()` bytes into `buf`.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            let Ok(n) = usize::try_from(n) else { break };
            if n == 0 {
                break;
            }
            changed |=
                events(&buf[..n]).any(|(wd, name)| wd != self.dev_watch || is_camera_name(name));
        }
        if changed {
            self.watch_cameras();
        }
        changed
    }
}

/// The watch and file name of each `inotify_event` in `bytes`.
fn events(bytes: &[u8]) -> impl Iterator<Item = (i32, &str)> {
    const HEAD: usize = 16;
    let mut at = 0;
    std::iter::from_fn(move || {
        let head = bytes.get(at..at + HEAD)?;
        let wd = i32::from_ne_bytes(head[0..4].try_into().ok()?);
        let len = u32::from_ne_bytes(head[12..16].try_into().ok()?) as usize;
        let name = bytes.get(at + HEAD..at + HEAD + len)?;
        at += HEAD + len;
        let name = std::str::from_utf8(name)
            .unwrap_or("")
            .trim_end_matches('\0');
        Some((wd, name))
    })
}
