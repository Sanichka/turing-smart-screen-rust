// SPDX-License-Identifier: GPL-3.0-or-later
//! Lenient theme-widget parsing + value formatting.
//!
//! Theme YAML is hand-edited (mixed case bools, `"r, g, b"` colors, missing
//! keys): every accessor tolerates that and falls back to the same defaults
//! as `lcd_comm.py` / `stats.py`. `parse_*` returns `None` when `SHOW` is
//! false/absent so callers skip hidden widgets.

use serde_yaml::Value;

use super::framebuf::Rgb;
use crate::config::value_to_rgb;

// ---------------------------------------------------------------------------
// Lenient accessors
// ---------------------------------------------------------------------------

pub fn child<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Mapping(m) => m.get(Value::String(key.to_string())),
        _ => None,
    }
}

pub fn path<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for k in keys {
        cur = child(cur, k)?;
    }
    Some(cur)
}

pub fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_u64().map(|x| x != 0),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn as_i32(v: &Value) -> Option<i32> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(|x| x as i32)
            .or_else(|| n.as_u64().map(|x| x as i32))
            .or_else(|| n.as_f64().map(|x| x as i32)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as i32),
        _ => None,
    }
}

pub fn as_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|x| x as f32),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as i32 as f32),
        _ => None,
    }
}

pub fn as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub fn get_bool(v: &Value, key: &str, default: bool) -> bool {
    child(v, key).and_then(as_bool).unwrap_or(default)
}

pub fn get_i32(v: &Value, key: &str, default: i32) -> i32 {
    child(v, key).and_then(as_i32).unwrap_or(default)
}

pub fn get_f32(v: &Value, key: &str, default: f32) -> f32 {
    child(v, key).and_then(as_f32).unwrap_or(default)
}

pub fn get_string(v: &Value, key: &str, default: &str) -> String {
    child(v, key)
        .and_then(as_string)
        .unwrap_or_else(|| default.to_string())
}

pub fn get_rgb(v: &Value, key: &str, default: Rgb) -> Rgb {
    match child(v, key) {
        Some(c) => value_to_rgb(c).unwrap_or_else(|e| {
            log::warn!("invalid color for {key}: {e}; using default");
            default
        }),
        None => default,
    }
}

/// `[x0, y0, x1, y1]` sequence (CUSTOM_BBOX) or `[0,0,0,0]`.
pub fn get_bbox4(v: &Value, key: &str) -> [i32; 4] {
    match child(v, key) {
        Some(Value::Sequence(s)) if s.len() == 4 => {
            let mut out = [0i32; 4];
            for (i, x) in s.iter().take(4).enumerate() {
                out[i] = as_i32(x).unwrap_or(0);
            }
            out
        }
        _ => [0, 0, 0, 0],
    }
}

/// `[dx, dy]` sequence (TEXT_OFFSET) or `[0, 0]`.
pub fn get_offset2(v: &Value, key: &str) -> [i32; 2] {
    match child(v, key) {
        Some(Value::Sequence(s)) if s.len() == 2 => {
            [as_i32(&s[0]).unwrap_or(0), as_i32(&s[1]).unwrap_or(0)]
        }
        _ => [0, 0],
    }
}

/// Theme poll interval in seconds (0 = disabled). Used by Step-5 scheduler.
#[allow(dead_code)]
pub fn interval_secs(v: &Value) -> f32 {
    child(v, "INTERVAL").and_then(as_f32).unwrap_or(0.0)
}

pub fn history_size(v: &Value) -> usize {
    child(v, "LINE_GRAPH")
        .and_then(|g| child(g, "HISTORY_SIZE"))
        .and_then(as_i32)
        .map(|n| n.max(2) as usize)
        .unwrap_or(10)
}

// ---------------------------------------------------------------------------
// Widget background: solid color or crop from the theme background image.
// ---------------------------------------------------------------------------

