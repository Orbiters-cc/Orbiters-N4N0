//! How far N4N0's bar has grown into its chat (REQUEST21 3): one spring from the bar (0) to the open chat (1), on the
//! window's clock. A new target takes over from where the shape is, at the speed it has, so an opening turned back
//! half-way (Esc, a press outside) shrinks back from there in one motion. With reduced motion there is no morph: the
//! chat fades in and out over the bar ([`FADE`]).
//!
//! It moves only while it is away from its target: at rest it asks for no frame ([`Morph::step`] says when it moves).

/// Seconds of the fade that replaces the morph under reduced motion.
pub const FADE: f32 = 0.12;

/// The spring: about a quarter of a second to open, a couple of percent of overshoot (the website's `settleSpring`
/// feel), critically enough damped that a reversal never wobbles.
const STIFFNESS: f32 = 260.0;
const DAMPING: f32 = 26.0;

/// Close enough to the target (and slow enough) to stop.
const EPS: f32 = 0.001;

#[derive(Clone, Debug, Default)]
pub struct Morph {
    open: bool,
    /// The spring's position and speed (0 the bar, 1 the chat).
    t: f32,
    v: f32,
    /// Under reduced motion, the chat's opacity over the bar.
    fade: f32,
    reduced: bool,
    /// The clock at the last step.
    last: Option<f64>,
}

impl Morph {
    /// Towards the chat (`true`) or back into the bar, from wherever it is now.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Advances to `now` (seconds); true while it still moves (the caller asks for the next frame then).
    pub fn step(&mut self, now: f64, reduced: bool) -> bool {
        let dt = self.last.map_or(0.0, |last| (now - last).clamp(0.0, 0.05) as f32);
        self.last = Some(now);
        let target = if self.open { 1.0 } else { 0.0 };
        if reduced != self.reduced {
            // Reduced motion changed under it: it lands where it was going.
            self.reduced = reduced;
            (self.t, self.v, self.fade) = (target, 0.0, target);
        }
        if reduced {
            let k = dt / FADE;
            self.fade = if self.open { (self.fade + k).min(1.0) } else { (self.fade - k).max(0.0) };
            // The shape is the chat's while it shows at all, the bar's once it has faded.
            self.t = if self.fade > 0.0 { 1.0 } else { 0.0 };
            self.v = 0.0;
            return self.fade != target;
        }
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(1.0 / 240.0);
            left -= h;
            let a = -STIFFNESS * (self.t - target) - DAMPING * self.v;
            self.v += a * h;
            self.t += self.v * h;
        }
        // Never under the bar: closing lands on it (the bar is where the shape comes to rest, not a spring to pass).
        if self.t < 0.0 {
            (self.t, self.v) = (0.0, 0.0);
        }
        if (self.t - target).abs() < EPS && self.v.abs() < EPS * 10.0 {
            (self.t, self.v) = (target, 0.0);
        }
        self.fade = if self.t > 0.0 { 1.0 } else { 0.0 };
        !self.at_rest()
    }

    /// The shape's progress: 0 the bar, 1 the chat (a little over 1 as it settles open).
    pub fn shape(&self) -> f32 {
        self.t
    }

    /// How much of the chat shows (1 but under reduced motion, where it fades).
    pub fn alpha(&self) -> f32 {
        if self.reduced {
            self.fade
        } else {
            1.0
        }
    }

    /// How open it is for what grows with it (the blur and dim behind): 0 to 1.
    pub fn openness(&self) -> f32 {
        if self.reduced {
            self.fade
        } else {
            self.t.clamp(0.0, 1.0)
        }
    }

    /// Resting where it was told to go: nothing moves, no frame is needed.
    pub fn at_rest(&self) -> bool {
        let target = if self.open { 1.0 } else { 0.0 };
        if self.reduced {
            self.fade == target
        } else {
            self.t == target && self.v == 0.0
        }
    }

    /// Shown as the bar alone (closed and landed).
    pub fn closed(&self) -> bool {
        !self.open && self.at_rest()
    }

    /// Under reduced motion (the chat fades over the bar instead of growing out of it).
    pub fn reduced(&self) -> bool {
        self.reduced
    }
}
