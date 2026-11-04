//! Builds the previews of Unity projects' scenes from their files and prints what each build found, read-only: the
//! projects are only read; previews go to the app's cache, or to `ORBITERS_PREVIEW_CACHE` when set.
//!
//! `cargo run -p orbiters-projects --example scene_preview -- [--scenes N] [--seconds S] <project>...`
//! (`--scenes` builds the first N scenes of each project, 1 by default.)
//!
//! `--materials <project>` compares the two ways of reading a model, material by material; `--draws <preview>...` lists
//! what a built preview draws (`docs/project-preview.md`, Models read from the file against Unity's import).
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

mod materials {
    use orbiters_projects::scene;
    use scene::prefab::{Loader, MatSource, MeshSource, Obj};
    use std::collections::HashMap;
    use std::path::Path;

    /// Each submesh's triangles and where they are, to tell one submesh from another whatever order they come in.
    fn signature(m: &scene::model::ModelMesh) -> Vec<(usize, [f32; 3])> {
        m.submeshes
            .iter()
            .map(|t| {
                let mut c = [0.0f64; 3];
                for &i in t {
                    let p = m.positions[i as usize];
                    for k in 0..3 {
                        c[k] += p[k] as f64;
                    }
                }
                let n = t.len().max(1) as f64;
                (t.len() / 3, [(c[0] / n) as f32, (c[1] / n) as f32, (c[2] / n) as f32])
            })
            .collect()
    }

    /// Unity's submesh as a submesh of the file's: nearly as many triangles (Unity's import drops degenerate ones, which
    /// the file still has), the nearest.
    fn same_part(of: (usize, [f32; 3]), among: &[(usize, [f32; 3])]) -> Option<usize> {
        let d = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
        let near = |n: usize| n.abs_diff(of.0) * 10 <= of.0.max(1);
        (0..among.len()).filter(|&j| near(among[j].0)).min_by(|&x, &y| d(of.1, among[x].1).total_cmp(&d(of.1, among[y].1)))
    }

    #[derive(Default)]
    struct Totals {
        models: usize,
        unreadable: usize,
        no_import: usize,
        renderers: usize,
        unmatched: usize,
        multi: usize,
        reordered: usize,
        degenerate: usize,
        own_materials: usize,
        own_unknown: usize,
        slots: usize,
        misplaced: usize,
        wrong_slot: usize,
        wrong_part: usize,
        pictures: usize,
        parts_differ: usize,
    }

