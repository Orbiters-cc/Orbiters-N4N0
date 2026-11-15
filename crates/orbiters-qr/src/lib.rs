//! Reads the QR codes in a picture of VRChat's camera (Enzo, K-qr-detect, 2026-11-15). People in VRChat post styled
//! codes: light modules on a dark poster (his shot1, white on purple; shot2, cyan to magenta on black), a logo or a
//! picture in the middle, colours, seen at an angle and small. `rqrr` alone reads dark modules on a light ground only, so
//! every inverted code was silently missed. [`read`] tries a few ways of telling a code's dark modules from its light ones
//! ([`PASSES`]), cheapest first, and stops at the first that reads a code. When none does, it says whether a code is in
//! view all the same ([`Glimpse`]): cut by the picture's edge (nothing can read it), or whole but not readable yet.
//!
//! Cost (measured by `tests::cost`, release build, one core): see the report `done6-qr.md`; a look at a picture with no
//! code runs every pass and is the dearest case.

/// A grey copy is at most this wide (and as many rows as keep the picture's shape).
pub const MOST_WIDTH: usize = 960;

/// A frame's pixels as Spout lends them: 4 bytes a pixel, `stride` bytes a row.
pub struct Pixels<'a> {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub data: &'a [u8],
    /// Blue first (BGRA) rather than red.
    pub bgra: bool,
}

/// Which part of a frame a look decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The whole picture, shrunk to [`MOST_WIDTH`].
    Whole,
    /// Its middle, half as wide and high, at full detail (shrunk only past [`MOST_WIDTH`]).
    Middle,
}

/// A grey copy of `view` of `frame`: (width, height, one byte a pixel). Each grey pixel is the mean of the square of
/// pixels it covers (Rec. 601 luma of the sRGB codes, which is what a QR decoder thresholds anyway).
pub fn grey(frame: &Pixels<'_>, view: View) -> (usize, usize, Vec<u8>) {
    let (x0, y0, w, h) = match view {
        View::Whole => (0, 0, frame.width, frame.height),
        View::Middle => (frame.width / 4, frame.height / 4, frame.width / 2, frame.height / 2),
    };
    let step = w.div_ceil(MOST_WIDTH).max(1);
    let (gw, gh) = (w / step, h / step);
    let (r, b) = if frame.bgra { (2, 0) } else { (0, 2) };
    let mut out = vec![0u8; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut sum = 0u32;
            for dy in 0..step {
                let row = (y0 + gy * step + dy) * frame.stride;
                for dx in 0..step {
                    let i = row + (x0 + gx * step + dx) * 4;
                    let p = &frame.data[i..i + 4];
                    sum += 77 * p[r] as u32 + 150 * p[1] as u32 + 29 * p[b] as u32;
                }
            }
            out[gy * gw + gx] = (sum / (256 * (step * step) as u32)) as u8;
        }
    }
    (gw, gh, out)
}

/// A code in view that could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glimpse {
    /// Part of it is outside the picture: nothing can read it until all of it is in.
    Cut,
    /// All of it seems in the picture, but it could not be read (too small, blurred, at too steep an angle).
    Unclear,
}

/// What one look at a picture found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The texts of the codes read.
    pub codes: Vec<String>,
    /// None read: whether a code is in view all the same.
    pub glimpse: Option<Glimpse>,
    /// And where: a closer look there may read it ([`closer`]).
    pub near: Option<Near>,
}

/// A way of telling dark modules from light ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// `rqrr`'s own: each pixel against the running mean of its row (dark modules on a light ground).
    Rows,
    /// The same on the picture turned negative: light modules on a dark ground (most styled posters).
    RowsInverted,
    /// Each pixel against the mean of the square around it, flat areas left out: lighting that changes across the code
    /// (a poster at an angle, a gradient), low contrast, colours.
    Square,
    /// [`Pass::Square`]'s light modules taken as the dark ones.
    SquareInverted,
}

