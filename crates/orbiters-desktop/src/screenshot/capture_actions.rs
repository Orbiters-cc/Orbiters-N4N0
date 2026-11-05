//! What a 3D capture's window, card and row can do, on the real view and the real worker. Both walks take a disposable
//! library in `--sessions` (hard-linked copies are enough: the worker moves new files into place and nothing is written
//! into a file the original shares), and the worker beside the program.
//!
//! `--only capture-actions-select`: Select splats at the size of Enzo's window (1030 x 800), then at 1280 x 820 and
//! 1920 x 1080, on the session named `...12-14-00`. Nothing selected, a pick, the brush, a box's selection, Hide: shot at
//! each step, and where every action was drawn is printed with whether it takes clicks and whether two overlap.
//!
//! `--only capture-actions-tint`: how soon a change of the selection shows ([`tint`]).
//!
//! `--only capture-actions`: Photos & captures' window of that session: Edit top right landing in Edit in 3D; Reconstruct
//! again asking first (the splats it would bring back), Cancel changing nothing; going ahead, the run's progress in the
//! window, on the card and in its menu; the window, left open, following the new result, the edits gone; Stop leaving the
//! result as it was; a second capture waiting, with its cancel. Then the captures that cannot: `...view-only` (capture
//! files deleted) and `...no-depth`. Then a session's detail, asking first too. Then Visited worlds: its row's Reconstruct again and Stop, and the floating back
//! button at rest, scrolled and under a dialog, at the narrowest and the widest window, and that it still goes back.

use super::*;
use crate::controller::shares::{Entry, Local as OnThisComputer};
use crate::pages::splats::Viewer;
use orbiters_core::reconstruction;
use orbiters_reconstruct::edit;

fn viewer<'a>(run: &'a mut Run<'_>) -> &'a mut Viewer {
    run.app.sessions_mut().viewer_mut().expect("the 3D view is open")
}

fn counts(run: &mut Run<'_>) -> [usize; 3] {
    viewer(run).select_counts().unwrap_or_default()
}

fn button(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn click_at(run: &mut Run<'_>, at: Pos2) -> anyhow::Result<()> {
    run.frame(vec![Event::PointerMoved(at)], false)?;
    run.frame(vec![button(at, true)], false)?;
    run.frame(vec![button(at, false)], false)?;
    run.frame(Vec::new(), false)?;
    Ok(())
}

fn drag(run: &mut Run<'_>, from: Pos2, to: Pos2) -> anyhow::Result<()> {
    run.frame(vec![Event::PointerMoved(from)], false)?;
    run.frame(vec![button(from, true)], false)?;
    let start = run.time();
    loop {
        let k = ((run.time() - start) / 0.3).min(1.0) as f32;
        run.frame(vec![Event::PointerMoved(from + (to - from) * k)], false)?;
        if k >= 1.0 {
            break;
        }
    }
    run.frame(vec![button(to, false)], false)?;
    run.frame(Vec::new(), false)?;
    Ok(())
}

fn key(run: &mut Run<'_>, key: egui::Key) -> anyhow::Result<()> {
    let modifiers = Default::default();
    run.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], false)?;
    run.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }], false)?;
    Ok(())
}

/// Where each of the actions was drawn and whether it takes clicks; two that overlap are said, and fail the walk.
fn audit(run: &Run<'_>, step: &str) -> anyhow::Result<()> {
    let labels = ["Pick", "Brush", "Box", "Sphere", "Grow", "Shrink", "Hide", "Delete", "Clear", "Invert", "Select all", "Show all", "Restore all"];
    let found: Vec<(&str, Rect, bool)> = labels.iter().filter_map(|l| widgets::control(&run.ctx, l).map(|(r, on)| (*l, r, on))).collect();
    println!("[{step}] window {:?}", run.size);
    for (l, r, on) in &found {
        println!("  {l:12} x {:6.0}..{:6.0}  y {:6.0}..{:6.0}  {}", r.left(), r.right(), r.top(), r.bottom(), if *on { "takes clicks" } else { "disabled" });
    }
    let window = Rect::from_min_size(Pos2::ZERO, run.size);
    for (i, a) in found.iter().enumerate() {
        anyhow::ensure!(window.contains_rect(a.1), "{step}: {} is cut by the window", a.0);
        for b in &found[i + 1..] {
            // The segmented control's labels touch each other by a point: not an overlap.
            anyhow::ensure!(!a.1.shrink(1.0).intersects(b.1.shrink(1.0)), "{step}: {} and {} overlap", a.0, b.0);
        }
    }
    Ok(())
}

