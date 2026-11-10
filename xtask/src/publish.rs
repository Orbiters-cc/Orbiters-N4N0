//! `cargo xtask publish`: makes `target/dist/<version>/` a channel's release on orbiters.cc. The files go over SSH to
//! the folder the site serves as `/downloads/desktop` (Caddy, server repository): the setup under its version, then
//! (stable only) as `OrbitersSetup.exe`, the download link people are given, and last the release file, which is what
//! installed apps read. Each lands under a temporary name and is renamed once whole, so nobody downloads half a file,
//! and no app hears of a version whose setup is not there yet.

use crate::{output, run, Config};
use anyhow::Context as _;
use orbiters_update::SignedRelease;
use std::path::Path;
use std::process::Command;

/// Refuses a version that is not after the channel's current release (when there is one to ask).
pub fn ensure_newer(version: &str, channel: &str) -> anyhow::Result<()> {
    let key = orbiters_update::public_key().context("crates/orbiters-update/update-key.pub holds no key (cargo xtask keygen)")?;
    match orbiters_update::net::fetch_release(channel, &key) {
        Ok(current) => anyhow::ensure!(
            orbiters_update::is_newer(version, &current.version),
            "the {channel} channel already has {} (cargo xtask bump <version>, or --force to publish {version} anyway)",
            current.version
        ),
        Err(e) => println!("No current {channel} release to compare with ({e:#})."),
    }
    Ok(())
}

/// What a release gives installed apps and people: the file its release names, in the channel's feed; the one people
/// download, and the name that always means the latest (Windows' setup is both).
struct Files {
    installer: std::path::PathBuf,
    /// Published under `<version>/`, beside the installer.
    download: Option<std::path::PathBuf>,
    /// The stable channel's name for the latest, in the feed's folder.
    latest: String,
}

fn release_files(version: &str) -> Files {
    let dist = crate::target_dir().join("dist").join(version);
    if cfg!(target_os = "macos") {
        Files {
            installer: dist.join(orbiters_update::mac_archive_name(version)),
            download: Some(dist.join(orbiters_update::mac_disk_image_name(version))),
            latest: "Orbiters-N4N0-macos-arm64.dmg".into(),
        }
    } else {
        Files { installer: dist.join(format!("OrbitersSetup-{version}.exe")), download: None, latest: "OrbitersSetup.exe".into() }
    }
}

pub fn publish(config: &Config, version: &str, channel: &str, force: bool) -> anyhow::Result<()> {
    let dist = crate::target_dir().join("dist").join(version);
    let files = release_files(version);
    let setup = files.installer.clone();
    let release_file = dist.join(format!("{channel}.json"));
    let bytes = std::fs::read(&release_file).with_context(|| format!("no {} (cargo xtask dist)", release_file.display()))?;

    // What the apps will check, checked first.
    let key = orbiters_update::public_key().context("crates/orbiters-update/update-key.pub holds no key (cargo xtask keygen)")?;
    let release = SignedRelease::open(&bytes, &key, channel)?;
    anyhow::ensure!(release.version == version, "{} is of version {}, not {version}", release_file.display(), release.version);
    anyhow::ensure!(
        std::fs::metadata(&setup).map(|m| m.len()).ok() == Some(release.installer.size) && orbiters_update::sha256_file(&setup)? == release.installer.sha256,
        "{} is not the setup the release file names (cargo xtask dist again)",
        setup.display()
    );
    if !force {
        ensure_newer(version, channel)?;
    }

    let host = config.host();
    let remote = format!("{}/{}", config.remote_dir(), orbiters_update::PLATFORM_FOLDER).trim_end_matches('/').to_string();
    let file_name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let versioned = format!("{remote}/{version}/{}", file_name(&setup));
    println!("\n== Uploading to {host}:{remote}");
    ssh(&host, &format!("mkdir -p {}", quote(&format!("{remote}/{version}"))))?;
    upload(&host, &setup, &versioned, Some(&release.installer.sha256))?;
    let mut download = versioned.clone();
    if let Some(image) = &files.download {
        download = format!("{remote}/{version}/{}", file_name(image));
        upload(&host, image, &download, None)?;
    }
    if channel == orbiters_update::STABLE {
        let latest = format!("{remote}/{}", files.latest);
        ssh(
            &host,
            &format!("cp -f {} {} && mv -f {} {}", quote(&download), quote(&format!("{latest}.part")), quote(&format!("{latest}.part")), quote(&latest)),
        )?;
    }
    upload(&host, &release_file, &format!("{remote}/{channel}.json"), None)?;

    // As an app sees it.
    match orbiters_update::net::fetch_release(channel, &key) {
        Ok(seen) if seen.version == version => {
            println!("\n{} now serves {version} on {channel}.", orbiters_update::release_url(&orbiters_update::feed(), channel))
        }
        Ok(seen) => println!("\nUploaded, but the site still serves {} on {channel} (a cache, or the downloads folder is not the one served).", seen.version),
        Err(e) => println!("\nUploaded, but the release could not be read back from the site: {e:#}"),
    }
    Ok(())
}

