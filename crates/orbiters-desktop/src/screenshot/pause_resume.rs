//! `--only pause-resume`: a reconstruction paused and going on, on the real view and the real worker, as a person taking one
//! 3D picture after another makes it happen: the capture itself is the only stand-in (the capture worker needs VRChat's
//! Spout stream: the state says capturing, and the controller does what starting one does). It takes a disposable
//! library in `--sessions` of two sessions without a result (hard-linked copies are enough) and the worker beside the
//! program, and fails on any broken promise: progress going back, a checkpoint left behind, a stop that did not stop, a run
//! that did not finish.
//!
//! The walk: a run under way; a capture pauses it (the window, the card, its menu, the session page and the row in
//! Visited worlds, each shot); it goes on, and pauses again and again, always ahead of where it was; VRChat holds it
//! (with the setting that says so); the app closes on it and opens again; it finishes. A second session paused and
//! stopped leaves nothing behind.

use super::capture_actions::{close_window, key, library, on, open_window};
use super::*;
use crate::controller::Shared;
use crate::pages::sessions::overall_progress;
use orbiters_core::reconstruction::{self, Stage};
use orbiters_core::settings::WhilePlaying;
use orbiters_reconstruct::checkpoint;

fn job(ctl: &Shared) -> Option<crate::state::ReconstructionJob> {
    ctl.view().reconstruction.clone()
}

/// How far the run on show got, as it shows it.
fn shown(ctl: &Shared) -> f32 {
    job(ctl).map_or(0.0, |j| overall_progress(&j))
}

fn training_past(run: &mut Run<'_>, ctl: &Shared, at: f32) -> anyhow::Result<()> {
    run.until("training under way", 600.0, |_| job(ctl).is_some_and(|j| j.stage == Stage::Training && !j.paused && !j.resuming && j.progress >= at))
}

/// A capture starts, as far as a reconstruction can tell: it waits for nothing, and says how long the worker took to
/// leave. Returns what the run shows once it stands still.
fn capture_starts(run: &mut Run<'_>, ctl: &Shared, name: &str) -> anyhow::Result<f32> {
    let before = shown(ctl);
    ctl.update(|s| s.capture.state = crate::state::CaptureState::Recording);
    let asked = Instant::now();
    ctl.capture_takes_the_gpu();
    let asking = asked.elapsed();
    anyhow::ensure!(asking < Duration::from_millis(50), "asking took {asking:?}: the capture must not wait");
    anyhow::ensure!(job(ctl).is_some_and(|j| j.paused), "paused as soon as it was asked");
    // The app's frames take the notices: looked at before the next one.
    anyhow::ensure!(ctl.take_notices().iter().any(|n| matches!(n, crate::controller::Notice::Toast { message, .. } if message.contains("continues from where it was"))), "the notice says it continues");
    if let Err(e) = run.until("the worker to leave", 60.0, |_| ctl.running_reconstruction().is_none()) {
        println!("{:?}", ctl.worker_log_lines());
        return Err(e);
    }
    println!("{name}: asked in {:.1} ms, worker gone after {:.2} s, {:.1} % kept", asking.as_secs_f32() * 1e3, asked.elapsed().as_secs_f32(), before * 100.0);
    let kept = shown(ctl);
    anyhow::ensure!(kept + 1e-3 >= before, "{name}: {before} before the pause, {kept} after");
    let dir = job(ctl).context("its job stays on show")?.session_dir;
    anyhow::ensure!(checkpoint::exists(&dir), "{name}: no checkpoint");
    anyhow::ensure!(ctl.view().reconstruct_queue.first() == Some(&dir), "{name}: first in line");
    Ok(kept)
}

/// The capture ends: the run goes on, and what it shows never falls under what it kept.
fn capture_ends(run: &mut Run<'_>, ctl: &Shared, kept: f32) -> anyhow::Result<()> {
    ctl.update(|s| s.capture.state = crate::state::CaptureState::Idle);
    ctl.next_reconstruction();
    run.until("the worker to start again", 15.0, |_| ctl.running_reconstruction().is_some())?;
    let goes_on = Instant::now();
    run.until("its next step", 300.0, |_| {
        assert!(shown(ctl) + 1e-3 >= kept, "{} shown after {kept} was kept", shown(ctl));
        job(ctl).is_none_or(|j| !j.resuming)
    })?;
    println!("  going on: at its next step after {:.2} s", goes_on.elapsed().as_secs_f32());
    Ok(())
}

