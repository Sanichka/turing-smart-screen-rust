// SPDX-License-Identifier: GPL-3.0-or-later
//! RGB565 framebuffer + raster primitives.
//!
//! The display hardware consumes RGB565LE; keeping the framebuffer in native
//! `u16` avoids a full-frame conversion before serial dispatch (Step 5 sends
//! regions directly). All primitives clip to bounds and never allocate.

pub type Rgb = [u8; 3];

#[inline]
pub fn rgb_to_565(c: Rgb) -> u16 {
    ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3)
}

#[inline]
pub fn rgb565_to_888(v: u16) -> Rgb {
    // Expand with low-bit replication for round-trip accuracy.
    let r = ((v >> 11) & 0x1F) as u8;
    let g = ((v >> 5) & 0x3F) as u8;
    let b = (v & 0x1F) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// Inclusive-exclusive dirty rectangle, display coordinates.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Rect { x0, y0, x1, y1 }
    }

    pub fn valid(&self) -> bool {
        self.x1 > self.x0 && self.y1 > self.y0
    }

    /// Union of two dirty rects (used by Step-5 serial coalescing).
    #[allow(dead_code)]
    pub fn union(&self, o: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

pub struct Framebuf {
    pub w: u32,
    pub h: u32,
    px: Vec<u16>,
}

/// Direct-draw RGB565 primitives (kept for Step-5 incremental updates;
/// single-shot rendering composites via `RgbCanvas` instead).
#[allow(dead_code)]
impl Framebuf {
    pub fn new(w: u32, h: u32, fill: Rgb) -> Self {
        let v = rgb_to_565(fill);
        Framebuf {
            w,
            h,
            px: vec![v; (w * h) as usize],
        }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: u16) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            let i = y as usize * self.w as usize + x as usize;
            // SAFETY: bounds checked above.
            unsafe { *self.px.get_unchecked_mut(i) = c };
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Option<u16> {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            Some(self.px[y as usize * self.w as usize + x as usize])
        } else {
            None
        }
    }

    /// Fill axis-aligned rect (clipped). Returns the clipped dirty rect.
    pub fn fill_rect(&mut self, r: Rect, c: Rgb) -> Option<Rect> {
        let c = rgb_to_565(c);
        let x0 = r.x0.max(0);
        let y0 = r.y0.max(0);
        let x1 = r.x1.min(self.w as i32);
        let y1 = r.y1.min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        for y in y0..y1 {
            let base = y as usize * self.w as usize;
            for x in x0..x1 {
                self.px[base + x as usize] = c;
            }
        }
        Some(Rect::new(x0, y0, x1, y1))
    }

    pub fn rect_outline(&mut self, r: Rect, c: Rgb) {
        let c = rgb_to_565(c);
        for x in r.x0..r.x1 {
            self.set(x, r.y0, c);
            self.set(x, r.y1 - 1, c);
        }
        for y in r.y0..r.y1 {
            self.set(r.x0, y, c);
            self.set(r.x1 - 1, y, c);
        }
    }

    /// Bresenham line (used by line graphs + axis).
    pub fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, c: Rgb) {
        let c = rgb_to_565(c);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.set(x0, y0, c);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Thick line via filled-circle stamping (radial bars, graph strokes).
    pub fn thick_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: u32, c: Rgb) {
        if width <= 1 {
            self.line(x0, y0, x1, y1, c);
            return;
        }
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()) as u32).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = (x0 as f32 + (x1 - x0) as f32 * t).round() as i32;
            let y = (y0 as f32 + (y1 - y0) as f32 * t).round() as i32;
            self.fill_circle(x, y, width as i32 / 2, c);
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, c: Rgb) {
        let c = rgb_to_565(c);
        for y in (cy - r)..=(cy + r) {
            let dy = y - cy;
            let dx = ((r * r - dy * dy).max(0) as f32).sqrt() as i32;
            for x in (cx - dx)..=(cx + dx) {
                self.set(x, y, c);
            }
        }
    }

    /// Blit an RGB8 row-major image at (x, y), clipped. Returns dirty rect.
    pub fn blit_rgb8(&mut self, x: i32, y: i32, w: u32, h: u32, rgb: &[u8]) -> Option<Rect> {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.w as i32);
        let y1 = (y + h as i32).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        for dy in y0..y1 {
            let sy = (dy - y) as usize;
            let dst_base = dy as usize * self.w as usize;
            for dx in x0..x1 {
                let sx = (dx - x) as usize;
                let si = (sy * w as usize + sx) * 3;
                let v = rgb_to_565([rgb[si], rgb[si + 1], rgb[si + 2]]);
                self.px[dst_base + dx as usize] = v;
            }
        }
        Some(Rect::new(x0, y0, x1, y1))
    }
}

/// RGB8 software canvas for widget compositing (background image crop +
/// vector drawing), blitted once to the RGB565 framebuffer. Keeps per-widget
/// logic free of framebuffer borrows.
pub struct RgbCanvas {
    pub w: u32,
    pub h: u32,
    pub buf: Vec<u8>,
}

