//! Reconstruction worker of Orbiters - N4N0:
//!
//! ```text
//! orbiters-reconstruct <session folder> [--width N] [--iterations N] [--budget N] [--robust T] [--gpu integrated|fastest]
//!     [--regularise-unseen] [--adam-restart] [--filter-3d S] [--cleanup]
//!     [--sources opacity|gradient|error|confidence] [--sources-by-opacity] [--split footprint|long-axis|long-axis-on-depth]
//!     [--noise λ | --no-noise] [--no-noise-gate] [--noise-visible] [--noise-dead-max share] [--depth-births share]
//!     [--nested-forward | --flat-forward] [--gpu-profile] [--resume] [--pause-at step,…] [--pause-in loading|seeding|finish|saving]
//! ```
//!
//! Prints one [`Event`] per line on stdout for the app and writes the session's `reconstruction/` folder: the splats
//! (PLY), the report, a preview and a render of every held-out keyframe. The folder is replaced only once everything
//! is written, and never left half deleted ([`replace`]).
//!
//! A line `pause` on stdin pauses it (orbiters_reconstruct::checkpoint): it frees the GPU, writes where it stood into the
//! session's folder and exits; `--resume` goes on from there. Without `--resume` a run starts over and leaves no
//! checkpoint of an earlier one. `--pause-at` and `--pause-in` are the lab's pauses at chosen points of a run: steps, or
//! shares of the steps under 1 (each start goes on until the next one).

use anyhow::{bail, Context as _};
use orbiters_contracts::qoi;
use orbiters_core::reconstruction::{self as rec, Event, Report, Stage};
use orbiters_i18n::tr;
use orbiters_reconstruct::checkpoint::{self, Extras, Interrupted, Lab, LabStage, Pause};
use orbiters_reconstruct::{gpu, init, ply, prepare, sky, timing, train};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// `--budget`: the most Gaussians whatever the session (else train::budget_for picks it).
    budget: Option<usize>,
    device: gpu::DeviceOptions,
    portrait_boxes: Option<PathBuf>,
    /// `--wearer`: whether the wearer leaves this session (else as the session says).
    wearer: orbiters_reconstruct::prepare::WearerOut,
    /// Goes on from a paused run's checkpoint, when it has one that still fits.
    resume: bool,
    lab: Lab,
}

