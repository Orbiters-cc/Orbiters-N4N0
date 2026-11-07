//! What step 1 lowers, and what it must never touch. Lowering is one thing only: a program on the allowlist, running as
//! this user, at exactly Normal priority, goes to Below Normal while VRChat has the scene. Nothing is ever closed,
//! suspended or killed, and no administrator rights are used.
//!
//! **The refusal is in the lowest layer.** Every route to changing a priority goes through [`set_class`], which refuses by
//! name before any handle is opened. Whatever a bug, a doctored journal or a future list says, VRChat, its anti-cheat,
//! SteamVR and the runtimes' processes cannot be changed from here.
//!
//! Listing processes opens none of them: names, creation times and processor time all come from one system call
//! ([`scan`]), so looking at what is running does not open VRChat either. A handle is opened only for a process whose name
//! passed already, and its real image name is checked again on that handle before it is changed.

use crate::journal::{Access, Applied, Change, Read, Store, Target, Unreadable, Value};

pub const NORMAL_PRIORITY: i64 = 0x20;
pub const BELOW_NORMAL_PRIORITY: i64 = 0x4000;

/// The programs step 1 may lower, by exact exe name without `.exe`: heavy, and none of them part of what VR needs while
/// someone plays. Everything else is left alone by design: Discord, OBS, audio, VR tools, overlays, tracking, Steam, security
/// software.
pub const ALLOWLIST: [&str; 24] = [
    "chrome", "msedge", "firefox", "brave", "opera", "code", "devenv", "rider64", "Unity", "blender", "node", "python", "pythonw", "rustc", "cargo", "link", "cl", "msbuild", "dotnet", "ffmpeg", "claude",
    "codex", "Docker Desktop", "com.docker.backend",
];

/// Names refused whatever any list says, in lower case. A name starting with one of these is refused too: `vr` takes in all
/// of SteamVR's (`vrserver`, `vrcompositor`, `vrmonitor`, `vrdashboard`, `vrstartup`, `vrwebhelper`) and VRChat itself.
const NEVER_STARTING: [&str; 20] = [
    "vr",
    "start_protected_game",
    "easyanticheat",
    "eac",
    "orbiters",
    "steam",
    "gameoverlayui",
    "ovr",
    "oculus",
    "meta",
    "virtualdesktop",
    "bigscreen",
    "pimax",
    "wmr",
    "mixedreality",
    "openxr",
    "alvr",
    "pico",
    "streaming_assistant",
    "lovr",
];

fn stem(name: &str) -> String {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name).to_lowercase();
    name.strip_suffix(".exe").map(str::to_string).unwrap_or(name)
}

/// VRChat, its anti-cheat, SteamVR, the VR runtimes or Orbiters itself: never lowered, never restored, never opened.
pub fn never_touch(name: &str) -> bool {
    let stem = stem(name);
    NEVER_STARTING.iter().any(|n| stem.starts_with(n))
}

/// May step 1 lower a program of this name.
pub fn may_lower(name: &str) -> bool {
    let stem = stem(name);
    !never_touch(&stem) && ALLOWLIST.iter().any(|a| a.eq_ignore_ascii_case(&stem))
}

/// What the system says of a running process, from [`scan`]: no process was opened to learn it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub pid: u32,
    pub name: String,
    /// FILETIME, the same as `GetProcessTimes` gives.
    pub created: u64,
    /// Kernel and user time so far, in 100 ns units.
    pub cpu: u64,
}

/// What opening a process finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub image: String,
    pub created: u64,
    pub class: i64,
    pub same_user: bool,
}

/// The operating system, as far as this file needs it. Tests give their own table of processes, or the real one on
/// processes they started themselves.
pub trait ProcessOs {
    /// Opens the process to read it: only ever called for a name that is not refused.
    fn open(&mut self, pid: u32) -> Option<Opened>;
    fn set_class(&mut self, pid: u32, class: i64) -> bool;
}

