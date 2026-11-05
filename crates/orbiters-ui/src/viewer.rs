//! A window that grows out of what was clicked (a card's picture, a button) and shrinks back into it, as the
//! website's card previews do (CardPreviewDialog, its gallery and capture windows): the bounds move on one
//! restrained spring ([`Spring::EXPAND`]) from the origin's rect to the window's, never an unequal scale of
//! pictures or text; the backdrop fades with the window, the content fades in once the window has most of its size
//! and goes first when it closes. Closing is the opening backwards, then a landing ([`crate::morph`]): a window whose
//! caller draws its card under it ([`Morph::riding`], [`crate::morph::ride`]) fades in over the card as it leaves it
//! and out over it as it lands, and the card bounces; other windows are opaque from their first frame and fade out on
//! their origin. The window follows its own size (a panel opening in it, another picture's shape) and its origin (the
//! card moved) on the same spring.
//!
//! Picture windows are sized to their picture ([`picture_layout`]): wide pictures sit over their details, tall ones
//! beside them, and a window whose details grow crops its picture rather than scroll.

use crate::anim::{self, Spring};
use crate::morph;
use crate::theme;
use crate::widgets::{self, IconButton};
use crate::{icons, paint};
use egui::{pos2, vec2, Context, Id, Painter, Rect, Ui, Vec2};

/// A window on its way between its origin and where it opens.
#[derive(Clone, Debug)]
pub struct Morph {
    id: Id,
    origin: Rect,
    origin_radius: f32,
    closing: bool,
    riding: bool,
}

/// Where a [`Morph`] is this frame and how much of it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The window's bounds now.
    pub rect: Rect,
    /// 0 on its origin, 1 open (a little past 1 while it settles).
    pub t: f32,
    /// Corner radius now (as for a circular corner; paint with [`widgets::frame`]).
    pub radius: f32,
    /// Opacity of what the window holds besides its picture.
    pub content: f32,
    /// Opacity of the backdrop.
    pub backdrop: f32,
    /// Opacity of the whole window: it fades over its origin in the last stretch of the way back (and, when its card
    /// rides under it, out of it as it opens).
    pub shape: f32,
    /// Opacity of the card under it, for a caller that draws it ([`Morph::riding`]).
    pub source: f32,
    /// Scale of the card it landed on (its bounce), 1 otherwise.
    pub landing: f32,
    /// Landed and settled: drop the window (after drawing this frame) and show the origin again.
    pub closed: bool,
}

/// Corner radius of open windows (the website's 24, a squircle of 48).
pub const RADIUS: f32 = 24.0;

impl Morph {
    /// A window opening from `origin`, whose corners have `origin_radius`. `id` must be new for each opening (the
    /// springs live under it).
    pub fn new(id: Id, origin: Rect, origin_radius: f32) -> Self {
        Self { id, origin, origin_radius, closing: false, riding: false }
    }

    /// Whether the caller draws the window's card under it where [`Frame::source`] says (the window then fades in
    /// over the card as it leaves it).
    pub fn riding(&mut self, riding: bool) {
        self.riding = riding;
    }

    pub fn id(&self) -> Id {
        self.id
    }

    pub fn origin(&self) -> Rect {
        self.origin
    }

    /// Another origin to close into (the window now shows another card's item).
    pub fn set_origin(&mut self, origin: Rect) {
        self.origin = origin;
    }

    pub fn close(&mut self) {
        self.closing = true;
    }

