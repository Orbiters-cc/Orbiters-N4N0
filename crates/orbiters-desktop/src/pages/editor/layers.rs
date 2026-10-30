//! The layer list on the editor's left: Crop and rotate on top, then the layers top first (the last applied on top,
//! as Lightroom and Photoshop list them), each with its eye, its kind and how much of it shows; a VRChat multi-layer
//! picture's own layers (the interface, the people, the world) each with theirs under it. A row is chosen by a press,
//! dragged up or down its own stack to move it, removed by its bin. Add opens the kinds of layer; a new one goes over
//! the layer chosen, in its stack.

use super::{glyph, tint, widgets as w, Editor, Selected};
use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::photo::{Effect, Kind, Layer, Mask};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size};
use orbiters_ui::icons;

const ROW_H: f32 = 46.0;

/// A row being dragged to another place in its stack.
#[derive(Clone, Copy, Debug)]
struct RowDrag {
    stack: Option<usize>,
    id: u32,
    /// Where in the row it was grabbed.
    grab: f32,
}

#[derive(Default)]
pub(super) struct State {
    drag: Option<RowDrag>,
}

pub(super) fn show(ed: &mut Editor, ui: &mut Ui, rect: Rect, title: &str) {
    widgets::panel(ui.painter(), rect, theme::CARD_RADIUS);
    let inner = rect.shrink2(vec2(14.0, 14.0));
    let mut pu = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(ed.id.with("layers")));
    pu.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let (head, _) = pu.allocate_exact_size(vec2(inner.width(), 36.0), Sense::hover());
    widgets::text(pu.painter(), pos2(head.left() + 4.0, head.center().y), Align2::LEFT_CENTER, &Txt::new("Layers", 15.5).weight(weight::SEMIBOLD));
    let add = Button::new(if ed.adding { "Done" } else { "Add" }).icon(if ed.adding { icons::CHECK } else { icons::PLUS }).size(Size::Sm).id_salt("editor-add");
    let aw = add.desired_width(&pu);
    if super::super::sharing::pressed(add.show_at(&mut pu, Rect::from_min_size(pos2(head.right() - aw, head.center().y - 16.0), vec2(aw, 32.0)))) {
        ed.adding = !ed.adding;
    }
    pu.add_space(8.0);
    let open = anim::toggle(pu.ctx(), ed.id.with("adding"), ed.adding, Spring::SOFT).clamp(0.0, 1.0);
    if open > 0.01 {
        add_grid(ed, &mut pu, open, title);
    }
    egui::ScrollArea::vertical().id_salt(ed.id.with("layers-scroll")).auto_shrink([false, false]).show(&mut pu, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        crop_row(ed, ui);
        if ed.doc.sources.is_empty() {
            stack_rows(ed, ui, None, 0.0);
            if ed.live.is_some() {
                picture_row(ui, "The 3D view", icons::CUBE, "Drawn live as you move around");
            } else {
                picture_row(ui, "The picture", icons::IMAGE, "Never changed: saving makes a copy");
            }
        } else {
            group_row(ui, "Whole picture", icons::IMAGES, &format!("{} over all its layers", count(ed.doc.layers.len())));
            stack_rows(ed, ui, None, 0.0);
            for i in (0..ed.doc.sources.len()).rev() {
                source_row(ed, ui, i);
                stack_rows(ed, ui, Some(i), 18.0);
            }
        }
        ui.add_space(12.0);
    });
}

fn count(n: usize) -> String {
    match n {
        0 => "No effects".into(),
        1 => "1 effect".into(),
        n => format!("{n} effects"),
    }
}

