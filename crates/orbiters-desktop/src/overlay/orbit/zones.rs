//! Where VRChat's menus are in its window, measured from the real game on 2026-11-06, and where Orbiters' parts go around them, in the menu's own units so that
//! one set of numbers serves every window size (and, as metres, the headset's plane: [`super::vr`]).
//!
//! **The wheel** scales with the window's height alone. At 1280 x 720, 1920 x 1080, 960 x 540, 1000 x 900, 1000 x 1000, 800 x 800, 900 x 720, 1100 x 800, 1400 x 1050
//! and 1700 x 600 its centre is 0.1556 of the height right of the window's middle and 0.1583 below it, and its radius 0.139 of the height: all ten came out with an
//! unbroken rim (`detect::wheel_seen` on those frames). So 2.46 radii are free below it in any window, and sideways at any shape from square up.
//!
//! **Around the wheel Orbiters is a second wheel:** one ring of six equal cells, each as wide as a slice of VRChat's and on the same angles, so that VRChat's spokes run on
//! through its gaps (Enzo: "more circular [...] so it feels more like an extension of it"; the first judges: capture must not own it). A cell can bloom a second, outer ring
//! ([`Group`]): the 3D pictures' choices, the music's controls.
//!
//! **The Quick Menu** is a canvas that scales with the height down to 16:9 and, narrower than that, somewhat less: `height / 720 * min(1, aspect / (16/9)) ^ 0.28` (fitted
//! on 900 x 720, 1000 x 900, 800 x 800: within 1%), centred on the window's width and at 0.55 of its height. Its units here are pixels of a 1280 x 720 window from that centre.
//! Orbiters is **one wing docked to My Profile's outer edge**, of its height and width, as VRChat's own wings are. Where the window has no room for it, it lies in My Profile's
//! place, as another wing would.

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

/// Where the ring starts and ends, in hundredths of the wheel's radius: just off VRChat's rim, a band about as deep as VRChat's own slices are long.
pub const RING_INNER: f32 = 106.0;
pub const RING_OUTER: f32 = 172.0;
/// The ring that blooms outside it.
pub const BLOOM_INNER: f32 = 176.0;
pub const BLOOM_OUTER: f32 = 238.0;
/// The music's controls need less depth.
pub const BLOOM_SHORT: f32 = 218.0;
/// What separates two cells, and the cells' corners, in those units.
const RING_GAP: f32 = 3.5;
const RING_CORNER: f32 = 9.0;

/// A cell of a ring: from and to, in degrees clockwise from straight up, and its radii.
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

    const fn bloom(from: f32, to: f32) -> Self {
        Self { from, to, inner: BLOOM_INNER, outer: BLOOM_OUTER }
    }

    const fn short(from: f32, to: f32) -> Self {
        Self { from, to, inner: BLOOM_INNER, outer: BLOOM_SHORT }
    }

    pub fn middle(&self) -> f32 {
        (self.from + self.to) / 2.0
    }

    /// Where its content centres: a little outside its middle radius, where it is wider.
    pub fn anchor(&self) -> Vec2 {
        polar(self.inner + (self.outer - self.inner) * 0.52, self.middle())
    }

    /// How wide it is across, at its anchor.
    pub fn room(&self) -> f32 {
        let radius = self.inner + (self.outer - self.inner) * 0.52;
        2.0 * radius * ((self.to - self.from) / 2.0).to_radians().sin() - RING_GAP - 12.0
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

/// A second ring blooming out of a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Pictures,
    Playing,
}

impl Group {
    /// The cell it blooms from.
    pub fn parent(self) -> Zone {
        match self {
            Group::Pictures => Zone::WheelPictures,
            Group::Playing => Zone::WheelPlaying,
        }
    }

    pub fn cells(self) -> &'static [Zone] {
        match self {
            Group::Pictures => &[Zone::BloomPeople, Zone::BloomWorld, Zone::BloomRecord],
            Group::Playing => &[Zone::BloomPrevious, Zone::BloomNext],
        }
    }
}

