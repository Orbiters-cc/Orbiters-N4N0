//! Orbiters - N4N0's setup: one program that installs it for the person (in `%LOCALAPPDATA%\Programs`, no
//! administrator rights), updates it, and uninstalls it, in the Orbiters look.
//!
//! - `OrbitersSetup-<version>.exe`: the programs inside ([`payload`]); installs them, or updates an install.
//! - `--update [--dir <folder>] [--relaunch] [--hidden]`: what the app runs to update itself, once it has quit: a
//!   small window while the files are replaced, then the app again (in the tray with `--hidden`).
//! - `--uninstall [--dir <folder>]`: what Windows' installed apps run (`orbiters-setup.exe`, the setup without
//!   programs inside, kept in the install folder). It copies itself out of the folder first, to be able to remove it.
//! - `--silent`: no window (default choices; a running app is closed); `--dir` picks the folder of an install.
//! - `--screenshots <folder> [--language <tag>]`: every page rendered offscreen, in a language.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod install;
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
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut parsed = Args { mode: Mode::Install, silent: false, dir: None, relaunch: false, hidden: false, relocated: false };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--update" => parsed.mode = Mode::Update,
                "--uninstall" => parsed.mode = Mode::Uninstall,
                "--silent" | "/S" => parsed.silent = true,
                "--relaunch" => parsed.relaunch = true,
                "--hidden" => parsed.hidden = true,
                "--relocated" => parsed.relocated = true,
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
                system::delete_after_exit(&copy);
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
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).filter(|d| InstallRecord::read(d).is_some());
    beside.or_else(system::registered_dir).filter(|d| InstallRecord::read(d).is_some())
}

/// The folder a new install goes to, unless the person picks another: where it already is, or the default.
#[cfg(windows)]
pub fn target_dir(args: &Args) -> PathBuf {
    args.dir
        .clone()
        .or_else(|| system::registered_dir().filter(|d| orbiters_update::InstallRecord::read(d).is_some()))
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

/// Without a window: the default choices, a running app closed, errors on stderr.
#[cfg(windows)]
fn silent(args: &Args, payload: Option<&'static [u8]>) -> i32 {
    let progress = install::Progress::default();
    let result = run_task(args, payload, &progress, true, false);
    if let Err(e) = &result {
        eprintln!("{e:#}");
    }
    result.is_err() as i32
}

/// The task of `args` as `--silent` and the message boxes run it.
#[cfg(windows)]
fn run_task(args: &Args, payload: Option<&'static [u8]>, progress: &install::Progress, silent: bool, remove_data: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let result = match args.mode {
        Mode::Install => {
            let bytes = payload.context("This setup holds no programs to install.")?;
            let options = install::Options {
                dir: target_dir(args),
                desktop_shortcut: true,
                start_with_windows: false,
                start_with_steamvr: false,
                channel: orbiters_update::STABLE.into(),
            };
            let fresh = orbiters_update::InstallRecord::read(&options.dir).is_none();
            install::install(bytes, &options, progress, silent, fresh).map(|_| options.dir)
        }
        Mode::Update => {
            let bytes = payload.context("This setup holds no programs to install.")?;
            let dir = install_dir(args).context("Orbiters - N4N0 is not installed for this Windows account.")?;
            install::update(bytes, &dir, progress, silent).map(|_| dir)
        }
        Mode::Uninstall => {
            let dir = install_dir(args).context("Orbiters - N4N0 is not installed for this Windows account.")?;
            let r = install::uninstall(&dir, remove_data, progress, silent).map(|_| dir);
            done_relocated(args);
            return r.map(drop);
        }
    };
    let dir = result?;
    if args.mode == Mode::Update && args.relaunch {
        relaunch(&dir, args.hidden);
    }
    Ok(())
}

/// The app again, after an update.
#[cfg(windows)]
pub fn relaunch(dir: &std::path::Path, hidden: bool) {
    let args: &[&str] = if hidden { &["--hidden"] } else { &[] };
    if let Err(e) = system::launch(&dir.join(orbiters_update::APP_EXE), args) {
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
    match run_task(args, payload, &progress, true, false) {
        Ok(()) => {
            match args.mode {
                Mode::Install => {
                    system::message(&orbiters_i18n::tr!("Orbiters - N4N0 {version} is installed. Find it in the Start menu.", version = version), false, false);
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
    }
}
