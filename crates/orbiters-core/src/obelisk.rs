//! VRChat on a Mac: what runs it there is Obelisk (github.com/Zagrios/Obelisk, GPL-3.0, a separate app that N4N0 only
//! opens and reads the files of, never a line of its code). Its Linux engine (Wine for Linux in native macOS processes,
//! x86 through FEX) starts VRChat's Easy Anti-Cheat as on a Steam Deck and draws its Direct3D 11 with DXMT; Obelisk's
//! own profile of VRChat chooses both, in its beta versions (Settings > Updates > Beta versions) as of November 2026.
//!
//! Obelisk downloads VRChat with the person's Steam account and has no way for another program to press its Play
//! (no link scheme, no command line: read at its commit 265eb5b). So here: is Obelisk on this Mac, did it install
//! VRChat, is that VRChat running. The desktop (`controller/launcher.rs`) opens Obelisk for the person to press Play.
//! All of it pure, over paths and texts, so that it is tested on every computer.
use std::path::{Path, PathBuf};

/// Obelisk's bundle identifier, which is also the name of its folder in `~/Library/Application Support`.
pub const BUNDLE_ID: &str = "com.zagrios.obelisk";
/// Where a person gets it.
pub const SITE: &str = "https://getobelisk.app";

/// The home of the person at this Mac.
pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Obelisk's library folder of the person whose home is `home`: its receipts, its Wine prefixes, its engine.
pub fn library(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join(BUNDLE_ID)
}

/// Where the app itself is looked for (the Finder's "Applications" of the computer, then the person's own).
pub fn app_places(home: &Path) -> [PathBuf; 2] {
    [PathBuf::from("/Applications/Obelisk.app"), home.join("Applications").join("Obelisk.app")]
}

/// The folders one receipt file (`steam-installations-<steam id>.json`: an array of installations, each with Steam's
/// `appId` and its `directory`) says VRChat is installed in. A text that is not such a file gives none.
pub fn vrchat_directories(receipts: &str) -> Vec<PathBuf> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(receipts) else { return Vec::new() };
    items
        .iter()
        .filter(|r| r.get("appId").and_then(serde_json::Value::as_u64).map(|id| id.to_string()).as_deref() == Some(crate::vrchat_launch::STEAM_APP))
        .filter_map(|r| r.get("directory").and_then(serde_json::Value::as_str))
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Whether `name` is a receipt file of installed Steam games (the `.incomplete.json` beside it lists downloads that did
/// not finish).
pub fn is_receipt_file(name: &str) -> bool {
    name.starts_with("steam-installations-") && name.ends_with(".json") && !name.ends_with(".incomplete.json")
}

/// VRChat's folder as Obelisk installed it, from the receipts in `library`: the first one whose game is really there.
pub fn vrchat_install(library: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(library).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_receipt_file)).collect();
    files.sort();
    files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|text| vrchat_directories(&text))
        .find(|d| d.join("VRChat.exe").is_file() || d.join("start_protected_game.exe").is_file())
}

/// Whether a process with this command line belongs to the VRChat that Obelisk runs from `install`. The engine's
/// launch script is given the installation's folder as an argument of its own and lives as long as the game; the
/// game's own processes name `VRChat.exe` or the anti-cheat's `start_protected_game.exe` of that folder.
pub fn is_vrchat_process(command: &[String], install: Option<&Path>) -> bool {
    let install = install.map(|p| p.to_string_lossy().trim_end_matches('/').to_string()).filter(|p| !p.is_empty());
    command.iter().any(|arg| {
        let lower = arg.to_lowercase();
        let own = |exe: &str| lower == exe || lower.ends_with(&format!("/{exe}")) || lower.ends_with(&format!("\\{exe}"));
        own("vrchat.exe") || install.as_deref().is_some_and(|dir| arg.trim_end_matches('/') == dir || (arg.starts_with(dir) && own("start_protected_game.exe")))
    })
}

/// What a Mac has of what VRChat needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Obelisk is not on this Mac.
    Missing,
    /// Obelisk is there and has not installed VRChat (or not all of it yet).
    NoVrchat,
    Ready,
}

