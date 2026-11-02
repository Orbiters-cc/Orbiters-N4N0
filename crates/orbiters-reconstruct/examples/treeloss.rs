//! Lab: what of a reconstruction reaches VRChat. Makes the PC, Quest and coarse trees as an upload does
//! (orbiters_reconstruct::tree_tune::vrchat_files_with, from the session's recorded cameras), reads the files back as a
//! world does, and scores against ground truth: the full model, the full model as the file rounds it, each tree's
//! leaves, and the boxes the world's viewer draws from each view (contracts' Docs/splat-tree.md, its selection rule
//! with the viewer's settings: 1000 pixels per radian, a fade band of 1.4, an error target of `τ` pixels; the far splats
//! dropped at the coarser qualities). Ground truth: the run's held-out keyframes (as `eval` scores them), and with
//! `--gt` the off-path views of Mixer's simulator (as `offpath` scores them). The export never sees the held-out
//! keyframes' cameras, so the trees are not fitted to the views they are scored from.
//!
//! ```text
//! cargo run --release --example treeloss -- <run folder> [--gt <views folder>] [--json <out.json>] [--save <dir>]
//!     [--files <dir>] [--rank area|contribution|seen] [--refit <steps>] [--cameras <n>] [--rank-cameras <n>]
//!     [--decoded] [--tight] [--tune-steps <n>] [--drawn-at <pc px>,<quest px>] [--refit-width <n>] [--pc-appearance]
//!     [--no-full] [--width <n>]
//!     [--arm <label> "<export options>"]... [--json-dir <dir> --name <session>] [--diag]
//! ```
//! `--files` scores saved pc.n4tree / quest.n4tree / coarse.n4tree instead of making them; `--save` keeps the files
//! made (in a folder per arm). `--arm` (repeated) scores several exports of the same model in one run, the dataset
//! and the full model's renders loaded once, each arm's JSON in `<json dir>/<name>.<label>.json`. `--no-full` leaves
//! out the rounded full model's score. `--diag` scores only the full model with each field of the splat record rounded
//! as the file rounds it (no trees).

use anyhow::Context as _;
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::gaussians::{logit, sigmoid, Gaussians};
use orbiters_reconstruct::tree::Tree;
use orbiters_reconstruct::tree_tune::{self, ExportOptions, Rank};
use orbiters_reconstruct::{aberration, dataset, gpu, metrics, ply, prepare, train};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The viewer's settings (N4N0Viewer SplatDisplaySettings): pixels per radian, fade band, the mobile error scale, the
/// far splats' radius at the coarser qualities.
const PPR: f32 = 1000.0;
const FADE_BAND: f32 = 1.4;
const MOBILE_SCALE: f32 = 1.5;
const SKY_CULL: f32 = 100.0;

/// One view scored: its camera, ground truth (packed RGBA8), the pixels that count, and the aberration a render passes
/// through first.
struct View {
    camera: Camera,
    truth: Vec<u32>,
    mask: Vec<f32>,
    aberration: Option<(f32, u32, Vec<f32>)>,
    /// The full model's render (packed), for the PSNR against it.
    full: Option<Vec<u32>>,
}

struct Set {
    name: &'static str,
    views: Vec<View>,
}

