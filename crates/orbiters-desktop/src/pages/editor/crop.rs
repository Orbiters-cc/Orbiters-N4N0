//! Crop and rotate, as the iPhone's Photos crops. Over the canvas: Cancel and Reset on the left, Done on the right (only
//! once something changed), and between them flip, rotate a quarter to the left, AUTO (the picture levelled by its own
//! lines, [`orbiters_ui::photo::level`]; lit while that is what it shows), the shapes and "…" (rotate right, flip
//! upside down, copy and paste a crop). The picture under a white frame with thick corners and edge marks, the rest
//! dimmed; thirds while it is dragged, a finer grid while it is straightened. The frame's corners and edges resize it
//! (the view holds still, then springs to the new crop centred and as large as fits), a drag inside moves the picture
//! under it, the wheel (or a pinch) zooms it about the pointer. Under the picture three dials, Straighten, Vertical and
//! Horizontal (the perspective keystones): the chosen one's value in a ring that fills with it, and a ruler scrubbed
//! left and right that coasts when let go and holds at zero. The shapes replace the dials: portrait or landscape, then
//! Original, Freeform, Square, 9:16, 4:5, 5:7, 3:4, 3:5 and 2:3 (or their landscape forms).
//!
//! The parts are functions of the frame's place on screen and the geometry ([`target`], [`View`], [`overlay_input`],
//! [`draw_overlay`]) so another picture (a 3D view) can be cropped by the same frame.

use super::Editor;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use orbiters_i18n::{mark, tr};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::photo::geometry::{self, Warp};
use orbiters_ui::photo::level::{self, Level};
use orbiters_ui::photo::render::Stage;
use orbiters_ui::photo::{Aspect, Crop, Geometry};
use orbiters_ui::popup::{self, Item};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Chip, IconButton, Size, Variant};
use orbiters_ui::{feedback, icons};
use std::sync::Mutex;

const MAX: f32 = Geometry::MAX_ANGLE;
/// The ruler's spacing: points per degree.
const SPACING: f32 = 8.0;
/// How far a handle reaches past the frame's line for a press.
const GRAB: f32 = 16.0;
/// The smallest crop, in shares of the frame.
const MIN: f32 = 0.04;
const TOOLS_H: f32 = 52.0;
const DIALS_H: f32 = 132.0;

/// A crop copied with "…", for another picture.
static COPIED: Mutex<Option<Geometry>> = Mutex::new(None);

/// The three dials under the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dial {
    Straighten,
    Vertical,
    Horizontal,
}

impl Dial {
    pub(crate) const ALL: [Dial; 3] = [Dial::Straighten, Dial::Vertical, Dial::Horizontal];

    /// The dial's name in English, a key: [`tr`] it where it is drawn.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Dial::Straighten => mark("Straighten"),
            Dial::Vertical => mark("Vertical"),
            Dial::Horizontal => mark("Horizontal"),
        }
    }

    pub(crate) fn about(self) -> &'static str {
        match self {
            Dial::Straighten => mark("Straighten: turn the picture a little to level it"),
            Dial::Vertical => mark("Vertical perspective: lines leaning together upwards, as a building shot from below, made upright"),
            Dial::Horizontal => mark("Horizontal perspective: a wall seen at an angle turned to face you"),
        }
    }

    pub(crate) fn value(self, g: &Geometry) -> f32 {
        match self {
            Dial::Straighten => g.straighten,
            Dial::Vertical => g.vertical,
            Dial::Horizontal => g.horizontal,
        }
    }

    pub(crate) fn value_mut(self, g: &mut Geometry) -> &mut f32 {
        match self {
            Dial::Straighten => &mut g.straighten,
            Dial::Vertical => &mut g.vertical,
            Dial::Horizontal => &mut g.horizontal,
        }
    }
}

/// Which part of the crop is dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Part {
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

/// A drag on the picture, with the frame's place on screen when it started (a handle's drag holds the view there).
#[derive(Clone, Copy, Debug)]
struct Drag {
    part: Part,
    start: Crop,
    grab: Pos2,
    frame: Rect,
}

/// How the view follows the crop this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Follow {
    /// Still (a handle is dragged: the frame stays under the pointer).
    Hold,
    /// At once (the picture panned or zoomed under the crop, or straightened: the crop stays where it is on screen).
    Snap,
    /// On a spring (a handle let go, a turn, a shape: the crop glides to the middle).
    Spring,
}

/// The frame's place on screen, on a spring of its own (it is held and snapped as well as animated).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct View {
    at: Option<[f32; 4]>,
    velocity: [f32; 4],
    time: f64,
}

impl View {
    /// The frame's place this frame, following `target` as `how` says.
    pub(crate) fn follow(&mut self, ctx: &egui::Context, target: Rect, how: Follow) -> Rect {
        let now = anim::time(ctx);
        let goal = [target.min.x, target.min.y, target.width(), target.height()];
        let mut at = match self.at {
            Some(a) if how != Follow::Snap && anim::is_live(ctx) => a,
            _ => {
                self.velocity = [0.0; 4];
                goal
            }
        };
        if how == Follow::Hold {
            self.velocity = [0.0; 4];
        } else if at != goal {
            // Soft and barely bouncing, as the iPhone's crop settles.
            let (k, c) = (210.0f32, 27.0f32);
            let mut dt = (now - self.time).clamp(0.0, 0.05) as f32;
            while dt > 0.0 {
                let h = dt.min(1.0 / 240.0);
                dt -= h;
                for i in 0..4 {
                    let a = -k * (at[i] - goal[i]) - c * self.velocity[i];
                    self.velocity[i] += a * h;
                    at[i] += self.velocity[i] * h;
                }
            }
            let still = (0..4).all(|i| (at[i] - goal[i]).abs() < 0.05 && self.velocity[i].abs() < 0.5);
            if still {
                at = goal;
                self.velocity = [0.0; 4];
            } else {
                ctx.request_repaint();
            }
        }
        self.time = now;
        self.at = Some(at);
        Rect::from_min_size(pos2(at[0], at[1]), vec2(at[2], at[3]))
    }

    pub(crate) fn rect(&self) -> Option<Rect> {
        self.at.map(|a| Rect::from_min_size(pos2(a[0], a[1]), vec2(a[2], a[3])))
    }
}

/// The ruler's scrub: how fast it coasts after a release, the notch at zero.
#[derive(Clone, Copy, Debug, Default)]
struct Ruler {
    /// Degrees a second while it coasts.
    velocity: f32,
    coasting: bool,
    /// Degrees of travel still held at zero after reaching it.
    notch: f32,
    /// When it last reached zero (the mark's tick).
    ticked: f64,
    time: f64,
    held: bool,
}

