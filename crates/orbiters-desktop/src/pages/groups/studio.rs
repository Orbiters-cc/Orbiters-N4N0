//! The banner studio (REQUEST22 1: "super easy to edit group banners"): one picture — a file, a drop, one of the
//! account's VRChat gallery or of its Photos & captures — under the zones of each target, moved and zoomed by hand, with
//! live previews (the nameplate collapsed and expanded, a short and a long name, a light and a dark world; the group's
//! card), its three crops kept per account and group, and lossless exports that never replace a file (m1, m2, M12).
//! The group banner can then be uploaded and applied as two separate, confirmed steps (M6); the nameplate banner is
//! exported only (B4).
use super::art::{self, Crop, Look, Saved, Source, Target, Zone};
use super::{failure_text, Kind, State};
use crate::controller::groups::{Outcome, Prepared, Reply, Request, Write};
use crate::pages::Cx;
use egui::{pos2, vec2, Align2, Color32, Id, Mesh, Rect, Sense, Shape, Stroke, Ui};
use orbiters_core::groups::{ArtSlot, FileTag, GroupCard, InventoryFile};
use orbiters_i18n::tr;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

/// The two names the previews are drawn with (made up).
const SHORT_NAME: &str = "Kai"; // i18n: not shown (a made-up player name in a preview)
const LONG_NAME: &str = "Maximilian Featherstone"; // i18n: not shown (a made-up player name in a preview)
/// The worlds behind the previews.
const LIGHT_WORLD: Color32 = Color32::from_rgb(214, 224, 234);
const DARK_WORLD: Color32 = Color32::from_rgb(20, 24, 36);
/// Recent pictures of Photos & captures offered at most.
const PHOTOS_MAX: usize = 36;

struct Loaded {
    full: Arc<image::RgbaImage>,
    texture: egui::TextureHandle,
}

impl Loaded {
    fn size(&self) -> (u32, u32) {
        (self.full.width(), self.full.height())
    }
}

/// A recent picture of the person's own (Photos & captures).
struct Photo {
    path: PathBuf,
    texture: Option<egui::TextureHandle>,
}

enum Loading {
    /// A picture being opened, with the number of the choice it answers (only the latest is taken, Blocker 5).
    Source(u64, mpsc::Receiver<Result<(image::RgbaImage, Source), String>>),
    Photos(mpsc::Receiver<Vec<(PathBuf, Option<egui::ColorImage>)>>),
}

/// What an export gives: for each target, its file and size, or why it failed (each said apart, Minor 3).
type Exported = Vec<(Target, Result<(PathBuf, u64), String>)>;

pub(crate) struct Studio {
    pub card: GroupCard,
    account: String,
    pub target: Target,
    saved: Saved,
    source: Option<Loaded>,
    pending: Option<image::RgbaImage>,
    loading: Vec<Loading>,
    pub show_zones: bool,
    error: Option<String>,
    /// Exports of this visit: target, file, size in bytes.
    pub exported: Vec<(Target, PathBuf, u64)>,
    exporting: Option<mpsc::Receiver<Exported>>,
    /// The upload's PNG being made, then on its way (prepare → confirm → commit).
    making_upload: Option<mpsc::Receiver<Result<Vec<u8>, String>>>,
    /// Uploaded to the gallery, the group not changed yet (M6), with the art revision it was made from (Blocker 5).
    pub uploaded: Option<(InventoryFile, Option<u64>)>,
    /// The revision of the upload being prepared, confirmed and sent.
    upload_revision: Option<u64>,
    /// The uploaded art as a picture, for the apply's confirmation.
    uploaded_art: Option<egui::TextureHandle>,
    /// Each picture chosen gets a number; only the latest one's arrives (Blocker 5).
    choice: u64,
    /// The gallery picture asked for in full, when that is the latest choice.
    wanted_source: Option<String>,
    /// A number for this visit's sources: an upload made from another source is never applied to this art.
    source_nonce: u64,
    /// The gallery tiles in view at the last frame: only their pictures are asked (Major 6).
    visible_urls: Vec<String>,
    pub applied: bool,
    gallery: Option<(Vec<InventoryFile>, usize, bool, bool)>,
    photos: Option<Vec<Photo>>,
    masks: HashMap<(Target, u8), egui::TextureHandle>,
    staged: bool,
    asked_source: bool,
}

