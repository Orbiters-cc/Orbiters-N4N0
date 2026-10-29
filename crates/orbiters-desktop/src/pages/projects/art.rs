//! What the Unity projects page and its dialogs draw alike: avatar pictures that fade in once loaded, a project's cover
//! (its avatars as a collage), tinted icon tiles, selection rings and the panel's cards.

use super::state::State;
use egui::{pos2, vec2, Align2, Color32, Id, Layout, Painter, Rect, Sense, Shape, Ui, UiBuilder};
use orbiters_projects::{
    avatars::Avatar,
    discovery::{self, Project},
};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{feedback, icons, paint, widgets};

/// Phosphor glyphs the shared icon list does not have yet.
pub(super) mod glyph {
    pub const PACKAGE: &str = "\u{E390}";
    pub const GIT_COMMIT: &str = "\u{E27A}";
    pub const FOLDER_PLUS: &str = "\u{E258}";
    pub const TRASH_SIMPLE: &str = "\u{E4A8}";
    pub const ARROW_RIGHT: &str = "\u{E06C}";
}

/// An avatar's picture in `rect` (squircle corners `radii`, circular radii from the top-left clockwise): a quiet
/// placeholder until it loads, then the picture fading in over it.
pub(super) fn avatar(p: &Painter, state: &State, a: &Avatar, rect: Rect, radii: [f32; 4], zoom: f32) {
    feedback::fill_corners(p, rect, radii, theme::INK_750);
    let tex = state.thumbs.get(&a.id);
    let k = feedback::loaded(p.ctx(), Id::unique(("avatar-picture", &a.id)), tex.is_some());
    if k < 1.0 {
        widgets::icon(p, rect.center(), icons::PERSON_ARMS_SPREAD, (rect.height() * 0.32).clamp(14.0, 40.0), theme::alpha(theme::FG_4, 1.0 - k));
    }
    if let Some(t) = tex {
        feedback::cover(p, rect, radii, t, zoom * (1.0 + 0.04 * (1.0 - k)), k);
    }
}

/// A project's cover in `rect` (squircle corners `radii`): its avatars as a collage (one alone, two side by side, three
/// as one large and two small, four or more in a square with how many more on the last), or a tile with its kind's
/// icon when it has none.
pub(super) fn cover(p: &Painter, state: &State, project: &Project, rect: Rect, radii: [f32; 4], zoom: f32) {
    let avatars = &project.avatars;
    let [tl, tr, br, bl] = radii;
    let gap = 2.0;
    match avatars.len() {
        0 => {
            let (tint, glyph) = kind_look(project);
            feedback::fill_corners(p, rect, radii, theme::mix(theme::INK_800, tint, 0.08));
            let s = (rect.height() * 0.34).clamp(32.0, 64.0);
            let tile = Rect::from_center_size(rect.center(), vec2(s, s));
            p.rect_filled(tile, theme::radius(s * 0.3), theme::alpha(tint, 0.14));
            widgets::icon(p, tile.center(), glyph, s * 0.46, tint);
        }
        1 => avatar(p, state, &avatars[0], rect, radii, zoom),
        2 => {
            let w = (rect.width() - gap) * 0.5;
            avatar(p, state, &avatars[0], Rect::from_min_size(rect.min, vec2(w, rect.height())), [tl, 0.0, 0.0, bl], zoom);
            avatar(p, state, &avatars[1], Rect::from_min_max(pos2(rect.right() - w, rect.top()), rect.max), [0.0, tr, br, 0.0], zoom);
        }
        3 => {
            let w = (rect.width() - gap) * 0.58;
            let h = (rect.height() - gap) * 0.5;
            avatar(p, state, &avatars[0], Rect::from_min_size(rect.min, vec2(w, rect.height())), [tl, 0.0, 0.0, bl], zoom);
            let x = rect.left() + w + gap;
            avatar(p, state, &avatars[1], Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right(), rect.top() + h)), [0.0, tr, 0.0, 0.0], zoom);
            avatar(p, state, &avatars[2], Rect::from_min_max(pos2(x, rect.bottom() - h), rect.max), [0.0, 0.0, br, 0.0], zoom);
        }
        n => {
            let w = (rect.width() - gap) * 0.5;
            let h = (rect.height() - gap) * 0.5;
            let cells = [
                (Rect::from_min_size(rect.min, vec2(w, h)), [tl, 0.0, 0.0, 0.0]),
                (Rect::from_min_size(pos2(rect.right() - w, rect.top()), vec2(w, h)), [0.0, tr, 0.0, 0.0]),
                (Rect::from_min_size(pos2(rect.left(), rect.bottom() - h), vec2(w, h)), [0.0, 0.0, 0.0, bl]),
                (Rect::from_min_size(pos2(rect.right() - w, rect.bottom() - h), vec2(w, h)), [0.0, 0.0, br, 0.0]),
            ];
            for (a, (r, rr)) in avatars.iter().zip(cells) {
                avatar(p, state, a, r, rr, zoom);
            }
            if n > 4 {
                let (r, rr) = cells[3];
                feedback::fill_corners(p, r, rr, theme::black(0.55));
                widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(format!("+{}", n - 3), 18.0).weight(weight::SEMIBOLD));
            }
        }
    }
}

