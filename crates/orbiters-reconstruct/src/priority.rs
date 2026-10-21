//! CPU priority of the worker. Training needs little CPU (about a millisecond a step to keep the GPU busy) but needs
//! it on time: while another program keeps every core busy (an IDE indexing, a compile), a below-normal process gets
//! no CPU at all, and neither do the GPU driver's threads inside it, so the GPU idles. Measured on the RTX 4080 with
//! every core spinning: 1500 Canine steps stalled at 700–1050 in four minutes below normal (even with a time-critical
//! training thread), took 19.9 s in the normal class, 9.2 s in the normal class with a time-critical training thread,
//! and 7.3 s in the above-normal class (7 s with the cores free). Loading starves the same way below normal.
//!
//! The app starts the worker below normal so it never gets ahead of VRChat. [`govern`] lifts it as soon as it runs
//! and then watches for VRChat (every 3 s): while VRChat is closed the worker runs above normal; while it runs, in the
//! normal class, never above it, and loading decodes on at most [`VRCHAT_LOADING_THREADS`] threads. The thread that
//! feeds the GPU runs at time-critical priority while it trains ([`Feeding`]) and at the normal level for long CPU work
//! between steps ([`CpuWork`]: relocation, evaluation). On other systems this does nothing: their schedulers share by
//! weight and never starve a process.

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

    pub fn set_thread(priority: THREAD_PRIORITY) {
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), priority);
        }
    }
}

/// Lifts the worker out of the app's below-normal class and adjusts it to VRChat from then on (see the module docs).
/// Idempotent.
pub fn govern() {
    #[cfg(windows)]
    imp::govern();
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

/// While alive, the calling thread feeds the GPU at time-critical priority.
pub struct Feeding(());

impl Feeding {
    pub fn start() -> Self {
        #[cfg(windows)]
        imp::set_thread(windows::Win32::System::Threading::THREAD_PRIORITY_TIME_CRITICAL);
        Self(())
    }
}

impl Drop for Feeding {
    fn drop(&mut self) {
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
        imp::set_thread(windows::Win32::System::Threading::THREAD_PRIORITY_TIME_CRITICAL);
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
        {
            let _feeding = Feeding::start();
            assert_eq!(thread(), THREAD_PRIORITY_TIME_CRITICAL.0);
            {
                let _cpu = CpuWork::start();
                assert_eq!(thread(), THREAD_PRIORITY_NORMAL.0);
            }
            assert_eq!(thread(), THREAD_PRIORITY_TIME_CRITICAL.0);
        }
        assert_eq!(thread(), THREAD_PRIORITY_NORMAL.0);
    }
}
