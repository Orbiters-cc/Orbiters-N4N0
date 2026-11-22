//! VRChat on a Mac, with no other window than N4N0's: `obelisk-cli` (`orbiters_core::obelisk` says what it is) is
//! downloaded from the fork's GitHub releases, checked (the SHA-256 its release publishes, then each program's
//! signature: Orbiters' own team), kept in the settings' folder, and run: to sign in to Steam with a QR code, to
//! download VRChat, to start it with N4N0's arguments. A file fetched by N4N0 carries no quarantine mark, so the check
//! Gatekeeper would make at its first run is made here, before it is ever run.
use anyhow::Context as _;
use orbiters_core::launcher::EngineSetup;
use orbiters_core::obelisk::{self, Event, Phase};
use orbiters_i18n::tr;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many times in a row a QR code nobody read is replaced by a new sign-in before the set-up gives up.
const SIGN_INS: u32 = 8;
/// A game that ends this soon after it started did not start for the person (the engine, or VRChat in it, crashed).
const TOO_SHORT: u64 = obelisk::EARLY_END.as_secs();

/// A command of the program that failed: its code (`docs/CLI.md` of the fork), and a sentence for the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub words: String,
}

impl Failure {
    fn new(code: &str, words: impl Into<String>) -> Self {
        Failure { code: code.into(), words: words.into() }
    }
}

/// What the person reads for a failure: N4N0's own words for the ones they can act on, else the program's.
pub fn words(code: &str, message: &str) -> String {
    match code {
        "not_signed_in" | "sign_in_expired" => tr("Steam asks you to sign in again.").into(),
        "login_expired" => tr("Nobody approved the Steam sign-in in time.").into(),
        "not_owned" => tr("This Steam account does not have VRChat: add it to the account (it is free) and try again.").into(),
        "network" => tr("Steam could not be reached. Check the connection and try again.").into(),
        "rate_limited" => tr("Steam asks to wait a little before another sign-in.").into(),
        "keychain_unavailable" => tr("The Mac's keychain did not give the Steam sign-in.").into(),
        "already_running" | "another_game_running" | "busy" => tr("VRChat is already running.").into(),
        "cloud_conflict" | "cloud_sync_failed" => tr("Steam Cloud could not be checked, and VRChat was not started.").into(),
        _ if message.is_empty() => code.to_string(),
        _ => message.to_string(),
    }
}

/// Runs the program with `args` until it ends; `each` hears every event. Ended early when `stop` is set.
pub fn run(program: &Path, args: &[&str], stop: &AtomicBool, mut each: impl FnMut(&Event)) -> Result<(), Failure> {
    let mut child = spawn(program, args).map_err(|e| Failure::new("spawn", e.to_string()))?;
    let out = child.stdout.take().expect("piped");
    let mut failed = None;
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        let Some(event) = obelisk::event(&line) else { continue };
        if let Event::Error { code, message } = &event {
            note(args, code, message);
            failed = Some(Failure::new(code, words(code, message)));
        }
        each(&event);
    }
    let status = child.wait().map_err(|e| Failure::new("wait", e.to_string()))?;
    match failed {
        Some(failure) => Err(failure),
        None if stop.load(Ordering::Relaxed) => Err(Failure::new("cancelled", tr("Stopped."))),
        None if status.success() => Ok(()),
        None => Err(Failure::new("internal", tr("What runs VRChat on this Mac stopped without saying why."))),
    }
}

/// What the program itself said of a failure (its code and its own sentence), into Settings, Log: the person reads
/// N4N0's words in the toast, and whoever looks into it reads these.
fn note(args: &[&str], code: &str, message: &str) {
    if cfg!(test) {
        return;
    }
    let text = format!("obelisk-cli {}: {code}: {message}", args.iter().take(2).copied().collect::<Vec<_>>().join(" ")); // i18n: not shown (a program's own words, for the log)
    orbiters_core::notice_log::append(&orbiters_core::paths::logs_dir(), "detail", &text, time::OffsetDateTime::now_utc().unix_timestamp());
}

