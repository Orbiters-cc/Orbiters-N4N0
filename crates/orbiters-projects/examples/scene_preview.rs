//! Builds the previews of Unity projects' scenes from their files and prints what each build found, read-only: the
//! projects are only read; previews go to the app's cache, or to `ORBITERS_PREVIEW_CACHE` when set.
//!
//! `cargo run -p orbiters-projects --example scene_preview -- [--scenes N] [--seconds S] <project>...`
//! (`--scenes` builds the first N scenes of each project, 1 by default.)
use orbiters_projects::scene;
use std::time::{Duration, Instant};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--model <file.fbx>`: a model's meshes, unskinned and posed by its own bones at rest (diagnostics).
    if args.first().map(String::as_str) == Some("--model") {
        let path = std::path::Path::new(&args[1]);
        let meta = std::fs::read_to_string(format!("{}.meta", path.display())).unwrap_or_default();
        let settings = scene::model::ImportSettings::read(&meta);
        let m = scene::model::load(path, &settings, &scene::Limits::default()).expect("model");
        // World matrices of the nodes at rest.
        let mut world = vec![scene::math::IDENTITY; m.nodes.len()];
        for (i, n) in m.nodes.iter().enumerate() {
            let local = scene::math::trs(n.position, n.rotation, n.scale);
            world[i] = match n.parent {
                Some(p) => scene::math::mul(&world[p], &local),
                None => local,
            };
        }
        for n in m.nodes.iter().take(6) {
            println!("node {:?} parent {:?} pos {:?} rot {:?} scale {:?}", n.name, n.parent, n.position, n.rotation, n.scale);
        }
        for (i, n) in m.nodes.iter().enumerate() {
            let Some(k) = n.mesh else { continue };
            let mesh = &m.meshes[k];
            let b = |pts: &mut dyn Iterator<Item = [f32; 3]>| {
                let mut lo = [f32::MAX; 3];
                let mut hi = [f32::MIN; 3];
                for p in pts {
                    for c in 0..3 {
                        lo[c] = lo[c].min(p[c]);
                        hi[c] = hi[c].max(p[c]);
                    }
                }
                (lo, hi)
            };
            let rigid = b(&mut mesh.positions.iter().map(|p| scene::math::point(&world[i], *p)));
            println!("{} v={} rigid {:?}", n.name, mesh.positions.len(), rigid);
            for (k, shape) in mesh.blend_shapes.iter().enumerate() {
                let big = shape
                    .frames
                    .iter()
                    .map(|f| f.1.iter().map(|(_, d)| (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()).fold(0.0f32, f32::max))
                    .collect::<Vec<_>>();
                println!(
                    "   shape {k} {:?} frames {:?} largest offsets {:?}",
                    shape.name,
                    shape.frames.iter().map(|f| (f.0, f.1.len())).collect::<Vec<_>>(),
                    big
                );
            }
            if let Some(skin) = &mesh.skin {
                let mats: Vec<[f32; 16]> = skin.bones.iter().zip(&skin.bind_poses).map(|(&bn, bp)| scene::math::mul(&world[bn], bp)).collect();
                let posed = b(&mut mesh.positions.iter().zip(&skin.weights).map(|(p, w)| {
                    let mut acc = [0.0f32; 3];
                    for (bi, wt) in w {
                        let q = scene::math::point(&mats[*bi as usize], *p);
                        for c in 0..3 {
                            acc[c] += q[c] * wt;
                        }
                    }
                    acc
                }));
                println!("   skinned {:?} bones={}", posed, skin.bones.len());
                // The bones whose skinning matrix moves the mesh the most.
                let mut worst: Vec<(f32, String)> = mats
                    .iter()
                    .zip(&skin.bones)
                    .map(|(mm, &bn)| {
                        (
                            (mm[12].powi(2) + mm[13].powi(2) + mm[14].powi(2)).sqrt() + (mm[0].abs() + mm[5].abs() + mm[10].abs() - 3.0).abs(),
                            m.nodes[bn].name.clone(),
                        )
                    })
                    .collect();
                worst.sort_by(|a, b| b.0.total_cmp(&a.0));
                println!("   largest skinning matrices: {:?}", &worst[..worst.len().min(4)]);
            }
        }
        return;
    }
    let mut take = |flag: &str, default: u64| -> u64 {
        match args.iter().position(|a| a == flag) {
            Some(i) if i + 1 < args.len() => {
                let v = args.remove(i + 1).parse().unwrap_or(default);
                args.remove(i);
                v
            }
            _ => default,
        }
    };
    let scenes_n = take("--scenes", 1) as usize;
    let seconds = take("--seconds", 60);
    println!("| Project | Scenes | Scene | Objects | Renderers drawn | Models | Meshes | Materials | Textures | Triangles | Build | File | Cache | Result |");
    println!("| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |");
    let cache_mb = |p: &std::path::Path| -> f64 {
        std::fs::read_dir(scene::cache_dir(p)).map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum::<u64>()).unwrap_or(0) as f64 / 1e6
    };
    for project in &args {
        let path = std::path::Path::new(project);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let t = Instant::now();
        let list = scene::scenes(path);
        let list_ms = t.elapsed().as_secs_f64() * 1000.0;
        if list.is_empty() {
            println!("| {name} | 0 | | | | | | | | | | | | no scene ({list_ms:.0} ms to look) |");
            continue;
        }
        for s in list.iter().take(scenes_n) {
            let limits = scene::Limits::new(Duration::from_secs(seconds));
            let t = Instant::now();
            let r = scene::build_scene(path, s, &limits);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let record: Option<scene::Record> =
                std::fs::read(scene::cache_dir(path).join(format!("{}.json", s.guid))).ok().and_then(|b| serde_json::from_slice(&b).ok());
            let rep = record.as_ref().map(|r| r.report.clone()).unwrap_or_default();
            let mark = if s.last_open {
                " (last open)"
            } else if s.in_build {
                " (build)"
            } else {
                ""
            };
            match r {
                Ok(e) => {
                    println!(
                        "| {name} | {} | {}{mark} | {} | {} of {} | {} | {} | {} | {} | {} | {:.1} s | {:.1} MB | {:.1} MB | ok{} |",
                        list.len(),
                        s.name,
                        rep.objects,
                        rep.drawn,
                        rep.renderers,
                        rep.models,
                        rep.meshes,
                        rep.materials,
                        rep.textures,
                        rep.triangles,
                        ms / 1000.0,
                        e.size as f64 / 1e6,
                        cache_mb(path),
                        if rep.missing.is_empty() { String::new() } else { format!(", {} unresolved", rep.missing.len()) }
                    );
                    eprintln!("  {} -> {}", s.rel, e.path.display());
                    for m in rep.missing.iter().take(6) {
                        eprintln!("    unresolved: {m}");
                    }
                }
                Err(why) => println!(
                    "| {name} | {} | {}{mark} | | | | | | | | {:.1} s | | {:.1} MB | No preview: {why} |",
                    list.len(),
                    s.name,
                    ms / 1000.0,
                    cache_mb(path)
                ),
            }
        }
    }
}
