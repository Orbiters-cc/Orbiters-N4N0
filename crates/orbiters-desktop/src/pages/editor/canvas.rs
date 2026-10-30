//! The editor's canvas: the preview as large as it fits, over a checkerboard where the picture is transparent (a
//! VRChat layer, a straightened picture's corners), and what the chosen layer moves on it. Crop and rotate shows the
//! straightened picture whole with the crop over it (the outside dimmed, thirds, corner and edge handles; drag inside
//! to move it, the shape chosen kept). The vignette's and the aberration's centre, a mask's centre and its circle's edge
//! or band, and texts (a press on one chooses it) are dragged on the picture.

use super::{find_mut, Editor, Selected};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Ui};
use orbiters_ui::photo::render::{self, Stage};
use orbiters_ui::photo::{self, geometry, Crop, Effect, Mask};
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::{feedback, paint, viewer, widgets};

/// Which part of the crop is dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// What a drag on the canvas moves.
#[derive(Clone, Copy, Debug)]
enum Drag {
    Crop { part: Part, start: Crop, grab: [f32; 2] },
    /// A centre (a vignette's, an aberration's, a mask's, a text's): where it was grabbed from it.
    Center { mask: bool, offset: [f32; 2] },
    MaskRadius,
}

#[derive(Default)]
pub(super) struct State {
    drag: Option<Drag>,
    checker: Option<egui::TextureHandle>,
}

/// A small repeating checkerboard (for transparent pixels).
fn checker(ctx: &egui::Context, state: &mut State) -> egui::TextureId {
    state
        .checker
        .get_or_insert_with(|| {
            let (a, b) = (Color32::from_gray(38), Color32::from_gray(30));
            let img = egui::ColorImage::new([2, 2], vec![a, b, b, a]);
            let options = egui::TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Nearest, wrap_mode: egui::TextureWrapMode::Repeat, ..Default::default() };
            ctx.load_texture("editor-checker", img, options)
        })
        .id()
}

pub(super) fn show(ed: &mut Editor, ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::radius(theme::CARD_RADIUS), theme::INK_900);
    let Some(full) = ed.loaded.as_ref().map(|l| l.full) else {
        match &ed.error {
            Some(e) => {
                widgets::text_truncated(&p, rect.center(), Align2::CENTER_CENTER, &Txt::new(e, 13.0).color(theme::DANGER), rect.width() - 48.0);
            }
            None => {
                feedback::spinner(&p, rect.center() - vec2(0.0, 18.0), 12.0, 2.5, theme::ACCENT);
                widgets::text(&p, rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, &Txt::new("Reading the picture…", 13.0).color(theme::FG_3));
                ctx.request_repaint();
            }
        }
        return;
    };
    let stage = ed.stage();
    let (fw, fh) = render::full_size(&ed.doc, full, stage);
    let frame = viewer::contain(rect.shrink(28.0), fw as f32 / fh.max(1) as f32);
    let transparent = !ed.doc.sources.is_empty() || stage == Stage::Uncropped;
    if transparent {
        let id = checker(&ctx, &mut ed.canvas);
        let cells = frame.size() / 12.0;
        p.image(id, frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(cells.x / 2.0, cells.y / 2.0)), Color32::WHITE);
    }
    match &ed.texture {
        Some(t) => {
            p.image(t.id(), frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => feedback::shimmer(&p, frame, 4.0),
    }
    // Making the preview of the latest change: a quiet spinner in the corner.
    if ed.preview.as_ref().is_some_and(|pv| pv.busy()) {
        feedback::spinner(&p, pos2(rect.right() - 20.0, rect.top() + 20.0), 7.0, 2.0, theme::FG_3);
        ctx.request_repaint();
    }
    if ed.comparing {
        let t = Txt::new("Original", 11.5).color(theme::FG);
        let w = widgets::text_size(ui, &t).x + 24.0;
        let r = Rect::from_min_size(pos2(frame.left() + 12.0, frame.top() + 12.0), vec2(w, 26.0));
        p.rect_filled(r, theme::radius(13.0), theme::black(0.6));
        widgets::text(&p, r.center(), Align2::CENTER_CENTER, &t);
    } else if ed.selected == Selected::Crop {
        crop(ed, ui, frame, full);
    } else {
        handles(ed, ui, frame, (fw, fh));
    }
    // What saving did, over the canvas's bottom.
    let at = Rect::from_min_max(pos2(rect.left() + 20.0, rect.bottom() - 120.0), pos2(rect.right() - 20.0, rect.bottom() - 12.0));
    if ed.notice.is_some() {
        let mut nu = ui.new_child(egui::UiBuilder::new().max_rect(at).id_salt(ed.id.with("notice-area")));
        let h = 84.0;
        let r = Rect::from_min_size(pos2(at.left(), at.bottom() - h), vec2(at.width(), h));
        nu.painter().rect_filled(r.expand(8.0), theme::radius(18.0), theme::black(0.6));
        let mut inner = nu.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("notice-inner"));
        ed.notice_ui(&mut inner, r);
    }
    if let (Some(e), true) = (&ed.error, ed.loaded.is_some()) {
        let t = Txt::new(e, 12.5).color(theme::DANGER);
        widgets::text_truncated(&p, pos2(rect.center().x, rect.bottom() - 16.0), Align2::CENTER_BOTTOM, &t, rect.width() - 40.0);
    }
}

