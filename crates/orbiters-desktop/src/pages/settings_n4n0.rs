//! N4N0's settings, moved from the N4N0 page into Settings (REQUEST21 2): the N4N0 card, its menu, calls, the assistant and
//! N4N0's ears. The same switches over the same stored values as on the N4N0 page before; what N4N0 is doing, its
//! conversation, calls under way, saved videos and its tests stay there.

use super::form::{self, Texts};
use super::mic::{mic_test, MicTest};
use super::settings::source_chip;
use super::settings_registry::{ROW_HELD, ROW_WHERE};
use super::{section, Cx};
use crate::ipc::N4n0Command;
use crate::state::CallLink;
use egui::{vec2, Id, Ui};
use orbiters_core::settings::{Settings, GOGOLOCO_PARAMETERS};
use orbiters_i18n::{mark, tr, tr_ctx};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Variant};

/// The N4N0 card: N4N0 on, the bot, the panel, the AFK message, what carries over and what is held.
pub fn n4n0_card(ui: &mut Ui, cx: &Cx, index: usize, base: &Settings, draft: &mut Settings, texts: &mut Texts) -> bool {
    let snap = cx.snap;
    let n4n0 = &snap.n4n0;
    let mut apply = false;
    let sub = super::n4n0::heading_line(snap);
    section(ui, cx, index, icons::N4N0, "N4N0", Some(&sub), |ui| {
        widgets::field(ui, "N4N0", Some(tr("The panel, the AFK message, carried and held parameters, saved videos.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-enabled", &mut draft.n4n0.enabled);
        });
        widgets::separator(ui);
        // The bot and the menu are two choices (screen version 17); an older screen hides with the bot.
        let bot_line = if n4n0.bot_keeps_menu || n4n0.prop_sync.is_none() {
            tr("The bot beside your head, its beams and its panel, for everyone around. Off, N4N0's menu still opens, before your eyes as ever. Show N4N0 in your Action Menu does the same.")
        } else {
            tr("The bot beside your head, its beams and its panel. This avatar's N4N0 hides its menu screen with it: while the bot is off, the menu opens in Orbiters' overlay. Update N4N0 in My Avatar and upload the avatar again to keep the menu on the avatar without the bot.")
        };
        let mut bot = n4n0.bot.unwrap_or(true);
        widgets::field(ui, tr("Show N4N0"), Some(bot_line), 48.0, |ui| {
            ui.add_enabled_ui(n4n0.prop_sync.is_some(), |ui| {
                if widgets::toggle(ui, "n4n0-bot", &mut bot) {
                    cx.ctl.n4n0(N4n0Command::ShowBot { on: bot, avatar: n4n0.avatar_id.clone() });
                }
            });
        });
        widgets::separator(ui);
        let mut step = draft.n4n0.text_step_ms as f32;
        widgets::field(ui, tr("Panel speed"), Some(tr("Time between two blocks of text. Faster than 250 ms, people around you miss blocks.")), 284.0, |ui| {
            if widgets::slider(ui, "n4n0-test-step", &mut step, 250.0..=1000.0, 50.0, |v| format!("{v:.0} ms")) {
                draft.n4n0.text_step_ms = step as u64;
                apply = true;
            }
        });
        widgets::separator(ui);
        widgets::field(ui, tr("AFK message"), Some(tr("While VRChat says you are away, the panel tells people around you.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-afk", &mut draft.n4n0.afk_message_enabled);
        });
        if draft.n4n0.afk_message_enabled {
            widgets::separator(ui);
            let (room, field) = form::text_row_width(ui, ui.available_width(), 360.0);
            widgets::field(ui, tr("Message"), Some(tr("What the panel says while you are away.")), room, |ui| {
                if let Some(text) = form::text_row(ui, texts, "n4n0-afk-text", &base.n4n0.afk_message, tr("Getting food, back in 10!"), field, false) {
                    draft.n4n0.afk_message = text;
                    apply = true;
                }
            });
        }
        widgets::separator(ui);
        let mut carry = !draft.n4n0.carry_parameters.is_empty();
        widgets::field(ui, tr("Keep my pose"), Some(tr("GoGoLoco's pose, height and float carry over to the next avatar you wear.")), 48.0, |ui| {
            if widgets::toggle(ui, "n4n0-carry", &mut carry) {
                draft.n4n0.carry_parameters = if carry { GOGOLOCO_PARAMETERS.iter().map(|s| s.to_string()).collect() } else { Vec::new() };
                apply = true;
            }
        });
        widgets::separator(ui);
        let input = super::n4n0::input_line(n4n0);
        widgets::field(ui, tr("Controller input"), Some(&input), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-controller-input", &mut draft.n4n0.controller_input);
        });
        widgets::separator(ui);
        let held = if draft.n4n0.pins.is_empty() { tr(HELD_NONE) } else { tr("Parameters N4N0 puts back whenever VRChat or a world resets them.") };
        widgets::field(ui, tr(ROW_HELD), Some(held), 0.0, |_| {});
        if !draft.n4n0.pins.is_empty() {
            let mut remove = None;
            // Each row is named by its parameter: not a setting of its own.
            widgets::unrecorded(|| {
                for (i, pin) in draft.n4n0.pins.iter().enumerate() {
                    widgets::separator(ui);
                    let value = match &pin.value {
                        orbiters_core::osc::OscValue::Bool(b) => {
                            if *b {
                                tr_ctx("value", "on").to_string()
                            } else {
                                tr_ctx("value", "off").to_string()
                            }
                        }
                        orbiters_core::osc::OscValue::Int(v) => v.to_string(),
                        orbiters_core::osc::OscValue::Float(v) => format!("{v:.2}"),
                        orbiters_core::osc::OscValue::Text(t) => t.clone(),
                    };
                    let desc = tr!("Held at {value}: N4N0 puts it back whenever VRChat or a world resets it.", value = value);
                    widgets::field(ui, &pin.parameter, Some(&desc), 100.0, |ui| {
                        if Button::new(tr("Let go")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            remove = Some(i);
                        }
                    });
                }
            });
            if let Some(i) = remove {
                draft.n4n0.pins.remove(i);
                apply = true;
            }
        }
    });
    apply
}

