//! PipeWire's objects as `pw-dump` prints them, folded into one state, and
//! what the strip needs from it: devices, the default ones, the streams
//! that play or record and the cameras in use.
//!
//! `pw-dump --monitor` prints whole objects when nodes, devices, clients or
//! links change, `{"id": N, "info": null}` when they go, and for metadata
//! only the entries that changed.

use super::{AudioSnapshot, CameraUse, Device, DeviceKind, Stream};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

type Props = Map<String, Value>;

/// Nodes looked at at most when following what feeds a virtual input.
const FEED_SEARCH: usize = 64;

/// Everything PipeWire told about its objects so far, by object id.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    objects: BTreeMap<u32, Value>,
}

impl Graph {
    /// A graph from one complete `pw-dump` document.
    pub fn from_dump(text: &str) -> Result<Self, String> {
        let document: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let mut graph = Self::default();
        graph.apply(&document);
        Ok(graph)
    }

    /// Folds one document of `pw-dump` (an array of objects) in.
    pub fn apply(&mut self, document: &Value) {
        let Some(items) = document.as_array() else {
            return;
        };
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let Ok(id) = u32::try_from(id) else {
                continue;
            };
            if item.get("type").is_none() {
                if item.get("info").is_some_and(Value::is_null) {
                    self.objects.remove(&id);
                }
                continue;
            }
            match self.objects.get_mut(&id) {
                Some(old) if old.get("type") == item.get("type") => merge(old, item),
                _ => {
                    self.objects.insert(id, item.clone());
                }
            }
        }
    }

    fn of_type<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = (u32, &'a Value)> + 'a {
        let full = format!("PipeWire:Interface:{kind}");
        self.objects
            .iter()
            .filter(move |(_, object)| object.get("type").and_then(Value::as_str) == Some(&full))
            .map(|(id, object)| (*id, object))
    }

    /// What the strip shows of the graph now.
    pub fn snapshot(&self) -> AudioSnapshot {
        let devices: HashMap<u32, &Props> = self
            .of_type("Device")
            .filter_map(|(id, object)| Some((id, info_props(object)?)))
            .collect();
        let clients: HashMap<u32, &Props> = self
            .of_type("Client")
            .filter_map(|(id, object)| Some((id, info_props(object)?)))
            .collect();
        let links: Vec<(u32, u32)> = self
            .of_type("Link")
            .filter_map(|(_, object)| {
                let info = object.get("info")?;
                let out = u32::try_from(info.get("output-node-id")?.as_u64()?).ok()?;
                let input = u32::try_from(info.get("input-node-id")?.as_u64()?).ok()?;
                Some((out, input))
            })
            .collect();
        let mut snapshot = AudioSnapshot::default();
        self.read_defaults(&mut snapshot);
        let wiring = Wiring::new(self, &links);
        let targets = self.stream_targets(&wiring);

        let mut outputs = Vec::new();
        let mut inputs = Vec::new();
        let mut video_sources = HashMap::new();
        for (id, object) in self.of_type("Node") {
            let Some(props) = info_props(object) else {
                continue;
            };
            let device = number(props, "device.id")
                .and_then(|device| devices.get(&device))
                .copied();
            let client = number(props, "client.id")
                .and_then(|client| clients.get(&client))
                .copied();
            let class = text(props, "media.class").unwrap_or_default();
            let running = object
                .pointer("/info/state")
                .and_then(Value::as_str)
                .is_some_and(|state| state == "running");
            // Loopbacks and filters PipeWire runs itself are no app.
            if class.starts_with("Stream/") && is_internal(client) {
                continue;
            }
            match class {
                "Audio/Sink" => outputs.push(device_of(id, object, props, device, false)),
                "Audio/Source" | "Audio/Source/Virtual" => {
                    let mut input = device_of(id, object, props, device, true);
                    input.virtual_device |= class.ends_with("Virtual");
                    input.monitor_only = input.virtual_device && wiring.monitor_only(id);
                    inputs.push(input);
                }
                "Stream/Output/Audio" => {
                    let peers = links
                        .iter()
                        .filter(|(out, _)| *out == id)
                        .map(|(_, input)| *input);
                    let mut stream = stream_of(id, object, props, client, running, peers);
                    stream.target = match targets.get(&id) {
                        Some(target) => *target,
                        None => wiring.target_in(props),
                    };
                    snapshot.playback.push(stream);
                }
                "Stream/Input/Audio" if !is_peak_meter(props) => {
                    let peers = links
                        .iter()
                        .filter(|(_, input)| *input == id)
                        .map(|(out, _)| *out);
                    snapshot
                        .recording
                        .push(stream_of(id, object, props, client, running, peers));
                }
                "Video/Source" => {
                    let name = device
                        .and_then(|d| text(d, "device.product.name"))
                        .or_else(|| text(props, "node.nick"))
                        .or_else(|| text(props, "node.description"))
                        .unwrap_or("Camera")
                        .trim_end_matches(" (V4L2)")
                        .to_string();
                    if let Some(path) = text(props, "api.v4l2.path") {
                        snapshot
                            .video_devices
                            .push((path.to_string(), name.clone()));
                    }
                    video_sources.insert(id, (name, running));
                }
                _ => {}
            }
        }
        // A camera PipeWire hands to an app (through the portal, or to
        // OBS's PipeWire source) is linked from its video source node.
        let consumers: HashMap<u32, (String, Option<u32>)> = self
            .of_type("Node")
            .filter_map(|(id, object)| {
                let props = info_props(object)?;
                let client = number(props, "client.id")
                    .and_then(|c| clients.get(&c))
                    .copied();
                Some((id, (app_name(props, client), pid(client))))
            })
            .collect();
        for (out, input) in &links {
            let Some((device, true)) = video_sources.get(out) else {
                continue;
            };
            if let Some((app, pid)) = consumers.get(input) {
                let camera = CameraUse {
                    app: app.clone(),
                    pid: *pid,
                    device: device.clone(),
                };
                if !snapshot.cameras.contains(&camera) {
                    snapshot.cameras.push(camera);
                }
            }
        }
        snapshot.apps_by_pid = clients
            .values()
            .filter_map(|client| Some((pid(Some(client))?, app_name(client, None))))
            .collect();
        snapshot.apps_by_pid.sort();
        let by_preference = |a: &Device, b: &Device| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.description.cmp(&b.description))
        };
        outputs.sort_by(by_preference);
        inputs.sort_by(by_preference);
        snapshot.outputs = outputs;
        snapshot.inputs = inputs;
        snapshot
    }

    /// The entries of the "default" metadata.
    fn default_metadata(&self) -> impl Iterator<Item = &Value> {
        let metadata = self.of_type("Metadata").find(|(_, object)| {
            object
                .pointer("/props/metadata.name")
                .and_then(Value::as_str)
                == Some("default")
        });
        metadata
            .and_then(|(_, object)| object.get("metadata")?.as_array())
            .into_iter()
            .flatten()
    }

    /// The default output and input from the "default" metadata.
    fn read_defaults(&self, snapshot: &mut AudioSnapshot) {
        for entry in self.default_metadata() {
            if entry.get("subject").and_then(Value::as_u64) != Some(0) {
                continue;
            }
            let name = entry.get("value").and_then(default_name);
            match entry.get("key").and_then(Value::as_str) {
                Some("default.audio.sink") => snapshot.default_output = name,
                Some("default.audio.source") => snapshot.default_input = name,
                _ => {}
            }
        }
    }

    /// Streams whose target the "default" metadata sets, read the way
    /// WirePlumber reads it: `target.object` (a serial or a name) before
    /// `target.node` (an id), and -1 for "follow the default".
    fn stream_targets(&self, wiring: &Wiring) -> HashMap<u32, Option<u32>> {
        let mut found: HashMap<u32, (bool, Option<u32>)> = HashMap::new();
        for entry in self.default_metadata() {
            let Some(subject) = entry
                .get("subject")
                .and_then(Value::as_u64)
                .and_then(|s| u32::try_from(s).ok())
                .filter(|s| *s != 0)
            else {
                continue;
            };
            let by_serial = match entry.get("key").and_then(Value::as_str) {
                Some("target.object") => true,
                Some("target.node") => false,
                _ => continue,
            };
            let Some(value) = entry.get("value").and_then(scalar) else {
                continue;
            };
            if !by_serial && found.get(&subject).is_some_and(|(serial, _)| *serial) {
                continue;
            }
            found.insert(subject, (by_serial, wiring.resolve(&value, by_serial)));
        }
        found
            .into_iter()
            .map(|(stream, (_, node))| (stream, node))
            .collect()
    }
}

