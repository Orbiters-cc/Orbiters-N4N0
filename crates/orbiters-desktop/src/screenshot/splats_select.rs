//! `--only splats-select`: Select splats from end to end on the newest session of `--sessions`, which must be a
//! disposable copy of a reconstructed session (it writes the session's splat edits, redraws its picture and prepares
//! its upload files beside them; a hard-linked copy is enough, nothing is written into a file the original shares).
//!
//! It opens the 3D view and Select splats, picks the splats under the view's middle and grows them, hides them, drags a
//! box and deletes what it holds, drags out a sphere on a surface, shrinks it and deletes it, paints with the brush,
//! undoes and redoes, orbits with the right button while a tool takes the left one, and leaves; shoots each step. Then
//! it checks the promise rather than the pixels: the edit saved beside the PLY leaves out what the view says is hidden
//! and deleted; the capture's picture was drawn again and the worker's kept aside; the view opened again has the same
//! edit; the upload's files, prepared as a share prepares them (the GPU-tuned PC and Quest trees, the coarse one, the
//! website's preview made from the PC tree's leaves), hold none of the removed splats. Last, failure and recovery: a
//! damaged edit file stops the upload with why, the view says so and offers to discard it, and once discarded the
//! capture is as trained again.

use super::*;
use crate::pages::splats::Viewer;
use orbiters_core::reconstruction;
use orbiters_reconstruct::edit;

fn viewer<'a>(run: &'a mut Run<'_>) -> &'a mut Viewer {
    run.app.sessions_mut().viewer_mut().expect("the 3D view is open")
}

/// Selected, hidden, deleted.
fn counts(run: &mut Run<'_>) -> [usize; 3] {
    viewer(run).select_counts().unwrap_or_default()
}

fn button(at: Pos2, pressed: bool, button: egui::PointerButton) -> Event {
    Event::PointerButton { pos: at, button, pressed, modifiers: Default::default() }
}

/// A press at `at` and its release on the next frame (a click).
fn click_at(run: &mut Run<'_>, at: Pos2) -> anyhow::Result<()> {
    run.frame(vec![Event::PointerMoved(at)], false)?;
    run.frame(vec![button(at, true, egui::PointerButton::Primary)], false)?;
    run.frame(vec![button(at, false, egui::PointerButton::Primary)], false)?;
    run.frame(Vec::new(), false)?;
    Ok(())
}

/// A drag of `with` from `from` to `to` over half a second, shot as `shot` just before it is let go.
fn drag(run: &mut Run<'_>, from: Pos2, to: Pos2, with: egui::PointerButton, shot: Option<(&Path, &str)>) -> anyhow::Result<()> {
    run.frame(vec![Event::PointerMoved(from)], false)?;
    run.frame(vec![button(from, true, with)], false)?;
    let start = run.time();
    loop {
        let k = ((run.time() - start) / 0.5).min(1.0) as f32;
        let at = from + (to - from) * k;
        run.frame(vec![Event::PointerMoved(at)], false)?;
        if k >= 1.0 {
            break;
        }
    }
    if let Some((out, name)) = shot {
        run.shoot(out, name)?;
    }
    run.frame(vec![button(to, false, with)], false)?;
    run.frame(Vec::new(), false)?;
    Ok(())
}

fn key(run: &mut Run<'_>, key: egui::Key, modifiers: egui::Modifiers) -> anyhow::Result<()> {
    run.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], false)?;
    run.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }], false)?;
    Ok(())
}

