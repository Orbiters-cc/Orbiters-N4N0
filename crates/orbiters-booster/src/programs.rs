//! What step 1 slows down, and what it must never touch. It puts a program on the allowlist, running as this user, in
//! **efficiency mode** while VRChat has the scene: Windows' EcoQoS (`ProcessPowerThrottling`), what Task Manager's
//! "Efficiency mode" also sets. On a processor with efficiency cores Windows then runs it on those, at lower clocks, which
//! leaves the performance cores to VRChat. Nothing is ever closed, suspended or killed, and no administrator rights are used.
//!
//! **Why efficiency mode and not Below Normal priority.** A program started by one at Below Normal starts at Below Normal too:
//! Windows hands the class on to children (the CreateProcess documentation says so, and `tests/inheritance.rs` shows it on this
//! machine). Every new browser tab, and anything started from a lowered editor's terminal, would then be changed without being
//! recorded, and a browser link or an SDK's Build & Test can start VRChat itself, which the booster may never touch, so could
//! never put back. Efficiency mode is not handed on (the same test shows it), so the booster changes exactly the processes it
//! records, and "put everything back" means everything. What it gains is unmeasured, as the priority's was; on a processor
//! without efficiency cores it is probably smaller.
//!
//! **The refusals are in the lowest layer, and before any handle.** Every route reads or changes a process through
//! [`read_state`] and [`set_state`], which refuse a name on the never-list, and refuse any PID that a fresh list of the system's
//! processes does not show with the same name and creation time. Listing opens nothing: names, creation times and processor time
//! come from one system call ([`scan`]). Then a process is opened **read-only**, and its image and creation time are checked
//! before anything else is read from it. Only while that checked handle is held, which keeps its PID from going to another
//! process, is a second handle asked for with the right to change it.
//!
//! **What remains:** between the list and that first read-only open, a program can end and the system give its PID to another,
//! VRChat or its anti-cheat included. That one gets a single read-only look at its image name and creation time, and nothing more:
//! no change right, no token, no other read. No API opens a process other than by its PID, so this can't be closed further; it is
//! the access Task Manager and game overlays use on VRChat all the time.
//!
//! **Sound.** A program with an active audio session, playing or recording, is left alone, by name: a browser's audio service and
//! its tabs alike. Efficiency mode on a saturated processor would make it crackle, and the comparison, which counts only VRChat's
//! frames, would take that for a gain.

use crate::journal::{Access, Applied, Change, Read, Store, Target, Unreadable, Value};

/// A program's power throttling, as one number: `PROCESS_POWER_THROTTLING_STATE`'s two masks whole, the control mask in the high
/// half. All of them, timer resolution's included: what is put back is exactly what was there.
pub const fn pack(control: u32, state: u32) -> i64 {
    (control as i64) << 32 | state as i64
}

pub const fn unpack(value: i64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

/// Execution speed: the bit efficiency mode is.
const EXECUTION_SPEED: u32 = 1;
/// Nobody chose anything: the system decides all of it.
pub const SYSTEM_DECIDES: i64 = 0;
/// Efficiency mode on, and nothing else chosen.
pub const EFFICIENCY: i64 = pack(EXECUTION_SPEED, EXECUTION_SPEED);

/// Someone already chose this program's execution speed, on or off: theirs.
pub fn speed_chosen(value: i64) -> bool {
    unpack(value).0 & EXECUTION_SPEED != 0
}

/// The same throttling with efficiency mode on, every other bit as it was.
pub fn slowed(original: i64) -> i64 {
    let (control, state) = unpack(original);
    pack(control | EXECUTION_SPEED, state | EXECUTION_SPEED)
}

/// Whether this value puts the program in efficiency mode.
pub fn slows(value: i64) -> bool {
    let (control, state) = unpack(value);
    control & EXECUTION_SPEED != 0 && state & EXECUTION_SPEED != 0
}

/// The programs step 1 may slow down, by exact exe name without `.exe`: browsers, editors and compilers. None is a host for
/// what VR needs while someone plays. Left out on purpose: general hosts (Python, Node, .NET, Docker), which run trackers,
/// face tracking, OSC and voice tools; recorders (ffmpeg); AI agents (Claude, Codex), which run Orbiters' own bridge.
pub const ALLOWLIST: [&str; 15] =
    ["chrome", "msedge", "firefox", "brave", "opera", "code", "devenv", "rider64", "Unity", "blender", "rustc", "cargo", "link", "cl", "msbuild"];

/// Names refused whatever any list says, in lower case. A name starting with one of these is refused too: `vr` takes in all of
/// SteamVR's (`vrserver`, `vrcompositor`, `vrmonitor`, `vrdashboard`, `vrstartup`, `vrwebhelper`), VRChat itself and VRChat's
/// tools named after it (VRCX, VRCFaceTracking). The rest are anti-cheat, VR runtimes and streamers, recorders and streaming,
/// voice, tracking and OSC tools, and Orbiters.
const NEVER_STARTING: [&str; 36] = [
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
    "obs",
    "sharex",
    "streamlabs",
    "xsplit",
    "bandicam",
    "nvcontainer",
    "nvidia",
    "discord",
    "voicemeeter",
    "slimevr",
    "driver4vr",
    "kinect",
    "xso",
    "ovrtoolkit",
    "fpsvr",
    "advanced settings",
];

fn stem(name: &str) -> String {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name).to_lowercase();
    name.strip_suffix(".exe").map(str::to_string).unwrap_or(name)
}

/// VRChat, its anti-cheat, SteamVR, the VR runtimes, recorders, voice, tracking and OSC tools, or Orbiters: never changed,
/// never restored, never opened.
pub fn never_touch(name: &str) -> bool {
    let stem = stem(name);
    NEVER_STARTING.iter().any(|n| stem.starts_with(n))
}

/// May step 1 slow down a program of this name.
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
    /// Its power throttling, as [`pack`] has it.
    pub state: i64,
    pub same_user: bool,
}

