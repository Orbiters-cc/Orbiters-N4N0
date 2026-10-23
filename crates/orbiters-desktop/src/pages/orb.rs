//! Orb page: talking to Orb (typed, or held to speak), what Orb knows of the avatar (the Orb prop and its panel, the
//! AFK message, carried and held parameters), saved videos, Orb's ears (speech models, microphone, wake word), its
//! assistant (on, or off for a tool without AI), what Orbiters costs this computer, and testing what everyone sees of
//! the orb (its states, sample texts, away, hidden, touched). Changes apply as they are made; the AFK message with
//! its Save button (see [`super::form`]).

use super::form::{self, Texts};
use super::mic::{mic_test, MicTest};
use super::settings::source_chip;
use super::{section, Cx};
use crate::ipc::OrbCommand;
use crate::state::{AccountState, CallLink, CallPhase, CallView, OrbFace, OrbLine, OrbStatus, OrbWho};
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder};
use orbiters_core::settings::{Settings, GOGOLOCO_PARAMETERS};
use orbiters_ui::anim;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

#[derive(Default)]
pub struct State {
    /// The AFK message as typed, until saved.
    texts: Texts,
    /// The question being typed.
    ask: String,
    mic: MicTest,
    /// The Talk button is held.
    talking: bool,
}

impl State {
    /// Ends the microphone test and a held Talk (leaving the page).
    pub fn leave(&mut self, cx: &crate::backend::Ctl) {
        self.mic = MicTest::default();
        if self.talking {
            self.talking = false;
            cx.orb(OrbCommand::Talk { down: false });
        }
    }
}

