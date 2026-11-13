//! Rays from an eye's pixels into the room and back, in SteamVR's standing space: what places a line of text read in a
//! picture where it is in the room. The conventions are SteamVR's and the overlay's (`camera_track.rs`): a 3 x 4 matrix of
//! rows (rotation | translation), an eye looking down its -Z with +Y up, and pictures whose sides are tangents of the
//! eye's field ([`Mirror`]), right and *down*.

pub type V3 = [f32; 3];
/// Rows of a rigid transform: rotation columns are the axes, the last column the origin.
pub type M34 = [[f32; 4]; 3];

pub const IDENTITY: M34 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn norm(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalized(a: V3) -> V3 {
    scale(a, 1.0 / norm(a).max(1e-9))
}
pub fn lerp(a: V3, b: V3, t: f32) -> V3 {
    add(a, scale(sub(b, a), t))
}

pub fn origin(m: &M34) -> V3 {
    [m[0][3], m[1][3], m[2][3]]
}
pub fn axis(m: &M34, i: usize) -> V3 {
    [m[0][i], m[1][i], m[2][i]]
}
pub fn apply(m: &M34, p: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
}
pub fn rotate(m: &M34, v: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}
/// `a` after `b`: a point in `b`'s space taken to `a`'s parent.
pub fn compose(a: &M34, b: &M34) -> M34 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum::<f32>() + if j == 3 { a[i][3] } else { 0.0 }))
}
/// The inverse of a rigid transform.
pub fn inverse(m: &M34) -> M34 {
    let t = origin(m);
    std::array::from_fn(|i| std::array::from_fn(|j| if j < 3 { m[j][i] } else { -(m[0][i] * t[0] + m[1][i] * t[1] + m[2][i] * t[2]) }))
}
/// The transform whose axes are `x`, `y`, `z` (unit, at right angles) at `at`.
pub fn from_axes(x: V3, y: V3, z: V3, at: V3) -> M34 {
    [[x[0], y[0], z[0], at[0]], [x[1], y[1], z[1], at[1]], [x[2], y[2], z[2], at[2]]]
}
/// A rotation of `angle` radians about `axis`, at `at`.
pub fn rotation(axis_: V3, angle: f32, at: V3) -> M34 {
    let [x, y, z] = normalized(axis_);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [[t * x * x + c, t * x * y - s * z, t * x * z + s * y, at[0]], [t * x * y + s * z, t * y * y + c, t * y * z - s * x, at[1]], [t * x * z - s * y, t * y * z + s * x, t * z * z + c, at[2]]]
}

/// What a picture covers of an eye's field, in tangents (right, down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    pub from: [f32; 2],
    pub spans: [f32; 2],
}

impl Mirror {
    /// The whole eye from SteamVR's `GetProjectionRaw` (left, right, top, bottom). SteamVR's bottom is the tangent of the
    /// side *up*, whatever its name (the overlay's `camera_track::eye_sides` says how that was found).
    pub fn whole_eye(raw: [f32; 4]) -> Self {
        let (up, below) = (raw[2].max(raw[3]), raw[2].min(raw[3]));
        let [left, right, top, bottom] = [raw[0].min(raw[1]), raw[0].max(raw[1]), -up, -below];
        Self { from: [left, top], spans: [right - left, bottom - top] }
    }
    pub fn tangent(&self, u: f32, v: f32) -> [f32; 2] {
        [self.from[0] + u * self.spans[0], self.from[1] + v * self.spans[1]]
    }
}

/// An eye as it was when its picture was taken: where it was in the room, its field and its picture's size in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeView {
    /// Eye to standing space (the head at capture composed with SteamVR's eye-to-head).
    pub eye: M34,
    pub mirror: Mirror,
    /// The whole eye's picture, pixels.
    pub size: [f32; 2],
}

impl EyeView {
    /// The ray through a pixel of the whole eye's picture: origin and unit direction, standing space.
    pub fn ray(&self, px: [f32; 2]) -> (V3, V3) {
        let t = self.mirror.tangent(px[0] / self.size[0], px[1] / self.size[1]);
        (origin(&self.eye), normalized(rotate(&self.eye, [t[0], -t[1], -1.0])))
    }
    /// Where a point of the room shows in the picture, pixels; `None` behind the eye.
    pub fn project(&self, world: V3) -> Option<[f32; 2]> {
        let v = apply(&inverse(&self.eye), world);
        if v[2] > -1e-4 {
            return None;
        }
        let t = [v[0] / -v[2], -v[1] / -v[2]];
        Some([(t[0] - self.mirror.from[0]) / self.mirror.spans[0] * self.size[0], (t[1] - self.mirror.from[1]) / self.mirror.spans[1] * self.size[1]])
    }
    /// Pixels a radian across, about the picture's middle.
    pub fn pixels_per_radian(&self) -> f32 {
        self.size[0] / self.mirror.spans[0]
    }
}