/// Under Held parameters while none is.
const HELD_NONE: &str = mark("None yet: ask N4N0 to hold a parameter, and it stays at its value here until you let it go.");

/// N4N0's menu: where it opens, how it is opened, its keyboard, look and size.
pub fn menu_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let snap = cx.snap;
    let n4n0 = &snap.n4n0;
    let mut apply = false;
    let menu_sub = if n4n0.screen {
        tr("Opens on the screen your N4N0 projects (\"Replace Orbiters overlay\" in My Avatar)")
    } else {
        tr("Opens in the SteamVR overlay, in front of you")
    };
    section(ui, cx, index, icons::STACK, tr("N4N0's menu"), Some(menu_sub), |ui| {
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            use orbiters_core::settings::MenuDisplay;
            // Two different things, each whole without the other (moved from Settings' Performance card).
            let ways = [SegOption::new(MenuDisplay::AvatarScreen, tr("Avatar's screen")), SegOption::new(MenuDisplay::Floating, tr("Floating"))];
            let said = match draft.orbit.display {
                MenuDisplay::AvatarScreen => tr("N4N0's own menu, opened by its dedicated button or the watch on your wrist, shows on your avatar's N4N0 screen when it has one, and floats before you when it does not."),
                MenuDisplay::Floating => tr("N4N0's own menu, opened by its dedicated button or the watch on your wrist, floats before you, even with an avatar that has N4N0's screen."),
            };
            widgets::field(ui, tr(ROW_WHERE), Some(said), 300.0, |ui| {
                apply |= widgets::segmented(ui, "orbit-display", &mut draft.orbit.display, &ways, true);
            });
            widgets::separator(ui);
        }
        widgets::field(
            ui,
            tr("Controller button"),
            Some(tr("SteamVR's \"Open N4N0's menu\" binding for Orbiters: two quick taps on the left thumbrest or trackpad (a double press of X on controllers with neither), unless you change it in SteamVR's controller settings. The watch on your wrist opens it too.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "n4n0-menu-button", &mut draft.n4n0.menu.button);
            },
        );
        let say = if draft.n4n0.wake_word {
            tr("The wake word on its own opens the menu.")
        } else {
            tr("The wake word on its own opens the menu, once N4N0 listens for its name: turn on Wake word under N4N0's ears.")
        };
        widgets::field(ui, tr("Say \"Nano\""), Some(say), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-menu-wake", &mut draft.n4n0.menu.wake_word);
        });
        widgets::field(ui, tr("Touch N4N0"), Some(tr("A hand on N4N0 opens the menu; touching it again closes it.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-menu-touch", &mut draft.n4n0.menu.touch);
        });
        widgets::field(ui, tr("Try it"), Some(tr("Opens the menu (or closes it) as the button and N4N0 do.")), 140.0, |ui| {
            if Button::new(tr("Open the menu")).icon(icons::STACK).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                cx.ctl.n4n0(N4n0Command::ToggleScreen);
            }
        });
        if snap.vrchat.running && snap.osc.enabled && !snap.osc.receiving {
            // VRChat sends avatar parameters only to OSC services it found: without them, N4N0's touch and the menu's
            // Talk button never reach Orbiters.
            ui.add_space(4.0);
            note(
                ui,
                tr("VRChat has sent Orbiters nothing for a minute: touching N4N0 and Talk in its menu cannot reach N4N0. Orbiters asks VRChat to find it again by itself; Reconnect asks now. Check that OSC is on in VRChat's Action Menu (Options › OSC)."),
                theme::WARN,
            );
            if Button::new(tr("Reconnect")).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                cx.ctl.osc_reconnect();
            }
        }
        widgets::separator(ui);
        widgets::field(ui, tr("Keyboard"), Some(tr("The letters of the search keyboard. Automatic follows this computer's keyboard.")), 284.0, |ui| {
            use orbiters_core::settings::KeyboardLayout as K;
            let options = [SegOption::new(K::Auto, tr("Automatic")), SegOption::new(K::Qwerty, "QWERTY"), SegOption::new(K::Azerty, "AZERTY")];
            apply |= widgets::segmented(ui, "n4n0-menu-keyboard", &mut draft.n4n0.menu.keyboard, &options, true);
        });
        widgets::field(
            ui,
            tr_ctx("appearance", "Look"),
            Some(tr("Retro: a green phosphor terminal, as on an old computer. N4N0's screen has its own Look, in My Avatar.")),
            200.0,
            |ui| {
                use orbiters_core::settings::MenuLook as L;
                let options = [SegOption::new(L::Modern, tr("Modern")), SegOption::new(L::Retro, tr("Retro"))];
                apply |= widgets::segmented(ui, "n4n0-menu-look", &mut draft.n4n0.menu.look, &options, true);
            },
        );
        widgets::field(ui, tr("Screen size"), Some(tr("How large N4N0's projected screen is, for you and everyone around.")), 330.0, |ui| {
            use orbiters_core::settings::ScreenSize as S;
            let options = [SegOption::new(S::Small, tr("Small")), SegOption::new(S::Medium, tr("Medium")), SegOption::new(S::Large, tr("Large")), SegOption::new(S::Larger, tr("Larger"))];
            apply |= widgets::segmented(ui, "n4n0-screen-size", &mut draft.n4n0.menu.screen_size, &options, true);
        });
    });
    apply
}

