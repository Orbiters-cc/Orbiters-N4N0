//! `orbiters-desktop --link-smoke [--no-window] [--no-panels] [--keep]`: the link between the service and the real
//! processes that show its state, seen working without the installed app and without a headset (architecture review
//! 2026-11-10, DRT-1's second review).
//!
//! It makes a service of its own: a controller that is never started (no OSC, no capture worker, no watcher, no
//! account), the real [`Service`] serving on a port of its own, every folder of the app in a temporary folder. It
//! starts the real window process, and the desktop panels' process, against that port, each through a
//! [`Between`] that can damage a line or drop the connection. Nothing looks for the running app: a second launch's
//! single-instance signal is the tray service's own (`service::run`), which this never calls.
//!
//! Then it changes the state as the service does, publishes with nothing changed, has lines cut short on their way,
//! and drops the connections. After each step it reads from each process, which prints a digest of the state its link
//! holds whenever that changes (`--say-state`), that it holds the service's state, whole. It never looks at the screen,
//! sends no key or pointer event, and the window is asked not to take the foreground (`--no-focus`), which it checks.
//!
//! It refuses to start while VRChat or SteamVR runs: the window is a GPU program beside the game, and the panels'
//! process would find the game's window. The overlay in the headset needs SteamVR and is not part of this.

use crate::controller::Controller;
use crate::ipc::{self, Endpoint, Role};
use crate::link_between::{Between, Line};
use crate::service::Service;
use anyhow::Context as _;
use orbiters_core::settings::Settings;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Every folder of the app inside `root`, for this process: what `--sandbox <folder>` asks, and this harness for itself.
pub fn sandbox(root: &Path) -> anyhow::Result<()> {
    let roots = orbiters_core::paths::Roots { config: root.join("config"), local_data: root.join("local"), cache: root.join("cache"), documents: root.join("documents") };
    for dir in [&roots.config, &roots.local_data, &roots.cache, &roots.documents] {
        std::fs::create_dir_all(dir)?;
    }
    orbiters_core::paths::set_roots(roots)
}

/// What a process said of its link.
#[derive(Default)]
struct Said {
    /// The digest of the state it holds, and how many states it has taken.
    state: Option<String>,
    states: u64,
    /// Lines it could not read.
    lost: u64,
    ended: bool,
}

struct Shown {
    name: &'static str,
    role: Role,
    process: std::process::Child,
    said: Arc<Mutex<Said>>,
    between: Between,
}

impl Drop for Shown {
    fn drop(&mut self) {
        self.between.hang_up();
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn start(name: &'static str, role: Role, mode: &str, extra: &[&str], service: &Endpoint, root: &Path) -> anyhow::Result<Shown> {
    let between = Between::start(service.port)?;
    let endpoint = Endpoint { port: between.port(), token: service.token.clone() };
    let mut command = Command::new(std::env::current_exe()?);
    command.arg(mode).args(endpoint.args()).arg("--sandbox").arg(root).arg("--say-state").args(extra);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(std::fs::File::create(root.join(format!("{name}.stderr.txt")))?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console of its own
    }
    let mut process = command.spawn().with_context(|| format!("starting the {name}"))?;
    crate::worker::keep_with_service(&process);
    let said = Arc::new(Mutex::new(Said::default()));
    let (out, heard) = (process.stdout.take().context("its output")?, said.clone());
    std::thread::Builder::new().name("link-smoke-reader".into()).spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let mut said = heard.lock().unwrap();
            if let Some(digest) = line.strip_prefix("state ") {
                said.state = Some(digest.trim().to_string());
                said.states += 1;
            } else if line.starts_with("lost step") {
                said.lost += 1;
            } else if line.starts_with("link ended") {
                said.ended = true;
            }
        }
    })?;
    Ok(Shown { name, role, process, said, between })
}

/// Whether the foreground window is `process`'s.
fn has_foreground(process: u32) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
        let mut owner = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&raw mut owner));
        owner == process
    }
    #[cfg(not(windows))]
    {
        let _ = process;
        false
    }
}

/// The game or SteamVR runs: their process's name, if so.
fn game_running() -> Option<String> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    system.processes().values().map(|p| p.name().to_string_lossy().to_lowercase()).find(|name| ["vrchat.exe", "vrserver.exe", "vrmonitor.exe", "vrchat", "vrserver"].contains(&name.as_str()))
}