/// Looks at the Mac of the person whose home is `home`.
pub fn engine(home: &Path) -> Engine {
    let library = library(home);
    if vrchat_install(&library).is_some() {
        return Engine::Ready;
    }
    if app_places(home).iter().any(|p| p.is_dir()) || library.is_dir() {
        Engine::NoVrchat
    } else {
        Engine::Missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIPTS: &str = r#"[
        {"appId": 252950, "language": "english", "name": "Rocket League", "directory": "/Games/Rocket League", "platform": "windows", "totalBytes": 1},
        {"appId": 438100, "language": "english", "name": "VRChat", "directory": "/Games/VRChat", "platform": "windows", "totalBytes": 2}
    ]"#;

    #[test]
    fn a_receipt_file_says_where_vrchat_is() {
        assert_eq!(vrchat_directories(RECEIPTS), [PathBuf::from("/Games/VRChat")]);
        assert!(vrchat_directories("not json").is_empty());
        assert!(vrchat_directories(r#"{"appId": 438100, "directory": "/x"}"#).is_empty(), "an array, or nothing");
        assert!(vrchat_directories(r#"[{"appId": 438100}, {"appId": "438100", "directory": "/x"}, {"appId": 438100, "directory": ""}]"#).is_empty());
        assert!(is_receipt_file("steam-installations-76561198000000000.json"));
        assert!(!is_receipt_file("steam-installations-76561198000000000.incomplete.json"), "a download that did not finish is no installation");
        assert!(!is_receipt_file("epic-installations-1.json"));
    }

    #[test]
    fn the_mac_has_obelisk_and_vrchat_or_says_which_is_missing() {
        struct Home(PathBuf);
        impl Home {
            fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Home {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let home = Home(std::env::temp_dir().join(format!("orbiters-obelisk-test-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(home.path());
        std::fs::create_dir_all(home.path()).unwrap();
        // `/Applications/Obelisk.app` of the computer running the test would answer for it.
        let engine = |home: &Path| if vrchat_install(&library(home)).is_some() { Engine::Ready } else if library(home).is_dir() { Engine::NoVrchat } else { Engine::Missing };
        assert_eq!(engine(home.path()), Engine::Missing);
        let library = library(home.path());
        std::fs::create_dir_all(&library).unwrap();
        assert_eq!(engine(home.path()), Engine::NoVrchat, "Obelisk has run here and has no VRChat");
        let game = home.path().join("Games").join("VRChat");
        let receipt = serde_json::json!([{"appId": 438100, "name": "VRChat", "directory": game, "platform": "windows", "totalBytes": 2}]).to_string();
        std::fs::write(library.join("steam-installations-42.json"), &receipt).unwrap();
        assert_eq!(engine(home.path()), Engine::NoVrchat, "a receipt whose folder is gone (a disk not connected) is no game to play");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("start_protected_game.exe"), b"").unwrap();
        assert_eq!(vrchat_install(&library), Some(game.clone()));
        assert_eq!(engine(home.path()), Engine::Ready);
        std::fs::remove_file(library.join("steam-installations-42.json")).unwrap();
        std::fs::write(library.join("steam-installations-42.incomplete.json"), &receipt).unwrap();
        assert_eq!(engine(home.path()), Engine::NoVrchat, "still downloading");
    }

    #[test]
    fn vrchats_processes_are_told_from_the_others() {
        let install = Path::new("/Games/VRChat");
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_vrchat_process(&args(&["/bin/bash", "/lib/launch.sh", "/voile", "/home", "/Games/VRChat", "/opt/game", "--", "/opt/game/start_protected_game.exe"]), Some(install)));
        assert!(is_vrchat_process(&args(&["Z:\\opt\\game\\VRChat.exe", "--fps=90"]), None));
        assert!(is_vrchat_process(&args(&["fexdarwin-run", "/opt/game/VRChat.exe"]), None));
        assert!(is_vrchat_process(&args(&["wine", "/Games/VRChat/start_protected_game.exe"]), Some(install)));
        assert!(!is_vrchat_process(&args(&["/bin/bash", "/lib/launch.sh", "/voile", "/home", "/Games/Rocket League"]), Some(install)));
        assert!(!is_vrchat_process(&args(&["wine", "/Games/Other/start_protected_game.exe"]), Some(install)), "another game's anti-cheat");
        assert!(!is_vrchat_process(&args(&["/Applications/NotVRChat.exe.app/x", "vrchat.exe.log"]), Some(install)));
        assert!(!is_vrchat_process(&[], Some(install)));
    }
}
