// SPDX-License-Identifier: GPL-3.0-or-later
//! Sensor providers.
//!
//! Ports `library/sensors/sensors.py` (ABC), `sensors_python.py` (psutil),
//! the stub providers, GPU sensors (`GPUtil`/`pyamdgpuinfo`) and the slow
//! sensors (weather, ping, date).
//!
//! Design: one stateful provider per process owns a single
//! `sysinfo::System` (+ `Networks`/`Disks` helpers) and a cached
//! `GpuCollector`, so repeated polls do zero re-allocation of the OS-handle
//! layer. `Snapshot` is scalars + small `String`s; `NaN`/`None` =
//! unsupported sensor (matches Python `math.nan`).

pub mod cputemp;
pub mod date;
pub mod gpu;
pub mod pawnio;
pub mod ping;
pub mod stub;
pub mod superio;
pub mod sysinfo_impl;
pub mod weather;
pub mod winring0;

use crate::config::{GeneralConfig, HwSensors};

/// Per-interface network counters. Mirrors `sensors.Net.stats()` return:
/// up rate (B/s), uploaded (B), down rate (B/s), downloaded (B).
#[derive(Debug, Clone, Copy, Default)]
pub struct NetIfStats {
    pub upload_rate_bps: f64,
    pub uploaded_bytes: u64,
    pub download_rate_bps: f64,
    pub downloaded_bytes: u64,
    pub present: bool,
}

/// Single poll of every sensor.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub cpu_percent: f32,
    pub cpu_freq_mhz: f32,
    pub cpu_load_1: f32,
    pub cpu_load_5: f32,
    pub cpu_load_15: f32,
    pub cpu_temp_c: f32,
    pub cpu_fan_percent: f32,
    pub mem_swap_percent: f32,
    pub mem_virtual_percent: f32,
    pub mem_used_bytes: u64,
    pub mem_free_bytes: u64,
    pub mem_total_bytes: u64,
    pub disk_usage_percent: f32,
    pub disk_used_bytes: u64,
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub uptime_secs: u64,
    pub net_eth: NetIfStats,
    pub net_wlo: NetIfStats,
    // GPU (Step 3). `gpu_fps` is -1 when unsupported (matches Python).
    pub gpu_load_pct: f32,
    pub gpu_mem_pct: f32,
    pub gpu_mem_used_mb: f32,
    pub gpu_mem_total_mb: f32,
    pub gpu_temp_c: f32,
    pub gpu_fps: i32,
    pub gpu_fan_pct: f32,
    pub gpu_freq_mhz: f32,
    // Slow sensors.
    pub ping_ms: f32,
    pub date_iso: String,
    pub date_epoch: i64,
    pub wx_temp: Option<String>,
    pub wx_felt: Option<String>,
    pub wx_description: Option<String>,
    pub wx_humidity: Option<String>,
    pub wx_update: Option<String>,
}