/// How the run goes.
struct Run {
    failed: u32,
    deadline: Instant,
    /// One of its processes had the foreground window at some moment.
    foreground: bool,
}

impl Run {
    /// Does `act`, then waits for every process to say it holds the service's state, having taken at least one more
    /// state than before.
    fn step(&mut self, what: &str, ctl: &crate::controller::Shared, shown: &[Shown], wait: Duration, act: &dyn Fn(&[Shown])) {
        let before: Vec<u64> = shown.iter().map(|s| s.said.lock().unwrap().states).collect();
        let began = Instant::now();
        let wait = wait.min(self.deadline.saturating_duration_since(began));
        act(shown);
        let wanted = ipc::digest(&ctl.view());
        let mut took: Vec<Option<Duration>> = vec![None; shown.len()];
        while took.iter().any(Option::is_none) && began.elapsed() < wait {
            for (i, s) in shown.iter().enumerate() {
                let said = s.said.lock().unwrap();
                if took[i].is_none() && said.state.as_deref() == Some(wanted.as_str()) && said.states > before[i] {
                    took[i] = Some(began.elapsed());
                }
            }
            self.foreground |= shown.iter().any(|s| has_foreground(s.process.id()));
            std::thread::sleep(Duration::from_millis(5));
        }
        let all = took.iter().all(Option::is_some);
        self.failed += !all as u32;
        let each: Vec<String> = shown
            .iter()
            .zip(&took)
            .map(|(s, took)| match took {
                Some(took) => format!("{} in {} ms", s.name, took.as_millis()),
                None => format!("{} NOT in step after {} ms (it says {:?}, the service holds {wanted})", s.name, wait.as_millis(), s.said.lock().unwrap().state),
            })
            .collect();
        println!("{} {what}: {}", if all { "ok  " } else { "FAIL" }, each.join("; "));
    }
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    if let Some(game) = game_running() {
        anyhow::bail!("{game} is running: this starts a window beside it and a process that looks for the game's window. Run it when neither VRChat nor SteamVR does.");
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let root = std::env::temp_dir().join(format!("orbiters-link-smoke-{}", std::process::id()));
    sandbox(&root)?;
    let started = Instant::now();

    // The service: a controller that is never started, holding a state the size of a busy evening, with VRChat said
    // to run in desktop mode (so the panels' process stays) though none does.
    let ctl = Controller::new(Settings::default());
    let mut state = crate::controller::publish_cost::busy_evening();
    state.account = Default::default();
    state.vrchat.pid = None;
    *state.steamvr = Default::default();
    ctl.set_snapshot(state);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let endpoint = Endpoint { port: listener.local_addr()?.port(), token: uuid::Uuid::new_v4().simple().to_string() };
    let service = Service::around(ctl.clone(), endpoint.clone(), std::env::current_exe()?);
    service.accept(listener)?;

    let mut shown = Vec::new();
    if !has("--no-window") {
        shown.push(start("window", Role::Window, "--window", &["--no-focus"], &endpoint, &root)?);
    }
    #[cfg(windows)]
    if !has("--no-panels") {
        shown.push(start("desktop panels", Role::Orbit, "--orbit-desktop", &[], &endpoint, &root)?);
    }
    anyhow::ensure!(!shown.is_empty(), "nothing to start");

    let mut run = Run { failed: 0, foreground: false, deadline: started + Duration::from_secs(45) };
    let soon = Duration::from_secs(8);

    run.step("linked, the first state (a process starting)", &ctl, &shown, Duration::from_secs(25), &|_| {});
    for s in &shown {
        anyhow::ensure!(service.linked(s.role) == 1, "the {} is not linked to the service", s.name);
    }
    run.step("a plain value changes", &ctl, &shown, soon, &|_| ctl.update(|s| s.projects_changed += 1));
    run.step("N4N0's panel text changes", &ctl, &shown, soon, &|_| ctl.update(|s| s.n4n0.panel = Some("Seen by a real process".into())));
    run.step("a line of the conversation is added", &ctl, &shown, soon, &|_| {
        ctl.update(|s| s.n4n0.lines.push(crate::state::N4n0Line { who: crate::state::N4n0Who::N4n0, text: "A line more".into(), at_ms: 1, streaming: false }))
    });
    run.step("someone joins the world", &ctl, &shown, soon, &|_| {
        ctl.update(|s| {
            s.vrchat.world.players.insert("usr_newcomer".into(), "A newcomer".into());
        })
    });
    run.step("a setting changes", &ctl, &shown, soon, &|_| ctl.update(|s| s.settings.walk_boost.strength = if s.settings.walk_boost.strength > 0.5 { 0.25 } else { 0.75 }));
    run.step("the avatar's speed changes 200 times at once", &ctl, &shown, soon, &|_| {
        for round in 0..200 {
            ctl.update(|s| s.vrchat.motion = Some(crate::state::AvatarMotion { in_station: false, seated: false, speed: 0.25 * (round % 12) as f32 }));
        }
    });
    for round in 1..=3 {
        run.step(&format!("a publish with nothing changed ({round})"), &ctl, &shown, soon, &|_| ctl.update(|_| {}));
    }
    let lost_before: Vec<u64> = shown.iter().map(|s| s.said.lock().unwrap().lost).collect();
    run.step("a change cut short on its way", &ctl, &shown, soon, &|shown| {
        shown.iter().for_each(|s| s.between.damage(&[Line::Changes]));
        ctl.update(|s| s.n4n0.panel = Some("After a line cut short".into()));
    });
    run.step("at once, another change cut short", &ctl, &shown, soon, &|shown| {
        shown.iter().for_each(|s| s.between.damage(&[Line::Changes]));
        ctl.update(|s| s.n4n0.panel = Some("After a second line cut short".into()));
    });
    run.step("a change and the whole state sent in answer both cut short, then nothing", &ctl, &shown, soon, &|shown| {
        shown.iter().for_each(|s| s.between.damage(&[Line::Changes, Line::Whole]));
        ctl.update(|s| s.n4n0.panel = Some("After an answer cut short".into()));
    });
    for (s, before) in shown.iter().zip(&lost_before) {
        let (lost, damaged) = (s.said.lock().unwrap().lost - before, s.between.passed().1);
        println!("     the {}: {damaged} lines cut short on their way, {lost} it said it could not read", s.name);
        run.failed += (damaged != 4 || lost < 3) as u32;
    }
    run.step("a change after all that", &ctl, &shown, soon, &|_| ctl.update(|s| s.projects_changed += 1));

    // The connection drops: each process says its link ended, and ends.
    for s in &shown {
        s.between.hang_up();
    }
    for s in &mut shown {
        let began = Instant::now();
        let mut code = None;
        while code.is_none() && began.elapsed() < Duration::from_secs(5) && started.elapsed() < Duration::from_secs(50) {
            code = s.process.try_wait()?;
            std::thread::sleep(Duration::from_millis(20));
        }
        let ended = s.said.lock().unwrap().ended;
        match code {
            Some(code) => println!("{} the connection dropped: the {} said its link ended: {ended}, and ended ({code}) after {} ms", if ended { "ok  " } else { "FAIL" }, s.name, began.elapsed().as_millis()),
            None => {
                println!("FAIL the connection dropped: the {} said its link ended: {ended}, and exceeded the shutdown deadline (stopped here)", s.name);
                let _ = s.process.kill();
                let _ = s.process.wait();
            }
        }
        run.failed += (code.is_none() || !ended) as u32;
    }
    let began = Instant::now();
    while shown.iter().any(|s| service.linked(s.role) > 0) && began.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let left: usize = shown.iter().map(|s| service.linked(s.role)).sum();
    println!("{} the service let every link go ({left} left)", if left == 0 { "ok  " } else { "FAIL" });
    run.failed += (left != 0) as u32;
    println!("{} no process of this run had the foreground window", if run.foreground { "FAIL" } else { "ok  " });
    run.failed += run.foreground as u32;
    println!("{} publishes, {:.1} s in all", ctl.publishes(), started.elapsed().as_secs_f32());
    if has("--keep") {
        println!("kept: {}", root.display());
    } else {
        let _ = std::fs::remove_dir_all(&root);
    }
    anyhow::ensure!(run.failed == 0, "{} steps failed", run.failed);
    Ok(())
}
