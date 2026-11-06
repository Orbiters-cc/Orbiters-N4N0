//! Where VRChat's menus are in its window, measured from the real game on 2026-11-06, and where Orbiters' panels go around them, in the menu's own units so that
//! one set of numbers serves every window size (and, as metres, the headset's plane: [`super::vr`]).
//!
//! **The wheel** scales with the window's height alone. At 1280 x 720, 1920 x 1080, 960 x 540, 1000 x 900, 1000 x 1000, 800 x 800, 900 x 720, 1100 x 800, 1400 x 1050
//! and 1700 x 600 its centre is 0.1556 of the height right of the window's middle and 0.1583 below it, and its radius 0.139 of the height: all ten came out with an
//! unbroken rim (`detect::wheel_seen` on those frames). So 2.46 radii are free below it in any window, and sideways at any shape from square up.
//!
//! **Around the wheel the panels are a second wheel** (Enzo, seeing boxes around it: "more circular [...] so it feels more like an extension of it"): one ring, concentric
//! with VRChat's, its cells cut on the same angles as VRChat's slices (every 30 degrees, which holds its spokes at 30, 90 and 150), each a [`Ring`] of its own.
//!
//! **The Quick Menu** is a canvas that scales with the height down to 16:9 and, narrower than that, somewhat less: `height / 720 * min(1, aspect / (16/9)) ^ 0.28` (fitted
//! on 900 x 720, 1000 x 900, 800 x 800: within 1%), centred on the window's width and at 0.55 of its height. Its units here are pixels of a 1280 x 720 window from that centre.

use egui::{pos2, vec2, Pos2, Rect, Vec2};

/// The two menus with a place around them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Around {
    Wheel,
    QuickMenu,
}

/// The wheel in a window of this client size: its centre and radius, in pixels.
pub fn wheel(client: Vec2) -> (Pos2, f32) {
    let (w, h) = (client.x, client.y);
    (pos2(w / 2.0 + 0.1556 * h, h / 2.0 + 0.1583 * h), 0.139 * h)
}

/// What one Quick Menu unit is in pixels, and where its centre is.
pub fn quick_menu(client: Vec2) -> (Pos2, f32) {
    let (w, h) = (client.x, client.y);
    let aspect = w / h.max(1.0);
    let scale = (h / 720.0) * (aspect / (16.0 / 9.0)).min(1.0).powf(0.28);
    (pos2(w / 2.0, 0.55 * h), scale)
}

/// The Quick Menu's bottom row of icons, in its units from its centre: rocket, bell, pin, camera, sound, chat, gear. They are on every page of it and nowhere else: what
/// tells it from the Main Menu and from anything in a world.
pub const QM_ICONS_Y: f32 = 224.0;
pub const QM_ICONS_X: [f32; 7] = [-132.0, -88.0, -44.0, 0.0, 44.0, 88.0, 132.0];

/// Where the ring starts and ends, in hundredths of the wheel's radius: just off VRChat's rim, and as far as the window allows at any size.
pub const RING_INNER: f32 = 108.0;
pub const RING_OUTER: f32 = 240.0;
/// What separates two cells, and the cells' corners, in those units.
const RING_GAP: f32 = 3.5;
const RING_CORNER: f32 = 10.0;

/// A cell of the ring: from and to, in degrees clockwise from straight up, and its radii.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub from: f32,
    pub to: f32,
    pub inner: f32,
    pub outer: f32,
}

/// A point at a radius and an angle (degrees clockwise from straight up), y down.
pub fn polar(radius: f32, degrees: f32) -> Vec2 {
    let (s, c) = degrees.to_radians().sin_cos();
    vec2(radius * s, -radius * c)
}

impl Ring {
    const fn cell(from: f32, to: f32) -> Self {
        Self { from, to, inner: RING_INNER, outer: RING_OUTER }
    }

    pub fn middle(&self) -> f32 {
        (self.from + self.to) / 2.0
    }

    /// Where its content centres: a little outside its middle radius, where it is wider.
    pub fn anchor(&self) -> Vec2 {
        polar(self.inner + (self.outer - self.inner) * 0.54, self.middle())
    }

    /// How wide it is across, at its anchor.
    pub fn room(&self) -> f32 {
        let radius = self.inner + (self.outer - self.inner) * 0.54;
        2.0 * radius * ((self.to - self.from) / 2.0).to_radians().sin() - RING_GAP - 10.0
    }

