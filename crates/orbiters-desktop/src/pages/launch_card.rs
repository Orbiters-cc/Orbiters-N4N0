//! Settings' "Play VRChat" card: how Home's Play buttons start VRChat. One switch, "Performance launch options", and
//! each option under it, with the command line it makes in VR and on the desktop (`orbiters_core::vrchat_launch`).
use super::{section, Cx};
use egui::Ui;
use orbiters_core::launcher::LaunchMode;
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets;

fn priority_name(level: f32) -> String {
    match level.round() as i32 {
        0 => tr("Normal").into(),
        1 => tr("Above normal").into(),
        _ => tr("High").into(),
    }
}

/// The card; whether `draft` changed.
pub fn card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut apply = false;
    section(ui, cx, index, icons::PLAY, tr("Play VRChat"), Some(tr("How Home's Play buttons start VRChat")), |ui| {
        let launch = &mut draft.launch;
        #[cfg(target_os = "macos")]
        if !orbiters_core::osc::VRCHAT_RUNS_HERE {
            // A Mac: how many pixels VRChat draws, per side, of what the display draws; macOS scales the picture to the
            // screen. 100 % is the panel's own pixels, never more.
            let sizes = crate::mac_display::sizes();
            let mut level = launch.mac_render_percent.clamp(25, 100) as f32;
            let size = move |v: f32| match sizes {
                Some((backing, panel)) => {
                    // what the engine will make of it: capped at the panel, never under 640 x 480 (its own rules)
                    let (w, h) = orbiters_core::obelisk::render_size(v.round() as u32, backing, panel);
                    let applied = orbiters_core::obelisk::effective_percent(v.round() as u32, backing, panel);
                    format!("{applied} % · {w} × {h}") // i18n: not shown (a number and a size)
                }
                None => format!("{v:.0} %"), // i18n: not shown (a number)
            };
            widgets::field(
                ui,
                tr("Picture detail"),
                Some(tr("How many pixels VRChat draws on this Mac, per side; macOS scales the picture to the screen. Less is faster and softer. 100 % is the screen's own pixels. Applies at the next start.")),
                284.0,
                |ui| {
                    if widgets::slider(ui, "launch-mac-render", &mut level, 25.0..=100.0, 1.0, size) {
                        launch.mac_render_percent = level.round() as u32;
                        apply = true;
                    }
                },
            );
            return;
        }
        widgets::field(
            ui,
            tr("Performance launch options"),
            Some(tr("VRChat gets a little more of the processor, a frame cap, video decoding on the graphics card and, in VR, a tiny desktop window. Helps when the processor holds VRChat back.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "launch-performance", &mut launch.performance);
            },
        );
        if !launch.performance {
            return;
        }
        widgets::separator(ui);
        let mut level = launch.process_priority.clamp(0, 2) as f32;
        widgets::field(ui, tr("VRChat's priority"), Some(tr("Above normal is enough. High can take time from SteamVR and Virtual Desktop and make the headset stutter.")), 284.0, |ui| {
            if widgets::slider(ui, "launch-priority", &mut level, 0.0..=2.0, 1.0, priority_name) {
                launch.process_priority = level.round() as i8;
                apply = true;
            }
        });
        widgets::separator(ui);
        let mut level = launch.main_thread_priority.clamp(0, 2) as f32;
        widgets::field(ui, tr("Main thread priority"), Some(tr("The thread VRChat's world runs on.")), 284.0, |ui| {
            if widgets::slider(ui, "launch-main-thread", &mut level, 0.0..=2.0, 1.0, priority_name) {
                launch.main_thread_priority = level.round() as i8;
                apply = true;
            }
        });
        widgets::separator(ui);
        let mut fps = launch.fps.clamp(30, 240) as f32;
        widgets::field(ui, tr("Frame cap"), Some(tr("Frames per second on the desktop. In VR, the headset sets the pace.")), 284.0, |ui| {
            if widgets::slider(ui, "launch-fps", &mut fps, 30.0..=240.0, 1.0, |v| format!("{v:.0} FPS")) {
                launch.fps = fps.round() as u32;
                apply = true;
            }
        });
        widgets::separator(ui);
        widgets::field(ui, tr("Video decoding on the graphics card"), Some(tr("Videos in worlds cost the processor less.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "launch-hw-video", &mut launch.hw_video_decoding);
        });
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Tiny desktop window in VR"),
            Some(tr("A 100 × 100 window on the monitor while you play in VR, so it costs almost nothing. Turn it off to show or record the game on the monitor. Desktop mode keeps its window.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "launch-small-window", &mut launch.small_window_in_vr);
            },
        );
        widgets::separator(ui);
        // What VRChat is given, as it is.
        for (mode, label) in [(LaunchMode::Vr, tr("In VR")), (LaunchMode::Desktop, tr("On the desktop"))] {
            let line = orbiters_core::vrchat_launch::arguments(mode, launch, None).join(" ");
            let width = ui.available_width();
            ui.label(Txt::new(format!("{label}: {line}"), 11.5).mono().color(theme::fg_3()).job(width)); // i18n: not shown (VRChat's options)
        }
    });
    apply
}
