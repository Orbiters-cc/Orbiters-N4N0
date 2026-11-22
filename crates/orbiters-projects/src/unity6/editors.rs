//! Which editor opens a project: the places Unity Hub, VCC and ALCOM name ([`crate::discovery`] gathers them) are
//! hints, never proof. Each candidate is an absolute path to a program that is there, and the program itself is asked
//! what it is: on Windows its version resource (`ProductVersion` is `6000.0.67f1_78a1c2bbeb6a`) and the processor its
//! header is for; on the Mac its bundle's `Info.plist` and its Mach-O header. One that says another version is never
//! used; one that says nothing is "found, not verified". Nothing is launched to ask, and Unity Hub is never started.

use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use vrc_get_vpm::version::UnityVersion;

/// Where a candidate was named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hint {
    /// Unity Hub's own install folder.
    HubFolder,
    /// Unity Hub's list of editors (`editors-v2.json`).
    HubList,
    /// The install folder chosen in Unity Hub's preferences, or the one for every user of the computer.
    HubSecondary,
    /// Creator Companion's settings.
    VccSettings,
    /// VCC's or ALCOM's database of editors.
    VccDatabase,
    /// Located by the person in Orbiters.
    Located,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arch {
    X86_64,
    Arm64,
    Other,
}

impl Arch {
    /// This computer's processor.
    pub fn here() -> Self {
        if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else {
            Arch::Other
        }
    }
}

/// What a program says it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The Unity version it names (`6000.0.67f1`).
    pub version: String,
    pub changeset: Option<String>,
    /// The processors it is built for (a Mac's universal program, both).
    pub arch: Vec<Arch>,
}

/// What was found out about a candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    /// It says it is this version, for this computer's processor.
    Verified,
    /// It says nothing readable about its version: found where a hint said, not verified.
    Unverified,
    /// It is another version, or for another processor: never used.
    Wrong(String),
}

/// The platform modules an editor has, from its folders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modules {
    /// Android Build Support.
    pub android: bool,
    /// With its SDK, NDK and OpenJDK, which Unity Hub installs beside it.
    pub android_tools: bool,
    /// iOS Build Support.
    pub ios: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub program: PathBuf,
    pub hint: Hint,
    pub check: Check,
    pub identity: Option<Identity>,
    pub modules: Modules,
}

/// What looking for one exact editor found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// A program that says it is exactly this version.
    Exact(Candidate),
    /// A program where a hint said, saying nothing of its version.
    Unverified(Candidate),
    /// None usable; `wrong` lists programs found that are another version or processor (said, never used).
    Missing { wrong: Vec<Candidate> },
}

impl Found {
    /// The program Open may use: exact, or found but not verified (never another version).
    pub fn usable(&self) -> Option<&Candidate> {
        match self {
            Found::Exact(c) | Found::Unverified(c) => Some(c),
            Found::Missing { .. } => None,
        }
    }
}

/// Judges what a program said against the version wanted.
pub fn judge(identity: Option<&Identity>, wanted: &str) -> Check {
    let Some(id) = identity else { return Check::Unverified };
    let same = match (UnityVersion::parse(&id.version), UnityVersion::parse(wanted)) {
        (Some(a), Some(b)) => a == b && a.to_string() == b.to_string(),
        _ => id.version == wanted,
    };
    if !same {
        return Check::Wrong(tr!("it is Unity {found}, not {wanted}", found = id.version, wanted = wanted));
    }
    if !id.arch.is_empty() && !id.arch.contains(&Arch::here()) && Arch::here() != Arch::Other {
        // Apple silicon runs an Intel editor through Rosetta: slower, not refused.
        let rosetta = cfg!(target_os = "macos") && Arch::here() == Arch::Arm64 && id.arch.contains(&Arch::X86_64);
        if !rosetta {
            return Check::Wrong(tr("it is built for another processor than this computer's").into());
        }
    }
    Check::Verified
}

