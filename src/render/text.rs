// SPDX-License-Identifier: GPL-3.0-or-later
//! Text rasterization via `ab_glyph` (no `imageproc`: we blend glyph coverage
//! directly into RGB8 canvases, then blit once to the RGB565 framebuffer).
//!
//! Note: ab_glyph 0.2 has no text-layout helper, so single-line layout
//! (advance + kerning) is done here explicitly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontArc, OutlinedGlyph, PxScale, Rect as GlyRect};

use super::framebuf::{Framebuf, Rect, Rgb};

pub struct FontCache {
    fonts_dir: PathBuf,
    fonts: HashMap<PathBuf, FontArc>,
}

impl FontCache {
    pub fn new(fonts_dir: PathBuf) -> Self {
        FontCache {
            fonts_dir,
            fonts: HashMap::new(),
        }
    }

    /// Load (cached) or `None` with a warning.
    pub fn font(&mut self, name: &str) -> Option<FontArc> {
        // Absolute paths pass through; theme fonts are relative to fonts_dir.
        let p = Path::new(name);
        let full = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.fonts_dir.join(name)
        };
        if let Some(f) = self.fonts.get(&full) {
            return Some(f.clone());
        }
        let bytes = match std::fs::read(&full) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("cannot load font {}: {e}", full.display());
                return None;
            }
        };
        match FontArc::try_from_vec(bytes) {
            Ok(f) => {
                self.fonts.insert(full, f.clone());
                Some(f)
            }
            Err(e) => {
                log::warn!("invalid font {name}: {e}");
                None
            }
        }
    }
}

/// Lay out one line: advance + kerning walk, calling `f` per outlined glyph
/// with its pixel bounds and coverage source. `origin` is the layout origin
/// (baseline start); bounds may extend negative of it.
pub fn layout_line(
    font: &FontArc,
    size: u32,
    text: &str,
    ox: f32,
    oy: f32,
    mut f: impl FnMut(GlyRect, &OutlinedGlyph),
) {
    let scale = PxScale::from(size as f32);
    let upem = font.units_per_em().unwrap_or(1000.0);
    let sf = size as f32 / upem;
    let mut caret = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        if let Some(p) = prev {
            caret += font.kern_unscaled(p, gid) * sf;
        }
        prev = Some(gid);
        let g = gid.with_scale_and_position(scale, ab_glyph::point(ox + caret, oy));
        caret += font.h_advance_unscaled(gid) * sf;
        if let Some(o) = font.outline_glyph(g) {
            f(o.px_bounds(), &o);
        }
    }
}

/// Tight pixel bounds of a line laid out at origin `(0,0)`:
/// `(width, height, min_x, min_y)`.
pub fn measure_line(font: &FontArc, size: u32, text: &str) -> Option<(u32, u32, i32, i32)> {
    let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
    let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
    layout_line(font, size, text, 0.0, 0.0, |bb, _| {
        min_x = min_x.min(bb.min.x);
        max_x = max_x.max(bb.max.x);
        min_y = min_y.min(bb.min.y);
        max_y = max_y.max(bb.max.y);
    });
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    Some((
        ((max_x - min_x).ceil() as u32).max(1),
        ((max_y - min_y).ceil() as u32).max(1),
        min_x as i32,
        min_y as i32,
    ))
}

/// Horizontal/vertical anchor fractions from PIL `anchor` (e.g. "lt", "mm").
fn anchor_frac(anchor: &str) -> (f32, f32) {
    let mut h = 0.0;
    let mut v = 0.0;
    let mut it = anchor.chars();
    match it.next() {
        Some('m') => h = 0.5,
        Some('r') => h = 1.0,
        _ => {}
    }
    match it.next() {
        Some('m') => v = 0.5,
        Some('b') | Some('d') | Some('s') => v = 1.0,
        _ => {}
    }
    (h, v)
}

