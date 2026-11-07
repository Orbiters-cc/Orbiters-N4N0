//! `--only language-live`: the language changed under a running window, through what really changes it: the settings the service holds (the picker in Settings, or the
//! account's answer in `controller/language.rs`, both end in them), which each frame follows. Every page is drawn in English, the language changes with it on screen, and whatever was
//! in English before and has a translation must be gone a moment later, with no restart and without leaving the page.
//!
//! A toast is told in two ways: a text kept as an English key follows, one written out when it was told stays what it was: it lasts seconds.
//! Prints what stayed (`stale`) and fails if anything did. Pictures of the first and last page, in both languages, to look at.

use super::*;
use crate::controller::NoticeKind;
use orbiters_i18n::{in_language, tr, Language};
use std::collections::BTreeSet;

/// A text told to a toast in English, as a share's result is.
const KEPT_IN_ENGLISH: &str = orbiters_i18n::mark("Everyone still sees the version shared before.");

/// What was drawn over `frames` frames from now. Text is read from frames that are painted.
fn drawn(run: &mut Run<'_>, frames: u32) -> anyhow::Result<BTreeSet<String>> {
    orbiters_ui::text_audit::take_seen();
    for _ in 0..frames {
        run.frame(Vec::new(), true)?;
    }
    Ok(orbiters_ui::text_audit::take_seen())
}

pub(super) fn walk(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    orbiters_ui::text_audit::turn_on();
    let sessions = controller::list_sessions(&settings.sessions_dir);
    let mut snapshot = idle_snapshot(settings, &sessions);
    snapshot.settings.language = Some("en".into());
    let ctl = Controller::new(snapshot.settings.clone());
    ctl.set_snapshot(snapshot);
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let mut run = Run::new(gpu, backend, scale);
    let pick = |language: Language| {
        // What the picker sends (`language::patch`).
        ctl.patch_settings(&crate::language::patch(Some(language)));
    };
    pick(Language::En);
    run.wait(1.0)?;
    let mut stale_anywhere = 0;
    for language in [Language::Fr, Language::Ja, Language::Es, Language::PtBr] {
        for (page, name) in [
            (Page::Home, "home"),
            (Page::N4n0, "n4n0"),
            (Page::Capture, "capture"),
            (Page::Projects, "projects"),
            (Page::Sessions, "gallery"),
            (Page::Worlds, "worlds"),
            (Page::Events, "events"),
            (Page::Videos, "videos"),
            (Page::Settings, "settings"),
        ] {
            pick(Language::En);
            let t = run.time();
            run.app.set_page(page, t);
            run.wait(1.2)?;
            // Told while English is on screen.
            ctl.notify(NoticeKind::Info, KEPT_IN_ENGLISH);
            ctl.notify(NoticeKind::Info, tr!("Not updated: {reason}", reason = "a reason"));
            run.wait(0.3)?;
            let before = drawn(&mut run, 6)?;
            // A run that reads nothing proves nothing.
            anyhow::ensure!(before.len() > 5, "{name}: only {} texts drawn", before.len());
            anyhow::ensure!(before.contains(KEPT_IN_ENGLISH), "{name}: the toast is not on screen");
            let first = language == Language::Fr && (page == Page::Home || page == Page::Settings);
            if first {
                run.shoot(out, &format!("live-{name}-en"))?;
            }
            pick(language);
            run.wait(0.4)?;
            let after = drawn(&mut run, 6)?;
            if first {
                run.shoot(out, &format!("live-{name}-{}", language.tag()))?;
            }
            let stale: Vec<&String> = before
                .iter()
                .filter(|text| after.contains(*text))
                .filter(|text| in_language(language, || tr(text.as_str()) != text.as_str()))
                .collect();
            println!("{} {name}: {} texts before, {} after, {} stayed in English", language.tag(), before.len(), after.len(), stale.len());
            for text in &stale {
                println!("  stale: {text:?}");
            }
            let key_follows = after.contains(in_language(language, || tr(KEPT_IN_ENGLISH)));
            let written_stays = after.iter().any(|t| t.starts_with("Not updated"));
            println!("  toasts: the one kept as a key follows: {key_follows}; the one written out when told stays English: {written_stays}");
            anyhow::ensure!(key_follows, "{name}: the toast did not follow");
            stale_anywhere += stale.len();
        }
    }
    pick(Language::En);
    run.wait(0.3)?;
    ctl.shutdown();
    anyhow::ensure!(stale_anywhere == 0, "{stale_anywhere} texts stayed in English after the language changed");
    Ok(())
}
