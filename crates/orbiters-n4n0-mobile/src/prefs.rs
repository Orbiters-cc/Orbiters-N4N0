//! What a phone app keeps of its own beside the shared settings (`settings.json`, `orbiters_core::settings`): the
//! Quest's address when the person typed one, and the ports of the OSC services. A file of the app's own name in the
//! settings' folder ([`crate::app::Config::prefs_file`]).

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The Quest's address, typed in Settings: OSC goes there without waiting for discovery. `None`: found on the
    /// Wi-Fi.
    pub quest: Option<Ipv4Addr>,
    /// The ports the OSC services had, OSC then OSCQuery, for an app that keeps them ([`crate::app::Config::keep_ports`]):
    /// VRChat only sends to the ports it found.
    pub ports: Option<[u16; 2]>,
}

impl Prefs {
    /// The file called `name` in the app's settings folder.
    pub fn file(name: &str) -> PathBuf {
        orbiters_core::paths::config_dir().join(name)
    }

    /// Reads `path`; a missing or unreadable file gives the defaults.
    pub fn load_from(path: &Path) -> Self {
        match orbiters_core::read_text(path) {
            Ok(Some(text)) => orbiters_core::json_lenient::<Self>(&text).0,
            _ => Self::default(),
        }
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        orbiters_core::write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("n4n0-mobile-prefs-{}", std::process::id()));
        let path = dir.join("phone.json");
        assert_eq!(Prefs::load_from(&path), Prefs::default(), "no file: the defaults");
        let prefs = Prefs { quest: Some(Ipv4Addr::new(192, 168, 1, 42)), ports: Some([41000, 41001]) };
        prefs.save_to(&path).unwrap();
        assert_eq!(Prefs::load_from(&path), prefs);
        std::fs::write(&path, "{\"quest\": 12}").unwrap();
        assert_eq!(Prefs::load_from(&path), Prefs::default(), "a value that does not fit gets its default");
        let _ = std::fs::remove_dir_all(dir);
    }
}