/// Checks `hints` (in order) for `version` with `probe`: the first exact one, else the first that exists unverified,
/// else what was found of other versions. Relative or empty paths and programs that are not there are left out.
pub fn find_with(version: &str, hints: Vec<(PathBuf, Hint)>, probe: &dyn Fn(&Path) -> Option<Identity>) -> Found {
    let mut unverified = None;
    let mut wrong = vec![];
    let mut seen = std::collections::HashSet::new();
    for (program, hint) in hints {
        if !program.is_absolute() || !program.is_file() || !seen.insert(crate::discovery::key(&program)) {
            continue;
        }
        let identity = probe(&program);
        let check = judge(identity.as_ref(), version);
        let candidate = Candidate { modules: modules(&program), program, hint, check: check.clone(), identity };
        match check {
            Check::Verified => return Found::Exact(candidate),
            Check::Unverified => {
                unverified.get_or_insert(candidate);
            }
            Check::Wrong(_) => wrong.push(candidate),
        }
    }
    match unverified {
        Some(c) => Found::Unverified(c),
        None => Found::Missing { wrong },
    }
}

/// [`find_with`] over every place this computer names, with the program's own answer.
pub fn find(version: &str) -> Found {
    let mut hints = located_hints(&crate::data_dir(), version);
    hints.extend(crate::discovery::editor_hints(version));
    let found = find_with(version, hints, &probe);
    if found.usable().is_some() {
        return found;
    }
    // VCC's or ALCOM's database, the slow source (bounded), only when nothing else had it.
    let more: Vec<_> = crate::discovery::database_editors(version).into_iter().map(|p| (p, Hint::VccDatabase)).collect();
    match find_with(version, more, &probe) {
        Found::Missing { wrong } => match found {
            Found::Missing { wrong: mut before } => {
                before.extend(wrong);
                Found::Missing { wrong: before }
            }
            other => other,
        },
        other => other,
    }
}

/// An editor the person located: the program, the version it was located for, and whether it said it is that one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    pub program: PathBuf,
    pub version: String,
    pub verified: bool,
}

const LOCATED_FILE: &str = "unity-editors-located.json";

