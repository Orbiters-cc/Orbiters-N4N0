//! Training beside VRChat in VR without taking its frames (crate::gpu::Gpu::governor). SteamVR's frame timing says how
//! much of each frame the GPU spent on the scene application and the compositor, and whether a frame came late. Before
//! each part of a training step goes to the GPU, the governor waits until the last frames left that part's own GPU
//! time idle, and backs off (from 50 ms, doubling, up to 2 s) after any frame SteamVR dropped or reprojected for the
//! GPU: PilotFish (USENIX ATC '22, up to 85 % of a game's idle GPU cycles harvested without hurting its frame rate)
//! without its hooks into the game, which Orbiters never has. The timings include the trainer's own work, so a step
//! that slows VRChat's frames holds back the next ones. Without a scene application's frames (SteamVR closed, VRChat on
//! the desktop) it waits for nothing.

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
    client: Client,
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
    /// A background SteamVR client (it never starts SteamVR), while SteamVR runs.
    pub fn connect() -> Option<Self> {
        let client = Client::connect(AppType::Background).ok()?;
        let hz = client.float_property(0, orbiters_openvr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).unwrap_or(90.0);
        Some(Self { client, frame_ms: 1000.0 / hz, timings: [FrameTiming::default(); RECENT], backoff: Duration::ZERO, answered: 0, waited: Duration::ZERO, backoffs: 0 })
    }

    /// Waits until the GPU can take `cost` of work without the scene application missing a frame. False once SteamVR
    /// quits: nothing is governed any more.
    pub fn wait(&mut self, cost: Duration) -> bool {
        let start = Instant::now();
        let alive = loop {
            let mut quitting = false;
            while let Some(e) = self.client.poll_event() {
                quitting |= e.event_type == orbiters_openvr::event::QUIT;
            }
            if quitting {
                self.client.acknowledge_quit();
                break false;
            }
            let Ok(compositor) = self.client.compositor() else { break true };
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
}
