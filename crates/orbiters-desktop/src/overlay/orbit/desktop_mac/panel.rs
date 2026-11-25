//! The parts' windows on a Mac: one borderless panel of AppKit's a part, that never brings this app in front and never takes the keyboard, its picture the part's own
//! pixels. Over the game: just above its level while it is in front (its full-screen window is at 25), right above its window and under whatever covers it while another app
//! is. They pass the mouse on except where the person points at them.
//!
//! **No picture of ours ever covers the game's whole window.** macOS stops showing a window that another covers, and the engine then presents nothing: measured with a panel
//! whose every pixel was opaque (2026-11-25), at once, whatever the panel says of itself. [`covered`] is asked before anything shows, and what would cover it is left out.

use super::super::zones::Zone;
use super::game::Game;
use egui::{pos2, vec2, Pos2, Rect, Vec2};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSPanel, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use std::cell::Cell;
use std::ffi::c_void;

type CFTypeRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDataProviderCreateWithCFData(data: CFTypeRef) -> CFTypeRef;
    fn CGColorSpaceCreateWithName(name: &objc2_core_foundation::CFString) -> CFTypeRef;
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(width: usize, height: usize, bits: usize, pixel: usize, row: usize, space: CFTypeRef, info: u32, provider: CFTypeRef, decode: *const f64, interpolate: bool, intent: i32) -> CFTypeRef;
    fn CGCursorIsVisible() -> bool;
    fn CGWarpMouseCursorPosition(at: NSPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(on: bool) -> i32;
    fn CGEventCreate(source: CFTypeRef) -> CFTypeRef;
    fn CGEventGetLocation(event: CFTypeRef) -> NSPoint;
    static kCGColorSpaceSRGB: &'static objc2_core_foundation::CFString;
}

/// Above the game's own level while it is in front: any window of its level or under is under ours.
pub fn level(game: &Game, front: bool) -> isize {
    if front {
        game.window.layer as isize + 1
    } else {
        0
    }
}

/// Whether the rectangles, together, cover the whole of `client`, whatever is clear in them: what macOS would take for a covered window if they were opaque.
pub fn covered(rects: &[Rect], client: Vec2) -> bool {
    let whole = Rect::from_min_size(pos2(0.0, 0.0), client);
    let mut edges: [Vec<f32>; 2] = [vec![whole.min.x, whole.max.x], vec![whole.min.y, whole.max.y]];
    for r in rects {
        for (list, values) in edges.iter_mut().zip([[r.min.x, r.max.x], [r.min.y, r.max.y]]) {
            list.extend(values);
        }
    }
    for (list, (low, high)) in edges.iter_mut().zip([(whole.min.x, whole.max.x), (whole.min.y, whole.max.y)]) {
        list.retain(|v| (low..=high).contains(v));
        list.sort_by(f32::total_cmp);
        list.dedup();
    }
    // Every cell between two edges is wholly in a rectangle or wholly out of it.
    edges[1].windows(2).all(|y| {
        edges[0].windows(2).all(|x| {
            let middle = pos2((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0);
            rects.iter().any(|r| r.contains(middle))
        })
    })
}

/// What of `rects` may show over a game of `client`: as many of them, in their order, as leave some of it in sight.
pub fn uncovering(rects: &[Rect], client: Vec2) -> Vec<bool> {
    let mut kept: Vec<Rect> = Vec::new();
    rects
        .iter()
        .map(|r| {
            kept.push(*r);
            let shows = !covered(&kept, client);
            if !shows {
                kept.pop();
            }
            shows
        })
        .collect()
}

pub struct Ivars {
    /// It may take the keyboard (the one panel that does, [`Hold`]).
    keyable: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSPanel, NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "OrbitersOverlayPanel"]
    #[ivars = Ivars]
    pub struct OverlayPanel;

    impl OverlayPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            self.ivars().keyable.get()
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }

        /// Where it is put is where it lies: half off a display, under the notch or the menu bar.
        #[unsafe(method(constrainFrameRect:toScreen:))]
        fn constrain(&self, frame: NSRect, _screen: *mut AnyObject) -> NSRect {
            frame
        }
    }

    unsafe impl NSObjectProtocol for OverlayPanel {}
);