pub fn located(dir: &Path) -> Vec<Located> {
    std::fs::read(dir.join(LOCATED_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn located_hints(dir: &Path, version: &str) -> Vec<(PathBuf, Hint)> {
    located(dir).into_iter().filter(|l| l.version == version).map(|l| (l.program, Hint::Located)).collect()
}

/// The program the person chose for `version`, kept when it is that version (or says nothing, their choice then):
/// refused when it says it is another one or for another processor.
pub fn locate(dir: &Path, program: &Path, version: &str, probe: &dyn Fn(&Path) -> Option<Identity>) -> anyhow::Result<Candidate> {
    let program = if cfg!(target_os = "macos") && program.extension().is_some_and(|e| e == "app") { program.join("Contents/MacOS/Unity") } else { program.to_path_buf() };
    anyhow::ensure!(program.is_absolute() && program.is_file(), "{}", tr!("{path} is not a program.", path = program.display()));
    let identity = probe(&program);
    let check = judge(identity.as_ref(), version);
    if let Check::Wrong(why) = &check {
        anyhow::bail!("{}", tr!("{path} cannot open this project: {why}.", path = program.display(), why = why));
    }
    let mut list = located(dir);
    list.retain(|l| !(l.version == version && crate::discovery::key(&l.program) == crate::discovery::key(&program)));
    list.push(Located { program: program.clone(), version: version.to_string(), verified: check == Check::Verified });
    crate::write_json(&dir.join(LOCATED_FILE), &list)?;
    Ok(Candidate { modules: modules(&program), program, hint: Hint::Located, check, identity })
}

/// The folder holding an editor's platform modules.
fn playback_engines(program: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        // .../<version>/Unity.app/Contents/MacOS/Unity: the modules are beside Unity.app.
        Some(program.parent()?.parent()?.parent()?.parent()?.join("PlaybackEngines"))
    } else {
        // .../<version>/Editor/Unity.exe
        Some(program.parent()?.join("Data/PlaybackEngines"))
    }
}

pub fn modules(program: &Path) -> Modules {
    let Some(engines) = playback_engines(program) else { return Modules::default() };
    let android = engines.join("AndroidPlayer");
    Modules {
        android: android.is_dir(),
        android_tools: ["SDK", "NDK", "OpenJDK"].iter().all(|d| android.join(d).is_dir()),
        ios: engines.join("iOSSupport").is_dir(),
    }
}

/// A version resource's `ProductVersion` (`6000.0.67f1_78a1c2bbeb6a`) as a version and a changeset.
pub fn parse_product_version(text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim().trim_matches('\0');
    let (version, changeset) = match text.split_once(['_', ' ']) {
        Some((v, c)) => (v, Some(c.trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace()).to_string()).filter(|c| !c.is_empty())),
        None => (text, None),
    };
    UnityVersion::parse(version)?;
    Some((version.to_string(), changeset))
}

/// The processor a Windows program is for, from its PE header.
pub fn pe_arch(head: &[u8]) -> Option<Arch> {
    let at = u32::from_le_bytes(head.get(0x3c..0x40)?.try_into().ok()?) as usize;
    (head.get(at..at + 4)? == b"PE\0\0").then_some(())?;
    Some(match u16::from_le_bytes(head.get(at + 4..at + 6)?.try_into().ok()?) {
        0x8664 => Arch::X86_64,
        0xaa64 => Arch::Arm64,
        _ => Arch::Other,
    })
}

/// The processors a Mac program is for, from its Mach-O header (a universal one lists each).
pub fn macho_arch(head: &[u8]) -> Vec<Arch> {
    let cpu = |t: u32| match t {
        0x0100_000c => Arch::Arm64,
        0x0100_0007 => Arch::X86_64,
        _ => Arch::Other,
    };
    let Some(magic) = head.get(0..4) else { return vec![] };
    match magic {
        // A universal program, big-endian: its count, then 20 bytes per processor.
        [0xca, 0xfe, 0xba, 0xbe] => {
            let n = head.get(4..8).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap())) as usize;
            (0..n.min(8)).filter_map(|i| head.get(8 + i * 20..12 + i * 20)).map(|b| cpu(u32::from_be_bytes(b.try_into().unwrap()))).collect()
        }
        [0xcf, 0xfa, 0xed, 0xfe] => head.get(4..8).map(|b| vec![cpu(u32::from_le_bytes(b.try_into().unwrap()))]).unwrap_or_default(),
        _ => vec![],
    }
}

fn head_of(program: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let mut head = vec![];
    if let Ok(f) = std::fs::File::open(program) {
        let _ = f.take(64 * 1024).read_to_end(&mut head);
    }
    head
}

/// What `program` says it is, read from its files (never by running it).
pub fn probe(program: &Path) -> Option<Identity> {
    #[cfg(windows)]
    {
        let (version, changeset) = parse_product_version(&windows_product_version(program)?)?;
        Some(Identity { version, changeset, arch: pe_arch(&head_of(program)).into_iter().collect() })
    }
    #[cfg(target_os = "macos")]
    {
        // .../Unity.app/Contents/MacOS/Unity → .../Unity.app/Contents/Info.plist
        let plist = plist::Value::from_file(program.parent()?.parent()?.join("Info.plist")).ok()?;
        let dict = plist.as_dictionary()?;
        let text = dict.get("CFBundleVersion").and_then(|v| v.as_string()).or_else(|| dict.get("CFBundleShortVersionString").and_then(|v| v.as_string()))?;
        let (version, _) = parse_product_version(text)?;
        let changeset = dict.get("UnityBuildNumber").and_then(|v| v.as_string()).map(str::to_string);
        Some(Identity { version, changeset, arch: macho_arch(&head_of(program)) })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (program, head_of);
        None
    }
}

