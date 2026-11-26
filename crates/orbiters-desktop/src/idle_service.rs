//! `orbiters-desktop --idle-service [--seconds <s>] [--settle <s>]` counts what wakes the tray service while nothing
//! happens: no VRChat, no SteamVR, nobody signed in, no window (review 1124, XCT-23). It runs the service's own threads
//! (`Controller::start_idle_service`, the bridge's keeper, `Service`'s keepers) over private folders, and after `--settle` seconds reads
//! how often each thread of this process was switched to over `--seconds` (Windows' and Linux' context switch counts).
//! Rescheduling within work also increments that count. The service's loop counters measure returns from waits separately.
//! The report includes CPU time and the number of snapshots published.
//!
//! It uses private paths and ephemeral loopback sockets, skips the fixed world-feedback port, and refuses to run beside VRChat or
//! SteamVR, which are not an idle computer. What it does not have: an account signed in (the event streams and the account's
//! own looks), mDNS (it would raise Windows' firewall prompt), the tray's message loop, and the capture worker's two reading
//! threads when the worker is not beside this program.

use crate::controller::Controller;
use crate::ipc::Endpoint;
use crate::mailbox::Mailbox;
use crate::service::Service;
use orbiters_core::settings::Settings;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::time::Duration;

/// A thread of this process: its name, how often it was switched to, its processor time (100 ns units).
struct Thread {
    name: String,
    switches: u64,
    cpu: u64,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let number = |flag: &str, or: u64| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|n| n.parse().ok()).unwrap_or(or);
    let (seconds, settle) = (number("--seconds", 60).max(1), number("--settle", 30));
    // mDNS binds every interface, and Windows' firewall asks the person about each new program that does.
    std::env::set_var("ORBITERS_NO_NETWORK_LISTENING", "1");
    require_idle_host()?;
    let mut settings = Settings::default();
    // Nothing is sent anywhere, whatever was noted.
    settings.usage.enabled = false;
    let ctl = Controller::new(settings);
    ctl.start_idle_service();
    ctl.start_bridge();
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let endpoint = Endpoint { port: listener.local_addr()?.port(), token: uuid::Uuid::new_v4().simple().to_string() };
    let service = Service::around(ctl.clone(), endpoint, std::env::current_exe()?);
    service.accept(listener)?;
    service.keep()?;
    // As the tray's menu follows the capture.
    let mailbox = Mailbox::watching(|s| s.is_capturing());
    ctl.subscribe(mailbox.clone());
    std::thread::Builder::new().name("tray-watch".into()).spawn(move || while mailbox.take().is_some() {})?;

    std::thread::sleep(Duration::from_secs(settle));
    let (before, published, turns_before) = (threads(), ctl.publishes(), ctl.wake_ups());
    std::thread::sleep(Duration::from_secs(seconds));
    let (after, published, turns_after) = (threads(), ctl.publishes() - published, ctl.wake_ups());
    let this = this_thread();
    let a_minute = |n: u64| n as f64 * 60.0 / seconds as f64;
    let mut rows: Vec<(String, u64, u64)> = after
        .iter()
        .filter(|(id, _)| **id != this)
        .map(|(id, now)| {
            let was = before.get(id);
            (now.name.clone(), now.switches - was.map_or(0, |w| w.switches), now.cpu - was.map_or(0, |w| w.cpu))
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (total, cpu) = rows.iter().fold((0, 0), |(n, c), r| (n + r.1, c + r.2));
    println!("{:<28} {:>14} {:>12}", "thread", "switches/min", "cpu ms/min");
    for (name, n, cpu) in &rows {
        println!("{:<28} {:>14.1} {:>12.2}", if name.is_empty() { "(no name)" } else { name }, a_minute(*n), a_minute(*cpu) / 1e4);
    }
    println!("{} threads, {} asleep all along", rows.len(), rows.iter().filter(|r| r.1 == 0).count());
    println!("{:.1} context switches a minute, {:.2} ms of processor, {:.1} publishes", a_minute(total), a_minute(cpu) / 1e4, a_minute(published));
    let turns: Vec<_> = turns_after.into_iter().filter_map(|(name, n)| {
        let n = n.saturating_sub(turns_before.get(name).copied().unwrap_or(0));
        (n > 0).then_some((name, n))
    }).collect();
    println!("{:.1} controller loop turns a minute: {turns:?}", a_minute(turns.iter().map(|(_, n)| n).sum()));
    ctl.shutdown();
    require_idle_host()
}

fn require_idle_host() -> anyhow::Result<()> {
    #[cfg(windows)]
    let processes = orbiters_booster::programs::scan().into_iter().map(|p| p.name);
    #[cfg(target_os = "linux")]
    let processes = orbiters_core::processes::all().into_iter().map(|p| p.name);
    #[cfg(not(any(windows, target_os = "linux")))]
    let processes = std::iter::empty::<String>();
    let names = ["vrchat.exe", "vrchat", "vrserver.exe", "vrserver"];
    anyhow::ensure!(
        !processes.into_iter().any(|p| names.iter().any(|name| p.eq_ignore_ascii_case(name))),
        "VRChat or SteamVR runs: not an idle computer"
    );
    Ok(())
}

#[cfg(windows)]
fn this_thread() -> u32 {
    // SAFETY: a plain call.
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

/// This process' threads by their number, from the one system call Task Manager makes too.
#[cfg(windows)]
fn threads() -> BTreeMap<u32, Thread> {
    use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
    // SYSTEM_PROCESS_INFORMATION's fixed part is 256 bytes on 64-bit Windows; its SYSTEM_THREAD_INFORMATIONs follow, 80 bytes each.
    const PROCESS: usize = 256;
    const THREAD: usize = 80;
    let mut buffer = vec![0u64; 512 * 1024];
    for _ in 0..4 {
        let mut needed = 0u32;
        // SAFETY: the buffer is as large as said, and aligned for the system's own structures.
        let status = unsafe { NtQuerySystemInformation(SystemProcessInformation, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed) };
        if !status.is_ok() {
            buffer = vec![0u64; (needed as usize / 8).max(buffer.len()) + 16 * 1024];
            continue;
        }
        // SAFETY: the same memory as bytes.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
        let u32_at = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_ne_bytes(bytes[at..at + 8].try_into().unwrap());
        let mut at = 0;
        loop {
            if u64_at(at + 80) as u32 == std::process::id() {
                return (0..u32_at(at + 4) as usize)
                    .map(|i| {
                        let t = at + PROCESS + i * THREAD;
                        let id = u64_at(t + 48) as u32;
                        (id, Thread { name: name_of(id), switches: u32_at(t + 64) as u64, cpu: u64_at(t) + u64_at(t + 8) })
                    })
                    .collect();
            }
            let next = u32_at(at) as usize;
            if next == 0 {
                break;
            }
            at += next;
        }
        break;
    }
    BTreeMap::new()
}

#[cfg(windows)]
fn name_of(id: u32) -> String {
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HLOCAL};
    use windows::Win32::System::Threading::{GetThreadDescription, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
    // SAFETY: a handle opened and closed here; the text is the system's, freed as it asks.
    unsafe {
        let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, id) else { return String::new() };
        let name = GetThreadDescription(thread)
            .map(|text| {
                let name = text.to_string().unwrap_or_default();
                let _ = LocalFree(Some(HLOCAL(text.0.cast())));
                name
            })
            .unwrap_or_default();
        let _ = CloseHandle(thread);
        name
    }
}

#[cfg(target_os = "linux")]
fn this_thread() -> u32 {
    std::fs::read_link("/proc/thread-self").ok().and_then(|p| p.file_name()?.to_str()?.parse().ok()).unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn threads() -> BTreeMap<u32, Thread> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir("/proc/self/task").into_iter().flatten().flatten() {
        let Some(id) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else { continue };
        let name = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default().trim().to_string();
        let status = std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
        let switches = status.lines().filter_map(|l| l.strip_prefix("voluntary_ctxt_switches:")).find_map(|n| n.trim().parse().ok()).unwrap_or(0);
        out.insert(id, Thread { name, switches, cpu: 0 });
    }
    out
}

/// Not counted on a Mac (its kernel gives no per-thread count of switches): only the publishes are.
#[cfg(target_os = "macos")]
fn this_thread() -> u32 {
    0
}

#[cfg(target_os = "macos")]
fn threads() -> BTreeMap<u32, Thread> {
    BTreeMap::new()
}
