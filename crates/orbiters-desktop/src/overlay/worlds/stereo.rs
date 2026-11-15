//! A 3D capture around the person in SteamVR, where VRChat owns the scene and only overlays can be shown: a cube of six
//! world-locked overlays around the head, each a side-by-side stereo picture (its left half for the left eye, its right
//! half for the right one). Each half is the capture projected from that eye's position onto the face's plane, an
//! off-axis pinhole camera through the face ([`Cube::camera`]). Such a picture is right for every direction the eye
//! looks from that position, so SteamVR's own reprojection of the world-locked quads keeps head turns exact at the
//! headset's rate, whatever the rate the pictures are drawn at; only moving the eyes needs new pictures (parallax). The
//! cube stays put while the head moves inside it and a second one takes over once the head wanders off its centre
//! ([`Cube::holds`]): six new faces shown together, the old ones hidden after (docs/world-view-overlay.md).
//!
//! Coordinates: SteamVR's standing play space (right-handed, +Y up, metres) for the cube, the eyes and the overlays;
//! the renderer's (Unity's world: the capture's splats, left-handed) for the capture, which [`Placement`] maps into
//! the play space.

use orbiters_openvr::Matrix34;
use orbiters_reconstruct::camera::Camera;

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

fn mat_vec(m: &[[f32; 3]; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

pub fn mat_mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// A pose's position.
pub fn position(m: &Matrix34) -> V3 {
    [m.m[0][3], m.m[1][3], m.m[2][3]]
}

/// A pose's axis `i` (0: right, 1: up, 2: back).
pub fn axis(m: &Matrix34, i: usize) -> V3 {
    [m.m[0][i], m.m[1][i], m.m[2][i]]
}

/// `a` then `b` (`a · b`: `b` expressed in `a`'s space, as eye-to-head after head-to-play-space).
pub fn compose(a: &Matrix34, b: &Matrix34) -> Matrix34 {
    let mut out = Matrix34::IDENTITY;
    for i in 0..3 {
        for j in 0..4 {
            let mut v: f32 = (0..3).map(|k| a.m[i][k] * b.m[k][j]).sum();
            if j == 3 {
                v += a.m[i][3];
            }
            out.m[i][j] = v;
        }
    }
    out
}

/// Eye height the contracts take when a capture does not say (`Docs/capture-sharing.md`, Start view).
pub const DEFAULT_EYE_HEIGHT: f32 = 1.6;

/// A capture's start view as Orbiters lists it: right-handed (Unity's world with Z negated), `floor` the ground the
/// person who captured stood on and `eye_height` their eyes above it, when the upload said.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartView {
    pub position: V3,
    pub target: V3,
    pub floor: Option<f32>,
    pub eye_height: Option<f32>,
}

impl StartView {
    /// The start eye in the renderer's coordinates (Unity's).
    pub fn eye(&self) -> V3 {
        [self.position[0], self.position[1], -self.position[2]]
    }

    /// The height of the ground under the start eye (the capture's own, Unity's Y). The world viewer keeps a capture's
    /// own heights when the upload did not say (its floor at the world's ground); here there is no world ground to
    /// keep, so the start eye stands at the eye height above it, 1.6 m when that is unknown too.
    pub fn ground(&self) -> f32 {
        self.floor.unwrap_or(self.position[1] - self.eye_height.unwrap_or(DEFAULT_EYE_HEIGHT))
    }

    /// The way the start view looks, as a yaw in Unity's convention (from +Z towards +X), 0 when it looks straight
    /// up or down.
    pub fn yaw(&self) -> f32 {
        let (eye, target) = (self.eye(), [self.target[0], self.target[1], -self.target[2]]);
        let d = sub(target, eye);
        if d[0] * d[0] + d[2] * d[2] > 1e-6 {
            d[0].atan2(d[2])
        } else {
            0.0
        }
    }
}

/// Where a capture stands in the play space: renderer coordinates (Unity's) to SteamVR's standing ones,
/// `p ↦ size · rotation · p + offset` (a reflection: Unity is left-handed). Seen from an eye, a capture at a size is the
/// same picture as the capture at its own size seen from the eye's place divided by that size, all of it nearer or
/// farther by the same factor: only the camera's translation knows of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub rotation: [[f32; 3]; 3],
    pub offset: V3,
    pub size: f32,
}

impl Placement {
    /// As the world viewer places a capture on its anchor (SplatViewer.AlignToView), with the anchor where `head`
    /// stands on the play space's floor (Y 0 of the standing universe) and facing the way `head` looks, level: the
    /// start view's ground on the floor, its eye over the head's place, looking the way the head looks.
    pub fn at(view: &StartView, head: &Matrix34) -> Self {
        let h = position(head);
        // The head looks along its −Z; level, then in Unity's coordinates (Z negated).
        let back = axis(head, 2);
        let forward_unity = [-back[0], 0.0, back[2]];
        let facing = if forward_unity[0].abs() + forward_unity[2].abs() > 1e-6 { forward_unity[0].atan2(forward_unity[2]) } else { 0.0 };
        let turn = facing - view.yaw();
        let (s, c) = turn.sin_cos();
        // Unity's rotation about +Y by `turn` (+Z towards +X), then Z negated into SteamVR's.
        let rotation = [[c, 0.0, s], [0.0, 1.0, 0.0], [s, 0.0, -c]];
        let eye = view.eye();
        let ground_point = [eye[0], view.ground(), eye[2]];
        let anchor = [h[0], 0.0, h[2]];
        Self { rotation, offset: sub(anchor, mat_vec(&rotation, ground_point)), size: 1.0 }
    }

    #[cfg(test)]
    pub fn apply(&self, p: V3) -> V3 {
        add(scale(mat_vec(&self.rotation, p), self.size), self.offset)
    }

    /// A play-space point back in the capture's coordinates (the rotation is orthonormal).
    #[cfg(test)]
    pub fn invert(&self, q: V3) -> V3 {
        let d = scale(sub(q, self.offset), 1.0 / self.size);
        let r = &self.rotation;
        [r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2], r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2], r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2]]
    }
}

/// Where on its plate's picture, from its middle to its edge, the rim lies: what the capture reaches.
pub const SMALL_PLATE_RIM: f32 = 0.86;

/// How far from its middle a capture standing small reaches, in the room, once out of the panel.
pub const SMALL_RADIUS_M: f32 = 0.26;
/// How much further than that a hand can take hold of it; how far from its place in the panel it goes back there when let go.
const SMALL_REACH_M: f32 = 0.15;
pub const SMALL_DOCKS_WITHIN_M: f32 = 0.12;
/// How many of what it reaches a touchpad held all the way moves its middle a second.
const SMALL_MOVES_REACHES_PER_S: f32 = 0.9;
/// How much a full stick zooms a second, and how far a thumb's rest does nothing.
const SMALL_ZOOM_PER_S: f32 = 1.4;
const SMALL_STICK_REST: f32 = 0.25;
/// How much of a capture it can show around its middle, in the capture's metres: from a table to a town.
const SMALL_REACHES: (f32, f32) = (0.5, 150.0);
/// How fast it settles where it goes, and how near that is there.
const SMALL_SETTLES_PER_S: f32 = 14.0;
const SMALL_SETTLED_M: f32 = 0.002;
const SMALL_TREMBLE_M: f32 = 0.003;

/// How much a capture in a panel's hole is tipped towards who looks, and one out in the room: held below the eyes that one is seen
/// from above already.
// Seen from low in its hole: what stands in it reads as a place, where from 30 degrees up it read as a brown round of floor (round 18's judges).
const SMALL_TILT_IN_A_HOLE: f32 = 25.0;
const SMALL_TILT_IN_THE_ROOM: f32 = 14.0;

/// How far under a hole's middle a capture's floor lies in it, in its radius: low enough that what stands as high as most captures do (0.7 of the radius) ends under the
/// hole's top (Enzo's request 60: the upper part is not cut), the floor's near edge going under the hole's foot.
const SMALL_FLOOR_UNDER_THE_MIDDLE: f32 = 0.2;
/// From how far out of its reach a capture standing small in the room fades to nothing at its edge.
const SMALL_FADES_FROM: f32 = 0.8;
/// How deep the soft near edge of a capture in a panel's hole is, in its reach: as deep as the round's edge in the room fades.
const HOLE_NEAR_FADE_PER_REACH: f32 = 1.0 - SMALL_FADES_FROM;
/// How far the near edge of a capture in a panel's hole recedes towards its sides, in its reach: back to about its middle's depth, then level.
const HOLE_NEAR_RECEDES_PER_REACH: f32 = 1.0;

/// What of a capture standing small is drawn. Each splat's share of it is worked out once (`FaceRenderer::clip`, the renderer's
/// marks), never per eye or per frame: both eyes see the same edge, and nothing pops as the head moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cut {
    /// In the room (in a hand, or let go there): a round across the ground around its middle, `reach` wide, its last part fading.
    Round { middle: V3, reach: f32 },
    /// In a panel's hole: only its near side is cut (Enzo, 2026-11-13, his mockup): a surface that comes nearest who looks at the middle, where the
    /// round's near edge was, always behind the panel's plane, and recedes smoothly towards the sides; beyond it, away from who looks, all of it,
    /// to the capture's far end. The hole is a window onto the place going on into the distance, and none of it comes out in front of the menu.
    ///
    /// In the capture's coordinates, as [`SmallDock::behind`] lays it in a hole: `ground` the ground under its middle, which lies `reach` behind
    /// the panel's plane; `toward` the panel's normal towards who looks (level the other way from the start view, tipped up by the hole's tilt);
    /// `across` the panel's across. A capture in a hole turns with the panel, so these never change as the panel or the head moves.
    Window { ground: V3, toward: V3, across: V3, reach: f32 },
}

