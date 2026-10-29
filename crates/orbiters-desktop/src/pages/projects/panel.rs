//! The selected project's panel: beside the cards on wide windows (its left edge drags to resize it; a double click
//! gives it its own width back), a sheet sliding over them on narrow ones. A hero card (the project's avatars as its
//! cover, where it is, opening it), then cards for its avatars (N4N0 and My Avatar for each), its packages (their
//! versions in dropdowns), the packages it could add, and its last commits. Switching projects crossfades the cards.

use super::art::{self, glyph};
use super::state::{server, Outcome, State};
use crate::pages::{Cx, PAD_X};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::{discovery::Project, vpm};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::layout::Split;
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::popup::{self, Item};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, format, icons};

/// Packages that could be added listed before "Show all".
const AVAILABLE_FIRST: usize = 8;

#[allow(clippy::too_many_arguments)]
pub(super) fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State, page: Rect, content_top: f32, split: Split, dock: f32, panel_w: f32) {
    let ctx = ui.ctx().clone();
    let Some(project) = state.scan.projects.iter().find(|p| Some(&p.path) == state.selected.as_ref()).cloned() else { return };
    let rect = if split.docked {
        state.panel_closing = false;
        let out = (1.0 - dock) * (panel_w + PAD_X + 24.0);
        Rect::from_min_max(pos2(page.right() - PAD_X - panel_w + out, content_top), pos2(page.right() - PAD_X + out, page.bottom() - 20.0))
    } else {
        // Over the cards: slides in from the right edge, the page dimmed behind it; a press beside it closes it.
        let v =
            anim::spring_from(&ctx, Id::unique(("project-sheet", state.panel_serial)), [0.0], [if state.panel_closing { 0.0 } else { 1.0 }], Spring::SOFT)[0];
        if state.panel_closing && v < 0.02 {
            state.selected = None;
            state.panel_closing = false;
            return;
        }
        let w = split.panel;
        let out = (1.0 - v) * (w + 24.0);
        let scrim = ui.interact(page, Id::unique("project-sheet-scrim"), Sense::click());
        ui.painter().rect_filled(page, 0.0, theme::black(0.5 * v.clamp(0.0, 1.0)));
        if widgets::pressed(&scrim) {
            state.panel_closing = true;
        }
        let r = Rect::from_min_max(pos2(page.right() - 12.0 - w + out, page.top() + cx.top + 8.0), pos2(page.right() - 12.0 + out, page.bottom() - 12.0));
        paint::shadow(ui.painter(), r, 24.0, vec2(-6.0, 12.0), 36.0, -8.0, theme::black(0.5 * v.clamp(0.0, 1.0)));
        r
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !split.docked && state.modal.is_none() && !popup::any_open(&ctx) {
        state.panel_closing = true;
    }
    let p = ui.painter();
    widgets::frame(p, rect, 24.0, theme::INK_900);
    // Clicks on the panel stay there.
    let _ = ui.interact(rect, Id::unique("project-panel-body"), Sense::click());
    if split.docked {
        resize_handle(ui, state, page, rect);
    }

    // Switching projects: the cards fade and rise into the new one.
    let k = anim::spring_from(&ctx, Id::unique(("project-panel-content", state.switches)), [0.0], [1.0], Spring::SOFT)[0];
    let mut pu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(8.0, 8.0), rect.max - vec2(14.0, 8.0))).id_salt("project-panel"));
    pu.set_clip_rect(rect.shrink(2.0).intersect(ui.clip_rect()));
    let packages_y = Id::unique("project-panel-packages-y");
    let bar = Rect::from_min_max(pos2(rect.right() - 13.0, rect.top() + 18.0), pos2(rect.right() - 1.0, rect.bottom() - 18.0));
    let mut scroll = egui::ScrollArea::vertical().id_salt(("project-panel-scroll", &project.path)).auto_shrink([false, false]).scroll_bar_rect(bar);
    if state.scroll_to_packages {
        if let Some(y) = ctx.data(|d| d.get_temp::<f32>(packages_y)) {
            scroll = scroll.vertical_scroll_offset((y - 64.0).max(0.0));
            state.scroll_to_packages = false;
        }
    }
    let from = orbiters_ui::blur::next_shape(ui);
    let out = scroll.show(&mut pu, |ui| {
        ui.multiply_opacity(k.clamp(0.0, 1.0));
        ui.add_space(10.0 * (1.0 - k).max(0.0));
        ui.spacing_mut().item_spacing = vec2(0.0, 10.0);
        let top = ui.cursor().top();
        hero(ui, cx, state, &project, !split.docked);
        if state.modal.is_none() {
            super::ui::errors(ui, state, "panel");
        }
        avatars(ui, cx, state, &project);
        ctx.data_mut(|d| d.insert_temp(packages_y, ui.cursor().top() - top));
        packages(ui, state, &project);
        available(ui, state, &project);
        commits(ui, state);
        ui.add_space(8.0);
    });
    sticky_bar(ui, cx, state, &project, rect, from, out.state.offset().y);
}