    /// The rectangle holding it.
    pub fn bounds(&self) -> Rect {
        let mut points = vec![polar(self.inner, self.from), polar(self.inner, self.to), polar(self.outer, self.from), polar(self.outer, self.to)];
        let mut at = (self.from / 90.0).ceil() * 90.0;
        while at <= self.to {
            points.push(polar(self.outer, at));
            points.push(polar(self.inner, at));
            at += 90.0;
        }
        let mut rect = Rect::NOTHING;
        for p in points {
            rect.extend_with(p.to_pos2());
        }
        rect
    }

    /// How much of a pixel at `p` (from the wheel's centre, in units) is on the cell: 0 to 1 over `soft` units at its edge, with rounded corners and a gap to its
    /// neighbours of the same width all along.
    pub fn coverage(&self, p: Vec2, soft: f32) -> f32 {
        let r = p.length();
        let angle = p.x.atan2(-p.y).to_degrees();
        let half = (self.to - self.from) / 2.0;
        let off = (angle - self.middle() + 540.0).rem_euclid(360.0) - 180.0;
        let along = r * (half - off.abs()).to_radians().sin() - RING_GAP / 2.0;
        let across = (r - self.inner).min(self.outer - r);
        let (x, y) = (RING_CORNER - across, RING_CORNER - along);
        let distance = if x > 0.0 && y > 0.0 { RING_CORNER - (x * x + y * y).sqrt() } else { across.min(along) };
        (distance / soft + 0.5).clamp(0.0, 1.0)
    }

    pub fn contains(&self, p: Vec2) -> bool {
        self.coverage(p, 0.001) >= 0.5
    }
}

/// The panels. Those around the wheel are the ring's cells, clockwise from straight up after the two 3D pictures which meet there; those around the Quick Menu are named
/// for the yellow zones Enzo drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// The largest cells, on top, where the eyes go first: a 3D picture of people, of the world.
    WheelPeople,
    WheelWorld,
    WheelRecord,
    /// Fast walk in VR, YouTube in desktop mode (where there is no fast walk).
    WheelWalk,
    WheelMusic,
    WheelTalk,
    /// All of N4N0's menu.
    WheelMore,
    /// Recent captures, the newest beside the 3D pictures.
    WheelRecent0,
    WheelRecent1,
    WheelRecent2,
    /// What is pointed at, in words, above the ring. Nothing presses here.
    WheelCaption,
    /// In VR only: the panels' close, for when VRChat's wheel closed without its button.
    WheelHide,
    /// The tall band left of the Quick Menu: recent captures.
    QmLeft,
    /// The tall band right of it: N4N0's pages.
    QmRight,
    /// Over it: what Orbiters is doing, and recording.
    QmAbove,
    /// Under its corners: a 3D picture of people, of the world.
    QmLowerLeft,
    QmLowerRight,
    /// Under its row of icons: a strip of single actions.
    QmUnder,
    /// What is pointed at, in words, over all that.
    QmCaption,
    /// In front of the Quick Menu's own surface, when more room is needed. Input to VRChat is blocked while pointing here.
    Takeover,
}

impl Zone {
    pub const WHEEL: [Zone; 11] = [
        Zone::WheelPeople,
        Zone::WheelWorld,
        Zone::WheelRecord,
        Zone::WheelWalk,
        Zone::WheelMusic,
        Zone::WheelTalk,
        Zone::WheelMore,
        Zone::WheelRecent2,
        Zone::WheelRecent1,
        Zone::WheelRecent0,
        Zone::WheelCaption,
    ];
    pub const QUICK_MENU: [Zone; 7] = [Zone::QmLeft, Zone::QmRight, Zone::QmAbove, Zone::QmLowerLeft, Zone::QmLowerRight, Zone::QmUnder, Zone::QmCaption];