pub(crate) struct State {
    /// The geometry when the crop was opened: Cancel puts it back, Done shows once it differs.
    entered: Option<Geometry>,
    pub(crate) dial: Dial,
    /// The shapes are shown under the picture instead of the dials.
    shapes: bool,
    /// The shapes' orientation, as last chosen (None: the crop's own, else the picture's).
    landscape: Option<bool>,
    drag: Option<Drag>,
    pub(crate) view: View,
    ruler: Ruler,
    /// What a dial was before a press on it set it to zero (a second press puts it back).
    zeroed: [Option<f32>; 3],
    /// The picture's lines' tilt, looked for once (AUTO).
    level: Option<Option<Level>>,
    /// When the picture was last moved, zoomed or straightened (the grids fade after).
    touched: f64,
    /// What moved it last (which grid shows).
    by_ruler: bool,
    /// The picture's pointer where the frame took nothing (a live picture's host moves its camera with it).
    pub(crate) free: Option<egui::Response>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            entered: None,
            dial: Dial::Straighten,
            shapes: false,
            landscape: None,
            drag: None,
            view: View::default(),
            ruler: Ruler::default(),
            zeroed: [None; 3],
            level: None,
            touched: f64::NEG_INFINITY,
            by_ruler: false,
            free: None,
        }
    }
}

impl State {
    /// The crop is left (Done, Cancel, another row): the next opening starts afresh.
    pub(super) fn leave(&mut self) {
        self.entered = None;
        self.drag = None;
        self.shapes = false;
        self.ruler = Ruler::default();
        self.view = View::default();
        self.zeroed = [None; 3];
    }

    /// Something is still being changed (dragged, or the ruler coasting): its changes are one step of undo.
    pub(super) fn moving(&self) -> bool {
        self.drag.is_some() || self.ruler.coasting || self.ruler.held
    }

    /// The whole frame's size on screen, in points, while the crop shows (the preview is made at about it).
    pub(super) fn shown_size(&self) -> Option<Vec2> {
        self.entered.as_ref().and(self.view.rect()).map(|r| r.size())
    }
}

/// The frame's place on screen (a `tw`×`th` frame) with its crop `c` as large as fits in `area`, in its middle.
pub(crate) fn target(area: Rect, c: Crop, tw: f32, th: f32) -> Rect {
    let (cw, ch) = ((c.w * tw).max(1e-3), (c.h * th).max(1e-3));
    let s = (area.width() / cw).min(area.height() / ch);
    let size = vec2(tw * s, th * s);
    let center = c.center();
    Rect::from_min_size(area.center() - vec2(center[0] * size.x, center[1] * size.y), size)
}

/// A share of `frame` on screen and back.
fn screen(frame: Rect, q: [f32; 2]) -> Pos2 {
    pos2(frame.left() + q[0] * frame.width(), frame.top() + q[1] * frame.height())
}

fn share(frame: Rect, s: Pos2) -> [f32; 2] {
    [(s.x - frame.left()) / frame.width().max(1e-3), (s.y - frame.top()) / frame.height().max(1e-3)]
}

fn crop_rect(frame: Rect, c: Crop) -> Rect {
    Rect::from_min_max(screen(frame, [c.x, c.y]), screen(frame, [c.x + c.w, c.y + c.h]))
}

/// The crop as it lies in the turned picture: the shape asked, as width over height in shares of the frame.
fn ratio_in_shares(g: &Geometry, tw: f32, th: f32) -> Option<f32> {
    g.aspect.ratio(tw / th.max(1.0)).map(|r| r * th / tw)
}

/// The largest step `t` of 0 to 1 for which `at(t)` lies inside the picture (`at(0)` does).
fn furthest(w: &Warp, at: impl Fn(f32) -> Crop) -> f32 {
    if geometry::crop_inside(at(1.0), w) {
        return 1.0;
    }
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..20 {
        let t = (lo + hi) * 0.5;
        if geometry::crop_inside(at(t), w) {
            lo = t;
        } else {
            hi = t;
        }
    }
    lo
}

