//! Select splats: taking what does not belong out of a capture (a leftover avatar, floaters, a menu caught in it)
//! before it is shared. Select splats in the 3D view, then hide or delete them: they are not drawn here, and leave
//! every file made of the capture (the VRChat trees for PC, Quest and the coarse first load, the website's preview made
//! from the PC tree, the command line's exports, the capture's picture). The PLY stays as trained: the edit is a file
//! beside it (orbiters_reconstruct::edit), saved after every change, and undo, Show all and Restore all bring any
//! splat back.
//!
//! The selection is a set of splats in the world, by their IDs (their order in the PLY), not a shape on the screen: it
//! stays on them as the camera moves. Its tools (orbiters_reconstruct::select): **Pick** clicks the splats under the
//! cursor (what the pixel shows, through what is hidden; a drag orbits), **Brush** paints over what you see (never what
//! it hides), **Box** drags a rectangle that selects through every depth, **Sphere** presses on a surface and drags
//! out a sphere around it. Shift adds to the selection, Ctrl takes away; Grow and Shrink widen or narrow it by its
//! neighbours, Invert swaps it with the rest. While a tool takes the drags, the right button orbits and the middle one
//! pans. Keys: H hides, Delete deletes, Alt+H shows all, Ctrl+A selects all, Ctrl+I inverts, Ctrl+Z and Ctrl+Shift+Z
//! undo and redo, 1 to 4 pick a tool, Esc clears the selection and then leaves. On a Mac Ctrl is Cmd, Alt is Option, the
//! delete key deletes and two fingers on a trackpad do what the right and middle buttons do ([`super::gestures`]).
//!
//! The capture's picture (its card, its uploads' link previews, a drag out of the app) is drawn again without what is
//! removed once the selection is left or the view closed ([`redraw_picture`], from the camera the worker drew it
//! from); the worker's own picture waits beside it until nothing is removed.

use super::*;
use egui::{PointerButton, Pos2};
use orbiters_i18n::{mark, tr, tr_ctx};
use orbiters_reconstruct::edit::{self, DELETED, Edit, Found, HIDDEN, History, Identity, SELECTED};
use orbiters_reconstruct::select::{self as pick, Splats};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::widgets::Size;

use std::sync::atomic::{AtomicU64, Ordering};

/// Pictures of captures drawn again for their edits since the app started: a session's detail keyed by it shows the
/// new one at once.
static PICTURES: AtomicU64 = AtomicU64::new(0);

pub fn pictures() -> u64 {
    PICTURES.load(Ordering::Relaxed)
}

/// Writes of edit files and of the picture's flag in this process, one at a time: a picture's flag is written only while
/// the file still holds the edit it was drawn for, and no save lands in between.
static WRITING: Mutex<()> = Mutex::new(());

fn writing() -> std::sync::MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why a view's splats or edits no longer go together with the PLY on the disk.
const RECONSTRUCTED: &str = mark("The capture was reconstructed again since its 3D view opened: close the view and open it again.");

/// A PLY's length and modification time: what tells cheaply that it is still the one an edit was made on (it is hashed
/// again only when they changed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Stamp {
    fn of(ply: &Path) -> Option<Self> {
        let m = std::fs::metadata(ply).ok()?;
        Some(Self { len: m.len(), modified: m.modified().ok() })
    }
}

/// Writes `edit` beside the PLY in `result` before it returns, so that whatever reads the edits next (a share asked
/// right after, the upload preparation in the service's process, this view opened again) reads these: only while the PLY
/// there is still the one the edit was made on (its stamp as loaded, `loaded`; hashed again when the stamp changed). An
/// edit that removes nothing takes its file away and puts the worker's picture of that PLY back.
fn persist(result: &Path, edit: &Edit, loaded: Option<Stamp>) -> anyhow::Result<()> {
    let _w = writing();
    let ply = result.join(reconstruction::PLY_FILE);
    if loaded.is_none() || Stamp::of(&ply) != loaded {
        anyhow::ensure!(Identity::read(&ply)? == edit.ply, "{} {}", tr(RECONSTRUCTED), tr("These edits were not saved."));
    }
    if edit.removes_nothing() {
        edit::discard_for(result, Some(edit.ply))?;
        PICTURES.fetch_add(1, Ordering::Relaxed);
    } else {
        edit.save(result)?;
    }
    Ok(())
}

/// A session's splats in PLY order (a downloaded `.splat`'s as they come), its sky map, and for a PLY its identity and
/// the edit found beside it.
pub(super) fn read(dir: &Path) -> anyhow::Result<(Gaussians, Option<SkyMap>, Option<(Identity, Found)>)> {
    if dir.extension().is_some_and(|e| e == "splat") {
        return Ok((orbiters_reconstruct::web::read_splat(&std::fs::read(dir)?)?, None, None));
    }
    let result = reconstruction::result_dir(dir);
    let (g, identity, found) = edit::read_for_view(&result.join(reconstruction::PLY_FILE))?;
    let sky_file = result.join(reconstruction::SKY_FILE);
    let sky = if sky_file.is_file() { Some(SkyMap::read(&sky_file)?) } else { None };
    Ok((g, sky, Some((identity, found))))
}

/// The splats and sky map of `dir` read again (moving them to the other GPU, a picture of the view), only when they are
/// still those of the PLY the view loaded (`expected`): the view's marks are never laid over another reconstruction's
/// splats.
pub(super) fn read_again(dir: &Path, expected: Option<Identity>) -> anyhow::Result<(Gaussians, Option<SkyMap>)> {
    let (g, sky, found) = read(dir)?;
    if let (Some(want), Some((now, _))) = (expected, found) {
        anyhow::ensure!(now == want, "{}", tr(RECONSTRUCTED));
    }
    Ok((g, sky))
}

/// The edits of a reconstruction's splats as the view holds them: the marks (hidden, deleted, selected), what the file
/// holds, undo, and what draws them.
pub(super) struct Edits {
    session: PathBuf,
    result: PathBuf,
    pub(super) edit: Edit,
    /// What the last save wrote (nothing to save while the outputs are the same).
    saved: Edit,
    /// The PLY as loaded: a save checks it is still there.
    stamp: Option<Stamp>,
    /// The edit found beside the PLY could not be applied (made on an earlier reconstruction, or damaged): why. Shares
    /// stop, pictures of the view are refused and selecting waits until it is discarded.
    pub(super) problem: Option<Problem>,
    /// The marks as the GPU draws them (orbiters_reconstruct::edit::Edit::view_marks) and their version; the version of
    /// what is drawn at all (hidden and deleted, which a pick's projection depends on).
    pub(super) version: u64,
    pub(super) drawn: Arc<Vec<u8>>,
    shown: u64,
    /// Selected, hidden, deleted.
    counts: [usize; 3],
    history: History,
    save_error: Option<String>,
    /// The capture's picture does not show the outputs as saved yet, and the job drawing it again.
    picture_stale: bool,
    drawing: Option<Job<Result<bool, String>>>,
    picture_error: Option<String>,
    window: (wgpu::Device, wgpu::Queue),
}

/// Why the edits found beside the PLY cannot be applied, kept as what happened and said in the language now where it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Problem {
    /// Made on an earlier reconstruction.
    Stale,
    /// Damaged or unreadable: what the reader said.
    Unreadable(String),
    /// Discard them failed: what went wrong.
    NotDiscarded(String),
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl Problem {
    pub(super) fn text(&self) -> String {
        match self {
            Problem::Stale => tr("Its splat edits were made on an earlier reconstruction: nothing they hid is hidden here, and sharing it stops until they are discarded.").to_string(),
            Problem::Unreadable(e) => tr!("Its splat edits could not be read ({e}): sharing it stops until they are discarded.", e = e),
            Problem::NotDiscarded(e) => tr!("They could not be discarded: {e}", e = e),
        }
    }
}

impl Edits {
    /// The edits of the `splats` splats of `session`'s PLY (`identity`) as `found` beside it.
    pub(super) fn new(session: &Path, splats: usize, identity: Identity, found: Found, window: (wgpu::Device, wgpu::Queue)) -> Self {
        let blank = Edit::new(splats, identity);
        let problem = match &found {
            Found::Stale(_) => Some(Problem::Stale),
            Found::Unreadable(e) => Some(Problem::Unreadable(e.to_string())),
            _ => None,
        };
        let (edit, picture_stale) = match found {
            Found::Current(e) => {
                let stale = !e.picture;
                (e, stale)
            }
            _ => (blank, false),
        };
        let result = reconstruction::result_dir(session);
        let mut e = Self {
            session: session.to_path_buf(),
            stamp: Stamp::of(&result.join(reconstruction::PLY_FILE)),
            result,
            saved: edit.clone(),
            drawn: Arc::new(edit.view_marks()),
            edit,
            problem,
            version: 1,
            shown: 1,
            counts: [0; 3],
            history: History::default(),
            save_error: None,
            picture_stale,
            drawing: None,
            picture_error: None,
            window,
        };
        e.count();
        e
    }

    fn count(&mut self) {
        let mut c = [0usize; 3];
        for &m in &self.edit.marks {
            c[0] += usize::from(m & SELECTED != 0);
            c[1] += usize::from(m & HIDDEN != 0);
            c[2] += usize::from(m & DELETED != 0);
        }
        self.counts = c;
    }

    pub(super) fn selected(&self) -> usize {
        self.counts[0]
    }

    pub(super) fn hidden(&self) -> usize {
        self.counts[1]
    }