/// The passes, in the order [`read`] tries them.
pub const PASSES: [Pass; 4] = [Pass::Rows, Pass::RowsInverted, Pass::Square, Pass::SquareInverted];

/// The codes in a grey picture (`width` × `height`, one byte a pixel), and when there are none whether one is in view.
/// `edges`: the picture's sides are the camera picture's own (a whole frame, not its middle), so a code running into them
/// is cut.
pub fn read(width: usize, height: usize, grey: &[u8], edges: bool) -> Reading {
    read_with(width, height, grey, edges, &PASSES)
}

/// [`read`] with only `passes` tried (comparisons).
pub fn read_with(width: usize, height: usize, grey: &[u8], edges: bool, passes: &[Pass]) -> Reading {
    if width < 21 || height < 21 || grey.len() < width * height {
        return Reading::default();
    }
    // The prepared picture's type is not nameable outside `rqrr`.
    macro_rules! decode {
        ($prepared:expr) => {{
            let mut prepared = $prepared;
            prepared.detect_grids().into_iter().filter_map(|grid| grid.decode().ok().map(|(_, text)| text)).collect::<Vec<String>>()
        }};
    }
    let mut classes: Option<Vec<i8>> = None;
    for &pass in passes {
        let codes = match pass {
            Pass::Rows => decode!(rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| grey[y * width + x])),
            Pass::RowsInverted => decode!(rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| 255 - grey[y * width + x])),
            Pass::Square | Pass::SquareInverted => {
                let c = classes.get_or_insert_with(|| classify(width, height, grey));
                let dark = if pass == Pass::Square { -1 } else { 1 };
                decode!(rqrr::PreparedImage::prepare_from_bitmap(width, height, |x, y| c[y * width + x] == dark))
            }
        };
        if !codes.is_empty() {
            return Reading { codes, glimpse: None, near: None };
        }
    }
    let classes = classes.unwrap_or_else(|| classify(width, height, grey));
    let all: Vec<(i8, Vec<Finder>)> = [-1i8, 1].into_iter().map(|dark| (dark, finders(width, height, &classes, dark))).collect();
    // What the person is told rests on finders clearly seen (on two rows or more, a pixel and a half a module or more):
    // a world's specks are not a code.
    let clear = |f: &&Finder| f.hits >= 2 && f.module >= 1.5;
    let seen = all.iter().filter_map(|(dark, fs)| glimpse(width, height, &classes, *dark, edges, &fs.iter().filter(clear).copied().collect::<Vec<_>>())).min_by_key(|(g, _)| *g as u8);
    // Where to look closer may rest on any: a look wasted costs a third of a second, a small code missed costs the code.
    let near = seen.map(|(_, near)| near).or_else(|| all.iter().find_map(|(dark, fs)| glimpse(width, height, &classes, *dark, edges, fs)).map(|(_, near)| near));
    Reading { codes: Vec::new(), glimpse: seen.map(|(g, _)| g), near }
}

/// Where in a picture a code was glimpsed: the rectangle its finders and what lies between them take, and the size of
/// one module there, in that picture's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Near {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub module: f32,
}