/// A shot of the 3D view as the app shows it once a change settled: this harness paints only its shots, the app paints the
/// frames after a change too, and the one right after a change may still show what was before it.
fn shoot_settled(run: &mut Run<'_>, out: &Path, name: &str) -> anyhow::Result<()> {
    for _ in 0..5 {
        run.frame(Vec::new(), true)?;
    }
    run.shoot(out, name)
}

/// One size's walk; `name` starts each shot's name.
fn select_walk(run: &mut Run<'_>, out: &Path, name: &str) -> anyhow::Result<()> {
    run.click("Select splats", 10.0)?;
    run.until("the splats ready to select", 120.0, |run| viewer(run).select_ready())?;
    run.click("Pick", 10.0)?;
    run.wait(0.5)?;
    audit(run, &format!("{name}: open, nothing selected"))?;
    anyhow::ensure!(widgets::control(&run.ctx, "Hide").is_none(), "Hide waits for a selection");
    shoot_settled(run, out, &format!("{name}-open"))?;
    let middle = Rect::from_min_max(pos2(0.0, crate::app::TITLE_H), run.size.to_pos2()).center();
    click_at(run, middle)?;
    // The ring leaving the click, on the frames right after it.
    run.shoot(out, &format!("{name}-pick-ring"))?;
    run.until("a pick", 10.0, |run| counts(run)[0] > 0)?;
    run.wait(0.8)?;
    println!("a pick selected {} splats", counts(run)[0]);
    audit(run, &format!("{name}: picked"))?;
    anyhow::ensure!(widgets::control(&run.ctx, "Hide").is_some_and(|(_, on)| on), "Hide once something is selected");
    anyhow::ensure!(widgets::control(&run.ctx, "Delete").is_some_and(|(_, on)| on), "Delete once something is selected");
    shoot_settled(run, out, &format!("{name}-picked"))?;
    run.click("Brush", 10.0)?;
    run.wait(0.4)?;
    audit(run, &format!("{name}: brush"))?;
    shoot_settled(run, out, &format!("{name}-brush"))?;
    run.click("Box", 10.0)?;
    drag(run, middle + vec2(-200.0, -120.0), middle + vec2(200.0, 40.0))?;
    run.until("a box's selection", 10.0, |run| counts(run)[0] > 1)?;
    run.wait(0.5)?;
    let boxed = counts(run)[0];
    println!("a box selected {boxed} splats");
    audit(run, &format!("{name}: boxed"))?;
    shoot_settled(run, out, &format!("{name}-boxed"))?;
    run.click("Hide", 10.0)?;
    run.until("the selection hidden", 10.0, |run| counts(run) == [0, boxed, 0])?;
    run.wait(0.5)?;
    anyhow::ensure!(widgets::control(&run.ctx, "Hide").is_none(), "nothing selected any more");
    shoot_settled(run, out, &format!("{name}-hidden"))?;
    run.click("Restore all", 10.0)?;
    run.until("everything back", 10.0, |run| counts(run) == [0, 0, 0])?;
    run.click("Done", 10.0)?;
    run.wait(0.3)?;
    Ok(())
}

/// The controller on this library, what the app talks to it through, and the offscreen GPU's handles.
fn library(settings: &Settings, gpu: &mut Offscreen) -> (crate::controller::Shared, Ctl, crate::gpu::Handles) {
    let ctl = Controller::new(settings.clone());
    let sessions = controller::list_sessions(&settings.sessions_dir);
    ctl.set_snapshot(idle_snapshot(settings, &sessions));
    let backend: Ctl = std::sync::Arc::new(crate::backend::Local::new(ctl.clone()));
    (ctl, backend, crate::gpu::Handles::of_offscreen(gpu))
}

