//! The floating panel: the SteamVR tab's interface ([`crate::overlay::ui`], the same state, its own egui context and
//! CPU painter, the tab's backdrop) on an overlay in the play space that is no dashboard overlay and takes no SteamVR
//! laser (no mouse input, no "interactive if visible"), so VRChat keeps the scene and the controllers; the lasers of
//! [`super::input`] feed its pointer. Under it, as N4N0's screen has its handle, a bar: smaller, the handle that moves
//! it, larger, close. It paints as the tab does: only when something asks (the laser, the service's news, an
//! animation) and only what changed, at most [`MIN_FRAME`] apart while a laser is on it and [`IDLE_FRAME`] apart
//! otherwise (it stays open beside the game: the service's news alone need not cost the game 45 pictures a second);
//! the bar only when its hover or the size changes.

use super::space::{self, Follow, Quad, V3};
use crate::backend::Ctl;
use crate::overlay::ui::{self, OverlayUi, Poses};
use crate::overlay::{Canvas, Presenter};
use crate::state::Snapshot;
use egui::{pos2, vec2, Event, PointerButton, Pos2, RawInput, Rect, Stroke, ViewportId, ViewportInfo};
use orbiters_core::settings::FloatingSettings;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_ui::anim::Spring;
use orbiters_ui::software::Software;
use orbiters_ui::{icons, theme, widgets};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The panel's pixels: the tab's.
pub const PX: [u32; 2] = [ui::SIZE.x as u32, ui::SIZE.y as u32];
/// The bar: its pixels and its width in the play space (its buttons about 3 cm across), its gap under the panel.
pub const BAR_PX: [u32; 2] = [480, 72];
pub const BAR_WIDTH_M: f32 = 0.3;
const BAR_GAP_M: f32 = 0.012;
/// Where it opens before the eyes.
const OPEN_DISTANCE_M: f32 = 0.7;
const OPEN_BELOW_M: f32 = 0.12;
/// Its pictures at most this often, as the tab's.
const MIN_FRAME: Duration = Duration::from_millis(22);
/// Its pictures at most this often while no laser is on it.
const IDLE_FRAME: Duration = Duration::from_millis(100);
/// Under N4N0's menu (10) and the watch, over the 3D worlds' cube (0).
const SORT_ORDER: u32 = 4;

/// The bar's buttons, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButton {
    Smaller,
    Larger,
    Handle,
    Close,
}

impl BarButton {
    /// Where it is on the bar (pixels).
    pub fn rect(self) -> Rect {
        let round = |x: f32| Rect::from_center_size(pos2(x, 36.0), vec2(56.0, 56.0));
        match self {
            BarButton::Smaller => round(40.0),
            BarButton::Larger => round(104.0),
            BarButton::Handle => Rect::from_min_max(pos2(150.0, 0.0), pos2(330.0, 72.0)),
            BarButton::Close => round(440.0),
        }
    }

    /// The button under (u, v) on the bar.
    pub fn at(uv: [f32; 2]) -> Option<BarButton> {
        let p = pos2(uv[0] * BAR_PX[0] as f32, uv[1] * BAR_PX[1] as f32);
        [BarButton::Smaller, BarButton::Larger, BarButton::Handle, BarButton::Close].into_iter().find(|b| b.rect().expand(4.0).contains(p))
    }

    /// The handle's part of the bar (u0, v0, u1, v1): a trigger pulled there takes the panel.
    pub fn handle_uv() -> [f32; 4] {
        let r = BarButton::Handle.rect();
        [r.left() / BAR_PX[0] as f32, r.top() / BAR_PX[1] as f32, r.right() / BAR_PX[0] as f32, r.bottom() / BAR_PX[1] as f32]
    }
}

/// The panel's height for its width (the tab's shape).
pub fn height(width: f32) -> f32 {
    width * PX[1] as f32 / PX[0] as f32
}

/// The bar's place under a panel at `placed`, `width` wide.
pub fn bar_at(placed: &Matrix34, width: f32) -> Matrix34 {
    let bar_height = BAR_WIDTH_M * BAR_PX[1] as f32 / BAR_PX[0] as f32;
    space::offset(placed, [0.0, -(height(width) / 2.0 + BAR_GAP_M + bar_height / 2.0), 0.0])
}

/// The next of the panel's widths, `step` steps from the one nearest `width`.
pub fn step_width(width: f32, step: i32) -> f32 {
    let widths = FloatingSettings::WIDTHS;
    let nearest = (0..widths.len()).min_by(|&a, &b| (widths[a] - width).abs().total_cmp(&(widths[b] - width).abs())).unwrap_or(0);
    widths[(nearest as i32 + step).clamp(0, widths.len() as i32 - 1) as usize]
}

