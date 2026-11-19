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
    /// VRChat's camera panel, where it is seen: in a headset (`camera_track`), and on the desktop in the middle of VRChat's window
    /// (`detect::camera_panel`, [`camera_on_screen`]).
    Camera,
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
// Nearly as deep as the first ring and a little more: its cells are half as wide, and a name, an icon and a laser want the room (round 19's judges).
pub const BLOOM_OUTER: f32 = 250.0;
/// What separates two cells, and the cells' corners, in those units.
// Gaps of the tiles' kind: cells that read as Orbiters' tiles round the wheel, not as a second VRChat ring (round 14's judges).
const RING_GAP: f32 = 6.0;
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
            // Its two 3D pictures: a recording starts beside VRChat's camera alone (Enzo, F15).
            Group::Pictures => &[Zone::BloomPeople, Zone::BloomWorld],
            Group::Playing => &[Zone::BloomPrevious, Zone::BloomNext],
        }
    }
}

/// Orbiters' parts. Around the wheel: the ring's cells clockwise from straight up. Around the Quick Menu: its wing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Over a corner, while a sheet is up: takes the sheet and all with it away for a moment, to reach VRChat's own menus, and brings them back as they were.
    SheetPeek,
    /// Around VRChat's camera panel, in its plane: a column of tiles beside it on the side with room (two 3D pictures, Record, the QR code), what they have to say
    /// under it with a button when there is one to press, four corners round its picture while it looks for a code, and the page a code opened, over its top edge.
    CameraPeople,
    CameraWorld,
    CameraRecord,
    CameraQr,
    CameraCaption,
    CameraAct,
    CameraScan,
    CameraWebBar,
    CameraWeb,
    /// Right after a 3D picture, under what the tiles say (REQUEST13): "Share to …" and "Share publicly", then how each share
    /// goes and what it earned. Never on the camera's picture or its buttons.
    CameraShare,
    /// The same on a screen, as a column at the panel's right: under the panel is VRChat's row of buttons, the caption
    /// and the window's foot (no room at 720 tall), and where the bridge clicks.
    CameraShareDesk,
    /// "Share to …" in a headset: the friends to choose, over the camera's top edge where a code's page goes (never both).
    CameraChooser,
    /// While a recording runs: its clock and Stop, under the wheel and under the wing (Enzo, F13). Nothing here starts one.
    WheelStop,
    QmStop,
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
    BloomPrevious,
    BloomNext,
    /// What is pointed at, in words, above the ring. Nothing presses here.
    WheelCaption,
    /// A dark veil over VRChat's own wheel while the mouse points at the ring: only one menu then looks active. Nothing presses here.
    WheelVeil,
    /// In VR only: the ring's close, for when VRChat's wheel closed without its button.
    WheelHide,
    /// The wing docked to the Quick Menu, each tile a part of its own with nothing behind them, so that each can move by itself in VR (Enzo). Its name, its six tiles, what plays.
    WingName,
    WingWorlds,
    WingYouTube,
    WingInstances,
    WingPictures,
    WingTalk,
    WingAll,
    WingPlaying,
    /// The wing's tiles in a row under the Quick Menu's tabs, where the window has no room beside it: VRChat's own panels stay uncovered.
    QmStrip,
    /// What is pointed at, in words, right above the wing.
    QmCaption,
    /// The sheet's back: over the Quick Menu's panels down to the main panel's own bottom edge, so that VRChat's tabs stay, with a head.
    SheetBack,
    /// Under a sheet on the desktop: says what is pointed at, over VRChat's tooltip for what the sheet hides.
    SheetHint,
    /// The page on the sheet. Input to VRChat is blocked while pointing here.
    Takeover,
}

