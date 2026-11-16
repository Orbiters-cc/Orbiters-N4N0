//! Orbiters - N4N0's release tooling, run as `cargo xtask <command>` (an alias in `.cargo/config.toml`). Releases are
//! built on this computer, not on GitHub's runners, and published on orbiters.cc, where installed apps find them
//! within hours (at once when the person looks in Settings):
//!
//! ```text
//! cargo xtask keygen          once: the release key (its public half goes in the repository)
//! cargo xtask bump 0.2.0      the workspace's version; commit and push it
//! cargo xtask release         build, sign, pack, publish, tag, and the GitHub release
//! ```
//!
//! `release` checks the commit (clean, pushed, on main, its version not released yet), runs `dist` (the programs
//! built in release and signed with Azure Artifact Signing, the setup with them inside, signed too, and the signed
//! release file: `target/dist/<version>/`), then `publish` (uploaded to orbiters.cc over SSH, the release file last:
//! installed apps see the new version only once its setup is there), then tags the commit `desktop-v<version>` and
//! makes that tag's release on GitHub with the setup attached (`github`). `--unsigned` releases before Authenticode
//! signing is set up: on GitHub only, since installed apps update only to signed releases. `dist`, `publish` and
//! `github` also run on their own (`publish` or `github` again after a cut upload). See docs/releasing.md.

mod dist;
mod macos;
mod pe;
mod publish;

use anyhow::Context as _;
use base64::Engine as _;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELP: &str = "\
cargo xtask <command>

  keygen [--force]             a new release key: the private half in ~/.orbiters-release, the public half in
                               crates/orbiters-update/update-key.pub (commit it)
  bump <version>               sets the workspace's version (Cargo.toml, Cargo.lock)
  bump --dev                   the next development build's version (0.3.2-dev.1 after 0.3.1, then dev.2 ...)
  dist [--unsigned] [--notes <file>] [--channel <name>] [--programs <folder>] [--dev]
                               builds, signs and packs target/dist/<version>/: the setup and the signed release
                               (--programs: packs the programs already built there instead, a test build;
                               --dev: compiled as development builds are, much faster to build, and only
                               for a development version)
  publish [--channel <name>] [--force] [--dev]
                               uploads target/dist/<version>/ to orbiters.cc and makes it the channel's release
                               (--dev: copies it to the development environment's feed on this computer)
  release [--notes <file>] [--channel <name>] [--allow-dirty] [--unsigned]
                               checks the commit, then dist, publish, the desktop-v<version> tag and its GitHub
                               release with the setup (--unsigned: no Authenticode signature yet)
  release --dev [--notes <file>] [--allow-dirty]
                               a development build (bump --dev first): dist --dev, publish --dev, push, tag and a
                               GitHub pre-release. Installed apps switched to the development environment update to it
  github [--notes <file>] [--unsigned]
                               the GitHub release of target/dist/<version>/'s setup (its tag pushed already)

Settings: ~/.orbiters-release/config.json, or the environment (docs/releasing.md).";

/// Where the release settings and the private key live: `~/.orbiters-release`, outside every repository.
pub fn release_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".orbiters-release")
}

