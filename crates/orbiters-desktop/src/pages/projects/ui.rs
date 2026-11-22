//! The Unity projects page: a static header (the title, the search and the actions on one row), the projects as cards
//! and those found on the computer as rows under them, and the selected project's panel: beside the cards on wide
//! windows (the cards make room for it with a spring), sliding over them on narrow ones. Dialogs grow out of what
//! opened them ([`super::dialogs`]).

use orbiters_i18n::{mark, tr, tr_n};
use super::art;
use super::menus;
use super::panel;
use super::state::{server, Modal, Outcome, State};
use crate::pages::header::{Action, Header};
use crate::pages::{self, Cx, PAD_X};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use std::path::PathBuf;
use orbiters_projects::{discovery::Project, vpm};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::layout::{self, PANE_GAP};
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{feedback, format, icons, logo, popup};
use std::sync::atomic::Ordering;

pub use super::dialogs::modals;

/// Narrowest project card, and the space between cards.
const CARD_MIN: f32 = 250.0;
const GAP: f32 = 16.0;
/// What a card has under its cover: name, when it was edited, its pills.
const CARD_BODY: f32 = 96.0;
/// Projects found on the computer listed before "Show all".
const FOUND_FIRST: usize = 6;

fn now() -> u64 {
    orbiters_projects::discovery::now()
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    state.ensure(&ctx);
    if state.visit != cx.visit {
        state.visit = cx.visit;
        // Sizes are looked up again (measured again only if the projects changed).
        state.sizes_asked.clear();
        if state.scan_rx.is_none() {
            state.refresh(&ctx, false);
        }
    }
    state.poll(&ctx, cx);
    let page = ui.max_rect();

    // Short, so that the toolbar keeps to the title's row on narrow windows (what the computer has besides is counted
    // in its own section). Hidden projects are not counted.
    let listed = |p: &&Project| p.registered && !state.is_hidden(&p.path);
    let registered = state.scan.projects.iter().filter(listed).count();
    let avatars: usize = state.scan.projects.iter().filter(listed).map(|p| p.avatars.len()).sum();
    let sub = if state.scanning() && state.scan.projects.is_empty() {
        tr("Looking for Unity projects…").to_string()
    } else if state.scanning() {
        tr!("{projects} · looking for more…", projects = tr_n("{count} project", "{count} projects", registered as u64))
    } else {
        tr!("{projects} · {avatars}", projects = tr_n("{count} project", "{count} projects", registered as u64), avatars = tr_n("{count} avatar", "{count} avatars", avatars as u64))
    };
    let header = Header {
        caption: orbiters_i18n::tr("Create & prepare"),
        title: orbiters_i18n::tr("Unity projects"),
        sub: &sub,
        busy: state.scanning(),
        search: Some(("project-search", tr("Find a project"))),
        actions: vec![
            Action::new("refresh", tr("Look for projects again"), icons::ARROW_COUNTER_CLOCKWISE).icon_only().spinning(state.scanning()),
            Action::new("repos", tr("Package repos"), icons::STACK),
            Action::new("add", tr("Add existing"), icons::FOLDER_OPEN).enabled(!state.running()),
            Action::new("new", tr("New project"), icons::PLUS).primary(),
        ],
    };
    let placed = header.place(ui, page, cx);
    let split = layout::split(page.width(), PAD_X, state.panel_width);

    // A selected project that went away (removed from the list): none.
    if state.selected.as_ref().is_some_and(|s| !state.scan.projects.iter().any(|p| &p.path == s)) && state.reveal.is_none() && !state.fixture {
        state.selected = None;
    }
    // Wide windows always show a project: the most recent one (not hidden) until another is picked.
    if split.docked && (state.selected.is_none() || state.panel_closing) {
        if let Some(p) = state.scan.projects.iter().filter(|p| p.registered && !state.is_hidden(&p.path)).max_by_key(|p| p.modified) {
            let path = p.path.clone();
            state.open(&ctx, path);
        }
    }
    let docked = split.docked && state.selected.is_some();
    // The cards make room for the panel (or take the width back) with a spring.
    let dock = anim::toggle(&ctx, Id::unique("projects-dock"), docked, Spring::SOFT).clamp(0.0, 1.0);
    let panel_w = anim::value(&ctx, Id::unique("projects-panel-w"), split.panel, Spring::SNAPPY);
    let gallery_right = page.right() - PAD_X - (panel_w + PANE_GAP) * dock;
    let edge = PAD_X + (12.0 - PAD_X) * dock;

    let from = orbiters_ui::blur::next_shape(ui);
    let gallery_rect = Rect::from_min_max(page.min, pos2(gallery_right + edge, page.bottom()));
    let mut gu = ui.new_child(UiBuilder::new().max_rect(gallery_rect).id_salt("projects-gallery"));
    let bar = Rect::from_min_max(pos2(gallery_rect.left(), placed.content_top), gallery_rect.max);
    let mut scroll = egui::ScrollArea::vertical().id_salt("unity-projects").auto_shrink([false, false]).scroll_bar_rect(bar);
    if let Some(y) = state.scroll_gallery {
        scroll = scroll.vertical_scroll_offset(y);
    } else if let Some(y) = state.scroll_fix.take() {
        scroll = scroll.vertical_scroll_offset(y);
    }
    state.row_places.clear();
    let out = scroll.show(&mut gu, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: PAD_X as i8, right: edge.round() as i8, top: 0, bottom: 104 }).show(ui, |ui| {
            ui.add_space(placed.content_top - page.top());
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            gallery(ui, cx, state);
        });
    });
    // While a scan adds rows (and when it ends and sorts), the first project in view stays where it is on screen: what
    // grows above it scrolls the gallery by as much (AGENTS.md: progressive galleries keep the visible item).
    let (offset, top) = (out.state.clamped_offset().y, out.inner_rect.top());
    let places: Vec<(PathBuf, f32)> = state.row_places.iter().map(|(p, r)| (p.clone(), r.top() - top + offset)).collect();
    let scanning = state.scanning();
    if scanning || state.was_scanning {
        if let Some(dy) = anchor_shift(state.scroll_anchor.as_ref(), &places) {
            state.scroll_fix = Some((offset + dy).max(0.0));
            ctx.request_repaint();
        }
    }
    state.was_scanning = scanning;
    state.scroll_anchor = first_in_view(&state.row_places, top).and_then(|path| places.iter().find(|(p, _)| *p == path).cloned());
    // Across the whole page, not the cards' width only: the docked panel starts lower, and the cards blurred under the
    // header would otherwise stop at a hard edge above the gap between them and the panel.
    pages::under_top(ui, cx, "unity-projects", from, placed.band_bottom);

    let pressed = header.show(ui, cx, &placed, Some(&mut state.query));
    for (a, r) in header.actions.iter().zip(&placed.actions) {
        state.anchors.insert(a.key.to_string(), *r);
    }
    match pressed {
        Some("refresh") if !state.scanning() => {
            state.refresh(&ctx, true);
            state.load_view(&ctx, true);
            // Asked for: every picture is checked again, kept or not.
            state.thumb_ids.clear();
            state.thumb_fresh.clear();
        }
        Some("repos") => {
            state.open_modal(&ctx, Modal::Repos, state.anchor("repos"));
            state.modal_fill = theme::control();
            state.catalog(&ctx, server(cx));
        }
        Some("add") => add_existing(&ctx, state),
        Some("new") => {
            state.open_modal(&ctx, Modal::New, state.anchor("new"));
            state.modal_fill = theme::ACCENT;
        }
        _ => {}
    }

    if state.selected.is_some() {
        panel::show(ui, cx, state, page, placed.content_top, split, dock, panel_w);
    }
    activity(ui, state, Rect::from_min_max(pos2(page.left(), page.top()), pos2(gallery_right + PAD_X * (1.0 - dock), page.bottom())));
    super::preview::window(ui, cx, state);
    modals(ui, cx, state);
}

