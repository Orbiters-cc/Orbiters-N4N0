//! Try on Unity 6: one decision per step. First what cannot be undone (the content IDs the trial shares), then the
//! 2022 baseline VRChat's guide asks for, then where the trial goes and the room it needs on each disk (checked again
//! just before copying), the copy, and what comes next in Creator Companion. The original stays on 2022 and stays the
//! one the gallery opens; nothing here migrates, uploads or deletes.

use orbiters_i18n::{mark, tr};
use super::art;
use super::dialogs::{inline_warning, note};
use super::state::{Modal, State};
use super::unity6::{Checked, Sheet, Step};
use super::unity6_card::{ids_warning, item, notice, para, vcc_steps};
use crate::pages::{Cx, Job};
use egui::{pos2, vec2, Align2, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::discovery::Project;
use orbiters_projects::unity6::{self, budget, standing, trial};
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Tone, Variant};
use orbiters_ui::{format, icons};
use std::path::{Path, PathBuf};

pub(super) fn open(ctx: &egui::Context, state: &mut State, project: &Project, origin: Rect) {
    state.u6.sheet = Sheet {
        name: trial::trial_name(&project.name),
        location: project.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        ..Default::default()
    };
    state.u6.error = None;
    state.modal_fill = theme::control();
    state.open_modal(ctx, Modal::Unity6(project.path.clone()), origin);
}

pub(super) fn head(state: &State, path: &Path) -> (String, String) {
    let name = state.scan.projects.iter().find(|p| p.path == path).map(|p| p.name.clone()).unwrap_or_default();
    (tr("Try on Unity 6").to_string(), tr!("A trial copy of {name}, migrated by Creator Companion; the original stays on Unity 2022", name = name))
}

fn disk(path: &Path) -> String {
    if cfg!(target_os = "macos") {
        match path.strip_prefix("/Volumes").ok().and_then(|p| p.components().next()) {
            Some(n) => n.as_os_str().to_string_lossy().into_owned(),
            None => tr("the startup disk").to_string(),
        }
    } else {
        let text = path.to_string_lossy();
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
        Path::new(&text).components().next().map_or(text.clone(), |c| c.as_os_str().to_string_lossy().trim_end_matches(':').to_string())
    }
}

/// Starts checking the place (on a worker) when it changed: the copy measured, the room on each disk, what is in the way.
fn look(ctx: &egui::Context, state: &mut State, project: &Project) {
    if let Some(c) = state.u6.sheet.check.take() {
        state.u6.sheet.checked = Some(c);
    }
    if let Some(Some(dir)) = state.u6.sheet.pick.take() {
        state.u6.sheet.location = dir.display().to_string();
    }
    let key = (state.u6.sheet.location.trim().to_string(), state.u6.sheet.name.clone());
    let fresh = state.u6.sheet.checked.as_ref().is_some_and(|(k, _)| *k == key);
    if fresh || state.u6.sheet.check.running() || state.fixture || key.0.is_empty() {
        return;
    }
    let source = project.path.clone();
    let target = state.u6.matrix().unity6_target().cloned();
    let editor_missing = target.as_ref().is_some_and(|t| state.u6.editors.get(&t.unity).is_none_or(|f| f.usable().is_none()));
    state.u6.sheet.check = Job::spawn(ctx, move || {
        let checked: Checked = (|| -> anyhow::Result<_> {
            let destination = PathBuf::from(&key.0).join(&key.1);
            let copy = orbiters_projects::templates::duplication_bytes(&source, orbiters_projects::templates::Keep::default())?;
            let library = orbiters_projects::size::lookup(&source).size.map(|s| s.library);
            let editor = target.as_ref().and_then(|t| t.size_here()).filter(|_| editor_missing).map(|s| (budget::editor_install_folder(), s.download + s.installed));
            let (demands, guessed) = budget::demands(&destination, copy, library, editor);
            let plan = budget::plan(demands, &budget::Disks, guessed);
            let s = standing::read(&source, &unity6::matrix::Matrix::built_in());
            let blockers = trial::blockers(&source, &s, trial::is_open(&source));
            if destination.exists() {
                anyhow::bail!("{}", tr!("{path} exists already: choose another name.", path = destination.display()));
            }
            Ok((plan, blockers))
        })()
        .map_err(|e| format!("{e:#}"));
        (key, checked)
    });
}