/// The crop `start` with `part` moved by `d` (shares), keeping `ratio` (width over height in shares) when given.
pub(crate) fn dragged(part: Part, start: Crop, d: [f32; 2], ratio: Option<f32>) -> Crop {
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

/// `part` of the crop `start` dragged by `d` (shares), as far as the picture goes: across first, then down.
pub(crate) fn dragged_inside(w: &Warp, part: Part, start: Crop, d: [f32; 2], ratio: Option<f32>) -> Crop {
    if part == Part::Move {
        // The picture moves under the crop: the crop goes the other way, until an edge of the picture reaches it.
        let tx = furthest(w, |t| dragged(Part::Move, start, [d[0] * t, 0.0], None));
        let ty = furthest(w, |t| dragged(Part::Move, start, [d[0] * tx, d[1] * t], None));
        return dragged(Part::Move, start, [d[0] * tx, d[1] * ty], None);
    }
    let tx = furthest(w, |t| dragged(part, start, [d[0] * t, 0.0], ratio));
    let ty = furthest(w, |t| dragged(part, start, [d[0] * tx, d[1] * t], ratio));
    dragged(part, start, [d[0] * tx, d[1] * ty], ratio)
}

/// The crop `c` zoomed by `k` about `q` (shares; above 1 the picture grows, the crop shrinks), kept its shape, inside the
/// frame and the picture.
pub(crate) fn zoomed(w: &Warp, c: Crop, q: [f32; 2], k: f32) -> Crop {
    let at = |k: f32| {
        // Never wider or taller than the frame: the shape is kept.
        let k = k.max(c.w).max(c.h);
        let k = k.min(c.w / MIN).min(c.h / MIN);
        let (nw, nh) = (c.w / k, c.h / k);
        geometry::clamp_to_frame(Crop { x: q[0] - (q[0] - c.x) / k, y: q[1] - (q[1] - c.y) / k, w: nw, h: nh })
    };
    if k >= 1.0 {
        return at(k);
    }
    let t = furthest(w, |t| at(1.0 + (k - 1.0) * t));
    at(1.0 + (k - 1.0) * t)
}

/// Turns the picture a quarter to the left (`left`) or right, the crop, the shape and the perspective with it.
pub(crate) fn turn(g: &mut Geometry, left: bool) {
    g.turns = (g.turns + if left { 3 } else { 1 }) % 4;
    let c = g.crop;
    g.crop = if left { Crop { x: c.y, y: 1.0 - c.x - c.w, w: c.h, h: c.w } } else { Crop { x: 1.0 - c.y - c.h, y: c.x, w: c.h, h: c.w } };
    g.aspect = g.aspect.turned();
    // Flips and tilts are along the turned picture's sides: what was across is now up and down.
    std::mem::swap(&mut g.flip_h, &mut g.flip_v);
    let (v, h) = (g.vertical, g.horizontal);
    (g.vertical, g.horizontal) = if left { (h, -v) } else { (-h, v) };
}

/// Mirrors the picture across (`across`) or upside down, as it shows: the crop, the straightening and the tilt mirrored.
pub(crate) fn flip(g: &mut Geometry, across: bool) {
    let c = g.crop;
    if across {
        g.flip_h = !g.flip_h;
        g.crop.x = 1.0 - c.x - c.w;
        g.horizontal = -g.horizontal;
    } else {
        g.flip_v = !g.flip_v;
        g.crop.y = 1.0 - c.y - c.h;
        g.vertical = -g.vertical;
    }
    g.straighten = -g.straighten;
}

/// Chooses the shape `a`: the largest crop of it about the middle (fitted inside the picture when drawn).
pub(crate) fn set_aspect(g: &mut Geometry, a: Aspect, tw: f32, th: f32) {
    g.aspect = a;
    if let Some(ratio) = a.ratio(tw / th.max(1.0)) {
        g.crop = geometry::with_ratio(Crop::FULL, ratio, tw, th);
    }
}

/// The straightening AUTO sets for `found` (a flipped picture leans the other way), within the dial's reach.
fn auto_straighten(g: &Geometry, found: &Level) -> f32 {
    let s = if g.flip_h != g.flip_v { -found.straighten() } else { found.straighten() };
    s.clamp(-MAX, MAX)
}

/// Takes the presses on the crop's frame, drawn last at `frame` (the whole turned picture on screen, `size` pixels):
/// its corners and edges resize the crop, a drag inside moves the picture under it, the wheel (or a pinch) zooms about
/// the pointer, inside `clip`. Returns how the view should follow the crop this frame ([`View::follow`]); then the
/// picture is drawn at the view's frame, and [`draw_overlay`] over it.
pub(crate) fn overlay_input(ui: &mut Ui, st: &mut State, id: Id, frame: Rect, clip: Rect, g: &mut Geometry, size: (f32, f32)) -> Follow {
    let ctx = ui.ctx().clone();
    let now = anim::time(&ctx);
    let (tw, th) = size;
    let warp = Warp::of(g, tw, th);
    let c = geometry::fit_warp(g.crop, &warp);
    let cr = crop_rect(frame, c);
    let resp = ui.interact(clip, id.with("crop-picture"), Sense::click_and_drag());
    let part_at = |s: Pos2| -> Option<Part> {
        let near = |a: f32, b: f32| (a - b).abs() < GRAB;
        let (l, r, t, b) = (near(s.x, cr.left()), near(s.x, cr.right()), near(s.y, cr.top()), near(s.y, cr.bottom()));
        let in_y = s.y > cr.top() - GRAB && s.y < cr.bottom() + GRAB;
        let in_x = s.x > cr.left() - GRAB && s.x < cr.right() + GRAB;
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
        let cursor = match st.drag.map(|d| Some(d.part)).unwrap_or_else(|| part_at(h)) {
            Some(Part::Move) => {
                if st.drag.is_some() {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::Grab
                }
            }
            Some(Part::Left | Part::Right) => CursorIcon::ResizeHorizontal,
            Some(Part::Top | Part::Bottom) => CursorIcon::ResizeVertical,
            Some(Part::TopLeft | Part::BottomRight) => CursorIcon::ResizeNwSe,
            Some(Part::TopRight | Part::BottomLeft) => CursorIcon::ResizeNeSw,
            None => CursorIcon::Default,
        };
        ctx.set_cursor_icon(cursor);
    }
    let mut follow = Follow::Spring;
    if resp.drag_started() {
        if let Some(s) = resp.interact_pointer_pos() {
            st.drag = part_at(s).map(|part| Drag { part, start: c, grab: s, frame });
        }
    }
    if let (Some(d), Some(s)) = (st.drag, resp.interact_pointer_pos()) {
        if resp.dragged() {
            let delta = [(s.x - d.grab.x) / d.frame.width().max(1.0), (s.y - d.grab.y) / d.frame.height().max(1.0)];
            if d.part == Part::Move {
                // The picture follows the pointer; the crop goes the other way and stays put on screen.
                g.crop = dragged_inside(&warp, Part::Move, d.start, [-delta[0], -delta[1]], None);
            } else {
                g.crop = dragged_inside(&warp, d.part, d.start, delta, ratio_in_shares(g, tw, th));
            }
            st.touched = now;
            st.by_ruler = false;
        }
        follow = if d.part == Part::Move { Follow::Snap } else { Follow::Hold };
    }
    if resp.drag_stopped() || (st.drag.is_some() && !ctx.input(|i| i.pointer.any_down())) {
        st.drag = None;
    }
    st.free = (st.drag.is_none() && follow == Follow::Spring).then(|| resp.clone());
    // The wheel or a pinch zooms the picture about the pointer.
    if resp.hovered() && st.drag.is_none() {
        let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let k = pinch * (scroll * 0.0025).exp();
        if (k - 1.0).abs() > 1e-4 {
            let q = resp.hover_pos().map(|p| share(frame, p)).unwrap_or(c.center());
            let q = [q[0].clamp(c.x, c.x + c.w), q[1].clamp(c.y, c.y + c.h)];
            g.crop = zoomed(&warp, c, q, k);
            st.touched = now;
            st.by_ruler = false;
            follow = Follow::Snap;
        }
    }

    follow
}

/// Draws the crop's frame over the picture drawn at `frame`: the outside dimmed (inside `clip`; darker at rest, as the
/// iPhone's), thirds while it is dragged and a finer grid while it is straightened, the frame, thick corners and marks
/// at the middle of its sides.
pub(crate) fn draw_overlay(ui: &Ui, st: &State, id: Id, frame: Rect, clip: Rect, g: &Geometry, size: (f32, f32)) {
    let ctx = ui.ctx().clone();
    let now = anim::time(&ctx);
    let c = geometry::fit_warp(g.crop, &Warp::of(g, size.0, size.1));
    let cr = crop_rect(frame, c);
    let p = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
    let busy = st.drag.is_some() || st.ruler.held || st.ruler.coasting || now - st.touched < 0.6;
    let calm = anim::toggle(&ctx, id.with("crop-calm"), !busy, Spring::SOFT);
    let dim = theme::black(0.5 + 0.28 * calm);
    let outer = clip.expand(2.0);
    p.rect_filled(Rect::from_min_max(outer.min, pos2(outer.right(), cr.top())), 0.0, dim);
    p.rect_filled(Rect::from_min_max(pos2(outer.left(), cr.bottom()), outer.max), 0.0, dim);
    p.rect_filled(Rect::from_min_max(pos2(outer.left(), cr.top()), pos2(cr.left(), cr.bottom())), 0.0, dim);
    p.rect_filled(Rect::from_min_max(pos2(cr.right(), cr.top()), pos2(outer.right(), cr.bottom())), 0.0, dim);
    let grid = 1.0 - calm;
    let fine = anim::toggle(&ctx, id.with("crop-fine"), st.by_ruler, Spring::SOFT) * grid;
    if grid > 0.01 {
        let n = 9;
        for k in 1..n {
            let t = k as f32 / n as f32;
            let a = if k % 3 == 0 { 0.5 * grid } else { 0.22 * fine };
            if a > 0.005 {
                p.vline(cr.left() + cr.width() * t, cr.y_range(), Stroke::new(1.0, theme::white(a)));
                p.hline(cr.x_range(), cr.top() + cr.height() * t, Stroke::new(1.0, theme::white(a)));
            }
        }
    }
    p.rect_stroke(cr, 0.0, Stroke::new(1.2, theme::white(0.92)), egui::StrokeKind::Outside);
    // Thick corners just outside the frame, short marks at the middle of its sides.
    let arm = 22.0f32.min(cr.width() * 0.45).min(cr.height() * 0.45);
    let wide = 3.5;
    let o = wide * 0.5 + 1.0;
    for (corner, dx, dy) in [(cr.left_top(), 1.0, 1.0), (cr.right_top(), -1.0, 1.0), (cr.left_bottom(), 1.0, -1.0), (cr.right_bottom(), -1.0, -1.0)] {
        let at = corner - vec2(o * dx, o * dy);
        let s = Stroke::new(wide, Color32::WHITE);
        p.add(Shape::line(vec![at + vec2(arm * dx, 0.0), at, at + vec2(0.0, arm * dy)], s));
    }
    let mark = (arm * 0.8).min(cr.width() * 0.2).min(cr.height() * 0.2);
    for (m, across, out) in [
        (pos2(cr.center().x, cr.top()), true, vec2(0.0, -o)),
        (pos2(cr.center().x, cr.bottom()), true, vec2(0.0, o)),
        (pos2(cr.left(), cr.center().y), false, vec2(-o, 0.0)),
        (pos2(cr.right(), cr.center().y), false, vec2(o, 0.0)),
    ] {
        let d = if across { vec2(mark * 0.5, 0.0) } else { vec2(0.0, mark * 0.5) };
        p.line_segment([m + out - d, m + out + d], Stroke::new(wide, Color32::WHITE));
    }
}

/// The crop over the editor's canvas `rect`: the tools on top, the picture, the dials (or the shapes) under it.
pub(super) fn show(ed: &mut Editor, ui: &mut Ui, rect: Rect, full: (u32, u32)) {
    let ctx = ui.ctx().clone();
    let id = ed.id.with("crop");
    if ed.crop.entered.is_none() {
        ed.crop.entered = Some(ed.doc.geometry.clone());
    }
    if ed.crop.level.is_none() {
        ed.crop.level = Some(ed.loaded.as_ref().and_then(|l| l.inputs.first()).and_then(level::detect));
    }
    let (tw, th) = geometry::turned_size(full.0, full.1, ed.doc.geometry.turns);
    let (tw, th) = (tw as f32, th as f32);
    let tools = Rect::from_min_size(rect.min + vec2(14.0, 10.0), vec2(rect.width() - 28.0, TOOLS_H - 12.0));
    let bottom = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - DIALS_H), pos2(rect.right() - 14.0, rect.bottom() - 10.0));
    let area = Rect::from_min_max(pos2(rect.left() + 40.0, rect.top() + TOOLS_H + 22.0), pos2(rect.right() - 40.0, bottom.top() - 18.0));
    let clip = Rect::from_min_max(pos2(rect.left(), tools.bottom() + 4.0), pos2(rect.right(), bottom.top() - 2.0));

    // The frame's presses first (on the frame as last drawn), then the view follows the crop, the picture is drawn
    // there and the frame over it.
    let held = ed.crop.view.rect().unwrap_or_else(|| target(area, ed.doc.geometry.effective_crop(full.0, full.1), tw, th));
    let follow = overlay_input(ui, &mut ed.crop, id, held, clip, &mut ed.doc.geometry, (tw, th));
    if let (Some(live), Some(free)) = (ed.live.as_mut(), ed.crop.free.take()) {
        live.free = Some(free);
    }
    let c = ed.doc.geometry.effective_crop(full.0, full.1);
    let follow = if ed.crop.ruler.held || ed.crop.ruler.coasting { Follow::Snap } else { follow };
    let frame = ed.crop.view.follow(&ctx, target(area, c, tw, th), follow);
    paint_picture(ed, ui, frame, clip, c);
    draw_overlay(ui, &ed.crop, id, frame, clip, &ed.doc.geometry, (tw, th));

    // The tools on top, the dials or the shapes under the picture.
    tools_row(ed, ui, id, tools, (tw, th));
    if ed.crop.shapes {
        shapes(ed, ui, id, bottom, (tw, th));
    } else {
        dials(ed, ui, id, bottom);
    }
}

