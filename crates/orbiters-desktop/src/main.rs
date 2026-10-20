//! Orbiters desktop: captures VRChat worlds (Spout frames with the avatar's pose strip) into session bundles for
//! Mixer. A native egui window on wgpu over a controller that keeps running from the tray.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod controller;
mod pages;
mod screenshot;
mod single;
mod state;
mod tray;
mod worker;

use orbiters_core::settings::Settings;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a request from another launch acts on, once the window exists.
struct Remote {
    ctx: egui::Context,
    ctl: controller::Ctl,
    quitting: Arc<AtomicBool>,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--screenshot") => return screenshot::run(&args[1..]),
        Some("--quit") => {
            single::signal(single::Request::Quit);
            return Ok(());
        }
        _ => {}
    }

    // A second launch shows the running window instead (or quits it with `--quit`).
    let remote: Arc<Mutex<Option<Remote>>> = Arc::default();
    let requests = remote.clone();
    let _instance = match single::acquire(move |request| {
        let guard = requests.lock().unwrap();
        let Some(r) = guard.as_ref() else { return };
        match request {
            single::Request::Show => tray::show_window(&r.ctx),
            single::Request::Quit => {
                r.quitting.store(true, Ordering::Relaxed);
                r.ctl.shutdown();
                r.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                r.ctx.request_repaint();
            }
        }
    })? {
        single::Instance::First(guard) => guard,
        single::Instance::Other => return Ok(()),
    };

    let settings = Settings::load();
    let _ = std::fs::create_dir_all(&settings.sessions_dir);
    let _ = std::fs::create_dir_all(&settings.raw_captures_dir);
    let ctl = controller::Controller::new(settings);
    ctl.start();
    let quitting = Arc::new(AtomicBool::new(false));

    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    // The interface is light: the integrated GPU (when there is one) leaves the other to VRChat.
    setup.power_preference = eframe::wgpu::PowerPreference::LowPower;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Orbiters desktop")
            .with_app_id("orbiters-desktop")
            .with_decorations(false)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([1040.0, 700.0])
            .with_icon(app::window_icon()),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration { wgpu_setup: setup.into(), ..Default::default() },
        ..Default::default()
    };
    let result = eframe::run_native(
        "Orbiters desktop",
        options,
        Box::new(move |cc| {
            *remote.lock().unwrap() = Some(Remote { ctx: cc.egui_ctx.clone(), ctl: ctl.clone(), quitting: quitting.clone() });
            Ok(Box::new(app::App::new(cc, ctl, quitting)))
        }),
    );
    result.map_err(|e| anyhow::anyhow!("{e}"))
}
