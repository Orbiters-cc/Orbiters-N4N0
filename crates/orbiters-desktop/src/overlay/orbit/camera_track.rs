//! Where VRChat's camera panel is, in a headset, to lay tiles around it. VRChat says nothing of it: `/usercamera/Pose` is the camera's own place, which leaves its panel behind
//! once it flies (Enzo). What is always on the panel can be seen, as Enzo proposed: the white **X** at its top left and the white **chevron** under its middle, on VRChat's own
//! window, which shows what one eye sees. How far apart they are says how large it is, the chevron's direction how it is turned. Nothing touches the game.
//!
//! Only what is white is looked at, as blobs, and only one X and one chevron that lie as they do on the panel count: a white world or another X alone finds nothing, and
//! [`CameraWatch`](super::camera_frames) only looks while VRChat says its camera is open. **A wrong tile is worse than a late one.**
//!
//! The mirror's view is taken to be the whole width of the left eye, cropped in height. That and the panel's size in metres ([`PANEL_UNIT_M`]) are not measured in a headset yet.

use orbiters_openvr::Matrix34;

/// The panel in its own units, as a picture of it 410 across has them, from its middle: its X, and its chevron.
pub const PANEL_X: [f32; 2] = [-181.0, -83.0];
pub const PANEL_CHEVRON: [f32; 2] = [-8.0, 100.0];
pub const PANEL_SIZE: [f32; 2] = [410.0, 220.0];
/// A panel's unit in the room at VRChat's usual size: about 30 cm across. Only how far away it seems comes from this; where it shows comes from where it is seen.
pub const PANEL_UNIT_M: f32 = 0.30 / 410.0;

/// What counts as white, on each channel: an icon's middle on any world.
const WHITE: u8 = 215;
/// A glyph's size in the picture, in pixels at the size it is read at: smaller is a speck, larger a white wall.
const SMALLEST: f32 = 6.0;
const LARGEST: usize = 120;
/// How a glyph's size compares with how far apart the two are, from the panel as it is, with room for how a turn of it foreshortens.
const X_SHARE: (f32, f32) = (0.03, 0.12);
const CHEVRON_SHARE: (f32, f32) = (0.04, 0.2);
/// As Enzo's picture of the panel has them: how large each is against how far apart they are, and how much of its square the X fills.
const X_IS: f32 = 0.044;
const CHEVRON_IS: f32 = 0.065;
const X_FILLS: f32 = 0.58;
const CHEVRON_BENDS: f32 = 0.6;
/// How far the two are apart at the least, in pixels: below this they are too small to tell from anything.
const NEAREST: f32 = 80.0;
/// How far a panel may lean in the picture: a head on its side is not how a camera is used, and a row of icons lying any way is not a panel.
const STANDS_AT_LEAST: f32 = 0.5;
/// How across and how far down the X → chevron line runs, as the panel has it (173 across, 183 down), with room for a turn of it.
const ACROSS_PER_DOWN: (f32, f32) = (0.35, 1.25);

/// One frame's reading, in pixels of what was given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub x: [f32; 2],
    pub chevron: [f32; 2],
    /// The way down the panel, in the picture: y down, a unit long.
    pub down: [f32; 2],
}

#[derive(Clone, Debug)]
struct Blob {
    area: usize,
    min: [usize; 2],
    max: [usize; 2],
    middle: [f32; 2],
    /// Its long and short lengths, and the way of the long one.
    long: f32,
    short: f32,
    along: [f32; 2],
    /// Pixels of the picture to one pixel read.
    scale: f32,
    /// How bent it is: 0 for a straight bar, near 1 for a V, whichever way it opens.
    bend: f32,
}

