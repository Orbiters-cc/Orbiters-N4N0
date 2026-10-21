//! Default locations. Everything the user produces lives under `Documents/Orbiters/`; settings under the
//! platform config folder (`%APPDATA%/Orbiters/Desktop` on Windows, `~/.config/orbiters-desktop` on Linux).

use std::path::PathBuf;

/// `Documents/Orbiters`.
pub fn documents_root() -> PathBuf {
    dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from(".")).join("Orbiters")
}

/// Default folder for session bundles.
pub fn default_sessions_dir() -> PathBuf {
    documents_root().join("Sessions")
}

/// Default folder where Mixer's capture simulator writes raw captures.
pub fn default_raw_captures_dir() -> PathBuf {
    documents_root().join("Raw captures")
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(windows) {
        base.join("Orbiters").join("Desktop")
    } else {
        base.join("orbiters-desktop")
    }
}

/// Where Unity editors running Mixer take requests ("Send to Mixer"): `%LOCALAPPDATA%/Orbiters/Mixer/inbox`, one
/// folder per editor process.
pub fn mixer_inbox_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("Orbiters").join("Mixer").join("inbox")
}

/// `%LOCALAPPDATA%/Orbiters` (`~/.local/share/Orbiters` on Linux): data shared by the Orbiters apps and tools on
/// this computer, too big or too local for the config folder.
pub fn local_data_root() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("Orbiters")
}

/// Avatar manifests Orbiters Toolkit writes while the VRChat SDK builds an avatar (contracts `Docs/avatar-manifest.md`).
pub fn avatars_dir() -> PathBuf {
    local_data_root().join("Avatars")
}

/// Speech models Orb downloads when the person asks for them.
pub fn models_dir() -> PathBuf {
    local_data_root().join("Models")
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn logs_dir() -> PathBuf {
    config_dir().join("logs")
}

/// VRChat's log folder on this machine, when it exists.
pub fn vrchat_log_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join("AppData").join("LocalLow").join("VRChat").join("VRChat"));
        }
    } else if let Some(home) = dirs::home_dir() {
        // Steam Proton prefix (default library and Flatpak Steam).
        for steam in [".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
            candidates.push(home.join(steam).join("steamapps/compatdata/438100/pfx/drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat"));
        }
    }
    candidates.into_iter().find(|p| p.is_dir())
}
