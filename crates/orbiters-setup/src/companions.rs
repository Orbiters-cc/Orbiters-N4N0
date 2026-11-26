//! Where Creator Companion and ALCOM keep their own (their settings and lists, their programs): nothing of Orbiters is put
//! there, whatever folder was chosen. The app's own writes are kept out by `orbiters-projects` (`disk.rs`, `programs.rs`),
//! which is far too much for a setup to carry; this is the part a folder to install in needs.

use std::path::{Path, PathBuf};

/// Where a portable Creator Companion keeps its settings and lists, beside its program.
const PORTABLE: &str = "VCCUserDataPortable"; // i18n: not shown (a name of a folder's, a program's or the registry's)

/// The folders that are theirs on this computer, as far as their installers and they themselves say.
pub fn folders() -> Vec<PathBuf> {
    let mut all = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        all.push(local.join("VRChatCreatorCompanion")); // i18n: not shown (a name of a folder's, a program's or the registry's)
    }
    let settings = [dirs::data_local_dir(), dirs::config_dir(), dirs::data_dir()];
    all.extend(settings.into_iter().flatten().map(|d| d.join("com.anatawa12.vrc-get-gui")));
    all.extend(installed().into_iter().filter(|dir| !persons(dir)));
    all
}

/// What their installers say: Creator Companion's and ALCOM's entries among the installed programs.
fn installed() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const UNINSTALL: [&str; 2] = [r"Software\Microsoft\Windows\CurrentVersion\Uninstall", r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"];
    const KEYS: [&str; 3] = ["{A20FE4C3-FE52-495B-B0DA-92992240BFC0}_is1", "{4C3D0631-AE29-4D20-A231-678D9CF8D6DB}_is1", "ALCOM"]; // i18n: not shown (a name of a folder's, a program's or the registry's)
    let theirs = |key: &str, name: &str| KEYS.contains(&key) || name == "ALCOM" || name.starts_with("ALCOM version") || name.starts_with("VRChat Creator Companion"); // i18n: not shown (a name of a folder's, a program's or the registry's)
    let mut all = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for path in UNINSTALL {
            let Ok(root) = winreg::RegKey::predef(hive).open_subkey(path) else { continue };
            for key in root.enum_keys().flatten() {
                let Ok(entry) = root.open_subkey(&key) else { continue };
                let value = |v: &str| entry.get_value::<String, _>(v).ok().map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
                if !theirs(&key, &value("DisplayName").unwrap_or_default()) {
                    continue;
                }
                // DisplayIcon is "program,index".
                let icon = || value("DisplayIcon").and_then(|s| Path::new(s.rsplit_once(',').map_or(s.as_str(), |(p, _)| p)).parent().map(Path::to_path_buf));
                all.extend(value("InstallLocation").map(PathBuf::from).or_else(icon).filter(|p| p.is_absolute()));
            }
        }
    }
    all
}

/// Whether `dir` is a disk or one of the person's own folders: a program put right in one has not made it its own.
fn persons(dir: &Path) -> bool {
    let Ok(real) = dir.canonicalize() else { return false };
    let usual = [
        dirs::home_dir(),
        dirs::desktop_dir(),
        dirs::document_dir(),
        dirs::download_dir(),
        dirs::data_local_dir(),
        dirs::data_dir(),
        dirs::public_dir(),
        Some(std::env::temp_dir()),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        dirs::data_local_dir().map(|d| d.join("Programs")),
    ];
    real.parent().is_none() || usual.into_iter().flatten().any(|u| u.canonicalize().is_ok_and(|u| u == real))
}

/// Whether `dir` is where a portable Creator Companion keeps its own, or its program's folder.
fn portable(dir: &Path) -> bool {
    if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PORTABLE)) {
        return true;
    }
    let program = || {
        std::fs::read_dir(dir).is_ok_and(|all| {
            all.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                name.starts_with("creatorcompanion") && name.ends_with(".exe")
            })
        })
    };
    dir.join(PORTABLE).is_dir() && program() && !persons(dir)
}

/// The folder of theirs that `dir` (there, or to be made) is, or is in: by its name and, through any link and whatever the
/// spelling, by what is really there. Looks only.
pub fn holding(dir: &Path, theirs: &[PathBuf]) -> Option<PathBuf> {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let key = |p: &Path| p.to_string_lossy().trim_start_matches(r"\\?\").replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let within = |inner: &Path, outer: &Path| {
        let (inner, outer) = (key(inner), key(outer));
        inner == outer || inner.strip_prefix(&outer).is_some_and(|rest| rest.starts_with('\\'))
    };
    // As it is named, and as the part of it that is there really is.
    let there = dir.ancestors().find_map(|a| a.canonicalize().ok().map(|real| real.join(dir.strip_prefix(a).unwrap_or(Path::new("")))));
    let spelled: Vec<PathBuf> = std::iter::once(dir.clone()).chain(there).collect();
    for folder in theirs {
        let real = folder.canonicalize().ok();
        let named = std::iter::once(folder.clone()).chain(real);
        if named.into_iter().any(|folder| spelled.iter().any(|s| within(s, &folder))) {
            return Some(folder.clone());
        }
    }
    spelled.iter().flat_map(|s| s.ancestors()).find(|a| portable(a)).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-setup-companions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_of_theirs_is_known_however_it_is_spelled() {
        let root = temp("spelled");
        let theirs = root.join("VRChatCreatorCompanion");
        std::fs::create_dir_all(theirs.join("Repos")).unwrap();
        let known = [theirs.clone(), root.join("Not there yet")];
        for inside in ["VRChatCreatorCompanion", "VRChatCreatorCompanion/Repos/Orbiters - N4N0", "vrchatcreatorcompanion\\x", "Other/../VRChatCreatorCompanion/x", "Not there yet/Orbiters - N4N0"] {
            assert!(holding(&root.join(inside), &known).is_some(), "{inside}");
        }
        for outside in ["Orbiters - N4N0", "VRChatCreatorCompanion 2/x", "Other/Orbiters - N4N0", "."] {
            assert_eq!(holding(&root.join(outside), &known), None, "{outside}");
        }
        // Through a link to it.
        let link = root.join("link");
        if std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&theirs).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) {
            assert!(holding(&link.join("Orbiters - N4N0"), &known).is_some());
            std::fs::remove_dir(&link).unwrap();
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_portable_one_is_known_by_its_look() {
        let root = temp("portable");
        let program = root.join("VCC");
        std::fs::create_dir_all(program.join(PORTABLE)).unwrap();
        assert_eq!(holding(&program.join("x"), &[]), None, "its data folder alone does not make the folder a program's");
        assert!(holding(&program.join(PORTABLE).join("x"), &[]).is_some());
        std::fs::write(program.join("CreatorCompanionBeta.exe"), "stand-in").unwrap();
        assert_eq!(holding(&program.join("a").join("b"), &[]), Some(program.clone()));
        assert_eq!(holding(&root.join("Orbiters - N4N0"), &[]), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