fn args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut session = None;
    let mut width = 1920;
    let mut settings = train::Settings::default();
    let mut budget = None;
    let mut device = gpu::DeviceOptions::default();
    let mut portrait_boxes = None;
    let mut wearer = orbiters_reconstruct::prepare::WearerOut::Auto;
    let mut resume = false;
    let mut lab = Lab::default();
    while let Some(a) = it.next() {
        let mut number = |name: &str| -> anyhow::Result<u64> { it.next().with_context(|| tr!("{name} needs a value", name = name))?.parse().with_context(|| tr!("{name} takes a number", name = name)) };
        match a.as_str() {
            "--width" => width = number("--width")? as u32,
            // Lab: the wearer leaves or stays whatever the session says (orbiters_reconstruct::wearer).
            "--wearer" => {
                wearer = match it.next().as_deref() {
                    Some("auto") => orbiters_reconstruct::prepare::WearerOut::Auto,
                    Some("keep") => orbiters_reconstruct::prepare::WearerOut::Keep,
                    Some("remove") => orbiters_reconstruct::prepare::WearerOut::Remove,
                    _ => bail!("--wearer takes auto, keep or remove"),
                }
            }
            "--iterations" => settings.iterations = number("--iterations")? as u32,
            "--budget" => budget = Some(number("--budget")? as usize),
            "--robust" => settings.robust = it.next().context("--robust needs a value")?.parse().context("--robust takes a number (0–1)")?,
            // Lab A/Bs: the regularisers on every Gaussian every step (orbiters_reconstruct::train::Settings).
            "--regularise-unseen" => settings.regularise_unseen = true,
            // Colours within 0–1 at every step, what the splat files hold.
            "--clamp-colors" => settings.clamp_colors = true,
            // Adam's bias correction per Gaussian from its restart, rather than from the run's first step.
            "--adam-restart" => settings.adam_restart = true,
            // Mip-Splatting's 3D filter's scale (0, the default: none; its own is orbiters_reconstruct::cpu::FILTER_3D).
            "--filter-3d" => settings.filter_3d = it.next().context("--filter-3d needs a value")?.parse().context("--filter-3d takes a number")?,
            // What the exact depth says is not there leaves the result (orbiters_reconstruct::cleanup).
            "--cleanup" => settings.cleanup = true,
            // The sky map's width (orbiters_reconstruct::sky::SkyMap; 0: the shell of discs).
            "--sky-map" => settings.sky_map = number("--sky-map")? as u32,
            // Lab A/Bs of the set's size (orbiters_reconstruct::train::Settings): fading what no training pixel sees
            // (every N steps, 0: never; at what blending weight) and the share of the steps relocation runs in.
            "--fade-every" => settings.fade_unseen_every = number("--fade-every")? as u32,
            "--unseen-weight" => settings.unseen_weight = it.next().context("--unseen-weight needs a value")?.parse().context("--unseen-weight takes a number")?,
            "--relocate-until" => settings.relocate_until = it.next().context("--relocate-until needs a value")?.parse().context("--relocate-until takes a number (0–1)")?,
            "--fade-to-the-end" => settings.fade_until_rising = false,
            // Lab A/Bs of where relocation places Gaussians (orbiters_reconstruct::train::{Sources, Split}): which live
            // ones it copies, how a copy splits its source, the SGLD noise on the means (on by default: λ 5e5, spared on
            // measured depth, only in rounds at most a quarter dead; --no-noise, --no-noise-gate, --noise-dead-max 0 for
            // every round, --noise-visible: only what the step shows) and the share of the moves born at measured depth.
            "--sources" => {
                settings.sources = match it.next().as_deref() {
                    Some("opacity") => train::Sources::Opacity,
                    Some("gradient") => train::Sources::Gradient,
                    Some("error") => train::Sources::Error,
                    Some("confidence") => train::Sources::Confidence,
                    _ => bail!("--sources takes opacity, gradient, error or confidence"),
                }
            }
            "--split" => {
                settings.split = match it.next().as_deref() {
                    Some("footprint") => train::Split::Footprint,
                    Some("long-axis") => train::Split::LongAxis,
                    Some("long-axis-on-depth") => train::Split::LongAxisOnDepth,
                    _ => bail!("--split takes footprint, long-axis or long-axis-on-depth"),
                }
            }
            "--sources-by-opacity" => settings.sources_by_opacity = true,
            "--noise" => settings.noise = it.next().context("--noise needs a value")?.parse().context("--noise takes a number")?,
            "--noise-gate" => settings.noise_gate = true,
            "--no-noise" => settings.noise = 0.0,
            "--no-noise-gate" => settings.noise_gate = false,
            "--noise-visible" => settings.noise_visible = true,
            "--noise-dead-max" => settings.noise_dead_max = it.next().context("--noise-dead-max needs a value")?.parse().context("--noise-dead-max takes a share (0–1)")?,
            // A 3D picture's detail views' share of the colour steps (orbiters_reconstruct::dataset::detail_views,
            // train::Settings::detail_share, a quarter by default; 0: trained as any keyframe).
            "--detail-share" => settings.detail_share = it.next().context("--detail-share needs a value")?.parse().context("--detail-share takes a share (0–1)")?,
            "--portrait-steps" => settings.portrait_steps = number("--portrait-steps")? as u32,
            "--portrait-moment" => settings.portrait_moment = it.next().context("--portrait-moment needs a value")?.parse().context("--portrait-moment takes seconds")?,
            "--portrait-boxes" => portrait_boxes = Some(PathBuf::from(it.next().context("--portrait-boxes needs a file")?)),
            "--depth-births" => settings.depth_births = it.next().context("--depth-births needs a value")?.parse().context("--depth-births takes a number (0–1)")?,
            "--gpu" => {
                device.low_power = match it.next().as_deref() {
                    Some("integrated") => true,
                    Some("fastest") => false,
                    _ => bail!("--gpu takes integrated or fastest"),
                }
            }
            // Lab: GPU timestamps around every pass of the whole run, printed as one `gpu_profile {json}` line on
            // stderr at the end (per section: milliseconds and passes summed over all steps). The timestamps are read
            // back after each step, which waits for it: the run is slower, the shares are what to read.
            "--gpu-profile" => device.profile = true,
            "--nested-forward" => settings.nested_forward = true,
            "--flat-forward" => settings.nested_forward = false,
            "--resume" => resume = true,
            "--pause-at" => {
                lab.at = it.next().context("--pause-at needs steps")?.split(',').map(str::parse).collect::<Result<_, _>>().context("--pause-at takes numbers")?;
            }
            "--pause-in" => lab.stage = Some(LabStage::parse(&it.next().context("--pause-in needs a stage")?)?),
            _ if a.starts_with("--") => bail!("{}", tr!("unknown option {option}", option = a)),
            _ => session = Some(PathBuf::from(a)),
        }
    }
    Ok(Args { resume, lab, portrait_boxes, session: session.with_context(|| tr("usage: orbiters-reconstruct <session folder> [--width N] [--iterations N] [--budget N] [--robust T] [--gpu integrated|fastest]"))?, width, settings, budget, device, wearer })
}

fn rgb8(img: &[[f32; 4]]) -> Vec<u8> {
    img.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
}

/// How a run ended without failing.
enum Ended {
    Done(Report),
    Paused,
}

/// Leaves before anything worth keeping exists (reading the session, seeding: seconds of the CPU, started over): a
/// checkpoint from an earlier pause stays where it is.
fn leave_early(session: &Path) -> ! {
    emit(&Event::Paused { kept: checkpoint::exists(session), message: None });
    std::process::exit(0);
}

