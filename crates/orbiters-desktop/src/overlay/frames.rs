//! VRChat's frame rate as SteamVR's compositor sees it (contracts `Docs/world-performance.md`): the interval between
//! the scene application's frames, gathered while VRChat has the scene and is not loading, handed to the tray service
//! about every ten seconds. The service matches them with the world and the avatars.

use crate::state::FrameChunk;
use orbiters_openvr::compositor::FrameTiming;
use orbiters_openvr::{self as vr};
use std::time::{Duration, Instant};

const CHUNK: Duration = Duration::from_secs(10);
/// The compositor keeps the last frames: reading 64 of them five times a second misses none below 320 Hz.
const POLL: Duration = Duration::from_millis(200);

pub struct FrameSampler {
    last_frame: Option<u32>,
    intervals: Vec<f32>,
    since: Instant,
    polled: Option<Instant>,
    refresh_hz: Option<f32>,
    headset: Option<String>,
    gpu: Option<String>,
}

impl FrameSampler {
    pub fn new(steamvr: &vr::Client) -> Self {
        Self {
            last_frame: None,
            intervals: Vec::new(),
            since: Instant::now(),
            polled: None,
            refresh_hz: steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0),
            headset: steamvr.string_property(0, vr::prop::MODEL_NUMBER),
            gpu: adapter_name(steamvr),
        }
    }

    /// Reads the frames VRChat showed since the last read (`vrchat_scene`: VRChat has the scene and is not loading),
    /// five times a second whatever the overlay loop's pace; a chunk once ten seconds were gathered.
    pub fn update(&mut self, steamvr: &vr::Client, vrchat_scene: bool) -> Option<FrameChunk> {
        if self.polled.is_some_and(|t| t.elapsed() < POLL) {
            return None;
        }
        self.polled = Some(Instant::now());
        let Ok(compositor) = steamvr.compositor() else { return None };
        let mut timings = [FrameTiming::default(); 64];
        let n = compositor.frame_timings(&mut timings);
        let newest = timings[..n].iter().map(|t| t.frame_index).max();
        if vrchat_scene {
            for t in &timings[..n] {
                let new = self.last_frame.is_none_or(|last| t.frame_index > last);
                if new && t.client_frame_interval_ms > 0.5 && t.client_frame_interval_ms < 2000.0 {
                    self.intervals.push(t.client_frame_interval_ms);
                }
            }
        }
        if newest.is_some() {
            self.last_frame = newest;
        }
        if self.since.elapsed() < CHUNK {
            return None;
        }
        self.since = Instant::now();
        if self.intervals.is_empty() {
            return None;
        }
        Some(FrameChunk {
            intervals_ms: std::mem::take(&mut self.intervals),
            refresh_hz: self.refresh_hz,
            headset: self.headset.clone(),
            gpu: self.gpu.clone(),
        })
    }
}

#[cfg(windows)]
fn adapter_name(steamvr: &vr::Client) -> Option<String> {
    vr::gpu::adapter_name(steamvr.dxgi_adapter_index())
}

#[cfg(not(windows))]
fn adapter_name(_: &vr::Client) -> Option<String> {
    None
}