/// A grey copy of the part of `frame` around `near` (in the frame's pixels), at the frame's full detail, twice as large
/// again when its modules are still under four pixels: a small code glimpsed in a shrunk copy is read there. At most
/// [`MOST_WIDTH`] wide and high.
pub fn closer(frame: &Pixels<'_>, near: &Near) -> (usize, usize, Vec<u8>) {
    // The code reaches past its finders by about four modules; a margin of as many again.
    let margin = near.module * 8.0;
    let x0 = (near.x0 - margin).max(0.0) as usize;
    let y0 = (near.y0 - margin).max(0.0) as usize;
    let x1 = ((near.x1 + margin).max(0.0) as usize).min(frame.width);
    let y1 = ((near.y1 + margin).max(0.0) as usize).min(frame.height);
    if x1 <= x0 + 21 || y1 <= y0 + 21 {
        return (0, 0, Vec::new());
    }
    let module = near.module;
    let (w, h) = (x1 - x0, y1 - y0);
    // Enlarged twice (modules under four pixels), or shrunk to fit.
    let scale = (if module < 4.0 { 2.0f32 } else { 1.0 }).min(MOST_WIDTH as f32 / w.max(h) as f32);
    let (gw, gh) = (((w as f32 * scale) as usize).max(1), ((h as f32 * scale) as usize).max(1));
    let (r, b) = if frame.bgra { (2, 0) } else { (0, 2) };
    let luma = |x: usize, y: usize| -> f32 {
        let i = y * frame.stride + x * 4;
        let p = &frame.data[i..i + 4];
        (77 * p[r] as u32 + 150 * p[1] as u32 + 29 * p[b] as u32) as f32 / 256.0
    };
    let mut out = vec![0u8; gw * gh];
    for gy in 0..gh {
        let fy = ((gy as f32 + 0.5) / scale - 0.5).clamp(0.0, (h - 1) as f32);
        let (iy, ty) = (fy as usize, fy.fract());
        let iy1 = (iy + 1).min(h - 1);
        for gx in 0..gw {
            let fx = ((gx as f32 + 0.5) / scale - 0.5).clamp(0.0, (w - 1) as f32);
            let (ix, tx) = (fx as usize, fx.fract());
            let ix1 = (ix + 1).min(w - 1);
            let v = luma(x0 + ix, y0 + iy) * (1.0 - tx) * (1.0 - ty)
                + luma(x0 + ix1, y0 + iy) * tx * (1.0 - ty)
                + luma(x0 + ix, y0 + iy1) * (1.0 - tx) * ty
                + luma(x0 + ix1, y0 + iy1) * tx * ty;
            out[gy * gw + gx] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    (gw, gh, out)
}


/// Each pixel against the mean of the square around it: -1 darker, 1 lighter, 0 neither (flat, or near the mean).
fn classify(w: usize, h: usize, grey: &[u8]) -> Vec<i8> {
    // A square about a tenth of the picture's height: wider than a finder pattern of any code that fits the picture.
    let r = (w.min(h) / 10).clamp(8, 48);
    let stride = w + 1;
    let mut sum = vec![0u32; stride * (h + 1)];
    let mut squares = vec![0u64; stride * (h + 1)];
    for y in 0..h {
        let (mut row, mut row_sq) = (0u32, 0u64);
        for x in 0..w {
            let v = grey[y * w + x] as u32;
            row += v;
            row_sq += (v * v) as u64;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
            squares[(y + 1) * stride + x + 1] = squares[y * stride + x + 1] + row_sq;
        }
    }
    let mut out = vec![0i8; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let n = ((x1 - x0) * (y1 - y0)) as f32;
            let s = (sum[y1 * stride + x1] + sum[y0 * stride + x0] - sum[y0 * stride + x1] - sum[y1 * stride + x0]) as f32;
            let sq = (squares[y1 * stride + x1] + squares[y0 * stride + x0] - squares[y0 * stride + x1] - squares[y1 * stride + x0]) as f32;
            let mean = s / n;
            let deviation = (sq / n - mean * mean).max(0.0).sqrt();
            // A flat area (a wall, the sky) is neither: its noise must not make finder-like specks.
            if deviation < 10.0 {
                continue;
            }
            let v = grey[y * w + x] as f32;
            let margin = (deviation * 0.1).max(2.0);
            out[y * w + x] = if v < mean - margin {
                -1
            } else if v > mean + margin {
                1
            } else {
                0
            };
        }
    }
    out
}

/// A finder pattern (one of a code's three corner squares): its middle and the size of one module, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finder {
    pub x: f32,
    pub y: f32,
    pub module: f32,
    /// Rows it was found on.
    pub hits: u32,
}

