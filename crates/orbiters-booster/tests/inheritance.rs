//! What a child process gets from its parent on this Windows: the fact step 1's design rests on. A Below Normal priority class
//! is inherited (the CreateProcess documentation says so, and the review that found it was right), and the efficiency mode that
//! step 1 uses instead is not. Only the test's own processes are touched: copies of this program in a scratch folder.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessInformation, OpenProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation, BELOW_NORMAL_PRIORITY_CLASS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION,
};

const ROLE: &str = "ORBITERS_INHERIT_ROLE";
const DIR: &str = "ORBITERS_INHERIT_DIR";

/// Not a test: what the parent copy runs. It waits until it has been changed, then starts a child, says its PID, and waits.
#[test]
#[ignore = "run by the test below, in a process of its own"]
fn parent_body() {
    let Some(dir) = std::env::var_os(DIR).map(PathBuf::from) else { return };
    // Each copy lives while the test's folder does, a minute at most: none outlives the test.
    let while_wanted = || {
        let began = Instant::now();
        while dir.exists() && began.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    if std::env::var(ROLE).as_deref() == Ok("child") {
        while_wanted();
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !dir.join("go").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "parent_body", "--ignored", "--nocapture"])
        .env(ROLE, "child")
        .env(DIR, &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _ = std::fs::write(dir.join("child"), child.id().to_string());
    while_wanted();
    let _ = child.wait();
}

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn open(pid: u32) -> HANDLE {
    // SAFETY: a process this test started; closed by the caller.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION, false, pid) }.unwrap()
}

fn throttled(process: HANDLE) -> (u32, u32) {
    let mut state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
    // SAFETY: a valid handle and a buffer of the size passed.
    unsafe {
        GetProcessInformation(process, ProcessPowerThrottling, (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(), std::mem::size_of_val(&state) as u32)
    }
    .unwrap();
    (state.ControlMask, state.StateMask)
}

/// Starts a parent copy, changes it with `change`, lets it start a child, and gives back the child's handle.
fn child_of_a_changed_parent(dir: &Path, change: impl FnOnce(HANDLE)) -> (Kill, HANDLE, u32) {
    let exe = dir.join("inherit-parent.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    let parent = Kill(
        Command::new(&exe)
            .args(["--exact", "parent_body", "--ignored", "--nocapture"])
            .env(ROLE, "parent")
            .env(DIR, dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let handle = open(parent.0.id());
    change(handle);
    // SAFETY: opened above.
    unsafe {
        let _ = CloseHandle(handle);
    }
    std::fs::write(dir.join("go"), "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("child")) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                break pid;
            }
        }
        assert!(Instant::now() < deadline, "the child never started");
        std::thread::sleep(Duration::from_millis(5));
    };
    (parent, open(pid), pid)
}

/// Ends the child too: it lives as long as its folder, which can't go while its program runs.
fn end(child: u32) {
    use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
    // SAFETY: the test's own grandchild; the handle is closed below.
    if let Ok(h) = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, child) } {
        unsafe {
            let _ = TerminateProcess(h, 0);
            let _ = WaitForSingleObject(h, 5_000);
            let _ = CloseHandle(h);
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("orbiters-booster-inherit-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn below_normal_is_inherited_by_children() {
    let dir = scratch("priority");
    let (parent, child, child_pid) = child_of_a_changed_parent(&dir, |p| unsafe { SetPriorityClass(p, BELOW_NORMAL_PRIORITY_CLASS) }.unwrap());
    // SAFETY: a live handle.
    let class = unsafe { GetPriorityClass(child) };
    unsafe {
        let _ = CloseHandle(child);
    }
    drop(parent);
    end(child_pid);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(class, BELOW_NORMAL_PRIORITY_CLASS.0, "a child of a Below Normal program starts Below Normal");
}

#[test]
fn efficiency_mode_is_not_inherited_by_children() {
    let dir = scratch("ecoqos");
    let (parent, child, child_pid) = child_of_a_changed_parent(&dir, |p| {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        // SAFETY: a valid handle and the structure's size.
        unsafe {
            SetProcessInformation(p, ProcessPowerThrottling, (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(), std::mem::size_of_val(&state) as u32)
        }
        .unwrap();
        assert_eq!(throttled(p), (1, 1), "the parent itself is in efficiency mode");
    });
    let got = throttled(child);
    unsafe {
        let _ = CloseHandle(child);
    }
    drop(parent);
    end(child_pid);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, (0, 0), "a child of a program in efficiency mode starts as the system decides");
}
