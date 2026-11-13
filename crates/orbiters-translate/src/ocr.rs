//! Reading text in a picture: PaddleOCR's PP-OCR models (Apache-2.0) through ONNX Runtime. A DB detector finds each line
//! of text as a rotated box ([`TextBox`]: four corners, any in-plane angle), a CTC recogniser reads each box. The models'
//! pre- and post-processing is written here after PaddleOCR's own (`DetResizeForTest`, `DBPostProcess`,
//! `RecResizeImg`, `CTCLabelDecode`), with no OpenCV: a box is the smallest rectangle around a component of the
//! detector's map, made larger by the same "unclip" distance.

use std::path::{Path, PathBuf};

/// A picture, RGBA, top row first.
#[derive(Clone, Copy)]
pub struct Rgba<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

impl Rgba<'_> {
    fn at(&self, x: i32, y: i32) -> [f32; 3] {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        let i = (y * self.width as usize + x) * 4;
        [self.pixels[i] as f32, self.pixels[i + 1] as f32, self.pixels[i + 2] as f32]
    }

    /// Bilinear sample at a point in pixels (centres at .5).
    pub fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (ax, ay) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let (a, b, c, d) = (self.at(x0, y0), self.at(x0 + 1, y0), self.at(x0, y0 + 1), self.at(x0 + 1, y0 + 1));
        let mut out = [0.0; 3];
        for k in 0..3 {
            out[k] = (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay;
        }
        out
    }
}

/// A line of text found in a picture: the corners of its rotated box in the picture's pixels, top-left, top-right,
/// bottom-right, bottom-left as the text reads (the "top" of a vertical line is its first letter's side).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    pub corners: [[f32; 2]; 4],
    /// The detector's mean confidence over the line.
    pub score: f32,
    /// Letters go down, one under the other (Japanese or Chinese written vertically).
    pub vertical: bool,
}

impl TextBox {
    pub fn centre(&self) -> [f32; 2] {
        let c = &self.corners;
        [(c[0][0] + c[1][0] + c[2][0] + c[3][0]) / 4.0, (c[0][1] + c[1][1] + c[2][1] + c[3][1]) / 4.0]
    }
    /// Along the reading direction, and across it, in pixels.
    pub fn length(&self) -> f32 {
        dist(self.corners[0], self.corners[1])
    }
    pub fn thickness(&self) -> f32 {
        dist(self.corners[0], self.corners[3])
    }
    /// The box's axis-aligned bounds: x0, y0, x1, y1.
    pub fn bounds(&self) -> [f32; 4] {
        let xs = self.corners.map(|c| c[0]);
        let ys = self.corners.map(|c| c[1]);
        [xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max)]
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// A line read.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub boxed: TextBox,
    pub text: String,
    /// Mean probability of the letters read.
    pub confidence: f32,
    /// Which recogniser read it.
    pub reader: Reader,
    /// Each letter, where it lies along the box (0 at its start, 1 at its end) and how sure it is.
    pub glyphs: Vec<(String, [f32; 2], f32)>,
}

/// The recognisers: one reads Chinese, Japanese, English and most Latin-script languages; Korean and Cyrillic have
/// their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reader {
    Main,
    Korean,
    Cyrillic,
}

/// The ONNX Runtime library beside the programs (the voice worker ships it): `ORBITERS_ONNXRUNTIME` names another. In a
/// build's folder, test programs live one folder deeper (`deps`) than the library.
pub fn runtime_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORBITERS_ONNXRUNTIME") {
        return Some(PathBuf::from(p));
    }
    #[cfg(windows)]
    const FILE: &str = "onnxruntime.dll";
    #[cfg(target_vendor = "apple")]
    const FILE: &str = "libonnxruntime.dylib";
    #[cfg(not(any(windows, target_vendor = "apple")))]
    const FILE: &str = "libonnxruntime.so";
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join(FILE), dir.parent()?.join(FILE)].into_iter().find(|p| p.is_file())
}