/// How far above its place the wing's caption lies in a headset, where the wing's name pill (with Hide tiles) is under it.
pub const QM_CAPTION_OVER_THE_NAME: f32 = 34.0;

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
        Zone::BloomPrevious,
        Zone::BloomNext,
        Zone::WheelCaption,
        Zone::WheelStop,
    ];
    pub const WING: [Zone; 8] = [Zone::WingName, Zone::WingWorlds, Zone::WingYouTube, Zone::WingInstances, Zone::WingPictures, Zone::WingTalk, Zone::WingAll, Zone::WingPlaying];
    pub const QUICK_MENU: [Zone; 10] =
        [Zone::WingName, Zone::WingWorlds, Zone::WingYouTube, Zone::WingInstances, Zone::WingPictures, Zone::WingTalk, Zone::WingAll, Zone::WingPlaying, Zone::QmCaption, Zone::QmStop];

    /// The zones around a menu.
    pub const CAMERA: [Zone; 12] = [
        Zone::CameraScan,
        Zone::CameraPeople,
        Zone::CameraWorld,
        Zone::CameraRecord,
        Zone::CameraQr,
        Zone::CameraCaption,
        Zone::CameraAct,
        Zone::CameraWebBar,
        Zone::CameraWeb,
        Zone::CameraShare,
        Zone::CameraShareDesk,
        Zone::CameraChooser,
    ];

    pub fn around(around: Around) -> &'static [Zone] {
        match around {
            Around::Wheel => &Self::WHEEL,
            Around::QuickMenu => &Self::QUICK_MENU,
            Around::Camera => &Self::CAMERA,
        }
    }

    /// Which menu its place is measured from.
    pub fn menu(self) -> Around {
        if Self::CAMERA.contains(&self) {
            Around::Camera
        } else if Self::WHEEL.contains(&self) || self == Zone::WheelHide {
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
            // One arc beside its parent, of one depth, as the ring is: short of the top, where the caption right over the ring lies.
            Zone::BloomPeople => Ring::bloom(285.0, 315.0),
            Zone::BloomWorld => Ring::bloom(255.0, 285.0),
            Zone::BloomPrevious => Ring::bloom(90.0, 120.0),
            Zone::BloomNext => Ring::bloom(120.0, 150.0),
            _ => return None,
        })
    }

    /// The second ring it belongs to.
    pub fn group(self) -> Option<Group> {
        match self {
            Zone::BloomPeople | Zone::BloomWorld => Some(Group::Pictures),
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
        matches!(self, Zone::WheelCaption | Zone::QmCaption | Zone::CameraCaption | Zone::CameraScan | Zone::WheelVeil | Zone::SheetHint)
    }

    /// A part that shows only while it has something to do (a recording to stop, a code being looked for, a page opened, a button to offer): it joins the menu's
    /// parts and leaves them as that changes ([`super::Panels::set_open`]).
    pub fn only_while_needed(self) -> bool {
        matches!(self, Zone::WheelStop | Zone::QmStop | Zone::CameraAct | Zone::CameraScan | Zone::CameraWebBar | Zone::CameraWeb | Zone::CameraShare | Zone::CameraShareDesk | Zone::CameraChooser)
    }

    /// Its place in the menu's units from the menu's centre, y down: around the wheel in hundredths of its radius (a 1280 x 720 window's pixels, nearly), around the Quick
    /// Menu in its units.
    pub fn units(self) -> Rect {
        if let Some(ring) = self.ring() {
            return ring.bounds();
        }
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        match self {
            // Right over the ring: what to do with it is said where the eyes are.
            // Clear of the ring's top and its curved names (round 11's judges found it against them).
            Zone::WheelCaption => r(-220.0, -254.0, 220.0, -196.0),
            Zone::WheelVeil => r(-109.0, -109.0, 109.0, 109.0),
            // Level with the caption over the ring, past its right end: clear of the ring and of either second ring (a judge found it sitting on the rim), and still beside them.
            // Nearer the ring than it was (round 12's judges found it detached), still clear of the second rings and the caption.
            // Level with the caption's pill, in its row, as the wing's Hide sits beside its name (round 14's judges saw it lower and off to the side).
            // In a headset the caption's pill ends at its zone's right edge, so this follows it at once (round 15's judges).
            // Over the caption, on the ring's axis: the caption's pill centred over the ring's top and this right over it (rounds 14 and 16 to 18: both hung off to the
            // ring's upper right, whatever was tried beside the caption).
            Zone::WheelHide => r(-56.0, -306.0, 56.0, -268.0),
            // As tall as My Profile and as wide, beside it.
            // As tall as My Profile and as wide, beside it.
            Zone::WingName => r(362.0, -150.0, 524.0, -120.0),
            Zone::WingWorlds => wing_tile(0.0, 0.0),
            Zone::WingYouTube => wing_tile(1.0, 0.0),
            Zone::WingInstances => wing_tile(0.0, 1.0),
            Zone::WingPictures => wing_tile(1.0, 1.0),
            Zone::WingTalk => wing_tile(0.0, 2.0),
            Zone::WingAll => wing_tile(1.0, 2.0),
            // Its foot level with VRChat's main panel's (round 15's judges, mockup-sonnet-1): the sheet ends there too.
            Zone::WingPlaying => r(362.0, 144.0, 524.0, 205.5),
            Zone::QmStrip => r(-300.0, 266.0, 300.0, 318.0),
            // On the wing, as wide as it, right over its name: its words wrap, and it is as high as they take.
            // Its foot where the wing's name pill is: on a screen the name heads it (the pill is not there); a headset keeps it above that pill ([`QM_CAPTION_OVER_THE_NAME`]).
            Zone::QmCaption => r(362.0, -228.0, 524.0, -120.0),
            // The Quick Menu's panels and the FPS line over them, ending where its main panel ends: its tab row and badge stay VRChat's.
            // Clear of what VRChat has over its menu, down over its main panel's foot.
            Zone::SheetBack => r(-352.0, -240.0, 352.0, 205.5),
            // Under the tab row, where VRChat's own tooltip for what lies under the pointer appears.
            Zone::SheetHint => r(-300.0, 268.0, 300.0, 302.0),
            Zone::SheetPeek => r(362.0, -300.0, 512.0, -268.0),
            // In the panel's own units (its picture 410 x 220, from its middle, y down; VRChat's row of buttons hangs under it from about x -165 to 150, y 113 to 158):
            // one column beside its left edge with a real gap, from the picture's top down to the foot of that row, mirrored on the right side ([`Placed::mirrored`]).
            // The pictures and Record together, the QR code apart, level with VRChat's own buttons.
            Zone::CameraPeople => camera_tile(0),
            Zone::CameraWorld => camera_tile(1),
            Zone::CameraRecord => camera_tile(2),
            Zone::CameraQr => camera_tile(3),
            // Under VRChat's row of buttons, from the column's outer edge: what the tiles have to say, and a button for it at its end.
            Zone::CameraCaption => r(CAMERA_COLUMN_OUT, CAMERA_UNDER[0], 64.0, CAMERA_UNDER[1]),
            Zone::CameraAct => r(72.0, CAMERA_UNDER[0], 296.0, CAMERA_UNDER[0] + 34.0),
            // Corners just off the picture's own while a code is looked for: outside it, clear of the column and of VRChat's buttons.
            Zone::CameraScan => r(-CAMERA_HALF[0] - CAMERA_SCAN_OUT, -CAMERA_HALF[1] - CAMERA_SCAN_OUT, CAMERA_HALF[0] + CAMERA_SCAN_OUT, CAMERA_HALF[1] + CAMERA_SCAN_OUT),
            // Over the camera's top edge, never in front of it: its bar nearest the camera, the page above. Stood up on that edge in a headset ([`super::vr`]).
            Zone::CameraWebBar => r(-CAMERA_HALF[0], CAMERA_WEB_BAR[0], CAMERA_HALF[0], CAMERA_WEB_BAR[1]),
            Zone::CameraWeb => r(-CAMERA_HALF[0], CAMERA_WEB_BAR[0] - 6.0 - CAMERA_WEB_TALL, CAMERA_HALF[0], CAMERA_WEB_BAR[0] - 6.0),
            // Under what the tiles say, from the column's outer edge to the picture's right edge: in reach of the hand that
            // holds the camera, clear of VRChat's row of buttons and of the caption.
            Zone::CameraShare => r(CAMERA_COLUMN_OUT, CAMERA_SHARE[0], CAMERA_HALF[0], CAMERA_SHARE[1]),
            // On a screen: right of the picture with the column's gap, from its top, two buttons over each other.
            Zone::CameraShareDesk => r(CAMERA_HALF[0] + CAMERA_GAP, -CAMERA_HALF[1], CAMERA_HALF[0] + CAMERA_GAP + 190.0, -CAMERA_HALF[1] + 268.0),
            // Where a code's page goes, as tall as it and its bar: stood up on the camera's top edge in a headset.
            Zone::CameraChooser => r(-CAMERA_HALF[0], CAMERA_WEB_BAR[1] - CAMERA_CHOOSER_TALL, CAMERA_HALF[0], CAMERA_WEB_BAR[1]),
            // Right under the ring, whose foot is free in any window: in reach, and over nothing of VRChat's.
            Zone::WheelStop => r(-78.0, 188.0, 78.0, 228.0),
            // Under the wing's foot, as wide as it.
            Zone::QmStop => r(362.0, 213.0, 524.0, 249.0),
            // N4N0's pages' own shape, 704 wide.
            Zone::Takeover => r(-352.0, -196.0, 352.0, 200.0),
            _ => unreachable!("the rings' cells return above"),
        }
    }

/// Where it lies where its first place does not fit: the caption, under the strip.
    fn fallback(self) -> Option<Rect> {
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        match self {
            // One line, close under the strip: two were squeezed against the window's foot.
            // Clear of the strip by about 8 px (round 16's judges found them touching).
            Zone::QmCaption => Some(r(-300.0, 330.0, 300.0, 360.0)),
            Zone::SheetPeek => Some(r(-352.0, -276.0, -202.0, -244.0)),
            _ => None,
        }
    }
}