    pub(super) fn deleted(&self) -> usize {
        self.counts[2]
    }

    /// The marks changed: drawn again, counted again, and saved when what leaves the capture changed.
    fn changed(&mut self) {
        let drawn = self.edit.view_marks();
        // What is drawn at all changed (not only the selection): the tools' projection is made again.
        if drawn.iter().zip(self.drawn.iter()).any(|(a, b)| (*a >= 2) != (*b >= 2)) {
            self.shown += 1;
        }
        self.drawn = Arc::new(drawn);
        self.version += 1;
        self.count();
        self.save();
    }

    /// One step of undo: `marks` (the new mark of each splat named).
    fn apply(&mut self, marks: impl IntoIterator<Item = (u32, u8)>) -> bool {
        let changed = self.history.apply(&mut self.edit, marks);
        if changed {
            self.changed();
        }
        changed
    }

    /// Starts a brush stroke: every change until [`Edits::end_stroke`] is one step of undo.
    fn begin_stroke(&mut self) {
        self.history.begin();
    }

    /// Ends the stroke under way, if any (its release, or a cancel: Esc, another tool, Done, the view closed).
    pub(super) fn end_stroke(&mut self) {
        self.history.end();
    }

    /// `ids` (drawn splats) into the selection as `mode` says.
    fn select(&mut self, ids: &[u32], mode: Mode) -> bool {
        // Adding or taking away touches only those (a brush's dabs, every frame of a stroke).
        if mode != Mode::Replace {
            let marks = &self.edit.marks;
            let changes: Vec<(u32, u8)> = ids
                .iter()
                .filter_map(|&i| {
                    let m = *marks.get(i as usize)?;
                    let new = if mode == Mode::Add { m | SELECTED } else { m & !SELECTED };
                    (new != m && pick::drawn(m)).then_some((i, new))
                })
                .collect();
            return self.apply(changes);
        }
        let n = self.edit.len();
        let mut want = vec![false; n];
        for &i in ids {
            if let Some(w) = want.get_mut(i as usize) {
                *w = true;
            }
        }
        let marks = &self.edit.marks;
        let changes: Vec<(u32, u8)> = (0..n)
            .filter_map(|i| {
                let m = marks[i];
                let new = if want[i] { m | SELECTED } else { m & !SELECTED };
                (new != m).then_some((i as u32, new))
            })
            .collect();
        self.apply(changes)
    }

    /// Every splat whose marks `f` changes, changed so (one step).
    fn map(&mut self, f: impl Fn(u8) -> u8) -> bool {
        let changes: Vec<(u32, u8)> = self.edit.marks.iter().enumerate().filter_map(|(i, &m)| (f(m) != m).then(|| (i as u32, f(m)))).collect();
        self.apply(changes)
    }

    fn select_all(&mut self) -> bool {
        self.map(|m| if pick::drawn(m) { m | SELECTED } else { m })
    }

    pub(super) fn clear(&mut self) -> bool {
        self.map(|m| m & !SELECTED)
    }

    fn invert(&mut self) -> bool {
        self.map(|m| if pick::drawn(m) { m ^ SELECTED } else { m })
    }

    /// The selection hidden (`mark` HIDDEN) or deleted (DELETED).
    fn remove(&mut self, mark: u8) -> bool {
        self.map(|m| if m & SELECTED != 0 { (m & !SELECTED) | mark } else { m })
    }

    fn show_all(&mut self) -> bool {
        self.map(|m| m & !HIDDEN)
    }

    fn restore_all(&mut self) -> bool {
        self.map(|m| m & !(HIDDEN | DELETED))
    }

    fn undo(&mut self) {
        if self.history.undo(&mut self.edit) {
            self.changed();
        }
    }

    fn redo(&mut self) {
        if self.history.redo(&mut self.edit) {
            self.changed();
        }
    }

    /// Saves the edit when what leaves the capture changed since the last save (or the last save failed), on the disk
    /// before it returns ([`persist`]); never while an edit that could not be applied waits to be discarded (saving
    /// would drop it unseen).
    fn save(&mut self) {
        if self.problem.is_some() || (self.save_error.is_none() && self.edit.same_outputs(&self.saved)) {
            return;
        }
        let mut e = self.edit.clone();
        e.picture = false;
        match persist(&self.result, &e, self.stamp) {
            Ok(()) => {
                self.picture_stale = !e.removes_nothing();
                self.saved = e;
                self.save_error = None;
            }
            Err(err) => self.save_error = Some(format!("{err:#}")),
        }
    }

    /// Saves again after a failure.
    fn retry(&mut self) {
        self.save();
    }

    /// Takes in what the picture's job came back with.
    pub(super) fn poll(&mut self) {
        // A drawing whose worker panicked ends as a failed one (it kept the view busy for ever).
        let r = self.drawing.as_mut().and_then(|j| j.take().or_else(|| j.died().then(|| Err("it stopped unexpectedly".to_string())))); // i18n: not shown (kept as the picture's error, which only stops a redraw)
        if let Some(r) = r {
            self.drawing = None;
            match r {
                Ok(_) => self.picture_error = None,
                Err(e) => {
                    self.picture_stale = true;
                    self.picture_error = Some(e);
                }
            }
        }
    }

    /// Draws the capture's picture again when it does not show the edit as saved.
    pub(super) fn redraw(&mut self, ctx: &egui::Context) {
        if !self.picture_stale || self.problem.is_some() || self.drawing.is_some() || self.save_error.is_some() || self.picture_error.is_some() {
            return;
        }
        self.picture_stale = false;
        let (session, window) = (self.session.clone(), self.window.clone());
        self.drawing = Some(Job::spawn(ctx, move || redraw_picture(&session, window).map_err(|e| format!("{e:#}"))));
    }

    /// The edit that could not be applied, discarded: the trained splats and the worker's picture of this PLY back,
    /// shares and pictures of the view possible again.
    fn discard(&mut self) -> anyhow::Result<()> {
        let _w = writing();
        edit::discard_for(&self.result, Some(self.edit.ply))?;
        PICTURES.fetch_add(1, Ordering::Relaxed);
        self.problem = None;
        Ok(())
    }

    /// Whether the picture is being drawn.
    fn busy(&self) -> bool {
        self.drawing.is_some()
    }

    /// Whether the last save failed: the view leaves out splats the file beside the PLY still keeps, so whatever is made
    /// from the file (Export) would bring them back until Retry saves it.
    pub(super) fn unsaved(&self) -> bool {
        self.save_error.is_some()
    }
}

