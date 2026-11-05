//! Settings page: the Orbiters account, capture, storage, VRChat and OSC, performance, starting, interface, about. Changes apply as they are made (shown at
//! once, saved in the background); text fields apply with their Save button (see [`super::form`]).

use super::form::{self, Texts};
use super::mic::{mic_test, MicTest};
use super::{section, Cx, Job};
use crate::backend::Ctl;
use crate::ipc::UpdateAction;
use crate::state::{AccountState, Snapshot, UpdateState};
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder};
use orbiters_core::settings::{CoverageOverlay, RigViews, ViewGpu, WhilePlaying, WindowGpu};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Variant};
use orbiters_ui::{format, icons, toast};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    Sessions,
    Raw,
}

#[derive(Default)]
pub struct State {
    vrchat: VrchatForm,
    /// The Spout sender and the record parameter as typed, until saved.
    texts: Texts,
    log_open: bool,
    picker: Option<(Folder, Job<Option<PathBuf>>)>,
    mic: MicTest,
    /// "Sign out" was pressed once: it asks before signing every tool out.
    confirm_sign_out: bool,
    /// Whether Windows starts the app: read when the page opens (on the visit it was read), set at once by its switch
    /// while the registry is written.
    windows_start: Option<bool>,
    windows_read: u64,
    windows_job: Option<Job<Result<(), String>>>,
    /// The start with SteamVR as switched, until the settings say so, and SteamVR being told meanwhile.
    steamvr_shown: Option<bool>,
    steamvr_job: Option<(bool, Job<Result<bool, String>>)>,
    /// "When SteamVR starts": Windows' sound devices in VR.
    vr_audio: super::vr_audio::State,
    /// VRChat on the network, where it does not run on this computer.
    network: super::network::State,
    /// Experimental: the VRChat bridge.
    experimental: super::experimental::State,
}

impl State {
    /// Ends the mic test (leaving the page).
    pub fn stop_mic_test(&mut self) {
        self.mic = MicTest::default();
        self.vr_audio.stop();
        self.vrchat.password.clear();
        self.vrchat.code.clear();
    }
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    // The controls edit a copy of the settings as published; what they change is saved over the settings as they
    // are by then (`form::save`).
    let base = cx.snap.settings.clone();
    let mut draft = base.clone();
    let mut apply = false;

    // A folder chosen in the picker.
    if let Some((which, job)) = state.picker.as_mut() {
        if let Some(choice) = job.take() {
            if let Some(dir) = choice {
                match which {
                    Folder::Sessions => draft.sessions_dir = dir,
                    Folder::Raw => draft.raw_captures_dir = dir,
                }
                apply = true;
            }
            state.picker = None;
        }
    }

    // Starting with Windows: read on each visit; a write that failed shows what is.
    if state.windows_read != cx.visit {
        state.windows_read = cx.visit;
        state.windows_start = Some(crate::startup::starts_with_windows());
    }
    if let Some(result) = state.windows_job.as_mut().and_then(Job::take) {
        state.windows_job = None;
        if let Err(e) = result {
            cx.toasts.push(&ctx, toast::Kind::Error, format!("The start with {} did not change: {e}", crate::words::SYSTEM));
            state.windows_start = Some(crate::startup::starts_with_windows());
        }
    }
    // Starting with SteamVR: told to SteamVR when it runs, kept for its next start otherwise.
    if let Some((on, result)) = state.steamvr_job.as_mut().and_then(|(on, job)| job.take().map(|r| (*on, r))) {
        state.steamvr_job = None;
        match result {
            Ok(told) => {
                draft.startup.steamvr = on;
                draft.startup.steamvr_pending = !told;
                apply = true;
            }
            Err(e) => {
                state.steamvr_shown = None;
                cx.toasts.push(&ctx, toast::Kind::Error, format!("SteamVR did not take the change: {e}"));
            }
        }
    }
    if state.steamvr_job.is_none() && state.steamvr_shown == Some(base.startup.steamvr) {
        state.steamvr_shown = None;
    }

    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            super::block(ui, cx, 0, 72.0, |ui, rect| super::heading(ui, rect, "Preferences", "Settings", None));
            let snap = cx.snap;
            let mut m = super::Masonry::new(ui, "settings");
            m.add(1, |ui| account(ui, cx, &mut state.confirm_sign_out));
            m.add(10, |ui| vrchat_account(ui, cx, &mut state.vrchat));
            // Only for an account the server says has it: nobody else sees a trace of it.
            if snap.bridge.allowed {
                m.add(11, |ui| apply |= super::experimental::show(ui, cx, 11, &mut state.experimental, &mut draft.experimental));
            }
            // What VRChat on this very computer gives (Spout, its log, SteamVR) has no settings where it cannot run.
            let vrchat_here = orbiters_core::osc::VRCHAT_RUNS_HERE;
            if !vrchat_here {
                m.add(12, |ui| apply |= super::network::card(ui, cx, 12, &mut state.network, &mut draft));
            }

