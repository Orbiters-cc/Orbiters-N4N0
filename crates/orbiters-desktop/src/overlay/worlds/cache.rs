//! Captures kept so that showing one again, or going back and forth between a few worlds, needs no new download:
//!
//! - **In memory**, the ready captures (the splats ordered for the reveal, the sky map) within a byte budget
//!   ([`MEMORY_BUDGET`], `ORBITERS_3D_WORLDS_MEMORY_MB` to change it), the least lately shown let go first and the one
//!   on screen never. A capture found there shows at once, without a download.
//! - **On disk**, the files as downloaded (a slot's PC tree or the API's `.splat`, its sky map) in the app's cache
//!   folder within a cap of their own ([`DISK_CAP`]), the least lately used removed first, written only while the drive
//!   keeps [`MIN_FREE`] free. A kept file is asked about again when Orbiters' storage said how (`ETag`,
//!   `Last-Modified`: a `304` costs no download), else trusted by its name: the listing's id, slot and the upload's
//!   key, which a capture shared again never keeps (and a slot's tree carries it, checked when read).

use super::catalog::{Capture, Entry};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Bytes of ready captures kept in memory at most: six of a million splats (60 MB each) beside the one on screen, as many as a carousel shows a step from. One shown longer ago is
/// read again from the disk, 0.3 to 0.7 s (the owner's answer, review 1124: 1.3 GB were kept beside VRChat).
pub const MEMORY_BUDGET: u64 = 384 << 20;
/// Bytes of downloaded files kept on disk at most.
pub const DISK_CAP: u64 = 2 << 30;
/// Files are kept on disk only while the drive keeps this much free after them.
pub const MIN_FREE: u64 = 10 << 30;

/// What names a capture's files: its listing id, its slot (none: the API's preview files) and its upload's key, with
/// its splats and whether it has a sky.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub id: u64,
    pub slot: Option<u32>,
    pub key: u32,
    pub splats: u32,
    pub sky: bool,
}

impl Key {
    pub fn of(e: &Entry) -> Self {
        Self { id: e.id, slot: e.slot, key: e.key, splats: e.splats, sky: e.sky }
    }

    /// Its files' name on disk (`<part>` says which: `pc`, `splat`, `sky`).
    pub fn file(&self, part: &str) -> String {
        let slot = self.slot.map_or("api".to_string(), |s| format!("s{s}"));
        format!("{}-{slot}-{:08x}-{}-{part}", self.id, self.key, self.splats)
    }
}

/// The bytes a ready capture holds in memory: every vector's room. Counting only the
/// splats' lengths, the budget saw 38 % of what the captures took: the owner's overlay held 4.1 GB under a 1.28 GB budget (request 18).
pub fn capture_bytes(c: &Capture) -> u64 {
    let distances = c.distances.capacity() * std::mem::size_of::<f32>();
    let sky = c.sky.as_ref().map_or(0, |s| s.rgb.capacity() * std::mem::size_of::<[f32; 3]>());
    (c.gaussians.heap_bytes() + distances + sky + std::mem::size_of::<Capture>()) as u64
}

struct Kept {
    key: Key,
    capture: Arc<Capture>,
    bytes: u64,
    /// When it was last shown (a counter).
    used: u64,
}

/// Ready captures, within a byte budget.
pub struct Memory {
    budget: u64,
    items: Vec<Kept>,
    clock: u64,
}

impl Memory {
    pub fn new(budget: u64) -> Self {
        Self { budget, items: Vec::new(), clock: 0 }
    }

    /// The budget [`MEMORY_BUDGET`], or `ORBITERS_3D_WORLDS_MEMORY_MB`'s.
    pub fn with_default_budget() -> Self {
        let mb = std::env::var("ORBITERS_3D_WORLDS_MEMORY_MB").ok().and_then(|v| v.trim().parse::<u64>().ok());
        Self::new(mb.map_or(MEMORY_BUDGET, |mb| mb << 20))
    }

    /// The capture kept for `key`, counted as shown now.
    pub fn get(&mut self, key: &Key) -> Option<Arc<Capture>> {
        self.clock += 1;
        let clock = self.clock;
        self.items.iter_mut().find(|k| k.key == *key).map(|k| {
            k.used = clock;
            k.capture.clone()
        })
    }

    /// Keeps `capture` (shown now), then lets go of the least lately shown ones while over the budget: never `on_screen`
    /// nor this one.
    pub fn put(&mut self, key: Key, capture: Arc<Capture>, on_screen: Option<&Key>) {
        self.clock += 1;
        let bytes = capture_bytes(&capture);
        self.items.retain(|k| k.key != key);
        self.items.push(Kept { key: key.clone(), capture, bytes, used: self.clock });
        while self.bytes() > self.budget {
            let oldest = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, k)| k.key != key && Some(&k.key) != on_screen)
                .min_by_key(|(_, k)| k.used)
                .map(|(i, _)| i);
            match oldest {
                Some(i) => {
                    self.items.remove(i);
                }
                None => break,
            }
        }
    }

    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|k| k.bytes).sum()
    }

    pub fn holds(&self, key: &Key) -> bool {
        self.items.iter().any(|k| k.key == *key)
    }

}

