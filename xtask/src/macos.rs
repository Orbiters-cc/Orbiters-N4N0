//! `cargo xtask dist` on a Mac: `target/dist/<version>/` with the application (`Orbiters - N4N0.app`), the disk image
//! people download, the zip installed apps update with, and the signed `macos/<channel>.json` that announces it.
//!
//! 1. The programs are built in release for Apple silicon, for the oldest macOS the app supports.
//! 2. The application is put together: the programs and the speech libraries in `Contents/MacOS`, what is not code in
//!    `Contents/Resources` (the icon, the face detector, the licences, the install record), each library checked: a program
//!    may only need the system's or what ships beside it.
//! 3. It is signed from the inside out with the best identity this computer has: Developer ID, else Apple Development,
//!    else ad hoc. Developer ID also goes to Apple's notary when a profile for it is set up (`macos_notary_profile`).
//! 4. It is zipped and put on a disk image with a link to Applications.
//! 5. The release file names the zip and is signed with the release key, when this computer has it.

use crate::{output, run, Config};
use anyhow::Context as _;
use orbiters_update::{mac_archive_name, mac_disk_image_name, Artifact, InstallRecord, Release, SignedRelease, MAC_APP, MAC_BUNDLE_ID, PRODUCT};
use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAMS: [&str; 5] = ["orbiters-desktop", "orbiters-capture", "orbiters-reconstruct", "orbiters-voice", "orbiters-call"];
/// The speech libraries beside the programs in `target/release` (sherpa-onnx's prebuilt ones, which its build copies
/// there); the reconstruction's face detector loads the same ONNX Runtime.
const LIBRARIES: [&str; 2] = ["libsherpa-onnx-c-api.dylib", "libonnxruntime.dylib"];
const FACE_MODEL: &str = "crates/orbiters-reconstruct/assets/faces-v1-int8.onnx";
const ICON: &str = "crates/orbiters-desktop/assets/AppIcon.icns";
const DOCUMENTS: [&str; 3] = ["LICENSE.md", "THIRD_PARTY_LICENSES.md", "THIRD_PARTY_NOTICES.md"];
/// The oldest macOS the app runs on. Rust's target asks for 11; Metal 3 features and the C libraries built here for this.
const MINIMUM_MACOS: &str = "13.0";

const ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.device.audio-input</key>
    <true/>
    <key>com.apple.security.automation.apple-events</key>
    <true/>
</dict>
</plist>
"#;

pub struct Options {
    pub channel: String,
    pub notes: String,
}

/// How the application gets signed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    DeveloperId(String),
    AppleDevelopment(String),
    AdHoc,
}

impl Identity {
    /// The identity named in the settings, else the best of `listed` (what `security find-identity` prints).
    fn choose(configured: Option<&str>, listed: &str) -> Self {
        if let Some(name) = configured {
            return match name {
                "-" => Identity::AdHoc,
                _ if name.contains("Developer ID") => Identity::DeveloperId(name.into()),
                _ => Identity::AppleDevelopment(name.into()),
            };
        }
        let names: Vec<&str> = listed.lines().filter_map(|line| line.split('"').nth(1)).collect();
        let find = |what: &str| names.iter().find(|n| n.starts_with(what)).map(|n| n.to_string());
        find("Developer ID Application").map(Identity::DeveloperId).or_else(|| find("Apple Development").map(Identity::AppleDevelopment)).unwrap_or(Identity::AdHoc)
    }

    fn name(&self) -> &str {
        match self {
            Identity::DeveloperId(n) | Identity::AppleDevelopment(n) => n,
            Identity::AdHoc => "-",
        }
    }

    /// A Developer ID signature opens anywhere once notarized; the others only where the person lets it (Privacy & Security,
    /// Open Anyway).
    fn describe(&self) -> &'static str {
        match self {
            Identity::DeveloperId(_) => "Developer ID",
            Identity::AppleDevelopment(_) => "Apple Development (not notarizable: Gatekeeper blocks it until allowed in Privacy & Security)",
            Identity::AdHoc => "ad hoc (Gatekeeper blocks it until allowed in Privacy & Security; permissions are asked again after each update)",
        }
    }
}