/// A metadata or property value as text: "123" and 123 alike.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn serial(props: &Props) -> Option<u64> {
    let value = props.get("object.serial")?;
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

/// Where the sound reaching a node comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feed {
    /// A microphone or another input with hardware behind it.
    Capture,
    /// An output's monitor: what it plays.
    Monitor,
    /// Nothing that tells.
    Unknown,
}

/// The nodes and links, to follow what feeds a virtual input upstream.
struct Wiring<'a> {
    nodes: Vec<(u32, &'a Props)>,
    links: &'a [(u32, u32)],
}

impl<'a> Wiring<'a> {
    fn new(graph: &'a Graph, links: &'a [(u32, u32)]) -> Self {
        let nodes = graph
            .of_type("Node")
            .filter_map(|(id, object)| Some((id, info_props(object)?)))
            .collect();
        Self { nodes, links }
    }

    fn props(&self, id: u32) -> Option<&'a Props> {
        self.nodes.iter().find(|(n, _)| *n == id).map(|(_, p)| *p)
    }

    /// The node a target names: by serial (`target.object`) or by id
    /// (`target.node`) when it is a number, else by `node.name`.
    fn resolve(&self, value: &str, by_serial: bool) -> Option<u32> {
        if value.is_empty() || value == "-1" {
            return None;
        }
        let found = match value.parse::<u64>() {
            Ok(n) if by_serial => self.nodes.iter().find(|(_, p)| serial(p) == Some(n)),
            Ok(n) => self.nodes.iter().find(|(id, _)| u64::from(*id) == n),
            Err(_) => self
                .nodes
                .iter()
                .find(|(_, p)| text(p, "node.name") == Some(value)),
        };
        found.map(|(id, _)| *id)
    }

    /// The node a stream's own properties send it to.
    fn target_in(&self, props: &Props) -> Option<u32> {
        if let Some(value) = props.get("target.object").and_then(scalar) {
            return self.resolve(&value, true);
        }
        let value = props.get("node.target").and_then(scalar)?;
        self.resolve(&value, false)
    }

    /// A virtual input gets only what outputs play: the loopback card's
    /// capture side, or a chain fed by monitors alone. Not knowing is not
    /// "only monitors".
    fn monitor_only(&self, id: u32) -> bool {
        if self.props(id).is_some_and(is_loopback_card) {
            return true;
        }
        let mut seen = HashSet::from([id]);
        self.feed_into(id, &mut seen) == Some(Feed::Monitor)
    }

    /// What is linked into `id` or into another node of its filter (nodes
    /// sharing a `node.link-group`, such as a filter's source and the
    /// stream that captures for it).
    fn feed_into(&self, id: u32, seen: &mut HashSet<u32>) -> Option<Feed> {
        let group = self.props(id).and_then(|p| text(p, "node.link-group"));
        let members: Vec<u32> = self
            .nodes
            .iter()
            .filter(|(n, p)| *n == id || (group.is_some() && text(p, "node.link-group") == group))
            .map(|(n, _)| *n)
            .collect();
        seen.extend(members.iter().copied());
        let mut feeds = Vec::new();
        for (out, input) in self.links {
            if members.contains(input) {
                feeds.extend(self.feed_of(*out, seen));
            }
        }
        if feeds.contains(&Feed::Capture) {
            Some(Feed::Capture)
        } else if feeds.contains(&Feed::Unknown) {
            Some(Feed::Unknown)
        } else {
            feeds.first().copied()
        }
    }

    /// What node `id`, linked into the chain, brings into it.
    fn feed_of(&self, id: u32, seen: &mut HashSet<u32>) -> Option<Feed> {
        if !seen.insert(id) {
            return None;
        }
        if seen.len() > FEED_SEARCH {
            return Some(Feed::Unknown);
        }
        let Some(props) = self.props(id) else {
            return Some(Feed::Unknown);
        };
        match text(props, "media.class").unwrap_or_default() {
            // A link out of a sink leaves from its monitor ports.
            "Audio/Sink" => Some(Feed::Monitor),
            "Audio/Source" | "Audio/Duplex" if is_loopback_card(props) => Some(Feed::Monitor),
            "Audio/Source" | "Audio/Duplex" if !is_virtual(props) => Some(Feed::Capture),
            _ => Some(self.feed_into(id, seen).unwrap_or(Feed::Unknown)),
        }
    }
}

