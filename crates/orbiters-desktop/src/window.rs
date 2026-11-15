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
    // What a Library move interrupted by closing the window left, on a worker: never in the way of starting. Here and not
    // in the service, which goes on running after its window closed: a move runs in this process, so closing the window is
    // what ends one, and opening it again is the next start. Screenshot runs and benches never come through here.
    // Not in a process given a folder of its own (`--sandbox`, by `--link-smoke`): it knows none of the person's
    // projects, and must not judge their caches.
    let sandboxed = args.iter().any(|a| a == "--sandbox");
    let _ = (!sandboxed).then(|| std::thread::Builder::new().name("unity-cache-orphans".into()).spawn(|| {
        orbiters_projects::discovery::low_priority();
        let swept = orbiters_projects::storage::remove_orphans(&std::sync::atomic::AtomicBool::new(false));
        for cache in &swept.removed {
            eprintln!("removed the cache an interrupted repair left: {}", cache.display());
        }
        for (path, why) in &swept.failed {
            eprintln!("could not remove {}: {why}", path.display());
        }
    }));
    #[cfg(target_os = "macos")]
    crate::mac::stay_awake();
    let backend: Ctl = Arc::new(Remote(Client::connect(&endpoint, Role::Window)?));
    crate::usage::forward(backend.clone());
    let viewport = egui::ViewportBuilder::default()
        .with_title("Orbiters - N4N0")
        .with_app_id("orbiters-desktop")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([1040.0, 700.0])
        .with_icon(app::window_icon());
    // `--link-smoke` starts a window to read what its link holds, on a computer someone may be using: it does not take
    // the foreground. The service never passes this.
    let viewport = if args.iter().any(|a| a == "--no-focus") { viewport.with_active(false) } else { viewport };
    // The app draws its own title bar. On Windows and elsewhere, a window without decorations. On the Mac such a window
    // would lose its rounded corners, shadow and edges: a titled window keeps them, with its title bar's every part
    // hidden under the app's.
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_decorations(false);
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false).with_titlebar_buttons_shown(false);
    let options = eframe::NativeOptions {
        // The application is an agent to macOS (Info.plist's LSUIElement: nothing in the Dock for the service alone): its
        // window is what makes it one with a Dock tile and a menu bar. Set by winit, when AppKit is ready for it.
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Regular);
            // Its menu is ours (mac.rs).
            builder.with_default_menu(false);
        })),
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        // The GPU the settings choose: by default the integrated one, which keeps the 3D view off the game's graphics card.
        wgpu_options: gpu::options(backend.view().settings.window_gpu),
        // `orbiters_ui::install` brings its own (the system's fonts for other scripts, opened as needed): eframe's
        // stand-in would only log that it has none.
        system_font_fallback: false,
        ..Default::default()
    };
    eframe::run_native("Orbiters - N4N0", options, Box::new(move |cc| Ok(Box::new(App::new(cc, backend, splash))))).map_err(|e| anyhow::anyhow!("{e}"))
}
