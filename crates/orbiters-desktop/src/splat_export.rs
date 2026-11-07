//! Export… on a capture: its splats as a standard file for other tools, made in this process off the interface thread
//! ([`export`]) and asked for by [`crate::pages::splat_export`] (from the 3D view, the capture's window in Photos &
//! captures, its card's menu and its detail in Capture files). Three formats (orbiters_reconstruct::export): SPZ
//! version 3 (small; every SPZ reader opens it), GLB with `KHR_gaussian_splatting` (the glTF standard) and the reference
//! 3DGS PLY (full precision; every splat tool reads it).
//!
//! **Every format holds what the capture's 3D view shows**: the splats as trained without those Select splats hid or
//! deleted (orbiters_reconstruct::edit), as every share and Mixer's copy leave them out. The PLY too: whoever exports
//! takes what they see, and a PLY with a removed avatar in it would bring it back into every tool it goes through. None
//! holds the sky: no other tool draws N4N0's sky map, and the sky shell's discs, 400 m away, would wall a world in. Edits
//! that cannot be applied (damaged, or made on an earlier reconstruction) stop the export as they stop shares
//! ([`Failure::Edits`]): the dialog offers to discard them, as the 3D view does.
//!
//! The file is made whole in memory first, checked against the room left on its disk ([`ROOM_KEPT`]), written beside
//! its name (`<name>.<n>.part`, a name of its own that no other file or export has, flushed to the disk) and renamed
//! over it once the edits it applied are still those saved: an export that fails or is stopped leaves nothing under the
//! chosen name and no part file behind. Never anywhere inside the capture's own folder: reconstructing it again deletes
//! its `reconstruction/` folder whole, and deleting the capture deletes the rest for good ([`Failure::OwnFiles`]).

use orbiters_core::reconstruction as rec;
use orbiters_i18n::{mark, tr};
use orbiters_reconstruct::{edit, export as encode};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The file formats Export offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Format {
    #[default]
    Spz,
    Glb,
    Ply,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Spz, Format::Glb, Format::Ply];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Spz => "spz",
            Format::Glb => "glb",
            Format::Ply => "ply",
        }
    }

    /// Its name, as other tools call it.
    pub fn label(self) -> &'static str {
        match self {
            Format::Spz => "SPZ",
            Format::Glb => "GLB",
            Format::Ply => "PLY",
        }
    }

    /// What it is, in two words.
    pub fn kind(self) -> &'static str {
        match self {
            Format::Spz => tr("Small"),
            Format::Glb => tr("glTF standard"),
            Format::Ply => tr("Full precision"),
        }
    }

    /// Where it goes, in one plain line: the tools that open it, and those that do not yet (Unity's and Blender's own glTF
    /// importers refuse the splat extension a GLB requires; VRChat's and Unity's splat importers read SPZ and PLY,
    /// Blender's splat add-ons PLY). SuperSplat has read SPZ since 2.19 (February 2026); Cesium, three.js
    /// (GLTFGaussianSplatLoaderExtension) and Babylon.js read `KHR_gaussian_splatting`.
    pub fn hint(self) -> &'static str {
        match self {
            Format::Spz => tr("For VRChat splat worlds (VRChat Gaussian Splatting), Unity splat importers, SuperSplat, Spark and web viewers."),
            Format::Glb => tr("For Cesium, three.js and other glTF viewers that draw splats. Unity's and Blender's own importers cannot open it yet."),
            Format::Ply => tr("The original splat training format, for every splat tool and Blender's splat add-ons."),
        }
    }

    /// The save dialog's name for its files.
    pub fn filter(self) -> &'static str {
        match self {
            Format::Spz => tr("SPZ splats"),
            Format::Glb => tr("glTF binary with splats"),
            Format::Ply => tr("PLY splats"),
        }
    }

    /// About how many bytes `splats` splats take: GLB and PLY exactly (56 and 68 bytes a splat, and a header), SPZ as
    /// gzip left Home's 20 bytes a splat (731,087 splats: 12.9 MB, 17.6 bytes a splat; version 4's Zstandard streams
    /// 12.8 MB for 729,451 splats of another training).
    pub fn estimate(self, splats: usize) -> u64 {
        let n = splats as u64;
        match self {
            Format::Spz => n * 176 / 10 + 16,
            Format::Glb => n * 56 + 1500,
            Format::Ply => n * 68 + 400,
        }
    }
}

/// What an export is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Reading,
    Making,
    Writing,
}

/// How far an export got (for the dialog), and its stop.
#[derive(Default)]
pub struct Progress {
    now: Mutex<Option<(Step, f32)>>,
    stop: AtomicBool,
}