impl Cut {
    /// What `small`, a capture whose start view is `view`, draws: in its hole, or out in the room.
    pub fn of(small: &Small, view: &StartView) -> Self {
        let middle = small.middle(view.eye());
        if !small.docked {
            return Cut::Round { middle, reach: small.reach() };
        }
        // SmallDock::behind's rotation is the panel's, times the tilt, times the turn by the start view's yaw: the panel's own axes, in the
        // capture, are that turn's and tilt's inverse of them.
        let (s, c) = (-view.yaw()).sin_cos();
        let (ts, tc) = SMALL_TILT_IN_A_HOLE.to_radians().sin_cos();
        Cut::Window { ground: [middle[0], view.ground(), middle[2]], toward: [s * tc, ts, -c * tc], across: [c, 0.0, s], reach: small.reach() }
    }

    /// How much of the splat whose middle is `p` (the capture's coordinates) is left out: 0 drawn whole, 1 not drawn, and in between its soft edge.
    pub fn gone(&self, p: V3) -> f32 {
        match *self {
            Cut::Round { middle, reach } => {
                // Across the ground only: moved, it shows the same round as before, with nothing cut for its height.
                let far = (p[0] - middle[0]).hypot(p[2] - middle[2]);
                ((far - reach * SMALL_FADES_FROM) / (reach * (1.0 - SMALL_FADES_FROM))).clamp(0.0, 1.0)
            }
            Cut::Window { ground, toward, across, reach } => {
                let q = sub(p, ground);
                // How far behind the panel's plane, and how far aside.
                let behind = reach - dot(q, toward);
                let aside = dot(q, across);
                (((Self::near_edge(reach, aside) + reach * HOLE_NEAR_FADE_PER_REACH) - behind) / (reach * HOLE_NEAR_FADE_PER_REACH)).clamp(0.0, 1.0)
            }
        }
    }

    /// Where a hole's near edge lies, behind the panel's plane, `aside` from its middle (the capture's metres; drawn whole from a fade's depth
    /// further). In the middle: as near as the round's near edge on the ground was (the ground under the middle is `reach` behind the plane, the
    /// edge `reach` nearer along the ground, tipped by the hole's tilt), so the floor starts where it did. Aside: a smooth curve back, round
    /// like the round's edge near the middle, then level about the middle's depth: seen from aside through the hole, what lies beside the window
    /// is no nearer than what lies ahead.
    pub fn near_edge(reach: f32, aside: f32) -> f32 {
        let recedes = reach * HOLE_NEAR_RECEDES_PER_REACH;
        reach * (1.0 - SMALL_TILT_IN_A_HOLE.to_radians().cos()) + recedes * (1.0 - (-aside * aside / (2.0 * recedes * reach)).exp())
    }

    /// Whether every splat drawn by `self` is drawn by `other` as much, to see: no need to work the marks out again.
    pub fn same_as(&self, other: &Cut) -> bool {
        let near = |a: f32, b: f32, reach: f32| (a - b).abs() < reach * 0.02;
        match (*self, *other) {
            (Cut::Round { middle: a, reach: r }, Cut::Round { middle: b, reach: q }) => near(a[0], b[0], r) && near(a[2], b[2], r) && near(r, q, r),
            (Cut::Window { ground: a, toward: t, reach: r, .. }, Cut::Window { ground: b, toward: u, reach: q, .. }) => {
                near(a[0], b[0], r) && near(a[1], b[1], r) && near(a[2], b[2], r) && near(r, q, r) && dot(t, u) > 0.9999
            }
            _ => false,
        }
    }

    /// Whether it may draw splats however far from its middle: the first ones by distance ([`super::catalog::Capture::within`]) are not enough.
    pub fn reaches_the_far_end(&self) -> bool {
        matches!(self, Cut::Window { .. })
    }
}

/// Where a capture standing small lies in a panel: as that panel turns, this large, this far behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmallDock {
    pub rotation: [[f32; 3]; 3],
    pub anchor: V3,
    pub radius: f32,
}

impl SmallDock {
    /// Behind the middle of a hole in a panel, whose place is `at` (its Z towards who looks), looking into the panel as its
    /// start view looks, for a capture shown `reach` metres around its middle: its floor at one place in the hole however near it shows.
    pub fn behind(view: &StartView, at: &Matrix34, radius: f32, reach: f32) -> Self {
        let (s, c) = (-view.yaw()).sin_cos();
        let turned = [[c, 0.0, s], [0.0, 1.0, 0.0], [s, 0.0, -c]];
        // Tipped towards who looks: a panel faces them, and a scene on it seen level would be seen edge on.
        let (ts, tc) = SMALL_TILT_IN_A_HOLE.to_radians().sin_cos();
        let tipped = [[1.0, 0.0, 0.0], [0.0, tc, -ts], [0.0, ts, tc]];
        // Its floor in the hole's lower part, clear of the hole's lower edge: what stands on it has the rest. Its middle is its eye, that high over its floor at its size.
        let rotation = mat_mul(&rotation_of(at), &mat_mul(&tipped, &turned));
        let floor = sub(position(at), scale(axis(at, 1), radius * SMALL_FLOOR_UNDER_THE_MIDDLE));
        let up = [rotation[0][1], rotation[1][1], rotation[2][1]];
        let raised = add(floor, scale(up, (view.eye()[1] - view.ground()) * radius / reach.max(1e-3)));
        // Its whole round behind the panel's plane: seen from low its near edge comes as far forward as its middle is back, and what is nearer than the panel while
        // the panel covers it would strain the eyes.
        Self { rotation, anchor: sub(raised, scale(axis(at, 2), radius)), radius }
    }
}

/// A capture standing small before the person, as Enzo asked: "a small element in front of us that we can grab with the
/// other hand, zoom using the joystick (up / down)". Zooming, it stays as large in the room: closer, it shows less of the
/// capture, larger. What is farther from its middle than it reaches is not drawn. In a panel with a place for it, it
/// follows that place until a hand takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Small {
    pub rotation: [[f32; 3]; 3],
    /// Where its middle is, in the room.
    pub anchor: V3,
    /// How far around its middle it shows the capture, in the capture's metres.
    reach: f32,
    /// How far it reaches in the room now.
    radius: f32,
    held: Option<SmallHold>,
    grips: [bool; 2],
    /// A hand took it out of its place, and it has not gone back.
    pub taken: bool,
    /// It is in a place in a panel, or on its way there; there.
    pub docked: bool,
    settled: bool,
    /// A hand is near enough to take it.
    pub near: bool,
    /// A stick zoomed it.
    pub zoomed: bool,
    /// How far its middle was moved from the capture's start eye, along the capture's ground (X and Z), in the capture's metres.
    moved: [f32; 2],
}

/// What a stick that turns does, as VRChat has it: smoothly, or a step each push.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnStick {
    pushed: bool,
}

/// How fast a stick all the way turns, and how far a step goes, in radians. Not read from VRChat, which does not say: about what it does.
const TURNS_PER_S: f32 = 1.75;
const TURN_STEP: f32 = std::f32::consts::FRAC_PI_6;
/// Under this a stick is at rest, and a step needs this far, and a return under the first to go again.
const TURN_REST: f32 = 0.2;
const TURN_PUSHED: f32 = 0.7;

impl TurnStick {
    /// How far the world turns now, in radians about up: pushed right, what is ahead goes to the left.
    pub fn step(&mut self, stick: f32, seconds: f32, in_steps: bool) -> f32 {
        if in_steps {
            if stick.abs() < TURN_REST {
                self.pushed = false;
            } else if stick.abs() >= TURN_PUSHED && !self.pushed {
                self.pushed = true;
                return TURN_STEP * stick.signum();
            }
            return 0.0;
        }
        if stick.abs() < TURN_REST {
            return 0.0;
        }
        (stick.abs() - TURN_REST) / (1.0 - TURN_REST) * stick.signum() * TURNS_PER_S * seconds
    }
}

/// How far apart two rotations are, about as radians for small ones.
fn turned_away(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> f32 {
    let mut trace = 0.0;
    for c in 0..3 {
        for row in 0..3 {
            trace += a[row][c] * b[row][c];
        }
    }
    (((trace - 1.0) / 2.0).clamp(-1.0, 1.0)).acos()
}

/// `placement` turned by `radians` about the vertical through `about`.
pub fn turned_about(placement: &Placement, about: V3, radians: f32) -> Placement {
    let (s, c) = radians.sin_cos();
    let turn = [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]];
    Placement { rotation: mat_mul(&turn, &placement.rotation), offset: add(about, mat_vec(&turn, sub(placement.offset, about))), size: placement.size }
}

/// A menu standing in the room, as far as what is drawn over it goes: its place (its middle, Z towards who looks) and size in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuBox {
    pub place: Matrix34,
    pub width: f32,
    pub height: f32,
}

/// The hand holding it, and it as that hand holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SmallHold {
    hand: usize,
    rotation: [[f32; 3]; 3],
    anchor: V3,
}

fn rotation_of(m: &Matrix34) -> [[f32; 3]; 3] {
    [[m.m[0][0], m.m[0][1], m.m[0][2]], [m.m[1][0], m.m[1][1], m.m[1][2]], [m.m[2][0], m.m[2][1], m.m[2][2]]]
}

