use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod desktop;

pub use desktop::{
    default_application, default_browser, default_terminal, session_environment, BrowserLauncher,
};

/// How long a detached launch (browser, app) is watched for an immediate
/// failure such as `xdg-open` finding no display, before it is left running.
const DETACH_GRACE: Duration = Duration::from_millis(1_500);

/// Receives the error of a detached launch that fails within
/// [`DETACH_GRACE`], called from a watcher thread.
pub type LaunchFailureReport = Box<dyn FnOnce(ActionError) + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendStatus {
    pub xdg_open: Option<PathBuf>,
    pub gtk_launch: Option<PathBuf>,
    pub kioclient: Option<PathBuf>,
    pub wpctl: Option<PathBuf>,
    pub busctl: Option<PathBuf>,
    pub wtype: Option<PathBuf>,
    pub ydotool: Option<PathBuf>,
    pub uinput_present: bool,
    pub input_helper_socket: PathBuf,
    pub input_helper_available: bool,
    pub systemd_run: Option<PathBuf>,
    pub curl: Option<PathBuf>,
    /// Graphical-session variables re-read from the systemd user manager;
    /// filled by [`BackendStatus::with_session_env`].
    pub session_env: Vec<(String, String)>,
    /// Default browser, when it supports opening URLs in a new window;
    /// filled by [`BackendStatus::with_default_browser`].
    pub default_browser: Option<BrowserLauncher>,
}

impl BackendStatus {
    pub fn probe() -> Self {
        let input_helper_socket = nexus_input::candidate_socket_paths()
            .into_iter()
            .find(|path| path.exists())
            .unwrap_or_else(nexus_input::default_socket_path);
        let input_helper_available = input_helper_socket.exists();
        Self {
            xdg_open: which("xdg-open"),
            gtk_launch: which("gtk-launch"),
            kioclient: which("kioclient").or_else(|| which("kioclient5")),
            wpctl: which("wpctl"),
            busctl: which("busctl"),
            wtype: which("wtype"),
            ydotool: which("ydotool"),
            uinput_present: Path::new("/dev/uinput").exists(),
            input_helper_socket,
            input_helper_available,
            systemd_run: which("systemd-run"),
            curl: which("curl"),
            session_env: Vec::new(),
            default_browser: None,
        }
    }

    /// Adds the graphical-session environment; only GUI launches need it.
    pub fn with_session_env(mut self) -> Self {
        self.session_env = session_environment();
        self
    }

    /// Adds the default browser launcher; only URL opening needs it.
    pub fn with_default_browser(mut self) -> Self {
        self.default_browser = default_browser(&self.session_env);
        self
    }

    pub fn supports_open_url(&self) -> bool {
        self.xdg_open.is_some() || self.kioclient.is_some()
    }

    pub fn supports_launch(&self) -> bool {
        self.gtk_launch.is_some() || self.xdg_open.is_some()
    }

    pub fn supports_volume(&self) -> bool {
        self.wpctl.is_some()
    }

    pub fn supports_mpris(&self) -> bool {
        self.busctl.is_some()
    }

    pub fn supports_input_helper(&self) -> bool {
        self.input_helper_available
    }

