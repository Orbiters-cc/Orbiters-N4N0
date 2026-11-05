//! Unity editors open on this computer and the project each has open, read from the editors' command lines
//! (`-projectPath`, as Unity Hub, VCC, ALCOM and this app start them), and what the panel does with one: bring its
//! window to the front, ask it to quit as its own close button does (Unity asks whether to save modified scenes
//! then), or end that one process. Import workers and batch-mode Unity processes are never taken for an editor; an
//! editor whose command line cannot be read (another user's, elevated) is matched by its window title only while the
//! project's `Temp/UnityLockfile` is held. Nothing in the project is deleted: Unity removes its own lock file.
use crate::discovery::key;
use std::path::{Path, PathBuf};

/// A Unity editor process with a project open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Editor {
    pub pid: u32,
    /// The project it was started with (empty when its command line could not be read).
    pub project: PathBuf,
    /// Its main window's title ("<project> - <scene> - <platform> - Unity <version>").
    pub title: Option<String>,
    /// Its main window waits on a dialog of its own (Unity's "save the modified scenes?", a progress bar).
    pub asking: bool,
}

/// An import worker or a batch-mode Unity, whose arguments give it away: never someone's editor.
fn worker_or_batch(args: &[String]) -> bool {
    let is = |a: &str, flag: &str| a.trim().eq_ignore_ascii_case(flag);
    args.iter().any(|a| is(a, "-batchmode") || a.trim().starts_with("AssetImportWorker") || is(a, "-workerJobId"))
}

/// The project an interactive editor started with these arguments has open; none for import workers, batch mode
/// and other Unity programs.
pub fn editor_project(args: &[String]) -> Option<PathBuf> {
    let is = |a: &str, flag: &str| a.trim().eq_ignore_ascii_case(flag);
    if worker_or_batch(args) {
        return None;
    }
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let a = a.trim();
        let value = if is(a, "-projectPath") {
            it.next().map(|s| s.trim().to_string())
        } else {
            a.split_once('=').filter(|(f, _)| is(f, "-projectPath")).map(|(_, v)| v.trim().to_string())
        };
        if let Some(v) = value.map(|v| v.trim_matches('"').to_string()).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(v));
        }
    }
    None
}

/// Whether two folders are the same one as Windows names them (case, separators, a trailing separator, `\\?\`).
pub fn same_folder(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// The editor that has `project` open: the one started with it, else (its command line unreadable, `locked`: the
/// project's lock file held) the one whose window title begins with the project's folder name.
pub fn of<'a>(editors: &'a [Editor], project: &Path, locked: bool) -> Option<&'a Editor> {
    let started_with = |folder: &Path| editors.iter().find(|e| !e.project.as_os_str().is_empty() && same_folder(&e.project, folder));
    started_with(project).or_else(|| {
        // The same folder through a link: `/tmp`, `/var` and a home folder elsewhere are links on the Mac, and `running`
        // reads each editor's folder without them.
        #[cfg(unix)]
        if !editors.is_empty() {
            return project.canonicalize().ok().and_then(|real| started_with(&real));
        }
        None
    }).or_else(|| {
        let name = project.file_name()?.to_string_lossy().to_lowercase();
        let prefix = format!("{name} - ");
        locked.then(|| editors.iter().find(|e| e.project.as_os_str().is_empty() && e.title.as_ref().is_some_and(|t| t.to_lowercase().starts_with(&prefix))))?
    })
}