fn transposed(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

/// `k` of the way from rotation `a` to `b`, still a rotation (or reflection, as they both are).
fn towards(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3], k: f32) -> [[f32; 3]; 3] {
    let mix: Vec<V3> = (0..3).map(|i| add(scale([a[0][i], a[1][i], a[2][i]], 1.0 - k), scale([b[0][i], b[1][i], b[2][i]], k))).collect();
    let unit = |v: V3| scale(v, 1.0 / length(v).max(1e-5));
    let x = unit(mix[0]);
    let y = unit(sub(mix[1], scale(x, dot(mix[1], x))));
    let z = unit(sub(sub(mix[2], scale(x, dot(mix[2], x))), scale(y, dot(mix[2], y))));
    [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
}

impl Small {
    /// Half a metre before `head`, at its chest, turned as the capture would be around it, showing `reach` metres of
    /// the capture around its middle.
    pub fn new(view: &StartView, head: &Matrix34, reach: f32) -> Self {
        let back = axis(head, 2);
        let level = length([back[0], 0.0, back[2]]).max(1e-3);
        let ahead = [-back[0] / level, 0.0, -back[2] / level];
        // Tipped towards who looks, about the level line across them.
        let across = [-ahead[2], 0.0, ahead[0]];
        let (ts, tc) = SMALL_TILT_IN_THE_ROOM.to_radians().sin_cos();
        let frame = [[across[0], 0.0, -ahead[0]], [across[1], 1.0, -ahead[1]], [across[2], 0.0, -ahead[2]]];
        let tipped = mat_mul(&frame, &mat_mul(&[[1.0, 0.0, 0.0], [0.0, tc, -ts], [0.0, ts, tc]], &transposed(&frame)));
        let rotation = mat_mul(&tipped, &Placement::at(view, head).rotation);
        let anchor = add(add(position(head), scale(ahead, 0.55)), [0.0, -0.15, 0.0]);
        Self { rotation, anchor, reach: reach.clamp(SMALL_REACHES.0, SMALL_REACHES.1), moved: [0.0; 2], radius: SMALL_RADIUS_M, held: None, grips: [false; 2], taken: false, docked: false, settled: false, near: false, zoomed: false }
    }

    /// Already in its place in a panel, as when it first shows there.
    pub fn at_dock(mut self, dock: &SmallDock) -> Self {
        (self.rotation, self.anchor, self.radius) = (dock.rotation, dock.anchor, dock.radius);
        (self.docked, self.settled) = (true, true);
        self
    }

    /// Showing this of the capture: its middle so far from the start eye across the ground, reaching so far ([`Capture::hole_frame`], for its place in a panel).
    pub fn framed(mut self, (moved, reach): ([f32; 2], f32)) -> Self {
        (self.moved, self.reach) = (moved, reach.clamp(SMALL_REACHES.0, SMALL_REACHES.1));
        self
    }

    /// Shows `share` of what it shows now, larger, with no hand asking.
    pub fn zoomed_in(mut self, share: f32) -> Self {
        self.reach = (self.reach * share).clamp(SMALL_REACHES.0, SMALL_REACHES.1);
        self
    }

    /// Where the plate it stands on lies, for ground `below` its middle in the capture's metres, and how wide: its picture's face
    /// up along the capture's up.
    pub fn plate(&self, below: f32) -> (Matrix34, f32) {
        let r = &self.rotation;
        let (x, up) = ([r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]]);
        let y = cross(up, x);
        let at = sub(self.anchor, scale(up, below * self.size()));
        (Matrix34 { m: [[x[0], y[0], up[0], at[0]], [x[1], y[1], up[1], at[1]], [x[2], y[2], up[2], at[2]]] }, self.radius * 2.0 / SMALL_PLATE_RIM)
    }

    /// How large it stands against the capture.
    pub fn size(&self) -> f32 {
        self.radius / self.reach
    }

    /// Where a capture whose middle is `middle` stands.
    pub fn placement(&self, middle: V3) -> Placement {
        let size = self.size();
        let middle = self.middle(middle);
        Placement { rotation: self.rotation, offset: sub(self.anchor, scale(mat_vec(&self.rotation, middle), size)), size }
    }

    /// Where its middle is in the capture, from the capture's start eye.
    pub fn middle(&self, eye: V3) -> V3 {
        add(eye, [self.moved[0], 0.0, self.moved[1]])
    }

    /// How far its middle was moved.
    pub fn moved_by(&self) -> f32 {
        self.moved[0].hypot(self.moved[1])
    }

    /// One frame of the touchpad of the hand holding it: its middle goes the way the thumb pushes, as that hand points, along the capture's ground (Enzo: no up and down, for
    /// now), at a pace that suits how much shows. No farther from the start than `limit`, where the capture ends.
    pub fn move_middle(&mut self, hands: [Option<Matrix34>; 2], pads: [[f32; 2]; 2], seconds: f32, limit: f32) {
        let Some(hold) = self.held else { return };
        let Some(pose) = hands[hold.hand].as_ref() else { return };
        self.move_along(pose, pads[hold.hand], seconds, limit);
    }

    /// One frame of the stick of a hand whose laser is on it where it lies (in a panel's hole, or left in the room): its middle goes the way the thumb pushes, as that
    /// hand points, as when held (Enzo, K-small-stick: on his Touch controllers moving in it "doesn't work at all", it moved only held, with the other hand's stick,
    /// which walks in VRChat). Not while a hand holds it: the held routes have it then.
    pub fn move_pointed(&mut self, pose: &Matrix34, pad: [f32; 2], seconds: f32, limit: f32) {
        if self.held.is_none() {
            self.move_along(pose, pad, seconds, limit);
        }
    }

    fn move_along(&mut self, pose: &Matrix34, pad: [f32; 2], seconds: f32, limit: f32) {
        if pad[0].hypot(pad[1]) <= SMALL_STICK_REST {
            return;
        }
        let world = add(scale(axis(pose, 0), pad[0]), scale(axis(pose, 2), -pad[1]));
        let inside = mat_vec(&transposed(&self.rotation), world);
        let level = inside[0].hypot(inside[2]);
        if level < 1e-3 {
            return;
        }
        let pace = pad[0].hypot(pad[1]).min(1.0) * self.reach * SMALL_MOVES_REACHES_PER_S * seconds;
        let to = [self.moved[0] + inside[0] / level * pace, self.moved[1] + inside[2] / level * pace];
        let far = to[0].hypot(to[1]);
        self.moved = if far > limit { [to[0] * limit / far, to[1] * limit / far] } else { to };
    }

    /// Whether a hand took it from its place.
    pub fn taken(&self) -> bool {
        self.taken
    }

    /// How far from its middle it shows the capture, in the capture's metres.
    pub fn reach(&self) -> f32 {
        self.reach
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }

    pub fn held(&self) -> bool {
        self.held.is_some()
    }

    /// The hand holding it.
    pub fn held_by(&self) -> Option<usize> {
        self.held.map(|h| h.hand)
    }

    /// Back to its place in the panel, from wherever a hand left it.
    pub fn bring_back(&mut self) {
        (self.taken, self.settled, self.held) = (false, false, None);
    }

    /// One frame of the hands: their poses, grips and sticks' up and down, and where in a panel it may lie. A grip closing
    /// near it takes it; it follows that hand until the grip opens, going back to its place if let go near it; that hand's
    /// stick zooms it.
    pub fn step(&mut self, hands: [Option<Matrix34>; 2], grips: [bool; 2], sticks: [f32; 2], seconds: f32, dock: Option<SmallDock>) {
        let was = std::mem::replace(&mut self.grips, grips);
        if let Some(hold) = self.held {
            if !grips[hold.hand] || hands[hold.hand].is_none() {
                self.held = None;
                if dock.is_some_and(|d| length(sub(self.anchor, d.anchor)) <= SMALL_DOCKS_WITHIN_M) {
                    self.taken = false;
                    self.settled = false;
                }
            }
        }
        let grasp = self.radius.max(0.05) + SMALL_REACH_M;
        let within = |pose: &Matrix34| length(sub(position(pose), self.anchor)) <= grasp;
        self.near = self.held.is_none() && hands.iter().flatten().any(within);
        if self.held.is_none() {
            for hand in 0..2 {
                let Some(pose) = hands[hand].as_ref().filter(|_| grips[hand] && !was[hand]) else { continue };
                if within(pose) {
                    let inverse = transposed(&rotation_of(pose));
                    self.held = Some(SmallHold { hand, rotation: mat_mul(&inverse, &self.rotation), anchor: mat_vec(&inverse, sub(self.anchor, position(pose))) });
                    self.taken = true;
                    self.settled = false;
                    self.near = false;
                    break;
                }
            }
        }
        let settling = 1.0 - (-seconds * SMALL_SETTLES_PER_S).exp();
        self.docked = false;
        if let Some(hold) = self.held {
            if let Some(pose) = hands[hold.hand].as_ref() {
                let turned = rotation_of(pose);
                self.rotation = mat_mul(&turned, &hold.rotation);
                self.anchor = add(mat_vec(&turned, hold.anchor), position(pose));
                let stick = sticks[hold.hand];
                if stick.abs() > SMALL_STICK_REST {
                    self.reach = (self.reach * (-stick * SMALL_ZOOM_PER_S * seconds).exp()).clamp(SMALL_REACHES.0, SMALL_REACHES.1);
                    self.zoomed = true;
                }
            }
            self.radius += (SMALL_RADIUS_M - self.radius) * settling;
            return;
        }
        match dock.filter(|_| !self.taken) {
            Some(d) => {
                self.docked = true;
                if self.settled {
                    // A hand holding the menu always trembles, and every move of this draws it all again, late: only a real move counts.
                    if length(sub(self.anchor, d.anchor)) > SMALL_TREMBLE_M || (self.radius - d.radius).abs() > SMALL_TREMBLE_M || turned_away(&self.rotation, &d.rotation) > 0.01 {
                        (self.rotation, self.anchor, self.radius) = (d.rotation, d.anchor, d.radius);
                    }
                } else {
                    self.rotation = towards(&self.rotation, &d.rotation, settling);
                    self.anchor = add(self.anchor, scale(sub(d.anchor, self.anchor), settling));
                    self.radius += (d.radius - self.radius) * settling;
                    self.settled = length(sub(self.anchor, d.anchor)) < SMALL_SETTLED_M && (self.radius - d.radius).abs() < SMALL_SETTLED_M;
                    if self.settled {
                        (self.rotation, self.anchor, self.radius) = (d.rotation, d.anchor, d.radius);
                    }
                }
            }
            None => self.radius += (SMALL_RADIUS_M - self.radius) * settling,
        }
    }
}

