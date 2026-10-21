//! Settings page: the Orbiters account, capture, storage, VRChat and OSC, performance, interface, about. Changes apply as they are made (shown at
//! once, saved in the background); text fields apply with their Save button.

use super::mic::{mic_test, MicTest};
use super::{section, Cx, Job};
use crate::state::AccountState;
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder};
use orbiters_core::settings::{Settings, WhilePlaying};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Variant};
use orbiters_ui::{format, icons, logo};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    Sessions,
    Raw,
}

#[derive(Default)]
pub struct State {
    /// Local copy the controls edit; refreshed whenever the saved settings change elsewhere.
    draft: Option<Settings>,
    synced: Option<Settings>,
    log_open: bool,
    picker: Option<(Folder, Job<Option<PathBuf>>)>,
    mic: MicTest,
    /// "Sign out" was pressed once: it asks before signing every tool out.
    confirm_sign_out: bool,
}

impl State {
    /// Ends the mic test (leaving the page).
    pub fn stop_mic_test(&mut self) {
        self.mic = MicTest::default();
    }
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    if state.synced.as_ref() != Some(&cx.snap.settings) {
        state.draft = Some(cx.snap.settings.clone());
        state.synced = Some(cx.snap.settings.clone());
    }
    let mut draft = state.draft.clone().unwrap_or_else(|| cx.snap.settings.clone());
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

    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            super::block(ui, cx, 0, 72.0, |ui, rect| super::heading(ui, rect, "Preferences", "Settings", None));
            let snap = cx.snap;
            account(ui, cx, &mut state.confirm_sign_out);

            section(ui, cx, 2, icons::GAUGE, "Capture", Some("How frames become a session"), |ui| {
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
                    if Button::new("Save").size(Size::Sm).show(ui).clicked() {
                        apply = true;
                    }
                    ui.add_space(8.0);
                    widgets::text_field(ui, "spout-sender", &mut draft.spout_sender, &placeholder, 208.0, false);
                });
                if !snap.sources.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                        for s in &snap.sources {
                            if source_chip(ui, &format!("{} · {}×{}", s.name, s.width, s.height), draft.spout_sender == s.name).clicked() {
                                draft.spout_sender = s.name.clone();
                                apply = true;
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                widgets::separator(ui);
                let places = [
                    SegOption::new(WhilePlaying::GraphicsCard, "Graphics card").hint("Fastest; VRChat stutters while it trains"),
                    SegOption::new(WhilePlaying::IntegratedGpu, "Integrated GPU")
                        .hint("About 40 times slower, and VRChat keeps its frame rate; once VRChat closes it starts again on the graphics card"),
                    SegOption::new(WhilePlaying::AfterVrchat, "After VRChat").hint("Reconstructions wait until VRChat closes"),
                ];
                widgets::field(
                    ui,
                    "Reconstruct while VRChat runs",
                    Some("Training on the graphics card makes VRChat stutter; the processor's integrated GPU leaves the game alone but is about 40 times slower."),
                    360.0,
                    |ui| {
                        apply |= widgets::segmented(ui, "while-playing", &mut draft.reconstruct_while_playing, &places, true);
                    },
                );
            });

            let st = &snap.storage;
            let storage_sub = format!(
                "{} in {} sessions{}",
                format::bytes(st.used_bytes as f64),
                st.sessions,
                st.free_bytes.map(|f| format!(" · {} free on the drive", format::bytes(f as f64))).unwrap_or_default()
            );
            section(ui, cx, 3, icons::HARD_DRIVE, "Storage", Some(&storage_sub), |ui| {
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
                widgets::field(ui, "Storage limit", Some("Recording stops when the library reaches it (0 = no limit)."), 284.0, |ui| {
                    let mut v = draft.storage_limit_gb as f32;
                    if widgets::slider(ui, "limit", &mut v, 0.0..=2000.0, 10.0, |v| if v == 0.0 { "No limit".into() } else { format!("{v:.0} GB") }) {
                        draft.storage_limit_gb = v as f64;
                        apply = true;
                    }
                });
                widgets::separator(ui);
                if folder_field(ui, "Sessions folder", None, &draft.sessions_dir) {
                    state.picker = Some((Folder::Sessions, pick(&ctx, &draft.sessions_dir)));
                }
                widgets::separator(ui);
                if folder_field(ui, "Raw captures folder", Some("Where Mixer's capture simulator writes."), &draft.raw_captures_dir) {
                    state.picker = Some((Folder::Raw, pick(&ctx, &draft.raw_captures_dir)));
                }
            });

