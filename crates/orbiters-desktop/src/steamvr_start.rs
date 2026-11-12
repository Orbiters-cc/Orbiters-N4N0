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

#[cfg(test)]
mod tests {
    use super::*;
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
