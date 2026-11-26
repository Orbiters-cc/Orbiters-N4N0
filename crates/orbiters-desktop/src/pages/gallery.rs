//! Photos & captures: local pictures, captures and the authenticated shared feed in one date-ordered gallery, grouped
//! by the instance they were taken in. Cards look like Visited worlds' and Capture files' and grow into the picture
//! window ([`sharing::Dialog`]) that shares them; the instance a group was taken in opens in a popover. Cards are
//! selected (Ctrl/Shift-click, their check, a rubber band) and dragged out of the window into other apps
//! ([`super::gallery_select`]). Each group is laid out as the picker over the gallery says ([`super::gallery_layout`]:
//! rows by default, the website's masonry, or the grid of cards), pictures keeping their shape in the first two; a
//! VRChat multi-layer picture is one card ([`super::gallery_layers`]). The member's Orbiters assets' pictures join in
//! (My assets, [`assets`]); every card has a menu of what can be done with it (a right press, [`card_menu`]).
#[path = "gallery_folders.rs"]
mod folders;
#[path = "gallery_assets.rs"]
mod assets;

use super::reconstruct_again::{self as again, Offer};
use super::{
    gallery_layout as arrangement,
    sharing::{self, Dialog},
    Cx, Job,
};
use crate::controller::shares::{self as controller, Entry};
use egui::emath::GuiRounding as _;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Id, Order, Rect, Sense, TextureHandle, Ui, UiBuilder};
use orbiters_core::{
    account::Server,
    settings::GalleryLayout,
    shares::{Feed, Query},
    vrchat_log::WorldState,
};
use orbiters_i18n::{mark, tr, tr_ctx, tr_n};
use orbiters_ui::{
    anim::{self, ease, Spring},
    feedback, icons, logo,
    paint::{self, Gradient},
    theme::{self, weight, Txt},
    widgets::{self, Button, SegOption, Size, Tone, Variant},
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    Mine,
    #[default]
    All,
    Shared,
    /// The galleries of the member's communities ([`super::community`]).
    Communities,
    /// The pictures of the Orbiters assets the member made or holds a licence to ([`assets`]).
    Assets,
}
impl Filter {
    fn value(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::All | Self::Communities | Self::Assets => "all",
            Self::Shared => "shared",
        }
    }

    /// Whether the communities' pictures show.
    fn communities(self) -> bool {
        matches!(self, Self::All | Self::Communities)
    }

    /// Whether the assets' pictures show.
    fn assets(self) -> bool {
        matches!(self, Self::All | Self::Assets)
    }

    /// Whether this computer's items and the sharing feed show (neither Communities nor My assets alone).
    fn own(self) -> bool {
        !matches!(self, Self::Communities | Self::Assets)
    }
}

/// The instance popover of a group: what it was, who was there, and joining it again while it is open.
struct Instance {
    entry: Entry,
    /// The group's Instance button (the popover grows out of it).
    anchor: Rect,
    serial: u64,
    closing: bool,
}

#[derive(Default)]
pub struct State {
    /// The text searched for at the last frame ([`super::new_search`]).
    searched: Option<String>,
    folders: folders::State,
    requests: super::requests::State,
    hash_job: Job<std::collections::HashMap<std::path::PathBuf, String>>,
    hash_asked: std::collections::HashSet<std::path::PathBuf>,
    shown_key: Option<(Filter, String)>,
    shown: Vec<Entry>,
    key: Option<(u64, u64, std::path::PathBuf, bool, String, u64)>,
    /// Who the account is, for the key above (not how its check with Orbiters goes).
    account: crate::pages::AccountFollow,
    /// This computer's entries being made, with the revision of what they are made from beyond the pictures, taken
    /// (and when) before any of it was read.
    /// This computer's entries, with the remote side read for the owner asked for (`ForAccount`: dropped when another).
    local_job: Job<super::ForAccount<(std::time::Instant, u64, Vec<Entry>)>>,
    locals: Vec<Entry>,
    /// The revision `locals` was made from, and when it was taken ([`list_is_stale`]).
    listed: Option<(std::time::Instant, u64)>,
    /// The visit history and the upload records at these files, no picture read (tests).
    #[cfg(test)]
    sources: Option<(std::path::PathBuf, Option<std::path::PathBuf>)>,
    feed: Feed,
    feed_job: Job<super::ForAccount<Result<Feed, anyhow::Error>>>,
    asked: Option<(Filter, String)>,
    query: String,
    filter: Filter,
    query_changed: f64,
    error: Option<String>,
    pictures: Option<crate::worlds::Pictures>,
    pub dialog: Option<Dialog>,
    instance: Option<Instance>,
    fixture: bool,
    roster_job: Job<Vec<String>>,
    instance_job: Job<Result<Option<bool>, String>>,
    instance_scope: Option<crate::controller::vrchat_data::Scope>,
    instance_checked: bool,
    instance_alive: Option<bool>,
    instance_error: Option<String>,
    /// Each card's picture as last drawn (by entry key): windows open out of it and close back into it.
    cards: HashMap<String, Rect>,
    /// The open window came out of a card of the grid (it rides on the window's way and lands back there; a window
    /// opened elsewhere, from a request, keeps to its own origin).
    from_card: bool,
    /// Each group's Instance button as last drawn (by its first entry's key).
    instance_buttons: HashMap<String, Rect>,
    serial: u64,
    /// The first group's instance popover opens once drawn (screenshots).
    stage_instance: bool,
    /// Screenshots of the gallery itself: this computer's items cut to the fixture's two.
    fixture_locals: bool,
    /// Screenshots (`gallery-server-only`): captures on Orbiters only, none on this computer, their pictures read as the
    /// app reads them (from the server `ORBITERS_PUBLIC_API` names).
    fixture_server_only: bool,
    /// The cards chosen, a rubber band and a drag out of the window.
    select: super::gallery_select::Selection,
    /// The galleries of the member's communities and their windows (an asset's picture opens in their window too).
    community: super::community::State,
    /// The member's Orbiters assets and their pictures.
    assets: assets::State,
    /// A picture from Orbiters saved from a card's menu (Download).
    download_job: Job<anyhow::Result<std::path::PathBuf>>,
    /// How the groups are laid out: chosen here at once, saved in the settings (`photos_layout`), followed when the
    /// settings change elsewhere.
    layout: GalleryLayout,
    layout_saved: Option<GalleryLayout>,
    /// The shape of each picture the listing gave no size for, once its thumbnail is decoded (by entry key).
    aspects: Shapes,
    /// The revision of `shown`: another one each time the list is made.
    entries_rev: u64,
    /// Where the list's groups and pictures are, made again when what they come from changed.
    places: Option<arrangement::Places>,
    /// How many times they were.
    places_made: u64,
    /// The topmost picture in view, its place in the list and the [`arrangement::Places`] it has it in.
    scroll_anchor: Option<(String, f32, u64)>,
    scroll_offset: f32,
    layout_width: f32,
    local_dirty: bool,
    /// How many of the feed's are someone else's.
    shared_count: usize,
    /// Where the open window's item was last found in the list.
    dialog_at: usize,
    /// Screenshots: made-up local pictures added to this computer's (several shapes, a multi-layer picture).
    fixture_extra: Vec<Entry>,
    /// Screenshots: the made-up pictures alone, none of this computer's.
    fixture_only_extra: bool,
    /// The photo editor on a picture from Orbiters (Edit in a card's menu), over the page while it is open.
    editor: Option<super::editor::Editor>,
    /// Export… of a 3D capture of this computer (its card's menu), over the page ([`super::splat_export`]).
    export: Option<super::splat_export::Export>,
    /// Reconstruct again's question from a card's menu, over the page ([`super::reconstruct_again`]).
    again: Option<again::Ask>,
    /// The capture element's height at the last frame (REQUEST21 5).
    capture_h: f32,
    /// A capture the capture element asked to open: its card's window at the next frame, or, not among the cards, its
    /// detail in Capture files (for the page around: [`State::take_review`]).
    review: Option<String>,
    review_elsewhere: Option<std::path::PathBuf>,
}
/// Screenshots: the world tour's captures of 2026-11-15 as Orbiters lists them once their folders here were deleted
/// (L-no-thumbnails): two worlds, private uploads and one public per world, each with its picture on Orbiters.
fn server_only_feed(template: &orbiters_core::shares::Item) -> Vec<orbiters_core::shares::Item> {
    use orbiters_core::shares;
    let worlds = [("wrld_6f3c1a8e-2b1d-4c55-9f00-3a5e8d7c1b21", "Audience Anarchy"), ("wrld_0a9d7e2c-5f4b-4e1a-8c3d-7b6a5f4e3d21", "Freddy's Custom Pizzeria 2.0")]; // i18n: not shown (VRChat world names: data, never translated)
    let made = [(66, 0, true, "2026-11-15T12:27:33.000Z"), (65, 0, false, "2026-11-15T12:26:56.000Z"), (61, 1, true, "2026-11-15T12:24:21.000Z"), (62, 1, false, "2026-11-15T12:24:40.000Z"), (64, 1, false, "2026-11-15T12:23:08.000Z"), (63, 1, false, "2026-11-15T12:22:47.000Z")];
    made.into_iter()
        .map(|(id, world, public, taken)| {
            let mut item = template.clone();
            item.kind = shares::Kind::Capture;
            item.id = id;
            item.mine = true;
            item.public = public;
            item.grants.clear();
            item.picture = None;
            item.world = shares::World { id: worlds[world].0.into(), name: worlds[world].1.into(), thumbnail_url: None };
            item.taken_at = Some(taken.into());
            item.created_at = taken.into();
            item.instance_id = Some(format!("{}~hidden(usr_3f0d9d62-6a55-4f07-9a0e-5b1d2b7f6c11)~region(eu)", 30000 + world * 7));
            item.source_key = None;
            let mut capture = item.capture.clone().unwrap_or_default();
            capture.picture_url = Some(format!("/captures/capture/{id}/file?part=picture"));
            item.capture = Some(capture);
            item
        })
        .collect()
}

impl State {
    pub fn stage_folders(&mut self) { self.folders.stage(); }
    /// The folders' fixture with the popover closed, for a scenario that opens it with a click.
    pub fn stage_folders_closed(&mut self) { self.folders.stage_closed(); }
    /// Where Manage folders was drawn, and whether its popover is open.
    pub fn folders_button(&self) -> Option<Rect> { self.folders.button() }
    pub fn folders_open(&self) -> bool { self.folders.is_open() }

    pub fn busy(&self) -> bool {
        self.busy_now()
    }

    /// A window, dialog or editor of the page is open (it would be lost if the window closed: `crate::window_rest`).
    pub fn holds(&self) -> bool {
        self.dialog.is_some()
            || self.editor.is_some()
            || self.export.is_some()
            || self.again.is_some()
            || self.instance.is_some()
            || self.community.is_open()
            || self.requests.open
            || self.requests.is_out()
    }

    fn busy_now(&self) -> bool {
        self.requests.busy()
            || self.hash_job.running()
            || self.local_job.running()
            || self.feed_job.running()
            || self.pictures.as_ref().is_some_and(|p| p.loading())
            || self.dialog.as_ref().is_some_and(Dialog::loading)
            || self.community.busy()
            || self.assets.busy()
            || self.download_job.running()
    }

    /// Screenshots: the member's assets from the fixture (`gallery-assets…`), My assets chosen unless the name ends in
    /// `-all`.
    pub fn stage_assets(&mut self, name: &str, sessions_dir: &std::path::Path) {
        self.assets.stage(sessions_dir);
        if !name.ends_with("-all") {
            self.filter = Filter::Assets;
        }
        self.shown_key = None;
    }

    /// Screenshots: the open asset picture window (the community picture window), the `n`th asset picture's.
    pub fn open_asset(&mut self, ctx: &egui::Context, n: usize) {
        let keys: Vec<String> = self.shown.iter().filter(|e| assets::owns(&e.key)).map(|e| e.key.clone()).collect();
        let Some(key) = keys.get(n).cloned() else { return };
        let origin = self.cards.get(&key).copied().unwrap_or(Rect::from_min_size(pos2(300.0, 300.0), vec2(320.0, 180.0)));
        if let Some((list, about, index, stand_ins)) = self.assets.window(&keys, &key, false) {
            self.community.open_pictures(ctx, list, about, index, origin, stand_ins);
        }
    }

    /// Screenshots: the middle of the `n`th card, where a pointer would press it.
    pub fn stage_card_centre(&self, n: usize) -> Option<egui::Pos2> {
        self.shown.get(n).and_then(|e| self.cards.get(&e.key)).map(|r| r.center())
    }

    /// Screenshots: a card's menu opened as a right press on the `n`th card would (at its middle).
    pub fn stage_menu(&mut self, ctx: &egui::Context, n: usize) {
        if let Some(e) = self.shown.get(n) {
            let id = Id::unique(("gallery-card", &e.key));
            let at = self.cards.get(&e.key).map_or(pos2(400.0, 400.0), |r| r.center());
            // As `popup::context_menu` keeps an open menu: its point and its open flag.
            let menu = id.with(("context-menu", "card-menu"));
            ctx.data_mut(|d| {
                d.insert_temp(menu.with("at"), at);
                d.insert_temp(menu.with("popup-open"), true);
            });
        }
    }

    /// Screenshots: the first `n` cards lifted, the drag waiting for `waiting` of them from Orbiters.
    pub fn stage_wait(&mut self, n: usize, waiting: usize) {
        let keys: Vec<String> = self.shown.iter().take(n).map(|e| e.key.clone()).collect();
        self.select.stage_wait(keys, waiting);
    }

    /// Screenshots: the communities' galleries from the fixture (`gallery-community…`), Communities chosen unless the
    /// name ends in `-all`.
    pub fn stage_community(&mut self, name: &str, sessions_dir: &std::path::Path) {
        self.community.stage(name, sessions_dir);
        if !name.ends_with("-all") {
            self.filter = Filter::Communities;
        }
        self.shown_key = None;
    }

    pub fn community_mut(&mut self) -> &mut super::community::State {
        &mut self.community
    }

    /// The entries the page shows (screenshots open their windows).
    pub fn shown(&self) -> &[Entry] {
        &self.shown
    }
    /// The fixture's local items: two of this computer's captures, as if taken in the feed's first instance.
    fn stage_locals(&mut self) {
        if self.feed.items.is_empty() {
            return;
        }
        self.locals.truncate(if self.fixture_only_extra { 0 } else { 2 });
        for (n, e) in self.locals.iter_mut().enumerate() {
            e.world_id = self.feed.items[0].world.id.clone();
            e.world_name = self.feed.items[0].world.name.clone();
            e.instance = self.feed.items[0].instance_id.clone();
            e.taken = controller::unix("2026-10-28T21:30:00Z") - n as u64 * 300;
        }
        self.locals.extend(self.fixture_extra.iter().cloned());
        self.shown_key = None;
    }

    /// Screenshots: made-up local pictures beside this computer's (written by the screenshot code), as if taken in the
    /// feed's first instance.
    pub fn stage_extra(&mut self, mut entries: Vec<Entry>) {
        if let Some(first) = self.feed.items.first() {
            for e in &mut entries {
                e.world_id = first.world.id.clone();
                e.world_name = first.world.name.clone();
                e.instance = first.instance_id.clone();
            }
        }
        self.fixture_extra = entries;
        self.fixture_locals = true;
        self.stage_locals();
    }

    /// Screenshots: the made-up pictures alone (none of this computer's beside them).
    pub fn stage_made_up_only(&mut self) {
        self.fixture_only_extra = true;
        self.stage_locals();
    }

    /// Screenshots: as if this was typed in the search field.
    pub fn stage_query(&mut self, query: &str) {
        self.query = query.into();
        self.shown_key = None;
    }

    /// Screenshots: the groups laid out so.
    pub fn stage_layout(&mut self, layout: GalleryLayout) {
        self.layout = layout;
    }

    /// Screenshots: in the open window, a multi-layer picture's layer chosen (None: the picture), then these layers'
    /// eyes pressed.
    pub fn stage_layer(&mut self, choice: Option<usize>, eyes: &[usize]) {
        if let Some(d) = self.dialog.as_mut() {
            d.choose_layer(choice);
            for i in eyes {
                d.layers.toggle(*i);
            }
        }
    }

    pub fn stage(&mut self, ctx: &egui::Context, name: &str) {
        self.requests.stage(name);
        self.shown_key = None;
        self.fixture = true;
        self.feed_job = Job::default();
        self.feed = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/captures.json")).unwrap();
        self.fixture_locals = name.starts_with("gallery-");
        if self.fixture_locals {
            self.stage_locals();
        }
        self.fixture_server_only = name == "gallery-server-only";
        if self.fixture_server_only {
            self.feed.items = server_only_feed(&self.feed.items[0]);
        }
        if name == "gallery-public" {
            self.feed.items[0].public = true;
        }
        if name.starts_with("gallery-instance") {
            // Opened on the first group's button once it is drawn.
            self.stage_instance = true;
            self.instance_alive = match name {
                "gallery-instance-live" => Some(true),
                "gallery-instance-closed" => Some(false),
                _ => None,
            };
            self.instance_error = (name == "gallery-instance-error").then(|| "VRChat is busy. Please try again shortly.".into());
        }
        if name == "gallery-empty" {
            self.query = "No matching world".into();
        }
        if name == "gallery-mine" {
            self.filter = Filter::Mine;
        }
        if name == "gallery-shared" {
            self.filter = Filter::Shared;
        }
        if name == "gallery-search" {
            self.query = "Midnight".into();
        }
        if [
            "gallery-owner",
            "gallery-public",
            "gallery-grantee",
            "gallery-friends",
            "gallery-friends-error",
            "gallery-friends-search",
            "gallery-explicit",
            "gallery-error",
            "gallery-picture",
            "gallery-progress",
            "gallery-request",
            "gallery-done",
            "gallery-copy-link",
            "gallery-copy-link-offer",
            "gallery-edited",
            "gallery-updating",
            "gallery-update-replacing",
            "gallery-updated",
            "gallery-update-failed",
            "gallery-reconstructed",
            "gallery-again-shared",
        ]
        .contains(&name)
        {
            let item = self.feed.items[usize::from(matches!(name, "gallery-grantee" | "gallery-picture" | "gallery-request"))].clone();
            let mut entry = controller::remote_entry(item);
            if let Some(p) = self.locals.iter().find(|e| e.picture.is_some()).and_then(|e| e.picture.clone()) {
                entry.picture = Some(p);
            }
            let mut d = Dialog::new(ctx, entry, false);
            d.fixture(match name {
                "gallery-request" => "friends",
                "gallery-copy-link-offer" => "owner",
                _ => name.trim_start_matches("gallery-"),
            });
            self.dialog = Some(d);
        }
    }

