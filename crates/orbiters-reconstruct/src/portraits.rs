//! Faces get extra training: VRChat avatars' faces, the wearer's and friends', are what people look at in a 3D picture.
//!
//! Why the mouth was mushy: measured on the 3D picture Enzo took on 2026-11-03 (METAFES, 8 s, a still avatar on a desktop:
//! its skeleton exactly constant, Viseme and Voice 0 all through). Nothing moved. The two views that show the face closely
//! got about 20 full-resolution steps each of a run of 4700, and over so few what training keeps of a texture follows its
//! contrast: rendered at those very views the gradient energy left was 0.14 of the truth's on the forehead, 0.33 on the
//! mouth, 0.5 on the nose, 0.63 on the eyes. That is why the eyes looked better than the mouth: their contrast is high, the
//! muzzle's brown on brown is not. Three times the steps, spread over the frames, changed nothing there (0.35).
//!
//! What this does: [`find`] runs a face detector over the keyframes of a session short enough to be a picture, and keeps
//! what two or more views agree is one head ([`heads`]). The second half of training then gets up to [`STEPS`] more steps, each
//! on a tile of [`TILE`] pixels over a face in a keyframe at full resolution ([`tile_over`]), the views of a person near the
//! picture's start counting most ([`Picker`]). All in the second half: from the start, with Gaussians still relocating,
//! the same steps melted the face into blur (Enzo's eyes from 27 to 17 dB in their own view), and from a quarter of the run
//! they were worse too. Measured figures: the desktop repository's docs/3d-picture-faces-2026-11-04.md.
//!
//! A session without faces, or a computer without the detector's file: nothing changes, step for step.

use crate::camera::{cross, dot, normalize, Camera};
use crate::dataset::{Dataset, View};
use std::path::{Path, PathBuf};

/// The side of the square tiles a portrait step trains on, in training pixels.
pub const TILE: u32 = 512;

/// The detector's file, beside the worker. Without it (or without ONNX Runtime beside it) a session trains as before.
pub const MODEL_FILE: &str = "faces-v1-int8.onnx";
/// The ONNX Runtime the voice worker already ships.
const RUNTIME_FILE: &str = "onnxruntime.dll";
/// The detector's fixed input: 16:9 frames fit it with a thin grey band above and below (a stride's multiple: YOLOv8's
/// 32 pixels), which takes 60 % of the time a square 640 × 640 input takes for the same boxes.
const INPUT_W: usize = 640;
const INPUT_H: usize = 384;
/// The grey YOLO's letterbox pads with, and what the strip and the skeleton band are painted with.
const PAD: f32 = 114.0 / 255.0;
/// The most steps on tiles over faces in a run ([`crate::train::Settings::portrait_steps`]), [`STEPS_PER_VIEW`] for each
/// view of a face up to this: a pair of boxes that may be a poster's costs little. In a friend's face in the first frame of the
/// 8 s picture of 2026-11-03: 28.9 dB without, 35.9 at 2000, 38.3 at 4000; in Enzo's close view, the whole face 23.2 without,
/// 24.5 at 2000, 25.3 at 3600, 25.8 at 8000 (one run). About 2 s a thousand on an RTX 4080, and whole runs (detector included) took
/// 8 to 12.5 s longer at 4000.
pub const STEPS: u32 = 4000;
pub const STEPS_PER_VIEW: u32 = 400;
/// The seconds over which a later view of a face counts for a factor of e less among the tile steps
/// ([`crate::train::Settings::portrait_moment`]). People move while the camera flies: of a friend's face in the
/// picture's first frame (8 s picture of 2026-11-03), the model without tiles keeps 17.3 dB, with tiles over all its views
/// 18.8, with this 29.1 (SSIM 0.48, 0.64, 0.94; 2000 tile steps). The picture is of that moment; views of them later
/// disagree with it.
pub const MOMENT: f32 = 1.0;
/// Added to a box on each side, of its size: the detector's boxes stop at the chin and the muzzle's tip.
pub const MARGIN: f32 = 0.15;
/// A detection counts from this confidence. Enzo's and his friends' avatars' faces of at least 40 pixels score 0.45 to
/// 0.87 on the 3D pictures of 2026-11-03.
pub const CONFIDENCE: f32 = 0.4;
/// Boxes overlapping by more than this are one face.
const OVERLAP: f32 = 0.5;
/// Keyframes a session may have to be read for faces, however long it lasts, and how long it may last, game seconds, to be
/// read whatever its keyframes: a 3D picture flying around its people keeps 80 to 150 in its 4 to 8 s (the desktop's
/// controller/around.rs). A longer recording is a walk, not a picture: it spends no time on this.
pub const MOST_KEYFRAMES: usize = 80;
pub const PICTURE_S: f32 = 15.0;
/// Keyframes a detection pass reads at most, spread over the picture, the first ones among them. At 60 to 70 ms each, about
/// three seconds: more boxes than [`STEPS`] can use.
pub const MOST_DETECTED: usize = 40;