impl Studio {
    pub fn new(card: GroupCard, account: &str, source: Option<image::RgbaImage>, drafts: &art::Drafts) -> Self {
        let staged = source.is_some();
        let saved = if staged { Saved::default() } else { art::load_saved(account, &card.id) };
        // An upload made earlier and not applied: kept, but its revision ties it to the art it was made from (Major 8).
        let uploaded = drafts.uploaded(&card.id, ArtSlot::Banner).map(|u| (u.file.clone(), u.revision));
        Self {
            card,
            account: account.to_string(),
            target: Target::GroupBanner,
            saved,
            source: None,
            pending: source,
            loading: Vec::new(),
            show_zones: true,
            error: None,
            exported: Vec::new(),
            exporting: None,
            making_upload: None,
            uploaded,
            upload_revision: None,
            uploaded_art: None,
            choice: 0,
            wanted_source: None,
            source_nonce: 0,
            visible_urls: Vec::new(),
            applied: false,
            gallery: None,
            photos: None,
            masks: HashMap::new(),
            staged,
            asked_source: false,
        }
    }
    /// The screenshot review: the studio before any picture.
    pub fn staged_empty(card: GroupCard) -> Self {
        let mut s = Self::new(card, "", Some(image::RgbaImage::new(1, 1)), &art::Drafts::default());
        s.pending = None;
        s.asked_source = true;
        s
    }
    /// The screenshot review: a crop given.
    pub fn set_crop(&mut self, target: Target, crop: Crop) {
        *self.saved.crop_mut(target) = crop;
    }
    fn remember(&self) {
        if !self.staged {
            art::store_saved(&self.account, &self.card.id, &self.saved);
        }
    }
    pub(crate) fn answer(&mut self, kind: Kind, reply: Reply) {
        match (kind, reply) {
            (Kind::Inventory { tag: FileTag::Gallery, offset }, Reply::Inventory(page)) => {
                if let Some((list, at, more, loading)) = self.gallery.as_mut().filter(|g| g.1 == offset) {
                    // By what VRChat gave, unusable entries included (Minor 5).
                    *more = page.raw >= orbiters_core::vrchat_api::GROUP_PAGE;
                    *at += page.raw.max(1);
                    *loading = false;
                    for f in page.items {
                        if !list.iter().any(|x| x.id == f.id) {
                            list.push(f);
                        }
                    }
                }
            }
            (Kind::Inventory { tag: FileTag::Gallery, .. }, Reply::Failed(f)) => {
                self.error = Some(failure_text(f));
                if let Some(g) = self.gallery.as_mut() {
                    g.3 = false;
                }
            }
            (Kind::Source(url), Reply::Failed(f)) => {
                let _ = url;
                self.error = Some(failure_text(f));
            }
            _ => {}
        }
    }
    /// A picture of the account's gallery, read in full for the studio: taken only when it is still the latest choice
    /// (an older choice arriving late never replaces a newer one, Blocker 5).
    pub(crate) fn source_arrived(&mut self, url: &str, img: Option<image::RgbaImage>) {
        if self.wanted_source.as_deref() != Some(url) {
            return;
        }
        self.wanted_source = None;
        match img {
            Some(img) => {
                self.take_source(img);
                self.remember();
            }
            None => self.error = Some(tr("That picture could not be read.").into()),
        }
    }
    fn take_source(&mut self, img: image::RgbaImage) {
        self.pending = Some(img);
        self.source_nonce += 1;
        self.error = None;
    }
    /// The art as it is now (source and group banner crop): what an upload is bound to (Blocker 5).
    pub(crate) fn revision(&self) -> u64 {
        art::revision(self.saved.source.as_ref(), self.saved.crop(Target::GroupBanner)) ^ self.source_nonce.wrapping_mul(0x9e3779b97f4a7c15)
    }
    /// The revision of the upload under way, for the uploaded picture's record.
    pub(crate) fn uploaded_revision(&self) -> Option<u64> {
        self.upload_revision
    }
    /// Whether the uploaded picture is the art shown now: only then is it applied from here.
    pub(crate) fn upload_matches(&self) -> bool {
        self.uploaded.as_ref().is_some_and(|(_, r)| *r == Some(self.revision()))
    }
    /// A write that ended: the studio takes its upload and its apply (M6). Whether it was the studio's.
    pub(crate) fn ended(&mut self, prepared: Option<&Prepared>, outcome: &Outcome) -> bool {
        use crate::controller::groups::Done;
        match (prepared.map(|p| &p.write), outcome) {
            (Some(Write::Upload { tag: FileTag::Gallery, group, .. }), Outcome::Done(Done::Uploaded(f))) if *group == self.card.id => {
                self.uploaded = Some((f.clone(), self.upload_revision));
                self.error = None;
                true
            }
            (Some(Write::ApplyArt { slot: ArtSlot::Banner, .. }), Outcome::Done(Done::Group(g))) => {
                self.applied = true;
                self.uploaded = None;
                self.card.banner_url = g.banner_url.clone();
                true
            }
            // Refusals and unknowns are said by the window's warnings, with their check.
            (Some(Write::Upload { .. } | Write::ApplyArt { .. }), Outcome::Refused(_) | Outcome::Unknown) | (None, Outcome::Refused(_)) => true,
            _ => false,
        }
    }
    fn crop(&self) -> Crop {
        self.saved.crop(self.target)
    }
}

/// Picture files on a thread of their own: the full picture, oriented.
fn load_file(ctx: &egui::Context, studio: &mut Studio, path: PathBuf) -> Loading {
    studio.choice += 1;
    studio.wanted_source = None;
    let choice = studio.choice;
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    // Work of the window while it runs: it does not rest to the tray in the middle of it.
    let working = crate::pages::Working::begin();
    std::thread::spawn(move || {
        let _working = working;
        let r = art::load_source(&path).map(|img| (img, Source::File(path))).map_err(|_| tr("That picture could not be read.").to_string());
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    Loading::Source(choice, rx)
}

/// The newest pictures of the person's VRChat pictures folder and the folders added in Settings, with thumbnails.
fn recent_photos(ctx: &egui::Context, folders: Vec<PathBuf>) -> Loading {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    // Work of the window while it runs: it does not rest to the tray in the middle of it.
    let working = crate::pages::Working::begin();
    std::thread::spawn(move || {
        let _working = working;
        let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        let mut seen = 0usize;
        let mut stack: Vec<(PathBuf, u8)> = folders.into_iter().map(|f| (f, 0)).collect();
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                seen += 1;
                if seen > 20_000 {
                    break;
                }
                let path = e.path();
                let Ok(meta) = e.metadata() else { continue };
                if meta.is_dir() {
                    if depth < 2 {
                        stack.push((path, depth + 1));
                    }
                } else if path.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp")) {
                    found.push((meta.modified().unwrap_or(std::time::UNIX_EPOCH), path));
                }
            }
        }
        found.sort_by_key(|f| std::cmp::Reverse(f.0));
        let out = found
            .into_iter()
            .take(PHOTOS_MAX)
            .map(|(_, path)| {
                // Within the same bounds as a source (size first, pixels from the header, one at a time, Major 5).
                let thumb = art::load_source(&path).ok().map(|i| image::imageops::thumbnail(&i, 320.min(i.width()), ((320.0 * i.height() as f32 / i.width() as f32) as u32).clamp(1, 320))).map(|t| egui::ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw()));
                (path, thumb)
            })
            .collect();
        let _ = tx.send(out);
        ctx.request_repaint();
    });
    Loading::Photos(rx)
}

/// The display texture of a source: at most 2048 px a side (exports read the full picture).
fn texture_of(ctx: &egui::Context, img: &image::RgbaImage) -> egui::TextureHandle {
    let small = if img.width() > 2048 || img.height() > 2048 { image::imageops::thumbnail(img, 2048.min(img.width()), ((2048.0 * img.height() as f32 / img.width() as f32) as u32).clamp(1, 2048)) } else { img.clone() };
    let small = if small.height() > 2048 { image::imageops::thumbnail(&small, ((2048.0 * small.width() as f32 / small.height() as f32) as u32).max(1), 2048) } else { small };
    ctx.load_texture("group-studio-source", egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw()), egui::TextureOptions::LINEAR)
}

/// The source picture drawn into `rect` as the crop puts it.
fn draw_crop(p: &egui::Painter, rect: Rect, loaded: &Loaded, target: Target, crop: Crop, tint: Color32) {
    let size = loaded.size();
    let crop = crop.clamped(size, target);
    let mut mesh = Mesh::with_texture(loaded.texture.id());
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let (sx, sy) = crop.source_point(size, target, u, v);
        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(rect.left() + u * rect.width(), rect.top() + v * rect.height()), uv: pos2(sx, sy), color: tint });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    p.add(Shape::mesh(mesh));
}

fn zone_colour(z: Option<Zone>) -> Color32 {
    match z {
        Some(Zone::Always) => theme::alpha(theme::ACCENT, 0.30),
        Some(Zone::Expanded) => theme::white(0.16),
        Some(Zone::ShortNames | Zone::LongNames) => theme::alpha(theme::WARN, 0.34),
        Some(Zone::Bleed) => theme::alpha(theme::DANGER, 0.30),
        None => theme::black(0.55),
    }
}

