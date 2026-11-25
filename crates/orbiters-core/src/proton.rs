//! Where VRChat is on Linux: the Steam that has it (native, Debian's, Flatpak's, Snap's), the library it is in, its Proton
//! prefix, and what that prefix says: which folder of this computer a Windows path is, and the values VRChat keeps in its
//! registry (a text file there, only ever read).
//!
//! The prefix is in the library that holds VRChat (`<library>/steamapps/compatdata/438100/pfx`), not always in Steam's own
//! folder.
use crate::vrchat_launch::STEAM_APP;
use std::path::{Component, Path, PathBuf};

/// How a Steam was installed: it says where its folders are, and how it is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The distribution's, or Valve's own.
    Native,
    /// `com.valvesoftware.Steam`: its home is its own folder; SteamVR is not supported in it.
    Flatpak,
    Snap,
}

/// A Steam on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Steam {
    pub root: PathBuf,
    pub kind: Kind,
}

/// VRChat as a Steam has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vrchat {
    pub steam: Steam,
    /// The Steam library it is in.
    pub library: PathBuf,
    /// Its folder there; its launcher is in it when it is installed.
    pub game: PathBuf,
    /// Its Proton prefix, there once it was started.
    pub prefix: Prefix,
}

impl Vrchat {
    pub fn installed(&self) -> bool {
        has_vrchat(&self.game)
    }
}

/// A Proton prefix (`…/compatdata/438100/pfx`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefix(pub PathBuf);

/// The Steams of the person whose home is `home` (`data`: their `XDG_DATA_HOME`, when they set one), the native ones first, each
/// once (several of these names lead to the same folder).
pub fn steams_in(home: &Path, data: Option<&Path>) -> Vec<Steam> {
    let native = [
        data.map(|d| d.join("Steam")),
        Some(home.join(".local/share/Steam")),
        Some(home.join(".steam/steam")),
        Some(home.join(".steam/root")),
        Some(home.join(".steam/debian-installation")),
    ];
    let candidates = native
        .into_iter()
        .flatten()
        .map(|root| (root, Kind::Native))
        .chain([".local/share/Steam", "data/Steam", ".steam/steam"].map(|r| (home.join(FLATPAK_HOME).join(r), Kind::Flatpak)))
        .chain([(home.join(SNAP_HOME).join(".local/share/Steam"), Kind::Snap)]);
    let mut found: Vec<(PathBuf, Steam)> = Vec::new();
    for (root, kind) in candidates {
        if !root.join("steamapps").is_dir() && !root.join("config").is_dir() {
            continue;
        }
        let real = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if !found.iter().any(|(r, _)| *r == real) {
            found.push((real, Steam { root, kind }));
        }
    }
    found.into_iter().map(|(_, steam)| steam).collect()
}

const FLATPAK_HOME: &str = ".var/app/com.valvesoftware.Steam";
const SNAP_HOME: &str = "snap/steam/common";

/// The libraries of `steam` that are there, its own folder first.
fn libraries(steam: &Steam, home: &Path) -> Vec<PathBuf> {
    let listed = ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"].iter().find_map(|f| std::fs::read_to_string(steam.root.join(f)).ok());
    let mut all = vec![steam.root.clone()];
    for path in listed.iter().flat_map(|vdf| crate::vrchat_launch::libraries(vdf)) {
        // A sandboxed Steam names its home's folders as it sees them: they are in the folder that is its home here.
        let inner = match steam.kind {
            Kind::Native => None,
            Kind::Flatpak => path.strip_prefix(home).ok().map(|rest| home.join(FLATPAK_HOME).join(rest)),
            Kind::Snap => path.strip_prefix(home).ok().map(|rest| home.join(SNAP_HOME).join(rest)),
        };
        let path = [inner, Some(path)].into_iter().flatten().find(|p| p.join("steamapps").is_dir());
        if let Some(path) = path {
            if !all.contains(&path) {
                all.push(path);
            }
        }
    }
    all
}

fn has_vrchat(game: &Path) -> bool {
    ["launch.exe", "VRChat.exe"].iter().any(|f| game.join(f).is_file())
}

