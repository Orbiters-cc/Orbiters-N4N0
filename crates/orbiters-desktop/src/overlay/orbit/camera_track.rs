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
    pub lines: Option<LineSet>,
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

type LineSet = [([[f32;2];2],[f32;3]);4];
type Outline = ([[f32;3];3],f32,(usize,[([f32;2],[f32;2]);10]),Option<LineSet>);

impl Found {
    fn moved(mut self, by: [f32; 2], scale: f32) -> Self {
        let at = |p: [f32; 2]| [p[0] / scale + by[0], p[1] / scale + by[1]];
        (self.x, self.chevron, self.middle) = (at(self.x), at(self.chevron), at(self.middle));
        self.across /= scale;
        self.tall /= scale;
        self.residual_px /= scale;
        if let Some(lines)=self.lines.as_mut() {for (_,l) in lines {
            l[2]=l[2]/scale-l[0]*by[0]-l[1]*by[1];
        }}
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

    /// The same, for a picture `factor` times as large: what was found on a picture read at half size, in the whole picture's pixels.
    pub fn enlarged(self, factor: f32) -> Self {
        self.moved([0.0, 0.0], 1.0 / factor)
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
    fn support(&self, all: &[Blob]) -> (usize,bool) {
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
        (icon as usize + slider as usize + (column >= 2) as usize + (buttons >= 2) as usize,slider && column>=2)
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
                let (support,opposite_sides) = reading.support(&all);
                if support < needs && !opposite_sides {
                    continue;
                }
                let fitted = landmark_fit(&reading, &all, x.middle, c.middle);
                let (perspective,residual_px,landmarks) = fitted.map_or((None,f32::INFINITY,None), |(h,e,p)| (Some(h),e,Some(p)));
                let score = looks + support as f32 * 0.2 - (x_share - X_IS).abs() * 3.0 - (c_share - CHEVRON_IS).abs() * 2.0;
                if best.as_ref().is_none_or(|b| score > b.0) {
                    let mut found = Found { perspective, lines:None, residual_px, landmarks, x: x.middle, chevron: c.middle, middle: reading.middle, right: reading.right, down: reading.down, across: reading.across, tall: reading.tall };
                    // Print omits the second top-left icon. Its left status
                    // column and right sliders still identify opposite sides;
                    // replace the missing icon vote with a measured border.
                    if support<needs || (lean.abs()>LEANS_AT_MOST.to_radians() && fitted.is_none()) {
                        let Some((h,error,points,lines))=outline_fit(light,&found) else {continue;};
                        found.perspective=Some(h);found.residual_px=error;found.landmarks=Some(points);found.lines=lines;
                    }
                    best = Some((score, found));
                }
            }
        }
    }
    best.map(|(_, mut found)| {
        // Native Photo/Stream rows have different icon centres. Locate the physical
        // viewfinder border independently; the glyph pair identifies it, not its pose.
        if let Some((h, error, points,lines)) = outline_fit(light, &found) {
            found.perspective = Some(h);
            found.residual_px = error;
            found.landmarks = Some(points);found.lines=lines;
        }
        found
    })
}

/// Native viewfinder is 16:9. PANEL_SIZE describes the older interior glyph grid,
/// not the outer border. Stereo measurements of the sandbox give 1.775:1.
const OUTLINE_HALF_HEIGHT: f32 = 205.0 * 9.0 / 16.0;

fn outline_fit(light: &Light, f: &Found) -> Option<Outline> {
    outline_search(light,f,false,false).or_else(||outline_search(light,f,true,false)).or_else(||outline_search(light,f,true,true))
}

fn outline_search(light: &Light, f: &Found, general:bool,gradient:bool) -> Option<Outline> {
    let sample = |p:[f32;2]| -> Option<f32> {
        let (x,y)=(p[0].round() as isize,p[1].round() as isize);
        if x<0 || y<0 || x>=light.width as isize || y>=light.height as isize { return None; }
        Some(light.px[y as usize*light.width+x as usize] as f32)
    };
    let scale=f.across.max(f.tall);
    let contrast=|p:[f32;2],n:[f32;2]| -> f32 {
        // Print has a gray beveled body, not a black stroke. Its material edge
        // is a first derivative; keep the same complete-plane validation.
        if gradient {
            let radius=scale.max(1.);
            return sample([p[0]+n[0]*radius,p[1]+n[1]*radius]).zip(sample([p[0]-n[0]*radius,p[1]-n[1]*radius])).map_or(0.,|(a,b)|((a-b).abs()-8.).clamp(0.,40.));
        }
        let Some(dark)=sample(p).filter(|v|*v<35.) else {return 0.;};
        let a=sample([p[0]+n[0]*4.*scale,p[1]+n[1]*4.*scale]);
        let b=sample([p[0]-n[0]*4.*scale,p[1]-n[1]*4.*scale]);
        a.zip(b).map_or(0.,|(a,b)| ((a+b)*0.5-dark-8.).clamp(0.,40.))
    };
    let seed=|x:f32,y:f32| [f.middle[0]+f.right[0]*x*f.across+f.down[0]*y*f.tall,
        f.middle[1]+f.right[1]*x*f.across+f.down[1]*y*f.tall];
    let seeds=[seed(-205.,-110.),seed(205.,-110.),seed(205.,110.),seed(-205.,110.)];
    #[derive(Clone,Copy)]
    struct Edge { p:[f32;2],u:[f32;2],error:f32 }
    let mut edges:[Vec<Edge>;4]=std::array::from_fn(|_|Vec::new());
    for i in 0..4 {
        let (a,b)=(seeds[i],seeds[(i+1)%4]);
        let length=(b[0]-a[0]).hypot(b[1]-a[1]);
        let middle=[(a[0]+b[0])*0.5,(a[1]+b[1])*0.5];
        let direction=[(b[0]-a[0])/length,(b[1]-a[1])/length];
        let mut candidates:Vec<(f32,[f32;2],[f32;2])>=Vec::new();
        let mut clipped=false;
        // A glyph similarity seed can be very far from the right border under
        // perspective. Search complete straight edges, rather than nearest blobs.
        let count=if general {181} else {33};
        for angle in 0..count {
            // The fast path is only a proposal. General acquisition searches the
            // entire half-circle: projective edges need not follow a similarity seed.
            let theta=if general {-std::f32::consts::FRAC_PI_2+std::f32::consts::PI*angle as f32/180.} else {-0.22+0.44*angle as f32/32.};
            let (s,c)=theta.sin_cos();
            let u=[direction[0]*c-direction[1]*s,direction[0]*s+direction[1]*c];
            let n=[-u[1],u[0]];
            let reach=100.*scale;
            for offset in 0..=(reach*2./0.8) as usize {
                let offset=-reach+offset as f32*0.8;
                let p=[middle[0]+offset*n[0],middle[1]+offset*n[1]];
                let score=(0..33).map(|k| {
                    let t=(-0.35+0.7*k as f32/32.)*length;
                    contrast([p[0]+t*u[0],p[1]+t*u[1]],n)
                }).sum::<f32>();
                if score<33.*10. {continue;}
                if !general {
                    if candidates.first().is_none_or(|v|score>v.0) {candidates=vec![(score,p,u)];clipped=angle==0 || angle+1==count;}
                } else {
                    // Keep distinct line hypotheses. The strongest individual
                    // line can belong to the hand behind the viewfinder; only a
                    // complete, glyph-identified quadrilateral may win.
                    let duplicate=candidates.iter().position(|&(_,q,v)| (u[0]*v[0]+u[1]*v[1]).abs()>3f32.to_radians().cos() && ((q[0]-p[0])*n[0]+(q[1]-p[1])*n[1]).abs()<6.*scale);
                    if let Some(k)=duplicate {
                        if score>candidates[k].0 {candidates[k]=(score,p,u);}
                    } else if candidates.len()<12 || score>candidates.last().unwrap().0 {
                        candidates.push((score,p,u));
                    }
                    candidates.sort_by(|a,b|b.0.total_cmp(&a.0));candidates.truncate(12);
                }
            }
        }
        // A maximum on the proposal window's boundary is not convergence.
        // Retry the unrestricted search instead of fitting a clipped edge.
        if !general && clipped {return None;}
        for (_,p,u) in candidates {
        let n=[-u[1],u[0]];
        // Refit the selected dark stroke from distributed local samples. This
        // removes angle-grid quantization without relaxing reprojection bounds.
        let mut samples=Vec::new();
        for k in 0..49 {
            let t=(-0.35+0.7*k as f32/48.)*length;
            let base=[p[0]+t*u[0],p[1]+t*u[1]];
            let mut best=(0.,0.);
            let mut weighted=(0.,0.);
            for j in -12..=12 {
                let d=j as f32*0.25;
                let score=contrast([base[0]+d*n[0],base[1]+d*n[1]],n);
                if score>best.0 {best=(score,d);}
                weighted.0+=score*d;weighted.1+=score;
            }
            if gradient && weighted.1>0. {best.1=weighted.0/weighted.1;}
            if best.0>10. {samples.push((t,best.1));}
        }
        if samples.len()<25 {continue;}
        let count=samples.len() as f32;
        let (mt,md)=(samples.iter().map(|p|p.0).sum::<f32>()/count,samples.iter().map(|p|p.1).sum::<f32>()/count);
        let slope=samples.iter().map(|&(t,d)|(t-mt)*(d-md)).sum::<f32>()/samples.iter().map(|&(t,_)|(t-mt).powi(2)).sum::<f32>();
        let offset=md-slope*mt;
        let error=(samples.iter().map(|&(t,d)|(d-offset-slope*t).powi(2)).sum::<f32>()/count).sqrt();
        if error>2.5 {continue;}
        let norm=(1.+slope*slope).sqrt();
        edges[i].push(Edge {p:[p[0]+offset*n[0],p[1]+offset*n[1]],u:[(u[0]+slope*n[0])/norm,(u[1]+slope*n[1])/norm],error});
        }
        if edges[i].is_empty() {return None;}
    }
    let model=[[-205.,-OUTLINE_HALF_HEIGHT],[205.,-OUTLINE_HALF_HEIGHT],[205.,OUTLINE_HALF_HEIGHT],[-205.,OUTLINE_HALF_HEIGHT]];
    let mut rails=Vec::new();
    if gradient {for level in BRIGHT {blobs(light,level,&mut rails);}}
    if gradient {
        let on=|p:[f32;2]| {
            let d=[p[0]-f.middle[0],p[1]-f.middle[1]];
            [(d[0]*f.right[0]+d[1]*f.right[1])/f.across,(d[0]*f.down[0]+d[1]*f.down[1])/f.tall]
        };
        let rails:Vec<_>=rails.iter().filter(|b| {
            let p=on(b.middle);
            (80.0..=210.0).contains(&p[0]) && (-70.0..=100.0).contains(&p[1]) && b.long>b.short*4. && b.long>12.*scale
        }).collect();
        let mut best=None;
        for top in &edges[0] {for left in &edges[3] {
        for one in &rails {for two in &rails {
            let separation=on(two.middle)[0]-on(one.middle)[0];
            if !(10.0..=40.0).contains(&separation){continue;}
            let solve=|| -> Option<_> {
                let edges=[*top,*left,Edge{p:one.middle,u:one.along,error:0.5},Edge{p:two.middle,u:two.along,error:0.5}];
                let models=[[[ -205.,-OUTLINE_HALF_HEIGHT],[205.,-OUTLINE_HALF_HEIGHT]],
                    [[-205.,-OUTLINE_HALF_HEIGHT],[-205.,OUTLINE_HALF_HEIGHT]],
                    [[164.,-OUTLINE_HALF_HEIGHT],[164.,OUTLINE_HALF_HEIGHT]],
                    [[186.,-OUTLINE_HALF_HEIGHT],[186.,OUTLINE_HALF_HEIGHT]]];
                let lines:LineSet=std::array::from_fn(|i| {
                    let e=edges[i];let (a,b)=(-e.u[1],e.u[0]);(models[i],[a,b,-a*e.p[0]-b*e.p[1]])
                });
                let mut rows=Vec::new();
                for (model,[a,b,c]) in lines {for [x,y] in model {
                    rows.push([a*x,a*y,a,b*x,b*y,b,c*x,c*y,-c]);
                }}
                for ([x,y],[u,v]) in [(PANEL_X,f.x),(PANEL_CHEVRON,f.chevron)] {
                    rows.push([x,y,1.,0.,0.,0.,-u*x,-u*y,u]);
                    rows.push([0.,0.,0.,x,y,1.,-v*x,-v*y,v]);
                }
                let h=homography_rows(&rows)?;
                let mut score=0.;
                let mut points=[([0.;2],[0.;2]);10];
                for (i,(p,q)) in [(PANEL_X,f.x),(PANEL_CHEVRON,f.chevron)].into_iter().enumerate() {
                    let r=project(h,p);let error=(r[0]-q[0]).hypot(r[1]-q[1]);
                    if error>16.*scale{return None;}score+=error*error;points[i]=(p,q);
                }
                for (model,l) in lines {for p in model {let q=project(h,p);score+=(l[0]*q[0]+l[1]*q[1]+l[2]).powi(2);}}
                Some((score,(h,top.error.max(left.error).max(0.5),(2,points),Some(lines))))
            };
            if let Some((score,result))=solve() {if best.as_ref().is_none_or(|(old,_)|score<*old){best=Some((score,result));}}
        }}}}
        if let Some((_,result))=best{return Some(result);}
    }
    let mut best = None;
    for a in &edges[0] {for b in &edges[1] {for c in &edges[2] {for d in &edges[3] {
    let lines=[a,b,c,d];
    let solve=|| -> Option<_> {
      let mut points=[([0.;2],[0.;2]);10];
      for i in 0..4 {
        let Edge {p,u,..}=*lines[(i+3)%4];let Edge {p:q,u:v,..}=*lines[i];
        let cross=|a:[f32;2],b:[f32;2]|a[0]*b[1]-a[1]*b[0];
        let determinant=cross(u,v);
        if determinant.abs()<0.2 {return None;}
        let t=cross([q[0]-p[0],q[1]-p[1]],v)/determinant;
        let corner=[p[0]+t*u[0],p[1]+t*u[1]];
        if (corner[0]-seeds[i][0]).hypot(corner[1]-seeds[i][1])>200.*scale {return None;}
        points[i]=(model[i],corner);
    }
    for i in 0..4 {
        let (a,b,c)=(points[i].1,points[(i+1)%4].1,points[(i+2)%4].1);
        if (b[0]-a[0])*(c[1]-b[1])-(b[1]-a[1])*(c[0]-b[0])<=0. {return None;}
    }
    let h=fit(&points[..4])?;
    // Glyph coordinates are an identity check with icon-sized tolerance, not
    // exact correspondences pretending that a raster glyph centroid is fixed.
    for (p,q) in [(PANEL_X,f.x),(PANEL_CHEVRON,f.chevron)] {
        let r=project(h,p);
        if (r[0]-q[0]).hypot(r[1]-q[1])>16.*scale {return None;}
    }
    if gradient {
        // Both white adjustment rails are on the front face, independent of
        // camera mode and slider value. A background edge to their right is
        // not the viewfinder border. Test them in the proposed panel plane.
        let inverse=|q:[f32;2]| -> Option<[f32;2]> {
            let a=h[0][0]-q[0]*h[2][0];let b=h[0][1]-q[0]*h[2][1];
            let c=h[1][0]-q[1]*h[2][0];let d=h[1][1]-q[1]*h[2][1];
            let u=q[0]-h[0][2];let v=q[1]-h[1][2];let det=a*d-b*c;
            (det.abs()>1e-8).then(||[(u*d-b*v)/det,(a*v-u*c)/det])
        };
        let rail_x:Vec<_>=rails.iter().filter(|b|b.long>=b.short*2. && b.long>=6.*scale).filter_map(|b| {
            let p=inverse(b.middle)?;
            let q=inverse([b.middle[0]+b.along[0]*b.long,b.middle[1]+b.along[1]*b.long])?;
            ((150.0..=200.0).contains(&p[0]) && (-20.0..=100.0).contains(&p[1]) && (q[0]-p[0]).abs()<(q[1]-p[1]).abs()*0.3).then_some(p[0])
        }).collect();
        if !rail_x.iter().any(|a|rail_x.iter().any(|b|(14.0..=30.0).contains(&(a-b).abs()))) {return None;}
    }
      let uncertainty=lines.iter().map(|e|e.error).fold(0.5f32,f32::max);
      Some((h,uncertainty,(4,points),None))
    };
    if let Some(result)=solve() {
        if !gradient {return Some(result);}
        // Evaluate the complete identity, rather than accepting the first four
        // strong scene edges. Print's bevel can otherwise pair the true left
        // edge with an unrelated parallel edge behind the camera.
        let score=[(PANEL_X,f.x),(PANEL_CHEVRON,f.chevron)].iter().map(|&(p,q)| {
            let r=project(result.0,p); (r[0]-q[0]).powi(2)+(r[1]-q[1]).powi(2)
        }).sum::<f32>()+result.1.powi(2);
        if best.as_ref().is_none_or(|(old,_)|score<*old) {best=Some((score,result));}
    }
    }}}}
    best.map(|(_,result)|result)
}

