//! Which VRChat profiles are used on this computer, by evidence (`orbiters_core::vrchat_launch::Profiles`): what VRChat itself
//! leaves here. Its command line, which its log lists, names the profile of each start; its data folder and its saved values are
//! looked at for names that mark another profile than the default one. Only names are read there, never a value, and of a log its
//! first lines alone. One log that cannot be read says nothing; a folder or the saved values that cannot be read leave it open
//! which profiles there are.

use orbiters_core::vrchat_launch::Profiles;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What was read before, so that a look reads what is new alone.
#[derive(Default)]
pub(crate) struct Data {
    /// The profiles each log's command line names, once it is whole.
    logs: HashMap<PathBuf, Vec<u32>>,
    saved: Option<((Option<SystemTime>, u64), Profiles)>,
}

/// How far into a log its command line is looked for.
const HEAD: usize = 64 * 1024;

impl Data {
    /// What VRChat's data folder `dir` shows (none there: nothing).
    pub(crate) fn folder(&mut self, dir: Option<&Path>) -> Profiles {
        let mut found = Profiles::default();
        let Some(dir) = dir else { return found };
        let Some(names) = names(dir, &mut found) else { return found };
        self.logs.retain(|log, _| names.iter().any(|name| log.file_name().is_some_and(|of| of.to_string_lossy() == *name)));
        for name in names {
            let path = dir.join(&name);
            if name.starts_with("output_log_") && name.ends_with(".txt") {
                if !self.logs.contains_key(&path) {
                    let said = crate::bridge::sessions::complete_args(&crate::bridge::sessions::read_head(&path, HEAD));
                    let Some(args) = said else { continue };
                    self.logs.insert(path.clone(), args.iter().filter_map(|arg| arg.strip_prefix("--profile=")?.trim().parse().ok()).collect());
                }
                found.add(self.logs.get(&path).into_iter().flatten().copied());
                continue;
            }
            found.add(Profiles::marked(&name));
            // Where each profile would have its own: the cookies it signs in with.
            if matches!(name.to_ascii_lowercase().as_str(), "cookies" | "profiles" | "profile") && path.is_dir() {
                if let Some(inner) = self::names(&path, &mut found) {
                    found.add(inner.iter().filter_map(|name| Profiles::marked(name).or_else(|| name.parse().ok())));
                }
            }
        }
        found
    }

    /// What the names of VRChat's saved values show, in the registry file `file` of the engine's Windows (a Mac's).
    pub(crate) fn registry_file(&mut self, file: &Path) -> Profiles {
        let meta = match std::fs::metadata(file) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Profiles::default(),
            Err(_) => return Profiles { unread: true, ..Profiles::default() },
        };
        let stamp = (meta.modified().ok(), meta.len());
        if let Some((_, found)) = self.saved.as_ref().filter(|(at, _)| *at == stamp) {
            return found.clone();
        }
        let Ok(bytes) = std::fs::read(file) else { return Profiles { unread: true, ..Profiles::default() } };
        let found = saved_values(value_names(&String::from_utf8_lossy(&bytes)));
        self.saved = Some((stamp, found.clone()));
        found
    }
}

/// The names in the folder `dir`; none, and `found` said to be open, when it cannot be read (not there: nothing to read).
fn names(dir: &Path, found: &mut Profiles) -> Option<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            found.unread |= e.kind() != std::io::ErrorKind::NotFound;
            return None;
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Err(_) => found.unread = true,
        }
    }
    Some(names)
}

/// What these names of VRChat's saved values show.
pub(crate) fn saved_values(names: impl IntoIterator<Item = String>) -> Profiles {
    let mut found = Profiles::default();
    found.add(names.into_iter().filter_map(|name| Profiles::marked(&name)));
    found
}

/// The names of VRChat's saved values in the text of a registry file (`[Software\\VRChat\\VRChat]`, then `"name"=…` lines).
fn value_names(text: &str) -> Vec<String> {
    let key = |line: &str| line.starts_with('[').then(|| line.to_ascii_lowercase().starts_with(r"[software\\vrchat\\vrchat]"));
    let mut inside = false;
    let mut names = Vec::new();
    for line in text.lines() {
        match key(line) {
            Some(ours) => inside = ours,
            None if inside => names.extend(line.strip_prefix('"').and_then(|rest| rest.split_once("\"=")).map(|(name, _)| name.to_string())),
            None => {}
        }
    }
    names
}

/// What the names of VRChat's saved values show, in this Windows' registry.
#[cfg(windows)]
pub(crate) fn registry() -> Profiles {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
    let unread = Profiles { unread: true, ..Profiles::default() };
    let mut key = HKEY::default();
    // SAFETY: the name lives for the call; the key opened is closed below.
    let opened = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(r"Software\VRChat\VRChat"), None, KEY_READ, &mut key) };
    if opened == ERROR_FILE_NOT_FOUND {
        return Profiles::default();
    }
    if opened != ERROR_SUCCESS {
        return unread;
    }
    let mut names = Vec::new();
    let mut whole = true;
    // A value's name is at most 16383 characters.
    let mut buffer = vec![0u16; 16384];
    for index in 0.. {
        let mut len = buffer.len() as u32;
        // SAFETY: the buffer and its length are passed; no type and no value is asked for.
        match unsafe { RegEnumValueW(key, index, Some(PWSTR(buffer.as_mut_ptr())), &mut len, None, None, None, None) } {
            ERROR_SUCCESS => names.push(String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())])),
            ERROR_NO_MORE_ITEMS => break,
            ERROR_MORE_DATA => {}
            _ => {
                whole = false;
                break;
            }
        }
    }
    // SAFETY: the key opened above.
    let _ = unsafe { RegCloseKey(key) };
    Profiles { unread: !whole, ..saved_values(names) }
}

