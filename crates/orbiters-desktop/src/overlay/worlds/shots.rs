//! `orbiters-desktop --world-view <out> <capture> [--sky <sky.jpg>] [--panel-only]`: the 3D worlds overlay checked
//! without a headset. `<capture>` is a PC tree (a slot's `pc`) or a web `.splat`.
//!
//! - `eye-left.png`, `eye-right.png`: each eye's picture as SteamVR composes it from the cube's faces (each pixel's
//!   ray through the face it leaves the cube by, sampled bilinearly from that face's half), standing at the capture's
//!   start view, an Index-like field of view; `stereo.png` both side by side; `direct-*.png` the same views drawn
//!   straight by the renderer. Their PSNR says the faces' maths and the compositing agree.
//! - `turned-*.png`: the head turned (yaw 35°, pitch −20°) with the faces left as they were drawn: SteamVR's
//!   reprojection of world-locked quads, against a direct render of the turned view. `moved-<mm>.png`: the eye moved
//!   with stale faces (the parallax a late batch shows).
//! - `faces/<face>.png`: each face's side-by-side texture.
//! - `report.json`: the PSNRs, the time a face (both eyes) takes at each quality, the faces an Index-like eye sees at
//!   several head poses, and whether the D3D11 side reads exactly what the D3D12 side drew into a shared texture.
//! - `panel-<page>.png`: the panel's pages with stand-in captures ([`super::panel`]).

use super::catalog::{self, Capture};
use super::render::{self, FaceRenderer};
use super::stereo::{self, Cube, Placement, PlayCamera, StartView, V3};
use super::view::{self, Job, Quality, Work, HALF_M, MARGIN, WIDE_MARGIN};
use anyhow::{bail, Context as _};
use orbiters_openvr::Matrix34;
use serde_json::json;
use std::path::{Path, PathBuf};

