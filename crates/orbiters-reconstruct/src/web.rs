//! Web previews of a reconstruction: a `.splat` file, the layout web viewers (GaussianSplats3D, antimatter15's splat)
//! load progressively: 32 bytes a splat, position and scale as six floats, colour and opacity as four bytes, rotation
//! (w x y z) as four bytes. Right-handed like the PLY (Unity's world with Z negated). It keeps at most `max` splats, the
//! most visible ones (opacity times size), ordered outward from the camera the viewer starts at, so that a progressive
//! load shows the world from where you stand outward.

use crate::gaussians::Gaussians;

pub const SPLAT_BYTES: usize = 32;
/// About 19 MB: what a phone on the website still loads and draws smoothly.
pub const DEFAULT_MAX: usize = 600_000;
/// Splats fainter than this never show and are left out.
const MIN_OPACITY: f32 = 1.0 / 255.0;

/// The `.splat` bytes and how many splats they hold.
pub struct WebPreview {
    pub bytes: Vec<u8>,
    pub count: usize,
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn byte(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `origin` is the start camera in Unity's world.
pub fn splat_file(g: &Gaussians, max: usize, origin: [f32; 3]) -> WebPreview {
    let mut keep: Vec<(usize, f32)> = (0..g.len())
        .filter_map(|i| {
            let opacity = sigmoid(g.opacity_logits[i]);
            let s = g.log_scales[i];
            let size = ((s[0] + s[1] + s[2]) / 3.0).exp();
            (opacity >= MIN_OPACITY && g.means[i].iter().chain(s.iter()).all(|v| v.is_finite())).then_some((i, opacity * size))
        })
        .collect();
    if keep.len() > max {
        keep.select_nth_unstable_by(max, |a, b| b.1.total_cmp(&a.1));
        keep.truncate(max);
    }
    let distance = |i: usize| {
        let m = g.means[i];
        (m[0] - origin[0]).powi(2) + (m[1] - origin[1]).powi(2) + (m[2] - origin[2]).powi(2)
    };
    keep.sort_by(|a, b| distance(a.0).total_cmp(&distance(b.0)));
    let mut bytes = Vec::with_capacity(keep.len() * SPLAT_BYTES);
    for &(i, _) in &keep {
        let [x, y, z] = g.means[i];
        let s = g.log_scales[i];
        let c = g.colors[i];
        let q = g.rotations[i];
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        // Mirroring Z: positions negate z; a rotation R becomes S R S, the quaternion (w, -x, -y, z).
        let rotation = [q[0] / n, -q[1] / n, -q[2] / n, q[3] / n];
        for v in [x, y, -z, s[0].exp(), s[1].exp(), s[2].exp()] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&[byte(c[0]), byte(c[1]), byte(c[2]), byte(sigmoid(g.opacity_logits[i]))]);
        bytes.extend(rotation.map(|v| (v * 128.0 + 128.0).round().clamp(0.0, 255.0) as u8));
    }
    WebPreview { count: keep.len(), bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gaussians(n: usize) -> Gaussians {
        Gaussians {
            means: (0..n).map(|i| [i as f32, 1.0, 2.0]).collect(),
            log_scales: (0..n).map(|i| [(i as f32 * 0.01).ln_1p() - 3.0; 3]).collect(),
            rotations: vec![[1.0, 0.0, 0.0, 0.0]; n],
            opacity_logits: (0..n).map(|i| if i == 0 { -20.0 } else { 2.0 }).collect(),
            colors: vec![[0.5, 0.25, 1.0]; n],
            sh1: vec![[0.0; 9]; n],
        }
    }

    #[test]
    fn keeps_the_most_visible_splats_ordered_outward_in_the_web_layout() {
        let g = gaussians(100);
        let web = splat_file(&g, 50, [99.0, 1.0, 2.0]);
        assert_eq!(web.count, 50);
        assert_eq!(web.bytes.len(), 50 * SPLAT_BYTES);
        let f = |offset: usize| f32::from_le_bytes(web.bytes[offset..offset + 4].try_into().unwrap());
        // The biggest splats are the ones near x = 99, which is also where the viewer starts: nearest first.
        assert_eq!(f(0), 99.0);
        assert!(f(SPLAT_BYTES) == 98.0);
        // Right-handed: z negated; linear scale; colour and opacity bytes; identity rotation.
        assert_eq!(f(8), -2.0);
        assert!((f(12) - g.log_scales[99][0].exp()).abs() < 1e-6);
        assert_eq!(&web.bytes[24..28], &[128, 64, 255, 225]);
        assert_eq!(&web.bytes[28..32], &[255, 128, 128, 128]);
        // The invisible splat is left out even with room to spare.
        assert_eq!(splat_file(&g, 1000, [0.0; 3]).count, 99);
    }
}
