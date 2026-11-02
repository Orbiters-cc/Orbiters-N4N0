//! The dialog of a package change that conflicted (a version list, an Update button, Update all): what conflicts
//! (who needs which versions), the plan that makes the whole coherent (each package from → to, added or removed,
//! and why) and one button applying it through `vpm::Change::Plan`, the usual install path. A package no version of
//! which fits blocks the plan, named with the range it holds; Remove it too solves again without it. Cancel changes
//! nothing.
use super::art::{self, glyph};
use super::state::{Outcome, State};
use crate::pages::Job;
use egui::{pos2, vec2, Align2, Rect, Sense, Ui};
use orbiters_projects::{coherent, vpm};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{feedback, icons};
use std::path::PathBuf;

pub(crate) struct View {
    pub path: PathBuf,
    pub plan: coherent::Plan,
    pub solving: Job<Result<coherent::Plan, String>>,
}

/// The dialog's title, subtitle and width.
pub(super) fn head(state: &State) -> (String, String) {
    let Some(v) = &state.conflict else { return ("Packages conflict".into(), String::new()) };
    let asked: Vec<String> = v.plan.requested.iter().map(|(n, ver)| format!("{} {ver}", display(v, n))).collect();
    let title = if v.plan.solved() || v.plan.blockers.is_empty() { "Update to coherent versions" } else { "This change is blocked" };
    (title.into(), format!("{} needs other packages to change with it", asked.join(", ")))
}

fn display(v: &View, name: &str) -> String {
    v.plan
        .conflicts
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.display_name.clone())
        .or_else(|| v.plan.steps.iter().find(|s| s.name == name).map(|s| s.display_name.clone()))
        .unwrap_or_else(|| name.to_string())
}

