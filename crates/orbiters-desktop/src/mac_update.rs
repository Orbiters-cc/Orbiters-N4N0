//! Installing an update on the Mac, where the setup's work falls to the app: an update is the whole application, zipped.
//! This program, started again from inside the application it replaces, waits for the app to quit, unpacks the archive beside
//! it, checks it (the application it should be, its code signature whole), swaps it in with a rename, putting the old one back
//! if that fails, and opens it again. A running program may lose its files: the system keeps them until it ends. A copy
//! elsewhere would not do, since macOS kills a copy of a hardened program outside its application. The archive was checked
//! against the signed release's size and SHA-256 before it got here.

use anyhow::Context as _;
use orbiters_i18n::tr;
use orbiters_update::{MAC_APP, MAC_BUNDLE_ID};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The app gets this long to finish and quit (a reconstruction is stopped, a call hung up).
const QUIT_WAIT: Duration = Duration::from_secs(120);

/// Starts this program again to install `archive` over the application at `bundle` once this app has quit, and open it again
/// (`hidden`: in the menu bar, as it was).
pub fn start(archive: &Path, bundle: &Path, hidden: bool) -> std::io::Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--apply-update").arg(archive).arg(bundle).arg(std::process::id().to_string()).arg(if hidden { "hidden" } else { "window" });
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Its own process group: nothing that ends this app ends it.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn().map(drop)
}

/// `--apply-update <archive> <application> <the app's process> <hidden|window>`.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let [archive, bundle, pid, how] = args else { anyhow::bail!("--apply-update <archive> <application> <process> <hidden|window>") };
    let (archive, bundle) = (PathBuf::from(archive), PathBuf::from(bundle));
    let hidden = how == "hidden";
    wait_for(pid.parse().context("not a process id")?);
    // Whatever happened, an app opens at the end: this version if it went wrong.
    if let Err(e) = apply(&archive, &bundle) {
        eprintln!("the update did not install: {e:#}");
        tell(&tr!("The update could not be installed: {error}\n\nThis version opens again. You can install the new one by hand from the disk image.", error = format!("{e:#}")));
    }
    let mut open = Command::new("/usr/bin/open");
    open.arg(&bundle);
    if hidden {
        open.args(["--args", "--hidden"]);
    }
    open.status().context("open")?;
    Ok(())
}

/// Until the process is gone, or for [`QUIT_WAIT`].
fn wait_for(pid: u32) {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    let started = Instant::now();
    while started.elapsed() < QUIT_WAIT {
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing());
        if system.process(pid).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn apply(archive: &Path, bundle: &Path) -> anyhow::Result<()> {
    let parent = bundle.parent().context("the application is in no folder")?;
    // Beside the application: on its volume, so that the swap is a rename.
    let unpacked = parent.join(format!(".orbiters-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).with_context(|| format!("{} cannot be written", parent.display()))?;
    let result = (|| {
        run_tool(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(archive).arg(&unpacked)).context("unpacking")?;
        let new = unpacked.join(MAC_APP);
        check(&new)?;
        swap(&new, bundle)
    })();
    let _ = std::fs::remove_dir_all(&unpacked);
    result
}

/// The application unpacked is this app's, with its signature whole.
fn check(new: &Path) -> anyhow::Result<()> {
    let id = run_tool(Command::new("/usr/bin/plutil").args(["-extract", "CFBundleIdentifier", "raw"]).arg(new.join("Contents/Info.plist"))).context("its Info.plist")?;
    anyhow::ensure!(id.trim() == MAC_BUNDLE_ID, "it is another application ({})", id.trim());
    run_tool(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(new)).context("its signature")?;
    Ok(())
}

/// `new` takes `bundle`'s place, by renames; if the second fails the old one goes back. The old one is removed last.
fn swap(new: &Path, bundle: &Path) -> anyhow::Result<()> {
    let old = bundle.with_file_name(format!(".orbiters-old-{}", std::process::id()));
    std::fs::rename(bundle, &old).with_context(|| format!("{} cannot be moved", bundle.display()))?;
    if let Err(e) = std::fs::rename(new, bundle) {
        std::fs::rename(&old, bundle).with_context(|| format!("the old version could not be put back from {}", old.display()))?;
        return Err(e).with_context(|| format!("{} cannot be replaced", bundle.display()));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

fn run_tool(cmd: &mut Command) -> anyhow::Result<String> {
    let out = cmd.output()?;
    anyhow::ensure!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A dialog, since no window of ours is open here.
fn tell(message: &str) {
    let text = message.replace('\\', "\\\\").replace('"', "\\\"");
    let _ = Command::new("/usr/bin/osascript").args(["-e", &format!("display alert \"Orbiters - N4N0\" message \"{text}\" as warning")]).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stand_in(dir: &Path, version: &str) -> PathBuf {
        let app = dir.join(MAC_APP);
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/version"), version).unwrap();
        app
    }

    #[test]
    fn the_swap_puts_the_new_one_in_place_and_leaves_nothing_of_the_old() {
        let root = std::env::temp_dir().join(format!("orbiters-swap-{}", uuid::Uuid::new_v4()));
        let apps = root.join("Applications");
        let unpacked = root.join("unpacked");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&unpacked).unwrap();
        let bundle = stand_in(&apps, "old");
        let new = stand_in(&unpacked, "new");
        swap(&new, &bundle).unwrap();
        assert_eq!(std::fs::read_to_string(bundle.join("Contents/MacOS/version")).unwrap(), "new");
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1, "the old one is gone");
        assert!(!new.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_swap_that_fails_puts_the_old_one_back() {
        let root = std::env::temp_dir().join(format!("orbiters-swap-{}", uuid::Uuid::new_v4()));
        let apps = root.join("Applications");
        std::fs::create_dir_all(&apps).unwrap();
        let bundle = stand_in(&apps, "old");
        let nothing_there = root.join("unpacked").join(MAC_APP);
        assert!(swap(&nothing_there, &bundle).is_err());
        assert_eq!(std::fs::read_to_string(bundle.join("Contents/MacOS/version")).unwrap(), "old");
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1, "nothing is left beside it");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn another_application_is_refused() {
        let root = std::env::temp_dir().join(format!("orbiters-check-{}", uuid::Uuid::new_v4()));
        let app = stand_in(&root, "x");
        std::fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.someone.else</string></dict></plist>"#,
        )
        .unwrap();
        let error = check(&app).unwrap_err().to_string();
        assert!(error.contains("another application"), "{error}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
