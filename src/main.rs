// Release builds on Windows open no console window; debug builds keep it for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod diff;
mod diffview;
mod git;
mod graph;
mod picker;
mod recent;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    let dir = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("gitgoo").with_inner_size([1600.0, 900.0]).with_app_id("gitgoo"),
        ..Default::default()
    };
    eframe::run_native("gitgoo", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, dir)))))
}