/// A target's zones as a picture over the canvas (`kind` 0), or what a look hides of it (`kind` 1 + look bits), drawn
/// once at 512 px wide with 2×2 samples a pixel.
fn mask(ctx: &egui::Context, masks: &mut HashMap<(Target, u8), egui::TextureHandle>, target: Target, kind: u8) -> egui::TextureHandle {
    masks
        .entry((target, kind))
        .or_insert_with(|| {
            let w = 512usize;
            let h = ((w as f32) / target.aspect()).round() as usize;
            let mut px = vec![Color32::TRANSPARENT; w * h];
            for y in 0..h {
                for x in 0..w {
                    let mut acc = [0f32; 4];
                    for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                        let z = art::zone_at(target, (x as f32 + dx) / w as f32, (y as f32 + dy) / h as f32);
                        let c = if kind == 0 {
                            zone_colour(z)
                        } else {
                            let look = Look { expanded: kind & 2 != 0, long_name: kind & 4 != 0 };
                            theme::white(1.0 - art::shown(z, look))
                        };
                        let [r, g, b, a] = c.to_srgba_unmultiplied();
                        for (i, v) in [r, g, b, a].into_iter().enumerate() {
                            acc[i] += v as f32 / 4.0;
                        }
                    }
                    px[y * w + x] = Color32::from_rgba_unmultiplied(acc[0] as u8, acc[1] as u8, acc[2] as u8, acc[3] as u8);
                }
            }
            ctx.load_texture(format!("group-zones-{target:?}-{kind}"), egui::ColorImage::new([w, h], px), egui::TextureOptions::LINEAR)
        })
        .clone()
}

fn label(target: Target) -> &'static str {
    match target {
        Target::GroupBanner => tr("Group banner"),
        Target::Anchored => tr("Nameplate · Anchored"),
        Target::Centered => tr("Nameplate · Centered"),
    }
}

fn zone_name(z: Zone) -> &'static str {
    match z {
        Zone::Always => tr("Always visible"),
        Zone::Expanded => tr("Visible when the nameplate is expanded"),
        Zone::ShortNames => tr("May be hidden for short names"),
        Zone::LongNames => tr("May be hidden for long names"),
        Zone::Bleed => tr("Bleed: let the art run on, keep faces and logos out"),
    }
}

enum Act {
    Back,
    Target(Target),
    PickFile,
    Gallery,
    MoreGallery,
    Photos,
    UseInventory(InventoryFile),
    UsePhoto(PathBuf),
    Export(bool),
    Reveal(PathBuf),
    Upload(Rect),
    Apply(Rect),
    Web,
}

pub(crate) fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let scope = state.scope.clone();
    let busy = state.flow.is_some();
    let Some(studio) = state.studio.as_mut() else { return };
    take_loading(&ctx, studio);
    // A picture dropped on the window.
    let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
    if let Some(path) = dropped.into_iter().next() {
        let l = load_file(&ctx, studio, path);
        studio.loading.push(l);
    }
    let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());
    // The remembered source, once.
    let remembered = if !studio.asked_source && studio.source.is_none() && studio.pending.is_none() {
        studio.asked_source = true;
        studio.saved.source.clone()
    } else {
        None
    };
    match remembered {
        Some(Source::File(path)) if path.is_file() => {
            let l = load_file(&ctx, studio, path);
            studio.loading.push(l);
        }
        Some(Source::Inventory { url, .. }) => {
            if let Some(scope) = &scope {
                studio.choice += 1;
                studio.wanted_source = Some(url.clone());
                state.jobs.ask(&ctx, cx.ctl, scope, Kind::Source(url.clone()), Request::Picture { url });
            }
        }
        _ => {}
    }
    // The gallery's pictures in view at the last frame, from memory or asked (never the whole gallery, Major 6).
    let urls: Vec<String> = state.studio.as_ref().map(|s| s.visible_urls.clone()).unwrap_or_default();
    let mut thumbs: HashMap<String, egui::TextureHandle> = HashMap::new();
    if let Some(scope) = &scope {
        for url in urls {
            if let Some(t) = state.picture(&ctx, cx.ctl, scope, &url) {
                thumbs.insert(url, t);
            }
        }
    }
    let Some(studio) = state.studio.as_mut() else { return };
    if let Some(img) = studio.pending.take() {
        let texture = texture_of(&ctx, &img);
        studio.source = Some(Loaded { full: Arc::new(img), texture });
    }
    let mut acts: Vec<Act> = Vec::new();
    let full = ui.max_rect();
    let from = orbiters_ui::blur::next_shape(ui);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full).id_salt("group-studio"));
    egui::ScrollArea::vertical().id_salt("group-studio").auto_shrink([false, false]).scroll_bar_rect(super::super::below_bar(&child, cx)).show(&mut child, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 16.0);
            ui.set_max_width(ui.available_width().min(1080.0));
            // An upload or apply whose outcome is not known, with its check (Blocker 2).
            let group = state.studio.as_ref().map(|st| st.card.id.clone()).unwrap_or_default();
            let ctx = ui.ctx().clone();
            super::uncertain_panel(ui, &ctx, cx.ctl, state, &group);
            let Some(studio) = state.studio.as_mut() else { return };
            body(ui, cx, studio, &thumbs, busy, hovering_files, &mut acts);
        });
    });
    super::super::under_top(ui, cx, "group-studio", from, full.top() + cx.top + 8.0);
    for act in acts {
        act_on(&ctx, cx, state, act);
    }
}

