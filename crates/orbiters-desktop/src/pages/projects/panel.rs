//! The selected project's panel: beside the cards on wide windows (its left edge drags to resize it; a double click
//! gives it its own width back), a sheet sliding over them on narrow ones. A hero card (the project's avatars as its
//! cover, where it is, how much it takes on disk, opening it; Duplicate…, Hide in its menu), then cards for its avatars
//! (N4N0 and My Avatar for each, its page on VRChat, its Blender files) or, for a world project, its worlds
//! ([`super::world_card`]), Mixer while the project
//! does not have it, its packages (their own icons, versions in dropdowns), the packages it could add, and its Git
//! repository (branch, changes, remote page, last commits on a timeline). Switching projects crossfades the cards.

use orbiters_i18n::{mark, tr, tr_ctx, tr_n};
use super::art::{self, glyph};
use super::state::{server, Outcome, PackageIcon, State, MIXER, MIXER_PAGE};
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
    // The scroll area reaches into the panel's right gutter, where egui puts its bar (a vertical bar sits at the
    // area's own right edge; `scroll_bar_rect` only gives its height), and keeps the cards out of it by a margin.
    let mut pu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(8.0, 8.0), rect.max - vec2(3.0, 8.0))).id_salt("project-panel"));
    pu.set_clip_rect(rect.shrink(2.0).intersect(ui.clip_rect()));
    let packages_y = Id::unique("project-panel-packages-y");
    let mixer_y = Id::unique("project-panel-mixer-y");
    let commits_y = Id::unique("project-panel-commits-y");
    let bar = Rect::from_min_max(pos2(rect.right() - 13.0, rect.top() + 18.0), pos2(rect.right() - 3.0, rect.bottom() - 18.0));
    let mut scroll = egui::ScrollArea::vertical().id_salt(("project-panel-scroll", &project.path)).auto_shrink([false, false]).scroll_bar_rect(bar);
    if state.scroll_to_packages || state.scroll_to_mixer || state.scroll_to_commits {
        let at = if state.scroll_to_commits {
            commits_y
        } else if state.scroll_to_mixer {
            mixer_y
        } else {
            packages_y
        };
        if let Some(y) = ctx.data(|d| d.get_temp::<f32>(at)) {
            scroll = scroll.vertical_scroll_offset((y - 64.0).max(0.0));
            state.scroll_to_packages = false;
            state.scroll_to_mixer = false;
            state.scroll_to_commits = false;
        }
    }
    let from = orbiters_ui::blur::next_shape(ui);
    let out = scroll.show(&mut pu, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { right: 11, ..Default::default() }).show(ui, |ui| {
            ui.multiply_opacity(k.clamp(0.0, 1.0));
            ui.add_space(10.0 * (1.0 - k).max(0.0));
            ui.spacing_mut().item_spacing = vec2(0.0, 10.0);
            let top = ui.cursor().top();
            hero(ui, cx, state, &project, !split.docked);
            super::storage_card::card(ui, state, &project);
            if state.modal.is_none() {
                super::ui::errors(ui, state, "panel");
            }
            // Which of their pictures the member can change on VRChat, and those changing.
            super::vrchat_picture::poll(&ctx, cx, state);
            // A world project has its worlds where an avatar project has its avatars.
            if world_project(&project) {
                super::world_card::card(ui, cx, state, &project);
            } else {
                avatars(ui, cx, state, &project);
            }
            ctx.data_mut(|d| d.insert_temp(mixer_y, ui.cursor().top() - top));
            mixer(ui, cx, state, &project);
            ctx.data_mut(|d| d.insert_temp(packages_y, ui.cursor().top() - top));
            super::packages_card::packages(ui, state, &project);
            available(ui, state, &project);
            ctx.data_mut(|d| d.insert_temp(commits_y, ui.cursor().top() - top));
            commits(ui, cx, state, &project);
            ui.add_space(8.0);
        });
    });
    // The list's cut follows the frame's corners, at all four and at any scroll: over the content, under the bar that
    // floats over it. The left and bottom of what is clipped are as the scroll area's, its right where the cards end (the
    // gutter beyond holds the scroll bar and no content).
    let cut = Rect::from_min_max(rect.min + vec2(8.0, 8.0), pos2(rect.right() - 3.0 - 11.0, rect.bottom() - 8.0));
    paint::round_clipped_corners(ui.painter(), rect, theme::squircle(24.0), cut, theme::INK_900);
    sticky_bar(ui, cx, state, &project, rect, from, out.state.offset().y, !split.docked);
}

/// Once the hero card scrolled away, the project's name and Open in Unity stay at the panel's top over the cards
/// passing under it, blurred (the large title that becomes a bar). A sheet keeps its × there too.
#[allow(clippy::too_many_arguments)]
fn sticky_bar(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project, rect: Rect, from: egui::layers::ShapeIdx, scrolled: f32, sheet: bool) {
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
    let b = Button::new(tr("Open in Unity"))
        .icon(icons::PLAY)
        .variant(Variant::Primary)
        .size(Size::Sm)
        .enabled(project.registered && !project.missing && !state.running() && !state.repairing(&project.path) && k > 0.5)
        .id_salt("panel-bar-open");
    let open = project.registered && state.session(&project.path).open();
    let bw = if open { 280.0_f32.min(band.width() * 0.6) } else { b.desired_width(&bu) };
    let mut right = band.right() - 20.0;
    if sheet {
        let close = Rect::from_center_size(pos2(right - 16.0, band.center().y + dy), vec2(32.0, 32.0));
        let pressed = widgets::activated(&IconButton::new(icons::X, mark("Close")).size(32.0, 15.0).id_salt("panel-bar-close").show_at(&mut bu, close));
        if pressed && k > 0.5 {
            state.panel_closing = true;
        }
        right = close.left() - 8.0;
    }
    let br = Rect::from_min_size(pos2(right - bw, band.center().y - 16.0 + dy), vec2(bw, 32.0));
    let thumb = Rect::from_min_size(pos2(band.left() + 18.0, band.center().y - 16.0 + dy), vec2(32.0, 32.0));
    art::cover(bu.painter(), state, project, thumb, [9.0; 4], 1.0);
    widgets::text_truncated(
        bu.painter(),
        pos2(thumb.right() + 10.0, band.center().y + dy),
        Align2::LEFT_CENTER,
        &Txt::new(&project.name, 15.0).weight(weight::SEMIBOLD),
        br.left() - thumb.right() - 22.0,
    );
    if open {
        if k > 0.5 {
            super::unity_session::controls(&mut bu, state, project, br, "bar");
        }
    } else if project.registered && widgets::activated(&b.show_at(&mut bu, br)) {
        state.open_unity(&ctx, project, false);
    }
}

