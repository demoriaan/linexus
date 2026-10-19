use anyhow::{bail, Context, Result};
use clap::Parser;
use nexus_input::{InputCommand, Request, Response};
use std::fs;
use std::io::{BufRead, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "linexus-inputd",
    version,
    about = "Privileged uinput helper for iCUE Nexus keyboard/text actions"
)]
struct Args {
    /// Override helper Unix socket path.
    #[arg(long)]
    socket_path: Option<PathBuf>,
    /// UID allowed to send input requests. Defaults to SUDO_UID or the current UID.
    #[arg(long)]
    allow_uid: Option<u32>,
    /// Do not open /dev/uinput; validate requests and print them only.
    #[arg(long)]
    dry_run: bool,
    /// Exit after one accepted connection. Useful for tests.
    #[arg(long)]
    once: bool,
    /// Allow injection while the current session appears locked. Default is deny when detected.
    #[arg(long)]
    allow_locked: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let socket_path = args
        .socket_path
        .clone()
        .unwrap_or_else(nexus_input::default_socket_path);
    let allow_uid = args.allow_uid.unwrap_or_else(default_allowed_uid);
    let backend = if args.dry_run {
        InputBackend::DryRun
    } else {
        InputBackend::Real(UinputKeyboard::create().context("failed to create uinput keyboard")?)
    };
    run_server(
        socket_path,
        allow_uid,
        args.allow_locked,
        args.once,
        Arc::new(Mutex::new(backend)),
    )
}

fn run_server(
    path: PathBuf,
    allow_uid: u32,
    allow_locked: bool,
    once: bool,
    backend: Arc<Mutex<InputBackend>>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        let existed = parent.exists();
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket directory {}", parent.display()))?;
        if !existed {
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
        }
    }
    if path.exists() {
        let meta = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect socket path {}", path.display()))?;
        if meta.file_type().is_socket() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
        } else {
            bail!(
                "refusing to remove non-socket file at input helper socket path {}",
                path.display()
            );
        }
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("failed to bind input helper socket {}", path.display()))?;
    secure_socket_for_uid(&path, allow_uid)
        .with_context(|| format!("failed to secure input helper socket {}", path.display()))?;
    let _guard = SocketCleanup { path: path.clone() };
    println!(
        "input helper socket: {} (allow_uid={}, dry_run={})",
        path.display(),
        allow_uid,
        matches!(
            *backend.lock().expect("backend lock poisoned"),
            InputBackend::DryRun
        )
    );

    if once {
        let (stream, _) = listener.accept()?;
        handle_stream(stream, allow_uid, allow_locked, &backend);
        return Ok(());
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let backend = backend.clone();
                thread::spawn(move || handle_stream(stream, allow_uid, allow_locked, &backend));
            }
            Err(error) => eprintln!("input helper accept failed: {error}"),
        }
    }
    Ok(())
}

fn secure_socket_for_uid(path: &std::path::Path, allow_uid: u32) -> Result<()> {
    if current_uid() == 0 {
        let rc = unsafe { libc::chown(path_to_cstring(path)?.as_ptr(), allow_uid, u32::MAX) };
        if rc < 0 {
            return Err(std::io::Error::last_os_error()).context("failed to chown socket");
        }
    } else if allow_uid != current_uid() {
        bail!(
            "non-root input helper cannot create a private socket for uid {}",
            allow_uid
        );
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("failed to chmod socket 0600")
}

fn path_to_cstring(path: &std::path::Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).context("socket path contains NUL byte")
}

fn handle_stream(
    mut stream: UnixStream,
    allow_uid: u32,
    allow_locked: bool,
    backend: &Arc<Mutex<InputBackend>>,
) {
    let response = match peer_uid(&stream) {
        Ok(uid) if uid == allow_uid => {
            handle_authorized_stream(&mut stream, allow_uid, allow_locked, backend)
        }
        Ok(uid) => Response::error(format!(
            "unauthorized uid {uid}; allowed uid is {allow_uid}"
        )),
        Err(error) => Response::error(format!("failed to inspect peer credentials: {error}")),
    };
    if let Err(error) = serde_json::to_writer(&mut stream, &response)
        .and_then(|_| stream.write_all(b"\n").map_err(serde_json::Error::io))
    {
        eprintln!("input helper response write failed: {error}");
    }
}

