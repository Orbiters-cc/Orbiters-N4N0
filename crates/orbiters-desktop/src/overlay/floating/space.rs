//! The floating panel's geometry, in SteamVR's standing play space (right-handed, +Y up, metres): an overlay's +Z faces
//! whoever looks at it and its +Y is its picture's top. Where a laser meets a panel, the frame a held panel rides (as
//! N4N0's screen rides a palm), where the watch sits on a wrist and how much it shows, a laser's beam about its axis.

use orbiters_openvr::Matrix34;
use orbiters_ui::anim::Spring;

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

/// `a` made one long, `None` when it has no direction.
pub fn unit(a: V3) -> Option<V3> {
    let l = length(a);
    (l > 1e-6 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

/// A pose's position.
pub fn position(m: &Matrix34) -> V3 {
    [m.m[0][3], m.m[1][3], m.m[2][3]]
}

/// A pose's axis `i` (0: right, 1: up, 2: back, towards whoever an overlay faces).
pub fn axis(m: &Matrix34, i: usize) -> V3 {
    [m.m[0][i], m.m[1][i], m.m[2][i]]
}

/// The pose with axes `x`, `y`, `z` (columns) at `p`.
pub fn from_axes(x: V3, y: V3, z: V3, p: V3) -> Matrix34 {
    Matrix34 { m: [[x[0], y[0], z[0], p[0]], [x[1], y[1], z[1], p[1]], [x[2], y[2], z[2], p[2]]] }
}

/// `a` then `b` (`a · b`: `b` expressed in `a`'s space).
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

/// The inverse of a pose (a rotation and a position).
pub fn inverse(m: &Matrix34) -> Matrix34 {
    let r = |i: usize, j: usize| m.m[j][i];
    let p = position(m);
    let mut out = Matrix34::IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            out.m[i][j] = r(i, j);
        }
        out.m[i][3] = -(r(i, 0) * p[0] + r(i, 1) * p[1] + r(i, 2) * p[2]);
    }
    out
}

/// A point in `m`'s space, in the space `m` is in.
pub fn apply(m: &Matrix34, p: V3) -> V3 {
    add(rotate(m, p), position(m))
}

/// A direction in `m`'s space, in the space `m` is in.
pub fn rotate(m: &Matrix34, v: V3) -> V3 {
    [dot([m.m[0][0], m.m[0][1], m.m[0][2]], v), dot([m.m[1][0], m.m[1][1], m.m[1][2]], v), dot([m.m[2][0], m.m[2][1], m.m[2][2]], v)]
}

/// `m` moved by `offset` along its own axes.
pub fn offset(m: &Matrix34, offset: V3) -> Matrix34 {
    let mut out = *m;
    let p = apply(m, offset);
    (0..3).for_each(|i| out.m[i][3] = p[i]);
    out
}

/// Whether two poses are within `metres` of each other and turned alike, each axis within `turn` (1 − its cosine).
pub fn near(a: &Matrix34, b: &Matrix34, metres: f32, turn: f32) -> bool {
    length(sub(position(a), position(b))) <= metres && (0..3).all(|i| 1.0 - dot(axis(a, i), axis(b, i)) <= turn)
}

/// A pose read back (from the settings): finite, its rotation made a rotation again (a matrix many compositions or a
/// hand edit left a little off). `None` when it is not one.
pub fn orthonormal(m: &Matrix34) -> Option<Matrix34> {
    if m.m.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let z = unit(axis(m, 2))?;
    let x = unit(sub(axis(m, 0), scale(z, dot(axis(m, 0), z))))?;
    Some(from_axes(x, cross(z, x), z, position(m)))
}

/// A laser: where it starts (its controller's tip) and the way it points (the tip's −Z).
pub fn ray(tip: &Matrix34) -> (V3, V3) {
    (position(tip), unit(scale(axis(tip, 2), -1.0)).unwrap_or([0.0, 0.0, -1.0]))
}

/// A flat overlay in the play space: where it is and how large.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub transform: Matrix34,
    pub width: f32,
    pub height: f32,
}