/// Starts ONNX Runtime once for the process, from [`runtime_path`].
pub fn start_runtime() -> anyhow::Result<()> {
    static STARTED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    STARTED
        .get_or_init(|| {
            let path = runtime_path().filter(|p| p.is_file()).ok_or_else(|| "onnxruntime is not beside the program".to_string())?; // i18n: not shown (log detail)
            std::panic::catch_unwind(|| ort::init_from(path.display().to_string()).commit().map(|_| ()).map_err(|e| e.to_string()))
                .map_err(|_| "ONNX Runtime did not load".to_string())? // i18n: not shown (log detail)
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

/// A session on `threads` threads that never spin while waiting.
pub(crate) fn session(model: &Path, threads: usize) -> anyhow::Result<ort::session::Session> {
    start_runtime()?;
    let e = |e: ort::Error| anyhow::anyhow!("{e}");
    ort::session::Session::builder()
        .map_err(e)?
        .with_intra_threads(threads.max(1))
        .map_err(e)?
        .with_inter_threads(1)
        .map_err(e)?
        // Threads that spin while waiting take CPU time from VRChat for nothing.
        .with_intra_op_spinning(false)
        .map_err(e)?
        .with_inter_op_spinning(false)
        .map_err(e)?
        // Input shapes change from run to run (each batch of lines has its own width): a memory plan per shape only holds memory.
        .with_memory_pattern(false)
        .map_err(e)?
        .commit_from_file(model)
        .map_err(e)
}

/// The detector's settings (PaddleOCR's defaults for PP-OCRv5 mobile det).
#[derive(Clone, Copy, Debug)]
pub struct DetSettings {
    /// A map value above this is text.
    pub thresh: f32,
    /// A line counts when the map's mean over it is above this.
    pub box_thresh: f32,
    pub unclip: f32,
    /// Lines thinner than this many pixels of the detector's input are dropped.
    pub min_size: f32,
}

impl Default for DetSettings {
    fn default() -> Self {
        Self { thresh: 0.3, box_thresh: 0.6, unclip: 1.5, min_size: 3.0 }
    }
}

pub struct Detector {
    session: ort::session::Session,
    pub settings: DetSettings,
    run: ort::session::run_options::RunOptions,
}

/// Runs that give the memory they took back to the system when done (ONNX Runtime keeps an arena otherwise, as large as
/// the largest run so far): the reader lives in the overlay's process, beside the game.
pub(crate) fn lean_run() -> anyhow::Result<ort::session::run_options::RunOptions> {
    let mut run = ort::session::run_options::RunOptions::new().map_err(|e| anyhow::anyhow!("{e}"))?;
    run.add_config_entry("memory.enable_memory_arena_shrinkage", "cpu:0").map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(run)
}

/// What the detector reads: a part of a picture (x, y, width, height) scaled to `size` (both multiples of 32), with
/// places to leave out (filled grey so nothing is found there).
pub struct DetInput<'a> {
    pub picture: Rgba<'a>,
    pub area: [u32; 4],
    pub size: [u32; 2],
    pub hidden: &'a [Polygon],
}

/// A convex polygon in a picture's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Polygon(pub Vec<[f32; 2]>);

impl Polygon {
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let n = self.0.len();
        if n < 3 {
            return false;
        }
        let mut sign = 0.0f32;
        for i in 0..n {
            let (a, b) = (self.0[i], self.0[(i + 1) % n]);
            let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            if cross.abs() < 1e-6 {
                continue;
            }
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
        true
    }
    pub fn bounds(&self) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in &self.0 {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        b
    }
}

/// The detector's input size for an area: its longer side `long` (rounded to 32), the other in proportion.
pub fn det_size(area: [u32; 4], long: u32) -> [u32; 2] {
    let (w, h) = (area[2] as f32, area[3] as f32);
    let s = (long as f32 / w.max(h)).min(1.0);
    let r = |v: f32| (((v * s) / 32.0).round() as u32).max(1) * 32;
    [r(w), r(h)]
}

const DET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const DET_STD: [f32; 3] = [0.229, 0.224, 0.225];

impl Detector {
    pub fn load(model: &Path, threads: usize) -> anyhow::Result<Self> {
        Ok(Self { session: session(model, threads)?, settings: DetSettings::default(), run: lean_run()? })
    }

    /// The lines of text in `input`, in the picture's pixels.
    pub fn detect(&mut self, input: &DetInput) -> anyhow::Result<Vec<TextBox>> {
        let [w, h] = input.size;
        let tensor_data = det_tensor(input);
        let tensor = ort::value::Tensor::from_array(([1usize, 3, h as usize, w as usize], tensor_data)).map_err(|e| anyhow::anyhow!("{e}"))?;
        let outputs = self.session.run_with_options(ort::inputs!["x" => tensor], &self.run).map_err(|e| anyhow::anyhow!("{e}"))?;
        let (shape, map) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("{e}"))?;
        let (mh, mw) = (shape[shape.len() - 2] as usize, shape[shape.len() - 1] as usize);
        let boxes = boxes_from_map(map, mw, mh, &self.settings);
        let [ax, ay, aw, ah] = input.area;
        let (sx, sy) = (aw as f32 / mw as f32, ah as f32 / mh as f32);
        Ok(boxes
            .into_iter()
            .map(|mut b| {
                for c in &mut b.corners {
                    *c = [ax as f32 + c[0] * sx, ay as f32 + c[1] * sy];
                }
                b
            })
            .collect())
    }
}

/// The detector's input tensor: the area box-filtered down to the input's size, BGR planes normalised as PaddleOCR does.
pub fn det_tensor(input: &DetInput) -> Vec<f32> {
    let [w, h] = input.size;
    let (w, h) = (w as usize, h as usize);
    let [ax, ay, aw, ah] = input.area;
    let pic = input.picture;
    let mut out = vec![0f32; 3 * w * h];
    let (sx, sy) = (aw as f32 / w as f32, ah as f32 / h as f32);
    let hidden: Vec<([f32; 4], &Polygon)> = input.hidden.iter().map(|p| (p.bounds(), p)).collect();
    let grey = [(0.5 - DET_MEAN[0]) / DET_STD[0], (0.5 - DET_MEAN[1]) / DET_STD[1], (0.5 - DET_MEAN[2]) / DET_STD[2]];
    let plane = w * h;
    for y in 0..h {
        let (y0, y1) = (ay as f32 + y as f32 * sy, ay as f32 + (y + 1) as f32 * sy);
        let (iy0, iy1) = (y0 as usize, (y1.ceil() as usize).max(y0 as usize + 1).min(pic.height as usize));
        for x in 0..w {
            let (x0, x1) = (ax as f32 + x as f32 * sx, ax as f32 + (x + 1) as f32 * sx);
            let centre = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
            let i = y * w + x;
            if hidden.iter().any(|(b, p)| centre[0] >= b[0] && centre[0] <= b[2] && centre[1] >= b[1] && centre[1] <= b[3] && p.contains(centre)) {
                for c in 0..3 {
                    out[c * plane + i] = grey[c];
                }
                continue;
            }
            let (ix0, ix1) = (x0 as usize, (x1.ceil() as usize).max(x0 as usize + 1).min(pic.width as usize));
            // At most a 4 x 4 footprint is averaged: enough against aliasing at the scales used, and cheap.
            let (stepx, stepy) = (((ix1 - ix0) / 4).max(1), ((iy1 - iy0) / 4).max(1));
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            let mut yy = iy0;
            while yy < iy1 {
                let row = yy * pic.width as usize;
                let mut xx = ix0;
                while xx < ix1 {
                    let p = (row + xx) * 4;
                    sum[0] += pic.pixels[p] as u32;
                    sum[1] += pic.pixels[p + 1] as u32;
                    sum[2] += pic.pixels[p + 2] as u32;
                    n += 1;
                    xx += stepx;
                }
                yy += stepy;
            }
            let n = n.max(1) as f32 * 255.0;
            // BGR, as PaddleOCR's models were trained on OpenCV's pictures.
            let bgr = [sum[2] as f32 / n, sum[1] as f32 / n, sum[0] as f32 / n];
            for c in 0..3 {
                out[c * plane + i] = (bgr[c] - DET_MEAN[c]) / DET_STD[c];
            }
        }
    }
    out
}

/// DB post-processing: the components of the map above `thresh`, each as its smallest rectangle made larger by the
/// unclip distance, kept when the map's mean over the component is above `box_thresh`. In the map's pixels.
pub fn boxes_from_map(map: &[f32], w: usize, h: usize, s: &DetSettings) -> Vec<TextBox> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    let mut rows: Vec<(i32, i32, i32)> = Vec::new();
    for start in 0..w * h {
        if seen[start] || map[start] <= s.thresh {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        rows.clear();
        let (mut sum, mut n) = (0f32, 0usize);
        // Row extremes of the component: its hull's points.
        let mut extremes: std::collections::HashMap<i32, (i32, i32)> = std::collections::HashMap::new();
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            sum += map[i];
            n += 1;
            let e = extremes.entry(y).or_insert((x, x));
            e.0 = e.0.min(x);
            e.1 = e.1.max(x);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && map[j] > s.thresh {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if n < 4 || sum / (n as f32) < s.box_thresh {
            continue;
        }
        let mut points = Vec::with_capacity(extremes.len() * 4);
        for (y, (x0, x1)) in extremes {
            let (y, x0, x1) = (y as f32, x0 as f32, x1 as f32 + 1.0);
            points.extend([[x0, y], [x1, y], [x0, y + 1.0], [x1, y + 1.0]]);
        }
        let Some(rect) = min_area_rect(&points) else { continue };
        if rect.size[0].min(rect.size[1]) < s.min_size {
            continue;
        }
        let area = rect.size[0] * rect.size[1];
        let d = area * s.unclip / (2.0 * (rect.size[0] + rect.size[1]));
        let rect = Rect { size: [rect.size[0] + 2.0 * d, rect.size[1] + 2.0 * d], ..rect };
        if rect.size[0].min(rect.size[1]) < s.min_size + 2.0 {
            continue;
        }
        out.push(rect.text_box(sum / n as f32));
    }
    out
}

/// A rotated rectangle: centre, unit axis of its first side, side lengths along that axis and across it.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub centre: [f32; 2],
    pub axis: [f32; 2],
    pub size: [f32; 2],
}

impl Rect {
    /// As a line of text: the side nearest to horizontal reads left to right, unless the box is much taller than wide
    /// (a vertical line, read top to bottom).
    pub fn text_box(&self, score: f32) -> TextBox {
        // Make the axis the one nearest horizontal, pointing right.
        let (mut u, mut size) = (self.axis, self.size);
        if u[0].abs() < u[1].abs() {
            u = [-u[1], u[0]];
            size = [size[1], size[0]];
        }
        if u[0] < 0.0 {
            u = [-u[0], -u[1]];
        }
        // Down the picture, across the text.
        let v = [-u[1], u[0]];
        let (hw, hh) = (size[0] / 2.0, size[1] / 2.0);
        let c = self.centre;
        let at = |a: f32, b: f32| [c[0] + u[0] * a + v[0] * b, c[1] + u[1] * a + v[1] * b];
        let vertical = size[1] > 1.5 * size[0];
        let corners = if vertical {
            // Read downwards: the reading direction is v; "top" is the side towards -u... rotated as PaddleOCR rotates
            // a tall crop (a quarter turn anticlockwise): the first letter at the top-right corner.
            [at(hw, -hh), at(hw, hh), at(-hw, hh), at(-hw, -hh)]
        } else {
            [at(-hw, -hh), at(hw, -hh), at(hw, hh), at(-hw, hh)]
        };
        TextBox { corners, score, vertical }
    }
}

/// The smallest rectangle around `points` (rotating calipers over their convex hull).
pub fn min_area_rect(points: &[[f32; 2]]) -> Option<Rect> {
    let hull = convex_hull(points);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, Rect)> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let len = dist(a, b);
        if len < 1e-6 {
            continue;
        }
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let v = [-u[1], u[0]];
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &hull {
            let (pu, pv) = (p[0] * u[0] + p[1] * u[1], p[0] * v[0] + p[1] * v[1]);
            u0 = u0.min(pu);
            u1 = u1.max(pu);
            v0 = v0.min(pv);
            v1 = v1.max(pv);
        }
        let area = (u1 - u0) * (v1 - v0);
        if best.as_ref().is_none_or(|(b, _)| area < *b) {
            let (cu, cv) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
            let centre = [u[0] * cu + v[0] * cv, u[1] * cu + v[1] * cv];
            best = Some((area, Rect { centre, axis: u, size: [u1 - u0, v1 - v0] }));
        }
    }
    best.map(|(_, r)| r)
}