/// The splat edits of `session` that could not be applied, discarded outside its 3D view (Export's dialog) as Discard
/// them does in the view ([`Edits::discard`]): the file gone and the worker's own picture of its PLY back, under the same
/// lock as every save.
pub fn discard_edits(session: &Path) -> anyhow::Result<()> {
    let _w = writing();
    edit::discard(&reconstruction::result_dir(session))?;
    PICTURES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

impl Drop for Edits {
    /// The view closed: a save that failed is tried once more (saves are written as they are made, so there is nothing
    /// else to write), a stroke under way ends, and the picture is drawn again in a thread of its own when it does not
    /// show the edit saved, after a drawing still under way (it checks the file still holds that edit before it says so;
    /// a view opened next draws it again otherwise).
    fn drop(&mut self) {
        self.history.end();
        if self.problem.is_some() {
            return;
        }
        if self.save_error.is_some() {
            self.save();
            if let Some(e) = &self.save_error {
                eprintln!("Splat edits of {} not saved: {e}", self.session.display());
                return;
            }
        }
        if !self.picture_stale {
            return;
        }
        // A drawing still under way was started for an older edit (it writes nothing once it sees the file changed):
        // this one waits for it, then draws the last edit (it was left undrawn until the view was opened again). One
        // renderer at a time, as on a small graphics card.
        let running = self.drawing.take();
        let (session, window) = (self.session.clone(), self.window.clone());
        // Work of the window (it writes the capture's picture): the window does not close to the tray meanwhile.
        let working = super::super::Working::begin();
        let spawned = std::thread::Builder::new().name("splat edits' picture".into()).spawn(move || { // i18n: not shown
            let _working = working;
            if let Some(mut job) = running {
                while job.poll().is_none() && job.running() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            if let Err(err) = redraw_picture(&session, window) {
                eprintln!("The picture of {}: {err:#}", session.display());
            }
        });
        if let Err(e) = spawned {
            eprintln!("Splat edits: {e}");
        }
    }
}

/// The capture's picture drawn again without what its edit removes: from the camera the worker drew it from, at its
/// size, as the worker draws (no viewer's guards, the sky map behind), into its place, the worker's kept aside for this
/// PLY (orbiters_reconstruct::edit::write_picture); then the edit file says so, only while it still holds the edit drawn.
/// Everything removed draws the sky alone. Nothing to do (false) when the file's edit already has its picture or
/// removes nothing. Not drawn, and an error, when the records do not say where the worker's camera was (the picture then
/// stays out of shares: orbiters_reconstruct::edit::picture_state).
pub(super) fn redraw_picture(session: &Path, window: (wgpu::Device, wgpu::Queue)) -> anyhow::Result<bool> {
    use anyhow::Context as _;
    let result = reconstruction::result_dir(session);
    let ply = result.join(reconstruction::PLY_FILE);
    let (mut g, identity, found) = edit::read_for_view(&ply)?;
    let Found::Current(drawn) = found else { return Ok(false) };
    if drawn.picture || drawn.removes_nothing() {
        return Ok(false);
    }
    let splats = g.len();
    let camera = edit::preview_camera(session).with_context(|| tr("its records do not say where its picture was taken from"))?;
    edit::apply(&mut g, &drawn);
    if g.is_empty() {
        // Everything removed: the renderer draws nothing of an empty set, so one splat too faint to show (far ahead,
        // fully transparent) stands in, and the picture is the sky behind.
        let f = camera.forward();
        let at = camera.position();
        g.push([at[0] + f[0] * 100.0, at[1] + f[1] * 100.0, at[2] + f[2] * 100.0], [-6.0; 3], [1.0, 0.0, 0.0, 0.0], -40.0, [0.0; 3]);
    }
    let mut gpu = Gpu::renderer(window.0, window.1);
    gpu.guards = false;
    gpu.upload(&g);
    drop(g);
    let sky_file = result.join(reconstruction::SKY_FILE);
    if sky_file.is_file() {
        let map = SkyMap::read(&sky_file)?;
        gpu.set_sky(Some((map.width, &map.rgb)))?;
    }
    let image = orbiters_reconstruct::train::render(&mut gpu, &camera, [0.0; 3])?;
    let rgb: Vec<u8> = image.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let qoi = orbiters_core::contracts::qoi::encode(&rgb, camera.width, camera.height, 3);
    // Only while the file still holds the edit drawn (a newer save draws its own).
    let _w = writing();
    match edit::find(&ply, identity, splats) {
        Found::Current(now) if now.same_outputs(&drawn) => {
            edit::write_picture(&result, identity, &qoi)?;
            Edit { picture: true, ..now }.save(&result)?;
            PICTURES.fetch_add(1, Ordering::Relaxed);
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tool {
    #[default]
    Pick,
    Brush,
    Box,
    Sphere,
}

impl Tool {
    /// Whether it takes the left button's drags (the right one orbits then).
    fn drags(self) -> bool {
        self != Tool::Pick
    }

    fn hint(self) -> &'static str {
        // A trackpad has no middle button, and its right button is a two-finger press: two fingers orbit and pan there
        // (super::gestures).
        #[cfg(target_os = "macos")]
        return match self {
            Tool::Pick => tr("Click a splat to select it · Shift adds, Cmd takes away · drag or two fingers to orbit, Shift and two fingers to pan"),
            Tool::Brush => tr("Paint over what to select · Shift adds, Cmd takes away · two fingers to orbit, Shift and two fingers to pan"),
            Tool::Box => tr("Drag a box: it selects at every depth · Shift adds, Cmd takes away · two fingers to orbit"),
            Tool::Sphere => tr("Press on a surface and drag out the sphere · Shift adds, Cmd takes away · two fingers to orbit"),
        };
        #[cfg(not(target_os = "macos"))]
        match self {
            Tool::Pick => tr("Click a splat to select it · Shift adds, Ctrl takes away · drag to orbit, right-drag to pan"),
            Tool::Brush => tr("Paint over what to select · Shift adds, Ctrl takes away · right-drag to orbit, middle-drag to pan"),
            Tool::Box => tr("Drag a box: it selects at every depth · Shift adds, Ctrl takes away · right-drag to orbit"),
            Tool::Sphere => tr("Press on a surface and drag out the sphere · Shift adds, Ctrl takes away · right-drag to orbit"),
        }
    }
}

/// How a tool's splats change the selection: instead of it, added to it, taken from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Replace,
    Add,
    Remove,
}

impl Mode {
    fn of(m: egui::Modifiers) -> Self {
        if m.shift {
            Mode::Add
        } else if m.command || m.alt {
            Mode::Remove
        } else {
            Mode::Replace
        }
    }
}

/// A tool's drag under way (positions in points on the screen).
enum Drag {
    Brush {
        mode: Mode,
        last: Pos2,
    },
    /// From where it was pressed to where the pointer is.
    Box {
        from: Pos2,
        to: Pos2,
        mode: Mode,
    },
    /// Its centre in the world, how far along the view it is, and its radius (metres).
    Sphere {
        centre: [f32; 3],
        depth: f32,
        radius: f32,
        mode: Mode,
    },
}

/// How long the ring of a pick lasts, seconds.
const PULSE_S: f64 = 0.55;

/// The brush's radius, points.
const BRUSH: std::ops::RangeInclusive<f32> = 4.0..=120.0;

pub(super) struct State {
    on: bool,
    pub(super) tool: Tool,
    brush: f32,
    /// The splats' geometry for the tools (read when the mode first opens), or why it could not be.
    splats: Option<Arc<Splats>>,
    loading: Option<Job<Result<Arc<Splats>, String>>>,
    error: Option<String>,
    /// The tools' projection, for its camera and the version of what is drawn.
    view: Option<(Camera, u64, pick::View)>,
    drag: Option<Drag>,
    /// A word under the tools (nothing under the cursor), and when it was said.
    notice: Option<(String, f64)>,
    /// Where a pick landed and when: a ring leaves that point. One splat of hundreds of thousands, tinted, shows nothing;
    /// this says the click did something, there.
    pulse: Option<(Pos2, f64)>,
}

impl Default for State {
    fn default() -> Self {
        Self { on: false, tool: Tool::Pick, brush: 24.0, splats: None, loading: None, error: None, view: None, drag: None, notice: None, pulse: None }
    }
}

/// What Select splats' whole-capture chip can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whole {
    Invert,
    SelectAll,
    ShowAll,
    RestoreAll,
}

/// What the selection's bar can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum On {
    Hide,
    Delete,
    Grow,
    Shrink,
    Clear,
}

/// What Select splats is for, in one line, while nothing is selected.
pub(super) const PURPOSE: &str = mark("For cleaning up: select floaters or stray splats, then hide or delete them from what is shown, shared and exported.");

/// [`PURPOSE`] in a chip over the middle of the view, its bottom at `bottom`.
fn purpose_line(p: &egui::Painter, ui: &Ui, rect: Rect, bottom: f32) {
    let t = Txt::new(tr(PURPOSE), 12.5).color(theme::fg());
    let room = rect.width() - 32.0 - 56.0;
    let tw = widgets::text_size(ui, &t).x.min(room);
    let r = Rect::from_center_size(pos2(rect.center().x, bottom - 20.0), vec2(tw + 56.0, 40.0));
    p.rect_filled(r, theme::radius(20.0), theme::black(0.72));
    widgets::icon(p, pos2(r.left() + 24.0, r.center().y), icons::SPARKLE, 15.0, theme::accent());
    widgets::text_truncated(p, pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, &t, tw);
}

/// The selection with what can be done to it, over the view's middle with its bottom at `bottom`, `k` of the way in.
/// Hide and Delete, the reason to select anything, are the large ones; on a filled panel, unlike the view's chips.
fn selection_bar(ui: &mut Ui, rect: Rect, bottom: f32, k: f32, selected: usize, ready: bool) -> Option<On> {
    let count = if selected == 1 { tr("1 splat selected").to_string() } else { tr!("{count} splats selected", count = format::integer(selected as f64)) };
    let count = Txt::new(count, 13.5).weight(weight::SEMIBOLD);
    let count_w = widgets::text_size(ui, &count).x;
    let hide = Button::new(tr("Hide")).icon(icons::EYE_SLASH).variant(Variant::Secondary).hint(tr("Hides the selection: not drawn, not shared; Show all brings it back (H)")).id_salt("splat-select-hide");
    let delete = Button::new(tr("Delete"))
        .icon(icons::TRASH)
        .variant(Variant::Danger)
        .hint(tr("Deletes the selection from everything shared or exported; undo brings it back (Delete)"))
        .id_salt("splat-select-delete");
    let grow = Button::new(tr("Grow")).icon(icons::ARROWS_OUT).variant(Variant::Ghost).size(Size::Sm).enabled(ready).hint(tr("Adds the splats right beside the selection")).id_salt("splat-select-grow");
    let shrink = Button::new(tr("Shrink")).icon(icons::CORNERS_IN).variant(Variant::Ghost).size(Size::Sm).enabled(ready).hint(tr("Takes the selection's edge away")).id_salt("splat-select-shrink");
    let (hw, dw, gw, sw) = (hide.desired_width(ui), delete.desired_width(ui), grow.desired_width(ui), shrink.desired_width(ui));
    // The dot, the count, the large ones, then the small ones and the ×.
    let lead = 20.0 + 16.0 + count_w;
    let w = lead + 16.0 + hw + 8.0 + dw + 12.0 + gw + 4.0 + sw + 4.0 + 32.0 + 12.0;
    let r = Rect::from_center_size(pos2(rect.center().x, bottom - 28.0), vec2(w.min(rect.width() - 32.0), 56.0));
    let mut bu = ui.new_child(UiBuilder::new().max_rect(r).id_salt("splat-select-bar"));
    bu.multiply_opacity(k);
    let bp = bu.painter().clone();
    // Takes the clicks that land between its buttons: a press on the bar is not a pick behind it.
    let _ = bu.interact(r, Id::unique("splat-select-bar-back"), Sense::click_and_drag());
    widgets::panel(&bp, r, 28.0);
    bp.circle_filled(pos2(r.left() + 25.0, r.center().y), 5.0, SELECTION);
    widgets::text(&bp, pos2(r.left() + 20.0 + 16.0, r.center().y), Align2::LEFT_CENTER, &count);
    let mut x = r.left() + lead + 16.0;
    let mut acted = None;
    let large = |x: f32, w: f32| Rect::from_min_size(pos2(x, r.center().y - 20.0), vec2(w, 40.0));
    let small = |x: f32, w: f32| Rect::from_min_size(pos2(x, r.center().y - 16.0), vec2(w, 32.0));
    if hide.show_at(&mut bu, large(x, hw)).clicked() {
        acted = Some(On::Hide);
    }
    x += hw + 8.0;
    if delete.confirm_at(&mut bu, large(x, dw)) {
        acted = Some(On::Delete);
    }
    x += dw + 12.0;
    if grow.show_at(&mut bu, small(x, gw)).clicked() && ready {
        acted = Some(On::Grow);
    }
    x += gw + 4.0;
    if shrink.show_at(&mut bu, small(x, sw)).clicked() && ready {
        acted = Some(On::Shrink);
    }
    x += sw + 4.0;
    if IconButton::new(icons::X, mark("Clear")).size(32.0, 15.0).id_salt("splat-select-clear").show_at(&mut bu, small(x, 32.0)).clicked() {
        acted = Some(On::Clear);
    }
    acted
}

/// How Select splats' bottom row fits a view `view_w` wide: the tools (`tools_w` without the brush's size) from the left,
/// what works on the whole capture (`whole_w`) from the right. At 1030 points the brush's full 284 point slider ran under
/// Grow and hid its own value; it narrows first, and only when even a short one does not fit does the chip at the right
/// move up a row.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bottom {
    /// The brush's slider (0 for the other tools).
    slider: f32,
    /// The chip at the right sits above the tools instead of beside them.
    stacked: bool,
}