/// The closest approach of two rays: the point halfway between them, how far apart they pass, and how far along each.
/// `None` for rays that are parallel or meet behind either eye.
pub fn closest_approach(o0: V3, d0: V3, o1: V3, d1: V3) -> Option<(V3, f32, f32, f32)> {
    // In f64: two eyes' rays to a point metres away are nearly parallel, and 1 - b² loses most of f32's digits.
    let f = |v: V3| v.map(|x| x as f64);
    let dot64 = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (o0, d0, o1, d1) = (f(o0), f(d0), f(o1), f(d1));
    let w = [o0[0] - o1[0], o0[1] - o1[1], o0[2] - o1[2]];
    let (b, d, e) = (dot64(d0, d1), dot64(d0, w), dot64(d1, w));
    let den = dot64(d0, d0) * dot64(d1, d1) - b * b;
    if den < 1e-12 {
        return None;
    }
    let (s, t) = ((b * e - d * dot64(d1, d1)) / den, (dot64(d0, d0) * e - b * d) / den);
    if s <= 0.0 || t <= 0.0 {
        return None;
    }
    let a: [f64; 3] = std::array::from_fn(|i| o0[i] + d0[i] * s);
    let c: [f64; 3] = std::array::from_fn(|i| o1[i] + d1[i] * t);
    let gap = ((a[0] - c[0]).powi(2) + (a[1] - c[1]).powi(2) + (a[2] - c[2]).powi(2)).sqrt();
    Some((std::array::from_fn(|i| ((a[i] + c[i]) / 2.0) as f32), gap as f32, s as f32, t as f32))
}

/// Where a ray meets the plane through `point` with normal `normal`: how far along it.
pub fn ray_plane(o: V3, d: V3, point: V3, normal: V3) -> Option<f32> {
    let den = dot(d, normal);
    if den.abs() < 1e-6 {
        return None;
    }
    let t = dot(sub(point, o), normal) / den;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEST_PRO: [f32; 4] = [-1.376, 0.839, -1.376, 0.900];

    #[test]
    fn a_pixel_and_its_ray_meet_again() {
        let head = compose(&rotation([0.0, 1.0, 0.0], 0.4, [0.3, 1.6, -0.2]), &rotation([1.0, 0.0, 0.0], -0.2, [0.0; 3]));
        let eye = compose(&head, &[[1.0, 0.0, 0.0, -0.032], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        let view = EyeView { eye, mirror: Mirror::whole_eye(QUEST_PRO), size: [1620.0, 1800.0] };
        for px in [[100.0, 200.0], [810.0, 900.0], [1500.0, 1700.0]] {
            let (o, d) = view.ray(px);
            let back = view.project(add(o, scale(d, 2.5))).unwrap();
            assert!((back[0] - px[0]).abs() < 0.05 && (back[1] - px[1]).abs() < 0.05, "{px:?} -> {back:?}");
        }
    }

    #[test]
    fn two_eyes_find_a_point_again() {
        let p = [0.2, 1.5, -2.0];
        let eyes = [[-0.032, 1.6, 0.0], [0.032, 1.6, 0.0]];
        let rays: Vec<(V3, V3)> = eyes.iter().map(|&e| (e, normalized(sub(p, e)))).collect();
        let (mid, gap, s, _) = closest_approach(rays[0].0, rays[0].1, rays[1].0, rays[1].1).unwrap();
        assert!(gap < 1e-4 && norm(sub(mid, p)) < 1e-4 && s > 2.0, "{gap} {mid:?} {s}");
    }

    #[test]
    fn inverse_undoes() {
        let m = rotation([0.3, 1.0, 0.2], 1.1, [1.0, 2.0, 3.0]);
        let p = [0.5, -0.2, 0.9];
        let back = apply(&inverse(&m), apply(&m, p));
        assert!(norm(sub(back, p)) < 1e-5);
    }
}