/// A rectangle of macOS's global coordinates (y down from the main display's top) in AppKit's (y up from its foot).
fn to_appkit(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    let high = crate::bridge::mac::main_display_height();
    NSRect::new(NSPoint::new(x, high - y - h), NSSize::new(w, h))
}

fn make(mtm: MainThreadMarker, keyable: bool, w: f64, h: f64) -> Retained<OverlayPanel> {
    let this = OverlayPanel::alloc(mtm).set_ivars(Ivars { keyable: Cell::new(keyable) });
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    // SAFETY: NSWindow's designated initialiser, on a freshly allocated object, on the main thread.
    let panel: Retained<OverlayPanel> =
        unsafe { msg_send![super(this), initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)), styleMask: style, backing: NSBackingStoreType::Buffered, defer: false] };
    // SAFETY: it is kept and let go by Rust alone.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setHidesOnDeactivate(false);
    panel.setIgnoresMouseEvents(true);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary | NSWindowCollectionBehavior::Stationary | NSWindowCollectionBehavior::IgnoresCycle,
    );
    // No animation of AppKit's as it comes and goes: the parts have their own.
    panel.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::None);
    panel
}

/// The window of a part.
pub struct PanelWindow {
    panel: Retained<OverlayPanel>,
    view: Retained<NSView>,
    px: [u32; 2],
    pixels: Vec<u8>,
    /// New pixels are not yet on it.
    stale: bool,
    shown: bool,
    /// Its corner over the game's picture, and its alpha, as last put.
    at: Option<([i32; 2], u8)>,
    /// The frame it was given.
    frame: Option<NSRect>,
    catches: bool,
}

impl PanelWindow {
    pub fn new(mtm: MainThreadMarker, px: [u32; 2], scale: f64) -> Self {
        let (w, h) = (f64::from(px[0]) / scale, f64::from(px[1]) / scale);
        let panel = make(mtm, false, w, h);
        let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
        view.setWantsLayer(true);
        panel.setContentView(Some(&view));
        Self { panel, view, px, pixels: vec![0; px[0] as usize * px[1] as usize * 4], stale: false, shown: false, at: None, frame: None, catches: false }
    }

