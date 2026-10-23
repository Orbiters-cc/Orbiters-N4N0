//! `cargo xtask dist`: `target/dist/<version>/OrbitersSetup-<version>.exe` and the signed `<channel>.json` that
//! announces it.
//!
//! 1. The programs are built in release (`target/release`, as every other release build here).
//! 2. The setup without programs inside is built with the C runtime linked in (`+crt-static`, its own target folder),
//!    so that it starts on a Windows without Visual C++'s runtime: it is the uninstaller left in the install folder.
//! 3. The programs, their libraries, Visual C++'s runtime (`vcruntime140.dll`, which they need and a clean Windows
//!    lacks), the licences and the uninstaller are gathered; each file is checked to need nothing else.
//! 4. Ours are signed (Azure Artifact Signing), then packed (`orbiters_update::payload`).
//! 5. The setup is built again with the payload inside, and signed: the one people download and apps update with.
//! 6. The release file names it (size, SHA-256) and is signed with the release key.

use crate::{cargo, pe, run, Config};
use anyhow::Context as _;
use orbiters_update::{payload, Artifact, Release, SignedRelease, PRODUCT, SETUP_EXE};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The programs the setup installs (`scripts/third_party_licenses.py` lists the same).
const PROGRAMS: [&str; 5] = ["orbiters-desktop", "orbiters-capture", "orbiters-reconstruct", "orbiters-voice", "orbiters-call"];
/// The libraries they load, beside them in `target/release` (sherpa-onnx's prebuilt ones, which its build copies there).
const LIBRARIES: [&str; 3] = ["onnxruntime.dll", "onnxruntime_providers_shared.dll", "sherpa-onnx-c-api.dll"];
/// The licences, from the repository's root.
const DOCUMENTS: [&str; 3] = ["LICENSE.md", "THIRD_PARTY_LICENSES.md", "THIRD_PARTY_NOTICES.md"];
/// The C runtime the programs link to, shipped beside them (Microsoft allows it, app-local).
const VCRUNTIME: &str = "vcruntime140.dll";
/// The setup's own target, so its static C runtime does not mix with the programs' build.
const SETUP_TRIPLE: &str = "x86_64-pc-windows-msvc";
/// Microsoft's timestamp server for Artifact Signing: its certificates last three days, the timestamp keeps a
/// signature valid after.
const TIMESTAMP: &str = "http://timestamp.acs.microsoft.com";

pub struct Options {
    /// Without Authenticode signatures (a local test build): Windows' SmartScreen warns about such a setup.
    pub unsigned: bool,
    pub channel: String,
    pub notes: String,
    /// Packs the programs (and their libraries) already in this folder instead of building them: a test of the
    /// setup with another build's programs. `release` never does.
    pub programs: Option<PathBuf>,
}