#[cfg(not(windows))]
pub(crate) fn registry() -> Profiles {
    Profiles::default()
}

/// What the bridge's journal `file` records: the profiles its sessions ran on. Its side sessions never run on the default one, so
/// that an account one was signed in with leaves open which profile it was.
pub(crate) fn journal(file: &Path) -> Profiles {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) => return Profiles { unread: e.kind() != std::io::ErrorKind::NotFound, ..Profiles::default() },
    };
    let Ok(journal) = serde_json::from_slice::<crate::bridge::journal::Journal>(&bytes) else { return Profiles { unread: true, ..Profiles::default() } };
    let mut found = Profiles { unread: !journal.accounts.is_empty(), ..Profiles::default() };
    found.add(journal.visits.iter().map(|visit| visit.profile).chain(journal.pictures.iter().map(|picture| picture.profile)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-profiles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn log(args: &[&str]) -> String {
        let mut text = format!("2026.11.24 00:00:00 Debug      -  Launching with args: {}\n", args.len());
        for arg in args {
            text.push_str(&format!("2026.11.24 00:00:00 Debug      -  Arg: {arg}\n"));
        }
        text
    }

    /// The default profile's folder as VRChat keeps it (the names measured on a Mac, 2026-11-24) shows one profile; a start on
    /// another one, in a log, or a name that marks one, shows several; a log that cannot be read, nothing; the folder, that it
    /// is not known.
    #[test]
    fn vrchat_s_own_folder_shows_the_profiles_used() {
        let dir = folder();
        for name in ["AvatarParts", "Avatars", "Cache-WindowsPlayer", "HTTPCache-WindowsPlayer", "LocalAvatarData", "LocalPlayerModerations", "OSC", "TextureDiskCache-WindowsPlayer", "Tools", "Unity", "worldconfig", "Cookies"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        for name in ["Library", "Cookies/Library", "config.json"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        std::fs::write(dir.join("output_log_2026-11-24_00-00-00.txt"), log(&["VRChat.exe", "--no-vr"])).unwrap();
        let mut data = Data::default();
        assert!(data.folder(Some(&dir)).single());
        assert!(Data::default().folder(None).single() && Data::default().folder(Some(&dir.join("gone"))).single(), "VRChat never ran");
        // A start on another profile, whoever made it.
        let other = dir.join("output_log_2026-11-24_01-00-00.txt");
        std::fs::write(&other, "2026.11.24 00:00:00 Debug      -  Launching with args: 2\n").unwrap();
        assert!(data.folder(Some(&dir)).single(), "not all said yet");
        std::fs::write(&other, log(&["VRChat.exe", "--profile=3"])).unwrap();
        assert!(!data.folder(Some(&dir)).single() && data.folder(Some(&dir)).seen.contains(&3));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let allow = |path: &Path, mode: u32| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
            allow(&other, 0o000);
            assert!(Data::default().folder(Some(&dir)).single(), "a log that cannot be read is no other profile");
            allow(&other, 0o600);
            std::fs::remove_file(&other).unwrap();
            for unreadable in [dir.clone(), dir.join("Cookies")] {
                allow(&unreadable, 0o000);
                let found = Data::default().folder(Some(&dir));
                allow(&unreadable, 0o700);
                assert!(found.unread && !found.single(), "{}", unreadable.display());
            }
        }
        let _ = std::fs::remove_file(&other);
        assert!(data.folder(Some(&dir)).single());
        for name in ["Cookies/Library1", "Cookies/2", "Library_4", "Cookies-5", "profile6", "Profile_7.dat"] {
            std::fs::write(dir.join(name), "x").unwrap();
            assert!(!data.folder(Some(&dir)).single(), "{name}");
            std::fs::remove_file(dir.join(name)).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_saved_values_and_the_bridge_s_journal_show_the_profiles_used() {
        let dir = folder();
        let file = dir.join("user.reg");
        let text = |name: &str| format!("WINE REGISTRY Version 2\n\n[Software\\\\Other] 1\n\"profile3\"=\"x\"\n\n[Software\\\\VRChat\\\\VRChat] 1795470087\n#time=1\n\"VRC_INPUT_OSC_h1387374534\"=dword:00000001\n\"{name}\"=hex:00\n\n[Volatile] 1\n\"profile2\"=\"x\"\n");
        assert!(Data::default().registry_file(&file).single(), "none there");
        let mut data = Data::default();
        // (A hash is no profile, nor is a word that has one for a part.)
        for (name, single) in [("usr_00000000-0000-4000-8000-000000000001_UI.RecentlyUsedEmojis_h1", true), ("VRC_PROFILE_PICTURE_h22", true), ("VRC_profile_2_token_h22", false), ("profile1_h3", false)] {
            std::fs::write(&file, text(name)).unwrap();
            assert_eq!(data.registry_file(&file).single(), single, "{name}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            assert!(Data::default().registry_file(&file).unread);
        }
        let file = dir.join("bridge-journal.json");
        assert!(journal(&file).single());
        for (said, single) in [(r#"{"visits":[{"world":"wrld_a","profile":0,"at":1}]}"#, true), (r#"{"visits":[{"world":"wrld_a","profile":2,"at":1}]}"#, false), (r#"{"pictures":[{"session":"s","length_s":1,"profile":1,"at":1}]}"#, false), (r#"{"accounts":["usr_side"]}"#, false), ("{", false)] {
            std::fs::write(&file, said).unwrap();
            assert_eq!(journal(&file).single(), single, "{said}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
