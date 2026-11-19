//! The tray service: what runs while Orbiters - N4N0 waits in the tray. It owns the controller (capture worker,
//! OSC, VRChat and SteamVR watchers), the tray icon and single-instance handling, and serves the window and the VR
//! overlay over the IPC link. It never creates a GPU device: the window process does, and exits when closed; the
//! overlay process runs while SteamVR does.

use crate::controller::{self, Controller, NoticeKind};
use crate::ipc::{self, Endpoint, N4n0Command, Role, ToClient, ToService};
use crate::mailbox::Mailbox;
use crate::single;
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(windows))]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Link {
    id: u64,
    role: Role,
    pid: u32,
    mailbox: Arc<Mailbox>,
    #[cfg(test)]
    writes: Arc<AtomicU64>,
}

/// Connections that have not said their token yet, at most: each holds a thread for up to `HELLO_TIMEOUT`.
const MAX_UNAUTHENTICATED: usize = 16;
/// The longest line a client may send (settings, a list of picture paths).
const MAX_COMMAND: usize = 4 << 20;
/// The window's link waits this long after a snapshot before the next.
const WINDOW_PACE: Duration = Duration::from_millis(33);

pub struct Service {
    pub ctl: controller::Shared,
    endpoint: Endpoint,
    exe: PathBuf,
    links: Mutex<Vec<Link>>,
    next_link: AtomicU64,
    /// Connections waiting to say their token.
    unauthenticated: AtomicUsize,
    /// The window process we started and when (a second request while it starts does not start another).
    window: Mutex<Option<(Child, Instant)>>,
    /// A page asked for while no window was there: the window starting now gets it once it is linked.
    page_for_window: Mutex<Option<String>>,
    /// The overlay process we started, when, and for which SteamVR (PID and creation time), and consecutive failures.
    overlay: Mutex<Option<(Child, Instant, Option<crate::steamvr_start::Session>, u32)>>,
    /// The process of the panels around VRChat's menus in desktop mode, and when it started.
    orbit: Mutex<Option<(Child, Instant)>>,
    /// The SteamVR (its `vrserver` PID and creation time) whose start of the app was looked at ([`Service::keep_steamvr_start`]).
    steamvr_start_seen: Mutex<Option<crate::steamvr_start::Session>>,
    /// Tracking smoothing's driver, kept in step with the settings ([`Service::keep_tracking`]).
    smoothing: Mutex<crate::tracking::Smoothing>,
    /// Which link holds the app's Talk button down (the window's and the desktop panels' are one Talk to N4N0): only
    /// that link going away lets it go (AD-1).
    talk: Mutex<TalkHolder>,
    quitting: AtomicBool,
    /// The beta's usage statistics, a session per link, waiting for Orbiters (`usage::spool`).
    usage: Arc<crate::usage::spool::Spool>,
    #[cfg(windows)]
    main_thread: u32,
    #[cfg(not(windows))]
    quit_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl Service {
    /// A service around `ctl`, which clients find at `endpoint` and which starts its processes from `exe`: as [`run`]
    /// makes it. `--link-smoke` and the tests make one around a controller that is never started.
    pub(crate) fn around(ctl: controller::Shared, endpoint: Endpoint, exe: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            ctl,
            endpoint,
            exe,
            links: Mutex::new(Vec::new()),
            next_link: AtomicU64::new(1),
            unauthenticated: AtomicUsize::new(0),
            window: Mutex::new(None),
            page_for_window: Mutex::new(None),
            overlay: Mutex::new(None),
            orbit: Mutex::new(None),
            steamvr_start_seen: Mutex::new(None),
            smoothing: Mutex::new(Default::default()),
            talk: Mutex::new(TalkHolder::default()),
            quitting: AtomicBool::new(false),
            usage: Arc::default(),
            #[cfg(windows)]
            main_thread: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
            #[cfg(not(windows))]
            quit_tx: Mutex::new(None),
        })
    }

    /// Serves every client that connects to `listener`, each on a thread of its own, for as long as the process runs.
    pub(crate) fn accept(self: &Arc<Self>, listener: TcpListener) -> std::io::Result<()> {
        let svc = self.clone();
        std::thread::Builder::new()
            .name("ipc".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    // Each one waits up to `HELLO_TIMEOUT` for its token on a thread: a flood of connections that never
                    // say it is turned away rather than given threads.
                    if svc.unauthenticated.fetch_add(1, Ordering::Relaxed) >= MAX_UNAUTHENTICATED {
                        svc.unauthenticated.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                    let s = svc.clone();
                    let spawned = std::thread::Builder::new().name("ipc-client".into()).spawn(move || s.serve(stream));
                    if spawned.is_err() {
                        svc.unauthenticated.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            })
            .map(|_| ())
    }

    /// How many clients of `role` are linked now.
    pub(crate) fn linked(&self, role: Role) -> usize {
        self.links.lock().unwrap().iter().filter(|l| l.role == role).count()
    }

    /// Brings the window to the front, starting a window process when there is none.
    pub fn show_window(&self, splash: bool) {
        if self.quitting.load(Ordering::Relaxed) {
            return;
        }
        if let Some(link) = self.links.lock().unwrap().iter().find(|l| l.role == Role::Window) {
            allow_foreground(link.pid);
            link.mailbox.post(ToClient::Show);
            return;
        }
        let mut window = self.window.lock().unwrap();
        if let Some((child, started)) = window.as_mut() {
            if matches!(child.try_wait(), Ok(None)) && started.elapsed() < Duration::from_secs(15) {
                return; // Still starting.
            }
        }
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--window").args(self.endpoint.args()).stdin(Stdio::null());
        if splash {
            cmd.arg("--splash");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console for debug builds
        }
        match cmd.spawn() {
            Ok(child) => {
                crate::worker::keep_with_service(&child);
                allow_foreground(child.id());
                *window = Some((child, Instant::now()));
            }
            Err(e) => eprintln!("could not start the window: {e}"),
        }
    }

    /// Once per SteamVR session: SteamVR is told a start with it chosen while it was not running, or else what it says
    /// is kept in the settings (its own settings may have changed it): [`crate::startup`]. It is asked only while it runs.
    fn keep_steamvr_start(&self) {
        if self.quitting.load(Ordering::Relaxed) {
            return;
        }
        let snap = self.ctl.view();
        // The snapshot's answers first: asking for the session lists every process of the computer, and it was asked
        // every 2 s with no SteamVR at all, or with the overlay (which does this itself) on: half of the idle
        // service's processor (LIGHT-DESKTOP-5, CTL-3).
        if !steamvr_start_may_be_due(snap.settings.steamvr_overlay, snap.steamvr.running) {
            return;
        }
        let Some(session) = crate::steamvr_start::ready_session() else { return };
        if (*self.steamvr_start_seen.lock().unwrap() == Some(session) && !snap.settings.startup.steamvr_pending)
            || Some(session.0) != snap.steamvr.pid {
            return;
        }
        // Closing an overlay is asynchronous: do not overlap its connection.
        if self.links.lock().unwrap().iter().any(|l| l.role == Role::Overlay)
            || self.overlay.lock().unwrap().as_mut().is_some_and(|(child, _, _, _)| matches!(child.try_wait(), Ok(None))) { return; }
        // Not yet answering (it is starting, or going): asked again next turn.
        let Ok(client) = orbiters_openvr::Client::connect(orbiters_openvr::AppType::Background) else { return };
        if crate::steamvr_start::current_session() != Some(session) { return; }
        *self.steamvr_start_seen.lock().unwrap() = Some(session);
        self.smoothing.lock().unwrap().set_kinds((0..64).map(|i| crate::tracking::DeviceKind::of(client.device_class(i))).collect());
        if let Some(startup) = crate::startup::reconcile(&client, &snap.settings.startup) {
            self.ctl.update_settings(|s| s.startup = startup);
        }
    }

    /// Tracking smoothing as the settings say: the driver registered for SteamVR, on or off, each device's strength;
    /// what it does now goes to N4N0's Tracking page. Run every keeper turn, at once when the settings change
    /// (`follow_tracking`) and when the overlay says which device is which.
    fn keep_tracking(&self) {
        if self.quitting.load(Ordering::Relaxed) {
            return;
        }
        let snap = self.ctl.view();
        let world = crate::tracking::World {
            settings: &snap.settings.tracking,
            steamvr: snap.steamvr.running,
            steamvr_pid: snap.steamvr.pid,
            overlay: snap.steamvr.overlay,
        };
        let note = self.smoothing.lock().unwrap().keep(&world);
        self.ctl.set_tracking_note(note);
    }

    /// Smoothing's settings reach the driver the moment they change (it reads them on SteamVR's next frame), not at
    /// the keeper's next turn. Its mailbox hands it the states whose tracking settings differ from the last it was
    /// handed: it is not woken for anything else the service publishes.
    fn follow_tracking(self: &Arc<Self>) {
        let mailbox = Mailbox::watching(|s| s.settings.tracking.clone());
        self.ctl.subscribe(mailbox.clone());
        let mut seen = None;
        while let Some((_, snapshot)) = mailbox.take() {
            if self.quitting.load(Ordering::Relaxed) {
                return;
            }
            let Some(snapshot) = snapshot else { continue };
            if seen.as_ref() != Some(&snapshot.settings.tracking) {
                let first = seen.is_none();
                seen = Some(snapshot.settings.tracking.clone());
                if !first {
                    self.keep_tracking();
                }
            }
        }
    }

    /// Starts the SteamVR overlay process while SteamVR runs (and the setting allows it); closes it otherwise.
    fn keep_overlay(&self) {
        if self.quitting.load(Ordering::Relaxed) {
            return;
        }
        let snap = self.ctl.view();
        let wanted = snap.steamvr.running && snap.settings.steamvr_overlay;
        let links = self.links.lock().unwrap();
        let connected = links.iter().find(|l| l.role == Role::Overlay);
        if !wanted {
            if let Some(link) = connected {
                if !snap.settings.steamvr_overlay {
                    link.mailbox.post(ToClient::Close);
                }
            }
            return;
        }
        if connected.is_some() {
            if let Some((_, _, _, failures)) = self.overlay.lock().unwrap().as_mut() { *failures = 0; }
            return;
        }
        let Some(session) = crate::steamvr_start::ready_session().filter(|s| Some(s.0) == snap.steamvr.pid) else { return };
        drop(links);
        let mut overlay = self.overlay.lock().unwrap();
        if let Some((child, started, steamvr, failures)) = overlay.as_mut() {
            let exited = match child.try_wait() {
                Ok(None) => None,
                Ok(Some(status)) => Some(status.code()),
                Err(_) => Some(None),
            };
            if !overlay_due(exited, started.elapsed(), *steamvr, Some(session), *failures) {
                return;
            }
            crate::overlay::vr_log::line(&format!("service: the SteamVR overlay ended (exit code {exited:?}) after {} s; starting it again", started.elapsed().as_secs())); // i18n: not shown (a log's line)
        }
        let failures = overlay.as_ref().filter(|(_, _, prior, _)| *prior == Some(session)).map_or(1, |(_, _, _, n)| n.saturating_add(1));
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--overlay").args(self.endpoint.args()).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn() {
            Ok(child) => {
                crate::worker::keep_with_service(&child);
                *overlay = Some((child, Instant::now(), Some(session), failures));
            }
            Err(e) => eprintln!("could not start the SteamVR overlay: {e}"),
        }
    }

    /// Starts the panels around VRChat's menus in desktop mode while VRChat runs without SteamVR, in a process of their own: a Direct3D
    /// device and Windows Graphics Capture have no place in the service, and whatever happens to them the game and the service go on.
    /// It ends by itself when they are no longer wanted.
    fn keep_orbit(&self) {
        if self.quitting.load(Ordering::Relaxed) {
            return;
        }
        self.ctl.follow_worn_kit();
        let snap = self.ctl.view();
        if !crate::overlay::orbit::wanted_on_desktop(&snap) {
            return;
        }
        let mut orbit = self.orbit.lock().unwrap();
        if let Some((child, started)) = orbit.as_mut() {
            let running = matches!(child.try_wait(), Ok(None));
            // One that ended on its own may start again, not at once: whatever made it end must not spin.
            if running || started.elapsed() < Duration::from_secs(10) {
                return;
            }
        }
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--orbit-desktop").args(self.endpoint.args()).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn() {
            Ok(child) => {
                crate::worker::keep_with_service(&child);
                *orbit = Some((child, Instant::now()));
            }
            Err(e) => eprintln!("could not start the panels around VRChat's menus: {e}"),
        }
    }

    /// Closes every window, stops the worker (it finishes a running session) and leaves.
    pub fn quit(&self) {
        if self.quitting.swap(true, Ordering::Relaxed) {
            return;
        }
        for link in self.links.lock().unwrap().iter() {
            link.mailbox.post(ToClient::Close);
        }
        self.ctl.booster_quit();
        self.ctl.shutdown();
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
            let _ = PostThreadMessageW(self.main_thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        #[cfg(not(windows))]
        if let Some(tx) = self.quit_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }

    /// The usage statistics turned off: what waits is forgotten at once, not at the next upload.
    fn follow_usage_switch(&self) {
        if !self.ctl.view().settings.usage.enabled {
            self.usage.forget();
        }
    }

    /// Restarts into an update's setup (controller/update.rs): starts it, then quits; the setup replaces the files
    /// once the app has gone and starts it again, with its window if one is open now, in the tray otherwise.
    fn restart_into(&self, setup: &std::path::Path, dir: &std::path::Path) {
        let window_open = self.links.lock().unwrap().iter().any(|l| l.role == Role::Window);
        match controller::update::start_setup(setup, dir, !window_open) {
            Ok(()) => self.quit(),
            Err(e) => self.ctl.notify(NoticeKind::Error, tr!("The update could not start: {error}", error = e)),
        }
    }

    fn handle(&self, link: u64, role: Role, msg: ToService) {
        let ctl = &self.ctl;
        let report = |r: anyhow::Result<()>| {
            if let Err(e) = r {
                ctl.notify(NoticeKind::Error, e.to_string());
            }
        };
        match msg {
            ToService::Hello { .. } => {}
            ToService::StartCapture { origin } => ctl.request_capture_from(origin),
            ToService::CameraQr { active } => ctl.camera_qr(active),
            ToService::SwitchDemoAvatar => ctl.switch_demo_avatar(),
            ToService::StopCapture => report(ctl.stop_capture()),
            ToService::Pause { paused } => report(ctl.pause_capture(paused)),
            ToService::ChooseVrchat { pid } => report(ctl.choose_vrchat(pid)),
            ToService::TestPattern => report(ctl.start_test_capture()),
            ToService::Scan => report(ctl.take_scan()),
            ToService::ImportRaw { dir } => report(ctl.import_raw_capture(dir)),
            ToService::SaveSettings { settings } => {
                ctl.save_settings(*settings);
                self.follow_usage_switch();
            }
            ToService::PatchSettings { patch } => {
                ctl.patch_settings(&patch);
                self.follow_usage_switch();
            }
            ToService::Usage { steps } => self.usage.steps(link, steps, ctl.view().settings.usage.enabled),
            ToService::PatchSmoothing { id, patch } => {
                ctl.patch_settings(&patch);
                ctl.update(|s| s.smoothing_revision += 1);
                let revision = ctl.view().smoothing_revision;
                if let Some(l) = self.links.lock().unwrap().iter().find(|l| l.id == link) {
                    l.mailbox.post(ToClient::SmoothingAck { id, revision });
                }
            }
            ToService::Trash { dir } => {
                let r = ctl.trash_session(&dir);
                if r.is_ok() {
                    ctl.notify(NoticeKind::Success, tr!("Session moved to the {bin}.", bin = tr(crate::words::BIN)));
                }
                report(r);
            }
            // Deleting runs on a thread of its own; the job shows in the snapshot.
            ToService::CleanUp { trim, delete } => report(ctl.clean_up(trim, delete)),
            ToService::Reconstruct { dir } => report(ctl.reconstruct(dir)),
            ToService::SendToMixer { dir, title } => report(ctl.send_to_mixer(dir, title)),
            ToService::SignIn => {
                // Asking Orbiters for the link is a network call: not on the link's reading thread.
                let c = ctl.clone();
                std::thread::spawn(move || {
                    if let Err(e) = c.sign_in() {
                        c.notify(NoticeKind::Error, tr!("Could not start the sign-in: {error}", error = e));
                    }
                });
            }
            ToService::ReopenSignIn => report(ctl.reopen_sign_in()),
            ToService::CancelSignIn => ctl.cancel_sign_in(),
            ToService::SignOut => report(ctl.sign_out()),
            ToService::VrchatLogin { command } => ctl.vrchat_command(command),
            ToService::Launcher { action } => ctl.launcher_action(action),
            ToService::VrchatData { id, scope, request } => {
                let mailbox = self.links.lock().unwrap().iter().find(|l| l.id == link).map(|l| l.mailbox.clone());
                if let Some(mailbox) = mailbox {
                    let ctl = ctl.clone();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                    std::thread::spawn(move || {
                        let result = ctl.vrchat_data_until(scope, request, deadline);
                        mailbox.post(ToClient::VrchatData { id, result });
                    });
                }
            }
            // Only the window picks picture files; the upload runs on a thread of its own.
            ToService::ShowWindow => self.show_window(false),
            ToService::ShowPage { page } => {
                let linked = self.links.lock().unwrap().iter().find(|l| l.role == Role::Window).map(|l| (l.pid, l.mailbox.clone()));
                match linked {
                    Some((pid, mailbox)) => {
                        allow_foreground(pid);
                        mailbox.post(ToClient::ShowPage { page });
                    }
                    None => {
                        *self.page_for_window.lock().unwrap() = Some(page);
                        self.show_window(false);
                    }
                }
            }
            ToService::Orbit { status } => self.ctl.set_orbit_status(status),
            ToService::TranslateStatus { status } => self.ctl.set_translate_status(status),
            ToService::Translate { id, batch } => {
                let mailbox = self.links.lock().unwrap().iter().find(|l| l.id == link).map(|l| l.mailbox.clone());
                if let Some(mailbox) = mailbox {
                    let ctl = ctl.clone();
                    std::thread::spawn(move || {
                        let result = ctl.translate_remote(&batch);
                        mailbox.post(ToClient::Translated { id, result });
                    });
                }
            }
            ToService::VrchatPicture { scope, id, path } if role == Role::Window => ctl.vrchat_picture(scope, id, path),
            ToService::VrchatPicture { .. } => {}
            ToService::StopReconstruction => {
                ctl.stop_reconstruction();
            }
            ToService::Unqueue { dir } => ctl.unqueue_reconstruction(&dir),
            ToService::WorkerLog => {
                let lines = ctl.worker_log();
                if let Some(l) = self.links.lock().unwrap().iter().find(|l| l.id == link) {
                    l.mailbox.post(ToClient::WorkerLog { lines });
                }
            }
            ToService::UploadPictures { world_id, world_name, paths } => report(ctl.upload_pictures(world_id, world_name, paths)),
            ToService::UploadPreview { world_id, world_name, session } => report(ctl.upload_preview(world_id, world_name, session)),
            ToService::CaptureShare { request } => ctl.capture_share(*request),
            ToService::PrepareShare { session } => ctl.prepare_share(session),
            ToService::Maps => ctl.refresh_maps(),
            ToService::Calibrated { calibration } => ctl.calibrated(*calibration),
            ToService::Frames { chunk } => ctl.frames(chunk),
            ToService::Headset { in_use } => ctl.set_headset(Some(in_use)),
            ToService::SteamVrDevices { kinds } => {
                self.smoothing.lock().unwrap().set_kinds(kinds);
                self.keep_tracking();
            }
            ToService::WalkBoost { strength } => ctl.set_walk_boost(strength, false),
            ToService::BoosterReport { report } => ctl.booster_report(*report),
            ToService::PerfTurns { turns } => crate::perf::keeper().lock().unwrap_or_else(|e| e.into_inner()).turns(*turns),
            ToService::PerfReport { command } => ctl.perf(command),
            ToService::Booster { command } => ctl.booster_command(command),
            ToService::WalkPath { path } => ctl.set_walk_path(crate::tracking::walk_path_said(path)),
            // Which Talk button is held matters: the service lets it go when that window or overlay goes away.
            ToService::N4n0 { command: N4n0Command::Talk { down } } => {
                if role != Role::Overlay {
                    self.talk.lock().unwrap().talk(link, down);
                }
                ctl.n4n0_app_talk(down, role == Role::Overlay)
            }
            ToService::N4n0 { command } => ctl.n4n0_command(command),
            ToService::OscReconnect => ctl.osc_rediscover(),
            ToService::Updates { action } => ctl.update_action(action),
            // Quickly, on this thread: Stop is the person taking control back.
            ToService::Bridge { command } => ctl.bridge_command(command),
            ToService::Quit => self.quit(),
        }
    }

    /// One client: check its token (the accept loop counted it among the unauthenticated), then stream every change
    /// to it and act on what it asks.
    fn serve(self: &Arc<Self>, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let hello = ipc::read_hello(&stream, ipc::HELLO_TIMEOUT);
        self.unauthenticated.fetch_sub(1, Ordering::Relaxed);
        let Some((token, role, pid)) = hello else { return };
        if !ipc::same_token(&token, &self.endpoint.token) {
            return;
        }
        let Ok(read_half) = stream.try_clone() else { return };
        let mut reader = BufReader::new(read_half);
        // The client's mailbox: the newest state first (a client reads a snapshot first), then the other messages in
        // order. A client that stops reading leaves one snapshot waiting, never a growing queue.
        let mailbox = Mailbox::new(true);
        let out = mailbox.clone();
        let mut writer = stream;
        // The window gets the state about 30 times a second at most (the service can publish faster: a voice's level,
        // an answer arriving, the avatar's speed): the newest waits in the mailbox meanwhile. The overlay, in the
        // headset, gets every one it can read.
        let pace = (role == Role::Window).then_some(WINDOW_PACE);
        #[cfg(test)]
        let writes = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let completed = writes.clone();
        std::thread::spawn(move || {
            // The state this link last wrote: the next one goes as what changed since (`ipc::write_state`), whatever
            // states the mailbox replaced in between. None yet, or the client lost step: the whole state.
            let mut sent: Option<Arc<crate::state::Snapshot>> = None;
            while let Some((messages, snapshot)) = out.take() {
                let paced = pace.filter(|_| snapshot.is_some());
                let written = snapshot
                    .map_or(Ok(()), |s| {
                        let written = ipc::write_state(&mut writer, sent.as_ref(), &s);
                        sent = Some(s);
                        written
                    })
                    .and_then(|_| messages.iter().try_for_each(|m| ipc::write_line(&mut writer, m)));
                if written.is_err() {
                    out.close();
                    return;
                }
                #[cfg(test)]
                completed.fetch_add(1, Ordering::Relaxed);
                if let Some(pace) = paced {
                    std::thread::sleep(pace);
                }
            }
        });
        // Subscribed before it is listed: the snapshot is in its mailbox before anything else can be (a Show).
        self.ctl.subscribe(mailbox.clone());
        let id = self.next_link.fetch_add(1, Ordering::Relaxed);
        self.usage.linked(id, role);
        self.links.lock().unwrap().push(Link { id, role, pid, mailbox: mailbox.clone(),
            #[cfg(test)]
            writes,
        });
        match role {
            Role::Window => {
                *self.window.lock().unwrap() = None;
                if let Some(page) = self.page_for_window.lock().unwrap().take() {
                    mailbox.post(ToClient::ShowPage { page });
                }
            }
            Role::Overlay => self.ctl.set_overlay_connected(true),
            Role::Orbit => {}
        }
        // Lines of a bounded length: a client cannot make the service hold an endless one.
        while let Ok(Some(line)) = ipc::read_line_capped(&mut reader, MAX_COMMAND) {
            if let Ok(msg) = serde_json::from_str::<ToService>(&line) {
                self.handle(id, role, msg);
            }
        }
        mailbox.close();
        self.usage.unlinked(id, self.ctl.view().settings.usage.enabled);
        self.links.lock().unwrap().retain(|l| l.id != id);
        let still_there = self.links.lock().unwrap().iter().any(|l| l.role == role);
        if role == Role::Overlay && !still_there {
            self.ctl.set_overlay_connected(false);
            self.ctl.booster_overlay_gone();
        }
        // The app's Talk (the window's or the panels') is let go only by the link that holds it down: the panels
        // closing must not end, nor send, a sentence the window is saying (AD-1).
        let held_talk = role != Role::Overlay && self.talk.lock().unwrap().closed(id);
        if role == Role::Overlay && !still_there {
            // What it held (its Talk button, its open menu) ends with it.
            self.ctl.n4n0_link_closed(true);
        }
        if role == Role::Window && held_talk {
            // A window gone with Talk held: its sentence is dropped (it may have crashed with the button down).
            self.ctl.n4n0_link_closed(false);
        }
        if role == Role::Orbit {
            // Its panels' Talk, should they have gone while held: the service never saw that let go.
            if held_talk {
                self.ctl.n4n0_command(crate::ipc::N4n0Command::Talk { down: false });
            }
            self.ctl.n4n0_command(crate::ipc::N4n0Command::MenuState { open: false, dictation: false, keyboard: false });
        }
        // Without a tray (Linux for now) nothing would bring the window back: closing it quits.
        if role == Role::Window && cfg!(not(any(windows, target_os = "macos"))) {
            self.quit();
        }
    }
}

