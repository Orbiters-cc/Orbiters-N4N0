//! Duplicate…: a copy of a project beside it (or anywhere), under the name the person gives it ("<name> copy" to begin
//! with), with or without Library, how much it takes against the room left on that disk, the copy's progress and its
//! Stop. Once copied its packages are restored, VCC and ALCOM list it, and it opens in the panel. Its scenes keep their
//! blueprint IDs, which the dialog says: uploads from the copy update the same avatars and worlds.

use orbiters_i18n::{mark, tr};
use super::art;
use super::dialogs::{creating, inline_warning, note};
use super::state::{Dup, Modal, Outcome, State};
use super::ui::errors;
use crate::pages::Job;
use egui::{pos2, vec2, Align2, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::{discovery::Project, size, templates};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Variant};
use orbiters_ui::{format, icons};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Opens the dialog on `project`, growing out of `origin`.
pub(super) fn open(ctx: &egui::Context, state: &mut State, project: &Project, origin: Rect) {
    state.dup = Dup {
        name: format!("{} copy", project.name),
        location: project.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        ..Default::default()
    };
    state.modal_fill = theme::control();
    state.open_modal(ctx, Modal::Duplicate(project.path.clone()), origin);
}

fn source<'a>(state: &'a State, path: &Path) -> Option<&'a Project> {
    state.scan.projects.iter().find(|p| p.path == path)
}

/// What the dialog's place for the copy says: the name refused, a folder there already, or not enough room.
fn problem(state: &State, project: &Project) -> Option<String> {
    let d = &state.dup;
    if state.repairing(&project.path) {
        return Some(tr("Orbiters is moving this project's Library: duplicate it once that ends.").into());
    }
    if let Err(e) = templates::validate_name(&d.name) {
        return Some(if d.name.is_empty() { tr("Name the copy.").into() } else { e.to_string() });
    }
    if d.location.trim().is_empty() || !Path::new(d.location.trim()).is_absolute() {
        return Some(tr("Choose where the copy goes.").into());
    }
    let (location, name, exists, free) = d.checked.as_ref()?;
    if location != d.location.trim() || name != &d.name {
        return None;
    }
    if *exists {
        return Some(tr!("{location} has a folder named {name} already: choose another name.", location = d.location.trim(), name = name));
    }
    if let Some((_, Err(error))) = &d.measured { return Some(error.clone()); }
    let Some(need) = needed(state, project) else { return Some(tr("Checking required disk space…").into()); };
    if free.is_none() { return Some(tr("Cannot check free space. Choose an accessible destination.").into()); }
    free.filter(|f| *f < need).map(|f| tr!("The copy needs about {need}, and that disk has {free} left. Free some space or choose another disk.", need = format::bytes(need as f64), free = format::bytes(f as f64)))
}

/// About what the copy takes, once the project was measured.
fn needed(state: &State, project: &Project) -> Option<u64> {
    if !state.fixture {
        return state.dup.measured.as_ref().filter(|(library, _)| *library == state.dup.library).and_then(|(_, size)| size.as_ref().ok()).copied();
    }
    let s = state.sizes.get(&project.path)?;
    Some(if state.dup.library { s.total } else { s.without_library() })
}