/// A world project (by its SDK, or by what its scenes were uploaded as): its panel has the Worlds card.
fn world_project(project: &Project) -> bool {
    project.kind == orbiters_projects::discovery::Kind::Worlds || (project.avatars.is_empty() && !project.worlds.is_empty())
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
        .on_hover_text(tr("Drag to resize, double-click for the usual width"));
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
    super::preview::cover(ui, cx, state, project, cover, [theme::CARD_RADIUS, theme::CARD_RADIUS, 0.0, 0.0], sheet);
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
        if widgets::activated(&IconButton::new(icons::X, mark("Close")).size(32.0, 15.0).backdrop(theme::black(0.55)).show_at(ui, close)) {
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
    let edited = tr!("Edited {when}", when = crate::pages::ago(ui.ctx(), project.modified, orbiters_projects::discovery::now()));
    for (g, label) in [(Some(icons::CUBE), format!("Unity {}", project.unity)), (Some(icons::CLOCK), edited)] {
        px += widgets::pill_at(&p, ui, pos2(px, y + 11.0), g, &label) + 6.0;
    }
    if project.missing {
        widgets::pill_at(&p, ui, pos2(px, y + 11.0), Some(icons::WARNING), tr("Folder missing"));
    } else {
        size_pill(ui, state, project, pos2(px, y + 11.0), r.right() - 16.0);
    }
    y += 22.0 + 16.0;

    // Opening it (or adding it first, for a project found on the computer).
    let row = Rect::from_min_size(pos2(x, y), vec2(w - 32.0, 40.0));
    let round = |i: usize| Rect::from_min_size(pos2(row.right() - 40.0 * (i + 1) as f32 - 8.0 * i as f32, row.top()), vec2(40.0, 40.0));
    let busy = state.running();
    let main_w = row.width() - 3.0 * 48.0;
    let main = Rect::from_min_size(row.min, vec2(main_w, 40.0));
    // Open in Unity already: Show, Save & Exit and Kill in its place, across the row up to the menu (Unity Hub is not
    // needed then; Open the folder moves into the menu).
    let open = project.registered && state.session(&project.path).open();
    if open {
        let wide = Rect::from_min_max(row.min, pos2(round(0).left() - 8.0, row.bottom()));
        super::unity_session::controls(ui, state, project, wide, "hero");
    } else if project.registered {
        let moving = state.repairing(&project.path);
        let b = Button::new(if moving { tr("Moving its Library…") } else { tr("Open in Unity") })
            .icon(icons::PLAY)
            .variant(Variant::Primary)
            .width(main_w)
            .enabled(!project.missing && !busy && !moving)
            .id_salt("panel-open");
        if widgets::activated(&b.show_at(ui, main)) {
            state.open_unity(&ctx, project, false);
        }
    } else {
        let b = Button::new(tr("Add to my projects")).icon(icons::PLUS).variant(Variant::Primary).width(main_w).enabled(!busy).id_salt("panel-add");
        if widgets::activated(&b.show_at(ui, main)) {
            let path = project.path.clone();
            state.task(&ctx, tr("Adding the project…"), move || {
                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, true))?;
                Ok(Outcome::Added(path, tr("Project added: VCC and ALCOM list it too").into()))
            });
        }
    }
    let folder = IconButton::new(icons::FOLDER_OPEN, mark("Open the folder")).size(40.0, 17.0).backdrop(theme::control()).id_salt("panel-folder");
    if !open && !project.missing && widgets::activated(&folder.show_at(ui, round(2))) {
        let path = project.path.clone();
        state.task(&ctx, tr("Opening the folder…"), move || {
            orbiters_projects::reveal(&path)?;
            Ok(Outcome::Picked(None))
        });
    }
    let hub = IconButton::new(icons::CUBE, mark("Open Unity Hub")).size(40.0, 17.0).backdrop(theme::control()).id_salt("panel-hub");
    if !open && widgets::activated(&hub.show_at(ui, round(1))) {
        state.task(&ctx, tr("Opening Unity Hub…"), move || {
            orbiters_projects::open_hub()?;
            Ok(Outcome::Picked(None))
        });
    }
    let menu_r = round(0);
    state.anchors.insert("project-menu".into(), menu_r);
    // The actions the cards' right-click menus have too (`menus`).
    let acts = super::menus::project_items(state, project, false);
    let items: Vec<Item> = acts.iter().map(|(i, _)| *i).collect();
    if let Some(act) = popup::menu_button(ui, "project-menu", menu_r, icons::DOTS_THREE, tr("More"), &items).map(|k| acts[k].1) {
        super::menus::project_act(&ctx, cx, state, project, act, menu_r, sheet);
    }
}