/// What the panel knows of a project's editor, from one look at the processes to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Session {
    #[default]
    Closed,
    Open {
        pid: u32,
    },
    /// Asked to quit (Save & Exit); `asking` while Unity waits on its save question, `answered` once it was.
    Closing {
        pid: u32,
        asking: bool,
        answered: bool,
    },
    /// Ended by Kill, until the process is gone.
    Killing {
        pid: u32,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A look at the processes: the project's editor (and whether it waits on a dialog), or none.
    Seen(Option<(u32, bool)>),
    Close,
    Kill,
    /// Unity stayed open after its save question was answered (Cancel).
    Stayed,
}
impl Session {
    pub fn pid(self) -> Option<u32> {
        match self {
            Session::Closed => None,
            Session::Open { pid } | Session::Closing { pid, .. } | Session::Killing { pid } => Some(pid),
        }
    }
    pub fn open(self) -> bool {
        self != Session::Closed
    }
    pub fn next(self, event: Event) -> Session {
        match (self, event) {
            (_, Event::Seen(None)) => Session::Closed,
            (Session::Closing { pid, answered, .. }, Event::Seen(Some((p, asking)))) if p == pid => {
                // The question went away while Unity is still there: answered (Save, Don't Save or Cancel).
                let was_asking = matches!(self, Session::Closing { asking: true, .. });
                Session::Closing { pid, asking, answered: answered || (was_asking && !asking) }
            }
            (Session::Killing { pid }, Event::Seen(Some((p, _)))) if p == pid => self,
            (_, Event::Seen(Some((pid, _)))) => Session::Open { pid },
            (Session::Open { pid }, Event::Close) => Session::Closing { pid, asking: false, answered: false },
            (Session::Open { pid } | Session::Closing { pid, .. }, Event::Kill) => Session::Killing { pid },
            (Session::Closing { pid, .. }, Event::Stayed) => Session::Open { pid },
            (s, _) => s,
        }
    }
    /// Whether going from `self` to `after` is the editor quitting after Save & Exit or Kill (said once).
    pub fn ended(self, after: Session) -> bool {
        matches!(self, Session::Closing { .. } | Session::Killing { .. }) && after == Session::Closed
    }
}

#[cfg(windows)]
mod win {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetLastActivePopup, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW,
        SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE, WM_CLOSE,
    };

    /// Top-level, visible, unowned windows with a title: (process, window, title).
    pub(super) fn main_windows() -> Vec<(u32, isize, String)> {
        unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
            let out = &mut *(out.0 as *mut Vec<(u32, isize, String)>);
            if IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).map_or(true, |o| o.0.is_null()) {
                let mut buf = [0u16; 512];
                let n = GetWindowTextW(hwnd, &mut buf);
                if n > 0 {
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    out.push((pid, hwnd.0 as isize, String::from_utf16_lossy(&buf[..n as usize])));
                }
            }
            BOOL(1)
        }
        let mut out: Vec<(u32, isize, String)> = vec![];
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }
    pub(super) fn waits(hwnd: isize) -> bool {
        unsafe { !IsWindowEnabled(HWND(hwnd as *mut _)).as_bool() }
    }
    pub(super) fn front(hwnd: isize) {
        unsafe {
            let h = HWND(hwnd as *mut _);
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            // Its dialog when it shows one (the save question), else the window itself.
            let popup = GetLastActivePopup(h);
            let _ = SetForegroundWindow(if popup.0.is_null() { h } else { popup });
        }
    }
    pub(super) fn close(hwnd: isize) -> anyhow::Result<()> {
        unsafe { Ok(PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0))?) }
    }
}

/// What Unity's main program is called here. Its helpers (Unity Hub, UnityPackageManager, UnityShaderCompiler) are others.
const PROGRAM: &str = if cfg!(windows) { "Unity.exe" } else { "Unity" };

/// A Unity process: (process, parent, its arguments, its working folder where that was asked for).
type Process = (u32, Option<u32>, Vec<String>, Option<PathBuf>);

/// Unity's processes.
fn unity_processes(system: &mut sysinfo::System) -> Vec<Process> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pids: Vec<sysinfo::Pid> = system.processes().iter().filter(|(_, p)| p.name().eq_ignore_ascii_case(PROGRAM)).map(|(pid, _)| *pid).collect();
    if pids.is_empty() {
        return vec![];
    }
    let refresh = ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always);
    #[cfg(target_os = "macos")]
    let refresh = refresh.with_cwd(UpdateKind::Always);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), false, refresh);
    pids.iter()
        .filter_map(|pid| system.process(*pid))
        .map(|p| (p.pid().as_u32(), p.parent().map(|p| p.as_u32()), p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(), p.cwd().map(Path::to_path_buf)))
        .collect()
}

