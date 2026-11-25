//! The generated matrix (second and third verifications): every place a trial, or a duplicate, can be asked to be made at ×
//! every way a step can fail, be cut short or be interfered with. After each run the original, everything it links to (its
//! Library on another disk, linked folders), its local package and what that links to, and another project are compared whole
//! (what each entry is: its identity, its bytes, its times and its permissions) with what they were, and where the trial or the
//! copy stands with what is expected. `ORBITERS_MATRIX_OUT` names a file that gets the table.

use super::journal;
use super::standing::tests::{project, U2022};
use super::trial::tests::{launch, look, *};
use super::trial::*;
use crate::copying::Step;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type Tree = BTreeMap<String, String>;

/// Everything under `root`, not through links: each entry's identity, permissions and (a file's, and a folder's when `times`)
/// time of change; a file by its bytes besides, a link by where it leads. What cannot be looked at is an error, never left out.
fn tree(root: &Path, skip: &[PathBuf], times: bool) -> Tree {
    let mut out = Tree::new();
    let walk = walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| !skip.iter().any(|s| e.path() == s));
    for e in walk {
        let e = e.unwrap();
        let m = std::fs::symlink_metadata(e.path()).unwrap();
        let what = if crate::owned::is_link(&m) {
            format!("link {:?} {:?} {:?}", std::fs::canonicalize(e.path()).ok(), times.then(|| m.modified().unwrap()), m.created().ok())
        } else {
            let id = crate::owned::id_of(e.path()).unwrap();
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&m.permissions());
            #[cfg(windows)]
            let mode = std::os::windows::fs::MetadataExt::file_attributes(&m);
            let changed = (m.is_file() || times).then(|| m.modified().unwrap());
            let bytes = if m.is_dir() { "folder".to_string() } else { journal::sha256(&std::fs::read(e.path()).unwrap()) };
            format!("{id:?} {mode:x} {changed:?} {:?} {bytes}", m.created().ok())
        };
        out.insert(e.path().strip_prefix(root).unwrap().display().to_string(), what);
    }
    out
}

fn differences(before: &Tree, after: &Tree) -> Vec<String> {
    before.iter().filter(|(k, v)| after.get(*k) != Some(v)).map(|(k, _)| k.clone()).chain(after.keys().filter(|k| !before.contains_key(*k)).cloned()).collect()
}

/// A temporary world: the original with a linked folder, a Library kept elsewhere and a package from outside; another project.
struct World {
    e: Env,
    shared: PathBuf,
    external: PathBuf,
    library: PathBuf,
    /// What its local package links to.
    nested: PathBuf,
    other: PathBuf,
    good: PathBuf,
}

fn world() -> World {
    let e = env();
    let w = World {
        shared: e.root.join("Shared assets"),
        external: external(&e, "AAAA"),
        library: e.root.join("Cache/Library"),
        nested: e.root.join("Shared by a package"),
        other: e.root.join("Projects/Other"),
        good: e.root.join("Good"),
        e,
    };
    for d in [&w.shared.join("Deep"), &w.nested.join("Deep"), &w.library, &w.good] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(w.shared.join("Deep/shared.mat"), "m").unwrap();
    std::fs::write(w.e.source.join("Assets/Big.bin"), "big").unwrap();
    std::fs::write(w.e.root.join("a file"), "x").unwrap();
    std::fs::write(w.nested.join("Deep/nested.mat"), "n").unwrap();
    project(&w.other, Some(U2022), &[], &[]);
    link(&w.shared, &w.e.source.join("Assets/Shared"));
    link(&w.nested, &w.external.join("Nested"));
    std::fs::remove_dir_all(w.e.source.join("Library")).unwrap();
    link(&w.library, &w.e.source.join("Library"));
    link(&w.good, &w.e.root.join("to good"));
    link(&w.e.source.join("Assets"), &w.e.root.join("to assets"));
    link(&w.e.root.join("to assets"), &w.e.root.join("to to assets"));
    link(&w.shared, &w.e.root.join("to shared"));
    link(&w.e.source, &w.good.join("to original"));
    std::fs::create_dir_all(w.good.join("not empty")).unwrap();
    std::fs::write(w.good.join("not empty/x"), "x").unwrap();
    std::fs::create_dir_all(w.good.join("empty")).unwrap();
    std::fs::write(w.good.join("file"), "x").unwrap();
    w
}

