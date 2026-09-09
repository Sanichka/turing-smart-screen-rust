// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal stderr logger. Replaces `env_logger` (~regex+jiff, MBs) with ~60
//! lines: `RUST_LOG` level filter (`off/error/warn/info/debug/trace`,
//! default `info`), `timestamp [LEVEL] message` lines on stderr.

use log::{Level, LevelFilter, Metadata, Record};
use std::sync::OnceLock;

struct Logger {
    level: LevelFilter,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

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
        eprintln!("{ts} [{level}] {}", record.args());
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