fn handle_authorized_stream(
    stream: &mut UnixStream,
    allow_uid: u32,
    allow_locked: bool,
    backend: &Arc<Mutex<InputBackend>>,
) -> Response {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let read = {
        let mut reader = std::io::BufReader::new(&mut *stream);
        reader.read_line(&mut line)
    };
    let request = match read {
        Ok(0) => return Response::error("empty input helper request"),
        Ok(_) => match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => return Response::error(format!("invalid input helper request: {error}")),
        },
        Err(error) => {
            return Response::error(format!("failed to read input helper request: {error}"))
        }
    };
    if !allow_locked {
        match screen_locked_for_uid(allow_uid) {
            Ok(true) => {
                return Response::error("screen/session appears locked; refusing input injection")
            }
            Ok(false) => {}
            Err(error) => {
                return Response::error(format!(
                    "could not prove session is unlocked; refusing input injection: {error:#}"
                ))
            }
        }
    }
    let mut backend = match backend.lock() {
        Ok(backend) => backend,
        Err(_) => return Response::error("input backend lock poisoned"),
    };
    match backend.execute(request.command) {
        Ok(message) => Response::ok(message),
        Err(error) => Response::error(error.to_string()),
    }
}

#[derive(Debug)]
enum InputBackend {
    DryRun,
    Real(UinputKeyboard),
}

