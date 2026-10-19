//! Now-playing information from MPRIS players: track, album art, progress
//! and which player a media widget should follow.
//!
//! Properties are read with `busctl --json=short` (systemd 240+), whose
//! typed JSON is far easier to take apart than the default text format.
//! Older busctl falls back to the text parsers in lib.rs.

use super::command::run_output;
use super::http::percent_decode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Each busctl call; a hung player must not stall the frame loop.
const BUSCTL_TIMEOUT: Duration = Duration::from_millis(300);
/// How long a `busctl list` of running players is reused.
const PLAYER_LIST_REFRESH: Duration = Duration::from_secs(3);
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const MAX_ART_BYTES: u64 = 4 << 20;
/// Downloaded covers kept on disk; the oldest go first.
const ART_CACHE_FILES: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaState {
    /// D-Bus name, e.g. `org.mpris.MediaPlayer2.spotify`; the address for
    /// play/pause commands.
    pub bus_name: String,
    /// Player name for display (`Spotify`, `Brave`).
    pub identity: String,
    /// Empty when the player does not report it.
    pub title: String,
    /// All artists, comma separated; empty when unknown.
    pub artist: String,
    pub album: String,
    /// `file://` or `http(s)://` cover address; see [`fetch_art_bytes`].
    pub art_url: Option<String>,
    pub length_s: Option<f32>,
    pub position_s: Option<f32>,
    pub playing: bool,
    /// False for players that only report what they play (no remote control).
    pub can_control: bool,
}

/// The player a media widget should show: the one whose bus name contains
/// `preferred` (case-insensitive) when it is running, otherwise the first
/// playing player, otherwise the first paused one. Stopped players are not
/// shown. Never starts a player through D-Bus activation.
pub fn current_media(preferred: Option<&str>) -> Option<MediaState> {
    current_media_and_players(preferred).0
}

/// The selected player's state plus every running MPRIS player, from one
/// `busctl list` (the metrics collector needs both every second).
pub fn current_media_and_players(preferred: Option<&str>) -> (Option<MediaState>, Vec<String>) {
    let players = list_players();
    (select_media(&players, preferred), players)
}

fn select_media(players: &[String], preferred: Option<&str>) -> Option<MediaState> {
    let preferred = preferred.map(str::trim).filter(|value| !value.is_empty());
    let probes: Vec<PlayerProbe> = match preferred.and_then(|p| preferred_index(players, p)) {
        Some(index) => vec![probe_player(&players[index])],
        None => players.iter().map(|bus| probe_player(bus)).collect(),
    };
    let statuses: Vec<(&str, Option<&str>)> = probes
        .iter()
        .map(|probe| (probe.bus_name.as_str(), probe.status.as_deref()))
        .collect();
    let index = select_player(&statuses, preferred)?;
    let probe = probes.into_iter().nth(index)?;

    let position_us = read_properties(&probe.bus_name, PLAYER_INTERFACE, &["Position"])
        .first()
        .and_then(Value::as_f64);
    let identity = read_properties(&probe.bus_name, ROOT_INTERFACE, &["Identity"])
        .first()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| identity_from_bus_name(&probe.bus_name));
    Some(probe.into_state(identity, position_us))
}

#[derive(Debug, Clone, Default, PartialEq)]
struct PlayerProbe {
    bus_name: String,
    status: Option<String>,
    track: TrackMetadata,
    can_control: Option<bool>,
}