/// An Index-like headset: each eye's tangents (left, right, top, bottom), its picture, the lenses' distance.
const LEFT_TANGENTS: [f32; 4] = [-1.39, 1.24, -1.47, 1.45];
const EYE_PIXELS: (u32, u32) = (1440, 1600);
const IPD_M: f32 = 0.064;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out = PathBuf::from(args.first().context("--world-view <out> <capture> [--sky <sky.jpg>]")?);
    std::fs::create_dir_all(&out)?;
    let panel_only = args.iter().any(|a| a == "--panel-only");
    // In any language, and where its text no longer fits (as `--screenshot` has them): `--language <tag>`, `--text-report <file>`.
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    if let Some(tag) = option("--language") {
        let language = orbiters_i18n::normalize(tag).with_context(|| format!("{tag} is not a language of the website"))?;
        orbiters_i18n::pin_language(language);
        orbiters_ui::fonts::prefer_language(language.tag());
    }
    let report = option("--text-report").map(PathBuf::from);
    if report.is_some() {
        orbiters_ui::text_audit::turn_on();
    }
    super::panel::shots(&out)?;
    super::small_hint::shots(&out)?;
    if let Some(report) = &report {
        std::fs::write(report, orbiters_ui::text_audit::report()).with_context(|| format!("writing {}", report.display()))?;
    }
    if panel_only {
        return Ok(());
    }
    let file = PathBuf::from(args.get(1).context("--world-view <out> <capture>: a PC tree or a .splat")?);
    let sky = args.iter().position(|a| a == "--sky").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let start = args.iter().position(|a| a == "--view").and_then(|i| args.get(i + 1)).map(|v| parse_view(v)).transpose()?;
    let capture = load(&file, sky.as_deref(), start)?;
    let independent = args.iter().any(|a| a == "--independent-eye-sizes");
    let tangents = if args.iter().any(|a| a == "--asymmetric") { [LEFT_TANGENTS, [-1.239, 1.401, -1.471, 1.455]] } else { [LEFT_TANGENTS, right_tangents()] };
    eprintln!("{} splats", capture.gaussians.len());
    if let Some(seconds) = args.iter().position(|a| a == "--load").and_then(|i| args.get(i + 1)).map(|s| s.parse::<f64>()).transpose()? {
        let quality = match args.iter().position(|a| a == "--quality").and_then(|i| args.get(i + 1)).map(String::as_str) {
            Some("light") => Quality::Performance,
            Some("sharp") => Quality::High,
            _ => Quality::Balanced,
        };
        let feedback = args.iter().position(|a| a == "--timing-feed").and_then(|i| args.get(i + 1)).map(Path::new);
        return load_run(&out, &capture, seconds, args.iter().any(|a| a == "--still"), quality, tangents, independent, feedback);
    }
    let (adapter, device, queue) = render::device(None)?;
    eprintln!("on {}", adapter.get_info().name);
    let mut renderer = FaceRenderer::new(device.clone(), queue.clone());
    renderer.load(&capture)?;
    let mut report = serde_json::Map::new();
    report.insert("adapter".into(), json!(adapter.get_info().name));
    report.insert("splats".into(), json!(capture.gaussians.len()));

    // The person stands where the capture starts, looking its way.
    let head = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.62], [0.0, 0.0, 1.0, 0.0]] };
    let placement = Placement::at(&capture.view, &head);
    let views = eye_views(&head);
    let eyes = [stereo::position(&views[0]), stereo::position(&views[1])];
    let pixels = Quality::Balanced.pixels();
    let cube = Cube { center: stereo::scale(stereo::add(eyes[0], eyes[1]), 0.5), half: HALF_M, pixels, margin: MARGIN };
    let count = capture.gaussians.len();

    // The six faces, both eyes.
    let faces: Vec<wgpu_face::Face> = (0..6)
        .map(|f| -> anyhow::Result<wgpu_face::Face> {
            let texture = render::face_texture(&device, cube.width());
            for eye in 0..2 {
                renderer.render(&cube, &placement, eyes[eye], eye, f, count, &texture)?;
            }
            Ok(wgpu_face::Face { width: cube.width(), rgba: render::read_texture(&device, &queue, &texture)? })
        })
        .collect::<anyhow::Result<_>>()?;
    std::fs::create_dir_all(out.join("faces"))?;
    for (f, face) in faces.iter().enumerate() {
        write_png(&out.join("faces").join(format!("{f}-{}.png", face_name(f))), 2 * face.width, face.width, &face.rgba)?;
    }

    let mut psnr = serde_json::Map::new();
    let (w, h) = EYE_PIXELS;
    let mut both = [Vec::new(), Vec::new()];
    for e in 0..2 {
        let composed = compose(&cube, &faces, &views[e], tangents[e], e, w, h);
        let direct = renderer.render_view(&stereo::eye_camera(&views[e], tangents[e], w, h, &placement), count)?;
        let side = ["left", "right"][e];
        write_png(&out.join(format!("eye-{side}.png")), w, h, &composed)?;
        write_png(&out.join(format!("direct-{side}.png")), w, h, &direct)?;
        psnr.insert(format!("eye-{side}"), json!(round2(psnr_of(&composed, &direct))));
        both[e] = composed;
    }
    write_png(&out.join("stereo.png"), 2 * w, h, &side_by_side(&both[0], &both[1], w, h))?;

    // Turned: the same faces, the head turned (about the eye: the faces need no new pictures for a turn).
    for (name, yaw, pitch) in [("turned", 35f32.to_radians(), -20f32.to_radians()), ("looking-down", 0.0, -70f32.to_radians())] {
        let turned = turn(&views[0], yaw, pitch);
        let composed = compose(&cube, &faces, &turned, tangents[0], 0, w, h);
        let direct = renderer.render_view(&stereo::eye_camera(&turned, tangents[0], w, h, &placement), count)?;
        write_png(&out.join(format!("{name}-left.png")), w, h, &composed)?;
        write_png(&out.join(format!("{name}-direct-left.png")), w, h, &direct)?;
        psnr.insert(format!("{name}-left"), json!(round2(psnr_of(&composed, &direct))));
    }
    // Moved: the eye moved sideways and forward with the faces drawn for where it was.
    for mm in [2.0f32, 5.0, 20.0, 50.0] {
        let mut moved = views[0];
        let step = stereo::add(stereo::scale(stereo::axis(&moved, 0), mm / 1000.0 * 0.8), stereo::scale(stereo::axis(&moved, 2), -mm / 1000.0 * 0.6));
        for (i, s) in step.iter().enumerate() {
            moved.m[i][3] += s;
        }
        let composed = compose(&cube, &faces, &moved, tangents[0], 0, w, h);
        let direct = renderer.render_view(&stereo::eye_camera(&moved, tangents[0], w, h, &placement), count)?;
        if mm == 20.0 {
            write_png(&out.join("moved-20mm-left.png"), w, h, &composed)?;
        }
        psnr.insert(format!("moved-{mm}mm-left"), json!(round2(psnr_of(&composed, &direct))));
    }
    // A batch as the overlay draws them once the cube is up: each eye's wide view, resampled into the faces it covers,
    // after the head turned and moved a little; against direct renders of the new views.
    let turned_views = [turn(&views[0], 25f32.to_radians(), -10f32.to_radians()), turn(&views[1], 25f32.to_radians(), -10f32.to_radians())];
    let moved_views = turned_views.map(|mut v| {
        v.m[0][3] += 0.012;
        v.m[2][3] -= 0.008;
        v
    });
    let new_eyes = [stereo::position(&moved_views[0]), stereo::position(&moved_views[1])];
    let targets: Vec<eframe::wgpu::Texture> = (0..12).map(|_| render::face_texture(&device, cube.width())).collect();
    let target_refs: Vec<&eframe::wgpu::Texture> = targets.iter().collect();
    // The cube first, drawn whole from where the person stood (as the overlay does for a new cube).
    let whole = Job { id: 1, cube, cube_index: 0, placement, eyes, work: Work::Faces((0..6).map(|f| (f, 0)).collect()), count, clip: None, wait: 0 };
    view::draw(&mut renderer, &whole, &target_refs)?;
    let cameras = wide_cameras(&moved_views, tangents, Quality::Balanced, independent);
    let covered: Vec<(usize, usize)> = (0..6).filter(|&f| cameras.iter().any(|c| cube.face_in(f, c))).map(|f| (f, 1)).collect();
    let batch = Job { id: 2, cube, cube_index: 0, placement, eyes: new_eyes, work: Work::Wide { cameras, faces: covered.clone() }, count, clip: None, wait: 0 };
    view::draw(&mut renderer, &batch, &target_refs)?;
    let after: Vec<wgpu_face::Face> = (0..6)
        .map(|f| -> anyhow::Result<wgpu_face::Face> {
            let slot = if covered.iter().any(|(c, _)| *c == f) { 1 } else { 0 };
            Ok(wgpu_face::Face { width: cube.width(), rgba: render::read_texture(&device, &queue, &targets[f * 2 + slot])? })
        })
        .collect::<anyhow::Result<_>>()?;
    for e in 0..2 {
        let composed = compose(&cube, &after, &moved_views[e], tangents[e], e, w, h);
        let direct = renderer.render_view(&stereo::eye_camera(&moved_views[e], tangents[e], w, h, &placement), count)?;
        let side = ["left", "right"][e];
        write_png(&out.join(format!("wide-batch-{side}.png")), w, h, &composed)?;
        psnr.insert(format!("wide-batch-{side}"), json!(round2(psnr_of(&composed, &direct))));
    }
    report.insert("wide_view".into(), json!({ "pixels": [cameras[0].width, cameras[0].height], "faces_covered": covered.len() }));
    // The graphics memory this process holds now, a capture shown at Balanced: the splats and the renderer's buffers,
    // the faces' pictures, twelve face textures, the wide views.
    #[cfg(windows)]
    {
        // SAFETY: a read-only query of the adapter's DXGI object.
        let used = unsafe {
            adapter.as_hal::<eframe::wgpu::hal::api::Dx12>().and_then(|a| {
                let mut info = windows::Win32::Graphics::Dxgi::DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                a.as_raw().QueryVideoMemoryInfo(0, windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).ok().map(|()| info.CurrentUsage)
            })
        };
        report.insert("process_gpu_memory_mb".into(), json!(used.map(|b| b / (1 << 20))));
    }
    report.insert("psnr_db".into(), serde_json::Value::Object(psnr));

    // What a face (both eyes) costs, at each quality, after a warm-up.
    let mut timing = serde_json::Map::new();
    for quality in Quality::ALL {
        let cube = Cube { pixels: quality.pixels(), ..cube };
        let texture = render::face_texture(&device, cube.width());
        let mut ms = Vec::new();
        for round in 0..13 {
            let f = round % 6;
            let (drawn, t) = render::timed(|| -> anyhow::Result<()> {
                for eye in 0..2 {
                    renderer.render(&cube, &placement, eyes[eye], eye, f, count, &texture)?;
                }
                renderer.finish().map(|_| ())
            });
            drawn?;
            if round >= 1 {
                ms.push(t);
            }
        }
        ms.sort_by(f64::total_cmp);
        timing.insert(
            quality.label().to_lowercase(),
            json!({ "pixels_per_eye": cube.width(), "face_both_eyes_ms_median": round2(ms[ms.len() / 2]), "face_both_eyes_ms_max": round2(*ms.last().unwrap()) }),
        );
    }
    report.insert("timing".into(), serde_json::Value::Object(timing));

    // What a wide batch costs (both eyes' wide views, resampled into the faces they cover), at each quality.
    let mut batches = serde_json::Map::new();
    for quality in Quality::ALL {
        let cube = Cube { pixels: quality.pixels(), ..cube };
        let targets: Vec<eframe::wgpu::Texture> = (0..12).map(|_| render::face_texture(&device, cube.width())).collect();
        let target_refs: Vec<&eframe::wgpu::Texture> = targets.iter().collect();
        let mut ms = Vec::new();
        let mut pixels = 0;
        for round in 0..13 {
            let turned = [0, 1].map(|e| turn(&views[e], (round as f32 * 3.0).to_radians(), 0.0));
            let cameras = wide_cameras(&turned, tangents, quality, independent);
            pixels = cameras[0].width * cameras[0].height;
            let faces: Vec<(usize, usize)> = (0..6).filter(|&f| cameras.iter().any(|c| cube.face_in(f, c))).map(|f| (f, round % 2)).collect();
            let job = Job { id: round as u64, cube, cube_index: 0, placement, eyes, work: Work::Wide { cameras, faces }, count, clip: None, wait: 0 };
            let (drawn, t) = render::timed(|| view::draw(&mut renderer, &job, &target_refs));
            drawn?;
            if round >= 1 {
                ms.push(t);
            }
        }
        ms.sort_by(f64::total_cmp);
        batches.insert(
            quality.label().to_lowercase(),
            json!({ "wide_pixels_per_eye": pixels, "batch_ms_median": round2(ms[ms.len() / 2]), "batch_ms_max": round2(*ms.last().unwrap()) }),
        );
    }
    report.insert("wide_batch".into(), serde_json::Value::Object(batches));

    // Time the visible update immediately after a maintenance face (not just consecutive wide views). Keep the
    // face itself outside this measurement: it exposes the buffer/binding churn paid at motion onset.
    let mut switched = Vec::new();
    let mut steady = Vec::new();
    for round in 0..24 {
        renderer.render_master(&cube, &placement, eyes[0], 0, 2, count)?;
        renderer.render_master(&cube, &placement, eyes[1], 1, 2, count)?;
        renderer.finish()?;
        for times in [&mut switched, &mut steady] {
            let (drawn, ms) = render::timed(|| view::draw(&mut renderer, &batch, &target_refs));
            drawn?;
            if round >= 4 {
                times.push(ms);
            }
        }
    }
    switched.sort_by(f64::total_cmp);
    steady.sort_by(f64::total_cmp);
    report.insert(
        "maintenance_return".into(),
        json!({
            "wide_after_face_wall_ms_p50": round2(switched[switched.len()/2]),
            "steady_wide_wall_ms_p50": round2(steady[steady.len()/2]),
            "independent_eye_sizes": independent, "tangents": tangents
        }),
    );

    // What a view's time follows: its splats (one eye, the ahead face), and a whole eye's view drawn directly.
    let mut scaling = serde_json::Map::new();
    let texture = render::face_texture(&device, cube.width());
    for n in [50_000usize, 200_000, count] {
        let mut ms = Vec::new();
        for _ in 0..7 {
            let (drawn, t) = render::timed(|| -> anyhow::Result<()> {
                renderer.render(&cube, &placement, eyes[0], 0, 0, n, &texture)?;
                renderer.finish().map(|_| ())
            });
            drawn?;
            ms.push(t);
        }
        ms.sort_by(f64::total_cmp);
        scaling.insert(format!("ahead_one_eye_{n}_splats_ms"), json!(round2(ms[3])));
    }
    let eye_cam = stereo::eye_camera(&views[0], tangents[0], w, h, &placement);
    let mut ms = Vec::new();
    for _ in 0..7 {
        let (drawn, t) = render::timed(|| renderer.render_view(&eye_cam, count));
        drawn?;
        ms.push(t);
    }
    ms.sort_by(f64::total_cmp);
    scaling.insert("direct_eye_1440x1600_with_readback_ms".into(), json!(round2(ms[3])));
    report.insert("scaling".into(), serde_json::Value::Object(scaling));

    // The faces an eye sees with the turn margin, looking ahead, sideways, up and down.
    let mut seen = serde_json::Map::new();
    for (name, yaw, pitch) in
        [("ahead", 0f32, 0f32), ("ahead-level-30-right", -30.0, 0.0), ("corner-45", 45.0, 0.0), ("up-60", 0.0, 60.0), ("down-50", 0.0, -50.0)]
    {
        let v = turn(&views[0], yaw.to_radians(), pitch.to_radians());
        let s = cube.faces_seen(&v, LEFT_TANGENTS, super::view::TURN_MARGIN);
        let s0 = cube.faces_seen(&v, LEFT_TANGENTS, 0.0);
        seen.insert(name.into(), json!({ "with_margin": s.iter().filter(|b| **b).count(), "without": s0.iter().filter(|b| **b).count() }));
    }
    report.insert("faces_seen".into(), serde_json::Value::Object(seen));

    #[cfg(windows)]
    report.insert("shared_texture".into(), shared_check(&adapter, &device, &queue, &mut renderer, &cube, &placement, eyes, count)?);

    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&serde_json::Value::Object(report))?)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

