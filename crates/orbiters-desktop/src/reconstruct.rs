//! The reconstruction worker process (`orbiters-reconstruct <session folder>`): one at a time, started on demand,
//! its events read line by line. Cancelling kills it; it only replaces the session's result once it is complete.

use crate::worker::sibling_exe;
use orbiters_core::reconstruction::Event;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub struct ReconstructHandle {
    child: Child,
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
        anyhow::ensure!(path.is_file(), "reconstruction worker not found at {}", path.display());
        let mut cmd = Command::new(&path);
        cmd.arg(session).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
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
        let mut child = cmd.spawn()?;
        crate::worker::keep_with_service(&child);
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
        Ok(Self { child })
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
