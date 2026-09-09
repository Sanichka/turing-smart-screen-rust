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

pub use imp::spawn;
