//! Developer entry points share one bootstrap, before panic logging and before any Controller, App or GPU.
//! Screenshots copy their library inputs before any worker starts. All roots remain private; other harnesses use explicit fixtures.

use std::path::Path;
use crate::link_smoke::Scratch;

pub const PRIVATE: &[&str] = &[
    "--screenshot", "--gpu-bench", "--idle-frames", "--publish-cost", "--link-smoke",
    "--orbit-bench", "--orbit-paint-bench", "--n4n0-menu", "--world-view", "--world-timing",
    "--n4n0-menu-strings", "--eye-picture", "--bridge-lab",
];

/// Production subprocesses/control commands, not developer harnesses. They must retain normal installed behavior.
#[cfg(test)]
const RUNTIME: &[&str] = &[
    "--window", "--overlay", "--orbit-desktop", "--prepare-upload", "--apply-update",
    "--bridge-shot", "--bridge-guard", "--quit", "--hidden",
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
    let scratch = Scratch::new("orbiters-harness", keep)?;
    let seed = std::env::var_os("ORBITERS_HARNESS_FIXTURES");
    if mode == "--screenshot" {
        seed_screenshot(seed.as_deref().map(Path::new), &scratch.path)?;
    } else if let Some(seed) = seed {
        seed_fixtures(Path::new(&seed), &scratch.path)?;
    }
    crate::link_smoke::sandbox(&scratch.path)?;
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

/// Main used these inputs for populated capture/world scenarios and the installed VCC cover-art scenario. Copy
/// them rather than replacing them with an empty library, and never give a worker a source directory. Explicit
/// fixture packs use the same layout and take precedence over the installed inputs.
fn seed_screenshot(seed: Option<&Path>, root: &Path) -> anyhow::Result<()> {
    use orbiters_core::paths;
    let config = seed.map_or_else(paths::config_dir, |p| p.join("config"));
    let cache = seed.map_or_else(paths::cache_dir, |p| p.join("cache"));
    let local = seed.map_or_else(paths::local_data_root, |p| p.join("local"));
    let documents = seed.map_or_else(paths::documents_root, |p| p.join("documents"));
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
    for name in ["Sessions", "Raw captures"] { copies.push((documents.join(name), root.join("documents").join(name))); }
    if let Some(app) = vcc_app {
        copies.push((app.join("WebApp/Dist/template-covers"), root.join("local/VCC-app/WebApp/Dist/template-covers")));
    }
    copies.retain(|(from, _)| from.exists());
    fn bytes(path: &Path) -> std::io::Result<u64> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() { return Err(std::io::Error::other("fixture links are not copied")); }
        #[cfg(windows)] {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 { return Err(std::io::Error::other("fixture reparse points are not copied")); }
        }
        if meta.is_file() { return Ok(meta.len()); }
        let mut total = 0u64;
        for entry in std::fs::read_dir(path)? { total = total.saturating_add(bytes(&entry?.path())?); }
        Ok(total)
    }
    let required = copies.iter().try_fold(0u64, |total, (from, _)| bytes(from).map(|n| total.saturating_add(n)))?;
    let free = orbiters_projects::size::free_space(root).ok_or_else(|| anyhow::anyhow!("cannot check room for private screenshot inputs"))?;
    anyhow::ensure!(free >= required.saturating_add(12 << 30), "private screenshot inputs need {required} bytes plus 12 GiB free; temporary volume has {free}. Choose a larger TEMP folder or an explicit ORBITERS_HARNESS_FIXTURES pack.");
    eprintln!("screenshots: copying {required} bytes of input files into private roots");
    for (from, to) in copies { copy_tree(&from, &to)?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_library_inputs_are_independent_complete_copies() {
        let source = Scratch::new("orbiters-screenshot-source", false).unwrap();
        let target = Scratch::new("orbiters-screenshot-copy", false).unwrap();
        let inputs = [
            "config/visited-worlds.json", "config/avatar-states.json", "config/calibration.json",
            "config/n4n0/state.json", "cache/worlds/world.png", "cache/3d-worlds/world.json",
            "local/Avatars/avatar.json", "local/project-pictures/thumbnail.tmp",
            "local/project-pictures/undecodable.jpg", "documents/Sessions/capture/manifest.json",
            "documents/Sessions/capture/images/frame.png", "documents/Raw captures/raw/points.bin",
            "local/VCC-app/WebApp/Dist/template-covers/template-avatar-2022.png",
        ];
        let modified = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for name in inputs.iter().chain(["config/settings.json", "config/cache/old.json"].iter()) {
            let path = source.path.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, name.as_bytes()).unwrap();
            std::fs::File::options().write(true).open(path).unwrap().set_modified(modified).unwrap();
        }
        seed_screenshot(Some(&source.path), &target.path).unwrap();
        assert!(!target.path.join("config/settings.json").exists());
        assert!(!target.path.join("config/cache").exists());
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

    #[test]
    fn every_harness_bootstraps_before_controller_and_panic_logging() {
        const CHILD: &str = "ORBITERS_BOOTSTRAP_CHILD";
        if let Ok(mode) = std::env::var(CHILD) {
            let outside = std::path::PathBuf::from(std::env::var_os("ORBITERS_HARNESS_FIXTURES").unwrap());
            let guard = prepare(&[mode]).unwrap().unwrap();
            let roots = orbiters_core::paths::roots().unwrap();
            for path in [&roots.config, &roots.local_data, &roots.cache, &roots.documents] {
                assert!(path.starts_with(&guard.path));
                assert!(!path.starts_with(&outside));
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