/// Asks for a Unity project's folder and adds it to the list VCC and ALCOM share; it opens once the scan lists it.
pub(super) fn add_existing(ctx: &egui::Context, state: &mut State) {
    state.task(ctx, tr("Choose a Unity project…"), move || {
        if let Some(p) = rfd::FileDialog::new().set_title(tr("Add a Unity project")).pick_folder() {
            orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &p, true))?;
            Ok(Outcome::Added(p, tr("Project added: VCC and ALCOM list it too").into()))
        } else {
            Ok(Outcome::Picked(None))
        }
    });
}

/// The banner of the page's last error (and the scan's warnings), dismissable.
pub(super) fn errors(ui: &mut Ui, state: &mut State, salt: &str) {
    // Packages missing from the repositories: their card, with Add their repositories, in the panel.
    if salt == "panel" && state.missing.as_ref().is_some_and(|m| state.selected.as_ref() == Some(&m.path)) {
        if super::missing_card::show(ui, state, salt) {
            return;
        }
    }
    if let Some(e) = state.error.clone() {
        if feedback::banner(ui, Id::unique(("projects-error", salt, &e)), Tone::Danger, &e, true) {
            state.error = None;
        }
        ui.add_space(12.0);
    }
}

/// A section's title row (its count in a pill, a line under it); returns the row.
fn section_title(ui: &mut Ui, title: &str, count: Option<usize>, sub: Option<&str>) -> Rect {
    let h = if sub.is_some() { 52.0 } else { 34.0 };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let title_end = widgets::text(p, pos2(r.left(), r.top() + 12.0), Align2::LEFT_CENTER, &Txt::new(title, 16.0).weight(weight::SEMIBOLD));
    if let Some(n) = count {
        widgets::pill_at(p, ui, pos2(title_end.right() + 8.0, title_end.center().y), None, &n.to_string());
    }
    if let Some(s) = sub {
        widgets::text_truncated(p, pos2(r.left(), r.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(s, 12.5).color(theme::FG_3), r.width());
    }
    r
}

/// The chip at the right of the projects' title that shows the hidden projects too (with how many), or not.
fn hidden_filter(ui: &mut Ui, state: &mut State, row: Rect, n: usize) {
    let label = tr!("Hidden · {n}", n = n);
    let chip = widgets::Chip::new(&label).icon(icons::EYE_SLASH).selected(state.show_hidden).id_salt("projects-hidden-filter");
    let w = chip.width(ui);
    let r = Rect::from_min_size(pos2(row.right() - w, row.top() + 12.0 - widgets::CHIP_H * 0.5), vec2(w, widgets::CHIP_H));
    state.anchors.insert("hidden-filter".into(), r);
    let out = chip.show_at(ui, r);
    let pressed = out.pressed;
    let _ = out.response.on_hover_text(if state.show_hidden { tr("Leave the hidden projects out") } else { tr("Show the projects you hid, dimmed") });
    if pressed {
        state.show_hidden = !state.show_hidden;
    }
}

/// How far `anchor` (a project and its place in the content) moved since it was taken, if it did.
fn anchor_shift(anchor: Option<&(PathBuf, f32)>, places: &[(PathBuf, f32)]) -> Option<f32> {
    let (path, was) = anchor?;
    let now = places.iter().find(|(p, _)| p == path)?.1;
    ((now - was).abs() > 0.5).then_some(now - was)
}

/// The project whose card or row is the first one in view (top at or below `top`, or across it).
fn first_in_view(places: &[(PathBuf, Rect)], top: f32) -> Option<PathBuf> {
    places.iter().filter(|(_, r)| r.bottom() > top).min_by(|a, b| a.1.top().total_cmp(&b.1.top()).then(a.1.left().total_cmp(&b.1.left()))).map(|(p, _)| p.clone())
}

/// The order of a list of projects: newest first, except while a scan runs (PROJECTS-18, Enzo 2026-11-19: "Yes please"):
/// then every row keeps the place it had and what the scan finds goes at the end, so nothing moves under the pointer.
/// The date order comes back when the scan ends.
fn keep_places(list: &mut [Project], places: &mut std::collections::HashMap<std::path::PathBuf, usize>, scanning: bool) {
    if !scanning {
        list.sort_by_key(|p| std::cmp::Reverse(p.modified));
        places.clear();
    }
    for p in list.iter() {
        let next = places.len();
        places.entry(p.path.clone()).or_insert(next);
    }
    list.sort_by_key(|p| places.get(&p.path).copied().unwrap_or(usize::MAX));
}

fn gallery(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    if state.selected.is_none() && state.modal.is_none() {
        errors(ui, state, "gallery");
    }
    for (i, w) in state.scan.warnings.clone().iter().enumerate() {
        feedback::banner(ui, Id::unique(("projects-warning", i, w)), Tone::Warn, w, false);
        ui.add_space(12.0);
    }
    let scanning = state.scanning();
    let [mut registered_places, mut found_places] = std::mem::take(&mut state.places);
    let q = state.query.trim().to_lowercase();
    let find = orbiters_core::search::Query::new(&q);
    let matches = |p: &&Project| find.matches(&p.name) || (!q.is_empty() && p.path.to_string_lossy().to_lowercase().contains(&q));
    // Hidden projects only when asked for (dimmed and marked then).
    let hidden_n = state.scan.projects.iter().filter(|p| state.is_hidden(&p.path)).count();
    let shown = |p: &&Project| state.show_hidden || !state.is_hidden(&p.path);
    let mut registered: Vec<Project> = state.scan.projects.iter().filter(|p| p.registered).filter(matches).filter(shown).cloned().collect();
    keep_places(&mut registered, &mut registered_places, scanning);
    let mut proposals: Vec<Project> = state.scan.projects.iter().filter(|p| !p.registered).filter(matches).filter(shown).cloned().collect();
    keep_places(&mut proposals, &mut found_places, scanning);
    let counted = |list: &[Project]| list.iter().filter(|p| !state.is_hidden(&p.path)).count();
    let (registered_n, proposals_n) = (counted(&registered), counted(&proposals));
    state.places = [registered_places, found_places];

    let title = section_title(ui, tr("Your projects"), (registered_n > 0).then_some(registered_n), None);
    if hidden_n > 0 || state.show_hidden {
        hidden_filter(ui, state, title, hidden_n);
    }
    ui.add_space(6.0);
    if registered.is_empty() {
        if state.scanning() {
            skeletons(ui, 3);
        } else if !q.is_empty() {
            quiet_empty(ui, cx, tr("No project matches"), tr("Try another name, or clear the search."));
        } else if state.scan.projects.iter().any(|p| p.registered) {
            quiet_empty(ui, cx, tr("Your projects are hidden"), tr("Hidden, above the cards, shows them again."));
        } else {
            empty(ui, cx, state);
        }
    } else {
        grid(ui, cx, state, &registered);
    }
    // Unity 6 trials not (yet) projects: being made, stopped, or not where they were (AGENTS.md: a retained copy is visible).
    super::unity6_card::job_rows(ui, state);
    if !proposals.is_empty() {
        ui.add_space(32.0);
        section_title(ui, tr("Found on this computer"), Some(proposals_n), Some(tr("Add one to manage its avatars and packages here, in VCC and in ALCOM.")));
        ui.add_space(10.0);
        found(ui, cx, state, &proposals);
    }
}

fn grid(ui: &mut Ui, cx: &mut Cx, state: &mut State, projects: &[Project]) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let (cols, card_w) = layout::columns(w, CARD_MIN, GAP);
    let card_h = (card_w * 0.5625).round() + CARD_BODY;
    let rows = projects.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + GAP) - GAP), Sense::hover());
    let mut opened = None;
    let mut picked = None;
    // Cards without an avatar picture get a still of their 3D preview, asked for in the gallery's order whatever is on
    // screen, once the pictures kept from before had their moment to show (screenshots: at once).
    let pictures_settled = state.fixture || ctx.input(|i| i.time) > 2.0;
    if let Some(gpu) = cx.gpu {
        state.preview.start(&ctx, gpu);
    }
    let unpictured: Vec<&Project> = projects.iter().filter(|p| pictures_settled && !state.is_hidden(&p.path) && art::pictured(state, p).is_empty()).collect();
    for p in unpictured {
        state.preview.want_still(&ctx, cx.gpu, &p.path);
    }
    for (i, p) in projects.iter().enumerate() {
        // Each card springs to its place (in the grid's own coordinates, so scrolling never animates): when the panel
        // takes room or gives it back, the cards reflow instead of jumping.
        // Only the place springs: a card's size is set at once (AGENTS.md: no spring on layout dimensions).
        let target = [(i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (card_h + GAP)];
        state.row_places.push((p.path.clone(), Rect::from_min_size(area.min + vec2(target[0], target[1]), vec2(card_w, card_h))));
        let [x, y] = anim::spring(&ctx, Id::unique(("project-card-place", &p.path)), target, Spring::SOFT);
        let cw = card_w;
        let r = Rect::from_min_size(area.min + vec2(x, y), vec2(cw, (cw * 0.5625).round() + CARD_BODY));
        state.anchors.insert(format!("card-{i}"), r);
        if !ui.is_rect_visible(r.expand(12.0)) {
            continue;
        }
        let resp = card(ui, cx, state, i, p, r);
        if widgets::activated(&resp) {
            opened = Some(p.path.clone());
        }
        // A right press: what the project offers (`menus`).
        let acts = menus::project_items(state, p, true);
        let items: Vec<popup::Item> = acts.iter().map(|(i, _)| *i).collect();
        if let Some(k) = popup::context_menu(ui, &resp, "project-card", &items) {
            picked = Some((p.clone(), acts[k].1, r));
        }
    }
    if let Some(path) = opened {
        state.open(&ctx, path);
    }
    if let Some((p, act, r)) = picked {
        menus::project_act(&ctx, cx, state, &p, act, r, false);
    }
}

