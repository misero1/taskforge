//! TaskForge — a modern Task Manager for Windows, written in Rust.
// Hide the console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod automation;
mod diag;
mod diagnose;
mod insights;
mod longterm;
mod monitor;
mod native;
mod platform;
mod repair;
mod rules;
#[cfg(windows)]
mod tray;
mod ui;

use eframe::egui;

fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()
}

fn main() -> eframe::Result<()> {
    diag::init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--uninstall") {
        let quiet = args.iter().any(|a| a == "--quiet");
        match platform::uninstall() {
            Ok(msg) => {
                if !quiet {
                    platform::message_box("TaskForge", &msg);
                }
                platform::schedule_install_dir_removal();
            }
            Err(e) => platform::message_box("TaskForge", &format!("Uninstall failed: {e}")),
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TaskForge")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            .with_icon(icon()),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "TaskForge",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