    pub fn px(&self) -> [u32; 2] {
        self.px
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// Its number among macOS's windows.
    pub fn number(&self) -> isize {
        self.panel.windowNumber()
    }

    /// New pixels in the part `changed` (x0, y0, x1, y1) of premultiplied RGBA of its size.
    pub fn picture(&mut self, rgba: &[u8], changed: [u32; 4]) {
        let (w, h) = (self.px[0] as usize, self.px[1] as usize);
        if rgba.len() < w * h * 4 {
            return;
        }
        for y in changed[1] as usize..(changed[3] as usize).min(h) {
            let (a, b) = ((y * w + changed[0] as usize) * 4, (y * w + (changed[2] as usize).min(w)) * 4);
            self.pixels[a..b].copy_from_slice(&rgba[a..b]);
        }
        self.stale = true;
    }

    /// Whether the pixel of its picture at `at` (pixels from its corner) is painted: where it is not, the mouse is the game's.
    pub fn painted(&self, at: Pos2) -> bool {
        let (x, y) = (at.x.floor(), at.y.floor());
        if x < 0.0 || y < 0.0 || x >= self.px[0] as f32 || y >= self.px[1] as f32 {
            return false;
        }
        self.pixels[(y as usize * self.px[0] as usize + x as usize) * 4 + 3] > 0
    }

    /// Its pixels: premultiplied RGBA rows of its width, from the top.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Its corner over the game's picture, where it was last put.
    pub fn spot(&self) -> Option<[i32; 2]> {
        self.at.map(|(spot, _)| spot)
    }

    /// Whether it takes the mouse from what is under it.
    pub fn catch_mouse(&mut self, on: bool) {
        if self.catches != on {
            self.catches = on;
            self.panel.setIgnoresMouseEvents(!on);
        }
    }

    pub fn catches(&self) -> bool {
        self.catches
    }

    /// Its pixels to the screen.
    fn upload(&mut self) {
        let (w, h) = (self.px[0] as usize, self.px[1] as usize);
        crate::perf::uploaded((w * h) as u64);
        // SAFETY: Core Graphics' own calls; the data is copied, each object made here is released once, and the layer keeps the picture.
        unsafe {
            let data = CFDataCreate(std::ptr::null(), self.pixels.as_ptr(), self.pixels.len() as isize);
            let provider = CGDataProviderCreateWithCFData(data);
            let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            // Premultiplied RGBA bytes in that order: kCGImageAlphaPremultipliedLast (1), kCGBitmapByteOrder32Big (4 << 12).
            let image = CGImageCreate(w, h, 8, 32, w * 4, space, 1 | (4 << 12), provider, std::ptr::null(), false, 0);
            let layer: Option<Retained<AnyObject>> = msg_send![&self.view, layer];
            if let Some(layer) = layer.filter(|_| !image.is_null()) {
                let () = msg_send![&layer, setContents: image.cast::<AnyObject>()];
            }
            for object in [image, space, provider, data] {
                if !object.is_null() {
                    CFRelease(object);
                }
            }
        }
        self.stale = false;
    }

    /// Puts it at `spot` over the game's picture, with its newest pixels (`ask`: macOS is asked where it really is). Says whether it came into view.
    pub fn place(&mut self, game: &Game, spot: [i32; 2], alpha: u8, ask: bool) -> bool {
        if self.stale {
            self.upload();
        }
        let (w, h) = (f64::from(self.px[0]) / game.scale, f64::from(self.px[1]) / game.scale);
        let (left, top) = game.point(pos2(spot[0] as f32, spot[1] as f32));
        let frame = to_appkit(left, top, w, h);
        let was = if ask || !self.shown { Some(self.panel.frame()) } else { self.frame };
        if was.is_none_or(|was| was.size != frame.size) {
            self.panel.setFrame_display(frame, true);
        } else if was.is_some_and(|was| was.origin != frame.origin) {
            self.panel.setFrameOrigin(frame.origin);
        }
        self.frame = Some(frame);
        if self.at.is_none_or(|(_, was)| was != alpha) {
            self.panel.setAlphaValue(f64::from(alpha) / 255.0);
        }
        self.at = Some((spot, alpha));
        let came = !self.shown;
        self.shown = true;
        came
    }

    pub fn hide(&mut self) {
        if self.shown {
            self.panel.orderOut(None);
            self.shown = false;
            self.catch_mouse(false);
        }
    }
}

impl Drop for PanelWindow {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        self.panel.close();
    }
}

/// Where ours lie: above everything of the game's level while it is in front, else right above its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Above(isize),
    With(u32),
}

/// How many levels above the parts of a menu the part in `zone` lies. **A click on a window raises it above the others of its level** (macOS does, whatever becomes of the
/// click: a page whose head was clicked had its back over it, plain grey, until the page itself was clicked: the owner's first session, 2026-11-25). What lies over one
/// another is at levels of its own, and no click changes that.
fn rank(zone: Zone) -> isize {
    match zone {
        Zone::SheetBack => 1,
        Zone::SheetHint => 2,
        Zone::Takeover => 3,
        Zone::SheetPeek => 4,
        _ => 0,
    }
}

/// How many levels above the first the arrow is.
const ARROW_RANK: isize = 5;

/// Lays the windows `ours` (bottom first) where `layer` says, in that order.
pub fn lay(ours: &[(Zone, &PanelWindow)], layer: Layer) {
    match layer {
        Layer::Above(level) => {
            for (zone, w) in ours {
                w.panel.setLevel(level + rank(*zone));
                w.panel.orderFrontRegardless();
            }
        }
        Layer::With(game) => {
            // Each right above the one before it, from the game's window up: under whatever covers the game. (Nothing of ours takes a click there.)
            let mut under = game as isize;
            for (_, w) in ours {
                w.panel.setLevel(0);
                w.panel.orderWindow_relativeTo(NSWindowOrderingMode::Above, under);
                under = w.panel.windowNumber();
            }
        }
    }
}

