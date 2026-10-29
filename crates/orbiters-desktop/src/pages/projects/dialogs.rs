//! The page's dialogs, each growing out of what opened it ([`orbiters_ui::dialog`]): the package repositories, a new
//! project (from a template or a copy), and Capture's way of putting N4N0 on an avatar (the avatar, the project copy
//! when there are several, then the setup's steps until Unity confirms). The steps of one flow stay in one dialog,
//! which follows their height and slides each step in.

use super::art::{self, glyph};
use super::state::{server, Modal, Outcome, State};
use super::ui::errors;
use crate::pages::{Cx, Go};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::{avatars::Avatar, discovery::Project, network, setup, templates, vpm};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::dialog::{Dialog, Part};
use orbiters_ui::layout;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Size, Tone, Variant};
use orbiters_ui::{feedback, format, icons};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn modals(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let Some(modal) = state.modal.clone() else { return };
    let ctx = ui.ctx().clone();
    let id = state.dialog_id();
    let nano = state.flow.as_ref().map(|f| !f.packages_only).or(state.pending.as_ref().map(|p| p.2)).unwrap_or(true);
    let names = state.flow.as_ref().map(|f| (f.avatar.name.clone(), f.project.name.clone())).or(state.pending.as_ref().map(|p| (p.1.clone(), p.0.clone())));
    let (glyph, title, subtitle, width) = match &modal {
        Modal::Repos => (icons::STACK, "Package repositories".to_string(), "Shared with VCC and ALCOM: what you add here, they list too".to_string(), 620.0),
        Modal::New => (glyph::FOLDER_PLUS, "New Unity project".into(), "From a VRChat template, or a copy of one of your projects".into(), 640.0),
        Modal::Avatars => {
            (icons::N4N0_MARK, "Add N4N0 to your avatar".into(), "Pick the avatar you capture with: Unity asks you to confirm before anything changes".into(), 720.0)
        }
        Modal::ChooseProject(id) => {
            let name = state.copies(id).first().map(|(_, a)| a.name.clone()).unwrap_or_else(|| "This avatar".into());
            (icons::N4N0_MARK, "Which copy gets N4N0?".into(), format!("{name} is saved in more than one project or scene"), 620.0)
        }
        Modal::Setup => (
            if nano { icons::N4N0_MARK } else { icons::PERSON },
            if nano { "Add N4N0 to your avatar".into() } else { "Add My Avatar".into() },
            names.map(|(a, p)| format!("{a} in {p}")).unwrap_or_default(),
            580.0,
        ),
    };
    let step = match &modal {
        Modal::Repos => 0,
        Modal::New => 1,
        Modal::Avatars => 2,
        Modal::ChooseProject(_) => 3,
        Modal::Setup => 4,
    };
    let creating = modal == Modal::New && state.running();
    let resp = Dialog::new(id, state.modal_origin, &title)
        .origin_fill(state.modal_fill)
        .glyph(glyph)
        .subtitle(&subtitle)
        .width(width)
        .footer(modal != Modal::Repos)
        .busy(creating)
        .show(&ctx, |ui, part| match part {
            Part::Body => slide(ui, id.with(("step", step)), |ui| match &modal {
                Modal::Repos => repos(ui, cx, state),
                Modal::New => new_project(ui, state),
                Modal::Avatars => avatar_picker(ui, cx, state),
                Modal::ChooseProject(avatar) => project_choice(ui, state, avatar),
                Modal::Setup => setup_steps(ui, state),
            }),
            Part::Footer => match &modal {
                Modal::Repos => {}
                Modal::New => new_footer(ui, state),
                Modal::Avatars => avatar_footer(ui, cx, state),
                Modal::ChooseProject(avatar) => choice_footer(ui, cx, state, avatar),
                Modal::Setup => setup_footer(ui, cx, state),
            },
        });
    if resp.closed {
        state.modal = None;
    }
}

/// A step of a dialog sliding in from the right and fading in the first time it shows.
fn slide(ui: &mut Ui, id: Id, f: impl FnOnce(&mut Ui)) {
    let k = anim::spring_from(ui.ctx(), id, [0.0], [1.0], Spring::SOFT)[0];
    let r = ui.available_rect_before_wrap();
    let mut c = ui.new_child(UiBuilder::new().max_rect(r.translate(vec2(28.0 * (1.0 - k), 0.0))).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(id));
    c.spacing_mut().item_spacing = vec2(0.0, 12.0);
    c.multiply_opacity(k.clamp(0.0, 1.0));
    f(&mut c);
    let size = vec2(r.width(), c.min_rect().height());
    ui.allocate_exact_size(size, Sense::hover());
}

/// A selectable row or tile: hover, press and the selection ring; true when pressed.
fn selectable(ui: &mut Ui, id: Id, rect: Rect, radius: f32, selected: bool) -> (egui::Response, f32, f32) {
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(ui.ctx(), id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.025 * press));
    let sel = art::selection(ui.painter(), id.with("ring"), r, radius, selected);
    widgets::frame(ui.painter(), r, radius, theme::mix(theme::surface_soft(), theme::INK_750, hover.max(sel * 0.7)));
    (resp, hover, sel)
}

/// A switch drawn in `rect`; true when it changed.
fn switch(ui: &mut Ui, salt: impl egui::AsIdSalt, rect: Rect, on: &mut bool, enabled: bool) -> bool {
    let mut su = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("switch-slot", &salt)));
    if !enabled {
        su.disable();
    }
    widgets::toggle(&mut su, salt, on)
}

// Package repositories.

