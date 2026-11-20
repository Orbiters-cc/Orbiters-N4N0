//! The test set of K-qr-detect: Enzo's three codes cut from his headset pictures, and codes made here the way people style
//! them in VRChat (a logo in the middle, colours, light on dark, a gradient, low contrast, at an angle, blurred, small,
//! cut by the picture's edge), drawn into frames as large as VRChat's stream camera sends (1920 × 1080) over a busy
//! poster-like world and read through the scanner's own grey copies (the whole picture, then its middle).
//!
//! `cargo test -p orbiters-qr --release -- --nocapture` prints the recognition rate of each kind of code before (the
//! reader of 0.3.8-dev.1: `rqrr`'s four passes, [`PASSES_BEFORE_ZXING`]) and after ([`read`], ZXing's pass first), and
//! what a look costs. `QR_SAMPLES=<folder>` saves a frame of each.
//!
//! Request 18 added the codes Enzo's 2026-11-20 session did not read: dots for modules and rounded finders (5-qr-a), posters
//! seen at a steep angle, frames softened and blocky as the camera's stream is, and his two pictures of them.

use super::*;
use std::time::Instant;

const W: usize = 1920;
const H: usize = 1080;

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
}

type Rgb = [f32; 3];

/// A frame being drawn: RGB, 0 to 255.
struct Canvas {
    px: Vec<Rgb>,
}

impl Canvas {
    /// A busy world: a gradient, posters of every colour, and text-like bars (which look a little like finder patterns).
    fn world(rng: &mut Rng) -> Self {
        let a = [rng.range(10.0, 120.0), rng.range(10.0, 120.0), rng.range(10.0, 120.0)];
        let b = [rng.range(10.0, 200.0), rng.range(10.0, 200.0), rng.range(10.0, 200.0)];
        let mut px = vec![[0.0; 3]; W * H];
        for y in 0..H {
            let t = y as f32 / H as f32;
            for x in 0..W {
                let n = ((x * 7 + y * 13) % 17) as f32 - 8.0;
                px[y * W + x] = [a[0] + (b[0] - a[0]) * t + n, a[1] + (b[1] - a[1]) * t + n, a[2] + (b[2] - a[2]) * t + n];
            }
        }
        let mut canvas = Self { px };
        for _ in 0..30 {
            let (x0, y0) = (rng.range(0.0, W as f32) as usize, rng.range(0.0, H as f32) as usize);
            let (w, h) = (rng.range(40.0, 500.0) as usize, rng.range(40.0, 400.0) as usize);
            let c = [rng.range(0.0, 255.0), rng.range(0.0, 255.0), rng.range(0.0, 255.0)];
            canvas.fill(x0, y0, w, h, c);
        }
        for _ in 0..400 {
            let (x0, y0) = (rng.range(0.0, W as f32) as usize, rng.range(0.0, H as f32) as usize);
            let (w, h) = (rng.range(2.0, 30.0) as usize, rng.range(2.0, 14.0) as usize);
            let v = if rng.f() < 0.5 { rng.range(0.0, 50.0) } else { rng.range(200.0, 255.0) };
            canvas.fill(x0, y0, w, h, [v, v, v]);
        }
        canvas
    }

    fn fill(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Rgb) {
        for y in y0..(y0 + h).min(H) {
            for x in x0..(x0 + w).min(W) {
                self.px[y * W + x] = c;
            }
        }
    }