/// Whether ours (their numbers, bottom first) lie right above the game's window `game` in the windows on screen (front to back), with no other ordinary window between.
pub fn holds(windows: &[(u32, i64)], game: u32, ours: &[u32]) -> bool {
    let Some(place) = windows.iter().position(|(id, _)| *id == game) else { return true };
    let above: Vec<u32> = windows[..place].iter().rev().filter(|(_, layer)| *layer == 0).map(|(id, _)| *id).take(ours.len()).collect();
    above == ours
}

/// The panel that takes the keyboard, for as long as the person points at the parts: two points of nothing over the game's corner, with which the engine lets the pointer go.
/// It brings no app in front: the game's stays the one in front, at its level.
pub struct Hold {
    panel: Retained<OverlayPanel>,
    held: bool,
}

impl Hold {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let panel = make(mtm, true, 2.0, 2.0);
        panel.setBecomesKeyOnlyIfNeeded(false);
        Self { panel, held: false }
    }

    pub fn held(&self) -> bool {
        self.held
    }

    /// Whether macOS gave it the keyboard, and it still has it.
    pub fn has_keyboard(&self) -> bool {
        self.held && self.panel.isKeyWindow()
    }

    pub fn take(&mut self, game: &Game, level: isize) {
        let (x, y) = game.origin();
        self.panel.setFrame_display(to_appkit(x, y, 2.0, 2.0), false);
        self.panel.setLevel(level);
        self.panel.orderFrontRegardless();
        self.panel.makeKeyWindow();
        self.held = true;
    }

    /// The game has the keyboard again: by itself, the moment this is out.
    pub fn give_back(&mut self) {
        if self.held {
            self.panel.orderOut(None);
            self.held = false;
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        self.panel.close();
    }
}

/// Where the pointer is, in macOS's global coordinates.
pub fn pointer_at() -> (f64, f64) {
    // SAFETY: plain calls; the event made here is released.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return (0.0, 0.0);
        }
        let at = CGEventGetLocation(event);
        CFRelease(event);
        (at.x, at.y)
    }
}

/// Whether macOS shows its pointer now (VRChat hides it and draws its own).
pub fn pointer_shows() -> bool {
    // SAFETY: a plain query.
    unsafe { CGCursorIsVisible() }
}

/// Puts the pointer at `at`. **Only ever while [`Hold::has_keyboard`]**: held by the game, the jump would land in its next movement.
pub fn put_pointer(hold: &Hold, at: (f64, f64)) -> bool {
    if !hold.has_keyboard() {
        return false;
    }
    // SAFETY: plain calls. The association is made again so that the mouse is not held still for a quarter of a second after the jump; the engine makes its own anew as
    // its window has the keyboard again.
    unsafe {
        CGWarpMouseCursorPosition(NSPoint::new(at.0, at.1));
        CGAssociateMouseAndMouseCursorPosition(true);
    }
    true
}

/// The arrow over ours, VRChat's own shape in the accent, drawn while macOS's pointer is hidden.
#[derive(Default)]
pub struct Arrow {
    window: Option<PanelWindow>,
    look: Option<([u8; 3], u32)>,
}

/// Its picture's side, in points.
const ARROW: f64 = 24.0;

impl Arrow {
    pub fn show(&mut self, mtm: MainThreadMarker, game: &Game, at: Pos2, level: isize) {
        let accent = super::super::panels::look::accent();
        let px = (ARROW * game.scale).round() as u32;
        let look = Some(([accent.r(), accent.g(), accent.b()], px));
        if self.look != look {
            self.look = look;
            self.window = None;
        }
        let window = self.window.get_or_insert_with(|| {
            let mut w = PanelWindow::new(mtm, [px, px], game.scale);
            w.picture(&super::super::desk_shared::arrow::picture(px, px as f32 / ARROW as f32, accent), [0, 0, px, px]);
            w
        });
        let tip = super::super::desk_shared::arrow::TIP.to_vec2() * (px as f32 / ARROW as f32);
        let spot = at - tip;
        if window.place(game, [spot.x.round() as i32, spot.y.round() as i32], 255, false) {
            // Above every part's level, and not moved in the order while it shows.
            window.panel.setLevel(level + ARROW_RANK);
            window.panel.orderFrontRegardless();
        }
    }

