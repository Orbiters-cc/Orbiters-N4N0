//! `--only splat-export`: Export… from end to end on the newest session of `--sessions`, which must be a disposable copy
//! of a reconstructed session (it writes the session's splat edits, damages and discards them; a hard-linked copy is
//! enough: nothing is written into a file the original shares). Windows' save dialog is stood in for (the files go to
//! `<out>/exports/`, under the name the dialog offers); the rest is the app's own.
//!
//! On the session's detail: Export… beside Send to Mixer, and its dialog saying what the edits leave out; GLB chosen and
//! exported (how far it got, then Saved and Show in folder), the file checked to hold exactly the splats its edits keep
//! without the sky; then a full disk (why, Choose another place, nothing written); damaged edits (Export waits, Discard
//! them, ready again). Then the 3D view: Export… in its top bar, the dialog over the view. Last, Photos & captures: the
//! capture's window with Export… among its actions and the dialog over it.
//!
//! `--only splat-export-cpu` is the same walk painted by the CPU (orbiters_ui::software), without the 3D view: no GPU,
//! so no wait for the lab's GPU lock.

use super::*;
use crate::pages::splat_export::{self as dialog, Hooks};
use orbiters_core::reconstruction as rec;
use orbiters_reconstruct::edit;
use std::sync::Mutex;

/// Where the stand-in save dialog puts files, and the room it says the disk has (the real room without one).
static TO: Mutex<Option<PathBuf>> = Mutex::new(None);
static FREE: Mutex<Option<u64>> = Mutex::new(None);

fn hooks() -> Hooks {
    Hooks {
        pick: |ask| TO.lock().unwrap().as_ref().map(|d| d.join(ask.name)),
        reveal: |p| {
            println!("Show in folder: {}", p.display());
            Ok(())
        },
        free_space: |p| FREE.lock().unwrap().or_else(|| orbiters_projects::size::free_space(p)),
    }
}

fn control(run: &Run<'_>, label: &str) -> Option<bool> {
    widgets::control(&run.ctx, label).map(|(_, enabled)| enabled)
}

fn backend(settings: &Settings) -> Ctl {
    let ctl = Controller::new(settings.clone());
    let sessions = controller::list_sessions(&settings.sessions_dir);
    ctl.set_snapshot(idle_snapshot(settings, &sessions));
    std::sync::Arc::new(Local::new(ctl))
}

pub(super) fn shots(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let handles = crate::gpu::Handles::of_offscreen(gpu);
    let mut run = Run::new(gpu, backend(settings), scale);
    run.app.set_gpu(handles.clone());
    walk(&mut run, out, settings, Some(&handles))
}

/// The walk painted on the CPU, without the 3D view.
pub(super) fn shots_cpu(out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let mut run = Run::cpu(backend(settings), scale);
    walk(&mut run, out, settings, None)
}

