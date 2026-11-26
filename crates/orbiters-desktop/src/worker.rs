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
                log_warn!("process {} is not in the service's job: {e}", child.id(), e = e);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// A child's pipe, line by line until it closes, each line without its `\r\n`: the one reader of every process started
/// here. A line that is not UTF-8 (a native library writing to the inherited stderr in the system's code page: an accented
/// path on a French Windows) is passed on with its odd bytes replaced: `BufRead::lines` ends on it, which took a living
/// worker for gone (stdout) or stopped draining its stderr until the worker blocked on a full pipe.
pub fn log_lines(pipe: impl Read) -> impl Iterator<Item = String> {
    Lines(BufReader::new(pipe)).map(|line| String::from_utf8_lossy(&line).into_owned())
}

/// [`log_lines`] for a protocol (a worker's events): a line that is not UTF-8 is passed over whole, with a note (its length
/// only: it may hold what someone said), and the next ones still arrive. Decoded with replacements it could have become a
/// valid event with another file path in it (verify-controller).
pub fn records(pipe: impl Read, what: &'static str) -> impl Iterator<Item = String> {
    Lines(BufReader::new(pipe)).filter_map(move |line| match String::from_utf8(line) {
        Ok(text) => Some(text),
        Err(e) => {
            log_info!("{what}: a line of {} bytes that is not UTF-8, left out", e.as_bytes().len(), what = what);
            None
        }
    })
}

/// A pipe's lines as bytes, without their `\r\n`, until it closes.
struct Lines<R: Read>(BufReader<R>);

impl<R: Read> Iterator for Lines<R> {
    type Item = Vec<u8>;
    fn next(&mut self) -> Option<Vec<u8>> {
        let mut line = Vec::new();
        loop {
            match self.0.read_until(b'\n', &mut line) {
                Ok(0) if line.is_empty() => return None,
                Ok(_) => {
                    while matches!(line.last(), Some(b'\n' | b'\r')) {
                        line.pop();
                    }
                    return Some(line);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
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
            for line in records(stdout, "worker") {
                // A number written as null (NaN) costs its field, not the event: a lost `Stopped` would leave the
                // session "being saved" (`WorkerEvent::from_line`).
                match WorkerEvent::from_line(&line) {
                    Some(e) => on_event(e),
                    None => log_info!("worker: unexpected output: {line}", line = line),
                }
            }
            on_exit();
        })?;
        std::thread::Builder::new().name("worker-stderr".into()).spawn(move || log_lines(stderr).for_each(on_log))?;
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

/// A stand-in for a worker: a program that writes `output`, byte for byte, on its stdout (`stderr`: on its stderr) and ends.
#[cfg(test)]
pub(crate) fn stand_in(output: &[u8], stderr: bool) -> Command {
    let file = std::env::temp_dir().join(format!("orbiters-stand-in-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::write(&file, output).unwrap();
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd");
        // Its file goes once it was said.
        cmd.raw_arg(format!(r#"/S /C "type "{0}"{1} & del "{0}"""#, file.display(), if stderr { " 1>&2" } else { "" })); // i18n: not shown
        cmd.creation_flags(0x0800_0000);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!("cat '{0}'{1}; rm '{0}'", file.display(), if stderr { " 1>&2" } else { "" })); // i18n: not shown
        cmd
    };
    cmd.stdin(Stdio::null());
    cmd
}

#[cfg(test)]
mod tests {
    /// XCT-15, DAP-20: what the readers of this crate are, not one of them by hand: a child's pipe read with `BufRead::lines`
    /// ends at the first line that is not UTF-8.
    #[test]
    fn every_pipe_is_read_by_the_shared_reader() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // Readers of what no process of ours pipes: (file, why).
        const OTHERS: &[(&str, &str)] = &[
            ("ipc.rs", "the link's own socket: what is not UTF-8 there is no message of ours, and the link ends"),
            ("bridge/guard.rs", "its own standard input, a line the service writes"),
            ("bridge/lab.rs", "a developer's command, its own standard input"),
        ];
        let mut found = Vec::new();
        for (name, text) in orbiters_core::source::named(&src) {
            if OTHERS.iter().any(|(f, _)| *f == name) {
                continue;
            }
            // (A statement may be over several lines.)
            for statement in text.split([';', '{']) {
                let code = statement.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<String>();
                if (code.contains("BufReader::new(") || code.contains("stdin()")) && code.contains(".lines()") {
                    found.push(format!("{name}: {}", code.trim()));
                }
            }
        }
        assert!(found.is_empty(), "read it with worker::records or worker::log_lines:\n{}", found.join("\n"));
    }

    /// CTL-2: a line that is not UTF-8 is a line, not the end of the pipe; the lines after it (an event, 64 KB more)
    /// still arrive.
    #[test]
    fn a_line_that_is_not_utf8_does_not_end_the_pipe() {
        let mut input = b"first\r\n\xE9t\xE9 L\xE9a\n{\"event\":1}\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 64 * 1024));
        input.extend_from_slice(b"\nlast");
        let mut lines = Vec::new();
        lines.extend(super::log_lines(&input[..]));
        // The worker's events: the line that is not UTF-8 (inside a quoted path, it would decode to another path) is
        // left out, the others arrive.
        let mut records = Vec::new();
        records.extend(super::records(&input[..], "test"));
        assert_eq!(records.len(), 4, "the one line that is not UTF-8 is left out");
        assert_eq!(records[1], "{\"event\":1}");
        assert!(records.iter().all(|r| !r.contains('\u{FFFD}')));
        assert_eq!(lines.len(), 5, "{:?}", lines.iter().map(|l| l.len()).collect::<Vec<_>>());
        assert_eq!(lines[0], "first");
        assert_eq!(lines[1], "\u{FFFD}t\u{FFFD} L\u{FFFD}a");
        assert_eq!(lines[2], "{\"event\":1}");
        assert_eq!(lines[3].len(), 64 * 1024);
        assert_eq!(lines[4], "last");
    }
}
