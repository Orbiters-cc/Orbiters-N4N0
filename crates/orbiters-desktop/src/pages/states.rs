//! Home's States page: N4N0's saved states of the avatar worn (controller/states.rs). Save how the avatar is now under a
//! name; for each saved state, apply it, make it the default (put back after each world join and each switch to the
//! avatar), rename or delete it, and choose whether it puts the GoGoLoco pose and the avatar's size back too.
//!
//! What a press asks shows at once: the page keeps it over the service's list until the list says the same (or a few
//! seconds passed, when the service refused it and says why). Switches and the default act on the pointer's press,
//! buttons on its release with the press shown at once, as on the rest of Home.

use super::{block, section, Cx};
use crate::state::{StateAction, StateView, StatesView};
use egui::{pos2, vec2, Align2, Key, Rect, Sense, Ui};
use orbiters_orb::menu::{next_state_name, StateRow};
use orbiters_ui::anim;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};

/// How long what was asked here shows over the service's list before the list has the last word.
const PENDING_FOR: f64 = 4.0;
/// A first press on Delete waits this long for the second.
const CONFIRM_FOR: f64 = 3.0;

/// What the page asks of Home.
pub enum Out {
    /// Back to Home itself.
    Home,
    /// Asked of the service.
    Action(StateAction),
}

#[derive(Default)]
pub struct StatesPage {
    /// The name typed for the next save.
    name: String,
    /// The state being renamed, and the name being typed (focused once).
    renaming: Option<(String, String, bool)>,
    /// What was asked and the service's list does not say yet, since when.
    pending: Vec<(StateAction, f64)>,
    /// The state whose Delete was pressed once, and when (a second press deletes).
    confirm: Option<(String, f64)>,
    /// The service's last note shown as a toast.
    note_seen: Option<u64>,
}

