//! Home's States page: N4N0's saved states (controller/states.rs), a card per avatar (its name as set in its SDK, which
//! VRChat's OSCQuery gives, how many states it keeps and its default), the avatar worn first. A card opens that
//! avatar's states: save how the avatar worn is now under a name; for each saved state, apply it (the avatar worn's
//! only), make it the default (put back after each world join and each switch to the avatar), rename or delete it, and
//! choose whether it puts the GoGoLoco pose and the avatar's size back too.
//!
//! What a press asks shows at once: the page keeps it over the service's list until the list says the same (or a few
//! seconds passed, when the service refused it and says why). Switches and the default act on the pointer's press,
//! buttons on its release with the press shown at once, as on the rest of Home.

use super::{block, section, Cx};
use crate::overlay::menu_ui;
use crate::state::{AvatarView, StateAction, StateView, StatesView};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Key, Rect, Sense, Ui};
use orbiters_i18n::{fill, tr, tr_n};
use orbiters_n4n0_core::menu::layout::Tint;
use orbiters_n4n0_core::menu::{next_state_name, StateRow};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

/// How long what was asked here shows over the service's list before the list has the last word.
const PENDING_FOR: f64 = 4.0;
/// A first press on Delete waits this long for the second.
const CONFIRM_FOR: f64 = 3.0;
/// The avatars' cards: their height, the room between them, their insets (Home's cards').
const CARD_H: f32 = 128.0;
const GAP: f32 = 16.0;
const INSET: f32 = 24.0;
/// The cards' width at most, as Home's.
const MAX_W: f32 = 980.0;

/// What the page asks of Home.
pub enum Out {
    /// Back to Home itself.
    Home,
    /// Asked of the service, for another avatar's states than the worn one's (`Some` of its id).
    Action(StateAction, Option<String>),
}

/// Whose states an action is for: the avatar's id, and whether it is the avatar worn.
#[derive(Clone)]
struct Whose {
    id: String,
    worn: bool,
}

#[derive(Default)]
pub struct StatesPage {
    /// The avatar whose states show (`None`: the avatars' cards).
    open: Option<String>,
    /// The name typed for the next save.
    name: String,
    /// The state being renamed, and the name being typed (focused once).
    renaming: Option<(String, String, bool)>,
    /// What was asked and the service's list does not say yet: for which avatar, since when.
    pending: Vec<(String, StateAction, f64)>,
    /// The state whose Delete was pressed once, and when (a second press deletes).
    confirm: Option<(String, f64)>,
    /// The service's last note shown as a toast.
    note_seen: Option<u64>,
}

impl StatesPage {
    /// Back to the avatars' cards (the page is entered anew).
    pub fn show_cards(&mut self) {
        self.open = None;
        self.renaming = None;
        self.confirm = None;
    }

    /// What was asked and still waits for the service's list (tests).
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// Something asked of avatar `id` at `now` that the service never answers (the count of frames at rest).
    pub fn stage_asked(&mut self, id: &str, now: f64) {
        self.pending.push((id.to_string(), StateAction::Default { id: None }, now));
    }

    /// Opens avatar `id`'s states (the screenshot review).
    pub fn open_avatar(&mut self, id: &str) {
        self.show_cards();
        self.open = Some(id.to_string());
    }

    /// What `states` (avatar `id`'s, as the service lists them) are once what was asked here for that avatar is applied
    /// over them (and what the list already says is dropped).
    fn shown(&mut self, id: &str, states: &[StateView], now: f64) -> Vec<StateView> {
        self.pending.retain(|(avatar, action, at)| now - at < PENDING_FOR && (avatar != id || !done(action, states)));
        let mut states = states.to_vec();
        for (_, action, _) in self.pending.iter().filter(|(avatar, _, _)| avatar == id) {
            match action {
                StateAction::Save { name, .. } => {
                    let rows: Vec<StateRow> = states.iter().map(|s| StateRow { name: s.name.clone(), ..Default::default() }).collect();
                    let name = name.clone().unwrap_or_else(|| next_state_name(&rows));
                    states.push(StateView { name, ..Default::default() });
                }
                StateAction::Default { id } => {
                    for s in &mut states {
                        s.default = id.as_deref() == Some(s.id.as_str());
                    }
                }
                StateAction::Rename { id, name } => {
                    if let Some(s) = states.iter_mut().find(|s| s.id == *id) {
                        s.name = name.clone();
                    }
                }
                StateAction::Delete { id } => states.retain(|s| s.id != *id),
                StateAction::Options { id, pose, size } => {
                    if let Some(s) = states.iter_mut().find(|s| s.id == *id) {
                        (s.pose, s.size) = (*pose, *size);
                    }
                }
                StateAction::Apply { .. } => {}
            }
        }
        states
    }

