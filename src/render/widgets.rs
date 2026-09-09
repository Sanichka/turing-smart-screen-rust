// SPDX-License-Identifier: GPL-3.0-or-later
//! Widget painters. Ports `LcdComm.DisplayProgressBar`,
//! `DisplayRadialProgressBar` and `DisplayLineGraph` onto `RgbCanvas`.

use super::framebuf::{Framebuf, Rect, RgbCanvas};
use super::text::{measure_line, FontCache, draw_string_rgb};
use super::theme_widgets::{paint_canvas, BarW, Bg, GraphW, RadialW};

// ---------------------------------------------------------------------------
// Progress bar
// ---------------------------------------------------------------------------

pub fn draw_bar(
    fb: &mut Framebuf,
    w: &BarW,
    value: f32,
    bg: &Bg,
) -> Option<super::framebuf::Rect> {
    if w.w <= 0 || w.h <= 0 {
        return None;
    }
    let span = w.max_v - w.min_v;
    if span <= 0.0 {
        return None;
    }
    let v = value.clamp(w.min_v, w.max_v);
    let frac = (v - w.min_v) / span;
    let (bw, bh) = (w.w as u32, w.h as u32);
    let mut cv = RgbCanvas::new(bw, bh, [0, 0, 0]);
    paint_canvas(&mut cv.buf, bw, bh, w.x, w.y, bg);

    if w.w > w.h {
        let filled = (frac * w.w as f32 - 1.0).max(0.0) as i32;
        let (x1, x2) = if w.reverse {
            (w.w - 1 - filled, w.w - 1)
        } else {
            (0, filled)
        };
        cv.fill_rect(x1, 0, x2 + 1, w.h, w.color);
        if w.outline {
            cv.rect_outline(0, 0, w.w, w.h, w.color);
        }
    } else {
        let filled = (frac * w.h as f32 - 1.0).max(0.0) as i32;
        let (y1, y2) = if w.reverse {
            (0, filled)
        } else {
            (w.h - 1 - filled, w.h - 1)
        };
        cv.fill_rect(0, y1, w.w, y2 + 1, w.color);
        if w.outline {
            cv.rect_outline(0, 0, w.w, w.h, w.color);
        }
    }
    cv.blit_into(fb, w.x, w.y)
}

// ---------------------------------------------------------------------------
// Radial progress bar
// ---------------------------------------------------------------------------

/// Python floored modulo (`%` on floats): `-90 % 361 == 271`.
fn pymod(a: f32, n: f32) -> f32 {
    ((a % n) + n) % n
}

