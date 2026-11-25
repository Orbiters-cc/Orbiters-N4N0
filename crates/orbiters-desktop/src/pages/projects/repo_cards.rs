//! The Suggested section of the Package repositories dialog: some 330 repositories as a grid of cards, with a search
//! ([`orbiters_projects::suggestions`]) that stays within reach however far the list is scrolled.
//!
//! The dialog has one scroll surface, so nothing here nests a scroll area. The section's heading and search field keep
//! their place in the flow while it is on screen and stay at the edge of the view when it is not: at the top once the
//! cards have scrolled under them, and at the bottom while "Your repositories" is tall enough to push the section out of
//! sight. Going to the field or typing in it brings the section to the top, so the results are what the person sees.
//!
//! This is a real list, and every frame of it counts: all cards are one height, so the grid's height and which of its
//! rows are in view are arithmetic, and only cards in view lay out text, take part in input or ask for their icons (an
//! icon is downloaded and decoded, so `State::repo_icons_wanted` must only ever hold what the person can see). The
//! filter is kept until the query or the catalog changes. Nothing else here depends on how long the list is.
//!
//! The dialog follows its content's height with a spring, which would make it jump about with every key pressed: the
//! section keeps the room the whole list would take in the window, however few cards the query leaves.

use orbiters_i18n::{mark, tr, tr_n};
use super::art::{self, glyph};
use super::state::State;
use egui::{pos2, vec2, Align2, Color32, Id, Key, Modifiers, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::network::{self, Listing};
use orbiters_projects::suggestions::{Hit, Suggestions};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{feedback, layout};
use std::collections::BTreeMap;
use std::ops::Range;

/// Every card's height: the grid stays even and needs no measuring. Its parts add up in [`card`].
const CARD_H: f32 = 150.0;
const GAP: f32 = 10.0;
/// The narrowest a card gets: two columns in the dialog, one on a very narrow window.
const CARD_MIN_W: f32 = 250.0;
const CARD_RADIUS: f32 = 16.0;
const PAD: f32 = 14.0;
const CAPS_H: f32 = 18.0;
const FIELD_H: f32 = 40.0;
/// The heading and the field.
const BAND_H: f32 = CAPS_H + 8.0 + FIELD_H;
/// Filled space kept between the band and what passes it, and the fade beyond that, while the band is pinned.
const BAND_ROOM: f32 = 6.0;
const BAND_FADE: f32 = 12.0;
/// The space between the band and the grid (the dialog's own between items).
const FLOW_GAP: f32 = 12.0;
const SKELETONS: usize = 6;
/// Seconds after a scroll before a card under the pointer says more about itself.
const HINT_AFTER_SCROLL: f32 = 0.8;
const SEARCH_HINT: &str = mark("Search packages, repositories, owners and descriptions");

/// How far apart rows are.
const PITCH: f32 = CARD_H + GAP;

/// The height of `rows` rows of cards.
fn rows_height(rows: usize) -> f32 {
    if rows == 0 {
        0.0
    } else {
        rows as f32 * PITCH - GAP
    }
}

/// Which of a grid's `rows` rows show between `top` and `bottom`, counted from the top of the grid. Rows only touching
/// the view do not count (a card there has nothing to show).
fn rows_in_view(top: f32, bottom: f32, rows: usize) -> Range<usize> {
    if rows == 0 || bottom <= 0.0 || bottom <= top {
        return 0..0;
    }
    let first = (top.max(0.0) / PITCH).floor() as usize;
    // In the gap after a row: that row is out of view.
    let first = if top > first as f32 * PITCH + CARD_H { first + 1 } else { first };
    let end = ((bottom / PITCH).ceil() as usize).min(rows);
    first.min(end)..end
}

/// Where the band sits: in the flow at `natural`, held inside the view at either end.
fn band_top(natural: f32, view: Rect) -> f32 {
    natural.min(view.bottom() - BAND_H).max(view.top())
}

/// The section's room below the band: what the cards shown take, or what the whole list would take in the view when
/// that is more (see the module's notes), and at least what the message in its place takes.
fn section_height(shown: f32, whole: f32, view_h: f32, at_least: f32) -> f32 {
    shown.max(whole.min((view_h - BAND_H - FLOW_GAP).max(0.0))).max(at_least)
}

/// Draws the section at the end of the dialog's body. True when the person asked to try loading the suggestions again.
pub(super) fn section(ui: &mut Ui, state: &mut State) -> bool {
    let loading = state.catalog_job.running();
    if state.suggestions.total() == 0 && !loading {
        art::caps(ui, tr("Suggested"));
        if state.catalog.errors.is_empty() {
            widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 10.0), Align2::LEFT_CENTER, &Txt::new(tr("No suggestion right now."), 12.5).color(theme::fg_3()));
            ui.add_space(20.0);
            return false;
        }
        return unreachable(ui, state);
    }
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let view = ui.clip_rect();
    let origin = ui.min_rect().top();
    let (natural, _) = ui.allocate_exact_size(vec2(w, BAND_H), Sense::hover());

    // Something appearing above (a repository added, a message) must not move the cards being looked at, nor an error
    // stay out of sight above them (an Add pressed far down the list can fail): that brings the person to it.
    let id = state.dialog_id().with("repo-cards");
    let above = natural.top() - origin;
    let was = ctx.data(|d| d.get_temp::<f32>(id.with("above"))).unwrap_or(above);
    ctx.data_mut(|d| d.insert_temp(id.with("above"), above));
    let error = state.error.is_some();
    let new_error = error && !ctx.data(|d| d.get_temp::<bool>(id.with("error"))).unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(id.with("error"), error));
    let mut shift = 0.0;
    let moved = above - was;
    if new_error {
        ui.scroll_with_delta(vec2(0.0, view.top() - origin));
    } else if moved != 0.0 && natural.top() - moved <= view.top() {
        // Scrolled past the section before the change: scrolled by as much, and this frame's cards drawn as they will be.
        ui.scroll_with_delta(vec2(0.0, -moved));
        shift = -moved;
    }

    let y = band_top(natural.top() + shift, view);
    let pinned_top = natural.top() + shift < view.top();
    let pinned_bottom = natural.top() + shift > y;
    let band = Rect::from_min_size(pos2(natural.left(), y), natural.size());
    let across = |top: f32, bottom: f32| Rect::from_min_max(pos2(view.left(), top), pos2(view.right(), bottom));
    if pinned_top || pinned_bottom {
        // What passes under it cannot be pressed through it either.
        let fill = if pinned_top { across(band.top(), band.bottom() + BAND_ROOM) } else { across(band.top() - BAND_ROOM, band.bottom()) };
        ui.painter().rect_filled(fill, 0.0, theme::ink_850());
        let _ = ui.interact(fill, id.with("band"), Sense::click());
    }
    let mut bu = ui.new_child(UiBuilder::new().max_rect(band).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("repo-cards-band"));
    bu.spacing_mut().item_spacing = vec2(0.0, 8.0);
    heading(&mut bu, state, loading);

    // Escape in the field takes the query away first and leaves the person in the field; the dialog closes on the next
    // one. egui has already taken the focus away for that key by now, so it is last frame's focus that tells.
    let field_id = bu.scope_id().with("repo-search");
    let was_in_field = ctx.data(|d| d.get_temp::<bool>(id.with("in-field"))).unwrap_or(false);
    if was_in_field && !state.repo_query.is_empty() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        state.repo_query.clear();
        bu.memory_mut(|m| m.request_focus(field_id));
    }
    let field = widgets::search_field_clearable(&mut bu, "repo-search", &mut state.repo_query, tr(SEARCH_HINT), w);
    let in_field = field.has_focus();
    ctx.data_mut(|d| d.insert_temp(id.with("in-field"), in_field));
    if state.fixture {
        state.anchors.insert("repo-search".into(), field.rect);
    }
    let changed = state.suggestions.set_query(&state.repo_query);
    if changed || (field.gained_focus() && pinned_bottom) {
        // The results where they can be seen, at once.
        let to = natural.top() - view.top();
        if to.abs() > 0.5 {
            ui.scroll_with_delta(vec2(0.0, -to));
        }
    }

    // What fits between the band and the view's far edge.
    let shown_view = Rect::from_min_max(
        pos2(view.left(), if pinned_top { band.bottom() + BAND_ROOM } else { view.top() }),
        pos2(view.right(), if pinned_bottom { band.top() - BAND_ROOM } else { view.bottom() }),
    );
    let (cols, card_w) = layout::columns(w, CARD_MIN_W, GAP);
    let top_of = |area: Rect| area.top() + shift;

    let mut retry = false;
    if state.suggestions.total() > 0 && !state.catalog.errors.is_empty() {
        retry = partly_loaded(ui, state, shown_view, shift);
    }

    if state.suggestions.total() == 0 {
        let (area, _) = ui.allocate_exact_size(vec2(w, rows_height(SKELETONS.div_ceil(cols))), Sense::hover());
        let p = ui.painter().with_clip_rect(shown_view.intersect(view));
        for i in 0..SKELETONS {
            skeleton(&p, cell(area.left(), top_of(area), i, cols, card_w));
        }
    } else {
        let suggestions = std::mem::take(&mut state.suggestions);
        let hits = suggestions.hits();
        let rows = hits.len().div_ceil(cols);
        let whole = rows_height(suggestions.total().div_ceil(cols));
        let at_least = if hits.is_empty() { NOTHING_H } else { 0.0 };
        let (area, _) = ui.allocate_exact_size(vec2(w, section_height(rows_height(rows), whole, view.height(), at_least)), Sense::hover());
        let mut gu = ui.new_child(UiBuilder::new().max_rect(area.translate(vec2(0.0, shift))).id_salt("repo-cards-grid"));
        gu.set_clip_rect(shown_view.intersect(view));
        if hits.is_empty() {
            nothing_found(&mut gu, state, area.translate(vec2(0.0, shift)));
        } else {
            let grid_top = top_of(area);
            let busy = state.running();
            let mut add = None;
            for row in rows_in_view(shown_view.top() - grid_top, shown_view.bottom() - grid_top, rows) {
                for (i, hit) in hits.iter().enumerate().skip(row * cols).take(cols) {
                    if card(&mut gu, state, &suggestions, hit, cell(area.left(), grid_top, i, cols, card_w), i, busy) {
                        add = Some(suggestions.listing(hit).url.clone());
                    }
                }
            }
            if let Some(url) = add {
                state.repo_adding = Some(url.clone());
                super::dialogs::add_repo(state, &ctx, url, BTreeMap::new());
            }
        }
        state.suggestions = suggestions;
    }

    // The soft edge of what passes the band, over it, and of a list going on past the bottom of the view.
    if pinned_top {
        fade(ui, across(band.bottom() + BAND_ROOM, band.bottom() + BAND_ROOM + BAND_FADE), theme::ink_850(), Color32::TRANSPARENT);
    } else if pinned_bottom {
        fade(ui, across(band.top() - BAND_ROOM - BAND_FADE, band.top() - BAND_ROOM), Color32::TRANSPARENT, theme::ink_850());
    }
    if !pinned_bottom && ui.min_rect().bottom() + shift > view.bottom() + 1.0 {
        fade(ui, across(view.bottom() - BAND_FADE, view.bottom()), Color32::TRANSPARENT, theme::ink_850());
    }
    retry
}

