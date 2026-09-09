// SPDX-License-Identifier: GPL-3.0-or-later
//! Native configuration GUI (Rust port of `configure.py`).
//! Step 2 skeleton: theme picker + live STATIC preview + Save.
//! Full form sections (display/system/weather dialogs) land next.

use std::path::{Path, PathBuf};

use eframe::egui;
use turing_smart_screen_rust::config::{
    load_app_config, load_theme, AppConfig, Revision, Theme,
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
            theme,
            preview: None,
            preview_size: (0.0, 0.0),
            status: String::new(),
        };
        app.com_port = app.cfg.general.com_port.clone();
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
        }
        if let Some(display) = v.get_mut("display") {
            display["REVISION"] =
                serde_yaml::Value::String(revision_str(self.cfg.display.revision).to_string());
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

fn theme_size_of(themes: &[(String, String)], name: &str) -> String {
    themes
        .iter()
        .find(|(t, _)| t == name)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| "?".to_string())
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
                cols[1].separator();
                cols[1].horizontal(|ui| {
                    if ui.button("Save settings").clicked() {
                        self.save_all();
                    }
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
                if !self.status.is_empty() {
                    cols[1].colored_label(egui::Color32::LIGHT_GREEN, &self.status);
                }
            });
        });
    }
}

fn main() {
    turing_smart_screen_rust::logger::init();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Turing System Monitor configuration")
            .with_inner_size([760.0, 640.0]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "turing-configure",
        opts,
        Box::new(|cc| match ConfigureApp::new(cc) {
            Ok(app) => Ok(Box::new(app)),
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