/// `{"name": "…"}`, also when it comes as JSON inside a string.
fn default_name(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map.get("name")?.as_str().map(str::to_string),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map.get("name")?.as_str().map(str::to_string),
            _ => Some(text.clone()).filter(|name| !name.is_empty()),
        },
        _ => None,
    }
}

/// Takes in a newer print of an object: info fields and params replace
/// the old ones, metadata entries are set or removed one by one.
fn merge(old: &mut Value, new: &Value) {
    if let Some(entries) = new.get("metadata").and_then(Value::as_array) {
        if !old.get("metadata").is_some_and(Value::is_array) {
            old["metadata"] = Value::Array(Vec::new());
        }
        if let Some(list) = old["metadata"].as_array_mut() {
            for entry in entries {
                let key = (entry.get("subject"), entry.get("key"));
                list.retain(|old| (old.get("subject"), old.get("key")) != key);
                if !entry.get("value").is_none_or(Value::is_null) {
                    list.push(entry.clone());
                }
            }
        }
    }
    if let Some(props) = new.get("props") {
        old["props"] = props.clone();
    }
    let Some(info) = new.get("info").and_then(Value::as_object) else {
        return;
    };
    if !old.get("info").is_some_and(Value::is_object) {
        old["info"] = Value::Object(Map::new());
    }
    let Some(old_info) = old["info"].as_object_mut() else {
        return;
    };
    for (key, value) in info {
        match (key.as_str(), value.as_object()) {
            ("params", Some(params)) => {
                let entry = old_info
                    .entry("params")
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(old_params) = entry.as_object_mut() {
                    for (name, list) in params {
                        old_params.insert(name.clone(), list.clone());
                    }
                }
            }
            _ => {
                old_info.insert(key.clone(), value.clone());
            }
        }
    }
}