/// The white blobs of a picture of `width` x `height` BGRA pixels, `stride` bytes a row, reading every `step`th pixel. In pixels of the picture.
fn blobs(bgra: &[u8], width: usize, height: usize, stride: usize, step: usize) -> Vec<Blob> {
    let (w, h) = (width / step, height / step);
    if w < 8 || h < 8 {
        return Vec::new();
    }
    let mut white = vec![false; w * h];
    for y in 0..h {
        let row = y * step * stride;
        for x in 0..w {
            let i = row + x * step * 4;
            if let Some(p) = bgra.get(i..i + 3) {
                white[y * w + x] = p[0] >= WHITE && p[1] >= WHITE && p[2] >= WHITE;
            }
        }
    }
    let mut out = Vec::new();
    let mut todo: Vec<usize> = Vec::new();
    let mut pixels: Vec<usize> = Vec::new();
    for start in 0..white.len() {
        if !white[start] {
            continue;
        }
        white[start] = false;
        todo.push(start);
        pixels.clear();
        let (mut min, mut max) = ([usize::MAX; 2], [0usize; 2]);
        while let Some(at) = todo.pop() {
            pixels.push(at);
            let (x, y) = (at % w, at / w);
            (min[0], min[1], max[0], max[1]) = (min[0].min(x), min[1].min(y), max[0].max(x), max[1].max(y));
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < w && ny < h && white[ny * w + nx] {
                    white[ny * w + nx] = false;
                    todo.push(ny * w + nx);
                }
            }
        }
        let wide = (max[0] - min[0] + 1).max(max[1] - min[1] + 1);
        if pixels.len() < 12 || wide > LARGEST {
            continue;
        }
        // Its shape: the spread of its pixels, whichever way it lies.
        let n = pixels.len() as f32;
        let (mut mx, mut my) = (0.0f32, 0.0f32);
        for at in &pixels {
            mx += (at % w) as f32;
            my += (at / w) as f32;
        }
        (mx, my) = (mx / n, my / n);
        let (mut xx, mut yy, mut xy) = (0.0f32, 0.0f32, 0.0f32);
        for at in &pixels {
            let (dx, dy) = ((at % w) as f32 - mx, (at / w) as f32 - my);
            xx += dx * dx;
            yy += dy * dy;
            xy += dx * dy;
        }
        (xx, yy, xy) = (xx / n, yy / n, xy / n);
        let middle = (xx + yy) / 2.0;
        let apart = (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
        let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
        let scale = step as f32;
        // How far out along its length goes with how far across: |t| against n, as a correlation.
        let (ca, sa) = (angle.cos(), angle.sin());
        let (mut sum_t, mut sum_n, mut tt, mut nn, mut tn) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for at in &pixels {
            let (dx, dy) = ((at % w) as f32 - mx, (at / w) as f32 - my);
            let (t, n) = ((dx * ca + dy * sa).abs(), dy * ca - dx * sa);
            sum_t += t;
            sum_n += n;
            tt += t * t;
            nn += n * n;
            tn += t * n;
        }
        let spread = ((tt - sum_t * sum_t / n_of(&pixels)) * (nn - sum_n * sum_n / n_of(&pixels))).max(1e-6).sqrt();
        let bend = ((tn - sum_t * sum_n / n_of(&pixels)) / spread).abs();
        out.push(Blob {
            area: pixels.len(),
            min: [min[0] * step, min[1] * step],
            max: [(max[0] + 1) * step, (max[1] + 1) * step],
            middle: [(mx + 0.5) * scale, (my + 0.5) * scale],
            long: (middle + apart).max(0.0).sqrt() * scale,
            short: (middle - apart).max(0.0).sqrt() * scale,
            along: [angle.cos(), angle.sin()],
            scale,
            bend,
        });
    }
    out
}

fn n_of(pixels: &[usize]) -> f32 {
    pixels.len() as f32
}

impl Blob {
    fn size(&self) -> f32 {
        (self.max[0] - self.min[0]).max(self.max[1] - self.min[1]) as f32
    }

    /// An X: as long as it is wide whichever way it lies, filled at its middle, with four arms round it, at two distances out. Measured on the real X at six turns and sizes,
    /// where the white icons nearest to it have anything from one to eight.
    fn is_x(&self, white: &dyn Fn(f32, f32) -> bool) -> bool {
        if self.size() < SMALLEST || self.long > self.short * 1.35 || !white(self.middle[0], self.middle[1]) {
            return false;
        }
        let arms = |reach: f32| {
            let mut count = 0;
            let mut was = white(self.middle[0] + reach, self.middle[1]);
            for i in 1..=48 {
                let (s, c) = (i as f32 / 48.0 * std::f32::consts::TAU).sin_cos();
                let on = white(self.middle[0] + reach * c, self.middle[1] + reach * s);
                if on && !was {
                    count += 1;
                }
                was = on;
            }
            count
        };
        let across = (self.area as f32).sqrt() * self.scale;
        arms(across * 0.55) == 4 && arms(across * 0.65) == 4
    }

