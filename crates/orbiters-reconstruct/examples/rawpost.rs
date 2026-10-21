//! Lab: a raw capture through VRChat worlds' post-processing, on the CPU. The capture simulator's PPv2 presets do not
//! run in its hidden preview scenes, so the bench applies them afterwards: every frame (strip and depth symbols
//! included, which VRChat draws before post-processing) goes through Post Processing Stack v2's chromatic aberration
//! as its Uber shader computes it (default spectral LUT, in linear light).
//!
//! ```text
//! cargo run --release --example rawpost -- <raw capture> <out folder> --aberration <intensity>
//! ```
//! `capture.json` and `groundtruth/` are copied as they are.

use anyhow::Context as _;
use orbiters_contracts::{color, qoi, raw_capture};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// Bilinear sample of a linear plane with clamping, like a GPU sampler (u, v in 0..1).
fn sample(p: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let (x, y) = ((u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32), (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = p[y0 * w + x0] * (1.0 - fx) + p[y0 * w + x1] * fx;
    let bottom = p[y1 * w + x0] * (1.0 - fx) + p[y1 * w + x1] * fx;
    top * (1.0 - fy) + bottom * fy
}

/// PPv2's default spectral LUT (red, green, blue texels, bilinear) at t.
fn spectral(t: f32) -> [f32; 3] {
    let x = (t * 3.0 - 0.5).clamp(0.0, 2.0);
    let i = (x.floor() as usize).min(1);
    let mut f = [0f32; 3];
    f[i] += 1.0 - (x - i as f32);
    f[i + 1] += x - i as f32;
    f
}

/// PPv2's chromatic aberration of a top-down RGB image (3 bytes a pixel).
fn aberrate(rgb: &[u8], w: usize, h: usize, intensity: f32) -> Vec<u8> {
    let planes: Vec<Vec<f32>> = (0..3).map(|c| (0..w * h).map(|i| color::srgb_to_linear(rgb[i * 3 + c] as f32 / 255.0)).collect()).collect();
    let mut out = vec![0u8; w * h * 3];
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            // Unity's uv has v up; the image is top-down, so flip v for the radial terms (they are symmetric anyway).
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let (cx, cy) = (2.0 * u - 1.0, 2.0 * v - 1.0);
            let d2 = cx * cx + cy * cy;
            let (dx, dy) = (-cx * d2 * 0.05 * intensity, -cy * d2 * 0.05 * intensity);
            let n = (((dx * w as f32).powi(2) + (dy * h as f32).powi(2)).sqrt() / 2.0).clamp(3.0, 16.0) as usize;
            let (mut sum, mut weight) = ([0f32; 3], [0f32; 3]);
            for i in 0..n {
                let f = spectral((i as f32 + 0.5) / n as f32);
                let (pu, pv) = (u + dx * i as f32 / n as f32, v + dy * i as f32 / n as f32);
                for c in 0..3 {
                    sum[c] += f[c] * sample(&planes[c], w, h, pu, pv);
                    weight[c] += f[c];
                }
            }
            for c in 0..3 {
                row[x * 3 + c] = color::linear_to_srgb8(sum[c] / weight[c]);
            }
        }
    });
    out
}

fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target)?;
        } else if std::fs::hard_link(e.path(), &target).is_err() {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut input, mut output, mut intensity) = (None, None, None);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--aberration" => intensity = Some(it.next().context("--aberration needs an intensity")?.parse::<f32>()?),
            _ if input.is_none() => input = Some(PathBuf::from(a)),
            _ => output = Some(PathBuf::from(a)),
        }
    }
    let usage = "usage: rawpost <raw capture> <out folder> --aberration <intensity>";
    let (input, output, intensity) = (input.context(usage)?, output.context(usage)?, intensity.context(usage)?);
    anyhow::ensure!(!output.exists(), "{} exists", output.display());
    std::fs::create_dir_all(output.join(raw_capture::FRAMES_DIR))?;
    std::fs::copy(input.join(raw_capture::RAW_FILE), output.join(raw_capture::RAW_FILE))?;
    if input.join(raw_capture::GROUND_TRUTH_DIR).is_dir() {
        copy_tree(&input.join(raw_capture::GROUND_TRUTH_DIR), &output.join(raw_capture::GROUND_TRUTH_DIR))?;
    }
    let mut frames: Vec<PathBuf> = std::fs::read_dir(input.join(raw_capture::FRAMES_DIR))?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
    frames.sort();
    let started = std::time::Instant::now();
    for (k, f) in frames.iter().enumerate() {
        let (header, rgba) = qoi::decode_rgba(&std::fs::read(f)?).map_err(|e| anyhow::anyhow!("{}: {e:?}", f.display()))?;
        let (w, h) = (header.width as usize, header.height as usize);
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let out = aberrate(&rgb, w, h, intensity);
        std::fs::write(output.join(raw_capture::FRAMES_DIR).join(f.file_name().context("frame name")?), qoi::encode(&out, w as u32, h as u32, 3))?;
        if k % 100 == 0 {
            eprintln!("{k}/{} frames, {:.0} s", frames.len(), started.elapsed().as_secs_f32());
        }
    }
    eprintln!("{} frames through chromatic aberration {intensity} in {:.0} s", frames.len(), started.elapsed().as_secs_f32());
    Ok(())
}
