use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
/// Independent watcher survives host exit; retained directories are durable retry records.
pub struct Profile {
    pub path: PathBuf,
    lock: Option<std::fs::File>,
}
impl Profile {
    pub fn new(closed: &std::sync::atomic::AtomicBool) -> anyhow::Result<Self> {
        let root = std::env::temp_dir().join("Orbiters-QR-Web-owned-v1");
        std::fs::create_dir_all(&root)?;
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(std::fs::symlink_metadata(&root)?.file_attributes() & 0x400 == 0, "profile root is redirected");
        let path = tempfile::Builder::new().prefix("session-").tempdir_in(&root)?.keep();
        use std::os::windows::fs::OpenOptionsExt;
        let mut guard = Self { path: path.clone(), lock: None };
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).share_mode(1).open(path.join("owner.lock"))?;
        guard.lock = Some(lock);
        let script = include_str!("watcher.ps1")
            .replace("__ROOT__", &root.to_string_lossy().replace('\'', "''"))
            .replace("__PATH__", &path.to_string_lossy().replace('\'', "''"))
            .replace("__PID__", &std::process::id().to_string());
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand", &encoded])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let start = Instant::now();
        while !path.join("ready").exists() {
            anyhow::ensure!(!closed.load(std::sync::atomic::Ordering::Relaxed), "browser startup cancelled");
            anyhow::ensure!(child.try_wait()?.is_none() && start.elapsed() < Duration::from_secs(15), "profile watcher failed to start");
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(guard)
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        self.lock.take();
        if !self.path.join("ready").exists() {
            // No browser is started until watcher readiness. Remove a failed bootstrap record.
            let _ = std::fs::remove_dir_all(&self.path);
            return;
        }
        let _ = std::fs::write(self.path.join("done"), b"");
        let start = Instant::now();
        while self.path.exists() && start.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