/// The keyframes a detection pass reads out of `views`: all of them, up to [`MOST_DETECTED`], else evenly spread.
pub fn detected_views(views: usize) -> Vec<usize> {
    if views <= MOST_DETECTED {
        (0..views).collect()
    } else {
        (0..MOST_DETECTED).map(|i| i * views / MOST_DETECTED).collect()
    }
}

/// A face seen in a keyframe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Portrait {
    /// Index into `Dataset::views`.
    pub view: usize,
    /// Its box in the keyframe, training pixels.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// How far it is from the camera, metres (from the box's width, a head being about [`HEAD`] across).
    pub distance: f32,
    /// Its view's game time.
    pub time: f32,
    /// A head known not to have moved: all its views count alike, whenever they were taken ([`Picker`]).
    pub still: bool,
}

/// About how wide a head is, metres, for a first idea of how far a box is from its camera.
pub const HEAD: f32 = 0.25;

impl Portrait {
    /// How often it is drawn: by its size in pixels.
    pub fn weight(&self) -> f32 {
        ((self.x1 - self.x0) * (self.y1 - self.y0)).max(1.0).sqrt()
    }
}

/// Which view of a face each tile step draws: by its box's size (a face seen close has more to teach), a later view for
/// less, over [`MOMENT`] seconds from the first face seen, unless it is known not to have moved. The moment stays with a 3D
/// picture that flies around its people: tiles over their whole bodies in every view that shows them, the moment not applying,
/// took a friend's face in the first frame from 33.8 to 18.3 dB (LPIPS 0.014 to 0.456; 8 s picture of 2026-11-03, three runs)
/// for 1.5 dB more on held-out views, and gained nothing from the side on a still avatar whose views ring it (23.3 to 23.4 dB on
/// its body, within the noise; the desktop repository's docs/3d-picture-angles-2026-11-04.md).
pub struct Picker {
    cumulative: Vec<f32>,
}

impl Picker {
    pub fn new(portraits: &[Portrait], tau: f32) -> Self {
        let first = portraits.iter().map(|p| p.time).fold(f32::INFINITY, f32::min);
        let mut sum = 0.0;
        Self {
            cumulative: portraits
                .iter()
                .map(|p| {
                    let late = if tau > 0.0 && !p.still { (-(p.time - first) / tau).exp() } else { 1.0 };
                    sum += p.weight() * late;
                    sum
                })
                .collect(),
        }
    }

    pub fn pick(&self, rng: &mut fastrand::Rng) -> usize {
        let last = *self.cumulative.last().expect("a portrait");
        let r = rng.f32() * last;
        self.cumulative.partition_point(|c| *c <= r).min(self.cumulative.len() - 1)
    }
}

/// The tile steps for `views` views of faces, at most `most`.
pub fn steps_for(views: usize, most: u32) -> u32 {
    (views as u32).saturating_mul(STEPS_PER_VIEW).min(most)
}

/// How far a run of `base` steps and `extra` tile steps is at step `it` (from 1), as the steps' schedules read it: the
/// first half the same as without the tile steps, the second half stretched over them; without any, exactly what it
/// always was.
pub fn progress(it: u32, base: u32, extra: u32) -> f32 {
    let first = base / 2;
    if extra == 0 || it <= first {
        return (it - 1) as f32 / base as f32;
    }
    let second = (base - first) as f32;
    (first as f32 + (it - 1 - first) as f32 / (second + extra as f32) * second) / base as f32
}

/// The chance of a step of the second half being a tile step, so that about `extra` of them come among its others.
pub fn tile_chance(base: u32, extra: u32) -> f32 {
    extra as f32 / (extra + base - base / 2).max(1) as f32
}