    /// A box blur of `radius` over the rectangle (twice, nearly a Gaussian).
    fn blur(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, radius: usize) {
        if radius == 0 {
            return;
        }
        for _ in 0..2 {
            for horizontal in [true, false] {
                let copy = self.px.clone();
                for y in y0..y1 {
                    for x in x0..x1 {
                        let mut s = [0.0f32; 3];
                        let mut n = 0.0;
                        for d in -(radius as isize)..=radius as isize {
                            let (sx, sy) = if horizontal { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                            if sx >= 0 && sy >= 0 && (sx as usize) < W && (sy as usize) < H {
                                let p = copy[sy as usize * W + sx as usize];
                                s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
                                n += 1.0;
                            }
                        }
                        self.px[y * W + x] = [s[0] / n, s[1] / n, s[2] / n];
                    }
                }
            }
        }
    }

    /// What the camera's stream does to a picture (VRChat's camera through Spout, then the grey copy): softened, its colour
    /// averaged over blocks of 8 × 8 pixels about each pixel's brightness (as a video encoder keeps brightness and halves
    /// colour), steps of brightness in 6 levels, a little noise.
    fn stream(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, rng: &mut Rng) {
        self.blur(x0, y0, x1, y1, 1);
        let luma = |p: Rgb| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        for by in (y0..y1).step_by(8) {
            for bx in (x0..x1).step_by(8) {
                let (ex, ey) = ((bx + 8).min(x1), (by + 8).min(y1));
                let mut mean = [0.0f32; 3];
                let mut count = 0.0;
                for y in by..ey {
                    for x in bx..ex {
                        let p = self.px[y * W + x];
                        mean = [mean[0] + p[0], mean[1] + p[1], mean[2] + p[2]];
                        count += 1.0;
                    }
                }
                let mean = mean.map(|v| v / count);
                let mean_luma = luma(mean);
                for y in by..ey {
                    for x in bx..ex {
                        let l = ((luma(self.px[y * W + x]) / 6.0).round() * 6.0 + rng.range(-5.0, 5.0)).clamp(0.0, 255.0);
                        self.px[y * W + x] = mean.map(|c| (c + l - mean_luma).clamp(0.0, 255.0));
                    }
                }
            }
        }
    }

    /// As Spout lends it: BGRA.
    fn bgra(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| [p[2].clamp(0.0, 255.0) as u8, p[1].clamp(0.0, 255.0) as u8, p[0].clamp(0.0, 255.0) as u8, 255]).collect()
    }
}

/// How a code is styled and placed.
#[derive(Clone, Debug)]
struct Style {
    text: String,
    /// Error correction H (posters with a logo use it).
    high: bool,
    /// Pixels a module in the 1920-wide frame (at the code's middle, before the angle).
    module: f32,
    /// The colour of the code's dark modules (light in a code in negative) and of its ground.
    on: Rgb,
    off: Rgb,
    /// The modules' colour at the code's bottom (a gradient from `on`).
    on_bottom: Option<Rgb>,
    /// A picture in the middle, this part of the code's side (0: none).
    logo: f32,
    quiet: f32,
    /// Turned about the vertical, the horizontal and the view axis, in degrees.
    yaw: f32,
    pitch: f32,
    roll: f32,
    blur: usize,
    centre: (f32, f32),
    /// Dots for modules and rounded finders, as code generators style them.
    dots: bool,
    /// The stream's softness and blocks (8 × 8 averaging of the colours over a softened picture) and its noise.
    stream: bool,
}

/// A 3 × 3 homography from 4 points to 4 points.
fn homography(from: [(f32, f32); 4], to: [(f32, f32); 4]) -> [f64; 9] {
    let mut a = [[0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (from[i].0 as f64, from[i].1 as f64);
        let (u, v) = (to[i].0 as f64, to[i].1 as f64);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for c in 0..8 {
        let p = (c..8).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs())).unwrap();
        a.swap(c, p);
        for r in 0..8 {
            if r != c {
                let f = a[r][c] / a[c][c];
                let pivot = a[c];
                for (k, v) in a[r].iter_mut().enumerate().skip(c) {
                    *v -= f * pivot[k];
                }
            }
        }
    }
    let h: Vec<f64> = (0..8).map(|i| a[i][8] / a[i][i]).collect();
    [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]
}

fn apply(m: &[f64; 9], x: f32, y: f32) -> (f32, f32) {
    let (x, y) = (x as f64, y as f64);
    let w = m[6] * x + m[7] * y + m[8];
    (((m[0] * x + m[1] * y + m[2]) / w) as f32, ((m[3] * x + m[4] * y + m[5]) / w) as f32)
}