/// Runs the program with `args` for at most `limit`, each line of what it says to `each`: for short commands whose events
/// the caller reads itself (the bridge's quit). Killed past its time.
pub fn run_for(program: &Path, args: &[&str], limit: Duration, mut each: impl FnMut(&str)) -> Result<(), String> {
    let mut child = spawn(program, args).map_err(|e| e.to_string())?;
    let out = child.stdout.take().expect("piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let until = std::time::Instant::now() + limit;
    loop {
        match receiver.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
            Ok(line) => each(&line),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(tr!("It did not answer within {seconds} s.", seconds = limit.as_secs()));
            }
        }
    }
    let _ = child.wait();
    Ok(())
}

fn spawn(program: &Path, args: &[&str]) -> std::io::Result<Child> {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
}

/// Everything between the press and a VRChat ready to start: the program (when `tools` has none), the Steam sign-in
/// (when no account is signed in), the engine's beta versions (VRChat's support is in them as of November 2026), then
/// VRChat and the engine. `progress` hears each step; each step that is already done is passed.
pub fn set_up(tools: &Path, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> Result<(), Failure> {
    let program = match obelisk::program(tools) {
        Some(program) => program,
        None => {
            install(tools, progress, stop).map_err(|e| Failure::new("download", format!("{e:#}")))?;
            obelisk::program(tools).ok_or_else(|| Failure::new("download", tr("What runs VRChat on this Mac is not whole.")))?
        }
    };
    progress(EngineSetup::Checking);
    let mut account = None;
    run(&program, &["status"], stop, |event| {
        if let Event::Status { account: a } = event {
            account = a.clone();
        }
    })?;
    if account.is_none() {
        sign_in(&program, progress, stop)?;
    }
    progress(EngineSetup::Checking);
    run(&program, &["settings", "prereleases", "on"], stop, |_| {})?;
    let installed = run(&program, &["steam", "install", obelisk::STEAM_APP], stop, |event| {
        if let Event::Progress { phase, fraction } = event {
            let percent = fraction.map(|f| (f * 100.0).round() as u8);
            progress(match phase {
                Phase::Checking => EngineSetup::Checking,
                Phase::Engine => EngineSetup::Game { game: false, percent },
                Phase::Game => EngineSetup::Game { game: true, percent },
            });
        }
    });
    match installed {
        // The sign-in Steam no longer takes: once more, then the download.
        Err(failure) if matches!(failure.code.as_str(), "not_signed_in" | "sign_in_expired") => {
            sign_in(&program, progress, stop)?;
            set_up(tools, progress, stop)
        }
        other => other,
    }
}

/// The Steam sign-in by QR code: each address Steam gives is shown, a code nobody read is replaced by a new one.
fn sign_in(program: &Path, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> Result<(), Failure> {
    let mut last = Failure::new("login_expired", words("login_expired", ""));
    for _ in 0..SIGN_INS {
        let mut signed_in = false;
        let outcome = run(program, &["steam", "login", "--qr"], stop, |event| match event {
            Event::Challenge { qr_url } => progress(EngineSetup::SignIn { qr_url: qr_url.clone() }),
            Event::SignedIn { .. } => signed_in = true,
            _ => {}
        });
        match outcome {
            Ok(()) if signed_in => return Ok(()),
            Ok(()) => return Err(Failure::new("login_failed", tr("The Steam sign-in ended without an account."))),
            Err(failure) if failure.code == "login_expired" => last = failure,
            Err(failure) => return Err(failure),
        }
    }
    Err(last)
}

/// A VRChat being started, or running, by this N4N0: the program stays attached until the game ends. Home's Play and the
/// VRChat bridge start through the same one (`MacMachine::play`): one start at a time, none for [`obelisk::AFTER_EARLY_END`]
/// after a start that ended early, and each start's outcome kept by its launch id for whoever made it.
#[derive(Default)]
pub struct Play {
    now: Mutex<Now>,
}

#[derive(Default)]
struct Now {
    /// The program is still there (from the start asked for until the game ended).
    attached: bool,
    /// The game was seen running by the program, under this process when it said which.
    running: Option<Option<u32>>,
    /// Why the last start failed, until Home told it (Home's toast: once).
    failure: Option<Failure>,
    /// The program's own process: once it is gone the game is neither starting nor running by its word, whoever
    /// still holds its output open (the game's supervisor inherits it).
    program: Option<u32>,
    /// The launches made, newest last (a few kept): what each came to, read by its launch id without being used up.
    launches: Vec<Launch>,
    made: u64,
}

/// What became of one start, as the program said it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub id: u64,
    /// The engine's launcher of the game, and its Game session (from 0.1.3 the program says it; before, the engine's
    /// records are read for it).
    pub launcher: Option<u32>,
    pub session: Option<String>,
    pub running: bool,
    /// The game ended: after how many seconds, with how many frames shown.
    pub ended: Option<(Option<u64>, Option<u64>)>,
    /// Why it did not start (or ended at once).
    pub failure: Option<Failure>,
    /// The program is no longer attached to it.
    pub over: bool,
}

/// Why a start was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A start is under way already, or the game runs.
    Busy,
    /// A start ended early this long ago: Easy Anti-Cheat fails a start this soon after one.
    Wait(Duration),
    /// The program could not be run.
    Failed(String),
}