/// A tile of a `width` × `height` keyframe over `p`, at a random place when the box is larger than the tile (each draw
/// shows another part of a big face), around it when smaller: its top-left corner, whole inside the keyframe. The
/// corner runs half a tile past the box's sides, so that its edges are covered about as often as its middle: from a
/// corner kept inside the box, the mouth, at the bottom of the box, got one tile in six.
pub fn tile_over(p: &Portrait, width: u32, height: u32, rng: &mut fastrand::Rng) -> (u32, u32) {
    let along = |lo: f32, hi: f32, size: u32, rng: &mut fastrand::Rng| -> u32 {
        let t = TILE as f32;
        let most = size.saturating_sub(TILE) as f32;
        let corner = if hi - lo > t { lo - t / 2.0 + rng.f32() * (hi - lo) } else { (lo + hi) / 2.0 - t / 2.0 + (rng.f32() - 0.5) * (t - (hi - lo)) * 0.5 };
        corner.clamp(0.0, most) as u32
    };
    (along(p.x0, p.x1, width, rng), along(p.y0, p.y1, height, rng))
}

/// The camera of a tile whose top-left corner is at (`ox`, `oy`): the same rays through the same pixels.
pub fn tile_camera(c: &Camera, ox: u32, oy: u32) -> Camera {
    Camera { r: c.r, t: c.t, fx: c.fx, fy: c.fy, cx: c.cx - ox as f32, cy: c.cy - oy as f32, width: TILE, height: TILE }
}

/// The lab's boxes: `[{"key": keyframe, "boxes": [[x0, y0, x1, y1, ...], ...]}, ...]` in the frames' pixels.
pub fn from_lab_file(path: &Path, data: &Dataset) -> anyhow::Result<Vec<Portrait>> {
    let list: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let scale = data.width as f32 / data.geometry.full_width as f32;
    let mut out = Vec::new();
    for item in list.as_array().into_iter().flatten() {
        let key = item["key"].as_i64().unwrap_or(-1) as i32;
        let Some(view) = data.views.iter().position(|v| v.keyframe == key) else { continue };
        for b in item["boxes"].as_array().into_iter().flatten() {
            let f = |i: usize| b[i].as_f64().unwrap_or(0.0) as f32 * scale;
            out.push(with_margin(view, &data.views[view], [f(0), f(1), f(2), f(3)], data.width, data.height));
        }
    }
    Ok(out)
}

fn with_margin(index: usize, view: &View, b: [f32; 4], width: u32, height: u32) -> Portrait {
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let margin = MARGIN;
    Portrait {
        view: index,
        x0: (b[0] - w * margin).max(0.0),
        y0: (b[1] - h * margin).max(0.0),
        x1: (b[2] + w * margin).min(width as f32),
        y1: (b[3] + h * margin).min(height as f32),
        distance: HEAD * view.camera.fx / w.max(1.0),
        time: view.game_time,
        still: false,
    }
}

/// A box found in a keyframe, in the keyframe's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub view: usize,
    pub b: [f32; 4],
    pub confidence: f32,
}

/// FacesV1 (YOLOv8m, one class, "face": see the repository's THIRD_PARTY_NOTICES.md) through ONNX Runtime: no Python,
/// no Ultralytics code; its output is plain numbers read here.
pub struct Detector {
    session: ort::session::Session,
}

fn beside_worker(file: &str) -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(file))
}

impl Detector {
    /// The detector beside the worker (`ORBITERS_FACES_MODEL` and `ORBITERS_ONNXRUNTIME` name others in the lab). `None`,
    /// said once on the log, when either file is missing or does not load: faces then get no extra training, nothing else changes.
    pub fn load(threads: usize) -> Option<Self> {
        let model = std::env::var_os("ORBITERS_FACES_MODEL").map(PathBuf::from).or_else(|| beside_worker(MODEL_FILE))?;
        let runtime = std::env::var_os("ORBITERS_ONNXRUNTIME").map(PathBuf::from).or_else(|| beside_worker(RUNTIME_FILE))?;
        if !model.is_file() || !runtime.is_file() {
            eprintln!("no face detector: {} or {} is missing", model.display(), runtime.display());
            return None;
        }
        let loaded = std::panic::catch_unwind(|| -> anyhow::Result<ort::session::Session> {
            let _ = ort::init_from(runtime.display().to_string()).commit();
            let session = ort::session::Session::builder()
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_intra_threads(threads.max(1))
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .commit_from_file(&model)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(session)
        });
        match loaded {
            Ok(Ok(session)) => Some(Self { session }),
            Ok(Err(e)) => {
                eprintln!("no face detector: {e}");
                None
            }
            Err(_) => {
                eprintln!("no face detector: ONNX Runtime did not load");
                None
            }
        }
    }

