// SPDX-License-Identifier: GPL-3.0-or-later
//! Serial port handling. Ports `LcdComm.openSerial/closeSerial/auto_detect`
//! with the same retry policy (10 attempts, 1s apart — the screen resets on
//! startup so its COM port can vanish for seconds).

use std::time::Duration;

const BAUD: u32 = 115_200;
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_RETRY_DELAY: Duration = Duration::from_secs(1);
const IO_TIMEOUT: Duration = Duration::from_secs(1);

pub struct SerialLink {
    port: Option<Box<dyn serialport::SerialPort>>,
    /// Configured name (`"AUTO"` or e.g. `"COM4"`); kept for reopen.
    /// On AUTO the port is re-detected every attempt (it can change across
    /// a display reset).
    com_port: String,
}

impl SerialLink {
    pub fn open(com_port: &str) -> Result<Self, String> {
        let mut link = SerialLink {
            port: None,
            com_port: com_port.to_string(),
        };
        link.reopen()?;
        Ok(link)
    }

    fn open_once(&mut self, path: &str) -> Result<(), String> {
        match serialport::new(path, BAUD)
            .timeout(IO_TIMEOUT)
            .flow_control(serialport::FlowControl::Hardware)
            .open()
        {
            Ok(p) => {
                self.port = Some(p);
                Ok(())
            }
            Err(e) => Err(format!("cannot open {path}: {e}")),
        }
    }

    /// (Re)open with retries. Consumes the old handle first.
    pub fn reopen(&mut self) -> Result<(), String> {
        self.port = None;
        for attempt in 1..=OPEN_ATTEMPTS {
            let name: Option<String> = if self.com_port == "AUTO" {
                match auto_detect() {
                    Some(p) => {
                        log::debug!("auto-detected COM port: {p}");
                        Some(p)
                    }
                    None => {
                        log::warn!(
                            "COM port auto-detect failed ({attempt}/{OPEN_ATTEMPTS})"
                        );
                        None
                    }
                }
            } else {
                Some(self.com_port.clone())
            };
            if let Some(path) = name {
                match self.open_once(&path) {
                    Ok(()) => return Ok(()),
                    Err(e) => log::warn!("{e} (attempt {attempt}/{OPEN_ATTEMPTS})"),
                }
            }
            std::thread::sleep(OPEN_RETRY_DELAY);
        }
        Err(format!(
            "cannot open COM port {} after {OPEN_ATTEMPTS} attempts; \
             run configure and select the port manually",
            self.com_port
        ))
    }

    pub fn close(&mut self) {
        self.port = None; // dropped → closed
    }

    pub fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        let port = self
            .port
            .as_mut()
            .ok_or_else(|| "serial port not open".to_string())?;
        port.write_all(data)
            .map_err(|e| format!("serial write failed: {e}"))
    }

    pub fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), String> {
        use std::io::Read;
        let port = self
            .port
            .as_mut()
            .ok_or_else(|| "serial port not open".to_string())?;
        port.read_exact(buf)
            .map_err(|e| format!("serial read failed: {e}"))
    }

    pub fn flush_input(&mut self) {
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
    }
}

/// Port of `LcdCommRevA.auto_detect_com_port`.
pub fn auto_detect() -> Option<String> {
    let ports = serialport::available_ports().ok()?;
    for p in ports {
        if let serialport::SerialPortType::UsbPort(info) = p.port_type {
            if info.serial_number.as_deref() == Some("USB35INCHIPSV2") {
                return Some(p.port_name);
            }
            if info.vid == 0x1a86 && info.pid == 0x5722 {
                return Some(p.port_name);
            }
        }
    }
    None
}