/// The release settings: `~/.orbiters-release/config.json`, each overridden by its environment variable.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The release key's private half (`ORBITERS_UPDATE_KEY`; `~/.orbiters-release/update-signing-key`).
    pub update_key: Option<PathBuf>,
    /// signtool.exe (`ORBITERS_SIGNTOOL`; the newest in the Windows SDK otherwise).
    pub signtool: Option<PathBuf>,
    /// Azure Artifact Signing's signtool plug-in, `Azure.CodeSigning.Dlib.dll`, x64 (`ORBITERS_SIGNING_DLIB`).
    pub signing_dlib: Option<PathBuf>,
    /// Its account: `{"Endpoint", "CodeSigningAccountName", "CertificateProfileName"}` (`ORBITERS_SIGNING_METADATA`).
    pub signing_metadata: Option<PathBuf>,
    /// The timestamp server (`ORBITERS_TIMESTAMP_URL`; Microsoft's for Artifact Signing).
    pub timestamp_url: Option<String>,
    /// `vcruntime140.dll` to ship beside the programs (`ORBITERS_VCRUNTIME`; Visual Studio's redistributable).
    pub vcruntime: Option<PathBuf>,
    /// Where releases are uploaded: an SSH destination (`ORBITERS_RELEASE_HOST`; `blackorbit@orbiters`, Tailscale).
    pub host: Option<String>,
    /// The folder there that orbiters.cc serves as `/downloads/desktop` (`ORBITERS_RELEASE_DIR`).
    pub remote_dir: Option<String>,
    /// Where the setup is built with the C runtime inside (`ORBITERS_SETUP_TARGET_DIR`; the target folder).
    pub setup_target_dir: Option<PathBuf>,
    /// The development environment's feed folder (`ORBITERS_DEV_DOWNLOADS`; the server repository's
    /// `docker-data/downloads/desktop` beside this one), served by the development server at
    /// `orbiters_update::DEVELOPMENT_FEED`.
    pub dev_dir: Option<PathBuf>,
    /// The Mac: the identity to sign with, `-` for ad hoc (`ORBITERS_MACOS_IDENTITY`; the best one the keychain has).
    pub macos_identity: Option<String>,
    /// The Mac: the `xcrun notarytool store-credentials` profile that notarizes Developer ID builds
    /// (`ORBITERS_MACOS_NOTARY_PROFILE`; none: they are not notarized).
    pub macos_notary_profile: Option<String>,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let file = release_home().join("config.json");
        let mut config: Config = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("{}", file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(e).with_context(|| format!("{}", file.display())),
        };
        let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let path = |name: &str| env(name).map(PathBuf::from);
        let text = |name: &str| env(name).map(|v| v.to_string_lossy().into_owned());
        config.update_key = path("ORBITERS_UPDATE_KEY").or(config.update_key);
        config.signtool = path("ORBITERS_SIGNTOOL").or(config.signtool);
        config.signing_dlib = path("ORBITERS_SIGNING_DLIB").or(config.signing_dlib);
        config.signing_metadata = path("ORBITERS_SIGNING_METADATA").or(config.signing_metadata);
        config.timestamp_url = text("ORBITERS_TIMESTAMP_URL").or(config.timestamp_url);
        config.vcruntime = path("ORBITERS_VCRUNTIME").or(config.vcruntime);
        config.host = text("ORBITERS_RELEASE_HOST").or(config.host);
        config.remote_dir = text("ORBITERS_RELEASE_DIR").or(config.remote_dir);
        config.setup_target_dir = path("ORBITERS_SETUP_TARGET_DIR").or(config.setup_target_dir);
        config.dev_dir = path("ORBITERS_DEV_DOWNLOADS").or(config.dev_dir);
        config.macos_identity = text("ORBITERS_MACOS_IDENTITY").or(config.macos_identity);
        config.macos_notary_profile = text("ORBITERS_MACOS_NOTARY_PROFILE").or(config.macos_notary_profile);
        Ok(config)
    }

    pub fn dev_dir(&self) -> PathBuf {
        self.dev_dir.clone().unwrap_or_else(|| {
            let root = root();
            root.parent().unwrap_or(&root).join("server").join("docker-data").join("downloads").join("desktop")
        })
    }

    pub fn key_path(&self) -> PathBuf {
        self.update_key.clone().unwrap_or_else(|| release_home().join("update-signing-key"))
    }

    pub fn host(&self) -> String {
        self.host.clone().unwrap_or_else(|| "blackorbit@orbiters".into())
    }

    pub fn remote_dir(&self) -> String {
        self.remote_dir.clone().unwrap_or_else(|| "/home/blackorbit/Orbiters/server/docker-data/downloads/desktop".into()).trim_end_matches('/').to_string()
    }
}

/// How a release's programs are compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// The root Cargo.toml's release profile as it stands: thin LTO over one codegen unit a crate. What people install.
    Stable,
    /// The dev-release profile, for the builds Enzo tries (`release --dev`): the same optimisation level without the
    /// two settings that make a build slow. One codegen unit compiles each crate on a single core, and thin LTO
    /// optimises every dependency again at the link of each of the five programs and of the setup, at every version
    /// bump. Without them a crate's code is generated on sixteen cores and a link is only a link. The programs come out
    /// larger and at most a few percent slower (the reconstruction's time is the GPU's: the lab's "process speed"
    /// report put every compiler setting together at about 1 % of a run).
    ///
    /// It is the release profile with those two settings replaced on cargo's command line, not a
    /// `[profile.dev-release]` of its own in Cargo.toml. A named profile builds into `target/dev-release`, and
    /// sherpa-onnx-sys's build script copies its DLLs into the folder named after cargo's `PROFILE`, which says
    /// `release` for every profile that inherits it: it finds no such folder above its output and the build stops, or
    /// it finds an older `target/release` and the setup packs whatever lies there. This way everything stays where
    /// `dist` and every script look for it, `target/release`, and cargo keeps both sets of compiled dependencies there
    /// side by side (their file names carry a hash of the settings), so a stable release after development builds
    /// compiles nothing again but what changed.
    Dev,
}

