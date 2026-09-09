// SPDX-License-Identifier: GPL-3.0-or-later
//! Native configuration GUI (Rust port of `configure.py`): live STATIC
//! preview plus the Display and System Monitor sections.

use std::path::{Path, PathBuf};

use eframe::egui;
use turing_smart_screen_rust::config::{
    load_app_config, load_theme, AppConfig, HwSensors, Revision, Theme,
};
use turing_smart_screen_rust::render::framebuf::rgb565_to_888;
use turing_smart_screen_rust::render::{ImageCache, Renderer};
use turing_smart_screen_rust::sensors::stub;

// -- Model/size catalog (ports configure.py model/size combos + maps) ------

const MODEL_TURING: &str = "Turing Smart Screen";
const MODEL_USBPC: &str = "UsbPCMonitor";
const MODEL_XUANFANG: &str = "XuanFang rev. B & flagship";
const MODEL_KIPYE: &str = "Kipye Qiye Smart Display";
const MODEL_WEACT: &str = "WeAct Studio Display FS V1";
const MODEL_SIMU: &str = "Simulated screen";

const SIZE_096: &str = "0.96\"";
const SIZE_21_28: &str = "2.1\" / 2.8\"";
const SIZE_28_ROUND: &str = "2.8\" round (V1.X new HW rev.)";
const SIZE_35: &str = "3.5\"";
const SIZE_46: &str = "4.6\"";
const SIZE_5: &str = "5\"";
const SIZE_52: &str = "5.2\"";
const SIZE_8: &str = "8\"";
const SIZE_88: &str = "8.8\"";
const SIZE_88_92_NEW: &str = "8.8\" / 9.2\" (V1.X new HW rev.)";
const SIZE_123: &str = "12.3\"";

fn sizes_for_model(model: &str) -> &'static [&'static str] {
    match model {
        MODEL_TURING => &[
            SIZE_21_28, SIZE_35, SIZE_46, SIZE_5, SIZE_52, SIZE_8, SIZE_88, SIZE_88_92_NEW,
            SIZE_123, SIZE_28_ROUND,
        ],
        MODEL_USBPC => &[SIZE_35, SIZE_5],
        MODEL_XUANFANG | MODEL_KIPYE => &[SIZE_35],
        MODEL_WEACT => &[SIZE_096, SIZE_35],
        _ => &[SIZE_35, SIZE_5],
    }
}

/// (model, size) -> REVISION. `None` = invalid combination.
fn revision_for(model: &str, size: &str) -> Option<Revision> {
    match (model, size) {
        (MODEL_KIPYE, SIZE_35) => Some(Revision::D),
        (MODEL_TURING, SIZE_21_28 | SIZE_5 | SIZE_88) => Some(Revision::C),
        (MODEL_TURING, SIZE_35) => Some(Revision::A),
        (MODEL_TURING, SIZE_46 | SIZE_52 | SIZE_8 | SIZE_88_92_NEW | SIZE_123 | SIZE_28_ROUND) => {
            Some(Revision::TurUsb)
        }
        (MODEL_USBPC, SIZE_35 | SIZE_5) => Some(Revision::A),
        (MODEL_XUANFANG, SIZE_35) => Some(Revision::B),
        (MODEL_WEACT, SIZE_096) => Some(Revision::WeActB),
        (MODEL_WEACT, SIZE_35) => Some(Revision::WeActA),
        (MODEL_SIMU, _) => Some(Revision::Simu),
        _ => None,
    }
}

/// (REVISION, theme DISPLAY_SIZE) -> (model, size option). Ports
/// `revision_and_size_to_model_map` incl. legacy size quirks.
fn model_size_for(rev: Revision, display_size: Option<&str>) -> (&'static str, &'static str) {
    let ds = display_size.unwrap_or("");
    match rev {
        Revision::D => (MODEL_KIPYE, SIZE_35),
        Revision::B => (MODEL_XUANFANG, SIZE_35),
        Revision::WeActA => (MODEL_WEACT, SIZE_35),
        Revision::WeActB => (MODEL_WEACT, SIZE_096),
        Revision::Simu => (MODEL_SIMU, SIZE_35),
        Revision::C => (
            MODEL_TURING,
            match ds {
                "2.1\"" | "2.8\"" => SIZE_21_28,
                "5\"" => SIZE_5,
                "8.8\"" => SIZE_88,
                _ => SIZE_35,
            },
        ),
        Revision::A => match ds {
            // 5" on rev A is always UsbPCMonitor (Turing 5" is rev C).
            "5\"" => (MODEL_USBPC, SIZE_5),
            _ => (MODEL_TURING, SIZE_35),
        },
        Revision::TurUsb => (
            MODEL_TURING,
            match ds {
                "2.1\"" | "2.8\"" => SIZE_28_ROUND,
                "4.6\"" => SIZE_46,
                "5.2\"" => SIZE_52,
                "8\"" => SIZE_8,
                "9.2\"" => SIZE_88_92_NEW,
                "8.8\"" => SIZE_88_92_NEW,
                "12.3\"" => SIZE_123,
                _ => SIZE_5,
            },
        ),
    }
}

