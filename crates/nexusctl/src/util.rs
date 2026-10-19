//! Small process and filesystem helpers shared by the subcommands.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|meta| meta.is_file() && (meta.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

/// `name` next to the running linexusctl binary, else from PATH.
pub(crate) fn find_sibling_or_path_binary(name: &str) -> Option<PathBuf> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| is_executable(path));
    sibling.or_else(|| which(name).filter(|path| is_executable(path)))
}

pub(crate) fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Stdout of a successful command run.
pub(crate) fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// `systemctl [--user] <args>` answer such as `active` or `enabled`.
pub(crate) fn systemctl_query(user: bool, args: &[&str]) -> String {
    let Some(systemctl) = which("systemctl") else {
        return "systemctl missing".into();
    };
    let mut command = Command::new(systemctl);
    if user {
        command.arg("--user");
    }
    let Ok(output) = command.args(args).output() else {
        return "query failed".into();
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stdout.is_empty() {
        stdout
    } else if !stderr.is_empty() {
        stderr
    } else if output.status.success() {
        "yes".into()
    } else {
        format!("no (status {})", output.status.code().unwrap_or(-1))
    }
}

pub(crate) fn run_best_effort(program: &Path, args: &[&str], label: &str) {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => println!("{label}: OK"),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                println!(
                    "{label}: failed (status {})",
                    output.status.code().unwrap_or(-1)
                );
            } else {
                println!("{label}: failed: {stderr}");
            }
        }
        Err(error) => println!("{label}: failed: {error}"),
    }
}

pub(crate) fn sudo_noninteractive_available() -> bool {
    Command::new("sudo")
        .args(["-n", "true"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub(crate) fn os_pretty_name() -> Option<String> {
    let content = fs::read_to_string("/etc/os-release").ok()?;
    content.lines().find_map(|line| {
        line.strip_prefix("PRETTY_NAME=")
            .map(|value| value.trim_matches('"').to_string())
    })
}

/// Shortens paths under the home directory to `~/...` for display.
pub(crate) fn tilde(path: &Path) -> String {
    match home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// PIDs of running processes whose name (`comm`) or executable file name
/// equals `name`, ignoring case.
pub(crate) fn processes_named(name: &str) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let comm = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
            let exe = fs::read_link(entry.path().join("exe"))
                .ok()
                .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let matches = comm.trim().eq_ignore_ascii_case(name) || exe.eq_ignore_ascii_case(name);
            matches.then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}