fn fade(ui: &Ui, rect: Rect, from: Color32, to: Color32) {
    paint::fill_gradient(ui.painter(), &paint::rounded_rect_points_corners(rect, [0.0; 4]), &Gradient::vertical(rect, &[(0.0, from), (1.0, to)]));
}

/// Card `i` of a grid.
fn cell(left: f32, top: f32, i: usize, cols: usize, card_w: f32) -> Rect {
    Rect::from_min_size(pos2(left + (i % cols) as f32 * (card_w + GAP), top + (i / cols) as f32 * PITCH), vec2(card_w, CARD_H))
}

/// The section's title, and on its right how many cards there are or what is running: the band is the one place of the
/// dialog that is always in view, so what the dialog is doing is said here without moving anything.
fn heading(ui: &mut Ui, state: &State, loading: bool) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), CAPS_H), Sense::hover());
    let p = ui.painter();
    widgets::text(p, r.left_center(), Align2::LEFT_CENTER, &Txt::caps(tr("Suggested")));
    let (total, shown) = (state.suggestions.total(), state.suggestions.hits().len());
    let working = state.acting().map(str::to_string).or_else(|| (loading && total == 0).then(|| tr("Loading…").to_string()));
    if let Some(label) = working {
        let t = Txt::new(label, 11.5).color(theme::fg_2());
        let title_end = widgets::text_truncated(p, r.right_center(), Align2::RIGHT_CENTER, &t, r.width() - 110.0);
        feedback::spinner(p, pos2(title_end.left() - 11.0, r.center().y), 5.0, 1.6, theme::accent());
        return;
    }
    let count = if state.suggestions.searching() {
        tr!("{shown} of {total}", shown = shown, total = total)
    } else {
        tr_n("{count} repository", "{count} repositories", total as u64)
    };
    widgets::text(
        p,
        r.right_center(),
        Align2::RIGHT_CENTER,
        &Txt::new(count, 11.5).color(if state.suggestions.searching() && shown > 0 { theme::accent() } else { theme::fg_3() }),
    );
}