    /// The faces in a keyframe: boxes in its pixels. `mask`: 1 where pixels are scene.
    pub fn detect(&mut self, image: &[u32], mask: &[f32], width: u32, height: u32) -> anyhow::Result<Vec<([f32; 4], f32)>> {
        let (input, scale, top) = letterbox(image, mask, width, height);
        let tensor = ort::value::Tensor::from_array(([1usize, 3, INPUT_H, INPUT_W], input)).map_err(|e| anyhow::anyhow!("{e}"))?;
        let outputs = self.session.run(ort::inputs!["images" => tensor]).map_err(|e| anyhow::anyhow!("{e}"))?;
        let (_, values) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(boxes(values, scale, top))
    }
}

/// The keyframe as the detector's input: RGB planes of 0..1, resampled over each input pixel's footprint, centred
/// between grey bands, the strip and band painted grey. Also the scale and the top band's height.
fn letterbox(image: &[u32], mask: &[f32], width: u32, height: u32) -> (Vec<f32>, f32, f32) {
    let scale = (INPUT_W as f32 / width as f32).min(INPUT_H as f32 / height as f32);
    let (w, h) = (((width as f32 * scale).round() as usize).min(INPUT_W), ((height as f32 * scale).round() as usize).min(INPUT_H));
    let (left, top) = ((INPUT_W - w) / 2, (INPUT_H - h) / 2);
    let mut input = vec![PAD; 3 * INPUT_W * INPUT_H];
    for y in 0..h {
        let (sy0, sy1) = ((y as f32 / scale) as u32, (((y + 1) as f32 / scale).ceil() as u32).min(height));
        for x in 0..w {
            let (sx0, sx1) = ((x as f32 / scale) as u32, (((x + 1) as f32 / scale).ceil() as u32).min(width));
            let mut sum = [0f32; 3];
            let mut n = 0.0f32;
            for sy in sy0..sy1.max(sy0 + 1) {
                for sx in sx0..sx1.max(sx0 + 1) {
                    let i = (sy.min(height - 1) * width + sx.min(width - 1)) as usize;
                    n += 1.0;
                    if mask[i] > 0.5 {
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += crate::dataset::channel(image[i], c);
                        }
                    } else {
                        for s in &mut sum {
                            *s += PAD;
                        }
                    }
                }
            }
            for c in 0..3 {
                input[(c * INPUT_H + y + top) * INPUT_W + x + left] = sum[c] / n;
            }
        }
    }
    (input, scale, top as f32)
}

/// The detector's output (5 rows: centre x, centre y, width, height, confidence, in its input's pixels) as boxes in the
/// keyframe's pixels, the weaker of two overlapping ones dropped.
fn boxes(values: &[f32], scale: f32, top: f32) -> Vec<([f32; 4], f32)> {
    let n = values.len() / 5;
    let at = |row: usize, i: usize| values[row * n + i];
    let mut found: Vec<([f32; 4], f32)> = (0..n)
        .filter(|&i| at(4, i) >= CONFIDENCE)
        .map(|i| {
            let (cx, cy, w, h) = (at(0, i), at(1, i) - top, at(2, i), at(3, i));
            ([(cx - w / 2.0) / scale, (cy - h / 2.0) / scale, (cx + w / 2.0) / scale, (cy + h / 2.0) / scale], at(4, i))
        })
        .collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<([f32; 4], f32)> = Vec::new();
    for f in found {
        if kept.iter().all(|k| overlap(&k.0, &f.0) <= OVERLAP) {
            kept.push(f);
        }
    }
    kept
}

fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = w * h;
    inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter).max(1e-6)
}

/// Two detections are of one head when their rays pass within this of each other (metres) somewhere near where their
/// boxes' sizes put them: a person moving during the flight stays within it. Generous for a static world: what it must
/// tell apart is a person from a detector's one-off mistake, and the watermark a world pins to the screen, whose ray
/// sweeps with the camera.
const SAME_HEAD: f32 = 0.3;

