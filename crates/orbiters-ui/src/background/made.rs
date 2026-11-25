//! The pictures the scenes read, made here as the website's canvases make theirs: Window Light's foliage
//! (`leafShadow/leafShadowTexture.js`) and Gradient ASCII's glyphs (`gradientAsciiRenderer.js`, `buildAtlas`).

use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{FontRef, LocationRef, MetadataProvider, Size};
use skrifa::raw::TableProvider;
use tiny_skia::{FillRule, LineCap, Mask, Path, PathBuilder, Stroke, Transform};

/// The foliage's side in pixels, and the part of the scene it covers: left, bottom, side.
pub const LEAVES: usize = 1024;
pub const REGION: [f32; 3] = [-0.05, -0.15, 1.15];

/// mulberry32, as the website draws its branches with.
struct Random(u32);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

fn to_canvas(x: f64, y: f64) -> (f32, f32) {
    let [rx, ry, size] = REGION.map(f64::from);
    (((x - rx) / size * LEAVES as f64) as f32, ((1.0 - (y - ry) / size) * LEAVES as f64) as f32)
}

fn pixels(units: f64) -> f64 {
    units / f64::from(REGION[2]) * LEAVES as f64
}

/// One layer of foliage: each shape casts its own blurred shadow, laid over those before (a canvas's `shadowBlur`).
struct Layer {
    alpha: Vec<f32>,
    sigma: f32,
}

impl Layer {
    fn shadow(&mut self, path: &Path, width: Option<f32>) {
        let stroked = match width {
            Some(width) => path.stroke(&Stroke { width, line_cap: LineCap::Round, ..Stroke::default() }, 1.0),
            None => None,
        };
        let path = stroked.as_ref().unwrap_or(path);
        let pad = (self.sigma * 3.0).ceil() + 2.0;
        let b = path.bounds();
        let (x0, y0) = ((b.left() - pad).floor(), (b.top() - pad).floor());
        let (w, h) = ((b.right() + pad - x0).ceil() as usize, (b.bottom() + pad - y0).ceil() as usize);
        let Some(mut mask) = Mask::new(w as u32, h as u32) else { return };
        mask.fill_path(path, FillRule::Winding, true, Transform::from_translate(-x0, -y0));
        let mut soft: Vec<f32> = mask.data().iter().map(|v| f32::from(*v) / 255.0).collect();
        blur(&mut soft, w, h, self.sigma);
        for y in 0..h {
            let at = y0 as i64 + y as i64;
            if !(0..LEAVES as i64).contains(&at) {
                continue;
            }
            for x in 0..w {
                let ax = x0 as i64 + x as i64;
                if (0..LEAVES as i64).contains(&ax) {
                    let a = &mut self.alpha[at as usize * LEAVES + ax as usize];
                    *a += soft[y * w + x] * (1.0 - *a);
                }
            }
        }
    }

    /// A long leaf with pointed ends on a short stalk.
    fn leaf(&mut self, x: f64, y: f64, angle: f64, length: f64) {
        let (cx, cy) = to_canvas(x, y);
        let l = pixels(length) as f32;
        let w = l * 0.17;
        let stalk = l * 0.1;
        let place = Transform::from_rotate((-angle).to_degrees() as f32).post_translate(cx, cy);
        let mut pen = PathBuilder::new();
        pen.move_to(0.0, 0.0);
        pen.line_to(stalk, 0.0);
        if let Some(p) = pen.finish().and_then(|p| p.transform(place)) {
            self.shadow(&p, Some((w * 0.18).max(1.0)));
        }
        let mut pen = PathBuilder::new();
        pen.move_to(stalk, 0.0);
        pen.cubic_to(stalk + l * 0.22, -w * 1.25, stalk + l * 0.62, -w * 1.15, stalk + l, 0.0);
        pen.cubic_to(stalk + l * 0.62, w * 1.15, stalk + l * 0.22, w * 1.25, stalk, 0.0);
        pen.close();
        if let Some(p) = pen.finish().and_then(|p| p.transform(place)) {
            self.shadow(&p, None);
        }
    }