    /// Something asked of `whose` states: kept until the service's list says it, and sent.
    fn ask(&mut self, whose: &Whose, action: StateAction, now: f64, out: &mut Vec<Out>) {
        if !matches!(action, StateAction::Apply { .. }) {
            self.pending.push((whose.id.clone(), action.clone(), now));
        }
        out.push(Out::Action(action, (!whose.worn).then(|| whose.id.clone())));
    }
}

/// Whether the service's list says what `action` asked.
fn done(action: &StateAction, states: &[StateView]) -> bool {
    let find = |id: &str| states.iter().find(|s| s.id == id);
    match action {
        StateAction::Save { name, .. } => name.as_ref().is_some_and(|n| states.iter().any(|s| s.name == *n)),
        StateAction::Default { id: Some(id) } => find(id).is_some_and(|s| s.default),
        StateAction::Default { id: None } => states.iter().all(|s| !s.default),
        StateAction::Rename { id, name } => find(id).is_none_or(|s| s.name == *name),
        StateAction::Delete { id } => find(id).is_none(),
        StateAction::Options { id, pose, size } => find(id).is_none_or(|s| s.pose == *pose && s.size == *size),
        StateAction::Apply { .. } => true,
    }
}

/// An avatar's name, or what stands for it when VRChat never gave one (an avatar saved before names were kept).
fn avatar_name(avatar: &AvatarView) -> String {
    match avatar.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => name.to_string(),
        None if avatar.worn => tr("This avatar").to_string(),
        None => tr!("Avatar {id}", id = short_id(&avatar.id)),
    }
}

/// An avatar's id shortened for a title: `avtr_1a2b3c4d…`.
fn short_id(id: &str) -> String {
    let short: String = id.chars().take(13).collect();
    if short.len() < id.len() {
        format!("{short}…")
    } else {
        short
    }
}

/// "3 saved states", "1 saved state", "Nothing saved yet".
fn count(n: usize) -> String {
    match n {
        0 => tr("Nothing saved yet").into(),
        n => tr_n("{count} saved state", "{count} saved states", n as u64),
    }
}

/// [`count`] after a "·" in the middle of a line: "3 saved states", "nothing saved yet".
fn count_inline(n: usize) -> String {
    match n {
        0 => tr("nothing saved yet").into(),
        n => count(n),
    }
}

/// The page: the avatars' cards, or one avatar's states.
pub fn show(ui: &mut Ui, cx: &mut Cx, page: &mut StatesPage, out: &mut Vec<Out>) {
    let now = anim::time(ui.ctx());
    let view = cx.snap.n4n0.states.clone();
    // What the service said of the last save, apply or change: a toast, once (what went wrong comes as the service's own
    // toast).
    if let Some(note) = &view.note {
        if page.note_seen.is_some_and(|seen| seen != note.id) && !note.error {
            cx.toasts.push(ui.ctx(), orbiters_ui::toast::Kind::Success, note.text.clone());
        }
        page.note_seen = Some(note.id);
    } else {
        page.note_seen.get_or_insert(0);
    }
    if page.confirm.as_ref().is_some_and(|(_, at)| now - at >= CONFIRM_FOR) {
        page.confirm = None;
    }
    // What was asked shows until the service's list says it, or its time is up: a frame then, should the list say
    // nothing. Its time is counted here, whichever page shows: an avatar's page also takes out what its list says, but
    // the cards never look at what was asked, and one left there with its time up (Back before the service answered, an
    // avatar that went) asked for a frame at every frame for as long as the cards showed.
    page.pending.retain(|(_, _, at)| now - at < PENDING_FOR);
    if let Some(oldest) = page.pending.iter().map(|(_, _, at)| *at).min_by(f64::total_cmp) {
        super::frame_in(ui.ctx(), PENDING_FOR - (now - oldest));
    }
    // An avatar whose card went (its last state deleted, no longer worn) shows the cards again.
    let opened = page.open.as_ref().and_then(|id| view.avatars.iter().find(|a| a.id == *id)).cloned();
    match opened {
        Some(avatar) => avatar_page(ui, cx, page, &view, &avatar, now, out),
        None => {
            page.open = None;
            cards_page(ui, cx, page, &view, out);
        }
    }
}

