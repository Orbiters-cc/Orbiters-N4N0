//! Visited worlds page: the worlds this player visited in VRChat (the tray service keeps them from VRChat's logs), the
//! pictures taken in each and the sessions recorded there. A card flies into the world's page, where pictures are
//! picked and shared on the Orbiters Map explorer, along with a session's 3D preview. Uploads run in the tray service.

use super::sessions::{badge, cover_uv, mini_stat, overall_progress, shimmer_card, stage_label, thumb_shape};
use super::{Cx, Job};
use crate::controller;
use crate::state::{UploadKind, UploadState, UploadStatus};
use crate::worlds::{self, Info, Infos, Picture, Pictures, VisitedWorld, THUMB_WIDTH};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Id, Order, Rect, Sense, TextureHandle, Ui, UiBuilder};
use orbiters_core::account::Server;
use orbiters_core::bundle::SessionSummary;
use orbiters_core::maps::{world_page_url, MineWorld};
use orbiters_core::visited::{self, Shot};
use orbiters_i18n::{fill, mark, tr, tr_n, tr_ctx};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{format, icons, logo, morph, toast};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Pictures in a world's page are at least this wide.
const TILE_W: f32 = 200.0;
/// Pictures are shown this wide in the viewer.
const LARGE_W: u32 = 2048;

struct Open {
    id: String,
    /// The card's picture when it was opened (the flight starts there).
    origin: Rect,
    serial: u64,
    closing: bool,
    /// The flight's progress (0 the card, 1 the page), as the last frame drew it.
    progress: f32,
    hashes: Job<Arc<HashMap<PathBuf, Vec<String>>>>,
    selected: BTreeSet<PathBuf>,
    /// Pictures just handed to the tray service (shown as sending until its upload shows up).
    handed: Option<(f64, BTreeSet<PathBuf>)>,
    /// The picture shown large, by its index in the world's pictures.
    viewer: Option<usize>,
    /// The large picture shrinks back into its tile (dropped once it landed there).
    viewer_closing: bool,
    /// Bumped on every opening of the large picture (its morph's springs).
    viewer_serial: u64,
    /// Each picture's tile as last drawn (by index): the large picture grows out of it and lands back on it.
    tiles: HashMap<usize, Rect>,
    /// Reconstruct again's question, over the page ([`super::reconstruct_again`]).
    again: Option<super::reconstruct_again::Ask>,
}

#[derive(Default)]
pub struct State {
    /// The text searched for at the last frame ([`super::new_search`]).
    searched: Option<String>,
    sharing: Option<super::sharing::Dialog>,
    share_prepare: Job<Vec<crate::controller::shares::Entry>>,
    share_public: bool,
    /// The picture the share window grows out of (a card's, a session row's) and its corner radius.
    share_origin: Option<(Rect, f32)>,
    /// Each card's picture as last drawn (by world id).
    card_thumbs: HashMap<String, Rect>,
    /// Each card as last drawn (by world id): an open world's card rides its picture's flight near its place.
    card_homes: HashMap<String, Rect>,
    /// Screenshots: the share window opens on a fixture state (made-up friends, a share running), never a real share.
    stage_share: Option<&'static str>,
    loaded: Option<(u64, PathBuf, u64)>,
    list: Job<Vec<VisitedWorld>>,
    worlds: Option<Vec<Arc<VisitedWorld>>>,
    query: String,
    /// The worlds the search shows (indices into `worlds`), for the search it was worked out for.
    shown: Option<(String, Vec<usize>)>,
    infos: Option<Infos>,
    pictures: Option<Pictures>,
    open: Option<Open>,
    serial: u64,
    /// Uploads seen finished.
    finished: HashSet<u64>,
}

impl State {
    fn prepare_share(&mut self, ctx: &egui::Context, root: PathBuf, paths: Vec<PathBuf>, public: bool, server: Server, origin: Option<(Rect, f32)>) {
        self.share_public = public;
        self.share_origin = origin;
        self.share_prepare = Job::spawn(ctx, move || {
            crate::controller::shares::local_entries(&root, server)
                .into_iter()
                .filter(|e| e.local.as_ref().is_some_and(|l| paths.iter().any(|p| p == l.path())))
                .collect()
        });
    }
    pub fn stage_detail_share(&self, ctx: &egui::Context) {
        if let Some(s) = self
            .open
            .as_ref()
            .and_then(|o| self.worlds.as_ref()?.iter().find(|w| w.id == o.id))
            .and_then(|w| w.sessions.iter().find(|s| s.reconstruction.is_some()))
        {
            ctx.data_mut(|d| d.insert_temp(Id::unique(("world-session-share", &s.dir)), true));
        }
    }
    /// Opens the share window out of the first card with a 3D view, as Share to … (`public` false) or Share publicly
    /// would, on a fixture state (screenshots).
    pub fn stage_share_window(&mut self, ctx: &egui::Context, root: PathBuf, public: bool) {
        self.stage_share_as(ctx, root, public, if public { "progress" } else { "friends" });
    }

    /// [`Self::stage_share_window`] on the fixture state `fixture` of the sharing window (`sending`: a friend on its way).
    pub fn stage_share_as(&mut self, ctx: &egui::Context, root: PathBuf, public: bool, fixture: &'static str) {
        let Some(w) = self.worlds.as_ref().and_then(|l| l.iter().find(|w| w.reconstructed() > 0)).cloned() else { return };
        let paths = w.sessions.iter().find(|s| s.reconstruction.is_some()).map(|s| vec![s.dir.clone()]).unwrap_or_default();
        let origin = self.card_thumbs.get(&w.id).copied();
        self.stage_share = Some(fixture);
        self.prepare_share(ctx, root, paths, public, Server::Development, origin.map(|o| (o, 20.0)));
    }

    pub fn stage_share_morph(&mut self, ctx: &egui::Context, progress: bool) {
        if let Some(w) = self.worlds.as_ref().and_then(|w| w.iter().find(|w| w.reconstructed() > 0 || !w.shots.is_empty())) {
            let id = Id::unique(("world-share", &w.id));
            ctx.data_mut(|d| d.insert_temp(id, true));
            if progress {
                let _ = orbiters_ui::anim::spring(ctx, id.with("split"), [0.0], Spring::MORPH);
            }
        }
    }

    /// Screenshots: where the first card shown was drawn.
    pub fn first_card(&self) -> Option<Rect> {
        let (_, shown) = self.shown.as_ref()?;
        let world = self.worlds.as_ref()?.get(*shown.first()?)?;
        self.card_homes.get(&world.id).copied()
    }
    pub fn loaded(&mut self) -> bool {
        if let Some(list) = self.list.take() {
            self.worlds = Some(list.into_iter().map(Arc::new).collect());
            self.shown = None;
        }
        self.worlds.is_some()
    }

    /// Whether something is still being read (the worlds, pictures): the screenshots wait for it.
    pub fn busy(&self) -> bool {
        self.busy_now()
    }

    /// A world's page, its Share window or a question is open (`crate::window_rest`).
    pub fn holds(&self) -> bool {
        self.open.is_some() || self.sharing.is_some()
    }

    fn busy_now(&self) -> bool {
        self.list.running() || self.pictures.as_ref().is_some_and(|p| p.loading()) || self.share_prepare.running()
    }

    /// Opens a world as if its card was clicked (screenshots): out of its card's picture when it was drawn.
    pub fn open_world(&mut self, ctx: &egui::Context, id: &str, origin: Rect) {
        let origin = self.card_thumbs.get(id).copied().unwrap_or(origin);
        self.open(ctx, id.to_string(), origin);
    }

    /// Selects `n` pictures after the first `skip` (screenshots).
    pub fn select_some(&mut self, skip: usize, n: usize) {
        let Some(o) = self.open.as_mut() else { return };
        let Some(w) = self.worlds.as_ref().and_then(|l| l.iter().find(|w| w.id == o.id)) else { return };
        o.selected = w.shots.iter().skip(skip).take(n).map(|s| s.path.clone()).collect();
    }

    /// Shows a picture large (screenshots).
    pub fn view_picture(&mut self, index: usize) {
        if let Some(o) = self.open.as_mut() {
            o.show_large(index);
        }
    }

    /// Starts closing the open world's page back into its card (screenshots).
    /// The world whose page is open (screenshots).
    pub fn open_world_id(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.id.as_str())
    }

    pub fn close_world(&mut self) {
        if let Some(o) = self.open.as_mut() {
            o.closing = true;
        }
    }

    /// Starts closing the large picture back into its tile (screenshots).
    pub fn close_picture(&mut self) {
        if let Some(o) = self.open.as_mut() {
            o.viewer_closing = true;
        }
    }

    fn open(&mut self, ctx: &egui::Context, id: String, origin: Rect) {
        self.serial += 1;
        let paths: Vec<PathBuf> =
            self.worlds.as_ref().and_then(|l| l.iter().find(|w| w.id == id)).map(|w| w.shots.iter().map(|s| s.path.clone()).collect()).unwrap_or_default();
        self.open = Some(Open {
            id,
            origin,
            serial: self.serial,
            closing: false,
            progress: 0.0,
            hashes: Job::spawn(ctx, move || Arc::new(worlds::map_hashes(paths))),
            selected: BTreeSet::new(),
            handed: None,
            viewer: None,
            viewer_closing: false,
            viewer_serial: 0,
            tiles: HashMap::new(),
            again: None,
        });
    }
}

impl Open {
    /// Shows picture `index` large, growing out of its tile.
    fn show_large(&mut self, index: usize) {
        self.viewer = Some(index);
        self.viewer_closing = false;
        self.viewer_serial += 1;
    }
}

fn now_secs() -> u64 {
    visited::now()
}

/// The Orbiters the app is set to, as everything else on this page reads it (its world pages, its shares): Open on
/// Orbiters read the account's server instead, which is another one for a moment after the environment is changed.
fn server(cx: &Cx) -> Server {
    if cx.snap.settings.development {
        Server::Development
    } else {
        Server::Production
    }
}

fn vrchat_url(id: &str) -> String {
    format!("https://vrchat.com/home/world/{id}")
}