    /// A thin curving branch with alternate leaves that hang slightly toward the ground.
    fn branch(&mut self, rand: &mut Random, b: &Branch) {
        let steps = (b.length / 0.02).ceil();
        let step = b.length / steps;
        let (mut px, mut py, mut heading, mut travelled, mut next, mut side) = (b.x, b.y, b.angle, 0.0, b.length * b.bare, 1.0);
        for i in 0..steps as usize {
            heading += b.bend * step + (rand.next() - 0.5) * 0.04;
            let (nx, ny) = (px + heading.cos() * step, py + heading.sin() * step);
            let mut pen = PathBuilder::new();
            let (a, z) = (to_canvas(px, py), to_canvas(nx, ny));
            pen.move_to(a.0, a.1);
            pen.line_to(z.0, z.1);
            if let Some(p) = pen.finish() {
                self.shadow(&p, Some((pixels(b.width) * (1.0 - (i as f64 / steps) * 0.7)).max(1.0) as f32));
            }
            travelled += step;
            if travelled >= next {
                let spread = side * (0.55 + rand.next() * 0.4);
                let direction: f64 = heading + spread;
                let droop = (direction.sin() - 0.45).atan2(direction.cos());
                self.leaf(nx, ny, droop, b.leaf * (0.8 + rand.next() * 0.35));
                side = -side;
                next += b.spacing * (0.8 + rand.next() * 0.4);
            }
            (px, py) = (nx, ny);
        }
        self.leaf(px, py, heading - 0.2, b.leaf);
    }
}

struct Branch {
    x: f64,
    y: f64,
    angle: f64,
    length: f64,
    bend: f64,
    width: f64,
    spacing: f64,
    leaf: f64,
    bare: f64,
}

/// Three box blurs (what a canvas draws a wide Gaussian with).
fn blur(a: &mut [f32], w: usize, h: usize, sigma: f32) {
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut low = ideal.floor() as i64;
    if low % 2 == 0 {
        low -= 1;
    }
    let wide = ((12.0 * sigma * sigma - (3 * low * low + 12 * low + 9) as f32) / (-4 * low - 4) as f32).round() as i64;
    let mut line = vec![0.0f32; w.max(h)];
    for pass in 0..3 {
        let r = ((if pass < wide { low } else { low + 2 }) - 1) / 2;
        let k = 1.0 / (2 * r + 1) as f32;
        let run = |get: &mut dyn FnMut(usize) -> f32, n: usize, line: &mut [f32]| {
            let mut sum = 0.0;
            for i in 0..=(r as usize).min(n - 1) {
                sum += get(i);
            }
            for i in 0..n {
                line[i] = sum * k;
                let add = i as i64 + r + 1;
                if add < n as i64 {
                    sum += get(add as usize);
                }
                let sub = i as i64 - r;
                if sub >= 0 {
                    sum -= get(sub as usize);
                }
            }
        };
        for y in 0..h {
            run(&mut |x| a[y * w + x], w, &mut line);
            a[y * w..(y + 1) * w].copy_from_slice(&line[..w]);
        }
        for x in 0..w {
            run(&mut |y| a[y * w + x], h, &mut line);
            for y in 0..h {
                a[y * w + x] = line[y];
            }
        }
    }
}

/// The foliage, RGBA: red the near branches, green a farther, softer layer. Its first row is the top.
pub fn leaves() -> Vec<u8> {
    let left = std::f64::consts::PI;
    let layer = |blur: f32, seed: u32, branches: &[Branch]| {
        let mut l = Layer { alpha: vec![0.0; LEAVES * LEAVES], sigma: blur / 2.0 };
        let mut rand = Random(seed);
        for b in branches {
            l.branch(&mut rand, b);
        }
        l.alpha
    };
    let near = layer(
        16.0,
        7,
        &[
            Branch { x: 1.06, y: 0.58, angle: left + 0.28, length: 0.9, bend: 0.32, width: 0.009, spacing: 0.085, leaf: 0.13, bare: 0.12 },
            Branch { x: 0.74, y: 0.47, angle: left + 0.95, length: 0.34, bend: -0.5, width: 0.005, spacing: 0.08, leaf: 0.115, bare: 0.2 },
        ],
    );
    let far = layer(34.0, 19, &[Branch { x: 1.08, y: 0.3, angle: left + 0.12, length: 0.5, bend: 0.4, width: 0.008, spacing: 0.1, leaf: 0.15, bare: 0.12 }]);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    near.iter().zip(&far).flat_map(|(n, f)| [byte(*n), byte(*f), 0, 255]).collect()
}

/// The glyphs' cell in the atlas, and the characters from faint to dense.
pub const CELL: [usize; 2] = [60, 100];
pub const CHARACTERS: &str = " =+xX0S8#@";