pub(super) fn select(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let (_ctl, backend, handles) = library(settings, gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles.clone());
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    let origin = Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0));
    run.app.sessions_mut().open_where(&ctx, origin, |s| s.dir.to_string_lossy().ends_with("12-14-00"));
    let dir = run.app.sessions_mut().open_dir().with_context(|| format!("no session in {}", settings.sessions_dir.display()))?;
    edit::discard(&reconstruction::result_dir(&dir))?;
    run.wait(1.0)?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_viewer(&ctx, &handles, false);
    run.until("the 3D view's splats", 120.0, |run| run.app.sessions_mut().viewer_loaded())?;
    for (w, h, name) in [(1030.0, 800.0, "select-1030"), (1280.0, 820.0, "select-1280"), (1920.0, 1080.0, "select-1920")] {
        run.size = vec2(w, h);
        run.wait(0.6)?;
        select_walk(&mut run, out, name)?;
    }
    edit::discard(&reconstruction::result_dir(&dir))?;
    Ok(())
}

/// `--only capture-actions-tint`: how soon a change of the selection shows, with frames painted one after the other as the
/// app paints them after a change (it keeps repainting for [`crate::pages::splats`]'s REDRAW_CHECKS frames; this harness
/// otherwise paints only its shots). Ctrl+A tints every splat, Esc clears it, Hide and undo remove and bring back, over and
/// over without the camera moving. Each change paints [`FRAMES`] frames and says, for each, how far it still is from the
/// last one (mean absolute difference of the colour channels, 0 to 255), and at which frame it first came within
/// [`SETTLED`] of it.
pub(super) fn tint(gpu: &mut Offscreen, _out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    const FRAMES: usize = 6;
    const SETTLED: f32 = 0.5;
    let (_ctl, backend, handles) = library(settings, gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles.clone());
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    let origin = Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0));
    run.app.sessions_mut().open_where(&ctx, origin, |s| s.dir.to_string_lossy().ends_with("12-14-00"));
    run.wait(1.0)?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_viewer(&ctx, &handles, false);
    run.until("the 3D view's splats", 120.0, |run| run.app.sessions_mut().viewer_loaded())?;
    run.click("Select splats", 10.0)?;
    run.until("the splats ready to select", 120.0, |run| viewer(run).select_ready())?;
    run.wait(1.0)?;
    let view = Rect::from_min_max(pos2(0.0, 120.0), pos2(1280.0, 620.0));
    let painted = |run: &mut Run<'_>| -> anyhow::Result<Vec<egui::Color32>> {
        let image = run.frame(Vec::new(), true)?.context("a picture")?;
        let (w, y0, y1) = (image.size[0], (view.top() * scale) as usize, (view.bottom() * scale) as usize);
        Ok(image.pixels[y0 * w..y1 * w].to_vec())
    };
    let apart = |a: &[egui::Color32], b: &[egui::Color32]| -> f32 {
        let sum: u64 = a.iter().zip(b).map(|(x, y)| (0..3).map(|c| u64::from(x[c].abs_diff(y[c]))).sum::<u64>()).sum();
        sum as f32 / (a.len() * 3).max(1) as f32
    };
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let none = egui::Modifiers::default();
    let mut settled_at = [0usize; FRAMES + 1];
    for _ in 0..6 {
        painted(&mut run)?;
    }
    for round in 0..24 {
        let (name, k, m): (&str, egui::Key, egui::Modifiers) = match round % 4 {
            0 => ("select all", egui::Key::A, ctrl),
            1 => ("clear", egui::Key::Escape, none),
            2 => ("select all", egui::Key::A, ctrl),
            _ => ("hide, then undo", egui::Key::H, none),
        };
        let before = viewer(&mut run).select_counts().unwrap_or_default();
        run.frame(vec![Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }], false)?;
        run.frame(vec![Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: m }], false)?;
        run.until("the marks to change", 10.0, |run| counts(run) != before)?;
        let frames: Vec<Vec<egui::Color32>> = (0..FRAMES).map(|_| painted(&mut run)).collect::<anyhow::Result<_>>()?;
        let last = frames.last().context("a frame")?;
        let distances: Vec<f32> = frames.iter().map(|f| apart(f, last)).collect();
        let at = distances.iter().position(|d| *d <= SETTLED).unwrap_or(FRAMES);
        settled_at[at] += 1;
        println!("round {round} ({name}): from the last frame {}; settled on frame {}", distances.iter().map(|d| format!("{d:.2}")).collect::<Vec<_>>().join(" "), at + 1);
        if round % 4 == 3 {
            // Back to nothing hidden, for the next rounds.
            let z = egui::Key::Z;
            run.frame(vec![Event::Key { key: z, physical_key: None, pressed: true, repeat: false, modifiers: ctrl }], false)?;
            run.frame(vec![Event::Key { key: z, physical_key: None, pressed: false, repeat: false, modifiers: ctrl }], false)?;
            run.frame(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: none }], false)?;
            run.frame(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: none }], false)?;
            run.until("nothing hidden or selected", 10.0, |run| counts(run) == [0, 0, 0])?;
            for _ in 0..FRAMES {
                painted(&mut run)?;
            }
        }
    }
    println!("settled on frame 1..{FRAMES}, then never: {settled_at:?}");
    Ok(())
}

