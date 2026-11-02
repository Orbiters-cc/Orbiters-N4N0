//! Splat edits: what the 3D view's selection hid or deleted of a reconstruction, kept beside its PLY
//! (`reconstruction/splat-edits.bin`, orbiters_core::reconstruction::EDIT_FILE) and never written into it, so the
//! splats as trained stay where they were and every edit can be undone.
//!
//! **A splat's ID is its index in `splats.ply`.** The worker writes the PLY once and nothing rewrites it, so that index
//! names the same splat for as long as the reconstruction stands, across reloads and in every process (the 3D view,
//! the upload preparation, the command line). Everything that reorders splats (the VRChat trees, the web preview's
//! nearest-first order) does so after the edit has been applied to the PLY's order. The edit records the PLY it was
//! made on ([`Identity`]: its length and a hash of all of its bytes); one that does not match is never applied to
//! splats it never saw ([`Found::Stale`]). A new reconstruction replaces the folder, its edits with it.
//!
//! **Hidden and deleted splats leave every output.** Hidden ones come back with Show all, deleted ones only with undo
//! (or Restore all); neither is drawn, shared, uploaded or exported: [`read_edited`] is what the VRChat trees (PC,
//! Quest, coarse), the web preview the server makes from the PC tree, `orbiters-reconstruct --web-preview/--vrchat` and
//! the pictures of the view read. An edit that cannot be read, or was made on another PLY, stops those outputs with an
//! explanation rather than letting a removed avatar back into a share; the 3D view shows it and offers to discard it.
//!
//! The file (little-endian): `N4EDIT01`, a u32 length and a JSON header (`{"format":"orbiters-splat-edit",
//! "version":1,"splats":N,"ply":{"bytes":…,"hash":"…"},"hidden":…,"deleted":…,"picture":…}`, readable by other
//! tools), then the marks of the N splats in PLY order as runs (a LEB128 run length, then the mark: 0 kept,
//! [`HIDDEN`], [`DELETED`]), then a u64 [`content_hash`] of everything before it. `picture` says whether the capture's
//! preview was drawn again with this edit (the 3D view draws it again otherwise).

use crate::gaussians::Gaussians;
use anyhow::{Context as _, bail, ensure};
use orbiters_core::reconstruction as rec;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A splat hidden: not drawn, not shared; Show all brings it back.
pub const HIDDEN: u8 = 1;
/// A splat deleted: as hidden, but only undo or Restore all brings it back.
pub const DELETED: u8 = 2;
/// Selected in the 3D view (its memory only: never written).
pub const SELECTED: u8 = 4;
/// The bits a file keeps.
const KEPT_BITS: u8 = HIDDEN | DELETED;

const MAGIC: &[u8; 8] = b"N4EDIT01";
const FORMAT: &str = "orbiters-splat-edit";
const VERSION: u32 = 1;
/// A header longer than this is not one.
const MAX_HEADER: usize = 16 << 10;
/// More splats than any reconstruction holds (training's budgets stay under ten million): a header claiming more is
/// damaged, and never gets the memory.
const MAX_SPLATS: usize = 1 << 26;
/// A PLY's bytes per splat at least (17 floats).
const PLY_SPLAT_BYTES: u64 = 68;

/// A 64-bit hash of `bytes`, eight at a time: each word is mixed in by a multiply and a shift (any change of any byte
/// changes the result with near certainty), at a few cycles a word: 25 ms for a 3-million-splat PLY of 276 MB. Not
/// cryptographic: it tells one reconstruction from another, it does not resist forgery.
pub fn content_hash(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h = 0xCBF2_9CE4_8422_2325u64 ^ (bytes.len() as u64).wrapping_mul(K);
    let (words, rest) = bytes.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(K);
        h ^= h >> 29;
    }
    let mut last = [0u8; 8];
    last[..rest.len()].copy_from_slice(rest);
    h = (h ^ u64::from_le_bytes(last) ^ (rest.len() as u64) << 59).wrapping_mul(K);
    // splitmix64's finaliser: the last words reach every bit too.
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Which PLY an edit was made on: its length and [`content_hash`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Identity {
    pub bytes: u64,
    pub hash: u64,
}

impl Identity {
    pub fn of(ply: &[u8]) -> Self {
        Self { bytes: ply.len() as u64, hash: content_hash(ply) }
    }

    /// The identity of the PLY at `path` (read whole).
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        Ok(Self::of(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?))
    }
}

