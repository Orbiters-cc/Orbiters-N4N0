//! `cargo xtask dist`: `target/dist/<version>/OrbitersSetup-<version>.exe` and the signed `<channel>.json` that
//! announces it.
//!
//! 1. The programs are built in release (`target/release`, as every other release build here); a development build
//!    with [`Profile::Dev`]'s settings, which compile several times faster.
//! 2. The setup without programs inside is built with the C runtime linked in (`+crt-static`, its own target folder),
//!    so that it starts on a Windows without Visual C++'s runtime: it is the uninstaller left in the install folder.
//! 3. The programs, their libraries, Visual C++'s runtime (`vcruntime140.dll`, which they need and a clean Windows
//!    lacks), the licences and the uninstaller are gathered; each file is checked to need nothing else.
//! 4. Ours are signed (Azure Artifact Signing), then packed (`orbiters_update::payload`).
//! 5. The setup is built again with the payload inside, and signed: the one people download and apps update with.
//! 6. The release file names it (size, SHA-256) and is signed with the release key.

use crate::{cargo, pe, run, Config, Profile};
use anyhow::Context as _;
use orbiters_update::{payload, Artifact, Release, SignedRelease, PRODUCT, SETUP_EXE};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The programs the setup installs (`scripts/third_party_licenses.py` lists the same).
pub(crate) const PROGRAMS: [&str; 5] = ["orbiters-desktop", "orbiters-capture", "orbiters-reconstruct", "orbiters-voice", "orbiters-call"];
/// The libraries they load, beside them in `target/release` (sherpa-onnx's prebuilt ones, which its build copies there).
const LIBRARIES: [&str; 3] = ["onnxruntime.dll", "onnxruntime_providers_shared.dll", "sherpa-onnx-c-api.dll"];
/// The reconstruction's face detector (orbiters_reconstruct::portraits), from the repository; the worker finds it beside
/// itself, and without it trains as it did. See THIRD_PARTY_NOTICES.md.
const FACE_MODEL: &str = "crates/orbiters-reconstruct/assets/faces-v1-int8.onnx";
/// Tracking smoothing's SteamVR driver (`orbiters-smooth-driver`), which the app installs for SteamVR when it is
/// switched on.
const DRIVER: &str = "driver_orbiters_smooth.dll";
/// The licences, from the repository's root.
const DOCUMENTS: [&str; 3] = ["LICENSE.md", "THIRD_PARTY_LICENSES.md", "THIRD_PARTY_NOTICES.md"];
/// The C runtime the programs link to, shipped beside them (Microsoft allows it, app-local).
const VCRUNTIME: &str = "vcruntime140.dll";
/// The setup's own target, so its static C runtime does not mix with the programs' build.
const SETUP_TRIPLE: &str = "x86_64-pc-windows-msvc";
/// Microsoft's timestamp server for Artifact Signing: its certificates last three days, the timestamp keeps a
/// signature valid after.
const TIMESTAMP: &str = "http://timestamp.acs.microsoft.com";

pub struct Options<'a> {
    /// Without Authenticode signatures (until Azure Artifact Signing is set up): Windows' SmartScreen warns about such
    /// a setup when it is downloaded by hand; installed apps update to it as to any release the release key signed.
    pub unsigned: bool,
    pub channel: String,
    pub notes: String,
    /// Packs the programs (and their libraries) already in this folder instead of building them: a test of the
    /// setup with another build's programs. `release` never does.
    pub programs: Option<PathBuf>,
    /// How the programs and the setup are compiled.
    pub profile: Profile,
    /// What they are compiled from: the release file names it.
    pub source: &'a crate::commit::Source,
}