fn convex_hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> = points.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<[f32; 2]> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<[f32; 2]> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The recogniser's input height.
pub const REC_HEIGHT: usize = 48;
/// The widest crop read at once (longer lines are squeezed: about 66 letters of a square script).
const REC_MOST_WIDTH: usize = 3200;
/// Crops read in one run of the model.
const REC_BATCH: usize = 8;

pub struct Recogniser {
    session: ort::session::Session,
    /// Letter `i` of the model's output is `letters[i - 1]` (0 is CTC's blank); one past the list is a space.
    letters: Vec<String>,
    pub reader: Reader,
    run: ort::session::run_options::RunOptions,
}

impl Recogniser {
    pub fn load(model: &Path, dictionary: &Path, reader: Reader, threads: usize) -> anyhow::Result<Self> {
        let letters = read_dictionary(&std::fs::read_to_string(dictionary)?);
        anyhow::ensure!(!letters.is_empty(), "no letters in {}", dictionary.display()); // i18n: not shown (log detail)
        Ok(Self { session: session(model, threads)?, letters, reader, run: lean_run()? })
    }

    /// Reads each box of `picture`.
    pub fn read(&mut self, picture: Rgba, boxes: &[TextBox]) -> anyhow::Result<Vec<Line>> {
        let mut order: Vec<usize> = (0..boxes.len()).collect();
        let width = |b: &TextBox| crop_width(b);
        order.sort_by_key(|&i| width(&boxes[i]));
        let mut out: Vec<Option<Line>> = vec![None; boxes.len()];
        for chunk in order.chunks(REC_BATCH) {
            let w = chunk.iter().map(|&i| width(&boxes[i])).max().unwrap_or(16);
            let mut data = vec![0f32; chunk.len() * 3 * REC_HEIGHT * w];
            for (k, &i) in chunk.iter().enumerate() {
                crop_into(picture, &boxes[i], width(&boxes[i]), w, &mut data[k * 3 * REC_HEIGHT * w..(k + 1) * 3 * REC_HEIGHT * w]);
            }
            let tensor = ort::value::Tensor::from_array(([chunk.len(), 3, REC_HEIGHT, w], data)).map_err(|e| anyhow::anyhow!("{e}"))?;
            let outputs = self.session.run_with_options(ort::inputs!["x" => tensor], &self.run).map_err(|e| anyhow::anyhow!("{e}"))?;
            let (shape, probs) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("{e}"))?;
            let (t, c) = (shape[1] as usize, shape[2] as usize);
            for (k, &i) in chunk.iter().enumerate() {
                let (text, confidence, glyphs) = ctc_glyphs(&probs[k * t * c..(k + 1) * t * c], t, c, &self.letters);
                // A step is `w / t` pixels of the batch's crops; this crop is `width(box)` of them.
                let along = |step: f32| (step * w as f32 / t as f32 / width(&boxes[i]) as f32).clamp(0.0, 1.0);
                let glyphs = glyphs.into_iter().map(|g| (g.text, [along(g.steps[0] as f32), along(g.steps[1] as f32 + 1.0)], g.p)).collect();
                out[i] = Some(Line { boxed: boxes[i].clone(), text, confidence, reader: self.reader, glyphs });
            }
        }
        Ok(out.into_iter().flatten().collect())
    }
}

