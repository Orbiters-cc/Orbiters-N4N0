//! Package updates and missing packages' repositories against loopback repositories and disposable projects (no
//! request leaves the computer): which update each package can take (releases or pre-releases, the other packages'
//! ranges, never the VRChat SDK behind the person's back), Update all applied through `vpm::change`, and a project
//! needing packages no repository of the person lists: found in a catalog, the repository added, resolved again.
use orbiters_projects::{coherent, missing, updates, vpm};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A ZIP with one stored `package.json`.
fn archive(manifest: &Value) -> Vec<u8> {
    let data = serde_json::to_vec(manifest).unwrap();
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

struct Server {
    base: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}
/// Serves listings (`/<file>.json`) of `packages` (listing file → name → versions with their dependencies), and the
/// packages' archives.
fn serve(listings: Vec<(&'static str, &'static str, Vec<(&'static str, &'static str, Value)>)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    for (file, id, packages) in listings {
        let mut by_name: serde_json::Map<String, Value> = Default::default();
        for (name, version, deps) in packages {
            let manifest = json!({"name":name,"version":version,"displayName":format!("{name} display"),"url":format!("{base}/zip/{name}/{version}.zip"),"vpmDependencies":deps});
            files.insert(format!("/zip/{name}/{version}.zip"), archive(&manifest));
            by_name.entry(name.to_string()).or_insert_with(|| json!({"versions":{}}))["versions"][version] = manifest;
        }
        let listing = json!({"id":id,"name":format!("{id} listing"),"url":format!("{base}/{file}"),"packages":by_name});
        files.insert(format!("/{file}"), serde_json::to_vec(&listing).unwrap());
    }
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
            let request = String::from_utf8_lossy(&data);
            let path = request.split_whitespace().nth(1).unwrap_or_default().to_string();
            let (status, body) = match files.get(&path) {
                Some(b) => ("200 OK", b.clone()),
                None => ("404 Not Found", vec![]),
            };
            let header = format!("HTTP/1.0 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n", body.len());
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    Server { base, stop, worker: Some(worker) }
}

fn vcc(root: &Path) {
    orbiters_projects::write_json(&root.join("vrc-get/settings.json"), &json!({"ignoreOfficialRepository":true,"ignoreCuratedRepository":true})).unwrap();
    orbiters_projects::write_json(&root.join("vrc-get/gui-config.json"), &json!({"guiHiddenRepositories":[]})).unwrap();
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
    let v: Value = serde_json::from_slice(&std::fs::read(path.join("Packages/vpm-manifest.json")).unwrap()).unwrap();
    v["locked"][name]["version"].as_str().map(ToOwned::to_owned)
}
async fn install(root: &Path, path: &Path, names: &[(&str, &str)]) -> anyhow::Result<()> {
    let names = names.iter().map(|(n, v)| (n.to_string(), Some(v.to_string()))).collect();
    vpm::change(root, path, vpm::Change::Install { names, prerelease: false }).await
}

#[test]
fn updates_respect_releases_ranges_and_the_sdk_and_update_all_applies_them() {
    let none = json!({});
    let server = serve(vec![(
        "index.json",
        "test.repo",
        vec![
            ("com.vrchat.base", "3.8.0", none.clone()),
            ("com.vrchat.base", "3.9.0", none.clone()),
            ("test.toolkit", "0.4.10", none.clone()),
            ("test.toolkit", "0.4.11", none.clone()),
            ("test.toolkit", "0.5.0-beta.1", none.clone()),
            ("test.myavatar", "0.9.18", json!({"test.toolkit":">=0.4.10"})),
            // Needs the newer Toolkit: brought along.
            ("test.myavatar", "0.9.19", json!({"test.toolkit":">=0.4.11"})),
            // Needs a newer VRChat SDK: not offered, never moved behind the person's back.
            ("test.myavatar", "0.10.0", json!({"test.toolkit":">=0.4.11","com.vrchat.base":">=3.9.0"})),
            ("test.mixer", "0.2.3", none.clone()),
            ("test.mixer", "0.2.4", none.clone()),
            ("test.mixer", "0.3.0", none.clone()),
            // Holds Mixer under 0.3.
            ("test.addon", "1.0.0", json!({"test.mixer":">=0.2.0 <0.3.0"})),
            ("test.latest", "1.0.0", none.clone()),
        ],
    )]);
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("Avatar project");
    project(&source);
    vcc(root.path());
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        vpm::add_repo(root.path(), &format!("{}/index.json", server.base), Default::default()).await.unwrap();
        install(
            root.path(),
            &source,
            &[("com.vrchat.base", "3.8.0"), ("test.toolkit", "0.4.10"), ("test.myavatar", "0.9.18"), ("test.mixer", "0.2.3"), ("test.addon", "1.0.0"), ("test.latest", "1.0.0")],
        )
        .await
        .unwrap();
        let check = updates::check(root.path(), &source).await.unwrap();
        assert!(check.skipped.is_none());
        let stable = &check.stable;
        let to = |u: &updates::Updates, n: &str| u.of(n).map(|u| u.to.clone());
        assert_eq!(to(stable, "com.vrchat.base").as_deref(), Some("3.9.0"));
        assert!(stable.of("com.vrchat.base").unwrap().sdk);
        assert_eq!(to(stable, "test.toolkit").as_deref(), Some("0.4.11"), "no pre-release unless asked for");
        assert_eq!(to(stable, "test.myavatar").as_deref(), Some("0.9.19"), "0.10.0 would move the SDK");
        let my = stable.of("test.myavatar").unwrap();
        assert_eq!(my.also().map(|s| (s.name.as_str(), s.to.as_deref())).collect::<Vec<_>>(), [("test.toolkit", Some("0.4.11"))]);
        assert_eq!(to(stable, "test.mixer").as_deref(), Some("0.2.4"), "Add-on holds Mixer under 0.3");
        assert_eq!(to(stable, "test.latest"), None, "already the latest");
        assert_eq!(to(stable, "test.addon"), None);
        let held: Vec<(&str, &str)> = stable.held.iter().map(|h| (h.name.as_str(), h.version.as_str())).collect();
        assert!(held.contains(&("test.myavatar", "0.10.0")) && held.contains(&("test.mixer", "0.3.0")), "{held:?}");
        let why = |n: &str| stable.held.iter().find(|h| h.name == n).unwrap().why.clone();
        assert!(why("test.myavatar").contains("com.vrchat.base"), "{}", why("test.myavatar"));
        assert!(why("test.mixer").contains("conflicts"), "{}", why("test.mixer"));
        assert_eq!(to(&check.prerelease, "test.toolkit").as_deref(), Some("0.5.0-beta.1"));

        // Update all: everything but the SDK, at once.
        let plan = &stable.all;
        assert_eq!(plan.left_out, ["com.vrchat.base"]);
        let mut targets = plan.targets.clone();
        targets.sort();
        assert_eq!(targets, [("test.mixer".into(), "0.2.4".into()), ("test.myavatar".into(), "0.9.19".into()), ("test.toolkit".into(), "0.4.11".into())]);
        assert!(!plan.steps.iter().any(|s| updates::is_sdk(&s.name)));
        let names = plan.targets.iter().map(|(n, v)| (n.clone(), Some(v.clone()))).collect();
        vpm::change(root.path(), &source, vpm::Change::Install { names, prerelease: false }).await.unwrap();
        for (n, v) in [("test.toolkit", "0.4.11"), ("test.myavatar", "0.9.19"), ("test.mixer", "0.2.4"), ("com.vrchat.base", "3.8.0")] {
            assert_eq!(installed(&source, n).as_deref(), Some(v), "{n}");
        }
        let after = updates::check(root.path(), &source).await.unwrap();
        assert_eq!(after.stable.updates.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["com.vrchat.base"]);
        assert!(after.stable.all.targets.is_empty());
        // The SDK's own update, asked for from its row, may move the SDK.
        install(root.path(), &source, &[("com.vrchat.base", "3.9.0")]).await.unwrap();
        let last = updates::check(root.path(), &source).await.unwrap();
        assert_eq!(to(&last.stable, "test.myavatar").as_deref(), Some("0.10.0"));
    });
}