#[derive(Serialize, Deserialize)]
struct PlyHeader {
    bytes: u64,
    /// [`content_hash`] as 16 hexadecimal digits (JSON numbers lose u64 precision in other readers).
    hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    format: String,
    version: u32,
    splats: usize,
    ply: PlyHeader,
    hidden: usize,
    deleted: usize,
    picture: bool,
}

/// The marks of a reconstruction's splats ([`HIDDEN`], [`DELETED`], in memory also [`SELECTED`]), one byte each in PLY
/// order (index = ID), for the PLY `ply`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub marks: Vec<u8>,
    pub ply: Identity,
    /// The capture's preview was drawn with these marks (a file says so in its header).
    pub picture: bool,
}

impl Edit {
    pub fn new(splats: usize, ply: Identity) -> Self {
        Self { marks: vec![0; splats], ply, picture: true }
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Whether splat `id` is in the outputs (neither hidden nor deleted).
    pub fn keeps(&self, id: usize) -> bool {
        self.marks.get(id).is_none_or(|m| m & KEPT_BITS == 0)
    }

    pub fn hidden(&self) -> usize {
        self.marks.iter().filter(|m| **m & HIDDEN != 0).count()
    }

    pub fn deleted(&self) -> usize {
        self.marks.iter().filter(|m| **m & DELETED != 0).count()
    }

    pub fn selected(&self) -> usize {
        self.marks.iter().filter(|m| **m & SELECTED != 0).count()
    }

    /// Splats left out of the outputs.
    pub fn removed(&self) -> usize {
        self.marks.iter().filter(|m| **m & KEPT_BITS != 0).count()
    }

    /// Whether the outputs are the PLY as trained.
    pub fn removes_nothing(&self) -> bool {
        self.marks.iter().all(|m| m & KEPT_BITS == 0)
    }

    /// Whether `other` leaves out the same splats (selections aside).
    pub fn same_outputs(&self, other: &Edit) -> bool {
        self.ply == other.ply && self.marks.len() == other.marks.len() && self.marks.iter().zip(&other.marks).all(|(a, b)| a & KEPT_BITS == b & KEPT_BITS)
    }

    /// What the GPU draws each splat as (crate::gpu::Gpu::set_marks): 0 as trained, 1 selected (tinted), 2 left out.
    pub fn view_marks(&self) -> Vec<u8> {
        self.marks.iter().map(|m| if m & KEPT_BITS != 0 { 2 } else { u8::from(m & SELECTED != 0) }).collect()
    }

    /// The file's bytes ([`SELECTED`] is not kept).
    pub fn encode(&self) -> Vec<u8> {
        let header = Header {
            format: FORMAT.into(),
            version: VERSION,
            splats: self.marks.len(),
            ply: PlyHeader { bytes: self.ply.bytes, hash: format!("{:016x}", self.ply.hash) },
            hidden: self.hidden(),
            deleted: self.deleted(),
            picture: self.picture,
        };
        let header = serde_json::to_vec(&header).expect("a header serialises");
        let mut out = Vec::with_capacity(16 + header.len() + 64);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        let mut i = 0;
        while i < self.marks.len() {
            let mark = self.marks[i] & KEPT_BITS;
            let run = self.marks[i..].iter().take_while(|m| **m & KEPT_BITS == mark).count();
            let mut n = run as u64;
            loop {
                let byte = (n & 0x7F) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(byte);
                    break;
                }
                out.push(byte | 0x80);
            }
            out.push(mark);
            i += run;
        }
        let hash = content_hash(&out);
        out.extend_from_slice(&hash.to_le_bytes());
        out
    }