/// VRChat on the computer of the person whose home is `home`: the installed one, else the one whose prefix is still there (its
/// logs and pictures are the person's all the same). Of several, the last one played.
pub fn vrchat_in(home: &Path, data: Option<&Path>) -> Option<Vrchat> {
    let mut found: Vec<Vrchat> = Vec::new();
    for steam in steams_in(home, data) {
        for library in libraries(&steam, home) {
            let apps = library.join("steamapps");
            let game = apps.join("common").join("VRChat");
            let prefix = Prefix(apps.join("compatdata").join(STEAM_APP).join("pfx"));
            if has_vrchat(&game) || prefix.0.is_dir() {
                found.push(Vrchat { steam: steam.clone(), library, game, prefix });
            }
        }
    }
    let played = |v: &Vrchat| std::fs::metadata(v.prefix.log_dir()).and_then(|m| m.modified()).ok();
    // (The first of equals: the native Steam's.)
    found.into_iter().rev().max_by_key(|v| (v.installed(), played(v)))
}

/// VRChat on this computer; `None` too where the app's folders were given (a harness, a test: never the person's own).
#[cfg(target_os = "linux")]
pub fn vrchat() -> Option<Vrchat> {
    if crate::paths::others_folders_in_roots().is_some() {
        return None;
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|d| d.is_absolute());
    vrchat_in(&dirs::home_dir()?, data.as_deref())
}

impl Prefix {
    /// The Windows user's folder: Proton's is always `steamuser`.
    pub fn user_dir(&self) -> PathBuf {
        self.0.join("drive_c").join("users").join("steamuser")
    }

    /// VRChat's own folder: its logs, `config.json`, `OSC`.
    pub fn log_dir(&self) -> PathBuf {
        self.user_dir().join("AppData").join("LocalLow").join("VRChat").join("VRChat")
    }

    /// Where VRChat saves pictures unless told otherwise.
    pub fn pictures_dir(&self) -> PathBuf {
        self.user_dir().join("Pictures").join("VRChat")
    }

    /// The path of this computer a Windows path (`C:\Users\…`, `Z:\home\…`) is inside the prefix: its drives are links in
    /// `dosdevices`. Letter case aside, as Windows has it, where a part is there under another. `None` for what is no drive's
    /// path, or leads up.
    pub fn unix_path(&self, windows: &str) -> Option<PathBuf> {
        let mut chars = windows.trim().chars();
        let drive = chars.next().filter(char::is_ascii_alphabetic)?.to_ascii_lowercase();
        (chars.next() == Some(':')).then_some(())?;
        let rest = chars.as_str();
        // (`C:folder` is relative to a folder nobody knows here.)
        rest.starts_with(['\\', '/']).then_some(())?;
        let device = self.0.join("dosdevices").join(format!("{drive}:"));
        let root = match drive {
            _ if cfg!(unix) && device.exists() => std::fs::canonicalize(&device).unwrap_or(device),
            'c' => self.0.join("drive_c"),
            'z' if cfg!(unix) => PathBuf::from("/"),
            _ => return None,
        };
        let mut path = root;
        for part in rest.split(['\\', '/']).filter(|p| !p.is_empty()) {
            let mut components = Path::new(part).components();
            match (components.next(), components.next()) {
                (Some(Component::Normal(_)), None) => path = child(&path, part),
                (Some(Component::CurDir), None) => {}
                _ => return None,
            }
        }
        Some(path)
    }

    /// What VRChat keeps in its registry ([`Registry`]); empty when the file cannot be read.
    pub fn vrchat_registry(&self) -> Registry {
        std::fs::read(self.0.join("user.reg")).map(|text| Registry::read(&text, r"Software\VRChat\VRChat")).unwrap_or_default()
    }
}

/// `part` inside `folder`: as written when it is there (or nothing like it is), else the entry that differs only in letter case.
fn child(folder: &Path, part: &str) -> PathBuf {
    let exact = folder.join(part);
    if std::fs::symlink_metadata(&exact).is_ok() {
        return exact;
    }
    let other = std::fs::read_dir(folder).into_iter().flatten().flatten().find(|e| {
        let name = e.file_name();
        name.to_string_lossy().to_lowercase() == part.to_lowercase()
    });
    other.map_or(exact, |e| e.path())
}

/// The values of one key of a Wine registry file (`user.reg`), as they were last written: wineserver writes the file from time
/// to time, so a value changed in the game may come later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry(std::collections::BTreeMap<String, Value>);

/// A value: its Windows type and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;

