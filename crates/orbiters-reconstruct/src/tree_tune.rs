//! VRChat's files of a reconstruction (OrbitersContracts Docs/splat-tree.md): its splat tree for PC, a smaller one for
//! Quest and the top of that one, which a world shows while the full file downloads. Each depth of merged boxes is
//! tuned on the GPU to look like the leaves it stands in for: from the session's recorded cameras, at the resolution
//! where that depth is drawn instead of them (the leaves' render there is the target, so a session whose keyframes
//! are gone tunes all the same).

use crate::camera::Camera;
use crate::gaussians::Gaussians;
use crate::gpu::{Gpu, LearningRates, Target};
use crate::timing::PrepStage;
use crate::tree::{self, Tree};
use orbiters_core::contracts::frame_record::record_flags;
use std::path::Path;

/// Splats in the PC file at most (32 MB).
pub const PC_SPLATS: usize = 1 << 20;
/// In the Quest file at most (16 MB): what a Quest decodes and sorts smoothly.
pub const QUEST_SPLATS: usize = 1 << 19;
/// In the coarse file at most (1.5 MB).
pub const COARSE_SPLATS: usize = 49_152;
/// Training steps per depth of merged boxes.
const STEPS: u32 = 400;
/// Cameras a depth trains from at most (spread over the session).
const CAMERAS: usize = 24;
/// The widest camera a depth trains from.
const MAX_WIDTH: u32 = 960;
/// A merged box is drawn once its error looks about this many pixels wide: what it is tuned at.
const DRAWN_AT_PIXELS: f32 = 1.5;

pub struct VrchatFiles {
    pub pc: Vec<u8>,
    pub quest: Vec<u8>,
    pub coarse: Vec<u8>,
    /// Splats in each.
    pub splats: [usize; 3],
}

/// The session's recorded cameras (one a keyframe), at most [`MAX_WIDTH`] wide.
pub fn session_cameras(session: &Path) -> anyhow::Result<Vec<Camera>> {
    let records = orbiters_core::bundle::read_records(session)?;
    Ok(records
        .iter()
        .filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0 && r.width > 0 && r.height > 0)
        .map(|r| Camera::from_strip(&r.camera_to_world, &r.projection, r.width, r.height, (MAX_WIDTH as f32 / r.width as f32).min(1.0)))
        .filter(|c| c.fx.is_finite() && c.fx > 0.0)
        .collect())
}

/// The largest tree of `g`'s most important splats within `max` splats.
fn fitting(g: &Gaussians, max: usize) -> Tree {
    let mut leaves = max * 3 / 4;
    loop {
        let t = tree::build(g, &tree::leaves(g, leaves));
        if t.len() <= max || leaves < 1000 {
            return t;
        }
        leaves = leaves * 9 / 10;
    }
}

/// The three files of `g` (a reconstruction), named `key` (the upload they belong to). Without a GPU, the merged
/// boxes stay as merged.
pub fn vrchat_files(g: &Gaussians, cameras: &[Camera], gpu: Option<&mut Gpu>, key: u32, mut progress: impl FnMut(f32)) -> anyhow::Result<VrchatFiles> {
    let mut pc = {
        let _time = PrepStage::new("tree build pc");
        fitting(g, PC_SPLATS)
    };
    anyhow::ensure!(pc.leaf_splats().len() >= 1000, "This reconstruction has too few splats to show the world.");
    let same = pc.len() <= QUEST_SPLATS;
    let mut quest = if same {
        None
    } else {
        let _time = PrepStage::new("tree build quest");
        Some(fitting(g, QUEST_SPLATS))
    };
    progress(0.1);
    if let Some(gpu) = gpu {
        let share = if same { 0.85 } else { 0.55 };
        let _time = PrepStage::new("tune pc");
        tune_for(&mut pc, cameras, gpu, "pc", |p| progress(0.1 + share * p))?;
        drop(_time);
        if let Some(q) = quest.as_mut() {
            let _time = PrepStage::new("tune quest");
            tune_for(q, cameras, gpu, "quest", |p| progress(0.65 + 0.3 * p))?;
        }
    }
    let _time = PrepStage::new("tree serialize");
    let quest = quest.unwrap_or_else(|| pc.clone());
    let coarse = quest.top(COARSE_SPLATS);
    progress(1.0);
    Ok(VrchatFiles {
        splats: [pc.len(), quest.len(), coarse.len()],
        pc: pc.file(key, 0),
        quest: quest.file(key, 0),
        coarse: coarse.file(key, tree::FLAG_COARSE),
    })
}

/// A render as a keyframe's packed pixels (RGBA8, red in the low byte).
fn packed(render: &[[f32; 4]]) -> Vec<u32> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    render.iter().map(|p| byte(p[0]) | byte(p[1]) << 8 | byte(p[2]) << 16 | 255 << 24).collect()
}

/// Tunes every depth of `tree`'s merged boxes against its leaves.
pub fn tune(tree: &mut Tree, cameras: &[Camera], gpu: &mut Gpu, mut progress: impl FnMut(f32)) -> anyhow::Result<()> {
    tune_for(tree, cameras, gpu, "tree", &mut progress)
}