pub(super) fn walk(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let sessions = controller::list_sessions(&settings.sessions_dir);
    // The one with a world: Photos & captures shows a world's captures, and a card is what the walk opens.
    let first = sessions.iter().find(|s| s.manifest.world.is_some()).context("a session of a world to reconstruct")?;
    let (main, world) = (first.dir.clone(), first.manifest.world.as_ref().and_then(|w| w.name.clone()).unwrap_or_default());
    let other = sessions.iter().find(|s| s.dir != main).context("a second session")?.dir.clone();
    let (ctl, backend, handles) = library(settings, gpu);
    {
        let mut run = Run::new(gpu, backend, scale);
        run.app.set_gpu(handles);
        let t = run.time();
        run.app.set_page(Page::Sessions, t);
        run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
        run.wait(1.0)?;
        // Only this library's world: what else this account has would push its card off the page.
        run.app.sessions_mut().gallery_mut().stage_query(&world);
        run.wait(1.0)?;

        // Under way.
        ctl.reconstruct(main.clone())?;
        run.pace = Duration::from_millis(60);
        training_past(&mut run, &ctl, 0.2)?;
        run.size = vec2(1280.0, 820.0);
        open_window(&mut run, &main)?;
        run.shoot(out, "01-running-window")?;

        // A capture: the window says paused, what was done stays.
        let kept = capture_starts(&mut run, &ctl, "first capture")?;
        run.wait(1.2)?;
        anyhow::ensure!(on(&run, "Stop reconstruction") == Some(true), "a paused run can be stopped");
        run.shoot(out, "02-paused-window")?;
        close_window(&mut run)?;
        run.size = vec2(1280.0, 1500.0);
        run.wait(1.0)?;
        run.shoot(out, "03-paused-card")?;
        run.size = vec2(1280.0, 820.0);
        run.wait(0.4)?;
        let ctx = run.ctx.clone();
        run.app.sessions_mut().open_session(&ctx, main.clone(), Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
        run.wait(1.4)?;
        run.shoot(out, "04-paused-session")?;

        // It goes on, shown going on, then ahead of where it was.
        ctl.update(|s| s.capture.state = crate::state::CaptureState::Idle);
        ctl.next_reconstruction();
        run.wait(0.2)?;
        run.shoot(out, "05-going-on-session")?;
        ctl.update(|s| s.capture.state = crate::state::CaptureState::Recording);
        run.until("the worker to reach its steps", 300.0, |_| job(&ctl).is_none_or(|j| !j.resuming))?;
        ctl.update(|s| s.capture.state = crate::state::CaptureState::Idle);
        training_past(&mut run, &ctl, 0.3)?;
        anyhow::ensure!(shown(&ctl) > kept, "ahead of what was kept");

        // One picture after another: paused and going on again and again, always ahead.
        let mut last = shown(&ctl);
        for (i, at) in [0.4f32, 0.5, 0.6].into_iter().enumerate() {
            training_past(&mut run, &ctl, at)?;
            let kept = capture_starts(&mut run, &ctl, &format!("capture {}", i + 2))?;
            anyhow::ensure!(kept >= last, "{kept} kept after {last}");
            capture_ends(&mut run, &ctl, kept)?;
            last = kept;
        }

        // VRChat holds it when the setting says so, whenever it started; it goes on once VRChat closes.
        training_past(&mut run, &ctl, 0.7)?;
        ctl.update(|s| {
            s.settings.reconstruct_while_playing = WhilePlaying::AfterVrchat;
            s.vrchat.running = true;
        });
        ctl.vrchat_changed(true);
        anyhow::ensure!(job(&ctl).is_some_and(|j| j.paused), "held for VRChat: paused");
        run.until("the worker to leave", 60.0, |_| ctl.running_reconstruction().is_none())?;
        run.wait(1.2)?;
        run.shoot(out, "06-paused-vrchat-session")?;
        key(&mut run, egui::Key::Escape)?;
        run.wait(1.0)?;
        run.size = vec2(1280.0, 1500.0);
        run.wait(1.0)?;
        run.shoot(out, "07-paused-vrchat-card")?;
        run.size = vec2(1280.0, 820.0);
        // The second session waits behind it, and its stop is shown by the second part below.
        ctl.update(|s| s.vrchat.running = false);
        ctl.vrchat_changed(false);
        run.until("the worker to start again", 15.0, |_| ctl.running_reconstruction().is_some())?;
        training_past(&mut run, &ctl, 0.75)?;
        // The app closes on it: it keeps what it has.
        let before = shown(&ctl);
        for n in ctl.take_notices() {
            println!("notice: {n:?}");
        }
        ctl.shutdown();
        anyhow::ensure!(checkpoint::exists(&main), "closing the app keeps the run");
        println!("the app closed at {:.1} %: {} MB kept", before * 100.0, std::fs::metadata(checkpoint::file(&main))?.len() >> 20);
    }

    // The app opens again: paused where it was, then on to the end.
    let (ctl, backend, handles) = library(settings, gpu);
    {
        let mut run = Run::new(gpu, backend, scale);
        run.app.set_gpu(handles);
        let t = run.time();
        run.app.set_page(Page::Sessions, t);
        run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
        run.app.sessions_mut().gallery_mut().stage_query(&world);
        // Held for the moment, to see it as it opens.
        ctl.update(|s| s.capture.state = crate::state::CaptureState::Recording);
        ctl.go_on_with_paused();
        let reopened = shown(&ctl);
        anyhow::ensure!(job(&ctl).is_some_and(|j| j.paused) && reopened > 0.7, "the app shows it paused where it was: {reopened}");
        run.pace = Duration::from_millis(60);
        run.size = vec2(1280.0, 1500.0);
        run.wait(1.4)?;
        run.shoot(out, "08-reopened-card")?;
        run.size = vec2(1280.0, 820.0);
        ctl.update(|s| s.capture.state = crate::state::CaptureState::Idle);
        ctl.next_reconstruction();
        let mut highest = reopened;
        run.until("the reconstruction to finish", 900.0, |_| {
            let now = shown(&ctl);
            if job(&ctl).is_some() {
                assert!(now + 1e-3 >= highest, "{now} after {highest}");
                highest = now;
            }
            job(&ctl).is_none()
        })?;
        run.pace = Duration::from_millis(16);
        anyhow::ensure!(!checkpoint::exists(&main), "no checkpoint left once it finished");
        let report = reconstruction::read_report(&main).context("a result")?;
        println!("finished: {} splats, {} steps, {:.1} s of training, {:?} dB", report.gaussians, report.iterations, report.seconds, report.psnr_held_out);
        anyhow::ensure!(report.gaussians > 0 && report.psnr_held_out.is_some(), "a whole result");

        // A second run: paused, then stopped. Stop means stop: nothing left, nothing goes on.
        ctl.reconstruct(other.clone())?;
        run.pace = Duration::from_millis(60);
        training_past(&mut run, &ctl, 0.1)?;
        capture_starts(&mut run, &ctl, "the second session")?;
        ctl.update(|s| s.capture.state = crate::state::CaptureState::Idle);
        anyhow::ensure!(ctl.stop_reconstruction(), "a paused run can be stopped");
        anyhow::ensure!(job(&ctl).is_none() && ctl.view().reconstruct_queue.is_empty() && !checkpoint::exists(&other), "stopped: nothing left");
        ctl.next_reconstruction();
        run.wait(1.0)?;
        anyhow::ensure!(job(&ctl).is_none() && ctl.running_reconstruction().is_none(), "stopped: nothing goes on");
        // Stopped while it runs too.
        ctl.reconstruct(other.clone())?;
        training_past(&mut run, &ctl, 0.05)?;
        capture_starts(&mut run, &ctl, "the second session again")?;
        capture_ends(&mut run, &ctl, 0.0)?;
        anyhow::ensure!(ctl.stop_reconstruction(), "a run going on can be stopped");
        run.until("the run to end", 30.0, |_| job(&ctl).is_none())?;
        anyhow::ensure!(!checkpoint::exists(&other), "stopped: its checkpoint went too");
        anyhow::ensure!(reconstruction::read_report(&other).is_none(), "and no result came of it");
        for n in ctl.take_notices() {
            println!("notice: {n:?}");
        }
        ctl.shutdown();
    }
    Ok(())
}
