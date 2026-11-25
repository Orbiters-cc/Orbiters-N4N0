//! Gaussian splat PLY, the layout of the reference 3DGS implementation that viewers and Unity importers read:
//! binary little-endian vertices with `x y z nx ny nz f_dc_0..2 opacity scale_0..2 rot_0..3` (degree-0 colour,
//! opacity as a logit, log scales, rotation w x y z). Positions are in the right-handed world of the contracts
//! (`Docs/coordinates.md`: Unity's world with Z negated), as other splat tools expect. Orbiters adds [`EXTRAS`], which
//! other tools skip: 1 on the sky shell, and what the training keyframes saw of each splat
//! (crate::gaussians::Seen: largest blending weight, negative where unknown; keyframes; compass sector bits; finest
//! sampling rate in pixels per metre; metres beyond what the depth frames reached).

use crate::gaussians::{Gaussians, Seen};
use anyhow::{bail, Context as _};
use orbiters_i18n::tr;
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
    std::fs::write(&tmp, &out).with_context(|| tr!("writing {path}", path = tmp.display()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Reads a PLY written by [`write`] back into the training parametrisation (Unity world), or one of the reference
/// layout alone (what the keyframes saw then unknown).
pub fn read(path: &Path) -> anyhow::Result<Gaussians> {
    parse(&std::fs::read(path)?)
}

/// [`read`] of a PLY's bytes.
pub fn parse(data: &[u8]) -> anyhow::Result<Gaussians> {
    let mut p = Parser::new(data.len() as u64);
    p.feed(data)?;
    p.finish()
}

/// The longest header looked for: ours are under a kilobyte.
const HEADER_MAX: usize = 1 << 20;

/// [`parse`] of a PLY of `total` bytes that comes in pieces: what is held beside the Gaussians read is one piece.
pub struct Parser {
    total: u64,
    /// The header until it is whole, then what is left of the last piece: part of a splat.
    left: Vec<u8>,
    layout: Option<Layout>,
    g: Gaussians,
}

#[derive(Clone, Copy)]
struct Layout {
    count: usize,
    stride: usize,
    extras: bool,
}

impl Parser {
    pub fn new(total: u64) -> Self {
        Self { total, left: Vec::new(), layout: None, g: Gaussians::default() }
    }

    /// The next piece.
    pub fn feed(&mut self, mut piece: &[u8]) -> anyhow::Result<()> {
        let layout = match self.layout {
            Some(l) => l,
            None => {
                let before = self.left.len();
                self.left.extend_from_slice(&piece[..piece.len().min(HEADER_MAX - before)]);
                let from = before.saturating_sub(10);
                let Some(at) = self.left[from..].windows(11).position(|w| w == b"end_header\n") else {
                    anyhow::ensure!(self.left.len() < HEADER_MAX, "{}", tr("no PLY header"));
                    return Ok(());
                };
                let end = from + at + 11;
                let l = layout(&self.left[..end], self.total)?;
                // The count is checked against the file's length: reserving it can never take more than the file is.
                self.g.reserve_exact(l.count);
                self.layout = Some(l);
                piece = &piece[end - before..];
                self.left.clear();
                l
            }
        };
        if !self.left.is_empty() {
            let take = (layout.stride - self.left.len()).min(piece.len());
            self.left.extend_from_slice(&piece[..take]);
            piece = &piece[take..];
            if self.left.len() < layout.stride {
                return Ok(());
            }
            let record = std::mem::take(&mut self.left);
            self.splat(layout, &record);
        }
        let records = piece.chunks_exact(layout.stride);
        let rest = records.remainder();
        for record in records {
            self.splat(layout, record);
        }
        self.left.extend_from_slice(rest);
        Ok(())
    }

    /// One splat's values (past the header's count: whatever follows the splats, which is no splat).
    fn splat(&mut self, l: Layout, record: &[u8]) {
        if self.g.len() >= l.count {
            return;
        }
        let f = |k: usize| f32::from_le_bytes(record[k * 4..k * 4 + 4].try_into().expect("four bytes"));
        let g = &mut self.g;
        g.push(
            [f(0), f(1), -f(2)],
            [f(10), f(11), f(12)],
            [f(13), -f(14), -f(15), f(16)],
            f(9),
            [0.5 + SH_C0 * f(6), 0.5 + SH_C0 * f(7), 0.5 + SH_C0 * f(8)],
        );
        if l.extras {
            *g.sky.last_mut().expect("pushed") = f(17) > 0.5;
            *g.seen.last_mut().expect("pushed") = Seen { weight: f(18), views: f(19) as u32, sectors: f(20) as u16, rate: f(21), beyond: f(22) };
        }
    }

    pub fn finish(self) -> anyhow::Result<Gaussians> {
        let Some(l) = self.layout else { bail!("{}", tr("no PLY header")) };
        if self.g.len() < l.count {
            bail!("{}", tr("PLY truncated"));
        }
        Ok(self.g)
    }
}

/// What a PLY's `header` (up to its end) says of the splats after it, for a file of `total` bytes.
fn layout(header: &[u8], total: u64) -> anyhow::Result<Layout> {
    let end = header.len();
    let header = std::str::from_utf8(header)?;
    let count: usize = header.lines().find_map(|l| l.strip_prefix("element vertex ")).with_context(|| tr("no vertex count"))?.trim().parse()?;
    let props: Vec<&str> = header.lines().filter_map(|l| l.strip_prefix("property float ")).collect();
    let extras = props.len() > PROPERTIES.len();
    if props[..props.len().min(PROPERTIES.len())] != PROPERTIES || (extras && props[PROPERTIES.len()..] != EXTRAS) {
        bail!("{}", tr!("unexpected PLY properties: {properties}", properties = format!("{props:?}")));
    }
    let stride = props.len() * 4;
    // A header may claim any count: one past what the file holds is an error, never an overflow.
    let needed = count.checked_mul(stride).and_then(|b| b.checked_add(end)).with_context(|| tr("PLY vertex count out of range"))?;
    if total < needed as u64 {
        bail!("{}", tr("PLY truncated"));
    }
    Ok(Layout { count, stride, extras })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PLY read in pieces of any size is the PLY read whole, and one cut short or of another kind is as refused.
    #[test]
    fn a_ply_read_in_pieces_is_the_ply_read_whole() {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(4);
        for i in 0..50 {
            g.push([rng.f32(), rng.f32(), rng.f32()], [-3.0; 3], [1.0, 0.0, rng.f32(), 0.0], rng.f32(), [rng.f32(), 0.5, 0.2]);
            g.sky[i] = i % 7 == 0;
        }
        let path = std::env::temp_dir().join(format!("orbiters-ply-pieces-{}.ply", std::process::id()));
        write(&path, &g).unwrap();
        let mut data = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        // Bytes after the splats are no splat.
        data.extend([7u8; 300]);
        let whole = parse(&data).unwrap();
        assert_eq!((whole.len(), &whole.means, &whole.sky), (50, &g.means, &g.sky));
        let read = |data: &[u8], largest: usize| {
            let mut rng = fastrand::Rng::with_seed(largest as u64);
            let mut p = Parser::new(data.len() as u64);
            let mut at = 0;
            while at < data.len() {
                let take = rng.usize(1..=largest).min(data.len() - at);
                p.feed(&data[at..at + take])?;
                at += take;
            }
            p.finish()
        };
        for largest in [1, 7, 11, 91, 92, 93, 500, 5000] {
            let pieces = read(&data, largest).unwrap();
            assert_eq!((&pieces.means, &pieces.rotations, &pieces.colors, &pieces.seen, &pieces.sky), (&whole.means, &whole.rotations, &whole.colors, &whole.seen, &whole.sky), "pieces of up to {largest}");
        }
        for cut in [100, data.len() - 300 - 1] {
            assert!(parse(&data[..cut]).is_err() && read(&data[..cut], 64).is_err(), "cut at {cut}");
        }
        assert!(parse(b"not a PLY").is_err());
    }
    use crate::gaussians::rotation_matrix;

    /// An error reads in the language of the window the worker reports to: a toast shows it as it is.
    #[test]
    fn a_damaged_file_is_reported_in_the_language_of_the_window() {
        use orbiters_i18n::{in_language, Language};
        assert_eq!(parse(b"").unwrap_err().to_string(), "no PLY header");
        assert_eq!(in_language(Language::Fr, || parse(b"").unwrap_err().to_string()), "pas d’en-tête PLY");
        assert_eq!(in_language(Language::Ja, || parse(b"").unwrap_err().to_string()), "PLYのヘッダーがありません");
    }

    /// RECONSTRUCT-14g: a PLY's splats are read into vectors sized once from its header, the same splats as before.
    #[test]
    fn reading_sizes_the_vectors_once() {
        let mut g = Gaussians::default();
        for i in 0..1000 {
            g.push([i as f32, 0.0, 1.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 0.1, [0.5; 3]);
        }
        let path = std::env::temp_dir().join(format!("orbiters-ply-capacity-{}.ply", std::process::id()));
        write(&path, &g).unwrap();
        let back = read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.len(), 1000);
        assert_eq!((back.means.capacity(), back.seen.capacity(), back.sky.capacity()), (1000, 1000, 1000), "grown by doubling (1024)");
        assert_eq!(back.means, g.means);
    }

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