/// Half VRChat's camera's picture, in its units (`camera_track::PANEL_SIZE`).
pub const CAMERA_HALF: [f32; 2] = [205.0, 110.0];
/// The gap between the picture's edge and the column: about 13 mm at the panel's usual size, so that they never touch as tracking moves them a little.
pub const CAMERA_GAP: f32 = 18.0;
const CAMERA_TILE: [f32; 2] = [92.0, 60.0];
/// The column's outer edge.
pub const CAMERA_COLUMN_OUT: f32 = -CAMERA_HALF[0] - CAMERA_GAP - CAMERA_TILE[0];
/// The line under VRChat's row of buttons (from, to).
const CAMERA_UNDER: [f32; 2] = [166.0, 226.0];
/// The share row's line under the caption (from, to).
pub const CAMERA_SHARE: [f32; 2] = [CAMERA_UNDER[1] + 8.0, CAMERA_UNDER[1] + 8.0 + 104.0];
/// How tall the friends' chooser over the camera is.
pub const CAMERA_CHOOSER_TALL: f32 = 516.0;
/// How far the scanning corners stand off the picture.
const CAMERA_SCAN_OUT: f32 = 8.0;
/// The page's bar over the camera's top edge (from, to), and how tall the page over it is: a phone's page, about 30 cm wide.
pub const CAMERA_WEB_BAR: [f32; 2] = [-CAMERA_HALF[1] - 16.0 - 40.0, -CAMERA_HALF[1] - 16.0];
const CAMERA_WEB_TALL: f32 = 470.0;