    /// Reads [`Edit::encode`]'s bytes: anything else (truncated, flipped, another format or version, runs that do not
    /// add up to its splats, marks it does not know) is an error, never a partial edit.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        ensure!(bytes.len() >= 20 && &bytes[..8] == MAGIC, "not a splat edit file");
        let (body, tail) = bytes.split_at(bytes.len() - 8);
        ensure!(content_hash(body) == u64::from_le_bytes(tail.try_into().expect("8 bytes")), "the splat edit file is damaged");
        let length = u32::from_le_bytes(body[8..12].try_into().expect("4 bytes")) as usize;
        ensure!(length <= MAX_HEADER && 12 + length <= body.len(), "the splat edit file is damaged");
        let header: Header = serde_json::from_slice(&body[12..12 + length]).context("the splat edit file's header")?;
        ensure!(header.format == FORMAT, "not a splat edit file");
        ensure!(header.version == VERSION, "splat edits of version {} (this app reads version {VERSION}): update the app", header.version);
        let hash = u64::from_str_radix(&header.ply.hash, 16).context("the splat edit file's PLY hash")?;
        // A header may claim any count: never more than a PLY of its size holds, nor than any reconstruction has.
        ensure!(header.splats <= MAX_SPLATS && header.splats as u64 <= header.ply.bytes / PLY_SPLAT_BYTES, "the splat edit file is damaged");
        let mut marks = Vec::with_capacity(header.splats);
        let mut at = 12 + length;
        while at < body.len() {
            let mut n = 0u64;
            let mut shift = 0;
            loop {
                let byte = *body.get(at).context("the splat edit file is damaged")?;
                at += 1;
                ensure!(shift < 63, "the splat edit file is damaged");
                n |= ((byte & 0x7F) as u64) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            let mark = *body.get(at).context("the splat edit file is damaged")?;
            at += 1;
            ensure!(mark == 0 || mark == HIDDEN || mark == DELETED, "the splat edit file has a mark it does not know ({mark})");
            ensure!(n > 0 && marks.len() as u64 + n <= header.splats as u64, "the splat edit file is damaged");
            marks.resize(marks.len() + n as usize, mark);
        }
        ensure!(marks.len() == header.splats, "the splat edit file is damaged");
        Ok(Self { marks, ply: Identity { bytes: header.ply.bytes, hash }, picture: header.picture })
    }

    /// Writes the edit beside the PLY in `result` (atomically), or removes the file when it leaves nothing out.
    pub fn save(&self, result: &Path) -> anyhow::Result<()> {
        let path = result.join(rec::EDIT_FILE);
        if self.removes_nothing() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).with_context(|| format!("removing {}", path.display())),
                _ => return Ok(()),
            }
        }
        orbiters_core::write_atomic(&path, &self.encode()).with_context(|| format!("writing {}", path.display()))
    }
}

/// The edit beside a PLY, as found.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// None: the PLY as trained.
    None,
    /// Made on this PLY: applied.
    Current(Edit),
    /// Made on another PLY (an earlier reconstruction): never applied.
    Stale(Edit),
    /// Not readable (damaged, another version): why.
    Unreadable(String),
}

impl Found {
    /// What stops the outputs, as the person reads it.
    pub fn problem(&self) -> Option<String> {
        match self {
            Found::Stale(_) => Some("Its splat edits were made on an earlier reconstruction of it: open its 3D view to discard them.".into()),
            Found::Unreadable(e) => Some(format!("Its splat edits could not be read ({e}): open its 3D view to discard them.")),
            _ => None,
        }
    }
}

/// The edit file beside the PLY at `ply`.
pub fn path_beside(ply: &Path) -> PathBuf {
    ply.with_file_name(rec::EDIT_FILE)
}