#[test]
fn missing_packages_repositories_are_added_and_the_project_resolves() {
    let server = serve(vec![
        ("index.json", "test.repo", vec![("test.base", "1.0.0", json!({}))]),
        ("extra.json", "test.extra", vec![("test.osc", "1.1.5", json!({})), ("test.osc", "1.2.0", json!({}))]),
    ]);
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("Copied project");
    project(&source);
    vcc(root.path());
    let extra = format!("{}/extra.json", server.base);
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        vpm::add_repo(root.path(), &format!("{}/index.json", server.base), Default::default()).await.unwrap();
        // A project from another computer: it locks packages none of this computer's repositories list.
        std::fs::write(
            source.join("Packages/vpm-manifest.json"),
            json!({"dependencies":{"test.osc":{"version":"1.1.5"},"test.gone":{"version":"2.0.0"}},
                   "locked":{"test.osc":{"version":"1.1.5","dependencies":{}},"test.gone":{"version":"2.0.0","dependencies":{}}}})
            .to_string(),
        )
        .unwrap();
        let error = format!("{:#}", vpm::change(root.path(), &source, vpm::Change::Resolve).await.unwrap_err());
        let needed = missing::parse(&error);
        let mut names: Vec<(&str, &str)> = needed.iter().map(|m| (m.name.as_str(), m.range.as_str())).collect();
        names.sort();
        assert_eq!(names, [("test.gone", "=2.0.0"), ("test.osc", "=1.1.5")], "{error}");
        // The catalog keeps the newest version only; the repository itself lists 1.1.5.
        let catalog = json!({"packages":[{"id":"test.osc","displayName":"OSC","repositoryUrl":extra,"versions":[{"version":"1.2.0"}]}]});
        let names = [orbiters_projects::network::Listing { name: "Extra packages".into(), url: extra.clone(), ..Default::default() }];
        let sources = missing::known_sources(&catalog, &names);
        let read = |url: &str, package: &str| -> Option<Vec<String>> {
            let body = reqwest_blocking(url)?;
            missing::listing_versions(&serde_json::from_slice(&body).ok()?).remove(package)
        };
        let lookup = missing::find(&needed, &sources, read);
        assert_eq!(lookup.found.len(), 1);
        assert_eq!((lookup.found[0].package.name.as_str(), lookup.found[0].version.as_str()), ("test.osc", "1.1.5"));
        assert_eq!(lookup.not_provided.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["test.gone"]);
        let additions = lookup.additions();
        assert_eq!(additions.len(), 1);
        assert_eq!((additions[0].name.as_str(), additions[0].url.as_str()), ("Extra packages", extra.as_str()));
        missing::add(root.path(), &additions).await.unwrap();
        // Still missing what no repository has, and said so.
        let error = format!("{:#}", vpm::change(root.path(), &source, vpm::Change::Resolve).await.unwrap_err());
        assert_eq!(missing::parse(&error).iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["test.gone"]);
        // Without it, the retry resolves.
        std::fs::write(
            source.join("Packages/vpm-manifest.json"),
            json!({"dependencies":{"test.osc":{"version":"1.1.5"}},"locked":{"test.osc":{"version":"1.1.5","dependencies":{}}}}).to_string(),
        )
        .unwrap();
        vpm::change(root.path(), &source, vpm::Change::Resolve).await.unwrap();
        assert!(source.join("Packages/test.osc/package.json").is_file());
        assert_eq!(installed(&source, "test.osc").as_deref(), Some("1.1.5"), "the pinned version, not the newest");
        // Added twice (a second press): nothing changes.
        missing::add(root.path(), &additions).await.unwrap();
        let view = vpm::view(root.path(), None, false).await.unwrap();
        assert_eq!(view.repos.iter().filter(|r| missing::same_url(&r.url, &extra)).count(), 1);
        // A repository of the person's turned off: turned back on instead of added again.
        vpm::set_enabled(root.path(), "test.extra", false).await.unwrap();
        missing::add(root.path(), &[missing::Addition { name: "Extra".into(), url: extra.clone(), turn_on: None, packages: vec![] }]).await.unwrap();
        assert!(vpm::view(root.path(), None, false).await.unwrap().repos.iter().find(|r| r.id == "test.extra").unwrap().enabled);
    });
}

