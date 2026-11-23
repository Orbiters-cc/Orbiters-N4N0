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

/// A command of the program that failed: its code (`docs/CLI.md` of the fork), and a sentence for the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub words: String,
    /// The engine's files no game starts on, when that is why.
    pub engine: Option<obelisk::EngineProblem>,
}

impl Failure {
    fn new(code: &str, words: impl Into<String>) -> Self {
        Failure { code: code.into(), words: words.into(), engine: None }
    }

    /// What a command said as it failed.
    fn said(code: &str, message: &str, engine: Option<&obelisk::EngineProblem>) -> Self {
        // A disk that cannot be repaired in place is no download to try again: told with its path (the program's own reason
        // and its argument, `docs/CLI.md`'s `voile_failed`).
        if let Some(path) = message.split_once("backend.voile.noExchange (").and_then(|(_, rest)| rest.strip_suffix(')')) {
            let words = tr!("The disk of {path} cannot be repaired in place: it is not an APFS disk. Nothing was changed.", path = path);
            return Failure { code: "no_exchange".into(), words: words.to_string(), engine: None };
        }
        Failure { code: code.into(), words: engine.map_or_else(|| words(code, message), engine_words), engine: engine.cloned() }
    }
}

/// What the person reads for the engine's files: what ends it, as Home offers it.
pub fn engine_words(problem: &obelisk::EngineProblem) -> String {
    match problem.action {
        obelisk::EngineAction::Repair => tr("What runs VRChat on this Mac is not whole: press Repair on Home.").into(),
        obelisk::EngineAction::QuitGame => tr("An update of what runs VRChat was cut short while a game runs on it: quit that game, then press Play.").into(),
        obelisk::EngineAction::CheckStorage => tr("What runs VRChat on this Mac cannot be locked while it plays: check that its folder can be written and that its disk is not full.").into(),
        obelisk::EngineAction::Other => problem.reason.clone(),
    }
}

/// What the person reads for a failure: N4N0's own words for the ones they can act on, else the program's.
pub fn words(code: &str, message: &str) -> String {
    match code {
        "not_signed_in" | "sign_in_expired" => tr("Steam asks you to sign in again.").into(),
        "login_expired" => tr("Nobody approved the Steam sign-in in time.").into(),
        "other_account" => tr("The account VRChat was started with is no longer the one signed in: press Play to start it on this one.").into(),
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
        if let Event::Error { code, message, engine } = &event {
            note(args, code, message);
            failed = Some(Failure::said(code, message, engine.as_ref()));
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
    // A bounded queue: a program that floods its output waits for this reader, never fills memory.
    let (sender, receiver) = std::sync::mpsc::sync_channel(64);
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let until = std::time::Instant::now() + limit;
    loop {
        // One deadline for all of it, however many lines keep coming.
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            return Err(tr!("It did not answer within {seconds} s.", seconds = limit.as_secs()));
        }
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
    // Its output ended; the program itself gets what is left of the time, and only its success is one.
    let status = crate::bridge::mac::wait_until(&mut child, until.saturating_duration_since(std::time::Instant::now()).max(Duration::from_millis(100)))
        .map_err(|_| tr!("It did not answer within {seconds} s.", seconds = limit.as_secs()))?;
    if status.success() {
        Ok(())
    } else {
        Err(tr!("It ended with {status}.", status = status.to_string()))
    }
}

/// Reads `out` line by line into `each` until it ends, `stop` is set (looked at every quarter second, whoever holds the
/// pipe open meanwhile) or `each` says no more.
#[cfg(unix)]
pub(crate) fn read_lines_until(out: impl std::io::Read + std::os::unix::io::AsRawFd, stop: &AtomicBool, mut each: impl FnMut(String) -> bool) {
    let fd = out.as_raw_fd();
    let mut out = out;
    let mut pending: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd of ours, for a descriptor this function holds open.
        let ready = unsafe { libc::poll(&mut poll, 1, 250) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            continue;
        }
        let read = match out.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        pending.extend_from_slice(&buffer[..read]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            if !each(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned()) {
                return;
            }
        }
    }
    if !pending.is_empty() && !stop.load(Ordering::Relaxed) {
        each(String::from_utf8_lossy(&pending).into_owned());
    }
}

/// Elsewhere (never run there: VRChat runs through Obelisk on a Mac only), a plain reader of lines.
#[cfg(not(unix))]
pub(crate) fn read_lines_until(out: impl std::io::Read, stop: &AtomicBool, mut each: impl FnMut(String) -> bool) {
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        if stop.load(Ordering::Relaxed) || !each(line) {
            return;
        }
    }
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
        if let Event::Status { account: a, .. } = event {
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
///
/// The wait after an early end has three sources, the longest wins: the moment this run saw one end (a monotonic clock,
/// at once, before the engine's record is written), a deadline kept in a file on the boot clock (`kern.bootsessionuuid`
/// and the seconds since that boot: a restart of N4N0 keeps it, a wall clock set either way does not move it, a reboot
/// ends it), and the engine's own records (on the same boot clock when they say it, obelisk-cli 0.1.6; else by when they
/// were written). A record that cannot be read holds starts for [`obelisk::AFTER_EARLY_END`] from when it was first seen
/// unchanged (whatever it was, it did not end within that time), then is history: a damaged record of the current
/// session, which the engine itself stops at, holds starts until the person sets it aside from Home ([`Play::set_aside`]).
/// A start in flight is said in the slot's file from before it is made ([`InFlight`]): an N4N0 that dies before it could keep
/// how it ended leaves a start that counts as ended early.
#[derive(Default)]
pub struct Play {
    now: Mutex<Now>,
    /// Where the boot-relative deadline is kept (none in a test that wants none).
    gate_file: Option<PathBuf>,
    /// The one slot there is for a start on this computer, whichever N4N0 makes it.
    slot: Arc<Mutex<Slot>>,
}

/// The slot: a lock beside the gate's file, held from before a start is decided (the gate read again from its file) until how
/// its game ended is known and kept there (or, where that cannot be written, until the wait it means is over). Two coordinators
/// (two N4N0s at a cold start) never both reserve, neither creates an instance for a start the other makes, and neither decides
/// on a gate the other has since closed. The lock's own file says a start in flight ([`InFlight`]), so that a holder that dies
/// does not take with it that a game of its own may have ended early.
#[derive(Default)]
struct Slot {
    file: Option<std::fs::File>,
    /// A start or a reservation is being decided with it: nothing that ends meanwhile lets go of it.
    deciding: bool,
    reserved: Option<u64>,
    /// The launch it is held for, until how that one ended is known and kept.
    attached: Option<u64>,
    /// The wait after an early end could not be written: which such wait this is, and until when it is held for it.
    unwritten: Option<(u64, std::time::Instant)>,
    waits: u64,
    /// The start of its own it says in its file.
    mine: Option<InFlight>,
    /// The start a holder that is gone left there, not settled yet.
    left: Option<InFlight>,
}

impl Slot {
    /// Let go of where nothing holds it. What it says of a start of its own goes with it: how that one ended is known and
    /// kept, or its wait is over.
    fn let_go(&mut self) {
        if self.deciding || self.reserved.is_some() || self.attached.is_some() || self.unwritten.is_some_and(|(_, until)| until > std::time::Instant::now()) {
            return;
        }
        self.unwritten = None;
        if self.mine.take().is_some() {
            let left = self.left.take();
            let _ = self.write(left.as_ref());
        }
        self.left = None;
        self.file = None;
    }

    /// Writes `note` as what the file says, in place (nothing is made beside it).
    fn write(&mut self, note: Option<&InFlight>) -> std::io::Result<()> {
        use std::io::{Seek as _, Write as _};
        let Some(file) = &mut self.file else { return Ok(()) };
        let text = note.map(InFlight::to_json).unwrap_or_default();
        file.seek(std::io::SeekFrom::Start(0))?;
        file.write_all(text.as_bytes())?;
        file.set_len(text.len() as u64)
    }

    /// Says its own start, as it is now.
    fn say(&mut self, note: InFlight) -> std::io::Result<()> {
        self.mine = Some(note.clone());
        self.write(Some(&note))
    }
}

/// A start in flight, as the slot's file says it from before it is made until how it ended is known and kept. Found by the
/// next one to take the slot, its holder is gone: it counts as a start that ended early, unless the engine's record of it says
/// how it ended.
#[derive(Clone, Debug, Default, PartialEq)]
struct InFlight {
    /// The boot it was made on; `None` where the file says something else than a start (this one's, then).
    boot: Option<String>,
    started: f64,
    /// The Game session it became, once the program said it (as it was admitted, before its game runs), or once its record was
    /// found.
    session: Option<String>,
    /// The program that makes it, by its number and its birth: until the engine's supervisor holds the game, the start lives in it.
    program: Option<(u32, (u64, u32))>,
    /// It ended, and this is the end of the wait after it, which could not be kept.
    until: Option<f64>,
}

impl InFlight {
    fn to_json(&self) -> String {
        serde_json::json!({ "boot": self.boot, "started": self.started, "session": self.session, "program": self.program, "until": self.until }).to_string()
    }

    /// What the slot's file says. Anything but nothing at all is a start.
    fn read(text: std::io::Result<String>) -> Option<InFlight> {
        let text = text.unwrap_or_else(|_| "?".into());
        if text.trim().is_empty() {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let word = |name: &str| value.get(name).and_then(serde_json::Value::as_str).map(str::to_owned);
        let number = |name: &str| value.get(name).and_then(serde_json::Value::as_f64).filter(|n| n.is_finite());
        let program = value.get("program").and_then(|p| serde_json::from_value(p.clone()).ok());
        Some(InFlight { boot: word("boot"), started: number("started").unwrap_or_default(), session: word("session"), program, until: number("until") })
    }
}

#[derive(Default)]
struct Now {
    /// The program is still there (from the start asked for until the game ended).
    attached: bool,
    /// The game was seen running by the program, under this process when it said which.
    running: Option<Option<u32>>,
    /// The program's own process: once it is gone the game is neither starting nor running by its word, whoever
    /// still holds its output open (the game's supervisor inherits it).
    program: Option<u32>,
    /// The launches made, newest last (a few kept): what each came to, read by its launch id without being used up.
    launches: Vec<Launch>,
    made: u64,
    /// No start before this (a start seen ending early, this run).
    cooldown: Option<std::time::Instant>,
    /// The gate as kept on the boot clock: read again from its file, and added to this, before every decision, since another
    /// N4N0 may have written it.
    kept: Option<Kept>,
    /// The file could not be read the last time: it held starts for the wait after an early end once, and holds none anew
    /// until it can be read again.
    unreadable: bool,
    /// A start reserved by the bridge (its token, until when): no other start is made meanwhile, so that an instance it
    /// creates for that start is not lost to one started between.
    reserved: Option<(u64, std::time::Instant)>,
    /// Failures not told yet to whoever started them, by launch id: kept apart from the launches (a few kept), so that later
    /// starts never push them out.
    unread: Vec<(u64, Failure)>,
    /// The engine's files no game starts on, as the last start that failed for it said: shown on Home until a game runs or
    /// they were installed again, whoever that start was for.
    engine: Option<obelisk::EngineProblem>,
}

/// How many untold failures are kept.
const UNREAD_KEPT: usize = 32;

/// How long a reservation holds at most when it is neither used nor given back (the bridge's call ended some other way).
const RESERVED_FOR: Duration = Duration::from_secs(150);

/// How long the slot is asked for before another N4N0 is taken to hold it.
const SLOT_WAIT: Duration = Duration::from_millis(300);

/// What became of one start, as the program said it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub id: u64,
    /// The VRChat profile it was made on (0: the default, no `--profile`).
    pub profile: u32,
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
    /// Its failure was told to the one who started it (Home's toast: once).
    pub told: bool,
    /// Its program was lost while the engine's record said the game runs: the record is read again until it ends.
    pub unreconciled: bool,
    /// When its program was lost (a monotonic clock).
    pub lost_at: Option<std::time::Instant>,
}

/// Why a start was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A start is under way already, or the game runs.
    Busy,
    /// A start ended early this long ago: Easy Anti-Cheat fails a start this soon after one.
    Wait(Duration),
    /// A record of the engine's could not be read ([`Trouble`] says which, and what can be done).
    Records(Trouble),
    /// The program could not be run.
    Failed(String),
}

/// A record of the engine's that cannot be read, as Home and the bridge tell it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trouble {
    /// The file (or the folder, when it cannot be listed).
    pub file: PathBuf,
    /// The folder of the records cannot be listed: nothing here can mend that.
    pub unlistable: bool,
    /// It is the current session's (the engine stops at it).
    pub current: bool,
    /// Starts are held by it now.
    pub blocks: bool,
    /// How long until it is known to be history (and may be set aside); `None` once it is.
    pub settled_in: Option<Duration>,
}

impl Trouble {
    /// What a start held by it waits for: `None` when only the person can end it (setting it aside, or the folder).
    pub fn wait(&self) -> Option<Duration> {
        (!self.current && !self.unlistable).then_some(self.settled_in).flatten()
    }
}

impl Refused {
    /// In the person's words.
    pub fn words(&self) -> String {
        match self {
            Refused::Busy => tr("VRChat is already starting.").into(),
            Refused::Wait(wait) => tr!("VRChat ended early a moment ago: it can be started again in {seconds} s.", seconds = wait.as_secs().max(1)),
            Refused::Records(trouble) if trouble.unlistable => tr!("The folder of VRChat's start records cannot be read: {file}", file = trouble.file.display().to_string()),
            Refused::Records(trouble) => match trouble.wait() {
                Some(wait) => tr!("A record of an earlier VRChat start cannot be read ({file}): VRChat can be started in {seconds} s.", file = file_name(&trouble.file), seconds = wait.as_secs().max(1)),
                None => tr!("A record of an earlier VRChat start cannot be read ({file}): set it aside in Home to play.", file = file_name(&trouble.file)),
            },
            Refused::Failed(why) => why.clone(),
        }
    }
}

