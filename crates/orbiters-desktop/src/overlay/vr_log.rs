//! `vr-test.log` in the logs' folder: what a headset test needs to settle what no test here can see. What was submitted, what each stick said, what a press did. A line a second at most for
//! each thing, and only while that thing is going on. English only: it is read by whoever fixes the code.

use std::time::{Duration, Instant};

/// A line now. Never from tests: their scanners, pages and overlays would write into the person's own `vr-test.log`
/// (found 2026-11-15: a desktop test run's lines among Enzo's).
pub fn line(text: &str) {
    if cfg!(test) {
        let _ = text;
        return;
    }
    let dir = orbiters_core::paths::logs_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("vr-test.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let old = dir.join("vr-test.old.log");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, old);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        use std::io::Write;
        let _ = writeln!(file, "{} {text}", orbiters_core::now_utc_iso());
    }
}

/// Says whether it is time for another line.
#[derive(Clone, Copy, Debug, Default)]
pub struct Every {
    last: Option<Instant>,
}

impl Every {
    pub fn due(&mut self, every: Duration) -> bool {
        if self.last.is_some_and(|at| at.elapsed() < every) {
            return false;
        }
        self.last = Some(Instant::now());
        true
    }
}