/// The ray through a detection's centre: from the camera's place, unit direction.
fn ray(camera: &Camera, b: &[f32; 4]) -> ([f32; 3], [f32; 3]) {
    let origin = camera.position();
    let through = camera.unproject((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0, 1.0);
    (origin, normalize([through[0] - origin[0], through[1] - origin[1], through[2] - origin[2]]))
}

/// Whether two detections can be one head. Their rays' closest points, along each ray, must lie within a factor of 2.5 of
/// what the box's size says (a head being 0.25 m give or take; an anime head is larger, a child's smaller) and within
/// [`SAME_HEAD`] of each other. Rays nearly parallel (the same place, a zoom) leave the distance to the sizes alone.
pub fn one_head(a: (&Camera, &[f32; 4]), b: (&Camera, &[f32; 4])) -> bool {
    let distance = |c: &Camera, d: &[f32; 4]| HEAD * c.fx / (d[2] - d[0]).max(1.0);
    let (da, db) = (distance(a.0, a.1), distance(b.0, b.1));
    let ((oa, ua), (ob, ub)) = (ray(a.0, a.1), ray(b.0, b.1));
    let w = [oa[0] - ob[0], oa[1] - ob[1], oa[2] - ob[2]];
    let cos = dot(ua, ub);
    let parallel = cross(ua, ub);
    let (ta, tb) = if dot(parallel, parallel) < 1e-5 {
        (da, db)
    } else {
        let denominator = 1.0 - cos * cos;
        ((cos * dot(ub, w) - dot(ua, w)) / denominator, (dot(ub, w) - cos * dot(ua, w)) / denominator)
    };
    let near = |t: f32, d: f32| t >= d / 2.5 && t <= d * 2.5;
    if !near(ta, da) || !near(tb, db) {
        return false;
    }
    let (pa, pb) = ([oa[0] + ua[0] * ta, oa[1] + ua[1] * ta, oa[2] + ua[2] * ta], [ob[0] + ub[0] * tb, ob[1] + ub[1] * tb, ob[2] + ub[2] * tb]);
    let gap = [pa[0] - pb[0], pa[1] - pb[1], pa[2] - pb[2]];
    dot(gap, gap).sqrt() <= SAME_HEAD
}

/// The detections of something seen at least twice, in groups (one per head). What only one keyframe shows is a mistake
/// or too small to matter.
pub fn heads(detections: &[Detection], cameras: &[Camera]) -> Vec<Vec<Detection>> {
    let n = detections.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = i;
        while parent[c] != r {
            let next = parent[c];
            parent[c] = r;
            c = next;
        }
        r
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&detections[i], &detections[j]);
            if a.view != b.view && one_head((&cameras[a.view], &a.b), (&cameras[b.view], &b.b)) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<Detection>> = Default::default();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(detections[i]);
    }
    groups.into_values().filter(|g| g.len() >= 2).collect()
}

/// `orbiters-reconstruct --find-faces <keyframe.qoi>`: one line of JSON about a frame being recorded, for the 3D picture
/// that is about to fly (the desktop's controller/burst.rs): the frame's size and projection from its strip, the faces' boxes in
/// its pixels, the hips of the avatar wearing the capture kit from its band. The whole of it takes under a second, most of
/// it loading the detector.
pub fn find_faces_cli(args: &[String]) -> anyhow::Result<()> {
    use orbiters_contracts::image::{ImageView, PixelOrder};
    use orbiters_contracts::{qoi, skeleton, strip};
    let path = args.first().ok_or_else(|| anyhow::anyhow!("usage: --find-faces <keyframe.qoi>"))?;
    let (header, rgba) = qoi::decode_rgba(&std::fs::read(path)?).map_err(|e| anyhow::anyhow!("{path}: {e:?}"))?;
    let (width, height) = (header.width, header.height);
    let view = ImageView::try_new(width, height, width as usize * 4, &rgba, PixelOrder::Rgba).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let decoded = strip::decode(&view).map_err(|e| anyhow::anyhow!("{path}: {e:?}"))?;
    let hips = skeleton::decode(&view, decoded.flipped).and_then(|b| b.result.ok()).map(|s| s.hips);
    let image: Vec<u32> = rgba.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect();
    let mut mask = vec![1.0f32; (width * height) as usize];
    let mut cut = |r: orbiters_contracts::image::Rect| {
        for y in r.y0..r.y1.min(height) {
            for x in r.x0..r.x1.min(width) {
                mask[(y * width + x) as usize] = 0.0;
            }
        }
    };
    if let Some(l) = strip::StripLayout::new(width, height) {
        cut(l.reserved);
    }
    if let Some(b) = skeleton::BandLayout::new(width, height) {
        cut(b.reserved());
    }
    let threads = crate::priority::loading_threads().min(crate::priority::VRCHAT_LOADING_THREADS);
    let boxes = match Detector::load(threads) {
        Some(mut detector) => detector.detect(&image, &mask, width, height)?,
        None => Vec::new(),
    };
    let list: Vec<[f32; 5]> = boxes.iter().map(|(b, c)| [b[0], b[1], b[2], b[3], *c]).collect();
    println!(
        "{}",
        serde_json::json!({
            "width": width,
            "height": height,
            "p00": decoded.payload.projection[0],
            "p11": decoded.payload.projection[1],
            "boxes": list,
            "hips": hips,
        })
    );
    Ok(())
}