    /// Opens the window of the `n`th card as if it was clicked (screenshots).
    pub fn open_card(&mut self, ctx: &egui::Context, n: usize) {
        let Some(e) = self.shown.get(n).cloned() else { return };
        let origin = self.cards.get(&e.key).copied().unwrap_or(Rect::from_min_size(pos2(264.0, 260.0), vec2(317.0, 178.0)));
        self.dialog = Some(Dialog::new(ctx, e, false).from(origin, picture_radius(self.layout)));
        self.from_card = true;
    }

    /// Opens the window of the card with this key, as if it was clicked (screenshots).
    pub fn open_key(&mut self, ctx: &egui::Context, key: &str) {
        if let Some(n) = self.shown.iter().position(|e| e.key == key) {
            self.open_card(ctx, n);
        }
    }

    /// A capture the capture element asked to open that is not among the cards: the page around opens its detail.
    pub fn take_review(&mut self) -> Option<std::path::PathBuf> {
        self.review_elsewhere.take()
    }

    /// Opens `entry`'s window on its friends to choose ("Share to …" beside VRChat's camera, REQUEST13).
    pub fn open_share_to(&mut self, ctx: &egui::Context, entry: Entry) {
        self.dialog = Some(Dialog::new(ctx, entry, true));
        self.from_card = false;
    }

    /// Starts closing the open window (screenshots).
    pub fn close_window(&mut self) {
        if let Some(d) = self.dialog.as_mut() {
            d.close();
        }
    }

    /// Goes to the next (1) or previous (-1) item in the open window (screenshots).
    pub fn step_window(&mut self, step: i64) {
        if let Some(d) = self.dialog.as_mut() {
            d.step = Some(step);
        }
    }

    /// The first `n` cards selected (screenshots).
    pub fn stage_select(&mut self, n: usize) {
        let keys: Vec<String> = self.shown.iter().take(n).map(|e| e.key.clone()).collect();
        self.select.stage(keys);
    }

    /// The selection lifted as a drag out of the window starts, and its drag image written to `image` (screenshots).
    pub fn stage_lift(&mut self, image: Option<&std::path::Path>) {
        let carried: Vec<Entry> = self.shown.iter().filter(|e| self.select.has(&e.key)).cloned().collect();
        self.select.stage_carry(&carried, image);
    }

    /// The open window's picture lifted by a drag out of it, as if Windows had it (or still `waiting` for it from
    /// Orbiters, the bar saying so over the window) (screenshots).
    pub fn stage_window_lift(&mut self, waiting: bool) {
        let shown = match self.dialog.as_ref() {
            Some(d) => Some(d.entry.clone()),
            None => self.community.window_key().and_then(|k| self.shown.iter().find(|e| e.key == k).cloned()),
        };
        if let Some(e) = shown {
            self.select.stage_window_carry(&e, waiting);
        }
    }

    /// What the last drag copied, said by the bar (screenshots).
    pub fn stage_copied(&mut self, ctx: &egui::Context, text: &str) {
        self.select.stage_said(ctx, text);
    }

    /// Copy public link was just pressed in the open window (screenshots).
    pub fn stage_copied_link(&mut self, ctx: &egui::Context) {
        if let Some(d) = self.dialog.as_mut() {
            d.stage_copied(ctx);
        }
    }

    /// Opens the requests window out of its button (screenshots).
    pub fn open_requests(&mut self) {
        self.requests.open = true;
    }

    /// Closes the instance popover back into its button (screenshots).
    pub fn close_instance(&mut self) {
        self.stage_instance = false;
        if let Some(i) = self.instance.as_mut() {
            i.closing = true;
        }
    }

    /// Closes the requests window back into its button (screenshots).
    pub fn close_requests(&mut self) {
        self.requests.open = false;
    }

    /// The account changed (another name, another environment, a sign-in refused): nothing of the old one stays on
    /// screen or arrives later. Its feed and what is on its way, the remote side of this computer's entries, the open
    /// picture and its editor, the decoded pictures, the communities' and the assets' lists and their windows, the
    /// selection. An editor of a picture of this computer stays (it holds the person's own work, nothing of the account).
    fn forget_account(&mut self) {
        self.feed_job = Job::default();
        self.hash_job = Job::default();
        self.feed = Feed::default();
        self.asked = None;
        self.shown_key = None;
        for entry in &mut self.locals {
            entry.remote = None;
        }
        self.dialog = None;
        if self.editor.as_ref().is_some_and(|e| e.is_remote()) {
            self.editor = None;
        }
        self.pictures = None;
        self.hash_asked.clear();
        self.community.forget_account();
        self.assets.forget_account();
        self.select.forget_account();
        self.instance = None;
    }

    fn observe_context(&mut self, key: (u64, u64, std::path::PathBuf, bool, String, u64)) {
        if self.key.as_ref().is_none_or(|k| k.1 != key.1 || k.3 != key.3 || k.4 != key.4) {
            self.asked = None;
        }
        self.local_dirty = false;
        self.key = Some(key);
    }

    fn fetch(&mut self, ctx: &egui::Context, server: Server, more: bool) {
        let q = Query {
            filter: self.filter.value().into(),
            q: self.query.trim().into(),
            cursor: if more { self.feed.next.clone() } else { None },
            limit: 24,
            ..Default::default()
        };
        self.error = None;
        let owner = self.account.owner().cloned().unwrap_or_default();
        let member = crate::member::reading(server);
        self.feed_job = super::job_for(ctx, &owner, move || {
            let (api, token) = member.ok_or_else(|| anyhow::anyhow!("{}", tr("Sign in to Orbiters in Settings first.")))?.api();
            api.captures(&token, &q)
        });
    }
}

/// Keeps this computer's pictures and captures as the list shows them: made again, on a worker, when what they are made
/// from changed (the folders' watch says so: pictures found, a capture's folder, the visits, the upload records), when
/// the app says its library did, on a visit, another account or environment, or a copy the editor saved; and taken in
/// when they land. What the person does on the page (a selection, a search, where the list is scrolled) is not
/// touched.
fn follow_locals(state: &mut State, ui: &Ui, cx: &Cx, server: Server) {
    let ctx = ui.ctx();
    if !state.fixture {
        let (listing_changed, _images_changed) = state.folders.tick(ui, cx);
        if listing_changed {
            state.local_dirty = true;
        }
        // Progressive discoveries keep existing decoded pictures visible.
    }
    let development = cx.snap.settings.development;
    let seen = state.account.see(&cx.snap.account, development);
    // Another account (or environment, or a sign-in refused): everything of the old one goes NOW, whatever the scan of
    // this computer's folders is doing (it went only once that scan ended, and an open remote picture, its editor or a
    // community's window stayed meanwhile).
    if seen.changed {
        state.forget_account();
    }
    // A list made for another owner (asked before the change, landing after it) is dropped and made again: its remote
    // side, its sharing and associations are the old account's (night4, verify2).
    let landed = state.local_job.take().map(|r| r.take_for(state.account.owner()));
    if let Some(None) = landed {
        state.local_dirty = true;
    }
    if let Some(Some((made, revision, entries))) = landed {
        state.locals = entries;
        state.listed = Some((made, revision));
        state.shown_key = None;
        // Screenshots: this computer's items, as the fixture has them, whenever they land.
        if state.fixture_locals {
            state.stage_locals();
        }
    }
    // What the list is made from changed outside the app since the list was made (a capture's folder, the visits, an
    // upload record): made again. Not while one is being made: what it was made from is known when it lands.
    if !state.local_job.running() && list_is_stale(state.folders.local_seen(), state.listed) {
        state.local_dirty = true;
    }
    let (account, answered) = (seen.owner.account.clone(), seen.answered);
    // The editor's saves too: a copy saved beside its original shows at once.
    let key = (
        cx.ctl.library_version(),
        cx.visit,
        cx.snap.settings.sessions_dir.clone(),
        development,
        // Who the account is, not how its check goes: offline, the check comes and goes every minute.
        account,
        crate::pages::editor::saved(),
    );
    if answered {
        // Orbiters answers again (it could not be reached, a sign-in was renewed): the feed is asked again; nothing
        // that shows is put away.
        state.asked = None;
    }
    if (state.local_dirty || state.key.as_ref() != Some(&key)) && !state.local_job.running() {
        let changed_account = state.key.as_ref().is_some_and(|k| k.3 != key.3 || k.4 != key.4);
        state.observe_context(key);
        let dir = cx.snap.settings.sessions_dir.clone();
        let folders = cx.snap.settings.picture_folders.clone();
        #[cfg(not(test))]
        let sources: Option<(std::path::PathBuf, Option<std::path::PathBuf>)> = None;
        #[cfg(test)]
        let sources = state.sources.clone();
        let owner = seen.owner.clone();
        state.local_job = super::job_for(ctx, &owner, move || {
            // The revision first, then the list: a change in between is in the list or not, and a later look, which
            // sees it, differs from this revision either way.
            let made = std::time::Instant::now();
            let mut revision = controller::LocalRevision::default();
            match sources {
                Some((history, records)) => {
                    let revision = revision.look_at(&dir, &history, records.as_deref());
                    (made, revision, controller::local_entries_at(&dir, Default::default(), &history, records.as_deref()))
                }
                None => (made, revision.look(&dir, server), controller::local_entries_with_folders(&dir, server, &folders)),
            }
        });
        if changed_account {
            // (Everything of the old account went when it changed: `forget_account`. The list is made again here.)
            state.shown_key = None;
        }
    }
}

/// What the page shows on its first frame, read before it (`App::prepare`).
pub fn prepare(ui: &Ui, cx: &Cx, state: &mut State) {
    let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
    follow_locals(state, ui, cx, server);
    follow_lists(state, ui.ctx(), cx, server);
}

/// Keeps what Orbiters lists as the page shows it: the communities' galleries and the member's assets, and the feed,
/// asked again when what is asked for changed.
fn follow_lists(state: &mut State, ctx: &egui::Context, cx: &Cx, server: Server) {
    // Screenshots read Orbiters' galleries and assets only from their fixtures (staged on their own).
    if state.community.poll(ctx, cx, state.filter.communities() && !state.fixture) {
        state.shown_key = None;
    }
    if state.assets.poll(ctx, cx, state.filter.assets() && !state.fixture) {
        state.shown_key = None;
    }
    if state.fixture || !state.filter.own() || anim::time(ctx) - state.query_changed <= 0.3 {
        return;
    }
    let query = state.query.trim();
    if state.asked.as_ref().is_none_or(|(filter, asked)| *filter != state.filter || asked != query) {
        state.asked = Some((state.filter, query.to_owned()));
        state.feed = Feed::default();
        state.shown_key = None;
        state.fetch(ctx, server, false);
    }
}

/// Whether the list is out of date: the watch saw (`looked`: when its look began, and the revision it saw) another
/// revision than the one the list was made from (`listed`: when that was taken, and what it was), in a look begun after
/// that one was taken. A look begun before says nothing: the list is the newer of the two, and the next look tells.
/// With no look yet, or no list, there is nothing to compare.
fn list_is_stale(looked: Option<(std::time::Instant, u64)>, listed: Option<(std::time::Instant, u64)>) -> bool {
    matches!((looked, listed), (Some((looked, seen)), Some((made, from))) if looked > made && seen != from)
}

/// Whether the row under the heading goes on two rows (the filters above; the layout picker, Requests and the search
/// under them): under 920 points of head, or when the last filter's words would reach the picker. Only something
/// really overlapping stacks it: the filters' track's own end (4 points of its edge and 14 of the last segment's
/// padding) may touch the picker, as English did at ~1225 wide for months (night3, verifier point 7: that size had
/// moved to two rows and its first picture 48 points down).
fn row_stacks(head_w: f32, filters_w: f32, requests_w: f32) -> bool {
    let last_words_end = filters_w - (4.0 + 14.0);
    head_w < 920.0 || head_w < last_words_end + arrangement::PICKER_W + 12.0 + requests_w + 12.0 + 260.0
}

/// The filters over the gallery.
fn filter_options() -> [SegOption<'static, Filter>; 5] {
    [
        SegOption::new(Filter::All, tr("All")),
        SegOption::new(Filter::Mine, tr("Mine")),
        SegOption::new(Filter::Shared, tr("Shared with me")),
        SegOption::new(Filter::Communities, tr("Communities")).hint(tr("Pictures of your communities' galleries and their events")),
        SegOption::new(Filter::Assets, tr("My assets")).hint(tr("Pictures of the avatars and other assets you made or own on Orbiters")),
    ]
}

/// The Requests button beside the search.
fn requests_button() -> Button<'static> {
    Button::new(tr("Requests")).icon(icons::TRAY).hint(tr("People asking to share what you shared with them, and what you asked"))
}

