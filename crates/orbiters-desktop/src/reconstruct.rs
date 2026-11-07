//! The reconstruction worker process (`orbiters-reconstruct <session folder>`): one at a time, started on demand,
//! its events read line by line. Cancelling kills it; it only replaces the session's result once it is complete. Pausing
//! asks it to leave the GPU and to write where it stands (orbiters_reconstruct::checkpoint): the next one goes on from
//! there, which is why every start asks for `--resume`. A press on Stop, not this module, is what deletes that file.

use crate::worker::sibling_exe;
use orbiters_core::reconstruction::Event;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

pub struct ReconstructHandle {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl ReconstructHandle {
    /// Starts the worker on a session, on the integrated GPU when `integrated`. `on_event` runs on a reader thread for
    /// every event, `on_exit` once when the worker's output ends.
    pub fn spawn(
        session: &Path,
        integrated: bool,
        on_event: impl Fn(Event) + Send + 'static,
        on_log: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Self> {
        let path = sibling_exe("orbiters-reconstruct");
        anyhow::ensure!(path.is_file(), "{}", orbiters_i18n::tr!("reconstruction worker not found at {path}", path = path.display()));
        let mut cmd = Command::new(&path);
        cmd.arg(session).arg("--resume");
        if integrated {
            cmd.args(["--gpu", "integrated"]);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW, normal priority class: the worker picks its own class right away (above normal while
            // VRChat is closed, normal while it runs: orbiters_reconstruct::priority). Started below normal, it could
            // not even reach that while another program keeps every core busy.
            cmd.creation_flags(0x0800_0000);
        }
        Self::spawn_command(cmd, on_event, on_log, on_exit)
    }

    /// Starts `cmd` as the worker (the program itself, or a test's stand-in for it).
    pub(crate) fn spawn_command(
        mut cmd: Command,
        on_event: impl Fn(Event) + Send + 'static,
        on_log: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Self> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        crate::worker::keep_with_service(&child);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        std::thread::Builder::new().name("reconstruct-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // A report with a number written as null (NaN) still ends the run (`Event::from_line`).
                match Event::from_line(&line) {
                    Some(e) => on_event(e),
                    None => eprintln!("reconstruct: unexpected output: {line}"),
                }
            }
            on_exit();
        })?;
        std::thread::Builder::new().name("reconstruct-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                on_log(line);
            }
        })?;
        Ok(Self { child, stdin })
    }

    /// Asks it to pause. This never waits for it: a capture is starting. A worker that cannot be asked has already gone.
    pub fn pause(&mut self) -> std::io::Result<()> {
        let stdin = self.stdin.as_mut().ok_or(std::io::ErrorKind::BrokenPipe)?;
        writeln!(stdin, "{}", orbiters_reconstruct::checkpoint::PAUSE_LINE)?;
        stdin.flush()
    }

    pub fn cancel(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for ReconstructHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
