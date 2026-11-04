//! Builds the previews of Unity projects' scenes from their files and prints what each build found, read-only: the
//! projects are only read; previews go to the app's cache, or to `ORBITERS_PREVIEW_CACHE` when set.
//!
//! `cargo run -p orbiters-projects --example scene_preview -- [--scenes N] [--seconds S] <project>...`
//! (`--scenes` builds the first N scenes of each project, 1 by default.)
use orbiters_projects::scene;
use std::time::{Duration, Instant};

/// A value for reading: long runs of bytes as their length.
fn print(v: &scene::yaml::Value) -> String {
    use scene::yaml::Value;
    match v {
        Value::Null => "null".into(),
        Value::Str(s) => s.clone(),
        Value::Bytes(b) => format!("<{} bytes>", b.len()),
        Value::Seq(l) => format!("[{}{}]", l.iter().take(8).map(print).collect::<Vec<_>>().join(", "), if l.len() > 8 { format!(", ... {}", l.len()) } else { String::new() }),
        Value::Map(m) => format!("{{{}}}", m.iter().map(|(k, v)| format!("{k}: {}", print(v))).collect::<Vec<_>>().join(", ")),
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--dump <binary file> [class]`: the objects of a Unity binary file (diagnostics).
    if args.first().map(String::as_str) == Some("--dump") {
        let bytes = std::fs::read(&args[1]).expect("file");
        let class: Option<i32> = args.get(2).and_then(|c| c.parse().ok());
        let limit: usize = args.get(3).and_then(|c| c.parse().ok()).unwrap_or(4000);
        let file = scene::serialized::SerializedFile::parse(&bytes).expect("a serialized file");
        let mut counts = std::collections::BTreeMap::new();
        for o in file.objects() {
            *counts.entry(o.class).or_insert(0usize) += 1;
        }
        println!("classes {counts:?}");
        for o in file.objects().iter().filter(|o| Some(o.class) == class) {
            let at = (file.data_offset + o.start) as usize;
            let v = file.value(o, &bytes[at..at + o.size as usize]).expect("object");
            let text = print(&v);
            println!("--- class {} id {} size {}: {}", o.class, o.id, o.size, &text[..text.len().min(limit)]);
        }
        return;
    }
    // `--artifact <project> <guid>`: what Unity's import made of an asset (diagnostics).
    if args.first().map(String::as_str) == Some("--artifact") {
        let db = scene::artifact::ArtifactDb::open(std::path::Path::new(&args[1])).expect("a database");
        let file = db.file_of(&args[2]).expect("an artifact");
        println!("{}", file.display());
        let a = scene::artifact::Artifact::open(&file, &args[2], 2 << 30).expect("artifact");
        for class in [1, 4, 21, 23, 28, 33, 43, 74, 95, 137] {
            println!("class {class}: {}", a.count(class));
        }
        for id in a.ids(43) {
            let m = a.mesh(id);
            match m {
                Ok(m) => {
                    let b = scene::build::bounds_of(&m.positions);
                    println!(
                        "mesh {id} {:?} v={} tris={} subs={} uv2={} shapes={} bones={} {:?}..{:?}",
                        m.name,
                        m.positions.len(),
                        m.triangles(),
                        m.submeshes.len(),
                        m.uv2.len(),
                        m.blend_shapes.len(),
                        m.skin.as_ref().map_or(0, |s| s.bind_poses.len()),
                        b.min,
                        b.max
                    );
                }
                Err(e) => println!("mesh {id}: {e:#}"),
            }
        }
        return;
    }
    // `--compare <project> <model>`: the file as ufbx reads it against Unity's import of it, bone by bone (diagnostics).
    if args.first().map(String::as_str) == Some("--compare") {
        use scene::prefab::Obj;
        let project = std::path::Path::new(&args[1]);
        let model = std::path::Path::new(&args[2]);
        let text = std::fs::read_to_string(format!("{}.meta", model.display())).unwrap_or_default();
        let guid = scene::yaml::meta_guid(&text).expect("a guid");
        let db = scene::artifact::ArtifactDb::open(project).expect("a database");
        let a = scene::artifact::Artifact::open(&db.file_of(&guid).expect("an artifact"), &guid, 2 << 30).expect("artifact");
        let m = scene::model::load(model, &scene::model::ImportSettings::read(&text), &scene::Limits::default()).expect("model");
        let g = a.graph().expect("graph");
        for (id, o) in &g.objects {
            let Obj::Renderer(r) = o else { continue };
            let mesh_id = match (&r.mesh, r.skinned) {
                (scene::prefab::MeshSource::Asset(x), true) => x.file_id,
                _ => continue,
            };
            let u = a.mesh(mesh_id).expect("mesh");
            let name_of = |t: i64| -> String {
                let Some(Obj::Transform(tr)) = g.objects.get(&t) else { return "?".into() };
                match g.objects.get(&tr.game_object) {
                    Some(Obj::GameObject(go)) => go.name.clone(),
                    _ => "?".into(),
                }
            };
            println!("renderer {id} mesh {:?} unity: v={} bones={} renderer bones={}", u.name, u.positions.len(), u.skin.as_ref().map_or(0, |s| s.bind_poses.len()), r.bones.len());
            let Some(mine) = m.meshes.iter().find(|x| x.name == u.name) else { println!("  not in the file by that name"); continue };
            println!("  file: v={} bones={}", mine.positions.len(), mine.skin.as_ref().map_or(0, |s| s.bind_poses.len()));
            let b = |p: &[[f32; 3]]| scene::build::bounds_of(p);
            println!("  unity vertices {:?}\n  file vertices  {:?}", b(&u.positions), b(&mine.positions));
            let (Some(us), Some(ms)) = (&u.skin, &mine.skin) else { continue };
            let mut worst = Vec::new();
            let mut missing = 0;
            for (i, t) in r.bones.iter().enumerate() {
                let n = name_of(*t);
                let Some(k) = ms.names.iter().position(|x| *x == n) else { missing += 1; continue };
                let d: f32 = (0..16).map(|c| (us.bind_poses[i][c] - ms.bind_poses[k][c]).abs()).fold(0.0, f32::max);
                worst.push((d, n, i));
            }
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
            println!("  bones by name: {} matched, {missing} not in the file; largest differences {:?}", worst.len(), &worst[..worst.len().min(5)]);
            if let Some((_, n, i)) = worst.first() {
                let k = ms.names.iter().position(|x| x == n).unwrap();
                println!("  {n}: unity {:?}\n  {n}: file  {:?}", us.bind_poses[*i], ms.bind_poses[k]);
            }
            let none: Vec<usize> = (0..ms.weights.len()).filter(|&i| ms.weights[i].iter().map(|w| w.1).sum::<f32>() < 1e-6).collect();
            println!("  file vertices without a weight: {} of {}", none.len(), ms.weights.len());
            for &i in none.iter().take(4) {
                println!("    vertex {i}: position {:?}, unity's weights {:?} (unity's bone 0 is {:?})", mine.positions[i], us.weights.get(i), r.bones.first().map(|t| name_of(*t)));
            }
            let differ = (0..ms.weights.len().min(us.weights.len()))
                .filter(|&i| {
                    let named = |w: &[(u16, f32); 4], n: &dyn Fn(u16) -> String| -> Vec<(String, i32)> {
                        let mut l: Vec<(String, i32)> = w.iter().filter(|x| x.1 > 1e-3).map(|x| (n(x.0), (x.1 * 100.0).round() as i32)).collect();
                        l.sort();
                        l
                    };
                    named(&ms.weights[i], &|b| ms.names[b as usize].clone()) != named(&us.weights[i], &|b| name_of(r.bones[b as usize]))
                })
                .count();
            println!("  vertices whose weights differ (by bone name, to 1 %): {differ}");
            let same = u.positions.iter().zip(&mine.positions).filter(|(a, b)| (0..3).all(|c| (a[c] - b[c]).abs() < 1e-3)).count();
            println!("  vertices the same in the same order: {same} of {}", u.positions.len());
        }
        return;
    }
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
    let no_artifacts = args.iter().position(|a| a == "--no-artifacts").map(|i| args.remove(i)).is_some();
    let only = args.iter().position(|a| a == "--scene").map(|i| {
        let v = args.remove(i + 1);
        args.remove(i);
        v
    });
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
        let wanted: Vec<_> = match &only {
            Some(name) => list.iter().filter(|s| s.rel.contains(name.as_str())).take(1).collect(),
            None => list.iter().take(scenes_n).collect(),
        };
        for s in wanted {
            let mut limits = scene::Limits::new(Duration::from_secs(seconds));
            limits.use_artifacts = !no_artifacts;
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
                    eprintln!("    {:?} kind {:?} bounds {:?} focus {:?}", e.header.stats, e.header.kind, e.header.bounds, e.header.focus);
                    eprintln!("    dropped {:?}", rep.dropped);
                    for m in rep.missing.iter().take(40) {
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