fn info_props(object: &Value) -> Option<&Props> {
    object.pointer("/info/props")?.as_object()
}

fn text<'a>(props: &'a Props, key: &str) -> Option<&'a str> {
    props
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn number(props: &Props, key: &str) -> Option<u32> {
    let value = props.get(key)?;
    let n = value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())?;
    u32::try_from(n).ok()
}

fn flag(props: &Props, key: &str) -> bool {
    match props.get(key) {
        Some(Value::Bool(on)) => *on,
        Some(Value::String(text)) => matches!(text.as_str(), "true" | "1"),
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        _ => false,
    }
}

fn pid(client: Option<&Props>) -> Option<u32> {
    let client = client?;
    number(client, "application.process.id").or_else(|| number(client, "pipewire.sec.pid"))
}

/// Volume in percent the way `wpctl` shows it (cube root of the linear
/// level), and the mute flag, from a node's `Props`.
fn volume_of(object: &Value) -> (u8, bool) {
    let props = object
        .pointer("/info/params/Props")
        .and_then(Value::as_array);
    let mut level = None;
    let mut muted = false;
    for entry in props.into_iter().flatten() {
        if let Some(channels) = entry.get("channelVolumes").and_then(Value::as_array) {
            let loudest = channels
                .iter()
                .filter_map(Value::as_f64)
                .fold(None, |max: Option<f64>, v| {
                    Some(max.map_or(v, |m| m.max(v)))
                });
            level = level.or(loudest);
        }
        if let Some(mute) = entry.get("mute").and_then(Value::as_bool) {
            muted |= mute;
        }
    }
    let percent = level.map_or(0.0, |linear| linear.max(0.0).cbrt() * 100.0);
    (percent.round().clamp(0.0, 255.0) as u8, muted)
}