/// The kinds of layer, two by two; a press adds one.
fn add_grid(ed: &mut Editor, ui: &mut Ui, open: f32, title: &str) {
    let w = ui.available_width();
    let cols = 2usize;
    let tile_h = 50.0;
    let rows = Kind::ALL.len().div_ceil(cols);
    let full_h = rows as f32 * (tile_h + 6.0) + 30.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, full_h * open), Sense::hover());
    let mut gu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(r.min, vec2(w, full_h))).id_salt("editor-add-grid"));
    gu.set_clip_rect(r.intersect(ui.clip_rect()));
    gu.multiply_opacity(open);
    let target = match ed.selected {
        Selected::Layer(s, _) => s,
        Selected::Source(i) => Some(i),
        _ => None,
    };
    let to = match target.and_then(|i| ed.doc.sources.get(i)) {
        Some(s) => format!("Adds to {}", s.name),
        None => "Adds to the whole picture".into(),
    };
    widgets::text(gu.painter(), pos2(r.left() + 4.0, r.top() + 10.0), Align2::LEFT_CENTER, &Txt::new(to, 11.5).color(theme::FG_3));
    let tw = (w - 6.0) / cols as f32;
    for (i, kind) in Kind::ALL.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.left() + (i % cols) as f32 * (tw + 6.0), r.top() + 24.0 + (i / cols) as f32 * (tile_h + 6.0)), vec2(tw, tile_h));
        let id = ed.id.with(("add", i));
        let resp = gu.interact(cell, id, Sense::click());
        let hover = anim::toggle(gu.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let press = anim::toggle(gu.ctx(), id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let c = Rect::from_center_size(cell.center(), cell.size() * (1.0 - 0.03 * press));
        let p = gu.painter();
        p.rect_filled(c, theme::radius(12.0), theme::mix(theme::white(0.04), theme::white(0.08), hover));
        let tile = Rect::from_center_size(pos2(c.left() + 22.0, c.center().y), vec2(28.0, 28.0));
        p.rect_filled(tile, theme::radius(9.0), theme::alpha(tint(*kind), 0.16));
        widgets::icon(p, tile.center(), glyph(*kind), 15.0, tint(*kind));
        let name = if *kind == Kind::Aberration { "Aberration" } else { kind.name() };
        widgets::text_truncated(p, pos2(tile.right() + 8.0, c.center().y), Align2::LEFT_CENTER, &Txt::new(name, 12.5).weight(weight::MEDIUM), c.right() - tile.right() - 12.0);
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(kind.about());
        if widgets::pressed(&resp) {
            add(ed, *kind, title);
        }
    }
    ui.add_space(8.0 * open);
}

/// Adds a layer of `kind` over the chosen one, in its stack (a text saying the world's name).
pub(super) fn add(ed: &mut Editor, kind: Kind, title: &str) {
    let stack = match ed.selected {
        Selected::Layer(s, _) => s,
        Selected::Source(i) => Some(i),
        _ => None,
    };
    let mut layer = ed.doc.layer(kind.effect());
    if let Effect::Text(t) = &mut layer.effect {
        if !title.trim().is_empty() {
            t.value = title.trim().to_string();
        }
    }
    let id = layer.id;
    let after = match ed.selected {
        Selected::Layer(s, sel) if s == stack => ed.doc.stack(stack).iter().position(|l| l.id == sel),
        _ => None,
    };
    let list = ed.doc.stack_mut(stack);
    match after {
        Some(i) => list.insert(i + 1, layer),
        None => list.push(layer),
    }
    ed.selected = Selected::Layer(stack, id);
    ed.adding = false;
}

/// Removes the chosen layer; the one under it (or over) is chosen next.
pub(super) fn delete_selected(ed: &mut Editor) {
    let Selected::Layer(stack, id) = ed.selected else { return };
    let list = ed.doc.stack_mut(stack);
    let Some(i) = list.iter().position(|l| l.id == id) else { return };
    list.remove(i);
    let next = if i > 0 { list.get(i - 1) } else { list.first() }.map(|l| l.id);
    ed.selected = match next {
        Some(n) => Selected::Layer(stack, n),
        None => stack.map_or(Selected::Nothing, Selected::Source),
    };
}

/// A copy of the chosen layer over it.
pub(super) fn duplicate_selected(ed: &mut Editor) {
    let Selected::Layer(stack, id) = ed.selected else { return };
    let Some(i) = ed.doc.stack(stack).iter().position(|l| l.id == id) else { return };
    let mut copy = ed.doc.stack(stack)[i].clone();
    copy.id = ed.doc.next_id;
    ed.doc.next_id += 1;
    let new = copy.id;
    ed.doc.stack_mut(stack).insert(i + 1, copy);
    ed.selected = Selected::Layer(stack, new);
}

