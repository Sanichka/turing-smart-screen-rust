// SPDX-License-Identifier: GPL-3.0-or-later
//! `sysinfo`-backed collector. Ports `sensors_python.py` (psutil).
//!
//! One `SysinfoCollector` per process: it owns `System` + `Networks` +
//! previous net counters so polls reuse allocations. GPU/LHM in Step 3.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sysinfo::{Components, Disks, Networks, System};

use super::{NetIfStats, NetSelection, Snapshot};

/// Minimum sample window for CPU usage (sysinfo needs two refreshes;
/// mirrors `psutil.cpu_percent(interval=...)` blocking semantics).
const CPU_SAMPLE: Duration = Duration::from_millis(300);

pub struct SysinfoCollector {
    sys: System,
    networks: Networks,
    disks: Disks,
    components: Components,
    prev_net: HashMap<String, (u64, u64, Instant)>,
    cpu_fan: String,
}

impl SysinfoCollector {
    pub fn new() -> Self {
        Self::with_cpu_fan("AUTO".to_string())
    }

    pub fn with_cpu_fan(cpu_fan: String) -> Self {
        // NOTE: `System::new()` (not `new_all()`): we never touch the process
        // list, and enumerating hundreds of processes costs MBs of resident
        // memory for nothing. CPUs still need one full populate up front.
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_cpu_usage();
        Self {
            sys,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            prev_net: HashMap::new(),
            cpu_fan,
        }
    }

    pub fn snapshot(&mut self, nets: &NetSelection) -> Snapshot {
        // -- CPU percent needs two samples --------------------------------
        self.sys.refresh_cpu_usage();
        std::thread::sleep(CPU_SAMPLE);
        self.sys.refresh_memory();
        self.sys.refresh_cpu_usage();

        let cpu_percent = self.sys.global_cpu_usage();
        let cpu_freq_mhz = self
            .sys
            .cpus()
            .iter()
            .map(|c| c.frequency())
            .max()
            .unwrap_or(0) as f32;

        let load = System::load_average();
        // sysinfo returns 0.0 on platforms without loadavg (Windows):
        // surface as NaN like Python's `(nan, nan, nan)` fallback.
        let (l1, l5, l15) = (
            norm_load(load.one),
            norm_load(load.five),
            norm_load(load.fifteen),
        );

        let cpu_temp_c = {
            self.components.refresh();
            cpu_temperature(&self.components)
        };
        let cpu_fan_percent = hwmon_fan_percent(&self.cpu_fan);

        // -- Memory (mirrors psutil comment: used = total - available) ----
        let mem_total = self.sys.total_memory();
        let mem_avail = self.sys.available_memory();
        let mem_used = mem_total.saturating_sub(mem_avail);
        let mem_virtual_percent = if mem_total > 0 {
            mem_used as f32 / mem_total as f32 * 100.0
        } else {
            f32::NAN
        };
        let swap_total = self.sys.total_swap();
        let swap_used = self.sys.used_swap();
        let mem_swap_percent = if swap_total > 0 {
            swap_used as f32 / swap_total as f32 * 100.0
        } else {
            0.0
        };

        // -- Disk (root filesystem, like `psutil.disk_usage("/")`) -------
        self.disks.refresh();
        let (disk_total, disk_free) = root_disk(&self.disks);
        let disk_used = disk_total.saturating_sub(disk_free);
        let disk_usage_percent = if disk_total > 0 {
            disk_used as f32 / disk_total as f32 * 100.0
        } else {
            f32::NAN
        };

        // -- Net (delta since previous poll, like PNIC_BEFORE) ------------
        self.networks.refresh();
        let now = Instant::now();
        let net_eth = self.net_stats(&nets.eth, now);
        let net_wlo = self.net_stats(&nets.wlo, now);

        Snapshot {
            cpu_percent,
            cpu_freq_mhz,
            cpu_load_1: l1,
            cpu_load_5: l5,
            cpu_load_15: l15,
            cpu_temp_c,
            cpu_fan_percent,
            mem_swap_percent,
            mem_virtual_percent,
            mem_used_bytes: mem_used,
            mem_free_bytes: mem_avail,
            mem_total_bytes: mem_total,
            disk_usage_percent,
            disk_used_bytes: disk_used,
            disk_free_bytes: disk_free,
            disk_total_bytes: disk_total,
            uptime_secs: System::uptime(),
            net_eth,
            net_wlo,
            // GPU + slow sensors are overlaid by `Provider::snapshot`.
            gpu_load_pct: f32::NAN,
            gpu_mem_pct: f32::NAN,
            gpu_mem_used_mb: f32::NAN,
            gpu_mem_total_mb: f32::NAN,
            gpu_temp_c: f32::NAN,
            gpu_fps: -1,
            gpu_fan_pct: f32::NAN,
            gpu_freq_mhz: f32::NAN,
            ping_ms: f32::NAN,
            date_iso: String::new(),
            date_epoch: 0,
            wx_temp: None,
            wx_felt: None,
            wx_description: None,
            wx_humidity: None,
            wx_update: None,
        }
    }