/// `true` opens the existing capture-management surface (reconstruction, storage, import).
/// The gallery, the capture element at the top of its list (`capture`, `projects`: REQUEST21 5). Whether Manage captures
/// was pressed.
pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State, capture: &mut super::capture::State, projects: &mut super::projects::State) -> bool {
    let ctx = ui.ctx().clone();
    state.requests.poll(&ctx, cx);
    let page = ui.max_rect();
    let now = anim::time(&ctx);
    let development = cx.snap.settings.development;
    let server = if development { Server::Development } else { Server::Production };
    // The layout the settings keep, unless it was just chosen here (the tray saves it a moment later).
    let saved = cx.snap.settings.photos_layout;
    if !state.fixture && state.layout_saved != Some(saved) {
        state.layout_saved = Some(saved);
        state.layout = saved;
    }
    let layout = state.layout;
    let tiles = arrangement::tiles(layout);
    follow_locals(state, ui, cx, server);
    if state.pictures.is_none() {
        state.pictures = Some(crate::worlds::Pictures::new(&ctx));
    }
    follow_lists(state, &ctx, cx, server);
    if let Some(r) = state.feed_job.take().and_then(|r| r.take_for(state.account.owner())) {
        match r {
            Ok(page) => {
                state.feed.append(page);
                state.shown_key = None;
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }

    if let Some(hashes) = state.hash_job.take() {
        for entry in &mut state.locals {
            if let Some(controller::Local::Picture(path)) = &entry.local {
                if let Some(hash) = hashes.get(path) {
                    entry.source_key = Some(hash.clone());
                }
            }
        }
        state.shown_key = None;
    }
    if let Some(r) = state.download_job.take() {
        match r {
            Ok(path) => {
                state.select.say(&ctx, tr("Saved in Documents › Orbiters › Downloaded pictures"));
                let result = super::editor::files::reveal(&path);
                cx.run(&ctx, result);
            }
            Err(e) => state.select.say(&ctx, &tr!("The picture could not be saved: {e}", e = e)),
        }
    }
    if state.filter.own() && state.asked.as_ref().is_some_and(|(_, q)| q != state.query.trim()) {
        super::frame_in(&ctx, 0.3 - (now - state.query_changed));
    }
    let head = Rect::from_min_size(pos2(page.left() + super::PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - super::PAD_X * 2.0, 72.0));
    let narrow = head.width() < 690.0;
    // The filters on one line, the layout, Requests and the search on the right; narrower, the layout and the search
    // go under them.
    // (Also when the row does not fit in this language: the filters ran under the layout picker in Portuguese.)
    let stacked = row_stacks(head.width(), widgets::segmented_width(ui, &filter_options(), false), requests_button().desired_width(ui));
    // Communities: the galleries as chips under the filters.
    let chips_h = if state.filter == Filter::Communities { 46.0 } else { 0.0 };
    let controls = Rect::from_min_size(pos2(head.left(), head.bottom() + 14.0), vec2(head.width(), if stacked { 88.0 } else { 40.0 } + chips_h));
    let top = controls.bottom() + 20.0;
    let q = state.query.trim().to_lowercase();
    let shown_key = (state.filter, q.clone());
    let entries = if state.shown_key.as_ref() == Some(&shown_key) {
        std::mem::take(&mut state.shown)
    } else {
        state.shown_key = Some(shown_key);
        state.select.list_changed();
        state.entries_rev += 1;
        state.shared_count = state.feed.items.iter().filter(|i| !i.mine).count();
        hash_what_may_match(&ctx, state);
        let mut entries: Vec<Entry> = state
            .locals
            .iter()
            .filter(|_| state.filter.own() && state.filter != Filter::Shared && !state.fixture_server_only)
            // Screenshots: one of this computer's, and the made-up pictures.
            .enumerate()
            .filter(|(n, e)| !state.fixture || *n == 0 || state.fixture_extra.iter().any(|x| x.key == e.key))
            .map(|(_, e)| e)
            .cloned()
            .collect();
        for item in &state.feed.items {
            if state.filter == Filter::Mine && !item.mine || state.filter == Filter::Shared && item.mine || !state.filter.own() {
                continue;
            }
            if let Some(e) = entries.iter_mut().find(|e| controller::matches_item(e, item)) {
                e.remote = Some(item.clone());
                e.world_name = item.world.name.clone();
                if e.instance.is_none() {
                    e.instance = item.instance_id.clone();
                }
            } else {
                let mut e = controller::remote_entry(item.clone());
                if !state.fixture {
                    // On Orbiters only (its folder here deleted, or never here): the picture Orbiters keeps of it.
                    e.picture = controller::remote_picture(item, development, false).or(e.picture);
                }
                if state.fixture_server_only {
                    e.picture = controller::remote_picture(item, development, false);
                } else if state.fixture {
                    e.picture = state.locals.iter().filter_map(|e| e.picture.clone()).nth(if item.mine { 0 } else { 1 });
                }
                entries.push(e);
            }
        }
        let find = orbiters_core::search::Query::new(&q);
        entries.retain(|e| find.matches(&e.world_name));
        if state.filter.communities() {
            entries.extend(state.community.entries(&find));
        }
        if state.filter.assets() {
            entries.extend(state.assets.entries(&find, development));
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.taken));
        state.community.gather(&mut entries);
        entries
    };

    // The card the window came out of is left out while the window draws it (riding under it as it leaves and lands)
    // and covers it, until the window landed back on it.
    // (A multi-layer picture's window showing one of its layers still belongs to the picture's card.)
    let open_key = state.dialog.as_ref().filter(|d| state.from_card && d.hides_source()).map(|d| d.layers.card_key(&d.entry).to_owned());
    let window_key = state.dialog.as_ref().map(|d| d.layers.card_key(&d.entry).to_owned());
    let mut window_card: Option<sharing::Source> = None;
    let loading = if state.filter == Filter::Communities {
        state.community.loading() && state.community.count() == 0
    } else if state.filter == Filter::Assets {
        state.assets.loading() && state.assets.count() == 0
    } else {
        state.local_job.running() || (state.feed_job.running() && state.feed.items.is_empty())
    };
    let from = orbiters_ui::blur::next_shape(ui);
    let mut gu = ui.new_child(UiBuilder::new().max_rect(page).id_salt("photos-captures-grid"));
    let mut clicked: Option<(Entry, Rect)> = None;
    let mut instance_clicked: Option<(Entry, Rect)> = None;
    let mut community_clicked: Option<(String, Rect)> = None;
    let mut asset_clicked: Option<(String, Rect)> = None;
    // (By key: an entry held in the grid's frame costs the main thread's stack.)
    let mut menu_picked: Option<(String, Rect, Menu)> = None;
    let modal = state.dialog.is_some() || state.instance.is_some() || state.community.is_open();
    state.select.begin(development);
    let mut background = None;
    let selecting = state.select.any();
    // Preserve the visible picture when progressive batches insert or reshape content above it.
    // Use the same layout calculation as rendering; compensate only for layout changes, leaving wheel input intact.
    // The capture element at the list's top (REQUEST21 5): as it grows or shrinks out of view (scrolled past), what is
    // in view stays where it is; in view, it pushes the gallery down as it opens. Its height is known before it is drawn.
    let capture_h = capture.height(now);
    let capture_moved = if state.scroll_offset > 1.0 && state.scroll_offset >= state.capture_h { capture_h - state.capture_h } else { 0.0 };
    state.capture_h = capture_h;
    let spacing = ui.spacing().item_spacing.y;
    if state.layout_width > 0.0 {
        let key = state.places_key(layout, state.layout_width, spacing);
        if state.places.as_ref().is_none_or(|p| p.key != key) {
            state.places = Some(state.make_places(key, &entries));
        }
    }
    // Where the picture held in view went, in the places made since it was last seen.
    let anchor_moved = match (&state.scroll_anchor, &state.places) {
        (Some((key, was, made)), Some(p)) if p.made != *made => entries.iter().position(|e| e.key == *key).map(|i| p.cells[i].top() - was),
        _ => None,
    };
    let mut scroll = egui::ScrollArea::vertical().id_salt("photos-captures");
    // A new search starts at the top of its results (not held at the picture that was in view).
    if super::new_search(&mut state.searched, &q) {
        scroll = scroll.vertical_scroll_offset(0.0);
    } else if let Some(offset) = anchored_scroll_offset(state.scroll_offset, anchor_moved) {
        scroll = scroll.vertical_scroll_offset(offset + capture_moved);
    } else if capture_moved.abs() > 0.1 {
        scroll = scroll.vertical_scroll_offset(state.scroll_offset + capture_moved);
    }
    let mut capture_out = None;
    // (Out of the state while the cards change it; made for the width drawn where it is another than the last frame's.)
    let places = state.places.take();
    let mut drawn = None;
    let output = scroll
        .auto_shrink([false, false])
        .scroll_bar_rect(Rect::from_min_max(pos2(page.left(), top), page.max))
        .show(&mut gu, |ui| {
            // The grid's empty space starts a rubber band (cards and buttons drawn later are on top of it).
            background = Some(ui.interact(ui.clip_rect(), Id::unique("photos-captures-band"), Sense::drag()));
            egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 40 }).show(ui, |ui| {
                ui.add_space(top - page.top());
                // The capture element: the Capture tab's content, a bar until it is needed (REQUEST21 5).
                {
                    let name_of = |dir: &std::path::Path| {
                        let key = dir.to_string_lossy();
                        entries.iter().find(|e| e.key == key).map(|e| e.world_name.clone()).filter(|n| !n.is_empty())
                    };
                    capture_out = super::capture::element(ui, cx, capture, projects, &name_of, top - page.top());
                }
                let width = ui.available_width();
                state.layout_width = width;
                if entries.is_empty() {
                    if loading {
                        let (cells, h) = shimmer_cells(layout, width, 2);
                        let (area, _) = ui.allocate_exact_size(vec2(width, 56.0 + h), Sense::hover());
                        feedback::shimmer(ui.painter(), Rect::from_min_size(area.min + vec2(0.0, 6.0), vec2(220.0, 18.0)), 9.0);
                        feedback::shimmer(ui.painter(), Rect::from_min_size(area.min + vec2(0.0, 32.0), vec2(160.0, 12.0)), 6.0);
                        for c in cells {
                            super::sessions::shimmer_card(ui, c.translate(area.min.to_vec2() + vec2(0.0, 56.0)));
                        }
                    } else {
                        empty_state(ui, cx, state.filter, !q.is_empty(), (page.bottom() - top) * 0.6);
                    }
                }
                let key = state.places_key(layout, width, spacing);
                if places.as_ref().is_none_or(|p| p.key.width != width) {
                    drawn = Some(state.make_places(key, &entries));
                }
                let at = drawn.as_ref().or(places.as_ref()).expect("just made");
                let base = ui.cursor().min.round_ui();
                // What is in view, and a little around it (a card's shadow, a heading on its way in).
                let (from, to) = (ui.clip_rect().top() - base.y - 24.0, ui.clip_rect().bottom() - base.y + 24.0);
                // The first headings come in one after the other, whether they are in view or not; those after together.
                for g in &at.groups {
                    let _ = super::entering(ui, cx, 1 + g.entries.start.min(8), g.heading(width));
                    if g.entries.start >= 8 {
                        break;
                    }
                }
                for group in at.groups_within(from, to) {
                    let (start, count) = (group.entries.start, group.entries.len());
                    let first = &entries[start];
                    let last = &entries[group.entries.end - 1];
                    let r = group.heading(width).translate(base.to_vec2());
                    if ui.is_rect_visible(r.expand(24.0)) {
                        let (r, a) = super::entering(ui, cx, 1 + start.min(8), r);
                        let mut hu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("group-head", &first.key)));
                        hu.multiply_opacity(a);
                        if super::community::owns(&first.key) {
                            let pictures = state.pictures.as_mut().expect("pictures");
                            state.community.group_header(&mut hu, cx, pictures, first, last, count, !modal);
                        } else if assets::owns(&first.key) {
                            state.assets.group_header(&mut hu, cx, first, count);
                        } else {
                            let (button, pressed) = group_header(&mut hu, first, last, count);
                            match state.instance_buttons.get_mut(&first.key) {
                                Some(b) => *b = button,
                                None => drop(state.instance_buttons.insert(first.key.clone(), button)),
                            }
                            if pressed && !modal {
                                instance_clicked = Some((first.clone(), button));
                            }
                        }
                    }
                    for i in at.cells_within(group, from, to) {
                        let e = &entries[i];
                        let r = at.cells[i].translate(base.to_vec2());
                        let index = i + 1;
                        if !ui.is_rect_visible(r) {
                            continue;
                        }
                        if super::community::owns(&e.key) {
                            let pictures = state.pictures.as_mut().expect("pictures");
                            if e.size.is_none() {
                                if let Some(t) = e.picture.as_ref().and_then(|p| pictures.get(&ctx, p)) {
                                    state.aspects.learn(&ctx, e, &t);
                                }
                            }
                            let marks = state.select.marks(&e.key);
                            if let Some((thumb, resp)) = state.community.card(ui, cx, pictures, index, e, r, !modal, tiles, marks) {
                                if !modal {
                                    if let super::gallery_select::CardAct::Open = state.select.card(&resp, e, super::gallery_select::check_rect(thumb), &entries) {
                                        community_clicked = Some((e.key.clone(), thumb));
                                    }
                                    if let Some(m) = card_menu(ui, &resp, e, &state.select, cx) {
                                        menu_picked = Some((e.key.clone(), thumb, m));
                                    }
                                }
                                state.cards.insert(e.key.clone(), thumb);
                            }
                            state.select.place(&e.key, r);
                            continue;
                        }
                        let tex = e.picture.as_ref().and_then(|p| state.pictures.as_mut()?.get(&ctx, p));
                        if let (None, Some(t)) = (e.size, tex.as_ref()) {
                            state.aspects.learn(&ctx, e, t);
                        }
                        let from_tex = e.remote.as_ref().and_then(|i| i.from.as_ref()).and_then(|from| {
                            if state.fixture {
                                return None;
                            }
                            let url = from.avatar_url.clone()?;
                            state.pictures.as_mut()?.get(&ctx, &crate::worlds::Picture::Remote { world_id: format!("person-{}", from.id), url, large: false })
                        });
                        let shown = if open_key.as_ref() == Some(&e.key) { 0.0 } else { state.community.card_shown(&e.key) };
                        let about = if assets::owns(&e.key) { state.assets.labels(e) } else { None };
                        let running = cx.snap.capture_shares.iter().filter(|j| j.key == e.key && j.outcome.is_none());
                        let (sharing, updating) = running.fold((false, false), |(s, u), j| (s || !j.update, u || j.update));
                        let look = CardLook {
                            tex: tex.as_ref(),
                            from: from_tex.as_ref(),
                            shown,
                            interactive: !modal,
                            sharing,
                            updating,
                            riding: false,
                            selected: state.select.has(&e.key),
                            selecting,
                            carried: state.select.carried(&e.key),
                            tile: tiles,
                            about: about.clone(),
                        };
                        let (thumb, resp) = card(ui, cx, index, e, r, &look);
                        if let Some(resp) = resp.filter(|_| !modal) {
                            if let super::gallery_select::CardAct::Open = state.select.card(&resp, e, super::gallery_select::check_rect(thumb), &entries) {
                                if assets::owns(&e.key) {
                                    asset_clicked = Some((e.key.clone(), thumb));
                                } else {
                                    clicked = Some((e.clone(), thumb));
                                }
                            }
                            if let Some(m) = card_menu(ui, &resp, e, &state.select, cx) {
                                menu_picked = Some((e.key.clone(), thumb, m));
                            }
                        }
                        if assets::owns(&e.key) {
                            state.community.note_card(&e.key, if tiles { r } else { Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0)) });
                        }
                        state.select.place(&e.key, r);
                        let picture = if tiles { r } else { Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0)) };
                        state.cards.insert(e.key.clone(), picture);
                        if window_key.as_ref() == Some(&e.key) {
                            let (e, tex, from_tex, about) = (e.clone(), tex.clone(), from_tex.clone(), about.clone());
                            window_card = Some(sharing::Source {
                                home: r,
                                picture,
                                paint: Box::new(move |ui, cx| {
                                    let look = CardLook {
                                        tex: tex.as_ref(),
                                        from: from_tex.as_ref(),
                                        shown: 1.0,
                                        interactive: false,
                                        sharing,
                                        updating,
                                        riding: true,
                                        selected: false,
                                        selecting: false,
                                        carried: false,
                                        tile: tiles,
                                        about: about.clone(),
                                    };
                                    let _ = card(ui, cx, 0, &e, r, &look);
                                }),
                            });
                        }
                    }
                }
                if !at.groups.is_empty() {
                    ui.add_space(at.height);
                }
                let more_feed = state.filter.own() && state.feed.next.is_some() && state.error.is_none();
                let more_community = state.filter.communities() && state.community.has_more() && state.community.error().is_none();
                if (more_feed || more_community) && !entries.is_empty() {
                    // Endless: the next page is asked for while the end is still a screen away, and a row of
                    // shimmering cards holds the place its pictures arrive in (a failed page says so below, with
                    // Try again, instead of asking again and again).
                    let (cells, h) = shimmer_cells(layout, width, 1);
                    let (r, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    if r.top() < ui.clip_rect().bottom() + ui.clip_rect().height() {
                        if more_feed && !state.feed_job.running() {
                            state.fetch(&ctx, server, true);
                        }
                        if more_community && !state.community.loading() {
                            state.community.more(&ctx, cx);
                        }
                    }
                    if ui.is_rect_visible(r) {
                        for c in cells {
                            super::sessions::shimmer_card(ui, c.translate(r.min.to_vec2()));
                        }
                    }
                }
                if let Some(error) = state.community.error().filter(|_| state.filter == Filter::Communities).map(str::to_owned) {
                    ui.add_space(12.0);
                    feedback::banner(
                        ui,
                        Id::unique("gallery-community-error"),
                        Tone::Danger,
                        &tr!("Your communities' galleries could not be loaded: {error}", error = error),
                        false,
                    );
                    ui.add_space(8.0);
                    let b = Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!state.community.loading());
                    let bw = b.desired_width(ui);
                    let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                    if sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                        state.community.retry();
                    }
                }
                if let Some(error) = state.assets.error().filter(|_| state.filter == Filter::Assets).map(str::to_owned) {
                    ui.add_space(12.0);
                    feedback::banner(ui, Id::unique("gallery-assets-error"), Tone::Danger, &tr!("Your assets could not be loaded: {error}", error = error), false);
                    ui.add_space(8.0);
                    let b = Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!state.assets.loading()).id_salt("gallery-assets-retry");
                    let bw = b.desired_width(ui);
                    let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                    if sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                        state.assets.retry();
                    }
                }
                if let Some(error) = state.error.clone() {
                    ui.add_space(12.0);
                    feedback::banner(ui, Id::unique("gallery-feed-error"), Tone::Danger, &tr!("Shared moments could not be loaded: {error}", error = error), false);
                    ui.add_space(8.0);
                    let b = Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!state.feed_job.running());
                    let bw = b.desired_width(ui);
                    let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                    if sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                        state.fetch(&ctx, server, false);
                    }
                }
                // The selection's bar floats over the bottom: the last row can scroll above it.
                if selecting {
                    ui.add_space(64.0);
                }
            });
        });
    state.scroll_offset = output.state.clamped_offset().y;
    // (The groups' places are counted from under the capture element.)
    let visible_top = state.scroll_offset - capture_h;
    if let Some(super::capture::Out::Review(dir)) = capture_out {
        let key = dir.to_string_lossy().to_string();
        match entries.iter().position(|e| e.key == key) {
            Some(_) => state.review = Some(key),
            None => state.review_elsewhere = Some(dir),
        }
    }
    // The topmost item in view; of a row's items, all at the same height, always the same one (by its key): the map
    // gives them in an order of its own at each frame, and the anchor went from one to another without a scroll.
    // (In the places this frame's offset was for: drawn at another width, the next frame's offset keeps it in view there.)
    let anchor = places.as_ref().and_then(|p| p.topmost(visible_top, |i| entries[i].key.as_str()).map(|(i, y)| (i, y, p.made)));
    match (&mut state.scroll_anchor, anchor) {
        (Some((key, y, made)), Some((i, top, of))) if *key == entries[i].key => (*y, *made) = (top, of),
        (slot, anchor) => *slot = anchor.map(|(i, y, made)| (entries[i].key.clone(), y, made)),
    }
    state.places = drawn.or(places);
    super::under_top(&gu, cx, "photos-captures", from, top);
    if let Some(background) = &background {
        // The keys (Escape, Ctrl+A, Ctrl+C) are not the grid's under a window opened from a card's menu (Export,
        // Reconstruct again) or the requests' window: they belong to what is in front.
        let covered = modal || state.export.is_some() || state.again.is_some() || state.requests.is_out();
        state.select.update(ui, &entries, background, covered);
    }

    // Heading and the bar over the grid (drawn after it: what scrolls up goes under them).
    let (head_r, head_a) = super::entering(ui, cx, 0, head);
    let mb = Button::new(tr("Manage captures")).icon(icons::FOLDER_OPEN).hint(tr("Reconstruct, free up space and import raw captures"));
    let mw = mb.desired_width(ui);
    let fw = Button::new(folders::LABEL).icon(icons::FOLDER_OPEN).desired_width(ui);
    // The line under the title stops before the two buttons on the head's right (Night PG2-12: cut at 60 % of the
    // head whatever stood there, it ran under "Manage folders" in French and Portuguese at the minimum width).
    let sub_room = (head.width() - mw - 10.0 - fw - 12.0).min(head.width() * 0.6).max(0.0);
    {
        let mut hu = ui.new_child(UiBuilder::new().max_rect(head_r).id_salt("gallery-head"));
        hu.multiply_opacity(head_a);
        let mine = state.locals.len();
        let shared = state.shared_count;
        let communities = state.community.count();
        let assets = state.assets.count();
        let sub = if loading {
            tr("Gathering your pictures and captures…").to_string()
        } else {
            // Each part says all it says, so each language puts its own words around its number.
            let mut parts = vec![tr!("{count} on this computer", count = mine), tr!("{count} shared with you", count = shared)];
            if communities > 0 {
                parts.push(tr!("{count} from your communities", count = communities));
            }
            if assets > 0 {
                parts.push(tr!("{count} of your assets", count = assets));
            }
            if !narrow {
                parts.push(tr("grouped by where they were taken").to_string());
            }
            parts.join(" · ")
        };
        super::heading_in(&hu, head_r, tr("Library"), tr("Photos & captures"), Some(&sub), sub_room);
    }
    let mut manage = false;
    let b = Rect::from_min_size(pos2(head.right() - mw, head.bottom() - 40.0), vec2(mw, 40.0));
    if sharing::pressed(mb.show_at(ui, b)) {
        manage = true;
    }
    state.folders.show(ui, cx, Rect::from_min_size(pos2(b.left() - 10.0 - fw, b.top()), vec2(fw, 40.0)));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(controls).id_salt("gallery-controls"));
    let before_filter = state.filter;
    widgets::segmented(&mut cu, "gallery-filter", &mut state.filter, &filter_options(), false);
    if state.filter == Filter::Communities {
        let chips = Rect::from_min_size(pos2(controls.left(), controls.bottom() - widgets::CHIP_H), vec2(controls.width(), widgets::CHIP_H));
        let before = state.community.chosen;
        state.community.chips(ui, chips);
        if state.community.chosen != before {
            state.shown_key = None;
        }
    }
    if state.filter != before_filter {
        state.query_changed = now - 1.0;
    }
    let picker_w = arrangement::PICKER_W;
    let search = if stacked {
        Rect::from_min_size(pos2(controls.left() + picker_w + 12.0, controls.top() + 48.0), vec2(controls.width() - picker_w - 12.0, 40.0))
    } else {
        Rect::from_min_size(pos2(controls.right() - 260.0, controls.top()), vec2(260.0, 40.0))
    };
    let pending = state.requests.pending();
    let rb = requests_button();
    let rw = rb.desired_width(ui);
    let request_button = Rect::from_min_size(pos2(if stacked { controls.right() - rw } else { search.left() - 12.0 - rw }, controls.top()), vec2(rw, 40.0));
    // The layout picker: before Requests, or before the search under the filters.
    let picker = if stacked {
        Rect::from_min_size(pos2(controls.left(), controls.top() + 48.0), vec2(picker_w, 40.0))
    } else {
        Rect::from_min_size(pos2(request_button.left() - 12.0 - picker_w, controls.top()), vec2(picker_w, 40.0))
    };
    {
        let mut pu = ui.new_child(UiBuilder::new().max_rect(picker).id_salt("gallery-layout"));
        if arrangement::picker(&mut pu, picker, &mut state.layout) && !state.fixture {
            cx.ctl.patch_settings(serde_json::json!({ "photos_layout": state.layout }));
        }
    }
    let requests_open = state.requests.open || state.requests.is_out();
    {
        let mut ru = ui.new_child(UiBuilder::new().max_rect(request_button.expand(10.0)).id_salt("requests-button"));
        // The window grows out of the button: the button is away while it is, and back as the window lands on it.
        ru.multiply_opacity(if state.requests.open && !state.requests.is_out() { 0.0 } else { state.requests.button_alpha(&ctx) });
        let resp = rb.width(rw).id_salt("requests-button").show_at(&mut ru, request_button);
        if pending > 0 {
            let k = anim::spring_from(&ctx, Id::unique(("requests-badge", pending)), [0.6], [1.0], Spring::BOUNCY)[0];
            let mut bp = ru.painter().clone();
            bp.multiply_opacity(k.clamp(0.0, 1.0));
            widgets::count_badge(&bp, pos2(request_button.right() - 6.0, request_button.top() + 4.0), pending);
        }
        if sharing::pressed(resp) && !requests_open {
            state.requests.open = true;
        }
    }
    let mut su = ui.new_child(UiBuilder::new().max_rect(search).id_salt("gallery-search"));
    let before = state.query.clone();
    let hint = match state.filter {
        Filter::Communities => tr("Search galleries and people"),
        Filter::Assets => tr("Search your assets"),
        _ => tr("Search worlds"),
    };
    widgets::search_field(&mut su, "gallery-search", &mut state.query, hint, search.width());
    if state.query != before {
        state.query_changed = now;
    }
    if let Some((item, origin)) = state.requests.show(ui, cx, request_button) {
        let entry = controller::remote_entry(item);
        let mut d = Dialog::new(&ctx, entry, false);
        if let Some(o) = origin {
            d = d.from(o, 14.0);
        }
        state.dialog = Some(d);
        state.from_card = false;
    }
    if let Some((e, thumb, m)) = menu_picked.and_then(|(k, t, m)| Some((entries.iter().find(|e| e.key == k)?.clone(), t, m))) {
        let mut opened = Opened { clicked: &mut clicked, community: &mut community_clicked, asset: &mut asset_clicked, instance: &mut instance_clicked };
        act_menu(&ctx, cx, state, &entries, e, thumb, m, &mut opened);
    }
    if let Some((e, thumb)) = clicked {
        let mut e = e;
        if let Some(item) = e.remote.as_ref() {
            if !state.fixture && e.picture.is_none() {
                e.picture = controller::remote_picture(item, development, false);
            }
        }
        state.dialog = Some(Dialog::new(&ctx, e, false).from(thumb, picture_radius(layout)));
        state.from_card = true;
    }
    if let Some(d) = state.dialog.as_mut() {
        // The items before and after it in the gallery: arrows and ←/→ go through them (not through the communities' and
        // the assets' pictures, which have a window of their own).
        let own = |e: &&Entry| !super::community::owns(&e.key) && !assets::owns(&e.key);
        let card = d.layers.card_key(&d.entry);
        if entries.get(state.dialog_at).is_none_or(|e| e.key != card) {
            state.dialog_at = entries.iter().position(|e| e.key == card && own(&e)).unwrap_or(usize::MAX);
        }
        let at = (state.dialog_at < entries.len()).then_some(state.dialog_at);
        let before = at.and_then(|i| entries[..i].iter().rev().find(own));
        let after = at.and_then(|i| entries[i + 1..].iter().find(own));
        d.neighbours = (before.is_some(), after.is_some());
        let typing = ctx.memory(|m| m.focused().is_some());
        let key = ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowLeft) {
                -1
            } else if i.key_pressed(egui::Key::ArrowRight) {
                1
            } else {
                0
            }
        });
        let step = d.step.take().or((key != 0 && !typing && d.can_navigate()).then_some(key));
        if let Some(step) = step {
            if let Some(next) = if step < 0 { before } else { after } {
                d.navigate(next.clone(), step as f32);
            }
        }
        // Its card where the grid drew it this frame (it follows a scroll, another item's card); off screen, it closes
        // into the middle of the page.
        if state.from_card {
            match window_card.take().filter(|c| c.home.intersects(page)) {
                Some(c) => {
                    d.set_origin(c.picture);
                    d.source = Some(c);
                }
                None => d.lose_origin(),
            }
        }
        // Its picture drags out of the window as its card does ([`super::gallery_select::Selection::carry`]).
        d.drag_out = true;
        d.lifted = state.select.window_lifted(&d.entry.key);
        let close = d.show(ui, cx);
        if std::mem::take(&mut d.dragged) {
            let payload = super::gallery_select::window_payload(&entries, &d.entry, d.layers.card_key(&d.entry));
            state.select.carry(&ctx, &payload);
        }
        if d.changed {
            state.shown_key = None;
            let entry = d.entry.clone();
            for e in &mut state.locals {
                if e.key == entry.key {
                    e.remote = entry.remote.clone();
                }
            }
            if let Some(item) = entry.remote {
                if let Some(i) = state.feed.items.iter_mut().find(|i| i.key() == item.key()) {
                    *i = item;
                }
            }
            d.changed = false;
        }
        if close {
            state.dialog = None;
        }
    }
    // The photo editor on a picture from Orbiters, over the whole window under its title bar.
    if let Some(e) = state.editor.as_mut() {
        let window = ctx.content_rect();
        let rect = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
        let mut open = true;
        orbiters_ui::front::area(&ctx, egui::Id::unique("gallery-remote-editor"), egui::Order::Foreground, true).fixed_pos(rect.min).show(&ctx, |ui| {
            ui.set_min_size(rect.size());
            open = e.show(ui, rect);
        });
        if !open {
            state.editor = None;
        }
    }
    if let Some(e) = state.export.as_mut() {
        if !e.show(&ctx) {
            state.export = None;
        }
    }
    // Reconstruct again's question, from a card's menu.
    if let Some(a) = state.again.as_mut() {
        if !a.show(&ctx, cx) {
            state.again = None;
        }
    }
    if let Some((key, thumb)) = asset_clicked {
        let keys: Vec<String> = entries.iter().filter(|e| assets::owns(&e.key)).map(|e| e.key.clone()).collect();
        if let Some((list, about, index, stand_ins)) = state.assets.window(&keys, &key, development) {
            state.community.open_pictures(&ctx, list, about, index, thumb, stand_ins);
        }
    }
    if let Some((key, thumb)) = community_clicked {
        let keys: Vec<String> = entries.iter().filter(|e| super::community::owns(&e.key)).map(|e| e.key.clone()).collect();
        state.community.open(&ctx, &key, thumb, &keys);
    }
    {
        let lifted = state.community.window_key().is_some_and(|k| state.select.window_lifted(&k));
        state.community.set_lifted(lifted);
        let pictures = state.pictures.as_mut().expect("pictures");
        state.community.show(ui, cx, pictures);
        // A community's or an asset's picture dragged out of its window: its card's file, as from the grid.
        if let Some(shown) = state.community.take_drag() {
            let payload = super::gallery_select::window_payload(&entries, &shown, &shown.key);
            state.select.carry(&ctx, &payload);
        }
    }
    if let Some((e, anchor)) = instance_clicked {
        state.serial += 1;
        state.instance = Some(Instance { entry: e.clone(), anchor, serial: state.serial, closing: false });
        state.instance_checked = false;
        state.instance_alive = None;
        state.instance_error = None;
        state.instance_job = Job::default();
        let location = e.instance.as_ref().map(|i| format!("{}:{i}", e.world_id));
        let taken = e.taken;
        state.roster_job = Job::spawn(&ctx, move || location.map(|l| orbiters_core::vrchat_accounts::instance_people(&l, taken)).unwrap_or_default());
    }
    if state.stage_instance && state.instance.is_none() {
        if let Some(first) = entries.first().cloned() {
            if let Some(anchor) = state.instance_buttons.get(&first.key).copied() {
                let mut entry = first;
                entry.instance = Some("12345~friends(usr_11111111-1111-1111-1111-111111111111)~region(eu)".into());
                entry.people = vec!["Alex".into(), "Mika".into(), "Robin".into()];
                state.serial += 1;
                state.instance = Some(Instance { entry, anchor, serial: state.serial, closing: false });
            }
        }
    }
    instance_popover(&ctx, cx, state);
    let modal = state.dialog.is_some() || state.instance.is_some() || state.community.is_open() || state.export.is_some() || state.again.is_some();
    state.select.bar(ui, page, &entries, &mut state.pictures, modal);
    state.shown = entries;
    if let Some(key) = state.review.take() {
        state.open_key(&ctx, &key);
    }
    manage
}

