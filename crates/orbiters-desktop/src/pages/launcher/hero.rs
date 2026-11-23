//! The top of Home: the two Play buttons, always there (REQUEST15 7a), with the VRChat profile they start on under
//! them (each profile its own account; one VRChat per profile, several side by side), then a received invite's card
//! with both Play buttons in it.

use super::states::{self, PlayState};
use super::{access_name, cover_uv, region_tag, Cx, State};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_core::launcher::{Action, EngineSetup, GameProfile, Invite, InviteKind, LaunchMode};
use orbiters_core::vrchat_launch::VR_HERE;
use orbiters_i18n::tr;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};

const PLAY_H: f32 = 84.0;
/// The Steam sign-in's card on a Mac: its QR code and what to do with it.
const SIGN_IN_H: f32 = 196.0;
/// The profile choice under the Play buttons: its chips' height, the room above it.
const CHIP_H: f32 = 32.0;
const CHIP_GAP: f32 = 8.0;
const CHOICE_TOP: f32 = 12.0;
const INVITE_H: f32 = 188.0;
const LINE_H: f32 = 52.0;

/// The invite to show: the newest one not put away; none of a list whose account is gone.
fn invite(state: &State) -> Option<Invite> {
    if state.stale() {
        return None;
    }
    state.data.invites.iter().find(|i| i.kind == InviteKind::Invite && !state.dismissed.contains(&i.id)).cloned()
}

/// Home's Play part as block `index`; nothing while there is nothing to play. On a PC the Play buttons stay whatever
/// runs, with the profile choice; an invite's card follows them.
pub fn hero(ui: &mut Ui, cx: &mut Cx, state: &mut State, index: usize) {
    if !state.shown() {
        return;
    }
    if VR_HERE {
        return hero_profiles(ui, cx, state, index);
    }
    let play = making(state, states::play(&state.data));
    // A Mac whose engine has a record it cannot read: said under the Play button, with what can be done.
    if let Some(trouble) = state.data.start_trouble.clone() {
        hero_play(ui, cx, state, index, play);
        return super::super::block(ui, cx, index + 1, LINE_H, |ui, rect| start_trouble(ui, cx, state, rect, &trouble));
    }
    // A Mac about to start VRChat in Low Power Mode while on power: said, with where to change it (never changed here).
    #[cfg(target_os = "macos")]
    if matches!(play, PlayState::Ready | PlayState::Starting(_)) && crate::mac_display::low_power_on_power() {
        hero_play(ui, cx, state, index, play);
        return super::super::block(ui, cx, index + 1, LINE_H, low_power);
    }
    hero_play(ui, cx, state, index, play)
}

/// Low Power Mode is on while the Mac is on power: a neutral line and a button to Battery settings.
#[cfg(target_os = "macos")]
fn low_power(ui: &mut Ui, rect: Rect) {
    let button = widgets::Button::new(tr("Battery settings")).variant(widgets::Variant::Secondary).id_salt("launcher-low-power");
    let bw = button.desired_width(ui);
    let at = Rect::from_min_size(pos2(rect.right() - 12.0 - bw, rect.center().y - 16.0), vec2(bw, 32.0));
    let p = ui.painter().clone();
    widgets::frame(&p, rect, 16.0, theme::surface_soft());
    widgets::icon(&p, pos2(rect.left() + 26.0, rect.center().y), icons::INFO, 17.0, theme::FG_3);
    let line = tr("Low Power Mode is on while this Mac is plugged in: VRChat may run slower.");
    widgets::text_truncated(&p, pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), at.left() - rect.left() - 58.0);
    if button.show_at(ui, at).clicked() {
        crate::mac_display::open_battery_settings();
    }
}