/// The finder patterns whose dark parts are the pixels of class `dark` (-1: dark modules; 1: light ones, a code in negative).
pub fn finders(w: usize, h: usize, classes: &[i8], dark: i8) -> Vec<Finder> {
    let is_dark = |x: usize, y: usize| classes[y * w + x] == dark;
    let mut found: Vec<Finder> = Vec::new();
    let mut runs: Vec<(usize, usize)> = Vec::with_capacity(w / 2);
    for y in (0..h).step_by(2) {
        // The row as runs of (start, length), alternating dark and light, the first one dark.
        runs.clear();
        let mut x = 0;
        while x < w && !is_dark(x, y) {
            x += 1;
        }
        while x < w {
            let start = x;
            let colour = is_dark(x, y);
            while x < w && is_dark(x, y) == colour {
                x += 1;
            }
            runs.push((start, x - start));
        }
        // Dark, light, dark (three times as long), light, dark: runs 0, 2, 4... are dark.
        for i in (0..runs.len().saturating_sub(4)).step_by(2) {
            let counts = [runs[i].1, runs[i + 1].1, runs[i + 2].1, runs[i + 3].1, runs[i + 4].1];
            if !ratios(&counts) {
                continue;
            }
            let total: usize = counts.iter().sum();
            let cx = runs[i + 2].0 + runs[i + 2].1 / 2;
            let Some((cy, tall)) = across(h, |k| is_dark(cx, k), y, total) else { continue };
            let Some((cx, wide)) = across(w, |k| is_dark(k, cy), cx, total) else { continue };
            let module = (wide + tall) as f32 / 14.0;
            let (fx, fy) = (cx as f32, cy as f32);
            match found.iter_mut().find(|f| (f.x - fx).hypot(f.y - fy) < 2.0 * f.module.max(module) && (f.module / module).max(module / f.module) < 1.6) {
                Some(f) => {
                    let n = f.hits as f32;
                    f.x = (f.x * n + fx) / (n + 1.0);
                    f.y = (f.y * n + fy) / (n + 1.0);
                    f.module = (f.module * n + module) / (n + 1.0);
                    f.hits += 1;
                }
                None => found.push(Finder { x: fx, y: fy, module, hits: 1 }),
            }
        }
    }
    // Seen on one row only, a finder of more than two pixels a module is a coincidence (it spans several scanned rows);
    // and its rings must be there all around, whatever the code's angle: a text's bars pass the row and column checks.
    found.retain(|f| (f.hits >= 2 || f.module < 2.0) && rings(w, h, &is_dark, f));
    found
}

/// Whether a finder's rings are there all around its middle, at radii that hold whatever the code's angle: its dark
/// middle (3 modules across), the light ring (to 2.5 modules from the middle) and, along its sides, the dark one.
fn rings(w: usize, h: usize, is_dark: &impl Fn(usize, usize) -> bool, f: &Finder) -> bool {
    let at = |r: f32, angle: f32| -> Option<bool> {
        let (x, y) = (f.x + r * f.module * angle.cos(), f.y + r * f.module * angle.sin());
        (x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h).then(|| is_dark(x as usize, y as usize))
    };
    let angles: Vec<f32> = (0..8).map(|k| k as f32 * std::f32::consts::FRAC_PI_4).collect();
    let count = |r: f32, dark: bool| angles.iter().filter(|&&a| at(r, a) == Some(dark)).count();
    // Inside 1.5 modules at any angle; outside 1.5 × √2 and inside 2.5; past 2.5 along a side (half the directions at most).
    count(0.9, true) >= 7 && count(2.25, false) >= 6 && count(3.0, true) >= 3
}