/// The world's picture: the latest screenshot, else a session's thumbnail, else VRChat's. Large (the page's hero),
/// VRChat's full picture comes before a session's small thumbnail.
fn cover(w: &VisitedWorld, info: Option<&Info>, width: u32) -> Option<Picture> {
    let large = width > THUMB_WIDTH;
    if let Some(s) = w.shots.first() {
        return Some(Picture::Shot { path: s.path.clone(), size: s.size, width });
    }
    let session = w.sessions.iter().find(|s| s.has_thumbnail).map(|s| Picture::Session(s.dir.clone()));
    let remote = match info {
        Some(Info::Listed(p)) => {
            let url = if large {
                p.world.image_url.clone().or_else(|| p.world.thumbnail_url.clone())
            } else {
                p.world.thumbnail_url.clone().or_else(|| p.world.image_url.clone())
            };
            url.map(|url| Picture::Remote { world_id: w.id.clone(), url, large })
        }
        _ => None,
    };
    if large {
        remote.or(session)
    } else {
        session.or(remote)
    }
}

fn mine_of<'a>(mine: &'a Option<Arc<crate::state::MapsMine>>, id: &str) -> Option<&'a MineWorld> {
    mine.as_ref()?.worlds.iter().find(|w| w.world_id == id)
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let page = ui.max_rect();

    // Reload when the library changes, the folder changes, or the page is entered again.
    let key = (cx.ctl.library_version(), cx.snap.settings.sessions_dir.clone(), cx.visit);
    if state.loaded.as_ref() != Some(&key) {
        let entered = state.loaded.as_ref().is_none_or(|k| k.2 != key.2);
        state.loaded = Some(key);
        let dir = cx.snap.settings.sessions_dir.clone();
        state.list = Job::spawn(&ctx, move || worlds::load(&dir));
        if entered {
            cx.ctl.request_maps();
        }
    }
    if let Some(list) = state.list.take() {
        state.worlds = Some(list.into_iter().map(Arc::new).collect());
        state.shown = None;
    }
    state.infos.get_or_insert_with(|| Infos::new(&ctx));
    state.pictures.get_or_insert_with(|| Pictures::new(&ctx));
    // A world shared to gets its page fetched again.
    for u in &cx.snap.uploads {
        if !u.running() && state.finished.insert(u.id) && matches!(u.state, UploadState::Done { .. }) {
            if let Some(infos) = state.infos.as_mut() {
                infos.refresh(&u.world_id);
            }
        }
    }

    let progress = state.open.as_ref().map_or(0.0, |o| o.progress);
    let shown = 1.0 - ease::out_cubic(progress.clamp(0.0, 1.0));
    if shown > 0.01 {
        if let Some((id, origin)) = grid(ui, cx, state, page, shown, state.open.is_none()) {
            state.open(&ctx, id, origin);
        }
    }
    if state.open.is_some() {
        detail(ui, cx, state, page);
    }
    if let Some(mut entries) = state.share_prepare.take() {
        if !entries.is_empty() {
            let first = entries.remove(0);
            let mut d = super::sharing::Dialog::new(&ctx, first, !state.share_public);
            if let Some((origin, radius)) = state.share_origin.take() {
                d = d.from(origin, radius);
            }
            d.extra = entries;
            if let Some(fixture) = state.stage_share.take() {
                d.fixture(fixture);
            } else if state.share_public {
                d.public(&ctx, if cx.snap.settings.development { Server::Development } else { Server::Production });
            }
            state.sharing = Some(d);
        }
    }
    if let Some(d) = state.sharing.as_mut() {
        if d.show(ui, cx) {
            state.sharing = None;
        }
    }
}

/// The grid of worlds and its heading; returns the world clicked and its card's picture.
fn grid(ui: &mut Ui, cx: &mut Cx, state: &mut State, page: Rect, shown: f32, interactive: bool) -> Option<(String, Rect)> {
    let ctx = ui.ctx().clone();
    let head = Rect::from_min_size(pos2(page.left() + super::PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - super::PAD_X * 2.0, 72.0));
    let actions_y = head.bottom() - 40.0;
    let folder_r = Rect::from_min_size(pos2(head.right() - 36.0, actions_y + 2.0), vec2(36.0, 36.0));
    let search_r = Rect::from_min_size(pos2(folder_r.left() - 8.0 - 224.0, actions_y), vec2(224.0, 40.0));
    let grid_top = head.bottom() + 20.0;
    let below_head = Rect::from_min_max(pos2(page.left(), grid_top), page.max);

    let mine = cx.ctl.maps();
    let q = state.query.trim().to_lowercase();
    if state.shown.as_ref().is_none_or(|(asked, _)| *asked != q) {
        let find = orbiters_core::search::Query::new(&q);
        let shown = state
            .worlds
            .as_ref()
            .map(|l| l.iter().enumerate().filter(|(_, w)| find.matches(&w.name) || (!q.is_empty() && w.id.contains(&q))).map(|(i, _)| i).collect())
            .unwrap_or_default();
        state.shown = Some((q.clone(), shown));
    }
    // The worlds the search shows are read by their place in the list: only the cards in view take a reference, where
    // every world of the list was copied into a new list at each frame.
    let shown_count = match (&state.shown, &state.worlds) {
        (Some((_, shown)), Some(_)) => shown.len(),
        _ => 0,
    };
    let shown_world = |state: &State, i: usize| -> Option<Arc<VisitedWorld>> {
        let (_, shown) = state.shown.as_ref()?;
        state.worlds.as_ref()?.get(*shown.get(i)?).cloned()
    };
    let mut clicked = None;
    let mut share_clicked: Option<(Vec<PathBuf>, bool, Rect)> = None;
    let mut menu_picked: Option<(CardAct, Arc<VisitedWorld>)> = None;
    let from = orbiters_ui::blur::next_shape(ui);
    let mut gu = ui.new_child(UiBuilder::new().max_rect(page).id_salt("worlds-grid"));
    gu.multiply_opacity(shown);
    let mut scroll = egui::ScrollArea::vertical().id_salt("worlds");
    if super::new_search(&mut state.searched, &q) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.auto_shrink([false, false]).scroll_bar_rect(below_head).show(&mut gu, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 32 }).show(ui, |ui| {
            ui.add_space(grid_top - page.top());
            let w = ui.available_width();
            if state.worlds.is_some() && shown_count == 0 {
                empty_state(ui, cx, !q.is_empty(), below_head.height() * 0.6);
                return;
            }
            let cols = (((w + 16.0) / (270.0 + 16.0)).floor() as usize).max(1);
            let card_w = (w - 16.0 * (cols - 1) as f32) / cols as f32;
            let card_h = card_w * 9.0 / 16.0 + 146.0;
            let count = if state.worlds.is_none() { 6 } else { shown_count };
            let rows = count.div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + 16.0) - 16.0), Sense::hover());
            for i in 0..count {
                let r = Rect::from_min_size(
                    pos2(area.left() + (i % cols) as f32 * (card_w + 16.0), area.top() + (i / cols) as f32 * (card_h + 16.0)),
                    vec2(card_w, card_h),
                );
                if !ui.is_rect_visible(r) {
                    continue;
                }
                let Some(world) = shown_world(state, i) else {
                    shimmer_card(ui, r);
                    continue;
                };
                let world = &world;
                let info = state.infos.as_mut().and_then(|i| i.get(&world.id));
                let tex = cover(world, info.as_ref(), THUMB_WIDTH).and_then(|p| state.pictures.as_mut()?.get(&ctx, &p));
                let open_here = state.open.as_ref().is_some_and(|o| o.id == world.id);
                let shared = mine_of(&mine, &world.id).is_some_and(|m| m.preview.is_some() || !m.pictures.is_empty());
                state.card_thumbs.insert(world.id.clone(), Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0)));
                state.card_homes.insert(world.id.clone(), r);
                let (pressed, resp) = world_card(ui, cx, i, world, r, tex.as_ref(), info.as_ref(), shared, if open_here { 0.0 } else { 1.0 }, interactive);
                if let Some(cover_r) = pressed {
                    clicked = Some((world.id.clone(), cover_r));
                }
                // A right press: what the world offers.
                let acts = || {
                    let sharable = (world.reconstructed() > 0 || !world.shots.is_empty()) && !state.share_prepare.running();
                    if interactive && !open_here { card_acts(world, sharable) } else { Vec::new() }
                };
                if let Some((_, act)) = orbiters_ui::popup::context_menu(ui, &resp, "world-card", acts, |(i, _)| *i) {
                    let thumb = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0));
                    match act {
                        CardAct::Open => clicked = Some((world.id.clone(), thumb)),
                        CardAct::Share(public) => share_clicked = Some((share_paths(world), public, thumb)),
                        act => menu_picked = Some((act, world.clone())),
                    }
                }
                // Faded in once the grid takes presses again (after a world's page landed back on its card).
                let pill = anim::toggle(&ctx, Id::unique(("world-share-shown", &world.id)), !open_here && interactive, Spring::SOFT);
                if pill > 0.01 && (world.reconstructed() > 0 || !world.shots.is_empty()) {
                    let mut pu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("world-share-pill", &world.id)));
                    pu.multiply_opacity(pill);
                    if let Some(public) = super::sharing::morph(
                        &mut pu,
                        Id::unique(("world-share", &world.id)),
                        pos2(r.right() - 12.0, r.bottom() - 50.0),
                        r.width() - 24.0,
                        !state.share_prepare.running() && interactive && !open_here,
                    ) {
                        share_clicked = Some((share_paths(world), public, Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0))));
                    }
                }
            }
        });
    });
    if let Some((act, world)) = menu_picked {
        card_act(&ctx, cx, act, &world);
    }
    if let Some((paths, public, origin)) = share_clicked {
        state.prepare_share(
            &ctx,
            cx.snap.settings.sessions_dir.clone(),
            paths,
            public,
            if cx.snap.settings.development { Server::Development } else { Server::Production },
            Some((origin, 20.0)),
        );
    }
    super::under_top(&gu, cx, "worlds", from, grid_top);

    let sub = match &state.worlds {
        Some(l) => {
            let pictures: usize = l.iter().map(|w| w.shots.len()).sum();
            let in_3d = l.iter().filter(|w| w.reconstructed() > 0).count();
            let worlds = tr_n("{count} world", "{count} worlds", l.len() as u64);
            let pictures = fill(&tr_n("{total} picture", "{total} pictures", pictures as u64), &[("total", &format::integer(pictures as f64))]);
            let mut s = format!("{worlds} · {pictures}");
            if in_3d > 0 {
                s.push_str(&format!(" · {}", tr!("{count} in 3D", count = in_3d)));
            }
            s
        }
        None => tr("Reading VRChat's logs and pictures…").into(),
    };
    let mut hu = ui.new_child(UiBuilder::new().max_rect(page).id_salt("worlds-head"));
    hu.multiply_opacity(shown);
    let (head_r, head_a) = super::entering(&hu, cx, 0, head);
    {
        let mut tu = hu.new_child(UiBuilder::new().max_rect(head_r).id_salt("worlds-title"));
        tu.multiply_opacity(head_a);
        super::heading(&tu, head_r, "VRChat", tr("Visited worlds"), Some(&sub));
    }
    // The search and the folder fade back in once the grid takes presses again.
    let actions = anim::toggle(&ctx, Id::unique("worlds-head-actions"), interactive, Spring::SOFT);
    if actions > 0.01 {
        let mut au = hu.new_child(UiBuilder::new().max_rect(page).id_salt("worlds-head-actions"));
        au.multiply_opacity(actions);
        if !interactive {
            au.disable();
        }
        let mut su = au.new_child(UiBuilder::new().max_rect(search_r).id_salt("worlds-search"));
        widgets::search_field(&mut su, "worlds-search", &mut state.query, tr("Search worlds"), 224.0);
        if IconButton::new(icons::IMAGES, mark("Open VRChat's pictures folder")).show_at(&mut au, folder_r).clicked() {
            if let Some(dir) = visited::screenshot_dirs().into_iter().next() {
                let r = controller::open_path(&dir);
                cx.run(&ctx, r);
            }
        }
    }
    clicked
}