fn tune_for(tree: &mut Tree, cameras: &[Camera], gpu: &mut Gpu, platform: &str, mut progress: impl FnMut(f32)) -> anyhow::Result<()> {
    if cameras.is_empty() {
        return Ok(());
    }
    let poses: Vec<([f32; 3], [f32; 3])> = cameras.iter().map(|c| (c.position(), c.forward())).collect();
    let cameras: Vec<Camera> = crate::dataset::spread(&poses, CAMERAS).into_iter().map(|i| cameras[i]).collect();
    let centre = cameras.iter().fold([0.0f32; 3], |a, c| [0, 1, 2].map(|k| a[k] + c.position()[k] / cameras.len() as f32));
    let extent = cameras.iter().map(|c| (0..3).map(|k| (c.position()[k] - centre[k]).powi(2)).sum::<f32>().sqrt()).fold(1.0f32, f32::max);
    let lr = LearningRates { means: 1.6e-5 * extent, log_scales: 0.005, rotations: 0.001, opacity: 0.025, colors: 0.0025 };
    let leaves = tree.leaf_splats();
    let mut depths: Vec<u32> = tree.nodes.iter().filter(|n| !n.leaf).map(|n| n.depth).collect();
    depths.sort_unstable();
    depths.dedup();
    let mut rng = fastrand::Rng::with_seed(1);
    for (done, &depth) in depths.iter().enumerate() {
        let _depth_time = PrepStage::new(format!("tune {platform} depth {depth}"));
        let gpu_start = gpu.profiler.as_ref().map(|p| p.span.get().0);
        let tuned: Vec<usize> = (0..tree.nodes.len()).filter(|&i| !tree.nodes[i].leaf && tree.nodes[i].depth == depth).collect();
        // Each camera at the resolution where this depth's boxes are drawn: their median error about DRAWN_AT_PIXELS.
        let views: Vec<Camera> = cameras
            .iter()
            .filter_map(|c| {
                let mut seen: Vec<f32> = tuned
                    .iter()
                    .filter_map(|&i| {
                        let n = &tree.nodes[i];
                        let p = c.to_camera(n.center);
                        (p[2] > 0.1).then(|| n.error * c.fx / p[2])
                    })
                    .collect();
                if seen.is_empty() {
                    return None;
                }
                let k = seen.len() / 2;
                let median = *seen.select_nth_unstable_by(k, f32::total_cmp).1;
                let factor = ((median / DRAWN_AT_PIXELS).ceil() as u32).clamp(1, (c.width / 48).max(1));
                Some(c.scaled(factor))
            })
            .collect();
        if views.is_empty() {
            continue;
        }
        let _target_time = PrepStage::new(format!("{platform} depth {depth} targets"));
        gpu.upload(&leaves);
        let targets: Vec<Vec<u32>> = views.iter().map(|v| Ok(packed(&crate::train::render(gpu, v, [0.0; 3])?))).collect::<anyhow::Result<_>>()?;
        drop(_target_time);
        let cut = tree.select(|n| n.depth == depth || (n.leaf && n.depth < depth));
        // Where each tuned box's splats sit in the cut (it keeps the tree's box order).
        let mut at = Vec::with_capacity(tuned.len());
        let mut offset = 0usize;
        for n in tree.nodes.iter().filter(|n| n.depth == depth || (n.leaf && n.depth < depth)) {
            if !n.leaf && n.depth == depth {
                at.push((offset, n.start as usize, n.count as usize));
            }
            offset += n.count as usize;
        }
        gpu.upload(&cut);
        gpu.step = 0;
        for v in &views {
            gpu.set_mask(v.width, v.height, &vec![1.0; (v.width * v.height) as usize])?;
        }
        let mean_score = |gpu: &mut Gpu| -> anyhow::Result<f32> {
            Ok(views.iter().zip(&targets).map(|(v, t)| gpu.score(v, [0.0; 3], t)).collect::<anyhow::Result<Vec<f32>>>()?.iter().sum::<f32>()
                / views.len() as f32)
        };
        let before = mean_score(gpu)?;
        let _step_time = PrepStage::new(format!("{platform} depth {depth} steps"));
        for _ in 0..STEPS {
            let j = rng.usize(..views.len());
            let v = &views[j];
            let target = Target::Color { rgba: &targets[j], scene_pixels: (v.width * v.height) as f32, weight: 1.0, ssim: 0.2, robust: 0.0 };
            gpu.step(v, [0.0; 3], target, Some(lr))?;
        }
        let after = mean_score(gpu)?;
        drop(_step_time);
        eprintln!("tree depth {depth}: {} boxes, PSNR against the leaves {before:.6} → {after:.6} dB", tuned.len());
        if std::env::var_os("ORBITERS_PREP_TIMING").is_some() {
            eprintln!(
                "upload_quality {}",
                serde_json::json!({"platform":platform,"depth":depth,"boxes":tuned.len(),"before_db":before,"after_db":after,"retained_db":before.max(after),"views":views.len(),"gpu_ms":gpu_start.zip(gpu.profiler.as_ref()).map(|(start,p)|p.span.get().0-start)})
            );
        }
        // Boxes that already looked like their leaves can only lose to the noise of a few hundred steps.
        if after <= before {
            progress((done + 1) as f32 / depths.len() as f32);
            continue;
        }
        let trained = gpu.download_params()?;
        for (from, to, count) in at {
            for k in 0..count {
                if trained.is_finite(from + k) {
                    tree.splats.means[to + k] = trained.means[from + k];
                    tree.splats.log_scales[to + k] = trained.log_scales[from + k];
                    tree.splats.rotations[to + k] = trained.rotations[from + k];
                    tree.splats.opacity_logits[to + k] = trained.opacity_logits[from + k];
                    tree.splats.colors[to + k] = trained.colors[from + k];
                }
            }
        }
        progress((done + 1) as f32 / depths.len() as f32);
    }
    Ok(())
}