/// A share of `frame` on screen and back.
fn screen(frame: Rect, q: [f32; 2]) -> Pos2 {
    pos2(frame.left() + q[0] * frame.width(), frame.top() + q[1] * frame.height())
}

fn share(frame: Rect, s: Pos2) -> [f32; 2] {
    [(s.x - frame.left()) / frame.width(), (s.y - frame.top()) / frame.height()]
}

/// The crop over the straightened picture.
fn crop(ed: &mut Editor, ui: &mut Ui, frame: Rect, full: (u32, u32)) {
    let id = ed.id.with("crop-canvas");
    let resp = ui.interact(frame.expand(18.0), id, Sense::click_and_drag());
    let g = &mut ed.doc.geometry;
    let (tw, th) = geometry::turned_size(full.0, full.1, g.turns);
    let c = g.effective_crop(full.0, full.1);
    let cr = Rect::from_min_max(screen(frame, [c.x, c.y]), screen(frame, [c.x + c.w, c.y + c.h]));
    let p = ui.painter().clone();
    // The outside dimmed, the thirds, the frame and its handles.
    let dim = theme::black(0.6);
    p.rect_filled(Rect::from_min_max(frame.min, pos2(frame.right(), cr.top())), 0.0, dim);
    p.rect_filled(Rect::from_min_max(pos2(frame.left(), cr.bottom()), frame.max), 0.0, dim);
    p.rect_filled(Rect::from_min_max(pos2(frame.left(), cr.top()), pos2(cr.left(), cr.bottom())), 0.0, dim);
    p.rect_filled(Rect::from_min_max(pos2(cr.right(), cr.top()), pos2(frame.right(), cr.bottom())), 0.0, dim);
    for k in 1..3 {
        let t = k as f32 / 3.0;
        p.vline(cr.left() + cr.width() * t, cr.y_range(), Stroke::new(1.0, theme::white(0.35)));
        p.hline(cr.x_range(), cr.top() + cr.height() * t, Stroke::new(1.0, theme::white(0.35)));
    }
    p.rect_stroke(cr, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Middle);
    let arm = 18.0f32.min(cr.width() * 0.3).min(cr.height() * 0.3);
    for (corner, dx, dy) in [(cr.left_top(), 1.0, 1.0), (cr.right_top(), -1.0, 1.0), (cr.left_bottom(), 1.0, -1.0), (cr.right_bottom(), -1.0, -1.0)] {
        let s = Stroke::new(4.0, Color32::WHITE);
        p.line_segment([corner, corner + vec2(arm * dx, 0.0)], s);
        p.line_segment([corner, corner + vec2(0.0, arm * dy)], s);
    }
    for (m, horizontal) in [(pos2(cr.center().x, cr.top()), true), (pos2(cr.center().x, cr.bottom()), true), (pos2(cr.left(), cr.center().y), false), (pos2(cr.right(), cr.center().y), false)] {
        let d = if horizontal { vec2(arm * 0.7, 0.0) } else { vec2(0.0, arm * 0.7) };
        p.line_segment([m - d, m + d], Stroke::new(4.0, Color32::WHITE));
    }
    // Which part a press takes.
    let part_at = |s: Pos2| -> Option<Part> {
        let near = |a: f32, b: f32| (a - b).abs() < 14.0;
        let (l, r, t, b) = (near(s.x, cr.left()), near(s.x, cr.right()), near(s.y, cr.top()), near(s.y, cr.bottom()));
        let in_y = s.y > cr.top() - 14.0 && s.y < cr.bottom() + 14.0;
        let in_x = s.x > cr.left() - 14.0 && s.x < cr.right() + 14.0;
        Some(match (l, r, t, b) {
            (true, _, true, _) => Part::TopLeft,
            (_, true, true, _) => Part::TopRight,
            (true, _, _, true) => Part::BottomLeft,
            (_, true, _, true) => Part::BottomRight,
            (true, _, _, _) if in_y => Part::Left,
            (_, true, _, _) if in_y => Part::Right,
            (_, _, true, _) if in_x => Part::Top,
            (_, _, _, true) if in_x => Part::Bottom,
            _ if cr.contains(s) => Part::Move,
            _ => return None,
        })
    };
    if let Some(h) = resp.hover_pos() {
        let cursor = match ed.canvas.drag.map(|d| if let Drag::Crop { part, .. } = d { Some(part) } else { None }).unwrap_or_else(|| part_at(h)) {
            Some(Part::Move) => CursorIcon::Grab,
            Some(Part::Left | Part::Right) => CursorIcon::ResizeHorizontal,
            Some(Part::Top | Part::Bottom) => CursorIcon::ResizeVertical,
            Some(Part::TopLeft | Part::BottomRight) => CursorIcon::ResizeNwSe,
            Some(Part::TopRight | Part::BottomLeft) => CursorIcon::ResizeNeSw,
            None => CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(cursor);
    }
    if resp.drag_started() {
        if let Some(s) = resp.interact_pointer_pos() {
            ed.canvas.drag = part_at(s).map(|part| Drag::Crop { part, start: c, grab: share(frame, s) });
        }
    }
    if let (Some(Drag::Crop { part, start, grab }), Some(s)) = (ed.canvas.drag, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let q = share(frame, s);
            let ratio = g.aspect.ratio(tw as f32 / th.max(1) as f32).map(|r| r * th as f32 / tw as f32);
            g.crop = dragged(part, start, [q[0] - grab[0], q[1] - grab[1]], ratio);
        }
    }
    if resp.drag_stopped() || !ui.ctx().input(|i| i.pointer.any_down()) {
        ed.canvas.drag = None;
    }
}