/// The brush's slider at its full width, and the narrowest that still reads.
const SLIDER_MAX: f32 = 284.0;
const SLIDER_MIN: f32 = 120.0;

impl Bottom {
    fn of(view_w: f32, tools_w: f32, whole_w: f32, brush: bool) -> Self {
        let beside = view_w - 2.0 * 16.0 - tools_w - 10.0 - whole_w;
        if !brush {
            return Self { slider: 0.0, stacked: beside < 0.0 };
        }
        let room = beside - 16.0;
        if room >= SLIDER_MIN {
            Self { slider: room.min(SLIDER_MAX), stacked: false }
        } else {
            Self { slider: (view_w - 2.0 * 16.0 - tools_w - 16.0).clamp(SLIDER_MIN, SLIDER_MAX), stacked: true }
        }
    }

    /// What the slider adds to the tools' chip.
    fn slider_room(self) -> f32 {
        if self.slider > 0.0 {
            self.slider + 16.0
        } else {
            0.0
        }
    }
}

/// [`Viewer::stop_drag`] on its parts.
fn stop_drag(s: &mut State, edits: Option<&mut Edits>) {
    if matches!(s.drag.take(), Some(Drag::Brush { .. })) {
        if let Some(e) = edits {
            e.end_stroke();
        }
    }
}

impl State {
    /// Select splats is open.
    pub(super) fn on(&self) -> bool {
        self.on
    }

    /// The left button's drags go to the tool.
    pub(super) fn takes_drags(&self) -> bool {
        self.on && self.tool.drags()
    }
}

impl Viewer {
    /// Select splats is open and its tools ready (screenshots).
    pub fn select_ready(&self) -> bool {
        self.select.on && self.select.splats.is_some()
    }

    /// What is selected, hidden and deleted (screenshots).
    pub fn select_counts(&self) -> Option<[usize; 3]> {
        self.loaded.as_ref()?.edits.as_ref().map(|e| e.counts)
    }

    /// Why the edit found beside the splats could not be applied, while it waits to be discarded (screenshots).
    pub fn edits_problem(&self) -> Option<String> {
        self.loaded.as_ref()?.edits.as_ref()?.problem.as_ref().map(Problem::text)
    }

    /// Why no picture of the view may be made now (Save this view, Edit in 3D's pictures: screenshots).
    pub fn picture_refused(&self) -> Option<String> {
        self.loaded.as_ref()?.drawn().err()
    }
}

impl Viewer {
    /// Opens Select splats (the splats' geometry read for its tools the first time).
    pub(super) fn enter_select(&mut self, ctx: &egui::Context) {
        let Some(e) = self.loaded.as_ref().and_then(|l| l.edits.as_ref()) else { return };
        if e.problem.is_some() {
            return;
        }
        let want = e.edit.ply;
        self.select.on = true;
        self.stop_drag();
        if self.select.splats.is_none() && self.select.loading.is_none() {
            let ply = reconstruction::result_dir(&self.session).join(reconstruction::PLY_FILE);
            self.select.error = None;
            self.select.loading = Some(Job::spawn(ctx, move || {
                // The very PLY the view shows (its hash, not only its count: IDs are its order).
                let (g, now, _) = edit::read_for_view(&ply).map_err(|e| format!("{e:#}"))?;
                if now != want {
                    return Err(tr(RECONSTRUCTED).into());
                }
                Ok(Arc::new(Splats::new(&g)))
            }));
        }
    }

    /// Leaves Select splats: a stroke under way ended, the selection cleared, the picture drawn again.
    pub(super) fn leave_select(&mut self, ctx: &egui::Context) {
        self.stop_drag();
        self.select.on = false;
        if let Some(e) = self.loaded.as_mut().and_then(|l| l.edits.as_mut()) {
            e.clear();
            e.redraw(ctx);
        }
    }

    /// Drops a tool's drag under way (Esc, another tool, the mode left): a brush stroke ends there as one step of undo,
    /// so that what comes next is a step of its own.
    pub(super) fn stop_drag(&mut self) {
        stop_drag(&mut self.select, self.loaded.as_mut().and_then(|l| l.edits.as_mut()));
    }

    /// Select splats' work each frame, open or not: what saving and the picture's job came back with, the tools'
    /// geometry, and the picture drawn again once the mode is left.
    pub(super) fn tend_edits(&mut self, ctx: &egui::Context) {
        if let Some(r) = self.select.loading.as_mut().and_then(|j| j.take()) {
            self.select.loading = None;
            match r {
                Ok(s) => self.select.splats = Some(s),
                Err(e) => self.select.error = Some(e),
            }
        }
        let on = self.select.on;
        if let Some(e) = self.loaded.as_mut().and_then(|l| l.edits.as_mut()) {
            e.poll();
            if !on {
                e.redraw(ctx);
            }
        }
    }

