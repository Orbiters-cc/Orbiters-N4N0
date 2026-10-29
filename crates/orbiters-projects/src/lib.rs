//! Unity discovery and VPM operations. Call on workers, never from an egui frame.
pub mod avatars;
pub mod discovery;
pub mod network;
pub mod setup;
pub mod templates;
pub mod vpm;

use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Orbiters")
}

pub fn vcc_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("VRChatCreatorCompanion")
}

pub fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?)
}

/// Atomic publication in the destination directory; no partially written JSON is visible.
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or_else(|| anyhow::anyhow!("Missing parent folder"))?)?;
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(value)?)?;
    f.sync_all()?;
    drop(f);
    // Windows rename does not replace existing files. MoveFileEx provides replacement without a gap.
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
        let src: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)?;
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c
}

pub fn unity_open(path: &Path) -> bool {
    let lock = path.join("Temp/UnityLockfile");
    lock.exists() && std::fs::OpenOptions::new().write(true).open(lock).is_err()
}

pub fn open_project(project: &discovery::Project) -> anyhow::Result<()> {
    let editor = discovery::find_editor(&project.unity)
        .ok_or_else(|| anyhow::anyhow!("Unity {} is not installed. Open Unity Hub to install this version.", project.unity))?;
    hidden_command(editor).arg("-projectPath").arg(&project.path).spawn()?;
    Ok(())
}

pub fn open_hub() -> anyhow::Result<()> {
    let settings: serde_json::Value = std::fs::read(vcc_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let path = settings["pathToUnityHub"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Unity Hub\Unity Hub.exe"));
    anyhow::ensure!(path.is_file(), "Unity Hub is not installed. Download it from unity.com/download.");
    hidden_command(path).spawn()?;
    Ok(())
}