/// One repository: its icon, name and owner, its description (or address), what it was found through, how many packages
/// it has and Add. True when Add was pressed.
///
/// `PAD` + icon 40 + 10 + two lines of 17 + 8 + button 32 + 12 is [`CARD_H`].
fn card(ui: &mut Ui, state: &mut State, suggestions: &Suggestions, hit: &Hit, r: Rect, i: usize, busy: bool) -> bool {
    let l = suggestions.listing(hit);
    let id = Id::unique(("repo-card", &l.url));
    let body = ui.interact(r, id, Sense::hover());
    let hover = anim::toggle(ui.ctx(), id.with("hover"), body.contains_pointer(), Spring::SOFT);
    let p = ui.painter().clone();
    widgets::frame(&p, r, CARD_RADIUS, theme::mix(theme::surface_soft(), theme::alpha(theme::ink_750(), 0.8), hover));

    let icon = Rect::from_min_size(r.min + vec2(PAD, PAD), vec2(40.0, 40.0));
    state.repo_icons_wanted.push(l.url.clone());
    art::repo_icon(&p, state.repo_icons.get(&l.url), icon, &l.url, true);
    let x = icon.right() + 12.0;
    let text_w = r.right() - PAD - x;
    let name = Txt::new(&l.name, 14.0).weight(weight::SEMIBOLD);
    if l.owner.is_empty() {
        widgets::text_truncated(&p, pos2(x, icon.center().y), Align2::LEFT_CENTER, &name, text_w);
    } else {
        widgets::text_truncated(&p, pos2(x, icon.center().y - 9.0), Align2::LEFT_CENTER, &name, text_w);
        widgets::text_truncated(&p, pos2(x, icon.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(&l.owner, 11.5).color(theme::fg_3()), text_w);
    }

    // Two lines under them; a package it was found through takes the second.
    let left = r.left() + PAD;
    let wide = r.width() - 2.0 * PAD;
    let lines_top = icon.bottom() + 10.0;
    let description_rows = if hit.because.is_some() { 1 } else { 2 };
    if l.description.is_empty() {
        // Nothing to say about it (a server that does not say yet): where its feed is, in a quieter voice.
        let address = l.url.trim_start_matches("https://").trim_start_matches("http://");
        widgets::text_truncated(&p, pos2(left, lines_top + 8.5), Align2::LEFT_CENTER, &Txt::new(address, 11.5).color(theme::fg_4()), wide);
    } else {
        let mut job = Txt::new(&l.description, 12.0).color(theme::fg_2()).line_height(17.0).job(wide);
        job.wrap.max_rows = description_rows;
        job.wrap.break_anywhere = description_rows == 1;
        p.galley(pos2(left, lines_top), p.layout_job(job), theme::fg_2());
    }
    if let Some(package) = &hit.because {
        let y = lines_top + 17.0 + 8.5;
        widgets::icon(&p, pos2(left + 6.5, y), glyph::PACKAGE, 13.0, theme::accent());
        widgets::text_truncated(
            &p,
            pos2(left + 18.0, y),
            Align2::LEFT_CENTER,
            &Txt::new(package, 12.0).weight(weight::MEDIUM).color(theme::accent()),
            wide - 18.0,
        );
    }

    // The foot: how many packages, and Add.
    let foot_y = r.bottom() - 12.0 - 16.0;
    let added = state.view.repos.iter().any(|repo| repo.url.trim_end_matches('/') == l.url.trim_end_matches('/'));
    let adding = busy && state.repo_adding.as_deref() == Some(l.url.as_str());
    let mut pressed = false;
    let mut button_left = r.right() - PAD;
    if adding {
        let t = widgets::text(&p, pos2(button_left, foot_y), Align2::RIGHT_CENTER, &Txt::new(tr("Adding…"), 12.0).color(theme::fg_2()));
        feedback::spinner(&p, pos2(t.left() - 13.0, foot_y), 6.5, 2.0, theme::accent());
        button_left = t.left() - 24.0;
    } else {
        let b = Button::new(if added { tr("Added") } else { tr("Add") })
            .icon(if added { orbiters_ui::icons::CHECK } else { orbiters_ui::icons::PLUS })
            .size(Size::Sm)
            .enabled(!added && !busy)
            .id_salt(("suggested", &l.url));
        let bw = b.desired_width(ui);
        let br = Rect::from_min_size(pos2(button_left - bw, foot_y - 16.0), vec2(bw, 32.0));
        if state.fixture {
            state.anchors.insert(format!("repo-card-add-{i}"), br);
        }
        pressed = widgets::activated(&b.show_at(ui, br));
        button_left = br.left();
    }
    if state.fixture {
        state.anchors.insert(format!("repo-card-{i}"), r);
    }
    if let Some(n) = l.package_total() {
        let label = match n {
            0 => tr("No packages").to_string(),
            n => tr_n("{count} package", "{count} packages", n as u64),
        };
        widgets::icon(&p, pos2(left + 6.5, foot_y), glyph::PACKAGE, 13.0, theme::fg_4());
        widgets::text_truncated(&p, pos2(left + 18.0, foot_y), Align2::LEFT_CENTER, &Txt::new(label, 11.5).color(theme::fg_3()), button_left - 8.0 - left - 18.0);
    }
    // Only now, as only one card in a frame is hovered: the whole of what is cut short, and where it comes from. Not for
    // the card that happens to stop under the pointer when the list stops scrolling.
    if body.hovered() && ui.input(|i| i.time_since_last_scroll()) > HINT_AFTER_SCROLL {
        body.on_hover_text(card_hint(l));
    }
    pressed
}

/// A card's tooltip.
fn card_hint(l: &Listing) -> String {
    let mut hint = l.name.clone();
    if !l.owner.is_empty() {
        hint = tr!("{name} by {owner}", name = l.name, owner = l.owner);
    }
    if !l.description.is_empty() {
        hint.push_str(&format!("\n{}", l.description));
    }
    hint.push_str(&format!("\n{}", l.url));
    if !l.packages.is_empty() {
        const NAMED: usize = 6;
        let names: Vec<&str> =
            l.packages.iter().take(NAMED).map(|p| if p.display_name.is_empty() { p.name.as_str() } else { p.display_name.as_str() }).collect();
        let more = l.package_total().unwrap_or(0).saturating_sub(NAMED);
        let listed = if more > 0 { tr!("{names} and {more} more", names = names.join(", "), more = more) } else { names.join(", ") };
        hint.push_str(&format!("\n\n{listed}"));
    }
    hint
}

/// A card that is still loading, in the same place and size as the one coming.
fn skeleton(p: &egui::Painter, r: Rect) {
    widgets::frame(p, r, CARD_RADIUS, theme::surface_soft());
    let piece = |x: f32, y: f32, w: f32, h: f32, radius: f32| feedback::shimmer(p, Rect::from_min_size(r.min + vec2(x, y), vec2(w, h)), radius);
    piece(PAD, PAD, 40.0, 40.0, 12.0);
    piece(PAD + 52.0, PAD + 6.0, (r.width() * 0.42).min(140.0), 12.0, 6.0);
    piece(PAD + 52.0, PAD + 26.0, (r.width() * 0.26).min(90.0), 9.0, 4.5);
    piece(PAD, PAD + 52.0, r.width() - 2.0 * PAD, 10.0, 5.0);
    piece(PAD, PAD + 70.0, (r.width() - 2.0 * PAD) * 0.7, 10.0, 5.0);
    piece(r.width() - PAD - 72.0, r.height() - 12.0 - 32.0, 72.0, 32.0, 16.0);
}

const NOTHING_H: f32 = 190.0;

/// The search found nothing: what was looked for and through what, and the way back to every card.
fn nothing_found(ui: &mut Ui, state: &mut State, area: Rect) {
    let c = area.center_top().x;
    let top = area.top() + 18.0;
    let p = ui.painter().clone();
    let tile = Rect::from_center_size(pos2(c, top + 24.0), vec2(48.0, 48.0));
    art::tile(&p, tile, orbiters_ui::icons::MAGNIFYING_GLASS, theme::fg_3());
    let query = state.repo_query.trim();
    widgets::text_truncated(
        &p,
        pos2(c, top + 68.0),
        Align2::CENTER_CENTER,
        &Txt::new(tr!("Nothing matches “{query}”", query = query), 14.5).weight(weight::SEMIBOLD),
        area.width() - 48.0,
    );
    widgets::text_truncated(
        &p,
        pos2(c, top + 90.0),
        Align2::CENTER_CENTER,
        &Txt::new(tr("Try fewer words, or the name of a package, a repository or its owner."), 12.5).color(theme::fg_3()),
        area.width() - 48.0,
    );
    let b = Button::new(tr("Clear the search")).icon(orbiters_ui::icons::X).size(Size::Sm).id_salt("repo-search-clear");
    let bw = b.desired_width(ui);
    let br = Rect::from_center_size(pos2(c, top + 128.0), vec2(bw, 32.0));
    if state.fixture {
        state.anchors.insert("repo-search-clear".into(), br);
    }
    if widgets::activated(&b.show_at(ui, br)) {
        state.repo_query.clear();
    }
}

/// None of the suggestions could be loaded: what that means for the dialog, and Try again. True when pressed.
fn unreachable(ui: &mut Ui, state: &mut State) -> bool {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, NOTHING_H - 8.0), Sense::hover());
    let p = ui.painter().clone();
    widgets::frame(&p, r, CARD_RADIUS, theme::surface_soft());
    let c = r.center().x;
    let top = r.top() + 20.0;
    art::tile(&p, Rect::from_center_size(pos2(c, top + 22.0), vec2(44.0, 44.0)), orbiters_ui::icons::WARNING, theme::WARN);
    let title =
        widgets::text(&p, pos2(c, top + 62.0), Align2::CENTER_CENTER, &Txt::new(tr("Orbiters' suggestions could not be loaded"), 14.5).weight(weight::SEMIBOLD));
    widgets::text_truncated(
        &p,
        pos2(c, top + 84.0),
        Align2::CENTER_CENTER,
        &Txt::new(tr("Your repositories still work, and you can add one by its address above."), 12.5).color(theme::fg_3()),
        w - 48.0,
    );
    let _ = ui.interact(title, Id::unique("repo-cards-unreachable"), Sense::hover()).on_hover_text(state.catalog.errors.join("\n"));
    let b = Button::new(tr("Try again")).icon(orbiters_ui::icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).id_salt("catalog-retry");
    let bw = b.desired_width(ui);
    widgets::activated(&b.show_at(ui, Rect::from_center_size(pos2(c, top + 122.0), vec2(bw, 32.0))))
}

