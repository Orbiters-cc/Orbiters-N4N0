//! CPU priority of the worker. Training needs little CPU (about a millisecond a step to keep the GPU busy) but needs
//! it on time: while another program keeps every core busy (an IDE indexing, a compile), a below-normal process gets
//! no CPU at all, and neither do the GPU driver's threads inside it, so the GPU idles. Measured on the RTX 4080 with
//! every core spinning: 1500 Canine steps stalled at 700–1050 in four minutes below normal (even with a time-critical
//! training thread), took 19.9 s in the normal class, 9.2 s in the normal class with a time-critical training thread,
//! and 7.3 s in the above-normal class (7 s with the cores free). Loading starves the same way below normal.
//!
//! The app starts the worker in the normal class. [`govern`] then watches for VRChat (every 3 s): while VRChat is
//! closed the worker runs above normal; while it runs, in the normal class, never above it, and loading (decoding,
//! depth validation, the aberration undo, seeding: [`loading_threads`], [`on_loading_threads`]) runs on at most
//! [`VRCHAT_LOADING_THREADS`] threads. The thread that feeds the GPU runs at time-critical priority while it trains
//! ([`Feeding`]) and VRChat is closed, at the normal level while VRChat runs (a time-critical thread would get ahead of
//! VRChat's own), and at the normal level for long CPU work between steps ([`CpuWork`]: relocation, evaluation). On
//! other systems this does nothing: their schedulers share by weight and never starve a process.

/// Most keyframe-decoding threads while VRChat runs.
pub const VRCHAT_LOADING_THREADS: usize = 4;

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority, ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_CREATION_FLAGS, THREAD_PRIORITY};

    static VRCHAT: OnceLock<AtomicBool> = OnceLock::new();

    fn vrchat_running(system: &mut sysinfo::System) -> bool {
        // Names only: the default refresh also reads every process's command line and environment.
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
        system.processes().values().any(|p| p.name().eq_ignore_ascii_case("vrchat.exe"))
    }

    fn set_class(class: PROCESS_CREATION_FLAGS) {
        // Best effort: a process that cannot change its priority runs as before.
        unsafe {
            let _ = SetPriorityClass(GetCurrentProcess(), class);
        }
    }

    fn class_for(vrchat: bool) -> PROCESS_CREATION_FLAGS {
        if vrchat {
            NORMAL_PRIORITY_CLASS
        } else {
            ABOVE_NORMAL_PRIORITY_CLASS
        }
    }

    pub fn govern() {
        VRCHAT.get_or_init(|| {
            // Normal first: looking for VRChat below normal can itself take seconds on a busy machine.
            set_class(NORMAL_PRIORITY_CLASS);
            let mut system = sysinfo::System::new();
            let running = vrchat_running(&mut system);
            set_class(class_for(running));
            let _ = std::thread::Builder::new().name("priority".into()).spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let running = vrchat_running(&mut system);
                if let Some(flag) = VRCHAT.get() {
                    if flag.swap(running, Ordering::SeqCst) != running {
                        set_class(class_for(running));
                    }
                }
            });
            AtomicBool::new(running)
        });
    }

    pub fn vrchat() -> bool {
        VRCHAT.get().is_some_and(|f| f.load(Ordering::SeqCst))
    }

    thread_local! {
        /// The level this thread was last set to (a change costs a system call; following VRChat checks every step).
        static LEVEL: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
    }

    pub fn set_thread(priority: THREAD_PRIORITY) {
        if LEVEL.get() == Some(priority.0) {
            return;
        }
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), priority);
        }
        LEVEL.set(Some(priority.0));
    }
}

/// The level the feeding thread runs at now: time-critical while VRChat is closed, normal while it runs.
#[cfg(windows)]
fn feeding_level() -> windows::Win32::System::Threading::THREAD_PRIORITY {
    use windows::Win32::System::Threading::{THREAD_PRIORITY_NORMAL, THREAD_PRIORITY_TIME_CRITICAL};
    if imp::vrchat() {
        THREAD_PRIORITY_NORMAL
    } else {
        THREAD_PRIORITY_TIME_CRITICAL
    }
}