/// The avatar gun: who may change this player's avatar with one (contracts `Docs/avatar-gun.md`). The gun itself, its
/// picker and how it works, are on the N4N0 page.
pub fn avatar_gun_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut apply = false;
    section(ui, cx, index, icons::CROSSHAIR, tr(super::avatar_gun::TITLE), Some(tr(super::avatar_gun::SUB)), |ui| {
        let options = super::avatar_gun::allow_options();
        let width = widgets::segmented_width(ui, &options, true);
        widgets::field(ui, tr(super::avatar_gun::WHO), Some(tr(super::avatar_gun::WHO_WHY)), width, |ui| {
            apply |= widgets::segmented(ui, "avatar-gun-allow", &mut draft.avatar_gun.allow, &options, true);
        });
    });
    apply
}

/// Calls on or off, and directly or through the relay. The calls themselves are on the N4N0 page.
pub fn calls_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let calls = &cx.snap.n4n0.calls;
    let mut apply = false;
    let sub = if calls.coming_soon() { tr(super::n4n0::CALLS_SOON_SUB) } else { tr("Call VRChat friends who have Orbiters and N4N0 too") };
    section(ui, cx, index, icons::BROADCAST, tr("Calls"), Some(sub), |ui| {
        widgets::field(
            ui,
            tr("Calls"),
            Some(tr("Your presence and your VRChat friends (as hashes, from VRCX and VRCNext) go to Orbiters, to find friends you can call. Calls go through Orbiters' relay: a friend never sees this computer's address.")),
            48.0,
            |ui| {
                // Greyed out while calls are not open to this account: the setting keeps what it was.
                let soon = calls.coming_soon();
                ui.add_enabled_ui(!soon, |ui| apply |= widgets::toggle(ui, "n4n0-calls", &mut draft.n4n0.calls));
            },
        );
        if calls.coming_soon() {
            widgets::separator(ui);
            widgets::field(ui, tr(orbiters_n4n0_core::menu::CALLS_SOON), Some(tr(super::n4n0::CALLS_SOON_WHY_PAGE)), 0.0, |_| {});
        } else if draft.n4n0.calls {
            widgets::separator(ui);
            widgets::field(
                ui,
                tr("Direct calls"),
                Some(tr("Connect straight to the friend when your networks allow it: a little faster, but each of you sees the other's IP address.")),
                48.0,
                |ui| {
                    apply |= widgets::toggle(ui, "n4n0-direct-calls", &mut draft.n4n0.direct_calls);
                },
            );
        }
        if matches!(calls.link, CallLink::Linked { .. } | CallLink::NotLinked { .. }) {
            ui.add_space(4.0);
            note(ui, tr(CALLS_ON_PAGE), theme::FG_3);
            ui.add_space(8.0);
        }
    });
    apply
}