/// Draws `style`'s code into `canvas`.
fn draw(canvas: &mut Canvas, style: &Style, rng: &mut Rng) {
    let level = if style.high { qrcode::EcLevel::H } else { qrcode::EcLevel::M };
    let code = qrcode::QrCode::with_error_correction_level(style.text.as_bytes(), level).unwrap();
    let n = code.width();
    let colors = code.to_colors();
    let side = n as f32 + 2.0 * style.quiet;
    // The code's corners in the frame: turned in 3D, seen by a camera 1500 pixels from its picture.
    let (yaw, pitch, roll) = (style.yaw.to_radians(), style.pitch.to_radians(), style.roll.to_radians());
    let half = side * style.module / 2.0;
    let corners = [(-half, -half), (half, -half), (half, half), (-half, half)].map(|(x, y)| {
        let (x, y) = (x * roll.cos() - y * roll.sin(), x * roll.sin() + y * roll.cos());
        let (x, z) = (x * yaw.cos(), x * yaw.sin());
        let (y, z) = (y * pitch.cos(), z + y * pitch.sin());
        let f = 1500.0;
        (style.centre.0 + f * x / (f + z), style.centre.1 + f * y / (f + z))
    });
    let to_code = homography(corners, [(0.0, 0.0), (side, 0.0), (side, side), (0.0, side)]);
    let xs = corners.map(|c| c.0);
    let ys = corners.map(|c| c.1);
    let x0 = xs.iter().cloned().fold(f32::MAX, f32::min).max(0.0) as usize;
    let x1 = (xs.iter().cloned().fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(W);
    let y0 = ys.iter().cloned().fold(f32::MAX, f32::min).max(0.0) as usize;
    let y1 = (ys.iter().cloned().fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(H);
    let logo_cells = (style.logo * n as f32).round() as isize;
    let logo_from = (n as isize - logo_cells) / 2;
    let palette: Vec<Rgb> = (0..6).map(|_| [rng.range(0.0, 255.0), rng.range(0.0, 255.0), rng.range(0.0, 255.0)]).collect();
    let colour = |u: f32, v: f32| -> Option<Rgb> {
        if !(0.0..side).contains(&u) || !(0.0..side).contains(&v) {
            return None;
        }
        let (mx, my) = ((u - style.quiet).floor() as isize, (v - style.quiet).floor() as isize);
        if mx < 0 || my < 0 || mx >= n as isize || my >= n as isize {
            return Some(style.off);
        }
        if logo_cells > 0 && (logo_from..logo_from + logo_cells).contains(&mx) && (logo_from..logo_from + logo_cells).contains(&my) {
            // A white rim, then a picture of coloured patches.
            if mx == logo_from || my == logo_from || mx == logo_from + logo_cells - 1 || my == logo_from + logo_cells - 1 {
                return Some(style.off);
            }
            let k = ((u * 0.7).floor() as usize * 31 + (v * 0.7).floor() as usize * 17) % palette.len();
            return Some(palette[k]);
        }
        if style.dots {
            // Finders: rounded squares (a ring 7 across, a core 3 across); other modules: dots.
            let (fu, fv) = (u - style.quiet, v - style.quiet);
            let finder = [(0.0, 0.0), (n as f32 - 7.0, 0.0), (0.0, n as f32 - 7.0)].into_iter().find(|(x, y)| (*x..*x + 7.0).contains(&fu) && (*y..*y + 7.0).contains(&fv));
            let ink = |dark: bool| Some(if dark { style.on } else { style.off });
            if let Some((x, y)) = finder {
                let (lx, ly) = (fu - x - 3.5, fv - y - 3.5);
                // Inside a rounded square of half side `half` and corner radius `r`.
                let inside = |half: f32, r: f32| {
                    let (qx, qy) = (lx.abs() - (half - r), ly.abs() - (half - r));
                    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) <= r
                };
                return ink((inside(3.5, 1.6) && !inside(2.5, 1.0)) || inside(1.5, 0.7));
            }
            let dark = colors[my as usize * n + mx as usize] == qrcode::Color::Dark;
            let (cx, cy) = (fu - mx as f32 - 0.5, fv - my as f32 - 0.5);
            return ink(dark && cx.hypot(cy) <= 0.42);
        }
        if colors[my as usize * n + mx as usize] == qrcode::Color::Dark {
            let t = v / side;
            Some(style.on_bottom.map_or(style.on, |b| [style.on[0] + (b[0] - style.on[0]) * t, style.on[1] + (b[1] - style.on[1]) * t, style.on[2] + (b[2] - style.on[2]) * t]))
        } else {
            Some(style.off)
        }
    };
    for y in y0..y1 {
        for x in x0..x1 {
            // 3 × 3 samples a pixel: the edges of modules are mixed as a camera's are.
            let mut s = [0.0f32; 3];
            let mut hits = 0;
            for sy in 0..3 {
                for sx in 0..3 {
                    let (u, v) = apply(&to_code, x as f32 + (sx as f32 + 0.5) / 3.0, y as f32 + (sy as f32 + 0.5) / 3.0);
                    let c = colour(u, v).unwrap_or(canvas.px[y * W + x]);
                    hits += 1;
                    s = [s[0] + c[0], s[1] + c[1], s[2] + c[2]];
                }
            }
            let n = hits as f32;
            canvas.px[y * W + x] = [s[0] / n, s[1] / n, s[2] / n];
        }
    }
    let margin = style.blur * 3;
    canvas.blur(x0.saturating_sub(margin), y0.saturating_sub(margin), (x1 + margin).min(W), (y1 + margin).min(H), style.blur);
    if style.stream {
        canvas.stream(x0.saturating_sub(16), y0.saturating_sub(16), (x1 + 16).min(W), (y1 + 16).min(H), rng);
    }
}

/// The kinds of code in the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Plain,
    Logo,
    ColourLogo,
    Negative,
    NegativeGradientPicture,
    LowContrast,
    Angle,
    Blur,
    Small,
    SmallMiddle,
    PosterMix,
    /// Dots for modules, rounded finders.
    Dots,
    /// Enzo's 5-qr-a: dots and rounded finders, a logo, dark modules on a yellow-green card, at an angle, through the stream.
    DotsPoster,
    /// A poster seen at a steep angle (50 to 62 degrees), as a camera held beside it sees it.
    Steep,
    /// A plain or negative code through the stream's softness and blocks.
    Stream,
    Cut,
    Nothing,
}

