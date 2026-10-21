use super::*;
use nexus_config::{template_page, PageTemplate, TextProps, Widget};
use std::net::TcpListener;
use tungstenite::Message;

/// What the fake OBS does with a request: its answer (or `Err(code)`) and
/// the events it sends afterwards.
type Answer = fn(&str, &Value) -> (Result<Value, u64>, Vec<Value>);

/// Plays OBS without a password for any number of connections; returns
/// the server and the request types it saw.
fn fake_obs(answer: Answer) -> (ObsServer, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let log = log.clone();
            thread::spawn(move || serve(stream, answer, &log));
        }
    });
    let server = ObsServer {
        host: "127.0.0.1".into(),
        port,
        password: String::new(),
    };
    (server, seen)
}

fn serve(stream: std::net::TcpStream, answer: Answer, seen: &Mutex<Vec<String>>) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    let send = |ws: &mut tungstenite::WebSocket<_>, value: Value| {
        ws.send(Message::Text(value.to_string().into()))
    };
    let hello = json!({"op": 0, "d": {"rpcVersion": 1, "obsStudioVersion": "32.2.2"}});
    if send(&mut ws, hello).is_err() {
        return;
    }
    let _identify = ws.read();
    let _ = send(&mut ws, json!({"op": 2, "d": {"negotiatedRpcVersion": 1}}));
    while let Ok(message) = ws.read() {
        let Ok(request) = serde_json::from_str::<Value>(message.to_text().unwrap_or_default())
        else {
            return;
        };
        let kind = request["d"]["requestType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        seen.lock().unwrap().push(kind.clone());
        let (result, events) = answer(&kind, &request["d"]["requestData"]);
        // Err(0): hang up without an answer.
        if result == Err(0) {
            return;
        }
        let mut reply =
            json!({"op": 7, "d": {"requestType": kind, "requestId": request["d"]["requestId"]}});
        match result {
            Ok(data) => {
                reply["d"]["requestStatus"] = json!({"result": true, "code": 100});
                reply["d"]["responseData"] = data;
            }
            Err(code) => {
                reply["d"]["requestStatus"] =
                    json!({"result": false, "code": code, "comment": format!("refused {kind}")});
            }
        }
        if send(&mut ws, reply).is_err() {
            return;
        }
        for event in events {
            let _ = send(&mut ws, json!({"op": 5, "d": event}));
        }
    }
}

fn studio_obs(kind: &str, data: &Value) -> (Result<Value, u64>, Vec<Value>) {
    let event =
        |kind: &str, data: Value| json!({"eventType": kind, "eventIntent": 1, "eventData": data});
    match kind {
        "GetCurrentProgramScene" => (Ok(json!({"sceneName": "Intro"})), vec![]),
        "GetReplayBufferStatus" => (Err(604), vec![]),
        "GetVirtualCamStatus" | "GetStreamStatus" => (Ok(json!({"outputActive": false})), vec![]),
        "GetRecordStatus" => (
            Ok(json!({"outputActive": false, "outputPaused": false, "outputDuration": 0})),
            vec![],
        ),
        "GetInputList" => (
            Ok(json!({"inputs": [{"inputName": "Mic/Aux"}, {"inputName": "Webcam"}]})),
            vec![],
        ),
        "GetInputMute" if data["inputName"] == "Mic/Aux" => {
            (Ok(json!({"inputMuted": false})), vec![])
        }
        "GetInputMute" => (Err(604), vec![]),
        "SetCurrentProgramScene" if data["sceneName"] == "Missing" => (Err(600), vec![]),
        "SetCurrentProgramScene" => (
            Ok(Value::Null),
            vec![event(
                "CurrentProgramSceneChanged",
                json!({"sceneName": data["sceneName"]}),
            )],
        ),
        "ToggleInputMute" => (
            Ok(json!({"inputMuted": true})),
            vec![event(
                "InputMuteStateChanged",
                json!({"inputName": data["inputName"], "inputMuted": true}),
            )],
        ),
        _ => (Ok(Value::Null), vec![]),
    }
}

fn logger() -> nexus_events::EventLogger {
    let path = std::env::temp_dir().join(format!("nexus-obs-events-{}.jsonl", std::process::id()));
    nexus_events::EventLogger::with_path("test", path)
}