/// The crop `start` with `part` moved by `d` (shares), keeping `ratio` (width over height in shares) when given.
fn dragged(part: Part, start: Crop, d: [f32; 2], ratio: Option<f32>) -> Crop {
    const MIN: f32 = 0.04;
    let (mut l, mut t, mut r, mut b) = (start.x, start.y, start.x + start.w, start.y + start.h);
    match part {
        Part::Move => {
            let dx = d[0].clamp(-l, 1.0 - r);
            let dy = d[1].clamp(-t, 1.0 - b);
            return Crop { x: l + dx, y: t + dy, w: start.w, h: start.h };
        }
        Part::Left | Part::TopLeft | Part::BottomLeft => l = (l + d[0]).clamp(0.0, r - MIN),
        Part::Right | Part::TopRight | Part::BottomRight => r = (r + d[0]).clamp(l + MIN, 1.0),
        _ => {}
    }
    match part {
        Part::Top | Part::TopLeft | Part::TopRight => t = (t + d[1]).clamp(0.0, b - MIN),
        Part::Bottom | Part::BottomLeft | Part::BottomRight => b = (b + d[1]).clamp(t + MIN, 1.0),
        _ => {}
    }
    let Some(ratio) = ratio else { return Crop { x: l, y: t, w: r - l, h: b - t } };
    // The shape kept: the side dragged sets the size, the opposite side or corner stays.
    let (mut w, mut h) = (r - l, b - t);
    match part {
        Part::Top | Part::Bottom => w = h * ratio,
        _ => h = w / ratio,
    }
    // Within the frame, shrunk as needed.
    let anchor_x = match part {
        Part::Left | Part::TopLeft | Part::BottomLeft => start.x + start.w,
        Part::Top | Part::Bottom => start.x + start.w * 0.5,
        _ => start.x,
    };
    let anchor_y = match part {
        Part::Top | Part::TopLeft | Part::TopRight => start.y + start.h,
        Part::Left | Part::Right => start.y + start.h * 0.5,
        _ => start.y,
    };
    let fx = match part {
        Part::Left | Part::TopLeft | Part::BottomLeft => anchor_x,
        Part::Top | Part::Bottom => (anchor_x * 2.0).min(2.0 - anchor_x * 2.0),
        _ => 1.0 - anchor_x,
    };
    let fy = match part {
        Part::Top | Part::TopLeft | Part::TopRight => anchor_y,
        Part::Left | Part::Right => (anchor_y * 2.0).min(2.0 - anchor_y * 2.0),
        _ => 1.0 - anchor_y,
    };
    let k = (fx / w.max(1e-6)).min(fy / h.max(1e-6)).min(1.0);
    w *= k;
    h *= k;
    let x = match part {
        Part::Left | Part::TopLeft | Part::BottomLeft => anchor_x - w,
        Part::Top | Part::Bottom => anchor_x - w * 0.5,
        _ => anchor_x,
    };
    let y = match part {
        Part::Top | Part::TopLeft | Part::TopRight => anchor_y - h,
        Part::Left | Part::Right => anchor_y - h * 0.5,
        _ => anchor_y,
    };
    geometry::clamp_to_frame(Crop { x, y, w, h })
}

