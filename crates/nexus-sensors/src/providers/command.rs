//! Running helper programs with a hard deadline.
//!
//! Every provider shells out (busctl, wpctl, curl, user commands). A stuck
//! child must never stall the daemon's collection loop, and a chatty one must
//! never deadlock on a full pipe, so stdout is drained on its own thread while
//! the caller waits with a timeout.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Most a command may print before the rest is discarded (still drained, so
/// the child finishes normally instead of dying on a closed pipe).
const MAX_CAPTURE_BYTES: usize = 4 << 20;
/// A panel row shows a sentence, not a log.
const MAX_SUMMARY_CHARS: usize = 200;

/// Runs a user-configured command (e.g. a "command output" widget) and
/// returns what it printed, shortened for display.
///
/// `program` must be a bare name looked up in `PATH`; paths are refused so a
/// config entry cannot point at an arbitrary file. Arguments are passed as-is,
/// without a shell. Stderr is ignored, the child is killed when `timeout`
/// passes, and a non-zero exit is an error.
pub fn run_command_capture(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<String, String> {
    if program.is_empty()
        || program.contains('/')
        || program.contains('\0')
        || program == "."
        || program == ".."
    {
        return Err(format!(
            "\"{program}\" is not a program name; use a bare name found in PATH"
        ));
    }
    let resolved = crate::which(program).ok_or_else(|| format!("{program} is not installed"))?;
    let output = run_output(&resolved, args, timeout)?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed ({})",
            describe_status(output.status)
        ));
    }
    Ok(summarize_output(&output.stdout))
}

/// Runs `program` with stdin closed and stderr discarded, collecting stdout.
/// Errors name what went wrong: not startable, or killed at the deadline.
pub(crate) fn run_output<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
) -> Result<Output, String> {
    // Its own process group, so a timeout can kill whatever it started in
    // the background too.
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", program.display()))?;
    let deadline = Instant::now() + timeout;
    let Some(mut stdout) = child.stdout.take() else {
        kill(&mut child);
        return Err("no stdout pipe".into());
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout)
            .take(MAX_CAPTURE_BYTES as u64)
            .read_to_end(&mut buf);
        let _ = io::copy(&mut stdout, &mut io::sink());
        let _ = tx.send(buf);
    });
    // The reader finishes when every holder of the pipe is gone. A
    // backgrounded grandchild can keep it open forever, so the wait is bounded;
    // killing the whole group then closes the pipe and ends the reader.
    let Ok(stdout) = rx.recv_timeout(timeout) else {
        kill(&mut child);
        return Err(format!("{} timed out", program.display()));
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Output {
                    status,
                    stdout,
                    stderr: Vec::new(),
                })
            }
            Ok(None) if Instant::now() >= deadline => {
                kill(&mut child);
                return Err(format!("{} timed out", program.display()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Kills the child's process group (the child leads it, see `run_output`)
/// and reaps the child. The group still exists while the child is an
/// unreaped zombie, so this also reaches grandchildren after it exited.
fn kill(child: &mut std::process::Child) {
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill(2) takes plain integers; a negative pid addresses the
        // process group.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn describe_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".into(),
    }
}

/// Plain text for a panel row: colour codes and control characters removed,
/// surrounding blank lines dropped, at most [`MAX_SUMMARY_CHARS`] characters.
fn summarize_output(stdout: &[u8]) -> String {
    let text = strip_ansi(&String::from_utf8_lossy(stdout));
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let last = lines.iter().rposition(|line| !line.trim().is_empty());
    let joined = match (first, last) {
        (Some(first), Some(last)) => lines[first..=last].join("\n"),
        _ => String::new(),
    };
    joined.trim().chars().take(MAX_SUMMARY_CHARS).collect()
}

/// Removes ANSI escape sequences (CSI like `ESC[1;31m` and OSC like terminal
/// titles) and other control characters except newline and tab.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\n' | '\t' => out.push(ch),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_paths_and_unknown_programs() {
        assert!(run_command_capture("/bin/echo", &[], Duration::from_secs(1)).is_err());
        assert!(run_command_capture("../echo", &[], Duration::from_secs(1)).is_err());
        assert!(run_command_capture("", &[], Duration::from_secs(1)).is_err());
        assert!(run_command_capture(
            "definitely-not-a-real-program-4711",
            &[],
            Duration::from_secs(1)
        )
        .is_err());
    }

    #[test]
    fn summarizes_output_for_a_panel_row() {
        assert_eq!(summarize_output(b"\n\n  up 3 days  \n\n"), "up 3 days");
        assert_eq!(
            summarize_output(b"\x1b[1;32mgreen\x1b[0m line\nsecond\x07\n"),
            "green line\nsecond"
        );
        assert_eq!(summarize_output(b"\x1b]0;title\x07text"), "text");
        let long = "x".repeat(500);
        assert_eq!(summarize_output(long.as_bytes()).chars().count(), 200);
        assert_eq!(summarize_output("äöü".as_bytes()), "äöü");
        assert_eq!(summarize_output(b"   \n \n"), "");
    }

    #[test]
    fn captures_stdout_of_a_real_command() {
        let output = run_command_capture(
            "echo",
            &["hello".into(), "nexus".into()],
            Duration::from_secs(2),
        );
        assert_eq!(output.as_deref(), Ok("hello nexus"));
    }

    #[test]
    fn kills_commands_at_the_deadline() {
        let started = Instant::now();
        let result = run_command_capture("sleep", &["5".into()], Duration::from_millis(200));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn timeout_kills_backgrounded_grandchildren() {
        let pid_file =
            std::env::temp_dir().join(format!("linexus-sensors-grandchild-{}", std::process::id()));
        // `sh` exits at once; the backgrounded sleep keeps stdout open.
        let script = format!("sleep 30 & echo $! > '{}'", pid_file.display());
        let started = Instant::now();
        let result = run_command_capture("sh", &["-c".into(), script], Duration::from_millis(300));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let _ = std::fs::remove_file(&pid_file);
        let stat = format!("/proc/{}/stat", pid.trim());
        // Gone, or a zombie waiting for its new parent to reap it.
        let dead = || {
            std::fs::read_to_string(&stat).map_or(true, |stat| {
                stat.rsplit(')')
                    .next()
                    .unwrap_or("")
                    .trim_start()
                    .starts_with('Z')
            })
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !dead() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(dead(), "the backgrounded sleep survived the timeout");
    }

    #[test]
    fn reports_non_zero_exit() {
        let result = run_command_capture("false", &[], Duration::from_secs(2));
        assert!(result.unwrap_err().contains("exit code 1"));
    }
}