            if vrchat_here {
            m.add(2, |ui| section(ui, cx, 2, icons::GAUGE, "Capture", Some("How frames become a session"), |ui| {
                let kf = &mut draft.keyframes;
                widgets::field(ui, "Keyframe distance", Some("A new lossless keyframe after the camera moves this far."), 284.0, |ui| {
                    apply |= widgets::slider(ui, "kf-distance", &mut kf.min_translation, 0.05..=2.0, 0.05, |v| format!("{v:.2} m"));
                });
                widgets::separator(ui);
                widgets::field(ui, "Keyframe turn", Some("…or after it turns this much."), 284.0, |ui| {
                    apply |= widgets::slider(ui, "kf-turn", &mut kf.min_rotation_deg, 2.0..=60.0, 1.0, |v| format!("{v:.0}°"));
                });
                widgets::separator(ui);
                widgets::field(ui, "Keyframe interval", Some("…or after this long, whatever happens."), 284.0, |ui| {
                    apply |= widgets::slider(ui, "kf-interval", &mut kf.max_interval_s, 0.25..=10.0, 0.25, |v| format!("{v:.2} s"));
                });
                widgets::separator(ui);
                widgets::field(ui, "New session on world change", Some("Each VRChat world gets its own session while you record."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "split", &mut draft.split_on_world_change);
                });
                widgets::separator(ui);
                let placeholder = snap.sources.first().map(|s| s.name.clone()).unwrap_or_else(|| "VRChat".into());
                widgets::field(ui, "Spout sender", Some("Leave empty to pick VRChat's Stream camera automatically."), 290.0, |ui| {
                    if let Some(text) = form::text_row(ui, &mut state.texts, "spout-sender", &base.spout_sender, &placeholder, 208.0, false) {
                        draft.spout_sender = text;
                        apply = true;
                    }
                });
                if !snap.sources.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                        for s in &snap.sources {
                            if source_chip(ui, &format!("{} · {}×{}", s.name, s.width, s.height), draft.spout_sender == s.name).clicked() {
                                draft.spout_sender = s.name.clone();
                                state.texts.forget("spout-sender");
                                apply = true;
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                widgets::separator(ui);
                let places = [
                    SegOption::new(WhilePlaying::GraphicsCard, "Graphics card")
                        .hint("Fastest; in VR it trains in the time VRChat's frames leave idle, so slower while the game is busy"),
                    SegOption::new(WhilePlaying::IntegratedGpu, "Integrated GPU")
                        .hint("About 40 times slower, and VRChat keeps its frame rate; once VRChat closes it starts again on the graphics card"),
                    SegOption::new(WhilePlaying::AfterVrchat, "After VRChat").hint("Reconstructions wait until VRChat closes or the headset has been off a minute"),
                ];
                widgets::field(
                    ui,
                    "Reconstruct while VRChat runs",
                    Some("On the graphics card training gives way to VRChat's frames; the processor's integrated GPU leaves the game alone but is about 40 times slower."),
                    360.0,
                    |ui| {
                        apply |= widgets::segmented(ui, "while-playing", &mut draft.reconstruct_while_playing, &places, true);
                    },
                );
            }));
            }

            let st = &snap.storage;
            let mut free_space = false;
            let storage_sub = format!(
                "{} in {} sessions{}",
                format::bytes(st.used_bytes as f64),
                st.sessions,
                st.free_bytes.map(|f| format!(" · {} free on the drive", format::bytes(f as f64))).unwrap_or_default()
            );
            m.add(3, |ui| section(ui, cx, 3, icons::HARD_DRIVE, "Storage", Some(&storage_sub), |ui| {
                let used_gb = st.used_bytes as f64 / 1e9;
                let limit = draft.storage_limit_gb;
                let usage = if limit > 0.0 { (used_gb / limit).min(1.0) as f32 } else { 0.0 };
                ui.add_space(12.0);
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
                let p = ui.painter();
                widgets::text(p, row.left_center(), Align2::LEFT_CENTER, &Txt::new("Library usage", 12.0).color(theme::FG_3));
                let lim = if limit > 0.0 { format!("{limit:.0} GB") } else { "unlimited".into() };
                widgets::text(p, row.right_center(), Align2::RIGHT_CENTER, &Txt::new(format!("{used_gb:.1} / {lim}"), 12.0).color(theme::FG_3));
                ui.add_space(8.0);
                widgets::progress(ui, Id::unique("storage-usage"), usage, 10.0);
                ui.add_space(12.0);
                widgets::separator(ui);
                let can_go = if st.capture_bytes > 0 {
                    format!(
                        "The capture files of {} reconstructed capture{} take {}: only reconstructing again needs them, their 3D views stay.",
                        st.capture_sessions,
                        if st.capture_sessions == 1 { "" } else { "s" },
                        format::bytes(st.capture_bytes as f64)
                    )
                } else {
                    "What the captures and 3D pictures take, and deleting what they no longer need.".to_string()
                };
                widgets::field(ui, "Free up space", Some(&can_go), 160.0, |ui| {
                    if Button::new("Free up space").icon(icons::BROOM).size(Size::Sm).show(ui).clicked() {
                        free_space = true;
                    }
                });
                widgets::separator(ui);
                widgets::field(ui, "Storage limit", Some("Recording stops when the library reaches it (0 = no limit)."), 284.0, |ui| {
                    let mut v = draft.storage_limit_gb as f32;
                    if widgets::slider(ui, "limit", &mut v, 0.0..=2000.0, 10.0, |v| if v == 0.0 { "No limit".into() } else { format!("{v:.0} GB") }) {
                        draft.storage_limit_gb = v as f64;
                        apply = true;
                    }
                });
                widgets::separator(ui);
                if folder_field(ui, "Captures folder", None, &draft.sessions_dir) {
                    state.picker = Some((Folder::Sessions, pick(&ctx, &draft.sessions_dir)));
                }
                widgets::separator(ui);
                if folder_field(ui, "Raw captures folder", Some("Where Mixer's capture simulator writes."), &draft.raw_captures_dir) {
                    state.picker = Some((Folder::Raw, pick(&ctx, &draft.raw_captures_dir)));
                }
            }));
            if free_space {
                cx.go = Some(super::Go::FreeSpace);
            }

            let osc = &snap.osc;
            let osc_sub = if osc.listening {
                format!("Listening on port {}{}", osc.port.unwrap_or(0), if osc.advertising { ", advertised through OSCQuery" } else { "" })
            } else {
                osc.error.clone().unwrap_or_else(|| "Off".into())
            };
            m.add(4, |ui| section(ui, cx, 4, icons::BROADCAST, "VRChat and OSC", Some(&osc_sub), |ui| {
                widgets::field(
                    ui,
                    "Action Menu control",
                    Some("Start and stop recording with N4N0 › Record in your avatar's Action Menu, and let N4N0 and its capture kit hear Orbiters (OSCQuery, works next to VRCFT and other OSC apps)."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "osc", &mut draft.osc.enabled);
                    },
                );
                if !vrchat_here {
                    return;
                }
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "Stream camera",
                    Some("While recording, open VRChat's camera in Stream mode with Spout Stream on (the picture comes from it), then put it back."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "osc-camera", &mut draft.osc.drive_camera);
                    },
                );
                widgets::separator(ui);
                let every = [
                    SegOption::new(RigViews::Off, "Off"),
                    SegOption::new(RigViews::Every2s, "2 s"),
                    SegOption::new(RigViews::Every3s, "3 s"),
                    SegOption::new(RigViews::Every5s, "5 s"),
                ];
                widgets::field(
                    ui,
                    "Six more views while recording",
                    Some("This often, N4N0's camera rig takes six views around your camera (left, front, right, back, up, down) wherever it has not yet, so the reconstruction sees what you did not film. VRChat draws the world six more times for those moments; 3D pictures and scans take theirs all along."),
                    // As wide as its four small segments, so that the description keeps the row.
                    180.0,
                    |ui| {
                        apply |= widgets::segmented(ui, "rig-views", &mut draft.rig_views, &every, true);
                    },
                );
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "Camera dolly streams (experimental)",
                    Some("VRC+: a 3D picture flies four cameras with VRChat's camera dolly, one left, one right and one above yours, all looking where yours does, and recordings also read the dolly's other streams. In VRChat, set Camera › Dolly › Streaming to Multi once."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "dolly-streams", &mut draft.dolly_streams);
                    },
                );
                widgets::separator(ui);
                let log_desc = snap.vrchat.world.log_file.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| {
                    if snap.vrchat.log_found {
                        "Found, waiting for VRChat".into()
                    } else {
                        "Not found on this machine".into()
                    }
                });
                widgets::field(ui, "VRChat log", Some(&log_desc), 120.0, |ui| {
                    let (t, c) = if snap.vrchat.log_found { ("Read locally", theme::OK) } else { ("–", theme::FG_4) };
                    let (r, _) = ui.allocate_exact_size(vec2(100.0, 20.0), Sense::hover());
                    widgets::text(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(t, 12.0).color(c));
                });
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "Share my frame rate per world",
                    Some("In VR, a summary a minute: the world, your FPS, your avatar, how many people were there and the names of their avatars, never who they are, plus your graphics card, processor and headset. It shows which worlds run well on the Map explorer."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "world-performance", &mut draft.world_performance);
                    },
                );
            }));

            if vrchat_here {
            let vr = &snap.steamvr;
            let perf_sub = match (&vr.calibration, vr.running) {
                (Some(c), _) => format!("Calibrated: eyes at {:.2} m, hands {:.2} m apart", c.head_height_m, c.arm_span_m),
                _ if snap.vrchat.running => "No T-pose needed in VRChat: it sends its head and hand poses".into(),
                (None, true) => "SteamVR is running: calibrate in the Orbiters tab of its dashboard".into(),
                (None, false) => "What a session records besides the frames".into(),
            };
            m.add(5, |ui| section(ui, cx, 5, icons::PERSON_ARMS_SPREAD, "Performance", Some(&perf_sub), |ui| {
                widgets::field(
                    ui,
                    "Face and avatar parameters",
                    Some("Every parameter VRChat sends over OSC: face tracking from VRCFaceTracking, gestures, visemes, movement."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "perf-parameters", &mut draft.osc.record_parameters);
                    },
                );
                widgets::separator(ui);
                widgets::field(ui, "Your voice", Some("The microphone, for your avatar's visemes in Mixer."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "perf-mic", &mut draft.performance.record_mic);
                });
                if draft.performance.record_mic {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                        if source_chip(ui, crate::words::DEFAULT_MIC, draft.performance.mic_device.is_empty()).clicked() {
                            draft.performance.mic_device.clear();
                            apply = true;
                        }
                        for m in &snap.mics {
                            if source_chip(ui, &m.name, draft.performance.mic_device == m.id).clicked() {
                                draft.performance.mic_device = m.id.clone();
                                apply = true;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    mic_test(ui, &mut state.mic, &draft.performance.mic_device);
                    ui.add_space(12.0);
                }
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "VRChat sound",
                    Some("What VRChat plays, world sounds and other players' voices; nothing else on the PC. Ask the people around you first."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "perf-game", &mut draft.performance.record_game_audio);
                    },
                );
                widgets::separator(ui);
                widgets::field(ui, "Body tracking", Some("Headset, controllers and trackers from SteamVR, full-body estimation included."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "perf-tracking", &mut draft.performance.record_tracking);
                });
                widgets::separator(ui);
                widgets::field(ui, "Tracking rate", Some("Device poses recorded per second."), 284.0, |ui| {
                    apply |= widgets::slider(ui, "perf-rate", &mut draft.performance.tracking_rate_hz, 30.0..=120.0, 10.0, |v| format!("{v:.0} Hz"));
                });
                widgets::separator(ui);
                let tab = match (vr.running, vr.overlay) {
                    (true, true) => "Orbiters is in SteamVR's dashboard: record and calibrate without leaving VR.",
                    (true, false) => "SteamVR is running; the tab appears in a moment.",
                    (false, _) => "Orbiters as a tab in SteamVR's dashboard while SteamVR runs.",
                };
                widgets::field(ui, "SteamVR dashboard tab", Some(tab), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "perf-overlay", &mut draft.steamvr_overlay);
                });
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "Orbiters watch",
                    Some("The N4N0 mark on a wrist in VR, shown as you turn the wrist to look at it. Point at it with the other hand and pull the trigger once: N4N0's menu opens before you (or on your avatar's N4N0 screen), used with your controllers' lasers while VRChat keeps them; pull again to close it. Grip it to move it. The Orbiters panel floats beside you from the SteamVR tab's Float button."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "floating-watch", &mut draft.floating.watch);
                    },
                );
                if draft.floating.watch {
                    widgets::separator(ui);
                    use orbiters_core::settings::Hand;
                    let wrists = [SegOption::new(Hand::Left, "Left wrist"), SegOption::new(Hand::Right, "Right wrist")];
                    widgets::field(ui, "Watch on", None, 220.0, |ui| {
                        apply |= widgets::segmented(ui, "floating-watch-hand", &mut draft.floating.watch_hand, &wrists, true);
                    });
                }
                widgets::separator(ui);
                let hands = [
                    SegOption::new(CoverageOverlay::Off, "Off"),
                    SegOption::new(CoverageOverlay::LeftHand, "Left hand"),
                    SegOption::new(CoverageOverlay::RightHand, "Right hand"),
                ];
                widgets::field(
                    ui,
                    "Coverage while recording",
                    Some("In VR, the camera path and what it has covered float above a controller while you record, as on the Capture page. SteamVR shows it in your headset only: never in VRChat's camera or the recording."),
                    300.0,
                    |ui| {
                        apply |= widgets::segmented(ui, "coverage-overlay", &mut draft.coverage_overlay, &hands, true);
                    },
                );
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "Walk boost",
                    Some("In VR, your play space moves along with your walking stick, so you walk faster in VRChat. Also in the SteamVR tab and N4N0's menu."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "walk-boost", &mut draft.walk_boost.enabled);
                    },
                );
                if draft.walk_boost.enabled {
                    widgets::separator(ui);
                    let max = orbiters_core::settings::WalkBoostSettings::MAX_EXTRA * 100.0;
                    widgets::field(ui, "Strength", Some("How much faster than VRChat's own walking."), 284.0, |ui| {
                        apply |= widgets::slider(ui, "walk-strength", &mut draft.walk_boost.strength, 0.0..=1.0, 0.05, |v| format!("+{:.0} %", v * max));
                    });
                }
            }));
            }

            let start_sub = format!("Orbiters - N4N0 opening on its own, in the {}", crate::words::TRAY);
            m.add(6, |ui| section(ui, cx, 6, icons::POWER, "Start", Some(&start_sub), |ui| {
                let mut windows = state.windows_start.unwrap_or(false);
                let (start_label, start_note) = if vrchat_here {
                    ("Start with Windows", "Waits in the tray, ready when VRChat starts.")
                } else {
                    ("Start at login", "Waits in the menu bar, ready to talk with VRChat on your network.")
                };
                widgets::field(ui, start_label, Some(start_note), 48.0, |ui| {
                    if widgets::toggle(ui, "start-windows", &mut windows) && state.windows_job.is_none() {
                        state.windows_start = Some(windows);
                        state.windows_job = Some(Job::spawn(&ctx, move || crate::startup::set_start_with_windows(windows).map_err(|e| e.to_string())));
                    }
                });
                if !vrchat_here {
                    return;
                }
                widgets::separator(ui);
                let startup = &snap.settings.startup;
                let mut steamvr = state.steamvr_shown.unwrap_or(startup.steamvr);
                let note = match (startup.steamvr_pending && state.steamvr_job.is_none(), startup.steamvr) {
                    (true, true) => "SteamVR is not running: it is told at its next start, while Orbiters - N4N0 runs.",
                    (true, false) => "SteamVR is not running: it stops at its next start, while Orbiters - N4N0 runs.",
                    _ => "Opens when SteamVR starts, in its dashboard (SteamVR lists it with its startup overlay apps).",
                };
                widgets::field(ui, "Start with SteamVR", Some(note), 48.0, |ui| {
                    if widgets::toggle(ui, "start-steamvr", &mut steamvr) && state.steamvr_job.is_none() {
                        state.steamvr_shown = Some(steamvr);
                        state.steamvr_job = Some((steamvr, Job::spawn(&ctx, move || crate::startup::set_start_with_steamvr(steamvr))));
                    }
                });
            }));
            if vrchat_here {
                m.add(9, |ui| apply |= super::vr_audio::show(ui, cx, 9, &mut state.vr_audio, &mut draft.vr_audio));
            }

            m.add(7, |ui| section(ui, cx, 7, icons::SPARKLE, "Interface", None, |ui| {
                widgets::field(ui, "Reduce motion", Some("Calmer transitions and a still background."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "reduce-motion", &mut draft.reduce_motion);
                });
                widgets::separator(ui);
                if let Some(gpu) = cx.gpu.map(|g| &g.adapter).filter(|a| a.choice) {
                    let gpus = [
                        SegOption::new(WindowGpu::IntegratedGpu, "Integrated GPU").hint("The processor's GPU draws; the graphics card copies each frame to show it"),
                        SegOption::new(WindowGpu::GraphicsCard, "Graphics card").hint("Costs a game less than copying the frames from the integrated GPU"),
                    ];
                    let now = if (draft.window_gpu == WindowGpu::IntegratedGpu) == gpu.integrated {
                        format!("Now on the {}.", gpu.name)
                    } else {
                        format!("On the {} until the window opens again.", gpu.name)
                    };
                    let note = format!("The graphics card draws the window for less than it costs to copy it there from the integrated GPU. {now}");
                    widgets::field(ui, "Window graphics", Some(&note), 260.0, |ui| {
                        apply |= widgets::segmented(ui, "window-gpu", &mut draft.window_gpu, &gpus, true);
                    });
                    widgets::separator(ui);
                    if !gpu.integrated {
                        let views = [
                            SegOption::new(ViewGpu::WhilePlaying, "While VRChat runs"),
                            SegOption::new(ViewGpu::Always, "Always"),
                            SegOption::new(ViewGpu::Never, "Never"),
                        ];
                        let note = "The 3D view's splats draw on the integrated GPU and only their pictures reach the graphics card: orbiting costs a game little, at about half the frame rate.";
                        widgets::field(ui, "3D view on the integrated GPU", Some(note), 340.0, |ui| {
                            apply |= widgets::segmented(ui, "view-gpu", &mut draft.view_gpu, &views, true);
                        });
                        widgets::separator(ui);
                    }
                }
                widgets::field(ui, "Capture notice", Some("The consent reminder shown before the first capture."), 120.0, |ui| {
                    if Button::new("Show again").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                        draft.consent_acknowledged = false;
                        apply = true;
                    }
                });
            }));

            let mut want_log = false;
            let mut quit = false;
            m.add(8, |ui| section(ui, cx, 8, icons::INFO, "About", None, |ui| {
                ui.add_space(8.0);
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
                let lr = Rect::from_min_size(pos2(row.left(), row.center().y - 21.0), orbiters_ui::n4n0_logo::size_for_width(48.0));
                orbiters_ui::n4n0_logo::paint(ui.painter(), lr, theme::MARK);
                let x = lr.right() + 16.0;
                let p = ui.painter();
                widgets::text(
                    p,
                    pos2(x, row.center().y - 10.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(format!("Orbiters - N4N0 {}", snap.version), 15.0).weight(weight::SEMIBOLD),
                );
                widgets::text_truncated(
                    p,
                    pos2(x, row.center().y + 10.0),
                    Align2::LEFT_CENTER,
                    &Txt::new("Copyright © 2026 Enzo DUTRA / blackorbit. Free to use; see the licence for what is allowed.", 12.5).color(theme::FG_3),
                    row.width() - (x - row.left()) - 240.0,
                );
                if let Some(err) = &snap.worker.error {
                    widgets::text_truncated(
                        p,
                        pos2(x, row.center().y + 28.0),
                        Align2::LEFT_CENTER,
                        &Txt::new(err, 12.0).color(theme::DANGER),
                        row.width() - 300.0,
                    );
                }
                let mut bu = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(egui::Align::Center)));
                bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
                if cx.windowed && Button::new("Quit").icon(icons::SIGN_OUT).variant(Variant::Danger).size(Size::Sm).show(&mut bu).clicked() {
                    quit = true;
                }
                if Button::new("Worker log").icon(icons::SCROLL).size(Size::Sm).show(&mut bu).clicked() {
                    want_log = true;
                }
                widgets::separator(ui);
                updates(ui, cx.ctl, snap);
                widgets::separator(ui);
                let note = if draft.development {
                    "On: the Orbiters server running on this computer, its account and its builds of Orbiters - N4N0 as updates."
                } else {
                    "For testing development builds: the Orbiters server running on this computer instead of orbiters.cc, with its own account and updates."
                };
                widgets::field(ui, "Development environment", Some(note), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "development", &mut draft.development);
                });
                if state.log_open {
                    let lines = cx.ctl.worker_log().unwrap_or_default();
                    let k = anim::spring_from(ui.ctx(), Id::unique("worker-log-open"), [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
                    let h = 240.0 * k;
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
                    ui.painter().rect_filled(r, theme::radius(12.0), theme::INK_900);
                    let mut lu = ui.new_child(UiBuilder::new().max_rect(r.shrink(12.0)).id_salt("worker-log"));
                    lu.multiply_opacity(k);
                    egui::ScrollArea::vertical().id_salt("worker-log-scroll").stick_to_bottom(true).auto_shrink([false, false]).show(&mut lu, |ui| {
                        let text = if lines.is_empty() { "No output yet.".to_string() } else { lines.join("\n") };
                        let g = ui.painter().layout_job(Txt::new(text, 11.0).mono().color(theme::FG_2).line_height(17.0).job(r.width() - 24.0));
                        let (gr, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                        ui.painter().galley(gr.min, g, theme::FG_2);
                    });
                    ui.add_space(12.0);
                }
            }));
            m.finish(ui);
            if want_log {
                state.log_open = !state.log_open;
                if state.log_open {
                    cx.ctl.request_worker_log();
                }
            }
            if quit {
                cx.ctl.quit();
            }
        });
    });
    // Down to where the content starts (its top margin).
    super::under_top(ui, cx, "settings", from, ui.max_rect().top() + cx.top + 8.0);

    if apply {
        form::save(cx.ctl, &base, &draft);
    }
}

