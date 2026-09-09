// SPDX-License-Identifier: GPL-3.0-or-later
//! Stub providers. Ports `sensors_stub_static.py` / `sensors_stub_random.py`.
//!
//! Used for `HW_SENSORS=STATIC|STUB`, theme screenshots without hardware,
//! and tests. RNG is a dependency-free xorshift (seed 10, like Python's
//! `random.seed(10)`) to keep the daemon footprint minimal.

use super::{NetIfStats, Snapshot};

pub const STATIC_PERCENT: f32 = 50.0;
pub const STATIC_TEMP_C: f32 = 67.3;
pub const STATIC_CPU_FREQ_MHZ: f32 = 2400.0;
pub const STATIC_GPU_FREQ_MHZ: f32 = 1500.0;
pub const STATIC_GPU_FPS: i32 = 120;

pub fn static_snapshot() -> Snapshot {
    let net = NetIfStats {
        upload_rate_bps: 1_061_000_000.0,
        uploaded_bytes: 1_061_000_000,
        download_rate_bps: 1_061_000_000.0,
        downloaded_bytes: 1_061_000_000,
        present: true,
    };
    Snapshot {
        cpu_percent: STATIC_PERCENT,
        cpu_freq_mhz: STATIC_CPU_FREQ_MHZ,
        cpu_load_1: STATIC_PERCENT,
        cpu_load_5: STATIC_PERCENT,
        cpu_load_15: STATIC_PERCENT,
        cpu_temp_c: STATIC_TEMP_C,
        cpu_fan_percent: STATIC_PERCENT,
        mem_swap_percent: STATIC_PERCENT,
        mem_virtual_percent: STATIC_PERCENT,
        mem_used_bytes: 32_000_000_000,
        mem_free_bytes: 32_000_000_000,
        mem_total_bytes: 64_000_000_000,
        disk_usage_percent: STATIC_PERCENT,
        disk_used_bytes: 500_000_000_000,
        disk_free_bytes: 500_000_000_000,
        disk_total_bytes: 1_000_000_000_000,
        uptime_secs: 4_294_036,
        net_eth: net,
        net_wlo: net,
        gpu_load_pct: STATIC_PERCENT,
        gpu_mem_pct: STATIC_PERCENT,
        gpu_mem_used_mb: 16_384.0,
        gpu_mem_total_mb: 32_768.0,
        gpu_temp_c: STATIC_TEMP_C,
        gpu_fps: STATIC_GPU_FPS,
        gpu_fan_pct: STATIC_PERCENT,
        gpu_freq_mhz: STATIC_GPU_FREQ_MHZ,
        ping_ms: 50.0,
        // Fixed stamp like Python's STUB date (1694014609).
        date_iso: "2023-09-06T11:56:49+02:00".to_string(),
        date_epoch: 1_694_014_609,
        wx_temp: Some("17.5°C".to_string()),
        wx_felt: Some("(17.2°C)".to_string()),
        wx_description: Some("Cloudy".to_string()),
        wx_humidity: Some("45%".to_string()),
        wx_update: Some("@15:33".to_string()),
    }
}

/// Minimal xorshift64* RNG (no `rand` dependency for one debug path).
#[derive(Debug, Clone)]
pub struct XorShift64(pub u64);

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        const DIV: f64 = u64::MAX as f64;
        lo + (self.next_u64() as f64 / DIV) * (hi - lo)
    }

    fn uniform_u64(&mut self, lo: u64, hi: u64) -> u64 {
        lo + (self.next_u64() % (hi - lo).max(1))
    }
}

pub fn random_snapshot(rng: &mut XorShift64) -> Snapshot {
    let net = NetIfStats {
        upload_rate_bps: rng.uniform(1e6, 999e6),
        uploaded_bytes: rng.uniform_u64(1_000_000, 999_000_000),
        download_rate_bps: rng.uniform(1e6, 999e6),
        downloaded_bytes: rng.uniform_u64(1_000_000, 999_000_000),
        present: true,
    };
    Snapshot {
        cpu_percent: rng.uniform(0.0, 100.0) as f32,
        cpu_freq_mhz: rng.uniform(800.0, 3400.0) as f32,
        cpu_load_1: rng.uniform(0.0, 100.0) as f32,
        cpu_load_5: rng.uniform(0.0, 100.0) as f32,
        cpu_load_15: rng.uniform(0.0, 100.0) as f32,
        cpu_temp_c: rng.uniform(30.0, 90.0) as f32,
        cpu_fan_percent: rng.uniform(0.0, 100.0) as f32,
        mem_swap_percent: rng.uniform(0.0, 100.0) as f32,
        mem_virtual_percent: rng.uniform(0.0, 100.0) as f32,
        mem_used_bytes: rng.uniform_u64(300_000_000, 16_000_000_000),
        mem_free_bytes: rng.uniform_u64(300_000_000, 16_000_000_000),
        mem_total_bytes: 64_000_000_000,
        disk_usage_percent: rng.uniform(0.0, 100.0) as f32,
        disk_used_bytes: rng.uniform_u64(1_000_000_000, 2_000_000_000_000),
        disk_free_bytes: rng.uniform_u64(1_000_000_000, 2_000_000_000_000),
        disk_total_bytes: 2_000_000_000_000,
        uptime_secs: 4_294_036,
        net_eth: net,
        net_wlo: net,
        gpu_load_pct: rng.uniform(0.0, 100.0) as f32,
        gpu_mem_pct: rng.uniform(0.0, 100.0) as f32,
        gpu_mem_used_mb: rng.uniform(300.0, 16000.0) as f32,
        gpu_mem_total_mb: 16000.0,
        gpu_temp_c: rng.uniform(30.0, 90.0) as f32,
        gpu_fps: rng.uniform_u64(20, 120) as i32,
        gpu_fan_pct: rng.uniform(0.0, 100.0) as f32,
        gpu_freq_mhz: rng.uniform(800.0, 3400.0) as f32,
        ping_ms: rng.uniform(5.0, 120.0) as f32,
        date_iso: "2023-09-06T11:56:49+02:00".to_string(),
        date_epoch: 1_694_014_609,
        wx_temp: Some("17.5°C".to_string()),
        wx_felt: Some("(17.2°C)".to_string()),
        wx_description: Some("Cloudy".to_string()),
        wx_humidity: Some("45%".to_string()),
        wx_update: Some("@15:33".to_string()),
    }
}