/// The panel in a picture of 4-byte pixels, whichever way round its colours are, or `None`.
fn project(h: [[f32; 3]; 3], p: [f32; 2]) -> [f32; 2] {
    let q = h.map(|r| r[0] * p[0] + r[1] * p[1] + r[2]);
    [q[0] / q[2], q[1] / q[2]]
}

/// Least squares homography from independently located, spread landmarks. Reprojection is in pixels.
pub fn fit(points: &[([f32; 2], [f32; 2])]) -> Option<[[f32; 3]; 3]> {
    // Four independently measured, non-collinear corners determine a plane.
    // Degenerate sets are still rejected by the pivot and rigid reprojection gates.
    if points.len() < 4 { return None; }
    let mut a = [[0.0f64; 9]; 8];
    for &(p, q) in points {
        let (x, y, u, v) = (p[0] as f64, p[1] as f64, q[0] as f64, q[1] as f64);
        for row in [[x,y,1.,0.,0.,0.,-u*x,-u*y,u], [0.,0.,0.,x,y,1.,-v*x,-v*y,v]] {
            for i in 0..8 { for j in 0..9 { a[i][j] += row[i] * row[j]; } }
        }
    }
    let h=solve_homography(a)?;
    let error = points.iter().map(|&(p,q)| { let r=project(h,p); (r[0]-q[0]).powi(2)+(r[1]-q[1]).powi(2) }).sum::<f32>() / points.len() as f32;
    (error.is_finite() && error.sqrt() <= 2.5).then_some(h)
}

