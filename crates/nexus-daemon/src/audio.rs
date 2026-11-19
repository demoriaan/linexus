//! PipeWire on the strip, off the render loop.
//!
//! While sound widgets or the privacy pill want it, a worker thread keeps
//! `pw-dump --monitor` running and folds its output into the latest state.
//! While a microphone widget with a level meter shows, it also records the
//! microphone with `pw-record` (16 kHz mono) and keeps its level. It looks
//! for processes with a camera open whenever inotify sees a camera opened
//! or closed, and every 30 seconds (every 3 while a camera cannot be
//! watched). Changes the strip asks for run on their own thread through
//! `wpctl`/`pw-metadata`.
//! Helper programs are stopped by their own PID when no longer wanted.

use crate::dispatch::Feedback;
use crate::log::{log_fields, Level};
use nexus_sensors::audio::{self, AudioCommand, AudioSnapshot, CameraOpen, Helper, PrivacyUse};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// A helper that died is started again after this long.
const RETRY: Duration = Duration::from_secs(3);
/// Cameras are looked for this often while some cannot be watched...
const CAMERA_EVERY: Duration = Duration::from_secs(3);
/// ...and this often while inotify tells when one is opened or closed.
const CAMERA_FALLBACK: Duration = Duration::from_secs(30);
/// Scans after inotify events are at least this far apart.
const CAMERA_GAP: Duration = Duration::from_secs(1);
/// The worker looks at what is wanted at least this often.
const POLL: Duration = Duration::from_millis(500);

#[derive(Default)]
struct Shared {
    status: Mutex<Status>,
    wake: Condvar,
}

#[derive(Default)]
struct Status {
    /// Follow PipeWire's graph.
    wanted: bool,
    /// Look for cameras in use too.
    cameras_wanted: bool,
    /// Record this source for the level meter.
    meter: Option<String>,
    stop: bool,
    snapshot: Option<AudioSnapshot>,
    level: Option<u8>,
    meter_pid: Option<u32>,
    open_cameras: Vec<CameraOpen>,
    /// Which run of `pw-dump` and of the meter may still publish: a reader
    /// thread that is behind writes nothing after its helper stopped.
    graph_run: u64,
    meter_run: u64,
}

impl Status {
    /// The graph's reader may publish this.
    fn graph_update(&mut self, run: u64, snapshot: AudioSnapshot) {
        if self.graph_run == run {
            self.snapshot = Some(snapshot);
        }
    }

    fn meter_update(&mut self, run: u64, level: u8) {
        if self.meter_run == run {
            self.level = Some(level);
        }
    }

    /// The graph's helper stopped: what it read is gone.
    fn graph_stopped(&mut self) {
        self.graph_run += 1;
        self.snapshot = None;
    }

    fn meter_stopped(&mut self) {
        self.meter_run += 1;
        self.level = None;
        self.meter_pid = None;
    }
}

enum Sink {
    None,
    Worker(Sender<(String, AudioCommand)>),
    /// `--dry-run-actions`: commands are logged, not run.
    DryRun(nexus_events::EventLogger),
    #[cfg(test)]
    Record(Arc<Mutex<Vec<AudioCommand>>>),
}

pub(crate) struct Audio {
    shared: Arc<Shared>,
    sink: Sink,
}

impl Audio {
    /// No worker: the sound widgets say PipeWire is not reachable.
    pub(crate) fn off() -> Self {
        Self {
            shared: Arc::default(),
            sink: Sink::None,
        }
    }

