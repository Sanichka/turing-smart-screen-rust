// SPDX-License-Identifier: GPL-3.0-or-later
//! Theme renderer. Ports `library/stats.py` (value dispatch) on top of the
//! RGB565 framebuffer.
//!
//! Deliberate deviation from the plan: `imageproc` is **not** used. Its
//! drawing API targets generic image buffers; hand-rolled RGB565/RGB8
//! primitives are smaller, avoid a pixel-format round-trip, and let Step 5
//! ship dirty regions without conversion.
//!
//! `Renderer` owns the framebuffer, font cache and line-graph histories.
//! Images are preloaded once into `ImageCache` (no FS I/O in the draw path).

pub mod framebuf;
pub mod text;
pub mod theme_widgets;
pub mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_yaml::Value;

use crate::config::Theme;
use crate::sensors::Snapshot;
use framebuf::{Framebuf, Rect, Rgb};
use text::{draw_text, FontCache};
use theme_widgets::{
    child, fmt_date, fmt_freq_ghz, fmt_giga, fmt_mega, fmt_percent, fmt_temp, fmt_time,
    fmt_uptime, fmt_value, history_size, path, rate2human, BarW, Bg, GraphW, RadialW,
    TextW,
};
use widgets::{draw_bar, draw_graph, draw_radial};

// ---------------------------------------------------------------------------
// Image cache (preloaded once, read-only during rendering)
// ---------------------------------------------------------------------------

pub struct DecodedImage {
    pub w: u32,
    pub h: u32,
    pub rgb: Vec<u8>,
}

pub struct ImageCache {
    map: HashMap<PathBuf, DecodedImage>,
}

impl ImageCache {
    /// Scan theme for every referenced bitmap and decode up front.
    pub fn preload(theme: &Theme) -> Self {
        let mut paths: HashSet<PathBuf> = HashSet::new();
        for img in theme.static_images.values() {
            paths.insert(theme.dir.join(&img.path));
        }
        collect_bg_images(&theme.raw, &theme.dir, &mut paths);
        let mut map = HashMap::new();
        for p in paths {
            match image::open(&p) {
                Ok(img) => {
                    let rgb = img.to_rgb8();
                    let (w, h) = (rgb.width(), rgb.height());
                    map.insert(p, DecodedImage { w, h, rgb: rgb.into_raw() });
                }
                Err(e) => log::warn!("cannot load image {}: {e}", p.display()),
            }
        }
        ImageCache { map }
    }

    pub fn get(&self, theme_dir: &Path, name: &str) -> Option<&DecodedImage> {
        self.map.get(&theme_dir.join(name))
    }
}