/// Moves the chosen layer `by` places up (over the next) or down its stack.
pub(super) fn nudge_selected(ed: &mut Editor, by: i64) {
    let Selected::Layer(stack, id) = ed.selected else { return };
    let list = ed.doc.stack_mut(stack);
    let Some(i) = list.iter().position(|l| l.id == id) else { return };
    let j = (i as i64 + by).clamp(0, list.len() as i64 - 1) as usize;
    let l = list.remove(i);
    list.insert(j, l);
}

/// The background and press of a row; true when pressed.
fn row_frame(ui: &mut Ui, id: Id, r: Rect, selected: bool, sense: Sense) -> egui::Response {
    let resp = ui.interact(r, id, sense);
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let chosen = anim::toggle(ui.ctx(), id.with("chosen"), selected, Spring::SOFT);
    let p = ui.painter();
    p.rect_filled(r, theme::radius(12.0), theme::mix(theme::white(0.035 * hover), theme::alpha(theme::ACCENT, 0.13), chosen));
    if chosen > 0.01 {
        p.rect_stroke(r, theme::radius(12.0), egui::Stroke::new(1.0, theme::alpha(theme::ACCENT, 0.45 * chosen)), egui::StrokeKind::Inside);
    }
    resp
}

/// An icon tile and two lines of text in a row.
#[allow(clippy::too_many_arguments)]
fn row_text(ui: &Ui, r: Rect, left: f32, glyph: &str, color: egui::Color32, title: &str, sub: &str, right: f32) {
    let p = ui.painter();
    let tile = Rect::from_center_size(pos2(left + 14.0, r.center().y), vec2(28.0, 28.0));
    p.rect_filled(tile, theme::radius(9.0), theme::alpha(color, 0.16));
    widgets::icon(p, tile.center(), glyph, 15.0, color);
    let x = tile.right() + 10.0;
    let max = (right - x).max(20.0);
    widgets::text_truncated(p, pos2(x, r.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(title, 13.0).weight(weight::MEDIUM), max);
    widgets::text_truncated(p, pos2(x, r.center().y + 9.0), Align2::LEFT_CENTER, &Txt::new(sub, 11.0).color(theme::FG_3), max);
}

fn crop_row(ed: &mut Editor, ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let id = ed.id.with("row-crop");
    let selected = ed.selected == Selected::Crop;
    let resp = row_frame(ui, id, r, selected, Sense::click());
    let g = &ed.doc.geometry;
    let mut parts = Vec::new();
    if !g.turns.is_multiple_of(4) {
        parts.push(format!("turned {}°", (g.turns % 4) as u32 * 90));
    }
    if g.flip_h || g.flip_v {
        parts.push("flipped".to_string());
    }
    if g.straighten != 0.0 {
        parts.push(format!("straightened {:.1}°", g.straighten));
    }
    if g.vertical != 0.0 || g.horizontal != 0.0 {
        parts.push("perspective".to_string());
    }
    if g.crop != orbiters_ui::photo::Crop::FULL {
        parts.push(if g.aspect == orbiters_ui::photo::Aspect::Free { "cropped".into() } else { format!("cropped {}", g.aspect.name()) });
    }
    let sub = if parts.is_empty() {
        "The whole picture".to_string()
    } else {
        let s = parts.join(", ");
        s[..1].to_uppercase() + &s[1..]
    };
    row_text(ui, r, r.left() + 8.0, icons::CROP, theme::ACCENT, "Crop and rotate", &sub, r.right() - 8.0);
    if widgets::pressed(&resp.on_hover_cursor(CursorIcon::PointingHand)) {
        ed.selected = if selected { Selected::Nothing } else { Selected::Crop };
    }
}

/// A row that only says something (the picture under a plain picture's layers).
fn picture_row(ui: &mut Ui, title: &str, glyph: &str, sub: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    row_text(ui, r, r.left() + 8.0, glyph, theme::FG_3, title, sub, r.right() - 8.0);
    widgets::icon(ui.painter(), pos2(r.right() - 20.0, r.center().y), icons::LOCK_SIMPLE, 13.0, theme::FG_4);
}

fn group_row(ui: &mut Ui, title: &str, glyph: &str, sub: &str) {
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let p = ui.painter();
    widgets::icon(p, pos2(r.left() + 16.0, r.center().y), glyph, 14.0, theme::FG_3);
    widgets::text(p, pos2(r.left() + 32.0, r.center().y - 7.0), Align2::LEFT_CENTER, &Txt::caps(title));
    widgets::text_truncated(p, pos2(r.left() + 32.0, r.center().y + 9.0), Align2::LEFT_CENTER, &Txt::new(sub, 11.0).color(theme::FG_4), r.width() - 40.0);
}

/// The icon of one of VRChat's layers.
pub(super) fn source_glyph(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "environment" => icons::GLOBE_HEMISPHERE_WEST,
        "player" | "players" | "users" => icons::USERS,
        "ui" => icons::FRAME_CORNERS,
        _ => icons::IMAGE,
    }
}