    pub fn supports_unprivileged_keyboard_text(&self) -> bool {
        self.wtype.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// True when the command is expected to return quickly and can safely be waited on.
    pub wait: bool,
    /// Hard cap for waited commands. Daemon integration must use an async queue around this.
    pub timeout: Option<Duration>,
}

impl PlannedCommand {
    pub fn display_shell_like(&self) -> String {
        let mut parts = vec![quote_arg(&self.program.display().to_string())];
        parts.extend(self.args.iter().map(|a| quote_arg(a)));
        parts.join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    Stop,
}

impl MediaCommand {
    pub fn busctl_method(self) -> &'static str {
        match self {
            Self::PlayPause => "PlayPause",
            Self::Next => "Next",
            Self::Previous => "Previous",
            Self::Stop => "Stop",
        }
    }
}

impl fmt::Display for MediaCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PlayPause => "play-pause",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Stop => "stop",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeCommand {
    Up(u8),
    Down(u8),
    Set(u8),
    ToggleMute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    OpenUrl(String),
    /// A file or folder, opened with its default application. `~` expands to
    /// the home directory.
    OpenPath(String),
    /// A desktop id (`org.kde.konsole.desktop`), an executable name, or one
    /// of the `default:browser|files|terminal|mail` placeholders resolved to
    /// the desktop's default application.
    Launch(String),
    Media {
        player: Option<String>,
        command: MediaCommand,
    },
    Volume(VolumeCommand),
    ShellCommand {
        program: String,
        args: Vec<String>,
    },
    KeyboardShortcut {
        keys: Vec<String>,
    },
    TextInput(String),
    /// Toggles the mute state of the default microphone.
    MicrophoneMute,
    HttpRequest {
        method: String,
        url: String,
        body: Option<String>,
        content_type: Option<String>,
    },
    LockScreen,
}

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("required backend is missing: {0}")]
    MissingBackend(&'static str),
    #[error("invalid action: {0}")]
    InvalidAction(String),
    #[error("no MPRIS player found; pass --player or start a media app")]
    NoMprisPlayer,
    #[error("command failed: {command} (status {status})")]
    CommandFailed { command: String, status: i32 },
    #[error("launch failed: {command} exited with status {status} (see the daemon journal for its stderr)")]
    LaunchFailed { command: String, status: i32 },
    #[error("command timed out after {timeout_ms}ms: {command}")]
    CommandTimedOut { command: String, timeout_ms: u64 },
    #[error("failed to execute {command}: {source}")]
    Io {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("keyboard/text input requires the privileged input helper; wtype fallback is not used implicitly")]
    InputHelperRequired,
    #[error("input helper request failed: {0}")]
    InputHelper(String),
}

pub type Result<T> = std::result::Result<T, ActionError>;

pub fn plan_action(action: &Action, status: &BackendStatus) -> Result<PlannedCommand> {
    match action {
        Action::OpenUrl(url) => plan_open_url(url, status),
        Action::OpenPath(path) => plan_open_path(path, status),
        Action::Launch(target) => plan_launch(target, status),
        Action::Media { player, command } => plan_media(player.as_deref(), *command, status),
        Action::Volume(command) => plan_volume(*command, status),
        Action::ShellCommand { program, args } => plan_shell(program, args),
        Action::KeyboardShortcut { keys } => plan_input_helper("keyboard-shortcut", keys, status),
        Action::TextInput(text) => {
            plan_input_helper("text-input", std::slice::from_ref(text), status)
        }
        Action::MicrophoneMute => plan_microphone_mute(status),
        Action::HttpRequest {
            method,
            url,
            body,
            content_type,
        } => plan_http_request(
            method,
            url,
            body.as_deref(),
            content_type.as_deref(),
            status,
        ),
        Action::LockScreen => plan_lock_screen(),
    }
}

/// Plans and runs `action`. A detached launch is watched for
/// [`DETACH_GRACE`] before this returns, so an immediate failure comes back as
/// the error (what an interactive CLI wants).
pub fn execute_action(action: &Action, dry_run: bool) -> Result<PlannedCommand> {
    execute_action_inner(action, dry_run, None)
}

/// Like [`execute_action`], but returns as soon as a detached launch has
/// started; a failure within the grace period goes to `on_launch_failure`
/// from a watcher thread, so a long-running app does not hold up the caller.
pub fn execute_action_watched(
    action: &Action,
    dry_run: bool,
    on_launch_failure: LaunchFailureReport,
) -> Result<PlannedCommand> {
    execute_action_inner(action, dry_run, Some(on_launch_failure))
}

