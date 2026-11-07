//! What one frame says, as SteamVR's compositor saw it, and what a stretch of frames comes to. Local only, and only ever
//! read from SteamVR's frame timing: nothing here looks inside VRChat.
//!
//! What the words mean, as [`openvr.h`](https://github.com/ValveSoftware/openvr/blob/master/headers/openvr.h) has them:
//! a frame presented more than once was reused, and that is SteamVR filling in; the reason flags `0x01` and `0x02` alone
//! say a frame was late for the processor or the graphics card. The other bits say what state SteamVR is in (asynchronous
//! reprojection, motion smoothing, throttling), and they are never counted as late.
//!
//! Virtual Desktop's and other streamers' frame synthesis happens outside SteamVR and does not show here: real frames stay
//! honest, and "filled in" means SteamVR's only.

use serde::{Deserialize, Serialize};

pub const LATE_CPU: u32 = 0x01;
pub const LATE_GPU: u32 = 0x02;
pub const ASYNC: u32 = 0x04;
pub const MOTION: u32 = 0x08;
pub const THROTTLE_MASK: u32 = 0xF00;

/// One frame of the application that has the scene (the fields of `Compositor_FrameTiming` that are used).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub index: u32,
    pub time_s: f64,
    pub presents: u32,
    pub mis_presented: u32,
    pub dropped: u32,
    pub flags: u32,
    pub interval_ms: f32,
    pub pre_submit_gpu_ms: f32,
    pub post_submit_gpu_ms: f32,
    pub total_gpu_ms: f32,
    pub compositor_gpu_ms: f32,
    pub compositor_cpu_ms: f32,
    pub poses_ready_ms: f32,
    pub frame_ready_ms: f32,
}

impl Frame {
    /// How long the application took between getting poses and having its frame ready. An estimate: it may include waiting.
    pub fn app_cpu_ms(&self) -> f32 {
        (self.frame_ready_ms - self.poses_ready_ms).max(0.0)
    }

    pub fn app_gpu_ms(&self) -> f32 {
        self.pre_submit_gpu_ms + self.post_submit_gpu_ms
    }
}

/// How a measure is spread over frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub median: f32,
    pub p95: f32,
    pub p99: f32,
}

impl Spread {
    fn of(mut values: Vec<f32>) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_by(f32::total_cmp);
        let at = |q: f32| values[((values.len() - 1) as f32 * q).round() as usize];
        Self { median: at(0.5), p95: at(0.95), p99: at(0.99) }
    }
}

/// What a stretch of frames comes to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub frames: u32,
    pub seconds: f32,
    /// Distinct frames a second: never counts what SteamVR filled in.
    pub real_fps: f32,
    /// Reused presents out of all presents: what SteamVR filled in.
    pub filled_share: f32,
    pub late_cpu_share: f32,
    pub late_gpu_share: f32,
    pub async_share: f32,
    pub motion_share: f32,
    /// The strongest throttling seen.
    pub throttled: u32,
    pub dropped: u32,
    pub mis_presented: u32,
    /// Frames that took longer than twice the headset's budget: what is felt as a stall.
    pub over_2x: u32,
    pub over_2x_share: f32,
    pub app_cpu: Spread,
    pub app_gpu: Spread,
    /// May include other programs' GPU work, as the header warns.
    pub total_gpu: Spread,
    pub compositor_cpu: Spread,
    pub compositor_gpu: Spread,
    pub interval: Spread,
}

/// The milliseconds one refresh of the headset has.
pub fn budget_ms(refresh_hz: f32) -> f32 {
    if refresh_hz > 1.0 {
        1000.0 / refresh_hz
    } else {
        1000.0 / 90.0
    }
}

/// What these frames (oldest first, each frame once) come to; `None` with too few to say anything.
pub fn summarize(frames: &[Frame], refresh_hz: f32) -> Option<Summary> {
    let (first, last) = (frames.first()?, frames.last()?);
    let seconds = (last.time_s - first.time_s) as f32;
    if frames.len() < 2 || seconds <= 0.0 {
        return None;
    }
    let count = frames.len() as f32;
    let share = |flag: u32| frames.iter().filter(|f| f.flags & flag != 0).count() as f32 / count;
    let presents: u32 = frames.iter().map(|f| f.presents).sum();
    let reused: u32 = frames.iter().map(|f| f.presents.saturating_sub(1)).sum();
    let budget = budget_ms(refresh_hz);
    let over_2x = frames.iter().filter(|f| f.interval_ms > 2.0 * budget).count() as u32;
    let spread = |read: fn(&Frame) -> f32| Spread::of(frames.iter().map(read).collect());
    Some(Summary {
        frames: frames.len() as u32,
        seconds,
        real_fps: (count - 1.0) / seconds,
        filled_share: if presents > 0 { reused as f32 / presents as f32 } else { 0.0 },
        late_cpu_share: share(LATE_CPU),
        late_gpu_share: share(LATE_GPU),
        async_share: share(ASYNC),
        motion_share: share(MOTION),
        throttled: frames.iter().map(|f| (f.flags & THROTTLE_MASK) >> 8).max().unwrap_or(0),
        dropped: frames.iter().map(|f| f.dropped).sum(),
        mis_presented: frames.iter().map(|f| f.mis_presented).sum(),
        over_2x,
        over_2x_share: over_2x as f32 / count,
        app_cpu: spread(Frame::app_cpu_ms),
        app_gpu: spread(Frame::app_gpu_ms),
        total_gpu: spread(|f| f.total_gpu_ms),
        compositor_cpu: spread(|f| f.compositor_cpu_ms),
        compositor_gpu: spread(|f| f.compositor_gpu_ms),
        interval: spread(|f| f.interval_ms),
    })
}