/// The same world, whose original names a package that is not installed.
fn incomplete_world() -> World {
    let w = world();
    let manifest = w.e.source.join("Packages/vpm-manifest.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    v["locked"]["gone.package"] = serde_json::json!({ "version": "1.0.0", "dependencies": {} });
    std::fs::write(&manifest, v.to_string()).unwrap();
    w
}

impl World {
    /// All of it but app data, VCC's folder and what is at `trial`.
    fn tree(&self, trial: &Path) -> Tree {
        tree(&self.e.root, &[self.e.app.clone(), self.e.vcc.clone(), self.e.root.join("vcc-file"), trial.to_path_buf(), self.e.root.join("Editors")], false)
    }

    /// VCC's folder: nothing of a trial's is ever in it.
    fn store(&self) -> Vec<(PathBuf, Vec<u8>)> {
        store(&self.e)
    }

    /// What nothing may be written in, the times of its folders included (`times`): the original, and everything it uses outside it.
    fn guarded(&self, times: bool) -> Vec<Tree> {
        [&self.e.source, &self.shared, &self.external, &self.nested, &self.library, &self.other].map(|p| tree(p, &[], times)).into()
    }
}

/// A drive letter standing for a folder, taken away again whatever happens.
#[cfg(windows)]
struct Subst(char);
#[cfg(windows)]
impl Subst {
    fn of(folder: &Path) -> Option<Subst> {
        let letter = ('P'..='Z').rev().find(|l| !Path::new(&format!("{l}:\\")).exists())?;
        let done = std::process::Command::new("subst").arg(format!("{letter}:")).arg(folder).status().ok()?.success();
        done.then_some(Subst(letter))
    }
    fn path(&self) -> PathBuf {
        PathBuf::from(format!("{}:\\", self.0))
    }
}
#[cfg(windows)]
impl Drop for Subst {
    fn drop(&mut self) {
        let _ = std::process::Command::new("subst").arg(format!("{}:", self.0)).arg("/d").status();
    }
}

#[cfg(windows)]
fn unc(path: &Path) -> PathBuf {
    let text = path.display().to_string();
    PathBuf::from(format!(r"\\localhost\{}$\{}", &text[..1], &text[3..]))
}

/// The 8.3 spelling of `path`, when its disk keeps one that differs.
#[cfg(windows)]
fn short(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: a null-terminated path and a buffer of the length given.
    let n = unsafe { windows::Win32::Storage::FileSystem::GetShortPathNameW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut buffer)) } as usize;
    let out = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..n]));
    (n > 0 && out != path).then_some(out)
}

/// A place asked for: its name in the table, the parent and the name, and whether a trial may be made there.
struct Place {
    form: &'static str,
    parent: PathBuf,
    name: String,
    good: bool,
    /// Whether a duplicate may be made there (folders on the way are made for it).
    duplicate: bool,
}

fn places(w: &World) -> Vec<Place> {
    let mut out = vec![];
    let mut add = |form: &'static str, parent: PathBuf, name: &str, good: bool| out.push(Place { form, parent, name: name.to_string(), good, duplicate: good || form == "a folder that is not there" });
    let (src, root) = (w.e.source.clone(), w.e.root.clone());
    add("an ordinary folder", w.good.clone(), "T", true);
    add("beside the original", root.join("Projects"), "T", true);
    add("equal to the original", root.join("Projects"), "My avatar", false);
    add("inside the original", src.clone(), "T", false);
    add("inside its Assets", src.join("Assets"), "T", false);
    add("inside a folder of it that does not exist", src.join("Assets/New/Deeper"), "T", false);
    add("through .. into it", root.join("Good").join("..").join("Projects").join("My avatar").join("Assets"), "T", false);
    add("through .. to an ordinary folder", src.join("..").join("..").join("Good"), "T", false);
    add("inside a folder it links to", w.shared.clone(), "T", false);
    add("deep inside a folder it links to", w.shared.join("Deep"), "T", false);
    add("inside its linked folder, by the link", src.join("Assets/Shared"), "T", false);
    add("inside the disk its Library is kept on", w.library.clone(), "T", false);
    add("inside its local package's folder", w.external.clone(), "T", false);
    add("inside a folder its local package links to", w.nested.clone(), "T", false);
    add("deep inside a folder its local package links to", w.nested.join("Deep"), "T", false);
    add("inside its local package's linked folder, by the link", w.external.join("Nested"), "T", false);
    add("inside a folder that is not there of what its package links to", w.nested.join("New/Deeper"), "T", false);
    add("inside it, through a link", root.join("to assets"), "T", false);
    add("inside it, through two links", root.join("to to assets"), "T", false);
    add("inside a folder it links to, through another link", root.join("to shared"), "T", false);
    add("an ordinary folder through a link", root.join("to good"), "T", true);
    add("a link to the original", w.good.clone(), "to original", false);
    add("inside another Unity project", w.other.clone(), "T", false);
    add("inside another project's Assets", w.other.join("Assets"), "T", false);
    add("a relative folder", PathBuf::from("Good"), "T", false);
    add("a folder that is not there", root.join("Nowhere"), "T", false);
    add("a file for a folder", root.join("a file"), "T", false);
    add("an existing file", w.good.clone(), "file", false);
    add("a folder that is not empty", w.good.clone(), "not empty", false);
    add("an empty folder that is there", w.good.clone(), "empty", false);
    add("a name with a dot at its end", w.good.clone(), "T.", false);
    add("a name with a space at its end", w.good.clone(), "T ", false);
    add("a stream's name", w.good.clone(), "T:s", false);
    add("a name with a separator", w.good.clone(), "T/U", false);
    add("a parent's name", w.good.clone(), "..", false);
    add("a device's name", w.good.clone(), "CON", false);
    add("an empty name", w.good.clone(), "", false);
    #[cfg(windows)]
    {
        let verbatim = |p: &Path| PathBuf::from(format!(r"\\?\{}", p.display()));
        add("another case of the original", PathBuf::from(root.join("Projects").to_string_lossy().to_uppercase()), "MY AVATAR", false);
        add("inside it, in another case", PathBuf::from(src.join("Assets").to_string_lossy().to_uppercase()), "T", false);
        add("an ordinary folder in another case", PathBuf::from(w.good.to_string_lossy().to_uppercase()), "T", true);
        add("inside it, by \\\\?\\", verbatim(&src.join("Assets")), "T", false);
        add("an ordinary folder by \\\\?\\", verbatim(&w.good), "T", false);
        add("a parent with a dot at its end", PathBuf::from(format!("{}.", src.join("Assets").display())), "T", false);
        add("a parent with a space at its end", PathBuf::from(format!("{} ", src.join("Assets").display())), "T", false);
        add("inside it, by a device path", PathBuf::from(format!(r"\\.\{}", src.join("Assets").display())), "T", false);
        if let Some(s) = short(&src.join("Assets")) {
            add("inside it, by its 8.3 names", s, "T", false);
        }
        if let Some(s) = short(&src) {
            add("the original's own 8.3 name", root.join("Projects"), &s.file_name().unwrap().to_string_lossy(), false);
        }
        if unc(&w.good).exists() {
            add("inside it, over the network", unc(&src.join("Assets")), "T", false);
            add("an ordinary folder over the network", unc(&w.good), "T", false);
        }
    }
    out
}

