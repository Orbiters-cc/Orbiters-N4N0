//! N4N0 page: talking to N4N0 (typed, or held to speak), what N4N0 knows of the avatar (the N4N0 prop and its panel, the
//! AFK message, carried and held parameters), saved videos, N4N0's ears (speech models, microphone, wake word), its
//! assistant (on, or off for a tool without AI), what Orbiters costs this computer, and testing what everyone sees of
//! N4N0 (its states, sample texts, away, hidden, touched). Changes apply as they are made; the AFK message with
//! its Save button (see [`super::form`]).

use super::form::{self, Texts};
use super::mic::{mic_test, MicTest};
use super::settings::source_chip;
use super::{section, Cx};
use crate::ipc::N4n0Command;
use crate::state::{AccountState, CallLink, CallPhase, CallView, N4n0Face, N4n0Line, N4n0Status, N4n0Who};
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder};
use orbiters_core::settings::{Settings, GOGOLOCO_PARAMETERS};
use orbiters_i18n::{mark, tr, tr_ctx, tr_latin, tr_n};
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
            cx.n4n0(N4n0Command::Talk { down: false });
        }
    }
}

/// How the assistant stands, for the conversation's subtitle.
fn assistant_line(n4n0: &N4n0Status, settings: &Settings, account: &AccountState) -> String {
    if !settings.n4n0.enabled {
        return tr("N4N0 is off").into();
    }
    if !settings.n4n0.assistant {
        return tr("Assistant off: N4N0 is a tool without AI").into();
    }
    if !matches!(account, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. }) {
        return tr("Sign in to Orbiters (Settings) to talk to N4N0").into();
    }
    match &n4n0.quota {
        Some(q) if q.own_key => tr("Answers on your own key").into(),
        Some(q) if q.limit > 0 => tr!("{left} of {limit} free answers left today", left = q.limit.saturating_sub(q.used), limit = q.limit),
        _ => tr("Ask anything, or hold Talk and speak").into(),
    }
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    // The controls edit a copy of the settings as published; what they change is saved over the settings as they
    // are by then (`form::save`), so a pin N4N0 just added stays.
    let base = cx.snap.settings.clone();
    let mut draft = base.clone();
    let mut apply = false;
    let snap = cx.snap;
    let n4n0 = &snap.n4n0;

    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt("n4n0").auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            let sub = heading_line(snap);
            super::block(ui, cx, 0, 72.0, |ui, rect| super::heading(ui, rect, tr("Companion"), "N4N0", Some(&sub)));

            let talk_sub = assistant_line(n4n0, &snap.settings, &snap.account.state);
            let mut m = super::Masonry::new(ui, "n4n0");
            m.add(1, |ui| section(ui, cx, 1, icons::N4N0, tr("Talk to N4N0"), Some(&talk_sub), |ui| {
                conversation(ui, n4n0);
                ui.add_space(12.0);
                let signed_in = matches!(snap.account.state, AccountState::SignedIn { .. } | AccountState::Offline { .. } | AccountState::Checking { .. });
                let can_ask = snap.settings.n4n0.enabled && snap.settings.n4n0.assistant && signed_in;
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                // A longer word in another language widens its button, and the field gives the room.
                let talk_label = if state.talking { tr("Listening…") } else { tr("Hold to talk") };
                let talk_w = Button::new(talk_label).icon(icons::MICROPHONE).desired_width(ui).max(132.0);
                let ask_w = Button::new(tr("Ask")).icon(icons::SPARKLE).desired_width(ui).max(92.0);
                let field = Rect::from_min_max(row.min, pos2(row.right() - talk_w - ask_w - 16.0, row.max.y));
                let mut fu = ui.new_child(UiBuilder::new().max_rect(field).layout(Layout::left_to_right(egui::Align::Center)));
                let resp = widgets::text_field(&mut fu, "n4n0-ask", &mut state.ask, tr("Ask N4N0 something…"), field.width(), false);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ask_rect = Rect::from_min_size(pos2(field.right() + 8.0, row.top()), vec2(ask_w, 40.0));
                let ask = Button::new(tr("Ask")).icon(icons::SPARKLE).variant(Variant::Primary).enabled(can_ask && !n4n0.thinking && !state.ask.trim().is_empty()).width(ask_w).show_at(ui, ask_rect);
                if (ask.clicked() || enter) && can_ask && !state.ask.trim().is_empty() && !n4n0.thinking {
                    cx.ctl.n4n0(N4n0Command::Ask { text: std::mem::take(&mut state.ask) });
                }
                // Talk: held to speak, like the menu's button. One widget whatever its label says: the press that
                // changes the label keeps holding it.
                let talk_rect = Rect::from_min_size(pos2(ask_rect.right() + 8.0, row.top()), vec2(talk_w, 40.0));
                let talk = Button::new(talk_label)
                    .icon(icons::MICROPHONE)
                    .enabled(can_ask && n4n0.voice.installed)
                    .width(talk_w)
                    .id_salt("n4n0-talk")
                    .show_at(ui, talk_rect);
                let held = talk.is_pointer_button_down_on();
                if held != state.talking && (can_ask && n4n0.voice.installed || state.talking) {
                    state.talking = held;
                    cx.ctl.n4n0(N4n0Command::Talk { down: held });
                }
                if !n4n0.voice.installed && can_ask {
                    ui.add_space(6.0);
                    note(ui, tr("Install N4N0's ears below to talk instead of typing."), theme::FG_3);
                }
                for c in &n4n0.confirmations {
                    ui.add_space(12.0);
                    // What N4N0 asks before it does it is answered here (or by saying yes); the rest on Orbiters.
                    let (fallback, label, icon) = if c.is_local() {
                        (tr("N4N0 asks before it does this. Say or type no to leave it."), tr("Yes, do it"), icons::CHECK)
                    } else {
                        (tr("Waiting for your approval on Orbiters."), tr("Approve on Orbiters"), icons::ARROW_SQUARE_OUT)
                    };
                    let desc = if c.summary.is_empty() { fallback } else { c.summary.as_str() };
                    widgets::field(ui, if c.title.is_empty() { tr("Needs your approval") } else { &c.title }, Some(desc), 190.0, |ui| {
                        if Button::new(label).icon(icon).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.n4n0(N4n0Command::OpenLink { url: c.approve_url.clone() });
                        }
                    });
                }
                if !n4n0.lines.is_empty() {
                    ui.add_space(8.0);
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                    let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::right_to_left(egui::Align::Center)));
                    if Button::new(tr("New conversation")).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Ghost).size(Size::Sm).show(&mut bu).clicked() {
                        cx.ctl.n4n0(N4n0Command::NewConversation);
                    }
                }
            }));

            let avatar_sub = avatar_line(snap);
            m.add(2, |ui| section(ui, cx, 2, icons::PERSON, tr("On your avatar"), Some(&avatar_sub), |ui| {
                panel_preview(ui, n4n0);
                ui.add_space(10.0);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
                bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
                if Button::new(tr("Show a test message")).icon(icons::PLAY).size(Size::Sm).enabled(n4n0.prop_sync.is_some()).show(&mut bu).clicked() {
                    // It reaches the avatar's panel, which has no Japanese glyphs: English there.
                    cx.ctl.n4n0(N4n0Command::Say { text: tr_latin(mark("Hi, I'm N4N0. Yes, the one in the menu. No, I'm not your friend.")).into() });
                }
                if Button::new(tr("Clear")).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).enabled(n4n0.panel.is_some()).show(&mut bu).clicked() {
                    cx.ctl.n4n0(N4n0Command::HidePanel);
                }
                let camera = snap.vrchat.running && !snap.is_capturing();
                if Button::new(tr("Picture in 3 s")).icon(icons::CAMERA).variant(Variant::Ghost).size(Size::Sm).enabled(camera).show(&mut bu).clicked() {
                    cx.ctl.n4n0(N4n0Command::Photo { delay_seconds: 3 });
                }
                ui.add_space(12.0);
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
                let menu = if n4n0.avatar_id.is_none() {
                    tr("N4N0 looks at your avatar once VRChat loads it.").to_string()
                } else if n4n0.manifest {
                    tr!("Orbiters Toolkit described this avatar's menu: N4N0 knows its {count} controls by name.", count = n4n0.controls)
                } else {
                    tr("N4N0 sees only parameter names. Upload the avatar from Unity with Orbiters Toolkit installed to give N4N0 its menu.").to_string()
                };
                widgets::field(ui, tr("Menu"), Some(&menu), 140.0, |ui| {
                    let (t, c) = if n4n0.manifest { (tr("Known by label"), theme::OK) } else if n4n0.avatar_id.is_some() { (tr("Names only"), theme::WARN) } else { ("–", theme::FG_4) };
                    let (r, _) = ui.allocate_exact_size(vec2(130.0, 20.0), Sense::hover());
                    widgets::text(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &Txt::new(t, 12.0).color(c));
                });
                if n4n0.screen && n4n0.prop_sync == Some(8) {
                    ui.add_space(6.0);
                    note(ui, tr("N4N0's screen fills twice as fast with 16 sync values: choose 16 in Orbiters Toolkit's N4N0 settings and upload the avatar again."), theme::FG_3);
                }
                widgets::separator(ui);
                widgets::field(ui, tr("AFK message"), Some(tr("While VRChat says you are away, the panel tells people around you.")), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "n4n0-afk", &mut draft.n4n0.afk_message_enabled);
                });
                if draft.n4n0.afk_message_enabled {
                    widgets::separator(ui);
                    widgets::field(ui, tr("Message"), Some(tr("What the panel says while you are away.")), 440.0, |ui| {
                        if let Some(text) = form::text_row(ui, &mut state.texts, "n4n0-afk-text", &base.n4n0.afk_message, tr("Getting food, back in 10!"), 360.0, false) {
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
                let input = input_line(n4n0);
                widgets::field(ui, tr("Controller input"), Some(&input), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "n4n0-controller-input", &mut draft.n4n0.controller_input);
                });
                if !draft.n4n0.pins.is_empty() {
                    widgets::separator(ui);
                    let mut remove = None;
                    for (i, pin) in draft.n4n0.pins.iter().enumerate() {
                        let value = match &pin.value {
                            orbiters_core::osc::OscValue::Bool(b) => if *b { tr_ctx("value", "on").to_string() } else { tr_ctx("value", "off").to_string() },
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
                    if let Some(i) = remove {
                        draft.n4n0.pins.remove(i);
                        apply = true;
                    }
                }
            }));

            // N4N0's menu: fast walk, YouTube, calls, a 3D picture. (Sections enter in the order they stand on the page.)
            let menu_sub = if n4n0.screen {
                tr("Opens on the screen your N4N0 projects (\"Replace Orbiters overlay\" in My Avatar)")
            } else {
                tr("Opens in the SteamVR overlay, in front of you")
            };
            m.add(3, |ui| section(ui, cx, 3, icons::STACK, tr("N4N0's menu"), Some(menu_sub), |ui| {
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
                    // VRChat sends avatar parameters only to OSC services it found: without them, N4N0's touch and
                    // the menu's Talk button never reach Orbiters.
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
                    let options = [widgets::SegOption::new(K::Auto, tr("Automatic")), widgets::SegOption::new(K::Qwerty, "QWERTY"), widgets::SegOption::new(K::Azerty, "AZERTY")];
                    apply |= widgets::segmented(ui, "n4n0-menu-keyboard", &mut draft.n4n0.menu.keyboard, &options, true);
                });
                widgets::field(
                    ui,
                    tr_ctx("appearance", "Look"),
                    Some(tr("Retro: a green phosphor terminal, as on an old computer. N4N0's screen has its own Look, in My Avatar.")),
                    200.0,
                    |ui| {
                        use orbiters_core::settings::MenuLook as L;
                        let options = [widgets::SegOption::new(L::Modern, tr("Modern")), widgets::SegOption::new(L::Retro, tr("Retro"))];
                        apply |= widgets::segmented(ui, "n4n0-menu-look", &mut draft.n4n0.menu.look, &options, true);
                    },
                );
                widgets::field(
                    ui,
                    tr("Screen size"),
                    Some(tr("How large N4N0's projected screen is, for you and everyone around.")),
                    330.0,
                    |ui| {
                        use orbiters_core::settings::ScreenSize as S;
                        let options = [
                            widgets::SegOption::new(S::Small, tr("Small")),
                            widgets::SegOption::new(S::Medium, tr("Medium")),
                            widgets::SegOption::new(S::Large, tr("Large")),
                            widgets::SegOption::new(S::Larger, tr("Larger")),
                        ];
                        apply |= widgets::segmented(ui, "n4n0-screen-size", &mut draft.n4n0.menu.screen_size, &options, true);
                    },
                );
            }));

            let calls = &n4n0.calls;
            let calls_sub = match (&calls.call, &calls.link) {
                // Not open to this Orbiters account yet: said, and nothing below can start one.
                (_, CallLink::ComingSoon) => tr(CALLS_SOON_SUB).to_string(),
                (Some(c), _) => call_line(c),
                (None, CallLink::Linked { .. }) if calls.contacts.is_empty() => tr("No friend with Orbiters and N4N0 on right now").to_string(),
                (None, CallLink::Linked { .. }) => tr_n("{count} friend can be called", "{count} friends can be called", calls.contacts.len() as u64),
                (None, CallLink::Off) => tr("Call VRChat friends who have Orbiters and N4N0 too").to_string(),
                (None, CallLink::SignedOut) => tr("Sign in to Orbiters to use calls").to_string(),
                (None, CallLink::NoAccount) => tr("Open VRChat, VRCX or VRCNext once so Orbiters knows your VRChat account").to_string(),
                (None, CallLink::NotLinked { .. }) => tr("Link your VRChat account on Orbiters to be called").to_string(),
            };
            m.add(4, |ui| section(ui, cx, 4, icons::BROADCAST, tr("Calls"), Some(&calls_sub), |ui| {
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
                    widgets::field(ui, tr(orbiters_n4n0_core::menu::CALLS_SOON), Some(tr(CALLS_SOON_WHY_PAGE)), 0.0, |_| {});
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
                if let CallLink::NotLinked { account, source } = &calls.link {
                    widgets::separator(ui);
                    let desc = tr!("{source} says this computer uses {account}. Confirm it's yours on Orbiters (a friend request, once) so friends can call you.", source = source, account = account);
                    widgets::field(ui, tr("Link your VRChat account"), Some(&desc), 160.0, |ui| {
                        if Button::new(tr("Link on Orbiters")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
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
                        let end = if ringing { tr("Decline") } else { tr("Hang up") };
                        if Button::new(end).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.n4n0(N4n0Command::EndCall);
                        }
                        ui.add_space(8.0);
                        if ringing {
                            if Button::new(tr("Answer")).icon(icons::MICROPHONE).variant(Variant::Primary).size(Size::Sm).show(ui).clicked() {
                                cx.ctl.n4n0(N4n0Command::AnswerCall);
                            }
                        } else {
                            let label = if call.muted { tr_ctx("call", "Unmute") } else { tr_ctx("call", "Mute") };
                            if Button::new(label).icon(icons::MICROPHONE).size(Size::Sm).show(ui).clicked() {
                                cx.ctl.n4n0(N4n0Command::MuteCall { on: !call.muted });
                            }
                        }
                    });
                }
                if matches!(calls.link, CallLink::Linked { .. }) {
                    for contact in &calls.contacts {
                        widgets::separator(ui);
                        widgets::field(ui, &contact.name, Some(tr("Has Orbiters and N4N0 on now")), 100.0, |ui| {
                            if Button::new(tr("Call")).icon(icons::BROADCAST).size(Size::Sm).enabled(calls.call.is_none()).show(ui).clicked() {
                                cx.ctl.n4n0(N4n0Command::Call { contact: contact.id.clone() });
                            }
                        });
                    }
                }
            }));

            let videos_sub = if n4n0.videos.is_empty() { tr("Ask N4N0 to “save this video” while a world plays one").to_string() } else { tr!("{count} saved", count = n4n0.videos.len()) };
            m.add(5, |ui| section(ui, cx, 5, icons::FILM_STRIP, tr("Saved videos"), Some(&videos_sub), |ui| {
                if let Some(video) = &snap.vrchat.world.last_video {
                    widgets::field(ui, tr("Playing in this world"), Some(&video.url), 150.0, |ui| {
                        if Button::new(tr("Save it")).icon(icons::DOWNLOAD_SIMPLE).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.n4n0(N4n0Command::SaveVideo);
                        }
                    });
                    if !n4n0.videos.is_empty() {
                        widgets::separator(ui);
                    }
                }
                for (i, v) in n4n0.videos.iter().take(12).enumerate() {
                    if i > 0 {
                        widgets::separator(ui);
                    }
                    let title = v.title.clone().unwrap_or_else(|| v.url.clone());
                    let when = super::local_time(&v.saved_utc);
                    let desc = match &v.world { Some(w) => format!("{w} · {when}"), None => when };
                    widgets::field(ui, &title, Some(&desc), 236.0, |ui| {
                        if Button::new("").icon(icons::TRASH).variant(Variant::Ghost).size(Size::Sm).hint(tr("Remove")).show(ui).clicked() {
                            cx.ctl.n4n0(N4n0Command::RemoveVideo { url: v.url.clone() });
                        }
                        ui.add_space(4.0);
                        if Button::new(tr("Open")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
                            let _ = crate::controller::open_url(&v.url);
                        }
                        ui.add_space(4.0);
                        if Button::new(tr("Copy link")).icon(icons::COPY_SIMPLE).size(Size::Sm).show(ui).clicked() {
                            cx.ctl.n4n0(N4n0Command::CopyText { text: v.url.clone() });
                        }
                    });
                }
                if n4n0.videos.is_empty() && snap.vrchat.world.last_video.is_none() {
                    note(ui, tr("Saved videos stay on this computer. Copy a link and paste it in a world's video player."), theme::FG_3);
                    ui.add_space(8.0);
                }
            }));

            let voice = &n4n0.voice;
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
            m.add(6, |ui| section(ui, cx, 6, icons::MICROPHONE, tr("N4N0's ears"), Some(&ears_sub), |ui| {
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
                    widgets::separator(ui);
                }
                if let Some(error) = voice.error.as_deref().filter(|_| voice.downloading.is_none()) {
                    note(ui, error, theme::DANGER);
                    ui.add_space(8.0);
                    widgets::separator(ui);
                }
                widgets::field(ui, tr("Microphone"), Some(tr("The one N4N0 listens to.")), 0.0, |_| {});
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    if source_chip(ui, super::settings::default_mic(), draft.n4n0.mic_device.is_empty()).clicked() {
                        draft.n4n0.mic_device.clear();
                        apply = true;
                    }
                    for m in &snap.mics {
                        if source_chip(ui, &m.name, draft.n4n0.mic_device == m.id).clicked() {
                            draft.n4n0.mic_device = m.id.clone();
                            apply = true;
                        }
                    }
                });
                ui.add_space(8.0);
                mic_test(ui, &mut state.mic, &draft.n4n0.mic_device);
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
            }));

            m.add(7, |ui| section(ui, cx, 7, icons::SPARKLE, tr("Assistant"), Some(tr("N4N0's brain runs on Orbiters")), |ui| {
                widgets::field(ui, "N4N0", Some(tr("The panel, the AFK message, carried and held parameters, saved videos.")), 48.0, |ui| {
                    apply |= widgets::toggle(ui, "n4n0-enabled", &mut draft.n4n0.enabled);
                });
                widgets::separator(ui);
                widgets::field(
                    ui,
                    tr("AI assistant"),
                    Some(tr("Off, N4N0 is a tool without AI: nothing is transcribed, and nothing goes to Orbiters.")),
                    48.0,
                    |ui| {
                        apply |= widgets::toggle(ui, "n4n0-ai", &mut draft.n4n0.assistant);
                    },
                );
                widgets::separator(ui);
                widgets::field(ui, tr("Personality and own key"), Some(tr("N4N0's roast level, its language and your own AI key are on the Orbiters website.")), 150.0, |ui| {
                    if Button::new(tr("Open")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        let site = if snap.account.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                        let _ = crate::controller::open_url(&format!("{}n4n0/workspace?tab=settings&section=n4n0", site.website()));
                    }
                });
            }));

            m.add(8, |ui| section(ui, cx, 8, icons::CPU, tr("Footprint"), Some(tr("What Orbiters and VRChat use on this computer")), |ui| {
                if n4n0.resources.is_empty() {
                    note(ui, tr("Measured every few seconds while Orbiters runs."), theme::FG_3);
                    ui.add_space(8.0);
                }
                for (i, p) in n4n0.resources.iter().enumerate() {
                    if i > 0 {
                        widgets::separator(ui);
                    }
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    let painter = ui.painter();
                    widgets::text(painter, row.left_center(), Align2::LEFT_CENTER, &Txt::new(&p.name, 13.5).weight(weight::MEDIUM));
                    let usage = format!("{:.0} % CPU · {:.0} MB", p.cpu_percent, p.memory_mb);
                    widgets::text(painter, row.right_center(), Align2::RIGHT_CENTER, &Txt::new(usage, 12.5).color(theme::FG_2).mono());
                }
            }));

            let prop = n4n0.prop_sync.is_some();
            let testing_sub = match n4n0.prop_sync {
                Some(n) => tr!("What everyone sees of your N4N0 · {count} text slots", count = n),
                None => tr("Wear an avatar with the N4N0 prop in VRChat to test it").to_string(),
            };
            m.add(9, |ui| section(ui, cx, 9, icons::FLASK, tr("Testing"), Some(&testing_sub), |ui| {
                // The button's own width (a longer word in another language), so the description does not run under it.
                let play_w = Button::new(tr("Play every state")).icon(icons::PLAY).size(Size::Sm).desired_width(ui).max(150.0);
                widgets::field(
                    ui,
                    tr("States"),
                    Some(tr("Hold N4N0 in one state, as everyone sees it, or play them all, two seconds each. Automatic follows what N4N0 is doing.")),
                    play_w + 8.0,
                    |ui| {
                        if Button::new(tr("Play every state")).icon(icons::PLAY).size(Size::Sm).enabled(prop && !n4n0.test_cycle).show(ui).clicked() {
                            cx.ctl.n4n0(N4n0Command::TestCycle);
                        }
                    },
                );
                ui.add_space(8.0);
                let mut face = n4n0.test_face;
                let mut options = vec![widgets::SegOption::new(None, tr("Automatic"))];
                options.extend(N4n0Face::ALL.iter().map(|f| widgets::SegOption::new(Some(*f), state_label(*f))));
                ui.add_enabled_ui(prop, |ui| {
                    if widgets::segmented(ui, "n4n0-test-face", &mut face, &options, true) {
                        cx.ctl.n4n0(N4n0Command::TestFace { face });
                    }
                });
                ui.add_space(12.0);
                widgets::separator(ui);
                widgets::field(ui, tr("Panel text"), Some(tr("Sample texts on the panel: a short line, a full panel (its edges marked), several pages, every glyph.")), 0.0, |_| {});
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let mut bu = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(egui::Align::Center)));
                bu.spacing_mut().item_spacing = vec2(8.0, 0.0);
                for (label, text) in [(tr("Short"), tr_latin(TEST_SHORT).to_string()), (tr("Full panel"), test_full_panel()), (tr("Several pages"), tr_latin(TEST_PAGES).to_string()), (tr("Every glyph"), test_every_glyph())] {
                    if Button::new(label).variant(Variant::Secondary).size(Size::Sm).enabled(prop).show(&mut bu).clicked() {
                        cx.ctl.n4n0(N4n0Command::Say { text });
                    }
                }
                if Button::new(tr("Clear")).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).enabled(n4n0.panel.is_some()).show(&mut bu).clicked() {
                    cx.ctl.n4n0(N4n0Command::HidePanel);
                }
                ui.add_space(12.0);
                widgets::separator(ui);
                let mut step = draft.n4n0.text_step_ms as f32;
                widgets::field(ui, tr("Panel speed"), Some(tr("Time between two blocks of text. Faster than 250 ms, people around you miss blocks.")), 284.0, |ui| {
                    if widgets::slider(ui, "n4n0-test-step", &mut step, 250.0..=1000.0, 50.0, |v| format!("{v:.0} ms")) {
                        draft.n4n0.text_step_ms = step as u64;
                        apply = true;
                    }
                });
                widgets::separator(ui);
                let mut away = n4n0.test_afk;
                widgets::field(ui, tr("Act as away"), Some(tr("The sleeping N4N0 and your AFK message, as when VRChat says you are away.")), 48.0, |ui| {
                    if widgets::toggle(ui, "n4n0-test-afk", &mut away) {
                        cx.ctl.n4n0(N4n0Command::TestAfk { on: away });
                    }
                });
                widgets::separator(ui);
                let mut diagnostics = n4n0.test_pictures;
                widgets::field(
                    ui,
                    tr("Picture diagnostics"),
                    Some(tr("The avatar's screen frames each picture in the colour of why it shows its thumbnail or its full picture, and shows the textures' state in a row at its bottom left (N4N0 screens of revision 12).")),
                    64.0,
                    |ui| {
                        if widgets::toggle(ui, "n4n0-test-pictures", &mut diagnostics) {
                            cx.ctl.n4n0(N4n0Command::TestPictures { on: diagnostics });
                        }
                    },
                );
                widgets::separator(ui);
                widgets::field(ui, tr("Touch N4N0"), Some(tr("As if your hand touched it: the menu opens (or N4N0 listens for one sentence, with that off).")), 110.0, |ui| {
                    let ears = n4n0.voice.installed && snap.settings.n4n0.assistant;
                    if Button::new(tr("Touch")).icon(icons::CROSSHAIR).variant(Variant::Secondary).size(Size::Sm).enabled(prop && ears).show(ui).clicked() {
                        cx.ctl.n4n0(N4n0Command::TestTouch);
                    }
                });
            }));
            m.finish(ui);
        });
    });
    super::under_top(ui, cx, "n4n0", from, ui.max_rect().top() + cx.top + 8.0);

    if apply {
        form::save(cx.ctl, &base, &draft);
    }
}