fn take_loading(ctx: &egui::Context, studio: &mut Studio) {
    let mut keep = Vec::new();
    let loading = std::mem::take(&mut studio.loading);
    for l in loading {
        match l {
            Loading::Source(choice, rx) => match rx.try_recv() {
                // An older choice that arrives late is dropped (Blocker 5).
                Ok(_) if choice != studio.choice => {}
                Ok(Ok((img, source))) => {
                    studio.take_source(img);
                    studio.saved.source = Some(source);
                    studio.photos = None;
                    studio.gallery = None;
                    studio.remember();
                }
                Ok(Err(e)) => studio.error = Some(e),
                Err(mpsc::TryRecvError::Empty) => keep.push(Loading::Source(choice, rx)),
                Err(_) => {}
            },
            Loading::Photos(rx) => match rx.try_recv() {
                Ok(list) => {
                    studio.photos = Some(list.into_iter().map(|(path, img)| Photo { texture: img.map(|i| ctx.load_texture("group-studio-photo", i, egui::TextureOptions::LINEAR)), path }).collect());
                }
                Err(mpsc::TryRecvError::Empty) => keep.push(Loading::Photos(rx)),
                Err(_) => {}
            },
        }
    }
    studio.loading = keep;
    if let Some(rx) = &studio.exporting {
        match rx.try_recv() {
            Ok(results) => {
                // Each target said apart: the ones written are listed even when another failed (Minor 3).
                for (t, r) in results {
                    match r {
                        Ok((path, size)) => studio.exported.push((t, path, size)),
                        Err(e) => studio.error = Some(format!("{}: {e}", label(t))),
                    }
                }
                studio.exporting = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(_) => studio.exporting = None,
        }
    }
}

fn body(ui: &mut Ui, cx: &Cx, studio: &mut Studio, thumbs: &HashMap<String, egui::TextureHandle>, busy: bool, hovering: bool, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    // Back, the title, the group.
    let (row, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
    if widgets::Button::new(tr("Back")).icon(icons::ARROW_LEFT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("studio-back").show_at(ui, Rect::from_min_size(row.min, vec2(96.0, 32.0))).clicked() {
        acts.push(Act::Back);
    }
    let (head, _) = ui.allocate_exact_size(vec2(w, 66.0), Sense::hover());
    let name = if studio.card.name.is_empty() { tr("Your group").to_string() } else { studio.card.name.clone() };
    super::super::heading_in(ui, head, tr("Banner studio"), tr("Make the group's banners"), Some(&orbiters_i18n::tr!("{group}: one picture for the group banner and both nameplate banners", group = name)), w);
    // The three targets.
    let options = Target::ALL.map(|t| widgets::SegOption::new(t, label(t)));
    let mut target = studio.target;
    ui.horizontal(|ui| {
        if widgets::segmented(ui, "studio-target", &mut target, &options, false) {
            acts.push(Act::Target(target));
        }
    });
    // Where the picture comes from.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if widgets::Button::new(tr("Choose a picture…")).icon(icons::FOLDER_OPEN).variant(if studio.source.is_none() { widgets::Variant::Primary } else { widgets::Variant::Secondary }).id_salt("studio-file").show(ui).clicked() {
            acts.push(Act::PickFile);
        }
        if widgets::Button::new(tr("Your VRChat gallery")).icon(icons::IMAGES).id_salt("studio-gallery").show(ui).clicked() {
            acts.push(Act::Gallery);
        }
        if widgets::Button::new(tr("Photos & captures")).icon(icons::IMAGE).id_salt("studio-photos").show(ui).clicked() {
            acts.push(Act::Photos);
        }
        if !studio.loading.is_empty() {
            ui.label(egui::RichText::new(tr("Opening the picture…")).color(theme::FG_3).size(13.0));
        }
    });
    if let Some(e) = &studio.error {
        ui.label(egui::RichText::new(e).color(theme::WARN).size(13.0));
    }
    choosers(ui, studio, thumbs, acts);
    let target = studio.target;
    // The canvas: the target's shape, the picture under it, the zones over it.
    let max_h = 420.0f32;
    let cw = w.min(max_h * target.aspect());
    let ch = cw / target.aspect();
    let (outer, _) = ui.allocate_exact_size(vec2(w, ch), Sense::hover());
    let canvas = Rect::from_center_size(outer.center(), vec2(cw, ch));
    let p = ui.painter().clone();
    let id = Id::unique("studio-canvas");
    let resp = ui.interact(canvas, id, Sense::click_and_drag());
    match studio.source.as_ref() {
        None => {
            p.rect_filled(canvas, theme::radius(16.0), theme::INK_800);
            let stroke = Stroke::new(1.5, if hovering { theme::ACCENT } else { theme::alpha(theme::FG_3, 0.5) });
            p.extend(Shape::dashed_line(&[canvas.left_top(), canvas.right_top(), canvas.right_bottom(), canvas.left_bottom(), canvas.left_top()], stroke, 8.0, 6.0));
            widgets::icon(&p, canvas.center() - vec2(0.0, 22.0), icons::IMAGE, 30.0, theme::FG_3);
            widgets::text(&p, canvas.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, &Txt::new(tr("Drop a picture here, or choose one above"), 14.0).color(theme::FG_2));
        }
        Some(loaded) => {
            let size = loaded.size();
            let mut crop = studio.crop().clamped(size, target);
            // Drag to move, the wheel to zoom about the pointer, a double click to start again.
            if resp.dragged() {
                let d = resp.drag_delta();
                crop = crop.panned(size, target, d.x / canvas.width(), d.y / canvas.height());
            }
            if resp.hovered() {
                let scroll = ctx.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
                if scroll.abs() > 0.1 {
                    if let Some(at) = resp.hover_pos() {
                        let (u, v) = ((at.x - canvas.left()) / canvas.width(), (at.y - canvas.top()) / canvas.height());
                        crop = crop.zoomed(size, target, (scroll / 300.0).exp(), u, v);
                    }
                }
            }
            if resp.double_clicked() {
                crop = Crop::default().clamped(size, target);
            }
            if crop != studio.crop() {
                *studio.saved.crop_mut(target) = crop;
                if !resp.dragged() {
                    studio.remember();
                }
            }
            if resp.drag_stopped() {
                studio.remember();
            }
            p.rect_filled(canvas, 0.0, theme::INK_950);
            draw_crop(&p, canvas, loaded, target, crop, Color32::WHITE);
            if studio.show_zones && target != Target::GroupBanner {
                let m = mask(&ctx, &mut studio.masks, target, 0);
                p.image(m.id(), canvas, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                for (_, poly) in target.zones() {
                    let pts: Vec<egui::Pos2> = poly.iter().map(|(x, y)| pos2(canvas.left() + x * canvas.width(), canvas.top() + y * canvas.height())).collect();
                    p.add(Shape::closed_line(pts, Stroke::new(1.25, theme::white(0.55))));
                }
            }
            p.rect_stroke(canvas, 0.0, Stroke::new(1.0, theme::white(0.18)), egui::StrokeKind::Outside);
            resp.on_hover_cursor(egui::CursorIcon::Grab);
        }
    }
    // Under it: zoom, a quarter turn, start again, the zones and what they mean.
    if let Some(loaded) = studio.source.as_ref() {
        let size = loaded.size();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            let mut zoom = studio.crop().zoom;
            ui.label(egui::RichText::new(tr("Zoom")).color(theme::FG_3).size(13.0));
            if widgets::slider(ui, "studio-zoom", &mut zoom, 1.0..=art::ZOOM_MAX, 0.05, |z| format!("{:.0} %", z * 100.0)) {
                let c = studio.crop();
                *studio.saved.crop_mut(studio.target) = Crop { zoom, ..c }.clamped(size, studio.target);
                studio.remember();
            }
            if widgets::Button::new(tr("Turn")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt("studio-turn").show(ui).clicked() {
                let c = studio.crop().turned_once(size, studio.target);
                *studio.saved.crop_mut(studio.target) = c;
                studio.remember();
            }
            if widgets::Button::new(tr("Fit")).icon(icons::CORNERS_IN).size(widgets::Size::Sm).id_salt("studio-fit").show(ui).clicked() {
                *studio.saved.crop_mut(studio.target) = Crop { turns: studio.crop().turns, ..Crop::default() }.clamped(size, studio.target);
                studio.remember();
            }
            if studio.target != Target::GroupBanner {
                ui.horizontal(|ui| {
                    widgets::toggle(ui, "studio-zones", &mut studio.show_zones);
                    ui.label(egui::RichText::new(tr("Show the zones")).color(theme::FG_2).size(13.0));
                });
            }
        });
        if studio.target != Target::GroupBanner && studio.show_zones {
            legend(ui, studio.target);
        }
        ui.label(egui::RichText::new(tr("Drag to move, scroll to zoom, double-click to start again. Each target keeps its own framing of the same picture.")).color(theme::FG_3).size(12.5));
        previews(ui, cx, studio);
    }
    export_card(ui, studio, busy, acts);
}

