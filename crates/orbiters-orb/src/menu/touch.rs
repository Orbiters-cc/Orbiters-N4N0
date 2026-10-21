//! Where the owner's finger touches the avatar's screen. The screen carries four contact receivers per hand (local, so
//! they cost nothing in VRChat's performance rank), each telling how near that hand's index finger is (VRChat's
//! proximity: 1 at the receiver's centre, 0 at its radius). Three sit on the screen's corners and one behind its middle;
//! from the four distances the desktop finds the finger (and the finger contact's own size, which VRChat sets from
//! the avatar's hand), then which button it is on and whether it presses.
//!
//! Positions are in the screen's own space as the prop is built (metres at 1.6 m eye height; the avatar's scale
//! changes nothing, as receivers and fingers scale together): x to the right, y up, z away from the person (the screen's
//! face looks at −z).

/// The screen's size.
pub const WIDTH: f64 = 0.48;
pub const HEIGHT: f64 = 0.27;
/// The receivers' radius.
pub const RADIUS: f64 = 0.8;
/// How far outside the screen's edges the receivers sit: a finger on the screen never reaches one's centre, where its
/// proximity would stop at 1.
pub const MARGIN: f64 = 0.05;
/// The receivers: past the top left and top right corners, under the bottom's middle, behind the middle.
pub const RECEIVERS: [[f64; 3]; 4] = [
    [-WIDTH / 2.0 - MARGIN, HEIGHT / 2.0 + MARGIN, 0.0],
    [WIDTH / 2.0 + MARGIN, HEIGHT / 2.0 + MARGIN, 0.0],
    [0.0, -HEIGHT / 2.0 - MARGIN, 0.0],
    [0.0, 0.0, 0.15],
];

/// A finger in front of (negative) or past (positive) the screen's face closer than this presses.
pub const PRESS_DEPTH: f64 = -0.012;
/// It lets go once this far in front again.
pub const RELEASE_DEPTH: f64 = -0.025;
/// Nearer than this it hovers.
pub const HOVER_DEPTH: f64 = -0.08;

/// Where a finger is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finger {
    /// On the page, in pixels (0 to `layout::WIDTH`, 0 to `layout::HEIGHT`, top left first).
    pub x: f32,
    pub y: f32,
    /// Distance from the screen's face (negative in front).
    pub depth: f64,
}

/// The finger from the four proximities (0 to 1), or `None` when one is out of reach or they do not agree.
pub fn locate(proximity: [f32; 4]) -> Option<Finger> {
    if proximity.iter().any(|&p| !(p > 0.0 && p <= 1.0)) {
        return None;
    }
    // Distance from each receiver's centre to the finger contact's surface.
    let d: Vec<f64> = proximity.iter().map(|&p| RADIUS * (1.0 - p as f64)).collect();
    // Unknowns: the finger (x, y, z) and its contact's radius r: |f − c_i| = d_i + r. Gauss–Newton from in front of the
    // screen's middle.
    let mut u = [0.0, 0.0, -0.05, 0.01];
    for _ in 0..30 {
        let mut jtj = [[0.0f64; 4]; 4];
        let mut jtr = [0.0f64; 4];
        for (c, &di) in RECEIVERS.iter().zip(&d) {
            let delta = [u[0] - c[0], u[1] - c[1], u[2] - c[2]];
            let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt().max(1e-9);
            let residual = length - di - u[3];
            let row = [delta[0] / length, delta[1] / length, delta[2] / length, -1.0];
            for a in 0..4 {
                jtr[a] += row[a] * residual;
                for b in 0..4 {
                    jtj[a][b] += row[a] * row[b];
                }
            }
        }
        let step = solve(jtj, jtr)?;
        for a in 0..4 {
            u[a] -= step[a];
        }
        if step.iter().map(|s| s.abs()).fold(0.0, f64::max) < 1e-7 {
            break;
        }
    }
    // The four distances must agree (two hands on one set of receivers do not).
    let worst = RECEIVERS
        .iter()
        .zip(&d)
        .map(|(c, &di)| (((u[0] - c[0]).powi(2) + (u[1] - c[1]).powi(2) + (u[2] - c[2]).powi(2)).sqrt() - di - u[3]).abs())
        .fold(0.0, f64::max);
    if !worst.is_finite() || worst > 0.01 || !(-0.01..0.06).contains(&u[3]) {
        return None;
    }
    let x = ((u[0] + WIDTH / 2.0) / WIDTH * super::layout::WIDTH as f64) as f32;
    let y = ((HEIGHT / 2.0 - u[1]) / HEIGHT * super::layout::HEIGHT as f64) as f32;
    Some(Finger { x, y, depth: u[2] })
}