/// The avatars' cards: the avatar worn first, then the others, the one saved in last first.
fn cards_page(ui: &mut Ui, cx: &mut Cx, page: &mut StatesPage, view: &StatesView, out: &mut Vec<Out>) {
    let total: usize = view.avatars.iter().map(|a| a.states.len()).sum();
    let avatars = view.avatars.len();
    let sub = match (view.avatar_id.is_some(), avatars) {
        (false, 0) => tr("VRChat's avatar is not known yet").to_string(),
        (true, 1) => tr!("Your avatar · {saved}", saved = count_inline(total)),
        (_, n) => fill(&tr_n("{count} avatar · {saved}", "{count} avatars · {saved}", n as u64), &[("saved", &count_inline(total))]),
    };
    block(ui, cx, 0, 116.0, |ui, rect| {
        let back = Button::new(tr("Home")).icon(icons::ARROW_LEFT).variant(Variant::Ghost).size(Size::Sm);
        let w = back.desired_width(ui);
        if back.show_at(ui, Rect::from_min_size(rect.min, vec2(w, 32.0))).clicked() {
            out.push(Out::Home);
        }
        super::heading(ui, Rect::from_min_max(pos2(rect.left(), rect.top() + 44.0), rect.max), tr("N4N0 menu"), tr("States"), Some(&sub));
    });
    if view.avatars.is_empty() {
        let text = tr("Start VRChat with OSC on: the avatar you wear gets a card here, named as in its SDK. A state keeps every parameter of it that OSC can set, and its size; applying one puts them back.");
        return note(ui, cx, 1, icons::BOOKMARKS_SIMPLE, tr("No avatar yet"), text);
    }
    let w = ui.available_width().min(MAX_W);
    let cols = if w >= 900.0 {
        3
    } else if w >= 560.0 {
        2
    } else {
        1
    };
    let card_w = (w - GAP * (cols - 1) as f32) / cols as f32;
    let rows = view.avatars.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (CARD_H + GAP) - GAP), Sense::hover());
    for (i, avatar) in view.avatars.iter().enumerate() {
        let at = area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (CARD_H + GAP));
        if avatar_card(ui, cx, 1 + i, Rect::from_min_size(at, vec2(card_w, CARD_H)), avatar) {
            page.open_avatar(&avatar.id);
        }
    }
}

/// An avatar's card: its chip, its name, how many states it keeps and its default, "Wearing now" on the worn one, the
/// way in. Whether it was pressed.
fn avatar_card(ui: &mut Ui, cx: &Cx, index: usize, rect: Rect, avatar: &AvatarView) -> bool {
    let (rect, alpha) = super::entering(ui, cx, index, rect);
    let ctx = ui.ctx().clone();
    let id = Id::unique(("orbiters-states-avatar", &avatar.id));
    let resp = ui.interact(rect, id, Sense::click());
    widgets::note_control(&ctx, &avatar_name(avatar), rect, true);
    let pressed = widgets::pressed(&resp);
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    resp.on_hover_cursor(CursorIcon::PointingHand);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.025 * press));
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover));
    // The chip in the States tile's colour; the avatar worn's in the accent.
    let tint = if avatar.worn { theme::ACCENT } else { menu_ui::tint(Tint::Pink) };
    let chip = Rect::from_min_size(r.min + vec2(INSET, 22.0), vec2(36.0, 36.0));
    p.rect_filled(chip, theme::radius(11.0), theme::alpha(tint, 0.12));
    widgets::icon(&p, chip.center(), if avatar.worn { icons::PERSON } else { icons::USER_CIRCLE }, 17.0, tint);
    if avatar.worn {
        let pill = Txt::new(tr("Wearing now"), 11.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
        let pw = widgets::text_size(ui, &pill).x + 20.0;
        let pr = Rect::from_min_size(pos2(r.right() - INSET - pw, chip.center().y - 10.0), vec2(pw, 20.0));
        p.rect_filled(pr, theme::radius(10.0), theme::alpha(theme::ACCENT, 0.14));
        widgets::text(&p, pr.center(), Align2::CENTER_CENTER, &pill);
    } else {
        widgets::icon(&p, pos2(r.right() - INSET - 8.0, chip.center().y), icons::CARET_RIGHT, 16.0, theme::mix(theme::FG_4, theme::FG_2, hover));
    }
    let x = r.left() + INSET;
    let room = r.width() - 2.0 * INSET;
    let name = Txt::new(avatar_name(avatar), 15.5).weight(weight::SEMIBOLD);
    widgets::text_truncated(&p, pos2(x, r.bottom() - 46.0), Align2::LEFT_CENTER, &name, room);
    let default = avatar.states.iter().find(|s| s.default).map(|s| s.name.as_str());
    let saved = count(avatar.states.len());
    let line = match default {
        Some(name) => tr!("{saved} · Default: {name}", saved = saved, name = name),
        None => saved,
    };
    let note = Txt::new(line, 12.5).color(if default.is_none() { theme::FG_3 } else { theme::ACCENT });
    widgets::text_truncated(&p, pos2(x, r.bottom() - 24.0), Align2::LEFT_CENTER, &note, room);
    pressed
}