/// The application's Info.plist. `CFBundleShortVersionString` takes numbers only: a development version shows as its
/// release's numbers, its build number in `CFBundleVersion` keeps the versions in order.
fn info_plist(version: &str) -> String {
    let (short, build) = bundle_versions(version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Orbiters - N4N0</string>
    <key>CFBundleDisplayName</key>
    <string>Orbiters - N4N0</string>
    <key>CFBundleIdentifier</key>
    <string>{MAC_BUNDLE_ID}</string>
    <key>CFBundleExecutable</key>
    <string>orbiters-desktop</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>{short}</string>
    <key>CFBundleVersion</key>
    <string>{build}</string>
    <key>LSMinimumSystemVersion</key>
    <string>{MINIMUM_MACOS}</string>
    <key>LSArchitecturePriority</key>
    <array>
        <string>arm64</string>
    </array>
    <key>LSRequiresNativeExecution</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2026 Enzo DUTRA / blackorbit</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>N4N0 hears you through the microphone while you talk to it (transcribed on this Mac), and calls use it.</string>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Orbiters - N4N0 looks for VRChat on your headset, phone or PC on this network and talks with it over OSC.</string>
    <key>NSBonjourServices</key>
    <array>
        <string>_oscjson._tcp</string>
        <string>_osc._udp</string>
    </array>
    <key>NSAppleEventsUsageDescription</key>
    <string>N4N0's menu shows what Music or Spotify plays, and controls it.</string>
    <key>NSDocumentsFolderUsageDescription</key>
    <string>Your captures are kept in Documents/Orbiters, and Unity projects may be there.</string>
    <key>NSDesktopFolderUsageDescription</key>
    <string>Unity projects and pictures you open may be on the Desktop.</string>
    <key>NSDownloadsFolderUsageDescription</key>
    <string>Unity projects and packages you open may be in Downloads.</string>
    <key>NSRemovableVolumesUsageDescription</key>
    <string>Unity projects and captures may be on an external disk.</string>
    <key>NSNetworkVolumesUsageDescription</key>
    <string>Unity projects and captures may be on a network disk.</string>
</dict>
</plist>
"#
    )
}

/// The numbers macOS wants: `0.3.4` and `0.3.4.32` for `0.3.4-dev.32`, `0.3.4` twice for `0.3.4`.
fn bundle_versions(version: &str) -> (String, String) {
    let (short, pre) = version.split_once('-').unwrap_or((version, ""));
    let build = pre.rsplit('.').next().filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (short.to_string(), build.map_or_else(|| short.to_string(), |n| format!("{short}.{n}")))
}

fn copy(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::copy(from, to).with_context(|| format!("{} -> {}", from.display(), to.display()))?;
    Ok(())
}

/// A library or program's own name and what it links to, as `otool -L` prints it.
fn linked(file: &Path) -> anyhow::Result<Vec<String>> {
    let text = output(Command::new("otool").arg("-L").arg(file))?;
    Ok(text.lines().skip(1).filter_map(|l| l.split_whitespace().next()).map(String::from).collect())
}

/// What `name` in a library list needs from the bundle: `None` for the system's own.
fn shipped_name(dependency: &str) -> Option<&str> {
    if dependency.starts_with("/usr/lib/") || dependency.starts_with("/System/Library/") {
        return None;
    }
    Some(dependency.rsplit('/').next().unwrap_or(dependency))
}

/// Everything each file in `MacOS` links to is the system's or sits beside it, found through `@rpath`, `@loader_path` or
/// `@executable_path`, and never an absolute path of this computer's.
fn check_libraries(macos: &Path) -> anyhow::Result<()> {
    let names: Vec<String> = std::fs::read_dir(macos)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for entry in std::fs::read_dir(macos)?.flatten() {
        for dependency in linked(&entry.path())? {
            let Some(name) = shipped_name(&dependency) else { continue };
            let relative = dependency.starts_with('@');
            anyhow::ensure!(
                relative && (names.iter().any(|n| n == name) || Some(name) == entry.file_name().to_str()),
                "{} needs {dependency}, which is not the system's and does not ship beside it",
                entry.path().display()
            );
        }
    }
    Ok(())
}

/// Signs one piece of code.
fn sign(identity: &Identity, entitlements: &Path, file: &Path, is_main: bool) -> anyhow::Result<()> {
    let mut cmd = Command::new("codesign");
    cmd.args(["--force", "--sign", identity.name()]);
    if *identity != Identity::AdHoc {
        // The hardened runtime, which notarization asks for; library validation holds beside it because the libraries
        // carry the same team's signature.
        cmd.args(["--options", "runtime"]);
        cmd.arg("--entitlements").arg(entitlements);
    }
    if matches!(identity, Identity::DeveloperId(_)) {
        cmd.arg("--timestamp");
    }
    if is_main {
        cmd.args(["--identifier", MAC_BUNDLE_ID]);
    }
    run(cmd.arg(file))
}

