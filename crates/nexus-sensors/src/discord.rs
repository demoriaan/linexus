//! The Discord desktop app's local RPC interface: voice channels, who sits
//! in them and who speaks, joining and leaving, mute and deafen.
//!
//! Discord listens on a local socket (`discord-ipc-N` in the runtime or
//! temp directory). A client says hello with the id of a Discord
//! application, asks the user once to authorize it (AUTHORIZE shows a prompt
//! inside Discord), trades the code for an OAuth token, and from then on
//! signs in with that token (AUTHENTICATE).
//!
//! The token only ever goes to a socket whose other end runs as the same
//! user: the fallback folders such as `/tmp` are shared, and a socket put
//! there by someone else must not receive it.
//!
//! Discord grants this interface only to applications it approved, and to
//! up to 50 testers of an application that is not. Linexus therefore uses an
//! application each user creates for themselves in the Discord developer
//! portal, with the user as its owner.
//!
//! Clients that only show what you play (arRPC in Vesktop and other
//! alternative clients) answer the hello but nothing else; they are told
//! apart by the user they introduce themselves as.
//!
//! [`DiscordConnection`] is a blocking client like the OBS one: send a
//! command and wait for its answer, or wait a little for the next event.

use crate::providers::http::{http_post_private_form, HttpResponse};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What Linexus asks the user to allow: the RPC interface, who they are,
/// reading and changing their voice state, and the notifications Discord
/// shows.
pub const SCOPES: [&str; 5] = [
    "rpc",
    "identify",
    "rpc.voice.read",
    "rpc.voice.write",
    NOTIFICATIONS_SCOPE,
];
/// Needed to receive NOTIFICATION_CREATE. Tokens from before Linexus asked
/// for it lack it; connecting again in Studio adds it.
pub const NOTIFICATIONS_SCOPE: &str = "rpc.notifications.read";
/// The redirect Studio asks users to add to their application. The local
/// flow never opens it, but Discord refuses applications without one.
pub const REDIRECT_URI: &str = "http://localhost";
const TOKEN_URL: &str = "https://discord.com/api/oauth2/token";
/// A token this close to expiring is renewed before signing in.
const RENEW_BEFORE: Duration = Duration::from_secs(24 * 3600);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// Far more than any answer this client asks for (a guild with hundreds of
/// channels is a few hundred KiB).
const MAX_FRAME_BYTES: usize = 4 << 20;
const MAX_QUEUED_EVENTS: usize = 512;
/// The user arRPC introduces itself as.
const ARRPC_USER_ID: &str = "1045800378228281345";

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// RPC error codes this client tells apart.
const ERROR_INVALID_TOKEN: u64 = 4009;
const ERROR_INVALID_CLIENT: u64 = 4007;
const ERROR_OAUTH: u64 = 5000;

/// The application the user created, and the token Discord gave it.
/// Stored in sensors.json, since the secret and the tokens are secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordApp {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
    /// Set once the user authorized the application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<DiscordToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordToken {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refresh_token: String,
    /// Unix time in seconds.
    pub expires_at: u64,
    /// Who authorized it, as Discord shows them; for Studio.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    /// What the user allowed; empty for tokens stored before Linexus kept it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl DiscordToken {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    fn wants_renewal(&self, now: u64) -> bool {
        !self.refresh_token.is_empty() && self.expires_at <= now + RENEW_BEFORE.as_secs()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscordError {
    /// No Discord app answered (not running, or no socket found).
    NotRunning,
    /// Only a client without remote control answered (Vesktop's arRPC).
    Unsupported,
    /// No token yet, or Discord no longer accepts it.
    NotAuthorized,
    /// The user declined the prompt in Discord, or did not answer it.
    Denied(String),
    /// Discord does not know the application id.
    UnknownApplication,
    /// The application lists no redirect under OAuth2, which Discord wants
    /// even though the local flow never opens it.
    NoRedirect,
    /// Discord refused a command.
    Command { code: u64, message: String },
    /// Trading or renewing the token failed.
    OAuth(String),
    /// The connection dropped or Discord sent something unexpected.
    Connection(String),
}

