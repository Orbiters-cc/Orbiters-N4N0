//! VRChat pictures that VRCX or VRCNext already keep on this computer (REQUEST17): a friend's picture N4N0 has not kept
//! yet is taken from there instead of being downloaded again. Strictly read only: their folders are listed and their
//! picture files read (Rust opens files sharing read, write and delete, so neither app is ever kept from writing, moving
//! or deleting one), nothing is written, locked, moved or deleted there, and nothing else of theirs is read.
//!
//! A picture is matched only where the app says exactly which VRChat file and version a cached file is
//! ([`vrchat_file`]): a VRChat picture's file version never changes once uploaded, so the same file and version is the
//! same picture (a thumbnail or the full size of it). Where that cannot be told, nothing is guessed.
//!
//! - VRCX (`%APPDATA%\VRCX\ImageCache\<file id>\<version>.png`, its `Dotnet/ImageCache.cs`): the folder and file name ARE
//!   the file id and version of the address it downloaded (its `extractFileId` / `extractFileVersion`). The bytes are what
//!   VRChat sent (PNG, JPEG or WebP, whatever the extension says). Only `file_…` folders are used: the others are named
//!   after the last part of a non-VRChat address and cannot be matched.
//! - VRCNext (`%APPDATA%\VRCNext\Caches\ImageCache\<Users|Avatars>\<user or avatar id>.<jpg|png|webp|gif>`, its
//!   `Services/Helpers/ImageCacheHelper.cs`): named after the person or avatar, not the picture; which address each was
//!   downloaded from is in its database's `image_versions` table (`key` `Users/usr_…`, `url`), the only thing read from
//!   `VRCNData.db` (and its other accounts' `usr_…_VRCNData.db`). That database holds no credentials (VRCNext keeps
//!   them in its settings, encrypted, which are never opened). A file two databases disagree about (another account's
//!   VRCNext downloaded another picture to the same name) is skipped, as are its resized copies in `Thumbs` (no address
//!   is kept for them).
//!
//! A file changed less than [`SETTLED`] ago may still be being written (VRCX writes in place) or not yet be recorded
//! (VRCNext saves the address after the file): it is skipped this time. A file that is cut short or not a picture is
//! the caller's to reject when it decodes it.

use crate::locks::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// A cached file changed more recently than this is not read yet.
pub const SETTLED: Duration = Duration::from_secs(30);
/// Larger files are not pictures of a profile (a 2048-pixel PNG is about 6 MB).
const LARGEST: u64 = 16 << 20;
/// VRCNext's table is read again after this long (it learns new pictures while it runs).
const REREAD: Duration = Duration::from_secs(300);

/// The VRChat file and version an address names: `https://api.vrchat.cloud/api/1/file/file_…/3/file`,
/// `…/api/1/image/file_…/3/256`, `https://files.vrchat.cloud/…/file_…/3/…`. None for another address.
pub fn vrchat_file(url: &str) -> Option<(String, u32)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if !matches!(host, "api.vrchat.cloud" | "files.vrchat.cloud" | "assets.vrchat.com") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.split('/');
    while let Some(part) = parts.next() {
        if let Some(id) = part.strip_prefix("file_") {
            let valid = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            let version = parts.next().and_then(|v| v.parse::<u32>().ok()).filter(|v| *v > 0);
            return match (valid, version) {
                (true, Some(v)) => Some((part.to_string(), v)),
                _ => None,
            };
        }
    }
    None
}

/// Which app a picture was found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Vrcx,
    VrcNext,
}

/// The two apps' folders on this computer, and what was read of VRCNext's table.
pub struct OtherCaches {
    /// VRCX's `ImageCache` folder.
    vrcx: Option<PathBuf>,
    /// VRCNext's own folder (`…\VRCNext`).
    vrcnext: Option<PathBuf>,
    /// VRCNext's table as last read: (file id, version) → its cached file's name without extension, under `ImageCache`.
    vrcnext_index: Mutex<Option<(Instant, HashMap<(String, u32), String>)>>,
}

impl OtherCaches {
    /// The folders as these apps keep them for the person signed in to Windows. None at all where the app's folders were
    /// given ([`crate::paths::others_folders_in_roots`]: a test, a harness): their folders are then never looked at.
    pub fn on_this_computer() -> Self {
        if crate::paths::others_folders_in_roots().is_some() {
            return Self::at(None, None);
        }
        let roaming = dirs::config_dir();
        Self::at(roaming.as_ref().map(|d| d.join("VRCX").join("ImageCache")), roaming.map(|d| d.join("VRCNext")))
    }

    /// The folders given: VRCX's `ImageCache` folder and VRCNext's own folder.
    pub fn at(vrcx_images: Option<PathBuf>, vrcnext: Option<PathBuf>) -> Self {
        Self { vrcx: vrcx_images.filter(|d| d.is_dir()), vrcnext: vrcnext.filter(|d| d.is_dir()), vrcnext_index: Mutex::new(None) }
    }