struct Pen {
    path: PathBuilder,
    x: f32,
    y: f32,
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x, self.y - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x, self.y - y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(self.x + cx, self.y - cy, self.x + x, self.y - y);
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.path.cubic_to(self.x + ax, self.y - ay, self.x + bx, self.y - by, self.x + x, self.y - y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// The glyphs' coverage in one row, with every smaller level (each half the one before): width, height, bytes.
pub fn glyphs() -> Vec<(usize, usize, Vec<u8>)> {
    let count = CHARACTERS.chars().count();
    let (w, h) = (CELL[0] * count, CELL[1]);
    let mut mask = Mask::new(w as u32, h as u32).expect("the atlas's size");
    if let Some(bytes) = crate::fonts::glyph_font() {
        if let Ok(font) = FontRef::from_index(bytes.as_ref().as_ref(), 0) {
            const PX: f32 = 78.0;
            // A canvas's "middle": halfway between the typographic ascent and descent, as shares of the em.
            let (up, down) = font.os2().map_or((0.75, 0.25), |t| {
                let (a, d) = (f32::from(t.s_typo_ascender()), -f32::from(t.s_typo_descender()));
                if a + d > 0.0 { (a / (a + d), d / (a + d)) } else { (0.75, 0.25) }
            });
            let widths = font.glyph_metrics(Size::new(PX), LocationRef::default());
            let outlines = font.outline_glyphs();
            for (i, c) in CHARACTERS.chars().enumerate() {
                let Some(id) = font.charmap().map(c) else { continue };
                let Some(outline) = outlines.get(id) else { continue };
                let x = (i * CELL[0]) as f32 + CELL[0] as f32 / 2.0 - widths.advance_width(id).unwrap_or(0.0) / 2.0;
                let y = CELL[1] as f32 / 2.0 + 3.0 + (up - down) / 2.0 * PX;
                let mut pen = Pen { path: PathBuilder::new(), x, y };
                if outline.draw(DrawSettings::unhinted(Size::new(PX), LocationRef::default()), &mut pen).is_ok() {
                    if let Some(path) = pen.path.finish() {
                        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
                    }
                }
            }
        }
    }
    let mut levels = vec![(w, h, mask.data().to_vec())];
    while levels.last().is_some_and(|(w, h, _)| *w > 1 || *h > 1) {
        let (pw, ph, from) = levels.last().expect("a level");
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut to = vec![0u8; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let (x0, x1) = (x * pw / nw, ((x + 1) * pw).div_ceil(nw).min(*pw));
                let (y0, y1) = (y * ph / nh, ((y + 1) * ph).div_ceil(nh).min(*ph));
                let mut sum = 0u32;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        sum += u32::from(from[sy * pw + sx]);
                    }
                }
                to[y * nw + x] = (sum / ((x1 - x0) * (y1 - y0)).max(1) as u32) as u8;
            }
        }
        levels.push((nw, nh, to));
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_random_numbers_are_the_website_s() {
        // `random(7)` and `random(19)` in Node, `leafShadowTexture.js`.
        let mut r = Random(7);
        let first: Vec<f64> = (0..3).map(|_| r.next()).collect();
        assert!(first.iter().all(|v| (0.0..1.0).contains(v)));
        assert!((first[0] - 0.011_704_753_153_026_104).abs() < 1e-12, "{first:?}");
    }

    #[test]
    fn foliage_and_glyphs_are_drawn() {
        let l = leaves();
        assert_eq!(l.len(), LEAVES * LEAVES * 4);
        let red = l.chunks(4).filter(|p| p[0] > 128).count();
        let green = l.chunks(4).filter(|p| p[1] > 128).count();
        assert!(red > 20_000 && green > 4_000, "{red} {green}");
        let g = glyphs();
        assert_eq!((g[0].0, g[0].1), (600, 100));
        assert_eq!(g.last().map(|l| (l.0, l.1)), Some((1, 1)));
        // A space is empty; each later glyph has ink (where this computer has a fixed-width font).
        let ink = |i: usize| (0..100).flat_map(|y| (0..60).map(move |x| y * 600 + i * 60 + x)).map(|k| u32::from(g[0].2[k])).sum::<u32>();
        assert_eq!(ink(0), 0);
        if crate::fonts::glyph_font().is_some() {
            assert!((1..10).all(|i| ink(i) > 10_000));
        }
    }
}
