//! The frame log: nobody can tell what worked without it, so it runs at every level, Off included, while VRChat has the
//! scene. It stays on this computer, is never uploaded, and is separate from the world performance statistics a person may
//! choose to share. It names no people and no world: a person's count, and a number that only says "another world".
//!
//! `fps-booster.jsonl` gets one line each five seconds, and goes to `.1` at 20 MB. `fps-frames-<date>.csv` gets each frame,
//! only during a comparison. And when a session ends, what SteamVR's compositor counted itself is logged beside what this
//! log counted: if the two disagree by more than 10%, the logger is what is wrong, and it says so.

use crate::classify::Limit;
use crate::engine::{Line, Wants};
use crate::frames::{Frame, Summary};
use crate::trial::{Kind, Side};
use crate::Level;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const LOG_NAME: &str = "fps-booster.jsonl";
const ROTATE_AT: u64 = 20 * 1024 * 1024;
/// How far real frames may be from what the compositor's own counts imply.
const SELF_CHECK_LIMIT: f32 = 0.10;

/// SteamVR's own settings at the time, read only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct SteamVrRead {
    pub disable_async: Option<bool>,
    pub motion_smoothing: Option<bool>,
    pub supersample_manual_override: Option<bool>,
    pub supersample_scale: Option<f32>,
    pub app_resolution_scale: Option<f32>,
}

/// One line of the log.
#[derive(Clone, Debug, Serialize)]
pub struct BlockLine {
    pub at: String,
    pub summary: Summary,
    pub refresh_hz: f32,
    pub level: Level,
    pub limit: Limit,
    pub line: Line,
    pub wants: Wants,
    pub lowered: u32,
    pub trial: Option<(Kind, Side)>,
    pub people: Option<u32>,
    /// A count of worlds this session: which world is not recorded.
    pub world_number: u32,
    pub reconstruction: bool,
    pub recording: bool,
    pub panel_3d: bool,
    pub cpu_total: f32,
    pub top: Vec<(String, f32)>,
    pub steamvr: SteamVrRead,
    /// Milliseconds the last line took to write: the observer is counted.
    pub log_ms: f32,
}

/// Writes the log.
pub struct Logger {
    dir: PathBuf,
    log_ms: f32,
}

impl Logger {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, log_ms: 0.0 }
    }

    pub fn log_ms(&self) -> f32 {
        self.log_ms
    }

    fn file(&self) -> PathBuf {
        self.dir.join(LOG_NAME)
    }

    /// Appends a line, going to a new file at the limit. A log that cannot be written is no reason for anything else to stop.
    pub fn write<T: Serialize>(&mut self, line: &T) {
        let began = std::time::Instant::now();
        let _ = self.try_write(line);
        self.log_ms = began.elapsed().as_secs_f32() * 1000.0;
    }

    fn try_write<T: Serialize>(&self, line: &T) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.file();
        if std::fs::metadata(&path).is_ok_and(|m| m.len() >= ROTATE_AT) {
            let mut old = path.as_os_str().to_owned();
            old.push(".1");
            std::fs::rename(&path, PathBuf::from(old))?;
        }
        let mut bytes = serde_json::to_vec(line).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        std::fs::OpenOptions::new().append(true).create(true).open(path)?.write_all(&bytes)
    }

    /// Each frame of a comparison, with which side it was on.
    pub fn write_raw(&self, frames: &[Frame], kind: Kind) -> std::io::Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let date = crate::now_text();
        let path = self.dir.join(format!("fps-frames-{}.csv", date.split('T').next().unwrap_or("")));
        let new = !path.exists();
        let mut file = std::io::BufWriter::new(std::fs::OpenOptions::new().append(true).create(true).open(path)?);
        if new {
            writeln!(
                file,
                "kind,index,time_s,presents,flags,interval_ms,app_cpu_ms,app_gpu_ms,total_gpu_ms,compositor_cpu_ms,compositor_gpu_ms,dropped,mis_presented"
            )?;
        }
        for f in frames {
            writeln!(
                file,
                "{kind:?},{},{:.4},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{}",
                f.index,
                f.time_s,
                f.presents,
                f.flags,
                f.interval_ms,
                f.app_cpu_ms(),
                f.app_gpu_ms(),
                f.total_gpu_ms,
                f.compositor_cpu_ms,
                f.compositor_gpu_ms,
                f.dropped,
                f.mis_presented
            )?;
        }
        file.flush()
    }
}

