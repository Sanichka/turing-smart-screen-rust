// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware drivers. Ports `library/display.py` (factory) + `LcdComm`.
//!
//! Only Rev A (`A`) and simulated (`SIMU`) are ported in Step 5 — the
//! machine under test is a Rev A screen. Other revisions fail fast with a
//! clear message; same trait, same `DisplayOp` path when they land.

pub mod port;
pub mod rev_a;
pub mod simu;

use std::sync::mpsc::{Receiver, SyncSender};
use std::thread::JoinHandle;

use crate::config::Revision;

/// Screen orientation. Discriminants match the Rev A wire values
/// (`orientation + 100`); keep them stable across drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait = 0,
    ReversePortrait = 1,
    Landscape = 2,
    ReverseLandscape = 3,
}

/// Uniform driver API (subset of `LcdComm` needed by the daemon).
/// All methods are synchronous: the dispatcher thread is the sole caller.
pub trait DisplayDriver: Send {
    fn name(&self) -> &'static str;
    /// Native portrait dimensions (device truth, cf. theme dims).
    fn native_dims(&self) -> (u32, u32);
    fn initialize(&mut self) -> Result<(), String>;
    fn reset(&mut self) -> Result<(), String>;
    /// White-clear (hardware quirk paths use it; boot paints static instead).
    #[allow(dead_code)]
    fn clear(&mut self) -> Result<(), String>;
    fn screen_on(&mut self) -> Result<(), String>;
    fn screen_off(&mut self) -> Result<(), String>;
    fn set_brightness(&mut self, level: u8) -> Result<(), String>;
    fn set_orientation(&mut self, orientation: Orientation) -> Result<(), String>;
    /// Push one RGB565LE region (uncropped `x0..x1`, `y0..y1`).
    fn send_region(
        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        rgb565le: &[u8],
    ) -> Result<(), String>;
}

/// Hardware routing lives here and nowhere else (AGENTS.md).
pub fn create(
    rev: Revision,
    com_port: &str,
    theme_w: u32,
    theme_h: u32,
) -> Result<Box<dyn DisplayDriver>, String> {
    match rev {
        Revision::A => Ok(Box::new(rev_a::RevA::new(com_port)?)),
        Revision::Simu => Ok(Box::new(simu::Simu::new(theme_w, theme_h))),
        other => Err(format!(
            "display revision {other:?} is not ported to Rust yet (Rev A + SIMU in Step 5)"
        )),
    }
}

/// Work item for the serial dispatcher thread. Regions carry owned bytes:
/// small (widget-scale), allocated per update — the bounded channel applies
/// backpressure instead of buffering unboundedly.
pub enum DisplayOp {
    Region {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        data: Vec<u8>,
    },
    /// Drain marker: the dispatcher processes all queued regions first,
    /// then turns the panel off and exits. Sending `Stop` on the bounded
    /// channel therefore doubles as the graceful-drain barrier.
    Stop,
}

/// Sole owner of the driver; sequential writes, no interleaving.
pub fn spawn_dispatcher(
    mut driver: Box<dyn DisplayDriver>,
    rx: Receiver<DisplayOp>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("display-io".into())
        .spawn(move || {
            for op in rx {
                match op {
                    DisplayOp::Region { x0, y0, x1, y1, data } => {
                        if let Err(e) = driver.send_region(x0, y0, x1, y1, &data) {
                            log::error!("send_region failed: {e}");
                        }
                    }
                    DisplayOp::Stop => {
                        log::info!("drain complete, turning panel off");
                        if let Err(e) = driver.screen_off() {
                            log::error!("screen_off failed: {e}");
                        }
                        break;
                    }
                }
            }
            log::info!("display-io thread exiting");
        })
        .expect("cannot spawn display-io thread")
}

/// Capacity: enough for a full frame of tiles without blocking render.
pub const CHANNEL_CAP: usize = 64;

pub fn channel() -> (SyncSender<DisplayOp>, Receiver<DisplayOp>) {
    std::sync::mpsc::sync_channel(CHANNEL_CAP)
}
