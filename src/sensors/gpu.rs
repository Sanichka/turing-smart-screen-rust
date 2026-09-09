// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU sensors. Ports `Gpu`/`GpuNvidia`/`GpuAmd` in `sensors_python.py`.
//!
//! Detection priority (same as Python `Gpu.is_available()`): NVIDIA first,
//! then AMD, else unsupported. Result is cached at startup — no per-poll
//! probing. `fps` is always -1: frame counters are not exposed by vendor
//! libraries (matches Python). Averages across all devices like `GPUtil`.

use super::Snapshot;

/// Cached detection result. The `Nvml` handle is kept alive inside the
/// collector (dropping it unloads the library binding); it is boxed so the
/// enum stays small next to the unit-like variants.
pub enum GpuCollector {
    Nvidia(Box<Nvidia>),
    Amd,
    Unsupported,
}

pub struct Nvidia {
    nvml: nvml_wrapper::Nvml,
}

impl GpuCollector {
    /// Probe once at startup. Logs match Python:
    /// "Detected Nvidia GPU(s)" / "Detected AMD GPU(s)" /
    /// "No supported GPU found".
    pub fn detect() -> Self {
        if let Some(n) = Nvidia::try_init() {
            log::info!("Detected Nvidia GPU(s)");
            return GpuCollector::Nvidia(Box::new(n));
        }
        if amd_available() {
            log::info!("Detected AMD GPU(s)");
            return GpuCollector::Amd;
        }
        log::warn!("No supported GPU found");
        GpuCollector::Unsupported
    }

    pub fn is_available(&self) -> bool {
        !matches!(self, GpuCollector::Unsupported)
    }

    /// Overlay GPU fields onto a snapshot (sysinfo path only; stubs fill
    /// their own values).
    pub fn fill(&self, snap: &mut Snapshot) {
        let (load, mem_pct, used_mb, total_mb, temp) = self.stats();
        snap.gpu_load_pct = load;
        snap.gpu_mem_pct = mem_pct;
        snap.gpu_mem_used_mb = used_mb;
        snap.gpu_mem_total_mb = total_mb;
        snap.gpu_temp_c = temp;
        snap.gpu_fps = self.fps();
        snap.gpu_fan_pct = self.fan_percent();
        snap.gpu_freq_mhz = self.frequency();
    }

    /// load (%) / mem (%) / used (MB) / total (MB) / temp (°C).
    fn stats(&self) -> (f32, f32, f32, f32, f32) {
        match self {
            GpuCollector::Nvidia(n) => n.stats(),
            GpuCollector::Amd => amd_stats(),
            GpuCollector::Unsupported => (f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN),
        }
    }

    fn fps(&self) -> i32 {
        // Not exposed by vendor libraries (Python returns -1 too).
        -1
    }

    fn fan_percent(&self) -> f32 {
        match self {
            GpuCollector::Nvidia(n) => n.fan_percent(),
            GpuCollector::Amd => amd_fan_percent(),
            GpuCollector::Unsupported => f32::NAN,
        }
    }

    fn frequency(&self) -> f32 {
        match self {
            GpuCollector::Nvidia(n) => n.frequency(),
            GpuCollector::Amd => amd_frequency(),
            GpuCollector::Unsupported => f32::NAN,
        }
    }
}

impl Nvidia {
    fn try_init() -> Option<Self> {
        let nvml = nvml_wrapper::Nvml::init().ok()?;
        // Force device enumeration now so `is_available` is truthful.
        let count = nvml.device_count().unwrap_or(0);
        if count == 0 {
            return None;
        }
        // Touch device 0 to catch headless/driver-stub setups.
        let _ = nvml.device_by_index(0).ok()?;
        Some(Nvidia { nvml })
    }

    fn for_each_device(&self, mut f: impl FnMut(nvml_wrapper::Device)) {
        let count = self.nvml.device_count().unwrap_or(0);
        for i in 0..count {
            if let Ok(d) = self.nvml.device_by_index(i) {
                f(d);
            }
        }
    }

    fn stats(&self) -> (f32, f32, f32, f32, f32) {
        let mut load_sum = 0f64;
        let mut used_sum = 0u64;
        let mut total_sum = 0u64;
        let mut temp_sum = 0u64;
        let mut n = 0u64;
        self.for_each_device(|d| {
            n += 1;
            if let Ok(u) = d.utilization_rates() {
                load_sum += u.gpu as f64;
            }
            if let Ok(m) = d.memory_info() {
                used_sum += m.used;
                total_sum += m.total;
            }
            if let Ok(t) = d.temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu) {
                temp_sum += t as u64;
            }
        });
        if n == 0 {
            return (f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN);
        }
        let load = load_sum / n as f64;
        let used_mb = used_sum as f64 / n as f64 / 1024.0 / 1024.0;
        let total_mb = total_sum as f64 / n as f64 / 1024.0 / 1024.0;
        let mem_pct = if total_mb > 0.0 {
            used_mb / total_mb * 100.0
        } else {
            f64::NAN
        };
        let temp = if temp_sum > 0 {
            temp_sum as f64 / n as f64
        } else {
            f64::NAN
        };
        (load as f32, mem_pct as f32, used_mb as f32, total_mb as f32, temp as f32)
    }

    fn fan_percent(&self) -> f32 {
        let mut sum = 0u64;
        let mut n = 0u64;
        self.for_each_device(|d| {
            if let Ok(f) = d.fan_speed(0) {
                sum += f as u64;
                n += 1;
            }
        });
        if n == 0 {
            f32::NAN
        } else {
            sum as f32 / n as f32
        }
    }

    fn frequency(&self) -> f32 {
        // Graphics clock of device 0 (per-device averaging is meaningless
        // for clocks; Python leaves this unsupported for NVIDIA anyway).
        self.nvml
            .device_by_index(0)
            .ok()
            .and_then(|d| {
                d.clock_info(nvml_wrapper::enum_wrappers::device::Clock::Graphics).ok()
            })
            .map(|mhz| mhz as f32)
            .unwrap_or(f32::NAN)
    }
}