/// How the assistant stands, for the conversation's subtitle.
fn assistant_line(orb: &OrbStatus, settings: &Settings, account: &AccountState) -> String {
    if !settings.orb.enabled {
        return "Orb is off".into();
    }
    if !settings.orb.assistant {
        return "Assistant off: Orb is a tool without AI".into();
    }
    if !matches!(account, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. }) {
        return "Sign in to Orbiters (Settings) to talk to Orb".into();
    }
    match &orb.quota {
        Some(q) if q.own_key => "Answers on your own key".into(),
        Some(q) if q.limit > 0 => format!("{} of {} free answers left today", q.limit.saturating_sub(q.used), q.limit),
        _ => "Ask anything, or hold Talk and speak".into(),
    }
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    // The controls edit a copy of the settings as published; what they change is saved over the settings as they
    // are by then (`form::save`), so a pin Orb just added stays.
    let base = cx.snap.settings.clone();
    let mut draft = base.clone();
    let mut apply = false;
    let snap = cx.snap;
    let orb = &snap.orb;

    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt("orb").auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            let sub = heading_line(snap);
            super::block(ui, cx, 0, 72.0, |ui, rect| super::heading(ui, rect, "Companion", "Orb", Some(&sub)));

            let talk_sub = assistant_line(orb, &snap.settings, &snap.account.state);
            section(ui, cx, 1, icons::SMILEY, "Talk to Orb", Some(&talk_sub), |ui| {
                conversation(ui, orb);
                ui.add_space(12.0);
                let signed_in = matches!(snap.account.state, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. });
                let can_ask = snap.settings.orb.enabled && snap.settings.orb.assistant && signed_in;
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                let talk_w = 132.0;
                let ask_w = 92.0;
                let field = Rect::from_min_max(row.min, pos2(row.right() - talk_w - ask_w - 16.0, row.max.y));
                let mut fu = ui.new_child(UiBuilder::new().max_rect(field).layout(Layout::left_to_right(egui::Align::Center)));
                let resp = widgets::text_field(&mut fu, "orb-ask", &mut state.ask, "Ask Orb something…", field.width(), false);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ask_rect = Rect::from_min_size(pos2(field.right() + 8.0, row.top()), vec2(ask_w, 40.0));
                let ask = Button::new("Ask").icon(icons::SPARKLE).variant(Variant::Primary).enabled(can_ask && !orb.thinking && !state.ask.trim().is_empty()).width(ask_w).show_at(ui, ask_rect);
                if (ask.clicked() || enter) && can_ask && !state.ask.trim().is_empty() && !orb.thinking {
                    cx.ctl.orb(OrbCommand::Ask { text: std::mem::take(&mut state.ask) });
                }
                // Talk: held to speak, like the menu's button. One widget whatever its label says: the press that
                // changes the label keeps holding it.
                let talk_rect = Rect::from_min_size(pos2(ask_rect.right() + 8.0, row.top()), vec2(talk_w, 40.0));
                let label = if state.talking { "Listening…" } else { "Hold to talk" };
                let talk = Button::new(label)
                    .icon(icons::MICROPHONE)
                    .enabled(can_ask && orb.voice.installed)
                    .width(talk_w)
                    .id_salt("orb-talk")
                    .show_at(ui, talk_rect);
                let held = talk.is_pointer_button_down_on();
                if held != state.talking && (can_ask && orb.voice.installed || state.talking) {
                    state.talking = held;
                    cx.ctl.orb(OrbCommand::Talk { down: held });
                }
                if !orb.voice.installed && can_ask {
                    ui.add_space(6.0);
                    note(ui, "Install Orb's ears below to talk instead of typing.", theme::FG_3);
                }
                for c in &orb.confirmations {
                    ui.add_space(12.0);
                    // What Orb asks before it does it is answered here (or by saying yes); the rest on Orbiters.
                    let (fallback, label, icon) = if c.is_local() {
                        ("Orb asks before it does this. Say or type no to leave it.", "Yes, do it", icons::CHECK)
                    } else {
                        ("Waiting for your approval on Orbiters.", "Approve on Orbiters", icons::ARROW_SQUARE_OUT)
                    };
                    let desc = if c.summary.is_empty() { fallback } else { c.summary.as_str() };
                    widgets::field(ui, if c.title.is_empty() { "Needs your approval" } else { &c.title }, Some(desc), 190.0, |ui| {
                        if Button::new(label).icon(icon).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::OpenLink { url: c.approve_url.clone() });
                        }
                    });
                }
                if !orb.lines.is_empty() {
                    ui.add_space(8.0);
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                    let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::right_to_left(egui::Align::Center)));
                    if Button::new("New conversation").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Ghost).size(Size::Sm).show(&mut bu).clicked() {
                        cx.ctl.orb(OrbCommand::NewConversation);
                    }
                }
            });

            let avatar_sub = avatar_line(snap);
            section(ui, cx, 2, icons::PERSON, "On your avatar", Some(&avatar_sub), |ui| {
                panel_preview(ui, orb);
                ui.add_space(10.0);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
                bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
                if Button::new("Show a test message").icon(icons::PLAY).size(Size::Sm).enabled(orb.prop_sync.is_some()).show(&mut bu).clicked() {
                    cx.ctl.orb(OrbCommand::Say { text: "Hi, I'm Orb. Yes, the one in the menu. No, I'm not your friend.".into() });
                }
                if Button::new("Clear").icon(icons::X).variant(Variant::Ghost).size(Size::Sm).enabled(orb.panel.is_some()).show(&mut bu).clicked() {
                    cx.ctl.orb(OrbCommand::HidePanel);
                }
                let camera = snap.vrchat.running && !snap.is_capturing();
                if Button::new("Picture in 3 s").icon(icons::CAMERA).variant(Variant::Ghost).size(Size::Sm).enabled(camera).show(&mut bu).clicked() {
                    cx.ctl.orb(OrbCommand::Photo { delay_seconds: 3 });
                }
                ui.add_space(12.0);
                widgets::separator(ui);
                let menu = if orb.avatar_id.is_none() {
                    "Orb looks at your avatar once VRChat loads it.".to_string()
                } else if orb.manifest {
                    format!("Orbiters Toolkit described this avatar's menu: Orb knows its {} controls by name.", orb.controls)
                } else {
                    "Orb sees only parameter names. Upload the avatar from Unity with Orbiters Toolkit installed to give Orb its menu.".to_string()
                };
                widgets::field(ui, "Menu", Some(&menu), 140.0, |ui| {
                    let (t, c) = if orb.manifest { ("Known by label", theme::OK) } else if orb.avatar_id.is_some() { ("Names only", theme::WARN) } else { ("–", theme::FG_4) };
                    let (r, _) = ui.allocate_exact_size(vec2(130.0, 20.0), Sense::hover());
                    widgets::text(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(t, 12.0).color(c));
                });
                if orb.screen && orb.prop_sync == Some(8) {
                    ui.add_space(6.0);
                    note(ui, "Orb's screen fills twice as fast with 16 sync values: choose 16 in Orbiters Toolkit's Orb settings and upload the avatar again.", theme::FG_3);
                }
                widgets::separator(ui);
                widgets::field(ui, "AFK message", Some("While VRChat says you are away, the panel tells people around you."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "orb-afk", &mut draft.orb.afk_message_enabled);
                });
                if draft.orb.afk_message_enabled {
                    widgets::separator(ui);
                    widgets::field(ui, "Message", Some("What the panel says while you are away."), 440.0, |ui| {
                        if let Some(text) = form::text_row(ui, &mut state.texts, "orb-afk-text", &base.orb.afk_message, "Getting food, back in 10!", 360.0, false) {
                            draft.orb.afk_message = text;
                            apply = true;
                        }
                    });
                }
                widgets::separator(ui);
                let mut carry = !draft.orb.carry_parameters.is_empty();
                widgets::field(ui, "Keep my pose", Some("GoGoLoco's pose, height and float carry over to the next avatar you wear."), 48.0, |ui| {
                    if widgets::toggle(ui, "orb-carry", &mut carry) {
                        draft.orb.carry_parameters = if carry { GOGOLOCO_PARAMETERS.iter().map(|s| s.to_string()).collect() } else { Vec::new() };
                        apply = true;
                    }
                });
                if !draft.orb.pins.is_empty() {
                    widgets::separator(ui);
                    let mut remove = None;
                    for (i, pin) in draft.orb.pins.iter().enumerate() {
                        let value = match &pin.value {
                            orbiters_core::osc::OscValue::Bool(b) => if *b { "on".to_string() } else { "off".to_string() },
                            orbiters_core::osc::OscValue::Int(v) => v.to_string(),
                            orbiters_core::osc::OscValue::Float(v) => format!("{v:.2}"),
                            orbiters_core::osc::OscValue::Text(t) => t.clone(),
                        };
                        let desc = format!("Held at {value}: Orb puts it back whenever VRChat or a world resets it.");
                        widgets::field(ui, &pin.parameter, Some(&desc), 100.0, |ui| {
                            if Button::new("Let go").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        draft.orb.pins.remove(i);
                        apply = true;
                    }
                }
            });

            // Orb's menu: fast walk, YouTube, calls, a 3D picture. (Sections enter in the order they stand on the page.)
            let menu_sub = if orb.screen {
                "Opens on the screen your orb projects (\"Replace Orbiters overlay\" in My Avatar)"
            } else {
                "Opens in the SteamVR overlay, in front of you"
            };
            section(ui, cx, 3, icons::STACK, "Orb's menu", Some(menu_sub), |ui| {
                widgets::field(
                    ui,
                    "Controller button",
                    Some("SteamVR's \"Open Orb's menu\" binding for Orbiters: a double press of the left stick, unless you change it in SteamVR's controller settings."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "orb-menu-button", &mut draft.orb.menu.button);
                    },
                );
                let say = if draft.orb.wake_word {
                    "The wake word on its own opens the menu."
                } else {
                    "The wake word on its own opens the menu, once Orb listens for its name: turn on Wake word under Orb's ears."
                };
                widgets::field(ui, "Say \"Orb\"", Some(say), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "orb-menu-wake", &mut draft.orb.menu.wake_word);
                });
                widgets::field(ui, "Touch the orb", Some("A hand on the orb opens the menu; touching it again closes it."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "orb-menu-touch", &mut draft.orb.menu.touch);
                });
                widgets::field(ui, "Try it", Some("Opens the menu (or closes it) as the button and the orb do."), 140.0, |ui| {
                    if Button::new("Open the menu").icon(icons::STACK).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.orb(OrbCommand::ToggleScreen);
                    }
                });
                if snap.vrchat.running && snap.osc.enabled && !snap.osc.receiving {
                    // VRChat sends avatar parameters only to OSC services it found: without them, the orb's touch and
                    // the menu's Talk button never reach Orbiters.
                    ui.add_space(4.0);
                    note(
                        ui,
                        "VRChat has sent Orbiters nothing for a minute: touching the orb and Talk in its menu cannot reach Orb. Orbiters asks VRChat to find it again by itself; Reconnect asks now. Check that OSC is on in VRChat's Action Menu (Options › OSC).",
                        theme::WARN,
                    );
                    if Button::new("Reconnect").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.osc_reconnect();
                    }
                }
                widgets::separator(ui);
                widgets::field(ui, "Keyboard", Some("The letters of the search keyboard. Automatic follows this computer's keyboard."), 284.0, |ui| {
                    use orbiters_core::settings::KeyboardLayout as K;
                    let options = [widgets::SegOption::new(K::Auto, "Automatic"), widgets::SegOption::new(K::Qwerty, "QWERTY"), widgets::SegOption::new(K::Azerty, "AZERTY")];
                    apply |= widgets::segmented(ui, "orb-menu-keyboard", &mut draft.orb.menu.keyboard, &options, true);
                });
                widgets::field(
                    ui,
                    "Look",
                    Some("Retro: a green phosphor terminal, as on an old computer. The orb's screen has its own Look, in My Avatar."),
                    200.0,
                    |ui| {
                        use orbiters_core::settings::MenuLook as L;
                        let options = [widgets::SegOption::new(L::Modern, "Modern"), widgets::SegOption::new(L::Retro, "Retro")];
                        apply |= widgets::segmented(ui, "orb-menu-look", &mut draft.orb.menu.look, &options, true);
                    },
                );
            });

            let calls = &orb.calls;
            let calls_sub = match (&calls.call, &calls.link) {
                (Some(c), _) => call_line(c),
                (None, CallLink::Linked { .. }) if calls.contacts.is_empty() => "No friend with Orbiters and Orb on right now".to_string(),
                (None, CallLink::Linked { .. }) => format!("{} friends can be called", calls.contacts.len()),
                (None, CallLink::Off) => "Call VRChat friends who have Orbiters and Orb too".to_string(),
                (None, CallLink::SignedOut) => "Sign in to Orbiters to use calls".to_string(),
                (None, CallLink::NoAccount) => "Open VRChat, VRCX or VRCNext once so Orbiters knows your VRChat account".to_string(),
                (None, CallLink::NotLinked { .. }) => "Link your VRChat account on Orbiters to be called".to_string(),
            };
            section(ui, cx, 4, icons::BROADCAST, "Calls", Some(&calls_sub), |ui| {
                widgets::field(
                    ui,
                    "Calls",
                    Some("Your presence and your VRChat friends (as hashes, from VRCX and VRCNext) go to Orbiters, to find friends you can call. Calls go through Orbiters' relay: a friend never sees this computer's address."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "orb-calls", &mut draft.orb.calls);
                    },
                );
                if draft.orb.calls {
                    widgets::separator(ui);
                    widgets::field(
                        ui,
                        "Direct calls",
                        Some("Connect straight to the friend when your networks allow it: a little faster, but each of you sees the other's IP address."),
                        48.0,
                        |ui| {
                            apply |= widgets::toggle(ui, "orb-direct-calls", &mut draft.orb.direct_calls);
                        },
                    );
                }
                if let CallLink::NotLinked { account, source } = &calls.link {
                    widgets::separator(ui);
                    let desc = format!("{source} says this computer uses {account}. Confirm it's yours on Orbiters (a friend request, once) so friends can call you.");
                    widgets::field(ui, "Link your VRChat account", Some(&desc), 160.0, |ui| {
                        if Button::new("Link on Orbiters").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                            let site = if snap.account.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                            let _ = crate::controller::open_url(&format!("{}account?tab=overview", site.website()));
                        }
                    });
                }
                if let Some(call) = &calls.call {
                    widgets::separator(ui);
                    let title = call.name.clone();
                    let line = call_line(call);
                    widgets::field(ui, &title, Some(&line), 260.0, |ui| {
                        let ringing = call.incoming && call.phase == CallPhase::Ringing;
                        let end = if ringing { "Decline" } else { "Hang up" };
                        if Button::new(end).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::EndCall);
                        }
                        ui.add_space(8.0);
                        if ringing {
                            if Button::new("Answer").icon(icons::MICROPHONE).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                                cx.ctl.orb(OrbCommand::AnswerCall);
                            }
                        } else {
                            let label = if call.muted { "Unmute" } else { "Mute" };
                            if Button::new(label).icon(icons::MICROPHONE).size(Size::Sm).show(ui).clicked() {
                                cx.ctl.orb(OrbCommand::MuteCall { on: !call.muted });
                            }
                        }
                    });
                }
                if matches!(calls.link, CallLink::Linked { .. }) {
                    for contact in &calls.contacts {
                        widgets::separator(ui);
                        widgets::field(ui, &contact.name, Some("Has Orbiters and Orb on now"), 100.0, |ui| {
                            if Button::new("Call").icon(icons::BROADCAST).size(Size::Sm).enabled(calls.call.is_none()).show(ui).clicked() {
                                cx.ctl.orb(OrbCommand::Call { contact: contact.id.clone() });
                            }
                        });
                    }
                }
            });

            let videos_sub = if orb.videos.is_empty() { "Ask Orb to “save this video” while a world plays one".to_string() } else { format!("{} saved", orb.videos.len()) };
            section(ui, cx, 5, icons::FILM_STRIP, "Saved videos", Some(&videos_sub), |ui| {
                if let Some(video) = &snap.vrchat.world.last_video {
                    widgets::field(ui, "Playing in this world", Some(&video.url), 150.0, |ui| {
                        if Button::new("Save it").icon(icons::DOWNLOAD_SIMPLE).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::SaveVideo);
                        }
                    });
                    if !orb.videos.is_empty() {
                        widgets::separator(ui);
                    }
                }
                for (i, v) in orb.videos.iter().take(12).enumerate() {
                    if i > 0 {
                        widgets::separator(ui);
                    }
                    let title = v.title.clone().unwrap_or_else(|| v.url.clone());
                    let when = super::local_time(&v.saved_utc);
                    let desc = match &v.world { Some(w) => format!("{w} · {when}"), None => when };
                    widgets::field(ui, &title, Some(&desc), 236.0, |ui| {
                        if Button::new("").icon(icons::TRASH).variant(Variant::Ghost).size(Size::Sm).hint("Remove").show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::RemoveVideo { url: v.url.clone() });
                        }
                        ui.add_space(4.0);
                        if Button::new("Open").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            let _ = crate::controller::open_url(&v.url);
                        }
                        ui.add_space(4.0);
                        if Button::new("Copy link").icon(icons::COPY_SIMPLE).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::CopyText { text: v.url.clone() });
                        }
                    });
                }
                if orb.videos.is_empty() && snap.vrchat.world.last_video.is_none() {
                    note(ui, "Saved videos stay on this computer. Copy a link and paste it in a world's video player.", theme::FG_3);
                    ui.add_space(8.0);
                }
            });

            let voice = &orb.voice;
            // The last tenth of the progress is the unpacking.
            let unpacking = voice.downloading.is_some_and(|f| f >= 0.9);
            let ears_sub = if unpacking {
                "Unpacking the speech models…".to_string()
            } else if let Some(f) = voice.downloading {
                format!("Downloading the speech models… {:.0} %", f * 100.0)
            } else if voice.repair {
                "The speech models need a repair".to_string()
            } else if !voice.installed {
                "Speech models not installed".to_string()
            } else if voice.running {
                "Ready: they start with VRChat".to_string()
            } else {
                "Installed: they start with VRChat".to_string()
            };
            section(ui, cx, 6, icons::MICROPHONE, "Orb's ears", Some(&ears_sub), |ui| {
                if !voice.installed {
                    let desc = "Speech to text on this computer, in 25 languages (NVIDIA Parakeet, Silero VAD): about 490 MB to download. Nothing you say leaves it until it is a sentence for Orb.";
                    widgets::field(ui, "Speech models", Some(desc), 280.0, |ui| {
                        let busy = voice.downloading.is_some();
                        let label = match (busy, unpacking, voice.repair) {
                            (true, true, _) => "Unpacking…",
                            (true, false, _) => "Downloading…",
                            (false, _, true) => "Repair (490 MB)",
                            (false, _, false) => "Install (490 MB)",
                        };
                        if Button::new(label).icon(icons::DOWNLOAD_SIMPLE).variant(Variant::Primary).size(Size::Sm).enabled(!busy).show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::InstallVoice);
                        }
                        if busy {
                            ui.add_space(8.0);
                            if Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                                cx.ctl.orb(OrbCommand::CancelVoiceInstall);
                            }
                        }
                    });
                    if let Some(f) = voice.downloading {
                        ui.add_space(4.0);
                        widgets::progress(ui, Id::unique("orb-voice-download"), f, 8.0);
                        ui.add_space(10.0);
                    }
                    widgets::separator(ui);
                }
                if let Some(error) = voice.error.as_deref().filter(|_| voice.downloading.is_none()) {
                    note(ui, error, theme::DANGER);
                    ui.add_space(8.0);
                    widgets::separator(ui);
                }
                widgets::field(ui, "Microphone", Some("The one Orb listens to."), 0.0, |_| {});
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    if source_chip(ui, "Windows default", draft.orb.mic_device.is_empty()).clicked() {
                        draft.orb.mic_device.clear();
                        apply = true;
                    }
                    for m in &snap.mics {
                        if source_chip(ui, &m.name, draft.orb.mic_device == m.id).clicked() {
                            draft.orb.mic_device = m.id.clone();
                            apply = true;
                        }
                    }
                });
                ui.add_space(8.0);
                mic_test(ui, &mut state.mic, &draft.orb.mic_device);
                ui.add_space(12.0);
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "Wake word",
                    Some("Start a sentence with “Orb” and Orb hears it, any time. Every sentence is transcribed on this computer to find it; none is kept."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "orb-wake", &mut draft.orb.wake_word);
                    },
                );
                widgets::separator(ui);
                widgets::field(ui, "Mute VRChat while I hold Talk", Some("The sentence for Orb stays out of the world."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "orb-mute", &mut draft.orb.mute_while_talking);
                });
                ui.add_space(4.0);
                note(ui, "In VRChat: hold Talk in Orb's menu, touch the orb with a hand, or start with “Orb” when the wake word is on.", theme::FG_3);
                ui.add_space(8.0);
            });

            section(ui, cx, 7, icons::SPARKLE, "Assistant", Some("Orb's brain runs on Orbiters"), |ui| {
                widgets::field(ui, "Orb", Some("The panel, the AFK message, carried and held parameters, saved videos."), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "orb-enabled", &mut draft.orb.enabled);
                });
                widgets::separator(ui);
                widgets::field(
                    ui,
                    "AI assistant",
                    Some("Off, Orb is a tool without AI: nothing is transcribed, and nothing goes to Orbiters."),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "orb-ai", &mut draft.orb.assistant);
                    },
                );
                widgets::separator(ui);
                widgets::field(ui, "Personality and own key", Some("Orb's roast level, its language and your own AI key are on the Orbiters website."), 150.0, |ui| {
                    if Button::new("Open").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        let site = if snap.account.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                        let _ = crate::controller::open_url(&format!("{}orb?tab=settings&section=orb", site.website()));
                    }
                });
            });

            section(ui, cx, 8, icons::CPU, "Footprint", Some("What Orbiters and VRChat use on this computer"), |ui| {
                if orb.resources.is_empty() {
                    note(ui, "Measured every few seconds while Orbiters runs.", theme::FG_3);
                    ui.add_space(8.0);
                }
                for (i, p) in orb.resources.iter().enumerate() {
                    if i > 0 {
                        widgets::separator(ui);
                    }
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    let painter = ui.painter();
                    widgets::text(painter, row.left_center(), Align2::LEFT_CENTER, &Txt::new(&p.name, 13.5).weight(weight::MEDIUM));
                    let usage = format!("{:.0} % CPU · {:.0} MB", p.cpu_percent, p.memory_mb);
                    widgets::text(painter, row.right_center(), Align2::RIGHT_CENTER, &Txt::new(usage, 12.5).color(theme::FG_2).mono());
                }
            });

            let prop = orb.prop_sync.is_some();
            let testing_sub = match orb.prop_sync {
                Some(n) => format!("What everyone sees of your orb · {n} text slots"),
                None => "Wear an avatar with the Orb prop in VRChat to test it".to_string(),
            };
            section(ui, cx, 9, icons::FLASK, "Testing", Some(&testing_sub), |ui| {
                widgets::field(
                    ui,
                    "States",
                    Some("Hold the orb in one state, as everyone sees it, or play them all, two seconds each. Automatic follows what Orb is doing."),
                    150.0,
                    |ui| {
                        if Button::new("Play every state").icon(icons::PLAY).size(Size::Sm).enabled(prop && !orb.test_cycle).show(ui).clicked() {
                            cx.ctl.orb(OrbCommand::TestCycle);
                        }
                    },
                );
                ui.add_space(8.0);
                let mut face = orb.test_face;
                let mut options = vec![widgets::SegOption::new(None, "Automatic")];
                options.extend(OrbFace::ALL.iter().map(|f| widgets::SegOption::new(Some(*f), state_label(*f))));
                ui.add_enabled_ui(prop, |ui| {
                    if widgets::segmented(ui, "orb-test-face", &mut face, &options, true) {
                        cx.ctl.orb(OrbCommand::TestFace { face });
                    }
                });
                ui.add_space(12.0);
                widgets::separator(ui);
                widgets::field(ui, "Panel text", Some("Sample texts on the panel: a short line, a full panel (its edges marked), several pages, every glyph."), 0.0, |_| {});
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
                bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
                for (label, text) in [("Short", TEST_SHORT.to_string()), ("Full panel", test_full_panel()), ("Several pages", TEST_PAGES.to_string()), ("Every glyph", test_every_glyph())] {
                    if Button::new(label).variant(Variant::Secondary).size(Size::Sm).enabled(prop).show(&mut bu).clicked() {
                        cx.ctl.orb(OrbCommand::Say { text });
                    }
                }
                if Button::new("Clear").icon(icons::X).variant(Variant::Ghost).size(Size::Sm).enabled(orb.panel.is_some()).show(&mut bu).clicked() {
                    cx.ctl.orb(OrbCommand::HidePanel);
                }
                ui.add_space(12.0);
                widgets::separator(ui);
                let mut step = draft.orb.text_step_ms as f32;
                widgets::field(ui, "Panel speed", Some("Time between two blocks of text. Faster than 250 ms, people around you miss blocks."), 284.0, |ui| {
                    if widgets::slider(ui, "orb-test-step", &mut step, 250.0..=1000.0, 50.0, |v| format!("{v:.0} ms")) {
                        draft.orb.text_step_ms = step as u64;
                        apply = true;
                    }
                });
                widgets::separator(ui);
                let mut away = orb.test_afk;
                widgets::field(ui, "Act as away", Some("The sleeping orb and your AFK message, as when VRChat says you are away."), 48.0, |ui| {
                    if widgets::toggle(ui, "orb-test-afk", &mut away) {
                        cx.ctl.orb(OrbCommand::TestAfk { on: away });
                    }
                });
                widgets::separator(ui);
                let mut shown = !orb.test_hidden;
                widgets::field(ui, "Show the orb", Some("Off hides the whole orb and its panel (Orb/Visible), like Show Orb in its menu."), 48.0, |ui| {
                    if widgets::toggle(ui, "orb-test-visible", &mut shown) {
                        cx.ctl.orb(OrbCommand::TestVisible { on: shown });
                    }
                });
                widgets::separator(ui);
                widgets::field(ui, "Touch the orb", Some("As if your hand touched it: the menu opens (or Orb listens for one sentence, with that off)."), 110.0, |ui| {
                    let ears = orb.voice.installed && snap.settings.orb.assistant;
                    if Button::new("Touch").icon(icons::CROSSHAIR).variant(Variant::Secondary).size(Size::Sm).enabled(prop && ears).show(ui).clicked() {
                        cx.ctl.orb(OrbCommand::TestTouch);
                    }
                });
            });
        });
    });
    super::under_top(ui, cx, "orb", from, ui.max_rect().top() + cx.top + 8.0);

    if apply {
        form::save(cx.ctl, &base, &draft);
    }
}