/// What a card's share offers: its first 3D capture, else its pictures.
fn share_paths(world: &VisitedWorld) -> Vec<PathBuf> {
    world
        .sessions
        .iter()
        .find(|s| s.reconstruction.is_some())
        .map(|s| vec![s.dir.clone()])
        .unwrap_or_else(|| world.shots.iter().map(|s| s.path.clone()).collect())
}

/// What a card's right-click menu does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CardAct {
    /// The world's page (as a press on the card).
    Open,
    OpenVrchat,
    /// VRChat's page that launches the world.
    Launch,
    CopyId,
    CopyLink,
    /// Share to … (`false`) or Share publicly, as the card's share pill.
    Share(bool),
    /// The folder of its newest picture.
    Pictures,
}

/// The right-click menu of `world`'s card; `sharable`: it has a 3D capture or pictures and no share is being prepared.
fn card_acts(world: &VisitedWorld, sharable: bool) -> Vec<(orbiters_ui::popup::Item<'static>, CardAct)> {
    use orbiters_ui::popup::Item;
    let mut acts = vec![
        (Item::new(tr("Open the world's page")).icon(icons::ARROW_RIGHT), CardAct::Open),
        (Item::new(tr("Open on VRChat")).icon(icons::ARROW_SQUARE_OUT), CardAct::OpenVrchat),
        (Item::new(tr("Launch in VRChat")).icon(icons::PLAY), CardAct::Launch),
        (Item::new(tr("Copy the world's ID")).icon(icons::COPY), CardAct::CopyId),
        (Item::new(tr("Copy its VRChat link")).icon(icons::LINK_SIMPLE), CardAct::CopyLink),
    ];
    if sharable {
        acts.push((Item::new(tr("Share to…")).icon(icons::PAPER_PLANE_TILT), CardAct::Share(false)));
        acts.push((Item::new(tr("Share publicly")).icon(icons::GLOBE), CardAct::Share(true)));
    }
    if !world.shots.is_empty() {
        let show_pictures = if cfg!(target_os = "macos") {
            tr("Show its pictures in Finder")
        } else if orbiters_core::here::LINUX {
            tr("Show its pictures in the file manager")
        } else {
            tr("Show its pictures in Explorer")
        };
        acts.push((Item::new(show_pictures).icon(icons::FOLDER_OPEN), CardAct::Pictures));
    }
    acts
}

fn card_act(ctx: &egui::Context, cx: &mut Cx, act: CardAct, world: &VisitedWorld) {
    match act {
        CardAct::OpenVrchat => {
            let r = controller::open_url(&vrchat_url(&world.id));
            cx.run(ctx, r);
        }
        CardAct::Launch => {
            let r = controller::open_url(&format!("https://vrchat.com/home/launch?worldId={}", world.id));
            cx.run(ctx, r);
        }
        CardAct::CopyId => {
            ctx.copy_text(world.id.clone());
            cx.toasts.push(ctx, toast::Kind::Success, tr("World ID copied"));
        }
        CardAct::CopyLink => {
            ctx.copy_text(vrchat_url(&world.id));
            cx.toasts.push(ctx, toast::Kind::Success, tr("Link copied"));
        }
        CardAct::Pictures => {
            if let Some(dir) = world.shots.first().and_then(|s| s.path.parent()) {
                let r = controller::open_path(dir);
                cx.run(ctx, r);
            }
        }
        CardAct::Open | CardAct::Share(_) => {}
    }
}

/// One card; returns its picture's rect when clicked, and its response (its right-click menu).
#[allow(clippy::too_many_arguments)]
fn world_card(
    ui: &mut Ui,
    cx: &Cx,
    i: usize,
    w: &VisitedWorld,
    rect: Rect,
    tex: Option<&TextureHandle>,
    info: Option<&Info>,
    shared: bool,
    shown: f32,
    interactive: bool,
) -> (Option<Rect>, egui::Response) {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("world-card", &w.id));
    let body = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 52.0));
    let resp = ui.interact(body, id, if interactive { Sense::click() } else { Sense::hover() });
    let resp = if interactive { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    // Entrance: up and in, 35 ms apart.
    let delay = 0.05 + i.min(12) as f64 * 0.035;
    let elapsed = anim::time(&ctx) - cx.entered;
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
    }
    let enter = anim::spring_from(&ctx, id.with(("enter", cx.visit)), [0.0], [if elapsed >= delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() && interactive, Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on() && interactive, Spring::SNAPPY);
    // A share window landing back on its picture bounces it.
    let landing = morph::bounce(&ctx, Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 9.0 / 16.0)));
    let scale = (0.97 + 0.03 * enter) * (1.0 - 0.03 * press) * landing;
    let r = Rect::from_center_size(rect.center() + vec2(0.0, 20.0 * (1.0 - enter) - 4.0 * hover), rect.size() * scale);
    let mut p = ui.painter().clone();
    p.multiply_opacity(enter.clamp(0.0, 1.0) * shown);

    widgets::frame(&p, r, theme::CARD_RADIUS, theme::surface());
    let thumb = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0));
    thumb_shape(&p, thumb, CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 }, tex, 1.0 + 0.06 * ease::out_cubic(hover));
    let shade = paint::rounded_rect_points_corners(thumb, [20.0, 20.0, 0.0, 0.0]);
    paint::fill_gradient(
        &p,
        &shade,
        &Gradient::vertical(thumb, &[(0.0, Color32::TRANSPARENT), (0.55, theme::alpha(theme::ink_950(), 0.1)), (1.0, theme::alpha(theme::ink_950(), 0.9))]),
    );
    let mut x = thumb.left() + 12.0;
    if w.reconstructed() > 0 {
        x += badge(&p, ui, pos2(x, thumb.top() + 12.0), "3D", theme::black(0.55), theme::accent(), false) + 6.0;
    }
    if !w.shots.is_empty() {
        badge(&p, ui, pos2(x, thumb.top() + 12.0), &tr_n("{count} picture", "{count} pictures", w.shots.len() as u64), theme::black(0.55), theme::fg_2(), false);
    }
    if shared {
        badge(&p, ui, pos2(thumb.right() - 12.0, thumb.top() + 12.0), tr("On Orbiters"), theme::alpha(theme::OK, 0.2), theme::OK, true);
    }

    let x = r.left() + 16.0;
    widgets::text_truncated(&p, pos2(x, thumb.bottom() + 22.0), Align2::LEFT_CENTER, &Txt::new(&w.name, 15.0).weight(weight::SEMIBOLD), r.width() - 32.0);
    let when = if w.visits > 0 {
        tr!("Visited {when}", when = crate::pages::ago(ui.ctx(), w.last, now_secs()))
    } else {
        tr!("Last seen {when}", when = crate::pages::ago(ui.ctx(), w.last, now_secs()))
    };
    let author = match info {
        Some(Info::Listed(page)) => page.world.author_name.clone(),
        _ => None,
    };
    let line = match author {
        Some(author) => tr!("{when} · by {author}", when = when, author = author),
        None => when,
    };
    widgets::text_truncated(
        &p,
        pos2(x, thumb.bottom() + 42.0),
        Align2::LEFT_CENTER,
        &Txt::new(line, 12.0).color(theme::fg_3()),
        r.width() - 32.0,
    );
    let mut px = x;
    let py = thumb.bottom() + 68.0;
    let mut pills: Vec<(Option<&str>, String)> = Vec::new();
    if w.visits > 0 {
        pills.push((Some(icons::CLOCK), tr_n("{count} visit", "{count} visits", w.visits as u64)));
    }
    if !w.sessions.is_empty() {
        pills.push((Some(icons::CUBE), tr_n("{count} capture", "{count} captures", w.sessions.len() as u64)));
    }
    if let Some(Info::Listed(page)) = info {
        if let Some(c) = page.world.capacity.filter(|c| *c > 0) {
            pills.push((Some(icons::USERS), c.to_string()));
        }
    }
    for (glyph, label) in pills {
        let pw = widgets::pill_at(&p, ui, pos2(px, py), glyph, &label);
        if px + pw > r.right() - 12.0 {
            break;
        }
        px += pw + 6.0;
    }
    (resp.clicked().then_some(thumb), resp)
}

fn empty_state(ui: &mut Ui, cx: &Cx, searching: bool, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(240.0)), Sense::hover());
    let (rect, alpha) = super::entering(ui, cx, 1, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let t = anim::clock(ui.ctx(), 30.0);
    let bob = -8.0 * ease::in_out(ease::mirror(t, 2.0));
    let lr = Rect::from_center_size(rect.center() + vec2(0.0, -44.0 + bob), logo::size_for_width(70.0));
    logo::paint(&p, lr, logo::Look::default());
    let (title, text) = if searching {
        (tr("No world matches"), tr("Try another name."))
    } else if !cx.snap.vrchat.log_found {
        (tr("VRChat was not found"), tr("Worlds show up here once VRChat ran on this computer."))
    } else {
        (tr("No world yet"), tr("Worlds you visit in VRChat show up here, with the pictures you take and the sessions you record there."))
    };
    widgets::text(&p, rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 13.0).color(theme::fg_3()).line_height(20.0).job(380.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y + 34.0), g, theme::fg_3());
}

