//! Lab: chromatic aberration (Post Processing Stack v2, what VRChat worlds use) against the capture's depth symbols.
//!
//! PPv2 samples the frame along the segment from each pixel towards the centre, `diff = -c·|c|²·0.05·intensity`
//! (`c` = 2·uv − 1), weighting red near the pixel, green in the middle and blue at the far end. Grey symbols come out
//! smeared along that segment: in the channel mean the capture's decoder reads, over the whole of it; in the red
//! channel alone, over about a third, shifted by about a fifth.
//!
//! ```text
//! cargo run --release --example cacheck -- measure <session> [every-nth-keyframe]
//! cargo run --release --example cacheck -- simulate [cell px]
//! ```
//!
//! `measure` estimates a session's intensity from its keyframes: the shift between the blue and the red channel of
//! textured patches, fitted to PPv2's law (blue − red = 0.64·diff). `simulate` draws depth frames of random codes,
//! aberrates them at several intensities and decodes them with the capture's decoder (channel mean) and variants,
//! per distance from the centre.

use anyhow::Context as _;
use orbiters_contracts::color;
use orbiters_contracts::depth::{LEVELS, LEVEL_SRGB, MIN_LEVEL_SPACING, SAMPLES_PER_TILE, TILE_CELLS};
use orbiters_reconstruct::dataset;
use rayon::prelude::*;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("measure") => measure(&args[1..]),
        Some("simulate") => simulate(&args[1..]),
        _ => anyhow::bail!("usage: cacheck measure <session> [every] | cacheck simulate [cell]"),
    }
}

/// Three planes (r, g, b) of a w × h picture.
#[derive(Clone)]
struct Planes {
    w: usize,
    h: usize,
    c: [Vec<f32>; 3],
}

impl Planes {
    fn new(w: usize, h: usize) -> Self {
        Self { w, h, c: [vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h]] }
    }

    fn px(&self, ch: usize, x: i64, y: i64) -> f32 {
        let x = x.clamp(0, self.w as i64 - 1) as usize;
        let y = y.clamp(0, self.h as i64 - 1) as usize;
        self.c[ch][y * self.w + x]
    }

    /// Bilinear sample at uv (texel centres at (i + 0.5) / size), clamped like a GPU sampler.
    fn sample(&self, ch: usize, u: f32, v: f32) -> f32 {
        let (x, y) = (u * self.w as f32 - 0.5, v * self.h as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.px(ch, x0, y0) * (1.0 - fx) + self.px(ch, x0 + 1, y0) * fx;
        let b = self.px(ch, x0, y0 + 1) * (1.0 - fx) + self.px(ch, x0 + 1, y0 + 1) * fx;
        a * (1.0 - fy) + b * fy
    }
}

/// PPv2's default spectral LUT (3 texels red, green, blue; bilinear, clamped) at t.
fn spectral(t: f32) -> [f32; 3] {
    let x = (t * 3.0 - 0.5).clamp(0.0, 2.0);
    let i = (x.floor() as usize).min(1);
    let f = x - i as f32;
    let mut out = [0.0; 3];
    out[i] += 1.0 - f;
    out[i + 1] += f;
    out
}

