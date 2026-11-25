//! A session as training data: its keyframes (lossless, upright) at the training resolution with their exact
//! cameras, its depth frames' exact samples with their cameras, and the mask of what is not scene (the capture
//! strip and, with a capture kit, the skeleton band). A long session keeps to a memory budget ([`Budget`]): past it,
//! the keyframes and depth frames that look at the world most differently stay ([`spread`]). A capture whose avatar
//! carried the camera rig also has its faces ([`Faces`]): square colour views around the camera's path, with no depth.

use crate::camera::{Camera, Geometry};
use anyhow::{anyhow, bail, ensure, Context as _};
use orbiters_contracts::depth::{self, DepthLayout, SAMPLES_X};
use orbiters_contracts::frame_record::{record_flags, FrameRecord};
use orbiters_contracts::manifest::{depth_file, DEPTH_DIR, KEYFRAMES_DIR};
use orbiters_contracts::image::Rect;
use orbiters_contracts::{qoi, rig, skeleton, strip};
use orbiters_core::bundle;
use orbiters_i18n::{tr, tr_n};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A keyframe: RGBA8 packed in a u32 (r in the low byte, encoded sRGB as captured, alpha unused), row-major, top row
/// first: the layout the GPU loss reads.
pub struct View {
    pub camera: Camera,
    pub image: Vec<u32>,
    pub game_time: f32,
    pub keyframe: i32,
}

/// A set of sample indices, a bit each (empty: none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks(Vec<u64>);

impl Marks {
    pub fn contains(&self, i: usize) -> bool {
        self.0.get(i / 64).is_some_and(|w| (w >> (i % 64)) & 1 == 1)
    }

    pub fn insert(&mut self, i: usize) {
        let w = i / 64;
        if self.0.len() <= w {
            self.0.resize(w + 1, 0);
        }
        self.0[w] |= 1 << (i % 64);
    }

    pub fn len(&self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }
}

/// Where the samples of depth frames of one size sit: the same for every one of them, so they share it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DepthGrid {
    pub width: u32,
    pub height: u32,
    /// Each sample's pixel position at the training resolution (pixel centres at integers), row-major.
    pub pixels: Vec<[f32; 2]>,
    /// Sample spacing in training pixels (for footprints), in a grid tile.
    pub spacing: f32,
    /// Grid cells that carry no sample: tiles that touch the capture strip's area, and a larger tile's other cells
    /// towards the frame's edges. Depth frames carry nothing there, and their code 0 there does not mean "nothing there".
    pub unused: Marks,
    /// Per sample, that spacing's multiple between it and the next samples of its tile: 1 in a grid tile, 2, 4 or 8 in
    /// the larger tiles towards the frame's edges (orbiters_contracts::depth::DepthLayout::grid_steps), 0 where nothing
    /// is carried. Empty: 1 everywhere.
    pub steps: Vec<u8>,
}

impl DepthGrid {
    /// The sample grid of depth frames laid out as `layout` (the captured size of `geometry`).
    pub fn new(layout: &DepthLayout, geometry: &Geometry) -> Self {
        let (gw, gh) = layout.grid_size();
        let mut pixels = Vec::with_capacity((gw * gh) as usize);
        let mut unused = Marks::default();
        let steps = layout.grid_steps();
        for gy in 0..gh {
            for gx in 0..gw {
                let (px, py) = layout.grid_pixel(gx, gy);
                pixels.push([geometry.to_training(px as f32), geometry.to_training(py as f32)]);
                if steps[(gy * gw + gx) as usize] == 0 {
                    unused.insert((gy * gw + gx) as usize);
                }
            }
        }
        Self { width: gw, height: gh, pixels, spacing: layout.tile_px as f32 / SAMPLES_X as f32 / geometry.factor as f32, unused, steps }
    }

    /// Grid cells between sample `i` and the next samples of its tile (1 in a grid tile).
    pub fn step(&self, i: usize) -> u32 {
        self.steps.get(i).map_or(1, |&s| s.max(1) as u32)
    }

    pub fn len(&self) -> usize {
        self.pixels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }
}

/// A depth frame's samples: eye depths on its [`DepthGrid`] (0: nothing there, or unknown).
pub struct DepthView {
    pub camera: Camera,
    pub grid: Arc<DepthGrid>,
    pub depth: Vec<f32>,
    pub game_time: f32,
    /// Samples that say nothing: tiles the frame never carries ([`DepthGrid::unused`]), tiles the decoder could not
    /// read (orbiters_contracts::depth::UNKNOWN), and samples depth validation distrusts (crate::depthfix). Neither
    /// seeds nor depth targets.
    pub unknown: Marks,
    /// Samples the capture's decoder itself marked unreadable: a session that has any tells failed tiles from empty
    /// sky by itself (crate::depthfix).
    pub decoder_unknown: usize,
    /// Whether code 0 in this frame's file may mean "not read" as well as "nothing there" (`.d16` version 1; version 2
    /// marks every sample nobody read unknown).
    pub zero_may_be_unread: bool,
}

impl DepthView {
    /// Samples that hit something and are trusted.
    pub fn valid(&self) -> usize {
        (0..self.depth.len()).filter(|&i| self.depth[i] > 0.0 && !self.unknown.contains(i)).count()
    }

    pub fn is_unknown(&self, i: usize) -> bool {
        self.unknown.contains(i)
    }
}

/// The capture rig's faces (contracts' Docs/rig-frames.md): colour views around the camera's path at the keyframes'
/// training scale, each with its own camera. No depth comes with them. A rig frame's layout may give its faces more than
/// one size, and a session from an older upload another size again: each record says its own.
pub struct Faces {
    pub views: Vec<View>,
    /// The sizes among `views`, and what is scene in each.
    pub sizes: Vec<FaceSize>,
}

/// One size of a session's rig faces.
pub struct FaceSize {
    pub geometry: Geometry,
    pub width: u32,
    pub height: u32,
    /// 1 where a face's pixel is scene: not its border (the world's bloom and aberration at the tile's place in the frame)
    /// and not its stamp.
    pub mask: Vec<f32>,
    pub scene_pixels: f32,
}

impl Faces {
    /// What is scene in a view of this size, `None` when no face has that size.
    pub fn size_of(&self, camera: &Camera) -> Option<&FaceSize> {
        self.sizes.iter().find(|s| (s.width, s.height) == (camera.width, camera.height))
    }
}

pub struct Dataset {
    pub dir: PathBuf,
    pub views: Vec<View>,
    pub depth_views: Vec<DepthView>,
    /// The rig's faces, when the capture has any.
    pub faces: Option<Faces>,
    pub width: u32,
    pub height: u32,
    /// How the captured frames map to the training resolution.
    pub geometry: Geometry,
    /// 1 where pixels are scene, 0 over the capture strip and the skeleton band (training resolution).
    pub mask: Vec<f32>,
    /// What the session holds and what loading left out.
    pub loaded: Loaded,
}

/// How much longer than the session's usual focal length a keyframe's must be for it to be a detail view
/// ([`detail_views`]). 3D pictures took such views for a while (none reached the capture: 2026-11-04, the desktop's
/// controller/burst.rs); what is left of them is a person zooming in during a recording, seldom this far.
pub const DETAIL_ZOOM: f32 = 1.3;

/// Which of keyframes with these focal lengths (any one unit) are detail views: those whose focal length is at least
/// [`DETAIL_ZOOM`] times the median one, when they are fewer than a third of all. Measured when 3D pictures were to start with
/// a few frames of VRChat's camera zoomed in on what it aims at, from where it stands, emulated: through
/// the same 1080p stream they show its subject — the people in the picture — with 2.35 times the pixels a degree of the
/// other keyframes, all within about a second, while the people hardly move. Measured on the real 3D pictures of
/// 2026-11-03 (Penthouse Palace), the reconstruction was mushy on the faces because the people moved during the
/// flight and only the first keyframe saw them as framed; the detail views are what the picture's subject is learnt
/// from (crate::train: never held out, and a quarter of the colour steps on top of the others'). A session most of whose keyframes are zoomed
/// in (a user who records zoomed) has none: its usual view is the zoomed one.
pub fn detail_views(focal_lengths: &[f32]) -> Vec<usize> {
    let mut sorted: Vec<f32> = focal_lengths.iter().copied().filter(|f| f.is_finite() && *f > 0.0).collect();
    if sorted.len() < 3 {
        return Vec::new();
    }
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    let detail: Vec<usize> = (0..focal_lengths.len()).filter(|&i| focal_lengths[i] >= DETAIL_ZOOM * median).collect();
    if detail.len() * 3 > focal_lengths.len() {
        return Vec::new();
    }
    detail
}

impl Dataset {
    /// The keyframes that are detail views ([`detail_views`]), as indices into [`Dataset::views`].
    pub fn detail_views(&self) -> Vec<usize> {
        detail_views(&self.views.iter().map(|v| v.camera.fy).collect::<Vec<_>>())
    }
}

/// Counts of what [`load`] found and kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    /// Keyframes and depth frames whose strip read.
    pub keyframes: usize,
    pub depth_frames: usize,
    /// Left out to keep to the [`Budget`].
    pub keyframes_over_budget: usize,
    pub depth_frames_over_budget: usize,
    /// Missing, unreadable, or of another size than the session's keyframes.
    pub keyframes_skipped: usize,
    pub depth_frames_skipped: usize,
    /// The rig's faces kept as keyframes, those left out to keep to the budget, and those unreadable or of a size no
    /// frame of the session gives.
    pub faces: usize,
    pub faces_over_budget: usize,
    pub faces_skipped: usize,
    /// How much the world's vignette darkened the rig's faces at the corners of the frame, undone as they were loaded
    /// ([`undo_faces_vignette`]), in percent of the frame's middle. 0 when it did not.
    pub faces_vignette: u32,
    /// Keyframes and rig faces left out as blown to white ([`leave_out_blown`]).
    pub keyframes_blown: usize,
    pub faces_blown: usize,
    /// Why the first skipped keyframe was skipped.
    pub first_problem: Option<String>,
}

