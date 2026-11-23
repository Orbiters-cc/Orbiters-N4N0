//! Which Creator Companion can migrate a trial to Unity 6: the installed one, or a portable beta the person points at.
//! Its version is read from its own `WebApp/Dist/buildInfo.json` (the beta's says `2.5.0-beta.3` and `"portable": true`);
//! nothing is started, written or configured. On a Mac there is no Creator Companion window to hand over to (VCC's GUI
//! is for Windows), so Orbiters only shows what the migration involves there.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use vrc_get_vpm::version::Version;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vcc {
    /// Its folder (the one holding its program and `WebApp`).
    pub folder: PathBuf,
    /// The version its build says, when it says one.
    pub version: Option<String>,
    pub portable: bool,
}

impl Vcc {
    /// Whether it is at least `needed` (`2.5.0-beta.3`); `None` when its version is unknown.
    pub fn can_migrate(&self, needed: &str) -> Option<bool> {
        let have = self.version.as_deref()?.parse::<Version>().ok()?;
        let need = needed.parse::<Version>().ok()?;
        Some(have >= need)
    }
}

/// What the Creator Companion in `path` (its folder, or its program) is.
pub fn read(path: &Path) -> Option<Vcc> {
    let folder = if path.is_file() { path.parent()?.to_path_buf() } else { path.to_path_buf() };
    let info: serde_json::Value = serde_json::from_slice(&std::fs::read(folder.join("WebApp/Dist/buildInfo.json")).ok()?).ok()?;
    let v = &info["version"];
    let version = match (v["Major"].as_u64(), v["Minor"].as_u64(), v["Patch"].as_u64()) { // i18n: not shown (field names of VCC's build file)
        (Some(a), Some(b), Some(c)) => Some(match v["PreRelease"].as_str().filter(|p| !p.is_empty()) {
            Some(pre) => format!("{a}.{b}.{c}-{pre}"),
            None => format!("{a}.{b}.{c}"),
        }),
        _ => None,
    };
    Some(Vcc { folder, version, portable: info["portable"].as_bool().unwrap_or(false) })
}

const CHOSEN_FILE: &str = "unity6-vcc.json";

/// The portable Creator Companion the person pointed at, if they did and it is still there.
pub fn chosen(dir: &Path) -> Option<Vcc> {
    let path: PathBuf = serde_json::from_slice(&std::fs::read(dir.join(CHOSEN_FILE)).ok()?).ok()?;
    read(&path)
}

/// Keeps the person's choice of a Creator Companion (its program or folder), when it is one.
pub fn choose(dir: &Path, path: &Path) -> anyhow::Result<Vcc> {
    let vcc = read(path).ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr!("{path} is not a VRChat Creator Companion.", path = path.display())))?;
    super::gate::AppRoot::open(dir)?.replace_json(Path::new(CHOSEN_FILE), &vcc.folder)?;
    Ok(vcc)
}

/// The Creator Companions on this computer: the person's choice first, then the installed one. None on a Mac.
pub fn found(dir: &Path) -> Vec<Vcc> {
    if cfg!(target_os = "macos") {
        return vec![];
    }
    let mut out: Vec<Vcc> = chosen(dir).into_iter().collect();
    if let Some(installed) = crate::vcc_app_dir().and_then(|d| read(&d)) {
        if !out.iter().any(|v| v.folder == installed.folder) {
            out.push(installed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(dir: &Path, info: &str) {
        std::fs::create_dir_all(dir.join("WebApp/Dist")).unwrap();
        std::fs::write(dir.join("WebApp/Dist/buildInfo.json"), info).unwrap();
        std::fs::write(dir.join("CreatorCompanionBeta.exe"), b"stand-in").unwrap();
    }

    #[test]
    fn the_portable_beta_is_read_from_its_build_and_judged_by_version() {
        let temp = tempfile::tempdir().unwrap();
        let beta = temp.path().join("vcc-beta");
        // What 2.5.0-beta.3's portable ZIP holds.
        build(&beta, r#"{"version":{"Major":2,"Minor":5,"Patch":0,"PreRelease":"beta.3","Build":null,"IsPreRelease":true},"branch":"release/2.5.0","portable":true}"#);
        let v = read(&beta.join("CreatorCompanionBeta.exe")).unwrap();
        assert_eq!((v.version.as_deref(), v.portable), (Some("2.5.0-beta.3"), true));
        assert_eq!(v.can_migrate("2.5.0-beta.3"), Some(true));
        let old = temp.path().join("vcc-old");
        build(&old, r#"{"version":{"Major":2,"Minor":4,"Patch":3,"PreRelease":""},"portable":false}"#);
        assert_eq!(read(&old).unwrap().can_migrate("2.5.0-beta.3"), Some(false));
        let unknown = temp.path().join("vcc-unknown");
        build(&unknown, r#"{"portable":false}"#);
        assert_eq!(read(&unknown).unwrap().can_migrate("2.5.0-beta.3"), None);
        assert_eq!(read(&temp.path().join("nothing")), None);
        // The choice is kept in app data and read again from the folder each time.
        let store = temp.path().join("app");
        assert!(choose(&store, &temp.path().join("nothing")).is_err());
        choose(&store, &beta).unwrap();
        assert_eq!(chosen(&store).unwrap().folder, beta);
    }
}