fn legend(ui: &mut Ui, target: Target) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(14.0, 6.0);
        let mut seen: Vec<Zone> = Vec::new();
        for (z, _) in target.zones().iter().rev() {
            if seen.contains(z) {
                continue;
            }
            seen.push(*z);
            let txt = Txt::new(zone_name(*z), 12.0).color(theme::FG_2);
            let tw = widgets::text_size(ui, &txt).x;
            let (r, _) = ui.allocate_exact_size(vec2(tw + 22.0, 18.0), Sense::hover());
            let p = ui.painter();
            let sw = Rect::from_min_size(pos2(r.left(), r.center().y - 6.0), vec2(12.0, 12.0));
            p.rect_filled(sw, theme::radius(3.0), zone_colour(Some(*z)).to_opaque().gamma_multiply(0.85));
            widgets::text(p, pos2(r.left() + 20.0, r.center().y), Align2::LEFT_CENTER, &txt);
        }
    });
}

/// Each target made and written on its own: one that fails says so, and the others' files are kept and reported
/// (verify-r22 Minor 3; `art::save_new` leaves no partial file of its own).
fn export_each(targets: Vec<(Target, Crop)>, full: &image::RgbaImage, write: impl Fn(Target, &[u8]) -> std::io::Result<PathBuf>) -> Exported {
    targets
        .into_iter()
        .map(|(t, c)| {
            let one = art::encode_png(&art::render(full, t, c))
                .map_err(|_| tr("The picture could not be made.").to_string())
                .and_then(|png| write(t, &png).map(|path| (path, png.len() as u64)).map_err(|_| tr("The file could not be written.").to_string()));
            (t, one)
        })
        .collect()
}

/// The account's gallery or its Photos & captures, as rows of the page (no scrolling of their own).
fn choosers(ui: &mut Ui, studio: &mut Studio, thumbs: &HashMap<String, egui::TextureHandle>, acts: &mut Vec<Act>) {
    let mut visible_out: Vec<String> = Vec::new();
    choosers_drawn(ui, studio, thumbs, acts, &mut visible_out);
    studio.visible_urls = visible_out;
}

