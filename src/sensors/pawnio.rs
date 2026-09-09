// SPDX-License-Identifier: GPL-3.0-or-later
//! PawnIO kernel-driver client. Ports the tiny protocol in LHM's
//! `PawnIo.cs`: open `\\?\GLOBALROOT\Device\PawnIO`, upload a pawn module
//! (`res/drivers/*.bin`, MPL-2.0 LHM resources), call exported functions.
//!
//! The driver is installed once via `external/PawnIO/PawnIO_setup.exe`
//! (run as admin); afterwards the daemon talks to it unelevated.
//! No .NET runtime, no sidecar programs — just syscalls.

#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::ffi::OsStrExt;

    const DEVICE_PATH: &str = r"\\?\GLOBALROOT\Device\PawnIO";
    const IOCTL_LOAD_BINARY: u32 = (41394 << 16) | (0x821 << 2);
    const IOCTL_EXECUTE: u32 = (41394 << 16) | (0x841 << 2);
    const FN_NAME_LEN: usize = 32;

    pub struct PawnIo {
        handle: isize,
    }

    // SAFETY: the handle is an owned kernel object used from one thread
    // (the sensor poller); PawnIo never shares it across threads.
    unsafe impl Send for PawnIo {}

    impl PawnIo {
        /// Open the driver device and upload `module` (pawn bytecode).
        pub fn load_module(module: &[u8]) -> Result<Self, String> {
            use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::Storage::FileSystem::{
                CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
                OPEN_EXISTING,
            };
            use windows_sys::Win32::System::IO::DeviceIoControl;

            let wide: Vec<u16> = std::ffi::OsStr::new(DEVICE_PATH)
                .encode_wide()
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
                // 2/3 = device missing (driver not installed/started);
                // 5 = access denied (open needs elevation).
                return Err(format!(
                    "PawnIO driver not available (CreateFile failed, Win32 error {code}); \
                     install via external/PawnIO/PawnIO_setup.exe as admin, then run this unelevated"
                ));
            }
            let mut returned = 0u32;
            // SAFETY: buffers are alive for the call, sizes exact.
            let ok = unsafe {
                DeviceIoControl(
                    handle,
                    IOCTL_LOAD_BINARY,
                    module.as_ptr() as *const _,
                    module.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                unsafe { CloseHandle(handle) };
                return Err("PawnIO rejected the module binary".to_string());
            }
            Ok(PawnIo { handle })
        }

        /// Call `name(args) -> Vec<i64>` (ASCII name, i64 LE vector ABI).
        pub fn execute(&self, name: &str, args: &[i64], out_len: usize) -> Result<Vec<i64>, String> {
            use windows_sys::Win32::System::IO::DeviceIoControl;

            let mut input = vec![0u8; FN_NAME_LEN + args.len() * 8];
            let name_bytes = name.as_bytes();
            let n = name_bytes.len().min(FN_NAME_LEN - 1);
            input[..n].copy_from_slice(&name_bytes[..n]);
            for (i, a) in args.iter().enumerate() {
                input[FN_NAME_LEN + i * 8..FN_NAME_LEN + (i + 1) * 8]
                    .copy_from_slice(&a.to_le_bytes());
            }
            let mut output = vec![0u8; out_len * 8];
            let mut returned = 0u32;
            // SAFETY: buffers are alive for the call, sizes exact.
            let ok = unsafe {
                DeviceIoControl(
                    self.handle,
                    IOCTL_EXECUTE,
                    input.as_ptr() as *const _,
                    input.len() as u32,
                    output.as_mut_ptr() as *mut _,
                    output.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
                return Err(format!("PawnIO call {name} failed (Win32 error {code})"));
            }
            let count = returned as usize / 8;
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let mut b = [0u8; 8];
                b.copy_from_slice(&output[i * 8..(i + 1) * 8]);
                out.push(i64::from_le_bytes(b));
            }
            Ok(out)
        }

        /// `ioctl_read_msr(index) -> eax|edx<<32`.
        pub fn read_msr(&self, index: u32) -> Option<u64> {
            let out = self.execute("ioctl_read_msr", &[index as i64], 1).ok()?;
            out.first().map(|v| *v as u64)
        }

        /// `ioctl_read_smn(offset) -> u32` (AMD Zen data fabric).
        pub fn read_smn(&self, offset: u32) -> Option<u32> {
            let out = self.execute("ioctl_read_smn", &[offset as i64], 1).ok()?;
            out.first().map(|v| (*v as u64 & 0xFFFF_FFFF) as u32)
        }
    }

    impl Drop for PawnIo {
        fn drop(&mut self) {
            use windows_sys::Win32::Foundation::CloseHandle;
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub struct PawnIo;

    impl PawnIo {
        pub fn load_module(_module: &[u8]) -> Result<Self, String> {
            Err("PawnIO is Windows-only".to_string())
        }

        pub fn execute(&self, _name: &str, _args: &[i64], _out_len: usize) -> Result<Vec<i64>, String> {
            Err("PawnIO is Windows-only".to_string())
        }

        pub fn read_msr(&self, _index: u32) -> Option<u64> {
            None
        }

        pub fn read_smn(&self, _offset: u32) -> Option<u32> {
            None
        }
    }
}

pub use imp::PawnIo;
