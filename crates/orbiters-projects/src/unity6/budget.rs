//! What a Unity 6 trial needs on each disk before anything is copied (B5): the copy itself (measured), the Library Unity
//! 6 builds when it first opens the trial (an estimate, with its uncertainty: Unity gives no rule for it), and the
//! editor with its modules when it is not installed yet (Unity's published sizes). Demands on the same real volume add
//! up (two folders on one disk, two APFS volumes of one container); each volume's total is compared with what is free
//! there now. It is checked again just before the copy; free space is an observation, not a reservation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What one demand is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Need {
    /// The trial's files (the project without its Library and caches), measured.
    Copy,
    /// The Library Unity 6 builds on first open, estimated.
    Import,
    /// Unity 6000 and the modules its 2022 editor has, installed, from Unity's published sizes.
    Editor,
    /// The same, downloaded first (where Unity Hub downloads).
    Download,
    /// Room kept free for Unity's and the system's temporary files.
    Margin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Demand {
    pub need: Need,
    /// Where it lands (the trial's folder, or the editor's install folder).
    pub at: PathBuf,
    /// The most it is expected to take; for an estimate, the high end.
    pub bytes: u64,
    /// For an estimate, the low end.
    pub low: Option<u64>,
}

/// One real volume and what is asked of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    /// A path on it, to name it by.
    pub at: PathBuf,
    pub free: Option<u64>,
    pub needs: Vec<Demand>,
}

impl Volume {
    pub fn total(&self) -> u64 {
        self.needs.iter().map(|d| d.bytes).sum()
    }

    /// Whether it has room: unknown free space is no room.
    pub fn fits(&self) -> bool {
        self.free.is_some_and(|f| f >= self.total())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub volumes: Vec<Volume>,
    /// The original's Library could not be measured (absent, a link elsewhere, or tiny): the import was estimated from the
    /// project's own files instead.
    pub import_guessed: bool,
}

impl Budget {
    pub fn fits(&self) -> bool {
        self.volumes.iter().all(Volume::fits)
    }
}

/// The room kept free beside a trial's own needs.
pub const MARGIN: u64 = 4 * 1024 * 1024 * 1024;
/// A Library below this is taken as not measured (a fresh project's, or one redirected elsewhere).
const LIBRARY_FLOOR: u64 = 64 * 1024 * 1024;

/// The fresh import's estimate: from the original's Library when it was measured (0.8× to 2× it), else from the copy's
/// own size (1× to 3× it).
pub fn import_estimate(copy: u64, library: Option<u64>) -> (u64, u64, bool) {
    match library.filter(|l| *l >= LIBRARY_FLOOR) {
        Some(l) => (l / 10 * 8, l.saturating_mul(2), false),
        None => (copy, copy.saturating_mul(3), true),
    }
}

/// What tells two paths share their room: a key per real volume and the room free there. `None`: not reachable.
pub trait Room {
    fn of(&self, path: &Path) -> Option<(String, u64)>;
}

/// This computer's disks.
pub struct Disks;

impl Room for Disks {
    fn of(&self, path: &Path) -> Option<(String, u64)> {
        let mut dir = path;
        while !dir.exists() {
            dir = dir.parent()?;
        }
        let free = crate::size::free_space(dir)?;
        #[cfg(target_os = "macos")]
        {
            // Two APFS volumes of one container share all its room: one key for the container.
            let v = crate::mac::volume_of(dir)?;
            let key = crate::mac::room_key(&v);
            Some((key, free))
        }
        #[cfg(windows)]
        {
            let _ = free;
            windows_volume(dir)
        }
        #[cfg(all(not(windows), not(target_os = "macos")))]
        {
            use std::os::unix::fs::MetadataExt;
            let dev = std::fs::metadata(dir).ok()?.dev();
            Some((format!("device-{dev}"), free))
        }
    }
}

/// The volume that really holds `dir` (through junctions, links and mounted folders: the folder is resolved first) and
/// the room free on that same volume, so the two always describe one disk (verification's blocker 6).
#[cfg(windows)]
fn windows_volume(dir: &Path) -> Option<(String, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
    let target = std::fs::canonicalize(dir).ok()?;
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut mount = vec![0u16; 1024];
    // SAFETY: valid null-terminated strings and buffers of the given lengths.
    unsafe { GetVolumePathNameW(PCWSTR(target.as_ptr()), &mut mount) }.ok()?;
    let mount = text(&mount);
    // `\\?\C:\` is named `C:\` by the other calls; a `\\?\Volume{…}\` path is the volume's own name already.
    let plain = match mount.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() <= 3 && rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
        _ => mount.clone(),
    };
    let mut name = vec![0u16; 128];
    // A volume this computer cannot name (a network share under any spelling) is not measured: no room is known there.
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(wide(&plain).as_ptr()), &mut name) }.ok()?;
    let key = text(&name);
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide(&plain).as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some((key.to_lowercase(), free))
}