/// How a run is interfered with, and where the job stands after, at a good place.
struct Trouble {
    what: &'static str,
    at: Option<Step>,
    act: Act,
    /// `None`: no job is recorded.
    then: Option<fn(&Status) -> bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Nothing,
    /// The `n`th look at whether Unity has the original open says yes.
    Open(usize),
    NoRoom,
    StopFirst,
    Stop,
    Fail,
    Crash,
    /// VCC's folder is a file: nothing can be listed.
    NoList,
    NoAppData,
    Locked,
    TrialAltered,
    TrialAdded,
    TrialRemoved,
    TrialLinked,
    OriginalEdited,
    OriginalAdded,
    OriginalRemoved,
}

fn troubles() -> Vec<Trouble> {
    fn waiting(s: &Status) -> bool {
        matches!(s, Status::Waiting { .. })
    }
    fn kept(s: &Status) -> bool {
        matches!(s, Status::Failed { kept: true })
    }
    fn gone(s: &Status) -> bool {
        matches!(s, Status::Failed { kept: false })
    }
    fn unfinished(s: &Status) -> bool {
        matches!(s, Status::Unfinished)
    }
    fn cut(s: &Status) -> bool {
        matches!(s, Status::Interrupted)
    }
    use Act::*;
    let t = |what, at, act, then| Trouble { what, at, act, then };
    let mut out = vec![
        t("nothing", None, Nothing, Some(waiting as fn(&Status) -> bool)),
        t("Unity has it open before", None, Open(1), None),
        t("Unity opens it once recorded", None, Open(2), Some(gone)),
        t("Unity opens it during the copy", None, Open(3), Some(kept)),
        t("no room", None, NoRoom, None),
        t("stopped at once", None, StopFirst, None),
        t("stopped, recorded", Some(Step::Recorded), Stop, Some(gone)),
        t("stopped, folder made", Some(Step::Made), Stop, Some(gone)),
        t("stopped, copied", Some(Step::Copied), Stop, Some(gone)),
        t("stopped too late", Some(Step::Compared), Stop, Some(waiting)),
        t("no list to write (a trial needs none)", None, NoList, Some(waiting)),
        t("no app data", None, NoAppData, None),
        t("a file that cannot be read", None, Locked, Some(kept)),
        t("the trial altered at the same size", Some(Step::Copied), TrialAltered, Some(kept)),
        t("a file added to the trial", Some(Step::Copied), TrialAdded, Some(kept)),
        t("a file of the trial removed", Some(Step::Copied), TrialRemoved, Some(kept)),
        t("a folder of the trial made a link to the original", Some(Step::Copied), TrialLinked, Some(kept)),
        t("the trial's Assets made a link before the copy", Some(Step::Made), TrialLinked, Some(kept)),
        t("the original edited after the copy", Some(Step::Copied), OriginalEdited, Some(kept)),
        t("a file added to the original", Some(Step::Copied), OriginalAdded, Some(kept)),
        t("a file of the original removed", Some(Step::Copied), OriginalRemoved, Some(kept)),
        // What was read and what is there at the end are the same: a whole copy of the original as it is now.
        t("the original edited before the copy", Some(Step::Recorded), OriginalEdited, Some(waiting)),
        t("a file added to the original before the copy", Some(Step::Recorded), OriginalAdded, Some(kept)),
        // Third verification: what changes between verifying and the end of listing.
        t("altered once verified", Some(Step::Verified), TrialAltered, Some(kept)),
        t("altered before the end", Some(Step::MarkerGone), TrialAltered, Some(kept)),
        t("a file added before the end", Some(Step::MarkerGone), TrialAdded, Some(kept)),
        t("a file removed before the end", Some(Step::MarkerGone), TrialRemoved, Some(kept)),
    ];
    for (step, failed, crashed) in [
        (Step::Recorded, gone as fn(&Status) -> bool, gone as fn(&Status) -> bool),
        (Step::Made, kept, cut),
        (Step::Folders, kept, cut),
        (Step::Copied, kept, cut),
        (Step::Compared, kept, cut),
        (Step::Verified, unfinished, unfinished),
        (Step::MarkerGone, unfinished, unfinished),
        (Step::Checked, unfinished, unfinished),
    ] {
        out.push(Trouble { what: "an error", at: Some(step), act: Fail, then: Some(failed) });
        out.push(Trouble { what: "a crash", at: Some(step), act: Crash, then: Some(crashed) });
    }
    out
}

