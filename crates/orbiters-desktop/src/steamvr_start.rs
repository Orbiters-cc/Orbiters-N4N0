//! Passive startup observation: never loads vrclient or opens a runtime IPC connection.
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
static READY: Mutex<Option<((u32, u64), (u32, u64))>> = Mutex::new(None);

pub fn ready_log(text: &str, compositor: u32) -> bool {
    let Some(start) = text.rfind(&format!("startup with PID={compositor},")) else { return false };
    let tail = &text[start..];
    !tail.contains("shutting down") && tail.contains("Startup Complete (")
}

/// Identity includes creation time, so PID reuse and old compositor log sections cannot admit a new server.
pub fn ready_session() -> Option<(u32, u64)> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
    let server = system.processes().values().find(|p| p.name().eq_ignore_ascii_case("vrserver.exe"))?;
    let compositor = system.processes().values().find(|p| p.name().eq_ignore_ascii_case("vrcompositor.exe") && p.start_time() >= server.start_time())?;
    let log = orbiters_openvr::log_dir()?.join("vrcompositor.txt");
    let identity = ((server.pid().as_u32(), server.start_time()), (compositor.pid().as_u32(), compositor.start_time()));
    if *READY.lock().ok()? == Some(identity) {
        return Some(identity.0);
    }
    let mut file = std::fs::File::open(&log).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(256 * 1024))).ok()?;
    let mut bytes = Vec::new();
    file.take(256 * 1024).read_to_end(&mut bytes).ok()?;
    let mut ready = ready_log(&String::from_utf8_lossy(&bytes), compositor.pid().as_u32());
    if !ready && len > 256 * 1024 {
        // Starting the app hours into a session must still find the current startup
        // section. Scan once, with bounded working memory, then cache exact identities.
        use std::io::BufRead;
        let file = std::fs::File::open(&log).ok()?;
        let needle = format!("startup with PID={},", compositor.pid().as_u32());
        let mut current = false;
        for line in std::io::BufReader::new(file).lines() {
            let line = line.ok()?;
            if line.contains("startup with PID=") {
                current = line.contains(&needle);
                ready = false;
            }
            if current && line.contains("Startup Complete (") {
                ready = true;
            }
            if current && line.contains("shutting down") {
                ready = false;
            }
        }
    }
    if ready {
        *READY.lock().ok()? = Some(identity);
        Some(identity.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_admission_requires_current_compositor_completion() {
        let old = "startup with PID=1,\nStartup Complete (0.3 seconds)\n";
        assert!(!ready_log(old, 2));
        let starting = format!("{old}startup with PID=2,\nFrame manager initialized\n");
        assert!(!ready_log(&starting, 2));
        assert!(ready_log(&(starting.clone() + "Startup Complete (0.4 seconds)"), 2));
        assert!(!ready_log(&(starting + "Startup Complete (0.4 seconds)\nshutting down"), 2));
    }
}
