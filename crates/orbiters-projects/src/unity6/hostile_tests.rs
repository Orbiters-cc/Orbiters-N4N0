//! The generated matrix (second verification): every place a trial can be asked to be made at × every way a step can fail, be
//! cut short or be interfered with. After each run the original, everything it links to, its local package and another
//! project are compared whole (every file's bytes, every folder, every link) with what they were, and the trial's recorded
//! state with the one expected. `ORBITERS_MATRIX_OUT` names a file that gets the table.

use super::journal;
use super::standing::tests::{project, U2022};
use super::trial::tests::*;
use super::trial::*;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type Tree = BTreeMap<String, String>;

/// Everything under `root`, not through links: a file by its bytes, a folder, a link by what it is.
fn tree(root: &Path, skip: &[PathBuf]) -> Tree {
    let mut out = Tree::new();
    let walk = walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| !skip.iter().any(|s| e.path() == s));
    for e in walk.flatten() {
        let m = std::fs::symlink_metadata(e.path()).unwrap();
        let what = if crate::owned::is_link(&m) {
            format!("link {:?}", std::fs::canonicalize(e.path()).ok())
        } else if m.is_dir() {
            "folder".to_string()
        } else {
            std::fs::read(e.path()).map_or("unreadable".into(), |b| journal::sha256(&b))
        };
        out.insert(e.path().strip_prefix(root).unwrap().display().to_string(), what);
    }
    out
}

/// A temporary world: the original with a linked folder, a Library kept elsewhere and a package from outside; another project.
struct World {
    e: Env,
    shared: PathBuf,
    external: PathBuf,
    library: PathBuf,
    other: PathBuf,
    good: PathBuf,
}

fn world() -> World {
    let e = env();
    let w = World {
        shared: e.root.join("Shared assets"),
        external: external(&e, "AAAA"),
        library: e.root.join("Cache/Library"),
        other: e.root.join("Projects/Other"),
        good: e.root.join("Good"),
        e,
    };
    for d in [&w.shared.join("Deep"), &w.library, &w.good] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(w.shared.join("Deep/shared.mat"), "m").unwrap();
    std::fs::write(w.e.source.join("Assets/Big.bin"), "big").unwrap();
    std::fs::write(w.e.root.join("a file"), "x").unwrap();
    project(&w.other, Some(U2022), &[], &[]);
    #[cfg(windows)]
    {
        junction::create(&w.shared, w.e.source.join("Assets/Shared")).unwrap();
        std::fs::remove_dir_all(w.e.source.join("Library")).unwrap();
        junction::create(&w.library, w.e.source.join("Library")).unwrap();
        junction::create(&w.good, w.e.root.join("to good")).unwrap();
        junction::create(w.e.source.join("Assets"), w.e.root.join("to assets")).unwrap();
        junction::create(w.e.root.join("to assets"), w.e.root.join("to to assets")).unwrap();
        junction::create(&w.shared, w.e.root.join("to shared")).unwrap();
        junction::create(&w.e.source, w.good.join("to original")).unwrap();
    }
    std::fs::create_dir_all(w.good.join("not empty")).unwrap();
    std::fs::write(w.good.join("not empty/x"), "x").unwrap();
    std::fs::create_dir_all(w.good.join("empty")).unwrap();
    std::fs::write(w.good.join("file"), "x").unwrap();
    w
}