impl Refused {
    /// In the person's words.
    pub fn words(&self) -> String {
        match self {
            Refused::Busy => tr("VRChat is already starting.").into(),
            Refused::Wait(wait) => tr!("VRChat ended early a moment ago: it can be started again in {seconds} s.", seconds = wait.as_secs().max(1)),
            Refused::Failed(why) => why.clone(),
        }
    }
}

/// How many launches' outcomes are kept.
const LAUNCHES_KEPT: usize = 8;

/// Whether the process `pid` still runs (one that ended and was not yet waited for does not).
fn alive(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing());
    system.process(pid).is_some_and(|p| !matches!(p.status(), sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead))
}

impl Now {
    fn launch(&mut self, id: u64) -> Option<&mut Launch> {
        self.launches.iter_mut().find(|l| l.id == id)
    }
}

impl Play {
    /// Starts VRChat with `args` from the library `library` (whose records say when a start last ended early); the events
    /// are followed on a thread of their own. The launch's id, or why it was not made.
    pub fn start(self: &Arc<Self>, program: &Path, library: &Path, args: &[String]) -> Result<u64, Refused> {
        self.start_at(program, library, args, std::time::SystemTime::now())
    }

    pub(crate) fn start_at(self: &Arc<Self>, program: &Path, library: &Path, args: &[String], now_is: std::time::SystemTime) -> Result<u64, Refused> {
        let mut now = self.now.lock().unwrap();
        if now.attached {
            return Err(Refused::Busy);
        }
        if let Some(wait) = obelisk::start_wait(obelisk::last_early_end(library), now_is) {
            return Err(Refused::Wait(wait));
        }
        let mut command: Vec<&str> = vec!["steam", "play", obelisk::STEAM_APP];
        if !args.is_empty() {
            command.push("--");
            command.extend(args.iter().map(String::as_str));
        }
        let mut child = spawn(program, &command).map_err(|e| Refused::Failed(e.to_string()))?;
        now.made += 1;
        let id = now.made;
        now.attached = true;
        now.running = None;
        now.failure = None;
        now.program = Some(child.id());
        now.launches.push(Launch { id, ..Default::default() });
        let drop_old = now.launches.len().saturating_sub(LAUNCHES_KEPT);
        now.launches.drain(..drop_old);
        drop(now);
        let play = self.clone();
        let library = library.to_path_buf();
        let followed = std::thread::Builder::new().name("obelisk-play".into()).spawn(move || {
            let out = child.stdout.take().expect("piped");
            play.follow(id, &library, BufReader::new(out).lines().map_while(Result::ok));
            let _ = child.wait();
            play.finish(id);
        });
        if followed.is_err() {
            let mut now = self.now.lock().unwrap();
            now.attached = false;
            return Err(Refused::Failed(tr("VRChat could not be followed.").into()));
        }
        Ok(id)
    }

