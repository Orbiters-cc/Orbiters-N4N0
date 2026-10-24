//! One instance at a time. A second launch asks the running one to show its window (or, with `--quit`, to shut
//! down cleanly, e.g. before an update replaces the files), then exits. A hidden one (Windows or SteamVR starting the
//! app) leaves it as it is.

/// What another launch asks the running instance to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Show,
    Quit,
}

/// Whether this process is the one that runs.
pub enum Instance {
    /// First instance; keep the guard alive. Requests from other launches arrive on a background thread.
    First(Guard),
    /// Another instance runs (and was asked to show itself, unless this launch was hidden).
    Other,
}

#[cfg(windows)]
pub use win::{acquire, signal, Guard};

#[cfg(windows)]
mod win {
    use super::{Instance, Request};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForMultipleObjects, EVENT_MODIFY_STATE, INFINITE};

    const SHOW: PCWSTR = w!("Local\\OrbitersDesktop.Show");
    const QUIT: PCWSTR = w!("Local\\OrbitersDesktop.Quit");

    /// Keeps the instance mutex and the request events open for the life of the process.
    pub struct Guard {
        _handles: [isize; 3],
    }

    /// Sends `request` to the running instance; false when none runs.
    pub fn signal(request: Request) -> bool {
        let name = if request == Request::Show { SHOW } else { QUIT };
        unsafe { OpenEventW(EVENT_MODIFY_STATE, false, name).map(|event| SetEvent(event).is_ok()).unwrap_or(false) }
    }

    /// This process runs, or another does (asked to show its window when `show`).
    pub fn acquire(show: bool, on_request: impl Fn(Request) + Send + 'static) -> anyhow::Result<Instance> {
        unsafe {
            let mutex = CreateMutexW(None, false, w!("Local\\OrbitersDesktop.Instance"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if show {
                    signal(Request::Show);
                }
                return Ok(Instance::Other);
            }
            let show = CreateEventW(None, false, false, SHOW)?;
            let quit = CreateEventW(None, false, false, QUIT)?;
            let raw = [show.0 as isize, quit.0 as isize];
            std::thread::Builder::new().name("single-instance".into()).spawn(move || {
                let handles = raw.map(|h| HANDLE(h as *mut _));
                loop {
                    let request = match WaitForMultipleObjects(&handles, false, INFINITE).0 {
                        0 => Request::Show,
                        1 => Request::Quit,
                        _ => break,
                    };
                    on_request(request);
                }
            })?;
            Ok(Instance::First(Guard { _handles: [mutex.0 as isize, raw[0], raw[1]] }))
        }
    }
}

#[cfg(unix)]
pub use unix::{acquire, signal, Guard};

#[cfg(unix)]
mod unix {
    use super::{Instance, Request};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub struct Guard {
        path: PathBuf,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn socket_path() -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join("orbiters-desktop.sock")
    }

    /// Sends `request` to the running instance; false when none runs.
    pub fn signal(request: Request) -> bool {
        let Ok(mut stream) = UnixStream::connect(socket_path()) else { return false };
        stream.write_all(if request == Request::Show { b"show\n" } else { b"quit\n" }).is_ok()
    }

    /// This process runs, or another does (asked to show its window when `show`).
    pub fn acquire(show: bool, on_request: impl Fn(Request) + Send + 'static) -> anyhow::Result<Instance> {
        let other = if show { signal(Request::Show) } else { UnixStream::connect(socket_path()).is_ok() };
        if other {
            return Ok(Instance::Other);
        }
        // Nobody answered: a stale socket from a crash.
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::thread::Builder::new().name("single-instance".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() {
                    // A hidden launch only looks: it sends nothing.
                    match line.trim() {
                        "quit" => on_request(Request::Quit),
                        "show" => on_request(Request::Show),
                        _ => {}
                    }
                }
            }
        })?;
        Ok(Instance::First(Guard { path }))
    }
}
