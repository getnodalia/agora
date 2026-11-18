//! Ágora's desktop client: native egui, drawn in the Nodalia Design Language.
//!
//! `AGORA_DESKTOP_DIR` stores settings and the session somewhere else than the platform's
//! config folder, which is how two clients run side by side on one computer.

// A window app on Windows: no console opens beside it.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod app;
mod chrome;
mod dialogs;
mod i18n;
mod icon;
#[cfg(target_os = "linux")]
mod linux_menu;
mod model;
mod net;
mod security;
mod settings;
mod signin;
mod updates;
mod view;
mod widgets;

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("agora {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ => {
                println!("Usage: agora\n\nThe server address is chosen on the sign-in screen.\nAGORA_DESKTOP_DIR=<folder> keeps settings and the session in <folder>.");
                return ExitCode::SUCCESS;
            }
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let settings = settings::Settings::load(settings::dir().as_deref());
    let ndl_controls = chrome::NDL_CONTROLS_POSSIBLE
        && settings.window_controls == settings::WindowControls::Nodalia;
    let mut viewport = chrome::frame(
        egui::ViewportBuilder::default()
            .with_title("Ágora")
            .with_app_id("dev.danielmiguelt.Agora")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 480.0]),
        ndl_controls,
    );
    if let Some(icon) = icon::window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Ágora",
        options,
        Box::new(|cc| match app::App::new(cc) {
            Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
            Err(e) => Err(Box::new(e)),
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agora: {e}");
            ExitCode::FAILURE
        }
    }
}