/// Orbiters' parts. Around the wheel: the ring's cells clockwise from straight up. Around the Quick Menu: its wing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Straight up, where the eyes go first: 3D world search.
    WheelWorlds,
    WheelYouTube,
    WheelPlaying,
    /// Everything Orbiters has, on one sheet.
    WheelAll,
    WheelInstances,
    /// 3D pictures: its second ring holds what it takes.
    WheelPictures,
    BloomPeople,
    BloomWorld,
    BloomRecord,
    BloomPrevious,
    BloomNext,
    /// What is pointed at, in words, above the ring. Nothing presses here.
    WheelCaption,
    /// A dark veil over VRChat's own wheel while the mouse points at the ring: only one menu then looks active. Nothing presses here.
    WheelVeil,
    /// In VR only: the ring's close, for when VRChat's wheel closed without its button.
    WheelHide,
    /// The one wing docked to the Quick Menu.
    QmWing,
    /// The wing's tiles in a row under the Quick Menu's tabs, where the window has no room beside it: VRChat's own panels stay uncovered.
    QmStrip,
    /// What is pointed at, in words, right above the wing.
    QmCaption,
    /// The sheet's back: over the Quick Menu's panels down to the main panel's own bottom edge, so that VRChat's tabs stay, with a head.
    SheetBack,
    /// The page on the sheet. Input to VRChat is blocked while pointing here.
    Takeover,
}

impl Zone {
    pub const WHEEL: [Zone; 13] = [
        Zone::WheelVeil,
        Zone::WheelWorlds,
        Zone::WheelYouTube,
        Zone::WheelPlaying,
        Zone::WheelAll,
        Zone::WheelInstances,
        Zone::WheelPictures,
        Zone::BloomPeople,
        Zone::BloomWorld,
        Zone::BloomRecord,
        Zone::BloomPrevious,
        Zone::BloomNext,
        Zone::WheelCaption,
    ];
    pub const QUICK_MENU: [Zone; 2] = [Zone::QmWing, Zone::QmCaption];

