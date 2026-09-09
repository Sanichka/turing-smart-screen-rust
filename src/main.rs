// SPDX-License-Identifier: GPL-3.0-or-later
//! Daemon entry point (Step 1: config load + `--dump-config` only).
//!
//! Init order mirrors `main.py`: config -> theme -> orientation/dims.
//! Sensor/render/serial threads land in Steps 2-5.

mod config;
mod daemon;
mod display;
mod logger;
mod render;
mod sensors;
mod tray;

use std::path::PathBuf;

use config::{load_app_config, load_theme};
use sensors::{NetSelection, Provider, SlowCtx};

#[derive(Debug, Default)]
struct Args {
    config_path: PathBuf,
    themes_root: PathBuf,
    default_theme_file: PathBuf,
    theme_override: Option<String>,
    com_override: Option<String>,
    tick_ms: u64,
    dump_config: bool,
    sensors_once: bool,
    render_once: bool,
    daemon: bool,
    send_test: bool,
}

fn parse_args() -> Result<Args, String> {
    // Minimal hand-rolled parser: keeps Step-1 binary free of clap bloat
    // (daemon hot path must stay lean; arg parsing runs once at startup).
    let mut args = Args {
        config_path: PathBuf::from("config.yaml"),
        themes_root: PathBuf::from("res/themes"),
        default_theme_file: PathBuf::from("res/themes/default.yaml"),
        tick_ms: 1000,
        ..Default::default()
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dump-config" => args.dump_config = true,
            "--sensors-once" => args.sensors_once = true,
            "--render-once" => args.render_once = true,
            "--daemon" => args.daemon = true,
            "--send-test" => args.send_test = true,
            "--com" => {
                args.com_override = Some(
                    it.next()
                        .ok_or_else(|| "missing value for --com".to_string())?,
                );
            }
            "--tick-ms" => {
                let v = it
                    .next()
                    .ok_or_else(|| "missing value for --tick-ms".to_string())?;
                args.tick_ms = v
                    .parse()
                    .map_err(|_| "invalid --tick-ms (milliseconds)".to_string())?;
                if args.tick_ms == 0 {
                    return Err("invalid --tick-ms (must be > 0)".to_string());
                }
            }
            "--config" => {
                args.config_path = PathBuf::from(
                    it.next()
                        .ok_or_else(|| "missing value for --config".to_string())?,
                );
            }
            "--themes-root" => {
                args.themes_root = PathBuf::from(
                    it.next()
                        .ok_or_else(|| "missing value for --themes-root".to_string())?,
                );
            }
            "--default-theme" => {
                args.default_theme_file = PathBuf::from(
                    it.next()
                        .ok_or_else(|| "missing value for --default-theme".to_string())?,
                );
            }
            "--theme" => {
                args.theme_override = Some(
                    it.next()
                        .ok_or_else(|| "missing value for --theme".to_string())?,
                );
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}\nRun with --help.")),
        }
    }
    Ok(args)
}

fn print_help() {
    println!(
        "turing-smart-screen (rust, step 5)\n\
         Usage:\n  \
           turing-smart-screen --dump-config [--config <path>] [--theme <name>]\n  \
           turing-smart-screen --sensors-once [--config <path>]\n  \
           turing-smart-screen --render-once [--config <path>] [--theme <name>]\n  \
           turing-smart-screen --daemon [--config <path>] [--theme <name>] [--com <port>] [--tick-ms <n>]\n  \
           turing-smart-screen --send-test [--config <path>] [--theme <name>] [--com <port>]\n  \
           turing-smart-screen --help\n\
         \n\
         --dump-config   load config.yaml + theme and print resolved display params\n\
         --sensors-once  take one sensor snapshot and print it as JSON\n\
         --render-once   render one frame to screencap.png (SIMU parity)\n\
         --daemon        run the monitor loop (Ctrl-C stops, panel blanks)\n\
         --send-test     init display + paint one frame, leave panel on"
    );
}

