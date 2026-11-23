pub use crate::bundled_templates::builtins;
use crate::copying::{self, between, Made, Step};
use crate::unity6::gate::{self, AppRoot, Destination, Parents};
use crate::unity6::budget::{self, Demand, Need, Room};
use crate::unity6::payload::{self, Making, Own, Payload};
use anyhow::{bail, Context};
use orbiters_i18n::{mark, tr};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
    pub unity: String,
    pub builtin: bool,
    /// Its package name (com.vrchat.template.avatar…).
    #[serde(default)]
    pub id: String,
    /// The cover Creator Companion shows for it, when VCC is installed.
    #[serde(default)]
    pub banner: Option<PathBuf>,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Progress {
    pub copied: usize,
    pub total: usize,
    pub file: String,
    pub phase: String,
}
pub type SharedProgress = Arc<Mutex<Progress>>;

pub fn list(root: &Path) -> Vec<Template> {
    let mut out = vec![];
    for (dir, builtin) in [("VRCTemplates", true), ("Templates", false)] { // i18n: not shown (folder names)
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() || std::fs::symlink_metadata(&p).ok().is_some_and(|m| crate::discovery::is_link(&m)) {
                continue;
            }
            let Ok(bytes) = std::fs::read(p.join("package.json")) else { continue };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
            let name = v["displayName"].as_str().or_else(|| v["name"].as_str()).unwrap_or(tr("Unity template")).to_string();
            let unity = std::fs::read_to_string(p.join("ProjectSettings/ProjectVersion.txt"))
                .ok()
                .and_then(|s| crate::discovery::parse_version(&s))
                .unwrap_or_default();
            let id = v["name"].as_str().unwrap_or_default().to_string();
            out.push(Template { name, path: p, unity, builtin, id, banner: None });
        }
    }
    if !out.iter().any(|t| t.builtin) {
        out.extend(builtins(&crate::data_dir().join("unity-templates/alcom-v1.9.3")).unwrap_or_default());
    }
    if let Some(app) = crate::vcc_app_dir() {
        for t in &mut out {
            t.banner = banner(&app, &t.id);
        }
    }
    order(&mut out);
    out
}

/// VRChat's own templates first, by id (the ones made here have translated names, so the order and the template the window opens on must not
/// follow the language), then the person's by name.
fn order(templates: &mut [Template]) {
    templates.sort_by(|a, b| (!a.builtin, if a.builtin { &a.id } else { &a.name }).cmp(&(!b.builtin, if b.builtin { &b.id } else { &b.name })));
}

/// The cover Creator Companion (installed in `app`) shows on a template's card: VRChat's own templates have theirs,
/// any other one VCC's "Custom" cover (as VCC does).
pub fn banner(app: &Path, id: &str) -> Option<PathBuf> {
    let file = match id {
        "com.vrchat.template.avatar" => "template-avatar-2022.png",
        "com.vrchat.template.world" => "template-world-2022.png",
        "com.vrchat.template.avatar.2019" => "template-avatar.png",
        "com.vrchat.template.world.2019" => "template-world-udon.png",
        _ => "template-custom.png",
    };
    Some(app.join("WebApp/Dist/template-covers").join(file)).filter(|p| p.is_file())
}

pub fn validate_name(name: &str) -> anyhow::Result<()> {
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if name.trim() != name
        || name.is_empty()
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || matches!(name, "." | "..")
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|s| stem.strip_prefix(s).is_some_and(|v| v.len() == 1 && v.chars().all(|c| c.is_ascii_digit())))
    {
        // Projects travel: the rule is Windows' on every computer, though only there is it the system's own.
        if cfg!(windows) {
            bail!("{}", tr("Choose a valid Windows folder name."));
        }
        bail!("{}", tr("Choose a folder name that also works on Windows: none of < > : \" / \\ | ? *, no space or dot at its end, not CON or NUL."));
    }
    Ok(())
}
/// What a copy of a project takes along besides its own files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keep {
    /// Library, which Unity otherwise makes again from the project's files when the copy is first opened (long for a
    /// large project).
    pub library: bool,
}

/// Left out of a copy: what Unity or the IDE make again at the project's top (Library unless `keep` asks for it,
/// Temp, Logs, obj, Visual Studio's .vs, the .csproj and .sln files), this computer's editor settings (UserSettings),
/// and Git's history and Node's packages wherever they are. A folder of the same name deeper in (an asset folder
/// called Library or Temp) is copied.
/// What a copy leaves out: [`excluded`] for a project (`project`), only Git's and Node's folders for a package folder
/// (PROJECTS-37: a package's own Library, Logs or Temp folder is part of it), and never a copy's marker at the top.
pub(crate) fn left_out(name: &str, depth: usize, dir: bool, keep: Keep, project: bool) -> bool {
    if depth == 1 && !dir && name == INCOMPLETE {
        return true;
    }
    if project {
        excluded(name, depth, dir, keep)
    } else {
        matches!(name.to_ascii_lowercase().as_str(), ".git" | "node_modules")
    }
}

fn excluded(name: &str, depth: usize, dir: bool, keep: Keep) -> bool {
    let n = name.to_ascii_lowercase();
    if matches!(n.as_str(), ".git" | "node_modules") {
        return true;
    }
    if depth != 1 {
        return false;
    }
    match n.as_str() {
        "library" => dir && !keep.library,
        "temp" | "logs" | "obj" | "usersettings" | ".vs" => dir,
        _ => !dir && (n.ends_with(".csproj") || n.ends_with(".sln")),
    }
}
/// The file a copy keeps at the top of its folder until every file is in (PROJECTS-4): a copy stopped, failed or cut by
/// a kill is then never taken for a whole project ([`crate::discovery::is_project`]). It holds the copy's own identity
/// ([`Marker`]); one copied into or restored in another folder names another destination and hides nothing.
pub const INCOMPLETE: &str = ".orbiters-incomplete-copy";

/// What [`INCOMPLETE`] holds: an id made by this copy (and kept in its memory), its destination, when it began.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub copy: String,
    pub destination: PathBuf,
    pub started: u64,
    /// The identity of the folder the copy made: a folder restored or moved to that place later is not it.
    #[serde(default)]
    pub folder: Option<crate::owned::FileId>,
}
impl Marker {
    /// The marker at the top of `folder`, if it is one.
    pub fn read(folder: &Path) -> Option<Marker> {
        serde_json::from_slice(&std::fs::read(folder.join(INCOMPLETE)).ok()?).ok()
    }
}

/// Copy into a new folder only, through its proof ([`crate::unity6::gate`]). Links and junctions in the source are followed:
/// what they lead to is copied as real files, so that the copy does not depend on them.
pub fn copy(source: &Path, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    copy_keeping(source, destination, Keep::default(), cancel, progress)
}

/// [`copy`], taking along what `keep` asks for: the files as they are, without the settings a project of its own gets.
pub fn copy_keeping(source: &Path, destination: &Path, keep: Keep, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    let (parent, name) = (destination.parent(), destination.file_name());
    let (parent, name) = parent.zip(name).with_context(|| tr("Choose a destination folder"))?;
    let stopped = |e| before_anything(e, destination, cancel);
    let payload = payload::scan(source_folder(source)?, Making { keep, ..Default::default() }, cancel).map_err(stopped)?;
    let place = Destination::check(parent, &name.to_string_lossy(), &payload.protected, Parents::Make)?;
    let filled = fill(&payload, place, destination, cancel, progress, &mut |_| Ok(())).map_err(unmarked)?;
    copying::unmark(&filled.root, filled.marker)
}

fn source_folder(source: &Path) -> anyhow::Result<&Path> {
    anyhow::ensure!(source.is_dir(), "{}", tr("The source folder no longer exists."));
    Ok(source)
}

/// A Stop before anything was made says so.
fn before_anything(e: anyhow::Error, destination: &Path, cancel: &AtomicBool) -> anyhow::Error {
    if cancel.load(Ordering::Relaxed) {
        return anyhow::anyhow!("{}", tr!("Copy stopped before anything was copied to {path}.", path = destination.display()));
    }
    e
}

