//! Where VRChat's camera panel is, in a headset, to lay tiles around it. VRChat says nothing of it: `/usercamera/Pose` is the camera's own place, which leaves its panel behind
//! once it flies (Enzo). What is always on the panel can be seen, as Enzo proposed: its white **X** at the top left and its white **chevron** under the middle, in what one eye sees.
//! How far apart they are says how large it is, the chevron's direction how it is turned. Nothing touches the game.
//!
//! In a headset these glyphs are small and soft: on Enzo's own pictures the X is 9 pixels across and hardly reaches 200 of 255, so what is bright is looked for at two levels and a glyph is
//! told by its shape on a grid, not by counting its arms. An X and a chevron alone could be anything, so what else is always on the panel must lie where the two say it does: the icon beside
//! the X, the sliders, the column of icons on the left, the row of buttons. **A wrong tile is worse than a late one.**
//!
//! [`Mirror`] says what the picture covers. What the compositor has of the eye covers all of it, known from SteamVR; VRChat's window does not, and is only used when that cannot be read.

use orbiters_openvr::Matrix34;

// The X/chevron pair only seeds recognition. A spread landmark homography and rigid-plane
// reprojection determine the live pose; panel_in_room is retained solely for the older fixture tests.

/// The panel in its own units, as a picture of it 410 across has them, from its middle: its X, and its chevron.
pub const PANEL_X: [f32; 2] = [-181.0, -83.0];
pub const PANEL_CHEVRON: [f32; 2] = [-8.0, 100.0];
pub const PANEL_SIZE: [f32; 2] = [410.0, 220.0];
/// How wide the panel is in the room at VRChat's usual size. Only how far away the tiles seem comes from this; where they show comes from where it is seen. `camera_panel_m` in `Tune`.
pub const PANEL_M: f32 = 0.30;

/// What counts as bright, of a pixel's darkest channel: a glyph's middle at the size a headset shows it, and one with a bright world close behind it.
const BRIGHT: [u8; 2] = [150, 190];
/// A blob's size in pixels: smaller is a speck, larger is a wall.
const SMALLEST_BLOB: usize = 6;
const LARGEST_BLOB: usize = 160;
const X_SIZES: (usize, usize) = (5, 48);
const CHEVRON_SIZES: (usize, usize) = (8, 90);
/// How large each is against how far apart they are, with room for how a turn of the panel foreshortens them; and as the panel has them.
const X_SHARE: (f32, f32) = (0.022, 0.09);
const CHEVRON_SHARE: (f32, f32) = (0.04, 0.16);
const X_IS: f32 = 0.044;
const CHEVRON_IS: f32 = 0.075;
/// How far apart they are at the least, in pixels of what is read.
const NEAREST: f32 = 70.0;
/// How far a panel may lean in the picture.
const LEANS_AT_MOST: f32 = 60.0;
/// How like an X it must be on its grid: 0.6 to 0.8 on real ones at 7 to 12 pixels, under 0.45 on the panel's other icons.
const X_LOOKS: f32 = 0.45;
/// How bent a chevron is: 0 for a straight bar, near 1 for a V.
const CHEVRON_BENDS: f32 = 0.42;
/// How much of what else is always on the panel must lie where these two say: of the icon beside the X, the sliders, the left column and the row of buttons. Four on a real one, at most two on
/// any of 258 frames with VRChat's menus and ours.
const SUPPORT: usize = 3;

/// The same when it is so small that it is read at twice its size: all of it, as a wrong tile is worse than a late one.
const SUPPORT_SMALL: usize = 4;

/// One frame's reading, in pixels of what was given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub perspective: Option<[[f32; 3]; 3]>,
    pub residual_px: f32,
    pub landmarks: Option<(usize, [([f32;2],[f32;2]);10])>,
    pub x: [f32; 2],
    pub chevron: [f32; 2],
    pub middle: [f32; 2],
    /// The panel's right and down in the picture, unit long and at right angles.
    pub right: [f32; 2],
    pub down: [f32; 2],
    /// Pixels to one of its units along each: a panel turned away is narrower, and as tall.
    pub across: f32,
    pub tall: f32,
}

impl Found {
    fn moved(mut self, by: [f32; 2], scale: f32) -> Self {
        let at = |p: [f32; 2]| [p[0] / scale + by[0], p[1] / scale + by[1]];
        (self.x, self.chevron, self.middle) = (at(self.x), at(self.chevron), at(self.middle));
        self.across /= scale;
        self.tall /= scale;
        self.residual_px /= scale;
        if let Some((count,points)) = self.landmarks.as_mut() {
            for (_,q) in points.iter_mut().take(*count) { *q=at(*q); }
        }
        if let Some(h) = self.perspective.as_mut() {
            for k in 0..3 {
                h[0][k] = h[0][k] / scale + by[0] * h[2][k];
                h[1][k] = h[1][k] / scale + by[1] * h[2][k];
            }
        }
        self
    }

    /// The same, for a picture that started `by` further on.
    pub fn shifted(self, by: [f32; 2]) -> Self {
        self.moved(by, 1.0)
    }

    /// Where a place on the panel in its units lies in the picture.
    pub fn at(&self, unit: [f32; 2]) -> [f32; 2] {
        if let Some(h) = self.perspective { return project(h, unit); }
        [
            self.middle[0] + self.right[0] * unit[0] * self.across + self.down[0] * unit[1] * self.tall,
            self.middle[1] + self.right[1] * unit[0] * self.across + self.down[1] * unit[1] * self.tall,
        ]
    }
}

/// How bright each pixel is, as its darkest channel: only white things stay.
struct Light {
    width: usize,
    height: usize,
    px: Vec<u8>,
}

impl Light {
    fn of(pixels: &[u8], width: usize, height: usize, stride: usize, area: [usize; 4]) -> Self {
        let [x0, y0, w, h] = [area[0].min(width), area[1].min(height), area[2], area[3]];
        let (w, h) = (w.min(width - x0), h.min(height - y0));
        let mut px = vec![0u8; w * h];
        for y in 0..h {
            let start = (y0 + y) * stride + x0 * 4;
            let Some(row) = pixels.get(start..start + w * 4) else { continue };
            for (out, p) in px[y * w..(y + 1) * w].iter_mut().zip(row.chunks_exact(4)) {
                *out = p[0].min(p[1]).min(p[2]);
            }
        }
        Self { width: w, height: h, px }
    }

    /// Twice as large, for what is small.
    fn doubled(&self) -> Self {
        let (w, h) = (self.width * 2, self.height * 2);
        let mut px = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] = self.sample((x as f32 + 0.5) / 2.0, (y as f32 + 0.5) / 2.0) as u8;
            }
        }
        Self { width: w, height: h, px }
    }

    /// Between pixels, whose middles lie at .5.
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (fx, fy) = (x - 0.5, y - 0.5);
        if fx < 0.0 || fy < 0.0 || fx >= (self.width - 1) as f32 || fy >= (self.height - 1) as f32 {
            return 0.0;
        }
        let (ix, iy, ax, ay) = (fx as usize, fy as usize, fx.fract(), fy.fract());
        let at = |x: usize, y: usize| self.px[y * self.width + x] as f32;
        (at(ix, iy) * (1.0 - ax) + at(ix + 1, iy) * ax) * (1.0 - ay) + (at(ix, iy + 1) * (1.0 - ax) + at(ix + 1, iy + 1) * ax) * ay
    }
}

#[derive(Clone, Debug)]
struct Blob {
    area: usize,
    wide: usize,
    high: usize,
    middle: [f32; 2],
    /// Its long and short spreads, and the way of the long one.
    long: f32,
    short: f32,
    along: [f32; 2],
    /// How bent it is: 0 for a straight bar, near 1 for a V, whichever way it opens.
    bend: f32,
}

impl Blob {
    fn size(&self) -> usize {
        self.wide.max(self.high)
    }

    fn may_be_x(&self) -> bool {
        let fill = self.area as f32 / (self.wide * self.high) as f32;
        (X_SIZES.0..=X_SIZES.1).contains(&self.size()) && self.wide.min(self.high) as f32 >= 0.55 * self.size() as f32 && self.long <= self.short * 1.7 && (0.3..=0.82).contains(&fill)
    }

    fn may_be_chevron(&self) -> bool {
        (CHEVRON_SIZES.0..=CHEVRON_SIZES.1).contains(&self.size()) && self.long >= self.short * 1.7 && self.long <= self.short * 8.0 && self.bend >= CHEVRON_BENDS
    }
}