impl Profile {
    /// What `cargo build --release` takes on top for this profile.
    pub fn cargo_args(self) -> &'static [&'static str] {
        match self {
            Profile::Stable => &[],
            Profile::Dev => &["--config", "profile.release.lto=false", "--config", "profile.release.codegen-units=16"],
        }
    }

    /// From a command's arguments: `--dev` asks for the development builds' profile.
    fn of(args: &[String]) -> Self {
        if flag(args, "--dev") {
            Profile::Dev
        } else {
            Profile::Stable
        }
    }

    /// This profile for a build of `version`, or why not: only a development version (`0.3.6-dev.2`) is built as
    /// development builds are. `dist` signs a release file for whatever it builds, in the same `dist/<version>` folder
    /// for both profiles, and `publish` checks the version, the hash and the signature, not how the programs were
    /// compiled: `dist --dev` on a stable version, then `publish`, would have put development-built programs out as
    /// the stable release. Asked before anything is built, and again where the build is made.
    pub fn for_version(self, version: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self == Profile::Stable || is_development(version),
            "{version} is not a development version: --dev compiles the programs as development builds are, and a stable \
             version's setup is only ever made with the full profile (cargo xtask bump --dev first, or leave --dev out)"
        );
        Ok(self)
    }
}

/// Whether `version` is a development build's: a version whose pre-release part is `dev.<number>`, as
/// [`next_dev_version`] writes it (`0.3.6-dev.2`). Read from the parsed version, never from its text: `1.0.0+build-dev.1`
/// holds `-dev.` in its build metadata and is the stable 1.0.0, and looking for the text let it be built, and released,
/// as a development build.
pub fn is_development(version: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|v| v.pre.as_str().strip_prefix("dev.").is_some_and(|n| n.parse::<u64>().is_ok()))
}

/// The repository's root (this crate's parent).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the repository").to_path_buf()
}

/// Cargo's target folder: `CARGO_TARGET_DIR`, or the repository's `target`.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| root().join("target"))
}

/// The release's version: `ORBITERS_VERSION` under `[env]` in `.cargo/config.toml`, which every program is built with
/// (`orbiters-version` says why). A development bump changes it alone; a stable one also `[workspace.package] version`.
///
/// It is the newer of that value and the crates' version, never older than either: on 2026-11-16 the merge that brought
/// this scheme carried a config written at 0.3.5 onto a main whose crates were at 0.3.6-dev.7, and `bump --dev`
/// continued from 0.3.5 to 0.3.6-dev.1, below the release already out.
pub fn release_version() -> anyhow::Result<String> {
    let config = std::fs::read_to_string(root().join(".cargo").join("config.toml"))?;
    let crates = std::fs::read_to_string(root().join("Cargo.toml"))?;
    current_version(&config, &crates)
}

/// The release's version from the texts of `.cargo/config.toml` and the root `Cargo.toml`: the newer of the two.
fn current_version(config: &str, cargo_toml: &str) -> anyhow::Result<String> {
    let env = env_version_line(config).map(|(_, v)| v).context("no ORBITERS_VERSION under [env] in .cargo/config.toml")?;
    let crates = version_line(cargo_toml).map(|(_, v)| v).context("no version under [workspace.package] in Cargo.toml")?;
    Ok(if orbiters_update::is_newer(&crates, &env) { crates } else { env })
}

/// `text` (`.cargo/config.toml`) with `ORBITERS_VERSION` set to `version`.
fn with_release_version(text: &str, version: &str) -> anyhow::Result<String> {
    let (line, _) = env_version_line(text).context("no ORBITERS_VERSION under [env] in .cargo/config.toml")?;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    lines[line] = format!("ORBITERS_VERSION = \"{version}\"");
    Ok(lines.join("\n") + "\n")
}

/// `text` (the root `Cargo.toml`) with `[workspace.package] version` set to `version`.
fn with_crate_version(text: &str, version: &str) -> anyhow::Result<String> {
    let (line, _) = version_line(text).context("no version under [workspace.package] in Cargo.toml")?;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    lines[line] = format!("version = \"{version}\"");
    Ok(lines.join("\n") + "\n")
}

/// The crates' own version (`[workspace.package] version` in the root Cargo.toml): the last stable release's.
pub fn crate_version() -> anyhow::Result<String> {
    let text = std::fs::read_to_string(root().join("Cargo.toml"))?;
    version_line(&text).map(|(_, v)| v).context("no version under [workspace.package] in Cargo.toml")
}

/// A stable release's programs carry its version as crates too; only development versions live in the config alone.
pub fn ensure_versions_agree(version: &str) -> anyhow::Result<()> {
    if !is_development(version) {
        let crates = crate_version()?;
        anyhow::ensure!(crates == version, "the release is {version} but the crates are {crates}: cargo xtask bump {version}");
    }
    Ok(())
}

/// The line index and value of `ORBITERS_VERSION = "…"` under `[env]`.
fn env_version_line(toml: &str) -> Option<(usize, String)> {
    let mut inside = false;
    for (i, line) in toml.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == "[env]";
        } else if inside {
            if let Some(rest) = trimmed.strip_prefix("ORBITERS_VERSION") {
                let value = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"');
                return Some((i, value.to_string()));
            }
        }
    }
    None
}