/// The operating system, as far as this file needs it. Tests give their own table of processes, or the real one limited to
/// processes they started themselves.
pub trait ProcessOs {
    /// The processes running now, opening none.
    fn list(&mut self) -> Vec<Seen>;
    /// Opens the process read-only. If its image or creation time is not the one meant, nothing else is read: `None`. Only
    /// ever called for a process [`listed`] says is the one meant.
    fn open(&mut self, pid: u32, created: u64, name: &str) -> Option<Opened>;
    /// Opens it read-only, checks that it is the process of `created` named `name` and this user's, and only then, holding that
    /// handle, asks for the right to change it and sets its state.
    fn set_state(&mut self, pid: u32, created: u64, name: &str, state: i64) -> bool;
    /// The processes with an active audio session, playing or recording. `None`: that could not be found out.
    fn audible(&mut self) -> Option<Vec<u32>>;
}

/// Whether a fresh list shows this PID as the process meant: same name, same creation time, and not refused. A PID the
/// system has given to anything else since fails here, before anything is opened.
pub fn listed(list: &[Seen], pid: u32, created: u64, name: &str) -> bool {
    list.iter().any(|p| p.pid == pid && p.created == created && stem(&p.name) == stem(name) && !never_touch(&p.name))
}

fn same(opened: &Opened, created: u64, name: &str) -> bool {
    opened.created == created && stem(&opened.image) == stem(name) && opened.same_user && !never_touch(&opened.image)
}

/// Reads a process's state, by name, PID and creation time against `list`: what the journal's table compares.
pub fn read_state(os: &mut dyn ProcessOs, list: &[Seen], pid: u32, created: u64, name: &str) -> Read {
    // Not opened, and the entry goes: refused, ended, or its PID another process's now.
    if never_touch(name) || !listed(list, pid, created, name) {
        return Read::Gone;
    }
    match os.open(pid, created, name) {
        Some(opened) if same(&opened, created, name) => Read::Value(Value::Int(opened.state)),
        _ => Read::Gone,
    }
}

/// Sets a process's state, and only by the same checks: the one place that does. `lowering` is step 1 itself; otherwise this
/// is a restore, which can only ever take efficiency mode off, whatever a journal says.
pub fn set_state(os: &mut dyn ProcessOs, list: &[Seen], pid: u32, created: u64, name: &str, state: i64, lowering: bool) -> bool {
    if never_touch(name) || !listed(list, pid, created, name) {
        return false;
    }
    if slows(state) && !(lowering && may_lower(name)) {
        return false;
    }
    os.set_state(pid, created, name, state)
}

/// [`Access`] for programs, by [`read_state`] and [`set_state`], against a list taken when it is made. SteamVR's settings
/// are not its to reach.
pub struct ProcessAccess<'a> {
    os: &'a mut dyn ProcessOs,
    list: Vec<Seen>,
    lowering: bool,
}

impl<'a> ProcessAccess<'a> {
    /// For a restore: lists the system now.
    pub fn new(os: &'a mut dyn ProcessOs) -> Self {
        let list = os.list();
        Self { os, list, lowering: false }
    }

    fn lowering(os: &'a mut dyn ProcessOs, list: Vec<Seen>) -> Self {
        Self { os, list, lowering: true }
    }
}

impl Access for ProcessAccess<'_> {
    fn read(&mut self, target: &Target) -> Read {
        match target {
            Target::Process { pid, created, name } => read_state(self.os, &self.list, *pid, *created, name),
            Target::SteamVr { .. } => Read::Unreachable,
        }
    }
    fn write(&mut self, target: &Target, value: &Value) -> bool {
        match (target, value) {
            (Target::Process { pid, created, name }, Value::Int(state)) => set_state(self.os, &self.list, *pid, *created, name, *state, self.lowering),
            _ => false,
        }
    }
}

/// The programs on the list in `seen`, without opening anything: what the group test looks at.
pub fn candidates(seen: &[Seen]) -> Vec<&Seen> {
    seen.iter().filter(|p| may_lower(&p.name)).collect()
}

fn is_process(e: &crate::journal::Entry) -> bool {
    matches!(e.target, Target::Process { .. })
}

/// The names of the programs with an active audio session: all of a browser's processes go with its audio service's.
pub fn audible_names(list: &[Seen], audible: &[u32]) -> std::collections::HashSet<String> {
    list.iter().filter(|p| audible.contains(&p.pid)).map(|p| stem(&p.name)).collect()
}

/// Step 1 over a session: what the person chose stays theirs until the step changes.
#[derive(Default)]
pub struct Slower {
    /// Processes taken out of efficiency mode, or put back, by someone after the booster set them: not set again.
    respected: std::collections::HashSet<(u32, u64)>,
}

impl Slower {
    /// The step changed: what the person chose under the last one is no longer held back from.
    pub fn forget(&mut self) {
        self.respected.clear();
    }