/// Does a theme DISPLAY_SIZE string belong to a size option?
/// Ports `get_themes()`: 2.x matches 2.1"+2.8", 8.x matches 8.8"+9.2".
fn size_matches(option: &str, display_size: &str) -> bool {
    match option {
        SIZE_21_28 | SIZE_28_ROUND => matches!(display_size, "2.1\"" | "2.8\""),
        SIZE_88 | SIZE_88_92_NEW => matches!(display_size, "8.8\"" | "9.2\""),
        _ => display_size == option,
    }
}

const CONFIG_PATH: &str = "config.yaml";
const THEMES_ROOT: &str = "res/themes";
const FONTS_DIR: &str = "res/fonts";

fn list_themes(root: &Path) -> Vec<(String, String)> {
    // (folder name, DISPLAY_SIZE string) sorted case-insensitively.
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            let yaml = p.join("theme.yaml");
            if !p.is_dir() || !yaml.is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            // Light single-file parse for the size label (no full merge).
            let size = std::fs::read_to_string(&yaml)
                .ok()
                .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
                .and_then(|v| v.get("display")?.get("DISPLAY_SIZE")?.as_str().map(str::to_string))
                .unwrap_or_else(|| "?".to_string());
            out.push((name, size));
        }
    }
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

/// Render a STATIC-data frame for `theme` (theme-editor parity).
fn render_preview(theme: &Theme) -> Option<(u32, u32, Vec<u8>)> {
    let mut r = Renderer::new(theme.width, theme.height, PathBuf::from(FONTS_DIR));
    let imgs = ImageCache::preload(theme);
    r.draw_static(theme, &imgs);
    let snap = stub::static_snapshot();
    r.draw_snapshot(theme, &imgs, &snap);
    let mut rgb = Vec::with_capacity((theme.width * theme.height * 3) as usize);
    for &v in r.fb.pixels() {
        let [rr, gg, bb] = rgb565_to_888(v);
        rgb.push(rr);
        rgb.push(gg);
        rgb.push(bb);
    }
    Some((theme.width, theme.height, rgb))
}

struct ConfigureApp {
    cfg: AppConfig,
    all_themes: Vec<(String, String)>,
    model: String,
    size: String,
    com_port: String,
    com_ports: Vec<String>,
    hw_sensors: HwSensors,
    eth: String,
    wlo: String,
    net_ifaces: Vec<String>,
    // Weather & ping dialog state (strings: validated on save).
    wx_open: bool,
    ping: String,
    wx_key: String,
    wx_lat: String,
    wx_lon: String,
    wx_units: String,
    wx_lang: String,
    wx_warn: String,
    city_query: String,
    city_results: Vec<(String, f64, f64)>,
    is_admin: bool,
    theme: Theme,
    preview: Option<egui::TextureHandle>,
    preview_size: (f32, f32),
    status: String,
}

impl ConfigureApp {
    /// Themes compatible with the selected size option.
    fn compatible_themes(&self) -> Vec<String> {
        self.all_themes
            .iter()
            .filter(|(_, s)| size_matches(&self.size, s))
            .map(|(t, _)| t.clone())
            .collect()
    }

    fn com_ports() -> Vec<String> {
        match serialport::available_ports() {
            Ok(ports) => ports.into_iter().map(|p| p.port_name).collect(),
            Err(e) => {
                log::warn!("COM port scan failed: {e}");
                Vec::new()
            }
        }
    }

    /// Interface names for ETH/WLO combos (mirrors psutil.net_if_addrs keys).
    fn net_ifaces() -> Vec<String> {
        use sysinfo::Networks;
        let nets = Networks::new_with_refreshed_list();
        let mut names: Vec<String> = nets.keys().cloned().collect();
        names.sort();
        names
    }

    #[cfg(target_os = "windows")]
    fn is_admin() -> bool {
        use windows_sys::Win32::UI::Shell::IsUserAnAdmin;
        // SAFETY: pure query, no side effects.
        unsafe { IsUserAnAdmin() != 0 }
    }

    #[cfg(not(target_os = "windows"))]
    fn is_admin() -> bool {
        true // gate only applies to Windows LHM use
    }