    /// Opens again while closing (pressed again before it was back).
    pub fn reopen(&mut self) {
        self.closing = false;
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// The window this frame, on its way to `target` (or back to its origin while closing, then landing on it).
    pub fn frame(&self, ctx: &Context, target: Rect) -> Frame {
        let travel = morph::travel(ctx, self.id, !self.closing, Spring::EXPAND);
        let goal = rect_array(target);
        let sprung = anim::spring_from(ctx, self.id.with("target"), goal, goal, Spring::EXPAND);
        let origin = self.sprung_origin(ctx);
        if travel.landed {
            morph::land(ctx, origin, travel.landing);
        }
        let f = frame_at(origin, self.origin_radius, array_rect(sprung), travel.t, self.closing || self.riding);
        Frame { landing: travel.landing, closed: travel.done, ..f }
    }

    /// The origin, following the card on the window's spring when it moves (another item's card, the grid scrolled).
    fn sprung_origin(&self, ctx: &Context) -> Rect {
        if !self.origin.is_finite() {
            return self.origin;
        }
        let goal = rect_array(self.origin);
        array_rect(anim::spring_from(ctx, self.id.with("origin"), goal, goal, Spring::EXPAND))
    }

    /// A part of the window (its picture) on the same way as the window `f`: from the origin to `target`, where it
    /// sits once open, following it on the same spring when it changes. `name` tells the parts apart.
    pub fn part(&self, ctx: &Context, name: &str, target: Rect, f: &Frame) -> Rect {
        let goal = rect_array(target);
        let sprung = anim::spring_from(ctx, self.id.with(("part", name)), goal, goal, Spring::EXPAND);
        lerp_rect(self.sprung_origin(ctx), array_rect(sprung), f.t)
    }
}

/// The window at progress `t` between `origin` and `target` (see [`Morph::frame`]); `fades`: it hands over to its
/// origin near it ([`morph::shape_alpha`]) instead of staying opaque (a window opening from a card nobody draws).
pub fn frame_at(origin: Rect, origin_radius: f32, target: Rect, t: f32, fades: bool) -> Frame {
    let k = t.clamp(0.0, 1.0);
    Frame {
        rect: lerp_rect(origin, target, t),
        t,
        radius: origin_radius + (RADIUS - origin_radius) * k,
        content: smoothstep(0.45, 0.9, t),
        backdrop: (t * 1.6).clamp(0.0, 1.0),
        shape: if fades { morph::shape_alpha(t) } else { 1.0 },
        source: morph::source_alpha(t),
        landing: 1.0,
        closed: false,
    }
}

fn rect_array(r: Rect) -> [f32; 4] {
    [r.left(), r.top(), r.width(), r.height()]
}

fn array_rect([x, y, w, h]: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w.max(0.0), h.max(0.0)))
}

/// `a` at 0, `b` at 1, each edge moving on its own (a little past `b` above 1).
pub fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rect::from_min_max(pos2(l(a.left(), b.left()), l(a.top(), b.top())), pos2(l(a.right(), b.right()), l(a.bottom(), b.bottom())))
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The dimmed window behind an open window (`area`: what it covers).
pub fn paint_backdrop(painter: &Painter, area: Rect, f: &Frame) {
    painter.rect_filled(area, 0.0, theme::alpha(theme::INK_950, 0.72 * f.backdrop));
}

/// The window's surface and its shadow, which grows in as it opens (both as opaque as the window, [`Frame::shape`]).
pub fn paint_surface(painter: &Painter, f: &Frame) {
    let k = f.t.clamp(0.0, 1.0);
    if k > 0.01 {
        paint::shadow(painter, f.rect, f.radius, vec2(0.0, 24.0 * k), 48.0, -8.0, theme::black(0.5 * k * f.shape));
    }
    widgets::frame(painter, f.rect, f.radius, theme::surface_strong().gamma_multiply(f.shape));
}

/// The close button inset in the window's top-right corner: a touch-sized target on a dark round backdrop, so it
/// reads over pictures. True when pressed.
pub fn close_button(ui: &mut Ui, window: Rect, label: &str) -> bool {
    let r = close_rect(window);
    let resp = IconButton::new(icons::X, label).size(40.0, 18.0).backdrop(theme::black(0.5)).show_at(ui, r);
    widgets::activated(&resp)
}

/// Where [`close_button`] sits in a window.
pub fn close_rect(window: Rect) -> Rect {
    Rect::from_min_size(pos2(window.right() - 12.0 - 40.0, window.top() + 12.0), vec2(40.0, 40.0))
}

/// Where a control of `width` goes on [`close_button`]'s row, just left of it: the window's Edit.
pub fn beside_close(window: Rect, width: f32) -> Rect {
    let close = close_rect(window);
    Rect::from_min_size(pos2(close.left() - 8.0 - width, close.top()), vec2(width, close.height()))
}