/// How much the project takes on disk, its left-centre at `at` (no further right than `right`): what Library takes of
/// it too when there is room (with all of it on hover), and "Measuring…" until the first answer.
fn size_pill(ui: &mut Ui, state: &mut State, project: &Project, at: egui::Pos2, right: f32) {
    let ctx = ui.ctx().clone();
    state.want_size(&ctx, &project.path);
    let measuring = state.measuring(&project.path);
    let p = ui.painter().clone();
    let Some(s) = state.sizes.get(&project.path).copied() else {
        if measuring || state.fixture {
            // "Measuring…" with its spinner, or the spinner alone where the words do not fit.
            let t = Txt::new(tr("Measuring…"), 11.5).color(theme::FG_3);
            let full = Rect::from_min_size(pos2(at.x, at.y - 10.0), vec2(widgets::text_size(ui, &t).x + 32.0, 20.0));
            let r = if full.right() <= right { full } else { Rect::from_min_size(full.min, vec2(28.0, 20.0)) };
            if r.right() <= right {
                feedback::shimmer(&p, r, 10.0);
                feedback::spinner(&p, pos2(r.left() + 14.0, r.center().y), 4.5, 1.5, theme::FG_3);
                if r == full {
                    widgets::text(&p, pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &t);
                }
                let _ = ui.interact(r, Id::unique(("project-size", &project.path)), Sense::hover()).on_hover_text(tr("Measuring how much the project takes on disk…"));
            }
        }
        return;
    };
    let total = format::bytes(s.total as f64);
    let library = format::bytes(s.library as f64);
    let wide = tr!("{total} · Library {library}", total = total, library = library);
    let width = |label: &str| widgets::text_size(ui, &Txt::new(label, 11.5)).x + 32.0;
    let label = if s.library > 0 && at.x + width(&wide) <= right {
        wide
    } else if at.x + width(&total) <= right {
        total.clone()
    } else {
        return;
    };
    let w = widgets::pill_at(&p, ui, at, Some(icons::HARD_DRIVE), &label);
    let files = format::integer(s.files as f64);
    let mut hint = if s.library > 0 {
        tr!("{total} on disk in {files} files. Library takes {library} of it: Unity makes it again from the project when it is missing.", total = total, files = files, library = library)
    } else {
        tr!("{total} on disk in {files} files.", total = total, files = files)
    };
    if measuring {
        hint.push(' ');
        hint.push_str(tr("Measuring again…"));
    }
    let _ = ui.interact(Rect::from_min_size(pos2(at.x, at.y - 10.0), vec2(w, 20.0)), Id::unique(("project-size", &project.path)), Sense::hover()).on_hover_text(hint);
}

/// The project's uploaded avatars, each with N4N0 and My Avatar to add.
fn avatars(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let count = (!project.avatars.is_empty()).then(|| project.avatars.len().to_string());
    art::card(
        ui,
        "avatars",
        icons::PERSON_ARMS_SPREAD,
        tr("Avatars"),
        count,
        |_, _| {},
        |ui| {
            if project.avatars.is_empty() {
                let text = if project.kind == orbiters_projects::discovery::Kind::Worlds {
                    tr("A world project: N4N0 goes on avatars.")
                } else {
                    tr("No uploaded avatar yet. Upload one from the VRChat SDK panel, save its scene, and it shows up here.")
                };
                let g = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::FG_3).line_height(19.0).job(ui.available_width()));
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 4.0), Sense::hover());
                ui.painter().galley(r.min, g, theme::FG_3);
                return;
            }
            let worn = cx.snap.n4n0.avatar_id.clone();
            for (i, a) in project.avatars.iter().enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                let w = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(w, 92.0), Sense::hover());
                let thumb = Rect::from_min_size(pos2(r.left(), r.top() + 6.0), vec2(64.0, 64.0));
                super::vrchat_picture::thumbnail(ui, cx, state, a, thumb, [14.0; 4], i);
                let x = thumb.right() + 14.0;
                // Its page on VRChat, at the row's top right.
                let link = Rect::from_min_size(pos2(r.right() - 32.0, r.top() + 2.0), vec2(32.0, 32.0));
                state.anchors.insert(format!("avatar-vrchat-{i}"), link);
                let url = format!("https://vrchat.com/home/avatar/{}", a.id);
                let open = IconButton::new(icons::ARROW_SQUARE_OUT, mark("Open on VRChat")).size(32.0, 15.0).backdrop(theme::control()).id_salt(("avatar-vrchat", &a.id));
                if widgets::activated(&open.show_at(ui, link).on_hover_text(&url)) {
                    if let Err(e) = crate::controller::open_url(&url) {
                        cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, tr!("The page could not open: {error}", error = e));
                    }
                }
                let p = ui.painter();
                let nr = widgets::text_truncated(
                    p,
                    pos2(x, thumb.top() + 11.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(&a.name, 14.5).weight(weight::SEMIBOLD),
                    link.left() - x - 88.0,
                );
                if worn.as_ref() == Some(&a.id) {
                    let t = Txt::new(tr("Wearing"), 10.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
                    let tw = widgets::text_size(ui, &t).x + 16.0;
                    let pr = Rect::from_min_size(pos2(nr.right() + 8.0, nr.center().y - 10.0), vec2(tw, 20.0));
                    p.rect_filled(pr, theme::radius(10.0), theme::alpha(theme::ACCENT, 0.14));
                    widgets::text(p, pr.center(), Align2::CENTER_CENTER, &t);
                }
                widgets::text_truncated(p, pos2(x, thumb.top() + 30.0), Align2::LEFT_CENTER, &Txt::new(&a.scene, 11.5).color(theme::FG_3), link.left() - 8.0 - x);
                let enabled = project.registered && !state.running();
                let nano = Button::new(tr("Add N4N0")).icon(icons::N4N0_MARK).size(Size::Sm).enabled(enabled).id_salt(("add-n4n0", &a.id, &a.scene));
                let nw = nano.desired_width(ui);
                let nr = Rect::from_min_size(pos2(x, thumb.top() + 46.0), vec2(nw, 32.0));
                state.anchors.insert(format!("add-n4n0-{i}"), nr);
                let hint = if project.registered { tr("My Avatar, Toolkit and N4N0: Unity asks you to confirm") } else { tr("Add the project to your list first") };
                if widgets::activated(&nano.show_at(ui, nr).on_hover_text(hint)) {
                    state.modal_fill = theme::control();
                    state.install(&ctx, project.clone(), a.clone(), true, server(cx), nr);
                }
                let mine = Button::new(tr("My Avatar only")).icon(icons::PERSON).variant(Variant::Ghost).size(Size::Sm).enabled(enabled).id_salt((
                    "add-my-avatar",
                    &a.id,
                    &a.scene,
                ));
                let mw = mine.desired_width(ui);
                let mr = Rect::from_min_size(pos2(nr.right() + 6.0, nr.top()), vec2(mw, 32.0));
                if mr.right() <= r.right() + 1.0 && widgets::activated(&mine.show_at(ui, mr).on_hover_text(tr("Installs My Avatar in the project, without N4N0"))) {
                    state.modal_fill = theme::INK_850;
                    state.install(&ctx, project.clone(), a.clone(), false, server(cx), mr);
                }
                // Under the avatar's words, beside its picture's column.
                super::blend::rows(ui, state, a, thumb.width() + 14.0);
                // A right press anywhere on the row: what it offers.
                let whole = Rect::from_min_max(r.min, pos2(r.right(), ui.min_rect().bottom().max(r.bottom())));
                let row = ui.interact(whole, Id::unique(("avatar-row", &a.id, &a.scene)), Sense::hover());
                super::menus::row_menu(ui, cx, state, project, a, &row);
            }
        },
    );
}