    /// (key, label) for WEATHER_UNITS.
    fn wx_units() -> [(&'static str, &'static str); 3] {
        [("metric", "metric - °C"), ("imperial", "imperial - °F"), ("standard", "standard - °K")]
    }

    /// (key, label) for WEATHER_LANGUAGE (ports configure.py's 48-code map).
    fn wx_langs() -> Vec<(&'static str, &'static str)> {
        vec![
            ("af", "Afrikaans"), ("al", "Albanian"), ("ar", "Arabic"), ("az", "Azerbaijani"),
            ("bg", "Bulgarian"), ("ca", "Catalan"), ("cz", "Czech"), ("da", "Danish"),
            ("de", "German"), ("el", "Greek"), ("en", "English"), ("eu", "Basque"),
            ("fa", "Persian"), ("fi", "Finnish"), ("fr", "French"), ("gl", "Galician"),
            ("he", "Hebrew"), ("hi", "Hindi"), ("hr", "Croatian"), ("hu", "Hungarian"),
            ("id", "Indonesian"), ("it", "Italian"), ("ja", "Japanese"), ("kr", "Korean"),
            ("la", "Latvian"), ("lt", "Lithuanian"), ("mk", "Macedonian"), ("no", "Norwegian"),
            ("nl", "Dutch"), ("pl", "Polish"), ("pt", "Portuguese"), ("pt_br", "Portuguese (Brazil)"),
            ("ro", "Romanian"), ("ru", "Russian"), ("sv", "Swedish"), ("sk", "Slovak"),
            ("sl", "Slovenian"), ("sp", "Spanish"), ("sr", "Serbian"), ("th", "Thai"),
            ("tr", "Turkish"), ("ua", "Ukrainian"), ("ug", "Uyghur"), ("uk", "Ukrainian"),
            ("vi", "Vietnamese"), ("zh_cn", "Chinese (simplified)"), ("zh_tw", "Chinese (traditional)"),
            ("zu", "Zulu"),
        ]
    }

