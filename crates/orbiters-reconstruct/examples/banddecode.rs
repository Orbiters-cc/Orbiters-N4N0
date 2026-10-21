//! Lab: the capture's symbol decoder on real frames of worlds with chromatic aberration. The skeleton band in every
//! colour keyframe uses the depth frames' tiles (pilot row, 4 grey levels in 3-px cells) and carries CRCs, so decoding
//! it from the stored keyframes tells how a decoder variant does on real post-processing: the capture's decoder (mean
//! of R, G, B), or one channel alone (copied to all three before the same decoder), optionally with the whole picture
//! shifted by a few pixels (the best shift per frame).
//!
//! ```text
//! cargo run --release --example banddecode -- <session> [every-nth-keyframe] [max shift px]
//! ```

use anyhow::Context as _;
use orbiters_contracts::frame_record::record_flags;
use orbiters_contracts::image::{ImageView, PixelOrder};
use orbiters_contracts::{qoi, skeleton};
use orbiters_core::bundle;
use rayon::prelude::*;
use std::path::PathBuf;

#[derive(Clone, Copy)]
enum Variant {
    Capture,
    Channel(usize),
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: banddecode <session> [every] [max shift]")?);
    let every: usize = it.next().map(|s| s.parse()).transpose()?.unwrap_or(1);
    let max_shift: i64 = it.next().map(|s| s.parse()).transpose()?.unwrap_or(0);
    let manifest = bundle::read_manifest(&session)?;
    let records = bundle::read_records(&session)?;
    let keyframes: Vec<i32> = records.iter().filter(|r| r.flags & record_flags::KEYFRAME != 0 && r.keyframe_index >= 0).map(|r| r.keyframe_index).collect();
    println!("{}: {} keyframes, skeleton frames decoded during the capture: {}", session.display(), keyframes.len(), manifest.performance.skeleton_frames);
    let variants = [("capture (mean of R, G, B)", Variant::Capture), ("red only", Variant::Channel(0)), ("green only", Variant::Channel(1)), ("blue only", Variant::Channel(2))];
    let picked: Vec<i32> = keyframes.iter().copied().step_by(every.max(1)).collect();
    let frames: Vec<(u32, u32, Vec<u8>)> = picked
        .par_iter()
        .map(|&k| {
            let path = session.join("keyframes").join(bundle::keyframe_file(k));
            let data = std::fs::read(&path).unwrap_or_default();
            let (head, rgba) = qoi::decode_rgba(&data).map_err(|e| anyhow::anyhow!("{}: {e:?}", path.display())).unwrap();
            (head.width, head.height, rgba)
        })
        .collect();
    for (name, v) in variants {
        for shift in [0i64, max_shift] {
            let results: Vec<(bool, usize, u32)> = frames
                .par_iter()
                .map(|(w, h, rgba)| {
                    let (w, h) = (*w as usize, *h as usize);
                    // Only the band's rows matter (the rest stays black).
                    let first = skeleton::BandLayout::new(w as u32, h as u32).map_or(0, |l| (l.y0 - l.cell) as usize);
                    let mut best = (false, 0usize, 0u32);
                    for dy in -shift..=shift {
                        for dx in -shift..=shift {
                            let mut img = vec![0u8; w * h * 4];
                            for y in first..h {
                                for x in 0..w {
                                    let (sx, sy) = ((x as i64 + dx).clamp(0, w as i64 - 1) as usize, (y as i64 + dy).clamp(0, h as i64 - 1) as usize);
                                    let s = &rgba[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4];
                                    let o = (y * w + x) * 4;
                                    match v {
                                        Variant::Capture => img[o..o + 4].copy_from_slice(s),
                                        Variant::Channel(c) => {
                                            img[o..o + 3].fill(s[c]);
                                            img[o + 3] = 255;
                                        }
                                    }
                                }
                            }
                            let view = ImageView::new(w as u32, h as u32, w * 4, &img, PixelOrder::Rgba);
                            let Some(d) = skeleton::decode(&view, false) else { continue };
                            let (ok, bones) = match &d.result {
                                Ok(s) => (true, s.bones.iter().filter(|b| b.is_some()).count()),
                                Err(_) => (false, 0),
                            };
                            if (ok, bones, d.tiles_read) > best {
                                best = (ok, bones, d.tiles_read);
                            }
                        }
                    }
                    best
                })
                .collect();
            let n = results.len().max(1) as f32;
            let headers = results.iter().filter(|r| r.0).count();
            let bones: usize = results.iter().map(|r| r.1).sum();
            let tiles: u32 = results.iter().map(|r| r.2).sum();
            println!(
                "  {name:<26} shift ±{shift}: band read in {headers} of {} keyframes ({:.0} %), {:.1} bones per keyframe, {:.1} of 30 tiles read",
                results.len(),
                100.0 * headers as f32 / n,
                bones as f32 / n,
                tiles as f32 / n
            );
            if max_shift == 0 {
                break;
            }
        }
    }
    Ok(())
}
