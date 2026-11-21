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

/// Obelisk's latest release, as GitHub tells it (no account, nothing of the person in the request).
pub const LATEST: &str = "https://api.github.com/repos/Zagrios/Obelisk/releases/latest";
/// Where a release's files are, and nowhere else is one taken from.
const DOWNLOADS: &str = "https://github.com/Zagrios/Obelisk/releases/download/";
/// The largest disk image taken (0.6.4's is 26 MB).
const LARGEST: u64 = 512 << 20;
/// What `codesign --verify -R` holds a downloaded Obelisk to: Obelisk's own identifier, signed with the Developer ID of
/// its maker's team (Mathieu GRIES-PEROZ, FGW3Z8B82D, read from its 0.6.4). Another program under its name, or Obelisk
/// signed by anyone else, is not installed.
pub const REQUIREMENT: &str = r#"anchor apple generic and identifier "com.zagrios.obelisk" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "FGW3Z8B82D""#; // i18n: not shown (a code requirement, in codesign's language)
/// Obelisk's own setting "Beta versions" (Settings > Updates), in its preferences (`defaults`, domain [`BUNDLE_ID`]):
/// VRChat's support is in the beta versions of its engine as of November 2026.
pub const BETA_SETTING: &str = "obelisk.updates.betaReleases";

/// The disk image of a release, to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The address of the file that says its SHA-256, published beside it.
    pub checksum_url: String,
}

/// The Apple silicon disk image that GitHub's answer for a release names, with its checksum file. An answer without
/// both, or naming files anywhere but Obelisk's releases, gives none.
pub fn latest_download(release: &str) -> Option<Download> {
    let release: serde_json::Value = serde_json::from_str(release).ok()?;
    let assets: Vec<(&str, &str, u64)> = release
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|a| Some((a.get("name")?.as_str()?, a.get("browser_download_url")?.as_str()?, a.get("size")?.as_u64()?)))
        .filter(|(name, url, _)| url.starts_with(DOWNLOADS) && url.ends_with(&format!("/{name}")) && !name.contains(['/', '\\']))
        .collect();
    let (name, url, size) = *assets.iter().find(|(name, _, size)| name.starts_with("Obelisk-") && name.ends_with("-arm64.dmg") && (1..=LARGEST).contains(size))?;
    let (_, checksum_url, _) = *assets.iter().find(|(other, _, _)| *other == format!("{name}.sha256"))?;
    Some(Download { name: name.into(), url: url.into(), size, checksum_url: checksum_url.into() })
}

/// The SHA-256 a checksum file says (`<64 hex digits>  <file name>`), in lower case.
pub fn checksum(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    (first.len() == 64 && first.bytes().all(|b| b.is_ascii_hexdigit())).then(|| first.to_ascii_lowercase())
}

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

/// The process of the VRChat that one of Obelisk's session records (`SteamSessions/<id>.v2.json`, which its supervisor
/// keeps while a Steam game runs: read from Obelisk 0.6.4's) says is running: Steam's VRChat, not finished, with the
/// game's process named. The caller checks that the process still lives (a record outlives a crash).
pub fn session_game(record: &str) -> Option<u32> {
    let record: serde_json::Value = serde_json::from_str(record).ok()?;
    let app = record.get("store")?.get("app_id")?.as_u64()?;
    if app.to_string() != crate::vrchat_launch::STEAM_APP || record.get("phase").and_then(serde_json::Value::as_str) == Some("Finished") { // i18n: not shown (a value in Obelisk's file)
        return None;
    }
    record.get("game")?.get("pid")?.as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 1)
}

/// The processes Obelisk's session records in `library` name as a running VRChat.
pub fn session_games(library: &Path) -> Vec<u32> {
    let Ok(files) = std::fs::read_dir(library.join("SteamSessions")) else { return Vec::new() };
    files.flatten().filter(|f| f.file_name().to_string_lossy().ends_with(".v2.json")).filter_map(|f| std::fs::read_to_string(f.path()).ok()).filter_map(|text| session_game(&text)).collect()
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
    fn a_release_names_the_disk_image_to_download() {
        let release = r#"{"tag_name": "v0.6.4", "assets": [
            {"name": "appcast.xml", "size": 7861, "browser_download_url": "https://github.com/Zagrios/Obelisk/releases/download/v0.6.4/appcast.xml"},
            {"name": "Obelisk-0.6.4-arm64.dmg", "size": 26422576, "browser_download_url": "https://github.com/Zagrios/Obelisk/releases/download/v0.6.4/Obelisk-0.6.4-arm64.dmg"},
            {"name": "Obelisk-0.6.4-arm64.dmg.sha256", "size": 90, "browser_download_url": "https://github.com/Zagrios/Obelisk/releases/download/v0.6.4/Obelisk-0.6.4-arm64.dmg.sha256"}
        ]}"#;
        let download = latest_download(release).unwrap();
        assert_eq!(download.name, "Obelisk-0.6.4-arm64.dmg");
        assert_eq!(download.size, 26422576);
        assert!(download.checksum_url.ends_with("/Obelisk-0.6.4-arm64.dmg.sha256"));
        assert_eq!(latest_download("not json"), None);
        assert_eq!(latest_download(&release.replace("-arm64.dmg.sha256", "-arm64.dmg.md5")), None, "no checksum, no download");
        assert_eq!(latest_download(&release.replace("https://github.com/Zagrios/Obelisk/", "https://example.com/Zagrios/Obelisk/")), None, "only from Obelisk's releases");
        assert_eq!(latest_download(&release.replace("26422576", "99999999999")), None, "not a disk image of that size");
        assert_eq!(latest_download(&release.replace("arm64", "x86_64")), None, "Apple silicon only");

        let sum = "1684f0cdea69a38770e70a934f75054454d9bd16f79571e0074292e11dec330c";
        assert_eq!(checksum(&format!("{sum}  Obelisk-0.6.4-arm64.dmg\n")).as_deref(), Some(sum));
        assert_eq!(checksum(&sum.to_uppercase()).as_deref(), Some(sum));
        assert_eq!(checksum("<html>Not Found</html>"), None);
        assert_eq!(checksum(""), None);
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
    fn a_session_record_names_the_running_vrchat() {
        let record = |app: u32, phase: &str, game: &str| format!(r#"{{"version":2,"id":"f2766c7f","account_id":"1","store":{{"app_id":{app},"host":"running"}},"phase":"{phase}","supervisor":{{"pid":48097,"uid":501}},"game":{game},"starting":null,"socket":"/tmp/x/control"}}"#);
        assert_eq!(session_game(&record(438100, "Running", r#"{"pid":48200,"uid":501,"executable":"/x/wine"}"#)), Some(48200));
        assert_eq!(session_game(&record(438100, "Finished", r#"{"pid":48200,"uid":501}"#)), None, "as Obelisk leaves it once the game closed");
        assert_eq!(session_game(&record(438100, "Running", "null")), None, "not started yet");
        assert_eq!(session_game(&record(252950, "Running", r#"{"pid":48200,"uid":501}"#)), None, "another game");
        assert_eq!(session_game(r#"{"version":2,"id":"f2766c7f"}"#), None, "the pointer to the last session");
        assert_eq!(session_game("not json"), None);
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