    pub(crate) fn on(
        feedback: Sender<Feedback>,
        logger: nexus_events::EventLogger,
        dry_run: bool,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let worker_logger = logger.clone();
        let started = thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(worker, worker_logger));
        if started.is_err() {
            return Self::off();
        }
        let sink = if dry_run {
            Sink::DryRun(logger)
        } else {
            let (tx, rx) = mpsc::channel::<(String, AudioCommand)>();
            let spawned = thread::Builder::new()
                .name("audio-commands".into())
                .spawn(move || {
                    for (label, command) in rx {
                        if let Err(error) = audio::run_audio_command(&command) {
                            log_fields(
                                &logger,
                                Level::Warn,
                                "audio_command_failed",
                                "sound command failed",
                                [("widget", label.clone()), ("error", error.clone())],
                            );
                            let _ = feedback.send(Feedback { label, error });
                        }
                    }
                });
            match spawned {
                Ok(_) => Sink::Worker(tx),
                Err(_) => Sink::None,
            }
        };
        Self { shared, sink }
    }

    /// A state to show without PipeWire, recording the commands sent.
    #[cfg(test)]
    pub(crate) fn fake(snapshot: AudioSnapshot) -> (Self, Arc<Mutex<Vec<AudioCommand>>>) {
        let shared = Arc::new(Shared::default());
        shared.status.lock().unwrap().snapshot = Some(snapshot);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let audio = Self {
            shared,
            sink: Sink::Record(sent.clone()),
        };
        (audio, sent)
    }

    #[cfg(test)]
    pub(crate) fn set_snapshot(&self, snapshot: AudioSnapshot) {
        self.lock().snapshot = Some(snapshot);
    }

    #[cfg(test)]
    pub(crate) fn set_level(&self, level: Option<u8>) {
        self.lock().level = level;
    }

    /// What the worker is asked to follow.
    #[cfg(test)]
    pub(crate) fn wanted(&self) -> (bool, bool, Option<String>) {
        let status = self.lock();
        (status.wanted, status.cameras_wanted, status.meter.clone())
    }

    /// What to follow: the graph, cameras, and the microphone to meter.
    pub(crate) fn set_wanted(&self, wanted: bool, cameras: bool, meter: Option<String>) {
        let mut status = self.lock();
        let meter = meter.filter(|_| wanted);
        if status.wanted != wanted
            || status.cameras_wanted != (cameras && wanted)
            || status.meter != meter
        {
            status.wanted = wanted;
            status.cameras_wanted = cameras && wanted;
            status.meter = meter;
            self.shared.wake.notify_all();
        }
    }

    pub(crate) fn snapshot(&self) -> Option<AudioSnapshot> {
        self.lock().snapshot.clone()
    }

    /// The microphone's level while the meter records.
    pub(crate) fn level(&self) -> Option<u8> {
        let status = self.lock();
        status.meter.as_ref().and(status.level)
    }

    /// Apps other than Linexus with a microphone or camera open.
    pub(crate) fn privacy(&self) -> Vec<PrivacyUse> {
        let status = self.lock();
        let Some(snapshot) = status.snapshot.as_ref() else {
            return Vec::new();
        };
        audio::privacy_uses(
            snapshot,
            &status.open_cameras,
            std::process::id(),
            status.meter_pid,
        )
    }

    pub(crate) fn send(&self, label: &str, commands: Vec<AudioCommand>) -> Result<(), String> {
        match &self.sink {
            Sink::None => Err("PipeWire is not reachable".into()),
            Sink::Worker(tx) => {
                for command in commands {
                    tx.send((label.to_string(), command))
                        .map_err(|_| "PipeWire is not reachable".to_string())?;
                }
                Ok(())
            }
            Sink::DryRun(logger) => {
                for command in commands {
                    log_fields(
                        logger,
                        Level::Info,
                        "action_dry_run",
                        "action executed",
                        [
                            ("widget", label.to_string()),
                            ("command", command.describe()),
                        ],
                    );
                }
                Ok(())
            }
            #[cfg(test)]
            Sink::Record(sent) => {
                sent.lock().unwrap().extend(commands);
                Ok(())
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Status> {
        lock(&self.shared)
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.lock().stop = true;
        self.shared.wake.notify_all();
    }
}

fn lock(shared: &Shared) -> MutexGuard<'_, Status> {
    shared.status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A helper program and when it may be started again after it ended.
#[derive(Default)]
struct Running {
    helper: Option<Helper>,
    retry_at: Option<Instant>,
}

impl Running {
    fn stop(&mut self) {
        if let Some(helper) = self.helper.take() {
            helper.stop();
        }
    }

    /// Notices a helper that ended by itself; true when it did.
    fn reap(&mut self, now: Instant) -> bool {
        if self.helper.as_mut().is_some_and(Helper::exited) {
            self.stop();
            self.retry_at = Some(now + RETRY);
            return true;
        }
        false
    }

    fn may_start(&self, now: Instant) -> bool {
        self.helper.is_none() && self.retry_at.is_none_or(|at| now >= at)
    }
}

fn run(shared: Arc<Shared>, logger: nexus_events::EventLogger) {
    let mut monitor = Running::default();
    let mut meter = Running::default();
    let mut metered: Option<String> = None;
    let mut scanned: Option<Instant> = None;
    let mut watch: Option<audio::CameraWatch> = None;
    let mut watch_failed = false;
    let mut pending = false;
    // Whether the last start of `pw-dump` failed, so that is logged once.
    let mut unavailable = false;
    loop {
        let (wanted, cameras, target) = {
            let status = lock(&shared);
            if status.stop {
                break;
            }
            (status.wanted, status.cameras_wanted, status.meter.clone())
        };
        let now = Instant::now();

        if monitor.reap(now) {
            lock(&shared).graph_stopped();
        }
        if !wanted {
            if monitor.helper.is_some() {
                monitor.stop();
                lock(&shared).graph_stopped();
            }
        } else if monitor.may_start(now) {
            match Helper::graph_monitor() {
                Ok((helper, stdout)) => {
                    if std::mem::take(&mut unavailable) {
                        crate::log::log(
                            &logger,
                            Level::Info,
                            "audio_available",
                            "following PipeWire again",
                        );
                    }
                    let run = {
                        let mut status = lock(&shared);
                        status.graph_run += 1;
                        status.graph_run
                    };
                    let reader = shared.clone();
                    let reader_logger = logger.clone();
                    let spawned = thread::Builder::new()
                        .name("audio-graph".into())
                        .spawn(move || follow(stdout, &reader, run, &reader_logger));
                    monitor.helper = Some(helper);
                    if spawned.is_err() {
                        monitor.stop();
                        lock(&shared).graph_stopped();
                    }
                }
                Err(error) => {
                    if !std::mem::replace(&mut unavailable, true) {
                        log_fields(
                            &logger,
                            Level::Warn,
                            "audio_unavailable",
                            "cannot follow PipeWire",
                            [("error", error.to_string())],
                        );
                    }
                    monitor.retry_at = Some(now + RETRY * 10);
                }
            }
        }

        // The meter follows the microphone in use.
        let ended = meter.reap(now);
        let switched = meter.helper.is_some() && target != metered;
        if switched {
            meter.stop();
            meter.retry_at = None;
        }
        if ended || switched {
            metered = None;
            lock(&shared).meter_stopped();
        }
        if let Some(source) = target.as_ref().filter(|_| meter.may_start(now)) {
            match Helper::level_meter(source, std::process::id()) {
                Ok((helper, stdout)) => {
                    let run = {
                        let mut status = lock(&shared);
                        status.meter_pid = Some(helper.pid());
                        status.meter_run += 1;
                        status.meter_run
                    };
                    let reader = shared.clone();
                    let spawned =
                        thread::Builder::new()
                            .name("audio-meter".into())
                            .spawn(move || {
                                let _ = audio::read_levels(stdout, |level| {
                                    lock(&reader).meter_update(run, level);
                                });
                                reader.wake.notify_all();
                            });
                    meter.helper = Some(helper);
                    metered = Some(source.clone());
                    if spawned.is_err() {
                        meter.stop();
                        lock(&shared).meter_stopped();
                    }
                }
                Err(_) => meter.retry_at = Some(now + RETRY * 10),
            }
        }

        // Cameras: a scan of every process's open files, run when inotify
        // sees a camera opened or closed, and on a timer besides.
        if !cameras {
            scanned = None;
            watch = None;
            watch_failed = false;
            pending = false;
            lock(&shared).open_cameras.clear();
        } else {
            if watch.is_none() && !watch_failed {
                watch = audio::CameraWatch::new(Path::new("/dev")).ok();
                watch_failed = watch.is_none();
            }
            pending |= watch.as_mut().is_some_and(audio::CameraWatch::changed);
            let every = if watch.as_ref().is_some_and(audio::CameraWatch::complete) {
                CAMERA_FALLBACK
            } else {
                CAMERA_EVERY
            };
            let due = scanned.is_none_or(|at| {
                let since = now.saturating_duration_since(at);
                since >= every || (pending && since >= CAMERA_GAP)
            });
            if due {
                scanned = Some(now);
                pending = false;
                let open = if audio::camera_present(Path::new("/dev")) {
                    let own = [std::process::id()];
                    audio::camera_users(audio::proc_root(), Path::new("/sys"), &own)
                } else {
                    Vec::new()
                };
                lock(&shared).open_cameras = open;
            }
        }

        let status = lock(&shared);
        if status.stop {
            break;
        }
        let _ = shared.wake.wait_timeout(status, POLL);
    }
    monitor.stop();
    meter.stop();
}

/// The `audio-graph` thread: folds `pw-dump`'s output into the state
/// while run `run` may publish. A document that does not parse is skipped
/// and logged, once per run.
fn follow(
    stdout: impl std::io::Read,
    shared: &Shared,
    run: u64,
    logger: &nexus_events::EventLogger,
) {
    let mut told = false;
    let ended = audio::follow_graph(
        stdout,
        |snapshot| lock(shared).graph_update(run, snapshot),
        |error| {
            if !std::mem::replace(&mut told, true) {
                log_fields(
                    logger,
                    Level::Warn,
                    "audio_graph_skipped",
                    "skipped a part of PipeWire's graph that did not parse",
                    [("error", error)],
                );
            }
        },
    );
    if let Err(error) = ended {
        log_fields(
            logger,
            Level::Warn,
            "audio_graph_failed",
            "stopped reading PipeWire's graph",
            [("error", error.to_string())],
        );
    }
    shared.wake.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_left_behind_publishes_nothing() {
        let snapshot = crate::sound::tests::desk();
        let mut status = Status {
            graph_run: 1,
            ..Status::default()
        };
        status.graph_update(1, snapshot.clone());
        assert!(status.snapshot.is_some());
        // The helper ends; its reader still has a document in hand.
        status.graph_stopped();
        status.graph_update(1, snapshot.clone());
        assert!(status.snapshot.is_none());
        // The next run publishes again.
        status.graph_run += 1;
        status.graph_update(1, snapshot.clone());
        assert!(status.snapshot.is_none());
        status.graph_update(status.graph_run, snapshot);
        assert!(status.snapshot.is_some());

        status.meter_run = 4;
        status.meter_pid = Some(42);
        status.meter_update(4, 60);
        assert_eq!(status.level, Some(60));
        status.meter_stopped();
        status.meter_update(4, 70);
        assert_eq!((status.level, status.meter_pid), (None, None));
    }
}