fn execute_action_inner(
    action: &Action,
    dry_run: bool,
    on_launch_failure: Option<LaunchFailureReport>,
) -> Result<PlannedCommand> {
    if dry_run {
        match action {
            Action::KeyboardShortcut { keys } => {
                return Ok(input_helper_plan(
                    "keyboard-shortcut",
                    keys,
                    &nexus_input::default_socket_path(),
                ));
            }
            Action::TextInput(text) => {
                return Ok(input_helper_plan(
                    "text-input",
                    std::slice::from_ref(text),
                    &nexus_input::default_socket_path(),
                ));
            }
            _ => {}
        }
    }
    let mut status = BackendStatus::probe();
    if matches!(
        action,
        Action::OpenUrl(_) | Action::OpenPath(_) | Action::Launch(_)
    ) {
        status = status.with_session_env();
    }
    if matches!(action, Action::OpenUrl(_)) {
        status = status.with_default_browser();
    }
    let planned = plan_action(action, &status)?;
    if dry_run {
        return Ok(planned);
    }
    match action {
        Action::KeyboardShortcut { keys } => {
            send_input_helper(
                &status.input_helper_socket,
                nexus_input::InputCommand::KeyCombo { keys: keys.clone() },
            )?;
        }
        Action::TextInput(text) => {
            send_input_helper(
                &status.input_helper_socket,
                nexus_input::InputCommand::Text { text: text.clone() },
            )?;
        }
        _ => run_planned(&planned, &status, on_launch_failure)?,
    }
    Ok(planned)
}

pub fn list_mpris_players() -> Vec<String> {
    let Some(busctl) = which("busctl") else {
        return Vec::new();
    };
    let Ok(output) = Command::new(busctl).args(["--user", "list"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
        .map(ToOwned::to_owned)
        .collect()
}

fn plan_input_helper(
    kind: &str,
    values: &[String],
    status: &BackendStatus,
) -> Result<PlannedCommand> {
    if !status.input_helper_available {
        return Err(ActionError::InputHelperRequired);
    }
    Ok(input_helper_plan(kind, values, &status.input_helper_socket))
}

fn input_helper_plan(kind: &str, values: &[String], socket_path: &Path) -> PlannedCommand {
    let mut args = vec![kind.to_string()];
    args.extend(values.iter().cloned());
    PlannedCommand {
        program: socket_path.to_path_buf(),
        args,
        wait: true,
        timeout: Some(Duration::from_secs(3)),
    }
}

fn send_input_helper(path: &Path, command: nexus_input::InputCommand) -> Result<()> {
    nexus_input::send_request(path, command)
        .map(|_| ())
        .map_err(|error| ActionError::InputHelper(error.to_string()))
}

fn plan_open_url(url: &str, status: &BackendStatus) -> Result<PlannedCommand> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ActionError::InvalidAction(
            "open-url currently accepts only http:// or https:// URLs".into(),
        ));
    }
    // Prefer the default browser's own launcher with --new-window: a freshly
    // mapped window gets focus from the compositor, whereas xdg-open usually
    // adds a background tab to whichever browser window was used last.
    if let Some(browser) = &status.default_browser {
        if let Some(program) = browser
            .argv
            .first()
            .and_then(|program| resolve_program(program))
        {
            let mut args = browser.argv[1..].to_vec();
            args.extend(["--new-window".to_string(), url.to_string()]);
            return Ok(PlannedCommand {
                program,
                args,
                wait: false,
                timeout: None,
            });
        }
    }
    if let Some(program) = &status.xdg_open {
        return Ok(PlannedCommand {
            program: program.clone(),
            args: vec![url.to_string()],
            wait: false,
            timeout: None,
        });
    }
    if let Some(program) = &status.kioclient {
        return Ok(PlannedCommand {
            program: program.clone(),
            args: vec!["exec".into(), url.to_string()],
            wait: false,
            timeout: None,
        });
    }
    Err(ActionError::MissingBackend("xdg-open or kioclient"))
}

fn plan_launch(target: &str, status: &BackendStatus) -> Result<PlannedCommand> {
    if let Some(role) = target.strip_prefix("default:") {
        return plan_default_app(role, status);
    }
    if target.trim().is_empty() || target.contains('/') {
        return Err(ActionError::InvalidAction(
            "launch target must be a desktop id or executable name, not a path".into(),
        ));
    }
    if target.ends_with(".desktop") {
        if let Some(program) = &status.gtk_launch {
            return Ok(PlannedCommand {
                program: program.clone(),
                args: vec![target.to_string()],
                wait: false,
                timeout: None,
            });
        }
        if let Some(program) = &status.xdg_open {
            return Ok(PlannedCommand {
                program: program.clone(),
                args: vec![format!("applications:{target}")],
                wait: false,
                timeout: None,
            });
        }
        return Err(ActionError::MissingBackend("gtk-launch or xdg-open"));
    }
    let Some(program) = which(target) else {
        return Err(ActionError::MissingBackend("target executable in PATH"));
    };
    Ok(PlannedCommand {
        program,
        args: Vec::new(),
        wait: false,
        timeout: None,
    })
}

