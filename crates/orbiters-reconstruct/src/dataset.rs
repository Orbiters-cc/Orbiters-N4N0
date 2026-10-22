//! A session as training data: its keyframes (lossless, upright) at the training resolution with their exact
//! cameras, its depth frames' exact samples with their cameras, and the mask of what is not scene (the capture
//! strip and, with a capture kit, the skeleton band). A long session keeps to a memory budget ([`Budget`]): past it,
//! the keyframes and depth frames that look at the world most differently stay ([`spread`]).

use crate::camera::{Camera, Geometry};
use anyhow::{anyhow, bail, ensure, Context as _};
use orbiters_contracts::depth::{self, DepthLayout, SAMPLES_X};
use orbiters_contracts::frame_record::{record_flags, FrameRecord};
use orbiters_contracts::manifest::{depth_file, DEPTH_DIR, KEYFRAMES_DIR};
use orbiters_contracts::{qoi, skeleton, strip};
use orbiters_core::bundle;
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

pub struct Dataset {
    pub dir: PathBuf,
    pub views: Vec<View>,
    pub depth_views: Vec<DepthView>,
    pub width: u32,
    pub height: u32,
    /// How the captured frames map to the training resolution.
    pub geometry: Geometry,
    /// 1 where pixels are scene, 0 over the capture strip and the skeleton band (training resolution).
    pub mask: Vec<f32>,
    /// What the session holds and what loading left out.
    pub loaded: Loaded,
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
    #[cfg(not(windows))]
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
    let distance = |a: usize, b: usize| {
        let ((pa, fa), (pb, fb)) = (poses[a], poses[b]);
        let d = |u: [f32; 3], v: [f32; 3]| ((u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2) + (u[2] - v[2]).powi(2)).sqrt();
        // The chord between unit forward axes is about the angle: 0.35 is 20°.
        d(pa, pb) + d(fa, fb) / 0.35
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
            nearest[i] = nearest[i].min(distance(next, i));
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
    let manifest = bundle::read_manifest(dir).context("reading the session's manifest")?;
    let records: Vec<FrameRecord> = bundle::read_records(dir).context("reading frames.bin")?.into_iter().filter(|r| r.has(record_flags::STRIP_OK) && posed(r)).collect();
    let keyframes: Vec<&FrameRecord> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.keyframe_index >= 0).collect();
    let depth_frames: Vec<&FrameRecord> = records.iter().filter(|r| r.has(record_flags::DEPTH) && r.depth_index >= 0).collect();
    // The session's frame size is its keyframes' most common one: after the stream's resolution changed mid-session,
    // the frames of the other size are left out.
    let size = most_common(keyframes.iter().map(|r| (r.width, r.height)));
    let sized: Vec<&FrameRecord> = keyframes.iter().copied().filter(|r| Some((r.width, r.height)) == size).collect();
    if sized.len() < 2 {
        bail!("This session has {} keyframes: reconstruction needs a few views of the world.", sized.len());
    }
    let (full_w, full_h) = size.expect("two keyframes");
    let geometry = Geometry::at_most(full_w, full_h, max_width);
    let (w, h) = (geometry.width(), geometry.height());
    ensure!(w >= 16 && h >= 16, "keyframes of {full_w}×{full_h} are too small to reconstruct from");
    let mut loaded = Loaded { keyframes: keyframes.len(), depth_frames: depth_frames.len(), keyframes_skipped: keyframes.len() - sized.len(), ..Default::default() };

