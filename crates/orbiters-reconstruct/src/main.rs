//! Reconstruction worker of Orbiters desktop:
//!
//! ```text
//! orbiters-reconstruct <session folder> [--width N] [--iterations N] [--budget N] [--robust T] [--gpu integrated|fastest]
//!     [--regularise-unseen] [--adam-restart] [--filter-3d S] [--cleanup]
//! ```
//!
//! Prints one [`Event`] per line on stdout for the app and writes the session's `reconstruction/` folder: the splats
//! (PLY), the report, a preview and a render of every held-out keyframe. The folder is replaced only once everything
//! is written, and never left half deleted ([`replace`]).

use anyhow::{bail, Context as _};
use orbiters_contracts::qoi;
use orbiters_core::reconstruction::{self as rec, Event, Report, Stage};
use orbiters_reconstruct::{gpu, init, ply, prepare, sky, timing, train};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Where the previous result waits while the new one moves into place (`reconstruction.old`).
const BACKUP_DIR: &str = "reconstruction.old";

fn emit(e: &Event) {
    let mut out = std::io::stdout().lock();
    // The app reads these lines: once its end of the pipe is gone, so is the reason to keep the GPU busy.
    if writeln!(out, "{}", e.to_line()).and_then(|_| out.flush()).is_err() {
        std::process::exit(3);
    }
}

fn stage(stage: Stage, progress: f32) {
    emit(&Event::Stage { stage, progress });
}

struct Args {
    session: PathBuf,
    width: u32,
    settings: train::Settings,
    device: gpu::DeviceOptions,
}

fn args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut session = None;
    let mut width = 1920;
    let mut settings = train::Settings::default();
    let mut device = gpu::DeviceOptions::default();
    while let Some(a) = it.next() {
        let mut number = |name: &str| -> anyhow::Result<u64> { it.next().with_context(|| format!("{name} needs a value"))?.parse().with_context(|| format!("{name} takes a number")) };
        match a.as_str() {
            "--width" => width = number("--width")? as u32,
            "--iterations" => settings.iterations = number("--iterations")? as u32,
            "--budget" => settings.budget = number("--budget")? as usize,
            "--robust" => settings.robust = it.next().context("--robust needs a value")?.parse().context("--robust takes a number (0–1)")?,
            // Lab A/Bs: the regularisers on every Gaussian every step (orbiters_reconstruct::train::Settings).
            "--regularise-unseen" => settings.regularise_unseen = true,
            // Adam's bias correction per Gaussian from its restart, rather than from the run's first step.
            "--adam-restart" => settings.adam_restart = true,
            // Mip-Splatting's 3D filter's scale (0, the default: none; its own is orbiters_reconstruct::cpu::FILTER_3D).
            "--filter-3d" => settings.filter_3d = it.next().context("--filter-3d needs a value")?.parse().context("--filter-3d takes a number")?,
            // What the exact depth says is not there leaves the result (orbiters_reconstruct::cleanup).
            "--cleanup" => settings.cleanup = true,
            // The sky map's width (orbiters_reconstruct::sky::SkyMap; 0: the shell of discs).
            "--sky-map" => settings.sky_map = number("--sky-map")? as u32,
            "--gpu" => {
                device.low_power = match it.next().as_deref() {
                    Some("integrated") => true,
                    Some("fastest") => false,
                    _ => bail!("--gpu takes integrated or fastest"),
                }
            }
            _ if a.starts_with("--") => bail!("unknown option {a}"),
            _ => session = Some(PathBuf::from(a)),
        }
    }
    Ok(Args { session: session.context("usage: orbiters-reconstruct <session folder> [--width N] [--iterations N] [--budget N] [--robust T] [--gpu integrated|fastest]")?, width, settings, device })
}

fn rgb8(img: &[[f32; 4]]) -> Vec<u8> {
    img.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
}

