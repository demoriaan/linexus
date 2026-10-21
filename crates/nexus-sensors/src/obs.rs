//! OBS Studio remote control through obs-websocket 5, which OBS includes
//! since version 28 (Tools → WebSocket Server Settings).
//!
//! [`ObsConnection`] is a blocking client: connect and identify, send a
//! request and wait for its answer, or wait a little for the next event.
//! Events that arrive while a request waits are kept for later.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use tungstenite::handshake::HandshakeError;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

pub const DEFAULT_PORT: u16 = 4455;
/// Inputs asked whether they carry audio; more are not listed.
pub const MAX_INPUTS: usize = 200;

/// obs-websocket's close code for a wrong or missing password.
const CLOSE_AUTH_FAILED: u16 = 4009;
const RPC_VERSION: u64 = 1;
/// General (exit), Config (scene collections), Scenes, Inputs (mute) and
/// Outputs (stream, record, replay buffer, virtual camera).
const EVENT_SUBSCRIPTIONS: u64 = 1 | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 6);
/// The events kept for [`ObsConnection::next_event`]; the subscriptions
/// bring others too, which are dropped as they arrive.
pub const FOLLOWED_EVENTS: [&str; 12] = [
    "CurrentProgramSceneChanged",
    "SceneNameChanged",
    "CurrentSceneCollectionChanged",
    "InputCreated",
    "InputRemoved",
    "InputNameChanged",
    "InputMuteStateChanged",
    "StreamStateChanged",
    "RecordStateChanged",
    "ReplayBufferStateChanged",
    "VirtualcamStateChanged",
    "ExitStarted",
];
/// Nothing this client asks for comes near this; a scene list of a few
/// hundred scenes is tens of KiB.
const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// Events waiting while a request runs, counted in message bytes.
const MAX_QUEUED_BYTES: usize = 1 << 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Where OBS listens. Stored in sensors.json next to the Home Assistant
/// server, since the password is a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObsServer {
    pub host: String,
    pub port: u16,
    /// Empty when the server has authentication turned off.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObsError {
    /// Nothing listens there (OBS not running, server turned off).
    Unreachable(String),
    /// Wrong or missing password.
    Auth,
    /// OBS refused a request, e.g. an unknown scene.
    Request { code: u64, comment: String },
    /// The connection dropped or OBS sent something unexpected.
    Connection(String),
}

impl fmt::Display for ObsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(_) => {
                f.write_str("OBS is not running, or its WebSocket server is off")
            }
            Self::Auth => f.write_str("OBS refused the password"),
            Self::Request { code, comment } if comment.is_empty() => {
                write!(f, "OBS refused the request (code {code})")
            }
            Self::Request { comment, .. } => write!(f, "OBS: {comment}"),
            Self::Connection(detail) => write!(f, "connection to OBS lost ({detail})"),
        }
    }
}

impl std::error::Error for ObsError {}

#[derive(Debug, Clone, PartialEq)]
pub struct ObsEvent {
    pub kind: String,
    pub data: Value,
}

pub struct ObsConnection {
    socket: WebSocket<Timed>,
    next_id: u64,
    /// Followed events with the size of the message each came in.
    events: VecDeque<(ObsEvent, usize)>,
    queued_bytes: usize,
    /// OBS version from the handshake, e.g. `32.2.2` (empty before 5.x).
    pub obs_version: String,
}

