// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared library root: config, sensors, render, display, daemon and GUI
//! tools all build on these modules. The daemon binary (`src/main.rs`) and
//! the `turing-configure` / `turing-theme-editor` tools link here.

pub mod cli;
pub mod config;
pub mod daemon;
pub mod display;
pub mod logger;
pub mod power;
pub mod render;
pub mod sensors;
pub mod tray;
