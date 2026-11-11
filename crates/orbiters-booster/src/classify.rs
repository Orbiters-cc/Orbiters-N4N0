//! What is limiting this PC, from the last ten seconds of frames. The thresholds are starting values for a test to correct,
//! none measured. Whether the reason flags fire at all over Virtual Desktop is not known, so the timing rules are there for
//! that case. Wait-inclusive processor time never establishes a processor limit. Processor and graphics card time overlap, so they are never added, and `1000 / GPU ms` is never called a frame
//! rate.

use crate::frames::{summarize, Frame, Summary};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Rolling over this long.
pub const WINDOW_S: f64 = 10.0;
/// Looked at this often, so two looks in a row are two windows, half overlapping.
pub const EVERY_S: f64 = 5.0;
/// Fewer frames than this say nothing.
const FEWEST_FRAMES: u32 = 200;

/// What holds this PC's frames back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    /// Late or repeated under 5%.
    Fine,
    Processor,
    GraphicsCard,
    Both,
    #[default]
    NotSure,
}

/// The result for one window.
pub fn classify(s: &Summary, refresh_hz: f32) -> Limit {
    if s.frames < FEWEST_FRAMES {
        return Limit::NotSure;
    }
    let budget = crate::frames::budget_ms(refresh_hz);
    let (cpu, gpu) = (s.late_cpu_share, s.late_gpu_share);
    let gpu_ms = s.app_gpu.median;
    if cpu.max(gpu) < 0.05 && s.filled_share < 0.05 {
        return Limit::Fine;
    }
    if cpu >= 0.05 && cpu >= 2.0 * gpu {
        return Limit::Processor;
    }
    if (gpu >= 0.05 && gpu >= 2.0 * cpu) || (gpu_ms >= 0.85 * budget && cpu < 0.05) {
        return Limit::GraphicsCard;
    }
    if cpu >= 0.05 && gpu >= 0.05 {
        return Limit::Both;
    }
    Limit::NotSure
}

/// Keeps the frames of the window, and says what limits this PC once two windows in a row agree.
#[derive(Default)]
pub struct Classifier {
    window: VecDeque<(f64, Frame)>,
    last_look: Option<f64>,
    last_result: Option<Limit>,
    confirmed: Limit,
}

impl Classifier {
    pub fn push(&mut self, now: f64, frame: Frame) {
        self.window.push_back((now, frame));
        while self.window.front().is_some_and(|(at, _)| now - at > WINDOW_S) {
            self.window.pop_front();
        }
    }

    /// Frames that must not count (loading, the dashboard open, just joined): what was gathered so far goes too, as a window
    /// is about one stretch of the same thing.
    pub fn ignore(&mut self) {
        self.window.clear();
        self.last_result = None;
        self.last_look = None;
    }

    /// Back to not knowing (another world).
    pub fn forget(&mut self) {
        self.ignore();
        self.confirmed = Limit::NotSure;
    }

    #[cfg(test)]
    pub(crate) fn has_decision_window(&self) -> bool { self.last_look.is_some() }

    pub fn confirmed(&self) -> Limit {
        self.confirmed
    }