/// Mixer, while the project does not have it: what it is in a line and Add Mixer; behind the info button (which turns
/// into a close button) what it adds, opening in place, with its page on orbiters.cc. It grows in once the project's
/// packages are known and folds away once Mixer is in.
fn mixer(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let known = !state.view_loading() || !state.view.packages.is_empty();
    let installed = state.view.packages.iter().any(|p| p.name == MIXER && p.installed.is_some());
    let show = known && !installed && !project.missing;
    let k = anim::spring_from(&ctx, Id::unique(("mixer-card", &project.path)), [0.0], [if show { 1.0 } else { 0.0 }], Spring::SOFT)[0].clamp(0.0, 1.0);
    if k < 0.005 {
        return;
    }
    state.icons_wanted.push(MIXER.into());
    let icon = state.icons.get(MIXER).cloned();
    // Drawn whole, shown as far as it has grown: the card's height last frame, the panel's gap folding with it.
    let height_id = Id::unique(("mixer-card-height", &project.path));
    let full = ctx.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(132.0);
    let w = ui.available_width();
    let top = ui.cursor().min;
    let shown = Rect::from_min_size(top, vec2(w, full * k));
    let mut cu = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(top, vec2(w, 100_000.0)))
            .layout(egui::Layout::top_down(egui::Align::Min))
            .id_salt(("mixer", &project.path)),
    );
    cu.set_clip_rect(shown.expand2(vec2(4.0, 0.0)).intersect(ui.clip_rect()));
    cu.multiply_opacity(k);
    let busy = state.running();
    let adding = state.busy_package.as_deref() == Some(MIXER) && busy;
    let info_id = Id::unique("mixer-info");
    let info_open = state.mixer_info;
    let mut toggle_info = false;
    let mut info_rect = Rect::NOTHING;
    art::card_with(
        &mut cu,
        "mixer",
        |p, t| art::package_icon(p, icon.as_ref(), t, MIXER),
        "Mixer",
        None,
        |ui, slot| {
            info_rect = Rect::from_min_size(pos2(slot.right() - 32.0, slot.center().y - 16.0), vec2(32.0, 32.0));
            toggle_info = info_button(ui, info_id, info_rect, info_open);
        },
        |ui| {
            let text = Txt::new(tr("Improves Unity's UI and adds features including Git management, sculpting, weight painting, mesh editing and more."), 12.5)
                .color(theme::FG_3)
                .line_height(19.0);
            let g = ui.painter().layout_job(text.job(ui.available_width()));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, theme::FG_3);
            let label = if adding { tr("Adding Mixer…") } else { tr("Add Mixer") };
            let b = Button::new(label).icon(icons::PLUS).size(Size::Sm).enabled(project.registered && !busy).id_salt(("add-mixer", &project.path));
            let bw = b.desired_width(ui);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
            let br = Rect::from_min_size(row.min, vec2(bw, 32.0));
            state.anchors.insert("add-mixer".into(), br);
            let hint = if project.registered { tr("Installs Mixer from the Orbiters repository") } else { tr("Add the project to your list first") };
            if widgets::activated(&b.show_at(ui, br).on_hover_text(hint)) {
                state.add_mixer(&ctx, project);
            }
            if adding {
                feedback::spinner(ui.painter(), pos2(br.right() + 18.0, br.center().y), 7.0, 2.0, theme::ACCENT);
            }
            mixer_info(ui, cx, info_id, info_open);
        },
    );
    state.anchors.insert("mixer-info".into(), info_rect);
    if toggle_info {
        state.mixer_info = !state.mixer_info;
    }
    let h = cu.min_rect().height();
    ctx.data_mut(|d| d.insert_temp(height_id, h));
    // The panel's gap after the card folds with it.
    ui.allocate_exact_size(vec2(w, (h * k - 10.0 * (1.0 - k)).max(0.0)), Sense::hover());
}