/// One run: what it answered, whether the trees are the same, and the job's state.
fn one(w: &World, place: &Place, trouble: &Trouble) -> (bool, Option<Status>) {
    let trial = std::fs::canonicalize(&place.parent)
        .map(|p| PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\").to_string()).join(&place.name))
        .unwrap_or_else(|_| w.e.root.join("no such place"));
    let trial = if place.good { trial } else { w.e.root.join("no such place") };
    let before = w.tree(&trial);
    let edits = matches!(trouble.act, Act::OriginalEdited | Act::OriginalAdded | Act::OriginalRemoved);
    // What the test itself edits of the original is put back: its folders then have new times.
    let guarded = w.guarded(!edits);
    let theirs = w.store();
    let cancel = Arc::new(AtomicBool::new(trouble.act == Act::StopFirst));
    let mine: Rc<Cell<Option<&'static str>>> = Rc::default();
    let (src, t, c, at, act, m) = (w.e.source.clone(), trial.clone(), cancel.clone(), trouble.at, trouble.act, mine.clone());
    let aside = w.e.root.join("Good/aside");
    let aside_too = aside.clone();
    let kept = std::fs::read(src.join("Assets/Avatar.unity")).unwrap();
    let time = std::fs::metadata(src.join("Assets/Avatar.unity")).unwrap().modified().unwrap();
    let hook = move |step: Step| -> anyhow::Result<()> {
        if Some(step) != at {
            return Ok(());
        }
        match act {
            Act::Stop => c.store(true, Ordering::Relaxed),
            Act::Fail => anyhow::bail!("made to fail"),
            Act::Crash => std::panic::panic_any("made to crash"),
            Act::TrialAltered => std::fs::write(t.join("Assets/Big.bin"), "BIG").unwrap(),
            Act::TrialAdded => std::fs::write(t.join("Assets/Added.txt"), "x").unwrap(),
            Act::TrialRemoved => std::fs::remove_file(t.join("Assets/Big.bin")).unwrap(),
            Act::TrialLinked => {
                // Where its folders are held this fails, and its files are gone all the same.
                let _ = std::fs::remove_dir_all(t.join("Assets"));
                if std::fs::symlink_metadata(t.join("Assets")).is_err() {
                    link(&src.join("Assets"), &t.join("Assets"));
                }
            }
            Act::OriginalEdited => {
                m.set(Some("Assets/Avatar.unity"));
                std::fs::write(src.join("Assets/Avatar.unity"), "%YAML 1.1\nanother\n").unwrap();
            }
            Act::OriginalAdded => {
                m.set(Some("Assets/Theirs.txt"));
                std::fs::write(src.join("Assets/Theirs.txt"), "x").unwrap();
            }
            Act::OriginalRemoved => {
                m.set(Some("aside"));
                std::fs::rename(src.join("Assets/Avatar.unity"), &aside_too).unwrap();
            }
            _ => {}
        }
        Ok(())
    };
    let _stepping = at_step(hook);
    let looks = Cell::new(0);
    let open = |_: &Path| {
        looks.set(looks.get() + 1);
        matches!(trouble.act, Act::Open(n) if n == looks.get())
    };
    let budget = |_: &Path, _: u64| if trouble.act == Act::NoRoom { Err(anyhow::anyhow!("no room")) } else { Ok(10) };
    let blocked = w.e.root.join("vcc-file");
    std::fs::write(&blocked, "x").unwrap();
    if trouble.act == Act::NoList {
        // Where VCC's folder would be.
        std::fs::write(&w.e.vcc, "x").unwrap();
    }
    let nowhere = w.e.root.join("a file");
    let app = if trouble.act == Act::NoAppData { &nowhere } else { &w.e.app };
    let _held = (trouble.act == Act::Locked).then(|| kept_from_all(&w.e.source.join("Assets/Big.bin")));
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prepare(
            Prepare { dir: app, source: &w.e.source, parent: &place.parent, name: &place.name, target: target(), open: &open, budget: &budget, recorded: &|_| {} },
            &cancel,
            &Default::default(),
        )
    }));
    let went = matches!(done, Ok(Ok(_)));
    drop(_held);
    // What the test itself changed of the original, put back before comparing.
    match mine.get() {
        Some("aside") => std::fs::rename(&aside, w.e.source.join("Assets/Avatar.unity")).unwrap(),
        Some("Assets/Avatar.unity") => {
            std::fs::write(w.e.source.join("Assets/Avatar.unity"), &kept).unwrap();
            std::fs::File::options().write(true).open(w.e.source.join("Assets/Avatar.unity")).unwrap().set_modified(time).unwrap();
        }
        Some(added) => std::fs::remove_file(w.e.source.join(added)).unwrap(),
        None => {}
    }
    let changed = differences(&before, &w.tree(&trial));
    assert!(changed.is_empty(), "{} × {} ({:?}): {changed:?}", place.form, trouble.what, trouble.at);
    assert_eq!(w.guarded(!edits), guarded, "{} × {} ({:?})", place.form, trouble.what, trouble.at);
    if trouble.act == Act::NoList {
        assert_eq!(std::fs::read(&w.e.vcc).unwrap(), b"x");
        std::fs::remove_file(&w.e.vcc).unwrap();
    } else {
        assert!(w.store() == theirs && w.e.vcc.exists() != theirs.is_empty(), "{} × {}: Creator Companion's files", place.form, trouble.what);
    }
    let record = journal::read(&w.e.app);
    let status = record.jobs.last().map(|j| status(&w.e.app, &record, j));
    // Tidied for the next run: the trial's folder (through no link of it), the job.
    if let Some(j) = record.jobs.last() {
        assert!(super::authority::allow(&w.e.app, &trial, super::authority::Action::Packages).is_err() || !trial.exists(), "{} × {}: a trial nothing guards", place.form, trouble.what);
        let ready = matches!(status, Some(Status::Waiting { .. }));
        assert_eq!(super::trial::folders(&w.e.app).contains(&j.trial.path), ready, "{} × {}: in the gallery", place.form, trouble.what);
    }
    if std::fs::symlink_metadata(trial.join("Assets")).is_ok_and(|m| crate::owned::is_link(&m)) {
        unlink(&trial.join("Assets"));
    }
    let _ = std::fs::remove_dir_all(&trial);
    let _ = std::fs::remove_dir_all(&w.e.app);
    (went, status)
}

