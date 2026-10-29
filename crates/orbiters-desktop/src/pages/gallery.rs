//! Photos & captures: local pictures, captures and the authenticated shared feed in one date-ordered gallery, grouped
//! by the instance they were taken in. Cards look like Visited worlds' and Capture files' and grow into the picture
//! window ([`sharing::Dialog`]) that shares them; the instance a group was taken in opens in a popover. Cards are
//! selected (Ctrl/Shift-click, their check, a rubber band) and dragged out of the window into other apps
//! ([`super::gallery_select`]).
use super::{
    sharing::{self, Dialog},
    Cx, Job,
};
use crate::controller::shares::{self as controller, Entry};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Id, Order, Rect, Sense, TextureHandle, Ui, UiBuilder};
use orbiters_core::{
    account::Server,
    shares::{Feed, Query},
    vrchat_log::WorldState,
};
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
}
impl Filter {
    fn value(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::All | Self::Communities => "all",
            Self::Shared => "shared",
        }
    }

    /// Whether the communities' pictures show.
    fn communities(self) -> bool {
        matches!(self, Self::All | Self::Communities)
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
    requests: super::requests::State,
    hash_job: Job<std::collections::HashMap<std::path::PathBuf, String>>,
    hash_asked: std::collections::HashSet<std::path::PathBuf>,
    shown_key: Option<(Filter, String)>,
    shown: Vec<Entry>,
    key: Option<(u64, u64, std::path::PathBuf, bool, String)>,
    local_job: Job<Vec<Entry>>,
    locals: Vec<Entry>,
    feed: Feed,
    feed_job: Job<Result<Feed, anyhow::Error>>,
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
    /// The cards chosen, a rubber band and a drag out of the window.
    select: super::gallery_select::Selection,
    /// The galleries of the member's communities and their windows.
    community: super::community::State,
}
impl State {
    pub fn busy(&self) -> bool {
        self.requests.busy()
            || self.hash_job.running()
            || self.local_job.running()
            || self.feed_job.running()
            || self.pictures.as_ref().is_some_and(|p| p.loading())
            || self.dialog.as_ref().is_some_and(Dialog::loading)
            || self.community.busy()
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
        self.locals.truncate(2);
        for (n, e) in self.locals.iter_mut().enumerate() {
            e.world_id = self.feed.items[0].world.id.clone();
            e.world_name = self.feed.items[0].world.name.clone();
            e.instance = self.feed.items[0].instance_id.clone();
            e.taken = controller::unix("2026-10-28T21:30:00Z") - n as u64 * 300;
        }
        self.shown_key = None;
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
            "gallery-explicit",
            "gallery-error",
            "gallery-picture",
            "gallery-progress",
            "gallery-request",
            "gallery-done",
            "gallery-copy-link",
            "gallery-copy-link-offer",
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
        self.dialog = Some(Dialog::new(ctx, e, false).from(origin, 20.0));
        self.from_card = true;
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

    fn fetch(&mut self, ctx: &egui::Context, server: Server, more: bool) {
        let q = Query {
            filter: self.filter.value().into(),
            q: self.query.trim().into(),
            cursor: if more { self.feed.next.clone() } else { None },
            limit: 24,
            ..Default::default()
        };
        self.error = None;
        self.feed_job = Job::spawn(ctx, move || controller::feed(server, q));
    }
}

/// `true` opens the existing capture-management surface (reconstruction, storage, import).
pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) -> bool {
    let ctx = ui.ctx().clone();
    state.requests.poll(&ctx, cx);
    let page = ui.max_rect();
    let now = anim::time(&ctx);
    let development = cx.snap.settings.development;
    let server = if development { Server::Development } else { Server::Production };
    let key = (cx.ctl.library_version(), cx.visit, cx.snap.settings.sessions_dir.clone(), development, format!("{:?}", cx.snap.account.state));
    if state.key.as_ref() != Some(&key) {
        let changed_account = state.key.as_ref().is_some_and(|k| k.3 != key.3 || k.4 != key.4);
        state.key = Some(key);
        let dir = cx.snap.settings.sessions_dir.clone();
        state.local_job = Job::spawn(&ctx, move || controller::local_entries(&dir, server));
        state.asked = None;
        if changed_account {
            state.feed_job = Job::default();
            state.hash_job = Job::default();
            state.feed = Feed::default();
            state.shown_key = None;
            for entry in &mut state.locals {
                entry.remote = None;
            }
            state.dialog = None;
            state.pictures = None;
            state.hash_asked.clear();
        }
    }
    if state.pictures.is_none() {
        state.pictures = Some(crate::worlds::Pictures::new(&ctx));
    }
    if let Some(l) = state.local_job.take() {
        state.locals = l;
        state.shown_key = None;
        // Screenshots: this computer's items, as the fixture has them, whenever they land.
        if state.fixture_locals {
            state.stage_locals();
        }
    }
    if let Some(r) = state.feed_job.take() {
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
    if !state.fixture && !state.hash_job.running() {
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
            state.hash_job = Job::spawn(&ctx, move || crate::worlds::hashes(paths));
        }
    }
    if state.community.poll(&ctx, cx, state.filter.communities()) {
        state.shown_key = None;
    }
    let requested = (state.filter, state.query.trim().to_string());
    if !state.fixture && state.filter != Filter::Communities && state.asked.as_ref() != Some(&requested) && now - state.query_changed > 0.3 {
        state.feed = Feed::default();
        state.shown_key = None;
        state.asked = Some(requested);
        state.fetch(&ctx, server, false);
    }
    if state.asked.as_ref().is_some_and(|(_, q)| q != state.query.trim()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(320));
    }
    let head = Rect::from_min_size(pos2(page.left() + super::PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - super::PAD_X * 2.0, 72.0));
    let narrow = head.width() < 690.0;
    // Communities: the galleries as chips under the filters.
    let chips_h = if state.filter == Filter::Communities { 46.0 } else { 0.0 };
    let controls = Rect::from_min_size(pos2(head.left(), head.bottom() + 14.0), vec2(head.width(), if narrow { 88.0 } else { 40.0 } + chips_h));
    let top = controls.bottom() + 20.0;
    let q = state.query.trim().to_lowercase();
    let shown_key = (state.filter, q.clone());
    let entries = if state.shown_key.as_ref() == Some(&shown_key) {
        std::mem::take(&mut state.shown)
    } else {
        state.shown_key = Some(shown_key);
        let mut entries: Vec<Entry> = state
            .locals
            .iter()
            .filter(|_| !matches!(state.filter, Filter::Shared | Filter::Communities))
            .take(if state.fixture { 1 } else { usize::MAX })
            .cloned()
            .collect();
        for item in &state.feed.items {
            if state.filter == Filter::Mine && !item.mine || state.filter == Filter::Shared && item.mine || state.filter == Filter::Communities {
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
                if item.picture.is_some() && !state.fixture {
                    e.picture = Some(crate::worlds::Picture::Capture { development, id: item.id, large: false });
                }
                if state.fixture {
                    e.picture = state.locals.iter().filter_map(|e| e.picture.clone()).nth(if item.mine { 0 } else { 1 });
                }
                entries.push(e);
            }
        }
        entries.retain(|e| e.world_name.to_lowercase().contains(&q));
        if state.filter.communities() {
            entries.extend(state.community.entries(&q));
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.taken));
        entries
    };

    // The card the window came out of is left out while the window draws it (riding under it as it leaves and lands)
    // and covers it, until the window landed back on it.
    let open_key = state.dialog.as_ref().filter(|d| state.from_card && d.hides_source()).map(|d| d.entry.key.clone());
    let window_key = state.dialog.as_ref().map(|d| d.entry.key.clone());
    let mut window_card: Option<sharing::Source> = None;
    let loading = if state.filter == Filter::Communities {
        state.community.loading() && state.community.count() == 0
    } else {
        state.local_job.running() || (state.feed_job.running() && state.feed.items.is_empty())
    };
    let from = orbiters_ui::blur::next_shape(ui);
    let mut gu = ui.new_child(UiBuilder::new().max_rect(page).id_salt("photos-captures-grid"));
    let mut clicked: Option<(Entry, Rect)> = None;
    let mut instance_clicked: Option<(Entry, Rect)> = None;
    let mut community_clicked: Option<(String, Rect)> = None;
    let modal = state.dialog.is_some() || state.instance.is_some() || state.community.is_open();
    state.select.begin();
    let mut background = None;
    let selecting = state.select.any();
    egui::ScrollArea::vertical()
        .id_salt("photos-captures")
        .auto_shrink([false, false])
        .scroll_bar_rect(Rect::from_min_max(pos2(page.left(), top), page.max))
        .show(&mut gu, |ui| {
            // The grid's empty space starts a rubber band (cards and buttons drawn later are on top of it).
            background = Some(ui.interact(ui.clip_rect(), Id::unique("photos-captures-band"), Sense::drag()));
            egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 40 }).show(ui, |ui| {
                ui.add_space(top - page.top());
                let width = ui.available_width();
                if entries.is_empty() {
                    if loading {
                        let (cols, w) = orbiters_ui::layout::columns(width, CARD_MIN, 16.0);
                        let h = w * 9.0 / 16.0 + CARD_BODY;
                        let (area, _) = ui.allocate_exact_size(vec2(width, 56.0 + h), Sense::hover());
                        feedback::shimmer(ui.painter(), Rect::from_min_size(area.min + vec2(0.0, 6.0), vec2(220.0, 18.0)), 9.0);
                        feedback::shimmer(ui.painter(), Rect::from_min_size(area.min + vec2(0.0, 32.0), vec2(160.0, 12.0)), 6.0);
                        for c in 0..cols {
                            super::sessions::shimmer_card(ui, Rect::from_min_size(area.min + vec2(c as f32 * (w + 16.0), 56.0), vec2(w, h)));
                        }
                    } else {
                        empty_state(ui, cx, state.filter, !q.is_empty(), (page.bottom() - top) * 0.6);
                    }
                }
                let mut index = 0usize;
                for indices in groups(&entries) {
                    let first = &entries[indices[0]];
                    let last = &entries[*indices.last().unwrap()];
                    let head_h = state.community.header_height(first).unwrap_or(52.0);
                    let (r, _) = ui.allocate_exact_size(vec2(width, head_h), Sense::hover());
                    let (r, a) = super::entering(ui, cx, 1 + index.min(8), r);
                    let mut hu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("group-head", &first.key)));
                    hu.multiply_opacity(a);
                    if super::community::owns(&first.key) {
                        let pictures = state.pictures.as_mut().expect("pictures");
                        state.community.group_header(&mut hu, cx, pictures, first, last, indices.len(), !modal);
                    } else {
                        let (button, pressed) = group_header(&mut hu, first, last, indices.len());
                        state.instance_buttons.insert(first.key.clone(), button);
                        if pressed && !modal {
                            instance_clicked = Some((first.clone(), button));
                        }
                    }
                    ui.add_space(12.0);
                    let (cols, w) = orbiters_ui::layout::columns(width, CARD_MIN, 16.0);
                    let h = w * 9.0 / 16.0 + CARD_BODY;
                    let rows = indices.len().div_ceil(cols);
                    let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * (h + 16.0) - 16.0), Sense::hover());
                    for (n, i) in indices.iter().enumerate() {
                        let e = &entries[*i];
                        let r =
                            Rect::from_min_size(pos2(area.left() + (n % cols) as f32 * (w + 16.0), area.top() + (n / cols) as f32 * (h + 16.0)), vec2(w, h));
                        index += 1;
                        if !ui.is_rect_visible(r) {
                            continue;
                        }
                        if super::community::owns(&e.key) {
                            let pictures = state.pictures.as_mut().expect("pictures");
                            if let Some(thumb) = state.community.card(ui, cx, pictures, index, e, r, !modal) {
                                community_clicked = Some((e.key.clone(), thumb));
                            }
                            continue;
                        }
                        let tex = e.picture.as_ref().and_then(|p| state.pictures.as_mut()?.get(&ctx, p));
                        let from_tex = e.remote.as_ref().and_then(|i| i.from.as_ref()).and_then(|from| {
                            if state.fixture {
                                return None;
                            }
                            let url = from.avatar_url.clone()?;
                            state.pictures.as_mut()?.get(&ctx, &crate::worlds::Picture::Remote { world_id: format!("person-{}", from.id), url, large: false })
                        });
                        let shown = if open_key.as_ref() == Some(&e.key) { 0.0 } else { 1.0 };
                        let sharing = cx.snap.capture_shares.iter().any(|j| j.key == e.key && j.outcome.is_none());
                        let look = CardLook {
                            tex: tex.as_ref(),
                            from: from_tex.as_ref(),
                            shown,
                            interactive: !modal,
                            sharing,
                            riding: false,
                            selected: state.select.has(&e.key),
                            selecting,
                            carried: state.select.carried(&e.key),
                        };
                        let (thumb, resp) = card(ui, cx, index, e, r, &look);
                        if let Some(resp) = resp.filter(|_| !modal) {
                            if let super::gallery_select::CardAct::Open = state.select.card(&resp, e, super::gallery_select::check_rect(thumb), &entries) {
                                clicked = Some((e.clone(), thumb));
                            }
                        }
                        state.select.place(&e.key, r);
                        let picture = Rect::from_min_size(r.min, vec2(w, w * 9.0 / 16.0));
                        state.cards.insert(e.key.clone(), picture);
                        if window_key.as_ref() == Some(&e.key) {
                            let (e, tex, from_tex) = (e.clone(), tex.clone(), from_tex.clone());
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
                                        riding: true,
                                        selected: false,
                                        selecting: false,
                                        carried: false,
                                    };
                                    let _ = card(ui, cx, 0, &e, r, &look);
                                }),
                            });
                        }
                    }
                    ui.add_space(32.0);
                }
                let more_feed = state.filter != Filter::Communities && state.feed.next.is_some();
                let more_community = state.filter.communities() && state.community.has_more();
                if more_feed || more_community {
                    let running = state.feed_job.running() || state.community.loading();
                    let b = Button::new(if running { "Loading…" } else { "Show more" }).variant(Variant::Ghost).enabled(!running);
                    let bw = b.desired_width(ui);
                    let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                    if sharing::pressed(b.show_at(ui, Rect::from_center_size(r.center(), vec2(bw, 40.0)))) {
                        if more_feed {
                            state.fetch(&ctx, server, true);
                        }
                        if more_community {
                            state.community.more(&ctx, cx);
                        }
                    }
                }
                if let Some(error) = state.community.error().filter(|_| state.filter == Filter::Communities).map(str::to_owned) {
                    ui.add_space(12.0);
                    feedback::banner(
                        ui,
                        Id::unique("gallery-community-error"),
                        Tone::Danger,
                        &format!("Your communities' galleries could not be loaded: {error}"),
                        false,
                    );
                    ui.add_space(8.0);
                    let b = Button::new("Try again").icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!state.community.loading());
                    let bw = b.desired_width(ui);
                    let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                    if sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                        state.community.retry();
                    }
                }
                if let Some(error) = state.error.clone() {
                    ui.add_space(12.0);
                    feedback::banner(ui, Id::unique("gallery-feed-error"), Tone::Danger, &format!("Shared moments could not be loaded: {error}"), false);
                    ui.add_space(8.0);
                    let b = Button::new("Try again").icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!state.feed_job.running());
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
    super::under_top(&gu, cx, "photos-captures", from, top);
    if let Some(background) = &background {
        state.select.update(ui, &entries, background, modal);
    }

    // Heading and the bar over the grid (drawn after it: what scrolls up goes under them).
    let (head_r, head_a) = super::entering(ui, cx, 0, head);
    {
        let mut hu = ui.new_child(UiBuilder::new().max_rect(head_r).id_salt("gallery-head"));
        hu.multiply_opacity(head_a);
        let mine = state.locals.len();
        let shared = state.feed.items.iter().filter(|i| !i.mine).count();
        let communities = state.community.count();
        let from_communities = if communities > 0 { format!(" · {communities} from your communities") } else { String::new() };
        let sub = if loading {
            "Gathering your pictures and captures…".to_string()
        } else if narrow {
            format!("{mine} on this computer · {shared} shared with you{from_communities}")
        } else {
            format!("{mine} on this computer · {shared} shared with you{from_communities} · grouped by where they were taken")
        };
        super::heading(&hu, head_r, "Library", "Photos & captures", Some(&sub));
    }
    let mut manage = false;
    let mb = Button::new("Manage captures").icon(icons::FOLDER_OPEN).hint("Reconstruct, free up space and import raw captures");
    let mw = mb.desired_width(ui);
    let b = Rect::from_min_size(pos2(head.right() - mw, head.bottom() - 40.0), vec2(mw, 40.0));
    if sharing::pressed(mb.show_at(ui, b)) {
        manage = true;
    }
    let mut cu = ui.new_child(UiBuilder::new().max_rect(controls).id_salt("gallery-controls"));
    let before_filter = state.filter;
    widgets::segmented(
        &mut cu,
        "gallery-filter",
        &mut state.filter,
        &[
            SegOption::new(Filter::All, "All"),
            SegOption::new(Filter::Mine, "Mine"),
            SegOption::new(Filter::Shared, "Shared with me"),
            SegOption::new(Filter::Communities, "Communities").hint("Pictures of your communities' galleries and their events"),
        ],
        false,
    );
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
    let search = if narrow {
        Rect::from_min_size(pos2(controls.left(), controls.top() + 48.0), vec2(controls.width(), 40.0))
    } else {
        Rect::from_min_size(pos2(controls.right() - 260.0, controls.top()), vec2(260.0, 40.0))
    };
    let pending = state.requests.pending();
    let rb = Button::new("Requests").icon(icons::TRAY).hint("People asking to share what you shared with them, and what you asked");
    let rw = rb.desired_width(ui);
    let request_button = Rect::from_min_size(pos2(if narrow { controls.right() - rw } else { search.left() - 12.0 - rw }, controls.top()), vec2(rw, 40.0));
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
    let hint = if state.filter == Filter::Communities { "Search galleries and people" } else { "Search worlds" };
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
    if let Some((e, thumb)) = clicked {
        let mut e = e;
        if let Some(item) = e.remote.as_ref() {
            if item.picture.is_some() && !state.fixture && e.picture.is_none() {
                e.picture = Some(crate::worlds::Picture::Capture { development, id: item.id, large: false });
            }
        }
        state.dialog = Some(Dialog::new(&ctx, e, false).from(thumb, 20.0));
        state.from_card = true;
    }
    if let Some(d) = state.dialog.as_mut() {
        // The items before and after it in the gallery: arrows and ←/→ go through them.
        let at = entries.iter().position(|e| e.key == d.entry.key);
        d.neighbours = (at.is_some_and(|i| i > 0), at.is_some_and(|i| i + 1 < entries.len()));
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
        if let (Some(step), Some(i)) = (step, at) {
            if let Some(next) = usize::try_from(i as i64 + step).ok().and_then(|j| entries.get(j)) {
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
        let close = d.show(ui, cx);
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
    if let Some((key, thumb)) = community_clicked {
        let keys: Vec<String> = entries.iter().filter(|e| super::community::owns(&e.key)).map(|e| e.key.clone()).collect();
        state.community.open(&ctx, &key, thumb, &keys);
    }
    {
        let pictures = state.pictures.as_mut().expect("pictures");
        state.community.show(ui, cx, pictures);
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
        if let Some(first) = groups(&entries).first().map(|g| entries[g[0]].clone()) {
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
    let modal = state.dialog.is_some() || state.instance.is_some() || state.community.is_open();
    state.select.bar(ui, page, &entries, &mut state.pictures, modal);
    state.shown = entries;
    manage
}

/// What a card's body takes under its picture.
const CARD_BODY: f32 = 70.0;
/// The narrowest card, as Visited worlds' (the same card): both pages get as many columns at every width.
const CARD_MIN: f32 = 270.0;

/// A group's heading: the world, when, what kind of instance and how many; the Instance button on the right.
/// Returns that button's rect and whether it was pressed.
fn group_header(ui: &mut Ui, first: &Entry, last: &Entry, count: usize) -> (Rect, bool) {
    let r = ui.max_rect();
    let p = ui.painter().clone();
    let b = Button::new("Instance").icon(icons::INFO).variant(Variant::Secondary).size(Size::Sm).id_salt(("instance", &first.key));
    let bw = b.desired_width(ui);
    let br = Rect::from_min_size(pos2(r.right() - bw, r.top() + 6.0), vec2(bw, 32.0));
    widgets::text_truncated(
        &p,
        pos2(r.left(), r.top() + 12.0),
        Align2::LEFT_CENTER,
        &Txt::new(&first.world_name, 18.0).weight(weight::SEMIBOLD).tracking(-0.01),
        r.width() - bw - 24.0,
    );
    let date = super::local_time(&controller::iso(first.taken));
    let span = if first.taken == last.taken {
        date
    } else {
        // Same day: "28 Oct 2026, 21:30 – 22:30".
        let start = super::local_time(&controller::iso(last.taken));
        match (start.rsplit_once(", "), date.rsplit_once(", ")) {
            (Some((d1, t1)), Some((d2, t2))) if d1 == d2 => format!("{d1}, {t1} – {t2}"),
            _ => format!("{start} – {date}"),
        }
    };
    let items = format!("{count} item{}", if count == 1 { "" } else { "s" });
    let mut x = r.left();
    let y = r.top() + 36.0;
    if let Some(kind) = first.instance.as_deref().map(|i| instance_parts(i).1) {
        let t = Txt::new(kind, 11.0).weight(weight::SEMIBOLD).color(theme::FG_2);
        let tw = widgets::text_size(ui, &t).x;
        let pill = Rect::from_min_size(pos2(x, y - 10.0), vec2(tw + 30.0, 20.0));
        p.rect_filled(pill, theme::radius(10.0), theme::white(0.06));
        widgets::icon(&p, pos2(pill.left() + 11.0, pill.center().y), instance_glyph(kind), 11.0, theme::FG_3);
        widgets::text(&p, pos2(pill.left() + 21.0, pill.center().y), Align2::LEFT_CENTER, &t);
        x = pill.right() + 10.0;
    }
    widgets::text_truncated(&p, pos2(x, y), Align2::LEFT_CENTER, &Txt::new(format!("{span} · {items}"), 12.0).color(theme::FG_3), br.left() - x - 16.0);
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
    /// Drawn by its window, riding under it: takes no input, its hover and press let go.
    riding: bool,
    /// Chosen in the selection; any card is (its check shows on every card).
    selected: bool,
    selecting: bool,
    /// Being dragged out of the window: lifted (smaller, faded) until the drop.
    carried: bool,
}

/// One card, as Visited worlds' and Capture files': the picture with what it is and who sees it, then its kind,
/// time and who it is from. Returns its picture's rect and, when it takes input, its response (a click opens or
/// selects it, a drag takes it out of the window: [`super::gallery_select`]).
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

    widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover));
    let thumb = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0));
    super::sessions::thumb_shape(&p, thumb, CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 }, look.tex, 1.0 + 0.06 * ease::out_cubic(hover));
    let shade = paint::rounded_rect_points_corners(thumb, [20.0, 20.0, 0.0, 0.0]);
    paint::fill_gradient(
        &p,
        &shade,
        &Gradient::vertical(
            thumb,
            &[(0.0, theme::alpha(theme::INK_950, 0.25)), (0.3, Color32::TRANSPARENT), (0.6, Color32::TRANSPARENT), (1.0, theme::alpha(theme::INK_950, 0.75))],
        ),
    );

    // What it is (top left), who sees it (top right).
    let job = match &e.local {
        Some(controller::Local::Session(d)) => cx.snap.reconstruction.as_ref().filter(|j| &j.session_dir == d),
        _ => None,
    };
    let queued = matches!(&e.local, Some(controller::Local::Session(d)) if cx.snap.reconstruct_queue.contains(d));
    let (kind_label, kind_color) = if e.is_picture() { ("Picture", theme::FG_2) } else { ("3D", theme::ACCENT) };
    // The check on its corner; the badges step aside for it.
    let check = if look.riding { 0.0 } else { super::gallery_select::paint_card(&p, &ctx, &e.key, r, thumb, look.selected, look.selecting, hover) };
    let mut x = thumb.left() + 12.0 + (super::gallery_select::CHECK + 6.0) * check;
    x += super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), kind_label, theme::black(0.55), kind_color, false) + 6.0;
    if let Some(j) = job {
        super::sessions::badge(
            &p,
            ui,
            pos2(x, thumb.top() + 12.0),
            &format!("Reconstructing {:.0}%", super::sessions::overall_progress(j) * 100.0),
            theme::black(0.55),
            theme::ACCENT,
            false,
        );
    } else if queued {
        super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), "Waiting to reconstruct", theme::black(0.55), theme::FG_2, false);
    } else if look.sharing {
        super::sessions::badge(&p, ui, pos2(x, thumb.top() + 12.0), "Sharing…", theme::black(0.55), theme::ACCENT, false);
    }
    if let Some(item) = e.remote.as_ref().filter(|i| i.mine) {
        if item.public {
            super::sessions::badge(&p, ui, pos2(thumb.right() - 12.0, thumb.top() + 12.0), "Public", theme::alpha(theme::OK, 0.22), theme::OK, true);
        } else if !item.grants.is_empty() {
            super::sessions::badge(
                &p,
                ui,
                pos2(thumb.right() - 12.0, thumb.top() + 12.0),
                &format!("Shared · {}", item.grants.len()),
                theme::black(0.55),
                theme::FG_2,
                true,
            );
        }
    }
    if let Some(j) = job {
        let bar = Rect::from_min_max(pos2(thumb.left(), thumb.bottom() - 3.0), thumb.max);
        let f = anim::spring_from(&ctx, id.with("reconstruct"), [0.0], [super::sessions::overall_progress(j)], Spring::SOFT)[0].clamp(0.0, 1.0);
        p.rect_filled(bar, 0.0, theme::white(0.08));
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, 3.0)), 0.0, theme::ACCENT);
    }

    // Kind and time, then who sees it or who it is from.
    let from = e.remote.as_ref().filter(|i| !i.mine).and_then(|i| i.from.as_ref());
    let bx = r.left() + 16.0;
    let title = if e.is_picture() {
        "VRChat picture"
    } else if e.ready {
        "3D capture"
    } else {
        "Capture"
    };
    let time = super::local_time(&controller::iso(e.taken));
    let time = time.rsplit_once(", ").map_or(time.clone(), |(_, t)| t.to_string());
    let tt = Txt::new(&time, 12.0).color(theme::FG_3);
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
            &Txt::new(format!("from {}", from.name), 12.0).weight(weight::MEDIUM).color(theme::FG_2),
            r.right() - ar.right() - 23.0,
        );
    } else {
        let (glyph, label, color) = match &e.remote {
            Some(i) if i.public => (icons::GLOBE, "Public on Orbiters".to_string(), theme::OK),
            Some(i) if !i.grants.is_empty() => (icons::USERS, format!("Shared with {}", i.grants.len()), theme::FG_2),
            Some(_) => (icons::LOCK_SIMPLE, "Only you".to_string(), theme::FG_3),
            None if !e.ready && !e.is_picture() => (icons::MAGIC_WAND, "Reconstruct it to share it".to_string(), theme::FG_3),
            None => (icons::HARD_DRIVE, "On this computer".to_string(), theme::FG_3),
        };
        widgets::icon(&p, pos2(bx + 7.0, line_y), glyph, 13.0, color);
        widgets::text_truncated(&p, pos2(bx + 20.0, line_y), Align2::LEFT_CENTER, &Txt::new(label, 12.0).color(color), r.width() - 52.0);
    }
    (thumb, resp.filter(|_| look.interactive))
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
        ("No world matches", "Try another world name.")
    } else if filter == Filter::Communities {
        (
            "Your communities' pictures belong here",
            "Pictures posted in the galleries of the Discord servers you're in show up here, with the events they were taken at.",
        )
    } else if filter == Filter::Shared {
        ("Nothing shared with you yet", "When friends share a picture or a 3D capture with you, it shows up here.")
    } else {
        ("Your moments belong here", "VRChat pictures and N4N0's 3D captures show up here together, grouped by the instance you took them in.")
    };
    widgets::text(&p, rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 13.0).color(theme::FG_3).line_height(20.0).job(400.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y + 34.0), g, theme::FG_3);
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
                    _ => Err("Unexpected instance response.".into()),
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
    let area = egui::Area::new(id).order(Order::Foreground).fixed_pos(rect.min).fade_in(false).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        ui.multiply_opacity(kk);
        let p = ui.painter().clone();
        widgets::panel(&p, rect, 18.0);
        let mut y = rect.top() + 20.0;
        let x = rect.left() + 20.0;
        let (number, kind, region) = e.instance.as_deref().map(instance_parts).unwrap_or(("", "Instance not recorded", ""));
        let tile = Rect::from_min_size(pos2(x, y + 2.0), vec2(40.0, 40.0));
        p.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.12));
        widgets::icon(&p, tile.center(), instance_glyph(kind), 19.0, theme::ACCENT);
        widgets::text_truncated(
            &p,
            pos2(tile.right() + 14.0, y + 12.0),
            Align2::LEFT_CENTER,
            &Txt::new(&e.world_name, 15.5).weight(weight::SEMIBOLD),
            inner - 54.0,
        );
        let mut meta = vec![kind.to_string()];
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
            &Txt::new(meta.join(" · "), 12.0).color(theme::FG_3),
            inner - 54.0,
        );
        y += 44.0 + 16.0;
        widgets::text(&p, pos2(x, y + 7.0), Align2::LEFT_CENTER, &Txt::caps(if current { "Here now" } else { "Who was there" }));
        y += 14.0 + 10.0;
        if people.is_empty() {
            widgets::text(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, &Txt::new("Not recorded: VRCX keeps who was there", 12.5).color(theme::FG_3));
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
            (Tone::Ok, "You are in this instance now")
        } else if state.instance_alive == Some(true) {
            (Tone::Ok, "Still open: you can join it again")
        } else if state.instance_alive == Some(false) {
            (Tone::Off, "This instance has closed")
        } else if state.instance_job.running() {
            (Tone::Info, "Checking whether it is still open…")
        } else if e.instance.is_none() {
            (Tone::Off, "The instance was not recorded for this moment")
        } else {
            (Tone::Off, "Joining works for instances still open")
        };
        if state.instance_job.running() {
            feedback::spinner(&p, pos2(x + 5.0, y + 11.0), 5.0, 1.8, theme::CYAN);
        } else {
            widgets::status_dot(&p, pos2(x + 5.0, y + 11.0), dot, dot == Tone::Ok);
        }
        widgets::text_truncated(&p, pos2(x + 18.0, y + 11.0), Align2::LEFT_CENTER, &Txt::new(status, 12.5).color(theme::FG_2), inner - 18.0);
        y += 22.0 + 16.0;
        if let Some(err) = &state.instance_error {
            widgets::text_truncated(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, &Txt::new(err, 12.0).color(theme::DANGER), inner - 110.0);
            let b = Button::new("Try again").size(Size::Sm).id_salt("instance-retry");
            let bw = b.desired_width(ui);
            if sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(rect.right() - 20.0 - bw, y - 6.0), vec2(bw, 32.0)))) {
                retry = true;
            }
            y += 40.0;
        }
        let br = Rect::from_min_size(pos2(x, y), vec2(inner, 40.0));
        // Green only while it can be joined: a closed or unknown instance keeps a quiet, disabled button.
        let open = Button::new("Join in VRChat")
            .icon(icons::ARROW_SQUARE_OUT)
            .variant(if join { Variant::Primary } else { Variant::Secondary })
            .enabled(join)
            .width(inner)
            .id_salt("instance-join");
        let open = if join { open } else { open.hint("Only an instance that is still open can be joined") };
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
    Txt::new(name, 12.5).weight(weight::MEDIUM).color(theme::FG_2)
}

