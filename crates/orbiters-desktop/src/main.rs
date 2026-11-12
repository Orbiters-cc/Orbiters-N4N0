//! Orbiters - N4N0: captures VRChat worlds (Spout frames with the avatar's pose strip) and what the avatar does
//! (avatar parameters, voice, sound, SteamVR tracking) into session bundles for Mixer. A small tray service
//! (controller, worker, OSC), a native egui window on wgpu in its own process while it is open, and a SteamVR
//! dashboard tab in another while SteamVR runs.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio_devices;
mod backend;
mod booster;
mod bench;
mod bridge;
mod capture_picture;
mod controller;
mod coverage_window;
mod device;
mod drag_out;
mod gpu;
mod harness;
mod idle_frames;
mod ipc;
mod keys;
mod language;
mod link_between;
mod link_smoke;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod mac_update;
mod mailbox;
mod media;
mod menu_host;
mod mixer_inbox;
mod overlay;
mod pages;
mod qr;
mod reconstruct;
mod screenshot;
mod service;
mod single;
mod splat_export;
mod startup;
mod steamvr_start;
mod state;
mod streams;
mod tracking;
mod tray;
mod upload_prep;
mod views;
mod worlds;
mod window;
mod words;
mod worker;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let private_roots = harness::prepare(&args)?;
    if args.iter().any(|a| a == "--say-state") {
        ipc::SAY_STATE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    orbiters_core::panic_log::install();
    let result = match args.first().map(String::as_str) {
        Some("--screenshot") => {
            // On a thread of its own: its table of scenarios and a page's frames outgrow the main thread's 1 MB stack in
            // debug builds (screenshot::run alone takes some 700 KB there).
            let args = args[1..].to_vec();
            std::thread::Builder::new()
                .name("screenshots".into())
                .stack_size(16 << 20)
                .spawn(move || screenshot::run(&args))
                .map_err(anyhow::Error::from)
                .and_then(|t| t.join().unwrap_or_else(|_| Err(anyhow::anyhow!("the screenshots stopped on a panic")))) // i18n: not shown (a developer's command line, in English)
        }
        // From RunOnce, at sign-in, after something died with a change of the FPS booster in force.
        Some(orbiters_booster::system::RESTORE_ARGUMENT) => controller::booster_restore_at_sign_in(),
        Some("--gpu-bench") => bench::run(&args[1..]),
        Some("--idle-frames") => idle_frames::run(&args[1..]),
        Some("--publish-cost") => controller::publish_cost::run(&args[1..]),
        Some("--link-smoke") => link_smoke::run(&args[1..]),
        Some("--window") => window::run(&args[1..]),
        Some("--overlay") => overlay::run(&args[1..]),
        #[cfg(windows)]
        Some("--orbit-desktop") => overlay::orbit::desktop::run(&args[1..]),
        #[cfg(windows)]
        Some("--orbit-bench") => overlay::orbit::desktop::bench(&args[1..]),
        #[cfg(windows)]
        Some("--orbit-paint-bench") => overlay::orbit::paint_bench::run(&args[1..]),
        Some("--n4n0-menu") => overlay::menu_pages::run(&args[1..]),
        Some("--world-view") => overlay::worlds::shots::run(&args[1..]),
        Some("--world-timing") => overlay::worlds::timing_lab::run(&args[1..]),
        Some("--n4n0-menu-strings") => overlay::menu_pages::run_strings(&args[1..]),
        Some("--prepare-upload") => upload_prep::run(&args[1..]),
        #[cfg(windows)]
        Some("--eye-picture") => overlay::eye_picture::run(&args[1..]),
        #[cfg(target_os = "macos")]
        Some("--apply-update") => mac_update::run(&args[1..]),
        // The VRChat bridge's helpers: a window's picture, what releases input if this app dies, and its lab.
        Some("--bridge-shot") => bridge::capture::run(&args[1..]),
        Some("--bridge-guard") => bridge::guard::run(),
        Some("--bridge-lab") => bridge::lab::run(&args[1..]),
        Some("--quit") => {
            single::signal(single::Request::Quit);
            Ok(())
        }
        _ => service::run(args.iter().any(|a| a == "--hidden")),
    };
    if let Some(private_roots) = private_roots { private_roots.cleanup()?; }
    result
}
