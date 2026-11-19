//! Sound devices, playing apps and microphone use, read from PipeWire.
//!
//! `pw-dump --monitor` tells about every change in PipeWire's graph as
//! JSON (`pactl`'s JSON output breaks on names with umlauts). `wpctl` sets
//! defaults, volumes and mute, and `pw-metadata` moves streams that were
//! sent to an output of their own (the others follow the default by
//! themselves). The level meter records the microphone with `pw-record`,
//! and cameras in use are found in `/proc`.

mod camera;
mod graph;
mod meter;
#[cfg(test)]
mod tests;

pub use camera::{camera_present, camera_users, CameraOpen, CameraWatch};
pub use graph::Graph;
pub use meter::{level_from_peak, peak_of, read_levels, METER_RATE, METER_WINDOW};

use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// `node.name` of the level meter's capture stream; the daemon's PID
/// follows, so its own meter is never reported as someone listening.
pub const METER_NODE: &str = "linexus-level-meter";
pub const METER_APP: &str = "Linexus level meter";

/// What PipeWire has now, as far as the strip cares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioSnapshot {
    /// Sinks, most preferred first.
    pub outputs: Vec<Device>,
    /// Sources, most preferred first.
    pub inputs: Vec<Device>,
    /// `node.name` of the default sink and source.
    pub default_output: Option<String>,
    pub default_input: Option<String>,
    /// Apps' streams that play sound, and that record.
    pub playback: Vec<Stream>,
    pub recording: Vec<Stream>,
    /// Cameras PipeWire hands to apps right now.
    pub cameras: Vec<CameraUse>,
    /// `/dev/videoN` and the camera's name.
    pub video_devices: Vec<(String, String)>,
    /// App names of PipeWire's clients by process id.
    pub apps_by_pid: Vec<(u32, String)>,
}

impl AudioSnapshot {
    pub fn output(&self, node: &str) -> Option<&Device> {
        self.outputs.iter().find(|device| device.node == node)
    }

    pub fn input(&self, node: &str) -> Option<&Device> {
        self.inputs.iter().find(|device| device.node == node)
    }

    pub fn app_of_pid(&self, pid: u32) -> Option<&str> {
        self.apps_by_pid
            .iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, name)| name.as_str())
    }

    pub fn camera_name(&self, path: &str) -> Option<&str> {
        self.video_devices
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, name)| name.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeviceKind {
    Headset,
    Monitor,
    #[default]
    Speaker,
    Microphone,
}

/// A sink or source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Device {
    /// PipeWire object id, for `wpctl`; changes when PipeWire restarts.
    pub id: u32,
    /// `node.name`, which stays.
    pub node: String,
    /// Short name for a tile ("HDMI 3"), and the line below ("Navi 10 HDMI").
    pub name: String,
    pub detail: String,
    /// PipeWire's full description.
    pub description: String,
    pub kind: DeviceKind,
    /// 0..=100 % and beyond, as `wpctl` shows it.
    pub volume: u8,
    pub muted: bool,
    /// A loopback, a filter or another device without hardware behind it.
    pub virtual_device: bool,
    /// A virtual input that only gets what outputs play, such as a
    /// loopback of an output's monitor: recording it is nobody listening.
    pub monitor_only: bool,
    pub priority: i64,
    /// `object.serial`, by which streams are sent to it.
    pub serial: Option<u64>,
}

/// One stream of an app.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stream {
    pub id: u32,
    pub app: String,
    pub node: String,
    pub pid: Option<u32>,
    pub volume: u8,
    pub muted: bool,
    /// Running and not paused.
    pub active: bool,
    /// Nodes it is linked to: the output it plays on, or what it records.
    pub peers: Vec<u32>,
    /// The node it was sent to (`target.object` in its properties or in
    /// PipeWire's metadata); without one it follows the default.
    pub target: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraUse {
    pub app: String,
    pub pid: Option<u32>,
    pub device: String,
}

/// One app with a microphone or camera open, for the privacy badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyUse {
    pub app: String,
    pub camera: bool,
    /// The microphone's `node.name`, or the camera's name.
    pub device: String,
}