/// A face of the cube: its outward normal and the up of its picture (right = normal × up, as the eye inside sees it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub normal: V3,
    pub up: V3,
}

impl Face {
    pub fn right(&self) -> V3 {
        cross(self.normal, self.up)
    }
}

/// The six faces: ahead (−Z), right, behind, left, up, down. Up and down keep +X as their right.
pub const FACES: [Face; 6] = [
    Face { normal: [0.0, 0.0, -1.0], up: [0.0, 1.0, 0.0] },
    Face { normal: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] },
    Face { normal: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] },
    Face { normal: [-1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] },
    Face { normal: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0] },
    Face { normal: [0.0, -1.0, 0.0], up: [0.0, 0.0, -1.0] },
];

/// The cube of overlays: centred at `center` in the play space, its faces `half` metres from it, each picture
/// `pixels` across per eye plus `margin` on every side, past the face: neighbouring faces overlap by two margins, so
/// SteamVR's filtering at their edges never shows a seam (both draw the same there).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cube {
    pub center: V3,
    pub half: f32,
    pub pixels: u32,
    pub margin: u32,
}

impl Cube {
    /// Pixels across one eye's picture of a face, margins included (the texture is twice as wide).
    pub fn width(&self) -> u32 {
        self.pixels + 2 * self.margin
    }

    /// Half the side of a face's quad, margins included.
    pub fn extent(&self) -> f32 {
        self.half * self.width() as f32 / self.pixels as f32
    }

    /// The overlay transform of face `f`: its quad's +X its right, +Y its up, +Z towards the inside (an overlay shows its
    /// front towards its +Z).
    pub fn transform(&self, f: usize) -> Matrix34 {
        let face = FACES[f];
        let (x, y, z) = (face.right(), face.up, scale(face.normal, -1.0));
        let p = add(self.center, scale(face.normal, self.half));
        Matrix34 { m: [[x[0], y[0], z[0], p[0]], [x[1], y[1], z[1], p[1]], [x[2], y[2], z[2], p[2]]] }
    }

    /// The quad's width in metres (SteamVR takes a side-by-side overlay's height from one half's aspect: square here).
    pub fn quad_width(&self) -> f32 {
        2.0 * self.extent()
    }

    /// The camera that draws face `f`'s picture for an eye at `eye` (play space), of a capture placed by `placement`:
    /// the eye's projection onto the face's plane, pixel (0, 0) the quad's top left. `eye` must be inside the cube.
    pub fn camera(&self, f: usize, eye: V3, placement: &Placement) -> Camera {
        let face = FACES[f];
        let (right, down, normal) = (face.right(), scale(face.up, -1.0), face.normal);
        let rel = sub(eye, self.center);
        let distance = self.half - dot(rel, normal);
        let w = self.width();
        let per_metre = w as f32 / (2.0 * self.extent());
        let focal = distance * per_metre;
        let mid = w as f32 / 2.0 - 0.5;
        // World (play space) to camera rows: right, down, forward; then the capture into the play space.
        let rows = [right, down, normal];
        let r = mat_mul(&rows, &placement.rotation);
        let t = scale(mat_vec(&rows, sub(placement.offset, eye)), 1.0 / placement.size);
        Camera { r, t, fx: focal, fy: focal, cx: mid + dot(rel, right) * per_metre, cy: mid + dot(rel, down) * per_metre, width: w, height: w }
    }

    /// Whether `eye` is within `reach` metres of the centre on every axis (else a new cube should take over).
    pub fn holds(&self, eye: V3, reach: f32) -> bool {
        let rel = sub(eye, self.center);
        rel.iter().all(|v| v.abs() <= reach)
    }

    /// The face a ray from `eye` along `dir` leaves the cube through (`eye` inside).
    pub fn face_hit(&self, eye: V3, dir: V3) -> usize {
        let rel = sub(eye, self.center);
        let mut best = (0, f32::INFINITY);
        for (i, face) in FACES.iter().enumerate() {
            let along = dot(dir, face.normal);
            if along > 1e-9 {
                let t = (self.half - dot(rel, face.normal)) / along;
                if t < best.1 {
                    best = (i, t);
                }
            }
        }
        best.0
    }

    /// Where a ray from `eye` along `dir` meets face `f`'s quad, in its picture's pixels (margins included), or `None`
    /// past the quad or behind the eye.
    pub fn pixel_hit(&self, f: usize, eye: V3, dir: V3) -> Option<[f32; 2]> {
        let face = FACES[f];
        let rel = sub(eye, self.center);
        let along = dot(dir, face.normal);
        if along <= 1e-9 {
            return None;
        }
        let t = (self.half - dot(rel, face.normal)) / along;
        let at = add(rel, scale(dir, t));
        let (a, b) = (dot(at, face.right()) / self.extent(), -dot(at, face.up) / self.extent());
        if a.abs() > 1.0 || b.abs() > 1.0 {
            return None;
        }
        let w = self.width() as f32;
        Some([(a + 1.0) / 2.0 * w - 0.5, (b + 1.0) / 2.0 * w - 0.5])
    }

    /// The faces an eye at `view` (eye to play space) sees with the field of view `tangents` (left, right, top, bottom,
    /// as SteamVR's GetProjectionRaw) widened by `margin` radians on every side: the head may turn that far before the
    /// next pictures. Sampled on a grid of directions over the widened field.
    pub fn faces_seen(&self, view: &Matrix34, tangents: [f32; 4], margin: f32) -> [bool; 6] {
        const STEPS: usize = 11;
        let eye = position(view);
        let (right, up, back) = (axis(view, 0), axis(view, 1), axis(view, 2));
        let widen = |t: f32, out: f32| (t.atan() + out * margin).clamp(-1.45, 1.45);
        let (l, r) = (widen(tangents[0].min(tangents[1]), -1.0), widen(tangents[0].max(tangents[1]), 1.0));
        let (lo, hi) = (widen(tangents[2].min(tangents[3]), -1.0), widen(tangents[2].max(tangents[3]), 1.0));
        let mut seen = [false; 6];
        for i in 0..STEPS {
            for j in 0..STEPS {
                let x = (l + (r - l) * i as f32 / (STEPS - 1) as f32).tan();
                let y = (lo + (hi - lo) * j as f32 / (STEPS - 1) as f32).tan();
                let dir = add(add(scale(right, x), scale(up, y)), scale(back, -1.0));
                seen[self.face_hit(eye, dir)] = true;
            }
        }
        seen
    }
}

/// A pinhole camera in the play space: rows right, down, forward; the eye; intrinsics (pixel centres at integers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayCamera {
    pub rows: [[f32; 3]; 3],
    pub eye: V3,
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub width: u32,
    pub height: u32,
}

impl PlayCamera {
    /// Two calibrated eyes with their own frusta but one buffer size. A one-pixel difference must not recreate the
    /// renderer's image buffers and bindings twice every stereo batch. Equal-sized eyes are unchanged exactly.
    pub fn wide_pair(views: &[Matrix34; 2], tangents: [[f32; 4]; 2], margin: f32, per_tan: f32, max: u32) -> [Self; 2] {
        let mut cameras = [0, 1].map(|e| Self::wide(&views[e], tangents[e], margin, per_tan, max));
        let width = cameras[0].width.max(cameras[1].width);
        let height = cameras[0].height.max(cameras[1].height);
        for c in &mut cameras {
            if c.width != width || c.height != height {
                let (sx, sy) = (width as f32 / c.width as f32, height as f32 / c.height as f32);
                c.fx *= sx;
                c.fy *= sy;
                c.cx = (c.cx + 0.5) * sx - 0.5;
                c.cy = (c.cy + 0.5) * sy - 0.5;
                c.width = width;
                c.height = height;
            }
        }
        cameras
    }

    /// An eye's wide view: its field of view `tangents` widened by `margin` radians each side, `per_tan` pixels per unit
    /// of tangent (the faces' density at their centres matches `pixels / 2`), at most `max` pixels across.
    pub fn wide(view: &Matrix34, tangents: [f32; 4], margin: f32, per_tan: f32, max: u32) -> Self {
        let (right, up, back) = (axis(view, 0), axis(view, 1), axis(view, 2));
        let widen = |t: f32, out: f32| (t.atan() + out * margin).clamp(-1.3, 1.3).tan();
        let (l, r) = (widen(tangents[0].min(tangents[1]), -1.0), widen(tangents[0].max(tangents[1]), 1.0));
        let (lo, hi) = (widen(tangents[2].min(tangents[3]), -1.0), widen(tangents[2].max(tangents[3]), 1.0));
        let per_tan = per_tan.min(max as f32 / (r - l)).min(max as f32 / (hi - lo));
        let (width, height) = (((r - l) * per_tan).round().max(16.0) as u32, ((hi - lo) * per_tan).round().max(16.0) as u32);
        let fx = width as f32 / (r - l);
        let fy = height as f32 / (hi - lo);
        Self { rows: [right, scale(up, -1.0), scale(back, -1.0)], eye: position(view), fx, fy, cx: -l * fx - 0.5, cy: hi * fy - 0.5, width, height }
    }