/// What the world's page shares and how.
struct Sharing<'a> {
    /// Signed in to Orbiters (or an account saved but not checked yet).
    account: bool,
    /// Signing in: finishing in the browser.
    signing_in: bool,
    /// Why nothing can be shared, when nothing can.
    blocked: Option<&'static str>,
    mine: Option<&'a MineWorld>,
    hashes: Option<&'a HashMap<PathBuf, Vec<String>>>,
    uploads: Vec<&'a UploadStatus>,
}

impl Sharing<'_> {
    fn shared(&self, shot: &Shot) -> bool {
        let (Some(mine), Some(hashes)) = (self.mine, self.hashes) else { return false };
        hashes.get(&shot.path).is_some_and(|h| mine.pictures.iter().any(|p| h.contains(&p.source_hash)))
    }

    fn sending(&self, path: &PathBuf) -> Option<&UploadStatus> {
        self.uploads.iter().copied().find(|u| u.running() && u.kind == UploadKind::Pictures && u.paths.contains(path))
    }

    fn preview_upload(&self, session: &PathBuf) -> Option<&UploadStatus> {
        self.uploads.iter().copied().filter(|u| u.kind == UploadKind::Preview && u.paths.first() == Some(session)).max_by_key(|u| u.id)
    }
}

fn detail(ui: &mut Ui, cx: &mut Cx, state: &mut State, page: Rect) {
    let ctx = ui.ctx().clone();
    // The world's card where the grid last drew it: the page's picture flies out of its picture and back.
    let home = state.open.as_ref().and_then(|o| state.card_homes.get(&o.id)).copied();
    // The Share window over the page takes Escape (drawn after the page, it closes on the same press).
    let share_open = state.sharing.is_some();
    let Some(o) = state.open.as_mut() else { return };
    if let Some(h) = home {
        o.origin = Rect::from_min_size(h.min, vec2(h.width(), h.width() * 9.0 / 16.0));
    }
    let Some(world) = state.worlds.as_ref().and_then(|l| l.iter().find(|w| w.id == o.id)).cloned() else {
        state.open = None;
        return;
    };
    let id = Id::unique(("world-page", o.serial));
    let travel = morph::travel(&ctx, id.with("flight"), !o.closing, Spring::MORPH);
    let t = travel.t;
    o.progress = t;
    if travel.done {
        state.open = None;
        if let Some(p) = state.pictures.as_mut() {
            p.forget_large();
        }
        return;
    }
    let content = ((t - 0.45) / 0.55).clamp(0.0, 1.0);
    let info = state.infos.as_mut().and_then(|i| i.get(&world.id));
    let mine = cx.ctl.maps();
    let hashes = o.hashes.poll().cloned();
    let blocked = match &info {
        Some(Info::Unlisted) => Some(tr("Orbiters lists public worlds only: this one is private or was removed from VRChat.")),
        _ => None,
    };
    let sharing = Sharing {
        account: cx.snap.account.state.username().is_some(),
        signing_in: cx.snap.account.state == crate::state::AccountState::SigningIn,
        blocked,
        mine: mine_of(&mine, &world.id),
        hashes: hashes.as_deref(),
        uploads: cx.snap.uploads.iter().filter(|u| u.world_id == world.id).collect(),
    };
    // Handed pictures show as sending until the service lists their upload (or for three seconds).
    if let Some((at, paths)) = &o.handed {
        if anim::time(&ctx) - at > 3.0 || paths.iter().any(|p| sharing.sending(p).is_some()) {
            o.handed = None;
        }
    }

    let mut close = false;
    let mut toggle: Option<PathBuf> = None;
    let mut view: Option<usize> = None;
    let mut select_new = false;
    let mut clear = false;
    let mut share_preview: Option<(PathBuf, bool, Rect)> = None;
    let mut reconstruct: Option<(PathBuf, Rect)> = None;
    let selecting = !o.selected.is_empty();

    let from = orbiters_ui::blur::next_shape(ui);
    let mut du = ui.new_child(UiBuilder::new().max_rect(page).id_salt(("world-page", o.serial)));
    egui::ScrollArea::vertical().id_salt(("world-scroll", o.serial)).auto_shrink([false, false]).scroll_bar_rect(super::below_bar(&du, cx)).show(
        &mut du,
        |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 120 }).show(ui, |ui| {
                ui.add_space(cx.top + 8.0);
                let w = ui.available_width();

                // The back button's place at rest: it floats there, over the scrolling page ([`back_rect`]).
                ui.add_space(BACK_H + 12.0);

                // The hero flies in from the card.
                let hero_h = (w * 0.36).clamp(220.0, 340.0);
                let (hero_r, _) = ui.allocate_exact_size(vec2(w, hero_h), Sense::hover());
                let e = t.clamp(0.0, 1.2);
                let lerp = |a: f32, b: f32| a + (b - a) * e;
                let flight = Rect::from_min_max(
                    pos2(lerp(o.origin.left(), hero_r.left()), lerp(o.origin.top(), hero_r.top())),
                    pos2(lerp(o.origin.right(), hero_r.right()), lerp(o.origin.bottom(), hero_r.bottom())),
                );
                let pictures = state.pictures.as_mut();
                let (small, large) = match pictures {
                    Some(p) => (
                        cover(&world, info.as_ref(), THUMB_WIDTH).and_then(|c| p.get(&ctx, &c)),
                        if t > 0.6 { cover(&world, info.as_ref(), 1280).and_then(|c| p.get(&ctx, &c)) } else { None },
                    ),
                    None => (None, None),
                };
                // Near its card, the card itself rides under the flying picture, which fades out over it as it lands
                // (in as it leaves); landed, the card bounces.
                if let Some(home) = home.filter(|_| morph::source_alpha(t) > 0.001) {
                    let ride = morph::ride(home, o.origin, flight, travel.landing);
                    let shared = sharing.mine.is_some_and(|m| m.preview.is_some() || !m.pictures.is_empty());
                    let mut ru = ui.new_child(UiBuilder::new().max_rect(home).id_salt("riding-card"));
                    ru.multiply_opacity(morph::source_alpha(t));
                    ru.with_visual_transform(ride, |ru| world_card(ru, cx, 0, &world, home, small.as_ref(), info.as_ref(), shared, 1.0, false));
                }
                let mut hu = ui.new_child(UiBuilder::new().max_rect(ui.max_rect()).id_salt("hero"));
                hu.multiply_opacity(morph::shape_alpha(t));
                hero(&mut hu, cx, &world, info.as_ref(), flight, small.as_ref(), large.as_ref(), t, content);
                ui.add_space(20.0);

                let mut cu = ui.new_child(UiBuilder::new().max_rect(ui.available_rect_before_wrap()).id_salt("world-body"));
                cu.multiply_opacity(content);
                let dy = 14.0 * (1.0 - content);

                // Stats.
                let (stats_r, _) = ui.allocate_exact_size(vec2(w, 78.0), Sense::hover());
                let stats_r = stats_r.translate(vec2(0.0, dy));
                // Visits count from when the app started keeping them; a world only seen in pictures or sessions has none.
                let (visits, last_visit) = if world.visits > 0 {
                    (world.visits.to_string(), Some(tr!("last {when}", when = crate::pages::ago(ui.ctx(), world.last, now_secs()))))
                } else {
                    ("–".to_string(), Some(tr("before Orbiters kept them").to_string()))
                };
                let shared_sub = match (world.shots.is_empty(), sharing.mine) {
                    (true, _) => None,
                    (false, Some(m)) => Some(tr!("{count} shared", count = m.pictures.len())),
                    (false, None) => Some(tr("none shared").to_string()),
                };
                let minis = [
                    (icons::CLOCK, tr("Visits"), visits, last_visit),
                    (icons::IMAGES, tr("Pictures"), world.shots.len().to_string(), shared_sub),
                    (icons::CUBE, tr("Captures"), world.sessions.len().to_string(), Some(tr!("{count} in 3D", count = world.reconstructed()))),
                    (
                        icons::USERS,
                        tr("Players"),
                        match &info {
                            Some(Info::Listed(p)) => p.world.capacity.map(|c| c.to_string()).unwrap_or_else(|| "–".into()),
                            _ => "–".into(),
                        },
                        match &info {
                            Some(Info::Listed(p)) => p.world.visits.map(|v| fill(&tr_n("{total} visit on VRChat", "{total} visits on VRChat", v), &[("total", &compact(v))])),
                            _ => None,
                        },
                    ),
                    (
                        icons::GAUGE,
                        tr_ctx("stat tile", "Frame rate"),
                        match &info {
                            Some(Info::Listed(p)) => p.world.performance.as_ref().map(|f| format!("{} fps", f.fps)).unwrap_or_else(|| "–".into()),
                            _ => "–".into(),
                        },
                        match &info {
                            Some(Info::Listed(p)) => Some(match &p.world.performance {
                                Some(f) => tr!("{percent} % of the headset's rate", percent = format!("{:.0}", f.ratio * 100.0)),
                                None => tr("not measured yet").into(),
                            }),
                            _ => None,
                        },
                    ),
                ];
                let mw = (w - 48.0) / 5.0;
                for (i, (glyph, label, value, sub)) in minis.iter().enumerate() {
                    let r = Rect::from_min_size(pos2(stats_r.left() + i as f32 * (mw + 12.0), stats_r.top()), vec2(mw, 78.0));
                    mini_stat(cu.painter(), r, glyph, label, value, sub.as_deref());
                }
                ui.add_space(32.0);

                // 3D preview: the sessions recorded here.
                let (title_r, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
                let preview_sub = match sharing.mine.and_then(|m| m.preview.as_ref()) {
                    Some(p) => tr!("Your 3D preview is on the Map explorer · {count} splats", count = compact(p.splats as u64)),
                    None => tr("Share a reconstructed session: the world opens in 3D on the Map explorer.").into(),
                };
                section_title(&cu, title_r.translate(vec2(0.0, dy)), tr("3D preview"), &preview_sub);
                ui.add_space(8.0);
                if world.sessions.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
                    note(
                        &cu,
                        r.translate(vec2(0.0, dy)),
                        icons::CUBE,
                        tr("No session recorded here yet"),
                        tr("Record this world from the capture bar of Photos & captures with the capture kit's depth frames on, then come back to share it in 3D."),
                    );
                }
                for s in &world.sessions {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 84.0), Sense::hover());
                    if !cu.is_rect_visible(r) {
                        ui.add_space(10.0);
                        continue;
                    }
                    let tex = state.pictures.as_mut().and_then(|p| if s.has_thumbnail { p.get(&ctx, &Picture::Session(s.dir.clone())) } else { None });
                    let row = r.translate(vec2(0.0, dy));
                    match session_row(&mut cu, cx, row, s, tex.as_ref(), &sharing) {
                        Some(RowAction::Share(public)) => share_preview = Some((s.dir.clone(), public, row_thumb(row))),
                        Some(RowAction::Reconstruct(origin)) => reconstruct = Some((s.dir.clone(), origin)),
                        Some(RowAction::Stop) => {
                            cx.ctl.stop_reconstruction();
                        }
                        Some(RowAction::Unqueue) => cx.ctl.unqueue_reconstruction(s.dir.clone()),
                        None => {}
                    }
                    ui.add_space(10.0);
                }
                ui.add_space(24.0);

                // Pictures.
                let (title_r, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
                let title_r = title_r.translate(vec2(0.0, dy));
                let new: Vec<&Shot> = world.shots.iter().filter(|s| !sharing.shared(s)).collect();
                let shared_n = world.shots.len() - new.len();
                let pictures_sub = if world.shots.is_empty() {
                    tr("Pictures you take here in VRChat show up here.").to_string()
                } else if shared_n > 0 {
                    tr!("{count} on this computer · {shared} shared on the Map explorer", count = world.shots.len(), shared = shared_n)
                } else {
                    tr!("{count} on this computer · pick the ones to show on the Map explorer", count = world.shots.len())
                };
                section_title(&cu, title_r, tr("Pictures"), &pictures_sub);
                if !new.is_empty() && sharing.blocked.is_none() {
                    let label = if selecting { tr("Clear selection") } else { tr("Select all new") };
                    let b = Button::new(label).variant(Variant::Ghost).size(Size::Sm);
                    let bw = b.desired_width(&cu);
                    if b.show_at(&mut cu, Rect::from_min_size(pos2(title_r.right() - bw, title_r.top() + 4.0), vec2(bw, 32.0))).clicked() {
                        if selecting {
                            clear = true;
                        } else {
                            select_new = true;
                        }
                    }
                }
                ui.add_space(8.0);
                if !world.shots.is_empty() {
                    let cols = (((w + 12.0) / (TILE_W + 12.0)).floor() as usize).max(1);
                    let tile_w = (w - 12.0 * (cols - 1) as f32) / cols as f32;
                    let tile_h = tile_w * 9.0 / 16.0;
                    let rows = world.shots.len().div_ceil(cols);
                    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (tile_h + 12.0) - 12.0), Sense::hover());
                    for (i, shot) in world.shots.iter().enumerate() {
                        let r = Rect::from_min_size(
                            pos2(area.left() + (i % cols) as f32 * (tile_w + 12.0), area.top() + (i / cols) as f32 * (tile_h + 12.0)),
                            vec2(tile_w, tile_h),
                        )
                        .translate(vec2(0.0, dy));
                        if !cu.is_rect_visible(r) {
                            continue;
                        }
                        let tex =
                            state.pictures.as_mut().and_then(|p| p.get(&ctx, &Picture::Shot { path: shot.path.clone(), size: shot.size, width: THUMB_WIDTH }));
                        let handed = o.handed.as_ref().is_some_and(|(_, h)| h.contains(&shot.path));
                        let look = TileLook {
                            selected: o.selected.contains(&shot.path),
                            selecting,
                            shared: sharing.shared(shot),
                            sending: handed || sharing.sending(&shot.path).is_some(),
                            selectable: sharing.blocked.is_none(),
                        };
                        o.tiles.insert(i, r);
                        // The large picture stands for its tile while it is out (it lands exactly on it).
                        if o.viewer == Some(i) {
                            continue;
                        }
                        match picture_tile(&mut cu, r, shot, tex.as_ref(), look, i) {
                            Some(TileAction::Toggle) => toggle = Some(shot.path.clone()),
                            Some(TileAction::View) => view = Some(i),
                            None => {}
                        }
                    }
                }
            });
        },
    );
    // Its own band: while the card flies open or closed, the grid draws one in the same frame.
    super::under_top(&du, cx, "world", from, page.top() + cx.top);

    // Back: over the page wherever it is scrolled, at the place it has at rest, and under whatever the window draws over
    // the page (the picture viewer, a share's window, toasts: they are layers above this one). Filled and lifted off what
    // passes under it, a photo or the page's own text; a ghost button would vanish on photos.
    {
        let mut bu = du.new_child(UiBuilder::new().max_rect(page).id_salt("back"));
        bu.multiply_opacity(content);
        let b = Button::new(tr("Visited worlds")).icon(icons::ARROW_LEFT).variant(Variant::Glass).size(Size::Sm);
        let r = back_rect(page, cx.top, b.desired_width(&bu));
        orbiters_ui::paint::shadow(bu.painter(), r, BACK_H * 0.5, vec2(0.0, 4.0), 16.0, 0.0, theme::black(0.4));
        if b.show_at(&mut bu, r).clicked() {
            close = true;
        }
    }

    // Actions gathered while drawing.
    let Some(o) = state.open.as_mut() else { return };
    if let Some(p) = toggle {
        if !o.selected.remove(&p) {
            o.selected.insert(p);
        }
    }
    if select_new {
        o.selected = world.shots.iter().filter(|s| !sharing.shared(s) && sharing.sending(&s.path).is_none()).map(|s| s.path.clone()).collect();
    }
    if clear {
        o.selected.clear();
    }
    if let Some(i) = view {
        o.show_large(i);
    }
    if let Some((dir, origin)) = reconstruct {
        o.again = super::reconstruct_again::press(&ctx, cx, &dir, &world.name, None, origin);
    }
    if let Some((dir, public, origin)) = share_preview {
        state.share_public = public;
        state.share_origin = Some((origin, 10.0));
        let root = cx.snap.settings.sessions_dir.clone();
        let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
        state.share_prepare = Job::spawn(&ctx, move || {
            crate::controller::shares::local_entries(&root, server).into_iter().filter(|e| e.local.as_ref().is_some_and(|l| l.path() == dir)).collect()
        });
    }

    // The bar of the selection, over the page's bottom.
    let bar = action_bar(&ctx, o, &world, &sharing, page);
    if let Some(BarAction::Share) = bar {
        let paths: Vec<PathBuf> = o.selected.iter().cloned().collect();
        state.share_public = false;
        let root = cx.snap.settings.sessions_dir.clone();
        let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
        state.share_prepare = Job::spawn(&ctx, move || {
            crate::controller::shares::local_entries(&root, server)
                .into_iter()
                .filter(|e| e.local.as_ref().is_some_and(|l| paths.iter().any(|p| p == l.path())))
                .collect()
        });
    }
    if let Some(BarAction::SignIn) = bar {
        if cx.snap.account.state == crate::state::AccountState::SigningIn {
            cx.ctl.reopen_sign_in();
        } else {
            cx.ctl.sign_in();
        }
    }
    if let Some(BarAction::Clear) = bar {
        o.selected.clear();
    }

    // Reconstruct again's question, over the page.
    let asking = o.again.is_some() || share_open;
    if let Some(a) = o.again.as_mut() {
        if !a.show(&ctx, cx) {
            o.again = None;
        }
    }

    if o.viewer.is_some() {
        let fixed: HashSet<PathBuf> = world.shots.iter().filter(|s| sharing.shared(s) || sharing.sending(&s.path).is_some()).map(|s| s.path.clone()).collect();
        let pickable = sharing.blocked.is_none();
        viewer(&ctx, cx, state, &world, &fixed, pickable);
    } else if !asking && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if let Some(o) = state.open.as_mut() {
            if o.selected.is_empty() {
                close = true;
            } else {
                o.selected.clear();
            }
        }
    }
    if close {
        if let Some(o) = state.open.as_mut() {
            o.closing = true;
            o.viewer = None;
        }
    }
}

