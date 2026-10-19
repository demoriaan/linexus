//! Current weather from Open-Meteo: no account or API key, so a weather
//! widget works out of the box. Their data is CC BY 4.0, so wherever it is
//! shown (Studio, docs) [`WEATHER_ATTRIBUTION`] has to appear too.

use super::http::{encode_query_value, http_request};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Credit line the CC BY 4.0 licence of Open-Meteo's data requires.
pub const WEATHER_ATTRIBUTION: &str = "Weather data by Open-Meteo.com (CC BY 4.0)";

const WEATHER_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherReport {
    pub temp_c: f32,
    /// WMO weather interpretation code (0 clear … 99 thunderstorm with hail).
    pub code: u16,
    pub is_day: bool,
    /// Today's high and low at the location.
    pub high_c: Option<f32>,
    pub low_c: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoPlace {
    pub name: String,
    /// State or region, e.g. "Bayern".
    pub admin1: Option<String>,
    pub country: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
}

/// Current conditions plus today's range at a coordinate. One request per
/// call; the forecast updates every 15 minutes, so callers should not ask
/// more often than that.
pub fn fetch_weather(lat: f64, lon: f64) -> Result<WeatherReport, String> {
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        return Err(format!("{lat}, {lon} is not a location on Earth"));
    }
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}\
         &current=temperature_2m,weather_code,is_day\
         &daily=temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=1"
    );
    let response = http_request("GET", &url, &[], None, WEATHER_TIMEOUT)?;
    // Open-Meteo explains failures as {"error":true,"reason":…}.
    parse_forecast(&String::from_utf8_lossy(&response.body))
}

pub fn parse_forecast(json: &str) -> Result<WeatherReport, String> {
    let value = parse_open_meteo(json)?;
    let current = value
        .get("current")
        .ok_or("the forecast has no current conditions")?;
    let number =
        |object: &serde_json::Value, key: &str| object.get(key).and_then(serde_json::Value::as_f64);
    let temp_c = number(current, "temperature_2m").ok_or("the forecast has no temperature")?;
    let code = number(current, "weather_code")
        .filter(|code| (0.0..=u16::MAX as f64).contains(code))
        .ok_or("the forecast has no weather code")?;
    let is_day = !matches!(number(current, "is_day"), Some(day) if day == 0.0);
    let today = |key: &str| {
        value
            .get("daily")
            .and_then(|daily| daily.get(key))
            .and_then(|values| values.get(0))
            .and_then(serde_json::Value::as_f64)
            .map(|value| value as f32)
    };
    Ok(WeatherReport {
        temp_c: temp_c as f32,
        code: code as u16,
        is_day,
        high_c: today("temperature_2m_max"),
        low_c: today("temperature_2m_min"),
    })
}

/// Places matching a name ("Hamburg", "Springfield"), best match first, for
/// the location picker. `language` localizes the names (`de`, `en`).
pub fn geocode(query: &str, language: &str) -> Result<Vec<GeoPlace>, String> {
    let query = query.trim();
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let language = if !language.is_empty()
        && language.len() <= 8
        && language
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
    {
        language.to_ascii_lowercase()
    } else {
        "en".into()
    };
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language={language}&format=json",
        encode_query_value(query)
    );
    let response = http_request("GET", &url, &[], None, WEATHER_TIMEOUT)?;
    parse_geocoding(&String::from_utf8_lossy(&response.body))
}

pub fn parse_geocoding(json: &str) -> Result<Vec<GeoPlace>, String> {
    let value = parse_open_meteo(json)?;
    // No match means no "results" key at all.
    let Some(results) = value.get("results").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let text = |item: &serde_json::Value, key: &str| {
        item.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
    };
    Ok(results
        .iter()
        .filter_map(|item| {
            Some(GeoPlace {
                name: text(item, "name")?,
                admin1: text(item, "admin1"),
                country: text(item, "country"),
                latitude: item.get("latitude")?.as_f64()?,
                longitude: item.get("longitude")?.as_f64()?,
            })
        })
        .collect())
}

fn parse_open_meteo(json: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| "Open-Meteo sent an unreadable answer".to_string())?;
    if value.get("error").and_then(serde_json::Value::as_bool) == Some(true) {
        let reason = value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("Open-Meteo: {reason}"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forecast() {
        let json = r#"{"latitude":52.52,"longitude":13.419998,"generationtime_ms":0.05,
            "utc_offset_seconds":7200,"timezone":"Europe/Berlin","timezone_abbreviation":"GMT+2",
            "elevation":38.0,
            "current_units":{"time":"iso8601","interval":"seconds","temperature_2m":"°C","weather_code":"wmo code","is_day":""},
            "current":{"time":"2026-10-19T03:00","interval":900,"temperature_2m":9.3,"weather_code":3,"is_day":0},
            "daily_units":{"time":"iso8601","temperature_2m_max":"°C","temperature_2m_min":"°C"},
            "daily":{"time":["2026-10-19"],"temperature_2m_max":[14.1],"temperature_2m_min":[7.0]}}"#;
        let report = parse_forecast(json).unwrap();
        assert_eq!(
            report,
            WeatherReport {
                temp_c: 9.3,
                code: 3,
                is_day: false,
                high_c: Some(14.1),
                low_c: Some(7.0),
            }
        );
    }

    #[test]
    fn forecast_without_daily_values_still_parses() {
        let report =
            parse_forecast(r#"{"current":{"temperature_2m":-2.5,"weather_code":71,"is_day":1}}"#)
                .unwrap();
        assert_eq!(report.temp_c, -2.5);
        assert_eq!(report.code, 71);
        assert!(report.is_day);
        assert_eq!(report.high_c, None);
    }

    #[test]
    fn reports_open_meteo_errors() {
        let error = parse_forecast(
            r#"{"error":true,"reason":"Latitude must be in range of -90 to 90°. Given: 91.0."}"#,
        )
        .unwrap_err();
        assert!(error.contains("Latitude must be in range"));
        assert!(parse_forecast("not json").is_err());
        assert!(parse_forecast(r#"{"current":{}}"#).is_err());
        assert!(fetch_weather(91.0, 0.0).is_err());
        assert!(fetch_weather(f64::NAN, 0.0).is_err());
    }

    #[test]
    fn parses_geocoding_results() {
        let json = r#"{"results":[
            {"id":2950159,"name":"Berlin","latitude":52.52437,"longitude":13.41053,"elevation":74.0,
             "feature_code":"PPLC","country_code":"DE","timezone":"Europe/Berlin",
             "population":3426354,"country":"Deutschland","admin1":"Land Berlin"},
            {"id":5083330,"name":"Berlin","latitude":44.46867,"longitude":-71.18508,
             "country":"Vereinigte Staaten","admin1":"New Hampshire"},
            {"id":1,"name":"Nowhere"}
        ],"generationtime_ms":0.6}"#;
        let places = parse_geocoding(json).unwrap();
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].name, "Berlin");
        assert_eq!(places[0].admin1.as_deref(), Some("Land Berlin"));
        assert_eq!(places[0].country.as_deref(), Some("Deutschland"));
        assert_eq!(places[1].longitude, -71.18508);
        assert!(parse_geocoding(r#"{"generationtime_ms":0.3}"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn short_queries_skip_the_request() {
        assert_eq!(geocode(" a ", "de"), Ok(Vec::new()));
    }
}
