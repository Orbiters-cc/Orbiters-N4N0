//! VRChat on a Mac: what runs it there is Obelisk's engine (github.com/Zagrios/Obelisk, GPL-3.0: Wine for Linux in
//! native macOS processes, x86 through FEX, so that VRChat's Easy Anti-Cheat starts as on a Steam Deck, Direct3D 11
//! drawn with DXMT). N4N0 never shows Obelisk's window: it runs `obelisk-cli`, the engine without a window, from the
//! fork at github.com/Orbiters-cc/Obelisk (branch `cli`, GPL-3.0 as Obelisk is; `docs/CLI.md` there is the contract
//! this module reads). It is a separate program N4N0 downloads and starts, never a line of its code in N4N0.
//!
//! The program signs in to Steam (the person's own account owns VRChat), downloads VRChat and the engine, and starts
//! the game with the arguments N4N0 gives. Here: which release to download, where things are kept, what the program
//! says (its JSON Lines), whether VRChat is installed and running. All of it pure, over paths and texts, so that it is
//! tested on every computer; the desktop (`controller/obelisk_cli.rs`) downloads and runs.
use std::path::{Path, PathBuf};

/// VRChat at Steam.
pub use crate::vrchat_launch::STEAM_APP;

/// The fork's releases, as GitHub tells them (no account, nothing of the person in the request).
pub const RELEASES: &str = "https://api.github.com/repos/Orbiters-cc/Obelisk/releases?per_page=30";
/// Where a release's files are, and nowhere else is one taken from.
const DOWNLOADS: &str = "https://github.com/Orbiters-cc/Obelisk/releases/download/";
/// The largest archive taken (0.1.0's is 11 MB).
const LARGEST: u64 = 256 << 20;
/// The first part of a release's tag (`cli-v0.1.0`).
const TAG: &str = "cli-v";
/// What `codesign --verify -R` holds each downloaded program to: signed by Orbiters' own team (Enzo Dutra,
/// Z57XP83A9Z; an Apple Development or a Developer ID certificate of it). A program signed by anyone else is not run.
pub const REQUIREMENT: &str = r#"anchor apple generic and certificate leaf[subject.OU] = "Z57XP83A9Z""#; // i18n: not shown (a code requirement, in codesign's language)
/// The programs of a release that are checked against [`REQUIREMENT`], from its folder.
pub const PROGRAMS: [&str; 4] = ["obelisk-cli", "SteamNative/obelisk-steam-host", "SteamNative/obelisk-steam-sdk-x86_64", "SteamNative/libobelisk_steam_discovery_adapter.dylib"];

/// The archive of a release, to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    /// `0.1.0`.
    pub version: String,
    /// The archive's name, and without `.tar.gz` the one folder it holds.
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The address of the file that says its SHA-256, published beside it.
    pub checksum_url: String,
}

impl Download {
    /// The folder the archive holds (`obelisk-cli-0.1.0-arm64`).
    pub fn folder(&self) -> &str {
        self.name.trim_end_matches(".tar.gz")
    }
}

/// A version's numbers, to tell the newer of two (`0.10.0` after `0.9.3`); none for anything else than numbers.
fn numbers(version: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = version.split('.').map(|n| n.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 4)
}