    /// A chevron: longer than it is thick, and bent, which a bar of text or a slider is not. Measured on the real one at 0.85 to 0.89, where straight things are under 0.35.
    fn is_chevron(&self) -> bool {
        self.size() >= SMALLEST && self.long >= self.short * 1.7 && self.long <= self.short * 7.0 && self.bend >= CHEVRON_BENDS
    }
}

/// The X and the chevron of a camera panel in a picture, or `None`.
pub fn find(bgra: &[u8], width: usize, height: usize, stride: usize) -> Option<Found> {
    let step = (width / 1280).max(1);
    let found = blobs(bgra, width, height, stride, step);
    let white = |x: f32, y: f32| -> bool {
        if x < 0.0 || y < 0.0 || x as usize >= width || y as usize >= height {
            return false;
        }
        let i = y as usize * stride + x as usize * 4;
        bgra.get(i..i + 3).is_some_and(|p| p[0] >= WHITE && p[1] >= WHITE && p[2] >= WHITE)
    };
    let xs: Vec<&Blob> = found.iter().filter(|b| b.is_x(&white)).collect();
    let chevrons: Vec<&Blob> = found.iter().filter(|b| b.is_chevron()).collect();
    let mut best: Option<(f32, Found)> = None;
    for x in &xs {
        for c in &chevrons {
            let v = [c.middle[0] - x.middle[0], c.middle[1] - x.middle[1]];
            let far = v[0].hypot(v[1]);
            if far < NEAREST {
                continue;
            }
            // The chevron lies across the panel: its short way is the panel's up and down, the one of the two that goes from the X to it.
            let mut down = [-c.along[1], c.along[0]];
            if down[0] * v[0] + down[1] * v[1] < 0.0 {
                down = [-down[0], -down[1]];
            }
            if down[1] < STANDS_AT_LEAST {
                continue;
            }
            let right = [down[1], -down[0]];
            let (across, along) = (v[0] * right[0] + v[1] * right[1], v[0] * down[0] + v[1] * down[1]);
            if along <= 0.0 || !(ACROSS_PER_DOWN.0..=ACROSS_PER_DOWN.1).contains(&(across / along)) {
                continue;
            }
            let (x_share, c_share) = (x.size() / far, c.size() / far);
            if !(X_SHARE.0..=X_SHARE.1).contains(&x_share) || !(CHEVRON_SHARE.0..=CHEVRON_SHARE.1).contains(&c_share) {
                continue;
            }
            // Nearest to how the panel has them.
            let score = (across / along - 173.0 / 183.0).abs() + (x_share - X_IS).abs() * 4.0 + (c_share - CHEVRON_IS).abs() * 4.0 + (x.area as f32 * x.scale * x.scale / (x.size() * x.size()) - X_FILLS).abs();
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, Found { x: x.middle, chevron: c.middle, down }));
            }
        }
    }
    best.map(|b| b.1)
}

/// How a frame of the mirror maps on the eye: its tangents as SteamVR gives them (left, right, top, bottom), and the picture's width over its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    pub tangents: [f32; 4],
    pub aspect: f32,
}

impl Mirror {
    /// A place on the picture (0 to 1 each way) as tangents from the eye: right, and down.
    pub fn tangent(&self, u: f32, v: f32) -> [f32; 2] {
        let [left, right, top, bottom] = self.tangents;
        let wide = right - left;
        [left + u * wide, (top + bottom) / 2.0 + (v - 0.5) * wide / self.aspect]
    }
}