impl fmt::Display for DiscordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("Discord is not running"),
            Self::Unsupported => f.write_str(
                "this Discord client only shares what you play (Vesktop and other clients \
                 using arRPC); voice control needs the official Discord app",
            ),
            Self::NotAuthorized => {
                f.write_str("Discord is not authorized; connect it in Studio under Connections")
            }
            Self::Denied(detail) if detail.is_empty() => {
                f.write_str("the authorization was declined in Discord")
            }
            Self::Denied(detail) => {
                write!(f, "the authorization was declined in Discord ({detail})")
            }
            Self::NoRedirect => write!(
                f,
                "the Discord application needs a redirect: add {REDIRECT_URI} under OAuth2 → \
                 Redirects in the developer portal, save, then connect again"
            ),
            Self::UnknownApplication => f.write_str(
                "Discord does not know this application id; check it in the developer portal",
            ),
            Self::Command { message, .. } if !message.is_empty() => write!(f, "Discord: {message}"),
            Self::Command { code, .. } => write!(f, "Discord refused the command (code {code})"),
            Self::OAuth(detail) => write!(f, "Discord authorization failed: {detail}"),
            Self::Connection(detail) => write!(f, "connection to Discord lost ({detail})"),
        }
    }
}

impl std::error::Error for DiscordError {}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscordEvent {
    pub evt: String,
    pub data: Value,
}

/// Someone in a voice channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceMember {
    pub user_id: String,
    /// Server nickname, else display name, else user name.
    pub name: String,
    pub avatar_url: String,
    /// Muted by themselves or by the server.
    pub muted: bool,
    pub deafened: bool,
    /// How loud you hear them, 0..=200 percent, set by you.
    pub volume: u16,
    /// Muted by you, for you only.
    pub local_muted: bool,
}

impl VoiceMember {
    /// Reads a `voice_states` entry or a VOICE_STATE_* event.
    pub fn from_voice_state(entry: &Value) -> Option<Self> {
        let user = &entry["user"];
        let user_id = user["id"].as_str()?.to_string();
        let text = |value: &Value| {
            value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        };
        let name = text(&entry["nick"])
            .or_else(|| text(&user["global_name"]))
            .or_else(|| text(&user["username"]))
            .unwrap_or_else(|| user_id.clone());
        let state = &entry["voice_state"];
        let flag = |key: &str| state[key] == true;
        Some(Self {
            avatar_url: avatar_url(
                &user_id,
                user["avatar"].as_str(),
                user["discriminator"].as_str(),
            ),
            name,
            muted: flag("self_mute") || flag("mute") || flag("suppress"),
            deafened: flag("self_deaf") || flag("deaf"),
            volume: entry["volume"]
                .as_f64()
                .map_or(100, |v| v.round().clamp(0.0, 200.0) as u16),
            local_muted: entry["mute"] == true,
            user_id,
        })
    }
}

/// The picture Discord shows for a user: their own, or one of the default
/// avatars, picked the way Discord picks it.
pub fn avatar_url(user_id: &str, avatar: Option<&str>, discriminator: Option<&str>) -> String {
    if let Some(hash) = avatar.filter(|hash| {
        !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }) {
        return format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.png?size=64");
    }
    let index = match discriminator.and_then(|d| d.parse::<u64>().ok()) {
        Some(discriminator) if discriminator != 0 => discriminator % 5,
        _ => user_id.parse::<u64>().map_or(0, |id| (id >> 22) % 6),
    };
    format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
}

/// A server and its voice channels, for Studio's pickers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscordGuild {
    pub id: String,
    pub name: String,
    pub voice_channels: Vec<DiscordChannel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscordChannel {
    pub id: String,
    pub name: String,
}

