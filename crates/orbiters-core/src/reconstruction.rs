//! Reconstruction of a session (`orbiters-reconstruct`): where its result lives in the session folder, the report
//! it leaves there, and the JSON lines it prints for the app while it works.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The result's folder inside a session folder (replaced as a whole when a run finishes).
pub const DIR: &str = "reconstruction";
/// Where a run writes before it replaces [`DIR`].
pub const PARTIAL_DIR: &str = "reconstruction.partial";
/// Where the previous result waits while a finished run's moves into [`DIR`]; a swap stopped halfway (the worker
/// killed) leaves the previous result here, put back by the next run.
pub const OLD_DIR: &str = "reconstruction.old";
pub const PLY_FILE: &str = "splats.ply";
/// The sky behind the splats, where training learnt it as a map (orbiters_reconstruct::sky::SkyMap): an
/// equirectangular 8-bit sRGB PNG twice as wide as high, longitude `atan2(x, z)` from −180° at the left edge, elevation
/// from +90° at the top row, by directions in the Unity world (the PLY's right-handed world with Z negated). Viewers
/// draw it wherever the splats leave the view open (contracts Docs/session-bundle.md).
pub const SKY_FILE: &str = "sky.png";
pub const REPORT_FILE: &str = "report.json";
/// A render of the first held-out keyframe (QOI, RGB). With splat edits ([`EDIT_FILE`]) it is drawn again without the
/// splats they remove, so that every picture of the capture (its card, its uploads, a drag out of the app) matches its
/// shared files; the worker's own render waits aside ([`trained_preview_file`]) until the edits are undone.
pub const PREVIEW_FILE: &str = "preview.qoi";

/// Where the worker's preview of the PLY whose hash (orbiters_reconstruct::edit::Identity) is `ply_hash` waits while
/// [`PREVIEW_FILE`] shows the edited splats: `preview.trained-<hash>.qoi`, so that one kept for an earlier
/// reconstruction is never put back over a newer one's.
pub fn trained_preview_file(ply_hash: u64) -> String {
    format!("preview.trained-{ply_hash:016x}.qoi")
}

/// Whether `name` is a file [`trained_preview_file`] names.
pub fn is_trained_preview_file(name: &str) -> bool {
    name.strip_prefix("preview.trained-").and_then(|r| r.strip_suffix(".qoi")).is_some_and(|h| h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()))
}
/// The splats hidden or deleted in the 3D view (orbiters_reconstruct::edit): beside [`PLY_FILE`], which is never
/// rewritten, and applied wherever the splats leave the app (the VRChat trees, the web preview, the pictures). A new
/// reconstruction replaces the folder and its edits with it: they name splats of the PLY they were made on.
pub const EDIT_FILE: &str = "splat-edits.bin";
/// Where a paused run stands (orbiters_reconstruct::checkpoint): one file beside the result, in the session folder,
/// deleted when the run ends or is stopped. What is written to replace it goes beside it, under this name and more.
pub const CHECKPOINT_FILE: &str = "reconstruction.checkpoint";
/// Renders of every held-out keyframe, named after the keyframe (`000004.qoi`).
pub const HELD_OUT_DIR: &str = "held-out";

/// `report.json`: what a finished run produced. A field missing (a report from another version) is left at its
/// default rather than the report being unread.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub gaussians: usize,
    pub iterations: u32,
    pub seconds: f32,
    /// Mean PSNR (dB) of the renders of keyframes the training never saw.
    pub psnr_held_out: Option<f32>,
    /// Mean PSNR (dB) over a sample of the training keyframes.
    pub psnr_train: f32,
    pub held_out_keyframes: Vec<i32>,
    pub width: u32,
    pub height: u32,
    pub adapter: String,
    pub created_utc: String,
    /// Keyframes and depth frames the session holds, and how many of them trained.
    pub keyframes_total: usize,
    pub keyframes_used: usize,
    pub depth_frames_total: usize,
    pub depth_frames_used: usize,
    /// The world's chromatic aberration undone in the keyframes (PPv2 intensity), if any.
    pub chromatic_aberration: Option<f32>,
    /// Depth samples the frames carry, and of those unknown after validation (the decoder's included).
    pub depth_samples: Option<usize>,
    pub depth_unknown: Option<usize>,
    /// The wearer of the capture kit, taken out of this result because they should not be in it and their avatar could not
    /// hide itself. `None`: nothing of the kind was done.
    #[serde(default)]
    pub wearer: Option<WearerOut>,
    /// They should have gone and nothing of them was found to take out.
    #[serde(default)]
    pub wearer_not_found: bool,
}