impl InputBackend {
    fn execute(&mut self, command: InputCommand) -> Result<String> {
        match command {
            InputCommand::Ping => Ok("pong".into()),
            InputCommand::KeyCombo { keys } => {
                let sequence = parse_key_combo(&keys)?;
                match self {
                    Self::DryRun => Ok(format!("dry-run key combo: {}", keys.join("+"))),
                    Self::Real(device) => {
                        device.key_combo(&sequence)?;
                        Ok(format!("sent key combo: {}", keys.join("+")))
                    }
                }
            }
            InputCommand::Text { text } => {
                let sequence = parse_text(&text)?;
                match self {
                    Self::DryRun => Ok(format!("dry-run text: {} chars", text.chars().count())),
                    Self::Real(device) => {
                        device.text(&sequence)?;
                        Ok(format!("sent text: {} chars", text.chars().count()))
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyStroke {
    code: u16,
    shift: bool,
}

fn parse_key_combo(keys: &[String]) -> Result<Vec<u16>> {
    if keys.is_empty() || keys.len() > 6 {
        bail!("key combo must contain 1..6 keys");
    }
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        out.push(key_name_to_code(key).with_context(|| format!("unsupported key '{key}'"))?);
    }
    Ok(out)
}

fn parse_text(text: &str) -> Result<Vec<KeyStroke>> {
    if text.chars().count() > 256 {
        bail!("text input is limited to 256 characters per request");
    }
    text.chars()
        .map(|ch| char_to_key(ch).with_context(|| format!("unsupported text character {ch:?}")))
        .collect()
}

fn key_name_to_code(name: &str) -> Option<u16> {
    let normalized = name.trim().to_ascii_lowercase().replace('_', "-");
    let code = match normalized.as_str() {
        "ctrl" | "control" | "leftctrl" | "left-control" => 29,
        "shift" | "leftshift" | "left-shift" => 42,
        "alt" | "leftalt" | "left-alt" => 56,
        "meta" | "super" | "win" | "leftmeta" | "left-meta" => 125,
        "rightctrl" | "right-control" => 97,
        "rightshift" | "right-shift" => 54,
        "rightalt" | "right-alt" | "altgr" => 100,
        "rightmeta" | "right-meta" => 126,
        "enter" | "return" => 28,
        "esc" | "escape" => 1,
        "tab" => 15,
        "space" => 57,
        "backspace" => 14,
        "delete" | "del" => 111,
        "home" => 102,
        "end" => 107,
        "pageup" | "page-up" => 104,
        "pagedown" | "page-down" => 109,
        "left" => 105,
        "right" => 106,
        "up" => 103,
        "down" => 108,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        one if one.len() == 1 => char_to_key(one.chars().next()?)?.code,
        _ => return None,
    };
    Some(code)
}

fn char_to_key(ch: char) -> Option<KeyStroke> {
    let (code, shift) = match ch {
        'a'..='z' => (letter_key_code(ch)?, false),
        'A'..='Z' => (letter_key_code(ch.to_ascii_lowercase())?, true),
        '1' => (2, false),
        '2' => (3, false),
        '3' => (4, false),
        '4' => (5, false),
        '5' => (6, false),
        '6' => (7, false),
        '7' => (8, false),
        '8' => (9, false),
        '9' => (10, false),
        '0' => (11, false),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        '\\' => (43, false),
        '|' => (43, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        _ => return None,
    };
    Some(KeyStroke { code, shift })
}

fn letter_key_code(ch: char) -> Option<u16> {
    Some(match ch {
        'a' => 30,
        'b' => 48,
        'c' => 46,
        'd' => 32,
        'e' => 18,
        'f' => 33,
        'g' => 34,
        'h' => 35,
        'i' => 23,
        'j' => 36,
        'k' => 37,
        'l' => 38,
        'm' => 50,
        'n' => 49,
        'o' => 24,
        'p' => 25,
        'q' => 16,
        'r' => 19,
        's' => 31,
        't' => 20,
        'u' => 22,
        'v' => 47,
        'w' => 17,
        'x' => 45,
        'y' => 21,
        'z' => 44,
        _ => return None,
    })
}

#[derive(Debug)]
struct UinputKeyboard {
    file: fs::File,
}

impl UinputKeyboard {
    fn create() -> Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .context("open /dev/uinput")?;
        let fd = file.as_raw_fd();
        ioctl_int(fd, UI_SET_EVBIT, EV_KEY)?;
        ioctl_int(fd, UI_SET_EVBIT, EV_SYN)?;
        for code in advertised_key_codes() {
            ioctl_int(fd, UI_SET_KEYBIT, i32::from(code))?;
        }
        let mut setup = UinputSetup::default();
        setup.id.bustype = BUS_USB;
        setup.id.vendor = 0x1b1c;
        setup.id.product = 0x1b8e;
        setup.id.version = 1;
        let name = b"iCUE Nexus Linux Input Helper\0";
        setup.name[..name.len()].copy_from_slice(name);
        unsafe {
            ioctl_ptr(
                fd,
                UI_DEV_SETUP,
                &setup as *const UinputSetup as *const libc::c_void,
            )?
        };
        unsafe { ioctl_noarg(fd, UI_DEV_CREATE)? };
        thread::sleep(Duration::from_millis(100));
        Ok(Self { file })
    }

    fn key_combo(&mut self, keys: &[u16]) -> Result<()> {
        for code in keys {
            self.key(*code, 1)?;
        }
        self.sync()?;
        for code in keys.iter().rev() {
            self.key(*code, 0)?;
        }
        self.sync()?;
        self.release_modifiers();
        Ok(())
    }

    fn text(&mut self, text: &[KeyStroke]) -> Result<()> {
        for stroke in text {
            if stroke.shift {
                self.key(42, 1)?;
            }
            self.key(stroke.code, 1)?;
            self.key(stroke.code, 0)?;
            if stroke.shift {
                self.key(42, 0)?;
            }
            self.sync()?;
        }
        self.release_modifiers();
        Ok(())
    }

    fn key(&mut self, code: u16, value: i32) -> Result<()> {
        let event = InputEvent::new(EV_KEY as u16, code, value);
        write_event(&mut self.file, &event)
    }

    fn sync(&mut self) -> Result<()> {
        let event = InputEvent::new(EV_SYN as u16, SYN_REPORT as u16, 0);
        write_event(&mut self.file, &event)
    }

    fn release_modifiers(&mut self) {
        for code in [29, 42, 56, 125, 97, 54, 100, 126] {
            let _ = self.key(code, 0);
        }
        let _ = self.sync();
    }
}

impl Drop for UinputKeyboard {
    fn drop(&mut self) {
        self.release_modifiers();
        unsafe {
            let _ = libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as libc::c_ulong);
        }
    }
}

fn advertised_key_codes() -> Vec<u16> {
    let mut codes = vec![
        1, 14, 15, 28, 29, 42, 43, 54, 56, 57, 97, 100, 102, 103, 104, 105, 106, 107, 108, 109,
        111, 125, 126,
    ];
    codes.extend(2..=13);
    codes.extend(16..=27);
    codes.extend(30..=40);
    codes.extend(44..=53);
    codes.extend(59..=68);
    codes.extend([87, 88]);
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn write_event(file: &mut fs::File, event: &InputEvent) -> Result<()> {
    let bytes = unsafe {
        std::slice::from_raw_parts(
            event as *const InputEvent as *const u8,
            std::mem::size_of::<InputEvent>(),
        )
    };
    file.write_all(bytes).context("write uinput event")
}

fn ioctl_int(fd: RawFd, request: u64, value: i32) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, request as libc::c_ulong, value) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error()).context("uinput ioctl failed");
    }
    Ok(())
}

unsafe fn ioctl_ptr(fd: RawFd, request: u64, value: *const libc::c_void) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, request as libc::c_ulong, value) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error()).context("uinput ioctl failed");
    }
    Ok(())
}