fn choosers_drawn(ui: &mut Ui, studio: &mut Studio, thumbs: &HashMap<String, egui::TextureHandle>, acts: &mut Vec<Act>, visible_out: &mut Vec<String>) {
    let w = ui.available_width();
    if let Some((files, _, more, loading)) = &studio.gallery {
        let (tile_w, tile_h) = (176.0, 99.0);
        let cols = (((w + 10.0) / (tile_w + 10.0)).floor() as usize).max(1);
        ui.label(egui::RichText::new(tr("Your VRChat gallery: press a picture to use it.")).color(theme::FG_2).size(13.0));
        if files.is_empty() && !loading {
            ui.label(egui::RichText::new(tr("Nothing here yet. VRChat+ lets you upload pictures to your gallery and icons.")).color(theme::FG_3).size(12.5));
        }
        let rows = files.len().div_ceil(cols);
        let (area, _) = ui.allocate_exact_size(vec2(w, (rows as f32 * (tile_h + 10.0) - 10.0).max(0.0)), Sense::hover());
        let mut visible = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (tile_w + 10.0), (i / cols) as f32 * (tile_h + 10.0)), vec2(tile_w, tile_h));
            if !ui.is_rect_visible(r.expand(100.0)) {
                continue;
            }
            visible.push(f.url.clone());
            let resp = ui.interact(r, Id::unique(("studio-gallery", f.id.as_str())), Sense::click());
            let p = ui.painter();
            widgets::frame(p, r, 12.0, theme::INK_750);
            if let Some(t) = thumbs.get(&f.url) {
                paint::squircle_texture(p, r, [theme::squircle(12.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
            }
            if resp.hovered() {
                p.rect_stroke(r, theme::radius(12.0), Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
            }
            if widgets::activated(&resp) {
                acts.push(Act::UseInventory(f.clone()));
            }
        }
        visible_out.extend(visible);
        if *loading {
            ui.label(egui::RichText::new(tr("Loading…")).color(theme::FG_3).size(12.5));
        } else if *more && widgets::Button::new(tr("Load more")).size(widgets::Size::Sm).id_salt("studio-gallery-more").show(ui).clicked() {
            acts.push(Act::MoreGallery);
        }
    }
    if let Some(photos) = &studio.photos {
        ui.label(egui::RichText::new(tr("Your newest VRChat pictures: press one to use it.")).color(theme::FG_2).size(13.0));
        if photos.is_empty() {
            ui.label(egui::RichText::new(tr("No pictures found in your VRChat pictures folder.")).color(theme::FG_3).size(12.5));
        }
        let (tile_w, tile_h) = (176.0, 99.0);
        let cols = (((w + 10.0) / (tile_w + 10.0)).floor() as usize).max(1);
        let rows = photos.len().div_ceil(cols);
        let (area, _) = ui.allocate_exact_size(vec2(w, (rows as f32 * (tile_h + 10.0) - 10.0).max(0.0)), Sense::hover());
        for (i, ph) in photos.iter().enumerate() {
            let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (tile_w + 10.0), (i / cols) as f32 * (tile_h + 10.0)), vec2(tile_w, tile_h));
            let resp = ui.interact(r, Id::unique(("studio-photo", i)), Sense::click());
            let p = ui.painter();
            widgets::frame(p, r, 12.0, theme::INK_750);
            if let Some(t) = &ph.texture {
                paint::squircle_texture(p, r, [theme::squircle(12.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
            }
            if resp.hovered() {
                p.rect_stroke(r, theme::radius(12.0), Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
            }
            if widgets::activated(&resp) {
                acts.push(Act::UsePhoto(ph.path.clone()));
            }
        }
    }
}

/// Live previews, labelled as guides: the nameplate in its four looks over a light and a dark world, or the group's card.
fn previews(ui: &mut Ui, cx: &Cx, studio: &mut Studio) {
    let _ = cx;
    let ctx = ui.ctx().clone();
    let Some(loaded) = studio.source.as_ref() else { return };
    let target = studio.target;
    let crop = studio.crop();
    let w = ui.available_width();
    let p = ui.painter().clone();
    let caps = |ui: &mut Ui, text: &str| {
        let (r, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
        widgets::text(ui.painter(), pos2(r.left() + 2.0, r.center().y), Align2::LEFT_CENTER, &Txt::caps(text));
    };
    if target == Target::GroupBanner {
        caps(ui, tr("Preview · the group's card (a guide)"));
        let card_w = w.min(520.0);
        let banner_h = card_w * 9.0 / 16.0;
        let (r, _) = ui.allocate_exact_size(vec2(w, banner_h + 92.0), Sense::hover());
        let card = Rect::from_min_size(r.min, vec2(card_w, banner_h + 92.0));
        widgets::frame(&p, card, 18.0, theme::INK_800);
        let banner = Rect::from_min_size(card.min, vec2(card_w, banner_h));
        draw_crop(&p, banner, loaded, target, crop, Color32::WHITE);
        let icon = Rect::from_min_size(pos2(card.left() + 18.0, banner.bottom() - 30.0), vec2(64.0, 64.0));
        p.circle_filled(icon.center(), 35.0, theme::INK_800);
        widgets::avatar(&p, icon, None, &studio.card.name);
        let name = if studio.card.name.is_empty() { tr("Your group").to_string() } else { studio.card.name.clone() };
        widgets::text_truncated(&p, pos2(icon.right() + 14.0, banner.bottom() + 22.0), Align2::LEFT_CENTER, &Txt::new(&name, 18.0).weight(weight::SEMIBOLD), card_w - 120.0);
        let code = super::short_code(&studio.card.short_code, &studio.card.discriminator);
        widgets::text_truncated(&p, pos2(icon.right() + 14.0, banner.bottom() + 46.0), Align2::LEFT_CENTER, &Txt::new(code, 12.5).color(theme::FG_3), card_w - 120.0);
        return;
    }
    caps(ui, tr("Preview · nameplates (a guide: VRChat has not published where names cut off)"));
    let gap = 12.0;
    // Two side by side (light, dark) on a wide page; one under the other on a narrow one.
    let cols = if w >= 760.0 { 2 } else { 1 };
    let col_w = ((w - gap * (cols - 1) as f32) / cols as f32).floor();
    let plate_h = col_w / target.aspect();
    let looks = [
        (Look { expanded: false, long_name: false }, tr("Collapsed, short name"), SHORT_NAME),
        (Look { expanded: true, long_name: false }, tr("Expanded, short name"), SHORT_NAME),
        (Look { expanded: false, long_name: true }, tr("Collapsed, long name"), LONG_NAME),
        (Look { expanded: true, long_name: true }, tr("Expanded, long name"), LONG_NAME),
    ];
    for (look, caption, player) in looks {
        let rows = 2 / cols;
        let (row, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (plate_h + 26.0) - 4.0), Sense::hover());
        for (i, (world, dark)) in [(LIGHT_WORLD, false), (DARK_WORLD, true)].into_iter().enumerate() {
            let (cx_, cy_) = (i % cols, i / cols);
            let r = Rect::from_min_size(pos2(row.left() + cx_ as f32 * (col_w + gap), row.top() + cy_ as f32 * (plate_h + 26.0)), vec2(col_w, plate_h));
            // The world behind: a soft gradient, then the art, then what this look hides, painted over in the world's colour.
            paint::fill_convex(&p, &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], |at| {
                let t = ((at.y - r.top()) / r.height()).clamp(0.0, 1.0);
                if dark { theme::mix(world, Color32::from_rgb(40, 48, 72), t) } else { theme::mix(world, Color32::from_rgb(240, 236, 226), t) }
            });
            draw_crop(&p, r, loaded, target, crop, Color32::WHITE);
            let kind = 1 | if look.expanded { 2 } else { 0 } | if look.long_name { 4 } else { 0 };
            let m = mask(&ctx, &mut studio.masks, target, kind);
            p.image(m.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), world);
            // The name where VRChat puts it: left on Anchored, in the middle on Centered.
            let size = (r.height() * 0.2).clamp(10.0, 22.0);
            let (at, align) = match target {
                Target::Anchored => (pos2(r.left() + r.width() * 0.055, r.top() + r.height() * 0.42), Align2::LEFT_CENTER),
                _ => (pos2(r.center().x, r.top() + r.height() * 0.33), Align2::CENTER_CENTER),
            };
            let name = Txt::new(player, size).weight(weight::BOLD).color(Color32::WHITE);
            widgets::text(&p, at + vec2(1.0, 1.5), align, &Txt::new(player, size).weight(weight::BOLD).color(theme::black(0.6)));
            widgets::text(&p, at, align, &name);
            widgets::text(&p, pos2(r.left(), r.bottom() + 11.0), Align2::LEFT_CENTER, &Txt::new(format!("{caption} · {}", if dark { tr("dark world") } else { tr("light world") }), 11.5).color(theme::FG_3));
        }
    }
}

fn export_card(ui: &mut Ui, studio: &mut Studio, busy: bool, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let (tw, th) = studio.target.size();
    let has = studio.source.is_some();
    let exporting = studio.exporting.is_some();
    ui.label(egui::RichText::new(orbiters_i18n::tr!("Lossless PNG at {w} × {h}, sRGB, transparency kept; a new file every time, nothing replaced.", w = tw, h = th)).color(theme::FG_3).size(12.5));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if widgets::Button::new(tr("Export this one")).icon(icons::DOWNLOAD_SIMPLE).variant(widgets::Variant::Primary).enabled(has && !exporting).id_salt("studio-export").show(ui).clicked() {
            acts.push(Act::Export(false));
        }
        if widgets::Button::new(tr("Export all three")).icon(icons::DOWNLOAD_SIMPLE).enabled(has && !exporting).id_salt("studio-export-all").show(ui).clicked() {
            acts.push(Act::Export(true));
        }
        if exporting {
            ui.label(egui::RichText::new(tr("Exporting…")).color(theme::FG_3).size(13.0));
        }
    });
    let exported = studio.exported.clone();
    for (t, path, bytes) in exported.iter().rev().take(6) {
        ui.horizontal(|ui| {
            ui.set_width(w);
            ui.label(egui::RichText::new(format!("{} · {} · {}", label(*t), path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), orbiters_ui::format::bytes(*bytes as f64))).color(theme::FG_2).size(12.5));
            if widgets::Button::new(tr("Show in folder")).icon(icons::FOLDER_OPEN).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("studio-reveal", path.to_string_lossy().to_string())).show(ui).clicked() {
                acts.push(Act::Reveal(path.clone()));
            }
        });
    }
    // Onto VRChat: the group banner in two confirmed steps (M6); the nameplate banner by hand on vrchat.com (B4).
    if studio.target == Target::GroupBanner {
        let art = studio.card.abilities().art;
        if !art {
            ui.label(egui::RichText::new(tr("Your role cannot change the group's banner: export it for someone who can.")).color(theme::FG_3).size(12.5));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            // The upload applied from here is only the one made from the art shown now (Blocker 5).
            if studio.upload_matches() {
                let r = widgets::Button::new(tr("Apply as the group's banner")).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(art && !busy).id_salt("studio-apply").show(ui);
                if crate::chat_bar::fresh_activation(ui, &r) {
                    acts.push(Act::Apply(r.rect));
                }
            } else {
                let r = widgets::Button::new(tr("Upload to my gallery, then apply")).icon(icons::UPLOAD_SIMPLE).enabled(art && has && !busy && studio.making_upload.is_none()).id_salt("studio-upload").show(ui);
                if crate::chat_bar::fresh_activation(ui, &r) {
                    acts.push(Act::Upload(r.rect));
                }
            }
        });
        if studio.upload_matches() {
            ui.label(egui::RichText::new(tr("Uploaded to your VRChat gallery. The group is unchanged until you apply it.")).color(theme::WARN).size(12.5));
        } else if studio.uploaded.is_some() {
            ui.label(egui::RichText::new(tr("The art changed since its upload: upload it again to apply it here. The earlier upload can be applied from the group's Look tab, where it is shown.")).color(theme::FG_3).size(12.5));
        }
        if studio.applied {
            ui.label(egui::RichText::new(tr("The group's banner is changed.")).color(theme::ACCENT).size(12.5));
        }
    } else {
        if !orbiters_core::groups::NameplateApply::current().can_apply() {
            ui.label(egui::RichText::new(tr("Direct apply is unavailable in this version. Export both modes and upload them on the group's page on vrchat.com.")).color(theme::WARN).size(12.5));
        }
        if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).id_salt("studio-web").show(ui).clicked() {
            acts.push(Act::Web);
        }
    }
}