    /// The tools on the view's pointer (`resp` over `rect`) and keys, with what they draw over the view.
    pub(super) fn select_pointer(&mut self, ui: &Ui, resp: &egui::Response, rect: Rect, p: &egui::Painter) {
        let ctx = ui.ctx().clone();
        if !self.select.on {
            return;
        }
        let now = ctx.input(|i| i.time);
        self.select_keys(&ctx);
        let (Some(l), Some(orbit), Some(splats)) = (self.loaded.as_mut(), self.orbit, self.select.splats.clone()) else { return };
        let Some(edits) = l.edits.as_mut() else { return };
        let s = &mut self.select;
        let ppp = ctx.pixels_per_point();
        let (w, h) = (((rect.width() * ppp).round() as u32).max(16), ((rect.height() * ppp).round() as u32).max(16));
        let camera = orbit.camera(w, h);
        // Points on the screen to the camera's pixels (pixel centres at integers).
        let px = |q: Pos2| [(q.x - rect.left()) * ppp - 0.5, (q.y - rect.top()) * ppp - 0.5];
        let mode = Mode::of(ctx.input(|i| i.modifiers));
        let (press, primary_down) = ctx.input(|i| (i.pointer.press_origin(), i.pointer.primary_down()));
        let hover = resp.hover_pos();
        let shown = edits.shown;
        // The projection of what is drawn, made again only when the camera or what is drawn changed.
        let view = |s: &mut State, edits: &Edits| {
            if s.view.as_ref().is_none_or(|(c, v, _)| *c != camera || *v != shown) {
                s.view = Some((camera, shown, splats.view(&camera, &edits.edit.marks)));
            }
        };
        let say = |s: &mut State, text: &str| s.notice = Some((text.to_string(), now));
        match s.tool {
            Tool::Pick => {
                if resp.clicked_by(PointerButton::Primary) {
                    if let Some(at) = resp.interact_pointer_pos() {
                        view(s, edits);
                        let [x, y] = px(at);
                        let ids = s.view.as_ref().map(|(_, _, v)| v.clicked(x, y)).unwrap_or_default();
                        if ids.is_empty() {
                            say(s, tr("Nothing there to select"));
                            if mode == Mode::Replace {
                                edits.clear();
                            }
                        } else {
                            edits.select(&ids, mode);
                            s.pulse = Some((at, now));
                        }
                    }
                }
            }
            Tool::Brush => {
                let radius = s.brush;
                // Dabs every half radius from `from` to `to`, one step of undo with the rest of the stroke.
                let dab = |s: &mut State, edits: &mut Edits, from: Pos2, to: Pos2, mode: Mode| {
                    view(s, edits);
                    let Some((_, _, v)) = s.view.as_ref() else { return };
                    let steps = ((to - from).length() / (0.5 * radius)).ceil().max(1.0) as usize;
                    let mut ids = Vec::new();
                    for k in 0..=steps {
                        let q = from + (to - from) * (k as f32 / steps as f32);
                        ids.extend(v.visible_in_circle(px(q), radius * ppp, pick::BRUSH_WEIGHT));
                    }
                    edits.select(&ids, mode);
                };
                // A stroke adds what it paints (or takes it away with Ctrl), after clearing the selection without
                // Shift or Ctrl.
                let paints = if mode == Mode::Remove { Mode::Remove } else { Mode::Add };
                let start = |s: &mut State, edits: &mut Edits, at: Pos2| {
                    edits.begin_stroke();
                    if mode == Mode::Replace {
                        edits.clear();
                    }
                    dab(s, edits, at, at, paints);
                };
                if resp.drag_started_by(PointerButton::Primary) {
                    if let Some(from) = press {
                        start(s, edits, from);
                        s.drag = Some(Drag::Brush { mode: paints, last: from });
                    }
                } else if resp.clicked_by(PointerButton::Primary) {
                    if let Some(at) = resp.interact_pointer_pos() {
                        start(s, edits, at);
                        edits.end_stroke();
                    }
                }
                let stroke = match s.drag {
                    Some(Drag::Brush { mode, last }) => Some((mode, last)),
                    _ => None,
                };
                if let (Some((m, last)), Some(at), true) = (stroke, resp.interact_pointer_pos(), resp.dragged_by(PointerButton::Primary)) {
                    if at != last {
                        dab(s, edits, last, at, m);
                        s.drag = Some(Drag::Brush { mode: m, last: at });
                    }
                }
                if (resp.drag_stopped_by(PointerButton::Primary) || !primary_down) && stroke.is_some() {
                    s.drag = None;
                    edits.end_stroke();
                }
                if let Some(at) = hover.or(resp.interact_pointer_pos()) {
                    p.circle_stroke(at, radius, egui::Stroke::new(1.5, theme::white(0.85)));
                    p.circle_stroke(at, radius + 1.5, egui::Stroke::new(1.0, theme::black(0.5)));
                }
            }
            Tool::Box => {
                if resp.drag_started_by(PointerButton::Primary) {
                    if let Some(from) = press {
                        s.drag = Some(Drag::Box { from, to: from, mode });
                    }
                }
                if let Some(Drag::Box { from, to, mode }) = s.drag.as_mut() {
                    if let Some(at) = resp.interact_pointer_pos() {
                        *to = at;
                    }
                    let (r, mode) = (Rect::from_two_pos(*from, *to), *mode);
                    if resp.drag_stopped_by(PointerButton::Primary) || !primary_down {
                        s.drag = None;
                        let ids = pick::in_rectangle(&splats, &edits.edit.marks, &camera, px(r.min), px(r.max));
                        if ids.is_empty() {
                            say(s, tr("Nothing there to select"));
                        }
                        edits.select(&ids, mode);
                    } else {
                        p.rect_filled(r, 0.0, theme::alpha(SELECTION, 0.12));
                        p.rect_stroke(r, 0.0, egui::Stroke::new(1.5, SELECTION), egui::StrokeKind::Inside);
                    }
                }
            }
            Tool::Sphere => {
                if resp.drag_started_by(PointerButton::Primary) {
                    if let Some(from) = press {
                        view(s, edits);
                        let [x, y] = px(from);
                        match s.view.as_ref().and_then(|(_, _, v)| v.pick(x, y)) {
                            Some(hit) => s.drag = Some(Drag::Sphere { centre: hit.point, depth: camera.to_camera(hit.point)[2], radius: 0.0, mode }),
                            None => say(s, tr("Press on a surface to place the sphere")),
                        }
                    }
                }
                if let Some(Drag::Sphere { centre, depth, radius, mode }) = s.drag.as_mut() {
                    // Its radius: from the centre to the point under the cursor at the centre's depth along the view.
                    if let Some(at) = resp.interact_pointer_pos() {
                        let [x, y] = px(at);
                        let q = camera.unproject(x, y, *depth);
                        *radius = ((q[0] - centre[0]).powi(2) + (q[1] - centre[1]).powi(2) + (q[2] - centre[2]).powi(2)).sqrt();
                    }
                    let (c3, d, r3, mode) = (*centre, *depth, *radius, *mode);
                    if resp.drag_stopped_by(PointerButton::Primary) || !primary_down {
                        s.drag = None;
                        let ids = pick::in_sphere(&splats, &edits.edit.marks, c3, r3);
                        if ids.is_empty() {
                            say(s, tr("Nothing inside the sphere"));
                        }
                        edits.select(&ids, mode);
                    } else if let Some((cx, cy, _)) = camera.project(c3) {
                        let c = pos2(rect.left() + (cx + 0.5) / ppp, rect.top() + (cy + 0.5) / ppp);
                        let r = r3 * camera.fy / d.max(1e-3) / ppp;
                        p.circle_filled(c, r, theme::alpha(SELECTION, 0.12));
                        p.circle_stroke(c, r, egui::Stroke::new(1.5, SELECTION));
                        p.circle_filled(c, 3.0, SELECTION);
                        let label = Txt::new(tr!("{size} across", size = metres(2.0 * r3)), 12.0).color(theme::fg());
                        let size = widgets::text_size(ui, &label);
                        let lr = Rect::from_center_size(c + vec2(0.0, -r - 18.0), size + vec2(20.0, 10.0));
                        p.rect_filled(lr, theme::radius(lr.height() * 0.5), theme::black(0.6));
                        widgets::text(p, lr.center(), Align2::CENTER_CENTER, &label);
                    }
                }
            }
        }
        if s.drag.is_some() {
            ctx.request_repaint();
        }
        if let Some((at, when)) = s.pulse {
            let k = ((now - when) / PULSE_S) as f32;
            if k < 1.0 {
                let e = anim::ease::out_cubic(k.max(0.0));
                p.circle_stroke(at, 5.0 + 26.0 * e, egui::Stroke::new(2.5 * (1.0 - e), theme::alpha(SELECTION, 1.0 - e)));
                p.circle_filled(at, 4.0 * (1.0 - e), SELECTION);
                ctx.request_repaint();
            } else {
                s.pulse = None;
            }
        }
        if s.notice.as_ref().is_some_and(|(_, at)| now - at > 2.5) {
            s.notice = None;
        }
    }

