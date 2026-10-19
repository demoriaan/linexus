//! Runs desktop actions off the render loop.
//!
//! Launches, media keys and HTTP calls can take a moment; a worker thread
//! executes them in order so a slow action never stalls the strip. Failures
//! come back as feedback the engine shows on the strip.

use crate::log::{log_fields, Level};
use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone)]
pub(crate) enum Job {
    Action {
        label: String,
        action: nexus_actions::Action,
    },
    HomeAssistant {
        label: String,
        domain: String,
        service: String,
        entity_id: Option<String>,
    },
}

impl Job {
    fn label(&self) -> &str {
        match self {
            Job::Action { label, .. } | Job::HomeAssistant { label, .. } => label,
        }
    }
}

/// Outcome the engine reports back on the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Feedback {
    pub(crate) label: String,
    pub(crate) error: String,
}

pub(crate) struct Dispatcher {
    /// `None` only while shutting down: dropping it ends the worker's queue.
    sender: Option<SyncSender<Job>>,
    /// Set on shutdown: actions still queued are skipped, not run.
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    allow_shell: bool,
}

impl Dispatcher {
    const QUEUE_CAPACITY: usize = 64;

    pub(crate) fn new(
        dry_run: bool,
        allow_shell: bool,
        logger: nexus_events::EventLogger,
        feedback: Sender<Feedback>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Job>(Self::QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let handle = thread::spawn(move || {
            while let Ok(job) = receiver.recv() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let label = job.label().to_string();
                let result = run(job, dry_run, &logger, &feedback);
                match result {
                    Ok(detail) => log_fields(
                        &logger,
                        Level::Info,
                        if dry_run {
                            "action_dry_run"
                        } else {
                            "action_executed"
                        },
                        "action executed",
                        [("widget", label), ("command", detail)],
                    ),
                    Err(error) => {
                        log_fields(
                            &logger,
                            Level::Error,
                            "action_failed",
                            "action failed",
                            [("widget", label.clone()), ("error", error.clone())],
                        );
                        let _ = feedback.send(Feedback { label, error });
                    }
                }
            }
        });
        Self {
            sender: Some(sender),
            stop,
            handle: Some(handle),
            allow_shell,
        }
    }

    pub(crate) fn dispatch(&self, job: Job) -> Result<()> {
        if let Job::Action {
            action: nexus_actions::Action::ShellCommand { .. },
            ..
        } = &job
        {
            if !self.allow_shell {
                bail!(
                    "shell actions are off; start linexusd with --allow-shell-actions to use them"
                );
            }
        }
        let Some(sender) = &self.sender else {
            bail!("action worker stopped");
        };
        sender.try_send(job).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("action queue is full; dropping action")
            }
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("action worker stopped"),
        })
    }

    pub(crate) fn shell_allowed(&self) -> bool {
        self.allow_shell
    }
}

fn run(
    job: Job,
    dry_run: bool,
    logger: &nexus_events::EventLogger,
    feedback: &Sender<Feedback>,
) -> Result<String, String> {
    match job {
        Job::Action { label, action } => {
            // Launches are watched off this thread so a browser that keeps
            // running does not delay the next action; an early failure is
            // still reported.
            let failure_logger = logger.clone();
            let failure_feedback = feedback.clone();
            let failed = label.clone();
            let on_launch_failure: nexus_actions::LaunchFailureReport = Box::new(move |error| {
                log_fields(
                    &failure_logger,
                    Level::Error,
                    "action_failed",
                    "launch failed",
                    [("widget", failed.clone()), ("error", error.to_string())],
                );
                let _ = failure_feedback.send(Feedback {
                    label: failed,
                    error: error.to_string(),
                });
            });
            nexus_actions::execute_action_watched(&action, dry_run, on_launch_failure)
                .map(|planned| planned.display_shell_like())
                .map_err(|error| error.to_string())
        }
        Job::HomeAssistant {
            domain,
            service,
            entity_id,
            ..
        } => {
            let detail = format!("{domain}.{service} {}", entity_id.as_deref().unwrap_or(""));
            if dry_run {
                return Ok(detail);
            }
            let config = nexus_sensors::load_sensor_config();
            let server = config.home_assistant_server.ok_or_else(|| {
                "Home Assistant is not connected (set it up in Studio)".to_string()
            })?;
            nexus_sensors::call_service(&server, &domain, &service, entity_id.as_deref())?;
            Ok(detail)
        }
    }
}

/// Waits for the action that is running, if any; queued ones are dropped so
/// a stop does not first work through a backlog of launches and requests.
impl Drop for Dispatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sender = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