/// A folder made and filled.
struct Filled {
    /// Held for the whole of the operation.
    new: gate::NewFolder,
    root: gate::WriteRoot,
    made: Made,
    /// Until it is taken away, it is not a project.
    marker: crate::owned::FileId,
}

/// Makes the folder and puts `payload` in it. `on_made`: told of the folder before anything is put in it. An error says whether
/// anything of it is left ([`Nothing`]: not).
fn fill(payload: &Payload, place: Destination, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress, on_made: &mut dyn FnMut(&gate::NewFolder) -> anyhow::Result<()>) -> anyhow::Result<Filled> {
    if cancel.load(Ordering::Relaxed) {
        return Err(before_anything(anyhow::anyhow!("{}", tr("Stopped.")), destination, cancel).context(Nothing));
    }
    let new = place.make().map_err(|e| anyhow::Error::new(e).context(tr("The destination already exists or cannot be created; choose a new project name.")).context(Nothing))?;
    let begun = new.root().map_err(anyhow::Error::new).and_then(|root| {
        anyhow::ensure!(!payload.protected.iter().any(|p| p.id == Some(new.id)), "{}", tr("Create the copy outside the source project."));
        on_made(&new).map(|()| root)
    });
    let root = match begun {
        Ok(root) => root,
        // Empty still: it goes.
        Err(e) if new.remove().unwrap_or(false) => return Err(e.context(Nothing)),
        Err(e) => return Err(e),
    };
    let marker = Marker { copy: uuid::Uuid::new_v4().simple().to_string(), destination: destination.to_path_buf(), started: crate::discovery::now(), folder: Some(new.id) };
    let mut made = Made::default();
    let filled = between(Step::Made).and_then(|()| copying::fill(payload, &root, &marker, &mut made, cancel, progress));
    match (filled, made.marker()) {
        (Ok(()), Some(marker)) => Ok(Filled { new, root, made, marker }),
        (Err(_), _) if cancel.load(Ordering::Relaxed) => {
            let said = anyhow::anyhow!("{}", copying::undo(root, new, made));
            Err(if std::fs::symlink_metadata(destination).is_err() { said.context(Nothing) } else { said })
        }
        (Err(e), _) => Err(e.context(tr!("The copy could not be finished: what was copied is kept at {path}, and is not listed as a project", path = destination.display()))),
        (Ok(()), None) => bail!("{}", tr("Stopped.")),
    }
}

/// Nothing of the copy was made, or nothing of it is left.
#[derive(Debug)]
struct Nothing;

impl std::fmt::Display for Nothing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr("Nothing of the copy is left."))
    }
}

/// An error in its own words, without [`Nothing`].
fn unmarked(e: anyhow::Error) -> anyhow::Error {
    if e.downcast_ref::<Nothing>().is_none() {
        return e;
    }
    anyhow::anyhow!("{}", e.chain().skip(1).map(|c| c.to_string()).collect::<Vec<_>>().join(": "))
}

/// The staging folders of no run at all (other tests' runs have theirs).
#[cfg(test)]
pub(crate) fn stages_left(app: &Path) -> Vec<OsString> {
    let left = || {
        let Ok(held) = AppRoot::existing(app) else { return vec![] };
        let free = |name: &OsString| {
            let lock = Stage::lock_of(&name.to_string_lossy());
            !app.join(&lock).exists() || held.try_lock(&lock).is_ok_and(|l| l.is_some())
        };
        held.folders(Path::new(STAGES)).into_iter().filter(|n| free(n)).collect()
    };
    let mut now: Vec<OsString> = left();
    // One between its folder and its lock going.
    for _ in 0..20 {
        if now.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        now = left();
    }
    now
}

/// Why a copy or a new project cannot be made in `parent`/`name`, as far as the place alone says (the source's own rules are asked
/// when it is made): for a window, before it offers to. Reads only.
pub fn refused_place(parent: &Path, name: &str) -> Option<String> {
    Destination::check(parent, name, &[], Parents::Make).err().map(|e| e.text())
}

/// A new project in `parent`/`name` from the template (or, with `duplicate`, the project) in `source`, its packages
/// restored and registered.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    root: &Path,
    source: &Path,
    parent: &Path,
    name: &str,
    unity: &str,
    duplicate: bool,
    cancel: &AtomicBool,
    progress: &SharedProgress,
) -> anyhow::Result<PathBuf> {
    make(root, source, parent, name, unity, duplicate.then_some(Keep::default()), cancel, progress).await
}

/// A copy of the project in `source` named `name` in `parent`, taking along what `keep` asks for: its packages
/// restored and registered in the list VCC and ALCOM share. Its scenes keep their blueprint IDs.
pub async fn duplicate(
    root: &Path,
    source: &Path,
    parent: &Path,
    name: &str,
    keep: Keep,
    cancel: &AtomicBool,
    progress: &SharedProgress,
) -> anyhow::Result<PathBuf> {
    let unity = std::fs::read_to_string(source.join("ProjectSettings/ProjectVersion.txt"))
        .ok()
        .and_then(|s| crate::discovery::parse_version(&s))
        .unwrap_or_default();
    make(root, source, parent, name, &unity, Some(keep), cancel, progress).await
}