/// Whether five runs are a finder pattern's 1:1:3:1:1, within half a module each.
fn ratios(counts: &[usize; 5]) -> bool {
    let total: usize = counts.iter().sum();
    if total < 7 {
        return false;
    }
    let module = total as f32 / 7.0;
    let slack = module * 0.5;
    counts.iter().zip([1.0, 1.0, 3.0, 1.0, 1.0]).all(|(&c, k)| (c as f32 - module * k).abs() < slack * k.max(1.5))
}

/// Across the line through `at` (a column or a row of `len` pixels, `dark` telling its pixels apart): whether it crosses a
/// finder pattern there too, about as long as `total`; its middle and its length.
fn across(len: usize, dark: impl Fn(usize) -> bool, at: usize, total: usize) -> Option<(usize, usize)> {
    if !dark(at) {
        return None;
    }
    let limit = total * 2;
    // From the middle outwards: dark, light, dark on each side.
    let side = |step: isize| -> Option<[usize; 3]> {
        let mut counts = [0usize; 3];
        let mut k = at as isize;
        for (state, count) in counts.iter_mut().enumerate() {
            let want = state != 1;
            while k >= 0 && (k as usize) < len && dark(k as usize) == want && *count <= limit {
                *count += 1;
                k += step;
            }
            if *count == 0 || *count > limit {
                return None;
            }
        }
        Some(counts)
    };
    let up = side(-1)?;
    let down = side(1)?;
    // The middle run was counted from both sides, its first pixel twice.
    let counts = [up[2], up[1], up[0] + down[0] - 1, down[1], down[2]];
    let found: usize = counts.iter().sum();
    if !ratios(&counts) || found * 5 < total * 2 || found * 2 > total * 5 {
        return None;
    }
    let start = at - (up[0] - 1);
    Some((start + counts[2] / 2, found))
}