/// The newest Apple silicon archive GitHub's list of releases names, with its checksum file. A list without one, or
/// naming files anywhere but the fork's releases, gives none.
pub fn latest_download(releases: &str) -> Option<Download> {
    let releases: serde_json::Value = serde_json::from_str(releases).ok()?;
    releases
        .as_array()?
        .iter()
        .filter(|r| r.get("draft").and_then(serde_json::Value::as_bool) != Some(true))
        .filter_map(|release| {
            let version = release.get("tag_name")?.as_str()?.strip_prefix(TAG)?;
            let order = numbers(version)?;
            let assets: Vec<(&str, &str, u64)> = release
                .get("assets")?
                .as_array()?
                .iter()
                .filter_map(|a| Some((a.get("name")?.as_str()?, a.get("browser_download_url")?.as_str()?, a.get("size")?.as_u64()?)))
                .filter(|(name, url, _)| *url == format!("{DOWNLOADS}{TAG}{version}/{name}"))
                .collect();
            let name = format!("obelisk-cli-{version}-arm64.tar.gz");
            let (_, url, size) = *assets.iter().find(|(n, _, size)| *n == name && (1..=LARGEST).contains(size))?;
            let (_, checksum_url, _) = *assets.iter().find(|(n, _, _)| *n == format!("{name}.sha256"))?;
            Some((order, Download { version: version.into(), name, url: url.into(), size, checksum_url: checksum_url.into() }))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, download)| download)
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

/// The program's library of the person whose home is `home` (its own default): the games, the engine, the receipts,
/// the records of what runs.
pub fn library(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("cc.orbiters.obelisk-cli")
}

/// Where N4N0 keeps the releases it downloaded, in its settings' folder `config`.
pub fn tools(config: &Path) -> PathBuf {
    config.join("obelisk-cli")
}

/// The program of the newest release in `tools`, whole (its Steam helper beside it).
pub fn program(tools: &Path) -> Option<PathBuf> {
    std::fs::read_dir(tools)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let order = numbers(name.strip_prefix("obelisk-cli-")?.strip_suffix("-arm64")?)?;
            let folder = entry.path();
            (folder.join(PROGRAMS[0]).is_file() && folder.join(PROGRAMS[1]).is_file()).then(|| (order, folder.join(PROGRAMS[0])))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, program)| program)
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

/// VRChat's folder as the program installed it, from the receipts in `library`: the first one whose game is really there.
pub fn vrchat_install(library: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(library).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_receipt_file)).collect();
    files.sort();
    files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|text| vrchat_directories(&text))
        .find(|d| d.join("VRChat.exe").is_file() || d.join("start_protected_game.exe").is_file())
}

/// The process of the VRChat that one of the engine's session records (`SteamSessions/<id>.v2.json`, which the game's
/// supervisor keeps while it runs) says is running: Steam's VRChat, neither finished nor failed, with the game's
/// process named. The caller checks that the process still lives (a record outlives a crash).
pub fn session_game(record: &str) -> Option<u32> {
    let record: serde_json::Value = serde_json::from_str(record).ok()?;
    let app = record.get("store")?.get("app_id")?.as_u64()?;
    let over = matches!(record.get("phase").and_then(serde_json::Value::as_str), Some("Finished" | "Failed")); // i18n: not shown (values in the engine's file)
    if app.to_string() != crate::vrchat_launch::STEAM_APP || over {
        return None;
    }
    record.get("game")?.get("pid")?.as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 1)
}

/// The processes the session records in `library` name as a running VRChat.
pub fn session_games(library: &Path) -> Vec<u32> {
    let Ok(files) = std::fs::read_dir(library.join("SteamSessions")) else { return Vec::new() };
    files.flatten().filter(|f| f.file_name().to_string_lossy().ends_with(".v2.json")).filter_map(|f| std::fs::read_to_string(f.path()).ok()).filter_map(|text| session_game(&text)).collect()
}

/// What a Mac has of what VRChat needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// The program is not downloaded yet.
    Missing,
    /// The program is there and has not installed VRChat (or not all of it yet).
    NoVrchat,
    Ready,
}

/// Looks at what N4N0 downloaded into `tools` and what the program installed in `library`.
pub fn engine(tools: &Path, library: &Path) -> Engine {
    match (program(tools), vrchat_install(library)) {
        (None, _) => Engine::Missing,
        (Some(_), None) => Engine::NoVrchat,
        (Some(_), Some(_)) => Engine::Ready,
    }
}

/// What a download of the game is at ([`Event::Progress`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Steam is asked what to install, or the files there are read.
    Checking,
    /// Valve's Steam components or the engine (Voile) download.
    Engine,
    /// The game's files download.
    Game,
}

