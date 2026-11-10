//! The avatars' and worlds' pictures the page showed, kept on this computer (`%LOCALAPPDATA%/Orbiters/project-pictures`)
//! as the window drew them, so that the next window shows them at once instead of asking the tray service for each
//! again: a JPEG per item, named after the item and the sum of the picture it was made from (`<id>.<sum>.jpg`). The
//! page asks the tray service (and through it VRChat or Orbiters) only for the pictures it does not keep, or keeps from
//! more than [`FRESH`] ago: one whose sum is the same is only marked fresh again (its file touched), a different one
//! replaces it (and its file). Read and written on workers only.

use egui::ColorImage;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Files kept at most (the newest), and how long one not written again is kept.
const MAX_FILES: usize = 600;
const MAX_AGE: Duration = Duration::from_secs(120 * 86_400);
/// A picture checked this recently is shown as kept, without asking anyone again (VRChat's API is not asked for the
/// same thumbnails at every window, sign-in or update).
pub(super) const FRESH: Duration = Duration::from_secs(86_400);

/// A picture read back: its item, the sum of the picture it was made from and the picture.
pub(super) struct Cached {
    pub id: String,
    pub sum: u64,
    pub image: ColorImage,
    /// Checked less than [`FRESH`] ago.
    pub fresh: bool,
}

pub(super) fn dir() -> PathBuf {
    orbiters_projects::data_dir().join("project-pictures")
}

/// FNV-1a over the picture's size and pixels: tells a picture from another (not a security check).
pub(super) fn sum(image: &ColorImage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for v in image.size {
        for b in (v as u32).to_le_bytes() {
            eat(b);
        }
    }
    for p in &image.pixels {
        for b in p.to_array() {
            eat(b);
        }
    }
    h
}

/// An avatar's or a world's ID as a file name (`avtr_…`, `wrld_…`); anything else is not kept.
fn safe(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn parse(name: &str) -> Option<(&str, u64)> {
    let stem = name.strip_suffix(".jpg")?;
    let (id, sum) = stem.rsplit_once('.')?;
    (safe(id) && sum.len() == 16).then_some(())?;
    Some((id, u64::from_str_radix(sum, 16).ok()?))
}

/// Keeps `image` (whose sum is `sum`) as the picture of `id` in `dir`, in place of the one kept before.
pub(super) fn save_in(dir: &Path, id: &str, sum: u64, image: &ColorImage) -> std::io::Result<()> {
    if !safe(id) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let [w, h] = image.size;
    // Flattened on black: the pictures are opaque, a world's own PNG nearly always.
    let rgb: Vec<u8> = image.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]).collect();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
        .encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
        .map_err(std::io::Error::other)?;
    let name = format!("{id}.{sum:016x}.jpg");
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dir.join(&name))?;
    for e in std::fs::read_dir(dir)?.flatten() {
        let other = e.file_name();
        let other = other.to_string_lossy();
        if other != name && parse(&other).is_some_and(|(i, _)| i == id) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(())
}

pub(super) fn save(id: &str, sum: u64, image: &ColorImage) {
    // A picture not kept is asked for again by the next window, as before.
    let _ = save_in(&dir(), id, sum, image);
}

/// The picture of `id` (sum `sum`) was asked for again and is the same: fresh for another [`FRESH`].
pub(super) fn touch_in(dir: &Path, id: &str, sum: u64) {
    if safe(id) {
        let file = dir.join(format!("{id}.{sum:016x}.jpg"));
        if let Ok(f) = std::fs::File::options().write(true).open(&file) {
            let _ = f.set_modified(SystemTime::now());
        }
    }
}

pub(super) fn touch(id: &str, sum: u64) {
    touch_in(&dir(), id, sum);
}

/// The pictures kept in `dir`, the newest first; old ones (and those past the newest [`MAX_FILES`]) are removed.
pub(super) fn load_in(dir: &Path) -> Vec<Cached> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let now = SystemTime::now();
    let mut files: Vec<(SystemTime, PathBuf, String, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tmp") {
                let _ = std::fs::remove_file(e.path());
                return None;
            }
            let (id, sum) = parse(&name)?;
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(now);
            Some((modified, e.path(), id.to_owned(), sum))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut wanted = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (n, (modified, path, id, sum)) in files.into_iter().enumerate() {
        let age = now.duration_since(modified).unwrap_or_default();
        if n >= MAX_FILES || age > MAX_AGE || !seen.insert(id.clone()) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        wanted.push((path, id, sum, age < FRESH));
    }
    // Decoded on every core: the page's first visit waits for them rather than drawing empty covers.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let chunk = wanted.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = wanted
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|(path, id, sum, fresh)| {
                            let Some(rgb) = image::open(path).ok().map(|i| i.to_rgb8()) else {
                                let _ = std::fs::remove_file(path);
                                return None;
                            };
                            let image = ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw());
                            Some(Cached { id: id.clone(), sum: *sum, image, fresh: *fresh })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    })
}

