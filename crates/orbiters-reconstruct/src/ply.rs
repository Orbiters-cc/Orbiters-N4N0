//! Gaussian splat PLY, the layout of the reference 3DGS implementation that viewers and Unity importers read:
//! binary little-endian vertices with `x y z nx ny nz f_dc_0..2 opacity scale_0..2 rot_0..3` (degree-0 colour,
//! opacity as a logit, log scales, rotation w x y z), and `f_rest_0..8` after `f_dc` for a degree-1 model (channel by
//! channel, as 3DGS orders them). Positions are in the right-handed world of the contracts (`Docs/coordinates.md`:
//! Unity's world with Z negated), as other splat tools expect; so are the harmonics' directions (the z basis flips).

use crate::gaussians::Gaussians;
use anyhow::{bail, Context as _};
use std::io::Write as _;
use std::path::Path;

/// Degree-0 spherical harmonic constant: colour = 0.5 + C0 · f_dc.
pub const SH_C0: f32 = 0.282_094_8;
const PROPERTIES: [&str; 17] =
    ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3"];
const REST: [&str; 9] = ["f_rest_0", "f_rest_1", "f_rest_2", "f_rest_3", "f_rest_4", "f_rest_5", "f_rest_6", "f_rest_7", "f_rest_8"];

/// The property names of a degree-0 or degree-1 file.
fn properties(degree1: bool) -> Vec<&'static str> {
    let mut p = PROPERTIES[..9].to_vec();
    if degree1 {
        p.extend(REST);
    }
    p.extend(&PROPERTIES[9..]);
    p
}

/// PLY's f_rest_{c·3 + k} from the trainer's sh1[k·3 + c]: the z basis (k = 1) flips with the mirrored world.
fn rest_from_sh1(sh1: &[f32; 9]) -> [f32; 9] {
    std::array::from_fn(|r| {
        let (c, k) = (r / 3, r % 3);
        if k == 1 {
            -sh1[k * 3 + c]
        } else {
            sh1[k * 3 + c]
        }
    })
}

