// SPDX-License-Identifier: GPL-3.0-or-later
//! Weather via OpenWeatherMap OneCall 3.0. Ports `stats.Weather`.
//!
//! Blocking `ureq` (no async runtime), one call per snapshot at most — the
//! daemon will move this to a >=300s thread in Step 4/5 like Python's
//! `WeatherStats` (`max(300s, INTERVAL)`).

use serde::Deserialize;

#[derive(Debug, Clone, Default)]
pub struct WeatherConfig {
    pub api_key: String,
    pub latitude: f64,
    pub longitude: f64,
    pub units: String,
    pub language: String,
}

#[derive(Debug, Clone, Default)]
pub struct Weather {
    /// e.g. "17.5°C" (unit suffix matches `WEATHER_UNITS`, like Python).
    pub temp: Option<String>,
    pub felt: Option<String>,
    pub description: Option<String>,
    pub humidity: Option<String>,
    /// Local update stamp, e.g. "@15:33".
    pub update: Option<String>,
}

#[derive(Deserialize)]
struct OneCall {
    current: Current,
}

#[derive(Deserialize)]
struct Current {
    temp: f64,
    feels_like: f64,
    humidity: f64,
    weather: Vec<WeatherDesc>,
}

#[derive(Deserialize)]
struct WeatherDesc {
    description: String,
}

pub fn fetch(cfg: &WeatherConfig) -> Weather {
    if cfg.api_key.is_empty() {
        log::warn!(
            "no OpenWeatherMap API key: set WEATHER_API_KEY in config.yaml or \
             the TSS_WEATHER_API_KEY env var (free key at \
             https://home.openweathermap.org/users/sign_up with OneCall 3.0)"
        );
        return Weather {
            description: Some("No OpenWeatherMap API key".to_string()),
            ..Default::default()
        };
    }
    if cfg.latitude == 0.0 && cfg.longitude == 0.0 {
        log::warn!("weather coordinates are (0, 0): set WEATHER_LATITUDE/LONGITUDE in config.yaml");
    }
    let deg = match cfg.units.as_str() {
        "metric" => "°C",
        "imperial" => "°F",
        "standard" => "°K",
        _ => "°?",
    };
    let url = format!(
        "https://api.openweathermap.org/data/3.0/onecall?lat={}&lon={}\
         &exclude=minutely,hourly,daily,alerts&appid={}&units={}&lang={}",
        cfg.latitude, cfg.longitude, cfg.api_key, cfg.units, cfg.language
    );
    // Blocking minreq (no async runtime, OS-native TLS). Single call per
    // slow tick; the daemon spaces these >=300s apart.
    let resp = match minreq::get(&url).with_timeout(10).send() {
        Ok(r) => r,
        Err(e) => {
            log::error!("OpenWeatherMap request failed: {e}");
            return Weather {
                description: Some("Error fetching OpenWeatherMap API".to_string()),
                ..Default::default()
            };
        }
    };
    if resp.status_code != 200 {
        let msg = resp
            .json::<serde_json::Value>()
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown error".to_string());
        log::error!("OpenWeatherMap API error: {msg}");
        return Weather {
            description: Some(msg),
            ..Default::default()
        };
    }
    let call: OneCall = match resp.json() {
        Ok(c) => c,
        Err(e) => {
            log::error!("OpenWeatherMap response parse failed: {e}");
            return Weather {
                description: Some("Error fetching OpenWeatherMap API".to_string()),
                ..Default::default()
            };
        }
    };
    let now = chrono::Local::now();
    let desc = call
        .current
        .weather
        .first()
        .map(|w| {
            let mut d = w.description.clone();
            if let Some(first) = d.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            d
        })
        .unwrap_or_default();
    Weather {
        temp: Some(format!("{:.1}{deg}", call.current.temp)),
        felt: Some(format!("({:.1}{deg})", call.current.feels_like)),
        description: Some(desc),
        humidity: Some(format!("{:.0}%", call.current.humidity)),
        update: Some(format!("@{}", now.format("%H:%M"))),
    }
}