/// The edit beside the PLY at `ply` whose identity is `identity` and which holds `splats` splats.
pub fn find(ply: &Path, identity: Identity, splats: usize) -> Found {
    let path = path_beside(ply);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Found::None,
        Err(e) => return Found::Unreadable(e.to_string()),
    };
    match Edit::decode(&bytes) {
        Ok(edit) if edit.ply == identity && edit.len() == splats => Found::Current(edit),
        Ok(edit) => Found::Stale(edit),
        Err(e) => Found::Unreadable(format!("{e:#}")),
    }
}

/// The PLY at `ply` whole, its identity and its edit: what the 3D view loads (it draws the edit's marks).
pub fn read_for_view(ply: &Path) -> anyhow::Result<(Gaussians, Identity, Found)> {
    let bytes = std::fs::read(ply).with_context(|| format!("reading {}", ply.display()))?;
    let identity = Identity::of(&bytes);
    let g = crate::ply::parse(&bytes)?;
    let found = find(ply, identity, g.len());
    Ok((g, identity, found))
}

/// The splats every output is made from: the PLY at `ply` without what its edit hides or deletes, and how many that
/// left out. An edit that cannot be applied stops it ([`Found::problem`]).
pub fn read_edited(ply: &Path) -> anyhow::Result<(Gaussians, usize)> {
    let (mut g, _, found) = read_for_view(ply)?;
    if let Some(problem) = found.problem() {
        bail!("{problem}");
    }
    let Found::Current(edit) = found else { return Ok((g, 0)) };
    let removed = apply(&mut g, &edit);
    Ok((g, removed))
}

/// Leaves out of `g` (in PLY order) what `edit` hides or deletes; returns how many.
pub fn apply(g: &mut Gaussians, edit: &Edit) -> usize {
    assert_eq!(g.len(), edit.len(), "an edit of another set of splats");
    let keep: Vec<bool> = (0..g.len()).map(|i| edit.keeps(i)).collect();
    let removed = keep.iter().filter(|k| !**k).count();
    if removed > 0 {
        g.retain(&keep);
    }
    removed
}

/// One step of undo: the splats it changed, their marks before and after.
#[derive(Clone, Debug, Default)]
struct Change {
    ids: Vec<u32>,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl Change {
    fn bytes(&self) -> usize {
        self.ids.len() * 6
    }
}

/// Undo and redo of an edit's marks (selections too), as the changes each step made: an invert of three million
/// splats is 18 MB, so the steps kept together stay under [`History::MAX_BYTES`] (the oldest go first). Between
/// [`History::begin`] and [`History::end`] every change is one step (a brush stroke's dabs).
#[derive(Default)]
pub struct History {
    past: Vec<Change>,
    future: Vec<Change>,
    /// The step under way: where each splat it changed is in it, and whether it is in `past` yet.
    open: Option<(std::collections::HashMap<u32, usize>, bool)>,
}

impl History {
    const MAX_BYTES: usize = 256 << 20;

    /// Starts a step that every change joins until [`History::end`].
    pub fn begin(&mut self) {
        self.end();
        self.open = Some((Default::default(), false));
    }

    pub fn end(&mut self) {
        self.open = None;
    }

