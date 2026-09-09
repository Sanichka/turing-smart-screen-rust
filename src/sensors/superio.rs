// SPDX-License-Identifier: GPL-3.0-or-later
//! SuperIO fan tachometers via ISA PNP probing + banked reads.
//!
//! Register maps ported from LHM `SuperIOHardware` / `Nct677X` / `IT87XX`
//! (chip IDs, enter/exit sequences, tach regs, RPM formulas). Raw port I/O
//! goes through a WinRing0-compatible driver (`WinRing0_1_2_0` service —
//! ours once installed, or an already-present one) via OLS ioctls; the
//! PawnIO `LpcIO` module cannot do raw ISA ports on all boards, so it is
//! only a fallback reference here.
//!
//! Same elevation story as CPU temp (see `pawnio.rs`).
//!
//! Scope: Nuvoton NCT677x/678x/679x + ITE IT87xx/IT8688E/IT8792E
//! tachometers. Voltages/temperatures/PWM stay out (widgets don't need them).

use super::winring0::WinRing0;

const REG_PORTS: [u16; 2] = [0x2E, 0x4E];

/// (id, rev) -> (name, fan count) for Nuvoton/Fintek HWM chips.
fn nct_ident(id: u8, rev: u8) -> Option<(&'static str, usize)> {
    match (id, rev) {
        (0xB4, 0x70..) => Some(("NCT6771F", 4)),
        (0xC3, 0x30..) => Some(("NCT6776F", 5)),
        (0xC5, 0x60..) => Some(("NCT6779D", 5)),
        (0xC8, 0x03) => Some(("NCT6791D", 6)),
        (0xC9, 0x11) => Some(("NCT6792D", 6)),
        (0xC9, 0x13) => Some(("NCT6792D-A", 6)),
        (0xD1, 0x21) => Some(("NCT6793D", 6)),
        (0xD3, 0x52) => Some(("NCT6795D", 6)),
        (0xD4, 0x23) => Some(("NCT6796D", 6)),
        (0xD4, 0x2A) => Some(("NCT6796D-R", 7)),
        (0xD4, 0x51) => Some(("NCT6797D", 7)),
        (0xD4, 0x2B) => Some(("NCT6798D", 7)),
        (0xD4, 0x40) | (0xD4, 0x41) => Some(("NCT6686D", 7)),
        (0xD8, 0x02) => Some(("NCT6799D", 7)),
        _ => None,
    }
}

/// ITE 16-bit chip ID word -> (name, fan count).
fn ite_ident(id: u16) -> Option<(&'static str, usize)> {
    match id {
        0x8688 => Some(("IT8688E", 6)),
        0x8792 | 0x8733 => Some(("IT8792E", 3)),
        0x8695 => Some(("IT87952E", 3)),
        0x8628 | 0x8631 | 0x8665 | 0x8686 | 0x8689 | 0x8696 | 0x8728 => {
            Some(("IT87xx", 5))
        }
        0x8613 | 0x8620 | 0x8625 | 0x8638 | 0x8655 | 0x8705 | 0x8712 | 0x8716 | 0x8718 | 0x8720
        | 0x8721 | 0x8726 | 0x8771 | 0x8772 | 0x8790 => Some(("IT87xx", 3)),
        _ => None,
    }
}