/// The blobs of what is at least `level`, added to `out`.
fn blobs(light: &Light, level: u8, out: &mut Vec<Blob>) {
    let (w, h) = (light.width, light.height);
    let mut left: Vec<bool> = light.px.iter().map(|&p| p >= level).collect();
    let mut todo: Vec<usize> = Vec::new();
    let mut pixels: Vec<usize> = Vec::new();
    for start in 0..left.len() {
        if !left[start] {
            continue;
        }
        left[start] = false;
        todo.push(start);
        pixels.clear();
        let (mut min, mut max) = ([usize::MAX; 2], [0usize; 2]);
        while let Some(at) = todo.pop() {
            pixels.push(at);
            let (x, y) = (at % w, at / w);
            (min[0], min[1], max[0], max[1]) = (min[0].min(x), min[1].min(y), max[0].max(x), max[1].max(y));
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    if left[ny * w + nx] {
                        left[ny * w + nx] = false;
                        todo.push(ny * w + nx);
                    }
                }
            }
        }
        let (wide, high) = (max[0] - min[0] + 1, max[1] - min[1] + 1);
        if pixels.len() < SMALLEST_BLOB || wide.max(high) > LARGEST_BLOB {
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
        // How far out along its length goes with how far across: |t| against n, as a correlation.
        let (ca, sa) = (angle.cos(), angle.sin());
        let (mut sum_t, mut sum_n, mut tt, mut nn, mut tn) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for at in &pixels {
            let (dx, dy) = ((at % w) as f32 - mx, (at / w) as f32 - my);
            let (t, m) = ((dx * ca + dy * sa).abs(), dy * ca - dx * sa);
            sum_t += t;
            sum_n += m;
            tt += t * t;
            nn += m * m;
            tn += t * m;
        }
        let spread = ((tt - sum_t * sum_t / n) * (nn - sum_n * sum_n / n)).max(1e-6).sqrt();
        out.push(Blob {
            area: pixels.len(),
            wide,
            high,
            middle: [mx + 0.5, my + 0.5],
            long: (middle + apart).max(0.0).sqrt(),
            short: (middle - apart).max(0.0).sqrt(),
            along: [ca, sa],
            bend: ((tn - sum_t * sum_n / n) / spread).abs(),
        });
    }
}

/// How like an X what is around `at` is, turned by `lean`: a correlation with an ideal one on a grid, which is all that can be said of 9 pixels.
fn x_looks(light: &Light, at: [f32; 2], half: f32, lean: f32) -> f32 {
    const N: usize = 11;
    let (s, c) = lean.sin_cos();
    let (mut seen, mut ideal) = ([0f32; N * N], [0f32; N * N]);
    for j in 0..N {
        for i in 0..N {
            let (u, v) = (i as f32 / (N - 1) as f32 * 2.0 - 1.0, j as f32 / (N - 1) as f32 * 2.0 - 1.0);
            seen[j * N + i] = light.sample(at[0] + half * (u * c - v * s), at[1] + half * (u * s + v * c));
            let (x, y) = (u * 1.25, v * 1.25);
            let on_arm = (x - y).abs() / std::f32::consts::SQRT_2 < 0.28 || (x + y).abs() / std::f32::consts::SQRT_2 < 0.28;
            ideal[j * N + i] = if on_arm && x.abs().max(y.abs()) <= 1.0 { 1.0 } else { 0.0 };
        }
    }
    let mean = |a: &[f32]| a.iter().sum::<f32>() / a.len() as f32;
    let (ms, mi) = (mean(&seen), mean(&ideal));
    let (mut both, mut ss, mut ii) = (0.0, 0.0, 0.0);
    for (a, b) in seen.iter().zip(&ideal) {
        both += (a - ms) * (b - mi);
        ss += (a - ms) * (a - ms);
        ii += (b - mi) * (b - mi);
    }
    both / (ss * ii).sqrt().max(1e-3)
}

/// One way of reading the panel from its X and its chevron.
#[derive(Clone, Copy)]
struct Reading {
    right: [f32; 2],
    down: [f32; 2],
    across: f32,
    tall: f32,
    middle: [f32; 2],
}

impl Reading {
    fn support(&self, all: &[Blob]) -> usize {
        let on = |b: &Blob| {
            let d = [b.middle[0] - self.middle[0], b.middle[1] - self.middle[1]];
            [(d[0] * self.right[0] + d[1] * self.right[1]) / self.across, (d[0] * self.down[0] + d[1] * self.down[1]) / self.tall]
        };
        let size = (self.across + self.tall) / 2.0;
        let near = |p: [f32; 2], t: [f32; 2]| all.iter().any(|b| { let u = on(b); (u[0] - p[0]).abs() <= t[0] && (u[1] - p[1]).abs() <= t[1] });
        let icon = all.iter().any(|b| {
            let u = on(b);
            (u[0] + 154.0).abs() <= 14.0 && (u[1] + 84.0).abs() <= 10.0 && b.size() as f32 >= 9.0 * size && b.area as f32 >= 30.0 * size * size
        });
        let slider = all.iter().any(|b| {
            let u = on(b);
            (130.0..=195.0).contains(&u[0]) && (-5.0..=95.0).contains(&u[1]) && b.long >= b.short * 2.0 && b.long / size >= 6.0 && (b.along[0] * self.down[0] + b.along[1] * self.down[1]).abs() > 30f32.to_radians().cos()
        });
        let column = [36.0, 54.0, 74.0, 93.0].iter().filter(|y| near([-190.0, **y], [14.0, 8.0])).count();
        let buttons = [[-148.0, 131.0], [-35.0, 133.0], [82.0, 135.0]].iter().filter(|p| near(**p, [18.0, 14.0])).count();
        icon as usize + slider as usize + (column >= 2) as usize + (buttons >= 2) as usize
    }
}