/// Resolves a `default:<role>` launch target to the desktop's default
/// application for that role.
fn plan_default_app(role: &str, status: &BackendStatus) -> Result<PlannedCommand> {
    let mime = match role {
        "browser" => "x-scheme-handler/https",
        "files" => "inode/directory",
        "mail" => "x-scheme-handler/mailto",
        "terminal" => {
            let argv = desktop::default_terminal(&status.session_env)
                .ok_or(ActionError::MissingBackend("a terminal emulator"))?;
            let program = resolve_program(&argv[0])
                .ok_or(ActionError::MissingBackend("a terminal emulator"))?;
            return Ok(PlannedCommand {
                program,
                args: argv[1..].to_vec(),
                wait: false,
                timeout: None,
            });
        }
        other => {
            return Err(ActionError::InvalidAction(format!(
                "unknown default application '{other}'"
            )))
        }
    };
    let desktop_id = desktop::default_application(mime, &status.session_env).ok_or(
        ActionError::MissingBackend("a default application (xdg-mime)"),
    )?;
    plan_launch(&desktop_id, status)
}

fn plan_open_path(path: &str, status: &BackendStatus) -> Result<PlannedCommand> {
    let expanded = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = env::var_os("HOME").ok_or(ActionError::InvalidAction(
                "cannot expand ~ without HOME".into(),
            ))?;
            format!("{}{rest}", PathBuf::from(home).display())
        }
        _ => path.to_string(),
    };
    if !Path::new(&expanded).is_absolute() {
        return Err(ActionError::InvalidAction(
            "open-path needs an absolute path or one starting with ~/".into(),
        ));
    }
    if let Some(program) = &status.xdg_open {
        return Ok(PlannedCommand {
            program: program.clone(),
            args: vec![expanded],
            wait: false,
            timeout: None,
        });
    }
    if let Some(program) = &status.kioclient {
        return Ok(PlannedCommand {
            program: program.clone(),
            args: vec!["exec".into(), expanded],
            wait: false,
            timeout: None,
        });
    }
    Err(ActionError::MissingBackend("xdg-open or kioclient"))
}

fn plan_microphone_mute(status: &BackendStatus) -> Result<PlannedCommand> {
    let Some(program) = &status.wpctl else {
        return Err(ActionError::MissingBackend("wpctl"));
    };
    Ok(PlannedCommand {
        program: program.clone(),
        args: vec![
            "set-mute".into(),
            "@DEFAULT_AUDIO_SOURCE@".into(),
            "toggle".into(),
        ],
        wait: true,
        timeout: Some(Duration::from_secs(3)),
    })
}

fn plan_http_request(
    method: &str,
    url: &str,
    body: Option<&str>,
    content_type: Option<&str>,
    status: &BackendStatus,
) -> Result<PlannedCommand> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ActionError::InvalidAction(
            "HTTP requests accept only http:// or https:// URLs".into(),
        ));
    }
    if !matches!(method, "GET" | "POST" | "PUT") {
        return Err(ActionError::InvalidAction(format!(
            "unsupported HTTP method '{method}'"
        )));
    }
    // The content type becomes a header line; a line break would add more.
    if content_type.is_some_and(|value| value.contains(['\r', '\n'])) {
        return Err(ActionError::InvalidAction(
            "HTTP content type must not contain line breaks".into(),
        ));
    }
    let Some(program) = status.curl.clone() else {
        return Err(ActionError::MissingBackend("curl"));
    };
    let mut args: Vec<String> = [
        "--silent",
        "--show-error",
        "--fail",
        "--max-time",
        "10",
        "--proto",
        "=http,https",
        "--output",
        "/dev/null",
        "--request",
        method,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(body) = body {
        args.push("--header".into());
        args.push(format!(
            "Content-Type: {}",
            content_type.unwrap_or("application/json")
        ));
        // Not --data-binary: it reads a body starting with `@` as a file
        // name and uploads that file.
        args.push("--data-raw".into());
        args.push(body.to_string());
    }
    args.push("--".into());
    args.push(url.to_string());
    Ok(PlannedCommand {
        program,
        args,
        wait: true,
        timeout: Some(Duration::from_secs(12)),
    })
}

