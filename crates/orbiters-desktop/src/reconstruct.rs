//! The reconstruction worker process (`orbiters-reconstruct <session folder>`): one at a time, started on demand,
//! its events read line by line. Cancelling kills it; it only replaces the session's result once it is complete. Pausing
//! asks it to leave the GPU and to write where it stands (orbiters_reconstruct::checkpoint): the next one goes on from
//! there, which is why every start asks for `--resume`. A press on Stop, not this module, is what deletes that file.

use crate::worker::sibling_exe;
use orbiters_core::reconstruction::Event;
use std::io::Write as _;
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
            for line in crate::worker::records(stdout, "reconstruct") {
                // A report with a number written as null (NaN) still ends the run (`Event::from_line`).
                match Event::from_line(&line) {
                    Some(e) => on_event(e),
                    None => eprintln!("reconstruct: unexpected output: {line}"),
                }
            }
            on_exit();
        })?;
        std::thread::Builder::new().name("reconstruct-stderr".into()).spawn(move || crate::worker::log_lines(stderr).for_each(on_log))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// XCT-15: a line of a driver's in the system's code page ends neither the events (the run was taken for over while it
    /// went on) nor the draining of its stderr.
    #[test]
    fn a_line_that_is_not_utf8_ends_neither_its_events_nor_its_log() {
        let heard = |stderr: bool| {
            let (done, over) = std::sync::mpsc::channel::<()>();
            let (events, logs) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
            let (e, l) = (events.clone(), logs.clone());
            let output = b"p\xE9riph\xE9rique\r\n{\"event\":\"pausing\"}\r\nlast\r\n";
            // (The log's thread ends when its closure goes.)
            let logged = done.clone();
            let mut handle = ReconstructHandle::spawn_command(
                crate::worker::stand_in(output, stderr),
                move |event| e.lock().unwrap().push(format!("{event:?}")),
                move |line| {
                    l.lock().unwrap().push(line);
                    let _ = &logged;
                },
                move || drop(done),
            )
            .unwrap();
            assert_eq!(over.recv_timeout(std::time::Duration::from_secs(30)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected), "both pipes were read to their end");
            handle.cancel();
            let heard = (events.lock().unwrap().clone(), logs.lock().unwrap().clone());
            heard
        };
        let (events, _) = heard(false);
        assert_eq!(events, ["Pausing"], "the event after the odd line arrives");
        let (_, logs) = heard(true);
        assert_eq!(logs, ["p\u{FFFD}riph\u{FFFD}rique", "{\"event\":\"pausing\"}", "last"]);
    }
}