/// What a loaded session may hold in memory: keyframes take 4 bytes a training pixel, depth samples about 4 bytes
/// each. Seeding holds more for a moment, which this does not count: its merge keeps a key and a candidate per
/// distinct merge cell (up to about 30 bytes each while its tables grow), 1 to 2 GB at 60 million samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub keyframe_bytes: u64,
    pub depth_samples: u64,
}

/// Fewest keyframes and depth frames kept whatever the budget says.
const MIN_KEPT: usize = 64;

impl Budget {
    pub const UNLIMITED: Self = Self { keyframe_bytes: u64::MAX, depth_samples: u64::MAX };

    /// A sixth of the PC's memory for keyframes, between 2 and 6 GiB (about 260 to 780 keyframes at 1080p: a long
    /// walk at 5 to 10 keyframes a second would otherwise take tens of GB), and 60 million depth samples (about
    /// 2,300 depth frames at 1080p, 250 MB).
    pub fn for_this_pc() -> Self {
        const GIB: u64 = 1 << 30;
        let keyframe_bytes = physical_memory().map_or(3 * GIB, |m| (m / 6).clamp(2 * GIB, 6 * GIB));
        Self { keyframe_bytes, depth_samples: 60_000_000 }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::for_this_pc()
    }
}

fn physical_memory() -> Option<u64> {
    #[cfg(windows)]
    {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        Some(system.total_memory()).filter(|m| *m > 0)
    }
    #[cfg(target_vendor = "apple")]
    {
        crate::apple::physical_memory()
    }
    #[cfg(not(any(windows, target_vendor = "apple")))]
    {
        None
    }
}

/// Which `k` of `poses` (camera position, forward axis) to keep: farthest-point sampling from the first, a metre of
/// travel counting about like 20° of turn, in their original order. All of them when `k` covers them.
pub fn spread(poses: &[([f32; 3], [f32; 3])], k: usize) -> Vec<usize> {
    let n = poses.len();
    if k >= n {
        return (0..n).collect();
    }
    // The distance from pose `a` to `b`, or anything at least `bound` when it is at least that: how far apart they stand says
    // so for most poses of a long walk, with no root taken (one part in a million over, past what its rounding could hide).
    let distance = |a: usize, b: usize, bound: f32| {
        let ((pa, fa), (pb, fb)) = (poses[a], poses[b]);
        let d2 = |u: [f32; 3], v: [f32; 3]| (u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2) + (u[2] - v[2]).powi(2);
        let apart = d2(pa, pb);
        if apart > bound * bound * 1.000_001 {
            return bound;
        }
        // The chord between unit forward axes is about the angle: 0.35 is 20°.
        apart.sqrt() + d2(fa, fb).sqrt() / 0.35
    };
    let mut nearest = vec![f32::INFINITY; n];
    let mut kept = vec![false; n];
    let mut out = Vec::with_capacity(k);
    let mut next = 0;
    while out.len() < k {
        kept[next] = true;
        out.push(next);
        let (mut best, mut far) = (None, f32::NEG_INFINITY);
        for i in 0..n {
            if kept[i] {
                continue;
            }
            nearest[i] = nearest[i].min(distance(next, i, nearest[i]));
            if nearest[i] > far {
                (best, far) = (Some(i), nearest[i]);
            }
        }
        let Some(b) = best else { break };
        next = b;
    }
    out.sort_unstable();
    out
}

/// The camera position and forward axis of a strip pose.
fn pose(r: &FrameRecord) -> ([f32; 3], [f32; 3]) {
    let m = &r.camera_to_world;
    ([m[3], m[7], m[11]], [m[2], m[6], m[10]])
}

/// Which `k` of `frames` to train on when the budget holds no more, in their order: spread over the poses ([`spread`]),
/// after up to a quarter of them for the detail views ([`detail_views`]: frames zoomed in on something, kept by the
/// capture for it at a pose its other frames have too). Poses alone cannot tell those from the wide frames beside them:
/// a long capture lost its magnified subject, and what was left had no detail view to find. A quarter is what training
/// gives them of its colour steps (crate::train::Settings::detail_share) and leaves them fewer than a third of what is
/// kept, which is what makes them detail views there. Without detail views, the spread alone, frame for frame.
fn within(frames: &[&FrameRecord], k: usize) -> Vec<usize> {
    let of = |ids: &[usize], k: usize| -> Vec<usize> { spread(&ids.iter().map(|&i| pose(frames[i])).collect::<Vec<_>>(), k).into_iter().map(|j| ids[j]).collect() };
    let all: Vec<usize> = (0..frames.len()).collect();
    let detail = if k < frames.len() { detail_views(&frames.iter().map(|r| r.projection[1]).collect::<Vec<_>>()) } else { Vec::new() };
    if detail.is_empty() {
        return of(&all, k);
    }
    let others: Vec<usize> = all.into_iter().filter(|i| detail.binary_search(i).is_err()).collect();
    let zoomed = (k / 4).max(k.saturating_sub(others.len())).min(detail.len());
    let mut kept = of(&detail, zoomed);
    kept.extend(of(&others, k - zoomed));
    kept.sort_unstable();
    kept
}

/// Whether a record's pose is all numbers.
fn posed(r: &FrameRecord) -> bool {
    r.camera_to_world.iter().chain(&r.projection).all(|v| v.is_finite())
}

/// Loads a session at a training width of at most `max_width` (an integer fraction of the captured width), within
/// this PC's [`Budget`].
pub fn load(dir: &Path, max_width: u32, progress: impl FnMut(f32)) -> anyhow::Result<Dataset> {
    load_with(dir, max_width, Budget::default(), progress)
}