/// On the desktop, VRChat's camera panel's own row of buttons (Take Photo to Anchor or Focus), against the outside of its rim, in its heights (350 at 720 tall): from
/// the rim's foot down, from its left edge in, from its right edge in. Its arrow to the next buttons hangs past the right edge. Measured on `p1-stream-camera` and
/// `07-after-walk` (1280 x 720): the row is 383 to 895 across, 535 to 620 down, under a rim 332 to 948, 185 to 535.
pub const DESK_CAMERA_BUTTONS: [f32; 3] = [86.0, 51.0, 53.0];
/// How far VRChat's arrow to its next camera buttons hangs past the panel's right edge, at 720 tall.
pub const DESK_CAMERA_ARROW: f32 = 25.0;
/// How large Orbiters' tiles are beside it, in pixels a unit at 720 tall: a tile of 92 x 60 is 124 x 81, as tall as VRChat's own buttons (80), the column of four
/// about the panel's height, as Enzo drew it (2026-11-15).
pub const DESK_CAMERA_UNIT: f32 = 1.35;

/// Where the camera's parts go on the desktop, around VRChat's camera panel whose rim's outside is `panel` (window pixels, [`super::detect::camera_panel`]): the
/// column at its left with the gap of the headset's, its top level with the panel's; their words under VRChat's row of buttons from the column's outer edge, with
/// the demo avatar's button after them; the corners just off the rim while a code is looked for. No page over it: on the desktop a code's page opens in the browser
/// (`Panels::follow`). Nothing lies on the panel's picture or on its buttons. Each part shows whole or not at all.
pub fn camera_on_screen(panel: Rect, client: Vec2) -> Vec<Placed> {
    let window = Rect::from_min_size(pos2(0.0, 0.0), client).shrink(EDGE);
    let k = panel.height() / CAMERA_PANEL_TALL;
    // Smaller where the window has no room at the panel's left for the column at its size.
    let unit = (DESK_CAMERA_UNIT * k).min((panel.left() - window.left()) / (CAMERA_TILE[0] + CAMERA_GAP));
    if unit <= 0.0 {
        return Vec::new();
    }
    let center = panel.center();
    let column_right = panel.left() - CAMERA_GAP * unit;
    let column_left = column_right - CAMERA_TILE[0] * unit;
    let tile = |row: usize| {
        let top = panel.top() + (camera_tile(row).top() + CAMERA_HALF[1]) * unit;
        Rect::from_min_size(pos2(column_left, top), vec2(CAMERA_TILE[0], CAMERA_TILE[1]) * unit)
    };
    let under = panel.bottom() + (DESK_CAMERA_BUTTONS[0] + 8.0) * k;
    let caption = Rect::from_min_size(pos2(column_left, under), vec2(CAMERA_COLUMN_OUT.abs() + 64.0, CAMERA_UNDER[1] - CAMERA_UNDER[0]) * unit);
    let act = Rect::from_min_size(pos2(caption.right() + 8.0 * unit, under), vec2(224.0, 34.0) * unit);
    let scan = panel.expand(CAMERA_SCAN_OUT * k);
    // Past VRChat's arrow to its next buttons, which hangs about 25 px (at 720) past the panel's right edge.
    let share = Rect::from_min_size(pos2(panel.right() + DESK_CAMERA_ARROW * k + CAMERA_GAP * unit, panel.top()), Zone::CameraShareDesk.units().size() * unit);
    [
        (Zone::CameraPeople, tile(0)),
        (Zone::CameraWorld, tile(1)),
        (Zone::CameraRecord, tile(2)),
        (Zone::CameraQr, tile(3)),
        (Zone::CameraCaption, caption),
        (Zone::CameraAct, act),
        (Zone::CameraScan, scan),
        (Zone::CameraShareDesk, share),
    ]
    .into_iter()
    .map(|(zone, rect)| Placed { zone, rect: Rect::from_min_max(rect.min.round(), rect.max.round()), center, unit })
    .filter(|p| window.contains_rect(p.rect))
    .collect()
}