/// VRChat's name for one of its layers as the list says it.
pub(super) fn source_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "environment" => "World (Environment)".into(),
        "player" | "players" | "users" => "People (Player)".into(),
        "ui" => "Interface (UI)".into(),
        _ => name.to_string(),
    }
}

fn eye(ui: &mut Ui, id: Id, at: egui::Pos2, visible: &mut bool) {
    let r = Rect::from_center_size(at, vec2(28.0, 28.0));
    let resp = IconButton::new(if *visible { icons::EYE } else { icons::EYE_SLASH }, if *visible { "Hide" } else { "Show" }).size(28.0, 14.0).id_salt(id).show_at(ui, r);
    if widgets::activated(&resp) {
        *visible = !*visible;
    }
}

fn source_row(ed: &mut Editor, ui: &mut Ui, i: usize) {
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let id = ed.id.with(("row-source", i));
    let selected = ed.selected == Selected::Source(i);
    let resp = row_frame(ui, id, r, selected, Sense::click());
    let s = &ed.doc.sources[i];
    let sub = if s.opacity < 1.0 { format!("{} · {}", count(s.layers.len()), w::percent(s.opacity)) } else { count(s.layers.len()) };
    row_text(ui, r, r.left() + 36.0, source_glyph(&s.name), theme::FG_2, &source_name(&s.name), &sub, r.right() - 8.0);
    let mut visible = s.visible;
    eye(ui, id.with("eye"), pos2(r.left() + 20.0, r.center().y), &mut visible);
    ed.doc.sources[i].visible = visible;
    if widgets::pressed(&resp.on_hover_cursor(CursorIcon::PointingHand)) {
        ed.selected = Selected::Source(i);
    }
}

/// What a layer's row says under its name.
fn layer_sub(l: &Layer) -> String {
    let mut parts = Vec::new();
    if !l.visible {
        parts.push("Hidden".to_string());
    } else if l.opacity < 1.0 {
        parts.push(w::percent(l.opacity));
    }
    match l.mask {
        Mask::Radial { invert, .. } => parts.push(if invert { "outside a circle" } else { "inside a circle" }.into()),
        Mask::Band { invert, .. } => parts.push(if invert { "outside a band" } else { "along a band" }.into()),
        Mask::Everywhere => {}
    }
    if parts.is_empty() {
        parts.push(summary(&l.effect));
    }
    let s = parts.join(", ");
    if s.is_empty() {
        s
    } else {
        s[..1].to_uppercase() + &s[1..]
    }
}

