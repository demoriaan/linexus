//! Small HTTP client on top of the `curl` binary, like the feed fetcher in
//! lib.rs, but reporting the status code and keeping headers out of argv.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends one request. Headers travel to curl as a config file on stdin
/// (`--config -`) instead of `--header` arguments: argv is world-readable
/// through `ps` and /proc, and headers carry tokens such as Home Assistant's
/// long-lived access token. The body is passed in argv, so it must not hold
/// secrets. Only http(s) is allowed, also after redirects.
pub(crate) fn http_request(
    method: &str,
    url: &str,
    headers: &[String],
    body: Option<&str>,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("{url} is not an http(s) address"));
    }
    let config = curl_header_config(headers)?;
    let max_time = timeout.as_secs().max(1).to_string();
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
        "--max-time",
        &max_time,
        "--user-agent",
        concat!("linexus/", env!("CARGO_PKG_VERSION")),
        "--request",
        method,
        // The status code follows the body on its own line.
        "--write-out",
        "\n%{http_code}",
        "--config",
        "-",
    ]);
    if let Some(body) = body {
        // curl reads `@name` as "send this file".
        if body.starts_with('@') {
            return Err("request bodies must not start with @".into());
        }
        command.args(["--data-binary", body]);
    }
    command
        .arg("--")
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start curl: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A write error means curl already exited; its status says why.
        let _ = stdin.write_all(config.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("curl failed: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("could not reach {url}")
        } else {
            message
        });
    }
    split_status(output.stdout).ok_or_else(|| format!("no HTTP answer from {url}"))
}

/// Header lines in curl's config-file syntax. Line breaks would let a value
/// inject further options or headers, so they are refused.
fn curl_header_config(headers: &[String]) -> Result<String, String> {
    let mut config = String::new();
    for header in headers {
        if header.contains(['\n', '\r', '\0']) {
            return Err("header values must not contain line breaks".into());
        }
        let escaped = header.replace('\\', "\\\\").replace('"', "\\\"");
        config.push_str(&format!("header = \"{escaped}\"\n"));
    }
    Ok(config)
}

fn split_status(mut stdout: Vec<u8>) -> Option<HttpResponse> {
    let newline = stdout.iter().rposition(|byte| *byte == b'\n')?;
    let status = std::str::from_utf8(&stdout[newline + 1..])
        .ok()?
        .trim()
        .parse::<u16>()
        .ok()?;
    stdout.truncate(newline);
    // curl reports 000 when no HTTP exchange happened.
    (status != 0).then_some(HttpResponse {
        status,
        body: stdout,
    })
}

/// Percent-encodes a query parameter value (RFC 3986 unreserved characters
/// stay as they are).
pub(crate) fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Decodes `%XX` escapes into raw bytes (file URLs may name non-UTF-8 paths).
pub(crate) fn percent_decode(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            let hex = bytes.get(idx + 1..idx + 3).and_then(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            });
            if let Some(byte) = hex {
                out.push(byte);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_body_and_status_code() {
        let response = split_status(b"{\"ok\":true}\n200".to_vec()).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"ok\":true}");
        assert!(response.is_success());
        let empty = split_status(b"\n401".to_vec()).unwrap();
        assert_eq!(empty.status, 401);
        assert!(empty.body.is_empty());
        assert!(split_status(b"\n000".to_vec()).is_none());
        assert!(split_status(b"garbage".to_vec()).is_none());
    }

    #[test]
    fn header_config_escapes_and_refuses_line_breaks() {
        let config = curl_header_config(&["Authorization: Bearer a\"b\\c".into()]).unwrap();
        assert_eq!(config, "header = \"Authorization: Bearer a\\\"b\\\\c\"\n");
        assert!(curl_header_config(&["X: a\nurl = file:///etc/passwd".into()]).is_err());
    }

    #[test]
    fn refuses_non_http_urls_without_running_curl() {
        assert!(http_request(
            "GET",
            "file:///etc/passwd",
            &[],
            None,
            Duration::from_secs(1)
        )
        .is_err());
    }

    #[test]
    fn percent_codecs_round_trip() {
        assert_eq!(
            encode_query_value("Frankfurt am Main"),
            "Frankfurt%20am%20Main"
        );
        assert_eq!(encode_query_value("Köln&x=1"), "K%C3%B6ln%26x%3D1");
        assert_eq!(percent_decode("K%C3%B6ln%20x"), "Köln x".as_bytes());
        assert_eq!(percent_decode("100%"), b"100%");
        assert_eq!(percent_decode("%zz%4"), b"%zz%4");
    }
}