fn plan_lock_screen() -> Result<PlannedCommand> {
    let program = which("loginctl").ok_or(ActionError::MissingBackend("loginctl"))?;
    Ok(PlannedCommand {
        program,
        args: vec!["lock-session".into()],
        wait: true,
        timeout: Some(Duration::from_secs(5)),
    })
}

fn plan_media(
    player: Option<&str>,
    command: MediaCommand,
    status: &BackendStatus,
) -> Result<PlannedCommand> {
    let Some(program) = &status.busctl else {
        return Err(ActionError::MissingBackend("busctl"));
    };
    let player = match player {
        Some(player) if player.starts_with("org.mpris.MediaPlayer2.") => player.to_string(),
        Some(_) => {
            return Err(ActionError::InvalidAction(
                "MPRIS player must start with org.mpris.MediaPlayer2.".into(),
            ))
        }
        None => list_mpris_players()
            .into_iter()
            .next()
            .ok_or(ActionError::NoMprisPlayer)?,
    };
    Ok(PlannedCommand {
        program: program.clone(),
        args: vec![
            "--user".into(),
            "call".into(),
            player,
            "/org/mpris/MediaPlayer2".into(),
            "org.mpris.MediaPlayer2.Player".into(),
            command.busctl_method().into(),
        ],
        wait: true,
        timeout: Some(Duration::from_secs(3)),
    })
}

fn plan_volume(command: VolumeCommand, status: &BackendStatus) -> Result<PlannedCommand> {
    let Some(program) = &status.wpctl else {
        return Err(ActionError::MissingBackend("wpctl"));
    };
    // wpctl has no ceiling of its own: without `--limit` a held volume-up
    // key keeps amplifying past 100 %.
    let args = match command {
        VolumeCommand::Up(step) => vec![
            "set-volume".into(),
            "--limit".into(),
            "1.0".into(),
            "@DEFAULT_AUDIO_SINK@".into(),
            format!("{}%+", step.clamp(1, 100)),
        ],
        VolumeCommand::Down(step) => vec![
            "set-volume".into(),
            "@DEFAULT_AUDIO_SINK@".into(),
            format!("{}%-", step.clamp(1, 100)),
        ],
        VolumeCommand::Set(percent) => vec![
            "set-volume".into(),
            "--limit".into(),
            "1.0".into(),
            "@DEFAULT_AUDIO_SINK@".into(),
            format!("{}%", percent.min(100)),
        ],
        VolumeCommand::ToggleMute => vec![
            "set-mute".into(),
            "@DEFAULT_AUDIO_SINK@".into(),
            "toggle".into(),
        ],
    };
    Ok(PlannedCommand {
        program: program.clone(),
        args,
        wait: true,
        timeout: Some(Duration::from_secs(3)),
    })
}

fn plan_shell(program: &str, args: &[String]) -> Result<PlannedCommand> {
    if program.trim().is_empty() || program.contains('/') {
        return Err(ActionError::InvalidAction(
            "shell command program must be a command name in PATH; arbitrary shell strings are intentionally not accepted here".into(),
        ));
    }
    let Some(program_path) = which(program) else {
        return Err(ActionError::MissingBackend("shell command in PATH"));
    };
    Ok(PlannedCommand {
        program: program_path,
        args: args.to_vec(),
        wait: true,
        timeout: Some(Duration::from_secs(10)),
    })
}