/// A file's own name, for the person.
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// How many launches' outcomes are kept.
const LAUNCHES_KEPT: usize = 8;
/// How long the program's output may stay open after the program itself ended (a process it started holds it): then the
/// start is over all the same.
const OUTPUT_AFTER_EXIT: Duration = Duration::from_secs(3);
/// How far ahead a kept deadline may be: one further is not a deadline any start wrote, and is taken as one just written.
const KEPT_AHEAD: f64 = obelisk::AFTER_EARLY_END.as_secs_f64() + 1.0;

/// Whether the process `pid` still runs (one that ended and was not yet waited for does not).
fn alive(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing());
    system.process(pid).is_some_and(|p| !matches!(p.status(), sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead))
}

/// Whether the process `pid` born at `birth` is there.
fn there((pid, birth): (u32, (u64, u32))) -> bool {
    #[cfg(target_os = "macos")]
    return super::bridge_mac::born(pid) == Some(birth);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = birth;
        alive(pid)
    }
}

/// The process `pid` as one that can be told later: its number and its birth, where that can be read.
fn named(pid: u32) -> Option<(u32, (u64, u32))> {
    #[cfg(target_os = "macos")]
    return super::bridge_mac::born(pid).map(|birth| (pid, birth));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// Whether the supervisor or the game `record` names, as it names it (its number and its birth), is there.
fn record_lives(record: &obelisk::SessionRecord) -> bool {
    record.supervisor.into_iter().chain(record.game.zip(record.game_birth)).any(there)
}

/// The VRChat session of the engine's that is not over and whose supervisor or game is there, by its record: whoever started it.
fn live_session(library: &Path) -> Option<String> {
    obelisk::session_records(library).into_iter().map(|(record, _)| record).find(|record| !record.over() && record_lives(record)).map(|record| record.id)
}

/// Why the slot's file could not be used: which one it is, and what ends that. It is never to be removed.
fn slot_refused(path: &Path, e: &std::io::Error) -> Refused {
    use std::io::ErrorKind;
    let file = path.display().to_string();
    Refused::Failed(match e.kind() {
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => tr!("{file} cannot be written, and it keeps two VRChats from starting at once: give yourself permission to write it, then press Play again.", file = file),
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => tr!("{file} cannot be written: the disk is full. Free some space, then press Play again.", file = file),
        _ => tr!("{file} cannot be written ({reason}): check its permissions and the free space on its disk, then press Play again.", file = file, reason = e.to_string()),
    })
}

/// Whether a process of the engine runs from `library` (its Wine, its servers, the game in it): any process whose
/// program or arguments are under that folder.
fn engine_runs(library: &Path) -> bool {
    let text = library.to_string_lossy();
    let mut system = sysinfo::System::new();
    let kind = sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always).with_cmd(sysinfo::UpdateKind::Always);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, kind);
    system.processes().values().any(|p| p.exe().is_some_and(|e| e.starts_with(library)) || p.cmd().iter().any(|a| a.to_string_lossy().contains(&*text)))
}

/// The clock since this Mac booted, sleep included: this boot's session UUID and the seconds since it. Where the boot's
/// UUID cannot be read, this process's own clock (kept for this run only: its deadline ends with the run).
pub(crate) fn boot_clock() -> obelisk::BootInstant {
    static PROCESS: std::sync::OnceLock<(String, std::time::Instant)> = std::sync::OnceLock::new();
    #[cfg(target_os = "macos")]
    {
        let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        let mut uuid = [0u8; 64];
        let mut len = uuid.len();
        // SAFETY: plain queries into buffers of ours, their sizes passed. CLOCK_MONOTONIC on macOS counts while asleep.
        let (clock, boot) = unsafe {
            (
                libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time),
                libc::sysctlbyname(c"kern.bootsessionuuid".as_ptr(), uuid.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0),
            )
        };
        let boot = std::str::from_utf8(&uuid[..len.min(uuid.len())]).ok().map(|s| s.trim_end_matches('\0').to_ascii_uppercase()).filter(|_| boot == 0);
        if let (0, Some(boot)) = (clock, boot.filter(|b| b.len() == 36)) {
            return obelisk::BootInstant { boot, seconds: time.tv_sec as f64 + time.tv_nsec as f64 / 1e9 };
        }
    }
    let (boot, at) = PROCESS.get_or_init(|| (format!("process-{}-{}", std::process::id(), time::OffsetDateTime::now_utc().unix_timestamp_nanos()), std::time::Instant::now()));
    obelisk::BootInstant { boot: boot.clone(), seconds: at.elapsed().as_secs_f64() + 1.0 }
}

/// The gate as kept on the boot clock: the deadline after the last early end this N4N0 saw, and when each record that
/// could not be read was first seen as it is.
#[derive(Clone, Debug, Default, PartialEq)]
struct Kept {
    boot: String,
    until: Option<f64>,
    /// A damaged record's key ([`damage_key`]) and when it was first seen so, on this boot.
    seen: Vec<(String, f64)>,
}

impl Kept {
    /// Adds what `other`, the same boot's, holds: the later deadline, the earlier first sight.
    fn add(&mut self, other: Kept) {
        if self.boot != other.boot {
            return;
        }
        self.until = [self.until, other.until].into_iter().flatten().reduce(f64::max);
        for (key, at) in other.seen {
            match self.seen.iter_mut().find(|(known, _)| *known == key) {
                Some((_, first)) => *first = first.min(at),
                None => self.seen.push((key, at)),
            }
        }
    }

    fn to_json(&self) -> String {
        let seen: serde_json::Map<String, serde_json::Value> = self.seen.iter().map(|(k, at)| (k.clone(), serde_json::json!(at))).collect();
        serde_json::json!({ "version": 2, "boot": self.boot, "until": self.until, "seen": seen }).to_string()
    }
}

/// Reads the kept gate at `clock`: a file of another boot holds nothing; a file that cannot be read or is not one is no
/// proof of none, and holds starts for the wait after an early end from now, once (it is written again as it is now).
/// The returned flag says the file must be written again.
fn read_kept(text: Option<Result<String, ()>>, clock: &obelisk::BootInstant) -> (Kept, bool) {
    let fresh = Kept { boot: clock.boot.clone(), ..Default::default() };
    let uncertain = || (Kept { until: Some(clock.seconds + obelisk::AFTER_EARLY_END.as_secs_f64()), ..fresh.clone() }, true);
    let text = match text {
        None => return (fresh, false),
        Some(Err(())) => return uncertain(),
        Some(Ok(text)) => text,
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return uncertain() };
    match value.get("boot") {
        // Before 0.3.9-dev.24 the boot was told by its time (`{"boot":<Unix s>,"until":<s since boot>}`): kept while that
        // same boot's deadline is still ahead, else nothing.
        Some(serde_json::Value::Number(booted)) => {
            let until = value.get("until").and_then(serde_json::Value::as_f64);
            let same = booted.as_u64().is_some_and(|b| b.abs_diff(sysinfo::System::boot_time()) <= 2);
            let until = until.filter(|u| same && *u > clock.seconds);
            (Kept { until, ..fresh }, true)
        }
        Some(serde_json::Value::String(boot)) if value.get("version").and_then(serde_json::Value::as_u64) == Some(2) && (boot.len() == 36 || boot.starts_with("process-")) => {
            let until = match value.get("until") {
                Some(serde_json::Value::Null) | None => None,
                Some(u) => match u.as_f64().filter(|u| u.is_finite()) {
                    Some(u) => Some(u),
                    None => return uncertain(),
                },
            };
            let Some(seen) = value.get("seen").and_then(serde_json::Value::as_object) else { return uncertain() };
            let seen: Option<Vec<(String, f64)>> = seen.iter().map(|(k, v)| Some((k.clone(), v.as_f64().filter(|v| v.is_finite())?))).collect();
            let Some(seen) = seen else { return uncertain() };
            if !boot.eq_ignore_ascii_case(&clock.boot) {
                return (fresh, true);
            }
            (Kept { boot: clock.boot.clone(), until, seen }, false)
        }
        _ => uncertain(),
    }
}

/// What tells one damaged record as it is now from itself after any change.
fn damage_key(damaged: &obelisk::Damaged) -> String {
    match damaged.identity {
        Some((dev, ino, len, changed)) => format!("{}|{dev}|{ino}|{len}|{changed}", damaged.path.display()),
        None => format!("{}|unreadable", damaged.path.display()),
    }
}

/// The gate at `clock` (and the wall clock `wall`, for older records), from the records' `scan`, the kept gate (updated
/// in place: the flag says to write it) and this run's own wait `memory`. `Ok(None)`: a start may be made now. Also the
/// trouble to show in Home, if any, blocking or not.
fn judge(scan: &Result<obelisk::RecordScan, obelisk::Unlistable>, kept: &mut Kept, clock: &obelisk::BootInstant, wall: std::time::SystemTime, memory: Option<Duration>) -> (Result<Option<Duration>, Refused>, Option<Trouble>, bool) {
    let after = obelisk::AFTER_EARLY_END.as_secs_f64();
    let mut write = false;
    if kept.boot != clock.boot {
        *kept = Kept { boot: clock.boot.clone(), ..Default::default() };
        write = true;
    }
    // A deadline further ahead than any start writes one: as if written now, once.
    if kept.until.is_some_and(|u| u - clock.seconds > KEPT_AHEAD) {
        kept.until = Some(clock.seconds + after);
        write = true;
    }
    let left = |until: f64| (until > clock.seconds).then(|| Duration::from_secs_f64((until - clock.seconds).min(after)));
    let mut waits: Vec<Duration> = memory.into_iter().chain(kept.until.and_then(left)).collect();
    let scan = match scan {
        Ok(scan) => scan,
        Err(obelisk::Unlistable(folder)) => {
            let trouble = Trouble { file: folder.clone(), unlistable: true, current: false, blocks: true, settled_in: None };
            return (Err(Refused::Records(trouble.clone())), Some(trouble), write);
        }
    };
    for (written, ended) in &scan.early_ends {
        match ended {
            Some(ended) if ended.boot.eq_ignore_ascii_case(&clock.boot) => waits.extend(left(ended.seconds + after)),
            // Ended on another boot: a reboot is past any wait.
            Some(_) => {}
            None => waits.extend(obelisk::start_wait(Some(*written), wall)),
        }
    }
    // Each damaged record: first seen so now, or when the kept gate says; the ones no longer there are forgotten.
    let keys: Vec<String> = scan.damaged.iter().map(damage_key).collect();
    let before = kept.seen.len();
    kept.seen.retain(|(k, _)| keys.contains(k));
    write |= kept.seen.len() != before;
    let mut trouble: Option<Trouble> = None;
    let mut blocking: Option<Trouble> = None;
    for (damaged, key) in scan.damaged.iter().zip(&keys) {
        let first = match kept.seen.iter().find(|(k, _)| k == key) {
            Some((_, first)) => (*first).min(clock.seconds),
            None => {
                kept.seen.push((key.clone(), clock.seconds));
                write = true;
                clock.seconds
            }
        };
        let settled_in = left(first + after);
        let blocks = damaged.current || settled_in.is_some();
        let this = Trouble { file: damaged.path.clone(), unlistable: false, current: damaged.current, blocks, settled_in };
        // The current session's first, then one that holds starts, then any.
        let rank = |t: &Trouble| (t.current, t.blocks, t.settled_in);
        if trouble.as_ref().is_none_or(|t| rank(&this) > rank(t)) {
            trouble = Some(this.clone());
        }
        if blocks && blocking.as_ref().is_none_or(|t| rank(&this) > rank(t)) {
            blocking = Some(this);
        }
    }
    let wait = waits.into_iter().max().filter(|w| !w.is_zero());
    let verdict = match blocking {
        Some(t) if t.current || t.settled_in >= wait => Err(Refused::Records(t)),
        _ => Ok(wait),
    };
    (verdict, trouble, write)
}

impl Now {
    fn launch(&mut self, id: u64) -> Option<&mut Launch> {
        self.launches.iter_mut().find(|l| l.id == id)
    }
}

impl Play {
    /// The start shared by Home and the bridge, keeping its boot-relative deadline in `gate_file`.
    pub fn new(gate_file: PathBuf) -> Self {
        Play { now: Mutex::default(), gate_file: Some(gate_file), slot: Arc::default() }
    }

    /// Takes the slot, or finds it this one's already: refused while another N4N0 holds it.
    fn take_slot(&self) -> Result<(), Refused> {
        self.take_slot_within(SLOT_WAIT)
    }

    /// The launch the slot is held for: one under way, or whose end is not known yet.
    fn owner(&self) -> Option<u64> {
        self.slot.lock().unwrap().attached
    }

    /// Holds the slot until `until`, for a wait that could not be written: whoever cannot read it starts nothing meanwhile.
    /// Only this wait ends then, whatever holds the slot by then.
    fn hold_slot(&self, until: std::time::Instant) {
        let deciding = self.slot.lock().unwrap().deciding;
        let _ = self.take_slot_within(Duration::ZERO);
        let wait = {
            let mut slot = self.slot.lock().unwrap();
            slot.deciding = deciding;
            slot.waits += 1;
            slot.unwritten = Some((slot.waits, until));
            slot.waits
        };
        let slot = Arc::downgrade(&self.slot);
        let _ = std::thread::Builder::new().name("obelisk-unwritten".into()).spawn(move || {
            std::thread::sleep(until.saturating_duration_since(std::time::Instant::now()));
            let Some(slot) = slot.upgrade() else { return };
            let mut slot = slot.lock().unwrap();
            if slot.unwritten.is_some_and(|(this, _)| this == wait) {
                slot.unwritten = None;
                slot.let_go();
            }
        });
    }