/// The panel in what is read, in its pixels.
fn look(light: &Light, needs: usize) -> Option<Found> {
    if light.width < 16 || light.height < 16 {
        return None;
    }
    let mut all = Vec::new();
    for level in BRIGHT {
        blobs(light, level, &mut all);
    }
    let panel = [PANEL_CHEVRON[0] - PANEL_X[0], PANEL_CHEVRON[1] - PANEL_X[1]];
    let (panel_far, panel_angle) = (panel[0].hypot(panel[1]), panel[1].atan2(panel[0]));
    let mut best: Option<(f32, Found)> = None;
    for x in all.iter().filter(|b| b.may_be_x()) {
        for c in all.iter().filter(|b| b.may_be_chevron()) {
            let v = [c.middle[0] - x.middle[0], c.middle[1] - x.middle[1]];
            let far = v[0].hypot(v[1]);
            if far < NEAREST {
                continue;
            }
            let (x_share, c_share) = (x.size() as f32 / far, c.size() as f32 / far);
            if !(X_SHARE.0..=X_SHARE.1).contains(&x_share) || !(CHEVRON_SHARE.0..=CHEVRON_SHARE.1).contains(&c_share) {
                continue;
            }
            // Seen straight on, the two say its size and how it leans.
            let lean = v[1].atan2(v[0]) - panel_angle;
            let (s, co) = lean.sin_cos();
            let (right, down) = ([co, s], [-s, co]);
            // The chevron lies along the panel's right.
            let mut along = c.along;
            let lies = along[0] * right[0] + along[1] * right[1];
            if lies.abs() < 35f32.to_radians().cos() {
                continue;
            }
            if lies < 0.0 {
                along = [-along[0], -along[1]];
            }
            let looks = x_looks(light, x.middle, x.size() as f32 * 0.625, lean);
            if looks < X_LOOKS {
                continue;
            }
            let per_unit = far / panel_far;
            let straight = Reading {
                right,
                down,
                across: per_unit,
                tall: per_unit,
                middle: [x.middle[0] - (right[0] * PANEL_X[0] + down[0] * PANEL_X[1]) * per_unit, x.middle[1] - (right[1] * PANEL_X[0] + down[1] * PANEL_X[1]) * per_unit],
            };
            // Turned away it is narrower, and the chevron still lies along its right: this reads it then.
            let down_of_chevron = [-along[1], along[0]];
            let (a, b) = (v[0] * along[0] + v[1] * along[1], v[0] * down_of_chevron[0] + v[1] * down_of_chevron[1]);
            let turned = (a > far * 0.25 && b > far * 0.25).then(|| {
                let (across, tall) = (a / panel[0], b / panel[1]);
                Reading {
                    right: along,
                    down: down_of_chevron,
                    across,
                    tall,
                    middle: [x.middle[0] - along[0] * PANEL_X[0] * across - down_of_chevron[0] * PANEL_X[1] * tall, x.middle[1] - along[1] * PANEL_X[0] * across - down_of_chevron[1] * PANEL_X[1] * tall],
                }
            });
            for reading in [Some(straight), turned].into_iter().flatten() {
                let support = reading.support(&all);
                if support < needs {
                    continue;
                }
                let fitted = landmark_fit(&reading, &all, x.middle, c.middle);
                if lean.abs() > LEANS_AT_MOST.to_radians() && fitted.is_none() { continue; }
                let (perspective,residual_px,landmarks) = fitted.map_or((None,f32::INFINITY,None), |(h,e,p)| (Some(h),e,Some(p)));
                let score = looks + support as f32 * 0.2 - (x_share - X_IS).abs() * 3.0 - (c_share - CHEVRON_IS).abs() * 2.0;
                if best.as_ref().is_none_or(|b| score > b.0) {
                    let found = Found { perspective, residual_px, landmarks, x: x.middle, chevron: c.middle, middle: reading.middle, right: reading.right, down: reading.down, across: reading.across, tall: reading.tall };
                    best = Some((score, found));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// The panel in a picture of 4-byte pixels, whichever way round its colours are, or `None`.
fn project(h: [[f32; 3]; 3], p: [f32; 2]) -> [f32; 2] {
    let q = h.map(|r| r[0] * p[0] + r[1] * p[1] + r[2]);
    [q[0] / q[2], q[1] / q[2]]
}

/// Least squares homography from independently located, spread landmarks. Reprojection is in pixels.
pub fn fit(points: &[([f32; 2], [f32; 2])]) -> Option<[[f32; 3]; 3]> {
    if points.len() < 5 { return None; }
    let mut a = [[0.0f64; 9]; 8];
    for &(p, q) in points {
        let (x, y, u, v) = (p[0] as f64, p[1] as f64, q[0] as f64, q[1] as f64);
        for row in [[x,y,1.,0.,0.,0.,-u*x,-u*y,u], [0.,0.,0.,x,y,1.,-v*x,-v*y,v]] {
            for i in 0..8 { for j in 0..9 { a[i][j] += row[i] * row[j]; } }
        }
    }
    for k in 0..8 {
        let pivot = (k..8).max_by(|&i,&j| a[i][k].abs().total_cmp(&a[j][k].abs()))?;
        a.swap(k,pivot);
        if a[k][k].abs() < 1e-8 { return None; }
        let d = a[k][k];
        for j in k..9 { a[k][j] /= d; }
        for i in 0..8 { if i != k { let d = a[i][k]; for j in k..9 { a[i][j] -= d * a[k][j]; } } }
    }
    let h = [[a[0][8] as f32,a[1][8] as f32,a[2][8] as f32], [a[3][8] as f32,a[4][8] as f32,a[5][8] as f32], [a[6][8] as f32,a[7][8] as f32,1.]];
    let error = points.iter().map(|&(p,q)| { let r=project(h,p); (r[0]-q[0]).powi(2)+(r[1]-q[1]).powi(2) }).sum::<f32>() / points.len() as f32;
    (error.is_finite() && error.sqrt() <= 2.5).then_some(h)
}

fn landmark_fit(r: &Reading, blobs: &[Blob], x: [f32;2], chevron: [f32;2]) -> Option<([[f32;3];3],f32,(usize,[([f32;2],[f32;2]);10]))> {
    let mut points = vec![(PANEL_X,x),(PANEL_CHEVRON,chevron)];
    // Fixed icon centres, rather than the variable slider thumbs or mode-dependent button labels.
    for p in [[-154.,-84.],[-190.,36.],[-190.,54.],[-190.,74.],[-190.,93.],[-148.,131.],[-35.,133.],[82.,135.]] {
        let expected = [r.middle[0]+r.right[0]*p[0]*r.across+r.down[0]*p[1]*r.tall, r.middle[1]+r.right[1]*p[0]*r.across+r.down[1]*p[1]*r.tall];
        let distance = |b: &Blob| (b.middle[0]-expected[0]).hypot(b.middle[1]-expected[1]);
        if let Some(b) = blobs.iter().filter(|b| distance(b) < 8.0*r.across.max(r.tall)).min_by(|a,b| distance(a).total_cmp(&distance(b))) {
            if !points.iter().any(|(_,q)| *q == b.middle) { points.push((p,b.middle)); }
        }
    }
    // Both horizontal and vertical spread are essential; the left icon column alone is degenerate.
    if !points.iter().any(|(p,_)| p[0] > 50.) { return None; }
    let h=fit(&points)?;
    let residual=(points.iter().map(|&(p,q)| { let r=project(h,p); (r[0]-q[0]).powi(2)+(r[1]-q[1]).powi(2) }).sum::<f32>()/points.len() as f32).sqrt();
    let mut landmarks=[([0.;2],[0.;2]);10];
    for (out,p) in landmarks.iter_mut().zip(&points) { *out=*p; }
    Some((h,residual,(points.len(),landmarks)))
}

/// Perspective pose using the actual eye projection, with rigid-plane consistency rejection.
pub fn perspective_pose(found: &Found, size: [f32;2], mirror: &Mirror, eye: &Matrix34, width: f32) -> Option<(Matrix34,f32)> {
    let h = found.perspective?;
    let points: Vec<_> = [[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.]].into_iter().map(|p| {
        let q=project(h,p); let t=mirror.tangent(q[0]/size[0],q[1]/size[1]); (p,t)
    }).collect();
    let h=fit(&points)?;
    let col = |k:usize| [h[0][k],-h[1][k],-h[2][k]];
    let norm = |v:[f32;3]| v.iter().map(|x|x*x).sum::<f32>().sqrt();
    let (a,b,t)=(col(0),col(1),col(2));
    let (na,nb)=(norm(a),norm(b));
    let dot: f32=a.iter().zip(b).map(|(x,y)|x*y).sum();
    if na.min(nb) <= 0. || (na/nb-1.).abs()>0.25 || dot.abs()/(na*nb)>0.2 { return None; }
    let unit=width/PANEL_SIZE[0];
    let scale=2.*unit/(na+nb);
    let right=a.map(|x|x/na);
    let mut down=b.map(|x|x/nb);
    let d: f32=right.iter().zip(down).map(|(x,y)|x*y).sum();
    down=std::array::from_fn(|i| down[i]-d*right[i]);
    let n=norm(down); let up=down.map(|x|-x/n);
    let z=[right[1]*up[2]-right[2]*up[1],right[2]*up[0]-right[0]*up[2],right[0]*up[1]-right[1]*up[0]];
    let position=t.map(|x|x*scale);
    if !(0.15..=4.).contains(&-position[2]) { return None; }
    // A texture homography can fit a sheared quadrilateral perfectly. Verify the recovered
    // rigid plane itself through the eye projection before accepting it as a physical panel.
    let mut errors=Vec::new();
    let check = |p:[f32;2],q:[f32;2]| -> Option<f32> {
        let v: [f32;3]=std::array::from_fn(|i| position[i]+right[i]*p[0]*unit-up[i]*p[1]*unit);
        if -v[2] <= 0.0001 { return None; }
        let tangent=[v[0]/-v[2],-v[1]/-v[2]];
        let px=[(tangent[0]-mirror.from[0])/mirror.spans[0]*size[0],(tangent[1]-mirror.from[1])/mirror.spans[1]*size[1]];
        Some((px[0]-q[0]).powi(2)+(px[1]-q[1]).powi(2))
    };
    if let Some((count,landmarks))=found.landmarks {
        for (p,q) in landmarks.into_iter().take(count) { errors.push(check(p,q)?); }
    } else {
        for &(p,_) in &points { errors.push(check(p,project(found.perspective?,p))?); }
    }
    let rms=(errors.iter().sum::<f32>()/errors.len().max(1) as f32).sqrt();
    if !rms.is_finite() || rms>2.5 || errors.iter().any(|e| *e>36.) { return None; }
    let local=Matrix34 { m: std::array::from_fn(|i| [right[i],up[i],z[i],position[i]]) };
    Some((then(eye,&local),unit))
}

pub fn find(pixels: &[u8], width: usize, height: usize, stride: usize) -> Option<Found> {
    find_in(pixels, width, height, stride, [0, 0, width, height], width <= LOOKS_TWICE_UP_TO)
}

/// Pictures up to this wide are read at twice their size too when nothing is found. A panel this picture shows 200 pixels across or less has an X of under 6.
const LOOKS_TWICE_UP_TO: usize = 1400;

/// The same, only in `area` (x, y, width, height), and at twice the size too if asked, when nothing is found.
pub fn find_in(pixels: &[u8], width: usize, height: usize, stride: usize, area: [usize; 4], twice: bool) -> Option<Found> {
    let light = Light::of(pixels, width, height, stride, area);
    let by = [area[0].min(width) as f32, area[1].min(height) as f32];
    look(&light, SUPPORT).map(|f| f.moved(by, 1.0)).or_else(|| twice.then(|| look(&light.doubled(), SUPPORT_SMALL).map(|f| f.moved(by, 2.0))).flatten())
}

/// Around a panel found, with room for how far it goes between two looks: what the next look reads.
pub fn around(found: &Found, width: usize, height: usize) -> [usize; 4] {
    let (reach_x, reach_y) = (PANEL_SIZE[0] * found.across.max(found.tall), PANEL_SIZE[1] * 1.4 * found.across.max(found.tall));
    let (x0, y0) = ((found.middle[0] - reach_x).max(0.0) as usize, (found.middle[1] - reach_y).max(0.0) as usize);
    [x0, y0, ((reach_x * 2.0) as usize).min(width - x0.min(width)), ((reach_y * 2.0) as usize).min(height - y0.min(height))]
}

/// What a picture covers of the eye: the tangents from the eye at its top left, right and down, and how many it spans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    pub from: [f32; 2],
    pub spans: [f32; 2],
}

/// The tangents of an eye's sides, right and down, from what SteamVR gives. SteamVR's bottom is the tangent of the side *up*, whatever its name: `ComposeProjection` puts it at the top of the
/// picture, and `stereo::eye_camera` reads it so. On the Quest Pro these are up 0.900 and down 1.376. Taking them the other way lays things 0.476 too high, which is how the tiles
/// came to be well above the panel in Enzo's pictures.
pub fn eye_sides(raw: [f32; 4]) -> [f32; 4] {
    let (up, below) = (raw[2].max(raw[3]), raw[2].min(raw[3]));
    [raw[0].min(raw[1]), raw[0].max(raw[1]), -up, -below]
}

impl Mirror {
    /// The whole of an eye, as the compositor has it.
    pub fn whole_eye(raw: [f32; 4]) -> Self {
        let [left, right, top, bottom] = eye_sides(raw);
        Self { from: [left, top], spans: [right - left, bottom - top] }
    }

    /// VRChat's window, `share` of the eye's width, as wide as it is against its height, about the eye's middle moved by `moved` of its width.
    pub fn window(raw: [f32; 4], aspect: f32, share: f32, moved: [f32; 2]) -> Self {
        let [left, right, top, bottom] = eye_sides(raw);
        let wide = (right - left) * share;
        let tall = wide / aspect.max(0.1);
        let middle = [(left + right) / 2.0 + moved[0] * (right - left), (top + bottom) / 2.0 + moved[1] * (right - left)];
        Self { from: [middle[0] - wide / 2.0, middle[1] - tall / 2.0], spans: [wide, tall] }
    }

    /// A place on the picture (0 to 1 each way) as tangents from the eye: right, and down.
    pub fn tangent(&self, u: f32, v: f32) -> [f32; 2] {
        [self.from[0] + u * self.spans[0], self.from[1] + v * self.spans[1]]
    }
}

/// Which side of the panel the tiles lie on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Left,
    Right,
}

impl Side {
    pub fn sign(self) -> f32 {
        match self {
            Side::Left => -1.0,
            Side::Right => 1.0,
        }
    }
}

/// How far the tiles' outer edge lies from the panel's middle, and how far their inner edge, in its units.
pub const TILES_REACH: f32 = 301.0;
pub const TILES_START: f32 = 215.0;

/// The side the tiles fit on, within `sides` (left, right, top, bottom, in the same units as `at`): where they are stays, unless the other has room and they have none. `at` turns a place on
/// the panel into a place where `sides` are.
pub fn side_with_room(at: impl Fn([f32; 2]) -> [f32; 2], sides: [f32; 4], was: Side) -> Side {
    let room = |side: Side| {
        let corners = [[side.sign() * TILES_REACH, -110.0], [side.sign() * TILES_REACH, 110.0]];
        corners.iter().map(|c| { let p = at(*c); (p[0] - sides[0]).min(sides[1] - p[0]) }).fold(f32::MAX, f32::min)
    };
    let other = if was == Side::Left { Side::Right } else { Side::Left };
    if room(was) < 0.0 && room(other) > room(was) {
        other
    } else {
        was
    }
}

/// A column crossing the eye's projection plane cannot be placed on that side.
pub fn side_projects(found: &Found, side: Side) -> bool {
    let Some(h) = found.perspective else { return false; };
    [side.sign()*215., side.sign()*330.].into_iter().all(|x| [-135.,135.].into_iter().all(|y| h[2][0]*x+h[2][1]*y+h[2][2] > 0.05))
}

/// Where a panel seen by the eye lies in the room, and what one of its units is there. `eye` is that eye's pose then. In the plane facing it, turned as it is seen turned, its middle moved so
/// that the tiles on `side` lie next to its edge even when a turn makes it narrower.
#[cfg(test)]
pub fn panel_in_room(found: &Found, size: [f32; 2], mirror: &Mirror, eye: &Matrix34, panel_m: f32, side: Side) -> (Matrix34, f32) {
    let on = |p: [f32; 2]| mirror.tangent(p[0] / size[0], p[1] / size[1]);
    // A place on the picture is a place in tangents; so is a way along it, and how long.
    let place = on(found.middle);
    let towards = on([found.middle[0] + found.down[0] * found.tall, found.middle[1] + found.down[1] * found.tall]);
    let along = [towards[0] - place[0], towards[1] - place[1]];
    let per_unit = along[0].hypot(along[1]).max(1e-5);
    let down = [along[0] / per_unit, along[1] / per_unit];
    let right = [down[1], -down[0]];
    let narrower = (PANEL_SIZE[0] / 2.0) * (1.0 - found.across / found.tall).max(0.0);
    let middle = [place[0] - side.sign() * right[0] * narrower * per_unit, place[1] - side.sign() * right[1] * narrower * per_unit];
    let depth = (panel_m / PANEL_SIZE[0] / per_unit).clamp(0.2, 3.0);
    let unit = per_unit * depth;
    let m = &eye.m;
    let turn = |a: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| m[i][0] * a[0] + m[i][1] * a[1] + m[i][2] * a[2]) };
    // The eye looks along -Z, with +Y up.
    let at = turn([middle[0] * depth, -middle[1] * depth, -depth]);
    let (r, u, z) = (turn([right[0], -right[1], 0.0]), turn([-down[0], down[1], 0.0]), turn([0.0, 0.0, 1.0]));
    let frame = Matrix34 { m: std::array::from_fn(|i| [r[i], u[i], z[i], at[i] + m[i][3]]) };
    (frame, unit)
}

/// Where the panel is, from what tracking already knows, as Enzo proposed: taken in a hand it goes with that controller until it is let go, and let go it stays still in the room. So nothing here
/// waits on a picture, and it does not tremble with one. What is seen is a regular recalibration, keeping this on VRChat's own panel: a little at each look, and at once only when two looks agree
/// that it is somewhere else (it was moved by what this cannot see: it flew, or it follows the head).
#[derive(Clone, Copy, Debug, Default)]
pub struct Follow {
    /// Where it shows and where it should, as what it is held by has it: the hand, or the room.
    at: Option<(Matrix34, f32)>,
    wanted: Option<(Matrix34, f32)>,
    /// A look far from there, in the room, waiting for a second.
    unsure: Option<(Matrix34, f32)>,
    held: Option<(usize, Matrix34)>,
    /// The hand moves too fast for a picture, which is older than the hand's pose, to say anything of it.
    hand_moving: bool,
    grips: [bool; 2],
}

/// How fast what shows goes to where it should, per second.
const SETTLES_PER_S: f32 = 8.0;
/// A look further than this from there takes a second look to believe, and is then believed at once.
const JUMPS_M: f32 = 0.15;
/// Nearer than this a look says nothing: depth read from a size trembles by that much.
const LOOK_SAYS_NOTHING_M: f32 = 0.006;
/// How much of what a look says it takes: little, unless it is clearly off.
const LOOK_NUDGES: f32 = 0.12;
const LOOK_CORRECTS: f32 = 0.5;
const CLEARLY_OFF_M: f32 = 0.04;
/// A hand faster than this, in metres a second, is not compared with a picture of it.
const HAND_TOO_FAST: f32 = 0.2;
/// How far around the panel a hand closing its grip takes it, and how far beside it a pointer does.
const HAND_TAKES_M: f32 = 0.14;
const POINTER_TAKES_M: f32 = 0.06;

fn apart(a: &Matrix34, b: &Matrix34) -> f32 {
    (0..3).map(|i| (a.m[i][3] - b.m[i][3]).powi(2)).sum::<f32>().sqrt()
}

fn turned_apart(a: &Matrix34, b: &Matrix34) -> f32 {
    let trace: f32 = (0..3).map(|c| (0..3).map(|i| a.m[i][c] * b.m[i][c]).sum::<f32>()).sum();
    ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos()
}

/// `a` then `b`, as frames.
fn then(a: &Matrix34, b: &Matrix34) -> Matrix34 {
    Matrix34 {
        m: std::array::from_fn(|i| {
            let row = |c: usize| a.m[i][0] * b.m[0][c] + a.m[i][1] * b.m[1][c] + a.m[i][2] * b.m[2][c];
            [row(0), row(1), row(2), row(3) + a.m[i][3]]
        }),
    }
}

/// What undoes a frame with no scale in it.
fn undone(a: &Matrix34) -> Matrix34 {
    let r = |i: usize, c: usize| a.m[c][i];
    Matrix34 { m: std::array::from_fn(|i| [r(i, 0), r(i, 1), r(i, 2), -(r(i, 0) * a.m[0][3] + r(i, 1) * a.m[1][3] + r(i, 2) * a.m[2][3])]) }
}

/// Part of the way from `a` to `b`, still a frame.
fn towards(a: &Matrix34, b: &Matrix34, k: f32) -> Matrix34 {
    let mut m = a.m;
    for (row, to) in m.iter_mut().zip(b.m.iter()) {
        for (v, t) in row.iter_mut().zip(to.iter()) {
            *v += (*t - *v) * k;
        }
    }
    let axis = |c: usize| -> [f32; 3] { [m[0][c], m[1][c], m[2][c]] };
    let unit_of = |a: [f32; 3]| -> [f32; 3] {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-5);
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let z = unit_of(axis(2));
    let x = unit_of(cross(axis(1), z));
    let y = cross(z, x);
    Matrix34 { m: std::array::from_fn(|i| [x[i], y[i], z[i], m[i][3]]) }
}

/// Whether a grip closing takes the panel at `frame`: the hand is at it, or points at it.
fn takes(frame: &Matrix34, unit: f32, hand: &Matrix34, pointer: Option<([f32; 3], [f32; 3])>) -> bool {
    let (half_w, half_h) = (PANEL_SIZE[0] / 2.0 * unit, PANEL_SIZE[1] / 2.0 * unit);
    let on_it = undone(frame);
    let local = |p: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| on_it.m[i][0] * p[0] + on_it.m[i][1] * p[1] + on_it.m[i][2] * p[2] + on_it.m[i][3]) };
    let at_hand = local([hand.m[0][3], hand.m[1][3], hand.m[2][3]]);
    if at_hand[0].abs() <= half_w + HAND_TAKES_M && at_hand[1].abs() <= half_h + HAND_TAKES_M && at_hand[2].abs() <= HAND_TAKES_M * 2.0 {
        return true;
    }
    pointer.is_some_and(|(from, along)| {
        let o = local(from);
        let d: [f32; 3] = std::array::from_fn(|i| on_it.m[i][0] * along[0] + on_it.m[i][1] * along[1] + on_it.m[i][2] * along[2]);
        if d[2].abs() < 1e-4 {
            return false;
        }
        let t = -o[2] / d[2];
        t > 0.0 && (o[0] + d[0] * t).abs() <= half_w + POINTER_TAKES_M && (o[1] + d[1] * t).abs() <= half_h + POINTER_TAKES_M
    })
}