            let osc = &snap.osc;
            let osc_sub = if osc.listening {
                format!("Listening on port {}{}", osc.port.unwrap_or(0), if osc.advertising { ", advertised through OSCQuery" } else { "" })
            } else {
                osc.error.clone().unwrap_or_else(|| "Off".into())
            };
            section(ui, cx, 4, icons::BROADCAST, "VRChat and OSC", Some(&osc_sub), |ui| {
                widgets::field(
                    ui,
                    "Action Menu control",
                    Some("Start and stop recording from your avatar's Action Menu (OSCQuery, works next to VRCFT and other OSC apps)."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "osc", &mut draft.osc.enabled);
                    },
                );
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
                widgets::field(ui, "Record parameter", Some("Bool avatar parameter that toggles recording."), 270.0, |ui| {
                    if Button::new("Save").size(Size::Sm).show(ui).clicked() {
                        apply = true;
                    }
                    ui.add_space(8.0);
                    widgets::text_field(ui, "record-parameter", &mut draft.osc.record_parameter, "OrbitersRecord", 192.0, true);
                });
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
            });

            let vr = &snap.steamvr;
            let perf_sub = match (&vr.calibration, vr.running) {
                (Some(c), _) => format!("Calibrated: eyes at {:.2} m, hands {:.2} m apart", c.head_height_m, c.arm_span_m),
                _ if snap.vrchat.running => "No T-pose needed in VRChat: it sends its head and hand poses".into(),
                (None, true) => "SteamVR is running: calibrate in the Orbiters tab of its dashboard".into(),
                (None, false) => "What a session records besides the frames".into(),
            };
            section(ui, cx, 5, icons::PERSON_ARMS_SPREAD, "Performance", Some(&perf_sub), |ui| {
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
                        if source_chip(ui, "Windows default", draft.performance.mic_device.is_empty()).clicked() {
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
                    "Walk boost",
                    Some("In VR, your play space moves along with your walking stick, so you walk faster in VRChat. Also in the SteamVR tab and Orb's menu."),
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
            });

            section(ui, cx, 6, icons::SPARKLE, "Interface", None, |ui| {
                widgets::field(ui, "Reduce motion", Some("Calmer transitions and a still background."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "reduce-motion", &mut draft.reduce_motion);
                });
                widgets::separator(ui);
                widgets::field(ui, "Capture notice", Some("The consent reminder shown before the first capture."), 120.0, |ui| {
                    if Button::new("Show again").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                        draft.consent_acknowledged = false;
                        apply = true;
                    }
                });
            });

            let mut want_log = false;
            let mut quit = false;
            section(ui, cx, 7, icons::INFO, "About", None, |ui| {
                ui.add_space(8.0);
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
                let lr = Rect::from_min_size(pos2(row.left(), row.center().y - 21.0), logo::size_for_width(48.0));
                logo::paint(ui.painter(), lr, logo::Look::default());
                let x = lr.right() + 16.0;
                let p = ui.painter();
                widgets::text(
                    p,
                    pos2(x, row.center().y - 10.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(format!("Orbiters desktop {}", snap.version), 15.0).weight(weight::SEMIBOLD),
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
            });
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
    super::under_top(ui, cx, from, ui.max_rect().top() + cx.top + 8.0);

    if apply {
        cx.ctl.save_settings(draft.clone());
    }
    state.draft = Some(draft);
}

fn pick(ctx: &egui::Context, current: &std::path::Path) -> Job<Option<PathBuf>> {
    let start = current.to_path_buf();
    Job::spawn(ctx, move || rfd::FileDialog::new().set_directory(&start).pick_folder())
}

/// The Orbiters account the Unity tools share: who is signed in, or the browser sign-in and its code.
fn account(ui: &mut Ui, cx: &Cx, confirm_sign_out: &mut bool) {
    let status = &cx.snap.account;
    let server = if status.development { " · development server" } else { "" };
    let sub = match &status.state {
        AccountState::SignedIn { username } => format!("Signed in as {username}{server}"),
        AccountState::Checking { username } => format!("Checking {}…", username.as_deref().unwrap_or("your sign-in")),
        AccountState::Offline { username } => format!("Signed in as {} · Orbiters could not be reached", username.as_deref().unwrap_or("you")),
        AccountState::Expired { .. } => "Your sign-in expired".into(),
        AccountState::SigningIn { .. } => "Finish in your browser".into(),
        AccountState::SignedOut | AccountState::Unknown => format!("Not signed in{server}"),
    };
    if !matches!(status.state, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. }) {
        *confirm_sign_out = false;
    }
    section(ui, cx, 1, icons::USER_CIRCLE, "Orbiters account", Some(&sub), |ui| {
        match &status.state {
            AccountState::SigningIn { code, provider } => {
                let via = if provider == "telegram" { "Telegram" } else { "Discord" };
                let desc = format!("Sign in with {via} on the Orbiters page that opened in your browser, then type this code there.");
                widgets::field(ui, "Code", Some(&desc), 360.0, |ui| {
                    if Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.cancel_sign_in();
                    }
                    ui.add_space(8.0);
                    if Button::new("Open the page again").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.reopen_sign_in();
                    }
                    ui.add_space(12.0);
                    let t = Txt::new(code, 22.0).weight(weight::SEMIBOLD).tracking(0.3).mono();
                    let s = widgets::text_size(ui, &t);
                    let (chip, _) = ui.allocate_exact_size(vec2(s.x + 28.0, 40.0), Sense::hover());
                    ui.painter().rect_filled(chip, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.14));
                    widgets::text(ui.painter(), chip.center(), Align2::CENTER_CENTER, &t.color(theme::ACCENT));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                });
            }
            AccountState::SignedOut | AccountState::Expired { .. } | AccountState::Unknown => {
                let desc = "One sign-in for Orbiters desktop and your Unity tools: MCB, My Avatar, ReFit and the Logger. Uploads to the Map explorer use it.";
                widgets::field(ui, "Sign in", Some(desc), 260.0, |ui| {
                    if Button::new("Telegram").icon(icons::TELEGRAM_LOGO).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.sign_in("telegram");
                    }
                    ui.add_space(8.0);
                    if Button::new("Discord").icon(icons::DISCORD_LOGO).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.sign_in("discord");
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
