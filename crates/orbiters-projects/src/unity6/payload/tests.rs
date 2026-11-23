use super::*;
use crate::unity6::trial::tests::{env, external, slash, Env};

fn scan_of(e: &Env) -> Payload {
    scan(&e.source, "T", "abcd", &AtomicBool::new(false)).unwrap()
}

fn manifest(e: &Env, text: &str) {
    std::fs::write(e.source.join("Packages/manifest.json"), text).unwrap();
}

fn made(p: &Payload) -> String {
    match &p.items[Path::new(MANIFEST)] {
        Item::Made(b) => String::from_utf8(b.clone()).unwrap(),
        Item::File(o) => String::from_utf8({
            let mut b = vec![];
            p.open(o).unwrap().read_to_end(&mut b).unwrap();
            b
        })
        .unwrap(),
        Item::Dir => panic!(),
    }
}

/// Every way a project names a local package: inside it (relative or whole path), outside, an archive; each ends named
/// inside the trial.
#[test]
fn every_local_package_ends_inside_the_trial() {
    let e = env();
    // Inside, by a relative path out of Packages: kept as it is.
    std::fs::create_dir_all(e.source.join("Local/com.in")).unwrap();
    std::fs::write(e.source.join("Local/com.in/package.json"), r#"{"name":"com.in"}"#).unwrap();
    manifest(&e, r#"{"dependencies":{"com.in":"file:../Local/com.in"}}"#);
    let p = scan_of(&e);
    assert!(p.problems.is_empty() && p.outside.is_empty() && p.localized.is_empty(), "{:?}", p.problems);
    assert!(made(&p).contains("file:../Local/com.in"));
    // Inside, by its whole path: named relative in the trial.
    manifest(&e, &format!(r#"{{"dependencies":{{"com.in":"file:{}"}}}}"#, slash(&e.source.join("Local/com.in"))));
    let p = scan_of(&e);
    assert!(p.problems.is_empty() && p.outside.is_empty(), "{:?}", p.problems);
    assert!(made(&p).contains("file:../Local/com.in") && !made(&p).contains("Projects"), "{}", made(&p));
    // Inside what a copy leaves out.
    std::fs::create_dir_all(e.source.join("Library/com.in")).unwrap();
    std::fs::write(e.source.join("Library/com.in/package.json"), r#"{"name":"com.in"}"#).unwrap();
    manifest(&e, r#"{"dependencies":{"com.in":"file:../Library/com.in"}}"#);
    assert!(!scan_of(&e).problems.is_empty());
    // Outside: a folder.
    let ext = external(&e, "x");
    let p = scan_of(&e);
    assert!(p.problems.is_empty() && p.outside.len() == 1 && p.items.contains_key(Path::new("Packages/com.example.tool/data.txt")));
    assert!(p.protected.iter().any(|x| crate::owned::id_of(&ext).ok() == x.id), "the package's folder is never a trial's place");
    assert!(made(&p).contains("file:com.example.tool"));
    // Outside: an archive, whole and broken.
    let tgz = e.root.join("External/tool.tgz");
    archive(&tgz, r#"{"name":"com.arc"}"#);
    manifest(&e, &format!(r#"{{"dependencies":{{"com.arc":"file:{}"}}}}"#, slash(&tgz)));
    let p = scan_of(&e);
    assert!(p.problems.is_empty() && p.items.contains_key(Path::new("Packages/com.arc.tgz")), "{:?}", p.problems);
    assert!(made(&p).contains("file:com.arc.tgz"));
    archive(&tgz, r#"{"name":"com.arc","dependencies":{"b":"file:../b.tgz"}}"#);
    assert!(scan_of(&e).problems.contains(&"com.arc".to_string()), "an archive asking for another local package");
    std::fs::write(&tgz, "not an archive").unwrap();
    assert!(scan_of(&e).problems.contains(&"com.arc".to_string()));
    // A folder that is no package.
    std::fs::remove_file(ext.join("package.json")).unwrap();
    manifest(&e, &format!(r#"{{"dependencies":{{"com.example.tool":"file:{}"}}}}"#, slash(&ext)));
    assert!(!scan_of(&e).problems.is_empty());
    // Names that are not names.
    for bad in ["a/b", "..", "C:x"] {
        manifest(&e, &format!(r#"{{"dependencies":{{"{bad}":"file:../../../External"}}}}"#));
        assert!(!scan_of(&e).problems.is_empty(), "{bad}");
    }
}

/// A `.tgz` holding one `package/package.json`.
fn archive(at: &Path, json: &str) {
    use std::io::Write;
    let mut tar = vec![];
    for (name, body) in [("package/readme.md", "hello"), ("package/package.json", json)] {
        let mut head = [0u8; 512];
        head[..name.len()].copy_from_slice(name.as_bytes());
        head[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
        tar.extend(head);
        tar.extend(body.as_bytes());
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.extend([0u8; 1024]);
    let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(at).unwrap(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap();
}

/// Links are followed as folders, reported, and what they lead to is never a trial's place, the left-out ones' included.
#[cfg(windows)]
#[test]
fn links_are_followed_reported_and_protected() {
    let e = env();
    let (shared, deeper, library) = (e.root.join("Shared"), e.root.join("Deeper"), e.root.join("Other disk/Library"));
    for d in [&shared, &deeper, &library] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(shared.join("a.mat"), "a").unwrap();
    std::fs::write(deeper.join("b.mat"), "b").unwrap();
    junction::create(&deeper, shared.join("In")).unwrap();
    junction::create(&shared, e.source.join("Assets/Shared")).unwrap();
    std::fs::remove_dir_all(e.source.join("Library")).unwrap();
    junction::create(&library, e.source.join("Library")).unwrap();
    let p = scan_of(&e);
    assert!(p.items.contains_key(Path::new("Assets/Shared/a.mat")) && p.items.contains_key(Path::new("Assets/Shared/In/b.mat")));
    assert!(!p.items.keys().any(|k| k.starts_with("Library")));
    assert_eq!(p.linked.len(), 2, "{:?}", p.linked);
    for d in [&e.source, &shared, &deeper, &library] {
        assert!(p.protected.iter().any(|x| x.id == crate::owned::id_of(d).ok()), "{}", d.display());
    }
    // A link that leads nowhere: the project is not copied without it.
    std::fs::remove_dir_all(&deeper).unwrap();
    assert!(scan(&e.source, "T", "", &AtomicBool::new(false)).is_err());
}

#[test]
fn content_ids_are_found_where_unity_keeps_them() {
    let ids = |t: &str| content_ids(t.as_bytes()).into_iter().collect::<Vec<_>>();
    let a = "avtr_11111111-2222-4333-8444-555555555555";
    let w = "wrld_11111111-2222-4333-8444-555555555555";
    assert_eq!(ids(&format!("%YAML 1.1\n  blueprintId: {a}\n")), [a]);
    assert_eq!(ids(&format!("%YAML 1.1\r\n      propertyPath: blueprintId\r\n      value: {a}\r\n")), [a]);
    assert!(ids(&format!("%YAML 1.1\n  roomId: {w}\n      propertyPath: roomId\n      value: {w}\n")).is_empty(), "a portal to another world");
    assert!(ids("%YAML 1.1\n  blueprintId: \n  blueprintId: avtr_x\n").is_empty());
    // A binary scene: whatever looks like one.
    assert_eq!(ids(&format!("\0\0UnityFS\0{w}\0")), [w]);
}

/// The digests of two trees, through their text form, and every kind of difference.
#[test]
fn every_difference_is_told() {
    let file = |b: &[u8]| digest(b, &mut [0; 8], &AtomicBool::new(false), |_| {}).unwrap();
    let wanted: Digests = [
        (PathBuf::from("d"), Digest::Dir),
        (PathBuf::from("d").join("a name with spaces"), file(b"AAAA")),
        (PathBuf::from("gone"), file(b"")),
        (PathBuf::from("kind"), Digest::Dir),
    ]
    .into();
    assert_eq!(from_text(&to_text(&wanted)).unwrap(), wanted);
    assert!(from_text("nonsense").is_none() && from_text("3 zz x").is_none());
    let mut found = wanted.clone();
    assert!(differences(&wanted, &found, &[]).is_empty());
    found.insert(PathBuf::from("d").join("a name with spaces"), file(b"BBBB"));
    found.remove(Path::new("gone"));
    found.insert(PathBuf::from("kind"), file(b"x"));
    found.insert(PathBuf::from("new"), file(b"x"));
    found.insert(PathBuf::from(crate::templates::INCOMPLETE), file(b"marker"));
    assert_eq!(differences(&wanted, &found, &["link".into()]).len(), 5);
}

/// The content of a project as it is now, its links followed; what cannot be read is an error, not "nothing".
#[test]
fn the_content_of_a_project_is_read_now() {
    let e = env();
    let of = || content_of(&e.source, &AtomicBool::new(false));
    assert_eq!(of().unwrap().into_iter().collect::<Vec<_>>(), [crate::unity6::trial::tests::CONTENT]);
    std::fs::create_dir_all(e.source.join("Packages/com.mine")).unwrap();
    std::fs::write(e.source.join("Packages/com.mine/Avatar.prefab"), "%YAML 1.1\n  blueprintId: avtr_77777777-2222-4333-8444-555555555555\n").unwrap();
    assert_eq!(of().unwrap().len(), 2);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _held = std::fs::OpenOptions::new().read(true).share_mode(0).open(e.source.join("Packages/com.mine/Avatar.prefab")).unwrap();
        assert!(of().is_err());
    }
    assert!(content_of(&e.root.join("Nowhere"), &AtomicBool::new(false)).is_err());
}