/// Reads a process's priority class by name, PID and creation time: what the journal's table compares.
pub fn read_class(os: &mut dyn ProcessOs, pid: u32, created: u64, name: &str) -> Read {
    // Not opened, and the entry goes: no journal, doctored or old, makes this touch one of these.
    if never_touch(name) {
        return Read::Gone;
    }
    match os.open(pid) {
        Some(opened) if opened.created == created && stem(&opened.image) == stem(name) && opened.same_user => Read::Value(Value::Int(opened.class)),
        _ => Read::Gone,
    }
}

/// Sets a process's priority class, and only by the same checks: the one place that does.
pub fn set_class(os: &mut dyn ProcessOs, pid: u32, created: u64, name: &str, class: i64) -> bool {
    if never_touch(name) {
        return false;
    }
    // Lowering needs the list; going back to Normal does not, should a later version have dropped a name from it.
    if class != NORMAL_PRIORITY && !may_lower(name) {
        return false;
    }
    match os.open(pid) {
        Some(opened) if opened.created == created && stem(&opened.image) == stem(name) && opened.same_user && !never_touch(&opened.image) => os.set_class(pid, class),
        _ => false,
    }
}

/// [`Access`] for programs' priorities, by [`set_class`]; SteamVR's settings are not its to reach.
pub struct ProcessAccess<'a>(pub &'a mut dyn ProcessOs);

impl Access for ProcessAccess<'_> {
    fn read(&mut self, target: &Target) -> Read {
        match target {
            Target::Process { pid, created, name } => read_class(self.0, *pid, *created, name),
            Target::SteamVr { .. } => Read::Unreachable,
        }
    }
    fn write(&mut self, target: &Target, value: &Value) -> bool {
        match (target, value) {
            (Target::Process { pid, created, name }, Value::Int(class)) => set_class(self.0, *pid, *created, name, *class),
            _ => false,
        }
    }
}

/// The programs on the list now, without opening anything: what the group test looks at.
pub fn candidates(seen: &[Seen]) -> Vec<&Seen> {
    seen.iter().filter(|p| may_lower(&p.name)).collect()
}

/// Lowers what is on the list, at exactly Normal, as this user. What is already lowered by the journal stays as it is; a
/// program someone set to another class is theirs, and not touched.
pub fn lower(store: &Store, os: &mut dyn ProcessOs, seen: &[Seen]) -> Result<usize, Unreadable> {
    let mut changes = Vec::new();
    for program in candidates(seen) {
        let Some(opened) = os.open(program.pid) else { continue };
        if !opened.same_user || opened.created != program.created || opened.class != NORMAL_PRIORITY || !may_lower(&opened.image) {
            continue;
        }
        changes.push(Change { target: Target::Process { pid: program.pid, created: program.created, name: stem(&program.name) }, value: Value::Int(BELOW_NORMAL_PRIORITY), leave_with: None });
    }
    if changes.is_empty() {
        return Ok(0);
    }
    let applied = store.apply(&mut ProcessAccess(os), &changes, &mut |_| {})?;
    Ok(applied.iter().filter(|a| **a == Applied::Done).count())
}

/// Puts back every program lowered: only if each is still the same process and its class is still what was set.
pub fn restore_all(store: &Store, os: &mut dyn ProcessOs) -> Result<crate::journal::Restored, Unreadable> {
    store.restore(&mut ProcessAccess(os), &|e| matches!(e.target, Target::Process { .. }), &mut |_| {})
}

/// Processor use between two looks at the system.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuUse {
    /// All of it, 1.0 being every core busy.
    pub total: f32,
    /// What the allowlisted programs used together, in cores.
    pub listed_cores: f32,
    /// The five heaviest programs by name, in cores: nothing else about them.
    pub top: Vec<(String, f32)>,
}

/// Compares two looks at the system, taken `seconds` apart on a computer of `cores` logical processors.
pub fn cpu_use(before: &[Seen], after: &[Seen], seconds: f32, cores: f32) -> CpuUse {
    use std::collections::HashMap;
    if seconds <= 0.0 {
        return CpuUse::default();
    }
    let earlier: HashMap<(u32, u64), u64> = before.iter().map(|p| ((p.pid, p.created), p.cpu)).collect();
    let mut by_name: HashMap<String, f32> = HashMap::new();
    let (mut total, mut listed) = (0.0f32, 0.0f32);
    for p in after {
        // The System Idle Process's time is what nobody used.
        if p.pid == 0 {
            continue;
        }
        let used = p.cpu.saturating_sub(earlier.get(&(p.pid, p.created)).copied().unwrap_or(p.cpu)) as f32 / 10_000_000.0 / seconds;
        total += used;
        if may_lower(&p.name) {
            listed += used;
        }
        *by_name.entry(stem(&p.name)).or_default() += used;
    }
    let mut top: Vec<(String, f32)> = by_name.into_iter().collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(5);
    CpuUse { total: total / cores.max(1.0), listed_cores: listed, top }
}

