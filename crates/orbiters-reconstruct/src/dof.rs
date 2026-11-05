//! A viewer's depth of field (crate::gpu::Gpu::dof, shaders/project.wgsl): each splat blurred on the GPU by its own
//! circle of confusion, a thin lens's, for its depth along the view. A point `z` metres ahead, seen through a lens of
//! `aperture` metres across focused at `focus` metres, spreads over a disc `fy · aperture · |1/focus − 1/z|` pixels
//! across (`fy`: the camera's focal length in pixels): nothing at the focus, growing on either side of it, towards
//! `fy · aperture / focus` far behind it. The splat's 2D covariance grows by that disc's variance (a disc of radius r
//! has r²/4 along each axis) and its opacity falls as its footprint grows, so the light it gives is kept (as the 2D filter
//! keeps it). Foreground and background scale the disc in front of the focus and behind it.
//!
//! Auto focus (crate::gpu::Gpu::probe): the projection pass also adds, at one pixel, each splat's optical depth there
//! (−ln(1 − α), α as the raster sees it and without the depth of field's blur) into [`BINS`] bins of depth. Light
//! through a set of splats keeps `Π(1 − α) = e^(−Σ optical depth)` in any order, so the bins say, without sorting, at
//! what depth what is under the pixel has hidden half of what lies behind it: the nearest surface that dominates the
//! pixel ([`focus_of`]). Under the sky, or nothing that hides half, the focus is far away ([`FAR_FOCUS`]).

/// Bins of the auto focus's depth histogram, from [`NEAREST`] to [`FARTHEST`] metres, evenly in the log of the depth.
pub const BINS: usize = 256;
pub const NEAREST: f32 = 0.05;
pub const FARTHEST: f32 = 2000.0;
/// The histogram's fixed point: optical depth × this per splat (a splat at most −ln(0.01) ≈ 4.6, so a bin holds
/// 220 000 opaque splats before it would wrap).
pub const SCALE: f32 = 4096.0;
/// Where the focus goes when nothing under the dot hides half of what is behind it (the sky, the background).
pub const FAR_FOCUS: f32 = 1000.0;
/// The aperture at full strength, in metres across: a subject 2 m away in a 60° view with the background far behind
/// it puts the background in discs about 8 % of the picture's height across.
pub const APERTURE_MAX: f32 = 0.2;
/// The largest disc, as a share of the picture's height (a splat near the eye focused far away would otherwise cover
/// the view, and cost as much).
pub const MAX_COC: f32 = 0.12;

/// The depth of field a viewer draws with. Off with an aperture of 0 (the default).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthOfField {
    /// The lens's opening, metres across (0: everything sharp).
    pub aperture: f32,
    /// Metres along the view to what is sharp.
    pub focus: f32,
    /// 0 to 1: how much of the disc shows in front of the focus and behind it.
    pub near: f32,
    pub far: f32,
    /// Tints the splats at the focus (while a focus distance is being set).
    pub show_plane: bool,
}

impl DepthOfField {
    pub fn on(&self) -> bool {
        self.aperture > 0.0 && self.focus > 0.0 && (self.near > 0.0 || self.far > 0.0)
    }

    /// The disc a point `z` metres along the view spreads over, in pixels across, for a camera of focal length `fy`
    /// pixels and a picture `height` pixels high (as project.wgsl computes it).
    pub fn coc(&self, fy: f32, height: f32, z: f32) -> f32 {
        if !self.on() || z <= 0.0 {
            return 0.0;
        }
        let d = 1.0 / self.focus - 1.0 / z;
        let side = if d > 0.0 { self.far } else { self.near };
        (fy * self.aperture * d.abs() * side.clamp(0.0, 1.0)).min(MAX_COC * height)
    }

    /// The variance (pixels²) the disc adds along each axis of a splat's 2D covariance.
    pub fn variance(&self, fy: f32, height: f32, z: f32) -> f32 {
        let r = 0.5 * self.coc(fy, height, z);
        r * r / 4.0
    }

