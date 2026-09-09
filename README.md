# ![Icon](./res/icons/monitor-icon-17865/24.png) turing-smart-screen-rust

> [!NOTE]
> This is a **Rust port / fork** of [mathoudebine/turing-smart-screen-python](https://github.com/mathoudebine/turing-smart-screen-python) by Oleksandr Koval — same themes, same `config.yaml` format, a fraction of the footprint.
> Hardware questions (which screen is which) are still best answered by the [upstream wiki](https://github.com/mathoudebine/turing-smart-screen-python/wiki/Hardware-revisions); Rust bugs belong [here](https://github.com/Sanichka/turing-smart-screen-rust/issues).

> [!WARNING]
>
> This project is **not affiliated, associated, authorized, endorsed by, or in any way officially connected with Turing / XuanFang / Kipye brands**, or any of theirs subsidiaries, affiliates, manufacturers or sellers of their products. All product and company names are the registered trademarks of their original owners.
>
> This project is an open-source alternative software, NOT the original software provided for the smart screens. **Please do not open issues for USBMonitor.exe/ExtendScreen.exe or for the smart screens hardware here**.

![Windows](https://img.shields.io/badge/Windows%2010%2F11%20tested-0078D6?style=for-the-badge&logoColor=white) ![Linux](https://img.shields.io/badge/Linux%20CI%20only-FCC624?style=for-the-badge&logo=linux&logoColor=black) ![macOS](https://img.shields.io/badge/macOS%20CI%20only-000000?style=for-the-badge&logo=apple&logoColor=white) ![Rust](https://img.shields.io/badge/Rust-stable-CE422B?style=for-the-badge&logo=rust&logoColor=white) [![Licence](https://img.shields.io/github/license/Sanichka/turing-smart-screen-rust?style=for-the-badge)](./LICENSE)

A Rust system monitor program and hardware drivers for **small IPS USB displays** — ~0.04% CPU, ~30 MB RAM, one 2.5 MB exe.

Supported operating systems: Windows (tested on hardware), Linux and macOS (compile + headless render in CI, no hardware validation yet).

### ✅ Supported smart screens models:

| ✅ Turing Smart Screen 3.5" / UsbPCMonitor 3.5" / 5" (revision A — tested on real hardware) | ✅ Simulated display (renders to `screencap.png` + live `:5678` preview) |
|---------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------|
| <img src="res/docs/turing.webp" width="30%" height="30%"/> <img src="res/docs/turing5inch.png" width="30%" height="30%"/>                     | No hardware needed for theming or CI                                                                                   |

| ❌ Other revisions (B/C/D/TUR_USB/WEACT) |
|------------------------------------------|
| Present in `config.yaml` but **not ported** — the driver factory rejects them with a clear error. XuanFang, Kipye, WeAct, Turing USB and all other sizes from the upstream gallery below are therefore not usable with this fork yet. |

<details>

<summary><h3>Upstream hardware gallery (reference only — most models not supported here)</h3></summary>

Turing 2.1" / 2.8" / 4.6" / 5.2" / 8.0" / 8.8" / 9.2" / 12.3", XuanFang 3.5", Kipye Qiye 3.5", WeAct 0.96"/3.5" — see the [upstream README](https://github.com/mathoudebine/turing-smart-screen-python#readme) and the [hardware revisions wiki](https://github.com/mathoudebine/turing-smart-screen-python/wiki/Hardware-revisions) for photos and identification.

</details>

### [> What is my smart screen model?](https://github.com/mathoudebine/turing-smart-screen-python/wiki/Hardware-revisions)

If you haven't received your screen yet but want to start developing your theme now, use **`REVISION: SIMU`** and open `http://localhost:5678` for the live preview.

## How to start

Prerequisites: [Rust stable toolchain](https://rustup.rs/), Windows 10/11 for hardware use.

```powershell
# 1. Build
cargo build --release
# 2. (admin, once) CPU temp/fan driver for Windows:
external\PawnIO\PawnIO_setup.exe
# 3. Run (elevated shell for temp/fan widgets)
$env:RUST_LOG="info"; .\target\release\turing-smart-screen.exe --daemon --com COM4
```

Or assemble the portable folder and install the logon task:

```powershell
powershell -ExecutionPolicy Bypass -File dist.ps1   # -> dist/turing-smart-screen-rust/
# then optional: compile tools/windows-installer/turing-smart-screen-rust.iss with Inno Setup
```

Settings live in `config.yaml` (same schema as upstream) and can be edited with the bundled GUI:

```powershell
.\target\release\turing-configure.exe
```

There are 2 programs in this fork:
* **`turing-smart-screen --daemon`**, the system monitor (see below).
* **`turing-configure`**, the native settings GUI (display model/size, COM port, theme, sensors, weather & ping, save + relaunch).

## System monitor

A complete standalone program that turns your screen into a live system monitor using upstream-compatible themes.

* 1 Hz tick with tile-diffed serial updates (only changed screen regions are sent), ~0.04% CPU on a 24-core box.
* Display configuration via GUI or `config.yaml`: no code to edit.
* Sensors: CPU %/freq/load, CPU temperature (ring-0 SMN on AMD Zen / DTS on Intel, needs driver + elevation on Windows; hwmon on Linux), CPU fan (SuperIO tachometers on Windows, hwmon on Linux), NVIDIA GPU (NVML), memory, disks, network rates, date/time (babel-style formats), uptime, ping, OpenWeatherMap weather (needs API key), and compiled-in custom data sources.
* Known simplifications vs upstream: per-widget refresh intervals are unified to the 1 Hz tick (slow sensors stay on their own cadence); GPU FPS, AMD-on-Windows GPU and Intel iGPU have no data source yet.
* Sleep/wake aware (panel blanks on suspend, full repaint on resume), tray icon with Configure/Exit, rotating `log.log`, graceful Ctrl-C drain.
* Auto-detect COM port; HELLO sub-model detection for 3.5"/5"/7" UsbPCMonitor panels.

### [> List and preview of included themes](res/themes/themes.md)
<img src="res/themes/3.5inchTheme2/preview.png" height="150" /> <img src="res/themes/Terminal/preview.png" height="150" /> <img src="res/themes/Cyberpunk-net/preview.png" height="150" /> <img src="res/themes/bash-dark-green-gpu/preview.png" height="150" /> <img src="res/themes/Landscape6Grid/preview.png" width="150" /> <img src="res/themes/LandscapeMagicBlue/preview.png" width="150" /> <img src="res/themes/LandscapeEarth/preview.png" width="150" /> ... [view full list](res/themes/themes.md)
### Themes creation/edition
Themes are plain `theme.yaml` files (same schema as upstream — most upstream themes render as-is). Live-preview a theme without hardware:
```powershell
.\target\release\turing-smart-screen.exe --render-once --theme <name>   # writes screencap.png
.\target\release\turing-smart-screen.exe --theme-screenshots 10         # batch mode for previews/CI
```
### [> Themes shared by the community](https://github.com/mathoudebine/turing-smart-screen-python/discussions/categories/themes)
Upstream theme collection (compatible format) — share Rust-specific findings in [this fork's issues](https://github.com/Sanichka/turing-smart-screen-rust/issues).

## Control the display from your own code

There is no separate Python-style module API (yet) — hardware access lives in the `DisplayDriver` trait (`src/display/`, Rev A + simulated). The closest to `simple-program.py` today:
```powershell
turing-smart-screen.exe --send-test --com COM4   # init + paint one live frame, leave it on
```

## Troubleshooting
Rust problems: [fork issues](https://github.com/Sanichka/turing-smart-screen-rust/issues) (please attach `log.log` and your theme name).
Hardware identification and theme authoring: [upstream wiki](https://github.com/mathoudebine/turing-smart-screen-python/wiki) (Python-specific setup steps do not apply here).