/// A plain GET on the loopback server (the test's own reader of a listing).
fn reqwest_blocking(url: &str) -> Option<Vec<u8>> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/')?;
    let mut s = std::net::TcpStream::connect(host).ok()?;
    write!(s, "GET /{path} HTTP/1.0\r\nHost: {host}\r\n\r\n").ok()?;
    let mut data = vec![];
    s.read_to_end(&mut data).ok()?;
    let at = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some(data[at + 4..].to_vec())
}

/// The N4N0 sample avatar's packages: the VRChat SDK 3.7.5 and tools pinning it below 3.8.
fn sample_avatar(osc_update: bool) -> Server {
    let mut packages = vec![
        ("com.vrchat.base", "3.7.5", json!({})),
        ("com.vrchat.base", "3.9.0", json!({})),
        ("com.vrchat.avatars", "3.7.5", json!({"com.vrchat.base":"3.7.5"})),
        ("com.vrchat.avatars", "3.9.0", json!({"com.vrchat.base":"3.9.0"})),
        ("vrchat.blackstartx.gesture-manager", "3.9.3", json!({"com.vrchat.avatars":">=3.5.2 <3.8.X"})),
        ("vrchat.blackstartx.gesture-manager", "3.9.4", json!({"com.vrchat.avatars":">=3.5.2 <3.10.X"})),
        ("dev.vrlabs.av3manager", "2.0.34", json!({"com.vrchat.avatars":"3.2 - 3.7"})),
        ("dev.vrlabs.av3manager", "2.1.0", json!({"com.vrchat.avatars":"3.2 - 3.9"})),
        ("dev.hai-vr.fuck-osc-config", "1.1.5", json!({"com.vrchat.avatars":">=3.1.0 <3.8.0-a"})),
    ];
    if osc_update {
        packages.push(("dev.hai-vr.fuck-osc-config", "1.2.0", json!({"com.vrchat.avatars":">=3.1.0 <3.10.0-a"})));
    }
    serve(vec![("index.json", "test.repo", packages)])
}
async fn sample_project(root: &Path, server: &Server, path: &Path) {
    project(path);
    vcc(root);
    vpm::add_repo(root, &format!("{}/index.json", server.base), Default::default()).await.unwrap();
    install(
        root,
        path,
        &[("com.vrchat.avatars", "3.7.5"), ("vrchat.blackstartx.gesture-manager", "3.9.3"), ("dev.vrlabs.av3manager", "2.0.34"), ("dev.hai-vr.fuck-osc-config", "1.1.5")],
    )
    .await
    .unwrap();
    assert_eq!(installed(path, "com.vrchat.base").as_deref(), Some("3.7.5"));
}
fn conflicted(e: anyhow::Error) -> coherent::Plan {
    e.downcast::<coherent::Conflicted>().unwrap_or_else(|e| panic!("not a conflict: {e:#}")).plan
}

