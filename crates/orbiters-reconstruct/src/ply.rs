//! Gaussian splat PLY, the layout of the reference 3DGS implementation that viewers and Unity importers read:
//! binary little-endian vertices with `x y z nx ny nz f_dc_0..2 opacity scale_0..2 rot_0..3` (degree-0 colour,
//! opacity as a logit, log scales, rotation w x y z). Positions are in the right-handed world of the contracts
//! (`Docs/coordinates.md`: Unity's world with Z negated), as other splat tools expect. Orbiters adds [`EXTRAS`], which
//! other tools skip: 1 on the sky shell, and what the training keyframes saw of each splat
//! (crate::gaussians::Seen: largest blending weight, negative where unknown; keyframes; compass sector bits; finest
//! sampling rate in pixels per metre; metres beyond what the depth frames reached).

use crate::gaussians::{Gaussians, Seen};
use anyhow::{bail, Context as _};
use std::io::Write as _;
use std::path::Path;

/// Degree-0 spherical harmonic constant: colour = 0.5 + C0 · f_dc.
pub const SH_C0: f32 = 0.282_094_8;
const PROPERTIES: [&str; 17] =
    ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3"];
/// Orbiters' own properties after [`PROPERTIES`] (floats too).
pub const EXTRAS: [&str; 6] = ["sky", "seen_weight", "seen_views", "seen_sectors", "seen_rate", "seen_beyond"];