    pub fn hide(&mut self) {
        if let Some(w) = self.window.as_mut() {
            w.hide();
        }
    }
}

/// Says what a 3D picture is doing while no menu is up: nothing else can then. The mouse passes through it, and it is only there while it has something to say.
#[derive(Default)]
pub struct Pill {
    window: Option<PanelWindow>,
    canvas: Option<crate::overlay::Canvas>,
    words: Option<(String, String)>,
    scale: f64,
}

/// The pill's size, in its units, and how far under the game's top: Windows' own.
const PILL: Vec2 = vec2(560.0, 56.0);
const PILL_TOP: f32 = 96.0;

impl Pill {
    pub fn show(&mut self, mtm: MainThreadMarker, words: Option<(String, String)>, game: &Game, level: isize) {
        let Some(said) = words else { return self.hide() };
        let client = game.client();
        let zoom = (client.y / 720.0).max(0.5);
        let px = [(PILL.x * zoom).ceil() as u32, (PILL.y * zoom).ceil() as u32];
        if self.scale != game.scale || self.window.as_ref().is_none_or(|w| w.px() != px) {
            self.window = Some(PanelWindow::new(mtm, px, game.scale));
            self.canvas = Some(crate::overlay::Canvas::zoomed(px[0], px[1], zoom));
            self.scale = game.scale;
            self.words = None;
        }
        let (Some(window), Some(canvas)) = (self.window.as_mut(), self.canvas.as_mut()) else { return };
        if self.words.as_ref() != Some(&said) {
            let pixels = canvas.paint(0.0, |p| super::super::panels::draw_words(p, Rect::from_min_size(pos2(0.0, 0.0), PILL), &said.0, &said.1));
            window.picture(pixels, [0, 0, px[0], px[1]]);
            self.words = Some(said);
        }
        let spot = [((client.x - px[0] as f32) / 2.0) as i32, (PILL_TOP * zoom) as i32];
        if window.place(game, spot, 255, false) {
            window.panel.setLevel(level);
            window.panel.orderFrontRegardless();
        }
    }

    pub fn hide(&mut self) {
        if let Some(w) = self.window.as_mut() {
            w.hide();
        }
    }
}

/// How much of a rest on a tool has passed ([`super::gesture`]): a ring round the arrow that closes as it does.
#[derive(Default)]
pub struct RestRing {
    window: Option<PanelWindow>,
    look: Option<([u8; 3], u32, u32)>,
}

/// Its size, in points, and in how many steps it closes.
const REST_RING: f64 = 46.0;
const REST_STEPS: f32 = 40.0;

impl RestRing {
    /// Round `at` (pixels of the game's picture), `passed` of it closed; none of it shows none.
    pub fn show(&mut self, mtm: MainThreadMarker, game: &Game, at: Pos2, passed: f32, level: isize) {
        if passed <= 0.0 {
            return self.hide();
        }
        let accent = super::super::panels::look::accent();
        let px = (REST_RING * game.scale).round() as u32;
        let step = (passed.min(1.0) * REST_STEPS).round() as u32;
        let look = ([accent.r(), accent.g(), accent.b()], px, step);
        if self.window.as_ref().is_some_and(|w| w.px() != [px, px]) {
            self.window = None;
        }
        let window = self.window.get_or_insert_with(|| PanelWindow::new(mtm, [px, px], game.scale));
        if self.look != Some(look) {
            self.look = Some(look);
            window.picture(&rest_ring(px, step as f32 / REST_STEPS, accent), [0, 0, px, px]);
        }
        let spot = at - vec2(px as f32, px as f32) / 2.0;
        if window.place(game, [spot.x.round() as i32, spot.y.round() as i32], 255, false) {
            window.panel.setLevel(level + ARROW_RANK);
            window.panel.orderFrontRegardless();
        }
    }

