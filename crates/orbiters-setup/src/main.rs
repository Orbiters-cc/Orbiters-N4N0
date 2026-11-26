//! Orbiters - N4N0's setup: one program that installs it for the person (in `%LOCALAPPDATA%\Programs`, no
//! administrator rights), updates it, and uninstalls it, in the Orbiters look.
//!
//! - `OrbitersSetup-<version>.exe`: the programs inside ([`payload`]); installs them, or updates an install.
//! - `--update [--dir <folder>] [--relaunch] [--hidden]`: what the app runs to update itself, once it has quit: a
//!   small window while the files are replaced, then the app again (in the tray with `--hidden`).
//! - `--uninstall [--dir <folder>]`: what Windows' installed apps run (`orbiters-setup.exe`, the setup without
//!   programs inside, kept in the install folder). It copies itself out of the folder first, to be able to remove it.
//! - `--recover [--dir <folder>]`: what Windows runs at the next sign-in when an update was cut ([`swap`]): the version
//!   that was there is put back, without a window.
//! - `--silent`: no window (default choices; a running app is closed); `--dir` picks the folder of an install.
//! - `--screenshots <folder> [--language <tag>]`: every page rendered offscreen, in a language.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod install;
#[cfg(windows)]
mod swap;
#[cfg(windows)]
mod system;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
use orbiters_i18n::{tr, Language};
use std::path::PathBuf;

/// What the setup was started to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Install,
    Update,
    Uninstall,
}

#[derive(Clone, Debug)]
pub struct Args {
    pub mode: Mode,
    pub silent: bool,
    pub dir: Option<PathBuf>,
    /// Start the app once updated.
    pub relaunch: bool,
    /// ...in the tray, without its window (it was started with Windows).
    pub hidden: bool,
    /// This is the copy an uninstall made of itself outside the install folder.
    pub relocated: bool,
    /// This is an automatic update tried again at sign-in: it runs only when it is still newer than what is installed.
    pub retry: bool,
    /// Only puts back what an update that was cut left.
    pub recover: bool,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut parsed = Args { mode: Mode::Install, silent: false, dir: None, relaunch: false, hidden: false, relocated: false, retry: false, recover: false };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--update" => parsed.mode = Mode::Update,
                "--uninstall" => parsed.mode = Mode::Uninstall,
                "--silent" | "/S" => parsed.silent = true,
                "--relaunch" => parsed.relaunch = true,
                "--hidden" => parsed.hidden = true,
                "--relocated" => parsed.relocated = true,
                "--retry" => parsed.retry = true,
                "--recover" => parsed.recover = true,
                "--dir" => parsed.dir = args.next().map(PathBuf::from),
                _ => {}
            }
        }
        parsed
    }
}

/// The programs inside this setup (`cargo xtask dist`); none in the uninstaller kept in the install folder.
pub fn payload() -> Option<&'static [u8]> {
    #[cfg(payload)]
    {
        Some(include_bytes!(env!("ORBITERS_SETUP_PAYLOAD_FILE")))
    }
    #[cfg(not(payload))]
    {
        None
    }
}

/// The language the setup speaks, for its window and its message boxes: the one the app saved when it is installed here (what the account gave or the
/// person picked), else this computer's. Nobody is signed in to the setup itself.
#[cfg(windows)]
pub fn language_wanted() -> Language {
    let saved = orbiters_update::app_settings_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|settings| settings["language"].as_str().and_then(orbiters_i18n::normalize));
    saved.or_else(|| orbiters_i18n::normalize(&orbiters_ui::fonts::user_locale())).unwrap_or(Language::En)
}

/// Takes the language of [`language_wanted`], and the kanji forms of Japanese for it.
#[cfg(windows)]
pub fn speak() {
    orbiters_i18n::set_language(language_wanted());
    orbiters_ui::fonts::prefer_language(orbiters_i18n::language().tag());
}