impl RgbCanvas {
    pub fn new(w: u32, h: u32, fill: Rgb) -> Self {
        let mut buf = vec![0u8; (w * h * 3) as usize];
        for px in buf.as_chunks_mut::<3>().0 {
            px.copy_from_slice(&fill);
        }
        RgbCanvas { w, h, buf }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            let i = (y as usize * self.w as usize + x as usize) * 3;
            self.buf[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn fill_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        let x0 = x0.max(0);
        let y0 = y0.max(0);
        let x1 = x1.min(self.w as i32);
        let y1 = y1.min(self.h as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y as usize * self.w as usize + x as usize) * 3;
                self.buf[i..i + 3].copy_from_slice(&c);
            }
        }
    }

    pub fn rect_outline(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        for x in x0..x1 {
            self.set(x, y0, c);
            self.set(x, y1 - 1, c);
        }
        for y in y0..y1 {
            self.set(x0, y, c);
            self.set(x1 - 1, y, c);
        }
    }

    pub fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, c: Rgb) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.set(x0, y0, c);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    pub fn thick_polyline(&mut self, pts: &[(i32, i32)], width: u32, c: Rgb) {
        if pts.len() < 2 || width == 0 {
            return;
        }
        for w in pts.windows(2) {
            self.thick_line(w[0].0, w[0].1, w[1].0, w[1].1, width, c);
        }
    }

    pub fn thick_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: u32, c: Rgb) {
        if width <= 1 {
            self.line(x0, y0, x1, y1, c);
            return;
        }
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()) as u32).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = (x0 as f32 + (x1 - x0) as f32 * t).round() as i32;
            let y = (y0 as f32 + (y1 - y0) as f32 * t).round() as i32;
            self.fill_circle(x, y, width as i32 / 2, c);
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, c: Rgb) {
        for y in (cy - r)..=(cy + r) {
            let dy = y - cy;
            let dx = ((r * r - dy * dy).max(0) as f32).sqrt() as i32;
            for x in (cx - dx)..=(cx + dx) {
                self.set(x, y, c);
            }
        }
    }

    pub fn blit_into(&self, fb: &mut Framebuf, x: i32, y: i32) -> Option<Rect> {
        fb.blit_rgb8(x, y, self.w, self.h, &self.buf)
    }

    /// Crop to sub-rectangle (custom_bbox), origin-relative.
    pub fn cropped(&self, r: Rect) -> RgbCanvas {
        let x0 = r.x0.max(0);
        let y0 = r.y0.max(0);
        let x1 = r.x1.min(self.w as i32);
        let y1 = r.y1.min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return RgbCanvas::new(1, 1, [0, 0, 0]);
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut buf = vec![0u8; (w * h * 3) as usize];
        for y in y0..y1 {
            for x in x0..x1 {
                let si = (y as usize * self.w as usize + x as usize) * 3;
                let di = ((y - y0) as usize * w as usize + (x - x0) as usize) * 3;
                buf[di..di + 3].copy_from_slice(&self.buf[si..si + 3]);
            }
        }
        RgbCanvas { w, h, buf }
    }
}

impl Framebuf {
    /// Raw RGB565LE bytes of a region (for Step-5 serial dispatch).
    #[allow(dead_code)]
    pub fn region_rgb565le(&self, r: &Rect, out: &mut Vec<u8>) {        out.clear();
        let x0 = r.x0.max(0);
        let y0 = r.y0.max(0);
        let x1 = r.x1.min(self.w as i32);
        let y1 = r.y1.min(self.h as i32);
        out.reserve(((x1 - x0).max(0) * (y1 - y0).max(0) * 2) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let v = self.px[y as usize * self.w as usize + x as usize];
                out.push((v & 0xFF) as u8);
                out.push((v >> 8) as u8);
            }
        }
    }

    /// Write full frame to PNG (`screencap.png`, SIMU parity).
    pub fn save_png(&self, path: &str) -> Result<(), String> {
        let mut rgb = Vec::with_capacity(self.px.len() * 3);
        for &v in &self.px {
            let [r, g, b] = rgb565_to_888(v);
            rgb.push(r);
            rgb.push(g);
            rgb.push(b);
        }
        let img: image::RgbImage = image::ImageBuffer::from_raw(self.w, self.h, rgb)
            .ok_or_else(|| "framebuffer size mismatch".to_string())?;
        img.save(path).map_err(|e| format!("cannot save {path}: {e}"))?;
        Ok(())
    }

    /// Read-only pixel view (native RGB565). Used for frame diffing.
    pub fn pixels(&self) -> &[u16] {
        &self.px
    }

    /// Tiles (`tw`×`th`) containing at least one pixel differing from `prev`
    /// (same dimensions). Coarse but allocation-free per tile; supersedes
    /// per-widget dirty rects for incremental display updates and absorbs
    /// text ghosting (stale pixels always differ).
    pub fn changed_tiles(&self, prev: &[u16], tw: u32, th: u32) -> Vec<Rect> {
        debug_assert_eq!(prev.len(), self.px.len());
        if prev.len() != self.px.len() {
            return vec![Rect::new(0, 0, self.w as i32, self.h as i32)];
        }
        let mut out = Vec::new();
        let nx = self.w.div_ceil(tw);
        let ny = self.h.div_ceil(th);
        for ty in 0..ny {
            for tx in 0..nx {
                let x0 = tx * tw;
                let y0 = ty * th;
                let x1 = (x0 + tw).min(self.w);
                let y1 = (y0 + th).min(self.h);
                let mut dirty = false;
                'scan: for y in y0..y1 {
                    let base = y as usize * self.w as usize;
                    for x in x0..x1 {
                        let i = base + x as usize;
                        if self.px[i] != prev[i] {
                            dirty = true;
                            break 'scan;
                        }
                    }
                }
                if dirty {
                    out.push(Rect::new(x0 as i32, y0 as i32, x1 as i32, y1 as i32));
                }
            }
        }
        out
    }
}
