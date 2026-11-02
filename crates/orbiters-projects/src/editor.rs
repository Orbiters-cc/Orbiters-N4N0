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

/// The project an interactive editor started with these arguments has open; none for import workers, batch mode
/// and other Unity programs.
pub fn editor_project(args: &[String]) -> Option<PathBuf> {
    let is = |a: &str, flag: &str| a.trim().eq_ignore_ascii_case(flag);
    if args.iter().any(|a| is(a, "-batchmode") || a.trim().starts_with("AssetImportWorker") || is(a, "-workerJobId")) {
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
    editors.iter().find(|e| !e.project.as_os_str().is_empty() && same_folder(&e.project, project)).or_else(|| {
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

/// Unity's processes: (process, parent, its arguments).
fn unity_processes(system: &mut sysinfo::System) -> Vec<(u32, Option<u32>, Vec<String>)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pids: Vec<sysinfo::Pid> = system.processes().iter().filter(|(_, p)| p.name().eq_ignore_ascii_case("Unity.exe")).map(|(pid, _)| *pid).collect();
    if pids.is_empty() {
        return vec![];
    }
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), false, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always));
    pids.iter()
        .filter_map(|pid| system.process(*pid))
        .map(|p| (p.pid().as_u32(), p.parent().map(|p| p.as_u32()), p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect()))
        .collect()
}

/// The Unity editors open now. Empty off Windows.
pub fn running() -> Vec<Editor> {
    let mut system = sysinfo::System::new();
    let processes = unity_processes(&mut system);
    #[cfg(windows)]
    let windows = if processes.is_empty() { vec![] } else { win::main_windows() };
    #[cfg(not(windows))]
    let windows: Vec<(u32, isize, String)> = vec![];
    processes
        .into_iter()
        .filter_map(|(pid, _, args)| {
            let window = windows.iter().find(|(p, _, _)| *p == pid);
            let project = match editor_project(&args) {
                Some(p) => p,
                // Unreadable (empty) command line: an editor if it has a window.
                None if args.is_empty() && window.is_some() => PathBuf::new(),
                None => return None,
            };
            #[cfg(windows)]
            let asking = window.is_some_and(|(_, h, _)| win::waits(*h));
            #[cfg(not(windows))]
            let asking = false;
            Some(Editor { pid, project, title: window.map(|(_, _, t)| t.clone()), asking })
        })
        .collect()
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
        .filter(|(p, parent, args)| *p != pid && *parent == Some(pid) && worker_of(args).is_some_and(|w| same_folder(&w, project)))
        .map(|(p, _, _)| *p)
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
        assert_eq!(of(&unread, Path::new(r"H:\Unity projects\Club outfits"), true).map(|e| e.pid), Some(20));
        assert_eq!(of(&unread, Path::new(r"H:\Unity projects\Club outfits"), false), None);
        assert_eq!(of(&unread, Path::new(r"H:\Unity projects\Club"), true), None);
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