fn repos(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    errors(ui, state, "repos");
    let busy = state.running();
    let w = ui.available_width();
    art::caps(ui, "Your repositories");
    if state.view.repos.is_empty() && state.view_loading() {
        for _ in 0..2 {
            let (r, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
            feedback::shimmer(ui.painter(), r, 14.0);
        }
    }
    for r in state.view.repos.clone() {
        let (row, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
        let p = ui.painter();
        widgets::frame(p, row, 14.0, theme::surface_soft());
        let t = Rect::from_min_size(pos2(row.left() + 12.0, row.center().y - 17.0), vec2(34.0, 34.0));
        art::tile(p, t, icons::STACK, if r.enabled { theme::ACCENT } else { theme::FG_4 });
        let x = t.right() + 12.0;
        let tw = row.right() - 12.0 - 48.0 - 12.0 - 90.0 - x;
        widgets::text_truncated(
            p,
            pos2(x, row.center().y - 8.0),
            Align2::LEFT_CENTER,
            &Txt::new(&r.name, 13.5).weight(weight::MEDIUM).color(if r.enabled { theme::FG } else { theme::FG_3 }),
            tw,
        );
        widgets::text_truncated(p, pos2(x, row.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(&r.url, 11.0).color(theme::FG_3), tw);
        let sw = Rect::from_min_size(pos2(row.right() - 12.0 - 48.0, row.center().y - 14.0), vec2(48.0, 28.0));
        let mut enabled = r.enabled;
        if switch(ui, ("repo-enabled", &r.id), sw, &mut enabled, !busy) {
            if let Some(repo) = state.view.repos.iter_mut().find(|repo| repo.id == r.id) {
                repo.enabled = enabled;
            }
            let id = r.id.clone();
            state.task(&ctx, if enabled { "Turning the repository on…" } else { "Turning the repository off…" }, move || {
                orbiters_projects::runtime()?.block_on(vpm::set_enabled(&orbiters_projects::vcc_dir(), &id, enabled))?;
                Ok(Outcome::Changed(if enabled { "Repository on: its packages are listed again" } else { "Repository off: it stays saved" }.into()))
            });
        }
        let slot = Rect::from_min_size(pos2(sw.left() - 12.0 - 90.0, row.center().y - 16.0), vec2(90.0, 32.0));
        if r.builtin {
            let t = Txt::new("Built in", 11.0).color(theme::FG_3);
            let tw = widgets::text_size(ui, &t).x + 18.0;
            let pr = Rect::from_min_size(pos2(slot.right() - tw, row.center().y - 10.0), vec2(tw, 20.0));
            ui.painter().rect_filled(pr, theme::radius(10.0), theme::white(0.05));
            widgets::text(ui.painter(), pr.center(), Align2::CENTER_CENTER, &t);
        } else {
            let b = IconButton::new(glyph::TRASH_SIMPLE, "Remove the repository").size(32.0, 15.0).danger().id_salt(("repo-remove", &r.id));
            if !busy && widgets::activated(&b.show_at(ui, Rect::from_min_size(pos2(slot.right() - 32.0, slot.top()), vec2(32.0, 32.0)))) {
                let id = r.id.clone();
                state.task(&ctx, "Removing the repository…", move || {
                    orbiters_projects::runtime()?.block_on(vpm::remove_repo(&orbiters_projects::vcc_dir(), &id))?;
                    Ok(Outcome::Changed("Repository removed".into()))
                });
            }
        }
    }

    ui.add_space(6.0);
    art::caps(ui, "Add a repository");
    let add_b = Button::new("Add").icon(icons::PLUS).variant(Variant::Primary).enabled(!state.repo_url.trim().is_empty() && !busy).id_salt("repo-add");
    let bw = add_b.desired_width(ui);
    let (row, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    {
        let mut fu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(row.min, vec2(w - bw - 8.0, 36.0))).id_salt("repo-url-slot"));
        widgets::text_field(&mut fu, "repo-url", &mut state.repo_url, "https://example.com/index.json", w - bw - 8.0, false);
    }
    let add = widgets::activated(&add_b.show_at(ui, Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, 36.0))));
    // Request headers, for private repositories: a disclosure that opens with a spring.
    let open_id = Id::unique("repo-headers-open");
    let mut open = ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(!state.repo_headers.is_empty());
    let (hr, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let hresp = ui.interact(hr, open_id.with("row"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if widgets::activated(&hresp) {
        open = !open;
        ctx.data_mut(|d| d.insert_temp(open_id, open));
    }
    let k = anim::toggle(&ctx, open_id.with("k"), open, Spring::SOFT);
    let fg = if hresp.hovered() { theme::FG } else { theme::FG_2 };
    let c = pos2(hr.left() + 7.0, hr.center().y);
    let s = 3.5;
    let turn = k.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    let rot = |v: egui::Vec2| vec2(v.x * turn.cos() - v.y * turn.sin(), v.x * turn.sin() + v.y * turn.cos());
    ui.painter().line(vec![c + rot(vec2(-s * 0.5, -s)), c + rot(vec2(s * 0.5, 0.0)), c + rot(vec2(-s * 0.5, s))], egui::Stroke::new(1.6, fg));
    widgets::text(
        ui.painter(),
        pos2(hr.left() + 20.0, hr.center().y),
        Align2::LEFT_CENTER,
        &Txt::new("Request headers, for a private repository", 12.5).color(fg),
    );
    if k > 0.01 {
        let full = 96.0;
        let (area, _) = ui.allocate_exact_size(vec2(w, full * k.clamp(0.0, 1.0)), Sense::hover());
        let mut hu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(area.min, vec2(w, full))).id_salt("repo-headers"));
        hu.set_clip_rect(area.intersect(ui.clip_rect()));
        hu.multiply_opacity(k.clamp(0.0, 1.0));
        let well = Rect::from_min_size(area.min, vec2(w, 88.0));
        hu.painter().rect_filled(well, theme::radius(12.0), theme::INK_750);
        let edit = egui::TextEdit::multiline(&mut state.repo_headers)
            .id(Id::unique("repo-headers-text"))
            .frame(egui::Frame::NONE)
            .hint_text(egui::RichText::new("One per line, e.g. Authorization: Bearer …").color(theme::FG_4))
            .font(egui::FontId::new(12.0, egui::FontFamily::Monospace))
            .text_color(theme::FG)
            .desired_width(w - 24.0)
            .desired_rows(4);
        hu.put(well.shrink2(vec2(12.0, 10.0)), edit);
    }
    if add {
        let url = state.repo_url.trim().to_string();
        let mut headers = BTreeMap::new();
        let mut valid = true;
        for line in state.repo_headers.lines().filter(|l| !l.trim().is_empty()) {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_string(), v.trim().to_string());
            } else {
                state.error = Some("Each header needs a name and a value separated by a colon.".into());
                valid = false;
            }
        }
        if valid {
            add_repo(state, &ctx, url, headers);
            state.repo_headers.clear();
            state.repo_url.clear();
        }
    }

    ui.add_space(6.0);
    art::caps(ui, "Suggested");
    let mut seen = std::collections::HashSet::new();
    let list: Vec<network::Listing> =
        state.catalog.known.iter().chain(&state.catalog.public).filter(|l| seen.insert(l.url.trim_end_matches('/').to_string())).cloned().collect();
    if list.is_empty() {
        if state.catalog_job.running() {
            for _ in 0..2 {
                let (r, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
                feedback::shimmer(ui.painter(), r, 14.0);
            }
        } else if state.catalog.errors.is_empty() {
            widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 10.0), Align2::LEFT_CENTER, &Txt::new("No suggestion right now.", 12.5).color(theme::FG_3));
            ui.add_space(20.0);
        }
    }
    for l in list {
        let (row, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
        let p = ui.painter();
        widgets::frame(p, row, 14.0, theme::surface_soft());
        let t = Rect::from_min_size(pos2(row.left() + 12.0, row.center().y - 17.0), vec2(34.0, 34.0));
        art::tile(p, t, glyph::PACKAGE, art::package_tint(&l.name.to_lowercase()));
        let x = t.right() + 12.0;
        let added = state.view.repos.iter().any(|repo| repo.url.trim_end_matches('/') == l.url.trim_end_matches('/'));
        let b = Button::new(if added { "Added" } else { "Add" })
            .icon(if added { icons::CHECK } else { icons::PLUS })
            .size(Size::Sm)
            .enabled(!added && !busy)
            .id_salt(("suggested", &l.url));
        let bw = b.desired_width(ui);
        let tw = row.right() - 12.0 - bw - 12.0 - x;
        widgets::text_truncated(p, pos2(x, row.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(&l.name, 13.5).weight(weight::MEDIUM), tw);
        widgets::text_truncated(
            p,
            pos2(x, row.center().y + 10.0),
            Align2::LEFT_CENTER,
            &Txt::new(if l.description.is_empty() { &l.url } else { &l.description }, 11.5).color(theme::FG_3),
            tw,
        );
        if widgets::activated(&b.show_at(ui, Rect::from_min_size(pos2(row.right() - 12.0 - bw, row.center().y - 16.0), vec2(bw, 32.0)))) {
            add_repo(state, &ctx, l.url.clone(), BTreeMap::new());
        }
    }
    if !state.catalog.errors.is_empty() {
        feedback::banner(ui, Id::unique("repos-catalog-error"), Tone::Warn, "Orbiters' suggestions could not be loaded.", false);
        let b = Button::new("Try again")
            .icon(icons::ARROW_COUNTER_CLOCKWISE)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(!state.catalog_job.running())
            .id_salt("catalog-retry");
        let bw = b.desired_width(ui);
        let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        if widgets::activated(&b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
            state.catalog(&ctx, server(cx));
        }
    }
    if let Some(label) = state.acting() {
        let label = label.to_string();
        progress_line(ui, &label);
    }
}

fn add_repo(state: &mut State, ctx: &egui::Context, url: String, headers: BTreeMap<String, String>) {
    state.task(ctx, "Adding the repository…", move || {
        orbiters_projects::runtime()?.block_on(vpm::add_repo(&orbiters_projects::vcc_dir(), &url, headers))?;
        Ok(Outcome::Changed("Repository added: its packages are listed now".into()))
    });
}

/// A spinner and what runs, inside a dialog.
fn progress_line(ui: &mut Ui, label: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    feedback::spinner(ui.painter(), pos2(r.left() + 9.0, r.center().y), 7.0, 2.0, theme::ACCENT);
    widgets::text(ui.painter(), pos2(r.left() + 26.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(label, 12.5).color(theme::FG_2));
}

// New project.

fn new_project(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    errors(ui, state, "new");
    let w = ui.available_width();
    let busy = state.running();
    if busy {
        creating(ui, state);
    }
    if busy {
        ui.disable();
    }
    let options = [SegOption::new(false, "From a template"), SegOption::new(true, "Copy a project")];
    widgets::segmented(ui, "new-mode", &mut state.duplicate, &options, false);
    let projects: Vec<Project> = state.scan.projects.iter().filter(|p| p.registered && !p.missing).cloned().collect();
    if state.duplicate {
        art::caps(ui, "Project to copy");
        if projects.is_empty() {
            feedback::banner(ui, Id::unique("new-no-projects"), Tone::Info, "Add a project to your list first: it can then be copied here.", false);
        }
        for (i, p) in projects.iter().enumerate() {
            let (r, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
            let (resp, _, _) = selectable(ui, Id::unique(("copy-source", &p.path)), r, 14.0, state.duplicate_index == i);
            let pt = ui.painter();
            art::cover(pt, state, p, Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 22.0), vec2(44.0, 44.0)), [10.0; 4], 1.0);
            let x = r.left() + 66.0;
            widgets::text_truncated(pt, pos2(x, r.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(&p.name, 13.5).weight(weight::MEDIUM), w - 90.0);
            let meta = format!(
                "Unity {} · {}",
                p.unity,
                if p.avatars.is_empty() {
                    p.kind.label().to_string()
                } else {
                    format!("{} avatar{}", p.avatars.len(), if p.avatars.len() == 1 { "" } else { "s" })
                }
            );
            widgets::text_truncated(pt, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(meta, 11.5).color(theme::FG_3), w - 90.0);
            if widgets::activated(&resp) {
                state.duplicate_index = i;
                if state.new_name.is_empty() || state.new_name.ends_with(" copy") || state.new_name == "My new avatar" {
                    state.new_name = format!("{} copy", p.name);
                }
            }
        }
        note(ui, "Copies Assets, Packages and the project's settings. Generated files (Library, Temp) and Git history stay behind.");
    } else {
        art::caps(ui, "Template");
        if state.templates.is_empty() {
            feedback::banner(ui, Id::unique("new-no-templates"), Tone::Info, "No VCC template was found in Templates or VRCTemplates.", false);
        }
        // With Creator Companion installed, its covers head the cards (as in VCC); without, an icon tile.
        let covers = state.templates.iter().any(|t| t.banner.is_some());
        let (cols, tw) = layout::columns(w, 220.0, 10.0);
        let cover_h = ((tw - 12.0) / 8.0).round();
        let card_h = if covers { 6.0 + cover_h + 52.0 } else { 72.0 };
        let rows = state.templates.len().div_ceil(cols);
        let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + 10.0) - 10.0), Sense::hover());
        for (i, t) in state.templates.clone().iter().enumerate() {
            let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (tw + 10.0), (i / cols) as f32 * (card_h + 10.0)), vec2(tw, card_h));
            let selected = state.template_index == i;
            let id = Id::unique(("template", &t.name));
            let (resp, hover, sel) = selectable(ui, id, r, 16.0, selected);
            let lower = t.name.to_lowercase();
            let (g, tint) = if lower.contains("world") {
                (icons::GLOBE, theme::VIOLET)
            } else if lower.contains("avatar") {
                (icons::PERSON_ARMS_SPREAD, theme::BLUE)
            } else {
                (icons::CUBE, theme::CYAN)
            };
            let pt = ui.painter();
            let sub = match (t.unity.is_empty(), t.builtin) {
                (false, true) => format!("Unity {} · VRChat's", t.unity),
                (false, false) => format!("Unity {}", t.unity),
                (true, _) => "Your template".into(),
            };
            if covers {
                let cover = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(tw - 12.0, cover_h));
                template_cover(pt, state, t.banner.as_ref(), cover, id, hover, g, tint);
                let x = r.left() + 14.0;
                let tw = r.width() - 28.0;
                widgets::text_truncated(pt, pos2(x, cover.bottom() + 17.0), Align2::LEFT_CENTER, &Txt::new(&t.name, 13.5).weight(weight::MEDIUM), tw);
                widgets::text_truncated(pt, pos2(x, cover.bottom() + 35.0), Align2::LEFT_CENTER, &Txt::new(sub, 11.5).color(theme::FG_3), tw);
                art::check(pt, pos2(cover.right() - 13.0, cover.center().y), sel);
            } else {
                art::tile(pt, Rect::from_min_size(pos2(r.left() + 14.0, r.center().y - 20.0), vec2(40.0, 40.0)), g, tint);
                let x = r.left() + 66.0;
                widgets::text_truncated(
                    pt,
                    pos2(x, r.center().y - 9.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(&t.name, 13.5).weight(weight::MEDIUM),
                    r.right() - x - 30.0,
                );
                widgets::text_truncated(pt, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(sub, 11.5).color(theme::FG_3), r.right() - x - 30.0);
                art::check(pt, pos2(r.right() - 16.0, r.top() + 16.0), sel);
            }
            if widgets::activated(&resp) {
                state.template_index = i;
            }
        }
    }

    art::caps(ui, "Name");
    widgets::text_field(ui, "new-name", &mut state.new_name, "My avatar", w, false);
    if let Err(e) = templates::validate_name(&state.new_name) {
        if !state.new_name.is_empty() {
            inline_warning(ui, &e.to_string());
        }
    }
    art::caps(ui, "Location");
    let (row, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    {
        let mut fu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(row.min, vec2(w - 44.0, 36.0))).id_salt("new-location-slot"));
        widgets::text_field(&mut fu, "new-location", &mut state.new_location, "Choose a folder", w - 44.0, false);
    }
    let browse = IconButton::new(icons::FOLDER_OPEN, "Choose a folder").size(36.0, 16.0).backdrop(theme::control()).id_salt("new-browse");
    if !busy && widgets::activated(&browse.show_at(ui, Rect::from_min_size(pos2(row.right() - 36.0, row.top()), vec2(36.0, 36.0)))) {
        state.task(&ctx, "Choose a location…", move || Ok(Outcome::Picked(rfd::FileDialog::new().pick_folder())));
    }
    let template = state.templates.get(state.template_index).cloned();
    if !state.duplicate && template.as_ref().is_some_and(|t| t.unity.is_empty()) {
        art::caps(ui, "Unity version");
        widgets::text_field(ui, "new-unity", &mut state.new_unity, "The exact editor version installed, e.g. 2022.3.22f1", w, false);
    }
    if !state.new_name.is_empty() && !state.new_location.is_empty() {
        // As the location is written (forward or back slashes).
        let sep = if state.new_location.contains('\\') { '\\' } else { '/' };
        let target = format!("{}{sep}{}", state.new_location.trim_end_matches(['/', '\\']), state.new_name);
        let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
        widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.center().y), glyph::ARROW_RIGHT, 13.0, theme::FG_4);
        widgets::text_truncated(ui.painter(), pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(target, 12.0).color(theme::FG_3), w - 22.0);
    }
}