/// What a card's menu does: the actions of the item's window, the selection and the group, at hand on its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    /// Reconstruct again, with or without the wearer (orbiters_core::picture::wearer_row).
    ReconstructWearer(orbiters_core::picture::WearerRow),
    Open,
    Edit,
    SharePublicly,
    ShareTo,
    Update,
    CopyLink,
    Copy,
    CopyLayers,
    Folder,
    Reconstruct,
    StopReconstruct,
    Unqueue,
    Download,
    OnOrbiters,
    Instance,
    World,
    Select,
    Export,
}

/// A capture's reconstruction as a row of its card's menu: the press it takes, what the row says, its glyph, and why it
/// takes none (none where it takes a press). A capture without a 3D view and without what makes one has no row: its card
/// says why.
fn again_row(o: Offer, ready: bool) -> Option<(Menu, String, &'static str, Option<&'static str>)> {
    match o {
        Offer::Start { .. } => Some((Menu::Reconstruct, o.label().into(), icons::MAGIC_WAND, None)),
        Offer::Blocked { short, .. } => ready.then(|| (Menu::Reconstruct, o.label().into(), icons::MAGIC_WAND, Some(short))),
        Offer::Running { fraction, .. } | Offer::Paused { fraction, .. } => Some((Menu::StopReconstruct, tr!("Stop reconstruction · {percent}%", percent = format!("{:.0}", fraction * 100.0)), icons::X, None)),
        Offer::Waiting { .. } => Some((Menu::Unqueue, o.label().into(), icons::X, None)),
    }
}

/// A card's menu (a right press on it): what can be done with this item. A picture on this computer: Open, Edit, Share
/// publicly, Share to …, Copy (with its layers for a multi-layer picture), Show in folder; a 3D capture: Edit in 3D once
/// reconstructed, the shares, Copy public link once public, Export… once reconstructed, Reconstruct or Reconstruct again
/// (or its run to stop, or why not), Copy its picture, Open its folder; what is on Orbiters only: Ask to share
/// or the owner's shares, Copy, Download, Open on Orbiters; then the group's Instance… and World on VRChat, and Select
/// (or Deselect). With the card in a selection of several, Copy copies the selection. The picked action, once. Made only
/// as it opens and while it shows.
fn card_menu(ui: &Ui, resp: &egui::Response, e: &Entry, select: &super::gallery_select::Selection, cx: &Cx) -> Option<Menu> {
    orbiters_ui::popup::context_menu(
        ui,
        resp,
        "card-menu",
        || menu_rows(ui, resp, e, select, cx),
        |(_, label, glyph, why)| {
            let item = orbiters_ui::popup::Item::new(label).icon(glyph);
            match *why {
                Some(why) => item.detail(tr(why)).disabled(),
                None => item,
            }
        },
    )
    .map(|row| row.0)
}

/// A row of a card's menu: what it does, says, its glyph, and why it takes no press (none where it takes one).
type MenuRow = (Menu, String, &'static str, Option<&'static str>);

/// The rows of a card's menu ([`card_menu`]).
fn menu_rows(ui: &Ui, resp: &egui::Response, e: &Entry, select: &super::gallery_select::Selection, cx: &Cx) -> Vec<MenuRow> {
    let mut acts: Vec<(Menu, String, &'static str)> = vec![(Menu::Open, tr("Open").into(), icons::ARROWS_OUT)];
    // Why the row at this place takes no press.
    let mut off: Option<(usize, &'static str)> = None;
    let item = e.remote.as_ref();
    let public = item.is_some_and(|i| i.public);
    let several = select.has(&e.key) && select.keys.len() > 1;
    let copy = |what: &str| if several { tr!("Copy the {count} selected", count = select.keys.len()) } else { tr(what).to_string() };
    let mine_shares = |acts: &mut Vec<(Menu, String, &'static str)>| {
        if e.edited_since_upload() && item.is_none_or(|i| i.hidden.is_none()) {
            // As its window says it: shared (publicly or with friends), or only uploaded.
            let label = if item.is_some_and(|i| i.public || !i.grants.is_empty()) { tr("Update the shared capture") } else { tr("Update the uploaded copy") };
            acts.push((Menu::Update, label.into(), icons::CLOUD_ARROW_UP));
        }
        if e.ready && item.is_none_or(|i| i.can_share && i.hidden.is_none()) {
            if !public {
                acts.push((Menu::SharePublicly, tr("Share publicly").into(), icons::GLOBE));
            }
            acts.push((Menu::ShareTo, tr("Share to …").into(), icons::USERS));
        }
    };
    match &e.local {
        Some(controller::Local::Picture(_)) => {
            acts.push((Menu::Edit, tr("Edit").into(), icons::SLIDERS_HORIZONTAL));
            mine_shares(&mut acts);
            acts.push((Menu::Copy, copy(mark("Copy")), icons::COPY));
            if !e.layers().is_empty() && !several {
                acts.push((Menu::CopyLayers, tr("Copy with its layers").into(), icons::STACK));
            }
            acts.push((Menu::Folder, tr("Show in folder").into(), icons::FOLDER_OPEN));
        }
        Some(controller::Local::Session(_)) => {
            let row = again::offer_of(e, cx.snap).and_then(|o| again_row(o, e.ready));
            let mut reconstruct = |acts: &mut Vec<(Menu, String, &'static str)>| {
                if let Some((m, label, glyph, why)) = row.clone() {
                    if let Some(why) = why {
                        off = Some((acts.len(), why));
                    }
                    acts.push((m, label, glyph));
                }
            };
            // Without a 3D view it is the first thing to do; with one, it stands with what else is done to it.
            if !e.ready {
                reconstruct(&mut acts);
            }
            if e.ready && cx.gpu.is_some() {
                acts.push((Menu::Edit, tr("Edit in 3D").into(), icons::SLIDERS_HORIZONTAL));
            }
            mine_shares(&mut acts);
            if public {
                acts.push((Menu::CopyLink, tr("Copy public link").into(), icons::LINK_SIMPLE));
            }
            if e.ready {
                acts.push((Menu::Export, tr("Export…").into(), icons::UPLOAD_SIMPLE));
                reconstruct(&mut acts);
                // Read at the right press and kept with the card: an open menu is made every frame.
                if let Some(controller::Local::Session(dir)) = &e.local {
                    let row_id = resp.id.with("wearer-row");
                    let ctx = ui.ctx();
                    if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) && resp.contains_pointer() {
                        let row = orbiters_core::picture::wearer_row(dir);
                        ctx.data_mut(|d| d.insert_temp(row_id, row));
                    }
                    if let Some(Some(row)) = ctx.data(|d| d.get_temp::<Option<orbiters_core::picture::WearerRow>>(row_id)) {
                        if matches!(again::offer_of(e, cx.snap), Some(Offer::Start { .. })) {
                            acts.push((Menu::ReconstructWearer(row), row.label().into(), if row.takes_out() { icons::USER_MINUS } else { icons::USER_PLUS }));
                        }
                    }
                }
            }
            acts.push((Menu::Copy, copy(mark("Copy its picture")), icons::COPY));
            acts.push((Menu::Folder, tr("Open its folder").into(), icons::FOLDER_OPEN));
        }
        None => {
            match item {
                Some(i) if i.mine => mine_shares(&mut acts),
                Some(i) if i.can_share && i.hidden.is_none() => acts.push((Menu::ShareTo, tr("Ask to share").into(), icons::PAPER_PLANE_TILT)),
                _ => {}
            }
            let capture = item.is_some_and(|i| i.kind == orbiters_core::shares::Kind::Capture);
            if capture && public {
                acts.push((Menu::CopyLink, tr("Copy public link").into(), icons::LINK_SIMPLE));
            }
            // A copy of a picture from Orbiters edited (its full picture fetched), saved among this computer's.
            if !capture && super::editor::can_edit(e, cx.snap.settings.development) {
                acts.push((Menu::Edit, tr("Edit").into(), icons::SLIDERS_HORIZONTAL));
            }
            if select.can_hand_out(e) {
                acts.push((Menu::Copy, copy(if capture { mark("Copy its picture") } else { mark("Copy") }), icons::COPY));
                if !capture {
                    acts.push((Menu::Download, tr("Download").into(), icons::DOWNLOAD_SIMPLE));
                }
            }
            if super::community::owns(&e.key) || assets::owns(&e.key) || capture && public {
                acts.push((Menu::OnOrbiters, tr("Open on Orbiters").into(), icons::ARROW_SQUARE_OUT));
            }
        }
    }
    if e.instance.is_some() && !super::community::owns(&e.key) && !assets::owns(&e.key) {
        acts.push((Menu::Instance, tr("Instance…").into(), icons::INFO));
    }
    if e.world_id.starts_with("wrld_") {
        acts.push((Menu::World, tr("World on VRChat").into(), icons::GLOBE_HEMISPHERE_WEST));
    }
    let selected = select.has(&e.key);
    acts.push((Menu::Select, if selected { tr("Deselect") } else { tr("Select") }.into(), if selected { icons::X_CIRCLE } else { icons::CHECK_CIRCLE }));
    acts.into_iter().enumerate().map(|(i, (menu, label, glyph))| (menu, label, glyph, off.filter(|(at, _)| *at == i).map(|(_, why)| why))).collect()
}

/// Where a card's menu opens what it opens (as a press on the card would, the instance popover from the card).
struct Opened<'a> {
    clicked: &'a mut Option<(Entry, Rect)>,
    community: &'a mut Option<(String, Rect)>,
    asset: &'a mut Option<(String, Rect)>,
    instance: &'a mut Option<(Entry, Rect)>,
}

