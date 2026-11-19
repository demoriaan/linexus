//! PipeWire's sound outputs, microphones and the apps playing now, for the
//! sound settings and the pickers of the sound widgets and buttons. Studio
//! only reads; the strip changes devices and volumes.

use crate::http::{Request, Response};
use nexus_sensors::audio::{AudioSnapshot, Device, DeviceKind};
use serde_json::{json, Value};

pub fn route(request: &Request) -> Option<Response> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/audio") => Some(devices()),
        _ => None,
    }
}

fn devices() -> Response {
    match nexus_sensors::audio::read_snapshot() {
        Ok(snapshot) => Response::json(200, &describe(&snapshot)),
        Err(error) => Response::error(503, format!("PipeWire is not reachable: {error}")),
    }
}

fn describe(snapshot: &AudioSnapshot) -> Value {
    let mut apps: Vec<&str> = Vec::new();
    for stream in &snapshot.playback {
        if !apps.contains(&stream.app.as_str()) {
            apps.push(&stream.app);
        }
    }
    json!({
        "ok": true,
        "outputs": snapshot.outputs.iter().map(device).collect::<Vec<_>>(),
        "inputs": snapshot.inputs.iter().map(device).collect::<Vec<_>>(),
        "default_output": snapshot.default_output,
        "default_input": snapshot.default_input,
        "apps": apps,
    })
}

fn device(device: &Device) -> Value {
    let kind = match device.kind {
        DeviceKind::Headset => "headset",
        DeviceKind::Monitor => "monitor",
        DeviceKind::Speaker => "speaker",
        DeviceKind::Microphone => "microphone",
    };
    json!({
        "node": device.node,
        "name": device.name,
        "detail": device.detail,
        "kind": kind,
        "virtual": device.virtual_device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_sensors::audio::Stream;

    #[test]
    fn devices_and_apps_are_described_for_the_pickers() {
        let speaker = Device {
            node: "alsa_output.pci.analog-stereo".into(),
            name: "Lautsprecher".into(),
            detail: "Audio".into(),
            ..Device::default()
        };
        let mic = Device {
            node: "loop.in".into(),
            name: "Schleife".into(),
            kind: DeviceKind::Microphone,
            virtual_device: true,
            ..Device::default()
        };
        let stream = |app: &str| Stream {
            app: app.into(),
            ..Stream::default()
        };
        let snapshot = AudioSnapshot {
            outputs: vec![speaker],
            inputs: vec![mic],
            default_output: Some("alsa_output.pci.analog-stereo".into()),
            playback: vec![stream("Firefox"), stream("Spotify"), stream("Firefox")],
            ..AudioSnapshot::default()
        };
        let body = describe(&snapshot);
        assert_eq!(
            body["outputs"][0],
            json!({
                "node": "alsa_output.pci.analog-stereo",
                "name": "Lautsprecher",
                "detail": "Audio",
                "kind": "speaker",
                "virtual": false,
            })
        );
        assert_eq!(body["inputs"][0]["kind"], "microphone");
        assert_eq!(body["inputs"][0]["virtual"], true);
        assert_eq!(body["default_input"], Value::Null);
        assert_eq!(body["apps"], json!(["Firefox", "Spotify"]));
    }

    #[test]
    fn only_reading_is_routed() {
        let post = crate::state::testing::post("/api/audio", b"{}");
        assert!(route(&post).is_none());
    }
}