pub fn dist(config: &Config, options: &Options) -> anyhow::Result<PathBuf> {
    let version = crate::workspace_version()?;
    let target = crate::target_dir();
    let signer = Signer::new(config)?;
    anyhow::ensure!(signer.is_some() || options.unsigned, "{}", SIGNING_HELP);
    // A test build (unsigned, or another build's programs) gets no release file: `publish` cannot put it out.
    let test = options.unsigned || options.programs.is_some();
    let key = if test { None } else { Some(crate::private_key(config)?) };

    let dist = target.join("dist").join(&version);
    if dist.exists() {
        std::fs::remove_dir_all(&dist).with_context(|| format!("{} cannot be emptied", dist.display()))?;
    }
    let stage = dist.join("stage");
    std::fs::create_dir_all(&stage)?;

    println!("\n== Programs {version}");
    let bin = match &options.programs {
        Some(dir) => {
            println!("Taken from {} (a test build)", dir.display());
            dir.clone()
        }
        None => {
            let mut build = cargo();
            build.args(["build", "--release", "--locked"]).env("CARGO_TARGET_DIR", &target).env_remove("ORBITERS_SETUP_PAYLOAD");
            for program in PROGRAMS {
                build.args(["-p", program]);
            }
            run(&mut build)?;
            target.join("release")
        }
    };
    let mut ours = Vec::new();
    for program in PROGRAMS {
        let name = format!("{program}.exe");
        copy(&bin.join(&name), &stage.join(&name))?;
        ours.push(stage.join(&name));
    }
    for library in LIBRARIES {
        copy(&bin.join(library), &stage.join(library))?;
    }
    copy(&vcruntime(config)?, &stage.join(VCRUNTIME))?;
    for document in DOCUMENTS {
        copy(&crate::root().join(document), &stage.join(document))?;
    }

    println!("\n== Uninstaller");
    let setup_target = config.setup_target_dir.clone().unwrap_or_else(|| target.clone());
    let stub = build_setup(&setup_target, None)?;
    copy(&stub, &stage.join(SETUP_EXE))?;
    ours.push(stage.join(SETUP_EXE));

    // Everything a file needs is Windows' or shipped: a clean Windows runs it.
    let files: Vec<PathBuf> =
        std::fs::read_dir(&stage)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "exe" || x == "dll")).collect();
    let shipped: Vec<String> = files.iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    let missing = pe::missing(&files, &shipped)?;
    anyhow::ensure!(missing.is_empty(), "files need DLLs that are neither Windows' nor shipped: {missing:?}");

    if let Some(signer) = &signer {
        println!("\n== Signing the programs");
        signer.sign(&ours)?;
    }

    println!("\n== Payload");
    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(&stage)?.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
    entries.sort();
    let packed = dist.join("payload.tar.zst");
    let index = payload::pack(&version, &entries, &packed)?;
    println!("{} files, {} MB, packed to {} MB", index.files.len(), index.total() >> 20, std::fs::metadata(&packed)?.len() >> 20);

    println!("\n== Setup");
    let full = build_setup(&setup_target, Some(&packed))?;
    let setup = dist.join(format!("OrbitersSetup-{version}.exe"));
    copy(&full, &setup)?;
    let missing = pe::missing(std::slice::from_ref(&setup), &[])?;
    anyhow::ensure!(missing.is_empty(), "the setup needs DLLs a clean Windows lacks: {missing:?}");
    if let Some(signer) = &signer {
        signer.sign(std::slice::from_ref(&setup))?;
    }

    let installer = Artifact {
        url: format!("{version}/OrbitersSetup-{version}.exe"),
        sha256: orbiters_update::sha256_file(&setup)?,
        size: std::fs::metadata(&setup)?.len(),
    };
    if let Some(key) = key {
        let release = Release {
            product: PRODUCT.into(),
            channel: options.channel.clone(),
            version: version.clone(),
            released: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?,
            commit: crate::git(&["rev-parse", "HEAD"]).unwrap_or_default(),
            notes: options.notes.clone(),
            installer: installer.clone(),
        };
        let signed = SignedRelease::sign(&release, &key)?;
        std::fs::write(dist.join(format!("{}.json", options.channel)), serde_json::to_vec_pretty(&signed)?)?;
    }
    // The setup holds them: only it and the release file stay.
    let _ = std::fs::remove_dir_all(&stage);
    let _ = std::fs::remove_file(&packed);

    println!(
        "\n{}\n  {} MB, SHA-256 {}{}",
        setup.display(),
        installer.size >> 20,
        installer.sha256,
        if test { "\n  A TEST BUILD: no release file, never to publish." } else { "" }
    );
    Ok(dist)
}

/// Builds the setup with the C runtime inside, with `payload` in it or none (the uninstaller); returns where it is.
fn build_setup(target: &Path, payload: Option<&Path>) -> anyhow::Result<PathBuf> {
    let mut build = cargo();
    build
        .args(["build", "--release", "--locked", "-p", "orbiters-setup", "--target", SETUP_TRIPLE])
        .env("CARGO_TARGET_DIR", target)
        // Taken before the target's own flags: they would drop `+crt-static`.
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS", "-C target-feature=+crt-static");
    match payload {
        Some(p) => build.env("ORBITERS_SETUP_PAYLOAD", p),
        None => build.env_remove("ORBITERS_SETUP_PAYLOAD"),
    };
    run(&mut build)?;
    Ok(target.join(SETUP_TRIPLE).join("release").join(SETUP_EXE))
}

fn copy(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::copy(from, to).with_context(|| format!("{} cannot be copied", from.display()))?;
    Ok(())
}