/// A head's place, from its boxes: the middle, axis by axis, of where each pair of them comes closest (their rays when
/// they part enough to say, else where the boxes' sizes put them).
fn centre(group: &[Detection], cameras: &[Camera]) -> Option<[f32; 3]> {
    let mut points: Vec<[f32; 3]> = Vec::new();
    for (i, a) in group.iter().enumerate() {
        for b in &group[i + 1..] {
            let (ca, cb) = (&cameras[a.view], &cameras[b.view]);
            let ((oa, ua), (ob, ub)) = (ray(ca, &a.b), ray(cb, &b.b));
            let (w, cos) = ([oa[0] - ob[0], oa[1] - ob[1], oa[2] - ob[2]], dot(ua, ub));
            let denominator = 1.0 - cos * cos;
            if denominator < 1e-3 {
                continue;
            }
            let ta = (cos * dot(ub, w) - dot(ua, w)) / denominator;
            let tb = (dot(ub, w) - cos * dot(ua, w)) / denominator;
            points.push(std::array::from_fn(|k| (oa[k] + ua[k] * ta + ob[k] + ub[k] * tb) / 2.0));
        }
    }
    if points.is_empty() {
        return None;
    }
    Some(std::array::from_fn(|k| {
        let mut values: Vec<f32> = points.iter().map(|p| p[k]).collect();
        values.sort_by(f32::total_cmp);
        values[values.len() / 2]
    }))
}

/// Where the head of the avatar wearing the capture kit stands, when its skeleton band says it never moved through the
/// session: hips within [`STILL_M`] and the head bone within [`STILL_DEG`]. Measured: Enzo's desktop avatar in the picture of
/// 2026-11-03 moved by nothing at all in 72 skeleton frames, and there the moment must not cost a view that came 3 s into
/// the flight its steps: the face in that view went from 22.5 dB with the moment to 24.4 without. The head stands above the
/// hips by what the rest of the eye height leaves ([`crate::ground::HIPS_PER_EYE_HEIGHT`]).
fn wearer_still(session: &Path) -> Option<[f32; 3]> {
    use orbiters_contracts::performance::{self, skeleton_frames};
    /// `HumanBodyBones.Head`.
    const HEAD_BONE: usize = 10;
    let frames = skeleton_frames::read(&std::fs::read(session.join(performance::SKELETON_FILE)).ok()?).ok()?;
    let first = frames.first()?;
    let (hips, head) = (first.hips, first.rotations[HEAD_BONE]?);
    let cos = (STILL_DEG.to_radians() / 2.0).cos();
    let still = frames.len() >= 3
        && frames.iter().all(|f| {
            let moved = (0..3).map(|k| (f.hips[k] - hips[k]).powi(2)).sum::<f32>().sqrt();
            let turned = f.rotations[HEAD_BONE].is_none_or(|r| (0..4).map(|k| r[k] * head[k]).sum::<f32>().abs() >= cos);
            moved <= STILL_M && turned
        });
    if !still {
        return None;
    }
    let height = crate::ground::eye_height(session, None).unwrap_or(crate::ground::EYE_HEIGHT);
    Some([hips[0], hips[1] + (1.0 - crate::ground::HIPS_PER_EYE_HEIGHT) * height, hips[2]])
}

/// How far the hips may wander, metres, and the head bone turn, degrees, for an avatar to count as still.
const STILL_M: f32 = 0.03;
const STILL_DEG: f32 = 3.0;
/// A head within this of the still wearer's is the wearer's, metres (its place comes from boxes on a muzzle, and from
/// an eye height rather than bones).
const WEARER_M: f32 = 0.5;

/// What a detection pass found, for the log and the report.
#[derive(Debug, Default)]
pub struct Found {
    pub portraits: Vec<Portrait>,
    pub keyframes: usize,
    pub detections: usize,
    pub heads: usize,
    /// Of them, known not to have moved.
    pub still: usize,
    pub seconds: f32,
    /// The detector's file and runtime were there.
    pub ran: bool,
}