/// 1 234 → "1.2K", 668 000 000 → "668M".
fn compact(n: u64) -> String {
    let f = n as f64;
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => trim(format!("{:.1}", f / 1e3)) + "K",
        1_000_000..1_000_000_000 => trim(format!("{:.1}", f / 1e6)) + "M",
        _ => trim(format!("{:.1}", f / 1e9)) + "B",
    }
}

fn trim(s: String) -> String {
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// The world's picture with its name, author and links over it: the card's picture, and the large one fading in
/// over it once decoded.
#[allow(clippy::too_many_arguments)]
fn hero(
    ui: &mut Ui,
    cx: &mut Cx,
    world: &VisitedWorld,
    info: Option<&Info>,
    rect: Rect,
    small: Option<&TextureHandle>,
    large: Option<&TextureHandle>,
    t: f32,
    content: f32,
) {
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    let k = t.clamp(0.0, 1.0);
    let top = 20.0 + 4.0 * k;
    let bottom = 24.0 * k;
    let radii = [top, top, bottom, bottom];
    let r8 = |r: f32| r.round().clamp(0.0, 255.0) as u8;
    let fade = anim::toggle(&ctx, Id::unique(("hero-large", &world.id)), large.is_some(), Spring::SOFT);
    if fade < 0.99 || large.is_none() {
        thumb_shape(&p, rect, CornerRadius { nw: r8(top), ne: r8(top), sw: r8(bottom), se: r8(bottom) }, small.or(large), 1.0);
    }
    if let Some(l) = large.filter(|_| fade > 0.01) {
        let size = l.size_vec2();
        let uv = cover_uv(rect, size.x / size.y.max(1.0), 1.0);
        paint::squircle_texture(&p, rect, radii.map(theme::squircle), l.id(), uv, Color32::from_white_alpha((fade.clamp(0.0, 1.0) * 255.0) as u8));
    }
    let shade = paint::rounded_rect_points_corners(rect, radii);
    paint::fill_gradient(
        &p,
        &shade,
        &Gradient::vertical(rect, &[(0.0, theme::alpha(theme::ink_950(), 0.15)), (0.45, Color32::TRANSPARENT), (1.0, theme::alpha(theme::ink_950(), 0.85))]),
    );
    if content <= 0.0 {
        return;
    }
    let mut cp = p.clone();
    cp.multiply_opacity(content);
    let dy = 14.0 * (1.0 - content);
    let listed = match info {
        Some(Info::Listed(page)) => Some(page),
        _ => None,
    };
    let buttons_w = 2.0 * 168.0 + 8.0;
    // Copy name right after the name (REQUEST20 1): its room comes off the name's.
    let copy_room = 32.0 + 8.0 + 72.0;
    let name_at = widgets::text_truncated(
        &cp,
        pos2(rect.left() + 28.0, rect.bottom() - 58.0 + dy),
        Align2::LEFT_CENTER,
        &Txt::new(&world.name, 30.0).weight(weight::SEMIBOLD).tracking(-0.015),
        rect.width() - 56.0 - buttons_w - copy_room,
    );
    let mut sub = Vec::new();
    if let Some(a) = listed.and_then(|p| p.world.author_name.clone()) {
        sub.push(tr!("by {author}", author = a));
    }
    if let Some(page) = listed {
        if page.previews > 0 {
            sub.push(tr_n("{count} 3D preview on Orbiters", "{count} 3D previews on Orbiters", page.previews as u64));
        }
        if page.picture_count > 0 {
            sub.push(tr_n("{count} picture on Orbiters", "{count} pictures on Orbiters", page.picture_count as u64));
        }
    }
    if matches!(info, Some(Info::Unlisted)) {
        sub.push(tr("Private or removed world").into());
    }
    let sub = if sub.is_empty() { world.id.clone() } else { sub.join(" · ") };
    widgets::text_truncated(
        &cp,
        pos2(rect.left() + 28.0, rect.bottom() - 28.0 + dy),
        Align2::LEFT_CENTER,
        &Txt::new(sub, 13.0).color(theme::fg_2()),
        rect.width() - 56.0 - buttons_w,
    );

    let mut hu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("hero-actions"));
    hu.multiply_opacity(content);
    copy_name(&mut hu, &world.id, &world.name, pos2(name_at.right() + 8.0, name_at.center().y));
    let y = rect.bottom() - 24.0 - 40.0 + dy;
    let map_r = Rect::from_min_size(pos2(rect.right() - 24.0 - 168.0, y), vec2(168.0, 40.0));
    let vrc_r = Rect::from_min_size(pos2(map_r.left() - 8.0 - 168.0, y), vec2(168.0, 40.0));
    if Button::new(tr("View on VRChat")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Glass).width(168.0).show_at(&mut hu, vrc_r).clicked() {
        let r = controller::open_url(&vrchat_url(&world.id));
        cx.run(&ctx, r);
    }
    let mut map = Button::new(tr("Map explorer")).icon(icons::MAP_TRIFOLD).variant(Variant::Glass).width(168.0);
    if listed.is_none() {
        map = map.enabled(false).hint(tr("Orbiters lists public worlds only"));
    }
    if map.show_at(&mut hu, map_r).clicked() {
        let r = controller::open_url(&world_page_url(server(cx), &world.id));
        cx.run(&ctx, r);
    }
}