/// The panel's height at 720 tall ([`super::detect::CAMERA_PANEL`]).
const CAMERA_PANEL_TALL: f32 = 350.0;

/// One of the camera's tiles, from the top: the 3D pictures and Record together, the QR code a little apart.
fn camera_tile(row: usize) -> Rect {
    const GAP: f32 = 6.0;
    let apart = if row == 3 { 14.0 - GAP } else { 0.0 };
    let top = -CAMERA_HALF[1] + row as f32 * (CAMERA_TILE[1] + GAP) + apart;
    Rect::from_min_size(pos2(CAMERA_COLUMN_OUT, top), vec2(CAMERA_TILE[0], CAMERA_TILE[1]))
}

/// A tile of the wing, in its column and row.
fn wing_tile(column: f32, row: f32) -> Rect {
    const WIDTH: f32 = 78.0;
    const HEIGHT: f32 = 80.0;
    const GAP: f32 = 6.0;
    let min = pos2(362.0 + column * (WIDTH + GAP), -114.0 + row * (HEIGHT + GAP));
    Rect::from_min_size(min, vec2(WIDTH, HEIGHT))
}

/// What a panel keeps clear of the window's edge, in pixels.
const EDGE: f32 = 4.0;

/// On a screen, how far each of the wing's tiles reaches into the gaps round it, in units: half a gap, so that the tiles lie on one continuous backing and
/// nothing of VRChat's shows between them (round 14's judges saw a dark VRChat element there). In a headset the tiles stay apart with nothing behind them (Enzo,
/// requests 4 and 39).
pub const WING_BLEED: f32 = 3.0;

impl Zone {
    /// One of the wing's tiles, which lie on one backing on a screen.
    pub fn on_the_wing_backing(self) -> bool {
        matches!(self, Zone::WingWorlds | Zone::WingYouTube | Zone::WingInstances | Zone::WingPictures | Zone::WingTalk | Zone::WingAll | Zone::WingPlaying)
    }
}

/// A panel placed in the window: where in pixels, the menu's centre and how many pixels a unit is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub zone: Zone,
    pub rect: Rect,
    pub center: Pos2,
    pub unit: f32,
}

impl Placed {
    /// On the other side of the middle of what it is around.
    pub fn mirrored(self) -> Self {
        Self { rect: Rect::from_min_max(pos2(-self.rect.max.x, self.rect.min.y), pos2(-self.rect.min.x, self.rect.max.y)), ..self }
    }

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
        Around::QuickMenu | Around::Camera => quick_menu(client),
    }
}

