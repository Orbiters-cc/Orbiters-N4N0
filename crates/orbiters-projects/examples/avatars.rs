//! The uploaded avatars and worlds found in Unity projects (`orbiters_projects::avatars::in_project`, what the
//! projects' discovery lists for each), one line of JSON a project, in a fixed order: to compare two builds on the same
//! projects. Read-only: nothing is written anywhere, and each project is read afresh.
//!
//! ```text
//! cargo run --release -p orbiters-projects --example avatars -- <project folder>...
//! ```

fn main() {
    for project in std::env::args().skip(1) {
        let start = std::time::Instant::now();
        let mut found = orbiters_projects::avatars::in_project(std::path::Path::new(&project), &mut Default::default());
        found.sort_by(|a, b| (&a.scene, &a.id, &a.path, &a.name).cmp(&(&b.scene, &b.id, &b.path, &b.name)));
        eprintln!("{project}: {} uploaded avatars and worlds, read in {:.2} s", found.len(), start.elapsed().as_secs_f32());
        println!("{}", serde_json::json!({ "project": project, "found": found }));
    }
}