enum Chip {
    Nct { base: u16, fans: usize, name: &'static str },
    Ite { base: u16, fans: usize, name: &'static str },
}

pub struct SuperIo {
    r0: WinRing0,
    chip: Chip,
}

impl SuperIo {
    fn outb(&self, port: u16, val: u8) {
        self.r0.write_io_byte(port, val);
    }

    fn inb(&self, port: u16) -> u8 {
        self.r0.read_io_byte(port).unwrap_or(0xFF)
    }

    /// PNP config byte read.
    fn conf(&self, port: u16, reg: u8) -> u8 {
        self.outb(port, reg);
        self.inb(port + 1)
    }

    /// Open the ring-0 driver and probe both PNP ports. `None` = no
    /// driver, no supported chip, or invalid base address (each logged).
    pub fn detect() -> Option<Self> {
        let r0 = match WinRing0::open() {
            Ok(r) => r,
            Err(e) => {
                log::debug!("SuperIO: {e}");
                return None;
            }
        };
        let io = SuperIo {
            r0,
            chip: Chip::Nct { base: 0, fans: 0, name: "" },
        };
        for &port in &REG_PORTS {
            // Nuvoton/Fintek/Winbond pass first (enter 0x87,0x87).
            io.outb(port, 0x87);
            io.outb(port, 0x87);
            let id = io.conf(port, 0x20);
            let rev = io.conf(port, 0x21);
            io.outb(port, 0xAA);
            log::debug!("SuperIO PNP probe {port:#X}: ID {id:#04X}/{rev:#04X}");
            if let Some((name, fans)) = nct_ident(id, rev) {
                log::info!("SuperIO: {name} at PNP {port:#X} (ID {id:#04X}/{rev:#04X})");
                return io.with_nct(port, name, fans);
            }
            // ITE pass (different enter sequence).
            io.outb(port, 0x87);
            io.outb(port, 0x01);
            io.outb(port, 0x55);
            io.outb(port, if port == 0x4E { 0xAA } else { 0x55 });
            let hi = io.conf(port, 0x20);
            let lo = io.conf(port, 0x21);
            let id = ((hi as u16) << 8) | lo as u16;
            if port != 0x4E {
                // ITE exit (never exit the 2nd SIO, e.g. Gigabyte IT8792E).
                io.outb(port, 0x02);
                io.outb(port + 1, 0x02);
            }
            log::debug!("SuperIO ITE probe {port:#X}: ID {id:#06X}");
            if let Some((name, fans)) = ite_ident(id) {
                log::info!("SuperIO: {name} at PNP {port:#X} (ID {id:#06X})");
                return io.with_ite(port, name, fans);
            }
            io.outb(port, 0xAA);
        }
        log::debug!("no supported SuperIO chip found");
        None
    }

    fn with_nct(self, port: u16, name: &'static str, fans: usize) -> Option<Self> {
        // Re-enter (probe exited), select HWM logical device 0x0B.
        self.outb(port, 0x87);
        self.outb(port, 0x87);
        self.outb(port, 0x07);
        self.outb(port + 1, 0x0B);
        let base = self.conf_word(port, 0x60);
        std::thread::sleep(std::time::Duration::from_millis(1));
        let verify = self.conf_word(port, 0x60);
        // Clear the HWM I/O space lock present on 6791D+ firmware.
        let lock = self.conf(port, 0x28);
        if lock & 0x10 != 0 {
            self.outb(port, 0x28);
            self.outb(port + 1, lock & !0x10);
        }
        self.outb(port, 0xAA);
        if base != verify || base < 0x100 || (base & 0xF007) != 0 {
            log::warn!("{name}: invalid HWM base {base:#X}");
            return None;
        }
        log::info!("{name}: HWM base {base:#X}, {fans} fans");
        Some(SuperIo { chip: Chip::Nct { base, fans, name }, ..self })
    }

    fn with_ite(self, port: u16, name: &'static str, fans: usize) -> Option<Self> {
        // Re-enter, select ENV logical device 0x04.
        self.outb(port, 0x87);
        self.outb(port, 0x01);
        self.outb(port, 0x55);
        self.outb(port, if port == 0x4E { 0xAA } else { 0x55 });
        self.outb(port, 0x07);
        self.outb(port + 1, 0x04);
        let base = self.conf_word(port, 0x60);
        std::thread::sleep(std::time::Duration::from_millis(1));
        let verify = self.conf_word(port, 0x60);
        if port != 0x4E {
            self.outb(port, 0x02);
            self.outb(port + 1, 0x02);
        }
        if base < 0x100 || base != verify || (base & 0xF007) != 0 {
            log::warn!("{name}: invalid ENV base {base:#X}");
            return None;
        }
        log::info!("{name}: ENV base {base:#X}, {fans} fans");
        Some(SuperIo { chip: Chip::Ite { base, fans, name }, ..self })
    }

    /// PNP config word read (big-endian).
    fn conf_word(&self, port: u16, reg: u8) -> u16 {
        let hi = self.conf(port, reg) as u16;
        let lo = self.conf(port, reg + 1) as u16;
        (hi << 8) | lo
    }

    /// Banked Nuvoton read: select the bank register through the index
    /// port, write the bank number through the DATA port, then read the
    /// target register through index/data.
    fn nct_read(&self, base: u16, addr: u16) -> u8 {
        let bank = (addr >> 8) as u8;
        let reg = (addr & 0xFF) as u8;
        self.outb(base + 5, 0x4E);
        self.outb(base + 6, bank);
        self.outb(base + 5, reg);
        self.inb(base + 6)
    }

    /// ITE ENV read (addrReg=base+5, dataReg=base+6).
    fn ite_read(&self, base: u16, reg: u8) -> u8 {
        self.outb(base + 5, reg);
        self.inb(base + 6)
    }

    /// All fan tachometers in RPM (`None` = stalled/disabled/invalid).
    pub fn fan_rpms(&self) -> Vec<Option<u32>> {
        match &self.chip {
            Chip::Nct { base, fans, .. } => {
                const REGS: [u16; 7] = [0x4B0, 0x4B2, 0x4B4, 0x4B6, 0x4B8, 0x4BA, 0x4CC];
                (0..*fans)
                    .map(|i| {
                        let high = self.nct_read(*base, REGS[i]) as u32;
                        let low = self.nct_read(*base, REGS[i] + 1) as u32;
                        nct_rpm((high << 5) | (low & 0x1F))
                    })
                    .collect()
            }
            Chip::Ite { base, fans, .. } => {
                const REG: [u8; 6] = [0x0D, 0x0E, 0x0F, 0x80, 0x82, 0x4C];
                const EXT: [u8; 6] = [0x18, 0x19, 0x1A, 0x81, 0x83, 0x4D];
                // 16-bit tach enable bits (fan3:bit4, fan4:bit5, fan5:bit2).
                let en = self.ite_read(*base, 0x0C);
                let sixteen = |i: usize| match i {
                    2 => en & 0x10 != 0,
                    3 => en & 0x20 != 0,
                    4 => en & 0x04 != 0,
                    _ => true,
                };
                (0..(*fans).min(6))
                    .map(|i| {
                        if !sixteen(i) {
                            return None;
                        }
                        let lo = self.ite_read(*base, REG[i]) as u32;
                        let hi = self.ite_read(*base, EXT[i]) as u32;
                        ite_rpm((hi << 8) | lo)
                    })
                    .collect()
            }
        }
    }

    pub fn chip_name(&self) -> &'static str {
        match &self.chip {
            Chip::Nct { name, .. } | Chip::Ite { name, .. } => name,
        }
    }

    /// CPU fan percent. `want` is `"AUTO"`, `"fan<N>"` (1-based), or the
    /// documented `"chip/fan"` form (e.g. `nct6798/fan2`, chip part
    /// informational only — mirrors the config.yaml CPU_FAN selector).
    /// AUTO = fan #1 when spinning, else the first spinning fan; all
    /// readings are logged so a wrong guess can be corrected explicitly.
    pub fn cpu_fan_percent(&self, want: &str) -> f32 {
        let rpms = self.fan_rpms();
        log::debug!(
            "{} tachometers: {:?}",
            self.chip_name(),
            rpms.iter().map(|r| r.unwrap_or(0)).collect::<Vec<_>>()
        );
        // Accept "fan2" and the documented "nct6798/fan2" alike.
        let selector = want.rsplit('/').next().unwrap_or(want);
        if selector != "AUTO" && !selector.starts_with("fan") {
            log::debug!("SuperIO: ignoring unparsable CPU_FAN '{want}', using AUTO");
        }
        let pick = if let Some(n) = selector
            .strip_prefix("fan")
            .and_then(|s| s.parse::<usize>().ok())
        {
            rpms.get(n.saturating_sub(1)).copied().flatten()
        } else {
            // Fan #1 when spinning, else the first spinning fan.
            rpms.first()
                .copied()
                .flatten()
                .filter(|r| *r > 0)
                .or_else(|| rpms.iter().find_map(|r| (*r).filter(|v| *v > 0)))
        };
        match pick {
            Some(rpm) if rpm > 0 => rpm_to_percent(rpm),
            _ => f32::NAN,
        }
    }
}

/// Nuvoton 13-bit count → RPM (count mode always on these chips).
fn nct_rpm(count: u32) -> Option<u32> {
    if count >= 0x1FFF {
        Some(0) // stalled
    } else if count >= 0x15 {
        Some(1_350_000 / count)
    } else {
        None // invalid
    }
}

/// ITE 16-bit tach → RPM.
fn ite_rpm(v: u32) -> Option<u32> {
    if v <= 0x3F {
        None
    } else if v < 0xFFFF {
        Some(1_350_000 / (v * 2))
    } else {
        Some(0)
    }
}

/// RPM → percent with the same max-RPM heuristic as the hwmon path
/// (AIO pumps ~3000, fast fans ~2200, otherwise ~1500).
fn rpm_to_percent(rpm: u32) -> f32 {
    let max = if rpm > 2200 {
        3000.0
    } else if rpm > 1500 {
        2200.0
    } else {
        1500.0
    };
    (rpm as f32 / max * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nct_formula_boundaries() {
        assert_eq!(nct_rpm(0x1FFF), Some(0));
        assert_eq!(nct_rpm(0x2000), Some(0));
        assert_eq!(nct_rpm(1350), Some(1000)); // 1350000/1350
        assert_eq!(nct_rpm(0x15), Some(1_350_000 / 0x15));
        assert_eq!(nct_rpm(0x14), None);
        assert_eq!(nct_rpm(0), None);
    }

    #[test]
    fn ite_formula_boundaries() {
        assert_eq!(ite_rpm(0x3F), None);
        assert_eq!(ite_rpm(0x40), Some(1_350_000 / (0x40 * 2)));
        assert_eq!(ite_rpm(0xFFFF), Some(0));
    }

    #[test]
    fn chip_tables() {
        assert_eq!(nct_ident(0xD4, 0x2B), Some(("NCT6798D", 7)));
        assert_eq!(nct_ident(0xC8, 0x03), Some(("NCT6791D", 6)));
        assert_eq!(nct_ident(0x00, 0x00), None);
        assert_eq!(ite_ident(0x8688), Some(("IT8688E", 6)));
        assert_eq!(ite_ident(0x8733), Some(("IT8792E", 3)));
        assert_eq!(ite_ident(0xFFFF), None);
    }

    #[test]
    fn percent_heuristic() {
        assert!((rpm_to_percent(1000) - 1000.0 / 1500.0 * 100.0).abs() < 1e-3);
        assert!((rpm_to_percent(2000) - 2000.0 / 2200.0 * 100.0).abs() < 1e-3);
        assert!((rpm_to_percent(2800) - 2800.0 / 3000.0 * 100.0).abs() < 1e-3);
    }

    /// Hardware diagnostic (PawnIO LpcIO init outcomes).
    /// Run: `cargo test --lib diag_io_ports -- --ignored --nocapture` as admin.
    #[test]
    #[ignore]
    fn diag_io_ports() {
        use crate::sensors::pawnio::PawnIo;
        let bytes = std::fs::read("res/drivers/LpcIO.bin").expect("LpcIO.bin");
        let pawn = PawnIo::load_module(&bytes).expect("load LpcIO module");
        println!("module loaded");
        for slot in [0i64, 1] {
            let r = pawn.execute("ioctl_select_slot", &[slot], 0);
            println!("select_slot({slot}): {r:?}");
        }
        let r = pawn.execute("ioctl_find_bars", &[], 0);
        println!("find_bars: {r:?}");
    }

    /// WinRing0 diagnostic: raw OLS PNP probe (no PawnIO involved).
    /// Run unelevated first (demand-start may kick in), else as admin:
    /// `cargo test --lib diag_ols -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diag_ols() {
        use crate::sensors::winring0::WinRing0;
        let r0 = match WinRing0::open() {
            Ok(r) => r,
            Err(e) => {
                println!("ols open: Err({e})");
                return;
            }
        };
        println!("ols open: Ok");
        for &port in &[0x2Eu16, 0x4E] {
            r0.write_io_byte(port, 0x87);
            r0.write_io_byte(port, 0x87);
            r0.write_io_byte(port, 0x20);
            let id = r0.read_io_byte(port + 1);
            r0.write_io_byte(port, 0x21);
            let rev = r0.read_io_byte(port + 1);
            r0.write_io_byte(port, 0xAA);
            println!("ols pnp {port:#X}: id={id:?} rev={rev:?}");
        }
        // CMOS sanity: seconds register should tick.
        r0.write_io_byte(0x70, 0x00);
        let s1 = r0.read_io_byte(0x71);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        r0.write_io_byte(0x70, 0x00);
        let s2 = r0.read_io_byte(0x71);
        println!("ols cmos sec: {s1:?} -> {s2:?}");
    }
}