const KINDS: [Kind; 17] = [
    Kind::Plain,
    Kind::Logo,
    Kind::ColourLogo,
    Kind::Negative,
    Kind::NegativeGradientPicture,
    Kind::LowContrast,
    Kind::Angle,
    Kind::Blur,
    Kind::Small,
    Kind::SmallMiddle,
    Kind::PosterMix,
    Kind::Dots,
    Kind::DotsPoster,
    Kind::Steep,
    Kind::Stream,
    Kind::Cut,
    Kind::Nothing,
];

fn link(rng: &mut Rng) -> String {
    let hosts = ["https://vrcidlinker.com/", "https://discord.gg/", "https://orbiters.cc/w/", "https://twitch.tv/", "https://linktr.ee/", "https://booth.pm/en/items/"];
    let len = rng.range(4.0, 40.0) as usize;
    let tail: String = (0..len).map(|_| (b'a' + (rng.next() % 26) as u8) as char).collect();
    format!("{}{tail}", rng.pick(&hosts))
}

/// A dark colour, a light one, at least `contrast` (0 to 1) apart in luma.
fn pair(rng: &mut Rng, contrast: f32) -> (Rgb, Rgb) {
    loop {
        let dark = [rng.range(0.0, 140.0), rng.range(0.0, 140.0), rng.range(0.0, 140.0)];
        let light = [rng.range(110.0, 255.0), rng.range(110.0, 255.0), rng.range(110.0, 255.0)];
        let luma = |c: Rgb| (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]) / 255.0;
        let d = luma(light) - luma(dark);
        if d >= contrast && d <= contrast + 0.25 {
            return (dark, light);
        }
    }
}

