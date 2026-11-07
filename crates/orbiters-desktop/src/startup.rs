//! Starting Orbiters - N4N0 with Windows (the person's own registry) and with SteamVR (among its startup overlay apps,
//! through the app's manifest). Settings shows both and changes them at once. SteamVR is told only while it runs (it
//! is never started for this): a choice made while it does not, here or in the setup, waits in the settings
//! (`startup.steamvr_pending`), and the service tells SteamVR the next time it runs. Otherwise the service takes what
//! SteamVR says then, which its own settings may have changed.

use orbiters_core::settings::StartupSettings;
use orbiters_i18n::tr;
use orbiters_openvr::applications::{self, Manifest};
use orbiters_update::{DISPLAY_NAME, STEAMVR_APP_KEY, STEAMVR_ARGUMENTS, STEAMVR_DESCRIPTION};
use std::path::{Path, PathBuf};

/// SteamVR's description of the program at `exe`.
fn manifest(exe: &Path) -> Manifest<'_> {
    Manifest { key: STEAMVR_APP_KEY, binary: exe, arguments: STEAMVR_ARGUMENTS, name: DISPLAY_NAME, description: STEAMVR_DESCRIPTION }
}

/// This program and where its SteamVR manifest goes.
fn this_program() -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = orbiters_update::steamvr_manifest_path().ok_or_else(|| tr("no settings folder for SteamVR's manifest"))?;
    Ok((exe, path))
}

/// Whether Windows starts the app when the person signs in (this copy or another one).
pub fn starts_with_windows() -> bool {
    #[cfg(windows)]
    {
        orbiters_update::windows_start::program().is_some()
    }
    #[cfg(target_os = "macos")]
    {
        mac::is_set()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

/// Has Windows start this program, in the tray, when the person signs in; or no longer.
pub fn set_start_with_windows(on: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        orbiters_update::windows_start::set(&std::env::current_exe()?, on)
    }
    #[cfg(target_os = "macos")]
    {
        mac::set(on)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = on;
        anyhow::bail!("{}", tr("starting with the system is for Windows only"))
    }
}

/// Starting at login on the Mac: a launch agent of the person's own (`~/Library/LaunchAgents`, no administrator rights)
/// that runs the app in the menu bar.
#[cfg(target_os = "macos")]
pub mod mac {
    use std::path::{Path, PathBuf};

    const LABEL: &str = "cc.orbiters.n4n0";

    fn agent_file() -> anyhow::Result<PathBuf> {
        Ok(std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("{}", tr("no home folder")))?.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// The launch agent that starts `exe` at login, in the menu bar.
    pub fn agent(exe: &Path) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--hidden</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>
"#,
            escape(&exe.to_string_lossy())
        )
    }

    pub fn is_set() -> bool {
        agent_file().is_ok_and(|p| p.is_file())
    }

    pub fn set(on: bool) -> anyhow::Result<()> {
        let path = agent_file()?;
        if on {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, agent(&std::env::current_exe()?))?;
        } else if let Err(e) = std::fs::remove_file(&path) {
            anyhow::ensure!(e.kind() == std::io::ErrorKind::NotFound, "{}: {e}", path.display());
        }
        Ok(())
    }

    /// An app that moved or was updated elsewhere keeps its start: the agent names the program running now.
    pub fn follow_this_program() {
        if !is_set() {
            return;
        }
        let (Ok(path), Ok(exe)) = (agent_file(), std::env::current_exe()) else { return };
        let wanted = agent(&exe);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(wanted.as_str()) {
            let _ = std::fs::write(&path, wanted);
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_agent_names_the_program_in_the_menu_bar() {
            let text = super::agent(std::path::Path::new("/Applications/Orbiters & co.app/Contents/MacOS/orbiters-desktop"));
            assert!(text.contains("<string>/Applications/Orbiters &amp; co.app/Contents/MacOS/orbiters-desktop</string>"));
            assert!(text.contains("<string>--hidden</string>") && text.contains("<key>RunAtLoad</key>"));
        }
    }
}

/// Has SteamVR start this program with itself, or no longer: `Ok(true)` once it was told, `Ok(false)` when SteamVR is not
/// running (the choice then waits for it in the settings).
pub fn set_start_with_steamvr(on: bool) -> Result<bool, String> {
    let (exe, path) = this_program()?;
    applications::set_start_with_steamvr(&manifest(&exe), &path, on)
}

/// What the settings keep once SteamVR, which runs, was asked: a choice still waiting is told to it; else what it says
/// is taken, and its manifest made to name this program if it starts the app (an install that moved). `None`: nothing
/// to change.
pub fn reconcile(client: &orbiters_openvr::Client, startup: &StartupSettings) -> Option<StartupSettings> {
    let (exe, path) = this_program().ok()?;
    let manifest = manifest(&exe);
    let mut next = startup.clone();
    if startup.steamvr_pending {
        match client.set_start_with_steamvr(&manifest, &path, startup.steamvr) {
            Ok(()) => next.steamvr_pending = false,
            Err(e) => eprintln!("SteamVR's start of the app: {e}"),
        }
    } else {
        next.steamvr = client.starts_with_steamvr(STEAMVR_APP_KEY);
        if next.steamvr && std::fs::read_to_string(&path).ok().as_deref() != Some(manifest.json().as_str()) {
            if let Err(e) = client.set_start_with_steamvr(&manifest, &path, true) {
                eprintln!("SteamVR's manifest of the app: {e}");
            }
        }
    }
    (next != *startup).then_some(next)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn the_setup_leaves_its_choice_where_the_app_reads_its_settings() {
        assert_eq!(orbiters_update::app_settings_file(), Some(orbiters_core::paths::settings_file()));
    }
}