    /// A 2D covariance `(a, b, c)` (pixels², `[[a, b], [b, c]]`) and opacity blurred by the disc for depth `z`: wider,
    /// fainter, its weight kept (as project.wgsl does).
    pub fn blur(&self, fy: f32, height: f32, z: f32, cov: [f32; 3], opacity: f32) -> ([f32; 3], f32) {
        let v = self.variance(fy, height, z);
        if v <= 0.0 {
            return (cov, opacity);
        }
        let [a, b, c] = cov;
        let before = a * c - b * b;
        let (a2, c2) = (a + v, c + v);
        let after = a2 * c2 - b * b;
        ([a2, b, c2], opacity * (before / after).max(0.0).sqrt())
    }
}

/// The bin of a depth (metres), as project.wgsl bins it.
pub fn bin_of(z: f32) -> usize {
    let t = (z.max(1e-6).ln() - NEAREST.ln()) / (FARTHEST.ln() - NEAREST.ln());
    ((t * BINS as f32).floor().max(0.0) as usize).min(BINS - 1)
}

/// The depth at `t` bins from the nearest (fractions between).
fn depth_at(t: f32) -> f32 {
    (NEAREST.ln() + t / BINS as f32 * (FARTHEST.ln() - NEAREST.ln())).exp()
}

/// The focus distance of a probe's histogram: the depth at which what is under the pixel, front to back, has let
/// through half of the light (optical depth ln 2), between the bin's ends by how far into it that falls; [`FAR_FOCUS`]
/// when it never does.
pub fn focus_of(bins: &[u32]) -> f32 {
    let half = std::f32::consts::LN_2;
    let mut seen = 0.0f32;
    for (i, &b) in bins.iter().enumerate() {
        let od = b as f32 / SCALE;
        if od > 0.0 && seen + od >= half {
            let into = ((half - seen) / od).clamp(0.0, 1.0);
            return depth_at(i as f32 + into);
        }
        seen += od;
    }
    FAR_FOCUS
}

/// The optical depth a splat of blending weight `alpha` adds to its bin, in the histogram's fixed point.
pub fn optical_depth(alpha: f32) -> u32 {
    (-(1.0 - alpha.clamp(0.0, 0.99)).ln() * SCALE).round() as u32
}