/// SteamVR's server or monitor runs: while either does, its settings file is not to be written.
pub fn steamvr_running(seen: &[Seen]) -> bool {
    seen.iter().any(|p| matches!(stem(&p.name).as_str(), "vrserver" | "vrmonitor"))
}

/// The processes running now. No process is opened for this: the system lists them all at once.
#[cfg(windows)]
pub fn scan() -> Vec<Seen> {
    use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
    // SYSTEM_PROCESS_INFORMATION's beginning, on 64-bit Windows.
    #[repr(C)]
    struct Head {
        next: u32,
        threads: u32,
        _private: [u8; 24],
        created: i64,
        user: i64,
        kernel: i64,
        name_bytes: u16,
        _name_max: u16,
        name: *const u16,
        _priority: i32,
        pid: usize,
    }
    let mut buffer = vec![0u64; 128 * 1024];
    for _ in 0..4 {
        let mut needed = 0u32;
        // SAFETY: the buffer is as large as said, and aligned for the system's own structures.
        let status = unsafe { NtQuerySystemInformation(SystemProcessInformation, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed) };
        if status.is_ok() {
            let mut out = Vec::with_capacity(512);
            let base = buffer.as_ptr().cast::<u8>();
            let mut at = 0usize;
            loop {
                // SAFETY: each entry's offset comes from the one before, within what the system filled.
                let head = unsafe { &*base.add(at).cast::<Head>() };
                let name = if head.name.is_null() || head.name_bytes == 0 {
                    String::new()
                } else {
                    // SAFETY: the system's own text, of the length it says, inside the buffer.
                    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(head.name, head.name_bytes as usize / 2) })
                };
                out.push(Seen { pid: head.pid as u32, name, created: head.created as u64, cpu: (head.user + head.kernel) as u64 });
                if head.next == 0 {
                    return out;
                }
                at += head.next as usize;
            }
        }
        // Too small: processes come and go, so a little more than asked.
        buffer = vec![0u64; (needed as usize / 8 + 16 * 1024).max(buffer.len() * 2)];
    }
    Vec::new()
}

#[cfg(not(windows))]
pub fn scan() -> Vec<Seen> {
    Vec::new()
}

/// How many logical processors, for [`cpu_use`].
pub fn core_count() -> f32 {
    std::thread::available_parallelism().map_or(1.0, |n| n.get() as f32)
}

/// The real system's processes.
pub struct RealOs;