/// One project's card; its response (pressed: opened; a right press: its menu).
fn card(ui: &mut Ui, cx: &Cx, state: &State, i: usize, p: &Project, rect: Rect) -> egui::Response {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("project-card", &p.path));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    // A retained cover must be visible on the first frame, including a new window.
    let enter = if !art::pictured(state, p).is_empty() { 1.0 } else { entrance(&ctx, cx, id, i) };
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let selected = state.selected.as_ref() == Some(&p.path) && !state.panel_closing;
    let scale = (0.97 + 0.03 * enter) * (1.0 - 0.035 * press);
    let r = Rect::from_center_size(rect.center() + vec2(0.0, 18.0 * (1.0 - enter) - 3.0 * hover), rect.size() * scale);
    let mut pt = ui.painter().clone();
    pt.multiply_opacity(enter.clamp(0.0, 1.0));
    // A hidden project, shown on request: dimmed (brighter on hover) and marked.
    let hidden = state.is_hidden(&p.path);
    let dim = anim::toggle(&ctx, id.with("hidden"), hidden, Spring::SOFT).clamp(0.0, 1.0);
    if dim > 0.0 {
        pt.multiply_opacity(1.0 - dim * (0.5 - 0.25 * hover.clamp(0.0, 1.0)));
    }
    let sel = art::selection(&pt, id.with("selected"), r, theme::CARD_RADIUS, selected);
    widgets::frame(&pt, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover.max(sel * 0.6)));
    let cover = Rect::from_min_size(r.min, vec2(r.width(), (r.width() * 0.5625).round()));
    art::cover(&pt, state, p, cover, [theme::CARD_RADIUS, theme::CARD_RADIUS, 0.0, 0.0], 1.0 + 0.05 * ease::out_cubic(hover));
    // The picture darkens towards its bottom edge, for the badges and the step to the card.
    let shade = paint::rounded_rect_points_corners(Rect::from_min_max(pos2(cover.left(), cover.center().y), cover.max), [0.0; 4]);
    paint::fill_gradient(
        &pt,
        &shade,
        &Gradient::vertical(Rect::from_min_max(pos2(cover.left(), cover.center().y), cover.max), &[(0.0, Color32::TRANSPARENT), (1.0, theme::black(0.35))]),
    );
    let (tint, glyph) = art::kind_look(p);
    badge(&pt, ui, pos2(cover.left() + 12.0, cover.top() + 12.0), Some(glyph), p.kind.label(), theme::black(0.6), tint, false);
    if p.missing {
        badge(
            &pt,
            ui,
            pos2(cover.right() - 12.0, cover.top() + 12.0),
            Some(icons::WARNING),
            tr("Folder missing"),
            theme::alpha(theme::DANGER, 0.25),
            theme::DANGER,
            true,
        );
    } else if hidden {
        badge(&pt, ui, pos2(cover.right() - 12.0, cover.top() + 12.0), Some(icons::EYE_SLASH), tr("Hidden"), theme::black(0.6), theme::FG_2, true);
    } else if state.u6.trial_at(&p.path).is_some() {
        badge(&pt, ui, pos2(cover.right() - 12.0, cover.top() + 12.0), Some(icons::COPY), tr("Unity 6 trial"), theme::black(0.6), theme::ACCENT, true);
    } else if !state.u6.trials_of(&p.path).is_empty() {
        badge(&pt, ui, pos2(cover.right() - 12.0, cover.top() + 12.0), Some(icons::COPY), tr("Has a Unity 6 trial"), theme::black(0.6), theme::FG_2, true);
    }
    let x = r.left() + 16.0;
    widgets::text_truncated(&pt, pos2(x, cover.bottom() + 22.0), Align2::LEFT_CENTER, &Txt::new(&p.name, 15.0).weight(weight::SEMIBOLD), r.width() - 32.0);
    widgets::text_truncated(
        &pt,
        pos2(x, cover.bottom() + 43.0),
        Align2::LEFT_CENTER,
        &Txt::new(tr!("Edited {when}", when = crate::pages::ago(ui.ctx(), p.modified, now())), 12.0).color(theme::FG_3),
        r.width() - 32.0,
    );
    let mut px = x;
    let py = cover.bottom() + 70.0;
    let pills = [
        (Some(icons::CUBE), state.u6.standing(&p.path).and_then(super::unity6_card::pill).unwrap_or_else(|| p.unity.clone())),
        if p.avatars.is_empty() {
            (Some(icons::GLOBE), if p.kind == orbiters_projects::discovery::Kind::Worlds { tr("World").into() } else { tr("No avatar yet").into() })
        } else {
            (Some(icons::PERSON), tr_n("{count} avatar", "{count} avatars", p.avatars.len() as u64))
        },
    ];
    for (g, label) in pills {
        if label.is_empty() {
            continue;
        }
        let w = widgets::pill_at(&pt, ui, pos2(px, py), g, &label);
        px += w + 6.0;
    }
    resp
}