/// Whether finders of class `dark` show a code that could not be read, and where: three that go together, or two (cut
/// when the code they start would run past the picture's edge, whichever two of its corners they are).
fn glimpse(w: usize, h: usize, classes: &[i8], dark: i8, edges: bool, finders: &[Finder]) -> Option<(Glimpse, Near)> {
    let pair = |a: &Finder, b: &Finder| -> bool {
        if (a.module / b.module).max(b.module / a.module) > 1.5 {
            return false;
        }
        // Two corners of a code: at least 14 modules apart (the smallest code's side), at most a large code's diagonal.
        let apart = (a.x - b.x).hypot(a.y - b.y) / ((a.module + b.module) / 2.0);
        (14.0..=250.0).contains(&apart)
    };
    let near = |fs: &[&Finder]| -> Near {
        let module = fs.iter().map(|f| f.module).sum::<f32>() / fs.len() as f32;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for f in fs {
            (x0, y0, x1, y1) = (x0.min(f.x), y0.min(f.y), x1.max(f.x), y1.max(f.y));
        }
        Near { x0, y0, x1, y1, module }
    };
    let mut best: Option<(Glimpse, Near)> = None;
    for i in 0..finders.len() {
        for j in i + 1..finders.len() {
            let (a, b) = (&finders[i], &finders[j]);
            if !pair(a, b) {
                continue;
            }
            // A third that goes with both: three corners of one code.
            if let Some(c) = (0..finders.len()).filter(|&k| k != i && k != j).map(|k| &finders[k]).find(|c| pair(a, c) && pair(b, c)) {
                let n = near(&[a, b, c]);
                // Wholly in view (the finders are a code's outermost parts), else cut.
                let inside = n.x0 >= 3.5 * n.module && n.y0 >= 3.5 * n.module && n.x1 <= w as f32 - 3.5 * n.module && n.y1 <= h as f32 - 3.5 * n.module;
                return Some((if inside || !edges { Glimpse::Unclear } else { Glimpse::Cut }, n));
            }
            // Two only: they are two corners of a square code, along a side (the code on either hand) or across it. The
            // square whose inside looks like a code's (dark and light modules mixed) is taken; none: not a code. Cut when
            // that square runs off the picture.
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let module = (a.module + b.module) / 2.0;
            let (mx, my) = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            let squares = [
                [(a.x, a.y), (b.x, b.y), (b.x - dy, b.y + dx), (a.x - dy, a.y + dx)],
                [(a.x, a.y), (b.x, b.y), (b.x + dy, b.y - dx), (a.x + dy, a.y - dx)],
                [(a.x, a.y), (mx - dy / 2.0, my + dx / 2.0), (b.x, b.y), (mx + dy / 2.0, my - dx / 2.0)],
            ];
            let Some(square) = squares.iter().filter_map(|q| code_like(w, h, classes, dark, q).map(|score| (score, q))).max_by(|x, y| x.0.total_cmp(&y.0)).map(|(_, q)| q) else {
                continue;
            };
            let edge = 3.5 * module;
            let off = |&(x, y): &(f32, f32)| x < edge || y < edge || x > w as f32 - edge || y > h as f32 - edge;
            let seen = if edges && square.iter().any(off) { Glimpse::Cut } else { Glimpse::Unclear };
            if best.is_none_or(|(g, _)| seen == Glimpse::Cut && g != Glimpse::Cut) {
                let n = near(&[a, b]);
                let (xs, ys) = (square.map(|c| c.0), square.map(|c| c.1));
                let clamp = |v: f32, most: usize| v.clamp(0.0, most as f32);
                let n = Near {
                    x0: clamp(xs.iter().cloned().fold(n.x0, f32::min), w),
                    y0: clamp(ys.iter().cloned().fold(n.y0, f32::min), h),
                    x1: clamp(xs.iter().cloned().fold(n.x1, f32::max), w),
                    y1: clamp(ys.iter().cloned().fold(n.y1, f32::max), h),
                    module,
                };
                best = Some((seen, n));
            }
        }
    }
    if best.is_some() {
        return best;
    }
    // One finder alone, clearly seen, beside what looks like a code: the rest of it is off the picture (or hidden).
    for f in finders.iter().filter(|f| f.hits >= 3 && f.module >= 2.0) {
        let side = 14.0 * f.module;
        let quadrant = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)].into_iter().find(|(u, v)| {
            let q = [(f.x, f.y), (f.x + u * side, f.y), (f.x + u * side, f.y + v * side), (f.x, f.y + v * side)];
            code_like(w, h, classes, dark, &q).is_some()
        });
        if let Some((u, v)) = quadrant {
            let reach = 25.0 * f.module;
            let near_edge = f.x < reach || f.y < reach || f.x > w as f32 - reach || f.y > h as f32 - reach;
            let (x1, y1) = (f.x + u * side, f.y + v * side);
            let n = Near { x0: f.x.min(x1).max(0.0), y0: f.y.min(y1).max(0.0), x1: f.x.max(x1).min(w as f32), y1: f.y.max(y1).min(h as f32), module: f.module };
            return Some((if edges && near_edge { Glimpse::Cut } else { Glimpse::Unclear }, n));
        }
    }
    None
}

