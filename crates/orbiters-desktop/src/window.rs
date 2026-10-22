//! The window process (`orbiters-desktop --window --service <port> <token>`): the egui window on wgpu, fed by
//! the tray service over the IPC link. It exists only while the window is open; closing it frees its GPU memory.

use crate::app::{self, App};
use crate::backend::{Ctl, Remote};
use crate::gpu;
use crate::ipc::{Client, Endpoint, Role};
use anyhow::Context as _;
use std::sync::Arc;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let endpoint = Endpoint::from_args(args).context("--window needs --service <port> <token>")?;
    let splash = args.iter().any(|a| a == "--splash");
    let backend: Ctl = Arc::new(Remote(Client::connect(&endpoint, Role::Window)?));
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
        // While VRChat runs, on the integrated GPU: off the game's graphics card.
        wgpu_options: gpu::options(backend.view().vrchat.running),
        // `orbiters_ui::install` brings its own (the system's fonts for other scripts, opened as needed): eframe's
        // stand-in would only log that it has none.
        system_font_fallback: false,
        ..Default::default()
    };
    eframe::run_native("Orbiters desktop", options, Box::new(move |cc| Ok(Box::new(App::new(cc, backend, splash))))).map_err(|e| anyhow::anyhow!("{e}"))
}
