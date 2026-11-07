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
use orbiters_i18n::tr;
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
        Ok(Self::of(&std::fs::read(path).with_context(|| tr!("reading {path}", path = path.display()))?))
    }
}

#[derive(Serialize, Deserialize)]
struct PlyHeader {
    bytes: u64,
    /// [`content_hash`] as 16 hexadecimal digits (JSON numbers lose u64 precision in other readers).
    hash: String,
    /// The PLY's modification time when the edit was saved (nanoseconds since 1970, 0 unknown): a reader that only needs
    /// to know whether the PLY is still the one (the capture's picture) hashes it only when this or its length differs.
    #[serde(default)]
    modified: u64,
}

/// A file's modification time in nanoseconds since 1970 (0 when unknown).
fn modified_ns(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos().min(u64::MAX as u128) as u64)
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
    /// The PLY's modification time when last saved ([`PlyHeader::modified`]; 0 unknown).
    pub ply_modified: u64,
}

impl Edit {
    pub fn new(splats: usize, ply: Identity) -> Self {
        Self { marks: vec![0; splats], ply, picture: true, ply_modified: 0 }
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

    /// What leaves the outputs, as one word ([`generation`]): [`UNEDITED`] when nothing does, else the PLY's hash and
    /// a hash of which splats it removes. Hidden or deleted, the picture flag and the selection do not change it.
    pub fn generation(&self) -> String {
        if self.removes_nothing() {
            return UNEDITED.into();
        }
        let removed: Vec<u8> = self.marks.iter().map(|m| u8::from(m & KEPT_BITS != 0)).collect();
        format!("{:016x}-{:016x}", self.ply.hash, content_hash(&removed))
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
            ply: PlyHeader { bytes: self.ply.bytes, hash: format!("{:016x}", self.ply.hash), modified: self.ply_modified },
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
        ensure!(bytes.len() >= 20 && &bytes[..8] == MAGIC, "{}", tr("not a splat edit file"));
        let (body, tail) = bytes.split_at(bytes.len() - 8);
        ensure!(content_hash(body) == u64::from_le_bytes(tail.try_into().expect("8 bytes")), "{}", tr("the splat edit file is damaged"));
        let length = u32::from_le_bytes(body[8..12].try_into().expect("4 bytes")) as usize;
        ensure!(length <= MAX_HEADER && 12 + length <= body.len(), "{}", tr("the splat edit file is damaged"));
        let header: Header = serde_json::from_slice(&body[12..12 + length]).with_context(|| tr("the splat edit file's header"))?;
        ensure!(header.format == FORMAT, "{}", tr("not a splat edit file"));
        ensure!(header.version == VERSION, "{}", tr!("splat edits of version {found} (this app reads version {expected}): update the app", found = header.version, expected = VERSION));
        let hash = u64::from_str_radix(&header.ply.hash, 16).with_context(|| tr("the splat edit file's PLY hash"))?;
        // A header may claim any count: never more than a PLY of its size holds, nor than any reconstruction has.
        ensure!(header.splats <= MAX_SPLATS && header.splats as u64 <= header.ply.bytes / PLY_SPLAT_BYTES, "{}", tr("the splat edit file is damaged"));
        let mut marks = Vec::with_capacity(header.splats);
        let mut at = 12 + length;
        while at < body.len() {
            let mut n = 0u64;
            let mut shift = 0;
            loop {
                let byte = *body.get(at).with_context(|| tr("the splat edit file is damaged"))?;
                at += 1;
                ensure!(shift < 63, "{}", tr("the splat edit file is damaged"));
                n |= ((byte & 0x7F) as u64) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            let mark = *body.get(at).with_context(|| tr("the splat edit file is damaged"))?;
            at += 1;
            ensure!(mark == 0 || mark == HIDDEN || mark == DELETED, "{}", tr!("the splat edit file has a mark it does not know ({mark})", mark = mark));
            ensure!(n > 0 && marks.len() as u64 + n <= header.splats as u64, "{}", tr("the splat edit file is damaged"));
            marks.resize(marks.len() + n as usize, mark);
        }
        ensure!(marks.len() == header.splats, "{}", tr("the splat edit file is damaged"));
        Ok(Self { marks, ply: Identity { bytes: header.ply.bytes, hash }, picture: header.picture, ply_modified: header.ply.modified })
    }

    /// Writes the edit beside the PLY in `result` (atomically), or removes the file when it leaves nothing out.
    pub fn save(&self, result: &Path) -> anyhow::Result<()> {
        let path = result.join(rec::EDIT_FILE);
        if self.removes_nothing() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).with_context(|| tr!("removing {path}", path = path.display())),
                _ => return Ok(()),
            }
        }
        let stamped = Edit { ply_modified: modified_ns(&result.join(rec::PLY_FILE)), ..self.clone() };
        orbiters_core::write_atomic(&path, &stamped.encode()).with_context(|| tr!("writing {path}", path = path.display()))
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
            Found::Stale(_) => Some(tr("Its splat edits were made on an earlier reconstruction of it: open its 3D view to discard them.").into()),
            Found::Unreadable(e) => Some(tr!("Its splat edits could not be read ({error}): open its 3D view to discard them.", error = e)),
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
    let bytes = std::fs::read(ply).with_context(|| tr!("reading {path}", path = ply.display()))?;
    parse_for_view(ply, &bytes)
}