/// Turns a program name into an app name: `obs` → `OBS`,
/// `firefox` → `Firefox`, `valheim.exe` → `Valheim`.
pub fn pretty_app_name(binary: &str) -> String {
    let base = binary.trim().trim_end_matches(" (deleted)");
    let base = base.rsplit('/').next().unwrap_or(base);
    let mut base = base;
    for suffix in [".exe", ".bin", "-bin", "-wrapped", ".AppImage"] {
        if let Some(rest) = base.strip_suffix(suffix) {
            base = rest;
        }
    }
    match base.to_ascii_lowercase().as_str() {
        "steamwebhelper" => return "Steam".to_string(),
        "chrome" | "google-chrome" => return "Chrome".to_string(),
        "" => return binary.to_string(),
        _ => {}
    }
    if base.chars().count() <= 3 {
        return base.to_uppercase();
    }
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Apps that record from a microphone or use a camera, leaving out
/// Linexus's own level meter (`own_pid` is the daemon's, `meter_pid` the
/// recorder's) and recordings of what an output plays. A virtual
/// microphone (noise filter, echo cancelling, loopback) counts unless all
/// it gets is what outputs play.
pub fn privacy_uses(
    snapshot: &AudioSnapshot,
    open_cameras: &[CameraOpen],
    own_pid: u32,
    meter_pid: Option<u32>,
) -> Vec<PrivacyUse> {
    let own_meter = format!("{METER_NODE}-{own_pid}");
    let mut uses: Vec<PrivacyUse> = Vec::new();
    let mut add = |found: PrivacyUse| {
        if !uses.contains(&found) {
            uses.push(found);
        }
    };
    for stream in &snapshot.recording {
        let own = stream.node == own_meter
            || stream
                .pid
                .is_some_and(|pid| pid == own_pid || Some(pid) == meter_pid);
        if own || !stream.active {
            continue;
        }
        // Linked from a source, not from an output's monitor.
        let source = stream.peers.iter().find_map(|peer| {
            snapshot
                .inputs
                .iter()
                .find(|input| input.id == *peer && !input.monitor_only)
        });
        if let Some(source) = source {
            add(PrivacyUse {
                app: stream.app.clone(),
                camera: false,
                device: source.node.clone(),
            });
        }
    }
    for camera in &snapshot.cameras {
        if camera.pid.is_some_and(|pid| pid == own_pid) {
            continue;
        }
        add(PrivacyUse {
            app: camera.app.clone(),
            camera: true,
            device: camera.device.clone(),
        });
    }
    for open in open_cameras {
        if open.pid == own_pid {
            continue;
        }
        let app = snapshot
            .app_of_pid(open.pid)
            .map(str::to_string)
            .unwrap_or_else(|| pretty_app_name(&open.program));
        let device = snapshot
            .camera_name(&open.path)
            .map(str::to_string)
            .unwrap_or_else(|| open.name.clone());
        add(PrivacyUse {
            app,
            camera: true,
            device,
        });
    }
    uses
}

/// A change to PipeWire's state the strip asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCommand {
    /// Makes a sink or source the default.
    SetDefault { id: u32 },
    /// Sets a node's volume in percent.
    SetVolume { id: u32, percent: u8 },
    /// Mutes or unmutes a node; `None` toggles.
    SetMute { id: u32, muted: Option<bool> },
    /// Sends a stream that was sent to an output of its own to another,
    /// by the output's `object.serial`; without one the stream follows
    /// the default again.
    MoveStream { stream: u32, serial: Option<u64> },
}

impl AudioCommand {
    /// The program and its arguments.
    pub fn argv(&self) -> (&'static str, Vec<String>) {
        match self {
            Self::SetDefault { id } => ("wpctl", vec!["set-default".into(), id.to_string()]),
            Self::SetVolume { id, percent } => (
                "wpctl",
                vec![
                    "set-volume".into(),
                    id.to_string(),
                    format!("{}%", (*percent).min(150)),
                ],
            ),
            Self::SetMute { id, muted } => (
                "wpctl",
                vec![
                    "set-mute".into(),
                    id.to_string(),
                    match muted {
                        Some(true) => "1",
                        Some(false) => "0",
                        None => "toggle",
                    }
                    .into(),
                ],
            ),
            // The same metadata `pactl move-sink-input` writes; WirePlumber
            // relinks the stream at once.
            Self::MoveStream {
                stream,
                serial: Some(serial),
            } => (
                "pw-metadata",
                vec![
                    "-n".into(),
                    "default".into(),
                    stream.to_string(),
                    "target.object".into(),
                    serial.to_string(),
                    "Spa:Id".into(),
                ],
            ),
            Self::MoveStream {
                stream,
                serial: None,
            } => (
                "pw-metadata",
                vec![
                    "-n".into(),
                    "default".into(),
                    "-d".into(),
                    stream.to_string(),
                    "target.object".into(),
                ],
            ),
        }
    }

    /// The command as one line, for logs.
    pub fn describe(&self) -> String {
        let (program, args) = self.argv();
        format!("{program} {}", args.join(" "))
    }
}

/// Runs a command and waits for it, at most two seconds.
pub fn run_audio_command(command: &AudioCommand) -> Result<(), String> {
    let (program, args) = command.argv();
    let path = crate::which(program).ok_or_else(|| format!("{program} is not installed"))?;
    let output = crate::providers::command::run_output(&path, &args, Duration::from_secs(2))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{} failed", command.describe()))
    }
}

/// The graph as it is now, from one `pw-dump`.
pub fn read_snapshot() -> Result<AudioSnapshot, String> {
    let path = crate::which("pw-dump").ok_or("pw-dump is not installed")?;
    let output =
        crate::providers::command::run_output(&path, &["--no-colors"], Duration::from_secs(3))?;
    if !output.status.success() {
        return Err("pw-dump failed; is PipeWire running?".into());
    }
    Ok(Graph::from_dump(&String::from_utf8_lossy(&output.stdout))?.snapshot())
}