impl Snapshot {
    /// `serde_json::Value` with `NaN`/`-1`/`None` mapped to `null`
    /// (JSON has no NaN).
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{json, Value};
        let f = |v: f32| -> Value {
            if v.is_nan() || v < 0.0 {
                Value::Null
            } else {
                json!(v)
            }
        };
        let opt = |v: &Option<String>| -> Value {
            match v {
                Some(s) => json!(s),
                None => Value::Null,
            }
        };
        let net = |n: &NetIfStats| {
            if n.present {
                json!({
                    "upload_bps": n.upload_rate_bps,
                    "uploaded_bytes": n.uploaded_bytes,
                    "download_bps": n.download_rate_bps,
                    "downloaded_bytes": n.downloaded_bytes,
                })
            } else {
                Value::Null
            }
        };
        json!({
            "cpu_percent": f(self.cpu_percent),
            "cpu_freq_mhz": f(self.cpu_freq_mhz),
            "cpu_load_1": f(self.cpu_load_1),
            "cpu_load_5": f(self.cpu_load_5),
            "cpu_load_15": f(self.cpu_load_15),
            "cpu_temp_c": f(self.cpu_temp_c),
            "cpu_fan_percent": f(self.cpu_fan_percent),
            "mem_swap_percent": f(self.mem_swap_percent),
            "mem_virtual_percent": f(self.mem_virtual_percent),
            "mem_used_bytes": self.mem_used_bytes,
            "mem_free_bytes": self.mem_free_bytes,
            "mem_total_bytes": self.mem_total_bytes,
            "disk_usage_percent": f(self.disk_usage_percent),
            "disk_used_bytes": self.disk_used_bytes,
            "disk_free_bytes": self.disk_free_bytes,
            "disk_total_bytes": self.disk_total_bytes,
            "uptime_secs": self.uptime_secs,
            "net_eth": net(&self.net_eth),
            "net_wlo": net(&self.net_wlo),
            "gpu_load_pct": f(self.gpu_load_pct),
            "gpu_mem_pct": f(self.gpu_mem_pct),
            "gpu_mem_used_mb": f(self.gpu_mem_used_mb),
            "gpu_mem_total_mb": f(self.gpu_mem_total_mb),
            "gpu_temp_c": f(self.gpu_temp_c),
            "gpu_fps": if self.gpu_fps < 0 { Value::Null } else { json!(self.gpu_fps) },
            "gpu_fan_pct": f(self.gpu_fan_pct),
            "gpu_freq_mhz": f(self.gpu_freq_mhz),
            "ping_ms": f(self.ping_ms),
            "date_iso": self.date_iso,
            "date_epoch": self.date_epoch,
            "wx_temp": opt(&self.wx_temp),
            "wx_felt": opt(&self.wx_felt),
            "wx_description": opt(&self.wx_description),
            "wx_humidity": opt(&self.wx_humidity),
            "wx_update": opt(&self.wx_update),
        })
    }
}

/// Which interfaces to poll (from `config.yaml` `ETH`/`WLO`).
#[derive(Debug, Clone, Default)]
pub struct NetSelection {
    pub eth: String,
    pub wlo: String,
}

/// Slow-poll inputs (ping dest + weather), from `config.yaml`.
/// The daemon will poll these on a >=300s thread; `--sensors-once` fetches
/// them inline once.
#[derive(Debug, Clone, Default)]
pub struct SlowCtx {
    pub ping_dest: String,
    pub weather: weather::WeatherConfig,
}

impl SlowCtx {
    pub fn from_config(cfg: &GeneralConfig) -> Self {
        // `TSS_WEATHER_API_KEY` env override (never logged): keeps secrets
        // out of config.yaml for packaged installs.
        let from_env = std::env::var("TSS_WEATHER_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if from_env.is_some() {
            log::debug!("using weather API key from TSS_WEATHER_API_KEY");
        }
        SlowCtx {
            ping_dest: cfg.ping.clone(),
            weather: weather::WeatherConfig {
                api_key: from_env.unwrap_or_else(|| cfg.weather_api_key.clone()),
                latitude: cfg.weather_latitude,
                longitude: cfg.weather_longitude,
                units: cfg.weather_units.clone(),
                language: cfg.weather_language.clone(),
            },
        }
    }
}

/// Stateful provider. Keep one per process; reuse across polls.
/// `Sysinfo` data is boxed: the collector owns `System`+`Networks`+maps and
/// is orders of magnitude larger than the stub variants.
pub enum Provider {
    Sysinfo {
        sys: Box<sysinfo_impl::SysinfoCollector>,
        gpu: Box<gpu::GpuCollector>,
    },
    StubStatic,
    StubRandom(stub::XorShift64),
}