    /// Puts what is on the list in efficiency mode, if it is this user's, nobody chose its execution speed, it has no active
    /// audio, and nobody took it out since the booster set it. First lets go of what someone changed (theirs from then on),
    /// and of what ended; puts back what has started playing or recording sound. Lists the system afresh itself. How many
    /// were set now.
    pub fn slow(&mut self, store: &Store, os: &mut dyn ProcessOs) -> Result<usize, Unreadable> {
        for target in store.release_changed(&mut ProcessAccess::new(os), &is_process)? {
            if let Target::Process { pid, created, .. } = target {
                self.respected.insert((pid, created));
            }
        }
        let list = os.list();
        let Some(audible) = os.audible() else {
            // Whether anything plays can't be told: nothing is slowed, and what was comes back.
            restore_all(store, os)?;
            return Ok(0);
        };
        let audible = audible_names(&list, &audible);
        if !audible.is_empty() {
            store.restore(&mut ProcessAccess::new(os), &|e| matches!(&e.target, Target::Process { name, .. } if audible.contains(name)), &mut |_| {})?;
        }
        let mut changes = Vec::new();
        for program in candidates(&list) {
            if self.respected.contains(&(program.pid, program.created)) || audible.contains(&stem(&program.name)) {
                continue;
            }
            let Some(opened) = os.open(program.pid, program.created, &program.name) else { continue };
            if !same(&opened, program.created, &program.name) || !may_lower(&opened.image) {
                continue;
            }
            // In efficiency mode already (the booster's, as the journal says), or someone else's choice either way.
            if speed_chosen(opened.state) {
                continue;
            }
            let target = Target::Process { pid: program.pid, created: program.created, name: stem(&program.name) };
            changes.push(Change { target, value: Value::Int(slowed(opened.state)), leave_with: None });
        }
        if changes.is_empty() {
            return Ok(0);
        }
        let applied = store.apply(&mut ProcessAccess::lowering(os, list), &changes, &mut |_| {})?;
        for (change, applied) in changes.iter().zip(&applied) {
            if let (Applied::ChangedSince, Target::Process { pid, created, .. }) = (applied, &change.target) {
                self.respected.insert((*pid, *created));
            }
        }
        Ok(applied.iter().filter(|a| **a == Applied::Done).count())
    }
}

/// One pass of step 1 with nothing remembered: what tests and a one-off use.
pub fn lower(store: &Store, os: &mut dyn ProcessOs) -> Result<usize, Unreadable> {
    Slower::default().slow(store, os)
}