impl Follow {
    fn held_by(&self) -> Option<&Matrix34> {
        self.held.as_ref().map(|(_, hand)| hand)
    }

    fn local(&self, frame: &Matrix34) -> Matrix34 {
        self.held_by().map_or(*frame, |hand| then(&undone(hand), frame))
    }

    fn in_room(&self, frame: &Matrix34) -> Matrix34 {
        self.held_by().map_or(*frame, |hand| then(hand, frame))
    }

    /// What a look saw, in the room.
    pub fn seen(&mut self, frame: Matrix34, unit: f32) -> bool {
        let local = self.local(&frame);
        let Some((wanted, wanted_unit)) = self.wanted else {
            self.at = Some((local, unit));
            self.wanted = Some((local, unit));
            return true;
        };
        if self.hand_moving {
            return false;
        }
        let far = apart(&wanted, &local);
        if far > JUMPS_M {
            if self.unsure.is_some_and(|(was, _)| apart(&was, &frame) <= JUMPS_M) {
                self.unsure = None;
                self.wanted = Some((local, unit));
                return true;
            } else {
                self.unsure = Some((frame, unit));
            }
            return false;
        }
        self.unsure = None;
        if far < LOOK_SAYS_NOTHING_M && turned_apart(&wanted, &local) < 2f32.to_radians() {
            return true;
        }
        let k = if far > CLEARLY_OFF_M { LOOK_CORRECTS } else { LOOK_NUDGES };
        self.wanted = Some((towards(&wanted, &local, k), wanted_unit + (unit - wanted_unit) * k));
        true
    }