/// Four busy threads and no GPU, until its file is gone or a few minutes passed, so that nothing ever has to kill it.
#[cfg(windows)]
fn test_load(running: Option<PathBuf>) {
    const AT_MOST: std::time::Duration = std::time::Duration::from_secs(300);
    let began = std::time::Instant::now();
    let go_on = move || began.elapsed() < AT_MOST && running.as_ref().is_some_and(|file| file.exists());
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let go_on = go_on.clone();
            std::thread::spawn(move || {
                let mut value = 1u64;
                while go_on() {
                    for _ in 0..200_000 {
                        value = value.wrapping_mul(0x0100_0000_01b3).rotate_left(7);
                    }
                    std::hint::black_box(value);
                }
            })
        })
        .collect();
    for thread in threads {
        let _ = thread.join();
    }
}

#[cfg(windows)]
fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    // What an uninstall left to do at the next sign-in, from a copy of this program: no window, and nothing else.
    if raw.first().map(String::as_str) == Some(orbiters_booster::system::RESTORE_ARGUMENT) {
        let copy = orbiters_booster::system::restorer_copy_path();
        let store = orbiters_booster::system::store_for_user(&copy);
        if orbiters_booster::system::run_restorer(&store) && store.remove_if_empty() {
            // Nothing owed any more: this copy goes too, once it has ended.
            if std::env::current_exe().ok().is_some_and(|me| me.to_string_lossy().eq_ignore_ascii_case(&copy.to_string_lossy())) {
                // And its folder, empty by then, and the Orbiters folder if nothing else of Orbiters' is in it.
                let folder = orbiters_booster::data_dir();
                let parent = folder.parent().map(std::path::Path::to_path_buf);
                let mut folders = vec![folder.as_path()];
                folders.extend(parent.as_deref());
                system::delete_after_exit_with(&copy, &folders);
            }
        }
        return;
    }
    // Test builds' test load for the FPS booster's step 1, as a copy of this program under an allowlisted name.
    if raw.first().map(String::as_str) == Some("--booster-test-load") {
        test_load(raw.get(1).map(PathBuf::from));
        return;
    }
    speak();
    // For review: `--language fr` stays in that language whatever the computer and the app say (the screenshots in each language).
    if let Some(language) = raw.iter().position(|a| a == "--language").and_then(|i| raw.get(i + 1)).and_then(|tag| orbiters_i18n::normalize(tag)) {
        orbiters_i18n::pin_language(language);
        orbiters_ui::fonts::prefer_language(language.tag());
    }
    let payload = payload();
    // For review: every page rendered offscreen.
    if let Some(i) = raw.iter().position(|a| a == "--screenshots") {
        let dir = raw.get(i + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("setup-screenshots"));
        if let Err(e) = ui::screenshots(&dir, payload) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    let mut args = Args::parse(raw);
    if args.recover {
        std::process::exit(recover(&args));
    }
    // The uninstaller started without asking for anything: it is there to uninstall.
    if payload.is_none() && args.mode == Mode::Install {
        args.mode = Mode::Uninstall;
    }
    if args.mode == Mode::Uninstall && !args.relocated {
        std::process::exit(relocate(&args));
    }
    let code = if args.silent {
        silent(&args, payload)
    } else {
        match ui::run(args.clone(), payload) {
            Ok(code) => code,
            // No window (no usable graphics): Windows' own message boxes instead.
            Err(e) => {
                eprintln!("the setup's window could not open: {e:#}");
                fallback(&args, payload)
            }
        }
    };
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    let _ = (Args::parse(std::env::args().skip(1)), payload());
    eprintln!("Orbiters - N4N0's setup is for Windows.");
    std::process::exit(1);
}

/// The install folder to uninstall or update: `--dir`, the folder this program is in when it holds an install, or
/// where Windows' installed apps say it is.
#[cfg(windows)]
pub fn install_dir(args: &Args) -> Option<PathBuf> {
    use orbiters_update::InstallRecord;
    if let Some(dir) = &args.dir {
        return Some(dir.clone());
    }
    // A record there that cannot be read now still marks the install (W1-1): what is done with it then says it cannot be read,
    // and a recovery at sign-in is scheduled again, instead of "not installed".
    let holds = |d: &PathBuf| !matches!(InstallRecord::load(d), Ok(None));
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).filter(holds);
    beside.or_else(system::registered_dir).filter(holds)
}