    /// The renderer's camera for a capture placed by `placement`.
    pub fn camera(&self, placement: &Placement) -> Camera {
        Camera {
            r: mat_mul(&self.rows, &placement.rotation),
            t: scale(mat_vec(&self.rows, sub(placement.offset, self.eye)), 1.0 / placement.size),
            fx: self.fx,
            fy: self.fy,
            cx: self.cx,
            cy: self.cy,
            width: self.width,
            height: self.height,
        }
    }

    /// Where a play-space point lands in its picture, if in front.
    pub fn project(&self, p: V3) -> Option<[f32; 2]> {
        let c = mat_vec(&self.rows, sub(p, self.eye));
        (c[2] > 1e-6).then(|| [self.fx * c[0] / c[2] + self.cx, self.fy * c[1] / c[2] + self.cy])
    }
}

impl Cube {
    /// The homography from face `f`'s texels (x, y, 1), pixel centres at integers, to `wide`'s pixels (homogeneous):
    /// what the eye at `wide.eye` sees through each texel, which `wide` drew from the same place. Resampling a wide
    /// view into the faces is exact but for the filtering, as both are projections from that one point.
    pub fn homography(&self, f: usize, wide: &PlayCamera) -> [[f32; 3]; 3] {
        let face = FACES[f];
        let (right, down) = (face.right(), scale(face.up, -1.0));
        let w = self.width() as f32;
        let step = 2.0 * self.extent() / w;
        let corner = add(
            add(sub(add(self.center, scale(face.normal, self.half)), wide.eye), scale(right, self.extent() * (1.0 / w - 1.0))),
            scale(down, self.extent() * (1.0 / w - 1.0)),
        );
        // Direction from the eye = A · (x, y, 1); camera coordinates = rows · A; pixels = K · that.
        let a = [scale(right, step), scale(down, step), corner];
        let k = [[wide.fx, 0.0, wide.cx], [0.0, wide.fy, wide.cy], [0.0, 0.0, 1.0]];
        let mut ra = [[0f32; 3]; 3];
        for (i, row) in ra.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = dot(wide.rows[i], a[j]);
            }
        }
        mat_mul(&k, &ra)
    }

    /// Whether `wide` sees any of face `f` (its corners and centre, or the face holds the view's axis).
    pub fn face_in(&self, f: usize, wide: &PlayCamera) -> bool {
        let face = FACES[f];
        let e = self.extent();
        let centre = add(self.center, scale(face.normal, self.half));
        let forward = wide.rows[2];
        let inside = |p: Option<[f32; 2]>| p.is_some_and(|[x, y]| x >= 0.0 && y >= 0.0 && x < wide.width as f32 && y < wide.height as f32);
        for a in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
            for b in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
                if inside(wide.project(add(add(centre, scale(face.right(), a * e)), scale(face.up, b * e)))) {
                    return true;
                }
            }
        }
        self.face_hit(wide.eye, forward) == f
    }
}

/// The camera an eye at `view` (eye to play space) with the field of view `tangents` (as [`Cube::faces_seen`]) sees
/// the capture through, `width × height` pixels: what a headset would show, for comparing the cube's pictures with.
pub fn eye_camera(view: &Matrix34, tangents: [f32; 4], width: u32, height: u32, placement: &Placement) -> Camera {
    let (right, up, back) = (axis(view, 0), axis(view, 1), axis(view, 2));
    let (l, r) = (tangents[0].min(tangents[1]), tangents[0].max(tangents[1]));
    let (lo, hi) = (tangents[2].min(tangents[3]), tangents[2].max(tangents[3]));
    let rows = [right, scale(up, -1.0), scale(back, -1.0)];
    let fx = width as f32 / (r - l);
    let fy = height as f32 / (hi - lo);
    Camera {
        r: mat_mul(&rows, &placement.rotation),
        t: scale(mat_vec(&rows, sub(placement.offset, position(view))), 1.0 / placement.size),
        fx,
        fy,
        cx: -l * fx - 0.5,
        cy: hi * fy - 0.5,
        width,
        height,
    }
}

/// The direction (play space) of pixel (`x`, `y`) of [`eye_camera`]'s picture.
pub fn eye_ray(view: &Matrix34, tangents: [f32; 4], width: u32, height: u32, x: f32, y: f32) -> V3 {
    let (right, up, back) = (axis(view, 0), axis(view, 1), axis(view, 2));
    let (l, r) = (tangents[0].min(tangents[1]), tangents[0].max(tangents[1]));
    let (lo, hi) = (tangents[2].min(tangents[3]), tangents[2].max(tangents[3]));
    let tx = l + (x + 0.5) / width as f32 * (r - l);
    let ty = hi - (y + 0.5) / height as f32 * (hi - lo);
    add(add(scale(right, tx), scale(up, ty)), scale(back, -1.0))
}