    /// What was decided holds the slot no more: let go of where nothing else does (no reservation, no launch, and no wait that
    /// could not be written).
    fn let_slot_go(&self) {
        let mut slot = self.slot.lock().unwrap();
        slot.deciding = false;
        slot.let_go();
    }

    fn take_slot_within(&self, wait: Duration) -> Result<(), Refused> {
        let Some(gate) = &self.gate_file else { return Ok(()) };
        let mut slot = self.slot.lock().unwrap();
        if slot.file.is_some() {
            slot.deciding = true;
            return Ok(());
        }
        let path = gate.with_extension("lock");
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(|e| slot_refused(&path, &e))?;
        // A program being started may hold a copy of a lock just let go of, for the moment before it runs.
        let until = std::time::Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
                Err(std::fs::TryLockError::WouldBlock) => return Err(Refused::Busy),
                Err(std::fs::TryLockError::Error(e)) => return Err(slot_refused(&path, &e)),
            }
        }
        let mut said = String::new();
        slot.left = InFlight::read(std::io::Read::read_to_string(&mut &file, &mut said).map(|_| said));
        slot.file = Some(file);
        slot.deciding = true;
        Ok(())
    }

    /// Settles the start a holder of the slot that is gone left in it: the engine's record says how it ended, or it counts as
    /// one that ended early, at the latest now. One that still lives stays there until it does not, whatever any wait says: its
    /// session's supervisor or game, the program that makes it, or (where the holder was gone before it could say which session it
    /// became) the one VRChat session of the engine's that lives.
    fn settle_left(&self, now: &mut Now, library: &Path, clock: &obelisk::BootInstant) {
        let left = {
            let slot = self.slot.lock().unwrap();
            let Some(left) = slot.left.clone().filter(|_| slot.mine.is_none()) else { return };
            left
        };
        let other_boot = left.boot.as_ref().is_some_and(|boot| !boot.eq_ignore_ascii_case(&clock.boot));
        if !other_boot && left.session.is_none() {
            if let Some(session) = live_session(library) {
                let mut slot = self.slot.lock().unwrap();
                let known = InFlight { session: Some(session), ..left };
                let _ = slot.write(Some(&known));
                slot.left = Some(known);
                return;
            }
        }
        let until = if other_boot {
            None
        } else if left.until.is_some() {
            left.until
        } else {
            match left.session.as_deref().map(|session| obelisk::session_record_of(library, session)) {
                // The records hold starts themselves where it ended early.
                Some(Ok(Some(record))) if record.over() => None,
                Some(Ok(Some(record))) if record_lives(&record) => return,
                // Not held by the engine's supervisor (yet, by what can be read): it lives in the program that makes it.
                None | Some(Ok(None) | Err(_)) if left.program.is_some_and(there) => return,
                _ => Some(clock.seconds + obelisk::AFTER_EARLY_END.as_secs_f64()),
            }
        };
        let kept = now.kept.as_mut().expect("read before");
        kept.until = [kept.until, until].into_iter().flatten().reduce(f64::max);
        let settled = until.is_none() || self.write_kept(kept);
        let mut slot = self.slot.lock().unwrap();
        slot.left = None;
        let still = (!settled).then_some(InFlight { until, ..left });
        let _ = slot.write(still.as_ref());
    }

    /// Takes the slot for the reservation `token`, which lets go of it at the latest when it expires, used or not.
    fn reserve_slot(&self, token: u64) -> Result<(), Refused> {
        self.take_slot()?;
        {
            let mut slot = self.slot.lock().unwrap();
            slot.reserved = Some(token);
            slot.deciding = false;
        }
        let slot = Arc::downgrade(&self.slot);
        let _ = std::thread::Builder::new().name("obelisk-reserved".into()).spawn(move || {
            std::thread::sleep(RESERVED_FOR);
            if let Some(slot) = slot.upgrade() {
                Self::unreserve_slot(&slot, token);
            }
        });
        Ok(())
    }

    fn unreserve_slot(slot: &Mutex<Slot>, token: u64) {
        let mut slot = slot.lock().unwrap();
        if slot.reserved == Some(token) {
            slot.reserved = None;
            slot.let_go();
        }
    }

    /// Launch `id` is over: the slot is free, unless it is reserved.
    fn free_slot(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap();
        if slot.attached == Some(id) {
            slot.attached = None;
            slot.let_go();
        }
    }

    /// Starts VRChat with `args` from the library `library` (whose records say when a start last ended early); the events
    /// are followed on a thread of their own. The launch's id, or why it was not made.
    #[cfg(test)]
    pub fn start(self: &Arc<Self>, program: &Path, library: &Path, args: &[String]) -> Result<u64, Refused> {
        self.start_at(program, library, args, std::time::SystemTime::now())
    }

    /// Home's start: as [`Play::start`], drawn at `render_percent` (the engine's percent of the display's drawing
    /// resolution, for this launch only) when the program takes it; an older program starts the game at its own size.
    /// Started for the Steam account `steam`, or not at all, when one is named: the program checks it against the very session
    /// it starts with.
    pub fn start_rendered(self: &Arc<Self>, program: &Path, library: &Path, args: &[String], render_percent: Option<u32>, steam: Option<u64>) -> Result<u64, Refused> {
        self.start_full(program, library, args, std::time::SystemTime::now(), None, render_percent.filter(|_| obelisk::takes_render_percent(program)), steam)
    }

    /// Reads the kept gate again from its file, adding it to what this run knows; whether it is to be written again.
    fn reload(&self, now: &mut Now, clock: &obelisk::BootInstant) -> bool {
        let text = self.gate_file.as_ref().and_then(|file| match std::fs::read_to_string(file) {
            Ok(text) => Some(Ok(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(Err(())),
        });
        let (mut read, write) = read_kept(text, clock);
        // A file that is no gate holds starts once, from when it was first found so.
        let unreadable = write && read.until == Some(clock.seconds + obelisk::AFTER_EARLY_END.as_secs_f64());
        if unreadable && now.unreadable {
            read = Kept { boot: clock.boot.clone(), ..Default::default() };
        }
        now.unreadable = unreadable;
        if let Some(known) = now.kept.take() {
            read.add(known);
        }
        now.kept = Some(read);
        write
    }

    /// How long a start must still wait (the longest of the three sources), or why it cannot be made; and the trouble with
    /// the records to show, blocking or not. The gate is read again from its file, and written only while the slot is held
    /// (by whoever decides a start here, or for this alone when nobody else holds it).
    fn judged(&self, now: &mut Now, library: &Path, wall: std::time::SystemTime) -> (Result<Option<Duration>, Refused>, Option<Trouble>) {
        self.reconcile(now, library);
        let held = self.take_slot_within(Duration::ZERO).is_ok();
        let clock = boot_clock();
        let memory = now.cooldown.and_then(|until| until.checked_duration_since(std::time::Instant::now()));
        let rewrite = self.reload(now, &clock);
        if held {
            self.settle_left(now, library, &clock);
        }
        let kept = now.kept.as_mut().expect("read above");
        let (verdict, trouble, changed) = judge(&obelisk::scan_records(library), kept, &clock, wall, memory);
        let unwritten = self.slot.lock().unwrap().unwritten.is_some();
        if held && (rewrite || changed || unwritten) {
            self.write_kept(kept);
        }
        (verdict, trouble)
    }

    /// Writes the kept gate whole (a part, then renamed over): whether it is. A failure keeps it in this run's memory all the
    /// same.
    fn write_kept(&self, kept: &Kept) -> bool {
        let Some(file) = &self.gate_file else { return true };
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let part = file.with_extension("part");
        if std::fs::write(&part, kept.to_json()).is_err() {
            return false;
        }
        if std::fs::rename(&part, file).is_err() {
            let _ = std::fs::remove_file(&part);
            return false;
        }
        self.slot.lock().unwrap().unwritten = None;
        true
    }

    /// How long a start must still wait, decided under the slot: it is held when this returns `Ok(None)`, for the start or the
    /// reservation that follows, and otherwise as it was.
    fn admitted(&self, now: &mut Now, library: &Path, wall: std::time::SystemTime) -> Result<(), Refused> {
        self.reconcile(now, library);
        self.take_slot()?;
        // Held for a launch whose end is not known yet, this run's own too.
        if self.owner().is_some() {
            self.let_slot_go();
            return Err(Refused::Busy);
        }
        let verdict = match self.judged(now, library, wall).0 {
            // A game a holder that is gone left there still runs.
            Ok(None) if self.slot.lock().unwrap().left.is_some() => Refused::Busy,
            // Nor over any that lives by the engine's own records, whatever was left of it here.
            Ok(None) if live_session(library).is_some() => Refused::Busy,
            Ok(None) => return Ok(()),
            Ok(Some(wait)) => Refused::Wait(wait),
            Err(refused) => refused,
        };
        self.let_slot_go();
        Err(verdict)
    }

    /// How long a start must still wait, for whoever only asks.
    fn wait(&self, now: &mut Now, library: &Path, wall: std::time::SystemTime) -> Result<Option<Duration>, Refused> {
        let verdict = self.judged(now, library, wall).0;
        self.let_slot_go();
        verdict
    }

    /// Reserves the next start for whoever holds the token returned (the bridge, before it creates an instance for it):
    /// refused like a start would be now, except for a game of the reserver's own still attached when `own_attached` is
    /// that launch and the slot is still held for it (it is to be closed first, its end and any wait after it are waited for).
    pub fn reserve(&self, library: &Path, own_attached: Option<u64>) -> Result<u64, Refused> {
        let mut now = self.now.lock().unwrap();
        if now.reserved.is_some_and(|(_, until)| until > std::time::Instant::now()) {
            return Err(Refused::Busy);
        }
        self.reconcile(&mut now, library);
        let attached = now.attached.then(|| now.launches.last().map(|l| l.id)).flatten();
        if now.attached && (own_attached.is_none() || attached != own_attached) {
            return Err(Refused::Busy);
        }
        // Only for the game the slot is held for now: one that has ended since it was looked at leaves a start like any other.
        if own_attached.is_none() || self.owner() != own_attached {
            self.admitted(&mut now, library, std::time::SystemTime::now())?;
        }
        now.made += 1;
        let token = now.made;
        self.reserve_slot(token)?;
        now.reserved = Some((token, std::time::Instant::now() + RESERVED_FOR));
        Ok(token)
    }

    /// Gives a reservation back unused.
    pub fn unreserve(&self, token: u64) {
        let mut now = self.now.lock().unwrap();
        if now.reserved.is_some_and(|(t, _)| t == token) {
            now.reserved = None;
        }
        Self::unreserve_slot(&self.slot, token);
    }

    /// Starts as [`Play::start`] does, with the reservation `token` (used up by it); without one, a start reserved by
    /// someone else is refused.
    /// The bridge's start: at 100 % always, said explicitly (fourth verification, finding 10: without the option the
    /// engine applies the installation's saved percentage, while the bridge measures its coordinates at 100 %).
    pub fn start_reserved(self: &Arc<Self>, program: &Path, library: &Path, args: &[String], token: Option<u64>) -> Result<u64, Refused> {
        let full = obelisk::takes_render_percent(program).then_some(100);
        self.start_full(program, library, args, std::time::SystemTime::now(), token, full, None)
    }

    #[cfg(test)]
    pub(crate) fn start_at(self: &Arc<Self>, program: &Path, library: &Path, args: &[String], now_is: std::time::SystemTime) -> Result<u64, Refused> {
        self.start_with(program, library, args, now_is, None)
    }

    fn start_with(self: &Arc<Self>, program: &Path, library: &Path, args: &[String], now_is: std::time::SystemTime, token: Option<u64>) -> Result<u64, Refused> {
        self.start_full(program, library, args, now_is, token, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn start_full(self: &Arc<Self>, program: &Path, library: &Path, args: &[String], now_is: std::time::SystemTime, token: Option<u64>, render_percent: Option<u32>, steam: Option<u64>) -> Result<u64, Refused> {
        let mut now = self.now.lock().unwrap();
        if now.attached {
            return Err(Refused::Busy);
        }
        match now.reserved {
            Some((held, until)) if until > std::time::Instant::now() && Some(held) != token => return Err(Refused::Busy),
            _ => {}
        }
        // Used up here, whatever comes of it.
        let held = now.reserved.take();
        let unused = |refused| {
            if let Some((token, _)) = held {
                Self::unreserve_slot(&self.slot, token);
            }
            refused
        };
        self.admitted(&mut now, library, now_is).map_err(unused)?;
        let mut command: Vec<&str> = vec!["steam", "play", obelisk::STEAM_APP];
        let steam = steam.map(|id| id.to_string());
        if let Some(steam) = &steam {
            command.extend(["--steam-id", steam.as_str()]);
        }
        let percent = render_percent.map(|p| p.clamp(1, 100).to_string());
        if let Some(percent) = &percent {
            command.extend(["--render-percent", percent.as_str()]);
        }
        if !args.is_empty() {
            command.push("--");
            command.extend(args.iter().map(String::as_str));
        }
        // Kept from here: a start that is not made lets go of it.
        let free = || {
            let mut slot = self.slot.lock().unwrap();
            slot.reserved = None;
            slot.deciding = false;
            slot.let_go();
        };
        // Said before it is made: this N4N0 may be gone before it can say how it ended.
        let clock = boot_clock();
        let said = self.slot.lock().unwrap().say(InFlight { boot: Some(clock.boot), started: clock.seconds, ..Default::default() });
        said.map_err(|e| {
            free();
            slot_refused(&self.gate_file.as_deref().unwrap_or(Path::new("")).with_extension("lock"), &e)
        })?;
        let mut child = spawn(program, &command).map_err(|e| {
            free();
            Refused::Failed(e.to_string())
        })?;
        let Some(out) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            free();
            return Err(Refused::Failed(tr("VRChat could not be followed.").into()));
        };
        now.made += 1;
        let id = now.made;
        {
            // The reservation is used up by its start.
            let mut slot = self.slot.lock().unwrap();
            slot.reserved = None;
            slot.attached = Some(id);
            slot.deciding = false;
            // The start lives in this process until the engine's supervisor holds it.
            if let Some(mine) = slot.mine.clone() {
                let _ = slot.say(InFlight { program: named(child.id()), ..mine });
            }
        }
        now.attached = true;
        now.running = None;
        now.program = Some(child.id());
        now.launches.push(Launch { id, profile: orbiters_core::vrchat_launch::profile_of(args), ..Default::default() });
        let drop_old = now.launches.len().saturating_sub(LAUNCHES_KEPT);
        now.launches.drain(..drop_old);
        drop(now);
        // One owner for the whole of this launch: it reads the program's output through a reader it can end, waits for the
        // program itself, and ends the launch once the program is gone (its output given a moment for its last lines),
        // whoever else still holds that output open (the game's supervisor inherits it).
        let play = self.clone();
        let library_for = library.to_path_buf();
        let owner = std::thread::Builder::new().name("obelisk-play".into()).spawn(move || play.own(id, &library_for, child, out));
        if owner.is_err() {
            // The child moved into the closure that could not run: it is dropped with it (its pipe closed); the start is
            // as never made, and the program, without a reader, is ended by the next look of the engine's records.
            self.abandon(id);
            return Err(Refused::Failed(tr("VRChat could not be followed.").into()));
        }
        Ok(id)
    }

    /// The owner of launch `id`: its program `child` and its output `out`, to the end.
    fn own(&self, id: u64, library: &Path, mut child: Child, out: std::process::ChildStdout) {
        let stop = Arc::new(AtomicBool::new(false));
        let (send, lines) = std::sync::mpsc::channel::<String>();
        let reading = stop.clone();
        let reader = std::thread::Builder::new().name("obelisk-play-out".into()).spawn(move || read_lines_until(out, &reading, |line| send.send(line).is_ok()));
        if reader.is_err() {
            let _ = child.kill();
        }
        let mut status = None;
        let mut exited_at: Option<std::time::Instant> = None;
        loop {
            match lines.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => self.follow_line(id, library, &line),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) if exited_at.is_some() => break,
                Err(_) => {}
            }
            if exited_at.is_none() {
                match child.try_wait() {
                    Ok(Some(s)) => {
                        status = Some(s);
                        exited_at = Some(std::time::Instant::now());
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // It cannot be waited for: ended, and reaped.
                        let _ = child.kill();
                        status = child.wait().ok();
                        exited_at = Some(std::time::Instant::now());
                    }
                }
            }
            // The program is gone: its last lines a moment, then the launch is over, whoever holds its output.
            if exited_at.is_some_and(|at| at.elapsed() >= OUTPUT_AFTER_EXIT) {
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
        while let Ok(line) = lines.try_recv() {
            self.follow_line(id, library, &line);
        }
        self.finish(id, library, status);
    }

    /// How long a start must still wait now, or why none can be made.
    pub fn gate(&self, library: &Path) -> Result<Option<Duration>, Refused> {
        let mut now = self.now.lock().unwrap();
        self.wait(&mut now, library, std::time::SystemTime::now())
    }

    /// A record of the engine's that cannot be read, for Home to show (whether or not it holds starts now).
    pub fn trouble(&self, library: &Path) -> Option<Trouble> {
        let mut now = self.now.lock().unwrap();
        let trouble = self.judged(&mut now, library, std::time::SystemTime::now()).1;
        self.let_slot_go();
        trouble
    }

    /// Sets the damaged record `file` aside, out of the engine's records (into `Set aside` in the library, never deleted):
    /// only one listed as damaged now, unchanged since it was first seen so for the whole wait after an early end, with no
    /// game of this N4N0 attached, no supervisor of the engine holding its records (its lock is held for the move) and no
    /// process of the engine running. Where it went, or why it was left.
    pub fn set_aside(&self, library: &Path, file: &Path) -> Result<PathBuf, String> {
        let mut now = self.now.lock().unwrap();
        if now.attached {
            return Err(tr("VRChat is starting or running: set the record aside once it has closed.").into());
        }
        // Decided and kept under the slot, as a start is: another N4N0 starts nothing meanwhile.
        if self.take_slot().is_err() {
            return Err(tr("VRChat is starting or running: set the record aside once it has closed.").into());
        }
        let aside = self.set_aside_here(&mut now, library, file);
        self.let_slot_go();
        aside
    }

    /// The same, with the slot held.
    fn set_aside_here(&self, now: &mut Now, library: &Path, file: &Path) -> Result<PathBuf, String> {
        let _ = self.judged(now, library, std::time::SystemTime::now());
        let scan = obelisk::scan_records(library).map_err(|obelisk::Unlistable(folder)| tr!("The folder of VRChat's start records cannot be read: {file}", file = folder.display().to_string()))?;
        let Some(damaged) = scan.damaged.iter().find(|d| d.path == file) else {
            return Err(tr("That record can be read now, or is gone: nothing was set aside.").into());
        };
        let key = damage_key(damaged);
        let clock = boot_clock();
        let first = now.kept.as_ref().and_then(|k| k.seen.iter().find(|(k, _)| *k == key).map(|(_, at)| *at)).unwrap_or(clock.seconds);
        let settled = clock.seconds - first >= obelisk::AFTER_EARLY_END.as_secs_f64();
        if !settled {
            let wait = (obelisk::AFTER_EARLY_END.as_secs_f64() - (clock.seconds - first)).max(1.0).ceil() as u64;
            return Err(tr!("The record was seen damaged only a moment ago: it can be set aside in {seconds} s.", seconds = wait));
        }
        let sessions = obelisk::sessions_dir(library);
        // The engine's own lock on its records: held for the move, so that no supervisor starts meanwhile.
        let lock = match std::fs::File::open(sessions.join("owner.lock")) {
            Ok(lock) => Some(lock),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(tr("VRChat's engine could not be checked: nothing was set aside.").into()),
        };
        if lock.as_ref().is_some_and(|l| l.try_lock().is_err()) {
            return Err(tr("VRChat's engine is running: set the record aside once it has closed.").into());
        }
        if engine_runs(library) {
            return Err(tr("VRChat's engine is running: set the record aside once it has closed.").into());
        }
        if obelisk::file_identity(file) != damaged.identity {
            return Err(tr("The record changed meanwhile: nothing was set aside. Look again in a moment.").into());
        }
        let stamp = time::OffsetDateTime::now_utc().unix_timestamp();
        let aside = library.join("Set aside").join(format!("{}.{stamp}.set-aside", file_name(file)));
        std::fs::create_dir_all(aside.parent().expect("a folder")).and_then(|()| std::fs::rename(file, &aside)).map_err(|e| tr!("The record could not be moved: {reason}", reason = e.to_string()))?;
        drop(lock);
        // It is gone from the records: forgotten by the gate.
        if let Some(kept) = now.kept.as_mut() {
            kept.seen.retain(|(k, _)| *k != key);
            let kept = kept.clone();
            self.write_kept(&kept);
        }
        Ok(aside)
    }

    /// The launch the program is attached to now, if one is (a start under way, a game running, or its end finalizing).
    pub fn attached(&self) -> Option<u64> {
        let now = self.now.lock().unwrap();
        now.attached.then(|| now.launches.last().map(|l| l.id)).flatten()
    }

    /// A start that could not be followed: as if it was never made.
    fn abandon(&self, id: u64) {
        self.free_slot(id);
        let mut now = self.now.lock().unwrap();
        now.attached = false;
        now.program = None;
        now.running = None;
        now.launches.retain(|l| l.id != id);
    }

    /// Notes that a start ended early: no start for [`obelisk::AFTER_EARLY_END`], at once and across a restart.
    fn cool_down(&self, now: &mut Now) {
        let cooldown = std::time::Instant::now() + obelisk::AFTER_EARLY_END;
        now.cooldown = Some(cooldown);
        let clock = boot_clock();
        self.reload(now, &clock);
        let kept = now.kept.as_mut().expect("read above");
        kept.until = Some(clock.seconds + obelisk::AFTER_EARLY_END.as_secs_f64());
        let until = kept.until;
        if !self.write_kept(kept) {
            self.hold_slot(cooldown);
            // Nor lost with this N4N0: whoever takes the slot next reads it there.
            let mut slot = self.slot.lock().unwrap();
            if let Some(mine) = slot.mine.clone() {
                let _ = slot.say(InFlight { until, ..mine });
            }
        }
    }

    /// One line the program said of launch `id`. Only a launch still followed changes anything, and only the current one
    /// what runs now.
    fn follow_line(&self, id: u64, library: &Path, line: &str) {
        let Some(event) = obelisk::event(line) else { return };
        let mut now = self.now.lock().unwrap();
        if now.launch(id).is_none_or(|l| l.over) {
            return;
        }
        let current = now.launches.last().is_some_and(|l| l.id == id);
        match event {
            // Admitted as that session, before its game runs: said at once, for whoever takes the slot if this N4N0 is gone by then.
            Event::Starting { session: Some(session) } => {
                if let Some(launch) = now.launch(id) {
                    launch.session = Some(session.clone());
                }
                let mut slot = self.slot.lock().unwrap();
                if let Some(mine) = slot.mine.clone().filter(|_| slot.attached == Some(id)) {
                    // (not written, it is found by its record)
                    let _ = slot.say(InFlight { session: Some(session), ..mine });
                }
            }
            Event::Running { pid, session } => {
                if current {
                    now.running = Some(pid);
                }
                now.engine = None;
                // Before 0.1.3 the program does not name the session: the engine's record of the game does. One said before stays.
                let session = session.or_else(|| now.launch(id).and_then(|l| l.session.clone())).or_else(|| {
                    pid.and_then(|pid| obelisk::session_records(library).into_iter().find(|(r, _)| r.game == Some(pid) && !r.over()).map(|(r, _)| r.id))
                });
                {
                    let mut slot = self.slot.lock().unwrap();
                    if let Some(mine) = slot.mine.clone().filter(|_| slot.attached == Some(id)) {
                        let _ = slot.say(InFlight { session: session.clone(), ..mine });
                    }
                }
                if let Some(launch) = now.launch(id) {
                    launch.launcher = pid;
                    launch.session = session;
                    launch.running = true;
                }
            }
            Event::Exited { code, seconds, frames, session } => {
                // Another session's end is not this launch's.
                let ours = now.launch(id).is_some_and(|l| l.session.is_none() || session.is_none() || l.session == session);
                if !ours {
                    return;
                }
                if current {
                    now.running = None;
                }
                if !cfg!(test) {
                    note(&["steam", "play"], "exited", &format!("exit code {code:?} after {seconds:?} s, {frames:?} frames shown")); // i18n: not shown (for the log)
                }
                self.ended(&mut now, id, seconds, frames);
            }
            // A failure after the game ran (its session, its Cloud saves) is not a start that failed.
            Event::Error { code, message, engine } => {
                if !cfg!(test) {
                    note(&["steam", "play"], &code, &message);
                    if let Some(engine) = &engine {
                        note(&["steam", "play"], &code, &format!("{} ({})", engine.reason, engine.paths.join(", "))); // i18n: not shown (for the log)
                    }
                }
                let ran = now.launch(id).is_some_and(|l| l.running || l.ended.is_some());
                if !ran {
                    let failure = Failure::said(&code, &message, engine.as_ref());
                    if engine.is_some() {
                        now.engine = engine;
                    }
                    if let Some(launch) = now.launch(id) {
                        launch.failure = Some(failure);
                    }
                }
            }
            _ => {}
        }
    }

    /// Launch `id`'s game ended after `seconds` with `frames` shown (each `None` when not known).
    fn ended(&self, now: &mut Now, id: u64, seconds: Option<u64>, frames: Option<u64>) {
        let early = obelisk::early(seconds, frames);
        // An early end is a wait for every start after it, unless another start was made since (it is moot then).
        if early && now.launches.last().is_some_and(|l| l.id == id) {
            self.cool_down(now);
        }
        let failure = early.then(|| Failure::new("ended_at_once", tr("VRChat closed shortly after it started. If it crashed, press Play again.")));
        if let Some(launch) = now.launch(id) {
            launch.running = false;
            launch.ended = Some((seconds, frames));
            launch.unreconciled = false;
            if launch.failure.is_none() {
                launch.failure = failure;
            }
        }
    }

    /// The program is gone: the launch is over. One that ended without saying how the game did is asked of the engine's
    /// record of its session: a record still running is looked at again later ([`Play::reconcile`]); without one, the
    /// start failed (with the program's exit status).
    /// The slot is kept until how it ended is known and kept.
    fn finish(&self, id: u64, library: &Path, status: Option<std::process::ExitStatus>) {
        let mut now = self.now.lock().unwrap();
        let Some(launch) = now.launch(id).cloned() else { return self.free_slot(id) };
        if launch.over {
            return;
        }
        if launch.ended.is_none() {
            let record = launch.session.as_deref().map(|s| obelisk::session_record_of(library, s));
            match record {
                // It failed before its game ran, and said so: that is all there is to say, unless its session lives on.
                Some(Ok(Some(record))) if launch.failure.is_some() && (record.over() || !record_lives(&record)) => {}
                Some(Ok(None) | Err(_)) if launch.failure.is_some() => {}
                Some(Ok(Some(record))) if record.over() => self.ended(&mut now, id, record.seconds, record.frames),
                // Still running by the engine's word (or not readable now): only this N4N0 lost the program; the record is
                // read again until it says how the game ended.
                Some(Ok(Some(_))) | Some(Err(_)) => {
                    if let Some(launch) = now.launch(id) {
                        launch.unreconciled = true;
                        launch.lost_at = Some(std::time::Instant::now());
                    }
                }
                _ if launch.failure.is_none() => {
                    let said = status.map(|s| s.to_string()).unwrap_or_else(|| "?".into());
                    if let Some(launch) = now.launch(id) {
                        launch.running = false;
                        launch.failure = Some(Failure::new("internal", tr!("What runs VRChat on this Mac stopped without saying why ({status}).", status = said)));
                    }
                    // Not known to have ended late: as a start that ended early.
                    if now.launches.last().is_some_and(|l| l.id == id) {
                        self.cool_down(&mut now);
                    }
                }
                _ => {}
            }
        }
        if now.launches.last().is_some_and(|l| l.id == id) {
            now.attached = false;
            now.running = None;
            now.program = None;
        }
        if let Some(launch) = now.launch(id) {
            launch.over = true;
            launch.running = false;
        }
        // An outcome nobody read yet is kept apart, whatever later launches push out of the list.
        if let Some(failure) = now.launch(id).filter(|l| !l.told).and_then(|l| l.failure.clone()) {
            now.unread.retain(|(i, _)| *i != id);
            now.unread.push((id, failure));
            let drop_old = now.unread.len().saturating_sub(UNREAD_KEPT);
            now.unread.drain(..drop_old);
        }
        // Last: what its end means for the next start is kept. One whose end is not known yet holds it until it is.
        if now.launch(id).is_none_or(|launch| !launch.unreconciled) {
            self.free_slot(id);
        }
    }

    /// Reads again the records of launches whose program was lost while their game ran: how each ended, once it did. Each
    /// holds the slot until that is known and kept, or until there is nobody left to say it: then it counts as an early end.
    fn reconcile(&self, now: &mut Now, library: &Path) {
        let pending: Vec<(u64, String, Option<std::time::Instant>)> = now.launches.iter().filter(|l| l.unreconciled).filter_map(|l| Some((l.id, l.session.clone()?, l.lost_at))).collect();
        for (id, session, lost_at) in pending {
            let long_lost = lost_at.is_none_or(|at| at.elapsed() >= obelisk::AFTER_EARLY_END);
            match obelisk::session_record_of(library, &session) {
                Ok(Some(record)) if record.over() => {
                    self.ended(now, id, record.seconds, record.frames);
                    if let Some(failure) = now.launch(id).filter(|l| !l.told).and_then(|l| l.failure.clone()) {
                        now.unread.push((id, failure));
                    }
                }
                // Neither its supervisor nor its game is there any more: nothing will ever say how it ended.
                Ok(Some(record)) if record.supervisor.is_some() && !record_lives(&record) => {
                    if now.launches.last().is_some_and(|l| l.id == id) {
                        self.cool_down(now);
                    }
                    if let Some(launch) = now.launch(id) {
                        launch.unreconciled = false;
                    }
                }
                // A record that names no supervisor: read on, but holding nothing for ever.
                Ok(Some(record)) if record.supervisor.is_none() && long_lost => {}
                Ok(Some(_)) => continue,
                // Gone, or not readable for as long as an early end holds starts: whatever it was is past that.
                _ if long_lost => {
                    if let Some(launch) = now.launch(id) {
                        launch.unreconciled = false;
                    }
                }
                _ => continue,
            }
            self.free_slot(id);
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

    /// Why launch `id` failed, told once to whoever started it (Home's toast); `None` before it failed or once told.
    pub fn take_failure(&self, id: u64) -> Option<Failure> {
        let mut now = self.now.lock().unwrap();
        let kept = now.unread.iter().position(|(i, _)| *i == id).map(|at| now.unread.remove(at).1);
        let launch = now.launch(id);
        if launch.as_ref().is_some_and(|l| l.told) {
            return None;
        }
        let failure = kept.or_else(|| launch.as_ref().and_then(|l| l.failure.clone()))?;
        if let Some(launch) = now.launch(id) {
            launch.told = true;
        }
        Some(failure)
    }

    /// The engine's files no game starts on, as a start here last said.
    pub fn engine_problem(&self) -> Option<obelisk::EngineProblem> {
        self.now.lock().unwrap().engine.clone()
    }

    /// Installs what the engine is made of again (`engine repair`), which the program does under the lock every launch
    /// holds, with what is there kept until the new one is whole: `progress` hears how far it is.
    pub fn repair(&self, program: &Path, stop: &AtomicBool, progress: &dyn Fn(Option<u8>)) -> Result<(), Failure> {
        let mut repaired = false;
        let done = run(program, &["engine", "repair"], stop, |event| match event {
            Event::Repairing { fraction } => progress(fraction.map(|f| (f * 100.0).round() as u8)),
            Event::Repaired => repaired = true,
            _ => {}
        });
        match done {
            Ok(()) if repaired => {
                self.now.lock().unwrap().engine = None;
                Ok(())
            }
            Ok(()) => Err(Failure::new("internal", tr("What runs VRChat on this Mac stopped without saying why."))),
            Err(failure) => {
                // What is there now is what it says.
                if failure.engine.is_some() {
                    self.now.lock().unwrap().engine = failure.engine.clone();
                }
                Err(failure)
            }
        }
    }

    /// The newest launch made, whoever made it and whatever became of it.
    pub fn newest(&self) -> Option<Launch> {
        self.now.lock().unwrap().launches.last().cloned()
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

/// The gate's rules on a clock of the test's (every computer).
#[cfg(test)]
mod gate_tests {
    use super::*;
    use obelisk::{BootInstant, Damaged, RecordScan, Unlistable};
    use std::time::SystemTime;

    const BOOT: &str = "11466E1D-BF8E-4060-A996-1C47BE650634";
    fn at(seconds: f64) -> BootInstant {
        BootInstant { boot: BOOT.into(), seconds }
    }
    fn damaged(name: &str, current: bool, len: u64) -> Damaged {
        Damaged { path: PathBuf::from(format!("/L/SteamSessions/{name}")), identity: Some((1, 2, len, 3)), current }
    }
    fn scan(damaged: Vec<Damaged>) -> Result<RecordScan, Unlistable> {
        Ok(RecordScan { early_ends: Vec::new(), damaged })
    }
    fn kept() -> Kept {
        Kept { boot: BOOT.into(), ..Default::default() }
    }
    const WALL: SystemTime = SystemTime::UNIX_EPOCH;

    /// B3: one old damaged record (another session's) holds starts only while it may still be a start that just ended;
    /// unchanged for the whole wait it is history, shown but holding nothing. A change to it is a new file, seen anew.
    #[test]
    fn a_damaged_old_record_holds_starts_for_the_wait_once_then_is_history() {
        let mut k = kept();
        let records = scan(vec![damaged("a.v2.json", false, 10)]);
        let (verdict, trouble, write) = judge(&records, &mut k, &at(1000.0), WALL, None);
        assert!(matches!(&verdict, Err(Refused::Records(t)) if t.wait() == Some(Duration::from_secs(80))), "{verdict:?}");
        assert!(write, "first seen: kept");
        assert!(trouble.unwrap().blocks);
        // Retried again and again: the wait counts down, never starts again.
        let (verdict, _, _) = judge(&records, &mut k, &at(1030.0), WALL, None);
        assert!(matches!(&verdict, Err(Refused::Records(t)) if t.wait() == Some(Duration::from_secs(50))), "{verdict:?}");
        let (verdict, trouble, _) = judge(&records, &mut k, &at(1081.0), WALL, None);
        assert_eq!(verdict, Ok(None), "history now: Play plays");
        let trouble = trouble.expect("still shown, to be set aside");
        assert!(!trouble.blocks && trouble.settled_in.is_none() && !trouble.current);
        // The same file written again: another content, seen anew.
        let changed = scan(vec![damaged("a.v2.json", false, 11)]);
        assert!(judge(&changed, &mut k, &at(1100.0), WALL, None).0.is_err());
        // Gone (set aside, or mended): forgotten.
        let (verdict, trouble, write) = judge(&scan(Vec::new()), &mut k, &at(1101.0), WALL, None);
        assert_eq!((verdict, trouble, write), (Ok(None), None, true));
        assert!(k.seen.is_empty());
    }

    /// The pointer to the current session, or the record it names, damaged: the engine stops there itself, so Home holds
    /// Play until the person sets it aside; it may be set aside once it was seen unchanged for the wait.
    #[test]
    fn a_damaged_current_record_holds_starts_until_it_is_set_aside() {
        let mut k = kept();
        let records = scan(vec![damaged("old.v2.json", false, 4), damaged("session.v2.json", true, 9)]);
        let (verdict, trouble, _) = judge(&records, &mut k, &at(50.0), WALL, None);
        let Err(Refused::Records(t)) = verdict else { panic!("{verdict:?}") };
        assert!(t.current && t.file.ends_with("session.v2.json") && t.wait().is_none(), "{t:?}");
        assert_eq!(trouble.unwrap().settled_in, Some(Duration::from_secs(80)));
        let (verdict, trouble, _) = judge(&records, &mut k, &at(500.0), WALL, None);
        assert!(matches!(verdict, Err(Refused::Records(t)) if t.current), "still held: only setting it aside ends it");
        assert_eq!(trouble.unwrap().settled_in, None, "now it may be set aside");
        assert!(Refused::Records(t).words().contains("session.v2.json"), "Home names the file");
    }

    #[test]
    fn a_folder_that_cannot_be_listed_holds_starts_and_says_which() {
        let mut k = kept();
        let (verdict, trouble, _) = judge(&Err(Unlistable(PathBuf::from("/L/SteamSessions"))), &mut k, &at(5.0), WALL, None);
        assert!(matches!(&verdict, Err(Refused::Records(t)) if t.unlistable && t.wait().is_none()));
        assert!(trouble.unwrap().unlistable);
        assert!(verdict.unwrap_err().words().contains("/L/SteamSessions"));
    }

    /// The kept gate: on this boot's UUID, so a wall clock set forward with a restart of N4N0 does not end it; another
    /// boot's is over; a deadline too far ahead counts down from now, not anew at each try; a file that is not one holds
    /// starts for the wait once, then is written again as it is.
    #[test]
    fn the_kept_deadline_is_bounded_and_survives_a_wall_clock_change() {
        let file = |text: &str| Some(Ok(text.to_string()));
        let (mut k, write) = read_kept(file(&format!(r#"{{"version":2,"boot":"{BOOT}","until":1080.0,"seen":{{}}}}"#)), &at(1030.0));
        assert!(!write);
        // The wall clock moved a day ahead: nothing of it is read.
        let tomorrow = WALL + Duration::from_secs(86_400);
        assert_eq!(judge(&scan(Vec::new()), &mut k, &at(1030.0), tomorrow, None).0, Ok(Some(Duration::from_secs(50))));
        let (mut other, write) = read_kept(file(&format!(r#"{{"version":2,"boot":"{BOOT}","until":1080.0,"seen":{{}}}}"#)), &BootInstant { boot: "AAAAAAAA-BF8E-4060-A996-1C47BE650634".into(), seconds: 20.0 });
        assert!(write && other.until.is_none(), "another boot: over");
        assert_eq!(judge(&scan(Vec::new()), &mut other, &BootInstant { boot: "AAAAAAAA-BF8E-4060-A996-1C47BE650634".into(), seconds: 20.0 }, WALL, None).0, Ok(None));
        // Far ahead: as if written now, then counted down.
        let (mut far, _) = read_kept(file(&format!(r#"{{"version":2,"boot":"{BOOT}","until":99999999.0,"seen":{{}}}}"#)), &at(100.0));
        let (verdict, _, write) = judge(&scan(Vec::new()), &mut far, &at(100.0), WALL, None);
        assert_eq!((verdict, write), (Ok(Some(Duration::from_secs(80))), true));
        assert_eq!(judge(&scan(Vec::new()), &mut far, &at(130.0), WALL, None).0, Ok(Some(Duration::from_secs(50))), "not 80 s again");
        assert_eq!(judge(&scan(Vec::new()), &mut far, &at(181.0), WALL, None).0, Ok(None));
        // Not a gate file: no proof of none, held for the wait once, from now.
        for broken in ["not json", "{}", r#"{"version":2,"boot":"x"}"#, r#"{"version":2,"boot":"11466E1D-BF8E-4060-A996-1C47BE650634","until":"soon","seen":{}}"#] {
            let (mut k, write) = read_kept(file(broken), &at(10.0));
            assert!(write, "{broken}: written again");
            assert_eq!(judge(&scan(Vec::new()), &mut k, &at(10.0), WALL, None).0, Ok(Some(Duration::from_secs(80))), "{broken}");
            assert_eq!(judge(&scan(Vec::new()), &mut k, &at(91.0), WALL, None).0, Ok(None), "{broken}: bounded");
        }
        let (mut k, write) = read_kept(Some(Err(())), &at(10.0));
        assert!(write && judge(&scan(Vec::new()), &mut k, &at(10.0), WALL, None).0.is_ok_and(|w| w.is_some()), "unreadable: the same");
        let (k, write) = read_kept(None, &at(10.0));
        assert!(!write && k.until.is_none(), "no file: nothing ever ended early here");
        // Its own text reads back the same.
        let kept = Kept { boot: BOOT.into(), until: Some(12.5), seen: vec![("/L/a|1|2|3|4".into(), 3.0)] };
        assert_eq!(read_kept(file(&kept.to_json()), &at(5.0)), (kept, false));
    }

    /// Records that say when they ended on the boot clock (obelisk-cli 0.1.6) are read on it; older ones by when they were
    /// written; a record of another boot is over.
    #[test]
    fn an_early_end_is_read_on_the_boot_clock_when_its_record_says_it() {
        let mut k = kept();
        let wall = SystemTime::now();
        let ends = |ended: Option<BootInstant>, written: SystemTime| Ok(RecordScan { early_ends: vec![(written, ended)], damaged: Vec::new() });
        let long_ago = wall - Duration::from_secs(86_400);
        assert_eq!(judge(&ends(Some(at(990.0)), long_ago), &mut k, &at(1000.0), wall, None).0, Ok(Some(Duration::from_secs(70))), "a wall clock set forward does not end it");
        assert_eq!(judge(&ends(Some(BootInstant { boot: "AAAAAAAA-BF8E-4060-A996-1C47BE650634".into(), seconds: 990.0 }), wall), &mut k, &at(1000.0), wall, None).0, Ok(None), "another boot");
        assert!(judge(&ends(None, wall - Duration::from_secs(10)), &mut k, &at(1000.0), wall, None).0.unwrap().is_some_and(|w| w > Duration::from_secs(60)), "an older record: by when it was written");
        // This run's own wait wins when longer.
        assert_eq!(judge(&scan(Vec::new()), &mut k, &at(1000.0), wall, Some(Duration::from_secs(33))).0, Ok(Some(Duration::from_secs(33))));
    }
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
        assert_eq!(status, Some(Event::Status { account: None, steam_id: None }));
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
        // Seen ending early by this run: no start for 80 s at once, before any record says so.
        let launch = wait_for(&play, id, |l| l.over);
        assert!(matches!(play.start(&program, &library, &[]), Err(Refused::Wait(_))), "the exit just seen holds the next start");
        clear(&play);
        assert_eq!((launch.launcher, launch.session.as_deref()), (Some(83030), Some("6da7c40d-9fd2-4f3b-8770-aadffa2d0088")));
        assert_eq!(launch.ended, Some((Some(100), Some(0))));
        assert_eq!(launch.failure.as_ref().map(|f| f.code.as_str()), Some("ended_at_once"), "no frame shown: a start that did not work");
        // Its starter's toast takes it once, by its id; the launch's own outcome stays for whoever reads it.
        assert!(play.take_failure(id).is_some());
        assert!(play.take_failure(id).is_none(), "told once");
        assert!(play.launch(id).unwrap().failure.is_some(), "the launch's own outcome is not used up");

        // One start at a time, for whoever asks.
        let slow = fake_program(&dir.join("slow"), &[r#"{"event":"starting","app_id":438100}"#], 3);
        let first = play.start(&slow, &library, &[]).expect("starts");
        assert_eq!(play.start(&slow, &library, &[]), Err(Refused::Busy));
        let gone = wait_for(&play, first, |l| l.over);
        // It ended without a word of the game: a failed start, and the same wait as after an early end.
        assert_eq!(gone.failure.map(|f| f.code), Some("internal".into()));
        assert!(matches!(play.start(&program, &library, &[]), Err(Refused::Wait(_))));
        clear(&play);

        // An early end in the engine's records holds every start for 80 s, from the records (a restart of N4N0 keeps it).
        std::fs::create_dir_all(library.join("SteamSessions")).unwrap();
        let record = library.join("SteamSessions").join("6da7c40d-9fd2-4f3b-8770-aadffa2d0088.v2.json");
        std::fs::write(&record, r#"{"id":"6da7c40d-9fd2-4f3b-8770-aadffa2d0088","store":{"app_id":438100},"phase":"Finished","played":{"seconds":100,"frames":{"count":0}}}"#).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let refused = play.start(&program, &library, &[]).unwrap_err();
        assert!(matches!(refused, Refused::Wait(wait) if wait > Duration::from_secs(70)), "{refused:?}");
        assert!(refused.words().contains("s."));
        let later = std::time::SystemTime::now() + Duration::from_secs(81);
        assert!(play.start_at(&program, &library, &[], later).is_ok(), "80 s on, it starts");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The verifier's cases: the program ends while a process it started keeps its output open (the game's supervisor
    /// inherits it): the start is over all the same, and having said nothing of the game it is a failure of its own launch,
    /// never another's; Home is told its own launch's failure once, by its id.
    #[test]
    fn a_program_gone_while_its_output_stays_open_ends_its_start_with_its_own_failure() {
        let dir = std::env::temp_dir().join(format!("orbiters-play-survivor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let survivor = fake_program(&dir, &[], 0);
        std::fs::write(&survivor, "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"starting\",\"app_id\":438100}'\n(sleep 30) &\nexit 0\n").unwrap();
        let play = Arc::new(Play::default());
        let id = play.start(&survivor, &library, &[]).expect("starts");
        let begun = std::time::Instant::now();
        let launch = wait_for(&play, id, |l| l.over);
        assert!(launch.over && begun.elapsed() < Duration::from_secs(10), "not held by the survivor: {launch:?}");
        assert_eq!(play.attached(), None, "the next start is not blocked");
        assert_eq!(launch.failure.as_ref().map(|f| f.code.as_str()), Some("internal"));
        assert_eq!(play.take_failure(id).map(|f| f.code), Some("internal".into()));
        assert_eq!(play.take_failure(id), None, "told once");
        assert_eq!(play.take_failure(id + 1), None, "another launch's is not this one's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Forgets this run's own wait (memory and kept), as a later run of N4N0 without a gate file would.
    fn clear(play: &Play) {
        let mut now = play.now.lock().unwrap();
        now.cooldown = None;
        now.kept = None;
    }

    #[test]
    fn the_wait_kept_on_the_boot_clock_is_read_back_by_the_next_run() {
        let file = std::env::temp_dir().join(format!("orbiters-gate-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let play = Play::new(file.clone());
        play.cool_down(&mut play.now.lock().unwrap());
        let next_run = Play::new(file.clone());
        let library = std::env::temp_dir().join(format!("orbiters-gate-library-{}", std::process::id()));
        assert!(matches!(next_run.gate(&library), Ok(Some(wait)) if wait > Duration::from_secs(70)), "{:?}", next_run.gate(&library));
        assert!(std::fs::read_to_string(&file).unwrap().contains(&boot_clock().boot), "kept on this boot's own UUID");
        let _ = std::fs::remove_file(&file);
    }

    /// B3's recovery in Home: a damaged current record holds Play and is named; it is set aside only once it is history,
    /// never while the engine holds its records, never once it changed; set aside, Play plays, and the file is kept.
    #[test]
    fn a_damaged_record_is_set_aside_only_once_it_is_history_and_then_play_plays() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-set-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let sessions = obelisk::sessions_dir(&library);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
        let write = |name: &str, text: &str| {
            let path = sessions.join(name);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            path
        };
        let id = "c29c0b3a-9cd4-4ca7-aeeb-e69195577759";
        write(obelisk::POINTER, &format!(r#"{{"version":2,"id":"{id}"}}"#));
        let record = write(&format!("{id}.v2.json"), r#"{"version":2,"id":"c29c"#);
        let play = Arc::new(Play::new(dir.join("gate.json")));
        let program = fake_program(&dir.join("bin"), &[], 0);
        let refused = play.start(&program, &library, &[]).unwrap_err();
        assert!(matches!(&refused, Refused::Records(t) if t.current && t.file == record), "{refused:?}");
        assert!(refused.words().contains(&format!("{id}.v2.json")), "the file is named: {}", refused.words());
        // Just seen: it may still be a start that just ended.
        assert!(play.set_aside(&library, &record).unwrap_err().contains(" s."));
        assert!(play.trouble(&library).unwrap().settled_in.is_some());
        // Seen unchanged for the whole wait (the kept first sight moved back, as 81 s on).
        let age = |play: &Play| {
            let mut now = play.now.lock().unwrap();
            for (_, first) in &mut now.kept.as_mut().unwrap().seen {
                *first -= 81.0;
            }
        };
        age(&play);
        assert_eq!(play.trouble(&library).unwrap().settled_in, None);
        // The engine's supervisor holds its records: left.
        let owner = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(sessions.join("owner.lock")).unwrap();
        owner.lock().unwrap();
        assert!(play.set_aside(&library, &record).is_err(), "a supervisor holds the records");
        owner.unlock().unwrap();
        // Written again meanwhile: another file, seen anew.
        std::thread::sleep(Duration::from_millis(5));
        write(&format!("{id}.v2.json"), r#"{"version":2,"id":"c29c0b3a"#);
        assert!(play.set_aside(&library, &record).unwrap_err().contains(" s."), "changed: seen anew, not history");
        age(&play);
        // A path that is no damaged record: nothing.
        assert!(play.set_aside(&library, &sessions.join("owner.lock")).is_err());
        let aside = play.set_aside(&library, &record).expect("set aside");
        assert!(aside.starts_with(library.join("Set aside")) && aside.is_file() && !record.exists(), "kept, out of the records: {aside:?}");
        assert_eq!(play.trouble(&library), None);
        assert!(play.gate(&library).is_ok_and(|w| w.is_none()), "Play plays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ruling 1 and R7: Home's start asks for its render scale for that launch alone, before the game's arguments, only of a
    /// program that takes it (0.1.7 and later: an older one would refuse the whole start); the bridge's starts never do (its
    /// coordinates are the engine's own size).
    #[test]
    fn home_s_start_asks_for_its_render_scale_and_the_bridge_s_for_the_whole_size() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let program_of = |version: &str| {
            let folder = dir.join(format!("obelisk-cli-{version}-arm64"));
            std::fs::create_dir_all(&folder).unwrap();
            let script = folder.join("obelisk-cli");
            let said = folder.join("args");
            std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"event\":\"starting\",\"app_id\":438100}}'\n", said.display())).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            (script, said)
        };
        let args = ["-screen-fullscreen".to_string(), "1".to_string()];
        let said = |file: &Path| -> Vec<String> {
            for _ in 0..200 {
                if let Ok(text) = std::fs::read_to_string(file) {
                    if text.ends_with('\n') {
                        return text.lines().map(str::to_owned).collect();
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Vec::new()
        };
        let (new, new_said) = program_of("0.1.7");
        let play = Arc::new(Play::default());
        let id = play.start_rendered(&new, &library, &args, Some(58), None).expect("starts");
        assert_eq!(said(&new_said), ["steam", "play", "438100", "--render-percent", "58", "--", "-screen-fullscreen", "1"]);
        let _ = wait_for(&play, id, |l| l.over);
        clear(&play);
        let (old, old_said) = program_of("0.1.6");
        let id = play.start_rendered(&old, &library, &args, Some(58), None).expect("starts");
        assert_eq!(said(&old_said), ["steam", "play", "438100", "--", "-screen-fullscreen", "1"], "an older program: the engine's size");
        let _ = wait_for(&play, id, |l| l.over);
        clear(&play);
        std::fs::remove_file(&new_said).unwrap();
        let id = play.start_reserved(&new, &library, &args, None).expect("the bridge's start");
        // Fourth verification, finding 10: the bridge's start says 100 % itself, so that no percentage saved for the
        // installation applies to the game the bridge measures at 100 %.
        assert_eq!(said(&new_said), ["steam", "play", "438100", "--render-percent", "100", "--", "-screen-fullscreen", "1"], "the bridge's: always the whole size, said");
        let _ = wait_for(&play, id, |l| l.over);
        clear(&play);
        std::fs::remove_file(&old_said).unwrap();
        let id = play.start_reserved(&old, &library, &args, None).expect("the bridge's start, older program");
        assert_eq!(said(&old_said), ["steam", "play", "438100", "--", "-screen-fullscreen", "1"], "an older program has no percent to save or override");
        let _ = wait_for(&play, id, |l| l.over);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B4: the bridge's reservation, taken before it creates an instance, holds Home's start off until the bridge's launch
    /// uses it or gives it back; a second reservation is refused; the reserver's own launch goes.
    #[test]
    fn a_reserved_start_is_the_reserver_s_alone_until_used_or_given_back() {
        let dir = std::env::temp_dir().join(format!("orbiters-reserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let program = fake_program(&dir, &[r#"{"event":"starting","app_id":438100}"#], 1);
        let play = Arc::new(Play::default());
        let token = play.reserve(&library, None).expect("reserved");
        assert_eq!(play.start(&program, &library, &[]), Err(Refused::Busy), "Home's start waits for the bridge's");
        assert_eq!(play.reserve(&library, None), Err(Refused::Busy), "one reservation at a time");
        play.unreserve(token);
        let home = play.start(&program, &library, &[]).expect("given back: Home starts");
        assert_eq!(play.reserve(&library, None), Err(Refused::Busy), "Home's game is attached");
        assert!(play.reserve(&library, Some(home + 1000)).is_err(), "not the reserver's own game");
        let own = play.reserve(&library, Some(home)).expect("its own attached game, to be closed first");
        let _ = wait_for(&play, home, |l| l.over);
        clear(&play);
        assert_eq!(play.start(&program, &library, &[]), Err(Refused::Busy), "reserved");
        let bridge = play.start_reserved(&program, &library, &[], Some(own)).expect("the reserver's launch");
        let _ = wait_for(&play, bridge, |l| l.over);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M9: one owner of each launch: the program's output closing while it lives does not end the launch; a process that
    /// keeps its output and speaks after the launch ended changes nothing of the next one; the exit status says why;
    /// untold failures outlive later launches.
    #[test]
    fn a_launch_ends_with_its_program_never_by_its_output_and_late_words_change_nothing() {
        let dir = std::env::temp_dir().join(format!("orbiters-play-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let play = Arc::new(Play::default());
        // Output closed, the program still there for 2 s.
        let quiet = fake_program(&dir.join("quiet"), &[], 0);
        std::fs::write(&quiet, "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"starting\",\"app_id\":438100}'\nexec 1>&-\nsleep 2\nexit 3\n").unwrap();
        let id = play.start(&quiet, &library, &[]).unwrap();
        std::thread::sleep(Duration::from_millis(1000));
        assert!(!play.launch(id).unwrap().over && play.attached() == Some(id), "the program lives: the launch goes on");
        let gone = wait_for(&play, id, |l| l.over);
        assert!(gone.failure.as_ref().is_some_and(|f| f.words.contains('3')), "its exit status said: {:?}", gone.failure);
        clear(&play);
        // A child keeps the output and speaks after the program ended.
        let late = fake_program(&dir.join("late"), &[], 0);
        std::fs::write(&late, "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"starting\",\"app_id\":438100}'\n(sleep 5; printf '%s\\n' '{\"event\":\"running\",\"app_id\":438100,\"pid\":4242}') &\nexit 0\n").unwrap();
        let first = play.start(&late, &library, &[]).unwrap();
        let first = wait_for(&play, first, |l| l.over);
        assert!(!first.running);
        clear(&play);
        let slow = fake_program(&dir.join("slow"), &[r#"{"event":"starting","app_id":438100}"#], 4);
        let second = play.start(&slow, &library, &[]).unwrap();
        std::thread::sleep(Duration::from_millis(3000));
        assert_eq!(play.running(), None, "the first one's late words are not the second's");
        assert!(!play.launch(first.id).unwrap().running);
        let _ = wait_for(&play, second, |l| l.over);
        // Untold failures stay for their starter, however many launches came after.
        clear(&play);
        let fails = fake_program(&dir.join("fails"), &[], 0);
        std::fs::write(&fails, "#!/bin/sh\nexit 7\n").unwrap();
        let mut ids = Vec::new();
        for _ in 0..(LAUNCHES_KEPT + 3) {
            let id = play.start(&fails, &library, &[]).unwrap();
            let _ = wait_for(&play, id, |l| l.over);
            clear(&play);
            ids.push(id);
        }
        assert!(play.launch(ids[0]).is_none(), "pushed out of the list");
        assert!(play.take_failure(ids[0]).is_some(), "its failure is still there for its starter");
        assert!(play.take_failure(ids[0]).is_none(), "told once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seventh verification (two coordinators): a deadline the other one kept is read before every decision and never
    /// written over, however long ago this one read the gate.
    #[test]
    fn a_deadline_another_coordinator_kept_is_read_before_every_decision() {
        let dir = std::env::temp_dir().join(format!("orbiters-gate-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (library, gate) = (dir.join("library"), dir.join("gate.json"));
        let program = fake_program(&dir.join("bin"), &[], 0);
        let (a, b) = (Arc::new(Play::new(gate.clone())), Arc::new(Play::new(gate.clone())));
        // Both have read the gate; then one sees an early end, which only its file tells the other.
        assert_eq!((a.gate(&library), b.gate(&library)), (Ok(None), Ok(None)));
        b.cool_down(&mut b.now.lock().unwrap());
        let waits = |refused: Result<u64, Refused>| matches!(refused, Err(Refused::Wait(wait)) if wait > Duration::from_secs(70));
        assert!(waits(a.reserve(&library, None)), "{:?}", a.gate(&library));
        assert!(waits(a.start(&program, &library, &[])));
        assert!(!std::fs::read_to_string(&gate).unwrap().contains("\"until\":null"), "and kept as it was");
        // What this one keeps is added to what is there, never put in its place.
        let until = |play: &Play| play.now.lock().unwrap().kept.as_ref().unwrap().until.unwrap();
        let theirs = until(&b);
        a.now.lock().unwrap().kept = None;
        a.cool_down(&mut a.now.lock().unwrap());
        assert!(until(&a) >= theirs);
        // A refusal holds nothing: the other one is asked again at once.
        assert!(waits(b.reserve(&library, None)));
        // A file that is no gate holds starts once, not anew at every look.
        std::fs::write(&gate, "{ half").unwrap();
        let held = std::fs::File::open(dir.join("gate.lock")).unwrap();
        held.lock().unwrap();
        let c = Play::new(gate.clone());
        assert!(matches!(c.gate(&library), Ok(Some(_))));
        c.now.lock().unwrap().kept.as_mut().unwrap().until = None;
        assert_eq!(c.gate(&library), Ok(None), "told once, and not written while the slot is another's");
        assert_eq!(std::fs::read_to_string(&gate).unwrap(), "{ half");
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Eighth verification (the verifier's fixture): a cooldown that cannot be written holds the slot, so that no coordinator
    /// starts, until it is written after all.
    #[test]
    fn a_cooldown_that_cannot_be_written_holds_every_coordinator() {
        let dir = std::env::temp_dir().join(format!("orbiters-gate-unwritten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (library, gate) = (dir.join("library"), dir.join("gate.json"));
        std::fs::create_dir_all(dir.join("gate.part")).unwrap();
        let program = fake_program(&dir.join("bin"), &[], 0);
        let (a, b) = (Arc::new(Play::new(gate.clone())), Arc::new(Play::new(gate.clone())));
        let id = a.start(&program, &library, &[]).unwrap();
        wait_for(&a, id, |l| l.over);
        for _ in 0..2 {
            assert!(matches!(a.gate(&library), Ok(Some(_))), "its end was early");
            assert!(matches!(a.reserve(&library, None), Err(Refused::Wait(_))));
            assert_eq!(b.reserve(&library, None), Err(Refused::Busy));
            assert_eq!(b.start(&program, &library, &[]), Err(Refused::Busy));
            assert!(!gate.exists());
        }
        // It can be written again: the next look does, and the other one reads it.
        std::fs::remove_dir(dir.join("gate.part")).unwrap();
        assert!(matches!(a.gate(&library), Ok(Some(_))));
        assert!(matches!(b.reserve(&library, None), Err(Refused::Wait(_))));
        assert!(matches!(a.reserve(&library, None), Err(Refused::Wait(_))));
        // Never written: let go of once the wait is over, whoever looks.
        let other = dir.join("other/gate.json");
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let (a, b) = (Play::new(other.clone()), Play::new(other));
        a.hold_slot(std::time::Instant::now() + Duration::from_millis(900));
        a.let_slot_go();
        assert_eq!(b.reserve(&library, None), Err(Refused::Busy));
        std::thread::sleep(Duration::from_millis(1000));
        assert!(b.reserve(&library, None).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ninth verification: the game looked at ends between the look and the reservation that names it. The id alone admits
    /// nothing: the slot must still be held for it.
    #[test]
    fn a_launch_that_ended_since_it_was_looked_at_admits_no_reservation() {
        let dir = std::env::temp_dir().join(format!("orbiters-reserve-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let program = fake_program(&dir.join("bin"), &[r#"{"event":"starting","app_id":438100}"#], 1);
        let (a, b) = (Arc::new(Play::new(dir.join("gate.json"))), Arc::new(Play::new(dir.join("gate.json"))));
        let id = a.start(&program, &library, &[]).unwrap();
        // Looked at while it ran, and while it runs its own reservation is its own.
        let looked = a.attached();
        assert_eq!(looked, Some(id));
        let token = a.reserve(&library, looked).expect("its own game, to be closed first");
        a.unreserve(token);
        let _ = wait_for(&a, id, |l| l.over);
        assert!(matches!(a.reserve(&library, looked), Err(Refused::Wait(_))), "its end was early: no instance is made now");
        assert!(matches!(b.reserve(&library, looked), Err(Refused::Wait(_))), "and nothing was left held");
        // Nor is another's game its own.
        let token = {
            clear(&b);
            std::fs::remove_file(dir.join("gate.json")).unwrap();
            b.reserve(&library, looked).expect("past the wait: a start like any other")
        };
        clear(&a);
        assert_eq!(a.reserve(&library, looked), Err(Refused::Busy));
        b.unreserve(token);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// As its process dying does: what it held of the slot is let go of, and nothing of it is said any more.
    fn dies(play: &Play) {
        play.slot.lock().unwrap().file = None;
    }

    /// The program of a holder that is gone ends too: nobody hears of it.
    fn its_program_ends(play: &Play) {
        let program = {
            let mut now = play.now.lock().unwrap();
            now.launches.iter_mut().for_each(|l| l.over = true);
            now.program
        };
        // SAFETY: the fake program this test started, whose owner's thread reaps it.
        unsafe { libc::kill(program.expect("attached") as i32, libc::SIGKILL) };
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while program.is_some_and(|pid| named(pid).is_some()) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Ninth verification (the verifier's fixture): a holder that dies with a start in flight, or with a wait it could not
    /// write, leaves that in the slot: the next one waits as after an early end, whether or not it can write the gate either,
    /// and once only.
    #[test]
    fn a_holder_that_dies_leaves_its_start_in_the_slot() {
        let dir = std::env::temp_dir().join(format!("orbiters-slot-dies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let slow = fake_program(&dir.join("slow"), &[r#"{"event":"starting","app_id":438100}"#], 2);
        let quick = fake_program(&dir.join("quick"), &[], 0);
        let waits = |refused: Result<u64, Refused>| matches!(refused, Err(Refused::Wait(wait)) if wait > Duration::from_secs(70));
        for unwritable in [false, true] {
            for in_flight in [true, false] {
                let here = dir.join(format!("{unwritable}-{in_flight}"));
                std::fs::create_dir_all(&here).unwrap();
                if unwritable {
                    std::fs::create_dir_all(here.join("gate.part")).unwrap();
                }
                let new = || Arc::new(Play::new(here.join("gate.json")));
                let a = new();
                let id = a.start(if in_flight { &slow } else { &quick }, &library, &[]).unwrap();
                if !in_flight {
                    let _ = wait_for(&a, id, |l| l.over);
                }
                let b = new();
                if in_flight || unwritable {
                    assert_eq!(b.reserve(&library, None), Err(Refused::Busy), "held while it lives");
                }
                dies(&a);
                if in_flight && cfg!(target_os = "macos") {
                    assert_eq!(b.reserve(&library, None), Err(Refused::Busy), "the program still makes it");
                    its_program_ends(&a);
                }
                assert!(waits(b.reserve(&library, None)), "unwritable {unwritable}, in flight {in_flight}: {:?}", b.gate(&library));
                assert!(waits(b.start(&quick, &library, &[])));
                // As for one that comes later, which nobody told: the same wait, not a new one.
                let until = |play: &Play| play.now.lock().unwrap().kept.as_ref().unwrap().until.unwrap();
                let c = new();
                assert!(waits(c.reserve(&library, None)));
                assert!((until(&b) - until(&c)).abs() < 0.01, "{unwritable} {in_flight}");
                let _ = wait_for(&a, id, |l| l.over);
            }
        }
        // Over, it holds nothing; another boot's neither; anything but a start is one, once.
        let gate = dir.join("over/gate.json");
        std::fs::create_dir_all(dir.join("over")).unwrap();
        let clock = boot_clock();
        let lock = dir.join("over/gate.lock");
        let note = |boot: &str, until: Option<f64>| InFlight { boot: Some(boot.into()), started: 1.0, until, ..Default::default() }.to_json();
        for over in [note(&clock.boot, Some(clock.seconds - 1.0)), note("AAAAAAAA-BF8E-4060-A996-1C47BE650634", None), String::new()] {
            std::fs::write(&lock, &over).unwrap();
            assert_eq!(Play::new(gate.clone()).gate(&library), Ok(None), "{over}");
            assert_eq!(std::fs::read_to_string(&lock).unwrap(), "", "{over}: settled");
        }
        std::fs::write(&lock, "{ half").unwrap();
        assert!(matches!(Play::new(gate.clone()).gate(&library), Ok(Some(_))));
        let later = Play::new(gate.clone());
        assert!(matches!(later.gate(&library), Ok(Some(_))));
        clear(&later);
        std::fs::remove_file(&gate).unwrap();
        assert_eq!(later.gate(&library), Ok(None), "told once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The engine's record says how the start a dead holder left ended: a game played to its end holds nothing, one that still
    /// runs nothing yet, one gone without a word as an early end.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_start_left_by_a_dead_holder_is_settled_by_its_record() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-slot-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let sessions = obelisk::sessions_dir(&library);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
        let id = "f16d947e-7d8f-4331-8731-bede005d29fe";
        let record = sessions.join(format!("{id}.v2.json"));
        let write = |text: &str| {
            std::fs::write(&record, text).unwrap();
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        let (seconds, micros) = super::super::bridge_mac::born(std::process::id()).unwrap();
        let running = |seconds: u64| {
            let me = format!(r#"{{"pid":{},"birth_seconds":{seconds},"birth_microseconds":{micros}}}"#, std::process::id());
            format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Running","supervisor":{me},"game":{me}}}"#)
        };
        let program = fake_program(&dir.join("bin"), &[&format!(r#"{{"event":"running","app_id":438100,"pid":4242,"session_id":"{id}"}}"#)], 2);
        for (name, ends, waits) in [
            ("played", format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Finished","played":{{"seconds":4000,"frames":{{"count":96000}}}}}}"#), false),
            ("gone", running(seconds - 100), true),
        ] {
            let gate = dir.join(name).join("gate.json");
            let a = Arc::new(Play::new(gate.clone()));
            let launch = a.start(&program, &library, &[]).unwrap();
            write(&running(seconds));
            assert!(wait_for(&a, launch, |l| l.running).running);
            dies(&a);
            let b = Play::new(gate.clone());
            assert_eq!(b.gate(&library), Ok(None), "{name}: it still runs");
            assert_eq!(b.reserve(&library, None), Err(Refused::Busy), "{name}: over which none is started");
            assert!(std::fs::read_to_string(gate.with_extension("lock")).unwrap().contains(id), "{name}: and is still said");
            write(&ends);
            assert_eq!(b.gate(&library).unwrap().is_some(), waits, "{name}");
            assert_eq!(std::fs::read_to_string(gate.with_extension("lock")).unwrap(), "", "{name}: settled");
            let _ = wait_for(&a, launch, |l| l.over);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tenth verification (the verifier's fixture, and the same with every word its holder may have left): a session that lives by
    /// the engine's records admits no start and no reservation, whatever was left of it and whatever wait is over; once it is
    /// over, its end decides.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_live_session_admits_nothing_whatever_its_dead_holder_left() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-slot-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = "f16d947e-7d8f-4331-8731-bede005d29fe";
        let me = named(std::process::id()).unwrap();
        let person = |(pid, birth): (u32, (u64, u32))| format!(r#"{{"pid":{pid},"birth_seconds":{},"birth_microseconds":{}}}"#, birth.0, birth.1);
        let gone = (me.0, (me.1 .0 - 100, me.1 .1));
        let clock = boot_clock();
        let left = |session: Option<&str>, program, until| InFlight { boot: Some(clock.boot.clone()), started: 1.0, session: session.map(str::to_owned), program, until };
        for (name, phase, note) in [
            ("anonymous", "Running", left(None, None, None)),
            ("preparing", "Preparing", left(None, None, None)),
            ("named", "Preparing", left(Some(id), None, None)),
            ("ended", "Finalizing", left(Some(id), None, Some(clock.seconds - 1.0))),
            ("program", "Running", left(None, Some(gone), None)),
            ("nothing", "Running", InFlight::default()),
        ] {
            let here = dir.join(name);
            let library = here.join("library");
            let sessions = obelisk::sessions_dir(&library);
            std::fs::create_dir_all(&sessions).unwrap();
            std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
            let record = sessions.join(format!("{id}.v2.json"));
            let write = |text: &str| {
                std::fs::write(&record, text).unwrap();
                std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
            };
            // (a session that prepares has no game yet, only its supervisor)
            let game = if phase == "Running" { format!(r#","game":{}"#, person(me)) } else { String::new() };
            write(&format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"{phase}","supervisor":{}{game}}}"#, person(me)));
            let gate = here.join("gate.json");
            let lock = gate.with_extension("lock");
            std::fs::write(&lock, if name == "nothing" { String::new() } else { note.to_json() }).unwrap();
            let quick = fake_program(&here.join("bin"), &[], 0);
            for expired in [false, true] {
                let play = Arc::new(Play::new(gate.clone()));
                if expired {
                    // Any wait there was is over.
                    std::fs::write(&gate, Kept { boot: clock.boot.clone(), until: Some(clock.seconds - 1.0), seen: Vec::new() }.to_json()).unwrap();
                }
                assert_eq!(play.reserve(&library, None), Err(Refused::Busy), "{name}, its wait over: {expired}");
                assert_eq!(play.start(&quick, &library, &[]), Err(Refused::Busy), "{name}");
            }
            // It was played to its end: nothing is held.
            write(&format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Finished","played":{{"seconds":4000,"frames":{{"count":96000}}}}}}"#));
            let play = Play::new(gate.clone());
            let token = play.reserve(&library, None).unwrap_or_else(|refused| panic!("{name}: {refused:?}"));
            play.unreserve(token);
            assert_eq!(std::fs::read_to_string(&lock).unwrap(), "", "{name}: settled");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The session is said in the slot as soon as the program names it, before its game runs, and the program itself from the
    /// moment it is started: a holder gone then leaves both, and while either lives nothing is admitted.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_session_is_said_as_soon_as_the_program_names_it() {
        let dir = std::env::temp_dir().join(format!("orbiters-slot-starting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let id = "f16d947e-7d8f-4331-8731-bede005d29fe";
        for named_session in [true, false] {
            let here = dir.join(named_session.to_string());
            let said = if named_session { format!(r#"{{"event":"starting","app_id":438100,"session_id":"{id}"}}"#) } else { r#"{"event":"preparing","app_id":438100}"#.to_string() };
            let program = fake_program(&here.join("bin"), &[&said], 3);
            let gate = here.join("gate.json");
            let a = Arc::new(Play::new(gate.clone()));
            let launch = a.start(&program, &library, &[]).unwrap();
            if named_session {
                assert_eq!(wait_for(&a, launch, |l| l.session.is_some()).session.as_deref(), Some(id));
                assert!(!a.launch(launch).unwrap().running, "its game does not run yet");
            }
            let note = InFlight::read(std::fs::read_to_string(gate.with_extension("lock"))).unwrap();
            assert_eq!((note.session.is_some(), note.program.map(|p| p.0)), (named_session, a.program()));
            dies(&a);
            let b = Play::new(gate.clone());
            assert_eq!(b.reserve(&library, None), Err(Refused::Busy), "the program still makes it");
            let _ = wait_for(&a, launch, |l| l.over);
            assert!(matches!(b.reserve(&library, None), Err(Refused::Wait(_))), "gone without a word: as an early end");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A slot file that cannot be written is named with what ends it, and is used again in its place once it can be.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_slot_file_that_cannot_be_written_is_named_with_its_remedy() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-slot-readonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("gate.lock");
        std::fs::write(&lock, "").unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444)).unwrap();
        let play = Arc::new(Play::new(dir.join("gate.json")));
        let fake = fake_program(&dir.join("fake"), &[r#"{"event":"error","code":"launch_failed","message":"failure before game"}"#], 0);
        let Err(refused) = play.start(&fake, &dir, &[]) else { panic!("it cannot be written") };
        let words = refused.words();
        assert!(words.contains(lock.to_str().unwrap()) && words.contains("permission"), "{words}");
        assert!(slot_refused(&lock, &std::io::Error::from(std::io::ErrorKind::StorageFull)).words().contains("Free some space"));
        assert!(slot_refused(&lock, &std::io::Error::other("odd")).words().contains("odd"));
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
        let launch = play.start(&fake, &dir, &[]).unwrap();
        let _ = wait_for(&play, launch, |l| l.over);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "", "the same file, in its place");
        let token = play.reserve(&dir, None).unwrap();
        play.unreserve(token);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ninth verification (the verifier's fixture): the timer of a wait that was written since lets go of nothing that holds the
    /// slot by then: a decision under way, another wait.
    #[test]
    fn the_timer_of_an_earlier_wait_ends_that_wait_only() {
        let dir = std::env::temp_dir().join(format!("orbiters-slot-timer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let new = || Play::new(dir.join("gate.json"));
        let soon = || std::time::Instant::now() + Duration::from_millis(300);
        // Written since, then a decision.
        let (a, b) = (new(), new());
        a.hold_slot(soon());
        a.slot.lock().unwrap().unwritten = None;
        a.let_slot_go();
        assert!(b.take_slot_within(Duration::ZERO).is_ok(), "written: free");
        b.let_slot_go();
        a.take_slot_within(Duration::ZERO).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(b.take_slot_within(Duration::ZERO), Err(Refused::Busy), "the decision is still under way");
        a.let_slot_go();
        // Not written, its time over while a decision is under way.
        a.hold_slot(soon());
        a.take_slot_within(Duration::ZERO).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(b.take_slot_within(Duration::ZERO), Err(Refused::Busy));
        a.let_slot_go();
        assert!(b.take_slot_within(Duration::ZERO).is_ok(), "and then it holds nothing");
        b.let_slot_go();
        // A later wait outlives the timer of an earlier one.
        a.hold_slot(soon());
        a.hold_slot(std::time::Instant::now() + Duration::from_millis(1200));
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(b.take_slot_within(Duration::ZERO), Err(Refused::Busy));
        std::thread::sleep(Duration::from_millis(900));
        assert!(b.take_slot_within(Duration::ZERO).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seventh verification: the slot is let go of only once how the launch ended is known and kept, so that no other
    /// coordinator decides before it; one whose supervisor and game are both gone counts as an early end, and never holds it
    /// for ever.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_slot_is_kept_until_how_a_launch_ended_is_known_and_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-slot-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let sessions = obelisk::sessions_dir(&library);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
        let id = "f16d947e-7d8f-4331-8731-bede005d29fe";
        let record = sessions.join(format!("{id}.v2.json"));
        let write = |text: &str| {
            std::fs::write(&record, text).unwrap();
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        let (seconds, micros) = super::super::bridge_mac::born(std::process::id()).unwrap();
        let running = |seconds: u64| {
            let me = format!(r#"{{"pid":{},"birth_seconds":{seconds},"birth_microseconds":{micros}}}"#, std::process::id());
            format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Running","supervisor":{me},"game":{me}}}"#)
        };
        let program = fake_program(&dir.join("bin"), &[&format!(r#"{{"event":"running","app_id":438100,"pid":4242,"session_id":"{id}"}}"#)], 1);
        for ends in [true, false] {
            let gate = dir.join(format!("gate-{ends}.json"));
            let (a, b) = (Arc::new(Play::new(gate.clone())), Arc::new(Play::new(gate.clone())));
            let _ = std::fs::remove_file(&record);
            let launch = a.start(&program, &library, &[]).unwrap();
            write(&running(seconds));
            assert!(wait_for(&a, launch, |l| l.over).unreconciled);
            assert_eq!(a.attached(), None);
            assert_eq!(a.gate(&library), Ok(None));
            assert_eq!(b.reserve(&library, None), Err(Refused::Busy), "its end is not known yet");
            // Eighth verification: nor does its own coordinator start another over it, for Home or for the bridge, but to close
            // that very game first.
            assert_eq!(a.reserve(&library, None), Err(Refused::Busy));
            assert_eq!(a.start(&program, &library, &[]), Err(Refused::Busy));
            assert_eq!(a.reserve(&library, Some(launch + 1)), Err(Refused::Busy), "another's is not its own");
            let token = a.reserve(&library, Some(launch)).unwrap();
            assert_eq!(a.start_reserved(&program, &library, &[], Some(token)), Err(Refused::Busy), "not closed yet");
            assert_eq!(b.reserve(&library, None), Err(Refused::Busy), "and it is kept");
            match ends {
                true => write(&format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Finished","played":{{"seconds":40,"frames":{{"count":96}}}}}}"#)),
                // Another birth: the process the record names is gone.
                false => write(&running(seconds - 100)),
            }
            std::thread::sleep(Duration::from_millis(5));
            assert!(matches!(a.gate(&library), Ok(Some(_))), "{ends}");
            assert!(!a.launch(launch).unwrap().unreconciled);
            assert!(std::fs::read_to_string(&gate).unwrap().contains("\"until\":"), "{ends}");
            assert!(matches!(b.reserve(&library, None), Err(Refused::Wait(_))), "{ends}: free, and the wait is there");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M9: a program lost while the engine's record says the game runs is read again later: once the record says it ended
    /// early, the wait after an early end holds and its failure is there to tell.
    #[test]
    fn a_launch_whose_program_was_lost_is_reconciled_from_its_record() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-play-reconcile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = dir.join("library");
        let sessions = obelisk::sessions_dir(&library);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
        let id = "f16d947e-7d8f-4331-8731-bede005d29fe";
        let record = sessions.join(format!("{id}.v2.json"));
        let write = |text: &str| {
            std::fs::write(&record, text).unwrap();
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        write(&format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Running","game":{{"pid":4242}}}}"#));
        let program = fake_program(&dir, &[&format!(r#"{{"event":"running","app_id":438100,"pid":4242,"session_id":"{id}"}}"#)], 0);
        let play = Arc::new(Play::default());
        let launch = play.start(&program, &library, &[]).unwrap();
        let lost = wait_for(&play, launch, |l| l.over);
        assert!(lost.unreconciled && lost.failure.is_none(), "{lost:?}");
        assert!(play.gate(&library).unwrap().is_none(), "nothing known to have ended early yet");
        write(&format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Finished","played":{{"seconds":40,"frames":{{"count":96}}}}}}"#));
        std::thread::sleep(Duration::from_millis(5));
        assert!(matches!(play.gate(&library), Ok(Some(_))), "ended early: the wait holds");
        let reconciled = play.launch(launch).unwrap();
        assert_eq!(reconciled.ended, Some((Some(40), Some(96))));
        assert_eq!(play.take_failure(launch).map(|f| f.code), Some("ended_at_once".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_command_is_held_to_its_time_and_to_its_success() {
        let dir = std::env::temp_dir().join(format!("orbiters-run-for-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Says nothing on its output, closes it, and goes on: the time still counts after the output ended.
        let closes = fake_program(&dir.join("closes"), &[], 0);
        std::fs::write(&closes, "#!/bin/sh\nexec 1>&-\nsleep 30\n").unwrap();
        let begun = std::time::Instant::now();
        assert!(run_for(&closes, &[], Duration::from_millis(500), |_| {}).is_err());
        assert!(begun.elapsed() < Duration::from_secs(5));
        // Fails without a word: not a success.
        let fails = fake_program(&dir.join("fails"), &[], 0);
        std::fs::write(&fails, "#!/bin/sh\nexit 3\n").unwrap();
        assert!(run_for(&fails, &[], Duration::from_secs(5), |_| {}).is_err());
        // M14: one that never stops talking is held to the same deadline.
        let talker = fake_program(&dir.join("talker"), &[], 0);
        std::fs::write(&talker, "#!/bin/sh\nwhile :; do echo '{\"event\":\"x\"}'; done\n").unwrap();
        let begun = std::time::Instant::now();
        assert!(run_for(&talker, &[], Duration::from_millis(500), |_| {}).is_err());
        assert!(begun.elapsed() < Duration::from_secs(3), "{:?}", begun.elapsed());
        let fine = fake_program(&dir.join("fine"), &[r#"{"event":"done"}"#], 0);
        let mut heard = 0;
        assert!(run_for(&fine, &[], Duration::from_secs(5), |_| heard += 1).is_ok());
        assert_eq!(heard, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Sixth verification, ruling 1: a disk that exchanges nothing is said with its path, never as a download to try again.
    #[test]
    fn a_disk_that_cannot_be_repaired_in_place_is_said_with_its_path() {
        let failure = Failure::said("voile_failed", "the Voile runtime could not be installed: backend.voile.noExchange (/Volumes/a (disk)/Voile)", None);
        assert_eq!(failure.code, "no_exchange");
        assert!(failure.words.contains("/Volumes/a (disk)/Voile"), "{}", failure.words);
        assert_eq!(Failure::said("voile_failed", "the Voile runtime could not be installed: backend.voile.damaged", None).code, "voile_failed");
    }

    #[test]
    fn a_failure_reads_as_something_the_person_can_act_on() {
        assert_eq!(words("something_new", "The engine said this."), "The engine said this.");
        assert_eq!(words("something_new", ""), "something_new");
        assert_ne!(words("not_owned", "x"), "x");
    }
}
