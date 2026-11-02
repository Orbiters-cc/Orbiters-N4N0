//! The panel's Packages card: each installed package with its version list, its update (Update to x.y.z: a version
//! the project can take as it is; Update to x.y.z…: a newer one needing other changes, whose plan the conflict dialog
//! shows first) and Remove; Update all in its heading, opening the plan of every update that goes together (what
//! changes, from → to; what is left out and why) above the rows. Updates come from `orbiters_projects::updates`
//! (checked after each look at the project's packages; releases, or pre-releases too with the switch). Every change
//! goes through `panel::package_action`, the version list's path.
use super::art::{self, glyph};
use super::panel::{package_action, package_row, row_skeletons, versions};
use super::state::State;
use egui::{pos2, vec2, Align2, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::{discovery::Project, updates, vpm};
use orbiters_ui::popup::{self, Item};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, icons};

pub(super) fn packages(ui: &mut Ui, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let installed: Vec<vpm::Package> = state.view.packages.iter().filter(|p| p.installed.is_some()).cloned().collect();
    let count = (!installed.is_empty()).then(|| installed.len().to_string());
    let loading = state.view_loading() && installed.is_empty();
    let busy = state.running();
    let ups = state.updates_of(&project.path).cloned().unwrap_or_default();
    let all_n = ups.all.targets.len();
    let mut restore = false;
    let mut open_plan = false;
    let mut update_all_at = None;
    art::card(
        ui,
        "packages",
        glyph::PACKAGE,
        "Packages",
        count,
        |ui, slot| {
            let enabled = !busy && !project.missing;
            let b = Button::new("Restore").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Ghost).size(Size::Sm).enabled(enabled).id_salt("restore");
            let restore_hint = "Downloads the packages the project lists but does not have (VCC's Resolve)";
            // Update all and Restore in what the title leaves: Update all's count, then Restore's word go first.
            let full = format!("Update all · {all_n}");
            let short = "Update all".to_string();
            let u_w = |label: &str| Button::new(label).icon(glyph::ARROW_UP).size(Size::Sm).desired_width(ui);
            let room = slot.width();
            let bw = b.desired_width(ui);
            let (label, restore_icon) = if all_n == 0 {
                (full.clone(), false)
            } else if u_w(&full) + 8.0 + bw <= room {
                (full.clone(), false)
            } else if u_w(&short) + 8.0 + bw <= room {
                (short.clone(), false)
            } else {
                (short.clone(), true)
            };
            let r = if restore_icon {
                let r = Rect::from_min_size(pos2(slot.right() - 32.0, slot.center().y - 16.0), vec2(32.0, 32.0));
                let ib = IconButton::new(icons::ARROW_COUNTER_CLOCKWISE, restore_hint).size(32.0, 15.0).id_salt("restore-icon");
                restore = enabled && widgets::activated(&ib.show_at(ui, r));
                r
            } else {
                let r = Rect::from_min_size(pos2(slot.right() - bw, slot.center().y - 16.0), vec2(bw, 32.0));
                restore = widgets::activated(&b.show_at(ui, r).on_hover_text(restore_hint));
                r
            };
            if all_n > 0 {
                let u = Button::new(&label).icon(glyph::ARROW_UP).variant(Variant::Primary).size(Size::Sm).enabled(enabled).id_salt("update-all");
                let uw = u.desired_width(ui);
                let ur = Rect::from_min_size(pos2(r.left() - 8.0 - uw, slot.center().y - 16.0), vec2(uw, 32.0));
                update_all_at = Some(ur);
                if widgets::activated(&u.show_at(ui, ur).on_hover_text("Shows what changes, then updates them at once")) {
                    open_plan = true;
                }
            }
        },
        |ui| {
            if loading {
                row_skeletons(ui, 3);
                return;
            }
            if installed.is_empty() {
                widgets::text(
                    ui.painter(),
                    ui.cursor().min + vec2(0.0, 10.0),
                    Align2::LEFT_CENTER,
                    &Txt::new("No VPM package in this project yet.", 12.5).color(theme::FG_3),
                );
                ui.add_space(22.0);
            }
            if state.plan_open && all_n > 0 {
                plan(ui, state, project, &ups);
            }
            for (i, pkg) in installed.iter().enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                row(ui, state, project, pkg, &ups, i);
            }
            widgets::separator(ui);
            widgets::field(ui, "Pre-release versions", Some("Beta versions in the version lists and updates"), 48.0, |ui| {
                widgets::toggle(ui, "prerelease", &mut state.prerelease);
            });
        },
    );
    if restore {
        package_action(state, &ctx, project, vpm::Change::Resolve, "Restoring missing packages…", "");
    }
    if open_plan {
        state.plan_open = !state.plan_open;
    }
    if let Some(r) = update_all_at {
        state.anchors.insert("update-all".into(), r);
    }
}