#[allow(clippy::too_many_arguments)]
async fn make(
    root: &Path,
    source: &Path,
    parent: &Path,
    name: &str,
    unity: &str,
    copy: Option<Keep>,
    cancel: &AtomicBool,
    progress: &SharedProgress,
) -> anyhow::Result<PathBuf> {
    validate_name(name)?;
    if !parent.is_absolute() {
        bail!("{}", tr("Choose an absolute destination folder."));
    }
    let app = crate::data_dir();
    if copy.is_some() {
        use crate::unity6::authority::{allow, Action};
        allow(&app, source, Action::Duplicate).map_err(|why| anyhow::anyhow!("{why}"))?;
    }
    Stage::sweep(&app);
    // Everything is read first, and the source (with what it links to and takes packages from) is only ever read.
    let from = copy.map(|_| source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let guid = uuid::Uuid::new_v4().simple().to_string();
    let keep = copy.unwrap_or_default();
    // A copy without its Library has the packages only the Library holds as its own (AGENTS.md: installed dependencies, without
    // their repositories).
    let installed = copy.is_some() && !keep.library;
    let how = Making { keep, own: Some(Own { name, from: from.as_deref(), guid: &guid }), unity: Some(unity), installed, ..Default::default() };
    let destination = parent.join(name);
    let mut payload = payload::scan(source_folder(source)?, how, cancel).map_err(|e| before_anything(e, &destination, cancel))?;
    if let Some((name, path)) = payload.missing.first() {
        bail!("{}", tr!("Required local package {name} is unavailable at {path}", name = name, path = path.display()));
    }
    // Where a project says no version, its copy is given one.
    if matches!(payload.items.get(Path::new(payload::VERSION_FILE)), Some(payload::Item::Made(_))) && vrc_get_vpm::version::UnityVersion::parse(unity).is_none() {
        bail!("{}", tr("Choose a Unity editor version."));
    }
    // Nothing is written anywhere the source can see: not the copy, not what Orbiters and VCC keep.
    gate::apart(&[&app, root], &payload.protected)?;
    let place = Destination::check(parent, name, &payload.protected, Parents::Make)?;
    let free = place.free().with_context(|| tr("Cannot check available disk space. Choose an accessible destination."))?;
    anyhow::ensure!(free >= payload.bytes, "{}", tr!("Not enough disk space: free at least {bytes} more bytes before duplicating.", bytes = payload.bytes.saturating_sub(free)));
    // Recorded before anything of it is made, until it is on the list: a copy that stays is never lost from sight.
    let mut record = crate::copies::Held::begin(&app, source, &destination)?;
    let filled = fill(&payload, place, &destination, cancel, progress, &mut |new| record.made(new.id));
    let Filled { new, root: held, mut made, marker } = match filled {
        Ok(f) => f,
        Err(e) if e.downcast_ref::<Nothing>().is_some() => {
            record.done();
            return Err(unmarked(e));
        }
        Err(e) => {
            record.failed(&e);
            return Err(e);
        }
    };
    // Every file is in: a Stop pressed now no longer leaves a whole copy unlisted (AGENTS.md: a retained project is
    // visible); it is registered like any other, before anything that can fail.
    let path = if crate::owned::is_same(&destination, new.id) { destination } else { new.path.clone() };
    let listed: anyhow::Result<()> = async {
        record.whole()?;
        between(Step::Copied)?;
        copying::unmark(&held, marker)?;
        between(Step::MarkerGone)?;
        progress.lock().unwrap().phase = mark("Registering project").into();
        crate::vpm::register(root, &path, true).await
    }
    .await;
    if let Err(e) = listed {
        let e = e.context(tr!("The copy is at {path}, but registration failed", path = path.display()));
        record.failed(&e);
        return Err(e);
    }
    record.done();
    between(Step::Listed)?;
    // From here on every failure leaves a project the gallery lists, and says where it is.
    if let Some(why) = payload.late.take() {
        return Err(anyhow::anyhow!("{why}").context(tr!("The copy is listed at {path}, but its local packages could not be copied", path = path.display())));
    }
    // Duplication preserves installed package contents and exact locked versions, including packages whose repositories are no
    // longer available. Only genuinely missing packages (a template's) are restored: in a folder of Orbiters' own, and put in
    // the copy from there through its proof.
    if copy.is_none() || !payload.packages_complete().unwrap_or(false) {
        let missing = || tr!("The copy is listed at {path}, but missing packages could not be restored", path = path.display());
        progress.lock().unwrap().phase = mark("Restoring missing packages").into();
        let restored: anyhow::Result<()> = async {
            let stage = Stage::restore(root, &app, &path, &payload, &budget::Disks).await?;
            // The source is let go; the new folder is held to the end.
            copying::take_restored(&stage.path, &stage.names, &held, &mut made, progress)?;
            between(Step::Restored)
        }
        .await;
        restored.with_context(missing)?;
        // Never called ready unless what it needs is in it.
        anyhow::ensure!(installed_dependencies_complete(&path).unwrap_or(false), "{}", missing());
    }
    drop(new);
    Ok(path)
}

#[cfg(test)]
thread_local! {
    /// A test's network that is not there.
    pub(crate) static OFFLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A test's package that a restore brings, without VPM.
    pub(crate) static RESTORES: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}

/// What one missing package may take at most, in the folder it is restored in (downloaded and unpacked), in VCC's cache, and in the
/// copy: the largest of VRChat's and the community's are well below. (No list says how large a package is.)
const RESTORED: u64 = 384 * 1024 * 1024;
const CACHED: u64 = 128 * 1024 * 1024;
/// A template names a package or two, and those name others.
const FEWEST: u64 = 4;

/// Room on every disk a restore writes on, before anything is downloaded: app data, VCC's cache and the copy's own.
fn restore_room(root: &Path, app: &Path, copy: &Path, payload: &Payload, room: &dyn Room) -> anyhow::Result<()> {
    let manifest: serde_json::Value = payload.read(Path::new("Packages/vpm-manifest.json"))?.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut names: Vec<&String> = ["dependencies", "locked"].iter().flat_map(|s| manifest[*s].as_object().into_iter().flatten()).map(|(n, _)| n).collect();
    names.sort();
    names.dedup();
    let missing = names.iter().filter(|n| !payload.items.contains_key(&Path::new("Packages").join(n).join("package.json"))).count() as u64; // i18n: not shown (a folder's name)
    let count = missing.max(FEWEST);
    let demand = |need, at: &Path, bytes| Demand { need, at: at.to_path_buf(), bytes, low: None };
    let demands = vec![demand(Need::Download, app, count * RESTORED), demand(Need::Download, root, count * CACHED), demand(Need::Copy, copy, count * RESTORED)];
    for v in budget::plan(demands, room, false).volumes {
        anyhow::ensure!(
            v.fits(),
            "{}",
            tr!(
                "Restoring missing packages may need {need} MB on the disk of {path}, which has {free} MB free.",
                need = v.total() / 1_000_000,
                path = v.at.display(),
                free = v.free.map_or_else(|| tr("unknown").to_string(), |f| (f / 1_000_000).to_string())
            )
        );
    }
    Ok(())
}

/// Where missing packages are restored: a folder of Orbiters' own, laid out as a project that has the copy's package lists and
/// nothing else. It goes when this is dropped; one a crash left goes at the next.
struct Stage {
    app: PathBuf,
    name: String,
    path: PathBuf,
    lock: Option<gate::Lock>,
    /// The packages restored in it.
    names: Vec<OsString>,
}

const STAGES: &str = "project-stage";

impl Stage {
    fn lock_of(name: &str) -> PathBuf {
        Path::new(STAGES).join(format!("{name}.lock"))
    }

    /// Takes away what runs that are gone left.
    fn sweep(app: &Path) {
        let Ok(held) = AppRoot::existing(app) else { return };
        for left in held.files(Path::new(STAGES)) {
            let Some(name) = left.to_str().and_then(|n| n.strip_suffix(".lock")) else { continue };
            // Nobody holds it: the process that made it is gone.
            if let Ok(Some(lock)) = held.try_lock(&Self::lock_of(name)) {
                drop(Stage { app: app.into(), name: name.into(), path: PathBuf::new(), lock: Some(lock), names: vec![] });
            }
        }
    }

    fn make(app: &Path) -> anyhow::Result<Self> {
        let held = AppRoot::open(app)?;
        let name = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let lock = held.lock(&Self::lock_of(&name))?;
        held.make_dir(&Path::new(STAGES).join(&name))?;
        Ok(Stage { app: app.into(), path: app.join(STAGES).join(&name), name, lock: Some(lock), names: vec![] })
    }

    /// The packages `payload` is missing, restored.
    async fn restore(root: &Path, app: &Path, copy: &Path, payload: &Payload, room: &dyn Room) -> anyhow::Result<Self> {
        restore_room(root, app, copy, payload, room)?;
        #[cfg(test)]
        if OFFLINE.with(|o| o.get()) {
            let _made = Self::make(app)?;
            bail!("offline");
        }
        #[cfg(test)]
        if let Some((name, version)) = RESTORES.with(|r| r.borrow().clone()) {
            // What VPM would leave there, without it.
            let mut stage = Self::make(app)?;
            let held = AppRoot::existing(app)?;
            let at = Path::new(STAGES).join(&stage.name).join("Packages");
            held.write_new(&at.join(&name).join("package.json"), serde_json::json!({ "name": name, "version": version }).to_string().as_bytes())?;
            held.write_new(&at.join(&name).join("Runtime/new.txt"), b"new")?;
            held.write_new(&at.join("vpm-manifest.json"), &payload.read(Path::new("Packages/vpm-manifest.json"))?.unwrap_or_default())?;
            stage.names.push(name.into());
            return Ok(stage);
        }
        let mut stage = Self::make(app)?;
        let held = AppRoot::existing(app)?;
        let mut packages = vec![];
        for at in payload.items.keys() {
            let names: Vec<_> = at.iter().collect();
            let wanted = match names[..] {
                [a, b] => a == "Packages" && (b == "manifest.json" || b == "vpm-manifest.json") || at.as_path() == Path::new(payload::VERSION_FILE), // i18n: not shown (a folder's name)
                [a, _, c] => a == "Packages" && c == "package.json", // i18n: not shown (a folder's name)
                _ => false,
            };
            let Some(bytes) = wanted.then(|| payload.read(at)).transpose()?.flatten() else { continue };
            held.write_new(&Path::new(STAGES).join(&stage.name).join(at), &bytes)?;
            if names.len() == 3 {
                packages.push((names[1].to_os_string(), bytes));
            }
        }
        crate::vpm::change(root, &stage.path, crate::vpm::Change::Resolve).await?;
        // What holds only the list put there was there already.
        for e in std::fs::read_dir(stage.path.join("Packages"))? {
            let e = e?;
            let Ok(list) = std::fs::read(e.path().join("package.json")) else { continue };
            let alone = std::fs::read_dir(e.path())?.count() == 1;
            let name = e.file_name();
            if !(alone && packages.iter().any(|(n, was)| *n == name && *was == list)) {
                stage.names.push(name);
            }
        }
        Ok(stage)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let Ok(held) = AppRoot::existing(&self.app) else { return };
        let gone = held.remove_tree(&Path::new(STAGES).join(&self.name));
        drop(self.lock.take());
        // What could not go is tried again at the next copy: its lock names it.
        if gone.is_ok() || !self.app.join(STAGES).join(&self.name).exists() {
            let _ = held.remove_file(&Self::lock_of(&self.name));
        }
    }
}

/// Fresh copy estimate using the same exclusions and link traversal as the copier, including local packages.
pub fn duplication_bytes(source: &Path, keep: Keep) -> anyhow::Result<u64> {
    fn bytes(path: &Path, keep: Keep, project: bool) -> anyhow::Result<u64> {
        let mut total = 0u64;
        for e in walkdir::WalkDir::new(path).follow_links(true).into_iter()
            .filter_entry(|e| !left_out(&e.file_name().to_string_lossy(), e.depth(), e.file_type().is_dir(), keep, project)) {
            let e = e?;
            if e.file_type().is_file() { total = total.saturating_add(e.metadata()?.len()); }
        }
        Ok(total)
    }
    let mut total = bytes(source, keep, true)?;
    let manifest = source.join("Packages/manifest.json");
    if manifest.is_file() {
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
        for (name, value) in manifest["dependencies"].as_object().into_iter().flatten() {
            if let Some(local) = value.as_str().and_then(|s| s.strip_prefix("file:")) {
                let at = source.join("Packages").join(local);
                let path = at.canonicalize().with_context(|| tr!("Required local package {name} is unavailable at {path}", name = name, path = at.display()))?;
                // As the copier does: a folder unless the project has one of that name already, an archive always.
                if path.is_file() {
                    total = total.saturating_add(std::fs::metadata(&path)?.len());
                } else if !source.join("Packages").join(name).exists() {
                    total = total.saturating_add(bytes(&path, Keep::default(), false)?);
                }
            }
        }
    }
    Ok(total)
}

/// The Unity packages of `project` that only its Library holds (PROJECTS-22): those `Packages/packages-lock.json` takes
/// from Git or from a registry other than Unity's, and that are not in `Packages/` itself. A copy made without Library
/// has them only after Unity downloads them again from their source, so it is not "ready" while that is unsure.
pub fn packages_only_in_library(project: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(project.join("Packages/packages-lock.json")) else { return vec![] };
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return vec![] };
    let mut names: Vec<String> = lock["dependencies"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, entry)| {
            let from_elsewhere = match entry["source"].as_str() {
                Some("git") => true,
                Some("registry") => entry["url"].as_str().is_some_and(|u| !u.trim_end_matches('/').eq_ignore_ascii_case("https://packages.unity.com")),
                _ => false,
            };
            from_elsewhere && !name.contains(['/', '\\']) && !project.join("Packages").join(name).is_dir()
        })
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