pub fn dist(config: &Config, options: &Options) -> anyhow::Result<PathBuf> {
    let version = crate::workspace_version()?;
    let target = crate::target_dir();
    let dist = target.join("dist").join(&version);
    if dist.exists() {
        std::fs::remove_dir_all(&dist).with_context(|| format!("{} cannot be emptied", dist.display()))?;
    }
    std::fs::create_dir_all(&dist)?;

    println!("\n== Programs {version}");
    let mut build = crate::cargo();
    build.args(["build", "--release", "--locked"]).env("CARGO_TARGET_DIR", &target).env("MACOSX_DEPLOYMENT_TARGET", MINIMUM_MACOS);
    for program in PROGRAMS {
        build.args(["-p", program]);
    }
    run(&mut build)?;
    let bin = target.join("release");
    let bumped = std::fs::metadata(crate::root().join("Cargo.toml"))?.modified()?;
    for program in PROGRAMS {
        let path = bin.join(program);
        let built = std::fs::metadata(&path).with_context(|| format!("{} is missing", path.display()))?.modified()?;
        anyhow::ensure!(built >= bumped, "{} is older than the version in Cargo.toml: it was not rebuilt", path.display());
    }

    println!("\n== Application");
    let app = dist.join(MAC_APP);
    let contents = app.join("Contents");
    let (macos, resources) = (contents.join("MacOS"), contents.join("Resources"));
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;
    std::fs::write(contents.join("Info.plist"), info_plist(&version))?;
    std::fs::write(contents.join("PkgInfo"), "APPL????")?;
    for program in PROGRAMS {
        copy(&bin.join(program), &macos.join(program))?;
    }
    for library in LIBRARIES {
        copy(&resolve_library(&bin, library)?, &macos.join(library))?;
    }
    copy(&crate::root().join(ICON), &resources.join("AppIcon.icns"))?;
    let face_model = crate::root().join(FACE_MODEL);
    copy(&face_model, &resources.join(face_model.file_name().expect("a file")))?;
    for document in DOCUMENTS {
        copy(&crate::root().join(document), &resources.join(document))?;
    }
    InstallRecord {
        product: PRODUCT.into(),
        version: version.clone(),
        channel: options.channel.clone(),
        files: Vec::new(),
        desktop_shortcut: false,
        start_with_windows: false,
        start_with_steamvr: false,
        installed: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?,
    }
    .write(&resources)?;
    check_libraries(&macos)?;
    for entry in std::fs::read_dir(&macos)?.flatten() {
        let file = entry.path();
        anyhow::ensure!(output(Command::new("lipo").arg("-archs").arg(&file))?.split_whitespace().any(|a| a == "arm64"), "{} has no arm64 code", file.display());
    }

    println!("\n== Signing");
    let listed = output(Command::new("security").args(["find-identity", "-v", "-p", "codesigning"])).unwrap_or_default();
    let identity = Identity::choose(config.macos_identity.as_deref(), &listed);
    println!("With {}: {}", identity.name(), identity.describe());
    let entitlements = dist.join("entitlements.plist");
    std::fs::write(&entitlements, ENTITLEMENTS)?;
    // From the inside out: libraries, the workers, then the application around them.
    for library in LIBRARIES {
        sign(&identity, &entitlements, &macos.join(library), false)?;
    }
    for program in PROGRAMS.iter().filter(|p| **p != "orbiters-desktop") {
        sign(&identity, &entitlements, &macos.join(program), false)?;
    }
    sign(&identity, &entitlements, &app, true)?;
    run(Command::new("codesign").args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(&app))?;

    let archive = dist.join(mac_archive_name(&version));
    let zip = |to: &Path| run(Command::new("ditto").args(["-c", "-k", "--keepParent", "--sequesterRsrc"]).arg(&app).arg(to));

    if let (Identity::DeveloperId(_), Some(profile)) = (&identity, &config.macos_notary_profile) {
        println!("\n== Notarizing");
        zip(&archive)?;
        run(Command::new("xcrun").args(["notarytool", "submit", "--wait", "--keychain-profile", profile]).arg(&archive))?;
        run(Command::new("xcrun").args(["stapler", "staple"]).arg(&app))?;
        std::fs::remove_file(&archive)?;
    }

    println!("\n== Archive and disk image");
    zip(&archive)?;
    let image = dist.join(mac_disk_image_name(&version));
    let staging = dist.join("image");
    std::fs::create_dir_all(&staging)?;
    run(Command::new("ditto").arg(&app).arg(staging.join(MAC_APP)))?;
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))?;
    run(Command::new("hdiutil")
        .args(["create", "-quiet", "-volname", "Orbiters - N4N0", "-fs", "HFS+", "-format", "UDZO", "-ov", "-srcfolder"])
        .arg(&staging)
        .arg(&image))?;
    std::fs::remove_dir_all(&staging)?;
    if identity != Identity::AdHoc {
        sign_image(&identity, &image)?;
    }
    if let (Identity::DeveloperId(_), Some(profile)) = (&identity, &config.macos_notary_profile) {
        run(Command::new("xcrun").args(["notarytool", "submit", "--wait", "--keychain-profile", profile]).arg(&image))?;
        run(Command::new("xcrun").args(["stapler", "staple"]).arg(&image))?;
    }

    let installer = Artifact {
        url: format!("{}{version}/{}", orbiters_update::PLATFORM_FOLDER, mac_archive_name(&version)),
        sha256: orbiters_update::sha256_file(&archive)?,
        size: std::fs::metadata(&archive)?.len(),
    };
    match crate::private_key(config) {
        Ok(key) => {
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
        // The release key stays off this computer until it is copied here (docs/releasing.md): the release goes out by
        // hand, and installed apps hear of it once it is signed and published from a computer that has the key.
        Err(e) => println!("\nNo release file: installed apps will not hear of this version. {e:#}"),
    }
    std::fs::remove_file(&entitlements)?;

    println!(
        "\n{}\n  {} MB\n{}\n  {} MB, SHA-256 {}\n{}",
        image.display(),
        std::fs::metadata(&image)?.len() >> 20,
        archive.display(),
        installer.size >> 20,
        installer.sha256,
        identity.describe()
    );
    Ok(dist)
}