unsafe fn ioctl_noarg(fd: RawFd, request: u64) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, request as libc::c_ulong) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error()).context("uinput ioctl failed");
    }
    Ok(())
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

impl Default for UinputSetup {
    fn default() -> Self {
        Self {
            id: InputId::default(),
            name: [0; 80],
            ff_effects_max: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct InputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

impl InputEvent {
    fn new(type_: u16, code: u16, value: i32) -> Self {
        Self {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_,
            code,
            value,
        }
    }
}

const EV_SYN: i32 = 0x00;
const EV_KEY: i32 = 0x01;
const SYN_REPORT: i32 = 0;
const BUS_USB: u16 = 0x03;
const UI_SET_EVBIT: u64 = 0x4004_5564;
const UI_SET_KEYBIT: u64 = 0x4004_5565;
const UI_DEV_SETUP: u64 = 0x405c_5503;
const UI_DEV_CREATE: u64 = 0x5501;
const UI_DEV_DESTROY: u64 = 0x5502;

fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
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

fn default_allowed_uid() -> u32 {
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
        .unwrap_or_else(current_uid)
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn screen_locked_for_uid(uid: u32) -> Result<bool> {
    if let Some(session) = std::env::var_os("XDG_SESSION_ID") {
        return loginctl_session_locked(&session.to_string_lossy());
    }
    let output = std::process::Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .context("loginctl list-sessions failed")?;
    if !output.status.success() {
        bail!(
            "loginctl list-sessions exited with status {}",
            output.status.code().unwrap_or(-1)
        );
    }
    let sessions = String::from_utf8_lossy(&output.stdout);
    let mut saw_user_session = false;
    for line in sessions.lines() {
        let mut parts = line.split_whitespace();
        let Some(session_id) = parts.next() else {
            continue;
        };
        let Some(session_uid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if session_uid != uid {
            continue;
        }
        saw_user_session = true;
        if loginctl_session_locked(session_id)? {
            return Ok(true);
        }
    }
    if saw_user_session {
        Ok(false)
    } else {
        bail!("no logind session found for uid {uid}")
    }
}

fn loginctl_session_locked(session: &str) -> Result<bool> {
    let output = std::process::Command::new("loginctl")
        .args(["show-session", session, "-p", "LockedHint", "--value"])
        .output()
        .with_context(|| format!("loginctl show-session {session} failed"))?;
    if !output.status.success() {
        bail!(
            "loginctl show-session {session} exited with status {}",
            output.status.code().unwrap_or(-1)
        );
    }
    Ok(parse_locked_hint(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_locked_hint(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("yes")
}

struct SocketCleanup {
    path: PathBuf,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if let Ok(meta) = fs::symlink_metadata(&self.path) {
            if meta.file_type().is_socket() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_key_combo_names() {
        assert_eq!(
            parse_key_combo(&["ctrl".into(), "alt".into(), "t".into()]).unwrap(),
            vec![29, 56, 20]
        );
        assert!(parse_key_combo(&["bad-key".into()]).is_err());
    }

    #[test]
    fn parses_ascii_text_to_key_strokes() {
        let strokes = parse_text("Az 1!").unwrap();
        assert_eq!(
            strokes[0],
            KeyStroke {
                code: 30,
                shift: true
            }
        );
        assert_eq!(
            strokes[1],
            KeyStroke {
                code: 44,
                shift: false
            }
        );
        assert_eq!(
            strokes[2],
            KeyStroke {
                code: 57,
                shift: false
            }
        );
        assert_eq!(
            strokes[3],
            KeyStroke {
                code: 2,
                shift: false
            }
        );
        assert_eq!(
            strokes[4],
            KeyStroke {
                code: 2,
                shift: true
            }
        );
    }

    #[test]
    fn text_length_is_limited() {
        assert!(parse_text(&"a".repeat(257)).is_err());
    }

    #[test]
    fn parses_locked_hint() {
        assert!(parse_locked_hint("yes\n"));
        assert!(!parse_locked_hint("no\n"));
        assert!(!parse_locked_hint("\n"));
    }
}