#[cfg(windows)]
impl ProcessOs for RealOs {
    fn open(&mut self, pid: u32) -> Option<Opened> {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
        use windows::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows::Win32::System::Threading::{GetCurrentProcess, GetPriorityClass, GetProcessTimes, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: the least rights there are, and closed at the end of this call.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        struct Close(HANDLE);
        impl Drop for Close {
            fn drop(&mut self) {
                // SAFETY: a handle opened here, closed once.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
        let _process = Close(process);
        let mut wide = vec![0u16; 1024];
        let mut length = wide.len() as u32;
        // SAFETY: the buffer's length is what is passed.
        unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(wide.as_mut_ptr()), &mut length) }.ok()?;
        let image = String::from_utf16_lossy(&wide[..length as usize]);
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: valid out parameters.
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
        // SAFETY: a handle with query rights.
        let class = unsafe { GetPriorityClass(process) } as i64;
        // Whose it is: another user's programs are not this one's to change, and asking the same of both tokens is the way to tell.
        let user_of = |process: HANDLE| -> Option<Vec<u8>> {
            let mut token = HANDLE::default();
            // SAFETY: a valid out parameter, closed below.
            unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
            let _token = Close(token);
            let mut bytes = vec![0u8; 256];
            let mut size = 0u32;
            // SAFETY: the buffer's size is what is passed.
            unsafe { GetTokenInformation(token, TokenUser, Some(bytes.as_mut_ptr().cast()), bytes.len() as u32, &mut size) }.ok()?;
            Some(bytes)
        };
        // SAFETY: the pseudo handle of this process needs no closing.
        let mine = user_of(unsafe { GetCurrentProcess() });
        let theirs = user_of(process);
        let same_user = match (&mine, &theirs) {
            (Some(a), Some(b)) => {
                // SAFETY: both buffers hold a TOKEN_USER written above, whose SID lies inside them.
                unsafe { EqualSid((*a.as_ptr().cast::<TOKEN_USER>()).User.Sid, (*b.as_ptr().cast::<TOKEN_USER>()).User.Sid) }.is_ok()
            }
            _ => false,
        };
        Some(Opened { image, created: (created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64, class, same_user })
    }

    fn set_class(&mut self, pid: u32, class: i64) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, SetPriorityClass, PROCESS_CREATION_FLAGS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION};
        // SAFETY: the rights it needs and no others, closed below.
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION, false, pid) }) else { return false };
        // SAFETY: a live handle with the right to set this.
        let done = unsafe { SetPriorityClass(process, PROCESS_CREATION_FLAGS(class as u32)) }.is_ok();
        // SAFETY: opened above, closed once.
        unsafe {
            let _ = CloseHandle(process);
        }
        done
    }
}

#[cfg(not(windows))]
impl ProcessOs for RealOs {
    fn open(&mut self, _pid: u32) -> Option<Opened> {
        None
    }
    fn set_class(&mut self, _pid: u32, _class: i64) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::tests::Scratch;
    use crate::journal::NotArmed;
    use std::collections::HashMap;

    /// A table of processes, which counts each open: being refused must mean not being opened either.
    #[derive(Default)]
    struct Table {
        processes: HashMap<u32, Opened>,
        opened: Vec<u32>,
        set: Vec<(u32, i64)>,
    }

    impl Table {
        fn add(&mut self, pid: u32, image: &str, class: i64) {
            self.processes.insert(pid, Opened { image: format!("C:\\Programs\\{image}"), created: pid as u64 * 100, class, same_user: true });
        }
    }

    impl ProcessOs for Table {
        fn open(&mut self, pid: u32) -> Option<Opened> {
            self.opened.push(pid);
            self.processes.get(&pid).cloned()
        }
        fn set_class(&mut self, pid: u32, class: i64) -> bool {
            self.set.push((pid, class));
            self.processes.get_mut(&pid).map(|p| p.class = class).is_some()
        }
    }

    fn seen(pid: u32, name: &str) -> Seen {
        Seen { pid, name: name.into(), created: pid as u64 * 100, cpu: 0 }
    }

    const NEVER: [&str; 26] = [
        "VRChat.exe",
        "VRChat",
        "vrchat.EXE",
        "start_protected_game.exe",
        "EasyAntiCheat.exe",
        "EasyAntiCheat_EOS.exe",
        "EasyAntiCheat_Setup.exe",
        "EACefSubProcess.exe",
        "vrserver.exe",
        "vrcompositor.exe",
        "vrmonitor.exe",
        "vrdashboard.exe",
        "vrstartup.exe",
        "vrwebhelper.exe",
        "vrservice.exe",
        "orbiters-desktop.exe",
        "Orbiters-Setup.exe",
        "steam.exe",
        "steamwebhelper.exe",
        "SteamVR.exe",
        "OVRServer_x64.exe",
        "OculusClient.exe",
        "VirtualDesktop.Streamer.exe",
        "VirtualDesktop.Service.exe",
        "Pimax.exe",
        "C:\\Program Files\\VRChat\\VRChat.exe",
    ];

    /// The hard line. Every one of these is refused by name, whatever the list says.
    #[test]
    fn every_never_touch_name_is_refused() {
        for name in NEVER {
            assert!(never_touch(name), "{name}");
            assert!(!may_lower(name), "{name}");
        }
    }