fn heading_line(snap: &crate::state::Snapshot) -> String {
    let orb = &snap.orb;
    if !snap.settings.orb.enabled {
        return "Orb is off".into();
    }
    if !snap.vrchat.running {
        return "Waiting for VRChat".into();
    }
    match (&snap.vrchat.world.avatar_name, orb.prop_sync) {
        (Some(a), Some(_)) => format!("On “{a}”"),
        (Some(a), None) => format!("“{a}” has no Orb: add it from My Avatar's gallery"),
        (None, Some(_)) => "On your avatar".into(),
        (None, None) => "Your avatar has no Orb: add it from My Avatar's gallery".into(),
    }
}

fn avatar_line(snap: &crate::state::Snapshot) -> String {
    let orb = &snap.orb;
    match (orb.prop_sync, orb.face) {
        (Some(_), face) if snap.vrchat.running => format!("Orb is {}", face_word(face)),
        (Some(n), _) => format!("Orb prop with {n} text values"),
        (None, _) if orb.avatar_id.is_some() => "No Orb prop on this avatar".into(),
        _ => "No avatar yet".into(),
    }
}

/// Where a call stands, in a few words.
fn call_line(call: &CallView) -> String {
    match call.phase {
        CallPhase::Calling => format!("Calling {}…", call.name),
        CallPhase::Ringing => format!("{} is calling", call.name),
        CallPhase::Connecting => "Connecting…".to_string(),
        CallPhase::Connected => {
            let since = call.connected_at_ms.map(|t| now_ms().saturating_sub(t) / 1000).unwrap_or(0);
            format!("On a call · {}:{:02}{}", since / 60, since % 60, if call.muted { " · muted" } else { "" })
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A state's name in the Testing section.
fn state_label(face: OrbFace) -> &'static str {
    match face {
        OrbFace::Idle => "Idle",
        OrbFace::Listening => "Listening",
        OrbFace::Thinking => "Thinking",
        OrbFace::Talking => "Talking",
        OrbFace::Sleeping => "Sleeping",
        OrbFace::Happy => "Happy",
        OrbFace::Annoyed => "Annoyed",
        OrbFace::Surprised => "Surprised",
    }
}

const TEST_SHORT: &str = "Testing, testing. Is this thing on?";
const TEST_PAGES: &str = "This is page one of a test that runs over several pages, to check that each page stays up long enough to read. \
Here comes page two: accents à é è ù ç ô, a little Cyrillic, Привет, and symbols ♥ ★ ♪ → ✓. \
And this is the third and last page. When it has been read, the panel goes away on its own.";

/// Four rows of exactly 32 characters, their first and last cells marked: the panel's edges.
fn test_full_panel() -> String {
    (1..=4).map(|r| format!("[{r}{}]", "·".repeat(29))).collect::<Vec<_>>().join("\n")
}

/// Every glyph of the panel but the space, in table order (two pages).
fn test_every_glyph() -> String {
    orbiters_contracts::orb::glyphs().iter().enumerate().filter(|(i, c)| *i > 0 && **c != '\0').map(|(_, c)| *c).collect()
}

fn face_word(face: crate::state::OrbFace) -> &'static str {
    use crate::state::OrbFace::*;
    match face {
        Idle => "floating around",
        Listening => "listening",
        Thinking => "thinking",
        Talking => "talking",
        Sleeping => "asleep",
        Happy => "happy",
        Annoyed => "annoyed",
        Surprised => "surprised",
    }
}

fn note(ui: &mut Ui, text: &str, color: egui::Color32) {
    let g = ui.painter().layout_job(Txt::new(text, 12.0).color(color).job(ui.available_width()));
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// The last lines of the conversation: the person's to the right, Orb's in green to the left, what Orb did in small.
fn conversation(ui: &mut Ui, orb: &OrbStatus) {
    let width = ui.available_width();
    let lines: Vec<&OrbLine> = orb.lines.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    if lines.is_empty() && !orb.thinking && !orb.listening {
        let (r, _) = ui.allocate_exact_size(vec2(width, 64.0), Sense::hover());
        ui.painter().rect_filled(r, theme::radius(14.0), theme::white(0.03));
        widgets::text(
            ui.painter(),
            r.center(),
            Align2::CENTER_CENTER,
            &Txt::new("“Orb, put my hoodie on.” “Orb, plan a movie night Saturday.” “Orb, save this video.”", 12.5).color(theme::FG_3),
        );
        return;
    }
    for line in lines {
        let max = (width * 0.72).min(560.0);
        match line.who {
            OrbWho::You | OrbWho::Orb => {
                let mine = line.who == OrbWho::You;
                let t = Txt::new(&line.text, 13.5).color(theme::FG).line_height(19.0);
                let g = ui.painter().layout_job(t.job(max - 28.0));
                let size = g.size() + vec2(28.0, 18.0);
                let (row, _) = ui.allocate_exact_size(vec2(width, size.y + 8.0), Sense::hover());
                let bubble = if mine {
                    Rect::from_min_size(pos2(row.right() - size.x, row.top()), size)
                } else {
                    Rect::from_min_size(pos2(row.left() + 34.0, row.top()), size)
                };
                let fill = if mine { theme::INK_700 } else { theme::alpha(theme::ACCENT, 0.13) };
                ui.painter().rect_filled(bubble, theme::radius(14.0), fill);
                ui.painter().galley(bubble.min + vec2(14.0, 9.0), g, theme::FG);
                if !mine {
                    orb_dot(ui, pos2(row.left() + 13.0, bubble.top() + 15.0), 11.0);
                }
            }
            OrbWho::Note | OrbWho::Error => {
                let color = if line.who == OrbWho::Error { theme::DANGER } else { theme::FG_3 };
                let g = ui.painter().layout_job(Txt::new(&line.text, 11.5).color(color).job(width - 34.0));
                let (row, _) = ui.allocate_exact_size(vec2(width, g.size().y + 8.0), Sense::hover());
                ui.painter().galley(pos2(row.left() + 34.0, row.top() + 2.0), g, color);
            }
        }
    }
    if orb.thinking || orb.listening {
        let (row, _) = ui.allocate_exact_size(vec2(width, 38.0), Sense::hover());
        let bubble = Rect::from_min_size(pos2(row.left() + 34.0, row.top()), vec2(if orb.listening { 150.0 } else { 64.0 }, 30.0));
        ui.painter().rect_filled(bubble, theme::radius(14.0), theme::alpha(theme::ACCENT, 0.13));
        orb_dot(ui, pos2(row.left() + 13.0, bubble.center().y), 11.0);
        let t = anim::time(ui.ctx());
        if orb.listening {
            // The microphone's level, as Orb hears it.
            let level = orb.voice.level.clamp(0.0, 1.0);
            let bar = Rect::from_min_size(pos2(bubble.left() + 14.0, bubble.center().y - 3.0), vec2(bubble.width() - 28.0, 6.0));
            ui.painter().rect_filled(bar, theme::radius(3.0), theme::white(0.08));
            ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * level).max(6.0), bar.height())), theme::radius(3.0), theme::ACCENT);
        } else {
            for i in 0..3 {
                let phase = ((t * 2.2 - i as f64 * 0.22).sin() * 0.5 + 0.5) as f32;
                let c = pos2(bubble.left() + 20.0 + i as f32 * 12.0, bubble.center().y - 2.0 * phase);
                ui.painter().circle_filled(c, 3.2, theme::alpha(theme::ACCENT, 0.45 + 0.55 * phase));
            }
        }
        // Dots and a level meter: 30 frames a second, not the monitor's rate.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// Orb's little face: a green disc with two eyes.