    /// Whether either app is there at all.
    pub fn any(&self) -> bool {
        self.vrcx.is_some() || self.vrcnext.is_some()
    }

    /// The bytes of the picture at `url` as one of the apps keeps it (VRCX first, then VRCNext), when one keeps exactly
    /// that file and version, settled. Never an error: whatever cannot be read is not there.
    pub fn find(&self, url: &str) -> Option<(App, Vec<u8>)> {
        let (file, version) = vrchat_file(url)?;
        if let Some(dir) = &self.vrcx {
            if let Some(bytes) = settled_read(&dir.join(&file).join(format!("{version}.png"))) {
                return Some((App::Vrcx, bytes));
            }
        }
        let root = self.vrcnext.as_ref()?;
        let name = self.vrcnext_name(root, &file, version)?;
        let images = root.join("Caches").join("ImageCache");
        ["jpg", "png", "webp", "gif"].iter().find_map(|ext| settled_read(&images.join(format!("{name}.{ext}")))).map(|b| (App::VrcNext, b))
    }

    fn vrcnext_name(&self, root: &Path, file: &str, version: u32) -> Option<String> {
        let mut index = self.vrcnext_index.locked();
        if index.as_ref().is_none_or(|(at, _)| at.elapsed() > REREAD) {
            *index = Some((Instant::now(), vrcnext_index(root)));
        }
        index.as_ref()?.1.get(&(file.to_string(), version)).cloned()
    }
}

/// VRCNext's `image_versions` rows of its databases (the main one and each other account's), as (file id, version) →
/// cached file name (`Users/usr_…`). A name two databases give different addresses is left out, as is one whose
/// address names no VRChat file.
fn vrcnext_index(root: &Path) -> HashMap<(String, u32), String> {
    let mut databases = vec![root.join("VRCNData.db")];
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(user) = name.strip_suffix("_VRCNData.db") {
                if crate::vrchat_accounts::is_user_id(user) {
                    databases.push(e.path());
                }
            }
        }
    }
    let mut by_name: HashMap<String, Option<String>> = HashMap::new();
    for db in databases {
        for (key, url) in image_versions(&db) {
            match by_name.get(&key) {
                Some(Some(seen)) if *seen != url => {
                    by_name.insert(key, None);
                }
                Some(_) => {}
                None => {
                    by_name.insert(key, Some(url));
                }
            }
        }
    }
    let mut index = HashMap::new();
    for (key, url) in by_name {
        let Some(url) = url else { continue };
        let Some((sub, entity)) = key.split_once('/') else { continue };
        // Only people's and avatars' pictures (a friend's picture is one of those), named by their id: never a path.
        let id_ok = (entity.starts_with("usr_") || entity.starts_with("avtr_"))
            && entity.len() <= 80
            && entity.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !matches!(sub, "Users" | "Avatars") || !id_ok { // i18n: not shown (VRCNext's folder names)
            continue;
        }
        if let Some(found) = vrchat_file(&url) {
            index.insert(found, format!("{sub}/{entity}"));
        }
    }
    index
}