fn sign_image(identity: &Identity, image: &Path) -> anyhow::Result<()> {
    run(Command::new("codesign").args(["--force", "--sign", identity.name()]).arg(image))
}

/// The library file under `name`, which sherpa-onnx's build may leave as a versioned file with links to it.
fn resolve_library(bin: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let path = bin.join(name);
    std::fs::canonicalize(&path).with_context(|| format!("{} is missing: is orbiters-voice built?", path.display()))
}

/// The files of a Mac release, for `github`.
pub fn release_assets(version: &str) -> Vec<PathBuf> {
    let dist = crate::target_dir().join("dist").join(version);
    vec![dist.join(mac_disk_image_name(version)), dist.join(mac_archive_name(version))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_identity_this_computer_has_signs() {
        let both = r#"  1) AAA "Apple Development: someone (TEAM)"
  2) BBB "Developer ID Application: someone (TEAM)"
     2 valid identities found"#;
        assert_eq!(Identity::choose(None, both), Identity::DeveloperId("Developer ID Application: someone (TEAM)".into()));
        let one = r#"  1) AAA "Apple Development: someone (TEAM)""#;
        assert_eq!(Identity::choose(None, one), Identity::AppleDevelopment("Apple Development: someone (TEAM)".into()));
        assert_eq!(Identity::choose(None, "     0 valid identities found"), Identity::AdHoc);
        assert_eq!(Identity::choose(Some("-"), both), Identity::AdHoc);
    }

    #[test]
    fn versions_become_the_numbers_macos_wants() {
        assert_eq!(bundle_versions("0.3.4-dev.32"), ("0.3.4".to_string(), "0.3.4.32".to_string()));
        assert_eq!(bundle_versions("1.2.3"), ("1.2.3".to_string(), "1.2.3".to_string()));
    }

    #[test]
    fn the_info_plist_asks_for_what_the_app_uses() {
        let plist = info_plist("0.3.4-dev.32");
        for needed in ["NSMicrophoneUsageDescription", "NSLocalNetworkUsageDescription", "_oscjson._tcp", "NSAppleEventsUsageDescription", "<string>cc.orbiters.n4n0</string>", "<string>0.3.4.32</string>", "LSUIElement"] {
            assert!(plist.contains(needed), "{needed}");
        }
    }

    #[test]
    fn only_the_system_s_libraries_are_not_shipped() {
        assert_eq!(shipped_name("/usr/lib/libSystem.B.dylib"), None);
        assert_eq!(shipped_name("/System/Library/Frameworks/Metal.framework/Versions/A/Metal"), None);
        assert_eq!(shipped_name("@rpath/libonnxruntime.dylib"), Some("libonnxruntime.dylib"));
        assert_eq!(shipped_name("/opt/homebrew/lib/libopus.dylib"), Some("libopus.dylib"));
    }
}