fn heading_line(snap: &crate::state::Snapshot) -> String {
    let n4n0 = &snap.n4n0;
    if !snap.settings.n4n0.enabled {
        return tr("N4N0 is off").into();
    }
    if !snap.vrchat.running {
        return tr("Waiting for VRChat").into();
    }
    match (&snap.vrchat.world.avatar_name, n4n0.prop_sync) {
        (Some(a), Some(_)) => tr!("On “{avatar}”", avatar = a),
        (Some(a), None) => tr!("“{avatar}” has no N4N0: add it from My Avatar's gallery", avatar = a),
        (None, Some(_)) => tr("On your avatar").into(),
        (None, None) => tr("Your avatar has no N4N0: add it from My Avatar's gallery").into(),
    }
}

/// What Controller input does, and what the worn avatar takes of it (contracts `Docs/n4n0-input.md`).
fn input_line(n4n0: &N4n0Status) -> String {
    let base = tr("Your touchpads and sticks reach avatars made to use N4N0's input parameters (Orbiters Toolkit's README tells creators how).");
    match (n4n0.avatar_id.is_some(), n4n0.input_params) {
        (false, _) => base.to_string(),
        (true, 0) => tr!("{base} This avatar uses none of them.", base = base),
        (true, 1) => tr!("{base} This avatar uses one.", base = base),
        (true, n) => tr!("{base} This avatar uses {count}.", base = base, count = n),
    }
}