/// Where the panel opens for the eyes at `head`: where it was left when that is before them, else before them.
pub fn opening_place(head: &Matrix34, settings: &FloatingSettings) -> Matrix34 {
    settings
        .placement
        .and_then(|m| space::orthonormal(&Matrix34 { m }))
        .filter(|placed| space::in_view(placed, head))
        .unwrap_or_else(|| space::before_eyes(head, OPEN_DISTANCE_M, OPEN_BELOW_M))
}

pub struct Panel {
    handle: vr::OverlayHandle,
    bar: vr::OverlayHandle,
    ctx: egui::Context,
    painter: Software,
    bar_canvas: Canvas,
    /// Open (closing, it fades out first); where it is and how wide.
    open: bool,
    placed: Matrix34,
    pub width: f32,
    /// Shown in SteamVR (the first picture went), its fade, the alpha SteamVR was last told.
    shown: bool,
    fade: Follow,
    alpha: f32,
    /// The head is behind it: it shows its picture mirrored, which reads right from there.
    behind: bool,
    pointer: Pointer,
    next_frame: Option<Instant>,
    last_frame: Option<Instant>,
    painted: Option<Arc<Snapshot>>,
    bar_drawn: Option<(Option<BarButton>, Option<BarButton>, f32)>,
    started: Instant,
}