/// The straightened (and tilted) picture whole at `frame`, inside `clip`; until its preview is made, the cropped one
/// inside the crop.
fn paint_picture(ed: &mut Editor, ui: &mut Ui, frame: Rect, clip: Rect, c: Crop) {
    let p = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
    if let Some(live) = ed.live.as_mut() {
        // Drawn by its host (Edit in 3D) in the place left here, under the frame.
        live.slot = Some(super::Slot { layer: p.layer_id(), clip: p.clip_rect(), shape: p.add(Shape::Noop), frame, stage: Stage::Uncropped });
        return;
    }
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let shown = ed.shown.as_ref().map(|a| a.stage);
    match (&ed.texture, shown) {
        (Some(t), Some(Stage::Uncropped)) => {
            p.image(t.id(), frame, uv, Color32::WHITE);
        }
        (Some(t), _) => {
            p.image(t.id(), crop_rect(frame, c), uv, Color32::WHITE);
        }
        (None, _) => feedback::shimmer(&p, crop_rect(frame, c), 4.0),
    }
    if ed.preview.as_ref().is_some_and(|pv| pv.busy()) {
        feedback::spinner(&p, pos2(clip.right() - 22.0, clip.top() + 18.0), 7.0, 2.0, theme::fg_3());
        ui.ctx().request_repaint();
    }
}

/// A round tool button with `draw` in it (an icon); `on` lights it.
fn tool(ui: &mut Ui, id: Id, r: Rect, label: &str, on: bool, enabled: bool, draw: impl Fn(&Painter, Pos2, Color32)) -> bool {
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    widgets::note_control(ui.ctx(), label, r, enabled);
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered() && enabled, Spring::SNAPPY);
    let press = anim::toggle(ui.ctx(), id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let p = ui.painter();
    let c = r.center();
    let rad = r.width() * 0.5 * (1.0 - 0.08 * press);
    p.circle_filled(c, rad, if on { theme::alpha(theme::accent(), 0.16 + 0.06 * hover) } else { theme::white(0.04 + 0.06 * hover) });
    let color = if !enabled {
        theme::fg_4()
    } else if on {
        theme::accent()
    } else {
        theme::mix(theme::fg_2(), theme::fg(), hover)
    };
    draw(p, c, color);
    // `label` is the English key: control notes find a control by it, the tip says it in the language.
    let resp = resp.on_hover_text(tr(label));
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    enabled && widgets::activated(&resp)
}