impl Progress {
    fn set(&self, step: Step, part: f32) {
        // Reading takes about 40% of an export (a PLY is the largest file of the three), making the file 30%, writing it
        // the rest.
        let (from, span) = match step {
            Step::Reading => (0.0, 0.4),
            Step::Making => (0.4, 0.3),
            Step::Writing => (0.7, 0.3),
        };
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) = Some((step, from + span * part.clamp(0.0, 1.0)));
    }

    /// The step under way and how much of the whole export is done (0 to 1); none before it starts.
    pub fn get(&self) -> Option<(Step, f32)> {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops the export at its next step (its part file goes).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn go_on(&self) -> Result<(), Failure> {
        if self.stop.load(Ordering::Relaxed) { Err(Failure::Stopped) } else { Ok(()) }
    }
}

/// An export done.
#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub path: PathBuf,
    pub format: Format,
    pub bytes: u64,
    /// The splats in the file, and those its splat edits left out.
    pub splats: usize,
    pub removed: usize,
    /// What SPZ's 8-bit values could not hold (none for GLB and PLY, which keep every value).
    pub saturated: Option<encode::Saturated>,
}

/// Why an export did not happen.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The capture's splat edits cannot be applied: why ([`edits_now`]). Discarding them is the way on.
    Edits(String),
    /// The file needs more room than its disk has left (with [`ROOM_KEPT`] kept free).
    NoRoom { needed: u64, free: u64, folder: PathBuf },
    /// The file cannot be written there: the folder, and why in words.
    NotWritable { folder: PathBuf, why: &'static str },
    /// The place chosen is inside the capture's own folder: reconstructing it again replaces its `reconstruction/`
    /// folder whole (the previous one is deleted) and deleting the capture deletes the folder for good, a file saved
    /// there with it.
    OwnFiles,
    /// The capture's splat edits were saved again while the file was made (other splats hidden, deleted or shown): it
    /// would not hold what its 3D view shows now, so it was not saved. Trying again exports them.
    Changed,
    /// Stopped (the dialog closed meanwhile).
    Stopped,
    /// Anything else (no splats, a damaged PLY): what happened.
    Failed(String),
}

impl Failure {
    /// What the dialog says, for an export to `format`.
    pub fn message(&self, format: Format) -> String {
        let mb = |b: u64| orbiters_ui::format::bytes(b as f64);
        match self {
            Failure::Edits(why) => tr!("{why} Discard them to export every splat as trained: Select splats in its 3D view can remove them again.", why = why),
            Failure::NoRoom { needed, free, folder } => tr!(
                "Not enough room: the {format} takes {size}, and {folder} has {free} left, with {kept} to leave free for {system} and other programs. Choose another place, or free up space there.",
                format = format.label(),
                size = mb(needed.saturating_sub(ROOM_KEPT)),
                folder = folder.display(),
                free = mb(*free),
                kept = mb(ROOM_KEPT),
                system = if cfg!(target_os = "macos") { "macOS" } else { "Windows" },
            ),
            Failure::NotWritable { folder, why } => tr!("N4N0 cannot write in {folder}: {why}. Choose another place.", folder = folder.display(), why = tr(why)),
            Failure::OwnFiles => {
                tr("That is inside the capture's own folder: N4N0 replaces its files when it reconstructs it, and deletes them with it. Choose another place.").into()
            }
            Failure::Changed => tr("Its splat edits changed while the file was made: nothing was saved. Try again to export what its 3D view shows now.").into(),
            Failure::Stopped => tr("The export was stopped: nothing was saved.").into(),
            Failure::Failed(e) => tr!("The export failed: {error}", error = e),
        }
    }

    /// Whether choosing another place is the way on.
    pub fn elsewhere(&self) -> bool {
        matches!(self, Failure::NoRoom { .. } | Failure::NotWritable { .. } | Failure::OwnFiles)
    }
}

/// Why a file cannot be written, as the end of "N4N0 cannot write in <folder>: …".
const WHY_OPEN: &str = mark("a file of that name is open in another program");
const WHY_PROTECTED: &str = mark("that place is read-only or protected");
const WHY_GONE: &str = mark("that folder is no longer there");

/// Room left free on the disk an export goes to, beyond the file: a full disk breaks whatever writes next (VRChat's
/// cache, Unity, Windows itself).
pub const ROOM_KEPT: u64 = 64_000_000;

/// What the capture's splat edits leave out now, or why they cannot be applied, and how many splats its reconstruction
/// made (its report's count, sky shell included; none without a report).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edits {
    pub removed: usize,
    pub problem: Option<String>,
    pub splats: Option<usize>,
}

