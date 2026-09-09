// SPDX-License-Identifier: GPL-3.0-or-later
//! System tray icon (Windows only; other platforms run headless).
//! Ports the `pystray` menu in `main.py`: Configure + Exit.
//! Failures are non-fatal — the daemon keeps running without a tray.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Held for the daemon's lifetime; dropping removes the icon.
pub struct Tray {
    #[cfg(target_os = "windows")]
    _item: tray_item::TrayItem,
}

#[cfg(target_os = "windows")]
fn load_icon(path: &str) -> Option<tray_item::IconSource> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        LoadImageW, IMAGE_ICON, LR_LOADFROMFILE,
    };
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: LoadImageW with LR_LOADFROMFILE reads a file; null instance.
    let handle = unsafe {
        LoadImageW(
            0,
            wide.as_ptr(),
            IMAGE_ICON,
            64,
            64,
            LR_LOADFROMFILE,
        )
    };
    if handle == 0 {
        log::warn!("cannot load tray icon {path}");
        return None;
    }
    // HICON is isize in windows-sys 0.52: the raw handle is already one.
    Some(tray_item::IconSource::RawIcon(handle))
}

/// Show the tray icon. `stopping` is shared with the daemon loop:
/// Exit (and Configure, like Python) requests shutdown.
pub fn show(stopping: Arc<AtomicBool>) -> Option<Tray> {
    #[cfg(not(target_os = "windows"))]
    {
        log::info!("tray icon is not supported on this platform; running headless");
        let _ = stopping;
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        let icon = load_icon("res/icons/monitor-icon-17865/icon.ico")?;
        let mut item = match tray_item::TrayItem::new("Turing System Monitor", icon) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("tray icon unavailable: {e}");
                return None;
            }
        };

        if let Err(e) = item.add_menu_item("Configure", {
            let stopping = stopping.clone();
            move || {
                log::info!("Configure requested from tray; stopping monitor");
                configure_and_stop(&stopping);
            }
        }) {
            log::warn!("tray menu unavailable: {e}");
            return None;
        }
        if let Err(e) = item.add_menu_item("Exit", move || {
            log::info!("Exit requested from tray");
            stopping.store(true, Ordering::SeqCst);
        }) {
            log::warn!("tray menu unavailable: {e}");
            return None;
        }
        log::info!("tray icon shown");
        Some(Tray { _item: item })
    }
}

/// Mirror `main.py:on_configure_tray`: best-effort launch of the Python
/// configure tool, then stop the monitor (it restarts with new settings).
#[cfg(target_os = "windows")]
fn configure_and_stop(stopping: &Arc<AtomicBool>) {
    let candidates: [&str; 2] = ["configure.py", "configure.exe"];
    for c in candidates {
        match std::process::Command::new("python")
            .arg(c)
            .spawn()
            .or_else(|_| std::process::Command::new(c).spawn())
        {
            Ok(_) => break,
            Err(e) => log::debug!("cannot launch {c}: {e}"),
        }
    }
    stopping.store(true, Ordering::SeqCst);
}