/// How long Copy name says Copied.
const COPIED_FOR: f64 = 1.6;

/// Copy name beside a world's name (REQUEST20 1), its left edge at `at`'s x, centred on its y: the name, exactly, to the
/// clipboard; Copied beside it for a moment.
pub(crate) fn copy_name(ui: &mut Ui, id: &str, name: &str, at: egui::Pos2) {
    if name.trim().is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let key = Id::unique(("world-name-copied", id));
    let now = ctx.input(|i| i.time);
    let copied = ctx.data(|d| d.get_temp::<f64>(key)).is_some_and(|at| now - at < COPIED_FOR);
    let rect = Rect::from_min_size(pos2(at.x, at.y - 16.0), vec2(32.0, 32.0));
    let (glyph, label) = if copied { (icons::CHECK, mark("Copied")) } else { (icons::COPY, mark("Copy the world's name")) };
    let b = IconButton::new(glyph, label).size(32.0, 15.0).backdrop(theme::alpha(theme::ink_950(), 0.35)).id_salt(("world-copy-name", id));
    if widgets::activated(&b.show_at(ui, rect)) {
        ctx.copy_text(name.to_string());
        ctx.data_mut(|d| d.insert_temp(key, now));
    }
    if copied {
        let t = Txt::new(tr("Copied"), 13.0).weight(weight::SEMIBOLD).color(theme::accent());
        widgets::text(ui.painter(), pos2(rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, &t);
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

fn section_title(ui: &Ui, rect: Rect, title: &str, sub: &str) {
    let p = ui.painter();
    widgets::text(p, pos2(rect.left(), rect.top() + 12.0), Align2::LEFT_CENTER, &Txt::new(title, 18.0).weight(weight::SEMIBOLD));
    widgets::text_truncated(p, pos2(rect.left(), rect.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::fg_3()), rect.width() - 180.0);
}

fn note(ui: &Ui, rect: Rect, glyph: &str, title: &str, text: &str) {
    let p = ui.painter();
    widgets::frame(p, rect, 16.0, theme::surface_soft());
    let tile = Rect::from_min_size(pos2(rect.left() + 16.0, rect.center().y - 18.0), vec2(36.0, 36.0));
    p.rect_filled(tile, theme::radius(10.0), theme::white(0.06));
    widgets::icon(p, tile.center(), glyph, 17.0, theme::fg_3());
    let x = tile.right() + 14.0;
    widgets::text(p, pos2(x, rect.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(title, 13.5).weight(weight::MEDIUM));
    widgets::text_truncated(p, pos2(x, rect.center().y + 10.0), Align2::LEFT_CENTER, &Txt::new(text, 12.0).color(theme::fg_3()), rect.right() - x - 16.0);
}

enum RowAction {
    Share(bool),
    /// Reconstruct or Reconstruct again, pressed in this rect.
    Reconstruct(Rect),
    Stop,
    Unqueue,
}

/// The floating back button's height.
const BACK_H: f32 = 32.0;

#[cfg(test)]
mod back_tests {
    use super::*;

    /// Wherever the window is, the button stays on the page's first line under the title bar, whose height the page
    /// leaves empty at rest; and its place is the page's, not the scrolled content's (nothing here reads a scroll).
    #[test]
    fn the_back_button_floats_over_the_pages_first_line() {
        for (w, h) in [(1040.0, 700.0), (1280.0, 820.0), (3840.0, 2160.0)] {
            let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
            let r = back_rect(page, 40.0, 150.0);
            assert_eq!((r.left(), r.top(), r.height()), (32.0, 48.0, BACK_H));
            assert!(page.contains_rect(r));
        }
    }
}

/// Where the world page's back button floats, `width` wide: the top left of the page under the title bar, where it would
/// rest as the page's first line. That line stays empty in the scrolling content, so that at rest the button covers none
/// of it, and everything scrolls on under it after that.
fn back_rect(page: Rect, top: f32, width: f32) -> Rect {
    Rect::from_min_size(pos2(page.left() + 32.0, page.top() + top + 8.0), vec2(width, BACK_H))
}

/// Where a session row's thumbnail sits.
fn row_thumb(rect: Rect) -> Rect {
    Rect::from_min_size(pos2(rect.left() + 10.0, rect.top() + 10.0), vec2((rect.height() - 20.0) * 16.0 / 9.0, rect.height() - 20.0))
}

/// A session recorded in the world: its thumbnail, when, and what can be done with it.
fn session_row(ui: &mut Ui, cx: &Cx, rect: Rect, s: &SessionSummary, tex: Option<&TextureHandle>, sharing: &Sharing) -> Option<RowAction> {
    let p = ui.painter().clone();
    widgets::frame(&p, rect, 18.0, theme::surface());
    let thumb = row_thumb(rect);
    thumb_shape(&p, thumb, theme::radius(10.0), tex, 1.0);
    let x = thumb.right() + 16.0;
    let title = super::local_time(&s.manifest.created_utc);
    widgets::text(&p, pos2(x, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(title, 14.0).weight(weight::SEMIBOLD));
    let st = &s.manifest.stats;
    let mut line = fill(
        &tr_n("{duration} · {total} keyframe", "{duration} · {total} keyframes", st.keyframes as u64),
        &[("duration", &format::duration(st.duration_s)), ("total", &format::integer(st.keyframes as f64))],
    );
    if s.trimmed && s.reconstruction.is_some() {
        line.push_str(&format!(" · {}", tr("3D view only")));
    }
    let job = cx.snap.reconstruction.as_ref().filter(|j| j.session_dir == s.dir);
    let upload = sharing.preview_upload(&s.dir);
    let waiting = cx.snap.reconstruct_queue.contains(&s.dir);
    let (state_text, state_color) = if let Some(j) = job {
        (format!("{} · {:.0}%", stage_label(j.stage), overall_progress(j) * 100.0), theme::accent())
    } else if waiting {
        (tr("Waiting to reconstruct").to_string(), theme::fg_2())
    } else if let Some(u) = upload.filter(|u| u.running()) {
        match u.state {
            UploadState::Preparing => (tr("Preparing the web preview…").to_string(), theme::accent()),
            _ => (tr!("Sharing · {percent}%", percent = format!("{:.0}", u.fraction() * 100.0)), theme::accent()),
        }
    } else if let Some(r) = &s.reconstruction {
        let splats = compact(r.gaussians as u64);
        match r.psnr_held_out {
            Some(q) => (tr!("Splats ready · {count} · {quality} dB", count = splats, quality = format!("{q:.1}")), theme::fg_2()),
            None => (tr!("Splats ready · {count}", count = splats), theme::fg_2()),
        }
    } else if st.depth_frames == 0 {
        (tr("No depth frames: cannot be reconstructed").to_string(), theme::fg_4())
    } else {
        (tr("Not reconstructed yet").to_string(), theme::fg_3())
    };
    if upload.is_some_and(|u| matches!(u.state, UploadState::Done { .. })) {
        line.push_str(&format!(" · {}", tr("shared")));
    }
    let offer = super::reconstruct_again::offer(&super::reconstruct_again::Facts::of_session(s), cx.snap);
    use super::reconstruct_again::Offer;
    let right_w = 330.0;
    widgets::text_truncated(&p, pos2(x, rect.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(line, 12.0).color(theme::fg_3()), rect.right() - x - right_w - 16.0);
    widgets::text_truncated(
        &p,
        pos2(x, rect.top() + 64.0),
        Align2::LEFT_CENTER,
        &Txt::new(state_text, 12.0).color(state_color),
        rect.right() - x - right_w - 16.0,
    );
    if let Some(u) = upload.filter(|u| u.running()) {
        let bar = Rect::from_min_size(pos2(x, rect.bottom() - 6.0), vec2(rect.right() - x - right_w - 16.0, 3.0));
        let f = anim::spring_from(ui.ctx(), Id::unique(("preview-upload", u.id)), [0.0], [u.fraction()], Spring::SOFT)[0].clamp(0.0, 1.0);
        p.rect_filled(bar, theme::radius(1.5), theme::white(0.08));
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, 3.0)), theme::radius(1.5), theme::accent());
    }

    if let (Offer::Running { fraction, .. } | Offer::Paused { fraction, .. }, Some(j)) = (offer, job) {
        let bar = Rect::from_min_size(pos2(x, rect.bottom() - 6.0), vec2(rect.right() - x - right_w - 16.0, 3.0));
        let f = anim::spring_from(ui.ctx(), Id::unique(("row-reconstruct", &j.session_dir)), [0.0], [fraction], Spring::SOFT)[0].clamp(0.0, 1.0);
        p.rect_filled(bar, theme::radius(1.5), theme::white(0.08));
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, 3.0)), theme::radius(1.5), super::sessions::job_bar(j));
    }
    let b = Rect::from_min_size(pos2(rect.right() - 16.0 - 314.0, rect.center().y - 20.0), vec2(314.0, 40.0));
    if upload.is_some_and(|u| u.running()) {
        return None;
    }
    // Every row has the same buttons in the same place: the session tells them apart.
    let salt = ("session-row", &s.dir);
    match offer {
        Offer::Running { .. } => {
            if Button::new(offer.label()).icon(icons::X).variant(Variant::Secondary).hint(tr("Stops it: its 3D view stays as it is")).width(b.width()).id_salt(salt).show_at(ui, b).clicked() {
                return Some(RowAction::Stop);
            }
        }
        Offer::Paused { why, .. } => {
            if Button::new(offer.label()).icon(icons::X).variant(Variant::Secondary).hint(why).width(b.width()).id_salt(salt).show_at(ui, b).clicked() {
                return Some(RowAction::Stop);
            }
        }
        Offer::Waiting { hint } => {
            if Button::new(offer.label()).icon(icons::X).variant(Variant::Secondary).hint(hint).width(b.width()).id_salt(salt).show_at(ui, b).clicked() {
                return Some(RowAction::Unqueue);
            }
        }
        Offer::Start { again: false } | Offer::Blocked { again: false, .. } => {
            let blocked = match offer {
                Offer::Blocked { reason, .. } => Some(reason),
                _ => None,
            };
            let mut button = Button::new(offer.label()).icon(icons::MAGIC_WAND).variant(Variant::Secondary).enabled(blocked.is_none()).width(b.width());
            if let Some(h) = blocked {
                button = button.hint(h);
            }
            if button.id_salt(salt).show_at(ui, b).clicked() && blocked.is_none() {
                return Some(RowAction::Reconstruct(b));
            }
        }
        Offer::Start { again: true } | Offer::Blocked { again: true, .. } => {
            let blocked = match offer {
                Offer::Blocked { reason, .. } => Some(reason),
                _ => None,
            };
            let hint = blocked.unwrap_or(tr("Trains a new 3D view from its frames: it replaces this one once it is done"));
            // Beside Share, in the room Share and Share publicly stretch into: it steps aside while they are out.
            let share_id = Id::unique(("world-session-share", &s.dir));
            let out = ui.ctx().data(|d| d.get_temp::<bool>(share_id)).unwrap_or(false);
            let shown = anim::toggle(ui.ctx(), share_id.with("again-shown"), !out, Spring::SNAPPY).clamp(0.0, 1.0);
            let again = Button::new(offer.label())
                .icon(icons::MAGIC_WAND)
                .variant(Variant::Secondary)
                .enabled(blocked.is_none())
                .hint(hint)
                .id_salt(("session-row-again", &s.dir));
            let share_w = Button::new(tr("Share")).icon(icons::CLOUD_ARROW_UP).desired_width(ui);
            let again_w = again.desired_width(ui);
            let at = Rect::from_min_size(pos2(b.right() - share_w - 8.0 - again_w, b.top()), vec2(again_w, 40.0));
            if shown > 0.01 {
                let mut au = ui.new_child(UiBuilder::new().max_rect(at).id_salt(("session-row-again-at", &s.dir)));
                au.multiply_opacity(shown);
                if out {
                    au.disable();
                }
                if again.show_at(&mut au, at).clicked() && blocked.is_none() && !out {
                    return Some(RowAction::Reconstruct(at));
                }
            }
            if let Some(public) = super::sharing::morph(ui, share_id, pos2(b.right(), b.top()), 304.0, sharing.account) {
                return Some(RowAction::Share(public));
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
struct TileLook {
    selected: bool,
    /// Some picture is selected: a click selects instead of opening.
    selecting: bool,
    shared: bool,
    sending: bool,
    selectable: bool,
}

enum TileAction {
    Toggle,
    View,
}

/// A picture: opens large, or (its check, or any click while picking) selects it. Shared ones carry a badge.
fn picture_tile(ui: &mut Ui, rect: Rect, shot: &Shot, tex: Option<&TextureHandle>, look: TileLook, i: usize) -> Option<TileAction> {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("picture-tile", &shot.path));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let check = Rect::from_center_size(pos2(rect.right() - 18.0, rect.top() + 18.0), vec2(24.0, 24.0));
    let can_pick = look.selectable && !look.shared && !look.sending;
    let over_check = resp.hover_pos().is_some_and(|p| check.expand(6.0).contains(p));
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let picked = anim::toggle(&ctx, id.with("picked"), look.selected, Spring::SNAPPY);
    // Pressing shows at once.
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let enter = anim::spring_from(&ctx, id.with("enter"), [0.0], [1.0], Spring::SOFT)[0];
    let inset = 10.0 * picked + 3.0 * press;
    let r = rect.shrink(inset).translate(vec2(0.0, 10.0 * (1.0 - enter)));
    let mut p = ui.painter().clone();
    p.multiply_opacity(enter.clamp(0.0, 1.0));
    if picked > 0.01 {
        p.rect_filled(rect, theme::radius(theme::TILE_RADIUS), theme::alpha(theme::accent(), 0.12 * picked));
    }
    match tex {
        Some(t) => {
            let size = t.size_vec2();
            let uv = cover_uv(r, size.x / size.y.max(1.0), 1.0 + 0.04 * hover);
            paint::squircle_texture(&p, r, [theme::squircle(theme::TILE_RADIUS - inset * 0.3); 4], t.id(), uv, Color32::WHITE);
        }
        None => {
            widgets::frame(&p, r, theme::TILE_RADIUS, theme::surface_soft());
            let k = ((anim::clock(&ctx, 30.0) + i as f64 * 0.12) % 1.6 / 1.6) as f32;
            p.rect_filled(r, theme::radius(theme::TILE_RADIUS), theme::white(0.03 + 0.03 * (k * std::f32::consts::TAU).sin().abs()));
        }
    }
    if look.shared || look.sending {
        let label = if look.sending { tr("Sharing…") } else { tr("Shared") };
        let fg = if look.sending { theme::fg_2() } else { theme::OK };
        badge(&p, ui, pos2(r.left() + 8.0, r.top() + 8.0), label, theme::black(0.6), fg, false);
    }
    if can_pick && (look.selecting || hover > 0.01 || picked > 0.01) {
        let a = if look.selecting || look.selected { 1.0 } else { hover };
        let mut cp = p.clone();
        cp.multiply_opacity(a);
        let c = check.center();
        cp.circle_filled(c, 11.0, theme::mix(theme::black(0.35), theme::accent(), picked));
        cp.circle_stroke(c, 11.0, egui::Stroke::new(1.5, theme::mix(theme::white(if over_check { 1.0 } else { 0.8 }), theme::accent(), picked)));
        if picked > 0.01 {
            let mut ip = cp.clone();
            ip.multiply_opacity(picked);
            widgets::icon(&ip, c, icons::CHECK, 13.0, theme::on_accent());
        }
    }
    if resp.clicked() {
        if can_pick && (look.selecting || over_check) {
            return Some(TileAction::Toggle);
        }
        return Some(TileAction::View);
    }
    None
}

enum BarAction {
    Share,
    Clear,
    SignIn,
}

/// The selection's bar at the bottom of the page: how many, and share them (or sign in first); a pictures upload of
/// this world shows its progress there.
fn action_bar(ctx: &egui::Context, o: &Open, world: &VisitedWorld, sharing: &Sharing, page: Rect) -> Option<BarAction> {
    let sending = sharing.uploads.iter().copied().filter(|u| u.kind == UploadKind::Pictures && u.running()).max_by_key(|u| u.id);
    let n = o.selected.len();
    let visible = n > 0 || sending.is_some();
    let id = Id::unique(("world-bar", o.serial));
    let k = anim::toggle(ctx, id, visible, Spring::BOUNCY);
    if k < 0.01 {
        return None;
    }
    let width = (page.width() - 64.0).min(640.0);
    let rect = Rect::from_min_size(pos2(page.center().x - width / 2.0, page.bottom() - 24.0 - 64.0 + 40.0 * (1.0 - k)), vec2(width, 64.0));
    let mut action = None;
    orbiters_ui::front::area(ctx, id.with("area"), Order::Middle, false).fixed_pos(rect.min).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        // The panel's shadow reaches past the bar.
        ui.set_clip_rect(ctx.content_rect());
        ui.multiply_opacity(k.clamp(0.0, 1.0));
        let p = ui.painter().clone();
        widgets::panel(&p, rect, 22.0);
        let x = rect.left() + 22.0;
        if n == 0 {
            if let Some(u) = sending {
                let count = u.paths.len();
                widgets::text(
                    &p,
                    pos2(x, rect.center().y - 9.0),
                    Align2::LEFT_CENTER,
                    &Txt::new(fill(&tr_n("Sharing {count} picture of {world}", "Sharing {count} pictures of {world}", count as u64), &[("world", &world.name)]), 13.5).weight(weight::SEMIBOLD),
                );
                let bar = Rect::from_min_size(pos2(x, rect.center().y + 9.0), vec2(rect.width() - 44.0 - 60.0, 4.0));
                let f = anim::spring_from(ctx, Id::unique(("pictures-upload", u.id)), [0.0], [u.fraction()], Spring::SOFT)[0].clamp(0.0, 1.0);
                p.rect_filled(bar, theme::radius(2.0), theme::white(0.08));
                p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(4.0), 4.0)), theme::radius(2.0), theme::accent());
                widgets::text(
                    &p,
                    pos2(rect.right() - 22.0, rect.center().y + 10.0),
                    Align2::RIGHT_CENTER,
                    &Txt::new(format!("{:.0}%", f * 100.0), 12.0).color(theme::fg_3()),
                );
            }
            return;
        }
        widgets::text(
            &p,
            pos2(x, rect.center().y - 9.0),
            Align2::LEFT_CENTER,
            &Txt::new(tr_n("{count} picture selected", "{count} pictures selected", n as u64), 13.5).weight(weight::SEMIBOLD),
        );
        widgets::text(
            &p,
            pos2(x, rect.center().y + 10.0),
            Align2::LEFT_CENTER,
            &Txt::new(tr("Public on the Map explorer, with everyone in them"), 11.5).color(theme::fg_3()),
        );
        let mut right = rect.right() - 12.0;
        if sharing.account {
            let share = Button::new(tr("Share on Orbiters")).icon(icons::CLOUD_ARROW_UP).variant(Variant::Primary);
            let sw = share.desired_width(ui);
            let sr = Rect::from_min_size(pos2(right - sw, rect.center().y - 20.0), vec2(sw, 40.0));
            let share = match sharing.blocked {
                Some(h) => share.enabled(false).hint(h),
                None => share,
            };
            if share.confirm_at(ui, sr) {
                action = Some(BarAction::Share);
            }
            right = sr.left() - 8.0;
        } else {
            // One sign-in, finished in the browser (Connect there); while it waits, the page opens again.
            let sign_in = if sharing.signing_in {
                Button::new(tr("Finish in your browser"))
                    .icon(icons::ARROW_SQUARE_OUT)
                    .variant(Variant::Secondary)
                    .hint(tr("Connect on the Orbiters page in your browser"))
            } else {
                Button::new(tr("Sign in to share")).icon(icons::SIGN_IN).variant(Variant::Primary)
            };
            let sw = sign_in.desired_width(ui);
            let sr = Rect::from_min_size(pos2(right - sw, rect.center().y - 20.0), vec2(sw, 40.0));
            if sign_in.id_salt("world-bar-sign-in").show_at(ui, sr).clicked() {
                action = Some(BarAction::SignIn);
            }
            right = sr.left() - 8.0;
        }
        let clear = Rect::from_min_size(pos2(right - 36.0, rect.center().y - 18.0), vec2(36.0, 36.0));
        if IconButton::new(icons::X, mark("Clear the selection")).show_at(ui, clear).clicked() {
            action = Some(BarAction::Clear);
        }
    });
    action
}