/// [`read_for_view`] of the PLY at `ply` already read whole into `bytes` (the desktop's export reads it in steps, to
/// show how far it got).
pub fn parse_for_view(ply: &Path, bytes: &[u8]) -> anyhow::Result<(Gaussians, Identity, Found)> {
    let identity = Identity::of(bytes);
    let g = crate::ply::parse(bytes)?;
    let found = find(ply, identity, g.len());
    Ok((g, identity, found))
}

/// The splats every output is made from: the PLY at `ply` without what its edit hides or deletes, how many that left
/// out, and the [`generation`] applied (what an upload records, and what its share checks again). An edit that cannot
/// be applied stops it ([`Found::problem`]).
pub fn read_edited(ply: &Path) -> anyhow::Result<(Gaussians, usize, String)> {
    let (mut g, _, found) = read_for_view(ply)?;
    if let Some(problem) = found.problem() {
        bail!("{problem}");
    }
    let Found::Current(edit) = found else { return Ok((g, 0, UNEDITED.into())) };
    let removed = apply(&mut g, &edit);
    Ok((g, removed, edit.generation()))
}

/// The [`generation`] of a reconstruction with no splat edits.
pub const UNEDITED: &str = "unedited";

/// What the outputs of the reconstruction in `result` leave out now, as one word ([`Edit::generation`]), from its edit
/// file alone (no PLY read: cheap enough to ask before and after each step of a share). An edit file that cannot be
/// read is an error (the outputs stop). It tells two states of the edits apart: whether they apply to the PLY there is
/// for [`read_edited`] to say.
pub fn generation(result: &Path) -> anyhow::Result<String> {
    let path = result.join(rec::EDIT_FILE);
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UNEDITED.into()),
        Err(e) => Err(e).with_context(|| tr!("Its splat edits could not be read ({path})", path = path.display())),
        Ok(bytes) => {
            Ok(Edit::decode(&bytes).map_err(|e| anyhow::anyhow!("{}", tr!("Its splat edits could not be read ({error}): open its 3D view to discard them.", error = format!("{e:#}"))))?.generation())
        }
    }
}

/// Whether the capture's picture (`preview.qoi`) may leave the app: what it shows against its splat edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PictureState {
    /// No edits: the picture as the worker drew it (or the recording's thumbnail without one).
    Unedited,
    /// Drawn again for the edits there are now.
    Drawn,
    /// Edited since it was drawn (or its redraw failed): it may show what was removed.
    NotDrawn,
    /// Edits that cannot be applied (damaged, or made on another PLY): why.
    Problem(String),
}

impl PictureState {
    /// Whether the preview may be shown outside the app (uploaded, dragged out, listed for a world).
    pub fn preview_ok(&self) -> bool {
        matches!(self, PictureState::Unedited | PictureState::Drawn)
    }

    /// Whether the recording's own thumbnail may stand in for a missing preview: only with no edits (it shows the
    /// capture as recorded, removed avatars and all).
    pub fn thumbnail_ok(&self) -> bool {
        *self == PictureState::Unedited
    }
}