pub fn write(path: &Path, g: &Gaussians) -> anyhow::Result<()> {
    let degree1 = g.sh1.iter().any(|s| s.iter().any(|v| *v != 0.0));
    let props = properties(degree1);
    let mut out = Vec::with_capacity(400 + g.len() * props.len() * 4);
    write!(out, "ply\nformat binary_little_endian 1.0\ncomment Orbiters desktop reconstruction (right-handed: Unity world with Z negated)\nelement vertex {}\n", g.len())?;
    for p in &props {
        writeln!(out, "property float {p}")?;
    }
    out.extend_from_slice(b"end_header\n");
    for i in 0..g.len() {
        let [x, y, z] = g.means[i];
        // Mirroring Z: positions negate z; a rotation R becomes S R S, the quaternion (w, -x, -y, z).
        let q = g.rotations[i];
        let n = (q.iter().map(|c| c * c).sum::<f32>()).sqrt().max(1e-12);
        let c = g.colors[i];
        let head = [x, y, -z, 0.0, 0.0, 0.0, (c[0] - 0.5) / SH_C0, (c[1] - 0.5) / SH_C0, (c[2] - 0.5) / SH_C0];
        for v in head {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if degree1 {
            for v in rest_from_sh1(&g.sh1[i]) {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        let values = [
            g.opacity_logits[i],
            g.log_scales[i][0],
            g.log_scales[i][1],
            g.log_scales[i][2],
            q[0] / n,
            -q[1] / n,
            -q[2] / n,
            q[3] / n,
        ];
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    let tmp = path.with_extension("ply.part");
    std::fs::write(&tmp, &out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Reads a PLY written by [`write`] back into the training parametrisation (Unity world).
pub fn read(path: &Path) -> anyhow::Result<Gaussians> {
    let data = std::fs::read(path)?;
    let end = data.windows(11).position(|w| w == b"end_header\n").context("no PLY header")? + 11;
    let header = std::str::from_utf8(&data[..end])?;
    let count: usize = header.lines().find_map(|l| l.strip_prefix("element vertex ")).context("no vertex count")?.trim().parse()?;
    let props: Vec<&str> = header.lines().filter_map(|l| l.strip_prefix("property float ")).collect();
    let degree1 = props == properties(true);
    if !degree1 && props != properties(false) {
        bail!("unexpected PLY properties: {props:?}");
    }
    let stride = props.len() * 4;
    // Where opacity and the rest start: after f_rest when there is one.
    let o = if degree1 { 9 } else { 0 };
    if data.len() < end + count * stride {
        bail!("PLY truncated");
    }
    let mut g = Gaussians::default();
    for i in 0..count {
        let f = |k: usize| f32::from_le_bytes(data[end + i * stride + k * 4..end + i * stride + k * 4 + 4].try_into().unwrap());
        g.push(
            [f(0), f(1), -f(2)],
            [f(o + 10), f(o + 11), f(o + 12)],
            [f(o + 13), -f(o + 14), -f(o + 15), f(o + 16)],
            f(o + 9),
            [0.5 + SH_C0 * f(6), 0.5 + SH_C0 * f(7), 0.5 + SH_C0 * f(8)],
        );
        if degree1 {
            // rest_from_sh1 is its own inverse.
            let rest: [f32; 9] = std::array::from_fn(|r| f(9 + r));
            let back: [f32; 9] = std::array::from_fn(|j| {
                let (k, c) = (j / 3, j % 3);
                if k == 1 {
                    -rest[c * 3 + k]
                } else {
                    rest[c * 3 + k]
                }
            });
            *g.sh1.last_mut().expect("just pushed") = back;
        }
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaussians::rotation_matrix;

    #[test]
    fn harmonics_round_trip_and_mirror_with_the_world() {
        let mut g = Gaussians::default();
        g.push([1.0, 2.0, 3.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 0.4, [0.2, 0.5, 0.8]);
        g.sh1[0] = [0.1, -0.2, 0.3, 0.4, -0.5, 0.6, 0.7, -0.8, 0.9];
        let path = std::env::temp_dir().join(format!("orbiters-ply-sh-{}.ply", std::process::id()));
        write(&path, &g).unwrap();
        let back = read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(back.sh1[0].iter().zip(g.sh1[0]).all(|(a, b)| (a - b).abs() < 1e-6), "{:?}", back.sh1[0]);
        // 3DGS evaluates −C1·y·r0 + C1·z·r1 − C1·x·r2 in the file's world, where z is mirrored.
        let rest = rest_from_sh1(&g.sh1[0]);
        let d = [0.36f32, -0.48, 0.8];
        let ours = crate::gaussians::sh1_basis(d);
        for c in 0..3 {
            let trainer: f32 = (0..3).map(|k| ours[k] * g.sh1[0][k * 3 + c]).sum();
            let c1 = crate::gaussians::SH_C1;
            let file = -c1 * d[1] * rest[c * 3] + c1 * (-d[2]) * rest[c * 3 + 1] - c1 * d[0] * rest[c * 3 + 2];
            assert!((trainer - file).abs() < 1e-6, "channel {c}: {trainer} vs {file}");
        }
    }

    #[test]
    fn round_trips_and_mirrors_rotations_with_positions() {
        let mut g = Gaussians::default();
        g.push([1.0, 2.0, 3.0], [-2.0, -3.0, -4.0], [0.9, 0.1, 0.3, -0.2], 0.4, [0.2, 0.5, 0.8]);
        let path = std::env::temp_dir().join(format!("orbiters-ply-{}.ply", std::process::id()));
        write(&path, &g).unwrap();
        let back = read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        let n = (0.81f32 + 0.01 + 0.09 + 0.04).sqrt();
        assert!((back.means[0][2] - 3.0).abs() < 1e-6 && (back.colors[0][2] - 0.8).abs() < 1e-5);
        assert!(back.sh1[0].iter().all(|v| *v == 0.0));
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
}