/// The folder a new install goes to, unless the person picks another: where it already is, or the default.
#[cfg(windows)]
pub fn target_dir(args: &Args) -> PathBuf {
    args.dir
        .clone()
        .or_else(|| system::registered_dir().filter(|d| !matches!(orbiters_update::InstallRecord::load(d), Ok(None))))
        .unwrap_or_else(orbiters_update::default_install_dir)
}

/// Starts a copy of this program outside the install folder to uninstall, so that the folder (with this program in
/// it) can be removed; the copy removes itself once done.
#[cfg(windows)]
fn relocate(args: &Args) -> i32 {
    let Some(dir) = install_dir(args) else {
        if !args.silent {
            system::message(tr("Orbiters - N4N0 is not installed for this Windows account."), false, false);
        }
        return 1;
    };
    let copy = std::env::temp_dir().join(format!("orbiters-uninstall-{}.exe", std::process::id()));
    let started = std::env::current_exe().and_then(|me| std::fs::copy(me, &copy)).and_then(|_| {
        let dir = dir.display().to_string();
        let mut list = vec!["--uninstall", "--relocated", "--dir", dir.as_str()];
        if args.silent {
            list.push("--silent");
        }
        system::launch(&copy, &list)
    });
    match started {
        Ok(()) => 0,
        Err(e) => {
            if !args.silent {
                system::message(&orbiters_i18n::tr!("The uninstall could not start: {error}", error = e), false, true);
            }
            1
        }
    }
}

/// Puts back what an update that was cut left, at the sign-in after it. The app, which could not start with Windows meanwhile,
/// starts if it is to.
#[cfg(windows)]
fn recover(args: &Args) -> i32 {
    let Some(dir) = install_dir(args) else { return 1 };
    match swap::recover(&dir) {
        Ok(found) => {
            if found && system::starts_with_windows_from(&dir) && !system::app_runs_from(&dir) {
                relaunch(&dir, true);
            }
            0
        }
        Err(e) => {
            eprintln!("{e:#}");
            // Tried again at the next one; the next setup tries too.
            if let Some(command) = std::env::current_exe().ok().and_then(|me| swap::recover_command(&me, &dir)) {
                let _ = system::schedule_recovery(&command);
            }
            1
        }
    }
}

/// Without a window: the default choices, a running app closed, errors on stderr.
#[cfg(windows)]
fn silent(args: &Args, payload: Option<&'static [u8]>) -> i32 {
    // A retry at sign-in (verifier round 2): only while what it holds is still newer than what is installed; an older
    // or the same version (the app updated meanwhile) is never installed again, and the retry is dropped.
    if args.retry && args.mode == Mode::Update {
        let holds = payload.and_then(|p| orbiters_update::payload::read_index(p).ok()).map(|i| i.version);
        let dir = install_dir(args);
        let installed = match dir.as_deref().map(orbiters_update::InstallRecord::load) {
            Some(Ok(record)) => record.map(|r| r.version),
            Some(Err(e)) => {
                eprintln!("the install record cannot be read now: {e}"); // i18n: not shown (a log line)
                retry_update_at_sign_in(args);
                return 1;
            }
            None => None,
        };
        if !retry_is_due(holds.as_deref(), installed.as_deref()) {
            system::cancel_update_retry();
            return 0;
        }
    }
    let progress = install::Progress::default();
    let result = run_task(args, payload, &progress, true, false);
    after_update(args, &result);
    if let Err(e) = &result {
        eprintln!("{e:#}");
    }
    result.is_err() as i32
}

/// Whether a retried update holding `holds` should run over the install of `installed`: only for a newer version.
fn retry_is_due(holds: Option<&str>, installed: Option<&str>) -> bool {
    matches!((holds, installed), (Some(h), Some(i)) if orbiters_update::is_newer(h, i))
}

/// After an update without a window (silent, or Windows' message boxes): one the app kept from finishing is tried again
/// at the next sign-in; one that went through drops any retry left.
#[cfg(windows)]
fn after_update(args: &Args, result: &anyhow::Result<()>) {
    if args.mode != Mode::Update {
        return;
    }
    match result {
        // The app kept running: nothing changed, and the same update is tried again at the next sign-in.
        Err(e) if e.downcast_ref::<install::StillRunning>().is_some() => retry_update_at_sign_in(args),
        Ok(()) => system::cancel_update_retry(),
        Err(_) => {}
    }
}

