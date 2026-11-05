//! Bounded, local timing summaries. No filesystem work or blocking sends on the overlay thread.

use super::pacing::{Kind, Pacer};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant};

const SAMPLES: usize = 2048;

#[derive(Default)]
struct Series(Vec<f64>);
impl Series {
    fn add(&mut self, value: f64) {
        if value.is_finite() && self.0.len() < SAMPLES {
            self.0.push(value);
        }
    }
    fn take(&mut self) -> Value {
        let mut values = std::mem::take(&mut self.0);
        if values.is_empty() {
            return Value::Null;
        }
        values.sort_by(f64::total_cmp);
        let at = |q: f64| values[((values.len() - 1) as f64 * q) as usize];
        json!({ "n": values.len(), "p50": at(0.5), "p95": at(0.95), "p99": at(0.99), "max": at(1.0) })
    }
}

pub struct Trace {
    tx: Option<SyncSender<Value>>,
    since: Instant,
    last_admit: Option<Instant>,
    last_turn: Option<Instant>,
    intervals: Series,
    turns: Series,
    render: [Series; 3],
    ready_delay: [Series; 3],
    submit: Series,
    horizon: Series,
    levels: [u64; 5],
    pub headset: Value,
}

impl Default for Trace {
    fn default() -> Self {
        Self {
            tx: None,
            since: Instant::now(),
            last_admit: None,
            last_turn: None,
            intervals: Series::default(),
            turns: Series::default(),
            render: Default::default(),
            ready_delay: Default::default(),
            submit: Series::default(),
            horizon: Series::default(),
            levels: [0; 5],
            headset: Value::Null,
        }
    }
}

impl Trace {
    pub fn begin(&mut self) {
        let tx = self.tx.take();
        *self = Self::default();
        self.tx = tx;
    }

    pub fn start(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel::<Value>(2);
        if std::thread::Builder::new()
            .name("3d-worlds-diagnostics".into())
            .spawn(move || {
                let dir = orbiters_core::paths::logs_dir();
                let path = dir.join("overlay-worlds.jsonl");
                if std::fs::create_dir_all(&dir).is_err() {
                    return;
                }
                while let Ok(value) = rx.recv() {
                    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
                        let old = dir.join("overlay-worlds.old.jsonl");
                        let _ = std::fs::remove_file(&old);
                        let _ = std::fs::rename(&path, old);
                    }
                    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&path) {
                        let _ = writeln!(f, "{value}");
                    }
                }
            })
            .is_ok()
        {
            self.tx = Some(tx);
        }
    }

    pub fn turn(&mut self, now: Instant) {
        if let Some(last) = self.last_turn.replace(now) {
            self.turns.add((now - last).as_secs_f64() * 1000.0);
        }
    }

    pub fn admitted(&mut self, now: Instant, horizon: f32) {
        if let Some(last) = self.last_admit.replace(now) {
            self.intervals.add((now - last).as_secs_f64() * 1000.0);
        }
        self.horizon.add(horizon as f64 * 1000.0);
    }

    pub fn rendered(&mut self, kind: Kind, wall_ms: f64, ready_delay: Duration, submit: Duration) {
        self.render[kind as usize].add(wall_ms);
        self.ready_delay[kind as usize].add(ready_delay.as_secs_f64() * 1000.0);
        self.submit.add(submit.as_secs_f64() * 1000.0);
    }

    pub fn flush(&mut self, now: Instant, pacer: &Pacer, force: bool) {
        self.levels[pacer.level.min(4) as usize] += 1;
        if !force && now.saturating_duration_since(self.since) < Duration::from_secs(10) {
            return;
        }
        let render = self.render.each_mut().map(Series::take);
        let ready = self.ready_delay.each_mut().map(Series::take);
        let summary = json!({
            "at": orbiters_core::now_utc_iso(), "version": orbiters_core::APP_VERSION,
            "seconds": now.saturating_duration_since(self.since).as_secs_f64(), "headset": self.headset,
            "kinds": ["wide", "face", "cube"], "render_wall_ms": render, "ready_to_submit_ms": ready,
            "texture_submission_ms": self.submit.take(), "admission_interval_ms": self.intervals.take(),
            "input_turn_ms": self.turns.take(), "prediction_horizon_ms": self.horizon.take(),
            "pacer_level_samples": self.levels, "pacer_interval_ms": pacer.interval.as_secs_f64() * 1000.0,
            "late_attributed_total": pacer.attributed, "late_unattributed_total": pacer.unattributed,
            "non_gpu_flags_total": pacer.ignored,
            "measurement": "CPU wall timings; no photon or compositor-latch measurement"
        });
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(summary);
        }
        self.since = now;
        self.levels = [0; 5];
    }
}