fn avatar_line(snap: &crate::state::Snapshot) -> String {
    let n4n0 = &snap.n4n0;
    match (n4n0.prop_sync, n4n0.face) {
        (Some(_), face) if snap.vrchat.running => face_line(face).to_string(),
        (Some(n), _) => tr!("N4N0 prop with {count} text values", count = n),
        (None, _) if n4n0.avatar_id.is_some() => tr("No N4N0 prop on this avatar").into(),
        _ => tr("No avatar yet").into(),
    }
}

/// Where a call stands, in a few words.
/// The Calls section's line, and what it says under the switch, while calls are not open to this account.
const CALLS_SOON_SUB: &str = mark("Coming soon: calls aren't open for your account yet");
const CALLS_SOON_WHY_PAGE: &str = mark("Calls between VRChat friends are on their way to Orbiters - N4N0. Your Orbiters account doesn't have them yet; they'll open here once it does.");

fn call_line(call: &CallView) -> String {
    match call.phase {
        CallPhase::Calling => tr!("Calling {name}…", name = call.name),
        CallPhase::Ringing => tr!("{name} is calling", name = call.name),
        CallPhase::Connecting => tr("Connecting…").to_string(),
        CallPhase::Connected => {
            let since = call.connected_at_ms.map(|t| now_ms().saturating_sub(t) / 1000).unwrap_or(0);
            let time = format!("{}:{:02}", since / 60, since % 60);
            if call.muted {
                tr!("On a call · {time} · muted", time = time)
            } else {
                tr!("On a call · {time}", time = time)
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A state's name in the Testing section.
fn state_label(face: N4n0Face) -> &'static str {
    match face {
        N4n0Face::Idle => tr("Idle"),
        N4n0Face::Listening => tr("Listening"),
        N4n0Face::Thinking => tr("Thinking"),
        N4n0Face::Talking => tr("Talking"),
        N4n0Face::Sleeping => tr("Sleeping"),
        N4n0Face::Happy => tr("Happy"),
        N4n0Face::Annoyed => tr("Annoyed"),
        N4n0Face::Surprised => tr("Surprised"),
    }
}

const TEST_SHORT: &str = mark("Testing, testing. Is this thing on?");
const TEST_PAGES: &str = mark("This is page one of a test that runs over several pages, to check that each page stays up long enough to read. \
Here comes page two: accents à é è ù ç ô, a little Cyrillic, Привет, and symbols ♥ ★ ♪ → ✓. \
And this is the third and last page. When it has been read, the panel goes away on its own.");

/// Four rows of exactly 32 characters, their first and last cells marked: the panel's edges.
fn test_full_panel() -> String {
    (1..=4).map(|r| format!("[{r}{}]", "·".repeat(29))).collect::<Vec<_>>().join("\n")
}

/// Every glyph of the panel but the space, in table order (two pages).
fn test_every_glyph() -> String {
    orbiters_contracts::n4n0::glyphs().iter().enumerate().filter(|(i, c)| *i > 0 && **c != '\0').map(|(_, c)| *c).collect()
}

/// What N4N0 is doing, as the subtitle says it: a whole sentence for each, so other languages can build it their way.
fn face_line(face: crate::state::N4n0Face) -> &'static str {
    use crate::state::N4n0Face::*;
    match face {
        Idle => tr("N4N0 is floating around"),
        Listening => tr("N4N0 is listening"),
        Thinking => tr("N4N0 is thinking"),
        Talking => tr("N4N0 is talking"),
        Sleeping => tr("N4N0 is asleep"),
        Happy => tr("N4N0 is happy"),
        Annoyed => tr("N4N0 is annoyed"),
        Surprised => tr("N4N0 is surprised"),
    }
}

fn note(ui: &mut Ui, text: &str, color: egui::Color32) {
    let g = ui.painter().layout_job(Txt::new(text, 12.0).color(color).job(ui.available_width()));
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// The last lines of the conversation: the person's to the right, N4N0's in green to the left, what N4N0 did in small.
fn conversation(ui: &mut Ui, n4n0: &N4n0Status) {
    let width = ui.available_width();
    let lines: Vec<&N4n0Line> = n4n0.lines.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    if lines.is_empty() && !n4n0.thinking && !n4n0.listening {
        let (r, _) = ui.allocate_exact_size(vec2(width, 64.0), Sense::hover());
        ui.painter().rect_filled(r, theme::radius(14.0), theme::white(0.03));
        widgets::text(
            ui.painter(),
            r.center(),
            Align2::CENTER_CENTER,
            &Txt::new(tr("“Nano, put my hoodie on.” “Nano, plan a movie night Saturday.” “Nano, save this video.”"), 12.5).color(theme::FG_3),
        );
        return;
    }
    for line in lines {
        let max = (width * 0.72).min(560.0);
        match line.who {
            N4n0Who::You | N4n0Who::N4n0 => {
                let mine = line.who == N4n0Who::You;
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
                    n4n0_dot(ui, pos2(row.left() + 13.0, bubble.top() + 15.0), 11.0);
                }
            }
            N4n0Who::Note | N4n0Who::Error => {
                let color = if line.who == N4n0Who::Error { theme::DANGER } else { theme::FG_3 };
                let g = ui.painter().layout_job(Txt::new(&line.text, 11.5).color(color).job(width - 34.0));
                let (row, _) = ui.allocate_exact_size(vec2(width, g.size().y + 8.0), Sense::hover());
                ui.painter().galley(pos2(row.left() + 34.0, row.top() + 2.0), g, color);
            }
        }
    }
    // The answer arriving says it all by itself: the dots are for before its first words.
    let arriving = n4n0.lines.last().is_some_and(|l| l.streaming);
    if (n4n0.thinking && !arriving) || n4n0.listening {
        let (row, _) = ui.allocate_exact_size(vec2(width, 38.0), Sense::hover());
        let bubble = Rect::from_min_size(pos2(row.left() + 34.0, row.top()), vec2(if n4n0.listening { 150.0 } else { 64.0 }, 30.0));
        ui.painter().rect_filled(bubble, theme::radius(14.0), theme::alpha(theme::ACCENT, 0.13));
        n4n0_dot(ui, pos2(row.left() + 13.0, bubble.center().y), 11.0);
        let t = anim::time(ui.ctx());
        if n4n0.listening {
            // The microphone's level, as N4N0 hears it.
            let level = n4n0.voice.level.clamp(0.0, 1.0);
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

/// N4N0's little face: a green disc with two eyes.
fn n4n0_dot(ui: &Ui, center: egui::Pos2, radius: f32) {
    let p = ui.painter();
    p.circle_filled(center, radius, theme::ACCENT);
    for dx in [-3.6f32, 3.6] {
        p.rect_filled(Rect::from_center_size(center + vec2(dx, -0.5), vec2(2.4, 5.0)), theme::radius(1.2), theme::ON_ACCENT);
    }
}

/// The panel as people see it: four rows of 32 characters, white on a dark glass, empty when N4N0 is quiet.
fn panel_preview(ui: &mut Ui, n4n0: &N4n0Status) {
    let width = ui.available_width().min(520.0);
    let (r, _) = ui.allocate_exact_size(vec2(width, 112.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, theme::radius(14.0), theme::INK_950);
    match (&n4n0.panel, n4n0.prop_sync) {
        (Some(text), _) => {
            for (i, row) in text.lines().take(4).enumerate() {
                widgets::text(p, pos2(r.left() + 18.0, r.top() + 22.0 + i as f32 * 22.0), Align2::LEFT_CENTER, &Txt::new(row, 14.0).mono().color(theme::FG));
            }
        }
        (None, Some(_)) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(tr("The panel is hidden until N4N0 speaks"), 12.5).color(theme::FG_4));
        }
        (None, None) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(tr("No N4N0 on this avatar: add it from My Avatar's gallery"), 12.5).color(theme::FG_4));
        }
    }
    widgets::text(p, pos2(r.right() - 14.0, r.bottom() - 12.0), Align2::RIGHT_CENTER, &Txt::caps(tr("Panel")).color(theme::FG_4));
}