    /// What the program says of launch `id`, line by line.
    fn follow(&self, id: u64, library: &Path, lines: impl Iterator<Item = String>) {
        for line in lines {
            let Some(event) = obelisk::event(&line) else { continue };
            let mut now = self.now.lock().unwrap();
            match event {
                Event::Running { pid, session } => {
                    now.running = Some(pid);
                    // Before 0.1.3 the program does not name the session: the engine's record of the game does.
                    let session = session.or_else(|| {
                        pid.and_then(|pid| obelisk::session_records(library).into_iter().find(|(r, _)| r.game == Some(pid) && !r.over()).map(|(r, _)| r.id))
                    });
                    if let Some(launch) = now.launch(id) {
                        launch.launcher = pid;
                        launch.session = session;
                        launch.running = true;
                    }
                }
                Event::Exited { code, seconds, frames } => {
                    now.running = None;
                    if !cfg!(test) {
                        note(&["steam", "play"], "exited", &format!("exit code {code:?} after {seconds:?} s, {frames:?} frames shown")); // i18n: not shown (for the log)
                    }
                    let early = seconds.is_some_and(|s| s < TOO_SHORT) || frames == Some(0);
                    let failure = early.then(|| Failure::new("ended_at_once", tr("VRChat closed shortly after it started. If it crashed, press Play again.")));
                    if let Some(failure) = &failure {
                        now.failure = Some(failure.clone());
                    }
                    if let Some(launch) = now.launch(id) {
                        launch.running = false;
                        launch.ended = Some((seconds, frames));
                        if launch.failure.is_none() {
                            launch.failure = failure;
                        }
                    }
                }
                // A failure after the game ran (its session, its Cloud saves) is not a start that failed.
                Event::Error { code, message } => {
                    if !cfg!(test) {
                        note(&["steam", "play"], &code, &message);
                    }
                    let ran = now.launch(id).is_some_and(|l| l.running || l.ended.is_some());
                    if !ran {
                        let failure = Failure::new(&code, words(&code, &message));
                        now.failure = Some(failure.clone());
                        if let Some(launch) = now.launch(id) {
                            launch.failure = Some(failure);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn finish(&self, id: u64) {
        let mut now = self.now.lock().unwrap();
        now.attached = false;
        now.running = None;
        now.program = None;
        if let Some(launch) = now.launch(id) {
            launch.over = true;
            launch.running = false;
        }
    }

    /// A start is under way and the game is not up yet (the engine or an update of VRChat may download first).
    pub fn starting(&self) -> bool {
        let now = self.now.lock().unwrap();
        now.attached && now.running.is_none() && now.program.is_some_and(alive)
    }

    /// The process the program said the game runs under, while it does and the program is there to say so.
    pub fn running(&self) -> Option<Option<u32>> {
        let now = self.now.lock().unwrap();
        now.running.filter(|_| now.program.is_some_and(alive))
    }

    /// The program's own process while it is attached to a game: a real process that lives exactly as long as the game is
    /// this N4N0's to follow (what `MacMachine` names the game by when the program did not say the game's process).
    pub fn program(&self) -> Option<u32> {
        let now = self.now.lock().unwrap();
        now.program.filter(|p| alive(*p))
    }

    /// Why the last start failed: told once, to Home (the bridge reads its own launch's by [`Play::launch`]).
    pub fn failure(&self) -> Option<Failure> {
        self.now.lock().unwrap().failure.take()
    }

    /// What became of launch `id`, as often as asked.
    pub fn launch(&self, id: u64) -> Option<Launch> {
        self.now.lock().unwrap().launches.iter().find(|l| l.id == id).cloned()
    }
}

/// Gets the newest release when `tools` holds an older one (a fix of the engine comes this way): quietly, and a
/// failure changes nothing. The next start runs the newest release there is.
pub fn update(tools: &Path, stop: &AtomicBool) {
    let Some(have) = obelisk::program(tools) else { return };
    let agent = orbiters_core::http_agent(Some(Duration::from_secs(20)), Some(Duration::from_secs(60)));
    let Some(latest) = text(&agent, obelisk::RELEASES, 4 << 20).ok().and_then(|list| obelisk::latest_download(&list)) else { return };
    if have.parent().and_then(|f| f.file_name()).is_some_and(|folder| folder == latest.folder()) || tools.join(latest.folder()).exists() {
        return;
    }
    if let Err(e) = install(tools, &|_| {}, stop) {
        note(&["update"], "download", &format!("{e:#}"));
    }
}

/// Downloads the newest release into `tools`, checked and whole; `progress` hears the download.
pub fn install(tools: &Path, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> anyhow::Result<()> {
    std::fs::create_dir_all(tools)?;
    let work = tools.join(format!(".part-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = install_from(tools, &work, progress, stop);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn install_from(tools: &Path, work: &Path, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> anyhow::Result<()> {
    progress(EngineSetup::Downloading { percent: 0 });
    let agent = orbiters_core::http_agent(Some(Duration::from_secs(20)), Some(Duration::from_secs(60)));
    let download = obelisk::latest_download(&text(&agent, obelisk::RELEASES, 4 << 20)?).with_context(|| tr("No version of what runs VRChat on a Mac was found on GitHub."))?;
    let sha256 = obelisk::checksum(&text(&agent, &download.checksum_url, 4096)?).with_context(|| tr("The download does not say its checksum."))?;
    let archive = work.join(&download.name);
    let shown = std::cell::Cell::new(0u8);
    orbiters_update::net::download(
        &orbiters_update::Artifact { url: download.url.clone(), sha256, size: download.size },
        &archive,
        |have, total| {
            let percent = (have.saturating_mul(100) / total.max(1)).min(100) as u8;
            if percent != shown.replace(percent) {
                progress(EngineSetup::Downloading { percent });
            }
        },
        stop,
    )?;
    progress(EngineSetup::Installing);
    let unpacked = work.join("unpacked");
    std::fs::create_dir_all(&unpacked)?;
    anyhow::ensure!(quiet(Command::new("/usr/bin/tar").arg("-xzf").arg(&archive).arg("-C").arg(&unpacked)), "{}", tr("The download could not be unpacked."));
    let folder = unpacked.join(download.folder());
    anyhow::ensure!(genuine(&folder), "{}", tr("The download is not signed by Orbiters: nothing was installed."));
    let to: PathBuf = tools.join(download.folder());
    let _ = std::fs::remove_dir_all(&to);
    std::fs::rename(&folder, &to).with_context(|| to.display().to_string())?;
    Ok(())
}

/// A small text over HTTPS, at most `limit` bytes of it.
fn text(agent: &ureq::Agent, url: &str, limit: u64) -> anyhow::Result<String> {
    anyhow::ensure!(url.starts_with("https://"), "{url}");
    Ok(agent.get(url).header("Accept", "application/vnd.github+json, text/plain").call().with_context(|| url.to_string())?.body_mut().with_config().limit(limit).read_to_string()?)
}

fn quiet(command: &mut Command) -> bool {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Whether every program of the release in `folder` is there and carries Orbiters' signature, unchanged since.
fn genuine(folder: &Path) -> bool {
    obelisk::PROGRAMS.iter().all(|program| {
        let file = folder.join(program);
        file.is_file() && quiet(Command::new("/usr/bin/codesign").args(["--verify", "--strict", &format!("-R={}", obelisk::REQUIREMENT)]).arg(&file))
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// The real thing, from GitHub into a folder of the test's (`cargo test -p orbiters-desktop obelisk_cli -- --ignored`):
    /// the download, its checksum, the signatures, then the program itself says its status with a library of the test's.
    #[test]
    #[ignore]
    fn the_newest_release_is_downloaded_checked_and_runs() {
        let work = std::env::temp_dir().join(format!("orbiters-obelisk-cli-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let tools = obelisk::tools(&work);
        let steps = Mutex::new(Vec::new());
        install(&tools, &|step| steps.lock().unwrap().push(step), &AtomicBool::new(false)).unwrap();
        let steps = steps.into_inner().unwrap();
        assert_eq!(steps.first(), Some(&EngineSetup::Downloading { percent: 0 }));
        assert!(steps.contains(&EngineSetup::Downloading { percent: 100 }));
        assert_eq!(steps.last(), Some(&EngineSetup::Installing));
        let program = obelisk::program(&tools).expect("a whole release");
        assert!(genuine(program.parent().unwrap()));
        assert!(std::fs::read_dir(&tools).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".part-")), "nothing left of the download");
        // The program answers, signed out, in a library of its own.
        let library = work.join("library");
        let mut status = None;
        run(&program, &["--library", library.to_str().unwrap(), "status"], &AtomicBool::new(false), |event| status = Some(event.clone())).unwrap();
        assert_eq!(status, Some(Event::Status { account: None }));
        let failure = run(&program, &["--library", library.to_str().unwrap(), "steam", "play", obelisk::STEAM_APP], &AtomicBool::new(false), |_| {}).unwrap_err();
        assert_eq!(failure.code, "not_signed_in");
        // A program someone else signed is refused.
        std::fs::copy("/bin/ls", program.parent().unwrap().join("obelisk-cli")).unwrap();
        assert!(!genuine(program.parent().unwrap()));
        let _ = std::fs::remove_dir_all(&work);
    }

    /// A program that says what obelisk-cli said on 2026-11-22 (recorded), then holds on as long as `hold` seconds.
    fn fake_program(dir: &Path, lines: &[&str], hold: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("obelisk-cli");
        let body: String = lines.iter().map(|l| format!("printf '%s\\n' '{l}'\n")).collect();
        std::fs::write(&script, format!("#!/bin/sh\n{body}sleep {hold}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn wait_for(play: &Play, id: u64, done: impl Fn(&Launch) -> bool) -> Launch {
        let begin = std::time::Instant::now();
        loop {
            let launch = play.launch(id).expect("kept");
            if done(&launch) || begin.elapsed() > Duration::from_secs(20) {
                return launch;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn each_start_s_outcome_is_kept_by_its_id_for_home_and_the_bridge_alike() {
        let dir = std::env::temp_dir().join(format!("orbiters-play-trace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        // The start of 19:54: running under its launcher, then out after 100 s with no frame shown.
        let program = fake_program(
            &dir,
            &[
                r#"{"event":"preparing","app_id":438100}"#,
                r#"{"event":"starting","app_id":438100}"#,
                r#"{"event":"running","app_id":438100,"pid":83030,"session_id":"6da7c40d-9fd2-4f3b-8770-aadffa2d0088"}"#,
                r#"{"event":"exited","app_id":438100,"session_id":"6da7c40d-9fd2-4f3b-8770-aadffa2d0088","frames":0,"exit_code":0,"exit_signal":null,"seconds":100,"ended_by_request":false}"#,
            ],
            0,
        );
        let play = Arc::new(Play::default());
        let id = play.start(&program, &library, &[]).expect("starts");
        let launch = wait_for(&play, id, |l| l.over);
        assert_eq!((launch.launcher, launch.session.as_deref()), (Some(83030), Some("6da7c40d-9fd2-4f3b-8770-aadffa2d0088")));
        assert_eq!(launch.ended, Some((Some(100), Some(0))));
        assert_eq!(launch.failure.as_ref().map(|f| f.code.as_str()), Some("ended_at_once"), "no frame shown: a start that did not work");
        // Home's toast takes it once; the bridge still reads its own launch's.
        assert!(play.failure().is_some());
        assert!(play.failure().is_none(), "told once");
        assert!(play.launch(id).unwrap().failure.is_some(), "the launch's own outcome is not used up");

        // One start at a time, for whoever asks.
        let slow = fake_program(&dir.join("slow"), &[r#"{"event":"starting","app_id":438100}"#], 3);
        let first = play.start(&slow, &library, &[]).expect("starts");
        assert_eq!(play.start(&slow, &library, &[]), Err(Refused::Busy));
        wait_for(&play, first, |l| l.over);

        // An early end in the engine's records holds every start for 80 s, from the records (a restart of N4N0 keeps it).
        std::fs::create_dir_all(library.join("SteamSessions")).unwrap();
        std::fs::write(library.join("SteamSessions").join("a.v2.json"), r#"{"id":"a","store":{"app_id":438100},"phase":"Finished","played":{"seconds":100,"frames":{"count":0}}}"#).unwrap();
        let refused = play.start(&program, &library, &[]).unwrap_err();
        assert!(matches!(refused, Refused::Wait(wait) if wait > Duration::from_secs(70)), "{refused:?}");
        assert!(refused.words().contains("s."));
        let later = std::time::SystemTime::now() + Duration::from_secs(81);
        assert!(play.start_at(&program, &library, &[], later).is_ok(), "80 s on, it starts");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_reads_as_something_the_person_can_act_on() {
        assert_eq!(words("something_new", "The engine said this."), "The engine said this.");
        assert_eq!(words("something_new", ""), "something_new");
        assert_ne!(words("not_owned", "x"), "x");
    }
}
