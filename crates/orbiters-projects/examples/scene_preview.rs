//! Builds the previews of Unity projects' scenes from their files and prints what each build found, read-only: the
//! projects are only read; previews go to the app's cache, or to `ORBITERS_PREVIEW_CACHE` when set.
//!
//! `cargo run -p orbiters-projects --example scene_preview -- [--scenes N] [--seconds S] <project>...`
//! (`--scenes` builds the first N scenes of each project, 1 by default.)
use orbiters_projects::scene;
use std::time::{Duration, Instant};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
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
    println!("| Project | Scenes | Scene | Objects | Renderers drawn | Models | Meshes | Materials | Textures | Triangles | Build | File | Result |");
    println!("| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |");
    for project in &args {
        let path = std::path::Path::new(project);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let t = Instant::now();
        let list = scene::scenes(path);
        let list_ms = t.elapsed().as_secs_f64() * 1000.0;
        if list.is_empty() {
            println!("| {name} | 0 | | | | | | | | | | | no scene ({list_ms:.0} ms to look) |");
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
                        "| {name} | {} | {}{mark} | {} | {} of {} | {} | {} | {} | {} | {} | {:.1} s | {:.1} MB | ok{} |",
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
                        if rep.missing.is_empty() { String::new() } else { format!(", {} unresolved", rep.missing.len()) }
                    );
                    eprintln!("  {} -> {}", s.rel, e.path.display());
                    for m in rep.missing.iter().take(6) {
                        eprintln!("    unresolved: {m}");
                    }
                }
                Err(why) => println!("| {name} | {} | {}{mark} | | | | | | | | {:.1} s | | No preview: {why} |", list.len(), s.name, ms / 1000.0),
            }
        }
    }
}