/// The shapes' icon: a frame with a smaller one in its corner.
fn aspect_icon(p: &Painter, c: Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    p.rect_stroke(Rect::from_center_size(c, vec2(18.0, 14.0)), 2.5, s, egui::StrokeKind::Middle);
    p.rect_stroke(Rect::from_min_size(c + vec2(-9.0, -1.0), vec2(9.0, 8.0)), 1.5, s, egui::StrokeKind::Middle);
}

/// A dial's icon: a level line across a tilted one, or a frame leaning together upwards or to the left.
fn dial_icon(p: &Painter, c: Pos2, dial: Dial, color: Color32) {
    let s = Stroke::new(1.6, color);
    match dial {
        Dial::Straighten => {
            // A level line, and the picture's line turned across it.
            let (sn, cs) = (-20f32).to_radians().sin_cos();
            p.line_segment([c + vec2(-8.5, 0.0), c + vec2(8.5, 0.0)], Stroke::new(1.2, theme::alpha(color, 0.55)));
            p.line_segment([c + vec2(-8.0 * cs, -8.0 * sn), c + vec2(8.0 * cs, 8.0 * sn)], s);
        }
        Dial::Vertical => {
            p.add(Shape::closed_line(vec![c + vec2(-4.5, -7.5), c + vec2(4.5, -7.5), c + vec2(7.5, 7.5), c + vec2(-7.5, 7.5)], s));
            p.line_segment([c + vec2(0.0, -4.0), c + vec2(0.0, 4.0)], Stroke::new(1.2, theme::alpha(color, 0.7)));
        }
        Dial::Horizontal => {
            p.add(Shape::closed_line(vec![c + vec2(-7.5, -4.5), c + vec2(7.5, -7.5), c + vec2(7.5, 7.5), c + vec2(-7.5, 4.5)], s));
            p.line_segment([c + vec2(-4.0, 0.0), c + vec2(4.0, 0.0)], Stroke::new(1.2, theme::alpha(color, 0.7)));
        }
    }
}

/// The orientation icons: a standing or a lying frame.
fn orientation_icon(p: &Painter, c: Pos2, landscape: bool, color: Color32) {
    let size = if landscape { vec2(18.0, 12.0) } else { vec2(12.0, 18.0) };
    p.rect_stroke(Rect::from_center_size(c, size), 2.5, Stroke::new(1.6, color), egui::StrokeKind::Middle);
}

/// Cancel and Reset, the tools, Done.
fn tools_row(ed: &mut Editor, ui: &mut Ui, id: Id, r: Rect, size: (f32, f32)) {
    let (tw, th) = size;
    let mid = r.center().y;
    let entered = ed.crop.entered.clone().unwrap_or_default();
    let changed = ed.doc.geometry != entered;
    // Cancel, and Reset while the crop is not the picture as it was taken.
    let b = Button::new(tr("Cancel")).size(Size::Sm).variant(Variant::Ghost).id_salt(id.with("cancel"));
    let cw = b.desired_width(ui);
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(r.left(), mid - 16.0), vec2(cw, 32.0)))) {
        ed.doc.geometry = entered.clone();
        ed.selected = super::Selected::Nothing;
        return;
    }
    let g = &mut ed.doc.geometry;
    let resettable = !g.is_identity() || g.aspect != Aspect::Free;
    let shown = anim::toggle(ui.ctx(), id.with("reset-shown"), resettable, Spring::SNAPPY);
    let reset = Button::new(tr("Reset")).icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).variant(Variant::Ghost).enabled(resettable).id_salt(id.with("reset"));
    let done = Button::new(tr("Done")).icon(icons::CHECK).size(Size::Sm).variant(Variant::Primary).enabled(changed).id_salt(id.with("done"));
    let dw = done.desired_width(ui);
    // The row laid out in one pass (Night PG2-10: Reset was drawn over Flip and Rotate on narrow canvases): the tools
    // centred as before when they clear Cancel and Reset; else just after them; else with tighter gaps; else Reset as
    // its icon alone (the right panel has a Reset of its own too).
    let auto_w = 62.0;
    let tools = |gap: f32| 4.0 * 36.0 + auto_w + 4.0 * gap;
    let after_left = |rw: f32| r.left() + cw + if shown > 0.01 { 6.0 + rw } else { 0.0 } + 10.0;
    let fits = |rw: f32, gap: f32| after_left(rw).max(r.center().x - tools(gap) * 0.5) + tools(gap) <= r.right() - dw - 10.0;
    let full_rw = reset.desired_width(ui);
    let (rw, gap, compact) = if fits(full_rw, 12.5) {
        (full_rw, 12.5, false)
    } else if fits(full_rw, 6.0) {
        (full_rw, 6.0, false)
    } else if fits(32.0, 12.5) {
        (32.0, 12.5, true)
    } else {
        (32.0, 6.0, true)
    };
    let start = after_left(rw).max(r.center().x - tools(gap) * 0.5);
    if shown > 0.01 {
        let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(id.with("reset-ui")));
        ru.multiply_opacity(shown);
        let at = Rect::from_min_size(pos2(r.left() + cw + 6.0, mid - 16.0), vec2(rw, 32.0));
        let resp = if compact {
            IconButton::new(icons::ARROW_COUNTER_CLOCKWISE, mark("Reset")).size(32.0, 16.0).id_salt(id.with("reset")).show_at(&mut ru, at)
        } else {
            reset.show_at(&mut ru, at)
        };
        if resettable && super::super::sharing::pressed(resp) {
            *g = Geometry::default();
            ed.crop.zeroed = [None; 3];
        }
    }
    // Done, lit once something changed.
    let b = done;
    if super::super::sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(r.right() - dw, mid - 16.0), vec2(dw, 32.0)))) {
        ed.selected = super::Selected::Nothing;
        return;
    }

    // Flip, rotate, AUTO, shapes, more, in the middle.
    let found = ed.crop.level.flatten();
    let auto_on = found.is_some_and(|f| (g.straighten - auto_straighten(g, &f)).abs() < 0.05 && g.straighten != 0.0);
    let mut x = start;
    let mut next = |w: f32| {
        let at = Rect::from_min_size(pos2(x, mid - 18.0), vec2(w, 36.0));
        x += w + gap;
        at
    };
    let flip_r = next(36.0);
    if tool(ui, id.with("flip"), flip_r, mark("Flip across"), false, true, |p, c, color| widgets::icon(p, c, icons::FLIP_HORIZONTAL, 18.0, color)) {
        flip(g, true);
    }
    let turn_r = next(36.0);
    if tool(ui, id.with("turn"), turn_r, mark("Rotate a quarter to the left"), false, true, |p, c, color| widgets::icon(p, c, icons::ARROW_ARC_LEFT, 18.0, color)) {
        turn(g, true);
    }
    // AUTO: the picture levelled by its lines; lit while it is.
    let auto_r = next(auto_w);
    let label = match found {
        Some(f) => tr!("Level the picture by its lines ({angle})", angle = format!("{:+.1}°", auto_straighten(g, &f))),
        None => tr("No straight lines to level the picture by").to_string(),
    };
    let aid = id.with("auto");
    let resp = ui.interact(auto_r, aid, if found.is_some() { Sense::click() } else { Sense::hover() });
    widgets::note_control(ui.ctx(), "AUTO", auto_r, found.is_some());
    let lit = anim::toggle(ui.ctx(), aid.with("lit"), auto_on, Spring::SNAPPY);
    let hover = anim::toggle(ui.ctx(), aid.with("hover"), resp.hovered() && found.is_some(), Spring::SNAPPY);
    {
        let p = ui.painter();
        let pill = Rect::from_center_size(auto_r.center(), vec2(auto_w - 6.0, 24.0));
        p.rect_filled(pill, theme::radius(12.0), theme::mix(theme::white(0.04 + 0.05 * hover), theme::accent(), lit));
        if lit < 0.99 {
            p.rect_stroke(pill, theme::radius(12.0), Stroke::new(1.2, theme::alpha(if found.is_some() { theme::fg_2() } else { theme::fg_4() }, 1.0 - lit)), egui::StrokeKind::Inside);
        }
        let color = theme::mix(if found.is_some() { theme::fg() } else { theme::fg_4() }, theme::on_accent(), lit);
        widgets::text(p, pill.center(), Align2::CENTER_CENTER, &Txt::new(tr("AUTO"), 11.5).weight(weight::BOLD).tracking(0.06).color(color));
    }
    let resp = resp.on_hover_text(label);
    let resp = if found.is_some() { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    if let (Some(f), true) = (found, found.is_some() && widgets::activated(&resp)) {
        g.straighten = if auto_on { 0.0 } else { auto_straighten(g, &f) };
        ed.crop.dial = Dial::Straighten;
        ed.crop.shapes = false;
        ed.crop.touched = anim::time(ui.ctx());
        ed.crop.by_ruler = true;
    }
    let aspect_r = next(36.0);
    let shapes = ed.crop.shapes;
    if tool(ui, id.with("aspect"), aspect_r, mark("Shapes"), shapes || g.aspect != Aspect::Free, true, aspect_icon) {
        ed.crop.shapes = !shapes;
    }
    let more_r = next(36.0);
    let pasted = COPIED.lock().ok().and_then(|c| c.clone());
    let items = [
        Item::new(tr("Rotate a quarter to the right")).icon(icons::ARROW_ARC_RIGHT),
        Item::new(tr("Flip upside down")).icon(icons::FLIP_VERTICAL),
        Item::new(tr("Copy this crop")).icon(icons::COPY),
        Item::new(if pasted.is_some() { tr("Paste the crop copied") } else { tr("Paste a crop (copy one first)") }).icon(icons::COPY_SIMPLE),
        Item::new(tr("Undo every change of the crop")).icon(icons::ARROW_U_UP_LEFT),
    ];
    match popup::menu_button(ui, id.with("more"), more_r, icons::DOTS_THREE, tr("More"), &items) {
        Some(0) => turn(g, false),
        Some(1) => flip(g, false),
        Some(2) => {
            if let Ok(mut c) = COPIED.lock() {
                *c = Some(g.clone());
            }
        }
        Some(3) => {
            if let Some(mut copied) = pasted {
                // Its shape kept for this picture: the crop asked again, fitted to this picture when drawn.
                copied.crop = geometry::clamp_to_frame(copied.crop);
                *g = copied;
                let _ = (tw, th);
            }
        }
        Some(4) => *g = entered,
        _ => {}
    }
}

