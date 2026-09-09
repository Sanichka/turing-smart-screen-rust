// SPDX-License-Identifier: GPL-3.0-or-later
//! Turing Smart Screen Rev A driver (3.5" + UsbPCMonitor). Ports
//! `library/lcd/lcd_comm_rev_a.py` byte-for-byte on the wire.

use super::port::SerialLink;
use super::{DisplayDriver, Orientation};

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum Command {
    Reset = 101,
    Clear = 102,
    ScreenOff = 108,
    ScreenOn = 109,
    SetBrightness = 110,
    SetOrientation = 121,
    DisplayBitmap = 197,
    Hello = 69,
}

pub struct RevA {
    link: SerialLink,
    width: u32,  // native portrait width (from HELLO sub-revision)
    height: u32, // native portrait height
    orientation: Orientation,
}

impl RevA {
    pub fn new(com_port: &str) -> Result<Self, String> {
        Ok(RevA {
            link: SerialLink::open(com_port)?,
            width: 320,
            height: 480,
            orientation: Orientation::Portrait,
        })
    }

    fn dims(&self) -> (u32, u32) {
        match self.orientation {
            Orientation::Portrait | Orientation::ReversePortrait => (self.width, self.height),
            Orientation::Landscape | Orientation::ReverseLandscape => (self.height, self.width),
        }
    }

    fn send_command(&mut self, cmd: Command, x: u32, y: u32, ex: u32, ey: u32) -> Result<(), String> {
        let buf = [
            (x >> 2) as u8,
            ((((x & 3) << 6) + (y >> 4)) & 0xFF) as u8,
            ((((y & 15) << 4) + (ex >> 6)) & 0xFF) as u8,
            ((((ex & 63) << 2) + (ey >> 8)) & 0xFF) as u8,
            ((ey & 255) & 0xFF) as u8,
            cmd as u8,
        ];
        self.write_line(&buf)
    }

    /// Port of `WriteLine`: write, reopening once on transport errors.
    fn write_line(&mut self, data: &[u8]) -> Result<(), String> {
        match self.link.write_all(data) {
            Ok(()) => Ok(()),
            Err(e) => {
                log::error!(
                    "serial write failed ({e}); closing and reopening port before retrying once"
                );
                self.link.close();
                std::thread::sleep(std::time::Duration::from_secs(1));
                self.link.reopen()?;
                self.link.write_all(data)
            }
        }
    }

    /// HELLO probe: identifies UsbPCMonitor sub-revisions and sizes.
    fn hello(&mut self) -> Result<(), String> {
        let ping = [Command::Hello as u8; 6];
        self.write_line(&ping)?;
        let mut resp = [0u8; 6];
        // A stock Turing 3.5" never answers: timeouts are expected there.
        match self.link.read_exact(&mut resp) {
            Ok(()) => {}
            Err(e) => {
                log::debug!("HELLO unread ({e}); assuming Turing 3.5\"");
                self.width = 320;
                self.height = 480;
                self.link.flush_input();
                return Ok(());
            }
        }
        self.link.flush_input();
        (self.width, self.height) = match resp {
            [1, 1, 1, 1, 1, 1] => (320, 480),
            [2, 2, 2, 2, 2, 2] => (480, 800),
            [3, 3, 3, 3, 3, 3] => (600, 1024),
            _ => (320, 480),
        };
        log::debug!("HELLO response {resp:?} → {}x{}", self.width, self.height);
        Ok(())
    }
}

impl DisplayDriver for RevA {
    fn name(&self) -> &'static str {
        "RevA"
    }

    fn native_dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.hello()
    }

    fn reset(&mut self) -> Result<(), String> {
        log::info!("display reset (COM port may change)...");
        self.send_command(Command::Reset, 0, 0, 0, 0)?;
        self.link.close();
        // Display reboots; the COM port can disappear for seconds.
        std::thread::sleep(std::time::Duration::from_secs(5));
        self.link.reopen()
    }

    fn clear(&mut self) -> Result<(), String> {
        // Hardware quirk: orientation must be PORTRAIT before clearing.
        let back = self.orientation;
        self.set_orientation(Orientation::Portrait)?;
        self.send_command(Command::Clear, 0, 0, 0, 0)?;
        self.set_orientation(back)
    }

    fn screen_off(&mut self) -> Result<(), String> {
        self.send_command(Command::ScreenOff, 0, 0, 0, 0)
    }

    fn screen_on(&mut self) -> Result<(), String> {
        self.send_command(Command::ScreenOn, 0, 0, 0, 0)
    }

    fn set_brightness(&mut self, level: u8) -> Result<(), String> {
        assert!(level <= 100, "brightness must be 0-100");
        // Device scale is inverted: 0 = brightest, 255 = darkest.
        let absolute = (255.0 - (level as f32 / 100.0) * 255.0) as u32;
        self.send_command(Command::SetBrightness, absolute, 0, 0, 0)
    }

    fn set_orientation(&mut self, orientation: Orientation) -> Result<(), String> {
        self.orientation = orientation;
        let (w, h) = self.dims();
        let (x, y, ex, ey) = (0u32, 0u32, 0u32, 0u32);
        let buf = [
            (x >> 2) as u8,
            ((((x & 3) << 6) + (y >> 4)) & 0xFF) as u8,
            ((((y & 15) << 4) + (ex >> 6)) & 0xFF) as u8,
            ((((ex & 63) << 2) + (ey >> 8)) & 0xFF) as u8,
            ((ey & 255) & 0xFF) as u8,
            Command::SetOrientation as u8,
            (orientation as u8 + 100),
            ((w >> 8) & 0xFF) as u8,
            ((w & 255) & 0xFF) as u8,
            ((h >> 8) & 0xFF) as u8,
            ((h & 255) & 0xFF) as u8,
            0, 0, 0, 0, 0,
        ];
        debug_assert_eq!(buf.len(), 16);
        self.write_line(&buf)
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
        // Crop to device like DisplayPILImage (never overflow).
        let x0c = x0.max(0);
        let y0c = y0.max(0);
        let x1c = x1.min(dw);
        let y1c = y1.min(dh);
        if x1c <= x0c || y1c <= y0c {
            return Ok(());
        }
        // Caller guarantees `rgb565le` matches the UNCROPPED (x0,y0,x1,y1);
        // crop rows here when the region was clamped.
        let full_w = (x1 - x0) as usize;
        let ox = (x0c - x0) as usize;
        let oy = (y0c - y0) as usize;
        let cw = (x1c - x0c) as usize;
        let ch = (y1c - y0c) as usize;
        debug_assert_eq!(rgb565le.len(), full_w * (y1 - y0) as usize * 2);
        let mut cropped;
        let payload: &[u8] = if ox == 0 && oy == 0 && cw == full_w && ch == (y1 - y0) as usize {
            rgb565le
        } else {
            cropped = Vec::with_capacity(cw * ch * 2);
            for row in 0..ch {
                let s = ((oy + row) * full_w + ox) * 2;
                cropped.extend_from_slice(&rgb565le[s..s + cw * 2]);
            }
            &cropped
        };

        self.send_command(
            Command::DisplayBitmap,
            x0c as u32,
            y0c as u32,
            (x1c - 1) as u32,
            (y1c - 1) as u32,
        )?;
        // Payload in multiples of display-width bytes (protocol requirement).
        let chunk = self.dims().0 as usize * 8;
        for part in payload.chunks(chunk.max(1)) {
            self.write_line(part)?;
        }
        Ok(())
    }
}
