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

pub fn publish(config: &Config, version: &str, channel: &str, force: bool) -> anyhow::Result<()> {
    let dist = crate::target_dir().join("dist").join(version);
    let setup = dist.join(format!("OrbitersSetup-{version}.exe"));
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
    let remote = config.remote_dir();
    let versioned = format!("{remote}/{version}/OrbitersSetup-{version}.exe");
    println!("\n== Uploading to {host}:{remote}");
    ssh(&host, &format!("mkdir -p {}", quote(&format!("{remote}/{version}"))))?;
    upload(&host, &setup, &versioned, Some(&release.installer.sha256))?;
    if channel == orbiters_update::STABLE {
        let latest = format!("{remote}/OrbitersSetup.exe");
        ssh(
            &host,
            &format!("cp -f {} {} && mv -f {} {}", quote(&versioned), quote(&format!("{latest}.part")), quote(&format!("{latest}.part")), quote(&latest)),
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
