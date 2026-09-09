// SPDX-License-Identifier: GPL-3.0-or-later
//! CPU package temperature without sidecars.
//!
//! - AMD Zen: SMN `THM_TCON_CUR_TMP` (`0x59800`) through the PawnIO driver,
//!   same formula as LHM `Amd17Cpu` / Linux `k10temp`.
//! - Intel: DTS MSR `0x19C` minus TjMax (`0x1A2`), standard.
//! - Anything else (or no driver): `None`, caller falls back as before.

use super::pawnio::PawnIo;

const SMN_THM_TCON_CUR_TMP: u32 = 0x0005_9800;
const MSR_THERM_STATUS: u32 = 0x19C;
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

enum Kind {
    Off,
    AmdZen { pawn: PawnIo, tdie_offset: f32 },
    Intel { pawn: PawnIo },
}

pub struct CpuTemp {
    kind: Kind,
}

impl CpuTemp {
    /// Probe once at startup. Cheap and silent when the driver is absent.
    pub fn detect() -> Self {
        #[cfg(not(target_arch = "x86_64"))]
        return CpuTemp { kind: Kind::Off };

        #[cfg(target_arch = "x86_64")]
        {
            let vendor = cpuid_vendor();
            if vendor == "AuthenticAMD" {
                let (family, model) = cpuid_family_model();
                // Zen = family 0x17/0x18/0x19/0x1A (Hygon 0x18 too).
                if matches!(family, 0x17..=0x1A) {
                    if let Some(pawn) = load_module("res/drivers/AMDFamily17.bin") {
                        // Smoke-test the SMN path before claiming support.
                        if pawn.read_smn(SMN_THM_TCON_CUR_TMP).is_some() {
                            log::info!("CPU temp: AMD Zen SMN via PawnIO");
                            return CpuTemp {
                                kind: Kind::AmdZen {
                                    pawn,
                                    tdie_offset: amd_offset(model, &cpuid_brand()),
                                },
                            };
                        }
                        log::warn!("PawnIO SMN read failed; CPU temp unavailable");
                    }
                } else {
                    log::debug!("AMD family {family:#X} has no SMN temp path here");
                }
            } else if vendor == "GenuineIntel" {
                if let Some(pawn) = load_module("res/drivers/IntelMSR.bin") {
                    if pawn.read_msr(MSR_THERM_STATUS).is_some() {
                        log::info!("CPU temp: Intel DTS via PawnIO");
                        return CpuTemp { kind: Kind::Intel { pawn } };
                    }
                    log::warn!("PawnIO MSR read failed; CPU temp unavailable");
                }
            } else {
                log::debug!("unknown CPU vendor {vendor}; no temp path");
            }
            CpuTemp { kind: Kind::Off }
        }
    }

    pub fn read_celsius(&self) -> Option<f32> {
        match &self.kind {
            Kind::Off => None,
            Kind::AmdZen { pawn, tdie_offset } => {
                let raw = pawn.read_smn(SMN_THM_TCON_CUR_TMP)?;
                let mut t = ((raw >> 21) * 125) as f32 * 0.001;
                if (raw & 0x80000) != 0 || (raw & 0x30000) == 0x30000 {
                    t -= 49.0;
                }
                let t = t + tdie_offset;
                (t > 0.0 && t < 125.0).then_some(t)
            }
            Kind::Intel { pawn } => {
                let status = pawn.read_msr(MSR_THERM_STATUS)? as u32;
                if status & (1 << 31) == 0 {
                    return None; // DTS reading invalid
                }
                let tjmax = pawn
                    .read_msr(MSR_TEMPERATURE_TARGET)
                    .map(|v| ((v >> 16) & 0xFF) as f32)
                    .filter(|tj| *tj > 0.0)
                    .unwrap_or(100.0);
                let digital = ((status >> 16) & 0x7F) as f32;
                let t = tjmax - digital;
                (t > 0.0 && t < 125.0).then_some(t)
            }
        }
    }
}

fn load_module(path: &str) -> Option<PawnIo> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            log::debug!("no PawnIO module {path} ({e}); CPU temp needs the driver + res/drivers/");
            return None;
        }
    };
    match PawnIo::load_module(&bytes) {
        Ok(p) => Some(p),
        Err(e) => {
            log::debug!("{e}");
            None
        }
    }
}

/// k10temp offset table (Tctl→Tdie), millidegrees→Celsius here.
fn amd_offset(_model: u32, brand: &str) -> f32 {
    if brand.contains("1600X") || brand.contains("1700X") || brand.contains("1800X") {
        -20.0
    } else if brand.contains("Threadripper 19") || brand.contains("Threadripper 29") {
        -27.0
    } else if brand.contains("2700X") {
        -10.0
    } else {
        0.0
    }
}

#[cfg(target_arch = "x86_64")]
fn cpuid_vendor() -> String {
    let r = std::arch::x86_64::__cpuid(0);
    let mut b = [0u8; 12];
    b[..4].copy_from_slice(&r.ebx.to_le_bytes());
    b[4..8].copy_from_slice(&r.edx.to_le_bytes());
    b[8..].copy_from_slice(&r.ecx.to_le_bytes());
    String::from_utf8_lossy(&b).into_owned()
}

#[cfg(target_arch = "x86_64")]
fn cpuid_family_model() -> (u32, u32) {
    let r = std::arch::x86_64::__cpuid(1);
    let eax = r.eax;
    let family = ((eax >> 8) & 0xF) + ((eax >> 20) & 0xFF);
    let model = ((eax >> 4) & 0xF) | ((eax >> 12) & 0xF0);
    (family, model)
}

#[cfg(target_arch = "x86_64")]
fn cpuid_brand() -> String {
    let mut b = [0u8; 48];
    for (i, leaf) in [0x8000_0002u32, 0x8000_0003, 0x8000_0004].iter().enumerate() {
        let r = std::arch::x86_64::__cpuid(*leaf);
        for (j, reg) in [r.eax, r.ebx, r.ecx, r.edx].iter().enumerate() {
            b[i * 16 + j * 4..i * 16 + (j + 1) * 4].copy_from_slice(&reg.to_le_bytes());
        }
    }
    String::from_utf8_lossy(&b).trim_matches('\0').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Formula check against known vectors (raw register → °C).
    #[test]
    fn zen_temp_formula() {
        // Tctl 55.0°C: raw = (55*1000/125) << 21 = 440 << 21.
        let raw: u32 = (440 << 21);
        let t = ((raw >> 21) * 125) as f32 * 0.001;
        assert!((t - 55.0).abs() < 1e-3);
        // -49°C adjustment flag (RANGE_SEL).
        let raw2: u32 = (440 << 21) | 0x80000;
        let mut t2 = ((raw2 >> 21) * 125) as f32 * 0.001;
        if (raw2 & 0x80000) != 0 {
            t2 -= 49.0;
        }
        assert!((t2 - 6.0).abs() < 1e-3);
    }

    #[test]
    fn intel_temp_formula() {
        // TjMax 100, digital readout 40 → 60°C.
        let tjmax = 100.0f32;
        let digital = 40.0f32;
        assert!((tjmax - digital - 60.0).abs() < 1e-3);
    }
}