fn act_on(ctx: &egui::Context, cx: &mut Cx, state: &mut State, act: Act) {
    let scope = state.scope.clone();
    let Some(studio) = state.studio.as_mut() else { return };
    match act {
        Act::Back => state.studio = None,
        Act::Target(t) => studio.target = t,
        Act::PickFile => {
            if let Some(path) = rfd::FileDialog::new().set_title(tr("Choose a picture")).add_filter(tr("Pictures"), &["png", "jpg", "jpeg", "webp"]).pick_file() {
                let l = load_file(ctx, studio, path);
                studio.loading.push(l);
            }
        }
        Act::Gallery => {
            studio.photos = None;
            if studio.gallery.is_some() {
                studio.gallery = None;
            } else if let Some(scope) = scope {
                studio.gallery = Some((Vec::new(), 0, false, true));
                state.jobs.ask(ctx, cx.ctl, &scope, Kind::Inventory { tag: FileTag::Gallery, offset: 0 }, Request::Inventory { tag: FileTag::Gallery, offset: 0 });
            }
        }
        Act::MoreGallery => {
            if let (Some(g), Some(scope)) = (studio.gallery.as_mut(), scope) {
                g.3 = true;
                let offset = g.1;
                state.jobs.ask(ctx, cx.ctl, &scope, Kind::Inventory { tag: FileTag::Gallery, offset }, Request::Inventory { tag: FileTag::Gallery, offset });
            }
        }
        Act::Photos => {
            studio.gallery = None;
            if studio.photos.is_some() {
                studio.photos = None;
            } else {
                let mut folders: Vec<PathBuf> = orbiters_core::paths::pictures_dir().map(|p| p.join("VRChat")).into_iter().collect();
                folders.extend(cx.snap.settings.picture_folders.iter().cloned());
                studio.photos = Some(Vec::new());
                studio.loading.push(recent_photos(ctx, folders));
            }
        }
        Act::UseInventory(f) => {
            if let Some(scope) = scope {
                studio.gallery = None;
                studio.choice += 1;
                studio.wanted_source = Some(f.url.clone());
                studio.saved.source = Some(Source::Inventory { id: f.id.clone(), url: f.url.clone() });
                state.jobs.ask(ctx, cx.ctl, &scope, Kind::Source(f.url.clone()), Request::Picture { url: f.url });
            }
        }
        Act::UsePhoto(path) => {
            let l = load_file(ctx, studio, path);
            studio.loading.push(l);
        }
        Act::Export(all) => {
            let Some(loaded) = studio.source.as_ref() else { return };
            let full = loaded.full.clone();
            let targets: Vec<(Target, Crop)> = if all { Target::ALL.iter().map(|t| (*t, studio.saved.crop(*t))).collect() } else { vec![(studio.target, studio.crop())] };
            let name = studio.card.name.clone();
            let (tx, rx) = mpsc::channel();
            let ctx2 = ctx.clone();
            // Work of the window while it runs: it does not rest to the tray in the middle of it.
            let working = crate::pages::Working::begin();
            std::thread::spawn(move || {
                let _working = working;
                let dir = art::export_dir();
                let r = export_each(targets, &full, |t, png| art::save_new(&dir, &art::file_stem(&name, t), png));
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
            studio.exporting = Some(rx);
        }
        Act::Reveal(path) => {
            let r = crate::pages::editor::files::reveal(&path);
            cx.run(ctx, r);
        }
        Act::Upload(origin) => {
            let Some(loaded) = studio.source.as_ref() else { return };
            let full = loaded.full.clone();
            let crop = studio.saved.crop(Target::GroupBanner);
            // Made here (it takes a moment), then staged for the service and prepared: the confirmation shows it before
            // anything goes, and the upload is bound to this art and this group (Blocker 5, Major 1).
            let rendered = art::render(&full, Target::GroupBanner, crop);
            let bytes = match art::encode_png(&rendered) {
                Ok(b) => b,
                Err(_) => {
                    studio.error = Some(tr("The picture could not be made.").into());
                    return;
                }
            };
            let staged = match super::stage_upload(&bytes) {
                Ok(s) => s,
                Err(_) => {
                    studio.error = Some(tr("The file could not be written.").into());
                    return;
                }
            };
            let thumb = image::imageops::thumbnail(&rendered, 480, 270);
            let tex = ctx.load_texture("group-upload-preview", egui::ColorImage::from_rgba_unmultiplied([thumb.width() as usize, thumb.height() as usize], thumb.as_raw()), egui::TextureOptions::LINEAR);
            studio.upload_revision = Some(studio.revision());
            studio.uploaded_art = Some(tex.clone());
            let group = studio.card.id.clone();
            state.confirm_art = Some(tex);
            state.prepare(ctx, cx.ctl, Write::Upload { tag: FileTag::Gallery, group, staged }, origin);
        }
        Act::Apply(origin) => {
            if !studio.upload_matches() {
                return;
            }
            if let Some((f, _)) = studio.uploaded.clone() {
                let group = studio.card.id.clone();
                state.confirm_art = studio.uploaded_art.clone();
                state.prepare(ctx, cx.ctl, Write::ApplyArt { group, slot: ArtSlot::Banner, file: f.id }, origin);
            }
        }
        Act::Web => {
            if let Some(url) = orbiters_core::groups::web_page(&studio.card.id) {
                let r = crate::controller::open_url(&url);
                cx.run(ctx, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Studio {
        let card = GroupCard { id: "grp_00000000-0000-4000-8000-0000000000aa".into(), name: "Made-up".into(), ..Default::default() };
        let mut s = Studio::new(card, "usr_00000000-0000-4000-8000-0000000000e1", Some(image::RgbaImage::new(64, 36)), &art::Drafts::default());
        s.saved.source = Some(Source::File("C:/made-up/a.png".into()));
        s
    }

    /// Minor 3: three targets exported, the second's write failing: the first and third are reported done with their
    /// files, the second as failed; nothing of the second is left.
    #[test]
    fn an_export_reports_each_target_on_its_own() {
        let dir = std::env::temp_dir().join(format!("orbiters-export-each-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let full = image::RgbaImage::from_pixel(64, 36, image::Rgba([10, 20, 30, 255]));
        let targets: Vec<(Target, Crop)> = Target::ALL.iter().map(|t| (*t, Crop::default())).collect();
        assert!(targets.len() >= 3);
        let failing = targets[1].0;
        let r = export_each(targets.clone(), &full, |t, png| if t == failing { art::save_new(&dir, "bad\u{0}name", png) } else { art::save_new(&dir, &art::file_stem("Made-up", t), png) });
        assert_eq!(r.len(), targets.len());
        for (t, one) in &r {
            if *t == failing {
                assert!(one.is_err(), "the failed one says so");
            } else {
                let (path, size) = one.as_ref().expect("the others are done and reported");
                assert_eq!(std::fs::metadata(path).unwrap().len(), *size);
            }
        }
        let files = std::fs::read_dir(&dir).unwrap().flatten().count();
        assert_eq!(files, targets.len() - 1, "only the whole files");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Minor 5: a page of the gallery with an unusable entry still advances by what VRChat gave and offers more.
    #[test]
    fn the_gallery_pages_by_what_vrchat_gave() {
        let mut s = studio();
        s.gallery = Some((Vec::new(), 0, true, true));
        let page = |n: usize, raw: usize| orbiters_core::vrchat_api::GroupInventory {
            items: (0..n).map(|i| InventoryFile { id: format!("file_{i}"), url: format!("https://api.vrchat.cloud/api/1/file/file_{i}/1/file") }).collect(),
            raw,
        };
        let full = orbiters_core::vrchat_api::GROUP_PAGE;
        s.answer(Kind::Inventory { tag: FileTag::Gallery, offset: 0 }, Reply::Inventory(page(full - 1, full)));
        let (list, at, more, _) = s.gallery.as_ref().unwrap();
        assert_eq!(list.len(), full - 1);
        assert_eq!(*at, full, "the next page starts after every entry VRChat gave");
        assert!(*more, "a full page from VRChat: there may be more");
    }

    /// Blocker 5: an upload is bound to the art it was made from; moving the crop, turning it or choosing another
    /// picture makes it no longer the one applied from here.
    #[test]
    fn an_upload_is_bound_to_its_art() {
        let mut s = studio();
        let file = InventoryFile { id: "file_a".into(), url: String::new() };
        s.upload_revision = Some(s.revision());
        let prepared = Prepared { op: "op".into(), write: Write::Upload { tag: FileTag::Gallery, group: s.card.id.clone(), staged: crate::controller::groups::Staged { name: "x.png".into(), size: 1, sha256: String::new() } }, confirm: true, role: None, post: None, member: None };
        s.ended(Some(&prepared), &Outcome::Done(crate::controller::groups::Done::Uploaded(file.clone())));
        assert!(s.upload_matches(), "the art it was made from");
        s.set_crop(Target::GroupBanner, Crop { zoom: 2.0, ..Crop::default() });
        assert!(!s.upload_matches(), "a new crop: not this upload");
        s.set_crop(Target::GroupBanner, Crop::default());
        assert!(s.upload_matches());
        s.take_source(image::RgbaImage::new(64, 36));
        assert!(!s.upload_matches(), "another picture: not this upload");
        // An upload done for another group is not this studio's.
        let other = Prepared { write: Write::Upload { tag: FileTag::Gallery, group: "grp_00000000-0000-4000-8000-0000000000ff".into(), staged: crate::controller::groups::Staged { name: "x.png".into(), size: 1, sha256: String::new() } }, ..prepared };
        let before = s.uploaded.clone();
        s.ended(Some(&other), &Outcome::Done(crate::controller::groups::Done::Uploaded(InventoryFile { id: "file_b".into(), url: String::new() })));
        assert_eq!(s.uploaded, before);
    }

    /// Blocker 5: a picture chosen earlier and arriving late never replaces the one chosen after it.
    #[test]
    fn a_late_older_choice_is_dropped() {
        let ctx = egui::Context::default();
        let mut s = studio();
        s.pending = None;
        let (tx_old, rx_old) = mpsc::channel();
        let (tx_new, rx_new) = mpsc::channel();
        s.choice = 2;
        s.loading = vec![Loading::Source(1, rx_old), Loading::Source(2, rx_new)];
        tx_new.send(Ok((image::RgbaImage::new(10, 10), Source::File("C:/made-up/new.png".into())))).unwrap();
        take_loading(&ctx, &mut s);
        assert_eq!(s.saved.source, Some(Source::File("C:/made-up/new.png".into())));
        assert_eq!(s.pending.as_ref().map(|p| p.width()), Some(10));
        // The older one's answer comes after it, at a later frame.
        tx_old.send(Ok((image::RgbaImage::new(20, 20), Source::File("C:/made-up/old.png".into())))).unwrap();
        take_loading(&ctx, &mut s);
        assert_eq!(s.saved.source, Some(Source::File("C:/made-up/new.png".into())), "the newer choice stays");
        assert_eq!(s.pending.as_ref().map(|p| p.width()), Some(10));
        // A gallery picture asked before a newer choice: dropped too.
        s.wanted_source = Some("https://api.vrchat.cloud/api/1/file/file_new/1/file".into());
        s.source_arrived("https://api.vrchat.cloud/api/1/file/file_old/1/file", Some(image::RgbaImage::new(30, 30)));
        assert_eq!(s.pending.as_ref().map(|p| p.width()), Some(10));
    }

    /// Major 6: only the gallery tiles in view are asked for; a gallery larger than the picture cache, idle, asks for
    /// no more than what shows.
    #[test]
    fn only_the_gallery_tiles_in_view_are_asked() {
        let ctx = egui::Context::default();
        let mut s = studio();
        s.gallery = Some(((0..150).map(|i| InventoryFile { id: format!("file_{i}"), url: format!("https://api.vrchat.cloud/api/1/file/file_{i}/1/file") }).collect(), 150, false, false));
        let thumbs = HashMap::new();
        for t in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 500.0))), time: Some(t as f64), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                egui::ScrollArea::vertical().max_height(500.0).show(ui, |ui| {
                    let mut acts = Vec::new();
                    choosers(ui, &mut s, &thumbs, &mut acts);
                });
            });
            out.textures_delta.clear();
        }
        assert!(!s.visible_urls.is_empty());
        assert!(s.visible_urls.len() <= 40, "{} asked of 150", s.visible_urls.len());
    }
}
