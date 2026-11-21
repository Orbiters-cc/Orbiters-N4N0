//! "Install Obelisk & play VRChat" on a Mac: Obelisk's latest release is downloaded from its GitHub releases, checked
//! (the SHA-256 its release publishes, then the code itself: Obelisk's identifier under its maker's Developer ID, and
//! Apple's notary), put in Applications, told to take its beta versions when the person never chose (VRChat's support
//! is in the engine's betas as of November 2026), with macOS's own tools. Nothing of Obelisk's code is in N4N0
//! (`orbiters_core::obelisk` says why); a file fetched by N4N0 carries no quarantine mark, so the checks Gatekeeper
//! would make at its first opening are made here, before it is installed.
use anyhow::Context as _;
use orbiters_core::launcher::EngineSetup;
use orbiters_core::obelisk;
use orbiters_i18n::tr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Downloads, checks and installs Obelisk for the person whose home is `home`; `progress` hears each step.
pub fn install(home: &Path, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> anyhow::Result<()> {
    let work = std::env::temp_dir().join(format!("orbiters-n4n0-obelisk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = install_from(&[PathBuf::from("/Applications"), home.join("Applications")], &work, progress, stop);
    let _ = std::fs::remove_dir_all(&work);
    result?;
    // The person's own choice of beta versions, made in Obelisk before, is left as it is.
    if !quiet(Command::new("/usr/bin/defaults").args(["read", obelisk::BUNDLE_ID, obelisk::BETA_SETTING])) {
        let _ = quiet(Command::new("/usr/bin/defaults").args(["write", obelisk::BUNDLE_ID, obelisk::BETA_SETTING, "-bool", "true"]));
    }
    Ok(())
}

/// Obelisk's latest release into the first of `folders` that takes it, with `work` for what is downloaded.
fn install_from(folders: &[PathBuf], work: &Path, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> anyhow::Result<()> {
    progress(EngineSetup::Downloading { percent: 0 });
    let agent = orbiters_core::http_agent(Some(Duration::from_secs(20)), Some(Duration::from_secs(60)));
    let download = obelisk::latest_download(&text(&agent, obelisk::LATEST, 1 << 20)?).with_context(|| tr("Obelisk's latest version was not found on GitHub."))?;
    let sha256 = obelisk::checksum(&text(&agent, &download.checksum_url, 4096)?).with_context(|| tr("Obelisk's release does not say its checksum."))?;
    let image = work.join("Obelisk.dmg");
    let shown = std::cell::Cell::new(0u8);
    orbiters_update::net::download(
        &orbiters_update::Artifact { url: download.url.clone(), sha256, size: download.size },
        &image,
        |have, total| {
            let percent = (have.saturating_mul(100) / total.max(1)).min(100) as u8;
            if percent != shown.replace(percent) {
                progress(EngineSetup::Downloading { percent });
            }
        },
        stop,
    )?;
    progress(EngineSetup::Installing);
    let mount = work.join("image");
    std::fs::create_dir_all(&mount)?;
    anyhow::ensure!(quiet(Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet", "-mountpoint"]).arg(&mount).arg(&image)), "{}", tr("Obelisk's disk image could not be opened."));
    let installed = from_image(folders, &mount.join("Obelisk.app"));
    let _ = quiet(Command::new("/usr/bin/hdiutil").args(["detach", "-quiet", "-force"]).arg(&mount));
    installed
}

/// A small text over HTTPS, at most `limit` bytes of it.
fn text(agent: &ureq::Agent, url: &str, limit: u64) -> anyhow::Result<String> {
    anyhow::ensure!(url.starts_with("https://"), "{url}");
    Ok(agent.get(url).header("Accept", "application/vnd.github+json, text/plain").call().with_context(|| url.to_string())?.body_mut().with_config().limit(limit).read_to_string()?)
}

fn quiet(command: &mut Command) -> bool {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Whether the application at `app` is Obelisk as its maker signed it and Apple's notary passed it.
fn genuine(app: &Path) -> bool {
    quiet(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict", &format!("-R={}", obelisk::REQUIREMENT)]).arg(app))
        && quiet(Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute"]).arg(app))
}

/// Copies the checked application into the first of `folders` that takes it (Applications: the computer's, else the
/// person's own where that one is not theirs to write), under another name until the copy is whole and checked again.
fn from_image(folders: &[PathBuf], app: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(app.is_dir() && genuine(app), "{}", tr("The download is not Obelisk as its maker signed it: nothing was installed."));
    for folder in folders {
        let _ = std::fs::create_dir_all(folder);
        let (part, to) = (folder.join(".Obelisk.app.orbiters-part"), folder.join("Obelisk.app"));
        let _ = std::fs::remove_dir_all(&part);
        if !to.exists() && quiet(Command::new("/usr/bin/ditto").arg(app).arg(&part)) && genuine(&part) && std::fs::rename(&part, &to).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_dir_all(&part);
    }
    anyhow::bail!("{}", tr("Obelisk could not be put in Applications."))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// The real thing, from GitHub into a folder of the test's (`cargo test -p orbiters-desktop obelisk_install -- --ignored`):
    /// the download, its checksum, Obelisk's signature and Apple's notary, the copy. Nothing of the person's is touched.
    #[test]
    #[ignore]
    fn obelisk_s_latest_release_is_downloaded_checked_and_put_in_a_folder() {
        let work = std::env::temp_dir().join(format!("orbiters-obelisk-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let (refused, folder) = (work.join("a file, not a folder"), work.join("Applications"));
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(&refused, b"").unwrap();
        let steps = std::sync::Mutex::new(Vec::new());
        install_from(&[refused, folder.clone()], &work.join("work"), &|step| steps.lock().unwrap().push(step), &AtomicBool::new(false)).unwrap();
        let steps = steps.into_inner().unwrap();
        assert_eq!(steps.first(), Some(&EngineSetup::Downloading { percent: 0 }));
        assert!(steps.contains(&EngineSetup::Downloading { percent: 100 }));
        assert_eq!(steps.last(), Some(&EngineSetup::Installing));
        assert!(genuine(&folder.join("Obelisk.app")), "installed in the folder that took it, whole and signed");
        assert!(!folder.join(".Obelisk.app.orbiters-part").exists());
        // Another program under Obelisk's name is refused.
        let fake = work.join("fake").join("Obelisk.app");
        std::fs::create_dir_all(fake.join("Contents")).unwrap();
        assert!(from_image(&[work.join("elsewhere")], &fake).is_err());
        assert!(!work.join("elsewhere").join("Obelisk.app").exists());
        let _ = std::fs::remove_dir_all(&work);
    }
}