/// One installed package: its update button (when there is one), its versions, Remove.
fn row(ui: &mut Ui, state: &mut State, project: &Project, pkg: &vpm::Package, ups: &updates::Updates, i: usize) {
    let ctx = ui.ctx().clone();
    let busy = state.running();
    let current = pkg.installed.clone().unwrap_or_default();
    let list: Vec<String> = versions(pkg, state.prerelease).into_iter().cloned().collect();
    let working = state.busy_package.as_deref() == Some(pkg.name.as_str());
    state.icons_wanted.push(pkg.name.clone());
    let icon = state.icons.get(&pkg.name).cloned();
    let update = ups.of(&pkg.name).cloned();
    let held = if update.is_none() { ups.held.iter().find(|h| h.name == pkg.name).cloned() } else { None };
    let (label, hint, to) = match (&update, &held) {
        (Some(u), _) => {
            let also: Vec<String> = u.also().map(|s| format!("{} to {}", s.display_name, s.to.as_deref().unwrap_or("removed"))).collect();
            let hint = if also.is_empty() { format!("Updates {} from {} to {}", pkg.display_name, u.from, u.to) } else { format!("Also updates {}", also.join(", ")) };
            (Some(format!("Update to {}", u.to)), hint, u.to.clone())
        }
        (None, Some(h)) => (Some(format!("Update to {}…", h.version)), format!("{} {} {}: shows what has to change with it first", pkg.display_name, h.version, h.why), h.version.clone()),
        _ => (None, String::new(), String::new()),
    };
    let sub = label.as_ref().map(|l| (l.clone(), if update.is_some() { theme::ACCENT } else { theme::WARN }, hint.clone()));
    let mut update_pressed = None;
    package_row_with(ui, icon.as_ref(), &pkg.name, &pkg.display_name, 108.0 + 8.0 + 32.0, sub, !busy && !working, &mut update_pressed, |ui, c| {
        let dd = Rect::from_min_size(pos2(c.left(), c.center().y - 15.0), vec2(108.0, 30.0));
        let rm = Rect::from_min_size(pos2(c.right() - 32.0, c.center().y - 16.0), vec2(32.0, 32.0));
        if i == 0 {
            state.anchors.insert("version-0".into(), dd);
        }
        if working {
            feedback::spinner(ui.painter(), dd.center(), 8.0, 2.0, theme::ACCENT);
        } else {
            let items: Vec<Item> = list.iter().map(|v| if v.contains('-') { Item::new(v).detail("Pre-release") } else { Item::new(v) }).collect();
            let selected = list.iter().position(|v| *v == current);
            if let Some(k) = popup::dropdown(ui, ("version", &pkg.name), dd, &current, &items, selected, !busy) {
                if list[k] != current {
                    state.anchors.insert("conflict-origin".into(), dd);
                    let change = vpm::Change::Install { names: vec![(pkg.name.clone(), Some(list[k].clone()))], prerelease: state.prerelease };
                    package_action(state, &ctx, project, change, &format!("Changing {} to {}…", pkg.display_name, list[k]), &pkg.name);
                }
            }
        }
        let rb = IconButton::new(glyph::TRASH_SIMPLE, "Remove the package").size(32.0, 15.0).danger().id_salt(("remove", &pkg.name));
        if !busy && widgets::activated(&rb.show_at(ui, rm)) {
            package_action(state, &ctx, project, vpm::Change::Remove(pkg.name.clone()), &format!("Removing {}…", pkg.display_name), &pkg.name);
        }
    });
    if let Some((r, pressed)) = update_pressed {
        state.anchors.insert(format!("update-{}", pkg.name), r);
        if pressed {
            state.anchors.insert("conflict-origin".into(), r);
            let change = vpm::Change::Install { names: vec![(pkg.name.clone(), Some(to.clone()))], prerelease: state.prerelease };
            package_action(state, &ctx, project, change, &format!("Updating {} to {to}…", pkg.display_name), &pkg.name);
        }
    }
}