/// Panics of the runs that are made to crash are not the test's output.
fn quietly<T>(f: impl FnOnce() -> T) -> T {
    let before = std::panic::take_hook();
    let quiet: Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync> = Arc::from(before);
    let outer = quiet.clone();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().downcast_ref::<&str>() != Some(&"made to crash") {
            outer(info);
        }
    }));
    let out = f();
    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| quiet(info)));
    out
}

/// [`places`] of `w`, and (Windows) those a substituted drive makes, with what keeps those drives.
#[cfg_attr(not(windows), allow(unused_mut))]
fn all_places(w: &World) -> (Vec<Place>, Vec<Box<dyn std::any::Any>>) {
    let mut all = places(w);
    let mut kept: Vec<Box<dyn std::any::Any>> = vec![];
    #[cfg(windows)]
    let drives = [Subst::of(&w.e.source), Subst::of(&w.e.root), Subst::of(&w.shared)];
    #[cfg(windows)]
    {
        let mut add = |form, parent, good| all.push(Place { form, parent, name: "T".into(), good, duplicate: good });
        if let Some(d) = &drives[0] {
            add("inside it, by a substituted drive", d.path().join("Assets"), false);
            add("inside it, a substituted drive of it", d.path(), false);
        }
        if let Some(d) = &drives[1] {
            add("an ordinary folder by a substituted drive", d.path().join("Good"), true);
            add("inside it, by a substituted drive of its parent", d.path().join("Projects").join("My avatar"), false);
        }
        if let Some(d) = &drives[2] {
            add("inside a folder it links to, by a substituted drive", d.path(), false);
        }
        kept.push(Box::new(drives));
    }
    (all, kept)
}