fn until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn follows_obs_and_sends_commands_while_wanted() {
    let (server, seen) = fake_obs(studio_obs);
    let (feedback_tx, feedback) = mpsc::channel();
    let obs = Obs::start(
        Box::new(move || Some(server.clone())),
        feedback_tx,
        logger(),
    );
    assert!(obs
        .send("scene", ObsCommand::SetScene, Some("Live".into()))
        .is_err());
    assert_eq!(obs.info(Instant::now()), None);

    obs.set_wanted(true);
    let info = until("connection", || obs.info(Instant::now()));
    assert_eq!(info.scene, "Intro");
    assert_eq!(info.muted, HashMap::from([("Mic/Aux".to_string(), false)]));
    assert!(!info.replay_buffer && !info.recording);

    obs.send("scene", ObsCommand::SetScene, Some("Live".into()))
        .unwrap();
    until("scene change", || {
        obs.info(Instant::now()).filter(|info| info.scene == "Live")
    });
    obs.send("mic", ObsCommand::ToggleMute, Some("Mic/Aux".into()))
        .unwrap();
    until("mute", || {
        obs.info(Instant::now())
            .filter(|info| info.muted.get("Mic/Aux") == Some(&true))
    });
    obs.send("scene", ObsCommand::SetScene, Some("Missing".into()))
        .unwrap();
    let refused = feedback.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(refused.label, "scene");
    assert_eq!(refused.error, "OBS: refused SetCurrentProgramScene");

    obs.set_wanted(false);
    until("disconnect", || {
        obs.info(Instant::now()).is_none().then_some(())
    });
    let seen = seen.lock().unwrap().clone();
    assert!(seen.contains(&"ToggleInputMute".to_string()), "{seen:?}");
}

#[test]
fn reports_why_obs_is_not_connected() {
    // Nothing listens on this port.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = ObsServer {
        host: "127.0.0.1".into(),
        port,
        password: String::new(),
    };
    let (feedback_tx, _feedback) = mpsc::channel();
    let obs = Obs::start(
        Box::new(move || Some(server.clone())),
        feedback_tx,
        logger(),
    );
    obs.set_wanted(true);
    let error = until("a failed attempt", || {
        obs.send("rec", ObsCommand::ToggleRecord, None)
            .err()
            .filter(|error| error.contains("not running"))
    });
    assert_eq!(error, "OBS is not running, or its WebSocket server is off");

    let unset = Obs::start(Box::new(|| None), mpsc::channel().0, logger());
    unset.set_wanted(true);
    assert_eq!(
        unset
            .send("rec", ObsCommand::ToggleRecord, None)
            .unwrap_err(),
        "OBS is not connected; set it up in Studio under Connections"
    );
}

#[test]
fn running_times_count_on_between_measurements() {
    let now = Instant::now();
    let tracked = Tracked {
        recording: true,
        record_ms: 61_000,
        streaming: true,
        stream_ms: 5_000,
        measured: Some(now),
        ..Tracked::default()
    };
    let later = now + Duration::from_millis(1500);
    let info = tracked.info(later);
    assert_eq!((info.record_ms, info.stream_ms), (62_500, 6_500));
    let paused = Tracked {
        record_paused: true,
        ..tracked
    };
    assert_eq!(paused.info(later).record_ms, 61_000);
    assert_eq!(Tracked::default().info(later).record_ms, 0);
}

#[test]
fn profiles_with_obs_widgets_want_obs() {
    let mut profile = nexus_config::default_config().profiles.remove(0);
    assert!(!profile_uses_obs(&profile));
    profile
        .pages
        .push(template_page(PageTemplate::Streaming, "stream"));
    assert!(profile_uses_obs(&profile));

    let mut profile = nexus_config::default_config().profiles.remove(0);
    let text = Widget::new(
        "scene",
        0,
        200,
        WidgetKind::Text(TextProps {
            text: "On air: {obs.scene}".into(),
            ..TextProps::default()
        }),
    );
    profile.pages[0].widgets = vec![text];
    assert!(profile_uses_obs(&profile));

    let mut profile = nexus_config::default_config().profiles.remove(0);
    let nested = ActionSpec::Sequence {
        steps: vec![ActionSpec::Obs {
            command: ObsCommand::SaveReplay,
            target: None,
        }],
        delay_ms: 0,
    };
    profile.pages[0].widgets[0].hold = Some(nested);
    assert!(profile_uses_obs(&profile));
}

/// The fake's pause switch, as OBS's status reports it.
static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While a recording stops, OBS's status still calls it active; the event
/// already says it is not.
fn stopping_obs(kind: &str, _data: &Value) -> (Result<Value, u64>, Vec<Value>) {
    use std::sync::atomic::Ordering;
    let state = |state: &str, active: bool| {
        json!({"eventType": "RecordStateChanged", "eventIntent": 64,
               "eventData": {"outputState": state, "outputActive": active}})
    };
    match kind {
        "GetCurrentProgramScene" => (Ok(json!({"sceneName": "Scene"})), vec![]),
        "GetInputList" => (Ok(json!({"inputs": []})), vec![]),
        "GetRecordStatus" => (
            Ok(
                json!({"outputActive": true, "outputPaused": PAUSED.load(Ordering::SeqCst),
                      "outputDuration": 4200}),
            ),
            vec![],
        ),
        "StartRecord" => (
            Ok(Value::Null),
            vec![state("OBS_WEBSOCKET_OUTPUT_STARTED", true)],
        ),
        "StopRecord" => (
            Ok(Value::Null),
            vec![state("OBS_WEBSOCKET_OUTPUT_STOPPING", false)],
        ),
        "ToggleRecordPause" => {
            PAUSED.store(true, Ordering::SeqCst);
            (
                Ok(Value::Null),
                vec![state("OBS_WEBSOCKET_OUTPUT_PAUSED", true)],
            )
        }
        _ => (Ok(json!({"outputActive": false})), vec![]),
    }
}

