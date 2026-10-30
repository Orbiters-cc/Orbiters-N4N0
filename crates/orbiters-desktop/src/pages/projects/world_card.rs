//! A world project's Worlds card, in place of the Avatars card: each world its scenes were uploaded as, with its
//! picture, its page on VRChat, what this member shared of it on Orbiters' Map explorer, and the ways to add to it: a
//! 3D capture recorded there (its splats become the world's 3D preview), VRChat's pictures, or any pictures from the
//! computer. What is added shows on orbiters.cc's Maps page and in the VRChat world "Orbiters 3D pictures viewer",
//! which reads the same server. Uploads run in the tray service (they go on when the window closes) and come back in
//! its snapshot, as on the Visited worlds page. Each world's Blender files follow it ([`super::blend`]).

use super::art;
use super::state::State;
use crate::pages::{Cx, Job};
use crate::state::{AccountState, UploadKind, UploadState, UploadStatus};
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_core::maps::MineWorld;
use orbiters_projects::{avatars::Avatar, discovery::Project};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::popup::{self, Item};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, icons};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The VRChat world that shows, in VRChat, what members added to worlds on Orbiters.
const VIEWER_WORLD: &str = "Orbiters 3D pictures viewer";
/// How long an upload handed to the tray service shows as starting before the service lists it.
const HANDED_SECS: f64 = 3.0;

/// What this computer has of a world: the sessions recorded there and the pictures taken there.
#[derive(Clone, Debug, Default)]
pub(super) struct Here {
    /// Its name as VRChat's log gave it.
    pub name: Option<String>,
    /// Newest first.
    pub captures: Vec<Capture>,
    /// How many pictures VRChat took there, and the folder of the newest (where the file dialog opens).
    pub pictures: usize,
    pub folder: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(super) struct Capture {
    pub dir: PathBuf,
    /// When it was recorded, as people read it.
    pub when: String,
    /// Its splats, once reconstructed.
    pub splats: Option<u64>,
}

/// The worlds this computer has captures or pictures of ([`crate::worlds::load`]), by ID. Reads the library and the
/// screenshots' index: call on a worker.
pub(super) fn load(sessions_dir: &Path) -> HashMap<String, Here> {
    crate::worlds::load(sessions_dir)
        .into_iter()
        .map(|w| {
            let captures = w
                .sessions
                .iter()
                .map(|s| Capture {
                    dir: s.dir.clone(),
                    when: crate::pages::local_time(&s.manifest.created_utc),
                    splats: s.reconstruction.as_ref().map(|r| r.gaussians as u64),
                })
                .collect();
            let folder = w.shots.first().and_then(|s| s.path.parent().map(Path::to_path_buf));
            let name = (!w.name.is_empty() && w.name != "Unnamed world").then(|| w.name.clone());
            (w.id, Here { name, captures, pictures: w.shots.len(), folder })
        })
        .collect()
}

/// What this member has on Orbiters of a world, in words ("3D preview · 12 pictures"); empty for nothing.
fn on_orbiters(m: &MineWorld) -> String {
    let n = m.pictures.len();
    let pictures = (n > 0).then(|| format!("{n} {}", if n == 1 { "picture" } else { "pictures" }));
    [m.preview.as_ref().map(|_| "3D preview".to_string()), pictures].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// The world's page on VRChat.
pub(super) fn vrchat_page(id: &str) -> String {
    format!("https://vrchat.com/home/world/{id}")
}

/// A world's name: VRChat's (from its log), else the scene's object's unless it is the SDK's default, else the
/// project's.
fn display_name(w: &Avatar, here: Option<&Here>, project: &Project) -> String {
    here.and_then(|h| h.name.clone()).unwrap_or_else(|| if w.name.is_empty() || w.name == "VRCWorld" { project.name.clone() } else { w.name.clone() })
}

/// The library is read again on each visit and when it changes; Orbiters is asked once a visit what this member
/// added to its Map explorer.
fn refresh(ctx: &egui::Context, cx: &mut Cx, state: &mut State) {
    if state.fixture {
        return;
    }
    let key = (cx.ctl.library_version(), cx.snap.settings.sessions_dir.clone(), cx.visit);
    if state.here_key.as_ref() != Some(&key) && !state.here_job.running() {
        state.here_key = Some(key);
        let dir = cx.snap.settings.sessions_dir.clone();
        state.here_job = Job::spawn(ctx, move || load(&dir));
    }
    if state.maps_visit != Some(cx.visit) {
        state.maps_visit = Some(cx.visit);
        cx.ctl.request_maps();
    }
}

pub(super) fn card(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    refresh(&ctx, cx, state);
    let count = (!project.worlds.is_empty()).then(|| project.worlds.len().to_string());
    let maps = state.maps(cx);
    art::card(
        ui,
        "worlds",
        icons::GLOBE,
        "Worlds",
        count,
        |_, _| {},
        |ui| {
            if project.worlds.is_empty() {
                let text = "No uploaded world yet. Upload it from the VRChat SDK panel, save its scene, and it shows up here.";
                let g = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::FG_3).line_height(19.0).job(ui.available_width()));
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 4.0), Sense::hover());
                ui.painter().galley(r.min, g, theme::FG_3);
                return;
            }
            for (i, w) in project.worlds.iter().enumerate() {
                if i > 0 {
                    widgets::separator(ui);
                }
                let mine = maps.as_ref().and_then(|m| m.worlds.iter().find(|m| m.world_id == w.id));
                row(ui, cx, state, project, w, maps.is_some(), mine, i);
            }
        },
    );
}