/// Draws one face into a shared texture and reads it on a D3D11 device of the same adapter (the overlay's side):
/// the pixels must be the ones drawn into a plain texture.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn shared_check(
    adapter: &eframe::wgpu::Adapter,
    device: &eframe::wgpu::Device,
    queue: &eframe::wgpu::Queue,
    renderer: &mut FaceRenderer,
    cube: &Cube,
    placement: &Placement,
    eyes: [V3; 2],
    count: usize,
) -> anyhow::Result<serde_json::Value> {
    let luid = render::adapter_luid(adapter).context("no adapter LUID")?;
    let mut overlay = super::interop::OverlaySide::new(luid)?;
    let (side, handles) = super::interop::RenderSide::new(device, 2, cube.width())?;
    overlay.open(&handles)?;
    let plain = render::face_texture(device, cube.width());
    for eye in 0..2 {
        renderer.render(cube, placement, eyes[eye], eye, 0, count, &plain)?;
    }
    // A first picture, handed over, then a second into the same texture after the fence.
    for eye in 0..2 {
        renderer.render(cube, placement, eyes[eye], eye, 1, count, &side.textures[0])?;
    }
    let _ = renderer.finish()?;
    let wait = overlay.handed_over();
    side.wait(queue, wait)?;
    for eye in 0..2 {
        renderer.render(cube, placement, eyes[eye], eye, 0, count, &side.textures[0])?;
    }
    let _ = renderer.finish()?;
    let expected = render::read_texture(device, queue, &plain)?;
    let got = overlay.read(0)?;
    let differing = expected.iter().zip(&got).filter(|(a, b)| a != b).count();
    Ok(json!({ "bytes": got.len(), "differing_bytes": differing, "same": differing == 0 && expected.len() == got.len() }))
}