    // Within the budget, the keyframes that look at the world most differently.
    let keep = usize::try_from(budget.keyframe_bytes / (w as u64 * h as u64 * 4)).unwrap_or(usize::MAX).max(MIN_KEPT);
    let chosen: Vec<&FrameRecord> = spread(&sized.iter().map(|r| pose(r)).collect::<Vec<_>>(), keep).into_iter().map(|i| sized[i]).collect();
    loaded.keyframes_over_budget = sized.len() - chosen.len();
    // Depth frames of the session's size, all on one grid, within the budget too.
    let grid = DepthLayout::new(full_w, full_h).map(|l| Arc::new(DepthGrid::new(&l, &geometry)));
    let depth_sized: Vec<&FrameRecord> = depth_frames.iter().copied().filter(|r| grid.is_some() && (r.width, r.height) == (full_w, full_h)).collect();
    loaded.depth_frames_skipped = depth_frames.len() - depth_sized.len();
    let per_frame = grid.as_ref().map_or(1, |g| g.len().max(1)) as u64;
    let keep = usize::try_from(budget.depth_samples / per_frame).unwrap_or(usize::MAX).max(MIN_KEPT);
    let depth_chosen: Vec<&FrameRecord> = spread(&depth_sized.iter().map(|r| pose(r)).collect::<Vec<_>>(), keep).into_iter().map(|i| depth_sized[i]).collect();
    loaded.depth_frames_over_budget = depth_sized.len() - depth_chosen.len();

    // Decoded on as many threads as loading may use (crate::priority); a frame that cannot be read is left out.
    enum Item {
        View(anyhow::Result<View>),
        Depth(Option<DepthView>),
    }
    let decode = |j: usize| match j.checked_sub(chosen.len()) {
        None => Item::View(load_keyframe(dir, chosen[j], &geometry)),
        Some(d) => Item::Depth(grid.as_ref().and_then(|g| load_depth(dir, depth_chosen[d], &geometry, g))),
    };
    let mut views = Vec::with_capacity(chosen.len());
    let mut depth_views = Vec::with_capacity(depth_chosen.len());
    for item in run_parallel(chosen.len() + depth_chosen.len(), decode, &mut progress) {
        match item {
            Item::View(Ok(v)) => views.push(v),
            Item::View(Err(e)) => {
                loaded.keyframes_skipped += 1;
                loaded.first_problem.get_or_insert_with(|| format!("{e:#}"));
            }
            Item::Depth(Some(d)) => depth_views.push(d),
            Item::Depth(None) => loaded.depth_frames_skipped += 1,
        }
    }
    if views.len() < 2 {
        let why = loaded.first_problem.as_deref().map(|p| format!(" ({p})")).unwrap_or_default();
        bail!("Only {} of this session's {} keyframes could be read{why}: reconstruction needs a few views of the world.", views.len(), loaded.keyframes);
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
    Ok(Dataset { dir: dir.to_path_buf(), views, depth_views, width: w, height: h, geometry, mask, loaded })
}

/// A keyframe at the training resolution with its camera.
fn load_keyframe(dir: &Path, r: &FrameRecord, geometry: &Geometry) -> anyhow::Result<View> {
    Ok(View { camera: geometry.camera(&r.camera_to_world, &r.projection), image: keyframe_image(dir, r.keyframe_index, geometry)?, game_time: r.game_time, keyframe: r.keyframe_index })
}

/// Keyframe `index` of the session in `dir` as captured, at the training resolution of `geometry` (scoring reads the
/// held-out ones again after the keyframes in memory were realigned: crate::aberration).
pub fn keyframe_image(dir: &Path, index: i32, geometry: &Geometry) -> anyhow::Result<Vec<u32>> {
    let path = dir.join(KEYFRAMES_DIR).join(bundle::keyframe_file(index));
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let (w, h) = (geometry.full_width, geometry.full_height);
    // Refused before anything is allocated when its header gives another size.
    let (_, rgba) = qoi::decode_rgba_sized(&data, w, h).map_err(|e| anyhow!("{} (a keyframe of the session's {w}×{h}): {e:?}", path.display()))?;
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

    /// A capture size the training factor does not divide: keyframes, cameras, depth-sample positions and the mask
    /// all have the training size (the remainder cropped), and the pixels are the captured ones box-filtered.
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