/// Once the hero card scrolled away, the project's name and Open in Unity stay at the panel's top over the cards
/// passing under it, blurred (the large title that becomes a bar).
fn sticky_bar(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project, rect: Rect, from: egui::layers::ShapeIdx, scrolled: f32) {
    let ctx = ui.ctx().clone();
    let w = rect.width() - 22.0;
    let threshold = hero_height(w) - 52.0;
    let k = anim::toggle(&ctx, Id::unique("project-panel-bar"), scrolled > threshold, Spring::SOFT).clamp(0.0, 1.0);
    if k < 0.01 {
        return;
    }
    let band = Rect::from_min_size(rect.min, vec2(rect.width(), 60.0));
    let fill = match cx.gpu {
        Some(gpu) => {
            let look = orbiters_ui::blur::Look { blur: 14.0 * k, solid: 24.0, darken: 0.55 * k, fade: 0.0 };
            orbiters_ui::blur::under(ui, Id::unique("project-panel-bar-blur"), &gpu.textures, from, band, look);
            theme::alpha(theme::INK_900, 0.55 * k)
        }
        None => theme::alpha(theme::INK_900, 0.94 * k),
    };
    let p = ui.painter();
    feedback::fill_corners(p, band, [24.0, 24.0, 0.0, 0.0], fill);
    let _ = ui.interact(band, Id::unique("project-panel-bar-body"), Sense::click());
    let dy = 8.0 * (1.0 - k);
    let mut bu = ui.new_child(UiBuilder::new().max_rect(band).id_salt("project-panel-bar"));
    bu.multiply_opacity(k);
    let b = Button::new("Open in Unity")
        .icon(icons::PLAY)
        .variant(Variant::Primary)
        .size(Size::Sm)
        .enabled(project.registered && !project.missing && !state.running() && k > 0.5)
        .id_salt("panel-bar-open");
    let bw = b.desired_width(&bu);
    let br = Rect::from_min_size(pos2(band.right() - 20.0 - bw, band.center().y - 16.0 + dy), vec2(bw, 32.0));
    let thumb = Rect::from_min_size(pos2(band.left() + 18.0, band.center().y - 16.0 + dy), vec2(32.0, 32.0));
    art::cover(bu.painter(), state, project, thumb, [9.0; 4], 1.0);
    widgets::text_truncated(
        bu.painter(),
        pos2(thumb.right() + 10.0, band.center().y + dy),
        Align2::LEFT_CENTER,
        &Txt::new(&project.name, 15.0).weight(weight::SEMIBOLD),
        br.left() - thumb.right() - 22.0,
    );
    if project.registered && widgets::activated(&b.show_at(&mut bu, br)) {
        let p = project.clone();
        state.task(&ctx, "Opening Unity…", move || {
            orbiters_projects::open_project(&p)?;
            Ok(Outcome::Changed("Project sent to Unity".into()))
        });
    }
}

/// The hero card's height in a panel `w` wide.
fn hero_height(w: f32) -> f32 {
    (w * 0.42).clamp(132.0, 190.0) + 14.0 + 20.0 + 10.0 + 22.0 + 16.0 + 40.0 + 16.0
}

