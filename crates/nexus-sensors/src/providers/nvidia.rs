//! NVIDIA GPU readings. The proprietary driver publishes neither hwmon
//! sensors nor load in sysfs, so load, VRAM, temperature, power and fan
//! come from `nvidia-smi --query-gpu`.
//!
//! On a desktop one nvidia-smi keeps running and prints a sample every
//! second (`--loop-ms`), which costs far less than starting it for each
//! reading; it stops again when nobody has asked for a while, and is
//! restarted when it stops printing. A GPU under runtime power management
//! (hybrid laptops) sleeps whenever no program uses it, and a running
//! nvidia-smi would keep it awake. Such a GPU is only asked when sysfs says
//! it is awake right then, and seldom enough (twice its autosuspend delay,
//! counted across all Linexus processes) that it can go back to sleep in
//! between.

use super::command::run_output;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const QUERY: &str =
    "--query-gpu=pci.bus_id,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed";
const FORMAT: &str = "--format=csv,noheader,nounits";
const LOOP_MS: &str = "--loop-ms=1000";
/// A sample older than this is gone (nvidia-smi stopped or hangs).
const FRESH: Duration = Duration::from_secs(3);
/// A running nvidia-smi that printed nothing for this long is replaced; the
/// wait doubles with each replacement that does not help.
const HUNG_AFTER: Duration = Duration::from_secs(10);
const HUNG_AFTER_MAX: Duration = Duration::from_secs(300);
/// The shared answer of a one-off query is never longer than this.
const MAX_SHARED_BYTES: u64 = 64 << 10;
/// The running nvidia-smi stops when nobody asked for this long.
const IDLE_STOP: Duration = Duration::from_secs(15);
/// How long the first reading may wait for a just started nvidia-smi.
const FIRST_SAMPLE_WAIT: Duration = Duration::from_millis(1500);
const RESTART_MIN: Duration = Duration::from_secs(1);
const RESTART_MAX: Duration = Duration::from_secs(60);
/// Shortest gap between two queries of a GPU that sleeps when idle.
const SLEEPY_EVERY_MIN: Duration = Duration::from_secs(10);
const RESCAN_EVERY: Duration = Duration::from_secs(30);
const MAX_LINE_BYTES: u64 = 4096;

/// One GPU as nvidia-smi reports it. Missing values (`[N/A]`) are None.
#[derive(Debug, Clone, PartialEq)]
pub struct NvidiaGpu {
    /// PCI address as sysfs writes it, e.g. `0000:01:00.0`.
    pub address: String,
    /// Load as a fraction (0..1).
    pub usage: Option<f32>,
    pub vram_used_mib: Option<f32>,
    pub vram_total_mib: Option<f32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    /// Fan speed in percent of its maximum; NVIDIA does not report RPM.
    pub fan_percent: Option<f32>,
}

impl NvidiaGpu {
    pub fn vram_usage(&self) -> Option<f32> {
        let total = self.vram_total_mib.filter(|total| *total > 0.0)?;
        Some((self.vram_used_mib? / total).clamp(0.0, 1.0))
    }
}

/// An NVIDIA display device bound to the proprietary driver.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PciGpu {
    address: String,
    /// Its `power` directory in sysfs.
    power: PathBuf,
    /// Runtime power management is on (`power/control` is `auto`) and the
    /// GPU is not the boot display: the graphics card of a hybrid laptop.
    /// A desktop's own display GPU keeps the faster loop even when a power
    /// tool turned runtime management on.
    sleeps_when_idle: bool,
    autosuspend: Option<Duration>,
}

impl PciGpu {
    /// Awake right now. Reading sysfs does not wake the device.
    fn awake(&self) -> bool {
        fs::read_to_string(self.power.join("runtime_status"))
            .is_ok_and(|status| status.trim() == "active")
    }
}

