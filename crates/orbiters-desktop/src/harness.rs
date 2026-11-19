//! Developer entry points share one bootstrap, before panic logging and before any Controller, App or GPU.
//! Mutable startup data stays private. Screenshots alone read large capture/picture inputs in place.

use std::path::Path;
use crate::link_smoke::Scratch;

pub const PRIVATE: &[&str] = &[
    "--screenshot", "--gpu-bench", "--idle-frames", "--publish-cost", "--link-smoke",
    "--orbit-bench", "--orbit-paint-bench", "--n4n0-menu", "--world-view", "--world-timing",
    "--n4n0-menu-strings", "--eye-picture", "--bridge-lab",
];

/// Screenshot inputs are captured before private roots replace the platform paths. Keep documents private for
/// everything else; only the screenshot Settings receive these two large read-in-place libraries.
static SCREENSHOT_LIBRARY: std::sync::OnceLock<(std::path::PathBuf, std::path::PathBuf)> = std::sync::OnceLock::new();

pub fn screenshot_library() -> Option<&'static (std::path::PathBuf, std::path::PathBuf)> {
    SCREENSHOT_LIBRARY.get()
}

/// `--screenshot <out> --real-data`: the scenes that show the person's own captures, pictures and VRChat's folders
/// read them in place (and copy the small settings and caches into the private folders). Off by default: a run then
/// reads a fixture pack (`ORBITERS_HARNESS_FIXTURES`) or nothing of the person's, and writes only under `<out>`.
pub const REAL_DATA: &str = "--real-data";

/// Production subprocesses/control commands, not developer harnesses. They must retain normal installed behavior.
#[cfg(test)]
const RUNTIME: &[&str] = &[
    "--window", "--overlay", "--orbit-desktop", "--prepare-upload", "--apply-update",
    "--bridge-shot", "--bridge-guard", "--quit", "--hidden",
    // Repairs the pictures of the person's own captures (controller::picture_repair), on purpose in place.
    "--repair-capture-pictures",
];

/// Runs once in main. Holding the returned guard through dispatch also cleans up early errors and panics.
pub fn prepare(args: &[String]) -> anyhow::Result<Option<Scratch>> {
    let mode = args.first().map_or("", String::as_str);
    if let Some(at) = args.iter().position(|a| a == "--sandbox") {
        anyhow::ensure!(["--window", "--overlay", "--orbit-desktop"].contains(&mode), "--sandbox is only for harness client processes");
        let root = args.get(at + 1).ok_or_else(|| anyhow::anyhow!("--sandbox needs a directory"))?;
        crate::link_smoke::sandbox(Path::new(root))?;
        return Ok(None);
    }
    if !PRIVATE.contains(&mode) { return Ok(None); }
    let keep = args.iter().any(|a| a == "--keep") || (mode == "--screenshot" && std::env::var_os("ORBITERS_SCREENSHOT_KEEP_SANDBOX").is_some());
    // Screenshots: their private folders under the output folder, so that the run writes nowhere else (night6).
    let real = mode == "--screenshot" && args.iter().any(|a| a == REAL_DATA);
    let scratch = match (mode == "--screenshot").then(|| args.get(1)).flatten() {
        Some(out) => {
            std::fs::create_dir_all(out)?;
            Scratch::new_in(Path::new(out), ".orbiters-harness", keep)?
        }
        None => Scratch::new("orbiters-harness", keep)?,
    };
    let seed = std::env::var_os("ORBITERS_HARNESS_FIXTURES");
    if mode == "--screenshot" {
        // The capture library: a fixture pack's, the person's own only with `--real-data` (read in place, never
        // written), else the private folders' (empty).
        let library = match (seed.as_ref(), real) {
            (Some(p), _) => (Path::new(p).join("documents/Sessions"), Path::new(p).join("documents/Raw captures")),
            (None, true) => (orbiters_core::paths::default_sessions_dir(), orbiters_core::paths::default_raw_captures_dir()),
            (None, false) => (scratch.path.join("documents/Sessions"), scratch.path.join("documents/Raw captures")),
        };
        SCREENSHOT_LIBRARY.set(library).map_err(|_| anyhow::anyhow!("screenshot inputs were given already"))?;
        // The person's settings folder, caches and Unity data are copied in only with `--real-data`.
        if seed.is_some() || real {
            seed_screenshot(seed.as_deref().map(Path::new), &scratch.path)?;
        }
        orbiters_core::bundle::disable_thumbnail_cache_writes();
    } else if let Some(seed) = seed {
        seed_fixtures(Path::new(&seed), &scratch.path)?;
    }
    crate::link_smoke::sandbox(&scratch.path)?;
    if real {
        // VRChat's own pictures and logs, read in place.
        orbiters_core::paths::allow_real_data();
    }
    if keep { eprintln!("harness data kept at {}", scratch.path.display()); }
    Ok(Some(scratch))
}

