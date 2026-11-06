//! Where VRChat's menus are in its window, measured from the real game on 2026-11-06, and where Orbiters' panels go around
//! them: the zones Enzo drew, in the menu's own units so that one set of numbers serves every window size (and, as metres,
//! the headset's plane: [`super::vr`]).
//!
//! **The wheel** scales with the window's height alone. At 1280 x 720, 1920 x 1080, 960 x 540, 1000 x 900, 1000 x 1000,
//! 800 x 800, 900 x 720, 1100 x 800, 1400 x 1050 and 1700 x 600 its centre is 0.1556 of the height right of the window's
//! middle and 0.1583 below it, and its radius 0.139 of the height: all ten came out with an unbroken rim
//! (`detect::wheel_seen` on those frames).
//!
//! **The Quick Menu** is a canvas that scales with the height down to 16:9 and, narrower than that, somewhat less:
//! `height / 720 * min(1, aspect / (16/9)) ^ 0.28` (fitted on 900 x 720, 1000 x 900, 800 x 800: within 1%), centred on the
//! window's width and at 0.55 of its height. Its units here are pixels of a 1280 x 720 window from that centre.

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

/// The Quick Menu's bottom row of icons, in its units from its centre: rocket, bell, pin, camera, sound, chat, gear. They
/// are on every page of it and nowhere else: what tells it from the Main Menu and from anything in a world.
pub const QM_ICONS_Y: f32 = 224.0;
pub const QM_ICONS_X: [f32; 7] = [-132.0, -88.0, -44.0, 0.0, 44.0, 88.0, 132.0];

/// The panels. Those around the wheel are named for what Enzo's drawing shows, as seen from the wheel; those around the
/// Quick Menu for the yellow zones of his.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// The large one over the wheel: 3D pictures.
    WheelTop,
    /// The large one left of it: recent captures.
    WheelLeft,
    /// The large one at the lower right: N4N0's pages.
    WheelLowerRight,
    /// The small ones: one action each.
    WheelTopCorner,
    WheelRightCorner,
    WheelRight,
    WheelBelow,
    WheelBelowLeft,
    WheelFarLeft,
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
    /// In front of the Quick Menu's own surface, when more room is needed. Input to VRChat is blocked while pointing here.
    Takeover,
}

impl Zone {
    pub const WHEEL: [Zone; 9] = [
        Zone::WheelTop,
        Zone::WheelLeft,
        Zone::WheelLowerRight,
        Zone::WheelTopCorner,
        Zone::WheelRightCorner,
        Zone::WheelRight,
        Zone::WheelBelow,
        Zone::WheelBelowLeft,
        Zone::WheelFarLeft,
    ];
    pub const QUICK_MENU: [Zone; 6] = [Zone::QmLeft, Zone::QmRight, Zone::QmAbove, Zone::QmLowerLeft, Zone::QmLowerRight, Zone::QmUnder];

    /// The zones around a menu.
    pub fn around(around: Around) -> &'static [Zone] {
        match around {
            Around::Wheel => &Self::WHEEL,
            Around::QuickMenu => &Self::QUICK_MENU,
        }
    }

    /// Which menu it is around.
    pub fn menu(self) -> Around {
        if Self::WHEEL.contains(&self) {
            Around::Wheel
        } else {
            Around::QuickMenu
        }
    }

    /// Its place in the menu's units from the menu's centre, y down: around the wheel in hundredths of its radius (a
    /// 1280 x 720 window's pixels, nearly), around the Quick Menu in its units. From the drawing, with the gaps evened out.
    pub fn units(self) -> Rect {
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        match self {
            Zone::WheelTop => r(-83.0, -248.0, 174.0, -82.0),
            Zone::WheelLeft => r(-226.0, -166.0, -88.0, 119.0),
            Zone::WheelLowerRight => r(62.0, 8.0, 251.0, 187.0),
            Zone::WheelTopCorner => r(-158.0, -246.0, -96.0, -180.0),
            Zone::WheelRightCorner => r(186.0, -220.0, 248.0, -152.0),
            Zone::WheelRight => r(130.0, -72.0, 212.0, -6.0),
            Zone::WheelBelow => r(-42.0, 129.0, 50.0, 229.0),
            Zone::WheelBelowLeft => r(-146.0, 129.0, -52.0, 212.0),
            Zone::WheelFarLeft => r(-275.0, 129.0, -156.0, 205.0),
            Zone::QmLeft => r(-470.0, -190.0, -362.0, 170.0),
            Zone::QmRight => r(362.0, -190.0, 470.0, 170.0),
            Zone::QmAbove => r(-86.0, -296.0, 188.0, -242.0),
            Zone::QmLowerLeft => r(-362.0, 196.0, -250.0, 316.0),
            Zone::QmLowerRight => r(250.0, 196.0, 362.0, 316.0),
            Zone::QmUnder => r(-190.0, 248.0, 190.0, 316.0),
            // As wide as the Quick Menu with its wings, N4N0's menu's own shape.
            Zone::Takeover => r(-351.0, -198.0, 351.0, 197.0),
        }
    }

    /// The panel's edge follows the wheel's where it comes this near its centre, in hundredths of its radius: the zones
    /// hugging the wheel are drawn with a concave side. `None`: a plain rectangle.
    pub fn bite(self) -> Option<f32> {
        match self {
            Zone::WheelTop | Zone::WheelLeft | Zone::WheelLowerRight => Some(BITE),
            _ => None,
        }
    }

    /// The smallest it can be (in units) and still hold what it holds: cut further than this by the window's edge, it does
    /// not show.
    pub fn smallest(self) -> Vec2 {
        let u = self.units().size();
        vec2((u.x * 0.72).min(u.x), (u.y * 0.72).min(u.y))
    }
}