/// The docked panel's left edge: a grip that shows on hover and drags the panel wider or narrower.
fn resize_handle(ui: &mut Ui, state: &mut State, page: Rect, rect: Rect) {
    let ctx = ui.ctx().clone();
    let strip = Rect::from_min_max(pos2(rect.left() - 14.0, rect.top() + 24.0), pos2(rect.left() - 2.0, rect.bottom() - 24.0));
    state.anchors.insert("panel-grip".into(), strip);
    let id = Id::unique("project-panel-resize");
    let resp = ui
        .interact(strip, id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::ResizeHorizontal)
        .on_hover_text("Drag to resize, double-click for the usual width");
    if resp.dragged() {
        if let Some(p) = ctx.pointer_interact_pos() {
            state.panel_width = Some(page.right() - PAD_X - p.x);
        }
    }
    if resp.double_clicked() {
        state.panel_width = None;
    }
    let active = resp.dragged();
    let k = anim::toggle(&ctx, id.with("k"), resp.hovered() || active, Spring::SNAPPY);
    if k > 0.01 {
        let grip = Rect::from_center_size(pos2(strip.center().x, rect.center().y), vec2(4.0, 28.0 + 20.0 * k));
        ui.painter().rect_filled(grip, theme::radius(2.0), if active { theme::ACCENT } else { theme::white(0.25 * k) });
    }
}

/// The project at the top: its cover with its name, where it is, what it is, and the ways to open it.
fn hero(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project, sheet: bool) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let cover_h = (w * 0.42).clamp(132.0, 190.0);
    let h = hero_height(w);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter().clone();
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::surface());
    let cover = Rect::from_min_size(r.min, vec2(w, cover_h));
    art::cover(&p, state, project, cover, [theme::CARD_RADIUS, theme::CARD_RADIUS, 0.0, 0.0], 1.0);
    let shade = Rect::from_min_max(pos2(cover.left(), cover.top() + cover_h * 0.35), cover.max);
    paint::fill_gradient(
        &p,
        &paint::rounded_rect_points_corners(shade, [0.0; 4]),
        &Gradient::vertical(shade, &[(0.0, Color32::TRANSPARENT), (1.0, theme::alpha(theme::INK_950, 0.82))]),
    );
    let (tint, kind_glyph) = art::kind_look(project);
    let kind = Txt::new(project.kind.label(), 10.5).weight(weight::SEMIBOLD).tracking(0.02);
    let kw = widgets::text_size(ui, &kind).x + 36.0;
    let kr = Rect::from_min_size(cover.min + vec2(12.0, 12.0), vec2(kw, 22.0));
    p.rect_filled(kr, theme::radius(11.0), theme::black(0.6));
    widgets::icon(&p, pos2(kr.left() + 16.0, kr.center().y), kind_glyph, 12.0, tint);
    widgets::text(&p, pos2(kr.left() + 26.0, kr.center().y), Align2::LEFT_CENTER, &kind);
    widgets::text_truncated(
        &p,
        pos2(cover.left() + 16.0, cover.bottom() - 22.0),
        Align2::LEFT_CENTER,
        &Txt::new(&project.name, 21.0).weight(weight::SEMIBOLD).tracking(-0.01),
        w - 32.0,
    );
    if sheet {
        let close = Rect::from_center_size(pos2(cover.right() - 26.0, cover.top() + 26.0), vec2(32.0, 32.0));
        if widgets::activated(&IconButton::new(icons::X, "Close").size(32.0, 15.0).backdrop(theme::black(0.55)).show_at(ui, close)) {
            state.panel_closing = true;
        }
    }

    // Where it is.
    let mut y = cover.bottom() + 14.0;
    let x = r.left() + 16.0;
    widgets::icon(&p, pos2(x + 7.0, y + 10.0), icons::FOLDER, 14.0, theme::FG_4);
    widgets::text_truncated(
        &p,
        pos2(x + 22.0, y + 10.0),
        Align2::LEFT_CENTER,
        &Txt::new(project.path.display().to_string(), 12.0).color(theme::FG_3),
        w - 54.0,
    );
    y += 30.0;
    // What it is.
    let mut px = x;
    let edited = format!("Edited {}", format::relative_time(project.modified, orbiters_projects::discovery::now()));
    for (g, label) in [(Some(icons::CUBE), format!("Unity {}", project.unity)), (Some(icons::CLOCK), edited)] {
        px += widgets::pill_at(&p, ui, pos2(px, y + 11.0), g, &label) + 6.0;
    }
    if project.missing {
        widgets::pill_at(&p, ui, pos2(px, y + 11.0), Some(icons::WARNING), "Folder missing");
    }
    y += 22.0 + 16.0;

    // Opening it (or adding it first, for a project found on the computer).
    let row = Rect::from_min_size(pos2(x, y), vec2(w - 32.0, 40.0));
    let round = |i: usize| Rect::from_min_size(pos2(row.right() - 40.0 * (i + 1) as f32 - 8.0 * i as f32, row.top()), vec2(40.0, 40.0));
    let busy = state.running();
    let main_w = row.width() - 3.0 * 48.0;
    let main = Rect::from_min_size(row.min, vec2(main_w, 40.0));
    if project.registered {
        let b = Button::new("Open in Unity").icon(icons::PLAY).variant(Variant::Primary).width(main_w).enabled(!project.missing && !busy).id_salt("panel-open");
        if widgets::activated(&b.show_at(ui, main)) {
            let p = project.clone();
            state.task(&ctx, "Opening Unity…", move || {
                orbiters_projects::open_project(&p)?;
                Ok(Outcome::Changed("Project sent to Unity".into()))
            });
        }
    } else {
        let b = Button::new("Add to my projects").icon(icons::PLUS).variant(Variant::Primary).width(main_w).enabled(!busy).id_salt("panel-add");
        if widgets::activated(&b.show_at(ui, main)) {
            let path = project.path.clone();
            state.task(&ctx, "Adding the project…", move || {
                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, true))?;
                Ok(Outcome::Added(path, "Project added: VCC and ALCOM list it too".into()))
            });
        }
    }
    let folder = IconButton::new(icons::FOLDER_OPEN, "Open the folder").size(40.0, 17.0).backdrop(theme::control()).id_salt("panel-folder");
    if !project.missing && widgets::activated(&folder.show_at(ui, round(2))) {
        let path = project.path.clone();
        state.task(&ctx, "Opening the folder…", move || {
            orbiters_projects::hidden_command("explorer.exe").arg(path).spawn()?;
            Ok(Outcome::Picked(None))
        });
    }
    let hub = IconButton::new(icons::CUBE, "Open Unity Hub").size(40.0, 17.0).backdrop(theme::control()).id_salt("panel-hub");
    if widgets::activated(&hub.show_at(ui, round(1))) {
        state.task(&ctx, "Opening Unity Hub…", move || {
            orbiters_projects::open_hub()?;
            Ok(Outcome::Picked(None))
        });
    }
    let menu_r = round(0);
    state.anchors.insert("project-menu".into(), menu_r);
    let items = if project.registered {
        vec![Item::new("Copy the folder's path").icon(icons::COPY), Item::new("Remove from the list").icon(glyph::TRASH_SIMPLE).danger()]
    } else {
        vec![Item::new("Copy the folder's path").icon(icons::COPY)]
    };
    match popup::menu_button(ui, "project-menu", menu_r, icons::DOTS_THREE, "More", &items) {
        Some(0) => {
            ctx.copy_text(project.path.display().to_string());
            cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Success, "Path copied");
        }
        Some(1) if !busy => {
            let path = project.path.clone();
            state.selected = None;
            state.task(&ctx, "Removing from the list…", move || {
                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, false))?;
                Ok(Outcome::Changed("Removed from the shared project list. Its files stay.".into()))
            });
        }
        _ => {}
    }
}

