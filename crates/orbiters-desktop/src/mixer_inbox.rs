//! "Send to Mixer": every Unity editor running Mixer keeps a folder `%LOCALAPPDATA%/Orbiters/Mixer/inbox/<pid>/`
//! with an `endpoint.json` naming its project (Mixer's `SplatInbox`), and imports the requests dropped next to it.

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
pub fn session_for_mixer(session: &Path, exports: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    use orbiters_core::reconstruction as rec;
    let result = rec::result_dir(session);
    if orbiters_reconstruct::edit::generation(&result)? == orbiters_reconstruct::edit::UNEDITED {
        return Ok(session.to_path_buf());
    }
    let (kept, removed, generation) = orbiters_reconstruct::edit::read_edited(&result.join(rec::PLY_FILE))?;
    let name = session.file_name().context("a session folder without a name")?;
    let dest = exports.join(name);
    let partial = exports.join(format!("{}.partial", name.to_string_lossy()));
    let sky = result.join(rec::SKY_FILE);
    let picture = orbiters_reconstruct::edit::picture_state(&result).preview_ok().then(|| result.join(rec::PREVIEW_FILE)).filter(|p| p.is_file());
    let small = [session.join(orbiters_core::contracts::manifest::MANIFEST_FILE), session.join(orbiters_core::contracts::manifest::FRAMES_FILE), sky.clone()];
    let needed = 1024 + kept.len() as u64 * 92 + small.iter().chain(picture.iter()).filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum::<u64>();
    std::fs::create_dir_all(exports).with_context(|| format!("making {}", exports.display()))?;
    if let Some(free) = orbiters_projects::size::free_space(exports) {
        anyhow::ensure!(
            free >= needed + EXPORT_MARGIN,
            "Mixer gets a copy of this capture without the splats its 3D view removed: {} MB, and {} has {} MB left (512 MB kept free).",
            needed >> 20,
            exports.display(),
            free >> 20
        );
    }
    let _ = std::fs::remove_dir_all(&partial);
    let out = partial.join(rec::DIR);
    std::fs::create_dir_all(&out)?;
    for (from, to) in [
        (session.join(orbiters_core::contracts::manifest::MANIFEST_FILE), partial.join(orbiters_core::contracts::manifest::MANIFEST_FILE)),
        (session.join(orbiters_core::contracts::manifest::FRAMES_FILE), partial.join(orbiters_core::contracts::manifest::FRAMES_FILE)),
    ] {
        std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
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
    if dest.exists() {
        std::fs::remove_dir_all(&dest).with_context(|| format!("replacing {}", dest.display()))?;
    }
    std::fs::rename(&partial, &dest).with_context(|| format!("putting {} in place", dest.display()))?;
    Ok(dest)
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
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing());
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
            assert_eq!(sent, exports.join("Home 2026-11-02"));
            let kept = orbiters_reconstruct::ply::read(&rec::dir(&sent).join(rec::PLY_FILE)).unwrap();
            assert_eq!(kept.means.iter().map(|m| m[0] as usize).collect::<Vec<_>>(), [0, 2, 4, 5], "the kept splats in order");
            assert_eq!(kept.sky, [false, false, false, true], "with the PLY's own properties");
            assert_eq!(std::fs::read(sent.join(FRAMES_FILE)).unwrap(), [7u8; 256]);
            assert!(rec::dir(&sent).join(rec::SKY_FILE).is_file() && !rec::dir(&sent).join(rec::EDIT_FILE).exists());
            assert!(!rec::dir(&sent).join(rec::PREVIEW_FILE).exists(), "a picture not drawn for the edits stays behind");
            assert_eq!(rec::read_report(&sent).map(|r| r.gaussians), Some(4));
            assert!(!exports.join("Home 2026-11-02.partial").exists());
        }
        std::fs::write(result.join(rec::EDIT_FILE), b"damaged").unwrap();
        assert!(session_for_mixer(&session, &exports).is_err(), "damaged edits: no handoff");
        std::fs::remove_dir_all(&root).ok();
    }
}
