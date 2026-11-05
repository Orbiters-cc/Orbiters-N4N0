//! Offscreen/windowless scheduler experiment. No SteamVR session or GPU work. Uses real Windows sleeps, a worker
//! standing in for GPU completion, and the production pacer/service slices. Old code is a measurement reference only.

use super::pacing::{self, Kind, Pacer};
use anyhow::Context;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn distribution(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
    json!({ "n": v.len(), "p50": at(0.5), "p95": at(0.95), "p99": at(0.99), "max": at(1.0) })
}

fn sample(rate: f32, improved: bool, paint: bool, seconds: f64) -> Value {
    let (tx, asks) = mpsc::channel::<u64>();
    let (answers, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        while let Ok(id) = asks.recv() {
            std::thread::sleep(Duration::from_millis(6));
            if answers.send((id, Instant::now())).is_err() {
                break;
            }
        }
    });
    let begin = Instant::now();
    let mut pacer = Pacer::default();
    let (mut pending, mut last, mut previous_turn) = (None::<Instant>, None::<Instant>, None::<Instant>);
    let (mut gaps, mut delays, mut turns) = (Vec::new(), Vec::new(), Vec::new());
    let (mut jobs, mut last_paint) = (0, None::<Instant>);
    while begin.elapsed().as_secs_f64() < seconds {
        let turn = Instant::now();
        if let Some(previous) = previous_turn.replace(turn) {
            turns.push((turn - previous).as_secs_f64() * 1000.0);
        }
        let end = turn + Duration::from_millis(11);
        let mut first_service = true;
        loop {
            let paint_slot = first_service;
            first_service = false;
            let now = Instant::now();
            if let Ok((id, ready)) = rx.try_recv() {
                delays.push((now - ready).as_secs_f64() * 1000.0);
                if let Some(asked) = pending.take() {
                    pacer.completed(id, Kind::Wide, ready - asked);
                }
            }
            pacer.configure(now, rate);
            let due = if improved { pacer.due(now) } else { last.is_none_or(|t| now.duration_since(t).as_secs_f32() >= 1.0 / rate) };
            if pending.is_none() && due {
                if let Some(previous) = last.replace(now) {
                    gaps.push((now - previous).as_secs_f64() * 1000.0);
                }
                pacer.admitted(now, jobs);
                pending = Some(now);
                tx.send(jobs).unwrap();
                jobs += 1;
            }
            if paint_slot && paint && last_paint.is_none_or(|t| t.elapsed() >= Duration::from_millis(33)) {
                last_paint = Some(Instant::now());
                // One non-preemptible UI paint. Both paths pay the same 8 ms; not a CPU-throughput benchmark.
                std::thread::sleep(Duration::from_millis(8));
                if improved {
                    continue;
                } // production publishes after painting
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if !improved {
                std::thread::sleep(left);
                break;
            }
            let wake = if pending.is_some() { Some(Duration::from_millis(1)) } else { pacer.wake_after(Instant::now()) };
            std::thread::sleep(pacing::service_slice(left, wake));
        }
    }
    let elapsed = begin.elapsed().as_secs_f64();
    drop(tx);
    let _ = worker.join();
    json!({ "policy": if improved { "current" } else { "baseline" }, "paint_8ms": paint, "nominal_hz": rate,
        "seconds": elapsed, "jobs": jobs, "jobs_per_second": jobs as f64 / elapsed,
        "admission_interval_ms": distribution(gaps), "ready_to_publish_ms": distribution(delays), "input_turn_ms": distribution(turns) })
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out = Path::new(args.first().context("--world-timing <directory> [seconds per case]")?);
    std::fs::create_dir_all(out)?;
    let seconds = args.get(1).map(|s| s.parse::<f64>()).transpose()?.unwrap_or(4.0).clamp(1.0, 60.0);
    let mut cases = Vec::new();
    // Reverse the A/B order on the second pass, to expose drift instead of always favouring the warm run.
    for pass in 0..2 {
        for paint in [false, true] {
            for rate in [20.0, 30.0, 45.0] {
                for improved in if pass == 0 { [false, true] } else { [true, false] } {
                    let result = sample(rate, improved, paint, seconds);
                    eprintln!("{result}");
                    cases.push(result);
                }
            }
        }
    }
    let report =
        json!({ "measurement": "windowless real-clock scheduling with a 6 ms sleep worker; no GPU, SteamVR or motion-to-photon measurement", "cases": cases });
    std::fs::write(out.join("timing.json"), serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