/// One avatar's states: saving (the avatar worn), then its states, the newest first.
fn avatar_page(ui: &mut Ui, cx: &mut Cx, page: &mut StatesPage, view: &StatesView, avatar: &AvatarView, now: f64, out: &mut Vec<Out>) {
    let whose = Whose { id: avatar.id.clone(), worn: avatar.worn };
    let states = page.shown(&avatar.id, &avatar.states, now);
    let sub = if avatar.worn {
        tr!("Wearing now · {saved}", saved = count_inline(states.len()))
    } else {
        tr!("{saved} · wear it in VRChat to apply them", saved = count(states.len()))
    };
    let mut back_to_cards = false;
    block(ui, cx, 0, 116.0, |ui, rect| {
        let back = Button::new(tr("States")).icon(icons::ARROW_LEFT).variant(Variant::Ghost).size(Size::Sm).id_salt("orbiters-states-cards");
        let w = back.desired_width(ui);
        back_to_cards = back.show_at(ui, Rect::from_min_size(rect.min, vec2(w, 32.0))).clicked();
        super::heading(ui, Rect::from_min_max(pos2(rect.left(), rect.top() + 44.0), rect.max), tr("States"), &avatar_name(avatar), Some(&sub));
    });
    if back_to_cards {
        page.show_cards();
        return;
    }
    let mut index = 1;
    if avatar.worn {
        save_card(ui, cx, page, &whose, &states, view.saving, now, out);
        index += 1;
    }
    if states.is_empty() {
        let text = tr("Save keeps how the avatar is now: its toggles, sliders and outfits. Make one the default and it is put back after each world join, and each time you switch to this avatar.");
        return note(ui, cx, index, icons::BOOKMARKS_SIMPLE, tr("Nothing saved for this avatar"), text);
    }
    // The newest first.
    for (i, state) in states.iter().enumerate().rev() {
        state_card(ui, cx, index + states.len() - 1 - i, page, &whose, state, now, out);
    }
}

/// A card that only explains.
fn note(ui: &mut Ui, cx: &Cx, index: usize, glyph: &str, title: &str, text: &str) {
    section(ui, cx, index, glyph, title, None, |ui| {
        let w = ui.available_width() - 48.0;
        let lines = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::FG_3).line_height(19.0).job(w));
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), lines.size().y + 8.0), Sense::hover());
        ui.painter().galley(pos2(r.left() + 48.0, r.top()), lines, theme::FG_3);
    });
}