/// Does what a card's menu picked (`thumb`: its picture, where windows grow out of).
#[allow(clippy::too_many_arguments)]
fn act_menu(ctx: &egui::Context, cx: &mut Cx, state: &mut State, entries: &[Entry], e: Entry, thumb: Rect, m: Menu, opened: &mut Opened<'_>) {
    let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
    match m {
        Menu::Open if super::community::owns(&e.key) => *opened.community = Some((e.key.clone(), thumb)),
        Menu::Open if assets::owns(&e.key) => *opened.asset = Some((e.key.clone(), thumb)),
        Menu::Open => *opened.clicked = Some((e, thumb)),
        // A picture from Orbiters: the editor at once (what was already fetched for a drag or a copy is not fetched again).
        Menu::Edit if e.local.is_none() => {
            if let Some(source) = super::editor::source_of(&e, cx.snap.settings.development) {
                if let Some(fetched) = state.select.fetched(&e.key) {
                    super::editor::files::seed(&source, &fetched);
                }
                state.editor = Some(super::editor::Editor::open(ctx, source, e.world_name.clone()));
            }
        }
        // The item's window out of its card, then what was picked in it (one window built: it is large on the stack).
        Menu::Edit | Menu::SharePublicly | Menu::ShareTo | Menu::Update => {
            state.from_card = true;
            let radius = picture_radius(state.layout);
            let d = state.dialog.insert(Dialog::new(ctx, e, m == Menu::ShareTo).from(thumb, radius));
            match m {
                Menu::Edit => d.edit(ctx, cx.snap.settings.development),
                Menu::SharePublicly => d.public(ctx, server),
                Menu::Update => d.update(ctx, server),
                _ => {}
            }
        }
        // Edited since it was shared: its link would show it as it was. Its window updates it first, then copies the link
        // (as the window's own Copy public link does).
        Menu::CopyLink if e.edited_since_upload() && e.remote.as_ref().is_some_and(|i| i.public) => {
            state.from_card = true;
            let radius = picture_radius(state.layout);
            state.dialog.insert(Dialog::new(ctx, e, false).from(thumb, radius)).update_then_copy_link(ctx, server);
        }
        Menu::CopyLink => {
            if let Some(item) = e.remote.as_ref().filter(|i| i.public) {
                let world = if item.world.id.is_empty() { &e.world_id } else { &item.world.id };
                ctx.copy_text(sharing::public_link(server, world, item.id));
                state.select.say(ctx, tr("Public link copied: paste it in Discord or anywhere, its picture shows with it"));
            }
        }
        Menu::Copy => {
            let keys: Vec<String> =
                if state.select.has(&e.key) { entries.iter().filter(|o| state.select.has(&o.key)).map(|o| o.key.clone()).collect() } else { vec![e.key.clone()] };
            state.select.copy(ctx, keys, entries);
        }
        Menu::CopyLayers => {
            let mut files: Vec<std::path::PathBuf> = Vec::new();
            if let Some(controller::Local::Picture(p)) = e.local.as_ref().filter(|_| e.has_composite()) {
                files.push(p.clone());
            }
            files.extend(e.layers().iter().map(|l| l.path.clone()));
            state.select.copy_paths(ctx, &files);
        }
        Menu::Folder => {
            let result = match &e.local {
                Some(controller::Local::Picture(p)) => super::editor::files::reveal(p),
                Some(controller::Local::Session(d)) => crate::controller::open_path(d),
                None => Ok(()),
            };
            cx.run(ctx, result);
        }
        Menu::Reconstruct => {
            if let Some(controller::Local::Session(dir)) = &e.local {
                state.again = again::press(ctx, cx, dir, &e.world_name, e.remote.as_ref(), thumb);
            }
        }
        Menu::ReconstructWearer(row) => {
            if let Some(controller::Local::Session(dir)) = &e.local {
                match (orbiters_core::picture::WearerChoice { take_out: row.takes_out() }).save(dir) {
                    Ok(()) => state.again = again::press(ctx, cx, dir, &e.world_name, e.remote.as_ref(), thumb),
                    Err(e) => {
                        cx.run(ctx, Err(e));
                    }
                }
            }
        }
        Menu::StopReconstruct => {
            cx.ctl.stop_reconstruction();
        }
        Menu::Unqueue => {
            if let Some(controller::Local::Session(dir)) = &e.local {
                cx.ctl.unqueue_reconstruction(dir.clone());
            }
        }
        Menu::Download => {
            if let Some(job) = state.select.download_job(&e) {
                state.download_job = Job::spawn(ctx, job);
                state.select.say(ctx, tr("Saving it from Orbiters…"));
            }
        }
        Menu::OnOrbiters => {
            let link = if super::community::owns(&e.key) {
                e.world_id.strip_prefix("gallery:").map(|g| format!("{}gallery?tab=gallery-{g}", server.website()))
            } else if assets::owns(&e.key) {
                state.assets.page(&e.key, server)
            } else {
                e.remote.as_ref().filter(|i| i.public).map(|i| sharing::public_link(server, if i.world.id.is_empty() { &e.world_id } else { &i.world.id }, i.id))
            };
            if let Some(link) = link {
                let result = crate::controller::open_url(&link);
                cx.run(ctx, result);
            }
        }
        Menu::Export => {
            if let Some(controller::Local::Session(dir)) = &e.local {
                state.export = Some(super::splat_export::Export::open(ctx, dir, &e.world_name, thumb));
            }
        }
        Menu::Instance => *opened.instance = Some((e, thumb)),
        Menu::World => {
            let result = crate::controller::open_url(&format!("https://vrchat.com/home/world/{}", e.world_id));
            cx.run(ctx, result);
        }
        Menu::Select => state.select.toggle(&e.key),
    }
}

/// What a card's body takes under its picture.
const CARD_BODY: f32 = 70.0;
/// The narrowest card, as Visited worlds' (the same card): both pages get as many columns at every width.
const CARD_MIN: f32 = 270.0;
/// A picture's corners in the tile layouts (masonry, rows).
pub const TILE_RADIUS: f32 = 16.0;

/// The corners of a card's picture in `layout` (the window grows out of them).
fn picture_radius(layout: GalleryLayout) -> f32 {
    if arrangement::tiles(layout) {
        TILE_RADIUS
    } else {
        20.0
    }
}

/// Keep the anchor, which `moved` this far in the list, at its screen coordinate, while leaving normal scrolling and the
/// top of the feed alone.
fn anchored_scroll_offset(offset: f32, moved: Option<f32>) -> Option<f32> {
    let delta = moved.filter(|_| offset > 1.0)?;
    (delta.abs() > 0.1).then_some((offset + delta).max(0.0))
}

/// The shapes of the pictures the listing gave no size for, by entry key, and their revision.
#[derive(Default)]
struct Shapes {
    known: HashMap<String, f32>,
    rev: u64,
}

impl Shapes {
    /// A picture's shape from metadata or its decoded thumbnail. Layout never uses an oscillating spring.
    fn of(&self, e: &Entry) -> f32 {
        match e.size.filter(|s| s[0] > 0 && s[1] > 0) {
            Some([w, h]) => w as f32 / h as f32,
            None => self.known.get(&e.key).copied().unwrap_or(arrangement::STAND_IN),
        }
    }

    /// Its thumbnail is decoded: the list takes its shape at the next frame.
    fn learn(&mut self, ctx: &egui::Context, e: &Entry, t: &TextureHandle) {
        let shape = t.size_vec2().x / t.size_vec2().y.max(1.0);
        if self.known.get(&e.key) != Some(&shape) {
            self.known.insert(e.key.clone(), shape);
            self.rev += 1;
            anim::wake(ctx);
        }
    }
}

impl State {
    /// What the places of the list are made from ([`arrangement::Places`]).
    fn places_key(&self, layout: GalleryLayout, width: f32, spacing: f32) -> arrangement::Key {
        // (The communities' galleries decide the headings' heights: the list is made again when they change.)
        let aspects = if arrangement::tiles(layout) { self.aspects.rev } else { 0 };
        arrangement::Key { entries: self.entries_rev, aspects, layout, width, spacing }
    }

    fn make_places(&mut self, key: arrangement::Key, entries: &[Entry]) -> arrangement::Places {
        self.places_made += 1;
        let groups = groups(entries, |e| self.community.whole(e) || assets::owns(&e.key));
        let head = |first: usize| self.community.header_height(&entries[first]).unwrap_or(52.0);
        arrangement::Places::make(key, self.places_made, &groups, head, |i| self.aspects.of(&entries[i]), CARD_MIN, CARD_BODY)
    }
}