fn orb_dot(ui: &Ui, center: egui::Pos2, radius: f32) {
    let p = ui.painter();
    p.circle_filled(center, radius, theme::ACCENT);
    for dx in [-3.6f32, 3.6] {
        p.rect_filled(Rect::from_center_size(center + vec2(dx, -0.5), vec2(2.4, 5.0)), theme::radius(1.2), theme::ON_ACCENT);
    }
}

/// The panel as people see it: four rows of 32 characters, white on a dark glass, empty when Orb is quiet.
fn panel_preview(ui: &mut Ui, orb: &OrbStatus) {
    let width = ui.available_width().min(520.0);
    let (r, _) = ui.allocate_exact_size(vec2(width, 112.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, theme::radius(14.0), theme::INK_950);
    match (&orb.panel, orb.prop_sync) {
        (Some(text), _) => {
            for (i, row) in text.lines().take(4).enumerate() {
                widgets::text(p, pos2(r.left() + 18.0, r.top() + 22.0 + i as f32 * 22.0), Align2::LEFT_CENTER, &Txt::new(row, 14.0).mono().color(theme::FG));
            }
        }
        (None, Some(_)) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new("The panel is hidden until Orb speaks", 12.5).color(theme::FG_4));
        }
        (None, None) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new("No Orb on this avatar: add it from My Avatar's gallery", 12.5).color(theme::FG_4));
        }
    }
    widgets::text(p, pos2(r.right() - 14.0, r.bottom() - 12.0), Align2::RIGHT_CENTER, &Txt::caps("Panel").color(theme::FG_4));
}