/// The three dials and the ruler of the chosen one.
fn dials(ed: &mut Editor, ui: &mut Ui, id: Id, r: Rect) {
    let ctx = ui.ctx().clone();
    let g = &mut ed.doc.geometry;
    let st = &mut ed.crop;
    let row_y = r.top() + 30.0;
    let gap = 76.0;
    for (i, dial) in Dial::ALL.iter().enumerate() {
        let c = pos2(r.center().x + (i as f32 - 1.0) * gap, row_y);
        let did = id.with(("dial", i));
        let hit = Rect::from_center_size(c, vec2(52.0, 52.0));
        let resp = ui.interact(hit, did, Sense::click());
        widgets::note_control(&ctx, dial.name(), hit, true);
        let chosen = st.dial == *dial;
        let value = dial.value(g);
        let hover = anim::toggle(&ctx, did.with("hover"), resp.hovered(), Spring::SNAPPY);
        let on = anim::toggle(&ctx, did.with("on"), chosen, Spring::SNAPPY);
        let shown = anim::value(&ctx, did.with("value"), value, Spring::SNAPPY);
        let p = ui.painter();
        let rad = 21.0;
        p.circle_filled(c, rad + 1.0, theme::white(0.03 + 0.04 * hover + 0.03 * on));
        p.circle_stroke(c, rad, Stroke::new(2.0, theme::white(0.14 + 0.1 * on)));
        // The ring fills with the amount: clockwise from the top for a positive value, back for a negative one.
        let k = (shown / MAX).clamp(-1.0, 1.0);
        if k.abs() > 0.002 {
            let n = ((k.abs() * 64.0) as usize).max(2);
            let pts: Vec<Pos2> = (0..=n)
                .map(|j| {
                    let a = (-90.0f32 + 360.0 * k * j as f32 / n as f32).to_radians();
                    c + vec2(a.cos(), a.sin()) * rad
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(3.0, theme::alpha(theme::accent(), 0.55 + 0.45 * on))));
        }
        let rounded = value.round() as i32;
        if chosen && rounded != 0 {
            widgets::text(p, c, Align2::CENTER_CENTER, &Txt::new(format!("{rounded}"), 14.0).weight(weight::SEMIBOLD).color(theme::fg()));
        } else {
            let color = if value.abs() >= 0.05 { theme::accent() } else { theme::mix(theme::fg_2(), theme::fg(), on.max(hover)) };
            dial_icon(p, c, *dial, color);
        }
        let resp = resp.on_hover_text(tr(dial.about())).on_hover_cursor(CursorIcon::PointingHand);
        if widgets::activated(&resp) {
            if chosen {
                // Pressed again: to zero, and back to what it was by a press after.
                let v = dial.value_mut(g);
                match st.zeroed[i].take() {
                    Some(before) if *v == 0.0 => *v = before,
                    _ if *v != 0.0 => {
                        st.zeroed[i] = Some(*v);
                        *v = 0.0;
                    }
                    _ => {}
                }
                st.touched = anim::time(&ctx);
                st.by_ruler = true;
            } else {
                st.dial = *dial;
                st.ruler.velocity = 0.0;
                st.ruler.coasting = false;
            }
        }
    }
    // The chosen dial's name, then its ruler.
    let name_y = row_y + 37.0;
    widgets::text(ui.painter(), pos2(r.center().x, name_y), Align2::CENTER_CENTER, &Txt::caps(tr(st.dial.name())));
    let ruler_r = Rect::from_center_size(pos2(r.center().x, r.bottom() - 22.0), vec2(r.width().min(460.0), 40.0));
    let dial = st.dial;
    let value = dial.value_mut(g);
    let before = *value;
    ruler(ui, &mut st.ruler, id.with(("ruler", dial.name())), ruler_r, value);
    if *value != before {
        st.touched = anim::time(&ctx);
        st.by_ruler = true;
        st.zeroed = [None; 3];
    }
}