impl Panel {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}"); // i18n: not shown (the overlay process's own stderr, a log nobody reads in a window)
        let handle = overlay.create(&format!("{key}.floating"), "Orbiters").map_err(|c| fail("floating panel", c))?; // i18n: not shown (an error text)
        let bar = overlay.create(&format!("{key}.floating-bar"), "Orbiters panel's bar").map_err(|c| fail("floating panel's bar", c))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(bar, BAR_WIDTH_M);
        for h in [handle, bar] {
            let _ = overlay.set_flag(h, overlay_flag::IS_PREMULTIPLIED, true);
            let _ = overlay.set_sort_order(h, SORT_ORDER);
        }
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        Ok(Self {
            handle,
            bar,
            ctx,
            painter: crate::overlay::painter(),
            bar_canvas: Canvas::new(BAR_PX[0], BAR_PX[1]),
            open: false,
            placed: Matrix34::IDENTITY,
            width: FloatingSettings::default().width_m,
            shown: false,
            fade: Follow::at(0.0),
            alpha: 0.0,
            behind: false,
            pointer: Pointer::default(),
            next_frame: None,
            last_frame: None,
            painted: None,
            bar_drawn: None,
            started: Instant::now(),
        })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
        let _ = overlay.destroy(self.bar);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open, or fading out.
    pub fn is_shown(&self) -> bool {
        self.open || self.shown
    }

    pub fn placed(&self) -> Matrix34 {
        self.placed
    }

    /// Opens it at `placed`, `width` wide; its first picture whole.
    pub fn open(&mut self, overlay: &vr::Overlay, placed: Matrix34, width: f32) {
        self.open = true;
        self.width = width;
        let _ = overlay.set_width_in_meters(self.handle, width);
        self.place(overlay, placed);
        self.painter.invalidate();
        self.bar_drawn = None;
        self.next_frame = Some(Instant::now());
    }

    /// Closes it (it fades out). A press held on it is let go in its last picture.
    pub fn close(&mut self) {
        self.open = false;
        self.let_go();
    }

    /// Puts it (and its bar) at `placed`.
    pub fn place(&mut self, overlay: &vr::Overlay, placed: Matrix34) {
        self.placed = placed;
        let _ = overlay.set_transform_absolute(self.handle, &placed);
        let _ = overlay.set_transform_absolute(self.bar, &bar_at(&placed, self.width));
    }

    /// A new width, about its middle.
    pub fn resize(&mut self, overlay: &vr::Overlay, width: f32) {
        if (width - self.width).abs() > 1e-4 {
            self.width = width;
            let _ = overlay.set_width_in_meters(self.handle, width);
            self.place(overlay, self.placed);
        }
    }

    /// What the lasers can point at: the panel and its bar, and the handle's middle (while open).
    pub fn targets(&self) -> Option<(Quad, Quad, V3)> {
        self.open.then(|| {
            let bar = bar_at(&self.placed, self.width);
            let bar_height = BAR_WIDTH_M * BAR_PX[1] as f32 / BAR_PX[0] as f32;
            (Quad { transform: self.placed, width: self.width, height: height(self.width) }, Quad { transform: bar, width: BAR_WIDTH_M, height: bar_height }, space::position(&bar))
        })
    }

    /// The head at `head`: behind the panel it shows its picture mirrored (SteamVR shows an overlay's back as its front
    /// seen through), so it reads right from there, and the lasers' places on it are mirrored back.
    pub fn face(&mut self, overlay: &vr::Overlay, head: &Matrix34) {
        let behind = crate::overlay::menu::behind(&self.placed, head, self.behind);
        if behind != self.behind {
            self.behind = behind;
            let (u0, u1) = if behind { (1.0, 0.0) } else { (0.0, 1.0) };
            let _ = overlay.set_texture_bounds(self.handle, u0, 0.0, u1, 1.0);
            let _ = overlay.set_texture_bounds(self.bar, u0, 0.0, u1, 1.0);
        }
    }

    /// A place on the panel or its bar (u, v as the laser meets it) in its picture's.
    pub fn picture_uv(&self, uv: [f32; 2]) -> [f32; 2] {
        if self.behind {
            [1.0 - uv[0], uv[1]]
        } else {
            uv
        }
    }

    /// The laser's pointer this turn: where on the panel (u, v as the laser meets it), its button, whether it left.
    pub fn pointer(&mut self, at: Option<[f32; 2]>, button: Option<bool>, gone: bool) {
        let at = at.map(|uv| self.picture_uv(uv)).map(|uv| pos2(uv[0] * PX[0] as f32, uv[1] * PX[1] as f32));
        self.pointer.update(at, button, gone);
    }

    fn let_go(&mut self) {
        self.pointer.let_go();
    }

    /// Paints it when something asked (and SteamVR gets only what changed), shows it with its first picture, fades it
    /// in or out. Returns whether one of its buttons was pressed (for the click sound).
    pub(in crate::overlay) fn paint(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, snap: &Arc<Snapshot>, backend: &Ctl, state: &mut OverlayUi, poses: &mut dyn Poses, dt: f32) -> bool {
        let mut pressed = false;
        if !self.open && !self.shown {
            self.pointer.events.clear();
            return false;
        }
        if !self.painted.as_ref().is_some_and(|s| Arc::ptr_eq(s, snap)) {
            self.painted = Some(snap.clone());
            self.next_frame = Some(self.next_frame.map_or(Instant::now(), |f| f.min(Instant::now())));
        }
        if !self.pointer.events.is_empty() {
            self.next_frame = Some(Instant::now());
        }
        let gap = if self.pointer.at.is_some() || !self.pointer.events.is_empty() { MIN_FRAME } else { IDLE_FRAME };
        let due = self.next_frame.is_some_and(|f| f <= Instant::now()) && self.last_frame.is_none_or(|f| f.elapsed() >= gap);
        if due {
            self.last_frame = Some(Instant::now());
            orbiters_ui::anim::set_live(&self.ctx, !snap.settings.reduce_motion);
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), ui::SIZE)),
                time: Some(self.started.elapsed().as_secs_f64()),
                events: std::mem::take(&mut self.pointer.events),
                ..Default::default()
            };
            input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
            orbiters_ui::fonts::follow_language(&self.ctx);
            let mut full = self.ctx.run_ui(input, |ui| state.show_in(ui, snap, backend, poses, ui::Surface::Floating));
            pressed |= orbiters_ui::widgets::take_press(&self.ctx);
            self.painter.set_textures(&mut full.textures_delta);
            let primitives = self.ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
            self.painter.paint(&primitives, full.pixels_per_point, theme::INK_950);
            if let Some(changed) = self.painter.damage() {
                presenter.show_part(overlay, self.handle, self.painter.pixels(), PX[0], PX[1], changed);
            }
            self.painter.free_textures(&mut full.textures_delta);
            let delay = full.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            self.next_frame = if delay < Duration::from_secs(3600) {
                Some(Instant::now() + delay.max(MIN_FRAME))
            } else if state.busy() {
                Some(Instant::now() + Duration::from_millis(100))
            } else {
                None
            };
            if !self.shown && self.open {
                let _ = overlay.set_alpha(self.handle, 0.0);
                let _ = overlay.set_alpha(self.bar, 0.0);
                self.alpha = 0.0;
                let _ = overlay.show(self.handle);
                let _ = overlay.show(self.bar);
                self.shown = true;
            }
        }
        // Its fade in, or out as it closes (hidden once out).
        if self.shown {
            let target = if self.open { 1.0 } else { 0.0 };
            if snap.settings.reduce_motion {
                self.fade = Follow::at(target);
            } else {
                self.fade.step(target, dt, Spring::SNAPPY);
            }
            let alpha = self.fade.value.clamp(0.0, 1.0);
            if (alpha - self.alpha).abs() >= 0.01 || (alpha == target && alpha != self.alpha) {
                let _ = overlay.set_alpha(self.handle, alpha);
                let _ = overlay.set_alpha(self.bar, alpha);
                self.alpha = alpha;
            }
            if !self.open && alpha <= 0.0 {
                let _ = overlay.hide(self.handle);
                let _ = overlay.hide(self.bar);
                self.shown = false;
                self.fade = Follow::at(0.0);
            }
        }
        pressed
    }

    /// The bar's picture for its button under a laser (`hover`) and the one just pressed, painted when that or the
    /// width changed.
    pub(in crate::overlay) fn paint_bar(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, hover: Option<BarButton>, pressed: Option<BarButton>) {
        let drawn = (hover, pressed, self.width);
        if !self.open || self.bar_drawn == Some(drawn) {
            return;
        }
        let whole = self.bar_drawn.is_none();
        if whole {
            self.bar_canvas.invalidate();
        }
        let width = self.width;
        // (A first paint twice: the icons' font textures load on the first.)
        for _ in 0..if whole { 2 } else { 1 } {
            self.bar_canvas.paint(0.0, |p| paint_bar(p, hover, pressed, width));
        }
        let changed = if whole { Some([0, 0, BAR_PX[0], BAR_PX[1]]) } else { self.bar_canvas.damage() };
        if let Some(changed) = changed {
            presenter.show_part(overlay, self.bar, self.bar_canvas.pixels(), BAR_PX[0], BAR_PX[1], changed);
        }
        self.bar_drawn = Some(drawn);
    }
}