impl ObsConnection {
    /// Connects, authenticates and subscribes to the events the strip
    /// shows. `timeout` bounds the TCP connect, and then the WebSocket
    /// handshake and the identification together.
    pub fn connect(server: &ObsServer, timeout: Duration) -> Result<Self, ObsError> {
        let addresses = (server.host.as_str(), server.port)
            .to_socket_addrs()
            .map_err(|error| ObsError::Unreachable(error.to_string()))?;
        let mut last = None;
        let mut stream = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        let stream = stream.ok_or_else(|| {
            ObsError::Unreachable(last.map_or("no address".into(), |error| error.to_string()))
        })?;
        let deadline = Instant::now() + timeout;
        stream
            .set_write_timeout(Some(timeout))
            .and_then(|()| stream.set_nodelay(true))
            .map_err(|error| ObsError::Connection(error.to_string()))?;
        let stream = Timed {
            stream,
            deadline: Some(deadline),
        };
        let host = if server.host.contains(':') {
            format!("[{}]", server.host)
        } else {
            server.host.clone()
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        // Reads stop at the deadline however slowly a server trickles its
        // answer; tungstenite reports that as an interrupted handshake.
        let mut attempt = tungstenite::client::client_with_config(
            format!("ws://{host}:{}", server.port),
            stream,
            Some(config),
        );
        let socket = loop {
            match attempt {
                Ok((socket, _)) => break socket,
                Err(HandshakeError::Interrupted(_)) if Instant::now() >= deadline => {
                    return Err(late());
                }
                Err(HandshakeError::Interrupted(handshake)) => attempt = handshake.handshake(),
                Err(HandshakeError::Failure(error)) => {
                    return Err(ObsError::Connection(error.to_string()));
                }
            }
        };
        let mut connection = Self {
            socket,
            next_id: 0,
            events: VecDeque::new(),
            queued_bytes: 0,
            obs_version: String::new(),
        };
        connection.identify(&server.password, deadline)?;
        Ok(connection)
    }

    fn identify(&mut self, password: &str, deadline: Instant) -> Result<(), ObsError> {
        let hello = loop {
            let (message, _) = self.read_message(deadline)?.ok_or_else(late)?;
            if message["op"] == 0 {
                break message;
            }
        };
        self.obs_version = hello["d"]["obsStudioVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut identify = json!({
            "rpcVersion": RPC_VERSION,
            "eventSubscriptions": EVENT_SUBSCRIPTIONS,
        });
        if let Some(auth) = hello["d"].get("authentication") {
            if password.is_empty() {
                return Err(ObsError::Auth);
            }
            let (Some(challenge), Some(salt)) = (auth["challenge"].as_str(), auth["salt"].as_str())
            else {
                return Err(ObsError::Connection("malformed hello".into()));
            };
            identify["authentication"] = auth_string(password, salt, challenge).into();
        }
        self.write(json!({ "op": 1, "d": identify }))?;
        loop {
            let (message, _) = self.read_message(deadline)?.ok_or_else(late)?;
            if message["op"] == 2 {
                return Ok(());
            }
        }
    }