/// What taking the wearer out did: numbers for the gallery's line and for whoever checks it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WearerOut {
    /// Share of the views' pixels that showed them.
    pub pixel_share: f32,
    /// Views (keyframes and rig faces) they were in, of all.
    pub views: usize,
    pub views_total: usize,
    /// Depth samples on them, of those that hit something.
    pub depth_samples: usize,
    pub depth_hits: usize,
    /// Gaussians that still lay where they stood at the end of training, and floor patched under their feet (a guess, from
    /// the floor's colour around).
    pub gaussians_left: usize,
    pub floor_patched: usize,
}

/// Where a run puts its result (the worker writes here). Readers take [`result_dir`].
pub fn dir(session: &Path) -> PathBuf {
    session.join(DIR)
}

/// Where a session's latest result is: [`DIR`], or [`OLD_DIR`] while a swap stopped halfway left it there (until the
/// next run puts it back).
pub fn result_dir(session: &Path) -> PathBuf {
    let current = dir(session);
    if current.is_dir() {
        return current;
    }
    let old = session.join(OLD_DIR);
    if old.is_dir() {
        old
    } else {
        current
    }
}

/// The latest finished run's report: what fits of it ([`crate::json_lenient`]), a number written as `null` (NaN)
/// costing that field only; `None` without one, or when it is not JSON.
pub fn read_report(session: &Path) -> Option<Report> {
    let text = crate::read_text(&result_dir(session).join(REPORT_FILE)).ok()??;
    let (report, dropped) = crate::json_lenient::<Report>(&text);
    dropped.iter().all(|d| d != "(not JSON)").then_some(report)
}

/// A preview read without a report to give its size is at most this large (4K).
const PREVIEW_PIXELS: u64 = 3840 * 2160;

/// The preview render as top-down RGBA: (width, height, pixels). It is rendered at the training's size, which the
/// report gives: a file of another size is not the preview, and gets no memory.
pub fn read_preview(session: &Path) -> Option<(u32, u32, Vec<u8>)> {
    use crate::contracts::qoi;
    let data = std::fs::read(result_dir(session).join(PREVIEW_FILE)).ok()?;
    let decoded = match read_report(session).filter(|r| r.width > 0 && r.height > 0) {
        Some(report) => qoi::decode_rgba_sized(&data, report.width, report.height),
        None => qoi::decode_rgba_max(&data, PREVIEW_PIXELS),
    };
    let (h, rgba) = decoded.ok()?;
    Some((h.width, h.height, rgba))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Decoding keyframes and depth frames (`progress` 0..1).
    Loading,
    /// Seeding Gaussians from the depth samples.
    Seeding,
    Training,
    /// Writing the result.
    Saving,
}

/// One line of the worker's stdout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Stage {
        stage: Stage,
        #[serde(default)]
        progress: f32,
    },
    Progress {
        iteration: u32,
        iterations: u32,
        gaussians: usize,
        /// Tile entries of the last step (what a step costs).
        intersections: u32,
        /// Held-out PSNR, measured every thousand steps.
        psnr: Option<f32>,
        seconds: f32,
    },
    /// Asked to pause, it goes on from where the checkpoint of an earlier pause left it (or, if that checkpoint cannot
    /// be read after all, starts over: [`Stage::Seeding`] follows).
    Resumed {
        iteration: u32,
        iterations: u32,
        gaussians: usize,
        psnr: Option<f32>,
        seconds: f32,
    },
    /// A pause has taken it off the GPU: its memory is free, what is left is writing the checkpoint.
    Pausing,
    /// It stopped for a pause, and exits. `kept`: a checkpoint stands to go on from (else it starts over, which costs
    /// little before its steps have begun); `message` says why not when that loses progress.
    Paused {
        kept: bool,
        #[serde(default)]
        message: Option<String>,
    },
    Done {
        report: Report,
    },
    Error {
        message: String,
    },
}