/// What SteamVR's compositor counted of a process, from the block it writes when that process leaves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CompositorStats {
    pub pid: u32,
    pub presents: u64,
    pub dropped: u64,
    pub reprojected: u64,
    pub app_cpu_ms: Option<f32>,
    pub target_hz: Option<f32>,
}

/// The "Cumulative stats for pid" blocks in `vrcompositor.txt`, oldest first. An undocumented format, read only for the
/// self-check: what does not parse is left out, and nothing depends on it.
pub fn parse_compositor(text: &str) -> Vec<CompositorStats> {
    let mut out: Vec<CompositorStats> = Vec::new();
    let mut current: Option<CompositorStats> = None;
    let body = |line: &str| line.split_once(" - ").map(|(_, b)| b.trim().to_string());
    for line in text.lines() {
        let Some(said) = body(line) else { continue };
        if let Some(pid) = said.strip_prefix("Cumulative stats for pid:") {
            if let Some(done) = current.take().filter(|c| c.presents > 0) {
                out.push(done);
            }
            let Ok(pid) = pid.trim().parse() else { continue };
            current = Some(CompositorStats { pid, presents: 0, dropped: 0, reprojected: 0, app_cpu_ms: None, target_hz: None });
            continue;
        }
        let Some(c) = current.as_mut() else { continue };
        if let Some(rest) = said.strip_prefix("Total.") {
            let numbers: Vec<u64> = rest.split(|ch: char| !ch.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
            if let [presents, dropped, reprojected] = numbers[..] {
                (c.presents, c.dropped, c.reprojected) = (presents, dropped, reprojected);
            }
        } else if let Some(rest) = said.strip_prefix("Game Info.") {
            let number_after = |word: &str| rest.split(word).nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<f32>().ok());
            c.target_hz = number_after("FPS Average Target");
            c.app_cpu_ms = number_after("ApplicationTime CPU:");
        }
    }
    if let Some(done) = current.filter(|c| c.presents > 0) {
        out.push(done);
    }
    out
}

/// The log's figure against the compositor's own.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SelfCheck {
    pub logged_real_fps: f32,
    pub implied_real_fps: f32,
    pub compositor_reprojected_share: f32,
    /// Off by more than the limit: the logger is what is wrong, not the frames.
    pub logger_is_wrong: bool,
}

/// `real frames = refresh × (1 − reprojected share)`.
pub fn self_check(logged_real_fps: f32, refresh_hz: f32, stats: &CompositorStats) -> Option<SelfCheck> {
    if stats.presents == 0 || refresh_hz <= 1.0 {
        return None;
    }
    let share = stats.reprojected as f32 / stats.presents as f32;
    let implied = refresh_hz * (1.0 - share);
    if implied <= 0.0 {
        return None;
    }
    Some(SelfCheck {
        logged_real_fps,
        implied_real_fps: implied,
        compositor_reprojected_share: share,
        logger_is_wrong: (logged_real_fps - implied).abs() / implied > SELF_CHECK_LIMIT,
    })
}