/// How much the inside of the square with corners `q` (finder middles, in turn) looks like a code's: dark and light
/// modules mixed, changing from one sample to the next. None when it does not, or too little of it is in the picture.
fn code_like(w: usize, h: usize, classes: &[i8], dark: i8, q: &[(f32, f32); 4]) -> Option<f32> {
    let (mut inside, mut dark_count, mut decided, mut changes, mut pairs) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let samples = 9;
    let mut total = 0;
    for j in 0..samples {
        let mut last: Option<bool> = None;
        for i in 0..samples {
            total += 1;
            // Across the middle of the square, clear of the finders (each 3.5 modules from a corner).
            let (s, t) = (0.2 + 0.6 * i as f32 / (samples - 1) as f32, 0.2 + 0.6 * j as f32 / (samples - 1) as f32);
            let top = (q[0].0 + (q[1].0 - q[0].0) * s, q[0].1 + (q[1].1 - q[0].1) * s);
            let bottom = (q[3].0 + (q[2].0 - q[3].0) * s, q[3].1 + (q[2].1 - q[3].1) * s);
            let (x, y) = (top.0 + (bottom.0 - top.0) * t, top.1 + (bottom.1 - top.1) * t);
            if x < 0.0 || y < 0.0 || x as usize >= w || y as usize >= h {
                last = None;
                continue;
            }
            inside += 1;
            let class = classes[y as usize * w + x as usize];
            decided += (class != 0) as usize;
            let is = class == dark;
            dark_count += is as usize;
            if let Some(l) = last {
                pairs += 1;
                changes += (l != is) as usize;
            }
            last = Some(is);
        }
    }
    if inside * 5 < total || pairs < 6 {
        return None;
    }
    let darkness = dark_count as f32 / inside as f32;
    let change = changes as f32 / pairs as f32;
    // A code's modules are dark or light, rarely in between; a poster's flat parts are neither.
    let decided = decided as f32 / inside as f32;
    ((0.2..=0.8).contains(&darkness) && change >= 0.3 && decided >= 0.6).then_some(change * decided)
}

/// What a look reads: the whole picture, a closer look where the last whole one glimpsed a code, or the middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    Whole,
    Closer(Near),
    Middle,
}

/// A grey copy made for one look.
pub struct Grey {
    pub look: Look,
    /// The frame's size.
    pub frame: (usize, usize),
    pub width: usize,
    pub height: usize,
    pub grey: Vec<u8>,
}

/// A scanner's looks, in turn: the whole picture; then a closer look where a code was glimpsed (else the middle); then
/// the middle (a code too small for the whole picture, held before the camera). About a second for the three, at three
/// looks a second.
#[derive(Default)]
pub struct Looks {
    count: u64,
    /// Where the last whole look, and the last middle one, glimpsed a code, in the frame's pixels.
    near_whole: Option<Near>,
    near_middle: Option<Near>,
    /// What the last whole look glimpsed.
    pub glimpse: Option<Glimpse>,
}

impl Looks {
    /// The next look.
    pub fn next_look(&self) -> Look {
        match self.count % 3 {
            0 => Look::Whole,
            1 => self.near_whole.or(self.near_middle).map_or(Look::Middle, Look::Closer),
            _ => Look::Middle,
        }
    }

    /// The grey copy of `frame` the next look reads (made while the frame is lent).
    pub fn copy(&self, frame: &Pixels<'_>) -> Grey {
        let look = self.next_look();
        let (width, height, grey) = match look {
            Look::Whole => grey(frame, View::Whole),
            Look::Middle => grey(frame, View::Middle),
            Look::Closer(near) => closer(frame, &near),
        };
        Grey { look, width, height, grey, frame: (frame.width, frame.height) }
    }

    /// Reads a copy made by [`Looks::copy`].
    pub fn read(&mut self, copy: &Grey) -> Reading {
        self.count += 1;
        let reading = read(copy.width, copy.height, &copy.grey, copy.look == Look::Whole);
        let (fw, fh) = copy.frame;
        // From the copy's pixels to the frame's.
        let to_frame = |near: Near, x: f32, y: f32, step: f32| Near { x0: x + near.x0 * step, y0: y + near.y0 * step, x1: x + near.x1 * step, y1: y + near.y1 * step, module: near.module * step };
        match copy.look {
            Look::Whole => {
                self.glimpse = reading.glimpse;
                let step = fw as f32 / copy.width.max(1) as f32;
                self.near_whole = reading.near.map(|n| to_frame(n, 0.0, 0.0, step));
            }
            Look::Middle => {
                let step = (fw / 2) as f32 / copy.width.max(1) as f32;
                self.near_middle = reading.near.map(|n| to_frame(n, (fw / 4) as f32, (fh / 4) as f32, step));
            }
            Look::Closer(_) => {}
        }
        reading
    }
}

#[cfg(test)]
mod tests;
