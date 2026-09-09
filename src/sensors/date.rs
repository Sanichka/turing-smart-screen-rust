// SPDX-License-Identifier: GPL-3.0-or-later
//! Local date/time. Ports `stats.Date` (minus `babel` locale bloat:
//! theme-level locale formatting lands with render in Step 4; the snapshot
//! carries ISO + epoch and the renderer formats per `FORMAT` key).

/// `(iso_local, epoch_secs)`, e.g. `("2026-09-08T12:34:56+02:00", 178...30)`.
pub fn now() -> (String, i64) {
    let t = chrono::Local::now();
    (t.to_rfc3339(), t.timestamp())
}