    pub fn compare(project: &Path, only: Option<&str>) {
        let mut limits = scene::Limits::new(std::time::Duration::from_secs(7200));
        limits.background = true;
        limits.lower_priority();
        let assets = scene::assets::index(project, None, &limits).expect("an index");
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for root in ["Assets", "Packages"] {
            for e in walkdir::WalkDir::new(project.join(root)).follow_links(true).into_iter().flatten() {
                let ext = e.path().extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if (ext == "fbx" || ext == "obj") && only.is_none_or(|o| e.path().to_string_lossy().contains(o)) {
                    files.push(e.path().to_path_buf());
                }
            }
        }
        files.sort();
        let mut t = Totals::default();
        for path in files {
            // One model at a time: a project's models together are more than a preview would ever hold.
            let mut loader = Loader::new(&assets, &limits);
            let meta = scene::prefab::meta_path(&path);
            let Ok(text) = std::fs::read_to_string(&meta) else { continue };
            let Some(guid) = scene::yaml::meta_guid(&text) else { continue };
            let Some(a) = loader.artifact(&guid, &path, false) else {
                t.no_import += 1;
                continue;
            };
            let rel = scene::prefab::rel(&assets, &path);
            let (Ok(g), Ok(m)) = (a.graph(), loader.model(&guid)) else {
                t.unreadable += 1;
                println!("{rel}: unreadable");
                continue;
            };
            t.models += 1;
            let filters: HashMap<i64, &MeshSource> = g.objects.values().filter_map(|o| if let Obj::Filter(f) = o { Some((f.game_object, &f.mesh)) } else { None }).collect();
            let mut shaders = HashMap::new();
            // A prefab refers to a model's own material by Unity's ID of it.
            for id in a.ids(21) {
                t.own_materials += 1;
                let Ok(v) = a.object(id, 21) else { continue };
                let name = v.get("m_Name").str_or("");
                if !m.materials.iter().any(|mm| scene::model::file_id("Material", &mm.name, 0) == id) {
                    t.own_unknown += 1;
                    println!("{rel}: Unity's material {name:?} has ID {id}, not the one its name makes (the file has {:?})", m.materials.iter().map(|x| x.name.as_str()).collect::<Vec<_>>());
                }
            }
            for o in g.objects.values() {
                let Obj::Renderer(r) = o else { continue };
                let source = if r.skinned { Some(&r.mesh) } else { filters.get(&r.game_object).copied() };
                let Some(MeshSource::Asset(mesh_ref)) = source else { continue };
                let Ok(u) = a.mesh(mesh_ref.file_id) else { continue };
                let Some(Obj::GameObject(go)) = g.objects.get(&r.game_object) else { continue };
                t.renderers += 1;
                // By the name of its node, else its mesh's (the model's root takes the file's name).
                let Some(node) = m.nodes.iter().find(|n| n.mesh.is_some() && n.name == go.name).or_else(|| m.nodes.iter().find(|n| n.mesh.is_some_and(|k| m.meshes[k].name == u.name))) else {
                    t.unmatched += 1;
                    println!("{rel}: {:?} is not in the file", go.name);
                    continue;
                };
                let mine = &m.meshes[node.mesh.unwrap()];
                let (us, ms) = (signature(&u), signature(mine));
                if u.submeshes.len() > 1 || mine.submeshes.len() > 1 {
                    t.multi += 1;
                }
                let order: Vec<Option<usize>> = if us.len() == 1 && ms.len() == 1 { vec![Some(0)] } else { us.iter().map(|s| same_part(*s, &ms)).collect() };
                let differ = u.submeshes.len() != mine.submeshes.len() || order.iter().any(Option::is_none);
                t.degenerate += us.iter().zip(&order).filter(|(s, j)| j.is_some_and(|j| ms[j].0 != s.0)).count();
                if differ {
                    t.parts_differ += 1;
                }
                let look = |loader: &mut Loader, src: Option<&MatSource>, shaders: &mut HashMap<String, String>| -> String {
                    match src {
                        Some(s) => scene::build::material_of_slot(loader, s, shaders),
                        None => "(nothing)".into(),
                    }
                };
                // What a prefab's or a scene's list of materials lands on: slot `i` is submesh `i`, whichever geometry
                // that is. Names only (two materials of one name are one, here), pictures counted apart.
                // A mesh without a material draws plain white under either name, whichever import mode named it.
                let name = |s: &str| s.rsplit_once(" [").map_or(s, |x| x.0).replace("No Name", "Default-Material");
                let mut lines = Vec::new();
                let (mut misplaced, mut wrong_slot, mut wrong_part, mut pictures) = (0, 0, 0, 0);
                for (i, s) in us.iter().enumerate() {
                    t.slots += 1;
                    let truth = look(&mut loader, r.materials.get(i), &mut shaders);
                    let model_slot = |j: usize| node.materials.get(j).map(|&k| MatSource::Model { guid: guid.clone(), material: k });
                    let in_slot = look(&mut loader, model_slot(i).as_ref(), &mut shaders);
                    let on_part = look(&mut loader, order[i].and_then(model_slot).as_ref(), &mut shaders);
                    let moved = order[i] != Some(i);
                    misplaced += moved as usize;
                    let bad_slot = name(&truth) != name(&in_slot);
                    wrong_slot += bad_slot as usize;
                    let bad_part = name(&truth) != name(&on_part);
                    wrong_part += bad_part as usize;
                    let picture = !bad_part && truth.rsplit_once(" [").map(|x| x.1) != on_part.rsplit_once(" [").map(|x| x.1);
                    pictures += picture as usize;
                    lines.push(format!(
                        "    unity slot {i} ({} tris) = file slot {:?}: unity {truth:?}, file's slot {i} {in_slot:?}{}{}{}",
                        s.0,
                        order[i],
                        if bad_slot { "  <-- SLOT" } else { "" },
                        if bad_part { "  <-- GEOMETRY" } else { "" },
                        if picture { "  <-- PICTURE" } else { "" }
                    ));
                }
                t.misplaced += misplaced;
                t.wrong_slot += wrong_slot;
                t.wrong_part += wrong_part;
                t.pictures += pictures;
                if misplaced > 0 {
                    t.reordered += 1;
                }
                if misplaced > 0 || wrong_slot > 0 || wrong_part > 0 || pictures > 0 || differ {
                    println!("{rel}: {:?} unity {} submeshes, file {}, order {:?}", go.name, u.submeshes.len(), mine.submeshes.len(), order);
                    if differ {
                        println!("    file's slots: {:?}", ms.iter().zip(&node.materials).map(|(s, &k)| (s.0, m.materials[k].name.as_str())).collect::<Vec<_>>());
                    }
                    lines.iter().for_each(|l| println!("{l}"));
                }
            }
        }
        println!(
            "TOTAL {}: models {} (no current import {}, unreadable {}), renderers {} (several submeshes {}, not in file {}), submeshes differ {}, renderers reordered {}, slots {} (other geometry {}, other material in the slot {}, other material on the geometry {}, same name other picture {}, triangle count off {}), model's own materials {} (ID not its name's {})",
            project.display(),
            t.models,
            t.no_import,
            t.unreadable,
            t.renderers,
            t.multi,
            t.unmatched,
            t.parts_differ,
            t.reordered,
            t.slots,
            t.misplaced,
            t.wrong_slot,
            t.wrong_part,
            t.pictures,
            t.degenerate,
            t.own_materials,
            t.own_unknown
        );
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
    // `--draws <preview file>...`: each draw of built previews, biggest first: its submeshes' triangles and materials,
    // to compare a scene built from the files with one built from Unity's import (diagnostics).
    if args.first().map(String::as_str) == Some("--draws") {
        for path in &args[1..] {
            let h = orbiters_projects::preview::read_header(std::path::Path::new(path)).expect("a preview");
            println!("{path}");
            let mut draws: Vec<_> = h.draws.iter().collect();
            draws.sort_by_key(|d| std::cmp::Reverse(h.meshes[d.mesh as usize].submeshes.iter().map(|s| s.count).sum::<u32>()));
            for d in draws {
                let parts: Vec<String> = h.meshes[d.mesh as usize]
                    .submeshes
                    .iter()
                    .zip(&d.materials)
                    .map(|(s, &m)| format!("{} tris: {}", s.count / 3, if m < 0 { "(not drawn)" } else { h.materials[m as usize].name.as_str() }))
                    .collect();
                let b = h.meshes[d.mesh as usize].bounds;
                let colours: Vec<[f32; 4]> = d.materials.iter().filter(|&&m| m >= 0).map(|&m| h.materials[m as usize].color).collect();
                println!("  {} (bounds {:?}..{:?}, colours {colours:?})", parts.join(" | "), b.min, b.max);
            }
        }
        return;
    }
    // `--materials <project> [part of a path]`: for every model with a current import, which material each submesh gets
    // from the file against Unity's import (diagnostics; the import is the truth, a difference is a bug of the file's path).
    if args.first().map(String::as_str) == Some("--materials") {
        materials::compare(std::path::Path::new(&args[1]), args.get(2).map(String::as_str));
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