    /// Nothing on the allowlist could ever be one of them, nor anything that looks like VR.
    #[test]
    fn the_list_and_the_refusals_do_not_meet() {
        for name in ALLOWLIST {
            assert!(!never_touch(name), "{name} is on the list and refused");
            assert!(may_lower(name), "{name}");
            assert!(may_lower(&format!("{name}.exe")), "{name}.exe");
            assert!(may_lower(&name.to_uppercase()), "{name} in other case");
        }
        for name in ["Discord.exe", "obs64.exe", "explorer.exe", "audiodg.exe", "Spotify.exe", "svchost.exe", "chrome2.exe", "notchrome.exe", ""] {
            assert!(!may_lower(name), "{name}: only exact names");
        }
    }

    /// Being refused means never opened, by each route: lowering, setting back, reading for the journal.
    #[test]
    fn a_refused_process_is_not_even_opened() {
        for (i, name) in NEVER.into_iter().enumerate() {
            let pid = i as u32 + 1;
            let mut os = Table::default();
            os.add(pid, name, NORMAL_PRIORITY);
            assert!(!set_class(&mut os, pid, pid as u64 * 100, name, BELOW_NORMAL_PRIORITY), "{name}");
            assert!(!set_class(&mut os, pid, pid as u64 * 100, name, NORMAL_PRIORITY), "{name}");
            assert_eq!(read_class(&mut os, pid, pid as u64 * 100, name), Read::Gone, "{name}");
            assert!(os.opened.is_empty() && os.set.is_empty(), "{name}: {:?}", os.opened);
        }
    }

    /// A scan with all of it in it lowers only what is allowed, and opens nothing else.
    #[test]
    fn lowering_a_whole_system_leaves_everything_else_alone() {
        let dir = Scratch::new("whole");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::default();
        let mut list = Vec::new();
        for (i, name) in NEVER.into_iter().enumerate() {
            os.add(i as u32 + 1, name, NORMAL_PRIORITY);
            list.push(seen(i as u32 + 1, name));
        }
        for (i, name) in ["Discord.exe", "obs64.exe", "explorer.exe"].into_iter().enumerate() {
            os.add(100 + i as u32, name, NORMAL_PRIORITY);
            list.push(seen(100 + i as u32, name));
        }
        os.add(200, "chrome.exe", NORMAL_PRIORITY);
        list.push(seen(200, "chrome.exe"));
        assert_eq!(lower(&store, &mut os, &list).unwrap(), 1);
        assert_eq!(os.set, vec![(200, BELOW_NORMAL_PRIORITY)]);
        assert!(os.opened.iter().all(|pid| *pid == 200), "{:?}", os.opened);
    }

    /// A journal someone edited to name VRChat: the entry goes, and VRChat is not opened or set.
    #[test]
    fn a_doctored_journal_cannot_reach_vrchat() {
        let dir = Scratch::new("doctored");
        std::fs::write(
            dir.0.join("journal.json"),
            r#"{"version":1,"entries":[{"target":{"kind":"process","pid":7,"created":700,"name":"VRChat"},"original":{"type":"int","value":32},"written":{"type":"int","value":16384},"stage":"applied","at":"","app_version":""}]}"#,
        )
        .unwrap();
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::default();
        os.add(7, "VRChat.exe", BELOW_NORMAL_PRIORITY);
        let got = restore_all(&store, &mut os).unwrap();
        assert_eq!(got.gone.len(), 1);
        assert!(os.opened.is_empty() && os.set.is_empty());
        assert!(store.entries().unwrap().is_empty());
    }