/// The session of the library whose folder ends so.
fn named(settings: &Settings, end: &str) -> anyhow::Result<PathBuf> {
    controller::list_sessions(&settings.sessions_dir).into_iter().map(|s| s.dir).find(|d| d.to_string_lossy().ends_with(end)).with_context(|| format!("no session ending {end}"))
}

/// Opens the window of the capture in `dir` as a click on its card would.
fn open_window(run: &mut Run<'_>, dir: &Path) -> anyhow::Result<usize> {
    let mine = |e: &Entry| matches!(&e.local, Some(OnThisComputer::Session(d)) if d == dir);
    run.until("the capture's card", 60.0, |run| {
        let g = run.app.sessions_mut().gallery_mut();
        !g.busy() && g.shown().iter().any(mine)
    })?;
    let n = run.app.sessions_mut().gallery_mut().shown().iter().position(mine).context("its card")?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().gallery_mut().open_card(&ctx, n);
    run.wait(1.4)?;
    Ok(n)
}

fn close_window(run: &mut Run<'_>) -> anyhow::Result<()> {
    run.click("Close", 10.0)?;
    run.wait(1.0)
}

/// Whether the control `label` is there and takes clicks.
fn on(run: &Run<'_>, label: &str) -> Option<bool> {
    widgets::control(&run.ctx, label).map(|(_, enabled)| enabled)
}

/// The capture's splats as Select splats leaves them: its first 25,000 removed, as hidden and deleted.
fn remove_some(dir: &Path) -> anyhow::Result<()> {
    let result = reconstruction::result_dir(dir);
    let ply = result.join(reconstruction::PLY_FILE);
    let (all, identity, _) = edit::read_for_view(&ply)?;
    let mut e = edit::Edit::new(all.len(), identity);
    for (i, m) in e.marks.iter_mut().enumerate().take(25_000) {
        *m = if i < 20_000 { edit::DELETED } else { edit::HIDDEN };
    }
    e.save(&result)?;
    Ok(())
}

fn wheel(run: &mut Run<'_>, at: Pos2, down: f32) -> anyhow::Result<()> {
    run.frame(vec![Event::PointerMoved(at)], false)?;
    run.frame(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -down), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE, source: egui::MouseWheelSource::Wheel }], false)?;
    run.wait(0.8)
}

