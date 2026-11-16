#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::path::PathBuf;
use twinforge::ui::{self, Options};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    // Everything that prints and exits instead of opening the window.
    if has("--help")
        || has("-h")
        || has("/?")
        || has("--version")
        || has("--scan")
        || has("--selftest")
    {
        twinforge::sys::attach_console();
        std::process::exit(twinforge::cli::run(&args));
    }

    if has("--selftest-ui") {
        twinforge::sys::attach_console();
        let result = twinforge::selftest::run_ui(|root, app_dir, log| {
            let options = Options {
                paths: vec![root],
                app_dir: Some(app_dir),
                autopilot: Some(log),
                ..Default::default()
            };
            ui::run(options, window_icon()).map_err(|e| e.to_string())
        });
        match result {
            Ok(log) => {
                for line in log {
                    println!("{line}");
                }
                println!("SELFTEST-UI PASSED");
                std::process::exit(0);
            }
            Err(e) => {
                println!("{e}");
                println!("SELFTEST-UI FAILED");
                std::process::exit(1);
            }
        }
    }

    // Plain arguments are folders to open with (e.g. from "Open with" or a shortcut).
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
        } else if matches!(a.as_str(), "--screenshot" | "--view" | "--pose") {
            skip = true;
        } else if !a.starts_with("--") && std::path::Path::new(a).is_dir() {
            paths.push(PathBuf::from(a));
        }
    }

    let options = Options {
        demo: has("--demo"),
        screenshot: value("--screenshot").map(PathBuf::from),
        view: value("--view").or_else(|| std::env::var("TWINFORGE_VIEW").ok()),
        pose: value("--pose"),
        light: has("--light"),
        paths,
        ..Default::default()
    };
    ui::run(options, window_icon())
}

fn window_icon() -> egui::IconData {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon-256.png"));
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}