/// The settings a checkpoint must have been made with. The GPU is not among them: a run on the integrated GPU goes on
/// on the graphics card and back. Whether the wearer leaves is, as the session says it now (`wearer.json` may have changed since
/// the pause): a run trained with them in the picture cannot go on as one without them, nor the other way round.
fn settings_key(args: &Args) -> String {
    let wearer_leaves = args.wearer.goes(&args.session);
    format!("{:?} width {} budget {:?} boxes {:?} wearer leaves {wearer_leaves}", args.settings, args.width, args.budget, args.portrait_boxes)
}

/// Pauses: the device goes first, which frees the graphics memory, then what the run keeps is written. At the disk's
/// normal priority: Windows' background mode made a 480 MB checkpoint crawl at 1.3 MB/s for six minutes beside other
/// programs' disk use, and the run cannot go on until it is written (0.7 s for 604 MB otherwise).
fn leave(session: &Path, key: &str, state: train::Start, extras: &Extras, gpu: gpu::Gpu, dev: wgpu::Device, pause: &Pause) -> anyhow::Result<Ended> {
    pause.ask();
    let read_back = pause.since_asked();
    drop(gpu);
    drop(dev);
    let freed = pause.since_asked();
    emit(&Event::Pausing);
    let writing = std::time::Instant::now();
    let _ = std::fs::remove_dir_all(session.join(rec::PARTIAL_DIR));
    let (kept, message) = match checkpoint::write(session, key, &state, extras) {
        Ok(bytes) => {
            eprintln!("checkpoint: {} MB; since asked: state read back {:.3} s, device gone {:.3} s; written in {:.3} s", bytes >> 20, read_back.as_secs_f32(), freed.as_secs_f32(), writing.elapsed().as_secs_f32());
            (true, None)
        }
        Err(e) => {
            // A checkpoint from an earlier pause stays: the run goes on from there.
            eprintln!("no checkpoint: {e:#}");
            (checkpoint::exists(session), Some(format!("{e:#}")))
        }
    };
    emit(&Event::Paused { kept, message });
    Ok(Ended::Paused)
}