fn style(kind: Kind, rng: &mut Rng) -> Style {
    let mut s = Style {
        text: link(rng),
        high: false,
        module: rng.range(5.0, 9.0),
        on: [15.0, 15.0, 20.0],
        off: [240.0, 240.0, 235.0],
        on_bottom: None,
        logo: 0.0,
        quiet: rng.range(1.0, 4.0),
        yaw: rng.range(-12.0, 12.0),
        pitch: rng.range(-12.0, 12.0),
        roll: rng.range(-10.0, 10.0),
        blur: 0,
        centre: (rng.range(500.0, 1420.0), rng.range(330.0, 750.0)),
        dots: false,
        stream: false,
    };
    match kind {
        Kind::Plain | Kind::Nothing => {}
        Kind::Logo => {
            s.high = true;
            s.logo = rng.range(0.2, 0.3);
        }
        Kind::ColourLogo => {
            s.high = true;
            s.logo = rng.range(0.2, 0.3);
            (s.on, s.off) = pair(rng, 0.4);
        }
        Kind::Negative => {
            let (dark, light) = pair(rng, 0.45);
            (s.on, s.off) = (light, dark);
            s.high = rng.f() < 0.5;
            s.logo = if s.high { rng.range(0.15, 0.28) } else { 0.0 };
        }
        Kind::NegativeGradientPicture => {
            // Enzo's shot2: cyan to magenta modules on black, a picture in the middle.
            s.high = true;
            s.logo = rng.range(0.2, 0.3);
            s.on = [rng.range(40.0, 90.0), rng.range(200.0, 240.0), rng.range(200.0, 240.0)];
            s.on_bottom = Some([rng.range(200.0, 240.0), rng.range(30.0, 70.0), rng.range(200.0, 240.0)]);
            s.off = [rng.range(5.0, 25.0), rng.range(5.0, 25.0), rng.range(10.0, 30.0)];
        }
        Kind::LowContrast => {
            (s.on, s.off) = pair(rng, 0.22);
            if rng.f() < 0.5 {
                (s.on, s.off) = (s.off, s.on);
            }
        }
        Kind::Angle => {
            s.yaw = rng.range(35.0, 55.0) * if rng.f() < 0.5 { -1.0 } else { 1.0 };
            s.pitch = rng.range(-25.0, 25.0);
            s.module = rng.range(6.0, 10.0);
        }
        Kind::Blur => {
            s.module = rng.range(5.0, 8.0);
            s.blur = rng.pick(&[1, 2]);
        }
        Kind::Small => {
            s.module = rng.range(2.6, 3.6);
            s.centre = (rng.range(150.0, 1770.0), rng.range(150.0, 930.0));
        }
        Kind::SmallMiddle => {
            s.module = rng.range(1.6, 2.4);
            s.centre = (rng.range(800.0, 1120.0), rng.range(420.0, 660.0));
        }
        Kind::PosterMix => {
            // Enzo's shot1 and what posters do at once: light on a coloured dark ground, a logo, an angle, a little blur.
            let (dark, light) = pair(rng, 0.4);
            (s.on, s.off) = (light, dark);
            s.high = true;
            s.logo = rng.range(0.18, 0.28);
            s.yaw = rng.range(20.0, 40.0) * if rng.f() < 0.5 { -1.0 } else { 1.0 };
            s.pitch = rng.range(-20.0, 20.0);
            s.blur = rng.pick(&[0, 1]);
            s.module = rng.range(5.0, 9.0);
        }
        Kind::Dots => {
            s.dots = true;
            s.module = rng.range(5.0, 9.0);
        }
        Kind::DotsPoster => {
            s.dots = true;
            s.high = true;
            s.logo = rng.range(0.15, 0.22);
            s.on = [rng.range(30.0, 70.0), rng.range(10.0, 40.0), rng.range(60.0, 110.0)];
            s.off = [rng.range(170.0, 210.0), rng.range(220.0, 245.0), rng.range(20.0, 70.0)];
            s.yaw = rng.range(15.0, 35.0) * if rng.f() < 0.5 { -1.0 } else { 1.0 };
            s.pitch = rng.range(-20.0, 20.0);
            s.roll = rng.range(-15.0, 15.0);
            s.module = rng.range(5.0, 9.0);
            s.stream = true;
        }
        Kind::Steep => {
            s.yaw = rng.range(50.0, 62.0) * if rng.f() < 0.5 { -1.0 } else { 1.0 };
            s.pitch = rng.range(-15.0, 15.0);
            s.module = rng.range(7.0, 11.0);
        }
        Kind::Stream => {
            if rng.f() < 0.5 {
                let (dark, light) = pair(rng, 0.45);
                (s.on, s.off) = (light, dark);
            }
            s.module = rng.range(4.0, 7.0);
            s.stream = true;
        }
        Kind::Cut => {
            // A third of it outside the picture, on any side.
            s.module = rng.range(6.0, 10.0);
            let n = qrcode::QrCode::new(s.text.as_bytes()).unwrap().width() as f32;
            let size = (n + 2.0 * s.quiet) * s.module;
            let out = size * rng.range(0.25, 0.4) - size / 2.0;
            s.centre = match rng.next() % 4 {
                0 => (-out, rng.range(300.0, 780.0)),
                1 => (W as f32 + out, rng.range(300.0, 780.0)),
                2 => (rng.range(400.0, 1520.0), -out),
                _ => (rng.range(400.0, 1520.0), H as f32 + out),
            };
        }
    }
    s
}

/// What the scanner read from a frame, before and after: (before, after, glimpse of the whole picture after).
fn look(bgra: &[u8]) -> (Vec<String>, Vec<String>, Option<Glimpse>) {
    let frame = Pixels { width: W, height: H, stride: W * 4, data: bgra, bgra: true };
    // The scanner's three looks of a second, with 0.3.8-dev.1's passes (before) and now's (after).
    let three = |mut looks: Looks| {
        let mut codes = Vec::new();
        for _ in 0..3 {
            let copy = looks.copy(&frame);
            codes.extend(looks.read(&copy).codes);
        }
        codes.dedup();
        (codes, looks.glimpse)
    };
    let (before, _) = three(Looks::with_passes(&PASSES_BEFORE_ZXING));
    let (after, glimpse) = three(Looks::default());
    (before, after, glimpse)
}

fn save(path: &std::path::Path, bgra: &[u8]) {
    let rgba: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), W as u32, H as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgba).unwrap();
}

/// Each kind's tally.
#[derive(Default, Debug)]
struct Tally {
    cases: u32,
    before: u32,
    after: u32,
    cut: u32,
    /// Cut codes said cut off the side they were placed past.
    cut_side_right: u32,
    unclear: u32,
    wrong: u32,
}

const PER_KIND: u32 = 12;

