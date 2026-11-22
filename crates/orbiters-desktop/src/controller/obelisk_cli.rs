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

/// A VRChat being started, or running, by this N4N0: the program stays attached until the game ends.
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
    /// Why the last start failed, until it is told.
    failure: Option<Failure>,
}

impl Play {
    /// Starts VRChat with `args`; the events are followed on a thread of their own. An error when the program cannot
    /// be run at all, or a start is already under way.
    pub fn start(self: &Arc<Self>, program: &Path, args: &[String]) -> Result<(), String> {
        let mut now = self.now.lock().unwrap();
        if now.attached {
            return Err(tr("VRChat is already starting.").into());
        }
        let mut command: Vec<&str> = vec!["steam", "play", obelisk::STEAM_APP];
        if !args.is_empty() {
            command.push("--");
            command.extend(args.iter().map(String::as_str));
        }
        let mut child = spawn(program, &command).map_err(|e| e.to_string())?;
        *now = Now { attached: true, running: None, failure: None };
        drop(now);
        let play = self.clone();
        let followed = std::thread::Builder::new().name("obelisk-play".into()).spawn(move || {
            let out = child.stdout.take().expect("piped");
            let mut failure = None;
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                match obelisk::event(&line) {
                    Some(Event::Running { pid }) => play.now.lock().unwrap().running = Some(pid),
                    Some(Event::Exited { .. }) => play.now.lock().unwrap().running = None,
                    // A failure after the game ran (its session, its Cloud saves) is not a start that failed.
                    Some(Event::Error { code, message }) => {
                        note(&["steam", "play"], &code, &message);
                        if play.now.lock().unwrap().running.is_none() {
                            failure = Some(Failure::new(&code, words(&code, &message)));
                        }
                    }
                    _ => {}
                }
            }
            let _ = child.wait();
            let mut now = play.now.lock().unwrap();
            *now = Now { attached: false, running: None, failure };
        });
        if followed.is_err() {
            self.now.lock().unwrap().attached = false;
            return Err(tr("VRChat could not be followed.").into());
        }
        Ok(())
    }

    /// A start is under way and the game is not up yet (the engine or an update of VRChat may download first).
    pub fn starting(&self) -> bool {
        let now = self.now.lock().unwrap();
        now.attached && now.running.is_none()
    }

    /// The process the program said the game runs under, while it does.
    pub fn running(&self) -> Option<Option<u32>> {
        self.now.lock().unwrap().running
    }

    /// Why the last start failed: told once.
    pub fn failure(&self) -> Option<Failure> {
        self.now.lock().unwrap().failure.take()
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

    #[test]
    fn a_failure_reads_as_something_the_person_can_act_on() {
        assert_eq!(words("something_new", "The engine said this."), "The engine said this.");
        assert_eq!(words("something_new", ""), "something_new");
        assert_ne!(words("not_owned", "x"), "x");
    }
}
