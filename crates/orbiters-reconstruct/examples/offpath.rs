//! Lab: scores a splat PLY on ground-truth renders from viewpoints a capture never had (the off-path benchmark of
//! Mixer's capture simulator: other heights, sideways, looking down or up). `views.jsonl` in the views folder lists
//! them, one JSON object a line: `file` (an sRGB PNG), `depth` (optional: true eye depth in metres, f32 little-endian,
//! top row first, 0 where nothing was drawn), `group`, and the strip's `camera_to_world` (Unity, row-major 3×4) and
//! `projection` (m00, m11, m02, m12) with the image's `width` and `height`.
//!
//! ```text
//! cargo run --release --example offpath -- <views folder> <splats.ply> [--session <dir>] [--json <out.json>]
//!     [--renders <dir>]
//! ```
//! Per group and over all views: PSNR, SSIM, PSNR over scene pixels (with ground-truth depth, the sky left out) and
//! the depth error there (median relative error, share within 5 %). With `--session` (the capture the PLY was trained
//! on), also the PSNR over covered pixels: those whose surface point one of the capture's depth samples saw (within
//! about 1.5 % of that sample's distance), and what share of each view is covered. What a reconstruction shows where
//! the capture never looked is a matter of coverage; the covered score measures how it renders what was seen.
//! `--renders` writes render | truth side by side.

use anyhow::Context as _;
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::{dataset, gpu, metrics, ply, train};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct View {
    file: String,
    #[serde(default)]
    depth: Option<String>,
    group: String,
    camera_to_world: [f32; 12],
    projection: [f32; 4],
    width: u32,
    height: u32,
}

fn read_png(path: &Path) -> anyhow::Result<(Vec<u32>, u32, u32)> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("png size")?];
    let info = reader.next_frame(&mut buf)?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => anyhow::bail!("{}: unsupported PNG colour type {other:?}", path.display()),
    };
    anyhow::ensure!(info.bit_depth == png::BitDepth::Eight, "{}: 8-bit PNGs only", path.display());
    let px = buf[..info.buffer_size()].chunks_exact(channels).map(|c| c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 | 255 << 24).collect();
    Ok((px, info.width, info.height))
}

