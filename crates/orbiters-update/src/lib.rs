//! Orbiters - N4N0's releases, shared by the app (which looks for updates), the setup (which installs one) and
//! `cargo xtask` (which makes and publishes them).
//!
//! - [`Release`]: one release of a channel, published as a [`SignedRelease`] at `<feed>/<channel>.json`. Its signature
//!   is checked against the release key ([`public_key`]) before anything in it is used: the server only hosts files,
//!   and cannot make an installed app run anything the release key did not sign.
//! - [`InstallRecord`]: `install.json` beside the installed programs: the version, the channel, and the files the
//!   setup put there (an update replaces those, an uninstall removes those and nothing else).
//! - [`payload`]: the programs inside the setup.
//! - [`net`]: fetching a release and its setup.

#[cfg(feature = "signing")]
use anyhow::Context as _;
#[cfg(feature = "signing")]
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(feature = "net")]
pub mod net;
#[cfg(feature = "payload")]
pub mod payload;
#[cfg(windows)]
pub mod windows_start;

/// What a release and an install record are of.
pub const PRODUCT: &str = "orbiters-desktop";
/// Where releases are published: `<feed>/<channel>.json` and the setups beside it.
pub const DEFAULT_FEED: &str = "https://orbiters.cc/downloads/desktop";
/// The channel everyone follows.
pub const STABLE: &str = "stable";
/// The app's program, in the install folder.
pub const APP_EXE: &str = "orbiters-desktop.exe";
/// The setup without its payload, in the install folder: what Windows runs to uninstall.
pub const SETUP_EXE: &str = "orbiters-setup.exe";
/// The install record's file, in the install folder.
pub const RECORD_FILE: &str = "install.json";
/// The name in Windows' installed apps, the Start menu and the shortcuts.
pub const DISPLAY_NAME: &str = "Orbiters - N4N0";
pub const PUBLISHER: &str = "Orbiters";
/// The app's entry in Windows' installed apps, under `HKEY_CURRENT_USER`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\OrbitersDesktop";
/// The value that starts the app with Windows, under `HKEY_CURRENT_USER\<RUN_KEY>`.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "Orbiters - N4N0";
/// The app's key in SteamVR: its application manifest, and SteamVR's start of it among its startup overlay apps.
pub const STEAMVR_APP_KEY: &str = "cc.orbiters.n4n0";
/// What SteamVR starts with itself: the app in the tray, as Windows does.
pub const STEAMVR_ARGUMENTS: &str = "--hidden";
pub const STEAMVR_DESCRIPTION: &str = "N4N0's menu for VRChat: fast walk, YouTube, calls, 3D pictures, music and the light";

/// The app's SteamVR manifest, in its settings folder (`%APPDATA%\Orbiters\Desktop`), where SteamVR reads it from:
/// the app and the setup write it there (the install folder's files are only the setup's payload).
pub fn steamvr_manifest_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("Orbiters").join("Desktop").join("orbiters-n4n0.vrmanifest"))
}

/// The app's settings file (`orbiters_core::paths::settings_file` in the app), where the setup leaves its "Start with
/// SteamVR" for the app when SteamVR could not be told at once.
pub fn app_settings_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("Orbiters").join("Desktop").join("settings.json"))
}

/// The public half of the release key (`update-key.pub`, written by `cargo xtask keygen`).
#[cfg(feature = "signing")]
const PUBLIC_KEY_FILE: &str = include_str!("../update-key.pub");

/// The release key releases are checked against; `None` in a build without one (no update is ever trusted then).
#[cfg(feature = "signing")]
pub fn public_key() -> Option<[u8; 32]> {
    parse_public_key(PUBLIC_KEY_FILE)
}

/// The key in a `update-key.pub`: its one line that is not a `#` comment, Base64 of the 32 bytes.
#[cfg(feature = "signing")]
pub fn parse_public_key(text: &str) -> Option<[u8; 32]> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))?;
    base64::engine::general_purpose::STANDARD.decode(line).ok()?.try_into().ok()
}

