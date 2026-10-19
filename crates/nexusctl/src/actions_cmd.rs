//! `linexusctl actions`, `linexusctl action` and `linexusctl input-helper`: check
//! and run desktop actions without the daemon.

use anyhow::{Context, Result};
use clap::Subcommand;
use nexus_actions::{Action, BackendStatus, MediaCommand, VolumeCommand};
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub(crate) enum ActionsCommand {
    /// Show which action backends are available and the MPRIS players.
    Doctor,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ActionCommand {
    /// Open an http(s) URL with the default browser.
    OpenUrl {
        url: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Open a file or folder with its default application.
    OpenPath {
        /// Absolute path or one starting with `~/`.
        path: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Launch a .desktop id, an executable from PATH, or
    /// default:browser|files|terminal|mail.
    Launch {
        target: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Send an MPRIS media command to the first or the selected player.
    Media {
        command: CliMediaCommand,
        #[arg(long)]
        player: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Change the PipeWire volume through wpctl.
    Volume {
        command: CliVolumeCommand,
        #[arg(long, default_value_t = 5)]
        step: u8,
        #[arg(long)]
        percent: Option<u8>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Toggle the default microphone's mute state.
    MicMute {
        #[arg(long)]
        dry_run: bool,
    },
    /// Lock the session.
    LockScreen {
        #[arg(long)]
        dry_run: bool,
    },
    /// Send an HTTP request with curl (webhooks, smart-home bridges).
    Http {
        url: String,
        #[arg(long, value_enum, default_value_t = CliHttpMethod::Post)]
        method: CliHttpMethod,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        content_type: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Send a keyboard shortcut through the input helper.
    KeyboardShortcut {
        keys: Vec<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Type text through the input helper.
    TextInput {
        text: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run a PATH command directly, without a shell.
    Shell {
        program: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub(crate) enum CliMediaCommand {
    PlayPause,
    Next,
    Previous,
    Stop,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub(crate) enum CliVolumeCommand {
    Up,
    Down,
    Set,
    ToggleMute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CliHttpMethod {
    Get,
    Post,
    Put,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InputHelperCommand {
    /// Print the input helper socket path.
    SocketPath,
    /// Check whether the helper answers, without typing anything.
    Ping,
}

pub(crate) fn run_actions(command: ActionsCommand) -> Result<()> {
    match command {
        ActionsCommand::Doctor => print_action_backends(),
    }
    Ok(())
}

fn print_action_backends() {
    let status = BackendStatus::probe()
        .with_session_env()
        .with_default_browser();
    println!("Action backends");
    println!("===============");
    println!("open URL / file: {}", yes_no(status.supports_open_url()));
    println!(
        "open URL in a new browser window: {}",
        status
            .default_browser
            .as_ref()
            .map(|browser| format!("{} ({})", browser.desktop_id, browser.argv.join(" ")))
            .unwrap_or_else(|| "no (falls back to xdg-open)".into())
    );
    println!(
        "graphical session env: {}",
        if status.session_env.is_empty() {
            "missing from the systemd user manager".to_string()
        } else {
            status
                .session_env
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!("launch apps: {}", yes_no(status.supports_launch()));
    println!(
        "PipeWire volume / mic mute: {}",
        yes_no(status.supports_volume())
    );
    println!("MPRIS media: {}", yes_no(status.supports_mpris()));
    println!(
        "HTTP requests (curl): {}",
        yes_no(crate::util::which("curl").is_some())
    );
    println!(
        "lock screen (loginctl): {}",
        yes_no(crate::util::which("loginctl").is_some())
    );
    println!(
        "keyboard shortcuts / text: {}",
        if status.supports_input_helper() {
            "available via the input helper"
        } else {
            "needs the input helper (see `linexusctl doctor`)"
        }
    );
    println!(
        "input helper socket: {} ({})",
        status.input_helper_socket.display(),
        if status.input_helper_available {
            "present"
        } else {
            "missing"
        }
    );
    println!("/dev/uinput: {}", yes_no(status.uinput_present));
    println!();
    println!("Binaries:");
    for (name, path) in [
        ("xdg-open", &status.xdg_open),
        ("gtk-launch", &status.gtk_launch),
        ("kioclient", &status.kioclient),
        ("wpctl", &status.wpctl),
        ("busctl", &status.busctl),
        ("systemd-run", &status.systemd_run),
    ] {
        println!("- {name}: {}", fmt_path(path.as_deref()));
    }
    println!();
    let players = nexus_actions::list_mpris_players();
    println!(
        "MPRIS players: {}",
        if players.is_empty() {
            "none".into()
        } else {
            players.join(", ")
        }
    );
}

fn fmt_path(path: Option<&Path>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "missing".into())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

pub(crate) fn run_action(command: ActionCommand) -> Result<()> {
    let (action, dry_run) = match command {
        ActionCommand::OpenUrl { url, dry_run } => (Action::OpenUrl(url), dry_run),
        ActionCommand::OpenPath { path, dry_run } => (Action::OpenPath(path), dry_run),
        ActionCommand::Launch { target, dry_run } => (Action::Launch(target), dry_run),
        ActionCommand::Media {
            command,
            player,
            dry_run,
        } => (
            Action::Media {
                player,
                command: match command {
                    CliMediaCommand::PlayPause => MediaCommand::PlayPause,
                    CliMediaCommand::Next => MediaCommand::Next,
                    CliMediaCommand::Previous => MediaCommand::Previous,
                    CliMediaCommand::Stop => MediaCommand::Stop,
                },
            },
            dry_run,
        ),
        ActionCommand::Volume {
            command,
            step,
            percent,
            dry_run,
        } => {
            let command = match command {
                CliVolumeCommand::Up => VolumeCommand::Up(step),
                CliVolumeCommand::Down => VolumeCommand::Down(step),
                CliVolumeCommand::Set => {
                    VolumeCommand::Set(percent.context("volume set needs --percent <0..150>")?)
                }
                CliVolumeCommand::ToggleMute => VolumeCommand::ToggleMute,
            };
            (Action::Volume(command), dry_run)
        }
        ActionCommand::MicMute { dry_run } => (Action::MicrophoneMute, dry_run),
        ActionCommand::LockScreen { dry_run } => (Action::LockScreen, dry_run),
        ActionCommand::Http {
            url,
            method,
            body,
            content_type,
            dry_run,
        } => (
            Action::HttpRequest {
                method: match method {
                    CliHttpMethod::Get => "GET",
                    CliHttpMethod::Post => "POST",
                    CliHttpMethod::Put => "PUT",
                }
                .into(),
                url,
                body,
                content_type,
            },
            dry_run,
        ),
        ActionCommand::KeyboardShortcut { keys, dry_run } => {
            (Action::KeyboardShortcut { keys }, dry_run)
        }
        ActionCommand::TextInput { text, dry_run } => (Action::TextInput(text), dry_run),
        ActionCommand::Shell {
            program,
            args,
            dry_run,
        } => {
            // trailing_var_arg swallows a --dry-run given after the program's
            // arguments; honour it instead of passing it on.
            let dry_run = dry_run || args.iter().any(|arg| arg == "--dry-run");
            let args = args.into_iter().filter(|arg| arg != "--dry-run").collect();
            (Action::ShellCommand { program, args }, dry_run)
        }
    };

    let planned = nexus_actions::execute_action(&action, dry_run)?;
    if dry_run {
        println!("DRY RUN: {}", planned.display_shell_like());
    } else {
        println!("Executed: {}", planned.display_shell_like());
    }
    Ok(())
}

pub(crate) fn run_input_helper(
    command: InputHelperCommand,
    socket_path: Option<PathBuf>,
) -> Result<()> {
    let path = socket_path.unwrap_or_else(|| BackendStatus::probe().input_helper_socket);
    match command {
        InputHelperCommand::SocketPath => println!("{}", path.display()),
        InputHelperCommand::Ping => {
            let response = nexus_input::send_request(&path, nexus_input::InputCommand::Ping)
                .with_context(|| format!("input helper ping failed via {}", path.display()))?;
            println!("{}", response.message);
        }
    }
    Ok(())
}