    fn select_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        use egui::{Key, Modifiers};
        let pressed = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
            let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
            let all = i.consume_key(Modifiers::COMMAND, Key::A);
            let invert = i.consume_key(Modifiers::COMMAND, Key::I);
            let show = i.consume_key(Modifiers::ALT, Key::H);
            let hide = i.consume_key(Modifiers::NONE, Key::H);
            let delete = i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace);
            let escape = i.consume_key(Modifiers::NONE, Key::Escape);
            let tool = [(Key::Num1, Tool::Pick), (Key::Num2, Tool::Brush), (Key::Num3, Tool::Box), (Key::Num4, Tool::Sphere)]
                .into_iter()
                .find(|(k, _)| i.consume_key(Modifiers::NONE, *k))
                .map(|(_, t)| t);
            (undo, redo, all, invert, show, hide, delete, escape, tool)
        });
        let (undo, redo, all, invert, show, hide, delete, escape, tool) = pressed;
        if let Some(t) = tool {
            self.select.tool = t;
            self.stop_drag();
        }
        if escape && self.select.drag.is_some() {
            self.stop_drag();
            return;
        }
        let Some(e) = self.loaded.as_mut().and_then(|l| l.edits.as_mut()) else { return };
        if undo {
            e.undo();
        }
        if redo {
            e.redo();
        }
        if all {
            e.select_all();
        }
        if invert {
            e.invert();
        }
        if show {
            e.show_all();
        }
        if hide {
            e.remove(HIDDEN);
        }
        if delete {
            e.remove(DELETED);
        }
        if escape {
            if e.selected() > 0 {
                e.clear();
            } else {
                self.leave_select(ctx);
            }
        }
    }

    /// The top bar while Select splats is open, after the title: Done, undo and redo, and what is hidden and deleted (what
    /// is selected has its bar) and whether it is saved, up to `right`, where the view's own controls start.
    pub(super) fn select_bar(&mut self, ui: &mut Ui, p: &egui::Painter, title_r: Rect, right: f32) {
        let ctx = ui.ctx().clone();
        let done = Button::new(tr("Done")).icon(icons::CHECK).variant(Variant::Primary).hint(tr("Leaves Select splats (Esc)")).id_salt("splat-select-done");
        let dw = done.desired_width(ui);
        let done_r = Rect::from_min_size(pos2(title_r.right() + 10.0, title_r.top() + 2.0), vec2(dw, 40.0));
        if done.show_at(ui, done_r).clicked() {
            self.leave_select(&ctx);
            return;
        }
        let Some(e) = self.loaded.as_mut().and_then(|l| l.edits.as_mut()) else { return };
        let mut x = done_r.right() + 10.0;
        let chip_r = Rect::from_min_size(pos2(x, title_r.top()), vec2(92.0, 44.0));
        p.rect_filled(chip_r, theme::radius(22.0), theme::black(0.55));
        for (glyph, label, enabled, undo) in [
            (icons::ARROW_U_UP_LEFT, if cfg!(target_os = "macos") { mark("Undo (Cmd+Z)") } else { mark("Undo (Ctrl+Z)") }, e.history.can_undo(), true),
            (icons::ARROW_U_UP_RIGHT, if cfg!(target_os = "macos") { mark("Redo (Cmd+Shift+Z)") } else { mark("Redo (Ctrl+Shift+Z)") }, e.history.can_redo(), false),
        ] {
            let r = Rect::from_center_size(pos2(x + 24.0, chip_r.center().y), vec2(36.0, 36.0));
            let mut bu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("splat-select-history", label)));
            if !enabled {
                bu.disable();
                bu.multiply_opacity(0.35);
            }
            if IconButton::new(glyph, label).size(36.0, 17.0).show_at(&mut bu, r).clicked() && enabled {
                if undo {
                    e.undo();
                } else {
                    e.redo();
                }
            }
            x += 44.0;
        }
        // What is hidden and deleted; saved or not.
        let counts = tr!("{hidden} hidden · {deleted} deleted", hidden = format::integer(e.hidden() as f64), deleted = format::integer(e.deleted() as f64));
        let (state, tone) = match (&e.save_error, e.drawing.is_some()) {
            (Some(_), _) => (tr("Not saved"), theme::DANGER),
            (None, true) => (tr("Saved · drawing its picture again…"), theme::fg_3()),
            (None, false) => (tr("Saved beside the splats"), theme::fg_3()),
        };
        let t1 = Txt::new(counts, 12.5).color(theme::fg());
        let t2 = Txt::new(state, 11.0).color(tone);
        let retry = e.save_error.is_some();
        let extra = 40.0 + if retry { 84.0 } else { 0.0 };
        // Short of the view's controls whatever the title and the counts are (a long world name, a narrow window).
        let room = right - 10.0 - (chip_r.right() + 10.0) - extra;
        let tw = widgets::text_size(ui, &t1).x.max(widgets::text_size(ui, &t2).x).min(room.max(0.0));
        let status_r = Rect::from_min_size(pos2(chip_r.right() + 10.0, title_r.top()), vec2(tw + extra, 44.0));
        p.rect_filled(status_r, theme::radius(22.0), theme::black(0.55));
        widgets::text_truncated(p, pos2(status_r.left() + 20.0, status_r.center().y - 7.0), Align2::LEFT_CENTER, &t1, tw);
        widgets::text_truncated(p, pos2(status_r.left() + 20.0, status_r.center().y + 9.0), Align2::LEFT_CENTER, &t2, tw);
        if retry {
            let r = Rect::from_min_size(pos2(status_r.right() - 84.0, status_r.center().y - 16.0), vec2(76.0, 32.0));
            if Button::new(tr("Retry"))
                .variant(Variant::Ghost)
                .size(Size::Sm)
                .hint(e.save_error.as_deref().unwrap_or_default())
                .id_salt("splat-select-retry")
                .show_at(ui, r)
                .clicked()
            {
                e.retry();
            }
        }
    }

    /// Select splats' controls over the view's bottom ([`Bottom`]): the tools at the left with its hint above them; what
    /// works on the whole capture at the right; and, in the middle above them, the moment's own line. While nothing is
    /// selected it says what all this is for; once something is, it is the selection with what can be done to it, Hide and
    /// Delete first.
    ///
    /// Why they are not all one row in the corner, as they were: at Enzo's window (1030 x 800) every action was drawn and
    /// took clicks, and he still asked what selecting is for. A click (Pick) selects one splat of hundreds of thousands,
    /// which changes nothing to see; Hide and Delete, in the bottom right corner where the view keeps its display pills,
    /// went from dim to a little less dim. So the place, the look and the moment all said "not for you".
    pub(super) fn select_controls(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect) {
        let ctx = ui.ctx().clone();
        let chip = |r: Rect| p.rect_filled(r, theme::radius(r.height() * 0.5), theme::black(0.55));
        let ready = self.select.splats.is_some();
        let Some((selected, hidden, deleted)) = self.loaded.as_ref().and_then(|l| l.edits.as_ref()).map(|e| (e.selected(), e.hidden(), e.deleted())) else { return };

        // What works on the whole capture, at the right: its width says how much room the tools have.
        type Act = (&'static str, &'static str, &'static str, bool, Whole);
        let whole: [Act; 4] = [
            (icons::CIRCLE_HALF, mark("Invert"), if cfg!(target_os = "macos") { mark("Selects everything else (Cmd+I)") } else { mark("Selects everything else (Ctrl+I)") }, true, Whole::Invert),
            (icons::SQUARES_FOUR, mark("Select all"), if cfg!(target_os = "macos") { mark("Selects every splat drawn (Cmd+A)") } else { mark("Selects every splat drawn (Ctrl+A)") }, true, Whole::SelectAll),
            (icons::EYE, mark("Show all"), if cfg!(target_os = "macos") { mark("Shows every hidden splat again (Option+H)") } else { mark("Shows every hidden splat again (Alt+H)") }, hidden > 0, Whole::ShowAll),
            (icons::ARROW_COUNTER_CLOCKWISE, mark("Restore all"), mark("Brings back every hidden and deleted splat: the capture as reconstructed"), hidden + deleted > 0, Whole::RestoreAll),
        ];
        let buttons: Vec<Button> = whole
            .iter()
            .map(|&(glyph, label, hint, enabled, _)| Button::new(tr(label)).icon(glyph).variant(Variant::Ghost).size(Size::Sm).enabled(enabled).hint(tr(hint)).id_salt(("splat-select-whole", label)))
            .collect();
        let widths: Vec<f32> = buttons.iter().map(|b| b.desired_width(ui)).collect();
        let whole_w = widths.iter().sum::<f32>() + 6.0 * (widths.len() as f32 - 1.0) + 12.0;

        // The tools: a segmented control, and the brush's size in the room left beside what works on the whole capture.
        let label = Txt::new(tr_ctx("tool", "Select"), 12.0).color(theme::fg_3());
        let label_w = widgets::text_size(ui, &label).x;
        let options = [
            SegOption::new(Tool::Pick, tr("Pick")).hint(tr("Click the splats under the cursor (1)")),
            SegOption::new(Tool::Brush, tr("Brush")).hint(tr("Paint over what you see (2)")),
            SegOption::new(Tool::Box, tr("Box")).hint(tr("Drag a rectangle: every depth inside it (3)")),
            SegOption::new(Tool::Sphere, tr("Sphere")).hint(tr("Press on a surface, drag out a sphere (4)")),
        ];
        let seg_w = options.iter().map(|o| widgets::text_size(ui, &Txt::new(o.label, 12.0).weight(weight::MEDIUM)).x + 24.0).sum::<f32>() + 8.0;
        let tools_w = 18.0 + label_w + 12.0 + seg_w + 6.0;
        let bottom = Bottom::of(rect.width(), tools_w, whole_w, self.select.tool == Tool::Brush);
        let tools_r = Rect::from_min_size(pos2(rect.left() + 16.0, rect.bottom() - 16.0 - 44.0), vec2(tools_w + bottom.slider_room(), 44.0));
        chip(tools_r);
        widgets::text(p, pos2(tools_r.left() + 18.0, tools_r.center().y), Align2::LEFT_CENTER, &label);
        let seg_r = Rect::from_min_size(pos2(tools_r.left() + 18.0 + label_w + 12.0, tools_r.center().y - 18.0), vec2(seg_w, 36.0));
        let mut seg = ui.new_child(UiBuilder::new().max_rect(seg_r).id_salt("splat-select-tool"));
        let before = self.select.tool;
        widgets::segmented(&mut seg, "splat-select-tool", &mut self.select.tool, &options, true);
        if self.select.tool != before {
            self.stop_drag();
        }
        if self.select.tool == Tool::Brush {
            let slider_r = Rect::from_min_size(pos2(seg_r.right() + 16.0, tools_r.center().y - 14.0), vec2(bottom.slider, 28.0));
            let mut su = ui.new_child(UiBuilder::new().max_rect(slider_r).id_salt("splat-select-brush"));
            widgets::slider(&mut su, "splat-select-brush", &mut self.select.brush, BRUSH, 1.0, |v| format!("{v:.0} px"));
        }

        // What works on the whole capture, in a chip at the right: on the tools' row, or above it where they do not fit.
        let whole_bottom = if bottom.stacked { tools_r.top() - 10.0 } else { rect.bottom() - 16.0 };
        let whole_r = Rect::from_min_size(pos2(rect.right() - 16.0 - whole_w, whole_bottom - 44.0), vec2(whole_w, 44.0));
        chip(whole_r);
        let mut clicked = None;
        let mut x = whole_r.left() + 6.0;
        for ((b, bw), act) in buttons.into_iter().zip(widths).zip(whole.iter()) {
            if b.show_at(ui, Rect::from_min_size(pos2(x, whole_r.center().y - 16.0), vec2(bw, 32.0))).clicked() && act.3 {
                clicked = Some(act.4);
            }
            x += bw + 6.0;
        }

        // Above the tools: the hint, or what there is to say (the geometry still being read, why not, nothing there).
        let (text, color) = match (&self.select.error, ready, &self.select.notice) {
            (Some(e), _, _) => (tr!("The splats cannot be selected: {e}", e = e), theme::DANGER),
            (None, false, _) => (tr("Getting the splats ready to select…").to_string(), theme::fg_2()),
            (None, true, Some((n, _))) => (n.clone(), theme::WARN),
            (None, true, None) => (self.select.tool.hint().to_string(), theme::fg_2()),
        };
        let hint = Txt::new(text, 11.5).color(color);
        // Not under the chip at the right when that came up to this row.
        let hint_max = if bottom.stacked { whole_r.left() - 10.0 - rect.left() - 16.0 } else { rect.width() - 32.0 };
        let hint_w = (widgets::text_size(ui, &hint).x + 28.0).min(hint_max);
        let hint_r = Rect::from_min_size(pos2(rect.left() + 16.0, tools_r.top() - 10.0 - 30.0), vec2(hint_w, 30.0));
        chip(hint_r);
        widgets::text_truncated(p, hint_r.center(), Align2::CENTER_CENTER, &hint, hint_w - 28.0);
        if self.select.loading.is_some() {
            orbiters_ui::feedback::spinner(p, pos2(hint_r.right() + 18.0, hint_r.center().y), 8.0, 2.0, theme::accent());
        }

        // The moment's own line, in the middle above all of them: what it is for, or the selection and its actions.
        let line_bottom = hint_r.top().min(whole_r.top()) - 10.0;
        let some = anim::toggle(&ctx, Id::unique("splat-select-some"), selected > 0, Spring::SOFT).clamp(0.0, 1.0);
        if some < 0.99 {
            let mut lp = p.clone();
            lp.multiply_opacity(1.0 - some);
            purpose_line(&lp, ui, rect, line_bottom + 8.0 * some);
        }
        let mut acted = None;
        if selected > 0 {
            acted = selection_bar(ui, rect, line_bottom + 12.0 * (1.0 - some), some, selected, ready);
        }

        let Some(e) = self.loaded.as_mut().and_then(|l| l.edits.as_mut()) else { return };
        match acted {
            Some(On::Grow) | Some(On::Shrink) => {
                if let Some(splats) = self.select.splats.clone() {
                    // A step of three times the selection's median size (orbiters_reconstruct::select::step).
                    let radius = pick::step(&splats, &e.edit.marks);
                    let grow = acted == Some(On::Grow);
                    let ids = if grow { pick::grow(&splats, &e.edit.marks, radius) } else { pick::shrink(&splats, &e.edit.marks, radius) };
                    let mode = if grow { Mode::Add } else { Mode::Remove };
                    if !e.select(&ids, mode) {
                        self.select.notice = Some((if grow { tr("Nothing beside the selection to add") } else { tr("Nothing to take away") }.into(), ctx.input(|i| i.time)));
                    }
                }
            }
            Some(On::Hide) => {
                e.remove(HIDDEN);
            }
            Some(On::Delete) => {
                e.remove(DELETED);
            }
            Some(On::Clear) => {
                e.clear();
            }
            None => {}
        }
        match clicked {
            Some(Whole::Invert) => {
                e.invert();
            }
            Some(Whole::SelectAll) => {
                e.select_all();
            }
            Some(Whole::ShowAll) => {
                e.show_all();
            }
            Some(Whole::RestoreAll) => {
                e.restore_all();
            }
            None => {}
        }
    }

    /// An edit that could not be applied: under the top bar, why, and Discard them. Returns whether one is shown.
    pub(super) fn problem_chip(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect, title_r: Rect) -> bool {
        let Some(e) = self.loaded.as_mut().and_then(|l| l.edits.as_mut()) else { return false };
        let Some(problem) = e.problem.as_ref().map(Problem::text) else { return false };
        let t = Txt::new(&problem, 12.5).color(theme::fg());
        let tw = widgets::text_size(ui, &t).x.min(rect.width() - 260.0);
        let b = Button::new(tr("Discard them")).icon(icons::TRASH).variant(Variant::Danger).size(Size::Sm).id_salt("splat-edits-discard");
        let bw = b.desired_width(ui);
        let chip_r = Rect::from_center_size(pos2(rect.center().x, title_r.bottom() + 16.0 + 22.0), vec2(42.0 + tw + 12.0 + bw + 8.0, 44.0));
        p.rect_filled(chip_r, theme::radius(22.0), theme::black(0.72));
        widgets::icon(p, pos2(chip_r.left() + 26.0, chip_r.center().y), icons::WARNING, 16.0, theme::WARN);
        widgets::text_truncated(p, pos2(chip_r.left() + 42.0, chip_r.center().y), Align2::LEFT_CENTER, &t, tw);
        if b.confirm_at(ui, Rect::from_min_size(pos2(chip_r.right() - 8.0 - bw, chip_r.center().y - 16.0), vec2(bw, 32.0))) {
            if let Err(err) = e.discard() {
                e.problem = Some(Problem::NotDiscarded(format!("{err:#}")));
            }
        }
        true
    }
}