/// How far the capture shows while it appears, from its start view outward like the world viewer's reveal
/// (SplatViewer.Reveal): the radius grows on a log scale over `seconds`, from 0 to past `farthest` (metres).
pub fn reveal_radius(elapsed: f32, seconds: f32, farthest: f32) -> f32 {
    let progress = (elapsed / seconds.max(0.01)).clamp(0.0, 1.0);
    // Ease out, as the panel's motion: fast first, settling at the far end.
    let eased = 1.0 - (1.0 - progress).powi(3);
    ((1.0 + farthest.max(1.0)).ln() * eased * 1.02).exp() - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_eye_size_preserves_both_frusta_and_identical_symmetric_cameras() {
        let views = [Matrix34::IDENTITY; 2];
        let t = [[-1.39, 1.24, -1.47, 1.45], [-1.24, 1.39, -1.47, 1.45]];
        let old = [0, 1].map(|e| PlayCamera::wide(&views[e], t[e], 0.105, 409.6, 3072));
        assert_eq!(PlayCamera::wide_pair(&views, t, 0.105, 409.6, 3072), old);
        let unequal = [t[0], [-1.239, 1.401, -1.471, 1.455]];
        let separate = [0, 1].map(|e| PlayCamera::wide(&views[e], unequal[e], 0.105, 409.6, 3072));
        let common = PlayCamera::wide_pair(&views, unequal, 0.105, 409.6, 3072);
        assert_eq!((common[0].width, common[0].height), (common[1].width, common[1].height));
        for e in 0..2 {
            for p in [[0.0, 0.0, -2.0], [0.8, -0.7, -1.0]] {
                let a = separate[e].project(p).unwrap();
                let b = common[e].project(p).unwrap();
                for (axis, old_size, new_size) in [(0, separate[e].width, common[e].width), (1, separate[e].height, common[e].height)] {
                    assert!(((a[axis] + 0.5) / old_size as f32 - (b[axis] + 0.5) / new_size as f32).abs() < 1e-6);
                }
            }
        }
    }

    fn close(a: V3, b: V3, tol: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
    }

    /// A capture at a size looks from an eye exactly as if it were that large: what the renderer is given says the same.
    #[test]
    fn a_placement_at_a_size_projects_where_the_point_stands_in_the_room() {
        let view = StartView { position: [1.0, 2.0, 3.0], target: [4.0, 1.0, 2.0], floor: None, eye_height: None };
        let head = head_at(0.3, 1.7, -0.2, 0.7);
        let small = Small::new(&view, &head, 6.0);
        let middle = view.eye();
        let placement = small.placement(middle);
        assert!(close(placement.apply(middle), small.anchor, 1e-5), "its middle stands at its anchor");
        // A point as far from the middle as it shows is as far from its anchor as it reaches in the room.
        let far = placement.apply(add(middle, [small.reach(), 0.0, 0.0]));
        assert!((length(sub(far, small.anchor)) - SMALL_RADIUS_M).abs() < 1e-4);
        // Through the camera, where the room says.
        let tangents = [-1.39, 1.24, -1.47, 1.45];
        let camera = eye_camera(&head, tangents, 800, 800, &placement);
        let direct = eye_camera(&head, tangents, 800, 800, &Placement { rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], offset: [0.0; 3], size: 1.0 });
        for p in [middle, add(middle, [0.5, 0.1, -0.2]), add(middle, [-1.0, 0.4, 0.8])] {
            let at = mat_vec(&camera.r, p);
            let (x, y, z) = (at[0] + camera.t[0], at[1] + camera.t[1], at[2] + camera.t[2]);
            let (u, v) = (camera.fx * x / z + camera.cx, camera.fy * y / z + camera.cy);
            let d = mat_vec(&direct.r, placement.apply(p));
            let (dx, dy, dz) = (d[0] + direct.t[0], d[1] + direct.t[1], d[2] + direct.t[2]);
            assert!(dz > 0.0, "in front");
            assert!((u - (direct.fx * dx / dz + direct.cx)).abs() < 0.05 && (v - (direct.fy * dy / dz + direct.cy)).abs() < 0.05, "{u} {v}");
        }
    }

    fn small_for_hands() -> Small {
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        Small::new(&view, &head_at(0.0, 1.7, 0.0, 0.0), 6.0)
    }

    /// Enzo: a touchpad moves the middle of what shows, along the ground; only the hand holding it, and not past the capture.
    #[test]
    fn the_touchpad_of_the_hand_holding_it_moves_its_middle() {
        let mut small = small_for_hands();
        let eye = [0.0, 1.6, 0.0];
        let start = small.placement(eye);
        let hand = head_at(small.anchor[0] + 0.05, small.anchor[1], small.anchor[2], 0.0);
        let hands = [None, Some(hand)];
        let pads = [[0.0, 0.0], [0.0, 1.0]];
        small.move_middle(hands, pads, 0.5, 100.0);
        assert_eq!(small.moved_by(), 0.0, "nothing held, nothing moves");
        small.step(hands, [false, true], [0.0; 2], 0.016, None);
        assert!(small.held());
        small.move_middle(hands, [[0.0, 1.0], [0.0, 0.0]], 0.5, 100.0);
        assert_eq!(small.moved_by(), 0.0, "the other hand's touchpad does nothing");
        small.move_middle(hands, [[0.0; 2], [0.02, 0.02]], 0.5, 100.0);
        assert_eq!(small.moved_by(), 0.0, "a thumb resting");
        small.move_middle(hands, pads, 0.5, 100.0);
        assert!(small.moved_by() > 0.1);
        let middle = small.middle(eye);
        assert_eq!(middle[1], eye[1], "no up and down");
        // What it shows is still at its anchor, whatever moved under it: the middle moved is what stands there now.
        let placed = small.placement(eye).apply(middle);
        assert!(length(sub(placed, small.anchor)) < 1e-3, "{placed:?} {:?}", small.anchor);
        assert_ne!(small.placement(eye).offset, start.offset);
        small.move_middle(hands, pads, 100.0, 1.5);
        assert!((small.moved_by() - 1.5).abs() < 1e-3, "stops where the capture ends");
    }

    /// K-small-stick: lying in its place, pointed at, the pointing hand's stick moves its middle (no grip, no other hand): pushed up, what was ahead comes nearer, as
    /// walking does; pushed right, sideways. Not while held (the held routes then), and never by a resting thumb.
    #[test]
    fn the_stick_of_the_hand_pointing_at_it_moves_its_middle_where_it_lies() {
        let mut small = small_for_hands();
        let eye = [0.0, 1.6, 0.0];
        // A hand before it, pointing at it (its -Z the way it points, as SteamVR's tip).
        let pointing = head_at(small.anchor[0], small.anchor[1] + 0.1, small.anchor[2] + 0.4, 0.0);
        small.move_pointed(&pointing, [0.0, 0.1], 0.5, 100.0);
        assert_eq!(small.moved_by(), 0.0, "a thumb resting");
        let before = small.placement(eye);
        small.move_pointed(&pointing, [0.0, 1.0], 0.5, 100.0);
        assert!(small.moved_by() > 0.5, "{}", small.moved_by());
        let middle = small.middle(eye);
        // What now stands at its middle stood ahead of it, away from who points: pushed up goes on into it, as walking does.
        let was = before.apply(middle);
        assert!(was[2] < small.anchor[2] - 0.05 && (was[0] - small.anchor[0]).abs() < 0.02, "pushed up: on into it ({was:?} from {:?})", small.anchor);
        assert_eq!(middle[1], eye[1], "no up and down");
        let forward = small.moved_by();
        let before = small.placement(eye);
        small.move_pointed(&pointing, [1.0, 0.0], 0.5, 100.0);
        let was = before.apply(small.middle(eye));
        assert!(was[0] > small.anchor[0] + 0.05, "pushed right: to the right ({was:?} from {:?})", small.anchor);
        assert!(small.moved_by() > forward);
        // Held: the pointing route does nothing, the held ones do.
        let hand = head_at(small.anchor[0] + 0.05, small.anchor[1], small.anchor[2], 0.0);
        small.step([None, Some(hand)], [false, true], [0.0; 2], 0.016, None);
        let held_at = small.moved_by();
        small.move_pointed(&pointing, [0.0, 1.0], 0.5, 100.0);
        assert_eq!(small.moved_by(), held_at);
    }

    /// Enzo: "the vrchat rotation joystick command to be reproduced". Pushed right, one turns right: what was ahead goes left, and where one stands stays.
    #[test]
    fn the_turning_stick_turns_the_world_about_the_head() {
        let placement = Placement { rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], offset: [0.0, 0.0, 0.0], size: 1.0 };
        let head = [0.0, 1.6, 0.0];
        let turned = turned_about(&placement, head, 0.5);
        assert!(length(sub(turned.apply(head), head)) < 1e-5, "where one stands stays");
        let ahead = turned.apply([0.0, 1.6, -1.0]);
        assert!(ahead[0] < -0.4, "what was ahead goes left: {ahead:?}");
        assert!((ahead[1] - 1.6).abs() < 1e-5, "no higher or lower");
        let mut stick = TurnStick::default();
        assert_eq!(stick.step(0.1, 0.1, false), 0.0, "a thumb at rest");
        assert!(stick.step(1.0, 0.1, false) > 0.0 && stick.step(-1.0, 0.1, false) < 0.0);
        // In steps: one for each push.
        assert_eq!(stick.step(0.9, 0.1, true), TURN_STEP);
        assert_eq!(stick.step(0.9, 0.1, true), 0.0, "held, no more");
        assert_eq!(stick.step(0.0, 0.1, true), 0.0);
        assert_eq!(stick.step(-0.9, 0.1, true), -TURN_STEP);
    }

    /// Enzo: "it's better that the 3D captures never go in front of the menus".
    #[test]
    fn a_hand_near_it_takes_it_and_its_stick_zooms_it_and_only_that_hand() {
        let mut small = small_for_hands();
        let start = small;
        let near = head_at(small.anchor[0] + 0.2, small.anchor[1], small.anchor[2], 0.0);
        let away = head_at(small.anchor[0] + 2.0, small.anchor[1], small.anchor[2], 0.0);
        // Too far: a grip does nothing, and a grip already closed when it comes near does not take it either.
        small.step([Some(away), None], [true, false], [1.0, 0.0], 0.1, None);
        assert!(!small.held() && !small.near);
        small.step([Some(near), None], [true, false], [1.0, 0.0], 0.1, None);
        assert!(!small.held() && small.near);
        assert_eq!(small.reach(), start.reach(), "nothing held, the stick zooms nothing");
        small.step([Some(near), None], [false, false], [0.0, 0.0], 0.1, None);
        small.step([Some(near), None], [true, false], [0.0, 0.0], 0.1, None);
        assert!(small.held() && small.taken && !small.near);
        assert!(!small.zoomed);
        // It stays where it was as it is taken, and follows the hand, turning with it.
        assert!(close(small.anchor, start.anchor, 1e-5));
        let moved = head_at(near.m[0][3] + 0.1, near.m[1][3] + 0.3, near.m[2][3], 1.0);
        small.step([Some(moved), None], [true, false], [0.0, 0.0], 0.1, None);
        assert!((length(sub(small.anchor, position(&moved))) - 0.2).abs() < 1e-4, "at the same distance from the hand");
        assert!(small.rotation != start.rotation);
        // The other hand's stick does nothing, a resting thumb neither, its own does.
        let reach = small.reach();
        small.step([Some(moved), None], [true, false], [0.1, 1.0], 0.5, None);
        assert_eq!(small.reach(), reach);
        assert!(!small.zoomed);
        small.step([Some(moved), None], [true, false], [1.0, 0.0], 0.5, None);
        assert!(small.reach() < reach, "up: closer, it shows less");
        assert!(small.zoomed);
        small.step([Some(moved), None], [true, false], [-1.0, 0.0], 0.5, None);
        assert!((small.reach() - reach).abs() < reach * 0.01);
        // Zooming, it stays as large in the room.
        assert!((small.radius() - SMALL_RADIUS_M).abs() < 1e-3);
        // Let go, it stays, and no stick moves it.
        let anchor = small.anchor;
        small.step([Some(near), None], [false, false], [1.0, 0.0], 0.5, None);
        assert!(!small.held() && small.taken);
        assert_eq!(small.anchor, anchor);
    }

    /// Enzo: "it follows the menu until we grab it".
    #[test]
    fn in_a_panel_it_follows_its_place_until_taken_and_goes_back_when_let_go_there() {
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        let sheet = head_at(0.3, 1.3, -0.4, 0.2);
        let dock = SmallDock::behind(&view, &sheet, 0.08, 3.0);
        assert!(dot(sub(dock.anchor, position(&sheet)), axis(&sheet, 2)) < 0.0, "behind the panel, as seen by who faces it");
        // Its start view looks into the panel.
        let d = mat_vec(&dock.rotation, sub([view.target[0], view.target[1], -view.target[2]], view.eye()));
        let length_d = length(d);
        assert!(dot(d, axis(&sheet, 2)) / length_d < -0.8, "{d:?}");
        let mut small = small_for_hands().at_dock(&dock);
        assert!(small.docked);
        assert_eq!(small.radius(), 0.08);
        // The panel moves: so does it, at once.
        let elsewhere = head_at(0.6, 1.5, -0.3, -0.4);
        let dock2 = SmallDock::behind(&view, &elsewhere, 0.08, 3.0);
        small.step([None, None], [false; 2], [0.0; 2], 1.0 / 90.0, Some(dock2));
        assert_eq!(small.anchor, dock2.anchor);
        assert_eq!(small.rotation, dock2.rotation);
        // A hand takes it: out of its place, growing; the panel no longer moves it.
        let hand = head_at(dock2.anchor[0], dock2.anchor[1] + 0.1, dock2.anchor[2], 0.0);
        small.step([Some(hand), None], [true, false], [0.0; 2], 1.0 / 90.0, Some(dock2));
        assert!(small.taken && !small.docked);
        for _ in 0..90 {
            small.step([Some(hand), None], [true, false], [0.0; 2], 1.0 / 90.0, Some(dock));
        }
        assert!((small.radius() - SMALL_RADIUS_M).abs() < 1e-2, "{}", small.radius());
        assert!(close(small.anchor, dock2.anchor, 1e-3), "it went with the hand, which stayed");
        // Let go far from its place: it stays there, wherever the panel goes.
        let far = head_at(hand.m[0][3] + 0.8, hand.m[1][3], hand.m[2][3], 0.0);
        small.step([Some(far), None], [true, false], [0.0; 2], 1.0 / 90.0, Some(dock));
        small.step([Some(far), None], [false, false], [0.0; 2], 1.0 / 90.0, Some(dock));
        assert!(small.taken && !small.docked);
        let there = small.anchor;
        small.step([None, None], [false; 2], [0.0; 2], 0.1, Some(dock2));
        assert_eq!(small.anchor, there);
        // Asked back, it settles in its place and shrinks to it, and then follows it again.
        small.bring_back();
        for _ in 0..200 {
            small.step([None, None], [false; 2], [0.0; 2], 1.0 / 90.0, Some(dock));
        }
        assert!(small.docked && !small.taken);
        assert_eq!(small.anchor, dock.anchor);
        assert_eq!(small.radius(), 0.08);
        small.step([None, None], [false; 2], [0.0; 2], 1.0 / 90.0, Some(dock2));
        assert_eq!(small.anchor, dock2.anchor);
        // Let go near its place, it goes back by itself.
        let at = head_at(dock2.anchor[0], dock2.anchor[1], dock2.anchor[2], 0.0);
        small.step([Some(at), None], [true, false], [0.0; 2], 1.0 / 90.0, Some(dock2));
        assert!(small.taken);
        small.step([Some(at), None], [false, false], [0.0; 2], 1.0 / 90.0, Some(dock2));
        assert!(!small.taken && small.docked);
        // With no place to lie, it is simply free, at its full size.
        let mut free = small_for_hands().at_dock(&dock);
        for _ in 0..90 {
            free.step([None, None], [false; 2], [0.0; 2], 1.0 / 90.0, None);
        }
        assert!(!free.docked && (free.radius() - SMALL_RADIUS_M).abs() < 1e-2);
    }

    /// A panel held as a hand holds one: at `x, y, z`, turned `yaw` and tipped back `pitch` radians.
    fn sheet_at(x: f32, y: f32, z: f32, yaw: f32, pitch: f32) -> Matrix34 {
        let (ps, pc) = pitch.sin_cos();
        let tipped = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, pc, -ps, 0.0], [0.0, ps, pc, 0.0]] };
        compose(&head_at(x, y, z, yaw), &tipped)
    }

    /// I-view-forward (Enzo, 2026-11-13, his mockup): in a panel's hole a capture is cut on its near side only. Whatever the panel's pose, the
    /// start view's way, the zoom or where its middle was moved: nothing that lies in front of the panel's plane (or on it) is drawn, what lies
    /// ahead is drawn whole to the capture's far end, and the cut is the same for every pose of the panel (never per head or per eye: both eyes see
    /// one edge, and nothing pops as the head moves).
    #[test]
    fn in_a_hole_only_the_near_side_is_cut_and_never_in_front_of_the_panel() {
        let views = [
            StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None },
            StartView { position: [3.0, 2.1, -4.0], target: [7.0, 1.0, 2.0], floor: Some(0.4), eye_height: None },
        ];
        let sheets = [sheet_at(0.3, 1.3, -0.4, 0.2, 0.0), sheet_at(-0.2, 1.1, -0.35, -0.7, 0.5), sheet_at(0.6, 1.5, -0.3, 2.5, -0.3)];
        for view in &views {
            for &(moved, reach) in &[([0.0, 0.0], 3.0), ([1.5, -2.0], 1.2), ([0.0, 0.0], 0.5), ([-4.0, 1.0], 12.0)] {
                let mut cuts = Vec::new();
                for sheet in &sheets {
                    let dock = SmallDock::behind(view, sheet, 0.08, reach);
                    let small = Small::new(view, &head_at(0.0, 1.7, 0.0, 0.0), 6.0).at_dock(&dock).framed((moved, reach));
                    let cut = Cut::of(&small, view);
                    assert!(cut.reaches_the_far_end());
                    let placement = small.placement(view.eye());
                    let middle = small.middle(view.eye());
                    let (plane, normal) = (position(sheet), axis(sheet, 2));
                    let mut ahead_seen = 0;
                    for i in -24..=24 {
                        for j in -24..=24 {
                            for k in -4..=12 {
                                let p = [middle[0] + i as f32 * reach * 0.4, view.ground() + k as f32 * reach * 0.25, middle[2] + j as f32 * reach * 0.4];
                                let depth = dot(sub(placement.apply(p), plane), normal);
                                let gone = cut.gone(p);
                                assert!((0.0..=1.0).contains(&gone));
                                if depth > -1e-5 {
                                    assert_eq!(gone, 1.0, "in front of the panel by {depth} m, drawn: {p:?} reach {reach}");
                                }
                                // Far behind the panel (in the room, its radius' worth and more): drawn whole, however far.
                                if depth < -0.08 * 1.4 {
                                    assert_eq!(gone, 0.0, "{} m behind the panel and cut: {p:?} reach {reach}", -depth);
                                    ahead_seen += 1;
                                }
                            }
                        }
                    }
                    assert!(ahead_seen > 5000, "{ahead_seen}");
                    // Far beyond its old round, along the way it looks into the hole: whole.
                    let forward = sub([view.target[0], view.target[1], -view.target[2]], view.eye());
                    let level = [forward[0], 0.0, forward[2]];
                    let level = scale(level, 1.0 / length(level));
                    for far in [1.5, 5.0, 40.0, 400.0] {
                        assert_eq!(cut.gone(add([middle[0], view.ground(), middle[2]], scale(level, reach * far))), 0.0, "{far} reaches ahead");
                    }
                    cuts.push(cut);
                }
                assert!(cuts.windows(2).all(|c| c[0] == c[1]), "the panel's pose changes nothing of the cut: {cuts:?}");
            }
        }
    }

    /// The near edge, seen from above as in Enzo's mockup: nearest at the middle, where the round's near edge was on the ground; smoothly further
    /// back aside, the same on either side; level about the middle's depth far aside.
    #[test]
    fn the_near_edge_bulges_at_the_middle_and_recedes_smoothly_to_the_sides() {
        let reach = 3.0;
        let at = |u: f32| Cut::near_edge(reach, u);
        // The round's near edge on the ground in a hole: `reach` nearer than the ground under the middle, tipped by the hole's tilt.
        assert!((at(0.0) - reach * (1.0 - SMALL_TILT_IN_A_HOLE.to_radians().cos())).abs() < 1e-5);
        assert!(at(0.0) > 0.0, "behind the panel even where nearest");
        let mut last = at(0.0);
        for i in 1..=200 {
            let u = i as f32 * reach * 0.03;
            assert!((at(u) - at(-u)).abs() < 1e-6, "the same on both sides");
            // Further back while it curves (to three reaches aside), never nearer once level.
            assert!(if u <= reach * 3.0 { at(u) > last } else { at(u) >= last }, "receding aside at {u}");
            // Smooth: no step between neighbours.
            assert!(at(u) - last < reach * 0.03, "no step at {u}");
            last = at(u);
        }
        assert!(at(reach * 6.0) < at(0.0) + reach * 1.0001 && at(reach * 6.0) > at(0.0) + reach * 0.99, "level far aside, about the middle's depth");
        // The middle's ground is drawn whole.
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        let dock = SmallDock::behind(&view, &sheet_at(0.0, 1.2, -0.4, 0.0, 0.0), 0.08, reach);
        let small = Small::new(&view, &head_at(0.0, 1.7, 0.0, 0.0), reach).at_dock(&dock).framed(([0.0; 2], reach));
        let cut = Cut::of(&small, &view);
        for y in [0.0, 0.5, 1.6, 2.5] {
            assert_eq!(cut.gone([0.0, view.ground() + y, 0.0]), 0.0, "{y} m over the middle's ground");
        }
    }

    /// Out of its hole (in a hand, or let go in the room) it is the round it always was: whole to 0.8 of its reach, nothing past it, nothing cut by height.
    #[test]
    fn in_the_room_it_is_still_its_round() {
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        let small = Small::new(&view, &head_at(0.0, 1.7, 0.0, 0.0), 4.0);
        let cut = Cut::of(&small, &view);
        assert!(!cut.reaches_the_far_end());
        let middle = small.middle(view.eye());
        for (far, height, gone) in [(0.0, 0.0, 0.0), (3.1, 30.0, 0.0), (3.6, -1.0, 0.5), (4.01, 0.0, 1.0), (40.0, 0.0, 1.0)] {
            assert!((cut.gone([middle[0] + far, height, middle[2]]) - gone).abs() < 1e-4, "{far} m out, {height} m high");
        }
        // Worked out again only for a change one can see.
        let moved = Cut::Round { middle: add(middle, [0.01, 0.0, 0.0]), reach: 4.0 };
        assert!(cut.same_as(&moved));
        assert!(!cut.same_as(&Cut::Round { middle: add(middle, [0.5, 0.0, 0.0]), reach: 4.0 }));
    }

    fn head_at(x: f32, y: f32, z: f32, yaw: f32) -> Matrix34 {
        // A head turned `yaw` radians left (about +Y) from looking down −Z.
        let (s, c) = yaw.sin_cos();
        Matrix34 { m: [[c, 0.0, s, x], [0.0, 1.0, 0.0, y], [-s, 0.0, c, z]] }
    }

    fn project(cam: &Camera, p: V3) -> Option<[f32; 2]> {
        let c = add(mat_vec(&cam.r, p), cam.t);
        (c[2] > 1e-6).then(|| [cam.fx * c[0] / c[2] + cam.cx, cam.fy * c[1] / c[2] + cam.cy])
    }

    #[test]
    fn the_start_view_lands_on_the_floor_under_the_head_facing_its_way() {
        // VRChat Home's start view, as listed (no floor: the eye 1.6 m over its ground).
        let view = StartView { position: [-0.148, 1.469, -0.350], target: [2.627, 1.046, 2.227], floor: None, eye_height: None };
        for yaw in [0.0f32, 0.7, -2.5] {
            let head = head_at(0.4, 1.72, -0.9, yaw);
            let p = Placement::at(&view, &head);
            // The start eye stands over the head, its eye height above the floor.
            assert!(close(p.apply(view.eye()), [0.4, 1.6, -0.9], 1e-4), "{:?}", p.apply(view.eye()));
            // Its target lies the way the head looks (level).
            let target = [view.target[0], view.target[1], -view.target[2]];
            let d = sub(p.apply(target), p.apply(view.eye()));
            let forward = scale(axis(&head, 2), -1.0);
            let level = [d[0], 0.0, d[2]];
            assert!((dot(level, forward) / length(level) - 1.0).abs() < 1e-4, "yaw {yaw}");
            // A reflection (Unity to SteamVR), undone by its inverse.
            let q = [3.0, -1.0, 7.0];
            assert!(close(p.invert(p.apply(q)), q, 1e-4));
        }
        // With a floor, that ground meets the play space's floor whatever the eye height.
        let floored = StartView { floor: Some(-4.33), eye_height: Some(1.66), ..view };
        let p = Placement::at(&floored, &head_at(0.0, 1.5, 0.0, 0.0));
        assert!((p.apply([1.0, -4.33, 2.0])[1]).abs() < 1e-4);
    }

    #[test]
    fn each_face_quad_faces_the_inside_and_is_right_handed() {
        let cube = Cube { center: [0.1, 1.6, -0.2], half: 2.0, pixels: 1024, margin: 6 };
        for f in 0..6 {
            let t = cube.transform(f);
            let (x, y, z) = (axis(&t, 0), axis(&t, 1), axis(&t, 2));
            assert!(close(cross(x, y), z, 1e-6), "face {f}");
            // Its front looks at the centre.
            assert!(dot(z, sub(cube.center, position(&t))) > 0.0, "face {f}");
        }
    }

    #[test]
    fn a_face_camera_draws_each_point_where_its_ray_meets_the_quad() {
        // Any point the eye sees through a face is drawn where the eye's ray to it crosses that face: the picture is
        // the eye's view through the quad, wherever the eye is inside the cube.
        let cube = Cube { center: [0.0, 1.5, 0.0], half: 2.0, pixels: 1000, margin: 5 };
        let placement = Placement::at(
            &StartView { position: [3.0, 2.0, -1.0], target: [4.0, 2.0, -3.0], floor: Some(0.4), eye_height: None },
            &head_at(0.2, 1.6, 0.1, 0.3),
        );
        let eye = [0.25, 1.62, 0.05];
        let mut checked = 0;
        for (i, p) in [[0.3f32, 1.9, -6.0], [5.0, 0.2, 1.0], [-0.4, 9.0, 0.7], [0.1, -3.0, 0.2], [-8.0, 2.0, 8.0], [0.5, 1.6, 4.0]].into_iter().enumerate() {
            let dir = sub(p, eye);
            let f = cube.face_hit(eye, dir);
            let expected = cube.pixel_hit(f, eye, dir).expect("inside the quad");
            let cam = cube.camera(f, eye, &placement);
            let got = project(&cam, placement.invert(p)).expect("in front");
            assert!((got[0] - expected[0]).abs() < 0.05 && (got[1] - expected[1]).abs() < 0.05, "point {i} face {f}: {got:?} vs {expected:?}");
            checked += 1;
        }
        assert_eq!(checked, 6);
    }

    #[test]
    fn neighbouring_faces_draw_the_same_place_in_their_overlap() {
        let cube = Cube { center: [0.0, 0.0, 0.0], half: 1.0, pixels: 512, margin: 8 };
        let eye = [0.3, -0.2, 0.1];
        // A direction just past the ahead face's right edge, inside its margin: both the ahead and the right face hold it.
        let dir = sub([1.0 + 0.004, 0.3, -1.0], eye);
        let ahead = cube.pixel_hit(0, eye, sub([0.996, 0.3, -1.0], eye));
        assert!(ahead.is_some());
        assert!(cube.pixel_hit(0, eye, dir).is_some(), "the ahead face's margin");
        assert!(cube.pixel_hit(1, eye, dir).is_some(), "the right face");
    }

    #[test]
    fn the_faces_seen_follow_where_the_eye_looks() {
        let cube = Cube { center: [0.0, 1.6, 0.0], half: 2.0, pixels: 1024, margin: 6 };
        let tangents = [-1.2, 1.1, -1.25, 1.25];
        let ahead = head_at(0.0, 1.6, 0.0, 0.0);
        let seen = cube.faces_seen(&ahead, tangents, 0.0);
        assert_eq!(seen, [true, true, false, true, true, true], "ahead with ~100° covers the side, up and down faces' edges");
        let narrow = cube.faces_seen(&ahead, [-0.5, 0.5, -0.5, 0.5], 0.0);
        assert_eq!(narrow, [true, false, false, false, false, false]);
        // Turned around: behind.
        let behind = cube.faces_seen(&head_at(0.0, 1.6, 0.0, std::f32::consts::PI), [-0.5, 0.5, -0.5, 0.5], 0.0);
        assert_eq!(behind, [false, false, true, false, false, false]);
        // A margin reaches the next face before the head turns to it.
        let widened = cube.faces_seen(&ahead, [-0.5, 0.5, -0.5, 0.5], 0.4);
        assert!(widened[1] && widened[3]);
    }

    #[test]
    fn the_eye_camera_and_its_rays_agree() {
        let placement = Placement::at(&StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None }, &Matrix34::IDENTITY);
        let view = head_at(0.1, 1.7, 0.2, 0.4);
        let tangents = [-1.3, 1.0, -1.2, 1.1];
        let cam = eye_camera(&view, tangents, 300, 320, &placement);
        for (x, y) in [(0.0f32, 0.0f32), (150.0, 40.0), (299.0, 319.0)] {
            let p = add(position(&view), scale(eye_ray(&view, tangents, 300, 320, x, y), 3.0));
            let got = project(&cam, placement.invert(p)).unwrap();
            assert!((got[0] - x).abs() < 0.02 && (got[1] - y).abs() < 0.02, "{got:?} vs {x},{y}");
        }
    }

    #[test]
    fn a_face_texel_maps_through_the_homography_to_where_the_wide_view_sees_it() {
        let cube = Cube { center: [0.1, 1.5, -0.2], half: 1.5, pixels: 512, margin: 4 };
        let view = head_at(0.15, 1.62, -0.1, 0.5);
        let wide = PlayCamera::wide(&view, [-1.39, 1.24, -1.47, 1.45], 0.15, 256.0, 4096);
        let w = cube.width() as f32;
        for f in 0..6 {
            let h = cube.homography(f, &wide);
            for (x, y) in [(0.0f32, 0.0f32), (100.0, 400.0), (w - 1.0, 17.0), (260.0, 260.0)] {
                // The texel's point on the quad.
                let face = FACES[f];
                let (a, b) = (((x + 0.5) / w * 2.0 - 1.0) * cube.extent(), ((y + 0.5) / w * 2.0 - 1.0) * cube.extent());
                let p = add(add(add(cube.center, scale(face.normal, cube.half)), scale(face.right(), a)), scale(face.up, -b));
                let hp = [0, 1, 2].map(|i| h[i][0] * x + h[i][1] * y + h[i][2]);
                match wide.project(p) {
                    Some([px, py]) => {
                        assert!(hp[2] > 0.0);
                        assert!((hp[0] / hp[2] - px).abs() < 0.05 && (hp[1] / hp[2] - py).abs() < 0.05, "face {f} texel {x},{y}");
                    }
                    None => assert!(hp[2] <= 1e-6, "behind the eye: face {f} texel {x},{y}"),
                }
            }
        }
        // Looking ahead, the wide view takes in the ahead face, not the one behind.
        assert!(cube.face_in(0, &wide) || cube.face_in(3, &wide));
        assert!(!cube.face_in(2, &PlayCamera::wide(&head_at(0.1, 1.5, -0.2, 0.0), [-0.5, 0.5, -0.5, 0.5], 0.0, 256.0, 4096)));
    }

    #[test]
    fn the_reveal_grows_from_the_start_view_past_the_farthest_splat() {
        assert!(reveal_radius(0.0, 0.8, 120.0) < 1e-6);
        let mid = reveal_radius(0.2, 0.8, 120.0);
        assert!(mid > 1.0 && mid < 120.0, "{mid}");
        assert!(reveal_radius(0.8, 0.8, 120.0) > 120.0);
        assert!(reveal_radius(0.5, 0.8, 120.0) > reveal_radius(0.4, 0.8, 120.0));
    }
}