/// Which link pressed the app's Talk button last and has not let it go. The window and the desktop panels talk to N4N0 as
/// one source: what one of them holds is let go when THAT link closes, not when another one does.
#[derive(Default)]
struct TalkHolder(Option<u64>);

impl TalkHolder {
    /// `link` pressed (`down`) or let go the app's Talk. Any link's release lets it go, as the runtime does.
    fn talk(&mut self, link: u64, down: bool) {
        self.0 = down.then_some(link);
    }

    /// `link` went away: whether it held the Talk button (which is then let go).
    fn closed(&mut self, link: u64) -> bool {
        let held = self.0 == Some(link);
        if held {
            self.0 = None;
        }
        held
    }
}

/// Whether [`Service::keep_steamvr_start`] has anything to look at, from the snapshot alone: SteamVR runs (the watchers
/// say) and the overlay, which looks at the SteamVR start itself, is not wanted.
fn steamvr_start_may_be_due(overlay_on: bool, steamvr_running: bool) -> bool {
    !overlay_on && steamvr_running
}

/// An overlay that failed is started again after this.
const OVERLAY_RETRY: Duration = Duration::from_secs(2);

/// Background connections cannot launch SteamVR. Every ended child is retryable,
/// including a transient not-running/changed-compositor result and actual QUIT.
/// Never overlap a live child. A new process identity resets the backoff.
fn overlay_due(exited: Option<Option<i32>>, since: Duration, started_for: Option<crate::steamvr_start::Session>, steamvr: Option<crate::steamvr_start::Session>, failures: u32) -> bool {
    if exited.is_none() { return false; }
    if steamvr != started_for { return true; }
    since >= OVERLAY_RETRY * if failures <= 1 { 1 } else { 2 }
}

