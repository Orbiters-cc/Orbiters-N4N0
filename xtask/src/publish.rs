//! `cargo xtask publish`: makes `target/dist/<version>/` a channel's release on orbiters.cc. The files go over SSH to
//! the folder the site serves as `/downloads/desktop` (Caddy, server repository): the setup under its version, then
//! (stable only) as `OrbitersSetup.exe`, the download link people are given, and last the release file, which is what
//! installed apps read. Each lands under a temporary name and is renamed once whole, so nobody downloads half a file,
//! and no app hears of a version whose setup is not there yet.

use crate::{output, run, Config};
use anyhow::Context as _;
use orbiters_update::{Release, SignedRelease};
use std::path::Path;
use std::process::Command;

/// Refuses a version that is not after the channel's current release. Only "there is none yet" lets a version out without one
/// to compare with: a site that cannot be asked, or a release that cannot be read, stops here.
pub fn ensure_newer(version: &str, channel: &str) -> anyhow::Result<()> {
    newer(version, channel, served(channel)?.as_ref(), None)
}

/// The same for the development environment's feed on this computer.
pub fn ensure_newer_dev(config: &Config, version: &str, channel: &str) -> anyhow::Result<()> {
    match config.dev_feed()? {
        Some(feed) => newer(version, channel, in_folder(&feed.join(orbiters_update::PLATFORM_FOLDER), channel)?.as_ref(), None),
        None => Ok(()),
    }
}

/// `current`, the release out now, against `version`: older, or the very `same` release put out again (an upload that was cut).
fn newer(version: &str, channel: &str, current: Option<&Release>, same: Option<&Release>) -> anyhow::Result<()> {
    let Some(current) = current else {
        println!("No {channel} release is out yet.");
        return Ok(());
    };
    let again = same.is_some_and(|mine| mine.version == current.version && mine.installer.sha256 == current.installer.sha256);
    anyhow::ensure!(
        again || orbiters_update::is_newer(version, &current.version),
        "the {channel} channel already has {} (cargo xtask bump <version>, or --force to publish {version} anyway)",
        current.version
    );
    Ok(())
}

fn key() -> anyhow::Result<[u8; 32]> {
    orbiters_update::public_key().context("crates/orbiters-update/update-key.pub holds no key (cargo xtask keygen)")
}

/// The channel's current release, as an app sees it; `None` only where the feed says it has none.
fn served(channel: &str) -> anyhow::Result<Option<Release>> {
    none_yet(orbiters_update::net::fetch_release(channel, &key()?))
}

fn none_yet(fetched: anyhow::Result<Release>) -> anyhow::Result<Option<Release>> {
    match fetched {
        Ok(current) => Ok(Some(current)),
        Err(e) if orbiters_update::net::none_published(&e) => Ok(None),
        Err(e) => Err(e.context("what is released now could not be read, so nothing newer can be said to be")),
    }
}