/// The NVIDIA GPU with the most video memory, or None when there is no
/// NVIDIA GPU on the proprietary driver, nvidia-smi is missing, or the GPU
/// sleeps.
pub fn read() -> Option<NvidiaGpu> {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    if state
        .scanned
        .is_none_or(|at| now.duration_since(at) > RESCAN_EVERY)
    {
        state.gpus = scan_pci(Path::new("/sys/bus/pci/devices"));
        state.gate = std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("linexus").join("nvidia-query"));
        state.program = if state.gpus.is_empty() {
            None
        } else {
            crate::which("nvidia-smi")
        };
        state.scanned = Some(now);
    }
    let Some(program) = state.program.clone() else {
        state.stop_loop();
        return None;
    };
    let gpus = if state.gpus.iter().any(|gpu| gpu.sleeps_when_idle) {
        state.stop_loop();
        state.read_sleepy(&program, now)
    } else {
        state.read_looping(&program, now)
    };
    largest(gpus)
}

/// An NVIDIA GPU on the proprietary driver exists, whether or not its
/// readings are available.
pub fn present() -> bool {
    !scan_pci(Path::new("/sys/bus/pci/devices")).is_empty()
}

static STATE: Mutex<State> = Mutex::new(State {
    gpus: Vec::new(),
    scanned: None,
    program: None,
    looping: None,
    sleepy: None,
    gate: None,
    hung_after: HUNG_AFTER,
});

struct State {
    gpus: Vec<PciGpu>,
    scanned: Option<Instant>,
    program: Option<PathBuf>,
    looping: Option<Arc<Loop>>,
    /// Last one-off query of a GPU that sleeps when idle.
    sleepy: Option<(Instant, Vec<NvidiaGpu>)>,
    /// File with the answer of the last one-off query by any Linexus
    /// process (daemon, Studio, linexusctl); its age tells when that was.
    gate: Option<PathBuf>,
    hung_after: Duration,
}

impl State {
    fn stop_loop(&mut self) {
        if let Some(running) = self.looping.take() {
            running.stop();
        }
    }

