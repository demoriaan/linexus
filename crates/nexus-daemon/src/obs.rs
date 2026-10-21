//! OBS Studio on the strip.
//!
//! While the active profile has OBS buttons (or `{obs.*}` text), a worker
//! thread stays connected to OBS's websocket server, follows the scene,
//! the recording, the stream and the mute state of audio sources, and sends
//! the buttons' commands. Without such widgets, or without OBS set up in
//! Studio, it stays idle and keeps no connection.

use crate::dispatch::Feedback;
use crate::log::{log_fields, Level};
use nexus_config::{ActionSpec, ObsCommand, Profile, WidgetKind};
use nexus_render::ObsInfo;
use nexus_sensors::{ObsConnection, ObsError, ObsEvent, ObsServer};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// First wait before connecting again; doubles while OBS stays away.
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(15);
/// A refused password is not retried soon: it will not change by itself.
const RETRY_AUTH: Duration = Duration::from_secs(30);
/// How long the worker waits for an event before it looks for commands.
const EVENT_WAIT: Duration = Duration::from_millis(100);
/// Running time of the recording and the stream is asked this often.
const OUTPUTS_EVERY: Duration = Duration::from_secs(1);
/// sensors.json is read this often while connected, to follow changes.
const CONFIG_EVERY: Duration = Duration::from_secs(5);
/// Audio sources asked for their mute state when connecting.
const MAX_INPUTS: usize = nexus_sensors::OBS_MAX_INPUTS;
/// Without a recording or stream to follow, OBS is asked for its scene this
/// often, so a host that vanished without closing the connection shows.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(10);
const NOT_CONNECTED: &str = "OBS is not connected";

type LoadServer = Box<dyn Fn() -> Option<ObsServer> + Send>;

struct Command {
    label: String,
    command: ObsCommand,
    target: Option<String>,
}

#[derive(Default)]
struct Shared {
    status: Mutex<Status>,
    wake: Condvar,
}

#[derive(Default)]
struct Status {
    wanted: bool,
    stop: bool,
    /// A connection is saved in sensors.json, as last read.
    configured: bool,
    /// Set while connected.
    tracked: Option<Tracked>,
    /// Why the last attempt failed; None when it worked or none was made.
    problem: Option<String>,
}

pub(crate) struct Obs {
    shared: Arc<Shared>,
    commands: Option<Sender<Command>>,
    /// `--dry-run-actions`: commands are logged, not sent.
    dry_run: Option<nexus_events::EventLogger>,
}

impl Obs {
    /// No worker: OBS buttons report that OBS is not connected.
    pub(crate) fn off() -> Self {
        Self {
            shared: Arc::default(),
            commands: None,
            dry_run: None,
        }
    }

    pub(crate) fn on(
        feedback: Sender<Feedback>,
        logger: nexus_events::EventLogger,
        dry_run: bool,
    ) -> Self {
        let mut obs = Self::start(
            Box::new(|| nexus_sensors::load_sensor_config().obs_server),
            feedback,
            logger.clone(),
        );
        obs.dry_run = dry_run.then_some(logger);
        obs
    }

    fn start(
        load: LoadServer,
        feedback: Sender<Feedback>,
        logger: nexus_events::EventLogger,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let worker = Worker {
            shared: shared.clone(),
            commands: rx,
            feedback,
            logger,
            load,
        };
        let started = thread::Builder::new()
            .name("obs".into())
            .spawn(move || worker.run());
        Self {
            shared,
            commands: started.is_ok().then_some(tx),
            dry_run: None,
        }
    }

    /// Connects while `wanted`, and lets go of OBS otherwise.
    pub(crate) fn set_wanted(&self, wanted: bool) {
        let mut status = self.lock();
        if status.wanted != wanted {
            status.wanted = wanted;
            self.shared.wake.notify_all();
        }
    }