/// The Unity editors open now.
pub fn running() -> Vec<Editor> {
    let mut system = sysinfo::System::new();
    let processes = unity_processes(&mut system);
    #[cfg(windows)]
    let windows = if processes.is_empty() { vec![] } else { win::main_windows() };
    #[cfg(not(windows))]
    let windows: Vec<(u32, isize, String)> = vec![];
    processes
        .into_iter()
        .filter_map(|(pid, _, args, _folder)| {
            let window = windows.iter().find(|(p, _, _)| *p == pid);
            let project = match editor_project(&args) {
                #[cfg(target_os = "macos")]
                Some(p) => real_folder(p, _folder.as_deref()),
                #[cfg(not(target_os = "macos"))]
                Some(p) => p,
                // Unreadable (empty) command line: an editor if it has a window.
                None if args.is_empty() && window.is_some() => PathBuf::new(),
                // Started without a project on its command line (from the Dock, by a .unity file): Unity works from the
                // folder of the project it has open. Before one is open, or for anything else, that is no project.
                #[cfg(target_os = "macos")]
                None if !args.is_empty() && !worker_or_batch(&args) => _folder.filter(|f| crate::discovery::is_project(f))?,
                None => return None,
            };
            #[cfg(windows)]
            let asking = window.is_some_and(|(_, h, _)| win::waits(*h));
            #[cfg(target_os = "macos")]
            let asking = crate::mac::asking_before_quit(pid);
            #[cfg(not(any(windows, target_os = "macos")))]
            let asking = false;
            Some(Editor { pid, project, title: window.map(|(_, _, t)| t.clone()), asking })
        })
        .collect()
}

/// The folder an editor names, whole (`-projectPath .` is relative to where it started) and without links.
#[cfg(target_os = "macos")]
fn real_folder(project: PathBuf, working: Option<&Path>) -> PathBuf {
    let whole = if project.is_absolute() { project } else { working.map_or_else(|| project.clone(), |w| w.join(&project)) };
    whole.canonicalize().unwrap_or(whole)
}

/// The editor `pid` if it still has `project` open (a process number is reused once its process ended).
fn current(pid: u32, project: &Path) -> anyhow::Result<Editor> {
    let editors = running();
    of(&editors, project, crate::unity_open(project))
        .filter(|e| e.pid == pid)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Unity no longer has {} open.", project.display()))
}

#[cfg(windows)]
fn window(pid: u32) -> Option<isize> {
    win::main_windows().into_iter().find(|(p, _, _)| *p == pid).map(|(_, h, _)| h)
}

/// Brings the editor's window to the front (restored when minimised), or the dialog it waits on.
pub fn show(pid: u32, project: &Path) -> anyhow::Result<()> {
    current(pid, project)?;
    #[cfg(windows)]
    {
        let hwnd = window(pid).ok_or_else(|| anyhow::anyhow!("Unity's window was not found."))?;
        win::front(hwnd);
    }
    #[cfg(target_os = "macos")]
    {
        use crate::mac;
        let mut shown = mac::activate(pid);
        // Activating leaves minimised windows in the Dock, and the system may decline it from an app that is not in front.
        // `open` on the bundle does both, what a click on its Dock icon does, but it means whichever process of that
        // bundle the system picks, so only when this is the only one.
        if !shown || mac::windows_on_screen(pid) == 0 {
            let editors: Vec<u32> = running().iter().map(|e| e.pid).collect();
            if let Some(app) = mac::bundle_if_alone(pid, &editors) {
                shown = mac::open_command().arg(app).status().is_ok_and(|s| s.success()) || shown;
            }
        }
        anyhow::ensure!(shown, "macOS did not bring Unity to the front. Its icon in the Dock does.");
    }
    Ok(())
}