/// This computer's pictures of the worlds the member has pictures of on Orbiters are hashed, each once, to find which
/// they are. Looked at when the list is made: what decides it is what makes the list again.
fn hash_what_may_match(ctx: &egui::Context, state: &mut State) {
    if state.fixture || state.hash_job.running() {
        return;
    }
    let worlds: std::collections::HashSet<_> =
        state.feed.items.iter().filter(|i| i.mine && i.picture.is_some() && i.source_key.is_some()).map(|i| i.world.id.as_str()).collect();
    let paths: Vec<_> = state
        .locals
        .iter()
        .filter(|e| e.source_key.is_none() && worlds.contains(e.world_id.as_str()))
        .filter_map(|e| match &e.local {
            Some(controller::Local::Picture(p)) if !state.hash_asked.contains(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    if !paths.is_empty() {
        state.hash_asked.extend(paths.iter().cloned());
        state.hash_job = Job::spawn(ctx, move || crate::worlds::hashes(paths));
    }
}

/// Shimmering places for pictures still coming: `rows` of cards in the grid, a band of mixed shapes otherwise. Their
/// rects from the top left, and their height.
fn shimmer_cells(layout: GalleryLayout, width: f32, rows: usize) -> (Vec<Rect>, f32) {
    match layout {
        GalleryLayout::Grid => {
            let (cols, _) = orbiters_ui::layout::columns(width, CARD_MIN, arrangement::GAP);
            let (mut cells, _) = arrangement::grid(cols, width, CARD_MIN, CARD_BODY);
            let h = cells.first().map_or(0.0, |c| c.height());
            cells.truncate(cols);
            (cells, h)
        }
        _ => {
            let shapes = [4.0 / 3.0, 3.0 / 4.0, 16.0 / 9.0, 1.0, 3.0 / 2.0, 9.0 / 16.0, 4.0 / 3.0, 16.0 / 9.0];
            let n = if layout == GalleryLayout::Rows { 4 * rows } else { (width / arrangement::COLUMN_MAX).ceil() as usize * rows };
            let aspects: Vec<f32> = shapes.iter().copied().cycle().take(n.max(1)).collect();
            let (cells, _) = arrangement::arrange(layout, &aspects, width, CARD_MIN, CARD_BODY);
            // As tall as its shortest column: an even band.
            let h = rows as f32 * arrangement::ROW_HEIGHT;
            (cells.into_iter().filter(|c| c.top() < h).map(|c| Rect::from_min_max(c.min, pos2(c.right(), c.bottom().min(h)))).collect(), h)
        }
    }
}

/// A group's heading: the world, when, what kind of instance and how many; the Instance button on the right.
/// Returns that button's rect and whether it was pressed.
fn group_header(ui: &mut Ui, first: &Entry, last: &Entry, count: usize) -> (Rect, bool) {
    let r = ui.max_rect();
    let p = ui.painter().clone();
    let b = Button::new(tr("Instance")).icon(icons::INFO).variant(Variant::Secondary).size(Size::Sm).id_salt(("instance", &first.key));
    let bw = b.desired_width(ui);
    let br = Rect::from_min_size(pos2(r.right() - bw, r.top() + 6.0), vec2(bw, 32.0));
    widgets::text_truncated(
        &p,
        pos2(r.left(), r.top() + 12.0),
        Align2::LEFT_CENTER,
        &Txt::new(&first.world_name, 18.0).weight(weight::SEMIBOLD).tracking(-0.01),
        r.width() - bw - 24.0,
    );
    let span = super::local_span(last.taken, first.taken);
    let items = tr_n("{count} item", "{count} items", count as u64);
    let mut x = r.left();
    let y = r.top() + 36.0;
    if let Some(kind) = first.instance.as_deref().map(|i| instance_parts(i).1) {
        let t = Txt::new(tr_ctx("instance", kind), 11.0).weight(weight::SEMIBOLD).color(theme::fg_2());
        let tw = widgets::text_size(ui, &t).x;
        let pill = Rect::from_min_size(pos2(x, y - 10.0), vec2(tw + 30.0, 20.0));
        p.rect_filled(pill, theme::radius(10.0), theme::white(0.06));
        widgets::icon(&p, pos2(pill.left() + 11.0, pill.center().y), instance_glyph(kind), 11.0, theme::fg_3());
        widgets::text(&p, pos2(pill.left() + 21.0, pill.center().y), Align2::LEFT_CENTER, &t);
        x = pill.right() + 10.0;
    }
    widgets::text_truncated(&p, pos2(x, y), Align2::LEFT_CENTER, &Txt::new(format!("{span} · {items}"), 12.0).color(theme::fg_3()), br.left() - x - 16.0);
    let resp = b.show_at(ui, br);
    (br, sharing::pressed(resp))
}

/// How a card looks this frame.
struct CardLook<'a> {
    tex: Option<&'a TextureHandle>,
    /// The picture of the person who shared it with you.
    from: Option<&'a TextureHandle>,
    /// 0 while its window is open over it.
    shown: f32,
    interactive: bool,
    /// A share of it is running.
    sharing: bool,
    /// Its shared copy is being updated (Update the shared capture).
    updating: bool,
    /// Drawn by its window, riding under it: takes no input, its hover and press let go.
    riding: bool,
    /// Chosen in the selection; any card is (its check shows on every card).
    selected: bool,
    selecting: bool,
    /// Being dragged out of the window: lifted (smaller, faded) until the drop.
    carried: bool,
    /// The picture alone, its shape kept (masonry, rows): what it is on it, its time on hover; else the grid's card.
    tile: bool,
    /// An asset's picture: what it is ("Avatar picture") and what the asset is to the member ("Made by you").
    about: Option<(String, String)>,
}

/// One card, as Visited worlds' and Capture files': the picture with what it is and who sees it, then its kind,
/// time and who it is from; in the tile layouts the picture alone, in its shape, who it is from on it and its kind
/// and time over its bottom on hover. Returns its picture's rect and, when it takes input, its response (a click
/// opens or selects it, a drag takes it out of the window: [`super::gallery_select`]).
fn card(ui: &mut Ui, cx: &Cx, i: usize, e: &Entry, rect: Rect, look: &CardLook<'_>) -> (Rect, Option<egui::Response>) {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("gallery-card", &e.key));
    let resp = (!look.riding).then(|| ui.interact(rect, id, if look.interactive { Sense::click_and_drag() } else { Sense::hover() }));
    let resp = resp.map(|r| if look.interactive { r.on_hover_cursor(CursorIcon::PointingHand) } else { r });
    // Entrance: up and in, 35 ms apart.
    let delay = 0.08 + i.min(12) as f64 * 0.035;
    let elapsed = anim::time(&ctx) - cx.entered;
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
    }
    let enter = anim::spring_from(&ctx, id.with(("enter", cx.visit)), [0.0], [if elapsed >= delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
    let hover = anim::toggle(&ctx, id.with("hover"), resp.as_ref().is_some_and(|r| r.hovered()) && look.interactive, Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.as_ref().is_some_and(|r| r.is_pointer_button_down_on()) && look.interactive, Spring::SNAPPY);
    // Lifted while dragged out (it settles back with a bounce), a little inset while chosen.
    let lift = anim::toggle(&ctx, id.with("lift"), look.carried, if look.carried { Spring::SNAPPY } else { Spring::BOUNCY });
    let chosen = anim::toggle(&ctx, id.with("chosen"), look.selected, Spring::SNAPPY);
    let scale = (0.97 + 0.03 * enter) * (1.0 - 0.03 * press) * (1.0 - 0.07 * lift) * (1.0 - 0.025 * chosen);
    let r = Rect::from_center_size(rect.center() + vec2(0.0, 20.0 * (1.0 - enter) - 4.0 * hover), rect.size() * scale);
    let mut p = ui.painter().clone();
    p.multiply_opacity(enter.clamp(0.0, 1.0) * look.shown * (1.0 - 0.6 * lift.clamp(0.0, 1.0)));

    let tile = look.tile;
    let (thumb, corners) = if tile {
        (r, [TILE_RADIUS; 4])
    } else {
        widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::ink_800(), hover));
        (Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0)), [20.0, 20.0, 0.0, 0.0])
    };
    let radius = CornerRadius { nw: corners[0] as u8, ne: corners[1] as u8, se: corners[2] as u8, sw: corners[3] as u8 };
    // A layer alone keeps its transparency: over a checkerboard.
    if look.tex.is_some() && matches!(e.picture, Some(crate::worlds::Picture::Layer { .. })) {
        super::gallery_layers::checkerboard(&p, thumb, corners, 1.0);
    }
    super::sessions::thumb_shape(&p, thumb, radius, look.tex, 1.0 + 0.06 * ease::out_cubic(hover));
    let from = e.remote.as_ref().filter(|i| !i.mine).and_then(|i| i.from.as_ref());
    // A tile darkens its bottom for who it is from, and more on hover for its kind and time.
    let bottom = if tile { 0.15 + 0.6 * ease::out_cubic(hover.clamp(0.0, 1.0)) + if from.is_some() { 0.25 } else { 0.0 } } else { 0.75 };
    let shade = if tile { paint::squircle_points_corners(thumb, corners.map(theme::squircle)) } else { paint::rounded_rect_points_corners(thumb, corners) };
    paint::fill_gradient(
        &p,
        &shade,
        &Gradient::vertical(
            thumb,
            &[
                (0.0, theme::alpha(theme::ink_950(), 0.25)),
                (0.3, Color32::TRANSPARENT),
                (if tile { 0.55 } else { 0.6 }, Color32::TRANSPARENT),
                (1.0, theme::alpha(theme::ink_950(), bottom.min(0.85))),
            ],
        ),
    );

    // What it is (top left), who sees it (top right).
    let job = match &e.local {
        Some(controller::Local::Session(d)) => cx.snap.reconstruction.as_ref().filter(|j| &j.session_dir == d),
        _ => None,
    };
    let queued = matches!(&e.local, Some(controller::Local::Session(d)) if cx.snap.reconstruct_queue.contains(d));
    let (kind_label, kind_color) = if e.is_picture() {
        (tr("Picture"), theme::fg_2())
    } else if e.no_keyframes {
        (tr("No 3D"), theme::DANGER)
    } else {
        ("3D", theme::accent())
    };
    // The check on its corner; the badges step aside for it.
    let card_radius = if tile { TILE_RADIUS } else { theme::CARD_RADIUS };
    let check =
        if look.riding { 0.0 } else { super::gallery_select::paint_card(&p, &ctx, &e.key, r, thumb, card_radius, look.selected, look.selecting, hover) };
    let mut x = thumb.left() + 12.0 + (super::gallery_select::CHECK + 6.0) * check;
    // A tile says what it is when it is not a picture (most are).
    if !tile || !e.is_picture() {
        x += super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), kind_label, theme::black(0.55), kind_color, false) + 6.0;
    }
    if let Some(layers) = super::gallery_layers::label(e) {
        x += super::gallery_layers::badge(&p, ui, pos2(x, thumb.top() + 12.0), icons::STACK, &layers, false) + 6.0;
    }
    if let Some(j) = job {
        let (label, color) = super::sessions::job_badge(j, cx.snap);
        super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), &label, theme::black(0.55), color, false);
    } else if queued {
        super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), tr("Waiting to reconstruct"), theme::black(0.55), theme::fg_2(), false);
    } else if look.updating {
        super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), tr("Updating…"), theme::black(0.55), theme::accent(), false);
    } else if look.sharing {
        super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), tr("Sharing…"), theme::black(0.55), theme::accent(), false);
    }
    if let Some(item) = e.remote.as_ref().filter(|i| i.mine) {
        if item.public {
            super::sessions::badge(&p, ui, pos2(thumb.right() - 12.0, thumb.top() + 12.0), tr("Public"), theme::alpha(theme::OK, 0.22), theme::OK, true);
        } else if !item.grants.is_empty() {
            super::sessions::badge(
                &p,
                ui,
                pos2(thumb.right() - 12.0, thumb.top() + 12.0),
                &tr!("Shared · {count}", count = item.grants.len()),
                theme::black(0.55),
                theme::fg_2(),
                true,
            );
        }
    }
    if let Some(j) = job {
        // Along the picture's bottom (inside a tile's rounded corners).
        let inset = if tile { 12.0 } else { 0.0 };
        let bar = Rect::from_min_max(pos2(thumb.left() + inset, thumb.bottom() - 3.0 - inset * 0.5), pos2(thumb.right() - inset, thumb.bottom() - inset * 0.5));
        let f = anim::spring_from(&ctx, id.with("reconstruct"), [0.0], [super::sessions::overall_progress(j)], Spring::SOFT)[0].clamp(0.0, 1.0);
        let round = if tile { 1.5 } else { 0.0 };
        p.rect_filled(bar, round, theme::white(0.08));
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, 3.0)), round, super::sessions::job_bar(j));
    }

    // Kind and time, then who sees it or who it is from.
    let bx = r.left() + 16.0;
    let title = if let Some((kind, _)) = &look.about {
        kind.as_str()
    } else if e.is_picture() {
        tr("VRChat picture")
    } else if e.picture_mode == Some(orbiters_core::settings::PictureMode::World) {
        tr("3D picture of the world")
    } else if e.picture_mode == Some(orbiters_core::settings::PictureMode::People) {
        tr("3D picture of people")
    } else if e.ready {
        tr("3D capture")
    } else if e.no_keyframes {
        tr("No keyframes")
    } else {
        tr("Capture")
    };
    let time = super::local_clock(e.taken);
    if tile {
        tile_caption(&p, ui, thumb, title, &time, from.map(|f| (f.name.as_str(), look.from)), hover);
        return (thumb, resp.filter(|_| look.interactive));
    }
    let tt = Txt::new(&time, 12.0).color(theme::fg_3());
    let tw = widgets::text_size(ui, &tt).x;
    widgets::text(&p, pos2(r.right() - 16.0, thumb.bottom() + 22.0), Align2::RIGHT_CENTER, &tt);
    widgets::text_truncated(&p, pos2(bx, thumb.bottom() + 22.0), Align2::LEFT_CENTER, &Txt::new(title, 14.5).weight(weight::SEMIBOLD), r.width() - 48.0 - tw);
    let line_y = thumb.bottom() + 46.0;
    if let Some(from) = from {
        let ar = Rect::from_center_size(pos2(bx + 9.0, line_y), vec2(18.0, 18.0));
        widgets::avatar(&p, ar, look.from, &from.name);
        widgets::text_truncated(
            &p,
            pos2(ar.right() + 7.0, line_y),
            Align2::LEFT_CENTER,
            &Txt::new(tr!("from {name}", name = from.name), 12.0).weight(weight::MEDIUM).color(theme::fg_2()),
            r.right() - ar.right() - 23.0,
        );
    } else {
        let (glyph, label, color) = match &e.remote {
            // Its splats edited since it was uploaded: what Orbiters shows is not what its 3D view shows.
            _ if e.edited_since_upload() && !look.updating => {
                let shared = e.remote.as_ref().is_some_and(|i| i.public || !i.grants.is_empty());
                let label = match (e.reconstructed_since_upload(), shared) {
                    (true, true) => tr("Reconstructed since shared: update it"),
                    (true, false) => tr("Reconstructed since uploaded: update it"),
                    (false, true) => tr("Edited since shared: update it"),
                    (false, false) => tr("Edited since uploaded: update it"),
                };
                (icons::CLOUD_ARROW_UP, label.to_string(), theme::WARN)
            }
            Some(i) if i.public => (icons::GLOBE, tr("Public on Orbiters").to_string(), theme::OK),
            Some(i) if !i.grants.is_empty() => (icons::USERS, tr!("Shared with {count}", count = i.grants.len()), theme::fg_2()),
            Some(_) => (icons::LOCK_SIMPLE, tr("Only you").to_string(), theme::fg_3()),
            None if look.about.is_some() => (icons::TAG, look.about.as_ref().map(|a| a.1.clone()).unwrap_or_default(), theme::accent()),
            None if e.no_keyframes => (icons::WARNING, tr("Recorded without the capture strip: nothing to make 3D from").to_string(), theme::DANGER),
            None if !e.ready && !e.is_picture() => (icons::MAGIC_WAND, tr("Reconstruct it to share it").to_string(), theme::fg_3()),
            None => (icons::HARD_DRIVE, tr("On this computer").to_string(), theme::fg_3()),
        };
        widgets::icon(&p, pos2(bx + 7.0, line_y), glyph, 13.0, color);
        widgets::text_truncated(&p, pos2(bx + 20.0, line_y), Align2::LEFT_CENTER, &Txt::new(label, 12.0).color(color), r.width() - 52.0);
    }
    (thumb, resp.filter(|_| look.interactive))
}

/// Over a tile's bottom: who shared it with you (always), its kind and time (on hover).
fn tile_caption(p: &egui::Painter, ui: &Ui, thumb: Rect, title: &str, time: &str, from: Option<(&str, Option<&TextureHandle>)>, hover: f32) {
    let y = thumb.bottom() - 12.0 - 11.0;
    let mut left = thumb.left() + 12.0;
    let tt = Txt::new(time, 11.5).weight(weight::MEDIUM).color(theme::fg_2());
    let tw = widgets::text_size(ui, &tt).x;
    if let Some((name, tex)) = from {
        let t = Txt::new(tr!("from {name}", name = name), 11.5).weight(weight::SEMIBOLD).color(theme::fg());
        let w = (widgets::text_size(ui, &t).x + 34.0).min(thumb.width() - 24.0);
        let pill = Rect::from_min_size(pos2(left, y - 11.0), vec2(w, 22.0));
        p.rect_filled(pill, theme::radius(11.0), theme::black(0.55));
        widgets::avatar(p, Rect::from_center_size(pos2(pill.left() + 11.0, pill.center().y), vec2(18.0, 18.0)), tex, name);
        widgets::text_truncated(p, pos2(pill.left() + 25.0, pill.center().y), Align2::LEFT_CENTER, &t, w - 32.0);
        left = pill.right() + 8.0;
    }
    let k = ease::out_cubic(hover.clamp(0.0, 1.0));
    if k < 0.01 {
        return;
    }
    let mut hp = p.clone();
    hp.multiply_opacity(k);
    let dy = 4.0 * (1.0 - k);
    widgets::text(&hp, pos2(thumb.right() - 12.0, y + dy), Align2::RIGHT_CENTER, &tt);
    let room = thumb.right() - 12.0 - tw - 10.0 - left;
    if room > 40.0 {
        widgets::text_truncated(&hp, pos2(left, y + dy), Align2::LEFT_CENTER, &Txt::new(title, 12.5).weight(weight::SEMIBOLD), room);
    }
}

fn empty_state(ui: &mut Ui, cx: &Cx, filter: Filter, searching: bool, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(260.0)), Sense::hover());
    let (rect, alpha) = super::entering(ui, cx, 1, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let t = anim::clock(ui.ctx(), 30.0);
    let bob = -8.0 * ease::in_out(ease::mirror(t, 2.0));
    let lr = Rect::from_center_size(rect.center() + vec2(0.0, -44.0 + bob), logo::size_for_width(70.0));
    logo::paint(&p, lr, logo::Look::default());
    let (title, text) = if searching {
        (tr("No world matches"), tr("Try another world name."))
    } else if filter == Filter::Assets {
        (
            tr("Your assets' pictures belong here"),
            tr("The pictures of the avatars and other assets you made or own on Orbiters show up here, one group for each."),
        )
    } else if filter == Filter::Communities {
        (
            tr("Your communities' pictures belong here"),
            tr("Pictures posted in the galleries of the Discord servers you're in show up here, with the events they were taken at."),
        )
    } else if filter == Filter::Shared {
        (tr("Nothing shared with you yet"), tr("When friends share a picture or a 3D capture with you, it shows up here."))
    } else if cfg!(target_os = "macos") {
        (tr("Your moments belong here"), tr("VRChat and N4N0's captures come from a PC. Add the folder with your VRChat pictures through Manage folders, or copy captures into your Sessions folder, and they show up here together."))
    } else {
        (tr("Your moments belong here"), tr("VRChat pictures and N4N0's 3D captures show up here together, grouped by the instance you took them in."))
    };
    widgets::text(&p, rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 13.0).color(theme::fg_3()).line_height(20.0).job(400.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y + 34.0), g, theme::fg_3());
}

/// The instance a group was taken in, in a popover that grows out of its button.
fn instance_popover(ctx: &egui::Context, cx: &mut Cx, state: &mut State) {
    if let Some(people) = state.roster_job.take() {
        if let Some(i) = state.instance.as_mut() {
            i.entry.people = people;
        }
    }
    let Some(inst) = state.instance.as_ref() else { return };
    let e = inst.entry.clone();
    let scope = crate::controller::vrchat_data::Scope::of(cx.snap);
    if !state.fixture && state.instance_scope.as_ref() != Some(&scope) {
        state.instance_scope = Some(scope.clone());
        state.instance_job = Job::default();
        state.instance_checked = false;
        state.instance_alive = None;
        state.instance_error = None;
    }
    if !state.fixture && !state.instance_checked {
        state.instance_checked = true;
        if let Some(instance) = e.instance.clone() {
            let backend = cx.ctl.clone();
            let world = e.world_id.clone();
            state.instance_job = Job::spawn(ctx, move || {
                use crate::controller::vrchat_data::{Request, Response};
                match backend.vrchat_data(scope, Request::Instance { world, instance })? {
                    Response::Instance(alive) => Ok(alive),
                    _ => Err(tr("Unexpected instance response.").into()),
                }
            });
        }
    }
    if let Some(result) = state.instance_job.take() {
        match result {
            Ok(alive) => state.instance_alive = alive,
            Err(e) => state.instance_error = Some(e),
        }
    }
    let Some(inst) = state.instance.as_ref() else { return };
    let id = Id::unique(("instance-popover", inst.serial));
    let (anchor, closing) = (inst.anchor, inst.closing);
    let current = cx.snap.vrchat.running && is_current(&cx.snap.vrchat.world, &e);
    let people = if current { cx.snap.vrchat.world.players.values().cloned().collect::<Vec<_>>() } else { e.people.clone() };
    let width = 380.0;
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, id));
    let widths: Vec<f32> = people.iter().map(|n| 34.0 + painter.layout_job(person_txt(n).job(f32::INFINITY)).size().x + 14.0).collect();
    let inner = width - 40.0;
    let (at, people_h) = widgets::flow(&widths, inner, widgets::CHIP_H, 8.0);
    let join = join_available(current, state.instance_alive) && e.instance.is_some();
    let h = 20.0 + 44.0 + 16.0 + 14.0 + 10.0 + people_h.max(20.0) + 18.0 + 22.0 + 16.0 + 40.0 + 20.0 + if state.instance_error.is_some() { 40.0 } else { 0.0 };
    let base = Rect::from_min_size(pos2(anchor.right() - width, anchor.bottom() + 8.0), vec2(width, h));
    // Grows from the button's corner, and back into it.
    let pop = orbiters_ui::popup::popover(ctx, id, anchor, base, !closing);
    if pop.gone {
        state.instance = None;
        return;
    }
    let (rect, kk) = (pop.rect, pop.opacity);
    let mut close = false;
    let mut open_link = None;
    let mut retry = false;
    let area = orbiters_ui::front::area(ctx, id, Order::Foreground, !closing).fixed_pos(rect.min).fade_in(false).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        ui.multiply_opacity(kk);
        let p = ui.painter().clone();
        widgets::panel(&p, rect, 18.0);
        let mut y = rect.top() + 20.0;
        let x = rect.left() + 20.0;
        let (number, kind, region) = e.instance.as_deref().map(instance_parts).unwrap_or(("", mark("Instance not recorded"), ""));
        let tile = Rect::from_min_size(pos2(x, y + 2.0), vec2(40.0, 40.0));
        p.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::accent(), 0.12));
        widgets::icon(&p, tile.center(), instance_glyph(kind), 19.0, theme::accent());
        widgets::text_truncated(
            &p,
            pos2(tile.right() + 14.0, y + 12.0),
            Align2::LEFT_CENTER,
            &Txt::new(&e.world_name, 15.5).weight(weight::SEMIBOLD),
            inner - 54.0,
        );
        let mut meta = vec![tr_ctx("instance", kind).to_string()];
        if !number.is_empty() {
            meta.push(format!("#{number}"));
        }
        if !region.is_empty() {
            meta.push(region.to_uppercase());
        }
        widgets::text_truncated(
            &p,
            pos2(tile.right() + 14.0, y + 32.0),
            Align2::LEFT_CENTER,
            &Txt::new(meta.join(" · "), 12.0).color(theme::fg_3()),
            inner - 54.0,
        );
        y += 44.0 + 16.0;
        widgets::text(&p, pos2(x, y + 7.0), Align2::LEFT_CENTER, &Txt::caps(if current { tr("Here now") } else { tr("Who was there") }));
        y += 14.0 + 10.0;
        if people.is_empty() {
            widgets::text(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, &Txt::new(tr("Not recorded: VRCX keeps who was there"), 12.5).color(theme::fg_3()));
        } else {
            for (n, name) in people.iter().enumerate() {
                let cr = Rect::from_min_size(pos2(x, y) + at[n], vec2(widths[n].min(inner), widgets::CHIP_H));
                p.rect_filled(cr, theme::radius(cr.height() * 0.5), theme::control());
                widgets::avatar(&p, Rect::from_center_size(pos2(cr.left() + 16.0, cr.center().y), vec2(24.0, 24.0)), None, name);
                widgets::text_truncated(&p, pos2(cr.left() + 34.0, cr.center().y), Align2::LEFT_CENTER, &person_txt(name), cr.width() - 46.0);
            }
        }
        y += people_h.max(20.0) + 18.0;
        let (dot, status) = if current {
            (Tone::Ok, tr("You are in this instance now"))
        } else if state.instance_alive == Some(true) {
            (Tone::Ok, tr("Still open: you can join it again"))
        } else if state.instance_alive == Some(false) {
            (Tone::Off, tr("Not open any more"))
        } else if state.instance_job.running() {
            (Tone::Info, tr("Checking whether it is still open…"))
        } else if e.instance.is_none() {
            (Tone::Off, tr("The instance was not recorded for this moment"))
        } else {
            (Tone::Off, tr("Joining works for instances still open"))
        };
        if state.instance_job.running() {
            feedback::spinner(&p, pos2(x + 5.0, y + 11.0), 5.0, 1.8, theme::CYAN);
        } else {
            widgets::status_dot(&p, pos2(x + 5.0, y + 11.0), dot, dot == Tone::Ok);
        }
        widgets::text_truncated(&p, pos2(x + 18.0, y + 11.0), Align2::LEFT_CENTER, &Txt::new(status, 12.5).color(theme::fg_2()), inner - 18.0);
        y += 22.0 + 16.0;
        if let Some(err) = &state.instance_error {
            widgets::text_truncated(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, &Txt::new(err, 12.0).color(theme::DANGER), inner - 110.0);
            // The whole of a long answer on hover.
            ui.interact(Rect::from_min_size(pos2(x, y), vec2(inner - 110.0, 20.0)), Id::unique("instance-error"), Sense::hover()).on_hover_text(err);
            let b = Button::new(tr("Try again")).size(Size::Sm).id_salt("instance-retry");
            let bw = b.desired_width(ui);
            if sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(rect.right() - 20.0 - bw, y - 6.0), vec2(bw, 32.0)))) {
                retry = true;
            }
            y += 40.0;
        }
        let br = Rect::from_min_size(pos2(x, y), vec2(inner, 40.0));
        // Green only while it can be joined: a closed or unknown instance keeps a quiet, disabled button.
        let open = Button::new(tr("Join in VRChat"))
            .icon(icons::ARROW_SQUARE_OUT)
            .variant(if join { Variant::Primary } else { Variant::Secondary })
            .enabled(join)
            .width(inner)
            .id_salt("instance-join");
        let open = if join { open } else { open.hint(tr("Only an instance that is still open can be joined")) };
        if sharing::pressed(open.show_at(ui, br)) {
            if let Some(instance) = &e.instance {
                open_link = Some(format!("https://vrchat.com/home/launch?worldId={}&instanceId={}", e.world_id, encode(instance)));
            }
        }
    });
    if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| anchor.contains(p))) {
        close = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if retry {
        state.instance_error = None;
        state.instance_checked = false;
    }
    if let Some(link) = open_link {
        let r = crate::controller::open_url(&link);
        cx.run(ctx, r);
        close = true;
    }
    if close {
        if let Some(i) = state.instance.as_mut() {
            i.closing = true;
        }
    }
}