impl StatesPage {
    /// What `view` lists once what was asked here is applied over it (and what the list already says is dropped).
    fn shown(&mut self, view: &StatesView, now: f64) -> Vec<StateView> {
        self.pending.retain(|(action, at)| now - at < PENDING_FOR && !done(action, &view.states));
        let mut states = view.states.clone();
        for (action, _) in &self.pending {
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

    /// Something asked: kept until the service's list says it, and sent.
    fn ask(&mut self, action: StateAction, now: f64, out: &mut Vec<Out>) {
        if !matches!(action, StateAction::Apply { .. }) {
            self.pending.push((action.clone(), now));
        }
        out.push(Out::Action(action));
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

/// The page: its heading, saving, then the saved states.
pub fn show(ui: &mut Ui, cx: &mut Cx, page: &mut StatesPage, out: &mut Vec<Out>) {
    let now = anim::time(ui.ctx());
    let view = cx.snap.orb.states.clone();
    // What the service said of the last save or apply: a toast, once (what went wrong comes as the service's own toast).
    if let Some(note) = &view.note {
        if page.note_seen.is_some_and(|seen| seen != note.id) && !note.error {
            cx.toasts.push(ui.ctx(), orbiters_ui::toast::Kind::Success, note.text.clone());
        }
        page.note_seen = Some(note.id);
    } else {
        page.note_seen.get_or_insert(0);
    }
    let states = page.shown(&view, now);
    if page.confirm.as_ref().is_some_and(|(_, at)| now - at >= CONFIRM_FOR) {
        page.confirm = None;
    }
    let avatar = view.avatar_name.clone().unwrap_or_else(|| "This avatar".into());
    let sub = match (&view.avatar_id, states.len()) {
        (None, _) => "VRChat's avatar is not known yet".to_string(),
        (Some(_), 0) => format!("{avatar}: nothing saved yet"),
        (Some(_), 1) => format!("{avatar}: 1 saved state"),
        (Some(_), n) => format!("{avatar}: {n} saved states"),
    };
    block(ui, cx, 0, 116.0, |ui, rect| {
        let back = Button::new("Home").icon(icons::ARROW_LEFT).variant(Variant::Ghost).size(Size::Sm);
        let w = back.desired_width(ui);
        if back.show_at(ui, Rect::from_min_size(rect.min, vec2(w, 32.0))).clicked() {
            out.push(Out::Home);
        }
        super::heading(ui, Rect::from_min_max(pos2(rect.left(), rect.top() + 44.0), rect.max), "N4N0 menu", "States", Some(&sub));
    });
    if view.avatar_id.is_none() {
        let text = "Start VRChat with OSC on: the states shown are those of the avatar you wear. A state keeps every parameter of it \
                    that OSC can set, and its size; applying one puts them back.";
        return note(ui, cx, 1, icons::BOOKMARKS_SIMPLE, "No avatar yet", text);
    }
    save_card(ui, cx, page, &states, view.saving, now, out);
    if states.is_empty() {
        let text = "Save keeps how the avatar is now: its toggles, sliders and outfits. Make one the default and it is put back \
                    after each world join, and each time you switch to this avatar.";
        return note(ui, cx, 2, icons::BOOKMARKS_SIMPLE, "Nothing saved for this avatar", text);
    }
    // The newest first.
    for (i, state) in states.iter().enumerate().rev() {
        state_card(ui, cx, 2 + states.len() - 1 - i, page, state, now, out);
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

/// Saving how the avatar is now: a name (or the next "State n"), and Save.
fn save_card(ui: &mut Ui, cx: &Cx, page: &mut StatesPage, states: &[StateView], saving: bool, now: f64, out: &mut Vec<Out>) {
    let rows: Vec<StateRow> = states.iter().map(|s| StateRow { name: s.name.clone(), ..Default::default() }).collect();
    let placeholder = next_state_name(&rows);
    let sub = "Every parameter OSC can set, GoGoLoco's pose and the avatar's size (N4N0's own stay out)";
    section(ui, cx, 1, icons::FLOPPY_DISK, "Save how it is now", Some(sub), |ui| {
        ui.add_space(14.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let button_w = 112.0;
        let field = Rect::from_min_size(row.min, vec2(row.width() - button_w - 12.0, 36.0)).translate(vec2(0.0, 2.0));
        let mut fu = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let resp = widgets::text_field(&mut fu, "orbiters-states-name", &mut page.name, &placeholder, field.width(), false);
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let label = if saving { "Saving…" } else { "Save" };
        let save = Button::new(label).icon(icons::FLOPPY_DISK).variant(Variant::Primary).width(button_w).id_salt("orbiters-states-save");
        let clicked = save.show_at(ui, Rect::from_min_size(pos2(row.right() - button_w, row.top()), vec2(button_w, 40.0))).clicked();
        if clicked || enter {
            let name = page.name.trim().to_string();
            let name = if name.is_empty() { placeholder.clone() } else { name };
            page.name.clear();
            page.ask(StateAction::Save { name: Some(name), size: true, pose: false, default: false }, now, out);
        }
        ui.add_space(8.0);
    });
}

/// One saved state: what it puts back, its default, and its buttons.
fn state_card(ui: &mut Ui, cx: &Cx, index: usize, page: &mut StatesPage, state: &StateView, now: f64, out: &mut Vec<Out>) {
    let saving = state.id.is_empty();
    let when = if saving { "Saving…".to_string() } else { super::local_time(&state.saved_utc) };
    let others = state.parameters.saturating_sub(state.pose_parameters);
    let sub = if saving { when } else { format!("Saved {when} · {others} parameter{}", if others == 1 { "" } else { "s" }) };
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
            "No GoGoLoco pose was saved with it".to_string()
        } else {
            format!("Puts GoGoLoco's pose back too ({} of its parameters)", state.pose_parameters)
        };
        widgets::field(ui, "Include GoGoLoco pose", Some(&pose_note), 48.0, |ui| {
            if saving || state.pose_parameters == 0 {
                ui.disable();
            }
            let mut on = state.pose;
            if widgets::toggle(ui, ("orbiters-state-pose", &id), &mut on) {
                page.ask(StateAction::Options { id: id.clone(), pose: on, size: state.size }, now, out);
            }
        });
        widgets::separator(ui);
        let size_note = match state.eye_height {
            Some(h) => format!("Puts your size back too: eyes at {h:.2} m, where the world allows it"),
            None => "VRChat did not say the avatar's size when it was saved".to_string(),
        };
        widgets::field(ui, "Include avatar size", Some(&size_note), 48.0, |ui| {
            if saving || state.eye_height.is_none() {
                ui.disable();
            }
            let mut on = state.size;
            if widgets::toggle(ui, ("orbiters-state-size", &id), &mut on) {
                page.ask(StateAction::Options { id: id.clone(), pose: state.pose, size: on }, now, out);
            }
        });
        widgets::separator(ui);
        ui.add_space(14.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        if renaming {
            rename_row(ui, row, page, &id, now, out);
        } else {
            buttons_row(ui, row, page, state, saving, now, out);
        }
        ui.add_space(8.0);
    });
}

/// "Default", in the accent, at `left_top`.
fn default_pill(ui: &Ui, left_top: egui::Pos2) {
    let txt = Txt::new("Default", 11.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
    let w = widgets::text_size(ui, &txt).x + 14.0 + 16.0;
    let r = Rect::from_min_size(left_top, vec2(w, 18.0));
    let p = ui.painter();
    p.rect_filled(r, theme::radius(9.0), theme::alpha(theme::ACCENT, 0.14));
    widgets::icon(p, pos2(r.left() + 13.0, r.center().y), icons::STAR, 11.0, theme::ACCENT);
    widgets::text(p, pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &txt);
}

/// Apply, the default, Rename and Delete.
fn buttons_row(ui: &mut Ui, row: Rect, page: &mut StatesPage, state: &StateView, saving: bool, now: f64, out: &mut Vec<Out>) {
    let id = state.id.clone();
    let mut x = row.left();
    let apply = Button::new("Apply").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Primary).enabled(!saving).id_salt(("orbiters-state-apply", &id));
    let w = apply.desired_width(ui).max(104.0);
    if apply.width(w).show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0))).clicked() {
        page.ask(StateAction::Apply { id: id.clone() }, now, out);
    }
    x += w + 8.0;
    // The default is a choice: it acts on the press, as the switches do.
    let (label, hint) = if state.default {
        ("Default", "Put back after each world join and each switch to this avatar. Press to stop.")
    } else {
        ("Set as default", "Put back after each world join and each switch to this avatar")
    };
    let default = Button::new(label).icon(icons::STAR).variant(if state.default { Variant::Glass } else { Variant::Secondary });
    let default = default.enabled(!saving).hint(hint).id_salt(("orbiters-state-default", &id));
    let w = default.desired_width(ui);
    let rect = Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0));
    let resp = default.show_at(ui, rect);
    if !saving && widgets::pressed(&resp) {
        page.ask(StateAction::Default { id: (!state.default).then(|| id.clone()) }, now, out);
    }
    if state.default {
        ui.painter().rect_stroke(rect, theme::radius(20.0), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.55)), egui::StrokeKind::Inside);
    }
    // At the right: Rename, then Delete (twice: the first press asks).
    let confirming = page.confirm.as_ref().is_some_and(|(c, _)| *c == id);
    let delete = Button::new(if confirming { "Delete it?" } else { "Delete" }).icon(icons::TRASH).variant(Variant::Danger).enabled(!saving);
    let delete = delete.id_salt(("orbiters-state-delete", &id));
    let dw = delete.desired_width(ui);
    if delete.show_at(ui, Rect::from_min_size(pos2(row.right() - dw, row.top()), vec2(dw, 40.0))).clicked() {
        if confirming {
            page.confirm = None;
            page.ask(StateAction::Delete { id: id.clone() }, now, out);
        } else {
            page.confirm = Some((id.clone(), now));
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(CONFIRM_FOR));
        }
    }
    let rename = Button::new("Rename").icon(icons::PENCIL_SIMPLE).variant(Variant::Ghost).enabled(!saving).id_salt(("orbiters-state-rename", &id));
    let rw = rename.desired_width(ui);
    if rename.show_at(ui, Rect::from_min_size(pos2(row.right() - dw - 8.0 - rw, row.top()), vec2(rw, 40.0))).clicked() {
        page.renaming = Some((id, state.name.clone(), true));
    }
}

