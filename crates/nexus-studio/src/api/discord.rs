//! Discord: store the user's own application, run the one-time
//! authorization (Discord shows a prompt in its window), report what is
//! connected, and list servers and voice channels for the pickers.

use super::parse_json;
use crate::http::{Request, Response};
use nexus_sensors::{DiscordApp, DiscordError};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// How long the user has to answer Discord's prompt.
const AUTHORIZE_WAIT: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub fn route(request: &Request) -> Option<Response> {
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/discord/status") => status(),
        ("POST", "/api/discord/app") => save(&request.body),
        ("DELETE", "/api/discord/app") => save(b"null"),
        ("POST", "/api/discord/authorize") => authorize(),
        // Not a GET: it can renew and store the token.
        ("POST", "/api/discord/guilds") => guilds(),
        _ => return None,
    };
    Some(response)
}

#[derive(Deserialize)]
struct AppRequest {
    client_id: String,
    /// Empty keeps the stored secret, so the page never needs to hold it.
    #[serde(default)]
    client_secret: String,
}

fn unavailable(error: &DiscordError) -> Response {
    let status = match error {
        DiscordError::NotAuthorized | DiscordError::Denied(_) => 401,
        DiscordError::UnknownApplication | DiscordError::NoRedirect => 400,
        _ => 502,
    };
    Response::error(status, error.to_string())
}

/// Whether an app is set up and authorized, and which Discord client
/// answers on this computer.
fn status() -> Response {
    let app = nexus_sensors::load_sensor_config().discord;
    // Without an application, a made-up id still tells the clients apart:
    // the official app refuses it, arRPC greets any id.
    let probe_id = app.as_ref().map_or("1", |app| app.client_id.as_str());
    let client = match nexus_sensors::DiscordConnection::connect(probe_id, PROBE_TIMEOUT) {
        Ok(_) => "ok",
        Err(DiscordError::Unsupported) => "unsupported",
        Err(DiscordError::UnknownApplication) if app.is_none() => "ok",
        Err(DiscordError::UnknownApplication) => "unknown_application",
        Err(_) => "not_running",
    };
    Response::json(
        200,
        &json!({
            "ok": true,
            "client": client,
            "configured": app.is_some(),
            "client_id": app.as_ref().map(|app| app.client_id.clone()),
            "has_secret": app.as_ref().is_some_and(|app| !app.client_secret.is_empty()),
            "user": app.as_ref().and_then(|app| app.token.as_ref()).map(|token| token.user.clone()),
            // Tokens from before Linexus asked for notifications lack them;
            // connecting again adds them.
            "notifications": app
                .as_ref()
                .and_then(|app| app.token.as_ref())
                .is_some_and(|token| token.has_scope(nexus_sensors::DISCORD_NOTIFICATIONS_SCOPE)),
        }),
    )
}

/// `{client_id, client_secret}` stores the application, `null` removes it.
/// A new application id forgets the old authorization.
fn save(body: &[u8]) -> Response {
    let request: Option<AppRequest> = match parse_json(body, "Discord application") {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Some(problem) = nexus_sensors::sensor_config_problem() {
        return Response::error(500, problem);
    }
    let updated = nexus_sensors::update_sensor_config(|config| match request {
        None => {
            config.discord = None;
            Ok("Discord connection removed")
        }
        Some(request) => updated_app(config.discord.clone(), request).map(|app| {
            config.discord = Some(app);
            "Discord application saved"
        }),
    });
    match updated {
        Ok(Ok(message)) => Response::json(200, &json!({ "ok": true, "message": message })),
        Ok(Err(problem)) => Response::error(400, problem),
        Err(error) => Response::error(500, format!("failed to save sensors.json: {error}")),
    }
}

/// The stored application changed as asked: an empty secret keeps the stored
/// one, and a different application or secret forgets the authorization.
fn updated_app(
    stored: Option<DiscordApp>,
    request: AppRequest,
) -> Result<DiscordApp, &'static str> {
    let client_id = request.client_id.trim().to_string();
    if client_id.is_empty()
        || client_id.len() > 20
        || !client_id.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("the application id is the number under General Information in the Discord developer portal");
    }
    let secret = request.client_secret.trim().to_string();
    let same_app = stored
        .as_ref()
        .is_some_and(|app| app.client_id == client_id);
    let client_secret = match (&stored, secret.is_empty()) {
        (Some(app), true) if same_app => app.client_secret.clone(),
        _ => secret,
    };
    if client_secret.is_empty() {
        return Err("enter the application's client secret");
    }
    let token = stored
        .filter(|app| same_app && app.client_secret == client_secret)
        .and_then(|app| app.token);
    Ok(DiscordApp {
        client_id,
        client_secret,
        token,
    })
}

/// Shows Discord's prompt and waits for the user; stores the token.
fn authorize() -> Response {
    let Some(app) = nexus_sensors::load_sensor_config().discord else {
        return Response::error(404, "save the Discord application first");
    };
    let token = match nexus_sensors::discord_authorize(&app, AUTHORIZE_WAIT) {
        Ok(token) => token,
        Err(error) => return unavailable(&error),
    };
    let user = token.user.clone();
    if let Err(error) = nexus_sensors::store_discord_token(&app.client_id, token) {
        return Response::error(500, format!("failed to save sensors.json: {error}"));
    }
    Response::json(
        200,
        &json!({ "ok": true, "user": user, "message": format!("Connected to Discord as {user}") }),
    )
}

/// Servers and their voice channels, for the widget and button pickers.
fn guilds() -> Response {
    let Some(app) = nexus_sensors::load_sensor_config().discord else {
        return Response::error(404, "Discord is not set up; connect it under Connections");
    };
    match nexus_sensors::discord_guilds(&app) {
        Ok(guilds) => Response::json(200, &json!({ "ok": true, "guilds": guilds })),
        Err(error) => unavailable(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, secret: &str, token: Option<&str>) -> DiscordApp {
        DiscordApp {
            client_id: id.into(),
            client_secret: secret.into(),
            token: token.map(|access| nexus_sensors::DiscordToken {
                access_token: access.into(),
                refresh_token: String::new(),
                expires_at: 1,
                user: String::new(),
                scopes: Vec::new(),
            }),
        }
    }

    fn asked(id: &str, secret: &str) -> AppRequest {
        AppRequest {
            client_id: id.into(),
            client_secret: secret.into(),
        }
    }

    #[test]
    fn saving_keeps_the_secret_and_forgets_tokens_of_another_app() {
        let stored = app("42", "secret", Some("token"));
        let same = updated_app(Some(stored.clone()), asked("42", "")).unwrap();
        assert_eq!(same, stored);
        let other = updated_app(Some(stored.clone()), asked("43", "new")).unwrap();
        assert_eq!(other, app("43", "new", None));
        assert!(updated_app(Some(stored.clone()), asked("43", "")).is_err());
        let rotated = updated_app(Some(stored), asked("42", "new")).unwrap();
        assert_eq!(rotated, app("42", "new", None));
        assert!(updated_app(None, asked("abc", "s")).is_err());
    }

    #[test]
    fn errors_map_to_statuses() {
        assert_eq!(unavailable(&DiscordError::NotAuthorized).status, 401);
        assert_eq!(unavailable(&DiscordError::UnknownApplication).status, 400);
        assert_eq!(unavailable(&DiscordError::NotRunning).status, 502);
        assert_eq!(unavailable(&DiscordError::Unsupported).status, 502);
    }
}