/// Updates of an installed app: where they stand, and the button that moves them on (controller/update.rs).
fn updates(ui: &mut Ui, ctl: &Ctl, snap: &Snapshot) {
    let u = &snap.update;
    if !u.enabled {
        widgets::field(ui, "Updates", Some("This copy was not installed by Orbiters - N4N0's setup: it does not update itself."), 0.0, |_| {});
        return;
    }
    let busy = snap.busy_with();
    let text = match &u.state {
        UpdateState::Idle => match u.checked {
            Some(at) => format!("Up to date (looked {}).", super::local_time_unix(at)),
            None => "Orbiters - N4N0 looks for a newer version shortly after it starts, then every few hours.".into(),
        },
        UpdateState::Checking => "Looking for a newer version…".into(),
        UpdateState::Downloading { version, fraction } => format!("Downloading version {version}: {:.0} %", fraction * 100.0),
        UpdateState::Ready { version, .. } if u.restart_when_idle => {
            format!("Version {version} is ready. Orbiters - N4N0 restarts to install it once {} ends.", busy.unwrap_or("the running work"))
        }
        UpdateState::Ready { version, .. } => match busy {
            Some(work) => format!("Version {version} is ready. Restarting now would stop {work}: it can wait for it to end."),
            None => format!("Version {version} is ready: a restart installs it, and Orbiters - N4N0 opens again."),
        },
        UpdateState::Failed { message } => message.clone(),
    };
    let hint = busy.map(|work| format!("Restarts once {work} ends")).unwrap_or_default();
    widgets::field(ui, "Updates", Some(&text), 300.0, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        match &u.state {
            UpdateState::Ready { .. } if u.restart_when_idle => {
                if Button::new("Restart now").variant(Variant::Danger).size(Size::Sm).show(ui).clicked() {
                    ctl.updates(UpdateAction::Restart { now: true });
                }
                if Button::new("Not after all").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                    ctl.updates(UpdateAction::CancelRestart);
                }
            }
            UpdateState::Ready { .. } => {
                let label = if busy.is_some() { "Restart when done" } else { "Restart to update" };
                let mut button = Button::new(label).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Primary).size(Size::Sm);
                if busy.is_some() {
                    button = button.hint(&hint);
                }
                if button.show(ui).clicked() {
                    ctl.updates(UpdateAction::Restart { now: false });
                }
            }
            UpdateState::Checking | UpdateState::Downloading { .. } => {
                Button::new("Look now").size(Size::Sm).enabled(false).show(ui);
            }
            UpdateState::Idle | UpdateState::Failed { .. } => {
                if Button::new("Look now").size(Size::Sm).show(ui).clicked() {
                    ctl.updates(UpdateAction::Check);
                }
            }
        }
    });
    if let UpdateState::Ready { notes, .. } = &u.state {
        let notes = notes.trim();
        if !notes.is_empty() {
            let w = ui.available_width();
            let mut job = Txt::new(notes, 12.0).color(theme::FG_3).line_height(18.0).job(w);
            job.wrap.max_rows = 8;
            let g = ui.painter().layout_job(job);
            let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y + 16.0), Sense::hover());
            ui.painter().galley(r.min, g, theme::FG_3);
        }
    }
}

