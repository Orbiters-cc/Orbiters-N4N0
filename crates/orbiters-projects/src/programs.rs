//! Where Creator Companion's and ALCOM's programs are on this computer: by what their installers and they themselves left in Windows'
//! registry, by the portable Creator Companion the person pointed at, and by the look of a folder on the way to a path. Nothing is
//! searched for on a disk.
use std::path::{Path, PathBuf};

/// Where a portable Creator Companion keeps its settings and lists, beside its program (its 2.5 beta).
const PORTABLE: &str = "VCCUserDataPortable"; // i18n: not shown (a name of a folder's, a program's or the registry's)

/// Whether `dir` is where a portable Creator Companion keeps its own.
pub(crate) fn is_portable_data(dir: &Path) -> bool {
    let named = |p: &Path| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PORTABLE));
    // (A short name of Windows' says nothing: what it stands for does.)
    named(dir) || dir.file_name().is_some_and(|n| n.to_string_lossy().contains('~')) && dir.canonicalize().is_ok_and(|real| named(&real))
}

/// Whether a portable Creator Companion's own folder is directly in `dir`.
pub(crate) fn holds_portable_data(dir: &Path) -> bool {
    dir.join(PORTABLE).is_dir()
}

/// Whether `dir` is a portable Creator Companion: its program beside its own folder, and nothing of the person's there.
pub(crate) fn is_portable(dir: &Path) -> bool {
    holds_portable_data(dir) && has_companion(dir) && !persons(dir)
}

fn is_companion(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("creatorcompanion") && name.ends_with(".exe")
}

fn has_companion(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|all| all.flatten().any(|e| is_companion(&e.file_name().to_string_lossy())))
}

/// Whether a program there is in a folder of the person's, which is not the program's: a disk, one of the person's own folders, a
/// project, or where there are projects (two folders down, the program's own templates left out).
fn persons(dir: &Path) -> bool {
    fn projects(dir: &Path, deeper: bool) -> bool {
        let Ok(all) = std::fs::read_dir(dir) else { return false };
        for (n, e) in all.flatten().enumerate() {
            // More than a program's folder has.
            if n > 2000 {
                return true;
            }
            let name = e.file_name();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || name.eq_ignore_ascii_case(PORTABLE) || name.eq_ignore_ascii_case("Templates") { // i18n: not shown (a name of a folder's, a program's or the registry's)
                continue;
            }
            if is_project(&e.path()) || deeper && projects(&e.path(), false) {
                return true;
            }
        }
        false
    }
    fn is_project(dir: &Path) -> bool {
        dir.join("ProjectSettings/ProjectVersion.txt").is_file()
    }
    let Ok(real) = dir.canonicalize() else { return false };
    if real.parent().is_none() {
        return true;
    }
    let usual = [
        dirs::home_dir(),
        dirs::desktop_dir(),
        dirs::document_dir(),
        dirs::download_dir(),
        dirs::data_local_dir(),
        dirs::data_dir(),
        dirs::config_dir(),
        dirs::picture_dir(),
        dirs::video_dir(),
        dirs::audio_dir(),
        dirs::public_dir(),
        Some(std::env::temp_dir()),
    ];
    usual.into_iter().flatten().any(|u| u.canonicalize().is_ok_and(|u| u == real)) || is_project(&real) || projects(&real, true)
}

/// The folders of both programs that are theirs alone.
pub(crate) fn folders() -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = crate::vcc_app_dir().into_iter().collect();
    let mut others = vec![];
    if let Some(roots) = orbiters_core::paths::roots() {
        others.push(roots.local_data.join("ALCOM-app")); // i18n: not shown (a name of a folder's, a program's or the registry's)
    } else {
        others.extend(installed());
        others.extend(commands().iter().filter_map(|c| of_command(c)));
        if cfg!(target_os = "macos") {
            others.extend([Some(PathBuf::from("/Applications")), dirs::home_dir().map(|h| h.join("Applications"))].into_iter().flatten().map(|d| d.join("ALCOM.app"))); // i18n: not shown (a name of a folder's, a program's or the registry's)
        }
    }
    // The one the person pointed at, for a trial.
    others.extend(crate::unity6::vcc::chosen(&crate::data_dir()).map(|vcc| vcc.folder));
    for dir in others {
        if dir.is_dir() && !persons(&dir) {
            all.push(dir);
        } else if holds_portable_data(&dir) {
            all.push(dir.join(PORTABLE));
        }
    }
    all
}

/// The folder of the program a link type of theirs is opened with, when it is one: an installed one (its uninstaller beside it; a
/// program alone, downloaded anywhere, has no folder of its own), or a portable one.
fn of_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split_once('"')?.0,
        None => &command[..command.to_ascii_lowercase().find(".exe")? + 4],
    };
    let program = Path::new(program);
    let name = program.file_name()?.to_string_lossy().into_owned();
    if !is_companion(&name) && !name.eq_ignore_ascii_case("ALCOM.exe") { // i18n: not shown (a name of a folder's, a program's or the registry's)
        return None;
    }
    let dir = program.parent()?;
    let uninstaller = |e: &std::fs::DirEntry| {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        name.starts_with("unins") && name.ends_with(".exe")
    };
    let installed = std::fs::read_dir(dir).is_ok_and(|all| all.flatten().any(|e| uninstaller(&e)));
    (installed || holds_portable_data(dir)).then(|| dir.to_path_buf())
}

