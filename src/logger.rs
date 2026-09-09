// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal stderr + file logger. Replaces `env_logger` (~regex+jiff, MBs)
//! with ~100 lines: `RUST_LOG` level filter (`off/error/warn/info/debug/trace`,
//! default `info`), `timestamp [LEVEL] message` lines on stderr and in
//! `log.log` (1MB cap, truncated on rollover like Python's backupCount=0).

use log::{Level, LevelFilter, Metadata, Record};
use std::sync::{Mutex, OnceLock};

/// Python parity: 1MB text log in the working directory.
const LOG_FILE: &str = "log.log";
const LOG_CAP: u64 = 1_000_000;

struct Logger {
    level: LevelFilter,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();
static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

fn log_file() -> Option<&'static Mutex<std::fs::File>> {
    FILE.get_or_init(|| {
        Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(LOG_FILE)
                .unwrap_or_else(|_| {
                    // Nowhere to log the failure; fall back to a sink.
                    #[cfg(unix)]
                    let sink =
                        std::fs::OpenOptions::new().write(true).open("/dev/null").unwrap();
                    #[cfg(windows)]
                    let sink =
                        std::fs::OpenOptions::new().write(true).open("NUL").unwrap();
                    sink
                }),
        )
    });
    FILE.get()
}

impl log::Log for Logger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= self.level
    }

    fn flush(&self) {}

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
        let level = match record.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };
        let line = format!("{ts} [{level}] {}", record.args());
        eprintln!("{line}");
        if let Some(f) = log_file() {
            if let Ok(mut f) = f.lock() {
                use std::io::Write;
                let roll = f
                    .metadata()
                    .map(|m| m.len() > LOG_CAP)
                    .unwrap_or(false);
                if roll {
                    // Truncate-rollover (backupCount=0 semantics).
                    if let Ok(trunc) = std::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(LOG_FILE)
                    {
                        *f = trunc;
                    }
                }
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

/// Install once (subsequent calls are no-ops). Level from `RUST_LOG`.
pub fn init() {
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LevelFilter::Info);
    let logger = LOGGER.get_or_init(|| Logger { level });
    let _ = log::set_logger(logger);
    log::set_max_level(level);
}