/// `--load <seconds> [--still]`: the overlay's batches for that long, as a person would make them in the headset (the
/// head turning ±35° and swaying ±4 cm, about one cycle every four seconds; `--still`: a head that does not move), at
/// most the quality's rate a second, using production motion gates and wall-cost admission (no compositor feedback).
/// Run beside a stand-in game
/// (orbiters-reconstruct's `framegame` example) to see what the overlay costs a game on the same GPU. Writes
/// `load.json`: batches drawn, render wall time (including CPU, queueing and GPU waits), and its elapsed-time share.
fn load_run(
    out: &Path,
    capture: &Capture,
    seconds: f64,
    still: bool,
    quality: Quality,
    tangents: [[f32; 4]; 2],
    independent: bool,
    feedback: Option<&Path>,
) -> anyhow::Result<()> {
    let (_, device, queue) = render::device(None)?;
    let mut renderer = FaceRenderer::new(device.clone(), queue.clone());
    renderer.load(capture)?;
    let head0 = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.62], [0.0, 0.0, 1.0, 0.0]] };
    let placement = Placement::at(&capture.view, &head0);
    let views0 = eye_views(&head0);
    let eyes0 = [stereo::position(&views0[0]), stereo::position(&views0[1])];
    let pixels = quality.pixels();
    let cube = Cube { center: stereo::scale(stereo::add(eyes0[0], eyes0[1]), 0.5), half: HALF_M, pixels, margin: MARGIN };
    let targets: Vec<eframe::wgpu::Texture> = (0..12).map(|_| render::face_texture(&device, cube.width())).collect();
    let target_refs: Vec<&eframe::wgpu::Texture> = targets.iter().collect();
    let count = capture.gaussians.len();
    view::draw(
        &mut renderer,
        &Job { id: 0, cube, cube_index: 0, placement, eyes: eyes0, work: Work::Faces((0..6).map(|f| (f, 0)).collect()), count, clip: None, wait: 0 },
        &target_refs,
    )?;
    let start = std::time::Instant::now();
    let (mut batches, mut busy, mut last) = (0u64, 0f64, None::<([V3; 2], V3)>);
    let mut times = Vec::new();
    let mut pacer = super::pacing::Pacer::default();
    let mut feed = feedback.map(TimingFeed::new).transpose()?;
    let mut was_still = true;
    let mut settling = false;
    while start.elapsed().as_secs_f64() < seconds {
        let now = std::time::Instant::now();
        if let Some(feed) = &mut feed {
            feed.observe(&mut pacer)?;
        }
        pacer.configure(now, quality.rate());
        if let Some(wait) = pacer.wake_after(now) {
            std::thread::sleep(wait);
        }
        let t = start.elapsed().as_secs_f32();
        let (yaw, sway) =
            if still { (0.0, 0.0) } else { ((t * std::f32::consts::TAU / 4.0).sin() * 35f32.to_radians(), (t * std::f32::consts::TAU / 3.3).sin() * 0.04) };
        let mut head = turn(&head0, yaw, 0.0);
        head.m[0][3] += sway;
        let views = eye_views(&head);
        let eyes = [stereo::position(&views[0]), stereo::position(&views[1])];
        let looking = stereo::scale(stereo::axis(&head, 2), -1.0);
        let speed = if still { 0.0 } else { (t * std::f32::consts::TAU / 3.3).cos().abs() * 0.04 * std::f32::consts::TAU / 3.3 };
        let angular = if still { 0.0 } else { (t * std::f32::consts::TAU / 4.0).cos().abs() * 35f32.to_radians() * std::f32::consts::TAU / 4.0 };
        let moving = speed > 0.02 || angular > 10f32.to_radians();
        let quiet = speed < 0.001 && angular < 0.5f32.to_radians();
        settling |= !was_still && quiet;
        was_still = quiet;
        let due = last.is_none_or(|(from, look)| {
            (0..2).any(|e| super::pacing::translation_due(stereo::length(stereo::sub(eyes[e], from[e])), moving, quiet, settling))
                || stereo::dot(look, looking).clamp(-1.0, 1.0).acos() > view::TURNED
        });
        if !due {
            std::thread::sleep(std::time::Duration::from_millis(11));
            continue;
        }
        settling = false;
        last = Some((eyes, looking));
        let cameras = wide_cameras(&views, tangents, quality, independent);
        let faces: Vec<(usize, usize)> = (0..6).filter(|&f| cameras.iter().any(|c| cube.face_in(f, c))).map(|f| (f, (batches % 2) as usize)).collect();
        let job = Job { id: batches, cube, cube_index: 0, placement, eyes, work: Work::Wide { cameras, faces }, count, clip: None, wait: 0 };
        let asked = std::time::Instant::now();
        pacer.admitted(asked, batches);
        let (drawn, ms) = render::timed(|| view::draw(&mut renderer, &job, &target_refs));
        drawn?;
        if let Some(feed) = &mut feed {
            feed.observe(&mut pacer)?;
        }
        pacer.completed(batches, super::pacing::Kind::Wide, asked.elapsed());
        batches += 1;
        busy += ms;
        times.push(ms);
    }
    times.sort_by(f64::total_cmp);
    let elapsed = start.elapsed().as_secs_f64();
    let report = json!({
        "seconds": round2(elapsed),
        "still": still,
        "quality": quality.label(),
        "batches": batches,
        "batches_per_second": round2(batches as f64 / elapsed),
        "batch_ms_median": times.get(times.len() / 2).copied().map(round2),
        "batch_ms_p95": times.get(times.len() * 95 / 100).copied().map(round2),
        "render_wall_share": round2(busy / 1000.0 / elapsed),
        "synthetic_timing_feed": feedback.is_some(), "pacer_level": pacer.level, "late_attributed": pacer.attributed,
        "pacer_interval_ms": round2(pacer.interval.as_secs_f64() * 1000.0),
        "measurement": "real GPU, production motion/duty gates; synchronous renderer, optional synthetic 90 Hz feedback, no SteamVR submission",
    });
    std::fs::write(out.join("load.json"), serde_json::to_string_pretty(&report)?)?;
    eprintln!("{report}");
    Ok(())
}