/// Under Calls: where the calls themselves are.
const CALLS_ON_PAGE: &str = mark("Friends to call, and the call under way, are on the N4N0 page.");

/// The assistant: AI or not, and what is set on the website.
pub fn assistant_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut apply = false;
    section(ui, cx, index, icons::SPARKLE, tr("Assistant"), Some(tr("N4N0's brain runs on Orbiters")), |ui| {
        widgets::field(ui, tr("AI assistant"), Some(tr("Off, N4N0 is a tool without AI: nothing is transcribed, and nothing goes to Orbiters.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-ai", &mut draft.n4n0.assistant);
        });
        widgets::separator(ui);
        widgets::field(ui, tr("Personality and own key"), Some(tr("N4N0's roast level, its language and your own AI key are on the Orbiters website.")), 150.0, |ui| {
            if Button::new(tr("Open")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                let site = if cx.snap.account.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                let _ = crate::controller::open_url(&format!("{}n4n0/workspace?tab=settings&section=n4n0", site.website()));
            }
        });
    });
    apply
}

/// N4N0's ears: the speech models, the microphone it listens to, the wake word, muting VRChat while talking.
pub fn ears_card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings, mic: &mut MicTest) -> bool {
    let snap = cx.snap;
    let voice = &snap.n4n0.voice;
    let mut apply = false;
    // The last tenth of the progress is the unpacking.
    let unpacking = voice.downloading.is_some_and(|f| f >= 0.9);
    let ears_sub = if unpacking {
        tr("Unpacking the speech models…").to_string()
    } else if let Some(f) = voice.downloading {
        tr!("Downloading the speech models… {percent} %", percent = format!("{:.0}", f * 100.0))
    } else if voice.repair {
        tr("The speech models need a repair").to_string()
    } else if !voice.installed {
        tr("Speech models not installed").to_string()
    } else if voice.running {
        tr("Ready: they start with VRChat").to_string()
    } else {
        tr("Installed: they start with VRChat").to_string()
    };
    section(ui, cx, index, icons::MICROPHONE, tr("N4N0's ears"), Some(&ears_sub), |ui| {
        if !voice.installed {
            let desc = tr("Speech to text on this computer, in 25 languages (NVIDIA Parakeet, Silero VAD): about 490 MB to download. Nothing you say leaves it until it is a sentence for N4N0.");
            widgets::field(ui, tr("Speech models"), Some(desc), 280.0, |ui| {
                let busy = voice.downloading.is_some();
                let label = match (busy, unpacking, voice.repair) {
                    (true, true, _) => tr("Unpacking…"),
                    (true, false, _) => tr("Downloading…"),
                    (false, _, true) => tr("Repair (490 MB)"),
                    (false, _, false) => tr("Install (490 MB)"),
                };
                if Button::new(label).icon(icons::DOWNLOAD_SIMPLE).variant(Variant::Primary).size(Size::Sm).enabled(!busy).show(ui).clicked() {
                    cx.ctl.n4n0(N4n0Command::InstallVoice);
                }
                if busy {
                    ui.add_space(8.0);
                    if Button::new(tr("Cancel")).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                        cx.ctl.n4n0(N4n0Command::CancelVoiceInstall);
                    }
                }
            });
            if let Some(f) = voice.downloading {
                ui.add_space(4.0);
                widgets::progress(ui, Id::unique("n4n0-voice-download"), f, 8.0);
                ui.add_space(10.0);
            }
        } else {
            // Installed: the row stays, so the models are found here by name too.
            widgets::field(ui, tr("Speech models"), Some(&ears_sub), 0.0, |_| {});
        }
        widgets::separator(ui);
        if let Some(error) = voice.error.as_deref().filter(|_| voice.downloading.is_none()) {
            note(ui, error, theme::DANGER);
            ui.add_space(8.0);
            widgets::separator(ui);
        }
        widgets::field(ui, tr("Microphone"), Some(tr("The one N4N0 listens to.")), 0.0, |_| {});
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if source_chip(ui, "n4n0-mic", super::settings::default_mic(), draft.n4n0.mic_device.is_empty()).clicked() {
                draft.n4n0.mic_device.clear();
                apply = true;
            }
            for m in &snap.mics {
                if source_chip(ui, "n4n0-mic", &m.name, draft.n4n0.mic_device == m.id).clicked() {
                    draft.n4n0.mic_device = m.id.clone();
                    apply = true;
                }
            }
        });
        ui.add_space(8.0);
        mic_test(ui, mic, &draft.n4n0.mic_device);
        ui.add_space(12.0);
        widgets::separator(ui);
        widgets::field(
            ui,
            tr("Wake word"),
            Some(tr("Start a sentence with “Nano” (N4N0 said aloud) and N4N0 hears it, any time. Every sentence is transcribed on this computer to find it; none is kept.")),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "n4n0-wake", &mut draft.n4n0.wake_word);
            },
        );
        widgets::separator(ui);
        widgets::field(ui, tr("Mute VRChat while I hold Talk"), Some(tr("The sentence for N4N0 stays out of the world.")), 48.0, |ui| {
            apply |= widgets::toggle(ui, "n4n0-mute", &mut draft.n4n0.mute_while_talking);
        });
        ui.add_space(4.0);
        note(ui, tr("In VRChat: hold Talk in N4N0's menu, touch N4N0 with a hand, or start with “Nano” when the wake word is on."), theme::FG_3);
        ui.add_space(8.0);
    });
    apply
}

fn note(ui: &mut Ui, text: &str, color: egui::Color32) {
    let g = ui.painter().layout_job(Txt::new(text, 12.0).color(color).job(ui.available_width()));
    let (r, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
    ui.painter().galley(r.min, g, color);
}