/// What a layer's settings come to, in a few words ("+0.45 EV, shadows +55").
pub(super) fn summary(e: &Effect) -> String {
    let some = |items: Vec<(f32, String)>| {
        let said: Vec<String> = items.into_iter().filter(|(v, _)| v.abs() > 0.005).map(|(_, s)| s).take(3).collect();
        if said.is_empty() {
            "No change yet".to_string()
        } else {
            said.join(", ")
        }
    };
    match e {
        Effect::Light(v) => some(vec![
            (v.exposure, format!("{:+.2} EV", v.exposure)),
            (v.brightness, format!("brightness {}", w::signed(v.brightness))),
            (v.contrast, format!("contrast {}", w::signed(v.contrast))),
            (v.highlights, format!("highlights {}", w::signed(v.highlights))),
            (v.shadows, format!("shadows {}", w::signed(v.shadows))),
            (v.black_point, format!("black point {}", w::signed(v.black_point))),
        ]),
        Effect::Color(v) => some(vec![
            (v.temperature, if v.temperature > 0.0 { format!("warmer {}", w::signed(v.temperature)) } else { format!("cooler {}", w::signed(v.temperature)) }),
            (v.tint, format!("tint {}", w::signed(v.tint))),
            (v.saturation, format!("saturation {}", w::signed(v.saturation))),
            (v.vibrance, format!("vibrance {}", w::signed(v.vibrance))),
            (v.hue, format!("hue {:+.0}°", v.hue)),
        ]),
        Effect::Curves(c) => {
            let line = |p: &Vec<[f32; 2]>| *p == vec![[0.0, 0.0], [1.0, 1.0]];
            let bent: Vec<&str> = [("RGB", &c.rgb), ("red", &c.red), ("green", &c.green), ("blue", &c.blue)].iter().filter(|(_, p)| !line(p)).map(|(n, _)| *n).collect();
            if bent.is_empty() {
                "Straight curves".into()
            } else {
                format!("{} curve{}", bent.join(", "), if bent.len() > 1 { "s" } else { "" })
            }
        }
        Effect::Vignette(v) => format!("{} edges {}", if v.amount >= 0.0 { "darker" } else { "lighter" }, w::percent(v.amount.abs())),
        Effect::Aberration(a) => format!("Amount {:.2}", a.amount),
        Effect::Comic(c) => format!("{} tones, ink {}{}", c.tones, w::percent(c.ink), if c.dots { ", dots" } else { "" }),
        Effect::Blur(b) => match b.kind {
            orbiters_ui::photo::BlurKind::Directional => format!("{:.1} % long, {:+.0}°", b.radius * 100.0, b.angle),
            orbiters_ui::photo::BlurKind::Lens => format!("Radius {:.1} %, bokeh {}", b.radius * 100.0, w::percent(b.bokeh)),
            orbiters_ui::photo::BlurKind::Gaussian => format!("Radius {:.1} %", b.radius * 100.0),
        },
        Effect::Text(t) => {
            let font = if t.font.is_empty() { "Inter".to_string() } else { std::path::Path::new(&t.font).file_stem().map_or(t.font.clone(), |s| s.to_string_lossy().into_owned()) };
            format!("{}, {font}", t.style.name())
        }
    }
}