    fn net_stats(&mut self, if_name: &str, now: Instant) -> NetIfStats {
        if if_name.is_empty() {
            return NetIfStats::default();
        }
        let (rx, tx) = match self.networks.get(if_name) {
            Some(data) => (data.total_received(), data.total_transmitted()),
            None => {
                log::warn!(
                    "network interface '{if_name}' not found; check ETH/WLO in config.yaml"
                );
                return NetIfStats::default();
            }
        };
        let stats = match self.prev_net.get(if_name) {
            Some(&(prx, ptx, t0)) => {
                let dt = now.duration_since(t0).as_secs_f64().max(1e-3);
                NetIfStats {
                    upload_rate_bps: (tx.saturating_sub(ptx)) as f64 / dt,
                    uploaded_bytes: tx,
                    download_rate_bps: (rx.saturating_sub(prx)) as f64 / dt,
                    downloaded_bytes: rx,
                    present: true,
                }
            }
            // First sighting (like PNIC_BEFORE miss): totals known, rate 0.
            None => NetIfStats {
                upload_rate_bps: 0.0,
                uploaded_bytes: tx,
                download_rate_bps: 0.0,
                downloaded_bytes: rx,
                present: true,
            },
        };
        self.prev_net.insert(if_name.to_string(), (rx, tx, now));
        stats
    }
}

impl Default for SysinfoCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn norm_load(v: f64) -> f32 {
    if v <= 0.0 {
        f32::NAN
    } else {
        v as f32
    }
}

/// CPU temperature with the same priority as `sensors_python.py`:
/// coretemp (Intel) > k10temp (AMD) > cpu_thermal (ARM) > zenpower.
/// The caller refreshes the component list in place (no per-poll realloc).
fn cpu_temperature(components: &Components) -> f32 {
    let mut fallback: Option<f32> = None;
    for c in components.list() {
        let label = c.label().to_ascii_lowercase();
        // sysinfo 0.32: temperature() -> f32 (<= 0 means no reading).
        let temp = c.temperature();
        if temp <= 0.0 {
            continue;
        }
        if label.contains("coretemp")
            || label.contains("k10temp")
            || label.contains("cpu_thermal")
            || label.contains("zenpower")
        {
            return temp;
        }
        if fallback.is_none() && (label.contains("cpu") || label.contains("package")) {
            fallback = Some(temp);
        }
    }
    fallback.unwrap_or(f32::NAN)
}

/// Root-filesystem (total, free) bytes. Prefers `/`, then `C:\` on Windows,
/// else the largest disk.
fn root_disk(disks: &Disks) -> (u64, u64) {
    let list = disks.list();
    if list.is_empty() {
        return (0, 0);
    }
    if let Some(d) = list
        .iter()
        .find(|d| d.mount_point().as_os_str() == "/")
    {
        return (d.total_space(), d.available_space());
    }
    #[cfg(target_os = "windows")]
    if let Some(d) = list
        .iter()
        .find(|d| d.mount_point().to_string_lossy().starts_with("C:"))
    {
        return (d.total_space(), d.available_space());
    }
    let d = list
        .iter()
        .max_by_key(|d| d.total_space())
        .expect("non-empty");
    (d.total_space(), d.available_space())
}

// ---------------------------------------------------------------------------
// Linux hwmon fans (port of `sensors_fans()` in `sensors_python.py`)
// ---------------------------------------------------------------------------

/// Returns CPU fan speed percent, or `NaN` when unavailable.
/// `want` is `"AUTO"` or `"controller/fan"` (e.g. `nct6798/fan2`).
#[cfg(target_os = "linux")]
fn hwmon_fan_percent(want: &str) -> f32 {
    use std::fs;
    let base = std::path::Path::new("/sys/class/hwmon");
    let entries = match fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return f32::NAN,
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let chip = fs::read_to_string(dir.join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let fan_inputs: Vec<_> = match fs::read_dir(&dir) {
            Ok(rd) => rd
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("fan")
                        && e.file_name().to_string_lossy().ends_with("_input")
                })
                .collect(),
            Err(_) => continue,
        };
        for input in fan_inputs {
            let stem = input
                .file_name()
                .to_string_lossy()
                .trim_end_matches("_input")
                .to_string(); // e.g. "fan2"
            let read_u64 = |p: std::path::PathBuf| -> Option<u64> {
                fs::read_to_string(p).ok()?.trim().parse().ok()
            };
            let current = match read_u64(input.path()) {
                Some(v) => v,
                None => continue,
            };
            let max = read_u64(dir.join(format!("{stem}_max"))).unwrap_or_else(|| {
                if current > 2200 {
                    3000 // AIO pumps are usually 3000 RPM
                } else if current > 1500 {
                    2200 // high-speed fans are usually 2200 RPM
                } else {
                    1500
                }
            });
            let min = read_u64(dir.join(format!("{stem}_min"))).unwrap_or(0);
            if max <= min {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let percent = (current.saturating_sub(min)) as f32 / (max - min) as f32 * 100.0;
            let label = fs::read_to_string(dir.join(format!("{stem}_label")))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| stem.clone());
            let id = format!("{chip}/{label}");
            if want != "AUTO" {
                if id == want {
                    return percent.clamp(0.0, 100.0);
                }
                continue;
            }
            if is_cpu_fan(&label) || is_cpu_fan(&chip) {
                return percent.clamp(0.0, 100.0);
            }
        }
    }
    // AUTO with no cpu-labelled fan: NaN (caller disables the widget with a
    // warning, like `stats.py`), NOT the first random fan.
    f32::NAN
}

#[cfg(not(target_os = "linux"))]
fn hwmon_fan_percent(_want: &str) -> f32 {
    f32::NAN
}

#[cfg(target_os = "linux")]
fn is_cpu_fan(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    l.contains("cpu") || l.contains("proc")
}