/// A ruler of degrees scrubbed left and right (its ticks under the pointer), coasting when let go, holding at zero a
/// moment as it passes (a tick of the mark there), within ±45°. A double press puts it back to zero.
fn ruler(ui: &mut Ui, st: &mut Ruler, id: Id, r: Rect, value: &mut f32) {
    let ctx = ui.ctx().clone();
    let now = anim::time(&ctx);
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let dt = (now - st.time).clamp(0.0, 0.05) as f32;
    st.time = now;
    let reach_zero = |st: &mut Ruler| {
        st.ticked = now;
        st.notch = 1.2;
    };
    if resp.double_clicked() {
        *value = 0.0;
        st.coasting = false;
        st.velocity = 0.0;
    } else if resp.dragged() {
        st.held = true;
        st.coasting = false;
        let mut change = -resp.drag_delta().x / SPACING;
        let old = *value;
        if old == 0.0 && st.notch > 0.0 {
            // Held at zero until the notch is passed.
            let used = change.abs().min(st.notch);
            st.notch -= used;
            change -= used * change.signum();
        }
        let mut new = (old + change).clamp(-MAX, MAX);
        if (old > 0.0 && new <= 0.0) || (old < 0.0 && new >= 0.0) {
            new = 0.0;
            reach_zero(st);
        }
        *value = new;
    } else if st.held {
        // Let go: it coasts on at the speed it had.
        st.held = false;
        let v = -ctx.input(|i| i.pointer.velocity().x) / SPACING;
        st.velocity = v.clamp(-160.0, 160.0);
        st.coasting = st.velocity.abs() > 4.0;
    }
    if st.coasting {
        let old = *value;
        let mut new = old + st.velocity * dt;
        st.velocity *= (-4.5 * dt).exp();
        if (old > 0.0 && new <= 0.0) || (old < 0.0 && new >= 0.0) {
            // Slow enough, it stops at zero; faster, it passes with a tick.
            if st.velocity.abs() < 40.0 {
                new = 0.0;
                st.coasting = false;
            }
            st.ticked = now;
        }
        if new.abs() >= MAX {
            new = new.clamp(-MAX, MAX);
            st.coasting = false;
        }
        if st.velocity.abs() < 0.8 {
            st.coasting = false;
        }
        *value = new;
        ctx.request_repaint();
    }
    if !resp.dragged() && !st.coasting {
        st.notch = 0.0;
    }

    // The ticks, fading to the sides; a dot over zero; the mark in the middle.
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    let c = r.center();
    let span = (r.width() * 0.5 / SPACING).ceil() as i32 + 1;
    let base = value.round() as i32;
    for u in (base - span)..=(base + span) {
        if u.abs() > MAX as i32 {
            continue;
        }
        let x = c.x + (u as f32 - *value) * SPACING;
        let edge = ((x - c.x).abs() / (r.width() * 0.5)).clamp(0.0, 1.0);
        let fade = 1.0 - edge * edge;
        let major = u % 5 == 0;
        let h = if major { 14.0 } else { 8.0 };
        p.line_segment([pos2(x, c.y + 4.0 - h * 0.5), pos2(x, c.y + 4.0 + h * 0.5)], Stroke::new(if major { 1.4 } else { 1.0 }, theme::white((if major { 0.62 } else { 0.32 }) * fade)));
        if u == 0 {
            p.circle_filled(pos2(x, c.y - 9.0), 2.0, theme::white(0.85 * fade));
        }
    }
    let tick = (1.0 - ((now - st.ticked) / 0.25) as f32).clamp(0.0, 1.0);
    if tick > 0.0 {
        ctx.request_repaint();
    }
    let lit = if value.abs() >= 0.05 { theme::accent() } else { Color32::WHITE };
    p.line_segment([pos2(c.x, c.y - 9.0 - 3.0 * tick), pos2(c.x, c.y + 15.0 + 3.0 * tick)], Stroke::new(2.0 + 1.2 * tick, lit));
    if resp.hovered() || resp.dragged() {
        ctx.set_cursor_icon(if resp.dragged() { CursorIcon::Grabbing } else { CursorIcon::ResizeHorizontal });
    }
    widgets::note_control(&ctx, "Ruler", r, true); // i18n: not shown
}

/// Whether the shapes show lying: the crop's shape's way, else as last chosen, else the picture's.
pub(super) fn landscape(st: &State, g: &Geometry, original: f32) -> bool {
    g.aspect.landscape(original).or(st.landscape).unwrap_or(original >= 1.0)
}

/// Shows the shapes lying (`landscape`) or standing: the crop's shape turned to match when it has a way.
pub(super) fn set_landscape(st: &mut State, g: &mut Geometry, landscape: bool, tw: f32, th: f32) {
    st.landscape = Some(landscape);
    if g.aspect.landscape(tw / th.max(1.0)).is_some_and(|l| l != landscape) {
        set_aspect(g, g.aspect.turned(), tw, th);
    }
}

/// Portrait or landscape, then the presets.
fn shapes(ed: &mut Editor, ui: &mut Ui, id: Id, r: Rect, size: (f32, f32)) {
    let (tw, th) = size;
    let original = tw / th.max(1.0);
    let g = &mut ed.doc.geometry;
    let st = &mut ed.crop;
    let landscape = landscape(st, g, original);
    // The orientation, two frames side by side.
    let row_y = r.top() + 24.0;
    for (i, l) in [false, true].into_iter().enumerate() {
        let b = Rect::from_center_size(pos2(r.center().x + (i as f32 - 0.5) * 48.0, row_y), vec2(40.0, 34.0));
        let label = if l { mark("Landscape") } else { mark("Portrait") };
        let on = landscape == l;
        if tool(ui, id.with(("orientation", i)), b, label, on, true, |p, c, color| orientation_icon(p, c, l, color)) {
            set_landscape(st, g, l, tw, th);
        }
    }
    // The presets in the orientation shown.
    let list: Vec<Aspect> = Aspect::PRESETS.iter().map(|a| a.oriented(landscape, original)).collect();
    let widths: Vec<f32> = list.iter().map(|a| Chip::new(a.name()).selected(g.aspect.preset() == a.preset()).width(ui)).collect();
    let room = r.width() - 16.0;
    let (pos, h) = widgets::flow(&widths, room, widgets::CHIP_H, 8.0);
    let rows_w: f32 = {
        // Each row centred: the widest row's width centres the block.
        pos.iter().zip(&widths).map(|(p, w)| p.x + w).fold(0.0, f32::max)
    };
    let origin = pos2(r.center().x - rows_w * 0.5, row_y + 30.0);
    let _ = h;
    for ((a, p), w) in list.iter().zip(pos).zip(widths) {
        let cr = Rect::from_min_size(origin + p, vec2(w, widgets::CHIP_H));
        let chosen = g.aspect.preset() == a.preset();
        if Chip::new(a.name()).selected(chosen).id_salt(id.with(("shape", orbiters_i18n::english_of(a.name())))).show_at(ui, cr).pressed {
            if a.landscape(original).is_some() {
                st.landscape = a.landscape(original);
            }
            set_aspect(g, *a, tw, th);
            st.touched = anim::time(ui.ctx());
            st.by_ruler = false;
        }
    }
}

