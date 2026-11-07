//! Admission and prediction for displayed content. Unlike background reconstruction, it must make progress even
//! beside a persistently late scene. All clocks are supplied by the caller, so overload and frame restarts are testable.

use orbiters_openvr::compositor::FrameTiming;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const MAX_INTERVAL: Duration = Duration::from_millis(250);
const RECOVER: Duration = Duration::from_millis(500);
/// What share of the time a displayed capture may spend drawing.
pub const DUTY: f64 = 0.40;

/// The same high-resolution sleep slice is used by the overlay and the windowless timing experiment.
pub fn service_slice(left: Duration, wake: Option<Duration>) -> Duration {
    wake.map_or(left, |d| d.min(left)).max(Duration::from_micros(500))
}

/// Tracking noise while stationary does not spend GPU time. A final settling picture removes motion prediction
/// after a stop. Even with a quiet velocity estimate, a 3 mm displacement recovers very slow movement.
pub fn translation_due(metres: f32, moving: bool, still: bool, settling: bool) -> bool {
    settling || metres > 0.003 || (!still && metres > if moving { 0.001 } else { super::view::MOVED_M })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Wide = 0,
    Face = 1,
    Cube = 2,
}

#[derive(Clone, Copy)]
struct Cost {
    mean: f64,
    deviation: f64,
    sampled: bool,
}

#[derive(Clone, Copy)]
struct Exposure {
    id: u64,
    start: u32,
    end: Option<u32>,
    escalated: bool,
}

pub struct Pacer {
    pub level: u32,
    pub attributed: u64,
    pub unattributed: u64,
    pub ignored: u64,
    last_frame: Option<u32>,
    exposures: VecDeque<Exposure>,
    recovered: Option<Instant>,
    costs: [Cost; 3],
    last_admit: Option<Instant>,
    deadline: Option<Instant>,
    pub interval: Duration,
}

fn newer(frame: u32, previous: u32) -> bool {
    let d = frame.wrapping_sub(previous);
    d != 0 && d < 1 << 31
}

impl Default for Pacer {
    fn default() -> Self {
        Self {
            level: 0,
            attributed: 0,
            unattributed: 0,
            ignored: 0,
            last_frame: None,
            exposures: VecDeque::new(),
            recovered: None,
            costs: [0.012, 0.012, 0.060].map(|mean| Cost { mean, deviation: 0.0, sampled: false }),
            last_admit: None,
            deadline: None,
            interval: Duration::from_secs_f64(1.0 / 30.0),
        }
    }
}

impl Pacer {
    /// Frames oldest first, polled even during a job/hold. Only new late frames overlapping one of our jobs (or
    /// its following two scene frames) escalate, at most once per job. A compositor drop alone has no known cause.
    pub fn observe(&mut self, now: Instant, frames: &[FrameTiming], protect: bool) {
        if !protect || frames.is_empty() {
            self.level = 0;
            self.exposures.clear();
            self.last_frame = frames.last().map(|f| f.frame_index);
            self.recovered = Some(now);
            return;
        }
        if let (Some(last), Some(new)) = (self.last_frame, frames.last()) {
            // A history going backwards is a compositor restart, not an unsigned wrap. Do not attribute old work.
            if new.frame_index != last && !newer(new.frame_index, last) {
                self.last_frame = None;
                self.exposures.clear();
                self.level = 0;
            }
        }
        for t in frames {
            if self.last_frame.is_some_and(|last| !newer(t.frame_index, last)) {
                continue;
            }
            self.last_frame = Some(t.frame_index);
            if t.reprojection_flags & 0x02 == 0 && t.num_dropped_frames == 0 {
                if t.reprojection_flags != 0 || t.num_mis_presented != 0 {
                    self.ignored += 1;
                }
                continue;
            }
            let exposure =
                self.exposures.iter_mut().rev().find(|j| newer(t.frame_index, j.start) && j.end.is_none_or(|end| !newer(t.frame_index, end.wrapping_add(2))));
            if let Some(j) = exposure {
                if !j.escalated {
                    j.escalated = true;
                    self.level = (self.level + 1).min(4);
                    self.attributed += 1;
                    self.recovered = Some(now);
                }
            } else {
                self.unattributed += 1;
            }
        }
        if self.level > 0 && self.recovered.is_none_or(|t| now.saturating_duration_since(t) >= RECOVER) {
            self.level -= 1;
            self.recovered = Some(now);
        }
    }