/// The line index and value of `version = "…"` under `[workspace.package]`.
fn version_line(toml: &str) -> Option<(usize, String)> {
    let mut inside = false;
    for (i, line) in toml.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == "[workspace.package]";
        } else if inside {
            if let Some(rest) = trimmed.strip_prefix("version") {
                let value = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"');
                return Some((i, value.to_string()));
            }
        }
    }
    None
}

/// A command, shown before it runs; an error when it fails.
pub fn run(cmd: &mut Command) -> anyhow::Result<()> {
    println!("> {}", shown(cmd));
    let status = cmd.status().with_context(|| format!("{} could not start", cmd.get_program().to_string_lossy()))?;
    anyhow::ensure!(status.success(), "{} failed ({status})", shown(cmd));
    Ok(())
}

/// A command's output; an error when it fails.
pub fn output(cmd: &mut Command) -> anyhow::Result<String> {
    let out = cmd.output().with_context(|| format!("{} could not start", cmd.get_program().to_string_lossy()))?;
    anyhow::ensure!(out.status.success(), "{} failed: {}", shown(cmd), String::from_utf8_lossy(&out.stderr).trim());
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn shown(cmd: &Command) -> String {
    std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
}

pub fn git(args: &[&str]) -> anyhow::Result<String> {
    output(Command::new("git").current_dir(root()).args(args)).map(|s| s.trim().to_string())
}

pub fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    // `cargo xtask` is `cargo run`, which gives this program its own package's variables (CARGO_PKG_VERSION_PRE is the
    // version being released). A cargo started from here passed them on to every build script, and ring's watches
    // CARGO_MANIFEST_DIR, CARGO_PKG_NAME and CARGO_PKG_VERSION_*: each version bump, and each switch between a build made
    // here and one made by hand, ran it again and recompiled ring and the 42 crates above it (rustls, reqwest, webrtc,
    // sherpa-onnx's users...). Cargo sets them anew for each package it builds; nothing needs ours.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(inherited_from_cargo_run) {
            cmd.env_remove(&key);
        }
    }
    cmd
}

/// Whether `name` is one of the variables `cargo run` sets for the program it runs, rather than one of cargo's settings
/// (CARGO, CARGO_HOME, CARGO_TARGET_DIR, CARGO_INCREMENTAL, CARGO_BUILD_*... are kept).
fn inherited_from_cargo_run(name: &str) -> bool {
    name.starts_with("CARGO_PKG_")
        || name.starts_with("CARGO_BIN_")
        || matches!(name, "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "CARGO_MANIFEST_LINKS" | "CARGO_CRATE_NAME" | "CARGO_PRIMARY_PACKAGE")
}

/// The release key's private half (PKCS#8), checked against the public half the apps are built with: a release they
/// would refuse is never made.
pub fn private_key(config: &Config) -> anyhow::Result<Vec<u8>> {
    let path = config.key_path();
    let text = std::fs::read_to_string(&path).with_context(|| format!("no release key at {} (cargo xtask keygen, or ORBITERS_UPDATE_KEY)", path.display()))?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).context("the release key file holds no key")?;
    let key = base64::engine::general_purpose::STANDARD.decode(line).context("the release key is not Base64")?;
    let public = orbiters_update::public_key_of(&key)?;
    anyhow::ensure!(
        Some(public) == orbiters_update::public_key(),
        "the release key at {} is not the one crates/orbiters-update/update-key.pub names: installed apps would refuse its releases",
        path.display()
    );
    Ok(key)
}

/// Whether `--name` is among the arguments.
fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// The value after `--name`.
fn value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run_command(&args) {
        eprintln!("\nerror: {e:#}");
        std::process::exit(1);
    }
}

fn run_command(args: &[String]) -> anyhow::Result<()> {
    command(args, &release_version)
}

