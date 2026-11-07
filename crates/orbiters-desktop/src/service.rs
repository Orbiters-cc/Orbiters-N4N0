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
    /// The overlay process we started, when, and for which SteamVR (its `vrserver` process id).
    overlay: Mutex<Option<(Child, Instant, Option<u32>)>>,
    /// The process of the panels around VRChat's menus in desktop mode, and when it started.
    orbit: Mutex<Option<(Child, Instant)>>,
    /// The SteamVR (its `vrserver` process id, 0 unknown) whose start of the app was looked at ([`Service::keep_steamvr_start`]).
    steamvr_start_seen: Mutex<Option<u32>>,
    /// Tracking smoothing's driver, kept in step with the settings ([`Service::keep_tracking`]).
    smoothing: Mutex<crate::tracking::Smoothing>,
    quitting: AtomicBool,
    #[cfg(windows)]
    main_thread: u32,
    #[cfg(not(windows))]
    quit_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl Service {
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
        let session = snap.steamvr.pid.unwrap_or(0);
        if !snap.steamvr.running || *self.steamvr_start_seen.lock().unwrap() == Some(session) {
            return;
        }
        // Not yet answering (it is starting, or going): asked again next turn.
        let Ok(client) = orbiters_openvr::Client::connect(orbiters_openvr::AppType::Background) else { return };
        *self.steamvr_start_seen.lock().unwrap() = Some(session);
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
    /// the keeper's next turn: every published state is looked at, the tracking settings compared.
    fn follow_tracking(self: &Arc<Self>) {
        let mailbox = Mailbox::new(false);
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
            return;
        }
        drop(links);
        let mut overlay = self.overlay.lock().unwrap();
        if let Some((child, started, steamvr)) = overlay.as_mut() {
            let exited = match child.try_wait() {
                Ok(None) => None,
                Ok(Some(status)) => Some(status.code()),
                Err(_) => Some(None),
            };
            if !overlay_due(exited, started.elapsed(), *steamvr, snap.steamvr.pid) {
                return;
            }
        }
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
                *overlay = Some((child, Instant::now(), snap.steamvr.pid));
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
            ToService::StartCapture => report(ctl.start_capture(false)),
            ToService::StopCapture => report(ctl.stop_capture()),
            ToService::Pause { paused } => report(ctl.pause_capture(paused)),
            ToService::TestPattern => report(ctl.start_test_capture()),
            ToService::Scan => report(ctl.take_scan()),
            ToService::ImportRaw { dir } => report(ctl.import_raw_capture(dir)),
            ToService::SaveSettings { settings } => ctl.save_settings(*settings),
            ToService::PatchSettings { patch } => ctl.patch_settings(&patch),
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
            ToService::VrchatData { id, scope, request } => {
                let mailbox = self.links.lock().unwrap().iter().find(|l| l.id == link).map(|l| l.mailbox.clone());
                if let Some(mailbox) = mailbox {
                    let ctl = ctl.clone();
                    std::thread::spawn(move || {
                        let result = ctl.vrchat_data(scope, request);
                        mailbox.post(ToClient::VrchatData { id, result });
                    });
                }
            }
            // Only the window picks picture files; the upload runs on a thread of its own.
            ToService::ShowWindow => self.show_window(false),
            ToService::Orbit { status } => self.ctl.set_orbit_status(status),
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
            ToService::Booster { command } => ctl.booster_command(command),
            ToService::WalkPath { path } => ctl.set_walk_path(crate::tracking::walk_path_said(path)),
            // Which Talk button is held matters: the service lets it go when that window or overlay goes away.
            ToService::N4n0 { command: N4n0Command::Talk { down } } => ctl.n4n0_app_talk(down, role == Role::Overlay),
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
        // The window gets the state about 30 times a second at most (an open N4N0 menu republishes 70 times): the newest
        // waits in the mailbox meanwhile. The overlay, in the headset, gets every one it can read.
        let pace = (role == Role::Window).then_some(WINDOW_PACE);
        std::thread::spawn(move || {
            while let Some((messages, snapshot)) = out.take() {
                let paced = pace.filter(|_| snapshot.is_some());
                let written = snapshot
                    .map_or(Ok(()), |s| ipc::write_snapshot(&mut writer, &s))
                    .and_then(|_| messages.iter().try_for_each(|m| ipc::write_line(&mut writer, m)));
                if written.is_err() {
                    out.close();
                    return;
                }
                if let Some(pace) = paced {
                    std::thread::sleep(pace);
                }
            }
        });
        // Subscribed before it is listed: the snapshot is in its mailbox before anything else can be (a Show).
        self.ctl.subscribe(mailbox.clone());
        let id = self.next_link.fetch_add(1, Ordering::Relaxed);
        self.links.lock().unwrap().push(Link { id, role, pid, mailbox: mailbox.clone() });
        match role {
            Role::Window => *self.window.lock().unwrap() = None,
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
        self.links.lock().unwrap().retain(|l| l.id != id);
        let still_there = self.links.lock().unwrap().iter().any(|l| l.role == role);
        if role == Role::Overlay && !still_there {
            self.ctl.set_overlay_connected(false);
            self.ctl.booster_overlay_gone();
        }
        if !still_there && role != Role::Orbit {
            // What it held (a Talk button, the overlay's open menu) ends with it.
            self.ctl.n4n0_link_closed(role == Role::Overlay);
        }
        if role == Role::Orbit {
            // Its panels' Talk, should it have gone while held: the service never saw that let go.
            self.ctl.n4n0_command(crate::ipc::N4n0Command::Talk { down: false });
            self.ctl.n4n0_command(crate::ipc::N4n0Command::MenuState { open: false, dictation: false, keyboard: false });
        }
        // Without a tray (Linux for now) nothing would bring the window back: closing it quits.
        if role == Role::Window && cfg!(not(any(windows, target_os = "macos"))) {
            self.quit();
        }
    }
}