/// PPv2's chromatic aberration (Uber shader, `CHROMATIC_ABERRATION` or `_LOW` for fast mode) on linear planes.
fn aberrate(src: &Planes, intensity: f32, fast: bool) -> Planes {
    let amount = intensity * 0.05;
    let (w, h) = (src.w, src.h);
    let mut out = Planes::new(w, h);
    let rows: Vec<Vec<[f32; 3]>> = (0..h)
        .into_par_iter()
        .map(|y| {
            (0..w)
                .map(|x| {
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    let (cx, cy) = (2.0 * u - 1.0, 2.0 * v - 1.0);
                    let d2 = cx * cx + cy * cy;
                    let (dx, dy) = (-cx * d2 * amount, -cy * d2 * amount);
                    let mut sum = [0.0f32; 3];
                    let mut wsum = [0.0f32; 3];
                    if fast {
                        for i in 0..3 {
                            let f = spectral((i as f32 + 0.5) / 3.0);
                            let (pu, pv) = (u + dx * i as f32 / 3.0, v + dy * i as f32 / 3.0);
                            for ch in 0..3 {
                                sum[ch] += f[ch] * src.sample(ch, pu, pv);
                                wsum[ch] += f[ch];
                            }
                        }
                    } else {
                        let len = ((dx * w as f32).powi(2) + (dy * h as f32).powi(2)).sqrt();
                        let n = ((len / 2.0) as i32).clamp(3, 16);
                        for i in 0..n {
                            let t = (i as f32 + 0.5) / n as f32;
                            let f = spectral(t);
                            let (pu, pv) = (u + dx * i as f32 / n as f32, v + dy * i as f32 / n as f32);
                            for ch in 0..3 {
                                sum[ch] += f[ch] * src.sample(ch, pu, pv);
                                wsum[ch] += f[ch];
                            }
                        }
                    }
                    [sum[0] / wsum[0].max(1e-6), sum[1] / wsum[1].max(1e-6), sum[2] / wsum[2].max(1e-6)]
                })
                .collect()
        })
        .collect();
    for (y, row) in rows.into_iter().enumerate() {
        for (x, p) in row.into_iter().enumerate() {
            for ch in 0..3 {
                out.c[ch][y * w + x] = p[ch];
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// simulate

/// A frame of symbol tiles with random codes: linear planes and the codes of every tile.
fn draw(w: usize, h: usize, cell: usize, seed: u64) -> (Planes, Vec<[u16; SAMPLES_PER_TILE as usize]>, usize, usize) {
    let tile = TILE_CELLS as usize * cell;
    let (tx, ty) = (w / tile, h / tile);
    let mut rng = fastrand::Rng::with_seed(seed);
    let lin: [f32; LEVELS] = LEVEL_SRGB.map(color::srgb_to_linear);
    let mut p = Planes::new(w, h);
    let mut codes = Vec::with_capacity(tx * ty);
    for t in 0..tx * ty {
        let mut c = [0u16; SAMPLES_PER_TILE as usize];
        for v in c.iter_mut() {
            *v = rng.u16(1..=u16::MAX);
        }
        let (x0, y0) = ((t % tx) * tile, (t / tx) * tile);
        for row in 0..TILE_CELLS as usize {
            for col in 0..TILE_CELLS as usize {
                let level = symbol(&c, col, row);
                for yy in 0..cell {
                    for xx in 0..cell {
                        let i = (y0 + row * cell + yy) * w + x0 + col * cell + xx;
                        for ch in 0..3 {
                            p.c[ch][i] = lin[level];
                        }
                    }
                }
            }
        }
        codes.push(c);
    }
    (p, codes, tx, ty)
}

/// Level of a cell: pilots on row 0, two bits of a sample elsewhere (as `orbiters_contracts::depth`).
fn symbol(codes: &[u16; SAMPLES_PER_TILE as usize], col: usize, row: usize) -> usize {
    if row == 0 {
        return col % LEVELS;
    }
    let e = (row - 1) * TILE_CELLS as usize + col;
    ((codes[e / 8] >> (14 - 2 * (e % 8))) & 3) as usize
}

/// What a decoder reads: one plane of 8-bit-like values (0..255).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Channel {
    Mean,
    Red,
    Green,
    Blue,
}

/// sRGB 8-bit planes of an aberrated linear frame (what the capture receives).
fn encode8(p: &Planes) -> Planes {
    let mut out = p.clone();
    for ch in 0..3 {
        for v in out.c[ch].iter_mut() {
            *v = color::linear_to_srgb8(*v) as f32;
        }
    }
    out
}

/// A channel's value at a sub-pixel position (pixel centres at integers + 0.5).
fn channel_value(p8: &Planes, ch: Channel, x: f32, y: f32) -> f32 {
    let (u, v) = (x / p8.w as f32, y / p8.h as f32);
    match ch {
        Channel::Mean => (p8.sample(0, u, v) + p8.sample(1, u, v) + p8.sample(2, u, v)) / 3.0,
        Channel::Red => p8.sample(0, u, v),
        Channel::Green => p8.sample(1, u, v),
        Channel::Blue => p8.sample(2, u, v),
    }
}

/// Where PPv2 takes what a channel shows at `(x, y)` from: the weighted mean of its taps' offsets (pixels), so the
/// symbol drawn at `p` shows near `p − offset`.
fn channel_offset(ch: Channel, x: f32, y: f32, w: usize, h: usize, intensity: f32, fast: bool) -> (f32, f32) {
    let amount = intensity * 0.05;
    let (cx, cy) = (2.0 * x / w as f32 - 1.0, 2.0 * y / h as f32 - 1.0);
    let d2 = cx * cx + cy * cy;
    let (dx, dy) = (-cx * d2 * amount * w as f32, -cy * d2 * amount * h as f32);
    let n = if fast { 3 } else { (((dx * dx + dy * dy).sqrt() / 2.0) as i32).clamp(3, 16) };
    let (mut sum, mut wsum) = (0.0f32, 0.0f32);
    for i in 0..n {
        let f = spectral((i as f32 + 0.5) / n as f32);
        let wgt = match ch {
            Channel::Red => f[0],
            Channel::Green => f[1],
            Channel::Blue => f[2],
            Channel::Mean => (f[0] + f[1] + f[2]) / 3.0,
        };
        sum += wgt * i as f32 / n as f32;
        wsum += wgt;
    }
    let t = sum / wsum.max(1e-6);
    (t * dx, t * dy)
}

struct Read {
    gap: f32,
    /// Share of data cells far from every level (more than 0.3 of the smallest gap from the nearest).
    ambiguous: f32,
    codes: [u16; SAMPLES_PER_TILE as usize],
}

/// The capture's tile reader on one channel, each cell read with its pixels moved by `offset(cell centre)`: pilot
/// means, nearest level per data cell.
fn read_tile(p8: &Planes, ch: Channel, x0: f32, y0: f32, cell: f32, offset: &dyn Fn(f32, f32) -> (f32, f32)) -> Read {
    let m = (cell / 3.0).floor().max(1.0);
    let inner = (cell - 2.0 * m).max(1.0) as i64;
    let value = |col: i64, row: i64| {
        let (cx, cy) = (x0 + (col as f32 + 0.5) * cell, y0 + (row as f32 + 0.5) * cell);
        let (ox, oy) = offset(cx, cy);
        let mut s = 0.0;
        for yy in 0..inner {
            for xx in 0..inner {
                // Pixel centres of the inner square, as the capture's decoder averages them.
                let (px, py) = (x0 + col as f32 * cell + m + xx as f32 + 0.5, y0 + row as f32 * cell + m + yy as f32 + 0.5);
                s += channel_value(p8, ch, px + ox, py + oy);
            }
        }
        s / (inner * inner) as f32
    };
    let mut lut = [0f32; LEVELS];
    for col in 0..TILE_CELLS as i64 {
        lut[col as usize % LEVELS] += value(col, 0) / (TILE_CELLS as usize / LEVELS) as f32;
    }
    let gap = (1..LEVELS).map(|k| lut[k] - lut[k - 1]).fold(f32::MAX, f32::min);
    let mut codes = [0u16; SAMPLES_PER_TILE as usize];
    let mut ambiguous = 0usize;
    for row in 1..TILE_CELLS as i64 {
        for col in 0..TILE_CELLS as i64 {
            let v = value(col, row);
            let mut best = 0;
            for k in 1..LEVELS {
                if (v - lut[k]).abs() < (v - lut[best]).abs() {
                    best = k;
                }
            }
            if (v - lut[best]).abs() > 0.3 * gap.max(1e-3) {
                ambiguous += 1;
            }
            let e = ((row - 1) * TILE_CELLS as i64 + col) as usize;
            codes[e / 8] |= (best as u16) << (14 - 2 * (e % 8));
        }
    }
    Read { gap, ambiguous: ambiguous as f32 / (TILE_CELLS * (TILE_CELLS - 1)) as f32, codes }
}

#[derive(Clone, Copy, Debug)]
enum Place {
    /// The nominal cell centres.
    Nominal,
    /// Whole-pixel shifts up to n, the largest pilot gap wins (near-ties: the smaller shift).
    Search(i64),
    /// PPv2's offset of the channel at the frame's intensity (known or estimated).
    Model,
}

#[derive(Clone, Copy, Debug)]
struct Decoder {
    name: &'static str,
    channels: &'static [Channel],
    place: Place,
    /// Also reject tiles with more than 5 % ambiguous cells.
    strict: bool,
}

const DECODERS: &[Decoder] = &[
    Decoder { name: "mean (capture)", channels: &[Channel::Mean], place: Place::Nominal, strict: false },
    Decoder { name: "mean, strict", channels: &[Channel::Mean], place: Place::Nominal, strict: true },
    Decoder { name: "red", channels: &[Channel::Red], place: Place::Nominal, strict: false },
    Decoder { name: "red, strict", channels: &[Channel::Red], place: Place::Nominal, strict: true },
    Decoder { name: "red search", channels: &[Channel::Red], place: Place::Search(6), strict: true },
    Decoder { name: "red model", channels: &[Channel::Red], place: Place::Model, strict: true },
    Decoder { name: "r|g|b model", channels: &[Channel::Red, Channel::Green, Channel::Blue], place: Place::Model, strict: true },
];

fn simulate(args: &[String]) -> anyhow::Result<()> {
    let cell: usize = args.first().map(|s| s.parse()).transpose()?.unwrap_or(3);
    let fast_modes: Vec<bool> = match args.get(1).map(String::as_str) {
        Some("fast") => vec![true],
        Some("normal") => vec![false],
        _ => vec![false, true],
    };
    let (w, h) = (1920usize, 1080usize);
    let (frame, truth, tx, ty) = draw(w, h, cell, 11);
    let tile = (TILE_CELLS as usize * cell) as f32;
    // Radius of each tile's centre in PPv2's coordinates (c = 2·uv − 1).
    let radius: Vec<f32> = (0..tx * ty)
        .map(|t| {
            let (x, y) = ((t % tx) as f32 * tile + tile / 2.0, (t / tx) as f32 * tile + tile / 2.0);
            let (cx, cy) = (2.0 * x / w as f32 - 1.0, 2.0 * y / h as f32 - 1.0);
            (cx * cx + cy * cy).sqrt()
        })
        .collect();
    let bands = [0.0f32, 0.3, 0.5, 0.7, 0.9, 1.1, 1.5];
    println!("cell {cell} px, {tx} × {ty} tiles; per band of radius (c = 2uv − 1), then all tiles: good samples % (accepted but wrong samples %)");
    for fast in fast_modes {
        for &intensity in &[0.0f32, 0.05, 0.092, 0.145, 0.2, 0.3, 0.5] {
            let p8 = encode8(&aberrate(&frame, intensity, fast));
            println!("\nintensity {intensity}{}", if fast { " (fast mode)" } else { "" });
            let mut header: Vec<String> = bands.windows(2).map(|b| format!("{:.1}–{:.1}", b[0], b[1])).collect();
            header.push("all".into());
            header.push("all, ECC 5 %".into());
            println!("  {:<16} {}", "decoder", header.iter().map(|s| format!("{s:>13}")).collect::<String>());
            for d in DECODERS {
                // Per tile: the decoder's codes when it accepts the tile, and the best read whenever the pilots are
                // readable (what an error-correcting code could repair).
                let results: Vec<(Option<[u16; SAMPLES_PER_TILE as usize]>, Option<[u16; SAMPLES_PER_TILE as usize]>)> = (0..tx * ty)
                    .into_par_iter()
                    .map(|t| {
                        let (x0, y0) = ((t % tx) as f32 * tile, (t / tx) as f32 * tile);
                        let mut best: Option<(Read, i64)> = None;
                        let mut consider = |r: Read, cost: i64| {
                            let better = match &best {
                                None => true,
                                Some((b, bc)) => r.gap > b.gap + 0.5 || (r.gap > b.gap - 0.5 && cost < *bc),
                            };
                            if better {
                                best = Some((r, cost));
                            }
                        };
                        for &ch in d.channels {
                            match d.place {
                                Place::Nominal => consider(read_tile(&p8, ch, x0, y0, cell as f32, &|_, _| (0.0, 0.0)), 0),
                                Place::Search(n) => {
                                    for sy in -n..=n {
                                        for sx in -n..=n {
                                            let r = read_tile(&p8, ch, x0, y0, cell as f32, &|_, _| (sx as f32, sy as f32));
                                            consider(r, sx.abs().max(sy.abs()));
                                        }
                                    }
                                }
                                Place::Model => {
                                    let r = read_tile(&p8, ch, x0, y0, cell as f32, &|x, y| {
                                        let (ox, oy) = channel_offset(ch, x, y, w, h, intensity, fast);
                                        (-ox, -oy)
                                    });
                                    consider(r, 0);
                                }
                            }
                        }
                        let readable = best.as_ref().filter(|(b, _)| b.gap >= MIN_LEVEL_SPACING).map(|(b, _)| b.codes);
                        let accepted = best.filter(|(b, _)| b.gap >= MIN_LEVEL_SPACING && (!d.strict || b.ambiguous <= 0.05)).map(|(b, _)| b.codes);
                        (accepted, readable)
                    })
                    .collect();
                let count = |lo: f32, hi: f32| {
                    let (mut good, mut wrong, mut total) = (0usize, 0usize, 0usize);
                    for t in 0..tx * ty {
                        if radius[t] < lo || radius[t] >= hi {
                            continue;
                        }
                        total += SAMPLES_PER_TILE as usize;
                        if let Some(codes) = &results[t].0 {
                            for (s, &c) in codes.iter().enumerate() {
                                if close(c, truth[t][s]) {
                                    good += 1;
                                } else {
                                    wrong += 1;
                                }
                            }
                        }
                    }
                    let pct = |n: usize| 100.0 * n as f32 / total.max(1) as f32;
                    format!("{:>13}", format!("{:.0} ({:.1})", pct(good), pct(wrong)))
                };
                let mut cells = String::new();
                for b in bands.windows(2) {
                    cells += &count(b[0], b[1]);
                }
                cells += &count(0.0, 9.0);
                // With a code that repairs up to 5 % wrong symbols per tile: every readable tile with at most 12 of its
                // 240 data symbols wrong counts as good.
                let (mut repaired, mut total) = (0usize, 0usize);
                for t in 0..tx * ty {
                    total += SAMPLES_PER_TILE as usize;
                    if let Some(codes) = &results[t].1 {
                        let wrong: u32 = codes.iter().zip(&truth[t]).map(|(&a, &b)| (0..8).filter(|k| ((a >> (2 * k)) & 3) != ((b >> (2 * k)) & 3)).count() as u32).sum();
                        if wrong <= 12 {
                            repaired += SAMPLES_PER_TILE as usize;
                        }
                    }
                }
                cells += &format!("{:>13}", format!("{:.0}", 100.0 * repaired as f32 / total.max(1) as f32));
                println!("  {:<16} {cells}", d.name);
            }
        }
    }
    Ok(())
}

// Codes within 2 % in depth (inverse-depth codes: relative depth error ≈ relative code error near the code).
fn close(a: u16, b: u16) -> bool {
    match (orbiters_contracts::depth::dequantize(a), orbiters_contracts::depth::dequantize(b)) {
        (Some(za), Some(zb)) => (za - zb).abs() <= 0.02 * zb,
        (None, None) => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// measure

fn measure(args: &[String]) -> anyhow::Result<()> {
    let session = PathBuf::from(args.first().context("measure <session> [every]")?);
    let every: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(8);
    let data = dataset::load(&session, 1920, |_| {})?;
    let (w, h) = (data.width as usize, data.height as usize);
    let (gx, gy, half, search) = (24usize, 14usize, 24i64, 14i64);
    let mut rows: Vec<(f32, f32, f32, f32)> = Vec::new(); // (cx, cy, shift x, shift y) of reliable patches
    for v in data.views.iter().step_by(every.max(1)) {
        let mut p = Planes::new(w, h);
        for (i, &px) in v.image.iter().enumerate() {
            for ch in 0..3 {
                p.c[ch][i] = ((px >> (8 * ch)) & 255) as f32 / 255.0;
            }
        }
        let found: Vec<Option<(f32, f32, f32, f32)>> = (0..gx * gy)
            .into_par_iter()
            .map(|k| {
                let (x, y) = (((k % gx) as f32 + 0.5) * w as f32 / gx as f32, ((k / gx) as f32 + 0.5) * h as f32 / gy as f32);
                let (x, y) = (x as i64, y as i64);
                // Whole patch plus the search window inside the scene.
                let r = half + search + 1;
                if x - r < 0 || y - r < 0 || x + r >= w as i64 || y + r >= h as i64 {
                    return None;
                }
                for yy in (y - r..=y + r).step_by(4) {
                    for xx in (x - r..=x + r).step_by(4) {
                        if data.mask[yy as usize * w + xx as usize] < 0.5 {
                            return None;
                        }
                    }
                }
                let s = best_shift(&p, x, y, half, search)?;
                let (cx, cy) = (2.0 * x as f32 / w as f32 - 1.0, 2.0 * y as f32 / h as f32 - 1.0);
                Some((cx, cy, s.0, s.1))
            })
            .collect();
        rows.extend(found.into_iter().flatten());
    }
    // PPv2: blue − red = 0.64·diff (normal mode; 0.67 in fast mode), diff = −c·|c|²·amount in uv.
    let (mut num, mut den) = (0f64, 0f64);
    let mut per: Vec<f32> = Vec::new();
    for &(cx, cy, sx, sy) in &rows {
        let d2 = cx * cx + cy * cy;
        let (ex, ey) = (-cx * d2 * w as f32 * 0.64, -cy * d2 * h as f32 * 0.64);
        let ee = ex * ex + ey * ey;
        num += (sx * ex + sy * ey) as f64;
        den += ee as f64;
        if d2.sqrt() > 0.6 {
            per.push((sx * ex + sy * ey) / ee);
        }
    }
    per.sort_by(f32::total_cmp);
    let amount = (num / den.max(1e-12)) as f32;
    let median = per.get(per.len() / 2).copied().unwrap_or(0.0);
    println!("{}: {} reliable patches; least-squares intensity {:.3}, median of {} outer patches {:.3}", session.display(), rows.len(), amount / 0.05, per.len(), median / 0.05);
    println!("  radius   patches   mean radial blue−red shift (px)   PPv2 at the fitted intensity (px)");
    for b in [0.0f32, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2] {
        let sel: Vec<_> = rows.iter().filter(|r| { let rr = (r.0 * r.0 + r.1 * r.1).sqrt(); rr >= b && rr < b + 0.2 }).collect();
        if sel.is_empty() {
            continue;
        }
        let mut radial = 0.0;
        let mut model = 0.0;
        for &&(cx, cy, sx, sy) in &sel {
            let rr = (cx * cx + cy * cy).sqrt().max(1e-6);
            // Radial unit vector in pixels (outwards).
            let (ux, uy) = (cx * w as f32, cy * h as f32);
            let ul = (ux * ux + uy * uy).sqrt().max(1e-6);
            radial += (sx * ux + sy * uy) / ul;
            let d2 = rr * rr;
            let (ex, ey) = (-cx * d2 * w as f32 * 0.64 * amount, -cy * d2 * h as f32 * 0.64 * amount);
            model += (ex * ux + ey * uy) / ul;
        }
        println!("  {:.1}–{:.1} {:>8} {:>20.2} {:>30.2}", b, b + 0.2, sel.len(), radial / sel.len() as f32, model / sel.len() as f32);
    }
    Ok(())
}

/// Shift s (sub-pixel) that best maps the red patch onto the blue one: blue(x) ≈ red(x + s). None for flat or
/// ambiguous patches.
fn best_shift(p: &Planes, x: i64, y: i64, half: i64, search: i64) -> Option<(f32, f32)> {
    let n = (2 * half + 1) as usize;
    let patch = |ch: usize, ox: i64, oy: i64| -> Vec<f32> {
        let mut v = Vec::with_capacity(n * n);
        for yy in -half..=half {
            for xx in -half..=half {
                v.push(p.px(ch, x + xx + ox, y + yy + oy));
            }
        }
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        v.iter_mut().for_each(|a| *a -= mean);
        v
    };
    let blue = patch(2, 0, 0);
    let bn = blue.iter().map(|a| a * a).sum::<f32>().sqrt();
    if bn / (n as f32) < 0.02 {
        return None;
    }
    let size = (2 * search + 1) as usize;
    let mut score = vec![f32::MIN; size * size];
    for sy in -search..=search {
        for sx in -search..=search {
            let red = patch(0, sx, sy);
            let rn = red.iter().map(|a| a * a).sum::<f32>().sqrt();
            if rn / (n as f32) < 0.02 {
                continue;
            }
            let dot: f32 = red.iter().zip(&blue).map(|(a, b)| a * b).sum();
            score[((sy + search) as usize) * size + (sx + search) as usize] = dot / (rn * bn);
        }
    }
    let (best, &peak) = score.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let (bx, by) = ((best % size) as i64 - search, (best / size) as i64 - search);
    if peak < 0.8 || bx.abs() == search || by.abs() == search {
        return None;
    }
    // Ambiguity: a different peak 3+ px away nearly as good (repetitive texture).
    for (i, &s) in score.iter().enumerate() {
        let (ix, iy) = ((i % size) as i64 - search, (i / size) as i64 - search);
        if (ix - bx).abs().max((iy - by).abs()) >= 3 && s > peak - 0.03 {
            return None;
        }
    }
    let at = |dx: i64, dy: i64| score[((by + dy + search) as usize) * size + (bx + dx + search) as usize];
    let parabola = |a: f32, b: f32, c: f32| {
        let d = a - 2.0 * b + c;
        if d.abs() < 1e-9 { 0.0 } else { (0.5 * (a - c) / d).clamp(-0.5, 0.5) }
    };
    Some((bx as f32 + parabola(at(-1, 0), peak, at(1, 0)), by as f32 + parabola(at(0, -1), peak, at(0, 1))))
}
