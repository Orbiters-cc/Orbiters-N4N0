//! Developer entry points share one bootstrap, before panic logging and before any Controller, App or GPU.
//! Defaults never seed from installed data. Explicit fixture trees are copied, never handed to a cache worker.

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
    // Copy only explicitly supplied scenario inputs. No directory is implicitly read from the installed app.
    if let Some(seed) = std::env::var_os("ORBITERS_HARNESS_FIXTURES") {
        seed_fixtures(Path::new(&seed), &scratch.path)?;
    }
    crate::link_smoke::sandbox(&scratch.path)?;
    if keep { eprintln!("harness data kept at {}", scratch.path.display()); }
    Ok(Some(scratch))
}

fn seed_fixtures(seed: &Path, root: &Path) -> anyhow::Result<()> {
    fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
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
                copy(&entry.path(), &to.join(entry.file_name()))?;
            }
        } else if meta.is_file() {
            if let Some(parent) = to.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::copy(from, to)?;
        }
        Ok(())
    }
    for relative in ["config/calibration.json", "cache/worlds", "cache/3d-worlds", "local/project-pictures", "documents/Sessions", "documents/Raw captures"] {
        let from = seed.join(relative);
        if from.exists() { copy(&from, &root.join(relative))?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let _controller = crate::controller::Controller::new(Default::default());
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