/// One zone in a window: `None` when nothing of it fits whole. Every part shows whole or not at all: a cell cut by the window's edge would no longer be a cell.
pub fn place(zone: Zone, client: Vec2) -> Option<Placed> {
    let (center, unit) = frame(zone.menu(), client);
    let window = Rect::from_min_size(pos2(0.0, 0.0), client).shrink(EDGE);
    let bleed = if zone.on_the_wing_backing() { WING_BLEED } else { 0.0 };
    let at = |units: Rect| {
        let units = units.expand(bleed);
        Rect::from_min_max(center + units.min.to_vec2() * unit, center + units.max.to_vec2() * unit)
    };
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

/// In a window as wide as 16:9 there is no room under the strip for its words: beside VRChat's tab row, right over the strip's end.
const STRIP_CAPTION_BESIDE: Rect = Rect { min: pos2(250.0, 210.0), max: pos2(524.0, 262.0) };

/// The strip's words, under it, or where else they fit.
fn strip_caption(client: Vec2) -> Option<Placed> {
    let (center, unit) = frame(Around::QuickMenu, client);
    let window = Rect::from_min_size(pos2(0.0, 0.0), client).shrink(EDGE);
    let at = |units: Rect| Rect::from_min_max(center + units.min.to_vec2() * unit, center + units.max.to_vec2() * unit);
    [Zone::QmCaption.fallback()?, STRIP_CAPTION_BESIDE].into_iter().map(at).find(|r| window.contains_rect(*r)).map(|rect| Placed { zone: Zone::QmCaption, rect: Rect::from_min_max(rect.min.round(), rect.max.round()), center, unit })
}

/// Every zone that fits around a menu, the Quick Menu's as chosen: the strip when `strip`, and in any case where the wing does not fit.
pub fn layout_as(around: Around, client: Vec2, strip: bool) -> Vec<Placed> {
    if strip && around == Around::QuickMenu {
        if let Some(row) = place(Zone::QmStrip, client) {
            return std::iter::once(row).chain(strip_caption(client)).collect();
        }
    }
    layout(around, client)
}

/// Every zone that fits around a menu.
pub fn layout(around: Around, client: Vec2) -> Vec<Placed> {
    let mut out: Vec<Placed> = Zone::around(around).iter().filter_map(|z| place(*z, client)).collect();
    // No room for the wing, all of it or none: a strip that covers none of VRChat's panels.
    if around == Around::QuickMenu && !Zone::WING.iter().all(|z| out.iter().any(|p| p.zone == *z)) {
        out.retain(|p| !Zone::WING.contains(&p.zone));
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

    /// Hide ring sits clear of everything the ring is: its cells, its second rings, and its caption.
    #[test]
    fn hide_ring_is_clear_of_the_ring() {
        let hide = Zone::WheelHide.units();
        assert!(!hide.intersects(Zone::WheelCaption.units()));
        let nearest = pos2(hide.left(), hide.bottom());
        assert!(nearest.to_vec2().length() > BLOOM_OUTER + 20.0, "{nearest:?}");
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
            for zone in Zone::WING {
                assert!(!zone.units().intersects(k), "the wing's {zone:?} on {k:?}");
            }
            assert!(!Zone::QmStrip.units().intersects(k), "the strip on {k:?}");
        }
        let (_, unit) = quick_menu(vec2(1280.0, 720.0));
        let docked = place(Zone::WingName, vec2(1280.0, 720.0)).unwrap();
        assert!((docked.rect.left() - (640.0 + 362.0 * unit)).abs() < 1.0);
        for (w, h) in SIZES {
            let around = layout(Around::QuickMenu, vec2(w, h));
            let (wing, strip) = (around.iter().filter(|p| Zone::WING.contains(&p.zone)).count(), around.iter().filter(|p| p.zone == Zone::QmStrip).count());
            assert!((wing, strip) == (Zone::WING.len(), 0) || (wing, strip) == (0, 1), "{w}x{h}: the whole wing or the strip: {wing} {strip}");
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
        assert!(Zone::WING.iter().all(|z| z.units().left() - back.right() >= 10.0));
        let page = Zone::Takeover.units();
        assert!(back.contains_rect(page));
        assert!((page.width() / page.height() - 1024.0 / 576.0).abs() < 0.01);
        assert!(Zone::WING.iter().all(|z| !z.units().intersects(back)), "the wing stays beside it as tabs");
        // No tile of the wing on another.
        for (i, a) in Zone::WING.iter().enumerate() {
            for b in &Zone::WING[i + 1..] {
                assert!(!a.units().intersects(b.units()), "{a:?} {b:?}");
            }
        }
        for (w, h) in SIZES {
            assert!(place(Zone::SheetBack, vec2(w, h)).is_some() && place(Zone::Takeover, vec2(w, h)).is_some(), "{w}x{h}");
        }
    }

    /// Enzo: the strip as a layout of its own. It has to fit the windows most people have, its words too.
    #[test]
    fn the_strip_can_be_chosen_in_a_wide_window_as_well() {
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0), (1000.0, 900.0)] {
            let parts = layout_as(Around::QuickMenu, vec2(w, h), true);
            assert!(parts.iter().any(|p| p.zone == Zone::QmStrip), "{w}x{h}");
            assert!(parts.iter().any(|p| p.zone == Zone::QmCaption), "{w}x{h}: its words");
            assert!(!parts.iter().any(|p| Zone::WING.contains(&p.zone)), "{w}x{h}: no wing with it");
            let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
            assert!(parts.iter().all(|p| window.contains_rect(p.rect)), "{w}x{h}");
            let (row, words) = (parts.iter().find(|p| p.zone == Zone::QmStrip).unwrap(), parts.iter().find(|p| p.zone == Zone::QmCaption).unwrap());
            assert!(!row.rect.intersects(words.rect), "{w}x{h}");
        }
        assert!(layout_as(Around::QuickMenu, vec2(1920.0, 1080.0), false).iter().any(|p| p.zone == Zone::WingWorlds));
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
    /// Enzo (F18, IMG3 and IMG4): the camera's tiles are one column beside its picture with a real gap, never over the picture or VRChat's row of buttons under it, on
    /// either side; what they say is under that row; the page a code opened is over the camera's top edge.
    #[test]
    fn the_camera_s_parts_keep_clear_of_the_camera() {
        let picture = Rect::from_min_max(pos2(-CAMERA_HALF[0], -CAMERA_HALF[1]), pos2(CAMERA_HALF[0], CAMERA_HALF[1]));
        // VRChat's buttons under its picture (Take Photo to its arrow), measured on `testdata_camera/camera-panel-752x417.jpg`, with a margin.
        let buttons = Rect::from_min_max(pos2(-172.0, 110.0), pos2(158.0, 160.0));
        for right in [false, true] {
            let at = |zone: Zone| if right { in_units(zone).mirrored().rect } else { in_units(zone).rect };
            for zone in Zone::CAMERA {
                let r = at(zone);
                if zone == Zone::CameraScan {
                    // Round the picture, just off it: corners only are drawn.
                    assert!(r.contains_rect(picture) && r.width() - picture.width() <= 20.0, "{r:?}");
                    continue;
                }
                assert!(!r.intersects(picture), "{zone:?} over the camera's picture ({right})");
                assert!(!r.intersects(buttons), "{zone:?} over VRChat's buttons ({right})");
            }
            let column = [Zone::CameraPeople, Zone::CameraWorld, Zone::CameraRecord, Zone::CameraQr];
            for zone in column {
                let r = at(zone);
                let gap = if right { r.left() - CAMERA_HALF[0] } else { -CAMERA_HALF[0] - r.right() };
                assert!(gap >= 15.0, "{zone:?}: a gap of {gap} units ({right})");
                assert!(r.top() >= -CAMERA_HALF[1] && r.bottom() <= 160.0, "{zone:?} reaches past the camera's height: {r:?}");
                assert!(r.right() < at(Zone::CameraScan).left() || r.left() > at(Zone::CameraScan).right(), "{zone:?} touches the scanning corners");
            }
            // One column, in order, none over another.
            for pair in column.windows(2) {
                assert!(at(pair[0]).bottom() < at(pair[1]).top(), "{pair:?}");
            }
            for zone in [Zone::CameraWebBar, Zone::CameraWeb] {
                assert!(at(zone).bottom() <= -CAMERA_HALF[1] - 10.0, "{zone:?} is not over the camera's top edge");
                assert!(at(zone).left() >= -CAMERA_HALF[0] - 1.0 && at(zone).right() <= CAMERA_HALF[0] + 1.0, "{zone:?} is wider than the camera");
            }
            assert!(at(Zone::CameraWebBar).top() > at(Zone::CameraWeb).bottom(), "the bar nearest the camera");
            assert!(!at(Zone::CameraCaption).intersects(at(Zone::CameraAct)));
        }
    }

    /// Enzo, 2026-11-15 (L-desk-camera): on the desktop the camera's tools are a column at the left of VRChat's camera panel, about its height, where he drew it. Nothing
    /// of it lies on the panel's picture or on VRChat's buttons under it, nor where the bridge clicks (the Quick Menu's camera icon and its Stream Camera tile), at
    /// the sizes the panel was measured at and at a window just wide enough.
    #[test]
    fn on_the_desktop_the_camera_s_column_is_left_of_its_panel_and_clear_of_vrchat_s_parts() {
        for (client, panel) in [
            (vec2(1280.0, 720.0), Rect::from_min_max(pos2(332.0, 185.0), pos2(948.0, 535.0))),
            (vec2(1920.0, 1080.0), Rect::from_min_max(pos2(498.0, 277.5), pos2(1422.0, 802.5))),
            (vec2(960.0, 540.0), Rect::from_min_max(pos2(249.0, 138.0), pos2(711.0, 403.0))),
            (vec2(800.0, 600.0), Rect::from_min_max(pos2(207.0, 159.0), pos2(593.0, 378.0))),
        ] {
            let k = panel.height() / 350.0;
            let placed = camera_on_screen(panel, client);
            let at = |zone: Zone| placed.iter().find(|p| p.zone == zone).map(|p| p.rect);
            let buttons = Rect::from_min_max(pos2(panel.left() + 51.0 * k, panel.bottom()), pos2(panel.right() + 25.0 * k, panel.bottom() + 86.0 * k));
            let column = [Zone::CameraPeople, Zone::CameraWorld, Zone::CameraRecord, Zone::CameraQr];
            for zone in column.into_iter().chain([Zone::CameraCaption]) {
                assert!(at(zone).is_some(), "{client:?}: {zone:?} has no room");
            }
            for p in &placed {
                assert!(Rect::from_min_size(pos2(0.0, 0.0), client).contains_rect(p.rect), "{client:?}: {:?} leaves the window", p.zone);
                if p.zone == Zone::CameraScan {
                    assert!(p.rect.contains_rect(panel) && p.rect.width() - panel.width() <= 20.0 * k, "{client:?}: corners just off the rim");
                    continue;
                }
                assert!(!p.rect.intersects(panel), "{client:?}: {:?} on the camera's panel", p.zone);
                assert!(!p.rect.intersects(buttons), "{client:?}: {:?} on VRChat's buttons", p.zone);
                assert!((p.logical() - p.zone.units().size()).length() < 2.5, "{client:?}: {:?} is drawn at the headset's own size in units", p.zone);
            }
            // The bridge's clicks, on the Quick Menu (the camera column is not up then, but nothing of it may ever be there).
            let (sx, sy) = (client.x / 1280.0, client.y / 720.0);
            for click in [pos2(640.0 * sx, 620.0 * sy), pos2(597.0 * sx, 345.0 * sy)] {
                assert!(!placed.iter().any(|p| !p.zone.passes_clicks() && p.rect.contains(click)), "{client:?}: something of ours where the bridge clicks {click:?}");
            }
            // One column at its left with a real gap, from its top, in order, about its height, its tiles about as tall as VRChat's buttons.
            for zone in column {
                let r = at(zone).unwrap();
                let gap = panel.left() - r.right();
                assert!(gap >= 15.0 * k && gap <= 40.0 * k, "{client:?}: {zone:?} {gap} px from the panel");
            }
            for pair in column.windows(2) {
                assert!(at(pair[0]).unwrap().bottom() < at(pair[1]).unwrap().top(), "{pair:?}");
            }
            let (top, foot) = (at(Zone::CameraPeople).unwrap().top(), at(Zone::CameraQr).unwrap().bottom());
            assert!((top - panel.top()).abs() <= 1.0, "{client:?}: the column starts level with the panel");
            assert!((foot - top) / panel.height() > 0.85 && (foot - top) / panel.height() < 1.15, "{client:?}: the column is about the panel's height");
            let tall = at(Zone::CameraPeople).unwrap().height() / k;
            assert!((70.0..=95.0).contains(&tall), "{client:?}: a tile {tall} px tall at 720");
            // The words under VRChat's buttons, starting under the column.
            let caption = at(Zone::CameraCaption).unwrap();
            assert!(caption.top() >= buttons.bottom() && (caption.left() - at(Zone::CameraPeople).unwrap().left()).abs() <= 1.0);
        }
        // No room at all at its left: nothing, rather than parts cut off.
        assert!(camera_on_screen(Rect::from_min_max(pos2(2.0, 100.0), pos2(1278.0, 600.0)), vec2(1280.0, 720.0)).iter().all(|p| p.zone == Zone::CameraCaption || p.zone == Zone::CameraAct || p.zone == Zone::CameraScan) );
    }

    /// Enzo (F13): the wheel's Stop lies under the ring, clear of its cells, its second rings and VRChat's wheel.
    #[test]
    fn the_wheel_s_stop_is_clear_of_the_ring() {
        let stop = Zone::WheelStop.units();
        let cells: Vec<Ring> = Zone::WHEEL.iter().filter_map(|z| z.ring()).collect();
        for i in 0..=20 {
            for j in 0..=8 {
                let p = pos2(stop.left() + stop.width() * i as f32 / 20.0, stop.top() + stop.height() * j as f32 / 8.0);
                assert!(cells.iter().all(|c| !c.contains(p.to_vec2())), "{p:?} on a cell");
                assert!(p.to_vec2().length() > 110.0, "{p:?} on VRChat's wheel");
            }
        }
        for (w, h) in SIZES {
            assert!(place(Zone::WheelStop, vec2(w, h)).is_some(), "{w}x{h}: no room under the ring");
        }
    }

    /// REQUEST13: the share column beside VRChat's camera on a screen is there at 1280 x 720 (the panel measured on
    /// `p1-stream-camera`), past VRChat's arrow, under nothing of VRChat's row, inside the window.
    #[test]
    fn the_share_column_is_right_of_the_camera_at_720() {
        let panel = Rect::from_min_max(pos2(332.0, 185.0), pos2(948.0, 535.0));
        let placed = camera_on_screen(panel, vec2(1280.0, 720.0));
        let share = placed.iter().find(|p| p.zone == Zone::CameraShareDesk).expect("placed at 720");
        assert!(share.rect.left() >= panel.right() + DESK_CAMERA_ARROW + 10.0, "{:?}", share.rect);
        assert!(share.rect.right() <= 1280.0 && share.rect.bottom() <= 720.0, "{:?}", share.rect);
        assert!((share.logical() - Zone::CameraShareDesk.units().size()).length() < 2.5);
    }

}