fn pick(ctx: &egui::Context, current: &std::path::Path) -> Job<Option<PathBuf>> {
    let start = current.to_path_buf();
    Job::spawn(ctx, move || rfd::FileDialog::new().set_directory(&start).pick_folder())
}

/// The Orbiters account the Unity tools share: who is signed in, or the browser sign-in under way.
fn account(ui: &mut Ui, cx: &Cx, confirm_sign_out: &mut bool) {
    let status = &cx.snap.account;
    let server = if status.development { " · development server" } else { "" };
    let sub = match &status.state {
        AccountState::SignedIn { username } => format!("Signed in as {username}{server}"),
        AccountState::Checking { username } => format!("Checking {}…", username.as_deref().unwrap_or("your sign-in")),
        AccountState::Offline { username } => format!("Signed in as {} · Orbiters could not be reached", username.as_deref().unwrap_or("you")),
        AccountState::Expired { .. } => "Your sign-in expired".into(),
        AccountState::SigningIn => format!("Finish in your browser{server}"),
        AccountState::SignedOut | AccountState::Unknown => format!("Not signed in{server}"),
    };
    if !matches!(status.state, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. }) {
        *confirm_sign_out = false;
    }
    section(ui, cx, 1, icons::USER_CIRCLE, "Orbiters account", Some(&sub), |ui| {
        match &status.state {
            AccountState::SigningIn => {
                let desc = "The Orbiters page opened in your browser: sign in there if it asks, then click Connect. Nothing to type here.";
                widgets::field(ui, "Connect in your browser", Some(desc), 240.0, |ui| {
                    if Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.cancel_sign_in();
                    }
                    ui.add_space(8.0);
                    if Button::new("Open again").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.reopen_sign_in();
                    }
                });
            }
            AccountState::SignedOut | AccountState::Expired { .. } | AccountState::Unknown => {
                let desc = "One sign-in for Orbiters - N4N0 and your Unity tools: MCB, My Avatar, ReFit and the Logger. Uploads to the Map explorer use it.";
                widgets::field(ui, "Sign in", Some(desc), 140.0, |ui| {
                    if Button::new("Sign in").icon(icons::SIGN_IN).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.sign_in();
                    }
                });
            }
            _ => {
                let desc = "The account MCB, My Avatar and your other Orbiters tools use: signing in or out in one of them does it here too.";
                widgets::field(ui, "Shared with your Unity tools", Some(desc), 300.0, |ui| {
                    if *confirm_sign_out {
                        if Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            *confirm_sign_out = false;
                        }
                        ui.add_space(8.0);
                        if Button::new("Sign out of every tool").icon(icons::SIGN_OUT).variant(Variant::Danger).size(Size::Sm).show(ui).clicked() {
                            *confirm_sign_out = false;
                            cx.ctl.sign_out();
                        }
                    } else if Button::new("Sign out").icon(icons::SIGN_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        *confirm_sign_out = true;
                    }
                });
            }
        }
        if let Some(error) = &status.error {
            ui.add_space(4.0);
            let g = ui.painter().layout_job(Txt::new(error, 12.0).color(theme::DANGER).job(ui.available_width()));
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, theme::DANGER);
            ui.add_space(8.0);
        }
    });
}