#[test]
fn a_stopping_recording_is_off_at_once() {
    let (server, _) = fake_obs(stopping_obs);
    let (feedback_tx, _feedback) = mpsc::channel();
    let obs = Obs::start(
        Box::new(move || Some(server.clone())),
        feedback_tx,
        logger(),
    );
    obs.set_wanted(true);
    let info = until("connection", || obs.info(Instant::now()));
    // The status at connecting decides: OBS says it records.
    assert!(info.recording);
    obs.send("pause", ObsCommand::PauseRecord, None).unwrap();
    until("pause", || {
        obs.info(Instant::now()).filter(|info| info.record_paused)
    });
    obs.send("stop", ObsCommand::StopRecord, None).unwrap();
    until("stop", || {
        obs.info(Instant::now()).filter(|info| !info.recording)
    });
    // Later measurements do not light it up again.
    thread::sleep(OUTPUTS_EVERY + Duration::from_millis(300));
    let info = obs.info(Instant::now()).unwrap();
    assert!(!info.recording && !info.record_paused && info.record_ms == 0);
    obs.send("start", ObsCommand::StartRecord, None).unwrap();
    let info = until("start", || {
        obs.info(Instant::now()).filter(|info| info.recording)
    });
    assert!(info.record_ms >= 4200);
}

/// A second collection: the scene changes, and SaveReplayBuffer hangs up.
static COLLECTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn live_obs(kind: &str, _data: &Value) -> (Result<Value, u64>, Vec<Value>) {
    use std::sync::atomic::Ordering;
    let event =
        |kind: &str, data: Value| json!({"eventType": kind, "eventIntent": 64, "eventData": data});
    match kind {
        "GetCurrentProgramScene" if COLLECTION.load(Ordering::SeqCst) => {
            (Ok(json!({"sceneName": "Other collection"})), vec![])
        }
        "GetCurrentProgramScene" => (Ok(json!({"sceneName": "Scene"})), vec![]),
        "GetInputList" => (Ok(json!({"inputs": []})), vec![]),
        "GetStreamStatus" => (
            Ok(json!({"outputActive": false, "outputReconnecting": true, "outputDuration": 9000})),
            vec![],
        ),
        "ToggleStream" => (
            Ok(Value::Null),
            vec![event(
                "StreamStateChanged",
                json!({"outputActive": false, "outputState": "OBS_WEBSOCKET_OUTPUT_RECONNECTING"}),
            )],
        ),
        "ToggleVirtualCam" => {
            COLLECTION.store(true, Ordering::SeqCst);
            (
                Ok(Value::Null),
                vec![event(
                    "CurrentSceneCollectionChanged",
                    json!({"sceneCollectionName": "B"}),
                )],
            )
        }
        "SaveReplayBuffer" => (Err(0), vec![]),
        _ => (Ok(json!({"outputActive": false})), vec![]),
    }
}

#[test]
fn reconnecting_streams_new_collections_and_lost_commands() {
    let (server, seen) = fake_obs(live_obs);
    let (feedback_tx, feedback) = mpsc::channel();
    let obs = Obs::start(
        Box::new(move || Some(server.clone())),
        feedback_tx,
        logger(),
    );
    obs.set_wanted(true);
    until("connection", || obs.info(Instant::now()));

    // A stream that reconnects is still live (and amber, not off).
    obs.send("live", ObsCommand::ToggleStream, None).unwrap();
    let info = until("reconnecting", || {
        obs.info(Instant::now())
            .filter(|info| info.stream_reconnecting)
    });
    assert!(info.streaming && info.stream_ms >= 9000);

    // Another scene collection: everything is read again.
    obs.send("cam", ObsCommand::ToggleVirtualCam, None).unwrap();
    until("resync", || {
        obs.info(Instant::now())
            .filter(|info| info.scene == "Other collection")
    });

    // OBS hangs up instead of answering: the button says so.
    obs.send("clip", ObsCommand::SaveReplay, None).unwrap();
    let lost = feedback.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(lost.label, "clip");
    assert!(
        lost.error.starts_with("connection to OBS lost"),
        "{}",
        lost.error
    );
    assert!(seen
        .lock()
        .unwrap()
        .contains(&"SaveReplayBuffer".to_string()));
}

#[test]
fn a_dry_run_sends_nothing() {
    let (server, seen) = fake_obs(studio_obs);
    let (feedback_tx, _feedback) = mpsc::channel();
    let mut obs = Obs::start(
        Box::new(move || Some(server.clone())),
        feedback_tx,
        logger(),
    );
    obs.dry_run = Some(logger());
    obs.set_wanted(true);
    until("connection", || obs.info(Instant::now()));
    obs.send("scene", ObsCommand::SetScene, Some("Live".into()))
        .unwrap();
    thread::sleep(Duration::from_millis(400));
    let seen = seen.lock().unwrap().clone();
    assert!(
        !seen.contains(&"SetCurrentProgramScene".to_string()),
        "{seen:?}"
    );
    assert_eq!(obs.info(Instant::now()).unwrap().scene, "Intro");
}