/// Check every locked dependency, including transitive dependencies, against the copied package metadata.
pub(crate) fn installed_dependencies_complete(project: &Path) -> anyhow::Result<bool> {
    dependencies_complete(|rel| match std::fs::read(project.join(rel)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => Ok(None),
        Err(e) => Err(e.into()),
    })
}

/// [`installed_dependencies_complete`] of the project whose files `read` gives.
pub(crate) fn dependencies_complete(read: impl Fn(&str) -> anyhow::Result<Option<Vec<u8>>>) -> anyhow::Result<bool> {
    let Some(manifest) = read("Packages/vpm-manifest.json")? else { return Ok(true) };
    let manifest: serde_json::Value = serde_json::from_slice(&manifest)?;
    let locked = manifest["locked"].as_object();
    if let Some(dependencies) = manifest["dependencies"].as_object() {
        if dependencies.keys().any(|name| !locked.is_some_and(|l| l.contains_key(name))) { return Ok(false); }
    }
    for (name, package) in locked.into_iter().flatten() {
        anyhow::ensure!(!name.contains(['/', '\\']) && name != "..", "{}", tr!("Invalid package name: {name}", name = name));
        let Ok(Some(bytes)) = read(&format!("Packages/{name}/package.json")) else { return Ok(false) };
        let installed: serde_json::Value = serde_json::from_slice(&bytes)?;
        if installed["name"].as_str() != Some(name) || installed["version"] != package["version"] { return Ok(false); }
    }
    Ok(true)
}

/// A project's `ProjectSettings.asset` (text) for a new project named `name`: a product GUID of its own, and its
/// product name (under which the editor keeps the project's PlayerPrefs) changed to `name` when it was `from` (the
/// copied project's name), or always without `from` (a template's). The other lines and every line's ending stay.
pub fn retarget(settings: &str, name: &str, from: Option<&str>) -> String {
    retarget_with(settings, name, from, &uuid::Uuid::new_v4().simple().to_string())
}