/// One line of what the program says (its JSON Lines), as far as N4N0 acts on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `status`: the Steam account signed in, if one is.
    Status { account: Option<String> },
    /// The address to show as a QR code, for the Steam app on the person's phone.
    Challenge { qr_url: String },
    SignedIn { account: String },
    Progress { phase: Phase, fraction: Option<f32> },
    Installed,
    /// The game's supervisor holds it.
    Starting,
    Running { pid: Option<u32> },
    Exited { code: Option<i64> },
    /// The command failed: its stable code, and a sentence in English for a log.
    Error { code: String, message: String },
    /// Anything else the program tells.
    Other,
}

/// Reads one line; a line that is not an event of the program gives none.
pub fn event(line: &str) -> Option<Event> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    Some(match value.get("event")?.as_str()? {
        "status" => Event::Status { account: value.get("account").and_then(|a| a.get("account_name")).and_then(serde_json::Value::as_str).map(str::to_owned) },
        "challenge" => Event::Challenge { qr_url: Some(text("qr_url")).filter(|url| url.starts_with("https://"))? },
        "signed_in" => Event::SignedIn { account: text("account_name") },
        "progress" => Event::Progress {
            phase: match value.get("phase").and_then(serde_json::Value::as_str) {
                Some("steam_components" | "voile") => Phase::Engine,
                Some("downloading") => Phase::Game,
                _ => Phase::Checking,
            },
            fraction: value.get("fraction").and_then(serde_json::Value::as_f64).map(|f| (f as f32).clamp(0.0, 1.0)),
        },
        "installed" => Event::Installed,
        "starting" => Event::Starting,
        "running" => Event::Running { pid: value.get("pid").and_then(serde_json::Value::as_u64).and_then(|pid| u32::try_from(pid).ok()) },
        "exited" => Event::Exited { code: value.get("exit_code").and_then(serde_json::Value::as_i64) },
        "error" => Event::Error { code: text("code"), message: text("message") },
        _ => Event::Other,
    })
}

