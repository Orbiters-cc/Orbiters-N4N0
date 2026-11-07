//! The live comparison that keeps a change or undoes it: without it, nothing can be called a gain. On the same scene, in
//! blocks A (without), B (with), A, B of 15 seconds each. The first seconds of each are left out of the averages, as the
//! switch itself disturbs them, but a stall in that time is still counted and shown.
//!
//! The rules' numbers are starting values for a test to correct. What is never done: comparing different worlds or
//! different sessions, estimating a gain, or counting a frame SteamVR filled in.

use crate::frames::{summarize, Frame, Summary};
use serde::{Deserialize, Serialize};

pub const BLOCK_S: f64 = 15.0;
pub const BLOCKS: usize = 4;
/// Left out after a switch.
pub const SETTLE_S: f64 = 3.0;
/// After a change of resolution, which takes longer to settle.
pub const SETTLE_RESOLUTION_S: f64 = 5.0;
const MIN_GAIN: f32 = 0.05;
const MIN_STALL_DROP: f32 = 0.25;
const A_BLOCKS_AGREE: f32 = 0.05;
/// How much worse the late frames for the other reason may get and still be the same: frames are noisy.
const OTHER_LATE_SLACK: f32 = 0.02;
/// Fewer frames over twice the budget than this is not enough to judge a change by them.
const STALLS_WORTH_JUDGING: f32 = 0.01;
/// Graphics card time that fell by less than this: the lower sharpness did not reach VRChat.
const GPU_MUST_FALL: f32 = 0.03;
/// A block with fewer settled frames says nothing.
const FEWEST_BLOCK_FRAMES: u32 = 30;

/// What is being compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Other programs lowered.
    Programs,
    /// SteamVR's sharpness × 0.85.
    Sharpness,
    /// 3D panels at their lightest.
    PanelQuality,
    /// 3D pictures waiting.
    ReconstructionHold,
}

/// What a comparison concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// More real frames, both times.
    Kept,
    /// Put back.
    NoGain,
    /// The two blocks without the change disagree, so nothing can be said of the change.
    TooMuchChanging,
    /// The graphics card's time did not respond: VRChat did not take a change of sharpness while running.
    NotTaken,
}

/// Before and after, only ever from a comparison that ran to its end.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub kind: Kind,
    pub verdict: Verdict,
    pub before_fps: f32,
    pub after_fps: f32,
    pub before_filled: f32,
    pub after_filled: f32,
    pub before_over_2x: f32,
    pub after_over_2x: f32,
    pub before_late_cpu: f32,
    pub after_late_cpu: f32,
    pub before_late_gpu: f32,
    pub after_late_gpu: f32,
    /// Frames over twice the budget just after the switches, which the averages leave out: never hidden.
    pub stalls_at_switch: u32,
}

/// Which of the two is in force now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Without,
    With,
}

/// A comparison under way.
#[derive(Clone, Debug)]
pub struct Trial {
    pub kind: Kind,
    started: f64,
    settle: f64,
    refresh_hz: f32,
    blocks: [Vec<Frame>; BLOCKS],
    settling: Vec<Frame>,
    /// What the frames were just before it began, as a trial's own blocks are the only comparison it may use.
    pub people_at_start: Option<u32>,
    /// To be kept if it gains, or only to be told about.
    pub keep_if_better: bool,
}

impl Trial {
    pub fn new(kind: Kind, now: f64, refresh_hz: f32, people_at_start: Option<u32>, keep_if_better: bool) -> Self {
        let settle = if kind == Kind::Sharpness { SETTLE_RESOLUTION_S } else { SETTLE_S };
        Self { kind, started: now, settle, refresh_hz, blocks: Default::default(), settling: Vec::new(), people_at_start, keep_if_better }
    }

    fn block_at(&self, now: f64) -> usize {
        (((now - self.started).max(0.0) / BLOCK_S) as usize).min(BLOCKS - 1)
    }