    /// Nominal caps stay 20/30/45 Hz. Absolute deadlines avoid rounding every interval up to an unrelated 11 ms
    /// input-loop tick. Long jobs are limited by measured wall cost; overload remains live, with one job at a time.
    pub fn configure(&mut self, now: Instant, rate: f32) {
        let nominal = 1.0 / rate.max(1.0) as f64;
        let cost = self.costs[Kind::Wide as usize];
        let duty = if cost.sampled { cost.mean / DUTY } else { nominal };
        // Very expensive jobs may need longer than the liveness floor to respect the duty ceiling. They still
        // make progress; pretending a 200 ms batch is safe every 250 ms would spend most of the game's GPU time.
        let interval = Duration::from_secs_f64((nominal * (1u32 << self.level) as f64).min(MAX_INTERVAL.as_secs_f64()).max(duty));
        if interval != self.interval {
            self.interval = interval;
            self.deadline = self.last_admit.map(|t| (t + interval).max(now));
        }
    }

    pub fn due(&self, now: Instant) -> bool {
        self.deadline.is_none_or(|t| now >= t)
    }

    pub fn wake_after(&self, now: Instant) -> Option<Duration> {
        self.deadline.filter(|t| *t > now).map(|t| t - now)
    }

    pub fn admitted(&mut self, now: Instant, id: u64) {
        // Preserve cadence through small wake jitter, but never catch up by queueing old poses after a slow job.
        self.deadline = Some(match self.deadline {
            Some(due) if now.saturating_duration_since(due) < self.interval / 2 => due + self.interval,
            _ => now + self.interval,
        });
        self.last_admit = Some(now);
        if let Some(start) = self.last_frame {
            self.exposures.push_back(Exposure { id, start, end: None, escalated: false });
            if self.exposures.len() > 16 {
                self.exposures.pop_front();
            }
        }
    }

    pub fn completed(&mut self, id: u64, kind: Kind, readiness: Duration) {
        if let Some(j) = self.exposures.iter_mut().find(|j| j.id == id) {
            j.end = self.last_frame.or(Some(j.start));
        }
        let c = &mut self.costs[kind as usize];
        let seconds = readiness.as_secs_f64().clamp(0.0001, 0.5);
        if c.sampled {
            c.deviation = 0.8 * c.deviation + 0.2 * (seconds - c.mean).abs();
            c.mean = 0.8 * c.mean + 0.2 * seconds;
        } else {
            c.mean = seconds;
            c.deviation = 0.0;
            c.sampled = true;
        }
    }

    pub fn readiness(&self, kind: Kind) -> f64 {
        let c = self.costs[kind as usize];
        c.mean + c.deviation
    }

    /// Modelled photon horizon, not a measured latch deadline. Include a small conservative latch lead and a
    /// bounded part of the picture's lifetime, only while velocity is stable (stop/reversal must not overshoot).
    pub fn horizon(&self, kind: Kind, hz: f32, since_vsync: f32, photons: f32, steady_motion: bool) -> f32 {
        prediction(self.readiness(kind), self.interval.as_secs_f64(), hz, since_vsync, photons, if steady_motion { 0.25 } else { 0.0 })
    }
}

