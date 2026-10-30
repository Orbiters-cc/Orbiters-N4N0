//! What the Unity projects page and its dialogs draw alike: avatar pictures that fade in once loaded, a project's cover
//! (its avatars as a collage), tinted icon tiles, packages' own icons, selection rings and the panel's cards.

use super::state::{PackageIcon, State};
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
    pub const ARROW_UP: &str = "\u{E08E}";
    pub const ARROW_DOWN: &str = "\u{E03E}";
    pub const GIT_BRANCH: &str = "\u{E278}";
    pub const GIT_DIFF: &str = "\u{E27C}";
    pub const GITHUB: &str = "\u{E576}";
    pub const GITLAB: &str = "\u{E694}";
}

/// An avatar's (or a world's) picture in `rect` (squircle corners `radii`, circular radii from the top-left
/// clockwise): until it loads, or without one, its initial on a soft tile of its own colour; the picture fades in
/// over it, and a different picture later over the one before.
pub(super) fn avatar(p: &Painter, state: &State, a: &Avatar, rect: Rect, radii: [f32; 4], zoom: f32) {
    picture(p, state, a, rect, radii, zoom, true);
}

/// [`avatar`]; without `initial`, the picture fades in over what is already there (a cover's own placeholder).
fn picture(p: &Painter, state: &State, a: &Avatar, rect: Rect, radii: [f32; 4], zoom: f32, initial: bool) {
    let tex = state.thumbs.get(&a.id);
    let k = feedback::loaded(p.ctx(), Id::unique(("avatar-picture", &a.id)), tex.is_some());
    if k < 1.0 && initial {
        let tint = widgets::name_color(&a.name);
        feedback::fill_corners(p, rect, radii, theme::mix(theme::INK_750, tint, 0.16));
        let shade = Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max);
        paint::fill_gradient(
            p,
            &paint::rounded_rect_points_corners(shade, [0.0, 0.0, radii[2], radii[3]]),
            &paint::Gradient::vertical(shade, &[(0.0, Color32::TRANSPARENT), (1.0, theme::alpha(tint, 0.1))]),
        );
        let initial = a.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
        let size = (rect.height() * 0.4).clamp(13.0, 44.0);
        widgets::text(p, rect.center(), Align2::CENTER_CENTER, &Txt::new(initial, size).weight(weight::SEMIBOLD).color(theme::alpha(tint, 0.85 * (1.0 - k))));
    }
    if let Some(t) = tex {
        let zoom = zoom * (1.0 + 0.04 * (1.0 - k));
        // A different picture in place of the one shown: it fades in over that one, which stays until it is covered.
        match state.thumb_before.get(&a.id) {
            Some((before, serial)) => {
                let x = State::thumb_swap(p.ctx(), &a.id, *serial).clamp(0.0, 1.0);
                feedback::cover(p, rect, radii, before, zoom, k);
                feedback::cover(p, rect, radii, t, zoom, k * x);
            }
            None => feedback::cover(p, rect, radii, t, zoom, k),
        }
    }
}

/// The avatars and worlds of `project` whose pictures are loaded, worlds first (a world project's own thumbnail).
pub(super) fn pictured<'a>(state: &State, project: &'a Project) -> Vec<&'a Avatar> {
    project.worlds.iter().chain(&project.avatars).filter(|a| state.thumbs.contains_key(&a.id)).collect()
}