/// A world: its picture (a rect of its own, [`super::vrchat_picture::thumbnail`]), its name and scene, what Orbiters
/// has of it, Open on VRChat, then the ways to add to it and its Blender files.
#[allow(clippy::too_many_arguments)]
fn row(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project, w: &Avatar, known: bool, mine: Option<&MineWorld>, i: usize) {
    let ctx = ui.ctx().clone();
    let here = state.here.get(&w.id).cloned();
    let name = display_name(w, here.as_ref(), project);
    let width = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(width, 72.0), Sense::hover());
    let thumb = Rect::from_min_size(pos2(r.left(), r.top() + 4.0), vec2(112.0, 63.0));
    super::vrchat_picture::thumbnail(ui, cx, state, w, thumb, [12.0; 4], i);
    let link = Rect::from_min_size(pos2(r.right() - 32.0, r.top() + 2.0), vec2(32.0, 32.0));
    let x = thumb.right() + 14.0;
    let tw = link.left() - 8.0 - x;
    let p = ui.painter();
    widgets::text_truncated(p, pos2(x, thumb.top() + 11.0), Align2::LEFT_CENTER, &Txt::new(&name, 14.5).weight(weight::SEMIBOLD), tw);
    let scene = if w.scene.is_empty() { w.id.clone() } else { w.scene.clone() };
    widgets::text_truncated(p, pos2(x, thumb.top() + 30.0), Align2::LEFT_CENTER, &Txt::new(scene, 11.5).color(theme::FG_3), tw);
    // What Orbiters has of it, in one pill (all of it on hover).
    let py = thumb.top() + 52.0;
    let on = mine.map(on_orbiters).unwrap_or_default();
    if known || mine.is_some() {
        let label = if on.is_empty() { "Not on Orbiters yet".to_string() } else { on.clone() };
        let pw = widgets::pill_at(p, ui, pos2(x, py), Some(icons::MAP_TRIFOLD), &label);
        let hint = if on.is_empty() { "Nothing of yours on Orbiters' Map explorer for this world yet".to_string() } else { format!("On Orbiters' Map explorer: {on}") };
        let _ = ui.interact(Rect::from_min_size(pos2(x, py - 10.0), vec2(pw, 20.0)), Id::unique(("world-on-orbiters", &w.id)), Sense::hover()).on_hover_text(hint);
    } else {
        // Orbiters has not answered yet.
        feedback::shimmer(p, Rect::from_min_size(pos2(x, py - 10.0), vec2(150.0, 20.0)), 10.0);
    }
    let url = vrchat_page(&w.id);
    state.anchors.insert(format!("world-vrchat-{i}"), link);
    let open = IconButton::new(icons::ARROW_SQUARE_OUT, "Open on VRChat").size(32.0, 15.0).backdrop(theme::control()).id_salt(("world-vrchat", &w.id));
    if widgets::activated(&open.show_at(ui, link).on_hover_text(&url)) {
        if let Err(e) = crate::controller::open_url(&url) {
            cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, format!("The page could not open: {e}"));
        }
    }
    add(ui, cx, state, w, &name, here.as_ref(), mine, i);
    super::blend::rows(ui, state, w, 0.0);
}

/// Where an upload of the world stands, to show under the ways to add to it.
enum Status {
    /// Handed to the tray service, not listed by it yet.
    Starting,
    Running(UploadStatus),
    Finished(UploadStatus),
}