/// Secrets are transient UI state, never part of Settings or the snapshot.
#[derive(Default)]
struct VrchatForm {
    username: String,
    password: String,
    code: String,
    method: Option<orbiters_core::vrchat_api::TwoFactor>,
    /// The password shown as typed (its eye).
    show_password: bool,
    pending: bool,
    pending_revision: u64,
    saw_busy: bool,
    picture: Option<(String, u64, egui::TextureHandle)>,
}

impl State {
    /// Synthetic offline data for the screenshot runner; never uses the person's account or credential store.
    pub(crate) fn stage_vrchat(&mut self, entering: bool) {
        if entering {
            self.vrchat.username = "DesktopTester".into();
            self.vrchat.password = "screenshot-only".into();
        }
    }
}

fn vrchat_pressed(response: egui::Response) -> bool {
    response.enabled() && (response.clicked() || (response.is_pointer_button_down_on() && response.ctx.input(|i| i.pointer.primary_pressed())))
}

/// The same flat well as widgets::text_field, with egui's password masking and an eye that shows what was typed.
fn vrchat_password(ui: &mut Ui, password: &mut String, width: f32, shown: &mut bool) -> egui::Response {
    let id = ui.scope_id().with("vrchat-password");
    let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, theme::radius(12.0), theme::INK_750);
    let ring = anim::toggle(ui.ctx(), id.with("ring"), ui.memory(|m| m.has_focus(id)), Spring::SNAPPY);
    if ring > 0.01 {
        ui.painter().rect_stroke(rect, theme::radius(12.0), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.8 * ring)), egui::StrokeKind::Outside);
    }
    let eye = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(28.0, 28.0));
    let label = if *shown { "Hide the password" } else { "Show the password" };
    if vrchat_pressed(
        widgets::IconButton::new(if *shown { icons::EYE_SLASH } else { icons::KEY }, label).size(28.0, 14.0).id_salt("vrchat-eye").show_at(ui, eye),
    ) {
        *shown = !*shown;
    }
    ui.put(
        Rect::from_min_max(pos2(rect.left() + 12.0, rect.top()), pos2(eye.left() - 4.0, rect.bottom())),
        egui::TextEdit::singleline(password)
            .id(id)
            .password(!*shown)
            .frame(egui::Frame::NONE)
            .font(theme::font(13.0, weight::REGULAR))
            .text_color(theme::FG)
            .hint_text(egui::RichText::new("VRChat password").color(theme::FG_4))
            .margin(egui::Margin::ZERO)
            .align(Align2::LEFT_CENTER),
    )
}