/// A command, on the workspace whose version `version` reads.
fn command(args: &[String], version: &dyn Fn() -> anyhow::Result<String>) -> anyhow::Result<()> {
    let rest = args.get(1..).unwrap_or_default();
    let channel = value(rest, "--channel").unwrap_or_else(|| orbiters_update::STABLE.into());
    let notes = || -> anyhow::Result<String> {
        match value(rest, "--notes") {
            Some(file) => Ok(std::fs::read_to_string(&file).with_context(|| file.clone())?.trim().to_string()),
            None => Ok(String::new()),
        }
    };
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(&Config::load()?, flag(rest, "--force")),
        Some("bump") if flag(rest, "--dev") => bump(&next_dev_version(&release_version()?)?),
        Some("bump") => bump(rest.first().context("cargo xtask bump <version>")?),
        Some("dist") => {
            // On Windows and on a Mac alike, before a setting is read or a program built.
            let profile = Profile::of(rest).for_version(&version()?)?;
            if cfg!(target_os = "macos") {
                return macos::dist(&Config::load()?, &macos::Options { channel, notes: notes()?, profile }).map(drop);
            }
            let programs = value(rest, "--programs").map(PathBuf::from);
            let options = dist::Options { unsigned: flag(rest, "--unsigned"), channel, notes: notes()?, programs, profile };
            dist::dist(&Config::load()?, &options).map(drop)
        }
        Some("publish") if flag(rest, "--dev") => publish::publish_dev(&Config::load()?, &release_version()?, &channel, flag(rest, "--force")),
        Some("publish") => publish::publish(&Config::load()?, &release_version()?, &channel, flag(rest, "--force")),
        Some("release") if flag(rest, "--dev") => release_dev(&Config::load()?, &channel, notes()?, flag(rest, "--allow-dirty")),
        Some("release") => release(&Config::load()?, &channel, notes()?, flag(rest, "--allow-dirty"), flag(rest, "--unsigned")),
        Some("github") => publish::github(&release_version()?, &notes()?, flag(rest, "--unsigned")),
        _ => {
            println!("{HELP}");
            Ok(())
        }
    }
}

/// A new release key. Its private half signs releases; its public half, built into every app, is what they trust.
fn keygen(config: &Config, force: bool) -> anyhow::Result<()> {
    let path = config.key_path();
    anyhow::ensure!(
        force || !path.exists(),
        "a release key exists at {}: a new one would stop every installed app from trusting new releases (--force to replace it anyway)",
        path.display()
    );
    let (private, public) = orbiters_update::generate_key()?;
    let b64 = base64::engine::general_purpose::STANDARD;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!(
            "# Orbiters desktop's release key, private half (Ed25519, PKCS#8, Base64). Whoever has it can publish updates every\n\
             # installed Orbiters desktop installs: keep it off every repository and server, with a copy offline.\n{}\n",
            b64.encode(&private)
        ),
    )?;
    let public_file = root().join("crates").join("orbiters-update").join("update-key.pub");
    std::fs::write(
        &public_file,
        format!(
            "# The public half of Orbiters desktop's release key (Ed25519, Base64), written by `cargo xtask keygen`. Releases are\n\
             # signed with the private half, which stays off this repository and off the server.\n{}\n",
            b64.encode(public)
        ),
    )?;
    println!(
        "Private half: {}\n  Keep a copy offline (a password manager). Lost, no installed app updates again; leaked, anyone can update them.",
        path.display()
    );
    println!("Public half: {}\n  Commit it: apps built from then on trust releases signed with this key.", public_file.display());
    Ok(())
}

/// Sets the workspace's version, which every program and the release carry.
fn bump(version: &str) -> anyhow::Result<()> {
    let version = version.trim_start_matches('v');
    anyhow::ensure!(orbiters_update::is_newer(version, &release_version()?), "{version} is not after the current version, {}", release_version()?);
    let config = root().join(".cargo").join("config.toml");
    std::fs::write(&config, with_release_version(&std::fs::read_to_string(&config)?, version)?)?;
    if is_development(version) {
        // The crates keep the last stable version: only the programs, which read ORBITERS_VERSION, recompile.
        println!("Version {version}. Commit .cargo/config.toml, then: cargo xtask release --dev");
    } else {
        let path = root().join("Cargo.toml");
        std::fs::write(&path, with_crate_version(&std::fs::read_to_string(&path)?, version)?)?;
        run(cargo().args(["update", "--workspace", "--offline"]))?;
        println!("Version {version}. Commit .cargo/config.toml, Cargo.toml and Cargo.lock, push, then: cargo xtask release");
    }
    Ok(())
}

/// Everything a version bump needs: the checks, `dist`, `publish`, the tag, the GitHub release. `unsigned`: before
/// Authenticode signing is set up, a setup without signatures, on GitHub only.
fn release(config: &Config, channel: &str, notes: String, allow_dirty: bool, unsigned: bool) -> anyhow::Result<()> {
    let version = release_version()?;
    let tag = format!("desktop-v{version}");

    // The release names a commit: one that is committed, pushed, and not released yet.
    if !allow_dirty {
        let changes = git(&["status", "--porcelain", "--untracked-files=no"])?;
        anyhow::ensure!(changes.is_empty(), "changes are not committed (--allow-dirty to release them anyway):\n{changes}");
    }
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    anyhow::ensure!(channel != orbiters_update::STABLE || branch == "main", "the stable channel is released from main, not {branch}");
    anyhow::ensure!(git(&["tag", "--list", &tag])?.is_empty(), "{tag} exists: bump the version first (cargo xtask bump <version>)");
    let _ = git(&["fetch", "--quiet", "origin"]);
    let pushed = git(&["branch", "--remotes", "--contains", "HEAD"])?;
    anyhow::ensure!(!pushed.is_empty(), "this commit is not pushed: push it first, the release names it");
    publish::ensure_newer(&version, channel)?;

    let dist = dist::dist(config, &dist::Options { unsigned, channel: channel.into(), notes: notes.clone(), programs: None, profile: Profile::Stable })?;
    publish::publish(config, &version, channel, false)?;

    let tagged = git(&["tag", "--annotate", &tag, "--message", &format!("Orbiters - N4N0 {version}")]).and_then(|_| git(&["push", "origin", &tag]));
    match tagged {
        Ok(_) => println!("Tagged {tag}."),
        Err(e) => {
            println!("The release is out, but tagging it failed ({e:#}): git tag -a {tag} -m \"Orbiters - N4N0 {version}\" && git push origin {tag}, then cargo xtask github");
            return Ok(());
        }
    }
    publish::github(&version, &notes, unsigned)?;
    println!("\nOrbiters - N4N0 {version} is released on {channel} and GitHub ({}).", dist.display());
    Ok(())
}

