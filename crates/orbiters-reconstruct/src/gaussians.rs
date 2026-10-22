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
    /// Seeded on the sky shell (crate::init::SKY): relocation never copies these (opaque and plentiful, they would
    /// take the budget). Training only; the PLY does not keep it.
    pub sky: Vec<bool>,
    /// Mip-Splatting's 3D filter: a variance (m²) added to each axis while rendering, and the opacity scaled by the
    /// volume ratio so that the Gaussian keeps its weight (crate::train computes it from the training cameras).
    /// Training only: [`Gaussians::fuse_filter`] bakes it into the scales and opacity a PLY keeps.
    pub filter_3d: Vec<f32>,
    /// What the training keyframes saw of each Gaussian ([`Seen::UNKNOWN`] until crate::gpu::Gpu::seen measured it),
    /// which the PLY keeps for viewers.
    pub seen: Vec<Seen>,
}

/// How strongly, how often, from where and how closely the training keyframes saw a Gaussian: what a viewer needs to
/// show how far to trust it (crate::gpu::Gpu::seen).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// The largest blending weight α·T it reached in a scene pixel of any training keyframe (RadSplat's importance
    /// score, which does not grow with the number of keyframes); negative where unknown.
    pub weight: f32,
    /// The training keyframes in which that weight reached [`SEEN_IN`].
    pub views: u32,
    /// Bit s: one of those keyframes looked from compass sector s, by the direction from the Gaussian towards the
    /// camera (0 = +Z, clockwise towards +X, 22.5° each: the capture coverage map's sectors).
    pub sectors: u16,
    /// The finest sampling rate among them: the focal length over the Gaussian's depth (pixels per metre).
    pub rate: f32,
    /// How far it lies beyond what the depth frames reached (metres; crate::cleanup::beyond): the far field a viewer
    /// may fog or crop.
    pub beyond: f32,
}

impl Seen {
    pub const UNKNOWN: Self = Self { weight: -1.0, views: 0, sectors: 0, rate: 0.0, beyond: 0.0 };
}

/// The blending weight at which a keyframe counts as seeing a Gaussian ([`Seen::views`]).
pub const SEEN_IN: f32 = 0.01;

/// Floats per Gaussian in the packed GPU layout (see [`Gaussians::pack`]).
pub const PACKED: usize = 16;
/// Packed slot of the sky flag (1: sky), which no learning rate moves.
pub const SKY_SLOT: usize = 14;
/// Packed slot the device keeps for itself: the Adam step at which this Gaussian's moments last restarted
/// (relocate.wgsl, adam.wgsl). Uploads write 0.
pub const RESTART_SLOT: usize = 15;

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
        self.sky.push(false);
        self.filter_3d.push(0.0);
        self.seen.push(Seen::UNKNOWN);
    }

    pub fn swap_remove(&mut self, i: usize) {
        self.means.swap_remove(i);
        self.log_scales.swap_remove(i);
        self.rotations.swap_remove(i);
        self.opacity_logits.swap_remove(i);
        self.colors.swap_remove(i);
        if i < self.sky.len() {
            self.sky.swap_remove(i);
        }
        if i < self.filter_3d.len() {
            self.filter_3d.swap_remove(i);
        }
        if i < self.seen.len() {
            self.seen.swap_remove(i);
        }
    }

    /// Whether every parameter of Gaussian `i` is a number (a NaN or infinite one renders nowhere and holds a slot).
    pub fn is_finite(&self, i: usize) -> bool {
        self.means[i].iter().chain(&self.log_scales[i]).chain(&self.rotations[i]).chain(&self.colors[i]).chain(std::iter::once(&self.opacity_logits[i])).all(|v| v.is_finite())
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
        filter(&mut self.sky, keep);
        filter(&mut self.filter_3d, keep);
        filter(&mut self.seen, keep);
    }

    /// What the keyframes saw of Gaussian `i` ([`Seen::UNKNOWN`] where nothing was set).
    pub fn seen(&self, i: usize) -> Seen {
        self.seen.get(i).copied().unwrap_or(Seen::UNKNOWN)
    }

    /// Gaussian `i`'s 3D filter (0 where none was set).
    pub fn filter(&self, i: usize) -> f32 {
        self.filter_3d.get(i).copied().unwrap_or(0.0)
    }

    /// Bakes the 3D filter into the parameters: each axis's variance grows by it and the opacity shrinks by the ratio
    /// of the volumes, `Π s / √(s² + σ²)`, which renders exactly what the filter did; the filter is then 0.
    pub fn fuse_filter(&mut self) {
        for i in 0..self.len() {
            let f = self.filter(i) as f64;
            if f <= 0.0 {
                continue;
            }
            let mut ratio = 1.0;
            for s in &mut self.log_scales[i] {
                let s2 = (2.0 * *s as f64).exp();
                ratio *= (s2 / (s2 + f)).sqrt();
                *s = (0.5 * (s2 + f).ln()) as f32;
            }
            self.opacity_logits[i] = logit((sigmoid(self.opacity_logits[i]) as f64 * ratio) as f32);
            self.filter_3d[i] = 0.0;
        }
    }

    /// The parameters as 16 floats each: mean (3), log scales (3), rotation (4), opacity logit (1), colour (3), the
    /// sky flag ([`SKY_SLOT`]) and the device's own slot ([`RESTART_SLOT`], 0).
    pub fn pack(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.len() * PACKED);
        for i in 0..self.len() {
            out.extend_from_slice(&self.means[i]);
            out.extend_from_slice(&self.log_scales[i]);
            out.extend_from_slice(&self.rotations[i]);
            out.push(self.opacity_logits[i]);
            out.extend_from_slice(&self.colors[i]);
            out.extend_from_slice(&[if self.sky.get(i).copied().unwrap_or(false) { 1.0 } else { 0.0 }, 0.0]);
        }
        out
    }

    pub fn unpack(data: &[f32]) -> Self {
        let mut g = Self::default();
        for p in data.as_chunks::<PACKED>().0 {
            g.push([p[0], p[1], p[2]], [p[3], p[4], p[5]], [p[6], p[7], p[8], p[9]], p[10], [p[11], p[12], p[13]]);
            *g.sky.last_mut().expect("pushed") = p[SKY_SLOT] > 0.5;
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
        g.sky[3] = true;
        assert_eq!(Gaussians::unpack(&g.pack()), g);
        // The filter lives in a buffer of its own: the packed layout leaves it out.
        let mut filtered = g.clone();
        filtered.filter_3d[2] = 0.5;
        assert_eq!(filtered.pack(), g.pack());
        assert_eq!(g.pack()[3 * PACKED + SKY_SLOT], 1.0);
        let mut h = g.clone();
        h.retain(&[true, false, true, true, true]);
        assert_eq!(h.means.iter().map(|m| m[0]).collect::<Vec<_>>(), vec![0.0, 2.0, 3.0, 4.0]);
        assert_eq!(h.sky, vec![false, false, true, false]);
        assert_eq!(h.filter_3d.len(), 4);
        h.colors[1][2] = f32::NAN;
        assert!(h.is_finite(0) && !h.is_finite(1));
    }
}