/// A sink or source as a tile: what it is, what to call it, and whether
/// it is only a loopback or another device without hardware behind it.
fn device_of(
    id: u32,
    object: &Value,
    props: &Props,
    device: Option<&Props>,
    input: bool,
) -> Device {
    let node = text(props, "node.name").unwrap_or_default().to_string();
    let description = text(props, "node.description")
        .or_else(|| text(props, "node.nick"))
        .unwrap_or(&node)
        .to_string();
    let (name, detail) = names(props, device, &description);
    let (volume, muted) = volume_of(object);
    let priority = props
        .get("priority.session")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    Device {
        id,
        node,
        name,
        detail,
        description,
        kind: if input {
            DeviceKind::Microphone
        } else {
            output_kind(props, device)
        },
        volume,
        muted,
        virtual_device: is_virtual(props),
        monitor_only: false,
        priority,
        serial: serial(props),
    }
}

/// Loopbacks, null sinks, filters and other devices without hardware
/// behind them.
fn is_virtual(props: &Props) -> bool {
    let lower = |key| text(props, key).unwrap_or_default().to_ascii_lowercase();
    flag(props, "node.virtual")
        || lower("factory.name") == "support.null-audio-sink"
        || is_loopback_card(props)
        || lower("node.name").starts_with("input.loopback")
        || lower("node.name").starts_with("output.loopback")
        // Echo cancelling, noise filters and loopbacks PipeWire runs.
        || (text(props, "node.link-group").is_some() && number(props, "device.id").is_none())
}

/// The ALSA loopback card: its capture side gets what its playback side
/// plays.
fn is_loopback_card(props: &Props) -> bool {
    let lower = |key| text(props, key).unwrap_or_default().to_ascii_lowercase();
    lower("alsa.driver_name") == "snd_aloop" || lower("api.alsa.card.name") == "loopback"
}