fn homography_rows(rows:&[[f32;9]]) -> Option<[[f32;3];3]> {
    let mut a=[[0.;9];8];
    for row in rows {for i in 0..8 {for j in 0..9 {a[i][j]+=row[i] as f64*row[j] as f64;}}}
    solve_homography(a)
}
fn solve_homography(mut a:[[f64;9];8]) -> Option<[[f32;3];3]> {
    for k in 0..8 {
        let pivot = (k..8).max_by(|&i,&j| a[i][k].abs().total_cmp(&a[j][k].abs()))?;
        a.swap(k,pivot);
        if a[k][k].abs() < 1e-8 { return None; }
        let d = a[k][k];
        for j in k..9 { a[k][j] /= d; }
        for i in 0..8 { if i != k { let d = a[i][k]; for j in k..9 { a[i][j] -= d * a[k][j]; } } }
    }
    let h = [[a[0][8] as f32,a[1][8] as f32,a[2][8] as f32], [a[3][8] as f32,a[4][8] as f32,a[5][8] as f32], [a[6][8] as f32,a[7][8] as f32,1.]];
    Some(h)
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

#[derive(Clone,Copy)]
struct PlaneObservation { model:[f32;2],pixel:[f32;2],line:Option<[f32;3]>,inverse_eye:Matrix34,mirror:Mirror,size:[f32;2] }

fn observations(found:&Found,size:[f32;2],mirror:Mirror,eye:&Matrix34) -> Vec<PlaneObservation> {
    let points:Vec<_>=if let Some((count,points))=found.landmarks {points.into_iter().take(count).collect()} else {
        [[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.]].into_iter().map(|p|(p,found.at(p))).collect()
    };
    let mut result:Vec<_>=points.into_iter().map(|(model,pixel)|PlaneObservation {model,pixel,line:None,inverse_eye:undone(eye),mirror,size}).collect();
    if let Some(lines)=found.lines {for (model,line) in lines {for model in model {
        result.push(PlaneObservation{model,pixel:[0.;2],line:Some(line),inverse_eye:undone(eye),mirror,size});
    }}}
    result
}

/// Optimize the rigid plane in image space. Triangulation is an initializer:
/// averaging noisy 3D corners does not minimize the observed pixel error (depth
/// noise grows quadratically with range). Rotation stays on SO(3), never shear.
fn refine_plane(mut frame:Matrix34,mut unit:f32,observed:&[PlaneObservation],metric:bool) -> Option<(Matrix34,f32)> {
    let residual=|frame:&Matrix34,unit:f32| -> Option<Vec<f64>> {
        let mut out=Vec::with_capacity(observed.len()*2);
        for o in observed {
            let p:[f64;3]=std::array::from_fn(|i|frame.m[i][3] as f64+frame.m[i][0] as f64*o.model[0] as f64*unit as f64-frame.m[i][1] as f64*o.model[1] as f64*unit as f64);
            let v:[f64;3]=std::array::from_fn(|i|o.inverse_eye.m[i][3] as f64+(0..3).map(|j|o.inverse_eye.m[i][j] as f64*p[j]).sum::<f64>());
            if v[2]>=-1e-5 {return None;}
            let tangent=[v[0]/-v[2],-v[1]/-v[2]];
            let px: [f64;2]=std::array::from_fn(|k|(tangent[k]-o.mirror.from[k] as f64)/o.mirror.spans[k] as f64*o.size[k] as f64);
            if let Some(l)=o.line {out.push(l[0] as f64*px[0]+l[1] as f64*px[1]+l[2] as f64);out.push(0.);}
            else {for k in 0..2 {out.push(px[k]-o.pixel[k] as f64);}}
        }
        out.iter().all(|v|v.is_finite()).then_some(out)
    };
    let advance=|frame:&Matrix34,unit:f32,delta:[f64;7],amount:f64| {
        let rotation=[delta[3]*amount,delta[4]*amount,delta[5]*amount];
        let angle=rotation.iter().map(|v|v*v).sum::<f64>().sqrt();
        let mut next=*frame;
        if angle>1e-12 {
            let axis=rotation.map(|v|v/angle);let (s,c)=angle.sin_cos();
            for k in 0..3 {
                let v=frame.m.map(|r|r[k] as f64);let dot=(0..3).map(|i|axis[i]*v[i]).sum::<f64>();
                let cross=[axis[1]*v[2]-axis[2]*v[1],axis[2]*v[0]-axis[0]*v[2],axis[0]*v[1]-axis[1]*v[0]];
                for i in 0..3 {next.m[i][k]=(v[i]*c+cross[i]*s+axis[i]*dot*(1.-c)) as f32;}
            }
        }
        for i in 0..3 {next.m[i][3]=(frame.m[i][3] as f64+delta[i]*amount) as f32;}
        (next,(unit as f64*(delta[6]*amount).exp()) as f32)
    };
    let n=if metric {7} else {6};let mut damping=1e-4f64;
    let mut r=residual(&frame,unit)?;
    for _ in 0..24 {
        let cost=r.iter().map(|v|v*v).sum::<f64>();
        if cost<1e-10 {break;}
        let mut jacobian:[Vec<f64>;7]=std::array::from_fn(|_|Vec::new());
        for k in 0..n {
            let epsilon=if k<3 {1e-4} else {1e-3};let mut d=[0.;7];d[k]=epsilon;
            let (candidate,scale)=advance(&frame,unit,d,1.);
            jacobian[k]=residual(&candidate,scale)?.iter().zip(&r).map(|(a,b)|(a-b)/epsilon).collect();
        }
        let mut a=[[0.0f64;8];7];
        for i in 0..n {
            for j in 0..n {a[i][j]=jacobian[i].iter().zip(&jacobian[j]).map(|(a,b)|a*b).sum();}
            a[i][n]=-jacobian[i].iter().zip(&r).map(|(a,b)|a*b).sum::<f64>();
            a[i][i]+=damping*(a[i][i]+1.);
        }
        for k in 0..n {
            let pivot=(k..n).max_by(|&i,&j|a[i][k].abs().total_cmp(&a[j][k].abs()))?;a.swap(k,pivot);
            if a[k][k].abs()<1e-12 {return None;}
            let d=a[k][k];for j in k..=n {a[k][j]/=d;}
            for i in 0..n {if i!=k {let d=a[i][k];for j in k..=n {a[i][j]-=d*a[k][j];}}}
        }
        let delta=std::array::from_fn(|i|if i<n {a[i][n]} else {0.});
        let mut accepted=false;
        for amount in [1.,0.5,0.25,0.125] {
            let (candidate,scale)=advance(&frame,unit,delta,amount);
            if !scale.is_finite() || scale<=0. {continue;}
            if let Some(next)=residual(&candidate,scale).filter(|v|v.iter().map(|v|v*v).sum::<f64>()<cost) {
                frame=candidate;unit=scale;r=next;damping=(damping*0.3).max(1e-8);accepted=true;break;
            }
        }
        if !accepted {damping*=10.;if damping>1e8 {break;}}
    }
    Some((frame,unit))
}

/// The fits' pixel gates (2.5 px of reprojection, 6 px for any one landmark) were set on 1620 px eyes with the Quest Pro's projection, the
/// sandbox's: about 731 pixels per tangent. Enzo's Virtual Desktop eyes are 3168 px across the same view, twice the pixels per degree, and a glyph's
/// centre is found to as many more pixels: 3 of his 12 looks failed the gate at 2.6 to 4 px. The gates keep the same angle on any eye.
const GATES_SET_AT_PX_PER_TANGENT: f32 = 1620.0 / 2.215;

/// The reprojection gate in this picture's pixels: 2.5 px at the gates' own sharpness, never less; and for a panel seen large, 0.8% of its
/// diagonal. How well its outline or glyphs fit a flat rectangle is limited by its frame's thickness and rounded corners as much as by the
/// pixels: in the sandbox at Enzo's 3168 px eyes, a camera just opened 0.4 m away and tilted fit 6.5 and 5.4 px (both eyes, 1150 px across)
/// and was refused for minutes, while the same camera at 0.67 m fits to 0.8 px. 0.8% still refuses a sheared match.
fn gate_px(size: [f32; 2], mirror: &Mirror, h: [[f32; 3]; 3]) -> f32 {
    let sharp = 2.5 * (size[0] / mirror.spans[0].abs().max(1e-6) / GATES_SET_AT_PX_PER_TANGENT).max(1.0);
    let (a, b) = (project(h, [-205., -110.]), project(h, [205., 110.]));
    let (c, d) = (project(h, [205., -110.]), project(h, [-205., 110.]));
    let diagonal = (a[0] - b[0]).hypot(a[1] - b[1]).max((c[0] - d[0]).hypot(c[1] - d[1]));
    if diagonal.is_finite() { sharp.max(0.008 * diagonal) } else { sharp }
}

/// Perspective pose using the actual eye projection, with rigid-plane consistency rejection.
pub fn perspective_pose(found: &Found, size: [f32;2], mirror: &Mirror, eye: &Matrix34, width: f32) -> Option<(Matrix34,f32)> {
    perspective_fit(found,size,mirror,eye,width).ok()
}

/// [`perspective_pose`], saying why a sighting gives no pose: `camera_turn` logs it, so one read of `vr-test.log` explains silence.
pub fn perspective_fit(found: &Found, size: [f32;2], mirror: &Mirror, eye: &Matrix34, width: f32) -> Result<(Matrix34,f32),&'static str> {
    let h = found.perspective.ok_or("no perspective fit (glyphs only)")?; // i18n: not shown (diagnostics: why a camera look places no tools)
    let h0=h;
    let points: Vec<_> = [[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.]].into_iter().map(|p| {
        let q=project(h,p); let t=mirror.tangent(q[0]/size[0],q[1]/size[1]); (p,t)
    }).collect();
    let h=fit(&points).ok_or("tangent homography is singular")?; // i18n: not shown (diagnostics: why a camera look places no tools)
    let col = |k:usize| [h[0][k],-h[1][k],-h[2][k]];
    let norm = |v:[f32;3]| v.iter().map(|x|x*x).sum::<f32>().sqrt();
    let (a,b,t)=(col(0),col(1),col(2));
    let (na,nb)=(norm(a),norm(b));
    let dot: f32=a.iter().zip(b).map(|(x,y)|x*y).sum();
    if na.min(nb) <= 0. || (na/nb-1.).abs()>0.25 || dot.abs()/(na*nb)>0.2 { return Err("homography is not a rigid plane (axes unequal or skewed)"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    let unit=width/PANEL_SIZE[0];
    let scale=2.*unit/(na+nb);
    let right=a.map(|x|x/na);
    let mut down=b.map(|x|x/nb);
    let d: f32=right.iter().zip(down).map(|(x,y)|x*y).sum();
    down=std::array::from_fn(|i| down[i]-d*right[i]);
    let n=norm(down); let up=down.map(|x|-x/n);
    let z=[right[1]*up[2]-right[2]*up[1],right[2]*up[0]-right[0]*up[2],right[0]*up[1]-right[1]*up[0]];
    let position=t.map(|x|x*scale);
    if !(0.15..=4.).contains(&-position[2]) { return Err("depth outside 0.15 to 4 m"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    let local=Matrix34 {m:std::array::from_fn(|i|[right[i],up[i],z[i],position[i]])};
    let identity=Matrix34 {m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]};
    let (local,_)=refine_plane(local,unit,&observations(found,size,*mirror,&identity),false).ok_or("rigid refinement left the eye's view")?; // i18n: not shown (diagnostics: why a camera look places no tools)
    let right=local.m.map(|r|r[0]);let up=local.m.map(|r|r[1]);let z=local.m.map(|r|r[2]);let position=local.m.map(|r|r[3]);
    if !(0.15..=4.).contains(&-position[2]) {return Err("refined depth outside 0.15 to 4 m");} // i18n: not shown (diagnostics: why a camera look places no tools)
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
        for (p,q) in landmarks.into_iter().take(count) { errors.push(check(p,q).ok_or("a landmark falls behind the eye")?); } // i18n: not shown (diagnostics: why a camera look places no tools)
    } else {
        for &(p,_) in &points { errors.push(check(p,project(h0,p)).ok_or("a corner falls behind the eye")?); } // i18n: not shown (diagnostics: why a camera look places no tools)
    }
    if let Some(lines)=found.lines {for (model,l) in lines {for p in model {
        let v:[f32;3]=std::array::from_fn(|i|position[i]+right[i]*p[0]*unit-up[i]*p[1]*unit);
        let tangent=[v[0]/-v[2],-v[1]/-v[2]];
        let px: [f32;2]=std::array::from_fn(|i|(tangent[i]-mirror.from[i])/mirror.spans[i]*size[i]);
        errors.push((l[0]*px[0]+l[1]*px[1]+l[2]).powi(2));
    }}}
    let rms=(errors.iter().sum::<f32>()/errors.len().max(1) as f32).sqrt();
    let gate=gate_px(size,mirror,h0);
    if !rms.is_finite() || rms>gate { return Err("rigid reprojection error over the gate (2.5 px at 1620 px eyes, or 0.8% of the panel)"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    if errors.iter().any(|e| *e>(2.4*gate).powi(2)) { return Err("a landmark reprojects over 6 px (at 1620 px eyes) off"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    let local=Matrix34 { m: std::array::from_fn(|i| [right[i],up[i],z[i],position[i]]) };
    Ok((then(eye,&local),unit))
}

/// Metric panel pose from a frame-coherent stereo pair. No physical panel width is assumed.
pub fn stereo_pose(left:&Found,right:&Found,sizes:[[f32;2];2],mirrors:[Mirror;2],eyes:[Matrix34;2],head:&Matrix34) -> Option<(Matrix34,f32,f32)> {
    stereo_fit(left,right,sizes,mirrors,eyes,head).ok()
}

/// [`stereo_pose`], saying why a pair gives no metric pose.
pub fn stereo_fit(left:&Found,right:&Found,sizes:[[f32;2];2],mirrors:[Mirror;2],eyes:[Matrix34;2],head:&Matrix34) -> Result<(Matrix34,f32,f32),&'static str> {
    let dot=|a:[f32;3],b:[f32;3]| (0..3).map(|i|a[i]*b[i]).sum::<f32>();
    let norm=|v:[f32;3]| dot(v,v).sqrt();
    let normalized=|v:[f32;3]| { let n=norm(v); v.map(|x|x/n.max(1e-9)) };
    let samples=[[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.]];
    let found=[left,right];
    let mut points=[[0.;3];4];
    for (out,p) in points.iter_mut().zip(samples) {
        let mut origins=[[0.;3];2]; let mut rays=[[0.;3];2];
        for k in 0..2 {
            let q=project(found[k].perspective.ok_or(if k==0 {"left eye has no perspective fit"} else {"right eye has no perspective fit"})?,p); // i18n: not shown (diagnostics: why a camera look places no tools)
            let t=mirrors[k].tangent(q[0]/sizes[k][0],q[1]/sizes[k][1]);
            let ray=[t[0],-t[1],-1.];
            origins[k]=std::array::from_fn(|i|eyes[k].m[i][3]);
            rays[k]=normalized(std::array::from_fn(|i| (0..3).map(|j|eyes[k].m[i][j]*ray[j]).sum()));
        }
        let w=std::array::from_fn(|i|origins[0][i]-origins[1][i]);
        let b=dot(rays[0],rays[1]); let d=dot(rays[0],w); let e=dot(rays[1],w);
        let denominator=1.-b*b;
        if denominator<1e-5 { return Err("the two eyes' rays are parallel"); } // i18n: not shown (diagnostics: why a camera look places no tools)
        let (s,t)=((b*e-d)/denominator,(e-b*d)/denominator);
        if s<=0. || t<=0. { return Err("the two eyes' rays meet behind an eye"); } // i18n: not shown (diagnostics: why a camera look places no tools)
        let a:[f32;3]=std::array::from_fn(|i|origins[0][i]+s*rays[0][i]);
        let b:[f32;3]=std::array::from_fn(|i|origins[1][i]+t*rays[1][i]);
        if norm(std::array::from_fn(|i|a[i]-b[i]))>0.01 { return Err("the two eyes' rays to a corner miss each other by over 1 cm"); } // i18n: not shown (diagnostics: why a camera look places no tools)
        *out=std::array::from_fn(|i|(a[i]+b[i])*0.5);
    }
    let across:[f32;3]=std::array::from_fn(|i|(points[1][i]-points[0][i]+points[2][i]-points[3][i])*0.5);
    let down:[f32;3]=std::array::from_fn(|i|(points[3][i]-points[0][i]+points[2][i]-points[1][i])*0.5);
    let (width,height)=(norm(across),norm(down));
    if width<=0. || height<=0. || (width/height/(410./220.)-1.).abs()>0.15 { return Err("triangulated panel is not the panel's shape"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    let r=normalized(across); let along=dot(r,down);
    let u=normalized(std::array::from_fn(|i| -down[i]+r[i]*along));
    let z=[r[1]*u[2]-r[2]*u[1],r[2]*u[0]-r[0]*u[2],r[0]*u[1]-r[1]*u[0]];
    let centre:[f32;3]=std::array::from_fn(|i|points.iter().map(|p|p[i]).sum::<f32>()*0.25);
    if !(0.15..=4.).contains(&-centre[2]) { return Err("triangulated depth outside 0.15 to 4 m"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    let unit=(width/410.+height/220.)*0.5;
    let local=Matrix34 {m:std::array::from_fn(|i|[r[i],u[i],z[i],centre[i]])};
    let observed:Vec<_>=(0..2).flat_map(|k|observations(found[k],sizes[k],mirrors[k],&eyes[k])).collect();
    let (local,unit)=refine_plane(local,unit,&observed,true).ok_or("stereo refinement left an eye's view")?; // i18n: not shown (diagnostics: why a camera look places no tools)
    let r=local.m.map(|row|row[0]);let u=local.m.map(|row|row[1]);let z=local.m.map(|row|row[2]);let centre=local.m.map(|row|row[3]);
    if !(0.15..=4.).contains(&-centre[2]) {return Err("refined stereo depth outside 0.15 to 4 m");} // i18n: not shown (diagnostics: why a camera look places no tools)
    let mut errors=Vec::new();
    for k in 0..2 {
        let inverse=undone(&eyes[k]);
        let check=|p:[f32;2],q:[f32;2]| -> Option<f32> {
            let world:[f32;3]=std::array::from_fn(|i|centre[i]+r[i]*p[0]*unit-u[i]*p[1]*unit);
            let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*world[j]).sum::<f32>());
            if v[2]>=0. { return None; }
            let t=[v[0]/-v[2],-v[1]/-v[2]];
            let px=[(t[0]-mirrors[k].from[0])/mirrors[k].spans[0]*sizes[k][0],(t[1]-mirrors[k].from[1])/mirrors[k].spans[1]*sizes[k][1]];
            Some((px[0]-q[0]).powi(2)+(px[1]-q[1]).powi(2))
        };
        if let Some((count,landmarks))=found[k].landmarks {
            for (p,q) in landmarks.into_iter().take(count) { errors.push(check(p,q).ok_or("a landmark falls behind an eye")?); } // i18n: not shown (diagnostics: why a camera look places no tools)
        } else {
            for p in samples { errors.push(check(p,project(found[k].perspective.ok_or("no perspective fit")?,p)).ok_or("a corner falls behind an eye")?); } // i18n: not shown (diagnostics: why a camera look places no tools)
        }
    }
    for k in 0..2 {if let Some(lines)=found[k].lines {for (model,l) in lines {for p in model {
        let world:[f32;3]=std::array::from_fn(|i|centre[i]+r[i]*p[0]*unit-u[i]*p[1]*unit);
        let inverse=undone(&eyes[k]);
        let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*world[j]).sum::<f32>());
        let t=[v[0]/-v[2],-v[1]/-v[2]];
        let px: [f32;2]=std::array::from_fn(|i|(t[i]-mirrors[k].from[i])/mirrors[k].spans[i]*sizes[k][i]);
        errors.push((l[0]*px[0]+l[1]*px[1]+l[2]).powi(2));
    }}}}
    let rms=(errors.iter().sum::<f32>()/errors.len().max(1) as f32).sqrt();
    let gate=found.iter().zip(sizes.iter().zip(&mirrors)).filter_map(|(f,(size,mirror))|f.perspective.map(|h|gate_px(*size,mirror,h))).fold(0.,f32::max);
    if !rms.is_finite() || rms>gate { return Err("stereo reprojection error over the gate (2.5 px at 1620 px eyes, or 0.8% of the panel)"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    if errors.iter().any(|e|*e>(2.4*gate).powi(2)) { return Err("a landmark reprojects over 6 px (at 1620 px eyes) off in one eye"); } // i18n: not shown (diagnostics: why a camera look places no tools)
    let local=Matrix34 {m:std::array::from_fn(|i|[r[i],u[i],z[i],centre[i]])};
    Ok((then(head,&local),unit,rms))
}

/// Locate only the two measured identity glyphs near a projected search hint.
/// The hint is never returned as evidence: no border/pose is inferred here.
pub fn record_near(pixels:&[u8],width:usize,height:usize,stride:usize,offset:[f32;2],center:[f32;2],radius:f32) -> Option<[f32;2]> {
    let cx=center[0]-offset[0];let cy=center[1]-offset[1];
    let x0=(cx-radius).max(0.) as usize;let y0=(cy-radius).max(0.) as usize;
    let x1=((cx+radius).max(0.) as usize).min(width);let y1=((cy+radius).max(0.) as usize).min(height);
    if x1<=x0 || y1<=y0{return None;}
    let w=x1-x0;let h=y1-y0;let mut mask=vec![false;w*h];
    for y in 0..h {for x in 0..w {
        let Some(p)=pixels.get((y+y0)*stride+(x+x0)*4..(y+y0)*stride+(x+x0)*4+3) else{continue;};
        let (r,g,b)=(p[0] as u32,p[1] as u32,p[2] as u32);
        // Record is pink/red in the actual RGBA compositor readback.
        mask[y*w+x]=r>100 && r*10>b*11 && b*10>g*13 && r*10>g*15;
    }}
    let mut best=None;
    for i in 0..mask.len() {if !mask[i]{continue;}let mut stack=vec![i];mask[i]=false;let mut component=Vec::new();let(mut count,mut sx,mut sy)=(0,0.,0.);let(mut xmin,mut xmax,mut ymin,mut ymax)=(w,0,h,0);
        while let Some(j)=stack.pop(){component.push(j);let x=j%w;let y=j/w;count+=1;sx+=x as f32;sy+=y as f32;xmin=xmin.min(x);xmax=xmax.max(x);ymin=ymin.min(y);ymax=ymax.max(y);
            for (nx,ny) in [(x.wrapping_sub(1),y),(x+1,y),(x,y.wrapping_sub(1)),(x,y+1)] {if nx<w && ny<h && mask[ny*w+nx]{mask[ny*w+nx]=false;stack.push(ny*w+nx);}}
        }
        if count<5 || xmax-xmin<2 || ymax-ymin<2{continue;}
        // The Record outline encloses a dark gap. A coloured avatar stripe
        // or the filled centre dot cannot supply compositor timing evidence.
        let bw=xmax-xmin+3;let bh=ymax-ymin+3;let mut occupied=vec![false;bw*bh];
        for j in component {occupied[(j/w-ymin+1)*bw+j%w-xmin+1]=true;}
        let mut outside=vec![false;bw*bh];let mut pending=vec![0];outside[0]=true;
        while let Some(j)=pending.pop(){let x=j%bw;let y=j/bw;
            for (nx,ny) in [(x.wrapping_sub(1),y),(x+1,y),(x,y.wrapping_sub(1)),(x,y+1)] {
                if nx<bw && ny<bh {let k=ny*bw+nx;if !occupied[k] && !outside[k]{outside[k]=true;pending.push(k);}}
            }
        }
        if !occupied.iter().zip(&outside).any(|(&solid,&exterior)|!solid && !exterior){continue;}
        let p=[sx/count as f32+x0 as f32+offset[0],sy/count as f32+y0 as f32+offset[1]];let d=(p[0]-center[0]).hypot(p[1]-center[1]);
        if best.is_none_or(|(distance,_)|d<distance){best=Some((d,p));}
    }
    best.map(|(_,p)|p)
}

pub fn glyphs_near(pixels:&[u8],width:usize,height:usize,stride:usize,offset:[f32;2],hint:Found) -> Option<Found> {
    let local=hint.shifted([-offset[0],-offset[1]]);
    let radius=(24.*local.across.max(local.tall)).max(12.);
    let min=[local.x[0].min(local.chevron[0])-radius,local.x[1].min(local.chevron[1])-radius];
    let max=[local.x[0].max(local.chevron[0])+radius,local.x[1].max(local.chevron[1])+radius];
    let origin=[min[0].max(0.) as usize,min[1].max(0.) as usize];
    if origin[0]>=width || origin[1]>=height{return None;}
    let light=Light::of(pixels,width,height,stride,[origin[0],origin[1],(max[0]-origin[0] as f32).max(0.) as usize,(max[1]-origin[1] as f32).max(0.) as usize]);
    if light.width<16 || light.height<16{return None;}
    let mut all=Vec::new();for level in BRIGHT {blobs(&light,level,&mut all);}
    let position=|b:&Blob|[b.middle[0]+origin[0] as f32,b.middle[1]+origin[1] as f32];
    let distance=|b:&Blob,p:[f32;2]|{let q=position(b);(q[0]-p[0]).hypot(q[1]-p[1])};
    let lean=local.right[1].atan2(local.right[0]);
    let x=all.iter().filter(|b|b.may_be_x() && distance(b,local.x)<=radius && x_looks(&light,b.middle,b.size() as f32*0.625,lean)>=X_LOOKS).min_by(|a,b|distance(a,local.x).total_cmp(&distance(b,local.x)))?;
    let c=all.iter().filter(|b|b.may_be_chevron() && distance(b,local.chevron)<=radius && (b.along[0]*local.right[0]+b.along[1]*local.right[1]).abs()>35f32.to_radians().cos()).min_by(|a,b|distance(a,local.chevron).total_cmp(&distance(b,local.chevron)))?;
    let mut found=local;found.x=position(x);found.chevron=position(c);
    found.perspective=None;found.landmarks=None;found.lines=None;found.residual_px=f32::INFINITY;
    Some(found.shifted(offset))
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

/// A low landmark RMS alone does not bound extrapolation at an oblique panel edge.
/// Check the recovered column against the observed edge before it can replace the detached fallback.
pub fn column_clear(found: &Found, size: [f32;2], mirror: &Mirror, eye: &Matrix34, frame: &Matrix34, unit: f32, side: Side, clearance: f32) -> bool {
    // Validate actual submitted opaque parts, rather than an oversized imaginary rectangle.
    // The caption is above the panel; a separating-axis test handles both above and beside.
    let panel = [[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.]].map(|p|found.at(p));
    let inverse = undone(eye);
    // The opaque parts beside and under the camera: not the scanning corners (round the picture by design, transparent but for their strokes off it) nor the page,
    // which stands on the top edge in a plane of its own.
    let opaque = [super::zones::Zone::CameraPeople, super::zones::Zone::CameraWorld, super::zones::Zone::CameraRecord, super::zones::Zone::CameraQr, super::zones::Zone::CameraCaption, super::zones::Zone::CameraAct];
    let mut places: Vec<_> = opaque.into_iter().map(super::zones::in_units).collect();
    if side == Side::Right { for placed in &mut places { *placed=placed.mirrored(); } }
    super::vr::camera_column_out(&mut places,clearance,side==Side::Right);
    places.iter().all(|placed| {
        let quad=super::vr::Plane {frame:*frame,unit}.camera_quad(placed);
        let local=then(&inverse,&quad.transform);
        let mut tile=[[0.;2];4];
        for (out,[x,y]) in tile.iter_mut().zip([[-1.,-1.],[1.,-1.],[1.,1.],[-1.,1.]]) {
            let v:[f32;3]=std::array::from_fn(|i|local.m[i][3]+local.m[i][0]*x*quad.width/2.+local.m[i][1]*y*quad.height/2.);
            if v[2]>=-0.0001 { return false; }
            let t=[v[0]/-v[2],-v[1]/-v[2]];
            *out=[(t[0]-mirror.from[0])/mirror.spans[0]*size[0],(t[1]-mirror.from[1])/mirror.spans[1]*size[1]];
            if !out.iter().all(|x|x.is_finite()) { return false; }
        }
        if !panel.iter().flatten().all(|v|v.is_finite()) { return false; }
        let separated=[&panel,&tile].into_iter().any(|polygon| (0..4).any(|i| {
            let a=polygon[i]; let b=polygon[(i+1)%4];
            let axis=[b[1]-a[1],a[0]-b[0]];
            let range=|points:&[[f32;2];4]| points.iter().map(|p|p[0]*axis[0]+p[1]*axis[1]).fold((f32::INFINITY,f32::NEG_INFINITY),|(lo,hi),v|(lo.min(v),hi.max(v)));
            let (a,b)=(range(&panel),range(&tile));
            a.1 < b.0 || b.1 < a.0
        }));
        separated
    })
}

/// Independent observed exclusions from both eyes are required for an opaque camera attachment.
pub fn binocular_clear(observations: &[(Found,[f32;2],Mirror,Matrix34)], frame:&Matrix34, unit:f32, side:Side, clearance:f32) -> bool {
    observations.len()==2 && observations.iter().all(|(found,size,mirror,eye)|column_clear(found,*size,mirror,eye,frame,unit,side,clearance))
}

/// What one eye saw in a look, with what its picture covers and where that eye sits on the head.
#[derive(Clone, Copy, Debug)]
pub struct EyeView { pub found: Found, pub size: [f32;2], pub mirror: Mirror, pub on_head: Matrix34 }

/// Where the panel's width came from: both eyes, an earlier stereo measurement, or VRChat's usual size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidthFrom { Stereo, Measured, Nominal }

/// A sighting accepted as the panel's place: tiles may stand there.
#[derive(Clone, Copy, Debug)]
pub struct Accepted { pub frame: Matrix34, pub unit: f32, pub side: Side, pub clearance: f32, pub stereo_rms: Option<f32>, pub width_from: WidthFrom }

/// What became of one look: accepted, or why not; and why stereo was not used, if it was not.
#[derive(Clone, Copy, Debug)]
pub struct Verdict { pub accepted: Result<Accepted,&'static str>, pub stereo: Option<&'static str> }

/// The whole decision `OrbitVr::camera_turn` makes about one look, without SteamVR, so a recorded headset session can be replayed through it.
/// `head` is the head's pose when the look was read; `side` the side the tiles are on now.
pub fn accept_sighting(left:&EyeView, right:Option<&EyeView>, head:&Matrix34, measured_width:Option<f32>, nominal_width:f32, side:Side) -> Verdict {
    let eye_left=then(head,&left.on_head);
    let stereo=match right {
        None => Err("no right-eye sighting in this look"), // i18n: not shown (diagnostics: why a camera look places no tools)
        Some(r) => stereo_fit(&left.found,&r.found,[left.size,r.size],[left.mirror,r.mirror],[left.on_head,r.on_head],head),
    };
    let stereo_note=stereo.err();
    let pose=match stereo {
        Ok((frame,unit,rms)) => Ok((frame,unit,Some(rms),WidthFrom::Stereo)),
        // One eye's perspective fit of a panel of known size is a measurement too: its place in the picture is exact, only its distance scales with the
        // width taken. VRChat's usual width until both eyes measure it (Enzo's Quest Pro over Virtual Desktop gave dev.44 no stereo pose at all, and so no
        // tools). "No guess" means never placing them without a sighting, never refusing a sighting for want of a second eye.
        Err(_) => {
            let (width,from)=measured_width.map_or((nominal_width,WidthFrom::Nominal),|w|(w,WidthFrom::Measured));
            perspective_fit(&left.found,left.size,&left.mirror,&eye_left,width).map(|(f,u)|(f,u,None,from))
        }
    };
    let (frame,unit,stereo_rms,width_from)=match pose { Ok(p)=>p, Err(why)=>return Verdict{accepted:Err(why),stereo:stereo_note} };
    let clearance=((2.5+left.found.residual_px)/left.found.across.min(left.found.tall).max(0.01)).max(0.012/unit.max(1e-6));
    let clear=|side| {
        column_clear(&left.found,left.size,&left.mirror,&eye_left,&frame,unit,side,clearance)
        // The right eye checks a stereo pose only: a one-eye pose's distance is only as right as its width, so its column lands beside the panel in the
        // eye it was measured in, and a little off it in the other (by the disparity of that distance error).
        && (width_from!=WidthFrom::Stereo || right.is_none_or(|r|column_clear(&r.found,r.size,&r.mirror,&then(head,&r.on_head),&frame,unit,side,clearance)))
    };
    let other=if side==Side::Left {Side::Right} else {Side::Left};
    let side=if clear(side) {side} else if clear(other) {other} else {
        return Verdict{accepted:Err("the tiles would cover the camera on either side"),stereo:stereo_note}; // i18n: not shown (diagnostics: why a camera look places no tools)
    };
    Verdict{accepted:Ok(Accepted{frame,unit,side,clearance,stereo_rms,width_from}),stereo:stereo_note}
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
    grab_candidates: [Option<(Matrix34,Matrix34,Matrix34,u32)>;2],
    image_grabs: [Option<ImageGrab>;2],
    held_anchors: Option<[[f32;2];2]>,
    record_anchor: Option<[f32;2]>,
    scene_ages: [f32;5],
    scene_age_count: usize,
    scene_age_next: usize,
    clock_pixels: Option<([f32;2],[f32;2])>,
    clock_submissions: [Option<ClockSubmission>;32],
    clock_submission_next: usize,
}

#[derive(Clone,Copy,Debug)]
struct ClockSubmission { at:std::time::Instant, point:[f32;3], delay:f32 }

#[derive(Clone,Copy,Debug)]
struct ImageGrab { panel:Matrix34,relation:Matrix34,unit:f32,anchors:[[f32;2];2] }

/// Session-only reopening estimate. No guessed pose is ever learned as a fitted pose.
#[derive(Clone, Debug, Default)]
pub struct Reopen {
    context: Option<(Option<String>, Option<String>, bool, Option<u32>)>,
    cached: Option<ReopenPose>,
}
#[derive(Clone, Copy, Debug)]
struct ReopenPose {
    anchor: Option<(usize, u32)>,
    local: Matrix34,
    pub unit: f32,
    pub side: Side,
    pub clearance: f32,
}
impl Reopen {
    pub fn context(&mut self, world: Option<&str>, instance: Option<&str>, in_world: bool, pid: Option<u32>) -> bool {
        if self.context.as_ref().is_some_and(|(w,i,inside,process)|w.as_deref()==world && i.as_deref()==instance && *inside==in_world && *process==pid) { return false; }
        self.context=Some((world.map(str::to_owned),instance.map(str::to_owned),in_world,pid));
        self.invalidate();
        true
    }
    pub fn invalidate(&mut self) { self.cached=None; }
    pub fn remember(&mut self, frame:Matrix34, unit:f32, side:Side, clearance:f32, head:&Matrix34, hands:[Option<(u32,Matrix34)>;2], held:Option<usize>) {
        if self.context.as_ref().is_none_or(|(world,instance,in_world,pid)|world.is_none() || instance.is_none() || !in_world || pid.is_none()) { return; }
        let (anchor,base)=match held {
            Some(hand) => { let Some((device,pose))=hands[hand] else {return}; (Some((hand,device)),pose) },
            None => (None,*head),
        };
        self.cached=Some(ReopenPose {anchor,local:then(&undone(&base),&frame),unit,side,clearance});
    }
    pub fn initial(&self, head:&Matrix34, hands:[Option<(u32,Matrix34)>;2], panel_width:f32) -> (Matrix34,f32,Side,f32,bool) {
        if let Some(cached)=self.cached {
            let base=match cached.anchor {
                Some((hand,device)) => hands[hand].filter(|(now,_)| *now==device).map(|(_,pose)|pose),
                None => Some(*head),
            };
            if let Some(base)=base { return (then(&base,&cached.local),cached.unit,cached.side,cached.clearance,true); }
        }
        // Uncalibrated first-use estimate, explicitly provisional until visual fitting.
        (super::super::floating::space::before_eyes(head,0.65,0.22),panel_width/PANEL_SIZE[0],Side::Left,0.,false)
    }
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
    /// Begin from an already measured camera, never from hand proximity. A
    /// later image must prove that its two identity glyphs moved with this hand.
    pub fn begin_image_grab(&mut self,baseline:&Found,size:[f32;2],mirror:Mirror,eye:&Matrix34,hands:[Option<Matrix34>;2],grips:[bool;2]) {
        if self.is_held(){return;}
        let Some((at,unit))=self.at else{return;};let panel=self.in_room(&at);
        // Intersect the actual glyph pixels with the measured plane. Glyph
        // raster centroids need not equal the ideal UI design coordinates.
        let local=undone(&panel);
        let anchor=|q:[f32;2]| -> Option<[f32;2]> {
            let t=mirror.tangent(q[0]/size[0],q[1]/size[1]);
            let ray:[f32;3]=std::array::from_fn(|i|eye.m[i][0]*t[0]-eye.m[i][1]*t[1]-eye.m[i][2]);
            let denominator=(0..3).map(|i|panel.m[i][2]*ray[i]).sum::<f32>();
            if denominator.abs()<1e-5{return None;}
            let distance=(0..3).map(|i|panel.m[i][2]*(panel.m[i][3]-eye.m[i][3])).sum::<f32>()/denominator;
            if distance<=0.{return None;}
            let world:[f32;3]=std::array::from_fn(|i|eye.m[i][3]+ray[i]*distance);
            let p:[f32;2]=std::array::from_fn(|i|local.m[i][3]+(0..3).map(|j|local.m[i][j]*world[j]).sum::<f32>());
            Some([p[0]/unit,-p[1]/unit])
        };
        let Some(anchors)=anchor(baseline.x).zip(anchor(baseline.chevron)).map(|(x,c)|[x,c]) else{return;};
        for hand in 0..2 {
            if !grips[hand] {self.image_grabs[hand]=None;continue;}
            if self.image_grabs[hand].is_none() {if let Some(pose)=hands[hand] {
                self.image_grabs[hand]=Some(ImageGrab{panel,relation:then(&undone(&pose),&panel),unit,anchors});
            }}
        }
    }
    /// Unknown input is not a release event. It cannot initiate a hold;
    /// controller tracking loss still ends an existing hold in step().
    pub fn grip_samples(&self,samples:[Option<bool>;2]) -> [bool;2] {
        std::array::from_fn(|i|samples[i].unwrap_or(self.held_hand()==Some(i)))
    }

    /// Hand motion predicts a search region, not a visible attachment. The
    /// reader must still find the actual glyph pixels before a grab is bound.
    pub fn search_hint(&self,size:[f32;2],mirror:Mirror,eye:&Matrix34,hands:[Option<Matrix34>;2]) -> Option<Found> {
        let (frame,unit,anchors)=if let Some((hand,_))=self.held {
            let (local,unit)=self.at?;(then(&hands[hand]?,&local),unit,self.held_anchors.unwrap_or([PANEL_X,PANEL_CHEVRON]))
        } else {
            if let Some((hand,candidate))=self.image_grabs.iter().enumerate().find_map(|(i,c)|c.map(|c|(i,c))) {
                (then(&hands[hand]?,&candidate.relation),candidate.unit,candidate.anchors)
            } else {let (frame,unit)=self.at?;(frame,unit,[PANEL_X,PANEL_CHEVRON])}
        };
        let inverse=undone(eye);
        let pixel=|p:[f32;2]| -> Option<[f32;2]> {
            let world:[f32;3]=std::array::from_fn(|i|frame.m[i][3]+frame.m[i][0]*p[0]*unit-frame.m[i][1]*p[1]*unit);
            let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*world[j]).sum::<f32>());
            if v[2]>=-0.0001{return None;}
            let t=[v[0]/-v[2],-v[1]/-v[2]];
            Some(std::array::from_fn(|i|(t[i]-mirror.from[i])/mirror.spans[i]*size[i]))
        };
        let middle=pixel([0.,0.])?;let r=pixel([1.,0.])?;let d=pixel([0.,1.])?;
        let across=(r[0]-middle[0]).hypot(r[1]-middle[1]);let tall=(d[0]-middle[0]).hypot(d[1]-middle[1]);
        if across<=0. || tall<=0.{return None;}
        Some(Found{perspective:None,lines:None,landmarks:None,residual_px:f32::INFINITY,x:pixel(anchors[0])?,chevron:pixel(anchors[1])?,middle,right:[(r[0]-middle[0])/across,(r[1]-middle[1])/across],down:[(d[0]-middle[0])/tall,(d[1]-middle[1])/tall],across,tall})
    }

    /// The controller time actually visible in the scene, measured from two
    /// native camera glyphs. No age is learned from a stationary/ambiguous hand.
    /// This compensates VRChat's scene latency rather than making overlays lead it.
    pub fn match_image_hands(&self,found:&Found,size:[f32;2],mirror:Mirror,eye:&Matrix34,samples:&[(f32,[Option<Matrix34>;2])]) -> Option<(f32,[Option<Matrix34>;2])> {
        let mut candidates=Vec::new();
        for &(age,hands) in samples {
            if !(0.0..=0.2).contains(&age) {continue;}
            let Some(predicted)=self.search_hint(size,mirror,eye,hands) else{continue;};
            let error=predicted.x.into_iter().zip(found.x).chain(predicted.chevron.into_iter().zip(found.chevron)).map(|(a,b)|(a-b).powi(2)).sum::<f32>();
            candidates.push((error,age,hands,predicted));
        }
        let first=candidates.first()?.3;
        let identifiable=candidates.iter().any(|(_,_,_,p)|(p.x[0]-first.x[0]).hypot(p.x[1]-first.x[1])>=3. && (p.chevron[0]-first.chevron[0]).hypot(p.chevron[1]-first.chevron[1])>=3.);
        if !identifiable {return None;}
        let (error,age,hands,_)=candidates.into_iter().min_by(|a,b|a.0.total_cmp(&b.0))?;
        // Same six-pixel two-glyph agreement required to prove the rigid grab.
        if error>2.*6f32.powi(2) {return None;}
        Some((age,hands))
    }
    pub fn panel_in_hands(&self,hands:[Option<Matrix34>;2]) -> Option<(Matrix34,f32)> {
        let (local,unit)=self.at?;
        Some((if let Some((hand,_))=self.held {then(&hands[hand]?,&local)} else {local},unit))
    }
    /// Calibrate the actual icon raster against the measured plane while still.
    /// It is an observation of our own compositor output, never a camera detector.
    pub fn calibrate_record(&mut self,p:[f32;2],size:[f32;2],mirror:Mirror,eye:&Matrix34,hands:[Option<Matrix34>;2],still:bool) {
        // Clearance and the measured panel can settle after the first ring.
        // Re-observe its local raster position while still; a stale anchor
        // otherwise turns a layout correction into false overlay latency.
        if !still{return;}
        let Some((panel,unit))=self.panel_in_hands(hands) else{return;};let local=undone(&panel);
        let t=mirror.tangent(p[0]/size[0],p[1]/size[1]);
        let ray:[f32;3]=std::array::from_fn(|i|eye.m[i][0]*t[0]-eye.m[i][1]*t[1]-eye.m[i][2]);
        let denominator=(0..3).map(|i|panel.m[i][2]*ray[i]).sum::<f32>();
        if denominator.abs()<1e-5{return;}
        let distance=(0..3).map(|i|panel.m[i][2]*(panel.m[i][3]-eye.m[i][3])).sum::<f32>()/denominator;
        if distance<=0.{return;}
        let world:[f32;3]=std::array::from_fn(|i|eye.m[i][3]+ray[i]*distance);
        let q:[f32;2]=std::array::from_fn(|i|(local.m[i][3]+(0..3).map(|j|local.m[i][j]*world[j]).sum::<f32>())/unit);
        self.record_anchor=Some([q[0],-q[1]]);
    }
    pub fn record_ready(&self) -> bool {self.record_anchor.is_some()}
    /// Keep the actual displayed raster point and the delay used for that
    /// submission. A delayed image must not correct a newer control value.
    pub fn record_submission(&mut self,frame:Matrix34,unit:f32,at:std::time::Instant) {
        if !self.is_held(){return;}
        let Some(anchor)=self.record_anchor else{return;};
        let point=std::array::from_fn(|i|frame.m[i][3]+frame.m[i][0]*anchor[0]*unit-frame.m[i][1]*anchor[1]*unit);
        self.clock_submissions[self.clock_submission_next]=Some(ClockSubmission{at,point,delay:self.scene_age().unwrap_or(0.)});
        self.clock_submission_next=(self.clock_submission_next+1)%self.clock_submissions.len();
    }
    pub fn record_still(&self,samples:&[(f32,[Option<Matrix34>;2])]) -> bool {
        let Some(hand)=self.held_hand() else{return true;};
        let Some((first_age,first))=samples.first() else{return false;};
        let Some((last_age,_))=samples.last() else{return false;};
        if (first_age-last_age).abs()<0.15{return false;}
        let Some(first)=first[hand] else{return false;};
        samples.iter().all(|(_,poses)|poses[hand].is_some_and(|pose|apart(&first,&pose)<0.001 && turned_apart(&first,&pose)<0.25f32.to_radians()))
    }
    /// Compare scene and overlay pixels from the SAME compositor image. Delaying
    /// by the scene's absolute age would count overlay submission latency twice.
    pub fn composited_clock(&mut self,native_age:f32,native:[f32;2],record:[f32;2],size:[f32;2],mirror:Mirror,eye:&Matrix34,samples:&[(f32,[Option<Matrix34>;2])],captured:std::time::Instant) {
        if !self.is_held(){return;}
        // A stopped camera can still match moving poses left in the history.
        // Repeated scene/overlay pixels contain no new timing information;
        // treating them as new samples makes the learned delay drift at stops.
        let previous=self.clock_pixels.replace((native,record));
        let Some((was_native,was_record))=previous else{return;};
        if (native[0]-was_native[0]).hypot(native[1]-was_native[1])<0.5
            || (record[0]-was_record[0]).hypot(record[1]-was_record[1])<0.5 {return;}
        let Some(anchor)=self.record_anchor else{return;};let inverse=undone(eye);
        let mut candidates=Vec::new();
        for &(age,hands) in samples {
            let Some((frame,unit))=self.panel_in_hands(hands) else{continue;};
            let world:[f32;3]=std::array::from_fn(|i|frame.m[i][3]+frame.m[i][0]*anchor[0]*unit-frame.m[i][1]*anchor[1]*unit);
            let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*world[j]).sum::<f32>());
            if v[2]>=-0.0001{continue;}
            let t=[v[0]/-v[2],-v[1]/-v[2]];let pixel:[f32;2]=std::array::from_fn(|i|(t[i]-mirror.from[i])/mirror.spans[i]*size[i]);
            let error=(pixel[0]-record[0]).hypot(pixel[1]-record[1]);candidates.push((error,age,pixel));
        }
        let Some(first)=candidates.first().map(|c|c.2) else{return;};
        if !candidates.iter().any(|c|(c.2[0]-first[0]).hypot(c.2[1]-first[1])>=3.){return;}
        let Some((error,overlay_age,_))=candidates.into_iter().min_by(|a,b|a.0.total_cmp(&b.0)) else{return;};
        if error>6.{return;}
        let mut submitted=Vec::new();
        for s in self.clock_submissions.iter().flatten() {
            if captured.checked_duration_since(s.at).is_none_or(|age|age.as_secs_f32()>0.2){continue;}
            let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*s.point[j]).sum::<f32>());
            if v[2]>=-0.0001{continue;}
            let t=[v[0]/-v[2],-v[1]/-v[2]];let p:[f32;2]=std::array::from_fn(|i|(t[i]-mirror.from[i])/mirror.spans[i]*size[i]);
            submitted.push(((p[0]-record[0]).hypot(p[1]-record[1]),s.delay,p));
        }
        let Some(first)=submitted.first().map(|s|s.2) else{return;};
        if !submitted.iter().any(|s|(s.2[0]-first[0]).hypot(s.2[1]-first[1])>=3.){return;}
        let Some((submission_error,applied_delay,_))=submitted.into_iter().min_by(|a,b|a.0.total_cmp(&b.0)) else{return;};
        if submission_error>6.{return;}
        // Native age minus overlay age is the remaining error in THAT
        // submission, not in the newer value current when recognition finishes.
        let delay=(applied_delay+native_age-overlay_age).clamp(0.,0.2);
        if cfg!(debug_assertions) && std::env::var_os("ORBITERS_VR_SANDBOX_ROOT").is_some() {
            super::super::vr_log::line(&format!("camera relative clock fit: native_age={native_age}, overlay_age={overlay_age}, applied_delay={applied_delay}, target_delay={delay}, residual_px={error}, submission_residual_px={submission_error}")); // i18n: not shown (sandbox diagnostics)
        }
        self.scene_ages[self.scene_age_next]=delay;
        self.scene_age_next=(self.scene_age_next+1)%self.scene_ages.len();
        self.scene_age_count=(self.scene_age_count+1).min(self.scene_ages.len());
    }
    pub fn scene_age(&self) -> Option<f32> {
        if self.scene_age_count==0 {return None;}
        let mut ages=self.scene_ages;
        ages[..self.scene_age_count].sort_by(f32::total_cmp);
        Some(ages[self.scene_age_count/2])
    }

    /// Border fitting may miss during a pickup. Verify the measured plane's
    /// predicted two-glyph motion instead; stationary cameras and unrelated
    /// grips cannot pass the observed-motion check. Capture-time hands only.
    pub fn observe_image_grab(&mut self,found:&Found,size:[f32;2],mirror:Mirror,eye:&Matrix34,hands:[Option<Matrix34>;2],grips:[bool;2]) -> Option<usize> {
        if self.is_held(){return None;}
        let inverse=undone(eye);
        let pixel=|frame:&Matrix34,unit:f32,p:[f32;2]| -> Option<[f32;2]> {
            let world:[f32;3]=std::array::from_fn(|i|frame.m[i][3]+frame.m[i][0]*p[0]*unit-frame.m[i][1]*p[1]*unit);
            let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*world[j]).sum::<f32>());
            if v[2]>=-0.0001{return None;}
            let t=[v[0]/-v[2],-v[1]/-v[2]];
            Some(std::array::from_fn(|i|(t[i]-mirror.from[i])/mirror.spans[i]*size[i]))
        };
        for hand in 0..2 {
            if !grips[hand]{self.image_grabs[hand]=None;continue;}
            let Some((candidate,pose))=self.image_grabs[hand].zip(hands[hand]) else{continue;};
            let carried=then(&pose,&candidate.relation);
            let consistent=candidate.anchors.into_iter().zip([found.x,found.chevron]).all(|(model,actual)| {
                let Some((before,predicted))=pixel(&candidate.panel,candidate.unit,model).zip(pixel(&carried,candidate.unit,model)) else{return false;};
                let seen=[actual[0]-before[0],actual[1]-before[1]];
                let moved=[predicted[0]-before[0],predicted[1]-before[1]];
                let a=seen[0].hypot(seen[1]);let b=moved[0].hypot(moved[1]);
                a>=3. && b>=3. && (0.5..=2.).contains(&(a/b)) && seen[0]*moved[0]+seen[1]*moved[1]>0.
                    && (actual[0]-predicted[0]).hypot(actual[1]-predicted[1])<=6.
            });
            if consistent {
                self.held=Some((hand,pose));self.at=Some((candidate.relation,candidate.unit));self.wanted=self.at;self.unsure=None;self.grips=grips;
                self.held_anchors=Some(candidate.anchors);
                self.image_grabs=[None;2];self.grab_candidates=[None;2];return Some(hand);
            }
        }
        None
    }
    /// Express the measured plane in the holding hand at capture time before
    /// carrying it to the current hand. Otherwise even slow motion acquires
    /// the image reader's latency through every visual correction.
    pub fn seen_in_hands(&mut self,frame:Matrix34,unit:f32,captured_hands:[Option<Matrix34>;2]) -> bool {
        let current=match self.held {
            Some((hand,current)) => {
                let Some(captured)=captured_hands[hand] else {return false;};
                then(&current,&then(&undone(&captured),&frame))
            }
            None => frame,
        };
        self.seen(current,unit)
    }
    /// Verify a grab by observed rigid motion, including handles outside the
    /// viewfinder rectangle. Proximity alone misses VRChat's offset hand grip.
    /// `hands` must be sampled at the picture's time, not at recognition completion.
    pub fn observe_grab(&mut self,frame:Matrix34,unit:f32,hands:[Option<Matrix34>;2],grips:[bool;2]) -> Option<usize> {
        if self.is_held() {return None;}
        for hand in 0..2 {
            let Some(pose)=hands[hand].filter(|_|grips[hand]) else {self.grab_candidates[hand]=None;continue;};
            let local=then(&undone(&pose),&frame);
            if let Some((first_hand,first_panel,relation,count))=self.grab_candidates[hand] {
                if apart(&relation,&local)<0.01 && turned_apart(&relation,&local)<3f32.to_radians() {
                    self.grab_candidates[hand]=Some((first_hand,first_panel,relation,count+1));
                    let moved=apart(&first_hand,&pose)>0.025 && apart(&first_panel,&frame)>0.025;
                    let turned=turned_apart(&first_hand,&pose)>8f32.to_radians() && turned_apart(&first_panel,&frame)>8f32.to_radians();
                    if count>=2 && (moved || turned) {
                        self.held=Some((hand,pose));self.at=Some((local,unit));self.wanted=Some((local,unit));self.grips=grips;self.unsure=None;
                        self.grab_candidates=[None;2];return Some(hand);
                    }
                    continue;
                }
            }
            self.grab_candidates[hand]=Some((pose,frame,local,1));
        }
        None
    }
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
        if far < LOOK_SAYS_NOTHING_M && turned_apart(&wanted, &local) < 2f32.to_radians() && (unit/wanted_unit-1.).abs()<0.01 {
            return true;
        }
        let k = if far > CLEARLY_OFF_M { LOOK_CORRECTS } else { LOOK_NUDGES };
        self.wanted = Some((towards(&wanted, &local, k), wanted_unit + (unit - wanted_unit) * k));
        true
    }

    /// First validated attachment: settle from the provisional place without waiting for a second look.
    pub fn attach(&mut self, frame:Matrix34, unit:f32) {
        if self.at.is_none() { self.seen(frame,unit); }
        self.wanted=Some((self.local(&frame),unit));
        self.unsure=None;
    }
    pub fn held_hand(&self) -> Option<usize> { self.held.map(|(hand,_)|hand) }

    /// Check the pose that will actually be submitted, including hand movement and smoothing.
    /// A target change can be adopted directly only when its complete hull passes the same guard.
    pub fn safe_display(&mut self, placed: Option<(Matrix34,f32)>, clear: impl Fn(&Matrix34,f32)->bool) -> Option<(Matrix34,f32)> {
        if let Some((frame,unit)) = placed.filter(|(frame,unit)| clear(frame,*unit)) { return Some((frame,unit)); }
        let (frame,unit) = self.wanted?;
        let frame = self.in_room(&frame);
        if !clear(&frame,unit) { return None; }
        self.at = Some((self.local(&frame),unit));
        Some((frame,unit))
    }

    pub fn awaiting_confirmation(&self) -> bool { self.unsure.is_some() }

    pub fn settled(&self) -> bool {
        self.at.zip(self.wanted).is_some_and(|((at,unit),(wanted,wanted_unit))| apart(&at,&wanted)<0.01 && turned_apart(&at,&wanted)<2f32.to_radians() && (unit/wanted_unit-1.).abs()<0.01)
    }

    pub fn can_bind_hand(&self) -> bool {
        self.is_held() || (self.settled() && !self.awaiting_confirmation())
    }

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
    ///
    /// Its 294 looks run on every core: one after the other in a debug build they took five minutes, the whole of the
    /// desktop tests' run (`cargo test` waits for its slowest test, and every other one is done after about 90 s).
    /// Each look is the same as before and the results are counted in the same order.
    #[test]
    fn how_often_it_finds_the_panel_at_the_sizes_a_person_sees() {
        let leans = [-45.0, -30.0, -15.0, 0.0, 15.0, 30.0, 45.0];
        let turns = [1.0, 0.9, 0.8];
        let scales = [1.3f32, 1.0, 0.8, 0.7, 0.6, 0.5, 0.4];
        let pictures = headset_pictures();
        let mut looks: Vec<(f32, usize, f32, f32)> = Vec::new();
        for scale in scales {
            for n in 0..pictures.len() {
                for lean in leans {
                    for turned in turns {
                        looks.push((scale, n, lean, turned));
                    }
                }
            }
        }
        // What each look found, with the panel's true X and chevron in that picture.
        type Look = (Option<Found>, [f32; 2], [f32; 2]);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(looks.len());
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<Look> = {
            let mut out: Vec<Option<Look>> = (0..looks.len()).map(|_| None).collect();
            let done: Vec<Vec<(usize, Look)>> = std::thread::scope(|s| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        s.spawn(|| {
                            let mut mine = Vec::new();
                            loop {
                                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let Some(&(scale, n, lean, turned)) = looks.get(i) else { break mine };
                                let p = &pictures[n];
                                let (picture, to) = seen(p, lean, scale, turned);
                                mine.push((i, (find(&picture, p.width, p.height, p.width * 4), to(p.x), to(p.chevron))));
                            }
                        })
                    })
                    .collect();
                workers.into_iter().map(|w| w.join().unwrap()).collect()
            });
            for (i, look) in done.into_iter().flatten() {
                out[i] = Some(look);
            }
            out.into_iter().map(Option::unwrap).collect()
        };
        for scale in scales {
            let (mut hit, mut straight, mut tries) = (0, 0, 0);
            let mut missed = Vec::new();
            for (&(_, n, lean, turned), (found, x, chevron)) in looks.iter().zip(&results).filter(|(l, _)| l.0 == scale) {
                tries += 1;
                if found.is_some_and(|f| near(f.x, *x, 9.0) && near(f.chevron, *chevron, 12.0)) {
                    hit += 1;
                    straight += (turned == 1.0) as usize;
                } else {
                    missed.push((n, lean, turned, found.is_some()));
                    if let Some(f) = found {
                        println!("  wrong at {scale} {n} {lean} {turned}: {:?} {:?} and not {:?} {:?}", f.x, f.chevron, x, chevron);
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
    fn native_glyph_motion_selects_scene_time_not_the_newest_controller_pose() {
        let mirror=Mirror::whole_eye(QUEST_PRO);let size=[1620.,1800.];let eye=STILL;
        let mut panel=STILL;panel.m[2][3]=-1.;let unit=0.001;
        let baseline=straight_found(|p| {let world=[p[0]*unit,-p[1]*unit,-1.];[(world[0]-mirror.from[0])/mirror.spans[0]*size[0],(-world[1]-mirror.from[1])/mirror.spans[1]*size[1]]},1.);
        let mut follow=Follow::default();follow.attach(panel,unit);
        follow.begin_image_grab(&baseline,size,mirror,&eye,[None,Some(STILL)],[false,true]);
        let samples:Vec<_>=(0..=10).map(|i| {let mut hand=STILL;hand.m[0][3]=0.1-i as f32*0.01;(i as f32*0.01,[None,Some(hand)])}).collect();
        let native=follow.search_hint(size,mirror,&eye,samples[6].1).unwrap();
        let (age,matched)=follow.match_image_hands(&native,size,mirror,&eye,&samples).unwrap();
        assert!((matched[1].unwrap().m[0][3]-0.04).abs()<1e-5);
        assert!((age-0.06).abs()<1e-5);
        assert_eq!(follow.observe_image_grab(&native,size,mirror,&eye,matched,[false,true]),Some(1));
        // Static poses cannot identify latency, and unrelated pixels cannot
        // manufacture a different clock or camera attachment.
        let static_samples=vec![(0.,matched),(0.1,matched)];
        assert!(follow.match_image_hands(&native,size,mirror,&eye,&static_samples).is_none());
        let mut wrong=native;wrong.x[0]+=500.;wrong.chevron[0]+=500.;
        assert!(follow.match_image_hands(&wrong,size,mirror,&eye,&samples).is_none());
        assert!(follow.scene_age().is_none());
        // The scene is 60ms old, but our visible overlay is already 20ms old.
        // Only the 40ms difference is corrected; absolute scene age double counts.
        let record_world=[samples[2].1[1].unwrap().m[0][3]-0.28,-0.04];
        let record=[(record_world[0]-mirror.from[0])/mirror.spans[0]*size[0],(-record_world[1]-mirror.from[1])/mirror.spans[1]*size[1]];
        follow.calibrate_record(record,size,mirror,&eye,samples[2].1,false);
        assert!(!follow.record_ready(),"a moving held camera cannot calibrate overlay latency");
        follow.calibrate_record(record,size,mirror,&eye,samples[2].1,true);
        assert!(follow.record_ready(),"the first ring can arrive AFTER hand binding");
        assert!((follow.record_anchor.unwrap()[0]+280.).abs()<0.1);
        let still_samples=[(0.2,samples[2].1),(0.,samples[2].1)];
        assert!(follow.record_still(&still_samples));assert!(!follow.record_still(&samples));
        for hands in [samples[2].1,samples[4].1] {
            let (frame,unit)=follow.panel_in_hands(hands).unwrap();
            follow.record_submission(frame,unit,std::time::Instant::now());
        }
        follow.clock_pixels=Some(([native.x[0]-2.,native.x[1]],[record[0]-2.,record[1]]));
        follow.composited_clock(age,native.x,record,size,mirror,&eye,&samples,std::time::Instant::now());
        assert!((follow.scene_age().unwrap()-0.04).abs()<1e-4,"the observed submission needs 40ms, not absolute scene age or a correction to a newer control value");
        let delay=follow.scene_age();
        for stopped_age in [0.08,0.1,0.15,0.19] {
            follow.composited_clock(stopped_age,native.x,record,size,mirror,&eye,&samples,std::time::Instant::now());
        }
        assert_eq!(follow.scene_age(),delay,"stopped pixels must not invent extra delay from old moving history");
    }

    #[test]
    fn compositor_backlog_uses_the_delay_of_the_visible_submission() {
        let identity=Matrix34 { m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]] };
        let mirror=Mirror::whole_eye(QUEST_PRO);let size=[1620.,1800.];let eye=identity;
        let mut panel=identity;panel.m[2][3]=-1.;let unit=0.001;
        let mut follow=Follow::default();follow.attach(panel,unit);follow.held=Some((1,identity));
        let pixel=|x:f32,y:f32|[(x-mirror.from[0])/mirror.spans[0]*size[0],(-y-mirror.from[1])/mirror.spans[1]*size[1]];
        follow.calibrate_record(pixel(-0.28,-0.04),size,mirror,&eye,[None,Some(identity)],true);
        // These outputs were submitted with zero delay. Recognition is delayed
        // until a newer control value has already risen to 80ms.
        for k in 0..=5 {
            let mut frame=panel;frame.m[0][3]=0.08+k as f32*0.01;
            follow.record_submission(frame,unit,std::time::Instant::now());
        }
        follow.scene_ages=[0.08;5];follow.scene_age_count=5;follow.scene_age_next=0;
        let captured=std::time::Instant::now();
        for k in 0..=5 {
            let samples:Vec<_>=(0..=10).map(|i| {let mut hand=identity;hand.m[0][3]=0.1+k as f32*0.01-i as f32*0.01;(i as f32*0.01,[None,Some(hand)])}).collect();
            let native=follow.search_hint(size,mirror,&eye,samples[6].1).unwrap();
            let record=pixel(0.08+k as f32*0.01-0.28,-0.04);
            follow.composited_clock(0.06,native.x,record,size,mirror,&eye,&samples,captured);
        }
        assert!((follow.scene_age().unwrap()-0.04).abs()<1e-4,"60ms native minus 20ms overlay corrects the visible zero-delay submission to 40ms; adding to the newer 80ms value double-counts feedback");
    }

    #[test]
    fn stationary_record_calibration_follows_measured_layout_changes() {
        let mirror=Mirror::whole_eye(QUEST_PRO);let size=[1620.,1800.];
        let mut panel=STILL;panel.m[2][3]=-1.;let mut follow=Follow::default();follow.attach(panel,0.001);
        let raster=|x:f32|[(x-mirror.from[0])/mirror.spans[0]*size[0],(0.04-mirror.from[1])/mirror.spans[1]*size[1]];
        follow.calibrate_record(raster(-0.28),size,mirror,&STILL,[None,None],true);
        follow.calibrate_record(raster(-0.31),size,mirror,&STILL,[None,None],false);
        assert!((follow.record_anchor.unwrap()[0]+280.).abs()<0.1,"moving pixels cannot recalibrate");
        follow.calibrate_record(raster(-0.31),size,mirror,&STILL,[None,None],true);
        assert!((follow.record_anchor.unwrap()[0]+310.).abs()<0.1,"still layout refinement must not leave a stale timing anchor");
    }

    #[test]
    fn composited_record_ring_is_observed_and_blue_hands_are_not_a_clock() {
        let im=image::open(testdata().join("sandbox-record-ring.png")).unwrap().to_rgba8();
        let p=record_near(im.as_raw(),im.width() as usize,im.height() as usize,im.width() as usize*4,[0.,0.],[48.,48.],40.).unwrap();
        assert!((p[0]-48.).hypot(p[1]-48.)<2.);
        let mut stripe=[0,0,0,255].repeat(96*96);
        for y in 20..76 {for x in 42..49 {stripe[(y*96+x)*4..(y*96+x)*4+4].copy_from_slice(&[240,40,110,255]);}}
        assert!(record_near(&stripe,96,96,96*4,[0.,0.],[48.,48.],40.).is_none(),"a nearby pink avatar stripe is not the Record outline");
        let blue=[0,0,255,255].repeat(96*96);
        assert!(record_near(&blue,96,96,96*4,[0.,0.],[48.,48.],40.).is_none());
    }

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
        Found { perspective: None, lines:None, residual_px: 0., landmarks: None, x: at(PANEL_X), chevron: at(PANEL_CHEVRON), middle: at([0.0, 0.0]), right: [1.0, 0.0], down: [0.0, 1.0], across: per_pixel, tall: per_pixel }
    }

    #[test]
    fn review_reopening_is_immediate_and_reuses_the_fitted_holding_hand() {
        for hand in [0,1] {
            let mut reopen=Reopen::default(); reopen.context(Some("world"),Some("1"),true,Some(7));
            let head=STILL;
            let initial=reopen.initial(&head,[None;2],0.3);
            assert!(!initial.4);
            assert!((initial.0.m[2][3]+0.65).abs()<0.001);
            assert!((initial.0.m[1][3]-(head.m[1][3]-0.22)).abs()<0.001);
            let mut old_hand=STILL; old_hand.m[0][3]=0.2;
            let mut panel=old_hand; panel.m[2][3]=-0.4;
            let mut hands=[None;2]; hands[hand]=Some((10+hand as u32,old_hand));
            reopen.remember(panel,0.001,Side::Right,17.,&head,hands,Some(hand));
            let mut moved=old_hand; moved.m[0][3]+=0.5;
            // Include a rotation, so this proves a controller-relative transform rather than a copied offset.
            moved.m[0][0]=0.; moved.m[0][2]=1.; moved.m[2][0]=-1.; moved.m[2][2]=0.;
            hands[hand]=Some((10+hand as u32,moved));
            let (got,unit,side,gap,reused)=reopen.initial(&head,hands,0.9);
            let wanted=then(&moved,&then(&undone(&old_hand),&panel));
            assert!(reused && apart(&got,&wanted)<0.0001 && turned_apart(&got,&wanted)<0.001);
            assert_eq!((unit,side,gap),(0.001,Side::Right,17.));
            hands[hand]=Some((99,moved));
            assert!(!reopen.initial(&head,hands,0.3).4,"a replaced controller cannot inherit the old hand's calibration");
            assert!(!reopen.initial(&head,[None;2],0.3).4);
        }
    }
    #[test]
    fn review_reopening_unheld_uses_head_and_invalidates_on_context_and_tracking_changes() {
        let mut reopen=Reopen::default(); reopen.context(Some("world"),Some("1"),true,Some(7));
        let mut panel=STILL; panel.m[2][3]=-0.5;
        reopen.remember(panel,0.001,Side::Left,12.,&STILL,[None;2],None);
        let mut head=STILL; head.m[0][3]=0.4;
        assert!((reopen.initial(&head,[None;2],0.3).0.m[0][3]-0.4).abs()<0.0001);
        assert!(!reopen.context(Some("world"),Some("1"),true,Some(7)));
        assert!(reopen.initial(&head,[None;2],0.3).4);
        for (world,instance,in_world,pid) in [(Some("new"),Some("1"),true,Some(7)),(Some("world"),Some("2"),true,Some(7)),(Some("world"),Some("1"),false,Some(7)),(Some("world"),Some("1"),true,Some(8))] {
            reopen.context(Some("world"),Some("1"),true,Some(7));
            reopen.remember(panel,0.001,Side::Left,12.,&STILL,[None;2],None);
            assert!(reopen.initial(&head,[None;2],0.3).4);
            assert!(reopen.context(world,instance,in_world,pid));
            assert!(!reopen.initial(&head,[None;2],0.3).4);
            reopen.remember(panel,0.001,Side::Left,12.,&STILL,[None;2],None);
        }
        reopen.invalidate();
        assert!(!reopen.initial(&head,[None;2],0.3).4,"recenter and play-space events clear reuse");
    }
    #[test]
    fn review_unknown_session_context_cannot_be_remembered_as_calibration() {
        let mut reopen=Reopen::default();
        reopen.remember(STILL,0.001,Side::Left,0.,&STILL,[None;2],None);
        assert!(!reopen.initial(&STILL,[None;2],0.3).4);
        reopen.context(None,None,false,None);
        reopen.remember(STILL,0.001,Side::Left,0.,&STILL,[None;2],None);
        assert!(!reopen.initial(&STILL,[None;2],0.3).4);
    }

    #[test]
    fn review_first_fit_settles_from_provisional_and_guard_checks_each_submitted_step() {
        let mut follow=Follow::default();
        let (initial,unit,_,_,_)=Reopen::default().initial(&STILL,[None;2],0.3);
        follow.seen(initial,unit);
        let mut target=initial; target.m[0][3]+=0.3;
        follow.attach(target,unit);
        assert!(!follow.awaiting_confirmation());
        let first=follow.step(1./90.,[None;2],[false;2],[None;2]).unwrap();
        assert!(apart(&first.0,&initial)>0. && apart(&first.0,&target)>0.);
        let shown=follow.safe_display(Some(first),|frame,_|apart(frame,&target)<0.0001).unwrap();
        assert!(apart(&shown.0,&target)<0.0001,"unsafe crossing adopts the validated endpoint without sliding across the panel");
    }

    #[test]
    fn review_a_visible_close_panel_at_sixty_degrees_accepts_its_actual_tiles() {
        let (sn,cs)=60f32.to_radians().sin_cos(); let unit=0.5/410.;
        let pose=Matrix34 {m:[[1.,0.,0.,0.],[0.,cs,-sn,1.5],[0.,sn,cs,-0.2]]};
        let at=|p:[f32;2]| {let v=[p[0]*unit,-cs*p[1]*unit,-0.2-sn*p[1]*unit]; [v[0]/-v[2],-v[1]/-v[2]]};
        let points:Vec<_>=[[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.]].into_iter().map(|p|(p,at(p))).collect();
        let found=Found {perspective:fit(&points),..straight_found(|p|p,1.)};
        let mirror=Mirror {from:[0.,0.],spans:[1.,1.]};
        assert!([Side::Left,Side::Right].into_iter().any(|side|column_clear(&found,[1.,1.],&mirror,&STILL,&pose,unit,side,0.012/unit)));
    }

    #[test]
    fn review_fit_acceptance_sweep() {
        let mut total=0; let mut fitted=0; let mut ordinary_total=0; let mut ordinary_fitted=0;
        for pitch in [-60f32,-40.,-20.,0.,20.,40.,60.] { for yaw in [-60f32,-40.,-20.,0.,20.,40.,60.] { for roll in [-45f32,0.,45.] { for distance in [0.15,0.3,0.65] { for width in [0.2,0.3,0.5] {
            let (sp,cp)=pitch.to_radians().sin_cos(); let (sy,cy)=yaw.to_radians().sin_cos(); let (sr,cr)=roll.to_radians().sin_cos();
            let r=[cy*cr,cp*sr+sp*sy*cr,sp*sr-cp*sy*cr]; let u=[-cy*sr,cp*cr-sp*sy*sr,sp*cr+cp*sy*sr];
            let unit=width/410.;
            let at=|p:[f32;2]| { let v:[f32;3]=std::array::from_fn(|i|r[i]*p[0]*unit-u[i]*p[1]*unit-if i==2 {distance} else {0.}); [v[0]/-v[2],-v[1]/-v[2]] };
            // Only physically visible camera panels belong in this acceptance denominator.
            if [[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.]].into_iter().any(|p| distance-r[2]*p[0]*unit+u[2]*p[1]*unit <= 0.001) {continue;}
            total+=1;
            let ordinary=distance>=0.3 && width<=0.3;
            if ordinary { ordinary_total+=1; }
            let points:Vec<_>=[[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.],[82.,135.]].into_iter().map(|p|(p,at(p))).collect();
            let found=Found {perspective:fit(&points),..straight_found(|p|p,1.)};
            let mirror=Mirror {from:[0.,0.],spans:[1.,1.]};
            if let Some((pose,unit))=perspective_pose(&found,[1.,1.],&mirror,&STILL,width) {
                if [Side::Left,Side::Right].into_iter().any(|side|column_clear(&found,[1.,1.],&mirror,&STILL,&pose,unit,side,0.012/unit)) { fitted+=1; if ordinary { ordinary_fitted+=1; } }
            }
        }}}}}
        eprintln!("REVIEW SWEEP: total={total}, fitted={fitted}, provisional={}",total-fitted);
        eprintln!("REVIEW ORDINARY SWEEP: total={ordinary_total}, fitted={ordinary_fitted}, provisional={}",ordinary_total-ordinary_fitted);
        assert_eq!(ordinary_fitted,ordinary_total,"ordinary hand-held tilts through sixty degrees must not lose valid fits");
        assert!(fitted>0);
    }

    #[test]
    fn review_both_eyes_must_clear_the_submitted_hulls() {
        let size=[1000.,1000.]; let mirror=mirror(); let unit=PANEL_M/410.;
        let pose=Matrix34 {m:[[1.,0.,0.,0.],[0.,1.,0.,1.5],[0.,0.,1.,-0.5]]};
        let observations:Vec<_>=[-0.032f32,0.032].into_iter().map(|x| {
            let mut eye=STILL; eye.m[0][3]=x;
            let scale=unit/0.5*500.;
            let found=Found {perspective:Some([[scale,0.,500.-x/0.5*500.],[0.,scale,500.],[0.,0.,1.]]),..straight_found(|p|p,1.)};
            (found,size,mirror,eye)
        }).collect();
        assert!(!binocular_clear(&observations[..1],&pose,unit,Side::Left,0.));
        assert!(binocular_clear(&observations,&pose,unit,Side::Left,0.));
        let mut wrong=observations.clone();
        wrong[1].0.perspective.as_mut().unwrap()[0][2]-=80.;
        assert!(!binocular_clear(&wrong,&pose,unit,Side::Left,0.));
    }

    #[test]
    fn review_displayed_intermediate_pose_never_crosses_the_observed_camera() {
        let size=[1000.,1000.]; let mirror=mirror();
        let scale=PANEL_M/410./0.5*500.;
        let found=Found {perspective:Some([[scale,0.,500.],[0.,scale,500.],[0.,0.,1.]]),..straight_found(|p|[500.+p[0]*scale,500.+p[1]*scale],scale)};
        let (pose,unit)=perspective_pose(&found,size,&mirror,&STILL,PANEL_M).unwrap();
        let mut bad=pose; bad.m[0][3]+=0.3;
        let mut follow=Follow::default();
        follow.seen(bad,unit); follow.seen(pose,unit); follow.seen(pose,unit);
        for side in [Side::Left,Side::Right,Side::Left] {
            for _ in 0..90 {
                let raw=follow.step(1./90.,[None;2],[false;2],[None;2]);
                let shown=follow.safe_display(raw,|frame,unit|column_clear(&found,size,&mirror,&STILL,frame,unit,side,0.));
                if let Some((frame,unit))=shown { assert!(column_clear(&found,size,&mirror,&STILL,&frame,unit,side,0.)); }
            }
        }
        assert!(Follow::default().safe_display(None,|_,_|true).is_none());
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
                            for i in 0..3 {
                                assert!((pose.m[i][0]-right[i]).abs()<0.0001, "recovered right axis differs from ground truth");
                                assert!((pose.m[i][1]-up[i]).abs()<0.0001, "recovered up axis differs from ground truth");
                                let wanted=STILL.m[i][3]-if i==2 {distance} else {0.};
                                assert!((pose.m[i][3]-wanted).abs()<0.002, "recovered position differs from ground truth");
                            }
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
                                        let v: [f32;3]=std::array::from_fn(|i| quad.transform.m[i][3]+quad.transform.m[i][0]*sx*quad.width/2.+quad.transform.m[i][1]*sy*quad.height/2.-STILL.m[i][3]);
                                        assert!(v[2]<0.);
                                        assert!(cross([v[0]/-v[2],-v[1]/-v[2]])*cross(centre)<0., "submitted camera quad crosses the panel edge: {pitch} {yaw} {roll} {distance} {width} {side} {zone:?} ({sx},{sy})");
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

    #[test]
    fn a_low_residual_does_not_authorize_a_column_that_crosses_the_observed_edge() {
        let size=[1000.,1000.];
        let mirror=mirror();
        let scale=PANEL_M/410./0.5*500.;
        let found=Found { perspective:Some([[scale,0.,500.],[0.,scale,500.],[0.,0.,1.]]), ..straight_found(|p|[500.+p[0]*scale,500.+p[1]*scale],scale) };
        let (pose,unit)=perspective_pose(&found,size,&mirror,&STILL,PANEL_M).unwrap();
        assert!(column_clear(&found,size,&mirror,&STILL,&pose,unit,Side::Left,0.));
        assert!(column_clear(&found,size,&mirror,&STILL,&pose,unit,Side::Right,0.));
        let mut bad=pose;
        bad.m[0][3]-=0.03;
        assert!(!column_clear(&found,size,&mirror,&STILL,&bad,unit,Side::Right,0.));
        bad.m[0][3]+=0.06;
        assert!(!column_clear(&found,size,&mirror,&STILL,&bad,unit,Side::Left,0.));
    }

    #[test]
    fn the_uncertainty_clearance_must_not_push_the_column_through_the_eye_plane() {
        let unit=PANEL_M/410.;
        let (s,c)=70f32.to_radians().sin_cos();
        let frame=Matrix34 {m:[[c,0.,-s,0.],[0.,1.,0.,1.5],[s,0.,c,-0.3]]};
        let size=[1000.,1000.]; let mirror=mirror();
        let pts:Vec<_>=[[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.]].into_iter().map(|p| {
            let v=[c*p[0]*unit,-p[1]*unit,s*p[0]*unit-0.3];
            (p,[(v[0]/-v[2]+1.)*500.,(-v[1]/-v[2]+1.)*500.])
        }).collect();
        let found=Found {perspective:Some(fit(&pts).unwrap()),..straight_found(|p|p,1.)};
        assert!(column_clear(&found,size,&mirror,&STILL,&frame,unit,Side::Right,0.));
        assert!(!column_clear(&found,size,&mirror,&STILL,&frame,unit,Side::Right,200.));
    }

    #[test]
    fn stereo_measures_scale_with_asymmetric_eyes_and_rejects_mismatches() {
        let sizes=[[1200.,1000.];2];
        let mirrors=[Mirror {from:[-0.85,-1.1],spans:[2.2,2.]},Mirror {from:[-1.3,-0.9],spans:[2.2,2.]}];
        for pitch in [-50f32,0.,50.] { for yaw in [-50f32,0.,50.] { for roll in [-60f32,0.,60.] {
            for distance in [0.4,0.8,1.8] { for width in [0.2,0.3,0.5] {
                let (sp,cp)=pitch.to_radians().sin_cos(); let (sy,cy)=yaw.to_radians().sin_cos(); let (sr,cr)=roll.to_radians().sin_cos();
                let r=[cy*cr,cp*sr+sp*sy*cr,sp*sr-cp*sy*cr];
                let u=[-cy*sr,cp*cr-sp*sy*sr,sp*cr+cp*sy*sr];
                let (s,c)=0.06f32.sin_cos();
                let eyes=[Matrix34 {m:[[c,0.,s,-0.032],[0.,1.,0.,0.],[-s,0.,c,0.]]},Matrix34 {m:[[c,0.,-s,0.032],[0.,1.,0.,0.],[s,0.,c,0.]]}];
                let unit=width/410.;
                let observations:[Found;2]=std::array::from_fn(|k| {
                    let points:Vec<_>=[[-205.,-110.],[205.,-110.],[205.,110.],[-205.,110.],[0.,0.],[82.,135.]].into_iter().map(|p| {
                        let world:[f32;3]=std::array::from_fn(|i|r[i]*p[0]*unit-u[i]*p[1]*unit+if i==2 {-distance} else {0.});
                        let inverse=undone(&eyes[k]);
                        let v:[f32;3]=std::array::from_fn(|i|inverse.m[i][3]+(0..3).map(|j|inverse.m[i][j]*world[j]).sum::<f32>());
                        let t=[v[0]/-v[2],-v[1]/-v[2]];
                        (p,[(t[0]-mirrors[k].from[0])/mirrors[k].spans[0]*sizes[k][0],(t[1]-mirrors[k].from[1])/mirrors[k].spans[1]*sizes[k][1]])
                    }).collect();
                    Found {perspective:Some(fit(&points).unwrap()),..straight_found(|p|p,1.)}
                });
                let (pose,measured,_)=stereo_pose(&observations[0],&observations[1],sizes,mirrors,eyes,&STILL).expect("frame-coherent physical stereo plane");
                assert!((pose.m[2][3]+distance).abs()<0.005,"{pitch} {yaw} {roll} {distance} {width}: {pose:?}");
                assert!((measured*410.-width).abs()<0.003);
                let mut wrong=observations[1];
                let mut h=wrong.perspective.unwrap();
                for j in 0..3 { h[1][j]+=40.*h[2][j]; }
                wrong.perspective=Some(h);
                assert!(stereo_pose(&observations[0],&wrong,sizes,mirrors,eyes,&STILL).is_none());
            } }
        } } }
    }

    #[test]
    fn observed_glyph_motion_verifies_a_pickup_without_a_new_border_fit() {
        let eye=STILL;let mirror=mirror();let size=[1000.,1000.];let unit=PANEL_M/410.;
        let mut panel=STILL;panel.m[2][3]=-0.65;
        let pixel=|frame:&Matrix34,p:[f32;2]| {
            let v=then(&undone(&eye),frame);
            let q:[f32;3]=std::array::from_fn(|i|v.m[i][3]+v.m[i][0]*p[0]*unit-v.m[i][1]*p[1]*unit);
            [(q[0]/-q[2]+1.)*500.,(-q[1]/-q[2]+1.)*500.]
        };
        let baseline=straight_found(|p|pixel(&panel,p),1.);
        let mut hand=panel;hand.m[1][3]+=0.3;
        for moving_camera in [false,true] {
            let mut f=Follow::default();f.attach(panel,unit);
            f.begin_image_grab(&baseline,size,mirror,&eye,[None,Some(hand)],[false,true]);
            let mut moved=hand;moved.m[0][3]+=0.012;
            let mut seen=panel;if moving_camera {seen.m[0][3]+=0.012;}
            let found=straight_found(|p|pixel(&seen,p),1.);
            assert!(found.perspective.is_none(),"the border fit is deliberately absent");
            assert_eq!(f.observe_image_grab(&found,size,mirror,&eye,[None,Some(moved)],[false,true]),moving_camera.then_some(1));
            assert_eq!(f.is_held(),moving_camera);
            if moving_camera {
                moved.m[0][3]+=0.1;
                let got=f.step(1./72.,[None,Some(moved)],[false,true],[None;2]).unwrap().0;
                assert!((got.m[0][3]-0.112).abs()<0.001);
            }
        }
    }

    #[test]
    fn missing_input_does_not_release_a_verified_camera_or_initiate_a_grab() {
        let mut f=Follow::default();assert_eq!(f.grip_samples([None;2]),[false;2]);
        let mut relation=STILL;relation.m[1][3]=0.3;
        for i in 0..4 {let mut hand=STILL;hand.m[0][3]=i as f32*0.02;f.observe_grab(then(&hand,&relation),0.001,[None,Some(hand)],[false,true]);}
        assert_eq!(f.held_hand(),Some(1));
        let mut hand=STILL;hand.m[0][3]=0.3;
        let grips=f.grip_samples([None;2]);assert_eq!(grips,[false,true]);
        let held=f.step(1./72.,[None,Some(hand)],grips,[None;2]).unwrap().0;
        assert!(apart(&held,&then(&hand,&relation))<0.001);
        let released=f.grip_samples([Some(false);2]);
        let dropped=f.step(1./72.,[None,Some(hand)],released,[None;2]).unwrap().0;
        assert!(!f.is_held());assert!(apart(&held,&dropped)<0.001);
    }

    #[test]
    fn landmark_count_cannot_substitute_for_geometric_rank() {
        let collinear:Vec<_>=(0..5).map(|i|([i as f32*100.,0.],[i as f32*20.,20.])).collect();
        assert!(fit(&collinear).is_none(),"five collinear detections do not locate a plane");
        let corners=[[-205.,-115.3125],[205.,-115.3125],[205.,115.3125],[-205.,115.3125]].map(|p|(p,[500.+p[0]*0.7,500.+p[1]*0.7]));
        assert!(fit(&corners).is_some(),"four independent corners determine a plane");
    }

    #[test]
    fn a_metric_size_change_is_not_discarded_as_pose_jitter() {
        let mut follow=Follow::default();
        follow.seen(at(0.),0.001);
        follow.seen(at(0.),0.002);
        assert!(!follow.settled());
        for _ in 0..100 {
            follow.seen(at(0.),0.002);
            follow.step(0.1,[None;2],[false;2],NOBODY);
        }
        assert!((follow.step(0.1,[None;2],[false;2],NOBODY).unwrap().1-0.002).abs()<0.00002);
    }

    #[test]
    fn real_camera_fixtures_supply_spread_landmarks() {
        for picture in headset_pictures() {
            let found=find(&picture.pixels,picture.width,picture.height,picture.width*4).expect("camera candidate");
            assert!(found.perspective.is_some(),"camera candidate did not produce a spread fit: {found:?}");
            let (count,points)=found.landmarks.expect("independently observed panel landmarks");
            let points=&points[..count];
            let mut model:Vec<_>=points.iter().map(|(p,_)|*p).collect();
            let measured=if let Some(lines)=found.lines {
                let mut rows=Vec::new();
                for (ends,[a,b,c]) in lines {for [x,y] in ends {
                    model.push([x,y]);rows.push([a*x,a*y,a,b*x,b*y,b,c*x,c*y,-c]);
                }}
                for &([x,y],[u,v]) in points {
                    rows.push([x,y,1.,0.,0.,0.,-u*x,-u*y,u]);
                    rows.push([0.,0.,0.,x,y,1.,-v*x,-v*y,v]);
                }
                homography_rows(&rows).expect("point and line observations must have full rank")
            } else {fit(points).expect("point observations must have full rank")};
            let span=|axis:usize| model.iter().map(|p|p[axis]).fold(f32::NEG_INFINITY,f32::max)-model.iter().map(|p|p[axis]).fold(f32::INFINITY,f32::min);
            assert!(span(0)>=250. && span(1)>=180.,"landmarks must cover both axes of the face: {found:?}");
            let mut errors=Vec::new();
            for &(p,q) in points {let r=project(measured,p);errors.push((r[0]-q[0]).hypot(r[1]-q[1]));}
            if let Some(lines)=found.lines {for (ends,l) in lines {for p in ends {let q=project(measured,p);errors.push((l[0]*q[0]+l[1]*q[1]+l[2]).abs());}}}
            assert!(errors.iter().all(|e|*e<=6.));
            let rms=(errors.iter().map(|e|e*e).sum::<f32>()/errors.len() as f32).sqrt();
            assert!(rms<=2.5);
        }
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
            let found = Found { perspective: None, lines:None, residual_px: 0., landmarks: None, x: at(PANEL_X), chevron: at(PANEL_CHEVRON), middle: at([0.0, 0.0]), right, down, across: per_pixel, tall: per_pixel };
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
        let found = Found { perspective: None, lines:None, residual_px: 0., landmarks: None, x: at(PANEL_X), chevron: at(PANEL_CHEVRON), middle: at([0.0, 0.0]), right, down, across: per_pixel, tall: per_pixel };
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
        assert!(follow.settled());
        assert!(!follow.seen(at(1.),0.001));
        assert!(follow.awaiting_confirmation());
        assert!(!follow.can_bind_hand());
        assert!(follow.seen(at(1.),0.001));
        assert!(!follow.awaiting_confirmation());
        assert!(!follow.settled() && !follow.can_bind_hand());
        assert!(follow.step(0.011,[None;2],[false;2],NOBODY).unwrap().0.m[0][3]<0.2);
        for _ in 0..180 { follow.step(0.011,[None;2],[false;2],NOBODY); }
        assert!(follow.settled() && follow.can_bind_hand());
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

#[cfg(test)]
mod sandbox_regression {
    use super::*;
    #[test]
    fn recorded_first_pickup_binds_before_border_reacquisition() {
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/sandbox");
        let metadata:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(root.join("pickup-motion.json")).unwrap()).unwrap();
        let matrix=|v:&serde_json::Value|Matrix34{m:std::array::from_fn(|i|std::array::from_fn(|j|v[i*4+j].as_f64().unwrap() as f32))};
        let identity=Matrix34{m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]};
        let mirrors=[Mirror::whole_eye([-1.376,0.839,-1.376,0.9]),Mirror::whole_eye([-0.839,1.376,-1.376,0.9])];
        let mut eyes=[identity;2];eyes[0].m[0][3]=-0.032;eyes[1].m[0][3]=0.032;
        let mut pairs=Vec::new();
        for entry in metadata.as_array().unwrap() {
            let name=entry["name"].as_str().unwrap();let mut pair=Vec::new();
            for (i,eye) in ["left","right"].into_iter().enumerate() {
                let image=image::open(root.join(format!("{name}-{eye}.png"))).unwrap().to_rgba8();
                let found=find(image.as_raw(),image.width() as usize,image.height() as usize,image.width() as usize*4).expect("visible pickup glyphs");
                pair.push(found.shifted([entry["origins"][i][0].as_f64().unwrap() as f32,entry["origins"][i][1].as_f64().unwrap() as f32]));
            }
            pairs.push(pair);
        }
        let head=matrix(&metadata[0]["head"]);
        let (frame,unit,_)=stereo_pose(&pairs[0][0],&pairs[0][1],[[1620.,1800.];2],mirrors,eyes,&head).expect("initial measured camera");
        let hands=|entry:usize|[Some(matrix(&metadata[entry]["hands"][0])),Some(matrix(&metadata[entry]["hands"][1]))];
        let mut follow=Follow::default();follow.attach(frame,unit);
        follow.begin_image_grab(&pairs[0][0],[1620.,1800.],mirrors[0],&then(&head,&eyes[0]),hands(0),[false,true]);
        let eye=then(&matrix(&metadata[1]["head"]),&eyes[0]);
        let hint=follow.search_hint([1620.,1800.],mirrors[0],&eye,hands(1)).expect("calibrated motion search hint");
        let image=image::open(root.join("pickup-first-motion-left.png")).unwrap().to_rgba8();
        let glyphs=glyphs_near(image.as_raw(),image.width() as usize,image.height() as usize,image.width() as usize*4,[500.,850.],hint).expect("the reader must deliver actual glyphs without full border recognition");
        assert!(glyphs.perspective.is_none(),"a search hint must not become a measured homography");
        assert!((glyphs.x[0]-pairs[1][0].x[0]).hypot(glyphs.x[1]-pairs[1][0].x[1])<1.);
        assert!((glyphs.chevron[0]-pairs[1][0].chevron[0]).hypot(glyphs.chevron[1]-pairs[1][0].chevron[1])<1.);
        assert_eq!(follow.observe_image_grab(&glyphs,[1620.,1800.],mirrors[0],&eye,hands(1),[false,true]),Some(1),"the first moving native frame must verify the calibrated hand relation without waiting for a border");
        let blank=vec![0u8;image.as_raw().len()];
        assert!(glyphs_near(&blank,image.width() as usize,image.height() as usize,image.width() as usize*4,[500.,850.],hint).is_none(),"the same pose hint with no camera pixels must never be published as evidence");
    }

    #[test]
    fn delayed_images_do_not_pull_a_slowly_held_camera_backwards() {
        let identity=Matrix34 {m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]};
        let mut relation=identity;relation.m[1][3]=-0.3;relation.m[2][3]=0.1;
        let setup=|| {
            let mut f=Follow::default();
            for i in 0..4 {let mut h=identity;h.m[0][3]=i as f32*0.02;f.observe_grab(then(&h,&relation),0.001,[None,Some(h)],[false,true]);}
            f
        };
        let (mut fixed,mut old)=(setup(),setup());
        let mut last_error=0.;
        for i in 0..200 {
            let mut current=identity;current.m[0][3]=0.06+i as f32*0.001;
            let mut captured=current;captured.m[0][3]-=0.015; // 150ms old at 0.1m/s
            let picture=then(&captured,&relation);
            fixed.step(0.01,[None,Some(current)],[false,true],[None;2]);
            old.step(0.01,[None,Some(current)],[false,true],[None;2]);
            fixed.seen_in_hands(picture,0.001,[None,Some(captured)]);
            old.seen(picture,0.001);
            let wanted=then(&current,&relation);
            let got=fixed.step(0.01,[None,Some(current)],[false,true],[None;2]).unwrap().0;
            let bad=old.step(0.01,[None,Some(current)],[false,true],[None;2]).unwrap().0;
            assert!(apart(&got,&wanted)<0.001);
            last_error=apart(&bad,&wanted);
        }
        assert!(last_error>0.006,"the pre-fix correction path must demonstrate the regression: {last_error}");
    }
    #[test]
    fn offset_camera_handle_is_verified_by_motion_not_a_larger_proximity_radius() {
        let identity=Matrix34 {m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]};
        let mut relation=identity;relation.m[1][3]=-0.3;relation.m[2][3]=0.1;
        let mut follow=Follow::default();
        for i in 0..4 {
            let mut hand=identity;hand.m[0][3]=i as f32*0.02;
            follow.observe_grab(then(&hand,&relation),0.001,[None,Some(hand)],[false,true]);
        }
        assert_eq!(follow.held_hand(),Some(1));
        let mut hand=identity;hand.m[0][3]=0.5;
        let (panel,_)=follow.step(1./72.,[None,Some(hand)],[false,true],[None;2]).unwrap();
        assert!(apart(&panel,&then(&hand,&relation))<0.001,"propagate directly from the controller");
        let (dropped,_)=follow.step(1./72.,[None,Some(hand)],[false,false],[None;2]).unwrap();
        assert!(apart(&panel,&dropped)<0.001);
    }
    #[test]
    fn unrelated_grip_and_camera_motion_never_verify_a_rigid_grab() {
        let identity=Matrix34 {m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]};
        for moving_panel in [false,true] {
            let mut follow=Follow::default();
            for i in 0..10 {
                let mut hand=identity;hand.m[0][3]=i as f32*0.02;
                let mut panel=identity;panel.m[1][3]=-0.3;
                if moving_panel {panel.m[1][3]+=i as f32*0.02;}
                assert_eq!(follow.observe_grab(panel,0.001,[None,Some(hand)],[false,true]),None);
            }
            assert!(!follow.is_held());
        }
    }
    #[test]
    fn live_photo_grab_and_stream_have_a_measured_plane_in_both_eyes() {
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/sandbox");
        let mirrors=[Mirror::whole_eye([-1.376,0.839,-1.376,0.9]),Mirror::whole_eye([-0.839,1.376,-1.376,0.9])];
        let identity=Matrix34 {m:[[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]]};
        let mut eyes=[identity;2];eyes[0].m[0][3]=-0.032;eyes[1].m[0][3]=0.032;
        let mut stereo_failures=Vec::new();
        for (mode,origins) in [("cold-highlight",[[550.,900.],[0.,900.]]),("camera-open-wheel",[[650.,650.],[180.,650.]]),("camera-grab-move",[[650.,650.],[180.,650.]]),("camera-stream-baseline",[[650.,650.],[180.,650.]]),("close-open",[[500.,900.],[0.,900.]]),("close-held",[[700.,800.],[150.,800.]]),("mode-print",[[830.,570.],[350.,570.]]),("mode-drone",[[830.,570.],[350.,570.]])] {
            let mut pair=Vec::new();
            for (index,eye) in ["left","right"].into_iter().enumerate() {
                let name=format!("{mode}-{eye}.png");
                let image=image::open(root.join(&name)).unwrap().to_rgba8();
                let found=find(image.as_raw(),image.width() as usize,image.height() as usize,image.width() as usize*4).expect(&name);
                assert!(found.perspective.is_some(),"{name}: glyphs found but no usable camera plane: {found:?}");
                let found=found.shifted(origins[index]);
                assert!(perspective_pose(&found,[1620.,1800.],&mirrors[index],&eyes[index],0.3).is_some(),"{name}: measured outline must pass unchanged rigid reprojection: {found:?}");
                pair.push(found);
            }
            let Some((_,unit,error))=stereo_pose(&pair[0],&pair[1],[[1620.,1800.];2],mirrors,eyes,&identity) else {
                eprintln!("stereo failure {mode}: {pair:?}");
                stereo_failures.push(mode);continue;
            };
            assert!((0.27..0.32).contains(&(unit*PANEL_SIZE[0])),"{mode}: wrong metric width {}",unit*PANEL_SIZE[0]);
            assert!(error<=2.5,"unchanged stereo reprojection gate");
        }
        assert!(stereo_failures.is_empty(),"native stereo must recover metric planes: {stereo_failures:?}");
    }
}