/// [`package_row`] whose second line, in place of the package's id, is its update (`sub`: label, colour, hint): a
/// small button under the name, so the name keeps its width and the row its height. `pressed` gets where it was drawn
/// and whether it was pressed.
#[allow(clippy::too_many_arguments)]
fn package_row_with(
    ui: &mut Ui,
    icon: Option<&super::state::PackageIcon>,
    name: &str,
    display: &str,
    control_w: f32,
    sub: Option<(String, egui::Color32, String)>,
    enabled: bool,
    pressed: &mut Option<(Rect, bool)>,
    control: impl FnOnce(&mut Ui, Rect),
) {
    let Some((label, color, hint)) = sub else {
        package_row(ui, icon, name, display, control_w, control);
        return;
    };
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
    let t = Rect::from_min_size(pos2(r.left(), r.center().y - 17.0), vec2(34.0, 34.0));
    art::package_icon(ui.painter(), icon, t, name);
    let x = t.right() + 12.0;
    let tw = r.right() - control_w - 10.0 - x;
    let title = Txt::new(display, 13.5).weight(weight::MEDIUM);
    widgets::text_truncated(ui.painter(), pos2(x, r.center().y - 9.0), Align2::LEFT_CENTER, &title, tw);
    // The update: an arrow and its words on a tint that deepens on hover.
    let words = Txt::new(&label, 11.5).weight(weight::SEMIBOLD).color(color);
    let lw = (widgets::text_size(ui, &words).x + 30.0).min(tw.max(40.0));
    let br = Rect::from_min_size(pos2(x - 4.0, r.center().y + 2.0), vec2(lw, 20.0));
    let id = egui::Id::unique(("package-update", name));
    let resp = ui.interact(br, id, if enabled { Sense::click() } else { Sense::hover() }).on_hover_text(&hint);
    let hover = orbiters_ui::anim::toggle(ui.ctx(), id.with("hover"), enabled && resp.hovered(), orbiters_ui::anim::Spring::SNAPPY);
    let p = ui.painter();
    p.rect_filled(br, theme::radius(10.0), theme::alpha(color, 0.12 + 0.12 * hover));
    widgets::icon(p, pos2(br.left() + 12.0, br.center().y), glyph::ARROW_UP, 11.0, color);
    widgets::text_truncated(p, pos2(br.left() + 22.0, br.center().y), Align2::LEFT_CENTER, &words, lw - 28.0);
    if enabled {
        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    *pressed = Some((br, enabled && widgets::activated(&resp)));
    if widgets::text_size(ui, &title).x > tw {
        let _ = ui.interact(Rect::from_min_max(pos2(x, r.top()), pos2(x + tw, r.center().y)), egui::Id::unique(("package-name", name)), Sense::hover()).on_hover_text(format!("{display} · {name}"));
    }
    let c = Rect::from_min_max(pos2(r.right() - control_w, r.top()), r.max);
    control(ui, c);
}

/// Update all's plan: every package that changes, what is left out and why, and the button applying it.
fn plan(ui: &mut Ui, state: &mut State, project: &Project, ups: &updates::Updates) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = Rect::from_min_size(top + vec2(14.0, 12.0), vec2(w - 28.0, 100_000.0));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("update-plan"));
    cu.spacing_mut().item_spacing = vec2(8.0, 4.0);
    let n = ups.all.targets.len();
    let (tr, _) = cu.allocate_exact_size(vec2(inner.width(), 20.0), Sense::hover());
    let title = if n == 1 { "Update 1 package".to_string() } else { format!("Update {n} packages together") };
    widgets::text(cu.painter(), tr.left_center(), Align2::LEFT_CENTER, &Txt::new(title, 13.0).weight(weight::SEMIBOLD));
    for s in &ups.all.steps {
        let (r, _) = cu.allocate_exact_size(vec2(inner.width(), 22.0), Sense::hover());
        let p = cu.painter();
        let label = match (&s.from, &s.to) {
            (Some(f), Some(t)) => format!("{f} → {t}"),
            (None, Some(t)) => format!("added · {t}"),
            (Some(f), None) => format!("{f} · removed"),
            _ => String::new(),
        };
        let v = Txt::new(&label, 12.0).weight(weight::MEDIUM).color(if s.to.is_some() { theme::ACCENT } else { theme::DANGER });
        let vr = widgets::text(p, pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, &v);
        widgets::icon(p, pos2(r.left() + 6.0, r.center().y), glyph::ARROW_UP, 11.0, theme::FG_3);
        widgets::text_truncated(p, pos2(r.left() + 20.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(&s.display_name, 12.5), vr.left() - r.left() - 30.0);
    }
    for name in &ups.all.left_out {
        let Some(u) = ups.of(name) else { continue };
        let why = if u.sdk { "the VRChat SDK is updated from its own row" } else { "does not go together with the others: update it on its own" };
        let galley = cu.painter().layout_job(Txt::new(format!("{} {}: {why}", u.display_name, u.to), 11.5).color(theme::FG_3).line_height(16.0).job(inner.width() - 20.0));
        let (r, _) = cu.allocate_exact_size(vec2(inner.width(), galley.size().y), Sense::hover());
        widgets::icon(cu.painter(), pos2(r.left() + 6.0, r.top() + 8.0), icons::INFO, 11.0, theme::FG_4);
        cu.painter().galley(pos2(r.left() + 20.0, r.top()), galley, theme::FG_3);
    }
    cu.add_space(6.0);
    let mut apply = false;
    let mut cancel = false;
    let running = state.running();
    cu.horizontal(|ui| {
        let label = if n == 1 { "Update it".to_string() } else { format!("Update {n} packages") };
        let b = Button::new(&label).icon(glyph::ARROW_UP).variant(Variant::Primary).size(Size::Sm).enabled(!running).id_salt("update-all-apply").show(ui);
        state.anchors.insert("update-all-apply".into(), b.rect);
        apply = widgets::activated(&b);
        cancel = widgets::activated(&Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).id_salt("update-all-cancel").show(ui));
    });
    let h = cu.min_rect().height() + 12.0 + 12.0;
    let rect = Rect::from_min_size(top, vec2(w, h));
    ui.painter().set(bg, egui::Shape::rect_filled(rect, theme::radius(14.0), theme::alpha(theme::ACCENT, 0.07)));
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.add_space(4.0);
    if apply {
        let names = ups.all.targets.iter().map(|(n, v)| (n.clone(), Some(v.clone()))).collect();
        let origin = state.anchor("update-all-apply");
        state.anchors.insert("conflict-origin".into(), origin);
        let change = vpm::Change::Install { names, prerelease: state.prerelease };
        package_action(state, &ctx, project, change, &format!("Updating {n} packages…"), "");
    }
    if cancel {
        state.plan_open = false;
    }
}