/// What Orbiters' storage said of a kept file, to ask about it again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validators {
    pub url: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
}

impl Validators {
    /// Whether the storage can be asked if it changed.
    pub fn askable(&self) -> bool {
        self.etag.is_some() || self.last_modified.is_some()
    }
}

/// Downloaded files on disk, within a cap.
pub struct Disk {
    dir: PathBuf,
    cap: u64,
    min_free: u64,
}

impl Disk {
    pub fn new(dir: PathBuf, cap: u64, min_free: u64) -> Self {
        Self { dir, cap, min_free }
    }

    /// In the app's cache folder.
    pub fn in_cache() -> Self {
        Self::new(orbiters_core::paths::cache_dir().join("3d-worlds"), DISK_CAP, MIN_FREE)
    }

    fn data(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.bin"))
    }

    fn meta(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// The file kept under `name`, and what the storage said of it.
    pub fn read(&self, name: &str) -> Option<(Vec<u8>, Validators)> {
        let validators: Validators = serde_json::from_slice(&std::fs::read(self.meta(name)).ok()?).ok()?;
        let bytes = std::fs::read(self.data(name)).ok()?;
        Some((bytes, validators))
    }

    /// Counts it as used now (the least lately used go first).
    pub fn touch(&self, name: &str) {
        if let Ok(f) = std::fs::File::options().write(true).open(self.data(name)) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    pub fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.data(name));
        let _ = std::fs::remove_file(self.meta(name));
    }

    /// Keeps `bytes` under `name` when the drive has room, then the least lately used go while over the cap. Returns
    /// whether it was kept.
    pub fn write(&self, name: &str, bytes: &[u8], validators: &Validators) -> bool {
        let len = bytes.len() as u64;
        if len > self.cap || std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        if free_space(&self.dir).is_none_or(|free| free < self.min_free + len) {
            return false;
        }
        let tmp = self.dir.join(format!("{name}.tmp"));
        let kept = std::fs::write(&tmp, bytes).is_ok()
            && std::fs::rename(&tmp, self.data(name)).is_ok()
            && serde_json::to_vec(validators).is_ok_and(|v| std::fs::write(self.meta(name), v).is_ok());
        if !kept {
            let _ = std::fs::remove_file(&tmp);
            self.remove(name);
            return false;
        }
        self.trim(Some(name));
        true
    }

