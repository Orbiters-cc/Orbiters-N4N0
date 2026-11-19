//! Exercise the real resolver/installer against a loopback repository and disposable projects.
use orbiters_projects::{templates, vpm};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

fn archive(name: &str, version: &str) -> Vec<u8> {
    // A single stored ZIP member. Keep the fixture independent of the production unzip implementation.
    let data = serde_json::to_vec(&json!({"name":name,"version":version,"displayName":name})).unwrap();
    let file = b"package.json";
    let mut crc = 0xffff_ffffu32;
    for b in &data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    let crc = !crc;
    let mut out = vec![];
    let u16s = |out: &mut Vec<u8>, v: u16| out.extend(v.to_le_bytes());
    let u32s = |out: &mut Vec<u8>, v: u32| out.extend(v.to_le_bytes());
    u32s(&mut out, 0x04034b50);
    for n in [20, 0, 0, 0, 0] {
        u16s(&mut out, n);
    }
    for n in [crc, data.len() as u32, data.len() as u32] {
        u32s(&mut out, n);
    }
    u16s(&mut out, file.len() as u16);
    u16s(&mut out, 0);
    out.extend(file);
    out.extend(&data);
    let start = out.len();
    u32s(&mut out, 0x02014b50);
    for n in [20, 20, 0, 0, 0, 0] {
        u16s(&mut out, n);
    }
    for n in [crc, data.len() as u32, data.len() as u32] {
        u32s(&mut out, n);
    }
    for n in [file.len() as u16, 0, 0, 0, 0] {
        u16s(&mut out, n);
    }
    u32s(&mut out, 0);
    u32s(&mut out, 0);
    out.extend(file);
    let len = out.len() - start;
    u32s(&mut out, 0x06054b50);
    for n in [0, 0, 1, 1] {
        u16s(&mut out, n);
    }
    u32s(&mut out, len as u32);
    u32s(&mut out, start as u32);
    u16s(&mut out, 0);
    out
}
struct Repository {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Repository {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}
/// A loopback HTTP server answering each request with `answer(request)`, a body sent with 200 OK.
fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Repository) {
    serve_raw(move |request| {
        let body = answer(request);
        let mut out = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n", body.len()).into_bytes();
        out.extend(body);
        out
    })
}
/// A loopback HTTP server answering each request with the whole response `answer(request)` gives.
fn serve_raw(answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Repository) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let worker = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            // On Windows a connection taken from a listener that does not wait does not wait either: a read before the
            // request had come ended at once, and the answer was an empty body ("EOF while parsing a value").
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut data = vec![];
            let mut buf = [0; 2048];
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let _ = stream.write_all(&answer(&String::from_utf8_lossy(&data)));
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    (base.clone(), Repository { url: format!("{base}/index.json"), stop, worker: Some(worker) })
}
/// A repository whose archives are at `archives` (its own address when `None`).
fn repository_with_archives(archives: Option<&str>) -> Repository {
    let manifest = Arc::new(Mutex::new(Vec::new()));
    let listing = manifest.clone();
    let (base, repository) = serve(move |request| {
        let path = request.split_whitespace().nth(1).unwrap_or_default();
        let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if path == "/index.json" {
            assert!(request.to_ascii_lowercase().contains("x-fixture: enabled"));
            listing.lock().unwrap().clone()
        } else if parts.len() == 2 {
            archive(parts[0], parts[1].trim_end_matches(".zip"))
        } else {
            vec![]
        }
    });
    let at = archives.unwrap_or(&base).to_string();
    let package = |name: &str, version: &str, dep: bool| json!({"name":name,"version":version,"displayName":name,"url":format!("{at}/{name}/{version}.zip"),"vpmDependencies":if dep{json!({"test.base":">=1.0.0"})}else{json!({})}});
    *manifest.lock().unwrap() = serde_json::to_vec(&json!({"id":"test.repo","name":"Disposable repository","url":format!("{base}/index.json"),"packages":{"test.avatar":{"versions":{"1.0.0":package("test.avatar","1.0.0",true),"2.0.0":package("test.avatar","2.0.0",true)}},"test.base":{"versions":{"1.0.0":package("test.base","1.0.0",false)}}}})).unwrap();
    repository
}
fn repository() -> Repository {
    repository_with_archives(None)
}
fn project(path: &Path) {
    std::fs::create_dir_all(path.join("ProjectSettings")).unwrap();
    std::fs::create_dir_all(path.join("Packages")).unwrap();
    std::fs::create_dir_all(path.join("Assets")).unwrap();
    std::fs::write(path.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
    std::fs::write(path.join("Packages/vpm-manifest.json"), r#"{"dependencies":{},"locked":{}}"#).unwrap();
    std::fs::write(path.join("Packages/manifest.json"), r#"{"dependencies":{}}"#).unwrap();
}
fn installed(path: &Path, name: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("Packages/vpm-manifest.json")).unwrap()).unwrap();
    v["locked"][name]["version"].as_str().map(ToOwned::to_owned)
}

#[test]
fn repositories_packages_restore_and_templates_work_in_isolation() {
    let repo = repository();
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("Source");
    project(&source);
    orbiters_projects::write_json(&root.path().join("vrc-get/settings.json"), &json!({"ignoreOfficialRepository":true,"ignoreCuratedRepository":true}))
        .unwrap();
    orbiters_projects::write_json(&root.path().join("vrc-get/gui-config.json"), &json!({"language":"en","guiHiddenRepositories":[]})).unwrap();
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        vpm::add_repo(root.path(), &repo.url, BTreeMap::from([("X-Fixture".into(), "enabled".into())])).await.unwrap();
        let view = vpm::view(root.path(), None, true).await.unwrap();
        assert_eq!(view.repos.len(), 1);
        assert_eq!(view.packages.iter().find(|p| p.name == "test.avatar").unwrap().versions, vec!["2.0.0", "1.0.0"]);
        vpm::set_enabled(root.path(), "test.repo", false).await.unwrap();
        assert!(vpm::view(root.path(), None, false).await.unwrap().packages.is_empty());
        vpm::set_enabled(root.path(), "test.repo", true).await.unwrap();
        for version in ["1.0.0", "2.0.0", "1.0.0"] {
            vpm::change(root.path(), &source, vpm::Change::Install { names: vec![("test.avatar".into(), Some(version.into()))], prerelease: false })
                .await
                .unwrap();
            assert_eq!(installed(&source, "test.avatar").as_deref(), Some(version));
            assert!(source.join("Packages/test.base/package.json").is_file());
        }
        assert!(vpm::change(root.path(), &source, vpm::Change::Remove("test.base".into())).await.is_err());
        assert!(source.join("Packages/test.base/package.json").is_file());
        std::fs::remove_dir_all(source.join("Packages/test.avatar")).unwrap();
        vpm::change(root.path(), &source, vpm::Change::Resolve).await.unwrap();
        assert!(source.join("Packages/test.avatar/package.json").is_file());
        let progress = Arc::new(Mutex::default());
        let cancel = AtomicBool::new(false);
        let copy = templates::create(root.path(), &source, projects.path(), "Copy", "2022.3.22f1", true, &cancel, &progress).await.unwrap();
        assert!(copy.join("Packages/test.avatar/package.json").is_file());
        assert_eq!(vpm::registered(root.path()).await.unwrap().len(), 1);
        let created = templates::create(root.path(), &source, projects.path(), "New from template", "2022.3.22f1", false, &cancel, &progress).await.unwrap();
        assert!(created.join("ProjectSettings/ProjectVersion.txt").is_file());
        assert_eq!(vpm::registered(root.path()).await.unwrap().len(), 2);
        // The panel's Duplicate: Library taken along, the product renamed where it carried the project's name.
        std::fs::create_dir_all(source.join("Library")).unwrap();
        std::fs::write(source.join("Library/ArtifactDB"), "artifacts").unwrap();
        std::fs::write(source.join("ProjectSettings/ProjectSettings.asset"), "PlayerSettings:\n  productGUID: 00000000000000000000000000000001\n  productName: Source\n")
            .unwrap();
        let keep = templates::Keep { library: true };
        let twin = templates::duplicate(root.path(), &source, projects.path(), "Source copy", keep, &cancel, &progress).await.unwrap();
        assert!(twin.join("Library/ArtifactDB").is_file());
        assert!(twin.join("Packages/test.avatar/package.json").is_file());
        let settings = std::fs::read_to_string(twin.join("ProjectSettings/ProjectSettings.asset")).unwrap();
        assert!(settings.contains("productName: \"Source copy\"") && !settings.contains("00000000000000000000000000000001"), "{settings}");
        assert_eq!(vpm::registered(root.path()).await.unwrap().len(), 3);
        assert!(templates::duplicate(root.path(), &source, projects.path(), "Source copy", keep, &cancel, &progress).await.is_err(), "never over a folder");
        vpm::change(root.path(), &source, vpm::Change::Remove("test.avatar".into())).await.unwrap();
        assert!(installed(&source, "test.avatar").is_none());
        vpm::remove_repo(root.path(), "test.repo").await.unwrap();
        assert!(vpm::view(root.path(), None, false).await.unwrap().repos.is_empty());
    });
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(root.path().join("vrc-get/gui-config.json")).unwrap()).unwrap();
    assert_eq!(config["language"], "en");
}

