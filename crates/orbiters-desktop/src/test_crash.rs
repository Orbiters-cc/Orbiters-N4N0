//! The test binary only: when it dies of an access violation (or another hardware fault), this says where.
//!
//! On 2026-11-22 the desktop tests ended twice with nothing but `exit code: 0xc0000005, STATUS_ACCESS_VIOLATION`: libtest
//! prints a test's line once it is over, so the one at fault and its place were lost. No debugger is installed on this
//! computer, so the binary watches itself:
//!
//! - every first fault of a thread (a vectored handler, before any handler of the code that faulted) is written to stderr
//!   in one line: code, address, module and offset, the address read or written, the thread. Nothing is allocated there,
//!   because the fault may be inside the allocator. Some libraries fault on purpose and handle it; their lines are
//!   harmless and the run goes on.
//! - the fault that nobody handles (the top-level filter, just before Windows ends the process) gets the whole story:
//!   the thread's name (libtest names each test's thread after its test), the backtrace with symbols, and a scan of the
//!   stack for addresses inside loaded modules, written to stderr and to a file: `ORBITERS_TEST_CRASH_LOG` when set,
//!   else `orbiters-desktop-test-crash-<pid>.log` in the temp folder.
//!
//! - a fault in a DLL that was unloaded since (its code gone, so "no module") is named from a record of every DLL loaded
//!   and unloaded since the start (the loader's notifications), which the whole story also lists.
//!
//! It is installed before `main` by the C runtime's initialiser table, so it covers every test without each one asking.
//! Nothing here is in the program people run.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
    parameters: u32,
    information: [usize; 15],
}

#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}