    /// Applies `marks` (the new mark of each splat `marks` names) to `edit` as one step (or into the step begun);
    /// false when nothing changed.
    pub fn apply(&mut self, edit: &mut Edit, marks: impl IntoIterator<Item = (u32, u8)>) -> bool {
        if let Some((index, started)) = self.open.as_mut() {
            let mut changed = false;
            for (id, mark) in marks {
                let Some(m) = edit.marks.get_mut(id as usize) else { continue };
                if *m == mark {
                    continue;
                }
                if !*started {
                    *started = true;
                    self.future.clear();
                    self.past.push(Change::default());
                }
                let c = self.past.last_mut().expect("the step begun");
                match index.get(&id) {
                    Some(&k) => c.after[k] = mark,
                    None => {
                        index.insert(id, c.ids.len());
                        c.ids.push(id);
                        c.before.push(*m);
                        c.after.push(mark);
                    }
                }
                *m = mark;
                changed = true;
            }
            return changed;
        }
        let mut change = Change::default();
        for (id, mark) in marks {
            let Some(m) = edit.marks.get_mut(id as usize) else { continue };
            if *m != mark {
                change.ids.push(id);
                change.before.push(*m);
                change.after.push(mark);
                *m = mark;
            }
        }
        if change.ids.is_empty() {
            return false;
        }
        self.future.clear();
        self.past.push(change);
        let mut total: usize = self.past.iter().map(Change::bytes).sum();
        while total > Self::MAX_BYTES && self.past.len() > 1 {
            total -= self.past.remove(0).bytes();
        }
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn undo(&mut self, edit: &mut Edit) -> bool {
        self.end();
        let Some(c) = self.past.pop() else { return false };
        for (&id, &m) in c.ids.iter().zip(&c.before) {
            edit.marks[id as usize] = m;
        }
        self.future.push(c);
        true
    }

    pub fn redo(&mut self, edit: &mut Edit) -> bool {
        self.end();
        let Some(c) = self.future.pop() else { return false };
        for (&id, &m) in c.ids.iter().zip(&c.after) {
            edit.marks[id as usize] = m;
        }
        self.past.push(c);
        true
    }
}

/// The edits of the reconstruction in `result` with `ply` left out: what the 3D view's Restore all does (the file
/// goes, and the trained preview comes back).
pub fn discard(result: &Path) -> anyhow::Result<()> {
    let path = result.join(rec::EDIT_FILE);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).with_context(|| format!("removing {}", path.display())),
        _ => {}
    }
    restore_picture(result)
}

/// Puts `qoi` (a render of the preview's view with the edits applied) in place of the preview, keeping the worker's
/// own render aside the first time ([`rec::TRAINED_PREVIEW_FILE`]).
pub fn write_picture(result: &Path, qoi: &[u8]) -> anyhow::Result<()> {
    let preview = result.join(rec::PREVIEW_FILE);
    let trained = result.join(rec::TRAINED_PREVIEW_FILE);
    if !trained.is_file() && preview.is_file() {
        std::fs::copy(&preview, &trained).with_context(|| format!("keeping {}", trained.display()))?;
    }
    orbiters_core::write_atomic(&preview, qoi).with_context(|| format!("writing {}", preview.display()))
}

/// The worker's own preview back in its place, once nothing is edited.
pub fn restore_picture(result: &Path) -> anyhow::Result<()> {
    let trained = result.join(rec::TRAINED_PREVIEW_FILE);
    if !trained.is_file() {
        return Ok(());
    }
    let bytes = std::fs::read(&trained)?;
    orbiters_core::write_atomic(&result.join(rec::PREVIEW_FILE), &bytes)?;
    std::fs::remove_file(&trained)?;
    Ok(())
}

