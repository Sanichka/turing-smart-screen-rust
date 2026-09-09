// SPDX-License-Identifier: GPL-3.0-or-later
//! ICMP ping via the OS `ping` binary. Ports `stats.Ping` (`ping3`).
//!
//! Deliberate choice over a raw-socket crate (`surge-ping` et al.): no admin
//! rights, no `tokio`, negligible cost at ping intervals (>=1s). Parses the
//! `time=XXms` token from one packet. Returns `NaN` on failure like Python's
//! `ping()` returning `None`.

pub fn ping_once(dest: &str) -> f32 {
    if dest.is_empty() {
        return f32::NAN;
    }
    let output = if cfg!(target_os = "windows") {
        std::process::Command::new("ping")
            .args(["-n", "1", "-w", "1000", dest])
            .output()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("ping")
            .args(["-c", "1", "-W", "1000", dest])
            .output()
    } else {
        std::process::Command::new("ping")
            .args(["-c", "1", "-W", "1", dest])
            .output()
    };
    let out = match output {
        Ok(o) if o.status.success() => o,
        _ => return f32::NAN,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    parse_ms(&text).unwrap_or(f32::NAN)
}

/// Extract `time=12.3 ms` / `time=12ms` / `time<1ms` from ping output.
fn parse_ms(text: &str) -> Option<f32> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find("time")?;
    let after = lower[idx + 4..].trim_start_matches(['=', '<', ' ']);
    // Some locales use ',' as decimal separator.
    let mut num = String::new();
    for c in after.chars() {
        if c.is_ascii_digit() || c == '.' || c == ',' {
            num.push(if c == ',' { '.' } else { c });
        } else {
            break;
        }
    }
    if num.is_empty() {
        return None;
    }
    num.parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_ms;

    #[test]
    fn parses_common_formats() {
        assert!((parse_ms("Reply from 8.8.8.8: bytes=32 time=12ms TTL=115").unwrap() - 12.0).abs() < 1e-3);
        assert!((parse_ms("64 bytes from 8.8.8.8: icmp_seq=1 ttl=115 time=12.3 ms").unwrap() - 12.3).abs() < 1e-3);
        assert!(parse_ms("Request timed out.").is_none());
    }
}