pub(super) fn body(ui: &mut Ui, state: &mut State, path: &Path) {
    let ctx = ui.ctx().clone();
    errors(ui, state, "duplicate");
    let Some(project) = source(state, path).cloned() else {
        note(ui, tr("This project is not listed anymore."));
        return;
    };
    state.want_size(&ctx, &project.path);
    if let Some(measured) = state.dup.measure.take() { state.dup.measured = Some(measured); }
    if !state.fixture && !state.dup.measure.running() && state.dup.measured.as_ref().is_none_or(|(library, _)| *library != state.dup.library) {
        let source = project.path.clone();
        let library = state.dup.library;
        state.dup.measure = Job::spawn(&ctx, move || (library, templates::duplication_bytes(&source, templates::Keep { library }).map_err(|e| e.to_string())));
    }
    if let Some(Some(dir)) = state.dup.pick.take() {
        state.dup.location = dir.display().to_string();
    }
    // Whether the place is free, and the room on its disk: looked at on a worker as they change.
    let key = (state.dup.location.trim().to_string(), state.dup.name.clone());
    let checked = state.dup.checked.as_ref().is_some_and(|(l, n, _, _)| (l, n) == (&key.0, &key.1));
    if !checked && !state.dup.check.running() && !state.fixture && !key.0.is_empty() {
        let (location, name) = key.clone();
        state.dup.check = Job::spawn(&ctx, move || {
            let dir = PathBuf::from(&location);
            let exists = !name.is_empty() && dir.join(&name).exists();
            let free = size::free_space(&dir);
            (location, name, exists, free)
        });
    }
    let w = ui.available_width();
    let busy = state.running();
    if busy {
        creating(ui, state);
        ui.disable();
    }

    // The project copied.
    let (r, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, r, 14.0, theme::surface_soft());
    art::cover(p, state, &project, Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 22.0), vec2(44.0, 44.0)), [10.0; 4], 1.0);
    let x = r.left() + 66.0;
    widgets::text_truncated(p, pos2(x, r.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(&project.name, 13.5).weight(weight::MEDIUM), w - 80.0);
    let size = match state.sizes.get(&project.path) {
        Some(s) => format!(" · {}", format::bytes(s.total as f64)),
        None => String::new(),
    };
    let meta = format!("{}{size}", project.path.display());
    widgets::text_truncated(p, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(meta, 11.5).color(theme::FG_3), w - 80.0);

    art::caps(ui, tr("Name of the copy"));
    widgets::text_field(ui, "dup-name", &mut state.dup.name, tr("My avatar copy"), w, false);
    art::caps(ui, tr("Location"));
    let (row, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    {
        let mut fu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(row.min, vec2(w - 44.0, 36.0))).id_salt("dup-location-slot"));
        widgets::text_field(&mut fu, "dup-location", &mut state.dup.location, tr("Choose a folder"), w - 44.0, false);
    }
    let browse = IconButton::new(icons::FOLDER_OPEN, mark("Choose a folder")).size(36.0, 16.0).backdrop(theme::control()).id_salt("dup-browse");
    if widgets::activated(&browse.show_at(ui, Rect::from_min_size(pos2(row.right() - 36.0, row.top()), vec2(36.0, 36.0)))) && !state.dup.pick.running() && !state.fixture {
        let start = PathBuf::from(state.dup.location.trim());
        state.dup.pick = Job::spawn(&ctx, move || {
            let mut d = rfd::FileDialog::new().set_title(tr("Where the copy goes"));
            if start.is_dir() {
                d = d.set_directory(start);
            }
            d.pick_folder()
        });
    }
    match problem(state, &project) {
        Some(e) if !busy => inline_warning(ui, &e),
        _ if !state.dup.name.is_empty() && !state.dup.location.trim().is_empty() => {
            // As the location is written (forward or back slashes).
            let loc = state.dup.location.trim();
            let sep = if loc.contains('\\') { '\\' } else { '/' };
            let target = format!("{}{sep}{}", loc.trim_end_matches(['/', '\\']), state.dup.name);
            let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
            widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.center().y), art::glyph::ARROW_RIGHT, 13.0, theme::FG_4);
            widgets::text_truncated(ui.painter(), pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(target, 12.0).color(theme::FG_3), w - 22.0);
        }
        _ => {}
    }

    // Library, or Unity importing everything again.
    let library = state.sizes.get(&project.path).map(|s| s.library).filter(|l| *l > 0);
    let description = match library {
        Some(l) => tr!("Opens at once instead of importing everything again, for {size} more", size = format::bytes(l as f64)),
        None => tr("Opens at once instead of importing everything again, for the room Library takes").into(),
    };
    widgets::field(ui, tr("Copy Library too"), Some(&description), 48.0, |ui| {
        widgets::toggle(ui, "dup-library", &mut state.dup.library);
    });
    if let (Some(need), Some((_, _, _, Some(free)))) = (needed(state, &project), state.dup.checked.as_ref()) {
        let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
        widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.center().y), icons::HARD_DRIVE, 13.0, theme::FG_4);
        let room = tr!("About {need} to copy · {free} free on that disk", need = format::bytes(need as f64), free = format::bytes(*free as f64));
        widgets::text_truncated(ui.painter(), pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(room, 12.0).color(theme::FG_3), w - 22.0);
    }
    note(
        ui,
        if project.kind == orbiters_projects::discovery::Kind::Worlds {
            tr("The copy's scenes keep their blueprint IDs: uploading from it updates the same worlds on VRChat. To upload new ones, detach the blueprint IDs in the scenes' Pipeline Manager first.")
        } else {
            tr("The copy's scenes keep their blueprint IDs: uploading from it updates the same avatars on VRChat. To upload new ones, detach the blueprint IDs in the scenes' Pipeline Manager first.")
        },
    );
    note(ui, tr("Temp, Logs, UserSettings and Git history stay behind. Installed dependencies are included automatically, and VCC and ALCOM list the copy too."));
}

pub(super) fn footer(ui: &mut Ui, state: &mut State, path: &Path) {
    let ctx = ui.ctx().clone();
    let Some(project) = source(state, path).cloned() else {
        if widgets::activated(&Button::new(tr("Close")).variant(Variant::Ghost).id_salt("dup-close").show(ui)) {
            state.close_modal(&ctx);
        }
        return;
    };
    let busy = state.running();
    let place = state.dup.checked.as_ref().is_some_and(|(l, n, _, _)| l == state.dup.location.trim() && n == &state.dup.name);
    let enabled = !busy && place && problem(state, &project).is_none();
    let b = Button::new(if busy { tr("Duplicating…") } else { tr("Duplicate") }).icon(icons::COPY).variant(Variant::Primary).enabled(enabled).id_salt("dup-go");
    if widgets::activated(&b.show(ui)) {
        let source = project.path.clone();
        let parent = PathBuf::from(state.dup.location.trim());
        let name = state.dup.name.clone();
        let keep = templates::Keep { library: state.dup.library };
        state.copy_cancel = Arc::new(AtomicBool::new(false));
        *state.copy_progress.lock().unwrap() = Default::default();
        let cancel = state.copy_cancel.clone();
        let progress = state.copy_progress.clone();
        state.task(&ctx, &tr!("Duplicating {name}…", name = project.name), move || {
            let root = orbiters_projects::vcc_dir();
            let path = orbiters_projects::runtime()?.block_on(templates::duplicate(&root, &source, &parent, &name, keep, &cancel, &progress))?;
            Ok(Outcome::Added(path, tr!("{name} is ready: VCC and ALCOM list it too", name = name)))
        });
    }
    if busy {
        let p = state.copy_progress.lock().unwrap().clone();
        if p.phase == mark("Copying project") && widgets::activated(&Button::new(tr("Stop copying")).variant(Variant::Ghost).id_salt("dup-stop").show(ui)) {
            state.copy_cancel.store(true, Ordering::Relaxed);
        }
    } else if widgets::activated(&Button::new(tr("Cancel")).variant(Variant::Ghost).id_salt("dup-cancel").show(ui)) {
        state.close_modal(&ctx);
    }
}