/// A template's cover from Creator Companion in `rect`: shimmering while it loads, then fading in (a little closer on
/// hover); a template VCC has no cover for keeps a strip in its colour with its icon.
#[allow(clippy::too_many_arguments)]
fn template_cover(p: &egui::Painter, state: &State, file: Option<&PathBuf>, rect: Rect, id: Id, hover: f32, glyph: &str, tint: egui::Color32) {
    let radii = [11.0, 11.0, 11.0, 11.0];
    let tex = file.and_then(|f| state.banners.get(f));
    let k = feedback::loaded(p.ctx(), id.with("cover"), tex.is_some());
    if k < 1.0 {
        if file.is_some() && state.banner_job.running() {
            feedback::shimmer(p, rect, 11.0);
        } else {
            feedback::fill_corners(p, rect, radii, theme::alpha(tint, 0.12 * (1.0 - k)));
            widgets::icon(p, rect.center(), glyph, (rect.height() * 0.5).clamp(14.0, 22.0), theme::alpha(tint, 1.0 - k));
        }
    }
    if let Some(t) = tex {
        feedback::cover(p, rect, radii, t, 1.0 + 0.04 * hover + 0.06 * (1.0 - k), k);
    }
}

/// How far the creation or the copy got.
fn creating(ui: &mut Ui, state: &State) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    {
        let p = state.copy_progress.lock().unwrap().clone();
        let label = state.acting().unwrap_or_default().to_string();
        let (r, _) = ui.allocate_exact_size(vec2(w, if p.total > 0 { 92.0 } else { 52.0 }), Sense::hover());
        let pt = ui.painter();
        widgets::frame(pt, r, 14.0, theme::alpha(theme::ACCENT, 0.07));
        feedback::spinner(pt, pos2(r.left() + 22.0, r.top() + 26.0), 8.0, 2.2, theme::ACCENT);
        let heading = if p.total > 0 { format!("{} · {:.0}%", p.phase, 100.0 * p.copied as f32 / p.total.max(1) as f32) } else { label };
        widgets::text(pt, pos2(r.left() + 42.0, r.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(heading, 13.5).weight(weight::MEDIUM));
        if p.total > 0 {
            let bar = Rect::from_min_size(pos2(r.left() + 16.0, r.top() + 50.0), vec2(r.width() - 32.0, 6.0));
            pt.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
            let f = anim::value(&ctx, Id::unique("new-copy-progress"), p.copied as f32 / p.total.max(1) as f32, Spring::SOFT).clamp(0.0, 1.0);
            pt.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), 6.0)), theme::radius(3.0), theme::ACCENT);
            let files = format!("{} of {} files · {}", format::integer(p.copied as f64), format::integer(p.total as f64), p.file);
            widgets::text_truncated(
                pt,
                pos2(r.left() + 16.0, r.top() + 74.0),
                Align2::LEFT_CENTER,
                &Txt::new(files, 11.5).color(theme::FG_3),
                r.width() - 32.0,
            );
        }
    }
}