/// `--only capture-wearer`: a capture that shows its wearer, on the real view and the real worker. A disposable library in
/// `--sessions` with one session whose name ends in `22-25-19` and whose result from before shows them. Its window offers
/// Reconstruct without my avatar; the press runs the worker; the window follows the new result, says what was done and offers the
/// way back; the card's menu says the same.
pub(super) fn wearer(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    use orbiters_core::picture::{WearerChoice, WEARER_FILE};
    let dir = named(settings, "22-25-19")?;
    let _ = std::fs::remove_file(dir.join(WEARER_FILE));
    let (ctl, backend, handles) = library(settings, gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles);
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    run.wait(1.0)?;
    run.size = vec2(1280.0, 820.0);
    let n = open_window(&mut run, &dir)?;
    run.wait(0.8)?;
    run.shoot(out, "wearer-window-before")?;
    let report = || reconstruction::read_report(&dir);
    anyhow::ensure!(report().is_some_and(|r| r.wearer.is_none()), "a result that shows its wearer to begin with");
    anyhow::ensure!(on(&run, "Reconstruct without my avatar") == Some(true), "its window offers to take them out");
    run.click("Reconstruct without my avatar", 10.0)?;
    run.wait(1.0)?;
    anyhow::ensure!(WearerChoice::read(&dir) == Some(WearerChoice { take_out: true }), "the choice is kept with the session");
    // Something to lose asks first; nothing to lose starts at once.
    if on(&run, "Reconstruct again") == Some(true) && ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty() {
        run.click("Reconstruct again", 10.0)?;
    }
    run.until("the reconstruction to start", 15.0, |_| ctl.view().reconstruction.is_some())?;
    run.pace = Duration::from_millis(60);
    run.until("the new result", 900.0, |_| ctl.view().reconstruction.is_none() && report().is_some_and(|r| r.wearer.is_some()))?;
    run.pace = Duration::ZERO;
    run.wait(2.0)?;
    let r = report().context("its report")?;
    println!("{:?}, wearer_not_found {}", r.wearer, r.wearer_not_found);
    run.shoot(out, "wearer-window-after")?;
    anyhow::ensure!(on(&run, "Reconstruct with my avatar") == Some(true), "the way back, once the result has come");
    close_window(&mut run)?;
    let ctx = run.ctx.clone();
    // As a right press on its card.
    let at = run.app.sessions_mut().gallery_mut().stage_card_centre(n).context("its card")?;
    run.frame(vec![Event::PointerMoved(at)], false)?;
    run.frame(vec![Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() }], false)?;
    run.frame(vec![Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() }], false)?;
    let _ = ctx;
    run.wait(0.8)?;
    run.shoot(out, "wearer-card-menu")?;
    Ok(())
}

