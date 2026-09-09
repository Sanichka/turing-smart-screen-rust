// SPDX-License-Identifier: GPL-3.0-or-later
//! System tray icon (Windows only; other platforms run headless).
//! Ports the `pystray` menu in `main.py`: Configure + Exit.
//! Failures are non-fatal — the daemon keeps running without a tray.

use std::sync::{
    atomic::AtomicBool,
    Arc,
};
#[cfg(target_os = "windows")]
use std::{path::PathBuf, sync::atomic::Ordering};

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
    if !interactive_desktop() {
        log::info!("no interactive desktop (service session?); running without tray icon");
        let _ = stopping;
        return None;
    }

    #[cfg(not(target_os = "windows"))]
    {
        log::info!("tray icon is not supported on this platform; running headless");
        let _ = stopping;
        None
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

/// True when the process can show UI: fails in session 0 / service
/// contexts where a message pump would spin against a missing desktop.
/// (tray-item creates a visible-style window; without a compositor its
/// pump can burn a whole core dispatching junk.)
#[cfg(target_os = "windows")]
fn interactive_desktop() -> bool {
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
    };
    // SAFETY: plain queries with valid buffers.
    unsafe {
        let station = GetProcessWindowStation();
        if station == 0 {
            return false;
        }
        let mut flags: USEROBJECTFLAGS = std::mem::zeroed();
        let mut needed = 0u32;
        if GetUserObjectInformationW(
            station,
            UOI_FLAGS,
            &mut flags as *mut _ as *mut _,
            std::mem::size_of::<USEROBJECTFLAGS>() as u32,
            &mut needed,
        ) == 0
        {
            return false;
        }
        // WSF_VISIBLE = 0x0001.
        (flags.dwFlags & 0x0001) != 0
    }
}

#[cfg(not(target_os = "windows"))]
fn interactive_desktop() -> bool {
    true
}

/// Mirror `main.py:on_configure_tray`: open the configuration tool, then
/// stop the monitor (its Save&Run starts a fresh daemon with new settings).
/// Preference: native `turing-configure` next to this exe; fallback to
/// `configure.py` via python after verifying its imports (a bare `spawn`
/// succeeds even when the script immediately dies on missing deps).
/// If nothing usable launches, the monitor keeps running.
#[cfg(target_os = "windows")]
fn configure_and_stop(stopping: &Arc<AtomicBool>) {
    if launch_native_configure() {
        stopping.store(true, Ordering::SeqCst);
        return;
    }
    if launch_python_configure() {
        stopping.store(true, Ordering::SeqCst);
        return;
    }
    log::error!("no configuration tool available (turing-configure.exe not found, python lacks deps); monitor keeps running");
}

/// Native tool beside the running exe (release/debug dirs, PATH fallback).
#[cfg(target_os = "windows")]
fn launch_native_configure() -> bool {
    let candidates = [
        sibling_exe("turing-configure"),
        PathBuf::from("turing-configure.exe"),
        PathBuf::from("turing-configure"),
    ];
    for c in candidates {
        match std::process::Command::new(&c).spawn() {
            Ok(_) => {
                log::info!("launched {}", c.display());
                return true;
            }
            Err(e) => log::debug!("cannot launch {}: {e}", c.display()),
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn sibling_exe(stem: &str) -> PathBuf {
    let exe = format!("{stem}.exe");
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(&exe)))
        .unwrap_or_else(|| PathBuf::from(&exe))
}

/// Python fallback only if the interpreter can actually import the
/// configure stack (babel/Pillow/pyserial are the usual missing pieces).
#[cfg(target_os = "windows")]
fn launch_python_configure() -> bool {
    use std::time::Duration;
    if !PathBuf::from("configure.py").is_file() {
        log::debug!("configure.py not in working dir");
        return false;
    }
    let check = std::process::Command::new("python")
        .args(["-c", "import babel,PIL,serial,yaml,psutil"])
        .output();
    match check {
        Ok(o) if o.status.success() => match std::process::Command::new("python")
            .arg("configure.py")
            .spawn()
        {
            Ok(_) => {
                log::info!("launched configure.py; stopping monitor");
                true
            }
            Err(e) => {
                log::debug!("cannot launch configure.py: {e}");
                false
            }
        },
        _ => {
            log::debug!("python interpreter missing configure deps");
            // Brief grace period so a half-working python's stderr (import
            // errors) is visible in the daemon console before we continue.
            std::thread::sleep(Duration::from_millis(200));
            false
        }
    }
}