/// [`load`] within `budget`.
pub fn load_with(dir: &Path, max_width: u32, budget: Budget, mut progress: impl FnMut(f32)) -> anyhow::Result<Dataset> {
    let manifest = bundle::read_manifest(dir).with_context(|| tr("reading the session's manifest"))?;
    let records: Vec<FrameRecord> = bundle::read_records(dir).with_context(|| tr("reading frames.bin"))?.into_iter().filter(|r| r.has(record_flags::STRIP_OK) && posed(r)).collect();
    // The rig's faces are keyframes of their own (their records say which face), of another size than the frames'.
    let face_of = |r: &FrameRecord| if strip::flags::kind(r.strip_flags) == strip::flags::KIND_RIG { strip::flags::face(r.strip_flags) } else { 0 };
    let pictures = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.keyframe_index >= 0);
    let (face_frames, keyframes): (Vec<&FrameRecord>, Vec<&FrameRecord>) = pictures.partition(|r| face_of(r) != 0);
    let depth_frames: Vec<&FrameRecord> = records.iter().filter(|r| r.has(record_flags::DEPTH) && r.depth_index >= 0).collect();
    // The session's frame size is its keyframes' most common one: after the stream's resolution changed mid-session,
    // the frames of the other size are left out.
    let size = most_common(keyframes.iter().map(|r| (r.width, r.height)));
    let sized: Vec<&FrameRecord> = keyframes.iter().copied().filter(|r| Some((r.width, r.height)) == size).collect();
    if sized.len() < 2 {
        bail!("{}", tr_n("This session has {count} keyframe: reconstruction needs a few views of the world.", "This session has {count} keyframes: reconstruction needs a few views of the world.", sized.len() as u64));
    }
    let (full_w, full_h) = size.expect("two keyframes");
    let geometry = Geometry::at_most(full_w, full_h, max_width);
    let (w, h) = (geometry.width(), geometry.height());
    ensure!(w >= 16 && h >= 16, "{}", tr!("keyframes of {width}×{height} are too small to reconstruct from", width = full_w, height = full_h));
    let mut loaded = Loaded { keyframes: keyframes.len(), depth_frames: depth_frames.len(), keyframes_skipped: keyframes.len() - sized.len(), ..Default::default() };

    // Within the budget, the keyframes that look at the world most differently, and those zoomed in.
    let keep = usize::try_from(budget.keyframe_bytes / (w as u64 * h as u64 * 4)).unwrap_or(usize::MAX).max(MIN_KEPT);
    let chosen: Vec<&FrameRecord> = within(&sized, keep).into_iter().map(|i| sized[i]).collect();
    loaded.keyframes_over_budget = sized.len() - chosen.len();
    // Depth frames of the session's size, all on one grid, within the budget too.
    let grid = DepthLayout::new(full_w, full_h).map(|l| Arc::new(DepthGrid::new(&l, &geometry)));
    let depth_sized: Vec<&FrameRecord> = depth_frames.iter().copied().filter(|r| grid.is_some() && (r.width, r.height) == (full_w, full_h)).collect();
    loaded.depth_frames_skipped = depth_frames.len() - depth_sized.len();
    let per_frame = grid.as_ref().map_or(1, |g| g.len().max(1)) as u64;
    let keep = usize::try_from(budget.depth_samples / per_frame).unwrap_or(usize::MAX).max(MIN_KEPT);
    let depth_chosen: Vec<&FrameRecord> = within(&depth_sized, keep).into_iter().map(|i| depth_sized[i]).collect();
    loaded.depth_frames_over_budget = depth_sized.len() - depth_chosen.len();
    // The rig's faces: each record gives its face's size and projection (contracts' rig::RigLayout, of whichever layout
    // the avatar's upload draws), at the frames' scale, within what the budget leaves.
    let face_geometry = |r: &FrameRecord| Geometry::new(r.width, r.height, geometry.factor);
    let face_sized: Vec<&FrameRecord> = face_frames.iter().copied().filter(|r| face_geometry(r).width() >= 16 && face_geometry(r).height() >= 16).collect();
    loaded.faces_skipped = face_frames.len() - face_sized.len();
    let face_chosen: Vec<&FrameRecord> = if face_sized.is_empty() {
        Vec::new()
    } else {
        let bytes: u64 = face_sized.iter().map(|r| face_geometry(r).width() as u64 * face_geometry(r).height() as u64 * 4).sum::<u64>() / face_sized.len() as u64;
        let left = budget.keyframe_bytes.saturating_sub(chosen.len() as u64 * w as u64 * h as u64 * 4);
        let keep = usize::try_from(left / bytes.max(1)).unwrap_or(usize::MAX).max(MIN_KEPT);
        within(&face_sized, keep).into_iter().map(|i| face_sized[i]).collect()
    };
    loaded.faces_over_budget = face_sized.len() - face_chosen.len();

    // Decoded on as many threads as loading may use (crate::priority); a frame that cannot be read is left out.
    enum Item {
        View(anyhow::Result<View>),
        Depth(Option<DepthView>),
        Face(anyhow::Result<View>),
    }
    let decode = |j: usize| {
        if j < chosen.len() {
            Item::View(load_keyframe(dir, chosen[j], &geometry))
        } else if j < chosen.len() + depth_chosen.len() {
            Item::Depth(grid.as_ref().and_then(|g| load_depth(dir, depth_chosen[j - chosen.len()], &geometry, g)))
        } else {
            let r = face_chosen[j - chosen.len() - depth_chosen.len()];
            Item::Face(load_keyframe(dir, r, &face_geometry(r)))
        }
    };
    let mut views = Vec::with_capacity(chosen.len());
    let mut depth_views = Vec::with_capacity(depth_chosen.len());
    let mut face_views = Vec::with_capacity(face_chosen.len());
    for item in run_parallel(chosen.len() + depth_chosen.len() + face_chosen.len(), decode, &mut progress) {
        match item {
            Item::View(Ok(v)) => views.push(v),
            Item::View(Err(e)) => {
                loaded.keyframes_skipped += 1;
                loaded.first_problem.get_or_insert_with(|| format!("{e:#}"));
            }
            Item::Depth(Some(d)) => depth_views.push(d),
            Item::Depth(None) => loaded.depth_frames_skipped += 1,
            Item::Face(Ok(v)) => face_views.push(v),
            Item::Face(Err(_)) => loaded.faces_skipped += 1,
        }
    }
    loaded.keyframes_blown = leave_out_blown(&mut views);
    loaded.faces_blown = leave_out_blown(&mut face_views);
    loaded.faces_vignette = (undo_faces_vignette(&mut face_views, &records, &geometry) * 100.0).round() as u32;
    face_views.sort_by_key(|v| v.keyframe);
    loaded.faces = face_views.len();
    let faces = (!face_views.is_empty()).then(|| {
        let mut sizes: Vec<FaceSize> = Vec::new();
        for r in &face_chosen {
            let g = face_geometry(r);
            if !sizes.iter().any(|s| (s.width, s.height) == (g.width(), g.height())) {
                let mask = face_mask(&g);
                let scene_pixels = mask.iter().filter(|m| **m > 0.5).count() as f32;
                sizes.push(FaceSize { geometry: g, width: g.width(), height: g.height(), mask, scene_pixels });
            }
        }
        Faces { views: face_views, sizes }
    });
    if views.len() < 2 {
        let (read, total) = (views.len(), loaded.keyframes);
        match loaded.first_problem.as_deref() {
            Some(why) => bail!("{}", tr!("Only {read} of this session's {total} keyframes could be read ({why}): reconstruction needs a few views of the world.", read = read, total = total, why = why)),
            None => bail!("{}", tr!("Only {read} of this session's {total} keyframes could be read: reconstruction needs a few views of the world.", read = read, total = total)),
        }
    }
    views.sort_by_key(|v| v.keyframe);

    // What is not scene: the strip (and patches), and the skeleton band when the session read one, came from the
    // capture kit on an avatar (it draws the band into every colour frame), or the keyframes show it. A world whose
    // post-processing breaks the band's symbols (chromatic aberration at the bottom of the frame) decodes no skeleton
    // frame, and an unmasked band trains its codes as bright specks on the floor ahead of every camera.
    let kit = records.iter().any(|r| strip::flags::source(r.strip_flags) == strip::flags::SOURCE_AVATAR);
    let mut mask = vec![1.0f32; (w * h) as usize];
    let mut cut = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let (x0, y0, x1, y1) = geometry.cover(x0, y0, x1, y1);
        for y in y0..y1 {
            for x in x0..x1 {
                mask[(y * w + x) as usize] = 0.0;
            }
        }
    };
    if let Some(l) = strip::StripLayout::new(full_w, full_h) {
        cut(l.reserved.x0, l.reserved.y0, l.reserved.x1, l.reserved.y1);
    }
    if manifest.performance.skeleton_frames > 0 || kit || band_drawn(&views, &geometry) {
        if let Some(b) = skeleton::BandLayout::new(full_w, full_h) {
            let r = b.reserved();
            cut(r.x0, r.y0, r.x1, r.y1);
        }
    }
    Ok(Dataset { dir: dir.to_path_buf(), views, depth_views, faces, width: w, height: h, geometry, mask, loaded })
}

/// The share of a keyframe's pixels at white from which it counts as blown out.
const BLOWN_SHARE: f32 = 0.25;
/// A session whose typical keyframe is this white already keeps them all: a snowfield or a white room is the world.
const BLOWN_TYPICAL: f32 = 0.05;

/// The share of an image's pixels at white.
fn blown_share(image: &[u32]) -> f32 {
    let white = image.iter().filter(|&&p| (p & 0xff) >= 250 && ((p >> 8) & 0xff) >= 250 && ((p >> 16) & 0xff) >= 250).count();
    white as f32 / image.len().max(1) as f32
}

/// Leaves out the views mostly blown to white when the session's others are not, the worst first and at most one view
/// in ten (at least one), and says how many left. A camera inside or just behind the world's geometry sees that (a
/// translucent wall or a haze at its near plane), and no scene explains it from elsewhere: training pays for it with
/// bright fog in front of that camera, seen from everywhere else. One such frame out of 37 turned the wearer into a
/// white ghost from behind in ちま部屋 2026-11-04 12-14-49, and the views it spoils lie off the camera's path, so scores on
/// held-out frames hardly move.
fn leave_out_blown(views: &mut Vec<View>) -> usize {
    if views.len() < 3 {
        return 0;
    }
    let mut shares: Vec<(f32, i32)> = views.iter().map(|v| (blown_share(&v.image), v.keyframe)).collect();
    shares.sort_by(|a, b| b.0.total_cmp(&a.0));
    if shares[shares.len() / 2].0 >= BLOWN_TYPICAL {
        return 0;
    }
    let out: Vec<i32> = shares.iter().take((views.len() / 10).max(1)).filter(|s| s.0 >= BLOWN_SHARE).map(|s| s.1).collect();
    views.retain(|v| !out.contains(&v.keyframe));
    out.len()
}

/// Bins of the distance from the frame's middle, [`undo_faces_vignette`]'s curve.
const VIGNETTE_BINS: usize = 32;
/// The largest distance from the frame's middle, `√2` in units of half the frame's width and height.
const VIGNETTE_REACH: f32 = std::f32::consts::SQRT_2;
/// Below this much at the corners, the curve is left alone: what the stamps' medians can tell from nothing.
const VIGNETTE_MIN: f32 = 0.02;