/// A headset, a screen with speakers, or a speaker.
pub(crate) fn output_kind(props: &Props, device: Option<&Props>) -> DeviceKind {
    let get = |key: &str| {
        text(props, key)
            .or_else(|| device.and_then(|d| text(d, key)))
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let form = get("device.form-factor");
    let icon = get("device.icon-name");
    let profile_set = get("device.profile-set");
    let bluetooth_profile = get("api.bluez5.profile");
    let headset_words = ["headset", "headphone", "kopfhörer", "casque"];
    let product = format!(
        "{} {}",
        get("device.product.name"),
        get("device.description")
    );
    if matches!(
        form.as_str(),
        "headset" | "headphone" | "hands-free" | "handset"
    ) || icon.contains("headset")
        || icon.contains("headphone")
        || profile_set.contains("headset")
        || ["headset", "hfp", "hsp"]
            .iter()
            .any(|p| bluetooth_profile.contains(p))
        || headset_words.iter().any(|word| product.contains(word))
    {
        return DeviceKind::Headset;
    }
    let port = format!(
        "{} {} {}",
        get("device.profile.name"),
        get("node.name"),
        get("node.description")
    );
    if matches!(form.as_str(), "tv" | "monitor")
        || ["hdmi", "displayport", "dp-"]
            .iter()
            .any(|word| port.contains(word))
    {
        return DeviceKind::Monitor;
    }
    DeviceKind::Speaker
}

/// What a tile calls a device and the smaller line below. A long ALSA
/// name such as "Navi 10 HDMI Audio Digital Stereo (HDMI 3)" becomes
/// "HDMI 3" over "Navi 10 HDMI".
pub(crate) fn names(props: &Props, device: Option<&Props>, description: &str) -> (String, String) {
    let card = device
        .and_then(|d| text(d, "device.description"))
        .unwrap_or_default();
    let product = device
        .and_then(|d| text(d, "device.product.name"))
        .or_else(|| text(props, "api.alsa.card.name"))
        .unwrap_or(card);
    let product = short_product(product);
    let api = text(props, "device.api")
        .or_else(|| device.and_then(|d| text(d, "device.api")))
        .unwrap_or_default();
    let bus = text(props, "device.bus")
        .or_else(|| device.and_then(|d| text(d, "device.bus")))
        .unwrap_or_default();
    if api == "bluez5" || bus == "bluetooth" {
        let name = if card.is_empty() { description } else { card };
        return (name.to_string(), "Bluetooth".to_string());
    }
    if bus == "usb" {
        if product.is_empty() || product == description {
            return (description.to_string(), "USB".to_string());
        }
        // "Astro A50 Spiel" of an "Astro A50" reads "A50 Spiel".
        let name = match product.split_once(' ') {
            Some((maker, _)) if description.starts_with(&format!("{product} ")) => {
                description[maker.len()..].trim_start().to_string()
            }
            _ => description.to_string(),
        };
        return (name, product);
    }
    let profile = text(props, "device.profile.description").unwrap_or_default();
    if !card.is_empty() && !profile.is_empty() && description.starts_with(card) {
        let name = match profile
            .rsplit_once('(')
            .and_then(|(_, rest)| rest.strip_suffix(')'))
        {
            Some(port) if port.eq_ignore_ascii_case("iec958") => "S/PDIF".to_string(),
            Some(port) => port.to_string(),
            None => profile.to_string(),
        };
        return (name, product);
    }
    (description.to_string(), product)
}

/// "Starship/Matisse HD Audio Controller" → "Starship/Matisse".
fn short_product(product: &str) -> String {
    let mut short = product.trim();
    for suffix in [" HD Audio Controller", " Audio Controller", " Audio"] {
        if let Some(rest) = short.strip_suffix(suffix) {
            short = rest;
            break;
        }
    }
    short.to_string()
}

fn is_internal(client: Option<&Props>) -> bool {
    client
        .and_then(|c| text(c, "application.process.binary"))
        .is_some_and(|binary| matches!(binary, "pipewire" | "wireplumber"))
}

/// Peak meters of volume applets look at a device without recording it.
fn is_peak_meter(props: &Props) -> bool {
    flag(props, "stream.monitor") || flag(props, "pulse.peak-detect")
}

fn stream_of(
    id: u32,
    object: &Value,
    props: &Props,
    client: Option<&Props>,
    running: bool,
    peers: impl Iterator<Item = u32>,
) -> Stream {
    let (volume, muted) = volume_of(object);
    let mut peers: Vec<u32> = peers.collect();
    peers.sort_unstable();
    peers.dedup();
    Stream {
        id,
        app: app_name(props, client),
        node: text(props, "node.name").unwrap_or_default().to_string(),
        pid: pid(client),
        volume,
        muted,
        active: running && !flag(props, "pulse.corked"),
        peers,
        target: None,
    }
}

/// Names apps give their streams that say nothing about the app.
const GENERIC_NAMES: [&str; 8] = [
    "chromium",
    "chromium input",
    "webrtc voiceengine",
    "electron",
    "pipewire alsa",
    "pw-cat",
    "pw-play",
    "pw-record",
];

/// The app behind a node: its own name, else its client's, else the
/// program's; generic names (Electron apps call themselves Chromium) give
/// way to the program's name.
pub(crate) fn app_name(props: &Props, client: Option<&Props>) -> String {
    let binary = client
        .and_then(|c| text(c, "application.process.binary"))
        .or_else(|| text(props, "application.process.binary"));
    let named = text(props, "application.name")
        .or_else(|| client.and_then(|c| text(c, "application.name")));
    if let Some(name) = named {
        // "ALSA plug-in [mpv]" names the program in brackets.
        if let Some(inner) = name
            .strip_prefix("ALSA plug-in [")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            return super::pretty_app_name(inner);
        }
        let lower = name.to_lowercase();
        // Chromium's capture streams: "Chromium input", "Brave input".
        let generic = GENERIC_NAMES.contains(&lower.as_str()) || lower.ends_with(" input");
        // Named the way the pill names programs found in /proc.
        match binary {
            Some(binary) if generic => return super::pretty_app_name(binary),
            _ => return name.to_string(),
        }
    }
    if let Some(binary) = binary {
        return super::pretty_app_name(binary);
    }
    text(props, "node.description")
        .or_else(|| text(props, "media.name"))
        .or_else(|| text(props, "node.name"))
        .unwrap_or("?")
        .to_string()
}