impl World {
    /// All of it but app data, VCC's list and what is below `trial`.
    fn tree(&self, trial: &Path) -> Tree {
        tree(&self.e.root, &[self.e.app.clone(), self.e.vcc.clone(), self.e.root.join("vcc-file"), trial.to_path_buf(), self.e.root.join("Editors")])
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
}

fn places(w: &World) -> Vec<Place> {
    let mut out = vec![];
    let mut add = |form: &'static str, parent: PathBuf, name: &str, good: bool| out.push(Place { form, parent, name: name.to_string(), good });
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
        add("inside it, through a junction", root.join("to assets"), "T", false);
        add("inside it, through two junctions", root.join("to to assets"), "T", false);
        add("inside a folder it links to, through another junction", root.join("to shared"), "T", false);
        add("an ordinary folder through a junction", root.join("to good"), "T", true);
        add("a junction to the original", w.good.clone(), "to original", false);
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
    fn unlisted(s: &Status) -> bool {
        matches!(s, Status::Unlisted)
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
        t("no list to write", None, NoList, Some(unlisted)),
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
    ];
    for (step, failed, crashed) in [
        (Step::Recorded, gone as fn(&Status) -> bool, gone as fn(&Status) -> bool),
        (Step::Made, kept, cut),
        (Step::Copied, kept, cut),
        (Step::Compared, kept, cut),
        (Step::Verified, unlisted, unlisted),
        (Step::MarkerGone, unlisted, unlisted),
        (Step::Listed, unlisted, unlisted),
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
    let cancel = Arc::new(AtomicBool::new(trouble.act == Act::StopFirst));
    let mine: Rc<Cell<Option<&'static str>>> = Rc::default();
    let (src, t, c, at, act, m) = (w.e.source.clone(), trial.clone(), cancel.clone(), trouble.at, trouble.act, mine.clone());
    let kept = std::fs::read(src.join("Assets/Avatar.unity")).unwrap();
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
                #[cfg(windows)]
                {
                    let _ = std::fs::remove_dir_all(t.join("Assets"));
                    junction::create(src.join("Assets"), t.join("Assets")).unwrap();
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
                m.set(Some("Assets/Avatar.unity"));
                std::fs::remove_file(src.join("Assets/Avatar.unity")).unwrap();
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
    let vcc = if trouble.act == Act::NoList { &blocked } else { &w.e.vcc };
    let nowhere = w.e.root.join("a file");
    let app = if trouble.act == Act::NoAppData { &nowhere } else { &w.e.app };
    #[cfg(windows)]
    let _held = (trouble.act == Act::Locked).then(|| {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).share_mode(0).open(w.e.source.join("Assets/Big.bin")).unwrap()
    });
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prepare(
            Prepare { dir: app, vcc, source: &w.e.source, parent: &place.parent, name: &place.name, target: target(), open: &open, budget: &budget, recorded: &|_| {} },
            &cancel,
            &Default::default(),
        )
    }));
    let went = matches!(done, Ok(Ok(_)));
    #[cfg(windows)]
    drop(_held);
    // What the test itself changed of the original, put back before comparing.
    match mine.get() {
        Some("Assets/Avatar.unity") => std::fs::write(w.e.source.join("Assets/Avatar.unity"), &kept).unwrap(),
        Some(added) => std::fs::remove_file(w.e.source.join(added)).unwrap(),
        None => {}
    }
    let after = w.tree(&trial);
    let changed: Vec<_> = before.iter().filter(|(k, v)| after.get(*k) != Some(v)).map(|(k, _)| k).chain(after.keys().filter(|k| !before.contains_key(*k))).collect();
    assert!(changed.is_empty(), "{} × {} ({:?}): {changed:?}", place.form, trouble.what, trouble.at);
    let record = journal::read(&w.e.app);
    let status = record.jobs.last().map(|j| status(&w.e.app, &record, j));
    // Tidied for the next run: the trial's folder (through no link of it), the job.
    if let Some(j) = record.jobs.last() {
        assert!(super::authority::allow(&w.e.app, &trial, super::authority::Action::Packages).is_err() || !trial.exists(), "{} × {}: a trial nothing guards", place.form, trouble.what);
        let listed_now = listed(&w.e, &j.trial.path);
        let ready = matches!(status, Some(Status::Waiting { .. }));
        // Listed only when ready, or when it stopped between the list and the record of it.
        assert!(!ready || listed_now, "{} × {}", place.form, trouble.what);
        assert!(ready || !listed_now || trouble.at == Some(Step::Listed), "{} × {}: listed and not ready", place.form, trouble.what);
    }
    #[cfg(windows)]
    if std::fs::symlink_metadata(trial.join("Assets")).is_ok_and(|m| crate::owned::is_link(&m)) {
        junction::delete(trial.join("Assets")).unwrap();
    }
    let _ = std::fs::remove_dir_all(&trial);
    let _ = std::fs::remove_dir_all(&w.e.app);
    let _ = std::fs::remove_dir_all(&w.e.vcc);
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