/// Stages the crop for a screenshot: `dial` chosen (by name), the shapes shown or not, AUTO applied.
pub(super) fn stage(ed: &mut Editor, dial: &str, shapes: bool, auto: bool) {
    ed.crop.dial = Dial::ALL.into_iter().find(|d| d.name().eq_ignore_ascii_case(dial)).unwrap_or(Dial::Straighten);
    ed.crop.shapes = shapes;
    if auto {
        if ed.crop.level.is_none() {
            ed.crop.level = Some(ed.loaded.as_ref().and_then(|l| l.inputs.first()).and_then(level::detect));
        }
        if let Some(found) = ed.crop.level.flatten() {
            ed.doc.geometry.straighten = auto_straighten(&ed.doc.geometry, &found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn crop_handles_move_their_side_and_keep_the_shape_asked() {
        let start = Crop { x: 0.2, y: 0.2, w: 0.4, h: 0.4 };
        let c = dragged(Part::Right, start, [0.1, 0.0], None);
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

    #[test]
    fn handles_and_the_picture_stop_at_the_picture_s_edge() {
        let w = Warp::new(1600.0, 900.0, 10.0, 0.0, 0.0);
        let start = geometry::fit_warp(Crop::FULL, &w);
        // Dragged out, a corner stops where the straightened picture ends.
        let c = dragged_inside(&w, Part::TopLeft, start, [-0.3, -0.3], None);
        assert!(geometry::crop_inside(c, &w), "{c:?}");
        // Moving the picture under a smaller crop: as far as it goes, along the other side once one is reached.
        let small = Crop { x: 0.4, y: 0.4, w: 0.2, h: 0.2 };
        let c = dragged_inside(&w, Part::Move, small, [0.5, 0.05], None);
        assert!(geometry::crop_inside(c, &w) && c.x > 0.7 && c.x < 0.8, "{c:?}");
        let c = dragged_inside(&w, Part::Move, small, [0.0, 0.1], None);
        assert!(near(c.y, 0.5) && near(c.x, 0.4), "{c:?}");
        // Zoomed in about a point it stays there; zoomed out it grows only as far as the picture.
        let z = zoomed(&w, start, [0.3, 0.4], 2.0);
        assert!(near(z.w, start.w / 2.0) && near(z.h, start.h / 2.0), "{z:?}");
        let rel = |c: Crop| [(0.3 - c.x) / c.w, (0.4 - c.y) / c.h];
        assert!(near(rel(z)[0], rel(start)[0]) && near(rel(z)[1], rel(start)[1]));
        let out = zoomed(&w, z, [0.5, 0.5], 0.1);
        assert!(geometry::crop_inside(out, &w) && out.w > z.w, "{out:?}");
        assert!(zoomed(&w, start, [0.5, 0.5], 1000.0).w >= MIN - 1e-6);
    }

    #[test]
    fn the_view_centres_the_crop_as_large_as_it_fits() {
        let area = Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0));
        let c = Crop { x: 0.25, y: 0.25, w: 0.5, h: 0.5 };
        let f = target(area, c, 1600.0, 900.0);
        let cr = crop_rect(f, c);
        assert!((cr.center() - area.center()).length() < 1e-3, "{cr:?}");
        assert!((cr.width() - 800.0).abs() < 1e-3 && cr.height() <= 600.0 + 1e-3, "{cr:?}");
        // A tall crop fills the height.
        let tall = Crop { x: 0.45, y: 0.0, w: 0.1, h: 1.0 };
        let cr = crop_rect(target(area, tall, 1600.0, 900.0), tall);
        assert!((cr.height() - 600.0).abs() < 1e-3 && (cr.center() - area.center()).length() < 1e-3, "{cr:?}");
    }

    #[test]
    fn turns_and_flips_keep_what_shows() {
        // Turned four times, or flipped twice, it is as it was.
        let g0 = Geometry { straighten: 6.0, vertical: 10.0, horizontal: -4.0, flip_h: true, crop: Crop { x: 0.1, y: 0.2, w: 0.5, h: 0.3 }, aspect: Aspect::R3x4, ..Default::default() };
        let mut g = g0.clone();
        for _ in 0..4 {
            turn(&mut g, true);
        }
        assert_eq!(g.turns, g0.turns);
        assert!((g.crop.x - g0.crop.x).abs() < 1e-5 && g.aspect == g0.aspect && g.flip_h == g0.flip_h && g.vertical == g0.vertical);
        turn(&mut g, true);
        turn(&mut g, false);
        assert_eq!((g.vertical, g.horizontal, g.flip_h, g.aspect), (g0.vertical, g0.horizontal, g0.flip_h, g0.aspect));
        flip(&mut g, true);
        assert_eq!((g.straighten, g.horizontal, g.flip_h), (-6.0, 4.0, false));
        assert!((g.crop.x - 0.4).abs() < 1e-6);
        flip(&mut g, true);
        assert!((g.crop.x - g0.crop.x).abs() < 1e-5 && (g.crop.y - g0.crop.y).abs() < 1e-5);
        assert_eq!(Geometry { crop: g0.crop, ..g }, g0);
        // A turn of a flipped picture shows the same picture turned: the source point under each corner turns.
        let aspect = 16.0 / 9.0;
        let g = Geometry { flip_h: true, ..Default::default() };
        let mut t = g.clone();
        turn(&mut t, true);
        // The turned picture's top right corner is the picture's top left before the turn (turned to the left).
        let before = g.source_point(Crop::FULL, aspect, 0.0, 0.0);
        let after_tl = t.source_point(Crop::FULL, aspect, 0.0, 1.0);
        assert!((after_tl[0] - before[0]).abs() < 1e-5 && (after_tl[1] - before[1]).abs() < 1e-5, "{before:?} {after_tl:?}");
    }

    #[test]
    fn auto_levels_a_flipped_picture_the_other_way() {
        let found = Level { tilt: 3.0, lines: 2, agreement: 1.0 };
        assert_eq!(auto_straighten(&Geometry::default(), &found), -3.0);
        assert_eq!(auto_straighten(&Geometry { flip_h: true, ..Default::default() }, &found), 3.0);
        assert_eq!(auto_straighten(&Geometry { flip_h: true, flip_v: true, ..Default::default() }, &found), -3.0);
    }
}