    /// Removes the least lately used files while over the cap (`keep` stays).
    pub fn trim(&self, keep: Option<&str>) {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(String, u64, SystemTime)> = dir
            .flatten()
            .filter_map(|f| {
                let path = f.path();
                (path.extension()? == "bin").then_some(())?;
                let name = path.file_stem()?.to_string_lossy().into_owned();
                let m = f.metadata().ok()?;
                Some((name, m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.2);
        for (name, len, _) in files {
            if total <= self.cap {
                break;
            }
            if Some(name.as_str()) == keep {
                continue;
            }
            self.remove(&name);
            total = total.saturating_sub(len);
        }
    }

    /// The bytes kept.
    pub fn bytes(&self) -> u64 {
        std::fs::read_dir(&self.dir)
            .map(|d| d.flatten().filter(|f| f.path().extension().is_some_and(|e| e == "bin")).filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum())
            .unwrap_or(0)
    }
}

/// The bytes free on the drive of `dir`, as the storage page measures them.
fn free_space(dir: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let dir = dir.to_string_lossy().to_lowercase();
    disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::stereo::StartView;
    use orbiters_reconstruct::gaussians::Gaussians;

    fn capture(splats: usize) -> Arc<Capture> {
        let mut g = Gaussians::default();
        for i in 0..splats {
            g.push([i as f32, 1.6, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 0.0, [0.5; 3]);
        }
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        Arc::new(Capture::new(g, None, view))
    }

    fn key(id: u64) -> Key {
        Key { id, slot: Some(id as u32), key: 7, splats: 1000, sky: false }
    }

    #[test]
    fn a_capture_shared_again_is_another_key() {
        let a = key(1);
        assert_eq!(a, key(1));
        assert_ne!(a, Key { key: 8, ..a.clone() }, "a new upload at the slot");
        assert_ne!(a, Key { slot: Some(2), ..a.clone() });
        assert_ne!(a, Key { splats: 999, ..a.clone() });
        assert_ne!(a.file("pc"), Key { key: 8, ..a.clone() }.file("pc"));
        assert_ne!(a.file("pc"), a.file("sky"));
        assert_eq!(Key { slot: None, ..a.clone() }.file("splat"), "1-api-00000007-1000-splat");
    }

    #[test]
    fn memory_lets_go_of_the_least_lately_shown_but_never_the_one_on_screen() {
        let one = capture_bytes(&capture(1000));
        // Each splat's 56 bytes and its distance (4): nothing that only training reads (DOV-5: 85 bytes a splat).
        assert!((1000 * 60..1000 * 62).contains(&one), "{one}");
        // Room for three.
        let mut m = Memory::new(one * 3);
        for id in 1..=3 {
            m.put(key(id), capture(1000), Some(&key(id)));
        }
        assert_eq!(m.items.len(), 3);
        // 1 shown again: 2 is now the least lately shown.
        assert!(m.get(&key(1)).is_some());
        m.put(key(4), capture(1000), Some(&key(4)));
        assert!(!m.holds(&key(2)) && m.holds(&key(1)) && m.holds(&key(3)) && m.holds(&key(4)));
        // On screen, 3 stays though it is the oldest.
        m.put(key(5), capture(1000), Some(&key(3)));
        assert!(m.holds(&key(3)) && m.holds(&key(5)) && !m.holds(&key(1)), "{:?}", (m.holds(&key(1)), m.holds(&key(4))));
        assert!(m.bytes() <= one * 3);
        // One larger than the budget is still kept while it is the one shown; the rest go.
        m.put(key(6), capture(5000), Some(&key(6)));
        assert!(m.holds(&key(6)));
        assert_eq!(m.items.len(), 1);
        // The same key again replaces it.
        m.put(key(6), capture(10), Some(&key(6)));
        assert_eq!((m.items.len(), m.bytes()), (1, capture_bytes(&capture(10))));
        assert!(m.get(&key(9)).is_none());
    }

    /// Request 18: the budget counts what a capture takes, the room of its vectors too; a capture made from splats pushed one by one
    /// holds no more room than it uses.
    #[test]
    fn the_budget_counts_what_a_capture_takes() {
        let mut pushed = Gaussians::default();
        for i in 0..5000 {
            pushed.push([i as f32, 1.6, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 0.0, [0.5; 3]);
        }
        assert!(pushed.means.capacity() > pushed.len(), "pushed one by one, a vector has room over");
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        let mut c = Capture::new(pushed, None, view);
        assert_eq!(c.gaussians.means.capacity(), c.gaussians.len(), "the capture's own vectors are exactly as long as needed");
        let exact = capture_bytes(&c);
        assert!((5000 * 60..5000 * 61).contains(&exact), "{exact}");
        assert!(c.gaussians.seen.capacity() == 0 && c.gaussians.sky.capacity() == 0 && c.gaussians.filter_3d.capacity() == 0);
        // The same small: a copy of the same kind.
        assert!((4000 * 60..5000 * 61).contains(&capture_bytes(&c.around_subject())));
        // Room over is counted.
        c.gaussians.seen.reserve_exact(5000);
        assert!(capture_bytes(&c) >= exact + 5000 * std::mem::size_of::<orbiters_reconstruct::gaussians::Seen>() as u64);
    }

    #[test]
    fn disk_keeps_files_within_its_cap_the_least_lately_used_going_first() {
        let dir = std::env::temp_dir().join(format!("orbiters-3d-worlds-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let disk = Disk::new(dir.clone(), 2500, 0);
        let v = |etag: Option<&str>| Validators { url: "https://files.orbiters.cc/x".into(), etag: etag.map(str::to_string), last_modified: None };
        assert!(disk.write("a", &[1u8; 1000], &v(Some("\"1\""))));
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(disk.write("b", &[2u8; 1000], &v(None)));
        std::thread::sleep(std::time::Duration::from_millis(30));
        // a used again: b goes when c needs the room.
        disk.touch("a");
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(disk.write("c", &[3u8; 1000], &v(None)));
        assert!(disk.read("b").is_none());
        let (bytes, kept) = disk.read("a").unwrap();
        assert_eq!((bytes.len(), kept.etag.as_deref(), kept.askable()), (1000, Some("\"1\""), true));
        assert!(!disk.read("c").unwrap().1.askable());
        assert!(disk.bytes() <= 2500);
        // Larger than the cap: not kept.
        assert!(!disk.write("d", &[4u8; 3000], &v(None)));
        // A drive without the room asked for: nothing written.
        let full = Disk::new(dir.clone(), 1 << 40, u64::MAX / 2);
        assert!(!full.write("e", &[5u8; 10], &v(None)));
        assert!(full.read("e").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