fn stale() -> String {
    tr("Its splat edits were made on an earlier reconstruction of this capture: they cannot say which splats to leave out.").into()
}

fn unreadable(e: impl std::fmt::Display) -> String {
    tr!("Its splat edits could not be read ({error}).", error = e)
}

/// A file's modification time as the edit file records its PLY's (nanoseconds since 1970, 0 unknown).
fn modified_ns(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos().min(u64::MAX as u128) as u64)
}

/// [`Edits`] of the capture in `session`, cheap enough to ask as the dialog opens: the edit file decoded, and checked to
/// be the PLY's by the PLY's length and modification time as saved with it (the PLY is hashed again only when the time
/// differs, as orbiters_reconstruct::edit::picture_state does). [`export`] checks again on the PLY itself.
pub fn edits_now(session: &Path) -> Edits {
    let result = rec::result_dir(session);
    let splats = rec::read_report(session).map(|r| r.gaussians);
    let problem = |p: String| Edits { removed: 0, problem: Some(p), splats };
    let bytes = match std::fs::read(result.join(rec::EDIT_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Edits { removed: 0, problem: None, splats },
        Err(e) => return problem(unreadable(e)),
        Ok(b) => b,
    };
    let e = match edit::Edit::decode(&bytes) {
        Ok(e) => e,
        Err(err) => return problem(unreadable(format!("{err:#}"))),
    };
    let ply = result.join(rec::PLY_FILE);
    let Ok(meta) = std::fs::metadata(&ply) else { return problem(tr("This capture's splats are missing: reconstruct it again.").into()) };
    let stamped = e.ply_modified != 0 && modified_ns(&meta) == e.ply_modified;
    let same = meta.len() == e.ply.bytes && (stamped || edit::Identity::read(&ply).is_ok_and(|id| id == e.ply));
    if !same {
        return problem(stale());
    }
    Edits { removed: e.removed(), problem: None, splats }
}

/// The name the save dialog offers: the capture's folder name (its world and when it was recorded, as N4N0 names its
/// sessions), else its title, with what Windows refuses in a name replaced, and the format's extension.
pub fn file_name(session: &Path, title: &str, format: Format) -> String {
    let base = session.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| title.to_string());
    let clean: String = base.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c }).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_end_matches('.');
    format!("{}.{}", if clean.is_empty() { tr("Capture") } else { clean }, format.extension())
}

/// `path` with `format`'s extension: as it is when it has it already (in any case), else with it added (a name typed
/// without it, or with another).
pub fn with_extension(path: PathBuf, format: Format) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(format.extension())) {
        return path;
    }
    let mut s = path.into_os_string();
    s.push(".");
    s.push(format.extension());
    PathBuf::from(s)
}

/// Exports the capture in `session` as `format` to `dest` ([`export_with`], the disk's real room).
pub fn export(session: &Path, format: Format, dest: &Path, progress: &Progress) -> Result<Exported, Failure> {
    export_with(session, format, dest, progress, &orbiters_projects::size::free_space)
}