    /// Sends a request and waits for its answer; returns its response data
    /// (`null` when there is none).
    pub fn request(&mut self, kind: &str, data: Option<Value>) -> Result<Value, ObsError> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let mut request = json!({ "requestType": kind, "requestId": id });
        if let Some(data) = data {
            request["requestData"] = data;
        }
        self.write(json!({ "op": 6, "d": request }))?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let (message, size) = self.read_message(deadline)?.ok_or_else(late)?;
            match message["op"].as_u64() {
                Some(5) => self.queue_event(&message["d"], size),
                Some(7) if message["d"]["requestId"] == id.as_str() => {
                    let status = &message["d"]["requestStatus"];
                    if status["result"] != true {
                        return Err(ObsError::Request {
                            code: status["code"].as_u64().unwrap_or(0),
                            comment: status["comment"].as_str().unwrap_or_default().to_string(),
                        });
                    }
                    return Ok(message["d"]["responseData"].clone());
                }
                _ => {}
            }
        }
    }

    /// The next event, waiting at most `wait`; None when none came.
    pub fn next_event(&mut self, wait: Duration) -> Result<Option<ObsEvent>, ObsError> {
        let deadline = Instant::now() + wait;
        while self.events.is_empty() {
            let Some((message, size)) = self.read_message(deadline)? else {
                return Ok(None);
            };
            if message["op"] == 5 {
                self.queue_event(&message["d"], size);
            }
        }
        Ok(self.events.pop_front().map(|(event, size)| {
            self.queued_bytes -= size;
            event
        }))
    }

    /// Says goodbye and waits briefly for OBS's answer, so OBS sees a
    /// regular close instead of a dropped connection.
    pub fn close(mut self) {
        if self.socket.close(None).is_err() {
            return;
        }
        // Until OBS answers the close, the connection drops, or 300 ms pass.
        self.socket.get_mut().deadline = Some(Instant::now() + Duration::from_millis(300));
        while self.socket.read().is_ok() {}
    }

    /// Keeps a followed event for later; the oldest go when the queue
    /// would hold more than [`MAX_QUEUED_BYTES`].
    fn queue_event(&mut self, data: &Value, size: usize) {
        let Some(kind) = data["eventType"]
            .as_str()
            .filter(|kind| FOLLOWED_EVENTS.contains(kind))
        else {
            return;
        };
        while self.queued_bytes + size > MAX_QUEUED_BYTES {
            let Some((_, dropped)) = self.events.pop_front() else {
                return;
            };
            self.queued_bytes -= dropped;
        }
        // New inputs bring their settings along; only the name is used.
        let data = if kind == "InputCreated" {
            json!({ "inputName": data["eventData"]["inputName"] })
        } else {
            data["eventData"].clone()
        };
        self.queued_bytes += size;
        self.events.push_back((
            ObsEvent {
                kind: kind.to_string(),
                data,
            },
            size,
        ));
    }

    fn write(&mut self, message: Value) -> Result<(), ObsError> {
        self.socket
            .send(Message::Text(message.to_string().into()))
            .map_err(|error| ObsError::Connection(error.to_string()))
    }

    /// The next JSON message and its size, or None when `deadline` passed
    /// first. Pings are answered by tungstenite on the next read or write.
    fn read_message(&mut self, deadline: Instant) -> Result<Option<(Value, usize)>, ObsError> {
        loop {
            if Instant::now() >= deadline {
                return Ok(None);
            }
            self.socket.get_mut().deadline = Some(deadline);
            let message = match self.socket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(ObsError::Connection(error.to_string())),
            };
            match message {
                Message::Text(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(text.as_str()) {
                        return Ok(Some((value, text.len())));
                    }
                }
                Message::Close(frame) => {
                    let code = frame.as_ref().map(|frame| u16::from(frame.code));
                    if code == Some(CLOSE_AUTH_FAILED) {
                        return Err(ObsError::Auth);
                    }
                    let reason = frame
                        .map(|frame| frame.reason.to_string())
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| "closed".into());
                    return Err(ObsError::Connection(reason));
                }
                _ => {}
            }
        }
    }
}

/// A TCP stream whose reads end at a deadline: each read waits at most
/// until then, and one after it reports `WouldBlock` at once. A plain read
/// timeout would restart with every byte a slow server sends.
struct Timed {
    stream: TcpStream,
    deadline: Option<Instant>,
}

impl io::Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(deadline) = self.deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

impl io::Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn late() -> ObsError {
    ObsError::Connection("OBS did not answer in time".into())
}

/// obs-websocket's authentication string:
/// base64(sha256(base64(sha256(password + salt)) + challenge)).
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let sha = |text: &str| ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    let secret = b64.encode(sha(&format!("{password}{salt}")));
    b64.encode(sha(&format!("{secret}{challenge}")))
}

/// What Studio offers in its pickers: scenes in the order OBS lists them,
/// and the inputs that carry audio (those that can be muted).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ObsOverview {
    pub version: String,
    pub current_scene: String,
    pub scenes: Vec<String>,
    pub audio_inputs: Vec<String>,
}