/// Experiment-only tail of gpu-lab's 90 Hz deadline CSV. No filesystem access is added to the production pacer.
struct TimingFeed {
    file: std::fs::File,
    tail: String,
    frames: Vec<orbiters_openvr::compositor::FrameTiming>,
    index: u32,
}

impl TimingFeed {
    fn new(path: &Path) -> anyhow::Result<Self> {
        Ok(Self { file: std::fs::File::open(path).context("open gpu-lab timing feed")?, tail: String::new(), frames: Vec::new(), index: 0 })
    }

    fn observe(&mut self, pacer: &mut super::pacing::Pacer) -> anyhow::Result<()> {
        use std::io::Read;
        self.file.read_to_string(&mut self.tail)?;
        if let Some(end) = self.tail.rfind('\n') {
            for line in self.tail[..end].lines() {
                if let Some(ready) = line.split(',').nth(3).and_then(|s| s.parse::<f64>().ok()) {
                    self.index = self.index.wrapping_add(1);
                    self.frames.push(orbiters_openvr::compositor::FrameTiming {
                        frame_index: self.index,
                        num_dropped_frames: u32::from(ready > 1000.0 / 90.0),
                        ..Default::default()
                    });
                }
            }
            self.tail.drain(..=end);
            let excess = self.frames.len().saturating_sub(64);
            self.frames.drain(..excess);
        }
        pacer.observe(std::time::Instant::now(), &self.frames, true);
        Ok(())
    }
}