fn need_label(n: budget::Need) -> &'static str {
    match n {
        budget::Need::Copy => tr("The trial's files, measured"),
        budget::Need::Import => tr("Unity 6's first import, estimated"),
        budget::Need::Editor => tr("Unity 6000 to download and install"),
        budget::Need::Margin => tr("Room kept free for Unity's temporary files"),
    }
}

fn budget_view(ui: &mut Ui, plan: &budget::Budget) {
    for v in &plan.volumes {
        let free = v.free.map(|f| format::bytes(f as f64)).unwrap_or_else(|| tr("unknown").into());
        let title = tr!("On {disk}: about {need} needed, {free} free", disk = disk(&v.at), need = format::bytes(v.total() as f64), free = free);
        let lines: Vec<String> = v
            .needs
            .iter()
            .map(|d| match d.low {
                Some(low) => tr!("{what}: {low} to {high}", what = need_label(d.need), low = format::bytes(low as f64), high = format::bytes(d.bytes as f64)),
                None => format!("{}: {}", need_label(d.need), format::bytes(d.bytes as f64)),
            })
            .collect();
        let (glyph, tint) = if v.fits() { (icons::CHECK_CIRCLE, theme::OK) } else { (icons::WARNING_CIRCLE, theme::WARN) };
        item(ui, glyph, tint, &title, Some(&lines.join("\n")));
    }
    note(
        ui,
        if plan.import_guessed {
            tr("The original's Library could not be measured, so the first import is estimated from the project's own files. Unity gives no rule for it: the estimate is wide on purpose.")
        } else {
            tr("The first import is estimated from the original's Library. Unity gives no rule for it: the estimate is wide on purpose.")
        },
    );
}