fn run_planned(
    planned: &PlannedCommand,
    status: &BackendStatus,
    on_launch_failure: Option<LaunchFailureReport>,
) -> Result<()> {
    if !planned.wait {
        return launch_detached(planned, status, on_launch_failure);
    }
    let mut child = Command::new(&planned.program)
        .args(&planned.args)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|source| ActionError::Io {
            command: planned.display_shell_like(),
            source,
        })?;
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|source| ActionError::Io {
            command: planned.display_shell_like(),
            source,
        })? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(ActionError::CommandFailed {
                    command: planned.display_shell_like(),
                    status: status.code().unwrap_or(-1),
                })
            }
            None => {
                if let Some(timeout) = planned.timeout {
                    if started.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(ActionError::CommandTimedOut {
                            command: planned.display_shell_like(),
                            timeout_ms: timeout.as_millis() as u64,
                        });
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Starts a GUI program that keeps running on its own (browser, app).
///
/// With a reachable desktop session it runs as a transient service of the
/// systemd user manager, the way desktop launchers start apps: it inherits the
/// manager's up-to-date graphical environment, its output lands in the journal
/// under its own unit instead of linexusd's, and it lives outside the
/// daemon's cgroup so restarting linexusd does not kill it. `--wait` keeps
/// systemd-run around until the program exits and passes its exit status on,
/// which is how an immediate failure (xdg-open: no display) is detected.
/// Without a session (no user manager, headless) the program is spawned
/// directly with the daemon's environment.
fn launch_detached(
    planned: &PlannedCommand,
    status: &BackendStatus,
    on_launch_failure: Option<LaunchFailureReport>,
) -> Result<()> {
    let mut command = match &status.systemd_run {
        Some(systemd_run) if !status.session_env.is_empty() => {
            let mut command = Command::new(systemd_run);
            command
                .args([
                    "--user",
                    "--wait",
                    "--collect",
                    "--quiet",
                    "--slice=app.slice",
                    "--property=Type=exec",
                    "--",
                ])
                .arg(&planned.program);
            command
        }
        _ => Command::new(&planned.program),
    };
    let child = command
        .args(&planned.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|source| ActionError::Io {
            command: planned.display_shell_like(),
            source,
        })?;
    let command = planned.display_shell_like();
    match on_launch_failure {
        None => watch_launch(child, command),
        Some(report) => {
            thread::spawn(move || {
                if let Err(error) = watch_launch(child, command) {
                    report(error);
                }
            });
            Ok(())
        }
    }
}

/// Waits up to [`DETACH_GRACE`] for an early exit; a program still running
/// after that is left alone with a waiter thread that reaps it later.
fn watch_launch(mut child: Child, command: String) -> Result<()> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(ActionError::LaunchFailed {
                    command,
                    status: status.code().unwrap_or(-1),
                })
            }
            Ok(None) if started.elapsed() < DETACH_GRACE => {
                thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(source) => return Err(ActionError::Io { command, source }),
        }
    }
}

/// Absolute path for a desktop-entry program, or its `PATH` lookup.
fn resolve_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return is_executable_file(path).then(|| path.to_path_buf());
    }
    which(program)
}

pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') || bin.is_empty() {
        return None;
    }
    let path = env::var_os("PATH")?;
    which_in_paths(bin, env::split_paths(&path))
}

