// SPDX-License-Identifier: GPL-3.0-or-later
//! Simulated display: paste regions into an in-memory image, save
//! `screencap.png` per update. Ports `LcdSimulated` (minus the HTTP preview;
//! open the PNG directly).

use super::{DisplayDriver, Orientation};

pub struct Simu {
    width: u32,
    height: u32,
    orientation: Orientation,
    img: image::RgbImage,
}

impl Simu {
    pub fn new(width: u32, height: u32) -> Self {
        Simu {
            width,
            height,
            orientation: Orientation::Portrait,
            img: image::ImageBuffer::from_pixel(width, height, image::Rgb([255, 255, 255])),
        }
    }

    fn dims(&self) -> (u32, u32) {
        match self.orientation {
            Orientation::Portrait | Orientation::ReversePortrait => (self.width, self.height),
            Orientation::Landscape | Orientation::ReverseLandscape => (self.height, self.width),
        }
    }

    fn save(&self) {
        if let Err(e) = self.img.save("screencap.png") {
            log::warn!("cannot save screencap.png: {e}");
        }
    }
}

impl DisplayDriver for Simu {
    fn name(&self) -> &'static str {
        "Simu"
    }

    fn native_dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn initialize(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn reset(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn clear(&mut self) -> Result<(), String> {
        let (w, h) = self.dims();
        self.img = image::ImageBuffer::from_pixel(w, h, image::Rgb([255, 255, 255]));
        self.save();
        Ok(())
    }

    fn screen_off(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn screen_on(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn set_brightness(&mut self, _level: u8) -> Result<(), String> {
        Ok(())
    }

    fn reconnect(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn set_orientation(&mut self, orientation: Orientation) -> Result<(), String> {
        self.orientation = orientation;
        let (w, h) = self.dims();
        self.img = image::ImageBuffer::from_pixel(w, h, image::Rgb([255, 255, 255]));
        self.save();
        Ok(())
    }

    fn send_region(
        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        rgb565le: &[u8],
    ) -> Result<(), String> {
        let (dw, dh) = (self.dims().0 as i32, self.dims().1 as i32);
        let x0c = x0.max(0);
        let y0c = y0.max(0);
        let x1c = x1.min(dw);
        let y1c = y1.min(dh);
        if x1c <= x0c || y1c <= y0c {
            return Ok(());
        }
        let full_w = (x1 - x0) as usize;
        let full_h = (y1 - y0) as usize;
        if rgb565le.len() != full_w * full_h * 2 {
            return Err(format!(
                "region payload size mismatch: {} bytes for {full_w}x{full_h}",
                rgb565le.len()
            ));
        }
        for y in y0c..y1c {
            for x in x0c..x1c {
                let si = ((y - y0) as usize * full_w + (x - x0) as usize) * 2;
                let v = rgb565le[si] as u16 | ((rgb565le[si + 1] as u16) << 8);
                let r = (((v >> 11) & 0x1F) as u8) << 3;
                let g = (((v >> 5) & 0x3F) as u8) << 2;
                let b = ((v & 0x1F) as u8) << 3;
                self.img.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
            }
        }
        self.save();
        Ok(())
    }
}