fn wide_cameras(views: &[Matrix34; 2], tangents: [[f32; 4]; 2], quality: Quality, independent: bool) -> [PlayCamera; 2] {
    if independent {
        [0, 1].map(|e| PlayCamera::wide(&views[e], tangents[e], WIDE_MARGIN, quality.wide_per_tan(), 3072))
    } else {
        PlayCamera::wide_pair(views, tangents, WIDE_MARGIN, quality.wide_per_tan(), 3072)
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn face_name(f: usize) -> &'static str {
    ["ahead", "right", "behind", "left", "up", "down"][f]
}

fn right_tangents() -> [f32; 4] {
    // The right eye mirrors the left one.
    [-LEFT_TANGENTS[1], -LEFT_TANGENTS[0], LEFT_TANGENTS[2], LEFT_TANGENTS[3]]
}

/// Each eye's pose: half the lens distance to each side of the head.
fn eye_views(head: &Matrix34) -> [Matrix34; 2] {
    [-1.0f32, 1.0].map(|side| {
        let mut eye = Matrix34::IDENTITY;
        eye.m[0][3] = side * IPD_M / 2.0;
        stereo::compose(head, &eye)
    })
}

/// `view` turned `yaw` (left, about +Y) then `pitch` (up, about its own +X), about its own position.
fn turn(view: &Matrix34, yaw: f32, pitch: f32) -> Matrix34 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let yaw_m = Matrix34 { m: [[cy, 0.0, sy, 0.0], [0.0, 1.0, 0.0, 0.0], [-sy, 0.0, cy, 0.0]] };
    let pitch_m = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, cp, -sp, 0.0], [0.0, sp, cp, 0.0]] };
    let mut rotated = stereo::compose(
        &stereo::compose(
            &yaw_m,
            &Matrix34 {
                m: [
                    [view.m[0][0], view.m[0][1], view.m[0][2], 0.0],
                    [view.m[1][0], view.m[1][1], view.m[1][2], 0.0],
                    [view.m[2][0], view.m[2][1], view.m[2][2], 0.0],
                ],
            },
        ),
        &pitch_m,
    );
    for i in 0..3 {
        rotated.m[i][3] = view.m[i][3];
    }
    rotated
}