/// One of the server's two lists did not load: a note above the cards it did give. True when Try again was pressed.
fn partly_loaded(ui: &mut Ui, state: &mut State, view: Rect, shift: f32) -> bool {
    let w = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(w, 52.0), Sense::hover());
    let r = area.translate(vec2(0.0, shift));
    let mut nu = ui.new_child(UiBuilder::new().max_rect(r).id_salt("repo-cards-partly"));
    nu.set_clip_rect(view.intersect(ui.clip_rect()));
    let p = nu.painter().clone();
    widgets::frame(&p, r, 14.0, theme::alpha(Tone::Warn.color(), 0.1));
    widgets::icon(&p, pos2(r.left() + 24.0, r.center().y), orbiters_ui::icons::WARNING, 16.0, Tone::Warn.color());
    let b = Button::new(tr("Try again"))
        .icon(orbiters_ui::icons::ARROW_COUNTER_CLOCKWISE)
        .variant(Variant::Ghost)
        .size(Size::Sm)
        .enabled(!state.catalog_job.running())
        .id_salt("catalog-retry");
    let bw = b.desired_width(&nu);
    widgets::text_truncated(
        &p,
        pos2(r.left() + 44.0, r.center().y),
        Align2::LEFT_CENTER,
        &Txt::new(tr("Some suggestions could not be loaded."), 13.0),
        r.width() - 44.0 - bw - 20.0,
    );
    widgets::activated(&b.show_at(&mut nu, Rect::from_min_size(pos2(r.right() - 10.0 - bw, r.center().y - 16.0), vec2(bw, 32.0))))
}