pub(super) fn walk(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let main = named(settings, "12-14-00")?;
    let other = named(settings, "12-20-00")?;
    let view_only = named(settings, "view-only")?;
    let no_depth = named(settings, "no-depth")?;
    let result = reconstruction::result_dir(&main);
    // From captures with nothing to lose, whatever an earlier run left: only those it removes splats from ask first.
    for s in controller::list_sessions(&settings.sessions_dir) {
        if s.reconstruction.is_some() {
            edit::discard(&reconstruction::result_dir(&s.dir))?;
        }
    }
    let (ctl, backend, handles) = library(settings, gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles);
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    run.wait(1.0)?;
    // Only this library's world: what else this account has would push its cards down the page, and a card's menu opens
    // on a card that is drawn.
    run.app.sessions_mut().gallery_mut().stage_query("VRChat Home");
    run.wait(1.0)?;
    let report = || reconstruction::read_report(&main).map(|r| r.created_utc);

    // Its window: Edit on the close button's row, Reconstruct again beside View in 3D, both as Enzo's window shows them.
    run.size = vec2(1030.0, 800.0);
    open_window(&mut run, &main)?;
    run.shoot(out, "capture-window-1030")?;
    run.size = vec2(1280.0, 820.0);
    run.wait(0.8)?;
    run.shoot(out, "capture-window")?;
    anyhow::ensure!(on(&run, "Edit") == Some(true), "Edit on a reconstructed capture");
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(true), "Reconstruct again on a reconstructed capture");
    let (edit_r, _) = widgets::control(&run.ctx, "Edit").context("Edit")?;
    let (close_r, _) = widgets::control(&run.ctx, "Close").context("Close")?;
    anyhow::ensure!(edit_r.right() < close_r.left() && edit_r.height() == close_r.height() && edit_r.top() == close_r.top(), "Edit {edit_r:?} on the row of Close {close_r:?}");

    // Edit lands directly in Edit in 3D; its way back is one step, to the 3D view.
    run.click("Edit", 10.0)?;
    run.until("Edit in 3D", 120.0, |run| widgets::control(&run.ctx, "Save picture").is_some())?;
    run.wait(1.0)?;
    run.shoot(out, "capture-edit-3d")?;
    run.click("Close the editor", 10.0)?;
    run.wait(0.8)?;
    run.shoot(out, "capture-edit-3d-left")?;
    run.click("Close the 3D view", 10.0)?;
    run.wait(1.0)?;
    close_window(&mut run)?;

    // Splats removed: Reconstruct again asks first, Cancel changes nothing. Nothing is lost without it.
    remove_some(&main)?;
    ctl.bump_library();
    run.wait(1.0)?;
    let before = report();
    open_window(&mut run, &main)?;
    run.click("Reconstruct again", 10.0)?;
    run.wait(1.0)?;
    anyhow::ensure!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty(), "it asks before it starts");
    run.shoot(out, "again-question")?;
    run.click("Cancel", 10.0)?;
    run.wait(1.0)?;
    anyhow::ensure!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty(), "Cancel starts nothing");
    anyhow::ensure!(result.join(reconstruction::EDIT_FILE).is_file(), "Cancel keeps its edits");

    // Going ahead: the run in the window, on its card, in the card's menu.
    run.click("Reconstruct again", 10.0)?;
    run.wait(1.0)?;
    run.click("Reconstruct again", 10.0)?;
    run.until("the reconstruction to start", 15.0, |_| ctl.view().reconstruction.is_some())?;
    run.pace = Duration::from_millis(60);
    run.until("training under way", 600.0, |_| ctl.view().reconstruction.as_ref().is_some_and(|j| j.stage == reconstruction::Stage::Training && j.progress >= 0.05))?;
    run.wait(0.8)?;
    anyhow::ensure!(on(&run, "Stop reconstruction") == Some(true), "the window offers to stop it");
    anyhow::ensure!(on(&run, "Export…") == Some(false), "nothing exported of the result going away");
    run.shoot(out, "again-running-window")?;
    close_window(&mut run)?;
    // Tall enough for every card of this library: a menu opens on a card that is drawn.
    run.size = vec2(1280.0, 1500.0);
    run.wait(1.0)?;
    run.shoot(out, "again-running-card")?;
    let n = run.app.sessions_mut().gallery_mut().shown().iter().position(|e| matches!(&e.local, Some(OnThisComputer::Session(d)) if *d == main)).context("its card")?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().gallery_mut().stage_menu(&ctx, n);
    run.wait(0.5)?;
    run.shoot(out, "again-running-menu")?;
    key(&mut run, egui::Key::Escape)?;
    run.wait(0.4)?;
    run.size = vec2(1280.0, 820.0);
    run.wait(0.6)?;

    // The window left open while it ends: it goes on from the new result, without being opened again.
    open_window(&mut run, &main)?;
    run.until("the reconstruction to finish", 900.0, |_| ctl.view().reconstruction.is_none())?;
    run.pace = Duration::from_millis(16);
    run.until("the window on the new result", 30.0, |run| on(run, "Reconstruct again") == Some(true))?;
    run.wait(1.0)?;
    run.shoot(out, "again-done-window")?;
    for n in ctl.take_notices() {
        println!("notice: {n:?}");
    }
    anyhow::ensure!(report() != before, "a new report");
    anyhow::ensure!(!result.join(reconstruction::EDIT_FILE).exists(), "the edits went with the old result");

    // Nothing to lose now: it starts at once; Stop leaves the result as it is.
    let finished = report();
    run.click("Reconstruct again", 10.0)?;
    run.until("the reconstruction to start at once", 15.0, |_| ctl.view().reconstruction.is_some())?;
    run.pace = Duration::from_millis(60);
    run.until("its Stop", 30.0, |run| on(run, "Stop reconstruction") == Some(true))?;

    // A second capture while it runs: it waits, says for what, and can be cancelled.
    close_window(&mut run)?;
    open_window(&mut run, &other)?;
    run.click("Reconstruct again", 10.0)?;
    run.until("it to wait", 10.0, |run| on(run, "Waiting · cancel") == Some(true))?;
    anyhow::ensure!(ctl.view().reconstruct_queue.contains(&other), "in the queue");
    run.wait(0.6)?;
    run.shoot(out, "again-waiting-window")?;
    run.click("Waiting · cancel", 10.0)?;
    run.until("it out of the queue", 10.0, |_| ctl.view().reconstruct_queue.is_empty())?;
    run.wait(0.4)?;
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(true), "offered again once cancelled");
    close_window(&mut run)?;
    open_window(&mut run, &main)?;
    run.click("Stop reconstruction", 10.0)?;
    run.until("the run to stop", 60.0, |_| ctl.view().reconstruction.is_none())?;
    run.pace = Duration::from_millis(16);
    run.wait(0.6)?;
    anyhow::ensure!(report() == finished, "stopped: the result as it was");
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(true), "offered again once stopped");
    run.shoot(out, "again-stopped")?;
    ctl.take_notices();
    close_window(&mut run)?;

    // Its card's menu, offered, and what its row does: as its window's button.
    run.size = vec2(1280.0, 1500.0);
    run.wait(1.0)?;
    let n = run.app.sessions_mut().gallery_mut().shown().iter().position(|e| matches!(&e.local, Some(OnThisComputer::Session(d)) if *d == main)).context("its card")?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().gallery_mut().stage_menu(&ctx, n);
    run.wait(0.5)?;
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(true), "in its card's menu");
    anyhow::ensure!(on(&run, "Edit in 3D") == Some(true), "in its card's menu");
    run.shoot(out, "again-menu")?;
    remove_some(&main)?;
    run.click("Reconstruct again", 10.0)?;
    run.wait(1.0)?;
    anyhow::ensure!(on(&run, "Cancel") == Some(true), "from its menu it asks first too");
    anyhow::ensure!(ctl.view().reconstruction.is_none(), "and nothing started");
    run.shoot(out, "again-menu-question")?;
    run.click("Cancel", 10.0)?;
    run.wait(1.0)?;
    edit::discard(&result)?;

    // The captures that cannot: why, in the window and in the menu.
    let n = run.app.sessions_mut().gallery_mut().shown().iter().position(|e| matches!(&e.local, Some(OnThisComputer::Session(d)) if *d == view_only)).context("its card")?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().gallery_mut().stage_menu(&ctx, n);
    run.wait(0.5)?;
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(false), "dim in its menu");
    run.shoot(out, "again-view-only-menu")?;
    key(&mut run, egui::Key::Escape)?;
    run.wait(0.4)?;
    run.size = vec2(1280.0, 820.0);
    run.wait(0.6)?;
    open_window(&mut run, &view_only)?;
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(false), "disabled on a capture reduced to its 3D view");
    run.shoot(out, "again-view-only")?;
    let (r, _) = widgets::control(&run.ctx, "Reconstruct again").context("its button")?;
    run.frame(vec![Event::PointerMoved(r.center())], false)?;
    run.wait(1.0)?;
    run.shoot(out, "again-view-only-hint")?;
    close_window(&mut run)?;
    open_window(&mut run, &no_depth)?;
    anyhow::ensure!(on(&run, "Reconstruct to share") == Some(false), "no depth frames: nothing to make a 3D view from");
    anyhow::ensure!(on(&run, "Edit").is_none(), "no 3D view to edit yet");
    run.shoot(out, "again-no-depth")?;
    close_window(&mut run)?;

    // A session's detail asks the same question; Escape answers it and leaves the detail open.
    remove_some(&main)?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_where(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)), |s| s.dir == main);
    run.wait(1.2)?;
    run.click("Reconstruct again", 10.0)?;
    run.wait(1.0)?;
    anyhow::ensure!(on(&run, "Cancel") == Some(true), "the session's detail asks first");
    anyhow::ensure!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty(), "and nothing started");
    run.shoot(out, "session-question")?;
    key(&mut run, egui::Key::Escape)?;
    run.wait(1.0)?;
    anyhow::ensure!(run.app.sessions_mut().open_dir().as_deref() == Some(main.as_path()), "Escape answered the question, and left the detail open");
    anyhow::ensure!(ctl.view().reconstruction.is_none(), "Escape starts nothing");
    key(&mut run, egui::Key::Escape)?;
    run.wait(1.0)?;
    edit::discard(&result)?;

    // Visited worlds: a world's page. Its row's Reconstruct again and Stop; the back button floating.
    let t = run.time();
    run.app.set_page(Page::Worlds, t);
    run.until("the worlds", 30.0, |run| run.app.worlds_mut().loaded())?;
    run.wait(1.5)?;
    let world = worlds_id(settings)?;
    let ctx = run.ctx.clone();
    run.app.worlds_mut().open_world(&ctx, &world, Rect::from_min_size(pos2(264.0, 152.0), vec2(300.0, 169.0)));
    run.wait(1.6)?;
    run.shoot(out, "world-back-rest")?;
    let (rest, _) = widgets::control(&run.ctx, "Visited worlds").context("the back button")?;
    // The rows at the top of this library's page are the two that cannot: they say so.
    anyhow::ensure!(on(&run, "Reconstruct") == Some(false), "the row without depth frames");
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(false), "the row reduced to its 3D view");
    let middle = pos2(760.0, 500.0);
    wheel(&mut run, middle, 300.0)?;
    anyhow::ensure!(on(&run, "Reconstruct again") == Some(true), "on a row of the world's page");
    let (scrolled, _) = widgets::control(&run.ctx, "Visited worlds").context("the back button")?;
    anyhow::ensure!(rest == scrolled, "it stays where it is: {rest:?}, {scrolled:?}");
    run.shoot(out, "world-back-scrolled")?;

    // A row whose splats were removed: the question, over the page and over the floating button.
    for s in controller::list_sessions(&settings.sessions_dir) {
        if s.reconstruction.is_some() && !s.trimmed {
            remove_some(&s.dir)?;
        }
    }
    run.click("Reconstruct again", 10.0)?;
    run.wait(1.0)?;
    anyhow::ensure!(ctl.view().reconstruction.is_none(), "it asks first here too");
    run.shoot(out, "world-back-under-question")?;
    key(&mut run, egui::Key::Escape)?;
    run.wait(1.0)?;
    anyhow::ensure!(run.app.worlds_mut().open_world_id().is_some(), "Escape answered the question, and left the page open");
    run.click("Reconstruct again", 10.0)?;
    run.wait(1.0)?;
    run.click("Reconstruct again", 10.0)?;
    run.until("the row's run", 15.0, |_| ctl.view().reconstruction.is_some())?;
    run.pace = Duration::from_millis(60);
    run.until("its Stop on the row", 60.0, |run| on(run, "Stop reconstruction") == Some(true))?;
    run.until("some progress", 300.0, |_| ctl.view().reconstruction.as_ref().is_some_and(|j| j.stage == reconstruction::Stage::Training && j.progress >= 0.02))?;
    // Its toast gone, the row up from under where it was.
    run.wait(6.0)?;
    wheel(&mut run, middle, 160.0)?;
    run.shoot(out, "world-row-running")?;
    run.click("Stop reconstruction", 10.0)?;
    run.until("the row's run to stop", 60.0, |_| ctl.view().reconstruction.is_none())?;
    run.pace = Duration::from_millis(16);
    run.wait(0.8)?;
    ctl.take_notices();

    // The narrowest and the widest window: where it floats, and over what passes under it.
    for (w, h, name) in [(1040.0, 700.0, "narrow"), (2560.0, 1300.0, "wide")] {
        run.size = vec2(w, h);
        run.wait(0.8)?;
        wheel(&mut run, pos2(w * 0.6, h * 0.6), -2000.0)?;
        run.shoot(out, &format!("world-back-rest-{name}"))?;
        let (at_rest, _) = widgets::control(&run.ctx, "Visited worlds").context("the back button")?;
        wheel(&mut run, pos2(w * 0.6, h * 0.6), 340.0)?;
        run.shoot(out, &format!("world-back-scrolled-{name}"))?;
        let (down, _) = widgets::control(&run.ctx, "Visited worlds").context("the back button")?;
        anyhow::ensure!(at_rest == down, "{name}: it stays where it is: {at_rest:?}, {down:?}");
    }
    // Scrolled down, it still goes back.
    run.size = vec2(1280.0, 820.0);
    run.wait(0.8)?;
    run.click("Visited worlds", 10.0)?;
    run.until("the page closed", 10.0, |run| run.app.worlds_mut().open_world_id().is_none())?;
    run.wait(0.6)?;
    run.shoot(out, "world-back-pressed")?;
    for n in ctl.take_notices() {
        println!("notice: {n:?}");
    }
    ctl.shutdown();
    Ok(())
}

/// The world of the library's sessions.
fn worlds_id(settings: &Settings) -> anyhow::Result<String> {
    controller::list_sessions(&settings.sessions_dir).iter().find_map(|s| s.manifest.world.as_ref().and_then(|w| w.id.clone())).context("a session with a world")
}