/// Home's Play part on a PC in Desktop only, or on a Mac.
fn hero_play(ui: &mut Ui, cx: &mut Cx, state: &mut State, index: usize, play: PlayState) {
    // While Where shows above, an invite is one of its places: its card here is for a VRChat already running (Join).
    let invite = invite(state).filter(|_| !state.places.showing);
    // A Mac being set up, at the Steam sign-in: the QR code to read with the phone.
    let sign_in = match (&state.data.engine_setup, play) {
        (Some(EngineSetup::SignIn { qr_url }), PlayState::NotInstalled | PlayState::NoEngine) if !VR_HERE => Some(qr_url.clone()),
        _ => None,
    };
    if let Some(qr_url) = sign_in {
        return super::super::block(ui, cx, index, SIGN_IN_H, |ui, rect| steam_sign_in(ui, rect, &qr_url));
    }
    let height = match (&invite, play) {
        (Some(_), PlayState::Ready | PlayState::Starting(_) | PlayState::Running(_)) => INVITE_H,
        (_, PlayState::Ready | PlayState::Starting(_)) => PLAY_H,
        (_, PlayState::Running(_) | PlayState::NotInstalled | PlayState::NoEngine) => LINE_H,
        (_, PlayState::Unavailable | PlayState::OtherStarting) => return,
    };
    super::super::block(ui, cx, index, height, |ui, rect| match (invite, play) {
        (Some(invite), PlayState::Ready | PlayState::Starting(_) | PlayState::Running(_)) => invite_card(ui, cx, state, rect, &invite, play),
        (_, PlayState::Ready | PlayState::Starting(_)) => {
            if !VR_HERE {
                // A Mac: one button, the desktop game.
                if play_button(ui, rect, LaunchMode::Desktop, play, true, None) {
                    super::destinations::play(state, cx, anim::time(ui.ctx()), LaunchMode::Desktop, None);
                }
                return;
            }
            let gap = 12.0;
            let w = (rect.width() - gap) * 0.5;
            let vr = Rect::from_min_size(rect.min, vec2(w, rect.height()));
            let desk = Rect::from_min_size(pos2(vr.right() + gap, rect.top()), vec2(w, rect.height()));
            if play_button(ui, vr, LaunchMode::Vr, play, true, None) {
                super::destinations::play(state, cx, anim::time(ui.ctx()), LaunchMode::Vr, None);
            }
            if play_button(ui, desk, LaunchMode::Desktop, play, true, None) {
                super::destinations::play(state, cx, anim::time(ui.ctx()), LaunchMode::Desktop, None);
            }
        }
        (_, PlayState::Running(mode)) => {
            let p = ui.painter();
            widgets::frame(p, rect, 16.0, theme::alpha(theme::ACCENT, 0.08));
            widgets::status_dot(p, pos2(rect.left() + 24.0, rect.center().y), widgets::Tone::Ok, true);
            let line = match mode {
                LaunchMode::Vr => tr("VRChat is running in VR"),
                LaunchMode::Desktop => tr("VRChat is running on the desktop"),
            };
            widgets::text(p, pos2(rect.left() + 42.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 14.0).weight(weight::MEDIUM));
        }
        (_, missing) if !VR_HERE => {
            // A Mac: VRChat is not set up yet. One button does it all; while it runs, what it is at.
            let setup = state.data.engine_setup.clone();
            let percent_of = |p: Option<u8>| p.map(|p| format!("{p} %")).unwrap_or_else(|| "…".into());
            let (line, label, action): (String, Option<&str>, Action) = match setup {
                Some(EngineSetup::Downloading { percent }) => (tr!("Getting what runs VRChat on a Mac: {percent} %", percent = percent), None, Action::InstallEngine),
                Some(EngineSetup::Installing | EngineSetup::Checking | EngineSetup::SignIn { .. }) => (tr("Setting VRChat up on this Mac…").into(), None, Action::InstallEngine),
                Some(EngineSetup::Game { game: false, percent }) => (tr!("Downloading the engine that runs VRChat on a Mac: {progress}", progress = percent_of(percent)), None, Action::InstallEngine),
                Some(EngineSetup::Game { game: true, percent }) => (tr!("Downloading VRChat: {progress}", progress = percent_of(percent)), None, Action::InstallEngine),
                Some(EngineSetup::Failed(_)) | None => (tr("VRChat can run on this Mac with your Steam account (about 2 GB).").into(), Some(tr("Install & play VRChat")), Action::InstallEngine),
            };
            let _ = missing;
            let primary = action == Action::InstallEngine;
            let button = label.map(|label| widgets::Button::new(label).variant(if primary { widgets::Variant::Primary } else { widgets::Variant::Secondary }).id_salt("launcher-engine"));
            let bw = button.as_ref().map_or(0.0, |b| b.desired_width(ui));
            let at = Rect::from_min_size(pos2(rect.right() - 12.0 - bw, rect.center().y - 16.0), vec2(bw, 32.0));
            let p = ui.painter().clone();
            widgets::frame(&p, rect, 16.0, theme::surface_soft());
            match button {
                Some(_) => widgets::icon(&p, pos2(rect.left() + 26.0, rect.center().y), icons::WARNING_CIRCLE, 17.0, theme::WARN),
                None => {
                    orbiters_ui::feedback::spinner(&p, pos2(rect.left() + 26.0, rect.center().y), 8.0, 2.0, theme::ACCENT);
                }
            }
            widgets::text_truncated(&p, pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), at.left() - rect.left() - 58.0);
            if button.is_some_and(|b| b.show_at(ui, at).clicked()) {
                state.act(cx, anim::time(ui.ctx()), action);
            }
        }
        (_, _) => {
            let p = ui.painter();
            widgets::frame(p, rect, 16.0, theme::surface_soft());
            widgets::icon(p, pos2(rect.left() + 26.0, rect.center().y), icons::WARNING_CIRCLE, 17.0, theme::WARN);
            let line = tr("VRChat isn't installed on this computer: install it from Steam or the Meta app, then come back.");
            widgets::text_truncated(p, pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), rect.width() - 60.0);
        }
    });
}