/// The world's vignette, measured from the rig's own stamps and undone in its faces. Faces are drawn before the world's
/// post-processing, so its vignette darkens them by where their tile stands in the frame, not by what they show. Tiles that fill
/// the frame reach its corners: 18 % darker in Calling all villains and 11 % in Kitty Lounge of 2026-11-04, nothing in Dia de
/// Muertos or Rainy Skyline. Emulated on those two, three runs each, faces trained with it lost 0.33 dB on held-out views of the
/// rig against the square layout (0.44 in the lounge, 0.21 in the hall) and undone 0.02 (0.30 gained, 0.34 lost: the lounge recovers,
/// the hall varies too much between runs to tell, its LPIPS level either way). Each stamp's `on` blocks are one grey, linear 0.3, at a known place in the
/// frame: in every rig frame they measure the world's vignette along two rows across the frame's whole width, whatever the
/// world looks like. The median of a session's blocks by distance from the frame's middle is its curve (spread 2 % of its value at the
/// far end on real faces, against 0.11 to 0.18 of effect, and it agrees with the darkening found by comparing keyframes to their
/// renders). The curve is divided out in the same coded sRGB the block was read in: a vignette is a factor on linear light, and sRGB's
/// gamma keeps a factor one across every level, near enough. A session whose curve stays under [`VIGNETTE_MIN`] is left as it was.
/// Returns the darkening at the frame's corners.
fn undo_faces_vignette(views: &mut [View], records: &[FrameRecord], geometry: &Geometry) -> f32 {
    use std::collections::HashMap;
    let face_record: HashMap<i32, &FrameRecord> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.keyframe_index >= 0 && strip::flags::face(r.strip_flags) != 0).map(|r| (r.keyframe_index, r)).collect();
    // A face's frame: the rig frame's own record, of the same game time.
    let frames: HashMap<u32, (u32, u32)> =
        records.iter().filter(|r| strip::flags::kind(r.strip_flags) == strip::flags::KIND_RIG && strip::flags::face(r.strip_flags) == 0).map(|r| (r.game_time.to_bits(), (r.width, r.height))).collect();
    // A face's place in its frame, when its record and that frame's layout agree: (the tile, its size at the views' scale).
    let place = |v: &View| -> Option<(rig::RigLayout, usize, u32, Rect)> {
        let r = face_record.get(&v.keyframe)?;
        let (w, h) = *frames.get(&r.game_time.to_bits())?;
        let layout = rig::RigLayout::of_strip(w, h, &r.projection)?;
        ((layout.tile_width, layout.tile_height) == (r.width, r.height)).then_some(())?;
        let t = rig::TILE_FACES.iter().position(|&f| f == strip::flags::face(r.strip_flags))?;
        Some((layout, t, r.game_time.to_bits(), layout.tile(t)))
    };
    let distance = |x: f32, y: f32, w: u32, h: u32| {
        let (cx, cy) = (2.0 * x / w as f32 - 1.0, 2.0 * y / h as f32 - 1.0);
        (cx * cx + cy * cy).sqrt()
    };
    let bin = |d: f32| ((d / VIGNETTE_REACH * VIGNETTE_BINS as f32) as usize).min(VIGNETTE_BINS - 1);
    let f = geometry.factor;
    let luma = |p: u32| ((p & 0xff) + ((p >> 8) & 0xff) + ((p >> 16) & 0xff)) as f32 / (3.0 * 255.0);
    let mut levels: Vec<Vec<f32>> = vec![Vec::new(); VIGNETTE_BINS];
    for v in views.iter() {
        let Some((layout, _, bits, tile)) = place(v) else { continue };
        let (tw, th) = (layout.tile_width, layout.tile_height);
        let width = v.camera.width;
        for block in 1..rig::STAMP_BLOCKS {
            if !rig::stamp_block_on(bits, block) {
                continue;
            }
            let c = rig::stamp_cell(tw, th, layout.generation, block);
            let (x0, x1, y0, y1) = (c.x0 / f, (c.x1 / f).max(c.x0 / f + 1), c.y0 / f, (c.y1 / f).max(c.y0 / f + 1));
            if x1 > width || y1 > v.camera.height {
                continue;
            }
            let (mut sum, mut n) = (0.0f32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += luma(v.image[(y * width + x) as usize]);
                    n += 1;
                }
            }
            let d = distance(tile.x0 as f32 + (c.x0 + c.x1) as f32 / 2.0, tile.y0 as f32 + (c.y0 + c.y1) as f32 / 2.0, layout.width, layout.height);
            levels[bin(d)].push(sum / n as f32);
        }
    }
    let mut medians: Vec<Option<f32>> = levels
        .iter_mut()
        .map(|l| {
            (l.len() >= 8).then(|| {
                l.sort_by(f32::total_cmp);
                l[l.len() / 2]
            })
        })
        .collect();
    let Some(first) = medians.iter().position(Option::is_some) else { return 0.0 };
    let reference = medians[first].unwrap_or(0.0);
    if reference < 0.1 {
        return 0.0;
    }
    // Falling with the distance, nothing over 1: what bleeds into a block is not the world's vignette. Bins with no block take the
    // nearest measured below them (the corners' from the last row's far end).
    let mut factor = [1.0f32; VIGNETTE_BINS];
    let mut last = 1.0f32;
    for (i, m) in medians.iter_mut().enumerate() {
        if let Some(m) = m {
            last = last.min(*m / reference);
        }
        factor[i] = last;
    }
    let corners = 1.0 - factor[VIGNETTE_BINS - 1];
    if corners < VIGNETTE_MIN {
        return 0.0;
    }
    for v in views.iter_mut() {
        let Some((layout, _, _, tile)) = place(v) else { continue };
        let width = v.camera.width;
        for y in 0..v.camera.height {
            for x in 0..width {
                let d = distance(tile.x0 as f32 + (x * f + f / 2) as f32, tile.y0 as f32 + (y * f + f / 2) as f32, layout.width, layout.height);
                let at = (d / VIGNETTE_REACH * VIGNETTE_BINS as f32 - 0.5).clamp(0.0, VIGNETTE_BINS as f32 - 1.0);
                let (i, t) = (at as usize, at.fract());
                let k = factor[i] * (1.0 - t) + factor[(i + 1).min(VIGNETTE_BINS - 1)] * t;
                let p = &mut v.image[(y * width + x) as usize];
                let channel = |c: u32| ((((*p >> c) & 0xff) as f32 / k + 0.5) as u32).min(255) << c;
                *p = (*p & 0xff00_0000) | channel(0) | channel(8) | channel(16);
            }
        }
    }
    corners
}

/// A face's mask at its training resolution: a training pixel is scene when the captured pixels it averages are
/// (rig::scene_pixel: not in the face's border or its stamp).
fn face_mask(g: &Geometry) -> Vec<f32> {
    let (f, w, h) = (g.factor, g.full_width, g.full_height);
    let scene = |x: u32, y: u32| rig::scene_pixel(w, h, x * f, y * f) && rig::scene_pixel(w, h, x * f + f - 1, y * f + f - 1);
    (0..g.height()).flat_map(|y| (0..g.width()).map(move |x| if scene(x, y) { 1.0 } else { 0.0 })).collect()
}

/// A keyframe at the training resolution with its camera.
fn load_keyframe(dir: &Path, r: &FrameRecord, geometry: &Geometry) -> anyhow::Result<View> {
    Ok(View { camera: geometry.camera(&r.camera_to_world, &r.projection), image: keyframe_image(dir, r.keyframe_index, geometry)?, game_time: r.game_time, keyframe: r.keyframe_index })
}

/// Keyframe `index` of the session in `dir` as captured, at the training resolution of `geometry` (scoring reads the
/// held-out ones again after the keyframes in memory were realigned: crate::aberration).
pub fn keyframe_image(dir: &Path, index: i32, geometry: &Geometry) -> anyhow::Result<Vec<u32>> {
    let path = dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(index));
    let data = std::fs::read(&path).with_context(|| tr!("reading {path}", path = path.display()))?;
    let (w, h) = (geometry.full_width, geometry.full_height);
    // Refused before anything is allocated when its header gives another size.
    let (_, rgba) = qoi::decode_rgba_sized(&data, w, h).map_err(|e| anyhow!("{}", tr!("{path} (a keyframe of the session's {width}×{height}): {error}", path = path.display(), width = w, height = h, error = format!("{e:?}"))))?;
    Ok(downscale(&rgba, w, h, geometry.factor))
}

/// A depth frame's samples on the session's grid; `None` when its file is missing, damaged (its CRC) or laid out
/// otherwise (another frame size or a damaged header would put its samples at the wrong pixels).
fn load_depth(dir: &Path, r: &FrameRecord, geometry: &Geometry, grid: &Arc<DepthGrid>) -> Option<DepthView> {
    let data = std::fs::read(dir.join(DEPTH_DIR).join(bundle::depth_file(r.depth_index))).ok()?;
    let file = depth_file::read_for_frame(&data, geometry.full_width, geometry.full_height).ok()?;
    if (file.grid_width, file.grid_height) != (grid.width, grid.height) || file.codes.len() != grid.len() {
        return None;
    }
    let (zero_may_be_unread, codes) = (file.zero_may_be_unread(), file.codes);
    // Unused tiles read nothing, which is not "nothing there"; the decoder's unknowns are its unreadable tiles.
    let mut unknown = grid.unused.clone();
    let mut decoder_unknown = 0;
    let mut depth = Vec::with_capacity(codes.len());
    for (i, &code) in codes.iter().enumerate() {
        if code == depth::UNKNOWN && !grid.unused.contains(i) {
            unknown.insert(i);
            decoder_unknown += 1;
        }
        depth.push(depth::dequantize(code).unwrap_or(0.0));
    }
    Some(DepthView { camera: geometry.camera(&r.camera_to_world, &r.projection), grid: grid.clone(), depth, game_time: r.game_time, unknown, decoder_unknown, zero_may_be_unread })
}

/// `job(0)`, …, `job(jobs − 1)` on the loading threads (crate::priority::loading_threads), work handed out one job at
/// a time; `progress` (0..1) runs on the calling thread every 50 ms. Results in job order.
fn run_parallel<T: Send>(jobs: usize, job: impl Fn(usize) -> T + Sync, progress: &mut impl FnMut(f32)) -> Vec<T> {
    let threads = crate::priority::loading_threads().clamp(1, jobs.max(1));
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let mut out: Vec<Option<T>> = (0..jobs).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let j = next.fetch_add(1, Ordering::Relaxed);
                        if j >= jobs {
                            break mine;
                        }
                        mine.push((j, job(j)));
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        loop {
            progress(done.load(Ordering::Relaxed) as f32 / jobs.max(1) as f32);
            if handles.iter().all(|h| h.is_finished()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for h in handles {
            for (j, v) in h.join().expect("loading thread") {
                out[j] = Some(v);
            }
        }
    });
    out.into_iter().map(|v| v.expect("every job ran")).collect()
}

fn most_common(sizes: impl Iterator<Item = (u32, u32)>) -> Option<(u32, u32)> {
    let mut counts: Vec<((u32, u32), usize)> = Vec::new();
    for s in sizes {
        match counts.iter_mut().find(|(t, _)| *t == s) {
            Some((_, c)) => *c += 1,
            None => counts.push((s, 1)),
        }
    }
    // The first of equally common sizes.
    counts.iter().rev().max_by_key(|(_, c)| *c).map(|(s, _)| *s)
}

/// Whether most keyframes show the skeleton band: the pilot rows of its tiles (levels 0, 1, 2, 3 repeated) follow
/// that ramp. Read in the red channel, which PPv2's chromatic aberration smears least.
fn band_drawn(views: &[View], geometry: &Geometry) -> bool {
    let Some(b) = skeleton::BandLayout::new(geometry.full_width, geometry.full_height) else { return false };
    let (w, h, f) = (geometry.width(), geometry.height(), geometry.factor as f32);
    let ramp: Vec<f32> = (0..depth::TILE_CELLS).map(|j| (j % depth::LEVELS as u32) as f32).collect();
    let sampled: Vec<&View> = views.iter().step_by((views.len() / 16).max(1)).collect();
    let shows = |v: &View| {
        let mut sum = 0.0;
        // The band's tiles (its last row may have room for more, left black).
        for p in 0..skeleton::BAND_TILES {
            let (x0, y0) = b.tile_origin(p);
            let cells: Vec<f32> = (0..depth::TILE_CELLS)
                .map(|j| {
                    let x = (((x0 + j * b.cell) as f32 + b.cell as f32 / 2.0) / f) as u32;
                    let y = ((y0 as f32 + b.cell as f32 / 2.0) / f) as u32;
                    channel(v.image[(y.min(h - 1) * w + x.min(w - 1)) as usize], 0)
                })
                .collect();
            sum += correlation(&cells, &ramp);
        }
        sum / skeleton::BAND_TILES as f32 > 0.5
    };
    2 * sampled.iter().filter(|v| shows(v)).count() > sampled.len()
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    if aa <= 1e-9 || bb <= 1e-9 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

/// Box-filters RGBA to RGB floats by an integer `factor`.
fn downscale(rgba: &[u8], width: u32, height: u32, factor: u32) -> Vec<u32> {
    let (w, h) = (width / factor, height / factor);
    let mut out = vec![0u32; (w * h) as usize];
    let n = factor * factor;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 3];
            for dy in 0..factor {
                let row = ((y * factor + dy) * width + x * factor) as usize * 4;
                for dx in 0..factor as usize {
                    let p = &rgba[row + dx * 4..row + dx * 4 + 3];
                    acc[0] += p[0] as u32;
                    acc[1] += p[1] as u32;
                    acc[2] += p[2] as u32;
                }
            }
            let [r, g, b] = acc.map(|a| (a + n / 2) / n);
            out[(y * w + x) as usize] = r | (g << 8) | (b << 16) | (255 << 24);
        }
    }
    out
}