/// Six cells for a six-digit code (an authenticator's, an email's): digits only, each popping in as it is typed, the
/// next cell ringed in green while the field has the keys.
fn code_cells(ui: &mut Ui, code: &mut String) -> egui::Response {
    let id = ui.scope_id().with("vrchat-code-cells");
    let (cell, gap, h) = (44.0, 8.0, 50.0);
    let w = 6.0 * cell + 5.0 * gap;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    // The text edit takes the keys (and clicks) over the cells; its own text, caret and selection stay unseen.
    let resp = {
        let mut eu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("vrchat-code-edit"));
        let v = eu.visuals_mut();
        v.text_cursor.stroke = egui::Stroke::NONE;
        v.selection.bg_fill = egui::Color32::TRANSPARENT;
        v.selection.stroke = egui::Stroke::NONE;
        eu.put(
            rect,
            egui::TextEdit::singleline(code)
                .id(id)
                .frame(egui::Frame::NONE)
                .text_color(egui::Color32::TRANSPARENT)
                .font(theme::font(13.0, weight::REGULAR))
                .char_limit(6)
                .margin(egui::Margin::ZERO)
                .desired_width(w),
        )
    };
    code.retain(|c| c.is_ascii_digit());
    let ctx = ui.ctx().clone();
    let focused = ui.memory(|m| m.has_focus(id));
    let n = code.chars().count();
    let p = ui.painter();
    for i in 0..6 {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (cell + gap), rect.top()), vec2(cell, h));
        let next = focused && i == n.min(5);
        let ring = anim::toggle(&ctx, id.with(("ring", i)), next, Spring::SNAPPY);
        let filled = anim::toggle(&ctx, id.with(("filled", i)), i < n, Spring::SOFT);
        p.rect_filled(r, theme::radius(12.0), theme::mix(theme::INK_750, theme::INK_700, filled));
        if ring > 0.01 {
            p.rect_stroke(r, theme::radius(12.0), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.85 * ring)), egui::StrokeKind::Outside);
        }
        if let Some(c) = code.chars().nth(i) {
            let k = anim::spring_from(&ctx, id.with(("digit", i, c)), [0.55], [1.0], Spring::BOUNCY)[0];
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 22.0 * k.clamp(0.4, 1.3)).weight(weight::SEMIBOLD));
        } else if next && anim::is_live(&ctx) {
            // A caret that blinks where the next digit goes.
            let t = anim::time(&ctx);
            if (t * 1.6).fract() < 0.6 {
                p.rect_filled(Rect::from_center_size(r.center(), vec2(2.0, 20.0)), theme::radius(1.0), theme::FG_2);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::Text)
}