thread_local! {
    /// Whether this thread feeds the GPU now ([`Feeding`]).
    static FEEDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Lifts the worker out of the app's below-normal class and adjusts it to VRChat from then on (see the module docs).
/// Idempotent.
pub fn govern() {
    #[cfg(windows)]
    imp::govern();
}

/// Whether VRChat runs, as [`govern`] last looked (false before it ran, and on other systems).
pub fn vrchat() -> bool {
    #[cfg(windows)]
    return imp::vrchat();
    #[cfg(not(windows))]
    false
}

/// Threads to decode keyframes on: every core (up to 16), but few while VRChat runs.
pub fn loading_threads() -> usize {
    let all = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    #[cfg(windows)]
    if imp::vrchat() {
        return all.min(VRCHAT_LOADING_THREADS);
    }
    all
}

/// Runs `f`, and the parallel work it starts (rayon), on [`loading_threads`] threads rather than every core.
pub fn on_loading_threads<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    match rayon::ThreadPoolBuilder::new().num_threads(loading_threads()).thread_name(|i| format!("loading {i}")).build() {
        Ok(pool) => pool.install(f),
        Err(_) => f(),
    }
}

/// While alive, the calling thread feeds the GPU: at time-critical priority while VRChat is closed, at the normal
/// level while it runs ([`Feeding::follow`] keeps up with it starting and closing).
pub struct Feeding(());

impl Feeding {
    pub fn start() -> Self {
        FEEDING.set(true);
        let f = Self(());
        f.follow();
        f
    }

    /// Follows VRChat starting or closing: cheap (a flag read; the priority changes only when VRChat did).
    pub fn follow(&self) {
        #[cfg(windows)]
        imp::set_thread(feeding_level());
    }
}

impl Drop for Feeding {
    fn drop(&mut self) {
        FEEDING.set(false);
        #[cfg(windows)]
        imp::set_thread(windows::Win32::System::Threading::THREAD_PRIORITY_NORMAL);
    }
}

/// While alive, the calling thread runs at the normal level (CPU work between GPU steps), then feeds the GPU again.
pub struct CpuWork(());

impl CpuWork {
    pub fn start() -> Self {
        #[cfg(windows)]
        imp::set_thread(windows::Win32::System::Threading::THREAD_PRIORITY_NORMAL);
        Self(())
    }
}

impl Drop for CpuWork {
    fn drop(&mut self) {
        #[cfg(windows)]
        if FEEDING.get() {
            imp::set_thread(feeding_level());
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, GetPriorityClass, GetThreadPriority, ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, THREAD_PRIORITY_NORMAL, THREAD_PRIORITY_TIME_CRITICAL};

    #[test]
    fn the_worker_follows_vrchat_and_its_training_thread_feeds_at_time_critical() {
        let thread = || unsafe { GetThreadPriority(GetCurrentThread()) };
        govern();
        let class = unsafe { GetPriorityClass(GetCurrentProcess()) };
        let expected = if imp::vrchat() { NORMAL_PRIORITY_CLASS.0 } else { ABOVE_NORMAL_PRIORITY_CLASS.0 };
        assert_eq!(class, expected);
        assert_eq!(thread(), THREAD_PRIORITY_NORMAL.0);
        // Time-critical only while VRChat is closed (this test may run while it plays).
        let feeding = if imp::vrchat() { THREAD_PRIORITY_NORMAL.0 } else { THREAD_PRIORITY_TIME_CRITICAL.0 };
        {
            let f = Feeding::start();
            assert_eq!(thread(), feeding);
            f.follow();
            assert_eq!(thread(), feeding);
            {
                let _cpu = CpuWork::start();
                assert_eq!(thread(), THREAD_PRIORITY_NORMAL.0);
            }
            assert_eq!(thread(), feeding);
        }
        assert_eq!(thread(), THREAD_PRIORITY_NORMAL.0);
        // CPU work outside a feeding stretch leaves the thread at the normal level.
        drop(CpuWork::start());
        assert_eq!(thread(), THREAD_PRIORITY_NORMAL.0);
    }
}