#[test]
fn every_place_and_every_trouble_leaves_the_original_as_it_was() {
    let w = world();
    let (all, drives) = all_places(&w);
    let troubles = troubles();
    let mut rows: Vec<String> = vec![];
    let (mut runs, mut hostile) = (0, 0);
    quietly(|| {
        for place in &all {
            let mut answers: Vec<String> = vec![];
            for trouble in &troubles {
                let (went, status) = one(&w, place, trouble);
                runs += 1;
                let name = format!("{} × {} at {:?}", place.form, trouble.what, trouble.at);
                if place.good {
                    match trouble.then {
                        None => assert!(!went && status.is_none(), "{name}: {status:?}"),
                        Some(is) => assert!(status.as_ref().is_some_and(is), "{name}: {status:?}"),
                    }
                    answers.push(status.map_or("no job".into(), |s| s.name().to_string()));
                } else {
                    hostile += 1;
                    assert!(!went, "{name}: made");
                    assert!(status.is_none(), "{name}: recorded {status:?}");
                }
            }
            rows.push(match place.good {
                true => format!("| {} | made | {} |", place.form, answers.join(", ")),
                false => format!("| {} | refused, no job, nothing written | in all {} runs |", place.form, troubles.len()),
            });
        }
    });
    drop(drives);
    // Duplicate, as the page's and the bridge's call it: of a whole project, and of one whose packages are not all there, each in a
    // world of its own with every place of it. Counted where it is called.
    let mut copies: Vec<String> = vec![];
    let mut calls = Calls::default();
    quietly(|| {
        for lacking in [false, true] {
            let w = if lacking { incomplete_world() } else { world() };
            let (all, _drives) = all_places(&w);
            for place in &all {
                let mut answers: Vec<&str> = vec![];
                for trouble in duplicate_troubles().iter().filter(|t| t.incomplete == lacking) {
                    answers.push(one_duplicate(&w, place, trouble, &mut calls));
                }
                let of = if lacking { "packages missing" } else { "whole" };
                copies.push(match place.duplicate {
                    true => format!("| {} | {of} | made | {} |", place.form, answers.join("; ")),
                    false => format!("| {} | {of} | refused, nothing made, listed or written | in all {} calls |", place.form, answers.len()),
                });
            }
        }
    });
    assert_eq!(calls.made + calls.refused, calls.all);
    if let Some(out) = std::env::var_os("ORBITERS_MATRIX_OUT") {
        let mut text = format!(
            "{} places × {} troubles = {runs} runs of a trial ({hostile} at hostile places); Duplicate was called {} times ({} at hostile places, all refused; {} at good ones), counted at the call: {} troubles of a whole project and {} of one with packages missing, each at every place of a world of its own. After every run the original, what it links to, its Library elsewhere, its local package, what that links to and another project were what they were: identity, bytes, times (of change and creation), permissions of every entry, a link's own times and where it leads, an entry that cannot be looked at being an error.\n\n## Trial\n\nTroubles, in the order of the answers: {}\n\n| Place | Result | Job state for each trouble |\n|---|---|---|\n",
            all.len(),
            troubles.len(),
            calls.all,
            calls.refused,
            calls.made,
            duplicate_troubles().iter().filter(|t| !t.incomplete).count(),
            duplicate_troubles().iter().filter(|t| t.incomplete).count(),
            troubles.iter().map(|t| t.at.map_or(t.what.to_string(), |s| format!("{} ({s:?})", t.what))).collect::<Vec<_>>().join("; ")
        );
        text.push_str(&rows.join("\n"));
        let names = |lacking: bool| duplicate_troubles().iter().filter(|t| t.incomplete == lacking).map(|t| t.at.map_or(t.what.to_string(), |s| format!("{} ({s:?})", t.what))).collect::<Vec<_>>().join("; ");
        text.push_str(&format!("\n\n## Duplicate\n\nTroubles of a whole project, in the order of the answers: {}\n\nOf one with packages missing: {}\n\n| Place | Project | Result | What is there after each trouble |\n|---|---|---|---|\n{}\n", names(false), names(true), copies.join("\n")));
        std::fs::write(out, text).unwrap();
    }
}

/// How a duplicate is interfered with, and what is there after at a good place.
struct DuplicateTrouble {
    what: &'static str,
    at: Option<Step>,
    act: Act,
    /// The project whose packages are not all there, with no network to restore them.
    incomplete: bool,
    then: &'static str,
}

const READY: &str = "ready and listed";
const LISTED: &str = "listed, and what is missing said";
const PART: &str = "kept, never a project";
const CUT: &str = "listed, the run cut after that";
const WHOLE: &str = "whole, not listed, said";
const GONE: &str = "nothing";
const OTHERS: &str = "only what is not the copy's";

/// The calls of Duplicate, counted where it is called.
#[derive(Default)]
struct Calls {
    all: usize,
    refused: usize,
    made: usize,
}