fn para(ui: &mut Ui, txt: Txt) {
    let w = ui.available_width();
    let galley = ui.painter().layout_job(txt.job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(r.min, galley, theme::FG);
}

pub(super) fn body(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    super::ui::errors(ui, state, "conflict");
    let Some(v) = state.conflict.as_mut() else { return };
    if let Some(r) = v.solving.take() {
        match r {
            Ok(plan) => v.plan = plan,
            Err(e) => state.error = Some(e),
        }
    }
    let Some(v) = state.conflict.as_ref() else { return };
    let solving = v.solving.running();
    let plan = v.plan.clone();
    let w = ui.available_width();
    if !plan.conflicts.is_empty() {
        art::caps(ui, "What conflicts");
        for c in &plan.conflicts {
            let lines: Vec<String> = c.needed_by.iter().map(|n| format!("{} {} accepts {}", n.display_name, n.version, if n.range.is_empty() { "another version" } else { &n.range })).collect();
            let body = Txt::new(lines.join("\n"), 12.0).color(theme::FG_3).line_height(18.0);
            let galley = ui.painter().layout_job(body.job(w - 44.0));
            let (r, _) = ui.allocate_exact_size(vec2(w, 34.0 + galley.size().y + 10.0), Sense::hover());
            let p = ui.painter();
            widgets::frame(p, r, 14.0, theme::alpha(theme::WARN, 0.08));
            widgets::icon(p, pos2(r.left() + 20.0, r.top() + 20.0), icons::WARNING, 14.0, theme::WARN);
            widgets::text_truncated(
                p,
                pos2(r.left() + 36.0, r.top() + 20.0),
                Align2::LEFT_CENTER,
                &Txt::new(format!("{} {}", c.display_name, c.version), 13.0).weight(weight::MEDIUM),
                w - 52.0,
            );
            p.galley(pos2(r.left() + 36.0, r.top() + 34.0), galley, theme::FG_3);
        }
    }
    if solving {
        let (r, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
        feedback::spinner(ui.painter(), pos2(r.left() + 12.0, r.center().y), 8.0, 2.0, theme::ACCENT);
        widgets::text(ui.painter(), pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, &Txt::new("Solving again…", 12.5).color(theme::FG_2));
        return;
    }
    if let Some(e) = &plan.error {
        feedback::banner(ui, egui::Id::unique(("conflict-error", e)), Tone::Danger, &format!("No plan could be made: {e}"), false);
    }
    if !plan.steps.is_empty() && plan.blockers.is_empty() {
        art::caps(ui, if plan.steps.len() == 1 { "What changes" } else { "What changes, all at once" });
        for (i, s) in plan.steps.iter().enumerate() {
            if i > 0 {
                widgets::separator(ui);
            }
            step(ui, state, s);
        }
    }
    if !plan.blockers.is_empty() {
        art::caps(ui, "What blocks it");
        let mut removing: Option<String> = None;
        for b in &plan.blockers {
            let text = format!(
                "{} {} accepts {} {} only, and none of its versions in your repositories works with {}.",
                b.display_name, b.version, b.dependency, b.range, b.wanted
            );
            let galley = ui.painter().layout_job(Txt::new(&text, 12.0).color(theme::FG_2).line_height(18.0).job(w - 44.0 - 150.0));
            let (r, _) = ui.allocate_exact_size(vec2(w, (galley.size().y + 24.0).max(56.0)), Sense::hover());
            let p = ui.painter();
            widgets::frame(p, r, 14.0, theme::alpha(theme::DANGER, 0.08));
            widgets::icon(p, pos2(r.left() + 20.0, r.top() + 20.0), icons::X_CIRCLE, 14.0, theme::DANGER);
            p.galley(pos2(r.left() + 36.0, r.top() + 12.0), galley, theme::FG_2);
            let br = Rect::from_min_size(pos2(r.right() - 12.0 - 132.0, r.center().y - 16.0), vec2(132.0, 32.0));
            if b.removable {
                let resp = Button::new("Remove it too").icon(glyph::TRASH_SIMPLE).variant(Variant::Danger).size(Size::Sm).width(132.0).enabled(!state.running()).id_salt(("conflict-remove", &b.name)).show_at(ui, br);
                state.anchors.insert(format!("conflict-remove-{}", b.name), br);
                if widgets::activated(&resp.on_hover_text("Solves the plan again without it; nothing changes before you apply")) {
                    removing = Some(b.name.clone());
                }
            } else {
                widgets::text(ui.painter(), pos2(br.right(), br.center().y), Align2::RIGHT_CENTER, &Txt::new("Needed by others", 11.5).color(theme::FG_3));
            }
        }
        para(ui, Txt::new("Or keep the versions you have: Cancel changes nothing.", 12.0).color(theme::FG_3).line_height(18.0));
        if let Some(name) = removing {
            let v = state.conflict.as_mut().unwrap();
            let (path, requested, prerelease) = (v.path.clone(), v.plan.requested.clone(), v.plan.prerelease);
            let mut remove = v.plan.remove.clone();
            remove.push(name);
            v.solving = Job::spawn(&ctx, move || {
                orbiters_projects::runtime()
                    .and_then(|rt| rt.block_on(coherent::solve(&orbiters_projects::vcc_dir(), &path, &requested, &remove, prerelease)))
                    .map_err(|e| format!("{e:#}"))
            });
        }
    }
}

/// A package of the plan: its icon, name, from → to, and why.
fn step(ui: &mut Ui, state: &mut State, s: &orbiters_projects::updates::Step) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 50.0), Sense::hover());
    let p = ui.painter();
    let t = Rect::from_min_size(pos2(r.left(), r.center().y - 16.0), vec2(32.0, 32.0));
    state.icons_wanted.push(s.name.clone());
    art::package_icon(p, state.icons.get(&s.name), t, &s.name);
    let (label, color) = match (&s.from, &s.to) {
        (Some(f), Some(t)) => (format!("{f} → {t}"), theme::ACCENT),
        (None, Some(t)) => (format!("added · {t}"), theme::ACCENT),
        (Some(f), None) => (format!("{f} · removed"), theme::DANGER),
        (None, None) => (String::new(), theme::FG_3),
    };
    let version = Txt::new(&label, 12.0).weight(weight::MEDIUM).color(color);
    let vw = widgets::text_size(ui, &version).x;
    let x = t.right() + 12.0;
    let tw = r.right() - vw - 16.0 - x;
    let p = ui.painter();
    widgets::text_truncated(p, pos2(x, r.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(&s.display_name, 13.0).weight(weight::MEDIUM), tw);
    widgets::text_truncated(p, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(s.why.as_deref().unwrap_or(&s.name), 11.0).color(theme::FG_3), tw);
    widgets::text(p, pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, &version);
    if let Some(why) = &s.why {
        let _ = ui.interact(r, egui::Id::unique(("conflict-step", &s.name)), Sense::hover()).on_hover_text(why);
    }
}

pub(super) fn footer(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(v) = state.conflict.as_ref() else { return };
    let solved = v.plan.solved() && !v.solving.running();
    let n = v.plan.steps.len();
    let label = if n == 1 { "Apply the change".to_string() } else { format!("Update {n} packages together") };
    let b = Button::new(&label).icon(glyph::ARROW_UP).variant(Variant::Primary).enabled(solved && !state.running()).id_salt("conflict-apply");
    if widgets::activated(&b.show(ui)) {
        let (path, plan) = (v.path.clone(), v.plan.clone());
        let change = vpm::Change::Plan { install: plan.install.clone(), remove: plan.remove.clone(), prerelease: plan.prerelease };
        state.last_change = Some((path.clone(), change.clone()));
        state.close_modal(&ctx);
        state.task(&ctx, &format!("Updating {n} packages…"), move || {
            orbiters_projects::runtime()?.block_on(vpm::change(&orbiters_projects::vcc_dir(), &path, change))?;
            Ok(Outcome::Changed(format!("{n} packages updated to versions that work together")))
        });
    }
    if widgets::activated(&Button::new("Cancel").variant(Variant::Ghost).id_salt("conflict-cancel").show(ui)) {
        state.close_modal(&ctx);
        state.conflict = None;
    }
}