fn collect_bg_images(v: &Value, theme_dir: &Path, out: &mut HashSet<PathBuf>) {
    match v {
        Value::Mapping(m) => {
            for (k, val) in m {
                if k.as_str() == Some("BACKGROUND_IMAGE") {
                    if let Value::String(s) = val {
                        if !s.is_empty() {
                            out.insert(theme_dir.join(s));
                        }
                    }
                } else {
                    collect_bg_images(val, theme_dir, out);
                }
            }
        }
        Value::Sequence(s) => {
            for val in s {
                collect_bg_images(val, theme_dir, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Histories (ring buffers for line graphs, NaN-filled like Python)
// ---------------------------------------------------------------------------

/// Record a dirty rectangle (degenerate rects are dropped). Free function
/// (not a method) so call sites can mutably borrow disjoint `Renderer`
/// fields (`fb`, `fonts`, `dirty`) in a single expression.
fn mark_rect(dirty: &mut Vec<Rect>, r: Option<Rect>) {
    if let Some(r) = r {
        if r.valid() {
            dirty.push(r);
        }
    }
}

struct History {
    buf: Vec<f32>,
    pos: usize,
}

impl History {
    fn push(&mut self, v: f32, size: usize) {
        if self.buf.len() != size {
            self.buf = vec![f32::NAN; size];
            self.pos = 0;
        }
        self.buf[self.pos] = v;
        self.pos = (self.pos + 1) % size;
    }

    /// Oldest → newest into `scratch` (no per-draw allocation in daemon).
    fn ordered<'a>(&self, scratch: &'a mut Vec<f32>) -> &'a [f32] {
        scratch.clear();
        let n = self.buf.len();
        for i in 0..n {
            scratch.push(self.buf[(self.pos + i) % n]);
        }
        scratch.as_slice()
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

pub struct Renderer {
    pub fb: Framebuf,
    fonts: FontCache,
    histories: HashMap<String, History>,
    scratch: Vec<f32>,
    pub dirty: Vec<Rect>,
    warned_custom: bool,
}

impl Renderer {
    pub fn new(w: u32, h: u32, fonts_dir: PathBuf) -> Self {
        Renderer {
            fb: Framebuf::new(w, h, [255, 255, 255]), // SIMU parity: white canvas
            fonts: FontCache::new(fonts_dir),
            histories: HashMap::new(),
            scratch: Vec::new(),
            dirty: Vec::new(),
            warned_custom: false,
        }
    }


    fn bg<'a>(&self, imgs: &'a ImageCache, theme_dir: &Path, name: Option<&str>, solid: Rgb) -> Bg<'a> {
        match name.and_then(|n| imgs.get(theme_dir, n)) {
            Some(img) => Bg::Crop { rgb: &img.rgb, w: img.w, h: img.h },
            None => Bg::Solid(solid),
        }
    }

    /// Static boot layer: images then texts (mirrors `main.py` order).
    pub fn draw_static(&mut self, theme: &Theme, imgs: &ImageCache) {
        for img in theme.static_images.values() {
            let p = theme.dir.join(&img.path);
            let Some(dec) = imgs.map.get(&p) else {
                log::warn!("static image missing: {}", p.display());
                continue;
            };
            let (mut w, mut h, mut rgb) = (dec.w, dec.h, None);
            if img.width > 0 && img.height > 0
                && (img.width as u32 != dec.w || img.height as u32 != dec.h)
            {
                // Resize once at boot (cost irrelevant outside the loop).
                let im: image::RgbImage =
                    image::ImageBuffer::from_raw(dec.w, dec.h, dec.rgb.clone())
                        .expect("cached image dims");
                let r = image::imageops::resize(
                    &im,
                    img.width as u32,
                    img.height as u32,
                    image::imageops::FilterType::Triangle,
                );
                w = r.width();
                h = r.height();
                rgb = Some(r.into_raw());
            }
            let bytes = rgb.as_deref().unwrap_or(&dec.rgb);
            // Crop to display like DisplayPILImage (never overflow).
            let cw = w.min(self.fb.w);
            let ch = h.min(self.fb.h);
            let mut cropped = Vec::with_capacity((cw * ch * 3) as usize);
            for y in 0..ch {
                let s = (y as usize * w as usize) * 3;
                cropped.extend_from_slice(&bytes[s..s + cw as usize * 3]);
            }
            mark_rect(&mut self.dirty, self.fb.blit_rgb8(img.x, img.y, cw, ch, &cropped));
        }
        for t in theme.static_texts.values() {
            let bg = self.bg(imgs, &theme.dir, t.background_image.as_deref(), t.background_color);
            mark_rect(&mut self.dirty, draw_text(
                &mut self.fb,
                &mut self.fonts,
                &t.text,
                t.x,
                t.y,
                t.width,
                t.height,
                &t.font,
                t.font_size,
                t.font_color,
                "left",
                "lt",
                |buf, w, h, ox, oy| {
                    theme_widgets::paint_canvas(buf, w, h, ox, oy, &bg);
                },
            ));
        }
    }

    /// Full stats layer from one snapshot (mirrors `stats.py` classes).
    pub fn draw_snapshot(&mut self, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(stats) = child(&theme.raw, "STATS") else {
            return;
        };
        self.cpu(path(stats, &["CPU"]), theme, imgs, snap);
        self.gpu(path(stats, &["GPU"]), theme, imgs, snap);
        self.memory(path(stats, &["MEMORY"]), theme, imgs, snap);
        self.disk(path(stats, &["DISK"]), theme, imgs, snap);
        self.net(path(stats, &["NET"]), theme, imgs, snap);
        self.date(path(stats, &["DATE"]), theme, imgs, snap);
        self.uptime(path(stats, &["UPTIME"]), theme, imgs, snap);
        self.custom(path(stats, &["CUSTOM"]), theme, imgs, snap);
        self.weather(path(stats, &["WEATHER"]), theme, imgs, snap);
        self.ping(path(stats, &["PING"]), theme, imgs, snap);
    }

    // -- helpers ----------------------------------------------------------

    fn hist(&mut self, key: &str, v: f32, size: usize) {
        self.histories
            .entry(key.to_string())
            .or_insert(History { buf: Vec::new(), pos: 0 })
            .push(v, size);
    }

    /// Draw TEXT/GRAPH/RADIAL/LINE_GRAPH quad for a percent metric.
    fn percent_quad(
        &mut self,
        node: Option<&Value>,
        theme: &Theme,
        imgs: &ImageCache,
        value: f32,
        hist_key: &str,
    ) {
        let Some(n) = node else { return };
        if value.is_nan() {
            return; // Python disables the widgets with a warning; daemon caches that
        }
        if let Some(t) = child(n, "TEXT").and_then(TextW::parse) {
            let s = fmt_percent(value, t.show_unit);
            self.text(theme, imgs, &t, &s);
        }
        if let Some(b) = child(n, "GRAPH").and_then(BarW::parse) {
            let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
            let r = draw_bar(&mut self.fb, &b, value, &bg);
            mark_rect(&mut self.dirty, r);
        }
        if let Some(rw) = child(n, "RADIAL").and_then(RadialW::parse) {
            let s = if rw.show_text {
                fmt_percent(value, rw.show_unit)
            } else {
                String::new()
            };
            let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
            let r = draw_radial(&mut self.fb, &mut self.fonts, &rw, value, &s, &bg);
            mark_rect(&mut self.dirty, r);
        }
        if let Some(g) = child(n, "LINE_GRAPH").and_then(GraphW::parse) {
            let size = history_size(n);
            self.hist(hist_key, value, size);
            let vals = self.histories[hist_key].ordered(&mut self.scratch).to_vec();
            // LINE_GRAPH has no BACKGROUND_IMAGE key in Python (solid only).
            let solid = Bg::Solid([0, 0, 0]);
            let r = draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid);
            mark_rect(&mut self.dirty, r);
        }
    }

    /// Draw TEXT/GRAPH/RADIAL/LINE_GRAPH quad for a temperature metric.
    fn temp_quad(
        &mut self,
        node: Option<&Value>,
        theme: &Theme,
        imgs: &ImageCache,
        value: f32,
        hist_key: &str,
    ) {
        let Some(n) = node else { return };
        if value.is_nan() {
            return;
        }
        if let Some(t) = child(n, "TEXT").and_then(TextW::parse) {
            let s = fmt_temp(value, t.show_unit);
            self.text(theme, imgs, &t, &s);
        }
        if let Some(b) = child(n, "GRAPH").and_then(BarW::parse) {
            let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
            let r = draw_bar(&mut self.fb, &b, value, &bg);
            mark_rect(&mut self.dirty, r);
        }
        if let Some(rw) = child(n, "RADIAL").and_then(RadialW::parse) {
            let s = if rw.show_text {
                fmt_temp(value, rw.show_unit)
            } else {
                String::new()
            };
            let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
            let r = draw_radial(&mut self.fb, &mut self.fonts, &rw, value, &s, &bg);
            mark_rect(&mut self.dirty, r);
        }
        if let Some(g) = child(n, "LINE_GRAPH").and_then(GraphW::parse) {
            let size = history_size(n);
            self.hist(hist_key, value, size);
            let vals = self.histories[hist_key].ordered(&mut self.scratch).to_vec();
            let solid = Bg::Solid([0, 0, 0]);
            let r = draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid);
            mark_rect(&mut self.dirty, r);
        }
    }

    fn text(&mut self, theme: &Theme, imgs: &ImageCache, t: &TextW, s: &str) {
        if s.is_empty() {
            return;
        }
        let bg = self.bg(imgs, &theme.dir, t.bg_image.as_deref(), t.bg);
        let r = draw_text(
            &mut self.fb,
            &mut self.fonts,
            s,
            t.x,
            t.y,
            t.w,
            t.h,
            &t.font,
            t.font_size,
            t.fg,
            &t.align,
            &t.anchor,
            |buf, w, h, ox, oy| {
                theme_widgets::paint_canvas(buf, w, h, ox, oy, &bg);
            },
        );
        mark_rect(&mut self.dirty, r);
    }

    /// Arbitrary TEXT widget with a preformatted string.
    fn raw_text(
        &mut self,
        node: Option<&Value>,
        theme: &Theme,
        imgs: &ImageCache,
        s: &str,
    ) {
        if s.is_empty() {
            return;
        }
        if let Some(t) = node.and_then(TextW::parse) {
            self.text(theme, imgs, &t, s);
        }
    }

    // -- sections (mirror stats.py classes) --------------------------------

    fn cpu(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        // PERCENTAGE quad + line history.
        if let Some(p) = path(n, &["PERCENTAGE"]) {
            if !snap.cpu_percent.is_nan() {
                let size = history_size(p);
                self.hist("cpu_pct", snap.cpu_percent, size);
                if let Some(t) = child(p, "TEXT").and_then(TextW::parse) {
                    self.text(theme, imgs, &t, &fmt_percent(snap.cpu_percent, t.show_unit));
                }
                if let Some(b) = child(p, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.cpu_percent, &bg));
                }
                if let Some(rw) = child(p, "RADIAL").and_then(RadialW::parse) {
                    let s = if rw.show_text {
                        fmt_percent(snap.cpu_percent, rw.show_unit)
                    } else {
                        String::new()
                    };
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        snap.cpu_percent,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(p, "LINE_GRAPH").and_then(GraphW::parse) {
                    let vals = self.histories["cpu_pct"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
        }
        // FREQUENCY.
        if let Some(fq) = path(n, &["FREQUENCY"]) {
            if !snap.cpu_freq_mhz.is_nan() {
                let s = fmt_freq_ghz(snap.cpu_freq_mhz);
                if let Some(t) = child(fq, "TEXT").and_then(TextW::parse) {
                    self.text(theme, imgs, &t, &s);
                }
                let ghz = snap.cpu_freq_mhz / 1000.0;
                if let Some(b) = child(fq, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, ghz, &bg));
                }
                if let Some(rw) = child(fq, "RADIAL").and_then(RadialW::parse) {
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        ghz,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(fq, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(fq);
                    self.hist("cpu_freq", ghz, size);
                    let vals = self.histories["cpu_freq"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
        }
        // LOAD 1/5/15 min.
        if let Some(load) = path(n, &["LOAD"]) {
            for (key, v) in [
                ("ONE", snap.cpu_load_1),
                ("FIVE", snap.cpu_load_5),
                ("FIFTEEN", snap.cpu_load_15),
            ] {
                if v.is_nan() {
                    continue;
                }
                if let Some(t) = path(load, &[key, "TEXT"]).and_then(TextW::parse) {
                    self.text(theme, imgs, &t, &fmt_percent(v, t.show_unit));
                }
            }
        }
        self.temp_quad(path(n, &["TEMPERATURE"]), theme, imgs, snap.cpu_temp_c, "cpu_temp");
        self.percent_quad(path(n, &["FAN_SPEED"]), theme, imgs, snap.cpu_fan_percent, "cpu_fan");
    }

    fn gpu(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        self.percent_quad(path(n, &["PERCENTAGE"]), theme, imgs, snap.gpu_load_pct, "gpu_pct");
        self.percent_quad(
            path(n, &["MEMORY_PERCENT"]),
            theme,
            imgs,
            snap.gpu_mem_pct,
            "gpu_mem",
        );
        // Absolute memory texts (M).
        if !snap.gpu_mem_used_mb.is_nan() {
            let s = fmt_mega((snap.gpu_mem_used_mb * 1024.0 * 1024.0) as u64, true);
            self.raw_text(path(n, &["MEMORY_USED", "TEXT"]), theme, imgs, &s);
            // Legacy MEMORY.TEXT (deprecated alias, same value).
            self.raw_text(path(n, &["MEMORY", "TEXT"]), theme, imgs, &s);
        }
        if !snap.gpu_mem_total_mb.is_nan() {
            let s = fmt_mega((snap.gpu_mem_total_mb * 1024.0 * 1024.0) as u64, true);
            self.raw_text(path(n, &["MEMORY_TOTAL", "TEXT"]), theme, imgs, &s);
        }
        // Legacy MEMORY graph/radial (deprecated alias of MEMORY_PERCENT).
        if !snap.gpu_mem_pct.is_nan() {
            if let Some(legacy) = path(n, &["MEMORY"]) {
                if let Some(b) = child(legacy, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.gpu_mem_pct, &bg));
                }
                if let Some(rw) = child(legacy, "RADIAL").and_then(RadialW::parse) {
                    let s = if rw.show_text {
                        fmt_percent(snap.gpu_mem_pct, rw.show_unit)
                    } else {
                        String::new()
                    };
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        snap.gpu_mem_pct,
                        &s,
                        &bg,
                    ));
                }
            }
        }
        self.temp_quad(path(n, &["TEMPERATURE"]), theme, imgs, snap.gpu_temp_c, "gpu_temp");
        // FPS (int + " FPS" unit, -1 = unsupported).
        if snap.gpu_fps >= 0 {
            if let Some(fps) = path(n, &["FPS"]) {
                let size = history_size(fps);
                self.hist("gpu_fps", snap.gpu_fps as f32, size);
                if let Some(t) = child(fps, "TEXT").and_then(TextW::parse) {
                    let s = fmt_value(snap.gpu_fps as i64, 4, " FPS", t.show_unit);
                    self.text(theme, imgs, &t, &s);
                }
                if let Some(b) = child(fps, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.gpu_fps as f32, &bg));
                }
                if let Some(rw) = child(fps, "RADIAL").and_then(RadialW::parse) {
                    let s = fmt_value(snap.gpu_fps as i64, 4, " FPS", rw.show_unit);
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        snap.gpu_fps as f32,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(fps, "LINE_GRAPH").and_then(GraphW::parse) {
                    let vals = self.histories["gpu_fps"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
        }
        self.percent_quad(path(n, &["FAN_SPEED"]), theme, imgs, snap.gpu_fan_pct, "gpu_fan");
        // FREQUENCY (GHz text like CPU).
        if let Some(fq) = path(n, &["FREQUENCY"]) {
            if !snap.gpu_freq_mhz.is_nan() {
                let s = fmt_freq_ghz(snap.gpu_freq_mhz);
                if let Some(t) = child(fq, "TEXT").and_then(TextW::parse) {
                    self.text(theme, imgs, &t, &s);
                }
                let ghz = snap.gpu_freq_mhz / 1000.0;
                if let Some(b) = child(fq, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, ghz, &bg));
                }
                if let Some(rw) = child(fq, "RADIAL").and_then(RadialW::parse) {
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        ghz,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(fq, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(fq);
                    self.hist("gpu_freq", ghz, size);
                    let vals = self.histories["gpu_freq"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
        }
    }

    fn memory(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        // SWAP: graph/radial/line only (no TEXT in schema).
        if let Some(swap) = path(n, &["SWAP"]) {
            if !snap.mem_swap_percent.is_nan() {
                if let Some(b) = child(swap, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.mem_swap_percent, &bg));
                }
                if let Some(rw) = child(swap, "RADIAL").and_then(RadialW::parse) {
                    let s = if rw.show_text {
                        fmt_percent(snap.mem_swap_percent, rw.show_unit)
                    } else {
                        String::new()
                    };
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        snap.mem_swap_percent,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(swap, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(swap);
                    self.hist("mem_swap", snap.mem_swap_percent, size);
                    let vals = self.histories["mem_swap"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
        }
        // VIRTUAL.
        if let Some(virt) = path(n, &["VIRTUAL"]) {
            if !snap.mem_virtual_percent.is_nan() {
                if let Some(t) = child(virt, "PERCENT_TEXT").and_then(TextW::parse) {
                    self.text(
                        theme,
                        imgs,
                        &t,
                        &fmt_percent(snap.mem_virtual_percent, t.show_unit),
                    );
                }
                if let Some(b) = child(virt, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.mem_virtual_percent, &bg));
                }
                if let Some(rw) = child(virt, "RADIAL").and_then(RadialW::parse) {
                    let s = if rw.show_text {
                        fmt_percent(snap.mem_virtual_percent, rw.show_unit)
                    } else {
                        String::new()
                    };
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        snap.mem_virtual_percent,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(virt, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(virt);
                    self.hist("mem_virt", snap.mem_virtual_percent, size);
                    let vals = self.histories["mem_virt"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
            if let Some(t) = child(virt, "USED").and_then(TextW::parse) {
                self.text(theme, imgs, &t, &fmt_mega(snap.mem_used_bytes, t.show_unit));
            }
            if let Some(t) = child(virt, "FREE").and_then(TextW::parse) {
                self.text(theme, imgs, &t, &fmt_mega(snap.mem_free_bytes, t.show_unit));
            }
            if let Some(t) = child(virt, "TOTAL").and_then(TextW::parse) {
                self.text(theme, imgs, &t, &fmt_mega(snap.mem_total_bytes, t.show_unit));
            }
        }
    }

    fn disk(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        if let Some(used) = path(n, &["USED"]) {
            if !snap.disk_usage_percent.is_nan() {
                if let Some(t) = child(used, "PERCENT_TEXT").and_then(TextW::parse) {
                    self.text(
                        theme,
                        imgs,
                        &t,
                        &fmt_percent(snap.disk_usage_percent, t.show_unit),
                    );
                }
                if let Some(b) = child(used, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.disk_usage_percent, &bg));
                }
                if let Some(rw) = child(used, "RADIAL").and_then(RadialW::parse) {
                    let s = if rw.show_text {
                        fmt_percent(snap.disk_usage_percent, rw.show_unit)
                    } else {
                        String::new()
                    };
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                            mark_rect(&mut self.dirty, draw_radial(
                        &mut self.fb,
                        &mut self.fonts,
                        &rw,
                        snap.disk_usage_percent,
                        &s,
                        &bg,
                    ));
                }
                if let Some(g) = child(used, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(used);
                    self.hist("disk_use", snap.disk_usage_percent, size);
                    let vals = self.histories["disk_use"].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
            if let Some(t) = child(used, "TEXT").and_then(TextW::parse) {
                self.text(theme, imgs, &t, &fmt_giga(snap.disk_used_bytes, t.show_unit));
            }
        }
        if let Some(t) = path(n, &["TOTAL", "TEXT"]).and_then(TextW::parse) {
            self.text(theme, imgs, &t, &fmt_giga(snap.disk_total_bytes, t.show_unit));
        }
        if let Some(t) = path(n, &["FREE", "TEXT"]).and_then(TextW::parse) {
            self.text(theme, imgs, &t, &fmt_giga(snap.disk_free_bytes, t.show_unit));
        }
    }

    fn net(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        for (key, stats) in [("WLO", &snap.net_wlo), ("ETH", &snap.net_eth)] {
            let Some(iface) = child(n, key) else {
                continue;
            };
            if !stats.present {
                continue;
            }
            // UPLOAD rate + graph.
            if let Some(up) = child(iface, "UPLOAD") {
                if let Some(t) = child(up, "TEXT").and_then(TextW::parse) {
                    let s = format!("{:>10}", rate2human(stats.upload_rate_bps));
                    self.text(theme, imgs, &t, &s);
                }
                if let Some(g) = child(up, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(up);
                    let hk = if key == "WLO" { "wlo_up" } else { "eth_up" };
                    self.hist(hk, stats.upload_rate_bps as f32, size);
                    let vals = self.histories[hk].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
            if let Some(t) = path(iface, &["UPLOADED", "TEXT"]).and_then(TextW::parse) {
                let s = format!("{:>6}", theme_widgets::bytes2human(stats.uploaded_bytes));
                self.text(theme, imgs, &t, &s);
            }
            if let Some(dl) = child(iface, "DOWNLOAD") {
                if let Some(t) = child(dl, "TEXT").and_then(TextW::parse) {
                    let s = format!("{:>10}", rate2human(stats.download_rate_bps));
                    self.text(theme, imgs, &t, &s);
                }
                if let Some(g) = child(dl, "LINE_GRAPH").and_then(GraphW::parse) {
                    let size = history_size(dl);
                    let hk = if key == "WLO" { "wlo_down" } else { "eth_down" };
                    self.hist(hk, stats.download_rate_bps as f32, size);
                    let vals = self.histories[hk].ordered(&mut self.scratch).to_vec();
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
                }
            }
            if let Some(t) = path(iface, &["DOWNLOADED", "TEXT"]).and_then(TextW::parse) {
                let s = format!("{:>6}", theme_widgets::bytes2human(stats.downloaded_bytes));
                self.text(theme, imgs, &t, &s);
            }
        }
    }

    fn date(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        if let Some(t) = path(n, &["DAY", "TEXT"]).and_then(TextW::parse) {
            self.text(theme, imgs, &t, &fmt_date(snap.date_epoch, &t.format));
        }
        if let Some(t) = path(n, &["HOUR", "TEXT"]).and_then(TextW::parse) {
            self.text(theme, imgs, &t, &fmt_time(snap.date_epoch, &t.format));
        }
    }

    fn uptime(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        if let Some(t) = path(n, &["SECONDS", "TEXT"]).and_then(TextW::parse) {
            self.text(theme, imgs, &t, &snap.uptime_secs.to_string());
        }
        if let Some(t) = path(n, &["FORMATTED", "TEXT"]).and_then(TextW::parse) {
            self.text(theme, imgs, &t, &fmt_uptime(snap.uptime_secs));
        }
    }

    /// Custom plugin sensors (ports `stats.Custom`). Theme `CUSTOM` keys
    /// name a registered `CustomDataSource`; unknown names warn once each.
    fn custom(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        let Value::Mapping(m) = n else { return };
        for (k, sub) in m {
            let Some(name) = k.as_str() else { continue };
            if name == "INTERVAL" {
                continue;
            }
            let Some(r) = snap.custom.iter().find(|r| r.name == name) else {
                if !self.warned_custom {
                    // Rate-limit: one warning per unknown name per process.
                    log::warn!("custom sensor '{name}' is not registered (see sensors/custom.rs)");
                    self.warned_custom = true;
                }
                continue;
            };
            let string = r.string.clone();
            if let Some(t) = child(sub, "TEXT").and_then(TextW::parse) {
                if let Some(s) = string.as_deref() {
                    self.text(theme, imgs, &t, s);
                }
            }
            if let Some(numeric) = r.numeric {
                if numeric.is_nan() {
                    continue;
                }
                if let Some(b) = child(sub, "GRAPH").and_then(BarW::parse) {
                    let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
                    mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, numeric, &bg));
                }
                if let Some(rw) = child(sub, "RADIAL").and_then(RadialW::parse) {
                    let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
                    let s = string.clone().unwrap_or_default();
                    mark_rect(
                        &mut self.dirty,
                        draw_radial(&mut self.fb, &mut self.fonts, &rw, numeric, &s, &bg),
                    );
                }
            }
            if let Some(g) = child(sub, "LINE_GRAPH").and_then(GraphW::parse) {
                if !r.history.is_empty() {
                    let solid = Bg::Solid([0, 0, 0]);
                    mark_rect(
                        &mut self.dirty,
                        draw_graph(&mut self.fb, &mut self.fonts, &g, &r.history, &solid),
                    );
                }
            }
        }
    }

    fn weather(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        self.raw_text(
            path(n, &["TEMPERATURE", "TEXT"]),
            theme,
            imgs,
            snap.wx_temp.as_deref().unwrap_or(""),
        );
        self.raw_text(
            path(n, &["TEMPERATURE_FELT", "TEXT"]),
            theme,
            imgs,
            snap.wx_felt.as_deref().unwrap_or(""),
        );
        self.raw_text(
            path(n, &["UPDATE_TIME", "TEXT"]),
            theme,
            imgs,
            snap.wx_update.as_deref().unwrap_or(""),
        );
        self.raw_text(
            path(n, &["HUMIDITY", "TEXT"]),
            theme,
            imgs,
            snap.wx_humidity.as_deref().unwrap_or(""),
        );
        self.raw_text(
            path(n, &["WEATHER_DESCRIPTION", "TEXT"]),
            theme,
            imgs,
            snap.wx_description.as_deref().unwrap_or(""),
        );
    }

    fn ping(&mut self, node: Option<&Value>, theme: &Theme, imgs: &ImageCache, snap: &Snapshot) {
        let Some(n) = node else { return };
        if snap.ping_ms.is_nan() || snap.ping_ms < 0.0 {
            return;
        }
        let v = snap.ping_ms as i64;
        if let Some(t) = child(n, "TEXT").and_then(TextW::parse) {
            let s = fmt_value(v, 6, "ms", t.show_unit);
            self.text(theme, imgs, &t, &s);
        }
        if let Some(b) = child(n, "GRAPH").and_then(BarW::parse) {
            let bg = self.bg(imgs, &theme.dir, b.bg_image.as_deref(), b.bg);
            mark_rect(&mut self.dirty, draw_bar(&mut self.fb, &b, snap.ping_ms, &bg));
        }
        if let Some(rw) = child(n, "RADIAL").and_then(RadialW::parse) {
            let s = fmt_value(v, 6, "ms", rw.show_unit);
            let bg = self.bg(imgs, &theme.dir, rw.bg_image.as_deref(), rw.bg);
            mark_rect(&mut self.dirty, draw_radial(
                &mut self.fb,
                &mut self.fonts,
                &rw,
                snap.ping_ms,
                &s,
                &bg,
            ));
        }
        if let Some(g) = child(n, "LINE_GRAPH").and_then(GraphW::parse) {
            let size = history_size(n);
            self.hist("ping", snap.ping_ms, size);
            let vals = self.histories["ping"].ordered(&mut self.scratch).to_vec();
            let solid = Bg::Solid([0, 0, 0]);
            mark_rect(&mut self.dirty, draw_graph(&mut self.fb, &mut self.fonts, &g, &vals, &solid));
        }
    }
}