/// [`retarget`] with the product GUID to give it.
pub(crate) fn retarget_with(settings: &str, name: &str, from: Option<&str>, guid: &str) -> String {
    let mut out = String::with_capacity(settings.len() + 64);
    for line in settings.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let end = &line[body.len()..];
        let trimmed = body.trim_start();
        let indent = &body[..body.len() - trimmed.len()];
        if trimmed.starts_with("productGUID:") {
            out.push_str(&format!("{indent}productGUID: {guid}{end}"));
        } else if trimmed.strip_prefix("productName:").is_some_and(|v| from.is_none_or(|f| unquote(v.trim()) == f)) {
            out.push_str(&format!("{indent}productName: {}{end}", serde_json::to_string(name).unwrap()));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A YAML scalar as Unity writes it: plain, 'single' or "double" quoted.
fn unquote(s: &str) -> String {
    if s.starts_with('"') {
        serde_json::from_str(s).unwrap_or_else(|_| s.trim_matches('"').into())
    } else if s.starts_with('\'') {
        s.trim_matches('\'').replace("''", "'")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_keep_their_order_in_every_language() {
        // "Projet de monde VRChat" sorts before "Projet d’avatar VRChat": by name the avatar template would no longer be first in French.
        for language in orbiters_i18n::Language::TRANSLATED.into_iter().chain([orbiters_i18n::Language::En]) {
            let ids = orbiters_i18n::in_language(language, || {
                let mut all = vec![
                    Template { name: tr("VRChat World Project").into(), id: "com.vrchat.template.world".into(), builtin: true, ..Default::default() },
                    Template { name: "Zeta".into(), id: "z".into(), ..Default::default() },
                    Template { name: tr("VRChat Avatar Project").into(), id: "com.vrchat.template.avatar".into(), builtin: true, ..Default::default() },
                    Template { name: "Alpha".into(), id: "a".into(), ..Default::default() },
                ];
                order(&mut all);
                all.into_iter().map(|t| t.id).collect::<Vec<_>>()
            });
            assert_eq!(ids, ["com.vrchat.template.avatar", "com.vrchat.template.world", "a", "z"], "{language:?}");
        }
    }

    #[test]
    fn the_bundled_templates_are_found_in_every_language() {
        // The avatar template used to be looked up by its (translated) name: in French it was "missing".
        for language in orbiters_i18n::Language::TRANSLATED {
            let temp = tempfile::tempdir().unwrap();
            let found = orbiters_i18n::in_language(language, || builtins(temp.path()).map(|all| all.len()));
            assert!(found.is_ok_and(|n| n >= 2), "{language:?}");
        }
    }

    /// PROJECTS-24 a: the base template's package list is written once, not again at every listing of the templates; one that
    /// is not the base's (the avatar template's, copied in first, or changed since) is written.
    #[test]
    fn the_base_template_s_manifest_is_written_only_when_it_differs() {
        let temp = tempfile::tempdir().unwrap();
        builtins(temp.path()).unwrap();
        let manifest = temp.path().join("base-2022.3.22f1/Packages/vpm-manifest.json");
        let base: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(base, serde_json::json!({"dependencies":{"com.vrchat.base":{"version":"3.x"}}}));
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options().write(true).open(&manifest).unwrap().set_modified(long_ago).unwrap();
        builtins(temp.path()).unwrap();
        assert_eq!(std::fs::metadata(&manifest).unwrap().modified().unwrap(), long_ago, "listed again: not written again");
        std::fs::write(&manifest, r#"{"dependencies":{"com.vrchat.avatars":{"version":"3.x"}}}"#).unwrap();
        builtins(temp.path()).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(again, base, "another one is replaced");
    }

    #[test]
    fn an_error_is_said_in_the_language_of_the_moment_it_happens() {
        use orbiters_i18n::{in_language, Language};
        let english = validate_name("").unwrap_err().to_string();
        let french = in_language(Language::Fr, || validate_name("").unwrap_err().to_string());
        let japanese = in_language(Language::Ja, || validate_name("").unwrap_err().to_string());
        assert_ne!(english, french);
        assert_ne!(english, japanese);
        assert!(japanese.contains("フォルダー"), "{japanese}");
        // A value in the sentence stays: the path of a copy that was kept.
        let cancel = AtomicBool::new(true);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Source/Assets")).unwrap();
        std::fs::write(temp.path().join("Source/Assets/a.txt"), "a").unwrap();
        let said = in_language(Language::Es, || copy(&temp.path().join("Source"), &temp.path().join("Copy"), &cancel, &Arc::new(Mutex::default())).unwrap_err().to_string());
        assert!(said.contains("Copia detenida") && said.contains("Copy"), "{said}");
    }

    #[test]
    fn a_copy_phase_is_the_english_text_that_the_window_translates_where_it_draws_it() {
        // The windows compare the phase with "Copying project" to show Stop: it must not be the translation.
        let phase = String::from(mark("Copying project"));
        let progress = Progress { phase: phase.clone(), ..Default::default() };
        assert_eq!(progress.phase, "Copying project");
        let drawn = orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || tr(&progress.phase).to_string());
        assert_ne!(drawn, "Copying project", "the French window shows its own words");
    }

    #[test]
    fn external_local_dependencies_are_counted_and_materialized() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        let external = temp.path().join("External");
        std::fs::create_dir_all(source.join("Packages")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("payload.bin"), vec![42u8; 4096]).unwrap();
        crate::write_json(&source.join("Packages/manifest.json"), &serde_json::json!({"dependencies":{"external.package":"file:../../External"}})).unwrap();
        let manifest_bytes = std::fs::metadata(source.join("Packages/manifest.json")).unwrap().len();
        assert_eq!(duplication_bytes(&source, Keep::default()).unwrap(), manifest_bytes + 4096);
        let destination = temp.path().join("Copy");
        // It never said which Unity it uses: a new project is given one.
        let made = made_from(temp.path(), &source, "Copy");
        assert!(made.is_ok(), "{made:?}");
        assert!(std::fs::read_to_string(destination.join("ProjectSettings/ProjectVersion.txt")).unwrap().contains("2022.3.22f1"));
        assert_eq!(std::fs::read(destination.join("Packages/external.package/payload.bin")).unwrap(), vec![42u8; 4096]);
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(destination.join("Packages/manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["dependencies"]["external.package"], "file:external.package");
    }

    #[test]
    fn a_name_that_would_not_travel_is_refused_in_words_for_this_computer() {
        for name in ["a:b", "a/b", "a?", "CON", "name.", "name "] {
            let said = validate_name(name).unwrap_err().to_string();
            assert_eq!(said.contains("Windows folder name"), cfg!(windows), "{said}");
            assert!(cfg!(windows) || said.contains("also works on Windows"), "{said}");
        }
        validate_name("My avatars copy").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_keeps_programs_runnable_and_stays_writable() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("Packages/tool/bin")).unwrap();
        let tool = source.join("Packages/tool/bin/run");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let locked = source.join("Packages/locked.json");
        std::fs::write(&locked, "{}").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
        let copy = temp.path().join("copy");
        copy_keeping(&source, &copy, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert_eq!(std::fs::metadata(copy.join("Packages/tool/bin/run")).unwrap().permissions().mode() & 0o111, 0o111);
        // As on Windows, what comes next may write to it: a copy's settings are retargeted.
        assert!(!std::fs::metadata(copy.join("Packages/locked.json")).unwrap().permissions().readonly());
    }

    #[test]
    fn duplicate_preserves_unlisted_dependencies_and_registers_offline() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        let packages = source.join("Packages");
        std::fs::create_dir_all(packages.join("private.package")).unwrap();
        std::fs::create_dir_all(source.join("ProjectSettings")).unwrap();
        std::fs::write(source.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        crate::write_json(&packages.join("private.package/package.json"), &serde_json::json!({"name":"private.package","version":"1.2.3"})).unwrap();
        crate::write_json(&packages.join("vpm-manifest.json"), &serde_json::json!({"dependencies":{"private.package":{"version":"1.2.3"}},"locked":{"private.package":{"version":"1.2.3","dependencies":{}}}})).unwrap();
        let root = temp.path().join("vcc");
        std::fs::create_dir_all(&root).unwrap();
        let rt = crate::runtime().unwrap();
        let copy = rt.block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default()))).unwrap();
        assert!(installed_dependencies_complete(&copy).unwrap());
        assert!(rt.block_on(crate::vpm::registered(&root)).unwrap().iter().any(|p| p.canonicalize().unwrap() == copy.canonicalize().unwrap()));
        std::fs::remove_file(copy.join("Packages/private.package/package.json")).unwrap();
        assert!(!installed_dependencies_complete(&copy).unwrap());
    }

    /// A new project from `source`, its packages not restored (no repository answers in a test).
    fn made_from(root: &Path, source: &Path, name: &str) -> Result<PathBuf, String> {
        let rt = crate::runtime().unwrap();
        rt.block_on(make(&root.join("vcc"), source, root, name, "2022.3.22f1", Some(Keep::default()), &AtomicBool::new(false), &Arc::new(Mutex::default()))).map_err(|e| format!("{e:#}"))
    }

    /// A project on disk in `dir`: its version file and a `Packages/manifest.json` with `dependencies`.
    fn project(dir: &Path, dependencies: serde_json::Value) {
        std::fs::create_dir_all(dir.join("ProjectSettings")).unwrap();
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        std::fs::write(dir.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        std::fs::write(dir.join("Assets/a.txt"), "a").unwrap();
        crate::write_json(&dir.join("Packages/manifest.json"), &serde_json::json!({ "dependencies": dependencies })).unwrap();
    }
    fn is_registered(rt: &tokio::runtime::Runtime, root: &Path, path: &Path) -> bool {
        let wanted = crate::discovery::key(path);
        rt.block_on(crate::vpm::registered(root)).unwrap().iter().any(|p| crate::discovery::key(p) == wanted)
    }

    #[test]
    fn a_copy_that_fails_after_its_files_are_copied_is_listed_and_named() {
        // PROJECTS-1: a failure in the steps after the copy (here its local packages) left the copy on disk, unregistered.
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        std::fs::create_dir_all(temp.path().join("External")).unwrap();
        std::fs::write(temp.path().join("External/package.json"), "{}").unwrap();
        project(&source, serde_json::json!({ "bad/name": "file:../../External" }));
        let root = temp.path().join("vcc");
        let rt = crate::runtime().unwrap();
        let said = rt
            .block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())))
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
            .unwrap_err();
        let copy = temp.path().join("Copy");
        assert!(said.contains(&copy.display().to_string()), "{said}");
        assert!(is_registered(&rt, &root, &copy), "the copy is in the gallery's list");
    }

    /// PROJECTS-4: a copy that stops halfway is not a project the gallery lists, and its folder is kept as it is.
    #[cfg(windows)]
    #[test]
    fn a_copy_cut_halfway_is_never_taken_for_a_project() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        std::fs::create_dir_all(source.join("Zz")).unwrap();
        std::fs::write(source.join("Zz/locked.bin"), "x").unwrap();
        // Held by another program: the copy fails on it, after ProjectSettings is in.
        let _held = std::fs::File::options().read(true).share_mode(0).open(source.join("Zz/locked.bin")).unwrap();
        let copy = temp.path().join("Copy");
        assert!(copy_keeping(&source, &copy, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).is_err());
        assert!(copy.join("ProjectSettings/ProjectVersion.txt").is_file(), "kept as it is");
        assert!(!crate::discovery::is_project(&copy));
        drop(_held);
        let whole = temp.path().join("Whole");
        copy_keeping(&source, &whole, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert!(crate::discovery::is_project(&whole) && !whole.join(INCOMPLETE).exists());
    }

    /// A copy of `source` into `copy`, stopped once its folder exists after `meanwhile` ran: what Stop says.
    fn stopped_after(source: &Path, copy: &Path, meanwhile: impl FnOnce(&Path)) -> String {
        let cancel = AtomicBool::new(false);
        // The hook runs inside the copy: what it borrows outlives it.
        let mut meanwhile = Some(meanwhile);
        let at = copy.to_path_buf();
        {
            let hook = |step: Step| {
                if step == Step::Folders {
                    if let Some(f) = meanwhile.take() {
                        f(&at);
                    }
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            };
            stepping(hook, || copy_keeping(source, copy, Keep::default(), &cancel, &Arc::new(Mutex::default())))
        }
        .unwrap_err()
        .to_string()
    }

    /// Runs `f` with `hook` between the steps of a copy.
    fn stepping<R>(hook: impl FnMut(Step) -> anyhow::Result<()>, f: impl FnOnce() -> R) -> R {
        struct Off;
        impl Drop for Off {
            fn drop(&mut self) {
                crate::copying::BETWEEN.with(|b| *b.borrow_mut() = None);
            }
        }
        let cell = std::cell::RefCell::new(hook);
        let hook: Box<dyn Fn(Step) -> anyhow::Result<()> + '_> = Box::new(|s| (cell.borrow_mut())(s));
        // SAFETY: taken out again (by `Off`, whatever happens) before what it borrows goes.
        let hook: crate::copying::Between = unsafe { std::mem::transmute(hook) };
        crate::copying::BETWEEN.with(|b| *b.borrow_mut() = Some(hook));
        let _off = Off;
        f()
    }

    /// PROJECTS-4: Stop removes what that copy made, and only that.
    #[test]
    fn stop_removes_the_incomplete_copy_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let neighbour = temp.path().join("Neighbour");
        std::fs::create_dir_all(&neighbour).unwrap();
        std::fs::write(neighbour.join("mine.txt"), "mine").unwrap();
        let copy = temp.path().join("Copy");
        let said = stopped_after(&source, &copy, |_| {});
        assert!(said.contains("removed"), "{said}");
        assert!(!copy.exists(), "the incomplete copy is gone");
        assert!(source.join("Assets/a.txt").is_file() && neighbour.join("mine.txt").is_file());
    }

    /// A file the person put in the copy while it ran stays, named; the copy's own go.
    #[test]
    fn stop_keeps_a_file_added_during_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let copy = temp.path().join("Copy");
        let said = stopped_after(&source, &copy, |dest| std::fs::write(dest.join("Assets/mine.txt"), "mine").unwrap());
        assert_eq!(std::fs::read_to_string(copy.join("Assets/mine.txt")).unwrap(), "mine");
        assert!(said.contains("mine.txt"), "{said}");
        assert!(!copy.join(INCOMPLETE).exists() && !copy.join("ProjectSettings").exists(), "the copy's own entries went");
    }

    /// The copy's folder moved away and another put at its name (a restored backup, a junction to elsewhere): Stop
    /// removes nothing in either.
    #[test]
    fn stop_never_touches_a_folder_put_in_the_copys_place() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        std::fs::create_dir_all(source.join("Assets/Deep")).unwrap();
        std::fs::write(source.join("Assets/Deep/x.txt"), "x").unwrap();
        let copy = temp.path().join("Copy");
        let moved = temp.path().join("Moved");
        #[cfg(windows)]
        {
            // Verifier round 2: the copy's root, and every folder it made, are held while it runs: renaming either (to
            // put another in its place, or to have later writes land in a second copy) is refused.
            let said = stopped_after(&source, &copy, |dest| {
                assert!(std::fs::rename(dest, &moved).is_err(), "the root cannot be renamed while the copy runs");
                assert!(std::fs::rename(dest.join("Assets/Deep"), temp.path().join("Away")).is_err(), "nor a folder it made");
                assert!(std::fs::remove_dir(dest.join("Assets/Deep")).is_err());
            });
            assert!(said.contains("removed"), "{said}");
            assert!(!copy.exists() && !moved.exists() && !temp.path().join("Away").exists());
        }
        #[cfg(not(windows))]
        {
            // Without holding: the name is checked against the identity, and a substitute is never touched.
            let said = stopped_after(&source, &copy, |dest| {
                std::fs::rename(dest, &moved).unwrap();
                std::fs::create_dir_all(dest.join("Assets")).unwrap();
                std::fs::copy(moved.join(INCOMPLETE), dest.join(INCOMPLETE)).unwrap();
                std::fs::write(dest.join("Assets/a.txt"), "theirs").unwrap();
            });
            assert!(said.contains("could not be removed"), "{said}");
            assert_eq!(std::fs::read_to_string(copy.join("Assets/a.txt")).unwrap(), "theirs");
        }
    }

    /// A copy's marker copied into a real project (or left in one finished by hand elsewhere) does not hide it: it
    /// names another folder. Our copies never take a marker along.
    #[test]
    fn a_marker_from_another_folder_hides_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let other = Marker { copy: "x".into(), destination: temp.path().join("Elsewhere"), started: 1, folder: None };
        std::fs::write(source.join(INCOMPLETE), serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(crate::discovery::is_project(&source), "a marker naming another folder");
        let here = Marker { copy: "x".into(), destination: source.clone(), started: 1, folder: None };
        std::fs::write(source.join(INCOMPLETE), serde_json::to_vec(&here).unwrap()).unwrap();
        assert!(!crate::discovery::is_project(&source), "its own marker: an unfinished copy");
        // A marker naming this place but another folder (a project restored here with it): not hidden.
        let restored = Marker { copy: "x".into(), destination: source.clone(), started: 1, folder: Some(crate::owned::id_of(&temp.path().join("Source/Assets")).unwrap()) };
        std::fs::write(source.join(INCOMPLETE), serde_json::to_vec(&restored).unwrap()).unwrap();
        assert!(crate::discovery::is_project(&source), "the marker's folder identity is another folder's");
        let copy = temp.path().join("Copy");
        copy_keeping(&source, &copy, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert!(!copy.join(INCOMPLETE).exists() && crate::discovery::is_project(&copy));
    }

    #[test]
    fn packages_kept_only_in_library_are_named() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path();
        std::fs::create_dir_all(p.join("Packages/com.me.embedded")).unwrap();
        crate::write_json(
            &p.join("Packages/packages-lock.json"),
            &serde_json::json!({"dependencies": {
                "com.unity.textmeshpro": {"version": "3.0.6", "source": "registry", "url": "https://packages.unity.com"},
                "com.me.tool": {"version": "https://github.com/me/tool.git", "source": "git", "hash": "abc"},
                "com.scoped.thing": {"version": "1.0.0", "source": "registry", "url": "https://npm.example.com"},
                "com.me.embedded": {"version": "https://github.com/me/e.git", "source": "git"},
                "com.unity.modules.ui": {"version": "1.0.0", "source": "builtin"}
            }}),
        )
        .unwrap();
        assert_eq!(packages_only_in_library(p), ["com.me.tool", "com.scoped.thing"]);
        assert!(packages_only_in_library(&p.join("nothing")).is_empty());
    }

    /// Verifier, point 7: Stop pressed once every file is in still lists the copy (a whole copy is never left unlisted).
    #[test]
    fn a_stop_after_the_last_file_still_lists_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let root = temp.path().join("vcc");
        let rt = crate::runtime().unwrap();
        let cancel = AtomicBool::new(false);
        let stop = |step: Step| {
            if step == Step::Copied {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        };
        stepping(stop, || rt.block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &cancel, &Arc::new(Mutex::default())))).unwrap();
        assert!(is_registered(&rt, &root, &temp.path().join("Copy")));
    }

    /// A package folder is copied whole (its own Logs or Library are part of it); an archive the project already holds
    /// under the name the copy needs is never overwritten; a local package that is gone is named before anything is made.
    #[test]
    fn local_packages_are_copied_whole_and_never_over_another() {
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("External");
        std::fs::create_dir_all(external.join("Logs")).unwrap();
        std::fs::write(external.join("package.json"), "{}").unwrap();
        std::fs::write(external.join("Logs/part-of-the-package.txt"), "kept").unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({ "my.tool": "file:../../External" }));
        let dst = temp.path().join("Copy");
        made_from(temp.path(), &source, "Copy").unwrap();
        assert!(dst.join("Packages/my.tool/Logs/part-of-the-package.txt").is_file());
        // An archive: the source holds another file under the name the copy would give it.
        std::fs::write(temp.path().join("tool.tgz"), "the archive").unwrap();
        let source = temp.path().join("Source2");
        project(&source, serde_json::json!({ "my.archive": "file:../../tool.tgz" }));
        std::fs::write(source.join("Packages/my.archive.tgz"), "something else").unwrap();
        let estimate = duplication_bytes(&source, Keep::default()).unwrap();
        assert!(estimate >= ("the archive".len() + "something else".len()) as u64, "both counted: {estimate}");
        let dst = temp.path().join("Copy2");
        let said = made_from(temp.path(), &source, "Copy2").unwrap_err();
        assert!(said.contains("my.archive.tgz") && said.contains("listed"), "{said}");
        let rt = crate::runtime().unwrap();
        assert!(is_registered(&rt, &temp.path().join("vcc"), &dst), "the copy is in the gallery's list");
        assert_eq!(std::fs::read_to_string(dst.join("Packages/my.archive.tgz")).unwrap(), "something else");
        // Gone: named with where it was looked for.
        let source = temp.path().join("Source3");
        project(&source, serde_json::json!({ "my.gone": "file:../../Gone" }));
        let said = format!("{:#}", duplication_bytes(&source, Keep::default()).unwrap_err());
        assert!(said.contains("my.gone") && said.contains("Gone"), "{said}");
        let said = made_from(temp.path(), &source, "Copy3").unwrap_err();
        assert!(said.contains("my.gone") && said.contains("Gone") && !temp.path().join("Copy3").exists(), "{said}");
    }

    /// Third verification R1: one proof of the new folder is held from the moment it is made to the end of its listing: nothing
    /// takes its place or its folders' meanwhile, and nothing of the original is ever written (its settings, the packages it
    /// takes from outside, what those link to).
    #[cfg(windows)]
    #[test]
    fn a_duplicates_folder_is_held_to_the_end() {
        let temp = tempfile::tempdir().unwrap();
        let (external, shared) = (temp.path().join("External"), temp.path().join("Shared"));
        std::fs::create_dir_all(&external).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(external.join("package.json"), r#"{"name":"my.tool","version":"1.0.0"}"#).unwrap();
        std::fs::write(shared.join("a.mat"), "a").unwrap();
        junction::create(&shared, external.join("Shared")).unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({ "my.tool": "file:../../External" }));
        std::fs::write(source.join("ProjectSettings/ProjectSettings.asset"), "  productName: Source\n  productGUID: aa\n").unwrap();
        let root = temp.path().join("vcc");
        let rt = crate::runtime().unwrap();
        let copy = temp.path().join("New/Deeper/Copy");
        let seen = std::cell::RefCell::new(vec![]);
        let hook = |step: Step| {
            seen.borrow_mut().push(step);
            assert!(std::fs::rename(&copy, temp.path().join("Away")).is_err(), "{step:?}");
            assert!(std::fs::rename(temp.path().join("New"), temp.path().join("Away")).is_err(), "{step:?}");
            Ok(())
        };
        let made = stepping(hook, || rt.block_on(duplicate(&root, &source, copy.parent().unwrap(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())))).unwrap();
        assert_eq!(*seen.borrow(), [Step::Made, Step::Folders, Step::Copied, Step::MarkerGone, Step::Listed]);
        assert!(crate::copies::shown(&crate::data_dir()).iter().all(|(c, _)| c.destination != copy), "on the list: no record of it left");
        assert!(made == copy && is_registered(&rt, &root, &copy));
        // Everything of its own was written once, with the rest.
        let settings = std::fs::read_to_string(copy.join("ProjectSettings/ProjectSettings.asset")).unwrap();
        assert!(settings.contains("productName: \"Copy\"") && !settings.contains("productGUID: aa"), "{settings}");
        assert_eq!(std::fs::read_to_string(copy.join("Packages/my.tool/Shared/a.mat")).unwrap(), "a");
        assert!(!crate::owned::is_link(&std::fs::symlink_metadata(copy.join("Packages/my.tool/Shared")).unwrap()));
        // What the package links to is the original's: no copy goes there.
        let said = rt.block_on(duplicate(&root, &source, &shared, "In", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default()))).map(|_| ()).unwrap_err().to_string();
        assert!(said.contains("original uses") && !shared.join("In").exists(), "{said}");
        // App data or VCC's folder that the project links to: nothing is written there either.
        let kept = temp.path().join("vcc2");
        std::fs::create_dir_all(&kept).unwrap();
        junction::create(&kept, source.join("Assets/Kept")).unwrap();
        assert!(rt.block_on(duplicate(&kept, &source, temp.path(), "Other", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default()))).is_err());
        assert!(std::fs::read_dir(&kept).unwrap().count() == 0 && !temp.path().join("Other").exists());
        junction::delete(source.join("Assets/Kept")).unwrap();
        junction::delete(external.join("Shared")).unwrap();
    }

    /// A copy whose missing packages cannot be restored is listed all the same, and named; what they were restored in goes.
    #[test]
    fn a_copy_whose_packages_cannot_be_restored_is_listed_and_named() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        crate::write_json(&source.join("Packages/vpm-manifest.json"), &serde_json::json!({"dependencies":{"gone.package":{"version":"1.0.0"}},"locked":{"gone.package":{"version":"1.0.0","dependencies":{}}}})).unwrap();
        let root = temp.path().join("vcc");
        let manifest = std::fs::read(source.join("Packages/vpm-manifest.json")).unwrap();
        let rt = crate::runtime().unwrap();
        let said = OFFLINE.with(|o| {
            o.set(true);
            let r = rt.block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())));
            o.set(false);
            format!("{:#}", r.map(|_| ()).unwrap_err())
        });
        let copy = temp.path().join("Copy");
        assert!(said.contains("could not be restored") && said.contains(&copy.display().to_string()), "{said}");
        assert!(is_registered(&rt, &root, &copy) && crate::discovery::is_project(&copy));
        assert_eq!(std::fs::read(copy.join("Packages/vpm-manifest.json")).unwrap(), manifest, "its package list is the project's");
        assert!(stages_left(&crate::data_dir()).is_empty());
    }

    /// AGENTS.md: a copy is listed before anything of it is restored; then packages restored in Orbiters' own folder take the place of
    /// what it had of them, with the list that goes with them. Everything else is the project's own.
    #[test]
    fn a_copy_is_listed_and_then_its_packages_are_restored() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        for (at, text) in [("Packages/kept/package.json", "{}"), ("Packages/kept/mine.txt", "mine"), ("Packages/half/package.json", r#"{"name":"half","version":"0.9.0"}"#), ("Packages/half/old.txt", "old")] {
            std::fs::create_dir_all(source.join(at).parent().unwrap()).unwrap();
            std::fs::write(source.join(at), text).unwrap();
        }
        crate::write_json(&source.join("Packages/vpm-manifest.json"), &serde_json::json!({"dependencies":{"half":{"version":"1.0.0"}},"locked":{"half":{"version":"1.0.0","dependencies":{}}}})).unwrap();
        let root = temp.path().join("vcc");
        let rt = crate::runtime().unwrap();
        let copy = temp.path().join("Copy");
        RESTORES.with(|r| *r.borrow_mut() = Some(("half".into(), "1.0.0".into())));
        let steps = std::cell::RefCell::new(vec![]);
        let made = stepping(
            |step| {
                steps.borrow_mut().push((step, copy.join("Packages/half/old.txt").is_file()));
                Ok(())
            },
            || rt.block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default()))),
        );
        RESTORES.with(|r| *r.borrow_mut() = None);
        assert_eq!(made.unwrap(), copy);
        let at = |s: Step| steps.borrow().iter().position(|(x, _)| *x == s).unwrap();
        assert!(at(Step::Listed) < at(Step::Restored));
        assert_eq!((steps.borrow()[at(Step::Listed)].1, steps.borrow()[at(Step::Restored)].1), (true, false), "what it had, then what was restored");
        let read = |at: &str| std::fs::read_to_string(copy.join(at)).ok();
        assert_eq!(read("Packages/kept/mine.txt").as_deref(), Some("mine"));
        assert_eq!(read("Packages/half/Runtime/new.txt").as_deref(), Some("new"));
        assert!(installed_dependencies_complete(&copy).unwrap() && is_registered(&rt, &root, &copy));
        assert_eq!(std::fs::read(source.join("Packages/half/old.txt")).unwrap(), b"old", "the source is the source's");
    }

    /// Room for a restore is asked of every disk it writes on, before anything is downloaded.
    #[test]
    fn a_restore_needs_room_on_every_disk_it_writes_on() {
        struct Disks(u64, u64, u64);
        impl Room for Disks {
            fn of(&self, path: &Path) -> Option<(String, u64)> {
                match path.to_str()? {
                    "app" => Some(("a".into(), self.0)),
                    "vcc" => Some(("v".into(), self.1)),
                    _ => Some(("c".into(), self.2)),
                }
            }
        }
        struct One(u64);
        impl Room for One {
            fn of(&self, _: &Path) -> Option<(String, u64)> {
                Some(("one".into(), self.0))
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let locked: serde_json::Map<_, _> = (0..6).map(|i| (format!("gone{i}"), serde_json::json!({"version":"1.0.0"}))).collect();
        crate::write_json(&source.join("Packages/vpm-manifest.json"), &serde_json::json!({ "locked": locked })).unwrap();
        let payload = payload::scan(&source, Making::default(), &AtomicBool::new(false)).unwrap();
        let ask = |room: &dyn Room| restore_room(Path::new("vcc"), Path::new("app"), Path::new("copy"), &payload, room);
        let (r, c) = (6 * RESTORED, 6 * CACHED);
        assert!(ask(&Disks(r, c, r)).is_ok());
        for short in [Disks(r - 1, c, r), Disks(r, c - 1, r), Disks(r, c, r - 1)] {
            assert!(ask(&short).unwrap_err().to_string().contains("MB"));
        }
        // One disk for all three: their sum.
        assert!(ask(&One(2 * r + c)).is_ok() && ask(&One(2 * r + c - 1)).is_err());
    }

    /// A restore's folder that could not be taken away stays named by its lock, and goes at the next copy.
    #[cfg(windows)]
    #[test]
    fn a_restore_s_folder_that_could_not_go_goes_at_the_next_copy() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let stage = Stage::make(&app).unwrap();
        let (path, name) = (stage.path.clone(), stage.name.clone());
        std::fs::write(path.join("in use.txt"), "x").unwrap();
        let held = std::fs::File::options().read(true).share_mode(1).open(path.join("in use.txt")).unwrap();
        drop(stage);
        assert!(path.exists() && app.join(Stage::lock_of(&name)).is_file());
        drop(held);
        // One that is in use is left to its run.
        let mine = Stage::make(&app).unwrap();
        Stage::sweep(&app);
        assert!(!path.exists() && !app.join(Stage::lock_of(&name)).exists() && mine.path.is_dir());
    }

    /// Where no copy can be made, said for a window before it offers to: a share, inside a project, a name or a folder taken.
    #[test]
    fn a_place_that_is_refused_is_said_before() {
        let temp = tempfile::tempdir().unwrap();
        let inside = temp.path().join("Project");
        project(&inside, serde_json::json!({}));
        assert_eq!(refused_place(temp.path(), "New"), None);
        assert_eq!(refused_place(&temp.path().join("not/there/yet"), "New"), None);
        assert!(refused_place(&inside.join("Assets"), "New").unwrap().contains("Unity project"));
        assert!(refused_place(temp.path(), "Project").is_some() && refused_place(temp.path(), "a:b").is_some() && refused_place(Path::new("relative"), "New").is_some());
        assert!(!cfg!(windows) || refused_place(Path::new(r"\\server\share"), "New").unwrap().contains("network share"));
    }

    #[test]
    fn a_copy_leaves_out_what_unity_makes_again() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("Source");
        for file in [
            "Assets/a.txt",
            "Assets/Library/kept.txt",
            "Assets/Tools/Temp/kept.txt",
            "Library/ArtifactDB",
            "Temp/x",
            "Logs/x",
            "obj/x",
            "UserSettings/x",
            ".vs/x",
            ".git/x",
            "Packages/local/node_modules/x",
            "Source.sln",
            "Assembly-CSharp.csproj",
            "Assets/Plugins/Tool.csproj",
            "ProjectSettings/ProjectVersion.txt",
        ] {
            let p = src.join(file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        std::fs::create_dir_all(src.join("Assets/Empty")).unwrap();
        std::fs::create_dir_all(src.join("Temp/EmptyToo")).unwrap();
        let progress: SharedProgress = Arc::new(Mutex::default());
        let dst = temp.path().join("Copy");
        copy(&src, &dst, &AtomicBool::new(false), &progress).unwrap();
        assert!(dst.join("Assets/Empty").is_dir() && !dst.join("Temp").exists());
        let time = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(time(&dst.join("Assets/a.txt")), time(&src.join("Assets/a.txt")), "times kept");
        let has = |f: &str| dst.join(f).exists();
        for kept in ["Assets/a.txt", "Assets/Library/kept.txt", "Assets/Tools/Temp/kept.txt", "Assets/Plugins/Tool.csproj", "ProjectSettings/ProjectVersion.txt"] {
            assert!(has(kept), "{kept}");
        }
        for left in ["Library", "Temp", "Logs", "obj", "UserSettings", ".vs", ".git", "Packages/local/node_modules", "Source.sln", "Assembly-CSharp.csproj"] {
            assert!(!has(left), "{left}");
        }
        assert_eq!(progress.lock().unwrap().total, 5);
        // An empty asset folder stays, beside its .meta (PROJECTS-41).
        assert!(!has("Library") && has("Assets/Library"));
        let with = temp.path().join("With Library");
        copy_keeping(&src, &with, Keep { library: true }, &AtomicBool::new(false), &progress).unwrap();
        assert!(with.join("Library/ArtifactDB").is_file());
        assert!(!with.join("Temp").exists());
        assert!(copy_keeping(&src, &with, Keep { library: true }, &AtomicBool::new(false), &progress).is_err(), "never over a folder");
    }

    #[test]
    fn a_new_project_gets_its_own_product_and_a_copy_its_name_when_it_carried_the_old_one() {
        let settings = "PlayerSettings:\r\n  productGUID: 0123456789abcdef0123456789abcdef\r\n  companyName: DefaultCompany\r\n  productName: My avatars\r\n  defaultScreenWidth: 1024\r\n";
        let copy = retarget(settings, "My avatars copy", Some("My avatars"));
        assert!(copy.contains("  productName: \"My avatars copy\"\r\n"), "{copy}");
        assert!(!copy.contains("0123456789abcdef0123456789abcdef"));
        assert!(copy.contains("  companyName: DefaultCompany\r\n") && copy.ends_with("  defaultScreenWidth: 1024\r\n"));
        assert_eq!(copy.lines().count(), settings.lines().count());
        // A product name of its own stays; a template's always becomes the project's.
        let custom = "  productName: 'Studio''s base'\n  productGUID: aa\n";
        assert!(retarget(custom, "Copy", Some("Base")).contains("productName: 'Studio''s base'\n"));
        assert!(retarget(custom, "Copy", Some("Studio's base")).contains("productName: \"Copy\"\n"));
        assert!(retarget(custom, "New", None).contains("productName: \"New\"\n"));
    }
}