fn run(args: Args) -> anyhow::Result<Report> {
    let run_start = std::time::Instant::now();
    let dir = &args.session;
    // Above normal while VRChat is closed, out of its way while it runs (orbiters_reconstruct::priority).
    orbiters_reconstruct::priority::govern();
    // A swap an earlier run stopped in the middle of: its previous result is back in place first.
    recover(dir);
    stage(Stage::Loading, 0.0);
    // Lab (S4_OVERLAP): the device and its kernels come up while the session loads.
    let device_options = gpu::DeviceOptions { low_priority: true, ..args.device };
    let early_device = std::env::var_os("S4_OVERLAP").is_some().then(|| std::thread::spawn(move || timing::section("2 device", None, || gpu::Gpu::with_options(device_options))));
    // Within the memory budget; depth that says nothing becomes unknown, the world's chromatic aberration leaves the
    // keyframes.
    let prepared = timing::section("1 prepare", None, || prepare::prepare(dir, &prepare::Options { width: args.width, ..Default::default() }, |p| stage(Stage::Loading, p)))?;
    for line in prepared.describe() {
        eprintln!("{line}");
    }
    let data = &prepared.data;
    let held_out = train::held_out(data.views.len(), args.settings.hold_out_every);
    // Training gives way to VRChat on its graphics card: the queue asks for low priority (nothing else waits on it when
    // the GPU is free), the budget fits the graphics memory VRChat leaves, and with SteamVR the frame-timing governor
    // paces the steps (orbiters_reconstruct::governor, joined by train::train while VRChat runs).
    let mut gpu = match early_device {
        Some(thread) => timing::section("2 device (waited)", None, || thread.join()).map_err(|_| anyhow::anyhow!("the GPU device's thread panicked"))??,
        None => timing::section("2 device", None, || gpu::Gpu::with_options(device_options))?,
    };
    let dev = gpu.device.clone();
    let mut settings = args.settings.clone();
    if orbiters_reconstruct::priority::vrchat() && !gpu.integrated {
        fit_memory(&gpu, &mut settings);
    }
    stage(Stage::Seeding, 0.0);
    let budget = settings.budget * 2 / 3;
    let mut seeded = timing::section("3 seeding", None, || orbiters_reconstruct::priority::on_loading_threads(|| init::from_depth(data, budget, &held_out, |p| stage(Stage::Seeding, p))));
    // Depth frames that say too little (bloom beyond what decoders take out, extreme post-processing, no depth frames
    // at all): seeds along the keyframes' rays make up a tenth of the seeding budget, and the set may grow while many
    // are dead.
    let few = budget / 10;
    if seeded.len() < few {
        if seeded.is_empty() {
            eprintln!("{}", prepared.nothing_to_seed());
        }
        let from_depth = seeded.len();
        init::add_from_rays(&mut seeded, data, few - from_depth, &held_out, settings.seed);
        eprintln!("depth seeded {from_depth} Gaussians: {} more along the keyframes' rays", seeded.len() - from_depth);
        settings.starved = true;
    }
    // The texels of the sky map that seeding saw (sky::SkyMap::fill keeps them).
    let mut sky_seen = Vec::new();
    if settings.sky_map > 0 {
        // The sky as a map, which completes the sky itself: the shell's discs give way to it where the parallax test
        // found true sky (orbiters_reconstruct::sky). A capture none of whose pixels saw the sky keeps the shell.
        let (map, found) = timing::section("3 sky seed", None, || sky::seed_map(data, settings.sky_map, &held_out));
        if found.pixels > 0 {
            settings.sky_fill = false;
            let left = sky::give_way(&mut seeded, &found, train::camera_centre(data));
            eprintln!(
                "sky map: {} across from {} sky pixels, {} texels seen; {} of the {} 5.6° cells seen from both halves of the walk differ between them; {left} of the shell's discs leave for the map",
                map.width, found.pixels, found.texels_seen, found.cells_parallax, found.cells_tested
            );
            gpu.set_sky(Some((map.width, &map.rgb)))?;
            sky_seen = found.texels;
        } else {
            eprintln!("sky map: no keyframe pixel saw the sky: the shell of discs stays");
        }
    }
    if seeded.is_empty() {
        bail!("{}", prepared.nothing_to_seed());
    }
    stage(Stage::Training, 0.0);
    let outcome = timing::section("4 train", Some(&dev), || {
        train::train(data, &mut gpu, seeded, &settings, |p| {
            emit(&Event::Progress { iteration: p.iteration, iterations: p.iterations, gaussians: p.gaussians, intersections: p.intersections, psnr: p.psnr, seconds: p.seconds });
        })
    })?;

    stage(Stage::Saving, 0.0);
    let partial = dir.join(rec::PARTIAL_DIR);
    if partial.exists() {
        std::fs::remove_dir_all(&partial).with_context(|| format!("clearing {}", partial.display()))?;
    }
    std::fs::create_dir_all(partial.join(rec::HELD_OUT_DIR))?;
    timing::section("5 ply", None, || ply::write(&partial.join(rec::PLY_FILE), &outcome.gaussians))?;
    let sky_start = std::time::Instant::now();
    if let Some((width, rgb)) = gpu.sky()? {
        // What neither seeding nor training saw takes the sky's colour at its elevation, carried smoothly on from what
        // they saw (sky::SkyMap::fill): the map as viewers draw it, and as the held-out keyframes are scored.
        let trained = gpu.sky_trained()?.unwrap_or_default();
        let known: Vec<bool> = (0..rgb.len()).map(|t| trained.get(t).copied().unwrap_or(false) || sky_seen.get(t).copied().unwrap_or(false)).collect();
        let mut map = sky::SkyMap { width, rgb };
        map.fill(&known);
        gpu.set_sky(Some((map.width, &map.rgb)))?;
        map.write(&partial.join(rec::SKY_FILE))?;
    }
    timing::add("5 sky fill", sky_start.elapsed());
    // One render of each held-out keyframe: its preview and its score (the device holds the result).
    let write = |i: usize, img: &[[f32; 4]], preview: bool| -> anyhow::Result<()> {
        let encoded = qoi::encode(&rgb8(img), data.width, data.height, 3);
        if preview {
            std::fs::write(partial.join(rec::PREVIEW_FILE), &encoded)?;
        }
        std::fs::write(partial.join(rec::HELD_OUT_DIR).join(format!("{:06}.qoi", data.views[i].keyframe)), &encoded)?;
        Ok(())
    };
    let mut first = true;
    let psnr_held_out = timing::section("6 score held out", Some(&dev), || {
        if std::env::var_os("S4_PAR_SCORE").is_some() {
            // Lab: the CPU's share in parallel; the first held-out keyframe's render is the preview, as below.
            let preview = outcome.held_out.first().copied();
            train::score_held_out_parallel(&mut gpu, data, prepared.undo.as_ref(), &outcome.held_out, settings.background, |i, img| write(i, img, Some(i) == preview))
        } else {
            train::score_held_out(&mut gpu, data, prepared.undo.as_ref(), &outcome.held_out, settings.background, |i, img| write(i, img, std::mem::take(&mut first)))
        }
    })?;
    if outcome.held_out.is_empty() {
        write(0, &train::render(&mut gpu, &data.views[0].camera, settings.background)?, true)?;
    }
    let report = Report {
        gaussians: outcome.gaussians.len(),
        iterations: outcome.iterations,
        seconds: outcome.seconds,
        psnr_held_out,
        psnr_train: outcome.psnr_train,
        held_out_keyframes: outcome.held_out.iter().map(|&i| data.views[i].keyframe).collect(),
        width: data.width,
        height: data.height,
        adapter: gpu.adapter.clone(),
        created_utc: orbiters_core::now_utc_iso(),
        keyframes_total: data.loaded.keyframes,
        keyframes_used: data.views.len(),
        depth_frames_total: data.loaded.depth_frames,
        depth_frames_used: data.depth_views.len(),
        chromatic_aberration: prepared.undo.as_ref().map(|u| u.intensity),
        depth_samples: prepared.depth.as_ref().map(|f| f.samples),
        depth_unknown: prepared.depth.as_ref().map(|f| f.unknown_samples),
    };
    std::fs::write(partial.join(rec::REPORT_FILE), serde_json::to_string_pretty(&report)?)?;
    timing::section("7 replace", None, || replace(dir, &partial))?;
    timing::add("0 run", run_start.elapsed());
    timing::print();
    Ok(report)
}