/// A record of the Mac's engine that cannot be read: which file (whole on hover), whether it holds Play, and Set aside once
/// it is known to be history (never before: it may be the record of a start that just ended).
fn start_trouble(ui: &mut Ui, cx: &Cx, state: &mut State, rect: Rect, trouble: &orbiters_core::launcher::StartTrouble) {
    let name = std::path::Path::new(&trouble.file).file_name().map_or_else(|| trouble.file.clone(), |n| n.to_string_lossy().into_owned());
    let line = match (trouble.folder, trouble.blocks, trouble.set_aside_in) {
        (true, _, _) => tr!("VRChat's start records cannot be read: check the permissions of {file}.", file = trouble.file.clone()),
        (false, true, Some(seconds)) => tr!("A record of an earlier VRChat start cannot be read ({file}): Play waits {seconds} s, in case it is a start that just ended.", file = name, seconds = seconds),
        (false, true, None) => tr!("VRChat's engine stops at a damaged record ({file}): set it aside to play.", file = name),
        (false, false, _) => tr!("A record of an earlier VRChat start cannot be read ({file}): it no longer holds Play.", file = name),
    };
    let button = (!trouble.folder && trouble.set_aside_in.is_none()).then(|| widgets::Button::new(tr("Set aside")).variant(if trouble.blocks { widgets::Variant::Primary } else { widgets::Variant::Secondary }).id_salt("launcher-set-aside"));
    let bw = button.as_ref().map_or(0.0, |b| b.desired_width(ui));
    let at = Rect::from_min_size(pos2(rect.right() - 12.0 - bw, rect.center().y - 16.0), vec2(bw, 32.0));
    let p = ui.painter().clone();
    widgets::frame(&p, rect, 16.0, theme::surface_soft());
    widgets::icon(&p, pos2(rect.left() + 26.0, rect.center().y), icons::WARNING_CIRCLE, 17.0, if trouble.blocks { theme::WARN } else { theme::FG_3 });
    let text = Rect::from_min_max(pos2(rect.left() + 46.0, rect.top()), pos2(at.left() - 12.0, rect.bottom()));
    widgets::text_truncated(&p, pos2(text.left(), rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), text.width());
    ui.interact(text, Id::unique("launcher-trouble-file"), Sense::hover()).on_hover_text(trouble.file.clone());
    if let Some(button) = button {
        let clicked = button.show_at(ui, at).on_hover_text(tr("Moves it into the folder Set aside, beside VRChat's engine records: nothing is deleted.")).clicked();
        if clicked {
            state.act(cx, anim::time(ui.ctx()), Action::SetAsideRecord { file: trouble.file.clone() });
        }
    }
}

/// The Play buttons while Where has VRChat make a new instance for the press: the mode pressed turns, as a start does.
fn making(state: &State, play: PlayState) -> PlayState {
    match (play, state.places.making()) {
        (PlayState::Ready, Some(mode)) => PlayState::Starting(mode),
        (play, _) => play,
    }
}

/// A profile's chip: its number and the account last seen on it, how it runs now.
fn chip_label(p: &GameProfile) -> String {
    match &p.account {
        Some(name) if !name.is_empty() => format!("{} · {name}", p.profile),
        _ => orbiters_i18n::tr!("Profile {n}", n = p.profile),
    }
}

fn chip_icon(p: &GameProfile) -> (&'static str, egui::Color32) {
    match (p.running, p.starting) {
        (Some(LaunchMode::Vr), _) => (icons::VIRTUAL_REALITY, theme::ACCENT),
        (Some(LaunchMode::Desktop), _) => (icons::DESKTOP, theme::ACCENT),
        (None, Some(_)) => (icons::HOURGLASS_MEDIUM, theme::WARN),
        (None, None) => (icons::USER, theme::FG_3),
    }
}