    /// Queues a button's command; refused while OBS is not connected.
    pub(crate) fn send(
        &self,
        label: &str,
        command: ObsCommand,
        target: Option<String>,
    ) -> Result<(), String> {
        let (connected, configured, problem) = {
            let status = self.lock();
            let configured = status.configured && status.wanted;
            (status.tracked.is_some(), configured, status.problem.clone())
        };
        if !connected {
            return Err(match problem {
                Some(problem) => problem,
                None if configured => "Connecting to OBS".to_string(),
                None => format!("{NOT_CONNECTED}; set it up in Studio under Connections"),
            });
        }
        if let Some(logger) = &self.dry_run {
            let (kind, _) = request(command, target.as_deref());
            log_fields(
                logger,
                Level::Info,
                "action_dry_run",
                "action executed",
                [
                    ("widget", label.to_string()),
                    ("command", format!("obs {kind}")),
                ],
            );
            return Ok(());
        }
        let sender = self.commands.as_ref().ok_or(NOT_CONNECTED)?;
        sender
            .send(Command {
                label: label.to_string(),
                command,
                target,
            })
            .map_err(|_| NOT_CONNECTED.to_string())
    }

    /// OBS's state for the strip, with running times counted up to `now`.
    pub(crate) fn info(&self, now: Instant) -> Option<ObsInfo> {
        self.lock()
            .tracked
            .as_ref()
            .map(|tracked| tracked.info(now))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Obs {
    fn drop(&mut self) {
        self.lock().stop = true;
        self.shared.wake.notify_all();
    }
}

/// The profile shows OBS's state or controls OBS somewhere.
pub(crate) fn profile_uses_obs(profile: &Profile) -> bool {
    fn controls_obs(action: &ActionSpec) -> bool {
        match action {
            ActionSpec::Obs { .. } => true,
            ActionSpec::Sequence { steps, .. } => steps.iter().any(controls_obs),
            _ => false,
        }
    }
    profile
        .pages
        .iter()
        .flat_map(|page| &page.widgets)
        .any(|widget| {
            let text = match &widget.kind {
                WidgetKind::Text(props) => props.text.contains("{obs."),
                _ => false,
            };
            text || [&widget.tap, &widget.double_tap, &widget.hold]
                .into_iter()
                .flatten()
                .any(controls_obs)
        })
}

/// What the strip shows of OBS, as last heard.
#[derive(Debug, Clone, Default, PartialEq)]
struct Tracked {
    scene: String,
    streaming: bool,
    /// The stream lost its connection and OBS is trying again.
    stream_reconnecting: bool,
    stream_ms: u64,
    recording: bool,
    record_paused: bool,
    record_ms: u64,
    /// When the running times were measured.
    measured: Option<Instant>,
    replay_buffer: bool,
    virtual_cam: bool,
    muted: HashMap<String, bool>,
}

impl Tracked {
    fn info(&self, now: Instant) -> ObsInfo {
        let since = self
            .measured
            .map_or(0, |at| now.saturating_duration_since(at).as_millis() as u64);
        ObsInfo {
            scene: self.scene.clone(),
            streaming: self.streaming,
            stream_reconnecting: self.stream_reconnecting,
            stream_ms: if self.streaming {
                self.stream_ms + since
            } else {
                0
            },
            recording: self.recording,
            record_paused: self.record_paused,
            record_ms: match (self.recording, self.record_paused) {
                (true, false) => self.record_ms + since,
                (true, true) => self.record_ms,
                _ => 0,
            },
            replay_buffer: self.replay_buffer,
            virtual_cam: self.virtual_cam,
            muted: self.muted.clone(),
        }
    }

    /// Everything at once, right after connecting.
    fn sync(obs: &mut ObsConnection) -> Result<Self, ObsError> {
        let mut tracked = Self {
            scene: scene_name(&obs.request("GetCurrentProgramScene", None)?),
            replay_buffer: active(obs.request("GetReplayBufferStatus", None)),
            virtual_cam: active(obs.request("GetVirtualCamStatus", None)),
            ..Self::default()
        };
        let stream = obs.request("GetStreamStatus", None)?;
        let record = obs.request("GetRecordStatus", None)?;
        tracked.streaming = stream["outputActive"] == true || stream["outputReconnecting"] == true;
        tracked.recording = record["outputActive"] == true;
        tracked.measure_outputs(obs, Instant::now())?;
        let inputs = nexus_sensors::obs_input_names(&obs.request("GetInputList", None)?);
        for input in inputs.into_iter().take(MAX_INPUTS) {
            tracked.ask_mute(obs, input)?;
        }
        Ok(tracked)
    }

    /// Running times (and the pause) of the outputs that run. Whether they
    /// run comes from the events: while an output stops, its status still
    /// says active for a while, and the button should not light up again.
    fn measure_outputs(&mut self, obs: &mut ObsConnection, now: Instant) -> Result<(), ObsError> {
        self.stream_ms = 0;
        self.record_ms = 0;
        if self.streaming {
            let stream = obs.request("GetStreamStatus", None)?;
            self.stream_ms = stream["outputDuration"].as_u64().unwrap_or(0);
            self.stream_reconnecting = stream["outputReconnecting"] == true;
        } else {
            self.stream_reconnecting = false;
        }
        if self.recording {
            let record = obs.request("GetRecordStatus", None)?;
            self.record_paused = record["outputPaused"] == true;
            self.record_ms = record["outputDuration"].as_u64().unwrap_or(0);
        } else {
            self.record_paused = false;
        }
        self.measured = Some(now);
        Ok(())
    }

    /// Notes the mute state of `input`; inputs without audio are skipped.
    fn ask_mute(&mut self, obs: &mut ObsConnection, input: String) -> Result<(), ObsError> {
        match obs.request("GetInputMute", Some(json!({ "inputName": input }))) {
            Ok(answer) => {
                if let Some(muted) = answer["inputMuted"].as_bool() {
                    self.muted.insert(input, muted);
                }
                Ok(())
            }
            Err(ObsError::Request { .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Follows one event. False when OBS is shutting down.
    fn apply(
        &mut self,
        event: &ObsEvent,
        obs: &mut ObsConnection,
        now: Instant,
    ) -> Result<bool, ObsError> {
        let data = &event.data;
        let text = |key: &str| data[key].as_str().unwrap_or_default().to_string();
        match event.kind.as_str() {
            "CurrentProgramSceneChanged" => self.scene = text("sceneName"),
            "SceneNameChanged" if self.scene == text("oldSceneName") => {
                self.scene = text("sceneName");
            }
            "InputMuteStateChanged" => {
                if let Some(muted) = data["inputMuted"].as_bool() {
                    self.muted.insert(text("inputName"), muted);
                }
            }
            "InputNameChanged" => {
                if let Some(muted) = self.muted.remove(&text("oldInputName")) {
                    self.muted.insert(text("inputName"), muted);
                }
            }
            "InputRemoved" => {
                self.muted.remove(&text("inputName"));
            }
            "InputCreated" if self.muted.len() < MAX_INPUTS => {
                self.ask_mute(obs, text("inputName"))?;
            }
            "StreamStateChanged" => {
                // While it reconnects the stream counts as inactive, but it
                // is still live: a toggle now would end it.
                let state = data["outputState"].as_str().unwrap_or_default();
                let reconnecting = matches!(
                    state,
                    "OBS_WEBSOCKET_OUTPUT_RECONNECTING" | "OBS_WEBSOCKET_OUTPUT_RECONNECTED"
                );
                self.streaming = data["outputActive"] == true || reconnecting;
                self.measure_outputs(obs, now)?;
                self.stream_reconnecting = state == "OBS_WEBSOCKET_OUTPUT_RECONNECTING";
            }
            // Another collection brings other scenes and sources.
            "CurrentSceneCollectionChanged" => *self = Self::sync(obs)?,
            "RecordStateChanged" => {
                match data["outputState"].as_str() {
                    Some("OBS_WEBSOCKET_OUTPUT_PAUSED") => self.record_paused = true,
                    Some("OBS_WEBSOCKET_OUTPUT_RESUMED") => self.record_paused = false,
                    _ => self.recording = data["outputActive"] == true,
                }
                self.measure_outputs(obs, now)?;
            }
            "ReplayBufferStateChanged" => self.replay_buffer = data["outputActive"] == true,
            "VirtualcamStateChanged" => self.virtual_cam = data["outputActive"] == true,
            "ExitStarted" => return Ok(false),
            _ => {}
        }
        Ok(true)
    }
}

fn scene_name(answer: &Value) -> String {
    answer["sceneName"]
        .as_str()
        .or_else(|| answer["currentProgramSceneName"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `outputActive` of a status answer; an error (no replay buffer set up)
/// counts as off.
fn active(answer: Result<Value, ObsError>) -> bool {
    answer.is_ok_and(|answer| answer["outputActive"] == true)
}

/// The obs-websocket request for a button's command.
fn request(command: ObsCommand, target: Option<&str>) -> (&'static str, Option<Value>) {
    match command {
        ObsCommand::ToggleRecord => ("ToggleRecord", None),
        ObsCommand::StartRecord => ("StartRecord", None),
        ObsCommand::StopRecord => ("StopRecord", None),
        ObsCommand::PauseRecord => ("ToggleRecordPause", None),
        ObsCommand::ToggleStream => ("ToggleStream", None),
        ObsCommand::StartStream => ("StartStream", None),
        ObsCommand::StopStream => ("StopStream", None),
        ObsCommand::SetScene => (
            "SetCurrentProgramScene",
            Some(json!({ "sceneName": target.unwrap_or_default() })),
        ),
        ObsCommand::ToggleMute => (
            "ToggleInputMute",
            Some(json!({ "inputName": target.unwrap_or_default() })),
        ),
        ObsCommand::ToggleReplayBuffer => ("ToggleReplayBuffer", None),
        ObsCommand::SaveReplay => ("SaveReplayBuffer", None),
        ObsCommand::ToggleVirtualCam => ("ToggleVirtualCam", None),
    }
}

struct Worker {
    shared: Arc<Shared>,
    commands: Receiver<Command>,
    feedback: Sender<Feedback>,
    logger: nexus_events::EventLogger,
    load: LoadServer,
}

impl Worker {
    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn run(self) {
        let mut retry = RETRY_MIN;
        let mut retry_at = Instant::now();
        loop {
            let wanted = {
                let status = self.lock();
                if status.stop {
                    return;
                }
                status.wanted
            };
            // Commands only queue while connected; anything left was not sent.
            while let Ok(command) = self.commands.try_recv() {
                self.not_sent(command, NOT_CONNECTED);
            }
            let server = if wanted && Instant::now() >= retry_at {
                let server = (self.load)();
                self.lock().configured = server.is_some();
                server
            } else {
                None
            };
            let Some(server) = server else {
                if !wanted {
                    self.lock().problem = None;
                }
                self.wait(Duration::from_secs(1));
                continue;
            };
            match ObsConnection::connect(&server, CONNECT_TIMEOUT) {
                Ok(obs) => {
                    retry = RETRY_MIN;
                    self.session(obs, &server);
                    retry_at = Instant::now() + RETRY_MIN;
                }
                Err(error) => {
                    let wait = if error == ObsError::Auth {
                        RETRY_AUTH
                    } else {
                        retry
                    };
                    retry = (retry * 2).min(RETRY_MAX);
                    retry_at = Instant::now() + wait;
                    let problem = error.to_string();
                    let mut status = self.lock();
                    if status.problem.as_deref() != Some(problem.as_str()) {
                        drop(status);
                        log_fields(
                            &self.logger,
                            Level::Warn,
                            "obs_unavailable",
                            "could not connect to OBS",
                            [("error", problem.clone())],
                        );
                        status = self.lock();
                    }
                    status.problem = Some(problem);
                }
            }
        }
    }

    /// Sleeps up to `wait`, or until wanted or stop change.
    fn wait(&self, wait: Duration) {
        let status = self.lock();
        let _ = self.shared.wake.wait_timeout(status, wait);
    }

    fn session(&self, mut obs: ObsConnection, server: &ObsServer) {
        let mut tracked = match Tracked::sync(&mut obs) {
            Ok(tracked) => tracked,
            Err(error) => {
                self.lock().problem = Some(error.to_string());
                return;
            }
        };
        log_fields(
            &self.logger,
            Level::Info,
            "obs_connected",
            "connected to OBS",
            [("version", obs.obs_version.clone())],
        );
        {
            let mut status = self.lock();
            status.problem = None;
            status.tracked = Some(tracked.clone());
        }
        let mut config_checked = Instant::now();
        let mut heartbeat = Instant::now();
        let ended = loop {
            let wanted = {
                let status = self.lock();
                !status.stop && status.wanted
            };
            if !wanted {
                break None;
            }
            if config_checked.elapsed() >= CONFIG_EVERY {
                config_checked = Instant::now();
                if (self.load)().as_ref() != Some(server) {
                    break None;
                }
            }
            if let Err(error) = self.run_commands(&mut obs) {
                break Some(error);
            }
            let now = Instant::now();
            let running = tracked.recording || tracked.streaming;
            if running
                && tracked
                    .measured
                    .is_none_or(|at| now.duration_since(at) >= OUTPUTS_EVERY)
            {
                if let Err(error) = tracked.measure_outputs(&mut obs, now) {
                    break Some(error);
                }
            }
            if !running && now.duration_since(heartbeat) >= HEARTBEAT_EVERY {
                heartbeat = now;
                match obs.request("GetCurrentProgramScene", None) {
                    Ok(answer) => tracked.scene = scene_name(&answer),
                    Err(error) => break Some(error),
                }
            }
            match obs.next_event(EVENT_WAIT) {
                Ok(Some(event)) => match tracked.apply(&event, &mut obs, Instant::now()) {
                    Ok(true) => {}
                    Ok(false) => break Some(ObsError::Connection("OBS is closing".into())),
                    Err(error) => break Some(error),
                },
                Ok(None) => {}
                Err(error) => break Some(error),
            }
            self.lock().tracked = Some(tracked.clone());
        };
        {
            let mut status = self.lock();
            status.tracked = None;
            status.problem = ended.as_ref().map(ToString::to_string);
        }
        log_fields(
            &self.logger,
            Level::Info,
            "obs_disconnected",
            "disconnected from OBS",
            [(
                "reason",
                ended.map_or_else(|| "no longer needed".into(), |error| error.to_string()),
            )],
        );
        obs.close();
    }

    /// Reports on the strip that a button's command did not reach OBS.
    fn not_sent(&self, command: Command, why: &str) {
        log_fields(
            &self.logger,
            Level::Error,
            "action_failed",
            "action failed",
            [
                ("widget", command.label.clone()),
                ("error", why.to_string()),
            ],
        );
        let _ = self.feedback.send(Feedback {
            label: command.label,
            error: why.to_string(),
        });
    }

    /// Sends the queued commands. OBS refusing one is reported on the
    /// strip; a broken connection ends the session.
    fn run_commands(&self, obs: &mut ObsConnection) -> Result<(), ObsError> {
        while let Ok(command) = self.commands.try_recv() {
            let (kind, data) = request(command.command, command.target.as_deref());
            match obs.request(kind, data) {
                Ok(_) => log_fields(
                    &self.logger,
                    Level::Info,
                    "action_executed",
                    "action executed",
                    [
                        ("widget", command.label),
                        ("command", format!("obs {kind}")),
                    ],
                ),
                Err(error @ ObsError::Request { .. }) => self.not_sent(command, &error.to_string()),
                Err(error) => {
                    self.not_sent(command, &error.to_string());
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
