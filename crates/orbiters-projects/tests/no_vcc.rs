//! A computer without Creator Companion: an empty VPM folder (what `ORBITERS_VCC_DIR` points the app at to test it).
//! Everything the Unity projects page does starts from nothing, and makes nothing there: what it keeps is Orbiters' own.
use orbiters_projects::{gallery, templates, vpm};
use serde_json::json;
use std::path::Path;

fn project(path: &Path) {
    std::fs::create_dir_all(path.join("ProjectSettings")).unwrap();
    std::fs::create_dir_all(path.join("Packages")).unwrap();
    std::fs::write(path.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
    std::fs::write(path.join("Packages/vpm-manifest.json"), r#"{"dependencies":{},"locked":{}}"#).unwrap();
    std::fs::write(path.join("Packages/manifest.json"), r#"{"dependencies":{}}"#).unwrap();
}

#[test]
fn the_page_works_from_an_empty_vpm_folder() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().join("VRChatCreatorCompanion");
    let projects = tempfile::tempdir().unwrap();
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        // Nothing there yet: no project, no error.
        assert!(gallery::listed(&root).projects.is_empty());
        // VRChat's repositories are left out here (no network in tests): the listing is empty but readable.
        std::fs::create_dir_all(root.join("vrc-get")).unwrap();
        std::fs::write(root.join("vrc-get/settings.json"), json!({"ignoreOfficialRepository":true,"ignoreCuratedRepository":true}).to_string()).unwrap();
        let view = vpm::view(&root, None, false).await.unwrap();
        assert!(view.repos.is_empty() && view.packages.is_empty());
        assert!(view.default_path.is_absolute());

        // Adding a project writes nothing there: the list is Orbiters' own.
        let a = projects.path().join("Avatar project");
        project(&a);
        gallery::add(&root, &a).unwrap();
        assert!(!root.join("settings.json").exists() && !root.join("vcc.liteDb").exists() && !root.join("Repos").exists());
        assert_eq!(gallery::listed(&root).projects, [a.clone()]);
        assert!(!gallery::listed(&root).companion);
        // Its packages read as any project's.
        assert!(vpm::view(&root, Some(&a), false).await.unwrap().packages.is_empty());
        gallery::remove(&root, &a).unwrap();
        assert!(gallery::listed(&root).projects.is_empty());
    });
    std::fs::remove_dir_all(orbiters_projects::own::dir(&root)).unwrap();

    // Templates come with the app: Avatar, World and Base, without VCC's covers.
    let cache = tempfile::tempdir().unwrap();
    let list = templates::builtins(cache.path()).unwrap();
    let ids: Vec<&str> = list.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["com.vrchat.template.avatar", "com.vrchat.template.world", "com.vrchat.template.base"]);
    assert!(list.iter().all(|t| t.banner.is_none() && t.path.join("ProjectSettings/ProjectVersion.txt").is_file()));
}

#[test]
fn template_covers_come_from_creator_companion_when_installed() {
    let app = tempfile::tempdir().unwrap();
    let covers = app.path().join("WebApp/Dist/template-covers");
    std::fs::create_dir_all(&covers).unwrap();
    for f in ["template-avatar-2022.png", "template-world-2022.png", "template-custom.png"] {
        std::fs::write(covers.join(f), b"png").unwrap();
    }
    let cover = |id: &str| templates::banner(app.path(), id).map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
    assert_eq!(cover("com.vrchat.template.avatar").as_deref(), Some("template-avatar-2022.png"));
    assert_eq!(cover("com.vrchat.template.world").as_deref(), Some("template-world-2022.png"));
    // VCC's own rule: any other template (Base, a person's own) gets the Custom cover.
    assert_eq!(cover("com.vrchat.template.base").as_deref(), Some("template-custom.png"));
    assert_eq!(cover("com.vrchat.template.ultinova").as_deref(), Some("template-custom.png"));
    // A cover VCC does not have: none (the card keeps its icon).
    assert_eq!(cover("com.vrchat.template.avatar.2019"), None);
}

/// Downloads VRChat's official repository into an empty folder, as the page does on a computer without VCC.
/// Network: `cargo test -p orbiters-projects --test no_vcc -- --ignored`.
#[test]
#[ignore]
fn vrchat_repositories_download_on_first_look() {
    let root = tempfile::tempdir().unwrap();
    let view = orbiters_projects::runtime().unwrap().block_on(vpm::view(root.path(), None, false)).unwrap();
    assert!(root.path().join("Repos/vrc-official.json").is_file());
    assert!(view.packages.iter().any(|p| p.name == "com.vrchat.avatars"));
    assert!(view.repos.iter().any(|r| r.builtin));
}