fn run_set() -> Vec<(Kind, Tally)> {
    let samples = std::env::var_os("QR_SAMPLES").map(std::path::PathBuf::from);
    let mut out = Vec::new();
    for (k, kind) in KINDS.into_iter().enumerate() {
        let mut tally = Tally::default();
        for i in 0..PER_KIND {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ ((k as u64) << 32 | i as u64).wrapping_mul(0x10_0000_01B3));
            for _ in 0..4 {
                rng.next();
            }
            let mut canvas = Canvas::world(&mut rng);
            let s = style(kind, &mut rng);
            if kind != Kind::Nothing {
                draw(&mut canvas, &s, &mut rng);
            }
            let bgra = canvas.bgra();
            if let Some(dir) = &samples {
                if i < 2 {
                    std::fs::create_dir_all(dir).unwrap();
                    save(&dir.join(format!("{kind:?}-{i}.png")), &bgra);
                }
            }
            let (before, after, glimpse) = look(&bgra);
            // A cut code is said cut, off the side it was placed past.
            // (`style` puts a cut code's middle within about 120 pixels of the side it runs past.)
            let placed_side = if s.centre.0 < 150.0 { Side::Left } else if s.centre.0 > W as f32 - 150.0 { Side::Right } else if s.centre.1 < 150.0 { Side::Top } else { Side::Bottom };
            let failed = !after.contains(&s.text) && kind != Kind::Cut && kind != Kind::Nothing || (kind == Kind::Nothing && glimpse.is_some()) || (kind == Kind::Cut && glimpse != Some(Glimpse::Cut(Some(placed_side))));
            if failed && std::env::var_os("QR_DEBUG").is_some() {
                let frame = Pixels { width: W, height: H, stride: W * 4, data: &bgra, bgra: true };
                let (ww, wh, whole) = grey(&frame, View::Whole);
                let classes = classify(ww, wh, &whole);
                for dark in [-1i8, 1] {
                    println!("    finders of {dark}: {:?}", finders(ww, wh, &classes, dark));
                }
            }
            if failed {
                println!("  {kind:?} {i}: not as wanted: glimpse {glimpse:?}, module {:.1}, logo {:.2}, high {}, angle {:.0}/{:.0}/{:.0}, blur {}, at {:?}, text length {}", s.module, s.logo, s.high, s.yaw, s.pitch, s.roll, s.blur, s.centre, s.text.len());
                if let Some(dir) = &samples {
                    std::fs::create_dir_all(dir).unwrap();
                    save(&dir.join(format!("failed-{kind:?}-{i}.png")), &bgra);
                }
            }
            tally.cases += 1;
            let wanted = |codes: &[String]| codes.contains(&s.text);
            tally.before += wanted(&before) as u32;
            tally.after += wanted(&after) as u32;
            tally.wrong += after.iter().any(|c| *c != s.text) as u32;
            if after.is_empty() {
                match glimpse {
                    Some(Glimpse::Cut(side)) => {
                        tally.cut += 1;
                        tally.cut_side_right += (kind == Kind::Cut && side == Some(placed_side)) as u32;
                    }
                    Some(Glimpse::Unclear) => tally.unclear += 1,
                    None => {}
                }
            }
        }
        println!(
            "{kind:?}: {} frames, read before {} ({:.0} %), after {} ({:.0} %); not read but seen: cut {} (its side right {}), unclear {}; wrong text {}",
            tally.cases,
            tally.before,
            100.0 * tally.before as f32 / tally.cases as f32,
            tally.after,
            100.0 * tally.after as f32 / tally.cases as f32,
            tally.cut,
            tally.cut_side_right,
            tally.unclear,
            tally.wrong
        );
        out.push((kind, tally));
    }
    out
}