/// Where a laser meets a quad: how far along the laser, the point, and the place on the quad (0 to 1 from its top
/// left, as its picture's pixels go).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub distance: f32,
    pub point: V3,
    pub uv: [f32; 2],
}

impl Quad {
    /// Where the laser from `origin` along `direction` (one long) meets the quad's plane, in front of it, wherever on
    /// the plane (a press held past the panel's edge): `uv` may then be past 0 to 1.
    pub fn plane_hit(&self, origin: V3, direction: V3) -> Option<Hit> {
        let local = inverse(&self.transform);
        let (o, d) = (apply(&local, origin), rotate(&local, direction));
        if d[2].abs() < 1e-6 {
            return None;
        }
        let distance = -o[2] / d[2];
        if !(distance > 0.0) {
            return None;
        }
        let at = add(o, scale(d, distance));
        let uv = [at[0] / self.width + 0.5, 0.5 - at[1] / self.height];
        Some(Hit { distance, point: add(origin, scale(direction, distance)), uv })
    }

    /// Where the laser meets the quad itself, or within `margin` metres past its edges.
    pub fn hit(&self, origin: V3, direction: V3, margin: f32) -> Option<Hit> {
        let hit = self.plane_hit(origin, direction)?;
        let (mu, mv) = (margin / self.width, margin / self.height);
        let inside = (-mu..=1.0 + mu).contains(&hit.uv[0]) && (-mv..=1.0 + mv).contains(&hit.uv[1]);
        inside.then_some(hit)
    }

    /// The point on the quad nearest `p` and how far `p` is from it (a fingertip at the watch).
    pub fn nearest(&self, p: V3) -> (Hit, f32) {
        let local = apply(&inverse(&self.transform), p);
        let (x, y) = (local[0].clamp(-self.width / 2.0, self.width / 2.0), local[1].clamp(-self.height / 2.0, self.height / 2.0));
        let point = apply(&self.transform, [x, y, 0.0]);
        let gap = length(sub(p, point));
        (Hit { distance: 0.0, point, uv: [x / self.width + 0.5, 0.5 - y / self.height] }, gap)
    }
}

/// Upright before the eyes of `head`: `distance` ahead of them along where they look (level), `below` lower, facing
/// them, upright as the play space is (N4N0's screen's home, its aim at the eyes with the scene's up).
pub fn before_eyes(head: &Matrix34, distance: f32, below: f32) -> Matrix34 {
    let back = axis(head, 2);
    let ahead = unit([-back[0], 0.0, -back[2]]).unwrap_or([0.0, 0.0, -1.0]);
    let eyes = position(head);
    let at = [eyes[0] + ahead[0] * distance, eyes[1] - below, eyes[2] + ahead[2] * distance];
    facing(at, eyes, [0.0, 1.0, 0.0]).unwrap_or_else(|| from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], at))
}

/// A pose at `at` whose +Z looks at `eyes`, its +Y as near `up` as it can be.
pub fn facing(at: V3, eyes: V3, up: V3) -> Option<Matrix34> {
    let z = unit(sub(eyes, at))?;
    let x = unit(cross(up, z))?;
    Some(from_axes(x, cross(z, x), z, at))
}

/// The frame a held panel rides, as N4N0's screen rides a palm's carry frame: at the hand, its +Z towards the eyes,
/// upright as the head is (the head's up, so a tilted head tilts it as it tilts the screen).
pub fn carry(hand: V3, head: &Matrix34) -> Matrix34 {
    let eyes = position(head);
    let z = unit(sub(eyes, hand)).unwrap_or_else(|| axis(head, 2));
    let x = unit(cross(axis(head, 1), z)).or_else(|| unit(cross(scale(axis(head, 2), -1.0), z))).unwrap_or([1.0, 0.0, 0.0]);
    from_axes(x, cross(z, x), z, hand)
}

/// Where a held panel is now: as it was when taken (`taken`, the carry frame then `carry_then`), moved and turned as
/// the carry frame moved and turned since.
pub fn carried(carry_now: &Matrix34, carry_then: &Matrix34, taken: &Matrix34) -> Matrix34 {
    compose(carry_now, &compose(&inverse(carry_then), taken))
}

