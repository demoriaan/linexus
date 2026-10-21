//! OBS Studio: test a connection, store it in sensors.json, and list the
//! scenes and audio sources for the button pickers.

use super::parse_json;
use crate::http::{Request, Response};
use nexus_sensors::{ObsError, ObsServer};
use serde::Deserialize;
use serde_json::json;

pub fn route(request: &Request) -> Option<Response> {
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/api/obs/test") => test(&request.body),
        ("GET", "/api/obs/sources") => sources(),
        ("POST", "/api/obs/server") => save(&request.body),
        _ => return None,
    };
    Some(response)
}

#[derive(Deserialize)]
struct ServerRequest {
    host: String,
    port: u16,
    #[serde(default)]
    password: String,
}

fn checked(request: ServerRequest) -> Result<ObsServer, Response> {
    let host = request.host.trim();
    // An IPv6 address may come in brackets, as in a URL.
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .to_string();
    if host.is_empty() || host.contains(['/', ' ', '[', ']']) {
        return Err(Response::error(
            400,
            "host must be a host name or address, e.g. localhost",
        ));
    }
    // Link-local addresses may name their interface: fe80::1%eth0.
    let address = host
        .split_once('%')
        .map_or(host.as_str(), |(address, _)| address);
    if host.contains(':') && address.parse::<std::net::Ipv6Addr>().is_err() {
        return Err(Response::error(
            400,
            "enter the port in its own field, not after the host",
        ));
    }
    if request.port == 0 {
        return Err(Response::error(400, "port must be between 1 and 65535"));
    }
    Ok(ObsServer {
        host,
        port: request.port,
        password: request.password,
    })
}

fn unavailable(error: &ObsError) -> Response {
    let status = if *error == ObsError::Auth { 401 } else { 502 };
    Response::error(status, error.to_string())
}

fn test(body: &[u8]) -> Response {
    let server = match parse_json(body, "OBS server").and_then(checked) {
        Ok(server) => server,
        Err(response) => return response,
    };
    match nexus_sensors::obs_overview(&server) {
        Ok(overview) => Response::json(
            200,
            &json!({
                "ok": true,
                "version": overview.version,
                "scenes": overview.scenes.len(),
                "message": format!("Connected to OBS {}: {} scenes", overview.version, overview.scenes.len()),
            }),
        ),
        Err(error) => unavailable(&error),
    }
}

/// Scenes and audio sources of the saved server.
fn sources() -> Response {
    let Some(server) = nexus_sensors::load_sensor_config().obs_server else {
        return Response::error(404, "OBS is not set up; save a connection first");
    };
    match nexus_sensors::obs_overview(&server) {
        Ok(overview) => Response::json(
            200,
            &json!({
                "ok": true,
                "version": overview.version,
                "current_scene": overview.current_scene,
                "scenes": overview.scenes,
                "audio_inputs": overview.audio_inputs,
            }),
        ),
        Err(error) => unavailable(&error),
    }
}

/// `{host, port, password}` stores the connection, `null` removes it.
fn save(body: &[u8]) -> Response {
    let request: Option<ServerRequest> = match parse_json(body, "OBS server") {
        Ok(request) => request,
        Err(response) => return response,
    };
    let server = match request.map(checked).transpose() {
        Ok(server) => server,
        Err(response) => return response,
    };
    if let Some(problem) = nexus_sensors::sensor_config_problem() {
        return Response::error(500, problem);
    }
    let message = if server.is_some() {
        "OBS connection saved"
    } else {
        "OBS connection removed"
    };
    let mut config = nexus_sensors::load_sensor_config();
    config.obs_server = server;
    match nexus_sensors::save_sensor_config(config) {
        Ok(_) => Response::json(200, &json!({ "ok": true, "message": message })),
        Err(error) => Response::error(500, format!("failed to save sensors.json: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_connection_fields() {
        let ok = checked(ServerRequest {
            host: " localhost ".into(),
            port: 4455,
            password: "secret".into(),
        })
        .unwrap();
        assert_eq!(ok.host, "localhost");
        let v6 = checked(ServerRequest {
            host: "[::1]".into(),
            port: 4455,
            password: String::new(),
        })
        .unwrap();
        assert_eq!(v6.host, "::1");
        let scoped = checked(ServerRequest {
            host: "fe80::1%eth0".into(),
            port: 4455,
            password: String::new(),
        });
        assert_eq!(scoped.unwrap().host, "fe80::1%eth0");
        for (host, port) in [
            ("", 4455),
            ("ws://localhost", 4455),
            ("localhost", 0),
            ("localhost:4455", 4455),
            ("[localhost]:4455", 4455),
        ] {
            assert!(checked(ServerRequest {
                host: host.into(),
                port,
                password: String::new(),
            })
            .is_err());
        }
    }

    #[test]
    fn a_refused_password_is_a_401() {
        assert_eq!(unavailable(&ObsError::Auth).status, 401);
        assert_eq!(
            unavailable(&ObsError::Unreachable("refused".into())).status,
            502
        );
    }
}
