//! An avatar's or a world's picture changed on VRChat from its thumbnail in the project panel, for the signed-in VRChat
//! account's own uploads while the server allows VRChat sign-in: a hover dims the thumbnail under an upload mark
//! ("Change the picture on VRChat"), a press opens Windows' file dialog for a picture (on a worker), and the tray
//! service checks, prepares and uploads it (`Controller::vrchat_picture`), its progress turning over that thumbnail;
//! its outcome comes as a toast. Which items are the member's is asked once per account, a batch at a time, for the
//! thumbnails drawn. Anyone else's thumbnails, and every thumbnail while signed out of VRChat, stay as they are.

use orbiters_i18n::tr;
use super::art;
use super::state::{Poke, State};
use crate::controller::vrchat_data::{Request, Response, Scope, OWNED_BATCH};
use crate::pages::{Cx, Job};
use egui::{ColorImage, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_projects::avatars::Avatar;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::{feedback, icons, paint, theme, widgets};
use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The picture files the dialog offers: those the service reads.
const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
/// How long a picture sent shows as starting before the service's snapshot tells of its upload (a refusal comes as a
/// toast instead).
const SENT_GRACE: Duration = Duration::from_secs(5);

/// What the panel knows of the member's VRChat pictures.
#[derive(Default)]
pub(super) struct Pictures {
    /// The account the answers below are for: another account or VRChat session asks again.
    scope: Option<Scope>,
    /// IDs asked about, and those of them the member uploaded.
    asked: HashSet<String>,
    owned: HashSet<String>,
    /// Thumbnails drawn whose owner is not known yet: asked about on the next frame.
    wanted: Vec<String>,
    owned_job: Job<(Vec<String>, Result<Vec<String>, String>)>,
    /// After an ask failed (VRChat rate limiting, offline), none before then.
    retry_at: Option<Instant>,
    /// The file dialog, open for an item: the item and the file picked (its worker hands that to the service).
    pick: Job<(String, Option<PathBuf>)>,
    picking: Option<String>,
    /// Pictures sent to the service, by item: the file (shown once VRChat has it) and when it was sent.
    sent: HashMap<String, (PathBuf, Instant)>,
    /// The service's last outcome taken (its upload's serial); none before the first look.
    handled: Option<u64>,
    /// The new picture, read from the file picked once VRChat has it, for its thumbnail (with its sum, kept for the
    /// next window).
    shown: Job<Option<(String, ColorImage, u64)>>,
}

/// VRChat sign-in is allowed and signed in: pictures can be changed.
fn signed_in(cx: &Cx) -> bool {
    cx.snap.vrchat_login.allowed && cx.snap.vrchat_login.user.is_some()
}

/// Once a frame while the panel shows: asks who uploaded the thumbnails drawn, follows the picture sent, and shows the
/// new picture once VRChat has it.
pub(super) fn poll(ctx: &egui::Context, cx: &Cx, state: &mut State) {
    if state.fixture {
        state.vrchat_pictures.wanted.clear();
        return;
    }
    let p = &mut state.vrchat_pictures;
    let scope = Scope::of(cx.snap);
    if p.scope.as_ref() != Some(&scope) {
        p.scope = Some(scope.clone());
        p.asked.clear();
        p.owned.clear();
        p.owned_job = Job::default();
        p.retry_at = None;
    }
    if let Some((ids, result)) = p.owned_job.take() {
        match result {
            Ok(owned) => p.owned.extend(owned),
            Err(_) => {
                for id in ids {
                    p.asked.remove(&id);
                }
                p.retry_at = Some(Instant::now() + Duration::from_secs(60));
            }
        }
    }
    let wanted = std::mem::take(&mut p.wanted);
    if signed_in(cx) && !p.owned_job.running() && p.retry_at.is_none_or(|t| Instant::now() >= t) {
        let mut ids: Vec<String> = Vec::new();
        for id in wanted {
            if ids.len() < OWNED_BATCH && !p.asked.contains(&id) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            p.asked.extend(ids.iter().cloned());
            let backend = cx.ctl.clone();
            let scope = scope.clone();
            p.owned_job = Job::spawn(ctx, move || {
                let result = match backend.vrchat_data(scope, Request::Owned { ids: ids.clone() }) {
                    Ok(Response::Owned(owned)) => Ok(owned),
                    Ok(_) => Err("Unexpected answer.".to_string()),
                    Err(e) => Err(e),
                };
                (ids, result)
            });
        }
    }
    // The dialog's worker already handed the file to the service (the panel may have closed meanwhile).
    if let Some((id, path)) = p.pick.take() {
        p.picking = None;
        if let Some(path) = path {
            p.sent.insert(id, (path, Instant::now()));
        }
    }
    let upload = cx.snap.vrchat_login.upload.as_ref();
    let ended = upload.filter(|u| !u.running());
    match p.handled {
        // The first look: what ended before was not sent from here.
        None => p.handled = Some(ended.map_or(0, |u| u.serial)),
        Some(h) => {
            if let Some(u) = ended.filter(|u| u.serial != h) {
                p.handled = Some(u.serial);
                if let Some((path, _)) = p.sent.remove(&u.id) {
                    if u.outcome == Some(Ok(())) {
                        let id = u.id.clone();
                        // Invalidate both queued results and disk writes from pre-upload requests.
                        *state.thumb_versions.lock().unwrap().entry(id.clone()).or_default() += 1;
                        state.thumb_ids.insert(id.clone());
                        state.thumb_fresh.insert(id.clone());
                        p.shown = Job::spawn(ctx, move || {
                            let image = picture(&path)?;
                            let sum = super::thumb_cache::sum(&image);
                            super::thumb_cache::save(&id, sum, &image);
                            Some((id, image, sum))
                        });
                    }
                }
            }
        }
    }
    let running = upload.filter(|u| u.running()).map(|u| u.id.as_str());
    p.sent.retain(|id, (_, at)| running == Some(id.as_str()) || at.elapsed() < SENT_GRACE);
    if !p.sent.is_empty() {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    if let Some(Some((id, image, sum))) = p.shown.take() {
        state.set_thumb(ctx, id, image, sum);
    }
}

/// The picture picked, as the thumbnail shows it: made VRChat's as the upload was (turned, cropped to 4:3), 640 × 480.
fn picture(path: &Path) -> Option<ColorImage> {
    let fitted = crate::controller::vrchat_data::fit_picture(&std::fs::read(path).ok()?).ok()?;
    let small = image::imageops::resize(&fitted, 640, 480, image::imageops::FilterType::Triangle);
    Some(ColorImage::from_rgb([640, 480], small.as_raw()))
}

/// The picture of avatar or world `a` in `rect` (squircle corners `radii`), as [`art::avatar`] draws it. When it is the
/// signed-in VRChat account's own: a hover dims it under an upload mark and a press picks its new picture; while that
/// uploads, its progress turns over it. `i` names its anchor for screenshots.
pub(super) fn thumbnail(ui: &mut Ui, cx: &Cx, state: &mut State, a: &Avatar, rect: Rect, radii: [f32; 4], i: usize) {
    art::avatar(ui.painter(), state, a, rect, radii, 1.0);
    state.anchors.insert(format!("vrchat-picture-{i}"), rect);
    if !signed_in(cx) {
        return;
    }
    let ctx = ui.ctx().clone();
    let p = &mut state.vrchat_pictures;
    if !p.asked.contains(&a.id) {
        p.wanted.push(a.id.clone());
    }
    let id = Id::unique(("vrchat-picture", &a.id));
    let running = cx.snap.vrchat_login.upload.as_ref().filter(|u| u.running());
    if let Some(u) = running.filter(|u| u.id == a.id) {
        let k = anim::value(&ctx, id.with("progress"), u.step.map_or(0.0, |s| s.progress()), Spring::SOFT);
        progress(ui, rect, radii, Some(k));
        let label = u.step.map_or(tr("Uploading to VRChat…"), |s| s.label());
        let _ = ui.interact(rect, id, Sense::hover()).on_hover_text(label);
        return;
    }
    if p.sent.contains_key(&a.id) {
        progress(ui, rect, radii, None);
        return;
    }
    if !p.owned.contains(&a.id) {
        return;
    }
    let open = p.picking.as_deref() == Some(a.id.as_str());
    let busy = running.is_some() || p.picking.is_some() || !p.sent.is_empty();
    let resp = ui.interact(rect, id, Sense::click());
    let k = anim::toggle(&ctx, id.with("hover"), (resp.hovered() && !busy) || open, Spring::SNAPPY).clamp(0.0, 1.0);
    if k > 0.01 {
        let painter = ui.painter();
        feedback::fill_corners(painter, rect, radii, theme::black(0.5 * k));
        let size = (rect.height() * 0.34).clamp(14.0, 24.0) * (0.85 + 0.15 * k);
        widgets::icon(painter, rect.center(), icons::UPLOAD_SIMPLE, size, theme::white(0.95 * k));
    }
    if busy {
        if running.is_some() && !open {
            let _ = resp.on_hover_text(tr("Another picture is uploading to VRChat"));
        }
        return;
    }
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Change the picture on VRChat"));
    if widgets::activated(&resp) {
        change(&ctx, cx, state, a);
    }
}

/// Whether the picture of `id` can be changed now: the signed-in VRChat account's own, nothing else uploading or being
/// picked (the rows' right-click menus offer it then).
pub(super) fn can_change(cx: &Cx, state: &State, id: &str) -> bool {
    let p = &state.vrchat_pictures;
    let running = cx.snap.vrchat_login.upload.as_ref().is_some_and(|u| u.running());
    signed_in(cx) && p.owned.contains(id) && !running && p.picking.is_none() && p.sent.is_empty()
}

/// Opens Windows' file dialog (on a worker) for the new picture of `a`, which the service then uploads.
pub(super) fn change(ctx: &egui::Context, cx: &Cx, state: &mut State, a: &Avatar) {
    if state.fixture {
        return;
    }
    let p = &mut state.vrchat_pictures;
    let (item, name, backend, scope) = (a.id.clone(), a.name.clone(), cx.ctl.clone(), Scope::of(cx.snap));
    p.picking = Some(item.clone());
    p.pick = Job::spawn(ctx, move || {
        let path = rfd::FileDialog::new().set_title(tr!("A new picture for {name} on VRChat", name = name)).add_filter(tr("Pictures"), &EXTENSIONS).pick_file();
        if let Some(path) = &path {
            backend.vrchat_picture(scope, item.clone(), path.clone());
        }
        (item, path)
    });
}

/// Over an uploading thumbnail: it dims, and a ring fills with how far the upload is (`None`: starting, a spinner).
fn progress(ui: &Ui, rect: Rect, radii: [f32; 4], k: Option<f32>) {
    let painter = ui.painter();
    feedback::fill_corners(painter, rect, radii, theme::black(0.55));
    let radius = (rect.height().min(rect.width()) * 0.26).clamp(9.0, 18.0);
    let width = (radius * 0.22).clamp(2.0, 3.5);
    match k {
        Some(k) => {
            paint::ring(painter, rect.center(), radius, width, theme::white(0.18));
            paint::arc(painter, rect.center(), radius, -FRAC_PI_2, TAU * k.clamp(0.02, 1.0), width, theme::ACCENT);
            anim::request_frame(ui.ctx());
        }
        None => feedback::spinner(painter, rect.center(), radius, width, theme::ACCENT),
    }
}

/// Screenshots (`projects-vrchat-picture*`): the first project open, its avatars the member's but the first in
/// `-theirs` (someone else's), the second uploading in `-uploading`; in `-world`, the world project and its world.
pub(super) fn stage(state: &mut State, name: &str) {
    if !name.starts_with("projects-vrchat-picture") {
        return;
    }
    let world = name == "projects-vrchat-picture-world";
    let Some(project) = state.scan.projects.iter().find(|p| !world || !p.worlds.is_empty()) else { return };
    state.selected = Some(project.path.clone());
    let items: Vec<String> = project.avatars.iter().chain(&project.worlds).map(|a| a.id.clone()).collect();
    let skip = usize::from(name == "projects-vrchat-picture-theirs");
    let p = &mut state.vrchat_pictures;
    p.owned = items.iter().skip(skip).cloned().collect();
    p.asked = items.into_iter().collect();
}

/// Screenshots: the pointer over the first avatar's thumbnail (the member's, or someone else's in `-theirs`).
pub(super) fn stage_frame(state: &State, name: &str, frame: u32) -> Option<Poke> {
    match (name, frame) {
        ("projects-vrchat-picture" | "projects-vrchat-picture-theirs" | "projects-vrchat-picture-world", 40..) => {
            state.anchors.get("vrchat-picture-0").map(|r| Poke::Hover(r.center()))
        }
        _ => None,
    }
}
