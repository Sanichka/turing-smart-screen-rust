// SPDX-License-Identifier: GPL-3.0-or-later
//! Live CPU clock on Windows, Task Manager style.
//!
//! Task Manager's "Speed" is `base clock × % Processor Performance`
//! (APERF/MPERF based, boost-aware). Neither sysinfo 0.32 (per-CPU frequency
//! is read once per process behind its `got_cpu_frequency` gate) nor
//! psutil/WMI (`CurrentClockSpeed` is static on this hardware) tracks boost,
//! so poll the PDH counter
//! `\Processor Information(_Total)\% Processor Performance` directly.
//!
//! The query object is persistent: PDH cooked values need two collections, so
//! the constructor warms up once and `poll_mhz` enforces a minimum spacing.
//! Single-shot callers (`--sensors-once`) get `None` and fall back to the
//! static clock; the 1 Hz daemon always has a valid sample after its first
//! tick.

use std::time::{Duration, Instant};

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE,
    PDH_FMT_DOUBLE,
};
use windows_sys::Win32::System::Power::{
    CallNtPowerInformation, PROCESSOR_POWER_INFORMATION, ProcessorInformation,
};

/// PDH cooked counters need two collections; below this spacing a sample is
/// meaningless (zero-interval reads fail), so report "no sample yet".
const MIN_SAMPLE: Duration = Duration::from_millis(250);

fn processor_power_infos() -> Option<Vec<PROCESSOR_POWER_INFORMATION>> {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let blank = PROCESSOR_POWER_INFORMATION {
        Number: 0,
        MaxMhz: 0,
        CurrentMhz: 0,
        MhzLimit: 0,
        MaxIdleState: 0,
        CurrentIdleState: 0,
    };
    let mut infos = vec![blank; n];
    // SAFETY: output buffer holds `n` entries; the kernel fills at most one
    // per active processor. Trailing zeroed entries (if any) are skipped by
    // the >0 filters below.
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            std::ptr::null(),
            0,
            infos.as_mut_ptr() as *mut core::ffi::c_void,
            (n * std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()) as u32,
        )
    };
    if status != 0 {
        return None;
    }
    Some(infos)
}

/// Max `CurrentMhz` across logical processors, or `None` on failure.
/// (Live on most hardware, but static on some AMD/Windows combos — hence the
/// PDH-based `CpuSpeed` below.)
pub fn live_cpu_freq_mhz() -> Option<f32> {
    processor_power_infos()?
        .iter()
        .map(|i| i.CurrentMhz)
        .filter(|m| *m > 0)
        .max()
        .map(|m| m as f32)
}

/// Max `MaxMhz` (base clock) across logical processors, or `None`.
pub fn cpu_base_mhz() -> Option<f32> {
    processor_power_infos()?
        .iter()
        .map(|i| i.MaxMhz)
        .filter(|m| *m > 0)
        .max()
        .map(|m| m as f32)
}

/// Persistent `% Processor Performance` query; speed = base × perf/100.
pub struct CpuSpeed {
    query: isize,
    counter: isize,
    base_mhz: f32,
    last_collect: Instant,
}

impl CpuSpeed {
    pub fn new() -> Option<Self> {
        let base_mhz = cpu_base_mhz().filter(|m| *m > 0.0)?;
        unsafe {
            let mut query: isize = 0;
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let path: Vec<u16> = "\\Processor Information(_Total)\\% Processor Performance"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut counter: isize = 0;
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            // Warmup collection; the next one (>= MIN_SAMPLE later) yields
            // the first valid cooked value.
            if PdhCollectQueryData(query) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            Some(CpuSpeed {
                query,
                counter,
                base_mhz,
                last_collect: Instant::now(),
            })
        }
    }

    /// Task Manager style speed in MHz, or `None` when no valid sample exists
    /// yet (first tick) or the read fails (caller falls back).
    pub fn poll_mhz(&mut self) -> Option<f32> {
        if self.last_collect.elapsed() < MIN_SAMPLE {
            return None;
        }
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let mut value: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
            if PdhGetFormattedCounterValue(
                self.counter,
                PDH_FMT_DOUBLE,
                std::ptr::null_mut(),
                &mut value,
            ) != 0
            {
                return None;
            }
            // PDH_CSTATUS_VALID_DATA == 0; anything else (e.g. a
            // zero-interval sample) is not a real measurement.
            if value.CStatus != 0 {
                return None;
            }
            let perf = value.Anonymous.doubleValue;
            self.last_collect = Instant::now();
            if !perf.is_finite() || perf < 0.0 {
                return None;
            }
            Some(self.base_mhz * perf as f32 / 100.0)
        }
    }
}

impl Drop for CpuSpeed {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn base_clock_is_sane() {
        let Some(base) = super::cpu_base_mhz() else {
            // Exotic Windows without processor power info: nothing to check.
            return;
        };
        assert!(base > 100.0, "implausible base clock: {base}");
    }

    /// Exercises the full PDH path (warmup + spaced poll). Needs ~300 ms.
    #[test]
    fn processor_performance_poll() {
        let Some(mut speed) = super::CpuSpeed::new() else {
            // Counter missing (some VMs): nothing to check.
            return;
        };
        std::thread::sleep(super::MIN_SAMPLE + Duration::from_millis(50));
        let Some(mhz) = speed.poll_mhz() else {
            // Valid setup but no sample yet: acceptable, no panic is the test.
            return;
        };
        assert!(
            (100.0..=20_000.0).contains(&mhz),
            "implausible CPU speed: {mhz}"
        );
    }
}