impl Registry {
    /// The values of `key` (letter case aside) in the registry file `text`.
    pub fn read(text: &[u8], key: &str) -> Self {
        let text = String::from_utf8_lossy(text);
        let mut values = std::collections::BTreeMap::new();
        let mut inside = false;
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let line = line.trim_end_matches('\r');
            if let Some(rest) = line.strip_prefix('[') {
                // `[Software\\VRChat\\VRChat] 1700000000`
                inside = rest.rfind(']').is_some_and(|end| unescaped(&rest[..end]).to_lowercase() == key.to_lowercase());
                continue;
            }
            if !inside || !line.starts_with('"') {
                continue;
            }
            let mut whole = line.to_string();
            // A long hexadecimal value goes on over lines ended by a backslash.
            while whole.ends_with('\\') {
                whole.pop();
                match lines.next() {
                    Some(next) => whole.push_str(next.trim()),
                    None => break,
                }
            }
            if let Some((name, value)) = value_of(&whole) {
                values.insert(name, value);
            }
        }
        Self(values)
    }

    /// The value `name` (letter case counts: Unity writes its own).
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    pub fn values(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(n, v)| (n.as_str(), v))
    }

    pub fn u32(&self, name: &str) -> Option<u32> {
        self.get(name)?.u32()
    }

    /// A number Unity keeps as a double's bits (its float preferences).
    pub fn f64(&self, name: &str) -> Option<f64> {
        Some(f64::from_le_bytes(self.get(name)?.bytes.as_slice().try_into().ok()?))
    }
}

impl Value {
    pub fn u32(&self) -> Option<u32> {
        (self.kind == REG_DWORD).then(|| self.bytes.as_slice().try_into().ok().map(u32::from_le_bytes))?
    }
}

/// `"name"=value` of a registry file.
fn value_of(line: &str) -> Option<(String, Value)> {
    let (name, rest) = quoted(line)?;
    let value = rest.strip_prefix('=')?;
    let hex = |text: &str| text.split(',').map(str::trim).filter(|b| !b.is_empty()).map(|b| u8::from_str_radix(b, 16).ok()).collect::<Option<Vec<u8>>>();
    let value = if let Some(dword) = value.strip_prefix("dword:") {
        Value { kind: REG_DWORD, bytes: u32::from_str_radix(dword.trim(), 16).ok()?.to_le_bytes().to_vec() }
    } else if let Some(bytes) = value.strip_prefix("hex:") {
        Value { kind: 3, bytes: hex(bytes)? }
    } else if let Some((kind, bytes)) = value.strip_prefix("hex(").and_then(|v| v.split_once("):")) {
        Value { kind: u32::from_str_radix(kind, 16).ok()?, bytes: hex(bytes)? }
    } else if value.starts_with('"') {
        Value { kind: REG_SZ, bytes: quoted(value)?.0.into_bytes() }
    } else {
        let (kind, text) = value.strip_prefix("str(")?.split_once("):")?;
        Value { kind: u32::from_str_radix(kind, 16).ok()?, bytes: quoted(text)?.0.into_bytes() }
    };
    Some((name, value))
}

/// The text between the quotes `line` starts with, and what comes after them.
fn quoted(line: &str) -> Option<(String, &str)> {
    let inner = line.strip_prefix('"')?;
    let mut escaped = false;
    let end = inner.char_indices().find_map(|(i, c)| match c {
        _ if std::mem::take(&mut escaped) => None,
        '\\' => {
            escaped = true;
            None
        }
        '"' => Some(i),
        _ => None,
    })?;
    Some((unescaped(&inner[..end]), &inner[end + 1..]))
}

