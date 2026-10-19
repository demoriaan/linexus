//! Linexus Studio: the local web UI for the Nexus strip. A small HTTP server
//! on a loopback address serves the embedded frontend and a JSON API that
//! edits the config, renders previews and talks to the daemon.

mod api;
mod assets;
mod http;
mod mime;
mod providers;
mod state;

use anyhow::{bail, Context, Result};
use clap::Parser;
use http::{ReadError, Response};
use state::AppState;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Generous for a 16 MiB upload over loopback, short enough that a stalled
/// client does not pin a thread for long.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// Time a client has to send its whole request, however it spreads the
/// bytes over reads.
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
/// Connections served at once. One tab needs a handful (status polls, the
/// live mirror, previews); more get 503 instead of another thread.
const MAX_CONNECTIONS: usize = 32;

#[derive(Debug, Parser)]
#[command(
    name = "linexus-studio",
    version,
    about = "Local Linexus Studio web UI"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 19777)]
    port: u16,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    socket_path: Option<PathBuf>,
    #[arg(long)]
    no_open: bool,
    /// Section to open in the browser, e.g. `news`.
    #[arg(long)]
    section: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !is_loopback_host(&args.host) {
        bail!("Linexus Studio is a local control surface and only binds to loopback hosts; use 127.0.0.1, ::1 or localhost");
    }
    let state = Arc::new(AppState::new(
        args.config
            .unwrap_or_else(nexus_config::default_config_path),
        args.socket_path
            .unwrap_or_else(nexus_ipc::default_socket_path),
        args.port,
        assets::Source::from_env(),
    ));
    let url = browser_url(&args.host, args.port, args.section.as_deref());
    let listener = match TcpListener::bind((args.host.as_str(), args.port)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == ErrorKind::AddrInUse => {
            println!("Linexus Studio already appears to be running: {url}");
            open_browser(&url, args.no_open);
            return Ok(());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to bind {}:{}", args.host, args.port))
        }
    };
    println!("Linexus Studio: {url}");
    println!("Config: {}", state.config_path.display());
    if let assets::Source::Disk(root) = &state.assets {
        println!("Serving the frontend from {}", root.display());
    }
    open_browser(&url, args.no_open);
    serve(listener, state);
    Ok(())
}

/// The URL we print and open. The Host check accepts `localhost`,
/// `127.0.0.1` and `[::1]`, so IPv6 literals get their brackets here.
fn browser_url(host: &str, port: u16, section: Option<&str>) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let url = format!("http://{host}:{port}/");
    // Sections are plain anchors; a safe charset keeps the launcher
    // argument from injecting anything into the URL.
    match section {
        Some(section)
            if !section.is_empty()
                && section
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-') =>
        {
            format!("{url}#{section}")
        }
        _ => url,
    }
}

fn open_browser(url: &str, disabled: bool) {
    if !disabled {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| addr.is_loopback())
}

/// One thread per connection, at most [`MAX_CONNECTIONS`] at a time:
/// requests are few (one browser tab) and some handlers block on systemctl
/// or the network.
fn serve(listener: TcpListener, state: Arc<AppState>) {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let Some(slot) = Slot::take(&active) else {
                    // The answer fits the socket buffer, so this write
                    // does not block the accept loop.
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let busy = Response::error(503, "Studio is busy; try again in a moment");
                    let _ = busy.write_to(&mut stream);
                    continue;
                };
                let state = state.clone();
                std::thread::spawn(move || {
                    let _slot = slot;
                    handle_connection(stream, &state);
                });
            }
            Err(error) => eprintln!("studio accept failed: {error}"),
        }
    }
}

/// One of the [`MAX_CONNECTIONS`] connection slots, given back on drop
/// (also when the connection thread panics).
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(active: &Arc<AtomicUsize>) -> Option<Self> {
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(active.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle_connection(mut stream: TcpStream, state: &AppState) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let response = match http::read_request(&mut stream, deadline, api::body_limit) {
        Ok(request) => respond(&request, state),
        Err(ReadError::Reject { status, message }) => Response::error(status, message),
        Err(ReadError::Gone) => return,
    };
    if let Err(error) = response.write_to(&mut stream) {
        eprintln!("studio response failed: {error}");
    }
}

/// Runs the handler and turns a panic (a renderer bug on odd input, say)
/// into a 500 instead of a dropped connection. Shared caches recover from
/// poisoning, so the server keeps working afterwards.
fn respond(request: &http::Request, state: &AppState) -> Response {
    catch_unwind(AssertUnwindSafe(|| api::handle(request, state)))
        .unwrap_or_else(|_| Response::error(500, "internal error while handling the request"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn binds_only_to_loopback() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.20"));
    }

    #[test]
    fn browser_url_brackets_ipv6_and_filters_sections() {
        assert_eq!(browser_url("::1", 5, None), "http://[::1]:5/");
        assert_eq!(
            browser_url("127.0.0.1", 5, Some("news")),
            "http://127.0.0.1:5/#news"
        );
        assert_eq!(
            browser_url("localhost", 5, Some("x\"><script>")),
            "http://localhost:5/"
        );
    }

    #[test]
    fn serves_real_connections_end_to_end() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut state = state::testing::state();
        state.port = port;
        let state = Arc::new(state);
        std::thread::spawn(move || serve(listener, state));

        let exchange = |raw: String| {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(raw.as_bytes()).unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).unwrap();
            reply
        };
        let ok = exchange(format!(
            "GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"
        ));
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        let big = exchange(format!(
            "POST /api/config HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: 99999999\r\n\r\n"
        ));
        assert!(big.starts_with("HTTP/1.1 413 "), "{big}");
        let hook = exchange(format!(
            "POST /api/sensors/webhook/door HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: 70000\r\n\r\n"
        ));
        assert!(hook.starts_with("HTTP/1.1 413 "), "{hook}");
        assert!(hook.contains("larger than 64 KiB"), "{hook}");
    }

    #[test]
    fn connections_beyond_the_cap_get_503() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut state = state::testing::state();
        state.port = port;
        let state = Arc::new(state);
        std::thread::spawn(move || serve(listener, state));

        // Idle connections hold their slots while their threads wait for a
        // request.
        let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        let mut extra = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut reply = String::new();
        extra.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 503 "), "{reply}");

        // Closing them frees the slots again.
        drop(idle);
        let request = format!("GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n");
        let started = Instant::now();
        loop {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut reply = String::new();
            let _ = stream.read_to_string(&mut reply);
            if reply.starts_with("HTTP/1.1 200 ") {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "{reply}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
