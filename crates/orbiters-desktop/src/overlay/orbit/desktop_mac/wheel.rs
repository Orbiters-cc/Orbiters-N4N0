//! VRChat's wheel on a Mac's picture of the game, where it is not to the pixel where [`zones::wheel`] has it, and the numbers of a look that found nothing.
//!
//! [`detect::wheel_seen`] holds only within about 3% of the radius: with the wheel 5% smaller, 0% of its ring is "of one colour" (the hub's side of the rim is the world), and
//! with it 5% larger its rim's line is 4% of what it was (measured on the real frames, 2026-11-25). Whatever moves VRChat's wheel that little (its view's point, an avatar's
//! height) hid the ring for good, and the owner's first real session on a Mac had every R looked at for its whole 1.5 s and none found. So after R it is also looked for
//! **around** its place: up to [`NEAR`] radii off and of a size within [`SIZES`], on the same three measures held to more than in place ([`FIT`]): on every frame here without
//! a wheel, and slid over every other whole one, nothing comes to them.

use super::super::detect::{self, Part};
use super::super::zones;
use egui::{pos2, vec2, Pos2, Vec2};

/// The share of the rim that stands out, of one colour inside it, and of the rim's line ([`detect::wheel_scores`]), that a wheel found beside its place must have.
pub const FIT: Scores = Scores { edge: 0.9, uniform: 0.6, line: 0.6 };
/// What one in place must have: [`detect`]'s own.
pub const IN_PLACE: Scores = Scores { edge: detect::WHEEL_EDGE, uniform: detect::WHEEL_UNIFORM, line: detect::WHEEL_LINE };
/// How far from its place, in its radii, and between what sizes of it.
const NEAR: f32 = 0.1;
const SIZES: std::ops::RangeInclusive<f32> = 0.9..=1.1;
const STEP: f32 = 0.02;
/// The wide search's: coarse places first, the best of them looked at closely.
const WIDE_SIZES: [f32; 10] = [0.45, 0.6, 0.75, 0.88, 1.0, 1.12, 1.25, 1.45, 1.7, 2.0];

/// How a wheel's place looks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scores {
    pub edge: f32,
    pub uniform: f32,
    pub line: f32,
}

impl Scores {
    fn at(part: &Part, center: Pos2, radius: f32) -> Self {
        let (edge, uniform, line) = detect::wheel_scores_at(part, center, radius);
        Self { edge, uniform, line }
    }

    /// How far it is from what is asked: 1 and over, it has all of it.
    pub fn of(&self, asked: &Scores) -> f32 {
        (self.edge / asked.edge).min(self.uniform / asked.uniform).min(self.line / asked.line)
    }

    pub fn has(&self, asked: &Scores) -> bool {
        self.of(asked) >= 1.0
    }

    fn sum(&self) -> f32 {
        self.edge + self.uniform + self.line
    }
}

/// Where a wheel is, against its place ([`zones::wheel`]): off it, in the window's heights, and how large against its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub off: Vec2,
    pub size: f32,
    pub scores: Scores,
}

impl Fit {
    /// Centre and radius in a window of `client`.
    pub fn circle(&self, client: Vec2) -> (Pos2, f32) {
        let (center, radius) = zones::wheel(client);
        (center + self.off * client.y, radius * self.size)
    }

    /// The same wheel, within what one look and the next differ by.
    pub fn same(&self, other: &Fit) -> bool {
        (self.off - other.off).length() <= 0.139 * 0.045 && (self.size - other.size).abs() <= 0.045
    }

    /// Where it moves what lies around the wheel, in pixels of a window of `client`.
    pub fn moved(&self, client: Vec2) -> Vec2 {
        (self.off * client.y).round()
    }
}

impl std::fmt::Display for Fit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Scores { edge, uniform, line } = self.scores;
        write!(f, "{:+.4} {:+.4} of the height off its place, {:.3} of its size: edge {edge:.2}, one colour {uniform:.2}, line {line:.2}", self.off.x, self.off.y, self.size) // i18n: not shown (a log's line)
    }
}

