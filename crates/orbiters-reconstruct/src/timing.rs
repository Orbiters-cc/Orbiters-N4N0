//! Lab timings (`RECONSTRUCT_TIMING=1`): sections of a run, each measured between two waits for the GPU, so that work
//! queued by the steps before it is not counted in it. Printed as `timing <section> <seconds>` lines on stderr.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static TOTALS: Mutex<BTreeMap<&'static str, (Duration, u32)>> = Mutex::new(BTreeMap::new());

pub fn on() -> bool {
    std::env::var_os("RECONSTRUCT_TIMING").is_some()
}

/// Runs `f` as section `name`: when timing, the device is idle before and after it.
pub fn section<T>(name: &'static str, device: Option<&wgpu::Device>, f: impl FnOnce() -> T) -> T {
    if !on() {
        return f();
    }
    if let Some(d) = device {
        let _ = d.poll(wgpu::PollType::wait_indefinitely());
    }
    let t = Instant::now();
    let out = f();
    if let Some(d) = device {
        let _ = d.poll(wgpu::PollType::wait_indefinitely());
    }
    add(name, t.elapsed());
    out
}

pub fn add(name: &'static str, d: Duration) {
    if on() {
        let mut t = TOTALS.lock().unwrap();
        let e = t.entry(name).or_insert((Duration::ZERO, 0));
        e.0 += d;
        e.1 += 1;
    }
}

pub fn print() {
    if on() {
        for (name, (d, n)) in TOTALS.lock().unwrap().iter() {
            eprintln!("timing {name} {:.3} s ({n}×)", d.as_secs_f64());
        }
    }
}