fn run(args: Args) -> anyhow::Result<Ended> {
    let run_start = std::time::Instant::now();
    let dir = &args.session;
    // Above normal while VRChat is closed, out of its way while it runs (orbiters_reconstruct::priority).
    orbiters_reconstruct::priority::govern();
    // A swap an earlier run stopped in the middle of: its previous result is back in place first.
    recover(dir);
    let pause = Arc::new(Pause::default());
    {
        let session = dir.clone();
        checkpoint::listen(pause.clone(), move || leave_early(&session));
    }
    let lab = &args.lab;
    let early_lab = |stage: LabStage| {
        if lab.stage == Some(stage) {
            leave_early(dir);
        }
    };
    stage(Stage::Loading, 0.0);
    early_lab(LabStage::Loading);
    let key = checkpoint::key(dir, &settings_key(&args))?;
    // What a killed writer left; then what it finished, when asked for and still of this run.
    checkpoint::sweep(dir);
    let header = if args.resume {
        checkpoint::matching_header(dir, &key)
    } else {
        checkpoint::discard(dir);
        None
    };
    if let Some(h) = &header {
        eprintln!("going on from step {} of {} ({} Gaussians, {:.1} s trained)", h.iteration, h.iterations, h.gaussians, h.seconds);
        emit(&Event::Resumed { iteration: h.iteration, iterations: h.iterations, gaussians: h.gaussians, psnr: h.psnr, seconds: h.seconds });
    }
    // Read beside the session's frames: both are mostly the disk.
    let body = header.clone().map(|h| {
        let session = dir.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let body = checkpoint::read_body(&session, &h);
            eprintln!("checkpoint read in {:.3} s", start.elapsed().as_secs_f32());
            body
        })
    });
    // Training gives way to VRChat on its graphics card: the queue asks for low priority (nothing else waits on it when
    // the GPU is free), the budget fits the graphics memory VRChat leaves, and with SteamVR the frame-timing governor
    // paces the steps (orbiters_reconstruct::governor, joined by train::train while VRChat runs). The device and its
    // kernels come up while the session loads (about half a second each).
    let device_options = gpu::DeviceOptions { low_priority: true, ..args.device };
    let device = std::thread::spawn(move || timing::section("2 device", None, || gpu::Gpu::with_options(device_options)));
    // Within the memory budget; depth that says nothing becomes unknown, the world's chromatic aberration leaves the
    // keyframes.
    let prepared = timing::section("1 prepare", None, || prepare::prepare(dir, &prepare::Options { width: args.width, wearer: args.wearer, ..Default::default() }, |p| stage(Stage::Loading, p)))?;
    for line in prepared.describe() {
        eprintln!("{line}");
    }
    let data = &prepared.data;
    let held_out = train::held_out_views(data, args.settings.hold_out_every);
    let mut gpu = device.join().map_err(|_| anyhow::anyhow!("{}", tr("the GPU device's thread panicked")))??;
    let dev = gpu.device.clone();
    let mut settings = args.settings.clone();
    settings.wearer = prepared.wearer.as_ref().map(|(volume, _)| std::sync::Arc::new(volume.clone())).filter(|v| !v.is_empty());
    // A picture of the world has no faces to spend steps on (orbiters_core::picture), resumed or not.
    if !orbiters_core::picture::Picture::trains_faces(dir) {
        settings.portrait_steps = 0;
        eprintln!("a picture of the world: no steps on faces");
    }
    // A checkpoint that cannot be read costs what it held, no more: the run starts over.
    let restored = match body.map(|b| b.join().map_err(|_| anyhow::anyhow!("reading the checkpoint panicked"))?).transpose() { // i18n: not shown (only reaches the worker's log: the run starts over)
        Ok(found) => found,
        Err(e) => {
            eprintln!("the paused run's checkpoint is not used ({e:#}): starting over");
            checkpoint::discard(dir);
            None
        }
    };
    let (from, extras) = match restored {
        Some(cp) => {
            settings.portraits = cp.extras.portraits.clone();
            settings.starved = cp.extras.starved;
            settings.sky_fill = cp.extras.sky_fill;
            // The budget it had, as far as the graphics memory free now holds it (beside VRChat, it may be less), and
            // never under the set it has.
            settings.budget = cp.extras.budget;
            // A Mac's GPU counts as integrated but is its only one: it shares its memory with every program, so a large
            // session keeps within what they leave, as it does beside VRChat.
            if !gpu.integrated || cfg!(target_vendor = "apple") {
                fit_memory(&gpu, &mut settings);
            }
            let held = match &cp.start {
                train::Start::Steps(s) => s.gaussians,
                train::Start::Trained(t) => t.gaussians.len(),
                train::Start::Seeds(g) => g.len(),
            };
            settings.budget = settings.budget.max(held);
            if let Some((width, rgb)) = &cp.extras.trained_sky {
                gpu.set_sky(Some((*width, rgb)))?;
            }
            (cp.start, cp.extras)
        }
        None => {
            // The faces in its keyframes, for the extra training they get (orbiters_reconstruct::portraits).
            if let Some(file) = &args.portrait_boxes {
                settings.portraits = orbiters_reconstruct::portraits::from_lab_file(file, data)?;
            } else if settings.portrait_steps > 0 {
                let found = timing::section("1b faces", None, || orbiters_reconstruct::portraits::find(data));
                if found.ran {
                    eprintln!("faces: {} heads ({} known still), {} of {} boxes in {} keyframes kept, {:.2} s", found.heads, found.still, found.portraits.len(), found.detections, found.keyframes, found.seconds);
                }
                settings.portraits = found.portraits;
            }
            // The wearer's own face, in a session they left, gets no steps: its pixels train on nothing.
            settings.portraits.retain(|p| {
                let (x, y) = ((((p.x0 + p.x1) / 2.0) as u32).min(data.width - 1), (((p.y0 + p.y1) / 2.0) as u32).min(data.height - 1));
                !orbiters_reconstruct::wearer::is_out(data.views[p.view].image[(y * data.width + x) as usize])
            });
            // As many Gaussians as the session's views can tell apart (train::budget_for), within the graphics memory the GPU
            // has free (beside VRChat, what it leaves).
            let views = data.views.len() + data.faces.as_ref().map_or(0, |f| f.views.len());
            settings.budget = args.budget.unwrap_or_else(|| train::budget_for(views));
            eprintln!("budget: {} Gaussians for {views} views", settings.budget);
            // A Mac's GPU counts as integrated but is its only one: it shares its memory with every program, so a large
            // session keeps within what they leave, as it does beside VRChat.
            if !gpu.integrated || cfg!(target_vendor = "apple") {
                fit_memory(&gpu, &mut settings);
            }
            stage(Stage::Seeding, 0.0);
            early_lab(LabStage::Seeding);
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
            // What only the capture rig's faces saw has no depth: seeds along their rays, a tenth of the seeding budget.
            if let Some(faces) = &data.faces {
                let before = seeded.len();
                init::add_from_face_rays(&mut seeded, data, budget / 10, settings.seed ^ 0xFACE);
                eprintln!("the rig's {} faces: {} seeds along their rays", faces.views.len(), seeded.len() - before);
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
            let extras = Extras { budget: settings.budget, starved: settings.starved, sky_fill: settings.sky_fill, portraits: settings.portraits.clone(), sky_seen, trained_sky: None };
            (train::Start::Seeds(seeded), extras)
        }
    };
    let mut extras = extras;
    let done_share = match &from {
        train::Start::Steps(s) => s.iteration as f32 / s.iterations.max(1) as f32,
        train::Start::Trained(_) => 1.0,
        train::Start::Seeds(_) => 0.0,
    };
    // From here on a pause has something to keep.
    pause.begin_steps();
    stage(Stage::Training, done_share);
    let control = train::Control { pause: Some(&pause), lab: Some(lab) };
    let ran = timing::section("4 train", Some(&dev), || {
        train::train_from(data, &mut gpu, from, &settings, control, |p| {
            emit(&Event::Progress { iteration: p.iteration, iterations: p.iterations, gaussians: p.gaussians, intersections: p.intersections, psnr: p.psnr, seconds: p.seconds });
        })
    })?;
    // What training left of the sky map: the texels it moved, and those seeding saw. Read here, so that a pause from here on
    // keeps the map as training left it (its Adam moments are gone once the map is completed).
    let sky_after = |gpu: &gpu::Gpu, seen: &[bool]| -> anyhow::Result<Option<(u32, Vec<[f32; 3]>, Vec<bool>)>> {
        let Some((width, rgb)) = gpu.sky()? else { return Ok(None) };
        let trained = gpu.sky_trained()?.unwrap_or_default();
        let known = (0..rgb.len()).map(|t| trained.get(t).copied().unwrap_or(false) || seen.get(t).copied().unwrap_or(false)).collect();
        Ok(Some((width, rgb, known)))
    };
    let outcome = match ran {
        train::Ran::Finished(outcome) => outcome,
        train::Ran::Paused(state) => {
            if matches!(state, train::Start::Trained(_)) {
                if let Some((width, rgb, known)) = sky_after(&gpu, &extras.sky_seen)? {
                    extras.trained_sky = Some((width, rgb));
                    extras.sky_seen = known;
                }
            }
            return leave(dir, &key, state, &extras, gpu, dev, &pause);
        }
    };
    let trained_sky = sky_after(&gpu, &extras.sky_seen)?;
    lab.entering(LabStage::Saving, Some(&pause));
    stage(Stage::Saving, 0.0);
    // Paused from here on: the final set and the map as training left it stay, what is below starts over (seconds).
    let paused_after_steps = |outcome: train::Outcome, extras: &mut Extras, psnr: Option<f32>| {
        if let Some((width, rgb, known)) = trained_sky.clone() {
            extras.trained_sky = Some((width, rgb));
            extras.sky_seen = known;
        }
        train::Start::Trained(Box::new(checkpoint::TrainedState { gaussians: outcome.gaussians, iterations: outcome.iterations, seconds: outcome.seconds, psnr, wearer_left: outcome.wearer_left, floor_patched: outcome.floor_patched }))
    };
    if pause.asked() {
        eprintln!("pausing while saving");
        let state = paused_after_steps(outcome, &mut extras, None);
        return leave(dir, &key, state, &extras, gpu, dev, &pause);
    }
    let partial = dir.join(rec::PARTIAL_DIR);
    if partial.exists() {
        std::fs::remove_dir_all(&partial).with_context(|| tr!("clearing {path}", path = partial.display()))?;
    }
    std::fs::create_dir_all(partial.join(rec::HELD_OUT_DIR))?;
    timing::section("5 ply", None, || ply::write(&partial.join(rec::PLY_FILE), &outcome.gaussians))?;
    let sky_start = std::time::Instant::now();
    if let Some((width, rgb, known)) = trained_sky.clone() {
        // What neither seeding nor training saw takes the sky's colour at its elevation, carried smoothly on from what
        // they saw (sky::SkyMap::fill): the map as viewers draw it, and as the held-out keyframes are scored.
        let mut map = sky::SkyMap { width, rgb };
        map.fill(&known);
        gpu.set_sky(Some((map.width, &map.rgb)))?;
        map.write(&partial.join(rec::SKY_FILE))?;
    }
    timing::add("5 sky fill", sky_start.elapsed());
    // One render of each held-out keyframe: its preview and its score (the device holds the result).
    let write = |i: usize, img: &[[f32; 4]]| -> anyhow::Result<()> {
        let encoded = qoi::encode(&rgb8(img), data.width, data.height, 3);
        std::fs::write(partial.join(rec::HELD_OUT_DIR).join(format!("{:06}.qoi", data.views[i].keyframe)), &encoded)?;
        Ok(())
    };
    let scored = timing::section("6 score held out", Some(&dev), || {
        train::score_held_out(&mut gpu, data, prepared.undo.as_ref(), &outcome.held_out, settings.background, write)
    });
    let psnr_held_out = match scored {
        Err(e) if e.is::<Interrupted>() => {
            eprintln!("pausing while scoring");
            let state = paused_after_steps(outcome, &mut extras, None);
            return leave(dir, &key, state, &extras, gpu, dev, &pause);
        }
        scored => scored?,
    };
    // The preview is the first keyframe's view, what the person framed. It was the first held-out keyframe's, and a 3D picture's
    // first seconds turn every way: on 2026-11-06 it showed a bare wall and an eye of what floated in front of it.
    let preview = train::render(&mut gpu, &data.views[0].camera, settings.background)?;
    std::fs::write(partial.join(rec::PREVIEW_FILE), qoi::encode(&rgb8(&preview), data.width, data.height, 3))?;
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
        wearer: prepared.wearer.as_ref().map(|(_, s)| rec::WearerOut { pixel_share: s.pixel_share, views: s.views, views_total: s.views_total, depth_samples: s.depth_samples, depth_hits: s.depth_hits, gaussians_left: outcome.wearer_left, floor_patched: outcome.floor_patched }),
        wearer_not_found: prepared.wearer_not_found,
        depth_blind: prepared.depth.as_ref().is_some_and(|f| f.blind_world),
        pacing: pacing_report(&gpu.pacing()),
        memory: gpu.memory_seen().map(|m| rec::MemorySeen { headroom_mb: m.headroom.map(|b| b >> 20), adapter_used_mb: m.adapter_used.map(|b| b >> 20), heap_mb: m.heap.map(|b| b >> 20) }),
    };
    std::fs::write(partial.join(rec::REPORT_FILE), serde_json::to_string_pretty(&report)?)?;
    // On the disk before the previous result goes (RECONSTRUCT-14a): a power cut after the swap leaves a result whose
    // files hold what was written, not zeros. A file that cannot be flushed fails the run with the previous one whole.
    timing::section("7 flush", None, || flush_all(&partial))?;
    timing::section("7 replace", None, || replace(dir, &partial))?;
    // The run is over: what it could have gone on from goes (a pause asked this late changes nothing).
    checkpoint::discard(dir);
    timing::add("0 run", run_start.elapsed());
    timing::print();
    if let Some(p) = &gpu.profiler {
        let (span, steps) = p.span.get();
        let sections: serde_json::Map<String, serde_json::Value> = p.totals.borrow().iter().map(|(k, (ms, n))| (k.to_string(), serde_json::json!({"ms": ms, "passes": n}))).collect();
        eprintln!("gpu_profile {}", serde_json::json!({"steps": steps, "span_ms": span, "sections": sections, "gaussians": report.gaussians, "iterations": report.iterations, "seconds": report.seconds}));
    }
    Ok(Ended::Done(report))
}