/// The camera the worker drew the preview from (crate's `main`: the first held-out keyframe, else the first keyframe of
/// the session's size), at the training's size as the report gives it; None when the records do not say.
pub fn preview_camera(session: &Path) -> Option<crate::camera::Camera> {
    use orbiters_core::contracts::frame_record::record_flags;
    let report = rec::read_report(session)?;
    let records = orbiters_core::bundle::read_records(session).ok()?;
    let keyframes: Vec<_> =
        records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0 && r.width > 0).collect();
    let fits = |r: &&&orbiters_core::contracts::frame_record::FrameRecord| {
        let f = r.width / report.width.max(1);
        f >= 1 && r.width / f == report.width && r.height / f == report.height
    };
    let record = match report.held_out_keyframes.first() {
        Some(&k) => keyframes.iter().find(|r| r.keyframe_index == k).filter(fits)?,
        None => keyframes.iter().find(fits)?,
    };
    let factor = record.width / report.width;
    Some(crate::camera::Geometry::new(record.width, record.height, factor).camera(&record.camera_to_world, &record.projection))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-edit-{name}-{}-{}", std::process::id(), fastrand::u32(..)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_hash_sees_every_byte() {
        let mut bytes: Vec<u8> = (0..1003u32).map(|i| (i * 7) as u8).collect();
        let h = content_hash(&bytes);
        for at in [0, 1, 7, 8, 500, 999, 1002] {
            bytes[at] ^= 1;
            assert_ne!(content_hash(&bytes), h, "byte {at}");
            bytes[at] ^= 1;
        }
        assert_ne!(content_hash(&bytes[..1002]), h, "its length");
        assert_eq!(content_hash(&bytes), h);
    }

    /// Runs of every length, sparse and dense marks, the selection left out, and damage refused whole.
    #[test]
    fn an_edit_reads_back_as_written_and_damage_is_refused() {
        let ply = Identity { bytes: 1 << 20, hash: 0xDEAD_BEEF_0123_4567 };
        let mut edit = Edit::new(5000, ply);
        for i in (0..5000).step_by(7) {
            edit.marks[i] = HIDDEN;
        }
        for m in &mut edit.marks[1000..3000] {
            *m = DELETED;
        }
        edit.marks[4000] = SELECTED;
        edit.marks[4001] = HIDDEN | SELECTED;
        edit.picture = false;
        let bytes = edit.encode();
        assert!(bytes.len() < 2000, "{} bytes for 5000 splats", bytes.len());
        let back = Edit::decode(&bytes).unwrap();
        assert_eq!(back.ply, ply);
        assert!(!back.picture);
        assert_eq!(back.marks[4000], 0, "a selection is not kept");
        assert_eq!(back.marks[4001], HIDDEN);
        assert!(back.same_outputs(&edit));
        assert_eq!((back.hidden(), back.deleted()), (edit.hidden(), edit.deleted()));
        for at in [0, 9, 14, bytes.len() / 2, bytes.len() - 9, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[at] ^= 0x10;
            assert!(Edit::decode(&bad).is_err(), "byte {at} flipped");
        }
        assert!(Edit::decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(Edit::decode(b"N4EDIT01").is_err());
        // The header is plain JSON other tools read.
        let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&bytes[12..12 + length]).unwrap();
        assert_eq!(header["format"], "orbiters-splat-edit");
        assert_eq!(header["ply"]["hash"], "deadbeef01234567");
        assert_eq!(header["deleted"], 2000);
    }

    #[test]
    fn undo_and_redo_walk_the_steps_and_a_new_step_drops_what_was_undone() {
        let mut edit = Edit::new(10, Identity::default());
        let mut h = History::default();
        assert!(h.apply(&mut edit, [(1, SELECTED), (2, SELECTED)]));
        assert!(!h.apply(&mut edit, [(1, SELECTED)]), "nothing changed: no step");
        assert!(h.apply(&mut edit, [(1, DELETED), (2, DELETED), (99, DELETED)]), "an ID past the end is passed over");
        assert_eq!(edit.removed(), 2);
        assert!(h.undo(&mut edit));
        assert_eq!((edit.removed(), edit.selected()), (0, 2));
        assert!(h.undo(&mut edit));
        assert_eq!(edit.selected(), 0);
        assert!(!h.undo(&mut edit));
        assert!(h.redo(&mut edit));
        assert_eq!(edit.selected(), 2);
        assert!(h.apply(&mut edit, [(5, HIDDEN)]));
        assert!(!h.can_redo(), "a new step after undo drops the redo");
        assert_eq!(edit.view_marks(), vec![0, 1, 1, 0, 0, 2, 0, 0, 0, 0]);
        // A stroke: its dabs, overlapping, are one step; an empty one is none.
        h.begin();
        h.end();
        h.begin();
        assert!(h.apply(&mut edit, [(7, SELECTED), (8, SELECTED)]));
        assert!(h.apply(&mut edit, [(8, SELECTED), (9, SELECTED)]));
        assert!(h.apply(&mut edit, [(7, 0)]));
        h.end();
        assert_eq!(&edit.marks[7..], &[0, SELECTED, SELECTED]);
        assert!(h.undo(&mut edit));
        assert_eq!(&edit.marks[5..], &[HIDDEN, 0, 0, 0, 0], "the stroke undone whole");
        assert!(h.undo(&mut edit));
        assert_eq!(edit.marks[5], 0, "then the step before it");
        h.redo(&mut edit);
        h.redo(&mut edit);
        assert_eq!(&edit.marks[5..], &[HIDDEN, 0, 0, SELECTED, SELECTED]);
    }

    /// The whole file round: written beside a PLY, found and applied for that PLY only, set aside when the PLY is
    /// replaced, refused when damaged, and gone (with the trained preview back) when nothing is left out.
    #[test]
    fn an_edit_applies_to_its_own_ply_only() {
        let dir = temp("apply");
        let mut g = Gaussians::default();
        for i in 0..10 {
            g.push([i as f32, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        }
        let ply = dir.join(rec::PLY_FILE);
        crate::ply::write(&ply, &g).unwrap();
        let (_, identity, found) = read_for_view(&ply).unwrap();
        assert_eq!(found, Found::None);
        assert_eq!(read_edited(&ply).unwrap().1, 0);
        let mut edit = Edit::new(10, identity);
        edit.marks[3] = HIDDEN;
        edit.marks[7] = DELETED;
        edit.save(&dir).unwrap();
        let (kept, removed) = read_edited(&ply).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(kept.means.iter().map(|m| m[0] as usize).collect::<Vec<_>>(), vec![0, 1, 2, 4, 5, 6, 8, 9], "in PLY order");
        // A new reconstruction (another PLY): the edit is never applied, and the outputs stop.
        g.means[0][1] = 0.5;
        crate::ply::write(&ply, &g).unwrap();
        assert!(matches!(read_for_view(&ply).unwrap().2, Found::Stale(_)));
        let e = read_edited(&ply).unwrap_err().to_string();
        assert!(e.contains("earlier reconstruction"), "{e}");
        // Damaged: refused too.
        std::fs::write(path_beside(&ply), b"N4EDIT01 not really").unwrap();
        assert!(matches!(read_for_view(&ply).unwrap().2, Found::Unreadable(_)));
        assert!(read_edited(&ply).is_err());
        // Nothing left out: the file goes.
        let identity = Identity::read(&ply).unwrap();
        Edit::new(10, identity).save(&dir).unwrap();
        assert!(!path_beside(&ply).exists());
        assert_eq!(read_edited(&ply).unwrap().1, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A grey floor kept; a magenta "leftover avatar" off to one side hidden, magenta floaters above deleted.
    fn edited_scene(dir: &Path) -> (PathBuf, Gaussians) {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(11);
        for _ in 0..4000 {
            g.push([rng.f32() * 10.0, 0.0, rng.f32() * 10.0], [-3.0, -6.0, -3.0], [1.0, 0.0, 0.0, 0.0], 2.0, [0.5; 3]);
        }
        for _ in 0..300 {
            g.push([40.0 + rng.f32(), rng.f32() * 1.7, 5.0 + rng.f32()], [-3.5; 3], [1.0, 0.0, 0.0, 0.0], 2.0, [1.0, 0.0, 1.0]);
        }
        for _ in 0..300 {
            g.push([rng.f32() * 10.0, 8.0 + rng.f32(), rng.f32() * 10.0], [-2.5; 3], [1.0, 0.0, 0.0, 0.0], 2.0, [1.0, 0.0, 1.0]);
        }
        let ply = dir.join(rec::PLY_FILE);
        crate::ply::write(&ply, &g).unwrap();
        (ply, g)
    }

    /// Whether `g` holds any of the scene's removed splats, or anything merged from them: a splat near the avatar or
    /// the floaters, or one not grey.
    fn holds_removed(g: &Gaussians) -> bool {
        (0..g.len()).any(|i| g.means[i][0] > 20.0 || g.means[i][1] > 4.0 || (g.colors[i][0] - g.colors[i][1]).abs() > 0.02)
    }

    /// The whole promise of an edit: what it hides or deletes is in none of the files made for sharing (the VRChat
    /// trees for PC, Quest and the coarse first load, their merged boxes included; the web preview the server makes from
    /// the PC tree's leaves; `--web-preview`'s), and every splat it keeps is. Without the edit the same scene's files do
    /// hold the removed splats: the check sees them when they are there.
    #[test]
    fn hidden_and_deleted_splats_leave_every_output() {
        let dir = temp("outputs");
        let (ply, g) = edited_scene(&dir);
        let outputs = |g: &Gaussians| {
            let files = crate::tree_tune::vrchat_files(g, &[], None, 7, |_| {}).unwrap();
            let trees: Vec<_> = [&files.pc, &files.quest, &files.coarse].map(|f| crate::tree::Tree::parse(f).unwrap()).into();
            let leaves = trees[0].leaf_splats();
            let mut web = vec![crate::web::read_splat(&crate::web::splat_file(&leaves, crate::web::DEFAULT_MAX, [5.0, 1.6, 5.0]).bytes).unwrap()];
            web.push(crate::web::read_splat(&crate::web::splat_file(g, crate::web::DEFAULT_MAX, [5.0, 1.6, 5.0]).bytes).unwrap());
            (trees, leaves, web)
        };
        // As trained: the avatar and the floaters are in every file.
        let (trees, _, web) = outputs(&g);
        assert!(trees.iter().all(|t| holds_removed(&t.splats)) && web.iter().all(holds_removed));
        let (_, identity, _) = read_for_view(&ply).unwrap();
        let mut edit = Edit::new(g.len(), identity);
        for m in &mut edit.marks[4000..4300] {
            *m = HIDDEN;
        }
        for m in &mut edit.marks[4300..] {
            *m = DELETED;
        }
        // A selection is no removal: the first floor splats stay.
        edit.marks[0] = SELECTED;
        edit.save(&dir).unwrap();
        let (kept, removed) = read_edited(&ply).unwrap();
        assert_eq!((kept.len(), removed), (4000, 600));
        let (trees, leaves, web) = outputs(&kept);
        for (t, name) in trees.iter().zip(["PC", "Quest", "coarse"]) {
            assert!(!holds_removed(&t.splats), "the {name} tree holds a removed splat");
            assert!(t.nodes.iter().all(|n| n.center[0] + n.radius < 20.0), "a {name} box reaches the removed avatar");
        }
        assert_eq!(leaves.len(), 4000, "every kept splat is a PC leaf");
        assert!(web.iter().all(|w| !holds_removed(w) && w.len() == 4000));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_trained_preview_waits_aside_while_the_edited_one_shows() {
        let dir = temp("picture");
        std::fs::write(dir.join(rec::PREVIEW_FILE), b"trained").unwrap();
        write_picture(&dir, b"edited 1").unwrap();
        write_picture(&dir, b"edited 2").unwrap();
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), b"edited 2");
        assert_eq!(std::fs::read(dir.join(rec::TRAINED_PREVIEW_FILE)).unwrap(), b"trained", "the worker's, not an edited one");
        let mut edit = Edit::new(4, Identity::default());
        edit.marks[0] = DELETED;
        edit.save(&dir).unwrap();
        discard(&dir).unwrap();
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), b"trained");
        assert!(!dir.join(rec::TRAINED_PREVIEW_FILE).exists() && !dir.join(rec::EDIT_FILE).exists());
        discard(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