/// The selection's colour on the screen (project.wgsl's SELECTED).
const SELECTION: Color32 = Color32::from_rgb(255, 199, 41);

fn metres(m: f32) -> String {
    if m < 1.0 { format!("{:.0} cm", m * 100.0) } else { format!("{m:.2} m") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At Enzo's window the brush's slider narrows to stay clear of the chip at the right, and only below the narrowest
    /// that reads does that chip move up a row; the other tools stay on one row down to the window's minimum and below.
    #[test]
    fn select_splats_bottom_row_never_overlaps() {
        // As measured on the real view: the tools 276 wide, the chip at the right 412.
        let (tools, whole) = (276.0, 412.0);
        for width in [720.0, 800.0, 900.0, 1030.0, 1280.0, 1920.0, 3840.0] {
            for brush in [false, true] {
                let b = Bottom::of(width, tools, whole, brush);
                let tools_right = 16.0 + tools + b.slider_room();
                let whole_left = width - 16.0 - whole;
                assert!(b.stacked || tools_right + 10.0 <= whole_left + 0.001, "{width} {brush}: {tools_right} into {whole_left}");
                assert!(tools_right <= width - 16.0 + 0.001, "{width} {brush}: past the view's edge");
                assert_eq!(b.slider > 0.0, brush);
                if brush {
                    assert!((SLIDER_MIN..=SLIDER_MAX).contains(&b.slider), "{width}: {}", b.slider);
                }
            }
        }
        assert!(!Bottom::of(1030.0, tools, whole, true).stacked, "at 1030 it fits narrowed");
        assert!(Bottom::of(900.0, tools, whole, true).slider < SLIDER_MAX, "narrowed before it moves");
        assert_eq!(Bottom::of(1920.0, tools, whole, true), Bottom { slider: SLIDER_MAX, stacked: false });
        assert!(!Bottom::of(1030.0, tools, whole, false).stacked);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-select-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(reconstruction::dir(&dir)).unwrap();
        dir
    }

    /// A session of one keyframe looking down +Z (64 × 36 at the training's size), a grey wall at 4 m and a red
    /// "avatar" in front of it, with the worker's picture (all grey) and report.
    fn session(dir: &Path) -> (Gaussians, std::ops::Range<usize>) {
        use orbiters_core::contracts::frame_record::{FrameRecord, record_flags};
        let mut g = Gaussians::default();
        for y in -12..=12 {
            for x in -20..=20 {
                g.push([x as f32 * 0.15, y as f32 * 0.15, 4.0], [(0.12f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], 3.0, [0.5; 3]);
            }
        }
        let avatar = g.len()..g.len() + 25;
        for y in 0..5 {
            for x in 0..5 {
                g.push([x as f32 * 0.08 - 0.16, y as f32 * 0.08 - 0.16, 2.0], [(0.06f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], 3.0, [1.0, 0.0, 0.0]);
            }
        }
        let result = reconstruction::dir(dir);
        orbiters_reconstruct::ply::write(&result.join(reconstruction::PLY_FILE), &g).unwrap();
        let record = FrameRecord {
            flags: record_flags::STRIP_OK | record_flags::KEYFRAME,
            camera_to_world: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            projection: [1.2, 2.1, 0.0, 0.0],
            width: 128,
            height: 72,
            keyframe_index: 3,
            ..Default::default()
        };
        std::fs::write(dir.join(orbiters_core::contracts::manifest::FRAMES_FILE), record.to_bytes()).unwrap();
        let report = reconstruction::Report { width: 64, height: 36, held_out_keyframes: vec![3], ..Default::default() };
        std::fs::write(result.join(reconstruction::REPORT_FILE), serde_json::to_vec(&report).unwrap()).unwrap();
        std::fs::write(result.join(reconstruction::PREVIEW_FILE), orbiters_core::contracts::qoi::encode(&[128u8; 64 * 36 * 3], 64, 36, 3)).unwrap();
        (g, avatar)
    }

    /// A device for a test, DX12 only, with the GPU held for that test alone while the guard lives (night5: these tests
    /// stalled for ever when they ran at once, each on its own device of every backend).
    fn device(what: &str) -> Option<((wgpu::Device, wgpu::Queue), std::sync::MutexGuard<'static, ()>)> {
        crate::gpu::testing::device(what).map(|(d, q, held)| ((d, q), held))
    }

    fn reds(session: &Path) -> usize {
        let (_, _, rgba) = reconstruction::read_preview(session).expect("a picture");
        rgba.chunks(4).filter(|p| p[0] > 150 && p[1] < 100).count()
    }

    fn stamp(dir: &Path) -> Option<Stamp> {
        Stamp::of(&reconstruction::dir(dir).join(reconstruction::PLY_FILE))
    }

    /// The edits a view would open with now.
    fn open(dir: &Path, window: &(wgpu::Device, wgpu::Queue)) -> Edits {
        let (g, identity, found) = edit::read_for_view(&reconstruction::dir(dir).join(reconstruction::PLY_FILE)).unwrap();
        Edits::new(dir, g.len(), identity, found, window.clone())
    }

    /// The capture's picture: drawn from the worker's camera with the avatar where it was, then again without it once
    /// it is deleted (the file says the picture is drawn), the worker's own kept aside under its PLY's name; with every
    /// splat deleted, the background alone; restored once nothing is removed. Saves of a view whose PLY was replaced
    /// meanwhile are refused. Opens its own GPU device.
    #[test]
    fn the_picture_is_drawn_again_without_what_is_removed() {
        let Some((window, _gpu)) = device("the_picture_is_drawn_again_without_what_is_removed") else { return };
        let dir = temp("picture");
        let (g, avatar) = session(&dir);
        let result = reconstruction::dir(&dir);
        let ply = result.join(reconstruction::PLY_FILE);
        let camera = edit::preview_camera(&dir).expect("the worker's camera");
        assert_eq!((camera.width, camera.height), (64, 36));
        // No edits: nothing to draw.
        assert!(!redraw_picture(&dir, window.clone()).unwrap());
        // An edit hiding a wall splat off to the side: the avatar shows.
        let identity = Identity::read(&ply).unwrap();
        let trained = result.join(reconstruction::trained_preview_file(identity.hash));
        let mut e = Edit::new(g.len(), identity);
        e.marks[0] = HIDDEN;
        e.picture = false;
        persist(&result, &e, stamp(&dir)).unwrap();
        assert_eq!(edit::picture_state(&result), edit::PictureState::NotDrawn);
        assert!(redraw_picture(&dir, window.clone()).unwrap());
        assert!(reds(&dir) > 10, "the avatar in the picture: {}", reds(&dir));
        assert_eq!(edit::picture_state(&result), edit::PictureState::Drawn);
        assert!(!redraw_picture(&dir, window.clone()).unwrap(), "drawn already");
        // Deleted: the picture has none of it; the worker's own kept aside.
        for i in avatar.clone() {
            e.marks[i] = DELETED;
        }
        persist(&result, &e, stamp(&dir)).unwrap();
        assert!(redraw_picture(&dir, window.clone()).unwrap());
        assert_eq!(reds(&dir), 0, "the avatar left the picture");
        assert_eq!(std::fs::read(&trained).unwrap(), orbiters_core::contracts::qoi::encode(&[128u8; 64 * 36 * 3], 64, 36, 3), "the worker's kept aside");
        // Everything deleted: the background alone (no sky map here), drawn all the same.
        let mut all = e.clone();
        all.marks.iter_mut().for_each(|m| *m = DELETED);
        persist(&result, &all, stamp(&dir)).unwrap();
        assert!(redraw_picture(&dir, window.clone()).unwrap(), "everything deleted is drawn too");
        let (_, _, rgba) = reconstruction::read_preview(&dir).unwrap();
        assert!(rgba.chunks(4).all(|p| p[0] < 8 && p[1] < 8 && p[2] < 8), "the background alone");
        assert_eq!(edit::picture_state(&result), edit::PictureState::Drawn);
        // Nothing removed: the worker's picture back, the edit file gone.
        persist(&result, &Edit::new(g.len(), identity), stamp(&dir)).unwrap();
        assert!(!result.join(reconstruction::EDIT_FILE).exists() && !trained.exists());
        assert_eq!(reconstruction::read_preview(&dir).unwrap().2[..4], [128, 128, 128, 255]);
        // The PLY replaced under the view (a new reconstruction): its saves are refused, the file left as it is.
        let before = stamp(&dir);
        let mut other = g.clone();
        other.means[0][1] += 0.5;
        std::thread::sleep(std::time::Duration::from_millis(20));
        orbiters_reconstruct::ply::write(&ply, &other).unwrap();
        let refused = persist(&result, &e, before).unwrap_err().to_string();
        assert!(refused.contains("reconstructed again"), "{refused}");
        assert!(!result.join(reconstruction::EDIT_FILE).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The edits as the view holds them: selections are steps of undo but never saved; hide and delete are on the disk
    /// as soon as they are made (a share or a view opened next reads them), and so are undo and redo; a view closed and
    /// opened again at once has every removal, and adds to them; edits that cannot be applied block saving until
    /// discarded. Opens its own GPU device (the picture's).
    #[test]
    fn edits_are_on_the_disk_as_they_are_made() {
        let Some((window, _gpu)) = device("edits_are_on_the_disk_as_they_are_made") else { return };
        let dir = temp("edits");
        let (g, avatar) = session(&dir);
        let ply = reconstruction::dir(&dir).join(reconstruction::PLY_FILE);
        let removed = || edit::read_edited(&ply).unwrap().1;
        let mut e = open(&dir, &window);
        let ids: Vec<u32> = avatar.clone().map(|i| i as u32).collect();
        assert!(e.select(&ids, Mode::Replace));
        assert_eq!(e.selected(), 25);
        assert_eq!(e.drawn[avatar.start], 1, "drawn tinted");
        assert!(!reconstruction::dir(&dir).join(reconstruction::EDIT_FILE).exists(), "a selection is not saved");
        assert!(e.remove(DELETED));
        assert_eq!((e.selected(), e.deleted()), (0, 25));
        assert_eq!(e.drawn[avatar.start], 2, "not drawn");
        assert!(e.save_error.is_none(), "{:?}", e.save_error);
        assert_eq!(removed(), 25, "on the disk at once");
        e.undo();
        assert_eq!((e.selected(), e.deleted(), removed()), (25, 0, 0));
        e.redo();
        assert_eq!(removed(), 25);
        // Invert then hide: everything else hidden; Show all brings it back, the deleted avatar stays out.
        e.invert();
        assert_eq!(e.selected(), g.len() - 25);
        e.remove(HIDDEN);
        assert_eq!(removed(), g.len());
        e.show_all();
        assert_eq!((e.hidden(), e.deleted(), removed()), (0, 25, 25));
        // Closed and opened again at once: the avatar is still out, and a new removal adds to it.
        drop(e);
        let mut again = open(&dir, &window);
        assert_eq!(again.deleted(), 25, "every removal of the view closed");
        let wall: Vec<u32> = (0..10).collect();
        again.select(&wall, Mode::Replace);
        again.remove(DELETED);
        assert_eq!(removed(), 35, "the earlier removals kept");
        drop(again);
        // Damaged: refused, nothing saved over it until discarded, then the trained splats.
        std::fs::write(reconstruction::dir(&dir).join(reconstruction::EDIT_FILE), b"N4EDIT01 damaged").unwrap();
        let mut e = open(&dir, &window);
        assert!(e.problem.is_some() && e.edit.removes_nothing());
        e.select(&ids, Mode::Replace);
        e.remove(DELETED);
        assert_eq!(std::fs::read(reconstruction::dir(&dir).join(reconstruction::EDIT_FILE)).unwrap(), b"N4EDIT01 damaged", "never saved over");
        assert!(edit::read_edited(&ply).is_err(), "and shares stop");
        e.discard().unwrap();
        assert!(e.problem.is_none());
        assert_eq!(removed(), 0);
        drop(e);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A brush stroke cancelled halfway (Esc, another tool, Done) ends there as one step: a delete made next is a step of
    /// its own, and undoing it brings back the selection the stroke made.
    #[test]
    fn a_cancelled_stroke_is_one_step_of_its_own() {
        let Some((window, _gpu)) = device("a_cancelled_stroke_is_one_step_of_its_own") else { return };
        let dir = temp("stroke");
        let (_, avatar) = session(&dir);
        let mut e = open(&dir, &window);
        let mut s = State::default();
        let ids: Vec<u32> = avatar.map(|i| i as u32).collect();
        e.begin_stroke();
        s.drag = Some(Drag::Brush { mode: Mode::Add, last: Pos2::ZERO });
        e.select(&ids[..10], Mode::Add);
        e.select(&ids[10..], Mode::Add);
        stop_drag(&mut s, Some(&mut e));
        assert!(s.drag.is_none() && !e.history.is_open());
        e.remove(DELETED);
        assert_eq!(e.deleted(), 25);
        e.undo();
        assert_eq!((e.deleted(), e.selected()), (0, 25), "the delete undone alone: the stroke's selection back");
        e.undo();
        assert_eq!(e.selected(), 0, "then the whole stroke");
        drop(e);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The splats read again (the other GPU, a picture of the view) only from the PLY the view loaded: another
    /// reconstruction's are refused, never drawn with this view's marks.
    #[test]
    fn splats_are_read_again_only_from_the_ply_the_view_loaded() {
        let dir = temp("again");
        let (mut g, _) = session(&dir);
        let ply = reconstruction::dir(&dir).join(reconstruction::PLY_FILE);
        let loaded = Identity::read(&ply).unwrap();
        assert_eq!(read_again(&dir, Some(loaded)).unwrap().0.len(), g.len());
        g.means[3][0] += 1.0;
        orbiters_reconstruct::ply::write(&ply, &g).unwrap();
        assert!(read_again(&dir, Some(loaded)).unwrap_err().to_string().contains("reconstructed again"));
        assert!(read_again(&dir, Some(Identity::read(&ply).unwrap())).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A problem is kept as what happened and said in the language where it is drawn, so a language change reaches an open chip.
    #[test]
    fn a_problem_is_said_in_the_language_now() {
        use orbiters_i18n::{in_language, Language};
        let problem = Problem::NotDiscarded("access denied".into());
        assert_eq!(problem.text(), "They could not be discarded: access denied");
        assert_eq!(in_language(Language::Fr, || problem.text()), "Impossible de les abandonner\u{a0}: access denied");
        assert!(in_language(Language::Ja, || Problem::Stale.text()).starts_with("スプラットの編集"));
    }
}