/// Lowers `settings.budget` until a run fits the graphics memory the GPU has free (beside VRChat, what it leaves),
/// leaving 1 GB for other programs' growth (VRChat's avatars loading in), but never under 200,000 Gaussians.
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
    eprintln!("graphics memory: {} MB free ({} MB needed): {budget} Gaussians instead of {}", free >> 20, need >> 20, settings.budget);
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

/// The report's account of a run's pacing (RECONSTRUCT-10), `None` when nothing paced it.
fn pacing_report(t: &orbiters_reconstruct::governor::Tally) -> Option<rec::Pacing> {
    let mode = match (t.vr, t.desktop) {
        (false, false) => return None,
        (true, false) => "vr",
        (false, true) => "desktop",
        (true, true) => "vr+desktop",
    };
    Some(rec::Pacing {
        mode: mode.into(),
        waited_s: t.waited.as_secs_f32(),
        longest_wait_s: t.longest.as_secs_f32(),
        waits: t.waits,
        backoffs: t.backoffs,
        cut_by_pause: t.cut_by_pause,
    })
}

/// Flushes every file under `dir` to the disk (`sync_all`), folders included where the system allows it.
fn flush_all(dir: &Path) -> anyhow::Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| tr!("writing {path}", path = dir.display()))? {
        let e = e?;
        let path = e.path();
        if e.file_type()?.is_dir() {
            flush_all(&path)?;
        } else {
            std::fs::OpenOptions::new().write(true).open(&path).and_then(|f| f.sync_all()).with_context(|| tr!("writing {path}", path = path.display()))?;
        }
    }
    Ok(())
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
        return replace_files(&out, partial).with_context(|| tr!("replacing the previous result in {path}", path = out.display()));
    }
    if let Err(e) = retry(|| std::fs::rename(partial, &out)) {
        // Back as it was.
        let _ = std::fs::rename(&backup, &out);
        return Err(e).with_context(|| tr!("moving the result to {path}", path = out.display()));
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
    retry(|| std::fs::rename(partial.join(rec::PLY_FILE), out.join(rec::PLY_FILE))).with_context(|| tr!("{path} is open in another program: close it and reconstruct again", path = out.join(rec::PLY_FILE).display()))?;
    // The previous splats' edits and the worker's pictures kept aside for them go with them (a folder moved as a whole
    // takes them along): edits name splats of the PLY they were made on, and a picture kept aside belongs to its PLY.
    retry(|| match std::fs::remove_file(out.join(rec::EDIT_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    })
    .with_context(|| tr!("{path} is open in another program: close it and reconstruct again", path = out.join(rec::EDIT_FILE).display()))?;
    for e in std::fs::read_dir(out).into_iter().flatten().flatten() {
        if rec::is_trained_preview_file(&e.file_name().to_string_lossy()) {
            let _ = retry(|| std::fs::remove_file(e.path()));
        }
    }
    // The sky map goes with the splats it was trained with: a result without one leaves none behind.
    let sky = out.join(rec::SKY_FILE);
    if partial.join(rec::SKY_FILE).exists() {
        retry(|| std::fs::rename(partial.join(rec::SKY_FILE), &sky)).with_context(|| tr!("{path} is open in another program: close it and reconstruct again", path = sky.display()))?;
    } else if sky.exists() {
        retry(|| std::fs::remove_file(&sky)).with_context(|| tr!("{path} is open in another program: close it and reconstruct again", path = sky.display()))?;
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
/// (orbiters_reconstruct::web), started from the scene's centre, without the splats its edits hide or delete.
fn web_preview(args: &[String]) -> anyhow::Result<()> {
    let (Some(ply_path), Some(out)) = (args.first(), args.get(1)) else { bail!("usage: orbiters-reconstruct --web-preview <splats.ply> <out.splat> [--max N]") };
    let max = match args.get(2).map(String::as_str) {
        Some("--max") => args.get(3).context("--max N")?.parse()?,
        _ => orbiters_reconstruct::web::DEFAULT_MAX,
    };
    // Without what the 3D view hid or deleted (orbiters_reconstruct::edit, the edit beside the PLY).
    let (g, removed, _) = orbiters_reconstruct::edit::read_edited(std::path::Path::new(ply_path))?;
    let n = g.len().max(1) as f32;
    let centre = g.means.iter().fold([0.0f32; 3], |a, m| [a[0] + m[0] / n, a[1] + m[1] / n, a[2] + m[2] / n]);
    let web = orbiters_reconstruct::web::splat_file(&g, max, centre);
    std::fs::write(out, &web.bytes)?;
    println!("{} of {} splats ({removed} left out by its edits), {:.1} MB", web.count, g.len(), web.bytes.len() as f64 / 1e6);
    Ok(())
}

/// `orbiters-reconstruct --vrchat <session folder> <out folder> [--untuned] [--before] [--rank area|contribution|seen]
/// [--refit <steps>] [--cameras <n>] [--rank-cameras <n>] [--decoded] [--tight] [--tune-steps <n>]
/// [--drawn-at <pc>,<quest>] [--refit-width <n>] [--pc-appearance]`: the reconstruction's files for VRChat worlds
/// (orbiters_reconstruct::tree_tune; the lab switches are tree_tune::ExportOptions'): pc.n4tree, quest.n4tree and
/// coarse.n4tree, without the splats its edits hide or delete (orbiters_reconstruct::edit).
fn vrchat(args: &[String]) -> anyhow::Result<()> {
    let usage = "usage: orbiters-reconstruct --vrchat <session folder> <out folder> [--untuned] [--before] [--rank area|contribution|seen] [--refit <steps>] [--cameras <n>] [--rank-cameras <n>] [--decoded] [--tight] [--tune-steps <n>] [--drawn-at <pc px>,<quest px>] [--refit-width <n>] [--pc-appearance]";
    let (Some(session), Some(out)) = (args.first(), args.get(1)) else { bail!(usage) };
    let mut options = orbiters_reconstruct::tree_tune::ExportOptions::default();
    let mut untuned = false;
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value")); // i18n: not shown (the --vrchat command-line tool, run from a terminal: the app never starts it)
        match a.as_str() {
            "--untuned" => untuned = true,
            "--before" => options = orbiters_reconstruct::tree_tune::ExportOptions::before(),
            "--rank" => options.rank = orbiters_reconstruct::tree_tune::Rank::parse(value()?)?,
            "--refit" => options.refit_steps = value()?.parse()?,
            "--cameras" => options.cameras = value()?.parse()?,
            "--rank-cameras" => options.rank_cameras = value()?.parse()?,
            "--tune-steps" => options.tune_steps = value()?.parse()?,
            "--drawn-at" => {
                let v: Vec<f32> = value()?.split(',').map(str::parse).collect::<Result<_, _>>()?;
                options.drawn_at = [v[0], *v.get(1).unwrap_or(&v[0])];
            }
            "--decoded" => options.decoded = true,
            "--refit-width" => options.refit_width = value()?.parse()?,
            "--pc-appearance" => options.pc_appearance = true,
            "--tight" => options.tight = true,
            _ => bail!("unknown argument {a}; {usage}"), // i18n: not shown (the --vrchat command-line tool, run from a terminal: the app never starts it)
        }
    }
    let session = std::path::Path::new(session);
    let (g, removed, _) = orbiters_reconstruct::edit::read_edited(&rec::result_dir(session).join(rec::PLY_FILE))?;
    if removed > 0 {
        eprintln!("{removed} splats left out by its edits");
    }
    let cameras = orbiters_reconstruct::tree_tune::session_cameras(session)?;
    let mut gpu = if untuned { None } else { Some(gpu::Gpu::new()?) };
    let start = std::time::Instant::now();
    let files = orbiters_reconstruct::tree_tune::vrchat_files_with(&g, &cameras, gpu.as_mut(), fastrand::u32(..), &options, |p| eprintln!("{:.0} %", p * 100.0))?;
    let out = std::path::Path::new(out);
    std::fs::create_dir_all(out)?;
    for (name, bytes) in [("pc.n4tree", &files.pc), ("quest.n4tree", &files.quest), ("coarse.n4tree", &files.coarse)] {
        std::fs::write(out.join(name), bytes)?;
    }
    println!(
        "{} splats, {} cameras: PC {} ({:.1} MB), Quest {} ({:.1} MB), coarse {} in {:.1} s",
        g.len(),
        cameras.len(),
        files.splats[0],
        files.pc.len() as f64 / 1e6,
        files.splats[1],
        files.quest.len() as f64 / 1e6,
        files.splats[2],
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

fn main() {
    // The release's version, for the libraries too (orbiters-version says why it is given here).
    orbiters_version::set(env!("ORBITERS_VERSION"));
    // Its messages read as the window it reports to does.
    orbiters_core::settings::Settings::speak_saved_language();
    orbiters_core::panic_log::install();
    orbiters_core::leave_with_parent();
    let all: Vec<String> = std::env::args().skip(1).collect();
    let tool: Option<fn(&[String]) -> anyhow::Result<()>> = match all.first().map(String::as_str) {
        Some("--mesh") => Some(orbiters_reconstruct::mesh::cli),
        Some("--web-preview") => Some(web_preview),
        Some("--vrchat") => Some(vrchat),
        Some("--find-faces") => Some(orbiters_reconstruct::portraits::find_faces_cli),
        Some("--export-gltf") => Some(orbiters_reconstruct::export::gltf_cli),
        Some("--export-spz") => Some(orbiters_reconstruct::export::spz_cli),
        _ => None,
    };
    if let Some(tool) = tool {
        if let Err(e) = tool(&all[1..]) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    let args = match args() {
        Ok(args) => args,
        Err(e) => {
            emit(&Event::Error { message: format!("{e:#}") });
            std::process::exit(1);
        }
    };
    let session = args.session.clone();
    match run(args) {
        Ok(Ended::Done(report)) => emit(&Event::Done { report }),
        // It said so as it left.
        Ok(Ended::Paused) => {}
        Err(e) => {
            // A run that failed goes again from the start: a checkpoint could be what made it fail.
            checkpoint::discard(&session);
            emit(&Event::Error { message: format!("{e:#}") });
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RECONSTRUCT-10: a paced run's report says how it was paced and what waiting cost; an unpaced one says nothing, and
    /// a report without the field (older runs) still reads.
    #[test]
    fn a_paced_run_reports_its_waits() {
        use orbiters_reconstruct::governor::Tally;
        use std::time::Duration;
        assert_eq!(pacing_report(&Tally::default()), None);
        let t = Tally { waited: Duration::from_millis(9500), longest: Duration::from_secs(4), waits: 812, backoffs: 7, cut_by_pause: 1, vr: true, desktop: false };
        let p = pacing_report(&t).unwrap();
        assert_eq!((p.mode.as_str(), p.waits, p.backoffs, p.cut_by_pause), ("vr", 812, 7, 1));
        assert!((p.waited_s - 9.5).abs() < 1e-3 && (p.longest_wait_s - 4.0).abs() < 1e-3);
        assert_eq!(pacing_report(&Tally { desktop: true, ..t }).unwrap().mode, "vr+desktop");
        let report = rec::Report { pacing: Some(p.clone()), ..Default::default() };
        let back: rec::Report = serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(back.pacing, Some(p));
        let unpaced = serde_json::to_string(&rec::Report::default()).unwrap();
        assert!(!unpaced.contains("pacing"), "{unpaced}");
        assert_eq!(serde_json::from_str::<rec::Report>(r#"{"gaussians":3}"#).unwrap().pacing, None);
    }

    /// RECONSTRUCT-14a: every file of a finished result, held-out renders included, is flushed before the swap; a folder
    /// that is not there fails (the swap then never runs, the previous result stays).
    #[test]
    fn a_result_is_flushed_before_it_replaces_the_previous_one() {
        let dir = session("flush");
        let partial = dir.join(rec::PARTIAL_DIR);
        std::fs::create_dir_all(partial.join(rec::HELD_OUT_DIR)).unwrap();
        std::fs::write(partial.join(rec::PLY_FILE), b"ply").unwrap();
        std::fs::write(partial.join(rec::HELD_OUT_DIR).join("000004.qoi"), b"qoi").unwrap();
        flush_all(&partial).unwrap();
        assert_eq!(std::fs::read(partial.join(rec::PLY_FILE)).unwrap(), b"ply", "flushing changes nothing");
        assert!(flush_all(&dir.join("not-there")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

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

    /// The previous splats' edits and the worker's picture kept aside for them go with them, whether the folder moves
    /// whole or file by file (a viewer holding a render open): a new result never meets an old result's edits.
    #[cfg(windows)]
    #[test]
    fn the_splat_edits_go_with_their_splats() {
        use std::os::windows::fs::OpenOptionsExt;
        const SHARE_READ: u32 = 1;
        let dir = session("edits");
        let trained = rec::trained_preview_file(0x1234);
        for held_open in [false, true] {
            result(&dir, rec::DIR, "old", "r0");
            std::fs::write(rec::dir(&dir).join(rec::EDIT_FILE), "edits of old").unwrap();
            std::fs::write(rec::dir(&dir).join(&trained), "old's own picture").unwrap();
            result(&dir, rec::PARTIAL_DIR, "new", "r1");
            let held = held_open.then(|| std::fs::OpenOptions::new().read(true).share_mode(SHARE_READ).open(rec::dir(&dir).join(rec::PREVIEW_FILE)).unwrap());
            replace(&dir, &dir.join(rec::PARTIAL_DIR)).expect("replaced");
            drop(held);
            assert_eq!(read(&dir, rec::PLY_FILE), "new");
            assert!(!rec::dir(&dir).join(rec::EDIT_FILE).exists(), "held open: {held_open}");
            assert!(!rec::dir(&dir).join(&trained).exists(), "held open: {held_open}");
            recover(&dir);
        }
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