/// The width a box is read at: its length at the recogniser's height.
fn crop_width(b: &TextBox) -> usize {
    let ratio = b.length() / b.thickness().max(1.0);
    ((REC_HEIGHT as f32 * ratio).ceil() as usize).clamp(16, REC_MOST_WIDTH)
}

/// The box sampled straight (a perspective-free warp along its sides) into `out` (3 planes of `REC_HEIGHT` x `stride`),
/// `width` wide, normalised to -1..1 in BGR order; the rest stays 0 (mid grey, as PaddleOCR pads).
fn crop_into(picture: Rgba, b: &TextBox, width: usize, stride: usize, out: &mut [f32]) {
    let [tl, tr, _br, bl] = b.corners;
    let plane = REC_HEIGHT * stride;
    let along = [(tr[0] - tl[0]) / width as f32, (tr[1] - tl[1]) / width as f32];
    let across = [(bl[0] - tl[0]) / REC_HEIGHT as f32, (bl[1] - tl[1]) / REC_HEIGHT as f32];
    // Shrinking more than 2x: average a few samples so thin strokes do not vanish.
    let taps = ((along[0].hypot(along[1]).max(across[0].hypot(across[1]))).ceil() as usize).clamp(1, 3);
    for y in 0..REC_HEIGHT {
        for x in 0..width {
            let mut acc = [0f32; 3];
            for ty in 0..taps {
                for tx in 0..taps {
                    let (fx, fy) = (x as f32 + (tx as f32 + 0.5) / taps as f32, y as f32 + (ty as f32 + 0.5) / taps as f32);
                    let p = [tl[0] + along[0] * fx + across[0] * fy, tl[1] + along[1] * fx + across[1] * fy];
                    let s = picture.sample(p[0], p[1]);
                    for c in 0..3 {
                        acc[c] += s[c];
                    }
                }
            }
            let n = (taps * taps) as f32 * 255.0;
            let bgr = [acc[2] / n, acc[1] / n, acc[0] / n];
            for c in 0..3 {
                out[c * plane + y * stride + x] = (bgr[c] - 0.5) / 0.5;
            }
        }
    }
}