#[cfg(windows)]
fn retry_update_at_sign_in(args: &Args) {
    if let (Ok(me), Some(dir)) = (std::env::current_exe(), install_dir(args)) {
        if let Err(e) = system::schedule_update_retry(&retry_command(&me, &dir, args)) {
            eprintln!("the update could not be scheduled again: {e:#}"); // i18n: not shown (a log line)
        }
    }
}

/// The command line that tries the update of `dir` again, silently, as `args` asked for it.
#[cfg(windows)]
fn retry_command(setup: &std::path::Path, dir: &std::path::Path, args: &Args) -> String {
    let mut line = format!("\"{}\" --update --silent --retry --dir \"{}\"", setup.display(), dir.display());
    if args.relaunch {
        line.push_str(" --relaunch");
    }
    if args.hidden {
        line.push_str(" --hidden");
    }
    line
}

/// The task of `args` as `--silent` and the message boxes run it.
#[cfg(windows)]
fn run_task(args: &Args, payload: Option<&'static [u8]>, progress: &install::Progress, silent: bool, remove_data: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let result = match args.mode {
        Mode::Install => {
            let bytes = payload.with_context(|| tr("This setup holds no programs to install."))?;
            let options = install::Options {
                dir: target_dir(args),
                desktop_shortcut: true,
                start_with_windows: false,
                start_with_steamvr: false,
                channel: orbiters_update::STABLE.into(),
            };
            let fresh = orbiters_update::InstallRecord::load(&options.dir)
                .map_err(|e| swap::unreadable(&options.dir.join(orbiters_update::RECORD_FILE), e))?
                .is_none();
            install::install(bytes, &options, progress, silent, fresh).map(|_| options.dir)
        }
        Mode::Update => {
            let bytes = payload.with_context(|| tr("This setup holds no programs to install."))?;
            let dir = install_dir(args).with_context(|| tr("Orbiters - N4N0 is not installed for this Windows account."))?;
            install::update(bytes, &dir, progress, silent).map(|_| dir)
        }
        Mode::Uninstall => {
            let dir = install_dir(args).with_context(|| tr("Orbiters - N4N0 is not installed for this Windows account."))?;
            let r = install::uninstall(&dir, remove_data, progress, silent).map(|_| dir);
            done_relocated(args);
            return r.map(drop);
        }
    };
    for note in progress.status().notes {
        eprintln!("{note}");
    }
    // After a failed update, relaunch checks that recovery left a complete installation.
    if args.mode == Mode::Update && args.relaunch {
        if let Some(dir) = install_dir(args).filter(|d| result.is_ok() || !system::app_runs_from(d)) {
            relaunch(&dir, args.hidden);
        }
    }
    result.map(drop)
}

/// The app again, after an update: only from a folder holding one version whole (W1-4). After a put-back that could not end, the
/// old program would start beside new files, and hold them while the next recovery tries to end it.
#[cfg(windows)]
pub fn relaunch(dir: &std::path::Path, hidden: bool) {
    relaunch_with(dir, hidden, system::launch);
}

/// [`relaunch`], with what starts a program (`launch`: Windows', or a test's that starts nothing).
#[cfg(windows)]
pub(crate) fn relaunch_with(dir: &std::path::Path, hidden: bool, launch: impl FnOnce(&std::path::Path, &[&str]) -> std::io::Result<()>) {
    if !swap::whole(dir) {
        eprintln!("Orbiters - N4N0 is not started again: {} holds no whole version", dir.display()); // i18n: not shown (a log line)
        return;
    }
    let args: &[&str] = if hidden { &["--hidden"] } else { &[] };
    if let Err(e) = launch(&dir.join(orbiters_update::APP_EXE), args) {
        eprintln!("Orbiters - N4N0 could not be started again: {e}");
    }
}

/// The uninstaller's copy outside the install folder goes once it has finished.
#[cfg(windows)]
pub fn done_relocated(args: &Args) {
    if args.relocated {
        if let Ok(me) = std::env::current_exe() {
            system::delete_after_exit(&me);
        }
    }
}