#[test]
fn a_conflicting_sdk_update_becomes_a_coherent_plan_applied_at_once() {
    let server = sample_avatar(true);
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("N4N0 sample avatar");
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        sample_project(root.path(), &server, &source).await;
        // The version list's change: refused, not applied, with the plan.
        let e = install(root.path(), &source, &[("com.vrchat.avatars", "3.9.0")]).await.unwrap_err();
        assert!(e.to_string().contains("conflict"), "{e}");
        let plan = conflicted(e);
        assert_eq!(installed(&source, "com.vrchat.avatars").as_deref(), Some("3.7.5"), "nothing applied");
        assert!(plan.solved(), "{plan:#?}");
        let c = plan.conflicts.iter().find(|c| c.name == "com.vrchat.avatars").expect("the SDK's conflict");
        assert_eq!(c.version, "3.9.0");
        let needs: Vec<(&str, &str)> = c.needed_by.iter().map(|n| (n.name.as_str(), n.range.as_str())).collect();
        assert!(needs.contains(&("dev.vrlabs.av3manager", "3.2 - 3.7")), "{needs:?}");
        assert_eq!(needs.len(), 3, "{needs:?}");
        let steps: Vec<(&str, Option<&str>, Option<&str>)> = plan.steps.iter().map(|s| (s.name.as_str(), s.from.as_deref(), s.to.as_deref())).collect();
        assert_eq!(steps[0], ("com.vrchat.avatars", Some("3.7.5"), Some("3.9.0")), "what was asked for first");
        for step in [
            ("com.vrchat.base", Some("3.7.5"), Some("3.9.0")),
            ("vrchat.blackstartx.gesture-manager", Some("3.9.3"), Some("3.9.4")),
            ("dev.vrlabs.av3manager", Some("2.0.34"), Some("2.1.0")),
            ("dev.hai-vr.fuck-osc-config", Some("1.1.5"), Some("1.2.0")),
        ] {
            assert!(steps.contains(&step), "{step:?} in {steps:?}");
        }
        assert_eq!(steps.len(), 5);
        let why = |n: &str| plan.steps.iter().find(|s| s.name == n).unwrap().why.clone().unwrap_or_default();
        assert!(why("com.vrchat.base").contains("needed by"), "{}", why("com.vrchat.base"));
        assert!(why("dev.vrlabs.av3manager").contains("3.2 - 3.7"), "{}", why("dev.vrlabs.av3manager"));
        // One press: the whole plan.
        vpm::change(root.path(), &source, vpm::Change::Plan { install: plan.install.clone(), remove: plan.remove.clone(), prerelease: false }).await.unwrap();
        for (n, v) in [("com.vrchat.avatars", "3.9.0"), ("com.vrchat.base", "3.9.0"), ("vrchat.blackstartx.gesture-manager", "3.9.4"), ("dev.vrlabs.av3manager", "2.1.0"), ("dev.hai-vr.fuck-osc-config", "1.2.0")] {
            assert_eq!(installed(&source, n).as_deref(), Some(v), "{n}");
        }
    });
}

