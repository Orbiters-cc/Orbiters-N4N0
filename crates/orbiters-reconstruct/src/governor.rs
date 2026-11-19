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
/// The longest sleep between two looks at whether a pause was asked (RECONSTRUCT-15): the app gives a pausing worker
/// 15 s to leave the GPU, and a back-off alone can last 2 s.
const PAUSE_LOOK: Duration = Duration::from_millis(50);

/// Sleeps `d`, looking at `asked` at least every [`PAUSE_LOOK`]; true when it was asked (the sleep is cut short).
fn sleep_unless(d: Duration, asked: &dyn Fn() -> bool) -> bool {
    let end = Instant::now() + d;
    loop {
        if asked() {
            return true;
        }
        let now = Instant::now();
        if now >= end {
            return false;
        }
        std::thread::sleep((end - now).min(PAUSE_LOOK));
    }
}

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

/// What pacing cost a run so far (RECONSTRUCT-10): kept in its report, so that a slow run explains itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    /// Time spent waiting or resting, and back-offs after late frames.
    pub waited: Duration,
    pub backoffs: u32,
    /// Looks that found too little idle time ([`Verdict::Wait`]), the longest single wait, and waits a pause cut short.
    pub waits: u32,
    pub longest: Duration,
    pub cut_by_pause: u32,
    /// Paced by SteamVR's frames, and resting beside VRChat on the desktop, at some point of the run.
    pub vr: bool,
    pub desktop: bool,
}

impl Tally {
    /// Both tallies, as one.
    pub fn add(&self, other: &Tally) -> Tally {
        Tally {
            waited: self.waited + other.waited,
            backoffs: self.backoffs + other.backoffs,
            waits: self.waits + other.waits,
            longest: self.longest.max(other.longest),
            cut_by_pause: self.cut_by_pause + other.cut_by_pause,
            vr: self.vr || other.vr,
            desktop: self.desktop || other.desktop,
        }
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
    /// What it cost since it joined.
    pub tally: Tally,
}

impl Governor {
    /// A background SteamVR client (it never starts SteamVR) while SteamVR runs; else the desktop's share of the GPU.
    pub fn connect() -> Option<Self> {
        let client = Client::connect(AppType::Background).ok();
        let hz = client.as_ref().and_then(|c| c.float_property(0, orbiters_openvr::prop::DISPLAY_FREQUENCY)).filter(|hz| *hz > 1.0).unwrap_or(90.0);
        Some(Self::with(client, 1000.0 / hz))
    }

    fn with(client: Option<Client>, frame_ms: f32) -> Self {
        Self {
            frame_ms,
            timings: [FrameTiming::default(); RECENT],
            backoff: Duration::ZERO,
            answered: 0,
            tally: Tally { vr: client.is_some(), desktop: client.is_none(), ..Default::default() },
            client,
        }
    }

    /// Beside VRChat on the desktop (no SteamVR to read frames from).
    pub fn on_desktop(&self) -> bool {
        self.client.is_none()
    }

    /// Waits until the GPU can take `cost` of work without the scene application missing a frame. False once SteamVR
    /// quits: nothing is governed any more. A pause asked (`asked`) ends the wait at once, as if the frames had room
    /// (RECONSTRUCT-15): the step under way finishes unpaced, and the run pauses between two steps as it always does,
    /// well within the time the app gives it.
    pub fn wait(&mut self, cost: Duration, asked: &dyn Fn() -> bool) -> bool {
        let start = Instant::now();
        let mut cut = false;
        let alive = match self.client.as_ref() {
            None => {
                cut = sleep_unless(desktop_rest(cost), asked);
                true
            }
            Some(client) => loop {
                if asked() {
                    cut = true;
                    break true;
                }
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
                        self.tally.backoffs += 1;
                        sleep_unless(self.backoff, asked);
                    }
                    // About a frame, then the next frames' timing.
                    Verdict::Wait => {
                        self.tally.waits += 1;
                        sleep_unless(Duration::from_secs_f32(self.frame_ms / 1000.0), asked);
                    }
                }
            },
        };
        let took = start.elapsed();
        self.tally.waited += took;
        self.tally.longest = self.tally.longest.max(took);
        self.tally.cut_by_pause += u32::from(cut);
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
        let mut desktop = Governor::with(None, 11.1);
        assert!(desktop.on_desktop());
        let began = Instant::now();
        assert!(desktop.wait(Duration::from_millis(3), &|| false), "the desktop never ends governing by itself");
        assert!(began.elapsed() >= Duration::from_millis(6), "{:?}", began.elapsed());
        assert_eq!(desktop.tally.cut_by_pause, 0);
    }

    /// RECONSTRUCT-15: a pause asked while the trainer rests is seen within a look, not after the whole rest.
    #[test]
    fn a_pause_cuts_a_rest_short() {
        let asked = std::sync::atomic::AtomicBool::new(false);
        let began = Instant::now();
        let cut = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                asked.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            sleep_unless(Duration::from_secs(5), &|| asked.load(std::sync::atomic::Ordering::SeqCst))
        });
        assert!(cut && began.elapsed() < Duration::from_secs(1), "{:?}", began.elapsed());
        assert!(!sleep_unless(Duration::from_millis(5), &|| false));
        // The desktop's rest (250 ms for a slow part) ends at once when the pause was already asked, and says so.
        let mut desktop = Governor::with(None, 11.1);
        let began = Instant::now();
        assert!(desktop.wait(Duration::from_secs(1), &|| true));
        assert!(began.elapsed() < Duration::from_millis(100), "{:?}", began.elapsed());
        assert_eq!(desktop.tally.cut_by_pause, 1);
    }
}