impl PlayerProbe {
    fn into_state(self, identity: String, position_us: Option<f64>) -> MediaState {
        let length_s = self
            .track
            .length_us
            .filter(|us| *us > 0.0)
            .map(|us| (us / 1_000_000.0) as f32);
        let position_s = position_us.filter(|us| *us >= 0.0).map(|us| {
            let seconds = (us / 1_000_000.0) as f32;
            length_s.map_or(seconds, |length| seconds.min(length))
        });
        MediaState {
            bus_name: self.bus_name,
            identity,
            title: self.track.title,
            artist: self.track.artist,
            album: self.track.album,
            art_url: self.track.art_url,
            length_s,
            position_s,
            playing: self.status.as_deref() == Some("Playing"),
            // A player that does not say is assumed controllable: a play/pause
            // it ignores costs nothing.
            can_control: self.can_control.unwrap_or(true),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct TrackMetadata {
    title: String,
    artist: String,
    album: String,
    art_url: Option<String>,
    length_us: Option<f64>,
}

fn probe_player(bus_name: &str) -> PlayerProbe {
    // One call for all three; busctl stops at the first property a player
    // lacks, so the optional CanControl goes last.
    let values = read_properties(
        bus_name,
        PLAYER_INTERFACE,
        &["PlaybackStatus", "Metadata", "CanControl"],
    );
    PlayerProbe {
        bus_name: bus_name.to_string(),
        status: values
            .first()
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        track: values.get(1).map(parse_metadata).unwrap_or_default(),
        can_control: values.get(2).and_then(Value::as_bool),
    }
}

/// Running players, listed at most every few seconds: the collector asks
/// every second, players come and go rarely. A player that quit in between
/// only fails its property reads (`--auto-start=no` keeps them from
/// launching it again).
fn list_players() -> Vec<String> {
    static LAST: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, players)) = last.as_ref() {
        if at.elapsed() < PLAYER_LIST_REFRESH {
            return players.clone();
        }
    }
    // `--acquired` hides merely activatable names: querying one of those
    // would launch the player.
    let players = run_output(
        Path::new("busctl"),
        &["--user", "--acquired", "--no-legend", "list"],
        BUSCTL_TIMEOUT,
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| crate::parse_mpris_players(&String::from_utf8_lossy(&output.stdout)))
    .unwrap_or_default();
    *last = Some((Instant::now(), players.clone()));
    players
}

fn preferred_index(players: &[String], preferred: &str) -> Option<usize> {
    let preferred = preferred.to_lowercase();
    players
        .iter()
        .position(|bus| bus.to_lowercase().contains(&preferred))
}

/// Index of the player to show; see [`current_media`].
fn select_player(players: &[(&str, Option<&str>)], preferred: Option<&str>) -> Option<usize> {
    if let Some(preferred) = preferred {
        let preferred = preferred.to_lowercase();
        if let Some(index) = players
            .iter()
            .position(|(bus, _)| bus.to_lowercase().contains(&preferred))
        {
            return Some(index);
        }
    }
    players
        .iter()
        .position(|(_, status)| *status == Some("Playing"))
        .or_else(|| {
            players
                .iter()
                .position(|(_, status)| *status == Some("Paused"))
        })
}

/// `org.mpris.MediaPlayer2.vlc.instance1234` → `Vlc`, for players that do
/// not answer `Identity`.
fn identity_from_bus_name(bus_name: &str) -> String {
    let name = bus_name
        .strip_prefix("org.mpris.MediaPlayer2.")
        .unwrap_or(bus_name)
        .split('.')
        .next()
        .unwrap_or_default();
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => bus_name.to_string(),
    }
}

/// Property values in request order, unwrapped from their D-Bus type tags.
/// Shorter than `properties` when a property is missing or the call failed.
fn read_properties(bus_name: &str, interface: &str, properties: &[&str]) -> Vec<Value> {
    if busctl_supports_json() {
        let mut args = vec![
            "--user",
            "--auto-start=no",
            "--json=short",
            "get-property",
            bus_name,
            MPRIS_PATH,
            interface,
        ];
        args.extend_from_slice(properties);
        // Values printed before a failing property are still valid.
        return run_output(Path::new("busctl"), &args, BUSCTL_TIMEOUT)
            .map(|output| parse_json_properties(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default();
    }
    let mut values = Vec::new();
    for property in properties {
        let Some(value) = run_output(
            Path::new("busctl"),
            &[
                "--user",
                "--auto-start=no",
                "get-property",
                bus_name,
                MPRIS_PATH,
                interface,
                property,
            ],
            BUSCTL_TIMEOUT,
        )
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| text_property_to_json(&String::from_utf8_lossy(&output.stdout))) else {
            break;
        };
        values.push(value);
    }
    values
}

/// `--json` arrived in systemd 240 (2018).
fn busctl_supports_json() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        run_output(Path::new("busctl"), &["--version"], Duration::from_secs(1))
            .ok()
            .and_then(|output| parse_systemd_version(&String::from_utf8_lossy(&output.stdout)))
            .is_some_and(|version| version >= 240)
    })
}