type Handler = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: Handler) -> *mut c_void;
    fn SetUnhandledExceptionFilter(filter: Option<Handler>) -> Option<Handler>;
    fn GetModuleHandleExW(flags: u32, name: *const c_void, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(file: *mut c_void, buffer: *const u8, length: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
}

const CONTINUE_SEARCH: i32 = 0;
const FROM_ADDRESS: u32 = 0x4;
const UNCHANGED_REFCOUNT: u32 = 0x2;
const STD_ERROR: u32 = -12i32 as u32;

/// The faults worth a line: access violation, in-page error, illegal instruction, privileged instruction, heap corruption.
fn is_fault(code: u32) -> bool {
    matches!(code, 0xC000_0005 | 0xC000_0006 | 0xC000_001D | 0xC000_0096 | 0xC000_0374)
}

#[used]
#[link_section = ".CRT$XCU"]
static INSTALL: extern "C" fn() = install;

static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
static FIRST_CHANCE_LINES: AtomicUsize = AtomicUsize::new(0);
static FATAL: AtomicUsize = AtomicUsize::new(0);

extern "C" fn install() {
    // SAFETY: each takes a function that lives as long as the process; the previous filter is kept to be called after ours.
    unsafe {
        let mut cookie = std::ptr::null_mut();
        LdrRegisterDllNotification(0, dll_noted, std::ptr::null_mut(), &mut cookie);
        AddVectoredExceptionHandler(1, first_chance);
        let previous = SetUnhandledExceptionFilter(Some(unhandled));
        PREVIOUS.store(previous.map_or(0, |f| f as usize), Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------------------- the DLLs loaded and let go

/// A fault in a DLL that was let go has no module any more: what lay at its address is known only from this record of
/// every DLL loaded and unloaded since the start, kept in a ring without allocating (the loader calls in under its lock).
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct DllNotification {
    flags: u32,
    full_name: *const UnicodeString,
    base_name: *const UnicodeString,
    base: *mut c_void,
    size: u32,
}

#[link(name = "ntdll", kind = "raw-dylib")]
extern "system" {
    fn LdrRegisterDllNotification(
        flags: u32,
        callback: unsafe extern "system" fn(u32, *const DllNotification, *mut c_void),
        context: *mut c_void,
        cookie: *mut *mut c_void,
    ) -> i32;
}

const LOADED: u32 = 1;
const UNLOADED: u32 = 2;
const RING: usize = 512;
const NAME: usize = 48;

#[derive(Clone, Copy)]
struct Dll {
    reason: u32,
    base: usize,
    size: usize,
    name: [u16; NAME],
    len: usize,
}

impl Dll {
    const NONE: Dll = Dll { reason: 0, base: 0, size: 0, name: [0; NAME], len: 0 };

    fn holds(&self, address: usize) -> bool {
        self.reason != 0 && address >= self.base && address < self.base + self.size
    }

    fn name_into(&self, out: &mut Line) {
        for c in char::decode_utf16(self.name[..self.len].iter().copied()) {
            let _ = out.write_char(c.unwrap_or('?'));
        }
    }
}

struct Ring(std::cell::UnsafeCell<[Dll; RING]>);
// SAFETY: written only by the loader's notifications, which come one at a time under its lock; read when the process dies.
unsafe impl Sync for Ring {}

static DLLS: Ring = Ring(std::cell::UnsafeCell::new([Dll::NONE; RING]));
static DLLS_NOTED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn dll_noted(reason: u32, data: *const DllNotification, _: *mut c_void) {
    if data.is_null() || (reason != LOADED && reason != UNLOADED) {
        return;
    }
    // SAFETY: the loader hands valid data for the call; the ring's slot is written by this callback alone (see `Ring`).
    unsafe {
        let data = &*data;
        let mut entry = Dll { reason, base: data.base as usize, size: data.size as usize, ..Dll::NONE };
        if let Some(name) = data.base_name.as_ref().filter(|n| !n.buffer.is_null()) {
            let n = (name.length as usize / 2).min(NAME);
            entry.name[..n].copy_from_slice(std::slice::from_raw_parts(name.buffer, n));
            entry.len = n;
        }
        let at = DLLS_NOTED.load(Ordering::Acquire);
        (*DLLS.0.get())[at % RING] = entry;
        DLLS_NOTED.store(at + 1, Ordering::Release);
    }
}

/// The DLLs noted, oldest first (the last `RING` of them), one entry copied at a time (a fault's stack may be short).
fn dlls_noted() -> impl Iterator<Item = Dll> {
    let noted = DLLS_NOTED.load(Ordering::Acquire);
    // SAFETY: plain copies of the ring's entries (see `Ring`).
    (noted.saturating_sub(RING)..noted).map(|i| unsafe { std::ptr::read_volatile(DLLS.0.get().cast::<Dll>().add(i % RING)) })
}

/// How many DLL loads and unloads were noted so far: a mark for [`dlls_unloaded_since`].
pub(crate) fn dlls_noted_count() -> usize {
    DLLS_NOTED.load(Ordering::Acquire)
}

/// The names of the DLLs unloaded since `mark` ([`dlls_noted_count`]).
pub(crate) fn dlls_unloaded_since(mark: usize) -> Vec<String> {
    let noted = dlls_noted_count();
    dlls_noted()
        .skip(RING.min(noted).saturating_sub(noted.saturating_sub(mark)))
        .filter(|d| d.reason == UNLOADED)
        .map(|d| String::from_utf16_lossy(&d.name[..d.len]))
        .collect()
}

/// `name.dll+0x1234 (unloaded)` for an address inside a DLL that was let go since, without allocating.
fn unloaded_module_of(address: usize, out: &mut Line) -> bool {
    let Some(dll) = dlls_noted().filter(|d| d.reason == UNLOADED && d.holds(address)).last() else { return false };
    dll.name_into(out);
    let _ = write!(out, "+{:#x} (a DLL unloaded since)", address - dll.base);
    true
}

/// A fixed buffer to format into without allocating.
struct Line {
    bytes: [u8; 1536],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line { bytes: [0; 1536], len: 0 }
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        // Cut on a character's edge so the line stays text.
        let mut take = s.len().min(self.bytes.len() - self.len);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Straight to the process's stderr: libtest's capture of `eprintln!` would keep it in memory that dies with the process.
fn to_stderr(text: &str) {
    // SAFETY: a handle of this process and a buffer that outlives the call.
    unsafe {
        let mut written = 0u32;
        WriteFile(GetStdHandle(STD_ERROR), text.as_ptr(), text.len() as u32, &mut written, std::ptr::null_mut());
    }
}

/// `module.dll+0x1234` for an address inside a loaded module, without allocating.
fn module_of(address: usize, out: &mut Line) -> bool {
    let mut module = std::ptr::null_mut();
    // SAFETY: the address is only compared with the loaded modules' ranges; the name buffer is ours.
    unsafe {
        if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address as *const c_void, &mut module) == 0 {
            return false;
        }
        let mut name = [0u16; 260];
        let n = GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) as usize;
        let name = &name[..n.min(name.len())];
        let file = name.iter().rposition(|&c| c == u16::from(b'\\')).map_or(name, |i| &name[i + 1..]);
        for c in char::decode_utf16(file.iter().copied()) {
            let _ = out.write_char(c.unwrap_or('?'));
        }
        let _ = write!(out, "+{:#x}", address - module as usize);
    }
    true
}

fn describe(record: &ExceptionRecord, out: &mut Line) {
    let _ = write!(out, "code {:#010x} at {:p} (", record.code, record.address);
    if !module_of(record.address as usize, out) && !unloaded_module_of(record.address as usize, out) {
        let _ = out.write_str("no module");
    }
    let _ = out.write_str(")");
    if record.code == 0xC000_0005 && record.parameters >= 2 {
        let what = match record.information[0] {
            0 => "reading",
            1 => "writing",
            8 => "executing",
            _ => "touching",
        };
        let _ = write!(out, " {what} {:#x}", record.information[1]);
        // What lay there: a fault reading a freed DLL's tables (an object whose code was unloaded) says so.
        let mut there = Line::new();
        if module_of(record.information[1], &mut there) || unloaded_module_of(record.information[1], &mut there) {
            let _ = write!(out, " ({})", there.as_str());
        }
    }
    // SAFETY: always callable.
    let _ = write!(out, " on thread {}", unsafe { GetCurrentThreadId() });
}

unsafe extern "system" fn first_chance(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: Windows hands a valid record for the time of the call.
    let record = unsafe { &*(*pointers).record };
    if !is_fault(record.code) || FIRST_CHANCE_LINES.fetch_add(1, Ordering::Relaxed) >= 32 {
        return CONTINUE_SEARCH;
    }
    let mut line = Line::new();
    let _ = line.write_str("\n[test-crash] first-chance fault: ");
    describe(record, &mut line);
    let _ = line.write_str("\n");
    to_stderr(line.as_str());
    CONTINUE_SEARCH
}

unsafe extern "system" fn unhandled(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: as above.
    let record = unsafe { &*(*pointers).record };
    if is_fault(record.code) && FATAL.fetch_add(1, Ordering::SeqCst) == 0 {
        let mut line = Line::new();
        let _ = line.write_str("\n[test-crash] UNHANDLED fault, the process ends: ");
        describe(record, &mut line);
        let _ = line.write_str("\n");
        // What needs no allocation first, should the rest not get through.
        to_stderr(line.as_str());
        // SAFETY: the context is valid for the call and the stack scanned is this thread's own, inside its limits.
        let report = whole_story(line.as_str(), unsafe { stack_pointer((*pointers).context) });
        to_stderr(&report);
        let path = std::env::var_os("ORBITERS_TEST_CRASH_LOG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join(format!("orbiters-desktop-test-crash-{}.log", std::process::id())));
        if let Ok(mut file) = std::fs::File::create(&path) {
            let _ = file.write_all(report.as_bytes());
            to_stderr(&format!("[test-crash] written to {}\n", path.display()));
        }
    }
    match PREVIOUS.load(Ordering::SeqCst) {
        0 => CONTINUE_SEARCH,
        // SAFETY: the filter that was there before ours, kept as it was given.
        previous => unsafe { std::mem::transmute::<usize, Handler>(previous)(pointers) },
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn stack_pointer(context: *mut c_void) -> usize {
    // x64 CONTEXT: Rsp at 0x98.
    // SAFETY: the caller's context is a whole x64 CONTEXT.
    unsafe { *(context.cast::<u8>().add(0x98).cast::<usize>()) }
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn stack_pointer(_: *mut c_void) -> usize {
    0
}

fn whole_story(summary: &str, rsp: usize) -> String {
    let mut out = String::new();
    let thread = std::thread::current();
    let _ = writeln!(out, "{}", summary.trim());
    let _ = writeln!(out, "[test-crash] thread name: {:?} (libtest names a test's thread after the test)", thread.name());
    let _ = writeln!(out, "[test-crash] backtrace:\n{}", std::backtrace::Backtrace::force_capture());
    let (mut low, mut high) = (0usize, 0usize);
    // SAFETY: writes the two limits of this thread's stack.
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    if rsp >= low && rsp < high {
        let _ = writeln!(out, "[test-crash] addresses in modules on the faulting stack (from rsp {rsp:#x} up):");
        let mut shown = 0;
        let mut at = rsp & !7;
        while at + 8 <= high && shown < 160 && at < rsp + 64 * 1024 {
            // SAFETY: inside this thread's committed stack, above its pointer at the fault.
            let value = unsafe { std::ptr::read_volatile(at as *const usize) };
            let mut place = Line::new();
            if value > 0x10000 && (module_of(value, &mut place) || unloaded_module_of(value, &mut place)) {
                let _ = writeln!(out, "  [rsp+{:#06x}] {value:#x} {}", at - rsp, place.as_str());
                shown += 1;
            }
            at += 8;
        }
    }
    let _ = writeln!(out, "[test-crash] DLLs loaded and unloaded since the start, oldest first:");
    for dll in dlls_noted() {
        let mut name = Line::new();
        dll.name_into(&mut name);
        let what = if dll.reason == LOADED { "loaded  " } else { "UNLOADED" };
        let _ = writeln!(out, "  {what} {:#x}..{:#x} {}", dll.base, dll.base + dll.size, name.as_str());
    }
    out
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    /// Not run by itself: the fault the test below has a child process make.
    #[test]
    #[ignore = "faults on purpose; run by a_fault_names_its_test_and_its_place in a child process"]
    fn faults_on_purpose() {
        if std::env::var_os("ORBITERS_TEST_CRASH_LOG").is_none() {
            return;
        }
        // SAFETY: none: this reads address 8 to make the access violation this module is to report.
        let _ = unsafe { std::ptr::read_volatile(std::hint::black_box(8usize) as *const u8) };
    }

    /// An address in a DLL that was loaded and let go is named after that DLL.
    #[test]
    fn an_address_in_an_unloaded_dll_is_named_after_it() {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryW(name: *const u16) -> *mut std::ffi::c_void;
            fn FreeLibrary(module: *mut std::ffi::c_void) -> i32;
            fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
        }
        let name: Vec<u16> = "msimg32.dll\0".encode_utf16().collect();
        // SAFETY: a system DLL of no use here, loaded and let go; nothing of it is called.
        let base = unsafe {
            if !GetModuleHandleW(name.as_ptr()).is_null() {
                return; // Already in the process for something else: it would not be unloaded.
            }
            let module = LoadLibraryW(name.as_ptr());
            assert!(!module.is_null());
            FreeLibrary(module);
            if !GetModuleHandleW(name.as_ptr()).is_null() {
                return;
            }
            module as usize
        };
        let mut line = super::Line::new();
        assert!(super::unloaded_module_of(base + 0x10, &mut line));
        assert_eq!(line.as_str().to_ascii_lowercase(), "msimg32.dll+0x10 (a dll unloaded since)");
    }

    /// A child test that reads address 8 dies with 0xc0000005, and what it leaves names the test, the reading and the function.
    #[test]
    fn a_fault_names_its_test_and_its_place() {
        let dir = std::env::temp_dir().join(format!("orbiters-test-crash-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("crash.log");
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "test_crash::tests::faults_on_purpose", "--test-threads=1"])
            .env("ORBITERS_TEST_CRASH_LOG", &log)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code().map(|c| c as u32), Some(0xC000_0005), "{stderr}");
        assert!(stderr.contains("[test-crash] first-chance fault: code 0xc0000005"), "{stderr}");
        let report = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(report.contains("reading 0x8"), "{report}");
        assert!(report.contains("Some(\"test_crash::tests::faults_on_purpose\")"), "{report}");
        assert!(report.contains("faults_on_purpose"), "{report}");
        assert!(report.contains("addresses in modules on the faulting stack"), "{report}");
    }
}
