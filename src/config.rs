// SPDX-License-Identifier: GPL-3.0-or-later
//! Step 1: YAML-first configuration loading.
//!
//! Ports `library/config.py` + `library/display.py` helpers:
//! - `config.yaml` -> [`AppConfig`]
//! - `res/themes/<THEME>/theme.yaml` overlaid on `res/themes/default.yaml`
//!   (recursive merge, theme wins) -> [`Theme`]
//! - `DISPLAY_SIZE` -> pixel dimensions, orientation resolution.

use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level `config.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    #[serde(rename = "config")]
    pub general: GeneralConfig,
    pub display: DisplayConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GeneralConfig {
    #[serde(rename = "COM_PORT", default = "default_com_port")]
    pub com_port: String,
    #[serde(rename = "THEME", default = "default_theme")]
    pub theme: String,
    #[serde(rename = "HW_SENSORS", default = "default_hw_sensors")]
    pub hw_sensors: HwSensors,
    #[serde(rename = "ETH", default, deserialize_with = "de_string_lenient_opt")]
    pub eth: String,
    #[serde(rename = "WLO", default, deserialize_with = "de_string_lenient_opt")]
    pub wlo: String,
    #[serde(rename = "CPU_FAN", default = "default_cpu_fan")]
    pub cpu_fan: String,
    #[serde(rename = "PING", default = "default_ping", deserialize_with = "de_string_lenient")]
    pub ping: String,
    #[serde(rename = "WEATHER_API_KEY", default)]
    pub weather_api_key: String,
    #[serde(rename = "WEATHER_LATITUDE", default)]
    pub weather_latitude: f64,
    #[serde(rename = "WEATHER_LONGITUDE", default)]
    pub weather_longitude: f64,
    #[serde(rename = "WEATHER_UNITS", default = "default_weather_units")]
    pub weather_units: String,
    #[serde(rename = "WEATHER_LANGUAGE", default = "default_weather_lang")]
    pub weather_language: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum HwSensors {
    #[default]
    #[serde(rename = "AUTO")]
    Auto,
    #[serde(rename = "PYTHON")]
    Python,
    #[serde(rename = "LHM")]
    Lhm,
    #[serde(rename = "STUB")]
    Stub,
    #[serde(rename = "STATIC")]
    Static,
}

impl HwSensors {
    pub fn as_str(&self) -> &'static str {
        match self {
            HwSensors::Auto => "AUTO",
            HwSensors::Python => "PYTHON",
            HwSensors::Lhm => "LHM",
            HwSensors::Stub => "STUB",
            HwSensors::Static => "STATIC",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            HwSensors::Auto => "Automatic",
            HwSensors::Python => "Python libraries",
            HwSensors::Lhm => "LibreHardwareMonitor (admin.)",
            HwSensors::Stub => "Fake random data",
            HwSensors::Static => "Fake static data",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayConfig {
    #[serde(rename = "REVISION")]
    pub revision: Revision,
    #[serde(rename = "BRIGHTNESS", default = "default_brightness")]
    pub brightness: u8,
    #[serde(
        rename = "DISPLAY_REVERSE",
        default,
        deserialize_with = "de_bool_lenient_default_false"
    )]
    pub display_reverse: bool,
    #[serde(
        rename = "RESET_ON_STARTUP",
        default = "default_true",
        deserialize_with = "de_bool_lenient_default_true"
    )]
    pub reset_on_startup: bool,
}

/// Hardware driver selection. Routing for new revisions must stay in
/// `display` factory (Step 5); do not match on this outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Revision {
    #[serde(rename = "A")]
    A,
    #[serde(rename = "B")]
    B,
    #[serde(rename = "C")]
    C,
    #[serde(rename = "D")]
    D,
    #[serde(rename = "TUR_USB")]
    TurUsb,
    #[serde(rename = "WEACT_A")]
    WeActA,
    #[serde(rename = "WEACT_B")]
    WeActB,
    #[serde(rename = "SIMU")]
    Simu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
    ReversePortrait,
    ReverseLandscape,
}

/// Resolved theme (strongly typed header + raw STATS kept for Step 4).
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub dir: PathBuf,
    pub display_orientation: String,
    pub display_size: Option<String>,
    pub display_rgb_led: [u8; 3],
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
    pub static_images: HashMap<String, StaticImage>,
    pub static_texts: HashMap<String, StaticText>,
    /// Untyped remainder (`STATS`, etc.) — fully typed in Step 4.
    pub raw: serde_yaml::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StaticImage {
    #[serde(rename = "PATH")]
    pub path: String,
    #[serde(rename = "X", default)]
    pub x: i32,
    #[serde(rename = "Y", default)]
    pub y: i32,
    #[serde(rename = "WIDTH", default)]
    pub width: i32,
    #[serde(rename = "HEIGHT", default)]
    pub height: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StaticText {
    #[serde(rename = "TEXT", default)]
    pub text: String,
    #[serde(rename = "X", default)]
    pub x: i32,
    #[serde(rename = "Y", default)]
    pub y: i32,
    #[serde(rename = "WIDTH", default)]
    pub width: i32,
    #[serde(rename = "HEIGHT", default)]
    pub height: i32,
    #[serde(rename = "FONT", default = "default_font")]
    pub font: String,
    #[serde(rename = "FONT_SIZE", default = "default_font_size")]
    pub font_size: u32,
    #[serde(
        rename = "FONT_COLOR",
        default = "default_fg",
        deserialize_with = "de_color_default_black"
    )]
    pub font_color: [u8; 3],
    #[serde(
        rename = "BACKGROUND_COLOR",
        default = "default_bg",
        deserialize_with = "de_color_default_white"
    )]
    pub background_color: [u8; 3],
    #[serde(rename = "BACKGROUND_IMAGE", default)]
    pub background_image: Option<String>,
    #[serde(rename = "ALIGN", default = "default_align")]
    pub align: String,
    #[serde(rename = "ANCHOR", default = "default_anchor")]
    pub anchor: String,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

pub fn load_app_config(path: &Path) -> Result<AppConfig, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let cfg: AppConfig =
        serde_yaml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    if cfg.display.brightness > 100 {
        return Err(format!(
            "BRIGHTNESS must be 0-100, got {}",
            cfg.display.brightness
        ));
    }
    Ok(cfg)
}

/// Load theme dir, overlay `default.yaml` beneath it (theme wins).
/// Mirrors `copy_default(THEME_DEFAULT, THEME_DATA)` in `config.py`.
pub fn load_theme(
    themes_root: &Path,
    default_file: &Path,
    theme_name: &str,
) -> Result<Theme, String> {
    let theme_dir = themes_root.join(theme_name);
    let theme_file = theme_dir.join("theme.yaml");
    let default_text = std::fs::read_to_string(default_file)
        .map_err(|e| format!("cannot read {}: {e}", default_file.display()))?;
    let theme_text = std::fs::read_to_string(&theme_file)
        .map_err(|e| format!("cannot read {}: {e}", theme_file.display()))?;
    let default_val: serde_yaml::Value =
        serde_yaml::from_str(&default_text).map_err(|e| format!("invalid default.yaml: {e}"))?;
    let theme_val: serde_yaml::Value =
        serde_yaml::from_str(&theme_text).map_err(|e| format!("invalid theme.yaml: {e}"))?;
    let merged = merge_yaml(default_val, theme_val);

    let display_orientation = merged
        .get("display")
        .and_then(|d| d.get("DISPLAY_ORIENTATION"))
        .and_then(|v| v.as_str())
        .unwrap_or("portrait")
        .to_string();
    let display_size = merged
        .get("display")
        .and_then(|d| d.get("DISPLAY_SIZE"))
        .and_then(value_to_string)
        .filter(|s| !s.is_empty());
    let display_rgb_led = merged
        .get("display")
        .and_then(|d| d.get("DISPLAY_RGB_LED"))
        .map(value_to_rgb)
        .transpose()?
        .unwrap_or([255, 255, 255]);

    let (w, h) = theme_size_to_dims(display_size.as_deref());

    let static_images = merged
        .get("static_images")
        .map(|v| serde_yaml::from_value::<HashMap<String, StaticImage>>(v.clone()))
        .transpose()
        .map_err(|e| format!("invalid static_images: {e}"))?
        .unwrap_or_default();
    let static_texts = merged
        .get("static_text")
        .map(|v| serde_yaml::from_value::<HashMap<String, StaticText>>(v.clone()))
        .transpose()
        .map_err(|e| format!("invalid static_text: {e}"))?
        .unwrap_or_default();

    Ok(Theme {
        name: theme_name.to_string(),
        dir: theme_dir,
        display_orientation,
        display_size,
        display_rgb_led,
        width: w,
        height: h,
        orientation: Orientation::Portrait, // finalized via resolve_orientation()
        static_images,
        static_texts,
        raw: merged,
    })
}

impl Theme {
    pub fn resolve_orientation(&mut self, reverse: bool) {
        let o = self.display_orientation.to_ascii_lowercase();
        self.orientation = match (o.as_str(), reverse) {
            ("landscape", false) => Orientation::Landscape,
            ("landscape", true) => Orientation::ReverseLandscape,
            ("portrait", true) => Orientation::ReversePortrait,
            _ => Orientation::Portrait, // unknown -> portrait + warn at call site
        };
    }

    /// Width/height after orientation applied.
    pub fn oriented_dims(&self) -> (u32, u32) {
        match self.orientation {
            Orientation::Portrait | Orientation::ReversePortrait => (self.width, self.height),
            Orientation::Landscape | Orientation::ReverseLandscape => (self.height, self.width),
        }
    }
}

/// Port of `_get_theme_size()` in `library/display.py`.
pub fn theme_size_to_dims(size: Option<&str>) -> (u32, u32) {
    match size {
        Some("0.96\"") => (80, 160),
        Some("2.1\"") => (480, 480),
        Some("2.8\"") => (480, 480),
        Some("3.5\"") => (320, 480),
        Some("4.6\"") => (320, 960),
        Some("5\"") => (480, 800),
        Some("5.2\"") => (720, 1280),
        Some("8\"") => (800, 1280),
        Some("8.8\"") => (480, 1920),
        Some("9.2\"") => (480, 1920),
        Some("12.3\"") => (720, 1920),
        _ => (320, 480), // default + warning, same as Python fallback
    }
}

/// Recursive mapping merge: `overlay` wins, nested maps merged.
pub fn merge_yaml(
    base: serde_yaml::Value,
    overlay: serde_yaml::Value,
) -> serde_yaml::Value {
    use serde_yaml::Value as V;
    match (base, overlay) {
        (V::Mapping(mut b), V::Mapping(o)) => {
            for (k, v) in o {
                match b.remove(&k) {
                    Some(old) => {
                        b.insert(k, merge_yaml(old, v));
                    }
                    None => {
                        b.insert(k, v);
                    }
                }
            }
            V::Mapping(b)
        }
        (_, o) => o,
    }
}

// ---------------------------------------------------------------------------
// Lenient deserializers (theme YAML is hand-edited, Python-style)
// ---------------------------------------------------------------------------

fn default_com_port() -> String {
    "AUTO".into()
}
fn default_theme() -> String {
    "3.5inchTheme2".into()
}
fn default_hw_sensors() -> HwSensors {
    HwSensors::Auto
}
fn default_cpu_fan() -> String {
    "AUTO".into()
}
fn default_ping() -> String {
    "8.8.8.8".into()
}
fn default_weather_units() -> String {
    "metric".into()
}
fn default_weather_lang() -> String {
    "en".into()
}
fn default_brightness() -> u8 {
    20
}
fn default_true() -> bool {
    true
}
fn default_font() -> String {
    "roboto-mono/RobotoMono-Regular.ttf".into()
}
fn default_font_size() -> u32 {
    10
}
fn default_fg() -> [u8; 3] {
    [0, 0, 0]
}
fn default_bg() -> [u8; 3] {
    [255, 255, 255]
}
fn default_align() -> String {
    "left".into()
}
fn default_anchor() -> String {
    "lt".into()
}

fn de_string_lenient<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(value_to_string(&v).unwrap_or_default())
}

fn de_string_lenient_opt<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(value_to_string(&v).unwrap_or_default())
}

fn de_bool_lenient_default_false<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(value_to_bool(&v).unwrap_or(false))
}

fn de_bool_lenient_default_true<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(value_to_bool(&v).unwrap_or(true))
}

fn de_color_default_black<'de, D>(d: D) -> Result<[u8; 3], D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_yaml::Value::deserialize(d)?;
    value_to_rgb(&v).map_err(Error::custom)
}

fn de_color_default_white<'de, D>(d: D) -> Result<[u8; 3], D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_yaml::Value::deserialize(d)?;
    value_to_rgb(&v).map_err(Error::custom)
}

fn value_to_string(v: &serde_yaml::Value) -> Option<String> {
    use serde_yaml::Value as V;
    match v {
        V::Null => None,
        V::Bool(b) => Some(b.to_string()),
        V::Number(n) => Some(n.to_string()),
        V::String(s) => Some(s.clone()),
        V::Sequence(seq) if seq.len() == 3 => Some(
            seq.iter()
                .filter_map(|x| x.as_u64().or_else(|| x.as_i64().map(|i| i as u64)))
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    }
}

fn value_to_bool(v: &serde_yaml::Value) -> Option<bool> {
    use serde_yaml::Value as V;
    match v {
        V::Bool(b) => Some(*b),
        V::Number(n) => n.as_u64().map(|x| x != 0),
        V::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Accepts `[r,g,b]` seq, `"r, g, b"`, `"#rrggbb"` / `"#rgb"`.
/// Named PIL colors (`red`, ...) are rejected in Step 1 with a clear error;
/// full table lands with render (Step 4).
pub fn value_to_rgb(v: &serde_yaml::Value) -> Result<[u8; 3], String> {
    use serde_yaml::Value as V;
    match v {
        V::Sequence(seq) if seq.len() == 3 => {
            let mut out = [0u8; 3];
            for (i, x) in seq.iter().enumerate() {
                let n = x
                    .as_u64()
                    .or_else(|| x.as_i64().map(|v| v.max(0) as u64))
                    .ok_or_else(|| format!("RGB sequence item {i} not a number: {x:?}"))?;
                if n > 255 {
                    return Err(format!("RGB value out of range: {n}"));
                }
                out[i] = n as u8;
            }
            Ok(out)
        }
        V::String(s) => parse_rgb_string(s),
        V::Number(n) => parse_rgb_string(&n.to_string()),
        V::Bool(_) | V::Null | V::Mapping(_) | V::Tagged(_) | V::Sequence(_) => {
            Err(format!("unsupported color value: {v:?}"))
        }
    }
}

fn parse_rgb_string(s: &str) -> Result<[u8; 3], String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let (r, g, b) = match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16);
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16);
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16);
                match (r, g, b) {
                    (Ok(r), Ok(g), Ok(b)) => (r, g, b),
                    _ => return Err(format!("invalid hex color: {s}")),
                }
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16);
                let g = u8::from_str_radix(&hex[2..4], 16);
                let b = u8::from_str_radix(&hex[4..6], 16);
                match (r, g, b) {
                    (Ok(r), Ok(g), Ok(b)) => (r, g, b),
                    _ => return Err(format!("invalid hex color: {s}")),
                }
            }
            _ => return Err(format!("invalid hex color: {s}")),
        };
        return Ok([r, g, b]);
    }
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 3 {
        let mut out = [0u8; 3];
        for (i, p) in parts.iter().enumerate() {
            out[i] = p
                .trim()
                .parse::<u8>()
                .map_err(|_| format!("invalid RGB triplet: {s}"))?;
        }
        return Ok(out);
    }
    Err(format!(
        "unsupported color '{s}' in Step 1 (use \"r, g, b\", \"#rrggbb\" or [r,g,b])"
    ))
}