    /// The zones around a menu.
    pub fn around(around: Around) -> &'static [Zone] {
        match around {
            Around::Wheel => &Self::WHEEL,
            Around::QuickMenu => &Self::QUICK_MENU,
        }
    }

    /// Which menu it is around.
    pub fn menu(self) -> Around {
        if Self::WHEEL.contains(&self) || self == Zone::WheelHide {
            Around::Wheel
        } else {
            Around::QuickMenu
        }
    }

    /// Its cell, when it is one of the ring's.
    pub fn ring(self) -> Option<Ring> {
        Some(match self {
            Zone::WheelPeople => Ring::cell(-60.0, 0.0),
            Zone::WheelWorld => Ring::cell(0.0, 60.0),
            Zone::WheelRecord => Ring::cell(60.0, 90.0),
            Zone::WheelWalk => Ring::cell(90.0, 120.0),
            Zone::WheelMusic => Ring::cell(120.0, 150.0),
            Zone::WheelTalk => Ring::cell(150.0, 180.0),
            Zone::WheelMore => Ring::cell(180.0, 210.0),
            Zone::WheelRecent2 => Ring::cell(210.0, 240.0),
            Zone::WheelRecent1 => Ring::cell(240.0, 270.0),
            Zone::WheelRecent0 => Ring::cell(270.0, 300.0),
            _ => return None,
        })
    }

    /// Nothing is pressed here: the pointer and a click pass through to what is under it.
    pub fn passes_clicks(self) -> bool {
        matches!(self, Zone::WheelCaption | Zone::QmCaption)
    }

    /// Its place in the menu's units from the menu's centre, y down: around the wheel in hundredths of its radius (a 1280 x 720 window's pixels, nearly), around the Quick
    /// Menu in its units.
    pub fn units(self) -> Rect {
        if let Some(ring) = self.ring() {
            return ring.bounds();
        }
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        match self {
            Zone::WheelCaption => r(-200.0, -306.0, 200.0, -248.0),
            Zone::WheelHide => r(182.0, -226.0, 212.0, -196.0),
            Zone::QmLeft => r(-470.0, -190.0, -362.0, 170.0),
            Zone::QmRight => r(362.0, -190.0, 470.0, 170.0),
            Zone::QmAbove => r(-86.0, -296.0, 188.0, -242.0),
            Zone::QmLowerLeft => r(-362.0, 196.0, -250.0, 316.0),
            Zone::QmLowerRight => r(250.0, 196.0, 362.0, 316.0),
            Zone::QmUnder => r(-190.0, 248.0, 190.0, 316.0),
            Zone::QmCaption => r(-86.0, -356.0, 300.0, -302.0),
            // As wide as the Quick Menu with its wings, N4N0's menu's own shape.
            Zone::Takeover => r(-351.0, -198.0, 351.0, 197.0),
            _ => unreachable!("the ring's cells return above"),
        }
    }

    /// The smallest it can be (in units) and still hold what it holds: cut further than this by the window's edge, it does not show. A cell of the ring, cut, would no
    /// longer be one: it shows whole or not at all.
    pub fn smallest(self) -> Vec2 {
        let u = self.units().size();
        if self.ring().is_some() {
            u
        } else {
            u * 0.72
        }
    }
}

/// What a panel keeps clear of the window's edge, in pixels.
const EDGE: f32 = 4.0;

/// A panel placed in the window: where in pixels, the menu's centre and how many pixels a unit is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub zone: Zone,
    pub rect: Rect,
    pub center: Pos2,
    pub unit: f32,
}

impl Placed {
    /// Its size in units.
    pub fn logical(&self) -> Vec2 {
        self.rect.size() / self.unit
    }

    /// Whether the pixel at `p` (window pixels) is on the panel: inside its shape, and not what only shows words.
    pub fn contains(&self, p: Pos2) -> bool {
        if self.zone.passes_clicks() {
            return false;
        }
        match self.zone.ring() {
            Some(ring) => ring.contains((p - self.center) / self.unit),
            None => self.rect.contains(p),
        }
    }
}

/// The menu's centre and how many pixels a unit is.
pub fn frame(around: Around, client: Vec2) -> (Pos2, f32) {
    match around {
        Around::Wheel => {
            let (center, radius) = wheel(client);
            (center, radius / 100.0)
        }
        Around::QuickMenu => quick_menu(client),
    }
}

/// One zone in a window: `None` when the window's edge leaves too little of it.
pub fn place(zone: Zone, client: Vec2) -> Option<Placed> {
    let (center, unit) = frame(zone.menu(), client);
    let units = zone.units();
    let rect = Rect::from_min_max(center + units.min.to_vec2() * unit, center + units.max.to_vec2() * unit);
    let window = Rect::from_min_size(pos2(0.0, 0.0), client).shrink(EDGE);
    let cut = rect.intersect(window);
    let smallest = zone.smallest() * unit;
    if !cut.is_positive() || cut.width() + 0.5 < smallest.x || cut.height() + 0.5 < smallest.y {
        return None;
    }
    // Whole pixels: a layered window has no other, and text stays crisp.
    let rect = Rect::from_min_max(cut.min.round(), cut.max.round());
    Some(Placed { zone, rect, center, unit })
}