/// Puts back every program changed: only if each is still the same process and still as the booster set it.
pub fn restore_all(store: &Store, os: &mut dyn ProcessOs) -> Result<crate::journal::Restored, Unreadable> {
    store.restore(&mut ProcessAccess::new(os), &is_process, &mut |_| {})
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

/// Compares two looks at the system, taken `seconds` apart on a computer of `cores` logical processors. Programs named in
/// `left_out` (playing sound) don't count as what step 1 could slow.
pub fn cpu_use(before: &[Seen], after: &[Seen], seconds: f32, cores: f32, left_out: &std::collections::HashSet<String>) -> CpuUse {
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
        if may_lower(&p.name) && !left_out.contains(&stem(&p.name)) {
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
mod real {
    use super::*;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetProcessInformation, GetProcessTimes, OpenProcess, OpenProcessToken, ProcessPowerThrottling, QueryFullProcessImageNameW,
        SetProcessInformation, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    };

    struct Close(HANDLE);

    impl Drop for Close {
        fn drop(&mut self) {
            // SAFETY: a handle opened here, closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn user_of(process: HANDLE) -> Option<Vec<u8>> {
        let mut token = HANDLE::default();
        // SAFETY: a valid out parameter, closed below.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        let _token = Close(token);
        let mut bytes = vec![0u8; 256];
        let mut size = 0u32;
        // SAFETY: the buffer's size is what is passed.
        unsafe { GetTokenInformation(token, TokenUser, Some(bytes.as_mut_ptr().cast()), bytes.len() as u32, &mut size) }.ok()?;
        Some(bytes)
    }

    /// Reads all [`Opened`] says from a read-only handle, if it is the process meant: its image and creation time first, and
    /// nothing more of a process that is another one (no throttling, no token).
    fn opened(process: HANDLE, expect_created: u64, expect_name: &str) -> Option<Opened> {
        let mut wide = vec![0u16; 1024];
        let mut length = wide.len() as u32;
        // SAFETY: the buffer's length is what is passed.
        unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(wide.as_mut_ptr()), &mut length) }.ok()?;
        let image = String::from_utf16_lossy(&wide[..length as usize]);
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: valid out parameters.
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
        let created = (created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64;
        if created != expect_created || stem(&image) != stem(expect_name) || never_touch(&image) {
            return None;
        }
        let mut throttling = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
        // SAFETY: a valid handle and a buffer of the size passed.
        unsafe {
            GetProcessInformation(
                process,
                ProcessPowerThrottling,
                (&mut throttling as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
                std::mem::size_of_val(&throttling) as u32,
            )
        }
        .ok()?;
        // Whose it is: another user's programs are not this one's to change, and asking the same of both tokens is the way to tell.
        // SAFETY: the pseudo handle of this process needs no closing.
        let mine = user_of(unsafe { GetCurrentProcess() });
        let theirs = user_of(process);
        let same_user = match (&mine, &theirs) {
            // SAFETY: both buffers hold a TOKEN_USER written above, whose SID lies inside them.
            (Some(a), Some(b)) => unsafe { EqualSid((*a.as_ptr().cast::<TOKEN_USER>()).User.Sid, (*b.as_ptr().cast::<TOKEN_USER>()).User.Sid) }.is_ok(),
            _ => false,
        };
        Some(Opened { image, created, state: pack(throttling.ControlMask, throttling.StateMask), same_user })
    }

    fn open_with(pid: u32, rights: PROCESS_ACCESS_RIGHTS) -> Option<Close> {
        // SAFETY: closed when dropped.
        unsafe { OpenProcess(rights, false, pid) }.ok().map(Close)
    }

    impl ProcessOs for RealOs {
        fn list(&mut self) -> Vec<Seen> {
            #[cfg(test)]
            panic!("tests list only the processes they started (`Only`), never the whole system to change");
            #[cfg(not(test))]
            scan()
        }

        fn open(&mut self, pid: u32, created: u64, name: &str) -> Option<Opened> {
            let process = open_with(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            opened(process.0, created, name)
        }

        fn audible(&mut self) -> Option<Vec<u32>> {
            audio_pids()
        }

        fn set_state(&mut self, pid: u32, created: u64, name: &str, state: i64) -> bool {
            // Read-only first, and checked.
            let Some(checked) = open_with(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return false };
            if !opened(checked.0, created, name).is_some_and(|o| same(&o, created, name)) {
                return false;
            }
            // Only now the right to change it. While `checked` is held the process object lives, so its PID can't be another
            // process's: this second open reaches the very process just checked.
            let Some(process) = open_with(pid, PROCESS_SET_INFORMATION) else { return false };
            let (control, on) = unpack(state);
            let throttling = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: control, StateMask: on };
            drop(checked);
            // SAFETY: a live handle with the right to set this, and the structure's size.
            unsafe {
                SetProcessInformation(
                    process.0,
                    ProcessPowerThrottling,
                    (&throttling as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    std::mem::size_of_val(&throttling) as u32,
                )
            }
            .is_ok()
        }
    }
}

/// The processes with an active audio session on any of Windows' active audio devices, playing or recording. No process is
/// opened: the audio service says which PIDs its sessions belong to.
#[cfg(windows)]
pub fn audio_pids() -> Option<Vec<u32>> {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        eAll, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    // SAFETY: COM calls on interfaces this function holds; each is released when dropped.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let collection = devices.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE).ok()?;
        let mut out = Vec::new();
        for i in 0..collection.GetCount().ok()? {
            let Ok(device) = collection.Item(i) else { continue };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(session) = sessions.GetSession(j) else { continue };
                if session.GetState().ok() != Some(AudioSessionStateActive) {
                    continue;
                }
                if let Ok(pid) = session.cast::<IAudioSessionControl2>().and_then(|s| s.GetProcessId()) {
                    out.push(pid);
                }
            }
        }
        Some(out)
    }
}

#[cfg(not(windows))]
pub fn audio_pids() -> Option<Vec<u32>> {
    None
}

#[cfg(not(windows))]
impl ProcessOs for RealOs {
    fn list(&mut self) -> Vec<Seen> {
        Vec::new()
    }
    fn open(&mut self, _pid: u32, _created: u64, _name: &str) -> Option<Opened> {
        None
    }
    fn audible(&mut self) -> Option<Vec<u32>> {
        None
    }
    fn set_state(&mut self, _pid: u32, _created: u64, _name: &str, _state: i64) -> bool {
        false
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::journal::tests::Scratch;
    use crate::journal::NotArmed;
    use std::collections::HashMap;

    /// A table of processes, which counts each open: being refused must mean not being opened either.
    #[derive(Default)]
    pub(crate) struct Table {
        pub processes: HashMap<u32, Opened>,
        pub opened: Vec<u32>,
        pub set: Vec<(u32, i64)>,
        /// What has an active audio session; `None`: it can't be told.
        pub audible: Option<Vec<u32>>,
        /// Without `audible` set, nothing plays.
        pub audible_default: bool,
    }

    impl Table {
        pub(crate) fn new() -> Self {
            Self { audible_default: true, ..Self::default() }
        }

        pub(crate) fn add(&mut self, pid: u32, image: &str, state: i64) {
            self.processes.insert(pid, Opened { image: format!("C:\\Programs\\{image}"), created: pid as u64 * 100, state, same_user: true });
        }
    }

    impl ProcessOs for Table {
        fn list(&mut self) -> Vec<Seen> {
            self.processes
                .iter()
                .map(|(pid, p)| Seen { pid: *pid, name: p.image.rsplit('\\').next().unwrap().to_string(), created: p.created, cpu: 0 })
                .collect()
        }
        fn open(&mut self, pid: u32, created: u64, name: &str) -> Option<Opened> {
            self.opened.push(pid);
            self.processes.get(&pid).filter(|p| p.created == created && stem(&p.image) == stem(name)).cloned()
        }
        fn audible(&mut self) -> Option<Vec<u32>> {
            (self.audible.is_some() || self.audible_default).then(|| self.audible.clone().unwrap_or_default())
        }
        fn set_state(&mut self, pid: u32, created: u64, name: &str, state: i64) -> bool {
            self.opened.push(pid);
            let Some(p) = self.processes.get_mut(&pid) else { return false };
            if !same(p, created, name) {
                return false;
            }
            self.set.push((pid, state));
            p.state = state;
            true
        }
    }

    const NEVER: [&str; 34] = [
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
        "VRCX.exe",
        "VRCFaceTracking.exe",
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
        "obs64.exe",
        "ShareX.exe",
        "Discord.exe",
        "voicemeeter8x64.exe",
        "slimevr.exe",
        "XSOverlay.exe",
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

    /// F3: nothing on the allowlist is a general host, a recorder or an agent, nor could ever be one of the refused.
    #[test]
    fn the_list_and_the_refusals_do_not_meet() {
        for name in ALLOWLIST {
            assert!(!never_touch(name), "{name} is on the list and refused");
            assert!(may_lower(name), "{name}");
            assert!(may_lower(&format!("{name}.exe")), "{name}.exe");
            assert!(may_lower(&name.to_uppercase()), "{name} in other case");
        }
        for name in [
            "python.exe",
            "pythonw.exe",
            "node.exe",
            "dotnet.exe",
            "ffmpeg.exe",
            "claude.exe",
            "codex.exe",
            "Docker Desktop.exe",
            "com.docker.backend.exe",
            "java.exe",
        ] {
            assert!(!may_lower(name), "{name}: hosts trackers, OSC or voice tools, records, or runs Orbiters' bridge");
        }
        for name in ["Discord.exe", "obs64.exe", "explorer.exe", "audiodg.exe", "Spotify.exe", "svchost.exe", "chrome2.exe", "notchrome.exe", ""] {
            assert!(!may_lower(name), "{name}: only exact names");
        }
    }

    /// Being refused means never opened, by each route: slowing down, setting back, reading for the journal.
    #[test]
    fn a_refused_process_is_not_even_opened() {
        for (i, name) in NEVER.into_iter().enumerate() {
            let pid = i as u32 + 1;
            let mut os = Table::new();
            os.add(pid, name, SYSTEM_DECIDES);
            let list = os.list();
            assert!(!set_state(&mut os, &list, pid, pid as u64 * 100, name, EFFICIENCY, true), "{name}");
            assert!(!set_state(&mut os, &list, pid, pid as u64 * 100, name, SYSTEM_DECIDES, false), "{name}");
            assert_eq!(read_state(&mut os, &list, pid, pid as u64 * 100, name), Read::Gone, "{name}");
            assert!(os.opened.is_empty() && os.set.is_empty(), "{name}: {:?}", os.opened);
        }
    }

    /// B3: the journal says chrome, but the PID is VRChat's now (Chrome ended, the system gave its PID to VRChat). Recovery
    /// must not open it: the fresh list shows another name and creation time, and that is the end of it.
    #[test]
    fn a_reused_pid_is_never_opened_by_recovery() {
        for now in ["VRChat.exe", "start_protected_game.exe", "EasyAntiCheat_EOS.exe", "notepad.exe", "chrome.exe"] {
            let dir = Scratch::new("reused");
            std::fs::write(
                dir.0.join("journal.json"),
                r#"{"version":1,"entries":[{"target":{"kind":"process","pid":5000,"created":700,"name":"chrome"},"original":{"type":"int","value":0},"written":{"type":"int","value":257},"stage":"applied","at":"","app_version":""}]}"#,
            )
            .unwrap();
            let store = Store::new(&dir.0, Box::new(NotArmed));
            let mut os = Table::new();
            // The process with that PID now: started later, whatever its name.
            os.processes.insert(5000, Opened { image: format!("C:\\Programs\\{now}"), created: 900, state: SYSTEM_DECIDES, same_user: true });
            let got = restore_all(&store, &mut os).unwrap();
            assert_eq!(got.gone.len(), 1, "{now}");
            assert!(os.opened.is_empty() && os.set.is_empty(), "{now}: opened {:?}", os.opened);
            assert!(store.entries().unwrap().is_empty());
        }
    }

    /// B3, the sign-in restorer's path: the same through `system::Offline`.
    #[test]
    fn a_reused_pid_is_never_opened_by_the_sign_in_restorer() {
        let dir = Scratch::new("reused-offline");
        std::fs::write(
            dir.0.join("journal.json"),
            r#"{"version":1,"entries":[{"target":{"kind":"process","pid":5000,"created":700,"name":"chrome"},"original":{"type":"int","value":0},"written":{"type":"int","value":257},"stage":"applied","at":"","app_version":""}]}"#,
        )
        .unwrap();
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::new();
        os.processes.insert(5000, Opened { image: "C:\\Games\\VRChat\\VRChat.exe".into(), created: 900, state: SYSTEM_DECIDES, same_user: true });
        let closed = || true;
        store.restore(&mut crate::system::Offline::new(&mut os, None, &closed), &|_| true, &mut |_| {}).unwrap();
        assert!(os.opened.is_empty(), "{:?}", os.opened);
    }

    /// A scan with all of it in it slows down only what is allowed, and opens nothing else.
    #[test]
    fn lowering_a_whole_system_leaves_everything_else_alone() {
        let dir = Scratch::new("whole");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::new();
        for (i, name) in NEVER.into_iter().enumerate() {
            os.add(i as u32 + 1, name, SYSTEM_DECIDES);
        }
        for (i, name) in ["explorer.exe", "python.exe", "node.exe"].into_iter().enumerate() {
            os.add(100 + i as u32, name, SYSTEM_DECIDES);
        }
        os.add(200, "chrome.exe", SYSTEM_DECIDES);
        assert_eq!(lower(&store, &mut os).unwrap(), 1);
        assert_eq!(os.set, vec![(200, EFFICIENCY)]);
        assert!(os.opened.iter().all(|pid| *pid == 200), "{:?}", os.opened);
    }

    /// F6: a journal whose original says "efficiency mode on" can't make a restore turn it on.
    #[test]
    fn a_restore_only_ever_takes_efficiency_mode_off() {
        let mut os = Table::new();
        os.add(1, "chrome.exe", SYSTEM_DECIDES);
        let list = os.list();
        assert!(!set_state(&mut os, &list, 1, 100, "chrome", EFFICIENCY, false));
        assert!(set_state(&mut os, &list, 1, 100, "chrome", pack(1, 0), false), "explicitly off is allowed");
        assert!(set_state(&mut os, &list, 1, 100, "chrome", SYSTEM_DECIDES, false));
        assert!(os.set.iter().all(|(_, s)| s & 1 == 0));
    }

    /// What was changed comes back; one its owner changed since stays theirs; a PID that is another process now is left; one
    /// that ended is forgotten.
    #[test]
    fn programs_come_back_only_if_they_are_still_ours() {
        let dir = Scratch::new("back");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::new();
        for (pid, name) in [(1, "chrome.exe"), (2, "code.exe"), (3, "cargo.exe"), (4, "rustc.exe")] {
            os.add(pid, name, SYSTEM_DECIDES);
        }
        assert_eq!(lower(&store, &mut os).unwrap(), 4);
        // Taken out of efficiency mode by its owner, or by the person in Task Manager.
        os.processes.get_mut(&2).unwrap().state = pack(1, 0);
        // PID reuse: same number, another program, started later.
        os.processes.insert(3, Opened { image: "C:\\Programs\\cargo.exe".into(), created: 99_999, state: EFFICIENCY, same_user: true });
        os.processes.remove(&4);
        let got = restore_all(&store, &mut os).unwrap();
        assert_eq!(os.processes[&1].state, SYSTEM_DECIDES, "still ours: back");
        assert_eq!(os.processes[&2].state, pack(1, 0), "its owner's");
        assert_eq!(os.processes[&3].state, EFFICIENCY, "not the same process");
        assert_eq!((got.back.len(), got.kept.len(), got.gone.len()), (1, 1, 2));
        assert!(store.entries().unwrap().is_empty());
    }

    /// Already set by someone, or another user's: not touched at all.
    #[test]
    fn only_programs_left_to_the_system_and_of_this_user_are_slowed() {
        let dir = Scratch::new("normal");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::new();
        os.add(1, "chrome.exe", pack(1, 0));
        os.add(2, "chrome.exe", SYSTEM_DECIDES);
        os.processes.get_mut(&2).unwrap().same_user = false;
        assert_eq!(lower(&store, &mut os).unwrap(), 0);
        assert!(os.set.is_empty());
    }

    #[test]
    fn processor_use_is_counted_by_name_and_in_cores() {
        let at = |pid, name: &str, seconds: f32| Seen { pid, name: name.into(), created: 1, cpu: (seconds * 10_000_000.0) as u64 };
        let before = [at(0, "Idle", 0.0), at(1, "chrome.exe", 1.0), at(2, "chrome.exe", 1.0), at(3, "Discord.exe", 0.0), at(4, "VRChat.exe", 10.0)];
        let after = [
            at(0, "Idle", 100.0),
            at(1, "chrome.exe", 6.0),
            at(2, "chrome.exe", 6.0),
            at(3, "Discord.exe", 1.0),
            at(4, "VRChat.exe", 20.0),
            at(5, "cargo.exe", 7.0),
        ];
        let got = cpu_use(&before, &after, 10.0, 8.0, &Default::default());
        assert!((got.listed_cores - 1.0).abs() < 0.01, "only the listed, and a new one counts from now: {got:?}");
        assert_eq!(got.top[0].0, "chrome");
        assert_eq!(got.top[1].0, "vrchat");
        assert!((got.total - (1.0 + 1.0 + 0.1) / 8.0).abs() < 0.01, "{got:?}");
    }

    /// The real system's processes, limited to those a test started: a test never lists the whole system to change it.
    #[cfg(windows)]
    pub(crate) struct Only(pub Vec<u32>);

    #[cfg(windows)]
    impl ProcessOs for Only {
        fn list(&mut self) -> Vec<Seen> {
            scan().into_iter().filter(|p| self.0.contains(&p.pid)).collect()
        }
        fn open(&mut self, pid: u32, created: u64, name: &str) -> Option<Opened> {
            assert!(self.0.contains(&pid), "a test opened {pid}, which it did not start");
            RealOs.open(pid, created, name)
        }
        fn audible(&mut self) -> Option<Vec<u32>> {
            Some(Vec::new())
        }
        fn set_state(&mut self, pid: u32, created: u64, name: &str, state: i64) -> bool {
            assert!(self.0.contains(&pid), "a test changed {pid}, which it did not start");
            RealOs.set_state(pid, created, name, state)
        }
    }

    /// A test's own process's power throttling, read directly, whatever its name: for names the real path refuses.
    #[cfg(windows)]
    fn raw_state(pid: u32) -> i64 {
        use windows::Win32::System::Threading::{
            GetProcessInformation, OpenProcess, ProcessPowerThrottling, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: a process this test started; closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.unwrap();
        let mut state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
        // SAFETY: a valid handle and a buffer of the size passed.
        unsafe {
            GetProcessInformation(h, ProcessPowerThrottling, (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(), std::mem::size_of_val(&state) as u32)
        }
        .unwrap();
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
        pack(state.ControlMask, state.StateMask)
    }

    /// A test's own process's power throttling, read through the real path.
    #[cfg(windows)]
    fn real_state(pid: u32) -> i64 {
        let me = scan().into_iter().find(|p| p.pid == pid).expect("the test's own process is listed");
        RealOs.open(pid, me.created, &me.name).expect("it opens").state
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
        // An Orbiters process, this one included, is refused on its handle too, before anything more is read from it.
        assert_eq!(RealOs.open(me.pid, me.created, &me.name), None);
        // The same creation time a handle gives, or the journal's comparison would never match: on a process of the test's own.
        let dir = Scratch::new("created");
        let sleeper = Sleeper::start(&dir.0, "blender.exe");
        let pid = sleeper.child.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let seen = loop {
            if let Some(seen) = scan().into_iter().find(|p| p.pid == pid) {
                break seen;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let opened = RealOs.open(pid, seen.created, &seen.name).unwrap();
        assert_eq!(opened.created, seen.created);
        assert!(opened.same_user);
        assert_eq!(RealOs.open(pid, seen.created + 1, &seen.name), None, "another creation time: nothing read");
        assert_eq!(RealOs.open(pid, seen.created, "chrome.exe"), None, "another name: nothing read");
    }

    /// A copy of this test program under a given name, for a process of the test's own that can be changed for real and killed.
    /// Nothing but this test sees them, and no name used here is one the app watches for.
    #[cfg(windows)]
    struct Sleeper {
        child: std::process::Child,
        dir: std::path::PathBuf,
    }

    #[cfg(windows)]
    impl Sleeper {
        fn start(dir: &std::path::Path, name: &str) -> Self {
            Self::start_with(dir, name, &[])
        }

        fn start_with(dir: &std::path::Path, name: &str, env: &[(&str, &str)]) -> Self {
            let exe = dir.join(name);
            if !exe.exists() {
                std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
            }
            let child = std::process::Command::new(&exe)
                .args(["--exact", "programs::tests::sleeper_body", "--ignored", "--nocapture"])
                .env("ORBITERS_BOOSTER_SLEEP", dir)
                .envs(env.iter().copied())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            Self { child, dir: dir.to_path_buf() }
        }

        /// Has it start a child of its own (a copy of itself), and gives back the child's PID.
        fn spawn_child(&self) -> u32 {
            std::fs::write(self.dir.join("spawn"), "").unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                if let Some(pid) = std::fs::read_to_string(self.dir.join("child")).ok().and_then(|t| t.trim().parse().ok()) {
                    return pid;
                }
                assert!(std::time::Instant::now() < deadline, "no child");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    #[cfg(windows)]
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(pid) = std::fs::read_to_string(self.dir.join("child")).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }

    /// What a sleeper runs: it waits, and starts a child of its own when asked (once).
    #[cfg(windows)]
    #[test]
    #[ignore = "what `Sleeper` runs"]
    fn sleeper_body() {
        let Some(dir) = std::env::var_os("ORBITERS_BOOSTER_SLEEP").map(std::path::PathBuf::from) else { return };
        // A program with a throttling choice of its own: it ignores timer resolution requests (0x4).
        if std::env::var_os("ORBITERS_BOOSTER_OWN_TIMER").is_some() {
            use windows::Win32::System::Threading::{
                GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE,
            };
            let own = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: 4, StateMask: 4 };
            // SAFETY: this process's own pseudo handle and the structure's size.
            unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&own as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    std::mem::size_of_val(&own) as u32,
                )
            }
            .unwrap();
            std::fs::write(dir.join("own-timer"), "").unwrap();
        }
        // Each copy lives while the test's folder does, two minutes at most: none outlives the test.
        let began = std::time::Instant::now();
        let wanted = || dir.exists() && began.elapsed() < std::time::Duration::from_secs(120);
        if std::env::var_os("ORBITERS_BOOSTER_IS_CHILD").is_some() {
            while wanted() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            return;
        }
        let mut spawned = None;
        while wanted() {
            if spawned.is_none() && dir.join("spawn").exists() && !dir.join("child").exists() {
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "programs::tests::sleeper_body", "--ignored", "--nocapture"])
                    .env("ORBITERS_BOOSTER_SLEEP", &dir)
                    .env("ORBITERS_BOOSTER_IS_CHILD", "1")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                let _ = std::fs::write(dir.join("child"), child.id().to_string());
                spawned = Some(child);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Some(mut child) = spawned {
            let _ = child.wait();
        }
    }

    #[cfg(windows)]
    fn priority_class(pid: u32) -> u32 {
        use windows::Win32::System::Threading::{GetPriorityClass, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: a process this test started; closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.unwrap();
        // SAFETY: a live handle.
        let class = unsafe { GetPriorityClass(h) };
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
        class
    }

    /// The real call on a real process, the test's own child, only ever: slowed, and back exactly.
    #[cfg(windows)]
    #[test]
    fn a_real_process_is_slowed_and_comes_back_exactly() {
        let dir = Scratch::new("real");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let sleeper = Sleeper::start(&dir.0, "blender.exe");
        let mut os = Only(vec![sleeper.child.id()]);
        let pid = sleeper.child.id();
        for _ in 0..200 {
            if !os.list().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(real_state(pid), SYSTEM_DECIDES);
        assert_eq!(lower(&store, &mut os).unwrap(), 1);
        assert_eq!(real_state(pid), EFFICIENCY);
        assert_eq!(restore_all(&store, &mut os).unwrap().back.len(), 1);
        assert_eq!(real_state(pid), SYSTEM_DECIDES);
    }

    /// B2: a program slowed down starts a child, as a browser does for a new tab or an editor for a terminal. The child is
    /// never changed (no inherited priority, no inherited efficiency mode), and after "put everything back" no process the
    /// booster ever had anything to do with is left changed.
    #[cfg(windows)]
    #[test]
    fn a_child_of_a_slowed_program_is_never_changed_and_put_back_leaves_nothing() {
        use windows::Win32::System::Threading::NORMAL_PRIORITY_CLASS;
        let dir = Scratch::new("child");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let sleeper = Sleeper::start(&dir.0, "blender.exe");
        let parent = sleeper.child.id();
        let mut os = Only(vec![parent]);
        for _ in 0..200 {
            if !os.list().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(lower(&store, &mut os).unwrap(), 1);
        let child = sleeper.spawn_child();
        let mut both = Only(vec![parent, child]);
        for _ in 0..200 {
            if both.list().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(priority_class(child), NORMAL_PRIORITY_CLASS.0, "the child is not Below Normal");
        assert_eq!(real_state(child), SYSTEM_DECIDES, "nor in efficiency mode");
        restore_all(&store, &mut both).unwrap();
        for pid in [parent, child] {
            assert_eq!(priority_class(pid), NORMAL_PRIORITY_CLASS.0, "{pid}");
            assert_eq!(real_state(pid), SYSTEM_DECIDES, "{pid}");
        }
        assert!(store.entries().unwrap().is_empty());
    }

    /// The owner of the journal is killed with a real process slowed: the next one reads the journal and puts it back.
    #[cfg(windows)]
    #[test]
    fn killed_with_a_real_process_slowed_the_next_one_puts_it_back() {
        let dir = Scratch::new("real-kill");
        let sleeper = Sleeper::start(&dir.0, "blender.exe");
        let pid = sleeper.child.id();
        let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "programs::tests::lowering_owner_body", "--ignored", "--nocapture"])
            .env("ORBITERS_BOOSTER_OWNER_DIR", &dir.0)
            .env("ORBITERS_BOOSTER_OWNER_PID", pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut os = Only(vec![pid]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !dir.0.join("lowered").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(real_state(pid), EFFICIENCY);
        owner.kill().unwrap();
        let _ = owner.wait();
        assert_eq!(real_state(pid), EFFICIENCY, "this gap is real until the next start: the owner's death alone restores nothing");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        assert_eq!(restore_all(&store, &mut os).unwrap().back.len(), 1);
        assert_eq!(real_state(pid), SYSTEM_DECIDES);
        assert!(store.entries().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "what the kill test runs, in a process of its own"]
    fn lowering_owner_body() {
        let Some(dir) = std::env::var_os("ORBITERS_BOOSTER_OWNER_DIR").map(std::path::PathBuf::from) else { return };
        let pid: u32 = std::env::var("ORBITERS_BOOSTER_OWNER_PID").unwrap().parse().unwrap();
        let store = Store::new(&dir, Box::new(NotArmed));
        let mut os = Only(vec![pid]);
        for _ in 0..200 {
            if !os.list().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(lower(&store, &mut os).unwrap(), 1);
        std::fs::write(dir.join("lowered"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    /// Real processes with names on the refused side, the test's own, each started and asked of: refused, and left as the system
    /// has them. The names are not VRChat's or SteamVR's own, which the running app would take for the real thing.
    #[cfg(windows)]
    #[test]
    fn real_processes_under_refused_names_stay_as_they_are() {
        let dir = Scratch::new("real-refused");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        for name in ["vrscratch.exe", "EasyAntiCheat_scratch.exe", "orbiters_scratch.exe", "not_on_the_list.exe", "python.exe"] {
            let sleeper = Sleeper::start(&dir.0, name);
            let pid = sleeper.child.id();
            let mut os = Only(vec![pid]);
            for _ in 0..200 {
                if !os.list().is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let list = os.list();
            let me = list[0].clone();
            assert_eq!(lower(&store, &mut os).unwrap(), 0, "{name}");
            assert!(!set_state(&mut os, &list, me.pid, me.created, &me.name, EFFICIENCY, true), "{name}");
            assert_eq!(raw_state(pid), SYSTEM_DECIDES, "{name}");
            assert!(store.entries().unwrap().is_empty(), "{name}");
            drop(sleeper);
            let _ = std::fs::remove_file(dir.0.join("child"));
        }
    }

    /// N3: a program's own throttling choice (it ignores timer resolution requests) is kept while it is slowed, and is what comes
    /// back, bit for bit.
    #[cfg(windows)]
    #[test]
    fn a_programs_own_throttling_bits_come_back_exactly() {
        let dir = Scratch::new("own-timer");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let sleeper = Sleeper::start_with(&dir.0, "blender.exe", &[("ORBITERS_BOOSTER_OWN_TIMER", "1")]);
        let pid = sleeper.child.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !dir.0.join("own-timer").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut os = Only(vec![pid]);
        assert_eq!(real_state(pid), pack(4, 4));
        assert_eq!(lower(&store, &mut os).unwrap(), 1);
        assert_eq!(real_state(pid), pack(5, 5), "efficiency mode added, its own bit kept");
        restore_all(&store, &mut os).unwrap();
        assert_eq!(real_state(pid), pack(4, 4), "exactly what it had");
    }

    /// N2: a browser playing sound, through its audio service, is left whole: its tabs too. One that starts playing once slowed
    /// comes back. If audio can't be enumerated, nothing is slowed and what was comes back.
    #[test]
    fn programs_with_sound_are_left_alone() {
        let dir = Scratch::new("audio");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::new();
        // 10: chrome's browser process, 11: its audio service, 12: a tab. 20: an editor.
        for (pid, name) in [(10, "chrome.exe"), (11, "chrome.exe"), (12, "chrome.exe"), (20, "code.exe")] {
            os.add(pid, name, SYSTEM_DECIDES);
        }
        os.audible = Some(vec![11]);
        let mut slower = Slower::default();
        assert_eq!(slower.slow(&store, &mut os).unwrap(), 1);
        assert_eq!(os.set, vec![(20, EFFICIENCY)], "only the editor");
        // The music stops: chrome may be slowed now. Then the editor plays a sound: it comes back.
        os.audible = Some(vec![]);
        assert_eq!(slower.slow(&store, &mut os).unwrap(), 3);
        os.audible = Some(vec![20]);
        slower.slow(&store, &mut os).unwrap();
        assert_eq!(os.processes[&20].state, SYSTEM_DECIDES, "back once it plays");
        // Can't tell: everything back.
        os.audible = None;
        os.audible_default = false;
        assert_eq!(slower.slow(&store, &mut os).unwrap(), 0);
        assert!(os.processes.values().all(|p| p.state == SYSTEM_DECIDES));
        assert!(store.entries().unwrap().is_empty());
    }

    /// N4 through the loop as it runs: slowed, taken out of efficiency mode by hand (or put back to the system's choice), and the
    /// next passes, ten seconds apart in real use, leave it as the person put it, until the step changes.
    #[test]
    fn a_choice_made_by_hand_stays_until_the_step_changes() {
        for by_hand in [SYSTEM_DECIDES, pack(1, 0)] {
            let dir = Scratch::new("respected");
            let store = Store::new(&dir.0, Box::new(NotArmed));
            let mut os = Table::new();
            os.add(1, "chrome.exe", SYSTEM_DECIDES);
            let mut slower = Slower::default();
            assert_eq!(slower.slow(&store, &mut os).unwrap(), 1);
            os.processes.get_mut(&1).unwrap().state = by_hand;
            for _ in 0..3 {
                assert_eq!(slower.slow(&store, &mut os).unwrap(), 0);
                assert_eq!(os.processes[&1].state, by_hand, "{by_hand:#x}: the person's");
            }
            assert!(store.entries().unwrap().is_empty());
            slower.forget();
            if by_hand == SYSTEM_DECIDES {
                assert_eq!(slower.slow(&store, &mut os).unwrap(), 1, "a new step may again");
            }
        }
    }

    /// N8: entries of programs that ended go at the next pass, and so does what they counted.
    #[test]
    fn ended_programs_leave_the_journal() {
        let dir = Scratch::new("ended");
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let mut os = Table::new();
        for pid in 1..=5 {
            os.add(pid, "rustc.exe", SYSTEM_DECIDES);
        }
        let mut slower = Slower::default();
        slower.slow(&store, &mut os).unwrap();
        assert_eq!(store.entries().unwrap().len(), 5);
        for pid in 1..=4 {
            os.processes.remove(&pid);
        }
        slower.slow(&store, &mut os).unwrap();
        assert_eq!(store.entries().unwrap().len(), 1);
    }

    /// Reads which processes have sound now, opening none; changes nothing.
    #[cfg(windows)]
    #[test]
    fn the_real_audio_sessions_can_be_listed() {
        assert!(audio_pids().is_some());
    }
}