/// A registry file's text: `\\`, `\"`, `\n` …, and `\x` with up to four hexadecimal digits for a UTF-16 unit.
fn unescaped(text: &str) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut buffer = [0u16; 2];
    while let Some(c) = chars.next() {
        if c != '\\' {
            units.extend_from_slice(c.encode_utf16(&mut buffer));
            continue;
        }
        match chars.next() {
            Some('x') => {
                let mut value = 0u32;
                for _ in 0..4 {
                    let Some(digit) = chars.peek().and_then(|c| c.to_digit(16)) else { break };
                    value = value * 16 + digit;
                    chars.next();
                }
                units.push(value as u16);
            }
            Some('n') => units.push(b'\n' as u16),
            Some('r') => units.push(b'\r' as u16),
            Some('t') => units.push(b'\t' as u16),
            Some('0') => units.push(0),
            Some(other) => units.extend_from_slice(other.encode_utf16(&mut buffer)),
            None => {}
        }
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Home {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("orbiters-steam-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        /// A Steam at `root` (under home) whose libraries are `libraries`; those with VRChat installed are marked.
        fn steam(&self, root: &str, libraries: &[(PathBuf, bool)]) -> PathBuf {
            let steam = self.0.join(root);
            std::fs::create_dir_all(steam.join("steamapps")).unwrap();
            let mut vdf = String::from("\"libraryfolders\"\n{\n");
            for (i, (path, has)) in libraries.iter().enumerate() {
                let app = if *has { "\"438100\" \"1\"" } else { "\"228980\" \"1\"" };
                let path = path.to_string_lossy().replace('\\', "\\\\");
                vdf += &format!("\"{i}\"\n{{\n\"path\" \"{path}\"\n\"apps\"\n{{\n{app}\n}}\n}}\n");
            }
            std::fs::write(steam.join("steamapps").join("libraryfolders.vdf"), vdf + "}\n").unwrap();
            steam
        }

        fn install(library: &Path) {
            let game = library.join("steamapps/common/VRChat");
            std::fs::create_dir_all(&game).unwrap();
            std::fs::write(game.join("launch.exe"), "x").unwrap();
        }

        fn prefix(library: &Path) -> Prefix {
            let prefix = Prefix(library.join("steamapps/compatdata/438100/pfx"));
            std::fs::create_dir_all(prefix.log_dir()).unwrap();
            prefix
        }
    }

    #[test]
    fn a_computer_without_steam_has_no_vrchat() {
        let home = Home::new();
        assert!(steams_in(&home.0, None).is_empty());
        assert_eq!(vrchat_in(&home.0, None), None);
    }

    /// The prefix is in the library that holds the game, on another disk here.
    #[test]
    fn vrchat_is_found_in_another_library_with_its_prefix() {
        let home = Home::new();
        let games = home.0.join("mnt/games");
        std::fs::create_dir_all(games.join("steamapps")).unwrap();
        let steam = home.steam(".local/share/Steam", &[(home.0.join(".local/share/Steam"), false), (games.clone(), true)]);
        Home::install(&games);
        let prefix = Home::prefix(&games);
        let v = vrchat_in(&home.0, None).unwrap();
        assert_eq!(v.steam, Steam { root: steam, kind: Kind::Native });
        assert_eq!((v.library.clone(), v.prefix.clone()), (games, prefix));
        assert!(v.installed());
    }

    /// Installed wins over a prefix an earlier install left; without any installed, the prefix still says where the logs are.
    #[test]
    fn an_installed_vrchat_wins_over_what_an_earlier_one_left() {
        let home = Home::new();
        let old = home.0.join("mnt/old");
        std::fs::create_dir_all(old.join("steamapps")).unwrap();
        let root = home.0.join(".local/share/Steam");
        home.steam(".local/share/Steam", &[(root.clone(), true), (old.clone(), false)]);
        let left = Home::prefix(&old);
        let v = vrchat_in(&home.0, None).unwrap();
        assert!(!v.installed());
        assert_eq!(v.prefix, left);
        Home::install(&root);
        let v = vrchat_in(&home.0, None).unwrap();
        assert!(v.installed());
        assert_eq!(v.library, root);
    }

    #[test]
    fn flatpaks_home_is_its_own_folder() {
        let home = Home::new();
        let inner = home.0.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        // As Flatpak's Steam names them: from inside its sandbox.
        let seen = home.0.join(".local/share/Steam");
        let seen_games = home.0.join("Games");
        home.steam(".var/app/com.valvesoftware.Steam/.local/share/Steam", &[(seen, false), (seen_games, true)]);
        let games = home.0.join(".var/app/com.valvesoftware.Steam/Games");
        Home::install(&games);
        let v = vrchat_in(&home.0, None).unwrap();
        assert_eq!(v.steam, Steam { root: inner, kind: Kind::Flatpak });
        assert_eq!(v.library, games);
    }

    /// `~/.steam/steam` and `~/.steam/root` are links to the one folder.
    #[cfg(unix)]
    #[test]
    fn the_links_to_one_steam_are_one_steam() {
        let home = Home::new();
        let root = home.0.join(".local/share/Steam");
        home.steam(".local/share/Steam", &[(root.clone(), true)]);
        std::fs::create_dir_all(home.0.join(".steam")).unwrap();
        std::os::unix::fs::symlink(&root, home.0.join(".steam/steam")).unwrap();
        std::os::unix::fs::symlink(&root, home.0.join(".steam/root")).unwrap();
        assert_eq!(steams_in(&home.0, Some(&home.0.join(".local/share"))), [Steam { root, kind: Kind::Native }]);
        // Debian's own name alone, with a data folder elsewhere.
        let home = Home::new();
        let root = home.steam(".steam/debian-installation", &[]);
        assert_eq!(steams_in(&home.0, None), [Steam { root, kind: Kind::Native }]);
    }

    #[test]
    fn a_windows_path_is_a_folder_of_this_computer() {
        let home = Home::new();
        let prefix = Home::prefix(&home.0);
        let pictures = prefix.0.join("drive_c/users/steamuser/Pictures");
        std::fs::create_dir_all(&pictures).unwrap();
        assert_eq!(prefix.unix_path(r"C:\users\steamuser\Pictures"), Some(pictures.clone()));
        // Windows' letter case (a disk that has it too answers with the case asked), what is not there yet, and slashes of either kind.
        let other = prefix.unix_path(r"c:\Users\SteamUser\Pictures\VRChat\New").unwrap();
        let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
        assert_eq!(lower(&other), lower(&pictures.join("VRChat").join("New")));
        if cfg!(unix) {
            assert_eq!(other, pictures.join("VRChat").join("New"));
        }
        assert_eq!(prefix.unix_path("C:/users/steamuser//Pictures/."), Some(pictures));
        for none in ["", "C:", "C:relative", r"\\server\share", "/home/me", r"C:\users\..\..\x", r"Q:\nothing"] {
            assert_eq!(prefix.unix_path(none), None, "{none}");
        }
        #[cfg(unix)]
        assert_eq!(prefix.unix_path(r"Z:\home\me\My pictures"), Some(PathBuf::from("/home/me/My pictures")));
    }

    #[cfg(unix)]
    #[test]
    fn drives_are_the_prefix_s_links() {
        let home = Home::new();
        let prefix = Home::prefix(&home.0);
        let disk = home.0.join("disk");
        std::fs::create_dir_all(disk.join("Photos")).unwrap();
        std::fs::create_dir_all(prefix.0.join("dosdevices")).unwrap();
        std::os::unix::fs::symlink("../drive_c", prefix.0.join("dosdevices/c:")).unwrap();
        std::os::unix::fs::symlink(&disk, prefix.0.join("dosdevices/e:")).unwrap();
        let disk = std::fs::canonicalize(disk).unwrap();
        assert_eq!(prefix.unix_path(r"E:\photos"), Some(disk.join("Photos")));
        assert_eq!(prefix.unix_path(r"C:\users"), Some(std::fs::canonicalize(prefix.0.join("drive_c")).unwrap().join("users")));
    }

    const USER_REG: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n[Software\\\\VRChat\\\\Other] 1700000000\n\"VRC_INPUT_COMFORT_TURNING_h2182102264\"=dword:00000007\n\n[Software\\\\vrchat\\\\VRChat] 1700000001 5\n#time=1da17c0f3c3a3f0\n\"PlayerHeight_h2758831273\"=hex(4):00,00,00,00,33,33,fb,3f\n\"VRC_INPUT_COMFORT_TURNING_h2182102264\"=dword:00000001\n\"name \\\"quoted\\\"\"=\"caf\\xe9 \\x30c6\"\n\"blob\"=hex:01,02,\\\n  03,ff\n\"wide\"=str(2):\"%USERPROFILE%\"\n\"odd\"=hex:zz\n@=\"default\"\n\n[Software\\\\VRChat\\\\VRChat\\\\Sub] 1\n\"blob\"=hex:00\n";

    #[test]
    fn the_registry_s_file_says_what_vrchat_keeps() {
        let r = Registry::read(USER_REG.as_bytes(), r"Software\VRChat\VRChat");
        assert_eq!(r.u32("VRC_INPUT_COMFORT_TURNING_h2182102264"), Some(1));
        assert!((r.f64("PlayerHeight_h2758831273").unwrap() - 1.7).abs() < 0.02, "{:?}", r.f64("PlayerHeight_h2758831273"));
        assert_eq!(r.get("name \"quoted\"").unwrap().bytes, "café テ".as_bytes());
        assert_eq!(r.get("blob").unwrap(), &Value { kind: 3, bytes: vec![1, 2, 3, 255] });
        assert_eq!(r.get("wide").unwrap().kind, 2);
        assert_eq!((r.get("odd"), r.get("none"), r.u32("blob")), (None, None, None));
        assert_eq!(r.values().count(), 5);
        // Windows' line ends, and a file cut in the middle of a value.
        assert_eq!(Registry::read(USER_REG.replace('\n', "\r\n").as_bytes(), r"Software\VRChat\VRChat").values().count(), 5);
        let cut = Registry::read(&USER_REG.as_bytes()[..USER_REG.find("03,ff").unwrap()], r"software\vrchat\vrchat");
        assert_eq!(cut.get("blob").unwrap().bytes, [1, 2]);
        assert_eq!(Registry::read(b"", "x"), Registry::default());
    }
}