/// Where a panel seen by the eye lies in the room, and what one of its units is there. `eye` is that eye's pose then. In the plane facing it, turned as it is seen turned.
pub fn panel_in_room(found: &Found, size: [f32; 2], mirror: &Mirror, eye: &Matrix34) -> (Matrix34, f32) {
    let on = |p: [f32; 2]| mirror.tangent(p[0] / size[0], p[1] / size[1]);
    let (x, chevron) = (on(found.x), on(found.chevron));
    // The same way in tangents as in pixels: they scale alike across and down.
    let down = found.down;
    let right = [down[1], -down[0]];
    let v = [chevron[0] - x[0], chevron[1] - x[1]];
    let panel_v = [PANEL_CHEVRON[0] - PANEL_X[0], PANEL_CHEVRON[1] - PANEL_X[1]];
    // Tangent per unit: by how far down the line runs, which a turn about the panel's up does not change.
    let per_unit = (v[0] * down[0] + v[1] * down[1]) / panel_v[1];
    let middle = [
        x[0] + (right[0] * -PANEL_X[0] + down[0] * -PANEL_X[1]) * per_unit,
        x[1] + (right[1] * -PANEL_X[0] + down[1] * -PANEL_X[1]) * per_unit,
    ];
    let depth = (PANEL_UNIT_M / per_unit.max(1e-5)).clamp(0.2, 3.0);
    let unit = per_unit * depth;
    let m = &eye.m;
    let turn = |a: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| m[i][0] * a[0] + m[i][1] * a[1] + m[i][2] * a[2]) };
    // The eye looks along -Z, with +Y up.
    let at = turn([middle[0] * depth, -middle[1] * depth, -depth]);
    let (r, u, z) = (turn([right[0], -right[1], 0.0]), turn([-down[0], down[1], 0.0]), turn([0.0, 0.0, 1.0]));
    let frame = Matrix34 { m: std::array::from_fn(|i| [r[i], u[i], z[i], at[i] + m[i][3]]) };
    (frame, unit)
}

/// Follows what is seen, ten times a second, smoothly at the headset's rate: at once over a jump, softly over a tremble.
#[derive(Clone, Copy, Debug, Default)]
pub struct Follow {
    target: Option<(Matrix34, f32)>,
    now: Option<(Matrix34, f32)>,
}

/// How far it goes towards what is seen, per second, and beyond what distance it simply is there.
const FOLLOWS_PER_S: f32 = 14.0;
const JUMPS_M: f32 = 0.25;

impl Follow {
    pub fn seen(&mut self, frame: Matrix34, unit: f32) {
        self.target = Some((frame, unit));
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn step(&mut self, seconds: f32) -> Option<(Matrix34, f32)> {
        let (target, unit) = self.target?;
        let Some((now, was)) = self.now else {
            self.now = Some((target, unit));
            return self.now;
        };
        let apart = (0..3).map(|i| (target.m[i][3] - now.m[i][3]).powi(2)).sum::<f32>().sqrt();
        let k = if apart > JUMPS_M { 1.0 } else { 1.0 - (-FOLLOWS_PER_S * seconds).exp() };
        let mut m = now.m;
        for (row, to) in m.iter_mut().zip(target.m.iter()) {
            for (a, b) in row.iter_mut().zip(to.iter()) {
                *a += (*b - *a) * k;
            }
        }
        // Straight again: what was mixed is not quite a rotation.
        let axis = |c: usize| -> [f32; 3] { [m[0][c], m[1][c], m[2][c]] };
        let unit_of = |a: [f32; 3]| -> [f32; 3] {
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-5);
            [a[0] / l, a[1] / l, a[2] / l]
        };
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let z = unit_of(axis(2));
        let x = unit_of(cross(axis(1), z));
        let y = cross(z, x);
        let shown = Matrix34 { m: std::array::from_fn(|i| [x[i], y[i], z[i], m[i][3]]) };
        self.now = Some((shown, was + (unit - was) * k));
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn testdata() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata")
    }

    /// Enzo's own picture of the panel: the red rings round the X and the chevron are his.
    fn panel_picture() -> (Vec<u8>, usize, usize) {
        let image = image::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/camera-panel-752x417.jpg")).expect("the picture").to_rgba8();
        let (width, height) = (image.width() as usize, image.height() as usize);
        let mut bgra = image.into_raw();
        for p in bgra.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        (bgra, width, height)
    }