/// The feed releases are read from: [`DEFAULT_FEED`], or `ORBITERS_UPDATE_FEED` when set (a test server; what it
/// serves is checked against the same key).
pub fn feed() -> String {
    std::env::var("ORBITERS_UPDATE_FEED")
        .ok()
        .map(|f| f.trim().trim_end_matches('/').to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| DEFAULT_FEED.into())
}

/// Where a channel's release is published.
pub fn release_url(feed: &str, channel: &str) -> String {
    format!("{}/{channel}.json", feed.trim_end_matches('/'))
}

/// One release of a channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// Always [`PRODUCT`].
    pub product: String,
    pub channel: String,
    /// Semantic version, the workspace's.
    pub version: String,
    /// When it was published (UTC, RFC 3339).
    pub released: String,
    /// The commit it was built from.
    #[serde(default)]
    pub commit: String,
    /// What changed, as plain text.
    #[serde(default)]
    pub notes: String,
    /// The setup that installs it (and updates an install to it).
    pub installer: Artifact,
}

/// A published file, checked against its size and SHA-256 once downloaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Relative to the feed, or a full `https://` address.
    pub url: String,
    /// Lowercase hexadecimal.
    pub sha256: String,
    pub size: u64,
}

impl Artifact {
    /// The address to download it from.
    pub fn resolve(&self, feed: &str) -> String {
        if self.url.starts_with("https://") || self.url.starts_with("http://") {
            self.url.clone()
        } else {
            format!("{}/{}", feed.trim_end_matches('/'), self.url.trim_start_matches('/'))
        }
    }
}

/// A [`Release`] as published: its JSON, exactly the bytes signed, and the Ed25519 signature of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRelease {
    pub release: String,
    /// Base64.
    pub signature: String,
}

#[cfg(feature = "signing")]
impl SignedRelease {
    /// Signs `release` with the release key's private half (PKCS#8, as `cargo xtask keygen` writes it).
    pub fn sign(release: &Release, private_key: &[u8]) -> anyhow::Result<Self> {
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(private_key).map_err(|e| anyhow::anyhow!("not an Ed25519 private key: {e}"))?;
        let json = serde_json::to_string_pretty(release)?;
        let signature = pair.sign(json.as_bytes());
        Ok(Self { release: json, signature: base64::engine::general_purpose::STANDARD.encode(signature.as_ref()) })
    }

    /// The release in a published file, once its signature checks out against `key` and it is of [`PRODUCT`] and
    /// `channel` (a release of another channel served in its place is refused, as is anything unsigned).
    pub fn open(bytes: &[u8], key: &[u8; 32], channel: &str) -> anyhow::Result<Release> {
        let signed: SignedRelease = serde_json::from_slice(bytes).context("not a signed release")?;
        let signature = base64::engine::general_purpose::STANDARD.decode(signed.signature.trim()).context("the signature is not Base64")?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(signed.release.as_bytes(), &signature)
            .map_err(|_| anyhow::anyhow!("the release is not signed with Orbiters' release key"))?;
        let release: Release = serde_json::from_str(&signed.release).context("the signed release cannot be read")?;
        anyhow::ensure!(release.product == PRODUCT, "the release is of {}, not Orbiters - N4N0", release.product);
        anyhow::ensure!(release.channel == channel, "the release is of the {} channel, not {channel}", release.channel);
        anyhow::ensure!(semver::Version::parse(&release.version).is_ok(), "the release's version ({}) is not a version", release.version);
        anyhow::ensure!(
            release.installer.sha256.len() == 64 && release.installer.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "the release's installer has no SHA-256"
        );
        Ok(release)
    }
}

/// A new release key: its private half (PKCS#8) and its public half.
#[cfg(feature = "signing")]
pub fn generate_key() -> anyhow::Result<(Vec<u8>, [u8; 32])> {
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).map_err(|_| anyhow::anyhow!("no randomness for a key"))?;
    let public = public_key_of(pkcs8.as_ref())?;
    Ok((pkcs8.as_ref().to_vec(), public))
}

