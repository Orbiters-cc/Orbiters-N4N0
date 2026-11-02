//! Web previews of a reconstruction: a `.splat` file, the layout web viewers (GaussianSplats3D, antimatter15's splat)
//! load progressively: 32 bytes a splat, position and scale as six floats, colour and opacity as four bytes, rotation
//! (w x y z) as four bytes. Right-handed like the PLY (Unity's world with Z negated). It keeps at most `max` splats, the
//! most visible ones (opacity times size), ordered outward from the camera the viewer starts at, so that a progressive
//! load shows the world from where you stand outward.

use crate::gaussians::Gaussians;

pub const SPLAT_BYTES: usize = 32;
/// About 19 MB: what a phone on the website still loads and draws smoothly.
pub const DEFAULT_MAX: usize = 600_000;

/// A downloaded web preview (the server makes it from the PC tree's leaves: at most crate::tree_tune::PC_SPLATS), back in
/// the renderer's Unity coordinates. Reject malformed or non-finite splats.
pub fn read_splat(bytes: &[u8]) -> anyhow::Result<Gaussians> {
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len().is_multiple_of(SPLAT_BYTES) && bytes.len() / SPLAT_BYTES <= crate::tree_tune::PC_SPLATS,
        "Invalid splat file"
    );
    let mut g = Gaussians::default();
    for b in bytes.chunks_exact(SPLAT_BYTES) {
        let f = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let position = [f(0), f(4), -f(8)];
        let scales = [f(12), f(16), f(20)];
        anyhow::ensure!(position.iter().chain(scales.iter()).all(|v| v.is_finite()) && scales.iter().all(|v| *v > 0.0), "Invalid splat position or scale");
        let mut q = [(b[28] as f32 - 128.0) / 128.0, -(b[29] as f32 - 128.0) / 128.0, -(b[30] as f32 - 128.0) / 128.0, (b[31] as f32 - 128.0) / 128.0];
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        anyhow::ensure!(n > 0.01, "Invalid splat rotation");
        for v in &mut q {
            *v /= n;
        }
        let alpha = (b[27] as f32 / 255.0).clamp(1e-6, 1.0 - 1e-6);
        g.means.push(position);
        g.log_scales.push(scales.map(f32::ln));
        g.rotations.push(q);
        g.opacity_logits.push((alpha / (1.0 - alpha)).ln());
        g.colors.push([b[24] as f32 / 255.0, b[25] as f32 / 255.0, b[26] as f32 / 255.0]);
        g.sky.push(false);
        g.filter_3d.push(0.0);
        g.seen.push(crate::gaussians::Seen::UNKNOWN);
    }
    Ok(g)
}
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
        write_splat(&mut bytes, g, i);
    }
    WebPreview { count: keep.len(), bytes }
}

/// Splat `i` of `g` in the `.splat` layout (also each splat of a VRChat tree, crate::tree).
pub fn write_splat(bytes: &mut Vec<u8>, g: &Gaussians, i: usize) {
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
    bytes.extend(rotation_bytes(rotation));
}

/// A unit quaternion (w, x, y, z) as the record's four bytes `q · 128 + 128`: of the byte quadruples one step around it
/// (each component rounded down or up, for `q` and for `-q`, the same rotation), the one whose decoded quaternion (every
/// reader normalises it: crate::web::read_splat, the VRChat world's SplatDecode) is nearest the true rotation. Rounding
/// each component alone cost the plaza walk's held-out views 0.12 dB (the rotation alone through the record, lab
/// examples/treeloss.rs --diag); this cuts the mean angular error by a quarter (0.0075 to 0.0057 rad over random
/// rotations), and every byte stays within one step of that rounding.
pub fn rotation_bytes(q: [f32; 4]) -> [u8; 4] {
    let decode = |b: [u8; 4]| {
        let d = b.map(|v| (v as f32 - 128.0) / 128.0);
        let n = d.iter().map(|v| v * v).sum::<f32>().sqrt();
        (n > 1e-6).then(|| d.map(|v| v / n))
    };
    let mut best = q.map(|v| (v * 128.0 + 128.0).round().clamp(0.0, 255.0) as u8);
    let mut best_dot = decode(best).map_or(-1.0, |d| (0..4).map(|k| d[k] * q[k]).sum::<f32>().abs());
    for sign in [1.0f32, -1.0] {
        let scaled = q.map(|v| (sign * v * 128.0 + 128.0).clamp(0.0, 255.0));
        for corner in 0..16u32 {
            let b: [u8; 4] = std::array::from_fn(|k| if corner >> k & 1 == 1 { scaled[k].ceil() } else { scaled[k].floor() } as u8);
            if let Some(d) = decode(b) {
                let dot = (0..4).map(|k| d[k] * q[k]).sum::<f32>().abs();
                if dot > best_dot {
                    (best, best_dot) = (b, dot);
                }
            }
        }
    }
    best
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
            sky: vec![false; n],
            filter_3d: vec![0.0; n],
            seen: vec![crate::gaussians::Seen::UNKNOWN; n],
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
    #[test]
    fn rotation_bytes_are_the_nearest_rotation_one_step_around_the_rounding() {
        let mut rng = fastrand::Rng::with_seed(9);
        let angle = |q: [f32; 4], b: [u8; 4]| {
            let d = b.map(|v| (v as f32 - 128.0) / 128.0);
            let n = d.iter().map(|v| v * v).sum::<f32>().sqrt();
            2.0 * ((0..4).map(|k| d[k] / n * q[k]).sum::<f32>().abs().min(1.0)).acos()
        };
        let (mut rounded, mut nearest) = (0.0f64, 0.0f64);
        for _ in 0..20_000 {
            let q = [rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5];
            let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            let q = q.map(|v| v / n);
            let plain = q.map(|v| (v * 128.0 + 128.0).round().clamp(0.0, 255.0) as u8);
            let b = rotation_bytes(q);
            let (a_plain, a_best) = (angle(q, plain), angle(q, b));
            assert!(a_best <= a_plain + 1e-6, "{q:?}: {a_best} rad against {a_plain}");
            // One step around q's rounding or -q's.
            let minus = q.map(|v| (-v * 128.0 + 128.0).round().clamp(0.0, 255.0) as u8);
            assert!(b.iter().zip(&plain).all(|(x, y)| x.abs_diff(*y) <= 1) || b.iter().zip(&minus).all(|(x, y)| x.abs_diff(*y) <= 1));
            rounded += a_plain as f64;
            nearest += a_best as f64;
        }
        assert!(nearest < 0.85 * rounded, "mean error {nearest} against {rounded}");
    }

    #[test]
    fn downloaded_splats_preserve_coordinates_and_reject_corruption() {
        let mut g = Gaussians::default();
        g.means.push([1.0, 2.0, 3.0]);
        g.log_scales.push([-1.0, -2.0, -3.0]);
        g.rotations.push([1.0, 0.0, 0.0, 0.0]);
        g.colors.push([0.2, 0.4, 0.6]);
        g.opacity_logits.push(0.0);
        let mut bytes = Vec::new();
        write_splat(&mut bytes, &g, 0);
        let back = read_splat(&bytes).unwrap();
        assert_eq!(back.means, g.means);
        assert_eq!(back.rotations[0], [1.0, 0.0, 0.0, 0.0]);
        assert!((back.log_scales[0][1] + 2.0).abs() < 1e-5);
        assert!(read_splat(&bytes[..31]).is_err());
        bytes[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(read_splat(&bytes).is_err());
    }
}