pub(super) fn body(ui: &mut Ui, cx: &mut Cx, state: &mut State, path: &Path) {
    let ctx = ui.ctx().clone();
    let Some(project) = state.scan.projects.iter().find(|p| p.path == path).cloned() else {
        note(ui, tr("This project is not listed anymore."));
        return;
    };
    let s = state.u6.standing(&project.path).cloned().unwrap_or_default();
    let worlds = s.sdk.content == Some(standing::Content::Worlds);
    if let Some(e) = state.u6.error.clone() {
        inline_warning(ui, &e);
    }
    match state.u6.sheet.step {
        Step::Ids => {
            ids_warning(ui, worlds);
            para(
                ui,
                tr("Orbiters makes a copy of the project for the trial: the original stays on Unity 2022 and stays as it is. Creator Companion then migrates the copy, as VRChat's guide describes. Orbiters cannot stop an upload made from the VRChat SDK's own panel."),
                12.5,
                theme::FG_2,
            );
            note(ui, tr("To test online, give the trial a test ID of its own first: in the trial's scene, detach the blueprint ID in its Pipeline Manager and upload it as a new private avatar or world. Orbiters does not change Pipeline Managers."));
        }
        Step::Baseline => {
            para(
                ui,
                tr("VRChat's guide asks to check the project in Unity 2022 first: open it, look at Unity's Console and at the VRChat SDK's Builder for errors. A trial of a project that does not build in 2022 says little about Unity 6."),
                12.5,
                theme::FG_2,
            );
            let label = tr("It builds in Unity 2022 today");
            let mut confirmed = ctx.data(|d| d.get_temp::<bool>(egui::Id::unique(("u6-baseline", &project.path)))).unwrap_or(false);
            widgets::field(ui, label, Some(tr("Console without errors, and the SDK's Builder without errors.")), 48.0, |ui| {
                widgets::toggle(ui, "u6-baseline", &mut confirmed);
            });
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(("u6-baseline", &project.path)), confirmed));
            let open = state.session(&project.path).open();
            let b = Button::new(if open { tr("Unity has it open") } else { tr("Open in Unity 2022") }).icon(icons::PLAY).variant(Variant::Ghost).enabled(!open && !state.running()).id_salt("u6-open-2022");
            if widgets::activated(&b.show(ui)) {
                state.open_unity(&ctx, &project, false);
            }
        }
        Step::Place => {
            look(&ctx, state, &project);
            let w = ui.available_width();
            art::caps(ui, tr("Name of the trial"));
            widgets::text_field(ui, "u6-name", &mut state.u6.sheet.name, tr("My avatar (Unity 6)"), w, false);
            art::caps(ui, tr("Location"));
            let (row, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
            {
                let mut fu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(row.min, vec2(w - 44.0, 36.0))).id_salt("u6-location-slot"));
                widgets::text_field(&mut fu, "u6-location", &mut state.u6.sheet.location, tr("Choose a folder"), w - 44.0, false);
            }
            let browse = IconButton::new(icons::FOLDER_OPEN, mark("Choose a folder")).size(36.0, 16.0).backdrop(theme::control()).id_salt("u6-browse");
            if widgets::activated(&browse.show_at(ui, Rect::from_min_size(pos2(row.right() - 36.0, row.top()), vec2(36.0, 36.0)))) && !state.u6.sheet.pick.running() && !state.fixture {
                let start = PathBuf::from(state.u6.sheet.location.trim());
                state.u6.sheet.pick = Job::spawn(&ctx, move || {
                    let mut d = rfd::FileDialog::new().set_title(tr("Where the trial goes"));
                    if start.is_dir() {
                        d = d.set_directory(start);
                    }
                    d.pick_folder()
                });
            }
            if let Err(e) = orbiters_projects::templates::validate_name(&state.u6.sheet.name) {
                inline_warning(ui, &e.to_string());
            }
            match state.u6.sheet.checked.clone() {
                None => para(ui, tr("Measuring the copy and the room on each disk…"), 12.0, theme::FG_3),
                Some((_, Err(e))) => inline_warning(ui, &e),
                Some((_, Ok((plan, blockers)))) => {
                    budget_view(ui, &plan);
                    if !blockers.is_empty() {
                        notice(ui, Tone::Warn, tr("Not ready for a trial yet"), &unity6::describe_blockers(&blockers));
                    }
                }
            }
            note(ui, tr("Library, Temp, Logs and Git history stay behind, as in Duplicate: Unity 6 imports the trial afresh. Installed packages are copied at their exact versions."));
        }
        Step::Copying => {
            let p = state.u6.run.as_ref().map(|r| r.progress.lock().unwrap().clone()).unwrap_or_default();
            let w = ui.available_width();
            let (r, _) = ui.allocate_exact_size(vec2(w, 92.0), Sense::hover());
            let pt = ui.painter();
            widgets::frame(pt, r, 14.0, theme::alpha(theme::ACCENT, 0.07));
            orbiters_ui::feedback::spinner(pt, pos2(r.left() + 22.0, r.top() + 26.0), 8.0, 2.2, theme::ACCENT);
            let pct = if p.total > 0 { format!(" · {:.0}%", 100.0 * p.copied as f32 / p.total as f32) } else { String::new() };
            widgets::text(pt, pos2(r.left() + 42.0, r.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(format!("{}{pct}", super::unity6::phase_label(&p.phase)), 13.5));
            let bar = Rect::from_min_size(pos2(r.left() + 16.0, r.top() + 50.0), vec2(r.width() - 32.0, 6.0));
            pt.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
            let f = if p.total > 0 { p.copied as f32 / p.total as f32 } else { 0.0 };
            pt.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), 6.0)), theme::radius(3.0), theme::ACCENT);
            widgets::text_truncated(pt, pos2(r.left() + 16.0, r.top() + 74.0), Align2::LEFT_CENTER, &Txt::new(p.file, 11.5).color(theme::FG_3), r.width() - 32.0);
            note(ui, tr("Orbiters recorded this trial before copying: if the copy is cut short, the gallery shows it under Unity 6 trials with its folder and what can be done."));
        }
        Step::Done => match state.u6.sheet.done.clone() {
            Some(Ok(job)) => {
                item(ui, icons::CHECK_CIRCLE, theme::OK, tr("The trial copy is ready"), Some(&job.trial.path.display().to_string()));
                vcc_steps(ui, state, &job.trial.path, &job.target);
            }
            Some(Err(e)) => {
                notice(ui, Tone::Danger, tr("The trial could not be made"), &e);
                note(ui, tr("What was made is listed in the gallery under Unity 6 trials, with its folder and what can be done."));
            }
            None => {}
        },
    }
    let _ = cx;
}