fn parse_systemd_version(text: &str) -> Option<u32> {
    let mut words = text.lines().next()?.split_whitespace();
    (words.next()? == "systemd").then_some(())?;
    words
        .next()?
        .split(|ch: char| !ch.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// busctl prints one `{"type":…,"data":…}` document per property.
fn parse_json_properties(text: &str) -> Vec<Value> {
    serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .map_while(Result::ok)
        .map(|value| variant_data(&value).clone())
        .collect()
}

/// Strips busctl's `{"type": …, "data": …}` wrappers (nested for variants).
fn variant_data(value: &Value) -> &Value {
    let mut value = value;
    while let Some(object) = value.as_object() {
        match (object.get("type"), object.get("data")) {
            (Some(Value::String(_)), Some(data)) if object.len() == 2 => value = data,
            _ => break,
        }
    }
    value
}

fn parse_metadata(data: &Value) -> TrackMetadata {
    let field = |key: &str| data.get(key).map(variant_data);
    let text = |key: &str| {
        field(key)
            .and_then(Value::as_str)
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    };
    let artist = match field("xesam:artist") {
        Some(Value::Array(names)) => names
            .iter()
            .filter_map(|name| variant_data(name).as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(name)) => name.trim().to_string(),
        _ => String::new(),
    };
    TrackMetadata {
        title: text("xesam:title"),
        artist,
        album: text("xesam:album"),
        art_url: Some(text("mpris:artUrl")).filter(|url| !url.is_empty()),
        length_us: field("mpris:length").and_then(Value::as_f64),
    }
}

/// Converts busctl's default output (`s "Playing"`, `b true`, `x 1200`,
/// `a{sv} 3 "xesam:title" s "…" …`) into the values the JSON path yields.
fn text_property_to_json(text: &str) -> Option<Value> {
    let text = text.trim();
    let (signature, rest) = text.split_once(' ')?;
    let rest = rest.trim();
    Some(match signature {
        "s" | "o" => Value::String(crate::parse_busctl_playback_status(rest).unwrap_or_default()),
        "b" => Value::Bool(rest == "true"),
        "x" | "i" | "n" => Value::from(rest.parse::<i64>().ok()?),
        "t" | "u" | "q" | "y" => Value::from(rest.parse::<u64>().ok()?),
        "d" => Value::from(rest.parse::<f64>().ok()?),
        "a{sv}" => {
            let mut object = serde_json::Map::new();
            for key in ["xesam:title", "xesam:album", "mpris:artUrl"] {
                if let Some(value) = crate::metadata_value(text, key) {
                    object.insert(key.into(), Value::String(value));
                }
            }
            if let Some(artist) = crate::metadata_value(text, "xesam:artist") {
                object.insert("xesam:artist".into(), Value::from(vec![artist]));
            }
            if let Some(length) = number_after_key(text, "mpris:length") {
                object.insert("mpris:length".into(), Value::from(length));
            }
            Value::Object(object)
        }
        _ => return None,
    })
}

/// `"mpris:length" x 989161000` → 989161000.
fn number_after_key(text: &str, key: &str) -> Option<i64> {
    let start = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let mut words = text[start..].split_whitespace();
    let _signature = words.next()?;
    words.next()?.parse().ok()
}

/// Cover art bytes for a [`MediaState::art_url`]. Local `file://` covers
/// (what browsers and most players hand out) are read directly; `http(s)`
/// covers are downloaded once with curl (5 s, ≤ 4 MiB, no redirects to other
/// schemes) into `$XDG_CACHE_HOME/linexus/art/` and served from there on
/// later calls. Anything else, or anything larger than 4 MiB, yields None.
pub fn fetch_art_bytes(url: &str) -> Option<Vec<u8>> {
    let url = url.trim();
    if let Some(path) = file_url_path(url) {
        return read_limited(&path);
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let dir = art_cache_dir();
    let path = dir.join(format!("{:016x}", fnv1a(url.as_bytes())));
    if let Some(bytes) = read_limited(&path).filter(|bytes| !bytes.is_empty()) {
        return Some(bytes);
    }
    fs::create_dir_all(&dir).ok()?;
    let mut tmp_name = path.file_name()?.to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--location",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "-m",
            "5",
            "--max-filesize",
            "4M",
            "--user-agent",
            concat!("linexus/", env!("CARGO_PKG_VERSION")),
            "--output",
        ])
        .arg(&tmp)
        .arg("--")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();
    // curl only knows the size up front when the server sends it, so the
    // limit is checked again on the file.
    let bytes = status
        .filter(|status| status.success())
        .and_then(|_| read_limited(&tmp))
        .filter(|bytes| !bytes.is_empty());
    match bytes {
        Some(bytes) => {
            let _ = fs::rename(&tmp, &path);
            prune_art_cache(&dir);
            Some(bytes)
        }
        None => {
            let _ = fs::remove_file(&tmp);
            None
        }
    }
}

/// `file:///home/a/Cover%20Art.jpg` → `/home/a/Cover Art.jpg`; only local
/// absolute paths (`file://localhost/…` included).
pub(crate) fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    Some(PathBuf::from(OsString::from_vec(percent_decode(path))))
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ART_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    (bytes.len() as u64 <= MAX_ART_BYTES).then_some(bytes)
}