/// The wheel's scores in its place.
pub fn in_place(part: &Part, client: Vec2) -> Scores {
    let (center, radius) = zones::wheel(client);
    Scores::at(part, center, radius)
}

/// A wheel known to be at `fit`: still there?
pub fn still(part: &Part, client: Vec2, fit: &Fit) -> bool {
    let (center, radius) = fit.circle(client);
    // (as steps of the search are, it may be one off: the hand of a highlighted slice takes its share, as in place)
    [0.0, -0.01, 0.01].iter().any(|d| Scores::at(part, center, radius * (1.0 + d)).has(&IN_PLACE))
}

/// The rim at a few angles: what a place that can be a wheel's has nearly all of. Cheap: most places end here.
fn rim(part: &Part, center: Pos2, radius: f32, angles: usize) -> f32 {
    let stands = |i: usize| {
        let (s, c) = (i as f32 / angles as f32 * std::f32::consts::TAU).sin_cos();
        let at = |r: f32| part.nearest(center.x + radius * r * c, center.y + radius * r * s);
        let (a, b) = (at(0.955), at(1.07));
        (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs() > 15.0
    };
    (0..angles).filter(|i| stands(*i)).count() as f32 / angles as f32
}

/// The best wheel around its place, whatever its scores are.
pub fn near(part: &Part, client: Vec2) -> Option<Fit> {
    let (center, radius) = zones::wheel(client);
    let n = (NEAR / STEP).round() as i32;
    let sizes = ((SIZES.end() - SIZES.start()) / STEP).round() as i32;
    let mut best: Option<Fit> = None;
    for (dx, dy) in (-n..=n).flat_map(|y| (-n..=n).map(move |x| (x, y))).filter(|(x, y)| x * x + y * y <= n * n) {
        for k in 0..=sizes {
            let (off, size) = (vec2(dx as f32, dy as f32) * STEP * radius, SIZES.start() + k as f32 * STEP);
            if rim(part, center + off, radius * size, 24) < 0.85 {
                continue;
            }
            let scores = Scores::at(part, center + off, radius * size);
            let better = best.is_none_or(|b| (scores.of(&FIT).min(1.0), scores.sum()) > (b.scores.of(&FIT).min(1.0), b.scores.sum()));
            if better {
                best = Some(Fit { off: off / client.y, size, scores });
            }
        }
    }
    best
}

/// The disc that looks most like a wheel anywhere in the picture, of any size it may be: for the log alone, nothing is opened on it.
pub fn anywhere(part: &Part, client: Vec2) -> Option<Fit> {
    let (center, radius) = zones::wheel(client);
    let mut found: Vec<(f32, Pos2, f32)> = Vec::new();
    for size in WIDE_SIZES {
        let r = radius * size;
        let step = (r * 0.09).max(4.0);
        let mut y = r * 0.8;
        while y < client.y - r * 0.8 {
            let mut x = r * 0.8;
            while x < client.x - r * 0.8 {
                if rim(part, pos2(x, y), r, 12) >= 0.8 {
                    let scores = Scores::at(part, pos2(x, y), r);
                    found.push((scores.sum(), pos2(x, y), r));
                }
                x += step;
            }
            y += step;
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.truncate(12);
    let mut best: Option<Fit> = None;
    for (_, at, r) in found {
        for (dx, dy, k) in (-3..=3).flat_map(|x| (-3..=3).flat_map(move |y| (-4..=4).map(move |k| (x, y, k)))) {
            let (c, r) = (at + vec2(dx as f32, dy as f32) * r * 0.03, r * (1.0 + k as f32 * 0.03));
            let scores = Scores::at(part, c, r);
            if best.is_none_or(|b| scores.sum() > b.scores.sum()) {
                best = Some(Fit { off: (c - center) / client.y, size: r / radius, scores });
            }
        }
    }
    best
}