/// How near and how far a panel may be left and still open where it was, and how far off where the eyes look.
pub const IN_VIEW_NEAR_M: f32 = 0.25;
pub const IN_VIEW_FAR_M: f32 = 2.5;
const IN_VIEW_COS: f32 = 0.77; // 40°

/// Whether a panel left at `placed` is before the eyes of `head`: in reach of the lasers, near where they look (left
/// and right, whatever the head's pitch), its face towards them. Otherwise it opens before them again.
pub fn in_view(placed: &Matrix34, head: &Matrix34) -> bool {
    let to = sub(position(placed), position(head));
    let distance = length(to);
    if !(IN_VIEW_NEAR_M..=IN_VIEW_FAR_M).contains(&distance) {
        return false;
    }
    let back = axis(head, 2);
    let (Some(ahead), Some(level)) = (unit([-back[0], 0.0, -back[2]]), unit([to[0], 0.0, to[2]])) else { return false };
    dot(ahead, level) >= IN_VIEW_COS && dot(axis(placed, 2), scale(to, -1.0 / distance)) > 0.3
}

/// The watch's place on a controller (SteamVR's controller space: X right, Y up, Z back towards the wrist): on the back
/// of the wrist (out of the back of the hand: −X on the left controller, +X on the right), 18 cm towards the elbow and
/// 3.5 cm down, as a watch is worn; its face out of the wrist, turned a little up and towards the elbow, so that it faces
/// the eyes as the wrist turns to look at it. Its picture's top is towards the thumb.
pub fn on_wrist(right: bool) -> Matrix34 {
    let side = if right { 1.0 } else { -1.0 };
    let z = unit([side, 0.3, 0.45]).expect("a direction");
    let x = unit(cross([0.0, 1.0, 0.0], z)).expect("not vertical");
    from_axes(x, cross(z, x), z, [side * 0.035, -0.035, 0.18])
}

/// How much the watch shows (0 to 1), as XSOverlay's wrist does: as its face turns to the eyes and the eyes look at it.
pub fn watch_shown(watch: &Matrix34, head: &Matrix34) -> f32 {
    let Some(to_eyes) = unit(sub(position(head), position(watch))) else { return 0.0 };
    let facing = dot(axis(watch, 2), to_eyes);
    let looking = dot(axis(head, 2), to_eyes);
    ramp(facing, 0.35, 0.7) * ramp(looking, 0.75, 0.9)
}

/// 0 below `from`, 1 past `to`, eased between.
fn ramp(x: f32, from: f32, to: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A laser's beam `length` long from its tip, as an overlay on its controller: `tip` in the controller's space, `eyes`
/// there too. Its +Y runs along the laser (its picture's top at the far end), its middle halfway, its face turned
/// about the laser towards the eyes, so that it is never seen edge on.
pub fn beam(tip: &Matrix34, length: f32, eyes: V3) -> Matrix34 {
    let (o, d) = ray(tip);
    let middle = add(o, scale(d, length / 2.0));
    let to_eyes = sub(eyes, middle);
    let z = unit(sub(to_eyes, scale(d, dot(to_eyes, d)))).or_else(|| unit(axis(tip, 1))).unwrap_or([0.0, 1.0, 0.0]);
    from_axes(cross(d, z), d, z, middle)
}

/// A value that springs towards its target, out of egui (the overlays' fades and the like), with the interface's
/// spring presets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Follow {
    pub value: f32,
    velocity: f32,
}

impl Follow {
    pub fn at(value: f32) -> Self {
        Self { value, velocity: 0.0 }
    }

    /// `dt` seconds towards `target`; returns whether it still moves.
    pub fn step(&mut self, target: f32, dt: f32, s: Spring) -> bool {
        let mut dt = dt.clamp(0.0, 0.05);
        while dt > 0.0 {
            let h = dt.min(1.0 / 240.0);
            dt -= h;
            let a = (-s.stiffness * (self.value - target) - s.damping * self.velocity) / s.mass;
            self.velocity += a * h;
            self.value += self.velocity * h;
        }
        let still = (self.value - target).abs() < 0.002 && self.velocity.abs() < 0.02;
        if still || !self.value.is_finite() {
            *self = Self::at(target);
        }
        !still
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        length(sub(a, b)) < 1e-4
    }