/// `a × x = b` for a 4 × 4 system (Gaussian elimination with pivoting).
fn solve(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..4 {
            let f = a[row][col] / a[col][col];
            for k in col..4 {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let sum: f64 = (row + 1..4).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    Some(x)
}

/// A hand's touch: hovering, pressing, letting go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Touch {
    pressing: bool,
    pub finger: Option<Finger>,
}

/// What a hand did this update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// Near the screen at (x, y): show what it would press.
    Hover { x: f32, y: f32 },
    /// Touched at (x, y).
    Press { x: f32, y: f32 },
    /// Still touching.
    Hold { x: f32, y: f32 },
    /// Left the screen or went away.
    Away,
}

impl Touch {
    /// The hand's new proximities.
    pub fn update(&mut self, proximity: [f32; 4]) -> Gesture {
        let finger = locate(proximity);
        self.finger = finger;
        let Some(f) = finger else {
            self.pressing = false;
            return Gesture::Away;
        };
        let inside = f.x >= 0.0 && f.y >= 0.0 && f.x < super::layout::WIDTH as f32 && f.y < super::layout::HEIGHT as f32;
        if !inside || f.depth < HOVER_DEPTH || f.depth > 0.08 {
            self.pressing = false;
            return Gesture::Away;
        }
        if self.pressing {
            if f.depth < RELEASE_DEPTH {
                self.pressing = false;
                return Gesture::Hover { x: f.x, y: f.y };
            }
            return Gesture::Hold { x: f.x, y: f.y };
        }
        if f.depth > PRESS_DEPTH {
            self.pressing = true;
            return Gesture::Press { x: f.x, y: f.y };
        }
        Gesture::Hover { x: f.x, y: f.y }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the receivers read for a finger at `f` whose contact has radius `r`.
    fn read(f: [f64; 3], r: f64) -> [f32; 4] {
        let mut out = [0f32; 4];
        for (o, c) in out.iter_mut().zip(RECEIVERS) {
            let d = ((f[0] - c[0]).powi(2) + (f[1] - c[1]).powi(2) + (f[2] - c[2]).powi(2)).sqrt() - r;
            *o = (1.0 - (d / RADIUS)).clamp(0.0, 1.0) as f32;
        }
        out
    }

    #[test]
    fn finds_the_finger_anywhere_on_the_screen() {
        for &r in &[0.0, 0.008, 0.02] {
            for &x in &[-0.23, -0.1, 0.0, 0.17, 0.235] {
                for &y in &[-0.13, -0.05, 0.0, 0.12] {
                    for &z in &[-0.07, -0.02, -0.005, 0.0, 0.01] {
                        let found = locate(read([x, y, z], r)).unwrap_or_else(|| panic!("{x} {y} {z} {r}"));
                        let px = ((x + WIDTH / 2.0) / WIDTH * 1024.0) as f32;
                        let py = ((HEIGHT / 2.0 - y) / HEIGHT * 576.0) as f32;
                        // Within 2 px on the page and 1 mm deep (float proximities).
                        assert!((found.x - px).abs() < 2.0 && (found.y - py).abs() < 2.0, "{x} {y} {z} {r}: {found:?} vs {px} {py}");
                        assert!((found.depth - z).abs() < 0.001, "{x} {y} {z} {r}: depth {}", found.depth);
                    }
                }
            }
        }
        assert_eq!(locate([0.0, 0.5, 0.5, 0.5]), None, "a receiver that does not see the finger");
        // Two fingers, one near the left corner and one near the right: the readings do not agree.
        let left = read([-0.2, 0.1, -0.01], 0.01);
        let right = read([0.2, -0.1, -0.01], 0.01);
        assert_eq!(locate([left[0], right[1], left[2], right[3]]), None);
    }

    #[test]
    fn presses_once_and_lets_go() {
        let mut touch = Touch::default();
        let at = |z: f64| read([0.0, 0.0, z], 0.01);
        assert!(matches!(touch.update(at(-0.2)), Gesture::Away), "too far");
        assert!(matches!(touch.update(at(-0.05)), Gesture::Hover { .. }));
        assert!(matches!(touch.update(at(-0.005)), Gesture::Press { .. }));
        assert!(matches!(touch.update(at(0.01)), Gesture::Hold { .. }), "pushing through holds");
        assert!(matches!(touch.update(at(-0.018)), Gesture::Hold { .. }), "between the depths it still holds");
        assert!(matches!(touch.update(at(-0.03)), Gesture::Hover { .. }));
        assert!(matches!(touch.update(at(-0.005)), Gesture::Press { .. }), "and presses again");
    }
}