/// The signed-in account: picture with a presence ring, name, how it is kept; Sign out on the right.
fn vrchat_identity(ui: &mut Ui, name: &str, picture: Option<&egui::TextureHandle>, busy: bool) -> bool {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
    let p = ui.painter().clone();
    let a = Rect::from_center_size(pos2(row.left() + 28.0, row.center().y), vec2(52.0, 52.0));
    p.circle_stroke(a.center(), 29.0, egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.9)));
    widgets::avatar(&p, a, picture, name);
    let dot = a.center() + vec2(19.0, 19.0);
    p.circle_filled(dot, 7.0, theme::surface_strong());
    p.circle_filled(dot, 5.0, theme::ACCENT);
    let x = a.right() + 18.0;
    widgets::text_truncated(&p, pos2(x, row.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(name, 16.0).weight(weight::SEMIBOLD), w - 220.0);
    widgets::icon(&p, pos2(x + 6.0, row.center().y + 12.0), icons::LOCK_SIMPLE, 12.0, theme::FG_3);
    widgets::text_truncated(
        &p,
        pos2(x + 18.0, row.center().y + 12.0),
        Align2::LEFT_CENTER,
        &Txt::new("Connected · the session stays encrypted on this computer", 12.0).color(theme::FG_3),
        w - 240.0,
    );
    let b = Button::new(if busy { "Signing out…" } else { "Sign out" }).icon(icons::SIGN_OUT).variant(Variant::Secondary).size(Size::Sm).enabled(!busy);
    let bw = b.desired_width(ui);
    vrchat_pressed(b.show_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.center().y - 16.0), vec2(bw, 32.0))))
}