/// What their installers say: ALCOM's (Inno Setup's entry, the older NSIS one, and where that one says it was last), and Creator
/// Companion's. ALCOM may be installed more than once.
#[cfg(windows)]
fn installed() -> Vec<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    const ALCOM: [&str; 2] = ["{4C3D0631-AE29-4D20-A231-678D9CF8D6DB}_is1", "ALCOM"]; // i18n: not shown (a name of a folder's, a program's or the registry's)
    let mut all: Vec<PathBuf> = crate::uninstall_entries(|key, name| ALCOM.contains(&key) || name == "ALCOM" || name.starts_with("ALCOM version") || crate::is_vcc_entry(key, name)) // i18n: not shown (a name of a folder's, a program's or the registry's)
        .into_iter()
        .filter_map(|(location, icon)| location.or_else(|| Some(icon?.parent()?.to_path_buf())))
        .collect();
    let before = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\anatawa12\vrc-get-gui").and_then(|k| k.get_value::<String, _>(""));
    all.extend(before.ok().map(|s| PathBuf::from(s.trim().trim_matches('"'))).filter(|p| p.is_absolute()));
    all
}

#[cfg(not(windows))]
fn installed() -> Vec<PathBuf> {
    vec![]
}

/// What opens their links and files, as they or their installers set it: an installed one may have been moved since.
#[cfg(windows)]
fn commands() -> Vec<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut all = vec![];
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for class in ["alcom", "vcc", "ALCOM Project Template"] { // i18n: not shown (names of the registry's)
            let command = winreg::RegKey::predef(hive).open_subkey(format!(r"Software\Classes\{class}\shell\open\command")).and_then(|k| k.get_value::<String, _>(""));
            all.extend(command.ok());
        }
    }
    all
}

#[cfg(not(windows))]
fn commands() -> Vec<String> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_known_by_what_opens_its_links() {
        let temp = tempfile::tempdir().unwrap();
        let dir = |name: &str, files: &[&str]| {
            let dir = temp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            files.iter().for_each(|f| std::fs::write(dir.join(f), "stand-in").unwrap());
            dir
        };
        let installed = dir("My ALCOM", &["ALCOM.exe", "unins000.exe"]);
        let older = dir("Older", &["ALCOM.exe", "uninstall.exe"]);
        let alone = dir("Downloaded", &["ALCOM.exe"]);
        let portable = dir("Portable", &["CreatorCompanionBeta.exe"]);
        std::fs::create_dir_all(portable.join(PORTABLE)).unwrap();
        let other = dir("Other", &["Other.exe", "unins000.exe"]);
        let quoted = |d: &Path, exe: &str| format!("\"{}\" link \"%1\"", d.join(exe).display());
        assert_eq!(of_command(&quoted(&installed, "ALCOM.exe")).as_ref(), Some(&installed));
        assert_eq!(of_command(&format!("{} %1", older.join("ALCOM.exe").display())).as_ref(), Some(&older));
        assert_eq!(of_command(&quoted(&alone, "ALCOM.exe")), None, "a program alone has no folder of its own");
        assert_eq!(of_command(&quoted(&portable, "CreatorCompanionBeta.exe")).as_ref(), Some(&portable));
        assert_eq!(of_command(&quoted(&other, "Other.exe")), None);
        for nothing in ["", "\"", "\"x", "ALCOM", "\"\" %1", ".exe"] {
            assert_eq!(of_command(nothing), None, "{nothing:?}");
        }
    }

    #[test]
    fn a_folder_of_the_persons_is_not_a_programs() {
        let temp = tempfile::tempdir().unwrap();
        let (program, shared) = (temp.path().join("Program"), temp.path().join("Shared"));
        for dir in [&program, &shared] {
            std::fs::create_dir_all(dir.join(PORTABLE)).unwrap();
            std::fs::write(dir.join("CreatorCompanionBeta.exe"), "stand-in").unwrap();
            std::fs::create_dir_all(dir.join("Templates/Avatar/ProjectSettings")).unwrap();
            std::fs::write(dir.join("Templates/Avatar/ProjectSettings/ProjectVersion.txt"), "").unwrap();
        }
        assert!(is_portable(&program), "its templates are its own");
        assert!(is_portable(&shared));
        std::fs::create_dir_all(shared.join("Projects/Mine/ProjectSettings")).unwrap();
        std::fs::write(shared.join("Projects/Mine/ProjectSettings/ProjectVersion.txt"), "").unwrap();
        assert!(!is_portable(&shared) && is_portable_data(&shared.join(PORTABLE)) && holds_portable_data(&shared));
        assert!(persons(&std::env::temp_dir()) && persons(&temp.path().ancestors().last().unwrap().to_path_buf()));
        assert!(!is_portable(&shared.join("Projects")) && !is_portable_data(&shared.join("Projects")));
    }
}