/// Starts reading the pictures kept, on a worker (at the window's start: they are there by the page's first visit).
pub(super) fn preload() -> std::sync::mpsc::Receiver<Vec<Cached>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("project-pictures".into()).spawn(move || {
        let _ = tx.send(load_in(&dir()));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Separate process: roots are set once at startup, before the actual preload worker is started.
    #[test]
    fn sandbox_preload_preserves_outside_sentinels() {
        const CHILD: &str = "ORBITERS_SENTINEL_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = PathBuf::from(root);
            crate::link_smoke::sandbox(&root).unwrap();
            assert_eq!(orbiters_projects::data_dir(), root.join("local"));
            assert_eq!(orbiters_projects::vcc_dir(), root.join("local/VCC"));
            assert_eq!(orbiters_projects::hub_data_dir(), Some(root.join("local/UnityHub")));
            assert!(orbiters_projects::discovery::seeds(&[]).is_empty());
            std::fs::create_dir_all(dir()).unwrap();
            let inside = dir().join("preload.tmp");
            std::fs::write(&inside, b"remove this private cache temporary file").unwrap();
            preload().recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(!inside.exists(), "the real preload ran and cleaned its own sandbox");
            return;
        }
        let root = std::env::temp_dir().join(format!("orbiters-sentinel-{}", uuid::Uuid::new_v4()));
        let outside = root.join("outside");
        let mut sentinels = Vec::new();
        for relative in ["project-pictures/sentinel.tmp", "project-pictures/avtr_sentinel.0000000000000001.jpg", "VCC/settings.json", "Hub/projects-v1.json"] {
            let path = outside.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = format!("untouched sentinel: {relative}").into_bytes();
            std::fs::write(&path, &bytes).unwrap();
            let meta = std::fs::metadata(&path).unwrap();
            sentinels.push((path, bytes, meta.modified().unwrap(), meta.created().ok()));
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pages::projects::thumb_cache::tests::sandbox_preload_preserves_outside_sentinels", "--nocapture"])
            .env(CHILD, root.join("sandbox"))
            .env("ORBITERS_VCC_DIR", outside.join("VCC"))
            .env("ORBITERS_UNITY_HUB_DIR", outside.join("Hub"))
            .status().unwrap();
        assert!(status.success());
        for (path, bytes, modified, created) in sentinels {
            let meta = std::fs::metadata(&path).unwrap();
            assert_eq!(meta.modified().unwrap(), modified, "{}", path.display());
            assert_eq!(meta.created().ok(), created, "{}", path.display());
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", path.display());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    fn picture(shade: u8) -> ColorImage {
        ColorImage::from_rgb([8, 6], &vec![shade; 8 * 6 * 3])
    }

    #[test]
    fn pictures_are_kept_by_item_and_sum_replacing_the_one_before() {
        let dir = std::env::temp_dir().join(format!("orbiters-project-pictures-{}", uuid::Uuid::new_v4()));
        let id = "avtr_00000000-0000-4000-8000-000000000001";
        let (a, b) = (picture(40), picture(200));
        assert_eq!(sum(&a), sum(&picture(40)));
        assert_ne!(sum(&a), sum(&b));
        save_in(&dir, id, sum(&a), &a).unwrap();
        save_in(&dir, id, sum(&b), &b).unwrap();
        save_in(&dir, "../not-an-id", 1, &a).unwrap();
        let kept = load_in(&dir);
        assert_eq!(kept.len(), 1, "one file per item, unsafe names never written");
        assert_eq!((kept[0].id.as_str(), kept[0].sum, kept[0].image.size), (id, sum(&b), [8, 6]));
        assert!(kept[0].image.pixels[0].r().abs_diff(200) <= 3, "JPEG keeps the picture");
        std::fs::write(dir.join("garbage.jpg"), b"x").unwrap();
        std::fs::write(dir.join("avtr_x.0123456789abcdef.jpg"), b"broken").unwrap();
        assert_eq!(load_in(&dir).len(), 1);
        assert!(load_in(&dir)[0].fresh, "just written: fresh");
        let file = dir.join(format!("{id}.{:016x}.jpg", sum(&b)));
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(SystemTime::now() - FRESH * 2).unwrap();
        assert!(!load_in(&dir)[0].fresh, "checked two days ago: asked for again");
        touch_in(&dir, id, sum(&b));
        assert!(load_in(&dir)[0].fresh, "the same picture again: fresh again");
        assert!(!dir.join("avtr_x.0123456789abcdef.jpg").exists(), "a broken file is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