    fn head_at(p: V3) -> Matrix34 {
        from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], p)
    }

    /// A head at `p` turned `yaw` radians left (looking along −Z when 0).
    fn head_turned(p: V3, yaw: f32) -> Matrix34 {
        let (s, c) = yaw.sin_cos();
        from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], p)
    }

    #[test]
    fn poses_compose_and_invert() {
        let a = offset(&head_turned([0.3, 1.2, -0.4], 0.7), [0.1, 0.0, 0.2]);
        let b = facing([1.0, 1.0, -2.0], [0.0, 1.6, 0.0], [0.0, 1.0, 0.0]).unwrap();
        let ab = compose(&a, &b);
        let back = compose(&inverse(&a), &ab);
        for i in 0..3 {
            for j in 0..4 {
                assert!((back.m[i][j] - b.m[i][j]).abs() < 1e-5);
            }
        }
        assert!(close(apply(&inverse(&b), apply(&b, [0.2, -0.3, 0.5])), [0.2, -0.3, 0.5]));
        // A rotation drifted off is made one again; nonsense is not a pose.
        let mut drifted = b;
        drifted.m[0][0] += 0.01;
        let fixed = orthonormal(&drifted).unwrap();
        assert!((dot(axis(&fixed, 0), axis(&fixed, 2))).abs() < 1e-5 && (length(axis(&fixed, 1)) - 1.0).abs() < 1e-5);
        assert!(orthonormal(&Matrix34 { m: [[f32::NAN; 4]; 3] }).is_none());
        assert!(orthonormal(&Matrix34 { m: [[0.0; 4]; 3] }).is_none());
    }

    #[test]
    fn a_laser_meets_a_panel_where_it_points() {
        // A panel 0.8 × 0.5 m, a metre ahead of the origin, facing back at it.
        let panel = Quad { transform: facing([0.0, 1.5, -1.0], [0.0, 1.5, 0.0], [0.0, 1.0, 0.0]).unwrap(), width: 0.8, height: 0.5 };
        let hit = panel.hit([0.0, 1.5, 0.0], [0.0, 0.0, -1.0], 0.0).unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-5 && (hit.uv[0] - 0.5).abs() < 1e-5 && (hit.uv[1] - 0.5).abs() < 1e-5);
        // Up and to the right: the picture's top right (v from the top).
        let d = unit([0.3, 0.2, -1.0]).unwrap();
        let hit = panel.hit([0.0, 1.5, 0.0], d, 0.0).unwrap();
        assert!((hit.uv[0] - (0.5 + 0.3 / 0.8)).abs() < 1e-4 && (hit.uv[1] - (0.5 - 0.2 / 0.5)).abs() < 1e-4, "{:?}", hit.uv);
        // Past its edge: a miss, but the plane still says where (a press held there).
        let past = unit([0.6, 0.0, -1.0]).unwrap();
        assert!(panel.hit([0.0, 1.5, 0.0], past, 0.0).is_none());
        assert!(panel.plane_hit([0.0, 1.5, 0.0], past).unwrap().uv[0] > 1.0);
        assert!(panel.hit([0.0, 1.5, 0.0], past, 0.25).is_some(), "within the margin");
        // Pointing away, or along the plane: nothing.
        assert!(panel.hit([0.0, 1.5, 0.0], [0.0, 0.0, 1.0], 0.0).is_none());
        assert!(panel.plane_hit([0.0, 1.5, 0.0], [1.0, 0.0, 0.0]).is_none());
        // From behind it works too (the panel shows its back mirrored; the caller flips u).
        let behind = panel.hit([0.1, 1.5, -2.0], [0.0, 0.0, 1.0], 0.0).unwrap();
        assert!((behind.uv[0] - (0.5 + 0.1 / 0.8)).abs() < 1e-4);
        // A fingertip 3 cm off the middle of its face.
        let (near, gap) = panel.nearest([0.0, 1.5, -0.97]);
        assert!((gap - 0.03).abs() < 1e-4 && (near.uv[0] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn it_opens_before_the_eyes_facing_them() {
        let head = head_turned([0.2, 1.7, 0.1], std::f32::consts::FRAC_PI_2);
        let t = before_eyes(&head, 0.7, 0.12);
        // Turned a quarter left the head looks down −X: ahead is −X.
        assert!(close(position(&t), [0.2 - 0.7, 1.58, 0.1]), "{:?}", position(&t));
        let to_eyes = unit(sub(position(&head), position(&t))).unwrap();
        assert!(close(axis(&t, 2), to_eyes), "faces the eyes");
        assert!(axis(&t, 0)[1].abs() < 1e-5, "upright: its right is level");
        assert!(in_view(&t, &head), "what opens before the eyes is in view");
        // A head looking straight down still gets a panel ahead, level.
        let down = from_axes([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [0.0, 1.7, 0.0]);
        assert!(position(&before_eyes(&down, 0.7, 0.12)).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_panel_left_out_of_view_does_not_open_there() {
        let head = head_at([0.0, 1.7, 0.0]);
        let ahead = before_eyes(&head, 0.8, 0.1);
        assert!(in_view(&ahead, &head));
        assert!(!in_view(&ahead, &head_turned([0.0, 1.7, 0.0], std::f32::consts::FRAC_PI_2)), "the head turned away");
        assert!(!in_view(&ahead, &head_turned([0.0, 1.7, 0.0], 0.8)), "46° off");
        assert!(in_view(&ahead, &head_turned([0.0, 1.7, 0.0], 0.5)), "29° off");
        assert!(!in_view(&ahead, &head_at([0.0, 1.7, 3.0])), "walked away from it");
        assert!(!in_view(&ahead, &head_at([0.0, 1.7, -0.85])), "walked past it: behind the head");
        // Facing away from the eyes (seen from its back).
        let turned = compose(&ahead, &head_turned([0.0; 3], std::f32::consts::PI));
        assert!(!in_view(&turned, &head));
    }

    #[test]
    fn a_held_panel_rides_the_hand_turned_towards_the_eyes() {
        let head = head_at([0.0, 1.7, 0.0]);
        let panel = before_eyes(&head, 0.7, 0.12);
        // Taken by its handle, under it.
        let hand = apply(&panel, [0.0, -0.3, 0.05]);
        let then = carry(hand, &head);
        let still = carried(&then, &then, &panel);
        assert!(close(position(&still), position(&panel)) && close(axis(&still, 0), axis(&panel, 0)), "not moved yet: where it was");
        // The hand moves 30 cm right: the panel goes right with it and turns to keep facing the eyes.
        let moved = add(hand, [0.3, 0.0, 0.0]);
        let now = carried(&carry(moved, &head), &then, &panel);
        assert!(position(&now)[0] > 0.3, "{:?}", position(&now));
        let to_eyes = unit(sub(position(&head), position(&now))).unwrap();
        assert!(dot(axis(&now, 2), to_eyes) > 0.98, "still faces the eyes");
        // The hand back where it took it: the panel back where it was.
        let back = carried(&carry(hand, &head), &then, &panel);
        assert!(close(position(&back), position(&panel)) && close(axis(&back, 2), axis(&panel, 2)));
        // Raised 20 cm: it rises the same, its middle as far from the hand as when taken.
        let raised = carried(&carry(add(hand, [0.0, 0.2, 0.0]), &head), &then, &panel);
        let (was, is) = (length(sub(position(&panel), hand)), length(sub(position(&raised), add(hand, [0.0, 0.2, 0.0]))));
        assert!((was - is).abs() < 1e-4);
        // A hand straight under the eyes still makes a frame.
        let under = carry([0.0, 1.0, 0.0], &head);
        assert!(position(&carried(&under, &then, &panel)).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_watch_sits_on_the_back_of_the_wrist_clear_of_the_coverage_panel() {
        for right in [false, true] {
            let w = on_wrist(right);
            let det = dot(axis(&w, 0), cross(axis(&w, 1), axis(&w, 2)));
            assert!((det - 1.0).abs() < 1e-5, "a rotation");
            let face = axis(&w, 2);
            assert_eq!(face[0] > 0.0, right, "out of the back of the hand");
            assert!(face[1] > 0.0 && face[2] > 0.0, "a little up and towards the elbow");
            assert!(axis(&w, 1)[1] > 0.8, "its top towards the thumb");
            // The coverage panel (20 × 15 cm) on the same controller: the watch's disc stays clear of it.
            let coverage = Quad { transform: crate::overlay::coverage::on_hand(), width: crate::overlay::coverage::WIDTH_M, height: 0.15 };
            for k in 0..16 {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                let rim = apply(&w, [a.cos() * super::super::watch::WIDTH_M / 2.0, a.sin() * super::super::watch::WIDTH_M / 2.0, 0.0]);
                assert!(coverage.nearest(rim).1 > 0.015, "{right} {k}: {}", coverage.nearest(rim).1);
            }
        }
    }

    #[test]
    fn the_watch_shows_as_the_wrist_turns_to_the_eyes() {
        let head = head_at([0.0, 1.6, 0.0]);
        // Before the chest, 35 cm ahead and 40 cm down, facing the eyes: shown.
        let at = [0.0, 1.2, -0.35];
        let facing_eyes = facing(at, position(&head), [0.0, 1.0, 0.0]).unwrap();
        let head_looking = facing(position(&head), at, [0.0, 1.0, 0.0]).map(|m| compose(&m, &head_turned([0.0; 3], std::f32::consts::PI))).unwrap();
        // (`facing` turns +Z to the target; a head looks along −Z, hence the half turn.)
        assert!(watch_shown(&facing_eyes, &head_looking) > 0.99);
        // The eyes look ahead instead (49° above it): faded out.
        assert!(watch_shown(&facing_eyes, &head) < 0.01, "{}", watch_shown(&facing_eyes, &head));
        // Looked at, but the wrist turned away (its face sideways): faded out.
        let sideways = compose(&facing_eyes, &head_turned([0.0; 3], std::f32::consts::FRAC_PI_2));
        assert!(watch_shown(&sideways, &head_looking) < 0.01);
        // Half turned: part way.
        let half = compose(&facing_eyes, &head_turned([0.0; 3], 0.95));
        let part = watch_shown(&half, &head_looking);
        assert!(part > 0.05 && part < 0.95, "{part}");
    }

    #[test]
    fn a_beam_runs_along_the_laser_facing_the_eyes() {
        // A tip at the controller's origin pointing down its −Z, the eyes above and behind.
        let tip = Matrix34::IDENTITY;
        let b = beam(&tip, 2.0, [0.0, 0.4, 0.3]);
        assert!(close(position(&b), [0.0, 0.0, -1.0]), "its middle halfway");
        assert!(close(axis(&b, 1), [0.0, 0.0, -1.0]), "its top at the far end");
        let z = axis(&b, 2);
        assert!(z[1] > 0.99 && dot(z, axis(&b, 1)).abs() < 1e-5, "turned up to the eyes, square to the laser: {z:?}");
        // Eyes straight along the laser: any turn, still a rotation.
        let along = beam(&tip, 1.0, [0.0, 0.0, 1.0]);
        assert!((dot(axis(&along, 0), cross(axis(&along, 1), axis(&along, 2))) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_follow_springs_and_settles() {
        let mut f = Follow::at(0.0);
        let mut moving = true;
        let mut steps = 0;
        while moving && steps < 1000 {
            moving = f.step(1.0, 1.0 / 90.0, Spring::SOFT);
            steps += 1;
        }
        assert_eq!(f.value, 1.0, "settles exactly on its target");
        assert!(steps < 200, "within a couple of seconds: {steps}");
        assert!(!f.step(1.0, 0.011, Spring::SOFT), "at rest it stays");
    }
}
