//! Where N4N0's bar sits in the window's title strip, and the chat it grows into (REQUEST21 3).
//!
//! The strip keeps its jobs: the app's name and BETA chip at the left (after a Mac's three buttons), the window buttons
//! (and a recording's Stop) at the right, and everything else of it drags the window. The bar takes the middle: centred
//! on the window, as wide as the window allows up to [`BAR_MAX`], never nearer than [`GAP`] to the name or the buttons.
//! Where a centred bar of [`BAR_MIN`] no longer fits, it slides off-centre into the room there is; where not even that
//! fits, it becomes a round button ([`ICON`]) that opens the same chat.

use egui::{pos2, vec2, Pos2, Rect};

/// The bar's height and its top in the strip.
pub const BAR_H: f32 = 28.0;
pub const BAR_TOP: f32 = 6.0;
/// Its widest and narrowest; the narrowest fits the placeholder, the microphone and the shortcut.
pub const BAR_MAX: f32 = 520.0;
pub const BAR_MIN: f32 = 220.0;
/// Points kept clear between the bar and the name, and the bar and the buttons.
pub const GAP: f32 = 16.0;
/// The button it becomes in a narrow strip.
pub const ICON: f32 = 32.0;
/// The bar's corner radius (a pill) and the open chat's.
pub const BAR_RADIUS: f32 = BAR_H / 2.0;
pub const PANEL_RADIUS: f32 = 22.0;
/// The open chat's widest, and its margin to the window's sides and bottom.
pub const PANEL_MAX_W: f32 = 760.0;
pub const PANEL_MAX_H: f32 = 860.0;
pub const PANEL_MARGIN: f32 = 24.0;

/// What the strip shows of N4N0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bar {
    /// The bar, with room for its text.
    Full(Rect),
    /// The round button of a narrow strip.
    Icon(Rect),
}

impl Bar {
    pub fn rect(&self) -> Rect {
        match self {
            Bar::Full(r) | Bar::Icon(r) => *r,
        }
    }
}

/// What a point of the strip does (for the tests of what each part of it takes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Drags the window (a double press maximises it).
    Drag,
    /// N4N0's bar (or its button).
    Bar,
    /// The name and its chip, the window buttons or a recording's Stop.
    Kept,
    /// Not in the strip.
    Outside,
}

/// The strip of a window `window` wide: the name ends at `left_end`, the buttons start at `right_start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub window: Rect,
    pub title_h: f32,
    pub left_end: f32,
    pub right_start: f32,
    pub bar: Bar,
}

impl Strip {
    pub fn new(window: Rect, title_h: f32, left_end: f32, right_start: f32) -> Self {
        Self { window, title_h, left_end, right_start, bar: place(window, left_end, right_start) }
    }

    /// What a press at `p` reaches.
    pub fn hit(&self, p: Pos2) -> Hit {
        let strip = Rect::from_min_size(self.window.min, vec2(self.window.width(), self.title_h));
        if !strip.contains(p) {
            Hit::Outside
        } else if self.bar.rect().contains(p) {
            Hit::Bar
        } else if p.x <= self.left_end || p.x >= self.right_start {
            Hit::Kept
        } else {
            Hit::Drag
        }
    }

    /// The left end of the bar (or its button): what is drawn after the name (the world's name) stops before it.
    pub fn free_until(&self) -> f32 {
        self.bar.rect().left() - GAP
    }
}

/// The bar's place (see the module).
pub fn place(window: Rect, left_end: f32, right_start: f32) -> Bar {
    let top = window.top() + BAR_TOP;
    let (from, to) = (left_end + GAP, right_start - GAP);
    let cx = window.center().x;
    let wanted = (window.width() * 0.4).clamp(BAR_MIN + 60.0, BAR_MAX);
    let half = (cx - from).min(to - cx);
    if 2.0 * half >= BAR_MIN {
        let w = wanted.min(2.0 * half);
        return Bar::Full(Rect::from_min_size(pos2(cx - w / 2.0, top), vec2(w, BAR_H)));
    }
    let room = to - from;
    if room >= BAR_MIN {
        // As near the middle as the room lets it.
        let w = wanted.min(room);
        let left = (cx - w / 2.0).clamp(from, to - w);
        return Bar::Full(Rect::from_min_size(pos2(left, top), vec2(w, BAR_H)));
    }
    let x = cx.clamp(from + ICON / 2.0, (to - ICON / 2.0).max(from + ICON / 2.0));
    Bar::Icon(Rect::from_center_size(pos2(x, top + BAR_H / 2.0), vec2(ICON, BAR_H)))
}

/// The open chat in a window: centred under the strip's middle, its top where the bar's is, as wide as fits up to
/// [`PANEL_MAX_W`], most of the window's height.
pub fn panel(window: Rect) -> Rect {
    let w = (window.width() - 2.0 * PANEL_MARGIN).min(PANEL_MAX_W);
    let top = window.top() + BAR_TOP;
    let h = (window.height() * 0.82).clamp(360.0, PANEL_MAX_H).min(window.bottom() - PANEL_MARGIN - top);
    Rect::from_min_size(pos2(window.center().x - w / 2.0, top), vec2(w, h))
}

/// The one shape between the bar (`t` 0) and the chat (`t` 1): its rect and corner radius. It widens a little ahead of
/// growing down, as a sheet unfolds; past 1 (the spring settling) both keep going, by the same small amount.
pub fn shape(bar: Rect, panel: Rect, t: f32) -> (Rect, f32) {
    let across = if t < 1.0 { 1.0 - (1.0 - t.max(0.0)).powf(1.6) } else { t };
    let down = t.max(0.0);
    let lerp = |a: f32, b: f32, k: f32| a + (b - a) * k;
    let rect = Rect::from_min_max(
        pos2(lerp(bar.left(), panel.left(), across), lerp(bar.top(), panel.top(), down)),
        pos2(lerp(bar.right(), panel.right(), across), lerp(bar.bottom(), panel.bottom(), down)),
    );
    let radius = lerp(bar.height().min(bar.width()) / 2.0, PANEL_RADIUS, t.clamp(0.0, 1.0)).min(rect.height() / 2.0).min(rect.width() / 2.0);
    (rect, radius)
}

/// The text field's rect between the bar's (`t` 0) and the composer's (`t` 1): it rides from the bar down to the chat's
/// foot as the shape grows, so what is typed never leaves the screen.
pub fn field(in_bar: Rect, in_composer: Rect, t: f32) -> Rect {
    let k = t.clamp(0.0, 1.0);
    let k = k * k * (3.0 - 2.0 * k);
    Rect::from_min_max(in_bar.min + (in_composer.min - in_bar.min) * k, in_bar.max + (in_composer.max - in_bar.max) * k)
}
