mod dispatch;
mod engine;
mod focus;
mod hardware;
mod ipc;
mod live;
mod log;
mod metrics_cache;
mod news;
mod notifications;
mod runtime;
mod scene;
mod session;
mod timers;
mod touch;

use anyhow::Result;
use clap::Parser;
use engine::{Engine, EngineParts};
use log::{log_fields, Level};
use metrics_cache::MetricsHandle;
use nexus_core::DeviceLock;
use runtime::{Runtime, RuntimeState};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
#[command(
    name = "linexusd",
    version,
    about = "Daemon that drives the Corsair iCUE Nexus touch strip"
)]
struct Args {
    /// Render one frame (to the panel, or to --emulator-output) and exit.
    #[arg(long)]
    once: bool,
    /// Do not open the panel; keep writing the current frame to this PNG.
    #[arg(long)]
    emulator_output: Option<PathBuf>,
    /// Config file; defaults to ~/.config/linexus/config.json, or the
    /// built-in pages when that does not exist.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Start on this page of the active profile.
    #[arg(long)]
    page: Option<String>,
    /// Plan desktop actions without running them (tests).
    #[arg(long)]
    dry_run_actions: bool,
    /// Longest wait for touch input between frames, in milliseconds.
    #[arg(long, default_value_t = 50)]
    touch_poll_ms: u64,
    /// Daemon API socket; defaults to $XDG_RUNTIME_DIR/linexus/daemon.sock.
    #[arg(long)]
    socket_path: Option<PathBuf>,
    /// Allow widgets to run programs (shell-command actions, command widgets).
    #[arg(long)]
    allow_shell_actions: bool,
    /// Keep the strip on while the session is locked or the system sleeps.
    #[arg(long)]
    disable_session_blank: bool,
    /// Start on the first page instead of the one active at the last stop.
    #[arg(long)]
    no_restore_state: bool,
    /// Where the last profile, page and brightness are kept.
    #[arg(long)]
    state_path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    hardware::install_signal_handlers();
    let logger = nexus_events::EventLogger::new("linexusd");
    log::log(&logger, Level::Info, "daemon_start", "daemon starting");
    // Only one process may drive the panel; the emulator never touches it.
    let _lock = match args.emulator_output {
        Some(_) => None,
        None => Some(DeviceLock::acquire()?),
    };

    let (mut runtime, report) = Runtime::load(args.config.as_deref())?;
    if let Some(report) = report {
        log_fields(
            &logger,
            Level::Info,
            "config_upgraded",
            "config upgraded to the current schema",
            [
                ("from_version", report.from_version.to_string()),
                ("notes", report.notes.join("; ")),
            ],
        );
    }
    let state_path = args
        .state_path
        .clone()
        .unwrap_or_else(RuntimeState::default_path);
    if let Some(page) = &args.page {
        runtime.go_to_page(page)?;
    } else if !args.no_restore_state && !args.once {
        if let Some(state) = RuntimeState::load(&state_path) {
            state.apply(&mut runtime);
        }
    }
    let session_blank = !args.disable_session_blank && runtime.settings().idle.blank_when_locked;

    let metrics = MetricsHandle::default();
    let _metrics_worker = if args.once {
        metrics.refresh_blocking();
        None
    } else {
        Some(metrics.spawn_worker(Duration::from_secs(1)))
    };
    let providers = live::ProviderHandle::default();
    let _provider_worker = (!args.once).then(|| providers.spawn());
    let notifications = if args.once {
        notifications::Notifications::off()
    } else {
        notifications::Notifications::on()
    };
    let focus = if args.once || args.emulator_output.is_some() {
        focus::Focus::off()
    } else {
        focus::Focus::on()
    };

    let (feedback_tx, feedback_rx) = mpsc::channel();
    let dispatcher = dispatch::Dispatcher::new(
        args.dry_run_actions,
        args.allow_shell_actions,
        logger.clone(),
        feedback_tx,
    );
    let mode = if args.emulator_output.is_some() {
        nexus_ipc::DaemonMode::Emulator
    } else {
        nexus_ipc::DaemonMode::Hardware
    };
    let mut engine = Engine::new(EngineParts {
        runtime,
        metrics,
        providers,
        dispatcher,
        feedback: feedback_rx,
        notifications,
        focus,
        logger: logger.clone(),
        state_path: (!args.once).then_some(state_path),
        mode,
    });

    if args.once {
        return match &args.emulator_output {
            Some(path) => hardware::write_png(&mut engine, path, Instant::now()),
            None => hardware::push_once(&mut engine),
        };
    }

    let (control_tx, control_rx) = mpsc::channel();
    let socket = args
        .socket_path
        .clone()
        .unwrap_or_else(nexus_ipc::default_socket_path);
    let _ipc = ipc::start(socket, control_tx, logger.clone())?;
    let cfg = hardware::LoopConfig {
        requests: &control_rx,
        logger: &logger,
        session_blank,
        touch_poll: Duration::from_millis(args.touch_poll_ms.max(5)),
    };
    let result = match &args.emulator_output {
        Some(path) => {
            engine.device = nexus_ipc::DeviceStatus::emulator();
            hardware::run_emulator(&mut engine, path, &cfg)
        }
        None => hardware::run_hardware(&mut engine, &cfg),
    };
    log::log(&logger, Level::Info, "daemon_stop", "daemon stopped");
    result
}