pub fn prediction(ready: f64, interval: f64, hz: f32, since: f32, photons: f32, beta: f64) -> f32 {
    let frame = 1.0 / if hz.is_finite() && hz > 1.0 { hz as f64 } else { 90.0 };
    let since = if since.is_finite() { (since as f64).clamp(0.0, frame) } else { 0.0 };
    let next = frame - since;
    let cycles = ((ready.clamp(0.0, 0.5) + 0.003 - next) / frame).ceil().max(0.0);
    let photons = if photons.is_finite() { photons.clamp(0.0, 0.05) as f64 } else { 0.011 };
    (next + cycles * frame + photons + (beta * interval).clamp(0.0, 0.012)).clamp(0.0, 0.1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(index: u32, flags: u32) -> FrameTiming {
        FrameTiming { frame_index: index, reprojection_flags: flags, ..Default::default() }
    }

    #[test]
    fn stationary_jitter_is_idle_and_stops_get_a_settling_picture() {
        for rms in [0.0003, 0.001] {
            for i in 0..6000 {
                let jitter = (i as f32 * 0.731).sin() * rms * 2.828;
                assert!(!translation_due(jitter.abs(), false, true, false));
            }
        }
        assert!(translation_due(0.0001, false, true, true));
        assert!(translation_due(0.0011, true, false, false));
        assert!(translation_due(0.0031, false, true, false));
        assert!(translation_due(0.0026, false, false, false));
    }

    #[test]
    fn persistent_lateness_is_live_including_the_first_cube() {
        let start = Instant::now();
        let mut p = Pacer::default();
        let mut admitted = Vec::new();
        let mut pending = None;
        for ms in 0..20_000 {
            let now = start + Duration::from_millis(ms);
            p.observe(now, &[frame((ms * 90 / 1000) as u32, 2)], true);
            if let Some((id, done, kind)) = pending {
                if ms >= done {
                    p.completed(id, kind, Duration::from_millis(if kind == Kind::Cube { 60 } else { 6 }));
                    pending = None;
                }
            }
            p.configure(now, 30.0);
            if pending.is_none() && p.due(now) {
                let id = admitted.len() as u64;
                p.admitted(now, id);
                admitted.push(ms);
                let kind = if id == 0 { Kind::Cube } else { Kind::Wide };
                pending = Some((id, ms + if id == 0 { 60 } else { 6 }, kind));
            }
        }
        assert_eq!(admitted[0], 0, "first cube cannot be starved by preexisting lateness");
        assert!(admitted.len() >= 77, "{admitted:?}");
        assert!(admitted.windows(2).all(|p| p[1] - p[0] <= 251));
    }

    #[test]
    fn sliding_history_escalates_once_and_recovers_by_elapsed_time() {
        let start = Instant::now();
        let mut p = Pacer::default();
        p.observe(start, &[frame(9, 0)], true);
        p.admitted(start, 1);
        for i in 0..16 {
            p.observe(start + Duration::from_millis(i * 5), &[frame(10, 2), frame(11, 0)], true);
        }
        assert_eq!((p.level, p.attributed), (1, 1));
        p.completed(1, Kind::Wide, Duration::from_millis(6));
        p.observe(start + RECOVER, &[frame(12, 0)], true);
        assert_eq!(p.level, 0);
    }

    #[test]
    fn cpu_async_throttle_unattributed_and_loading_frames_do_not_escalate() {
        let now = Instant::now();
        let mut p = Pacer::default();
        p.observe(now, &[frame(1, 2)], true);
        assert_eq!(p.level, 0);
        p.admitted(now, 1);
        for (i, flags) in [1, 4, 8, 0x100].into_iter().enumerate() {
            p.observe(now, &[frame(i as u32 + 2, flags)], true);
        }
        assert_eq!(p.level, 0);
        p.observe(now, &[frame(8, 2)], false);
        assert_eq!(p.level, 0);
    }

    #[test]
    fn wrap_restart_and_missing_history_do_not_stick() {
        let now = Instant::now();
        let mut p = Pacer::default();
        p.observe(now, &[frame(u32::MAX - 1, 0)], true);
        p.admitted(now, 1);
        p.observe(now, &[frame(u32::MAX, 2), frame(0, 2)], true);
        assert_eq!(p.attributed, 1);
        p.observe(now, &[frame(9000, 0)], true);
        p.observe(now, &[frame(1, 2)], true);
        assert_eq!(p.level, 0);
        p.observe(now, &[], true);
        assert_eq!(p.level, 0);
    }

    #[test]
    fn cadence_does_not_round_each_interval_to_the_input_loop_or_catch_up() {
        for rate in [20.0, 30.0, 45.0] {
            let start = Instant::now();
            let mut p = Pacer::default();
            let mut n = 0;
            for ms in 0..10_000 {
                let now = start + Duration::from_millis(ms);
                p.configure(now, rate);
                if p.due(now) {
                    p.admitted(now, n);
                    n += 1;
                }
            }
            assert!((n as f32 - rate * 10.0).abs() <= 1.0, "{rate}: {n}");
            p.admitted(start + Duration::from_secs(20), n);
            assert!(!p.due(start + Duration::from_secs(20) + Duration::from_millis(1)));
        }
    }

    #[test]
    fn long_jobs_limit_duty_and_cube_cost_does_not_pollute_wide_prediction() {
        let now = Instant::now();
        let mut p = Pacer::default();
        let before = p.horizon(Kind::Wide, 90.0, 0.0, 0.011, true);
        p.completed(1, Kind::Cube, Duration::from_millis(80));
        assert_eq!(before, p.horizon(Kind::Wide, 90.0, 0.0, 0.011, true));
        p.completed(2, Kind::Wide, Duration::from_millis(40));
        p.configure(now, 45.0);
        assert!(p.interval >= Duration::from_millis(100));
        for hz in [72.0, 80.0, 90.0, 120.0, 144.0] {
            for phase in [0.0, 0.003, 0.009] {
                let h = p.horizon(Kind::Wide, hz, phase, 0.011, true);
                assert!((0.04..=0.1).contains(&h));
            }
        }
    }
}
