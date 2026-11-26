//! "Send to Mixer": every Unity editor running Mixer keeps a folder `%LOCALAPPDATA%/Orbiters/Mixer/inbox/<pid>/`
//! with an `endpoint.json` naming its project (Mixer's `SplatInbox`), and imports the requests dropped next to it.

use orbiters_core::locks::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const ENDPOINT_KIND: &str = "orbiters.mixer.inbox";
const REQUEST_KIND: &str = "orbiters.mixer.request";
const IMPORT_SPLATS: &str = "import-splats";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub kind: String,
    pub pid: u32,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub updated_utc: String,
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Serialize)]
struct Request<'a> {
    kind: &'a str,
    action: &'a str,
    session: &'a Path,
    title: &'a str,
}

/// Editors whose process still runs, the most recently (re)loaded first.
pub fn endpoints(root: &Path, alive: impl Fn(u32) -> bool) -> Vec<Endpoint> {
    let mut found: Vec<Endpoint> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let mut ep: Endpoint = serde_json::from_str(&std::fs::read_to_string(dir.join("endpoint.json")).ok()?).ok()?;
            ep.dir = dir;
            (ep.kind == ENDPOINT_KIND && alive(ep.pid)).then_some(ep)
        })
        .collect();
    found.sort_by(|a, b| b.updated_utc.cmp(&a.updated_utc));
    found
}

/// Where the copies of edited sessions handed to Mixer go ([`session_for_mixer`]): one per session folder name.
pub fn exports_dir() -> PathBuf {
    orbiters_core::paths::mixer_inbox_dir().with_file_name("exports")
}

/// Room kept free on the disk an export goes to, beyond the export itself.
const EXPORT_MARGIN: u64 = 512 << 20;

/// The session folder Mixer imports for `session`: the session itself when its splat edits (orbiters_reconstruct::edit)
/// leave nothing out; else a copy of what its 3D view reads (the manifest, the frame records, and `reconstruction/` with
/// the sky map, the report and a PLY of only the splats kept, in the same layout, as "Keep only the 3D view" leaves a
/// session) in `exports/<session folder name>/`, so that Mixer, which reads the PLY itself, imports what the 3D view
/// shows. Edits that cannot be applied stop the handoff (as they stop shares). The copy is checked against the room left
/// first, written beside, then put in place of the previous copy whole. Pictures go only when drawn for the edits (never
/// the recording's own thumbnail of what they removed).
///
/// Each copy is named by what it is: the session's folder name, its edits' generation and a hash of where the session is
/// (`<name>-<edits>-<where>`), and never changed once made: a request Mixer has not imported yet still finds the version
/// it was sent, a second send of the same edits reuses it, and a newer edit makes a new folder (SWEEP-13). It is built in
/// a folder of its own (`.partial-<uuid>`), removed only by that send. Older copies of the same session go once no request
/// in `requests` (Mixer's inboxes: it removes a request once imported) names them.
pub fn session_for_mixer(session: &Path, exports: &Path) -> anyhow::Result<PathBuf> {
    session_for_mixer_with(session, exports, &orbiters_core::paths::mixer_inbox_dir())
}

