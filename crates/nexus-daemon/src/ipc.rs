//! Local control socket for linexusctl and Studio. Requests are handed to the
//! render loop, which answers them between frames.

use crate::log::{log_fields, Level};
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{BufRead, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct ControlRequest {
    pub(crate) command: nexus_ipc::Command,
    pub(crate) reply: Sender<nexus_ipc::Response>,
}

pub(crate) struct IpcServer {
    path: PathBuf,
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| m.file_type().is_socket()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn start(
    path: PathBuf,
    control: Sender<ControlRequest>,
    logger: nexus_events::EventLogger,
) -> Result<IpcServer> {
    if let Some(parent) = path.parent() {
        let existed = parent.exists();
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket directory {}", parent.display()))?;
        if !existed {
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() {
            bail!("refusing to replace non-socket file {}", path.display());
        }
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("failed to bind daemon socket {}", path.display()))?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    log_fields(
        &logger,
        Level::Info,
        "ipc_socket_ready",
        "daemon API socket ready",
        [("path", path.display().to_string())],
    );
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let control = control.clone();
            thread::spawn(move || serve(stream, &control));
        }
    });
    Ok(IpcServer { path })
}

fn serve(mut stream: UnixStream, control: &Sender<ControlRequest>) {
    let response = match peer_uid(&stream) {
        Ok(uid) if uid == unsafe { libc::getuid() } => handle(&stream, control),
        Ok(uid) => nexus_ipc::Response::error(format!("peer uid {uid} is not allowed")),
        Err(error) => {
            nexus_ipc::Response::error(format!("peer credential check failed: {error:#}"))
        }
    };
    // One write: serde_json would otherwise issue a syscall per token, and
    // frame responses are tens of kilobytes.
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = stream.write_all(&bytes);
    }
}

fn handle(stream: &UnixStream, control: &Sender<ControlRequest>) -> nexus_ipc::Response {
    let timeout = Some(Duration::from_secs(2));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    if let Err(error) = std::io::BufReader::new(stream).read_line(&mut line) {
        return nexus_ipc::Response::error(format!("failed to read request: {error}"));
    }
    let request = match serde_json::from_str::<nexus_ipc::Request>(&line) {
        Ok(request) => request,
        Err(error) => return nexus_ipc::Response::error(format!("invalid request: {error}")),
    };
    let (reply, answer) = mpsc::channel();
    if control
        .send(ControlRequest {
            command: request.command,
            reply,
        })
        .is_err()
    {
        return nexus_ipc::Response::error("daemon is shutting down");
    }
    answer
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|_| nexus_ipc::Response::error("daemon did not answer in time"))
}

fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `cred`.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error()).context("SO_PEERCRED failed");
    }
    Ok(cred.uid)
}
