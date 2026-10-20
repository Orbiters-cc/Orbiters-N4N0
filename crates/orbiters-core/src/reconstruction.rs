//! Reconstruction of a session (`orbiters-reconstruct`): where its result lives in the session folder, the report
//! it leaves there, and the JSON lines it prints for the app while it works.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The result's folder inside a session folder (replaced as a whole when a run finishes).
pub const DIR: &str = "reconstruction";
/// Where a run writes before it replaces [`DIR`].
pub const PARTIAL_DIR: &str = "reconstruction.partial";
pub const PLY_FILE: &str = "splats.ply";
pub const REPORT_FILE: &str = "report.json";
/// A render of the first held-out keyframe (QOI, RGB).
pub const PREVIEW_FILE: &str = "preview.qoi";
/// Renders of every held-out keyframe, named after the keyframe (`000004.qoi`).
pub const HELD_OUT_DIR: &str = "held-out";

/// `report.json`: what a finished run produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
}

pub fn dir(session: &Path) -> PathBuf {
    session.join(DIR)
}

pub fn read_report(session: &Path) -> Option<Report> {
    serde_json::from_str(&std::fs::read_to_string(dir(session).join(REPORT_FILE)).ok()?).ok()
}

/// The preview render as top-down RGBA: (width, height, pixels).
pub fn read_preview(session: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let data = std::fs::read(dir(session).join(PREVIEW_FILE)).ok()?;
    let (h, rgba) = crate::contracts::qoi::decode_rgba(&data).ok()?;
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
        assert_eq!(s, Event::Stage { stage: Stage::Seeding, progress: 0.0 });
    }
}