/// A zone in its menu's own units, no window to fit in: in the headset, where a plane says what a unit is ([`super::vr`]).
pub fn in_units(zone: Zone) -> Placed {
    Placed { zone, rect: zone.units(), center: pos2(0.0, 0.0), unit: 1.0 }
}

/// Every zone that fits around a menu.
pub fn layout(around: Around, client: Vec2) -> Vec<Placed> {
    Zone::around(around).iter().filter_map(|z| place(*z, client)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window sizes the real game was measured in.
    const SIZES: [(f32, f32); 10] = [
        (1280.0, 720.0),
        (1920.0, 1080.0),
        (960.0, 540.0),
        (1000.0, 900.0),
        (1000.0, 1000.0),
        (800.0, 800.0),
        (900.0, 720.0),
        (1100.0, 800.0),
        (1400.0, 1050.0),
        (1700.0, 600.0),
    ];

    #[test]
    fn the_menus_are_where_the_game_drew_them() {
        // (window, wheel's centre, its radius), read off the real frames.
        for ((w, h), (cx, cy), r) in [((1280.0, 720.0), (752.0, 474.0), 100.0), ((1920.0, 1080.0), (1128.0, 711.0), 150.0), ((1000.0, 900.0), (640.0, 592.0), 125.0), ((1700.0, 600.0), (943.0, 395.0), 83.0)] {
            let (c, radius) = wheel(vec2(w, h));
            assert!((c.x - cx).abs() < 3.0 && (c.y - cy).abs() < 3.0 && (radius - r).abs() < 2.5, "{w}x{h}: {c:?} {radius}");
        }
        // (window, the Quick Menu's main panel's width in pixels).
        for ((w, h), width) in [((1280.0, 720.0), 376.0), ((1000.0, 900.0), 412.0), ((800.0, 800.0), 360.0), ((900.0, 720.0), 338.0), ((1700.0, 600.0), 312.0)] {
            let (_, unit) = quick_menu(vec2(w, h));
            assert!((376.0 * unit - width).abs() < 8.0, "{w}x{h}: {}", 376.0 * unit);
        }
    }

    #[test]
    fn the_ring_goes_all_the_way_round_with_nothing_twice() {
        let cells: Vec<Ring> = Zone::WHEEL.iter().filter_map(|z| z.ring()).collect();
        let covered: f32 = cells.iter().map(|c| c.to - c.from).sum();
        assert_eq!(covered, 360.0, "a whole ring");
        // Every angle at the ring's middle is on one cell, or in a gap between two.
        for t in 0..1440 {
            let p = polar((RING_INNER + RING_OUTER) / 2.0, t as f32 / 4.0);
            let on = cells.iter().filter(|c| c.contains(p)).count();
            assert!(on <= 1, "two cells at {t}");
        }
        // VRChat's spokes (at 30, 90 and 150, each way) run on into its gaps, or into the middle of a double cell.
        for spoke in [30.0f32, 90.0, 150.0, 210.0, 270.0, 330.0] {
            let p = polar(180.0, spoke);
            let on_a_gap = cells.iter().all(|c| !c.contains(p));
            let in_a_cell_cut_there = cells.iter().any(|c| c.contains(p) && (c.to - c.from) > 30.0);
            assert!(on_a_gap || in_a_cell_cut_there, "{spoke}");
        }
        for (w, h) in SIZES {
            let (center, radius) = wheel(vec2(w, h));
            for placed in layout(Around::Wheel, vec2(w, h)) {
                for t in 0..720 {
                    let on_rim = center + polar(radius * 1.03, t as f32 / 2.0);
                    assert!(!placed.contains(on_rim), "{w}x{h}: {:?} covers the wheel's rim at {t}", placed.zone);
                }
            }
        }
    }

    #[test]
    fn a_cell_has_a_soft_edge_rounded_corners_and_keeps_to_its_shape() {
        let cell = Zone::WheelRecord.ring().unwrap();
        let middle = polar((cell.inner + cell.outer) / 2.0, cell.middle());
        assert_eq!(cell.coverage(middle, 1.0), 1.0);
        assert_eq!(cell.coverage(polar(cell.outer + 5.0, cell.middle()), 1.0), 0.0, "past its outer edge");
        assert_eq!(cell.coverage(polar(cell.inner - 5.0, cell.middle()), 1.0), 0.0, "on the wheel's side");
        // Its very corner is rounded away.
        assert_eq!(cell.coverage(polar(cell.outer - 0.5, cell.from + 0.8), 1.0), 0.0);
        // The edge is soft over a pixel.
        let edge = cell.coverage(polar(cell.outer, cell.middle()), 1.0);
        assert!((edge - 0.5).abs() < 0.01, "{edge}");
        // Its bounds hold it, across an axis too.
        let across = Zone::WheelMore.ring().unwrap();
        let bounds = across.bounds();
        for t in 0..60 {
            for r in [across.inner, across.outer] {
                let at = polar(r, across.from + t as f32 / 2.0).to_pos2();
                assert!(bounds.expand(0.01).contains(at));
            }
        }
    }

    #[test]
    fn the_quick_menu_panels_stay_off_it_and_off_each_other() {
        for (w, h) in SIZES {
            let placed = layout(Around::QuickMenu, vec2(w, h));
            for (i, a) in placed.iter().enumerate() {
                for b in &placed[i + 1..] {
                    assert!(!a.rect.shrink(1.0).intersects(b.rect.shrink(1.0)), "{w}x{h}: {:?} and {:?}", a.zone, b.zone);
                }
                assert!(a.rect.left() >= 0.0 && a.rect.top() >= 0.0 && a.rect.right() <= w && a.rect.bottom() <= h, "{w}x{h}: {:?} leaves the window", a.zone);
            }
        }
        // Off its main panel, its wings, its row of icons and its safety and microphone buttons.
        let keep_off = [
            Rect::from_min_max(pos2(-188.0, -204.0), pos2(188.0, 204.0)),
            Rect::from_min_max(pos2(-351.0, -150.0), pos2(351.0, 186.0)),
            Rect::from_min_max(pos2(-242.0, 204.0), pos2(242.0, 242.0)),
        ];
        for z in Zone::QUICK_MENU {
            for k in keep_off {
                assert!(!z.units().intersects(k), "{z:?} on {k:?}");
            }
        }
    }

    #[test]
    fn a_common_window_shows_every_panel_and_a_narrow_one_drops_what_does_not_fit() {
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0), (960.0, 540.0), (1700.0, 600.0), (1000.0, 900.0), (800.0, 800.0)] {
            assert_eq!(layout(Around::Wheel, vec2(w, h)).len(), Zone::WHEEL.len(), "the whole ring at {w}x{h}");
        }
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0), (960.0, 540.0)] {
            assert_eq!(layout(Around::QuickMenu, vec2(w, h)).len(), Zone::QUICK_MENU.len(), "{w}x{h}");
        }
        let narrow = layout(Around::QuickMenu, vec2(800.0, 800.0));
        assert!(!narrow.iter().any(|p| p.zone == Zone::QmLeft), "the bands have no room beside the wings");
        assert!(narrow.iter().any(|p| p.zone == Zone::QmUnder));
        assert!(place(Zone::Takeover, vec2(1280.0, 720.0)).is_some() && place(Zone::Takeover, vec2(800.0, 800.0)).is_some());
        // Narrower than square, a cell cut by the window's edge does not show at all.
        let tall = layout(Around::Wheel, vec2(560.0, 800.0));
        assert!(tall.len() < Zone::WHEEL.len());
        for p in tall {
            assert!((p.logical() - p.zone.units().size()).length() < 2.0 || p.zone.ring().is_none(), "{:?}", p.zone);
        }
    }

    #[test]
    fn panels_scale_with_the_menu() {
        let small = place(Zone::WheelPeople, vec2(960.0, 540.0)).unwrap();
        let large = place(Zone::WheelPeople, vec2(1920.0, 1080.0)).unwrap();
        assert!((large.rect.width() / small.rect.width() - 2.0).abs() < 0.02);
        assert!((large.logical() - small.logical()).length() < 1.5, "the same drawing");
    }

    #[test]
    fn a_click_is_on_a_cell_s_shape_not_on_its_rectangle_and_never_on_a_caption() {
        let p = place(Zone::WheelMusic, vec2(1280.0, 720.0)).unwrap();
        // A corner of its rectangle, off its arc.
        let corners = [p.rect.left_top(), p.rect.right_top(), p.rect.left_bottom(), p.rect.right_bottom()];
        assert!(corners.iter().any(|c| !p.contains(*c)));
        let anchor = p.center + p.zone.ring().unwrap().anchor() * p.unit;
        assert!(p.contains(anchor));
        let caption = place(Zone::WheelCaption, vec2(1280.0, 720.0)).unwrap();
        assert!(!caption.contains(caption.rect.center()));
    }
}