/// Visual C++'s `vcruntime140.dll`: the configured one, Visual Studio's redistributable, or System32's.
fn vcruntime(config: &Config) -> anyhow::Result<PathBuf> {
    if let Some(path) = &config.vcruntime {
        anyhow::ensure!(path.is_file(), "no {VCRUNTIME} at {}", path.display());
        return Ok(path.clone());
    }
    let vswhere = PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    if let Ok(found) = crate::output(Command::new(&vswhere).args(["-latest", "-products", "*", "-property", "installationPath"])) {
        let redist = PathBuf::from(found.trim()).join("VC").join("Redist").join("MSVC");
        let mut versions: Vec<PathBuf> = std::fs::read_dir(&redist).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        versions.sort_by_key(|p| version_key(&p.file_name().unwrap_or_default().to_string_lossy()));
        for dir in versions.iter().rev() {
            let crts = std::fs::read_dir(dir.join("x64")).into_iter().flatten().flatten().map(|e| e.path());
            if let Some(dll) =
                crts.filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".CRT"))).map(|p| p.join(VCRUNTIME)).find(|p| p.is_file())
            {
                return Ok(dll);
            }
        }
    }
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32").join(VCRUNTIME);
    anyhow::ensure!(system.is_file(), "no {VCRUNTIME} found: install Visual Studio's C++ tools, or set ORBITERS_VCRUNTIME");
    Ok(system)
}

/// A dotted version as numbers, to sort folders named after versions.
fn version_key(name: &str) -> Vec<u32> {
    name.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

const SIGNING_HELP: &str = "Signing is not set up (Azure Artifact Signing): set signing_dlib and signing_metadata in \
~/.orbiters-release/config.json (or ORBITERS_SIGNING_DLIB and ORBITERS_SIGNING_METADATA), sign in with `az login`, \
and see docs/releasing.md. `--unsigned` makes a test build that is never to be published.";

/// Authenticode signatures through Azure Artifact Signing (signtool with its plug-in; the account signed in with
/// `az login` or another Azure credential).
struct Signer {
    signtool: PathBuf,
    dlib: PathBuf,
    metadata: PathBuf,
    timestamp: String,
}

impl Signer {
    /// The signer the settings describe; `None` when they describe none.
    fn new(config: &Config) -> anyhow::Result<Option<Self>> {
        let (Some(dlib), Some(metadata)) = (&config.signing_dlib, &config.signing_metadata) else { return Ok(None) };
        anyhow::ensure!(dlib.is_file(), "no signing plug-in at {}", dlib.display());
        anyhow::ensure!(metadata.is_file(), "no signing account file at {}", metadata.display());
        let signtool = match &config.signtool {
            Some(path) => path.clone(),
            None => find_signtool().context("no signtool.exe: install the Windows SDK, or set ORBITERS_SIGNTOOL")?,
        };
        Ok(Some(Self { signtool, dlib: dlib.clone(), metadata: metadata.clone(), timestamp: config.timestamp_url.clone().unwrap_or_else(|| TIMESTAMP.into()) }))
    }

    fn sign(&self, files: &[PathBuf]) -> anyhow::Result<()> {
        let mut cmd = Command::new(&self.signtool);
        cmd.args(["sign", "/v", "/fd", "SHA256", "/tr", &self.timestamp, "/td", "SHA256", "/dlib"])
            .arg(&self.dlib)
            .arg("/dmdf")
            .arg(&self.metadata)
            .args(files);
        run(&mut cmd)?;
        for file in files {
            run(Command::new(&self.signtool).args(["verify", "/pa", "/q"]).arg(file)).with_context(|| format!("{} is not signed after all", file.display()))?;
        }
        Ok(())
    }
}

/// The newest x64 signtool of the Windows SDK.
fn find_signtool() -> Option<PathBuf> {
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> =
        std::fs::read_dir(&kits).ok()?.flatten().map(|e| e.path()).filter(|p| p.join("x64").join("signtool.exe").is_file()).collect();
    versions.sort_by_key(|p| version_key(&p.file_name().unwrap_or_default().to_string_lossy()));
    versions.last().map(|p| p.join("x64").join("signtool.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_folders_sort_as_numbers() {
        let mut names = vec!["14.9.1", "14.40.33807", "14.38.33130"];
        names.sort_by_key(|n| version_key(n));
        assert_eq!(names, ["14.9.1", "14.38.33130", "14.40.33807"]);
    }
}