impl State {
    /// A screenshot scenario of the section ([`crate::screenshot`]): the catalog as the server may answer, what is typed.
    pub(super) fn stage_repo_cards(&mut self, ctx: &egui::Context, name: &str) {
        self.modal = Some(super::state::Modal::Repos);
        let catalog = match name {
            "projects-repos-unreachable" => {
                network::Catalog { errors: vec!["vpm/known: connection refused".into(), "vpm/public: connection refused".into()], ..Default::default() } // i18n: not shown (sample catalog data of a screenshot scene)
            }
            "projects-repos-partly" => network::Catalog { errors: vec!["vpm/public: connection refused".into()], ..fixture_catalog() }, // i18n: not shown (sample catalog data of a screenshot scene)
            // The server's answer until owners, descriptions and packages land.
            "projects-repos-bare" => {
                let mut c = fixture_catalog();
                for l in &mut c.known {
                    (l.description, l.owner, l.packages, l.package_count) = (String::new(), String::new(), vec![], None);
                }
                c
            }
            _ => fixture_catalog(),
        };
        // Pictures for most of them, as they arrive over the network: some with a mark, some still on their way.
        for (i, l) in catalog.known.iter().enumerate().take(120) {
            let hue = (i as f32 * 47.0) % 360.0;
            let colour = |shift: f32| 120.0 + 110.0 * ((hue + shift).to_radians().cos() * 0.5 + 0.5);
            let tint = [colour(0.0), colour(120.0), colour(240.0)];
            let look = match i % 6 {
                4 => None,
                3 | 5 => Some(super::state::Look::Mark(tint)),
                _ => Some(super::state::Look::Picture(tint)),
            };
            if let Some(look) = look {
                self.repo_icons.insert(l.url.clone(), super::state::stand_in_icon(ctx, &format!("repo-{}", l.url), look));
            }
        }
        self.set_catalog(catalog);
        match name {
            "projects-repos-many" => {
                let template = self.view.repos[0].clone();
                for i in 0..8 {
                    let mut r = template.clone();
                    r.builtin = false;
                    r.id = format!("extra-{i}");
                    r.name = format!("Extra repository {i}"); // i18n: not shown (sample catalog data of a screenshot scene)
                    r.url = format!("https://extra{i}.example.com/index.json");
                    self.view.repos.push(r);
                }
            }
            "projects-repos-added" => {
                let added = self.suggestions.listing(&self.suggestions.hits()[1]).url.clone();
                let mut r = self.view.repos[0].clone();
                r.name = self.suggestions.listing(&self.suggestions.hits()[1]).name.clone();
                (r.url, r.builtin, r.id) = (added.clone(), false, "added".into());
                self.view.repos.push(r);
                self.repo_adding = Some(self.suggestions.listing(&self.suggestions.hits()[2]).url.clone());
                self.fixture_action = Some(tr("Adding the repository…").into());
            }
            _ => {}
        }
    }

    /// What a scenario of the section types, once the dialog is open.
    pub(super) fn stage_repo_query(&mut self, name: &str, frame: u32) {
        if frame != 70 {
            return;
        }
        match name {
            "projects-repos-search" => self.repo_query = "modular".into(),
            "projects-repos-search-japanese" => self.repo_query = "アバター".into(),
            "projects-repos-search-owner" => self.repo_query = "anatawa".into(),
            "projects-repos-nothing" => self.repo_query = "zzz-no-such-package".into(),
            _ => {}
        }
    }
}