/// One channel (0 red, 1 green, 2 blue) of a packed pixel, in 0..1.
pub fn channel(pixel: u32, c: usize) -> f32 {
    ((pixel >> (8 * c)) & 255) as f32 / 255.0
}

/// Bilinear sample of a keyframe at pixel coordinates (pixel centres at integers), RGB in 0..1; `None` outside.
pub fn sample_rgb(image: &[u32], width: u32, height: u32, u: f32, v: f32) -> Option<[f32; 3]> {
    if !(u >= 0.0 && v >= 0.0 && u <= (width - 1) as f32 && v <= (height - 1) as f32) {
        return None;
    }
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: u32, y: u32, c: usize| channel(image[(y * width + x) as usize], c);
    let mut out = [0f32; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        *o = top * (1.0 - fy) + bottom * fy;
    }
    Some(out)
}

/// Session folders for tests.
#[cfg(test)]
pub(crate) mod fixture {
    use orbiters_contracts::frame_record::{record_flags, FrameRecord};
    use orbiters_contracts::manifest::{depth_file, SessionManifest, DEPTH_DIR, FRAMES_FILE, KEYFRAMES_DIR, MANIFEST_FILE};
    use std::path::PathBuf;

    /// A session folder in the temp folder, removed when dropped.
    pub struct Session {
        pub dir: PathBuf,
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The pose of frame `k`: at (0.1·k, 1.6, 0), turned 9° a frame about the vertical.
    pub fn pose(k: usize) -> [f32; 12] {
        let a = (9.0 * k as f32).to_radians();
        [a.cos(), 0.0, a.sin(), 0.1 * k as f32, 0.0, 1.0, 0.0, 1.6, -a.sin(), 0.0, a.cos(), 0.0]
    }

    /// Writes `keyframes` keyframes of `w × h` (random colours) at [`pose`] `k`, then one depth frame per entry of
    /// `depth` at pose `k` too (its codes on the layout's sample grid of `w × h`; `None`: its file is missing).
    pub fn session(name: &str, w: u32, h: u32, keyframes: usize, depth: &[Option<Vec<u16>>]) -> Session {
        let dir = std::env::temp_dir().join(format!("orbiters-reconstruct-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(KEYFRAMES_DIR)).unwrap();
        std::fs::create_dir_all(dir.join(DEPTH_DIR)).unwrap();
        let manifest = SessionManifest::new("test".into(), "2026-10-21T00:00:00Z".into(), "0");
        std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_string(&manifest).unwrap()).unwrap();
        let mut rng = fastrand::Rng::with_seed(9);
        let mut frames = Vec::new();
        let record = |k: usize, flags: u16| FrameRecord {
            flags: record_flags::STRIP_OK | flags,
            frame_index: k as u64,
            game_time: k as f32 * 0.1,
            camera_to_world: pose(k),
            projection: [1.2, 1.2 * w as f32 / h as f32, 0.0, 0.0],
            width: w,
            height: h,
            ..Default::default()
        };
        for k in 0..keyframes {
            let rgb: Vec<u8> = (0..w * h * 3).map(|_| rng.u8(..)).collect();
            std::fs::write(dir.join(KEYFRAMES_DIR).join(orbiters_core::bundle::keyframe_file(k as i32)), orbiters_contracts::qoi::encode(&rgb, w, h, 3)).unwrap();
            frames.extend_from_slice(&FrameRecord { keyframe_index: k as i32, ..record(k, record_flags::KEYFRAME) }.to_bytes());
        }
        for (d, codes) in depth.iter().enumerate() {
            if let Some(codes) = codes {
                let (gw, gh) = orbiters_contracts::depth::DepthLayout::new(w, h).expect("a depth layout").grid_size();
                std::fs::write(dir.join(DEPTH_DIR).join(orbiters_core::bundle::depth_file(d as i32)), depth_file::write(gw, gh, w, h, codes)).unwrap();
            }
            frames.extend_from_slice(&FrameRecord { depth_index: d as i32, ..record(d, record_flags::DEPTH) }.to_bytes());
        }
        std::fs::write(dir.join(FRAMES_FILE), frames).unwrap();
        Session { dir }
    }

    /// Adds `rig_frames` rig frames to `session` (frames of `w × h`) as an avatar of `generation` draws them: each one's
    /// record (kind 2, the rig at [`pose`] `k`, no picture) and its six faces' keyframes (random colours, the face's size),
    /// numbered after the session's.
    pub fn add_rig_frames(session: &Session, w: u32, h: u32, rig_frames: usize, generation: orbiters_contracts::rig::Generation) {
        use orbiters_contracts::{rig, strip};
        let layout = rig::RigLayout::of_generation(w, h, generation).expect("a rig layout");
        let (fw, fh) = (layout.tile_width, layout.tile_height);
        let mut frames = std::fs::read(session.dir.join(FRAMES_FILE)).unwrap();
        let first = std::fs::read_dir(session.dir.join(KEYFRAMES_DIR)).unwrap().count();
        let mut rng = fastrand::Rng::with_seed(11);
        let flags = |face: u32| strip::flags::compose(strip::flags::KIND_RIG, 2, 0, face, strip::flags::SOURCE_TEST);
        let mut keyframe = first;
        for k in 0..rig_frames {
            let rig_pose = pose(100 + k);
            let base = FrameRecord {
                flags: record_flags::STRIP_OK,
                frame_index: 1000 + 7 * k as u64,
                game_time: 10.0 + k as f32 * 0.1,
                strip_flags: flags(0),
                camera_to_world: rig_pose,
                projection: generation.projection(),
                near: rig::NEAR,
                far: rig::FAR,
                width: w,
                height: h,
                ..Default::default()
            };
            frames.extend_from_slice(&base.to_bytes());
            for (t, &face) in rig::TILE_FACES.iter().enumerate() {
                let rgb: Vec<u8> = (0..fw * fh * 3).map(|_| rng.u8(..)).collect();
                std::fs::write(session.dir.join(KEYFRAMES_DIR).join(orbiters_core::bundle::keyframe_file(keyframe as i32)), orbiters_contracts::qoi::encode(&rgb, fw, fh, 3)).unwrap();
                let r = FrameRecord {
                    flags: record_flags::STRIP_OK | record_flags::KEYFRAME,
                    frame_index: base.frame_index + 1 + t as u64,
                    strip_flags: flags(face),
                    camera_to_world: rig::face_camera_to_world(&rig_pose, face),
                    projection: generation.projection(),
                    width: fw,
                    height: fh,
                    keyframe_index: keyframe as i32,
                    ..base
                };
                frames.extend_from_slice(&r.to_bytes());
                keyframe += 1;
            }
        }
        std::fs::write(session.dir.join(FRAMES_FILE), frames).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::image::RgbaImage;

    fn view(img: &RgbaImage) -> View {
        let (w, h) = (img.width, img.height);
        let image = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| {
            let p = img.get(x, y);
            p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16 | 255 << 24
        });
        View { camera: Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, w, h), image: image.collect(), game_time: 0.0, keyframe: 0 }
    }

    #[test]
    fn detail_views_are_the_few_keyframes_zoomed_in_far() {
        // A 4 s picture: 37 keyframes at 47.3° (focal length over height 1.143), nine close-ups at 20° (2.836).
        let mut focals = vec![1.143f32; 37];
        focals.splice(1..1, [2.836; 9]);
        assert_eq!(detail_views(&focals), (1..10).collect::<Vec<_>>());
        // Not a share of a recording zoomed in for most of it (its usual view is the zoomed one), nor a slight zoom.
        let mut zoomed = vec![1.143f32; 20];
        zoomed.extend([2.836; 15]);
        assert!(detail_views(&zoomed).is_empty());
        let mut slight = vec![1.143f32; 30];
        slight[4] = 1.143 * 1.25;
        assert!(detail_views(&slight).is_empty());
        // Frames of another size (NaN) count for nothing; too few keyframes have none.
        let mut odd = vec![f32::NAN; 3];
        odd.extend([1.143, 1.143, 1.143, 2.836]);
        assert_eq!(detail_views(&odd), vec![6]);
        assert!(detail_views(&[1.0, 3.0]).is_empty());
        // A loaded session's views: by their training cameras.
        let s = fixture::session("detail-views", 160, 90, 12, &[]);
        let mut d = load_with(&s.dir, 160, Budget::UNLIMITED, |_| {}).unwrap();
        assert!(d.detail_views().is_empty());
        for v in &mut d.views[2..4] {
            v.camera.fx *= 2.35;
            v.camera.fy *= 2.35;
        }
        assert_eq!(d.detail_views(), vec![2, 3]);
    }