/// Frames at a steady rate, for tests here and elsewhere.
#[cfg(test)]
pub(crate) fn steady(start_s: f64, seconds: f64, fps: f64, template: Frame) -> Vec<Frame> {
    let count = (seconds * fps) as usize;
    (0..count).map(|i| Frame { index: i as u32, time_s: start_s + i as f64 / fps, interval_ms: (1000.0 / fps) as f32, ..template }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT_72: f32 = 72.0;

    #[test]
    fn real_frames_are_distinct_frames_a_second() {
        let frames = steady(0.0, 10.0, 36.0, Frame { presents: 2, ..Frame::default() });
        let got = summarize(&frames, AT_72).unwrap();
        assert!((got.real_fps - 36.0).abs() < 0.2, "{}", got.real_fps);
        assert!((got.filled_share - 0.5).abs() < 0.01, "one of two presents reused: {}", got.filled_share);
    }

    /// Only 0x01 and 0x02 say late. The earlier reading counted any flag without the GPU's, which took in the async and
    /// prediction bits, and that is what this guards against.
    #[test]
    fn only_the_two_reason_bits_count_as_late() {
        let frames = steady(0.0, 5.0, 72.0, Frame { presents: 1, flags: ASYNC | MOTION | 0x30, ..Frame::default() });
        let got = summarize(&frames, AT_72).unwrap();
        assert_eq!((got.late_cpu_share, got.late_gpu_share), (0.0, 0.0));
        assert_eq!((got.async_share, got.motion_share), (1.0, 1.0));
        let frames = steady(0.0, 5.0, 72.0, Frame { presents: 1, flags: LATE_CPU, ..Frame::default() });
        let got = summarize(&frames, AT_72).unwrap();
        assert_eq!((got.late_cpu_share, got.late_gpu_share), (1.0, 0.0));
    }

    #[test]
    fn throttling_is_a_state_not_lateness() {
        let frames = steady(0.0, 5.0, 72.0, Frame { presents: 1, flags: 0x200, ..Frame::default() });
        let got = summarize(&frames, AT_72).unwrap();
        assert_eq!(got.throttled, 2);
        assert_eq!(got.late_cpu_share, 0.0);
    }

    #[test]
    fn app_times_come_from_the_right_fields() {
        let frame = Frame { pre_submit_gpu_ms: 7.0, post_submit_gpu_ms: 2.0, total_gpu_ms: 11.0, poses_ready_ms: 1.0, frame_ready_ms: 17.0, presents: 1, ..Frame::default() };
        let got = summarize(&steady(0.0, 5.0, 72.0, frame), AT_72).unwrap();
        assert_eq!((got.app_gpu.median, got.total_gpu.median, got.app_cpu.median), (9.0, 11.0, 16.0));
        // A frame whose poses came after its "ready": an estimate, and never negative.
        assert_eq!(Frame { poses_ready_ms: 5.0, frame_ready_ms: 1.0, ..Frame::default() }.app_cpu_ms(), 0.0);
    }

    #[test]
    fn stalls_are_frames_over_twice_the_budget() {
        let mut frames = steady(0.0, 10.0, 72.0, Frame { presents: 1, ..Frame::default() });
        for f in frames.iter_mut().take(36) {
            f.interval_ms = 40.0;
        }
        let got = summarize(&frames, AT_72).unwrap();
        assert_eq!(got.over_2x, 36);
        assert!((got.over_2x_share - 0.05).abs() < 0.001);
    }

    #[test]
    fn too_few_frames_say_nothing() {
        assert!(summarize(&[], AT_72).is_none());
        assert!(summarize(&[Frame::default()], AT_72).is_none());
    }

    #[test]
    fn percentiles_are_of_the_whole_spread() {
        let got = Spread::of((1..=100).map(|v| v as f32).collect());
        assert_eq!((got.median, got.p95, got.p99), (51.0, 95.0, 99.0));
    }
}