pub fn obs_overview(server: &ObsServer) -> Result<ObsOverview, ObsError> {
    let mut obs = ObsConnection::connect(server, Duration::from_secs(3))?;
    let list = obs.request("GetSceneList", None)?;
    let inputs = input_names(&obs.request("GetInputList", None)?);
    let mut audio_inputs = Vec::new();
    for input in inputs.into_iter().take(MAX_INPUTS) {
        match obs.request("GetInputMute", Some(json!({ "inputName": input }))) {
            Ok(_) => audio_inputs.push(input),
            // No audio: not a source to mute.
            Err(ObsError::Request { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let overview = ObsOverview {
        version: obs.obs_version.clone(),
        current_scene: list["currentProgramSceneName"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        scenes: scene_names(&list),
        audio_inputs,
    };
    obs.close();
    Ok(overview)
}

/// Scene names from `GetSceneList`, top to bottom as OBS shows them (the
/// list counts its index from the bottom).
pub fn scene_names(list: &Value) -> Vec<String> {
    let mut scenes: Vec<(i64, String)> = list["scenes"]
        .as_array()
        .map(|scenes| {
            scenes
                .iter()
                .filter_map(|scene| {
                    Some((
                        scene["sceneIndex"].as_i64().unwrap_or(0),
                        scene["sceneName"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    scenes.sort_by_key(|scene| std::cmp::Reverse(scene.0));
    scenes.into_iter().map(|(_, name)| name).collect()
}

pub fn input_names(list: &Value) -> Vec<String> {
    list["inputs"]
        .as_array()
        .map(|inputs| {
            inputs
                .iter()
                .filter_map(|input| Some(input["inputName"].as_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn close_with(code: u16) -> Message {
        Message::Close(Some(tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::from(code),
            reason: "".into(),
        }))
    }

    #[test]
    fn computes_the_documented_authentication() {
        assert_eq!(
            auth_string(
                "supersecretpassword",
                "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="
            ),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }

    #[test]
    fn lists_scenes_top_to_bottom() {
        let list = json!({"scenes": [
            {"sceneIndex": 0, "sceneName": "Bottom"},
            {"sceneIndex": 2, "sceneName": "Top"},
            {"sceneIndex": 1, "sceneName": "Middle"},
            {"sceneIndex": 3}
        ]});
        assert_eq!(scene_names(&list), ["Top", "Middle", "Bottom"]);
        assert_eq!(
            input_names(&json!({"inputs": [{"inputName": "Mic/Aux"}, {}]})),
            ["Mic/Aux"]
        );
    }

    /// Plays OBS for one connection: hello (with a password), identify,
    /// then answers requests from `answer` and sends `events` first.
    fn fake_obs(
        password: &'static str,
        events: Vec<Value>,
        answer: fn(&str, &Value) -> Value,
    ) -> (ObsServer, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let salt = "c2FsdA==";
            let challenge = "Y2hhbGxlbmdl";
            let hello = json!({"op": 0, "d": {
                "obsWebSocketVersion": "5.6.3", "rpcVersion": 1, "obsStudioVersion": "32.2.2",
                "authentication": {"challenge": challenge, "salt": salt}}});
            ws.send(Message::Text(hello.to_string().into())).unwrap();
            let identify: Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(identify["op"], 1);
            assert_eq!(identify["d"]["eventSubscriptions"], EVENT_SUBSCRIPTIONS);
            if identify["d"]["authentication"] != auth_string(password, salt, challenge).as_str() {
                ws.send(close_with(CLOSE_AUTH_FAILED)).unwrap();
                let _ = ws.flush();
                return Vec::new();
            }
            ws.send(Message::Text(
                json!({"op": 2, "d": {"negotiatedRpcVersion": 1}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
            let mut seen = Vec::new();
            loop {
                let Ok(message) = ws.read() else { break };
                let Ok(text) = message.to_text() else { break };
                let Ok(request) = serde_json::from_str::<Value>(text) else {
                    break;
                };
                let kind = request["d"]["requestType"].as_str().unwrap().to_string();
                for event in &events {
                    ws.send(Message::Text(
                        json!({"op": 5, "d": event}).to_string().into(),
                    ))
                    .unwrap();
                }
                let response = answer(&kind, &request["d"]["requestData"]);
                let mut reply = json!({"op": 7, "d": {
                    "requestType": kind, "requestId": request["d"]["requestId"],
                    "requestStatus": {"result": true, "code": 100}}});
                if response.get("code").is_some() {
                    reply["d"]["requestStatus"] = json!({"result": false, "code": response["code"], "comment": response["comment"]});
                } else {
                    reply["d"]["responseData"] = response;
                }
                ws.send(Message::Text(reply.to_string().into())).unwrap();
                seen.push(kind);
            }
            seen
        });
        let server = ObsServer {
            host: "127.0.0.1".into(),
            port,
            password: "right".into(),
        };
        (server, handle)
    }

    #[test]
    fn requests_and_keeps_events_that_arrive_meanwhile() {
        let events = vec![json!({"eventType": "CurrentProgramSceneChanged",
            "eventIntent": 4, "eventData": {"sceneName": "Gameplay"}})];
        let (server, handle) = fake_obs("right", events, |kind, data| match kind {
            "SetCurrentProgramScene" if data["sceneName"] == "Missing" => {
                json!({"code": 600, "comment": "No source was found by the name of `Missing`."})
            }
            "GetRecordStatus" => json!({"outputActive": true, "outputDuration": 61000}),
            _ => Value::Null,
        });
        let mut obs = ObsConnection::connect(&server, Duration::from_secs(2)).unwrap();
        assert_eq!(obs.obs_version, "32.2.2");
        let status = obs.request("GetRecordStatus", None).unwrap();
        assert_eq!(status["outputDuration"], 61000);
        let error = obs
            .request(
                "SetCurrentProgramScene",
                Some(json!({"sceneName": "Missing"})),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "OBS: No source was found by the name of `Missing`."
        );
        let event = obs.next_event(Duration::from_millis(50)).unwrap().unwrap();
        assert_eq!(event.kind, "CurrentProgramSceneChanged");
        assert_eq!(event.data["sceneName"], "Gameplay");
        assert!(obs.next_event(Duration::from_millis(50)).unwrap().is_some());
        assert_eq!(obs.next_event(Duration::from_millis(50)).unwrap(), None);
        obs.close();
        assert_eq!(
            handle.join().unwrap(),
            ["GetRecordStatus", "SetCurrentProgramScene"]
        );
    }

    #[test]
    fn keeps_only_followed_events_within_a_byte_budget() {
        let pad = "x".repeat(600 << 10);
        let scene = |name: &str, pad: &str| {
            json!({"eventType": "CurrentProgramSceneChanged", "eventIntent": 4,
                   "eventData": {"sceneName": name, "pad": pad}})
        };
        let events = vec![
            scene("First", &pad),
            scene("Second", &pad),
            json!({"eventType": "SceneItemCreated", "eventIntent": 128, "eventData": {}}),
            scene("Third", ""),
        ];
        let (server, handle) = fake_obs("right", events, |_, _| Value::Null);
        let mut obs = ObsConnection::connect(&server, Duration::from_secs(2)).unwrap();
        obs.request("GetVersion", None).unwrap();
        let wait = Duration::from_millis(50);
        let names: Vec<Value> = std::iter::from_fn(|| obs.next_event(wait).unwrap())
            .map(|event| event.data["sceneName"].clone())
            .collect();
        // The first did not fit next to the second; the item event is not followed.
        assert_eq!(names, [json!("Second"), json!("Third")]);
        assert_eq!(obs.queued_bytes, 0);
        obs.close();
        handle.join().unwrap();
    }

    #[test]
    fn a_trickling_handshake_ends_at_the_deadline() {
        use std::io::Write;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for byte in b"HTTP/1.1 101 Switching Protocols\r\n" {
                if stream.write_all(&[*byte]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(150));
            }
        });
        let started = Instant::now();
        let result = ObsConnection::connect(
            &ObsServer {
                host: "127.0.0.1".into(),
                port,
                password: String::new(),
            },
            Duration::from_secs(1),
        );
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_millis(1800),
            "{:?}",
            started.elapsed()
        );
        drop(result);
        server.join().unwrap();
    }

    #[test]
    fn a_wrong_password_is_reported_as_such() {
        let (mut server, handle) = fake_obs("right", Vec::new(), |_, _| Value::Null);
        server.password = "wrong".into();
        let error = ObsConnection::connect(&server, Duration::from_secs(2))
            .err()
            .unwrap();
        assert_eq!(error, ObsError::Auth);
        handle.join().unwrap();
    }
}