/// Lowers `settings.budget` until a run fits the graphics memory the GPU has free (beside VRChat), leaving 1 GB for
/// VRChat's own growth (avatars loading in), but never under 200,000 Gaussians.
fn fit_memory(gpu: &gpu::Gpu, settings: &mut train::Settings) {
    const LEFT: u64 = 1 << 30;
    const FEWEST: usize = 200_000;
    let Some(free) = gpu.free_memory() else { return };
    let need = gpu.training_memory(settings.budget);
    if free >= need + LEFT {
        return;
    }
    let fixed = gpu.training_memory(0);
    let fits = (settings.budget as f64 * free.saturating_sub(LEFT + fixed) as f64 / need.saturating_sub(fixed).max(1) as f64) as usize;
    let budget = fits.clamp(FEWEST.min(settings.budget), settings.budget);
    eprintln!("graphics memory: {} MB free beside VRChat ({} MB needed): {budget} Gaussians instead of {}", free >> 20, need >> 20, settings.budget);
    settings.budget = budget;
}

/// Retries a file operation that another program (an indexer, an antivirus scan, a viewer just closing) holds for a
/// moment.
fn retry(mut f: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut wait = std::time::Duration::from_millis(50);
    for _ in 0..4 {
        if f().is_ok() {
            return Ok(());
        }
        std::thread::sleep(wait);
        wait *= 2;
    }
    f()
}