mod wgpu_face {
    /// A face's side-by-side texture read back.
    pub struct Face {
        pub width: u32,
        pub rgba: Vec<u8>,
    }
}

/// Eye `eye`'s picture as SteamVR composes the cube: each pixel's ray through the face it leaves by, bilinear.
fn compose(cube: &Cube, faces: &[wgpu_face::Face], view: &Matrix34, tangents: [f32; 4], eye: usize, w: u32, h: u32) -> Vec<u8> {
    let at = stereo::position(view);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let dir = stereo::eye_ray(view, tangents, w, h, x as f32, y as f32);
            let f = cube.face_hit(at, dir);
            let Some([u, v]) = cube.pixel_hit(f, at, dir) else { continue };
            let face = &faces[f];
            let px = sample(face, eye, u, v);
            out[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize].copy_from_slice(&px);
        }
    }
    out
}

fn sample(face: &wgpu_face::Face, eye: usize, u: f32, v: f32) -> [u8; 4] {
    let w = face.width as i32;
    let (x0, y0) = (u.floor() as i32, v.floor() as i32);
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let texel = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, w - 1), y.clamp(0, w - 1));
        let i = ((y * 2 * w + eye as i32 * w + x) * 4) as usize;
        [face.rgba[i] as f32, face.rgba[i + 1] as f32, face.rgba[i + 2] as f32]
    };
    let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
    let mut px = [0u8, 0, 0, 255];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        px[k] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    px
}

