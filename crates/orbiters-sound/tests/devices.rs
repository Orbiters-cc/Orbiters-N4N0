//! On this PC's real devices (run by hand): cargo test -p orbiters-sound --release --test devices -- --ignored --nocapture

use orbiters_sound::{MicMonitor, Output, Sound, RATE};
use std::sync::Arc;
use std::time::Duration;

#[test]
#[ignore]
fn output_plays_a_silent_sound() {
    let out = Output::new();
    out.play(&Arc::new(Sound { rate: RATE, frames: vec![[0.0; 2]; RATE as usize / 4] }));
    // Opens, plays, then lets the device go a second after.
    std::thread::sleep(Duration::from_millis(1600));
}

#[test]
#[ignore]
fn the_mic_monitor_starts_and_measures() {
    let m = MicMonitor::start(None).expect("default microphone");
    std::thread::sleep(Duration::from_millis(300));
    let level = m.take_level();
    eprintln!("level {level:.4}");
    assert!((0.0..=1.0).contains(&level));
}