/// The tint and icon of a project's kind.
pub(super) fn kind_look(project: &Project) -> (Color32, &'static str) {
    match project.kind {
        discovery::Kind::Worlds => (theme::VIOLET, icons::GLOBE),
        discovery::Kind::Avatars => (theme::BLUE, icons::PERSON_ARMS_SPREAD),
        _ => (theme::CYAN, icons::CUBE),
    }
}

/// A tinted icon tile (the website's form icons, Home's features).
pub(super) fn tile(p: &Painter, rect: Rect, glyph: &str, tint: Color32) {
    p.rect_filled(rect, theme::radius(rect.height() * 0.3), theme::alpha(tint, 0.13));
    widgets::icon(p, rect.center(), glyph, rect.height() * 0.47, tint);
}

/// A package's tint, by who makes it.
pub(super) fn package_tint(name: &str) -> Color32 {
    if name.starts_with("orbiters.") {
        theme::ACCENT
    } else if name.starts_with("com.vrchat.") {
        theme::BLUE
    } else {
        let palette = [theme::VIOLET, theme::CYAN, theme::INDIGO, theme::WARN, theme::PURPLE];
        palette[name.bytes().fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % palette.len()]
    }
}

/// The selection ring around a chosen card (the same green ring as a focused field), springing in.
pub(super) fn selection(p: &Painter, id: Id, rect: Rect, radius: f32, selected: bool) -> f32 {
    let k = anim::toggle(p.ctx(), id, selected, Spring::BOUNCY);
    if k > 0.01 {
        let r = rect.expand(3.0 * k.clamp(0.0, 1.2));
        paint::stroke_squircle(p, r, theme::squircle(radius + 3.0), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, k.clamp(0.0, 1.0))));
    }
    k
}

/// A filled check mark in a green disc (a chosen card's corner).
pub(super) fn check(p: &Painter, center: egui::Pos2, k: f32) {
    if k <= 0.01 {
        return;
    }
    let r = 11.0 * k.clamp(0.0, 1.2);
    p.circle_filled(center, r + 2.0, theme::INK_850);
    p.circle_filled(center, r, theme::ACCENT);
    widgets::icon(p, center, icons::CHECK, 12.0 * k.clamp(0.0, 1.0), theme::ON_ACCENT);
}

/// A small label in caps over a group of rows.
pub(super) fn caps(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    widgets::text(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &Txt::caps(text));
}

/// A card of the project panel: tinted icon tile, title and a count, an action slot on the right of the heading
/// (`right` gets its rect), then the body; sized to what the body drew.
pub(super) fn card(ui: &mut Ui, salt: &str, glyph: &str, title: &str, count: Option<String>, right: impl FnOnce(&mut Ui, Rect), body: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let top_left = ui.cursor().min;
    let bg = ui.painter().add(Shape::Noop);
    let inner = Rect::from_min_size(top_left + vec2(16.0, 14.0), vec2(width - 32.0, 100_000.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(egui::Align::Min)).id_salt(("panel-card", salt)));
    child.spacing_mut().item_spacing = vec2(0.0, 8.0);
    let (head, _) = child.allocate_exact_size(vec2(inner.width(), 36.0), Sense::hover());
    let t = Rect::from_min_size(pos2(head.left(), head.center().y - 16.0), vec2(32.0, 32.0));
    tile(child.painter(), t, glyph, theme::ACCENT);
    let tr = widgets::text(child.painter(), pos2(t.right() + 12.0, head.center().y), Align2::LEFT_CENTER, &Txt::new(title, 14.5).weight(weight::SEMIBOLD));
    if let Some(c) = count {
        widgets::pill_at(child.painter(), &child, pos2(tr.right() + 8.0, head.center().y), None, &c);
    }
    let slot = Rect::from_min_max(pos2(head.right() - 200.0, head.top()), head.max);
    let mut su = child.new_child(UiBuilder::new().max_rect(slot).id_salt(("panel-card-right", salt)));
    right(&mut su, slot);
    child.add_space(2.0);
    body(&mut child);
    let h = child.min_rect().height() + 14.0 + 16.0;
    let rect = Rect::from_min_size(top_left, vec2(width, h));
    ui.painter().set(bg, Shape::mesh(paint::squircle_mesh(rect, theme::squircle(theme::CARD_RADIUS), theme::surface(), 1.0 / ui.ctx().pixels_per_point())));
    ui.allocate_exact_size(vec2(width, h), Sense::hover());
}