pub fn write(path: &Path, g: &Gaussians) -> anyhow::Result<()> {
    let mut out = Vec::with_capacity(400 + g.len() * (PROPERTIES.len() + EXTRAS.len()) * 4);
    write!(out, "ply\nformat binary_little_endian 1.0\ncomment Orbiters - N4N0 reconstruction (right-handed: Unity world with Z negated)\nelement vertex {}\n", g.len())?;
    for p in PROPERTIES.iter().chain(&EXTRAS) {
        writeln!(out, "property float {p}")?;
    }
    out.extend_from_slice(b"end_header\n");
    for i in 0..g.len() {
        let [x, y, z] = g.means[i];
        // Mirroring Z: positions negate z; a rotation R becomes S R S, the quaternion (w, -x, -y, z).
        let q = g.rotations[i];
        let n = (q.iter().map(|c| c * c).sum::<f32>()).sqrt().max(1e-12);
        let c = g.colors[i];
        let values = [
            x,
            y,
            -z,
            0.0,
            0.0,
            0.0,
            (c[0] - 0.5) / SH_C0,
            (c[1] - 0.5) / SH_C0,
            (c[2] - 0.5) / SH_C0,
            g.opacity_logits[i],
            g.log_scales[i][0],
            g.log_scales[i][1],
            g.log_scales[i][2],
            q[0] / n,
            -q[1] / n,
            -q[2] / n,
            q[3] / n,
        ];
        let seen = g.seen(i);
        let extras = [if g.sky.get(i).copied().unwrap_or(false) { 1.0 } else { 0.0 }, seen.weight, seen.views as f32, seen.sectors as f32, seen.rate, seen.beyond];
        for v in values.into_iter().chain(extras) {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    let tmp = path.with_extension("ply.part");
    std::fs::write(&tmp, &out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Reads a PLY written by [`write`] back into the training parametrisation (Unity world), or one of the reference
/// layout alone (what the keyframes saw then unknown).
pub fn read(path: &Path) -> anyhow::Result<Gaussians> {
    let data = std::fs::read(path)?;
    let end = data.windows(11).position(|w| w == b"end_header\n").context("no PLY header")? + 11;
    let header = std::str::from_utf8(&data[..end])?;
    let count: usize = header.lines().find_map(|l| l.strip_prefix("element vertex ")).context("no vertex count")?.trim().parse()?;
    let props: Vec<&str> = header.lines().filter_map(|l| l.strip_prefix("property float ")).collect();
    let extras = props.len() > PROPERTIES.len();
    if props[..props.len().min(PROPERTIES.len())] != PROPERTIES || (extras && props[PROPERTIES.len()..] != EXTRAS) {
        bail!("unexpected PLY properties: {props:?}");
    }
    let stride = props.len() * 4;
    // A header may claim any count: one past what the file holds is an error, never an overflow.
    let needed = count.checked_mul(stride).and_then(|b| b.checked_add(end)).context("PLY vertex count out of range")?;
    if data.len() < needed {
        bail!("PLY truncated");
    }
    let mut g = Gaussians::default();
    for i in 0..count {
        let f = |k: usize| f32::from_le_bytes(data[end + i * stride + k * 4..end + i * stride + k * 4 + 4].try_into().unwrap());
        g.push(
            [f(0), f(1), -f(2)],
            [f(10), f(11), f(12)],
            [f(13), -f(14), -f(15), f(16)],
            f(9),
            [0.5 + SH_C0 * f(6), 0.5 + SH_C0 * f(7), 0.5 + SH_C0 * f(8)],
        );
        if extras {
            *g.sky.last_mut().expect("pushed") = f(17) > 0.5;
            *g.seen.last_mut().expect("pushed") = Seen { weight: f(18), views: f(19) as u32, sectors: f(20) as u16, rate: f(21), beyond: f(22) };
        }
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaussians::rotation_matrix;

    #[test]
    fn round_trips_and_mirrors_rotations_with_positions() {
        let mut g = Gaussians::default();
        g.push([1.0, 2.0, 3.0], [-2.0, -3.0, -4.0], [0.9, 0.1, 0.3, -0.2], 0.4, [0.2, 0.5, 0.8]);
        g.push([4.0, 5.0, 6.0], [-1.0; 3], [1.0, 0.0, 0.0, 0.0], 2.0, [0.5; 3]);
        g.sky[1] = true;
        g.seen[0] = Seen { weight: 0.42, views: 7, sectors: 0b1000_0000_0000_0011, rate: 1234.5, beyond: 17.25 };
        let path = std::env::temp_dir().join(format!("orbiters-ply-{}.ply", std::process::id()));
        write(&path, &g).unwrap();
        let back = read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        let n = (0.81f32 + 0.01 + 0.09 + 0.04).sqrt();
        assert!((back.means[0][2] - 3.0).abs() < 1e-6 && (back.colors[0][2] - 0.8).abs() < 1e-5);
        assert_eq!((back.sky.clone(), back.seen.clone()), (g.sky.clone(), g.seen.clone()));
        assert!(back.rotations[0].iter().zip([0.9, 0.1, 0.3, -0.2]).all(|(a, b)| (a - b / n).abs() < 1e-5));
        // The stored rotation is the mirror image: R_rh = S R S.
        let r = rotation_matrix(g.rotations[0]);
        let mirrored = rotation_matrix([0.9, -0.1, -0.3, -0.2]);
        let s = [1.0, 1.0, -1.0];
        for i in 0..3 {
            for j in 0..3 {
                assert!((mirrored[i][j] - s[i] * r[i][j] * s[j]).abs() < 1e-5);
            }
        }
    }

    /// A header claiming more vertices than the file holds, or more than fit in memory, is an error, not a panic.
    #[test]
    fn a_crafted_vertex_count_is_refused() {
        let mut g = Gaussians::default();
        g.push([1.0, 2.0, 3.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 0.4, [0.2, 0.5, 0.8]);
        let path = std::env::temp_dir().join(format!("orbiters-ply-crafted-{}.ply", std::process::id()));
        write(&path, &g).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let line = b"element vertex 1\n";
        let at = bytes.windows(line.len()).position(|w| w == line).expect("vertex line");
        for count in ["2", "18446744073709551615", "1229782938247303441"] {
            let mut crafted = bytes[..at].to_vec();
            crafted.extend_from_slice(format!("element vertex {count}\n").as_bytes());
            crafted.extend_from_slice(&bytes[at + line.len()..]);
            std::fs::write(&path, &crafted).unwrap();
            assert!(read(&path).is_err(), "{count}");
        }
        std::fs::remove_file(&path).ok();
    }
}