    fn read_looping(&mut self, program: &Path, now: Instant) -> Vec<NvidiaGpu> {
        if let Some(running) = &self.looping {
            if running.ended() {
                self.looping = None;
            } else if running.stopped() {
                // An nvidia-smi stuck in the kernel ignores the kill, and a
                // new one would get stuck beside it: wait for this one.
                return Vec::new();
            } else if running.hung(now, self.hung_after) {
                running.stop();
                self.hung_after = (self.hung_after * 2).min(HUNG_AFTER_MAX);
                return Vec::new();
            }
        }
        let running = match &self.looping {
            Some(running) => running.clone(),
            None => {
                let running = Loop::start(program.to_path_buf());
                self.looping = Some(running.clone());
                running
            }
        };
        running.touch(now);
        let deadline = now + FIRST_SAMPLE_WAIT;
        loop {
            let samples = running.fresh(Instant::now());
            if !samples.is_empty() {
                self.hung_after = HUNG_AFTER;
                return samples;
            }
            if running.ended() || !running.starting() {
                return samples;
            }
            if Instant::now() >= deadline {
                return samples;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn read_sleepy(&mut self, program: &Path, now: Instant) -> Vec<NvidiaGpu> {
        // nvidia-smi asks every GPU, and asking a sleeping one wakes it.
        if !self.gpus.iter().all(PciGpu::awake) {
            self.sleepy = None;
            return Vec::new();
        }
        let every = self
            .gpus
            .iter()
            .filter_map(|gpu| gpu.autosuspend)
            .max()
            .map_or(SLEEPY_EVERY_MIN, |delay| (delay * 2).max(SLEEPY_EVERY_MIN));
        let own = self
            .sleepy
            .as_ref()
            .filter(|(at, _)| now.duration_since(*at) < every);
        if let Some((_, gpus)) = own {
            return gpus.clone();
        }
        // A Linexus process asked recently: its answer stands, and its
        // query keeps the GPU awake for a while too.
        if let Some(shared) = self
            .gate
            .as_deref()
            .and_then(|gate| shared_answer(gate, every))
        {
            return parse(&shared);
        }
        let output = run_output(program, &[QUERY, FORMAT], Duration::from_secs(3))
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        if let Some(gate) = &self.gate {
            share(gate, &output);
        }
        let gpus = parse(&output);
        self.sleepy = Some((now, gpus.clone()));
        gpus
    }
}

/// The answer in the gate file, when it is younger than `every`.
fn shared_answer(gate: &Path, every: Duration) -> Option<String> {
    let file = fs::File::open(gate).ok()?;
    let age = file.metadata().ok()?.modified().ok()?.elapsed().ok()?;
    if age >= every {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_SHARED_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// Replaces the gate file's answer in one step, so readers never see half.
fn share(gate: &Path, answer: &str) {
    let Some(dir) = gate.parent() else {
        return;
    };
    let _ = fs::create_dir_all(dir);
    let partial = gate.with_extension(format!("{}.part", std::process::id()));
    if fs::write(&partial, answer).is_err() || fs::rename(&partial, gate).is_err() {
        let _ = fs::remove_file(&partial);
    }
}

fn largest(gpus: Vec<NvidiaGpu>) -> Option<NvidiaGpu> {
    gpus.into_iter().max_by(|a, b| {
        a.vram_total_mib
            .unwrap_or(0.0)
            .total_cmp(&b.vram_total_mib.unwrap_or(0.0))
    })
}

/// A long-running `nvidia-smi --loop-ms` and its latest sample per GPU.
struct Loop {
    inner: Mutex<LoopState>,
}

struct LoopState {
    samples: HashMap<String, (Instant, NvidiaGpu)>,
    asked: Instant,
    started: Instant,
    /// When nvidia-smi last printed a sample (or the loop started).
    printed: Instant,
    child: Option<Child>,
    stopped: bool,
    ended: bool,
}

impl Loop {
    fn start(program: PathBuf) -> Arc<Loop> {
        let now = Instant::now();
        let running = Arc::new(Loop {
            inner: Mutex::new(LoopState {
                samples: HashMap::new(),
                asked: now,
                started: now,
                printed: now,
                child: None,
                stopped: false,
                ended: false,
            }),
        });
        let worker = running.clone();
        let spawned = thread::Builder::new()
            .name("nvidia-smi".into())
            .spawn(move || worker.supervise(&program));
        if spawned.is_err() {
            running.lock().ended = true;
        }
        running
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LoopState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn touch(&self, now: Instant) {
        self.lock().asked = now;
    }

    fn ended(&self) -> bool {
        self.lock().ended
    }

    /// The first sample may still be on its way.
    fn starting(&self) -> bool {
        self.lock().started.elapsed() < FIRST_SAMPLE_WAIT
    }

    fn stopped(&self) -> bool {
        self.lock().stopped
    }

    /// nvidia-smi runs but has printed nothing for `after` (a GPU that fell
    /// off the bus can leave it stuck). Not while it waits to restart.
    fn hung(&self, now: Instant, after: Duration) -> bool {
        let state = self.lock();
        state.child.is_some() && now.saturating_duration_since(state.printed) > after
    }

    fn fresh(&self, now: Instant) -> Vec<NvidiaGpu> {
        self.lock()
            .samples
            .values()
            .filter(|(at, _)| now.duration_since(*at) < FRESH)
            .map(|(_, gpu)| gpu.clone())
            .collect()
    }

    fn stop(&self) {
        let mut state = self.lock();
        state.stopped = true;
        if let Some(child) = state.child.as_mut() {
            let _ = child.kill();
        }
    }

    /// Keeps nvidia-smi running while someone asks for readings: restarts
    /// it after 1 s when it exits, doubling up to 60 s while it keeps
    /// failing (no driver loaded, for example).
    fn supervise(&self, program: &Path) {
        let mut backoff = RESTART_MIN;
        loop {
            let started = Instant::now();
            let stdout = {
                let mut state = self.lock();
                if state.stopped || state.asked.elapsed() > IDLE_STOP {
                    break;
                }
                match spawn(program) {
                    Some(mut child) => {
                        let stdout = child.stdout.take();
                        state.child = Some(child);
                        state.printed = Instant::now();
                        stdout
                    }
                    None => None,
                }
            };
            if let Some(stdout) = stdout {
                self.read_samples(stdout);
            }
            if let Some(mut child) = self.lock().child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if started.elapsed() > RESTART_MAX {
                backoff = RESTART_MIN;
            }
            let wake = Instant::now() + backoff;
            while Instant::now() < wake {
                let state = self.lock();
                if state.stopped || state.asked.elapsed() > IDLE_STOP {
                    break;
                }
                drop(state);
                thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(RESTART_MAX);
        }
        let mut state = self.lock();
        state.ended = true;
        state.samples.clear();
    }

    /// Reads samples until nvidia-smi exits or nobody asks anymore.
    fn read_samples(&self, stdout: std::process::ChildStdout) {
        let mut reader = BufReader::new(stdout);
        let mut raw = Vec::new();
        loop {
            raw.clear();
            match reader
                .by_ref()
                .take(MAX_LINE_BYTES)
                .read_until(b'\n', &mut raw)
            {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if raw.last() != Some(&b'\n') && raw.len() as u64 >= MAX_LINE_BYTES {
                if reader.skip_until(b'\n').is_err() {
                    return;
                }
                continue;
            }
            let mut state = self.lock();
            if state.stopped || state.asked.elapsed() > IDLE_STOP {
                return;
            }
            state.printed = Instant::now();
            if let Some(gpu) = parse_line(&String::from_utf8_lossy(&raw)) {
                state
                    .samples
                    .insert(gpu.address.clone(), (Instant::now(), gpu));
            }
        }
    }
}

fn spawn(program: &Path) -> Option<Child> {
    Command::new(program)
        .args([QUERY, FORMAT, LOOP_MS])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// Display devices from NVIDIA (vendor 0x10de, class 0x03xxxx) that the
/// proprietary `nvidia` driver runs. Nouveau publishes hwmon sensors and is
/// read like any other chip.
fn scan_pci(devices: &Path) -> Vec<PciGpu> {
    let Ok(entries) = fs::read_dir(devices) else {
        return Vec::new();
    };
    let read = |path: &Path| {
        fs::read_to_string(path)
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    };
    let mut gpus: Vec<PciGpu> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if read(&path.join("vendor")) != "0x10de"
                || !read(&path.join("class")).starts_with("0x03")
            {
                return None;
            }
            let driver = fs::read_link(path.join("driver")).ok()?;
            if driver.file_name()? != "nvidia" {
                return None;
            }
            let power = path.join("power");
            Some(PciGpu {
                address: entry.file_name().to_string_lossy().to_lowercase(),
                sleeps_when_idle: read(&power.join("control")) == "auto"
                    && read(&path.join("boot_vga")) != "1",
                autosuspend: read(&power.join("autosuspend_delay_ms"))
                    .parse::<u64>()
                    .ok()
                    .map(Duration::from_millis),
                power,
            })
        })
        .collect();
    gpus.sort_by(|a, b| a.address.cmp(&b.address));
    gpus
}

fn parse(output: &str) -> Vec<NvidiaGpu> {
    output.lines().filter_map(parse_line).collect()
}

/// One line of `pci.bus_id, utilization.gpu, memory.used, memory.total,
/// temperature.gpu, power.draw, fan.speed`.
fn parse_line(line: &str) -> Option<NvidiaGpu> {
    let fields: Vec<&str> = line.trim().split(',').map(str::trim).collect();
    let [bus_id, usage, used, total, temp, power, fan] = fields[..] else {
        return None;
    };
    let number = |text: &str| text.parse::<f32>().ok().filter(|value| value.is_finite());
    Some(NvidiaGpu {
        address: sysfs_address(bus_id)?,
        usage: number(usage).map(|percent| (percent / 100.0).clamp(0.0, 1.0)),
        vram_used_mib: number(used),
        vram_total_mib: number(total),
        temp_c: number(temp),
        power_w: number(power),
        fan_percent: number(fan),
    })
}

/// nvidia-smi writes `00000000:01:00.0`, sysfs `0000:01:00.0`.
fn sysfs_address(bus_id: &str) -> Option<String> {
    let (domain, rest) = bus_id.split_once(':')?;
    let domain = u32::from_str_radix(domain, 16).ok()?;
    if rest.len() != 7 || !rest.is_ascii() {
        return None;
    }
    Some(format!("{domain:04x}:{}", rest.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn parses_samples_and_missing_values() {
        let output = "00000000:01:00.0, 37, 2048, 8192, 54, 112.45, 41\n\
                      00000000:0A:00.0, [N/A], [N/A], 24576, 40, [Not Supported], [N/A]\n\
                      garbage line\n";
        let gpus = parse(output);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].address, "0000:01:00.0");
        assert_eq!(gpus[0].usage, Some(0.37));
        assert_eq!(gpus[0].vram_usage(), Some(0.25));
        assert_eq!(gpus[0].temp_c, Some(54.0));
        assert_eq!(gpus[0].power_w, Some(112.45));
        assert_eq!(gpus[0].fan_percent, Some(41.0));
        assert_eq!(gpus[1].address, "0000:0a:00.0");
        assert_eq!(gpus[1].usage, None);
        assert_eq!(gpus[1].vram_usage(), None);
        assert_eq!(gpus[1].power_w, None);
        assert_eq!(largest(gpus).unwrap().address, "0000:0a:00.0");
        assert_eq!(sysfs_address("bogus"), None);
        assert_eq!(parse_line("00000000:01:00.0, 1, 2"), None);
    }

    /// A PCI device: (address, vendor, class, driver, power/control,
    /// power/runtime_status, boot_vga).
    type Device<'a> = (
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
    );

    fn fake_device(root: &Path, device: Device) {
        let (address, vendor, class, driver, control, status, boot_vga) = device;
        let dir = root.join("devices").join(address);
        fs::create_dir_all(dir.join("power")).unwrap();
        fs::create_dir_all(root.join("drivers").join(driver)).unwrap();
        fs::write(dir.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(dir.join("class"), format!("{class}\n")).unwrap();
        fs::write(dir.join("boot_vga"), format!("{boot_vga}\n")).unwrap();
        fs::write(dir.join("power/control"), format!("{control}\n")).unwrap();
        fs::write(dir.join("power/runtime_status"), format!("{status}\n")).unwrap();
        fs::write(dir.join("power/autosuspend_delay_ms"), "8000\n").unwrap();
        symlink(root.join("drivers").join(driver), dir.join("driver")).unwrap();
    }

    #[test]
    fn finds_gpus_on_the_proprietary_driver() {
        let root = std::env::temp_dir().join(format!("nexus-nvidia-pci-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for device in [
            // A desktop card driving the screen, runtime management on.
            (
                "0000:01:00.0",
                "0x10de",
                "0x030000",
                "nvidia",
                "auto",
                "active",
                "1",
            ),
            (
                "0000:01:00.1",
                "0x10de",
                "0x040300",
                "snd_hda_intel",
                "auto",
                "active",
                "0",
            ),
            (
                "0000:02:00.0",
                "0x10de",
                "0x030200",
                "nouveau",
                "auto",
                "active",
                "0",
            ),
            (
                "0000:03:00.0",
                "0x1002",
                "0x030000",
                "amdgpu",
                "auto",
                "active",
                "0",
            ),
            // A laptop's graphics card next to the boot display.
            (
                "0000:04:00.0",
                "0x10de",
                "0x030200",
                "nvidia",
                "auto",
                "suspended",
                "0",
            ),
        ] {
            fake_device(&root, device);
        }
        let devices = root.join("devices");
        let gpus = scan_pci(&devices);
        let summary: Vec<_> = gpus
            .iter()
            .map(|gpu| {
                (
                    gpu.address.as_str(),
                    gpu.sleeps_when_idle,
                    gpu.awake(),
                    gpu.autosuspend,
                )
            })
            .collect();
        let delay = Some(Duration::from_millis(8000));
        assert_eq!(
            summary,
            [
                ("0000:01:00.0", false, true, delay),
                ("0000:04:00.0", true, false, delay)
            ]
        );
        assert_eq!(gpus[1].power, devices.join("0000:04:00.0/power"));
        let _ = fs::remove_dir_all(&root);
    }

    fn fake_smi(dir: &Path, script: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("nvidia-smi");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_sleeping_gpu_is_not_asked() {
        let dir = std::env::temp_dir().join(format!("nexus-nvidia-sleepy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log = dir.join("calls");
        let smi = fake_smi(
            &dir,
            &format!(
                "echo \"$@\" >> {}\necho '00000000:01:00.0, 12, 100, 4096, 45, 9.5, [N/A]'",
                log.display()
            ),
        );
        let power = dir.join("power");
        fs::create_dir_all(&power).unwrap();
        let status = |text: &str| fs::write(power.join("runtime_status"), text).unwrap();
        let gate = dir.join("run/nvidia-query");
        let mut state = State {
            gpus: vec![PciGpu {
                address: "0000:01:00.0".into(),
                power: power.clone(),
                sleeps_when_idle: true,
                autosuspend: Some(Duration::from_secs(8)),
            }],
            scanned: None,
            program: None,
            looping: None,
            sleepy: None,
            gate: Some(gate.clone()),
            hung_after: HUNG_AFTER,
        };
        let now = Instant::now();
        status("suspended\n");
        assert!(state.read_sleepy(&smi, now).is_empty());
        assert!(!log.exists() && !gate.exists());
        status("active\n");
        let gpus = state.read_sleepy(&smi, now);
        assert_eq!(gpus[0].usage, Some(0.12));
        assert!(gate.exists());
        // Within twice the autosuspend delay the last answer stands.
        assert_eq!(state.read_sleepy(&smi, now + Duration::from_secs(15)), gpus);
        let calls = fs::read_to_string(&log).unwrap();
        assert_eq!(calls.lines().count(), 1);
        assert!(!calls.contains("loop"));
        // Asleep again: nothing, and no query, even when one is due.
        status("suspended\n");
        assert!(state
            .read_sleepy(&smi, now + Duration::from_secs(17))
            .is_empty());
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
        // Awake, and a process asked a moment ago (the gate is fresh): its
        // answer stands, without a new query.
        status("active\n");
        assert_eq!(state.read_sleepy(&smi, now + Duration::from_secs(17)), gpus);
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
        // Once the gate is old, the next query runs.
        let old = std::time::SystemTime::now() - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&gate)
            .unwrap()
            .set_modified(old)
            .unwrap();
        state.read_sleepy(&smi, now + Duration::from_secs(34));
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_loop_reports_samples_and_stops_when_asked() {
        let dir = std::env::temp_dir().join(format!("nexus-nvidia-loop-{}", std::process::id()));
        let smi = fake_smi(
            &dir,
            "while true; do echo '00000000:01:00.0, 80, 6144, 8192, 70, 250.0, 60'; sleep 0.2; done",
        );
        let mut state = State {
            gpus: vec![PciGpu {
                address: "0000:01:00.0".into(),
                power: PathBuf::new(),
                sleeps_when_idle: false,
                autosuspend: None,
            }],
            scanned: None,
            program: None,
            looping: None,
            sleepy: None,
            gate: None,
            hung_after: HUNG_AFTER,
        };
        let gpus = state.read_looping(&smi, Instant::now());
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vram_usage(), Some(0.75));
        assert_eq!(gpus[0].fan_percent, Some(60.0));
        let running = state.looping.clone().unwrap();
        // One that stopped printing is stopped; a new one starts only once
        // it has ended, and the next one gets longer before it counts as hung.
        running.lock().printed = Instant::now() - HUNG_AFTER - Duration::from_secs(1);
        assert!(state.read_looping(&smi, Instant::now()).is_empty());
        assert_eq!(state.hung_after, HUNG_AFTER * 2);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !running.ended() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(running.ended());
        assert_eq!(state.read_looping(&smi, Instant::now()).len(), 1);
        assert_eq!(state.hung_after, HUNG_AFTER);
        let running = state.looping.clone().unwrap();
        state.stop_loop();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !running.ended() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(running.ended());
        let _ = fs::remove_dir_all(&dir);
    }
}
