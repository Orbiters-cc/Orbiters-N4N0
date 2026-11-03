//! Column-major 4×4 matrices (Unity's `Matrix4x4` order) for posing a scene.

pub const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Translation, rotation (a quaternion x, y, z, w; normalised here) and scale, as a transform's local matrix.
pub fn trs(t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> [f32; 16] {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let [x, y, z, w] = if l > 1e-12 { q.map(|c| c / l) } else { [0.0, 0.0, 0.0, 1.0] };
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    [
        r[0][0] * s[0],
        r[0][1] * s[0],
        r[0][2] * s[0],
        0.0,
        r[1][0] * s[1],
        r[1][1] * s[1],
        r[1][2] * s[1],
        0.0,
        r[2][0] * s[2],
        r[2][1] * s[2],
        r[2][2] * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

pub fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

pub fn point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

pub fn vector(m: &[f32; 16], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2])
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    super::model::normalize(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transform_rotates_then_moves() {
        // 90° about Y: +Z turns to +X.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let m = trs([1.0, 2.0, 3.0], [0.0, h, 0.0, h], [2.0, 2.0, 2.0]);
        let p = point(&m, [0.0, 0.0, 1.0]);
        assert!((p[0] - 3.0).abs() < 1e-5 && (p[1] - 2.0).abs() < 1e-5 && (p[2] - 3.0).abs() < 1e-5, "{p:?}");
        assert_eq!(mul(&IDENTITY, &m), m);
    }
}