/// Blend one laid-out line onto an RGB8 canvas.
///
/// `(ox, oy)` is the desired top-left of the line's tight box: glyphs are
/// laid out at the origin internally, then shifted so the measured minimum
/// lands exactly at `(ox, oy)` (callers pre-subtract the measured min).
#[allow(clippy::too_many_arguments)] // mirrors DisplayText's wide PIL-style signature
pub fn draw_string_rgb(
    font: &FontArc,
    size: u32,
    text: &str,
    fg: Rgb,
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    ox: i32,
    oy: i32,
) {
    layout_line(font, size, text, 0.0, 0.0, |bb, o| {
        let bx = bb.min.x as i32 + ox;
        let by = bb.min.y as i32 + oy;
        o.draw(|gx, gy, cov| {
            let px = bx + gx as i32;
            let py = by + gy as i32;
            if px < 0 || py < 0 || px >= canvas_w as i32 || py >= canvas_h as i32 {
                return;
            }
            let i = (py as usize * canvas_w as usize + px as usize) * 3;
            let a = cov;
            let inv = 1.0 - a;
            canvas[i] = (fg[0] as f32 * a + canvas[i] as f32 * inv) as u8;
            canvas[i + 1] = (fg[1] as f32 * a + canvas[i + 1] as f32 * inv) as u8;
            canvas[i + 2] = (fg[2] as f32 * a + canvas[i + 2] as f32 * inv) as u8;
        });
    });
}