fn person_txt(name: &str) -> Txt {
    Txt::new(name, 12.5).weight(weight::MEDIUM).color(theme::fg_2())
}

/// Runs of pictures of one instance, or of one world (or gallery) and day; `whole` groups (an event's gallery) span days.
fn groups(entries: &[Entry], whole: impl Fn(&Entry) -> bool) -> Vec<std::ops::Range<usize>> {
    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let same = groups.last().is_some_and(|g| {
            let a = &entries[g.start];
            a.world_id == e.world_id && a.instance == e.instance && (a.instance.is_some() || a.taken / 86400 == e.taken / 86400 || whole(a))
        });
        match groups.last_mut() {
            Some(g) if same => g.end = i + 1,
            _ => groups.push(i..i + 1),
        }
    }
    groups
}
fn is_current(world: &WorldState, e: &Entry) -> bool {
    world.in_world && world.world_id.as_ref() == Some(&e.world_id) && e.instance.is_some() && world.instance == e.instance
}
pub(crate) fn join_available(current: bool, alive: Option<bool>) -> bool {
    alive.unwrap_or(current)
}

/// An instance id ("12345~friends(usr_…)~region(eu)") as its number, its kind and its region.
fn instance_parts(instance: &str) -> (&str, &'static str, &str) {
    let number = instance.split('~').next().unwrap_or("");
    let region = instance.split('~').find_map(|p| p.strip_prefix("region(").and_then(|r| r.strip_suffix(')'))).unwrap_or("");
    (number, instance_type(instance), region)
}

fn instance_type(instance: &str) -> &'static str {
    if instance.contains("~private(") {
        if instance.contains("~canRequestInvite") {
            mark("Invite+")
        } else {
            mark("Invite only")
        }
    } else if instance.contains("~friends(") {
        mark("Friends")
    } else if instance.contains("~hidden(") {
        mark("Friends+")
    } else if instance.contains("~group(") {
        mark("Group")
    } else {
        mark("Public")
    }
}

fn instance_glyph(kind: &str) -> &'static str {
    match kind {
        "Friends" | "Friends+" => icons::USERS, // i18n: not shown (matches instance_type's English)
        "Invite only" | "Invite+" => icons::ENVELOPE_SIMPLE, // i18n: not shown
        "Group" => icons::USERS, // i18n: not shown
        "Public" => icons::GLOBE, // i18n: not shown
        _ => icons::INFO,
    }
}

fn encode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[cfg(test)]
mod tests {
    /// A card's menu row of a capture: what it offers, says and takes in each state of its reconstruction.
    #[test]
    fn a_cards_menu_says_where_its_reconstruction_stands() {
        use super::{again_row, Menu, Offer};
        let (menu, label, _, why) = again_row(Offer::Start { again: true }, true).unwrap();
        assert_eq!((menu, label.as_str(), why), (Menu::Reconstruct, "Reconstruct again", None));
        let (menu, label, _, why) = again_row(Offer::Start { again: false }, false).unwrap();
        assert_eq!((menu, label.as_str(), why), (Menu::Reconstruct, "Reconstruct", None));
        let blocked = Offer::Blocked { again: true, reason: "why", short: "capture files deleted" };
        let (menu, label, _, why) = again_row(blocked, true).unwrap();
        assert_eq!((menu, label.as_str(), why), (Menu::Reconstruct, "Reconstruct again", Some("capture files deleted")), "dim, and why");
        assert!(again_row(Offer::Blocked { again: false, reason: "why", short: "no keyframes" }, false).is_none(), "without a 3D view its card says why");
        let (menu, label, _, why) = again_row(Offer::Running { fraction: 0.42, stage: "Training splats" }, true).unwrap();
        assert_eq!((menu, label.as_str(), why), (Menu::StopReconstruct, "Stop reconstruction · 42%", None));
        let (menu, label, _, why) = again_row(Offer::Waiting { hint: "Starts in a moment" }, true).unwrap();
        assert_eq!((menu, label.as_str(), why), (Menu::Unqueue, "Waiting · cancel", None));
    }

    use super::*;
    /// The list is made again when the watch saw, after the list was made, another revision than the one it was made
    /// from; a look begun before the list says nothing of it. (The first look used to be taken as the list's own
    /// state: a capture deleted between the list and that look stayed on the page.)
    #[test]
    fn a_list_is_out_of_date_when_a_later_look_saw_something_else() {
        let t = std::time::Instant::now();
        let later = t + std::time::Duration::from_secs(1);
        assert!(!list_is_stale(None, None));
        assert!(!list_is_stale(Some((t, 1)), None), "no list yet: it is being made");
        assert!(!list_is_stale(None, Some((t, 1))), "no look yet");
        assert!(!list_is_stale(Some((later, 1)), Some((t, 1))), "the same");
        assert!(list_is_stale(Some((later, 2)), Some((t, 1))), "changed since the list was made");
        assert!(!list_is_stale(Some((t, 2)), Some((later, 1))), "a look from before the list: the list is the newer");
    }

    /// The whole way from a change outside the app to the list on the page, with nothing of the member's read: the
    /// captures, the visit history and the upload records are files of the test's. A capture's folder is deleted, the
    /// history learns of an instance and an upload is recorded, all from outside, the pictures' index untouched and
    /// the page not visited again: the page's list is made again by itself, without the deleted capture, the other
    /// one filed under the instance it was taken in and marked as uploaded, and what the person had typed and where
    /// they had scrolled is as it was.
    #[test]
    fn a_change_from_outside_reaches_the_list_on_the_page_without_a_visit() {
        use orbiters_core::contracts::manifest::{SessionManifest, WorldInfo, MANIFEST_FILE, MANIFEST_FORMAT};
        use orbiters_core::visited::{History, InstanceVisit};
        let root = std::env::temp_dir().join(format!("orbiters-gallery-delivery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let captures = root.join("Sessions");
        let capture = |name: &str, created: &str| {
            let dir = captures.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let world = WorldInfo { id: Some("wrld_a".into()), name: Some("World A".into()), ..Default::default() };
            let manifest = SessionManifest { format: MANIFEST_FORMAT.into(), created_utc: created.into(), world: Some(world), ..Default::default() };
            std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec(&manifest).unwrap()).unwrap();
            dir
        };
        let (one, two) = (capture("one", "2026-11-09T20:00:00Z"), capture("two", "2026-11-10T20:00:00Z"));
        let (history_file, records_file) = (root.join("visited-worlds.json"), root.join("capture-uploads.json"));
        let mut history = History::default();
        history.save(&history_file).unwrap();

        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let mut snap = (*ctl.view()).clone();
        snap.settings.sessions_dir = captures.clone();
        let _pictures = crate::worlds::tests::pictures_lock();
        let pictures = crate::worlds::picture_revision();
        // With what the person did on the page: a search typed, the list scrolled.
        let mut state = State { sources: Some((history_file.clone(), Some(records_file.clone()))), query: "world".into(), scroll_offset: 240.0, ..Default::default() };
        state.folders.set_sources(history_file.clone(), Some(records_file.clone()));
        let mut time = 0.0;
        let mut frame = |state: &mut State| {
            time += 1.0 / 60.0;
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut output = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                follow_locals(state, ui, &cx, Server::Production);
            });
            output.textures_delta.clear();
        };
        // Frames as they are asked for, until the list is as `done` wants it.
        let mut until = |state: &mut State, what: &str, done: &dyn Fn(&State) -> bool| {
            let limit = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                frame(state);
                if done(state) {
                    return;
                }
                assert!(std::time::Instant::now() < limit, "{what}: {} entries", state.locals.len());
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        let is = |entry: &Entry, dir: &std::path::Path| matches!(&entry.local, Some(controller::Local::Session(d)) if d == dir);

        until(&mut state, "the first list", &|s| s.locals.len() == 2 && s.folders.local_seen().is_some());
        assert!(state.locals.iter().any(|e| is(e, &one)) && state.locals.iter().any(|e| is(e, &two)));
        assert!(state.locals.iter().all(|e| e.instance.is_none() && e.remote.is_none()), "no instance known, nothing uploaded");
        let made = state.listed;

        // From outside: a capture's folder goes, the history learns where the other was taken, its upload is recorded.
        std::fs::remove_dir_all(&one).unwrap();
        history.instances.push(InstanceVisit { world_id: "wrld_a".into(), instance: "77~private".into(), at: controller::unix("2026-11-10T19:30:00Z") });
        history.save(&history_file).unwrap();
        let item = orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Capture, id: 9, mine: true, ..Default::default() };
        controller::write_record_for_test(&records_file, &two, item);
        state.folders.look_now();