/// The setup in Windows' message boxes, for a computer where its window cannot open.
#[cfg(windows)]
fn fallback(args: &Args, payload: Option<&'static [u8]>) -> i32 {
    let version = payload.and_then(|p| orbiters_update::payload::read_index(p).ok()).map(|i| i.version).unwrap_or_default();
    let ask = match args.mode {
        Mode::Install => Some(orbiters_i18n::tr!("Install Orbiters - N4N0 {version} in {folder}?\n\nIf it is running, it closes first.", version = version, folder = target_dir(args).display())),
        Mode::Uninstall => Some(tr("Uninstall Orbiters - N4N0?\n\nYour sessions and settings are kept.").to_string()),
        Mode::Update => None,
    };
    if let Some(question) = ask {
        if !system::message(&question, true, false) {
            done_relocated(args);
            return 1;
        }
    }
    let progress = install::Progress::default();
    let result = run_task(args, payload, &progress, true, false);
    // The same retry rule as the silent update (verifier round 2: the message boxes skipped it).
    after_update(args, &result);
    match result {
        Ok(()) => {
            match args.mode {
                Mode::Install => {
                    let notes = progress.status().notes;
                    let said = if notes.is_empty() {
                        orbiters_i18n::tr!("Orbiters - N4N0 {version} is installed. Find it in the Start menu.", version = version)
                    } else {
                        format!("{}\n\n{}", orbiters_i18n::tr!("Version {version} is in place.", version = version), notes.join("\n"))
                    };
                    system::message(&said, false, false);
                    relaunch(&target_dir(args), false);
                }
                Mode::Uninstall => {
                    system::message(tr("Orbiters - N4N0 is uninstalled."), false, false);
                }
                Mode::Update => {}
            }
            0
        }
        Err(e) => {
            system::message(&format!("{e:#}"), false, true);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retry a waiting automatic update schedules is the same update, silent, with what it was asked for.
    #[cfg(windows)]
    #[test]
    fn a_waiting_update_is_tried_again_the_same_way() {
        let args = Args::parse(["--update", "--silent", "--relaunch", "--hidden"].map(String::from));
        let line = retry_command(std::path::Path::new(r"C:\U\setup.exe"), std::path::Path::new(r"C:\P\Orbiters - N4N0"), &args);
        assert_eq!(line, r#""C:\U\setup.exe" --update --silent --retry --dir "C:\P\Orbiters - N4N0" --relaunch --hidden"#);
        assert!(Args::parse(["--update", "--silent", "--retry"].map(String::from)).retry);
    }

    /// Verifier round 2: a retry left from an older update never puts that older (or the same) version back over a
    /// newer install; it runs only while still newer.
    #[test]
    fn a_retry_never_installs_an_older_or_the_same_version() {
        assert!(retry_is_due(Some("0.3.6-dev.26"), Some("0.3.6-dev.25")));
        assert!(!retry_is_due(Some("0.3.6-dev.25"), Some("0.3.6-dev.25")));
        assert!(!retry_is_due(Some("0.3.6-dev.24"), Some("0.3.6-dev.30")));
        assert!(!retry_is_due(Some("0.3.6-dev.26"), None), "nothing installed: no retry");
        assert!(!retry_is_due(None, Some("0.3.6")));
    }

    #[test]
    fn arguments_pick_the_mode() {
        let a = Args::parse(["--update", "--relaunch", "--hidden", "--dir", "C:/x"].map(String::from));
        assert_eq!(a.mode, Mode::Update);
        assert!(a.relaunch && a.hidden && !a.silent);
        assert_eq!(a.dir, Some(PathBuf::from("C:/x")));
        let u = Args::parse(["--uninstall", "--silent"].map(String::from));
        assert_eq!(u.mode, Mode::Uninstall);
        assert!(u.silent && !u.relocated);
        assert_eq!(Args::parse(Vec::new()).mode, Mode::Install);
        let r = Args::parse(["--recover", "--dir", "C:/x"].map(String::from));
        assert!(r.recover && r.dir == Some(PathBuf::from("C:/x")) && !a.recover && !u.recover);
    }
}