/// How a picture window is arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
    /// Wide pictures: the picture over its details.
    Under,
    /// Tall pictures: the details in a column beside the picture.
    Beside,
}

/// A picture window's parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PictureLayout {
    pub window: Rect,
    pub picture: Rect,
    pub details: Rect,
    pub arrangement: Arrangement,
}

/// Width of the details column beside tall pictures.
pub const SIDE_W: f32 = 340.0;
/// Pictures whose window would be narrower than this get their details beside them.
pub const NARROW: f32 = 560.0;
/// A picture is cropped to make room for its details down to this height, no further.
pub const MIN_PICTURE_H: f32 = 160.0;

/// Width of a picture window for a picture of `aspect` (width / height) in an `area`, as the website's: at most
/// 960 points wide, the picture at most 65 % of the area's height and 600 points high, never narrower than 360.
pub fn picture_width(area: Vec2, aspect: f32) -> f32 {
    ((area.y * 0.65).min(600.0) * aspect).max(360.0).min(960.0).min(area.x)
}

fn sane_aspect(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 {
        aspect.clamp(0.2, 5.0)
    } else {
        16.0 / 9.0
    }
}

/// A picture window centred in `area` for a picture of `aspect`; `details_h(width)` is how high its details are at
/// a given width. A picture whose details do not fit under it is cropped (from [`MIN_PICTURE_H`] on, the window
/// grows past the area instead).
pub fn picture_layout(area: Rect, aspect: f32, details_h: impl Fn(f32) -> f32) -> PictureLayout {
    let aspect = sane_aspect(aspect);
    let w = picture_width(area.size(), aspect);
    if w >= NARROW.min(area.width()) {
        let dh = details_h(w).max(0.0);
        let ph = (w / aspect).min((area.height() - dh).max(MIN_PICTURE_H));
        let window = Rect::from_center_size(area.center(), vec2(w, ph + dh));
        let window = window.translate(vec2(0.0, (area.top() - window.top()).max(0.0)));
        let picture = Rect::from_min_size(window.min, vec2(w, ph));
        let details = Rect::from_min_max(pos2(window.left(), picture.bottom()), window.max);
        return PictureLayout { window, picture, details, arrangement: Arrangement::Under };
    }
    let side = SIDE_W.min(area.width() * 0.5);
    let mut ph = area.height().min(680.0);
    let mut pw = ph * aspect;
    if pw + side > area.width() {
        pw = (area.width() - side).max(1.0);
        ph = pw / aspect;
    }
    let h = ph.max(details_h(side)).min(area.height().max(ph));
    let window = Rect::from_center_size(area.center(), vec2(pw + side, h));
    let picture = Rect::from_min_size(window.min, vec2(pw, h));
    let details = Rect::from_min_max(pos2(picture.right(), window.top()), window.max);
    PictureLayout { window, picture, details, arrangement: Arrangement::Beside }
}

