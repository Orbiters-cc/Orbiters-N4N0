//! The service's side of the performance self-check (`crate::perf`): a sample every ten seconds while VRChat or SteamVR
//! runs (every minute otherwise), the hint said once in the window, and the Performance report made and sent on request.

use super::{Controller, Shared};
use crate::perf::{self, Command, ReportView};
use super::NoticeKind;
use std::time::{Duration, Instant};

impl Controller {
    pub(super) fn start_perf(self: &Shared) {
        let ctl = std::sync::Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("perf".into()).spawn(move || {
            let mut next = Instant::now();
            // Windows' GPU counters, opened while the usage statistics are on (`usage::perf`).
            let mut gpu: Option<perf::gpu::Gpu> = None;
            loop {
                std::thread::sleep(next.saturating_duration_since(Instant::now()).max(Duration::from_millis(100)));
                let Some(ctl) = ctl.upgrade() else { return };
                if ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let snap = ctl.view();
                let mode = match (snap.vrchat.running, snap.steamvr.running) {
                    (true, true) => "vr",
                    (true, false) => "desktop",
                    (false, true) => "steamvr",
                    (false, false) => "idle",
                };
                next = Instant::now() + if mode == "idle" { perf::QUIET_WINDOW } else { perf::WINDOW };
                let began = Instant::now();
                let (line, lines) = {
                    let mut keeper = perf::keeper().lock().unwrap_or_else(|e| e.into_inner());
                    match keeper.sample(Instant::now(), mode, perf::features(&snap)) {
                        Some(line) => (Some(line), Some(keeper.lines())),
                        None => (None, None),
                    }
                };
                usage_sample(&snap, line.as_ref(), &mut gpu, began);
                let Some(lines) = lines else { continue };
                ctl.perf_follow(&lines);
            }
        });
    }

    /// The hint of the last minute: in the window's state, and said once as a notice (unless muted).
    fn perf_follow(self: &Shared, lines: &[perf::Line]) {
        let muted = perf::hints_muted();
        let hint = perf::hint(lines).filter(|h| !muted.contains(&h.key));
        if let Some(h) = &hint {
            let tell = perf::keeper().lock().unwrap_or_else(|e| e.into_inner()).to_tell(h, &muted);
            if tell {
                let (title, advice) = h.words();
                crate::overlay::vr_log::line(&format!("performance self-check: {} ({:?})", h.key, h.part)); // i18n: not shown (a log's line)
                self.notify(NoticeKind::Info, format!("{title}. {advice}"));
            }
        }
        self.update_if(|s| {
            if s.perf.hint == hint {
                return false;
            }
            s.perf.hint = hint.clone();
            true
        });
    }

    /// What the window asks: a report made, sent, put away, or a hint muted.
    pub fn perf(self: &Shared, command: Command) {
        match command {
            Command::Make => {
                let already = self.view().perf.report.as_ref().is_some_and(|r| r.making || r.sending);
                if already {
                    return;
                }
                self.set_perf_report(Some(ReportView { making: true, ..Default::default() }));
                let ctl = self.clone();
                let _ = std::thread::Builder::new().name("perf-report".into()).spawn(move || {
                    let lines = perf::keeper().lock().unwrap_or_else(|e| e.into_inner()).lines();
                    let report = perf::report::make(&lines, &ctl.view());
                    let summary = perf::report::summary(&report);
                    let (path, error) = match perf::report::save(&report) {
                        Ok(p) => (Some(p), None),
                        Err(e) => (None, Some(e.to_string())),
                    };
                    *ctl.perf_last.lock().unwrap_or_else(|e| e.into_inner()) = Some(report);
                    ctl.set_perf_report(Some(ReportView { making: false, path, summary, sending: false, sent: None, error }));
                });
            }
            Command::Send => {
                let Some(report) = self.perf_last.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
                let Some(view) = self.view().perf.report.clone().filter(|r| !r.making && !r.sending) else { return };
                self.set_perf_report(Some(ReportView { sending: true, sent: None, error: None, ..view.clone() }));
                let ctl = self.clone();
                let _ = std::thread::Builder::new().name("perf-send".into()).spawn(move || {
                    let sent = perf::report::send(&report);
                    if let Err(e) = &sent {
                        eprintln!("performance report: {e}"); // i18n: not shown (a log's line)
                    }
                    ctl.set_perf_report(Some(ReportView { sending: false, sent: Some(sent.is_ok()), error: sent.err(), ..view }));
                });
            }
            Command::Mute(key) => {
                perf::mute_hint(&key);
                self.update_if(|s| {
                    let had = s.perf.hint.as_ref().is_some_and(|h| h.key == key);
                    if had {
                        s.perf.hint = None;
                    }
                    had
                });
            }
            Command::Close => self.set_perf_report(None),
        }
    }

    fn set_perf_report(&self, report: Option<ReportView>) {
        self.update_if(|s| {
            if s.perf.report == report {
                return false;
            }
            s.perf.report = report;
            true
        });
    }
}

/// The beta's performance figures (`crate::usage::perf`): the self-check's sample with the GPU's counters and what N4N0 was doing, into
/// the usage statistics' spool, while they are on. `began` is when the self-check's own sample started: the time to here is what the
/// figures cost, the self-check's system call included.
fn usage_sample(snap: &crate::state::Snapshot, line: Option<&perf::Line>, gpu: &mut Option<perf::gpu::Gpu>, began: Instant) {
    use crate::usage::perf as up;
    let Some(spool) = crate::usage::spool::current() else { return };
    let enabled = snap.settings.usage.enabled;
    spool.origin(up::agent_driving(snap), up::team_code(snap));
    if !enabled {
        *gpu = None;
        spool.perf(Default::default(), false);
        return;
    }
    if gpu.is_none() {
        *gpu = perf::gpu::Gpu::open();
    }
    let now = gpu.as_mut().and_then(perf::gpu::Gpu::sample);
    let Some(line) = line else { return };
    let (sessions, screens) = spool.context();
    let active = up::active(snap, line, &screens);
    let context = up::Context { mode: line.mode.clone(), screens, sessions, active };
    let sample = up::sample(line, now.as_ref(), context, crate::usage::now_ms(), began.elapsed());
    spool.perf(sample, true);
}