/// A package in development (a folder of the project's Packages no repository installed) that holds a change back is
/// named from its own package.json, with its version and range, and is never offered for removal nor removed.
#[test]
fn a_package_in_the_projects_own_folder_blocks_with_its_name_version_and_range_and_is_never_removed() {
    let server = sample_avatar(true);
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("N4N0 sample avatar");
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        sample_project(root.path(), &server, &source).await;
        let folder = source.join("Packages/My tool in progress");
        std::fs::create_dir_all(&folder).unwrap();
        orbiters_projects::write_json(
            &folder.join("package.json"),
            &json!({"name":"my.tool","displayName":"My Tool","version":"0.1.0","vpmDependencies":{"com.vrchat.avatars":">=3.7.0 <3.8.0"}}),
        )
        .unwrap();
        let plan = conflicted(install(root.path(), &source, &[("com.vrchat.avatars", "3.9.0")]).await.unwrap_err());
        assert!(!plan.solved(), "{plan:#?}");
        let need = plan.conflicts.iter().flat_map(|c| &c.needed_by).find(|n| n.name == "my.tool").expect("named among who needs another version");
        assert_eq!((need.display_name.as_str(), need.version.as_str(), need.range.as_str()), ("My Tool", "0.1.0", ">=3.7.0 <3.8.0"));
        assert_eq!(plan.blockers.len(), 1, "{plan:#?}");
        let b = &plan.blockers[0];
        assert_eq!((b.name.as_str(), b.display_name.as_str(), b.version.as_str(), b.range.as_str(), b.wanted.as_str()), ("my.tool", "My Tool", "0.1.0", ">=3.7.0 <3.8.0", "3.9.0"));
        assert_eq!(b.embedded.as_deref(), Some("My tool in progress"));
        assert!(!b.removable);
        // Asked anyway (the bridge can): refused, and the folder is still there.
        let plan = coherent::solve(root.path(), &source, &plan.requested, &["my.tool".into()], false).await.unwrap();
        assert!(!plan.solved() && plan.error.as_deref().is_some_and(|e| e.contains("My Tool")), "{plan:#?}");
        assert!(folder.join("package.json").is_file());
        // Its range widened, as the dialog says: the same change now goes through with the others.
        orbiters_projects::write_json(
            &folder.join("package.json"),
            &json!({"name":"my.tool","displayName":"My Tool","version":"0.1.0","vpmDependencies":{"com.vrchat.avatars":">=3.7.0 <3.10.0"}}),
        )
        .unwrap();
        let plan = conflicted(install(root.path(), &source, &[("com.vrchat.avatars", "3.9.0")]).await.unwrap_err());
        assert!(plan.solved() && plan.blockers.is_empty(), "{plan:#?}");
        assert!(plan.steps.iter().all(|s| s.name != "my.tool"), "{plan:#?}");
    });
}