fn note(ui: &mut Ui, text: &str) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(text, 12.0).color(theme::FG_3).line_height(18.0).job(w - 22.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.top() + 9.0), icons::INFO, 13.0, theme::FG_4);
    ui.painter().galley(pos2(r.left() + 22.0, r.top()), g, theme::FG_3);
}

fn inline_warning(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.center().y), icons::WARNING, 13.0, theme::WARN);
    widgets::text(ui.painter(), pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 12.0).color(theme::WARN));
}

fn new_source(state: &State) -> Option<(PathBuf, String)> {
    if state.duplicate {
        state.scan.projects.iter().filter(|p| p.registered && !p.missing).nth(state.duplicate_index).map(|p| (p.path.clone(), p.unity.clone()))
    } else {
        state.templates.get(state.template_index).map(|t| (t.path.clone(), if t.unity.is_empty() { state.new_unity.clone() } else { t.unity.clone() }))
    }
}

fn new_footer(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    let source = new_source(state);
    let valid = templates::validate_name(&state.new_name).is_ok();
    let enabled = source.as_ref().is_some_and(|(_, u)| !u.is_empty()) && valid && !state.new_location.is_empty() && !state.running();
    let label = match (state.running(), state.duplicate) {
        (true, true) => "Copying…",
        (true, false) => "Creating…",
        (false, true) => "Copy the project",
        (false, false) => "Create the project",
    };
    let b = Button::new(label).icon(icons::PLUS).variant(Variant::Primary).enabled(enabled).id_salt("new-create");
    if widgets::activated(&b.show(ui)) {
        let (source, unity) = source.unwrap();
        let name = state.new_name.clone();
        let location = PathBuf::from(&state.new_location);
        let duplicate = state.duplicate;
        state.copy_cancel = Arc::new(AtomicBool::new(false));
        *state.copy_progress.lock().unwrap() = Default::default();
        let cancel = state.copy_cancel.clone();
        let progress = state.copy_progress.clone();
        let id = state.dialog_id();
        state.task(&ctx, if duplicate { "Copying the project…" } else { "Creating the project…" }, move || {
            let path = orbiters_projects::runtime()?.block_on(templates::create(
                &orbiters_projects::vcc_dir(),
                &source,
                &location,
                &name,
                &unity,
                duplicate,
                &cancel,
                &progress,
            ))?;
            Ok(Outcome::Added(
                path.clone(),
                format!("{} is ready: VCC and ALCOM list it too", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
            ))
        });
        let _ = id;
    }
    if state.running() {
        let p = state.copy_progress.lock().unwrap().clone();
        if p.phase == "Copying project" && widgets::activated(&Button::new("Stop copying").variant(Variant::Ghost).id_salt("new-stop").show(ui)) {
            state.copy_cancel.store(true, Ordering::Relaxed);
        }
    } else if widgets::activated(&Button::new("Cancel").variant(Variant::Ghost).id_salt("new-cancel").show(ui)) {
        state.close_modal(&ctx);
    }
}

// Capture's avatar picker.

/// The registered projects' avatars, each once (with how many copies it has), the one worn first.
fn avatar_list(state: &State, worn: Option<&String>) -> Vec<(Project, Avatar, usize)> {
    let mut avatars: BTreeMap<String, (Project, Avatar, usize)> = BTreeMap::new();
    let mut projects: Vec<&Project> = state.scan.projects.iter().filter(|p| p.registered).collect();
    projects.sort_by_key(|p| std::cmp::Reverse(p.modified));
    for p in projects {
        for a in &p.avatars {
            avatars.entry(a.id.clone()).and_modify(|v| v.2 += 1).or_insert(((*p).clone(), a.clone(), 1));
        }
    }
    let mut list: Vec<_> = avatars.into_values().collect();
    list.sort_by_key(|(_, a, _)| (Some(&a.id) != worn, a.name.to_lowercase()));
    list
}

fn avatar_picker(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    errors(ui, state, "avatars");
    let worn = cx.snap.orb.avatar_id.clone();
    let list = avatar_list(state, worn.as_ref());
    if state.preselected.as_ref().is_none_or(|id| !list.iter().any(|(_, a, _)| &a.id == id)) {
        state.preselected = list.first().map(|(_, a, _)| a.id.clone());
    }
    let w = ui.available_width();
    let (cols, tw) = layout::columns(w, 180.0, 12.0);
    let th = (tw * 0.78).round();
    let tile_h = th + 62.0;
    if list.is_empty() {
        if state.scanning() {
            let (area, _) = ui.allocate_exact_size(vec2(w, tile_h), Sense::hover());
            for i in 0..cols.min(3) {
                let r = Rect::from_min_size(area.min + vec2(i as f32 * (tw + 12.0), 0.0), vec2(tw, tile_h));
                widgets::frame(ui.painter(), r, 16.0, theme::surface_soft());
                feedback::shimmer(ui.painter(), Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(tw - 12.0, th - 6.0)), 12.0);
                feedback::shimmer(ui.painter(), Rect::from_min_size(pos2(r.left() + 14.0, r.top() + th + 16.0), vec2(tw * 0.5, 12.0)), 6.0);
                feedback::shimmer(ui.painter(), Rect::from_min_size(pos2(r.left() + 14.0, r.top() + th + 38.0), vec2(tw * 0.34, 10.0)), 5.0);
            }
            progress_line(ui, "Finding the avatars of your projects…");
        } else {
            let (r, _) = ui.allocate_exact_size(vec2(w, 236.0), Sense::hover());
            let p = ui.painter();
            widgets::frame(p, r, 18.0, theme::white(0.02));
            art::tile(p, Rect::from_center_size(r.center() + vec2(0.0, -70.0), vec2(48.0, 48.0)), icons::PERSON_ARMS_SPREAD, theme::BLUE);
            widgets::text(p, r.center() + vec2(0.0, -24.0), Align2::CENTER_CENTER, &Txt::new("No uploaded avatar found yet", 15.0).weight(weight::SEMIBOLD));
            let g = p.layout_job({
                let mut job =
                    Txt::new("Add your avatar's Unity project on the Unity projects page. An avatar shows up once it was uploaded and its scene saved.", 12.5)
                        .color(theme::FG_3)
                        .line_height(19.0)
                        .job(420.0);
                job.halign = egui::Align::Center;
                job
            });
            p.galley(pos2(r.center().x, r.center().y - 6.0), g, theme::FG_3);
            let b = Button::new("Open Unity projects").icon(glyph::ARROW_RIGHT).size(Size::Sm).id_salt("avatars-go-projects");
            let bw = b.desired_width(ui);
            if widgets::activated(&b.show_at(ui, Rect::from_min_size(pos2(r.center().x - bw * 0.5, r.bottom() - 52.0), vec2(bw, 32.0)))) {
                state.close_modal(&ctx);
                cx.go = Some(Go::Projects);
            }
        }
        return;
    }
    let rows = list.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (tile_h + 12.0) - 12.0), Sense::hover());
    let mut proceed = None;
    for (i, (p, a, n)) in list.iter().enumerate() {
        let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (tw + 12.0), (i / cols) as f32 * (tile_h + 12.0)), vec2(tw, tile_h));
        let id = Id::unique(("pick-avatar", &a.id));
        let selected = state.preselected.as_ref() == Some(&a.id);
        let (resp, hover, sel) = selectable(ui, id, r, 18.0, selected);
        let pt = ui.painter();
        let thumb = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(tw - 12.0, th - 6.0));
        art::avatar(pt, state, a, thumb, [13.0; 4], 1.0 + 0.04 * hover);
        if worn.as_ref() == Some(&a.id) {
            let t = Txt::new("Wearing now", 10.5).weight(weight::SEMIBOLD).color(theme::FG);
            let bw = widgets::text_size(ui, &t).x + 30.0;
            let br = Rect::from_min_size(thumb.min + vec2(8.0, 8.0), vec2(bw, 22.0));
            pt.rect_filled(br, theme::radius(11.0), theme::black(0.62));
            widgets::status_dot(pt, pos2(br.left() + 12.0, br.center().y), Tone::Ok, false);
            widgets::text(pt, pos2(br.left() + 22.0, br.center().y), Align2::LEFT_CENTER, &t);
        }
        art::check(pt, pos2(thumb.right() - 16.0, thumb.top() + 16.0), sel);
        widgets::text_truncated(
            pt,
            pos2(r.left() + 14.0, thumb.bottom() + 20.0),
            Align2::LEFT_CENTER,
            &Txt::new(&a.name, 14.0).weight(weight::SEMIBOLD),
            tw - 28.0,
        );
        let sub = if *n > 1 { format!("In {n} projects") } else { p.name.clone() };
        widgets::text_truncated(pt, pos2(r.left() + 14.0, thumb.bottom() + 40.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.0).color(theme::FG_3), tw - 28.0);
        if widgets::activated(&resp) {
            state.preselected = Some(a.id.clone());
        }
        if resp.double_clicked() && !state.running() {
            proceed = Some(a.id.clone());
        }
    }
    if let Some(id) = proceed {
        state.choose_avatar(&ctx, id, server(cx));
    }
}