fn vrchat_account(ui: &mut Ui, cx: &Cx, state: &mut VrchatForm) {
    use crate::controller::vrchat_login::{Command, Recovery};
    use orbiters_core::vrchat_api::{Secret, TwoFactor};
    let status = &cx.snap.vrchat_login;
    if !status.allowed || !matches!(cx.snap.account.state, AccountState::SignedIn { .. }) {
        *state = VrchatForm::default();
        return;
    }
    if status.busy {
        state.saw_busy = true;
    }
    if (state.saw_busy && !status.busy) || (state.pending && state.pending_revision != status.revision) {
        state.pending = false;
        state.saw_busy = false;
    }
    let busy = status.busy || state.pending;
    let subtitle = if let Some(user) = &status.user {
        format!("Signed in as {}", user.display_name)
    } else if !status.methods.is_empty() {
        "Two-factor authentication: one more step".into()
    } else {
        "Your own account, kept on this computer".into()
    };
    let mut command = None;
    section(ui, cx, 10, icons::VIRTUAL_REALITY, "VRChat account", Some(&subtitle), |ui| {
        ui.add_space(6.0);
        if let Some(user) = &status.user {
            if let Some(picture) = &status.picture {
                if state.picture.as_ref().is_none_or(|(id, revision, _)| id != &user.id || *revision != status.revision) {
                    let image = egui::ColorImage::from_rgba_unmultiplied([picture.width as usize, picture.height as usize], &picture.rgba);
                    let texture = ui.ctx().load_texture(format!("vrchat-account-{}", user.id), image, egui::TextureOptions::LINEAR);
                    state.picture = Some((user.id.clone(), status.revision, texture));
                }
            } else {
                state.picture = None;
            }
            if vrchat_identity(ui, &user.display_name, state.picture.as_ref().map(|(_, _, t)| t), busy) {
                command = Some(Command::SignOut);
            }
        } else {
            state.picture = None;
            ui.add_enabled_ui(!busy, |ui| {
                if status.methods.is_empty() {
                    widgets::field(ui, "Username", Some("Your VRChat login name or email, not your display name."), 300.0, |ui| {
                        widgets::text_field(ui, "vrchat-username", &mut state.username, "VRChat username or email", 300.0, false);
                    });
                    widgets::separator(ui);
                    let mut enter = false;
                    widgets::field(ui, "Password", Some("Sent to VRChat only, never to Orbiters."), 300.0, |ui| {
                        let response = vrchat_password(ui, &mut state.password, 300.0, &mut state.show_password);
                        enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    });
                    widgets::separator(ui);
                    ui.add_space(14.0);
                    let ready = !state.username.trim().is_empty() && !state.password.is_empty();
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    let note = "Instead of Orbiters' shared account: friends, instances and pictures come from yours.";
                    widgets::icon(ui.painter(), pos2(row.left() + 7.0, row.center().y), icons::SHIELD_CHECK, 14.0, theme::FG_3);
                    widgets::text_truncated(
                        ui.painter(),
                        pos2(row.left() + 22.0, row.center().y),
                        Align2::LEFT_CENTER,
                        &Txt::new(note, 12.0).color(theme::FG_3),
                        row.width() - 230.0,
                    );
                    let b = Button::new(if busy { "Signing in…" } else { "Sign in to VRChat" }).icon(icons::SIGN_IN).variant(Variant::Primary).enabled(ready);
                    let bw = b.desired_width(ui);
                    if (vrchat_pressed(b.show_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, 40.0)))) || enter) && ready {
                        command = Some(Command::SignIn { username: state.username.clone(), password: Secret(std::mem::take(&mut state.password)) });
                    }
                } else {
                    let method = state.method.get_or_insert(status.methods[0]);
                    if !status.methods.contains(method) {
                        *method = status.methods[0];
                    }
                    if status.methods.len() > 1 {
                        let before = *method;
                        let options: Vec<_> = status
                            .methods
                            .iter()
                            .map(|m| {
                                SegOption::new(
                                    *m,
                                    match m {
                                        TwoFactor::Totp => "Authenticator app",
                                        TwoFactor::Email => "Email",
                                        TwoFactor::Recovery => "Recovery code",
                                    },
                                )
                            })
                            .collect();
                        widgets::segmented(ui, "vrchat-factor-method", method, &options, false);
                        if *method != before {
                            state.code.clear();
                        }
                        ui.add_space(6.0);
                    }
                    let hint = match method {
                        TwoFactor::Totp => "The six digits your authenticator app shows for VRChat now.",
                        TwoFactor::Email => "VRChat emailed you six digits: check your inbox and spam folder.",
                        TwoFactor::Recovery => "One of the recovery codes you saved when you turned two-factor authentication on.",
                    };
                    let mut enter = false;
                    let six = *method != TwoFactor::Recovery;
                    widgets::field(ui, method.label(), Some(hint), if six { 304.0 } else { 300.0 }, |ui| {
                        let response = if six {
                            code_cells(ui, &mut state.code)
                        } else {
                            widgets::text_field(ui, "vrchat-factor-code", &mut state.code, "xxxx-xxxx", 300.0, true)
                        };
                        enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    });
                    widgets::separator(ui);
                    ui.add_space(14.0);
                    let ready = if six { state.code.chars().count() == 6 } else { !state.code.trim().is_empty() };
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    let verify =
                        Button::new(if busy { "Verifying…" } else { "Verify and sign in" }).icon(icons::SHIELD_CHECK).variant(Variant::Primary).enabled(ready);
                    let cancel = Button::new("Cancel").variant(Variant::Ghost);
                    let (vw, cw) = (verify.desired_width(ui), cancel.desired_width(ui));
                    if (vrchat_pressed(verify.show_at(ui, Rect::from_min_size(pos2(row.right() - vw, row.top()), vec2(vw, 40.0)))) || enter) && ready {
                        command = Some(Command::Verify { method: *method, code: Secret(std::mem::take(&mut state.code)) });
                    }
                    if vrchat_pressed(cancel.show_at(ui, Rect::from_min_size(pos2(row.right() - vw - 8.0 - cw, row.top()), vec2(cw, 40.0)))) {
                        command = Some(Command::Cancel);
                    }
                }
            });
        }
        if busy {
            ui.add_space(12.0);
            super::sharing::working(ui, "Waiting for VRChat…");
        }
        if let Some(error) = &status.error {
            ui.add_space(12.0);
            orbiters_ui::feedback::banner(ui, Id::unique(("vrchat-login-error", error)), widgets::Tone::Danger, error, false);
            let retry = match status.recovery {
                Recovery::Resume => Some(("Retry the saved session", icons::ARROW_CLOCKWISE, Command::Resume)),
                Recovery::RemoveSavedSession => Some(("Remove the saved session", icons::TRASH, Command::SignOut)),
                Recovery::None => None,
            };
            if let Some((label, glyph, retry)) = retry {
                ui.add_space(10.0);
                let b = Button::new(label).icon(glyph).variant(Variant::Secondary).size(Size::Sm).enabled(!busy);
                let bw = b.desired_width(ui);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                if vrchat_pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                    command = Some(retry);
                }
            }
        }
        ui.add_space(4.0);
    });
    if let Some(command) = command {
        state.pending = true;
        state.pending_revision = status.revision;
        state.saw_busy = false;
        state.code.clear();
        cx.ctl.vrchat_login(command);
        orbiters_ui::anim::request_frame(ui.ctx());
    }
}
/// Folder row: label, path, Change button. True when Change was clicked.
fn folder_field(ui: &mut Ui, label: &str, description: Option<&str>, value: &std::path::Path) -> bool {
    let shown = value.display().to_string();
    let mut clicked = false;
    let desc = description.unwrap_or(&shown);
    widgets::field(ui, label, Some(desc), if description.is_some() { 380.0 } else { 110.0 }, |ui| {
        clicked = Button::new("Change").icon(icons::FOLDER).size(Size::Sm).show(ui).clicked();
        if description.is_some() {
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(260.0, 20.0), Sense::hover());
            widgets::text_truncated(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(&shown, 12.0).color(theme::FG_3), 260.0);
        }
    });
    clicked
}

pub(super) fn source_chip(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
    let t = Txt::new(label, 12.0).color(if active { theme::FG } else { theme::FG_2 });
    let s = widgets::text_size(ui, &t);
    let (r, resp) = ui.allocate_exact_size(vec2(s.x + 24.0, 30.0), Sense::click());
    let hover = anim::toggle(ui.ctx(), resp.id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let press = anim::toggle(ui.ctx(), resp.id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let rr = Rect::from_center_size(r.center(), r.size() * (1.0 - 0.05 * press));
    let bg = if active { theme::INK_600 } else { theme::mix(theme::control(), theme::INK_600, 0.5 * hover) };
    ui.painter().rect_filled(rr, theme::radius(15.0), bg);
    widgets::text(ui.painter(), rr.center(), Align2::CENTER_CENTER, &t);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
