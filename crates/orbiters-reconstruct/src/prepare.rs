//! A session as the worker, `eval` and `kbench` all train or score on it: loaded within the memory budget
//! (crate::dataset), its untrusted depth marked unknown (crate::depthfix) and the world's chromatic aberration undone
//! in its keyframes (crate::aberration), so the lab's numbers are the app's.

use crate::aberration::{self, Undo};
use crate::dataset::{self, Budget, Dataset};
use crate::depthfix::{self, FixParams, FixStats};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Options {
    /// Training width at most (an integer fraction of the captured width).
    pub width: u32,
    pub budget: Budget,
    /// Depth validation (crate::depthfix).
    pub validate: bool,
    /// Undo the world's chromatic aberration when it has any (crate::aberration).
    pub aberration: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { width: 1920, budget: Budget::default(), validate: true, aberration: true }
    }
}

pub struct Prepared {
    pub data: Dataset,
    /// What depth validation found (`None`: not run).
    pub depth: Option<FixStats>,
    /// The aberration undone, if any: scoring passes renders through it (crate::train::score_held_out).
    pub undo: Option<Undo>,
}

/// Loads and prepares a session; `progress` (0..1) follows the loading.
pub fn prepare(dir: &Path, options: &Options, progress: impl FnMut(f32)) -> anyhow::Result<Prepared> {
    let mut data = dataset::load_with(dir, options.width, options.budget, progress)?;
    let depth = options.validate.then(|| crate::priority::on_loading_threads(|| depthfix::mark_untrusted(&mut data, &FixParams::default())));
    let undo = if options.aberration { aberration::undo(&mut data) } else { None };
    Ok(Prepared { data, depth, undo })
}

impl Prepared {
    /// Lines for the log: what loading kept and left out, what validation found, what was undone.
    pub fn describe(&self) -> Vec<String> {
        let l = &self.data.loaded;
        let mut out = vec![format!(
            "{} of {} keyframes at {}×{} ({} over the memory budget, {} unreadable or of another size), {} of {} depth frames ({} over the budget, {} unreadable)",
            self.data.views.len(),
            l.keyframes,
            self.data.width,
            self.data.height,
            l.keyframes_over_budget,
            l.keyframes_skipped,
            self.data.depth_views.len(),
            l.depth_frames,
            l.depth_frames_over_budget,
            l.depth_frames_skipped
        )];
        if let Some(p) = &l.first_problem {
            out.push(format!("first unreadable keyframe: {p}"));
        }
        if let Some(f) = &self.depth {
            out.push(format!(
                "depth validation: {} of {} samples unknown ({} marked by the decoder); \"never reads\" rule {}: {} of {} tile positions never read ({} of their samples kept as sky); {} misread tiles, {} impossibly near samples",
                f.unknown_samples,
                f.samples,
                f.decoder_unknown,
                if f.never_read_rule { "on" } else { "off (this session's depth files mark what was not read)" },
                f.dead_tiles,
                f.tiles,
                f.sky_samples,
                f.misread_tiles,
                f.near_samples
            ));
        }
        if let Some(u) = &self.undo {
            out.push(format!("chromatic aberration (intensity {:.3}) undone in {} keyframes", u.intensity, self.data.views.len()));
        }
        out
    }

    /// Why a session gives no Gaussians to start from: no depth frames at all, or none that could be read.
    pub fn nothing_to_seed(&self) -> String {
        let d = &self.data;
        // Samples the frames carry that are not unknown.
        let known: usize = d.depth_views.iter().map(|v| (0..v.depth.len()).filter(|&i| !v.is_unknown(i)).count()).sum();
        if d.loaded.depth_frames == 0 {
            "This session has no depth frames to start from: record with the capture kit's depth frames on.".into()
        } else if d.depth_views.is_empty() {
            format!("None of this session's {} depth frames could be read (missing or damaged files).", d.loaded.depth_frames)
        } else if known == 0 {
            format!(
                "Every depth sample of this session's {} depth frames is unknown: the world's post-processing (chromatic aberration, bloom) breaks the depth symbols. Turn it down in the world, or capture in another one.",
                d.depth_views.len()
            )
        } else {
            format!("This session's {} depth frames give nothing to start from (no sample over the scene).", d.depth_views.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::fixture;
    use orbiters_contracts::depth::{DepthLayout, UNKNOWN};

    fn options() -> Options {
        Options { width: 640, budget: Budget::UNLIMITED, ..Default::default() }
    }

    /// A session whose every depth sample the decoder could not read says so (it used to be told to record depth
    /// frames, which it had); a session without depth frames is told that.
    #[test]
    fn a_session_with_nothing_to_seed_says_why() {
        let (w, h) = (640u32, 360u32);
        let (gw, gh) = DepthLayout::new(w, h).expect("layout").grid_size();
        let s = fixture::session("all-unknown", w, h, 3, &vec![Some(vec![UNKNOWN; (gw * gh) as usize]); 25]);
        let p = prepare(&s.dir, &options(), |_| {}).expect("prepare");
        assert_eq!(p.data.depth_views.len(), 25);
        assert!(p.depth.as_ref().is_some_and(|f| !f.never_read_rule && f.unknown_samples == f.samples));
        assert!(crate::init::from_depth(&p.data, 1000, &[], |_| {}).is_empty());
        assert!(p.nothing_to_seed().contains("Every depth sample"), "{}", p.nothing_to_seed());
        let none = fixture::session("no-depth", w, h, 3, &[]);
        let p = prepare(&none.dir, &options(), |_| {}).expect("prepare");
        assert!(p.nothing_to_seed().contains("no depth frames"), "{}", p.nothing_to_seed());
        assert!(p.undo.is_none(), "random colours show no aberration");
        assert!(p.describe()[0].starts_with("3 of 3 keyframes at 640×360"), "{:?}", p.describe());
    }
}