/// The project's uploaded avatars, each with N4N0 and My Avatar to add.
fn avatars(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let count = (!project.avatars.is_empty()).then(|| project.avatars.len().to_string());
    art::card(
        ui,
        "avatars",
        icons::PERSON_ARMS_SPREAD,
        "Avatars",
        count,
        |_, _| {},
        |ui| {
            if project.avatars.is_empty() {
                let text = if project.kind == orbiters_projects::discovery::Kind::Worlds {
                    "A world project: N4N0 goes on avatars."
                } else {
                    "No uploaded avatar yet. Upload one from the VRChat SDK panel, save its scene, and it shows up here."
                };
                let g = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::FG_3).line_height(19.0).job(ui.available_width()));
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 4.0), Sense::hover());
                ui.painter().galley(r.min, g, theme::FG_3);
                return;
            }
            let worn = cx.snap.orb.avatar_id.clone();
            for (i, a) in project.avatars.iter().enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                let w = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(w, 92.0), Sense::hover());
                let thumb = Rect::from_min_size(pos2(r.left(), r.top() + 6.0), vec2(64.0, 64.0));
                art::avatar(ui.painter(), state, a, thumb, [14.0; 4], 1.0);
                let x = thumb.right() + 14.0;
                let p = ui.painter();
                let nr = widgets::text_truncated(
                    p,
                    pos2(x, thumb.top() + 11.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(&a.name, 14.5).weight(weight::SEMIBOLD),
                    r.right() - x - 80.0,
                );
                if worn.as_ref() == Some(&a.id) {
                    let t = Txt::new("Wearing", 10.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
                    let tw = widgets::text_size(ui, &t).x + 16.0;
                    let pr = Rect::from_min_size(pos2(nr.right() + 8.0, nr.center().y - 10.0), vec2(tw, 20.0));
                    p.rect_filled(pr, theme::radius(10.0), theme::alpha(theme::ACCENT, 0.14));
                    widgets::text(p, pr.center(), Align2::CENTER_CENTER, &t);
                }
                widgets::text_truncated(p, pos2(x, thumb.top() + 30.0), Align2::LEFT_CENTER, &Txt::new(&a.scene, 11.5).color(theme::FG_3), r.right() - x);
                let enabled = project.registered && !state.running();
                let nano = Button::new("Add N4N0").icon(icons::N4N0).size(Size::Sm).enabled(enabled).id_salt(("add-n4n0", &a.id, &a.scene));
                let nw = nano.desired_width(ui);
                let nr = Rect::from_min_size(pos2(x, thumb.top() + 46.0), vec2(nw, 32.0));
                state.anchors.insert(format!("add-n4n0-{i}"), nr);
                let hint = if project.registered { "My Avatar, Toolkit and N4N0: Unity asks you to confirm" } else { "Add the project to your list first" };
                if widgets::activated(&nano.show_at(ui, nr).on_hover_text(hint)) {
                    state.modal_fill = theme::control();
                    state.install(&ctx, project.clone(), a.clone(), true, server(cx), nr);
                }
                let mine = Button::new("My Avatar only").icon(icons::PERSON).variant(Variant::Ghost).size(Size::Sm).enabled(enabled).id_salt((
                    "add-my-avatar",
                    &a.id,
                    &a.scene,
                ));
                let mw = mine.desired_width(ui);
                let mr = Rect::from_min_size(pos2(nr.right() + 6.0, nr.top()), vec2(mw, 32.0));
                if mr.right() <= r.right() + 1.0 && widgets::activated(&mine.show_at(ui, mr).on_hover_text("Installs My Avatar in the project, without N4N0")) {
                    state.modal_fill = theme::INK_850;
                    state.install(&ctx, project.clone(), a.clone(), false, server(cx), mr);
                }
            }
        },
    );
}

