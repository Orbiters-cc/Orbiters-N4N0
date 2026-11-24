//! The processes of a Linux computer, from `/proc`, read by the person's own means: nothing is opened of them but the text files
//! `/proc` shows of every process (the names, the command line where it is allowed), VRChat's least of all.
//!
//! Two things would go wrong with a plain list. `sysinfo` counts every *thread* as a process unless told otherwise, and Wine's
//! threads have their program's name: one VRChat would look like a hundred. And the kernel keeps 15 characters of a name,
//! where a Windows program's (`start_protected_game.exe`) is often longer: the name here is the whole one, from the command
//! line. (`/proc`'s own folders are the processes; their threads are under `<pid>/task`, never looked at.)
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    /// The program's file name: a Windows program's under Wine with its `.exe`.
    pub name: String,
    /// The command line, its program first; empty where the system would not say (another person's process).
    pub args: Vec<String>,
    /// The program's file, where the system says (the person's own processes): Wine's loader for a Windows program.
    pub exe: Option<PathBuf>,
    /// When it started, in Unix nanoseconds, as precisely as the kernel keeps it (a hundredth of a second). The same process
    /// always has the same; a process number taken again has another.
    pub started_ns: u64,
}

impl Process {
    /// When it started, in Unix seconds.
    pub fn started_s(&self) -> u64 {
        self.started_ns / 1_000_000_000
    }

    /// Whether its name is `name`, letter case aside.
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    pub fn is_vrchat(&self) -> bool {
        self.is("VRChat.exe")
    }
}

/// The processes running now, without their threads. Empty when `/proc` cannot be read: that proves nothing gone.
#[cfg(target_os = "linux")]
pub fn all() -> Vec<Process> {
    // SAFETY: asks for a constant of the system's.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    all_in(Path::new("/proc"), u64::try_from(ticks).unwrap_or(100).max(1))
}

/// The process `pid`, if it runs.
#[cfg(target_os = "linux")]
pub fn one(pid: u32) -> Option<Process> {
    // SAFETY: as above.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let proc = Path::new("/proc");
    read_one(proc, pid, u64::try_from(ticks).unwrap_or(100).max(1), boot_time(proc)?)
}

/// [`all`] of the `/proc` at `proc`, whose clock ticks `ticks` times a second.
pub fn all_in(proc: &Path, ticks: u64) -> Vec<Process> {
    let Some(boot) = boot_time(proc) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(proc) else { return Vec::new() };
    let pids = entries.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok());
    let mut found: Vec<Process> = pids.filter_map(|pid| read_one(proc, pid, ticks, boot)).collect();
    found.sort_by_key(|p| p.pid);
    found
}

/// When the computer started, in Unix seconds (`btime` in `/proc/stat`).
fn boot_time(proc: &Path) -> Option<u64> {
    std::fs::read_to_string(proc.join("stat")).ok()?.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())
}

fn read_one(proc: &Path, pid: u32, ticks: u64, boot: u64) -> Option<Process> {
    let dir = proc.join(pid.to_string());
    let stat = std::fs::read(dir.join("stat")).ok()?;
    let Stat { name, state, parent, started } = read_stat(&stat)?;
    // One that ended and that nobody asked about yet, or that is being taken away.
    if matches!(state, b'Z' | b'X' | b'x') {
        return None;
    }
    let args = std::fs::read(dir.join("cmdline")).map(|line| arguments(&line)).unwrap_or_default();
    let exe = std::fs::read_link(dir.join("exe")).ok();
    let name = whole_name(&name, &args, exe.as_deref());
    let started_ns = (boot as u128 * 1_000_000_000 + started as u128 * 1_000_000_000 / ticks as u128) as u64;
    Some(Process { pid, parent, name, args, exe, started_ns })
}

struct Stat {
    name: String,
    state: u8,
    parent: u32,
    /// Clock ticks after the computer's start.
    started: u64,
}

/// `/proc/<pid>/stat`: `pid (name) state parent …`, the start its 22nd field. The name may hold anything, parentheses and spaces
/// too: it ends at the last `)`.
fn read_stat(stat: &[u8]) -> Option<Stat> {
    let (open, close) = (stat.iter().position(|b| *b == b'(')?, stat.iter().rposition(|b| *b == b')')?);
    let name = String::from_utf8_lossy(stat.get(open + 1..close)?).into_owned();
    let rest = std::str::from_utf8(stat.get(close + 1..)?).ok()?;
    let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
    Some(Stat { name, state: *fields.first()?.as_bytes().first()?, parent: fields.get(1)?.parse().ok()?, started: fields.get(19)?.parse().ok()? })
}