pub fn dist(config: &Config, options: &Options<'_>) -> anyhow::Result<PathBuf> {
    let version = crate::release_version()?;
    // Whoever asks: a stable version's programs are never compiled as development builds are.
    options.profile.for_version(&version)?;
    crate::ensure_versions_agree(&version)?;
    crate::licenses::check(&PROGRAMS)?;
    let target = crate::target_dir();
    let signer = Signer::new(config)?;
    anyhow::ensure!(signer.is_some() || options.unsigned, "{}", SIGNING_HELP);
    // A test build (another build's programs) gets no release file: `publish` cannot put it out. A setup without
    // Authenticode signatures still gets one: installed apps trust a release for the release key's signature, and
    // only a setup downloaded by hand meets SmartScreen.
    let test = options.programs.is_some();
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
            build.args(["build", "--release", "--locked"]).args(options.profile.cargo_args()).env("CARGO_TARGET_DIR", &target).env_remove("ORBITERS_SETUP_PAYLOAD");
            for program in PROGRAMS {
                build.args(["-p", program]);
            }
            build.args(["-p", "orbiters-smooth-driver"]);
            run(&mut build)?;
            target.join("release")
        }
    };
    check_programs(&bin, &version)?;
    let mut ours = Vec::new();
    for program in PROGRAMS {
        let name = format!("{program}.exe");
        copy(&bin.join(&name), &stage.join(&name))?;
        ours.push(stage.join(&name));
    }
    for library in LIBRARIES {
        copy(&bin.join(library), &stage.join(library))?;
    }
    let face_model = crate::root().join(FACE_MODEL);
    copy(&face_model, &stage.join(face_model.file_name().expect("a file")))?;
    copy(&bin.join(DRIVER), &stage.join(DRIVER))?;
    ours.push(stage.join(DRIVER));
    check_staged(&stage, &version)?;
    let runtime = vcruntime(config)?;
    copy(&runtime, &stage.join(VCRUNTIME))?;
    // The camera/WebView desktop also imports the x64 exception-handling runtime. Keep both from the same redist.
    let companion = "vcruntime140_1.dll";
    copy(&runtime.with_file_name(companion), &stage.join(companion))?;
    for document in DOCUMENTS {
        copy(&crate::root().join(document), &stage.join(document))?;
    }

    println!("\n== Uninstaller");
    let setup_target = config.setup_target_dir.clone().unwrap_or_else(|| target.clone());
    let stub = build_setup(&setup_target, None, options.profile)?;
    copy(&stub, &stage.join(SETUP_EXE))?;
    ours.push(stage.join(SETUP_EXE));

    // Everything a file needs is Windows' or shipped: a clean Windows runs it.
    let files: Vec<PathBuf> =
        std::fs::read_dir(&stage)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "exe" || x == "dll")).collect();
    let shipped: Vec<String> = files.iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    let missing = pe::missing(&files, &shipped)?;
    anyhow::ensure!(missing.is_empty(), "files need DLLs that are neither Windows' nor shipped: {missing:?}");

    // Nothing is signed unless the sources still are the commit's.
    options.source.built()?;
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
    let full = build_setup(&setup_target, Some(&packed), options.profile)?;
    let setup = dist.join(format!("OrbitersSetup-{version}.exe"));
    copy(&full, &setup)?;
    let missing = pe::missing(std::slice::from_ref(&setup), &[])?;
    anyhow::ensure!(missing.is_empty(), "the setup needs DLLs a clean Windows lacks: {missing:?}");
    let commit = options.source.built()?;
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
            commit,
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
fn build_setup(target: &Path, payload: Option<&Path>, profile: Profile) -> anyhow::Result<PathBuf> {
    let mut build = cargo();
    build
        .args(["build", "--release", "--locked", "-p", "orbiters-setup", "--target", SETUP_TRIPLE])
        .args(profile.cargo_args())
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
and see docs/releasing.md. `--unsigned` builds without Authenticode signatures (SmartScreen warns about a setup \
downloaded by hand).";

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

/// Refuses programs from before this version: each built after the workspace's version last changed in Cargo.toml (a
/// stable bump rebuilds them all), and the window's program saying this version in its Windows version resource. A
/// program the running app holds open cannot be replaced, and cargo then leaves the old one in place: 0.2.1's setup
/// shipped a window labelled 0.2.0 that way. A development bump writes only `.cargo/config.toml`, so the dates say
/// nothing then: [`check_staged`] reads what each staged file says.
fn check_programs(bin: &Path, version: &str) -> anyhow::Result<()> {
    let bumped = std::fs::metadata(crate::root().join("Cargo.toml"))?.modified()?;
    for program in PROGRAMS {
        let path = bin.join(format!("{program}.exe"));
        let built = std::fs::metadata(&path).with_context(|| format!("{} is missing", path.display()))?.modified()?;
        anyhow::ensure!(built >= bumped, "{} is older than the version in Cargo.toml: it was not rebuilt (is the app running from there?)", path.display());
    }
    let window = bin.join("orbiters-desktop.exe");
    let embedded = product_version(&std::fs::read(&window)?);
    anyhow::ensure!(embedded.as_deref() == Some(version), "{} says version {:?}, not {version}", window.display(), embedded);
    Ok(())
}

/// Refuses a staged program or driver that was not built for `version`, from the bytes that are packed: each is built
/// with `env!("ORBITERS_VERSION")` (`orbiters_version::set` in each program's `main`, `DRIVER_VERSION` in the driver),
/// so the version's text is in it. Before a development bump stopped touching Cargo.toml, a worker cargo could not
/// replace passed the date check unseen (BUILD-16).
fn check_staged(stage: &Path, version: &str) -> anyhow::Result<()> {
    let files = PROGRAMS.iter().map(|p| format!("{p}.exe")).chain([DRIVER.to_string()]);
    for name in files {
        let path = stage.join(&name);
        let bytes = std::fs::read(&path).with_context(|| format!("{} cannot be read", path.display()))?;
        ensure_says_version(&path, &bytes, version)?;
    }
    Ok(())
}

/// Refuses a program that was not built for `version`, from its bytes.
pub(crate) fn ensure_says_version(path: &Path, bytes: &[u8], version: &str) -> anyhow::Result<()> {
    anyhow::ensure!(says_version(bytes, version), "{} does not say version {version}: it was not rebuilt (is a program from there running?)", path.display());
    Ok(())
}

/// Whether a version right after `before` would be the end of a longer one: its first number goes on from digits there, or it follows a dot.
fn longer_before(before: &[u8]) -> bool {
    let digits = before.iter().rev().take_while(|b| b.is_ascii_digit()).count();
    match before[before.len() - digits..].first() {
        Some(first) => *first != b'0',
        None => before.last() == Some(&b'.'),
    }
}

/// Whether `bytes` hold `version` as a whole version: not as the start of a longer one (0.3.6-dev.1 in 0.3.6-dev.16,
/// 0.3.7 in 0.3.7-dev.3) nor as the end of one (0.3.6 in 10.3.6). A letter may touch it: the compiler packs string
/// literals end to end (the capture worker's reads `0.3.6-dev.16o…`). So may a number that no version's could be the start of: `127.0.0.1:0` packed before 0.3.11-dev.10 in
/// the call's, as no number of a version starts with a zero.
fn says_version(bytes: &[u8], version: &str) -> bool {
    let needle = version.as_bytes();
    if needle.is_empty() {
        return false;
    }
    bytes.windows(needle.len()).enumerate().any(|(at, w)| {
        w == needle
            && !longer_before(&bytes[..at])
            && !bytes.get(at + needle.len()).is_some_and(|a| matches!(a, b'0'..=b'9' | b'.' | b'-' | b'+'))
    })
}

/// The ProductVersion string of a Windows version resource (UTF-16: the key, its padding to 4 bytes, the value).
fn product_version(bytes: &[u8]) -> Option<String> {
    let key: Vec<u8> = "ProductVersion\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let at = bytes.windows(key.len()).position(|w| w == key.as_slice())?;
    let start = (at + key.len() + 3) & !3;
    let units: Vec<u16> = bytes.get(start..)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).take(64).collect();
    String::from_utf16(&units).ok().filter(|v| !v.is_empty())
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
    fn the_product_version_is_read_from_a_version_resource() {
        let mut bytes = vec![0u8; 6];
        bytes.extend("ProductVersion\0".encode_utf16().flat_map(u16::to_le_bytes));
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes.extend("0.3.0\0".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(product_version(&bytes).as_deref(), Some("0.3.0"));
        assert_eq!(product_version(b"no resource here"), None);
    }

    /// BUILD-16: a worker of another version is refused whatever its date, a version is never taken for a longer or
    /// shorter one, and the text the compiler packs beside it does not hide it.
    #[test]
    fn a_staged_program_must_say_this_very_version() {
        assert!(says_version(b"\x00\x000.3.6-dev.16\x00\n", "0.3.6-dev.16"));
        assert!(says_version(b"\x000.3.6-dev.16oneshot", "0.3.6-dev.16"), "a packed literal beside it");
        assert!(says_version(b"e0.3.6-dev.16\xc1", "0.3.6-dev.16"));
        assert!(!says_version(b"\x000.3.6-dev.15\x00", "0.3.6-dev.16"), "the version before");
        assert!(!says_version(b"\x000.3.6-dev.16\x00", "0.3.6-dev.1"), "a prefix of a longer one");
        assert!(!says_version(b"\x000.3.6-dev.1\x00", "0.3.6-dev.10"));
        assert!(!says_version(b"\x000.3.7-dev.3\x00", "0.3.7"), "a development build of a stable version");
        assert!(!says_version(b"\x0010.3.6\x00", "0.3.6"));
        assert!(!says_version(b"\x00120.3.6\x00", "0.3.6"));
        assert!(!says_version(b"\x001.0.3.6\x00", "0.3.6"));
        assert!(says_version(b"127.0.0.1:00.3.11-dev.10\x00", "0.3.11-dev.10"), "an address packed before it: no version's number starts with a zero");
        assert!(!says_version(b"orbiters-core-0.3.6.1\x00", "0.3.6"));
        assert!(!says_version(b"", "0.3.6"));
        assert!(!says_version(b"0.3.6", ""));
        assert!(says_version(b"0.3.6", "0.3.6"), "at both ends of the file");

        let stage = std::env::temp_dir().join(format!("xtask-check-staged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage).unwrap();
        let write = |name: &str, version: &str| std::fs::write(stage.join(name), [b"MZ\x00".as_slice(), version.as_bytes(), b"\x00"].concat()).unwrap();
        for program in PROGRAMS {
            write(&format!("{program}.exe"), "0.3.6-dev.16");
        }
        write(DRIVER, "0.3.6-dev.16");
        check_staged(&stage, "0.3.6-dev.16").expect("all of this version");
        write("orbiters-voice.exe", "0.3.6-dev.15");
        let refused = check_staged(&stage, "0.3.6-dev.16").expect_err("a stale worker").to_string();
        assert!(refused.contains("orbiters-voice.exe"), "{refused}");
        write("orbiters-voice.exe", "0.3.6-dev.16");
        write(DRIVER, "0.3.6-dev.1");
        assert!(check_staged(&stage, "0.3.6-dev.16").is_err(), "a stale driver");
        std::fs::remove_file(stage.join(DRIVER)).unwrap();
        assert!(check_staged(&stage, "0.3.6-dev.16").is_err(), "a missing driver");
        let _ = std::fs::remove_dir_all(&stage);
    }

    #[test]
    fn version_folders_sort_as_numbers() {
        let mut names = vec!["14.9.1", "14.40.33807", "14.38.33130"];
        names.sort_by_key(|n| version_key(n));
        assert_eq!(names, ["14.9.1", "14.38.33130", "14.40.33807"]);
    }
}
