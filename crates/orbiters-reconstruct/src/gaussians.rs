//! A set of 3D Gaussians in the parametrisation the optimiser works in: centre, log scales, rotation quaternion
//! (w, x, y, z, normalised when used), opacity logit and colour (RGB, encoded sRGB like the frames; the PLY stores
//! it as the degree-0 spherical harmonic).

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gaussians {
    pub means: Vec<[f32; 3]>,
    pub log_scales: Vec<[f32; 3]>,
    pub rotations: Vec<[f32; 4]>,
    pub opacity_logits: Vec<f32>,
    pub colors: Vec<[f32; 3]>,
    /// Degree-1 spherical harmonics: `sh1[k * 3 + c]` scales [`sh1_basis`]`[k]` of the direction from the camera in
    /// channel c, on top of the colour (all zero for a degree-0 model).
    pub sh1: Vec<[f32; 9]>,
}

/// Floats per Gaussian in the packed GPU layout (see [`Gaussians::pack`]).
pub const PACKED: usize = 24;

/// Degree-1 spherical harmonic constant.
pub const SH_C1: f32 = 0.488_602_5;

/// The degree-1 basis of a unit direction from the camera to a Gaussian (the reference 3DGS order, in Unity's world).
pub fn sh1_basis(d: [f32; 3]) -> [f32; 3] {
    [-SH_C1 * d[1], SH_C1 * d[2], -SH_C1 * d[0]]
}

impl Gaussians {
    pub fn len(&self) -> usize {
        self.means.len()
    }

    pub fn is_empty(&self) -> bool {
        self.means.is_empty()
    }

    pub fn push(&mut self, mean: [f32; 3], log_scale: [f32; 3], rotation: [f32; 4], opacity_logit: f32, color: [f32; 3]) {
        self.means.push(mean);
        self.log_scales.push(log_scale);
        self.rotations.push(rotation);
        self.opacity_logits.push(opacity_logit);
        self.colors.push(color);
        self.sh1.push([0.0; 9]);
    }

    pub fn swap_remove(&mut self, i: usize) {
        self.means.swap_remove(i);
        self.log_scales.swap_remove(i);
        self.rotations.swap_remove(i);
        self.opacity_logits.swap_remove(i);
        self.colors.swap_remove(i);
        self.sh1.swap_remove(i);
    }

    /// Keeps the Gaussians for which `keep` is true, in order.
    pub fn retain(&mut self, keep: &[bool]) {
        fn filter<T: Copy>(v: &mut Vec<T>, keep: &[bool]) {
            let mut i = 0;
            v.retain(|_| {
                let k = keep[i];
                i += 1;
                k
            });
        }
        filter(&mut self.means, keep);
        filter(&mut self.log_scales, keep);
        filter(&mut self.rotations, keep);
        filter(&mut self.opacity_logits, keep);
        filter(&mut self.colors, keep);
        filter(&mut self.sh1, keep);
    }

    /// The parameters as 24 floats each: mean (3), log scales (3), rotation (4), opacity logit (1), colour (3), degree-1
    /// harmonics (9), pad (1).
    pub fn pack(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.len() * PACKED);
        for i in 0..self.len() {
            out.extend_from_slice(&self.means[i]);
            out.extend_from_slice(&self.log_scales[i]);
            out.extend_from_slice(&self.rotations[i]);
            out.push(self.opacity_logits[i]);
            out.extend_from_slice(&self.colors[i]);
            out.extend_from_slice(&self.sh1[i]);
            out.push(0.0);
        }
        out
    }

    pub fn unpack(data: &[f32]) -> Self {
        let mut g = Self::default();
        for p in data.as_chunks::<PACKED>().0 {
            g.push([p[0], p[1], p[2]], [p[3], p[4], p[5]], [p[6], p[7], p[8], p[9]], p[10], [p[11], p[12], p[13]]);
            *g.sh1.last_mut().expect("just pushed") = std::array::from_fn(|k| p[14 + k]);
        }
        g
    }
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

pub fn logit(p: f32) -> f32 {
    let p = p.clamp(1e-6, 1.0 - 1e-6);
    (p / (1.0 - p)).ln()
}

/// Rotation matrix of a quaternion (w, x, y, z), normalised first.
pub fn rotation_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-12);
    let [w, x, y, z] = q.map(|c| c / n);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// The quaternion (w, x, y, z) rotating +Z onto unit vector `n`.
pub fn quat_from_z(n: [f32; 3]) -> [f32; 4] {
    // Half-way vector between +Z and n.
    let (x, y, z) = (n[0], n[1], n[2]);
    if z < -0.999_999 {
        return [0.0, 1.0, 0.0, 0.0];
    }
    let w = 1.0 + z;
    let q = [w, -y, x, 0.0];
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    q.map(|c| c / len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternion_from_z_maps_z_to_the_normal() {
        for n in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.6, 0.0, -0.8], [0.0, 0.0, -1.0]] {
            let r = rotation_matrix(quat_from_z(n));
            let z = [r[0][2], r[1][2], r[2][2]];
            assert!(z.iter().zip(n).all(|(a, b)| (a - b).abs() < 1e-5), "{n:?} → {z:?}");
        }
    }

    #[test]
    fn pack_round_trips_and_retain_keeps_order() {
        let mut g = Gaussians::default();
        for i in 0..5 {
            let f = i as f32;
            g.push([f, f + 1.0, f + 2.0], [-f; 3], [1.0, 0.0, 0.0, f], f * 0.1, [0.5; 3]);
        }
        assert_eq!(Gaussians::unpack(&g.pack()), g);
        let mut h = g.clone();
        h.retain(&[true, false, true, false, true]);
        assert_eq!(h.means.iter().map(|m| m[0]).collect::<Vec<_>>(), vec![0.0, 2.0, 4.0]);
    }
}