/// A picture large over the window: arrows (or ←/→) go through the world's pictures, Esc or a click outside closes.
fn viewer(ctx: &egui::Context, cx: &mut Cx, state: &mut State, world: &VisitedWorld, fixed: &HashSet<PathBuf>, pickable: bool) {
    let share_open = state.sharing.is_some();
    let Some(o) = state.open.as_mut() else { return };
    let Some(index) = o.viewer else { return };
    let count = world.shots.len();
    if count == 0 {
        o.viewer = None;
        return;
    }
    let index = index.min(count - 1);
    let shot = &world.shots[index];
    let id = Id::unique(("picture-viewer", o.serial, o.viewer_serial));
    // The picture flies out of its tile and back (exactly onto the tile's crop, which then bounces).
    let closing = o.viewer_closing;
    let travel = morph::travel(ctx, id, !closing, Spring::EXPAND);
    if travel.done {
        o.viewer = None;
        if let Some(p) = state.pictures.as_mut() {
            p.forget_large();
        }
        return;
    }
    let t = travel.t;
    let k = orbiters_ui::viewer::smoothstep(0.5, 0.95, t);
    let tile = o.tiles.get(&index).copied();
    let window = ctx.content_rect();
    let area = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
    let pictures = state.pictures.as_mut();
    let (large, small) = match pictures {
        Some(p) => (
            p.get(ctx, &Picture::Shot { path: shot.path.clone(), size: shot.size, width: LARGE_W }),
            p.get(ctx, &Picture::Shot { path: shot.path.clone(), size: shot.size, width: THUMB_WIDTH }),
        ),
        None => (None, None),
    };
    let mut step: i64 = 0;
    let mut close = false;
    let mut toggle = false;
    let selected = o.selected.contains(&shot.path);
    // While it closes, the page takes presses again.
    orbiters_ui::front::area(ctx, id, Order::Foreground, !closing).fixed_pos(area.min).interactable(!closing).show(ctx, |ui| {
        let (_, backdrop) = ui.allocate_exact_size(area.size(), Sense::click());
        let p = ui.painter().clone();
        p.rect_filled(area, 0.0, theme::alpha(theme::ink_950(), 0.92 * morph::smoothstep(0.05, 0.6, t)));
        let stage = area.shrink2(vec2(88.0, 72.0));
        let aspect = if shot.info.height > 0 { shot.info.width as f32 / shot.info.height as f32 } else { 16.0 / 9.0 };
        let (w, h) = if stage.width() / stage.height() > aspect { (stage.height() * aspect, stage.height()) } else { (stage.width(), stage.width() / aspect) };
        let target = Rect::from_center_size(stage.center(), vec2(w, h));
        // From its tile (or, scrolled away, a little smaller in the middle, fading), its crop opening to the whole picture.
        let from = tile.map(|r| morph::scaled(r, travel.landing)).unwrap_or(Rect::from_center_size(target.center(), target.size() * 0.9));
        let pic = orbiters_ui::viewer::lerp_rect(from, target, t);
        let mut pp = p.clone();
        pp.multiply_opacity(if tile.is_some() { 1.0 } else { morph::shape_alpha(t) });
        let radius = theme::TILE_RADIUS + (16.0 - theme::TILE_RADIUS) * t.clamp(0.0, 1.0);
        match large.as_ref().or(small.as_ref()) {
            Some(tex) => {
                let size = tex.size_vec2();
                let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                let uv = orbiters_ui::viewer::lerp_rect(cover_uv(from, size.x / size.y.max(1.0), 1.0), whole, t.clamp(0.0, 1.0));
                paint::squircle_texture(&pp, pic, [theme::squircle(radius); 4], tex.id(), uv, Color32::WHITE)
            }
            None => widgets::frame(&pp, pic, radius, theme::surface_soft()),
        }
        let mut pp = p.clone();
        pp.multiply_opacity(k);
        if backdrop.clicked() && !pic.contains(backdrop.interact_pointer_pos().unwrap_or_default()) {
            close = true;
        }
        let caption = tr!(
            "{date} · {width}×{height} · {number} of {count}",
            date = super::local_time_unix(shot.info.taken),
            width = shot.info.width,
            height = shot.info.height,
            number = index + 1,
            count = count
        );
        widgets::text(&pp, pos2(pic.left(), pic.bottom() + 24.0), Align2::LEFT_CENTER, &Txt::new(caption, 12.5).color(theme::fg_2()));
        let mut vu = ui.new_child(UiBuilder::new().max_rect(area).id_salt("viewer-actions"));
        vu.multiply_opacity(k);
        if closing {
            vu.disable();
        }
        if count > 1 {
            for (s, glyph, label, x) in
                [(-1, icons::CARET_LEFT, tr("Previous picture"), area.left() + 24.0 + 22.0), (1, icons::CARET_RIGHT, tr("Next picture"), area.right() - 24.0 - 22.0)]
            {
                let r = Rect::from_center_size(pos2(x, area.center().y), vec2(44.0, 44.0));
                if IconButton::new(glyph, label).size(44.0, 20.0).backdrop(theme::white(0.08)).show_at(&mut vu, r).clicked() {
                    step = s;
                }
            }
        }
        let close_r = Rect::from_min_size(pos2(area.right() - 24.0 - 40.0, area.top() + 16.0), vec2(40.0, 40.0));
        if IconButton::new(icons::X, mark("Close")).size(40.0, 18.0).backdrop(theme::white(0.08)).show_at(&mut vu, close_r).clicked() {
            close = true;
        }
        let mut left = pic.right();
        if pickable && !fixed.contains(&shot.path) {
            let sel = Button::new(if selected { tr("Selected") } else { tr("Select") })
                .icon(if selected { icons::CHECK_CIRCLE } else { icons::CIRCLE })
                .variant(if selected { Variant::Primary } else { Variant::Glass })
                .size(Size::Sm);
            let sw = sel.desired_width(&vu);
            let sel_r = Rect::from_min_size(pos2(pic.right() - sw, pic.bottom() + 8.0), vec2(sw, 32.0));
            if sel.show_at(&mut vu, sel_r).clicked() {
                toggle = true;
            }
            left = sel_r.left() - 8.0;
        } else if fixed.contains(&shot.path) {
            // The folder button sits beside the words, whatever length they have in the language.
            let said = widgets::text(&pp, pos2(pic.right(), pic.bottom() + 24.0), Align2::RIGHT_CENTER, &Txt::new(tr("Shared on the Map explorer"), 12.5).color(theme::OK));
            left = (pic.right() - 190.0).min(said.left() - 14.0);
        }
        let folder_r = Rect::from_min_size(pos2(left - 32.0, pic.bottom() + 8.0), vec2(32.0, 32.0));
        if IconButton::new(icons::FOLDER_OPEN, mark("Show in its folder")).size(32.0, 15.0).show_at(&mut vu, folder_r).clicked() {
            if let Some(dir) = shot.path.parent() {
                let r = controller::open_path(dir);
                cx.run(ctx, r);
            }
        }
    });
    if !closing {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowLeft) {
                step = -1;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                step = 1;
            }
            // (Not while the Share window is over it: that press is the window's.)
            if i.key_pressed(egui::Key::Escape) && !share_open {
                close = true;
            }
        });
    }
    let Some(o) = state.open.as_mut() else { return };
    if toggle && !o.selected.remove(&shot.path) {
        o.selected.insert(shot.path.clone());
    }
    if step != 0 {
        o.viewer = Some((index as i64 + step).rem_euclid(count as i64) as usize);
    }
    if close {
        o.viewer_closing = true;
    }
}