/// The public half of a private key (PKCS#8).
#[cfg(feature = "signing")]
pub fn public_key_of(private_key: &[u8]) -> anyhow::Result<[u8; 32]> {
    use ring::signature::KeyPair as _;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(private_key).map_err(|e| anyhow::anyhow!("not an Ed25519 private key: {e}"))?;
    pair.public_key().as_ref().try_into().context("an Ed25519 public key is 32 bytes")
}

/// Whether `candidate` is a later version than `current` (semantic versions; anything unreadable is not).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (semver::Version::parse(candidate), semver::Version::parse(current)) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

/// What the setup wrote in the install folder (`install.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    /// Always [`PRODUCT`].
    pub product: String,
    pub version: String,
    /// The channel the app looks for updates on.
    pub channel: String,
    /// The installed files, relative to the install folder with `/` between folders: what an update replaces and an
    /// uninstall removes. Nothing else in the folder is touched (it may be one the person chose, with their files).
    pub files: Vec<String>,
    pub desktop_shortcut: bool,
    pub start_with_windows: bool,
    /// The setup's "Start with SteamVR" (SteamVR is told at once when it runs, else the app tells it the next time it
    /// runs). Records of setups without the choice say no.
    #[serde(default)]
    pub start_with_steamvr: bool,
    /// When this version was installed (UTC, RFC 3339).
    pub installed: String,
}

impl InstallRecord {
    /// The record in `dir`, if it holds an install.
    pub fn read(dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(dir.join(RECORD_FILE)).ok()?;
        serde_json::from_slice::<Self>(&bytes).ok().filter(|r| r.product == PRODUCT)
    }

    /// Writes the record in `dir` through a temporary file renamed over it: a cut write leaves the old record.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join(RECORD_FILE);
        let tmp = dir.join(format!("{RECORD_FILE}.tmp"));
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(serde_json::to_string_pretty(self).map_err(std::io::Error::other)?.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    }

    /// The install this program runs from: its folder and record. `None` for a program that was not installed by the
    /// setup (a development build, an unpacked copy), which then never updates itself.
    pub fn of_this_program() -> Option<(PathBuf, Self)> {
        let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let record = Self::read(&dir)?;
        Some((dir, record))
    }
}

/// Where the setup installs by default: `%LOCALAPPDATA%\Programs\Orbiters - N4N0`, the person's own programs
/// folder (no administrator rights, to install or to update).
pub fn default_install_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Programs").join(DISPLAY_NAME)
}

/// Where the app keeps a downloaded setup until it installs it: `%LOCALAPPDATA%\Orbiters\Desktop\updates`, beside
/// its cache (`orbiters_core::paths::cache_dir`).
pub fn updates_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Orbiters").join("Desktop").join("updates")
}

/// The file a release's setup is downloaded to.
pub fn setup_file(version: &str) -> PathBuf {
    updates_dir().join(format!("OrbitersSetup-{version}.exe"))
}

