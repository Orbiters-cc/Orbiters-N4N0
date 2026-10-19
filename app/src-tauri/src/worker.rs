//! The capture worker process (`orbiters-capture worker`): started on demand, spoken to in JSON lines.

use orbiters_core::protocol::{WorkerCommand, WorkerEvent};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

pub struct WorkerHandle {
    child: Child,
    stdin: ChildStdin,
}

/// `orbiters-capture` next to the app executable (the same cargo target folder in development).
pub fn worker_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    dir.join(format!("orbiters-capture{}", std::env::consts::EXE_SUFFIX))
}

impl WorkerHandle {
    /// Starts the worker. `on_event` runs on a reader thread for every event; `on_exit` once when it ends.
    pub fn spawn(
        on_event: impl Fn(WorkerEvent) + Send + 'static,
        on_log: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Self> {
        let path = worker_path();
        anyhow::ensure!(path.is_file(), "capture worker not found at {}", path.display());
        let mut cmd = Command::new(&path);
        cmd.arg("worker").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        std::thread::Builder::new().name("worker-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<WorkerEvent>(&line) {
                    Ok(e) => on_event(e),
                    Err(_) => eprintln!("worker: unexpected output: {line}"),
                }
            }
            on_exit();
        })?;
        std::thread::Builder::new().name("worker-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                on_log(line);
            }
        })?;
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

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.send(&WorkerCommand::Quit);
        // Give the worker a moment to write its manifest, then make sure it is gone.
        for _ in 0..30 {
            if !self.alive() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = self.child.kill();
    }
}
