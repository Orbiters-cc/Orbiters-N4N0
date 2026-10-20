//! `orbiters-capture verify <raw capture>`: decodes every frame of a raw capture and compares it with its
//! ground truth (poses.jsonl and true depth). This is how a Mixer simulator capture proves that the shader,
//! the C# encoder and the Rust decoder agree.

use flate2::read::DeflateDecoder;
use orbiters_contracts::raw_capture::{self, gt_depth, GroundTruthPose};
use orbiters_contracts::{depth, qoi, strip};
use serde::Serialize;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Default, Serialize)]
pub struct VerifyReport {
    pub frames: u32,
    pub strip_ok: u32,
    pub strip_failed: u32,
    pub flipped: u32,
    pub pose_exact: u32,
    pub pose_close: u32,
    pub pose_wrong: u32,
    pub corrected_bits_max: u32,
    pub depth_frames: u32,
    pub depth_samples: u64,
    pub depth_valid_both: u64,
    pub depth_missing: u64,
    pub depth_spurious: u64,
    pub depth_error_median_mm: f32,
    pub depth_error_p95_mm: f32,
    pub depth_relative_p95: f32,
    pub tiles_rejected: u64,
    pub first_failure: Option<String>,
}

fn load_gt_depth(path: &Path) -> anyhow::Result<(u32, u32, Vec<f32>)> {
    let data = std::fs::read(path)?;
    let (w, h, offset) = gt_depth::read_header(&data).ok_or_else(|| anyhow::anyhow!("{}: bad header", path.display()))?;
    let mut raw = Vec::with_capacity((w * h * 2) as usize);
    DeflateDecoder::new(&data[offset..]).read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() >= (w * h * 2) as usize, "{}: truncated", path.display());
    Ok((w, h, raw.as_chunks::<2>().0.iter().map(|b| gt_depth::half_to_f32(u16::from_le_bytes(*b))).collect()))
}

pub fn verify(dir: &Path, max_frames: Option<u32>) -> anyhow::Result<VerifyReport> {
    let meta: raw_capture::RawCapture = serde_json::from_str(&std::fs::read_to_string(dir.join(raw_capture::RAW_FILE))?)?;
    let poses: Vec<GroundTruthPose> = std::fs::read_to_string(dir.join(raw_capture::POSES_FILE))
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default();
    let mut report = VerifyReport::default();
    let mut errors_mm: Vec<f32> = Vec::new();
    let mut relative: Vec<f32> = Vec::new();
    let n = meta.frame_count.min(max_frames.unwrap_or(u32::MAX));
    for i in 0..n {
        let path = dir.join(raw_capture::FRAMES_DIR).join(raw_capture::frame_file(i));
        let (h, rgba) = qoi::decode_rgba(&std::fs::read(&path)?)?;
        let img = orbiters_contracts::image::RgbaImage::from_raw(h.width, h.height, rgba);
        report.frames += 1;
        let decoded = match strip::decode(&img.view()) {
            Ok(d) => d,
            Err(e) => {
                report.strip_failed += 1;
                report.first_failure.get_or_insert_with(|| format!("frame {i}: {e}"));
                continue;
            }
        };
        report.strip_ok += 1;
        report.flipped += decoded.flipped as u32;
        report.corrected_bits_max = report.corrected_bits_max.max(decoded.corrected_bits);
        if let Some(gt) = poses.iter().find(|p| p.index == i) {
            let exact = decoded.payload.camera_to_world == gt.camera_to_world && decoded.payload.projection == gt.projection;
            let close = decoded.payload.camera_to_world.iter().zip(gt.camera_to_world.iter()).all(|(a, b)| (a - b).abs() <= 1e-4 * b.abs().max(1.0));
            if exact {
                report.pose_exact += 1;
            } else if close {
                report.pose_close += 1;
            } else {
                report.pose_wrong += 1;
                report.first_failure.get_or_insert_with(|| format!("frame {i}: pose differs from ground truth"));
            }
        }
        if decoded.payload.is_depth_frame() {
            report.depth_frames += 1;
            let Some(dd) = depth::decode(&img.view(), decoded.flipped) else { continue };
            report.tiles_rejected += dd.tiles_rejected as u64;
            let gt_path = dir.join(raw_capture::GT_DEPTH_DIR).join(raw_capture::gt_depth_file(i));
            let Ok((gw, gh, gt)) = load_gt_depth(&gt_path) else { continue };
            if gw != h.width || gh != h.height {
                continue;
            }
            for gy in 0..dd.grid_height {
                for gx in 0..dd.grid_width {
                    if !dd.layout.tile_used(gx / depth::SAMPLES_X, gy / depth::SAMPLES_Y) {
                        continue;
                    }
                    report.depth_samples += 1;
                    let (x, y) = dd.layout.grid_pixel(gx, gy);
                    let truth = gt[(y * gw + x) as usize];
                    let truth_valid = truth > 0.0 && depth::quantize(truth) != 0;
                    match (depth::dequantize(dd.code(gx, gy)), truth_valid) {
                        (Some(z), true) => {
                            report.depth_valid_both += 1;
                            errors_mm.push((z - truth).abs() * 1000.0);
                            relative.push((z - truth).abs() / truth);
                        }
                        (None, true) => report.depth_missing += 1,
                        (Some(_), false) => report.depth_spurious += 1,
                        (None, false) => {}
                    }
                }
            }
        }
    }
    let pct = |v: &mut Vec<f32>, q: f32| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f32 * q) as usize]
    };
    report.depth_error_median_mm = pct(&mut errors_mm, 0.5);
    report.depth_error_p95_mm = pct(&mut errors_mm, 0.95);
    report.depth_relative_p95 = pct(&mut relative, 0.95);
    Ok(report)
}
