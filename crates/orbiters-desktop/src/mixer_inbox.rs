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

/// Drops an import request for a session's splats into `endpoint`'s inbox (written whole, then renamed into view).
pub fn send_splats(endpoint: &Endpoint, session: &Path, title: &str) -> anyhow::Result<()> {
    let request = Request { kind: REQUEST_KIND, action: IMPORT_SPLATS, session, title };
    let name = uuid::Uuid::new_v4().to_string();
    let part = endpoint.dir.join(format!("{name}.json.part"));
    std::fs::write(&part, serde_json::to_vec_pretty(&request)?)?;
    std::fs::rename(&part, endpoint.dir.join(format!("{name}.json")))?;
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
}
