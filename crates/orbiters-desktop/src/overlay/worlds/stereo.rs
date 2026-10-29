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

fn mat_mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
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
/// `p ↦ rotation · p + offset` (a reflection: Unity is left-handed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub rotation: [[f32; 3]; 3],
    pub offset: V3,
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
        Self { rotation, offset: sub(anchor, mat_vec(&rotation, ground_point)) }
    }

    #[cfg(test)]
    pub fn apply(&self, p: V3) -> V3 {
        add(mat_vec(&self.rotation, p), self.offset)
    }

    /// A play-space point back in the capture's coordinates (the rotation is orthonormal).
    #[cfg(test)]
    pub fn invert(&self, q: V3) -> V3 {
        let d = sub(q, self.offset);
        let r = &self.rotation;
        [r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2], r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2], r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2]]
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
        let t = mat_vec(&rows, sub(placement.offset, eye));
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
            t: mat_vec(&self.rows, sub(placement.offset, self.eye)),
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
        let corner = add(add(sub(add(self.center, scale(face.normal, self.half)), wide.eye), scale(right, self.extent() * (1.0 / w - 1.0))), scale(down, self.extent() * (1.0 / w - 1.0)));
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
        t: mat_vec(&rows, sub(placement.offset, position(view))),
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

    fn close(a: V3, b: V3, tol: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
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