/// A card's entrance: up and in, 35 ms apart, the first time the window shows it (a card that appears later, a project
/// just added, comes in as it appears). Coming back to the page, the cards and their pictures are there already: they
/// never leave and come in again.
fn entrance(ctx: &egui::Context, cx: &Cx, id: Id, i: usize) -> f32 {
    let delay = 0.05 + i.min(12) as f64 * 0.035;
    let elapsed = anim::time(ctx) - cx.entered;
    let key = id.with("enter");
    let seen = ctx.data(|d| d.get_temp::<bool>(key.with("seen")).is_some());
    if !seen && elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
        return 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(key.with("seen"), true));
    anim::spring_from(ctx, key, [0.0], [1.0], Spring::SOFT)[0]
}

/// A small label on a picture, its left (or with `right` its right) edge at `at`.
#[allow(clippy::too_many_arguments)]
fn badge(p: &egui::Painter, ui: &Ui, at: egui::Pos2, glyph: Option<&str>, label: &str, bg: Color32, fg: Color32, right: bool) {
    let t = Txt::new(label, 10.5).weight(weight::SEMIBOLD).tracking(0.02).color(theme::FG);
    let tw = widgets::text_size(ui, &t).x;
    let w = tw + 20.0 + if glyph.is_some() { 16.0 } else { 0.0 };
    let rect = Rect::from_min_size(if right { pos2(at.x - w, at.y) } else { at }, vec2(w, 22.0));
    p.rect_filled(rect, theme::radius(11.0), bg);
    let mut x = rect.left() + 10.0;
    if let Some(g) = glyph {
        widgets::icon(p, pos2(x + 6.0, rect.center().y), g, 12.0, fg);
        x += 16.0;
    }
    widgets::text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, &t);
}