fn duplicate_troubles() -> Vec<DuplicateTrouble> {
    use Act::*;
    let t = |what, at, act, incomplete, then| DuplicateTrouble { what, at, act, incomplete, then };
    let mut out = vec![
        t("nothing", None, Nothing, false, READY),
        t("stopped at once", None, StopFirst, false, GONE),
        t("stopped, folder made", Some(Step::Made), Stop, false, GONE),
        t("stopped, folders made", Some(Step::Folders), Stop, false, GONE),
        t("stopped too late", Some(Step::Copied), Stop, false, READY),
        t("no list to write", None, NoList, false, WHOLE),
        t("a file that cannot be read", None, Locked, false, GONE),
        t("its Assets made a link to the original before the copy", Some(Step::Made), TrialLinked, false, OTHERS),
        t("packages that cannot be restored", None, Nothing, true, LISTED),
        t("packages that cannot be restored, stopped too late", Some(Step::Listed), Stop, true, LISTED),
        t("packages that cannot be restored, stopped", Some(Step::Folders), Stop, true, GONE),
        t("packages that cannot be restored, no list to write", None, NoList, true, WHOLE),
        t("packages that cannot be restored, a file that cannot be read", None, Locked, true, GONE),
        t("restored packages, then an error", Some(Step::Restored), Fail, true, LISTED),
        t("restored packages, then a crash", Some(Step::Restored), Crash, true, CUT),
    ];
    for (step, then) in [(Step::Made, PART), (Step::Folders, PART), (Step::Copied, PART), (Step::MarkerGone, WHOLE), (Step::Listed, CUT)] {
        for incomplete in [false, true] {
            // (An error while files go in: what went in is taken back.)
            let failed = if matches!(step, Step::Made | Step::Folders) { GONE } else { then };
            out.push(t("an error", Some(step), Fail, incomplete, failed));
            out.push(t("a crash", Some(step), Crash, incomplete, then));
        }
    }
    out
}

/// One Duplicate, as the page's and the bridge's is made: what is there after.
fn one_duplicate(w: &World, place: &Place, trouble: &DuplicateTrouble, calls: &mut Calls) -> &'static str {
    let name = format!("Duplicate at {} × {} at {:?}", place.form, trouble.what, trouble.at);
    let source = &w.e.source;
    let copy = match place.duplicate {
        true => place.parent.join(&place.name),
        false => w.e.root.join("no such place"),
    };
    let made = std::fs::canonicalize(&place.parent).map(|p| p.join(&place.name)).unwrap_or_else(|_| copy.clone());
    let made = PathBuf::from(made.to_string_lossy().trim_start_matches(r"\\?\").to_string());
    let nowhere = w.e.root.join("Nowhere");
    let skip = if place.duplicate { if place.parent == nowhere { nowhere.clone() } else { made.clone() } } else { copy.clone() };
    let (before, guarded) = (w.tree(&skip), w.guarded(true));
    let cancel = Arc::new(AtomicBool::new(trouble.act == Act::StopFirst));
    let (c, t, src, at, act) = (cancel.clone(), copy.clone(), w.e.source.clone(), trouble.at, trouble.act);
    let _stepping = at_step(move |step| {
        if Some(step) != at {
            return Ok(());
        }
        match act {
            Act::Stop => c.store(true, Ordering::Relaxed),
            Act::Fail => anyhow::bail!("made to fail"),
            Act::Crash => std::panic::panic_any("made to crash"),
            Act::TrialLinked => link(&src.join("Assets"), &t.join("Assets")),
            _ => {}
        }
        Ok(())
    });
    let blocked = w.e.root.join("vcc-file");
    let full = (trouble.act == Act::NoList).then(|| crate::own::full_for(&blocked));
    let vcc = if trouble.act == Act::NoList { &blocked } else { &w.e.vcc };
    let held = (trouble.act == Act::Locked).then(|| kept_from_all(&w.e.source.join("Assets/Big.bin")));
    // Nothing is downloaded in a test: a restore fails, or (its own step's trouble) puts one package there.
    let restores = trouble.at == Some(Step::Restored);
    crate::templates::OFFLINE.with(|o| o.set(!restores));
    crate::templates::RESTORES.with(|r| *r.borrow_mut() = restores.then(|| ("gone.package".to_string(), "1.0.0".to_string())));
    let rt = crate::runtime().unwrap();
    calls.all += 1;
    if place.duplicate {
        calls.made += 1;
    } else {
        calls.refused += 1;
    }
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.block_on(crate::templates::duplicate(vcc, source, &place.parent, &place.name, Default::default(), &cancel, &Default::default()))
    }));
    crate::templates::OFFLINE.with(|o| o.set(false));
    crate::templates::RESTORES.with(|r| *r.borrow_mut() = None);
    drop((held, full));
    let changed = differences(&before, &w.tree(&skip));
    assert!(changed.is_empty(), "{name}: {changed:?}");
    assert_eq!(w.guarded(true), guarded, "{name}");
    // Nothing of a restore is left, whatever happened.
    assert!(crate::templates::stages_left(&crate::data_dir()).is_empty(), "{name}");
    let listed_now = listed(&w.e, &copy);
    // AGENTS.md: what stays and is not on the list is in Orbiters' own record, with where it is; nothing else is.
    let mine: Vec<_> = crate::copies::shown(&crate::data_dir()).into_iter().filter(|(c, _)| c.destination == copy || c.destination == made).collect();
    let is = crate::discovery::is_project(&copy);
    let there = std::fs::symlink_metadata(&copy).is_ok();
    let said = match &done {
        Ok(Ok(_)) => String::new(),
        Ok(Err(e)) => format!("{e:#}"),
        Err(_) => "crashed".into(),
    };
    let found = match (matches!(done, Ok(Ok(_))), there, is, listed_now) {
        (true, true, true, true) => READY,
        // An error after the list was written: the copy is whole, listed, and said where it is (a crash says nothing).
        (false, true, true, true) if said.contains("could not be restored") => LISTED,
        (false, true, true, true) => CUT,
        (false, true, true, false) => WHOLE,
        // (A crash leaves its record, whatever it left in the folder.)
        (false, true, false, false) if copy.join(crate::templates::INCOMPLETE).exists() || done.is_err() => PART,
        (false, true, false, false) => OTHERS,
        (false, false, false, false) => GONE,
        other => panic!("{name}: {other:?} {said}"),
    };
    match found {
        PART => assert!(matches!(mine[..], [(_, crate::copies::Status::Incomplete)]), "{name}: {mine:?}"),
        WHOLE => {
            // (A crash right after its marker went leaves it whole and unlisted too.)
            assert!(matches!(mine[..], [(_, crate::copies::Status::Unlisted)]), "{name}: {mine:?}");
            assert!(crate::copies::folders(&crate::data_dir()).contains(&copy), "{name}: in the gallery");
        }
        _ => assert!(mine.is_empty(), "{name}: {mine:?}"),
    }
    for (c, _) in mine {
        crate::copies::dismiss(&crate::data_dir(), &c.id).unwrap();
    }
    if place.duplicate {
        assert_eq!(found, trouble.then, "{name}: {said}");
        if matches!(found, LISTED | WHOLE | PART) && trouble.act != Act::Crash {
            assert!(said.contains(&copy.display().to_string()), "{name}: where it is: {said}");
        }
        if found == READY {
            let settings = std::fs::read_to_string(copy.join("ProjectSettings/ProjectSettings.asset")).unwrap_or_default();
            assert!((settings.contains("\"T\"") && copy.join("Packages/com.example.tool/Nested/Deep/nested.mat").is_file()), "{name}: {settings}");
        }
    } else {
        assert!(matches!(done, Ok(Err(_))), "{name}: made");
        assert_eq!(found, GONE, "{name}");
        assert!(!listed(&w.e, &made), "{name}");
    }
    if std::fs::symlink_metadata(copy.join("Assets")).is_ok_and(|m| crate::owned::is_link(&m)) {
        unlink(&copy.join("Assets"));
    }
    if place.duplicate {
        let _ = std::fs::remove_dir_all(if place.parent == nowhere { &nowhere } else { &made });
    }
    let _ = std::fs::remove_dir_all(&w.e.vcc);
    let _ = std::fs::remove_dir_all(crate::own::dir(&w.e.vcc));
    found
}