/// `cargo xtask publish --dev`: makes `target/dist/<version>/` the channel's release in the development environment,
/// the folder the development server on this computer serves at [`orbiters_update::DEVELOPMENT_FEED`] (the server
/// repository's `docker-data/downloads/desktop`). The same files, in the same order, as [`publish`]; copied, not
/// uploaded.
pub fn publish_dev(config: &Config, version: &str, channel: &str, force: bool) -> anyhow::Result<()> {
    orbiters_update::set_development(true);
    let dist = crate::target_dir().join("dist").join(version);
    let files = release_files(version);
    let setup = files.installer.clone();
    let release_file = dist.join(format!("{channel}.json"));
    // A Mac without the release key makes no release file: its build goes on GitHub only.
    if cfg!(target_os = "macos") && !release_file.exists() {
        println!("\nNo release file (this computer has no release key): nothing for the development environment's feed.");
        return Ok(());
    }
    let bytes = std::fs::read(&release_file).with_context(|| format!("no {} (cargo xtask dist)", release_file.display()))?;
    let key = orbiters_update::public_key().context("crates/orbiters-update/update-key.pub holds no key (cargo xtask keygen)")?;
    let release = SignedRelease::open(&bytes, &key, channel)?;
    anyhow::ensure!(release.version == version, "{} is of version {}, not {version}", release_file.display(), release.version);
    anyhow::ensure!(
        std::fs::metadata(&setup).map(|m| m.len()).ok() == Some(release.installer.size) && orbiters_update::sha256_file(&setup)? == release.installer.sha256,
        "{} is not the setup the release file names (cargo xtask dist again)",
        setup.display()
    );
    if !force {
        ensure_newer(version, channel)?;
    }
    let dir = config.dev_dir();
    println!("\n== Copying to the development environment: {}", dir.display());
    let place = |from: &Path, to: &Path| -> anyhow::Result<()> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let part = to.with_file_name(format!("{}.part", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        std::fs::copy(from, &part).with_context(|| format!("{} cannot be written", part.display()))?;
        std::fs::rename(&part, to).with_context(|| format!("{} cannot be replaced", to.display()))?;
        Ok(())
    };
    let dir = dir.join(orbiters_update::PLATFORM_FOLDER);
    let versioned = dir.join(version).join(setup.file_name().expect("a file"));
    place(&setup, &versioned)?;
    if let Some(image) = &files.download {
        place(image, &dir.join(version).join(image.file_name().expect("a file")))?;
    }
    if channel == orbiters_update::STABLE {
        place(files.download.as_ref().unwrap_or(&setup), &dir.join(&files.latest))?;
    }
    place(&release_file, &dir.join(format!("{channel}.json")))?;
    match orbiters_update::net::fetch_release(channel, &key) {
        Ok(seen) if seen.version == version => {
            println!("\n{} now serves {version} on {channel}.", orbiters_update::release_url(&orbiters_update::feed(), channel))
        }
        Ok(seen) => println!("\nCopied, but the development server still serves {} on {channel}.", seen.version),
        Err(e) => println!("\nCopied, but the development server did not serve it back (is it running on 127.0.0.1:4100?): {e:#}"),
    }
    Ok(())
}

/// Where releases also go: this repository on GitHub, a release per `desktop-v<version>` tag with its setup attached.
pub const GITHUB_REPO: &str = "Orbiters-cc/Orbiters-N4N0";

/// The GitHub release of `version`: its tag (pushed already) with `target/dist/<version>/`'s setup attached, through
/// GitHub's CLI (`gh`, signed in). For a version released there already, its setup is replaced.
pub fn github(version: &str, notes: &str, unsigned: bool) -> anyhow::Result<()> {
    let tag = format!("desktop-v{version}");
    let assets: Vec<std::path::PathBuf> =
        if cfg!(target_os = "macos") { crate::macos::release_assets(version) } else { vec![crate::target_dir().join("dist").join(version).join(format!("OrbitersSetup-{version}.exe"))] };
    for asset in &assets {
        anyhow::ensure!(asset.is_file(), "no {} (cargo xtask dist)", asset.display());
    }
    let setup = assets[0].clone();
    let mut body = if notes.trim().is_empty() { format!("Orbiters - N4N0 {version}.") } else { notes.trim().to_string() };
    if cfg!(target_os = "macos") {
        body.push_str(&format!(
            "\n\nInstall on a Mac with Apple silicon: open `{}` below and drag the app to Applications. It is not notarized yet: the first time, macOS refuses to open it; then allow it in System Settings, Privacy & Security, Open Anyway.",
            orbiters_update::mac_disk_image_name(version)
        ));
    } else {
        body.push_str(&format!("\n\nInstall: download `OrbitersSetup-{version}.exe` below and run it (for you only, no administrator rights)."));
        if unsigned {
            body.push_str(" It is not code-signed yet: Windows SmartScreen warns about it (More info, then Run anyway).");
        }
    }
    let notes_file = setup.with_file_name("github-notes.md");
    std::fs::write(&notes_file, &body)?;
    let exists = Command::new("gh").args(["release", "view", &tag, "--repo", GITHUB_REPO]).output().is_ok_and(|o| o.status.success());
    if exists {
        run(Command::new("gh").args(["release", "upload", &tag, "--repo", GITHUB_REPO, "--clobber"]).args(&assets))?;
    } else {
        run(Command::new("gh")
            .args(["release", "create", &tag, "--repo", GITHUB_REPO, "--verify-tag", "--title", &format!("Orbiters - N4N0 {version}")])
            .args(crate::is_development(version).then_some("--prerelease"))
            .arg("--notes-file")
            .arg(&notes_file)
            .args(&assets))?;
    }
    let _ = std::fs::remove_file(&notes_file);
    println!("\nhttps://github.com/{GITHUB_REPO}/releases/tag/{tag}");
    Ok(())
}

/// Copies `local` to `remote` through `<remote>.part`, checked against `sha256` there when given, then renamed.
fn upload(host: &str, local: &Path, remote: &str, sha256: Option<&str>) -> anyhow::Result<()> {
    let part = format!("{remote}.part");
    run(Command::new("scp").args(["-q", "-o", "BatchMode=yes"]).arg(local).arg(format!("{host}:{part}")))?;
    if let Some(expected) = sha256 {
        let line = output(Command::new("ssh").args(["-o", "BatchMode=yes", host, &format!("sha256sum {}", quote(&part))]))?;
        anyhow::ensure!(line.split_whitespace().next() == Some(expected), "{part} arrived damaged ({})", line.trim());
    }
    ssh(host, &format!("chmod 644 {} && mv -f {} {}", quote(&part), quote(&part), quote(remote)))
}

fn ssh(host: &str, command: &str) -> anyhow::Result<()> {
    run(Command::new("ssh").args(["-o", "BatchMode=yes", host, command]))
}

/// A word for the remote shell, in single quotes.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_words_are_quoted() {
        assert_eq!(quote("/srv/downloads/desktop"), "'/srv/downloads/desktop'");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