/// `/proc/<pid>/cmdline`: each argument ended by a NUL.
fn arguments(line: &[u8]) -> Vec<String> {
    let line = line.strip_suffix(b"\0").unwrap_or(line);
    if line.is_empty() {
        return Vec::new();
    }
    line.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

/// What the kernel keeps of a name (`TASK_COMM_LEN` without its end).
const KEPT: usize = 15;

/// The program's whole name: what the kernel kept of it, or, where that was cut, the file's of the command line or of the program.
fn whole_name(kept: &str, args: &[String], exe: Option<&Path>) -> String {
    let file = |path: &str| path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
    let windows = |name: &str| name.len() > 4 && name.is_char_boundary(name.len() - 4) && name[name.len() - 4..].eq_ignore_ascii_case(".exe");
    if windows(kept) {
        return kept.to_string();
    }
    // Wine's loader, before it gave the process its program's name: `wine64 C:\…\VRChat.exe`.
    let first = args.first().map(|a| file(a)).unwrap_or_default();
    let program = match first.as_str() {
        "wine" | "wine64" | "wine-preloader" | "wine64-preloader" => args.get(1).map(|a| file(a)).filter(|n| windows(n)),
        _ => None,
    };
    if let Some(program) = program {
        return program;
    }
    if kept.len() >= KEPT {
        let exe = exe.and_then(|e| e.file_name()).map(|n| n.to_string_lossy().into_owned());
        // (The kernel cuts at bytes: a character cut in two came out as a replacement one.)
        let begins = kept.trim_end_matches('\u{fffd}');
        if let Some(whole) = [Some(first), exe].into_iter().flatten().find(|n| n.starts_with(begins)) {
            return whole;
        }
    }
    kept.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Proc(PathBuf);
    impl Drop for Proc {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Proc {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("orbiters-proc-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("stat"), "cpu  1 2 3\nintr 5\nbtime 1700000000\nprocesses 9\n").unwrap();
            std::fs::write(root.join("uptime"), "1 1").unwrap();
            Self(root)
        }

        /// A process, with threads of its name as Wine's have.
        fn add(&self, pid: u32, name: &str, state: char, started: u64, args: &[&str]) {
            let dir = self.0.join(pid.to_string());
            let kept: String = name.chars().take(KEPT).collect();
            let stat = format!("{pid} ({kept}) {state} 7 1 1 0 -1 4194304 1 0 0 0 5 5 0 0 20 0 99 0 {started} 1 1");
            std::fs::create_dir_all(dir.join("task").join((pid + 1).to_string())).unwrap();
            std::fs::write(dir.join("stat"), &stat).unwrap();
            std::fs::write(dir.join("task").join((pid + 1).to_string()).join("stat"), stat.replace(&pid.to_string(), &(pid + 1).to_string())).unwrap();
            let line: Vec<u8> = args.iter().flat_map(|a| a.bytes().chain([0])).collect();
            std::fs::write(dir.join("cmdline"), line).unwrap();
        }
    }

    #[test]
    fn a_vrchat_is_one_process_whatever_its_threads_are_named() {
        let proc = Proc::new();
        proc.add(4242, "VRChat.exe", 'S', 12_345, &[r"Z:\steam\VRChat\VRChat.exe", "--no-vr", "--profile=2"]);
        proc.add(50, "systemd", 'S', 1, &["/sbin/init"]);
        let all = all_in(&proc.0, 100);
        assert_eq!(all.iter().map(|p| p.pid).collect::<Vec<_>>(), [50, 4242]);
        let game = all.iter().find(|p| p.is_vrchat()).unwrap();
        assert_eq!(game.args, [r"Z:\steam\VRChat\VRChat.exe", "--no-vr", "--profile=2"]);
        assert_eq!(game.parent, 7);
        assert_eq!(game.started_ns, 1_700_000_123_450_000_000);
        assert_eq!(game.started_s(), 1_700_000_123);
        assert_eq!(all.iter().filter(|p| p.is_vrchat()).count(), 1);
        assert_eq!(crate::vrchat_launch::profile_of(&game.args), 2);
    }

    #[test]
    fn a_name_the_kernel_cut_is_whole_again() {
        let proc = Proc::new();
        proc.add(1, "start_protected_game.exe", 'S', 1, &[r"C:\VRChat\start_protected_game.exe", "x"]);
        // Cut before ".exe", as the kernel does it.
        assert_eq!(std::fs::read_to_string(proc.0.join("1").join("stat")).unwrap().split(['(', ')']).nth(1), Some("start_protected"));
        // Its name given by a program that has another command line: what the kernel kept.
        proc.add(2, "a name with (parens", 'R', 1, &["/usr/bin/else"]);
        proc.add(3, "wine64-preloader", 'S', 1, &["/opt/wine64", r"Z:\x\VRChat.exe"]);
        let names: Vec<String> = all_in(&proc.0, 100).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["start_protected_game.exe", "a name with (pa", "VRChat.exe"]);
    }

    #[test]
    fn what_ended_or_cannot_be_read_is_no_process() {
        let proc = Proc::new();
        proc.add(10, "VRChat.exe", 'Z', 1, &[]);
        std::fs::create_dir_all(proc.0.join("11")).unwrap();
        std::fs::create_dir_all(proc.0.join("self")).unwrap();
        std::fs::create_dir_all(proc.0.join("12")).unwrap();
        std::fs::write(proc.0.join("12").join("stat"), "12 (odd").unwrap();
        // Another person's: a name, nothing more.
        proc.add(13, "vrserver", 'S', 100, &[]);
        std::fs::remove_file(proc.0.join("13").join("cmdline")).unwrap();
        let all = all_in(&proc.0, 100);
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].name.as_str(), all[0].args.len(), all[0].exe.clone()), ("vrserver", 0, None));
        assert!(all_in(&proc.0.join("none"), 100).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_test_is_one_of_them_and_its_threads_are_not() {
        let held = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..3).map(|_| { let held = held.clone(); std::thread::spawn(move || held.wait()) }).collect();
        let me = std::process::id();
        let all = all();
        let mine: Vec<_> = all.iter().filter(|p| p.pid == me).collect();
        assert_eq!(mine.len(), 1);
        // No thread of this process is listed: each has this process's program and would have its parent.
        assert!(!all.iter().any(|p| p.pid != me && p.parent == mine[0].parent && p.args == mine[0].args), "a thread was taken for a process");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(mine[0].started_s() <= now + 2 && mine[0].started_s() + 24 * 3600 > now, "{} {now}", mine[0].started_s());
        assert_eq!(mine[0].exe.as_deref(), std::env::current_exe().ok().as_deref());
        assert_eq!(one(me).as_ref(), Some(mine[0]));
        assert_eq!(one(u32::MAX), None);
        held.wait();
        for t in threads {
            let _ = t.join();
        }
    }
}
