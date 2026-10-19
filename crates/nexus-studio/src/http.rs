//! Minimal HTTP/1.1 for a loopback-only control surface: one request per
//! connection (`Connection: close`) and Content-Length bodies only. Parsing
//! works on any `Read` and responses on any `Write`, so every handler is
//! testable without sockets.

use serde::Serialize;
use std::io::{self, Read, Write};
use std::time::Instant;

/// Largest accepted request body (pack imports, configs with images).
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Sent with every response. The UI only talks to its own origin, so
/// anything else (inline scripts, foreign hosts) is refused by the browser.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; img-src 'self' data: blob:; \
style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; font-src 'self'";

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    /// Path without the query string, still percent-encoded.
    pub path: String,
    pub query: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// First value of a query parameter, percent-decoded (`+` is a space).
    pub fn query_param(&self, key: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (k == key).then(|| String::from_utf8_lossy(&percent_decode(v, true)).into_owned())
        })
    }

    /// Methods that change state and therefore need the write token.
    pub fn is_mutating(&self) -> bool {
        matches!(self.method.as_str(), "POST" | "PUT" | "DELETE" | "PATCH")
    }
}

/// Why no request could be read.
#[derive(Debug)]
pub enum ReadError {
    /// The peer sent nothing usable or went away; there is nobody to answer.
    Gone,
    /// Malformed or oversized request that still deserves an answer.
    Reject { status: u16, message: String },
}

fn reject(status: u16, message: impl Into<String>) -> ReadError {
    ReadError::Reject {
        status,
        message: message.into(),
    }
}

/// Reads one request: the header block (at most 64 KiB) and a body of
/// exactly `Content-Length` bytes, at most `body_limit(head)` (the limit may
/// depend on the route). The whole request must arrive before `deadline`,
/// so a client that trickles bytes cannot hold the connection open.
pub fn read_request(
    reader: &mut impl Read,
    deadline: Instant,
    body_limit: impl Fn(&Request) -> usize,
) -> Result<Request, ReadError> {
    let (head, mut body) = read_head(reader, deadline)?;
    let mut request = parse_head(&head)?;
    if request.header("transfer-encoding").is_some() {
        return Err(reject(
            411,
            "chunked bodies are not supported; send Content-Length",
        ));
    }
    let length = match request.header("content-length") {
        None => 0,
        Some(value) => value
            .trim()
            .parse::<usize>()
            .map_err(|_| reject(400, "invalid Content-Length"))?,
    };
    let limit = body_limit(&request);
    if length > limit {
        return Err(reject(
            413,
            format!("request body is larger than {}", size_text(limit)),
        ));
    }
    let mut chunk = [0u8; 8192];
    while body.len() < length {
        check_deadline(deadline)?;
        let n = reader.read(&mut chunk).map_err(|_| ReadError::Gone)?;
        if n == 0 {
            return Err(reject(400, "request body is shorter than Content-Length"));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    request.body = body;
    Ok(request)
}

fn check_deadline(deadline: Instant) -> Result<(), ReadError> {
    if Instant::now() >= deadline {
        return Err(reject(408, "the request took too long to arrive"));
    }
    Ok(())
}

/// `16 MiB`, `64 KiB`, `100 bytes`.
fn size_text(bytes: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

/// Splits the stream at the blank line: returns the header text and any
/// body bytes that arrived in the same reads.
fn read_head(reader: &mut impl Read, deadline: Instant) -> Result<(Vec<u8>, Vec<u8>), ReadError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        check_deadline(deadline)?;
        let n = reader.read(&mut chunk).map_err(|_| ReadError::Gone)?;
        if n == 0 {
            return Err(ReadError::Gone);
        }
        // Only the new bytes plus three of the old can complete "\r\n\r\n".
        let search_from = buf.len().saturating_sub(3);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_blank_line(&buf[search_from..]) {
            let end = search_from + pos;
            let body = buf.split_off(end + 4);
            buf.truncate(end);
            return Ok((buf, body));
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(reject(431, "request headers are too large"));
        }
    }
}

fn find_blank_line(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_head(head: &[u8]) -> Result<Request, ReadError> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(reject(400, "malformed request line"));
    };
    let valid_method = !method.is_empty() && method.bytes().all(|b| b.is_ascii_uppercase());
    if !valid_method || !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(reject(400, "malformed request line"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| reject(400, "malformed header line"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(Request {
        method: method.to_string(),
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body: Vec::new(),
    })
}

/// DNS-rebinding guard: a page on `evil.example` that resolves to 127.0.0.1
/// still sends `Host: evil.example`, so only the loopback names for our own
/// port are accepted. Browsers omit `:80`, hence the bare form for port 80.
pub fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host.trim().to_ascii_lowercase();
    let (name, host_port) = match host.rsplit_once(':') {
        Some((name, p)) if !name.is_empty() && !p.contains(']') => (name, p.parse::<u16>().ok()),
        _ => (host.as_str(), None),
    };
    let port_ok = host_port.map_or(port == 80, |p| p == port);
    port_ok && matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

/// Write requests must carry `X-Nexus-Studio: 1` and the per-run token. A
/// cross-site page cannot add either header without a CORS preflight that
/// Studio never grants, and it cannot read the token from `/`.
pub fn token_valid(request: &Request, token: &str) -> bool {
    let marker = request.header("x-nexus-studio").map(str::trim) == Some("1");
    let sent = request.header("x-nexus-studio-token").unwrap_or_default();
    marker && !token.is_empty() && constant_time_eq(sent.trim().as_bytes(), token.as_bytes())
}

/// Compares without an early exit so response timing does not reveal how
/// many leading token characters were right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Decodes `%XX` escapes into raw bytes; malformed escapes stay literal.
pub fn percent_decode(input: &str, plus_as_space: bool) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| Some(hex_value(bytes[i + 1])? * 16 + hex_value(bytes[i + 2])?))
            .flatten();
        match (escaped, bytes[i]) {
            (Some(byte), _) => {
                out.push(byte);
                i += 3;
                continue;
            }
            (None, b'+') if plus_as_space => out.push(b' '),
            (None, byte) => out.push(byte),
        }
        i += 1;
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|v| v as u8)
}