/// Groups `demands` by real volume and puts each volume's free room beside them.
pub fn plan(demands: Vec<Demand>, room: &dyn Room, import_guessed: bool) -> Budget {
    let mut volumes: Vec<(Option<String>, Volume)> = vec![];
    for d in demands {
        let found = room.of(&d.at);
        let key = found.as_ref().map(|(k, _)| k.clone());
        match volumes.iter_mut().find(|(k, _)| k.is_some() && *k == key) {
            Some((_, v)) => {
                v.needs.push(d);
                // Two folders of one disk can have different room (a quota): the lesser of the two counts.
                v.free = match (v.free, found.map(|(_, f)| f)) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    _ => None,
                };
            }
            None => volumes.push((key, Volume { at: d.at.clone(), free: found.map(|(_, f)| f), needs: vec![d] })),
        }
    }
    Budget { volumes: volumes.into_iter().map(|(_, v)| v).collect(), import_guessed }
}

/// What installing the editor takes, when it is missing: its installed bytes where Unity Hub installs, its download
/// where Unity Hub downloads, each with the modules counted in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorNeed {
    pub install_at: PathBuf,
    pub installed: u64,
    pub download_at: PathBuf,
    pub download: u64,
}

impl EditorNeed {
    /// What adding modules to the editor `program` installed here takes, on the disk it is on: `None` when it needs none.
    pub fn modules(program: &Path, size: &super::matrix::EditorSize, android: bool, ios: bool) -> Option<Self> {
        let mut need = Self { install_at: program.parent().unwrap_or(program).to_path_buf(), installed: 0, download_at: std::env::temp_dir(), download: 0 };
        for (wanted, module) in [(android, size.android), (ios, size.ios)] {
            if let (true, Some([d, i])) = (wanted, module) {
                need.download += d;
                need.installed += i;
            }
        }
        (need.installed > 0).then_some(need)
    }

    /// What the editor `size` takes here, seen now: all of it when it is not verified here, else the modules it lacks of
    /// those the project's 2022 editor has (every module, when that editor cannot be told).
    pub fn here(size: &super::matrix::EditorSize, unity6: &super::editors::Found, unity2022: &super::editors::Found) -> Option<Self> {
        let wanted = unity2022.usable().map(|c| c.modules).unwrap_or(super::editors::Modules { android: true, android_tools: true, ios: true });
        match unity6 {
            super::editors::Found::Exact(c) => {
                // Its Android module without the SDK, NDK and JDK is not enough to build: they are counted whole.
                let android = (wanted.android && !c.modules.android) || (wanted.android_tools && !c.modules.android_tools);
                Self::modules(&c.program, size, android, wanted.ios && !c.modules.ios)
            }
            _ => Some(Self::of(size, wanted.android, wanted.ios)),
        }
    }

