// SPDX-License-Identifier: GPL-3.0-or-later
//! Turing Smart Screen system monitor (Rust port).
//! Thin binary: all logic lives in the library so the GUI tools
//! (`turing-configure`, `turing-theme-editor`) can share it.

fn main() {
    turing_smart_screen_rust::cli::run();
}