/// A project's cover in `rect` (squircle corners `radii`): the pictures it has, as a collage (one alone, two side by
/// side, three as one large and two small, four or more in a square with how many more on the last); avatars without
/// a picture are left out. A project without any picture shows its kind's icon on a tile.
pub(super) fn cover(p: &Painter, state: &State, project: &Project, rect: Rect, radii: [f32; 4], zoom: f32) {
    let pictures = pictured(state, project);
    let [tl, tr, br, bl] = radii;
    let gap = 2.0;
    let settled = pictures.iter().all(|a| feedback::loaded(p.ctx(), Id::unique(("avatar-picture", &a.id)), true) >= 1.0);
    if pictures.is_empty() || !settled {
        let (tint, glyph) = kind_look(project);
        feedback::fill_corners(p, rect, radii, theme::mix(theme::INK_800, tint, 0.08));
        let s = (rect.height() * 0.34).clamp(20.0, 64.0);
        let tile = Rect::from_center_size(rect.center(), vec2(s, s));
        p.rect_filled(tile, theme::radius(s * 0.3), theme::alpha(tint, 0.14));
        widgets::icon(p, tile.center(), glyph, s * 0.46, tint);
    }
    match pictures.len() {
        0 => {}
        1 => picture(p, state, pictures[0], rect, radii, zoom, false),
        2 => {
            let w = (rect.width() - gap) * 0.5;
            picture(p, state, pictures[0], Rect::from_min_size(rect.min, vec2(w, rect.height())), [tl, 0.0, 0.0, bl], zoom, false);
            picture(p, state, pictures[1], Rect::from_min_max(pos2(rect.right() - w, rect.top()), rect.max), [0.0, tr, br, 0.0], zoom, false);
        }
        3 => {
            let w = (rect.width() - gap) * 0.58;
            let h = (rect.height() - gap) * 0.5;
            picture(p, state, pictures[0], Rect::from_min_size(rect.min, vec2(w, rect.height())), [tl, 0.0, 0.0, bl], zoom, false);
            let x = rect.left() + w + gap;
            picture(p, state, pictures[1], Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right(), rect.top() + h)), [0.0, tr, 0.0, 0.0], zoom, false);
            picture(p, state, pictures[2], Rect::from_min_max(pos2(x, rect.bottom() - h), rect.max), [0.0, 0.0, br, 0.0], zoom, false);
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
            for (a, (r, rr)) in pictures.iter().zip(cells) {
                picture(p, state, a, r, rr, zoom, false);
            }
            if n > 4 {
                let (r, rr) = cells[3];
                feedback::fill_corners(p, r, rr, theme::black(0.55));
                let size = (r.height() * 0.28).clamp(11.0, 18.0);
                widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(format!("+{}", n - 3), size).weight(weight::SEMIBOLD));
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

/// A package's icon in `rect`: its tinted tile with the package glyph until the icon arrives, then the icon fading in
/// over it: a picture fills the tile, a mark sits in it on a plate that shows it (light behind a dark mark).
pub(super) fn package_icon(p: &Painter, icon: Option<&PackageIcon>, rect: Rect, name: &str) {
    icon_tile(p, icon, rect, name, glyph::PACKAGE, package_tint(name), 1.0);
}

/// A package repository's icon in `rect` (its website's icon, or the logo Orbiters keeps for it): its tinted tile with
/// the stack glyph until it arrives, then the icon fading in; a repository turned off shows it dimmed.
pub(super) fn repo_icon(p: &Painter, icon: Option<&PackageIcon>, rect: Rect, url: &str, enabled: bool) {
    let on = anim::toggle(p.ctx(), Id::unique(("repo-icon-on", url)), enabled, Spring::SOFT).clamp(0.0, 1.0);
    icon_tile(p, icon, rect, url, icons::STACK, theme::mix(theme::FG_4, theme::ACCENT, on), 0.45 + 0.55 * on);
}

/// An icon on its tile: the tile tinted `tint` with `placeholder` until `icon` arrives, then the icon fading in over it
/// (a picture fills the tile, a mark sits on a plate that shows it), at `opacity`.
fn icon_tile(p: &Painter, icon: Option<&PackageIcon>, rect: Rect, key: &str, placeholder_glyph: &str, tint: Color32, opacity: f32) {
    let mut p = p.clone();
    p.multiply_opacity(opacity);
    let p = &p;
    let k = feedback::loaded(p.ctx(), Id::unique(("package-icon", key)), icon.is_some());
    let radius = rect.height() * 0.3;
    let placeholder = theme::alpha(tint, 0.13);
    let plate = match icon {
        Some(i) if i.fills => Color32::TRANSPARENT,
        Some(i) if i.dark => theme::white(0.88),
        _ => theme::white(0.07),
    };
    feedback::fill_corners(p, rect, [radius; 4], theme::mix(placeholder, plate, k));
    if k < 1.0 {
        widgets::icon(p, rect.center(), placeholder_glyph, rect.height() * 0.47, theme::alpha(tint, 1.0 - k));
    }
    let Some(i) = icon else { return };
    if i.fills {
        feedback::cover(p, rect, [radius; 4], &i.tex, 1.0 + 0.1 * (1.0 - k), k);
        // A hairline keeps a dark picture's edge on the dark cards.
        paint::stroke_squircle(p, rect, theme::squircle(radius), egui::Stroke::new(1.0, theme::white(0.08 * k)));
    } else {
        // Contained, a little larger as it settles.
        let room = rect.shrink(rect.height() * 0.18).size() * (0.86 + 0.14 * k);
        let size = i.tex.size_vec2();
        let scale = (room.x / size.x).min(room.y / size.y);
        let r = Rect::from_center_size(rect.center(), size * scale);
        p.image(i.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(k));
    }
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
    card_with(ui, salt, |p, t| tile(p, t, glyph, theme::ACCENT), title, count, right, body);
}

/// [`card`] with its own tile: `tile` draws into the heading's 32-point square.
pub(super) fn card_with(
    ui: &mut Ui,
    salt: &str,
    tile: impl FnOnce(&Painter, Rect),
    title: &str,
    count: Option<String>,
    right: impl FnOnce(&mut Ui, Rect),
    body: impl FnOnce(&mut Ui),
) {
    let width = ui.available_width();
    let top_left = ui.cursor().min;
    let bg = ui.painter().add(Shape::Noop);
    let inner = Rect::from_min_size(top_left + vec2(16.0, 14.0), vec2(width - 32.0, 100_000.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(egui::Align::Min)).id_salt(("panel-card", salt)));
    child.spacing_mut().item_spacing = vec2(0.0, 8.0);
    let (head, _) = child.allocate_exact_size(vec2(inner.width(), 36.0), Sense::hover());
    let t = Rect::from_min_size(pos2(head.left(), head.center().y - 16.0), vec2(32.0, 32.0));
    tile(child.painter(), t);
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