    /// From a release row's sizes on this computer, with Android (and its SDK, NDK, JDK) and iOS when `android`/`ios`.
    pub fn of(size: &super::matrix::EditorSize, android: bool, ios: bool) -> Self {
        let mut need = Self { install_at: editor_install_folder(), installed: size.installed, download_at: std::env::temp_dir(), download: size.download };
        for (wanted, module) in [(android, size.android), (ios, size.ios)] {
            if let (true, Some([d, i])) = (wanted, module) {
                need.download += d;
                need.installed += i;
            }
        }
        need
    }
}

/// The demands of a trial of a project whose copy takes `copy` bytes (measured) and whose Library took `library`,
/// copied into `trial`; with the editor's install and download (`editor`) when it is missing.
pub fn demands(trial: &Path, copy: u64, library: Option<u64>, editor: Option<EditorNeed>) -> (Vec<Demand>, bool) {
    let (low, high, guessed) = import_estimate(copy, library);
    let mut out = vec![
        Demand { need: Need::Copy, at: trial.to_path_buf(), bytes: copy, low: None },
        Demand { need: Need::Import, at: trial.to_path_buf(), bytes: high, low: Some(low) },
        Demand { need: Need::Margin, at: trial.to_path_buf(), bytes: MARGIN, low: None },
    ];
    if let Some(e) = editor {
        out.push(Demand { need: Need::Editor, at: e.install_at, bytes: e.installed, low: None });
        out.push(Demand { need: Need::Download, at: e.download_at, bytes: e.download, low: None });
    }
    (out, guessed)
}

