//! What the fingers on N4N0's screen did, in `n4n0-pointer.log` beside the app's other logs. The beam is the avatar's own
//! drawing and shows whatever the desktop reads, so a pointer can show and press nothing; this says why. It writes a line when
//! what a hand's senders say changes (at most five a second a hand), one for each trigger pull and press with where it went
//! or which guard stopped it, one when the menu opens, and every five seconds how the readings went while the menu is open.
//! A live test is settled by reading it: `silent` all the time for one sender is VRChat's broad phase dropping it (a screen
//! of version 15 or older with people near), `mixed` is readings of two frames, and no line for a pull is the trigger not
//! reaching the app (SteamVR's actions).

use orbiters_n4n0_core::menu::touch::{Geometry, Guard, Reading, Sender};
use std::time::{Duration, Instant};

const SUMMARY_EVERY: Duration = Duration::from_secs(5);
const LINE_AT_MOST: Duration = Duration::from_millis(200);
const HANDS: [&str; 2] = ["left", "right"];

/// A hand's readings since the last summary.
#[derive(Clone, Copy, Default)]
struct Counts {
    readings: u32,
    pointing: u32,
    kept: u32,
    tip_silent: u32,
    aim_silent: u32,
    mixed: u32,
    other: u32,
}

#[derive(Default)]
pub(crate) struct PointerLog {
    counts: [Counts; 2],
    said: [Option<(Sender, Sender, Option<Guard>, bool)>; 2],
    said_at: [Option<Instant>; 2],
    summary_at: Option<Instant>,
    /// What was written, while testing.
    #[cfg(test)]
    pub lines: Vec<String>,
}

impl PointerLog {
    fn write(&mut self, line: &str) {
        #[cfg(test)]
        self.lines.push(line.to_string());
        #[cfg(not(test))]
        {
            use std::io::Write;
            let dir = orbiters_core::paths::logs_dir();
            if std::fs::create_dir_all(&dir).is_err() {
                return;
            }
            let path = dir.join("n4n0-pointer.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
                let _ = std::fs::rename(&path, dir.join("n4n0-pointer.old.log"));
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{} {line}", orbiters_core::now_utc_iso());
            }
        }
    }

    /// The menu opened on the avatar's screen.
    pub fn opened(&mut self, page: &str, geometry: &Geometry, pointing: bool) {
        self.counts = Default::default();
        self.said = [None; 2];
        self.summary_at = None;
        self.write(&format!(
            "menu open on {page}: pointing {pointing}, receivers' reach {:.3}, sender {:.4}, aim {:.3} (screen units)", // i18n: not shown
            geometry.reach, geometry.sender, geometry.aim
        ));
    }

    /// What a hand's senders say now.
    pub fn reading(&mut self, hand: usize, r: &Reading, now: Instant) {
        let c = &mut self.counts[hand];
        c.readings += 1;
        match (r.point, r.kept) {
            (Ok(_), false) => c.pointing += 1,
            (Ok(_), true) => c.kept += 1,
            (Err(Guard::Tip(Sender::Silent)), _) => c.tip_silent += 1,
            (Err(Guard::Aim(Sender::Silent)), _) => c.aim_silent += 1,
            (Err(Guard::Tip(Sender::Mixed) | Guard::Aim(Sender::Mixed) | Guard::WrongLength), _) => c.mixed += 1,
            (Err(_), _) => c.other += 1,
        }
        let state = (r.tip, r.aim, r.point.err(), r.kept);
        if self.said[hand] != Some(state) && self.said_at[hand].is_none_or(|at| now.saturating_duration_since(at) >= LINE_AT_MOST) {
            self.said[hand] = Some(state);
            self.said_at[hand] = Some(now);
            let point = match r.point {
                Ok((x, y)) => format!("points at {x:.0}, {y:.0}"), // i18n: not shown
                Err(g) => format!("no point: {}", g.name()), // i18n: not shown
            };
            let line = format!("{}: tip {}, aim {}, {point}{}", HANDS[hand], r.tip.name(), r.aim.name(), if r.kept { " (on the last direction)" } else { "" }); // i18n: not shown
            self.write(&line);
        }
        self.summary(now);
    }

    /// A hand's trigger came down: what it did, or why it did nothing.
    pub fn pull(&mut self, hand: usize, what: &str) {
        let line = format!("{} trigger: {what}", HANDS[hand]);
        self.write(&line);
    }

    /// A press came: by whom, where, on what.
    pub fn press(&mut self, who: &str, x: f32, y: f32, what: &str) {
        let line = format!("{who} press at {x:.0}, {y:.0}: {what}"); // i18n: not shown
        self.write(&line);
    }

    /// Something worth a line of its own.
    pub fn note(&mut self, line: &str) {
        self.write(line);
    }

    fn summary(&mut self, now: Instant) {
        let at = *self.summary_at.get_or_insert(now);
        if now.saturating_duration_since(at) < SUMMARY_EVERY {
            return;
        }
        self.summary_at = Some(now);
        for hand in 0..2 {
            let c = std::mem::take(&mut self.counts[hand]);
            if c.readings > 0 {
                let share = |n: u32| n * 100 / c.readings;
                let line = format!(
                    "{}: {} readings, {} % pointing, {} % on the last direction, tip silent {} %, aim silent {} %, mixed {} %, other {} %", // i18n: not shown
                    HANDS[hand],
                    c.readings,
                    share(c.pointing),
                    share(c.kept),
                    share(c.tip_silent),
                    share(c.aim_silent),
                    share(c.mixed),
                    share(c.other)
                );
                self.write(&line);
            }
        }
    }
}