impl Provider {
    /// Build from `HW_SENSORS`. `AUTO` resolves to sysinfo + GPU detect on
    /// all OS; on Windows, CPU temperature additionally flows from the
    /// PawnIO driver when installed and elevated (no CLR hosting, no apps).
    pub fn from_hw(hw: HwSensors, cpu_fan: &str) -> Self {
        match hw {
            HwSensors::Python | HwSensors::Auto => {
                if hw == HwSensors::Auto && cfg!(target_os = "windows") {
                    log::warn!(
                        "HW_SENSORS=AUTO on Windows uses sysinfo+NVML; CPU temp additionally needs \
                         the PawnIO driver installed and this process elevated (run as admin)"
                    );
                }
                Provider::Sysinfo {
                    sys: Box::new(sysinfo_impl::SysinfoCollector::with_cpu_fan(
                        cpu_fan.to_string(),
                    )),
                    gpu: Box::new(gpu::GpuCollector::detect()),
                }
            }
            HwSensors::Stub => Provider::StubRandom(stub::XorShift64::new(10)),
            HwSensors::Static => Provider::StubStatic,
            HwSensors::Lhm => {
                log::warn!(
                    "HW_SENSORS=LHM is served by the PawnIO path on Windows (no web server needed)"
                );
                Provider::Sysinfo {
                    sys: Box::new(sysinfo_impl::SysinfoCollector::with_cpu_fan(
                        cpu_fan.to_string(),
                    )),
                    gpu: Box::new(gpu::GpuCollector::detect()),
                }
            }
        }
    }

    pub fn gpu_available(&self) -> bool {
        match self {
            Provider::Sysinfo { gpu, .. } => gpu.is_available(),
            Provider::StubStatic | Provider::StubRandom(_) => true,
        }
    }

    /// Fast path for the daemon render tick: sysinfo + GPU + PawnIO temp,
    /// no ping. Slow fields keep caller-supplied values: use `apply_slow`
    /// to overlay the latest `SlowData`.
    pub fn snapshot_fast(&mut self, nets: &NetSelection) -> Snapshot {
        match self {
            Provider::Sysinfo { sys, gpu } => {
                let mut snap = sys.snapshot(nets);
                gpu.fill(&mut snap);
                let (iso, epoch) = date::now();
                snap.date_iso = iso;
                snap.date_epoch = epoch;
                snap
            }
            Provider::StubStatic => stub::static_snapshot(),
            Provider::StubRandom(rng) => stub::random_snapshot(rng),
        }
    }

    /// One-shot snapshot with inline slow sensors (`--sensors-once`).
    pub fn snapshot(&mut self, nets: &NetSelection, slow: &SlowCtx) -> Snapshot {
        let mut snap = self.snapshot_fast(nets);
        if matches!(self, Provider::Sysinfo { .. }) {
            // Stubs already carry fixed slow values (fixed STUB date etc.).
            apply_slow(&mut snap, &fetch_slow(slow));
        }
        snap
    }
}

/// Slow-sensor results, refreshed on a low-frequency thread by the daemon.
#[derive(Debug, Clone, Default)]
pub struct SlowData {
    pub ping_ms: f32,
    pub wx_temp: Option<String>,
    pub wx_felt: Option<String>,
    pub wx_description: Option<String>,
    pub wx_humidity: Option<String>,
    pub wx_update: Option<String>,
}

/// Ping + date always; weather only when an API key is configured
/// (mirrors Python: no key → warning + placeholder, no HTTP).
pub fn fetch_slow(slow: &SlowCtx) -> SlowData {
    let mut out = SlowData {
        ping_ms: ping::ping_once(&slow.ping_dest),
        ..Default::default()
    };
    if slow.weather.api_key.is_empty() {
        let w = weather::fetch(&slow.weather);
        // fetch() already warned; keep its placeholder description.
        out.wx_description = w.description;
    } else {
        let w = weather::fetch(&slow.weather);
        out.wx_temp = w.temp;
        out.wx_felt = w.felt;
        out.wx_description = w.description;
        out.wx_humidity = w.humidity;
        out.wx_update = w.update;
    }
    out
}

pub fn apply_slow(snap: &mut Snapshot, data: &SlowData) {
    snap.ping_ms = data.ping_ms;
    snap.wx_temp = data.wx_temp.clone();
    snap.wx_felt = data.wx_felt.clone();
    snap.wx_description = data.wx_description.clone();
    snap.wx_humidity = data.wx_humidity.clone();
    snap.wx_update = data.wx_update.clone();
}