/// Points along an arc; PIL convention (0° at 3 o'clock, grows clockwise on
/// screen because y points down).
fn arc_points(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<(i32, i32)> {
    let sweep = (a1 - a0).abs();
    if sweep <= f32::EPSILON {
        return vec![(cx.round() as i32, cy.round() as i32)];
    }
    let n = ((sweep / 2.0).ceil() as usize).clamp(2, 720);
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (
                (cx + r * a.cos()).round() as i32,
                (cy + r * a.sin()).round() as i32,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn draw_radial(
    fb: &mut Framebuf,
    fonts: &mut FontCache,
    w: &RadialW,
    value: f32,
    text: &str,
    bg: &Bg,
) -> Option<Rect> {
    if w.radius <= 0 || w.width <= 0 || w.width > w.radius {
        log::warn!("radial out of range: radius={} width={}", w.radius, w.width);
        return None;
    }
    if w.steps <= 0 || w.sep < 0.0 || w.sep * w.steps as f32 >= 360.0 {
        log::warn!("radial bad segmentation: steps={} sep={}", w.steps, w.sep);
        return None;
    }
    let span = w.max_v - w.min_v;
    if span <= 0.0 {
        return None;
    }
    let v = value.clamp(w.min_v, w.max_v);
    let pct = (v - w.min_v) / span;

    let (mut a_start, mut a_end) = (w.a_start, w.a_end);
    if pymod(a_start, 361.0) == pymod(a_end, 361.0) {
        if w.clockwise {
            a_start += 0.1;
        } else {
            a_end += 0.1;
        }
    }
    a_start = pymod(a_start, 361.0);
    a_end = pymod(a_end, 361.0);

    let r = w.radius as f32;
    let bw = w.width;
    let rc = r - bw as f32 / 2.0; // stroke centerline (dots use the same)
    let d = (2 * w.radius) as u32;
    let mut cv = RgbCanvas::new(d, d, [0, 0, 0]);
    paint_canvas(&mut cv.buf, d, d, w.xc - w.radius, w.yc - w.radius, bg);

    let dot = |cv: &mut RgbCanvas, angle_deg: f32, c: super::framebuf::Rgb| {
        let a = angle_deg.to_radians();
        let x = (r + rc * a.cos()).round() as i32;
        let y = (r + rc * a.sin()).round() as i32;
        cv.fill_circle(x, y, bw / 2, c);
    };

    if w.clockwise {
        let ecart = if a_end < a_start {
            360.0 - a_start + a_end
        } else {
            a_end - a_start
        };
        if w.draw_bg {
            let pts = arc_points(r, r, rc, a_start, a_start + ecart);
            cv.thick_polyline(&pts, bw as u32, w.bg_bar_color);
        }
        if w.decoration == "Ellipse" {
            dot(&mut cv, a_end, w.bg_bar_color);
            dot(&mut cv, a_start, w.color);
            dot(&mut cv, a_start + pct * ecart, w.color);
        }
        if w.sep == 0.0 {
            let pts = arc_points(r, r, rc, a_start, a_start + pct * ecart);
            cv.thick_polyline(&pts, bw as u32, w.color);
        } else {
            let a_e = a_start + pct * ecart;
            let step = ecart / w.steps as f32;
            let full = ((a_e - a_start) / step).floor() as i32;
            for i in 0..full {
                let pts = arc_points(
                    r,
                    r,
                    rc,
                    a_start + i as f32 * step,
                    a_start + (i + 1) as f32 * step - w.sep,
                );
                cv.thick_polyline(&pts, bw as u32, w.color);
            }
            let pts = arc_points(r, r, rc, a_start + full as f32 * step, a_e);
            cv.thick_polyline(&pts, bw as u32, w.color);
        }
    } else {
        let ecart = if a_end < a_start {
            a_start - a_end
        } else {
            360.0 - a_end + a_start
        };
        if w.draw_bg {
            let pts = arc_points(r, r, rc, a_start - ecart, a_start);
            cv.thick_polyline(&pts, bw as u32, w.bg_bar_color);
        }
        if w.decoration == "Ellipse" {
            dot(&mut cv, a_end, w.bg_bar_color);
            dot(&mut cv, a_start, w.color);
            dot(&mut cv, a_start - pct * ecart, w.color);
        }
        if w.sep == 0.0 {
            let pts = arc_points(r, r, rc, a_start - pct * ecart, a_start);
            cv.thick_polyline(&pts, bw as u32, w.color);
        } else {
            let a_s = a_start - pct * ecart;
            let step = ecart / w.steps as f32;
            let full = ((a_start - a_s) / step).floor() as i32;
            for i in 0..full {
                let pts = arc_points(
                    r,
                    r,
                    rc,
                    a_start - (i + 1) as f32 * step + w.sep,
                    a_start - i as f32 * step,
                );
                cv.thick_polyline(&pts, bw as u32, w.color);
            }
            let pts = arc_points(r, r, rc, a_s, a_start - full as f32 * step);
            cv.thick_polyline(&pts, bw as u32, w.color);
        }
    }

    // Centered value text.
    if !text.is_empty() {
        if let Some(font) = fonts.font(&w.font) {
            if let Some((tw, th, min_x, min_y)) = measure_line(&font, w.font_size, text) {
                let ox = r as i32 + w.text_offset[0] - tw as i32 / 2 - min_x;
                let oy = r as i32 + w.text_offset[1] - th as i32 / 2 - min_y;
                draw_string_rgb(&font, w.font_size, text, w.fg, &mut cv.buf, d, d, ox, oy);
            }
        }
    }

    // custom_bbox crop (nonzero = active, like Python).
    let cb = w.custom_bbox;
    if cb != [0, 0, 0, 0] {
        let sub = cv.cropped(Rect::new(cb[0], cb[1], cb[2], cb[3]));
        let (sw, sh) = (sub.w, sub.h);
        let mut cv2 = RgbCanvas::new(sw, sh, [0, 0, 0]);
        cv2.buf = sub.buf;
        cv2.blit_into(fb, w.xc - w.radius + cb[0], w.yc - w.radius + cb[1])
    } else {
        cv.blit_into(fb, w.xc - w.radius, w.yc - w.radius)
    }
}

// ---------------------------------------------------------------------------
// Line graph
// ---------------------------------------------------------------------------

pub fn draw_graph(
    fb: &mut Framebuf,
    fonts: &mut FontCache,
    w: &GraphW,
    values: &[f32],
    bg: &Bg,
) -> Option<Rect> {
    if w.w <= 0 || w.h <= 0 || values.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (w.min_v, w.max_v);
    if w.autoscale {
        let (mut tmin, mut tmax) = (hi, lo);
        for &v in values {
            if !v.is_nan() {
                tmin = tmin.min(v);
                tmax = tmax.max(v);
            }
        }
        if tmin != hi && tmax != lo {
            lo = (tmin - 5.0).max(lo);
            hi = (tmax + 5.0).min(hi);
        }
    }
    if hi <= lo {
        return None;
    }
    let (bw, bh) = (w.w as u32, w.h as u32);
    let mut cv = RgbCanvas::new(bw, bh, [0, 0, 0]);
    paint_canvas(&mut cv.buf, bw, bh, w.x, w.y, bg);

    let yscale = bh as f32 / (hi - lo);
    let step = w.w as f32 / values.len() as f32;
    let mut pts: Vec<(i32, i32)> = Vec::new();
    let mut count = 0;
    for &v in values {
        if v.is_nan() {
            continue;
        }
        let v = v.clamp(lo, hi);
        pts.push((
            (count as f32 * step) as i32,
            (bh as f32 - (v - lo) * yscale) as i32,
        ));
        count += 1;
    }
    cv.thick_polyline(&pts, w.line_width, w.line_color);

    if w.axis {
        cv.line(0, w.h - 1, w.w - 1, w.h - 1, w.axis_color);
        cv.line(0, 0, 0, w.h - 1, w.axis_color);
        cv.line(0, 0, 1, 0, w.axis_color);
        if let Some(font) = fonts.font(&w.axis_font) {
            for (label, lx, ly) in [
                (format!("{hi:.0}"), 2, 0),
                (
                    format!("{lo:.0}"),
                    w.w - 2 - 8 * format!("{lo:.0}").len() as i32,
                    w.h - 2 - w.axis_font_size as i32,
                ),
            ] {
                if let Some((_, _, min_x, min_y)) =
                    measure_line(&font, w.axis_font_size, &label)
                {
                    draw_string_rgb(
                        &font,
                        w.axis_font_size,
                        &label,
                        w.axis_color,
                        &mut cv.buf,
                        bw,
                        bh,
                        lx - min_x,
                        ly - min_y,
                    );
                }
            }
        }
    }
    cv.blit_into(fb, w.x, w.y)
}