fn groups(entries: &[Entry]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let same = groups.last().and_then(|g| g.first()).is_some_and(|j| {
            let a = &entries[*j];
            a.world_id == e.world_id && a.instance == e.instance && (a.instance.is_some() || a.taken / 86400 == e.taken / 86400)
        });
        if same {
            groups.last_mut().unwrap().push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    groups
}
fn is_current(world: &WorldState, e: &Entry) -> bool {
    world.in_world && world.world_id.as_ref() == Some(&e.world_id) && e.instance.is_some() && world.instance == e.instance
}
fn join_available(current: bool, alive: Option<bool>) -> bool {
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
            "Invite+"
        } else {
            "Invite only"
        }
    } else if instance.contains("~friends(") {
        "Friends"
    } else if instance.contains("~hidden(") {
        "Friends+"
    } else if instance.contains("~group(") {
        "Group"
    } else {
        "Public"
    }
}

fn instance_glyph(kind: &str) -> &'static str {
    match kind {
        "Friends" | "Friends+" => icons::USERS,
        "Invite only" | "Invite+" => icons::ENVELOPE_SIMPLE,
        "Group" => icons::USERS,
        "Public" => icons::GLOBE,
        _ => icons::INFO,
    }
}

fn encode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(groups(&entries), vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5]]);
    }
    #[test]
    fn instances_read_as_number_kind_and_region() {
        assert_eq!(instance_parts("12345~friends(usr_1)~region(eu)"), ("12345", "Friends", "eu"));
        assert_eq!(instance_parts("777~private(usr_1)~canRequestInvite~region(jp)"), ("777", "Invite+", "jp"));
        assert_eq!(instance_parts("42"), ("42", "Public", ""));
    }
}