/// AS-11: a private repository's header (its credential) is never sent to the host its archives are on.
#[test]
fn a_repositorys_credential_stays_on_its_own_origin() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    let (cdn, _cdn) = serve(move |request| {
        log.lock().unwrap().push(request.to_ascii_lowercase());
        let path = request.split_whitespace().nth(1).unwrap_or_default();
        let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if parts.len() == 2 { archive(parts[0], parts[1].trim_end_matches(".zip")) } else { vec![] }
    });
    let repo = repository_with_archives(Some(&cdn));
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("Source");
    project(&source);
    orbiters_projects::write_json(&root.path().join("vrc-get/settings.json"), &json!({"ignoreOfficialRepository":true,"ignoreCuratedRepository":true}))
        .unwrap();
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        vpm::add_repo(root.path(), &repo.url, BTreeMap::from([("X-Fixture".into(), "enabled".into())])).await.unwrap();
        vpm::change(root.path(), &source, vpm::Change::Install { names: vec![("test.avatar".into(), Some("1.0.0".into()))], prerelease: false })
            .await
            .unwrap();
    });
    assert_eq!(installed(&source, "test.avatar").as_deref(), Some("1.0.0"));
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 2, "both archives came from the other host: {seen:?}");
    assert!(seen.iter().all(|r| !r.contains("x-fixture")), "the credential went to another host: {seen:?}");
}

