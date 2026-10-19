//! Sensor-backed lookups Studio needs: geocoding, weather, installed
//! applications, icon themes and the Home Assistant server.

pub use nexus_sensors::{AppEntry, EntityState, GeoPlace, HomeAssistantServer, WeatherReport};
use std::path::PathBuf;

pub fn geocode(query: &str, lang: &str) -> Result<Vec<GeoPlace>, String> {
    nexus_sensors::geocode(query, lang)
}

pub fn fetch_weather(lat: f64, lon: f64) -> Result<WeatherReport, String> {
    nexus_sensors::fetch_weather(lat, lon)
}

pub fn list_applications() -> Vec<AppEntry> {
    nexus_sensors::list_applications()
}

pub fn resolve_icon(icon: &str, size: u32) -> Option<PathBuf> {
    nexus_sensors::resolve_icon(icon, size)
}

pub fn fetch_states(server: &HomeAssistantServer) -> Result<Vec<EntityState>, String> {
    nexus_sensors::fetch_states(server)
}

/// The server saved in sensors.json.
pub fn home_assistant_server() -> Option<HomeAssistantServer> {
    nexus_sensors::load_sensor_config().home_assistant_server
}

/// Stores (or clears) the server and keeps every other sensors.json
/// field. A file that does not parse is left alone instead of being
/// replaced by defaults.
pub fn save_home_assistant_server(server: Option<HomeAssistantServer>) -> Result<(), String> {
    if let Some(problem) = nexus_sensors::sensor_config_problem() {
        return Err(problem);
    }
    let mut config = nexus_sensors::load_sensor_config();
    config.home_assistant_server = server;
    nexus_sensors::save_sensor_config(config)
        .map(|_| ())
        .map_err(|error| format!("failed to save sensors.json: {error}"))
}