    fn wx_lang_label(key: &str) -> String {
        Self::wx_langs()
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| key.to_string())
    }

    /// City search via OWM Geo API (mirrors the dialog's Search button).
    fn city_search(&mut self) {
        self.wx_warn.clear();
        self.city_results.clear();
        if self.wx_key.trim().is_empty() {
            self.wx_warn = "enter an API key first".to_string();
            return;
        }
        if self.city_query.trim().is_empty() {
            self.wx_warn = "enter a city name first".to_string();
            return;
        }
        let url = format!(
            "http://api.openweathermap.org/geo/1.0/direct?q={}&limit=10&appid={}",
            self.city_query.trim(),
            self.wx_key.trim()
        );
        let resp = match minreq::get(&url).with_timeout(5).send() {
            Ok(r) => r,
            Err(e) => {
                self.wx_warn = format!("error fetching Geo API: {e}");
                return;
            }
        };
        if resp.status_code == 401 {
            self.wx_warn = "invalid API key".to_string();
            return;
        }
        if resp.status_code != 200 {
            self.wx_warn = format!("Geo API error #{}", resp.status_code);
            return;
        }
        let list: Vec<serde_json::Value> = match resp.json() {
            Ok(l) => l,
            Err(e) => {
                self.wx_warn = format!("error parsing Geo API: {e}");
                return;
            }
        };
        if list.is_empty() {
            self.wx_warn = "no given city found".to_string();
            return;
        }
        for c in list {
            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let state = c.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let country = c.get("country").and_then(|v| v.as_str()).unwrap_or("");
            let lat = c.get("lat").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let lon = c.get("lon").and_then(|v| v.as_f64()).unwrap_or(0.0);
            self.city_results.push((
                format!("{name}, {state} {country} ({lat}, {lon})"),
                lat,
                lon,
            ));
        }
    }

    /// Validate + write weather/ping keys. `false` = validation failed.
    fn save_weather(&mut self) -> bool {
        let lat: f64 = match self.wx_lat.trim().parse() {
            Ok(v) => v,
            Err(_) => {
                self.wx_warn = "latitude must be a number".to_string();
                return false;
            }
        };
        let lon: f64 = match self.wx_lon.trim().parse() {
            Ok(v) => v,
            Err(_) => {
                self.wx_warn = "longitude must be a number".to_string();
                return false;
            }
        };
        self.cfg.general.ping = self.ping.trim().to_string();
        self.cfg.general.weather_api_key = self.wx_key.trim().to_string();
        self.cfg.general.weather_latitude = lat;
        self.cfg.general.weather_longitude = lon;
        self.cfg.general.weather_units = self.wx_units.clone();
        self.cfg.general.weather_language = self.wx_lang.clone();
        true
    }

    fn load_sized_theme(&self, name: &str) -> Result<Theme, String> {
        let mut th = load_theme(
            Path::new(THEMES_ROOT),
            &Path::new(THEMES_ROOT).join("default.yaml"),
            name,
        )?;
        th.resolve_orientation(self.cfg.display.display_reverse);
        Ok(th)
    }

    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let cfg = load_app_config(Path::new(CONFIG_PATH))?;
        let all_themes = list_themes(Path::new(THEMES_ROOT));
        if all_themes.is_empty() {
            return Err("no themes found in res/themes".to_string());
        }
        // Theme first: its DISPLAY_SIZE disambiguates the model on load.
        let mut theme_name = cfg.general.theme.clone();
        let mut theme = load_theme(
            Path::new(THEMES_ROOT),
            &Path::new(THEMES_ROOT).join("default.yaml"),
            &theme_name,
        )
        .map_err(|_| {
            format!(
                "configured theme '{theme_name}' not found; pick another in the dialog"
            )
        })?;
        theme.resolve_orientation(cfg.display.display_reverse);
        let (model, size) =
            model_size_for(cfg.display.revision, theme.display_size.as_deref());
        let mut app = ConfigureApp {
            cfg,
            all_themes,
            model: model.to_string(),
            size: size.to_string(),
            com_port: String::new(), // set below from config
            com_ports: Self::com_ports(),
            hw_sensors: HwSensors::Auto, // set below from config
            eth: String::new(),
            wlo: String::new(),
            net_ifaces: Self::net_ifaces(),
            wx_open: false,
            ping: String::new(),
            wx_key: String::new(),
            wx_lat: String::new(),
            wx_lon: String::new(),
            wx_units: "metric".to_string(),
            wx_lang: "en".to_string(),
            wx_warn: String::new(),
            city_query: String::new(),
            city_results: Vec::new(),
            is_admin: true,
            theme,
            preview: None,
            preview_size: (0.0, 0.0),
            status: String::new(),
        };
        app.com_port = app.cfg.general.com_port.clone();
        app.hw_sensors = app.cfg.general.hw_sensors;
        app.eth = app.cfg.general.eth.clone();
        app.wlo = app.cfg.general.wlo.clone();
        app.ping = app.cfg.general.ping.clone();
        app.wx_key = app.cfg.general.weather_api_key.clone();
        app.wx_lat = app.cfg.general.weather_latitude.to_string();
        app.wx_lon = app.cfg.general.weather_longitude.to_string();
        app.wx_units = app.cfg.general.weather_units.clone();
        app.wx_lang = app.cfg.general.weather_language.clone();
        app.is_admin = Self::is_admin();
        // Python parity: reset to first size-compatible theme if needed.
        if !app.compatible_themes().contains(&theme_name) {
            if let Some(first) = app.compatible_themes().into_iter().next() {
                theme_name = first.clone();
                app.cfg.general.theme = first;
                app.theme = app.load_sized_theme(&theme_name)?;
            }
        }
        app.refresh_preview(&cc.egui_ctx);
        Ok(app)
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        match render_preview(&self.theme) {
            Some((w, h, rgb)) => {
                // Fit into a ~300x560 box, keep aspect.
                let s = (300.0 / w as f32).min(560.0 / h as f32).min(1.0);
                self.preview_size = (w as f32 * s, h as f32 * s);
                let img = egui::ColorImage::from_rgb([w as usize, h as usize], &rgb);
                self.preview = Some(ctx.load_texture(
                    format!("preview:{}", self.theme.name),
                    img,
                    egui::TextureOptions::LINEAR,
                ));
                self.status.clear();
            }
            None => self.status = "preview render failed".to_string(),
        }
    }

    /// Save display + theme settings to config.yaml. Other keys round-trip
    /// untouched (formatting/comments are normalized — serde round-trip).
    fn save_all(&mut self) {
        let text = match std::fs::read_to_string(CONFIG_PATH) {
            Ok(t) => t,
            Err(e) => {
                self.status = format!("cannot read {CONFIG_PATH}: {e}");
                return;
            }
        };
        let mut v: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("invalid {CONFIG_PATH}: {e}");
                return;
            }
        };
        if let Some(config) = v.get_mut("config") {
            config["THEME"] = serde_yaml::Value::String(self.cfg.general.theme.clone());
            config["COM_PORT"] = serde_yaml::Value::String(self.com_port.clone());
            config["HW_SENSORS"] =
                serde_yaml::Value::String(self.hw_sensors.as_str().to_string());
            config["ETH"] = serde_yaml::Value::String(self.eth.clone());
            config["WLO"] = serde_yaml::Value::String(self.wlo.clone());
            // Ping/weather staged by the dialog's Save (validated there).
            config["PING"] = serde_yaml::Value::String(self.cfg.general.ping.clone());
            config["WEATHER_API_KEY"] =
                serde_yaml::Value::String(self.cfg.general.weather_api_key.clone());
            config["WEATHER_LATITUDE"] =
                serde_yaml::Value::Number(serde_yaml::Number::from(self.cfg.general.weather_latitude));
            config["WEATHER_LONGITUDE"] =
                serde_yaml::Value::Number(serde_yaml::Number::from(self.cfg.general.weather_longitude));
            config["WEATHER_UNITS"] =
                serde_yaml::Value::String(self.cfg.general.weather_units.clone());
            config["WEATHER_LANGUAGE"] =
                serde_yaml::Value::String(self.cfg.general.weather_language.clone());
        }
        if let Some(display) = v.get_mut("display") {
            display["REVISION"] =
                serde_yaml::Value::String(Self::revision_str(self.cfg.display.revision).to_string());
            display["BRIGHTNESS"] =
                serde_yaml::Value::Number(self.cfg.display.brightness.into());
            display["DISPLAY_REVERSE"] =
                serde_yaml::Value::Bool(self.cfg.display.display_reverse);
        }
        match std::fs::write(CONFIG_PATH, serde_yaml::to_string(&v).unwrap_or_default()) {
            Ok(()) => {
                self.status =
                    "settings saved — press Save and run to restart the monitor".to_string()
            }
            Err(e) => self.status = format!("cannot write {CONFIG_PATH}: {e}"),
        }
    }

    /// After model/size edits: store REVISION, keep a size-compatible theme.
    fn sync_revision_theme(&mut self, ctx: &egui::Context) {
        match revision_for(&self.model, &self.size) {
            Some(rev) => self.cfg.display.revision = rev,
            None => {
                self.status = "invalid model/size combination".to_string();
                return;
            }
        }
        if !self.compatible_themes().contains(&self.cfg.general.theme) {
            match self.compatible_themes().into_iter().next() {
                Some(first) => {
                    self.cfg.general.theme = first.clone();
                    match self.load_sized_theme(&first) {
                        Ok(th) => {
                            self.theme = th;
                            self.status = format!(
                                "theme reset to '{first}' (matches {})",
                                self.size
                            );
                        }
                        Err(e) => self.status = format!("cannot load theme: {e}"),
                    }
                }
                None => {
                    self.status =
                        format!("no themes found for size {}", self.size);
                    return;
                }
            }
        }
        self.refresh_preview(ctx);
    }