/// A helper program whose output the audio worker reads until it stops it.
pub struct Helper {
    child: Child,
}

impl Helper {
    fn spawn(program: &str, args: &[String]) -> io::Result<(Self, ChildStdout)> {
        let path = crate::which(program).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{program} is not installed"),
            )
        })?;
        let parent = std::process::id();
        let mut command = Command::new(path);
        command
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: between fork and exec the closure calls only prctl and
        // getppid, which are async-signal-safe, and allocates nothing.
        unsafe {
            command.pre_exec(move || {
                // The helper ends with the thread that started it, even
                // when the daemon is killed and cannot stop it.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // The daemon died before prctl took hold.
                if u32::try_from(libc::getppid()).ok() != Some(parent) {
                    return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("no stdout pipe"));
        };
        Ok((Self { child }, stdout))
    }

    /// `pw-dump --monitor`: the whole graph, then every change.
    pub fn graph_monitor() -> io::Result<(Self, ChildStdout)> {
        Self::spawn("pw-dump", &["--monitor".into(), "--no-colors".into()])
    }

    /// `pw-record` of `source` as 16 kHz mono, for the level meter. The
    /// stream carries the daemon's PID in its node name.
    pub fn level_meter(source: &str, own_pid: u32) -> io::Result<(Self, ChildStdout)> {
        Self::spawn("pw-record", &meter_args(source, own_pid))
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether it ended by itself.
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Ends it, by its own PID only.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Helper {
    /// A helper never outlives its handle: it is killed and reaped, by the
    /// child handle (no PID that could have been reused).
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `pw-record`'s arguments for the level meter. `stream.monitor` marks it
/// as a peak meter: WirePlumber does not switch a Bluetooth headset to
/// its call profile for it.
pub fn meter_args(source: &str, own_pid: u32) -> Vec<String> {
    let props = format!(
        "{{ application.name = \"{METER_APP}\" media.name = \"{METER_APP}\" \
         node.name = \"{METER_NODE}-{own_pid}\" node.dont-reconnect = true \
         stream.monitor = true }}"
    );
    vec![
        "--target".into(),
        source.to_string(),
        "--rate".into(),
        METER_RATE.to_string(),
        "--channels".into(),
        "1".into(),
        "--format".into(),
        "s16".into(),
        "--latency".into(),
        "64ms".into(),
        "--raw".into(),
        "-P".into(),
        props,
        "-".into(),
    ]
}

/// Reads `pw-dump --monitor` output and calls `changed` with every new
/// state that differs from the one before. Returns when the output ends.
///
/// Bytes that are no UTF-8 (a Latin-1 window title in `media.name`) become
/// U+FFFD; a document that still does not parse is passed to `skipped`
/// with the reason and left out, and the next one is read.
pub fn follow_graph(
    reader: impl Read,
    mut changed: impl FnMut(AudioSnapshot),
    mut skipped: impl FnMut(String),
) -> io::Result<()> {
    let mut graph = Graph::default();
    let mut last = None;
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut document = String::new();
    let mut depth = Depth::default();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line);
        let mut rest: &str = &text;
        while let Some(end) = depth.end_in(rest) {
            document.push_str(&rest[..end]);
            rest = &rest[end..];
            match serde_json::from_str::<serde_json::Value>(&document) {
                Ok(parsed) => {
                    graph.apply(&parsed);
                    let snapshot = graph.snapshot();
                    if last.as_ref() != Some(&snapshot) {
                        last = Some(snapshot.clone());
                        changed(snapshot);
                    }
                }
                Err(error) => skipped(error.to_string()),
            }
            document.clear();
            depth = Depth::default();
        }
        document.push_str(rest);
        if document.len() > DOCUMENT_MAX {
            skipped(format!("a document longer than {DOCUMENT_MAX} bytes"));
            document.clear();
            depth = Depth::default();
        }
    }
}

/// A `pw-dump` document is never this long; past it the reader starts over.
const DOCUMENT_MAX: usize = 64 << 20;

/// How deep in brackets the text read so far is, outside strings, to tell
/// where one of `pw-dump`'s documents ends.
#[derive(Default)]
struct Depth {
    open: usize,
    started: bool,
    in_string: bool,
    escaped: bool,
}

impl Depth {
    /// Takes in `text` up to the end of a document, and returns where that
    /// end is (the byte after it), if it is in `text`.
    fn end_in(&mut self, text: &str) -> Option<usize> {
        for (at, c) in text.char_indices() {
            if self.in_string {
                match c {
                    _ if self.escaped => self.escaped = false,
                    '\\' => self.escaped = true,
                    '"' => self.in_string = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => self.in_string = true,
                '[' | '{' => {
                    self.open += 1;
                    self.started = true;
                }
                ']' | '}' => {
                    self.open = self.open.saturating_sub(1);
                    if self.started && self.open == 0 {
                        return Some(at + c.len_utf8());
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// The `/proc` the camera scan reads.
pub fn proc_root() -> &'static Path {
    Path::new("/proc")
}