/// How far from the wheel's centre its neighbours keep, in hundredths of its radius.
pub const BITE: f32 = 110.0;
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

    /// Whether the pixel at `p` (window pixels) is on the panel: inside its rectangle and out of the wheel's bite.
    pub fn contains(&self, p: Pos2) -> bool {
        self.rect.contains(p) && self.zone.bite().is_none_or(|b| (p - self.center).length() >= b * self.unit)
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
    if !cut.is_positive() || cut.width() < smallest.x || cut.height() < smallest.y {
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
    fn no_panel_overlaps_another_or_the_wheel_or_the_quick_menu() {
        for around in [Around::Wheel, Around::QuickMenu] {
            for (w, h) in SIZES {
                let placed = layout(around, vec2(w, h));
                for (i, a) in placed.iter().enumerate() {
                    for b in &placed[i + 1..] {
                        assert!(!a.rect.shrink(1.0).intersects(b.rect.shrink(1.0)), "{w}x{h}: {:?} and {:?}", a.zone, b.zone);
                    }
                    assert!(a.rect.left() >= 0.0 && a.rect.top() >= 0.0 && a.rect.right() <= w && a.rect.bottom() <= h, "{w}x{h}: {:?} leaves the window", a.zone);
                }
            }
        }
        // The rectangles without a bite stay off the wheel entirely; the others keep off it by their bite.
        let (center, radius) = wheel(vec2(1280.0, 720.0));
        for placed in layout(Around::Wheel, vec2(1280.0, 720.0)) {
            for t in 0..720 {
                let (s, c) = (t as f32 / 2.0).to_radians().sin_cos();
                let on_rim = center + vec2(c, s) * radius * 1.03;
                assert!(!placed.contains(on_rim), "{:?} covers the wheel's rim at {t}", placed.zone);
            }
        }
        // Around the Quick Menu: off its main panel, its wings, its row of icons and its safety and microphone buttons.
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
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0), (960.0, 540.0)] {
            assert_eq!(layout(Around::Wheel, vec2(w, h)).len(), Zone::WHEEL.len(), "{w}x{h}");
            assert_eq!(layout(Around::QuickMenu, vec2(w, h)).len(), Zone::QUICK_MENU.len(), "{w}x{h}");
        }
        let narrow = layout(Around::QuickMenu, vec2(800.0, 800.0));
        assert!(!narrow.iter().any(|p| p.zone == Zone::QmLeft), "the bands have no room beside the wings");
        assert!(narrow.iter().any(|p| p.zone == Zone::QmUnder));
        assert!(place(Zone::Takeover, vec2(1280.0, 720.0)).is_some() && place(Zone::Takeover, vec2(800.0, 800.0)).is_some());
    }

    #[test]
    fn panels_scale_with_the_menu() {
        let small = place(Zone::WheelTop, vec2(960.0, 540.0)).unwrap();
        let large = place(Zone::WheelTop, vec2(1920.0, 1080.0)).unwrap();
        assert!((large.rect.width() / small.rect.width() - 2.0).abs() < 0.02);
        assert!((large.logical() - small.logical()).length() < 1.5, "the same drawing");
    }

    #[test]
    fn the_bite_is_no_part_of_the_panel() {
        let p = place(Zone::WheelLowerRight, vec2(1280.0, 720.0)).unwrap();
        // Its top left corner lies inside the wheel's clearance: the pointer there is on VRChat's wheel, not on the panel.
        assert!(p.rect.contains(p.rect.min + vec2(2.0, 2.0)));
        assert!(!p.contains(p.rect.min + vec2(2.0, 2.0)));
        assert!(p.contains(p.rect.max - vec2(4.0, 4.0)));
    }
}