/// The faces of a session's keyframes (see the module's comment).
pub fn find(data: &Dataset) -> Found {
    let mut found = Found::default();
    let times = data.views.iter().map(|v| v.game_time);
    let span = times.clone().fold(f32::MIN, f32::max) - times.fold(f32::MAX, f32::min);
    if data.views.is_empty() || (data.views.len() > MOST_KEYFRAMES && span > PICTURE_S) {
        return found;
    }
    let start = std::time::Instant::now();
    let Some(mut detector) = Detector::load(crate::priority::loading_threads()) else { return found };
    found.ran = true;
    let mut detections = Vec::new();
    for i in detected_views(data.views.len()) {
        let v = &data.views[i];
        match detector.detect(&v.image, &data.mask, data.width, data.height) {
            Ok(list) => detections.extend(list.into_iter().map(|(b, confidence)| Detection { view: i, b, confidence })),
            Err(e) => {
                eprintln!("the face detector failed: {e}");
                return Found::default();
            }
        }
    }
    let cameras: Vec<Camera> = data.views.iter().map(|v| v.camera).collect();
    let groups = heads(&detections, &cameras);
    found.keyframes = data.views.len();
    found.detections = detections.len();
    found.heads = groups.len();
    let wearer = wearer_still(&data.dir);
    for group in &groups {
        let still = wearer.is_some_and(|w| centre(group, &cameras).is_some_and(|c| (0..3).map(|k| (c[k] - w[k]).powi(2)).sum::<f32>().sqrt() <= WEARER_M));
        found.still += still as usize;
        for d in group {
            found.portraits.push(Portrait { still, ..with_margin(d.view, &data.views[d.view], d.b, data.width, data.height) });
        }
    }
    found.seconds = start.elapsed().as_secs_f32();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(eye: [f32; 3], at: [f32; 3]) -> Camera {
        Camera::look(eye, [at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]], [0.0, 1.0, 0.0], 0.8, 1920, 1080)
    }

    fn box_of(c: &Camera, p: [f32; 3], head: f32) -> [f32; 4] {
        let (u, v, z) = c.project(p).expect("in front");
        let half = head * c.fx / z / 2.0;
        [u - half, v - half, u + half, v + half]
    }

    #[test]
    fn a_head_seen_from_two_sides_is_one_a_mistake_is_not() {
        let head = [0.0, 1.7, 0.0];
        let (a, b, c) = (camera([2.0, 1.7, 0.0], head), camera([0.0, 1.7, 2.0], head), camera([0.0, 1.7, -3.0], [5.0, 3.0, 0.0]));
        let d = vec![
            Detection { view: 0, b: box_of(&a, head, 0.25), confidence: 0.8 },
            Detection { view: 1, b: box_of(&b, head, 0.25), confidence: 0.8 },
            Detection { view: 2, b: [100.0, 100.0, 180.0, 180.0], confidence: 0.6 },
        ];
        let groups = heads(&d, &[a, b, c]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn tile_steps_leave_the_first_half_alone_and_stretch_the_second() {
        let (base, extra) = (4700u32, 2000u32);
        for it in [1, 2, 1000, base / 2] {
            assert_eq!(progress(it, base, extra), (it - 1) as f32 / base as f32);
        }
        for it in [1, 777, 4700] {
            assert_eq!(progress(it, base, 0), (it - 1) as f32 / base as f32, "no faces: as ever");
        }
        let mut last = 0.0;
        for it in 1..=base + extra {
            let p = progress(it, base, extra);
            assert!(p >= last && p < 1.0, "{it}: {p}");
            last = p;
        }
        assert!(last > 0.999, "{last}");
        assert_eq!(tile_chance(base, 0), 0.0);
        assert!((tile_chance(base, extra) * (base / 2 + extra) as f32 - extra as f32).abs() < 1.0);
    }

    #[test]
    fn a_long_picture_is_read_in_part_from_its_start() {
        assert_eq!(detected_views(3), vec![0, 1, 2]);
        assert_eq!(detected_views(MOST_DETECTED), (0..MOST_DETECTED).collect::<Vec<_>>());
        let picks = detected_views(120);
        assert_eq!(picks.len(), MOST_DETECTED);
        assert_eq!(picks[0], 0);
        assert!(picks.windows(2).all(|w| w[1] - w[0] == 3), "{picks:?}");
    }

    #[test]
    fn a_few_views_get_few_steps() {
        assert_eq!(steps_for(2, STEPS), 800);
        assert_eq!(steps_for(9, STEPS), 3600);
        assert_eq!(steps_for(40, STEPS), STEPS);
        assert_eq!(steps_for(9, 0), 0);
    }

    #[test]
    fn without_the_detectors_files_there_is_no_detector() {
        // A test's executable has no model beside it: nothing fails, nothing is found.
        std::env::remove_var("ORBITERS_FACES_MODEL");
        assert!(Detector::load(1).is_none());
    }

    #[test]
    fn a_later_view_of_a_face_counts_for_less_a_nearer_one_for_more() {
        let at = |w: f32, time: f32| Portrait { view: 0, x0: 0.0, y0: 0.0, x1: w, y1: w, distance: 1.0, time, still: false };
        let share = |portraits: &[Portrait], moment: f32| {
            let picker = Picker::new(portraits, moment);
            let mut rng = fastrand::Rng::with_seed(3);
            (0..20_000).filter(|_| picker.pick(&mut rng) == 0).count() as f32 / 20_000.0
        };
        assert!((share(&[at(100.0, 0.0), at(100.0, 0.0)], MOMENT) - 0.5).abs() < 0.02);
        // e against 1 for a second later; three to one for a box three times as wide.
        assert!((share(&[at(100.0, 0.0), at(100.0, 1.0)], 1.0) - 0.731).abs() < 0.02);
        assert!((share(&[at(300.0, 0.0), at(100.0, 0.0)], 1.0) - 0.75).abs() < 0.02);
        assert!((share(&[at(100.0, 0.0), at(100.0, 5.0)], 0.0) - 0.5).abs() < 0.02, "a moment of 0: all alike");
        assert!(
            (share(&[Portrait { still: true, ..at(100.0, 0.0) }, Portrait { still: true, ..at(100.0, 5.0) }], 1.0) - 0.5).abs() < 0.02,
            "a still head: all alike"
        );
    }

    #[test]
    fn a_tile_stays_inside_the_keyframe_and_reaches_a_big_faces_edges() {
        let p = Portrait { view: 0, x0: 610.0, y0: 113.0, x1: 1340.0, y1: 1080.0, distance: 0.4, time: 0.0, still: false };
        let mut rng = fastrand::Rng::with_seed(1);
        let mut mouth = 0;
        let draws = 4000;
        for _ in 0..draws {
            let (x, y) = tile_over(&p, 1920, 1080, &mut rng);
            assert!(x + TILE <= 1920 && y + TILE <= 1080);
            if y + TILE >= 1000 && y <= 895 {
                mouth += 1;
            }
        }
        assert!(mouth * 3 > draws, "the bottom of the box in {mouth} of {draws} tiles");
    }

    #[test]
    fn a_tiles_camera_sees_the_same_rays_through_its_pixels() {
        let c = camera([0.0, 1.7, 0.0], [0.0, 1.7, 2.0]);
        let t = tile_camera(&c, 100, 200);
        let p = [0.3, 1.8, 2.0];
        let (u, v, _) = c.project(p).unwrap();
        let (tu, tv, _) = t.project(p).unwrap();
        assert!((tu - (u - 100.0)).abs() < 1e-3 && (tv - (v - 200.0)).abs() < 1e-3);
    }

    #[test]
    fn the_detectors_output_becomes_boxes_in_the_keyframes_pixels() {
        // Two rows of the output: one over the confidence, one under, at a 1920 × 1080 keyframe's scale of a third with
        // a band of 12 on top.
        let (cx, cy, w, h, c) = ([320.0, 100.0], [192.0, 100.0], [64.0, 10.0], [64.0, 10.0], [0.8, 0.1]);
        let values: Vec<f32> = [cx, cy, w, h, c].concat();
        let b = boxes(&values, 1.0 / 3.0, 12.0);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, [864.0, 444.0, 1056.0, 636.0]);
    }

    #[test]
    fn overlapping_boxes_are_one_face_the_stronger() {
        let (cx, cy, w, h, c) = ([100.0, 104.0, 300.0], [100.0, 100.0, 100.0], [40.0, 40.0, 40.0], [40.0, 40.0, 40.0], [0.6, 0.9, 0.7]);
        let values: Vec<f32> = [cx, cy, w, h, c].concat();
        let b = boxes(&values, 1.0, 0.0);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].1, 0.9);
    }
}
