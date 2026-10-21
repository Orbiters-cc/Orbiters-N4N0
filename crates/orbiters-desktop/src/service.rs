//! The tray service: what runs while Orbiters desktop waits in the tray. It owns the controller (capture worker,
//! OSC, VRChat and SteamVR watchers), the tray icon and single-instance handling, and serves the window and the VR
//! overlay over the IPC link. It never creates a GPU device: the window process does, and exits when closed; the
//! overlay process runs while SteamVR does.

use crate::controller::{self, Controller, NoticeKind};
use crate::ipc::{self, Endpoint, Role, ToClient, ToService};
use crate::single;
use orbiters_core::settings::Settings;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

struct Link {
    id: u64,
    role: Role,
    pid: u32,
    tx: mpsc::Sender<ToClient>,
}

pub struct Service {
    pub ctl: controller::Shared,
    endpoint: Endpoint,
    exe: PathBuf,
    links: Mutex<Vec<Link>>,
    next_link: AtomicU64,
    /// The window process we started and when (a second request while it starts does not start another).
    window: Mutex<Option<(Child, Instant)>>,
    /// The overlay process we started and when.
    overlay: Mutex<Option<(Child, Instant)>>,
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
            let _ = link.tx.send(ToClient::Show);
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
                allow_foreground(child.id());
                *window = Some((child, Instant::now()));
            }
            Err(e) => eprintln!("could not start the window: {e}"),
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
                    let _ = link.tx.send(ToClient::Close);
                }
            }
            return;
        }
        if connected.is_some() {
            return;
        }
        drop(links);
        let mut overlay = self.overlay.lock().unwrap();
        if let Some((child, started)) = overlay.as_mut() {
            // Starting, or it failed a moment ago: try again later.
            if matches!(child.try_wait(), Ok(None)) || started.elapsed() < Duration::from_secs(20) {
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
            Ok(child) => *overlay = Some((child, Instant::now())),
            Err(e) => eprintln!("could not start the SteamVR overlay: {e}"),
        }
    }

    /// Closes every window, stops the worker (it finishes a running session) and leaves.
    pub fn quit(&self) {
        if self.quitting.swap(true, Ordering::Relaxed) {
            return;
        }
        for link in self.links.lock().unwrap().iter() {
            let _ = link.tx.send(ToClient::Close);
        }
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

    fn handle(&self, link: u64, msg: ToService) {
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
            ToService::ImportRaw { dir } => report(ctl.import_raw_capture(dir)),
            ToService::SaveSettings { settings } => ctl.save_settings(*settings),
            ToService::Trash { dir } => {
                let r = ctl.trash_session(&dir);
                if r.is_ok() {
                    ctl.notify(NoticeKind::Success, "Session moved to the recycle bin.");
                }
                report(r);
            }
            ToService::Reconstruct { dir } => report(ctl.reconstruct(dir)),
            ToService::SendToMixer { dir, title } => report(ctl.send_to_mixer(dir, title)),
            ToService::SignIn { provider } => {
                // Asking Orbiters for the link is a network call: not on the link's reading thread.
                let c = ctl.clone();
                std::thread::spawn(move || {
                    if let Err(e) = c.sign_in(&provider) {
                        c.notify(NoticeKind::Error, format!("Could not start the sign-in: {e}"));
                    }
                });
            }
            ToService::ReopenSignIn => report(ctl.reopen_sign_in()),
            ToService::CancelSignIn => ctl.cancel_sign_in(),
            ToService::SignOut => report(ctl.sign_out()),
            ToService::StopReconstruction => {
                ctl.stop_reconstruction();
            }
            ToService::WorkerLog => {
                let lines = ctl.worker_log();
                if let Some(l) = self.links.lock().unwrap().iter().find(|l| l.id == link) {
                    let _ = l.tx.send(ToClient::WorkerLog { lines });
                }
            }
            ToService::UploadPictures { world_id, world_name, paths } => report(ctl.upload_pictures(world_id, world_name, paths)),
            ToService::UploadPreview { world_id, world_name, session } => report(ctl.upload_preview(world_id, world_name, session)),
            ToService::Maps => ctl.refresh_maps(),
            ToService::Calibrated { calibration } => ctl.calibrated(*calibration),
            ToService::Frames { chunk } => ctl.frames(chunk),
            ToService::WalkBoost { strength } => ctl.set_walk_boost(strength),
            ToService::Orb { command } => ctl.orb_command(command),
            ToService::Quit => self.quit(),
        }
    }

    /// One client: check its token, then stream every change to it and act on what it asks.
    fn serve(self: &Arc<Self>, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        let Some((token, role, pid)) = ipc::read_hello(&stream, ipc::HELLO_TIMEOUT) else { return };
        if token != self.endpoint.token {
            return;
        }
        let Ok(read_half) = stream.try_clone() else { return };
        let reader = BufReader::new(read_half);
        let (tx, rx) = mpsc::channel::<ToClient>();
        let mut writer = stream;
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                // Of a burst of snapshots only the newest matters.
                let mut batch = vec![first];
                batch.extend(rx.try_iter());
                let newest = batch.iter().rposition(|m| matches!(m, ToClient::Snapshot { .. }));
                for (i, msg) in batch.iter().enumerate() {
                    if matches!(msg, ToClient::Snapshot { .. }) && Some(i) != newest {
                        continue;
                    }
                    if ipc::write_line(&mut writer, msg).is_err() {
                        return;
                    }
                }
            }
        });
        let id = self.next_link.fetch_add(1, Ordering::Relaxed);
        self.links.lock().unwrap().push(Link { id, role, pid, tx: tx.clone() });
        match role {
            Role::Window => *self.window.lock().unwrap() = None,
            Role::Overlay => self.ctl.set_overlay_connected(true),
        }
        self.ctl.subscribe(tx);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Ok(msg) = serde_json::from_str::<ToService>(&line) {
                self.handle(id, msg);
            }
        }
        self.links.lock().unwrap().retain(|l| l.id != id);
        if role == Role::Overlay && !self.links.lock().unwrap().iter().any(|l| l.role == Role::Overlay) {
            self.ctl.set_overlay_connected(false);
        }
        // Without a tray (Linux for now) nothing would bring the window back: closing it quits.
        if role == Role::Window && cfg!(not(windows)) {
            self.quit();
        }
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
    // A second launch shows the window instead (or quits the app with `--quit`).
    let slot: Arc<Mutex<Option<Arc<Service>>>> = Arc::default();
    let requests = slot.clone();
    let _instance = match single::acquire(move |request| {
        let Some(svc) = requests.lock().unwrap().clone() else { return };
        match request {
            single::Request::Show => svc.show_window(false),
            single::Request::Quit => svc.quit(),
        }
    })? {
        single::Instance::First(guard) => guard,
        single::Instance::Other => return Ok(()),
    };

    let settings = Settings::load();
    let _ = std::fs::create_dir_all(&settings.sessions_dir);
    let _ = std::fs::create_dir_all(&settings.raw_captures_dir);
    let ctl = Controller::new(settings);
    ctl.start();

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
        window: Mutex::new(None),
        overlay: Mutex::new(None),
        quitting: AtomicBool::new(false),
        #[cfg(windows)]
        main_thread: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
        #[cfg(not(windows))]
        quit_tx: Mutex::new(Some(quit_tx)),
    });
    *slot.lock().unwrap() = Some(service.clone());

    let svc = service.clone();
    std::thread::Builder::new().name("ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = svc.clone();
            let _ = std::thread::Builder::new().name("ipc-client".into()).spawn(move || s.serve(stream));
        }
    })?;

    if !hidden {
        service.show_window(true);
    }

    let svc = service.clone();
    std::thread::Builder::new().name("overlay-keeper".into()).spawn(move || loop {
        svc.keep_overlay();
        std::thread::sleep(Duration::from_secs(2));
    })?;

    #[cfg(windows)]
    tray_loop(&service)?;
    #[cfg(not(windows))]
    let _ = quit_rx.recv();

    // Let the windows read their Close before the process goes.
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
    // Tell the loop when capturing starts or stops (menu items live on this thread).
    let (tx, rx) = mpsc::channel();
    service.ctl.subscribe(tx);
    let main_thread = service.main_thread;
    std::thread::Builder::new().name("tray-watch".into()).spawn(move || {
        let mut capturing = false;
        for msg in rx {
            if let ToClient::Snapshot { snapshot } = msg {
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