/// The ways to add to the world on Orbiters (a 3D capture, VRChat's pictures, pictures from the computer), in a well
/// that says where they show, and how its latest upload goes.
#[allow(clippy::too_many_arguments)]
fn add(ui: &mut Ui, cx: &mut Cx, state: &mut State, w: &Avatar, name: &str, here: Option<&Here>, mine: Option<&MineWorld>, i: usize) {
    let ctx = ui.ctx().clone();
    let now = anim::time(&ctx);
    let uploads = state.uploads(cx, &w.id);
    // Listed by the service, or long enough: no longer starting.
    if let Some((at, kind)) = state.handed.get(&w.id).copied() {
        if now - at > HANDED_SECS || uploads.iter().any(|u| u.kind == kind && u.running()) {
            state.handed.remove(&w.id);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
    let status = if state.handed.contains_key(&w.id) {
        Some(Status::Starting)
    } else if let Some(u) = uploads.iter().find(|u| u.running()) {
        Some(Status::Running(u.clone()))
    } else {
        uploads.first().cloned().map(Status::Finished)
    };
    let sending = |kind: UploadKind| {
        state.handed.get(&w.id).is_some_and(|(_, k)| *k == kind) || uploads.iter().any(|u| u.running() && u.kind == kind)
    };
    let (sending_preview, sending_pictures) = (sending(UploadKind::Preview), sending(UploadKind::Pictures));
    let signed_in = cx.snap.account.state.username().is_some() && cx.snap.account.state != AccountState::SigningIn;
    let picking = state.pick_job.running();

    let width = ui.available_width();
    let inner = width - 28.0;
    let text = format!(
        "Add your 3D captures and pictures of {name}: they show on orbiters.cc's Maps page and in the VRChat world {VIEWER_WORLD}, for everyone."
    );
    let g = ui.painter().layout_job(Txt::new(&text, 12.5).color(theme::FG_3).line_height(19.0).job(inner));

    // The controls, in a flow that wraps on narrow panels.
    let captures: Vec<&Capture> = here.map(|h| h.captures.iter().filter(|c| c.splats.is_some()).collect()).unwrap_or_default();
    let recorded = here.is_some_and(|h| !h.captures.is_empty());
    let capture_label = if sending_preview {
        "Sharing in 3D…"
    } else if captures.len() > 1 {
        "Share a 3D capture"
    } else {
        "Share my 3D capture"
    };
    let capture_b = Button::new(capture_label).icon(icons::CUBE).size(Size::Sm);
    let pictures_b = Button::new("VRChat pictures").icon(icons::IMAGES).size(Size::Sm);
    let files_b = Button::new("From this computer").icon(icons::FOLDER_OPEN).size(Size::Sm);
    let sign_b = Button::new(if cx.snap.account.state == AccountState::SigningIn { "Finish in your browser" } else { "Sign in to Orbiters to add" })
        .icon(icons::SIGN_IN)
        .variant(Variant::Primary)
        .size(Size::Sm);
    // A dropdown of the captures when there are several: its caret takes room.
    let capture_w = capture_b.desired_width(ui) + if captures.len() > 1 && !sending_preview { 18.0 } else { 0.0 };
    let mut widths: Vec<f32> =
        if signed_in { vec![capture_w, pictures_b.desired_width(ui), files_b.desired_width(ui)] } else { vec![sign_b.desired_width(ui)] };
    // On one row when they fit, else one under the other, the well's width each.
    if widths.iter().sum::<f32>() + 8.0 * (widths.len() - 1) as f32 > inner {
        widths.iter_mut().for_each(|w| *w = inner);
    }
    let (places, buttons_h) = widgets::flow(&widths, inner, 32.0, 8.0);
    let status_h = if status.is_some() { 10.0 + 28.0 } else { 0.0 };
    let h = 12.0 + g.size().y + 10.0 + buttons_h + status_h + 12.0;
    ui.add_space(2.0);
    let (well, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let p = ui.painter().clone();
    widgets::frame(&p, well, 14.0, theme::white(0.035));
    let text_at = pos2(well.left() + 14.0, well.top() + 12.0);
    let text_h = g.size().y;
    p.galley(text_at, g, theme::FG_3);
    let top = text_at.y + text_h + 10.0;
    let at = |k: usize| Rect::from_min_size(pos2(well.left() + 14.0, top) + places[k], vec2(widths[k], 32.0));
    let mut bu = ui.new_child(UiBuilder::new().max_rect(well).id_salt(("world-add", &w.id)));
    if !signed_in {
        let r = at(0);
        state.anchors.insert(format!("world-sign-in-{i}"), r);
        if widgets::activated(&sign_b.id_salt(("world-sign-in", &w.id)).show_at(&mut bu, r)) {
            if cx.snap.account.state == AccountState::SigningIn {
                cx.ctl.reopen_sign_in();
            } else {
                cx.ctl.sign_in();
            }
        }
    } else {
        // A 3D capture: the one reconstructed here, or a pick among them.
        let r = at(0);
        state.anchors.insert(format!("world-capture-{i}"), r);
        let replaces = mine.is_some_and(|m| m.preview.is_some());
        let hint = if !recorded {
            "Record this world from the Capture page, then reconstruct it in Photos & captures"
        } else if captures.is_empty() {
            "Reconstruct a capture of this world in Photos & captures first"
        } else if replaces {
            "Its splats become the world's 3D preview on Orbiters, in place of yours"
        } else {
            "Its splats become the world's 3D preview on Orbiters"
        };
        let mut share: Option<PathBuf> = None;
        if captures.len() > 1 && !sending_preview {
            let labels: Vec<String> = captures.iter().map(|c| c.when.clone()).collect();
            let details: Vec<String> = captures.iter().map(|c| format!("{} splats", compact(c.splats.unwrap_or(0)))).collect();
            let items: Vec<Item> = labels.iter().zip(&details).map(|(l, d)| Item::new(l).detail(d).icon(icons::CUBE)).collect();
            if let Some(k) = popup::dropdown(&mut bu, ("world-capture", &w.id), r, capture_label, &items, None, true) {
                share = Some(captures[k].dir.clone());
            }
        } else {
            let b = capture_b.enabled(captures.len() == 1 && !sending_preview).hint(hint).id_salt(("world-capture", &w.id));
            if widgets::activated(&b.show_at(&mut bu, r)) {
                share = captures.first().map(|c| c.dir.clone());
            }
        }
        if let Some(dir) = share {
            state.handed.insert(w.id.clone(), (now, UploadKind::Preview));
            cx.ctl.upload_preview(w.id.clone(), name.to_string(), dir);
        }
        // Pictures: VRChat's (the dialog opens where the newest taken here is), or any from the computer.
        let taken = here.map_or(0, |h| h.pictures);
        let pictures_hint = match taken {
            0 => "Pick pictures you took in VRChat".to_string(),
            1 => "Pick pictures you took in VRChat: 1 was taken in this world".to_string(),
            n => format!("Pick pictures you took in VRChat: {n} were taken in this world"),
        };
        let r = at(1);
        state.anchors.insert(format!("world-pictures-{i}"), r);
        let enabled = !picking && !sending_pictures;
        if widgets::activated(&pictures_b.enabled(enabled).hint(&pictures_hint).id_salt(("world-pictures", &w.id)).show_at(&mut bu, r)) {
            let folder = here.and_then(|h| h.folder.clone());
            pick(state, &ctx, w, name, move || folder.or_else(|| orbiters_core::visited::screenshot_dirs().into_iter().next()), "Pictures of this world from VRChat");
        }
        let r = at(2);
        if widgets::activated(&files_b.enabled(enabled).hint("Pick pictures of this world from anywhere on the computer").id_salt(("world-files", &w.id)).show_at(&mut bu, r)) {
            let pictures = || std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Pictures")).filter(|p| p.is_dir());
            pick(state, &ctx, w, name, pictures, "Pictures of this world");
        }
    }
    if let Some(s) = status {
        let r = Rect::from_min_size(pos2(well.left() + 14.0, top + buttons_h + 10.0), vec2(inner, 28.0));
        status_line(&bu, r, &s, name);
    }
}

/// Opens the file dialog on a worker (in the folder `start` gives, worked out there too), for pictures to add to `w`.
fn pick(state: &mut State, ctx: &egui::Context, w: &Avatar, name: &str, start: impl FnOnce() -> Option<PathBuf> + Send + 'static, title: &'static str) {
    if state.fixture || state.pick_job.running() {
        return;
    }
    let (id, name) = (w.id.clone(), name.to_string());
    state.pick_job = Job::spawn(ctx, move || {
        let mut dialog = rfd::FileDialog::new().set_title(title).add_filter("Pictures", &["png", "jpg", "jpeg", "webp"]);
        if let Some(dir) = start() {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_files().map(|paths| (id, name, paths))
    });
}

/// How the world's latest upload goes: starting, preparing or sending (with its share of the bytes), added, or why
/// not.
fn status_line(ui: &Ui, r: Rect, s: &Status, name: &str) {
    let p = ui.painter();
    let icon_at = pos2(r.left() + 8.0, r.center().y - 2.0);
    let text_at = pos2(r.left() + 24.0, r.center().y - 2.0);
    let line = |text: &str, color: egui::Color32| {
        widgets::text_truncated(p, text_at, Align2::LEFT_CENTER, &Txt::new(text, 12.0).weight(weight::MEDIUM).color(color), r.width() - 24.0);
    };
    match s {
        Status::Starting => {
            feedback::spinner(p, icon_at, 6.0, 1.8, theme::ACCENT);
            line("Sending to Orbiters…", theme::FG_2);
        }
        Status::Running(u) => {
            feedback::spinner(p, icon_at, 6.0, 1.8, theme::ACCENT);
            let what = match u.kind {
                UploadKind::Preview => "the 3D capture".to_string(),
                UploadKind::Pictures => format!("{} {}", u.paths.len(), if u.paths.len() == 1 { "picture" } else { "pictures" }),
            };
            match u.state {
                UploadState::Preparing => line("Preparing the 3D capture for Orbiters…", theme::FG_2),
                _ => {
                    let f = anim::spring_from(ui.ctx(), Id::unique(("world-upload", u.id)), [0.0], [u.fraction()], Spring::SOFT)[0].clamp(0.0, 1.0);
                    line(&format!("Sharing {what} · {:.0}%", f * 100.0), theme::FG_2);
                    let bar = Rect::from_min_size(pos2(text_at.x, r.bottom() - 4.0), vec2(r.right() - text_at.x, 3.0));
                    p.rect_filled(bar, theme::radius(1.5), theme::white(0.08));
                    p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(3.0), 3.0)), theme::radius(1.5), theme::ACCENT);
                }
            }
        }
        Status::Finished(u) => {
            let pictures = |n: usize| format!("{n} {}", if n == 1 { "picture" } else { "pictures" });
            // Short on the line; the whole of it on hover.
            let (glyph, color, short, whole) = match (&u.state, u.kind) {
                (UploadState::Failed { message }, _) => (icons::WARNING, theme::DANGER, format!("Not added: {message}"), format!("Not added to {name}: {message}")),
                (UploadState::Done { .. }, UploadKind::Preview) => (
                    icons::CHECK_CIRCLE,
                    theme::ACCENT,
                    "3D capture shared".to_string(),
                    format!("Your 3D capture is {name}'s 3D preview on Orbiters"),
                ),
                (UploadState::Done { added: 0, .. }, UploadKind::Pictures) => {
                    (icons::CHECK_CIRCLE, theme::FG_3, "Already on Orbiters".to_string(), format!("These pictures of {name} were on Orbiters already"))
                }
                (UploadState::Done { added, skipped }, UploadKind::Pictures) => {
                    let more = if *skipped > 0 { format!(" · {skipped} there already") } else { String::new() };
                    (icons::CHECK_CIRCLE, theme::ACCENT, format!("{} added{more}", pictures(*added)), format!("{} added to {name} on Orbiters{more}", pictures(*added)))
                }
                _ => return,
            };
            widgets::icon(p, icon_at, glyph, 14.0, color);
            line(&short, if color == theme::ACCENT { theme::FG_2 } else { color });
            let _ = ui.interact(r, Id::unique(("world-upload-result", u.id)), Sense::hover()).on_hover_text(whole);
        }
    }
}

/// 1 234 → "1.2K", 860 000 → "860K", 1 260 000 → "1.3M".
fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}K", n as f64 / 1e3),
        10_000..=999_999 => format!("{:.0}K", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_is_named_as_vrchat_knows_it() {
        let project = Project { name: "Moonlight world".into(), ..Default::default() };
        let w = Avatar { id: "wrld_1".into(), name: "VRCWorld".into(), ..Default::default() };
        assert_eq!(display_name(&w, None, &project), "Moonlight world");
        let here = Here { name: Some("Moonlight".into()), ..Default::default() };
        assert_eq!(display_name(&w, Some(&here), &project), "Moonlight");
        let named = Avatar { name: "Moon Hall".into(), ..w };
        assert_eq!(display_name(&named, None, &project), "Moon Hall");
        assert_eq!(compact(1_260_000), "1.3M");
        assert_eq!(compact(980), "980");
        assert_eq!(compact(860_000), "860K");
        assert_eq!(vrchat_page("wrld_1"), "https://vrchat.com/home/world/wrld_1");
    }
}
