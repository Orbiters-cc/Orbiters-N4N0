//! The capture worker process (`orbiters-capture worker`): started on demand, spoken to in JSON lines.

use orbiters_core::protocol::{WorkerCommand, WorkerEvent};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

pub struct WorkerHandle {
    child: Child,
    stdin: ChildStdin,
}

/// A worker executable next to the app's (the same cargo target folder in development).
pub fn sibling_exe(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// Puts a process the service started (the reconstruction, N4N0's ears, the call worker, the window, the overlay) in
/// the service's job: Windows ends it with the service, even when the service is killed or crashes. The reconstruction
/// would otherwise keep the GPU for minutes and race the one a restarted service starts on the same session; the
/// others leave once their link closes, unless they hang. What such a process starts in turn (a browser) is not in the
/// job.
pub fn keep_with_service(child: &Child) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::sync::OnceLock;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        };
        // Its handle stays open while the service runs and closes with it, which ends what is in the job. Not
        // inheritable: no child keeps the job alive.
        static JOB: OnceLock<Option<usize>> = OnceLock::new();
        let job = JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
            let size = std::mem::size_of_val(&info) as u32;
            SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size).ok()?;
            Some(job.0 as usize)
        });
        if let Some(job) = *job {
            if let Err(e) = unsafe { AssignProcessToJobObject(HANDLE(job as *mut _), HANDLE(child.as_raw_handle())) } {
                eprintln!("process {} is not in the service's job: {e}", child.id());
            }
        }
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// Reads a child's pipe line by line until it closes, each line without its `\r\n`. A line that is not UTF-8 (a native
/// library writing to the inherited stderr in the system's code page: an accented path on a French Windows) is passed
/// on with its odd bytes replaced: `BufRead::lines` ends on it, which took a living worker for gone (stdout) or stopped
/// draining its stderr until the worker blocked on a full pipe.
pub fn pipe_lines(pipe: impl Read, mut each: impl FnMut(String)) {
    let mut reader = BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => return,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                each(String::from_utf8_lossy(&buf).into_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

impl WorkerHandle {
    /// Starts the worker. `on_event` runs on a reader thread for every event; `on_exit` once when it ends.
    pub fn spawn(
        on_event: impl Fn(WorkerEvent) + Send + 'static,
        on_log: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Self> {
        let path = sibling_exe("orbiters-capture");
        anyhow::ensure!(path.is_file(), "{}", orbiters_i18n::tr!("capture worker not found at {path}", path = path.display()));
        let mut cmd = Command::new(&path);
        cmd.arg("worker").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        // Not in the service's job: if the service goes, its end of stdin closes and the worker finishes the session
        // it records (manifest, audio, tracking) before it leaves.
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        std::thread::Builder::new().name("worker-stdout".into()).spawn(move || {
            pipe_lines(stdout, |line| {
                // A number written as null (NaN) costs its field, not the event: a lost `Stopped` would leave the
                // session "being saved" (`WorkerEvent::from_line`).
                match WorkerEvent::from_line(&line) {
                    Some(e) => on_event(e),
                    None => eprintln!("worker: unexpected output: {line}"),
                }
            });
            on_exit();
        })?;
        std::thread::Builder::new().name("worker-stderr".into()).spawn(move || pipe_lines(stderr, on_log))?;
        Ok(Self { child, stdin })
    }

    pub fn send(&mut self, cmd: &WorkerCommand) -> anyhow::Result<()> {
        writeln!(self.stdin, "{}", cmd.to_line())?;
        self.stdin.flush()?;
        Ok(())
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// How long a quitting app waits for the worker to save the session it records.
const QUIT_WAIT: Duration = Duration::from_secs(30);

impl WorkerHandle {
    /// The app quits: the worker saves the session it records (asked to stop before) and leaves, given the time that
    /// takes, rather than the moment a worker being replaced gets.
    pub fn quit(mut self) {
        self.leave(QUIT_WAIT);
    }

    /// Asks the worker to leave (it finishes a session first) and waits up to `wait` for it, then ends it.
    fn leave(&mut self, wait: Duration) {
        let _ = self.send(&WorkerCommand::Quit);
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if !self.alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // A moment to write its manifest, then it is gone (a stuck worker being replaced).
        self.leave(Duration::from_secs(3));
    }
}

#[cfg(test)]
mod tests {
    /// CTL-2: a line that is not UTF-8 is a line, not the end of the pipe; the lines after it (an event, 64 KB more)
    /// still arrive.
    #[test]
    fn a_line_that_is_not_utf8_does_not_end_the_pipe() {
        let mut input = b"first\r\n\xE9t\xE9 L\xE9a\n{\"event\":1}\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 64 * 1024));
        input.extend_from_slice(b"\nlast");
        let mut lines = Vec::new();
        super::pipe_lines(&input[..], |l| lines.push(l));
        assert_eq!(lines.len(), 5, "{:?}", lines.iter().map(|l| l.len()).collect::<Vec<_>>());
        assert_eq!(lines[0], "first");
        assert_eq!(lines[1], "\u{FFFD}t\u{FFFD} L\u{FFFD}a");
        assert_eq!(lines[2], "{\"event\":1}");
        assert_eq!(lines[3].len(), 64 * 1024);
        assert_eq!(lines[4], "last");
    }
}