fn seed_fixtures(seed: &Path, root: &Path) -> anyhow::Result<()> {
    for relative in ["config/calibration.json", "cache/worlds", "cache/3d-worlds", "local/project-pictures", "documents/Sessions", "documents/Raw captures"] {
        let from = seed.join(relative);
        if from.exists() { copy_tree(&from, &root.join(relative))?; }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() { return Err(std::io::Error::other("fixture links are not copied")); }
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return Err(std::io::Error::other("fixture reparse points are not copied")); }
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else if meta.is_file() {
        if let Some(parent) = to.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::copy(from, to)?;
        // Session order, age labels and cache freshness are part of the fixture, not the time this copy was made.
        std::fs::File::options().write(true).open(to)?.set_modified(meta.modified()?)?;
    }
    Ok(())
}

/// Copy only the small startup inputs that may be cleaned or rewritten. Sessions and raw captures stay in place;
/// explicit fixture packs replace their source paths as well as these copied inputs.
fn seed_screenshot(seed: Option<&Path>, root: &Path) -> anyhow::Result<()> {
    use orbiters_core::paths;
    let config = seed.map_or_else(paths::config_dir, |p| p.join("config"));
    let cache = seed.map_or_else(paths::cache_dir, |p| p.join("cache"));
    let local = seed.map_or_else(paths::local_data_root, |p| p.join("local"));
    let vcc_app = seed.map(|p| p.join("local/VCC-app")).or_else(orbiters_projects::vcc_app_dir);
    let mut copies = Vec::new();
    if config.is_dir() {
        for entry in std::fs::read_dir(&config)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().eq_ignore_ascii_case("settings.json") || name.to_string_lossy().eq_ignore_ascii_case("cache") { continue; }
            copies.push((entry.path(), root.join("config").join(name)));
        }
    }
    for name in ["worlds", "3d-worlds"] { copies.push((cache.join(name), root.join("cache").join(name))); }
    for name in ["Avatars", "project-pictures"] { copies.push((local.join(name), root.join("local").join(name))); }
    if let Some(app) = vcc_app {
        copies.push((app.join("WebApp/Dist/template-covers"), root.join("local/VCC-app/WebApp/Dist/template-covers")));
    }
    // A fixture pack's VRChat pictures (`documents/Pictures/VRChat`): where the private folders have VRChat's.
    if let Some(seed) = seed {
        copies.push((seed.join("documents/Pictures"), root.join("documents/Pictures")));
    }
    copies.retain(|(from, _)| from.exists());
    for (from, to) in copies { copy_tree(&from, &to)?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_copies_small_mutable_inputs_but_not_capture_payloads() {
        let source = Scratch::new("orbiters-screenshot-source", false).unwrap();
        let target = Scratch::new("orbiters-screenshot-copy", false).unwrap();
        let inputs = [
            "config/visited-worlds.json", "config/avatar-states.json", "config/calibration.json",
            "config/n4n0/state.json", "cache/worlds/world.png", "cache/3d-worlds/world.json",
            "local/Avatars/avatar.json", "local/project-pictures/thumbnail.tmp",
            "local/project-pictures/undecodable.jpg",
            "local/VCC-app/WebApp/Dist/template-covers/template-avatar-2022.png",
        ];
        let modified = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for name in inputs.iter().chain(["config/settings.json", "config/cache/old.json", "documents/Sessions/capture/payload", "documents/Raw captures/raw/payload"].iter()) {
            let path = source.path.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, name.as_bytes()).unwrap();
            std::fs::File::options().write(true).open(path).unwrap().set_modified(modified).unwrap();
        }
        seed_screenshot(Some(&source.path), &target.path).unwrap();
        assert!(!target.path.join("config/settings.json").exists());
        assert!(!target.path.join("config/cache").exists());
        assert!(!target.path.join("documents").exists(), "capture payloads are not copied or walked");
        for name in inputs {
            let copied = target.path.join(name);
            assert_eq!(std::fs::read(&copied).unwrap(), name.as_bytes());
            assert_eq!(std::fs::metadata(&copied).unwrap().modified().unwrap(), modified);
            // Exercise the operations preload/keep_aside can perform, on the copies only.
            std::fs::write(&copied, b"changed private data").unwrap();
            std::fs::remove_file(&copied).unwrap();
            let original = source.path.join(name);
            assert_eq!(std::fs::read(&original).unwrap(), name.as_bytes());
            assert_eq!(std::fs::metadata(original).unwrap().modified().unwrap(), modified);
        }
    }

    /// An added dispatch switch must be classified here. Source ordering plus subprocess checks ensures bootstrap
    /// precedes both panic-hook capture and dispatch, including platform-specific branches on this test platform.
    #[test]
    fn every_dispatch_switch_has_an_isolation_policy() {
        const READS_IN_PLACE: &[(&str, &str)] = &[
            ("--screenshot", "with --real-data only: reads the capture library (Sessions, Raw captures) and VRChat's pictures in place"),
        ];
        assert_eq!(READS_IN_PLACE.len(), 1);
        assert!(READS_IN_PLACE.iter().all(|(mode, why)| PRIVATE.contains(mode) && !why.is_empty()));
        let main = include_str!("main.rs");
        let dispatch = main.split("match args.first()").nth(1).unwrap();
        for part in dispatch.split("Some(\"").skip(1) {
            let mode = part.split('"').next().unwrap();
            assert!(PRIVATE.contains(&mode) || RUNTIME.contains(&mode), "unclassified dispatch: {mode}");
        }
        assert!(main.find("harness::prepare(&args)").unwrap() < main.find("panic_log::install()").unwrap());
        assert!(main.find("panic_log::install()").unwrap() < main.find("match args.first()").unwrap());
        assert!(dispatch.contains("Some(orbiters_booster::system::RESTORE_ARGUMENT)"), "the production RunOnce restore remains explicit");
        for source in [include_str!("screenshot.rs"), include_str!("idle_frames.rs"), include_str!("controller/publish_cost.rs"), include_str!("overlay/orbit/paint_bench.rs")] {
            assert!(!source.contains("paths::set_roots("), "harnesses consume the startup roots rather than replacing them");
        }
    }

    /// Night6: every per-user folder a desktop test can reach is the test process's own (`paths::test_roots`, under
    /// the system's temporary folder): settings, logs (perf's), caches (the editor's, the drag's), captures, Unity's
    /// project data, VRChat's pictures and logs. A path that resolves to the person's own folders fails this test.
    #[test]
    fn a_test_never_resolves_the_persons_folders() {
        let root = orbiters_core::paths::test_roots().config.parent().unwrap().to_path_buf();
        let real: Vec<std::path::PathBuf> = [
            dirs::config_dir().map(|d| d.join("Orbiters")),
            dirs::data_local_dir().map(|d| d.join("Orbiters")),
            dirs::document_dir().map(|d| d.join("Orbiters")),
            dirs::picture_dir(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let settings = orbiters_core::settings::Settings::load();
        let mut ours = vec![
            orbiters_core::paths::settings_file(),
            orbiters_core::paths::logs_dir(),
            orbiters_core::paths::cache_dir(),
            crate::pages::cache_root(),
            settings.sessions_dir,
            settings.raw_captures_dir,
            orbiters_core::settings::Settings::default().sessions_dir,
            orbiters_projects::data_dir(),
            orbiters_projects::vcc_dir(),
        ];
        ours.extend(orbiters_projects::hub_data_dir());
        ours.extend(orbiters_core::paths::vrchat_log_dir());
        ours.extend(orbiters_core::visited::screenshot_dirs());
        for p in ours {
            assert!(p.starts_with(&root), "not the test's own: {}", p.display());
            assert!(!real.iter().any(|r| p.starts_with(r)), "the person's own: {}", p.display());
        }
    }

    #[test]
    fn every_harness_bootstraps_before_controller_and_panic_logging() {
        const CHILD: &str = "ORBITERS_BOOTSTRAP_CHILD";
        if let Ok(mode) = std::env::var(CHILD) {
            let outside = std::path::PathBuf::from(std::env::var_os("ORBITERS_HARNESS_FIXTURES").unwrap());
            let guard = prepare(&[mode.clone()]).unwrap().unwrap();
            let roots = orbiters_core::paths::roots().unwrap();
            for path in [&roots.config, &roots.local_data, &roots.cache, &roots.documents] {
                assert!(path.starts_with(&guard.path));
                assert!(!path.starts_with(&outside));
            }
            if mode == "--screenshot" {
                assert_eq!(screenshot_library().unwrap(), &(outside.join("documents/Sessions"), outside.join("documents/Raw captures")));
                assert!(!guard.path.join("documents/Sessions").exists());
                assert!(!guard.path.join("documents/Raw captures").exists());
            } else {
                assert!(screenshot_library().is_none());
            }
            assert_eq!(orbiters_projects::data_dir(), roots.local_data);
            assert_eq!(orbiters_projects::vcc_dir(), roots.local_data.join("VCC"));
            assert_eq!(orbiters_projects::hub_data_dir(), Some(roots.local_data.join("UnityHub")));
            orbiters_core::panic_log::install();
            let _controller = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
            // This writes a real panic note through the hook installed after bootstrap, then continues the test.
            assert!(std::panic::catch_unwind(|| panic!("contained harness panic")).is_err());
            assert!(std::fs::read_dir(orbiters_core::paths::logs_dir()).unwrap().next().is_some());
            // Controller skips calibration under cfg(test); a non-test publish-cost run verifies keep_aside too.
            assert!(roots.config.join("calibration.json").is_file());
            return;
        }
        let outside = Scratch::new("orbiters-harness-sentinels", false).unwrap();
        let calibration = outside.path.join("config/calibration.json");
        let panic_log = outside.path.join("local/logs/installed.log");
        std::fs::create_dir_all(calibration.parent().unwrap()).unwrap();
        std::fs::create_dir_all(panic_log.parent().unwrap()).unwrap();
        std::fs::write(&calibration, b"malformed calibration sentinel").unwrap();
        std::fs::write(&panic_log, vec![b'x'; (1 << 20) + 1]).unwrap();
        let before: Vec<_> = [&calibration, &panic_log].into_iter().map(|p| (std::fs::read(p).unwrap(), std::fs::metadata(p).unwrap().modified().unwrap())).collect();
        for mode in PRIVATE {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "harness::tests::every_harness_bootstraps_before_controller_and_panic_logging"])
                .env(CHILD, mode).env("ORBITERS_HARNESS_FIXTURES", &outside.path)
                .env_remove("ORBITERS_SCREENSHOT_KEEP_SANDBOX").output().unwrap();
            assert!(result.status.success(), "{mode}: {} {}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
            for (p, (bytes, modified)) in [&calibration, &panic_log].into_iter().zip(&before) {
                assert_eq!(std::fs::read(p).unwrap(), *bytes, "{mode}");
                assert_eq!(std::fs::metadata(p).unwrap().modified().unwrap(), *modified, "{mode}");
            }
        }
    }

    #[test]
    fn ordinary_launches_do_not_set_private_roots() {
        for mode in RUNTIME.iter().copied().chain(["", orbiters_booster::system::RESTORE_ARGUMENT]) {
            assert!(prepare(&[mode.into()]).unwrap().is_none());
        }
        assert!(prepare(&["--hidden".into(), "--sandbox".into(), "unused".into()]).is_err());
    }
}