/// The release file of `channel` in a feed's `folder`; `None` when it has none.
fn in_folder(folder: &Path, channel: &str) -> anyhow::Result<Option<Release>> {
    let file = folder.join(format!("{channel}.json"));
    match std::fs::read(&file) {
        Ok(bytes) => SignedRelease::open(&bytes, &key()?, channel).map(Some).with_context(|| format!("{} cannot be read as a release", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| file.display().to_string()),
    }
}

/// Whether what was put out is what is served: said once all the rest is done, and then a failure of the command.
#[must_use]
pub struct Published(Result<(), String>);

impl Published {
    fn seen(version: &str, channel: &str, seen: anyhow::Result<Release>) -> Self {
        let url = orbiters_update::release_url(&orbiters_update::feed(), channel);
        Self(match seen {
            Ok(seen) if seen.version == version => {
                println!("\n{url} now serves {version} on {channel}.");
                Ok(())
            }
            Ok(seen) => Err(format!("{version} was put out, but {url} still serves {} on {channel} (a cache, or the folder is not the one served)", seen.version)),
            Err(e) => Err(format!("{version} was put out, but {url} did not serve it back: {e:#}")),
        })
    }

    pub fn served(self) -> anyhow::Result<()> {
        self.0.map_err(anyhow::Error::msg)
    }
}

/// The release file `target/dist/<version>/` holds, as the apps will check it: of this version, for this very setup, and made
/// of a commit.
fn made(dist: &Path, files: &Files, version: &str, channel: &str) -> anyhow::Result<Release> {
    let release_file = dist.join(format!("{channel}.json"));
    let bytes = std::fs::read(&release_file).with_context(|| format!("no {} (cargo xtask dist)", release_file.display()))?;
    let release = SignedRelease::open(&bytes, &key()?, channel)?;
    anyhow::ensure!(release.version == version, "{} is of version {}, not {version}", release_file.display(), release.version);
    anyhow::ensure!(
        crate::commit::is_commit(&release.commit),
        "{} was built from {:?}, which is no commit: commit the changes and build it again (cargo xtask release)",
        release_file.display(),
        release.commit
    );
    let setup = &files.installer;
    anyhow::ensure!(
        std::fs::metadata(setup).map(|m| m.len()).ok() == Some(release.installer.size) && orbiters_update::sha256_file(setup)? == release.installer.sha256,
        "{} is not the setup the release file names (cargo xtask dist again)",
        setup.display()
    );
    Ok(release)
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

pub fn publish(config: &Config, version: &str, channel: &str, force: bool) -> anyhow::Result<Published> {
    let dist = crate::target_dir().join("dist").join(version);
    let files = release_files(version);
    let setup = files.installer.clone();
    let release_file = dist.join(format!("{channel}.json"));
    // What the apps will check, checked first.
    let release = made(&dist, &files, version, channel)?;
    if !force {
        newer(version, channel, served(channel)?.as_ref(), Some(&release))?;
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
    Ok(Published::seen(version, channel, orbiters_update::net::fetch_release(channel, &key()?)))
}

/// `cargo xtask publish --dev`: makes `target/dist/<version>/` the channel's release in the development environment,
/// the folder the development server on this computer serves at [`orbiters_update::DEVELOPMENT_FEED`] (the server
/// repository's `docker-data/downloads/desktop`). The same files, in the same order, as [`publish`]; copied, not
/// uploaded. The feed keeps the last [`KEPT`] versions; GitHub keeps them all.
pub fn publish_dev(config: &Config, version: &str, channel: &str, force: bool) -> anyhow::Result<Published> {
    orbiters_update::set_development(true);
    let dist = crate::target_dir().join("dist").join(version);
    let files = release_files(version);
    let setup = files.installer.clone();
    let release_file = dist.join(format!("{channel}.json"));
    // A Mac without the release key makes no release file, and one without the server has no feed: GitHub only.
    let Some(dir) = config.dev_feed()?.filter(|_| !cfg!(target_os = "macos") || release_file.exists()) else {
        println!("\nNothing for the development environment's feed: this computer has no release key, or no server.");
        return Ok(Published(Ok(())));
    };
    let release = made(&dist, &files, version, channel)?;
    if !force {
        newer(version, channel, in_folder(&dir.join(orbiters_update::PLATFORM_FOLDER), channel)?.as_ref(), Some(&release))?;
    }
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
    for old in prune(&dir, KEPT) {
        println!("Took {old} out of the feed (GitHub keeps it).");
    }
    Ok(Published::seen(version, channel, orbiters_update::net::fetch_release(channel, &key()?)))
}

/// How many versions the development feed keeps.
const KEPT: usize = 8;

/// Takes out of a feed's `folder` the versions before the newest `keep`, and never one a channel's release file there names:
/// the files a release put in a version's folder, and the folder once empty. What else is there stays. Those taken out, named;
/// one that could not be (a download of it under way) is left for the next time.
fn prune(folder: &Path, keep: usize) -> Vec<String> {
    let entries = |dir: &Path| std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default();
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // What the release files there name. One that cannot be read may name any: nothing is taken out.
    let mut named = Vec::new();
    for file in entries(folder).into_iter().filter(|p| p.extension().is_some_and(|x| x == "json")) {
        let Some(channel) = file.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        match in_folder(folder, &channel) {
            Ok(Some(release)) => named.push(release.version),
            Ok(None) => {}
            Err(e) => {
                println!("Nothing was taken out of the feed: {e:#}");
                return Vec::new();
            }
        }
    }
    prune_versions(folder, keep, &named).into_iter().map(|p| name(&p)).collect()
}

fn prune_versions(folder: &Path, keep: usize, named: &[String]) -> Vec<std::path::PathBuf> {
    let entries = |dir: &Path| std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default();
    let mut versions: Vec<(semver::Version, std::path::PathBuf)> = entries(folder)
        .into_iter()
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .filter_map(|p| Some((semver::Version::parse(p.file_name()?.to_str()?).ok()?, p)))
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    let mut gone = Vec::new();
    for (version, dir) in versions.into_iter().skip(keep) {
        let version = version.to_string();
        if named.contains(&version) {
            continue;
        }
        for file in [format!("OrbitersSetup-{version}.exe"), orbiters_update::mac_archive_name(&version), orbiters_update::mac_disk_image_name(&version)] {
            let _ = std::fs::remove_file(dir.join(&file));
            let _ = std::fs::remove_file(dir.join(format!("{file}.part")));
        }
        if std::fs::remove_dir(&dir).is_ok() {
            gone.push(dir);
        }
    }
    gone
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
    // The tag is on the commit that setup was made of (a Mac without the release key has no release file that says so).
    if let Some(release) = in_folder(&crate::target_dir().join("dist").join(version), orbiters_update::STABLE)? {
        let tagged = output(Command::new("git").current_dir(crate::root()).args(["ls-remote", "origin", &format!("refs/tags/{tag}"), &format!("refs/tags/{tag}^{{}}")]))?;
        let tagged = tagged.lines().filter_map(|l| l.split_whitespace().next()).next_back();
        anyhow::ensure!(tagged == Some(release.commit.as_str()), "{tag} at origin is on {tagged:?}, and the setup was made of {:?}", release.commit);
    }
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

    fn release(version: &str, sha256: &str) -> Release {
        Release {
            product: orbiters_update::PRODUCT.into(),
            channel: orbiters_update::STABLE.into(),
            version: version.into(),
            released: String::new(),
            commit: String::new(),
            notes: String::new(),
            installer: orbiters_update::Artifact { url: String::new(), sha256: sha256.into(), size: 1 },
        }
    }

    /// PRJ-51: "there is none yet" is the one answer that lets a version out with nothing to compare; a site that cannot be
    /// asked or a release that cannot be read never does. The very release that is out may be put out again.
    #[test]
    fn a_version_is_compared_or_it_is_not_released() {
        let none = || Err(anyhow::Error::from(orbiters_update::net::no_release()).context("stable.json"));
        assert!(none_yet(none()).unwrap().is_none());
        for failed in ["connection refused", "the signature does not match", "500"] {
            assert!(none_yet(Err(anyhow::anyhow!(failed))).is_err(), "{failed}");
        }
        let (out, mine) = (release("0.3.10-dev.6", "a"), release("0.3.10-dev.6", "a"));
        newer("0.3.10-dev.7", "stable", Some(&out), None).unwrap();
        newer("0.3.10-dev.5", "stable", None, None).unwrap();
        for version in ["0.3.10-dev.5", "0.3.10-dev.6", "0.3.9"] {
            assert!(newer(version, "stable", Some(&out), None).is_err(), "{version}");
        }
        newer("0.3.10-dev.6", "stable", Some(&out), Some(&mine)).unwrap();
        assert!(newer("0.3.10-dev.6", "stable", Some(&out), Some(&release("0.3.10-dev.6", "b"))).is_err(), "another build of the version that is out");
        // A feed's file that cannot be read as a release is no "none".
        let dir = std::env::temp_dir().join(format!("xtask-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(in_folder(&dir, "stable").unwrap().is_none());
        std::fs::write(dir.join("stable.json"), b"{}").unwrap();
        assert!(in_folder(&dir, "stable").is_err());
        let _ = std::fs::remove_dir_all(&dir);
        // What was put out and is not what is served is the command's failure.
        assert!(Published::seen("0.3.10-dev.7", "stable", Ok(release("0.3.10-dev.7", "c"))).served().is_ok());
        assert!(Published::seen("0.3.10-dev.7", "stable", Ok(out)).served().is_err());
        assert!(Published::seen("0.3.10-dev.7", "stable", Err(anyhow::anyhow!("refused"))).served().is_err());
    }

    /// PRJ-54: the newest eight stay, and what a release file names; an older version's own files go, anything else stays.
    #[test]
    fn the_development_feed_keeps_the_last_versions() {
        let dir = std::env::temp_dir().join(format!("xtask-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for n in 1..=12 {
            let version = format!("0.3.10-dev.{n}");
            std::fs::create_dir_all(dir.join(&version)).unwrap();
            std::fs::write(dir.join(&version).join(format!("OrbitersSetup-{version}.exe")), b"x").unwrap();
        }
        std::fs::write(dir.join("0.3.10-dev.2").join("somebody's.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("macos").join("0.3.9")).unwrap();
        std::fs::write(dir.join("OrbitersSetup.exe"), b"x").unwrap();
        let mut gone: Vec<String> = prune_versions(&dir, KEPT, &["0.3.10-dev.1".into()]).into_iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        gone.sort();
        // (dev.10 to dev.12 are after dev.9, as versions are.)
        assert_eq!(gone, ["0.3.10-dev.3", "0.3.10-dev.4"]);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("0.")).collect();
        left.sort();
        assert_eq!(left, ["0.3.10-dev.1", "0.3.10-dev.10", "0.3.10-dev.11", "0.3.10-dev.12", "0.3.10-dev.2", "0.3.10-dev.5", "0.3.10-dev.6", "0.3.10-dev.7", "0.3.10-dev.8", "0.3.10-dev.9"]);
        assert!(dir.join("0.3.10-dev.1/OrbitersSetup-0.3.10-dev.1.exe").is_file(), "a release file names it");
        assert!(dir.join("0.3.10-dev.2/somebody's.txt").is_file() && dir.join("OrbitersSetup.exe").is_file() && dir.join("macos/0.3.9").is_dir());
        // A release file that cannot be read may name any of them.
        std::fs::write(dir.join("beta.json"), b"{}").unwrap();
        assert!(prune(&dir, 1).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_words_are_quoted() {
        assert_eq!(quote("/srv/downloads/desktop"), "'/srv/downloads/desktop'");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
