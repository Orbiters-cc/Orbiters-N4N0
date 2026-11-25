//! The programs of Creator Companion and ALCOM are protected where they are: in the folder ALCOM's installer was given, and beside a
//! portable Creator Companion. (A process of its own, with folders of its own: nothing real is looked for.)
use orbiters_projects::templates;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn project(p: &Path) -> PathBuf {
    for dir in ["ProjectSettings", "Packages", "Assets"] {
        std::fs::create_dir_all(p.join(dir)).unwrap();
    }
    std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
    std::fs::write(p.join("Packages/vpm-manifest.json"), r#"{"dependencies":{},"locked":{}}"#).unwrap();
    std::fs::write(p.join("Packages/manifest.json"), r#"{"dependencies":{}}"#).unwrap();
    std::fs::write(p.join("Assets/a.txt"), "a").unwrap();
    p.to_path_buf()
}

fn everything(root: &Path) -> Vec<(PathBuf, bool, Vec<u8>)> {
    let mut all: Vec<_> = walkdir::WalkDir::new(root).into_iter().flatten().map(|e| (e.path().to_path_buf(), e.file_type().is_dir(), std::fs::read(e.path()).unwrap_or_default())).collect();
    all.sort();
    all
}

fn duplicate(root: &Path, from: &Path, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let runtime = orbiters_projects::runtime().unwrap();
    runtime.block_on(templates::duplicate(root, from, parent, name, templates::Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())))
}

fn create(root: &Path, from: &Path, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let runtime = orbiters_projects::runtime().unwrap();
    runtime.block_on(templates::create(root, from, parent, name, "2022.3.22f1", true, &AtomicBool::new(false), &Arc::new(Mutex::default())))
}

#[test]
fn programs_are_protected_where_they_are_installed_or_unpacked() {
    let temp = tempfile::tempdir().unwrap();
    let at = |p: &str| temp.path().join(p);
    let roots = orbiters_core::paths::Roots { config: at("config"), local_data: at("local"), cache: at("cache"), documents: at("documents") };
    orbiters_core::paths::set_roots(roots).unwrap();
    let root = orbiters_projects::vcc_dir();
    assert_eq!(root, at("local/VCC"));
    // (Where the stand-in of an installed ALCOM is, as `VCC-app` is Creator Companion's.)
    let (alcom, portable, shared) = (at("local/ALCOM-app"), at("Unpacked/VCC beta"), at("Shared"));
    for dir in [&root, &alcom, &portable.join("VCCUserDataPortable/Repos"), &portable.join("WebApp/Dist"), &shared.join("VCCUserDataPortable"), &at("VCCUserData")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(alcom.join("ALCOM.exe"), "stand-in").unwrap();
    std::fs::write(portable.join("CreatorCompanionBeta.exe"), "stand-in").unwrap();
    std::fs::write(portable.join("VCCUserDataPortable/settings.json"), "{}").unwrap();
    // One unpacked loosely where the person keeps projects too: its data is its own, their folder is theirs.
    std::fs::write(shared.join("CreatorCompanionBeta.exe"), "stand-in").unwrap();
    project(&shared.join("My projects/Mine"));
    let a = project(&at("Projects/A"));
    let link = at("Link");
    #[cfg(windows)]
    junction::create(&portable, &link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&portable, &link).unwrap();

    let theirs = [&alcom, &portable, &shared.join("VCCUserDataPortable")];
    let before: Vec<_> = theirs.iter().map(|d| everything(d)).collect();
    let mut let_through = vec![];
    let mut parents = vec![alcom.clone(), alcom.join("Sub/Deeper"), portable.clone(), portable.join("VCCUserDataPortable"), portable.join("VCCUserDataPortable/Repos"), portable.join("New"), link.clone(), link.join("VCCUserDataPortable"), shared.join("VCCUserDataPortable"), shared.join("My projects/../VCCUserDataPortable")];
    if cfg!(windows) {
        parents.extend([PathBuf::from(alcom.to_string_lossy().to_uppercase()), shared.join("vccuserdataportable"), PathBuf::from(format!(r"\?\{}", portable.display()))]);
    }
    for (i, parent) in parents.iter().enumerate() {
        for (how, made) in [("duplicate", duplicate(&root, &a, parent, &format!("D{i}"))), ("create", create(&root, &a, parent, &format!("C{i}")))] {
            if made.is_ok() {
                let_through.push(format!("{how} in {}", parent.display()));
            }
        }
    }
    for dir in [&alcom, &portable.join("VCCUserDataPortable"), &shared.join("VCCUserDataPortable")] {
        if orbiters_projects::write_json(&dir.join("settings.json"), &1).is_ok() {
            let_through.push(format!("a file in {}", dir.display()));
        }
    }
    let changed: Vec<_> = theirs.iter().zip(&before).filter(|(d, was)| everything(d) != **was).map(|(d, _)| d.display().to_string()).collect();
    assert!(let_through.is_empty() && changed.is_empty(), "let through: {let_through:#?}\nchanged: {changed:#?}");

    // What is not theirs goes on as ever: beside the program's folder, where the person keeps projects beside a portable one, a like name.
    for (parent, name) in [(at("Unpacked"), "Beside"), (shared.join("My projects"), "Copy"), (shared.clone(), "Top"), (at("VCCUserData"), "Like"), (at("Projects"), "Plain")] {
        let copy = duplicate(&root, &a, &parent, name).unwrap_or_else(|e| panic!("{}: {e:#}", parent.display()));
        assert!(copy.join("Assets/a.txt").is_file());
    }
}