/// The largest rect of `aspect` that fits in `area`, centred (a picture shown whole).
pub fn contain(area: Rect, aspect: f32) -> Rect {
    let aspect = sane_aspect(aspect);
    let (w, h) = if area.width() / area.height().max(1.0) > aspect { (area.height() * aspect, area.height()) } else { (area.width(), area.width() / aspect) };
    Rect::from_center_size(area.center(), vec2(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_window_starts_on_its_origin_and_ends_on_its_target() {
        let origin = Rect::from_min_size(pos2(10.0, 20.0), vec2(300.0, 169.0));
        let target = Rect::from_min_size(pos2(100.0, 60.0), vec2(900.0, 620.0));
        let start = frame_at(origin, 20.0, target, 0.0, false);
        assert_eq!(start.rect, origin);
        assert_eq!(start.radius, 20.0);
        assert_eq!(start.content, 0.0);
        let end = frame_at(origin, 20.0, target, 1.0, false);
        assert_eq!(end.rect, target);
        assert_eq!(end.radius, RADIUS);
        assert_eq!(end.content, 1.0);
        assert_eq!(end.backdrop, 1.0);
        // Halfway: the bounds are halfway, the text not there yet (it never shows squeezed).
        let mid = frame_at(origin, 20.0, target, 0.5, false);
        assert!(close(mid.rect.width(), 600.0) && close(mid.rect.left(), 55.0));
        assert!(mid.content < 0.2, "{}", mid.content);
        // A bounce past 1 overshoots the target a little, the radius never past its own.
        let past = frame_at(origin, 20.0, target, 1.03, false);
        assert!(past.rect.width() > target.width() && past.radius == RADIUS);
        // Near its origin it hands over to it when it fades (closing, or its card drawn under it), else stays opaque.
        assert_eq!(frame_at(origin, 20.0, target, 0.0, true).shape, 0.0);
        assert_eq!(frame_at(origin, 20.0, target, 0.0, false).shape, 1.0);
        assert_eq!(frame_at(origin, 20.0, target, 0.0, true).source, 1.0);
        assert_eq!(frame_at(origin, 20.0, target, 0.6, true).source, 0.0);
    }

    #[test]
    fn picture_windows_follow_the_website_width() {
        // The desktop's page below its title bar, less its margins.
        let area = vec2(1000.0, 732.0);
        assert!(close(picture_width(area, 16.0 / 9.0), 732.0 * 0.65 * 16.0 / 9.0));
        assert_eq!(picture_width(vec2(2000.0, 1400.0), 16.0 / 9.0), 960.0);
        assert_eq!(picture_width(area, 9.0 / 16.0), 360.0);
        assert_eq!(picture_width(vec2(300.0, 700.0), 16.0 / 9.0), 300.0);
    }

    #[test]
    fn wide_pictures_sit_over_their_details_and_crop_for_them() {
        let area = Rect::from_min_size(pos2(256.0, 64.0), vec2(1000.0, 732.0));
        let l = picture_layout(area, 16.0 / 9.0, |_| 110.0);
        assert_eq!(l.arrangement, Arrangement::Under);
        assert!(close(l.picture.height(), l.picture.width() * 9.0 / 16.0), "{l:?}");
        assert!(close(l.window.height(), l.picture.height() + 110.0));
        assert!(close(l.window.center().x, area.center().x) && close(l.window.center().y, area.center().y));
        assert_eq!(l.details.top(), l.picture.bottom());
        assert!(area.contains_rect(l.window));
        // Details that do not fit under the whole picture take some of it.
        let tall = picture_layout(area, 16.0 / 9.0, |_| 400.0);
        assert!(close(tall.picture.height(), 332.0) && close(tall.window.height(), 732.0), "{tall:?}");
        assert!(close(tall.window.width(), l.window.width()));
        // Never under the minimum: the window then grows past the area, downwards.
        let huge = picture_layout(area, 16.0 / 9.0, |_| 700.0);
        assert!(close(huge.picture.height(), MIN_PICTURE_H) && close(huge.window.top(), area.top()), "{huge:?}");
    }

    #[test]
    fn tall_pictures_get_their_details_beside_them() {
        let area = Rect::from_min_size(pos2(256.0, 64.0), vec2(1000.0, 732.0));
        let l = picture_layout(area, 9.0 / 16.0, |w| if w == SIDE_W { 300.0 } else { 900.0 });
        assert_eq!(l.arrangement, Arrangement::Beside);
        assert!(close(l.details.width(), SIDE_W));
        assert!(close(l.picture.width(), l.picture.height() * 9.0 / 16.0), "{l:?}");
        assert!(area.contains_rect(l.window));
        assert_eq!(l.details.left(), l.picture.right());
        // An unusable aspect is a 16:9 picture.
        assert_eq!(picture_layout(area, f32::NAN, |_| 100.0).arrangement, Arrangement::Under);
    }

    #[test]
    fn contained_pictures_keep_their_shape() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 500.0));
        let r = contain(area, 16.0 / 9.0);
        assert!(close(r.height(), 500.0) && close(r.width(), 888.89) && close(r.center().x, 500.0));
        let r = contain(area, 4.0);
        assert!(close(r.width(), 1000.0) && close(r.height(), 250.0));
    }
}