/// A round handle; brighter while held.
fn knob(p: &egui::Painter, at: Pos2, held: bool, square: bool) {
    let r = if held { 9.0 } else { 7.5 };
    p.circle_filled(at, r + 3.0, theme::black(0.35));
    if square {
        let q = Rect::from_center_size(at, vec2(r * 1.8, r * 1.8));
        p.rect_filled(q, theme::radius(3.0), Color32::WHITE);
        p.rect_filled(q.shrink(3.0), theme::radius(2.0), theme::ACCENT);
    } else {
        p.circle_filled(at, r, Color32::WHITE);
        p.circle_filled(at, r - 3.0, theme::ACCENT);
    }
}

/// What the chosen layer moves on the canvas, and texts chosen by a press on them.
fn handles(ed: &mut Editor, ui: &mut Ui, frame: Rect, size: (u32, u32)) {
    let id = ed.id.with("canvas-handles");
    let resp = ui.interact(frame, id, Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(frame.intersect(ui.clip_rect()));
    let held = ed.canvas.drag;
    let (center, mask, text_box) = match ed.layer() {
        Some(l) => {
            let center = match &l.effect {
                Effect::Vignette(v) => Some((v.center, Some(v.size / 1.8f32.sqrt()))),
                Effect::Aberration(a) => Some((a.center, None)),
                Effect::Text(t) => Some((t.center, None)),
                _ => None,
            };
            let text_box = match &l.effect {
                Effect::Text(t) => photo::text::bounds(t, size.0, size.1),
                _ => None,
            };
            (center, l.mask.clone(), text_box)
        }
        None => (None, Mask::Everywhere, None),
    };
    // The mask's shape.
    match &mask {
        Mask::Radial { center, radius, feather, .. } => {
            let c = screen(frame, *center);
            paint::dashed_circle(&p, c, radius * frame.height(), [6.0, 5.0], 1.5, Color32::WHITE, 0.0);
            paint::dashed_circle(&p, c, (radius + feather) * frame.height(), [3.0, 6.0], 1.0, theme::white(0.6), 0.0);
        }
        Mask::Band { center, angle, width, .. } => {
            let c = screen(frame, *center);
            let (s, co) = angle.to_radians().sin_cos();
            let along = vec2(co, s) * frame.width().max(frame.height()) * 1.5;
            let across = vec2(-s, co) * (width * 0.5 * frame.height());
            for side in [-1.0, 1.0] {
                p.line_segment([c + across * side - along, c + across * side + along], Stroke::new(1.5, theme::white(0.85)));
            }
        }
        Mask::Everywhere => {}
    }
    if let Some(b) = text_box {
        let r = Rect::from_min_max(screen(frame, [b[0], b[1]]), screen(frame, [b[2], b[3]])).expand(6.0);
        p.rect_stroke(r, theme::radius(6.0), Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.9)), egui::StrokeKind::Middle);
    } else if let Some((c, ring)) = center {
        let at = screen(frame, c);
        if let Some(rr) = ring {
            paint::dashed_circle(&p, at, rr * frame.height(), [6.0, 5.0], 1.5, theme::white(0.7), 0.0);
        }
        knob(&p, at, matches!(held, Some(Drag::Center { mask: false, .. })), false);
    }
    if let Some(c) = mask.center() {
        knob(&p, screen(frame, c), matches!(held, Some(Drag::Center { mask: true, .. })), true);
    }

    // A press: the nearest handle, a mask circle's edge, or a text under the pointer.
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started() || widgets::pressed(&resp) {
        if let Some(s) = pointer {
            let q = share(frame, s);
            let near = |c: [f32; 2]| screen(frame, c).distance(s) < 16.0;
            let edge = match &mask {
                Mask::Radial { center, radius, .. } => (screen(frame, *center).distance(s) - radius * frame.height()).abs() < 8.0,
                _ => false,
            };
            ed.canvas.drag = if mask.center().is_some_and(near) {
                let c = mask.center().unwrap_or_default();
                Some(Drag::Center { mask: true, offset: [c[0] - q[0], c[1] - q[1]] })
            } else if edge {
                Some(Drag::MaskRadius)
            } else if let Some((c, _)) = center.filter(|(c, _)| near(*c) || text_box.is_some_and(|b| q[0] >= b[0] && q[0] <= b[2] && q[1] >= b[1] && q[1] <= b[3])) {
                Some(Drag::Center { mask: false, offset: [c[0] - q[0], c[1] - q[1]] })
            } else if let Some((stack, lid, c)) = text_under(ed, q, size) {
                ed.selected = Selected::Layer(stack, lid);
                Some(Drag::Center { mask: false, offset: [c[0] - q[0], c[1] - q[1]] })
            } else {
                None
            };
        }
    }
    if let (Some(drag), Some(s)) = (ed.canvas.drag, pointer) {
        if resp.dragged() || resp.is_pointer_button_down_on() {
            let q = share(frame, s);
            if let Some(l) = find_mut(&mut ed.doc, ed.selected) {
                match drag {
                    Drag::Center { mask: true, offset } => l.mask.set_center([(q[0] + offset[0]).clamp(0.0, 1.0), (q[1] + offset[1]).clamp(0.0, 1.0)]),
                    Drag::Center { mask: false, offset } => {
                        let c = [(q[0] + offset[0]).clamp(0.0, 1.0), (q[1] + offset[1]).clamp(0.0, 1.0)];
                        match &mut l.effect {
                            Effect::Vignette(v) => v.center = c,
                            Effect::Aberration(a) => a.center = c,
                            Effect::Text(t) => t.center = c,
                            _ => {}
                        }
                    }
                    Drag::MaskRadius => {
                        if let Mask::Radial { center, radius, .. } = &mut l.mask {
                            *radius = (screen(frame, *center).distance(s) / frame.height()).clamp(0.02, 1.5);
                        }
                    }
                    Drag::Crop { .. } => {}
                }
            }
        }
    }
    if resp.drag_stopped() || !ui.ctx().input(|i| i.pointer.any_down()) {
        ed.canvas.drag = None;
    }
    if resp.hovered() {
        let over_text = pointer.or(resp.hover_pos()).is_some_and(|s| text_under(ed, share(frame, s), size).is_some());
        if ed.canvas.drag.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if over_text {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }
}