/// Background source for a widget canvas, at display coordinates.
#[derive(Clone, Copy)]
pub enum Bg<'a> {
    Solid(Rgb),
    /// Crop from a full-screen RGB8 image (`rgb.len() == w*h*3`).
    Crop { rgb: &'a [u8], w: u32, h: u32 },
}

impl Bg<'_> {
    pub fn at(&self, x: i32, y: i32) -> Rgb {
        match self {
            Bg::Solid(c) => *c,
            Bg::Crop { rgb, w, h } => {
                if x >= 0 && y >= 0 && (x as u32) < *w && (y as u32) < *h {
                    let i = (y as usize * *w as usize + x as usize) * 3;
                    [rgb[i], rgb[i + 1], rgb[i + 2]]
                } else {
                    [0, 0, 0]
                }
            }
        }
    }
}

/// Fill an RGB8 canvas (`w*h*3`) from `bg` positioned at display `(ox, oy)`.
pub fn paint_canvas(buf: &mut [u8], w: u32, h: u32, ox: i32, oy: i32, bg: &Bg) {
    match bg {
        Bg::Solid(c) => {
            for px in buf.as_chunks_mut::<3>().0 {
                px.copy_from_slice(c);
            }
        }
        crop => {
            for y in 0..h {
                for x in 0..w {
                    let c = crop.at(ox + x as i32, oy + y as i32);
                    let i = (y as usize * w as usize + x as usize) * 3;
                    buf[i..i + 3].copy_from_slice(&c);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Widget structs (defaults mirror lcd_comm.py signatures)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TextW {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub font: String,
    pub font_size: u32,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bg_image: Option<String>,
    pub align: String,
    pub anchor: String,
    pub show_unit: bool,
    pub format: String,
}

impl TextW {
    pub fn parse(v: &Value) -> Option<Self> {
        if !get_bool(v, "SHOW", false) {
            return None;
        }
        Some(TextW {
            x: get_i32(v, "X", 0),
            y: get_i32(v, "Y", 0),
            w: get_i32(v, "WIDTH", 0),
            h: get_i32(v, "HEIGHT", 0),
            font: get_string(v, "FONT", "roboto-mono/RobotoMono-Regular.ttf"),
            font_size: get_i32(v, "FONT_SIZE", 10).max(1) as u32,
            fg: get_rgb(v, "FONT_COLOR", [0, 0, 0]),
            bg: get_rgb(v, "BACKGROUND_COLOR", [255, 255, 255]),
            bg_image: child(v, "BACKGROUND_IMAGE").and_then(as_string),
            align: get_string(v, "ALIGN", "left").to_ascii_lowercase(),
            anchor: get_string(v, "ANCHOR", "lt").to_ascii_lowercase(),
            show_unit: get_bool(v, "SHOW_UNIT", true),
            format: get_string(v, "FORMAT", "medium"),
        })
    }
}

#[derive(Debug, Clone)]
pub struct BarW {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub min_v: f32,
    pub max_v: f32,
    pub color: Rgb,
    pub outline: bool,
    pub bg: Rgb,
    pub bg_image: Option<String>,
    pub reverse: bool,
}

impl BarW {
    pub fn parse(v: &Value) -> Option<Self> {
        if !get_bool(v, "SHOW", false) {
            return None;
        }
        Some(BarW {
            x: get_i32(v, "X", 0),
            y: get_i32(v, "Y", 0),
            w: get_i32(v, "WIDTH", 0),
            h: get_i32(v, "HEIGHT", 0),
            min_v: get_f32(v, "MIN_VALUE", 0.0),
            max_v: get_f32(v, "MAX_VALUE", 100.0),
            color: get_rgb(v, "BAR_COLOR", [0, 0, 0]),
            outline: get_bool(v, "BAR_OUTLINE", true),
            bg: get_rgb(v, "BACKGROUND_COLOR", [255, 255, 255]),
            bg_image: child(v, "BACKGROUND_IMAGE").and_then(as_string),
            reverse: get_bool(v, "REVERSE_DIRECTION", false),
        })
    }
}

#[derive(Debug, Clone)]
pub struct RadialW {
    pub xc: i32,
    pub yc: i32,
    pub radius: i32,
    pub width: i32,
    pub min_v: f32,
    pub max_v: f32,
    pub a_start: f32,
    pub a_end: f32,
    pub steps: i32,
    pub sep: f32,
    pub clockwise: bool,
    pub show_text: bool,
    pub show_unit: bool,
    pub font: String,
    pub font_size: u32,
    pub fg: Rgb,
    pub color: Rgb,
    pub bg: Rgb,
    pub bg_image: Option<String>,
    pub custom_bbox: [i32; 4],
    pub text_offset: [i32; 2],
    pub bg_bar_color: Rgb,
    pub draw_bg: bool,
    pub decoration: String,
}

impl RadialW {
    pub fn parse(v: &Value) -> Option<Self> {
        if !get_bool(v, "SHOW", false) {
            return None;
        }
        Some(RadialW {
            xc: get_i32(v, "X", 0),
            yc: get_i32(v, "Y", 0),
            radius: get_i32(v, "RADIUS", 1).max(1),
            width: get_i32(v, "WIDTH", 1).max(1),
            min_v: get_f32(v, "MIN_VALUE", 0.0),
            max_v: get_f32(v, "MAX_VALUE", 100.0),
            a_start: get_f32(v, "ANGLE_START", 0.0),
            a_end: get_f32(v, "ANGLE_END", 360.0),
            steps: get_i32(v, "ANGLE_STEPS", 1).max(1),
            sep: get_f32(v, "ANGLE_SEP", 0.0).max(0.0),
            clockwise: get_bool(v, "CLOCKWISE", false),
            show_text: get_bool(v, "SHOW_TEXT", false),
            show_unit: get_bool(v, "SHOW_UNIT", true),
            font: get_string(v, "FONT", "roboto-mono/RobotoMono-Regular.ttf"),
            font_size: get_i32(v, "FONT_SIZE", 10).max(1) as u32,
            fg: get_rgb(v, "FONT_COLOR", [0, 0, 0]),
            color: get_rgb(v, "BAR_COLOR", [0, 0, 0]),
            bg: get_rgb(v, "BACKGROUND_COLOR", [0, 0, 0]),
            bg_image: child(v, "BACKGROUND_IMAGE").and_then(as_string),
            custom_bbox: get_bbox4(v, "CUSTOM_BBOX"),
            text_offset: get_offset2(v, "TEXT_OFFSET"),
            bg_bar_color: get_rgb(v, "BAR_BACKGROUND_COLOR", [0, 0, 0]),
            draw_bg: get_bool(v, "DRAW_BAR_BACKGROUND", false),
            decoration: get_string(v, "BAR_DECORATION", ""),
        })
    }
}

#[derive(Debug, Clone)]
pub struct GraphW {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub min_v: f32,
    pub max_v: f32,
    pub autoscale: bool,
    pub line_color: Rgb,
    pub line_width: u32,
    pub axis: bool,
    pub axis_color: Rgb,
    pub axis_font: String,
    pub axis_font_size: u32,
}

impl GraphW {
    pub fn parse(v: &Value) -> Option<Self> {
        if !get_bool(v, "SHOW", false) {
            return None;
        }
        let line_color = get_rgb(v, "LINE_COLOR", [0, 0, 0]);
        Some(GraphW {
            x: get_i32(v, "X", 0),
            y: get_i32(v, "Y", 0),
            w: get_i32(v, "WIDTH", 1),
            h: get_i32(v, "HEIGHT", 1),
            min_v: get_f32(v, "MIN_VALUE", 0.0),
            max_v: get_f32(v, "MAX_VALUE", 100.0),
            autoscale: get_bool(v, "AUTOSCALE", false),
            line_color,
            line_width: get_i32(v, "LINE_WIDTH", 2).max(1) as u32,
            axis: get_bool(v, "AXIS", true),
            axis_color: get_rgb(v, "AXIS_COLOR", line_color),
            axis_font: get_string(v, "AXIS_FONT", "roboto/Roboto-Black.ttf"),
            axis_font_size: get_i32(v, "AXIS_FONT_SIZE", 10).max(1) as u32,
        })
    }
}

// ---------------------------------------------------------------------------
// Value formatting (ports stats.py display_themed_* helpers)
// ---------------------------------------------------------------------------

/// Right-aligned int in `min_size` cols + optional unit.
pub fn fmt_value(v: i64, min_size: usize, unit: &str, show_unit: bool) -> String {
    let mut s = format!("{v:>min_size$}");
    if show_unit && !unit.is_empty() {
        s.push_str(unit);
    }
    s
}

pub fn fmt_percent(v: f32, show_unit: bool) -> String {
    if v.is_nan() {
        return String::new();
    }
    fmt_value(v as i64, 3, "%", show_unit)
}

pub fn fmt_temp(v: f32, show_unit: bool) -> String {
    if v.is_nan() {
        return String::new();
    }
    fmt_value(v as i64, 3, "°C", show_unit)
}

pub fn fmt_freq_ghz(mhz: f32) -> String {
    if mhz.is_nan() {
        return String::new();
    }
    format!("{:>4.2} GHz", mhz / 1000.0)
}

#[allow(clippy::cast_possible_truncation)]
pub fn fmt_mega(bytes: u64, show_unit: bool) -> String {
    fmt_value(bytes as i64 / 1024 / 1024, 5, " M", show_unit)
}

#[allow(clippy::cast_possible_truncation)]
pub fn fmt_giga(bytes: u64, show_unit: bool) -> String {
    fmt_value(bytes as i64 / 1_000_000_000, 5, " G", show_unit)
}

/// Port of psutil `bytes2human`.
pub fn bytes2human(n: u64) -> String {
    const SYM: &[&str] = &["B", "K", "M", "G", "T", "P"];
    let mut v = n as f64;
    let mut unit = "B";
    for s in SYM {
        unit = s;
        if v < 1024.0 || *s == "P" {
            break;
        }
        v /= 1024.0;
    }
    if unit == "B" {
        format!("{n}{unit}")
    } else {
        format!("{v:.1}{unit}")
    }
}

/// Port of `bytes2human(rate, '%(value).1f %(symbol)s/s')`.
pub fn rate2human(bps: f64) -> String {
    const SYM: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = bps.max(0.0);
    let mut unit = "B";
    for s in SYM {
        unit = s;
        if v < 1024.0 || *s == "T" {
            break;
        }
        v /= 1024.0;
    }
    format!("{v:>10.1}{unit}/s")
}

/// Port of `str(timedelta(seconds=...))`.
pub fn fmt_uptime(secs: u64) -> String {
    let (d, r) = (secs / 86400, secs % 86400);
    let (h, r) = (r / 3600, r % 3600);
    let (m, s) = (r / 60, r % 60);
    if d > 0 {
        format!("{d} days, {h}:{m:02}:{s:02}")
    } else {
        format!("{h}:{m:02}:{s:02}")
    }
}

/// Map babel date/time `FORMAT` names to chrono patterns.
pub fn fmt_date(epoch_secs: i64, format: &str) -> String {
    let dt = chrono::DateTime::from_timestamp(epoch_secs, 0)
        .map(|u| u.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    match format {
        "short" => dt.format("%-m/%-d/%y").to_string(),
        "long" => dt.format("%B %-d, %Y").to_string(),
        "full" => dt.format("%A, %B %-d, %Y").to_string(),
        _ => dt.format("%b %-d, %Y").to_string(), // medium + default
    }
}

pub fn fmt_time(epoch_secs: i64, format: &str) -> String {
    let dt = chrono::DateTime::from_timestamp(epoch_secs, 0)
        .map(|u| u.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    match format {
        "short" => dt.format("%-I:%M %p").to_string(),
        "long" | "full" => dt.format("%-I:%M:%S %p %Z").to_string(),
        _ => dt.format("%-I:%M:%S %p").to_string(), // medium + default
    }
}