    /// The zones around a menu.
    pub fn around(around: Around) -> &'static [Zone] {
        match around {
            Around::Wheel => &Self::WHEEL,
            Around::QuickMenu => &Self::QUICK_MENU,
        }
    }

    /// Which menu its place is measured from.
    pub fn menu(self) -> Around {
        if Self::WHEEL.contains(&self) || self == Zone::WheelHide {
            Around::Wheel
        } else {
            Around::QuickMenu
        }
    }

    /// Its cell, when it is one of a ring's.
    pub fn ring(self) -> Option<Ring> {
        Some(match self {
            Zone::WheelWorlds => Ring::cell(-30.0, 30.0),
            Zone::WheelYouTube => Ring::cell(30.0, 90.0),
            Zone::WheelPlaying => Ring::cell(90.0, 150.0),
            Zone::WheelAll => Ring::cell(150.0, 210.0),
            Zone::WheelInstances => Ring::cell(210.0, 270.0),
            Zone::WheelPictures => Ring::cell(270.0, 330.0),
            // On the left, so as to stay clear of the caption over the ring.
            Zone::BloomPeople => Ring::bloom(285.0, 315.0),
            Zone::BloomWorld => Ring::bloom(255.0, 285.0),
            Zone::BloomRecord => Ring::bloom(225.0, 255.0),
            Zone::BloomPrevious => Ring::short(90.0, 120.0),
            Zone::BloomNext => Ring::short(120.0, 150.0),
            _ => return None,
        })
    }

    /// The second ring it belongs to.
    pub fn group(self) -> Option<Group> {
        match self {
            Zone::BloomPeople | Zone::BloomWorld | Zone::BloomRecord => Some(Group::Pictures),
            Zone::BloomPrevious | Zone::BloomNext => Some(Group::Playing),
            _ => None,
        }
    }

    /// The second ring it opens, when it is a cell that blooms.
    pub fn blooms(self) -> Option<Group> {
        match self {
            Zone::WheelPictures => Some(Group::Pictures),
            Zone::WheelPlaying => Some(Group::Playing),
            _ => None,
        }
    }

    /// Nothing is pressed here: the pointer and a click pass through to what is under it.
    pub fn passes_clicks(self) -> bool {
        matches!(self, Zone::WheelCaption | Zone::QmCaption | Zone::WheelVeil)
    }

    /// Its place in the menu's units from the menu's centre, y down: around the wheel in hundredths of its radius (a 1280 x 720 window's pixels, nearly), around the Quick
    /// Menu in its units.
    pub fn units(self) -> Rect {
        if let Some(ring) = self.ring() {
            return ring.bounds();
        }
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        match self {
            Zone::WheelCaption => r(-220.0, -236.0, 220.0, -180.0),
            Zone::WheelVeil => r(-103.0, -103.0, 103.0, 103.0),
            Zone::WheelHide => r(126.0, -156.0, 156.0, -126.0),
            // As tall as My Profile and as wide, beside it.
            Zone::QmWing => r(362.0, -150.0, 524.0, 186.0),
            Zone::QmStrip => r(-262.0, 266.0, 262.0, 322.0),
            Zone::QmCaption => r(200.0, -206.0, 524.0, -154.0),
            // The Quick Menu's panels and the FPS line over them, ending where its main panel ends: its tab row and badge stay VRChat's.
            Zone::SheetBack => r(-352.0, -248.0, 352.0, 204.0),
            // N4N0's pages' own shape, 704 wide.
            Zone::Takeover => r(-352.0, -196.0, 352.0, 200.0),
            _ => unreachable!("the rings' cells return above"),
        }
    }

    /// Where it lies where its first place does not fit: the caption, under the strip.
    fn fallback(self) -> Option<Rect> {
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        match self {
            Zone::QmCaption => Some(r(-262.0, 324.0, 262.0, 364.0)),
            _ => None,
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

/// One zone in a window: `None` when nothing of it fits whole. Every part shows whole or not at all: a cell cut by the window's edge would no longer be a cell.
pub fn place(zone: Zone, client: Vec2) -> Option<Placed> {
    let (center, unit) = frame(zone.menu(), client);
    let window = Rect::from_min_size(pos2(0.0, 0.0), client).shrink(EDGE);
    let at = |units: Rect| Rect::from_min_max(center + units.min.to_vec2() * unit, center + units.max.to_vec2() * unit);
    let first = at(zone.units());
    let rect = if window.contains_rect(first) {
        first
    } else {
        let second = at(zone.fallback()?);
        if !window.contains_rect(second) {
            return None;
        }
        second
    };
    // Whole pixels: a layered window has no other, and text stays crisp.
    let rect = Rect::from_min_max(rect.min.round(), rect.max.round());
    Some(Placed { zone, rect, center, unit })
}

/// A zone in its menu's own units, no window to fit in: in the headset, where a plane says what a unit is ([`super::vr`]).
pub fn in_units(zone: Zone) -> Placed {
    Placed { zone, rect: zone.units(), center: pos2(0.0, 0.0), unit: 1.0 }
}

/// Every zone that fits around a menu.
pub fn layout(around: Around, client: Vec2) -> Vec<Placed> {
    let mut out: Vec<Placed> = Zone::around(around).iter().filter_map(|z| place(*z, client)).collect();
    // No room for the wing: a strip that covers none of VRChat's panels.
    if around == Around::QuickMenu && !out.iter().any(|p| p.zone == Zone::QmWing) {
        out.extend(place(Zone::QmStrip, client));
    }
    out
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

    const CELLS: [Zone; 6] = [Zone::WheelWorlds, Zone::WheelYouTube, Zone::WheelPlaying, Zone::WheelAll, Zone::WheelInstances, Zone::WheelPictures];

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
    fn six_equal_cells_on_vrchat_s_own_slices_all_the_way_round() {
        let cells: Vec<Ring> = CELLS.iter().filter_map(|z| z.ring()).collect();
        assert_eq!(cells.iter().map(|c| c.to - c.from).sum::<f32>(), 360.0, "a whole ring");
        assert!(cells.iter().all(|c| c.to - c.from == 60.0), "none larger than another");
        // VRChat's spokes (at 30, 90 and 150, each way) run on through its gaps.
        for spoke in [30.0f32, 90.0, 150.0, 210.0, 270.0, 330.0] {
            for r in [120.0, 150.0, 190.0] {
                assert!(cells.iter().all(|c| !c.contains(polar(r, spoke))), "{spoke} at {r}");
            }
        }
        // Worlds straight up, centred on VRChat's top slice.
        assert_eq!(Zone::WheelWorlds.ring().unwrap().middle(), 0.0);
        for t in 0..1440 {
            let p = polar((RING_INNER + RING_OUTER) / 2.0, t as f32 / 4.0);
            assert!(cells.iter().filter(|c| c.contains(p)).count() <= 1, "two cells at {t}");
        }
    }

    #[test]
    fn nothing_covers_vrchat_s_wheel_but_the_veil() {
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
    fn the_second_rings_stay_off_each_other_and_off_the_caption() {
        let caption = Zone::WheelCaption.units();
        for group in [Group::Pictures, Group::Playing] {
            assert!(group.cells().iter().all(|z| z.group() == Some(group)));
            assert_eq!(group.parent().blooms(), Some(group));
            for zone in group.cells() {
                let ring = zone.ring().unwrap();
                assert!(ring.inner >= RING_OUTER, "{zone:?} is outside the first ring");
                // None of it under the caption.
                for t in 0..=10 {
                    for r in [ring.inner, ring.outer] {
                        let p = polar(r, ring.from + (ring.to - ring.from) * t as f32 / 10.0).to_pos2();
                        assert!(!caption.contains(p), "{zone:?} under the caption at {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_cell_has_a_soft_edge_rounded_corners_and_keeps_to_its_shape() {
        let cell = Zone::WheelAll.ring().unwrap();
        let middle = polar((cell.inner + cell.outer) / 2.0, cell.middle());
        assert_eq!(cell.coverage(middle, 1.0), 1.0);
        assert_eq!(cell.coverage(polar(cell.outer + 5.0, cell.middle()), 1.0), 0.0, "past its outer edge");
        assert_eq!(cell.coverage(polar(cell.inner - 5.0, cell.middle()), 1.0), 0.0, "on the wheel's side");
        assert_eq!(cell.coverage(polar(cell.outer - 0.5, cell.from + 0.8), 1.0), 0.0, "its very corner is rounded away");
        let edge = cell.coverage(polar(cell.outer, cell.middle()), 1.0);
        assert!((edge - 0.5).abs() < 0.01, "{edge}");
        // Its bounds hold it, across an axis too.
        let bounds = cell.bounds();
        for t in 0..120 {
            for r in [cell.inner, cell.outer] {
                assert!(bounds.expand(0.01).contains(polar(r, cell.from + t as f32 / 2.0).to_pos2()));
            }
        }
    }

    #[test]
    fn the_wing_docks_where_there_is_room_and_no_part_ever_covers_vrchat_s_panels_at_rest() {
        // The Quick Menu's main panel, its wings, its row of icons and its safety button, its badge.
        let keep_off = [
            Rect::from_min_max(pos2(-188.0, -204.0), pos2(188.0, 204.0)),
            Rect::from_min_max(pos2(-351.0, -150.0), pos2(351.0, 186.0)),
            Rect::from_min_max(pos2(-242.0, 204.0), pos2(242.0, 262.0)),
        ];
        for k in keep_off {
            assert!(!Zone::QmWing.units().intersects(k), "the wing on {k:?}");
            assert!(!Zone::QmStrip.units().intersects(k), "the strip on {k:?}");
        }
        let (_, unit) = quick_menu(vec2(1280.0, 720.0));
        let docked = place(Zone::QmWing, vec2(1280.0, 720.0)).unwrap();
        assert!((docked.rect.left() - (640.0 + 362.0 * unit)).abs() < 1.0);
        for (w, h) in SIZES {
            let around = layout(Around::QuickMenu, vec2(w, h));
            let wing = around.iter().filter(|p| matches!(p.zone, Zone::QmWing | Zone::QmStrip)).count();
            assert_eq!(wing, 1, "{w}x{h}: one of them, always");
            for placed in &around {
                assert!(placed.rect.left() >= 0.0 && placed.rect.right() <= w && placed.rect.bottom() <= h, "{w}x{h}: {:?}", placed.zone);
            }
        }
        // A narrow window has the strip, and its words under it.
        let narrow = layout(Around::QuickMenu, vec2(1000.0, 900.0));
        assert!(narrow.iter().any(|p| p.zone == Zone::QmStrip) && narrow.iter().any(|p| p.zone == Zone::QmCaption));
        assert!(!layout(Around::QuickMenu, vec2(1920.0, 1080.0)).iter().any(|p| p.zone == Zone::QmStrip));
    }

    #[test]
    fn the_whole_ring_fits_every_window_from_square_up_and_narrower_shows_whole_cells_only() {
        for (w, h) in SIZES {
            let cells = layout(Around::Wheel, vec2(w, h));
            for zone in CELLS {
                assert!(cells.iter().any(|p| p.zone == zone), "{zone:?} at {w}x{h}");
            }
        }
        // The second rings are there in common windows.
        assert_eq!(layout(Around::Wheel, vec2(1280.0, 720.0)).len(), Zone::WHEEL.len());
        // Narrower than square, what would be cut does not show.
        let tall = layout(Around::Wheel, vec2(560.0, 800.0));
        assert!(tall.len() < Zone::WHEEL.len());
        for p in tall {
            assert!((p.logical() - p.zone.units().size()).length() < 2.0, "{:?}", p.zone);
        }
    }

    #[test]
    fn the_sheet_covers_the_panels_leaves_vrchat_s_tabs_and_the_page_keeps_its_shape() {
        let back = Zone::SheetBack.units();
        // Its main panel, wings and FPS line, leaving no sliver of them.
        for rect in [Rect::from_min_max(pos2(-351.0, -240.0), pos2(351.0, 204.0))] {
            assert!(back.contains_rect(rect), "{rect:?}");
        }
        // Its tab row and the badge on it stay VRChat's, as does what lies over the FPS line.
        assert!(!back.intersects(Rect::from_min_max(pos2(-242.0, 206.0), pos2(242.0, 262.0))));
        // It keeps a gutter to the wing beside it.
        assert!(Zone::QmWing.units().left() - back.right() >= 10.0);
        let page = Zone::Takeover.units();
        assert!(back.contains_rect(page));
        assert!((page.width() / page.height() - 1024.0 / 576.0).abs() < 0.01);
        assert!(!Zone::QmWing.units().intersects(back), "the wing stays beside it as tabs");
        for (w, h) in SIZES {
            assert!(place(Zone::SheetBack, vec2(w, h)).is_some() && place(Zone::Takeover, vec2(w, h)).is_some(), "{w}x{h}");
        }
    }

    #[test]
    fn a_click_is_on_a_cell_s_shape_and_never_on_words_or_the_veil() {
        let p = place(Zone::WheelPlaying, vec2(1280.0, 720.0)).unwrap();
        let corners = [p.rect.left_top(), p.rect.right_top(), p.rect.left_bottom(), p.rect.right_bottom()];
        assert!(corners.iter().any(|c| !p.contains(*c)));
        assert!(p.contains(p.center + p.zone.ring().unwrap().anchor() * p.unit));
        for zone in [Zone::WheelCaption, Zone::WheelVeil, Zone::QmCaption] {
            let placed = place(zone, vec2(1280.0, 720.0)).unwrap();
            assert!(!placed.contains(placed.rect.center()), "{zone:?}");
        }
    }

    #[test]
    fn panels_scale_with_the_menu() {
        let small = place(Zone::WheelWorlds, vec2(960.0, 540.0)).unwrap();
        let large = place(Zone::WheelWorlds, vec2(1920.0, 1080.0)).unwrap();
        assert!((large.rect.width() / small.rect.width() - 2.0).abs() < 0.02);
        // Within what whole pixels round away: a pixel is 1.33 units in the small window, on each side.
        assert!((large.logical() - small.logical()).length() < 2.7, "the same drawing");
    }
}