/// Greedy CTC: the most likely letter at each step, repeats and blanks dropped. Also the mean probability of the letters
/// kept.
pub fn ctc_decode(probs: &[f32], steps: usize, classes: usize, letters: &[String]) -> (String, f32) {
    let (text, confidence, _) = ctc_glyphs(probs, steps, classes, letters);
    (text, confidence)
}

/// A letter read and where: the recogniser's steps it was read over (first, last) and how sure it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub text: String,
    pub steps: [usize; 2],
    pub p: f32,
}

/// [`ctc_decode`] with each letter's place along the line, in the recogniser's steps.
pub fn ctc_glyphs(probs: &[f32], steps: usize, classes: usize, letters: &[String]) -> (String, f32, Vec<Glyph>) {
    let mut text = String::new();
    let (mut sum, mut n) = (0f32, 0usize);
    let mut last = 0usize;
    let mut glyphs: Vec<Glyph> = Vec::new();
    for t in 0..steps {
        let row = &probs[t * classes..(t + 1) * classes];
        let (best, p) = row.iter().enumerate().fold((0usize, f32::MIN), |acc, (i, &p)| if p > acc.1 { (i, p) } else { acc });
        if best != 0 && best == last {
            // The same letter held over more steps: it is that wide.
            if let Some(g) = glyphs.last_mut() {
                g.steps[1] = t;
                g.p = g.p.max(p);
            }
        }
        if best != 0 && best != last {
            let l = letters.get(best - 1).cloned().unwrap_or_else(|| " ".to_string());
            text.push_str(&l);
            // Spaces say nothing of how sure the read is: a line of spaces is no read.
            if !l.trim().is_empty() {
                sum += p;
                n += 1;
            }
            glyphs.push(Glyph { text: l, steps: [t, t], p });
        }
        last = best;
    }
    let text = text.trim().to_string();
    let confidence = if n == 0 || text.is_empty() { 0.0 } else { sum / n as f32 };
    (text, confidence, glyphs)
}

