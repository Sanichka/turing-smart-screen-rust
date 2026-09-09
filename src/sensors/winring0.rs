// SPDX-License-Identifier: GPL-3.0-or-later
//! WinRing0 (OLS) raw port I/O client. Used for SuperIO PNP probing and
//! runtime tachometer reads where PawnIO's BAR-gated `pio` calls cannot go
//! (e.g. AMD boards). Protocol from OHM `OlsIoctl.h`:
//! `CTL_CODE(40000, fn, METHOD_BUFFERED, access)`.

#[cfg(target_os = "windows")]
mod imp {
    const DEVICE_PATH: &str = r"\\.\WinRing0_1_2_0";
    // CTL_CODE(40000, fn, METHOD_BUFFERED, access): access READ=1, WRITE=2.
    const IOCTL_READ_IO_PORT_BYTE: u32 = (40000 << 16) | (1 << 14) | (0x833 << 2);
    const IOCTL_WRITE_IO_PORT_BYTE: u32 = (40000 << 16) | (2 << 14) | (0x836 << 2);

    pub struct WinRing0 {
        handle: isize,
    }

    // SAFETY: owned handle, used from the single sensor thread.
    unsafe impl Send for WinRing0 {}

    impl WinRing0 {
        /// Open the driver device (demand-starts a manual service).
        pub fn open() -> Result<Self, String> {
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::Storage::FileSystem::{
                CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
                OPEN_EXISTING,
            };
            let wide: Vec<u16> = DEVICE_PATH
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: plain Win32 open with valid params.
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    0xC000_0000, // GENERIC_READ | GENERIC_WRITE
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    0,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
                return Err(format!("WinRing0 open failed (Win32 error {code})"));
            }
            Ok(WinRing0 { handle })
        }

        fn ioctl(&self, code: u32, input: &[u8], out_len: usize) -> Result<Vec<u8>, String> {
            use windows_sys::Win32::System::IO::DeviceIoControl;
            let mut output = vec![0u8; out_len];
            let mut returned = 0u32;
            // SAFETY: buffers alive for the call, sizes exact.
            let ok = unsafe {
                DeviceIoControl(
                    self.handle,
                    code,
                    if input.is_empty() { std::ptr::null() } else { input.as_ptr() as *const _ },
                    input.len() as u32,
                    if output.is_empty() {
                        std::ptr::null_mut()
                    } else {
                        output.as_mut_ptr() as *mut _
                    },
                    output.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
                return Err(format!("WinRing0 ioctl {code:#X} failed (Win32 error {err})"));
            }
            output.truncate(returned as usize);
            Ok(output)
        }

        pub fn read_io_byte(&self, port: u16) -> Option<u8> {
            let mut input = [0u8; 4];
            input.copy_from_slice(&(port as u32).to_le_bytes());
            let out = self.ioctl(IOCTL_READ_IO_PORT_BYTE, &input, 4).ok()?;
            out.first().copied()
        }

        pub fn write_io_byte(&self, port: u16, val: u8) {
            let mut input = [0u8; 8];
            input[..4].copy_from_slice(&(port as u32).to_le_bytes());
            input[4] = val;
            let _ = self.ioctl(IOCTL_WRITE_IO_PORT_BYTE, &input, 0);
        }
    }

    impl Drop for WinRing0 {
        fn drop(&mut self) {
            use windows_sys::Win32::Foundation::CloseHandle;
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub struct WinRing0;

    impl WinRing0 {
        pub fn open() -> Result<Self, String> {
            Err("WinRing0 is Windows-only".to_string())
        }

        pub fn read_io_byte(&self, _port: u16) -> Option<u8> {
            None
        }

        pub fn write_io_byte(&self, _port: u16, _val: u8) {}
    }
}

pub use imp::WinRing0;