/// A program's `ProductVersion` string, through Windows' version API (it only reads the file).
#[cfg(windows)]
fn windows_product_version(program: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let path: Vec<u16> = program.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: valid null-terminated strings, a buffer of the size asked for, and pointers into that buffer only.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let translation = wide("\\VarFileInfo\\Translation");
        let mut tables = vec![];
        if VerQueryValueW(data.as_ptr().cast(), PCWSTR(translation.as_ptr()), &mut ptr, &mut len).as_bool() && len >= 4 {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for pair in pairs.chunks_exact(2) {
                tables.push(format!("{:04x}{:04x}", pair[0], pair[1]));
            }
        }
        tables.extend(["040904b0".to_string(), "040904e4".to_string()]);
        for table in tables {
            let key = wide(&format!("\\StringFileInfo\\{table}\\ProductVersion"));
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(data.as_ptr().cast(), PCWSTR(key.as_ptr()), &mut ptr, &mut len).as_bool() && len > 0 {
                let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let text = String::from_utf16_lossy(chars).trim_end_matches('\0').to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(at: &Path) -> PathBuf {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, b"stand-in").unwrap();
        at.to_path_buf()
    }

    fn says(version: &str) -> Option<Identity> {
        Some(Identity { version: version.into(), changeset: None, arch: vec![Arch::here()] })
    }

    #[test]
    fn only_the_exact_version_is_used_and_another_is_named_never_used() {
        let temp = tempfile::tempdir().unwrap();
        let wrong = program(&temp.path().join("A/6000.0.67f1/Editor/Unity.exe"));
        let unverified = program(&temp.path().join("B/6000.0.67f1/Editor/Unity.exe"));
        let exact = program(&temp.path().join("C/6000.0.67f1/Editor/Unity.exe"));
        let probe = |p: &Path| -> Option<Identity> {
            if p == wrong {
                says("2022.3.22f1")
            } else if p == exact {
                says("6000.0.67f1")
            } else {
                None
            }
        };
        let hints = |list: &[&PathBuf]| list.iter().map(|p| ((*p).clone(), Hint::HubList)).collect::<Vec<_>>();
        match find_with("6000.0.67f1", hints(&[&wrong, &unverified, &exact]), &probe) {
            Found::Exact(c) => assert_eq!(c.program, exact),
            other => panic!("{other:?}"),
        }
        match find_with("6000.0.67f1", hints(&[&wrong, &unverified]), &probe) {
            Found::Unverified(c) => assert_eq!((c.program.clone(), c.check), (unverified.clone(), Check::Unverified)),
            other => panic!("{other:?}"),
        }
        match find_with("6000.0.67f1", hints(&[&wrong]), &probe) {
            Found::Missing { wrong: w } => {
                assert_eq!(w.len(), 1);
                assert!(matches!(&w[0].check, Check::Wrong(why) if why.contains("2022.3.22f1")), "{:?}", w[0].check);
            }
            other => panic!("{other:?}"),
        }
        // A missing program, an empty hint and a relative one name nothing (no relative candidate is ever tried).
        let nothing = find_with("6000.0.67f1", vec![(temp.path().join("none/Unity.exe"), Hint::HubFolder), (PathBuf::new(), Hint::HubSecondary), (PathBuf::from("6000.0.67f1/Editor/Unity.exe"), Hint::HubSecondary)], &probe);
        assert_eq!(nothing, Found::Missing { wrong: vec![] });
    }

    #[test]
    fn a_version_is_judged_by_the_parser_and_the_processor() {
        assert_eq!(judge(says("6000.0.67f1").as_ref(), "6000.0.67f1"), Check::Verified);
        assert!(matches!(judge(says("6000.0.68f1").as_ref(), "6000.0.67f1"), Check::Wrong(_)));
        assert!(matches!(judge(says("6000.0.67f2").as_ref(), "6000.0.67f1"), Check::Wrong(_)));
        assert_eq!(judge(None, "6000.0.67f1"), Check::Unverified);
        let other = if Arch::here() == Arch::Arm64 { Arch::X86_64 } else { Arch::Arm64 };
        let foreign = Identity { version: "6000.0.67f1".into(), changeset: None, arch: vec![other] };
        if !(cfg!(target_os = "macos") && Arch::here() == Arch::Arm64) {
            assert!(matches!(judge(Some(&foreign), "6000.0.67f1"), Check::Wrong(_)), "built for another processor");
        }
        assert_eq!(parse_product_version("6000.0.67f1_78a1c2bbeb6a"), Some(("6000.0.67f1".into(), Some("78a1c2bbeb6a".into()))));
        assert_eq!(parse_product_version("2022.3.22f1"), Some(("2022.3.22f1".into(), None)));
        assert_eq!(parse_product_version("8,0,3326,52219"), None, "a program that is not Unity says no Unity version");
    }

    #[test]
    fn headers_say_the_processor() {
        let mut pe = vec![0u8; 0x100];
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        assert_eq!(pe_arch(&pe), Some(Arch::X86_64));
        pe[0x84..0x86].copy_from_slice(&0xaa64u16.to_le_bytes());
        assert_eq!(pe_arch(&pe), Some(Arch::Arm64));
        assert_eq!(pe_arch(b"MZ"), None);
        let mut fat = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2];
        fat.extend([0x01, 0, 0, 0x07]);
        fat.extend([0u8; 16]);
        fat.extend([0x01, 0, 0, 0x0c]);
        fat.extend([0u8; 16]);
        assert_eq!(macho_arch(&fat), vec![Arch::X86_64, Arch::Arm64]);
        assert_eq!(macho_arch(&[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01]), vec![Arch::Arm64]);
    }

    #[test]
    fn a_located_editor_is_kept_unless_it_is_another_version() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("app");
        let good = program(&temp.path().join("Mine/Unity.exe"));
        let bad = program(&temp.path().join("Old/Unity.exe"));
        let probe = |p: &Path| if p == bad { says("2022.3.22f1") } else { None };
        let c = locate(&store, &good, "6000.0.67f1", &probe).unwrap();
        assert_eq!((c.hint, c.check), (Hint::Located, Check::Unverified));
        assert!(locate(&store, &bad, "6000.0.67f1", &probe).is_err());
        assert!(locate(&store, Path::new("relative/Unity.exe"), "6000.0.67f1", &probe).is_err());
        assert_eq!(located(&store), vec![Located { program: good.clone(), version: "6000.0.67f1".into(), verified: false }]);
        assert_eq!(located_hints(&store, "6000.0.67f1"), vec![(good, Hint::Located)]);
        assert!(located_hints(&store, "2022.3.22f1").is_empty());
    }

    #[test]
    fn modules_are_read_from_the_editors_folders() {
        let temp = tempfile::tempdir().unwrap();
        let exe = if cfg!(target_os = "macos") { "6000.0.67f1/Unity.app/Contents/MacOS/Unity" } else { "6000.0.67f1/Editor/Unity.exe" };
        let p = program(&temp.path().join(exe));
        assert_eq!(modules(&p), Modules::default());
        let engines = playback_engines(&p).unwrap();
        std::fs::create_dir_all(engines.join("AndroidPlayer")).unwrap();
        assert_eq!(modules(&p), Modules { android: true, android_tools: false, ios: false });
        for d in ["SDK", "NDK", "OpenJDK"] {
            std::fs::create_dir_all(engines.join("AndroidPlayer").join(d)).unwrap();
        }
        std::fs::create_dir_all(engines.join("iOSSupport")).unwrap();
        assert_eq!(modules(&p), Modules { android: true, android_tools: true, ios: true });
    }
}