/// Escapes everything but RFC 3986 unreserved characters, for building
/// query strings (application icon URLs).
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub cache_control: &'static str,
    pub extra_headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.to_string(),
            cache_control: "no-store",
            extra_headers: Vec::new(),
            body,
        }
    }

    pub fn json<T: Serialize + ?Sized>(status: u16, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self::bytes(status, "application/json; charset=utf-8", body),
            Err(error) => Self::error(500, format!("failed to encode response: {error}")),
        }
    }

    /// `{ok:false, message}` with the given status.
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        let body = serde_json::json!({ "ok": false, "message": message.into() });
        Self::bytes(
            status,
            "application/json; charset=utf-8",
            body.to_string().into_bytes(),
        )
    }

    pub fn not_found() -> Self {
        Self::error(404, "not found")
    }

    pub fn with_cache(mut self, cache_control: &'static str) -> Self {
        self.cache_control = cache_control;
        self
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.extra_headers.push((name, value.into()));
        self
    }

    /// Serializes status line, headers (including the security headers every
    /// response carries) and body.
    pub fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: {}\r\nX-Content-Type-Options: nosniff\r\n\
             Content-Security-Policy: {}\r\nConnection: close\r\n",
            self.status,
            reason_phrase(self.status),
            self.content_type,
            self.body.len(),
            self.cache_control,
            CONTENT_SECURITY_POLICY,
        );
        for (name, value) in &self.extra_headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.write_all(&self.body)?;
        out.flush()
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        428 => "Precondition Required",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8]) -> Result<Request, ReadError> {
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        read_request(&mut &raw[..], deadline, |_| MAX_BODY_BYTES)
    }

    fn rejected_status(raw: &[u8]) -> u16 {
        match parse(raw) {
            Err(ReadError::Reject { status, .. }) => status,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn parses_request_with_body_and_query() {
        let raw = b"POST /api/config?x=1 HTTP/1.1\r\nHost: localhost:1\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        let request = parse(raw).unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/config");
        assert_eq!(request.query, "x=1");
        assert_eq!(request.body, b"{\"ok\":true}");
        assert_eq!(request.header("HOST"), Some("localhost:1"));
    }

    #[test]
    fn body_split_across_reads_is_reassembled() {
        struct Trickle(Vec<u8>);
        impl Read for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0.is_empty() || buf.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.0.remove(0);
                Ok(1)
            }
        }
        let raw = b"POST /x HTTP/1.1\r\ncontent-length: 5\r\n\r\nhello".to_vec();
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let request = read_request(&mut Trickle(raw), deadline, |_| 5).unwrap();
        assert_eq!(request.body, b"hello");
    }

    #[test]
    fn slow_requests_hit_the_deadline() {
        struct Slow(Vec<u8>);
        impl Read for Slow {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                std::thread::sleep(std::time::Duration::from_millis(5));
                if self.0.is_empty() || buf.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.0.remove(0);
                Ok(1)
            }
        }
        let raw = format!(
            "POST /x HTTP/1.1\r\ncontent-length: 100\r\n\r\n{}",
            "a".repeat(100)
        );
        let deadline = Instant::now() + std::time::Duration::from_millis(60);
        match read_request(&mut Slow(raw.into_bytes()), deadline, |_| MAX_BODY_BYTES) {
            Err(ReadError::Reject { status, .. }) => assert_eq!(status, 408),
            other => panic!("expected 408, got {other:?}"),
        }
    }

    #[test]
    fn body_limit_can_depend_on_the_route() {
        let small = |request: &Request| if request.path == "/small" { 4 } else { 64 };
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let raw = b"POST /small HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        match read_request(&mut &raw[..], deadline, small) {
            Err(ReadError::Reject { status, message }) => {
                assert_eq!(
                    (status, message.as_str()),
                    (413, "request body is larger than 4 bytes")
                );
            }
            other => panic!("expected 413, got {other:?}"),
        }
        let raw = b"POST /big HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        assert!(read_request(&mut &raw[..], deadline, small).is_ok());
        assert_eq!(size_text(MAX_BODY_BYTES), "16 MiB");
        assert_eq!(size_text(64 * 1024), "64 KiB");
    }

    #[test]
    fn rejects_malformed_and_oversized_requests() {
        let too_big = format!(
            "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert_eq!(rejected_status(too_big.as_bytes()), 413);
        assert_eq!(
            rejected_status(b"POST /x HTTP/1.1\r\nContent-Length: abc\r\n\r\n"),
            400
        );
        assert_eq!(
            rejected_status(b"POST /x HTTP/1.1\r\nContent-Length: 9\r\n\r\nshort"),
            400
        );
        assert_eq!(rejected_status(b"GARBAGE\r\n\r\n"), 400);
        assert_eq!(rejected_status(b"GET http://x/ HTTP/1.1\r\n\r\n"), 400);
        assert_eq!(
            rejected_status(b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            411
        );
        let huge_header = format!("GET / HTTP/1.1\r\nX: {}\r\n", "a".repeat(70_000));
        assert_eq!(rejected_status(huge_header.as_bytes()), 431);
        assert!(matches!(parse(b""), Err(ReadError::Gone)));
    }

    #[test]
    fn decodes_query_parameters() {
        let raw = b"GET /api/geocode?q=Bad%20T%C3%B6lz&lang=de&plus=a+b&bad=%zz%e HTTP/1.1\r\n\r\n";
        let request = parse(raw).unwrap();
        assert_eq!(request.query_param("q").as_deref(), Some("Bad Tölz"));
        assert_eq!(request.query_param("lang").as_deref(), Some("de"));
        assert_eq!(request.query_param("plus").as_deref(), Some("a b"));
        assert_eq!(request.query_param("bad").as_deref(), Some("%zz%e"));
        assert_eq!(request.query_param("missing"), None);
    }

    #[test]
    fn host_must_name_loopback_and_our_port() {
        for host in [
            "127.0.0.1:19777",
            "localhost:19777",
            "LOCALHOST:19777",
            "[::1]:19777",
        ] {
            assert!(host_allowed(Some(host), 19777), "{host}");
        }
        for host in [
            "evil.example:19777",
            "127.0.0.1:80",
            "localhost",
            "127.0.0.1.evil.example:19777",
            "[::1]",
            "",
        ] {
            assert!(!host_allowed(Some(host), 19777), "{host}");
        }
        assert!(!host_allowed(None, 19777));
        assert!(host_allowed(Some("localhost"), 80));
    }

    #[test]
    fn token_needs_marker_header_and_exact_token() {
        let mut request = parse(b"POST /api/config HTTP/1.1\r\n\r\n").unwrap();
        assert!(!token_valid(&request, "secret"));
        request
            .headers
            .push(("x-nexus-studio-token".into(), "secret".into()));
        assert!(!token_valid(&request, "secret"), "marker header missing");
        request.headers.push(("x-nexus-studio".into(), "1".into()));
        assert!(token_valid(&request, "secret"));
        assert!(!token_valid(&request, "secreT"));
        assert!(!token_valid(&request, "secret-longer"));
        assert!(!token_valid(&request, ""));
    }

    #[test]
    fn response_carries_length_and_security_headers() {
        let mut out = Vec::new();
        Response::error(404, "nope").write_to(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let head = format!("{head}\r\n");
        assert!(head.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(head.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(head.contains("Cache-Control: no-store\r\n"));
        assert!(head.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(head.contains(&format!(
            "Content-Security-Policy: {CONTENT_SECURITY_POLICY}\r\n"
        )));
        assert!(head.contains("Connection: close\r\n"));
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json, serde_json::json!({ "ok": false, "message": "nope" }));
    }

    #[test]
    fn percent_encoding_round_trips() {
        let id = "org.kde.dolphin desktop/ü";
        let encoded = percent_encode(id);
        assert!(encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b)));
        assert_eq!(percent_decode(&encoded, false), id.as_bytes());
    }
}