pub(super) fn footer(ui: &mut Ui, cx: &mut Cx, state: &mut State, path: &Path) {
    let ctx = ui.ctx().clone();
    let close = |ui: &mut Ui, state: &mut State, label: &str| {
        if widgets::activated(&Button::new(label).variant(Variant::Ghost).id_salt("u6-close").show(ui)) {
            state.close_modal(ui.ctx());
        }
    };
    let next = |ui: &mut Ui, label: &str, enabled: bool| widgets::activated(&Button::new(label).variant(Variant::Primary).enabled(enabled).id_salt(("u6-next", label)).show(ui));
    match state.u6.sheet.step {
        Step::Ids => {
            if next(ui, tr("I understand, go on"), true) {
                state.u6.sheet.step = Step::Baseline;
            }
            close(ui, state, tr("Cancel"));
        }
        Step::Baseline => {
            let confirmed = ctx.data(|d| d.get_temp::<bool>(egui::Id::unique(("u6-baseline", path)))).unwrap_or(false);
            if next(ui, tr("Go on"), confirmed) {
                state.u6.sheet.step = Step::Place;
            }
            if widgets::activated(&Button::new(tr("Back")).variant(Variant::Ghost).id_salt("u6-back").show(ui)) {
                state.u6.sheet.step = Step::Ids;
            }
        }
        Step::Place => {
            let key = (state.u6.sheet.location.trim().to_string(), state.u6.sheet.name.clone());
            let ready = state.u6.sheet.checked.as_ref().is_some_and(|(k, c)| *k == key && c.as_ref().is_ok_and(|(plan, blockers)| plan.fits() && blockers.is_empty()))
                && orbiters_projects::templates::validate_name(&key.1).is_ok()
                && Path::new(&key.0).is_absolute();
            if next(ui, tr("Make the trial copy"), ready && state.u6.run.is_none()) {
                state.start_trial(&ctx, path.to_path_buf(), PathBuf::from(&key.0), key.1);
            }
            if widgets::activated(&Button::new(tr("Back")).variant(Variant::Ghost).id_salt("u6-back").show(ui)) {
                state.u6.sheet.step = Step::Baseline;
            }
        }
        Step::Copying => {
            if widgets::activated(&Button::new(tr("Stop copying")).variant(Variant::Ghost).id_salt("u6-stop").show(ui)) {
                if let Some(r) = &state.u6.run {
                    r.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        Step::Done => {
            if let Some(Ok(job)) = state.u6.sheet.done.clone() {
                if next(ui, tr("Show the trial"), true) {
                    state.close_modal(&ctx);
                    state.open(&ctx, job.trial.path.clone());
                }
            }
            close(ui, state, tr("Close"));
        }
    }
    let _ = cx;
}