/// Saving how the avatar worn is now: a name (or the next "State n"), and Save.
#[allow(clippy::too_many_arguments)]
fn save_card(ui: &mut Ui, cx: &Cx, page: &mut StatesPage, whose: &Whose, states: &[StateView], saving: bool, now: f64, out: &mut Vec<Out>) {
    let rows: Vec<StateRow> = states.iter().map(|s| StateRow { name: s.name.clone(), ..Default::default() }).collect();
    let placeholder = next_state_name(&rows);
    let sub = tr("Every parameter OSC can set, GoGoLoco's pose and the avatar's size (N4N0's own stay out)");
    section(ui, cx, 1, icons::FLOPPY_DISK, tr("Save how it is now"), Some(sub), |ui| {
        ui.add_space(14.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let button_w = 112.0;
        let field = Rect::from_min_size(row.min, vec2(row.width() - button_w - 12.0, 36.0)).translate(vec2(0.0, 2.0));
        let mut fu = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let resp = widgets::text_field(&mut fu, "orbiters-states-name", &mut page.name, &placeholder, field.width(), false);
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let label = if saving { tr("Saving…") } else { tr("Save") };
        let save = Button::new(label).icon(icons::FLOPPY_DISK).variant(Variant::Primary).width(button_w).id_salt("orbiters-states-save");
        let clicked = save.show_at(ui, Rect::from_min_size(pos2(row.right() - button_w, row.top()), vec2(button_w, 40.0))).clicked();
        if clicked || enter {
            let name = page.name.trim().to_string();
            let name = if name.is_empty() { placeholder.clone() } else { name };
            page.name.clear();
            page.ask(whose, StateAction::Save { name: Some(name), size: true, pose: false, default: false }, now, out);
        }
        ui.add_space(8.0);
    });
}

/// One saved state: what it puts back, its default, and its buttons.
#[allow(clippy::too_many_arguments)]
fn state_card(ui: &mut Ui, cx: &Cx, index: usize, page: &mut StatesPage, whose: &Whose, state: &StateView, now: f64, out: &mut Vec<Out>) {
    let saving = state.id.is_empty();
    let when = if saving { tr("Saving…").to_string() } else { super::local_time(&state.saved_utc) };
    let others = state.parameters.saturating_sub(state.pose_parameters);
    let sub = if saving {
        when
    } else {
        fill(&tr_n("Saved {when} · {count} parameter", "Saved {when} · {count} parameters", others as u64), &[("when", &when)])
    };
    let glyph = if state.default { icons::STAR } else { icons::BOOKMARKS_SIMPLE };
    let renaming = page.renaming.as_ref().is_some_and(|(id, _, _)| *id == state.id && !saving);
    section(ui, cx, index, glyph, &state.name, Some(&sub), |ui| {
        // The default, plainly: a pill beside the name.
        if state.default {
            let header = Rect::from_min_size(ui.min_rect().min, vec2(ui.available_width(), 40.0));
            let x = header.left() + 48.0 + widgets::text_size(ui, &Txt::new(&state.name, 15.5).weight(weight::SEMIBOLD)).x + 10.0;
            default_pill(ui, pos2(x, header.center().y - 18.0));
        }
        let id = state.id.clone();
        let pose_note = if state.pose_parameters == 0 {
            tr("No GoGoLoco pose was saved with it").to_string()
        } else {
            tr!("Puts GoGoLoco's pose back too ({count} of its parameters)", count = state.pose_parameters)
        };
        widgets::field(ui, tr("Include GoGoLoco pose"), Some(&pose_note), 48.0, |ui| {
            if saving || state.pose_parameters == 0 {
                ui.disable();
            }
            let mut on = state.pose;
            if widgets::toggle(ui, ("orbiters-state-pose", &id), &mut on) {
                page.ask(whose, StateAction::Options { id: id.clone(), pose: on, size: state.size }, now, out);
            }
        });
        widgets::separator(ui);
        let size_note = match state.eye_height {
            Some(h) => tr!("Puts your size back too: eyes at {height} m, where the world allows it", height = format!("{h:.2}")),
            None => tr("VRChat did not say the avatar's size when it was saved").to_string(),
        };
        widgets::field(ui, tr("Include avatar size"), Some(&size_note), 48.0, |ui| {
            if saving || state.eye_height.is_none() {
                ui.disable();
            }
            let mut on = state.size;
            if widgets::toggle(ui, ("orbiters-state-size", &id), &mut on) {
                page.ask(whose, StateAction::Options { id: id.clone(), pose: state.pose, size: on }, now, out);
            }
        });
        widgets::separator(ui);
        ui.add_space(14.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        if renaming {
            rename_row(ui, row, page, whose, &id, now, out);
        } else {
            buttons_row(ui, row, page, whose, state, saving, now, out);
        }
        ui.add_space(8.0);
    });
}

/// "Default", in the accent, at `left_top`.
fn default_pill(ui: &Ui, left_top: egui::Pos2) {
    let txt = Txt::new(tr("Default"), 11.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
    let w = widgets::text_size(ui, &txt).x + 14.0 + 16.0;
    let r = Rect::from_min_size(left_top, vec2(w, 18.0));
    let p = ui.painter();
    p.rect_filled(r, theme::radius(9.0), theme::alpha(theme::ACCENT, 0.14));
    widgets::icon(p, pos2(r.left() + 13.0, r.center().y), icons::STAR, 11.0, theme::ACCENT);
    widgets::text(p, pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &txt);
}

/// Apply (the avatar worn's states only), the default, Rename and Delete.
#[allow(clippy::too_many_arguments)]
fn buttons_row(ui: &mut Ui, row: Rect, page: &mut StatesPage, whose: &Whose, state: &StateView, saving: bool, now: f64, out: &mut Vec<Out>) {
    let id = state.id.clone();
    let mut x = row.left();
    let apply = Button::new(tr("Apply")).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Primary).enabled(!saving && whose.worn);
    let mut apply = apply.id_salt(("orbiters-state-apply", &id));
    if !whose.worn {
        apply = apply.hint(tr("Wear this avatar in VRChat to apply its states"));
    }
    let w = apply.desired_width(ui).max(104.0);
    if apply.width(w).show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0))).clicked() {
        page.ask(whose, StateAction::Apply { id: id.clone() }, now, out);
    }
    x += w + 8.0;
    // The default is a choice: it acts on the press, as the switches do.
    let (label, hint) = if state.default {
        (tr("Default"), tr("Put back after each world join and each switch to this avatar. Press to stop."))
    } else {
        (tr("Set as default"), tr("Put back after each world join and each switch to this avatar"))
    };
    let default = Button::new(label).icon(icons::STAR).variant(if state.default { Variant::Glass } else { Variant::Secondary });
    let default = default.enabled(!saving).hint(hint).id_salt(("orbiters-state-default", &id));
    let w = default.desired_width(ui);
    let rect = Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0));
    let resp = default.show_at(ui, rect);
    if !saving && widgets::pressed(&resp) {
        page.ask(whose, StateAction::Default { id: (!state.default).then(|| id.clone()) }, now, out);
    }
    if state.default {
        ui.painter().rect_stroke(rect, theme::radius(20.0), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.55)), egui::StrokeKind::Inside);
    }
    // At the right: Rename, then Delete (twice: the first press asks).
    let confirming = page.confirm.as_ref().is_some_and(|(c, _)| *c == id);
    let delete = Button::new(if confirming { tr("Delete it?") } else { tr("Delete") }).icon(icons::TRASH).variant(Variant::Danger).enabled(!saving);
    let delete = delete.id_salt(("orbiters-state-delete", &id));
    let dw = delete.desired_width(ui);
    if delete.confirm_at(ui, Rect::from_min_size(pos2(row.right() - dw, row.top()), vec2(dw, 40.0))) {
        if confirming {
            page.confirm = None;
            page.ask(whose, StateAction::Delete { id: id.clone() }, now, out);
        } else {
            page.confirm = Some((id.clone(), now));
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(CONFIRM_FOR));
        }
    }
    let rename = Button::new(tr("Rename")).icon(icons::PENCIL_SIMPLE).variant(Variant::Ghost).enabled(!saving).id_salt(("orbiters-state-rename", &id));
    let rw = rename.desired_width(ui);
    if rename.show_at(ui, Rect::from_min_size(pos2(row.right() - dw - 8.0 - rw, row.top()), vec2(rw, 40.0))).clicked() {
        page.renaming = Some((id, state.name.clone(), true));
    }
}