        until(&mut state, "the list made again", &|s| s.listed != made && !s.local_job.running());
        assert_eq!(state.locals.len(), 1, "the deleted capture is gone from the page");
        let left = &state.locals[0];
        assert!(is(left, &two));
        assert_eq!(left.instance.as_deref(), Some("77~private"), "filed under the instance the history learned of");
        assert_eq!(left.remote.as_ref().map(|item| item.id), Some(9), "and marked as uploaded");
        // Nothing the person did was touched, the pictures' index did not change, and no visit was needed.
        assert_eq!((state.query.as_str(), state.scroll_offset), ("world", 240.0));
        assert_eq!(crate::worlds::picture_revision(), pictures);
        // And it rests: the look that follows sees what the list was made from.
        let settled = state.listed;
        state.folders.look_now();
        std::thread::sleep(std::time::Duration::from_millis(150));
        for _ in 0..5 {
            frame(&mut state);
        }
        assert!(state.listed == settled && !state.local_job.running(), "made once, not again and again");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Night PAGES-UI-23: while Orbiters cannot be reached the account's check comes and goes every minute (Offline,
    /// Checking, Offline): the same person, so the decoded pictures stay and the list is not made again. Orbiters
    /// answering again asks the feed again; another name is another account and starts over.
    #[test]
    fn the_account_check_coming_and_going_is_not_another_account() {
        use crate::state::AccountState as A;
        let root = std::env::temp_dir().join(format!("orbiters-gallery-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Sessions")).unwrap();
        let history_file = root.join("visited-worlds.json");
        orbiters_core::visited::History::default().save(&history_file).unwrap();
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let mut snap = (*ctl.view()).clone();
        snap.settings.sessions_dir = root.join("Sessions");
        let mut state = State { sources: Some((history_file.clone(), None)), ..Default::default() };
        state.folders.set_sources(history_file, None);
        let mut time = 0.0;
        let mut frame = |state: &mut State, account: A| {
            // (The member's key, as the service gives it with them.)
            snap.account.member = account.username().map(|u| format!("production/{u}"));
            snap.account.state = account;
            time += 1.0 / 60.0;
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut output = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                follow_locals(state, ui, &cx, Server::Production);
            });
            output.textures_delta.clear();
        };
        let offline = || A::Offline { username: Some("x".into()) };
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while state.listed.is_none() || state.local_job.running() {
            frame(&mut state, offline());
            assert!(std::time::Instant::now() < limit, "the first list");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.pictures = Some(crate::worlds::Pictures::new(&ctx));
        state.asked = Some((Filter::All, String::new()));
        let listed = state.listed;
        for account in [A::Checking { username: Some("x".into()) }, offline(), A::Checking { username: Some("x".into()) }, offline()] {
            frame(&mut state, account);
            assert!(!state.local_job.running(), "the list is not made again");
            assert!(state.pictures.is_some(), "the decoded pictures stay");
            assert!(state.asked.is_some(), "nothing new to ask while Orbiters cannot be reached");
        }
        assert_eq!(state.listed, listed);
        frame(&mut state, A::SignedIn { username: "x".into() });
        assert!(state.pictures.is_some() && !state.local_job.running(), "Orbiters answering is not another account");
        assert!(state.asked.is_none(), "but the feed is asked again");
        state.asked = Some((Filter::All, String::new()));
        frame(&mut state, A::Checking { username: Some("x".into()) });
        frame(&mut state, A::SignedIn { username: "x".into() });
        assert!(state.asked.is_some(), "a check that confirms the sign-in asks nothing again");
        frame(&mut state, A::SignedIn { username: "y".into() });
        assert!(state.pictures.is_none(), "another account starts over");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Night3, verifier point 7: the row stacks only when a filter's words would reach the layout picker, not when the
    /// two tracks only touch. English at 1225 wide (a head of 929 points; filters 420, Requests 113) stays on one row as
    /// it was; Portuguese there (filters 457) stacks.
    #[test]
    fn the_filters_row_stacks_only_when_words_would_reach_the_picker() {
        assert!(!row_stacks(929.0, 420.0, 113.0), "English at 1225: the tracks touch, no words overlap");
        assert!(row_stacks(929.0, 457.0, 113.0), "Portuguese at 1225: its last filter would run under the picker");
        assert!(!row_stacks(984.0, 457.0, 113.0), "Portuguese at 1280: room for all");
        assert!(row_stacks(919.0, 300.0, 113.0), "under 920 points of head, always two rows");
    }

    /// Verifier (night3, point 1): another account, or the same one whose sign-in was refused, takes everything of the
    /// old one off the screen at once, while this computer's folders are still being read: its feed, the decoded
    /// pictures, an editor of one of its pictures. (They stayed until the scan ended.) An editor of a picture of this
    /// computer stays.
    #[test]
    fn another_account_takes_the_old_ones_pictures_away_at_once_even_during_a_scan() {
        use crate::state::AccountState as A;
        use super::super::editor::{files::{Remote, RemoteKind}, Editor, Source};
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let mut snap = (*ctl.view()).clone();
        let root = std::env::temp_dir().join(format!("orbiters-gallery-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Sessions")).unwrap();
        let history_file = root.join("visited-worlds.json");
        orbiters_core::visited::History::default().save(&history_file).unwrap();
        snap.settings.sessions_dir = root.join("Sessions");
        let mut state = State { fixture: true, sources: Some((history_file, None)), ..Default::default() };
        let mut frame = |state: &mut State, account: A| {
            // (The member's key, as the service gives it with them.)
            snap.account.member = account.username().map(|u| format!("production/{u}"));
            snap.account.state = account;
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                follow_locals(state, ui, &cx, Server::Production);
            });
            output.textures_delta.clear();
        };
        let signed_in = |name: &str| A::SignedIn { username: name.into() };
        let remote_editor = || Editor::open(&ctx, Source::remote(Remote { kind: RemoteKind::Gallery, id: 12, ..Default::default() }), "World");
        for change in [signed_in("y"), A::Expired { username: Some("x".into()) }] {
            frame(&mut state, signed_in("x"));
            // A scan of this computer's folders that does not end, made for x: it read x's upload records (an entry
            // shared by x, `x_key`).
            let (hold, held) = std::sync::mpsc::channel::<()>();
            let x = state.account.owner().cloned().expect("x seen");
            let x_entry = controller::remote_entry(orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Picture, id: 77, mine: true, ..Default::default() });
            let x_key = x_entry.key.clone();
            state.local_job = Job::spawn(&ctx, move || {
                let _ = held.recv();
                super::super::ForAccount::new(&x, (std::time::Instant::now(), 0, vec![x_entry]))
            });
            // What x has on screen: a page of its feed, decoded pictures, the editor of one of its pictures.
            state.feed = Feed { items: vec![orbiters_core::shares::Item::default()], ..Default::default() };
            state.pictures = Some(crate::worlds::Pictures::new(&ctx));
            state.editor = Some(remote_editor());
            // And a picture of x carried out of its window, its file arriving (verify2: it stayed, and could still be
            // handed to Windows while the button was down).
            let carried = controller::remote_entry(orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Picture, id: 78, ..Default::default() });
            state.select.stage_carried(&carried, root.join("x-picture.png"));
            frame(&mut state, change.clone());
            assert!(!state.select.keeps_anything(), "{change:?}: nothing of x's to drag, copy or fetch");
            assert!(state.local_job.running(), "the scan is still running");
            assert!(state.feed.items.is_empty(), "{change:?}: the old account's feed goes at once");
            assert!(state.pictures.is_none(), "{change:?}: and its decoded pictures");
            assert!(state.editor.is_none(), "{change:?}: and the editor of one of its pictures");
            // Night4, verify2: x's scan ends after the change: its list is never taken in (it was, wholesale, with x's
            // sharing and associations); it is made again for who is signed in now.
            drop(hold);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                frame(&mut state, change.clone());
                assert!(!state.locals.iter().any(|e| e.key == x_key), "{change:?}: x's list landed after the change");
                if !state.local_job.running() {
                    break;
                }
                assert!(std::time::Instant::now() < until, "the scans never ended");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(state.listed.is_some(), "{change:?}: the list was made again");
        }
        // An editor of a picture of this computer is the person's own work: it stays.
        frame(&mut state, signed_in("x"));
        let path = std::env::temp_dir().join("orbiters-test-local-edit.png");
        state.editor = Some(Editor::open(&ctx, Source::picture(&path), "World"));
        frame(&mut state, signed_in("z"));
        assert!(state.editor.is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A capture of this computer shared publicly, then edited: its card says so where it says who sees it ("Edited since
    /// shared: update it"), and "Updating…" while the update runs; painted on the CPU into `ORBITERS_TEST_OUT`
    /// (`card-edited.png`, `card-updating.png`) when it is set.
    #[test]
    fn an_edited_shared_capture_says_so_on_its_card() {
        use egui::RawInput;
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let snap = ctl.view();
        let mut e = controller::remote_entry(orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Capture, id: 3, mine: true, public: true, ..Default::default() });
        e.world_name = "Exoplanet Journey".into();
        e.taken = controller::unix("2026-11-02T13:30:00Z");
        e.local = Some(controller::Local::Session(std::path::PathBuf::from("C:/Sessions/Exoplanet Journey 2026-11-02 14-30-00")));
        e.ready = true;
        e.edits = Some(Ok("0f3a9c2be1d47a60-5b1e0c9d2a7f3e18".into()));
        e.uploaded_edits = Some(orbiters_reconstruct::edit::UNEDITED.into());
        assert!(e.edited_since_upload());
        let size = vec2(380.0, 320.0);
        let mut painter = std::env::var_os("ORBITERS_TEST_OUT").map(|d| (std::path::PathBuf::from(d), orbiters_ui::software::Software::new(size.x as u32, size.y as u32)));
        for (name, updating) in [("edited", false), ("updating", true)] {
            let mut shapes = None;
            for frame in 0..90 {
                let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)), time: Some(frame as f64 / 60.0 + 10.0), ..Default::default() };
                let mut toasts = orbiters_ui::toast::Toasts::default();
                let mut output = ctx.run_ui(input, |ui| {
                    let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                    let look = CardLook {
                        tex: None,
                        from: None,
                        shown: 1.0,
                        interactive: true,
                        sharing: false,
                        updating,
                        riding: false,
                        selected: false,
                        selecting: false,
                        carried: false,
                        tile: false,
                        about: None,
                    };
                    card(ui, &cx, 0, &e, Rect::from_min_size(pos2(24.0, 24.0), vec2(332.0, 262.0)), &look);
                });
                if let Some((_, sw)) = painter.as_mut() {
                    sw.set_textures(&mut output.textures_delta);
                    sw.free_textures(&mut output.textures_delta);
                    shapes = Some((output.shapes, output.pixels_per_point));
                }
                output.textures_delta.clear();
            }
            if let (Some((dir, sw)), Some((shapes, ppp))) = (painter.as_mut(), shapes) {
                let primitives = ctx.tessellate(shapes, ppp);
                let pixels = sw.paint(&primitives, ppp, egui::Color32::from_rgb(9, 9, 11));
                let image = egui::ColorImage::from_rgba_premultiplied([size.x as usize, size.y as usize], pixels);
                std::fs::create_dir_all(&*dir).unwrap();
                orbiters_ui::offscreen::save_png(&dir.join(format!("card-{name}.png")), &image).unwrap();
            }
        }
    }

    /// Copy public link on the card of a capture edited since it was shared never copies the link of the version shared
    /// before: its window opens, updates it and copies the link once that is done (as its own Copy public link does). An
    /// unedited capture's link is copied at once, without a window.
    #[test]
    fn copy_public_link_on_an_edited_shared_capture_updates_it_first() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let snap = ctl.view();
        let mut e = controller::remote_entry(orbiters_core::shares::Item {
            kind: orbiters_core::shares::Kind::Capture,
            id: 3,
            mine: true,
            public: true,
            can_share: true,
            ..Default::default()
        });
        e.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        e.local = Some(controller::Local::Session(std::path::PathBuf::from("C:/Sessions/Exoplanet Journey 2026-11-02 14-30-00")));
        e.ready = true;
        e.edits = Some(Ok("0f3a9c2be1d47a60-5b1e0c9d2a7f3e18".into()));
        e.uploaded_edits = Some(orbiters_reconstruct::edit::UNEDITED.into());
        let copy_link = |e: &Entry| {
            let mut state = State::default();
            let (mut clicked, mut community, mut asset, mut instance) = (None, None, None, None);
            let mut opened = Opened { clicked: &mut clicked, community: &mut community, asset: &mut asset, instance: &mut instance };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
            act_menu(&ctx, &mut cx, &mut state, &[], e.clone(), Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 120.0)), Menu::CopyLink, &mut opened);
            state.dialog.as_ref().and_then(|d| d.asked().map(|(operation, copy)| (matches!(operation, controller::Operation::Update), copy)))
        };
        assert!(e.edited_since_upload());
        assert_eq!(copy_link(&e), Some((true, true)), "its window updates it, then copies the link");
        e.uploaded_edits = e.edits.clone().and_then(Result::ok);
        assert_eq!(copy_link(&e), None, "as shared: the link is copied at once");
    }

    /// DPG-29: under the splash, before the page is drawn once, its list is read (it was asked for when the splash ended,
    /// 1.9 s after the window opened).
    #[test]
    fn the_list_is_read_before_the_page_shows() {
        let temp = crate::link_smoke::Scratch::new("orbiters-gallery-prepare", false).unwrap();
        let root = &temp.path;
        std::fs::create_dir_all(root.join("Sessions")).unwrap();
        let history = root.join("visited-worlds.json");
        orbiters_core::visited::History::default().save(&history).unwrap();
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let mut snap = (*ctl.view()).clone();
        snap.settings.sessions_dir = root.join("Sessions");
        // (Nothing is asked of Orbiters here.)
        let mut state = State { sources: Some((history.clone(), None)), filter: Filter::Mine, asked: Some((Filter::Mine, String::new())), ..Default::default() };
        state.folders.set_sources(history, None);
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut time = 0.0;
        let mut frames = 0;
        while state.listed.is_none() {
            time += 1.0 / 60.0;
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut output = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                prepare(ui, &cx, &mut state);
            });
            output.textures_delta.clear();
            frames += 1;
            assert!(frames > 1 || state.local_job.running() || state.listed.is_some(), "asked at the first frame");
            assert!(std::time::Instant::now() < limit, "the list");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // The page's own first frame asks for nothing more.
        assert!(!state.local_job.running() && !state.local_dirty);
    }

    /// The page over `n` pictures of this computer, ten to an instance, at rest: what a frame allocates and takes (in
    /// microseconds), and how many times the list was laid out over those frames.
    fn frames_over(n: usize, layout: GalleryLayout, remote: bool) -> (u64, f64, u64) {
        let temp = crate::link_smoke::Scratch::new("orbiters-gallery-frames", false).unwrap();
        let root = &temp.path;
        std::fs::create_dir_all(root.join("Sessions")).unwrap();
        let history = root.join("visited-worlds.json");
        orbiters_core::visited::History::default().save(&history).unwrap();
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl =
            std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let mut snap = (*ctl.view()).clone();
        snap.settings.sessions_dir = root.join("Sessions");
        let mut state = State { sources: Some((history.clone(), None)), filter: Filter::Mine, asked: Some((Filter::Mine, String::new())), ..Default::default() };
        state.folders.set_sources(history, None);
        state.layout = layout;
        state.layout_saved = Some(snap.settings.photos_layout);
        state.locals = (0..n)
            .map(|i| {
                let mut e = controller::remote_entry(orbiters_core::shares::Item::default());
                e.key = format!("C:/Pictures/VRChat/picture-{i:06}.png");
                e.remote = remote.then(|| orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Picture, id: 1 + i as u64, ..Default::default() });
                e.local = (!remote).then(|| controller::Local::Picture(e.key.clone().into()));
                e.world_id = format!("wrld_{}", i / 10);
                e.world_name = format!("World {}", i / 10);
                e.instance = Some(format!("{}~friends(usr_x)", i / 10));
                e.taken = 1_790_000_000 - i as u64 * 60;
                e.size = Some([1600 + 100 * (i as u32 % 5), 900]);
                e
            })
            .collect();
        // The list is not read again while this one is looked at.
        let (hold, held) = std::sync::mpsc::channel::<()>();
        state.local_job = Job::spawn(&ctx, move || {
            let _ = held.recv();
            super::super::ForAccount::new(&Default::default(), (std::time::Instant::now(), 0, Vec::new()))
        });
        let (mut capture, mut projects) = (super::super::capture::State::default(), super::super::projects::State::default());
        let mut time = 0.0;
        let mut frame = |state: &mut State| {
            time += 1.0 / 60.0;
            let input = egui::RawInput { time: Some(time), screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 820.0))), ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut output = ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                show(ui, &mut cx, state, &mut capture, &mut projects);
            });
            output.textures_delta.clear();
        };
        for _ in 0..240 {
            frame(&mut state);
        }
        assert_eq!(state.shown.len(), n);
        let made = state.places_made;
        let from = std::time::Instant::now();
        let allocated = crate::test_alloc::count(|| (0..60).for_each(|_| frame(&mut state)));
        let took = from.elapsed().as_secs_f64() * 1e6 / 60.0;
        let made = state.places_made - made;
        // The same pictures drawn in both, so what a frame costs can be compared.
        assert!(state.cards.len() < 60, "{} cards drawn", state.cards.len());
        drop(hold);
        (allocated / 60, took, made)
    }

    /// DPG-3: a frame of the gallery costs what is in view, whatever the library. It laid every group out twice, made a
    /// map of every picture's key, and drew every group's heading, at every frame.
    #[test]
    fn a_frame_costs_what_is_in_view_whatever_the_library() {
        for layout in [GalleryLayout::Rows, GalleryLayout::Masonry, GalleryLayout::Grid] {
            let (small, small_us, _) = frames_over(200, layout, false);
            let (large, large_us, made) = frames_over(5000, layout, false);
            println!("{layout:?}: 200 pictures {small} allocations {small_us:.0} us a frame, 5000 pictures {large} allocations {large_us:.0} us a frame");
            assert_eq!(made, 0, "{layout:?}: nothing changed, nothing laid out again");
            assert!(large <= small + small / 5 + 40, "{layout:?}: {large} allocations a frame over 5000 pictures, {small} over 200");
        }
    }

    #[test]
    fn a_frame_of_remote_gallery_uses_published_settings() {
        let (allocations, micros, layouts) = frames_over(200, GalleryLayout::Grid, true);
        println!("Remote gallery: {allocations} allocations, {micros:.0} us per frame over 60 frames, {layouts} layouts");
        assert!(allocations < 5_000, "{allocations} allocations a frame for eight visible Orbiters-only cards");
        assert_eq!(layouts, 0);
    }

    /// What every group laid out one after the other gave, as egui stacked them (DPG-3: the list's places are made once
    /// and looked up), and what is looked up in them is what a walk over everything finds.
    #[test]
    fn the_places_of_the_list_are_those_egui_stacked() {
        for layout in [GalleryLayout::Rows, GalleryLayout::Masonry, GalleryLayout::Grid] {
            let groups = vec![0..3, 3..4, 4..40, 40..41, 41..90];
            let aspect = |i: usize| [1.78f32, 0.5625, 1.0, 1.333, 2.2][i % 5];
            let head = |first: usize| if first == 4 { 116.0 } else { 52.0 };
            let (width, spacing) = (1013.37f32, 3.0f32);
            let key = arrangement::Key { entries: 1, aspects: 1, layout, width, spacing };
            let places = arrangement::Places::make(key, 1, &groups, head, aspect, CARD_MIN, CARD_BODY);
            let ctx = egui::Context::default();
            let (mut heads, mut cells, mut bottom, mut base) = (Vec::new(), Vec::new(), 0.0, egui::Pos2::ZERO);
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 820.0))), ..Default::default() }, |ui| {
                (heads, cells) = (Vec::new(), Vec::new());
                ui.spacing_mut().item_spacing.y = spacing;
                ui.add_space(17.3);
                base = ui.cursor().min.round_ui();
                for g in &groups {
                    heads.push(ui.allocate_exact_size(vec2(width, head(g.start)), Sense::hover()).0);
                    ui.add_space(12.0);
                    let aspects: Vec<f32> = g.clone().map(aspect).collect();
                    let (rects, height) = arrangement::arrange(layout, &aspects, width, CARD_MIN, CARD_BODY);
                    let (area, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                    cells.extend(rects.into_iter().map(|r| r.translate(area.min.to_vec2())));
                    ui.add_space(32.0);
                }
                bottom = ui.cursor().top();
            });
            output.textures_delta.clear();
            let same = |a: Rect, b: Rect| a.min.distance(b.min) < 0.002 && a.max.distance(b.max) < 0.002;
            for (g, was) in places.groups.iter().zip(&heads) {
                let is = g.heading(width).translate(base.to_vec2());
                assert!(same(is, *was), "{layout:?}: {is:?}, {was:?}");
            }
            for (is, was) in places.cells.iter().zip(&cells) {
                let is = is.translate(base.to_vec2());
                assert!(same(is, *was), "{layout:?}: {is:?}, {was:?}");
            }
            assert!((base.y + places.height - bottom).abs() < 0.05, "{layout:?}");
            let keys: Vec<String> = (0..90).map(|i| format!("k{}", (i * 37) % 90)).collect();
            for step in 0..200 {
                let from = step as f32 * places.height / 190.0 - 100.0;
                let to = from + 700.0;
                let shows = |r: &Rect| r.bottom() >= from && r.top() <= to;
                let mut found = Vec::new();
                for g in places.groups_within(from, to) {
                    found.extend(places.cells_within(g, from, to).filter(|i| shows(&places.cells[*i])));
                }
                assert_eq!(found, (0..90).filter(|i| shows(&places.cells[*i])).collect::<Vec<_>>(), "{layout:?} from {from}");
                let headings = places.groups.iter().filter(|g| shows(&g.heading(width))).count();
                assert_eq!(places.groups_within(from, to).iter().filter(|g| shows(&g.heading(width))).count(), headings, "{layout:?} from {from}");
                let topmost = (0..90).filter(|i| places.cells[*i].top() >= from).min_by(|a, b| places.cells[*a].top().total_cmp(&places.cells[*b].top()).then_with(|| keys[*a].cmp(&keys[*b])));
                assert_eq!(places.topmost(from, |i| &keys[i]).map(|t| t.0), topmost, "{layout:?} from {from}");
            }
        }
    }

    #[test]
    fn progressive_layout_keeps_the_visible_picture_at_the_same_screen_position() {
        let (before, after) = (900.0, 1240.0);
        let offset = anchored_scroll_offset(850.0, Some(after - before)).unwrap();
        assert_eq!(after - offset, before - 850.0);
        assert!(anchored_scroll_offset(0.0, Some(after - before)).is_none());
        assert!(anchored_scroll_offset(850.0, Some(0.0)).is_none());
        assert!(anchored_scroll_offset(850.0, None).is_none(), "removed, or nothing laid out again");
    }

    #[test]
    fn scan_batches_do_not_retry_the_remote_feed_or_clear_its_error() {
        let mut state = State::default();
        let mut key = (0, 1, std::path::PathBuf::from("captures"), false, "signed-in".into(), 0);
        state.observe_context(key.clone());
        state.asked = Some((Filter::All, String::new()));
        state.error = Some("Too many attempts".into());
        for batch in 1..100 {
            state.local_dirty = true;
            key.0 = batch;
            key.5 = batch;
            state.observe_context(key.clone());
            assert!(state.asked.is_some(), "local discoveries must not cause a remote request");
            assert!(state.error.is_some(), "a scan must not retry a rate limited feed");
        }
        key.4 = "other-account".into();
        state.observe_context(key);
        assert!(state.asked.is_none(), "a different account needs its own feed");
    }

    #[test]
    fn join_requires_live_confirmation_or_the_current_instance() {
        assert!(!join_available(false, None));
        assert!(join_available(true, None));
        assert!(join_available(false, Some(true)));
        assert!(!join_available(true, Some(false)));
    }
    #[test]
    fn instance_groups_cross_midnight_but_never_worlds_or_unknown_days() {
        let entry = |world: &str, instance: Option<&str>, taken| {
            let mut e = controller::remote_entry(orbiters_core::shares::Item::default());
            e.world_id = world.into();
            e.instance = instance.map(str::to_owned);
            e.taken = taken;
            e
        };
        let entries = vec![
            entry("a", Some("one"), 172805),
            entry("a", Some("one"), 172799),
            entry("a", Some("two"), 172798),
            entry("b", Some("one"), 172797),
            entry("a", None, 172796),
            entry("a", None, 86399),
        ];
        assert_eq!(groups(&entries, |_| false), vec![0..2, 2..3, 3..4, 4..5, 5..6]);
        assert_eq!(groups(&entries, |e| e.world_id == "a" && e.instance.is_none()), vec![0..2, 2..3, 3..4, 4..6]);
    }
    #[test]
    fn instances_read_as_number_kind_and_region() {
        assert_eq!(instance_parts("12345~friends(usr_1)~region(eu)"), ("12345", "Friends", "eu"));
        assert_eq!(instance_parts("777~private(usr_1)~canRequestInvite~region(jp)"), ("777", "Invite+", "jp"));
        assert_eq!(instance_parts("42"), ("42", "Public", ""));
    }
}