/// Port of `LcdComm.DisplayText`: render text into its box and blit to `fb`.
///
/// `bg_fill` paints the box background (solid or theme-image crop); kept as
/// a closure so text.rs stays free of image-cache types.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    fb: &mut Framebuf,
    fonts: &mut FontCache,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font_name: &str,
    font_size: u32,
    fg: Rgb,
    _align: &str, // single-line themes: alignment is a no-op (noted for Step 5)
    anchor: &str,
    bg_fill: impl FnMut(&mut Vec<u8>, u32, u32, i32, i32),
) -> Option<Rect> {
    if text.is_empty() || font_size == 0 {
        return None;
    }
    let font = fonts.font(font_name)?;
    let (tw, th, min_x, min_y) =
        measure_line(&font, font_size, text).unwrap_or((1, font_size, 0, 0));
    let (tw, th) = (tw as i32, th as i32);
    let (hf, vf) = anchor_frac(anchor);

    // Resolve text box + glyph origin, mirroring lcd_comm.py branches.
    let (bx0, by0, bw, bh, ox, oy) = if w > 0 && h > 0 {
        let ax = x + ((w - tw) as f32 * hf) as i32;
        let ay = y + ((h - th) as f32 * vf) as i32;
        (x, y, w, h, ax, ay)
    } else {
        let bx = x - (tw as f32 * hf) as i32;
        let by = y - (th as f32 * vf) as i32;
        (bx, by, tw, th, bx, by)
    };
    if bw <= 0 || bh <= 0 {
        return None;
    }

    // Box canvas in RGB8: painted background, glyphs blended on top.
    // Canvas coordinates are box-local, so the draw origin is relative to
    // the box: tight-box top-left lands at (ox - bx0, oy - by0).
    let (bw_u, bh_u) = (bw as u32, bh as u32);
    let mut canvas = vec![0u8; (bw_u * bh_u * 3) as usize];
    let mut bg_fill = bg_fill;
    bg_fill(&mut canvas, bw_u, bh_u, bx0, by0);
    draw_string_rgb(
        &font,
        font_size,
        text,
        fg,
        &mut canvas,
        bw_u,
        bh_u,
        ox - bx0 - min_x,
        oy - by0 - min_y,
    );
    fb.blit_rgb8(bx0, by0, bw_u, bh_u, &canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_visible_glyphs() {
        let mut fonts = FontCache::new(PathBuf::from("res/fonts"));
        // Run from crate root (cargo test sets cwd there).
        let font = fonts.font("jetbrains-mono/JetBrainsMono-Bold.ttf");
        assert!(font.is_some(), "font must load");
        let font = font.unwrap();
        let m = measure_line(&font, 23, "15%");
        assert!(m.is_some(), "measure must find glyphs");
        let (tw, th, min_x, min_y) = m.unwrap();
        assert!(tw > 5 && th > 5, "glyph box too small: {tw}x{th}");
        let (w, h) = (tw + 4, th + 4);
        let mut canvas = vec![100u8; (w * h * 3) as usize];
        // Place tight-box top-left at (2,2) per draw_string_rgb contract.
        draw_string_rgb(&font, 23, "15%", [255, 255, 255], &mut canvas, w, h, 2 - min_x, 2 - min_y);
        let bright = canvas
            .chunks_exact(3)
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
            .count();
        assert!(bright > 20, "expected bright glyph pixels, got {bright}");
    }

    /// Count near-white pixels inside a rect of screencap.png.
    /// Run after `--render-once` with the default 3.5inchTheme2 theme:
    /// `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn cpu_text_zones_are_not_blank() {
        let img = image::open("screencap.png").unwrap().to_rgb8();
        let count = |x0: u32, y0: u32, x1: u32, y1: u32| -> u32 {
            let mut n = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = img.get_pixel(x, y);
                    if p[0] > 200 && p[1] > 200 && p[2] > 200 {
                        n += 1;
                    }
                }
            }
            n
        };
        // CPU temp text zone (154,24,size23) and pct zone (250,24).
        let temp = count(150, 20, 240, 55);
        let pct = count(245, 20, 320, 55);
        let date = count(150, 0, 320, 20);
        println!("temp={temp} pct={pct} date={date}");
        assert!(pct > 30, "cpu pct zone blank: {pct}");
    }

    #[test]
    #[ignore]
    fn dump_freq_canvas() {
        use super::super::framebuf::Framebuf;
        let mut fonts = FontCache::new(PathBuf::from("res/fonts"));
        let mut fb = Framebuf::new(480, 800, [132, 154, 165]);
        let r = draw_text(
            &mut fb, &mut fonts, "2.40 GHz", 300, 100, 0, 0,
            "jetbrains-mono/JetBrainsMono-Bold.ttf", 30, [255, 255, 255],
            "left", "lt",
            |buf, w, h, ox, oy| {
                super::super::theme_widgets::paint_canvas(
                    buf, w, h, ox, oy,
                    &super::super::theme_widgets::Bg::Solid([132, 154, 165]),
                );
            },
        );
        println!("rect={r:?}");
        fb.save_png("dbg-text.png").unwrap();
    }

    /// Zone probe for the 5inchTheme2Radial STATIC render (480x800):
    /// CPU freq "2.40 GHz" at (300,100,size30), GPU pct at (285,230),
    /// GPU radial ring around (141,275,r28).
    #[test]
    #[ignore]
    fn radial_theme_zones() {
        let img = image::open("screencap-radial.png").unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (480, 800));
        let white = |x0: u32, y0: u32, x1: u32, y1: u32| -> u32 {
            let mut n = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = img.get_pixel(x, y);
                    if p[0] > 200 && p[1] > 200 && p[2] > 200 {
                        n += 1;
                    }
                }
            }
            n
        };
        let red = |x0: u32, y0: u32, x1: u32, y1: u32| -> u32 {
            let mut n = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = img.get_pixel(x, y);
                    if p[0] > 180 && p[1] < 100 && p[2] < 100 {
                        n += 1;
                    }
                }
            }
            n
        };
        let freq = white(295, 95, 430, 140);
        let gpct = white(280, 225, 360, 260);
        let gring = red(100, 235, 185, 315);
        println!("freq={freq} gpct={gpct} gring={gring}");
        assert!(freq > 30, "cpu freq zone blank: {freq}");
        assert!(gpct > 30, "gpu pct zone blank: {gpct}");
        assert!(gring > 30, "gpu radial ring missing: {gring}");
    }
}