fn art_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("linexus")
        .join("art")
}

fn prune_art_cache(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    if files.len() <= ART_CACHE_FILES {
        return;
    }
    files.sort();
    for (_, path) in &files[..files.len() - ART_CACHE_FILES] {
        let _ = fs::remove_file(path);
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_busctl_json_properties() {
        let output = concat!(
            r#"{"type":"s","data":"Playing"}"#,
            "\n",
            r#"{"type":"a{sv}","data":{"mpris:trackid":{"type":"o","data":"/org/mpris/MediaPlayer2/Track/1"},"#,
            r#""xesam:title":{"type":"s","data":" Bohemian Rhapsody "},"#,
            r#""xesam:artist":{"type":"as","data":["Queen","Freddie Mercury"]},"#,
            r#""xesam:album":{"type":"s","data":"A Night at the Opera"},"#,
            r#""mpris:artUrl":{"type":"s","data":"https://i.scdn.co/image/ab67"},"#,
            r#""mpris:length":{"type":"x","data":354000000}}}"#,
            "\n",
            r#"{"type":"b","data":true}"#,
            "\n",
        );
        let values = parse_json_properties(output);
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], "Playing");
        assert_eq!(values[2], true);
        let track = parse_metadata(&values[1]);
        assert_eq!(track.title, "Bohemian Rhapsody");
        assert_eq!(track.artist, "Queen, Freddie Mercury");
        assert_eq!(track.album, "A Night at the Opera");
        assert_eq!(
            track.art_url.as_deref(),
            Some("https://i.scdn.co/image/ab67")
        );
        assert_eq!(track.length_us, Some(354_000_000.0));
    }

    #[test]
    fn keeps_values_printed_before_a_failing_property() {
        let output = "{\"type\":\"s\",\"data\":\"Paused\"}\nFailed to get property CanControl: Unknown property\n";
        assert_eq!(parse_json_properties(output), vec![Value::from("Paused")]);
        assert!(parse_json_properties("").is_empty());
    }

    #[test]
    fn unwraps_nested_variants() {
        let value: Value =
            serde_json::from_str(r#"{"type":"v","data":{"type":"s","data":"x"}}"#).unwrap();
        assert_eq!(variant_data(&value), "x");
        let plain: Value = serde_json::from_str(r#"{"type":"s","data":"x","extra":1}"#).unwrap();
        assert_eq!(variant_data(&plain), &plain);
    }

    #[test]
    fn builds_state_with_seconds_and_clamped_position() {
        let probe = PlayerProbe {
            bus_name: "org.mpris.MediaPlayer2.spotify".into(),
            status: Some("Playing".into()),
            track: TrackMetadata {
                title: "Song".into(),
                length_us: Some(200_000_000.0),
                ..TrackMetadata::default()
            },
            can_control: None,
        };
        let state = probe.into_state("Spotify".into(), Some(250_000_000.0));
        assert_eq!(state.length_s, Some(200.0));
        assert_eq!(state.position_s, Some(200.0));
        assert!(state.playing);
        assert!(state.can_control);
        assert_eq!(state.identity, "Spotify");

        let stopped = PlayerProbe {
            status: Some("Paused".into()),
            can_control: Some(false),
            ..PlayerProbe::default()
        }
        .into_state("X".into(), Some(-1.0));
        assert!(!stopped.playing);
        assert!(!stopped.can_control);
        assert_eq!(stopped.position_s, None);
        assert_eq!(stopped.length_s, None);
    }

    #[test]
    fn selects_preferred_then_playing_then_paused() {
        let players = [
            ("org.mpris.MediaPlayer2.vlc", Some("Paused")),
            ("org.mpris.MediaPlayer2.brave.instance42", Some("Playing")),
            ("org.mpris.MediaPlayer2.spotify", Some("Stopped")),
        ];
        assert_eq!(select_player(&players, Some("Spotify")), Some(2));
        assert_eq!(select_player(&players, Some("mpd")), Some(1));
        assert_eq!(select_player(&players, None), Some(1));
        assert_eq!(select_player(&players[..1], None), Some(0));
        assert_eq!(select_player(&players[2..], None), None);
        assert_eq!(select_player(&[], None), None);
        assert_eq!(
            preferred_index(&["org.mpris.MediaPlayer2.Spotify".into()], "spotify"),
            Some(0)
        );
    }

    #[test]
    fn derives_identity_from_bus_name() {
        assert_eq!(
            identity_from_bus_name("org.mpris.MediaPlayer2.vlc.instance1234"),
            "Vlc"
        );
        assert_eq!(
            identity_from_bus_name("org.mpris.MediaPlayer2.spotify"),
            "Spotify"
        );
    }

    #[test]
    fn parses_systemd_version() {
        assert_eq!(
            parse_systemd_version("systemd 262 (262-1-arch)\n+PAM +AUDIT\n"),
            Some(262)
        );
        assert_eq!(parse_systemd_version("systemd 239\n"), Some(239));
        assert_eq!(parse_systemd_version("busybox\n"), None);
    }

    #[test]
    fn converts_text_output_for_old_busctl() {
        assert_eq!(
            text_property_to_json("s \"Playing\"\n"),
            Some(Value::from("Playing"))
        );
        assert_eq!(text_property_to_json("b false\n"), Some(Value::from(false)));
        assert_eq!(
            text_property_to_json("x 1500000\n"),
            Some(Value::from(1_500_000))
        );
        let metadata = text_property_to_json(
            r#"a{sv} 4 "xesam:title" s "F\303\274r Elise" "xesam:artist" as 1 "Beethoven" "mpris:artUrl" s "file:///tmp/a.png" "mpris:length" x 175000000"#,
        )
        .unwrap();
        let track = parse_metadata(&metadata);
        assert_eq!(track.title, "Für Elise");
        assert_eq!(track.artist, "Beethoven");
        assert_eq!(track.art_url.as_deref(), Some("file:///tmp/a.png"));
        assert_eq!(track.length_us, Some(175_000_000.0));
        assert_eq!(text_property_to_json("garbage"), None);
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(
            file_url_path("file:///home/a/Cover%20Art.jpg"),
            Some(PathBuf::from("/home/a/Cover Art.jpg"))
        );
        assert_eq!(
            file_url_path("file://localhost/tmp/x.png"),
            Some(PathBuf::from("/tmp/x.png"))
        );
        assert_eq!(file_url_path("file://server/share/x.png"), None);
        assert_eq!(file_url_path("https://example.test/x.png"), None);
    }

    #[test]
    fn reads_local_art_and_rejects_other_schemes() {
        let path = std::env::temp_dir().join(format!("nexus art {}.png", std::process::id()));
        fs::write(&path, b"\x89PNG fake").unwrap();
        let url = format!("file://{}", path.to_string_lossy().replace(' ', "%20"));
        assert_eq!(fetch_art_bytes(&url).as_deref(), Some(&b"\x89PNG fake"[..]));
        let _ = fs::remove_file(&path);
        assert_eq!(fetch_art_bytes(&url), None);
        assert_eq!(fetch_art_bytes("data:image/png;base64,AAAA"), None);
        assert_eq!(fetch_art_bytes("ftp://example.test/a.png"), None);
    }
}