/// Renaming: the name in a field, Save and Cancel (Enter saves, Escape cancels).
#[allow(clippy::too_many_arguments)]
fn rename_row(ui: &mut Ui, row: Rect, page: &mut StatesPage, whose: &Whose, id: &str, now: f64, out: &mut Vec<Out>) {
    let Some((_, name, focus)) = page.renaming.as_mut() else { return };
    let buttons_w = 96.0 + 8.0 + 96.0;
    let field = Rect::from_min_size(row.min + vec2(0.0, 2.0), vec2((row.width() - buttons_w - 12.0).min(420.0), 36.0));
    let mut fu = ui.new_child(egui::UiBuilder::new().max_rect(field));
    let resp = widgets::text_field(&mut fu, ("orbiters-state-rename-field", id), name, tr("Name"), field.width(), false);
    if std::mem::take(focus) {
        resp.request_focus();
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    // Taken: this press cancels the rename and nothing else (Home's page would go back to Home on it too).
    let escape = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
    let x = field.right() + 12.0;
    let save = Button::new(tr("Save")).variant(Variant::Primary).width(96.0).id_salt(("orbiters-state-rename-save", id));
    let saved = save.show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(96.0, 40.0))).clicked();
    let cancel = Button::new(tr("Cancel")).variant(Variant::Ghost).width(96.0).id_salt(("orbiters-state-rename-cancel", id));
    let cancelled = cancel.show_at(ui, Rect::from_min_size(pos2(x + 104.0, row.top()), vec2(96.0, 40.0))).clicked();
    if saved || enter {
        let name = name.trim().to_string();
        page.renaming = None;
        if !name.is_empty() {
            page.ask(whose, StateAction::Rename { id: id.to_string(), name }, now, out);
        }
    } else if cancelled || escape {
        page.renaming = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(states: &[(&str, &str, bool)]) -> Vec<StateView> {
        states.iter().map(|(id, name, default)| StateView { id: id.to_string(), name: name.to_string(), default: *default, ..Default::default() }).collect()
    }

    /// Verify3-r21 B7: Enter held on a state's Delete opens "Delete it?" and no more; let go and pressed again, it deletes.
    #[test]
    fn a_held_enter_on_delete_never_confirms_it() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut page = StatesPage::default();
        let whose = Whose { id: "avtr_a".into(), worn: true };
        let state = states(&[("s1", "Hoodie on", false)]).remove(0);
        let mut said = Vec::new();
        let mut frame = |page: &mut StatesPage, said: &mut Vec<Out>, events: Vec<egui::Event>, focus: bool| {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 200.0))), events, ..Default::default() }, |ui| {
                buttons_row(ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 40.0)), page, &whose, &state, false, 0.0, said);
                if focus {
                    let id = ui.scope_id().with("button").with_salt(egui::IdSalt::new(("orbiters-state-delete", "s1".to_string())));
                    ui.memory_mut(|m| m.request_focus(id));
                }
            });
            out.textures_delta.clear();
        };
        frame(&mut page, &mut said, Vec::new(), true);
        frame(&mut page, &mut said, Vec::new(), true);
        let enter = |pressed| egui::Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        for _ in 0..10 {
            frame(&mut page, &mut said, vec![enter(true)], false);
        }
        assert!(page.confirm.is_some(), "Delete asks");
        assert!(said.is_empty(), "one held Enter deleted the state");
        frame(&mut page, &mut said, vec![enter(false)], false);
        frame(&mut page, &mut said, Vec::new(), false);
        frame(&mut page, &mut said, vec![enter(true)], false);
        assert_eq!(said.len(), 1, "a press of its own does");
        assert!(page.confirm.is_none());
    }

    /// Night PAGES-UI-9: Escape while renaming cancels the rename and is taken: Home's page, which asks after it, does
    /// not go back to Home on the same press.
    #[test]
    fn escape_while_renaming_cancels_the_rename_only() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut page = StatesPage { renaming: Some(("st_1".into(), "Old".into(), false)), ..Default::default() };
        let whose = Whose { id: "avtr_a".into(), worn: true };
        let mut left = None;
        let escape = egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let mut out = ctx.run_ui(egui::RawInput { events: vec![escape], ..Default::default() }, |ui| {
            let mut said = Vec::new();
            rename_row(ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 40.0)), &mut page, &whose, "st_1", 0.0, &mut said);
            left = Some(ui.input(|i| i.key_pressed(Key::Escape)));
        });
        out.textures_delta.clear();
        assert!(page.renaming.is_none(), "the rename is cancelled");
        assert_eq!(left, Some(false), "and the press is not seen again after it");
    }

    #[test]
    fn what_was_asked_shows_at_once_until_the_service_says_it() {
        let mut page = StatesPage::default();
        let mut out = Vec::new();
        let worn = Whose { id: "avtr_a".into(), worn: true };
        let before = states(&[("s1", "Hoodie on", false), ("s2", "Cosy", true)]);
        page.ask(&worn, StateAction::Default { id: Some("s1".into()) }, 0.0, &mut out);
        page.ask(&worn, StateAction::Rename { id: "s2".into(), name: "Cosy evening".into() }, 0.0, &mut out);
        page.ask(&worn, StateAction::Save { name: Some("State 3".into()), size: true, pose: false, default: false }, 0.0, &mut out);
        page.ask(&worn, StateAction::Apply { id: "s1".into() }, 0.0, &mut out);
        assert_eq!(out.len(), 4, "all of it sent");
        assert!(out.iter().all(|o| matches!(o, Out::Action(_, None))), "the avatar worn's: no avatar named");
        let shown = page.shown("avtr_a", &before, 0.5);
        let rows: Vec<(&str, &str, bool)> = shown.iter().map(|s| (s.id.as_str(), s.name.as_str(), s.default)).collect();
        assert_eq!(rows, vec![("s1", "Hoodie on", true), ("s2", "Cosy evening", false), ("", "State 3", false)], "the service has not answered yet");
        // Another avatar's list is not this one's.
        assert_eq!(page.shown("avtr_b", &before, 0.5).len(), 2);
        // The service says the default and the name: those are its own now; the save is not listed yet.
        let after = states(&[("s1", "Hoodie on", true), ("s2", "Cosy evening", false)]);
        assert_eq!(page.shown("avtr_a", &after, 1.0).len(), 3);
        assert_eq!(page.pending.len(), 1, "only the save waits");
        let saved = states(&[("s1", "Hoodie on", true), ("s2", "Cosy evening", false), ("s3", "State 3", false)]);
        assert_eq!(page.shown("avtr_a", &saved, 1.5).len(), 3);
        assert!(page.pending.is_empty());
        // A delete the service refused shows again once the wait is over.
        page.ask(&worn, StateAction::Delete { id: "s3".into() }, 2.0, &mut out);
        assert_eq!(page.shown("avtr_a", &saved, 2.1).len(), 2);
        assert_eq!(page.shown("avtr_a", &saved, 2.0 + PENDING_FOR).len(), 3, "the service's list has the last word");
    }

    #[test]
    fn another_avatar_s_states_are_asked_for_by_its_id_and_named_without_a_name() {
        let mut page = StatesPage::default();
        let mut out = Vec::new();
        let other = Whose { id: "avtr_b".into(), worn: false };
        page.ask(&other, StateAction::Delete { id: "s1".into() }, 0.0, &mut out);
        assert!(matches!(&out[..], [Out::Action(StateAction::Delete { .. }, Some(id))] if id == "avtr_b"));
        assert!(page.shown("avtr_b", &states(&[("s1", "Party", false)]), 0.1).is_empty());
        let unnamed = AvatarView { id: "avtr_0123456789abcdef".into(), name: None, worn: false, states: Vec::new() };
        assert_eq!(avatar_name(&unnamed), "Avatar avtr_01234567…");
        assert_eq!(avatar_name(&AvatarView { worn: true, ..unnamed.clone() }), "This avatar");
        assert_eq!(avatar_name(&AvatarView { name: Some("Ultirex ✨".into()), ..unnamed }), "Ultirex ✨");
        assert_eq!((count(0).as_str(), count(1).as_str(), count(3).as_str()), ("Nothing saved yet", "1 saved state", "3 saved states"));
    }

    #[test]
    fn the_cards_words_follow_the_language_and_the_counts_follow_its_plurals() {
        use orbiters_i18n::{in_language, Language};
        let unnamed = AvatarView { id: "avtr_0123456789abcdef".into(), name: None, worn: false, states: Vec::new() };
        in_language(Language::Fr, || {
            assert_eq!((count(0).as_str(), count(1).as_str(), count(3).as_str()), ("Rien d’enregistré pour l’instant", "1 état enregistré", "3 états enregistrés"));
            assert_eq!(count_inline(0), "rien d’enregistré pour l’instant", "after a dot, not capitalized");
            assert_eq!(avatar_name(&unnamed), "Avatar avtr_01234567…");
            assert_eq!(avatar_name(&AvatarView { worn: true, ..unnamed.clone() }), "Cet avatar");
        });
        in_language(Language::Ja, || assert_eq!(count(2), "保存済みの状態 2件"));
        // A name the person chose is theirs, in any language.
        in_language(Language::Es, || assert_eq!(avatar_name(&AvatarView { name: Some("Ultirex ✨".into()), ..unnamed }), "Ultirex ✨"));
    }
}