/// Enzo's shot1 and shot2 were never read in his headset (both light modules on a dark ground); shot3 was.
#[test]
fn the_test_set_is_read() {
    let set = run_set();
    let get = |k: Kind| &set.iter().find(|(kind, _)| *kind == k).unwrap().1;
    let (codes, before, after) = set
        .iter()
        .filter(|(k, _)| !matches!(k, Kind::Cut | Kind::Nothing))
        .fold((0, 0, 0), |(c, b, a), (_, t)| (c + t.cases, b + t.before, a + t.after));
    println!("all codes that can be read: {codes} frames, before {before} ({:.0} %), after {after} ({:.0} %)", 100.0 * before as f32 / codes as f32, 100.0 * after as f32 / codes as f32);
    // Nothing is ever read wrong, and a frame with no code reads none.
    assert!(set.iter().all(|(_, t)| t.wrong == 0), "a wrong text was read");
    assert_eq!(get(Kind::Nothing).after, 0);
    // Every kind of code below is read in every frame (2026-11-15: all but one Logo frame, which ZXing reads).
    for kind in [Kind::Plain, Kind::Logo, Kind::ColourLogo, Kind::Negative, Kind::NegativeGradientPicture, Kind::LowContrast, Kind::Angle, Kind::Blur, Kind::PosterMix, Kind::Dots, Kind::Stream] {
        assert_eq!(get(kind).after, PER_KIND, "{kind:?}: {:?}", get(kind));
    }
    // Request 18: Enzo's kind of poster (dots, rounded finders, a logo, an angle, the stream) and steep angles.
    for kind in [Kind::DotsPoster, Kind::Steep] {
        assert!(get(kind).after * 12 >= PER_KIND * 11, "{kind:?}: {:?}", get(kind));
        assert!(get(kind).after >= get(kind).before, "{kind:?} is read no less often than by 0.3.8-dev.1: {:?}", get(kind));
    }
    // Measured 2026-11-20: dots and rounded finders were never read before (0 of 12), Enzo's kind of poster 4 of 12.
    assert!(get(Kind::Dots).before < get(Kind::Dots).after && get(Kind::DotsPoster).before < get(Kind::DotsPoster).after);
    // Small codes (1.3 to 1.8 pixels a module in the whole picture's grey copy) are read in a closer look where they
    // were glimpsed; tinier ones in the middle (1.6 to 2.4 pixels a module in the camera's full frame) only half the time.
    assert!(get(Kind::Small).after * 4 >= PER_KIND * 3, "{:?}", get(Kind::Small));
    assert!(get(Kind::SmallMiddle).after >= get(Kind::SmallMiddle).before, "{:?}", get(Kind::SmallMiddle));
    assert!(after * 100 >= codes * 90, "{after} of {codes}");
    // A code cut by the frame cannot be read, but is seen: the person is told to bring all of it in.
    let cut = get(Kind::Cut);
    assert_eq!(cut.after, 0);
    assert_eq!(cut.cut + cut.unclear, PER_KIND, "every cut code is seen: {cut:?}");
    assert!(cut.cut * 4 >= PER_KIND * 3, "cut codes said cut: {cut:?}");
    assert_eq!(cut.cut_side_right, cut.cut, "each said cut off the side it runs past: {cut:?}");
    // A world with no code shows none, not even a glimpse of one.
    assert_eq!(get(Kind::Nothing).cut + get(Kind::Nothing).unclear, 0, "{:?}", get(Kind::Nothing));
}

fn load_png(path: &std::path::Path) -> (usize, usize, Vec<Rgb>) {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    let px = buf[..info.buffer_size()].chunks(channels).map(|p| if channels >= 3 { [p[0] as f32, p[1] as f32, p[2] as f32] } else { [p[0] as f32; 3] }).collect();
    (info.width as usize, info.height as usize, px)
}

