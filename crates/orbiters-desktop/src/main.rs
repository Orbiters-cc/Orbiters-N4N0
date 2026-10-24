//! Orbiters - N4N0: captures VRChat worlds (Spout frames with the avatar's pose strip) and what the avatar does
//! (avatar parameters, voice, sound, SteamVR tracking) into session bundles for Mixer. A small tray service
//! (controller, worker, OSC), a native egui window on wgpu in its own process while it is open, and a SteamVR
//! dashboard tab in another while SteamVR runs.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod controller;
mod gpu;
mod ipc;
mod mailbox;
mod media;
mod menu_host;
mod mixer_inbox;
mod overlay;
mod pages;
mod reconstruct;
mod screenshot;
mod service;
mod single;
mod startup;
mod state;
mod streams;
mod tray;
mod views;
mod worlds;
mod window;
mod worker;

fn main() -> anyhow::Result<()> {
    orbiters_core::panic_log::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--screenshot") => screenshot::run(&args[1..]),
        Some("--window") => window::run(&args[1..]),
        Some("--overlay") => overlay::run(&args[1..]),
        Some("--orb-menu") => overlay::menu_pages::run(&args[1..]),
        #[cfg(windows)]
        Some("--eye-picture") => overlay::eye_picture::run(&args[1..]),
        Some("--quit") => {
            single::signal(single::Request::Quit);
            Ok(())
        }
        _ => service::run(args.iter().any(|a| a == "--hidden")),
    }
}