    /// What was lowered comes back; a program the person set to another class stays theirs; a program that is another one
    /// by now, on the same PID, is left alone; one that ended is forgotten.
    #[test]
    fn programs_come_back_only_if_they_are_still_ours() {
        let dir = Scratch::new("back");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::default();
        let names = [(1, "chrome.exe"), (2, "code.exe"), (3, "cargo.exe"), (4, "node.exe")];
        let list: Vec<Seen> = names.iter().map(|(pid, name)| seen(*pid, name)).collect();
        for (pid, name) in names {
            os.add(pid, name, NORMAL_PRIORITY);
        }
        assert_eq!(lower(&store, &mut os, &list).unwrap(), 4);
        // Set by hand.
        os.processes.get_mut(&2).unwrap().class = 0x80;
        // PID reuse: same number, another program, started later.
        os.processes.insert(3, Opened { image: "C:\\Programs\\cargo.exe".into(), created: 99_999, class: BELOW_NORMAL_PRIORITY, same_user: true });
        // It ended.
        os.processes.remove(&4);
        let got = restore_all(&store, &mut os).unwrap();
        assert_eq!(os.processes[&1].class, NORMAL_PRIORITY, "still ours: back");
        assert_eq!(os.processes[&2].class, 0x80, "the person's");
        assert_eq!(os.processes[&3].class, BELOW_NORMAL_PRIORITY, "not the same process");
        assert_eq!((got.back.len(), got.kept.len(), got.gone.len()), (1, 1, 2));
        assert!(store.entries().unwrap().is_empty());
    }

    /// Already a class of its own, or another user's: not touched at all.
    #[test]
    fn only_normal_programs_of_this_user_are_lowered() {
        let dir = Scratch::new("normal");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::default();
        os.add(1, "chrome.exe", 0x80);
        os.add(2, "chrome.exe", NORMAL_PRIORITY);
        os.processes.get_mut(&2).unwrap().same_user = false;
        assert_eq!(lower(&store, &mut os, &[seen(1, "chrome.exe"), seen(2, "chrome.exe")]).unwrap(), 0);
        assert!(os.set.is_empty());
    }

    #[test]
    fn processor_use_is_counted_by_name_and_in_cores() {
        let at = |pid, name: &str, seconds: f32| Seen { pid, name: name.into(), created: 1, cpu: (seconds * 10_000_000.0) as u64 };
        let before = [at(0, "Idle", 0.0), at(1, "chrome.exe", 1.0), at(2, "chrome.exe", 1.0), at(3, "Discord.exe", 0.0), at(4, "VRChat.exe", 10.0)];
        let after = [at(0, "Idle", 100.0), at(1, "chrome.exe", 6.0), at(2, "chrome.exe", 6.0), at(3, "Discord.exe", 1.0), at(4, "VRChat.exe", 20.0), at(5, "node.exe", 7.0)];
        let got = cpu_use(&before, &after, 10.0, 8.0);
        assert!((got.listed_cores - 1.0).abs() < 0.01, "only the listed, and a new one counts from now: {got:?}");
        assert_eq!(got.top[0].0, "chrome");
        assert_eq!(got.top[1].0, "vrchat");
        assert!((got.total - (1.0 + 1.0 + 0.1) / 8.0).abs() < 0.01, "{got:?}");
    }

    /// Reads the real system, changes nothing: this very test's process is in the list, under its name.
    #[cfg(windows)]
    #[test]
    fn the_real_list_opens_nothing_and_finds_this_process() {
        let list = scan();
        let me = list.iter().find(|p| p.pid == std::process::id()).expect("this process is listed");
        assert!(me.name.to_lowercase().starts_with("orbiters_booster"), "{}", me.name);
        assert!(me.created != 0);
        assert!(list.len() > 5);
        // The same creation time a handle gives, or the journal's comparison would never match.
        let opened = RealOs.open(std::process::id()).unwrap();
        assert_eq!(opened.created, me.created);
        assert_eq!(opened.class, NORMAL_PRIORITY);
        assert!(opened.same_user);
    }

    /// A copy of this test program under a given name, for a process of the test's own that can be lowered for real and
    /// killed. Nothing but this test sees them, and no name used here is one the app watches for.
    #[cfg(windows)]
    struct Sleeper {
        child: std::process::Child,
    }

    #[cfg(windows)]
    impl Sleeper {
        fn start(dir: &std::path::Path, name: &str) -> Self {
            let exe = dir.join(name);
            std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
            let child = std::process::Command::new(&exe)
                .args(["--exact", "programs::tests::sleeper_body", "--ignored", "--nocapture"])
                .env("ORBITERS_BOOSTER_SLEEP", "1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            Self { child }
        }
    }