/// VRChat's arguments for a start on a Mac, from those of a desktop start on a PC: the destination's link alone.
/// `--no-vr` is left out (a Mac has no SteamVR to keep out of, and people who run VRChat this way report starts that
/// hang with it), and so are the PC's performance options (process priorities, video decoding by the graphics card:
/// tuned for Windows, never tried under the engine, and not offered in a Mac's settings).
pub fn game_arguments(desktop: Vec<String>) -> Vec<String> {
    desktop.into_iter().filter(|a| a.starts_with("vrchat://")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIPTS: &str = r#"[
        {"appId": 252950, "language": "english", "name": "Rocket League", "directory": "/Games/Rocket League", "platform": "windows", "totalBytes": 1},
        {"appId": 438100, "language": "english", "name": "VRChat", "directory": "/Games/VRChat", "platform": "windows", "totalBytes": 2}
    ]"#;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("orbiters-obelisk-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Temp(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn release(version: &str) -> String {
        let at = format!("https://github.com/Orbiters-cc/Obelisk/releases/download/cli-v{version}");
        format!(
            r#"{{"tag_name": "cli-v{version}", "prerelease": true, "assets": [
            {{"name": "obelisk-cli-{version}-arm64.tar.gz", "size": 11295267, "browser_download_url": "{at}/obelisk-cli-{version}-arm64.tar.gz"}},
            {{"name": "obelisk-cli-{version}-arm64.tar.gz.sha256", "size": 97, "browser_download_url": "{at}/obelisk-cli-{version}-arm64.tar.gz.sha256"}}
        ]}}"#
        )
    }

    #[test]
    fn the_newest_release_names_the_archive_to_download() {
        let list = format!(r#"[{}, {}, {{"tag_name": "v0.6.4", "assets": []}}, {}]"#, release("0.9.3"), release("0.10.0"), release("0.1.0"));
        let download = latest_download(&list).unwrap();
        assert_eq!(download.version, "0.10.0", "by its numbers, not as text");
        assert_eq!(download.name, "obelisk-cli-0.10.0-arm64.tar.gz");
        assert_eq!(download.folder(), "obelisk-cli-0.10.0-arm64");
        assert!(download.checksum_url.ends_with("/obelisk-cli-0.10.0-arm64.tar.gz.sha256"));
        assert_eq!(latest_download("not json"), None);
        assert_eq!(latest_download("[]"), None);
        let one = format!("[{}]", release("0.1.0"));
        assert!(latest_download(&one).is_some());
        assert_eq!(latest_download(&one.replace(".tar.gz.sha256", ".tar.gz.md5")), None, "no checksum, no download");
        assert_eq!(latest_download(&one.replace("https://github.com/Orbiters-cc/", "https://github.com/someone-else/")), None, "only from the fork's releases");
        assert_eq!(latest_download(&one.replace("11295267", "99999999999")), None, "not an archive of that size");
        assert_eq!(latest_download(&one.replace("\"prerelease\": true", "\"draft\": true")), None, "a draft is not out");
        assert_eq!(latest_download(&one.replace("cli-v0.1.0\"", "cli-v../x\"")), None);

        let sum = "1684f0cdea69a38770e70a934f75054454d9bd16f79571e0074292e11dec330c";
        assert_eq!(checksum(&format!("{sum}  obelisk-cli-0.1.0-arm64.tar.gz\n")).as_deref(), Some(sum));
        assert_eq!(checksum(&sum.to_uppercase()).as_deref(), Some(sum));
        assert_eq!(checksum("<html>Not Found</html>"), None);
        assert_eq!(checksum(""), None);
    }

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
    fn the_mac_has_the_program_and_vrchat_or_says_which_is_missing() {
        let temp = Temp::new("engine");
        let (tools, library) = (tools(&temp.0), temp.0.join("library"));
        assert_eq!(engine(&tools, &library), Engine::Missing);
        // An older release, whole, and a newer one whose download was cut short.
        for (version, whole) in [("0.9.0", true), ("0.10.0", false)] {
            let folder = tools.join(format!("obelisk-cli-{version}-arm64"));
            std::fs::create_dir_all(folder.join("SteamNative")).unwrap();
            std::fs::write(folder.join("obelisk-cli"), b"").unwrap();
            if whole {
                std::fs::write(folder.join("SteamNative").join("obelisk-steam-host"), b"").unwrap();
            }
        }
        assert_eq!(program(&tools), Some(tools.join("obelisk-cli-0.9.0-arm64").join("obelisk-cli")), "only a whole release is run");
        std::fs::write(tools.join("obelisk-cli-0.10.0-arm64").join("SteamNative").join("obelisk-steam-host"), b"").unwrap();
        assert_eq!(program(&tools), Some(tools.join("obelisk-cli-0.10.0-arm64").join("obelisk-cli")), "the newest by its numbers");
        assert_eq!(engine(&tools, &library), Engine::NoVrchat);

        std::fs::create_dir_all(&library).unwrap();
        let game = temp.0.join("Games").join("VRChat");
        let receipt = serde_json::json!([{"appId": 438100, "name": "VRChat", "directory": game, "platform": "windows", "totalBytes": 2}]).to_string();
        std::fs::write(library.join("steam-installations-42.json"), &receipt).unwrap();
        assert_eq!(engine(&tools, &library), Engine::NoVrchat, "a receipt whose folder is gone is no game to play");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("start_protected_game.exe"), b"").unwrap();
        assert_eq!(vrchat_install(&library), Some(game.clone()));
        assert_eq!(engine(&tools, &library), Engine::Ready);
        std::fs::remove_file(library.join("steam-installations-42.json")).unwrap();
        std::fs::write(library.join("steam-installations-42.incomplete.json"), &receipt).unwrap();
        assert_eq!(engine(&tools, &library), Engine::NoVrchat, "still downloading");
    }

    #[test]
    fn a_session_record_names_the_running_vrchat() {
        let record = |app: u32, phase: &str, game: &str| format!(r#"{{"version":2,"id":"f2766c7f","account_id":"1","store":{{"app_id":{app},"host":"running"}},"phase":"{phase}","supervisor":{{"pid":48097,"uid":501}},"game":{game},"starting":null,"socket":"/tmp/x/control"}}"#);
        assert_eq!(session_game(&record(438100, "Running", r#"{"pid":48200,"uid":501,"executable":"/x/wine"}"#)), Some(48200));
        assert_eq!(session_game(&record(438100, "Finished", r#"{"pid":48200,"uid":501}"#)), None, "as the engine leaves it once the game closed");
        assert_eq!(session_game(&record(438100, "Failed", r#"{"pid":48200,"uid":501}"#)), None);
        assert_eq!(session_game(&record(438100, "Running", "null")), None, "not started yet");
        assert_eq!(session_game(&record(252950, "Running", r#"{"pid":48200,"uid":501}"#)), None, "another game");
        assert_eq!(session_game(r#"{"version":2,"id":"f2766c7f"}"#), None, "the pointer to the last session");
        assert_eq!(session_game("not json"), None);
    }

    #[test]
    fn the_program_s_lines_are_read_as_what_n4n0_acts_on() {
        assert_eq!(event(r#"{"event":"status","version":"0.1.0","account":null,"keychain":"ok"}"#), Some(Event::Status { account: None }));
        assert_eq!(event(r#"{"event":"status","account":{"steam_id":"7656","account_name":"someone"}}"#), Some(Event::Status { account: Some("someone".into()) }));
        assert_eq!(event(r#"{"event":"challenge","qr_url":"https://s.team/q/1/2"}"#), Some(Event::Challenge { qr_url: "https://s.team/q/1/2".into() }));
        assert_eq!(event(r#"{"event":"challenge","qr_url":"javascript:alert(1)"}"#), None, "only a web address is ever shown as a code");
        assert_eq!(event(r#"{"event":"signed_in","steam_id":"7656","account_name":"someone"}"#), Some(Event::SignedIn { account: "someone".into() }));
        assert_eq!(event(r#"{"event":"progress","app_id":438100,"phase":"voile","bytes_done":null,"bytes_total":null,"fraction":0.25}"#), Some(Event::Progress { phase: Phase::Engine, fraction: Some(0.25) }));
        assert_eq!(event(r#"{"event":"progress","app_id":438100,"phase":"downloading","bytes_done":5,"bytes_total":0,"fraction":null}"#), Some(Event::Progress { phase: Phase::Game, fraction: None }));
        assert_eq!(event(r#"{"event":"progress","app_id":438100,"phase":"verifying","fraction":7.0}"#), Some(Event::Progress { phase: Phase::Checking, fraction: Some(1.0) }));
        assert_eq!(event(r#"{"event":"installed","app_id":438100}"#), Some(Event::Installed));
        assert_eq!(event(r#"{"event":"starting","app_id":438100}"#), Some(Event::Starting));
        assert_eq!(event(r#"{"event":"running","app_id":438100,"pid":4242}"#), Some(Event::Running { pid: Some(4242) }));
        assert_eq!(event(r#"{"event":"running","app_id":438100,"pid":null}"#), Some(Event::Running { pid: None }));
        assert_eq!(event(r#"{"event":"exited","app_id":438100,"exit_code":0,"exit_signal":null}"#), Some(Event::Exited { code: Some(0) }));
        assert_eq!(event(r#"{"event":"error","code":"not_signed_in","message":"No Steam account is signed in.","key":null}"#), Some(Event::Error { code: "not_signed_in".into(), message: "No Steam account is signed in.".into() }));
        assert_eq!(event(r#"{"event":"voile_update","version":"v1"}"#), Some(Event::Other));
        assert_eq!(event("warning: something on the wrong stream"), None);
        assert_eq!(event(r#"{"no":"event"}"#), None);
    }

    #[test]
    fn a_mac_s_start_is_given_the_destination_alone() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(game_arguments(args(&["--no-vr", "--process-priority=1", "--fps=90", "vrchat://launch?ref=vrchat.com&id=wrld_1:2"])), args(&["vrchat://launch?ref=vrchat.com&id=wrld_1:2"]));
        assert!(game_arguments(args(&["--no-vr"])).is_empty());
    }
}