/// Asks the editor to quit as its close button does: Unity saves its settings and asks whether to save modified
/// scenes first (that question is brought to the front by [`show`]).
pub fn close(pid: u32, project: &Path) -> anyhow::Result<()> {
    current(pid, project)?;
    #[cfg(windows)]
    {
        let hwnd = window(pid).ok_or_else(|| anyhow::anyhow!("Unity's window was not found."))?;
        win::close(hwnd)?;
    }
    #[cfg(target_os = "macos")]
    {
        // Its windows now: one more later is its question about unsaved scenes.
        crate::mac::note_windows_before_quit(pid);
        anyhow::ensure!(crate::mac::terminate(pid), "macOS could not ask Unity to quit. Quit it from its own menu.");
    }
    Ok(())
}

/// Ends the editor of `project` (and its own import workers, started for that project), losing what was not saved.
/// Other Unity editors are left alone; nothing in the project is deleted.
pub fn kill(pid: u32, project: &Path) -> anyhow::Result<()> {
    current(pid, project)?;
    let mut system = sysinfo::System::new();
    let processes = unity_processes(&mut system);
    let workers: Vec<u32> = processes
        .iter()
        .filter(|(p, parent, args, _)| *p != pid && *parent == Some(pid) && worker_of(args).is_some_and(|w| same_folder(&w, project)))
        .map(|(p, _, _, _)| *p)
        .collect();
    let process = system.process(sysinfo::Pid::from_u32(pid)).ok_or_else(|| anyhow::anyhow!("Unity already quit."))?;
    anyhow::ensure!(process.kill(), "Unity could not be ended (it may run as another user).");
    for w in workers {
        if let Some(p) = system.process(sysinfo::Pid::from_u32(w)) {
            p.kill();
        }
    }
    Ok(())
}

