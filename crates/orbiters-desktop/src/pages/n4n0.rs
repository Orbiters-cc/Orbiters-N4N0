//! N4N0 page (talking to N4N0 is the bar at the top of the window, `crate::chat_bar`, on every page): what N4N0 knows of the avatar (the N4N0 prop and its panel),
//! calls, saved videos, what Orbiters costs this computer, and testing what everyone sees of N4N0 (its states, sample
//! texts, away, hidden, touched). N4N0's settings (the bot, the AFK message, carried and held parameters, its menu, calls
//! on or off, its ears and its assistant) are in Settings › N4N0 companion and Voice & assistant (REQUEST21 2): the
//! heading's Settings button goes there.

use super::{section, Cx};
use crate::ipc::N4n0Command;
use crate::state::{CallLink, CallPhase, CallView, N4n0Face, N4n0Status};
use egui::{pos2, vec2, Align2, Layout, Rect, Sense, Ui, UiBuilder};
use orbiters_i18n::{mark, tr, tr_ctx, tr_latin, tr_n};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

#[derive(Default)]
pub struct State {
    /// The avatar gun's card and its picker (`super::avatar_gun`).
    gun: super::avatar_gun::State,
}

impl State {
    /// The avatar gun's card and picker (the screenshot review stages them).
    pub fn gun_mut(&mut self) -> &mut super::avatar_gun::State {
        &mut self.gun
    }

    /// Leaving the page (talking to N4N0, and a held Talk, are the chat bar's: `crate::chat_bar`).
    pub fn leave(&mut self, _cx: &crate::backend::Ctl) {}
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let snap = cx.snap;
    // A setting of N4N0's asked for (the heading's Settings, the Calls card's): Settings opens on it.
    let mut to_settings: Option<&'static str> = None;
    let n4n0 = &snap.n4n0;

    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt("n4n0").auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            let sub = heading_line(snap);
            super::block(ui, cx, 0, 72.0, |ui, rect| {
                let b = Button::new(tr("Settings")).icon(icons::SLIDERS_HORIZONTAL).variant(Variant::Secondary).size(Size::Sm);
                let bw = b.desired_width(ui);
                super::heading_in(ui, rect, tr("Companion"), "N4N0", Some(&sub), rect.width() - bw - 24.0);
                if b.show_at(ui, Rect::from_min_size(pos2(rect.right() - bw, rect.bottom() - 36.0), vec2(bw, 32.0))).clicked() {
                    to_settings = Some("n4n0");
                }
            });

            // Talking to N4N0 is the bar at the top of the window, on every page (REQUEST21 3).
            let mut m = super::Masonry::new(ui, "n4n0");
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
                // Calls on or off, and directly or not, are settings (Settings › N4N0 companion); here, what calls do.
                let (why, settings) = if calls.coming_soon() {
                    (tr(CALLS_SOON_WHY_PAGE), false)
                } else if snap.settings.n4n0.calls {
                    (tr(CALLS_ON), true)
                } else {
                    (tr(CALLS_OFF), true)
                };
                widgets::field(ui, tr("Calls"), Some(why), 150.0, |ui| {
                    if settings && Button::new(tr("Settings")).icon(icons::SLIDERS_HORIZONTAL).variant(Variant::Secondary).size(Size::Sm).show(ui).clicked() {
                        to_settings = Some("n4n0.calls");
                    }
                });
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
                        // Each for the call it shows: another one's button is another button.
                        let number = Some(call.number);
                        if Button::new(end).icon(icons::X).variant(Variant::Ghost).size(Size::Sm).id_salt(("n4n0-call-end", number)).confirm(ui) {
                            cx.ctl.n4n0(N4n0Command::EndCall { call: number, stamp: None });
                        }
                        ui.add_space(8.0);
                        if ringing {
                            if Button::new(tr("Answer")).icon(icons::MICROPHONE).variant(Variant::Primary).size(Size::Sm).id_salt(("n4n0-call-answer", number)).confirm(ui) {
                                cx.ctl.n4n0(N4n0Command::AnswerCall { call: number, stamp: None });
                            }
                        } else {
                            let label = if call.muted { tr_ctx("call", "Unmute") } else { tr_ctx("call", "Mute") };
                            if Button::new(label).icon(icons::MICROPHONE).size(Size::Sm).id_salt(("n4n0-call-mute", number, call.muted)).show(ui).clicked() {
                                cx.ctl.n4n0(N4n0Command::MuteCall { on: !call.muted, call: number, stamp: None });
                            }
                        }
                    });
                }
                if matches!(calls.link, CallLink::Linked { .. }) {
                    for contact in &calls.contacts {
                        widgets::separator(ui);
                        widgets::field(ui, &contact.name, Some(tr("Has Orbiters and N4N0 on now")), 100.0, |ui| {
                            if Button::new(tr("Call")).icon(icons::BROADCAST).size(Size::Sm).enabled(calls.call.is_none()).confirm(ui) {
                                cx.ctl.n4n0(N4n0Command::Call { contact: contact.id.clone(), stamp: None });
                            }
                        });
                    }
                }
            }));

            let videos_sub = if n4n0.videos.is_empty() { tr("Ask N4N0 to “save this video” while a world plays one").to_string() } else { tr!("{count} saved", count = n4n0.videos.len()) };
            // The avatar gun: what it is loaded with, its picker, who may change this avatar (where VRChat runs here).
            if orbiters_core::osc::VRCHAT_RUNS_HERE {
                let gun = &mut state.gun;
                m.add(6, |ui| super::avatar_gun::card(ui, cx, 6, gun));
            }
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
                        if Button::new("").icon(icons::TRASH).variant(Variant::Ghost).size(Size::Sm).hint(tr("Remove")).confirm(ui) {
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
    if let Some(target) = to_settings {
        cx.go = Some(super::Go::Settings(target));
    }
    // The avatar gun's picker, over the page while it shows.
    let ctx = ui.ctx().clone();
    super::avatar_gun::detail(&ctx, cx, &mut state.gun);
}

pub(super) fn heading_line(snap: &crate::state::Snapshot) -> String {
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
pub(super) fn input_line(n4n0: &N4n0Status) -> String {
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
pub(super) const CALLS_SOON_SUB: &str = mark("Coming soon: calls aren't open for your account yet");
/// The Calls card's first row, while calls are on or off for this account (the switch is in Settings).
const CALLS_ON: &str = mark("On: friends with Orbiters and N4N0 can call you, and you them. Turned off, and direct or not, in Settings.");
const CALLS_OFF: &str = mark("Off: nobody can call you. Turn calls on in Settings to call friends who have Orbiters and N4N0 too.");
pub(super) const CALLS_SOON_WHY_PAGE: &str = mark("Calls between VRChat friends are on their way to Orbiters - N4N0. Your Orbiters account doesn't have them yet; they'll open here once it does.");

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