fn avatar_footer(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let worn = cx.snap.orb.avatar_id.clone();
    let list = avatar_list(state, worn.as_ref());
    if list.is_empty() {
        if widgets::activated(&Button::new("Close").variant(Variant::Ghost).id_salt("avatars-close").show(ui)) {
            state.close_modal(&ctx);
        }
        return;
    }
    let chosen = state.preselected.clone().filter(|id| list.iter().any(|(_, a, _)| &a.id == id));
    let copies = chosen.as_ref().map(|id| state.copies(id).len()).unwrap_or(0);
    let label = if copies > 1 { "Choose the copy" } else { "Add N4N0" };
    let b = Button::new(label)
        .icon(if copies > 1 { glyph::ARROW_RIGHT } else { icons::N4N0_MARK })
        .variant(Variant::Primary)
        .enabled(chosen.is_some() && !state.running())
        .id_salt("avatars-go");
    if widgets::activated(&b.show(ui)) {
        if let Some(id) = chosen {
            state.choose_avatar(&ctx, id, server(cx));
        }
    }
    if widgets::activated(&Button::new("Cancel").variant(Variant::Ghost).id_salt("avatars-cancel").show(ui)) {
        state.close_modal(&ctx);
    }
    if copies > 1 {
        let left = ui.available_rect_before_wrap();
        widgets::text_truncated(
            ui.painter(),
            pos2(left.left(), left.center().y),
            Align2::LEFT_CENTER,
            &Txt::new(format!("Saved in {copies} places: you choose which next"), 12.0).color(theme::FG_3),
            left.width() - 12.0,
        );
    }
}

