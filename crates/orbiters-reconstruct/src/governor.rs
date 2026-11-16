//! Training beside VRChat in VR without taking its frames (crate::gpu::Gpu::governor). SteamVR's frame timing says how
//! much of each frame the GPU spent on the scene application and the compositor, and whether a frame came late. Before
//! each part of a training step goes to the GPU, the governor waits until the last frames left that part's own GPU
//! time idle, and backs off (from 50 ms, doubling, up to 2 s) after any frame SteamVR dropped or reprojected for the
//! GPU: PilotFish (USENIX ATC '22, up to 85 % of a game's idle GPU cycles harvested without hurting its frame rate)
//! without its hooks into the game, which Orbiters never has. The timings include the trainer's own work, so a step
//! that slows VRChat's frames holds back the next ones.
//!
//! VRChat on the desktop (SteamVR not running) has no frame timing to read. Up to 0.3.5 the trainer then waited for
//! nothing and took the whole graphics card while the person played, which on a mid-range card is minutes of a halved
//! frame rate after every 3D picture (N-friend-lag, 2026-11-16). There it now rests after each part of a step for as long
//! as the part took the GPU, times (1 / [`DESKTOP_SHARE`] - 1): the trainer keeps to that share of the GPU's time.

use orbiters_openvr::compositor::FrameTiming;
use orbiters_openvr::{AppType, Client};
use std::time::{Duration, Instant};

/// SteamVR's `VRCompositor_ReprojectionReason_Gpu`.
const GPU_REPROJECTION: u32 = 0x02;
/// Frames looked at: about 45 ms at 90 Hz.
const RECENT: usize = 4;
/// Idle GPU time left to the scene application beyond a part's own, milliseconds.
const MARGIN_MS: f32 = 1.0;
const FIRST_BACKOFF: Duration = Duration::from_millis(50);
const LONGEST_BACKOFF: Duration = Duration::from_secs(2);
/// The share of the GPU's time training keeps to beside VRChat on the desktop.
pub const DESKTOP_SHARE: f32 = 0.3;
/// The longest rest after one part (a slow part's rest is not allowed to stall training for seconds).
const LONGEST_REST: Duration = Duration::from_millis(250);

/// How long to rest after a part that took the GPU `cost`, beside VRChat on the desktop.
pub fn desktop_rest(cost: Duration) -> Duration {
    cost.mul_f32(1.0 / DESKTOP_SHARE - 1.0).min(LONGEST_REST)
}

/// What the last frames say of a part of `cost_ms` milliseconds of GPU work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Their idle GPU time holds it.
    Go,
    /// Not enough idle time: look again a frame later.
    Wait,
    /// A frame came late (dropped, or reprojected for the GPU) that no back-off answered yet: the newest frame's index.
    BackOff(u32),
    /// No scene application's frames: nothing to protect.
    Free,
}

/// The verdict on `cost_ms` of work from the last frames (oldest first) of a headset whose frames last `frame_ms`;
/// `answered` is the newest frame a back-off already answered.
pub fn verdict(recent: &[FrameTiming], frame_ms: f32, cost_ms: f32, answered: u32) -> Verdict {
    if recent.is_empty() {
        return Verdict::Free;
    }
    let newest = recent.iter().map(|t| t.frame_index).max().unwrap_or(0);
    let late = recent.iter().any(|t| t.num_dropped_frames > 0 || t.reprojection_flags & GPU_REPROJECTION != 0);
    if late && newest != answered {
        return Verdict::BackOff(newest);
    }
    let busy = recent.iter().map(|t| t.total_render_gpu_ms).fold(0.0, f32::max);
    if frame_ms - busy >= cost_ms + MARGIN_MS {
        Verdict::Go
    } else {
        Verdict::Wait
    }
}

pub struct Governor {
    /// SteamVR's client; `None` beside VRChat on the desktop, where the trainer keeps to [`DESKTOP_SHARE`] of the GPU.
    client: Option<Client>,
    /// A frame of the headset, milliseconds.
    frame_ms: f32,
    timings: [FrameTiming; RECENT],
    backoff: Duration,
    /// The newest frame a back-off answered: a late frame stays among the recent ones for a few frames.
    answered: u32,
    /// Time spent waiting, and back-offs, since the start.
    pub waited: Duration,
    pub backoffs: u32,
}

impl Governor {
    /// A background SteamVR client (it never starts SteamVR) while SteamVR runs; else the desktop's share of the GPU.
    pub fn connect() -> Option<Self> {
        let client = Client::connect(AppType::Background).ok();
        let hz = client.as_ref().and_then(|c| c.float_property(0, orbiters_openvr::prop::DISPLAY_FREQUENCY)).filter(|hz| *hz > 1.0).unwrap_or(90.0);
        Some(Self { client, frame_ms: 1000.0 / hz, timings: [FrameTiming::default(); RECENT], backoff: Duration::ZERO, answered: 0, waited: Duration::ZERO, backoffs: 0 })
    }