/// The rows of a stack, top first; a row dragged follows the pointer and the others make room.
fn stack_rows(ed: &mut Editor, ui: &mut Ui, stack: Option<usize>, indent: f32) {
    let n = ed.doc.stack(stack).len();
    if n == 0 {
        return;
    }
    let width = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(width, n as f32 * (ROW_H + 2.0)), Sense::hover());
    let pointer = ui.ctx().pointer_interact_pos();
    let dragging = ed.rows.drag.filter(|d| d.stack == stack);
    // Where the dragged row would land (shown order, top first).
    let target = dragging.and_then(|d| pointer.map(|p| (((p.y - d.grab - area.top()) / (ROW_H + 2.0)).round().clamp(0.0, (n - 1) as f32)) as usize));
    let order: Vec<u32> = ed.doc.stack(stack).iter().rev().map(|l| l.id).collect();
    let dragged_at = dragging.and_then(|d| order.iter().position(|id| *id == d.id));
    let mut slot = 0usize;
    let mut released: Option<(u32, usize)> = None;
    for (shown, id) in order.iter().enumerate() {
        let is_dragged = dragged_at == Some(shown);
        let place = if is_dragged {
            target.unwrap_or(shown)
        } else {
            if target.is_some_and(|t| t == slot) && dragged_at.is_some() {
                slot += 1;
            }
            let s = slot;
            slot += 1;
            s
        };
        let row_id = ed.id.with(("row", stack, *id));
        let y_target = area.top() + place as f32 * (ROW_H + 2.0);
        let y = if is_dragged {
            pointer.map_or(y_target, |p| (p.y - dragging.map_or(0.0, |d| d.grab)).clamp(area.top() - 8.0, area.bottom() - ROW_H + 8.0))
        } else {
            anim::value(ui.ctx(), row_id.with("y"), y_target, Spring::SNAPPY)
        };
        let r = Rect::from_min_size(pos2(area.left() + indent, y), vec2(width - indent, ROW_H));
        let Some(l) = ed.doc.stack(stack).iter().find(|l| l.id == *id).cloned() else { continue };
        let selected = ed.selected == Selected::Layer(stack, *id);
        let mut lu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(row_id));
        if is_dragged {
            lu.painter().rect_filled(r.translate(vec2(0.0, 4.0)), theme::radius(12.0), theme::black(0.35));
            lu.painter().rect_filled(r, theme::radius(12.0), theme::INK_750);
        }
        let resp = row_frame(&mut lu, row_id, r, selected || is_dragged, Sense::click_and_drag());
        let kind = l.effect.kind();
        let mut title = l.effect.title();
        if title.chars().count() > 40 {
            title = title.chars().take(40).collect::<String>() + "…";
        }
        let right_room = if selected || resp.hovered() { r.right() - 64.0 } else { r.right() - 10.0 };
        let mut faded = lu.new_child(UiBuilder::new().max_rect(r).id_salt(row_id.with("text")));
        if !l.visible {
            faded.multiply_opacity(0.45);
        }
        row_text(&faded, r, r.left() + 36.0, glyph(kind), tint(kind), &title, &layer_sub(&l), right_room);
        let mut visible = l.visible;
        eye(&mut lu, row_id.with("eye"), pos2(r.left() + 20.0, r.center().y), &mut visible);
        if visible != l.visible {
            if let Some(layer) = ed.doc.stack_mut(stack).iter_mut().find(|x| x.id == *id) {
                layer.visible = visible;
            }
        }
        if selected || resp.hovered() {
            widgets::icon(lu.painter(), pos2(r.right() - 52.0, r.center().y), icons::DOTS_SIX_VERTICAL, 14.0, theme::FG_4);
            let bin = Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(28.0, 28.0));
            if widgets::activated(&IconButton::new(icons::TRASH, "Remove this layer").size(28.0, 14.0).danger().id_salt(row_id.with("bin")).show_at(&mut lu, bin)) {
                ed.selected = Selected::Layer(stack, *id);
                delete_selected(ed);
                return;
            }
        }
        if resp.drag_started() {
            let grab = resp.interact_pointer_pos().map_or(ROW_H * 0.5, |p| p.y - r.top());
            ed.rows.drag = Some(RowDrag { stack, id: *id, grab });
            ed.selected = Selected::Layer(stack, *id);
        }
        if is_dragged && resp.drag_stopped() {
            released = Some((*id, target.unwrap_or(shown)));
        }
        if widgets::pressed(&resp) {
            ed.selected = Selected::Layer(stack, *id);
        }
        resp.on_hover_cursor(if is_dragged { CursorIcon::Grabbing } else { CursorIcon::PointingHand });
    }
    if dragging.is_some() && !ui.ctx().input(|i| i.pointer.any_down()) && released.is_none() {
        // Let go somewhere else.
        if let (Some(d), Some(t)) = (dragging, target) {
            released = Some((d.id, t));
        } else {
            ed.rows.drag = None;
        }
    }
    if let Some((id, shown)) = released {
        ed.rows.drag = None;
        let list = ed.doc.stack_mut(stack);
        if let Some(i) = list.iter().position(|l| l.id == id) {
            let l = list.remove(i);
            // Shown top first: place `shown` from the top is index n - 1 - shown.
            let to = (n - 1).saturating_sub(shown).min(list.len());
            list.insert(to, l);
        }
    }
}