/// Enzo's codes, cut from his pictures of the headset (Virtual Desktop's compression and the panel's scaling make them far
/// blurrier than the camera's own frames), enlarged into a stream frame to about the size they had there. shot4 and shot5
/// are the two of Request 18 (2026-11-20, 5-qr-a and 5-qr-b): shot5 is shot1's poster again (his log: 25 s of looks before
/// it was read); shot4, dots and rounded finders round a logo, was never read (nor by OpenCV's reader from his picture).
#[test]
fn enzos_codes() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let mut results = Vec::new();
    for name in ["shot1", "shot2", "shot3", "shot4", "shot5"] {
        let (cw, ch, crop) = load_png(&dir.join(format!("{name}.png")));
        for scale in [3.0f32, 4.5, 6.0, 7.0, 8.0] {
            // Larger than the frame: not a size the camera saw it at.
            if (cw.max(ch) as f32 * scale) as usize >= H {
                continue;
            }
            let mut canvas = Canvas { px: vec![crop[0]; W * H] };
            let (ow, oh) = ((cw as f32 * scale) as usize, (ch as f32 * scale) as usize);
            let (x0, y0) = ((W - ow) / 2, (H - oh) / 2);
            for y in 0..oh {
                for x in 0..ow {
                    // Bilinear, as a camera's picture is smooth.
                    let (fx, fy) = ((x as f32 + 0.5) / scale - 0.5, (y as f32 + 0.5) / scale - 0.5);
                    let (ix, iy) = (fx.floor().clamp(0.0, (cw - 2) as f32) as usize, fy.floor().clamp(0.0, (ch - 2) as f32) as usize);
                    let (tx, ty) = ((fx - ix as f32).clamp(0.0, 1.0), (fy - iy as f32).clamp(0.0, 1.0));
                    let p = |dx: usize, dy: usize| crop[(iy + dy) * cw + ix + dx];
                    let mut c = [0.0; 3];
                    for (k, v) in c.iter_mut().enumerate() {
                        *v = p(0, 0)[k] * (1.0 - tx) * (1.0 - ty) + p(1, 0)[k] * tx * (1.0 - ty) + p(0, 1)[k] * (1.0 - tx) * ty + p(1, 1)[k] * tx * ty;
                    }
                    canvas.px[(y0 + y) * W + x0 + x] = c;
                }
            }
            let (before, after, glimpse) = look(&canvas.bgra());
            println!("{name} x{scale}: before {before:?}, after {after:?}, glimpse {glimpse:?}");
            results.push((name, scale, before, after, glimpse));
        }
    }
    let reads = |shot: &str, after: bool| results.iter().filter(|(n, _, before_codes, after_codes, _)| *n == shot && !(if after { after_codes } else { before_codes }).is_empty()).count();
    for shot in ["shot1", "shot2", "shot3", "shot4", "shot5"] {
        println!("{shot}: read at {} sizes before, {} after", reads(shot, false), reads(shot, true));
        assert!(reads(shot, true) >= reads(shot, false), "{shot} is read no less often: {results:?}");
    }
    // Nothing is read wrong.
    let known = ["https://qrco.de/bdeKjG", "https://linktr.ee/the_real_nagia"];
    for (n, _, _, after, _) in &results {
        if matches!(*n, "shot1" | "shot5") {
            assert!(after.iter().all(|c| c == known[0]), "{n}: {after:?}");
        }
    }
    // shot1 and shot2 are read from his pictures of them; shot4, never read in his headset, is now.
    for (shot, text) in [("shot1", known[0]), ("shot2", known[1]), ("shot4", "https://udexreal.com/discount/Popcorn")] {
        assert!(results.iter().any(|(n, _, _, after, _)| *n == shot && after.iter().any(|c| c == text)), "{shot}: {results:?}");
    }
}

/// What a look costs: the dearest case is a picture with no code (every pass runs, then the finders are looked for).
#[test]
fn cost() {
    let mut rng = Rng(7);
    let world = Canvas::world(&mut rng).bgra();
    let mut canvas = Canvas::world(&mut rng);
    let mut s = style(Kind::Negative, &mut rng);
    s.high = false;
    s.logo = 0.0;
    draw(&mut canvas, &s, &mut rng);
    let negative = canvas.bgra();
    for (what, bgra) in [("no code", &world), ("a code in negative", &negative)] {
        let frame = Pixels { width: W, height: H, stride: W * 4, data: bgra, bgra: true };
        let rounds = 8;
        let t = Instant::now();
        for _ in 0..rounds {
            let (w, h, g) = grey(&frame, View::Whole);
            std::hint::black_box(read(w, h, &g, true));
        }
        let after = t.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        let t = Instant::now();
        for _ in 0..rounds {
            let (w, h, g) = grey(&frame, View::Whole);
            std::hint::black_box(read_with(w, h, &g, true, &PASSES_BEFORE_ZXING));
        }
        let before = t.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        let t = Instant::now();
        for _ in 0..rounds {
            let (w, h, g) = grey(&frame, View::Whole);
            std::hint::black_box(read_with(w, h, &g, true, &[Pass::Zxing]));
        }
        let zxing = t.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        println!("  ZXing's pass alone, grey copy included: {zxing:.1} ms");
        let t = Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(grey(&frame, View::Whole));
        }
        let copy = t.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        println!("a look at {what} (1920 x 1080, grey copy included): before {before:.1} ms, after {after:.1} ms (grey copy alone {copy:.1} ms)");
    }
}

#[test]
fn finders_of_a_plain_code_are_found_in_both_polarities() {
    let code = qrcode::QrCode::new(b"https://orbiters.cc").unwrap();
    let n = code.width();
    let colors = code.to_colors();
    let m = 6;
    let side = (n + 8) * m;
    for negative in [false, true] {
        let mut grey = vec![if negative { 20u8 } else { 230 }; side * side];
        for y in 0..n * m {
            for x in 0..n * m {
                if colors[(y / m) * n + x / m] == qrcode::Color::Dark {
                    grey[(y + 4 * m) * side + x + 4 * m] = if negative { 230 } else { 20 };
                }
            }
        }
        let classes = classify(side, side, &grey);
        let found = finders(side, side, &classes, if negative { 1 } else { -1 });
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found.iter().all(|f| (f.module - m as f32).abs() < 1.0), "{found:?}");
        let read = read(side, side, &grey, true);
        assert_eq!(read.codes, ["https://orbiters.cc"]);
    }
}