/// The round info button of Mixer's card; open, its "i" turns into a close mark on a green-tinted disc. True when
/// pressed.
fn info_button(ui: &mut Ui, id: Id, r: Rect, open: bool) -> bool {
    let ctx = ui.ctx().clone();
    let resp = ui.interact(r, id.with("button"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(if open {
        tr("Hide what Mixer adds")
    } else {
        tr("What Mixer adds")
    });
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let o = anim::toggle(&ctx, id.with("open"), open, Spring::BOUNCY);
    let p = ui.painter();
    let c = r.center();
    let radius = 16.0 * (1.0 - 0.06 * press);
    let bg = theme::mix(theme::mix(theme::control(), theme::INK_750, hover), theme::alpha(theme::ACCENT, 0.16), o.clamp(0.0, 1.0));
    p.circle_filled(c, radius, bg);
    // The two glyphs cross: the "i" shrinks and fades as the close mark grows in, both turning a quarter.
    let a = o.clamp(0.0, 1.0);
    let fg = theme::mix(theme::mix(theme::FG_2, theme::FG, hover), theme::ACCENT, a);
    if a < 0.99 {
        widgets::icon(p, c, icons::INFO, 17.0 * (1.0 - 0.4 * a), theme::alpha(fg, 1.0 - a));
    }
    if a > 0.01 {
        widgets::icon(p, c, icons::X, 14.0 * (0.6 + 0.4 * o.clamp(0.0, 1.2)), theme::alpha(fg, a));
    }
    widgets::activated(&resp)
}

/// What Mixer adds, opening in place under Add Mixer (its height springs open, the words come down into it), and the
/// way to its page on orbiters.cc.
fn mixer_info(ui: &mut Ui, cx: &mut Cx, id: Id, open: bool) {
    let ctx = ui.ctx().clone();
    let o = anim::toggle(&ctx, id.with("section"), open, Spring::SOFT);
    if o < 0.005 {
        return;
    }
    let w = ui.available_width();
    let text = Txt::new(
        tr("Mixer adds new features to the Unity Editor such as sculpting, weight painting, mesh editing and syncing with a Blender project, and improves the existing panels."),
        13.0,
    )
    .color(theme::FG_2)
    .line_height(20.0);
    let g = ui.painter().layout_job(text.job(w - 32.0));
    let full = 14.0 + g.size().y + 10.0 + 28.0 + 10.0;
    let (area, _) = ui.allocate_exact_size(vec2(w, full * o.clamp(0.0, 1.0)), Sense::hover());
    let well = Rect::from_min_size(area.min, vec2(w, full));
    let mut iu = ui.new_child(UiBuilder::new().max_rect(well).id_salt("mixer-info-section"));
    iu.set_clip_rect(area.intersect(ui.clip_rect()));
    let p = iu.painter().clone();
    widgets::frame(&p, well, 14.0, theme::white(0.035));
    let a = o.clamp(0.0, 1.0);
    let drop = 10.0 * (1.0 - a);
    let mut tp = p.clone();
    tp.multiply_opacity(a);
    tp.galley(pos2(well.left() + 16.0, well.top() + 14.0 - drop), g, theme::FG_2);
    // The page: an accent link that brightens on hover, its arrow nudging out.
    let link = Txt::new(tr("Mixer's presentation on orbiters.cc"), 12.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
    let lw = widgets::text_size(&iu, &link).x + 24.0;
    let lr = Rect::from_min_size(pos2(well.left() + 10.0, well.bottom() - 10.0 - 28.0 - drop * 0.5), vec2(lw + 12.0, 28.0));
    let resp = iu.interact(lr, id.with("link"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(MIXER_PAGE);
    let hover = anim::toggle(&ctx, id.with("link-hover"), resp.hovered(), Spring::SOFT);
    tp.rect_filled(lr, theme::radius(14.0), theme::alpha(theme::ACCENT, 0.1 * hover));
    widgets::text(&tp, pos2(lr.left() + 6.0, lr.center().y), Align2::LEFT_CENTER, &link);
    widgets::icon(&tp, pos2(lr.right() - 14.0 + 2.0 * hover, lr.center().y - 2.0 * hover), icons::ARROW_SQUARE_OUT, 13.0, theme::ACCENT);
    if widgets::activated(&resp) && a > 0.5 {
        if let Err(e) = crate::controller::open_url(MIXER_PAGE) {
            cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, tr!("The page could not open: {error}", error = e));
        }
    }
}

/// A package's row: its icon, names, and on the right what `control` draws (given the rect left for it).
pub(super) fn package_row(ui: &mut Ui, icon: Option<&PackageIcon>, name: &str, display: &str, control_w: f32, control: impl FnOnce(&mut Ui, Rect)) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
    let p = ui.painter();
    let t = Rect::from_min_size(pos2(r.left(), r.center().y - 17.0), vec2(34.0, 34.0));
    art::package_icon(p, icon, t, name);
    let x = t.right() + 12.0;
    let tw = r.right() - control_w - 10.0 - x;
    let title = Txt::new(display, 13.5).weight(weight::MEDIUM);
    widgets::text_truncated(p, pos2(x, r.center().y - 8.0), Align2::LEFT_CENTER, &title, tw);
    widgets::text_truncated(p, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(name, 11.0).color(theme::FG_3), tw);
    // A name cut short (narrow panels) shows whole on hover.
    if widgets::text_size(ui, &title).x > tw {
        let _ = ui
            .interact(Rect::from_min_max(pos2(x, r.top()), pos2(x + tw, r.bottom())), Id::unique(("package-name", name)), Sense::hover())
            .on_hover_text(display);
    }
    let c = Rect::from_min_max(pos2(r.right() - control_w, r.top()), r.max);
    control(ui, c);
}

/// Shimmering rows while the packages are read.
pub(super) fn row_skeletons(ui: &mut Ui, n: usize) {
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
pub(super) fn versions<'a>(p: &'a vpm::Package, prerelease: bool) -> Vec<&'a String> {
    p.versions.iter().filter(|v| prerelease || !v.contains('-') || Some(v.as_str()) == p.installed.as_deref()).collect()
}

fn available(ui: &mut Ui, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    if state.view_loading() && state.view.packages.is_empty() {
        return;
    }
    let find = orbiters_core::search::Query::new(&state.package_query);
    let list: Vec<vpm::Package> = state
        .view
        .packages
        .iter()
        .filter(|p| p.installed.is_none() && !versions(p, state.prerelease).is_empty())
        .filter(|p| find.matches_any([p.name.as_str(), p.display_name.as_str()]))
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
        tr("Add packages"),
        Some(all.to_string()),
        |_, _| {},
        |ui| {
            widgets::search_field(ui, "package-search", &mut state.package_query, tr("Search the repositories' packages"), ui.available_width());
            ui.add_space(2.0);
            if list.is_empty() {
                widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 12.0), Align2::LEFT_CENTER, &Txt::new(tr("No package matches."), 12.5).color(theme::FG_3));
                ui.add_space(24.0);
            }
            let shown = if state.all_packages || !find.is_empty() { list.len() } else { list.len().min(AVAILABLE_FIRST) };
            for (i, pkg) in list.iter().take(shown).enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                let vs = versions(pkg, state.prerelease);
                let id = Id::unique(("available-version", &project.path, &pkg.name));
                let chosen = ctx.data(|d| d.get_temp::<String>(id)).filter(|v| vs.iter().any(|x| *x == v)).unwrap_or_else(|| vs[0].clone());
                let working = state.busy_package.as_deref() == Some(pkg.name.as_str());
                state.icons_wanted.push(pkg.name.clone());
                let icon = state.icons.get(&pkg.name).cloned();
                package_row(ui, icon.as_ref(), &pkg.name, &pkg.display_name, 100.0 + 8.0 + 32.0, |ui, c| {
                    let dd = Rect::from_min_size(pos2(c.left(), c.center().y - 15.0), vec2(100.0, 30.0));
                    let add = Rect::from_min_size(pos2(c.right() - 32.0, c.center().y - 16.0), vec2(32.0, 32.0));
                    let items: Vec<Item> = vs.iter().map(|v| if v.contains('-') { Item::new(v).detail(tr("Pre-release")) } else { Item::new(v) }).collect();
                    let selected = vs.iter().position(|v| **v == chosen);
                    if let Some(k) = popup::dropdown(ui, ("available", &pkg.name), dd, &chosen, &items, selected, !busy) {
                        ctx.data_mut(|d| d.insert_temp(id, vs[k].clone()));
                    }
                    if working {
                        feedback::spinner(ui.painter(), add.center(), 8.0, 2.0, theme::ACCENT);
                    } else {
                        let b = IconButton::new(icons::PLUS, mark("Add to the project")).size(32.0, 15.0).backdrop(theme::control()).id_salt(("add", &pkg.name));
                        if !busy && widgets::activated(&b.show_at(ui, add)) {
                            let change = vpm::Change::Install { names: vec![(pkg.name.clone(), Some(chosen.clone()))], prerelease: state.prerelease };
                            package_action(state, &ctx, project, change, &tr!("Adding {package} {version}…", package = pkg.display_name, version = chosen), &pkg.name);
                        }
                    }
                });
            }
            if list.len() > shown || (state.all_packages && list.len() > AVAILABLE_FIRST && find.is_empty()) {
                let label = if state.all_packages { tr("Show fewer").to_string() } else { tr!("Show all {count}", count = list.len()) };
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

pub(super) fn package_action(state: &mut State, ctx: &egui::Context, p: &Project, change: vpm::Change, label: &str, name: &str) {
    let path = p.path.clone();
    state.busy_package = (!name.is_empty()).then(|| name.to_string());
    state.last_change = Some((path.clone(), change.clone()));
    state.plan_open = false;
    state.task(ctx, label, move || {
        match orbiters_projects::runtime()?.block_on(vpm::change(&orbiters_projects::vcc_dir(), &path, change)) {
            Ok(()) => Ok(Outcome::Changed(tr("Packages updated").into())),
            // Not cancelled: the plan making it coherent, in its dialog.
            Err(e) => match e.downcast::<orbiters_projects::coherent::Conflicted>() {
                Ok(c) => Ok(Outcome::Conflict(path, c.plan)),
                Err(e) => Err(e),
            },
        }
    });
}

/// The project's Git repository: its branch, how it stands against its remote (ahead, behind, up to date), the changes
/// not committed yet, the remote's page, and its last commits on a timeline. Each commit's author is a disc on the
/// line; a press opens a commit to its whole message (its height springs open), its hash copies with a press.
fn commits(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let Some(repo) = state.git.clone() else { return };
    if repo.commits.is_empty() && repo.branch.is_none() {
        return;
    }
    // The rows come in one after the other the first time this repository's history shows.
    let start_id = Id::unique(("commits-start", &project.path, repo.commits.first().map(|c| c.hash.clone())));
    let now_t = anim::time(&ctx);
    let start = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(start_id, || now_t));
    let mut open_remote = false;
    art::card(
        ui,
        "commits",
        glyph::GIT_COMMIT,
        tr("Recent commits"),
        None,
        |ui, slot| {
            let Some(web) = &repo.web else { return };
            let host = repo.host.as_deref().unwrap_or_default();
            let (g, label) = match host {
                "github.com" => (glyph::GITHUB, "GitHub".to_string()),
                h if h.contains("gitlab") => (glyph::GITLAB, "GitLab".to_string()),
                h => (icons::ARROW_SQUARE_OUT, h.trim_start_matches("www.").split('.').next().map(capitalised).unwrap_or_else(|| tr("the remote").into())),
            };
            let label = tr!("Open on {label}", label = label);
            let b = Button::new(&label).icon(g).variant(Variant::Ghost).size(Size::Sm).id_salt(("commits-remote", &project.path));
            let bw = b.desired_width(ui).min(slot.width());
            let r = Rect::from_min_size(pos2(slot.right() - bw, slot.center().y - 16.0), vec2(bw, 32.0));
            state.anchors.insert("commits-remote".into(), r);
            open_remote = widgets::activated(&b.show_at(ui, r).on_hover_text(web));
        },
        |ui| {
            git_status(ui, &repo);
            let w = ui.available_width();
            let now = orbiters_projects::discovery::now();
            // The line runs behind the discs: drawn first, once the rows are placed.
            let line = ui.painter().add(egui::Shape::Noop);
            let mut discs: Vec<(egui::Pos2, f32)> = vec![];
            let mut copy = None;
            let mut toggle = None;
            for (i, c) in repo.commits.iter().enumerate() {
                let (a, rise) = widgets::entrance(ui, Id::unique(("commit-in", &project.path, &c.hash)), start, i);
                let id = Id::unique(("commit", &project.path, &c.hash));
                let open = state.commit_open.contains(&c.hash);
                let o = anim::toggle(&ctx, id.with("open"), open, Spring::EXPAND).max(0.0);
                // The whole message, wrapped, and the full hash with its date under it when open.
                let text_w = w - 44.0 - 10.0;
                let message = if c.body.is_empty() { c.subject.clone() } else { format!("{}\n\n{}", c.subject, c.body) };
                let g = ui.painter().layout_job(Txt::new(&message, 12.5).color(theme::FG_2).line_height(19.0).job(text_w - 24.0));
                let well_h = 12.0 + g.size().y + 10.0 + 18.0 + 12.0;
                let row_h = 54.0 + (well_h + 6.0) * o.clamp(0.0, 1.0);
                let (row, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
                let head = Rect::from_min_size(row.min, vec2(w, 54.0));
                let resp = ui.interact(head, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
                let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
                let mut p = ui.painter().clone();
                p.multiply_opacity(a);
                let dy = rise;
                let back = Rect::from_min_max(pos2(row.left() - 6.0, row.top() + dy), pos2(row.right() + 6.0, row.bottom() + dy));
                let fill = theme::white(0.035 * hover.max(o.clamp(0.0, 1.0)) + 0.02 * press);
                if fill.a() > 0 {
                    p.rect_filled(back, theme::radius(14.0), fill);
                }
                // The author's disc on the line; the latest commit's ringed in green.
                let center = pos2(row.left() + 16.0, row.top() + 27.0 + dy);
                discs.push((center, a));
                let radius = 13.0 * (1.0 + 0.06 * hover - 0.05 * press);
                p.circle_filled(center, radius + 3.0, theme::surface());
                if i == 0 {
                    let pulse = anim::since(&ctx, start + 0.25, 1.2) as f32;
                    let e = orbiters_ui::anim::ease::out_cubic(pulse.clamp(0.0, 1.0));
                    if pulse < 1.0 {
                        p.circle_filled(center, radius + 3.0 + 9.0 * e, theme::alpha(theme::ACCENT, 0.3 * (1.0 - e)));
                    }
                    p.circle_stroke(center, radius + 2.0, egui::Stroke::new(1.6, theme::ACCENT));
                }
                widgets::avatar(&p, Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0)), None, &c.author);
                let x = row.left() + 44.0;
                let caret_w = 22.0;
                let subject = Txt::new(&c.subject, 13.0).weight(weight::MEDIUM).color(theme::mix(theme::FG, theme::FG, hover));
                widgets::text_truncated(&p, pos2(x, row.top() + 18.0 + dy), Align2::LEFT_CENTER, &subject, text_w - caret_w);
                // Who and when, and the hash in a pill on the right that copies it.
                let hash = Txt::new(&c.short, 11.0).mono().color(theme::FG_2);
                let hw = widgets::text_size(ui, &hash).x + 14.0;
                let hr = Rect::from_min_size(pos2(row.right() - hw, row.top() + 30.0 + dy), vec2(hw, 18.0));
                let hresp = ui.interact(hr, id.with("hash"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Copy the commit's hash"));
                let hh = anim::toggle(&ctx, id.with("hash-hover"), hresp.hovered(), Spring::SOFT);
                p.rect_filled(hr, theme::radius(9.0), theme::white(0.05 + 0.05 * hh));
                widgets::text(&p, hr.center(), Align2::CENTER_CENTER, &hash);
                if widgets::activated(&hresp) {
                    copy = Some(c.hash.clone());
                }
                let when = format!("{} · {}", c.author, crate::pages::ago(ui.ctx(), c.time, now));
                widgets::text_truncated(&p, pos2(x, row.top() + 39.0 + dy), Align2::LEFT_CENTER, &Txt::new(when, 11.5).color(theme::FG_3), hr.left() - x - 8.0);
                // The caret turns as the commit opens.
                let cc = pos2(row.right() - 9.0, row.top() + 18.0 + dy);
                let turn = o.clamp(0.0, 1.0) * std::f32::consts::PI;
                let rot = |v: egui::Vec2| vec2(v.x * turn.cos() - v.y * turn.sin(), v.x * turn.sin() + v.y * turn.cos());
                let s = 3.4;
                let cf = theme::alpha(theme::FG_3, 0.5 + 0.5 * hover.max(o.clamp(0.0, 1.0)));
                p.line(vec![cc + rot(vec2(-s, -s * 0.5)), cc + rot(vec2(0.0, s * 0.5)), cc + rot(vec2(s, -s * 0.5))], egui::Stroke::new(1.5, cf));
                if !c.subject.is_empty() && widgets::text_size(ui, &subject).x > text_w - caret_w && !open {
                    resp.clone().on_hover_text(&c.subject);
                }
                if widgets::activated(&resp) && !hresp.hovered() {
                    toggle = Some(c.hash.clone());
                }
                if o > 0.005 {
                    let well = Rect::from_min_size(pos2(x - 4.0, row.top() + 54.0 + dy), vec2(text_w + 4.0, well_h));
                    let mut wp = p.clone();
                    wp.set_clip_rect(Rect::from_min_max(well.min, pos2(well.right(), row.bottom() + dy)).intersect(ui.clip_rect()));
                    let k = o.clamp(0.0, 1.0);
                    wp.multiply_opacity(k);
                    wp.rect_filled(well, theme::radius(12.0), theme::white(0.035));
                    let drop = 8.0 * (1.0 - k);
                    wp.galley(pos2(well.left() + 12.0, well.top() + 12.0 - drop), g, theme::FG_2);
                    let meta = format!("{} · {}", c.hash, date(c.time));
                    widgets::text_truncated(
                        &wp,
                        pos2(well.left() + 12.0, well.bottom() - 12.0 - 9.0 - drop * 0.5),
                        Align2::LEFT_CENTER,
                        &Txt::new(meta, 10.5).mono().color(theme::FG_4),
                        well.width() - 24.0,
                    );
                }
            }
            // The line between the discs, fading out under the last one (older history goes on).
            let mut shapes = vec![];
            for pair in discs.windows(2) {
                let ((a, ka), (b, kb)) = (pair[0], pair[1]);
                shapes.push(egui::Shape::line_segment([a, b], egui::Stroke::new(2.0, theme::white(0.09 * ka.min(kb)))));
            }
            if let Some(&(last, k)) = discs.last() {
                if repo.commits.len() >= orbiters_projects::git::COMMITS {
                    let n = 6;
                    for s in 0..n {
                        let (t0, t1) = (s as f32 / n as f32, (s + 1) as f32 / n as f32);
                        shapes.push(egui::Shape::line_segment(
                            [last + vec2(0.0, 22.0 * t0), last + vec2(0.0, 22.0 * t1)],
                            egui::Stroke::new(2.0, theme::white(0.09 * k * (1.0 - t0))),
                        ));
                    }
                }
            }
            ui.painter().set(line, egui::Shape::Vec(shapes));
            if let Some(hash) = copy {
                ctx.copy_text(hash);
                cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Success, tr("Commit hash copied"));
            } else if let Some(hash) = toggle {
                if !state.commit_open.remove(&hash) {
                    state.commit_open.insert(hash);
                }
            }
        },
    );
    if open_remote {
        if let Some(web) = &repo.web {
            if let Err(e) = crate::controller::open_url(web) {
                cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, tr!("The page could not open: {error}", error = e));
            }
        }
    }
}

/// The repository's state in chips: its branch, how it stands against its upstream, and its changes not committed.
fn git_status(ui: &mut Ui, repo: &orbiters_projects::git::Repo) {
    let ctx = ui.ctx().clone();
    // (glyph, label, colour, a pulsing dot)
    let mut chips: Vec<(Option<&str>, String, Color32, bool)> = vec![];
    chips.push((Some(glyph::GIT_BRANCH), repo.branch.clone().unwrap_or_else(|| tr("Detached").into()), theme::FG, false));
    match &repo.upstream {
        Some(up) if repo.ahead == 0 && repo.behind == 0 => chips.push((Some(icons::CHECK), tr!("Up to date with {upstream}", upstream = up), theme::FG_3, false)),
        Some(_) => {
            if repo.ahead > 0 {
                chips.push((Some(glyph::ARROW_UP), tr!("{count} to push", count = repo.ahead), theme::ACCENT, false));
            }
            if repo.behind > 0 {
                chips.push((Some(glyph::ARROW_DOWN), tr!("{count} to pull", count = repo.behind), theme::BLUE, false));
            }
        }
        None => chips.push((None, tr("Not on a remote").into(), theme::FG_3, false)),
    }
    if repo.changes > 0 {
        let n = repo.changes;
        chips.push((None, tr_n("{count} uncommitted change", "{count} uncommitted changes", n as u64), theme::WARN, true));
    }
    let style = |label: &str, c: Color32| Txt::new(label, 11.5).weight(weight::MEDIUM).color(c);
    let widths: Vec<f32> =
        chips.iter().map(|(g, l, c, dot)| widgets::text_size(ui, &style(l, *c)).x + 20.0 + if g.is_some() || *dot { 16.0 } else { 0.0 }).collect();
    let w = ui.available_width();
    let (places, h) = widgets::flow(&widths, w, 24.0, 6.0);
    let (area, _) = ui.allocate_exact_size(vec2(w, h + 4.0), Sense::hover());
    let p = ui.painter();
    for (i, ((g, label, c, dot), (at, cw))) in chips.iter().zip(places.iter().zip(&widths)).enumerate() {
        let k = anim::spring_from(&ctx, Id::unique(("git-chip", i, label)), [0.0], [1.0], Spring::BOUNCY)[0];
        let r = Rect::from_min_size(area.min + *at, vec2(*cw, 24.0));
        let r = Rect::from_center_size(r.center(), r.size() * (0.92 + 0.08 * k.clamp(0.0, 1.2)));
        let a = k.clamp(0.0, 1.0);
        let tinted = *c != theme::FG && *c != theme::FG_3;
        p.rect_filled(r, theme::radius(12.0), if tinted { theme::alpha(*c, 0.13 * a) } else { theme::white(0.06 * a) });
        let mut x = r.left() + 10.0;
        if *dot {
            let center = pos2(x + 5.0, r.center().y);
            let t = anim::clock(&ctx, 30.0);
            let e = orbiters_ui::anim::ease::out_cubic((t % 1.6 / 1.6) as f32);
            p.circle_filled(center, 3.5 + 4.0 * e, theme::alpha(*c, 0.35 * (1.0 - e) * a));
            p.circle_filled(center, 3.5, theme::alpha(*c, a));
            x += 16.0;
        } else if let Some(g) = g {
            widgets::icon(p, pos2(x + 6.0, r.center().y), g, 12.0, theme::alpha(if *c == theme::FG { theme::FG_3 } else { *c }, a));
            x += 16.0;
        }
        widgets::text(p, pos2(x, r.center().y), Align2::LEFT_CENTER, &style(label, theme::alpha(*c, a)));
    }
}

/// A date as people read it ("29 Oct 2026, 14:02"), in local time when the system gives its offset.
fn date(unix: u64) -> String {
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(unix as i64) else { return String::new() };
    let t = time::UtcOffset::current_local_offset().map(|o| t.to_offset(o)).unwrap_or(t);
    let month = [
        tr_ctx("month", "Jan"),
        tr_ctx("month", "Feb"),
        tr_ctx("month", "Mar"),
        tr_ctx("month", "Apr"),
        tr_ctx("month", "May"),
        tr_ctx("month", "Jun"),
        tr_ctx("month", "Jul"),
        tr_ctx("month", "Aug"),
        tr_ctx("month", "Sep"),
        tr_ctx("month", "Oct"),
        tr_ctx("month", "Nov"),
        tr_ctx("month", "Dec"),
    ][t.month() as usize - 1];
    tr!("{day} {month} {year}, {time}", day = t.day(), month = month, year = t.year(), time = format!("{:02}:{:02}", t.hour(), t.minute()))
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_i18n::{in_language, Language};

    #[test]
    fn a_date_is_written_in_the_language_of_the_window() {
        // The 15th of February 1970, whatever the zone this runs in.
        let unix = 45 * 86_400;
        in_language(Language::En, || assert!(date(unix).contains("Feb 1970"), "{}", date(unix)));
        in_language(Language::Fr, || assert!(date(unix).contains("févr. 1970"), "{}", date(unix)));
        in_language(Language::Ja, || assert!(date(unix).starts_with("1970年2月"), "{}", date(unix)));
        in_language(Language::PtBr, || assert!(date(unix).contains("fev 1970"), "{}", date(unix)));
    }
}