/// Grey cards that shimmer while the first projects are found.
fn skeletons(ui: &mut Ui, n: usize) {
    let w = ui.available_width();
    let (cols, card_w) = layout::columns(w, CARD_MIN, GAP);
    let card_h = (card_w * 0.5625).round() + CARD_BODY;
    let rows = n.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + GAP) - GAP), Sense::hover());
    let p = ui.painter();
    for i in 0..n {
        let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (card_h + GAP)), vec2(card_w, card_h));
        widgets::frame(p, r, theme::CARD_RADIUS, theme::surface());
        let cover = Rect::from_min_size(r.min, vec2(card_w, card_h - CARD_BODY));
        feedback::shimmer(p, cover.shrink(6.0), theme::CARD_RADIUS - 6.0);
        feedback::shimmer(p, Rect::from_min_size(pos2(r.left() + 16.0, cover.bottom() + 15.0), vec2(card_w * 0.5, 14.0)), 7.0);
        feedback::shimmer(p, Rect::from_min_size(pos2(r.left() + 16.0, cover.bottom() + 38.0), vec2(card_w * 0.32, 10.0)), 5.0);
        feedback::shimmer(p, Rect::from_min_size(pos2(r.left() + 16.0, cover.bottom() + 60.0), vec2(card_w * 0.6, 20.0)), 10.0);
    }
}