/// Voice (2) and stage (13) channels, in the order Discord lists them.
pub fn voice_channels(answer: &Value) -> Vec<DiscordChannel> {
    let mut channels: Vec<(i64, DiscordChannel)> = answer["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|channel| matches!(channel["type"].as_u64(), Some(2 | 13)))
        .filter_map(|channel| {
            Some((
                channel["position"].as_i64().unwrap_or(0),
                DiscordChannel {
                    id: channel["id"].as_str()?.to_string(),
                    name: channel["name"].as_str().unwrap_or_default().to_string(),
                },
            ))
        })
        .collect();
    channels.sort_by_key(|(position, _)| *position);
    channels.into_iter().map(|(_, channel)| channel).collect()
}

pub fn guild_list(answer: &Value) -> Vec<(String, String)> {
    answer["guilds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|guild| {
            Some((
                guild["id"].as_str()?.to_string(),
                guild["name"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect()
}

pub struct DiscordConnection {
    pipe: Pipe,
    nonce: u64,
    events: VecDeque<DiscordEvent>,
    /// Who signed in, once AUTHENTICATE answered.
    user_id: Option<String>,
}

impl DiscordConnection {
    /// Says hello to the first Discord app that answers. An arRPC server
    /// is skipped, so the official app is found next to Vesktop.
    pub fn connect(client_id: &str, timeout: Duration) -> Result<Self, DiscordError> {
        if client_id.trim().is_empty() || !client_id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DiscordError::UnknownApplication);
        }
        let mut found_arrpc = false;
        let mut last = None;
        for path in socket_paths() {
            let Ok(pipe) = Pipe::open(&path) else {
                continue;
            };
            let mut connection = Self {
                pipe,
                nonce: 0,
                events: VecDeque::new(),
                user_id: None,
            };
            match connection.handshake(client_id, Instant::now() + timeout) {
                Ok(true) => return Ok(connection),
                Ok(false) => found_arrpc = true,
                Err(error @ DiscordError::UnknownApplication) => return Err(error),
                Err(error) => last = Some(error),
            }
        }
        if found_arrpc {
            return Err(DiscordError::Unsupported);
        }
        Err(last.unwrap_or(DiscordError::NotRunning))
    }

    /// True for a Discord app, false for an arRPC server.
    fn handshake(&mut self, client_id: &str, deadline: Instant) -> Result<bool, DiscordError> {
        self.pipe
            .write_frame(OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        loop {
            let message = self.read(deadline)?.ok_or_else(late)?;
            if message["evt"] == "READY" {
                return Ok(message["data"]["user"]["id"] != ARRPC_USER_ID);
            }
        }
    }

    /// Asks the user to authorize the application; Discord shows a prompt
    /// and this waits up to `wait` for the answer. Returns the code to trade.
    pub fn authorize(&mut self, client_id: &str, wait: Duration) -> Result<String, DiscordError> {
        self.authorize_scopes(client_id, &SCOPES, wait)
    }

    fn authorize_scopes(
        &mut self,
        client_id: &str,
        scopes: &[&str],
        wait: Duration,
    ) -> Result<String, DiscordError> {
        let answer = self.command_with(
            "AUTHORIZE",
            json!({ "client_id": client_id, "scopes": scopes }),
            None,
            wait,
        );
        match answer {
            Ok(data) => data["code"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| DiscordError::Connection("no code in the answer".into())),
            Err(DiscordError::Command { code, message }) if code == ERROR_OAUTH => {
                Err(if message.contains("redirect_uri") {
                    DiscordError::NoRedirect
                } else {
                    DiscordError::Denied(message)
                })
            }
            Err(error) => Err(error),
        }
    }

    /// Signs in; returns the user's display name.
    pub fn authenticate(&mut self, access_token: &str) -> Result<String, DiscordError> {
        let data = match self.command("AUTHENTICATE", json!({ "access_token": access_token })) {
            Ok(data) => data,
            Err(DiscordError::Command { code, .. }) if code == ERROR_INVALID_TOKEN => {
                return Err(DiscordError::NotAuthorized)
            }
            Err(error) => return Err(error),
        };
        let user = &data["user"];
        self.user_id = user["id"].as_str().map(str::to_string);
        Ok(user["global_name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .or_else(|| user["username"].as_str())
            .unwrap_or_default()
            .to_string())
    }

    /// The signed-in user's id.
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// Sends a command and waits for its answer's `data`.
    pub fn command(&mut self, cmd: &str, args: Value) -> Result<Value, DiscordError> {
        self.command_with(cmd, args, None, COMMAND_TIMEOUT)
    }

    /// Follows an event, e.g. VOICE_STATE_CREATE for one channel.
    pub fn subscribe(&mut self, evt: &str, args: Value) -> Result<(), DiscordError> {
        self.command_with("SUBSCRIBE", args, Some(evt), COMMAND_TIMEOUT)
            .map(drop)
    }

    pub fn unsubscribe(&mut self, evt: &str, args: Value) -> Result<(), DiscordError> {
        self.command_with("UNSUBSCRIBE", args, Some(evt), COMMAND_TIMEOUT)
            .map(drop)
    }

    fn command_with(
        &mut self,
        cmd: &str,
        args: Value,
        evt: Option<&str>,
        wait: Duration,
    ) -> Result<Value, DiscordError> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let mut message = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        if let Some(evt) = evt {
            message["evt"] = evt.into();
        }
        self.pipe.write_frame(OP_FRAME, &message)?;
        let deadline = Instant::now() + wait;
        loop {
            let message = self.read(deadline)?.ok_or_else(late)?;
            if message["nonce"] == nonce.as_str() {
                if message["evt"] == "ERROR" {
                    return Err(DiscordError::Command {
                        code: message["data"]["code"].as_u64().unwrap_or(0),
                        message: message["data"]["message"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                return Ok(message["data"].clone());
            }
            self.queue(message);
        }
    }

    /// The next event, waiting at most `wait`; None when none came.
    pub fn next_event(&mut self, wait: Duration) -> Result<Option<DiscordEvent>, DiscordError> {
        let deadline = Instant::now() + wait;
        while self.events.is_empty() {
            match self.read(deadline)? {
                Some(message) => self.queue(message),
                None => return Ok(None),
            }
        }
        Ok(self.events.pop_front())
    }

    /// Keeps a dispatched event; the oldest go when too many wait.
    fn queue(&mut self, message: Value) {
        if message["cmd"] != "DISPATCH" {
            return;
        }
        let Some(evt) = message["evt"].as_str() else {
            return;
        };
        if self.events.len() >= MAX_QUEUED_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(DiscordEvent {
            evt: evt.to_string(),
            data: message["data"].clone(),
        });
    }

    /// The next JSON frame, or None when `deadline` passed first. Pings
    /// are answered here; a close ends the connection.
    fn read(&mut self, deadline: Instant) -> Result<Option<Value>, DiscordError> {
        loop {
            let Some((op, payload)) = self.pipe.read_frame(deadline)? else {
                return Ok(None);
            };
            match op {
                OP_FRAME => {
                    if let Ok(value) = serde_json::from_slice::<Value>(&payload) {
                        return Ok(Some(value));
                    }
                }
                OP_PING => {
                    let value = serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null);
                    self.pipe.write_frame(OP_PONG, &value)?;
                }
                OP_CLOSE => {
                    let value = serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null);
                    return Err(closed(&value));
                }
                _ => {}
            }
        }
    }
}

fn closed(payload: &Value) -> DiscordError {
    let code = payload["code"].as_u64().unwrap_or(0);
    let message = payload["message"].as_str().unwrap_or("closed").to_string();
    if code == ERROR_INVALID_CLIENT || message.to_ascii_lowercase().contains("client id") {
        DiscordError::UnknownApplication
    } else {
        DiscordError::Connection(message)
    }
}

fn late() -> DiscordError {
    DiscordError::Connection("Discord did not answer in time".into())
}

/// A connection that signed in.
pub struct SignedIn {
    pub connection: DiscordConnection,
    /// Who signed in, as Discord shows them.
    pub user: String,
    /// The token in use: the stored one, or a renewed one.
    pub token: DiscordToken,
    /// Why a renewed token could not be saved. Discord replaces the refresh
    /// token on renewal, so an unsaved one means asking the user again.
    pub unsaved: Option<String>,
}

/// Connects and signs in with the stored token, renewing it first when it
/// is about to expire, or when Discord refuses it. A renewed token is saved
/// before anything else can go wrong.
pub fn sign_in(app: &DiscordApp, timeout: Duration) -> Result<SignedIn, DiscordError> {
    let Some(stored) = app.token.clone() else {
        return Err(DiscordError::NotAuthorized);
    };
    let mut connection = DiscordConnection::connect(&app.client_id, timeout)?;
    let mut token = stored;
    let mut unsaved = None;
    let mut renewed = false;
    if token.wants_renewal(unix_now()) {
        if let Ok(fresh) = renew(app, &token) {
            unsaved = save_renewed(app, &fresh);
            token = fresh;
            renewed = true;
        }
    }
    let user = match connection.authenticate(&token.access_token) {
        Ok(user) => user,
        Err(DiscordError::NotAuthorized) if !renewed && !token.refresh_token.is_empty() => {
            let fresh = renew(app, &token).map_err(|_| DiscordError::NotAuthorized)?;
            unsaved = save_renewed(app, &fresh);
            token = fresh;
            connection.authenticate(&token.access_token)?
        }
        Err(error) => return Err(error),
    };
    Ok(SignedIn {
        connection,
        user,
        token,
        unsaved,
    })
}

fn save_renewed(app: &DiscordApp, token: &DiscordToken) -> Option<String> {
    crate::store_discord_token(&app.client_id, token.clone())
        .err()
        .map(|error| error.to_string())
}

/// Runs the whole first-time authorization: the prompt in Discord, then
/// the code traded for a token. `wait` is how long the user has to answer.
pub fn authorize(app: &DiscordApp, wait: Duration) -> Result<DiscordToken, DiscordError> {
    if app.client_secret.trim().is_empty() {
        return Err(DiscordError::OAuth("the client secret is missing".into()));
    }
    let mut connection = DiscordConnection::connect(&app.client_id, COMMAND_TIMEOUT)?;
    let code = match connection.authorize(&app.client_id, wait) {
        // Should Discord refuse the notifications to this application,
        // voice control alone still works.
        Err(DiscordError::Denied(detail)) if detail.contains("scope") => {
            let voice: Vec<&str> = SCOPES
                .into_iter()
                .filter(|scope| *scope != NOTIFICATIONS_SCOPE)
                .collect();
            connection.authorize_scopes(&app.client_id, &voice, wait)?
        }
        other => other?,
    };
    let grant = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
    ];
    let mut token = match trade(app, &grant) {
        // Discord picked the application's only redirect for the prompt;
        // some answers want it named again when trading the code.
        Err(DiscordError::OAuth(detail)) if detail.contains("redirect_uri") => {
            let mut grant = grant.to_vec();
            grant.push(("redirect_uri", REDIRECT_URI));
            trade(app, &grant)?
        }
        other => other?,
    };
    token.user = connection.authenticate(&token.access_token)?;
    Ok(token)
}

fn renew(app: &DiscordApp, token: &DiscordToken) -> Result<DiscordToken, DiscordError> {
    let mut fresh = trade(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &token.refresh_token),
        ],
    )?;
    if fresh.refresh_token.is_empty() {
        fresh.refresh_token = token.refresh_token.clone();
    }
    if fresh.scopes.is_empty() {
        fresh.scopes = token.scopes.clone();
    }
    fresh.user = token.user.clone();
    Ok(fresh)
}

fn trade(app: &DiscordApp, grant: &[(&str, &str)]) -> Result<DiscordToken, DiscordError> {
    let mut fields = vec![
        ("client_id", app.client_id.as_str()),
        ("client_secret", app.client_secret.as_str()),
    ];
    fields.extend_from_slice(grant);
    let response =
        http_post_private_form(TOKEN_URL, &fields, HTTP_TIMEOUT).map_err(DiscordError::OAuth)?;
    parse_token(&response, unix_now())
}

fn parse_token(response: &HttpResponse, now: u64) -> Result<DiscordToken, DiscordError> {
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if !response.is_success() {
        let detail = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {}", response.status));
        return Err(DiscordError::OAuth(detail));
    }
    let access_token = body["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| DiscordError::OAuth("no access token in the answer".into()))?;
    Ok(DiscordToken {
        access_token: access_token.to_string(),
        refresh_token: body["refresh_token"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        expires_at: now + body["expires_in"].as_u64().unwrap_or(7 * 24 * 3600),
        user: String::new(),
        scopes: body["scope"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// A picture Discord names may only come from Discord's picture server, and
/// never as SVG: the notification's icon is whatever the event says it is.
pub fn trusted_image_url(url: &str) -> Option<&str> {
    let path = url.strip_prefix("https://cdn.discordapp.com/")?;
    let file = path.split(['?', '#']).next().unwrap_or_default();
    (!file.is_empty() && !file.to_ascii_lowercase().ends_with(".svg")).then_some(url)
}

/// Servers with their voice channels, for Studio's pickers.
pub fn discord_guilds(app: &DiscordApp) -> Result<Vec<DiscordGuild>, DiscordError> {
    let mut connection = sign_in(app, COMMAND_TIMEOUT)?.connection;
    let guilds = guild_list(&connection.command("GET_GUILDS", json!({}))?);
    let mut out = Vec::with_capacity(guilds.len());
    for (id, name) in guilds {
        let voice_channels = match connection.command("GET_CHANNELS", json!({ "guild_id": id })) {
            Ok(answer) => voice_channels(&answer),
            Err(DiscordError::Command { .. }) => Vec::new(),
            Err(error) => return Err(error),
        };
        out.push(DiscordGuild {
            id,
            name,
            voice_channels,
        });
    }
    Ok(out)
}

/// Every place a Discord app may listen, in the order to try them. Paths
/// that do not exist are left out.
pub fn socket_paths() -> Vec<PathBuf> {
    candidate_paths(|name| std::env::var_os(name).map(PathBuf::from))
        .into_iter()
        .filter(|path| path.exists())
        .collect()
}

fn candidate_paths(env: impl Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    // Sandboxed builds put the socket in their own folder below the base.
    const SUBDIRS: [&str; 8] = [
        "",
        "app/com.discordapp.Discord",
        "app/com.discordapp.DiscordCanary",
        "app/com.discordapp.DiscordPTB",
        ".flatpak/com.discordapp.Discord/xdg-run",
        "snap.discord",
        "snap.discord-canary",
        "app/dev.vencord.Vesktop",
    ];
    let mut bases: Vec<PathBuf> = Vec::new();
    for base in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .filter_map(&env)
        .chain([PathBuf::from("/tmp")])
    {
        if !base.as_os_str().is_empty() && !bases.contains(&base) {
            bases.push(base);
        }
    }
    let mut paths = Vec::new();
    for base in &bases {
        for subdir in SUBDIRS {
            for n in 0..10 {
                paths.push(base.join(subdir).join(format!("discord-ipc-{n}")));
            }
        }
    }
    paths
}

/// Who runs the process at the other end of `stream`.
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` and `len` outlive the call and describe a buffer
    // of exactly the size SO_PEERCRED writes.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut credentials).cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid takes nothing and cannot fail.
    unsafe { libc::geteuid() }
}

fn check_peer(peer: u32, own: u32) -> io::Result<()> {
    if peer == own {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the socket belongs to another user",
        ))
    }
}

/// One end of the socket, with the bytes read but not yet framed.
struct Pipe {
    raw: RawPipe,
    buffer: Vec<u8>,
}

impl Pipe {
    fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            raw: RawPipe::open(path)?,
            buffer: Vec::new(),
        })
    }

    fn write_frame(&mut self, op: u32, payload: &Value) -> Result<(), DiscordError> {
        let body = payload.to_string().into_bytes();
        let frame = encode_frame(op, &body);
        self.raw
            .write_all(&frame)
            .and_then(|()| self.raw.flush())
            .map_err(|error| DiscordError::Connection(error.to_string()))
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<Option<(u32, Vec<u8>)>, DiscordError> {
        loop {
            match decode_frame(&mut self.buffer) {
                Ok(Some(frame)) => return Ok(Some(frame)),
                Ok(None) => {}
                Err(error) => return Err(DiscordError::Connection(error)),
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let mut chunk = [0u8; 16 * 1024];
            match self.raw.read_until(&mut chunk, deadline - now) {
                Ok(0) => {
                    return Err(DiscordError::Connection(
                        "Discord closed the connection".into(),
                    ))
                }
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(DiscordError::Connection(error.to_string())),
            }
        }
    }
}

/// Opcode and length (both little-endian 32-bit), then the JSON.
fn encode_frame(op: u32, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Takes one whole frame off the front of `buffer`, if there is one.
fn decode_frame(buffer: &mut Vec<u8>) -> Result<Option<(u32, Vec<u8>)>, String> {
    if buffer.len() < 8 {
        return Ok(None);
    }
    let op = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
    let len = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(format!("frame of {len} bytes is too large"));
    }
    if buffer.len() < 8 + len {
        return Ok(None);
    }
    let payload = buffer[8..8 + len].to_vec();
    buffer.drain(..8 + len);
    Ok(Some((op, payload)))
}

struct RawPipe(UnixStream);

impl RawPipe {
    fn open(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        check_peer(peer_uid(&stream)?, effective_uid())?;
        stream.set_write_timeout(Some(COMMAND_TIMEOUT))?;
        Ok(Self(stream))
    }

    /// Reads what arrives within `wait`.
    fn read_until(&mut self, chunk: &mut [u8], wait: Duration) -> io::Result<usize> {
        self.0
            .set_read_timeout(Some(wait.max(Duration::from_millis(1))))?;
        self.0.read(chunk)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_wait_for_their_whole_body() {
        let frame = encode_frame(OP_FRAME, br#"{"cmd":"DISPATCH"}"#);
        assert_eq!(&frame[..8], &[1, 0, 0, 0, 18, 0, 0, 0]);
        let mut buffer = frame[..10].to_vec();
        assert_eq!(decode_frame(&mut buffer), Ok(None));
        buffer.extend_from_slice(&frame[10..]);
        buffer.extend_from_slice(&encode_frame(OP_PING, b"{}")[..3]);
        let (op, body) = decode_frame(&mut buffer).unwrap().unwrap();
        assert_eq!(op, OP_FRAME);
        assert_eq!(body, br#"{"cmd":"DISPATCH"}"#);
        assert_eq!(buffer.len(), 3);
        let mut huge = encode_frame(OP_FRAME, b"")[..4].to_vec();
        huge.extend_from_slice(&(u32::MAX).to_le_bytes());
        assert!(decode_frame(&mut huge).is_err());
    }

    #[test]
    fn reads_who_sits_in_a_voice_channel() {
        let entry = json!({
            "nick": "",
            "mute": false,
            "volume": 100,
            "voice_state": { "mute": false, "deaf": false, "self_mute": true, "self_deaf": false, "suppress": false },
            "user": { "id": "80351110224678912", "username": "nelly", "global_name": "Nelly", "discriminator": "0", "avatar": "8342729096ea3675442027381ff50dfe" }
        });
        let member = VoiceMember::from_voice_state(&entry).unwrap();
        assert_eq!(member.name, "Nelly");
        assert!(member.muted);
        assert!(!member.deafened);
        assert_eq!((member.volume, member.local_muted), (100, false));
        assert_eq!(
            member.avatar_url,
            "https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png?size=64"
        );
        let nick = json!({ "nick": "Boss", "user": { "id": "1", "username": "x" } });
        assert_eq!(VoiceMember::from_voice_state(&nick).unwrap().name, "Boss");
        assert!(VoiceMember::from_voice_state(&json!({ "user": {} })).is_none());
    }

    #[test]
    fn default_avatars_follow_discords_rule() {
        // New user names: (id >> 22) % 6.
        assert_eq!(
            avatar_url("80351110224678912", None, Some("0")),
            format!(
                "https://cdn.discordapp.com/embed/avatars/{}.png",
                (80351110224678912u64 >> 22) % 6
            )
        );
        // Legacy discriminators: discriminator % 5.
        assert_eq!(
            avatar_url("1", Some(""), Some("1337")),
            "https://cdn.discordapp.com/embed/avatars/2.png"
        );
        // A hash that could leave the path is not used.
        assert!(avatar_url("1", Some("../x"), None).contains("/embed/avatars/"));
    }

    #[test]
    fn lists_voice_and_stage_channels_in_order() {
        let answer = json!({ "channels": [
            { "id": "3", "name": "Stage", "type": 13, "position": 2 },
            { "id": "1", "name": "general", "type": 0, "position": 0 },
            { "id": "2", "name": "Lounge", "type": 2, "position": 1 },
        ]});
        let names: Vec<_> = voice_channels(&answer)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["Lounge", "Stage"]);
    }

    #[test]
    fn token_answers_and_refusals() {
        let ok = HttpResponse {
            status: 200,
            body: br#"{"access_token":"a","refresh_token":"r","expires_in":604800,"scope":"rpc identify rpc.notifications.read"}"#.to_vec(),
        };
        let token = parse_token(&ok, 1000).unwrap();
        assert!(token.has_scope(NOTIFICATIONS_SCOPE));
        assert!(!token.has_scope("rpc.voice.write"));
        assert_eq!(token.expires_at, 1000 + 604800);
        assert!(!token.wants_renewal(1000));
        assert!(token.wants_renewal(1000 + 604800 - 60));
        let refused = HttpResponse {
            status: 400,
            body: br#"{"error":"invalid_grant","error_description":"Invalid \"redirect_uri\" in request."}"#.to_vec(),
        };
        assert_eq!(
            parse_token(&refused, 0),
            Err(DiscordError::OAuth(
                "Invalid \"redirect_uri\" in request.".into()
            ))
        );
    }

    #[test]
    fn only_sockets_of_the_same_user_are_trusted() {
        assert!(check_peer(1000, 1000).is_ok());
        let refused = check_peer(1001, 1000).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), effective_uid());
    }

    #[test]
    fn pictures_only_come_from_discords_server() {
        let good = "https://cdn.discordapp.com/avatars/1/a.png?size=64";
        assert_eq!(trusted_image_url(good), Some(good));
        for bad in [
            "http://cdn.discordapp.com/a.png",
            "https://cdn.discordapp.com.evil.test/a.png",
            "https://evil.test/https://cdn.discordapp.com/a.png",
            "https://cdn.discordapp.com/",
            "https://cdn.discordapp.com/a.SVG",
            "https://cdn.discordapp.com/a.svg?x=1",
        ] {
            assert_eq!(trusted_image_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn looks_in_the_runtime_dir_and_sandbox_folders() {
        let paths = candidate_paths(|name| match name {
            "XDG_RUNTIME_DIR" => Some(PathBuf::from("/run/user/1000")),
            "TMPDIR" => Some(PathBuf::from("/run/user/1000")),
            _ => None,
        });
        assert_eq!(paths[0], PathBuf::from("/run/user/1000/discord-ipc-0"));
        assert!(paths.contains(&PathBuf::from(
            "/run/user/1000/app/com.discordapp.Discord/discord-ipc-0"
        )));
        assert!(paths.contains(&PathBuf::from("/tmp/discord-ipc-9")));
        // The same base is not searched twice.
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.ends_with("discord-ipc-0")
                    && p.parent() == Some(std::path::Path::new("/run/user/1000")))
                .count(),
            1
        );
    }

    /// A fake Discord app on a socket: the hello, AUTHENTICATE, a command
    /// answered after an unrelated event, then a ping.
    #[test]
    fn talks_to_a_discord_app() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("linexus-discord-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("discord-ipc-0");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut pipe = Pipe {
                raw: RawPipe(stream),
                buffer: Vec::new(),
            };
            let far = Instant::now() + Duration::from_secs(5);
            let (op, hello) = pipe.read_frame(far).unwrap().unwrap();
            assert_eq!(op, OP_HANDSHAKE);
            assert_eq!(
                serde_json::from_slice::<Value>(&hello).unwrap()["client_id"],
                "42"
            );
            pipe.write_frame(
                OP_FRAME,
                &json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "user": { "id": "7" } } }),
            )
            .unwrap();
            let (_, auth) = pipe.read_frame(far).unwrap().unwrap();
            let auth: Value = serde_json::from_slice(&auth).unwrap();
            assert_eq!(auth["args"]["access_token"], "tok");
            pipe.write_frame(OP_FRAME, &json!({ "cmd": "AUTHENTICATE", "nonce": auth["nonce"], "data": { "user": { "id": "8", "username": "axel", "global_name": "Axel" } } })).unwrap();
            let (_, get) = pipe.read_frame(far).unwrap().unwrap();
            let get: Value = serde_json::from_slice(&get).unwrap();
            pipe.write_frame(
                OP_FRAME,
                &json!({ "cmd": "DISPATCH", "evt": "SPEAKING_START", "data": { "user_id": "9" } }),
            )
            .unwrap();
            pipe.write_frame(OP_FRAME, &json!({ "cmd": "GET_GUILDS", "nonce": get["nonce"], "data": { "guilds": [{ "id": "5", "name": "Home" }] } })).unwrap();
            let (_, bad) = pipe.read_frame(far).unwrap().unwrap();
            let bad: Value = serde_json::from_slice(&bad).unwrap();
            pipe.write_frame(OP_PING, &json!({ "n": 1 })).unwrap();
            pipe.write_frame(OP_FRAME, &json!({ "cmd": "SELECT_VOICE_CHANNEL", "evt": "ERROR", "nonce": bad["nonce"], "data": { "code": 5003, "message": "already in a channel" } })).unwrap();
            let (op, _) = pipe.read_frame(far).unwrap().unwrap();
            assert_eq!(op, OP_PONG);
        });
        let mut connection = DiscordConnection {
            pipe: Pipe::open(&path).unwrap(),
            nonce: 0,
            events: VecDeque::new(),
            user_id: None,
        };
        assert!(connection
            .handshake("42", Instant::now() + Duration::from_secs(5))
            .unwrap());
        assert_eq!(connection.authenticate("tok").unwrap(), "Axel");
        assert_eq!(connection.user_id(), Some("8"));
        let guilds = connection.command("GET_GUILDS", json!({})).unwrap();
        assert_eq!(guild_list(&guilds), [("5".to_string(), "Home".to_string())]);
        let event = connection
            .next_event(Duration::from_millis(10))
            .unwrap()
            .unwrap();
        assert_eq!(event.evt, "SPEAKING_START");
        assert_eq!(
            connection.command("SELECT_VOICE_CHANNEL", json!({ "channel_id": "1" })),
            Err(DiscordError::Command {
                code: 5003,
                message: "already in a channel".into()
            })
        );
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