/// The project an import worker works for (a batch-mode Unity with `-projectPath`).
fn worker_of(args: &[String]) -> Option<PathBuf> {
    let i = args.iter().position(|a| a.trim().eq_ignore_ascii_case("-projectPath"))?;
    args.get(i + 1).map(|p| PathBuf::from(p.trim().trim_matches('"')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn an_editor_is_told_by_its_project_path() {
        let unity = r"C:\Program Files\Unity\Hub\Editor\2022.3.22f1\Editor\Unity.exe";
        assert_eq!(
            editor_project(&args(&[unity, "-projectpath", r"H:\Unity projects\N4N0 sample avatar", "-useHub", "-hubIPC"])),
            Some(PathBuf::from(r"H:\Unity projects\N4N0 sample avatar"))
        );
        assert_eq!(editor_project(&args(&[unity, "-projectPath", "\"D:/My Avatars/\""])), Some(PathBuf::from("D:/My Avatars/")));
        assert_eq!(editor_project(&args(&[unity, "-projectPath=E:/World"])), Some(PathBuf::from("E:/World")));
        // Import workers and batch builds of the same project are not its editor.
        let worker = args(&[unity, "-adb2", "-batchMode", "-noUpm", "-name", "AssetImportWorker0", "-projectPath", r"H:\Unity projects\N4N0 sample avatar"]);
        assert_eq!(editor_project(&worker), None);
        assert_eq!(worker_of(&worker), Some(PathBuf::from(r"H:\Unity projects\N4N0 sample avatar")));
        assert_eq!(editor_project(&args(&[unity, "-batchmode", "-quit", "-projectPath", "E:/World"])), None);
        assert_eq!(editor_project(&args(&[unity])), None);
    }

    #[test]
    fn the_project_matches_whatever_its_spelling_and_only_its_own_editor() {
        let editors = vec![
            Editor { pid: 10, project: PathBuf::from(r"H:\Unity projects\N4N0 sample avatar\"), title: Some("N4N0 sample avatar - ultifallbacktest - Windows, Mac, Linux - Unity 2022.3.22f1*".into()), asking: false },
            Editor { pid: 11, project: PathBuf::from(r"H:\Unity projects\N4N0 sample avatar copy"), title: None, asking: false },
            Editor { pid: 12, project: PathBuf::from("d:/worlds/moonlight"), title: None, asking: true },
        ];
        assert_eq!(of(&editors, Path::new("h:/unity projects/n4n0 sample avatar"), false).map(|e| e.pid), Some(10));
        assert_eq!(of(&editors, Path::new(r"H:\Unity projects\N4N0 sample avatar copy"), false).map(|e| e.pid), Some(11));
        assert_eq!(of(&editors, Path::new(r"D:\Worlds\Moonlight\"), true).map(|e| e.pid), Some(12));
        assert_eq!(of(&editors, Path::new(r"H:\Unity projects\N4N0 sample"), true), None, "a folder whose name begins another's");
        assert_eq!(of(&editors, Path::new(r"C:\Elsewhere\N4N0 sample avatar"), true), None, "another folder of the same name");
        // A command line that could not be read: the title, only while the project's lock is held.
        let unread = vec![Editor { pid: 20, project: PathBuf::new(), title: Some("Club outfits - Main - Windows, Mac, Linux - Unity 2022.3.22f1".into()), asking: false }];
        assert_eq!(of(&unread, Path::new("H:/Unity projects/Club outfits"), true).map(|e| e.pid), Some(20));
        assert_eq!(of(&unread, Path::new("H:/Unity projects/Club outfits"), false), None);
        assert_eq!(of(&unread, Path::new("H:/Unity projects/Club"), true), None);
    }

    /// What the tests below run as an editor: this test program, under the name `Unity`.
    #[test]
    #[ignore]
    fn stand_in_editor() {
        if std::env::var_os("ORBITERS_TEST_STAND_IN").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    /// Starts this test program as a process named `Unity`, with these arguments after the ones the test harness takes.
    #[cfg(target_os = "macos")]
    fn start_stand_in(program: &Path, folder: &Path, args: &[&str]) -> std::process::Child {
        std::process::Command::new(program)
            .args(["--exact", "editor::tests::stand_in_editor", "--ignored", "--"])
            .args(args)
            .current_dir(folder)
            .env("ORBITERS_TEST_STAND_IN", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[cfg(target_os = "macos")]
    fn seen(what: impl Fn(&[Editor]) -> bool) -> Vec<Editor> {
        for _ in 0..100 {
            let editors = running();
            if what(&editors) {
                return editors;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("not seen in time: {:?}", running());
    }

    /// The real thing with a stand-in program: the system's process list, its arguments and working folder. Unity's
    /// bundle has it at `Unity.app/Contents/MacOS/Unity`.
    #[cfg(target_os = "macos")]
    #[test]
    fn running_editors_are_read_from_the_system_and_kill_ends_only_its_own() {
        let temp = tempfile::tempdir().unwrap();
        let macos = temp.path().join("Unity.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let program = macos.join("Unity");
        // A hard link keeps its signature valid and costs no copy of a large test program.
        std::fs::hard_link(std::env::current_exe().unwrap(), &program).or_else(|_| std::fs::copy(std::env::current_exe().unwrap(), &program).map(|_| ())).unwrap();
        let project = |name: &str| {
            let p = temp.path().join("Unity projects").join(name);
            std::fs::create_dir_all(p.join("ProjectSettings")).unwrap();
            std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
            p
        };
        let (avatar, world, other) = (project("Avatar"), project("World"), project("Other"));

        // Started with its project, as Unity Hub and this app do.
        let mut started = start_stand_in(&program, temp.path(), &["-projectPath", avatar.to_str().unwrap(), "-useHub"]);
        // A worker of the same project is no editor.
        let mut worker = start_stand_in(&program, &avatar, &["-batchMode", "-name", "AssetImportWorker0", "-projectPath", avatar.to_str().unwrap()]);
        // From the Dock: nothing on its command line, the project's folder its working folder.
        let dock = start_stand_in(&program, &world, &[]);
        // Nothing open yet.
        let empty = start_stand_in(&program, temp.path(), &[]);

        let editors = seen(|e| of(e, &avatar, false).is_some() && of(e, &world, false).is_some());
        assert_eq!(of(&editors, &avatar, false).map(|e| e.pid), Some(started.id()));
        assert_eq!(of(&editors, &world, false).map(|e| e.pid), Some(dock.id()));
        assert_eq!(of(&editors, &other, false), None);
        assert!(!editors.iter().any(|e| e.pid == worker.id()), "a worker: {editors:?}");
        assert!(!editors.iter().any(|e| e.pid == empty.id()), "no project yet: {editors:?}");
        assert!(editors.iter().all(|e| e.title.is_none()), "titles need a permission we do not ask for");
        // Unity's lock file is not needed to know: nothing here holds one.
        assert!(crate::unity_open(&avatar) && crate::unity_open(&world));
        assert!(!crate::unity_open(&other));

        // Asking for it through a spelling of its own, a link in the path.
        let link = temp.path().join("linked");
        std::os::unix::fs::symlink(temp.path().join("Unity projects"), &link).unwrap();
        assert_eq!(of(&editors, &link.join("Avatar"), false).map(|e| e.pid), Some(started.id()));

        // A process that is not this project's cannot be ended through it.
        assert!(kill(dock.id(), &avatar).is_err());
        // Kill ends that editor, and leaves the others and the worker of another parent running.
        kill(started.id(), &avatar).unwrap();
        started.wait().unwrap();
        let editors = seen(|e| of(e, &avatar, false).is_none());
        assert_eq!(of(&editors, &world, false).map(|e| e.pid), Some(dock.id()));
        assert_eq!(worker.try_wait().unwrap(), None);
        assert!(!crate::unity_open(&avatar));
        for mut process in [worker, dock, empty] {
            let _ = process.kill();
            let _ = process.wait();
        }
    }

    #[test]
    fn save_and_exit_and_kill_go_back_to_open_in_unity() {
        let open = Session::Closed.next(Event::Seen(Some((10, false))));
        assert_eq!(open, Session::Open { pid: 10 });
        assert_eq!(Session::Closed.next(Event::Close), Session::Closed, "nothing to close");
        // Save & Exit: Unity asks whether to save, the question is answered, Unity quits.
        let closing = open.next(Event::Close);
        assert_eq!(closing, Session::Closing { pid: 10, asking: false, answered: false });
        let asking = closing.next(Event::Seen(Some((10, true))));
        assert_eq!(asking, Session::Closing { pid: 10, asking: true, answered: false });
        let answered = asking.next(Event::Seen(Some((10, false))));
        assert_eq!(answered, Session::Closing { pid: 10, asking: false, answered: true });
        let gone = answered.next(Event::Seen(None));
        assert_eq!(gone, Session::Closed);
        assert!(answered.ended(gone));
        // Cancel in Unity's question: open again.
        assert_eq!(answered.next(Event::Stayed), Session::Open { pid: 10 });
        // Kill, from open or while closing; another editor of the project later is open, not being killed.
        let killing = closing.next(Event::Kill);
        assert_eq!(killing, Session::Killing { pid: 10 });
        assert_eq!(killing.next(Event::Seen(Some((10, false)))), killing);
        assert_eq!(killing.next(Event::Seen(Some((44, false)))), Session::Open { pid: 44 });
        assert!(killing.ended(killing.next(Event::Seen(None))));
        // Closed by the person in Unity: not something this app ended.
        assert!(!open.ended(open.next(Event::Seen(None))));
    }
}
