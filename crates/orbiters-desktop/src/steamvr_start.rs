//! Passive admission is only a load-reduction hint. Background Init is the authoritative,
//! non-launching usability check. No log file is required or read.
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub type Session = (u32, u64);
/// Every live server is tried after this age, without requiring any optional hint.
const MAX_STARTUP_WAIT_SECS: u64 = 2;
static WAITING: Mutex<Option<(Session, bool)>> = Mutex::new(None);

fn admitted(server: Session, now: u64) -> bool {
    // Presence is not health. Background Init is the actual usability handshake.
    // No optional log/compositor hint can prevent or reset this short bound.
    now.saturating_sub(server.1) >= MAX_STARTUP_WAIT_SECS
}

/// PID plus creation time avoids treating a reused PID as the previous runtime.
pub fn current_session() -> Option<Session> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
    system.processes().values().find(|p| p.name().eq_ignore_ascii_case("vrserver.exe")).map(|p| (p.pid().as_u32(), p.start_time()))
}

pub fn ready_session() -> Option<Session> {
    let Some(server) = current_session() else {
        if let Ok(mut waiting) = WAITING.lock() {
            *waiting = None;
        }
        return None;
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let ready = admitted(server, now);
    if let Ok(mut waiting) = WAITING.lock() {
        if *waiting != Some((server, ready)) {
            eprintln!(
                "SteamVR {}: {}",
                server.0,
                if ready { "trying non-launching Background connection" } else { "waiting for two-second server age bound; no readiness hint required" }
            );
            *waiting = Some((server, ready));
        }
    }
    ready.then_some(server)
}

/// Whether the SteamVR that runs is the developer VR sandbox's (`tools/vr-sandbox`, an emulated headset that a VR session of
/// the VRChat bridge drives, docs/vr-sandbox.md): its driver is the one external driver in `openvrpaths.vrpath`. The app
/// then leaves it alone, as if no SteamVR ran (no overlay, no sound devices, no SteamVR start choice): it is not the
/// person's VR, and the person goes on working beside it (nofocus, 2026-11-15).
pub fn sandbox_runs() -> bool {
    let Some(base) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) else { return false };
    let Ok(text) = std::fs::read_to_string(base.join("openvr").join("openvrpaths.vrpath")) else { return false };
    sandbox_in_vrpath(&text, |dir| std::fs::read_to_string(dir.join("driver.vrdrivermanifest")).ok())
}

/// A debug build the sandbox itself started for a test (its `app` action sets `ORBITERS_VR_SANDBOX_ROOT`): the sandbox's
/// SteamVR is the one it is there to test, so it is taken as SteamVR. Since the tour's VR sessions (0e8b23ff) every app left
/// it alone, test builds included, and no overlay ever started in the sandbox (N-friend-lag, 2026-11-16).
pub fn sandbox_test_app() -> bool {
    cfg!(debug_assertions) && std::env::var_os("ORBITERS_VR_SANDBOX_ROOT").is_some()
}

/// [`sandbox_runs`] on the file's text: an external driver whose manifest names `orbiters_sandbox`.
pub fn sandbox_in_vrpath(text: &str, manifest: impl Fn(&std::path::Path) -> Option<String>) -> bool {
    let Ok(paths) = serde_json::from_str::<serde_json::Value>(text) else { return false };
    paths["external_drivers"].as_array().is_some_and(|drivers| {
        drivers.iter().filter_map(|d| d.as_str()).any(|dir| {
            manifest(std::path::Path::new(dir))
                .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
                .is_some_and(|m| m["name"] == "orbiters_sandbox")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_sandboxs_steamvr_is_told_by_its_driver_and_the_persons_is_not() {
        // As the sandbox writes it (2026-11-15, during a session).
        let sandbox = r#"{"config":["C:\\Program Files (x86)\\Steam\\config"],"external_drivers":["C:\\Users\\Enzo\\.orbiters-build\\vr-sandbox\\driver"],"runtime":["C:\\Steam\\steamapps\\common\\SteamVR"],"version":1}"#;
        let manifest = |dir: &std::path::Path| dir.ends_with("driver").then(|| r#"{"alwaysActivate": false, "name": "orbiters_sandbox"}"#.to_string());
        assert!(sandbox_in_vrpath(sandbox, manifest));
        let persons = r#"{"external_drivers":["C:\\Steam\\steamapps\\common\\Virtual Desktop Streamer\\OpenVR"],"runtime":["C:\\Steam"]}"#;
        let vd = |_: &std::path::Path| Some(r#"{"name": "virtualdesktop"}"#.to_string());
        assert!(!sandbox_in_vrpath(persons, vd), "the person's own drivers");
        assert!(!sandbox_in_vrpath(r#"{"runtime":["C:\\Steam"]}"#, manifest), "no external driver");
        assert!(!sandbox_in_vrpath("not json", manifest));
    }
    #[test]
    fn no_readiness_hint_safe_mode_missing_unreadable_rotated_logs_are_bounded() {
        // No file I/O: all absent, unreadable, rotated or differently worded logs
        // have exactly this same admission path, including no compositor at all.
        let server = (7, 100);
        assert!(!admitted(server, 101));
        assert!(admitted(server, 102));
        assert!(admitted(server, 1000));
    }
    #[test]
    fn app_started_after_steamvr_is_up_is_admitted_immediately() {
        // Admission is based on server age, not time since this app started.
        assert!(admitted((7, 100), 500));
    }
    #[test]
    fn runtime_restart_has_a_new_wait_bound_even_with_pid_reuse() {
        assert!(admitted((7, 100), 200));
        assert!(!admitted((7, 200), 201));
        assert!(admitted((7, 200), 202));
    }
}