/// Puts back a previous result that a swap left in [`BACKUP_DIR`] (the run stopped between moving it out and moving
/// the new one in), and clears a backup whose swap finished.
fn recover(dir: &Path) {
    let (out, backup) = (rec::dir(dir), dir.join(BACKUP_DIR));
    if !backup.exists() {
        return;
    }
    if out.exists() {
        let _ = std::fs::remove_dir_all(&backup);
    } else if let Err(e) = std::fs::rename(&backup, &out) {
        eprintln!("could not put the previous result back from {}: {e}", backup.display());
    }
}

/// Moves the finished result in `partial` into place without ever deleting the previous one first: the previous
/// folder moves aside, the new one moves in, and only then is the previous one deleted (best effort; a backup left
/// behind is cleared by the next run). When the previous folder cannot move (a program has one of its files open),
/// its files are replaced one by one, the splats first: if even those cannot be replaced, the previous result stays
/// whole and the run fails saying so.
fn replace(dir: &Path, partial: &Path) -> anyhow::Result<()> {
    let out = rec::dir(dir);
    let backup = dir.join(BACKUP_DIR);
    recover(dir);
    if out.exists() && retry(|| std::fs::rename(&out, &backup)).is_err() {
        return replace_files(&out, partial).with_context(|| format!("replacing the previous result in {}", out.display()));
    }
    if let Err(e) = retry(|| std::fs::rename(partial, &out)) {
        // Back as it was.
        let _ = std::fs::rename(&backup, &out);
        return Err(e).with_context(|| format!("moving the result to {}", out.display()));
    }
    if backup.exists() {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(())
}

/// The fallback of [`replace`]: each file of `partial` over its namesake in `out`, the splats first (the file a viewer
/// keeps open) and the report last (it says what the folder holds). Held-out renders that cannot be replaced keep the
/// previous ones; renders of keyframes the new run did not hold out are deleted where possible.
fn replace_files(out: &Path, partial: &Path) -> anyhow::Result<()> {
    retry(|| std::fs::rename(partial.join(rec::PLY_FILE), out.join(rec::PLY_FILE))).with_context(|| format!("{} is open in another program: close it and reconstruct again", out.join(rec::PLY_FILE).display()))?;
    // The sky map goes with the splats it was trained with: a result without one leaves none behind.
    let sky = out.join(rec::SKY_FILE);
    if partial.join(rec::SKY_FILE).exists() {
        retry(|| std::fs::rename(partial.join(rec::SKY_FILE), &sky)).with_context(|| format!("{} is open in another program: close it and reconstruct again", sky.display()))?;
    } else if sky.exists() {
        retry(|| std::fs::remove_file(&sky)).with_context(|| format!("{} is open in another program: close it and reconstruct again", sky.display()))?;
    }
    let _ = retry(|| std::fs::rename(partial.join(rec::PREVIEW_FILE), out.join(rec::PREVIEW_FILE)));
    let (from, to) = (partial.join(rec::HELD_OUT_DIR), out.join(rec::HELD_OUT_DIR));
    std::fs::create_dir_all(&to)?;
    let fresh: Vec<std::ffi::OsString> = std::fs::read_dir(&from).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    for name in &fresh {
        let _ = retry(|| std::fs::rename(from.join(name), to.join(name)));
    }
    if let Ok(old) = std::fs::read_dir(&to) {
        for e in old.flatten().filter(|e| !fresh.contains(&e.file_name())) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    retry(|| std::fs::rename(partial.join(rec::REPORT_FILE), out.join(rec::REPORT_FILE)))?;
    let _ = std::fs::remove_dir_all(partial);
    Ok(())
}

/// `orbiters-reconstruct --web-preview <splats.ply> <out.splat> [--max N]`: the reconstruction as a web preview
/// (orbiters_reconstruct::web), started from the scene's centre.
fn web_preview(args: &[String]) -> anyhow::Result<()> {
    let (Some(ply_path), Some(out)) = (args.first(), args.get(1)) else { bail!("usage: orbiters-reconstruct --web-preview <splats.ply> <out.splat> [--max N]") };
    let max = match args.get(2).map(String::as_str) {
        Some("--max") => args.get(3).context("--max N")?.parse()?,
        _ => orbiters_reconstruct::web::DEFAULT_MAX,
    };
    let g = ply::read(std::path::Path::new(ply_path))?;
    let n = g.len().max(1) as f32;
    let centre = g.means.iter().fold([0.0f32; 3], |a, m| [a[0] + m[0] / n, a[1] + m[1] / n, a[2] + m[2] / n]);
    let web = orbiters_reconstruct::web::splat_file(&g, max, centre);
    std::fs::write(out, &web.bytes)?;
    println!("{} of {} splats, {:.1} MB", web.count, g.len(), web.bytes.len() as f64 / 1e6);
    Ok(())
}

fn main() {
    orbiters_core::panic_log::install();
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().map(String::as_str) == Some("--web-preview") {
        if let Err(e) = web_preview(&all[1..]) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    match args().and_then(run) {
        Ok(report) => emit(&Event::Done { report }),
        Err(e) => {
            emit(&Event::Error { message: format!("{e:#}") });
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-reconstruct-swap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A result folder `name` in `dir` whose splats read `ply` and whose report, preview and one render read `report`.
    fn result(dir: &Path, name: &str, ply: &str, report: &str) {
        let d = dir.join(name);
        std::fs::create_dir_all(d.join(rec::HELD_OUT_DIR)).unwrap();
        std::fs::write(d.join(rec::PLY_FILE), ply).unwrap();
        std::fs::write(d.join(rec::REPORT_FILE), report).unwrap();
        std::fs::write(d.join(rec::PREVIEW_FILE), report).unwrap();
        std::fs::write(d.join(rec::HELD_OUT_DIR).join(format!("{report}.qoi")), report).unwrap();
    }

    fn read(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(rec::dir(dir).join(file)).unwrap()
    }

    #[test]
    fn a_new_result_replaces_the_previous_one_whole() {
        let dir = session("whole");
        result(&dir, rec::PARTIAL_DIR, "new-1", "r1");
        replace(&dir, &dir.join(rec::PARTIAL_DIR)).unwrap();
        assert_eq!(read(&dir, rec::PLY_FILE), "new-1");
        result(&dir, rec::PARTIAL_DIR, "new-2", "r2");
        replace(&dir, &dir.join(rec::PARTIAL_DIR)).unwrap();
        assert_eq!((read(&dir, rec::PLY_FILE), read(&dir, rec::REPORT_FILE)), ("new-2".into(), "r2".into()));
        assert!(!dir.join(BACKUP_DIR).exists() && !dir.join(rec::PARTIAL_DIR).exists());
        let renders = rec::dir(&dir).join(rec::HELD_OUT_DIR);
        assert!(renders.join("r2.qoi").exists() && !renders.join("r1.qoi").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A run stopped between moving the previous result aside and moving the new one in: the previous one comes back.
    #[test]
    fn a_swap_cut_short_puts_the_previous_result_back() {
        let dir = session("cut");
        result(&dir, BACKUP_DIR, "old", "r0");
        recover(&dir);
        assert_eq!(read(&dir, rec::PLY_FILE), "old");
        assert!(!dir.join(BACKUP_DIR).exists());
        // Both there (the swap finished, its backup stayed): the backup goes.
        result(&dir, BACKUP_DIR, "older", "r-1");
        recover(&dir);
        assert_eq!(read(&dir, rec::PLY_FILE), "old");
        assert!(!dir.join(BACKUP_DIR).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A viewer holding the previous splats open without delete sharing (as most programs open files): the previous
    /// result stays whole and the run says why. With only a render held open, the splats and report are the new ones.
    #[cfg(windows)]
    #[test]
    fn a_result_held_open_by_another_program_is_never_left_half_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        const SHARE_READ: u32 = 1;
        let dir = session("held");
        result(&dir, rec::DIR, "old", "r0");
        result(&dir, rec::PARTIAL_DIR, "new", "r1");
        let held = std::fs::OpenOptions::new().read(true).share_mode(SHARE_READ).open(rec::dir(&dir).join(rec::PLY_FILE)).unwrap();
        let outcome = replace(&dir, &dir.join(rec::PARTIAL_DIR));
        let (ply, report) = (read(&dir, rec::PLY_FILE), read(&dir, rec::REPORT_FILE));
        match outcome {
            Ok(()) => assert_eq!((ply.as_str(), report.as_str()), ("new", "r1")),
            Err(e) => {
                assert_eq!((ply.as_str(), report.as_str()), ("old", "r0"), "{e:#}");
                assert!(format!("{e:#}").contains("open in another program"), "{e:#}");
            }
        }
        drop(held);
        result(&dir, rec::PARTIAL_DIR, "newer", "r2");
        let held = std::fs::OpenOptions::new().read(true).share_mode(SHARE_READ).open(rec::dir(&dir).join(rec::PREVIEW_FILE)).unwrap();
        replace(&dir, &dir.join(rec::PARTIAL_DIR)).expect("replaced");
        drop(held);
        assert_eq!((read(&dir, rec::PLY_FILE), read(&dir, rec::REPORT_FILE)), ("newer".into(), "r2".into()));
        recover(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The sky map goes with its splats, file by file too (a viewer holding a render open): a new map replaces the
    /// previous one, and a result without one leaves none behind.
    #[cfg(windows)]
    #[test]
    fn the_sky_map_goes_with_its_splats() {
        use std::os::windows::fs::OpenOptionsExt;
        const SHARE_READ: u32 = 1;
        let dir = session("sky");
        result(&dir, rec::DIR, "old", "r0");
        std::fs::write(rec::dir(&dir).join(rec::SKY_FILE), "sky0").unwrap();
        for (ply, report, sky) in [("new", "r1", Some("sky1")), ("newer", "r2", None)] {
            result(&dir, rec::PARTIAL_DIR, ply, report);
            if let Some(s) = sky {
                std::fs::write(dir.join(rec::PARTIAL_DIR).join(rec::SKY_FILE), s).unwrap();
            }
            let held = std::fs::OpenOptions::new().read(true).share_mode(SHARE_READ).open(rec::dir(&dir).join(rec::PREVIEW_FILE)).unwrap();
            replace(&dir, &dir.join(rec::PARTIAL_DIR)).expect("replaced");
            drop(held);
            assert_eq!(read(&dir, rec::PLY_FILE), ply);
            assert_eq!(std::fs::read_to_string(rec::dir(&dir).join(rec::SKY_FILE)).ok().as_deref(), sky);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