fn write_side_by_side(path: &Path, img: &[[f32; 4]], truth: &[u32], w: u32, h: u32) -> anyhow::Result<()> {
    let (w, h) = (w as usize, h as usize);
    let mut rgb = vec![0u8; 2 * w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let k = y * w + x;
            for c in 0..3 {
                rgb[(y * 2 * w + x) * 3 + c] = (img[k][c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                rgb[(y * 2 * w + w + x) * 3 + c] = ((truth[k] >> (8 * c)) & 255) as u8;
            }
        }
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), 2 * w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

/// Where the capture's depth samples are: a voxel hash per level, level L of 5 cm × 2^L, each sample in the level
/// whose voxels are at least 1.5 % of its distance from its camera.
struct Coverage {
    levels: Vec<std::collections::HashSet<u64>>,
}

impl Coverage {
    const BASE: f32 = 0.05;
    const RELATIVE: f32 = 0.015;
    const LEVELS: usize = 11;

    fn key(p: [f32; 3], size: f32) -> [i64; 3] {
        p.map(|v| (v / size).floor() as i64)
    }

    fn pack(k: [i64; 3]) -> u64 {
        k.iter().fold(0u64, |acc, v| (acc << 21) | ((v + (1 << 20)) as u64 & 0x1F_FFFF))
    }

    fn build(data: &dataset::Dataset) -> Self {
        let mut levels = vec![std::collections::HashSet::new(); Self::LEVELS];
        for dv in &data.depth_views {
            let c = dv.camera.position();
            for (k, &z) in dv.depth.iter().enumerate() {
                if z <= 0.0 || dv.is_unknown(k) {
                    continue;
                }
                let p = dv.camera.unproject(dv.grid.pixels[k][0], dv.grid.pixels[k][1], z);
                let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt();
                let level = ((Self::RELATIVE * d / Self::BASE).max(1.0).log2().ceil() as usize).min(Self::LEVELS - 1);
                levels[level].insert(Self::pack(Self::key(p, Self::BASE * (1 << level) as f32)));
            }
        }
        Self { levels }
    }

    /// A sample lies in the voxel of `p` or a neighbour, at some level.
    fn covers(&self, p: [f32; 3]) -> bool {
        self.levels.iter().enumerate().any(|(l, set)| {
            if set.is_empty() {
                return false;
            }
            let k = Self::key(p, Self::BASE * (1 << l) as f32);
            (-1..=1).any(|dx| (-1..=1).any(|dy| (-1..=1).any(|dz| set.contains(&Self::pack([k[0] + dx, k[1] + dy, k[2] + dz])))))
        })
    }
}

#[derive(Default)]
struct Sums {
    views: usize,
    psnr: f64,
    ssim: f64,
    scene_psnr: f64,
    scene_views: usize,
    depth_rel: f64,
    depth_within: f64,
    depth_views: usize,
    covered_psnr: f64,
    covered_views: usize,
    covered_share: f64,
}

impl Sums {
    fn json(&self) -> serde_json::Value {
        let n = self.views.max(1) as f64;
        let s = self.scene_views.max(1) as f64;
        let d = self.depth_views.max(1) as f64;
        serde_json::json!({
            "views": self.views,
            "psnr": self.psnr / n,
            "ssim": self.ssim / n,
            "scene_psnr": self.scene_psnr / s,
            "depth_median_rel": self.depth_rel / d,
            "depth_within_5pct": self.depth_within / d,
            "covered_psnr": self.covered_psnr / self.covered_views.max(1) as f64,
            "covered_share": self.covered_share / n,
        })
    }
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut views_dir, mut splats, mut json, mut renders, mut session) = (None, None, None, None, None);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--session" => session = Some(PathBuf::from(it.next().context("--session needs a folder")?)),
            "--json" => json = Some(PathBuf::from(it.next().context("--json needs a path")?)),
            "--renders" => renders = Some(PathBuf::from(it.next().context("--renders needs a folder")?)),
            _ if views_dir.is_none() => views_dir = Some(PathBuf::from(a)),
            _ => splats = Some(PathBuf::from(a)),
        }
    }
    let usage = "usage: offpath <views folder> <splats.ply> [--session <dir>] [--json <out.json>] [--renders <dir>]";
    let views_dir = views_dir.context(usage)?;
    let splats = splats.context(usage)?;
    let views: Vec<View> =
        std::fs::read_to_string(views_dir.join("views.jsonl"))?.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<Result<_, _>>()?;
    let coverage = match &session {
        Some(dir) => Some(Coverage::build(&dataset::load(dir, 1920, |_| {})?)),
        None => None,
    };
    let g = ply::read(&splats)?;
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    // The sky map beside the PLY (orbiters_reconstruct::sky::SkyMap), drawn behind the splats as every viewer does.
    let map = splats.with_file_name(orbiters_core::reconstruction::SKY_FILE);
    if map.is_file() {
        let sky = orbiters_reconstruct::sky::SkyMap::read(&map)?;
        gpu.set_sky(Some((sky.width, &sky.rgb)))?;
    }
    if let Some(dir) = &renders {
        std::fs::create_dir_all(dir)?;
    }
    let mut groups: BTreeMap<String, Sums> = BTreeMap::new();
    let mut all = Sums::default();
    let mut rows = Vec::new();
    for (i, v) in views.iter().enumerate() {
        let (truth, w, h) = read_png(&views_dir.join(&v.file))?;
        anyhow::ensure!(w == v.width && h == v.height, "{}: {w}×{h}, the list says {}×{}", v.file, v.width, v.height);
        let cam = Camera::from_strip(&v.camera_to_world, &v.projection, w, h, 1.0);
        let ones = vec![1.0f32; (w * h) as usize];
        gpu.set_mask(w, h, &ones);
        let img = train::render(&mut gpu, &cam, [0.0; 3])?;
        let (psnr, _, ssim) = metrics::metrics(&img, &truth, &ones, w as usize, h as usize);
        let depth: Option<Vec<f32>> = match &v.depth {
            Some(f) => Some(std::fs::read(views_dir.join(f))?.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()),
            None => None,
        };
        let (mut scene_psnr, mut depth_rel, mut depth_within, mut covered_psnr, mut covered_share) = (None, None, None, None, None);
        if let (Some(d), Some(cov)) = (&depth, &coverage) {
            // Every 4th pixel each way, spread over its 4 × 4 block.
            let mut mask = vec![0f32; (w * h) as usize];
            for by in (0..h).step_by(4) {
                for bx in (0..w).step_by(4) {
                    let (x, y) = ((bx + 2).min(w - 1), (by + 2).min(h - 1));
                    let z = d[(y * w + x) as usize];
                    if z > 0.0 && cov.covers(cam.unproject(x as f32, y as f32, z)) {
                        for yy in by..(by + 4).min(h) {
                            for xx in bx..(bx + 4).min(w) {
                                mask[(yy * w + xx) as usize] = 1.0;
                            }
                        }
                    }
                }
            }
            let share = mask.iter().sum::<f32>() as f64 / mask.len() as f64;
            covered_share = Some(share);
            if share > 0.01 {
                covered_psnr = Some(metrics::metrics(&img, &truth, &mask, w as usize, h as usize).0);
            }
        }
        if let Some(d) = &depth {
            let scene: Vec<f32> = d.iter().map(|z| if *z > 0.0 { 1.0 } else { 0.0 }).collect();
            if scene.iter().any(|m| *m > 0.5) {
                scene_psnr = Some(metrics::metrics(&img, &truth, &scene, w as usize, h as usize).0);
                // Rendered depth: the expected depth over the pixel's Gaussians.
                let mut rel: Vec<f32> = d.iter().zip(&img).filter(|(z, _)| **z > 0.0).map(|(z, p)| ((p[3] - z) / z).abs()).collect();
                rel.sort_by(f32::total_cmp);
                depth_rel = Some(rel[rel.len() / 2] as f64);
                depth_within = Some(rel.iter().filter(|r| **r <= 0.05).count() as f64 / rel.len() as f64);
            }
        }
        for s in [groups.entry(v.group.clone()).or_default(), &mut all] {
            s.views += 1;
            s.psnr += psnr;
            s.ssim += ssim;
            if let Some(p) = scene_psnr {
                s.scene_psnr += p;
                s.scene_views += 1;
            }
            if let (Some(r), Some(wi)) = (depth_rel, depth_within) {
                s.depth_rel += r;
                s.depth_within += wi;
                s.depth_views += 1;
            }
            if let Some(p) = covered_psnr {
                s.covered_psnr += p;
                s.covered_views += 1;
            }
            s.covered_share += covered_share.unwrap_or(0.0);
        }
        rows.push(serde_json::json!({ "file": v.file, "group": v.group, "psnr": psnr, "ssim": ssim, "scene_psnr": scene_psnr, "depth_median_rel": depth_rel, "depth_within_5pct": depth_within, "covered_psnr": covered_psnr, "covered_share": covered_share }));
        if let Some(dir) = &renders {
            write_side_by_side(&dir.join(format!("{i:03}_{}.png", v.group)), &img, &truth, w, h)?;
        }
    }
    println!("{} splats, {} views", g.len(), views.len());
    println!(
        "{:<12} {:>5} {:>8} {:>7} {:>10} {:>9} {:>8} {:>12} {:>8}",
        "group", "views", "PSNR", "SSIM", "scene PSNR", "depth err", "≤5 %", "covered PSNR", "covered"
    );
    let line = |name: &str, s: &Sums| {
        let j = s.json();
        println!(
            "{:<12} {:>5} {:>8.2} {:>7.4} {:>10.2} {:>8.1}% {:>7.1}% {:>12.2} {:>7.1}%",
            name,
            s.views,
            j["psnr"].as_f64().unwrap_or(0.0),
            j["ssim"].as_f64().unwrap_or(0.0),
            j["scene_psnr"].as_f64().unwrap_or(0.0),
            100.0 * j["depth_median_rel"].as_f64().unwrap_or(0.0),
            100.0 * j["depth_within_5pct"].as_f64().unwrap_or(0.0),
            j["covered_psnr"].as_f64().unwrap_or(0.0),
            100.0 * j["covered_share"].as_f64().unwrap_or(0.0)
        );
    };
    for (name, s) in &groups {
        line(name, s);
    }
    line("all", &all);
    if let Some(path) = json {
        let per_group: serde_json::Map<String, serde_json::Value> = groups.iter().map(|(k, s)| (k.clone(), s.json())).collect();
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({ "splats": g.len(), "all": all.json(), "groups": per_group, "per_view": rows }))?,
        )?;
    }
    Ok(())
}