pub(super) fn shots(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let ctl = Controller::new(settings.clone());
    let sessions = controller::list_sessions(&settings.sessions_dir);
    ctl.set_snapshot(idle_snapshot(settings, &sessions));
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl));
    let handles = crate::gpu::Handles::of_offscreen(gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles.clone());
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
    let dir = run.app.sessions_mut().open_dir().with_context(|| format!("no session in {}", settings.sessions_dir.display()))?;
    let result = reconstruction::result_dir(&dir);
    let ply = result.join(reconstruction::PLY_FILE);
    anyhow::ensure!(ply.is_file(), "{} is not reconstructed", dir.display());
    // From the capture as trained (an earlier run's edits discarded, its picture back).
    edit::discard(&result)?;
    let trained_picture = std::fs::read(result.join(reconstruction::PREVIEW_FILE))?;
    run.wait(1.0)?;

    let open = |run: &mut Run<'_>| -> anyhow::Result<()> {
        let ctx = run.ctx.clone();
        run.app.sessions_mut().open_viewer(&ctx, &handles, false);
        run.until("the 3D view's splats", 120.0, |run| run.app.sessions_mut().viewer_loaded())
    };
    open(&mut run)?;
    run.wait(0.6)?;
    run.shoot(out, "splats-select-view")?;
    run.click("Select splats", 10.0)?;
    run.until("the splats ready to select", 120.0, |run| viewer(run).select_ready())?;
    run.wait(0.4)?;
    run.shoot(out, "splats-select-open")?;

    // The view's middle (the window under its title bar).
    let view = Rect::from_min_max(pos2(0.0, crate::app::TITLE_H), pos2(1280.0, 820.0));
    let middle = view.center();
    // Pick, then grow three times: a patch of what the middle shows.
    click_at(&mut run, middle)?;
    run.until("a pick", 10.0, |run| counts(run)[0] > 0)?;
    let picked = counts(&mut run)[0];
    run.shoot(out, "splats-select-pick")?;
    for _ in 0..3 {
        let before = counts(&mut run)[0];
        run.click("Grow", 10.0)?;
        // A thin test pattern may have nothing beside what was picked.
        if run.until("the selection to grow", 5.0, |run| counts(run)[0] > before).is_err() {
            break;
        }
    }
    run.wait(0.3)?;
    run.shoot(out, "splats-select-grow")?;
    let grown = counts(&mut run)[0];
    println!("picked {picked} splats, grown to {grown}");
    run.click("Hide", 10.0)?;
    run.until("the selection hidden", 10.0, |run| counts(run) == [0, grown, 0])?;
    run.wait(0.3)?;
    run.shoot(out, "splats-select-hidden")?;

    // A box at the left, through every depth: deleted.
    run.click("Box", 10.0)?;
    drag(&mut run, middle + vec2(-420.0, -120.0), middle + vec2(-260.0, 60.0), egui::PointerButton::Primary, Some((out, "splats-select-box")))?;
    run.until("a box's selection", 10.0, |run| counts(run)[0] > 0)?;
    let boxed = counts(&mut run)[0];
    run.click("Delete", 10.0)?;
    run.until("the box deleted", 10.0, |run| counts(run) == [0, grown, boxed])?;
    run.wait(0.3)?;
    run.shoot(out, "splats-select-deleted")?;

    // A sphere on what the right of the view shows, shrunk once, deleted.
    run.click("Sphere", 10.0)?;
    let on = middle + vec2(260.0, 40.0);
    drag(&mut run, on, on + vec2(70.0, 0.0), egui::PointerButton::Primary, Some((out, "splats-select-sphere")))?;
    run.until("a sphere's selection", 10.0, |run| counts(run)[0] > 0)?;
    let sphered = counts(&mut run)[0];
    run.click("Shrink", 10.0)?;
    run.until("the selection to shrink", 20.0, |run| counts(run)[0] < sphered)?;
    let shrunk = counts(&mut run)[0];
    run.click("Delete", 10.0)?;
    run.until("the sphere deleted", 10.0, |run| counts(run)[2] == boxed + shrunk)?;
    println!("box {boxed}, sphere {sphered} shrunk to {shrunk}");

    // The brush across the lower middle; Esc clears what it painted.
    run.click("Brush", 10.0)?;
    drag(&mut run, middle + vec2(-160.0, 150.0), middle + vec2(160.0, 170.0), egui::PointerButton::Primary, Some((out, "splats-select-brush")))?;
    run.until("a brush stroke's selection", 10.0, |run| counts(run)[0] > 0)?;
    key(&mut run, egui::Key::Escape, Default::default())?;
    run.until("the selection cleared", 10.0, |run| counts(run)[0] == 0)?;
    anyhow::ensure!(viewer(&mut run).select_ready(), "Esc with a selection clears it, and stays");

    // Undo, step by step: the clearing (the brush's selection back), the stroke (nothing selected), the sphere's
    // delete (the box's alone); then redo the delete.
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let painted = viewer(&mut run).select_counts().map(|c| c[0]);
    key(&mut run, egui::Key::Z, ctrl)?;
    run.until("undo of the clearing", 10.0, |run| counts(run)[0] > 0)?;
    anyhow::ensure!(painted == Some(0), "Esc had cleared the stroke");
    key(&mut run, egui::Key::Z, ctrl)?;
    run.until("undo of the stroke", 10.0, |run| counts(run)[0] == 0)?;
    key(&mut run, egui::Key::Z, ctrl)?;
    run.until("undo of the sphere's delete", 10.0, |run| counts(run)[2] == boxed)?;
    run.shoot(out, "splats-select-undo")?;
    key(&mut run, egui::Key::Z, egui::Modifiers { shift: true, ..ctrl })?;
    run.until("redo", 10.0, |run| counts(run)[2] == boxed + shrunk)?;
    // The right button orbits while a tool takes the left.
    drag(&mut run, middle, middle + vec2(-220.0, 0.0), egui::PointerButton::Secondary, None)?;
    run.wait(0.6)?;
    run.shoot(out, "splats-select-orbit")?;
    let [_, hidden, deleted] = counts(&mut run);
    run.click("Done", 10.0)?;
    run.wait(0.6)?;
    run.shoot(out, "splats-select-done")?;

    // Closed: saved beside the PLY, the picture drawn again without them.
    run.click("Close the 3D view", 10.0)?;
    run.until("the edit saved and its picture drawn", 180.0, |_| {
        std::fs::read(result.join(reconstruction::EDIT_FILE))
            .ok()
            .and_then(|b| edit::Edit::decode(&b).ok())
            .is_some_and(|e| e.picture && e.hidden() == hidden && e.deleted() == deleted)
    })?;
    let (kept, removed, _) = edit::read_edited(&ply)?;
    anyhow::ensure!(removed == hidden + deleted, "the file leaves out {removed}, the view said {hidden} hidden and {deleted} deleted");
    let trained_file = result.join(reconstruction::trained_preview_file(edit::Identity::read(&ply)?.hash));
    anyhow::ensure!(std::fs::read(&trained_file)? == trained_picture, "the worker's picture kept aside for this PLY");
    anyhow::ensure!(std::fs::read(result.join(reconstruction::PREVIEW_FILE))? != trained_picture, "the picture drawn again");
    run.wait(1.0)?;
    run.shoot(out, "splats-select-detail")?;

    // Opened again: the same edit.
    open(&mut run)?;
    run.wait(0.6)?;
    anyhow::ensure!(counts(&mut run) == [0, hidden, deleted], "reopened with {:?}", counts(&mut run));
    run.shoot(out, "splats-select-reopened")?;
    run.click("Close the 3D view", 10.0)?;
    run.wait(0.3)?;

    // The upload's files, as a share prepares them: none of the removed splats.
    let (all, _, _) = edit::read_for_view(&ply)?;
    let found = edit::find(&ply, edit::Identity::read(&ply)?, all.len());
    let edit::Found::Current(e) = found else { anyhow::bail!("the edit is not the PLY's") };
    let bits = |p: [f32; 3]| p.map(f32::to_bits);
    let gone: std::collections::HashSet<[u32; 3]> = (0..all.len()).filter(|&i| !e.keeps(i)).map(|i| bits(all.means[i])).collect();
    let start = Instant::now();
    let crate::controller::Prepared { files, generation, .. } = crate::controller::prepare_upload(&dir, orbiters_reconstruct::gpu::DeviceOptions::default())?;
    anyhow::ensure!(generation == e.generation(), "prepared with the edits there are");
    println!("upload files prepared in {:.1} s", start.elapsed().as_secs_f32());
    let mut report = Vec::new();
    for (name, bytes) in [("PC", &files.vrchat_pc), ("Quest", &files.vrchat_quest), ("coarse", &files.vrchat_coarse)] {
        let tree = orbiters_reconstruct::tree::Tree::parse(bytes)?;
        let leaves = tree.leaf_splats();
        let held = (0..tree.splats.len()).filter(|&i| gone.contains(&bits(tree.splats.means[i]))).count();
        anyhow::ensure!(held == 0, "the {name} tree holds {held} removed splats");
        anyhow::ensure!(leaves.len() <= kept.len(), "the {name} tree has more leaves than splats kept");
        report.push(format!("{name}: {} splats ({} leaves), none removed", tree.splats.len(), leaves.len()));
        if name == "PC" {
            // The website's preview, as the server makes it from the PC tree's leaves.
            let web =
                orbiters_reconstruct::web::read_splat(&orbiters_reconstruct::web::splat_file(&leaves, orbiters_reconstruct::web::DEFAULT_MAX, [0.0; 3]).bytes)?;
            let held = (0..web.len()).filter(|&i| gone.contains(&bits(web.means[i]))).count();
            anyhow::ensure!(held == 0, "the web preview holds {held} removed splats");
            report.push(format!("web: {} splats, none removed", web.len()));
        }
    }
    println!("{} removed of {}: {}", gone.len(), all.len(), report.join("; "));
    // The capture's picture leaves the app (uploads, drag-out) now that it is drawn for the edits.
    anyhow::ensure!(crate::capture_picture::jpeg(&dir).is_some(), "the picture drawn for the edits is given");
    // Mixer gets a copy of the 3D view's files holding only the kept splats (in a folder of this run, gone after).
    let exports = out.join("mixer-exports");
    let sent = crate::mixer_inbox::session_for_mixer(&dir, &exports)?;
    let to_mixer = orbiters_reconstruct::ply::read(&reconstruction::dir(&sent).join(reconstruction::PLY_FILE))?;
    anyhow::ensure!(to_mixer.len() == kept.len(), "Mixer's copy holds {} splats, the view keeps {}", to_mixer.len(), kept.len());
    anyhow::ensure!((0..to_mixer.len()).all(|i| !gone.contains(&bits(to_mixer.means[i]))), "Mixer's copy holds a removed splat");
    println!("Mixer's copy: {} splats, none removed", to_mixer.len());
    std::fs::remove_dir_all(&exports)?;

    // Damaged: the upload stops with why; the view says so, Select splats waits, Discard them recovers.
    std::fs::write(result.join(reconstruction::EDIT_FILE), b"N4EDIT01 damaged")?;
    let refused = crate::controller::prepare_upload(&dir, orbiters_reconstruct::gpu::DeviceOptions::default()).err().map(|e| format!("{e:#}"));
    anyhow::ensure!(refused.as_deref().is_some_and(|e| e.contains("could not be read")), "a damaged edit stops the upload: {refused:?}");
    open(&mut run)?;
    run.wait(0.6)?;
    anyhow::ensure!(viewer(&mut run).edits_problem().is_some(), "the view says so");
    anyhow::ensure!(viewer(&mut run).picture_refused().is_some(), "no picture of the view (Save this view, Edit in 3D) while it waits");
    anyhow::ensure!(crate::capture_picture::jpeg(&dir).is_none(), "nor of the capture");
    anyhow::ensure!(counts(&mut run) == [0, 0, 0], "nothing applied from it");
    run.shoot(out, "splats-select-problem")?;
    run.click("Discard them", 10.0)?;
    run.until("the edit discarded", 10.0, |run| viewer(run).edits_problem().is_none())?;
    anyhow::ensure!(viewer(&mut run).picture_refused().is_none(), "pictures of the view again");
    anyhow::ensure!(!result.join(reconstruction::EDIT_FILE).exists() && edit::read_edited(&ply)?.1 == 0, "the capture as trained");
    anyhow::ensure!(std::fs::read(result.join(reconstruction::PREVIEW_FILE))? == trained_picture, "the worker's picture back");
    run.wait(0.4)?;
    run.shoot(out, "splats-select-discarded")?;
    run.click("Select splats", 10.0)?;
    run.until("Select splats again", 60.0, |run| viewer(run).select_ready())?;
    run.click("Done", 10.0)?;
    run.click("Close the 3D view", 10.0)?;
    run.wait(0.5)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The walkthrough above on a disposable copy of a reconstructed session (a GPU test: under the lab's GPU lock):
    /// `ORBITERS_TEST_SESSIONS=<folder holding the copy> ORBITERS_TEST_OUT=<folder for the shots> cargo test -p
    /// orbiters-desktop select_splats_walkthrough -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn select_splats_walkthrough() {
        let sessions = std::env::var("ORBITERS_TEST_SESSIONS").expect("ORBITERS_TEST_SESSIONS");
        let out = std::env::var("ORBITERS_TEST_OUT").unwrap_or_else(|_| std::env::temp_dir().join("orbiters-splats-select").display().to_string());
        let args: Vec<String> = [out.as_str(), "--sessions", sessions.as_str(), "--only", "splats-select"].map(String::from).into();
        let run = std::thread::Builder::new().stack_size(16 << 20).spawn(move || super::super::run(&args)).unwrap();
        run.join().expect("no panic").expect("the walkthrough");
    }
}