    #[cfg(windows)]
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "what `Sleeper` runs"]
    fn sleeper_body() {
        if std::env::var_os("ORBITERS_BOOSTER_SLEEP").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
    }

    #[cfg(windows)]
    fn seen_of(pid: u32) -> Seen {
        for _ in 0..200 {
            if let Some(found) = scan().into_iter().find(|p| p.pid == pid) {
                return found;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("process {pid} is not listed")
    }

    /// The real call on a real process, the test's own child, only ever: lowered, and back exactly.
    #[cfg(windows)]
    #[test]
    fn a_real_process_is_lowered_and_comes_back_exactly() {
        let dir = Scratch::new("real");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let sleeper = Sleeper::start(&dir.0, "node.exe");
        let me = seen_of(sleeper.child.id());
        let mut os = RealOs;
        assert_eq!(os.open(me.pid).unwrap().class, NORMAL_PRIORITY);
        assert_eq!(lower(&store, &mut os, std::slice::from_ref(&me)).unwrap(), 1);
        assert_eq!(os.open(me.pid).unwrap().class, BELOW_NORMAL_PRIORITY);
        let got = restore_all(&store, &mut os).unwrap();
        assert_eq!(got.back.len(), 1);
        assert_eq!(os.open(me.pid).unwrap().class, NORMAL_PRIORITY);
    }

    /// The owner of the journal is killed with a real process lowered: the next one reads the journal and puts it back.
    #[cfg(windows)]
    #[test]
    fn killed_with_a_real_process_lowered_the_next_one_puts_it_back() {
        let dir = Scratch::new("real-kill");
        let sleeper = Sleeper::start(&dir.0, "node.exe");
        let me = seen_of(sleeper.child.id());
        let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "programs::tests::lowering_owner_body", "--ignored", "--nocapture"])
            .env("ORBITERS_BOOSTER_OWNER_DIR", &dir.0)
            .env("ORBITERS_BOOSTER_OWNER_PID", me.pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut os = RealOs;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !dir.0.join("lowered").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(os.open(me.pid).unwrap().class, BELOW_NORMAL_PRIORITY);
        owner.kill().unwrap();
        let _ = owner.wait();
        assert_eq!(os.open(me.pid).unwrap().class, BELOW_NORMAL_PRIORITY, "this gap is real until the next start: the owner's death alone restores nothing");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        assert_eq!(restore_all(&store, &mut os).unwrap().back.len(), 1);
        assert_eq!(os.open(me.pid).unwrap().class, NORMAL_PRIORITY);
        assert!(store.entries().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "what the kill test runs, in a process of its own"]
    fn lowering_owner_body() {
        let Some(dir) = std::env::var_os("ORBITERS_BOOSTER_OWNER_DIR").map(std::path::PathBuf::from) else { return };
        let pid: u32 = std::env::var("ORBITERS_BOOSTER_OWNER_PID").unwrap().parse().unwrap();
        let store = Store::new(&dir, Box::new(NotArmed));
        assert_eq!(lower(&store, &mut RealOs, std::slice::from_ref(&seen_of(pid))).unwrap(), 1);
        std::fs::write(dir.join("lowered"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    /// Real processes with names on the refused side, the test's own, each started and asked of: refused, and left at
    /// Normal. The names are not VRChat's or SteamVR's own, which the running app would take for the real thing.
    #[cfg(windows)]
    #[test]
    fn real_processes_under_refused_names_stay_as_they_are() {
        let dir = Scratch::new("real-refused");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        for name in ["vrscratch.exe", "EasyAntiCheat_scratch.exe", "orbiters_scratch.exe", "not_on_the_list.exe"] {
            let sleeper = Sleeper::start(&dir.0, name);
            let me = seen_of(sleeper.child.id());
            assert_eq!(lower(&store, &mut RealOs, std::slice::from_ref(&me)).unwrap(), 0, "{name}");
            assert!(!set_class(&mut RealOs, me.pid, me.created, &me.name, BELOW_NORMAL_PRIORITY), "{name}");
            assert_eq!(RealOs.open(me.pid).unwrap().class, NORMAL_PRIORITY, "{name}");
            assert!(store.entries().unwrap().is_empty(), "{name}");
        }
    }
}