/// A package's row: its tile, names, and on the right what `control` draws (given the rect left for it).
fn package_row(ui: &mut Ui, name: &str, display: &str, control_w: f32, control: impl FnOnce(&mut Ui, Rect)) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
    let p = ui.painter();
    let t = Rect::from_min_size(pos2(r.left(), r.center().y - 17.0), vec2(34.0, 34.0));
    art::tile(p, t, glyph::PACKAGE, art::package_tint(name));
    let x = t.right() + 12.0;
    let tw = r.right() - control_w - 10.0 - x;
    widgets::text_truncated(p, pos2(x, r.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(display, 13.5).weight(weight::MEDIUM), tw);
    widgets::text_truncated(p, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(name, 11.0).color(theme::FG_3), tw);
    let c = Rect::from_min_max(pos2(r.right() - control_w, r.top()), r.max);
    control(ui, c);
}

/// Shimmering rows while the packages are read.
fn row_skeletons(ui: &mut Ui, n: usize) {
    for _ in 0..n {
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
        let p = ui.painter();
        feedback::shimmer(p, Rect::from_min_size(pos2(r.left(), r.center().y - 17.0), vec2(34.0, 34.0)), 10.0);
        feedback::shimmer(p, Rect::from_min_size(pos2(r.left() + 46.0, r.center().y - 13.0), vec2(w * 0.4, 11.0)), 5.5);
        feedback::shimmer(p, Rect::from_min_size(pos2(r.left() + 46.0, r.center().y + 5.0), vec2(w * 0.28, 9.0)), 4.5);
        feedback::shimmer(p, Rect::from_min_size(pos2(r.right() - 140.0, r.center().y - 15.0), vec2(100.0, 30.0)), 15.0);
    }
}