/// No project yet: the logo, what this page is for and its two ways in.
fn empty(ui: &mut Ui, cx: &Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 340.0), Sense::hover());
    let (rect, alpha) = pages::entering(ui, cx, 2, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let t = anim::clock(&ctx, 30.0);
    let bob = -8.0 * ease::in_out(ease::mirror(t, 2.0));
    let lr = Rect::from_center_size(rect.center() + vec2(0.0, -96.0 + bob), logo::size_for_width(70.0));
    logo::paint(&p, lr, logo::Look::default());
    widgets::text(&p, rect.center() + vec2(0.0, -24.0), Align2::CENTER_CENTER, &Txt::new(tr("Your avatar starts here"), 19.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(tr("Create a project from a VRChat template, or add one you already have: its avatars and packages show up here."), 13.0)
            .color(theme::FG_3)
            .line_height(20.0)
            .job(420.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y - 4.0), g, theme::FG_3);
    let create = Button::new(tr("New project")).icon(icons::PLUS).variant(Variant::Primary);
    let add = Button::new(tr("Add existing")).icon(icons::FOLDER_OPEN);
    let (cw, aw) = (create.desired_width(ui), add.desired_width(ui));
    let x0 = rect.center().x - (cw + aw + 10.0) * 0.5;
    let y = rect.center().y + 60.0;
    let mut bu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("projects-empty"));
    bu.multiply_opacity(alpha);
    let cr = Rect::from_min_size(pos2(x0, y), vec2(cw, 40.0));
    if widgets::activated(&create.id_salt("empty-new").show_at(&mut bu, cr)) {
        state.open_modal(&ctx, Modal::New, cr);
        state.modal_fill = theme::ACCENT;
    }
    if widgets::activated(&add.id_salt("empty-add").enabled(!state.running()).show_at(&mut bu, Rect::from_min_size(pos2(cr.right() + 10.0, y), vec2(aw, 40.0))))
    {
        add_existing(&ctx, state);
    }
}

/// A quiet line where a list is empty (a search that matched nothing).
fn quiet_empty(ui: &mut Ui, cx: &Cx, title: &str, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
    let (rect, alpha) = pages::entering(ui, cx, 2, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    widgets::frame(&p, rect, theme::CARD_RADIUS, theme::white(0.02));
    widgets::icon(&p, rect.center() + vec2(0.0, -24.0), icons::MAGNIFYING_GLASS, 22.0, theme::FG_4);
    widgets::text(&p, rect.center() + vec2(0.0, 6.0), Align2::CENTER_CENTER, &Txt::new(title, 14.5).weight(weight::SEMIBOLD));
    widgets::text(&p, rect.center() + vec2(0.0, 28.0), Align2::CENTER_CENTER, &Txt::new(text, 12.5).color(theme::FG_3));
}

/// Projects found on the computer that are not in the list yet: compact rows, the first few until "Show all".
fn found(ui: &mut Ui, cx: &mut Cx, state: &mut State, list: &[Project]) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let (cols, row_w) = layout::columns(w, 300.0, 12.0);
    let shown = if state.all_proposals { list.len() } else { list.len().min(FOUND_FIRST) };
    let rows = shown.div_ceil(cols);
    let row_h = 76.0;
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (row_h + 12.0) - 12.0), Sense::hover());
    let mut open = None;
    let mut add = None;
    let mut picked = None;
    for (i, p) in list.iter().take(shown).enumerate() {
        let target = [(i % cols) as f32 * (row_w + 12.0), (i / cols) as f32 * (row_h + 12.0)];
        state.row_places.push((p.path.clone(), Rect::from_min_size(area.min + vec2(target[0], target[1]), vec2(row_w, row_h))));
        let [x, y] = anim::spring(&ctx, Id::unique(("found-place", &p.path)), target, Spring::SOFT);
        let rw = row_w;
        let r = Rect::from_min_size(area.min + vec2(x, y), vec2(rw, row_h));
        if !ui.is_rect_visible(r.expand(8.0)) {
            continue;
        }
        let (pressed, resp) = found_row(ui, state, p, r);
        match pressed {
            Some(true) => add = Some(p.path.clone()),
            Some(false) => open = Some(p.path.clone()),
            None => {}
        }
        let acts = menus::project_items(state, p, true);
        let items: Vec<popup::Item> = acts.iter().map(|(i, _)| *i).collect();
        if let Some(k) = popup::context_menu(ui, &resp, "found-row", &items) {
            picked = Some((p.clone(), acts[k].1, r));
        }
    }
    if let Some((p, act, r)) = picked {
        menus::project_act(&ctx, cx, state, &p, act, r, false);
    }
    if let Some(path) = add {
        state.task(&ctx, tr("Adding the project…"), move || {
            orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, true))?;
            Ok(Outcome::Added(path, tr("Project added: VCC and ALCOM list it too").into()))
        });
    } else if let Some(path) = open {
        state.open(&ctx, path);
    }
    if list.len() > FOUND_FIRST {
        ui.add_space(12.0);
        let label = if state.all_proposals { tr("Show fewer").to_string() } else { tr!("Show all {count}", count = list.len()) };
        let b = Button::new(&label)
            .icon(if state.all_proposals { icons::CARET_LEFT } else { icons::CARET_DOWN })
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .id_salt("found-all");
        let bw = b.desired_width(ui);
        let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        if widgets::activated(&b.show_at(ui, Rect::from_min_size(pos2(r.center().x - bw * 0.5, r.top()), vec2(bw, 32.0)))) {
            state.all_proposals = !state.all_proposals;
        }
    }
}