/// One database's `image_versions` rows, read only; none when it cannot be opened or read now.
fn image_versions(db: &Path) -> Vec<(String, String)> {
    let Some(connection) = crate::vrchat_accounts::open(db) else { return Vec::new() };
    let Ok(mut query) = connection.prepare("SELECT key, url FROM image_versions") else { return Vec::new() }; // i18n: not shown (SQL)
    query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// A file's bytes when it is a file, not too large, and settled ([`SETTLED`]); read twice over nothing (its size and
/// time the same before and after).
fn settled_read(path: &Path) -> Option<Vec<u8>> {
    let before = std::fs::metadata(path).ok().filter(|m| m.is_file() && m.len() > 0 && m.len() <= LARGEST)?;
    let modified = before.modified().ok()?;
    if SystemTime::now().duration_since(modified).unwrap_or_default() < SETTLED {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let after = std::fs::metadata(path).ok()?;
    (after.len() == before.len() && after.modified().ok() == Some(modified) && bytes.len() as u64 == before.len()).then_some(bytes)
}

/// A VRCNext database with only its `image_versions` table, for tests (of this crate and the desktop's).
#[cfg(any(test, feature = "test-support"))]
pub fn fake_vrcnext_table(db: &Path, rows: &[(&str, &str)]) {
    let c = rusqlite::Connection::open(db).expect("a test database"); // i18n: not shown (tests)
    c.execute_batch("CREATE TABLE IF NOT EXISTS image_versions (key TEXT PRIMARY KEY, url TEXT NOT NULL); CREATE TABLE IF NOT EXISTS secrets (v TEXT);").expect("a test table"); // i18n: not shown (tests)
    for (k, u) in rows {
        c.execute("INSERT OR REPLACE INTO image_versions (key, url) VALUES (?1, ?2)", [k, u]).expect("a test row"); // i18n: not shown (tests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "file_0a1b2c3d-1111-2222-3333-444455556666";

    /// A folder of the test's own, removed after it.
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("orbiters-picture-sources-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn old(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
    }

    fn put(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        old(path);
    }

    #[test]
    fn a_vrchat_address_names_its_file_and_version() {
        assert_eq!(vrchat_file(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/3/file")), Some((FILE.into(), 3)));
        assert_eq!(vrchat_file(&format!("https://api.vrchat.cloud/api/1/image/{FILE}/12/256")), Some((FILE.into(), 12)));
        assert_eq!(vrchat_file(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/2")), Some((FILE.into(), 2)));
        for other in [
            "https://example.com/api/1/file/file_ab/1/file".to_string(),
            "http://api.vrchat.cloud/api/1/file/file_ab/1/file".into(),
            "https://api.vrchat.cloud/api/1/file/file_ab/x/file".into(),
            "https://api.vrchat.cloud/api/1/file/file_../1/file".into(),
            "https://api.vrchat.cloud/api/1/file/file_ab".into(),
            "https://assets.vrchat.com/default/unknown.png".into(),
            String::new(),
        ] {
            assert_eq!(vrchat_file(&other), None, "{other}");
        }
    }

    #[test]
    fn vrcx_pictures_are_found_by_file_and_version_and_unsettled_ones_wait() {
        let dir = Temp::new();
        let images = dir.path().join("ImageCache");
        put(&images.join(FILE).join("3.png"), b"picture three");
        let caches = OtherCaches::at(Some(images.clone()), None);
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/image/{FILE}/3/256")), Some((App::Vrcx, b"picture three".to_vec())));
        // Another version of the same file is another picture.
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/4/file")), None);
        // Being written (changed just now): not yet; empty: never.
        std::fs::write(images.join(FILE).join("3.png"), b"half").unwrap();
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/image/{FILE}/3/256")), None);
        put(&images.join(FILE).join("5.png"), b"");
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/image/{FILE}/5/256")), None);
    }

    #[test]
    fn vrcnext_pictures_are_found_through_its_table_and_disagreements_are_skipped() {
        let dir = Temp::new();
        let root = dir.path().join("VRCNext");
        let images = root.join("Caches").join("ImageCache");
        let a = "usr_00000000-0000-4000-8000-00000000000a";
        let b = "usr_00000000-0000-4000-8000-00000000000b";
        put(&images.join("Users").join(format!("{a}.webp")), b"a's picture");
        put(&images.join("Users").join(format!("{b}.png")), b"b's picture");
        put(&images.join("Users").join("Thumbs").join(format!("{a}_64.png")), b"small");
        let table = |path: &Path, rows: &[(&str, String)]| {
            let rows: Vec<(&str, &str)> = rows.iter().map(|(k, u)| (*k, u.as_str())).collect();
            fake_vrcnext_table(path, &rows);
        };
        let file_b = "file_0a1b2c3d-1111-2222-3333-44445555bbbb";
        table(&root.join("VRCNData.db"), &[
            (&format!("Users/{a}"), format!("https://api.vrchat.cloud/api/1/image/{FILE}/7/800")),
            (&format!("Users/{b}"), format!("https://api.vrchat.cloud/api/1/image/{file_b}/1/800")),
            ("Users/../../secrets", format!("https://api.vrchat.cloud/api/1/image/{FILE}/9/800")),
        ]);
        // Another account's database says b's file is another picture: b is skipped.
        table(&root.join(format!("{a}_VRCNData.db")), &[(&format!("Users/{b}"), format!("https://api.vrchat.cloud/api/1/image/{file_b}/2/800"))]);
        let caches = OtherCaches::at(None, Some(root));
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/7/file")), Some((App::VrcNext, b"a's picture".to_vec())));
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/6/file")), None, "another version");
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/file/{file_b}/1/file")), None, "two databases disagree");
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/9/file")), None, "never a path");
    }

    #[test]
    fn missing_or_broken_folders_find_nothing_and_tests_never_look_at_the_real_ones() {
        let dir = Temp::new();
        std::fs::create_dir_all(dir.path().join("VRCNext")).unwrap();
        std::fs::write(dir.path().join("VRCNext").join("VRCNData.db"), b"not a database").unwrap();
        let caches = OtherCaches::at(Some(dir.path().join("nowhere")), Some(dir.path().join("VRCNext")));
        assert_eq!(caches.find(&format!("https://api.vrchat.cloud/api/1/file/{FILE}/1/file")), None);
        assert!(!OtherCaches::on_this_computer().any(), "a test build never reads VRCX's or VRCNext's real folders");
    }
}