/// Where the logs are, for "Open the log folder".
pub fn is_log_dir(dir: &Path) -> bool {
    dir.join(LOG_NAME).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::tests::Scratch;

    const LOG: &str = "Sat Nov 07 2026 18:47:27.458 [Info] - something else\n\
Sat Nov 07 2026 19:20:33.280 [Info] - Cumulative stats for pid: 70532\n\
Sat Nov 07 2026 19:20:33.280 [Info] - Total..................112049 presents.    0 dropped. 55210 reprojected\n\
Sat Nov 07 2026 19:20:33.280 [Info] - Startup................    31 presents.    0 dropped.    0 reprojected\n\
Sat Nov 07 2026 19:20:33.280 [Info] - Loading...  1 total....  1002 presents.    0 dropped.   41 reprojected\n\
Sat Nov 07 2026 19:20:33.280 [Info] - Compositor Time........CPU: 0.771ms / GPU: 0.508ms\n\
Sat Nov 07 2026 19:20:33.280 [Info] - Game Info..............FPS Average Target 72  ApplicationTime CPU: 18.98\n\
Sat Nov 07 2026 19:20:33.280 [Info] - ######################################################################\n";

    #[test]
    fn the_compositors_own_block_is_read() {
        let got = parse_compositor(LOG);
        assert_eq!(got.len(), 1);
        let c = got[0];
        assert_eq!((c.pid, c.presents, c.dropped, c.reprojected), (70532, 112049, 0, 55210));
        assert_eq!((c.target_hz, c.app_cpu_ms), (Some(72.0), Some(18.98)));
    }

    #[test]
    fn what_does_not_parse_is_left_out() {
        assert!(parse_compositor("nothing of the kind\n - Total.... 1 presents").is_empty());
        assert!(parse_compositor(" - Cumulative stats for pid: nope\n").is_empty());
    }

    #[test]
    fn the_self_check_catches_a_logger_that_disagrees() {
        let stats = CompositorStats { pid: 1, presents: 1000, dropped: 0, reprojected: 500, app_cpu_ms: None, target_hz: None };
        let good = self_check(36.0, 72.0, &stats).unwrap();
        assert!(!good.logger_is_wrong);
        assert_eq!(good.implied_real_fps, 36.0);
        assert!(self_check(50.0, 72.0, &stats).unwrap().logger_is_wrong);
        assert!(self_check(36.0, 72.0, &CompositorStats { presents: 0, ..stats }).is_none());
    }

    #[test]
    fn a_line_is_one_line_of_json_and_the_file_rotates() {
        let dir = Scratch::new("log");
        let mut logger = Logger::new(dir.0.clone());
        logger.write(&serde_json::json!({ "a": 1 }));
        logger.write(&serde_json::json!({ "a": 2 }));
        let text = std::fs::read_to_string(dir.0.join(LOG_NAME)).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(serde_json::from_str::<serde_json::Value>(text.lines().next().unwrap()).is_ok());
        assert!(is_log_dir(&dir.0));
        let file = std::fs::OpenOptions::new().write(true).open(dir.0.join(LOG_NAME)).unwrap();
        file.set_len(ROTATE_AT).unwrap();
        drop(file);
        logger.write(&serde_json::json!({ "a": 3 }));
        assert_eq!(std::fs::read_to_string(dir.0.join(LOG_NAME)).unwrap().lines().count(), 1);
        assert!(dir.0.join(format!("{LOG_NAME}.1")).exists());
    }

    /// No names and no worlds in a line: the fields say what is recorded, and this is what would catch one being added.
    #[test]
    fn a_line_names_nobody_and_no_world() {
        let line = BlockLine {
            at: "t".into(),
            summary: Summary::default(),
            refresh_hz: 72.0,
            level: Level::Off,
            limit: Limit::Fine,
            line: Line::Watching,
            wants: Wants::default(),
            lowered: 0,
            trial: None,
            people: Some(12),
            world_number: 3,
            reconstruction: false,
            recording: false,
            panel_3d: false,
            cpu_total: 0.4,
            top: vec![("chrome".into(), 1.5)],
            steamvr: SteamVrRead::default(),
            log_ms: 0.1,
        };
        let text = serde_json::to_string(&line).unwrap();
        assert!(!text.contains("usr_") && !text.contains("wrld_"));
        assert!(text.contains("\"people\":12"));
    }

    #[test]
    fn raw_frames_are_written_for_a_comparison() {
        let dir = Scratch::new("raw");
        let logger = Logger::new(dir.0.clone());
        logger.write_raw(&[Frame { index: 5, presents: 2, ..Frame::default() }], Kind::Programs).unwrap();
        logger.write_raw(&[Frame { index: 6, ..Frame::default() }], Kind::Programs).unwrap();
        let file = std::fs::read_dir(&dir.0).unwrap().flatten().next().unwrap().path();
        let text = std::fs::read_to_string(file).unwrap();
        assert_eq!(text.lines().count(), 3, "a head and two frames");
        assert!(text.lines().nth(1).unwrap().starts_with("Programs,5,"));
    }
}
