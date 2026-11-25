//! The editor's canvas: the preview as large as it fits, over a checkerboard where the picture is transparent (a
//! VRChat layer, a straightened picture's corners), and what the chosen layer moves on it. Crop and rotate draws the
//! canvas itself ([`super::crop`]). The vignette's and the aberration's centre, a mask's centre and its circle's edge
//! or band, texts (a press on one chooses it) and a depth blur's focus dot are dragged on the picture.

use orbiters_i18n::tr;
use super::{find_mut, Editor, Selected};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Ui};
use orbiters_ui::photo::render::{self, Stage};
use orbiters_ui::photo::{self, BlurKind, Effect, Focus, Mask};
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::{feedback, paint, viewer, widgets};

/// What a drag on the canvas moves.
#[derive(Clone, Copy, Debug)]
enum Drag {
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
    p.rect_filled(rect, theme::radius(theme::CARD_RADIUS), theme::ink_900());
    let Some(full) = ed.loaded.as_ref().map(|l| l.full) else {
        match &ed.error {
            Some(e) => {
                widgets::text_truncated(&p, rect.center(), Align2::CENTER_CENTER, &Txt::new(e, 13.0).color(theme::DANGER), rect.width() - 48.0);
            }
            None => {
                feedback::spinner(&p, rect.center() - vec2(0.0, 18.0), 12.0, 2.5, theme::accent());
                widgets::text(&p, rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, &Txt::new(if ed.source.remote.is_some() { tr("Downloading the picture at its full size…") } else { tr("Reading the picture…") }, 13.0).color(theme::fg_3()));
                ctx.request_repaint();
            }
        }
        return;
    };
    // The canvas never scrolls: a middle press is left to it (the crop zooms with the wheel, the 3D view moves with it).
    orbiters_ui::autoscroll::keep_middle(&ctx, rect);
    if ed.selected == Selected::Crop && !ed.comparing {
        // Crop and rotate draws the picture under a view of its own, with its tools and dials.
        super::crop::show(ed, ui, rect, full);
    } else {
        picture(ed, ui, rect, full);
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

/// The picture as large as it fits (over a checkerboard where it is transparent), and what the chosen layer moves on
/// it.
fn picture(ed: &mut Editor, ui: &mut Ui, rect: Rect, full: (u32, u32)) {
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    let stage = ed.stage();
    let (fw, fh) = render::full_size(&ed.doc, full, stage);
    let frame = viewer::contain(rect.shrink(28.0), fw as f32 / fh.max(1) as f32);
    let transparent = !ed.doc.sources.is_empty() || stage == Stage::Uncropped;
    if transparent {
        let id = checker(&ctx, &mut ed.canvas);
        let cells = frame.size() / 12.0;
        p.image(id, frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(cells.x / 2.0, cells.y / 2.0)), Color32::WHITE);
    }
    if let Some(live) = ed.live.as_mut() {
        // Drawn by its host (Edit in 3D) in the place left here, under the handles.
        live.slot = Some(super::Slot { layer: p.layer_id(), clip: p.clip_rect(), shape: p.add(egui::Shape::Noop), frame, stage });
    } else {
        match &ed.texture {
            Some(t) => {
                p.image(t.id(), frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => feedback::shimmer(&p, frame, 4.0),
        }
    }
    // Making the preview of the latest change: a quiet spinner in the corner.
    if ed.preview.as_ref().is_some_and(|pv| pv.busy()) {
        feedback::spinner(&p, pos2(rect.right() - 20.0, rect.top() + 20.0), 7.0, 2.0, theme::fg_3());
        ctx.request_repaint();
    }
    if ed.comparing {
        let t = Txt::new(tr("Original"), 11.5).color(theme::fg());
        let w = widgets::text_size(ui, &t).x + 24.0;
        let r = Rect::from_min_size(pos2(frame.left() + 12.0, frame.top() + 12.0), vec2(w, 26.0));
        p.rect_filled(r, theme::radius(13.0), theme::black(0.6));
        widgets::text(&p, r.center(), Align2::CENTER_CENTER, &t);
    } else {
        handles(ed, ui, frame, (fw, fh));
    }
}

/// A live picture's host takes the pointer where no handle did (the 3D view's camera moves with it).
fn hand_over(ed: &mut Editor, resp: &egui::Response) {
    if ed.canvas.drag.is_none() {
        if let Some(live) = ed.live.as_mut() {
            live.free = Some(resp.clone());
        }
    }
}

/// A share of `frame` on screen and back.
fn screen(frame: Rect, q: [f32; 2]) -> Pos2 {
    pos2(frame.left() + q[0] * frame.width(), frame.top() + q[1] * frame.height())
}

fn share(frame: Rect, s: Pos2) -> [f32; 2] {
    [(s.x - frame.left()) / frame.width(), (s.y - frame.top()) / frame.height()]
}

/// A round handle; brighter while held.
fn knob(p: &egui::Painter, at: Pos2, held: bool, square: bool) {
    let r = if held { 9.0 } else { 7.5 };
    p.circle_filled(at, r + 3.0, theme::black(0.35));
    if square {
        let q = Rect::from_center_size(at, vec2(r * 1.8, r * 1.8));
        p.rect_filled(q, theme::radius(3.0), Color32::WHITE);
        p.rect_filled(q.shrink(3.0), theme::radius(2.0), theme::accent());
    } else {
        p.circle_filled(at, r, Color32::WHITE);
        p.circle_filled(at, r - 3.0, theme::accent());
    }
}

/// The auto focus's dot: a camera's focus point (a ring with four ticks around a dot), and the distance it focuses at.
fn focus_reticle(p: &egui::Painter, at: Pos2, held: bool, distance: Option<f32>) {
    let r = if held { 19.0 } else { 16.0 };
    let shadow = Stroke::new(4.0, theme::black(0.35));
    let line = Stroke::new(1.8, if held { theme::accent() } else { Color32::WHITE });
    p.circle_stroke(at, r, shadow);
    p.circle_stroke(at, r, line);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let d = vec2(dx, dy);
        p.line_segment([at + d * (r - 5.0), at + d * (r + 5.0)], shadow);
        p.line_segment([at + d * (r - 5.0), at + d * (r + 5.0)], line);
    }
    p.circle_filled(at, 4.5, theme::black(0.35));
    p.circle_filled(at, 3.0, theme::accent());
    if let Some(d) = distance {
        let t = Txt::new(super::controls::distance_text(d), 11.5).color(theme::fg());
        let w = p.layout_job(t.job(f32::INFINITY)).size().x + 16.0;
        let pill = Rect::from_min_size(pos2(at.x + r + 8.0, at.y - 11.0), vec2(w, 22.0));
        p.rect_filled(pill, theme::radius(11.0), theme::black(0.6));
        widgets::text(p, pill.center(), Align2::CENTER_CENTER, &t);
    }
}

/// What the chosen layer moves on the canvas, and texts chosen by a press on them.
fn handles(ed: &mut Editor, ui: &mut Ui, frame: Rect, size: (u32, u32)) {
    let id = ed.id.with("canvas-handles");
    let resp = ui.interact(frame, id, Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(frame.intersect(ui.clip_rect()));
    let held = ed.canvas.drag;
    let focus_dot = ed.layer().is_some_and(|l| matches!(&l.effect, Effect::Blur(b) if b.kind == BlurKind::Depth && b.focus == Focus::Auto));
    let (center, mask, text_box) = match ed.layer() {
        Some(l) => {
            let center = match &l.effect {
                Effect::Vignette(v) => Some((v.center, Some(v.size / 1.8f32.sqrt()))),
                Effect::Aberration(a) => Some((a.center, None)),
                Effect::Text(t) => Some((t.center, None)),
                Effect::Blur(b) if b.kind == BlurKind::Depth && b.focus == Focus::Auto => Some((b.point, None)),
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
        p.rect_stroke(r, theme::radius(6.0), Stroke::new(1.5, theme::alpha(theme::accent(), 0.9)), egui::StrokeKind::Middle);
    } else if let Some((c, ring)) = center {
        let at = screen(frame, c);
        if let Some(rr) = ring {
            paint::dashed_circle(&p, at, rr * frame.height(), [6.0, 5.0], 1.5, theme::white(0.7), 0.0);
        }
        if focus_dot {
            focus_reticle(&p, at, matches!(held, Some(Drag::Center { mask: false, .. })), ed.live.as_ref().and_then(|l| l.focus));
        } else {
            knob(&p, at, matches!(held, Some(Drag::Center { mask: false, .. })), false);
        }
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
                            Effect::Blur(b) => b.point = c,
                            _ => {}
                        }
                    }
                    Drag::MaskRadius => {
                        if let Mask::Radial { center, radius, .. } = &mut l.mask {
                            *radius = (screen(frame, *center).distance(s) / frame.height()).clamp(0.02, 1.5);
                        }
                    }
                }
            }
        }
    }
    hand_over(ed, &resp);
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