/// Renaming: the name in a field, Save and Cancel (Enter saves, Escape cancels).
fn rename_row(ui: &mut Ui, row: Rect, page: &mut StatesPage, id: &str, now: f64, out: &mut Vec<Out>) {
    let Some((_, name, focus)) = page.renaming.as_mut() else { return };
    let buttons_w = 96.0 + 8.0 + 96.0;
    let field = Rect::from_min_size(row.min + vec2(0.0, 2.0), vec2((row.width() - buttons_w - 12.0).min(420.0), 36.0));
    let mut fu = ui.new_child(egui::UiBuilder::new().max_rect(field));
    let resp = widgets::text_field(&mut fu, ("orbiters-state-rename-field", id), name, "Name", field.width(), false);
    if std::mem::take(focus) {
        resp.request_focus();
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    let x = field.right() + 12.0;
    let save = Button::new("Save").variant(Variant::Primary).width(96.0).id_salt(("orbiters-state-rename-save", id));
    let saved = save.show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(96.0, 40.0))).clicked();
    let cancel = Button::new("Cancel").variant(Variant::Ghost).width(96.0).id_salt(("orbiters-state-rename-cancel", id));
    let cancelled = cancel.show_at(ui, Rect::from_min_size(pos2(x + 104.0, row.top()), vec2(96.0, 40.0))).clicked();
    if saved || enter {
        let name = name.trim().to_string();
        page.renaming = None;
        if !name.is_empty() {
            page.ask(StateAction::Rename { id: id.to_string(), name }, now, out);
        }
    } else if cancelled || escape {
        page.renaming = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(states: &[(&str, &str, bool)]) -> StatesView {
        StatesView {
            avatar_id: Some("avtr_a".into()),
            states: states
                .iter()
                .map(|(id, name, default)| StateView { id: id.to_string(), name: name.to_string(), default: *default, ..Default::default() })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn what_was_asked_shows_at_once_until_the_service_says_it() {
        let mut page = StatesPage::default();
        let mut out = Vec::new();
        let before = view(&[("s1", "Hoodie on", false), ("s2", "Cosy", true)]);
        page.ask(StateAction::Default { id: Some("s1".into()) }, 0.0, &mut out);
        page.ask(StateAction::Rename { id: "s2".into(), name: "Cosy evening".into() }, 0.0, &mut out);
        page.ask(StateAction::Save { name: Some("State 3".into()), size: true, pose: false, default: false }, 0.0, &mut out);
        page.ask(StateAction::Apply { id: "s1".into() }, 0.0, &mut out);
        assert_eq!(out.len(), 4, "all of it sent");
        let shown = page.shown(&before, 0.5);
        let rows: Vec<(&str, &str, bool)> = shown.iter().map(|s| (s.id.as_str(), s.name.as_str(), s.default)).collect();
        assert_eq!(rows, vec![("s1", "Hoodie on", true), ("s2", "Cosy evening", false), ("", "State 3", false)], "the service has not answered yet");
        // The service says the default and the name: those are its own now; the save is not listed yet.
        let after = view(&[("s1", "Hoodie on", true), ("s2", "Cosy evening", false)]);
        assert_eq!(page.shown(&after, 1.0).len(), 3);
        assert_eq!(page.pending.len(), 1, "only the save waits");
        let saved = view(&[("s1", "Hoodie on", true), ("s2", "Cosy evening", false), ("s3", "State 3", false)]);
        assert_eq!(page.shown(&saved, 1.5).len(), 3);
        assert!(page.pending.is_empty());
        // A delete the service refused shows again once the wait is over.
        page.ask(StateAction::Delete { id: "s3".into() }, 2.0, &mut out);
        assert_eq!(page.shown(&saved, 2.1).len(), 2);
        assert_eq!(page.shown(&saved, 2.0 + PENDING_FOR).len(), 3, "the service's list has the last word");
    }
}