/// What a chip says on hover.
fn chip_hint(p: &GameProfile) -> String {
    let whose = if p.assistant { tr("The assistant's VRChat runs on this profile.").to_string() } else { String::new() };
    let state = match (p.running, p.starting) {
        (Some(LaunchMode::Vr), _) => tr("VRChat runs on this profile, in VR."),
        (Some(LaunchMode::Desktop), _) => tr("VRChat runs on this profile, on the desktop."),
        (None, Some(_)) => tr("VRChat is starting on this profile."),
        (None, None) => tr("Each VRChat profile keeps its own sign-in: start VRChat on it to use that account."),
    };
    if whose.is_empty() { state.to_string() } else { format!("{state} {whose}") }
}

/// The Play buttons and the profile choice (Windows).
fn hero_profiles(ui: &mut Ui, cx: &mut Cx, state: &mut State, index: usize) {
    let offered = states::profiles(&state.data, state.added_profile);
    let next = states::next_profile(&offered, state.chosen_profile);
    let play = making(state, states::play_on(&state.data, &offered, next));
    let invite = invite(state).filter(|_| !state.places.showing);
    match play {
        PlayState::Unavailable => return,
        PlayState::NotInstalled | PlayState::NoEngine => {
            super::super::block(ui, cx, index, LINE_H, |ui, rect| {
                let p = ui.painter();
                widgets::frame(p, rect, 16.0, theme::surface_soft());
                widgets::icon(p, pos2(rect.left() + 26.0, rect.center().y), icons::WARNING_CIRCLE, 17.0, theme::WARN);
                let line = tr("VRChat isn't installed on this computer: install it from Steam or the Meta app, then come back.");
                widgets::text_truncated(p, pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), rect.width() - 60.0);
            });
            return;
        }
        _ => {}
    }
    // The chips, wrapped in rows when the page is narrow.
    let caption = Txt::new(tr("VRChat profile"), 12.5).weight(weight::MEDIUM).color(theme::FG_3);
    let caption_w = widgets::text_size(ui, &caption).x + 14.0;
    let new = states::new_profile(&offered);
    let new_label = tr("Another profile");
    let mut widths: Vec<f32> = offered.iter().map(|p| widgets::Chip::new(&chip_label(p)).icon(chip_icon(p).0).selected(p.profile == next).width(ui)).collect();
    if new.is_some() {
        widths.push(widgets::Chip::new(new_label).icon(icons::PLUS).width(ui));
    }
    let room = ui.available_width() - caption_w;
    let (places, chips_h) = widgets::flow(&widths, room.max(120.0), CHIP_H, CHIP_GAP);
    let running_here = matches!(play, PlayState::Running(_));
    // A VRChat whose profile is not known yet holds every start: said, rather than a Play that only answers no.
    let unresolved = state.data.unresolved > 0;
    // When the chosen profile runs, or a VRChat has not said its profile: one line that says why.
    let hint_h = if running_here || unresolved { 22.0 } else { 0.0 };
    let height = PLAY_H + CHOICE_TOP + chips_h.max(CHIP_H) + hint_h;
    super::super::block(ui, cx, index, height, |ui, rect| {
        let clock = anim::time(ui.ctx());
        let row = Rect::from_min_size(rect.min, vec2(rect.width(), PLAY_H));
        let gap = 12.0;
        let w = (row.width() - gap) * 0.5;
        let vr = Rect::from_min_size(row.min, vec2(w, row.height()));
        let desk = Rect::from_min_size(pos2(vr.right() + gap, row.top()), vec2(w, row.height()));
        // With several profiles, the buttons say whose VRChat they start.
        let whose = (offered.len() > 1).then(|| match offered.iter().find(|p| p.profile == next).and_then(|p| p.account.clone()).filter(|a| !a.is_empty()) {
            Some(account) => orbiters_i18n::tr!("As {account}", account = account),
            None => orbiters_i18n::tr!("On profile {n}", n = next),
        });
        for (r, mode) in [(vr, LaunchMode::Vr), (desk, LaunchMode::Desktop)] {
            if play_button(ui, r, mode, play, true, whose.as_deref()) {
                // The choice stays on the profile just started, so the buttons say it starts, then runs.
                state.chosen_profile = Some(next);
                // Into the place chosen above (Where), or VRChat's own start.
                super::destinations::play(state, cx, clock, mode, Some(next));
            }
        }
        // The profile choice.
        let top = row.bottom() + CHOICE_TOP;
        widgets::text(ui.painter(), pos2(rect.left() + 2.0, top + CHIP_H * 0.5), Align2::LEFT_CENTER, &caption);
        let x0 = rect.left() + caption_w;
        for (i, at) in places.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x0 + at.x, top + at.y), vec2(widths[i], CHIP_H));
            match offered.get(i) {
                Some(p) => {
                    let label = chip_label(p);
                    let (glyph, tone) = chip_icon(p);
                    let chip = widgets::Chip::new(&label).icon(glyph).tone(tone).selected(p.profile == next).id_salt(("launcher-profile", p.profile));
                    let got = chip.show_at(ui, r);
                    if got.pressed {
                        state.chosen_profile = Some(p.profile);
                    }
                    got.response.on_hover_text(chip_hint(p));
                }
                None => {
                    let got = widgets::Chip::new(new_label).icon(icons::PLUS).id_salt("launcher-profile-new").show_at(ui, r);
                    if got.pressed {
                        state.added_profile = new;
                        state.chosen_profile = new;
                    }
                    got.response.on_hover_text(tr("Start VRChat on a profile of its own, to sign in there with another account."));
                }
            }
        }
        if running_here || unresolved {
            let line = if unresolved {
                tr("A VRChat has not said its profile yet: Play waits a minute for it, then asks you before it starts another.").to_string()
            } else {
                orbiters_i18n::tr!("VRChat runs on profile {n}: choose another profile to start one more.", n = next)
            };
            let y = top + chips_h.max(CHIP_H) + 4.0 + hint_h * 0.5;
            widgets::text_truncated(ui.painter(), pos2(rect.left() + 2.0, y), Align2::LEFT_CENTER, &Txt::new(line, 12.5).color(theme::FG_3), rect.width());
        }
    });
    if let Some(invite) = invite {
        // The Play buttons above start a VRChat of the chosen profile; the invite's own buttons go to its world.
        let card_play = match states::play(&state.data) {
            PlayState::Running(mode) => PlayState::Running(mode),
            PlayState::Starting(mode) => PlayState::Starting(mode),
            _ => PlayState::Ready,
        };
        super::super::block(ui, cx, index + 1, INVITE_H, |ui, rect| invite_card(ui, cx, state, rect, &invite, card_play));
    }
}