fn side_by_side(left: &[u8], right: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(left.len() * 2);
    for y in 0..h as usize {
        let row = w as usize * 4;
        out.extend_from_slice(&left[y * row..(y + 1) * row]);
        out.extend_from_slice(&right[y * row..(y + 1) * row]);
    }
    out
}

/// PSNR (dB) of RGB over pixels both pictures cover.
fn psnr_of(a: &[u8], b: &[u8]) -> f64 {
    let (mut sum, mut n) = (0f64, 0usize);
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        if p[3] == 0 {
            continue;
        }
        for k in 0..3 {
            let d = p[k] as f64 - q[k] as f64;
            sum += d * d;
        }
        n += 3;
    }
    let mse = sum / n.max(1) as f64;
    if mse <= 1e-12 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// `x,y,z,tx,ty,tz` (right-handed, as the listing's start views).
fn parse_view(v: &str) -> anyhow::Result<StartView> {
    let n: Vec<f32> = v.split(',').map(|s| s.trim().parse::<f32>()).collect::<Result<_, _>>()?;
    if n.len() != 6 {
        bail!("--view x,y,z,tx,ty,tz");
    }
    Ok(StartView { position: [n[0], n[1], n[2]], target: [n[3], n[4], n[5]], floor: None, eye_height: None })
}

/// A capture file: a PC tree or a `.splat`, with its start view (VRChat Home's when not given).
fn load(file: &Path, sky: Option<&Path>, view: Option<StartView>) -> anyhow::Result<Capture> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let g = if bytes.starts_with(b"N4N0TREE") {
        let key = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        catalog::tree_leaves(&bytes, key)?
    } else {
        orbiters_reconstruct::web::read_splat(&bytes)?
    };
    let sky = sky.map(|p| std::fs::read(p).map_err(anyhow::Error::from).and_then(|b| catalog::sky_map(&b))).transpose()?;
    let view = view.unwrap_or(StartView { position: [-0.148, 1.469, -0.350], target: [2.627, 1.046, 2.227], floor: None, eye_height: None });
    Ok(Capture::new(g, sky, view))
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    // What the text report found while this picture was painted belongs to it.
    orbiters_ui::text_audit::end_scene(&path.file_stem().unwrap_or_default().to_string_lossy());
    let file = std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(())
}