    /// Beside VRChat on the desktop (no SteamVR to read frames from).
    pub fn on_desktop(&self) -> bool {
        self.client.is_none()
    }

    /// Waits until the GPU can take `cost` of work without the scene application missing a frame. False once SteamVR
    /// quits: nothing is governed any more.
    pub fn wait(&mut self, cost: Duration) -> bool {
        let start = Instant::now();
        let Some(client) = self.client.as_ref() else {
            std::thread::sleep(desktop_rest(cost));
            self.waited += start.elapsed();
            return true;
        };
        let alive = loop {
            let mut quitting = false;
            while let Some(e) = client.poll_event() {
                quitting |= e.event_type == orbiters_openvr::event::QUIT;
            }
            if quitting {
                client.acknowledge_quit();
                break false;
            }
            let Ok(compositor) = client.compositor() else { break true };
            let n = compositor.frame_timings(&mut self.timings);
            match verdict(&self.timings[..n], self.frame_ms, cost.as_secs_f32() * 1000.0, self.answered) {
                Verdict::Free => break true,
                Verdict::Go => {
                    // Frames on time: the next back-off starts lower.
                    self.backoff /= 2;
                    break true;
                }
                Verdict::BackOff(newest) => {
                    self.answered = newest;
                    self.backoff = (self.backoff * 2).clamp(FIRST_BACKOFF, LONGEST_BACKOFF);
                    self.backoffs += 1;
                    std::thread::sleep(self.backoff);
                }
                // About a frame, then the next frames' timing.
                Verdict::Wait => std::thread::sleep(Duration::from_secs_f32(self.frame_ms / 1000.0)),
            }
        };
        self.waited += start.elapsed();
        alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(index: u32, gpu_ms: f32) -> FrameTiming {
        FrameTiming { frame_index: index, total_render_gpu_ms: gpu_ms, ..Default::default() }
    }

    #[test]
    fn goes_in_idle_time_waits_without_it_and_backs_off_once_per_late_frame() {
        let frame_ms = 1000.0 / 90.0;
        // VRChat's frames take 6 ms of the 11.1: a 2 ms part fits, a 5 ms one does not.
        let calm = [frame(10, 5.5), frame(11, 6.0), frame(12, 5.0), frame(13, 5.8)];
        assert_eq!(verdict(&calm, frame_ms, 2.0, 0), Verdict::Go);
        assert_eq!(verdict(&calm, frame_ms, 5.0, 0), Verdict::Wait);
        // A frame reprojected for the GPU: back off, once for it.
        let mut late = calm;
        late[2].reprojection_flags = GPU_REPROJECTION;
        assert_eq!(verdict(&late, frame_ms, 2.0, 0), Verdict::BackOff(13));
        assert_eq!(verdict(&late, frame_ms, 2.0, 13), Verdict::Go);
        // A dropped frame too; reprojection for the CPU (0x01) is VRChat's own business.
        late[2].reprojection_flags = 0x01;
        assert_eq!(verdict(&late, frame_ms, 2.0, 0), Verdict::Go);
        late[3].num_dropped_frames = 1;
        assert_eq!(verdict(&late, frame_ms, 2.0, 0), Verdict::BackOff(13));
        // No scene application.
        assert_eq!(verdict(&[], frame_ms, 2.0, 0), Verdict::Free);
    }

    /// N-friend-lag: beside VRChat on the desktop the trainer rests in proportion to what each part took, keeping to 30 %
    /// of the GPU's time, and never rests long after one slow part.
    #[test]
    fn on_the_desktop_training_keeps_to_its_share_of_the_gpu() {
        for ms in [1u64, 4, 20] {
            let cost = Duration::from_millis(ms);
            let rest = desktop_rest(cost);
            let share = cost.as_secs_f32() / (cost + rest).as_secs_f32();
            assert!((share - DESKTOP_SHARE).abs() < 0.01, "{ms} ms: share {share}");
        }
        assert_eq!(desktop_rest(Duration::from_secs(1)), LONGEST_REST);
        let mut desktop = Governor { client: None, frame_ms: 11.1, timings: [FrameTiming::default(); RECENT], backoff: Duration::ZERO, answered: 0, waited: Duration::ZERO, backoffs: 0 };
        assert!(desktop.on_desktop());
        let began = Instant::now();
        assert!(desktop.wait(Duration::from_millis(3)), "the desktop never ends governing by itself");
        assert!(began.elapsed() >= Duration::from_millis(6), "{:?}", began.elapsed());
    }
}