    /// Which side should be in force now: without, with, without, with.
    pub fn side(&self, now: f64) -> Side {
        if self.block_at(now).is_multiple_of(2) {
            Side::Without
        } else {
            Side::With
        }
    }

    pub fn seconds_left(&self, now: f64) -> f32 {
        (self.started + BLOCK_S * BLOCKS as f64 - now).max(0.0) as f32
    }

    pub fn done(&self, now: f64) -> bool {
        now - self.started >= BLOCK_S * BLOCKS as f64
    }

    pub fn push(&mut self, now: f64, frame: Frame) {
        let at = self.block_at(now);
        let into_block = now - self.started - at as f64 * BLOCK_S;
        if into_block < self.settle {
            self.settling.push(frame);
        } else {
            self.blocks[at].push(frame);
        }
    }

    /// The frames so far, for the raw log: nothing is kept for it apart from these.
    pub fn finish(&self) -> Option<Comparison> {
        let summaries: Vec<Summary> = self.blocks.iter().filter_map(|b| summarize(b, self.refresh_hz)).collect();
        let [a1, b1, a2, b2] = summaries.as_slice() else { return None };
        if [a1, b1, a2, b2].iter().any(|s| s.frames < FEWEST_BLOCK_FRAMES) {
            return None;
        }
        let budget = crate::frames::budget_ms(self.refresh_hz);
        let stalls = self.settling.iter().filter(|f| f.interval_ms > 2.0 * budget).count() as u32;
        Some(judge(self.kind, [*a1, *a2], [*b1, *b2], stalls))
    }
}

/// The rules: both blocks with the change beat both without, on real frames or on stalls, the two without agree, and what
/// the change did not aim at did not get worse.
pub fn judge(kind: Kind, a: [Summary; 2], b: [Summary; 2], stalls_at_switch: u32) -> Comparison {
    let mean = |pair: [Summary; 2], read: fn(&Summary) -> f32| (read(&pair[0]) + read(&pair[1])) / 2.0;
    let (a_fps, b_fps) = (a.map(|s| s.real_fps), b.map(|s| s.real_fps));
    let (a_low, a_high) = (a_fps[0].min(a_fps[1]), a_fps[0].max(a_fps[1]));
    let (b_low, _) = (b_fps[0].min(b_fps[1]), b_fps[0].max(b_fps[1]));
    let agree = a_high <= 0.0 || (a_high - a_low) / a_high <= A_BLOCKS_AGREE;
    let gained_frames = b_low >= a_high * (1.0 + MIN_GAIN);
    let (a_stalls, b_stalls) = (a.map(|s| s.over_2x_share), b.map(|s| s.over_2x_share));
    let a_stalls_low = a_stalls[0].min(a_stalls[1]);
    let b_stalls_high = b_stalls[0].max(b_stalls[1]);
    let fewer_stalls = a_stalls_low >= STALLS_WORTH_JUDGING && b_stalls_high <= a_stalls_low * (1.0 - MIN_STALL_DROP);
    let (other_a, other_b) = match kind {
        Kind::Sharpness | Kind::PanelQuality => (a.map(|s| s.late_cpu_share), b.map(|s| s.late_cpu_share)),
        Kind::Programs | Kind::ReconstructionHold => (a.map(|s| s.late_gpu_share), b.map(|s| s.late_gpu_share)),
    };
    let other_fine = other_b[0].max(other_b[1]) <= other_a[0].max(other_a[1]) + OTHER_LATE_SLACK;
    let gpu_responded = {
        let before = mean(a, |s| s.app_gpu.median);
        let after = mean(b, |s| s.app_gpu.median);
        before <= 0.0 || after <= before * (1.0 - GPU_MUST_FALL)
    };
    let verdict = if kind == Kind::Sharpness && !gpu_responded {
        Verdict::NotTaken
    } else if !agree {
        Verdict::TooMuchChanging
    } else if (gained_frames || fewer_stalls) && other_fine {
        Verdict::Kept
    } else {
        Verdict::NoGain
    };
    Comparison {
        kind,
        verdict,
        before_fps: mean(a, |s| s.real_fps),
        after_fps: mean(b, |s| s.real_fps),
        before_filled: mean(a, |s| s.filled_share),
        after_filled: mean(b, |s| s.filled_share),
        before_over_2x: mean(a, |s| s.over_2x_share),
        after_over_2x: mean(b, |s| s.over_2x_share),
        before_late_cpu: mean(a, |s| s.late_cpu_share),
        after_late_cpu: mean(b, |s| s.late_cpu_share),
        before_late_gpu: mean(a, |s| s.late_gpu_share),
        after_late_gpu: mean(b, |s| s.late_gpu_share),
        stalls_at_switch,
    }
}