    pub fn hide(&mut self) {
        if let Some(w) = self.window.as_mut() {
            w.hide();
        }
    }
}

/// The ring in a square of `px` pixels, `passed` (0 to 1) of it closed clockwise from the top, RGBA premultiplied.
fn rest_ring(px: u32, passed: f32, colour: egui::Color32) -> Vec<u8> {
    let mut canvas = crate::overlay::Canvas::new(px, px);
    canvas.paint(0.0, |p| {
        let (middle, radius, width) = (pos2(px as f32 / 2.0, px as f32 / 2.0), px as f32 * 0.38, px as f32 * 0.09);
        p.circle_stroke(middle, radius, egui::Stroke::new(width + px as f32 * 0.03, egui::Color32::from_black_alpha(150)));
        let n = (64.0 * passed).ceil().max(1.0) as usize;
        let points: Vec<Pos2> = (0..=n)
            .map(|i| {
                let angle = std::f32::consts::TAU * passed * i as f32 / n as f32;
                middle + vec2(angle.sin(), -angle.cos()) * radius
            })
            .collect();
        p.add(egui::Shape::line(points, egui::Stroke::new(width, colour)));
    });
    canvas.pixels().to_vec()
}

/// Whether a press at `at` (pixels of the game's picture) is on a painted pixel of a part that takes presses.
pub fn part_at<'a>(parts: impl Iterator<Item = (&'a Zone, &'a PanelWindow)>, at: Pos2) -> Option<Zone> {
    parts
        .filter(|(zone, w)| w.shown() && !zone.passes_clicks())
        .find(|(_, w)| w.spot().is_some_and(|s| w.painted(at - vec2(s[0] as f32, s[1] as f32))))
        .map(|(zone, _)| *zone)
}

#[cfg(test)]
mod tests {
    use super::super::super::zones::{self, Around};
    use super::*;

    /// The ring round the arrow closes from the top, clockwise.
    #[test]
    fn the_rest_shows_as_a_ring_that_closes() {
        let px = 92u32;
        let colour = egui::Color32::from_rgb(0, 218, 109);
        let lit = |passed: f32| {
            let pixels = rest_ring(px, passed, colour);
            assert_eq!(pixels.len(), (px * px * 4) as usize);
            (0..24)
                .filter(|i| {
                    let angle = std::f32::consts::TAU * (*i as f32 + 0.5) / 24.0;
                    let at = pos2(46.0, 46.0) + vec2(angle.sin(), -angle.cos()) * (px as f32 * 0.38);
                    let p = (at.y as usize * px as usize + at.x as usize) * 4;
                    pixels[p + 1] > 150 && pixels[p + 3] > 200
                })
                .collect::<Vec<usize>>()
        };
        assert_eq!(lit(0.5), (0..12).collect::<Vec<_>>());
        assert_eq!(lit(0.25), (0..6).collect::<Vec<_>>());
        assert_eq!(lit(1.0).len(), 24);
    }

    #[test]
    fn rectangles_that_leave_a_pixel_in_sight_do_not_cover() {
        let client = vec2(2940.0, 1912.0);
        let whole = Rect::from_min_size(pos2(0.0, 0.0), client);
        assert!(covered(&[whole], client));
        assert!(covered(&[whole.expand(40.0)], client), "past its edges");
        assert!(!covered(&[], client));
        assert!(!covered(&[Rect::from_min_max(pos2(0.0, 0.0), pos2(2939.0, 1912.0))], client));
        let (left, right) = whole.split_left_right_at_fraction(0.4);
        assert!(covered(&[left, right], client), "two that touch");
        assert!(!covered(&[left, right.translate(vec2(1.0, 0.0))], client), "a column of pixels between them");
        let (top, foot) = right.split_top_bottom_at_fraction(0.5);
        assert!(covered(&[left, top, foot], client));
        assert_eq!(uncovering(&[left, top, foot], client), [true, true, false], "the last one would cover it: left out");
        assert_eq!(uncovering(&[whole, left], client), [false, true]);
    }