/// A big Play button: VR in Orbiters green, desktop in grey, an icon, a name and what it uses. While it starts, a
/// turning ring and "Starting…"; the other one waits. Returns whether it was pressed.
fn play_button(ui: &mut Ui, rect: Rect, mode: LaunchMode, play: PlayState, big: bool, whose: Option<&str>) -> bool {
    let ctx = ui.ctx().clone();
    let (label, sub, glyph) = match mode {
        LaunchMode::Vr => (tr("Play in VR"), tr("With your headset, through SteamVR"), icons::VIRTUAL_REALITY),
        LaunchMode::Desktop if !VR_HERE => (tr("Play VRChat"), tr("On this Mac, with Obelisk's engine"), icons::DESKTOP),
        LaunchMode::Desktop => (tr("Play in Desktop"), tr("Keyboard and mouse, no headset"), icons::DESKTOP),
    };
    let starting = play == PlayState::Starting(mode);
    // The big buttons stay while VRChat runs on the chosen profile: the one of its mode says so, neither starts a second.
    let running = big && play == PlayState::Running(mode);
    let enabled = play == PlayState::Ready || (!big && matches!(play, PlayState::Running(m) if m == mode));
    let id = Id::unique(("launcher-play", mode == LaunchMode::Vr, big));
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    widgets::note_control(&ctx, label, rect, enabled);
    let pressed = enabled && widgets::pressed_fresh(&resp);
    let hover = anim::toggle(&ctx, id.with("hover"), enabled && resp.hovered(), Spring::SNAPPY);
    let down = anim::toggle(&ctx, id.with("down"), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.02 * down));
    let mut p = ui.painter().clone();
    if !enabled && !starting && !running {
        p.multiply_opacity(0.4);
    }
    // The one button of a Mac is the green one.
    let primary = mode == LaunchMode::Vr || !VR_HERE;
    let (fill, fg, fg2) = if running {
        (theme::alpha(theme::ACCENT, 0.14), theme::FG, theme::FG_2)
    } else if primary {
        (theme::mix(theme::ACCENT, theme::ACCENT_HOVER, hover), theme::ON_ACCENT, theme::alpha(theme::ON_ACCENT, 0.72))
    } else {
        (theme::mix(theme::control(), theme::INK_600, hover), theme::FG, theme::FG_3)
    };
    let radius = if big { 20.0 } else { 16.0 };
    widgets::frame(&p, r, radius, fill);
    // The icon in a round of its own, nudged up on hover.
    let chip = if big { 46.0 } else { 0.0 };
    let mut x = r.left() + if big { 18.0 } else { 0.0 };
    if big {
        let c = pos2(x + chip * 0.5, r.center().y - 2.0 * hover);
        p.circle_filled(c, chip * 0.5, if primary { theme::black(0.12) } else { theme::white(0.06) });
        if starting {
            let t = anim::clock(&ctx, 60.0) as f32;
            paint::ring(&p, c, 12.0, 2.5, theme::alpha(fg, 0.25));
            paint::arc(&p, c, 12.0, t * 6.0, 1.9, 2.5, fg);
            ctx.request_repaint();
        } else if running {
            widgets::icon(&p, c, glyph, 24.0, theme::ACCENT);
            widgets::status_dot(&p, c + vec2(17.0, 15.0), widgets::Tone::Ok, true);
        } else {
            widgets::icon(&p, c, glyph, 24.0, fg);
        }
        x += chip + 14.0;
        let (title, sub) = match starting {
            true => (tr("Starting VRChat…"), sub),
            false if running => (
                match mode {
                    LaunchMode::Vr => tr("VRChat is running in VR"),
                    LaunchMode::Desktop => tr("VRChat is running on the desktop"),
                },
                tr("On the profile chosen below"),
            ),
            false if play == PlayState::OtherStarting => (label, tr("Wait for the VRChat starting now")),
            false => (label, sub),
        };
        let sub: &str = match (whose, starting, running) {
            (Some(w), false, false) if play != PlayState::OtherStarting => w,
            _ => sub,
        };
        let room = r.right() - x - 14.0;
        // What it uses, when it fits whole; the name alone otherwise.
        let sub = Txt::new(sub, 12.5).color(fg2);
        let fits = widgets::text_size(ui, &sub).x <= room;
        let dy = if fits { -10.0 } else { 0.0 };
        widgets::text_truncated(&p, pos2(x, r.center().y + dy), Align2::LEFT_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD).tracking(-0.01).color(fg), room);
        if fits {
            widgets::text(&p, pos2(x, r.center().y + 13.0), Align2::LEFT_CENTER, &sub);
        }
    } else {
        // In the invite's card: a pill, the icon and the name centred.
        let txt = Txt::new(label, 14.0).weight(weight::SEMIBOLD).color(fg);
        let w = widgets::text_size(ui, &txt).x + 26.0;
        let x0 = r.center().x - w * 0.5;
        if starting {
            let t = anim::clock(&ctx, 60.0) as f32;
            paint::arc(&p, pos2(x0 + 9.0, r.center().y), 7.0, t * 6.0, 1.9, 2.0, fg);
            ctx.request_repaint();
        } else {
            widgets::icon(&p, pos2(x0 + 9.0, r.center().y), glyph, 17.0, fg);
        }
        widgets::text(&p, pos2(x0 + 26.0, r.center().y), Align2::LEFT_CENTER, &txt);
    }
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand);
    }
    pressed
}