/// The next development build's version after `current`: `0.3.2-dev.1` after `0.3.1`, `0.3.2-dev.5` after
/// `0.3.2-dev.4`.
fn next_dev_version(current: &str) -> anyhow::Result<String> {
    let mut v = semver::Version::parse(current).with_context(|| format!("{current} is not a version"))?;
    let n = v.pre.as_str().strip_prefix("dev.").and_then(|n| n.parse::<u64>().ok());
    match n {
        Some(n) => v.pre = semver::Prerelease::new(&format!("dev.{}", n + 1))?,
        None => {
            anyhow::ensure!(v.pre.is_empty(), "{current} is a pre-release that is not a development build");
            v.patch += 1;
            v.pre = semver::Prerelease::new("dev.1")?;
        }
    }
    Ok(v.to_string())
}

/// A development build (`cargo xtask bump --dev` first, committed): built with [`Profile::Dev`] and packed as a
/// release, put in the development environment's feed on this computer, where installed apps switched to the
/// development environment find it within two minutes, then pushed, tagged and made a GitHub pre-release.
fn release_dev(config: &Config, channel: &str, notes: String, allow_dirty: bool) -> anyhow::Result<()> {
    let version = release_version()?;
    anyhow::ensure!(is_development(&version), "{version} is not a development version: cargo xtask bump --dev, commit, then release --dev");
    if !allow_dirty {
        let changes = git(&["status", "--porcelain", "--untracked-files=no"])?;
        anyhow::ensure!(changes.is_empty(), "changes are not committed (--allow-dirty to build them anyway):\n{changes}");
    }
    orbiters_update::set_development(true);
    publish::ensure_newer(&version, channel)?;
    let notes = if notes.is_empty() { format!("Development build {version} ({})", git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default()) } else { notes };
    let dist = if cfg!(target_os = "macos") {
        macos::dist(config, &macos::Options { channel: channel.into(), notes: notes.clone(), profile: Profile::Dev })?
    } else {
        dist::dist(config, &dist::Options { unsigned: true, channel: channel.into(), notes: notes.clone(), programs: None, profile: Profile::Dev })?
    };
    publish::publish_dev(config, &version, channel, false)?;
    // Every build Enzo is to try is also on GitHub's releases (a pre-release), tagged on the pushed commit.
    let tag = format!("desktop-v{version}");
    git(&["-C", "contracts", "push", "origin", "HEAD:main"])?;
    git(&["push", "origin", "HEAD"])?;
    if git(&["tag", "--list", &tag])?.is_empty() {
        git(&["tag", "--annotate", &tag, "--message", &format!("Orbiters - N4N0 {version}")])?;
    }
    git(&["push", "origin", &tag])?;
    publish::github(&version, &notes, true)?;
    println!("\nOrbiters - N4N0 {version} is in the development environment ({}) and on GitHub's releases.", dist.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cargo started for a release does not hand this program's package variables to build scripts (ring reran at
    /// every version bump), and keeps cargo's own settings.
    #[test]
    fn a_cargo_started_here_gets_none_of_cargo_runs_package_variables() {
        // `cargo test` sets these for this test binary too, as `cargo run` does for xtask.
        assert!(std::env::var_os("CARGO_PKG_VERSION").is_some() && std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        let cmd = cargo();
        let removed: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        for name in ["CARGO_PKG_VERSION", "CARGO_PKG_VERSION_PRE", "CARGO_PKG_NAME", "CARGO_MANIFEST_DIR"] {
            assert!(removed.iter().any(|r| r == name), "{name} is passed on: {removed:?}");
        }
        for kept in ["CARGO", "CARGO_HOME", "CARGO_TARGET_DIR", "CARGO_INCREMENTAL", "CARGO_BUILD_JOBS"] {
            assert!(!inherited_from_cargo_run(kept), "{kept}");
        }
    }

    #[test]
    fn the_release_version_is_found_under_env() {
        let toml = "[alias]\nORBITERS_VERSION = \"9\"\n[env]\nCMAKE_POLICY_VERSION_MINIMUM = \"3.5\"\nORBITERS_VERSION = \"0.3.6-dev.2\"\n";
        assert_eq!(env_version_line(toml), Some((4, "0.3.6-dev.2".into())));
        assert_eq!(env_version_line("[env]\nOTHER = \"1\"\n"), None);
        // The repository's own config has one, and a stable one agrees with the crates.
        let version = release_version().unwrap();
        assert!(is_development(&version) || crate_version().unwrap() == version, "{version}");
    }

    /// The repository's ORBITERS_VERSION is never older than its crates' version: a merge that carries an older config
    /// value is caught here, before a bump goes backwards (0.3.6-dev.1 after 0.3.6-dev.7 on 2026-11-16).
    #[test]
    fn the_config_version_is_not_older_than_the_crates() {
        let config = std::fs::read_to_string(root().join(".cargo").join("config.toml")).unwrap();
        let env = env_version_line(&config).unwrap().1;
        let crates = crate_version().unwrap();
        assert!(!orbiters_update::is_newer(&crates, &env), "ORBITERS_VERSION {env} is older than the crates' {crates}");
    }

    /// Bumps go on from the real current version: dev.N gives dev.N+1, also when the config value is stale, and a stable
    /// bump still sets both and is followed by the next patch's dev.1.
    #[test]
    fn a_bump_goes_on_from_the_current_version() {
        let config = |v: &str| format!("[alias]\nxtask = \"run --package xtask --\"\n[env]\nCMAKE_POLICY_VERSION_MINIMUM = \"3.5\"\nORBITERS_VERSION = \"{v}\"\n");
        let crates = |v: &str| format!("[workspace]\nmembers = []\n[workspace.package]\nversion = \"{v}\"\nedition = \"2021\"\n");
        // Both in step on the new scheme.
        let now = current_version(&config("0.3.6-dev.8"), &crates("0.3.6-dev.7")).unwrap();
        assert_eq!(next_dev_version(&now).unwrap(), "0.3.6-dev.9");
        // The stale config of 2026-11-16: the crates' dev.7 wins, the next is dev.8.
        let now = current_version(&config("0.3.5"), &crates("0.3.6-dev.7")).unwrap();
        assert_eq!(now, "0.3.6-dev.7");
        let next = next_dev_version(&now).unwrap();
        assert_eq!(next, "0.3.6-dev.8");
        // A dev bump writes the config alone, and the next bump continues from it.
        let after = with_release_version(&config("0.3.5"), &next).unwrap();
        assert!(after.contains("CMAKE_POLICY_VERSION_MINIMUM = \"3.5\"") && after.contains("xtask = "));
        assert_eq!(next_dev_version(&current_version(&after, &crates("0.3.6-dev.7")).unwrap()).unwrap(), "0.3.6-dev.9");
        // A stable bump sets both; they agree, and the next development version is the next patch's first.
        let (config_after, crates_after) = (with_release_version(&after, "0.3.6").unwrap(), with_crate_version(&crates("0.3.6-dev.7"), "0.3.6").unwrap());
        assert_eq!(current_version(&config_after, &crates_after).unwrap(), "0.3.6");
        assert_eq!(version_line(&crates_after).unwrap().1, "0.3.6");
        assert_eq!(next_dev_version("0.3.6").unwrap(), "0.3.7-dev.1");
        assert!(orbiters_update::is_newer("0.3.6", "0.3.6-dev.8"));
    }

    /// Each program gives the libraries its version first thing (a worker that did not would report the crates'
    /// version, and the shell refuses a worker of another version); setup and the SteamVR driver read it directly.
    #[test]
    fn every_program_gives_the_release_version_at_start() {
        for crate_name in ["orbiters-desktop", "orbiters-capture", "orbiters-reconstruct", "orbiters-voice", "orbiters-call"] {
            let main = std::fs::read_to_string(root().join("crates").join(crate_name).join("src").join("main.rs")).unwrap();
            let body = main.split("\nfn main()").nth(1).unwrap_or_else(|| panic!("{crate_name} has no main"));
            let first: String = body.lines().skip(1).filter(|l| !l.trim_start().starts_with("//")).take(1).collect();
            assert_eq!(first.trim(), "orbiters_version::set(env!(\"ORBITERS_VERSION\"));", "{crate_name}");
        }
        for (crate_name, file) in [("orbiters-setup", "ui.rs"), ("orbiters-smooth-driver", "lib.rs")] {
            let text = std::fs::read_to_string(root().join("crates").join(crate_name).join("src").join(file)).unwrap();
            assert!(text.contains("env!(\"ORBITERS_VERSION\")") && !text.contains("CARGO_PKG_VERSION"), "{crate_name}");
        }
    }

    #[test]
    fn the_workspace_version_is_found_under_its_section() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[package]\nversion = \"9.9.9\"\n";
        assert_eq!(version_line(toml), Some((4, "0.1.0".into())));
        assert_eq!(version_line("[package]\nversion = \"1.0.0\"\n"), None);
    }

    #[test]
    fn development_builds_count_up_after_a_release() {
        assert_eq!(next_dev_version("0.3.1").unwrap(), "0.3.2-dev.1");
        assert_eq!(next_dev_version("0.3.2-dev.1").unwrap(), "0.3.2-dev.2");
        assert_eq!(next_dev_version("0.3.2-dev.9").unwrap(), "0.3.2-dev.10");
        assert!(next_dev_version("0.4.0-beta.1").is_err(), "another pre-release is not a development build");
        assert!(orbiters_update::is_newer("0.3.2-dev.2", "0.3.2-dev.1") && orbiters_update::is_newer("0.3.2", "0.3.2-dev.9"));
    }

    /// A development build replaces the release profile's two slow settings and nothing else, and that profile still
    /// has them: a stable release is compiled as it always was.
    #[test]
    fn development_builds_replace_only_the_slow_settings() {
        assert!(Profile::Stable.cargo_args().is_empty(), "a stable release is the manifest's profile, untouched");
        let dev = Profile::Dev.cargo_args();
        assert!(dev.chunks(2).all(|pair| pair[0] == "--config"));
        let settings: Vec<&str> = dev.chunks(2).map(|pair| pair[1]).collect();
        assert_eq!(settings, ["profile.release.lto=false", "profile.release.codegen-units=16"]);
        let toml = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
        let release: Vec<&str> = toml.lines().skip_while(|l| l.trim() != "[profile.release]").skip(1).take_while(|l| !l.trim().starts_with('[')).map(str::trim).collect();
        for kept in [r#"lto = "thin""#, "codegen-units = 1", "strip = true"] {
            assert!(release.contains(&kept), "the release profile lost {kept}: {release:?}");
        }
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(Profile::of(&args(&["--unsigned", "--dev"])), Profile::Dev);
        assert_eq!(Profile::of(&args(&["--unsigned"])), Profile::Stable);
    }

    /// `dist --dev` on a stable version is refused, as a command, before anything is read or built: with `publish` after
    /// it, it shipped development-built programs as a stable release. A development version takes either profile, and
    /// `release --dev`, which only runs on one, keeps building as development builds are.
    #[test]
    fn a_stable_version_is_never_built_as_a_development_build() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        // Stable versions, another pre-release, and stable versions whose build metadata or pre-release only looks like
        // a development build's.
        for stable in ["0.3.5", "1.0.0", "0.4.0-beta.1", "1.0.0+build-dev.1", "1.0.0+x-dev.2", "0.3.5-dev.x", "0.3.5-rc-dev.1", "0.3.5-dev"] {
            let version = || Ok(stable.to_string());
            for command_line in [&["dist", "--dev"][..], &["dist", "--dev", "--unsigned"], &["dist", "--unsigned", "--channel", "stable", "--dev"], &["dist", "--programs", "elsewhere", "--dev"]] {
                let refused = command(&args(command_line), &version).expect_err("built a stable version as a development build");
                let said = format!("{refused:#}");
                assert!(said.contains(stable) && said.contains("not a development version"), "{said}");
            }
            assert!(Profile::Dev.for_version(stable).is_err());
            assert_eq!(Profile::Stable.for_version(stable).unwrap(), Profile::Stable);
        }
        // The version is asked for the refusal alone: a failure to read it is the command's failure, not a build.
        assert!(command(&args(&["dist", "--dev"]), &|| anyhow::bail!("no Cargo.toml")).is_err());
        for development in ["0.3.6-dev.1", "0.3.5-dev.32", "0.3.5-dev.33", "0.3.5-dev.33+built.today"] {
            assert!(is_development(development), "{development}");
            assert_eq!(Profile::Dev.for_version(development).unwrap(), Profile::Dev);
            assert_eq!(Profile::Stable.for_version(development).unwrap(), Profile::Stable);
        }
        // What `bump --dev` writes is what the guards take.
        assert!(is_development(&next_dev_version("0.3.5").unwrap()) && is_development(&next_dev_version("0.3.5-dev.33").unwrap()));
        assert!(!is_development("not a version") && !is_development("1.0.0+build-dev.1"));
    }

    #[test]
    fn options_are_read_after_their_flag() {
        let args: Vec<String> = ["--channel", "beta", "--force"].map(String::from).to_vec();
        assert_eq!(value(&args, "--channel").as_deref(), Some("beta"));
        assert!(flag(&args, "--force"));
        assert_eq!(value(&args, "--notes"), None);
    }
}