/// Exports the capture in `session` as `format` to `dest`: reads its PLY (in steps, saying how far), leaves out what its
/// edits remove (stops when they cannot be applied), makes the file, checks the room `free_space` says the disk has,
/// writes it beside `dest` and renames it over `dest` once the edits saved are still those it applied (else
/// [`Failure::Changed`]); stops at the next step once `progress` is told to.
pub fn export_with(session: &Path, format: Format, dest: &Path, progress: &Progress, free_space: &dyn Fn(&Path) -> Option<u64>) -> Result<Exported, Failure> {
    let result = rec::result_dir(session);
    let ply = result.join(rec::PLY_FILE);
    let folder = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    let name = dest.file_name().ok_or_else(|| Failure::Failed(tr!("{path} is not a file name", path = dest.display())))?;
    if std::fs::canonicalize(&folder).ok().zip(std::fs::canonicalize(session).ok()).is_some_and(|(a, b)| a.starts_with(b)) {
        return Err(Failure::OwnFiles);
    }
    // A rename replaces a read-only file on a Mac, where Windows refuses: the file in the way stays there too.
    if cfg!(not(windows)) && std::fs::metadata(dest).is_ok_and(|m| m.permissions().readonly()) {
        return Err(Failure::NotWritable { folder, why: WHY_PROTECTED });
    }
    let bytes = read(&ply, progress)?;
    progress.set(Step::Making, 0.0);
    let (mut g, _, found) = edit::parse_for_view(&ply, &bytes).map_err(|e| Failure::Failed(format!("{e:#}")))?;
    drop(bytes);
    // What the edits leave out, and their generation (edit::generation): what the file beside the PLY must still say
    // once the export is written.
    let (removed, applied) = match found {
        edit::Found::None => (0, edit::UNEDITED.to_string()),
        edit::Found::Current(e) => (edit::apply(&mut g, &e), e.generation()),
        edit::Found::Stale(_) => return Err(Failure::Edits(stale())),
        edit::Found::Unreadable(e) => return Err(Failure::Edits(unreadable(e))),
    };
    let splats = (0..g.len()).filter(|&i| !g.sky.get(i).copied().unwrap_or(false)).count();
    if splats == 0 {
        return Err(Failure::Failed("its 3D view hides or deletes every splat: show some again in Select splats to export them".into()));
    }
    progress.go_on()?;
    let made = match format {
        Format::Spz => encode::spz3(&g, &mut |f| {
            progress.set(Step::Making, f);
            progress.go_on().map_err(|_| anyhow::anyhow!("stopped"))
        })
        .map(|(b, s)| (b, Some(s))),
        Format::Glb => encode::gltf(&g).map(|b| (b, None)),
        Format::Ply => encode::ply(&g).map(|b| (b, None)),
    };
    drop(g);
    progress.go_on()?;
    let (data, saturated) = made.map_err(|e| Failure::Failed(format!("{e:#}")))?;
    let needed = data.len() as u64 + ROOM_KEPT;
    if let Some(free) = free_space(&folder) {
        if free < needed {
            return Err(Failure::NoRoom { needed, free, folder });
        }
    }
    // A part name of its own: never a file that was there (a download's `.part`), nor another export's to the same name
    // (one stopped a moment ago still clearing its own).
    let part = folder.join(format!("{}.{:x}-{}.part", name.to_string_lossy(), std::process::id(), PARTS.fetch_add(1, Ordering::Relaxed)));
    let written = write(&part, &data, progress).and_then(|()| {
        // Edits saved since the PLY was read (in another 3D view, or one that saved again as it closed): the file would
        // not be what the view shows now.
        if edit::generation(&result).ok().as_deref() != Some(applied.as_str()) {
            return Err(Wrote::Changed);
        }
        std::fs::rename(&part, dest).map_err(Wrote::Io)
    });
    if let Err(e) = written {
        if !matches!(e, Wrote::Exists) {
            let _ = std::fs::remove_file(&part);
        }
        return Err(match e {
            Wrote::Stopped => Failure::Stopped,
            Wrote::Changed => Failure::Changed,
            Wrote::Exists => Failure::Failed(format!("{} is already there", part.display())),
            Wrote::Io(e) => failure(&e, &folder, needed, free_space),
        });
    }
    progress.set(Step::Writing, 1.0);
    Ok(Exported { path: dest.to_path_buf(), format, bytes: data.len() as u64, splats, removed, saturated })
}

/// The PLY at `path` whole, read 8 MB at a time (saying how far, stopping when asked).
fn read(path: &Path, progress: &Progress) -> Result<Vec<u8>, Failure> {
    let missing = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => Failure::Failed(tr("this capture has no splats any more: reconstruct it again").into()),
        _ => Failure::Failed(tr!("reading {path}: {error}", path = path.display(), error = e)),
    };
    let mut file = std::fs::File::open(path).map_err(missing)?;
    let len = file.metadata().map_err(missing)?.len();
    let mut out = Vec::with_capacity(len as usize);
    let mut buf = vec![0u8; 8 << 20];
    progress.set(Step::Reading, 0.0);
    loop {
        let n = file.read(&mut buf).map_err(missing)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        progress.set(Step::Reading, out.len() as f32 / len.max(1) as f32);
        progress.go_on()?;
    }
    Ok(out)
}

/// Part files made by this run of the app: each export's own number.
static PARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Why the part file could not be written, or not renamed.
enum Wrote {
    Stopped,
    /// The edits saved are no longer those applied.
    Changed,
    /// A file of the part's name was there already (left alone).
    Exists,
    Io(std::io::Error),
}

/// `data` written to `part` (a new file: one already there is left alone) 4 MB at a time (saying how far, stopping when
/// asked), then flushed to the disk.
fn write(part: &Path, data: &[u8], progress: &Progress) -> Result<(), Wrote> {
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(part).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => Wrote::Exists,
        _ => Wrote::Io(e),
    })?;
    let mut done = 0;
    for chunk in data.chunks(4 << 20) {
        file.write_all(chunk).map_err(Wrote::Io)?;
        done += chunk.len();
        progress.set(Step::Writing, done as f32 / data.len().max(1) as f32);
        if progress.go_on().is_err() {
            return Err(Wrote::Stopped);
        }
    }
    file.sync_all().map_err(Wrote::Io)
}