/// A received invite: who, the world with its picture, the instance, the message and when; Play in VR and Play in
/// Desktop accept it; × puts it away. While VRChat runs, one Join button sends it there.
fn invite_card(ui: &mut Ui, cx: &Cx, state: &mut State, rect: Rect, invite: &Invite, play: PlayState) {
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let p = ui.painter().clone();
    // A soft green edge says it is for the person, now.
    paint::shadow(&p, rect, 20.0, vec2(0.0, 10.0), 28.0, -8.0, theme::black(0.35));
    widgets::frame(&p, rect, 20.0, theme::surface_strong());
    paint::stroke_squircle(&p, rect, theme::squircle(20.0), egui::Stroke::new(1.0, theme::alpha(theme::ACCENT, 0.35)));
    let pad = 16.0;
    let pic_h = rect.height() - 2.0 * pad;
    let pic_w = (pic_h * 16.0 / 9.0).min(rect.width() * 0.4);
    let pic = Rect::from_min_size(rect.min + vec2(pad, pad), vec2(pic_w, pic_h));
    let place = invite.place.clone();
    let world_picture = place.as_ref().and_then(|pl| state.picture(&pl.world_picture));
    match &world_picture {
        Some(t) => paint::squircle_texture(&p, pic, [14.0; 4], t.id(), cover_uv(t.size_vec2(), pic.size()), egui::Color32::WHITE),
        None => {
            let c = widgets::name_color(place.as_ref().map_or("", |pl| pl.world_name.as_str()));
            paint::fill_convex(&p, &paint::squircle_points(pic, theme::squircle(14.0)), |at| {
                let t = ((at.x - pic.left()) / pic.width() + (at.y - pic.top()) / pic.height()) * 0.5;
                theme::mix(theme::alpha(c, 0.35), theme::INK_800, t)
            });
            widgets::icon(&p, pic.center(), icons::GLOBE_HEMISPHERE_WEST, 30.0, theme::alpha(theme::FG, 0.5));
        }
    }
    // The instance on the picture.
    if let Some(pl) = &place {
        let mut chips = access_name(pl.access).to_string();
        if !pl.region.is_empty() {
            chips = if chips.is_empty() { region_tag(&pl.region) } else { format!("{chips} · {}", region_tag(&pl.region)) };
        }
        if !chips.is_empty() {
            let txt = Txt::new(chips, 11.5).weight(weight::SEMIBOLD).color(theme::FG);
            let w = widgets::text_size(ui, &txt).x + 30.0;
            let chip = Rect::from_min_size(pic.left_bottom() + vec2(8.0, -30.0), vec2(w, 22.0));
            p.rect_filled(chip, theme::radius(11.0), theme::black(0.6));
            widgets::icon(&p, pos2(chip.left() + 12.0, chip.center().y), super::access_icon(pl.access), 12.0, theme::FG);
            widgets::text(&p, pos2(chip.left() + 22.0, chip.center().y), Align2::LEFT_CENTER, &txt);
        }
    }

    let x = pic.right() + 18.0;
    let right = rect.right() - pad;
    let room = right - x - 36.0;
    // Who, and when.
    let from = Rect::from_min_size(pos2(x, rect.top() + pad), vec2(24.0, 24.0));
    let icon = state.picture(&invite.from_icon);
    widgets::avatar(&p, from, icon.as_ref(), &invite.from_name);
    let who = orbiters_i18n::tr!("{name} invited you", name = invite.from_name);
    let w = widgets::text_truncated(&p, pos2(from.right() + 8.0, from.center().y), Align2::LEFT_CENTER, &Txt::new(who, 13.0).weight(weight::MEDIUM).color(theme::FG_2), room - 70.0);
    let when = super::super::ago(&ctx, invite.received_at.max(0) as u64, state.now().max(0) as u64);
    widgets::text(&p, pos2(w.right() + 8.0, from.center().y), Align2::LEFT_CENTER, &Txt::new(format!("· {when}"), 12.0).color(theme::FG_4));
    let close = Rect::from_center_size(pos2(right - 12.0, from.center().y), vec2(30.0, 30.0));
    if widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Dismiss")).size(30.0, 14.0).id_salt(("launcher-invite-x", &invite.id)).show_at(ui, close)) {
        state.act(cx, clock, Action::Dismiss { id: invite.id.clone() });
    }
    // The world.
    let world = place.as_ref().map(|pl| pl.world_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| tr("A world").to_string());
    widgets::text_truncated(&p, pos2(x, rect.top() + pad + 46.0), Align2::LEFT_CENTER, &Txt::new(world, 22.0).weight(weight::SEMIBOLD).tracking(-0.015), right - x);
    if !invite.message.trim().is_empty() {
        let quote = format!("“{}”", invite.message.trim());
        widgets::text_truncated(&p, pos2(x, rect.top() + pad + 76.0), Align2::LEFT_CENTER, &Txt::new(quote, 13.5).color(theme::FG_2), right - x);
    }
    // Both Play buttons, or Join while VRChat runs.
    let bh = 44.0;
    let by = rect.bottom() - pad - bh;
    match play {
        PlayState::Running(mode) => {
            let b = widgets::Button::new(tr("Join now")).icon(icons::ARROW_RIGHT).variant(widgets::Variant::Primary).size(widgets::Size::Lg).id_salt("launcher-invite-join");
            let bw = b.desired_width(ui).max(160.0);
            if b.show_at(ui, Rect::from_min_size(pos2(x, by), vec2(bw, bh))).clicked() {
                state.act(cx, clock, Action::AcceptInvite { invite: invite.id.clone(), mode });
            }
        }
        _ => {
            let gap = 10.0;
            let bw = ((right - x - gap) * 0.5).min(220.0);
            if !VR_HERE {
                if play_button(ui, Rect::from_min_size(pos2(x, by), vec2(bw, bh)), LaunchMode::Desktop, play, false, None) {
                    state.act(cx, clock, Action::AcceptInvite { invite: invite.id.clone(), mode: LaunchMode::Desktop });
                }
            } else {
            let vr = Rect::from_min_size(pos2(x, by), vec2(bw, bh));
            let desk = Rect::from_min_size(pos2(vr.right() + gap, by), vec2(bw, bh));
            if play_button(ui, vr, LaunchMode::Vr, play, false, None) {
                state.act(cx, clock, Action::AcceptInvite { invite: invite.id.clone(), mode: LaunchMode::Vr });
            }
            if play_button(ui, desk, LaunchMode::Desktop, play, false, None) {
                state.act(cx, clock, Action::AcceptInvite { invite: invite.id.clone(), mode: LaunchMode::Desktop });
            }
            }
        }
    }
    let more = state.data.invites.iter().filter(|i| i.kind == InviteKind::Invite && !state.dismissed.contains(&i.id)).count().saturating_sub(1);
    if more > 0 {
        let t = orbiters_i18n::tr!("{n} more invites", n = more);
        widgets::text(&p, pos2(right, by + bh * 0.5), Align2::RIGHT_CENTER, &Txt::new(t, 12.0).color(theme::FG_4));
    }
}