/// An overlay that failed is started again after this.
const OVERLAY_RETRY: Duration = Duration::from_secs(20);

/// Whether to start an overlay again, the last one started `since` ago for the SteamVR `started_for` (its process id)
/// having `exited` (`None` while it runs; its exit code, if any), SteamVR now being `steamvr`. Not while it runs; not
/// for the SteamVR it left because that one was gone or quitting ([`crate::overlay::EXIT_STEAMVR_GONE`]), which would
/// start SteamVR again or meet it going: only another SteamVR brings it back; after another failure, in a while.
fn overlay_due(exited: Option<Option<i32>>, since: Duration, started_for: Option<u32>, steamvr: Option<u32>) -> bool {
    match exited {
        None => false,
        Some(Some(crate::overlay::EXIT_STEAMVR_GONE)) => steamvr != started_for,
        Some(_) => since >= OVERLAY_RETRY,
    }
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
    let service = Arc::new(Service {
        ctl,
        endpoint,
        exe: std::env::current_exe()?,
        links: Mutex::new(Vec::new()),
        next_link: AtomicU64::new(1),
        unauthenticated: AtomicUsize::new(0),
        window: Mutex::new(None),
        overlay: Mutex::new(None),
        orbit: Mutex::new(None),
        steamvr_start_seen: Mutex::new(None),
        smoothing: Mutex::new(Default::default()),
        quitting: AtomicBool::new(false),
        #[cfg(windows)]
        main_thread: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
        #[cfg(not(windows))]
        quit_tx: Mutex::new(Some(quit_tx)),
    });
    *slot.lock().unwrap() = Some(service.clone());
    let weak = Arc::downgrade(&service);
    service.ctl.set_restarter(Box::new(move |setup, dir| {
        if let Some(svc) = weak.upgrade() {
            svc.restart_into(setup, dir);
        }
    }));

    let svc = service.clone();
    std::thread::Builder::new().name("ipc".into()).spawn(move || {
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
    })?;

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
    // Tell the loop when capturing starts or stops (menu items live on this thread). The tray shows no notice: one
    // that comes while no window is open waits for the next.
    let mailbox = Mailbox::new(false);
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
mod tests {
    use super::*;

    #[test]
    fn an_overlay_that_left_with_steamvr_waits_for_another_steamvr() {
        let gone = Some(Some(crate::overlay::EXIT_STEAMVR_GONE));
        let (soon, later) = (Duration::from_secs(1), OVERLAY_RETRY);
        // Running, or still starting.
        assert!(!overlay_due(None, later, Some(7), Some(7)));
        // It left because SteamVR quit or was gone: not for that SteamVR, at once for another.
        assert!(!overlay_due(gone, later * 10, Some(7), Some(7)));
        assert!(overlay_due(gone, soon, Some(7), Some(8)));
        // Another failure: tried again after a while.
        assert!(!overlay_due(Some(Some(1)), soon, Some(7), Some(7)));
        assert!(overlay_due(Some(Some(1)), later, Some(7), Some(7)));
        assert!(overlay_due(Some(None), later, Some(7), Some(7)), "ended without a code");
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
}
