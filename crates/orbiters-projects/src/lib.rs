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
pub mod scene;
pub mod setup;
pub mod size;
pub mod storage;
pub mod suggestions;
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

/// What Open in Unity did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Opened {
    /// Unity had this project open already: its window was brought to the front (if it has one yet) rather than a second
    /// one started.
    pub already_open: bool,
    /// A Library that barely existed was moved to a disk with room first ([`storage::preflight`]).
    pub moved: Option<PathBuf>,
}

/// How long Open in Unity may take before it says so. Everything it does is a moment's work on a disk that answers; a
/// disk that does not (a USB disk resetting) must not leave "Opening Unity…" spinning with every other action greyed out.
const OPEN_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a started editor is watched: Unity that cannot run at all ends within this. Longer would keep the toast up
/// for nothing, since the panel sees the editor by itself from then on.
const START_WATCH: std::time::Duration = std::time::Duration::from_millis(1500);

/// Runs `f` on its own thread and gives up waiting after `limit` (`None`; the thread then ends by itself, if ever). For
/// calls that cannot be interrupted and may block on a disk or a locked database.
pub(crate) fn within<T: Send + 'static>(limit: std::time::Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok()
}

/// Opens `project` in its Unity version and returns once the editor has started (or was already there). Always prompt:
/// never a long operation, which [`storage::repair`] is, asked for separately. Call on a worker.
pub fn open_project(project: &discovery::Project) -> anyhow::Result<Opened> {
    let name = project.name.clone();
    let p = project.clone();
    within(OPEN_LIMIT, move || open_now(&p)).unwrap_or_else(|| {
        anyhow::bail!("Opening {name} is taking too long: its disk or Unity's list of editors is not answering. Unity may still open by itself.")
    })
}

fn open_now(project: &discovery::Project) -> anyhow::Result<Opened> {
    anyhow::ensure!(
        discovery::is_project(&project.path),
        "{} is not at {} any more. If it is on a disk that is not connected, connect it and try again.",
        project.name,
        project.path.display()
    );
    let locked = unity_open(&project.path);
    let editors = editor::running();
    if let Some(open) = editor::of(&editors, &project.path, locked) {
        // Unity still starting has no window to bring forward yet; it is open all the same.
        let _ = editor::show(open.pid, &project.path);
        return Ok(Opened { already_open: true, moved: None });
    }
    // Unity would only say the same, in a dialog of its own, and quit.
    anyhow::ensure!(
        !locked,
        "Something else holds {}'s lock (its Temp/UnityLockfile): Unity still quitting, or Orbiters repairing its storage. Try again in a moment.",
        project.name
    );
    anyhow::ensure!(!project.unity.is_empty(), "{} does not say which Unity version it uses (ProjectSettings/ProjectVersion.txt).", project.name);
    let editor = discovery::find_editor(&project.unity)
        .ok_or_else(|| anyhow::anyhow!("Unity {} is not installed. Open Unity Hub to install this version.", project.unity))?;
    // Only ever instant. A problem with it must not stop Unity opening: the panel's warning says what to do about the disk.
    let moved = storage::preflight(&project.path).unwrap_or_else(|e| {
        eprintln!("the storage check of {} did not finish: {e:#}", project.path.display());
        None
    });
    let mut command = hidden_command(&editor);
    command.arg("-projectPath").arg(&project.path);
    start_watched(command, START_WATCH).map_err(|e| anyhow::anyhow!("Unity {} ({}) {e:#}", project.unity, editor.display()))?;
    Ok(Opened { already_open: false, moved })
}

/// Starts `command` and watches it for `watch`: a program that could not start, or that quit at once with a failure, is an
/// error rather than a success said too soon. One still running, or ended well (it handed over to another process), is
/// started.
fn start_watched(mut command: std::process::Command, watch: std::time::Duration) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut child = command.spawn().context("could not be started")?;
    let start = std::time::Instant::now();
    while start.elapsed() < watch {
        if let Some(status) = child.try_wait().context("could not be watched")? {
            anyhow::ensure!(status.success(), r"quit right after starting ({status}). Its log is at %LOCALAPPDATA%\Unity\Editor\Editor.log.");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

pub fn open_hub() -> anyhow::Result<()> {
    let path = unity_hub().ok_or_else(|| anyhow::anyhow!("Unity Hub is not installed. Download it from unity.com/download."))?;
    hidden_command(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod open_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    fn cmd(args: &[&str]) -> std::process::Command {
        let mut c = hidden_command("cmd");
        c.arg("/c").args(args);
        c
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_quits_at_once_with_a_failure_is_said_to_have_failed() {
        let error = start_watched(cmd(&["exit", "3"]), Duration::from_secs(10)).unwrap_err();
        assert!(format!("{error:#}").contains("quit right after starting"), "{error:#}");
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_ended_well_or_goes_on_running_is_started_without_waiting_for_it() {
        start_watched(cmd(&["exit", "0"]), Duration::from_secs(10)).unwrap();
        let start = Instant::now();
        start_watched(cmd(&["ping", "-n", "4", "127.0.0.1"]), Duration::from_millis(200)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2), "the watch ends the wait, not the program");
    }

    #[test]
    fn a_program_that_cannot_start_says_so() {
        let error = start_watched(hidden_command("Z:/no/such/Unity.exe"), Duration::from_secs(1)).unwrap_err();
        assert!(format!("{error:#}").contains("could not be started"), "{error:#}");
    }

    #[test]
    fn a_call_that_blocks_is_given_up_on() {
        let start = Instant::now();
        assert_eq!(within(Duration::from_millis(50), || std::thread::sleep(Duration::from_secs(5))), None);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(within(Duration::from_secs(5), || 7), Some(7));
    }

    #[test]
    fn a_project_gone_from_its_folder_is_not_handed_to_unity() {
        // Unity would make a new, empty project there.
        let temp = tempfile::tempdir().unwrap();
        let project = discovery::Project { name: "Gone".into(), path: temp.path().join("gone"), unity: "2022.3.22f1".into(), ..Default::default() };
        let error = open_now(&project).unwrap_err();
        assert!(format!("{error:#}").contains("is not at"), "{error:#}");
        assert!(!temp.path().join("gone").exists());
    }
}