/// The versions offered for a package: releases, pre-releases only when asked for (and the installed one always).
fn versions<'a>(p: &'a vpm::Package, prerelease: bool) -> Vec<&'a String> {
    p.versions.iter().filter(|v| prerelease || !v.contains('-') || Some(v.as_str()) == p.installed.as_deref()).collect()
}

fn packages(ui: &mut Ui, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let installed: Vec<vpm::Package> = state.view.packages.iter().filter(|p| p.installed.is_some()).cloned().collect();
    let count = (!installed.is_empty()).then(|| installed.len().to_string());
    let loading = state.view_loading() && installed.is_empty();
    let busy = state.running();
    let mut restore = false;
    art::card(
        ui,
        "packages",
        glyph::PACKAGE,
        "Packages",
        count,
        |ui, slot| {
            let b = Button::new("Restore")
                .icon(icons::ARROW_COUNTER_CLOCKWISE)
                .variant(Variant::Ghost)
                .size(Size::Sm)
                .enabled(!busy && !project.missing)
                .id_salt("restore");
            let bw = b.desired_width(ui);
            let r = Rect::from_min_size(pos2(slot.right() - bw, slot.center().y - 16.0), vec2(bw, 32.0));
            restore = widgets::activated(&b.show_at(ui, r).on_hover_text("Downloads the packages the project lists but does not have (VCC's Resolve)"));
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
            for (i, pkg) in installed.iter().enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                let current = pkg.installed.clone().unwrap_or_default();
                let list = versions(pkg, state.prerelease);
                let working = state.busy_package.as_deref() == Some(pkg.name.as_str());
                package_row(ui, &pkg.name, &pkg.display_name, 108.0 + 8.0 + 32.0, |ui, c| {
                    let dd = Rect::from_min_size(pos2(c.left(), c.center().y - 15.0), vec2(108.0, 30.0));
                    let rm = Rect::from_min_size(pos2(c.right() - 32.0, c.center().y - 16.0), vec2(32.0, 32.0));
                    if i == 0 {
                        state.anchors.insert("version-0".into(), dd);
                    }
                    if working {
                        feedback::spinner(ui.painter(), dd.center(), 8.0, 2.0, theme::ACCENT);
                    } else {
                        let items: Vec<Item> = list.iter().map(|v| if v.contains('-') { Item::new(v).detail("Pre-release") } else { Item::new(v) }).collect();
                        let selected = list.iter().position(|v| **v == current);
                        if let Some(k) = popup::dropdown(ui, ("version", &pkg.name), dd, &current, &items, selected, !busy) {
                            if list[k] != &current {
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
            }
            widgets::separator(ui);
            widgets::field(ui, "Pre-release versions", Some("Beta versions in the version lists"), 48.0, |ui| {
                widgets::toggle(ui, "prerelease", &mut state.prerelease);
            });
        },
    );
    if restore {
        package_action(state, &ctx, project, vpm::Change::Resolve, "Restoring missing packages…", "");
    }
}

fn available(ui: &mut Ui, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    if state.view_loading() && state.view.packages.is_empty() {
        return;
    }
    let q = state.package_query.trim().to_lowercase();
    let list: Vec<vpm::Package> = state
        .view
        .packages
        .iter()
        .filter(|p| p.installed.is_none() && !versions(p, state.prerelease).is_empty())
        .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.display_name.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let all = state.view.packages.iter().filter(|p| p.installed.is_none()).count();
    if all == 0 {
        return;
    }
    let busy = state.running();
    art::card(
        ui,
        "available",
        icons::PLUS,
        "Add packages",
        Some(all.to_string()),
        |_, _| {},
        |ui| {
            widgets::search_field(ui, "package-search", &mut state.package_query, "Search the repositories' packages", ui.available_width());
            ui.add_space(2.0);
            if list.is_empty() {
                widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 12.0), Align2::LEFT_CENTER, &Txt::new("No package matches.", 12.5).color(theme::FG_3));
                ui.add_space(24.0);
            }
            let shown = if state.all_packages || !q.is_empty() { list.len() } else { list.len().min(AVAILABLE_FIRST) };
            for (i, pkg) in list.iter().take(shown).enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                let vs = versions(pkg, state.prerelease);
                let id = Id::unique(("available-version", &project.path, &pkg.name));
                let chosen = ctx.data(|d| d.get_temp::<String>(id)).filter(|v| vs.iter().any(|x| *x == v)).unwrap_or_else(|| vs[0].clone());
                let working = state.busy_package.as_deref() == Some(pkg.name.as_str());
                package_row(ui, &pkg.name, &pkg.display_name, 100.0 + 8.0 + 32.0, |ui, c| {
                    let dd = Rect::from_min_size(pos2(c.left(), c.center().y - 15.0), vec2(100.0, 30.0));
                    let add = Rect::from_min_size(pos2(c.right() - 32.0, c.center().y - 16.0), vec2(32.0, 32.0));
                    let items: Vec<Item> = vs.iter().map(|v| if v.contains('-') { Item::new(v).detail("Pre-release") } else { Item::new(v) }).collect();
                    let selected = vs.iter().position(|v| **v == chosen);
                    if let Some(k) = popup::dropdown(ui, ("available", &pkg.name), dd, &chosen, &items, selected, !busy) {
                        ctx.data_mut(|d| d.insert_temp(id, vs[k].clone()));
                    }
                    if working {
                        feedback::spinner(ui.painter(), add.center(), 8.0, 2.0, theme::ACCENT);
                    } else {
                        let b = IconButton::new(icons::PLUS, "Add to the project").size(32.0, 15.0).backdrop(theme::control()).id_salt(("add", &pkg.name));
                        if !busy && widgets::activated(&b.show_at(ui, add)) {
                            let change = vpm::Change::Install { names: vec![(pkg.name.clone(), Some(chosen.clone()))], prerelease: state.prerelease };
                            package_action(state, &ctx, project, change, &format!("Adding {} {}…", pkg.display_name, chosen), &pkg.name);
                        }
                    }
                });
            }
            if list.len() > shown || (state.all_packages && list.len() > AVAILABLE_FIRST && q.is_empty()) {
                let label = if state.all_packages { "Show fewer".to_string() } else { format!("Show all {}", list.len()) };
                let b = Button::new(&label).variant(Variant::Ghost).size(Size::Sm).id_salt("available-all");
                let bw = b.desired_width(ui);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                if widgets::activated(&b.show_at(ui, Rect::from_min_size(pos2(r.center().x - bw * 0.5, r.top()), vec2(bw, 32.0)))) {
                    state.all_packages = !state.all_packages;
                }
            }
        },
    );
}

fn package_action(state: &mut State, ctx: &egui::Context, p: &Project, change: vpm::Change, label: &str, name: &str) {
    let path = p.path.clone();
    state.busy_package = (!name.is_empty()).then(|| name.to_string());
    state.task(ctx, label, move || {
        orbiters_projects::runtime()?.block_on(vpm::change(&orbiters_projects::vcc_dir(), &path, change))?;
        Ok(Outcome::Changed("Packages updated".into()))
    });
}

/// The last commits, on a thread.
fn commits(ui: &mut Ui, state: &State) {
    let commits: Vec<Vec<String>> = state.commits.iter().map(|c| c.split('\x1f').map(str::to_string).collect::<Vec<_>>()).filter(|p| p.len() == 4).collect();
    if commits.is_empty() {
        return;
    }
    art::card(
        ui,
        "commits",
        glyph::GIT_COMMIT,
        "Recent commits",
        None,
        |_, _| {},
        |ui| {
            let n = commits.len();
            for (i, c) in commits.iter().enumerate() {
                let w = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
                let p = ui.painter();
                let dot = pos2(r.left() + 8.0, r.top() + 14.0);
                if i + 1 < n {
                    p.line_segment([dot + vec2(0.0, 6.0), pos2(dot.x, r.bottom() + 8.0)], egui::Stroke::new(1.5, theme::white(0.08)));
                }
                p.circle_filled(dot, 4.5, if i == 0 { theme::ACCENT } else { theme::INK_600 });
                let x = r.left() + 26.0;
                widgets::text_truncated(p, pos2(x, r.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(&c[1], 13.0).weight(weight::MEDIUM), w - 26.0);
                let hash = Txt::new(&c[0], 11.0).mono().color(theme::FG_2);
                let hr = widgets::text(p, pos2(x, r.top() + 33.0), Align2::LEFT_CENTER, &hash);
                widgets::text_truncated(
                    p,
                    pos2(hr.right() + 8.0, r.top() + 33.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(format!("{} · {}", c[2], c[3]), 11.5).color(theme::FG_3),
                    w - 26.0 - hr.width() - 8.0,
                );
            }
        },
    );
}