/// One project found on the computer: `Some(true)` when Add was pressed, `Some(false)` when the row was; and the row's
/// response (its right-click menu).
fn found_row(ui: &mut Ui, state: &State, p: &Project, rect: Rect) -> (Option<bool>, egui::Response) {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("found-row", &p.path));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let selected = state.selected.as_ref() == Some(&p.path) && !state.panel_closing;
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.02 * press));
    let mut dimmed = ui.painter().clone();
    let hidden = state.is_hidden(&p.path);
    if hidden {
        dimmed.multiply_opacity(0.5 + 0.25 * hover.clamp(0.0, 1.0));
    }
    let pt = &dimmed;
    let sel = art::selection(pt, id.with("selected"), r, 16.0, selected);
    widgets::frame(pt, r, 16.0, theme::mix(theme::alpha(theme::INK_850, 0.6), theme::INK_800, hover.max(sel * 0.6)));
    let thumb = Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 28.0), vec2(56.0, 56.0));
    art::cover(pt, state, p, thumb, [12.0; 4], 1.0);
    let add_b = Button::new(tr("Add")).icon(icons::PLUS).size(Size::Sm).id_salt(("found-add", &p.path)).enabled(!state.running());
    let aw = add_b.desired_width(ui);
    let x = thumb.right() + 12.0;
    let tw = r.right() - 14.0 - aw - 12.0 - x;
    widgets::text_truncated(pt, pos2(x, r.center().y - 17.0), Align2::LEFT_CENTER, &Txt::new(&p.name, 14.0).weight(weight::SEMIBOLD), tw);
    widgets::text_truncated(pt, pos2(x, r.center().y + 1.0), Align2::LEFT_CENTER, &Txt::new(p.path.display().to_string(), 11.5).color(theme::FG_3), tw);
    let meta = if p.avatars.is_empty() {
        tr!("Unity {version} · {what}", version = p.unity, what = p.kind.label())
    } else {
        tr!("Unity {version} · {what}", version = p.unity, what = tr_n("{count} avatar", "{count} avatars", p.avatars.len() as u64))
    };
    let meta = if hidden { tr!("Hidden · {meta}", meta = meta) } else { meta };
    widgets::text_truncated(pt, pos2(x, r.center().y + 18.0), Align2::LEFT_CENTER, &Txt::new(meta, 11.5).color(theme::FG_4), tw);
    let br = Rect::from_min_size(pos2(r.right() - 14.0 - aw, r.center().y - 16.0), vec2(aw, 32.0));
    if widgets::activated(&add_b.show_at(ui, br)) {
        return (Some(true), resp);
    }
    (widgets::activated(&resp).then_some(false), resp)
}