/// What a write error means for the person: no room, a place N4N0 may not write in (read-only, protected, a file of
/// that name open in another program, a folder gone with its drive), or something else. Windows' own codes first: its
/// sharing and disk-full errors have no portable kind.
fn failure(e: &std::io::Error, folder: &Path, needed: u64, free_space: &dyn Fn(&Path) -> Option<u64>) -> Failure {
    use std::io::ErrorKind as K;
    let folder = folder.to_path_buf();
    // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL; ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION; ERROR_ACCESS_DENIED,
    // ERROR_WRITE_PROTECT; ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_NOT_READY.
    let windows_code = if cfg!(windows) { e.raw_os_error() } else { None };
    match (windows_code, e.kind()) {
        (Some(39 | 112), _) | (_, K::StorageFull) => Failure::NoRoom { needed, free: free_space(&folder).unwrap_or(0), folder },
        (Some(32 | 33), _) => Failure::NotWritable { folder, why: WHY_OPEN },
        (Some(5 | 19), _) | (_, K::PermissionDenied | K::ReadOnlyFilesystem) => Failure::NotWritable { folder, why: WHY_PROTECTED },
        (Some(2 | 3 | 21), _) | (_, K::NotFound) => Failure::NotWritable { folder, why: WHY_GONE },
        _ => Failure::Failed(tr!("writing in {path}: {error}", path = folder.display(), error = e)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use orbiters_reconstruct::gaussians::Gaussians;

    /// A reconstructed session in a fresh folder: eight splats along X at z = 2 (the third on the sky shell), each its
    /// own colour, a report, and with `removed` its splat edits hiding splat 1 and deleting splat 5. Returns the folder
    /// and the positions of the splats an export holds (Unity's world, PLY order).
    pub(crate) fn session(name: &str, removed: bool) -> (PathBuf, Vec<[f32; 3]>) {
        let dir = std::env::temp_dir().join(format!("orbiters-export-{name}-{}", uuid::Uuid::new_v4())).join("Home 2026-11-02 21-30");
        let result = rec::dir(&dir);
        std::fs::create_dir_all(&result).unwrap();
        let mut g = Gaussians::default();
        for i in 0..8 {
            g.push([i as f32, 1.0, 2.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 2.0, [i as f32 / 8.0, 0.5, 0.25]);
        }
        g.sky[2] = true;
        let ply = result.join(rec::PLY_FILE);
        orbiters_reconstruct::ply::write(&ply, &g).unwrap();
        let report = rec::Report { gaussians: 8, ..Default::default() };
        std::fs::write(result.join(rec::REPORT_FILE), serde_json::to_vec(&report).unwrap()).unwrap();
        let mut kept: Vec<usize> = vec![0, 1, 3, 4, 5, 6, 7];
        if removed {
            let (_, identity, _) = edit::read_for_view(&ply).unwrap();
            let mut e = edit::Edit::new(8, identity);
            e.marks[1] = edit::HIDDEN;
            e.marks[5] = edit::DELETED;
            e.save(&result).unwrap();
            kept.retain(|i| *i != 1 && *i != 5);
        }
        (dir, kept.into_iter().map(|i| g.means[i]).collect())
    }

    pub(crate) fn remove(session: &Path) {
        let _ = std::fs::remove_dir_all(session.parent().unwrap());
    }

    /// The positions an exported file holds, in Unity's world, as each format's readers read them (GLB: glTF's frame,
    /// X negated; SPZ and PLY: Z negated), checked against the format's own header and count.
    pub(crate) fn positions(path: &Path, format: Format) -> Vec<[f32; 3]> {
        let b = std::fs::read(path).unwrap();
        match format {
            Format::Spz => {
                let mut raw = Vec::new();
                flate2::read::GzDecoder::new(&b[..]).read_to_end(&mut raw).unwrap();
                assert_eq!(raw[..4], *b"NGSP");
                assert_eq!(u32::from_le_bytes(raw[4..8].try_into().unwrap()), 3, "version 3");
                let n = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
                assert_eq!(raw.len(), 16 + n * 20, "degree zero: 20 bytes a splat");
                (0..n)
                    .map(|i| {
                        let p: [f32; 3] = std::array::from_fn(|k| {
                            let c = &raw[16 + (3 * i + k) * 3..16 + (3 * i + k) * 3 + 3];
                            ((c[0] as i32 | (c[1] as i32) << 8 | (c[2] as i32) << 16) << 8 >> 8) as f32 / 4096.0
                        });
                        [p[0], p[1], -p[2]]
                    })
                    .collect()
            }
            Format::Glb => {
                assert_eq!(b[..4], *b"glTF");
                let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
                let j: serde_json::Value = serde_json::from_slice(&b[20..20 + n]).unwrap();
                assert_eq!(j["extensionsUsed"][0], "KHR_gaussian_splatting");
                let count = j["accessors"][0]["count"].as_u64().unwrap() as usize;
                let at = 28 + n + j["bufferViews"][0]["byteOffset"].as_u64().unwrap() as usize;
                (0..count)
                    .map(|i| {
                        let f = |k: usize| f32::from_le_bytes(b[at + (3 * i + k) * 4..at + (3 * i + k) * 4 + 4].try_into().unwrap());
                        [-f(0), f(1), f(2)]
                    })
                    .collect()
            }
            Format::Ply => {
                let g = orbiters_reconstruct::ply::parse(&b).unwrap();
                let end = b.windows(11).position(|w| w == b"end_header\n").unwrap();
                assert!(!std::str::from_utf8(&b[..end]).unwrap().contains("sky"), "the reference layout alone");
                g.means
            }
        }
    }

    fn close(a: &[[f32; 3]], b: &[[f32; 3]]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(p, q)| p.iter().zip(q).all(|(x, y)| (x - y).abs() <= 0.5 / 4096.0))
    }

    /// Each format holds the splats the 3D view shows, in its readers' frame: none hidden, none deleted, no sky; the
    /// destination is replaced whole and no part file stays.
    #[test]
    fn every_format_leaves_out_what_the_3d_view_removed_and_the_sky() {
        let (dir, kept) = session("formats", true);
        let out = dir.parent().unwrap().join("out");
        std::fs::create_dir_all(&out).unwrap();
        for format in Format::ALL {
            let dest = out.join(file_name(&dir, "Home", format));
            std::fs::write(&dest, b"an older export").unwrap();
            let progress = Progress::default();
            let done = export(&dir, format, &dest, &progress).unwrap();
            assert_eq!((done.splats, done.removed, done.format), (5, 2, format));
            assert_eq!(done.bytes, std::fs::metadata(&dest).unwrap().len());
            assert_eq!(progress.get(), Some((Step::Writing, 1.0)));
            assert!(close(&positions(&dest, format), &kept), "{format:?}: {:?}", positions(&dest, format));
            assert_eq!(done.saturated.is_some(), format == Format::Spz);
        }
        let left: Vec<_> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), 3, "no part file: {left:?}");
        assert!(left.contains(&"Home 2026-11-02 21-30.spz".to_string()));
        // Without edits: every splat but the sky.
        edit::discard(&rec::result_dir(&dir)).unwrap();
        let dest = out.join("all.glb");
        assert_eq!(export(&dir, Format::Glb, &dest, &Progress::default()).unwrap().splats, 7);
        remove(&dir);
    }

    /// Edits that cannot be applied stop the export with why, before anything is written; discarded, it goes on.
    #[test]
    fn damaged_or_stale_edits_stop_the_export() {
        let (dir, _) = session("edits", true);
        let result = rec::result_dir(&dir);
        let dest = dir.parent().unwrap().join("x.spz");
        assert_eq!(edits_now(&dir), Edits { removed: 2, problem: None, splats: Some(8) });
        std::fs::write(result.join(rec::EDIT_FILE), b"N4EDIT01 damaged").unwrap();
        let problem = edits_now(&dir).problem.expect("damaged");
        assert!(problem.contains("could not be read"), "{problem}");
        match export(&dir, Format::Spz, &dest, &Progress::default()) {
            Err(Failure::Edits(why)) => assert!(why.contains("could not be read")),
            other => panic!("{other:?}"),
        }
        assert!(!dest.exists());
        // An edit of another PLY.
        let mut e = edit::Edit::new(8, edit::Identity { bytes: 1234, hash: 99 });
        e.marks[0] = edit::HIDDEN;
        std::fs::write(result.join(rec::EDIT_FILE), e.encode()).unwrap();
        assert_eq!(edits_now(&dir).problem, Some(stale()));
        assert_eq!(export(&dir, Format::Spz, &dest, &Progress::default()), Err(Failure::Edits(stale())));
        crate::pages::splats::discard_edits(&dir).unwrap();
        assert_eq!(edits_now(&dir), Edits { removed: 0, problem: None, splats: Some(8) });
        assert_eq!(export(&dir, Format::Spz, &dest, &Progress::default()).unwrap().splats, 7);
        remove(&dir);
    }

    /// No room, a read-only file in the way, a folder gone, the capture's own folder, a stop: each says so, and nothing
    /// is left behind (an older file stays as it was).
    #[test]
    fn failures_say_why_and_leave_nothing_behind() {
        let (dir, _) = session("failures", false);
        let out = dir.parent().unwrap().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let dest = out.join("x.glb");
        let parts = || std::fs::read_dir(&out).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count();

        let full = export_with(&dir, Format::Glb, &dest, &Progress::default(), &|_| Some(1 << 20));
        match &full {
            Err(Failure::NoRoom { needed, free, folder }) => {
                assert!(*needed > ROOM_KEPT + 7 * 56 && *needed < ROOM_KEPT + Format::Glb.estimate(7), "{needed}");
                assert_eq!((*free, folder), (1 << 20, &out));
            }
            other => panic!("{other:?}"),
        }
        let said = full.unwrap_err().message(Format::Glb);
        assert!(said.contains("Not enough room") && said.contains("Choose another place"), "{said}");
        assert!(!dest.exists() && parts() == 0);

        std::fs::write(&dest, b"kept").unwrap();
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&dest, perms.clone()).unwrap();
        match export(&dir, Format::Glb, &dest, &Progress::default()) {
            Err(f @ Failure::NotWritable { .. }) => assert!(f.elsewhere() && f.message(Format::Glb).contains("read-only"), "{f:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"kept", "the file in the way stays as it was");
        assert_eq!(parts(), 0);
        perms.set_readonly(false);
        std::fs::set_permissions(&dest, perms).unwrap();

        let gone = out.join("unplugged").join("x.glb");
        match export(&dir, Format::Glb, &gone, &Progress::default()) {
            Err(Failure::NotWritable { why, .. }) => assert_eq!(why, "that folder is no longer there"),
            other => panic!("{other:?}"),
        }

        // Anywhere inside the capture's folder: its reconstruction folder (over the PLY itself too), a folder in it
        // (deleted with it when the capture is reconstructed again), the capture's folder (deleted with the capture).
        let result = rec::result_dir(&dir);
        let trained = std::fs::read(result.join(rec::PLY_FILE)).unwrap();
        std::fs::create_dir_all(result.join("exports")).unwrap();
        for own in [result.join("copy.ply"), result.join(rec::PLY_FILE), result.join("exports").join("x.ply"), dir.join("x.ply")] {
            assert_eq!(export(&dir, Format::Ply, &own, &Progress::default()), Err(Failure::OwnFiles), "{}", own.display());
            assert!(Failure::OwnFiles.elsewhere());
        }
        assert_eq!(std::fs::read(result.join(rec::PLY_FILE)).unwrap(), trained, "the PLY as trained");
        assert!(!result.join("copy.ply").exists() && !dir.join("x.ply").exists());

        let stopped = Progress::default();
        stopped.stop();
        assert_eq!(export(&dir, Format::Spz, &out.join("y.spz"), &stopped), Err(Failure::Stopped));
        assert!(!out.join("y.spz").exists() && parts() == 0);

        // A file named as a part file of the old scheme (a download's `.part`) is never the export's.
        let theirs = out.join("z.spz.part");
        std::fs::write(&theirs, b"someone else's").unwrap();
        export(&dir, Format::Spz, &out.join("z.spz"), &Progress::default()).unwrap();
        assert_eq!(std::fs::read(&theirs).unwrap(), b"someone else's");
        std::fs::remove_file(&theirs).unwrap();
        assert_eq!(parts(), 0);

        // Windows' own codes for a disk filling during the write, and for a file of that name open in another program.
        if cfg!(windows) {
            for code in [39, 112] {
                let f = failure(&std::io::Error::from_raw_os_error(code), &out, ROOM_KEPT + 100, &|_| Some(5));
                assert_eq!(f, Failure::NoRoom { needed: ROOM_KEPT + 100, free: 5, folder: out.clone() });
            }
            let open = failure(&std::io::Error::from_raw_os_error(32), &out, 0, &|_| None);
            assert!(open.elsewhere() && open.message(Format::Spz).contains("open in another program"), "{open:?}");
        }
        // Elsewhere the same numbers mean something else (32 a broken pipe, 5 an input/output error), and the system's
        // error kinds say what happened: no room (ENOSPC), read-only (EACCES).
        if cfg!(unix) {
            let f = failure(&std::io::Error::from_raw_os_error(28), &out, ROOM_KEPT + 100, &|_| Some(5));
            assert_eq!(f, Failure::NoRoom { needed: ROOM_KEPT + 100, free: 5, folder: out.clone() });
            let denied = failure(&std::io::Error::from_raw_os_error(13), &out, 0, &|_| None);
            assert!(denied.elsewhere() && denied.message(Format::Spz).contains("read-only"), "{denied:?}");
            let pipe = failure(&std::io::Error::from_raw_os_error(32), &out, 0, &|_| None);
            assert!(matches!(pipe, Failure::Failed(_)), "{pipe:?}");
        }
        remove(&dir);
    }

    /// Edits saved while the file is made (here as the room on the disk is asked, after the PLY was read): the export is
    /// not saved, says why, and the next one holds the edits there are now.
    #[test]
    fn edits_saved_during_an_export_keep_it_from_being_saved() {
        let (dir, _) = session("changed", true);
        let out = dir.parent().unwrap().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let dest = out.join("x.glb");
        let result = rec::result_dir(&dir);
        let hide_another = |_: &Path| {
            let mut e = edit::Edit::decode(&std::fs::read(result.join(rec::EDIT_FILE)).unwrap()).unwrap();
            e.marks[0] = edit::HIDDEN;
            e.save(&result).unwrap();
            None::<u64>
        };
        assert_eq!(export_with(&dir, Format::Glb, &dest, &Progress::default(), &hide_another), Err(Failure::Changed));
        assert!(Failure::Changed.message(Format::Glb).contains("Try again"));
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "no part file");
        let done = export(&dir, Format::Glb, &dest, &Progress::default()).unwrap();
        assert_eq!((done.splats, done.removed), (4, 3));
        assert_eq!(positions(&dest, Format::Glb).len(), 4);

        // Every splat hidden or deleted: why, not an encoder's limit.
        let mut e = edit::Edit::decode(&std::fs::read(result.join(rec::EDIT_FILE)).unwrap()).unwrap();
        for (i, m) in e.marks.iter_mut().enumerate() {
            if i != 2 {
                *m = edit::DELETED;
            }
        }
        e.save(&result).unwrap();
        match export(&dir, Format::Spz, &out.join("none.spz"), &Progress::default()) {
            Err(Failure::Failed(why)) => assert!(why.contains("every splat"), "{why}"),
            other => panic!("{other:?}"),
        }
        remove(&dir);
    }

    /// Every format of a real capture through the whole export, timed (a lab run, not by default; nothing is written in
    /// the session): `ORBITERS_EXPORT_SESSION=<session folder> ORBITERS_EXPORT_OUT=<folder> cargo test -p
    /// orbiters-desktop export_a_real_capture -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn export_a_real_capture() {
        let dir = PathBuf::from(std::env::var("ORBITERS_EXPORT_SESSION").expect("ORBITERS_EXPORT_SESSION"));
        let out = PathBuf::from(std::env::var("ORBITERS_EXPORT_OUT").expect("ORBITERS_EXPORT_OUT"));
        std::fs::create_dir_all(&out).unwrap();
        println!("{:?}", edits_now(&dir));
        for format in Format::ALL {
            let start = std::time::Instant::now();
            let done = export(&dir, format, &out.join(file_name(&dir, "", format)), &Progress::default()).unwrap();
            println!(
                "{}: {} bytes ({:.2} a splat; estimated {}), {} splats, {} removed, {:.2} s, {:?}",
                format.label(),
                done.bytes,
                done.bytes as f64 / done.splats as f64,
                format.estimate(done.splats),
                done.splats,
                done.removed,
                start.elapsed().as_secs_f32(),
                done.saturated
            );
        }
    }

    #[test]
    fn names_are_the_capture_folders_with_the_formats_extension() {
        assert_eq!(file_name(Path::new("C:/Sessions/Home 2026-11-02 21-30"), "Home", Format::Spz), "Home 2026-11-02 21-30.spz");
        assert_eq!(file_name(Path::new(""), "Bar: \"The\" <Club>?", Format::Glb), "Bar The Club.glb");
        assert_eq!(file_name(Path::new(""), " .. ", Format::Ply), "Capture.ply");
        assert_eq!(with_extension(PathBuf::from("C:/x/model"), Format::Spz), PathBuf::from("C:/x/model.spz"));
        assert_eq!(with_extension(PathBuf::from("C:/x/model.SPZ"), Format::Spz), PathBuf::from("C:/x/model.SPZ"));
        assert_eq!(with_extension(PathBuf::from("C:/x/model.glb"), Format::Ply), PathBuf::from("C:/x/model.glb.ply"));
    }
}