/// A trial is abandoned when what it compares stopped being the same scene: over 10% of the people came or went.
pub fn people_changed_too_much(at_start: Option<u32>, now: Option<u32>) -> bool {
    match (at_start, now) {
        (Some(before), Some(now)) => (now as f32 - before as f32).abs() > (before.max(1) as f32) * 0.10,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::steady;

    fn block(fps: f32, stalls: f32, late_cpu: f32, late_gpu: f32, gpu_ms: f32) -> Summary {
        let mut s = Summary { frames: 400, real_fps: fps, over_2x_share: stalls, late_cpu_share: late_cpu, late_gpu_share: late_gpu, ..Summary::default() };
        s.app_gpu.median = gpu_ms;
        s
    }

    fn plain(fps: f32) -> Summary {
        block(fps, 0.0, 0.0, 0.0, 10.0)
    }

    #[test]
    fn both_blocks_must_beat_both_blocks() {
        assert_eq!(judge(Kind::Programs, [plain(36.0), plain(36.0)], [plain(40.0), plain(40.0)], 0).verdict, Verdict::Kept);
        // One of the two with the change did not gain: no.
        assert_eq!(judge(Kind::Programs, [plain(36.0), plain(36.0)], [plain(40.0), plain(36.5)], 0).verdict, Verdict::NoGain);
        // A gain smaller than the rule: no.
        assert_eq!(judge(Kind::Programs, [plain(36.0), plain(36.0)], [plain(37.0), plain(37.0)], 0).verdict, Verdict::NoGain);
        // No gain at all, the honest result: said as such.
        assert_eq!(judge(Kind::Programs, [plain(36.0), plain(36.0)], [plain(36.0), plain(36.0)], 0).verdict, Verdict::NoGain);
    }

    #[test]
    fn the_blocks_without_must_agree() {
        let got = judge(Kind::Programs, [plain(30.0), plain(36.0)], [plain(50.0), plain(50.0)], 0);
        assert_eq!(got.verdict, Verdict::TooMuchChanging, "whatever the blocks with it say");
    }

    #[test]
    fn fewer_stalls_count_when_there_were_enough_to_count() {
        let a = [block(36.0, 0.10, 0.0, 0.0, 10.0), block(36.0, 0.10, 0.0, 0.0, 10.0)];
        let b = [block(36.0, 0.05, 0.0, 0.0, 10.0), block(36.0, 0.07, 0.0, 0.0, 10.0)];
        assert_eq!(judge(Kind::Programs, a, b, 0).verdict, Verdict::Kept);
        let a = [block(36.0, 0.002, 0.0, 0.0, 10.0), block(36.0, 0.002, 0.0, 0.0, 10.0)];
        let b = [block(36.0, 0.0, 0.0, 0.0, 10.0), block(36.0, 0.0, 0.0, 0.0, 10.0)];
        assert_eq!(judge(Kind::Programs, a, b, 0).verdict, Verdict::NoGain, "two stalls among a thousand frames say nothing");
    }

    #[test]
    fn what_it_did_not_aim_at_must_not_get_worse() {
        let a = [block(36.0, 0.0, 0.0, 0.05, 10.0), block(36.0, 0.0, 0.0, 0.05, 10.0)];
        let b = [block(40.0, 0.0, 0.0, 0.20, 10.0), block(40.0, 0.0, 0.0, 0.20, 10.0)];
        assert_eq!(judge(Kind::Programs, a, b, 0).verdict, Verdict::NoGain);
        let a = [block(36.0, 0.0, 0.05, 0.0, 10.0), block(36.0, 0.0, 0.05, 0.0, 10.0)];
        let b = [block(40.0, 0.0, 0.20, 0.0, 8.0), block(40.0, 0.0, 0.20, 0.0, 8.0)];
        assert_eq!(judge(Kind::Sharpness, a, b, 0).verdict, Verdict::NoGain);
    }

    /// Test A's question: does VRChat take a change of sharpness while it runs? If the graphics card's time does not move,
    /// that is what is said, whatever frames did by chance.
    #[test]
    fn sharpness_the_graphics_card_did_not_feel_was_not_taken() {
        let a = [plain(36.0), plain(36.0)];
        let b = [plain(40.0), plain(40.0)];
        assert_eq!(judge(Kind::Sharpness, a, b, 0).verdict, Verdict::NotTaken);
        let b = [block(40.0, 0.0, 0.0, 0.0, 8.0), block(40.0, 0.0, 0.0, 0.0, 8.0)];
        assert_eq!(judge(Kind::Sharpness, a, b, 0).verdict, Verdict::Kept);
    }

    #[test]
    fn it_runs_without_with_without_with() {
        let mut trial = Trial::new(Kind::Programs, 100.0, 72.0, None, true);
        assert_eq!(trial.side(100.0), Side::Without);
        assert_eq!(trial.side(115.5), Side::With);
        assert_eq!(trial.side(131.0), Side::Without);
        assert_eq!(trial.side(159.0), Side::With);
        assert!(!trial.done(159.0));
        assert!(trial.done(160.0));
        for (block, fps) in [(0.0, 36.0), (15.0, 40.0), (30.0, 36.0), (45.0, 40.0)] {
            for f in steady(100.0 + block, 15.0, fps, Frame { presents: 1, ..Frame::default() }) {
                trial.push(f.time_s, f);
            }
        }
        let got = trial.finish().unwrap();
        assert_eq!(got.verdict, Verdict::Kept);
        assert!((got.before_fps - 36.0).abs() < 0.5 && (got.after_fps - 40.0).abs() < 0.5, "{got:?}");
    }

    /// The first seconds after a switch are out of the averages, and their stalls are in the result anyway.
    #[test]
    fn a_stall_at_the_switch_is_left_out_of_the_average_and_shown() {
        let mut trial = Trial::new(Kind::Programs, 0.0, 72.0, None, true);
        for (block, fps) in [(0.0, 36.0), (15.0, 40.0), (30.0, 36.0), (45.0, 40.0)] {
            let mut frames = steady(block, 15.0, fps, Frame { presents: 1, ..Frame::default() });
            if block == 15.0 {
                for f in frames.iter_mut().take(10) {
                    f.interval_ms = 100.0;
                }
            }
            for f in frames {
                trial.push(f.time_s, f);
            }
        }
        let got = trial.finish().unwrap();
        assert_eq!(got.stalls_at_switch, 10);
        assert_eq!(got.after_over_2x, 0.0);
    }

    #[test]
    fn a_block_with_almost_nothing_says_nothing() {
        let mut trial = Trial::new(Kind::Programs, 0.0, 72.0, None, true);
        for f in steady(0.0, 15.0, 36.0, Frame { presents: 1, ..Frame::default() }) {
            trial.push(f.time_s, f);
        }
        assert!(trial.finish().is_none(), "only one block has frames");
    }

    #[test]
    fn people_coming_and_going_end_a_trial() {
        assert!(!people_changed_too_much(Some(20), Some(22)));
        assert!(people_changed_too_much(Some(20), Some(23)));
        assert!(people_changed_too_much(Some(20), Some(17)));
        assert!(!people_changed_too_much(None, Some(17)));
        assert!(people_changed_too_much(Some(1), Some(3)));
    }
}