// Several copies of the avatar.

fn project_choice(ui: &mut Ui, state: &mut State, avatar: &str) {
    errors(ui, state, "choice");
    let w = ui.available_width();
    let copies = state.copies(avatar);
    if state.choice.as_ref().is_none_or(|(p, s)| !copies.iter().any(|(cp, ca)| &cp.path == p && &ca.scene == s)) {
        state.choice = copies.first().map(|(p, a)| (p.path.clone(), a.scene.clone()));
    }
    for (p, a) in &copies {
        let (r, _) = ui.allocate_exact_size(vec2(w, 76.0), Sense::hover());
        let selected = state.choice.as_ref().is_some_and(|(cp, cs)| cp == &p.path && cs == &a.scene);
        let (resp, _, sel) = selectable(ui, Id::unique(("copy-choice", &p.path, &a.scene)), r, 16.0, selected);
        let pt = ui.painter();
        art::cover(pt, state, p, Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 26.0), vec2(52.0, 52.0)), [12.0; 4], 1.0);
        let x = r.left() + 78.0;
        let tw = r.right() - 50.0 - x;
        widgets::text_truncated(pt, pos2(x, r.center().y - 17.0), Align2::LEFT_CENTER, &Txt::new(&p.name, 14.0).weight(weight::SEMIBOLD), tw);
        widgets::text_truncated(pt, pos2(x, r.center().y + 1.0), Align2::LEFT_CENTER, &Txt::new(p.path.display().to_string(), 11.5).color(theme::FG_3), tw);
        widgets::text_truncated(
            pt,
            pos2(x, r.center().y + 18.0),
            Align2::LEFT_CENTER,
            &Txt::new(format!("{} · edited {}", a.scene, format::relative_time(p.modified, orbiters_projects::discovery::now())), 11.5).color(theme::FG_4),
            tw,
        );
        // A radio: a ring, filled with a check once chosen.
        let c = pos2(r.right() - 26.0, r.center().y);
        pt.circle_stroke(c, 10.0, egui::Stroke::new(1.5, theme::mix(theme::FG_4, theme::ACCENT, sel.clamp(0.0, 1.0))));
        art::check(pt, c, sel);
        if widgets::activated(&resp) {
            state.choice = Some((p.path.clone(), a.scene.clone()));
        }
    }
}

