use orbiters_projects::{avatars, discovery, setup, templates, vpm};
use std::sync::{atomic::AtomicBool, Arc, Mutex};

#[test]
fn unity_version() {
    assert_eq!(discovery::parse_version("m_EditorVersion: 2022.3.22f1\nm_EditorVersionWithRevision: ignored"), Some("2022.3.22f1".into()));
    assert_eq!(discovery::parse_version("m_EditorVersionWithRevision: nope"), None);
}
#[test]
fn vrchat_detection() {
    assert_eq!(discovery::manifest_kind(r#"{"locked":{"com.vrchat.avatars":{"version":"3.10.0"}}}"#), discovery::Kind::Avatars);
    assert_eq!(discovery::manifest_kind(r#"{"dependencies":{"com.vrchat.worlds":{"version":"3.10.0"}}}"#), discovery::Kind::Worlds);
    assert_eq!(discovery::manifest_kind(r#"{"dependencies":{"other":{}}}"#), discovery::Kind::Unity);
}
#[test]
fn registry_binary() {
    assert_eq!(discovery::decode_registry_path(b"H:\\Projects\\Canine\0garbage").unwrap().to_string_lossy(), "H:\\Projects\\Canine");
    assert!(discovery::decode_registry_path(&[255, 0]).is_none());
    assert!(discovery::decode_registry_path(&[0]).is_none());
}
#[test]
fn scene_avatar() {
    let a = avatars::extract(include_str!("fixtures/avatar.unity"), "Assets/Canine.unity");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].name, "MasculineCanine");
    assert_eq!(a[0].path, "Scene root/MasculineCanine");
    assert_eq!(a[0].id, "avtr_0bf7ea0b-0849-47d5-8a2b-b0a217aa676c");
    assert!(avatars::extract("binary scene", "scene").is_empty());
}
#[test]
fn prefab_override() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("Assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(
        assets.join("Canine.prefab"),
        include_str!("fixtures/avatar.unity").replace("blueprintId: avtr_0bf7ea0b-0849-47d5-8a2b-b0a217aa676c", "blueprintId:"),
    )
    .unwrap();
    std::fs::write(assets.join("Canine.prefab.meta"), "guid: d557a5d29fcedd044a0ecae138acff81\n").unwrap();
    std::fs::write(assets.join("Scene.unity"),"%YAML 1.1\n--- !u!1001 &100\nPrefabInstance:\n  m_Modification:\n    m_TransformParent: {fileID: 0}\n    m_Modifications:\n    - target: {fileID: 6583202450129855070, guid: d557a5d29fcedd044a0ecae138acff81, type: 3}\n      propertyPath: blueprintId\n      value: avtr_0bf7ea0b-0849-47d5-8a2b-b0a217aa676c\n      objectReference: {fileID: 0}\n  m_SourcePrefab: {fileID: 100100000, guid: d557a5d29fcedd044a0ecae138acff81, type: 3}\n").unwrap();
    let mut cache = Default::default();
    let a = avatars::in_project(temp.path(), &mut cache);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].name, "MasculineCanine");
    assert_eq!(avatars::in_project(temp.path(), &mut cache), a);
}
#[test]
fn a_world_scene_gives_its_world_and_my_worlds_thumbnail() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("Assets");
    std::fs::create_dir_all(assets.join("Orbiters/MyWorld/Thumbnails/World")).unwrap();
    let png = assets.join("Orbiters/MyWorld/Thumbnails/World/World thumbnail.png");
    std::fs::write(&png, b"png").unwrap();
    std::fs::write(
        png.with_extension("png.meta"),
        "fileFormatVersion: 2
guid: 0123456789abcdef0123456789abcdef
",
    )
    .unwrap();
    let scene = "%YAML 1.1
--- !u!1 &10
GameObject:
  m_Name: VRCWorld
--- !u!4 &11
Transform:
  m_GameObject: {fileID: 10}
  m_Father: {fileID: 0}
--- !u!114 &12
MonoBehaviour:
  m_GameObject: {fileID: 10}
  blueprintId: wrld_20495852-08c6-4632-9dba-f4320a155e80
--- !u!114 &13
MonoBehaviour:
  m_GameObject: {fileID: 10}
  thumbnail: {fileID: 2800000, guid: 0123456789abcdef0123456789abcdef, type: 3}
  thumbnailCamera: {fileID: 0}
--- !u!114 &14
MonoBehaviour:
  m_GameObject: {fileID: 10}
  thumbnail: {fileID: 1969104925}
";
    std::fs::write(assets.join("World.unity"), scene).unwrap();
    let mut cache = Default::default();
    let found = avatars::in_project(temp.path(), &mut cache);
    assert_eq!(found.len(), 1);
    assert!(found[0].is_world());
    assert_eq!(found[0].name, "VRCWorld");
    assert_eq!(found[0].thumbnail.as_deref(), Some(png.as_path()));
    // Without My World, the world alone.
    std::fs::write(assets.join("World.unity"), scene.split("--- !u!114 &13").next().unwrap()).unwrap();
    let found = avatars::in_project(temp.path(), &mut cache);
    assert_eq!((found.len(), found[0].thumbnail.clone()), (1, None));
}
#[test]
fn skip_rules() {
    for n in [
        "Windows",
        "PROGRAM FILES (x86)",
        "ProgramData",
        "$Recycle.Bin",
        "System Volume Information",
        "AppData",
        "Library",
        "Temp",
        "Logs",
        "obj",
        "node_modules",
        ".git",
    ] {
        assert!(discovery::skip_dir(n), "{n}");
    }
    assert!(!discovery::skip_dir("My avatars"));
}
#[test]
fn setup_request_matches_contract() {
    let temp = tempfile::tempdir().unwrap();
    let avatar = avatars::Avatar {
        id: "avtr_0bf7ea0b-0849-47d5-8a2b-b0a217aa676c".into(),
        name: "MasculineCanine".into(),
        scene: "Assets/MasculineCanine/Canine.unity".into(),
        path: "MasculineCanine".into(),
        ..Default::default()
    };
    let mut r = setup::request(avatar, 30, "https://api.orbiters.cc/", "0.3.3").unwrap();
    r.id = "5f0c4d1e-8a8b-4d58-9a3a-2b7c1c4f0e11".into();
    r.created_utc = "2026-10-29T01:50:00Z".into();
    setup::write(temp.path(), &r).unwrap();
    let p = temp.path().join("UserSettings/Orbiters/SetupRequests").join(format!("{}.json", r.id));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"schema":1,"id":"5f0c4d1e-8a8b-4d58-9a3a-2b7c1c4f0e11","createdUtc":"2026-10-29T01:50:00Z","from":"Orbiters - N4N0 0.3.3","server":"https://api.orbiters.cc/","avatar":{"id":"avtr_0bf7ea0b-0849-47d5-8a2b-b0a217aa676c","name":"MasculineCanine","scene":"Assets/MasculineCanine/Canine.unity","path":"MasculineCanine"},"items":[{"kind":"gallery","assetId":30,"version":null,"name":"N4N0"}],"then":["save-scene","open-build-panel"]})
    );
    assert!(!p.with_extension("json.tmp").exists());
}
#[test]
fn setup_results() {
    let temp = tempfile::tempdir().unwrap();
    let id = "5f0c4d1e-8a8b-4d58-9a3a-2b7c1c4f0e11";
    assert!(setup::take_result(temp.path(), id).unwrap().is_none());
    let p = temp.path().join("UserSettings/Orbiters/SetupRequests").join(format!("{id}.result.json"));
    for status in ["done", "declined", "failed"] {
        orbiters_projects::write_json(&p,&serde_json::json!({"schema":1,"id":id,"status":status,"message":"Result message","finishedUtc":"2026-10-29T01:52:10Z","installed":[{"assetId":30,"version":"1.0.14"}]})).unwrap();
        assert!(setup::read_result(temp.path(), id).unwrap().is_some());
        assert!(p.exists(), "A result must remain until it has been displayed.");
        let r = setup::take_result(temp.path(), id).unwrap().unwrap();
        assert_eq!(r.message, "Result message");
        assert!(!p.exists());
    }
    assert!(setup::take_result(temp.path(), "../../other").is_err());
}
#[test]
fn copy_excludes_generated_folders() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("source");
    for name in ["Assets", "Library", "Temp", "UserSettings", ".git"] {
        std::fs::create_dir_all(src.join(name)).unwrap();
        std::fs::write(src.join(name).join("file"), "content").unwrap();
    }
    let dst = temp.path().join("copy");
    templates::copy(&src, &dst, &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
    assert!(dst.join("Assets/file").exists());
    assert!(!dst.join("Library").exists());
    assert!(!dst.join("UserSettings").exists());
    assert!(templates::copy(&src, &dst, &AtomicBool::new(false), &Arc::new(Mutex::default())).is_err());
}
#[test]
fn shared_registration_uses_temp_store_only() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("ProjectSettings")).unwrap();
    std::fs::create_dir_all(project.path().join("Packages")).unwrap();
    std::fs::write(project.path().join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
    std::fs::write(project.path().join("Packages/vpm-manifest.json"), "{\"dependencies\":{},\"locked\":{}}").unwrap();
    orbiters_projects::runtime().unwrap().block_on(async {
        vpm::register(root.path(), project.path(), true).await.unwrap();
        assert_eq!(vpm::registered(root.path()).await.unwrap().len(), 1);
        vpm::register(root.path(), project.path(), false).await.unwrap();
        assert!(vpm::registered(root.path()).await.unwrap().is_empty());
        assert!(project.path().join("ProjectSettings/ProjectVersion.txt").exists());
    });
}

#[test]
fn fresh_install_has_builtin_templates_without_vcc() {
    let temp = tempfile::tempdir().unwrap();
    let templates = templates::builtins(&temp.path().join("cache")).unwrap();
    assert_eq!(templates.len(), 3);
    for t in &templates {
        assert!(t.path.join("Assets").is_dir());
        assert_eq!(
            discovery::parse_version(&std::fs::read_to_string(t.path.join("ProjectSettings/ProjectVersion.txt")).unwrap()).as_deref(),
            Some("2022.3.22f1")
        );
    }
    let base = templates.iter().find(|t| t.name.contains("Base")).unwrap();
    let manifest = std::fs::read_to_string(base.path.join("Packages/vpm-manifest.json")).unwrap();
    assert_eq!(discovery::manifest_kind(&manifest), discovery::Kind::Vrchat);
    assert_eq!(templates::builtins(&temp.path().join("cache")).unwrap().len(), 3);
}