/// The capsule over the bottom of the cards while an action runs (unless a dialog shows it), with a project copy's
/// progress and its Cancel.
fn activity(ui: &mut Ui, state: &mut State, area: Rect) {
    let shown_in_dialog = matches!(state.modal, Some(Modal::New | Modal::Setup | Modal::Repos | Modal::Duplicate(_) | Modal::Conflict));
    let label = state.acting().map(str::to_string).filter(|_| !shown_in_dialog);
    let p = state.copy_progress.lock().unwrap().clone();
    let copying = p.total > 0 && label.is_some();
    let detail = copying.then(|| tr!("{copied} of {total} files · {file}", copied = format::integer(p.copied as f64), total = format::integer(p.total as f64), file = p.file));
    let a = label.as_deref().map(|l| feedback::Activity {
        label: l,
        detail: detail.as_deref(),
        fraction: copying.then(|| p.copied as f32 / p.total.max(1) as f32),
        cancel: copying && p.phase == mark("Copying project"),
    });
    // A Library move elsewhere than the panel on screen: an action of the page has the capsule while it runs, a moment.
    let repair = if a.is_none() && !shown_in_dialog { state.repair_capsule() } else { None };
    let b = repair.as_ref().map(|(label, detail, fraction, cancel)| feedback::Activity {
        label,
        detail: (!detail.is_empty()).then_some(detail.as_str()),
        fraction: *fraction,
        cancel: *cancel,
    });
    if feedback::activity(ui, Id::unique("projects-activity"), area, a.as_ref().or(b.as_ref())) {
        if repair.is_some() {
            state.cancel_repair();
        } else {
            state.copy_cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The Capture page's N4N0 card, among its status cards: whether N4N0 is on the avatar worn in VRChat, or the way to put
/// it there (the avatar picker, which grows out of this card).
pub fn capture_card(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect) {
    let ctx = ui.ctx().clone();
    state.poll(&ctx, cx);
    state.anchors.insert("capture-card".into(), rect);
    let on_avatar = cx.snap.n4n0.manifest && cx.snap.n4n0.prop_sync.is_some();
    let waiting = state.flow.as_ref().is_some_and(|f| f.request.is_some() && f.result.is_none());
    let (value, tone, trailing) = if state.running() {
        (tr("Setting up…"), Tone::Info, None)
    } else if waiting && !on_avatar {
        (tr("Confirm in Unity"), Tone::Warn, Some(art::glyph::ARROW_RIGHT))
    } else if on_avatar {
        (tr("On your avatar"), Tone::Ok, Some(icons::CHECK))
    } else {
        (tr("Add to my avatar"), Tone::Ok, Some(icons::PLUS))
    };
    let id = Id::unique("capture-n4n0-card");
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let resp = resp.on_hover_text(if on_avatar {
        tr("N4N0 is on your avatar: you're ready to capture. Press to put it on another avatar.")
    } else {
        tr("Put N4N0 on the avatar you capture with: Orbiters installs it in its Unity project, you confirm in Unity")
    });
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.03 * press));
    let p = ui.painter();
    widgets::frame(p, r, 16.0, theme::mix(theme::surface(), theme::INK_800, hover));
    let tile = Rect::from_min_size(pos2(r.left() + 14.0, r.center().y - 18.0), vec2(36.0, 36.0));
    // A call to act stands out with the green tile while N4N0 is not on the avatar.
    let c = anim::color(&ctx, id.with("tone"), tone.color(), Spring::SOFT);
    p.rect_filled(tile, theme::radius(12.0), theme::alpha(c, if on_avatar { 0.1 } else { 0.16 }));
    if state.running() {
        feedback::spinner(p, tile.center(), 8.0, 2.0, c);
    } else {
        widgets::icon(p, tile.center(), icons::N4N0_MARK, 18.0, c);
    }
    let x = tile.right() + 12.0;
    let tw = r.right() - x - 12.0 - if trailing.is_some() { 26.0 } else { 0.0 };
    widgets::text(p, pos2(x, r.center().y - 9.0), Align2::LEFT_CENTER, &Txt::caps("N4N0"));
    widgets::sliding_value(
        ui,
        id.with("value"),
        pos2(x, r.center().y + 9.0),
        Align2::LEFT_CENTER,
        value,
        &Txt::new("", 13.0).weight(weight::MEDIUM).color(theme::FG),
        tw,
    );
    if let Some(g) = trailing {
        let tc = Rect::from_center_size(pos2(r.right() - 24.0, r.center().y), vec2(24.0, 24.0));
        let shift = 2.0 * hover;
        p.circle_filled(tc.center() + vec2(shift, 0.0), 12.0, theme::white(0.04 + 0.05 * hover));
        widgets::icon(p, tc.center() + vec2(shift, 0.0), g, 13.0, if on_avatar { theme::ACCENT } else { theme::mix(theme::FG_2, theme::FG, hover) });
    }
    if widgets::activated(&resp) {
        state.ensure(&ctx);
        state.modal_fill = theme::surface();
        if waiting && !on_avatar && !state.running() {
            state.open_modal(&ctx, Modal::Setup, rect);
        } else {
            let worn = cx.snap.n4n0.avatar_id.clone().filter(|id| state.scan.projects.iter().any(|p| p.registered && p.avatars.iter().any(|a| &a.id == id)));
            state.preselected = worn.or_else(|| state.preselected.clone());
            state.open_modal(&ctx, if state.running() { Modal::Setup } else { Modal::Avatars }, rect);
        }
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    fn project(name: &str, modified: u64) -> Project {
        Project { path: std::path::PathBuf::from(format!("C:/P/{name}")), name: name.into(), modified, ..Default::default() }
    }
    fn names(list: &[Project]) -> Vec<String> {
        list.iter().map(|p| p.name.clone()).collect()
    }

    /// Verifier, point 8: rows added above the first project in view (the other section growing, a banner) scroll the
    /// gallery by as much, so that project stays where it was on screen.
    #[test]
    fn growth_above_the_first_row_in_view_is_followed() {
        let p = |n: &str| std::path::PathBuf::from(format!("C:/P/{n}"));
        let rect = |y: f32| Rect::from_min_size(pos2(0.0, y), vec2(100.0, 80.0));
        // In view from 300 on (the viewport's top): "b" is the first row there.
        let frame1 = vec![(p("a"), rect(200.0)), (p("b"), rect(310.0)), (p("c"), rect(420.0))];
        assert_eq!(first_in_view(&frame1, 300.0), Some(p("b")));
        let anchor = (p("b"), 310.0 - 300.0 + 40.0);
        // Next frame a row of 110 came in above it: the gallery scrolls 110 further.
        let frame2 = vec![(p("new"), 50.0 + 40.0), (p("a"), 310.0 - 300.0 + 40.0 - 0.0), (p("b"), 420.0 - 300.0 + 40.0)];
        assert_eq!(anchor_shift(Some(&anchor), &frame2), Some(110.0));
        // Nothing moved: nothing to do.
        assert_eq!(anchor_shift(Some(&anchor), &[(p("b"), 50.0)]), None);
    }

    /// PROJECTS-18: while a scan runs, a newer project found later goes at the end and no row moves; once it ends, the
    /// newest is first again.
    #[test]
    fn rows_stay_in_place_while_a_scan_runs() {
        let mut places = std::collections::HashMap::new();
        let mut list = vec![project("old", 1), project("mid", 5)];
        keep_places(&mut list, &mut places, false);
        assert_eq!(names(&list), ["mid", "old"]);
        // The scan's next update: a newer one, given in date order as the scan sends them.
        let mut list = vec![project("new", 9), project("mid", 5), project("old", 1)];
        keep_places(&mut list, &mut places, true);
        assert_eq!(names(&list), ["mid", "old", "new"]);
        let mut list = vec![project("newer", 10), project("new", 9), project("mid", 5), project("old", 1)];
        keep_places(&mut list, &mut places, true);
        assert_eq!(names(&list), ["mid", "old", "new", "newer"]);
        // The scan ended.
        keep_places(&mut list, &mut places, false);
        assert_eq!(names(&list), ["newer", "new", "mid", "old"]);
    }
}