/// The later steps of a trial, each with its ways to fail: the original and all it can see stay as they were, and so does
/// the trial's own content where Orbiters has nothing to write in it.
#[test]
fn every_later_step_leaves_the_original_as_it_was() {
    use super::editors::Found;
    let w = world();
    let job = run_at(&w.e, &w.good, "T", &|_| false, &ok).unwrap();
    let trial = job.trial.path.clone();
    let before = w.tree(&trial);
    let guarded = w.guarded(true);
    let inside = tree(&trial, &[], true);
    let same = |what: &str| {
        assert_eq!(w.tree(&trial), before, "{what}");
        assert_eq!(w.guarded(true), guarded, "{what}");
        assert_eq!(tree(&trial, &[], true), inside, "{what}: the trial's own files");
    };
    assert!(again(&w.e, &job.id).is_err());
    same("finishing what is finished");
    let _ = super::look(&w.e.app, &[w.e.source.clone(), trial.clone()]);
    let _ = look(&w.e.app, &w.e.source, &w.good, "U", false, &AtomicBool::new(false));
    same("looking");
    let editor = exact(&w.e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    assert!(launch(&w.e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| panic!("started")).is_err());
    same("opening before the migration");
    for junk in [w.e.source.clone(), w.other.clone(), trial.clone(), w.shared.clone(), w.e.root.join("Nowhere")] {
        assert!(adopt_vcc_copy(&w.e.app, &job.id, &junk).is_err(), "{}", junk.display());
    }
    same("taking something else for the migrated copy");
    let copy = w.good.join("T-Migrated");
    copy_tree(&trial, &copy);
    vcc_migrates(&copy);
    let before = w.tree(&trial);
    adopt_vcc_copy(&w.e.app, &job.id, &copy).unwrap();
    assert_eq!(w.tree(&trial), before, "taking the migrated copy");
    let log = Cell::new(false);
    launch(&w.e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, at, l| {
        log.set(at == copy && l.starts_with(&w.e.app));
        Ok(None)
    })
    .unwrap();
    assert!(log.get());
    assert_eq!(w.tree(&trial), before, "opening it");
    abandon(&w.e.app, &job.id).unwrap();
    assert_eq!(w.tree(&trial), before, "taking it off");
    assert_eq!(tree(&trial, &[], true), inside, "nothing of it deleted");
    assert!(!w.e.vcc.exists(), "nothing of Creator Companion's, from the first step to the last");
    assert_eq!(w.guarded(true), guarded);
}