    /// A capture size the training factor does not divide: keyframes, cameras, depth-sample positions and the mask
    /// all have the training size (the remainder cropped), and the pixels are the captured ones box-filtered.
    #[test]
    fn rig_faces_load_as_views_of_their_own() {
        use orbiters_contracts::rig;
        let s = fixture::session("rig-faces", 1280, 720, 6, &[]);
        fixture::add_rig_frames(&s, 1280, 720, 3, rig::Generation::Fill);
        let l = rig::RigLayout::new(1280, 720).unwrap();
        for (max_width, factor) in [(1280, 1), (640, 2)] {
            let d = load_with(&s.dir, max_width, Budget::UNLIMITED, |_| {}).unwrap();
            // The frames' keyframes as before: the faces are neither among them nor left out as another size.
            assert_eq!((d.views.len(), d.width, d.loaded.keyframes_skipped), (6, 1280 / factor, 0));
            let f = d.faces.as_ref().expect("faces");
            assert_eq!((f.views.len(), d.loaded.faces, f.sizes.len()), (18, 18, 1));
            let size = &f.sizes[0];
            assert_eq!((size.geometry.factor, size.width, size.height), (factor, l.tile_width / factor, l.tile_height / factor));
            // Their cameras: the faces' projection stretched over the tile, along the rig's axes.
            for v in &f.views {
                assert_eq!((v.camera.width, v.camera.height), (size.width, size.height));
                assert!(f.size_of(&v.camera).is_some());
                let (fx, fy) = (rig::PROJECTION[0] * l.tile_width as f32 / 2.0 / factor as f32, rig::PROJECTION[1] * l.tile_height as f32 / 2.0 / factor as f32);
                assert!((v.camera.fx - fx).abs() < 1e-2 && (v.camera.fy - fy).abs() < 1e-2, "{} {}", v.camera.fx, v.camera.fy);
            }
            // The mask leaves the border and the stamp out, keeps the middle.
            let at = |x: u32, y: u32| size.mask[(y * size.width + x) as usize];
            assert_eq!((at(0, size.height / 2), at(size.width / 2, size.height / 2), at(size.width / 2, size.height - 1)), (0.0, 1.0, 0.0));
            assert!(size.scene_pixels > 0.9 * (size.width * size.height) as f32);
        }
        // Without faces, nothing changes.
        let plain = fixture::session("no-rig-faces", 1280, 720, 6, &[]);
        assert!(load_with(&plain.dir, 1280, Budget::UNLIMITED, |_| {}).unwrap().faces.is_none());
    }

    /// A session recorded before the layout filled the frame still reconstructs: its records give the square faces' size
    /// and projection, as they were stored.
    #[test]
    fn a_session_of_square_faces_still_loads() {
        use orbiters_contracts::rig;
        let s = fixture::session("rig-faces-square", 1920, 1080, 6, &[]);
        fixture::add_rig_frames(&s, 1920, 1080, 2, rig::Generation::Square);
        let d = load_with(&s.dir, 1920, Budget::UNLIMITED, |_| {}).unwrap();
        let f = d.faces.as_ref().expect("faces");
        assert_eq!((f.views.len(), d.loaded.faces_skipped, f.sizes.len()), (12, 0, 1));
        assert_eq!((f.sizes[0].width, f.sizes[0].height), (420, 420));
        // 90° square pinholes, the mask as it always was: the 3 px border, the stamp's 7 rows.
        for v in &f.views {
            assert!((v.camera.fx - 210.0).abs() < 1e-3 && (v.camera.fy - 210.0).abs() < 1e-3);
        }
        let at = |x: u32, y: u32| f.sizes[0].mask[(y * 420 + x) as usize];
        assert_eq!((at(2, 200), at(3, 200), at(200, 412), at(200, 413)), (0.0, 1.0, 1.0, 0.0));
    }

    /// A world's vignette: a rig frame's faces with their stamps, every pixel darkened by the place of its tile in the frame. Reading
    /// the stamps gives the curve back, and the same scene comes back the same in every tile.
    #[test]
    fn the_stamps_measure_the_worlds_vignette_and_it_is_undone() {
        use orbiters_contracts::{image::RgbaImage, rig};
        let (w, h) = (1920u32, 1080u32);
        let layout = rig::RigLayout::new(w, h).unwrap();
        let vignette = |x: f32, y: f32| {
            let (cx, cy) = (2.0 * x / w as f32 - 1.0, 2.0 * y / h as f32 - 1.0);
            1.0 - 0.1 * (cx * cx + cy * cy)
        };
        let s = fixture::session("rig-faces-vignette", w, h, 6, &[]);
        fixture::add_rig_frames(&s, w, h, 30, rig::Generation::Fill);
        let records = bundle::read_records(&s.dir).unwrap();
        let scene = 160u8;
        for r in records.iter().filter(|r| strip::flags::face(r.strip_flags) != 0 && r.has(record_flags::KEYFRAME)) {
            let t = rig::TILE_FACES.iter().position(|&f| f == strip::flags::face(r.strip_flags)).unwrap();
            let tile = layout.tile(t);
            let mut img = RgbaImage::new(layout.tile_width, layout.tile_height);
            img.fill_rect(0, 0, layout.tile_width, layout.tile_height, [scene; 3]);
            rig::draw_stamp(&mut img, r.game_time, rig::Generation::Fill);
            let mut rgb = Vec::new();
            for y in 0..img.height {
                for x in 0..img.width {
                    let k = vignette(tile.x0 as f32 + x as f32 + 0.5, tile.y0 as f32 + y as f32 + 0.5);
                    rgb.extend(img.get(x, y)[..3].iter().map(|c| (*c as f32 * k + 0.5) as u8));
                }
            }
            std::fs::write(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(r.keyframe_index)), qoi::encode(&rgb, img.width, img.height, 3)).unwrap();
        }
        let d = load_with(&s.dir, w, Budget::UNLIMITED, |_| {}).unwrap();
        // The corners' 20 %: measured along the bottom row's far ends, 0.2 at most.
        assert!((17..=23).contains(&d.loaded.faces_vignette), "{}", d.loaded.faces_vignette);
        let f = d.faces.as_ref().unwrap();
        // The scene in a far corner of a bottom tile and in the middle of the top one, as one level again.
        for v in &f.views {
            let at = |x: u32, y: u32| (v.image[(y * v.camera.width + x) as usize] & 0xff) as f32;
            for (x, y) in [(10, 10), (320, 200), (630, 400)] {
                assert!((at(x, y) - scene as f32).abs() <= 4.0, "keyframe {}: {} at ({x}, {y})", v.keyframe, at(x, y));
            }
        }
    }