#[cfg(test)]
mod destructive_tests {
    /// Verify3-r21 F: every destructive button of the window (red, or a bin's) is pressed through the shared fresh press, which
    /// no held key and no press of another control reaches. Those listed here only open a menu or a question, or take back a
    /// step the editor can undo.
    #[test]
    fn every_destructive_button_takes_a_press_of_its_own() {
        const NOT: [(&str, &str); 3] = [
            ("pages/editor/controls.rs", "b = b.danger();"),
            ("pages/editor/looks.rs", "Item::new(r).icon(orbiters_ui::icons::TRASH).danger()"),
            ("pages/projects/menus.rs", "Item::new("),
        ];
        fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    files(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut all = Vec::new();
        files(&src.join("pages"), &mut all);
        files(&src.join("chat_bar"), &mut all);
        all.push(src.join("app.rs"));
        let (mut checked, mut wrong) = (0, Vec::new());
        for path in all {
            let name = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            let source = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            let lines: Vec<&str> = source.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                if line.trim_start().starts_with("//") || !(line.contains("Variant::Danger") || line.contains(".danger()")) {
                    continue;
                }
                // (This test's own words.)
                if name == "pages/states.rs" && lines[..n].contains(&"mod destructive_tests {") {
                    continue;
                }
                if NOT.iter().any(|(file, what)| name == *file && line.contains(what)) {
                    continue;
                }
                checked += 1;
                let around = lines[n.saturating_sub(4)..(n + 36).min(lines.len())].join("\n");
                let fresh = [".confirm(", ".confirm_at(", "activated_fresh(", "pressed_fresh(", "sharing::pressed(", "if pressed(", "kill_confirm = Some"];
                if !fresh.iter().any(|f| around.contains(f)) {
                    wrong.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        assert!(checked >= 15, "the sweep reads the sources ({checked})");
    }
}