    pub fn awaiting_confirmation(&self) -> bool { self.unsure.is_some() }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether a hand has it.
    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }

    /// One frame: the hands' poses, their grips, and where they point. Where it is now, in the room.
    pub fn step(&mut self, seconds: f32, hands: [Option<Matrix34>; 2], grips: [bool; 2], pointers: [Option<([f32; 3], [f32; 3])>; 2]) -> Option<(Matrix34, f32)> {
        let (at, unit) = self.at?;
        let (wanted, wanted_unit) = self.wanted.unwrap_or((at, unit));
        // The hand that has it moves it, and that is that; let go, it stays where it was let go of.
        let room = self.in_room(&at);
        let room_wanted = self.in_room(&wanted);
        let was_held = self.held;
        self.hand_moving = false;
        if let Some((hand, was)) = was_held {
            match hands[hand].filter(|_| grips[hand]) {
                Some(pose) => {
                    self.hand_moving = apart(&was, &pose) > HAND_TOO_FAST * seconds.max(0.001);
                    self.held = Some((hand, pose));
                }
                None => {
                    self.held = None;
                    self.at = Some((room, unit));
                    self.wanted = Some((room_wanted, wanted_unit));
                }
            }
        } else {
            for hand in 0..2 {
                if grips[hand] && !self.grips[hand] {
                    if let Some(pose) = hands[hand].filter(|pose| takes(&room, unit, pose, pointers[hand])) {
                        self.held = Some((hand, pose));
                        self.at = Some((then(&undone(&pose), &room), unit));
                        self.wanted = Some((then(&undone(&pose), &room_wanted), wanted_unit));
                        break;
                    }
                }
            }
        }
        self.grips = grips;
        let (at, unit) = self.at?;
        let (wanted, wanted_unit) = self.wanted.unwrap_or((at, unit));
        let k = 1.0 - (-SETTLES_PER_S * seconds).exp();
        let now = (towards(&at, &wanted, k), unit + (wanted_unit - unit) * k);
        self.at = Some(now);
        Some((self.in_room(&now.0), now.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn testdata() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata")
    }

    struct Picture {
        pixels: Vec<u8>,
        width: usize,
        height: usize,
        /// Where its X and its chevron are, read by hand.
        x: [f32; 2],
        chevron: [f32; 2],
    }

    fn picture(name: &str, x: [f32; 2], chevron: [f32; 2]) -> Picture {
        let image = image::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera").join(name)).expect("the picture").to_rgba8();
        let (width, height) = (image.width() as usize, image.height() as usize);
        Picture { pixels: image.into_raw(), width, height, x, chevron }
    }

    /// Enzo's two pictures from a headset, as they are, at 1280 x 720: its panel is 330 pixels across and its X 9, tilted each way, with its button row open and a world behind.
    fn headset_pictures() -> [Picture; 2] {
        [picture("camera-headset-1.png", [683.5, 323.0], [839.5, 451.0]), picture("camera-headset-2.png", [645.5, 393.0], [746.5, 574.5])]
    }

    /// His first small picture, of a panel straight on, and the panel on a desktop's camera, large and in another of its modes.
    fn other_pictures() -> [Picture; 2] {
        [picture("camera-panel-752x417.jpg", [204.0, 102.0], [377.0, 285.0]), picture("camera-desktop-960x540.jpg", [280.0, 169.0], [480.0, 388.0])]
    }

    /// What a picture is turned to by a lean, a size and a turn away (the width's share), about its middle. Sampled as a GPU would at that size: several times within a pixel when smaller.
    fn seen(p: &Picture, degrees: f32, scale: f32, turned: f32) -> (Vec<u8>, impl Fn([f32; 2]) -> [f32; 2] + use<>) {
        let (s, c) = degrees.to_radians().sin_cos();
        let (w, h) = (p.width, p.height);
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let mut out = vec![0u8; w * h * 4];
        let source = |x: f32, y: f32, ch: usize| -> f32 {
            let (fx, fy) = ((x - 0.5).clamp(0.0, (w - 2) as f32), (y - 0.5).clamp(0.0, (h - 2) as f32));
            let (ix, iy, ax, ay) = (fx as usize, fy as usize, fx.fract(), fy.fract());
            let at = |x: usize, y: usize| p.pixels[(y * w + x) * 4 + ch] as f32;
            (at(ix, iy) * (1.0 - ax) + at(ix + 1, iy) * ax) * (1.0 - ay) + (at(ix, iy + 1) * (1.0 - ax) + at(ix + 1, iy + 1) * ax) * ay
        };
        let samples: &[(f32, f32)] = if scale < 0.95 { &[(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] } else { &[(0.0, 0.0)] };
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0f32; 3];
                for (ox, oy) in samples {
                    let (dx, dy) = ((x as f32 + 0.5 + ox - cx) / (scale * turned), (y as f32 + 0.5 + oy - cy) / scale);
                    let (fx, fy) = (cx + dx * c + dy * s, cy - dx * s + dy * c);
                    for (ch, v) in sum.iter_mut().enumerate() {
                        *v += source(fx, fy, ch);
                    }
                }
                let i = (y * w + x) * 4;
                for ch in 0..3 {
                    out[i + ch] = (sum[ch] / samples.len() as f32) as u8;
                }
                out[i + 3] = 255;
            }
        }
        (out, move |q: [f32; 2]| {
            let (dx, dy) = ((q[0] - cx) * scale, (q[1] - cy) * scale);
            [cx + (dx * c - dy * s) * turned, cy + dx * s + dy * c]
        })
    }

    fn near(a: [f32; 2], b: [f32; 2], within: f32) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) < within
    }

    /// Enzo: "having a hard time to track the camera UI". As they are: the X and the chevron themselves, and it leans as it does there.
    #[test]
    fn it_finds_the_panel_on_enzos_pictures_as_they_are() {
        for (p, leans) in headset_pictures().iter().zip([-7.0f32, 14.0]) {
            let found = find(&p.pixels, p.width, p.height, p.width * 4).expect("the panel");
            assert!(near(found.x, p.x, 3.0) && near(found.chevron, p.chevron, 3.0), "{found:?}");
            let lean = (-found.down[0]).atan2(found.down[1]).to_degrees();
            assert!((lean - leans).abs() < 8.0, "it leans {lean}");
            assert!((found.across * PANEL_SIZE[0] - 330.0).abs() < 50.0, "{found:?}");
        }
        for p in other_pictures() {
            let found = find(&p.pixels, p.width, p.height, p.width * 4).expect("the panel");
            assert!(near(found.x, p.x, 8.0) && near(found.chevron, p.chevron, 8.0), "{found:?}");
        }
    }

    /// How often it finds them, by how large the panel is and by how it leans and turns: printed, and held to.
    #[test]
    fn how_often_it_finds_the_panel_at_the_sizes_a_person_sees() {
        let leans = [-45.0, -30.0, -15.0, 0.0, 15.0, 30.0, 45.0];
        let turns = [1.0, 0.9, 0.8];
        let pictures = headset_pictures();
        for scale in [1.3f32, 1.0, 0.8, 0.7, 0.6, 0.5, 0.4] {
            let (mut hit, mut straight, mut tries) = (0, 0, 0);
            let mut missed = Vec::new();
            for (n, p) in pictures.iter().enumerate() {
                for lean in leans {
                    for turned in turns {
                        let (picture, to) = seen(p, lean, scale, turned);
                        tries += 1;
                        let found = find(&picture, p.width, p.height, p.width * 4);
                        if found.is_some_and(|f| near(f.x, to(p.x), 9.0) && near(f.chevron, to(p.chevron), 12.0)) {
                            hit += 1;
                            straight += (turned == 1.0) as usize;
                        } else {
                            missed.push((n, lean, turned, found.is_some()));
                            if let Some(f) = found {
                                println!("  wrong at {scale} {n} {lean} {turned}: {:?} {:?} and not {:?} {:?}", f.x, f.chevron, to(p.x), to(p.chevron));
                            }
                        }
                    }
                }
            }
            println!("panel {:>3} px wide, X {:>4.1} px: {hit}/{tries} ({straight}/14 facing the eye); missed {missed:?}", (330.0 * scale) as u32, 9.0 * scale);
            // No wrong place, ever: what is found is the panel.
            assert!(missed.iter().all(|m| !m.3), "found somewhere else: {missed:?}");
            if scale >= 0.6 {
                assert!(straight >= 13, "{scale}: {straight} of 14");
                assert!(hit >= tries * 7 / 10, "{scale}: {hit} of {tries}");
            }
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
            // Parts of frames are too small to hold a panel: whole frames.
            let Ok(image) = image::open(&path) else { continue };
            let image = image.to_rgba8();
            if image.width() < 400 {
                continue;
            }
            let (width, height) = (image.width() as usize, image.height() as usize);
            checked += 1;
            if let Some(found) = find(image.as_raw(), width, height, width * 4) {
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

    /// What is read around the last place finds the same as the whole picture.
    #[test]
    fn around_the_last_place_finds_the_same() {
        for p in headset_pictures() {
            let whole = find(&p.pixels, p.width, p.height, p.width * 4).unwrap();
            let area = around(&whole, p.width, p.height);
            assert!(area[2] < p.width * 3 / 4 && area[3] <= p.height, "{area:?}");
            let there = find_in(&p.pixels, p.width, p.height, p.width * 4, area, false).unwrap();
            assert!(near(there.x, whole.x, 0.6) && near(there.chevron, whole.chevron, 0.6), "{there:?} {whole:?}");
        }
    }

    /// How long a look takes on 1280 x 720, whole and around the panel. Printed: a debug build, so far slower than what ships.
    #[test]
    fn how_long_a_look_takes() {
        let [p, _] = headset_pictures();
        let began = std::time::Instant::now();
        let whole = find(&p.pixels, p.width, p.height, p.width * 4).unwrap();
        let once = began.elapsed();
        let area = around(&whole, p.width, p.height);
        let began = std::time::Instant::now();
        find_in(&p.pixels, p.width, p.height, p.width * 4, area, false).unwrap();
        let there = began.elapsed();
        let black = vec![0u8; p.pixels.len()];
        let began = std::time::Instant::now();
        assert_eq!(find(&black, p.width, p.height, p.width * 4), None);
        println!("a look: whole {once:?}, around it {there:?}, nothing there at both sizes {:?}", began.elapsed());
    }

    /// Quest Pro's left eye, as SteamVR's logs give it.
    const QUEST_PRO: [f32; 4] = [-1.376, 0.839, -1.376, 0.900];

    /// The cause of the tiles' lying above the panel. Up is the smaller tangent here (0.900), as `stereo::eye_camera` reads it, and the picture's middle is below straight ahead.
    #[test]
    fn the_eye_reaches_further_down_than_up() {
        let eye = Mirror::whole_eye(QUEST_PRO);
        assert_eq!(eye.tangent(0.0, 0.0), [-1.376, -0.9]);
        let [right, bottom] = eye.tangent(1.0, 1.0);
        assert!((right - 0.839).abs() < 1e-4 && (bottom - 1.376).abs() < 1e-4);
        assert!(eye.tangent(0.5, 0.5)[1] > 0.2, "the middle of the picture is below straight ahead");
        // eye_camera's row 0 looks up by the same, for the 3D views that are seen right.
        assert_eq!(eye_sides(QUEST_PRO)[2], -QUEST_PRO[2].max(QUEST_PRO[3]));
    }

    fn straight_found(at: impl Fn([f32; 2]) -> [f32; 2], per_pixel: f32) -> Found {
        Found { perspective: None, residual_px: 0., landmarks: None, x: at(PANEL_X), chevron: at(PANEL_CHEVRON), middle: at([0.0, 0.0]), right: [1.0, 0.0], down: [0.0, 1.0], across: per_pixel, tall: per_pixel }
    }

    #[test]
    fn perspective_ground_truth_recovers_tilted_plane_and_separates_tiles() {
        for pitch in [-60f32, -25., 0., 40., 65.] {
            for yaw in [-55f32,0.,55.] {
                for roll in [-45f32,0.,70.] {
                    for distance in [0.3,0.7,1.8] {
                        for width in [0.2,0.3,0.5] {
                            let (sp,cp)=pitch.to_radians().sin_cos();
                            let (sy,cy)=yaw.to_radians().sin_cos();
                            let (sr,cr)=roll.to_radians().sin_cos();
                            let right=[cy*cr,cp*sr+sp*sy*cr,sp*sr-cp*sy*cr];
                            let up=[-cy*sr,cp*cr-sp*sy*sr,sp*cr+cp*sy*sr];
                            let unit=width/410.;
                            let tangent=|p:[f32;2]| {
                                let v: [f32;3]=std::array::from_fn(|i| right[i]*p[0]*unit-up[i]*p[1]*unit-if i==2 {distance} else {0.});
                                [v[0]/-v[2],-v[1]/-v[2]]
                            };
                            let pts:Vec<_>=[[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.],[82.,135.]].into_iter().map(|p|(p,tangent(p))).collect();
                            let Some(h)=fit(&pts) else { panic!("ground truth fit"); };
                            let found=Found { perspective:Some(h), ..straight_found(|p|p,1.) };
                            let mirror=Mirror {from:[0.,0.],spans:[1.,1.]};
                            let (pose,got_unit)=perspective_pose(&found,[1.,1.],&mirror,&STILL,width).expect("physical plane");
                            assert!((pose.m[2][3]+distance).abs()<0.002);
                            assert!((got_unit-unit).abs()<1e-6);
                            for side in [-1.,1.] {
                                let which = if side < 0. { Side::Left } else { Side::Right };
                                if !side_projects(&found,which) { continue; }
                                for zone in [super::super::zones::Zone::CameraPeople,super::super::zones::Zone::CameraWorld,super::super::zones::Zone::CameraQr] {
                                    let placed=super::super::zones::in_units(zone);
                                    let placed=if side>0. {placed.mirrored()} else {placed};
                                    let quad=super::super::vr::Plane {frame:pose,unit:got_unit}.camera_quad(&placed);
                                    let edge0=tangent([side*205.,-110.]);
                                    let edge1=tangent([side*205.,110.]);
                                    let centre=tangent([0.,0.]);
                                    let cross=|p:[f32;2]| (edge1[0]-edge0[0])*(p[1]-edge0[1])-(edge1[1]-edge0[1])*(p[0]-edge0[0]);
                                    for sx in [-1.,1.] { for sy in [-1.,1.] {
                                        let v: [f32;3]=std::array::from_fn(|i| quad.transform.m[i][3]+quad.transform.m[i][0]*sx*quad.width/2.+quad.transform.m[i][1]*sy*quad.height/2.);
                                        assert!(v[2]<0.);
                                        assert!(cross([v[0]/-v[2],-v[1]/-v[2]])*cross(centre)<0., "submitted camera quad crosses the panel edge");
                                    } }
                                }
                                // Tile hull and panel hull have a separating line at their common plane's edge,
                                // preserved by perspective for positive depth, including both horizontal sides.
                                for y in [-110.,0.,110.] {
                                    for tile_x in [side*215.,side*301.] {
                                    let q=tangent([tile_x,y]);
                                    let edge0=tangent([side*205.,-110.]);
                                    let edge1=tangent([side*205.,110.]);
                                    let centre=tangent([0.,0.]);
                                    let cross=|p:[f32;2]| (edge1[0]-edge0[0])*(p[1]-edge0[1])-(edge1[1]-edge0[1])*(p[0]-edge0[0]);
                                    assert!(cross(q)*cross(centre)<0.);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(fit(&[([0.,0.],[0.,0.]);5]).is_none());
    }
    #[test]
    fn a_sheared_texture_is_not_a_rigid_camera_panel() {
        let found=Found { perspective:Some([[1.,0.18,500.],[0.,1.,500.],[0.,0.,1.]]), ..straight_found(|p|p,1.) };
        let mirror=Mirror { from:[-1.,-1.],spans:[2.,2.] };
        assert!(perspective_pose(&found,[1000.,1000.],&mirror,&STILL,PANEL_M).is_none());
    }

    fn mirror() -> Mirror {
        Mirror::whole_eye([-1.0, 1.0, -1.0, 1.0])
    }

    const STILL: Matrix34 = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.5], [0.0, 0.0, 1.0, 0.0]] };

    /// A panel straight ahead, of its usual size at half a metre, comes out there, and its parts lie where they were seen.
    #[test]
    fn a_panel_seen_lands_where_it_was_seen_in_the_room() {
        let size = [1000.0, 1000.0];
        let per_unit = PANEL_M / PANEL_SIZE[0] / 0.5;
        let pixels = per_unit * 500.0;
        let found = straight_found(|p| [500.0 + p[0] * pixels, 500.0 + p[1] * pixels], pixels);
        let (frame, unit) = panel_in_room(&found, size, &mirror(), &STILL, PANEL_M, Side::Left);
        assert!((frame.m[2][3] + 0.5).abs() < 1e-3, "half a metre ahead: {:?}", frame.m);
        assert!(frame.m[0][3].abs() < 1e-3 && (frame.m[1][3] - 1.5).abs() < 1e-3, "straight ahead: {:?}", frame.m);
        assert!((unit - PANEL_M / PANEL_SIZE[0]).abs() < 1e-5);
        assert_eq!((frame.m[0][0], frame.m[1][1], frame.m[2][2]), (1.0, 1.0, 1.0), "facing the eye, upright");
    }

    /// Whatever the panel's real size, it comes back to the same place on the picture: where the tiles show comes from where it is seen. Worked back through the eye as the headset draws it.
    #[test]
    fn the_tiles_land_beside_the_panel_on_the_picture_whatever_its_size_and_tilt() {
        let eye = Mirror::whole_eye(QUEST_PRO);
        let size = [2000.0, 2055.0];
        let back = |frame: &Matrix34, unit: f32, u: [f32; 2]| -> [f32; 2] {
            // From the panel's plane to the room, the eye being STILL's, and on to the picture.
            let p: [f32; 3] = std::array::from_fn(|i| frame.m[i][0] * u[0] * unit - frame.m[i][1] * u[1] * unit + frame.m[i][3] - STILL.m[i][3]);
            let (tx, ty) = (p[0] / -p[2], -p[1] / -p[2]);
            [(tx - eye.from[0]) / eye.spans[0] * size[0], (ty - eye.from[1]) / eye.spans[1] * size[1]]
        };
        for (px, py, lean, per_pixel) in [(1000.0f32, 900.0f32, 0.0f32, 0.8f32), (1400.0, 600.0, -7.0, 1.1), (700.0, 1500.0, 14.0, 0.6), (1100.0, 1200.0, 35.0, 1.5)] {
            let (s, c) = lean.to_radians().sin_cos();
            let (right, down) = ([c, s], [-s, c]);
            let at = |p: [f32; 2]| [px + (p[0] * right[0] + p[1] * down[0]) * per_pixel, py + (p[0] * right[1] + p[1] * down[1]) * per_pixel];
            let found = Found { perspective: None, residual_px: 0., landmarks: None, x: at(PANEL_X), chevron: at(PANEL_CHEVRON), middle: at([0.0, 0.0]), right, down, across: per_pixel, tall: per_pixel };
            for panel_m in [0.2, 0.3, 0.45] {
                for side in [Side::Left, Side::Right] {
                    let (frame, unit) = panel_in_room(&found, size, &eye, &STILL, panel_m, side);
                    for spot in [[side.sign() * 258.0, 0.0], [side.sign() * 258.0, 75.0], [0.0, 0.0]] {
                        let (seen, wanted) = (back(&frame, unit, spot), at(spot));
                        assert!(near(seen, wanted, 2.0), "{lean} {panel_m} {side:?} {spot:?}: {seen:?} and not {wanted:?}");
                    }
                }
            }
        }
    }

    /// A panel turned away is narrower: the tiles stay next to its edge and not where a straight one's would be.
    #[test]
    fn a_narrower_panel_keeps_the_tiles_next_to_its_edge() {
        let size = [1000.0, 1000.0];
        let found = Found { across: 0.8 * 0.7, ..straight_found(|p| [500.0 + p[0] * 0.8, 500.0 + p[1] * 0.8], 0.8) };
        let (straight, unit) = panel_in_room(&Found { across: 0.8, ..found }, size, &mirror(), &STILL, PANEL_M, Side::Left);
        let (left, _) = panel_in_room(&found, size, &mirror(), &STILL, PANEL_M, Side::Left);
        let (right, _) = panel_in_room(&found, size, &mirror(), &STILL, PANEL_M, Side::Right);
        let units = |frame: &Matrix34| (frame.m[0][3] - straight.m[0][3]) / unit;
        assert!((units(&left) - 61.5).abs() < 1.0, "{}", units(&left));
        assert!((units(&right) + 61.5).abs() < 1.0, "{}", units(&right));
    }

    /// Enzo's first picture has the panel low and right: there is room on its left. Close to the left of the view they go to its right, and stay there until they must go back.
    #[test]
    fn the_tiles_take_the_side_with_room() {
        let picture = [0.0, 1280.0, 0.0, 720.0];
        let at = |middle: f32| move |u: [f32; 2]| [middle + u[0] * 0.8, 360.0 + u[1] * 0.8];
        assert_eq!(side_with_room(at(840.0), picture, Side::Left), Side::Left);
        assert_eq!(side_with_room(at(200.0), picture, Side::Left), Side::Right);
        assert_eq!(side_with_room(at(640.0), picture, Side::Right), Side::Right, "it stays while it fits");
        assert_eq!(side_with_room(at(1100.0), picture, Side::Right), Side::Left);
    }

    #[test]
    fn a_panel_turned_in_the_picture_is_turned_in_the_room_and_low_in_the_picture_is_low() {
        // Down the picture leaning to the left: the panel's right goes down on the right.
        let s = 20.0f32.to_radians();
        let (right, down) = ([s.cos(), s.sin()], [-s.sin(), s.cos()]);
        let size = [1000.0, 1000.0];
        let per_pixel = PANEL_M / PANEL_SIZE[0] / 0.5 * 500.0;
        let at = |p: [f32; 2]| [500.0 + (p[0] * right[0] + p[1] * down[0]) * per_pixel, 700.0 + (p[0] * right[1] + p[1] * down[1]) * per_pixel];
        let found = Found { perspective: None, residual_px: 0., landmarks: None, x: at(PANEL_X), chevron: at(PANEL_CHEVRON), middle: at([0.0, 0.0]), right, down, across: per_pixel, tall: per_pixel };
        let (frame, _) = panel_in_room(&found, size, &mirror(), &STILL, PANEL_M, Side::Left);
        assert!(frame.m[1][0] < -0.3, "the panel's right goes down: {:?}", frame.m);
        assert!(frame.m[1][3] < 1.5, "below the eye: {:?}", frame.m);
    }

    fn at(x: f32) -> Matrix34 {
        Matrix34 { m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, -0.5]] }
    }

    const NOBODY: [Option<([f32; 3], [f32; 3])>; 2] = [None, None];

    /// Enzo: "when it's dropped, it stays static". Looks that tremble do not move it, and one wrong look does not either.
    #[test]
    fn let_go_it_stays_still_and_a_look_only_recalibrates() {
        let mut follow = Follow::default();
        assert!(follow.step(0.01, [None; 2], [false; 2], NOBODY).is_none());
        follow.seen(at(0.0), 0.001);
        assert_eq!(follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3], 0.0);
        // What depth reads trembles by this: nothing moves.
        follow.seen(at(0.004), 0.001);
        assert_eq!(follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3], 0.0);
        // One look far away moves nothing.
        follow.seen(at(1.0), 0.001);
        assert!(follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3].abs() < 0.001);
        // Off by a little, again and again: it comes over gently, and does not stay apart from what VRChat shows.
        follow.seen(at(0.02), 0.001);
        let first = follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3];
        assert!(first > 0.0 && first < 0.004, "{first}");
        for _ in 0..60 {
            follow.seen(at(0.02), 0.001);
            for _ in 0..9 {
                follow.step(0.011, [None; 2], [false; 2], NOBODY);
            }
        }
        assert!((follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3] - 0.02).abs() < 0.008);
        // Two agreeing looks change the target; the rendered plane settles without teleporting.
        follow.seen(at(1.0), 0.001);
        follow.seen(at(1.0), 0.001);
        let first = follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3];
        assert!(first > 0.02 && first < 0.2);
        for _ in 0..180 { follow.step(0.011, [None; 2], [false; 2], NOBODY); }
        assert!((follow.step(0.011, [None; 2], [false; 2], NOBODY).unwrap().0.m[0][3] - 1.).abs() < 0.001);
    }
    #[test]
    fn a_far_fit_remains_provisional_until_its_target_is_adopted() {
        let mut follow=Follow::default();
        assert!(follow.seen(at(0.),0.001));
        assert!(!follow.seen(at(1.),0.001));
        assert!(follow.awaiting_confirmation());
        assert!(follow.seen(at(1.),0.001));
        assert!(!follow.awaiting_confirmation());
        assert!(follow.step(0.011,[None;2],[false;2],NOBODY).unwrap().0.m[0][3]<0.2);
    }

    /// Enzo: "use the tracking of the controller that grabbed it + the headset tracking ... until the user drops it".
    #[test]
    fn taken_in_a_hand_it_goes_with_that_hand_until_let_go() {
        let mut follow = Follow::default();
        follow.seen(at(0.0), PANEL_M / PANEL_SIZE[0]);
        follow.step(0.011, [None; 2], [false; 2], NOBODY);
        let hand = |x: f32| Some(Matrix34 { m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, -0.45]] });
        // A grip far off takes nothing, and neither does a hand held closed coming to it.
        let far = Some(Matrix34 { m: [[1.0, 0.0, 0.0, 3.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0]] });
        follow.step(0.011, [None, far], [false, true], NOBODY);
        assert!(!follow.is_held());
        follow.step(0.011, [None, hand(0.0)], [false, true], NOBODY);
        assert!(!follow.is_held(), "the grip closed elsewhere");
        follow.step(0.011, [None, hand(0.0)], [false; 2], NOBODY);
        // It closes on it: the panel goes with it exactly, with no picture.
        follow.step(0.011, [None, hand(0.0)], [false, true], NOBODY);
        assert!(follow.is_held());
        let moved = follow.step(0.011, [None, hand(0.5)], [false, true], NOBODY).unwrap().0;
        assert!((moved.m[0][3] - 0.5).abs() < 1e-3, "{:?}", moved.m);
        // What a picture says of a moving hand is older than the hand: nothing.
        follow.seen(at(0.1), PANEL_M / PANEL_SIZE[0]);
        assert!((follow.step(0.011, [None, hand(0.6)], [false, true], NOBODY).unwrap().0.m[0][3] - 0.6).abs() < 1e-3);
        // Let go, it stays there.
        let left = follow.step(0.011, [None, hand(0.6)], [false; 2], NOBODY).unwrap().0.m[0][3];
        assert!(!follow.is_held());
        assert!((follow.step(0.011, [None, hand(2.0)], [false; 2], NOBODY).unwrap().0.m[0][3] - left).abs() < 1e-5);
    }

    /// A pointer across the room takes it too, as VRChat lets it: and one pointing away does not.
    #[test]
    fn a_pointer_on_the_panel_takes_it() {
        let panel = at(0.0);
        let hand = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 2.0]] };
        assert!(takes(&panel, PANEL_M / PANEL_SIZE[0], &hand, Some(([0.0, 1.0, 2.0], [0.0, 0.0, -1.0]))));
        assert!(!takes(&panel, PANEL_M / PANEL_SIZE[0], &hand, Some(([0.0, 1.0, 2.0], [1.0, 0.0, 0.0]))));
        assert!(!takes(&panel, PANEL_M / PANEL_SIZE[0], &hand, None));
    }

    /// `then` and `undone` are what the above stands on.
    #[test]
    fn a_frame_undone_is_nothing() {
        let (s, c) = 0.7f32.sin_cos();
        let a = Matrix34 { m: [[c, 0.0, s, 1.0], [0.0, 1.0, 0.0, 2.0], [-s, 0.0, c, 3.0]] };
        let none = then(&undone(&a), &a);
        for i in 0..3 {
            for j in 0..4 {
                let wanted = if i == j { 1.0 } else { 0.0 };
                assert!((none.m[i][j] - wanted).abs() < 1e-5, "{:?}", none.m);
            }
        }
    }
}