/// Verifier, point 5: a listing whose address redirects to another host: the repository's header is not sent there,
/// while the listing still arrives.
#[test]
fn a_redirected_listing_never_takes_the_credential_along() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    let listing = Arc::new(Mutex::new(Vec::new()));
    let body = listing.clone();
    let (other, _other) = serve(move |request| {
        log.lock().unwrap().push(request.to_ascii_lowercase());
        body.lock().unwrap().clone()
    });
    *listing.lock().unwrap() = serde_json::to_vec(&json!({"id":"test.moved","name":"Moved","url":format!("{other}/index.json"),"packages":{}})).unwrap();
    let target = format!("{other}/index.json");
    let (first, _first) = serve_raw(move |_| format!("HTTP/1.0 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes());
    let root = tempfile::tempdir().unwrap();
    orbiters_projects::write_json(&root.path().join("vrc-get/settings.json"), &json!({"ignoreOfficialRepository":true,"ignoreCuratedRepository":true}))
        .unwrap();
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        vpm::add_repo(root.path(), &format!("{first}/index.json"), BTreeMap::from([("X-Fixture".into(), "secret".into())])).await.unwrap();
        vpm::view(root.path(), None, true).await.unwrap();
    });
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty(), "the listing was fetched from where it moved");
    assert!(seen.iter().all(|r| !r.contains("x-fixture")), "the credential went to another host: {seen:?}");
}