fn which_in_paths<I>(bin: &str, paths: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    paths
        .into_iter()
        .map(|dir| dir.join(bin))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = path.metadata() else {
        return false;
    };
    meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn quote_arg(arg: &str) -> String {
    if arg.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | ':' | '@' | '%' | '+')
    }) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> BackendStatus {
        BackendStatus {
            xdg_open: Some(PathBuf::from("/usr/bin/xdg-open")),
            gtk_launch: Some(PathBuf::from("/usr/bin/gtk-launch")),
            kioclient: Some(PathBuf::from("/usr/bin/kioclient")),
            wpctl: Some(PathBuf::from("/usr/bin/wpctl")),
            busctl: Some(PathBuf::from("/usr/bin/busctl")),
            wtype: Some(PathBuf::from("/usr/bin/wtype")),
            ydotool: None,
            uinput_present: true,
            input_helper_socket: PathBuf::from("/run/linexus/inputd.sock"),
            input_helper_available: false,
            systemd_run: Some(PathBuf::from("/usr/bin/systemd-run")),
            curl: Some(PathBuf::from("/usr/bin/curl")),
            session_env: Vec::new(),
            default_browser: None,
        }
    }

    #[test]
    fn plans_open_url_with_xdg_open() {
        let planned =
            plan_action(&Action::OpenUrl("https://example.com".into()), &status()).unwrap();
        assert_eq!(planned.program, PathBuf::from("/usr/bin/xdg-open"));
        assert_eq!(planned.args, vec!["https://example.com"]);
        assert!(!planned.wait);
    }

    #[test]
    fn plans_open_url_in_a_new_default_browser_window() {
        let mut status = status();
        status.default_browser = Some(BrowserLauncher {
            desktop_id: "test-browser.desktop".into(),
            argv: vec!["/bin/sh".into(), "--profile-flag".into()],
        });
        let planned = plan_action(&Action::OpenUrl("https://example.com".into()), &status).unwrap();
        assert_eq!(planned.program, PathBuf::from("/bin/sh"));
        assert_eq!(
            planned.args,
            vec!["--profile-flag", "--new-window", "https://example.com"]
        );
        assert!(!planned.wait);

        // A launcher whose program is gone falls back to xdg-open.
        status.default_browser = Some(BrowserLauncher {
            desktop_id: "gone.desktop".into(),
            argv: vec!["/nonexistent/brave".into()],
        });
        let planned = plan_action(&Action::OpenUrl("https://example.com".into()), &status).unwrap();
        assert_eq!(planned.program, PathBuf::from("/usr/bin/xdg-open"));
    }

    #[test]
    fn detached_launch_reports_immediate_failure() {
        // No session env: spawned directly, no systemd-run involved.
        let status = status();
        let failing = PlannedCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "exit 3".into()],
            wait: false,
            timeout: None,
        };
        let err = run_planned(&failing, &status, None).unwrap_err();
        assert!(matches!(err, ActionError::LaunchFailed { status: 3, .. }));
        let succeeding = PlannedCommand {
            args: vec!["-c".into(), "exit 0".into()],
            ..failing.clone()
        };
        run_planned(&succeeding, &status, None).unwrap();

        // Watched launches return immediately and report from a thread.
        let (sender, receiver) = std::sync::mpsc::channel();
        let started = Instant::now();
        run_planned(
            &failing,
            &status,
            Some(Box::new(move |error| {
                let _ = sender.send(error);
            })),
        )
        .unwrap();
        assert!(started.elapsed() < DETACH_GRACE);
        let reported = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            reported,
            ActionError::LaunchFailed { status: 3, .. }
        ));
    }

    #[test]
    fn rejects_non_http_urls_for_now() {
        let err = plan_action(&Action::OpenUrl("file:///tmp/x".into()), &status()).unwrap_err();
        assert!(matches!(err, ActionError::InvalidAction(_)));
    }

    #[test]
    fn plans_desktop_launch_with_gtk_launch() {
        let planned =
            plan_action(&Action::Launch("org.kde.kate.desktop".into()), &status()).unwrap();
        assert_eq!(planned.program, PathBuf::from("/usr/bin/gtk-launch"));
        assert_eq!(planned.args, vec!["org.kde.kate.desktop"]);
    }

    #[test]
    fn plans_mpris_play_pause() {
        let planned = plan_action(
            &Action::Media {
                player: Some("org.mpris.MediaPlayer2.brave.instance1".into()),
                command: MediaCommand::PlayPause,
            },
            &status(),
        )
        .unwrap();
        assert_eq!(planned.program, PathBuf::from("/usr/bin/busctl"));
        assert_eq!(planned.args[0], "--user");
        assert!(planned.args.contains(&"PlayPause".to_string()));
        assert!(planned.wait);
    }

    #[test]
    fn plans_volume_up() {
        let planned = plan_action(&Action::Volume(VolumeCommand::Up(5)), &status()).unwrap();
        assert_eq!(
            planned.args,
            vec![
                "set-volume",
                "--limit",
                "1.0",
                "@DEFAULT_AUDIO_SINK@",
                "5%+"
            ]
        );
    }

    #[test]
    fn volume_never_goes_past_100_percent() {
        let planned = plan_action(&Action::Volume(VolumeCommand::Set(150)), &status()).unwrap();
        assert_eq!(
            planned.args,
            vec![
                "set-volume",
                "--limit",
                "1.0",
                "@DEFAULT_AUDIO_SINK@",
                "100%"
            ]
        );
        let planned = plan_action(&Action::Volume(VolumeCommand::Down(5)), &status()).unwrap();
        assert_eq!(planned.args[2], "5%-");
    }

    #[test]
    fn http_body_is_sent_as_text_never_as_a_file() {
        let planned = plan_action(
            &Action::HttpRequest {
                method: "POST".into(),
                url: "https://example.com/hook".into(),
                body: Some("@/home/user/.ssh/id_ed25519".into()),
                content_type: Some("text/plain".into()),
            },
            &status(),
        )
        .unwrap();
        assert_eq!(planned.program, PathBuf::from("/usr/bin/curl"));
        let at = planned
            .args
            .iter()
            .position(|arg| arg == "@/home/user/.ssh/id_ed25519")
            .unwrap();
        // --data-raw is the one curl data option without `@file` reading.
        assert_eq!(planned.args[at - 1], "--data-raw");
        assert!(!planned.args.iter().any(|arg| arg == "--data-binary"
            || arg == "--data"
            || arg == "-d"
            || arg == "--json"));
        assert!(planned
            .args
            .contains(&"Content-Type: text/plain".to_string()));
        assert_eq!(
            planned.args[planned.args.len() - 2..],
            ["--", "https://example.com/hook"]
        );
    }

    #[test]
    fn http_content_type_must_be_a_single_line() {
        for content_type in ["text/plain\r\nX-Evil: 1", "text/plain\nX-Evil: 1"] {
            let err = plan_action(
                &Action::HttpRequest {
                    method: "POST".into(),
                    url: "https://example.com/hook".into(),
                    body: Some("{}".into()),
                    content_type: Some(content_type.into()),
                },
                &status(),
            )
            .unwrap_err();
            assert!(
                matches!(err, ActionError::InvalidAction(_)),
                "{content_type:?}"
            );
        }
    }

    #[test]
    fn shell_command_must_be_path_command_not_shell_string() {
        let err = plan_action(
            &Action::ShellCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "rm -rf /".into()],
            },
            &status(),
        )
        .unwrap_err();
        assert!(matches!(err, ActionError::InvalidAction(_)));
    }

    #[test]
    fn keyboard_requires_helper() {
        let err = plan_action(
            &Action::KeyboardShortcut {
                keys: vec!["ctrl".into(), "alt".into(), "t".into()],
            },
            &status(),
        )
        .unwrap_err();
        assert!(matches!(err, ActionError::InputHelperRequired));
    }

    #[test]
    fn keyboard_plans_input_helper_when_socket_available() {
        let mut status = status();
        status.input_helper_available = true;
        status.input_helper_socket = PathBuf::from("/run/linexus/inputd.sock");
        let planned = plan_action(
            &Action::KeyboardShortcut {
                keys: vec!["ctrl".into(), "l".into()],
            },
            &status,
        )
        .unwrap();
        assert_eq!(planned.program, PathBuf::from("/run/linexus/inputd.sock"));
        assert_eq!(planned.args, vec!["keyboard-shortcut", "ctrl", "l"]);
    }

    #[test]
    fn text_input_requires_helper() {
        let err = plan_action(&Action::TextInput("hello".into()), &status()).unwrap_err();
        assert!(matches!(err, ActionError::InputHelperRequired));
    }

    #[test]
    fn which_ignores_non_executable_path_entries() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = env::temp_dir().join(format!(
            "linexus-action-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let candidate = dir.join("fakecmd");
        fs::write(&candidate, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(which_in_paths("fakecmd", vec![dir.clone()]), None);

        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            which_in_paths("fakecmd", vec![dir.clone()]),
            Some(candidate)
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