fn main() {
    logger::init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    if !args.dump_config
        && !args.sensors_once
        && !args.render_once
        && !args.daemon
        && !args.send_test
    {
        eprintln!("pass --dump-config, --sensors-once, --render-once, --daemon or --send-test (see --help).");
        std::process::exit(2);
    }

    let cfg = match load_app_config(&args.config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    if args.sensors_once {
        let nets = NetSelection {
            eth: cfg.general.eth.clone(),
            wlo: cfg.general.wlo.clone(),
        };
        let slow = SlowCtx::from_config(&cfg.general);
        let mut provider = Provider::from_hw(cfg.general.hw_sensors, &cfg.general.cpu_fan);
        // Mirrors `stats.Gpu.is_available()` gate in main.py (scheduler only
        // starts GpuStats when a GPU was detected).
        log::info!("gpu_available={}", provider.gpu_available());
        let snap = provider.snapshot(&nets, &slow);
        println!("{}", serde_json::to_string_pretty(&snap.to_json()).unwrap());
        return;
    }

    if !args.dump_config && !args.render_once && !args.daemon && !args.send_test {
        eprintln!("pass --dump-config, --render-once, --daemon or --send-test (see --help).");
        std::process::exit(2);
    }
    let theme_name = args
        .theme_override
        .as_deref()
        .unwrap_or(&cfg.general.theme);

    let mut theme =
        match load_theme(&args.themes_root, &args.default_theme_file, theme_name) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        };
    if !["portrait", "landscape"].contains(&theme.display_orientation.to_ascii_lowercase().as_str())
    {
        log::warn!(
            "unknown DISPLAY_ORIENTATION '{}', using portrait",
            theme.display_orientation
        );
    }
    theme.resolve_orientation(cfg.display.display_reverse);

    if args.daemon || args.send_test {
        let dargs = daemon::DaemonArgs {
            com_override: args.com_override,
            tick: std::time::Duration::from_millis(args.tick_ms),
            test_once: args.send_test,
        };
        std::process::exit(daemon::run(&cfg, &theme, &dargs));
    }

    if args.render_once {
        render_once(&cfg, &theme);
        return;
    }

    let (ow, oh) = theme.oriented_dims();

    println!("config: {}", args.config_path.display());
    println!("theme: {} ({})", theme.name, theme.dir.display());
    println!("revision: {:?}", cfg.display.revision);
    println!("com_port: {}", cfg.general.com_port);
    println!("hw_sensors: {:?}", cfg.general.hw_sensors);
    println!("orientation: {:?}", theme.orientation);
    println!("theme_size: {:?}", theme.display_size.as_deref().unwrap_or("<unset>"));
    println!("dims_native: {}x{}", theme.width, theme.height);
    println!("dims_oriented: {ow}x{oh}");
    println!("brightness: {}", cfg.display.brightness);
    println!("display_reverse: {}", cfg.display.display_reverse);
    println!("reset_on_startup: {}", cfg.display.reset_on_startup);
    println!(
        "rgb_led: {}, {}, {}",
        theme.display_rgb_led[0], theme.display_rgb_led[1], theme.display_rgb_led[2]
    );
    println!("static_images: {}", theme.static_images.len());
    for (k, v) in theme.static_images.iter() {
        println!("  image {k}: {} @ {},{} {}x{}", v.path, v.x, v.y, v.width, v.height);
    }
    println!("static_texts: {}", theme.static_texts.len());
    for (k, v) in theme.static_texts.iter() {
        println!(
            "  text {k}: {:?} @ {},{} font={}({})",
            v.text, v.x, v.y, v.font, v.font_size
        );
    }
    // Raw STATS keys present (proves default.yaml merge worked, Step 4 types it).
    if let Some(stats) = theme.raw.get("STATS") {
        if let Some(map) = stats.as_mapping() {
            let keys: Vec<String> = map
                .keys()
                .filter_map(|k| k.as_str().map(|s| s.to_string()))
                .collect();
            println!("stats_sections: {}", keys.join(", "));
        }
    }
}

/// Single-frame render to `screencap.png` (parity with `REVISION: SIMU`).
fn render_once(cfg: &config::AppConfig, theme: &config::Theme) {
    use render::{ImageCache, Renderer};

    let nets = NetSelection {
        eth: cfg.general.eth.clone(),
        wlo: cfg.general.wlo.clone(),
    };
    let slow = SlowCtx::from_config(&cfg.general);
    let mut provider = Provider::from_hw(cfg.general.hw_sensors, &cfg.general.cpu_fan);
    log::info!("gpu_available={}", provider.gpu_available());
    let snap = provider.snapshot(&nets, &slow);

    // Native theme dims (orientation is a hardware command in Python too;
    // SIMU renders in theme space as well).
    let mut r = Renderer::new(theme.width, theme.height, PathBuf::from("res/fonts"));
    let imgs = ImageCache::preload(theme);
    r.draw_static(theme, &imgs);
    r.draw_snapshot(theme, &imgs, &snap);
    if std::env::var("DUMP_DIRTY").is_ok() {
        for d in &r.dirty {
            println!("dirty {} {} {} {}", d.x0, d.y0, d.x1, d.y1);
        }
    }
    match r.fb.save_png("screencap.png") {
        Ok(()) => println!(
            "wrote screencap.png ({}x{}, {} dirty rects)",
            theme.width,
            theme.height,
            r.dirty.len()
        ),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