fn choice_footer(ui: &mut Ui, cx: &mut Cx, state: &mut State, avatar: &str) {
    let ctx = ui.ctx().clone();
    let picked = state.copies(avatar).into_iter().find(|(p, a)| state.choice.as_ref().is_some_and(|(cp, cs)| cp == &p.path && cs == &a.scene));
    let b = Button::new("Add N4N0 here").icon(icons::N4N0_MARK).variant(Variant::Primary).enabled(picked.is_some() && !state.running()).id_salt("choice-go");
    if widgets::activated(&b.show(ui)) {
        if let Some((p, a)) = picked {
            state.install(&ctx, p, a, true, server(cx), Rect::NOTHING);
        }
    }
    if widgets::activated(&Button::new("Back").icon(icons::ARROW_LEFT).variant(Variant::Ghost).id_salt("choice-back").show(ui)) {
        state.modal = Some(Modal::Avatars);
    }
}

// The setup's steps.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Installing,
    Installed,
    OpenUnity,
    Confirm,
    Done,
    Declined,
    Failed,
}

fn stage(state: &State) -> Stage {
    match state.flow.as_ref() {
        None => Stage::Installing,
        Some(f) => match f.result.as_ref().map(|r| r.status.clone()) {
            Some(setup::Status::Done) => Stage::Done,
            Some(setup::Status::Declined) => Stage::Declined,
            Some(setup::Status::Failed) => Stage::Failed,
            None if f.packages_only => Stage::Installed,
            None if f.unity_open => Stage::Confirm,
            None => Stage::OpenUnity,
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Done,
    Now,
    Waiting,
    Later,
    Stopped(Tone),
}

fn setup_steps(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    errors(ui, state, "setup");
    let s = stage(state);
    let nano = state.flow.as_ref().map(|f| !f.packages_only).or(state.pending.as_ref().map(|p| p.2)).unwrap_or(true);
    let result = state.flow.as_ref().and_then(|f| f.result.as_ref()).map(|r| r.message.clone()).unwrap_or_default();
    let (tone, icon, headline, message) = match s {
        Stage::Installing => (
            Tone::Info,
            None,
            if nano { "Installing My Avatar and Toolkit" } else { "Installing My Avatar" },
            "Orbiters adds the packages to the project through VCC's own files: Unity does not need to be open.".to_string(),
        ),
        Stage::Installed => {
            (Tone::Ok, Some(icons::CHECK), "My Avatar is installed", "Open the project: My Avatar's window is in Unity's Tools menu.".to_string())
        }
        Stage::OpenUnity => (
            Tone::Warn,
            Some(icons::CUBE),
            "Open the project to continue",
            "My Avatar and Toolkit are in. Open the project in Unity: it asks you to confirm adding N4N0 to the avatar.".to_string(),
        ),
        Stage::Confirm => (
            Tone::Info,
            Some(icons::N4N0_MARK),
            "Confirm in Unity",
            "Unity is open on this project: switch to it and accept adding N4N0 to the avatar. This page follows along.".to_string(),
        ),
        Stage::Done => (
            Tone::Ok,
            Some(icons::CHECK),
            "N4N0 is on your avatar in Unity",
            format!("{result} Upload the avatar from the VRChat SDK panel that opened, then wear it in VRChat to capture.").trim().to_string(),
        ),
        Stage::Declined => {
            (Tone::Warn, Some(icons::X), "You said not now in Unity", if result.is_empty() { "Nothing was changed.".into() } else { result.clone() })
        }
        Stage::Failed => (Tone::Danger, Some(icons::WARNING), "The setup needs a look", result.clone()),
    };
    // The state at the top: a big tinted disc whose icon pops in when the state changes.
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(&message, 13.0).color(theme::FG_2).line_height(20.0).job(w - 100.0));
    let h = (g.size().y + 40.0).max(76.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let pop = anim::spring_from(&ctx, Id::unique(("setup-pop", format!("{s:?}"))), [0.4], [1.0], Spring::BOUNCY)[0];
    let c = pos2(r.left() + 38.0, r.top() + 38.0);
    let p = ui.painter();
    let col = anim::color(&ctx, Id::unique("setup-tone"), tone.color(), Spring::SOFT);
    if matches!(s, Stage::Confirm) {
        // Waiting on Unity: a slow ring breathing around the disc.
        let t = anim::clock(&ctx, 30.0);
        let k = ((t % 2.0) / 2.0) as f32;
        p.circle_stroke(c, 30.0 + 7.0 * k, egui::Stroke::new(2.0, theme::alpha(col, 0.35 * (1.0 - k))));
    }
    p.circle_filled(c, 30.0 * pop.clamp(0.0, 1.2), theme::alpha(col, 0.14));
    match icon {
        Some(g) => widgets::icon(p, c, g, 26.0 * pop.clamp(0.0, 1.2), col),
        None => feedback::spinner(p, c, 13.0, 2.6, col),
    }
    widgets::text(p, pos2(r.left() + 92.0, r.top() + 20.0), Align2::LEFT_CENTER, &Txt::new(headline, 17.0).weight(weight::SEMIBOLD));
    p.galley(pos2(r.left() + 92.0, r.top() + 34.0), g, theme::FG_2);

    // The steps, on a thread.
    let mut steps: Vec<(&str, Step)> =
        vec![(if nano { "Install My Avatar and Toolkit" } else { "Install My Avatar" }, if s == Stage::Installing { Step::Now } else { Step::Done })];
    if nano {
        steps.push((
            "Open the project in Unity",
            match s {
                Stage::Installing => Step::Later,
                Stage::OpenUnity => Step::Now,
                _ => Step::Done,
            },
        ));
        steps.push((
            "Confirm adding N4N0 in Unity",
            match s {
                Stage::Installing | Stage::OpenUnity => Step::Later,
                Stage::Confirm => Step::Waiting,
                Stage::Done => Step::Done,
                Stage::Declined => Step::Stopped(Tone::Warn),
                Stage::Failed => Step::Stopped(Tone::Danger),
                Stage::Installed => Step::Done,
            },
        ));
        steps.push(("Upload the avatar and wear it in VRChat", if s == Stage::Done { Step::Now } else { Step::Later }));
    } else {
        steps.push(("Open the project and My Avatar's window", if s == Stage::Installed { Step::Now } else { Step::Later }));
    }
    ui.add_space(4.0);
    let (area, _) = ui.allocate_exact_size(vec2(w, steps.len() as f32 * 40.0), Sense::hover());
    widgets::frame(ui.painter(), area.expand2(vec2(0.0, 8.0)), 16.0, theme::surface_soft());
    let n = steps.len();
    for (i, (label, step)) in steps.into_iter().enumerate() {
        let row = Rect::from_min_size(area.min + vec2(0.0, i as f32 * 40.0), vec2(w, 40.0));
        let c = pos2(row.left() + 28.0, row.center().y);
        let p = ui.painter();
        if i + 1 < n {
            let done = matches!(step, Step::Done);
            p.line_segment(
                [c + vec2(0.0, 12.0), c + vec2(0.0, 28.0)],
                egui::Stroke::new(2.0, if done { theme::alpha(theme::ACCENT, 0.5) } else { theme::white(0.08) }),
            );
        }
        let k = anim::spring_from(&ctx, Id::unique(("setup-step", i, format!("{s:?}"))), [0.6], [1.0], Spring::BOUNCY)[0].clamp(0.0, 1.2);
        let (fg, weight) = match step {
            Step::Done => {
                p.circle_filled(c, 11.0 * k, theme::ACCENT);
                widgets::icon(p, c, icons::CHECK, 12.0 * k, theme::ON_ACCENT);
                (theme::FG_2, weight::REGULAR)
            }
            Step::Now => {
                p.circle_filled(c, 11.0 * k, theme::alpha(theme::ACCENT, 0.18));
                if s == Stage::Installing {
                    feedback::spinner(p, c, 6.5, 2.0, theme::ACCENT);
                } else {
                    p.circle_filled(c, 4.5, theme::ACCENT);
                }
                (theme::FG, weight::SEMIBOLD)
            }
            Step::Waiting => {
                p.circle_filled(c, 11.0 * k, theme::alpha(theme::ACCENT, 0.12));
                feedback::spinner(p, c, 6.5, 2.0, theme::ACCENT);
                (theme::FG, weight::SEMIBOLD)
            }
            Step::Later => {
                p.circle_stroke(c, 10.0, egui::Stroke::new(1.5, theme::white(0.14)));
                widgets::text(p, c, Align2::CENTER_CENTER, &Txt::new((i + 1).to_string(), 11.0).color(theme::FG_4));
                (theme::FG_3, weight::REGULAR)
            }
            Step::Stopped(t) => {
                p.circle_filled(c, 11.0 * k, theme::alpha(t.color(), 0.2));
                widgets::icon(p, c, if t == Tone::Danger { icons::WARNING } else { icons::X }, 12.0 * k, t.color());
                (t.color(), weight::SEMIBOLD)
            }
        };
        widgets::text(p, pos2(row.left() + 52.0, row.center().y), Align2::LEFT_CENTER, &Txt::new(label, 13.5).weight(weight).color(fg));
    }
    ui.add_space(6.0);
    state.acknowledge_result(&ctx);
}

fn setup_footer(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let s = stage(state);
    let open_project = |state: &mut State, ui: &mut Ui, primary: bool| {
        let Some(project) = state.flow.as_ref().map(|f| f.project.clone()) else { return };
        let b = Button::new("Open project")
            .icon(icons::PLAY)
            .variant(if primary { Variant::Primary } else { Variant::Secondary })
            .enabled(!state.running())
            .id_salt("setup-open");
        if widgets::activated(&b.show(ui)) {
            let ctx = ui.ctx().clone();
            state.task(&ctx, "Opening Unity…", move || {
                orbiters_projects::open_project(&project)?;
                Ok(Outcome::Picked(None))
            });
            if let Some(f) = state.flow.as_mut() {
                f.unity_open = true;
            }
        }
    };
    match s {
        Stage::Installing => {
            Button::new("Working…").variant(Variant::Secondary).enabled(false).id_salt("setup-working").show(ui);
        }
        Stage::Done => {
            if widgets::activated(&Button::new("Done").icon(icons::CHECK).variant(Variant::Primary).id_salt("setup-done").show(ui)) {
                state.close_modal(&ctx);
            }
        }
        Stage::Declined | Stage::Failed => {
            let again =
                Button::new("Try again").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Primary).enabled(!state.running()).id_salt("setup-again");
            if widgets::activated(&again.show(ui)) {
                if let Some((project, avatar)) = state.flow.as_ref().map(|f| (f.project.clone(), f.avatar.clone())) {
                    state.install(&ctx, project, avatar, true, server(cx), Rect::NOTHING);
                }
            }
            open_project(state, ui, false);
        }
        Stage::Installed | Stage::OpenUnity => {
            open_project(state, ui, true);
            if widgets::activated(&Button::new("Close").variant(Variant::Ghost).id_salt("setup-close").show(ui)) {
                state.close_modal(&ctx);
            }
        }
        Stage::Confirm => {
            open_project(state, ui, false);
            let hub = Button::new("Unity Hub").icon(icons::CUBE).variant(Variant::Ghost).id_salt("setup-hub");
            if widgets::activated(&hub.show(ui)) {
                state.task(&ctx, "Opening Unity Hub…", move || {
                    orbiters_projects::open_hub()?;
                    Ok(Outcome::Picked(None))
                });
            }
        }
    }
}