/// [`session_for_mixer`], with the inboxes whose requests keep older copies in `requests`.
pub fn session_for_mixer_with(session: &Path, exports: &Path, requests: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    use orbiters_core::reconstruction as rec;
    // One export at a time: two sends at once (two presses, the window and the VR menu) make one copy, not two halves.
    static EXPORTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = EXPORTING.locked();
    let result = rec::result_dir(session);
    if orbiters_reconstruct::edit::generation(&result)? == orbiters_reconstruct::edit::UNEDITED {
        return Ok(session.to_path_buf());
    }
    let (kept, removed, generation) = orbiters_reconstruct::edit::read_edited(&result.join(rec::PLY_FILE))?;
    let name = session.file_name().with_context(|| orbiters_i18n::tr("a session folder without a name"))?.to_string_lossy().into_owned();
    let place = session.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    let suffix = format!("-{:08x}", place as u32);
    let tag = format!("{name}-{generation}{suffix}");
    let dest = exports.join(&tag);
    if copy_made_for(&dest, session, &generation) {
        prune_old_copies(exports, &name, &suffix, &tag, requests);
        return Ok(dest);
    }
    let partial = exports.join(format!("{tag}.partial-{}", uuid::Uuid::new_v4().simple()));
    // Whatever stops this send removes its own partial folder, and only it.
    struct Partial(PathBuf);
    impl Drop for Partial {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let partial_guard = Partial(partial.clone());
    let sky = result.join(rec::SKY_FILE);
    let picture = orbiters_reconstruct::edit::picture_state(&result).preview_ok().then(|| result.join(rec::PREVIEW_FILE)).filter(|p| p.is_file());
    let small = [session.join(orbiters_core::contracts::manifest::MANIFEST_FILE), session.join(orbiters_core::contracts::manifest::FRAMES_FILE), sky.clone()];
    let needed = 1024 + kept.len() as u64 * 92 + small.iter().chain(picture.iter()).filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum::<u64>();
    std::fs::create_dir_all(exports).with_context(|| orbiters_i18n::tr!("making {path}", path = exports.display()))?;
    if let Some(free) = orbiters_projects::size::free_space(exports) {
        anyhow::ensure!(
            free >= needed + EXPORT_MARGIN,
            "{}",
            orbiters_i18n::tr!(
                "Mixer gets a copy of this capture without the splats its 3D view removed: {needed} MB, and {folder} has {free} MB left (512 MB kept free).",
                needed = needed >> 20,
                folder = exports.display(),
                free = free >> 20
            )
        );
    }
    let out = partial.join(rec::DIR);
    std::fs::create_dir_all(&out)?;
    for (from, to) in [
        (session.join(orbiters_core::contracts::manifest::MANIFEST_FILE), partial.join(orbiters_core::contracts::manifest::MANIFEST_FILE)),
        (session.join(orbiters_core::contracts::manifest::FRAMES_FILE), partial.join(orbiters_core::contracts::manifest::FRAMES_FILE)),
    ] {
        std::fs::copy(&from, &to).with_context(|| orbiters_i18n::tr!("copying {path}", path = from.display()))?;
    }
    if sky.is_file() {
        std::fs::copy(&sky, out.join(rec::SKY_FILE))?;
    }
    if let Some(p) = &picture {
        std::fs::copy(p, out.join(rec::PREVIEW_FILE))?;
    }
    orbiters_reconstruct::ply::write(&out.join(rec::PLY_FILE), &kept)?;
    let mut report = rec::read_report(session).unwrap_or_default();
    report.gaussians = kept.len();
    std::fs::write(out.join(rec::REPORT_FILE), serde_json::to_vec_pretty(&report)?)?;
    // What it was made from, for whoever wonders (Mixer passes it by).
    let made = serde_json::json!({ "from": session, "splatEdits": generation, "removed": removed, "kept": kept.len() });
    std::fs::write(partial.join("export.json"), serde_json::to_vec_pretty(&made)?)?;
    // A folder of that name is a copy of exactly these edits (an earlier one not complete is replaced: no request
    // can name a copy that never was put in place).
    if dest.exists() {
        std::fs::remove_dir_all(&dest).with_context(|| orbiters_i18n::tr!("replacing {path}", path = dest.display()))?;
    }
    std::fs::rename(&partial, &dest).with_context(|| orbiters_i18n::tr!("putting {path} in place", path = dest.display()))?;
    drop(partial_guard);
    prune_old_copies(exports, &name, &suffix, &tag, requests);
    Ok(dest)
}

/// Whether `dest` is a complete copy of `session` with the edits `generation` (its `export.json` says so, its PLY is there).
fn copy_made_for(dest: &Path, session: &Path, generation: &str) -> bool {
    use orbiters_core::reconstruction as rec;
    let Ok(text) = std::fs::read(dest.join("export.json")) else { return false };
    let Ok(made) = serde_json::from_slice::<serde_json::Value>(&text) else { return false };
    made["splatEdits"] == generation && made["from"].as_str().map(Path::new) == Some(session) && rec::dir(dest).join(rec::PLY_FILE).is_file()
}

/// The sessions Mixer's inboxes under `requests` still have a request for (not imported yet).
fn requested(requests: &Path) -> Vec<PathBuf> {
    let mut named = Vec::new();
    for inbox in std::fs::read_dir(requests).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(inbox.path()).into_iter().flatten().flatten() {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "json") && path.file_name().is_some_and(|n| n != "endpoint.json") {
                let session = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).and_then(|v| v["session"].as_str().map(PathBuf::from));
                named.extend(session);
            }
        }
    }
    named
}