/// The topmost shown text under `q` (shares of the picture): its stack, id and centre.
fn text_under(ed: &Editor, q: [f32; 2], size: (u32, u32)) -> Option<(Option<usize>, u32, [f32; 2])> {
    let stacks = std::iter::once(None).chain((0..ed.doc.sources.len()).rev().map(Some));
    for stack in stacks {
        for l in ed.doc.stack(stack).iter().rev() {
            let Effect::Text(t) = &l.effect else { continue };
            if !l.visible {
                continue;
            }
            if let Some(b) = photo::text::bounds(t, size.0, size.1) {
                if q[0] >= b[0] && q[0] <= b[2] && q[1] >= b[1] && q[1] <= b[3] {
                    return Some((stack, l.id, t.center));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_handles_move_their_side_and_keep_the_shape_asked() {
        let start = Crop { x: 0.2, y: 0.2, w: 0.4, h: 0.4 };
        let c = dragged(Part::Right, start, [0.1, 0.0], None);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(near(c.w, 0.5) && near(c.x, 0.2) && near(c.h, 0.4), "{c:?}");
        let c = dragged(Part::TopLeft, start, [-0.1, -0.1], None);
        assert!(near(c.x, 0.1) && near(c.y, 0.1) && near(c.w, 0.5), "{c:?}");
        // Moved, it stays inside.
        let c = dragged(Part::Move, start, [0.9, -0.9], None);
        assert!(near(c.x, 0.6) && near(c.y, 0.0) && near(c.w, 0.4), "{c:?}");
        // Never smaller than a little.
        let c = dragged(Part::Left, start, [0.9, 0.0], None);
        assert!(c.w >= 0.039);
        // A shape kept: widening makes it taller, from the corner opposite.
        let c = dragged(Part::BottomRight, start, [0.2, 0.0], Some(1.0));
        assert!(near(c.w, c.h) && near(c.w, 0.6) && near(c.x, 0.2) && near(c.y, 0.2), "{c:?}");
        let c = dragged(Part::Top, start, [0.0, -0.1], Some(2.0));
        assert!((c.w - 2.0 * c.h).abs() < 1e-5, "{c:?}");
        assert!(c.x >= 0.0 && c.x + c.w <= 1.0 + 1e-6);
    }
}
