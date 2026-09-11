// SPDX-License-Identifier: GPL-3.0-or-later
//! OS power/session events. Ports the `WM_POWERBROADCAST` / shutdown
//! handling in `main.py`: suspend blanks the panel, resume restores it
//! with a full repaint (some panels lose GRAM over sleep), and session
//! end requests a graceful stop.
//!
//! Windows: hidden message-only window on its own thread. Other platforms:
//! no hook (the daemon loop is unaffected).

#[derive(Debug, Clone, Copy)]
pub enum PowerEvent {
    Suspend,
    Resume,
    Shutdown,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::PowerEvent;
    use std::sync::mpsc::SyncSender;
    use std::sync::OnceLock;
    use std::thread::JoinHandle;
    use windows_sys::Win32::Foundation::HWND;

    static SINK: OnceLock<SyncSender<PowerEvent>> = OnceLock::new();

    // SAFETY: window procedure called by the OS on our message thread;
    // only forwards through the channel (never blocks).
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let send = |ev: PowerEvent| {
            if let Some(tx) = SINK.get() {
                let _ = tx.try_send(ev);
            }
        };
        match msg {
            WM_POWERBROADCAST => {
                match wparam as u32 {
                    PBT_APMSUSPEND => send(PowerEvent::Suspend),
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                        send(PowerEvent::Resume)
                    }
                    _ => {}
                }
                return 1;
            }
            WM_QUERYENDSESSION | WM_ENDSESSION | WM_CLOSE | WM_DESTROY => {
                send(PowerEvent::Shutdown);
                if msg == WM_DESTROY {
                    PostQuitMessage(0);
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    pub fn spawn(tx: SyncSender<PowerEvent>) -> Option<JoinHandle<()>> {
        let _ = SINK.set(tx);
        std::thread::Builder::new()
            .name("power-events".into())
            .spawn(|| unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::*;

                let cls: Vec<u16> = "TssPowerWnd\0".encode_utf16().collect();
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.lpfnWndProc = Some(wndproc);
                wc.lpszClassName = cls.as_ptr();
                if RegisterClassW(&wc) == 0 {
                    log::debug!("power-events: RegisterClass failed");
                    return;
                }
                let hwnd = CreateWindowExW(
                    0,
                    cls.as_ptr(),
                    cls.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    0,
                    0,
                    std::ptr::null(),
                );
                if hwnd == 0 {
                    log::debug!("power-events: CreateWindow failed");
                    return;
                }
                log::info!("power-events listener running");
                let mut m: MSG = std::mem::zeroed();
                while GetMessageW(&mut m, 0, 0, 0) > 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
                log::info!("power-events thread exiting");
            })
            .ok()
    }

    /// Session-0-proof suspend/resume delivery: `PowerRegisterSuspend-
    /// ResumeNotification` with a callback needs no window, so it works
    /// where `WM_POWERBROADCAST` never arrives (services, task session 0).
    /// Returns an opaque handle for `unregister`, or `None` (the time-gap
    /// detector in the daemon covers the rest).
    pub fn register_callback() -> Option<isize> {
        use windows_sys::Win32::System::Power::PowerRegisterSuspendResumeNotification;
        use windows_sys::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;

        // SAFETY: plain callback registration; the routine only try_sends.
        unsafe extern "system" fn on_power(
            _context: *const std::ffi::c_void,
            ty: u32,
            _setting: *const std::ffi::c_void,
        ) -> u32 {
            // PBT_APMSUSPEND = 4, PBT_APMRESUMEAUTOMATIC = 18,
            // PBT_APMRESUMESUSPEND = 7.
            let ev = match ty {
                4 => Some(PowerEvent::Suspend),
                7 | 18 => Some(PowerEvent::Resume),
                _ => None,
            };
            if let (Some(ev), Some(tx)) = (ev, SINK.get()) {
                let _ = tx.try_send(ev);
            }
            0 // ERROR_SUCCESS: we handled it
        }

        // SAFETY: flags valid, callback has the required signature.
        let cb: unsafe extern "system" fn(
            *const std::ffi::c_void,
            u32,
            *const std::ffi::c_void,
        ) -> u32 = on_power;
        let mut handle: *mut std::ffi::c_void = std::ptr::null_mut();
        let err = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                cb as usize as isize,
                &mut handle,
            )
        };
        if err != 0 {
            log::debug!("power callback registration failed (Win32 error {err})");
            return None;
        }
        log::info!("power suspend/resume callback registered");
        Some(handle as isize)
    }

    /// Undo `register_callback` (best effort, at shutdown).
    ///
    /// # Safety
    /// `handle` must come from `register_callback` and be unregistered once.
    pub unsafe fn unregister(handle: isize) {
        use windows_sys::Win32::System::Power::PowerUnregisterSuspendResumeNotification;
        // SAFETY: contract above.
        unsafe {
            PowerUnregisterSuspendResumeNotification(handle);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::PowerEvent;
    use std::sync::mpsc::SyncSender;
    use std::thread::JoinHandle;

    pub fn spawn(_tx: SyncSender<PowerEvent>) -> Option<JoinHandle<()>> {
        log::debug!("power-events: not supported on this platform");
        None
    }

    pub fn _use_event(_e: PowerEvent) {}
}

#[cfg(target_os = "windows")]
pub use imp::{register_callback, spawn, unregister};
#[cfg(not(target_os = "windows"))]
pub use imp::spawn;