    /// Looks, if it is time. What is confirmed changes only when two looks in a row agree.
    pub fn look(&mut self, now: f64, refresh_hz: f32) {
        if self.last_look.is_some_and(|at| now - at < EVERY_S) {
            return;
        }
        let Some((oldest, _)) = self.window.front() else { return };
        if now - oldest < WINDOW_S - 0.5 {
            return;
        }
        self.last_look = Some(now);
        let frames: Vec<Frame> = self.window.iter().map(|(_, f)| *f).collect();
        let result = summarize(&frames, refresh_hz).map_or(Limit::NotSure, |s| classify(&s, refresh_hz));
        if self.last_result == Some(result) {
            self.confirmed = result;
        }
        self.last_result = Some(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{steady, LATE_CPU, LATE_GPU};

    const HZ: f32 = 72.0;
    const B: f32 = 1000.0 / 72.0;

    fn summary(late_cpu: f32, late_gpu: f32, filled: f32, cpu_ms: f32, gpu_ms: f32) -> Summary {
        let mut s = Summary {
            frames: 400,
            seconds: 10.0,
            real_fps: 40.0,
            late_cpu_share: late_cpu,
            late_gpu_share: late_gpu,
            filled_share: filled,
            ..Summary::default()
        };
        s.app_cpu.median = cpu_ms;
        s.app_gpu.median = gpu_ms;
        s
    }

    /// Every row of the design's table.
    #[test]
    fn the_rules() {
        assert_eq!(classify(&summary(0.02, 0.02, 0.02, 10.0, 8.0), HZ), Limit::Fine);
        // By flags.
        assert_eq!(classify(&summary(0.40, 0.05, 0.4, 10.0, 8.0), HZ), Limit::Processor);
        assert_eq!(classify(&summary(0.05, 0.40, 0.4, 10.0, 8.0), HZ), Limit::GraphicsCard);
        assert_eq!(classify(&summary(0.30, 0.20, 0.4, 10.0, 8.0), HZ), Limit::Both);
        // By timing, with flags that never fire, as may be the case over a streamer.
        assert_eq!(classify(&summary(0.0, 0.0, 0.48, B, 9.0), HZ), Limit::NotSure, "wait-inclusive timing cannot identify a processor limit");
        assert_eq!(classify(&summary(0.0, 0.0, 0.48, 6.0, B), HZ), Limit::GraphicsCard);
        assert_eq!(classify(&summary(0.0, 0.0, 0.48, B, B), HZ), Limit::GraphicsCard);
        // Repeating, with neither flag nor time saying why.
        assert_eq!(classify(&summary(0.0, 0.0, 0.48, 6.0, 6.0), HZ), Limit::NotSure);
    }

    #[test]
    fn too_few_frames_are_not_sure() {
        assert_eq!(classify(&Summary { frames: 199, late_cpu_share: 1.0, ..Summary::default() }, HZ), Limit::NotSure);
    }

    fn feed(c: &mut Classifier, from: f64, seconds: f64, flags: u32) {
        for f in steady(from, seconds, 72.0, Frame { presents: 2, flags, ..Frame::default() }) {
            c.push(f.time_s, f);
            c.look(f.time_s, HZ);
        }
    }

    /// One window alone changes nothing: a moment's load must not spend anything.
    #[test]
    fn two_windows_in_a_row_must_agree() {
        let mut c = Classifier::default();
        feed(&mut c, 0.0, 9.0, LATE_CPU);
        assert_eq!(c.confirmed(), Limit::NotSure, "not a whole window yet");
        feed(&mut c, 9.0, 1.5, LATE_CPU);
        assert_eq!(c.confirmed(), Limit::NotSure, "one window");
        feed(&mut c, 10.5, 5.0, LATE_CPU);
        assert_eq!(c.confirmed(), Limit::Processor);
        // A blip in the other direction, too short to fill a window's majority twice, does not flip it.
        feed(&mut c, 15.5, 6.0, LATE_GPU);
        assert_eq!(c.confirmed(), Limit::Processor);
        feed(&mut c, 21.5, 10.0, LATE_GPU);
        assert_eq!(c.confirmed(), Limit::GraphicsCard);
    }

    #[test]
    fn ignored_frames_start_a_window_over() {
        let mut c = Classifier::default();
        feed(&mut c, 0.0, 12.0, LATE_CPU);
        c.ignore();
        assert_eq!(c.confirmed(), Limit::NotSure, "still not confirmed");
        feed(&mut c, 12.0, 5.0, LATE_CPU);
        assert_eq!(c.confirmed(), Limit::NotSure, "the frames before do not help");
        feed(&mut c, 17.0, 11.0, LATE_CPU);
        assert_eq!(c.confirmed(), Limit::Processor);
        c.forget();
        assert_eq!(c.confirmed(), Limit::NotSure);
    }
}
