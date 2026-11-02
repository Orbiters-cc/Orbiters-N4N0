//! Unity discovery and VPM operations. Call on workers, never from an egui frame.
pub mod avatars;
pub mod blender;
mod builtin_templates;
pub mod coherent;
pub mod discovery;
pub mod editor;
pub mod git;
pub mod hidden;
pub mod icons;
pub mod missing;
pub mod network;
pub mod preview;
pub mod setup;
pub mod size;
pub mod storage;
pub mod templates;
pub mod updates;
pub mod vpm;

use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Orbiters")
}

/// The shared VPM environment VCC and ALCOM keep (settings, project list, repositories, templates). Without Creator
/// Companion it starts empty and VPM creates what it needs, as ALCOM does. `ORBITERS_VCC_DIR` points it elsewhere
/// (tests of a computer without VCC; never the person's real folder).
pub fn vcc_dir() -> PathBuf {
    match std::env::var_os("ORBITERS_VCC_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("VRChatCreatorCompanion"),
    }
}

/// Where Creator Companion itself is installed (its template covers), from its uninstall entry; none when it is not.
/// `ORBITERS_VCC_APP_DIR` replaces it (empty: not installed).
pub fn vcc_app_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ORBITERS_VCC_APP_DIR") {
        return (!dir.is_empty()).then(|| PathBuf::from(dir)).filter(|p| p.is_dir());
    }
    uninstall_entry(|key, name| key == "{A20FE4C3-FE52-495B-B0DA-92992240BFC0}_is1" || name.starts_with("VRChat Creator Companion"))
        .and_then(|(location, _)| location)
        .filter(|p| p.is_dir())
}

/// The Windows uninstall entry `matches` picks (by its key and display name): its install folder and display icon.
#[allow(unused_variables)]
fn uninstall_entry(matches: impl Fn(&str, &str) -> bool) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        const UNINSTALL: [&str; 2] =
            [r"Software\Microsoft\Windows\CurrentVersion\Uninstall", r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"];
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for path in UNINSTALL {
                let Ok(root) = winreg::RegKey::predef(hive).open_subkey(path) else { continue };
                for key in root.enum_keys().flatten() {
                    let Ok(entry) = root.open_subkey(&key) else { continue };
                    let name: String = entry.get_value("DisplayName").unwrap_or_default();
                    if matches(&key, &name) {
                        let value = |v: &str| entry.get_value::<String, _>(v).ok().map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
                        // DisplayIcon is "path,index".
                        let icon = value("DisplayIcon")
                            .map(|s| PathBuf::from(s.rsplit_once(',').filter(|(_, i)| i.parse::<i32>().is_ok()).map_or(s.as_str(), |(p, _)| p)));
                        return Some((value("InstallLocation").map(PathBuf::from), icon));
                    }
                }
            }
        }
    }
    None
}

/// Unity Hub's program: where VCC was told it is, else where its installer says, else its usual folder.
pub fn unity_hub() -> Option<PathBuf> {
    let settings: serde_json::Value = std::fs::read(vcc_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let configured = settings["pathToUnityHub"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from);
    let installed = uninstall_entry(|key, name| key == "Unity Technologies - Hub" || name.starts_with("Unity Hub")).and_then(|(location, icon)| {
        icon.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))).or(location.map(|l| l.join("Unity Hub.exe")))
    });
    [configured, installed, Some(PathBuf::from(r"C:\Program Files\Unity Hub\Unity Hub.exe"))].into_iter().flatten().find(|p| p.is_file())
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
    storage::prepare(&project.path)?;
    hidden_command(editor).arg("-projectPath").arg(&project.path).spawn()?;
    Ok(())
}

pub fn open_hub() -> anyhow::Result<()> {
    let path = unity_hub().ok_or_else(|| anyhow::anyhow!("Unity Hub is not installed. Download it from unity.com/download."))?;
    hidden_command(path).spawn()?;
    Ok(())
}