fn revision_str(rev: Revision) -> &'static str {
    match rev {
        Revision::A => "A",
        Revision::B => "B",
        Revision::C => "C",
        Revision::D => "D",
        Revision::TurUsb => "TUR_USB",
        Revision::WeActA => "WEACT_A",
        Revision::WeActB => "WEACT_B",
        Revision::Simu => "SIMU",
    }
}

    /// Spawn the theme editor for the current theme (falls back to
    /// opening theme.yaml, mirroring configure.py's Edit theme button).
    fn edit_theme(&mut self) {
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| {
                d.join(if cfg!(windows) {
                    "turing-theme-editor.exe"
                } else {
                    "turing-theme-editor"
                })
            }))
            .filter(|p| p.is_file());
        if let Some(exe) = sibling {
            match std::process::Command::new(&exe)
                .arg(&self.cfg.general.theme)
                .spawn()
            {
                Ok(_) => {
                    self.status = format!("opened theme editor for '{}'", self.cfg.general.theme);
                    return;
                }
                Err(e) => self.status = format!("cannot start {}: {e}", exe.display()),
            }
            return;
        }
        // No theme-editor binary yet: open the YAML directly.
        open_path(&Path::new(THEMES_ROOT).join(&self.cfg.general.theme).join("theme.yaml"));
        self.status = "theme editor not built yet — opened theme.yaml instead".to_string();
    }

    /// Weather & ping dialog (ports MoreConfigWindow).
    fn weather_dialog(&mut self, ctx: &egui::Context) {
        if !self.wx_open {
            return;
        }
        let mut save_close = false;
        let mut wx_open = true;
        egui::Window::new("Weather & ping")
            .open(&mut wx_open)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Hostname/IP to ping");
                    ui.text_edit_singleline(&mut self.ping);
                });
                ui.separator();
                ui.strong("Weather forecast (OpenWeatherMap API)");
                ui.horizontal(|ui| {
                    ui.label("API key");
                    ui.text_edit_singleline(&mut self.wx_key);
                });
                ui.horizontal(|ui| {
                    ui.label("Latitude");
                    ui.text_edit_singleline(&mut self.wx_lat);
                    ui.label("Longitude");
                    ui.text_edit_singleline(&mut self.wx_lon);
                });
                ui.horizontal(|ui| {
                    ui.label("Units");
                    egui::ComboBox::from_id_salt("wxunits")
                        .selected_text(
                            Self::wx_units()
                                .into_iter()
                                .find(|(k, _)| *k == self.wx_units)
                                .map(|(_, l)| l)
                                .unwrap_or(&self.wx_units),
                        )
                        .show_ui(ui, |ui| {
                            for (k, l) in Self::wx_units() {
                                ui.selectable_value(&mut self.wx_units, k.to_string(), l);
                            }
                        });
                    ui.label("Language");
                    egui::ComboBox::from_id_salt("wxlang")
                        .selected_text(Self::wx_lang_label(&self.wx_lang))
                        .show_ui(ui, |ui| {
                            for (k, l) in Self::wx_langs() {
                                ui.selectable_value(&mut self.wx_lang, k.to_string(), l);
                            }
                        });
                });
                ui.separator();
                ui.strong("Location search");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.city_query);
                    if ui.button("Search").clicked() {
                        self.city_search();
                    }
                });
                if !self.city_results.is_empty() {
                    let mut picked: Option<(f64, f64)> = None;
                    egui::ComboBox::from_id_salt("city")
                        .selected_text("pick a result…")
                        .show_ui(ui, |ui| {
                            for (label, lat, lon) in &self.city_results {
                                if ui.button(label).clicked() {
                                    picked = Some((*lat, *lon));
                                }
                            }
                        });
                    if let Some((lat, lon)) = picked {
                        self.wx_lat = lat.to_string();
                        self.wx_lon = lon.to_string();
                    }
                }
                if !self.wx_warn.is_empty() {
                    ui.colored_label(egui::Color32::RED, &self.wx_warn);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() && self.save_weather() {
                        save_close = true;
                    }
                });
            });
        if save_close {
            self.wx_open = false;
            self.status = "weather & ping settings staged — press Save settings".to_string();
        } else {
            self.wx_open = wx_open;
        }
    }
}