/// The letters of a model's dictionary: one a line, or the `character_dict` list of PaddleOCR's `inference.yml`.
pub fn read_dictionary(text: &str) -> Vec<String> {
    let mut lines = text.lines();
    if !text.contains("character_dict:") {
        return lines.map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).collect();
    }
    let mut out = Vec::new();
    for l in lines.by_ref() {
        if l.trim_start().starts_with("character_dict:") {
            break;
        }
    }
    for l in lines {
        let l = l.trim_end_matches('\r');
        let Some(item) = l.strip_prefix("  - ") else { break };
        let item = if item.len() >= 2 && item.starts_with('\'') && item.ends_with('\'') {
            item[1..item.len() - 1].replace("''", "'")
        } else if item.len() >= 2 && item.starts_with('"') && item.ends_with('"') {
            serde_json::from_str::<String>(item).unwrap_or_else(|_| item[1..item.len() - 1].to_string())
        } else {
            item.to_string()
        };
        out.push(item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tilted_line_of_the_map_becomes_one_box_along_it() {
        let (w, h) = (200usize, 120usize);
        let mut map = vec![0f32; w * h];
        // A bar 100 x 10 at 20 degrees, centre (100, 60).
        let (a, c) = (20f32.to_radians(), [100f32, 60.0]);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]);
                let (u, v) = (dx * a.cos() + dy * a.sin(), -dx * a.sin() + dy * a.cos());
                if u.abs() < 50.0 && v.abs() < 5.0 {
                    map[y * w + x] = 0.9;
                }
            }
        }
        let boxes = boxes_from_map(&map, w, h, &DetSettings::default());
        assert_eq!(boxes.len(), 1);
        let b = &boxes[0];
        let angle = (b.corners[1][1] - b.corners[0][1]).atan2(b.corners[1][0] - b.corners[0][0]).to_degrees();
        assert!((angle - 20.0).abs() < 2.0, "reads along the bar: {angle}");
        assert!(!b.vertical);
        let centre = b.centre();
        assert!((centre[0] - 100.0).abs() < 1.5 && (centre[1] - 60.0).abs() < 1.5, "{centre:?}");
        // Unclipped: d = area * 1.5 / perimeter = 1000 * 1.5 / 220 = 6.8 each side.
        assert!((b.length() - 113.6).abs() < 4.0, "{}", b.length());
        assert!((b.thickness() - 23.6).abs() < 4.0, "{}", b.thickness());
    }

    #[test]
    fn a_tall_box_is_a_vertical_line_read_from_its_top() {
        let r = Rect { centre: [50.0, 50.0], axis: [1.0, 0.0], size: [10.0, 60.0] };
        let b = r.text_box(1.0);
        assert!(b.vertical);
        // The first letter is at the top: the reading direction (corner 0 to 1) goes down the picture.
        assert!(b.corners[1][1] > b.corners[0][1] + 50.0, "{:?}", b.corners);
        assert!((b.length() - 60.0).abs() < 0.01 && (b.thickness() - 10.0).abs() < 0.01);
    }

    #[test]
    fn ctc_drops_blanks_and_repeats() {
        let letters: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        // steps: a a blank a b space
        let rows = [[0.1, 0.8, 0.1, 0.0], [0.1, 0.8, 0.1, 0.0], [0.9, 0.05, 0.05, 0.0], [0.1, 0.7, 0.2, 0.0], [0.0, 0.1, 0.9, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let probs: Vec<f32> = rows.iter().flatten().copied().collect();
        let (text, p) = ctc_decode(&probs, rows.len(), 4, &letters);
        assert_eq!(text, "aab");
        assert!(p > 0.7 && p <= 1.0);
    }

    #[test]
    fn the_dictionary_is_read_from_paddles_yaml() {
        let yml = "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - ''''\n  - $\n  - 漢\n  - \"\\\"\"\nOther: 1\n";
        assert_eq!(read_dictionary(yml), vec!["!", "'", "$", "漢", "\""]);
    }

    #[test]
    fn hidden_places_are_grey_for_the_detector() {
        let pixels = vec![255u8; 64 * 64 * 4];
        let pic = Rgba { width: 64, height: 64, pixels: &pixels };
        let hidden = [Polygon(vec![[0.0, 0.0], [32.0, 0.0], [32.0, 64.0], [0.0, 64.0]])];
        let t = det_tensor(&DetInput { picture: pic, area: [0, 0, 64, 64], size: [32, 32], hidden: &hidden });
        let white = (1.0 - DET_MEAN[0]) / DET_STD[0];
        let grey = (0.5 - DET_MEAN[0]) / DET_STD[0];
        assert!((t[0] - grey).abs() < 1e-4, "left half hidden");
        assert!((t[31] - white).abs() < 1e-4, "right half seen");
    }
}