    /// A world with no vignette: its faces are as they were read.
    #[test]
    fn no_vignette_leaves_the_faces_as_they_are() {
        use orbiters_contracts::{image::RgbaImage, rig};
        let (w, h) = (1920u32, 1080u32);
        let layout = rig::RigLayout::new(w, h).unwrap();
        let s = fixture::session("rig-faces-no-vignette", w, h, 6, &[]);
        fixture::add_rig_frames(&s, w, h, 12, rig::Generation::Fill);
        let records = bundle::read_records(&s.dir).unwrap();
        for r in records.iter().filter(|r| strip::flags::face(r.strip_flags) != 0 && r.has(record_flags::KEYFRAME)) {
            let mut img = RgbaImage::new(layout.tile_width, layout.tile_height);
            img.fill_rect(0, 0, layout.tile_width, layout.tile_height, [160; 3]);
            rig::draw_stamp(&mut img, r.game_time, rig::Generation::Fill);
            let rgb: Vec<u8> = img.data.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            std::fs::write(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(r.keyframe_index)), qoi::encode(&rgb, img.width, img.height, 3)).unwrap();
        }
        let d = load_with(&s.dir, w, Budget::UNLIMITED, |_| {}).unwrap();
        assert_eq!(d.loaded.faces_vignette, 0);
        for v in &d.faces.as_ref().unwrap().views {
            assert!(v.image[..(v.camera.width * 400) as usize].iter().all(|p| p & 0xff == 160));
        }
    }

    /// A session holding both (the avatar re-uploaded and worn again in one recording): each face with its own size and mask.
    #[test]
    fn a_session_of_both_layouts_keeps_each_faces_size() {
        use orbiters_contracts::rig;
        let s = fixture::session("rig-faces-both", 1920, 1080, 6, &[]);
        fixture::add_rig_frames(&s, 1920, 1080, 1, rig::Generation::Square);
        fixture::add_rig_frames(&s, 1920, 1080, 1, rig::Generation::Fill);
        let d = load_with(&s.dir, 1920, Budget::UNLIMITED, |_| {}).unwrap();
        let f = d.faces.as_ref().expect("faces");
        assert_eq!((f.views.len(), f.sizes.len()), (12, 2));
        for v in &f.views {
            let size = f.size_of(&v.camera).expect("its size");
            assert_eq!(size.mask.len(), (v.camera.width * v.camera.height) as usize);
        }
    }

    /// The whole way: a synthetic rig frame is drawn as the shader draws it, read as the capture reads it, and a point of
    /// the world lands in the stored face where the reconstruction's camera says.
    #[test]
    fn a_world_point_lands_where_the_training_camera_says() {
        use orbiters_contracts::{image::RgbaImage, rig, strip};
        let (w, h, time) = (1920u32, 1080u32, 12.5f32);
        let rig_pose = fixture::pose(3);
        let origin = [rig_pose[3], rig_pose[7], rig_pose[11]];
        // A point in the world, in front of each face in turn: the render's own pixel for it is painted white.
        for (t, &face) in rig::TILE_FACES.iter().enumerate() {
            let camera = rig::face_camera_to_world(&rig_pose, face);
            let projection = rig::PROJECTION;
            let (ax, ay, depth) = (0.27f32, -0.19f32, 6.0f32);
            let world: [f32; 3] = std::array::from_fn(|i| origin[i] + depth * (ax * camera[4 * i] + ay * camera[4 * i + 1] + camera[4 * i + 2]));
            // The render's picture: the face camera's uv of the point, 640 × 420.
            let (rw, rh) = rig::FACE_RENDER;
            let u = 0.5 + 0.5 * projection[0] * ax;
            let v = 0.5 + 0.5 * projection[1] * ay;
            let (px, py) = ((u * rw as f32) as u32, ((1.0 - v) * rh as f32) as u32);
            let pictures: [RgbaImage; rig::FACES] = std::array::from_fn(|i| {
                let mut img = RgbaImage::new(rw, rh);
                img.fill_rect(0, 0, rw, rh, [30, 40, 50]);
                if i as u32 + 1 == face {
                    img.set(px, py, [255, 255, 255]);
                }
                rig::draw_stamp(&mut img, time, rig::Generation::Fill);
                img
            });
            let mut frame = RgbaImage::new(w, h);
            let payload = rig::payload(rig_pose, time, w, h, 2, strip::flags::SOURCE_TEST);
            rig::encode(&mut frame, &payload, &pictures).unwrap();
            // Read as the capture reads it.
            let view = frame.view();
            let decoded = strip::decode(&view).unwrap();
            let faces = rig::faces(&decoded.payload, &view, decoded.flipped).unwrap();
            let f = faces[t];
            assert!(f.current(time) && f.face == face);
            let tile = f.tile;
            let found = (tile.y0..tile.y1).flat_map(|y| (tile.x0..tile.x1).map(move |x| (x, y))).find(|&(x, y)| frame.get(x, y)[0] == 255).expect("the point's pixel");
            // The training camera of that stored face (crate::camera, the record's pose and projection).
            let g = Geometry::new(tile.x1 - tile.x0, tile.y1 - tile.y0, 1);
            let cam = g.camera(&f.camera_to_world, &f.projection);
            let (cx, cy, z) = cam.project(world).expect("in front");
            assert!((z - depth).abs() < 1e-3, "face {face}: depth {z}");
            let (sx, sy) = ((found.0 - tile.x0) as f32, (found.1 - tile.y0) as f32);
            assert!((cx - sx).abs() <= 1.0 && (cy - sy).abs() <= 1.0, "face {face}: the camera says ({cx}, {cy}), the pixel is at ({sx}, {sy})");
        }
    }

    #[test]
    fn odd_capture_sizes_train_at_one_consistent_size() {
        let (w, h) = (1283u32, 723u32);
        let layout = DepthLayout::new(w, h).expect("layout");
        let (gw, gh) = layout.grid_size();
        let s = fixture::session("odd", w, h, 3, &[Some(vec![30_000; (gw * gh) as usize])]);
        let data = load_with(&s.dir, 640, Budget::UNLIMITED, |_| {}).expect("load");
        assert_eq!((data.width, data.height, data.geometry.factor), (427, 241, 3));
        assert_eq!(data.mask.len(), 427 * 241);
        for v in &data.views {
            assert_eq!((v.camera.width, v.camera.height), (data.width, data.height));
            assert_eq!(v.image.len(), (data.width * data.height) as usize);
        }
        // The first keyframe's pixel (100, 50) averages captured pixels 300..303 × 150..153.
        let file = std::fs::read(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(0))).unwrap();
        let (_, rgba) = qoi::decode_rgba(&file).unwrap();
        let mut sum = [0u32; 3];
        for y in 150..153 {
            for x in 300..303 {
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += rgba[((y * w + x) * 4) as usize + c] as u32;
                }
            }
        }
        let got = data.views[0].image[(50 * data.width + 100) as usize];
        assert_eq!([0, 1, 2].map(|c| (got >> (8 * c)) & 255), sum.map(|s| (s + 4) / 9));
        // Depth samples sit where their captured pixels are, and its camera is the keyframes' kind.
        let d = &data.depth_views[0];
        assert_eq!((d.camera.width, d.camera.height), (data.width, data.height));
        let (px, py) = layout.grid_pixel(7, 4);
        assert_eq!(d.grid.pixels[(4 * gw + 7) as usize], [data.geometry.to_training(px as f32), data.geometry.to_training(py as f32)]);
        assert!((d.grid.spacing - layout.tile_px as f32 / SAMPLES_X as f32 / 3.0).abs() < 1e-6);
    }

    /// Code 0 is "nothing there"; 65535 is the decoder's unknown; grid cells without a sample (tiles under the strip's
    /// area, a larger tile's other cells) carry nothing and say nothing either; a depth file whose grid is not the
    /// layout's is left out.
    #[test]
    fn depth_codes_tell_sky_from_unknown_and_unused_tiles() {
        let (w, h) = (960u32, 540u32);
        let layout = DepthLayout::new(w, h).expect("layout");
        let (gw, gh) = layout.grid_size();
        let n = (gw * gh) as usize;
        let mut codes = vec![depth::quantize(5.0); n];
        let sampled = |i: usize| layout.grid_sampled(i as u32 % gw, i as u32 / gw);
        let used = (0..n - 2).find(|&i| (i..i + 3).all(sampled)).unwrap();
        let unused = (0..n).find(|&i| !sampled(i)).expect("cells without a sample");
        assert!((0..n).any(|i| !sampled(i) && !layout.tile_used(i as u32 % gw / SAMPLES_X, i as u32 / gw / depth::SAMPLES_Y)), "tiles under the strip");
        codes[used] = 0;
        codes[used + 1] = depth::UNKNOWN;
        let s = fixture::session("codes", w, h, 2, &[Some(codes.clone()), None, None]);
        // A second depth frame whose header claims another grid, a third in `.d16` version 1 (no CRC: zeros there may
        // be samples nobody read).
        let bad = depth_file::write(gw + 1, gh, w, h, &vec![1; ((gw + 1) * gh) as usize]);
        std::fs::write(s.dir.join(DEPTH_DIR).join(bundle::depth_file(1)), bad).unwrap();
        let mut v1 = depth_file::write(gw, gh, w, h, &codes);
        v1.truncate(v1.len() - depth_file::CRC_SIZE);
        v1[4..6].copy_from_slice(&1u16.to_le_bytes());
        std::fs::write(s.dir.join(DEPTH_DIR).join(bundle::depth_file(2)), v1).unwrap();
        let data = load_with(&s.dir, 960, Budget::UNLIMITED, |_| {}).expect("load");
        assert_eq!(data.depth_views.len(), 2);
        assert_eq!(data.loaded.depth_frames_skipped, 1);
        assert!(!data.depth_views[0].zero_may_be_unread && data.depth_views[1].zero_may_be_unread);
        let d = &data.depth_views[0];
        assert_eq!(d.depth[used], 0.0);
        assert!(!d.is_unknown(used), "0 stays sky");
        assert!(d.is_unknown(used + 1) && d.depth[used + 1] == 0.0);
        assert!(d.is_unknown(unused), "unused tiles say nothing");
        assert_eq!(d.decoder_unknown, 1);
        assert!((d.depth[used + 2] - 5.0).abs() < 0.01);
        assert_eq!(d.valid(), n - 2 - d.grid.unused.len());
    }

    /// A keyframe that cannot be read costs that keyframe, not the session.
    #[test]
    fn unreadable_keyframes_are_left_out() {
        let s = fixture::session("unreadable", 640, 360, 5, &[]);
        std::fs::remove_file(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(1))).unwrap();
        std::fs::write(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(3)), b"qoif broken").unwrap();
        let data = load_with(&s.dir, 640, Budget::UNLIMITED, |_| {}).expect("load");
        assert_eq!(data.views.iter().map(|v| v.keyframe).collect::<Vec<_>>(), vec![0, 2, 4]);
        assert_eq!(data.loaded.keyframes_skipped, 2);
        assert!(data.loaded.first_problem.is_some());
        for k in [0, 2] {
            std::fs::remove_file(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(k))).unwrap();
        }
        let e = load_with(&s.dir, 640, Budget::UNLIMITED, |_| {}).err().expect("one keyframe left");
        assert!(format!("{e:#}").contains("Only 1 of this session's 5 keyframes"), "{e:#}");
    }

    /// A frame mostly blown to white leaves, as long as the session is not a white world: the fog it trains is seen from
    /// everywhere else. In a white world every frame stays.
    #[test]
    fn blown_out_keyframes_are_left_out_unless_the_world_is_white() {
        let (w, h) = (64u32, 36u32);
        let white = |s: &fixture::Session, k: i32, share: f32| {
            let n = (w * h) as usize;
            let rgb: Vec<u8> = (0..n).flat_map(|i| if (i as f32) < share * n as f32 { [255u8; 3] } else { [40u8; 3] }).collect();
            std::fs::write(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(k)), orbiters_contracts::qoi::encode(&rgb, w, h, 3)).unwrap();
        };
        let s = fixture::session("blown", w, h, 12, &[]);
        white(&s, 7, 0.5);
        // White in a part of the frame is a lamp or a window.
        white(&s, 3, 0.1);
        let data = load_with(&s.dir, w, Budget::UNLIMITED, |_| {}).expect("load");
        assert_eq!(data.loaded.keyframes_blown, 1);
        assert!(data.views.iter().all(|v| v.keyframe != 7) && data.views.iter().any(|v| v.keyframe == 3));

        let s = fixture::session("blown-world", w, h, 12, &[]);
        (0..12).for_each(|k| white(&s, k, 0.6));
        let data = load_with(&s.dir, w, Budget::UNLIMITED, |_| {}).expect("load");
        assert_eq!((data.loaded.keyframes_blown, data.views.len()), (0, 12));
    }

    /// Records may name files the capture never finished writing (CAP-5): a missing keyframe and a damaged or missing
    /// depth file are left out and counted, the rest of the session trains.
    #[test]
    fn a_session_missing_files_still_loads() {
        let (w, h) = (640u32, 360u32);
        let (gw, gh) = DepthLayout::new(w, h).expect("layout").grid_size();
        let codes = vec![depth::quantize(4.0); (gw * gh) as usize];
        let s = fixture::session("missing", w, h, 4, &[Some(codes.clone()), Some(codes.clone()), None, Some(codes)]);
        std::fs::remove_file(s.dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(2))).unwrap();
        // One flipped byte in the second depth file's codes: its CRC no longer matches.
        let path = s.dir.join(DEPTH_DIR).join(bundle::depth_file(1));
        let mut damaged = std::fs::read(&path).unwrap();
        damaged[depth_file::HEADER_SIZE + 7] ^= 0x40;
        std::fs::write(&path, damaged).unwrap();
        let data = load_with(&s.dir, 640, Budget::UNLIMITED, |_| {}).expect("load");
        assert_eq!(data.views.iter().map(|v| v.keyframe).collect::<Vec<_>>(), vec![0, 1, 3]);
        assert_eq!(data.depth_views.len(), 2);
        let l = &data.loaded;
        assert_eq!((l.keyframes, l.keyframes_skipped, l.depth_frames, l.depth_frames_skipped), (4, 1, 4, 2));
        assert!(l.first_problem.as_deref().is_some_and(|p| p.contains("000002")), "{l:?}");
    }

    /// Past the budget, the keyframes kept are the most different views.
    #[test]
    fn the_budget_keeps_the_most_different_views() {
        // Ten distinct views among a hundred copies of one.
        let mut poses: Vec<([f32; 3], [f32; 3])> = vec![([0.0, 1.6, 0.0], [0.0, 0.0, 1.0]); 100];
        for k in 0..10 {
            poses[5 + 9 * k] = ([3.0 * k as f32 + 3.0, 1.6, 0.0], [0.0, 0.0, 1.0]);
        }
        let kept = spread(&poses, 11);
        assert_eq!(kept.len(), 11);
        assert!(kept.windows(2).all(|p| p[0] < p[1]), "in order");
        assert!((0..10).all(|k| kept.contains(&(5 + 9 * k))), "{kept:?}");
        assert_eq!(spread(&poses, 500).len(), 100);
        // At load: 70 keyframes' worth of budget out of 100.
        let s = fixture::session("budget", 320, 180, 100, &[]);
        let budget = Budget { keyframe_bytes: 70 * 320 * 180 * 4, depth_samples: u64::MAX };
        let data = load_with(&s.dir, 320, budget, |_| {}).expect("load");
        assert_eq!(data.views.len(), 70);
        assert_eq!(data.loaded.keyframes_over_budget, 30);
    }

    /// REC-20: over its budget, a capture keeps the frames zoomed in on something (at a pose its wide frames have too, which
    /// the poses cannot tell apart), as the detail views they train as.
    #[test]
    fn frames_zoomed_in_are_kept_within_the_budget() {
        use orbiters_contracts::manifest::FRAMES_FILE;
        let s = fixture::session("zoomed", 320, 180, 100, &[]);
        let mut records = bundle::read_records(&s.dir).expect("records");
        let standing = records[39].camera_to_world;
        for r in &mut records[40..44] {
            r.camera_to_world = standing;
            r.projection[0] *= 2.35;
            r.projection[1] *= 2.35;
        }
        std::fs::write(s.dir.join(FRAMES_FILE), records.iter().flat_map(|r| r.to_bytes()).collect::<Vec<u8>>()).expect("frames");
        let budget = Budget { keyframe_bytes: 70 * 320 * 180 * 4, depth_samples: u64::MAX };
        let data = load_with(&s.dir, 320, budget, |_| {}).expect("load");
        assert_eq!((data.views.len(), data.loaded.keyframes_over_budget), (70, 30));
        let kept: Vec<i32> = data.detail_views().into_iter().map(|i| data.views[i].keyframe as i32).collect();
        assert_eq!(kept, [40, 41, 42, 43], "the magnified frames");
        // With room for all, all of them; with no zoomed frame, what the poses alone keep (above).
        assert_eq!(load_with(&s.dir, 320, Budget::UNLIMITED, |_| {}).expect("load").detail_views().len(), 4);
    }

    /// REC-21: what is skipped without a root changes nothing that is kept.
    #[test]
    fn the_spread_keeps_what_every_distance_taken_whole_keeps() {
        let whole = |poses: &[([f32; 3], [f32; 3])], k: usize| {
            let distance = |a: usize, b: usize| {
                let ((pa, fa), (pb, fb)) = (poses[a], poses[b]);
                let d = |u: [f32; 3], v: [f32; 3]| ((u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2) + (u[2] - v[2]).powi(2)).sqrt();
                d(pa, pb) + d(fa, fb) / 0.35
            };
            let (mut nearest, mut out, mut next) = (vec![f32::INFINITY; poses.len()], Vec::new(), 0);
            while out.len() < k {
                out.push(next);
                let (mut best, mut far) = (0, f32::NEG_INFINITY);
                for i in (0..poses.len()).filter(|i| !out.contains(i)) {
                    nearest[i] = nearest[i].min(distance(next, i));
                    if nearest[i] > far {
                        (best, far) = (i, nearest[i]);
                    }
                }
                next = best;
            }
            out.sort_unstable();
            out
        };
        let mut rng = fastrand::Rng::with_seed(12);
        for (n, k) in [(300, 40), (300, 299), (50, 25)] {
            let mut at = [0f32; 3];
            let poses: Vec<([f32; 3], [f32; 3])> = (0..n)
                .map(|i| {
                    // A walk that stands still now and then, and turns.
                    if i % 5 != 0 {
                        at = [at[0] + rng.f32() - 0.4, at[1], at[2] + rng.f32() * 0.3];
                    }
                    let (s, c) = (i as f32 * 0.3).sin_cos();
                    (at, [s, 0.0, c])
                })
                .collect();
            assert_eq!(spread(&poses, k), whole(&poses, k));
        }
    }

    /// A quarter of what is kept at most goes to the zoomed frames, unless the others do not fill the rest; order kept; none
    /// zoomed, the spread alone.
    #[test]
    fn zoomed_frames_take_a_quarter_at_most() {
        let record = |i: usize, zoom: f32| FrameRecord { camera_to_world: [1.0, 0.0, 0.0, i as f32, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], projection: [zoom, zoom, 0.0, 0.0], ..Default::default() };
        let kept = |zoomed: usize, n: usize, k: usize| {
            let records: Vec<FrameRecord> = (0..n).map(|i| record(i, if i < zoomed { 3.0 } else { 1.0 })).collect();
            let kept = within(&records.iter().collect::<Vec<_>>(), k);
            assert!(kept.windows(2).all(|p| p[0] < p[1]) && kept.len() == k.min(n), "{kept:?}");
            (kept.iter().filter(|&&i| i < zoomed).count(), kept)
        };
        assert_eq!(kept(60, 200, 80).0, 20);
        assert_eq!(kept(5, 200, 80).0, 5);
        // 130 wide frames for 139 places.
        assert_eq!(kept(60, 190, 189).0, 59);
        assert_eq!(kept(60, 190, 190).0, 60);
        let poses: Vec<_> = (0..200).map(|i| pose(&record(i, 1.0))).collect();
        assert_eq!(kept(0, 200, 80).1, spread(&poses, 80));
    }

    /// The band is found in pictures whose band does not decode: with green and blue displaced towards the centre
    /// (chromatic aberration), and not in pictures without one.
    #[test]
    fn band_found_without_decoding() {
        let (w, h) = (1920u32, 1080u32);
        let mut rng = fastrand::Rng::with_seed(5);
        let noise = |rng: &mut fastrand::Rng| {
            let mut img = RgbaImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    img.set(x, y, [rng.u8(..), rng.u8(..), rng.u8(..)]);
                }
            }
            img
        };
        let skeleton = skeleton::Skeleton { time: 3.5, hips: [0.0, 1.0, 0.0], avatar: 7, ..Default::default() };
        let (mut plain, mut banded, mut smeared) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..4 {
            let img = noise(&mut rng);
            plain.push(view(&img));
            let mut b = img;
            skeleton::draw(&mut b, &skeleton).expect("band");
            banded.push(view(&b));
            // Green taken from 4 px and blue from 8 px nearer the centre, as chromatic aberration does.
            let mut s = b.clone();
            for y in 0..h {
                for x in 0..w {
                    let towards = |d: i64| (x as i64 + if x < w / 2 { d } else { -d }).clamp(0, w as i64 - 1) as u32;
                    let (g, bl) = (b.get(towards(4), y)[1], b.get(towards(8), y)[2]);
                    let r = b.get(x, y)[0];
                    s.set(x, y, [r, g, bl]);
                }
            }
            smeared.push(view(&s));
        }
        let full = Geometry::new(w, h, 1);
        assert!(!band_drawn(&plain, &full));
        assert!(band_drawn(&banded, &full));
        assert!(band_drawn(&smeared, &full));
        // At half the training resolution too.
        let half: Vec<View> = banded
            .iter()
            .map(|v| View { camera: v.camera, image: downscale(&v.image.iter().flat_map(|p| p.to_le_bytes()).collect::<Vec<u8>>(), w, h, 2), game_time: 0.0, keyframe: 0 })
            .collect();
        assert!(band_drawn(&half, &Geometry::new(w, h, 2)));
    }

    /// Frames narrower than about 1600 px leave room in the band's last row for tiles it never draws: those do not
    /// count against it. (Its pilot cells read through noise here, about 0.7 correlated with the ramp: averaged over
    /// every slot, the undrawn ones would pull that under the 0.5 threshold.)
    #[test]
    fn a_narrow_band_is_found_from_its_drawn_tiles() {
        let (w, h) = (1280u32, 720u32);
        let b = skeleton::BandLayout::new(w, h).expect("band");
        assert!(b.per_row * b.rows > skeleton::BAND_TILES as u32, "this size leaves undrawn slots");
        let mut rng = fastrand::Rng::with_seed(6);
        let views: Vec<View> = (0..3)
            .map(|_| {
                let mut img = RgbaImage::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        img.set(x, y, [rng.u8(..), rng.u8(..), rng.u8(..)]);
                    }
                }
                skeleton::draw(&mut img, &skeleton::Skeleton { time: 1.0, avatar: 3, ..Default::default() }).expect("band");
                let r = b.reserved();
                for y in r.y0..r.y1 {
                    for x in r.x0..r.x1 {
                        let p = img.get(x, y);
                        let red = (p[0] as i32 + rng.i32(-90..=90)).clamp(0, 255) as u8;
                        img.set(x, y, [red, p[1], p[2]]);
                    }
                }
                view(&img)
            })
            .collect();
        assert!(band_drawn(&views, &Geometry::new(w, h, 1)));
    }
}