impl Event {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads one line of the worker's output: a `Done` whose report holds a number written as `null` (NaN) keeps the
    /// rest of it, rather than the run's end being lost.
    pub fn from_line(line: &str) -> Option<Event> {
        if let Ok(event) = serde_json::from_str(line) {
            return Some(event);
        }
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        (value["event"] == "done").then(|| Event::Done { report: crate::json_lenient::<Report>(&value["report"].to_string()).0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_as_tagged_lines() {
        let e = Event::Progress { iteration: 50, iterations: 9000, gaussians: 97_448, intersections: 567_127, psnr: None, seconds: 1.5 };
        let line = e.to_line();
        assert!(line.starts_with("{\"event\":\"progress\""), "{line}");
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), e);
        let s: Event = serde_json::from_str(r#"{"event":"stage","stage":"seeding"}"#).unwrap();
        for e in [Event::Pausing, Event::Paused { kept: true, message: None }, Event::Resumed { iteration: 5, iterations: 9, gaussians: 7, psnr: None, seconds: 1.0 }] {
            assert_eq!(Event::from_line(&e.to_line()), Some(e));
        }
        assert_eq!(s, Event::Stage { stage: Stage::Seeding, progress: 0.0 });
        // A report with a number written as null (NaN) still ends the run.
        let done = Event::Done { report: Report { gaussians: 5, psnr_train: f32::NAN, ..Default::default() } }.to_line();
        assert!(matches!(Event::from_line(&done), Some(Event::Done { report }) if report.gaussians == 5 && report.psnr_train == 0.0));
    }

    #[test]
    fn a_preview_is_read_at_the_trainings_size_only() {
        use crate::contracts::qoi;
        let session = std::env::temp_dir().join(format!("orbiters-preview-{}", std::process::id()));
        let out = dir(&session);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(PREVIEW_FILE), qoi::encode(&[9u8; 8 * 8 * 3], 8, 8, 3)).unwrap();
        assert_eq!(read_preview(&session).map(|(w, h, _)| (w, h)), Some((8, 8)), "no report: any size up to 4K");
        let report = Report { width: 4, height: 2, ..Default::default() };
        std::fs::write(out.join(REPORT_FILE), serde_json::to_vec(&report).unwrap()).unwrap();
        assert!(read_preview(&session).is_none(), "another size than the training's");
        std::fs::write(out.join(PREVIEW_FILE), qoi::encode(&[9u8; 4 * 2 * 3], 4, 2, 3)).unwrap();
        assert_eq!(read_preview(&session).map(|(w, h, p)| (w, h, p.len())), Some((4, 2, 32)));
        std::fs::remove_dir_all(&session).ok();
    }

    #[test]
    fn a_result_left_aside_by_a_swap_stopped_halfway_is_read() {
        let session = std::env::temp_dir().join(format!("orbiters-result-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&session);
        let old = session.join(OLD_DIR);
        std::fs::create_dir_all(&old).unwrap();
        // A report from a worker of this version: its counts are read too.
        let written = r#"{"gaussians":7,"keyframes_total":40,"keyframes_used":38,"chromatic_aberration":0.4,"depth_samples":900,"depth_unknown":12}"#;
        std::fs::write(old.join(REPORT_FILE), written).unwrap();
        assert_eq!(result_dir(&session), old);
        let report = read_report(&session).unwrap();
        assert_eq!((report.gaussians, report.keyframes_used, report.chromatic_aberration, report.depth_unknown), (7, 38, Some(0.4), Some(12)));
        // Once the result is in place again, that is the one.
        std::fs::create_dir_all(dir(&session)).unwrap();
        std::fs::write(dir(&session).join(REPORT_FILE), r#"{"gaussians":9}"#).unwrap();
        assert_eq!(read_report(&session).map(|r| (r.gaussians, r.keyframes_total)), Some((9, 0)));
        std::fs::remove_dir_all(&session).ok();
    }
}