/// What is drawn: a fixed set of splats, or the boxes of a tree the viewer picks from each view.
enum Drawn<'a> {
    Model(&'a Gaussians),
    Cut { tree: &'a Tree, tau: f32, sky_cull: Option<[f32; 3]> },
}

#[derive(Deserialize)]
struct GtView {
    file: String,
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
    let px = buf[..info.buffer_size()].chunks_exact(channels).map(|c| c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 | 255 << 24).collect();
    Ok((px, info.width, info.height))
}

fn packed(img: &[[f32; 4]]) -> Vec<u32> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    img.iter().map(|p| byte(p[0]) | byte(p[1]) << 8 | byte(p[2]) << 16 | 255 << 24).collect()
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The splats the viewer draws of `tree` from `eye` at an error target of `tau` pixels, each box's opacity weighted
/// as it fades between depths (splat-tree.md, Drawing a tree), and how many.
fn cut(tree: &Tree, eye: [f32; 3], tau: f32, sky_cull: Option<[f32; 3]>) -> Gaussians {
    let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let f: Vec<f32> = tree.nodes.iter().map(|n| n.error * PPR / ((dist(eye, n.center) - n.radius).max(0.05) * tau)).collect();
    let s = |x: f32| smoothstep(1.0, FADE_BAND, x);
    let mut g = Gaussians::default();
    for (b, n) in tree.nodes.iter().enumerate() {
        let above = n.parent.map_or(1.0, |p| s(f[p as usize]));
        let w = (above - s(f[b])).max(0.0);
        if w <= 0.0 {
            continue;
        }
        let t = &tree.splats;
        for i in n.start as usize..(n.start + n.count) as usize {
            if sky_cull.is_some_and(|c| dist(t.means[i], c) > SKY_CULL) {
                continue;
            }
            let opacity = if w >= 1.0 { t.opacity_logits[i] } else { logit((sigmoid(t.opacity_logits[i]) * w).max(1e-6)) };
            g.push(t.means[i], t.log_scales[i], t.rotations[i], opacity, t.colors[i]);
        }
    }
    g
}

/// Mean PSNR, SSIM and PSNR against the full model's render of `drawn` over `set`'s views; with `keep_full`, its
/// renders become the views' full-model renders.
fn score(gpu: &mut gpu::Gpu, set: &mut Set, drawn: &Drawn, keep_full: bool) -> anyhow::Result<serde_json::Value> {
    let (mut psnr, mut ssim, mut vs_full, mut drawn_splats) = (0f64, 0f64, 0f64, 0f64);
    if let Drawn::Model(g) = drawn {
        gpu.upload(g);
    }
    for v in set.views.iter_mut() {
        let (w, h) = (v.camera.width as usize, v.camera.height as usize);
        match drawn {
            Drawn::Model(g) => drawn_splats += g.len() as f64,
            Drawn::Cut { tree, tau, sky_cull } => {
                let g = cut(tree, v.camera.position(), *tau, *sky_cull);
                drawn_splats += g.len() as f64;
                gpu.upload(&g);
            }
        }
        gpu.set_mask(v.camera.width, v.camera.height, &v.mask)?;
        let img = train::render(gpu, &v.camera, [0.0; 3])?;
        let shown = match &v.aberration {
            Some((intensity, factor, _)) => aberration::aberrate(&img, w, h, *intensity, *factor),
            None => img.clone(),
        };
        let mask = v.aberration.as_ref().map_or(&v.mask, |a| &a.2);
        let (p, _, s) = metrics::metrics(&shown, &v.truth, mask, w, h);
        psnr += p;
        ssim += s;
        if keep_full {
            v.full = Some(packed(&img));
        }
        if let Some(full) = &v.full {
            vs_full += metrics::metrics(&img, full, &v.mask, w, h).0;
        }
    }
    let n = set.views.len().max(1) as f64;
    Ok(serde_json::json!({ "psnr": psnr / n, "ssim": ssim / n, "psnr_vs_full": vs_full / n, "drawn_splats": drawn_splats / n }))
}

/// An export option of `args` at `a` (taking its value from `it`); false when `a` is none.
fn export_option(a: &str, it: &mut dyn Iterator<Item = String>, options: &mut ExportOptions) -> anyhow::Result<bool> {
    let mut value = || it.next().with_context(|| format!("{a} needs a value"));
    match a {
        "--rank" => options.rank = Rank::parse(&value()?)?,
        "--refit" => options.refit_steps = value()?.parse()?,
        "--cameras" => options.cameras = value()?.parse()?,
        "--rank-cameras" => options.rank_cameras = value()?.parse()?,
        "--tune-steps" => options.tune_steps = value()?.parse()?,
        "--drawn-at" => {
            let v: Vec<f32> = value()?.split(',').map(str::parse).collect::<Result<_, _>>()?;
            options.drawn_at = [v[0], *v.get(1).unwrap_or(&v[0])];
        }
        "--decoded" => options.decoded = true,
        "--refit-width" => options.refit_width = value()?.parse()?,
        "--pc-appearance" => options.pc_appearance = true,
        "--tight" => options.tight = true,
        _ => return Ok(false),
    }
    Ok(true)
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut run, mut gt, mut json, mut save, mut files, mut width, mut full, mut diag) = (None, None, None, None, None, 1920u32, true, false);
    let (mut json_dir, mut name): (Option<PathBuf>, Option<String>) = (None, None);
    let mut options = ExportOptions::default();
    let mut arm_specs: Vec<(String, String)> = Vec::new();
    while let Some(a) = it.next() {
        if export_option(&a, &mut it, &mut options)? {
            continue;
        }
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--gt" => gt = Some(PathBuf::from(value()?)),
            "--json" => json = Some(PathBuf::from(value()?)),
            "--json-dir" => json_dir = Some(PathBuf::from(value()?)),
            "--name" => name = Some(value()?),
            "--arm" => {
                let label = value()?;
                arm_specs.push((label, value()?));
            }
            "--save" => save = Some(PathBuf::from(value()?)),
            "--files" => files = Some(PathBuf::from(value()?)),
            "--width" => width = value()?.parse()?,
            "--no-full" => full = false,
            "--diag" => diag = true,
            _ if run.is_none() => run = Some(PathBuf::from(&a)),
            _ => anyhow::bail!("unknown argument {a}"),
        }
    }
    let run = run.context("usage: treeloss <run folder> [options]")?;
    // The arms: `--arm <label> "<export options>"` each (JSON in <json dir>/<name>.<label>.json), else the one made of
    // the command line's options.
    let mut arms: Vec<(String, ExportOptions, Option<PathBuf>)> = Vec::new();
    for (label, spec) in &arm_specs {
        let mut o = ExportOptions::default();
        let mut words = spec.split_whitespace().map(String::from);
        while let Some(w) = words.next() {
            anyhow::ensure!(export_option(&w, &mut words, &mut o)?, "arm {label}: unknown option {w}");
        }
        let path = json_dir.as_ref().map(|d| d.join(format!("{}.{label}.json", name.as_deref().unwrap_or("run"))));
        arms.push((label.clone(), o, path));
    }
    if arms.is_empty() {
        arms.push((String::from("arm"), options, json.clone()));
    }
    let result = orbiters_core::reconstruction::result_dir(&run);
    let ply_path = result.join(orbiters_core::reconstruction::PLY_FILE);
    let g = ply::read(&ply_path)?;
    let mut gpu = gpu::Gpu::new()?;

    // The held-out keyframes (the run's report), scored as eval scores them (depth validation only marks depth samples,
    // which scoring never reads).
    let prepared = prepare::prepare(&run, &prepare::Options { width, validate: false, ..Default::default() }, |_| {})?;
    let data = &prepared.data;
    let report = orbiters_core::reconstruction::read_report(&run).context("the run has no report")?;
    let held: Vec<usize> = report.held_out_keyframes.iter().filter_map(|&k| data.views.iter().position(|v| v.keyframe == k)).collect();
    anyhow::ensure!(!held.is_empty(), "no held-out keyframe loaded");
    let mut sets = vec![Set {
        name: "held_out",
        views: held
            .iter()
            .map(|&i| {
                let v = &data.views[i];
                Ok(match &prepared.undo {
                    Some(u) => View {
                        camera: v.camera,
                        truth: dataset::keyframe_image(&data.dir, v.keyframe, &data.geometry)?,
                        mask: data.mask.clone(),
                        aberration: Some((u.intensity, u.factor, u.mask.clone())),
                        full: None,
                    },
                    None => View { camera: v.camera, truth: v.image.clone(), mask: data.mask.clone(), aberration: None, full: None },
                })
            })
            .collect::<anyhow::Result<_>>()?,
    }];
    if let Some(dir) = &gt {
        let list: Vec<GtView> =
            std::fs::read_to_string(dir.join("views.jsonl"))?.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<Result<_, _>>()?;
        let views = list
            .iter()
            .map(|v| {
                let (truth, w, h) = read_png(&dir.join(&v.file))?;
                anyhow::ensure!(w == v.width && h == v.height, "{}: size", v.file);
                Ok(View { camera: Camera::from_strip(&v.camera_to_world, &v.projection, w, h, 1.0), truth, mask: vec![1.0; (w * h) as usize], aberration: None, full: None })
            })
            .collect::<anyhow::Result<_>>()?;
        sets.push(Set { name: "offpath", views });
    }
    // The sky map behind everything, as the world draws it.
    let map = ply_path.with_file_name(orbiters_core::reconstruction::SKY_FILE);
    let sky = map.is_file();
    if sky {
        let s = orbiters_reconstruct::sky::SkyMap::read(&map)?;
        gpu.set_sky(Some((s.width, &s.rgb)))?;
    }

    if diag {
        // What each field of the splat record costs: the full model with one field as the file rounds it.
        let field = |f: &dyn Fn(&mut Gaussians, &Gaussians, usize)| {
            let mut rounded = g.clone();
            tree_tune::quantize(&mut rounded);
            let mut out = g.clone();
            (0..g.len()).for_each(|i| f(&mut out, &rounded, i));
            out
        };
        let variants: Vec<(&str, Gaussians)> = vec![
            ("full", g.clone()),
            ("colour_clamped", field(&|o, _, i| o.colors[i] = o.colors[i].map(|c| c.clamp(0.0, 1.0)))),
            ("colour_rounded", field(&|o, r, i| o.colors[i] = r.colors[i])),
            ("opacity_rounded", field(&|o, r, i| o.opacity_logits[i] = r.opacity_logits[i])),
            ("rotation_rounded", field(&|o, r, i| o.rotations[i] = r.rotations[i])),
            ("all_rounded", field(&|o, r, i| {
                o.colors[i] = r.colors[i];
                o.opacity_logits[i] = r.opacity_logits[i];
                o.rotations[i] = r.rotations[i];
                o.log_scales[i] = r.log_scales[i];
            })),
        ];
        let outside = (0..g.len()).filter(|&i| g.colors[i].iter().any(|c| !(0.0..=1.0).contains(c))).count();
        println!("{} splats, {outside} with a colour outside 0–1", g.len());
        let mut out = serde_json::Map::new();
        for set in &mut sets {
            let mut scores = serde_json::Map::new();
            for (k, (name, v)) in variants.iter().enumerate() {
                let s = score(&mut gpu, set, &Drawn::Model(v), k == 0)?;
                println!("  {} {name:<16} PSNR {:7.3} dB  SSIM {:.4}  vs full {:7.3} dB", set.name, s["psnr"].as_f64().unwrap_or(0.0), s["ssim"].as_f64().unwrap_or(0.0), s["psnr_vs_full"].as_f64().unwrap_or(0.0));
                scores.insert((*name).into(), s);
            }
            out.insert(set.name.into(), serde_json::Value::Object(scores));
        }
        if let Some(path) = json {
            std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({ "run": run.display().to_string(), "outside": outside, "scores": out }))?)?;
        }
        return Ok(());
    }

    // The trees: made as an upload makes them, from the recorded cameras but the held-out ones.
    let held_keyframes: Vec<i32> = report.held_out_keyframes.clone();
    let cameras = tree_tune::session_cameras_without(&run, &held_keyframes)?;
    let n = cameras.len().max(1) as f32;
    let centre = cameras.iter().fold([0.0f32; 3], |a, c| [0, 1, 2].map(|k| a[k] + c.position()[k] / n));
    // The far splats beyond the start view's radius: approximated by the cameras' centre.
    let far = |t: &Gaussians| (0..t.len()).filter(|&i| (0..3).map(|k| (t.means[i][k] - centre[k]).powi(2)).sum::<f32>().sqrt() > SKY_CULL).count();
    let sky_cull = sky.then_some(centre);
    // The leaves without the far splats (the world's cull alone, no LOD).
    let near = |t: &Gaussians| {
        let keep: Vec<bool> = (0..t.len()).map(|i| !sky || (0..3).map(|k| (t.means[i][k] - centre[k]).powi(2)).sum::<f32>().sqrt() <= SKY_CULL).collect();
        let mut out = t.clone();
        out.retain(&keep);
        out
    };

    // The full model first, once: its renders are what the trees are compared with too.
    let mut rounded = g.clone();
    tree_tune::quantize(&mut rounded);
    let mut full_scores = Vec::new();
    for set in &mut sets {
        let mut scores = serde_json::Map::new();
        scores.insert("full".into(), score(&mut gpu, set, &Drawn::Model(&g), true)?);
        if full {
            scores.insert("full_rounded".into(), score(&mut gpu, set, &Drawn::Model(&rounded), false)?);
        }
        full_scores.push(scores);
    }
    drop(rounded);

    for (label, options, json) in &arms {
        let (pc_bytes, quest_bytes, coarse_bytes, seconds) = match &files {
            Some(dir) => (std::fs::read(dir.join("pc.n4tree"))?, std::fs::read(dir.join("quest.n4tree"))?, std::fs::read(dir.join("coarse.n4tree"))?, None),
            None => {
                // Tuning draws no sky (the leaves are the target): the device keeps the map for scoring afterwards.
                let mut prep = gpu::Gpu::new()?;
                let start = Instant::now();
                let made = tree_tune::vrchat_files_with(&g, &cameras, Some(&mut prep), 7, options, |_| {})?;
                (made.pc, made.quest, made.coarse, Some(start.elapsed().as_secs_f64()))
            }
        };
        if let Some(dir) = &save {
            let dir = dir.join(label);
            std::fs::create_dir_all(&dir)?;
            for (name, bytes) in [("pc.n4tree", &pc_bytes), ("quest.n4tree", &quest_bytes), ("coarse.n4tree", &coarse_bytes)] {
                std::fs::write(dir.join(name), bytes)?;
            }
        }
        let pc = Tree::parse(&pc_bytes)?;
        let quest = Tree::parse(&quest_bytes)?;
        let coarse = Tree::parse(&coarse_bytes)?;
        let (pc_leaves, quest_leaves, coarse_leaves) = (pc.leaf_splats(), quest.leaf_splats(), coarse.leaf_splats());
        let (pc_near, quest_near) = (near(&pc_leaves), near(&quest_leaves));
        println!(
            "[{label}] {}: {} splats ({} far); PC {} splats in {} boxes ({} leaves, {} far), Quest {} ({} leaves, {} far), coarse {}{}",
            run.display(),
            g.len(),
            far(&g),
            pc.len(),
            pc.nodes.len(),
            pc_leaves.len(),
            far(&pc_leaves),
            quest.len(),
            quest_leaves.len(),
            far(&quest_leaves),
            coarse.len(),
            seconds.map_or(String::new(), |s| format!(", made in {s:.1} s"))
        );
        let mut out = serde_json::Map::new();
        for (set, full) in sets.iter_mut().zip(&full_scores) {
            let mut scores = full.clone();
            let drawn: Vec<(&str, Drawn)> = vec![
                ("pc_leaves", Drawn::Model(&pc_leaves)),
                ("quest_leaves", Drawn::Model(&quest_leaves)),
                ("pc_leaves_culled", Drawn::Model(&pc_near)),
                ("quest_leaves_culled", Drawn::Model(&quest_near)),
                // PC: the high quality (2 px, no cull) and auto's start (4.5 px, far splats dropped with a sky map).
                ("pc_cut_2", Drawn::Cut { tree: &pc, tau: 2.0, sky_cull: None }),
                ("pc_cut_4.5", Drawn::Cut { tree: &pc, tau: 4.5, sky_cull }),
                // Quest: balanced (10 px) and auto's start (22 px), 1.5 times coarser, far splats dropped.
                ("quest_cut_15", Drawn::Cut { tree: &quest, tau: 10.0 * MOBILE_SCALE, sky_cull }),
                ("quest_cut_33", Drawn::Cut { tree: &quest, tau: 22.0 * MOBILE_SCALE, sky_cull }),
                ("coarse_leaves", Drawn::Model(&coarse_leaves)),
            ];
            for (name, d) in &drawn {
                scores.insert((*name).into(), score(&mut gpu, set, d, false)?);
            }
            println!("[{label}] {} ({} views):", set.name, set.views.len());
            for (name, s) in &scores {
                println!(
                    "  {name:<19} PSNR {:7.3} dB  SSIM {:.4}  vs full {:7.3} dB  {:>9.0} splats drawn",
                    s["psnr"].as_f64().unwrap_or(0.0),
                    s["ssim"].as_f64().unwrap_or(0.0),
                    s["psnr_vs_full"].as_f64().unwrap_or(0.0),
                    s["drawn_splats"].as_f64().unwrap_or(0.0)
                );
            }
            out.insert(set.name.into(), serde_json::Value::Object(scores));
        }
        let summary = serde_json::json!({
            "run": run.display().to_string(),
            "label": label,
            "options": format!("{options:?}"),
            "files": files.as_ref().map(|f| f.display().to_string()),
            "prep_seconds": seconds,
            "splats": g.len(),
            "far_splats": far(&g),
            "pc": { "splats": pc.len(), "boxes": pc.nodes.len(), "leaves": pc_leaves.len(), "far_leaves": far(&pc_leaves) },
            "quest": { "splats": quest.len(), "boxes": quest.nodes.len(), "leaves": quest_leaves.len(), "far_leaves": far(&quest_leaves) },
            "coarse": { "splats": coarse.len() },
            "scores": out,
        });
        if let Some(path) = json {
            std::fs::write(path, serde_json::to_string_pretty(&summary)?)?;
        }
    }
    Ok(())
}