/// The Steam sign-in of a Mac's set-up: the address Steam gave as a QR code, and what to do with it.
fn steam_sign_in(ui: &mut Ui, rect: Rect, qr_url: &str) {
    let p = ui.painter().clone();
    widgets::frame(&p, rect, 16.0, theme::surface_soft());
    let side = rect.height() - 32.0;
    let card = Rect::from_min_size(pos2(rect.left() + 16.0, rect.top() + 16.0), vec2(side, side));
    p.rect_filled(card, theme::radius(12.0), egui::Color32::WHITE);
    // The code's modules, kept from one frame to the next for the same address.
    let id = Id::unique("launcher-steam-qr");
    let kept: Option<(String, usize, std::sync::Arc<Vec<bool>>)> = ui.ctx().data(|d| d.get_temp(id));
    let (n, dark) = match kept {
        Some((url, n, dark)) if url == qr_url => (n, dark),
        _ => {
            let (n, dark) = match qrcode::QrCode::new(qr_url.as_bytes()) {
                Ok(code) => (code.width(), code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()),
                Err(_) => (0, Vec::new()),
            };
            let dark = std::sync::Arc::new(dark);
            ui.ctx().data_mut(|d| d.insert_temp(id, (qr_url.to_string(), n, dark.clone())));
            (n, dark)
        }
    };
    if n > 0 {
        let inner = card.shrink(10.0);
        let module = inner.width() / n as f32;
        for (i, _) in dark.iter().enumerate().filter(|(_, dark)| **dark) {
            let (x, y) = ((i % n) as f32, (i / n) as f32);
            // A hair wider than a module, so that no light line shows between two dark ones.
            p.rect_filled(Rect::from_min_size(pos2(inner.left() + x * module, inner.top() + y * module), vec2(module + 0.35, module + 0.35)), 0.0, egui::Color32::BLACK);
        }
    }
    let x = card.right() + 20.0;
    let room = rect.right() - 16.0 - x;
    widgets::text_truncated(&p, pos2(x, rect.top() + 40.0), Align2::LEFT_CENTER, &Txt::new(tr("Sign in to Steam"), 18.0).weight(weight::SEMIBOLD), room);
    let lines = [
        tr("VRChat is downloaded with your Steam account."),
        tr("1. Open the Steam app on your phone."),
        tr("2. Scan this code with it and approve the sign-in."),
        tr("The download starts by itself after that."),
    ];
    for (i, line) in lines.into_iter().enumerate() {
        widgets::text_truncated(&p, pos2(x, rect.top() + 72.0 + 24.0 * i as f32), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(if i == 0 || i == 3 { theme::FG_3 } else { theme::FG_2 }), room);
    }
    orbiters_ui::feedback::spinner(&p, pos2(x + 8.0, rect.bottom() - 26.0), 7.0, 2.0, theme::ACCENT);
    widgets::text_truncated(&p, pos2(x + 24.0, rect.bottom() - 26.0), Align2::LEFT_CENTER, &Txt::new(tr("Waiting for your phone…"), 12.5).color(theme::FG_3), room - 24.0);
}
