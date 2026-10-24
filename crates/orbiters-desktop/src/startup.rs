//! Starting Orbiters - N4N0 with Windows (the person's own registry) and with SteamVR (among its startup overlay apps,
//! through the app's manifest). Settings shows both and changes them at once. SteamVR is told only while it runs (it
//! is never started for this): a choice made while it does not, here or in the setup, waits in the settings
//! (`startup.steamvr_pending`), and the service tells SteamVR the next time it runs. Otherwise the service takes what
//! SteamVR says then, which its own settings may have changed.

use orbiters_core::settings::StartupSettings;
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
    let path = orbiters_update::steamvr_manifest_path().ok_or("no settings folder for SteamVR's manifest")?;
    Ok((exe, path))
}

/// Whether Windows starts the app when the person signs in (this copy or another one).
pub fn starts_with_windows() -> bool {
    #[cfg(windows)]
    {
        orbiters_update::windows_start::program().is_some()
    }
    #[cfg(not(windows))]
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
    #[cfg(not(windows))]
    {
        let _ = on;
        anyhow::bail!("starting with the system is for Windows only")
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