/// Removes the older copies of one session (`<name>-…<suffix>`, not `keep`) that no request still names.
fn prune_old_copies(exports: &Path, name: &str, suffix: &str, keep: &str, requests: &Path) {
    let named = requested(requests);
    for entry in std::fs::read_dir(exports).into_iter().flatten().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let older = file != keep && file.starts_with(&format!("{name}-")) && file.ends_with(suffix) && !file.contains(".partial-");
        if older && entry.path().is_dir() && !named.iter().any(|n| n == &entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Drops an import request for a session's splats into `endpoint`'s inbox (written whole, flushed to the disk, then
/// renamed into view: Mixer never reads half a request). `session` is what [`session_for_mixer`] gives.
pub fn send_splats(endpoint: &Endpoint, session: &Path, title: &str) -> anyhow::Result<()> {
    let request = Request { kind: REQUEST_KIND, action: IMPORT_SPLATS, session, title };
    let name = uuid::Uuid::new_v4().to_string();
    orbiters_core::write_atomic(&endpoint.dir.join(format!("{name}.json")), &serde_json::to_vec_pretty(&request)?)?;
    Ok(())
}

/// Whether a process runs.
pub fn process_alive(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing().without_tasks());
    system.process(pid).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_land_in_live_editors_only() {
        let root = std::env::temp_dir().join(format!("orbiters-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (pid, updated) in [(11u32, "2026-10-20T10:00:00Z"), (22, "2026-10-20T12:00:00Z"), (33, "2026-10-20T13:00:00Z")] {
            let dir = root.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let json = format!(r#"{{"kind":"orbiters.mixer.inbox","pid":{pid},"projectPath":"H:/p{pid}","projectName":"P{pid}","updatedUtc":"{updated}"}}"#);
            std::fs::write(dir.join("endpoint.json"), json).unwrap();
        }
        // 33 is gone.
        let live = endpoints(&root, |pid| pid != 33);
        assert_eq!(live.iter().map(|e| e.pid).collect::<Vec<_>>(), vec![22, 11]);
        assert_eq!(live[0].project_name, "P22");
        send_splats(&live[0], Path::new("C:/Sessions/Canine"), "Canine").unwrap();
        let files: Vec<_> = std::fs::read_dir(root.join("22")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        let request = files.iter().find(|f| f.ends_with(".json") && *f != "endpoint.json").expect("a request");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("22").join(request)).unwrap()).unwrap();
        assert_eq!(v["kind"], "orbiters.mixer.request");
        assert_eq!(v["action"], "import-splats");
        assert_eq!(v["session"], "C:/Sessions/Canine");
        assert!(!files.iter().any(|f| f.ends_with(".part")));
        std::fs::remove_dir_all(&root).ok();
    }

    /// SWEEP-13: a request sent before a second edit still names a folder with the first edit's PLY (while Mixer has not
    /// imported it, the folder stays); a second send of the same edits reuses the copy; two sends at once both get a
    /// complete folder; an old copy no request names goes.
    #[test]
    fn a_request_keeps_the_copy_it_was_sent() {
        use orbiters_core::contracts::manifest::{FRAMES_FILE, MANIFEST_FILE};
        use orbiters_core::reconstruction as rec;
        use orbiters_reconstruct::edit::{DELETED, Edit, Identity};
        let root = std::env::temp_dir().join(format!("orbiters-mixer-copies-{}", uuid::Uuid::new_v4()));
        let (session, exports, inboxes) = (root.join("Home 2026-11-19"), root.join("exports"), root.join("inbox"));
        let result = rec::dir(&session);
        std::fs::create_dir_all(&result).unwrap();
        std::fs::create_dir_all(inboxes.join("42")).unwrap();
        std::fs::write(session.join(MANIFEST_FILE), b"{}").unwrap();
        std::fs::write(session.join(FRAMES_FILE), [7u8; 64]).unwrap();
        let mut g = orbiters_reconstruct::gaussians::Gaussians::default();
        for i in 0..6 {
            g.push([i as f32, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        }
        orbiters_reconstruct::ply::write(&result.join(rec::PLY_FILE), &g).unwrap();
        let edit_out = |n: usize| {
            let mut edit = Edit::new(6, Identity::read(&result.join(rec::PLY_FILE)).unwrap());
            edit.marks[n] = DELETED;
            edit.save(&result).unwrap();
        };
        let kept = |dir: &Path| orbiters_reconstruct::ply::read(&rec::dir(dir).join(rec::PLY_FILE)).unwrap().means.iter().map(|m| m[0] as usize).collect::<Vec<_>>();
        edit_out(1);
        let first = session_for_mixer_with(&session, &exports, &inboxes).unwrap();
        // Mixer has not imported it yet: its request is in the inbox.
        std::fs::write(inboxes.join("42").join("a.json"), serde_json::to_vec(&serde_json::json!({ "session": first })).unwrap()).unwrap();
        assert_eq!(session_for_mixer_with(&session, &exports, &inboxes).unwrap(), first, "the same edits: the same copy");
        edit_out(4);
        let second = session_for_mixer_with(&session, &exports, &inboxes).unwrap();
        assert_ne!(second, first);
        assert_eq!(kept(&first), [0, 2, 3, 4, 5], "the first request still imports the first edit's splats");
        assert_eq!(kept(&second), [0, 1, 2, 3, 5]);
        // Imported (its request gone): the old copy goes at the next send.
        std::fs::remove_file(inboxes.join("42").join("a.json")).unwrap();
        let both: Vec<_> = (0..2)
            .map(|_| {
                let (s, e, i) = (session.clone(), exports.clone(), inboxes.clone());
                std::thread::spawn(move || session_for_mixer_with(&s, &e, &i).unwrap())
            })
            .collect();
        for sent in both.into_iter().map(|t| t.join().unwrap()) {
            assert_eq!(sent, second);
            assert_eq!(kept(&sent), [0, 1, 2, 3, 5], "complete");
        }
        assert!(!first.exists(), "no request names it any more");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Mixer gets what the 3D view shows: a session without edits as it is; an edited one as a copy of its 3D view's
    /// files whose PLY holds only the splats kept (the same layout, read back whole), sent again over its previous copy;
    /// damaged edits stop the handoff; a picture not drawn for the edits stays behind.
    #[test]
    fn mixer_gets_the_splats_the_view_keeps() {
        use orbiters_core::contracts::manifest::{FRAMES_FILE, MANIFEST_FILE};
        use orbiters_core::reconstruction as rec;
        use orbiters_reconstruct::edit::{DELETED, Edit, HIDDEN, Identity};
        let root = std::env::temp_dir().join(format!("orbiters-mixer-export-{}", uuid::Uuid::new_v4()));
        let (session, exports) = (root.join("Home 2026-11-02"), root.join("exports"));
        let result = rec::dir(&session);
        std::fs::create_dir_all(&result).unwrap();
        std::fs::write(session.join(MANIFEST_FILE), b"{}").unwrap();
        std::fs::write(session.join(FRAMES_FILE), [7u8; 256]).unwrap();
        std::fs::write(result.join(rec::SKY_FILE), b"sky").unwrap();
        std::fs::write(result.join(rec::PREVIEW_FILE), b"trained picture").unwrap();
        let mut g = orbiters_reconstruct::gaussians::Gaussians::default();
        for i in 0..6 {
            g.push([i as f32, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        }
        g.sky[5] = true;
        orbiters_reconstruct::ply::write(&result.join(rec::PLY_FILE), &g).unwrap();
        assert_eq!(session_for_mixer(&session, &exports).unwrap(), session, "no edits: the session itself");
        let mut edit = Edit::new(6, Identity::read(&result.join(rec::PLY_FILE)).unwrap());
        edit.marks[1] = HIDDEN;
        edit.marks[3] = DELETED;
        edit.picture = false;
        edit.save(&result).unwrap();
        for _ in 0..2 {
            let sent = session_for_mixer(&session, &exports).unwrap();
            assert!(sent.file_name().unwrap().to_string_lossy().starts_with("Home 2026-11-02-"), "{sent:?}");
            let kept = orbiters_reconstruct::ply::read(&rec::dir(&sent).join(rec::PLY_FILE)).unwrap();
            assert_eq!(kept.means.iter().map(|m| m[0] as usize).collect::<Vec<_>>(), [0, 2, 4, 5], "the kept splats in order");
            assert_eq!(kept.sky, [false, false, false, true], "with the PLY's own properties");
            assert_eq!(std::fs::read(sent.join(FRAMES_FILE)).unwrap(), [7u8; 256]);
            assert!(rec::dir(&sent).join(rec::SKY_FILE).is_file() && !rec::dir(&sent).join(rec::EDIT_FILE).exists());
            assert!(!rec::dir(&sent).join(rec::PREVIEW_FILE).exists(), "a picture not drawn for the edits stays behind");
            assert_eq!(rec::read_report(&sent).map(|r| r.gaussians), Some(4));
            assert!(!std::fs::read_dir(&exports).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".partial")));
        }
        std::fs::write(result.join(rec::EDIT_FILE), b"damaged").unwrap();
        assert!(session_for_mixer(&session, &exports).is_err(), "damaged edits: no handoff");
        std::fs::remove_dir_all(&root).ok();
    }
}