/// The SHA-256 of a file, lowercase hexadecimal.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A relative path of the install folder (`/` between folders) as a path under `dir`; `None` for one that would land
/// outside it (absolute, a drive, `..`), which no payload or record may hold.
pub fn inside(dir: &Path, relative: &str) -> Option<PathBuf> {
    let rel = Path::new(relative);
    let normal = !relative.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    normal.then(|| dir.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "signing")]
    fn release(channel: &str) -> Release {
        Release {
            product: PRODUCT.into(),
            channel: channel.into(),
            version: "1.2.3".into(),
            released: "2026-10-23T12:00:00Z".into(),
            commit: "abc".into(),
            notes: "Faster.".into(),
            installer: Artifact { url: "1.2.3/OrbitersSetup-1.2.3.exe".into(), sha256: "a".repeat(64), size: 42 },
        }
    }

    #[test]
    #[cfg(feature = "signing")]
    fn a_signed_release_opens_with_its_key_only() {
        let (private, public) = generate_key().unwrap();
        let signed = SignedRelease::sign(&release(STABLE), &private).unwrap();
        let bytes = serde_json::to_vec(&signed).unwrap();
        assert_eq!(SignedRelease::open(&bytes, &public, STABLE).unwrap(), release(STABLE));

        let (_, other) = generate_key().unwrap();
        assert!(SignedRelease::open(&bytes, &other, STABLE).is_err(), "another key");
        assert!(SignedRelease::open(&bytes, &public, "beta").is_err(), "another channel");

        let mut tampered = signed.clone();
        tampered.release = tampered.release.replace("1.2.3/", "9.9.9/");
        assert!(SignedRelease::open(&serde_json::to_vec(&tampered).unwrap(), &public, STABLE).is_err(), "changed after signing");
        assert!(SignedRelease::open(b"{\"product\":\"orbiters-desktop\"}", &public, STABLE).is_err(), "unsigned");
    }

    #[test]
    #[cfg(feature = "signing")]
    fn a_release_needs_its_installers_hash() {
        let (private, public) = generate_key().unwrap();
        let mut r = release(STABLE);
        r.installer.sha256 = "A".repeat(64);
        let bytes = serde_json::to_vec(&SignedRelease::sign(&r, &private).unwrap()).unwrap();
        assert!(SignedRelease::open(&bytes, &public, STABLE).is_err());
    }

    #[test]
    #[cfg(feature = "signing")]
    fn the_public_key_file_is_read_past_its_comments() {
        let (private, public) = generate_key().unwrap();
        let text = format!("# comment\n\n{}\n", base64::engine::general_purpose::STANDARD.encode(public));
        assert_eq!(parse_public_key(&text), Some(public));
        assert_eq!(public_key_of(&private).unwrap(), public);
        assert_eq!(parse_public_key("# no key yet\n"), None);
        assert_eq!(parse_public_key("c2hvcnQ=\n"), None, "not 32 bytes");
    }

    #[test]
    fn versions_compare_as_semantic_versions() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("soon", "0.1.0"));
    }

    #[test]
    fn artifacts_resolve_against_the_feed() {
        let a = Artifact { url: "1.2.3/OrbitersSetup-1.2.3.exe".into(), sha256: String::new(), size: 0 };
        assert_eq!(a.resolve("https://orbiters.cc/downloads/desktop/"), "https://orbiters.cc/downloads/desktop/1.2.3/OrbitersSetup-1.2.3.exe");
        let b = Artifact { url: "https://cdn.example/x.exe".into(), ..a };
        assert_eq!(b.resolve("https://orbiters.cc/downloads/desktop"), "https://cdn.example/x.exe");
        assert_eq!(release_url("https://orbiters.cc/downloads/desktop/", STABLE), "https://orbiters.cc/downloads/desktop/stable.json");
    }

    #[test]
    fn only_paths_inside_the_folder_are_accepted() {
        let dir = Path::new("C:/Programs/Orbiters");
        assert_eq!(inside(dir, "orbiters-desktop.exe"), Some(dir.join("orbiters-desktop.exe")));
        assert_eq!(inside(dir, "licenses/LICENSE.md"), Some(dir.join("licenses/LICENSE.md")));
        for bad in ["", "../x.exe", "a/../../x", "/etc/passwd", "C:/Windows/x.dll", "./x"] {
            assert_eq!(inside(dir, bad), None, "{bad}");
        }
    }

    #[test]
    fn an_install_record_round_trips() {
        let dir = std::env::temp_dir().join(format!("orbiters-update-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let record = InstallRecord {
            product: PRODUCT.into(),
            version: "1.2.3".into(),
            channel: STABLE.into(),
            files: vec![APP_EXE.into(), SETUP_EXE.into()],
            desktop_shortcut: true,
            start_with_windows: false,
            start_with_steamvr: true,
            installed: "2026-10-23T12:00:00Z".into(),
        };
        record.write(&dir).unwrap();
        assert_eq!(InstallRecord::read(&dir), Some(record));
        std::fs::write(dir.join(RECORD_FILE), br#"{"product":"something-else"}"#).unwrap();
        assert_eq!(InstallRecord::read(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