/// Lets a process we start or signal take the foreground (Windows keeps background processes from stealing it).
fn allow_foreground(pid: u32) {
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// Runs the service until Quit. `hidden` starts in the tray without opening the window.
pub fn run(hidden: bool) -> anyhow::Result<()> {
    // A second launch shows the window instead (or quits the app with `--quit`); a hidden one (Windows or SteamVR
    // starting the app while it runs) changes nothing.
    let slot: Arc<Mutex<Option<Arc<Service>>>> = Arc::default();
    let requests = slot.clone();
    let _instance = match single::acquire(!hidden, move |request| {
        let Some(svc) = requests.lock().unwrap().clone() else { return };
        match request {
            single::Request::Show => svc.show_window(false),
            single::Request::Quit => svc.quit(),
        }
    })? {
        single::Instance::First(guard) => guard,
        single::Instance::Other => return Ok(()),
    };

    // An update that was ready when the app last quit installs now, before anything runs; its setup starts the app
    // again once done.
    if let Some((setup, dir)) = controller::update::pending_setup() {
        match controller::update::start_setup(&setup, &dir, hidden) {
            Ok(()) => return Ok(()),
            Err(e) => eprintln!("the update could not start: {e}"),
        }
    }

    #[cfg(target_os = "macos")]
    crate::startup::mac::follow_this_program();

    let settings = Settings::load();
    let _ = std::fs::create_dir_all(&settings.sessions_dir);
    let _ = std::fs::create_dir_all(&settings.raw_captures_dir);
    let ctl = Controller::new(settings);
    // What an earlier run of the FPS booster left comes back before anything else starts.
    ctl.start_booster();
    ctl.start();
    ctl.start_bridge();

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let endpoint = Endpoint { port: listener.local_addr()?.port(), token: uuid::Uuid::new_v4().simple().to_string() };
    #[cfg(not(windows))]
    let (quit_tx, quit_rx) = mpsc::channel();
    let service = Service::around(ctl, endpoint, std::env::current_exe()?);
    #[cfg(not(windows))]
    {
        *service.quit_tx.lock().unwrap() = Some(quit_tx);
    }
    *slot.lock().unwrap() = Some(service.clone());
    let weak = Arc::downgrade(&service);
    service.ctl.set_restarter(Box::new(move |setup, dir| {
        if let Some(svc) = weak.upgrade() {
            svc.restart_into(setup, dir);
        }
    }));

    service.usage.open(crate::usage::spool::folder());
    crate::usage::spool::set_current(service.usage.clone());
    let ctl = service.ctl.clone();
    service.usage.clone().run(move || ctl.view().settings.usage.enabled);
    service.accept(listener)?;

    if !hidden {
        service.show_window(true);
    }

    let svc = service.clone();
    std::thread::Builder::new().name("overlay-keeper".into()).spawn(move || loop {
        svc.keep_overlay();
        svc.keep_orbit();
        svc.keep_steamvr_start();
        svc.keep_tracking();
        std::thread::sleep(Duration::from_secs(2));
    })?;
    let svc = service.clone();
    std::thread::Builder::new().name("smoothing".into()).spawn(move || svc.follow_tracking())?;
    // Windows' default sound devices while SteamVR runs (Settings, "When SteamVR starts").
    let ctl = service.ctl.clone();
    std::thread::Builder::new().name("vr-audio".into()).spawn(move || controller::vr_audio::run(ctl))?;
    // In desktop mode, a recording's coverage in a corner of the screen (in VR the SteamVR overlay shows it).
    crate::coverage_window::start(service.ctl.clone());

    #[cfg(windows)]
    tray_loop(&service)?;
    // AppKit keeps the main thread until the process ends: this thread ends it once the service has quit.
    #[cfg(target_os = "macos")]
    {
        std::thread::Builder::new().name("leaving".into()).spawn(move || {
            let _ = quit_rx.recv();
            std::thread::sleep(Duration::from_millis(300));
            crate::mac::leave(move || drop(_instance));
        })?;
        crate::mac::run_service(service.clone())?;
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let _ = quit_rx.recv();

    // Let the windows read their Close before the process goes.
    #[cfg(not(target_os = "macos"))]
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

/// The tray needs a message loop on the thread that created it; menu text follows the capture state.
#[cfg(windows)]
fn tray_loop(service: &Arc<Service>) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_APP};
    const CAPTURE_CHANGED: u32 = WM_APP + 1;

    let mut tray = match crate::tray::Tray::new(service.clone()) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("tray icon: {e}");
            None
        }
    };
    // Tell the loop when capturing starts or stops (menu items live on this thread): its mailbox hands it the states
    // where that changed, and no other. The tray shows no notice: one that comes while no window is open waits for the
    // next.
    let mailbox = Mailbox::watching(|s| s.is_capturing());
    service.ctl.subscribe(mailbox.clone());
    let main_thread = service.main_thread;
    std::thread::Builder::new().name("tray-watch".into()).spawn(move || {
        let mut capturing = false;
        while let Some((_, snapshot)) = mailbox.take() {
            if let Some(snapshot) = snapshot {
                if snapshot.is_capturing() != capturing {
                    capturing = snapshot.is_capturing();
                    unsafe {
                        let _ = PostThreadMessageW(main_thread, CAPTURE_CHANGED, WPARAM(capturing as usize), LPARAM(0));
                    }
                }
            }
        }
    })?;

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        if msg.message == CAPTURE_CHANGED {
            if let Some(t) = tray.as_mut() {
                t.update(msg.wParam.0 != 0);
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn transient_unready_and_quit_retry_on_the_same_server_forever() {
        let session = Some((7, 100));
        for code in [Some(crate::overlay::EXIT_STEAMVR_GONE), Some(0), Some(1), None] {
            assert!(!overlay_due(None, OVERLAY_RETRY * 10, session, session, 1));
            assert!(!overlay_due(Some(code), Duration::from_secs(1), session, session, 1));
            assert!(overlay_due(Some(code), OVERLAY_RETRY, session, session, 1));
            for failures in [2, 10, u32::MAX] {
                assert!(!overlay_due(Some(code), OVERLAY_RETRY, session, session, failures));
                assert!(overlay_due(Some(code), OVERLAY_RETRY * 2, session, session, failures));
            }
        }
    }
    #[test]
    fn steamvr_exit_restart_and_pid_reuse_allow_immediate_retry() {
        for next in [Some((8, 200)), Some((7, 200))] {
            assert!(overlay_due(Some(Some(crate::overlay::EXIT_STEAMVR_GONE)), Duration::ZERO, Some((7, 100)), next, 100));
            assert!(!overlay_due(None, Duration::ZERO, Some((7, 100)), next, 100));
        }
    }

    /// AD-1: the panels closing do not let go the Talk the window holds, and the reverse; the link that holds it does.
    #[test]
    fn only_the_link_that_holds_talk_lets_it_go_when_it_closes() {
        let (window, panels) = (1, 2);
        let mut talk = TalkHolder::default();
        talk.talk(window, true);
        assert!(!talk.closed(panels), "the panels never pressed Talk");
        assert!(talk.closed(window), "the window held it");
        assert!(!talk.closed(window), "let go once");
        talk.talk(panels, true);
        assert!(!talk.closed(window));
        assert!(talk.closed(panels));
        // A release from either lets it go: nothing left for a later close.
        talk.talk(window, true);
        talk.talk(panels, false);
        assert!(!talk.closed(window));
    }

    /// LIGHT-DESKTOP-5: the SteamVR start is looked at (a scan of every process) only while SteamVR runs and the
    /// overlay is off; the two other cases returned after the scan before.
    #[test]
    fn the_steamvr_start_is_looked_for_only_while_steamvr_runs_without_the_overlay() {
        assert!(steamvr_start_may_be_due(false, true));
        assert!(!steamvr_start_may_be_due(false, false), "no SteamVR");
        assert!(!steamvr_start_may_be_due(true, true), "the overlay looks itself");
        assert!(!steamvr_start_may_be_due(true, false));
    }

    use crate::link_between::{Between, Line};
    use crate::state::{AvatarMotion, N4n0Line, N4n0Who, ReconstructionJob, Snapshot};

    /// A service as `run` makes it and serving as `run` serves, around a controller that is never started.
    pub(crate) fn service() -> Arc<Service> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = Endpoint { port: listener.local_addr().unwrap().port(), token: "link-test".into() };
        let service = Service::around(Controller::new(Settings::default()), endpoint, PathBuf::new());
        service.accept(listener).unwrap();
        service
    }

    /// A client of `role` linked to `service`.
    fn linked(service: &Arc<Service>, role: Role) -> Arc<ipc::Client> {
        ipc::Client::connect(&service.endpoint, role).unwrap()
    }

    /// A client of `role` linked to `service` through what can damage or hold the service's lines, or drop the
    /// connection.
    pub(crate) fn linked_through(service: &Arc<Service>, role: Role) -> (Arc<ipc::Client>, Between) {
        let between = Between::start(service.endpoint.port).unwrap();
        let client = ipc::Client::connect(&Endpoint { port: between.port(), token: service.endpoint.token.clone() }, role).unwrap();
        (client, between)
    }

    /// Waits up to five seconds for `done`.
    pub(crate) fn until(done: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while !done() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        done()
    }

    fn said(text: &str) -> N4n0Line {
        N4n0Line { who: N4n0Who::N4n0, text: text.into(), at_ms: 1, streaming: false }
    }

    fn whole(snapshot: &crate::state::Snapshot) -> serde_json::Value {
        serde_json::to_value(snapshot).unwrap()
    }

    /// Waits up to five seconds for `client` to hold what the service published, whole.
    fn in_step(service: &Service, client: &ipc::Client) -> bool {
        let started = Instant::now();
        loop {
            if whole(&client.snapshot()) == whole(&service.ctl.view()) {
                return true;
            }
            if started.elapsed() > Duration::from_secs(5) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The whole way, as the app runs it: the controller publishes, each link's thread writes what changed since what
    /// it last wrote, each client's thread takes it in. Every client holds what the service published, whichever part
    /// changed, however fast, whenever it connected, and after it asked for everything again.
    #[test]
    fn every_client_holds_what_the_service_published() {
        use orbiters_core::slice::Slice;
        let service = service();
        let ctl = service.ctl.clone();
        ctl.update(|s| s.n4n0.lines.push(said("before anyone was linked")));
        // The overlay's link: every state it can read.
        let overlay = linked(&service, Role::Overlay);
        assert!(in_step(&service, &overlay), "the first state, and the overlay counted as linked");
        assert!(overlay.snapshot().steamvr.overlay);
        // One part at a time: a plain value, a slice, a part inside a part, the settings.
        let before = overlay.snapshot();
        ctl.update(|s| s.vrchat.motion = Some(crate::state::AvatarMotion { in_station: false, seated: true, speed: 1.25, speed_at_ms: None }));
        assert!(in_step(&service, &overlay));
        ctl.update(|s| s.n4n0.panel = Some("N4N0's panel".into()));
        ctl.update(|s| {
            s.vrchat.world.players.insert("usr_1".into(), "Ada".into());
        });
        ctl.update(|s| s.settings.walk_boost.strength = 0.75);
        ctl.update(|s| s.projects_changed += 1);
        assert!(in_step(&service, &overlay));
        let after = overlay.snapshot();
        assert_eq!(after.settings.walk_boost.strength, 0.75);
        assert!(Slice::same(&before.n4n0.lines, &after.n4n0.lines) && Slice::same(&before.storage, &after.storage), "what did not change was not sent again");
        // A publish that changes nothing is told all the same: the client holds a new snapshot, and draws for it.
        let before = overlay.snapshot();
        let told = ctl.publishes();
        ctl.update(|_| {});
        assert_eq!(ctl.publishes(), told + 1);
        let started = Instant::now();
        while Arc::ptr_eq(&before, &overlay.snapshot()) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let after = overlay.snapshot();
        assert!(!Arc::ptr_eq(&before, &after), "told of a publish that changed nothing");
        assert!(Slice::same(&before.settings, &after.settings) && Slice::same(&before.capture, &after.capture));
        // A window that opens later is handed everything, and then only what changes, at its own pace.
        let window = linked(&service, Role::Window);
        assert!(in_step(&service, &window));
        assert_eq!(window.snapshot().n4n0.lines.len(), 1);
        // Changes faster than the window's link writes (it skips states), of two parts that go together.
        for round in 0..300u32 {
            ctl.update(|s| {
                s.account.state = crate::state::AccountState::SignedIn { username: format!("member-{round}") };
                s.n4n0.calls.picked = round as usize;
                if round % 50 == 0 {
                    s.n4n0.lines.push(crate::state::N4n0Line { who: crate::state::N4n0Who::You, text: format!("line {round}"), at_ms: round as u64, streaming: false });
                }
            });
            // Whatever state a client holds meanwhile is one the service published: the two parts agree.
            for client in [&overlay, &window] {
                let held = client.snapshot();
                if let crate::state::AccountState::SignedIn { username } = &held.account.state {
                    assert_eq!(*username, format!("member-{}", held.n4n0.calls.picked), "parts of two states");
                }
            }
        }
        assert!(in_step(&service, &overlay) && in_step(&service, &window));
        assert_eq!(window.snapshot().n4n0.calls.picked, 299);
        ctl.update(|s| s.n4n0.afk = true);
        assert!(in_step(&service, &window) && in_step(&service, &overlay));
        assert!(window.snapshot().n4n0.afk);
    }

    /// Review S2: a client whose connection drops is let go by the service, and the one that comes back is a new link,
    /// handed everything as the service has it then.
    #[test]
    fn a_client_that_comes_back_is_handed_everything_again() {
        let service = service();
        let ctl = service.ctl.clone();
        let (first, between) = linked_through(&service, Role::Overlay);
        ctl.update(|s| s.projects_changed = 42);
        assert!(in_step(&service, &first));
        assert!(ctl.view().steamvr.overlay && service.linked(Role::Overlay) == 1);
        // The connection drops: the client says the service is gone, the service lets the link go and no longer counts
        // an overlay as linked.
        between.hang_up();
        assert!(until(|| first.should_close()));
        assert!(until(|| service.linked(Role::Overlay) == 0 && !ctl.view().steamvr.overlay));
        let held = first.snapshot();
        // The service goes on, with nobody linked.
        ctl.update(|s| s.n4n0.lines.push(said("said while nobody was linked")));
        ctl.update(|s| s.settings.walk_boost.strength = 0.5);
        // It comes back: handed everything, and then what changes.
        let again = linked(&service, Role::Overlay);
        assert!(in_step(&service, &again));
        assert_eq!(again.snapshot().n4n0.lines.len(), 1);
        assert_eq!(again.snapshot().projects_changed, 42);
        assert!(until(|| ctl.view().steamvr.overlay));
        ctl.update(|s| s.n4n0.afk = true);
        assert!(in_step(&service, &again));
        assert!(Arc::ptr_eq(&held, &first.snapshot()), "nothing of the new link reaches the client that went");
        // Coming and going again and again leaves no link behind.
        for _ in 0..3 {
            let (window, between) = linked_through(&service, Role::Window);
            assert!(in_step(&service, &window));
            between.hang_up();
            assert!(until(|| window.should_close()));
        }
        assert!(until(|| service.linked(Role::Window) == 0));
        assert_eq!(service.linked(Role::Overlay), 1);
    }

    /// A damaged line terminates the real service link. A new client receives a full current baseline.
    #[test]
    fn a_damaged_line_closes_and_a_new_link_starts_whole() {
        let service = service();
        for round in 1..=2 {
            let (client, between) = linked_through(&service, Role::Window);
            assert!(in_step(&service, &client));
            between.damage(&[Line::Changes]);
            service.ctl.update(|s| s.projects_changed = round);
            assert!(until(|| client.should_close()));
            assert!(until(|| service.linked(Role::Window) == 0));
            assert_eq!(between.passed().1, 1);
        }
        let again = linked(&service, Role::Window);
        assert!(in_step(&service, &again));
        assert_eq!(again.snapshot().projects_changed, 2);
    }

    #[test]
    fn socket_regressions_twenty_rounds() {
        for _ in 0..20 {
            ipc::tests::unreadable_changes_close_without_the_writer_and_never_revive();
            ipc::tests::unreadable_wire_lines_end_the_link();
            a_damaged_line_closes_and_a_new_link_starts_whole();
            a_client_that_comes_back_is_handed_everything_again();
        }
    }

    /// Review S2: a reader that stops reading until the socket's buffers are full, then reads again. The service never
    /// waits for it, keeps one state for it and not a queue, and the newest state arrives whole when it reads again; the
    /// states it missed are never sent.
    #[test]
    fn a_reader_that_stalls_gets_the_newest_state_when_it_reads_again() {
        const ROUNDS: u32 = 300;
        let service = service();
        let ctl = service.ctl.clone();
        let (client, between) = linked_through(&service, Role::Overlay);
        assert!(in_step(&service, &client));
        let before = between.passed().0;
        between.hold(true);
        let writes = service.links.lock().unwrap()[0].writes.clone();
        // Await each write before publishing another, until one cannot complete while the proxy is held. This avoids
        // mistaking mailbox coalescing for full socket buffers. The overlay writer has no pacing delay.
        let mut blocked = false;
        for round in 0..100 {
            let before = writes.load(Ordering::Relaxed);
            ctl.update(|s| {
                *s.n4n0.lines = vec![said(&format!("{} {}", round, "x".repeat(1_000_000)))];
                s.projects_changed = round;
            });
            let began = Instant::now();
            while writes.load(Ordering::Relaxed) == before && began.elapsed() < Duration::from_millis(300) {
                std::thread::sleep(Duration::from_millis(2));
            }
            if writes.load(Ordering::Relaxed) == before {
                blocked = true;
                break;
            }
        }
        assert!(blocked, "the service writer must stall before the reader drains");
        // 200 kB that change every time, with a number in another part that goes with them: 60 MB in all, far more than
        // lies between two sockets.
        let big = "x".repeat(200_000);
        let started = Instant::now();
        for round in 0..ROUNDS {
            ctl.update(|s| {
                *s.n4n0.lines = vec![said(&format!("{round} {big}"))];
                s.projects_changed = round as u64;
            });
        }
        let publishing = started.elapsed();
        assert!(publishing < Duration::from_secs(20), "publishing waited for a reader that had stopped: {publishing:?}");
        let waiting: Vec<(usize, usize)> = service.links.lock().unwrap().iter().map(|l| l.mailbox.waiting()).collect();
        assert!(waiting.iter().all(|(states, messages)| *states <= 1 && *messages < 8), "one state waits, not a queue: {waiting:?}");
        assert!(client.snapshot().projects_changed < (ROUNDS - 1) as u64, "the reader has stopped");
        // It reads again. Whatever it holds on the way is a state the service published, and it ends on the newest.
        between.hold(false);
        let together = |s: &Snapshot| s.n4n0.lines.first().is_none_or(|line| line.text.starts_with(&format!("{} ", s.projects_changed)));
        let started = Instant::now();
        while client.snapshot().projects_changed < (ROUNDS - 1) as u64 && started.elapsed() < Duration::from_secs(20) {
            assert!(together(&client.snapshot()), "parts of two states");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(in_step(&service, &client));
        assert_eq!(client.snapshot().projects_changed, (ROUNDS - 1) as u64);
        let sent = between.passed().0 - before;
        assert!(sent < ROUNDS as u64 / 2, "{sent} lines for {ROUNDS} states: what a stalled reader missed is skipped, not queued");
    }

    /// Review S2: parts that change in one publish arrive in one line (a world with who is in it; a capture with its
    /// statistics and its 3D view's progress), on a link that writes every state and on the window's, which skips
    /// some; and a change undone before the paced link wrote it ends undone on the client.
    #[test]
    fn parts_that_change_together_arrive_together_and_a_change_undone_ends_undone() {
        let service = service();
        let ctl = service.ctl.clone();
        let (window, overlay) = (linked(&service, Role::Window), linked(&service, Role::Overlay));
        let together = |s: &Snapshot| {
            let world = s.vrchat.world.world_id.clone().unwrap_or_default();
            let people = s.vrchat.world.players.keys().all(|id| id.ends_with(&world)) && (world.is_empty() || !s.vrchat.world.players.is_empty());
            let frames = s.capture.stats.as_ref().map_or(0, |stats| stats.frames);
            let capture = s.capture.session_dir.as_ref().map_or(frames == 0, |dir| *dir == PathBuf::from(format!("session-{frames}")))
                && s.reconstruction.as_ref().map_or(frames == 0, |job| job.iteration as u64 == frames);
            people && capture
        };
        for round in 1..=200u32 {
            ctl.update(|s| {
                let world = format!("wrld_{round}");
                s.vrchat.world.players = (0..3).map(|i| (format!("usr_{i}_{world}"), format!("Someone {i}"))).collect();
                s.vrchat.world.world_id = Some(world);
                s.capture.session_dir = Some(PathBuf::from(format!("session-{round}")));
                s.capture.stats.get_or_insert_with(Default::default).frames = round as u64;
                let mut job = ReconstructionJob::new(PathBuf::from("a session"), false);
                job.iteration = round;
                s.reconstruction = Some(job);
            });
            for client in [&window, &overlay] {
                assert!(together(&client.snapshot()), "round {round}: parts of two states");
            }
        }
        assert!(in_step(&service, &window) && in_step(&service, &overlay));
        assert_eq!(window.snapshot().capture.stats.as_ref().map(|stats| stats.frames), Some(200));
        // A to B and back to A, faster than the window's link writes: a plain value, a slice, a part inside a part.
        let a = ctl.view();
        for round in 0..40u32 {
            ctl.update(|s| {
                s.settings.walk_boost.strength = 0.9;
                s.worn_kit = Some(true);
                s.vrchat.motion = Some(AvatarMotion { in_station: true, seated: false, speed: round as f32, speed_at_ms: None });
                s.n4n0.panel = Some("B".into());
            });
            ctl.update(|s| {
                s.settings = a.settings.clone();
                s.worn_kit = a.worn_kit;
                s.vrchat.motion = a.vrchat.motion;
                s.n4n0.panel = a.n4n0.panel.clone();
            });
            if round % 8 == 0 {
                assert!(in_step(&service, &window) && in_step(&service, &overlay), "round {round}");
            }
        }
        assert!(in_step(&service, &window) && in_step(&service, &overlay));
        assert_eq!(whole(&window.snapshot()), whole(&a), "back on A, whole");
        assert_ne!(window.snapshot().settings.walk_boost.strength, 0.9);
    }

    #[test]
    fn every_declared_part_arrives_through_the_service() {
        use orbiters_core::slice::{apply_changes, Parts};
        let service = service();
        // Publication intentionally clears VRChat login state without a signed-in account on the active server.
        // Supply that prerequisite instead of expecting an invalid fixture to survive the real publisher.
        service.ctl.update(|s| {
            s.account.state = crate::state::AccountState::SignedIn { username: "wire fixture".into() };
            s.account.development = orbiters_core::account::Server::current() == orbiters_core::account::Server::Development;
        });
        let client = linked(&service, Role::Window);
        assert!(in_step(&service, &client));
        let baseline = service.ctl.view();
        let full = whole(&ipc::tests::every_part_set());
        let mut names = Vec::new();
        baseline.part_names("", &mut names);
        for name in names {
            let value = name.split('.').fold(&full, |at, key| &at[key]);
            let mut next = (*baseline).clone();
            apply_changes(&mut next, &serde_json::json!({name.as_str(): value}).to_string()).unwrap();
            service.ctl.set_snapshot(next);
            assert!(in_step(&service, &client), "{name} did not arrive whole");
            let received = whole(&client.snapshot());
            assert_eq!(name.split('.').fold(&received, |at, key| &at[key]), value, "{name}");
        }
    }

    #[test]
    fn update_actions_reach_the_service() {
        let line = serde_json::to_string(&ToService::Updates { action: ipc::UpdateAction::Restart { now: false } }).unwrap();
        assert_eq!(line, r#"{"type":"updates","action":{"kind":"restart","now":false}}"#);
        match serde_json::from_str::<ToService>(r#"{"type":"updates","action":{"kind":"check"}}"#).unwrap() {
            ToService::Updates { action } => assert_eq!(action, ipc::UpdateAction::Check),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_settings_patch_reaches_the_service_as_a_patch() {
        let line = r#"{"type":"patch_settings","patch":{"n4n0":{"wake_word":true}}}"#;
        match serde_json::from_str::<ToService>(line).unwrap() {
            ToService::PatchSettings { patch } => assert_eq!(patch["n4n0"]["wake_word"], true),
            other => panic!("{other:?}"),
        }
    }

    /// The beta's usage statistics from a linked window: its steps become a session (started at the first one) in the service's spool; turned
    /// off in the settings, everything waiting is forgotten at once and nothing more is kept; a link that goes ends its session where it was.
    #[test]
    fn a_links_usage_becomes_a_session_and_the_switch_forgets_it() {
        use crate::usage::{Kind, Step};
        let service = service();
        let (window, between) = linked_through(&service, Role::Window);
        let step = |kind, screen: &str, control: Option<&str>| Step { at: crate::usage::now_ms(), kind, screen: screen.into(), from: None, control: control.map(Into::into) };
        window.send(&ToService::Usage { steps: vec![step(Kind::Enter, "Home", None), step(Kind::Press, "Home", Some("Open Settings")), step(Kind::Enter, "Settings", None)] });
        assert!(until(|| service.usage.waiting().len() == 4), "{:?}", service.usage.waiting());
        let kinds: Vec<String> = service.usage.waiting().iter().map(|e| format!("{} {}", e.kind, e.screen.as_deref().unwrap_or(""))).collect();
        assert_eq!(kinds, ["start Home", "enter Home", "press Home", "enter Settings"]);
        assert!(service.usage.waiting().iter().all(|e| e.surface == "app"));
        // The window goes: its session ends on Settings.
        drop(between);
        assert!(until(|| service.usage.waiting().last().is_some_and(|e| e.kind == "end")));
        assert_eq!(service.usage.waiting().last().and_then(|e| e.screen.clone()).as_deref(), Some("Settings"));
        // Turned off from another window: all forgotten, and what comes next is not kept.
        let other = linked(&service, Role::Window);
        other.send(&ToService::PatchSettings { patch: serde_json::json!({ "usage": { "enabled": false } }) });
        assert!(until(|| service.usage.waiting().is_empty()));
        other.send(&ToService::Usage { steps: vec![step(Kind::Enter, "Home", None)] });
        std::thread::sleep(Duration::from_millis(200));
        assert!(service.usage.waiting().is_empty(), "nothing kept while off");
    }
}