/// The walk; with `handles` (a GPU) the 3D view too.
fn walk(run: &mut Run<'_>, out: &Path, settings: &Settings, handles: Option<&crate::gpu::Handles>) -> anyhow::Result<()> {
    let exports = out.join("exports");
    let _ = std::fs::remove_dir_all(&exports);
    std::fs::create_dir_all(&exports)?;
    *TO.lock().unwrap() = Some(exports.clone());
    dialog::stage(hooks());
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
    let dir = run.app.sessions_mut().open_dir().with_context(|| format!("no session in {}", settings.sessions_dir.display()))?;
    let result = rec::result_dir(&dir);
    let ply = result.join(rec::PLY_FILE);
    anyhow::ensure!(ply.is_file(), "{} is not reconstructed", dir.display());

    // Edits as Select splats leaves them: the first 20,000 splats deleted, the next 5,000 hidden.
    edit::discard(&result)?;
    let (all, identity, _) = edit::read_for_view(&ply)?;
    let mut e = edit::Edit::new(all.len(), identity);
    for (i, m) in e.marks.iter_mut().enumerate().take(25_000) {
        *m = if i < 20_000 { edit::DELETED } else { edit::HIDDEN };
    }
    e.save(&result)?;
    let bits = |p: [f32; 3]| p.map(f32::to_bits);
    // The GLB's positions are Unity's with X negated.
    let gone: std::collections::HashSet<[u32; 3]> =
        (0..all.len()).filter(|&i| !e.keeps(i) || all.sky[i]).map(|i| bits([-all.means[i][0], all.means[i][1], all.means[i][2]])).collect();
    let kept = (0..all.len()).filter(|&i| e.keeps(i) && !all.sky[i]).count();
    run.wait(1.0)?;
    run.shoot(out, "splat-export-detail")?;

    // The dialog, GLB chosen, exported.
    run.click("Export…", 10.0)?;
    run.wait(0.9)?;
    run.shoot(out, "splat-export-dialog")?;
    run.click("GLB", 10.0)?;
    run.wait(0.3)?;
    run.click("Export GLB…", 10.0)?;
    run.until("the export under way", 30.0, |run| control(run, "Stop").is_some() || control(run, "Show in folder").is_some())?;
    if control(run, "Stop").is_some() {
        run.shoot(out, "splat-export-progress")?;
    }
    run.until("the export", 300.0, |run| control(run, "Show in folder").is_some())?;
    run.wait(0.5)?;
    run.shoot(out, "splat-export-saved")?;
    let name = crate::splat_export::file_name(&dir, "", crate::splat_export::Format::Glb);
    let glb = std::fs::read(exports.join(&name)).with_context(|| format!("{name} exported"))?;
    let n = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + n])?;
    let count = json["accessors"][0]["count"].as_u64().context("a count")? as usize;
    anyhow::ensure!(count == kept, "the GLB holds {count} splats, the edits keep {kept} (sky aside)");
    let at = 28 + n + json["bufferViews"][0]["byteOffset"].as_u64().context("an offset")? as usize;
    let held = (0..count)
        .filter(|i| {
            let f = |k: usize| f32::from_le_bytes(glb[at + (3 * i + k) * 4..at + (3 * i + k) * 4 + 4].try_into().unwrap());
            gone.contains(&bits([f(0), f(1), f(2)]))
        })
        .count();
    anyhow::ensure!(held == 0, "the GLB holds {held} removed or sky splats");
    println!("{name}: {} bytes, {count} splats of {}, none removed, no sky", glb.len(), all.len());
    run.click("Show in folder", 10.0)?;
    run.click("Done", 10.0)?;
    run.wait(0.8)?;
    anyhow::ensure!(!run.app.sessions_mut().exporting(), "Done closes the dialog");

    // A full disk: why, and Choose another place; nothing written.
    *FREE.lock().unwrap() = Some(1 << 20);
    run.click("Export…", 10.0)?;
    run.wait(0.9)?;
    run.click("SPZ", 10.0)?;
    run.click("Export SPZ…", 10.0)?;
    run.until("the full disk", 60.0, |run| control(run, "Choose another place…").is_some())?;
    run.wait(0.5)?;
    run.shoot(out, "splat-export-no-room")?;
    let spz = crate::splat_export::file_name(&dir, "", crate::splat_export::Format::Spz);
    let parts = std::fs::read_dir(&exports)?.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count();
    anyhow::ensure!(!exports.join(&spz).exists() && parts == 0, "nothing written on a full disk");
    *FREE.lock().unwrap() = None;
    run.click("Close", 10.0)?;
    run.wait(0.8)?;

    // Damaged edits: Export waits, Discard them, ready again.
    std::fs::write(result.join(rec::EDIT_FILE), b"N4EDIT01 damaged")?;
    run.click("Export…", 10.0)?;
    run.until("the damaged edits found", 10.0, |run| control(run, "Discard them") == Some(true))?;
    anyhow::ensure!(control(run, "Export GLB…") == Some(false), "Export waits while the edits cannot be applied (on GLB, the format last exported)");
    run.wait(0.6)?;
    run.shoot(out, "splat-export-damaged")?;
    run.click("Discard them", 10.0)?;
    run.until("the edits discarded", 10.0, |run| control(run, "Export GLB…") == Some(true))?;
    anyhow::ensure!(!result.join(rec::EDIT_FILE).exists(), "the edit file is gone");
    run.wait(0.5)?;
    run.shoot(out, "splat-export-discarded")?;
    run.click("Cancel", 10.0)?;
    run.wait(0.8)?;

    // The 3D view: Export… in its top bar, the dialog over the view.
    if let Some(handles) = handles {
        let ctx = run.ctx.clone();
        run.app.sessions_mut().open_viewer(&ctx, handles, false);
        run.until("the 3D view's splats", 120.0, |run| run.app.sessions_mut().viewer_loaded())?;
        run.wait(0.8)?;
        run.shoot(out, "splat-export-view")?;
        run.click("Export…", 10.0)?;
        run.wait(0.9)?;
        run.shoot(out, "splat-export-over-view")?;
        run.click("Cancel", 10.0)?;
        run.wait(0.8)?;
        run.click("Close the 3D view", 10.0)?;
        run.wait(0.5)?;
    }

    // Photos & captures: Export… among the capture's window's actions and its dialog over the window.
    run.click("Close", 10.0)?;
    run.wait(1.2)?;
    let mine = |e: &crate::controller::shares::Entry| matches!(&e.local, Some(crate::controller::shares::Local::Session(d)) if *d == dir);
    run.until("the capture's card", 60.0, |run| {
        let g = run.app.sessions_mut().gallery_mut();
        !g.busy() && g.shown().iter().any(mine)
    })?;
    let n = run.app.sessions_mut().gallery_mut().shown().iter().position(mine).context("its card")?;
    run.wait(0.5)?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().gallery_mut().open_card(&ctx, n);
    run.wait(1.4)?;
    run.shoot(out, "splat-export-window")?;
    run.click("Export…", 10.0)?;
    run.wait(0.9)?;
    run.shoot(out, "splat-export-over-window")?;
    run.click("Cancel", 10.0)?;
    run.wait(0.8)?;
    run.app.sessions_mut().gallery_mut().close_window();
    run.wait(1.2)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The walkthrough above on a disposable copy of a reconstructed session (a GPU run: under the lab's GPU lock):
    /// `ORBITERS_TEST_SESSIONS=<folder holding the copy> ORBITERS_TEST_OUT=<folder for the shots> cargo test -p
    /// orbiters-desktop export_walkthrough -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn export_walkthrough() {
        let sessions = std::env::var("ORBITERS_TEST_SESSIONS").expect("ORBITERS_TEST_SESSIONS");
        let out = std::env::var("ORBITERS_TEST_OUT").unwrap_or_else(|_| std::env::temp_dir().join("orbiters-splat-export").display().to_string());
        let args: Vec<String> = [out.as_str(), "--sessions", sessions.as_str(), "--only", "splat-export"].map(String::from).into();
        let run = std::thread::Builder::new().stack_size(16 << 20).spawn(move || super::super::run(&args)).unwrap();
        run.join().expect("no panic").expect("the walkthrough");
    }

    /// The same walk painted on the CPU, without the 3D view (no GPU, no lock): `ORBITERS_TEST_SESSIONS=<folder holding
    /// the copy> ORBITERS_TEST_OUT=<folder for the shots> cargo test -p orbiters-desktop export_walkthrough_on_the_cpu --
    /// --ignored --nocapture`
    #[test]
    #[ignore]
    fn export_walkthrough_on_the_cpu() {
        let sessions = std::env::var("ORBITERS_TEST_SESSIONS").expect("ORBITERS_TEST_SESSIONS");
        let out = std::env::var("ORBITERS_TEST_OUT").unwrap_or_else(|_| std::env::temp_dir().join("orbiters-splat-export").display().to_string());
        let args: Vec<String> = [out.as_str(), "--sessions", sessions.as_str(), "--only", "splat-export-cpu"].map(String::from).into();
        let run = std::thread::Builder::new().stack_size(16 << 20).spawn(move || super::super::run(&args)).unwrap();
        run.join().expect("no panic").expect("the walkthrough");
    }
}