#[test]
fn a_tool_without_a_coherent_version_blocks_until_removed_and_failures_roll_back() {
    let server = sample_avatar(false);
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("N4N0 sample avatar");
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        sample_project(root.path(), &server, &source).await;
        let plan = conflicted(install(root.path(), &source, &[("com.vrchat.avatars", "3.9.0")]).await.unwrap_err());
        assert!(!plan.solved());
        assert_eq!(plan.blockers.len(), 1, "{plan:#?}");
        let b = &plan.blockers[0];
        assert_eq!((b.name.as_str(), b.range.as_str(), b.wanted.as_str()), ("dev.hai-vr.fuck-osc-config", ">=3.1.0 <3.8.0-a", "3.9.0"));
        assert!(b.removable);
        // Removing it too, as the dialog offers.
        let plan = coherent::solve(root.path(), &source, &plan.requested, &["dev.hai-vr.fuck-osc-config".into()], false).await.unwrap();
        assert!(plan.solved(), "{plan:#?}");
        let removed = plan.steps.iter().find(|s| s.name == "dev.hai-vr.fuck-osc-config").unwrap();
        assert_eq!((removed.from.as_deref(), removed.to.as_deref()), (Some("1.1.5"), None));
        // A plan asking for a version gone from the repositories changes nothing at all: it is looked up first.
        let broken = vpm::Change::Plan { install: vec![("com.vrchat.avatars".into(), "9.9.9".into())], remove: plan.remove.clone(), prerelease: false };
        let e = vpm::change(root.path(), &source, broken).await.unwrap_err();
        assert!(format!("{e:#}").contains("no longer in the enabled repositories"), "{e:#}");
        assert_eq!(installed(&source, "dev.hai-vr.fuck-osc-config").as_deref(), Some("1.1.5"));
        assert!(source.join("Packages/dev.hai-vr.fuck-osc-config/package.json").is_file(), "the removed package restored");
        assert_eq!(installed(&source, "com.vrchat.avatars").as_deref(), Some("3.7.5"));
        vpm::change(root.path(), &source, vpm::Change::Plan { install: plan.install.clone(), remove: plan.remove.clone(), prerelease: false }).await.unwrap();
        assert_eq!(installed(&source, "com.vrchat.avatars").as_deref(), Some("3.9.0"));
        assert_eq!(installed(&source, "com.vrchat.base").as_deref(), Some("3.9.0"));
        assert_eq!(installed(&source, "dev.hai-vr.fuck-osc-config"), None);
        assert!(!source.join("Packages/dev.hai-vr.fuck-osc-config").exists());
    });
}

/// PROJECTS-35: a plan that fails after its removals (the repository gone mid-way) puts back the very folders it took
/// away, a local edit included, not a fresh download of the same version.
#[test]
fn a_failed_plan_puts_back_the_removed_package_with_its_local_edits() {
    let server = sample_avatar(false);
    let root = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let source = projects.path().join("N4N0 sample avatar");
    let rt = orbiters_projects::runtime().unwrap();
    rt.block_on(async {
        sample_project(root.path(), &server, &source).await;
        let plan = conflicted(install(root.path(), &source, &[("com.vrchat.avatars", "3.9.0")]).await.unwrap_err());
        let plan = coherent::solve(root.path(), &source, &plan.requested, &["dev.hai-vr.fuck-osc-config".into()], false).await.unwrap();
        assert!(plan.solved(), "{plan:#?}");
        let edit = source.join("Packages/dev.hai-vr.fuck-osc-config/Mine.cs");
        std::fs::write(&edit, "// my own change").unwrap();
        let avatars_edit = source.join("Packages/com.vrchat.avatars/Mine.txt");
        std::fs::write(&avatars_edit, "kept").unwrap();
        // The repository goes away: the removal works, the 3.9.0 archives cannot be downloaded.
        drop(server);
        let e = vpm::change(root.path(), &source, vpm::Change::Plan { install: plan.install.clone(), remove: plan.remove.clone(), prerelease: false })
            .await
            .unwrap_err();
        assert!(format!("{e:#}").contains("as they were"), "{e:#}");
        assert_eq!(std::fs::read_to_string(&edit).unwrap(), "// my own change");
        assert_eq!(std::fs::read_to_string(&avatars_edit).unwrap(), "kept");
        assert_eq!(installed(&source, "dev.hai-vr.fuck-osc-config").as_deref(), Some("1.1.5"));
        assert_eq!(installed(&source, "com.vrchat.avatars").as_deref(), Some("3.7.5"));
        let left: Vec<_> = std::fs::read_dir(&source).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".orbiters-plan-")).collect();
        assert!(left.is_empty(), "the kept copies are cleaned up");
    });
}
