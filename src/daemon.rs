// SPDX-License-Identifier: GPL-3.0-or-later
//! Daemon: init → static layer → render/dispatch threads → clean shutdown.
//! Mirrors `main.py` init order and `scheduler.py` staggering, minus per-
//! metric threads: one fast tick (sysinfo+GPU+render, 1Hz) and one slow
//! thread (ping/weather) keep the footprint at 3 threads + serial I/O.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, Theme};
use crate::display::{self, DisplayOp, Orientation};
use crate::render::theme_widgets::{interval_secs, path};
use crate::render::{ImageCache, Renderer};
use crate::sensors::{self, NetSelection, Provider, SlowCtx};

/// Diff granularity for incremental updates (device pixels).
const TILE_W: u32 = 64;
const TILE_H: u32 = 32;

/// Ensures a single daemon: a second copy exits instead of fighting over
/// the COM port (open-fail/reopen storms burn CPU and tear the display).
/// Windows: named mutex. Elsewhere: advisory lock file next to the log.
fn single_instance_guard() -> Result<InstanceGuard, String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        const ERROR_ALREADY_EXISTS: u32 = 183;
        let name: Vec<u16> = "TuringSmartScreenDaemon\0".encode_utf16().collect();
        // SAFETY: plain Win32 call with valid params.
        let handle: HANDLE = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle == 0 {
            return Err("cannot create instance mutex".to_string());
        }
        // SAFETY: GetLastError immediately after the call.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return Err("another daemon instance is already running".to_string());
        }
        Ok(InstanceGuard::Windows(handle))
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::fs::OpenOptions;
        // Exclusive create: fails when the lock file already exists.
        match OpenOptions::new().write(true).create_new(true).open(".tss-daemon.lock") {
            Ok(f) => Ok(InstanceGuard::File(f)),
            Err(_) => Err("another daemon instance is already running".to_string()),
        }
    }
}

enum InstanceGuard {
    #[cfg(target_os = "windows")]
    Windows(isize),
    #[cfg(not(target_os = "windows"))]
    File(std::fs::File),
}

#[cfg(target_os = "windows")]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let InstanceGuard::Windows(h) = *self;
        // SAFETY: we own the handle.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    }
}

#[cfg(not(target_os = "windows"))]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(".tss-daemon.lock");
    }
}

pub struct DaemonArgs {
    pub com_override: Option<String>,
    pub tick: Duration,
    /// `--send-test`: paint static + one frame, leave the image on screen.
    pub test_once: bool,
    /// `--no-tray`: skip the tray icon (service/headless use).
    pub no_tray: bool,
}

fn display_orientation(t: &Theme) -> Orientation {
    match t.orientation {
        crate::config::Orientation::Portrait => Orientation::Portrait,
        crate::config::Orientation::ReversePortrait => Orientation::ReversePortrait,
        crate::config::Orientation::Landscape => Orientation::Landscape,
        crate::config::Orientation::ReverseLandscape => Orientation::ReverseLandscape,
    }
}

fn stats_interval(theme: &Theme, keys: &[&str]) -> f32 {
    theme
        .raw
        .get("STATS")
        .and_then(|s| path(s, keys))
        .map(interval_secs)
        .unwrap_or(0.0)
}

fn send_rect(
    tx: &std::sync::mpsc::SyncSender<DisplayOp>,
    r: &crate::render::framebuf::Rect,
    fb: &crate::render::framebuf::Framebuf,
) -> bool {
    // One small alloc per region (widget-scale, ~1Hz): keeps the hot path
    // borrow-simple; the bounded channel provides backpressure instead.
    let mut data = Vec::new();
    fb.region_rgb565le(r, &mut data);
    tx.send(DisplayOp::Region {
        x0: r.x0,
        y0: r.y0,
        x1: r.x1,
        y1: r.y1,
        data,
    })
    .is_ok()
}