/// [`PictureState`] of the reconstruction in `result`: the edit file's flag, once it is known to be the PLY's (its
/// length and modification time as saved; the PLY hashed again only when the time differs).
pub fn picture_state(result: &Path) -> PictureState {
    let path = result.join(rec::EDIT_FILE);
    let bytes = match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PictureState::Unedited,
        Err(e) => return PictureState::Problem(e.to_string()),
        Ok(b) => b,
    };
    let edit = match Edit::decode(&bytes) {
        Ok(e) => e,
        Err(e) => return PictureState::Problem(format!("{e:#}")),
    };
    let ply = result.join(rec::PLY_FILE);
    let len = std::fs::metadata(&ply).map_or(0, |m| m.len());
    let stale = || PictureState::Problem("made on an earlier reconstruction".into());
    if len != edit.ply.bytes {
        return stale();
    }
    if edit.ply_modified == 0 || modified_ns(&ply) != edit.ply_modified {
        match Identity::read(&ply) {
            Ok(id) if id == edit.ply => {}
            Ok(_) => return stale(),
            Err(e) => return PictureState::Problem(format!("{e:#}")),
        }
    }
    if edit.removes_nothing() {
        PictureState::Unedited
    } else if edit.picture {
        PictureState::Drawn
    } else {
        PictureState::NotDrawn
    }
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
    /// The memory it holds: its vectors' capacities, not their lengths (a stroke's vectors grow by doubling).
    fn bytes(&self) -> usize {
        self.ids.capacity() * 4 + self.before.capacity() + self.after.capacity()
    }

    fn shrink(&mut self) {
        self.ids.shrink_to_fit();
        self.before.shrink_to_fit();
        self.after.shrink_to_fit();
    }
}

/// Undo and redo of an edit's marks (selections too), as the changes each step made (six bytes a splat changed: an
/// invert of three million splats is 18 MB). Between [`History::begin`] and [`History::end`] every change joins one step
/// (a brush stroke's dabs). Everything kept (the steps to undo and to redo, and the stroke under way with its index)
/// stays under [`History::MAX_BYTES`], counted by what the vectors hold allocated: the oldest steps go first, then the
/// furthest redo; the newest completed step and a stroke under way always stay.
pub struct History {
    past: Vec<Change>,
    future: Vec<Change>,
    /// The step under way: where each splat it changed is in it, and whether it is in `past` yet.
    open: Option<(std::collections::HashMap<u32, usize>, bool)>,
    budget: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::with_budget(Self::MAX_BYTES)
    }
}

impl History {
    pub const MAX_BYTES: usize = 256 << 20;

    /// A history kept under `budget` bytes (tests use small ones).
    pub fn with_budget(budget: usize) -> Self {
        Self { past: Vec::new(), future: Vec::new(), open: None, budget }
    }

    /// The bytes it holds now: its steps' vectors and the open stroke's index (about 16 bytes an entry).
    pub fn memory(&self) -> usize {
        let index = self.open.as_ref().map_or(0, |(i, _)| i.capacity() * 16);
        self.past.iter().chain(&self.future).map(Change::bytes).sum::<usize>() + index
    }

    /// Starts a step that every change joins until [`History::end`] (one already under way ends first).
    pub fn begin(&mut self) {
        self.end();
        self.open = Some((Default::default(), false));
    }

    /// Ends the step under way, if any: what it changed stays one step of undo.
    pub fn end(&mut self) {
        if let Some((_, true)) = self.open.take() {
            if let Some(c) = self.past.last_mut() {
                c.shrink();
            }
        }
        self.trim();
    }

    /// Whether a step is under way.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Drops the oldest steps, then the furthest redo, while it holds more than its budget.
    fn trim(&mut self) {
        let mut total = self.memory();
        let keep = 1 + usize::from(self.open.as_ref().is_some_and(|o| o.1));
        while total > self.budget && self.past.len() > keep {
            total -= self.past.remove(0).bytes();
        }
        while total > self.budget && !self.future.is_empty() {
            total -= self.future.remove(0).bytes();
        }
    }