/// Enzo's own session of 2026-11-13 (0.3.5-dev.44, Quest Pro over Virtual Desktop, 3168 x 3456 per eye): the panel was found with a perspective fit on
/// most looks, and no tool ever showed. Its looks, as `camera-tiles.log` recorded them, are replayed through the same decision `camera_turn` makes.
#[cfg(test)]
mod enzo_headset_replay {
    use super::*;

    const SIZE: [f32; 2] = [3168., 3456.];
    /// Quest Pro's eyes as SteamVR gives them (see `the_eye_reaches_further_down_than_up`), the right one mirrored; his vrserver.txt says EyeToHead is
    /// a translation of 31.5 mm each way.
    const LEFT: [f32; 4] = [-1.376, 0.839, -1.376, 0.900];
    const RIGHT: [f32; 4] = [-0.839, 1.376, -1.376, 0.900];
    const IDENTITY: Matrix34 = Matrix34 { m: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]] };

    fn on_head(x: f32, yaw_degrees: f32) -> Matrix34 {
        let (s, c) = yaw_degrees.to_radians().sin_cos();
        Matrix34 { m: [[c, 0., s, x], [0., 1., 0., 0.], [-s, 0., c, 0.]] }
    }

    /// Standing, eyes 1.6 m up, looking ahead and a little down, as one does at a camera held in front.
    fn head() -> Matrix34 {
        let (s, c) = (-20f32).to_radians().sin_cos();
        Matrix34 { m: [[1., 0., 0., 0.1], [0., c, -s, 1.6], [0., s, c, -0.2]] }
    }

    /// The numbers after `key` in a logged `Found`, up to the next field.
    fn numbers(text: &str, key: &str) -> Vec<f32> {
        const FIELDS: [&str; 11] = ["perspective", "lines", "residual_px", "landmarks", "x", "chevron", "middle", "right", "down", "across", "tall"];
        let from = text.find(&format!(" {key}: ")).unwrap_or_else(|| panic!("{key} in {text}")) + key.len() + 3;
        let to = FIELDS.iter().filter_map(|k| text[from..].find(&format!(", {k}: ")).map(|i| from + i)).min().unwrap_or(text.len());
        text[from..to].split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')).filter_map(|t| t.parse().ok()).collect()
    }

    /// Every look of that session that found the panel, as the reader handed it over.
    fn sightings() -> Vec<Found> {
        let log = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/enzo-2026-11-13/camera-tiles.txt")).unwrap();
        log.lines().filter_map(|line| line.split_once("fit=Found {").map(|(_, f)| f.to_string())).map(|f| {
            let field = |key: &str| numbers(&f, key);
            let two = |key: &str| { let v = field(key); [v[0], v[1]] };
            let perspective = f.contains("perspective: Some").then(|| { let v = field("perspective"); [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]] });
            let landmarks = f.contains("landmarks: Some").then(|| {
                let v = field("landmarks");
                let count = v[0] as usize;
                let mut out = [([0.; 2], [0.; 2]); 10];
                for (i, slot) in out.iter_mut().enumerate().take(count) { let k = 1 + i * 4; *slot = ([v[k], v[k + 1]], [v[k + 2], v[k + 3]]); }
                (count, out)
            });
            let residual = if f.contains("residual_px: inf") { f32::INFINITY } else { field("residual_px")[0] };
            Found { perspective, lines: None, residual_px: residual, landmarks, x: two("x"), chevron: two("chevron"), middle: two("middle"), right: two("right"), down: two("down"), across: field("across")[0], tall: field("tall")[0] }
        }).collect()
    }

    fn left_view(found: Found) -> EyeView { EyeView { found, size: SIZE, mirror: Mirror::whole_eye(LEFT), on_head: on_head(-0.0315, 0.) } }

    #[test]
    fn the_logged_session_is_read_whole() {
        let all = sightings();
        assert_eq!(all.len(), 14, "every landmarks line of 01:46:58 to 01:48:43");
        let fitted: Vec<_> = all.iter().filter(|f| f.perspective.is_some()).collect();
        assert!(fitted.len() >= 10, "{}", fitted.len());
        let first = fitted[0];
        assert_eq!(first.landmarks.unwrap().0, 5);
        assert!((first.landmarks.unwrap().1[4].1[0] - 2436.9136).abs() < 1e-3 && (first.residual_px - 1.1394122).abs() < 1e-6 && (first.across - 2.4259875).abs() < 1e-6);
    }

    /// The bug: every one of these looks was found with a perspective fit, and dev.44 made no pose of any, with no right eye in the look and no width
    /// measured earlier. A perspective fit of a panel of known size is a measurement: the tools must stand beside it.
    #[test]
    fn enzos_logged_looks_place_the_tools_beside_the_camera() {
        let head = head();
        let mut placed = 0; let mut fitted = 0;
        let mut follow = Follow::default();
        let mut shown = None;
        for found in sightings().into_iter().filter(|f| f.perspective.is_some()) {
            fitted += 1;
            let view = left_view(found);
            let eye = then(&head, &view.on_head);
            for width in [0.2f32, PANEL_M, 0.5] {
                eprintln!("  at width {width}: {:?}", perspective_fit(&found, SIZE, &view.mirror, &eye, width).map(|(_, u)| u));
            }
            let verdict = accept_sighting(&view, None, &head, None, PANEL_M, Side::Left);
            eprintln!("look {fitted}: residual {} -> {:?}", found.residual_px, verdict.accepted.map(|a| (a.side, a.width_from, a.frame.m.map(|r| r[3]))));
            let Ok(accepted) = verdict.accepted else { continue };
            assert_eq!(accepted.width_from, WidthFrom::Nominal);
            assert_eq!(verdict.stereo, Some("no right-eye sighting in this look")); // i18n: not shown (diagnostics: why a camera look places no tools)
            placed += 1;
            // Where it is: in front of the eye, within arm's reach, and the column clear of the panel as this eye saw it.
            let local = then(&undone(&eye), &accepted.frame);
            assert!((0.15..1.2).contains(&-local.m[2][3]), "{:?}", local.m);
            assert!(column_clear(&found, SIZE, &view.mirror, &eye, &accepted.frame, accepted.unit, accepted.side, accepted.clearance));
            if shown.is_none() { follow.attach(accepted.frame, accepted.unit); } else { follow.seen(accepted.frame, accepted.unit); }
            shown = follow.step(1. / 72., [None; 2], [false; 2], [None; 2]);
            assert!(shown.is_some(), "an accepted look is shown");
        }
        eprintln!("placed {placed} of {fitted} perspective looks");
        // Measured on this session: 9 of 12. Look 9 is a four-landmark fit whose homography is sheared (no rigid plane at any width); looks 7 and 8
        // reproject over the gate even at his eyes' sharpness (it is scaled from 2.5 to 4.9 px there). Both are refused, and say so.
        assert!(placed >= 9, "placed {placed} of {fitted}");
    }

    /// A panel at `distance` in front of the head, turned by `yaw`, as one eye (`render`, `raw`) would see its five landmarks at 3168 x 3456, with up to
    /// `noise` pixels of error that differs per landmark and eye.
    fn seen_by(render: &Matrix34, raw: [f32; 4], head: &Matrix34, panel: &Matrix34, unit: f32, noise: f32, seed: f32) -> Found {
        let eye = then(head, render);
        let inverse = undone(&eye);
        let mirror = Mirror::whole_eye(raw);
        let model = [PANEL_X, PANEL_CHEVRON, [-154., -84.], [-35., 133.], [82., 135.]];
        let pixel = |p: [f32; 2]| -> [f32; 2] {
            let world: [f32; 3] = std::array::from_fn(|i| panel.m[i][3] + panel.m[i][0] * p[0] * unit - panel.m[i][1] * p[1] * unit);
            let v: [f32; 3] = std::array::from_fn(|i| inverse.m[i][3] + (0..3).map(|j| inverse.m[i][j] * world[j]).sum::<f32>());
            let t = [v[0] / -v[2], -v[1] / -v[2]];
            std::array::from_fn(|k| (t[k] - mirror.from[k]) / mirror.spans[k] * SIZE[k])
        };
        let mut landmarks = [([0.; 2], [0.; 2]); 10];
        let mut points = Vec::new();
        for (i, p) in model.into_iter().enumerate() {
            let q = pixel(p);
            let q = [q[0] + noise * (seed + i as f32 * 1.7).sin(), q[1] + noise * (seed * 1.3 + i as f32 * 2.3).cos()];
            landmarks[i] = (p, q);
            points.push((p, q));
        }
        let h = fit(&points).unwrap();
        let middle = project(h, [0., 0.]);
        let r = project(h, [1., 0.]);
        let d = project(h, [0., 1.]);
        let across = (r[0] - middle[0]).hypot(r[1] - middle[1]);
        let tall = (d[0] - middle[0]).hypot(d[1] - middle[1]);
        let residual = (points.iter().map(|&(p, q)| { let r = project(h, p); (r[0] - q[0]).powi(2) + (r[1] - q[1]).powi(2) }).sum::<f32>() / 5.).sqrt();
        Found { perspective: Some(h), lines: None, residual_px: residual, landmarks: Some((5, landmarks)), x: points[0].1, chevron: points[1].1, middle,
            right: [(r[0] - middle[0]) / across, (r[1] - middle[1]) / across], down: [(d[0] - middle[0]) / tall, (d[1] - middle[1]) / tall], across, tall }
    }

    /// A panel held in front, a little right and below the eyes, facing them, turned by `yaw` degrees about its upright.
    fn held(head: &Matrix34, distance: f32, yaw: f32) -> Matrix34 {
        let (s, c) = yaw.to_radians().sin_cos();
        let local = Matrix34 { m: [[c, 0., s, 0.06], [0., 1., 0., -0.05], [-s, 0., c, -distance]] };
        then(head, &local)
    }

    /// Stereo at his resolution and projection: parallel eyes (his EyeToHead is a pure 31.5 mm translation), and canted ones, as long as SteamVR
    /// reports the eyes as they are rendered. Before, nothing covered 3168 x 3456 or a canted pair.
    #[test]
    fn stereo_measures_the_width_at_enzos_resolution_parallel_or_canted() {
        let head = head();
        let unit = PANEL_M / PANEL_SIZE[0];
        let (mut stereo, mut cases) = (0, 0);
        for cant in [0f32, 5., 10.] {
            for distance in [0.3f32, 0.45, 0.7, 1.0] {
                for yaw in [-35f32, 0., 35.] {
                    for noise in [0f32, 1.2] {
                        let eyes = [on_head(-0.0315, cant), on_head(0.0315, -cant)];
                        let panel = held(&head, distance, yaw);
                        let left = seen_by(&eyes[0], LEFT, &head, &panel, unit, noise, 0.3);
                        let right = seen_by(&eyes[1], RIGHT, &head, &panel, unit, noise, 1.9);
                        let views = [EyeView { found: left, size: SIZE, mirror: Mirror::whole_eye(LEFT), on_head: eyes[0] }, EyeView { found: right, size: SIZE, mirror: Mirror::whole_eye(RIGHT), on_head: eyes[1] }];
                        let verdict = accept_sighting(&views[0], Some(&views[1]), &head, None, PANEL_M, Side::Left);
                        let case = format!("cant {cant} distance {distance} yaw {yaw} noise {noise}");
                        let accepted = verdict.accepted.unwrap_or_else(|why| panic!("{case}: {why}"));
                        // Every look places the tools, from one eye if stereo cannot measure it; and where they are is right either way.
                        assert!(apart(&accepted.frame, &panel) < 0.05, "{case}: {:?}", accepted.frame.m);
                        if accepted.width_from == WidthFrom::Stereo {
                            let width = accepted.unit * PANEL_SIZE[0];
                            assert!((width - PANEL_M).abs() < 0.03, "{case}: width {width}");
                            stereo += 1;
                        } else {
                            assert!(noise > 0., "{case}: noise-free stereo must measure: {:?}", verdict.stereo);
                            eprintln!("{case}: one eye, stereo not used because {:?}", verdict.stereo);
                        }
                        cases += 1;
                    }
                }
            }
        }
        eprintln!("stereo measured {stereo} of {cases} looks at 3168 x 3456");
    }

    /// When the eyes are not where SteamVR says (a canted render reported as parallel, or another IPD), or the right eye saw something else, stereo
    /// fails: the left eye's pose places the tools anyway, and the verdict says why stereo was not used.
    #[test]
    fn a_stereo_pair_that_disagrees_still_places_the_tools_from_one_eye() {
        let head = head();
        let unit = PANEL_M / PANEL_SIZE[0];
        let reported = [on_head(-0.0315, 0.), on_head(0.0315, 0.)];
        let panel = held(&head, 0.45, 15.);
        let left = seen_by(&reported[0], LEFT, &head, &panel, unit, 1.2, 0.3);
        let views = |right: Found| [EyeView { found: left, size: SIZE, mirror: Mirror::whole_eye(LEFT), on_head: reported[0] }, EyeView { found: right, size: SIZE, mirror: Mirror::whole_eye(RIGHT), on_head: reported[1] }];
        // Rendered canted by 8 degrees, reported parallel; and a right eye whose sighting is 60 px off vertically.
        let canted = seen_by(&on_head(0.0315, -8.), RIGHT, &head, &panel, unit, 1.2, 1.9);
        let mut lifted = seen_by(&reported[1], RIGHT, &head, &panel, unit, 1.2, 1.9);
        let mut h = lifted.perspective.unwrap();
        for j in 0..3 { h[1][j] += 60. * h[2][j]; }
        lifted.perspective = Some(h);
        if let Some((count, mut points)) = lifted.landmarks { for p in points.iter_mut().take(count) { p.1[1] += 60.; } lifted.landmarks = Some((count, points)); }
        for (name, right) in [("canted render reported parallel", canted), ("right sighting 60 px off", lifted)] {
            let [l, r] = views(right);
            let verdict = accept_sighting(&l, Some(&r), &head, None, PANEL_M, Side::Left);
            let accepted = verdict.accepted.unwrap_or_else(|why| panic!("{name}: {why}"));
            // Whatever stereo made of it, the tools stand beside the panel as the left eye sees it. A canted render reported parallel can pass the
            // stereo gates with a wrong width (measured here; Enzo's Quest Pro reports parallel eyes, as his vrserver.txt shows): only the depth
            // is then off, not the place in the eye that measured it.
            let eye = then(&head, &l.on_head);
            assert!(column_clear(&l.found, SIZE, &l.mirror, &eye, &accepted.frame, accepted.unit, accepted.side, accepted.clearance), "{name}");
            eprintln!("{name}: {:?}, width {:.3}, stereo not used because {:?}", accepted.width_from, accepted.unit * PANEL_SIZE[0], verdict.stereo);
            if accepted.width_from != WidthFrom::Stereo {
                assert!(verdict.stereo.is_some(), "{name}: the reason stereo was not used is said");
                assert!(apart(&accepted.frame, &panel) < 0.05, "{name}: one eye at the usual width puts it where it is: {:?}", accepted.frame.m);
            }
        }
        // A width measured earlier by both eyes is used, and said to be.
        let [l, _] = views(canted);
        assert_eq!(accept_sighting(&l, None, &head, Some(0.28), PANEL_M, Side::Left).accepted.unwrap().width_from, WidthFrom::Measured);
    }

    /// No sighting, no tools: a look with only glyphs (no perspective fit) is never placed, and says so.
    #[test]
    fn glyphs_alone_never_place_the_tools() {
        let mut found = sightings().into_iter().find(|f| f.perspective.is_none()).expect("a look without a perspective fit in the session");
        found.landmarks = None;
        let verdict = accept_sighting(&left_view(found), None, &head(), None, PANEL_M, Side::Left);
        assert_eq!(verdict.accepted.err(), Some("no perspective fit (glyphs only)")); // i18n: not shown (diagnostics: why a camera look places no tools)
    }

    /// The sandbox at Enzo's 3168 x 3456 (2026-11-13, camera3/m1): the camera just opened, 0.4 m away, low and tilted, seen by both eyes as these
    /// outline corners. Refused by the fixed pixel gate for three minutes (rigid fits of 6.5 and 5.4 px); placed now, from both eyes.
    #[test]
    fn a_camera_just_opened_close_and_tilted_is_placed() {
        let corners = |q: [[f32; 2]; 4]| {
            let model = [[-205., -115.3125], [205., -115.3125], [205., 115.3125], [-205., 115.3125]];
            let points: Vec<_> = model.into_iter().zip(q).collect();
            let h = fit(&points).unwrap();
            let mut landmarks = [([0.; 2], [0.; 2]); 10];
            for (slot, p) in landmarks.iter_mut().zip(&points) { *slot = *p; }
            let middle = project(h, [0., 0.]); let r = project(h, [1., 0.]); let d = project(h, [0., 1.]);
            let (across, tall) = ((r[0] - middle[0]).hypot(r[1] - middle[1]), (d[0] - middle[0]).hypot(d[1] - middle[1]));
            Found { perspective: Some(h), lines: None, residual_px: 1.04, landmarks: Some((4, landmarks)), x: project(h, PANEL_X), chevron: project(h, PANEL_CHEVRON), middle,
                right: [(r[0] - middle[0]) / across, (r[1] - middle[1]) / across], down: [(d[0] - middle[0]) / tall, (d[1] - middle[1]) / tall], across, tall }
        };
        let left = corners([[1229.1563, 2184.5796], [2082.5117, 1879.5481], [2255.023, 2424.6096], [1273.1213, 2916.1262]]);
        let right = corners([[248.91309, 2185.6714], [1140.4647, 1879.0818], [1285.1508, 2423.2778], [257.17578, 2915.0972]]);
        let views = [EyeView { found: left, size: SIZE, mirror: Mirror::whole_eye(LEFT), on_head: on_head(-0.032, 0.) }, EyeView { found: right, size: SIZE, mirror: Mirror::whole_eye(RIGHT), on_head: on_head(0.032, 0.) }];
        assert!(perspective_fit(&left, SIZE, &views[0].mirror, &then(&IDENTITY, &views[0].on_head), PANEL_M).is_ok(), "one eye alone");
        let verdict = accept_sighting(&views[0], Some(&views[1]), &IDENTITY, None, PANEL_M, Side::Left);
        let accepted = verdict.accepted.unwrap_or_else(|why| panic!("{why}"));
        eprintln!("close tilted camera: {:?}, width {:.3}, depth {:.3}, stereo not used because {:?}", accepted.width_from, accepted.unit * PANEL_SIZE[0], -accepted.frame.m[2][3], verdict.stereo);
        assert!((0.3..0.55).contains(&-accepted.frame.m[2][3]));
    }
}