pub fn run(cfg: &AppConfig, theme: &Theme, args: &DaemonArgs) -> i32 {
    let _instance = match single_instance_guard() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: another daemon is already running (check Task Manager / tray icon)");
            return 2;
        }
    };
    let com = args
        .com_override
        .as_deref()
        .unwrap_or(&cfg.general.com_port);
    log::info!("opening display (revision {:?}) on {com}", cfg.display.revision);

    let mut driver = match display::create(cfg.display.revision, com, theme.width, theme.height) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    log::info!("driver: {}", driver.name());

    // -- init sequence (mirrors Display.initialize_display) ----------------
    if cfg.display.reset_on_startup {
        if let Err(e) = driver.reset() {
            eprintln!("error: reset failed: {e}");
            return 1;
        }
    }
    if let Err(e) = driver.initialize() {
        eprintln!("error: initialize failed: {e}");
        return 1;
    }
    // Native dims are only known after initialize (HELLO probe).
    let (nw, nh) = driver.native_dims();
    if nw != theme.width || nh != theme.height {
        log::warn!(
            "theme is {}x{} but display reports {nw}x{nh}; regions will be cropped",
            theme.width, theme.height
        );
    }
    if let Err(e) = driver
        .screen_on()
        .and_then(|_| driver.set_brightness(cfg.display.brightness))
        .and_then(|_| driver.set_orientation(display_orientation(theme)))
    {
        eprintln!("error: display setup failed: {e}");
        return 1;
    }
    // Backplate LED: Rev A has none; theme key is honored where supported.
    log::debug!(
        "theme LED color: {:?}",
        theme.display_rgb_led
    );

    // -- static layer -------------------------------------------------------
    let mut renderer = Renderer::new(theme.width, theme.height, std::path::PathBuf::from("res/fonts"));
    let imgs = ImageCache::preload(theme);
    renderer.draw_static(theme, &imgs);

    let (tx, rx) = display::channel();
    let io = display::spawn_dispatcher(driver, rx);
    let fb = &renderer.fb;
    let rects = std::mem::take(&mut renderer.dirty);
    log::info!("sending static layer ({} rects)...", rects.len());
    for r in &rects {
        if !send_rect(&tx, r, fb) {
            eprintln!("error: display-io thread died during static send");
            return 1;
        }
    }

    // -- first snapshot -----------------------------------------------------
    let nets = NetSelection {
        eth: cfg.general.eth.clone(),
        wlo: cfg.general.wlo.clone(),
    };
    let slow_ctx = SlowCtx::from_config(&cfg.general);
    let mut provider = Provider::from_hw(cfg.general.hw_sensors, &cfg.general.cpu_fan);
    log::info!("gpu_available={}", provider.gpu_available());
    let slow = sensors::fetch_slow(&slow_ctx);
    let mut snap = provider.snapshot_fast(&nets);
    sensors::apply_slow(&mut snap, &slow);
    renderer.draw_snapshot(theme, &imgs, &snap);
    let rects = std::mem::take(&mut renderer.dirty);
    log::info!("sending first frame ({} rects)...", rects.len());
    for r in &rects {
        if !send_rect(&tx, r, &renderer.fb) {
            eprintln!("error: display-io thread died during first frame");
            return 1;
        }
    }

    if args.test_once {
        // Leave the image on screen; do NOT send Stop (which blanks it).
        drop(tx);
        let _ = io.join();
        println!("test frame sent; panel left on ({} rects total)", rects.len());
        return 0;
    }

    // -- steady state: tile-diffed incremental updates ----------------------
    let stopping = Arc::new(AtomicBool::new(false));
    {
        let s = stopping.clone();
        if let Err(e) = ctrlc::set_handler(move || {
            log::info!("interrupt received, draining...");
            s.store(true, Ordering::SeqCst);
        }) {
            eprintln!("error: cannot install signal handler: {e}");
            return 1;
        }
    }
    // Tray icon (best effort, kept alive until shutdown; skipped for
    // headless/service contexts and with --no-tray).
    let _tray = if args.no_tray {
        log::info!("tray icon disabled by --no-tray");
        None
    } else {
        crate::tray::show(stopping.clone())
    };
    // Simulated display web preview (Python parity: always on in SIMU).
    let simu_web = if cfg.display.revision == crate::config::Revision::Simu {
        display::simuserve::spawn(stopping.clone())
    } else {
        None
    };
    let slow_shared = Arc::new(Mutex::new(slow));
    let ping_iv = stats_interval(theme, &["PING"]);
    let wx_iv = {
        let iv = stats_interval(theme, &["WEATHER"]);
        if slow_ctx.weather.api_key.is_empty() {
            0.0
        } else {
            iv.max(300.0)
        }
    };
    let slow_thread = {
        let stopping = stopping.clone();
        let slow_shared = slow_shared.clone();
        std::thread::Builder::new()
            .name("slow-sensors".into())
            .spawn(move || {
                let mut last_ping = Instant::now();
                let mut last_wx = Instant::now();
                // Initial fetch already done; refresh on schedule.
                while !stopping.load(Ordering::SeqCst) {
                    let now = Instant::now();
                    if ping_iv > 0.0 && now.duration_since(last_ping).as_secs_f32() >= ping_iv {
                        last_ping = now;
                        let ms = sensors::ping::ping_once(&slow_ctx.ping_dest);
                        slow_shared.lock().unwrap().ping_ms = ms;
                    }
                    if wx_iv > 0.0 && now.duration_since(last_wx).as_secs_f32() >= wx_iv {
                        last_wx = now;
                        let w = sensors::weather::fetch(&slow_ctx.weather);
                        let mut s = slow_shared.lock().unwrap();
                        s.wx_temp = w.temp;
                        s.wx_felt = w.felt;
                        s.wx_description = w.description;
                        s.wx_humidity = w.humidity;
                        s.wx_update = w.update;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                log::info!("slow-sensors thread exiting");
            })
            .expect("cannot spawn slow-sensors thread")
    };

    let mut last_sent = renderer.fb.pixels().to_vec();
    let (power_tx, power_rx) = std::sync::mpsc::sync_channel::<crate::power::PowerEvent>(8);
    // Not joined at shutdown: the message loop has no quit source and the
    // process exit reaps it (same as Python's daemon threads).
    let _power_thread = crate::power::spawn(power_tx);
    let mut force_full = false;
    log::info!("entering render loop ({:?} tick)", args.tick);
    while !stopping.load(Ordering::SeqCst) {
        let t0 = Instant::now();
        // OS power/session events (suspend blanks, resume repaints).
        while let Ok(ev) = power_rx.try_recv() {
            match ev {
                crate::power::PowerEvent::Suspend => {
                    log::info!("system suspending: panel off");
                    let _ = tx.send(DisplayOp::ScreenOff);
                }
                crate::power::PowerEvent::Resume => {
                    log::info!("system resumed: panel on + full repaint");
                    let _ = tx.send(DisplayOp::ScreenOn);
                    force_full = true;
                }
                crate::power::PowerEvent::Shutdown => {
                    log::info!("session ending: stopping");
                    stopping.store(true, Ordering::SeqCst);
                }
            }
        }
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        let mut snap = provider.snapshot_fast(&nets);
        sensors::apply_slow(&mut snap, &slow_shared.lock().unwrap());
        renderer.draw_snapshot(theme, &imgs, &snap);
        renderer.dirty.clear(); // authoritative source is the tile diff
        let tiles = if force_full {
            force_full = false;
            vec![crate::render::framebuf::Rect::new(
                0,
                0,
                renderer.fb.w as i32,
                renderer.fb.h as i32,
            )]
        } else {
            renderer.fb.changed_tiles(&last_sent, TILE_W, TILE_H)
        };
        if !tiles.is_empty() {
            log::debug!("{} changed tiles", tiles.len());
        }
        for t in &tiles {
            if !send_rect(&tx, t, &renderer.fb) {
                log::error!("display-io thread died; stopping");
                stopping.store(true, Ordering::SeqCst);
                break;
            }
        }
        last_sent.copy_from_slice(renderer.fb.pixels());
        let elapsed = t0.elapsed();
        if elapsed < args.tick {
            // Interruptible sleep so Ctrl-C reacts within ~50ms.
            let mut slept = Duration::ZERO;
            while slept < args.tick - elapsed && !stopping.load(Ordering::SeqCst) {
                let step = Duration::from_millis(50).min(args.tick - elapsed - slept);
                std::thread::sleep(step);
                slept += step;
            }
        }
    }

    // -- graceful drain: Stop is processed after all queued regions --------
    let _ = slow_thread.join();
    log::info!("waiting for display queue to drain...");
    match tx.send(DisplayOp::Stop) {
        Ok(()) => log::info!("drain marker sent"),
        Err(_) => log::warn!("display-io thread already gone"),
    }
    drop(tx);
    let _ = io.join();
    if let Some(h) = simu_web {
        let _ = h.join();
    }
    log::info!("clean stop");
    0
}