    /// Applies `marks` (the new mark of each splat `marks` names) to `edit` as one step (or into the step begun);
    /// false when nothing changed.
    pub fn apply(&mut self, edit: &mut Edit, marks: impl IntoIterator<Item = (u32, u8)>) -> bool {
        if let Some((index, started)) = self.open.as_mut() {
            let mut changed = false;
            let mut began = false;
            for (id, mark) in marks {
                let Some(m) = edit.marks.get_mut(id as usize) else { continue };
                if *m == mark {
                    continue;
                }
                if !*started {
                    *started = true;
                    began = true;
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
            if began {
                self.trim();
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
        change.shrink();
        self.future.clear();
        self.past.push(change);
        self.trim();
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

/// The edits of the reconstruction in `result` left out: what Discard them does (the file goes, and the worker's own
/// picture of this PLY comes back).
pub fn discard(result: &Path) -> anyhow::Result<()> {
    let identity = Identity::read(&result.join(rec::PLY_FILE)).ok();
    discard_for(result, identity)
}

/// [`discard`] knowing the identity of the PLY there (None: no PLY, no picture to put back).
pub fn discard_for(result: &Path, ply: Option<Identity>) -> anyhow::Result<()> {
    let path = result.join(rec::EDIT_FILE);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).with_context(|| tr!("removing {path}", path = path.display())),
        _ => {}
    }
    restore_picture(result, ply)
}

/// The most pixels a picture backed up or put back may have (4K): a file claiming more is damaged.
const PICTURE_PIXELS: u64 = 3840 * 2160;

/// Whether `bytes` are a whole QOI picture.
fn whole_picture(bytes: &[u8]) -> bool {
    orbiters_contracts::qoi::decode_rgba_max(bytes, PICTURE_PIXELS).is_ok()
}

/// Puts `qoi` (a render of the preview's view with the edits applied) in place of the preview of the PLY `ply`,
/// keeping the worker's own render aside the first time, under that PLY's name
/// (orbiters_core::reconstruction::trained_preview_file): written whole and checked before the preview is replaced,
/// so a backup cut short (a crash, a full disk) is never taken for one. Backups of other PLYs (an earlier
/// reconstruction's) go.
pub fn write_picture(result: &Path, ply: Identity, qoi: &[u8]) -> anyhow::Result<()> {
    let preview = result.join(rec::PREVIEW_FILE);
    let trained = result.join(rec::trained_preview_file(ply.hash));
    let kept = std::fs::read(&trained).ok().filter(|b| whole_picture(b));
    if kept.is_none() {
        if let Ok(bytes) = std::fs::read(&preview) {
            ensure!(whole_picture(&bytes), "{}", tr!("{path} is damaged: nothing to keep aside", path = preview.display()));
            orbiters_core::write_atomic(&trained, &bytes).with_context(|| tr!("keeping {path}", path = trained.display()))?;
            ensure!(std::fs::read(&trained).is_ok_and(|b| b == bytes), "{}", tr!("{path} was not kept whole", path = trained.display()));
        }
    }
    remove_other_backups(result, Some(ply));
    orbiters_core::write_atomic(&preview, qoi).with_context(|| tr!("writing {path}", path = preview.display()))
}

/// The worker's own picture of the PLY `ply` back in its place once nothing is edited (a damaged backup is refused,
/// the picture there staying), and backups of other PLYs gone.
pub fn restore_picture(result: &Path, ply: Option<Identity>) -> anyhow::Result<()> {
    remove_other_backups(result, ply);
    let Some(ply) = ply else { return Ok(()) };
    let trained = result.join(rec::trained_preview_file(ply.hash));
    let bytes = match std::fs::read(&trained) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        r => r.with_context(|| tr!("reading {path}", path = trained.display()))?,
    };
    if !whole_picture(&bytes) {
        let _ = std::fs::remove_file(&trained);
        bail!("{}", tr!("the worker's picture kept aside ({path}) was damaged: the picture there stays", path = trained.display()));
    }
    orbiters_core::write_atomic(&result.join(rec::PREVIEW_FILE), &bytes)?;
    std::fs::remove_file(&trained)?;
    Ok(())
}

/// Removes the pictures kept aside for PLYs other than `ply` (every one with None).
fn remove_other_backups(result: &Path, ply: Option<Identity>) {
    let mine = ply.map(|p| rec::trained_preview_file(p.hash));
    for e in std::fs::read_dir(result).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if rec::is_trained_preview_file(&name) && Some(&name) != mine.as_ref() {
            let _ = std::fs::remove_file(e.path());
        }
    }
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

    /// What stops an upload or an export reads in the window's language, with the reason put in its place.
    #[test]
    fn an_edit_problem_is_reported_in_the_language_of_the_window() {
        use orbiters_i18n::{in_language, Language};
        let unreadable = Found::Unreadable("x".into());
        assert_eq!(unreadable.problem().unwrap(), "Its splat edits could not be read (x): open its 3D view to discard them.");
        assert_eq!(
            in_language(Language::Es, || unreadable.problem().unwrap()),
            "No se pudieron leer sus ediciones de splats (x): abre su vista 3D para descartarlas."
        );
        assert_eq!(in_language(Language::PtBr, || Edit::decode(b"").unwrap_err().to_string()), "não é um arquivo de edições de splats");
    }

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
        let (kept, removed, _) = read_edited(&ply).unwrap();
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
        let (kept, removed, _) = read_edited(&ply).unwrap();
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

    fn qoi(value: u8) -> Vec<u8> {
        orbiters_contracts::qoi::encode(&[value; 4 * 2 * 3], 4, 2, 3)
    }

    /// The worker's picture waits aside under its PLY's name, written whole; a damaged one (a copy cut short) is never
    /// taken for a backup nor put back; one of another PLY (an earlier reconstruction) is never put back and goes.
    #[test]
    fn the_trained_preview_waits_aside_for_its_own_ply_only() {
        let dir = temp("picture");
        let ply = Identity { bytes: 100, hash: 7 };
        let trained = dir.join(rec::trained_preview_file(7));
        std::fs::write(dir.join(rec::PREVIEW_FILE), qoi(1)).unwrap();
        write_picture(&dir, ply, &qoi(2)).unwrap();
        write_picture(&dir, ply, &qoi(3)).unwrap();
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), qoi(3));
        assert_eq!(std::fs::read(&trained).unwrap(), qoi(1), "the worker's, not an edited one");
        restore_picture(&dir, Some(ply)).unwrap();
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), qoi(1));
        assert!(!trained.exists());
        // A backup cut short: taken again from the preview, never restored as is.
        std::fs::write(&trained, &qoi(1)[..10]).unwrap();
        write_picture(&dir, ply, &qoi(2)).unwrap();
        assert_eq!(std::fs::read(&trained).unwrap(), qoi(1), "the cut-short backup replaced by a whole one");
        std::fs::write(&trained, &qoi(1)[..10]).unwrap();
        assert!(restore_picture(&dir, Some(ply)).is_err(), "damaged: refused");
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), qoi(2), "the picture there stays");
        // Another PLY's backup (an earlier reconstruction's): never put back, gone.
        let other = dir.join(rec::trained_preview_file(9));
        std::fs::write(&other, qoi(5)).unwrap();
        restore_picture(&dir, Some(ply)).unwrap();
        assert!(!other.exists());
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), qoi(2));
        // Discard: the edit goes, the PLY's own backup comes back.
        write_picture(&dir, ply, &qoi(4)).unwrap();
        let mut edit = Edit::new(1, ply);
        edit.marks[0] = DELETED;
        edit.save(&dir).unwrap();
        discard_for(&dir, Some(ply)).unwrap();
        assert_eq!(std::fs::read(dir.join(rec::PREVIEW_FILE)).unwrap(), qoi(2));
        assert!(!dir.join(rec::EDIT_FILE).exists() && !trained.exists());
        assert!(rec::is_trained_preview_file(&rec::trained_preview_file(u64::MAX)) && !rec::is_trained_preview_file("preview.trained.qoi"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The generation: one word for what leaves the outputs, the same for the same removals whatever else changed (the
    /// picture flag, hidden or deleted, a selection), different for other removals; damaged edits are an error.
    #[test]
    fn the_generation_names_what_leaves_the_outputs() {
        let dir = temp("generation");
        assert_eq!(generation(&dir).unwrap(), UNEDITED);
        let ply = Identity { bytes: 1 << 20, hash: 0xABC };
        let mut e = Edit::new(100, ply);
        e.marks[3] = HIDDEN;
        e.save(&dir).unwrap();
        let g = generation(&dir).unwrap();
        assert_ne!(g, UNEDITED);
        e.marks[3] = DELETED | SELECTED;
        e.picture = false;
        e.save(&dir).unwrap();
        assert_eq!(generation(&dir).unwrap(), g, "deleted instead of hidden, a selection, the picture flag: the same outputs");
        e.marks[4] = HIDDEN;
        e.save(&dir).unwrap();
        assert_ne!(generation(&dir).unwrap(), g);
        assert_eq!(Edit { ply: Identity { hash: 0xABD, ..ply }, ..e.clone() }.generation() == e.generation(), false, "another PLY");
        std::fs::write(dir.join(rec::EDIT_FILE), b"damaged").unwrap();
        assert!(generation(&dir).is_err());
        assert_eq!(Edit::new(5, ply).generation(), UNEDITED);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Whether the picture may leave the app: with no edits, yes; with edits, only once drawn for them, never while
    /// edited since, damaged, or made on another PLY (its length or, when its time changed, its hash).
    #[test]
    fn the_picture_leaves_the_app_only_when_drawn_for_the_edits_there_are() {
        let dir = temp("picture-state");
        let mut g = Gaussians::default();
        for i in 0..10 {
            g.push([i as f32, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        }
        let ply = dir.join(rec::PLY_FILE);
        crate::ply::write(&ply, &g).unwrap();
        assert_eq!(picture_state(&dir), PictureState::Unedited);
        assert!(picture_state(&dir).thumbnail_ok());
        let mut e = Edit::new(10, Identity::read(&ply).unwrap());
        e.marks[2] = DELETED;
        e.picture = false;
        e.save(&dir).unwrap();
        assert_eq!(picture_state(&dir), PictureState::NotDrawn);
        assert!(!picture_state(&dir).preview_ok());
        e.picture = true;
        e.save(&dir).unwrap();
        assert_eq!(picture_state(&dir), PictureState::Drawn);
        assert!(picture_state(&dir).preview_ok() && !picture_state(&dir).thumbnail_ok());
        // Another reconstruction of the same size (same length, a new time): hashed again, refused.
        g.means[0][1] = 0.25;
        std::thread::sleep(std::time::Duration::from_millis(20));
        crate::ply::write(&ply, &g).unwrap();
        assert!(matches!(picture_state(&dir), PictureState::Problem(_)));
        std::fs::write(dir.join(rec::EDIT_FILE), b"damaged").unwrap();
        assert!(matches!(picture_state(&dir), PictureState::Problem(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Strokes held open (a brush's dabs) are trimmed like any step, by what their vectors hold allocated: sixteen
    /// strokes over a million splats stay under a budget of four of them, the newest stroke kept, and undo still walks
    /// back what is kept.
    #[test]
    fn the_history_stays_within_its_budget_with_strokes_too() {
        let n = 1_000_000u32;
        let mut edit = Edit::new(n as usize, Identity::default());
        let step = (n as usize) * 6;
        let mut h = History::with_budget(4 * step);
        for k in 0..16u32 {
            h.begin();
            // Each stroke in two dabs, touching every splat (the selection flipped).
            let mark = if k % 2 == 0 { SELECTED } else { 0 };
            assert!(h.apply(&mut edit, (0..n / 2).map(|i| (i, mark))));
            assert!(h.apply(&mut edit, (n / 2..n).map(|i| (i, mark))));
            h.end();
            assert!(h.memory() <= 4 * step, "stroke {k}: {} bytes held for a budget of {}", h.memory(), 4 * step);
        }
        assert!(h.can_undo());
        assert!(h.undo(&mut edit));
        assert_eq!(edit.marks[0], SELECTED, "the last stroke (cleared) undone");
        // A stroke cancelled halfway (ended without its release) is one step: what comes next is a step of its own.
        let mut e = Edit::new(10, Identity::default());
        let mut h = History::default();
        h.begin();
        h.apply(&mut e, [(1, SELECTED)]);
        h.end();
        assert!(!h.is_open());
        h.apply(&mut e, [(1, DELETED)]);
        assert!(h.undo(&mut e));
        assert_eq!(e.marks[1], SELECTED, "the delete undone alone, the selection back");
    }
}