    /// The same twice as large, as VRChat's window really is, turned by `degrees` and scaled by `scale` about its middle, blended as a panel in 3D is drawn.
    fn turned(from: &[u8], width: usize, height: usize, degrees: f32, scale: f32) -> (Vec<u8>, usize, usize) {
        let (w, h) = (width * 2, height * 2);
        let (s, c) = degrees.to_radians().sin_cos();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let mut out = vec![0u8; w * h * 4];
        let at = |x: usize, y: usize, ch: usize| from[(y.min(height - 1) * width + x.min(width - 1)) * 4 + ch] as f32;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = ((x as f32 - cx) / scale, (y as f32 - cy) / scale);
                let (fx, fy) = ((cx + dx * c + dy * s) / 2.0, (cy - dx * s + dy * c) / 2.0);
                if fx < 0.0 || fy < 0.0 || fx >= width as f32 || fy >= height as f32 {
                    continue;
                }
                let (ix, iy, ax, ay) = (fx as usize, fy as usize, fx.fract(), fy.fract());
                for ch in 0..4 {
                    let v = (at(ix, iy, ch) * (1.0 - ax) + at(ix + 1, iy, ch) * ax) * (1.0 - ay) + (at(ix, iy + 1, ch) * (1.0 - ax) + at(ix + 1, iy + 1, ch) * ax) * ay;
                    out[(y * w + x) * 4 + ch] = v as u8;
                }
            }
        }
        (out, w, h)
    }

    #[test]
    fn it_finds_the_x_and_the_chevron_on_the_real_panel() {
        let (bgra, width, height) = panel_picture();
        let found = find(&bgra, width, height, width * 4).expect("the panel");
        assert!((found.x[0] - 204.0).abs() < 8.0 && (found.x[1] - 102.0).abs() < 8.0, "{found:?}");
        assert!((found.chevron[0] - 377.0).abs() < 8.0 && (found.chevron[1] - 285.0).abs() < 8.0, "{found:?}");
        assert!(found.down[1] > 0.98, "it stands straight here: {found:?}");
    }

    /// Every turn and size finds the X and the chevron themselves, where they went, and not something else that lies about right.
    #[test]
    fn it_finds_the_panel_turned_and_at_other_distances() {
        let (bgra, width, height) = panel_picture();
        for (degrees, scale) in [(0.0, 1.0), (10.0, 1.0), (-15.0, 1.0), (25.0, 0.9), (0.0, 0.7), (-8.0, 1.2), (35.0, 1.0), (-30.0, 0.8), (5.0, 0.5)] {
            let (picture, w, h) = turned(&bgra, width, height, degrees, scale);
            let seen = find(&picture, w, h, w * 4).unwrap_or_else(|| panic!("at {degrees} degrees, {scale}"));
            let (s, c) = (degrees as f32).to_radians().sin_cos();
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            let where_to = |x: f32, y: f32| {
                let (dx, dy) = ((x * 2.0 - cx) * scale, (y * 2.0 - cy) * scale);
                [cx + dx * c - dy * s, cy + dx * s + dy * c]
            };
            let (x, chevron) = (where_to(204.0, 102.0), where_to(377.0, 285.0));
            assert!((seen.x[0] - x[0]).hypot(seen.x[1] - x[1]) < 14.0, "{degrees} {scale}: the X is at {x:?}: {seen:?}");
            assert!((seen.chevron[0] - chevron[0]).hypot(seen.chevron[1] - chevron[1]) < 14.0, "{degrees} {scale}: the chevron is at {chevron:?}: {seen:?}");
            let angle = (-seen.down[0]).atan2(seen.down[1]).to_degrees();
            assert!((angle - degrees).abs() < 12.0, "{degrees} {scale}: it leans {angle}");
        }
    }

    /// Frames of the game with VRChat's own menus, or none, measured for the wheel and the Quick Menu: nothing is a camera panel.
    #[test]
    fn nothing_else_seen_in_the_game_is_a_panel() {
        let mut checked = 0;
        let mut wrong = Vec::new();
        for entry in std::fs::read_dir(testdata()).unwrap().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("camera-panel") {
                continue;
            }
            // Parts of frames are too small to hold a panel: whole frames.
            let Ok(image) = image::open(&path) else { continue };
            let image = image.to_rgba8();
            if image.width() < 400 {
                continue;
            }
            let (width, height) = (image.width() as usize, image.height() as usize);
            let mut bgra = image.into_raw();
            for p in bgra.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            checked += 1;
            if let Some(found) = find(&bgra, width, height, width * 4) {
                wrong.push(format!("{name}: {found:?}"));
            }
        }
        assert!(checked >= 5, "{checked} frames");
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn plain_white_or_nothing_finds_nothing() {
        let (w, h) = (640usize, 360usize);
        assert_eq!(find(&vec![0u8; w * h * 4], w, h, w * 4), None);
        assert_eq!(find(&vec![255u8; w * h * 4], w, h, w * 4), None);
    }

    fn mirror() -> Mirror {
        Mirror { tangents: [-1.0, 1.0, -1.0, 1.0], aspect: 16.0 / 9.0 }
    }

    const STILL: Matrix34 = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.5], [0.0, 0.0, 1.0, 0.0]] };

    /// A panel straight ahead, of its usual size at half a metre, comes out there, and its parts lie where they were seen.
    #[test]
    fn a_panel_seen_lands_where_it_was_seen_in_the_room() {
        let size = [1600.0, 900.0];
        // Half a metre away: a unit is that many tangents.
        let per_unit = PANEL_UNIT_M / 0.5;
        let pixels_per_tangent = 800.0;
        let at = |p: [f32; 2]| [800.0 + p[0] * per_unit * pixels_per_tangent, 450.0 + p[1] * per_unit * pixels_per_tangent];
        let found = Found { x: at(PANEL_X), chevron: at(PANEL_CHEVRON), down: [0.0, 1.0] };
        let (frame, unit) = panel_in_room(&found, size, &mirror(), &STILL);
        assert!((frame.m[2][3] + 0.5).abs() < 1e-3, "half a metre ahead: {:?}", frame.m);
        assert!(frame.m[0][3].abs() < 1e-3 && (frame.m[1][3] - 1.5).abs() < 1e-3, "straight ahead: {:?}", frame.m);
        assert!((unit - PANEL_UNIT_M).abs() < 1e-5);
        assert_eq!((frame.m[0][0], frame.m[1][1], frame.m[2][2]), (1.0, 1.0, 1.0), "facing the eye, upright");
    }

    #[test]
    fn a_panel_turned_in_the_picture_is_turned_in_the_room_and_low_in_the_picture_is_low() {
        // Down the picture leaning to the left: the panel's right goes down on the right.
        let s = 20.0f32.to_radians();
        let down = [-s.sin(), s.cos()];
        let right = [down[1], -down[0]];
        let size = [1600.0, 900.0];
        let per_unit = PANEL_UNIT_M / 0.5;
        let at = |p: [f32; 2]| [800.0 + (p[0] * right[0] + p[1] * down[0]) * per_unit * 800.0, 650.0 + (p[0] * right[1] + p[1] * down[1]) * per_unit * 800.0];
        let (frame, _) = panel_in_room(&Found { x: at(PANEL_X), chevron: at(PANEL_CHEVRON), down }, size, &mirror(), &STILL);
        assert!(frame.m[1][0] < -0.3, "the panel's right goes down: {:?}", frame.m);
        assert!(frame.m[1][3] < 1.5, "below the eye: {:?}", frame.m);
    }

    #[test]
    fn it_follows_softly_and_jumps_when_far() {
        let mut follow = Follow::default();
        assert!(follow.step(0.01).is_none());
        let at = |x: f32| Matrix34 { m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, -0.5]] };
        follow.seen(at(0.0), 0.001);
        assert_eq!(follow.step(0.01).unwrap().0.m[0][3], 0.0);
        follow.seen(at(0.1), 0.001);
        let soft = follow.step(0.011).unwrap().0.m[0][3];
        assert!(soft > 0.0 && soft < 0.05, "{soft}");
        follow.seen(at(1.0), 0.001);
        assert_eq!(follow.step(0.011).unwrap().0.m[0][3], 1.0);
    }
}