fn theme_size_of(themes: &[(String, String)], name: &str) -> String {
    themes
        .iter()
        .find(|(t, _)| t == name)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| "?".to_string())
}

/// Open a file/folder with the OS default handler.
fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = r {
        log::warn!("cannot open {}: {e}", path.display());
    }
}

impl eframe::App for ConfigureApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.columns(2, |cols| {
                cols[0].heading("Preview (static data)");
                if let Some(tex) = &self.preview {
                    cols[0].add(
                        egui::Image::from_texture(tex)
                            .fit_to_exact_size(egui::vec2(self.preview_size.0, self.preview_size.1)),
                    );
                } else {
                    cols[0].label("no preview");
                }
                cols[0].small(format!("theme: {}", self.theme.name));

                cols[1].heading("Turing System Monitor configuration");
                cols[1].separator();

                // -- Display configuration --------------------------------
                cols[1].strong("Display configuration");
                let mut hw_changed = false;
                cols[1].horizontal(|ui| {
                    ui.label("Smart screen model");
                    egui::ComboBox::from_id_salt("model")
                        .selected_text(&self.model)
                        .show_ui(ui, |ui| {
                            for m in [
                                MODEL_TURING,
                                MODEL_USBPC,
                                MODEL_XUANFANG,
                                MODEL_KIPYE,
                                MODEL_WEACT,
                                MODEL_SIMU,
                            ] {
                                if ui
                                    .selectable_value(&mut self.model, m.to_string(), m)
                                    .changed()
                                {
                                    // Clamp size into the new model's catalog.
                                    let sizes = sizes_for_model(&self.model);
                                    if !sizes.contains(&self.size.as_str()) {
                                        self.size = sizes[0].to_string();
                                    }
                                    hw_changed = true;
                                }
                            }
                        });
                });
                cols[1].horizontal(|ui| {
                    ui.label("Smart screen size");
                    egui::ComboBox::from_id_salt("size")
                        .selected_text(&self.size)
                        .show_ui(ui, |ui| {
                            for s in sizes_for_model(&self.model) {
                                if ui
                                    .selectable_value(&mut self.size, s.to_string(), *s)
                                    .changed()
                                {
                                    hw_changed = true;
                                }
                            }
                        });
                });
                let simu = self.model == MODEL_SIMU;
                cols[1].horizontal(|ui| {
                    ui.label("COM port");
                    ui.add_enabled_ui(!simu, |ui| {
                        let label = if self.com_port == "AUTO" {
                            "Automatic detection".to_string()
                        } else {
                            self.com_port.clone()
                        };
                        egui::ComboBox::from_id_salt("com")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut self.com_port,
                                        "AUTO".to_string(),
                                        "Automatic detection",
                                    )
                                    .changed()
                                {
                                    hw_changed = true;
                                }
                                for p in &self.com_ports {
                                    if ui
                                        .selectable_value(&mut self.com_port, p.clone(), p)
                                        .changed()
                                    {
                                        hw_changed = true;
                                    }
                                }
                            });
                    });
                });
                cols[1].horizontal(|ui| {
                    ui.label("Orientation");
                    ui.add_enabled_ui(!simu, |ui| {
                        let label = if self.cfg.display.display_reverse {
                            "reverse"
                        } else {
                            "classic"
                        };
                        egui::ComboBox::from_id_salt("orient")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut self.cfg.display.display_reverse,
                                        false,
                                        "classic",
                                    )
                                    .changed()
                                {
                                    hw_changed = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.cfg.display.display_reverse,
                                        true,
                                        "reverse",
                                    )
                                    .changed()
                                {
                                    hw_changed = true;
                                }
                            });
                    });
                });
                cols[1].horizontal(|ui| {
                    ui.label("Brightness");
                    ui.add_enabled_ui(!simu, |ui| {
                        ui.add(
                            egui::Slider::new(&mut self.cfg.display.brightness, 0..=100)
                                .suffix("%"),
                        );
                    });
                });
                if self.cfg.display.brightness > 50
                    && self.model == MODEL_TURING
                    && self.size == SIZE_35
                {
                    cols[1].colored_label(
                        egui::Color32::ORANGE,
                        "warning: revision A display can get hot at high brightness",
                    );
                }
                if hw_changed {
                    // Re-resolve orientation (reverse flag may have flipped),
                    // then sync REVISION + theme compatibility + preview.
                    self.theme
                        .resolve_orientation(self.cfg.display.display_reverse);
                    self.cfg.general.com_port = self.com_port.clone();
                    let ctx = cols[1].ctx().clone();
                    self.sync_revision_theme(&ctx);
                }

                // -- System Monitor Configuration --------------------------
                cols[1].separator();
                cols[1].strong("System Monitor Configuration");
                cols[1].label("Theme");
                let compatible = self.compatible_themes();
                let mut changed = false;
                egui::ComboBox::from_id_salt("theme")
                    .selected_text(format!(
                        "{} ({})",
                        self.cfg.general.theme,
                        theme_size_of(&self.all_themes, &self.cfg.general.theme)
                    ))
                    .show_ui(&mut cols[1], |ui| {
                        for t in &compatible {
                            if ui
                                .selectable_value(
                                    &mut self.cfg.general.theme,
                                    t.clone(),
                                    format!(
                                        "{t} ({})",
                                        theme_size_of(&self.all_themes, t)
                                    ),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
                if compatible.is_empty() {
                    cols[1].colored_label(
                        egui::Color32::YELLOW,
                        format!("no themes for size {}", self.size),
                    );
                }
                if changed {
                    let old_size = self.theme.display_size.clone();
                    match self.load_sized_theme(&self.cfg.general.theme.clone()) {
                        Ok(th) => {
                            if th.display_size != old_size {
                                self.status = format!(
                                    "note: '{}' is a {:?} theme (was {:?}) — make sure it matches your panel size",
                                    th.name,
                                    th.display_size.as_deref().unwrap_or("?"),
                                    old_size.as_deref().unwrap_or("?"),
                                );
                            }
                            self.theme = th;
                            let ctx = cols[1].ctx().clone();
                            self.refresh_preview(&ctx);
                        }
                        Err(e) => self.status = format!("cannot load theme: {e}"),
                    }
                }
                // -- Hardware monitoring + network interfaces --------------
                cols[1].horizontal(|ui| {
                    ui.label("Hardware monitoring");
                    egui::ComboBox::from_id_salt("hw")
                        .selected_text(self.hw_sensors.label())
                        .show_ui(ui, |ui| {
                            let mut opts = vec![
                                HwSensors::Auto,
                                HwSensors::Python,
                                HwSensors::Stub,
                                HwSensors::Static,
                            ];
                            // LHM is Windows-only (mirrors configure.py).
                            if cfg!(target_os = "windows") {
                                opts.insert(1, HwSensors::Lhm);
                            }
                            for o in opts {
                                ui.selectable_value(&mut self.hw_sensors, o, o.label());
                            }
                        });
                });
                // Fake sensors need no interfaces (mirrors configure.py).
                let nets_on = !matches!(self.hw_sensors, HwSensors::Stub | HwSensors::Static);
                cols[1].horizontal(|ui| {
                    ui.label("Ethernet interface");
                    ui.add_enabled_ui(nets_on, |ui| {
                        let label =
                            if self.eth.is_empty() { "None".to_string() } else { self.eth.clone() };
                        egui::ComboBox::from_id_salt("eth")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.eth, String::new(), "None");
                                for n in &self.net_ifaces {
                                    ui.selectable_value(&mut self.eth, n.clone(), n);
                                }
                            });
                    });
                });
                cols[1].horizontal(|ui| {
                    ui.label("Wi-Fi interface");
                    ui.add_enabled_ui(nets_on, |ui| {
                        let label =
                            if self.wlo.is_empty() { "None".to_string() } else { self.wlo.clone() };
                        egui::ComboBox::from_id_salt("wlo")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.wlo, String::new(), "None");
                                for n in &self.net_ifaces {
                                    ui.selectable_value(&mut self.wlo, n.clone(), n);
                                }
                            });
                    });
                });
                // Admin gate (mirrors configure.py): LHM needs elevation.
                let needs_admin = matches!(self.hw_sensors, HwSensors::Auto | HwSensors::Lhm)
                    && cfg!(target_os = "windows")
                    && !self.is_admin;
                if needs_admin {
                    cols[1].colored_label(
                        egui::Color32::RED,
                        "Restart as admin, or select another Hardware monitoring",
                    );
                }
                cols[1].separator();
                cols[1].horizontal_wrapped(|ui| {
                    if ui.button("Weather & ping").clicked() {
                        self.wx_open = true;
                        self.wx_warn.clear();
                    }
                    if ui.button("Open themes folder").clicked() {
                        open_path(Path::new(THEMES_ROOT));
                    }
                    if ui.button("Edit theme").clicked() {
                        self.edit_theme();
                    }
                    if ui.button("Save settings").clicked() {
                        self.save_all();
                    }
                    ui.add_enabled_ui(!needs_admin, |ui| {
                        if ui.button("Save and run").clicked() {
                            self.save_all();
                            // Prefer the daemon exe next to this tool; fall back
                            // to PATH lookup (matches tray-launch layouts).
                            let daemon = std::env::current_exe()
                                .ok()
                                .and_then(|p| p.parent().map(|d| {
                                    d.join(if cfg!(windows) {
                                        "turing-smart-screen.exe"
                                    } else {
                                        "turing-smart-screen"
                                    })
                                }))
                                .filter(|p| p.is_file())
                                .unwrap_or_else(|| PathBuf::from("turing-smart-screen"));
                            match std::process::Command::new(&daemon).arg("--daemon").spawn() {
                                Ok(_) => std::process::exit(0),
                                Err(e) => {
                                    self.status =
                                        format!("cannot start {}: {e}", daemon.display())
                                }
                            }
                        }
                    });
                });
                if !self.status.is_empty() {
                    cols[1].colored_label(egui::Color32::LIGHT_GREEN, &self.status);
                }
                self.weather_dialog(cols[1].ctx());
            });
        });
    }
}

fn main() {
    turing_smart_screen_rust::logger::init();
    turing_smart_screen_rust::cli::ensure_working_dir(&PathBuf::from("config.yaml"));
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Turing System Monitor configuration")
            .with_inner_size([880.0, 700.0]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "turing-configure",
        opts,
        Box::new(|cc| match ConfigureApp::new(cc) {
            Ok(app) => {
                // Python's GUI type runs larger than egui's default: scale
                // text styles directly (deterministic; eframe may override
                // pixels_per_point from OS DPI behind our back).
                cc.egui_ctx.global_style_mut(|style| {
                    for (ts, size) in [
                        (egui::TextStyle::Small, 13.0),
                        (egui::TextStyle::Body, 16.0),
                        (egui::TextStyle::Monospace, 15.0),
                        (egui::TextStyle::Button, 16.0),
                        (egui::TextStyle::Heading, 24.0),
                    ] {
                        if let Some(font) = style.text_styles.get_mut(&ts) {
                            font.size = size;
                        }
                    }
                });
                Ok(Box::new(app))
            }
            Err(err) => {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }),
    ) {
        eprintln!("error: GUI failed: {e}");
        std::process::exit(1);
    }
}
