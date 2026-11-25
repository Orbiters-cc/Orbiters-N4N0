//! SteamVR's client on Linux. Nothing is loaded yet (the port's SteamVR stream, L4: `<runtime>/bin/linux64/vrclient.so`); the
//! layouts of its structures are in place.
use std::path::Path;

pub struct Library;

impl Library {
    pub fn open(_runtime: &Path) -> Result<Self, String> {
        Err("SteamVR is not talked with on Linux yet".into()) // i18n: not shown (the overlay logs the load error)
    }

    pub fn symbol(&self, _name: &std::ffi::CStr) -> Option<*const std::ffi::c_void> {
        None
    }
}

/// Whether SteamVR's server runs: a process (not a thread: those are under `<pid>/task`) of that name in `/proc`. A list that
/// cannot be read says nothing: `true`, so the caller connects as it always did.
pub fn server_running() -> bool {
    let Ok(processes) = std::fs::read_dir("/proc") else { return true };
    processes.flatten().filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())).any(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|name| name.trim_end() == "vrserver"))
}