/// Eases the focus from `from` towards `to` over `dt` seconds (in the log of the distance, so near and far ease alike;
/// about 0.12 s to get most of the way).
pub fn ease(from: f32, to: f32, dt: f32) -> f32 {
    if !(from > 0.0) || !from.is_finite() {
        return to;
    }
    let k = 1.0 - (-dt.max(0.0) / 0.12).exp();
    (from.ln() + (to.ln() - from.ln()) * k).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dof(strength: f32, focus: f32) -> DepthOfField {
        DepthOfField { aperture: strength * APERTURE_MAX, focus, near: 1.0, far: 1.0, show_plane: false }
    }

    #[test]
    fn the_focus_is_sharp_and_the_disc_grows_away_from_it() {
        let (fy, h) = (900.0, 1080.0);
        let d = dof(0.5, 3.0);
        assert_eq!(d.coc(fy, h, 3.0), 0.0, "in focus");
        // Growing with defocus on both sides, never shrinking.
        let mut last = 0.0;
        for z in [3.05, 3.3, 4.0, 6.0, 12.0, 50.0, 400.0] {
            let c = d.coc(fy, h, z);
            assert!(c > last, "behind at {z}: {c} after {last}");
            last = c;
        }
        // Far behind, towards fy · A / focus.
        assert!((d.coc(fy, h, 1e6) - fy * d.aperture / 3.0).abs() < 0.01);
        let mut last = 0.0;
        for z in [2.95, 2.5, 2.0, 1.2, 0.8] {
            let c = d.coc(fy, h, z);
            assert!(c > last, "in front at {z}: {c} after {last}");
            last = c;
        }
        // The thin lens: 1 m in front of a focus at 3 m, fy · A · (1/2 − 1/3).
        assert!((d.coc(fy, h, 2.0) - fy * d.aperture * (0.5 - 1.0 / 3.0)).abs() < 1e-3);
        // Capped to a share of the height up close.
        assert!((d.coc(fy, h, 0.06) - MAX_COC * h).abs() < 1e-3);
        // A wider aperture blurs more.
        assert!(dof(1.0, 3.0).coc(fy, h, 10.0) > d.coc(fy, h, 10.0));
    }

    #[test]
    fn strength_zero_or_no_side_changes_nothing() {
        let cov = [4.0, 0.5, 2.0];
        for z in [0.3, 3.0, 30.0] {
            assert_eq!(dof(0.0, 3.0).blur(900.0, 1080.0, z, cov, 0.8), (cov, 0.8));
            assert!(!dof(0.0, 3.0).on());
        }
        let only_far = DepthOfField { near: 0.0, ..dof(0.5, 3.0) };
        assert_eq!(only_far.coc(900.0, 1080.0, 1.0), 0.0, "the foreground left sharp");
        assert!(only_far.coc(900.0, 1080.0, 10.0) > 0.0);
        let only_near = DepthOfField { far: 0.0, ..dof(0.5, 3.0) };
        assert_eq!(only_near.coc(900.0, 1080.0, 10.0), 0.0, "the background left sharp");
        assert!(only_near.coc(900.0, 1080.0, 1.0) > 0.0);
    }

    #[test]
    fn a_blurred_splat_is_wider_fainter_and_keeps_its_weight() {
        let d = dof(0.6, 2.0);
        let cov = [3.0, 1.0, 2.0];
        let (out, op) = d.blur(900.0, 1080.0, 20.0, cov, 0.9);
        assert!(out[0] > cov[0] && out[2] > cov[2] && out[1] == cov[1]);
        assert!(op < 0.9);
        // A 2D Gaussian's weight is its peak times 2π√det: the same before and after.
        let weight = |c: [f32; 3], o: f32| o * (c[0] * c[2] - c[1] * c[1]).sqrt();
        assert!((weight(out, op) - weight(cov, 0.9)).abs() < 1e-4 * weight(cov, 0.9));
        // In focus, as it was.
        assert_eq!(d.blur(900.0, 1080.0, 2.0, cov, 0.9), (cov, 0.9));
    }

    #[test]
    fn the_focus_is_where_half_the_light_is_hidden() {
        let mut bins = vec![0u32; BINS];
        // A thin veil at 1 m (lets 90 % through), a surface at 4 m (hides 80 %), more behind at 30 m.
        bins[bin_of(1.0)] += optical_depth(0.1);
        bins[bin_of(4.0)] += optical_depth(0.8);
        bins[bin_of(30.0)] += optical_depth(0.99);
        let f = focus_of(&bins);
        let step = (FARTHEST.ln() - NEAREST.ln()) / BINS as f32;
        assert!((f.ln() - 4f32.ln()).abs() <= step * 1.01, "{f}");
        // Without the surface, past the veil: the far wall.
        bins[bin_of(4.0)] = 0;
        assert!((focus_of(&bins).ln() - 30f32.ln()).abs() <= step * 1.01);
        // Nothing that hides half: far away (the sky).
        let mut thin = vec![0u32; BINS];
        thin[bin_of(2.0)] = optical_depth(0.3);
        assert_eq!(focus_of(&thin), FAR_FOCUS);
        assert_eq!(focus_of(&vec![0; BINS]), FAR_FOCUS);
        // Several faint splats at one depth add up to a surface.
        let mut many = vec![0u32; BINS];
        for _ in 0..4 {
            many[bin_of(7.0)] += optical_depth(0.25);
        }
        assert!((focus_of(&many).ln() - 7f32.ln()).abs() <= step * 1.01);
    }

    #[test]
    fn bins_cover_their_range_in_order() {
        assert_eq!(bin_of(0.001), 0);
        assert_eq!(bin_of(1e6), BINS - 1);
        assert!(bin_of(1.0) < bin_of(1.1) && bin_of(10.0) < bin_of(11.0));
        for z in [0.1f32, 1.0, 9.0, 120.0] {
            let i = bin_of(z) as f32;
            assert!(depth_at(i) <= z * 1.0001 && depth_at(i + 1.0) >= z * 0.9999);
        }
    }

    #[test]
    fn easing_heads_for_the_new_focus_without_overshooting() {
        let mut f = 2.0;
        let mut last = f;
        for _ in 0..30 {
            f = ease(f, 10.0, 1.0 / 60.0);
            assert!(f > last && f <= 10.0);
            last = f;
        }
        assert!(f > 9.0, "{f}");
        assert_eq!(ease(0.0, 5.0, 0.016), 5.0, "nothing to ease from");
    }
}
