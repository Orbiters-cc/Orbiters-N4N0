//! REL-1: the worker's stderr is the service's pipe. When the service goes, its end closes; a line written then must not
//! kill the worker (std's `eprintln!` panics on a failed write, exit code 101).

use std::process::{Command, Stdio};

#[test]
fn a_closed_stderr_does_not_kill_the_worker() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_orbiters-capture"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Closed before the worker writes its usage text to it (it starts in milliseconds; should it write first, the
    // text sits in the pipe and the test passes as well).
    drop(child.stderr.take());
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0), "the worker died writing to a closed stderr: {status:?}");
}