    /// The parts of every menu, the sheet with everything it shows over them, at every shape a window may have: never the whole of it. (An opaque cover stops the game's
    /// frames: the rule this module keeps.)
    #[test]
    fn the_parts_never_cover_the_game_s_whole_window() {
        for (w, h) in [(2940.0, 1912.0), (1470.0, 956.0), (3840.0, 2160.0), (2560.0, 1080.0), (1280.0, 720.0), (640.0, 360.0), (800.0, 800.0), (900.0, 720.0), (1000.0, 1000.0), (720.0, 1280.0), (320.0, 232.0)] {
            let client = vec2(w, h);
            for menu in [Around::Wheel, Around::QuickMenu] {
                for strip in [false, true] {
                    let mut rects: Vec<Rect> = zones::layout_as(menu, client, strip).iter().map(|p| p.rect).collect();
                    rects.extend([Zone::SheetBack, Zone::Takeover, Zone::SheetHint, Zone::SheetPeek].into_iter().filter_map(|z| zones::place(z, client)).map(|p| p.rect));
                    assert!(!covered(&rects, client), "{client:?} {menu:?} strip {strip}");
                    assert!(uncovering(&rects, client).iter().all(|k| *k), "{client:?} {menu:?}: none is left out");
                }
            }
        }
    }

    /// What lies over one another is at levels of its own: macOS raises a window that is clicked among those of its level (the page's back, whose head has the tabs, came over the
    /// page: plain grey), and no click reorders these. The arrow is over them all.
    #[test]
    fn a_click_puts_no_part_over_the_page() {
        assert!(rank(Zone::WingWorlds) < rank(Zone::SheetBack) && rank(Zone::SheetBack) < rank(Zone::Takeover), "the page over its back, its back over the tiles");
        assert!(rank(Zone::SheetBack) < rank(Zone::SheetHint));
        assert!(rank(Zone::Takeover) < rank(Zone::SheetPeek), "the way back over what it brings back");
        let most = [Zone::SheetBack, Zone::SheetHint, Zone::Takeover, Zone::SheetPeek, Zone::WheelWorlds].map(rank).into_iter().max().unwrap();
        assert!(ARROW_RANK > most);
    }

    #[test]
    fn ours_are_one_above_the_game_in_front_and_with_it_behind_another_app() {
        let window = |layer| crate::bridge::mac::MacWindow { id: 1, owner: 2, x: 0.0, y: 0.0, width: 1470.0, height: 956.0, layer, on_screen: true, alpha: None };
        let game = |layer| Game::of(window(layer), 2.0, Some((0.0, 0.0, 1470.0, 956.0)));
        assert_eq!(level(&game(25), true), 26, "full screen, over the menu bar's level");
        assert_eq!(level(&game(0), true), 1, "in a window");
        assert_eq!(level(&game(0), false), 0);
    }

    /// Front to back: whatever covers the game, ours, the game. Another app's window come between, or ours out of their order: laid again.
    #[test]
    fn ours_stay_right_above_the_game_and_under_what_covers_it() {
        let (menu_bar, cover, game, under) = ((90, 24), (50, 0), (7, 0), (60, 0));
        let ours = [21, 22, 23];
        let list = |ids: &[(u32, i64)]| ids.to_vec();
        assert!(holds(&list(&[menu_bar, cover, (23, 0), (22, 0), (21, 0), game, under]), 7, &ours));
        assert!(holds(&list(&[(23, 0), (22, 0), (21, 0), game]), 7, &ours), "nothing covers it");
        assert!(!holds(&list(&[(23, 0), (22, 0), (21, 0), cover, game]), 7, &ours), "ours over what covers the game");
        assert!(!holds(&list(&[cover, (22, 0), (23, 0), (21, 0), game]), 7, &ours), "out of their order");
        assert!(!holds(&list(&[(23, 0), (22, 0), cover, (21, 0), game]), 7, &ours));
        assert!(holds(&list(&[cover]), 7, &ours), "the game is not listed: nothing to lie over");
        assert!(holds(&list(&[cover, game]), 7, &[]));
    }
}