/// Where Unity Hub installs editors on this computer: its secondary folder when one is set, else its own.
pub fn editor_install_folder() -> PathBuf {
    let hub = crate::hub_data_dir();
    let secondary = hub
        .as_ref()
        .and_then(|h| std::fs::read_to_string(h.join("secondaryInstallPath.json")).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_str().or_else(|| v["secondaryInstallPath"].as_str()).map(str::trim).map(ToOwned::to_owned))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    #[cfg(target_os = "macos")]
    let secondary = secondary.or_else(|| {
        let files = [hub.as_ref().map(|h| h.join("settings.json")), Some(PathBuf::from("/Library/Application Support/Unity/config/services-config.json"))];
        files.into_iter().flatten().find_map(|f| {
            let v: serde_json::Value = serde_json::from_slice(&std::fs::read(f).ok()?).ok()?;
            v["machineWideSecondaryInstallLocation"].as_str().map(PathBuf::from).filter(|p| p.is_absolute())
        })
    });
    secondary.unwrap_or_else(|| if cfg!(target_os = "macos") { PathBuf::from("/Applications/Unity/Hub/Editor") } else { PathBuf::from(r"C:\Program Files\Unity\Hub\Editor") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Disks by the first component of a path: `C` and `H`, with `C` and `D` sharing one volume's room.
    struct Fake(HashMap<&'static str, (&'static str, u64)>);
    impl Room for Fake {
        fn of(&self, path: &Path) -> Option<(String, u64)> {
            let first = path.components().next()?.as_os_str().to_string_lossy().to_string();
            self.0.get(first.as_str()).map(|(k, f)| (k.to_string(), *f))
        }
    }

    const GB: u64 = 1_000_000_000;

    #[test]
    fn demands_on_one_volume_add_up_and_each_volume_is_judged() {
        let room = Fake(HashMap::from([("C", ("one", 30 * GB)), ("D", ("one", 30 * GB)), ("H", ("two", 17 * GB))]));
        // The trial on H, the editor on C: H must hold copy + import + margin.
        let editor = |at: &str| EditorNeed { install_at: PathBuf::from(at), installed: 13 * GB, download_at: PathBuf::from(at), download: 0 };
        let (d, guessed) = demands(Path::new("H/Projects/Trial"), 5 * GB, Some(4 * GB), Some(editor("C/Unity")));
        let b = plan(d, &room, guessed);
        assert_eq!(b.volumes.len(), 2);
        assert!(!guessed);
        let h = b.volumes.iter().find(|v| v.at.starts_with("H")).unwrap();
        assert_eq!(h.total(), 5 * GB + 8 * GB + MARGIN);
        assert!(!h.fits(), "17 GB free on H is less than {}", h.total());
        assert!(!b.fits());
        // On D, which shares C's room with the editor: one volume, the demands summed against its room.
        let (d, _) = demands(Path::new("D/Trial"), 5 * GB, Some(4 * GB), Some(editor("C/Unity")));
        let b = plan(d, &room, false);
        assert_eq!(b.volumes.len(), 1);
        assert_eq!(b.volumes[0].total(), 5 * GB + 8 * GB + MARGIN + 13 * GB);
        // Verification blocker 6: the modules the 2022 editor has are counted, and the download lands where Hub downloads.
        let size = super::super::matrix::EditorSize { download: 4 * GB, installed: 8 * GB, android: Some([2 * GB, 6 * GB]), ios: Some([GB, GB]) };
        let need = EditorNeed::of(&size, true, false);
        assert_eq!((need.download, need.installed), (6 * GB, 14 * GB));
        assert!(!b.volumes[0].fits(), "30 GB free is less than the sum, though each alone fits");
        // Editor installed already: only the trial's needs.
        let (d, _) = demands(Path::new("D/Trial"), 5 * GB, Some(4 * GB), None);
        assert!(plan(d, &room, false).fits());
        // An unreachable destination has no room.
        let (d, _) = demands(Path::new("Z/Trial"), GB, None, None);
        assert!(!plan(d, &room, false).fits());
    }

    #[test]
    fn a_library_that_cannot_be_measured_is_said_to_be_a_guess() {
        assert_eq!(import_estimate(10, Some(1_000_000_000)), (800_000_000, 2_000_000_000, false));
        // Absent, or tiny (redirected elsewhere): from the copy's size, wider.
        assert_eq!(import_estimate(1_000, None), (1_000, 3_000, true));
        assert_eq!(import_estimate(1_000, Some(10)), (1_000, 3_000, true));
    }

    /// Verification blocker 6: a junction is keyed by the volume it leads to, as its free room is. Across two volumes this
    /// needs a folder on another one (`ORBITERS_TEST_OTHER_VOLUME`); on one volume it checks the same rule.
    #[cfg(windows)]
    #[test]
    fn a_junction_is_counted_on_the_volume_it_leads_to() {
        let temp = tempfile::tempdir().unwrap();
        let other = std::env::var_os("ORBITERS_TEST_OTHER_VOLUME").map(PathBuf::from).unwrap_or_else(|| temp.path().join("elsewhere"));
        let target = other.join(format!("u6-junction-{}", std::process::id()));
        std::fs::create_dir_all(&target).unwrap();
        let link = temp.path().join("link");
        junction::create(&target, &link).unwrap();
        let (through, there) = (Disks.of(&link.join("Trial")).unwrap(), Disks.of(&target).unwrap());
        let _ = junction::delete(&link);
        let _ = std::fs::remove_dir(&target);
        assert_eq!(through.0, there.0, "the junction's key is its target's volume");
    }

    #[test]
    fn this_computers_disks_are_keyed_by_volume() {
        let temp = tempfile::tempdir().unwrap();
        let a = Disks.of(temp.path()).unwrap();
        let b = Disks.of(&temp.path().join("not/made/yet")).unwrap();
        assert_eq!(a.0, b.0, "a folder not made yet is on its nearest parent's volume");
    }

    /// Second verification B6: two folders of one disk with different room (a quota) count the lesser; a network spelling of a
    /// disk, or one that cannot be named, has no known room, so nothing fits there.
    #[test]
    fn the_lesser_room_of_one_disk_counts_and_a_network_has_none() {
        struct Quota;
        impl Room for Quota {
            fn of(&self, path: &Path) -> Option<(String, u64)> {
                match path.to_str()? {
                    "a" => Some(("disk".into(), 100)),
                    "b" => Some(("disk".into(), 10)),
                    _ => None,
                }
            }
        }
        let d = |at: &str, bytes| Demand { need: Need::Copy, at: PathBuf::from(at), bytes, low: None };
        let p = plan(vec![d("a", 5), d("b", 4)], &Quota, false);
        assert_eq!((p.volumes.len(), p.volumes[0].free, p.fits()), (1, Some(10), true));
        assert!(!plan(vec![d("a", 5), d("b", 6)], &Quota, false).fits());
        assert!(!plan(vec![d("elsewhere", 1)], &Quota, false).fits());
        #[cfg(windows)]
        {
            let temp = tempfile::tempdir().unwrap();
            let text = std::fs::canonicalize(temp.path()).unwrap().display().to_string();
            let rest = text.trim_start_matches(r"\\?\");
            let unc = PathBuf::from(format!(r"\\localhost\{}$\{}", &rest[..1], &rest[3..]));
            assert!(Disks.of(temp.path()).is_some());
            if unc.exists() {
                assert!(Disks.of(&unc).is_none(), "{}", unc.display());
            }
        }
    }

    /// Second verification B6: an editor that is there and lacks modules the project's 2022 editor has costs those; one that
    /// is not verified costs all of it; a 2022 editor that cannot be told counts every module.
    #[test]
    fn missing_modules_of_an_installed_editor_are_counted() {
        use super::super::editors::{Candidate, Check, Found, Hint, Modules};
        let matrix = super::super::matrix::Matrix::built_in();
        let row = matrix.unity6_target().unwrap();
        let size = row.editor.get("windows-x86_64").unwrap();
        let c = |m: Modules| Candidate { program: "x".into(), hint: Hint::HubFolder, check: Check::Verified, identity: None, modules: m };
        let with = |android, ios| Modules { android, android_tools: android, ios };
        let six = |m| Found::Exact(c(m));
        assert_eq!(EditorNeed::here(size, &six(with(true, true)), &Found::Exact(c(with(true, true)))), None);
        let one = EditorNeed::here(size, &six(with(false, true)), &Found::Exact(c(with(true, true)))).unwrap();
        assert_eq!(one.installed, size.android.unwrap()[1]);
        // Third verification B6: where that editor really is, and its Android tools with it.
        let mut elsewhere = c(with(true, false));
        elsewhere.program = PathBuf::from("E:/Editors/6000/Editor/Unity.exe");
        let at = EditorNeed::here(size, &Found::Exact(elsewhere), &Found::Exact(c(with(true, true)))).unwrap();
        assert!(at.installed == size.ios.unwrap()[1] && at.install_at == Path::new("E:/Editors/6000/Editor"));
        let no_tools = Modules { android: true, android_tools: false, ios: true };
        assert_eq!(EditorNeed::here(size, &six(no_tools), &Found::Exact(c(with(true, true)))).unwrap().installed, size.android.unwrap()[1]);
        assert_eq!(EditorNeed::here(size, &six(no_tools), &Found::Exact(c(no_tools))), None, "its 2022 editor has none either");
        let all = EditorNeed::here(size, &Found::Missing { wrong: vec![] }, &Found::Exact(c(with(false, false)))).unwrap();
        assert_eq!(all.installed, size.installed);
        let unknown = EditorNeed::here(size, &six(with(false, false)), &Found::Missing { wrong: vec![] }).unwrap();
        assert_eq!(unknown.installed, size.android.unwrap()[1] + size.ios.unwrap()[1]);
        let unverified = EditorNeed::here(size, &Found::Unverified(c(with(true, true))), &Found::Exact(c(with(true, false)))).unwrap();
        assert_eq!(unverified.installed, size.installed + size.android.unwrap()[1]);
    }
}