// ---------------------------------------------------------------------------
// AMD via Linux sysfs (`/sys/class/drm/card*/device`). Windows AMD without
// LHM reports NaN (full LHM coverage is a later step).
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn amd_available() -> bool {
    each_amd_device(|_| {}).is_some()
}

#[cfg(not(target_os = "linux"))]
fn amd_available() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn amd_stats() -> (f32, f32, f32, f32, f32) {
    let mut load_sum = 0f64;
    let mut used_sum = 0u64;
    let mut total_sum = 0u64;
    let mut temp_sum = 0i64;
    let mut temp_n = 0u64;
    let mut n = 0u64;
    if each_amd_device(|dev| {
        n += 1;
        if let Some(v) = read_u64(dev.join("gpu_busy_percent")) {
            load_sum += v.min(100) as f64;
        }
        if let Some(u) = read_u64(dev.join("mem_info_vram_used")) {
            used_sum += u;
        }
        if let Some(t) = read_u64(dev.join("mem_info_vram_total")) {
            total_sum += t;
        }
        if let Some(t) = amd_temp_millideg(&dev) {
            temp_sum += t;
            temp_n += 1;
        }
    })
    .is_none()
    {
        return (f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN);
    }
    let load = if n > 0 { load_sum / n as f64 } else { f64::NAN };
    let used_mb = used_sum as f64 / n.max(1) as f64 / 1024.0 / 1024.0;
    let total_mb = total_sum as f64 / n.max(1) as f64 / 1024.0 / 1024.0;
    let mem_pct = if total_mb > 0.0 {
        used_mb / total_mb * 100.0
    } else {
        f64::NAN
    };
    let temp = if temp_n > 0 {
        temp_sum as f64 / temp_n as f64 / 1000.0
    } else {
        f64::NAN
    };
    (load as f32, mem_pct as f32, used_mb as f32, total_mb as f32, temp as f32)
}

#[cfg(not(target_os = "linux"))]
fn amd_stats() -> (f32, f32, f32, f32, f32) {
    (f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN)
}

#[cfg(target_os = "linux")]
fn amd_fan_percent() -> f32 {
    // Reuse the hwmon scan with a "gpu" label filter (mirrors Python's
    // `GpuNvidia.fan_percent` psutil fallback).
    use std::fs;
    let mut found = false;
    let mut pct = f32::NAN;
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let dir = entry.path();
            let chip = fs::read_to_string(dir.join("name"))
                .map(|s| s.trim().to_ascii_lowercase())
                .unwrap_or_default();
            let Ok(rd) = fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.starts_with("fan") || !name.ends_with("_input") {
                    continue;
                }
                let stem = name.trim_end_matches("_input");
                let label = fs::read_to_string(dir.join(format!("{stem}_label")))
                    .map(|s| s.trim().to_ascii_lowercase())
                    .unwrap_or_default();
                if !label.contains("gpu") && !chip.contains("gpu") && !chip.contains("amdgpu") {
                    continue;
                }
                let cur: Option<u64> = fs::read_to_string(e.path())
                    .ok()
                    .and_then(|s| s.trim().parse().ok());
                let max: u64 = fs::read_to_string(dir.join(format!("{stem}_max")))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(3000);
                if let Some(c) = cur {
                    if max > 0 {
                        pct = (c.min(max) as f32 / max as f32 * 100.0).clamp(0.0, 100.0);
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
    }
    pct
}

#[cfg(not(target_os = "linux"))]
fn amd_fan_percent() -> f32 {
    f32::NAN
}

#[cfg(target_os = "linux")]
fn amd_frequency() -> f32 {
    // Active `pp_dpm_sclk` entry (marked with `*`), first card.
    let mut out = f32::NAN;
    each_amd_device(|dev| {
        if out.is_nan() {
            if let Ok(text) = std::fs::read_to_string(dev.join("pp_dpm_sclk")) {
                for line in text.lines() {
                    if line.contains('*') {
                        // e.g. "1: 1500Mhz *"
                        let mhz: String =
                            line.chars().filter(|c| c.is_ascii_digit()).collect();
                        if let Ok(v) = mhz.parse::<f32>() {
                            out = v;
                        }
                        break;
                    }
                }
            }
        }
    });
    out
}

#[cfg(not(target_os = "linux"))]
fn amd_frequency() -> f32 {
    f32::NAN
}

#[cfg(target_os = "linux")]
fn each_amd_device(mut f: impl FnMut(std::path::PathBuf)) -> Option<()> {
    let drm = std::fs::read_dir("/sys/class/drm").ok()?;
    let mut found = false;
    for entry in drm.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue; // skip card0-HDMI-A-1 connectors, keep card0/card1
        }
        let dev = entry.path().join("device");
        if dev.join("gpu_busy_percent").exists() {
            found = true;
            f(dev);
        }
    }
    if found { Some(()) } else { None }
}

#[cfg(target_os = "linux")]
fn read_u64(p: std::path::PathBuf) -> Option<u64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn amd_temp_millideg(dev: &std::path::Path) -> Option<i64> {
    let hwmon = dev.join("hwmon");
    let entries = std::fs::read_dir(hwmon).ok()?;
    for e in entries.flatten() {
        for i in 1..=4 {
            let p = e.path().join(format!("temp{i}_input"));
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(v) = s.trim().parse::<i64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}