/// The panel's pointer as egui hears it: the events of the turns since its last picture, where it is, its button.
#[derive(Debug, Default)]
struct Pointer {
    events: Vec<Event>,
    at: Option<Pos2>,
    /// The button is down (its end is owed to egui, whatever happens).
    down: bool,
}

impl Pointer {
    /// This turn's pointer: where on the panel (pixels; none: unchanged), its button, whether it left.
    fn update(&mut self, at: Option<Pos2>, button: Option<bool>, gone: bool) {
        if let Some(p) = at {
            if self.at.is_none_or(|q| q.distance(p) >= 0.5) {
                self.events.push(Event::PointerMoved(p));
                self.at = Some(p);
            }
        }
        if let Some(pressed) = button.filter(|&b| b != self.down) {
            let pos = self.at.unwrap_or(pos2(-100.0, -100.0));
            self.events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() });
            self.down = pressed;
        }
        if gone {
            self.let_go();
        }
    }

    /// The pointer goes, its button up first if it was down (egui holds a button down until it hears it go up).
    fn let_go(&mut self) {
        if self.down || self.at.is_some() {
            crate::overlay::let_go(&mut self.events);
            self.down = false;
            self.at = None;
        }
    }
}

/// The bar: a dark pill with smaller and larger on the left, the handle in the middle (as N4N0's screen's, the bar
/// it is moved by), close on the right. A button under a laser lights up; one at its end of the sizes is dim.
pub fn paint_bar(p: &egui::Painter, hover: Option<BarButton>, pressed: Option<BarButton>, width: f32) {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(BAR_PX[0] as f32, BAR_PX[1] as f32));
    p.rect_filled(rect.shrink(2.0), theme::radius(34.0), theme::alpha(theme::INK_900, 0.96));
    p.rect_stroke(rect.shrink(2.0), theme::radius(34.0), Stroke::new(2.0, theme::white(0.1)), egui::StrokeKind::Inside);
    let widths = FloatingSettings::WIDTHS;
    let can = |b: BarButton| match b {
        BarButton::Smaller => width > widths[0] + 1e-3,
        BarButton::Larger => width < widths[widths.len() - 1] - 1e-3,
        _ => true,
    };
    for (b, glyph) in [(BarButton::Smaller, icons::MINUS), (BarButton::Larger, icons::PLUS), (BarButton::Close, icons::X)] {
        let r = b.rect();
        let lit = hover == Some(b) && can(b);
        let down = pressed == Some(b) && can(b);
        let bg = if down {
            theme::white(0.2)
        } else if lit && b == BarButton::Close {
            theme::alpha(theme::DANGER, 0.2)
        } else if lit {
            theme::white(0.12)
        } else {
            theme::white(0.05)
        };
        p.circle_filled(r.center(), r.width() / 2.0 * if down { 0.9 } else { 1.0 }, bg);
        let fg = match (can(b), lit, b) {
            (false, _, _) => theme::FG_4,
            (true, true, BarButton::Close) => theme::DANGER,
            (true, true, _) => theme::FG,
            (true, false, _) => theme::FG_2,
        };
        widgets::icon(p, r.center(), glyph, 26.0, fg);
    }
    // The handle: a bar, lighter under a laser.
    let handle = Rect::from_center_size(BarButton::Handle.rect().center(), vec2(if hover == Some(BarButton::Handle) { 150.0 } else { 136.0 }, 14.0));
    let tone = if hover == Some(BarButton::Handle) { theme::FG } else { theme::FG_3 };
    p.rect_filled(handle, theme::radius(7.0), tone);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_s_buttons_are_where_they_are_drawn() {
        for b in [BarButton::Smaller, BarButton::Larger, BarButton::Handle, BarButton::Close] {
            let c = b.rect().center();
            assert_eq!(BarButton::at([c.x / BAR_PX[0] as f32, c.y / BAR_PX[1] as f32]), Some(b));
        }
        assert_eq!(BarButton::at([0.29, 0.5]), None, "between larger and the handle");
        let h = BarButton::handle_uv();
        assert!(h[0] < 0.5 && h[2] > 0.5 && (h[0] + h[2] - 1.0).abs() < 0.01, "the handle in the middle: mirrored, it is the handle still");
    }

    #[test]
    fn widths_step_and_stop_at_their_ends() {
        let widths = FloatingSettings::WIDTHS;
        assert_eq!(step_width(0.8, 1), 1.0);
        assert_eq!(step_width(0.8, -1), 0.65);
        assert_eq!(step_width(widths[0], -1), widths[0]);
        assert_eq!(step_width(widths[4], 1), widths[4]);
        assert_eq!(step_width(0.83, 0), 0.8, "an odd width goes to the nearest");
    }

    #[test]
    fn the_bar_hangs_under_the_panel() {
        let placed = space::before_eyes(&Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] }, 0.7, 0.12);
        for width in FloatingSettings::WIDTHS {
            let bar = bar_at(&placed, width);
            let down = space::sub(space::position(&bar), space::position(&placed));
            let below = -space::dot(down, space::axis(&placed, 1));
            let bar_height = BAR_WIDTH_M * BAR_PX[1] as f32 / BAR_PX[0] as f32;
            assert!((below - (height(width) / 2.0 + BAR_GAP_M + bar_height / 2.0)).abs() < 1e-5);
            assert!(space::dot(down, space::axis(&placed, 0)).abs() < 1e-5, "centred under it");
        }
    }

    #[test]
    fn it_opens_where_it_was_left_only_before_the_eyes() {
        let head = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] };
        let mut settings = FloatingSettings::default();
        let fresh = opening_place(&head, &settings);
        assert!(space::near(&fresh, &space::before_eyes(&head, OPEN_DISTANCE_M, OPEN_BELOW_M), 1e-5, 1e-6), "nothing kept: before the eyes");
        // Left a little to the left and lower, facing the eyes: it opens there.
        let left = space::facing([-0.3, 1.3, -0.8], [0.0, 1.6, 0.0], [0.0, 1.0, 0.0]).unwrap();
        settings.placement = Some(left.m);
        assert!(space::near(&opening_place(&head, &settings), &left, 1e-4, 1e-5));
        // Left behind the head: before the eyes instead.
        let behind = space::facing([0.0, 1.5, 1.0], [0.0, 1.6, 0.0], [0.0, 1.0, 0.0]).unwrap();
        settings.placement = Some(behind.m);
        assert!(space::near(&opening_place(&head, &settings), &fresh, 1e-5, 1e-6));
        // A place that is no pose (an edited file): before the eyes.
        settings.placement = Some([[0.0; 4]; 3]);
        assert!(space::near(&opening_place(&head, &settings), &fresh, 1e-5, 1e-6));
    }

    #[test]
    fn the_pointer_moves_presses_and_is_let_go() {
        let mut p = Pointer::default();
        p.update(Some(pos2(100.0, 50.0)), None, false);
        p.update(Some(pos2(100.2, 50.0)), Some(true), false);
        assert_eq!(p.events.len(), 2, "a move under half a pixel is no move: {:?}", p.events);
        assert!(matches!(p.events[1], Event::PointerButton { pressed: true, pos, .. } if pos == pos2(100.0, 50.0)));
        // Closed with the button down: its end, then the pointer gone.
        p.let_go();
        assert!(matches!(p.events.last(), Some(Event::PointerGone)));
        assert!(p.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: false, .. })));
        let n = p.events.len();
        p.let_go();
        assert_eq!(p.events.len(), n, "nothing more to let go of");
        // An up without a down is not sent.
        p.update(None, Some(false), false);
        assert_eq!(p.events.len(), n);
    }
}