/// What the screenshots and tests list: repositories with the names, owners and packages people really meet, in Latin
/// and Japanese, then as many others as the server has.
pub(super) fn fixture_catalog() -> network::Catalog {
    let package = |(name, display): &(&str, &str)| network::ListingPackage { name: (*name).into(), display_name: (*display).into() };
    // Name, owner, address, description, how many packages, some of them.
    type Fixture<'a> = (&'a str, &'a str, &'a str, &'a str, usize, &'a [(&'a str, &'a str)]);
    let real: &[Fixture] = &[
        (
            "32ba VPM repository", // i18n: not shown (sample catalog data of a screenshot scene)
            "32ba",
            "https://vpm.32ba.net/index.json",
            "Small editor tools for lighting, baking and scene cleanup.", // i18n: not shown (sample catalog data of a screenshot scene)
            6,
            &[("net.32ba.lightprobe", "LightProbe Helper")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        (
            "AAO: Avatar Optimizer", // i18n: not shown (sample catalog data of a screenshot scene)
            "anatawa12",
            "https://vpm.anatawa12.com/vpm.json",
            "Non-destructive optimization: merges meshes, removes unused blendshapes and bones.", // i18n: not shown (sample catalog data of a screenshot scene)
            3,
            &[("com.anatawa12.avatar-optimizer", "AAO Avatar Optimizer")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        ("AITSYS VCC", "AITSYS", "https://vcc.aitsys.dev/index.json", "", 12, &[("dev.aitsys.lens", "Lens Setup"), ("dev.aitsys.udon", "Udon Extras")]), // i18n: not shown (sample catalog data of a screenshot scene)
        (
            "AjisaiFlow",
            "lighfu",
            "https://lighfu.github.io/vpm/index.json",
            "Dressing and weight transfer helpers for Japanese avatar outfits.", // i18n: not shown (sample catalog data of a screenshot scene)
            4,
            &[("com.lighfu.ajisai", "AjisaiFlow")],
        ),
        (
            "AKATSUKIYA-NoctLab",
            "AKATSUKIYA",
            "https://vpm.noctlab.com/vpm.json",
            "夜の撮影ワールド向けのシェーダーとライティングツール集です。",
            5,
            &[("com.noctlab.shader", "NoctLab Shader")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        ("Akiiray VPM Repository", "Akiiray", "https://akiiray.github.io/vpm-repository/index.json", "", 1, &[("com.akiiray.gesture", "Gesture Bits")]), // i18n: not shown (sample catalog data of a screenshot scene)
        (
            "bd_",
            "bdunderscore",
            "https://vpm.nadena.dev/vpm.json",
            "Non-destructive avatar assembly tools.", // i18n: not shown (sample catalog data of a screenshot scene)
            4,
            &[("nadena.dev.modular-avatar", "Modular Avatar"), ("nadena.dev.ndmf", "Non-Destructive Modular Framework")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        (
            "VRCFury",
            "VRCFury",
            "https://vcc.vrcfury.com",
            "Non-destructive tools that merge props, toggles and gestures at build time.", // i18n: not shown (sample catalog data of a screenshot scene)
            1,
            &[("com.vrcfury.vrcfury", "VRCFury")],
        ),
        (
            "Poiyomi", // i18n: not shown (sample catalog data of a screenshot scene)
            "Poiyomi", // i18n: not shown (sample catalog data of a screenshot scene)
            "https://poiyomi.github.io/vpm/index.json",
            "Poiyomi Toon, the shader most avatars are made with, free and Pro.", // i18n: not shown (sample catalog data of a screenshot scene)
            2,
            &[("com.poiyomi.toon", "Poiyomi Toon"), ("com.poiyomi.pro", "Poiyomi Pro")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        (
            "lilToon",
            "lilxyzw",
            "https://lilxyzw.github.io/vpm-repos/vpm.json",
            "アバター向けの多機能トゥーンシェーダーと、その関連ツールを配布しています。",
            4,
            &[("jp.lilxyzw.liltoon", "lilToon"), ("jp.lilxyzw.lilycalinventory", "lilycalInventory")],
        ),
        (
            "すずりょぐのアバターツール",
            "suzuryg",
            "https://suzuryg.github.io/vpm-repos/vpm.json",
            "表情メニューをまとめて作れる FaceEmo など、アバター改変をもっと楽にする道具を公開しています。長い説明文は二行で切られます。", // i18n: not shown (sample catalog data of a screenshot scene)
            3,
            &[("jp.suzuryg.face-emo", "FaceEmo")],
        ),
        (
            "とても長い名前のアバター衣装改変ツールセット配布リポジトリ",
            "株式会社とても長い会社名のクリエイターチーム",
            "https://long.example.jp/vpm.json",
            "衣装の着せ替えからボーンの調整まで。",
            28,
            &[("jp.example.kisekae", "着せ替えアシスタント")],
        ),
        (
            "ReinaS' Packages", // i18n: not shown (sample catalog data of a screenshot scene)
            "ReinaS",
            "https://vpm.rs64.net/vpm.json",
            "TexTransTool: edit an avatar's textures without touching their files.", // i18n: not shown (sample catalog data of a screenshot scene)
            2,
            &[("net.rs64.tex-trans-tool", "TexTransTool")],
        ),
        (
            "Orbiters",
            "Orbiters",
            "https://orbiters.cc/vpm/orbiters/index.json",
            "My Avatar, Toolkit, Mixer and more for Unity and VRChat.", // i18n: not shown (sample catalog data of a screenshot scene)
            7,
            &[("orbiters.toolkit", "Toolkit"), ("orbiters.myavatar", "My Avatar"), ("orbiters.mixer", "Mixer")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        (
            "Gesture Manager", // i18n: not shown (sample catalog data of a screenshot scene)
            "BlackStartx",
            "https://blackstartx.github.io/vcc/index.json",
            "Test Avatar 3.0 gestures and menus in the editor's play mode.", // i18n: not shown (sample catalog data of a screenshot scene)
            1,
            &[("vrchat.blackstartx.gesture-manager", "Gesture Manager")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
        (
            "Pumkin's Avatar Tools", // i18n: not shown (sample catalog data of a screenshot scene)
            "rurre",
            "https://rurre.github.io/vpm/index.json",
            "Copy components between avatars, reset poses and fix what breaks.", // i18n: not shown (sample catalog data of a screenshot scene)
            2,
            &[("io.github.rurre.pumkintools", "Pumkin's Avatar Tools")], // i18n: not shown (sample catalog data of a screenshot scene)
        ),
    ];
    let mut known: Vec<Listing> = real
        .iter()
        .map(|(name, owner, url, description, count, packages)| Listing {
            name: (*name).into(),
            url: (*url).into(),
            description: (*description).into(),
            owner: (*owner).into(),
            packages: packages.iter().map(package).collect(),
            package_count: Some(*count),
            ..Default::default()
        })
        .collect();
    known.extend((known.len()..330).map(|i| {
        let id = format!("com.creator{i}.tools");
        Listing {
            name: format!("Creator {i} VPM"), // i18n: not shown (sample catalog data of a screenshot scene)
            url: format!("https://creator{i}.github.io/vpm/index.json"),
            description: if i % 7 == 0 { String::new() } else { format!("Avatar and world helpers, number {i} of the many on the server.") }, // i18n: not shown (sample catalog data of a screenshot scene)
            owner: if i % 5 == 0 { String::new() } else { format!("creator{i}") },
            packages: vec![network::ListingPackage { name: id, display_name: format!("Creator Tools {i}") }], // i18n: not shown (sample catalog data of a screenshot scene)
            package_count: Some(1 + i % 12),
            ..Default::default()
        }
    }));
    network::Catalog { known, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, RawInput};
    use std::time::{Duration, Instant};

    #[test]
    fn only_rows_in_view_are_drawn() {
        // Rows every 160 points, cards 150 high.
        assert_eq!(rows_in_view(0.0, 600.0, 100), 0..4);
        assert_eq!(rows_in_view(-300.0, 200.0, 100), 0..2, "the grid starts inside the view");
        assert_eq!(rows_in_view(-900.0, -100.0, 100), 0..0, "still below the view");
        assert_eq!(rows_in_view(155.0, 300.0, 100), 1..2, "the view starts in the gap after a row");
        assert_eq!(rows_in_view(1600.0, 2000.0, 100), 10..13);
        assert_eq!(rows_in_view(30_000.0, 40_000.0, 100), 100..100, "past the end");
        assert_eq!(rows_in_view(0.0, 600.0, 2), 0..2);
        assert_eq!(rows_in_view(0.0, 600.0, 0), 0..0);
    }

    #[test]
    fn the_band_stays_inside_the_view_at_either_end() {
        let view = Rect::from_min_size(pos2(0.0, 100.0), vec2(600.0, 500.0));
        assert_eq!(band_top(300.0, view), 300.0, "in the flow");
        assert_eq!(band_top(-4000.0, view), 100.0, "scrolled past: at the top");
        assert_eq!(band_top(4000.0, view), 600.0 - BAND_H, "pushed down by what is above: at the bottom");
    }

    #[test]
    fn the_section_keeps_its_room_however_few_cards_the_search_leaves() {
        // 330 cards in a view of 700: the view's height whatever is shown.
        let whole = rows_height(165);
        assert_eq!(section_height(rows_height(1), whole, 700.0, 0.0), 700.0 - BAND_H - FLOW_GAP);
        assert_eq!(section_height(0.0, whole, 700.0, NOTHING_H), 700.0 - BAND_H - FLOW_GAP);
        // All of them: the whole list.
        assert_eq!(section_height(whole, whole, 700.0, 0.0), whole);
        // Three cards that fit: no more than they take with no search.
        assert_eq!(section_height(rows_height(1), rows_height(2), 700.0, 0.0), rows_height(2));
        assert_eq!(section_height(0.0, rows_height(1), 700.0, NOTHING_H), NOTHING_H);
    }

    #[test]
    fn a_cards_parts_add_up_to_its_height() {
        assert_eq!(PAD + 40.0 + 10.0 + 2.0 * 17.0 + 8.0 + 32.0 + 12.0, CARD_H);
    }

    /// A stand-in dialog's body: `above` points of something else, then the section, in one scroll area.
    struct Harness {
        ctx: egui::Context,
        state: State,
        frame: u32,
        above: f32,
        view: Rect,
        /// Whether an Escape was left for the dialog on the last frame.
        escape_left: bool,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let mut state = State::default();
            state.fixture = true;
            state.modal = Some(super::super::state::Modal::Repos);
            state.set_catalog(fixture_catalog());
            Self { ctx, state, frame: 0, above: 200.0, view: Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 820.0)), escape_left: false }
        }

        fn run(&mut self, events: Vec<Event>) {
            self.frame += 1;
            let input = RawInput { screen_rect: Some(self.view), time: Some(self.frame as f64 / 60.0), events, ..Default::default() };
            let (state, above, escape_left) = (&mut self.state, self.above, &mut self.escape_left);
            let mut output = self.ctx.run_ui(input, |ui| {
                let body = Rect::from_min_size(pos2(330.0, 90.0), vec2(572.0, 640.0));
                let mut bu = ui.new_child(UiBuilder::new().max_rect(body).id_salt("body"));
                bu.set_clip_rect(body);
                // As the dialog's body has it.
                egui::ScrollArea::vertical().id_salt("scroll").auto_shrink([false, false]).animated(false).show(&mut bu, |ui| {
                    let mut c = ui.new_child(UiBuilder::new().max_rect(ui.available_rect_before_wrap()).layout(egui::Layout::top_down(egui::Align::Min)));
                    c.spacing_mut().item_spacing = vec2(0.0, 12.0);
                    c.add_space(above);
                    section(&mut c, state);
                    let h = c.min_rect().height();
                    ui.allocate_exact_size(vec2(572.0, h), Sense::hover());
                });
                *escape_left = ui.input(|i| i.key_pressed(Key::Escape));
            });
            output.textures_delta.clear();
            // What the frame costs on the processor up to the graphics card's part: the meshes too.
            let _ = self.ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
        }

        fn wheel(&mut self, down: f32) {
            self.run(vec![
                Event::PointerMoved(pos2(600.0, 400.0)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, -down),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                    source: egui::MouseWheelSource::Wheel,
                },
            ]);
            for _ in 0..40 {
                self.run(vec![]);
            }
        }

        fn take_wanted(&mut self) -> Vec<String> {
            std::mem::take(&mut self.state.repo_icons_wanted)
        }

        fn search_field(&self) -> Rect {
            self.state.anchors["repo-search"]
        }

        /// The cards drawn last frame, by their place in the list.
        fn drawn(&self) -> Vec<(usize, Rect)> {
            let mut cards: Vec<(usize, Rect)> =
                self.state.anchors.iter().filter_map(|(k, r)| k.strip_prefix("repo-card-").and_then(|i| i.parse().ok()).map(|i| (i, *r))).collect();
            cards.sort_by_key(|(i, _)| *i);
            cards
        }

        /// Forgets what earlier frames drew.
        fn forget_drawn(&mut self) {
            self.state.anchors.retain(|k, _| !k.starts_with("repo-card-"));
        }
    }

    const BODY: Rect = Rect { min: pos2(330.0, 90.0), max: pos2(902.0, 730.0) };

    #[test]
    fn only_cards_on_screen_ask_for_their_icons_at_any_scroll() {
        let mut h = Harness::new();
        for scrolled in [0.0, 400.0, 8000.0, 12000.0] {
            h.wheel(scrolled);
            h.take_wanted();
            h.run(vec![]);
            let wanted = h.take_wanted();
            // Two columns, at most five rows of 160 points in a view of 640 (one partly hidden each way).
            assert!(!wanted.is_empty() && wanted.len() <= 10, "{} icons wanted after scrolling {scrolled}", wanted.len());
        }
    }

    #[test]
    fn the_search_field_stays_in_view_at_any_scroll_and_above_a_tall_section() {
        let mut h = Harness::new();
        h.run(vec![]);
        assert!(BODY.contains_rect(h.search_field()), "in the flow");
        h.wheel(6000.0);
        let field = h.search_field();
        assert!(BODY.contains_rect(field), "scrolled far down: {field:?}");
        assert!(field.top() < BODY.top() + 40.0, "at the top: {field:?}");
        // Cards pass under it, drawn only below it.
        h.forget_drawn();
        h.run(vec![]);
        let first = h.drawn()[0].1;
        assert!(first.bottom() > field.bottom(), "{first:?}");

        // "Your repositories" taller than the view: the field waits at the bottom.
        let mut h = Harness::new();
        h.above = 1500.0;
        h.run(vec![]);
        let field = h.search_field();
        assert!(BODY.contains_rect(field), "pushed down: {field:?}");
        assert!(field.bottom() > BODY.bottom() - 20.0, "at the bottom: {field:?}");
        assert!(h.drawn().is_empty(), "no card is in view yet");
        // Going to it brings the section up, to show what is typed for.
        h.run(vec![
            Event::PointerMoved(field.center()),
            Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        ]);
        h.run(vec![Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        for _ in 0..5 {
            h.run(vec![]);
        }
        let field = h.search_field();
        assert!(field.top() < BODY.top() + 40.0, "brought up: {field:?}");
        assert!(!h.drawn().is_empty(), "the cards show under it");
    }

    #[test]
    fn typing_filters_on_the_same_frame_and_brings_the_results_to_the_top() {
        let mut h = Harness::new();
        h.wheel(6000.0);
        let field = h.search_field();
        h.run(vec![
            Event::PointerMoved(field.center()),
            Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        ]);
        h.run(vec![Event::PointerButton { pos: field.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        h.run(vec![Event::Text("modular".into())]);
        assert_eq!(h.state.repo_query, "modular");
        assert_eq!(h.state.suggestions.hits().len(), 1, "on the frame the key came");
        assert_eq!(h.state.suggestions.listing(&h.state.suggestions.hits()[0]).name, "bd_");
        assert_eq!(h.state.suggestions.hits()[0].because.as_deref(), Some("Modular Avatar"));
        h.forget_drawn();
        for _ in 0..5 {
            h.run(vec![]);
        }
        let cards = h.drawn();
        assert_eq!(cards.len(), 1);
        assert!(BODY.contains_rect(cards[0].1), "the result shows: {:?}", cards[0].1);
        // Escape takes the query away and stays in the field, the dialog stays open; the next one is the dialog's.
        let escape = || Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        h.run(vec![escape()]);
        assert_eq!(h.state.repo_query, "");
        assert_eq!(h.state.suggestions.hits().len(), 330);
        assert!(!h.escape_left, "the dialog would close with its query");
        h.run(vec![]);
        h.run(vec![Event::Text("a".into())]);
        assert_eq!(h.state.repo_query, "a", "still in the field");
        h.run(vec![escape()]);
        assert_eq!(h.state.repo_query, "");
        h.run(vec![]);
        h.run(vec![escape()]);
        assert!(h.escape_left, "nothing typed: Escape closes the dialog");
    }

    #[test]
    fn nothing_found_offers_to_clear_the_search() {
        let mut h = Harness::new();
        h.state.repo_query = "zzzzzz".into();
        for _ in 0..4 {
            h.run(vec![]);
        }
        assert!(h.state.suggestions.hits().is_empty());
        let clear = h.state.anchors["repo-search-clear"].center();
        h.run(vec![Event::PointerMoved(clear)]);
        h.run(vec![Event::PointerButton { pos: clear, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        h.run(vec![Event::PointerButton { pos: clear, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        h.run(vec![]);
        assert_eq!(h.state.repo_query, "");
        assert_eq!(h.state.suggestions.hits().len(), 330);
    }

    /// A repository added while the list is scrolled far down grows "Your repositories" above the section: the cards being
    /// looked at must not move, not even for a frame.
    #[test]
    fn cards_stay_where_they_are_when_something_appears_above() {
        let mut h = Harness::new();
        h.wheel(6000.0);
        h.forget_drawn();
        h.run(vec![]);
        let before = h.drawn();
        h.above += 72.0;
        for frame in 0..4 {
            h.forget_drawn();
            h.run(vec![]);
            let after = h.drawn();
            assert_eq!(after, before, "on frame {frame} after the change");
        }
    }

    /// Add on a card far down the list: it is that card that says it is being added. (A fixture's task does not run, so
    /// the repository is not really added here and no setting of this computer changes.)
    #[test]
    fn add_on_a_card_adds_that_repository() {
        let mut h = Harness::new();
        h.wheel(6000.0);
        h.forget_drawn();
        h.state.anchors.retain(|k, _| !k.starts_with("repo-card"));
        h.run(vec![]);
        let (i, _) = h.drawn()[2];
        let add = h.state.anchors[&format!("repo-card-add-{i}")].center();
        let url = h.state.suggestions.listing(&h.state.suggestions.hits()[i]).url.clone();
        h.run(vec![Event::PointerMoved(add)]);
        h.run(vec![Event::PointerButton { pos: add, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        h.run(vec![Event::PointerButton { pos: add, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        assert_eq!(h.state.repo_adding.as_deref(), Some(url.as_str()));
    }

    /// An Add pressed far down the list fails: the message is at the dialog's top, and the person is brought to it.
    #[test]
    fn an_error_brings_the_person_to_it() {
        let mut h = Harness::new();
        h.wheel(6000.0);
        h.state.error = Some("The repository could not be added.".into());
        for _ in 0..3 {
            h.forget_drawn();
            h.run(vec![]);
        }
        assert_eq!(h.drawn()[0].0, 0, "back at the first card");
        // Once: dismissed or not, the person can scroll away again.
        h.wheel(6000.0);
        h.forget_drawn();
        h.run(vec![]);
        assert!(h.drawn()[0].0 > 10);
    }

    /// The number asked for: one frame of the section with all 330 repositories, while it scrolls. Printed with
    /// `--nocapture`. A debug build of this crate (its dependencies are optimised): the app's release build is faster.
    #[test]
    fn a_frame_of_330_cards_costs_about_what_a_frame_of_10_does() {
        fn median(mut h: Vec<Duration>) -> Duration {
            h.sort();
            h[h.len() / 2]
        }
        let mut scrolling = Harness::new();
        scrolling.run(vec![]);
        let mut in_view = vec![];
        for i in 0..240 {
            let delta = if i % 120 < 60 { 40.0 } else { -40.0 };
            let events = vec![
                Event::PointerMoved(pos2(600.0, 400.0)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, delta),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                    source: egui::MouseWheelSource::Wheel,
                },
            ];
            let t = Instant::now();
            scrolling.run(events);
            in_view.push(t.elapsed());
        }
        // Every card laid out and drawn, as a plain list of them would: a window tall enough to see them all.
        let mut all = Harness::new();
        all.view = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 40_000.0));
        let mut drawn_all = vec![];
        for _ in 0..30 {
            let t = Instant::now();
            all.run_all();
            drawn_all.push(t.elapsed());
        }
        let worst = in_view.iter().max().copied().unwrap_or_default();
        let (in_view, drawn_all) = (median(in_view), median(drawn_all));
        eprintln!("a frame with 330 repositories, tessellated: {in_view:?} (worst {worst:?}) scrolling, {drawn_all:?} with all of them drawn");
        assert!(in_view * 3 < drawn_all, "drawing what is in view must cost far less than drawing them all: {in_view:?} against {drawn_all:?}");
    }

    impl Harness {
        /// One frame of the section in a view that holds all of it.
        fn run_all(&mut self) {
            self.frame += 1;
            let input = RawInput { screen_rect: Some(self.view), time: Some(self.frame as f64 / 60.0), ..Default::default() };
            let state = &mut self.state;
            let mut output = self.ctx.run_ui(input, |ui| {
                let body = Rect::from_min_size(pos2(330.0, 0.0), vec2(572.0, 40_000.0));
                let mut bu = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("body"));
                bu.set_clip_rect(body);
                section(&mut bu, state);
            });
            output.textures_delta.clear();
            let _ = self.ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
        }
    }
}