#[test]
fn every_place_and_every_trouble_leaves_the_original_as_it_was() {
    let w = world();
    let mut all = places(&w);
    #[cfg(windows)]
    let drives = [Subst::of(&w.e.source), Subst::of(&w.e.root), Subst::of(&w.shared)];
    #[cfg(windows)]
    {
        let mut add = |form, parent, good| all.push(Place { form, parent, name: "T".into(), good });
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
    }
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
    // The Duplicate of the project page, at the same hostile places: never into what the original can see.
    let mut duplicates = 0;
    for place in all.iter().filter(|p| !p.good && p.parent.is_absolute()) {
        let destination = place.parent.join(&place.name);
        let before = [&w.e.source, &w.shared, &w.external, &w.library, &w.other].map(|p| tree(p, &[]));
        let _ = crate::templates::copy(&w.e.source, &destination, &AtomicBool::new(false), &Default::default());
        let after = [&w.e.source, &w.shared, &w.external, &w.library, &w.other].map(|p| tree(p, &[]));
        assert_eq!(before, after, "Duplicate at {}", place.form);
        duplicates += 1;
    }
    if let Some(out) = std::env::var_os("ORBITERS_MATRIX_OUT") {
        let mut text = format!(
            "{} places × {} troubles = {runs} runs ({hostile} at hostile places), {duplicates} Duplicate runs at the hostile places; the whole trees equal before and after every one.\n\nTroubles, in the order of the answers: {}\n\n| Place | Result | Job state for each trouble |\n|---|---|---|\n",
            all.len(),
            troubles.len(),
            troubles.iter().map(|t| t.at.map_or(t.what.to_string(), |s| format!("{} ({s:?})", t.what))).collect::<Vec<_>>().join("; ")
        );
        text.push_str(&rows.join("\n"));
        text.push('\n');
        std::fs::write(out, text).unwrap();
    }
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
    let inside = tree(&trial, &[]);
    let same = |what: &str| {
        assert_eq!(w.tree(&trial), before, "{what}");
        assert_eq!(tree(&trial, &[]), inside, "{what}: the trial's own files");
    };
    let blocked = w.e.root.join("vcc-file");
    std::fs::write(&blocked, "x").unwrap();
    assert!(again(&w.e, &job.id).is_err());
    same("finishing what is finished");
    let _ = super::look(&w.e.app, &[w.e.source.clone(), trial.clone()]);
    let _ = look(&w.e.app, &w.e.source, &w.good, "U", false, &AtomicBool::new(false));
    same("looking");
    let editor = exact(&w.e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    assert!(launch(&w.e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| panic!("started")).is_err());
    same("opening before the migration");
    for junk in [w.e.source.clone(), w.other.clone(), trial.clone(), w.shared.clone(), w.e.root.join("Nowhere")] {
        assert!(adopt_vcc_copy(&w.e.app, &w.e.vcc, &job.id, &junk).is_err(), "{}", junk.display());
    }
    same("taking something else for the migrated copy");
    assert!(abandon(&w.e.app, &blocked, &job.id).is_err());
    same("taking it off a list that cannot be written");
    let copy = w.good.join("T-Migrated");
    copy_tree(&trial, &copy);
    vcc_migrates(&copy);
    let before = w.tree(&trial);
    adopt_vcc_copy(&w.e.app, &w.e.vcc, &job.id, &copy).unwrap();
    assert_eq!(w.tree(&trial), before, "taking the migrated copy");
    let log = Cell::new(false);
    launch(&w.e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, at, l| {
        log.set(at == copy && l.starts_with(&w.e.app));
        Ok(None)
    })
    .unwrap();
    assert!(log.get());
    assert_eq!(w.tree(&trial), before, "opening it");
    abandon(&w.e.app, &w.e.vcc, &job.id).unwrap();
    assert_eq!(w.tree(&trial), before, "taking it off");
    assert_eq!(tree(&trial, &[]), inside, "nothing of it deleted");
    assert!(!listed(&w.e, &trial) && !listed(&w.e, &copy));
}
