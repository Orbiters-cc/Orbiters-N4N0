//! Photos & captures' community galleries: the galleries of the communities (Discord servers) the member is in, their
//! own and the public ones, as the website's gallery page lists them (`orbiters_core::community`). Their pictures join
//! the member's own in the gallery, newest first, grouped by gallery and day; Communities shows only them, with a chip
//! for each gallery. An event's gallery heads its pictures with the event's card (the website's GalleryEventCard),
//! which opens the event's window ([`super::events::Window`]); a picture opens in the picture window ([`PictureWindow`],
//! the capture window's look and motion: grown out of its card, ←/→ through the gallery, the event it was taken at).

use super::events::{self, CoverSize, Covers, Phase};
use super::{sharing, Cx, Job};
use crate::controller::shares::{self as controller, Entry};
use crate::worlds::{Picture, Pictures};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Order, Rect, Sense, TextureHandle, Ui, UiBuilder};
use orbiters_core::account::{Api, Server};
use orbiters_core::community::{self, Card, Galleries, Gallery};
use orbiters_i18n::{mark, tr, tr_n};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::viewer::{self, Arrangement, Morph};
use orbiters_ui::widgets::{self, Chip, IconButton, Tone};
use orbiters_ui::{feedback, icons, morph};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Entries of community pictures are keyed `community:<placement id>`, their group `gallery:<gallery id>`.
const KEY: &str = "community:";
const GROUP: &str = "gallery:";

pub fn owns(key: &str) -> bool {
    key.starts_with(KEY)
}

fn picture_id(key: &str) -> Option<u64> {
    key.strip_prefix(KEY)?.parse().ok()
}

fn gallery_of(e: &Entry) -> Option<u64> {
    e.world_id.strip_prefix(GROUP)?.parse().ok()
}

fn visibility(g: Option<&Gallery>) -> (&'static str, &'static str, Color32) {
    match g.map(|g| g.visibility.as_str()) {
        Some("public") => (icons::GLOBE, tr("Public"), theme::OK),
        Some("private") => (icons::LOCK_SIMPLE, tr("Yours"), theme::FG_3),
        _ => (icons::USERS_THREE, tr("Members"), theme::CYAN),
    }
}

/// The picture to show for a gallery picture: Orbiters' preview (or full picture), or a stand-in for screenshots. Most
/// pictures list without a link (Discord's expired): the loader asks Orbiters for a renewed one.
pub(super) fn source(p: &community::Picture, large: bool, development: bool, stand_ins: &[Picture]) -> Option<Picture> {
    if !stand_ins.is_empty() {
        let _ = large;
        return Some(stand_ins[(p.id as usize) % stand_ins.len()].clone());
    }
    let url = if large { p.full_url.as_ref().or(p.src.as_ref()) } else { p.src.as_ref() };
    let url = url.filter(|u| u.starts_with("https://")).cloned().unwrap_or_default();
    // An asset's picture (id 0) loads from its link alone.
    (p.id != 0 || !url.is_empty()).then_some(Picture::Gallery { development, asset: p.asset_id, id: p.id, url, large })
}

fn avatar_of(m: Option<&community::Member>) -> Option<Picture> {
    m?.avatar_url().map(|url| Picture::Gallery { development: false, asset: 0, id: 0, url, large: false })
}

#[derive(Default)]
pub struct State {
    galleries: Option<Galleries>,
    galleries_job: Job<anyhow::Result<Galleries>>,
    feed: community::Pictures,
    feed_job: Job<anyhow::Result<community::Pictures>>,
    /// The gallery chosen among the chips (none: all of them).
    pub chosen: Option<u64>,
    /// What was asked: the environment, the account, the gallery.
    asked: Option<(bool, String, Option<u64>)>,
    error: Option<String>,
    window: Option<PictureWindow>,
    event: Option<events::Window>,
    covers: Covers,
    /// Each event header's cover as last drawn (by gallery).
    headers: HashMap<u64, Rect>,
    /// Each card's picture as last drawn (by entry key).
    cards: HashMap<String, Rect>,
    changed: bool,
    stand_ins: Vec<Picture>,
    fixture: bool,
    /// Screenshots: what the pictures' windows say (their replies and reactions) and the reaction hovered.
    talk: Option<super::community_talk::Said>,
    talk_hover: Option<usize>,
}

impl State {
    pub fn busy(&self) -> bool {
        self.galleries_job.running() || self.feed_job.running() || self.event.as_ref().is_some_and(events::Window::loading)
    }

    /// A window of its own is open (a picture, an event).
    pub fn is_open(&self) -> bool {
        self.window.is_some() || self.event.is_some()
    }

    pub fn has_more(&self) -> bool {
        self.feed.next().is_some()
    }

    pub fn loading(&self) -> bool {
        self.feed_job.running()
    }

    pub fn count(&self) -> usize {
        self.feed.items.len()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Screenshots: the fixture's galleries and pictures, this computer's VRChat pictures as their pictures.
    pub fn stage(&mut self, name: &str, sessions_dir: &std::path::Path) {
        self.fixture = true;
        self.galleries_job = Job::default();
        self.feed_job = Job::default();
        let mut galleries: Galleries = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/galleries.json")).unwrap();
        let mut feed: community::Pictures = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/gallery-pictures.json")).unwrap();
        // The event's gallery is the one under way on the Events page.
        let mut cards: community::Cards = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/events.json")).unwrap();
        events::retime(&mut cards.items);
        if let Some(first) = cards.items.first() {
            for g in galleries.member_galleries.iter_mut().filter(|g| g.event.is_some()) {
                g.event = Some(Card { status: Some("published".into()), ..first.clone() });
            }
            for p in feed.items.iter_mut().filter(|p| p.event.is_some()) {
                p.event = Some(Card { status: Some("published".into()), ..first.clone() });
            }
        }
        // Posted over the last hours, the event's last two after another gallery's (posted later, as a second batch):
        // the event's card still heads them all once.
        let now = events::now();
        let places = [0i64, 1, 2, 5, 6, 3, 4, 7, 8, 9];
        for (i, p) in feed.items.iter_mut().enumerate() {
            let place = places.get(i).copied().unwrap_or(i as i64);
            p.posted_at = time::OffsetDateTime::from_unix_timestamp(now - 1500 - place * 1300)
                .ok()
                .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok());
        }
        self.stand_ins = Covers::stand_ins(sessions_dir);
        self.covers = Covers { development: false, fixture: self.stand_ins.clone(), order: cards.items.iter().map(|c| c.id.clone()).collect() };
        if name == "gallery-community-one" {
            self.chosen = Some(31);
        }
        self.galleries = Some(galleries);
        self.feed = feed;
        self.changed = true;
    }

    /// Reads the galleries and their newest pictures when Photos & captures shows them (`wanted`), again when the
    /// account, the environment or the chosen gallery change. True when what it shows changed.
    pub fn poll(&mut self, ctx: &egui::Context, cx: &Cx, wanted: bool) -> bool {
        let development = cx.snap.settings.development;
        let server = if development { Server::Development } else { Server::Production };
        self.covers.development = development;
        if !self.fixture && wanted {
            let account = format!("{:?}", cx.snap.account.state);
            let key = (development, account, self.chosen);
            if self.asked.as_ref() != Some(&key) {
                let account_changed = self.asked.as_ref().is_none_or(|k| k.0 != key.0 || k.1 != key.1);
                self.asked = Some(key);
                self.error = None;
                self.feed = community::Pictures::default();
                self.changed = true;
                if account_changed {
                    self.galleries = None;
                    self.galleries_job = Job::spawn(ctx, move || {
                        let account = orbiters_core::account::read(server).ok_or_else(|| anyhow::anyhow!("Sign in to Orbiters in Settings first."))?;
                        Api::new(server).galleries(&account.token)
                    });
                }
                self.fetch(ctx, server, false);
            }
        }
        if let Some(r) = self.galleries_job.take() {
            match r {
                Ok(g) => self.galleries = Some(g),
                Err(e) => self.error = Some(e.to_string()),
            }
            self.changed = true;
        }
        if let Some(r) = self.feed_job.take() {
            match r {
                Ok(page) => self.feed.append(page),
                Err(e) => self.error = Some(e.to_string()),
            }
            self.changed = true;
        }
        std::mem::take(&mut self.changed)
    }

    fn fetch(&mut self, ctx: &egui::Context, server: Server, more: bool) {
        let (gallery, cursor) = (self.chosen, if more { self.feed.next() } else { None });
        self.feed_job = Job::spawn(ctx, move || {
            let Some(account) = orbiters_core::account::read(server) else { return Ok(community::Pictures::default()) };
            Api::new(server).gallery_pictures(&account.token, gallery, cursor.as_deref(), 24)
        });
    }

    /// The next page of pictures (Show more).
    pub fn more(&mut self, ctx: &egui::Context, cx: &Cx) {
        if self.fixture || self.feed_job.running() {
            return;
        }
        let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
        self.fetch(ctx, server, true);
    }

    pub fn retry(&mut self) {
        self.asked = None;
    }

    fn gallery(&self, id: u64) -> Option<&Gallery> {
        self.galleries.as_ref()?.get(id)
    }

    /// The pictures as the gallery's entries, those `q` (lowercase) finds: their gallery, event, author or words.
    pub fn entries(&self, q: &str) -> Vec<Entry> {
        let mut out = Vec::new();
        for p in &self.feed.items {
            if self.chosen.is_some_and(|c| c != p.gallery_id) {
                continue;
            }
            let g = self.gallery(p.gallery_id);
            let name = g.map(|g| g.name.clone()).unwrap_or_else(|| tr("Community gallery").into());
            let author = p.author.as_ref().map(|a| a.username.clone()).unwrap_or_default();
            if !q.is_empty() {
                let event = p.event.as_ref().map(|e| e.title.to_lowercase()).unwrap_or_default();
                let words = p.message.as_ref().map(|m| m.content.to_lowercase()).unwrap_or_default();
                if ![name.to_lowercase(), author.to_lowercase(), event, words].iter().any(|s| s.contains(q)) {
                    continue;
                }
            }
            out.push(picture_entry(format!("{KEY}{}", p.id), name, p, self.covers.development, &self.stand_ins));
        }
        out
    }

    fn picture(&self, key: &str) -> Option<&community::Picture> {
        let id = picture_id(key)?;
        self.feed.items.iter().find(|p| p.id == id)
    }

    /// The event a gallery belongs to: its own word for it, or its pictures'.
    fn event_of(&self, gallery: u64) -> Option<&Card> {
        self.gallery(gallery).and_then(|g| g.event.as_ref()).or_else(|| self.feed.items.iter().find(|p| p.gallery_id == gallery).and_then(|p| p.event.as_ref()))
    }

    /// An event's gallery shows as one group, whatever the days its pictures span.
    pub fn whole(&self, e: &Entry) -> bool {
        gallery_of(e).is_some_and(|g| self.event_of(g).is_some())
    }

    /// An event's gallery heads its pictures with the event's card once, as the website's event gallery: its pictures
    /// join its newest one, out of the places other pictures' times put them in between.
    pub fn gather(&self, entries: &mut Vec<Entry>) {
        let mut events: HashMap<u64, Vec<Entry>> = HashMap::new();
        let mut order: Vec<Result<Entry, u64>> = Vec::with_capacity(entries.len());
        for e in entries.drain(..) {
            match gallery_of(&e).filter(|g| self.event_of(*g).is_some()) {
                Some(g) => {
                    let list = events.entry(g).or_default();
                    if list.is_empty() {
                        order.push(Err(g));
                    }
                    list.push(e);
                }
                None => order.push(Ok(e)),
            }
        }
        for slot in order {
            match slot {
                Ok(e) => entries.push(e),
                Err(g) => entries.extend(events.remove(&g).unwrap_or_default()),
            }
        }
    }

    /// The height of a group's heading when the group is a gallery's (an event's gallery heads with its card).
    pub fn header_height(&self, first: &Entry) -> Option<f32> {
        let g = gallery_of(first)?;
        Some(if self.event_of(g).is_some() { 116.0 } else { 52.0 })
    }

    /// A gallery group's heading: the gallery, whose it is, when and how many; an event's gallery heads with the
    /// event's card, which opens the event's window.
    #[allow(clippy::too_many_arguments)]
    pub fn group_header(&mut self, ui: &mut Ui, cx: &Cx, pictures: &mut Pictures, first: &Entry, last: &Entry, count: usize, interactive: bool) {
        let Some(gid) = gallery_of(first) else { return };
        let r = ui.max_rect();
        let g = self.gallery(gid).cloned();
        let span = span(first.taken, last.taken);
        let items = tr_n("{count} picture", "{count} pictures", count as u64);
        if let Some(event) = self.event_of(gid).cloned() {
            let card = Rect::from_min_size(r.min, vec2(r.width(), 100.0));
            let tex = self.covers.card(&event, CoverSize::Card).and_then(|p| pictures.get(ui.ctx(), &p));
            let shown = self.event.as_ref().filter(|w| w.id == event.id).map_or(1.0, |w| w.card_shown());
            let mut cu = ui.new_child(UiBuilder::new().max_rect(card.expand(4.0)).id_salt(("event-header", gid)));
            cu.multiply_opacity(shown);
            if let Some(origin) = event_card(&mut cu, &event, card, tex.as_ref(), &format!("{span} · {items}"), interactive) {
                let mut e = event.clone();
                e.status = e.status.or(Some("published".into()));
                let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
                let mut w = events::Window::open(ui.ctx(), e.clone(), origin, 16.0, server);
                w.gallery = Some(gid);
                w.show_pictures = false;
                if self.fixture {
                    w.stage(events::staged_overview(&e), &[], Vec::new());
                }
                self.event = Some(w);
            }
            self.headers.insert(gid, Rect::from_min_size(card.min + vec2(10.0, 10.0), vec2(128.0, 80.0)));
            return;
        }
        let p = ui.painter().clone();
        let site = format!("{}gallery?tab=gallery-{gid}", if cx.snap.settings.development { Server::Development } else { Server::Production }.website());
        let b = IconButton::new(icons::ARROW_SQUARE_OUT, tr("Open on Orbiters")).size(32.0, 15.0).backdrop(theme::control()).id_salt(("gallery-site", gid));
        let br = Rect::from_min_size(pos2(r.right() - 32.0, r.top() + 6.0), vec2(32.0, 32.0));
        widgets::text_truncated(
            &p,
            pos2(r.left(), r.top() + 12.0),
            Align2::LEFT_CENTER,
            &Txt::new(&first.world_name, 18.0).weight(weight::SEMIBOLD).tracking(-0.01),
            r.width() - 56.0,
        );
        let (glyph, label, color) = visibility(g.as_ref());
        let y = r.top() + 36.0;
        let t = Txt::new(label, 11.0).weight(weight::SEMIBOLD).color(theme::FG_2);
        let tw = widgets::text_size(ui, &t).x;
        let pill = Rect::from_min_size(pos2(r.left(), y - 10.0), vec2(tw + 30.0, 20.0));
        p.rect_filled(pill, theme::radius(10.0), theme::white(0.06));
        widgets::icon(&p, pos2(pill.left() + 11.0, pill.center().y), glyph, 11.0, color);
        widgets::text(&p, pos2(pill.left() + 21.0, pill.center().y), Align2::LEFT_CENTER, &t);
        widgets::text_truncated(
            &p,
            pos2(pill.right() + 10.0, y),
            Align2::LEFT_CENTER,
            &Txt::new(format!("{span} · {items}"), 12.0).color(theme::FG_3),
            br.left() - pill.right() - 26.0,
        );
        if sharing::pressed(b.show_at(ui, br)) {
            let _ = crate::controller::open_url(&site);
        }
    }

    /// One picture's card, as the gallery's: the picture with its gallery's audience and its likes, who posted it and
    /// when, then its words or its gallery; with `tile` (the gallery's masonry and rows) the picture alone in its
    /// shape, who posted it over its bottom. Its part in the selection (`marks`) as the gallery's cards show theirs.
    /// Returns its picture's rect and its response (a click opens or selects it, a drag takes it out of the window:
    /// [`super::gallery_select`]), None for a picture no longer listed.
    #[allow(clippy::too_many_arguments)]
    pub fn card(
        &mut self,
        ui: &mut Ui,
        cx: &Cx,
        pictures: &mut Pictures,
        i: usize,
        e: &Entry,
        rect: Rect,
        interactive: bool,
        tile: bool,
        marks: super::gallery_select::Marks,
    ) -> Option<(Rect, egui::Response)> {
        let ctx = ui.ctx().clone();
        let p = self.picture(&e.key)?.clone();
        let tex = e.picture.as_ref().and_then(|pic| pictures.get(&ctx, pic));
        let author_tex = if self.fixture { None } else { avatar_of(p.author.as_ref()).and_then(|a| pictures.get(&ctx, &a)) };
        let id = Id::unique(("community-card", &e.key));
        let resp = ui.interact(rect, id, if interactive { Sense::click_and_drag() } else { Sense::hover() });
        let resp = if interactive { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        let delay = 0.08 + i.min(12) as f64 * 0.035;
        let elapsed = anim::time(&ctx) - cx.entered;
        if elapsed < delay {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
        }
        let enter = anim::spring_from(&ctx, id.with(("enter", cx.visit)), [0.0], [if elapsed >= delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
        let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() && interactive, Spring::SOFT);
        let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on() && interactive, Spring::SNAPPY);
        let thumb0 = if tile { rect } else { Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 9.0 / 16.0)) };
        let landing = morph::bounce(&ctx, thumb0);
        // Lifted while dragged out, a little inset while chosen (as the gallery's cards).
        let lift = anim::toggle(&ctx, id.with("lift"), marks.carried, if marks.carried { Spring::SNAPPY } else { Spring::BOUNCY });
        let chosen = anim::toggle(&ctx, id.with("chosen"), marks.selected, Spring::SNAPPY);
        let scale = (0.97 + 0.03 * enter) * (1.0 - 0.03 * press) * landing * (1.0 - 0.07 * lift) * (1.0 - 0.025 * chosen);
        let r = Rect::from_center_size(rect.center() + vec2(0.0, 20.0 * (1.0 - enter) - 4.0 * hover), rect.size() * scale);
        let shown = self.card_shown(&e.key);
        let mut pa = ui.painter().clone();
        pa.multiply_opacity(enter.clamp(0.0, 1.0) * shown * (1.0 - 0.6 * lift.clamp(0.0, 1.0)));
        let (thumb, corners) = if tile {
            (r, [super::gallery::TILE_RADIUS; 4])
        } else {
            widgets::frame(&pa, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover));
            (Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0)), [20.0, 20.0, 0.0, 0.0])
        };
        match &tex {
            Some(t) => feedback::cover(&pa, thumb, corners, t, 1.0 + 0.06 * ease::out_cubic(hover), 1.0),
            None => {
                feedback::fill_corners(&pa, thumb, corners, theme::INK_800);
                widgets::icon(&pa, thumb.center(), icons::IMAGE, 28.0, theme::FG_4);
            }
        }
        let shade = paint::rounded_rect_points_corners(thumb, corners.map(theme::squircle));
        paint::fill_gradient(
            &pa,
            &shade,
            &Gradient::vertical(
                thumb,
                &[
                    (0.0, theme::alpha(theme::INK_950, 0.25)),
                    (0.3, Color32::TRANSPARENT),
                    (0.6, Color32::TRANSPARENT),
                    (1.0, theme::alpha(theme::INK_950, 0.75)),
                ],
            ),
        );
        let g = self.gallery(p.gallery_id);
        let (_, label, color) = visibility(g);
        // The check on its corner; the badge steps aside for it.
        let radius = if tile { super::gallery::TILE_RADIUS } else { theme::CARD_RADIUS };
        let check = super::gallery_select::paint_card(&pa, &ctx, &e.key, r, thumb, radius, marks.selected, marks.selecting, hover);
        let mut x = thumb.left() + 12.0 + (super::gallery_select::CHECK + 6.0) * check;
        x += super::sessions::badge(
            &pa,
            ui,
            pos2(x, thumb.top() + 12.0),
            if p.event.is_some() { tr("Event") } else { label },
            theme::black(0.55),
            if p.event.is_some() { theme::ACCENT } else { color },
            false,
        ) + 6.0;
        let _ = x;
        if p.reaction_count > 0 {
            let t = Txt::new(p.reaction_count.to_string(), 10.5).weight(weight::SEMIBOLD).color(theme::FG);
            let tw = widgets::text_size(ui, &t).x;
            let br = Rect::from_min_size(pos2(thumb.right() - 12.0 - tw - 34.0, thumb.top() + 12.0), vec2(tw + 34.0, 22.0));
            pa.rect_filled(br, theme::radius(11.0), theme::black(0.55));
            widgets::icon_fill(&pa, pos2(br.left() + 14.0, br.center().y), icons::HEART, 11.0, theme::REC);
            widgets::text(&pa, pos2(br.left() + 24.0, br.center().y), Align2::LEFT_CENTER, &t);
        }
        // A tile: who posted it, over its bottom.
        if tile {
            let name = p.author.as_ref().map(|a| a.username.clone()).unwrap_or_else(|| tr("Someone").into());
            let t = Txt::new(&name, 11.5).weight(weight::SEMIBOLD).color(theme::FG);
            let w = (widgets::text_size(ui, &t).x + 34.0).min(thumb.width() - 24.0);
            let pill = Rect::from_min_size(pos2(thumb.left() + 12.0, thumb.bottom() - 12.0 - 22.0), vec2(w, 22.0));
            pa.rect_filled(pill, theme::radius(11.0), theme::black(0.55));
            widgets::avatar(&pa, Rect::from_center_size(pos2(pill.left() + 11.0, pill.center().y), vec2(18.0, 18.0)), author_tex.as_ref(), &name);
            widgets::text_truncated(&pa, pos2(pill.left() + 25.0, pill.center().y), Align2::LEFT_CENTER, &t, w - 32.0);
            self.cards.insert(e.key.clone(), thumb0);
            return Some((thumb, resp));
        }
        // Who posted it and when, then its words (or its gallery).
        let bx = r.left() + 16.0;
        let time = super::local_time(&controller::iso(e.taken));
        let time = time.rsplit_once(", ").map_or(time.clone(), |(_, t)| t.to_string());
        let tt = Txt::new(&time, 12.0).color(theme::FG_3);
        let tw = widgets::text_size(ui, &tt).x;
        widgets::text(&pa, pos2(r.right() - 16.0, thumb.bottom() + 22.0), Align2::RIGHT_CENTER, &tt);
        let name = p.author.as_ref().map(|a| a.username.clone()).unwrap_or_else(|| tr("Someone").into());
        let ar = Rect::from_center_size(pos2(bx + 10.0, thumb.bottom() + 22.0), vec2(20.0, 20.0));
        widgets::avatar(&pa, ar, author_tex.as_ref(), &name);
        widgets::text_truncated(
            &pa,
            pos2(ar.right() + 8.0, thumb.bottom() + 22.0),
            Align2::LEFT_CENTER,
            &Txt::new(&name, 14.0).weight(weight::SEMIBOLD),
            r.width() - 64.0 - tw,
        );
        let line_y = thumb.bottom() + 46.0;
        let words = p.message.as_ref().map(|m| m.text()).map(|t| t.lines().next().unwrap_or_default().to_string()).filter(|t| !t.trim().is_empty());
        match words {
            Some(w) => {
                widgets::icon(&pa, pos2(bx + 7.0, line_y), icons::CHAT_CENTERED_TEXT, 13.0, theme::FG_3);
                widgets::text_truncated(&pa, pos2(bx + 20.0, line_y), Align2::LEFT_CENTER, &Txt::new(w, 12.0).color(theme::FG_2), r.width() - 52.0);
            }
            None => {
                widgets::icon(&pa, pos2(bx + 7.0, line_y), icons::IMAGES_SQUARE, 13.0, theme::FG_3);
                widgets::text_truncated(
                    &pa,
                    pos2(bx + 20.0, line_y),
                    Align2::LEFT_CENTER,
                    &Txt::new(tr!("In {world}", world = e.world_name), 12.0).color(theme::FG_3),
                    r.width() - 52.0,
                );
            }
        }
        self.cards.insert(e.key.clone(), thumb0);
        Some((thumb, resp))
    }

    /// How much of a card shows while its picture's window is open over it (0) or lands back in it.
    pub fn card_shown(&self, key: &str) -> f32 {
        self.window.as_ref().filter(|w| w.key() == key).map_or(1.0, PictureWindow::card_shown)
    }

    /// Where another page's card drew its picture (the window opened out of it closes back into it).
    pub fn note_card(&mut self, key: &str, rect: Rect) {
        self.cards.insert(key.to_owned(), rect);
    }

    /// Opens a picture window on pictures that are not a community's (an asset's: `about` says whose each is), the
    /// `index`th out of `origin` (screenshots: `stand_ins` for their pictures).
    #[allow(clippy::too_many_arguments)]
    pub fn open_pictures(&mut self, ctx: &egui::Context, list: Vec<community::Picture>, about: Vec<About>, index: usize, origin: Rect, stand_ins: Vec<Picture>) {
        let mut w = PictureWindow::open(ctx, list, index, origin, None, None);
        w.about = about;
        w.stand_ins = stand_ins;
        w.drag_out = true;
        if let Some(said) = self.talk.clone() {
            w.talk.stage(said);
        }
        self.window = Some(w);
    }

    /// The picture its window shows was dragged out of it this frame: what it hands out (its card's entry).
    pub fn take_drag(&mut self) -> Option<Entry> {
        self.window.as_mut()?.dragged.take()
    }

    /// The key of the card whose picture the window shows.
    pub fn window_key(&self) -> Option<String> {
        self.window.as_ref().map(PictureWindow::key)
    }

    /// The window's picture is carried out of it by a drag (drawn lifted).
    pub fn set_lifted(&mut self, lifted: bool) {
        if let Some(w) = self.window.as_mut() {
            w.lifted = lifted;
        }
    }

    /// Screenshots: every picture window says this (its replies and reactions, never read from Orbiters), with the
    /// `hover`th reaction hovered.
    pub fn stage_talk(&mut self, said: super::community_talk::Said, hover: Option<usize>) {
        self.talk = Some(said.clone());
        if let Some(w) = self.window.as_mut() {
            w.talk.stage(said);
            w.talk.hover = hover;
        }
        self.talk_hover = hover;
    }

    /// The gallery chips (Communities): all of them, then each gallery with its audience or its event.
    pub fn chips(&mut self, ui: &mut Ui, rect: Rect) {
        let Some(galleries) = self.galleries.clone() else {
            let p = ui.painter();
            for i in 0..4 {
                feedback::shimmer(p, Rect::from_min_size(pos2(rect.left() + i as f32 * 128.0, rect.top()), vec2(116.0, widgets::CHIP_H)), 16.0);
            }
            return;
        };
        let mut cu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("community-chips"));
        egui::ScrollArea::horizontal().id_salt("community-chips-scroll").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
            &mut cu,
            |ui| {
                let mut options: Vec<(Option<u64>, String, &'static str, Option<Color32>)> = vec![(None, tr("All galleries").into(), icons::IMAGES_SQUARE, None)];
                for g in galleries.all() {
                    let (glyph, _, color) = visibility(Some(g));
                    let (glyph, color) = if g.event.is_some() { (icons::CALENDAR_DOTS, theme::ACCENT) } else { (glyph, color) };
                    options.push((Some(g.id), g.name.clone(), glyph, Some(color)));
                }
                // Each as wide as when chosen (its check), so choosing one never moves the others.
                let widths: Vec<f32> = options.iter().map(|(_, l, g, _)| Chip::new(l).icon(g).selected(true).width(ui).min(240.0)).collect();
                let total = widths.iter().sum::<f32>() + 8.0 * widths.len().saturating_sub(1) as f32;
                let (row, _) = ui.allocate_exact_size(vec2(total, widgets::CHIP_H), Sense::hover());
                let mut x = row.left();
                for (i, (id, label, glyph, color)) in options.iter().enumerate() {
                    let mut chip = Chip::new(label).icon(glyph).selected(self.chosen == *id).id_salt(("community-chip", *id));
                    if let Some(c) = color {
                        chip = chip.tone(*c);
                    }
                    if chip.show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(widths[i], widgets::CHIP_H))).pressed {
                        self.chosen = *id;
                    }
                    x += widths[i] + 8.0;
                }
            },
        );
    }

    /// Opens the picture of `key` out of `origin`, with the gallery's other pictures (`keys`, in the page's order)
    /// before and after it.
    pub fn open(&mut self, ctx: &egui::Context, key: &str, origin: Rect, keys: &[String]) {
        let list: Vec<community::Picture> = keys.iter().filter_map(|k| self.picture(k).cloned()).collect();
        let Some(index) = picture_id(key).and_then(|id| list.iter().position(|p| p.id == id)) else { return };
        let mut w = PictureWindow::open(ctx, list, index, origin, self.galleries.as_ref(), None);
        w.stand_ins = self.stand_ins.clone();
        w.drag_out = true;
        if let Some(said) = self.talk.clone() {
            w.talk.stage(said);
            w.talk.hover = self.talk_hover;
        }
        self.window = Some(w);
    }

    /// Opens the `n`th picture as if its card was pressed (screenshots).
    pub fn open_nth(&mut self, ctx: &egui::Context, n: usize, entries: &[Entry]) {
        let keys: Vec<String> = entries.iter().filter(|e| owns(&e.key)).map(|e| e.key.clone()).collect();
        let Some(key) = keys.get(n).cloned() else { return };
        let origin = self.cards.get(&key).copied().unwrap_or(Rect::from_min_size(pos2(300.0, 300.0), vec2(320.0, 180.0)));
        self.open(ctx, &key, origin, &keys);
    }

    /// Opens the first event gallery's event as if its card was pressed (screenshots).
    pub fn open_event(&mut self, ctx: &egui::Context) {
        let Some(g) = self.galleries.as_ref().and_then(|g| g.all().find(|g| g.event.is_some()).cloned()) else { return };
        let Some(e) = g.event.clone() else { return };
        let origin = self.headers.get(&g.id).copied().unwrap_or(Rect::from_min_size(pos2(300.0, 200.0), vec2(128.0, 80.0)));
        let mut w = events::Window::open(ctx, e.clone(), origin, 16.0, Server::Production);
        w.gallery = Some(g.id);
        w.show_pictures = false;
        w.stage(events::staged_overview(&e), &[("main", events::Ask::Idle)], Vec::new());
        self.event = Some(w);
    }

    /// The windows (a picture, an event) over the page.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx, pictures: &mut Pictures) {
        let ctx = ui.ctx().clone();
        let page = ui.max_rect();
        if let Some(w) = self.window.as_mut() {
            if let Some(r) = self.cards.get(&w.key()).copied().filter(|r| r.intersects(page)) {
                w.set_origin(r);
            }
            w.covered = self.event.is_some();
            if w.show(ui, cx, pictures, Order::Foreground) {
                self.window = None;
            }
        }
        // The event a picture was taken at, out of the picture window's event line.
        if let Some((card, origin)) = self.window.as_mut().and_then(|w| w.event_pressed.take()) {
            let full = self.galleries.as_ref().and_then(|g| g.all().find_map(|g| g.event.clone().filter(|e| e.id == card.id)));
            let card = full.unwrap_or(card);
            let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
            let mut w = events::Window::open(&ctx, card.clone(), origin, 14.0, server);
            w.show_pictures = false;
            if self.fixture {
                w.stage(events::staged_overview(&card), &[], Vec::new());
            }
            self.event = Some(w);
        }
        if let Some(w) = self.event.as_mut() {
            let covers = self.covers.clone();
            if w.show(ui, cx, pictures, &covers) {
                self.event = None;
            }
        }
        let _ = ctx;
    }
}

/// A gallery picture as the gallery's entry (its card, what it hands out when dragged): `key`, under `name`.
pub(super) fn picture_entry(key: String, name: String, p: &community::Picture, development: bool, stand_ins: &[Picture]) -> Entry {
    let author = p.author.as_ref().map(|a| a.username.clone()).unwrap_or_default();
    Entry {
        key,
        world_id: format!("{GROUP}{}", p.gallery_id),
        world_name: name,
        taken: controller::unix(p.posted_at.as_deref().unwrap_or_default()),
        instance: None,
        source_key: None,
        people: if author.is_empty() { Vec::new() } else { vec![author] },
        local: None,
        remote: None,
        content: Default::default(),
        ready: true,
        no_keyframes: false,
        picture_mode: None,
        no_depth: false,
        trimmed: false,
        result: None,
        picture: source(p, false, development, stand_ins),
        size: (p.width > 0 && p.height > 0).then_some([p.width, p.height]),
        layers: Vec::new(),
        edits: None,
        uploaded_edits: None,
    }
}

/// "28 Oct 2026, 21:30 – 22:30" for a group's pictures (the newest first).
fn span(newest: u64, oldest: u64) -> String {
    let date = super::local_time(&controller::iso(newest));
    if newest == oldest {
        return date;
    }
    let start = super::local_time(&controller::iso(oldest));
    match (start.rsplit_once(", "), date.rsplit_once(", ")) {
        (Some((d1, t1)), Some((d2, t2))) if d1 == d2 => format!("{d1}, {t1} – {t2}"),
        _ => format!("{start} – {date}"),
    }
}

/// The event an event gallery belongs to, as the website shows it above the gallery: its cover, Event and its status,
/// the title, when, where and who hosts it, Event details. Returns its cover's rect when pressed.
fn event_card(ui: &mut Ui, e: &Card, rect: Rect, tex: Option<&TextureHandle>, line: &str, interactive: bool) -> Option<Rect> {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("community-event", &e.id, rect.top() as i32));
    let resp = ui.interact(rect, id, if interactive { Sense::click() } else { Sense::hover() });
    let resp = if interactive { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() && interactive, Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on() && interactive, Spring::SNAPPY);
    let cover = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(128.0, 80.0));
    let landing = morph::bounce(&ctx, cover);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.01 * press) * landing);
    let p = ui.painter().clone();
    widgets::frame(&p, r, 24.0, theme::mix(theme::surface(), theme::INK_800, hover));
    let cover = Rect::from_min_size(r.min + vec2(10.0, 10.0), vec2(128.0, r.height() - 20.0));
    events::paint_cover(&p, cover, [16.0; 4], tex, &e.id, 1.0 + 0.05 * hover, 1.0);
    let x = cover.right() + 16.0;
    let go_t = Txt::new(tr("Event details"), 13.0).weight(weight::SEMIBOLD).color(theme::ACCENT);
    let go_w = widgets::text_size(ui, &go_t).x + 40.0;
    let narrow = r.width() < 560.0;
    let room = if narrow { r.right() - x - 52.0 } else { r.right() - x - go_w - 24.0 };
    // Event and its status.
    let event_t = Txt::caps(tr("Event"));
    widgets::text(&p, pos2(x, r.top() + 22.0), Align2::LEFT_CENTER, &event_t);
    // The status sits after the word: 48 points does for "EVENT", not for "ÉVÉNEMENT".
    let status_x = x + (widgets::text_size(ui, &event_t).x + 8.0).max(48.0);
    let ph = events::phase(e.status.as_deref(), e.starts_at.as_deref(), e.ends_at.as_deref(), false, events::now());
    let (fg, bg) = match ph {
        Phase::Live => (theme::REC, theme::alpha(theme::REC, 0.14)),
        Phase::Soon(_) => (theme::ACCENT, theme::alpha(theme::ACCENT, 0.14)),
        Phase::Cancelled => (theme::DANGER, theme::alpha(theme::DANGER, 0.12)),
        _ => (theme::FG_3, theme::white(0.07)),
    };
    let st = Txt::new(events::phase_label(ph), 11.0).weight(weight::SEMIBOLD).color(fg);
    let sw = widgets::text_size(ui, &st).x;
    let pill = Rect::from_min_size(pos2(status_x, r.top() + 13.0), vec2(sw + if ph == Phase::Live { 28.0 } else { 18.0 }, 18.0));
    p.rect_filled(pill, theme::radius(9.0), bg);
    let mut tx = pill.left() + 9.0;
    if ph == Phase::Live {
        p.circle_filled(pos2(tx + 3.0, pill.center().y), 3.0, fg);
        tx += 10.0;
    }
    widgets::text(&p, pos2(tx, pill.center().y), Align2::LEFT_CENTER, &st);
    widgets::text_truncated(&p, pos2(x, r.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(&e.title, 16.5).weight(weight::BOLD).tracking(-0.01), room);
    // When, where, who.
    let fy = r.top() + 70.0;
    let mut fx = x;
    let facts_end = x + room;
    let when = events::when(e.starts_at.as_deref(), e.ends_at.as_deref());
    widgets::icon(&p, pos2(fx + 6.0, fy), icons::CALENDAR_BLANK, 12.5, theme::FG_3);
    let wr = widgets::text_truncated(&p, pos2(fx + 19.0, fy), Align2::LEFT_CENTER, &Txt::new(when, 12.5).color(theme::FG_3), (facts_end - fx - 19.0).max(40.0));
    fx = wr.right() + 14.0;
    if let Some(w) = e.world.as_ref().and_then(|w| w.name.as_ref()) {
        if facts_end - fx > 60.0 {
            widgets::icon(&p, pos2(fx + 6.0, fy), icons::GLOBE, 12.5, theme::FG_3);
            let wr = widgets::text_truncated(
                &p,
                pos2(fx + 19.0, fy),
                Align2::LEFT_CENTER,
                &Txt::new(w, 12.5).color(theme::FG_3),
                (facts_end - fx - 19.0).min(180.0),
            );
            fx = wr.right() + 14.0;
        }
    }
    if let Some(h) = e.community.as_ref().or(e.discord.as_ref()) {
        if facts_end - fx > 60.0 {
            let ar = Rect::from_center_size(pos2(fx + 8.0, fy), vec2(16.0, 16.0));
            widgets::avatar(&p, ar, None, &h.name);
            widgets::text_truncated(
                &p,
                pos2(ar.right() + 6.0, fy),
                Align2::LEFT_CENTER,
                &Txt::new(&h.name, 12.5).color(theme::FG_3),
                facts_end - ar.right() - 6.0,
            );
        }
    }
    widgets::text_truncated(&p, pos2(x, r.bottom() - 8.0 - 2.0), Align2::LEFT_CENTER, &Txt::new(line, 11.5).color(theme::FG_4), room);
    // Event details.
    let go = if narrow {
        Rect::from_center_size(pos2(r.right() - 30.0, r.center().y), vec2(36.0, 36.0))
    } else {
        Rect::from_min_size(pos2(r.right() - 16.0 - go_w, r.center().y - 17.0), vec2(go_w, 34.0))
    };
    p.rect_filled(go, theme::radius(17.0), theme::alpha(theme::ACCENT, 0.12 + 0.06 * hover));
    if narrow {
        widgets::icon(&p, go.center(), icons::CARET_RIGHT, 16.0, theme::ACCENT);
    } else {
        widgets::text(&p, pos2(go.left() + 14.0, go.center().y), Align2::LEFT_CENTER, &go_t);
        widgets::icon(&p, pos2(go.right() - 16.0 + 3.0 * hover, go.center().y), icons::CARET_RIGHT, 14.0, theme::ACCENT);
    }
    (interactive && sharing::pressed(resp)).then_some(cover)
}

// ---------------------------------------------------------------------------------------------------------------------
// The picture window.

static OPENED: AtomicU64 = AtomicU64::new(0);

/// Whose a picture of the window is when it is not a community's (an asset's, in My assets): its card's key, the
/// asset's name, what it is to the member ("Avatar · Bought on Orbiters"), its page on the website.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct About {
    pub key: String,
    pub title: String,
    pub line: String,
    pub site: String,
}

/// A gallery picture in the capture window's look: grown out of its card (shared geometry on the window's spring),
/// sized to the picture (wide ones over their details, tall ones beside), ←/→ to the gallery's other pictures. Its
/// details: who posted it and when, the gallery, its words, its likes and replies, and the event it was taken at.
pub struct PictureWindow {
    list: Vec<community::Picture>,
    index: usize,
    morph: Option<Morph>,
    origin: Rect,
    galleries: HashMap<u64, Gallery>,
    /// The event every picture was taken at (opened from the event's window).
    event: Option<Card>,
    /// The way the last step went (for the slide) and its serial.
    nav: (u64, f32),
    previous: Option<TextureHandle>,
    last: Option<TextureHandle>,
    measured: HashMap<u32, f32>,
    /// The event line was pressed: its card and where it is.
    pub event_pressed: Option<(Card, Rect)>,
    stand_ins: Vec<Picture>,
    progress: f32,
    /// Another window is over it (the event it was taken at): Escape, arrows and presses are that window's.
    pub covered: bool,
    /// Whose each picture is, when they are not a community's (in the list's order; empty for a community's).
    about: Vec<About>,
    /// The photo editor on a copy of the picture (downloaded at its full size), over the window while it is open.
    editor: Option<super::editor::Editor>,
    /// Its replies and reactions, read once per picture ([`super::community_talk`]).
    pub talk: super::community_talk::Talk,
    /// Its page drags pictures out of the window (Photos & captures; not the Events page): a press on the picture that
    /// moves is a drag out ([`super::gallery_select::WindowDrag`]).
    pub drag_out: bool,
    /// What the picture's drag out hands out, the frame it started (its card's entry).
    pub dragged: Option<Entry>,
    /// The page carries the picture out of the window: drawn lifted until the drop.
    pub lifted: bool,
    drag: super::gallery_select::WindowDrag,
}

const WINDOW_PAD: f32 = 24.0;
const WINDOW_GUTTER: f32 = 24.0;

impl PictureWindow {
    pub fn open(_ctx: &egui::Context, list: Vec<community::Picture>, index: usize, origin: Rect, galleries: Option<&Galleries>, event: Option<Card>) -> Self {
        Self {
            index: index.min(list.len().saturating_sub(1)),
            list,
            morph: None,
            origin,
            galleries: galleries.map(|g| g.all().map(|g| (g.id, g.clone())).collect()).unwrap_or_default(),
            event,
            nav: (0, 0.0),
            previous: None,
            last: None,
            measured: HashMap::new(),
            event_pressed: None,
            stand_ins: Vec::new(),
            progress: 0.0,
            covered: false,
            about: Vec::new(),
            editor: None,
            talk: Default::default(),
            drag_out: false,
            dragged: None,
            lifted: false,
            drag: Default::default(),
        }
    }

    /// Screenshots: this computer's VRChat pictures stand in for the gallery's.
    pub fn with_stand_ins(mut self, stand_ins: Vec<Picture>) -> Self {
        self.stand_ins = stand_ins;
        self
    }

    pub fn key(&self) -> String {
        if let Some(a) = self.about.get(self.index) {
            return a.key.clone();
        }
        self.list.get(self.index).map(|p| format!("{KEY}{}", p.id)).unwrap_or_default()
    }

    pub fn set_origin(&mut self, r: Rect) {
        if let Some(m) = self.morph.as_mut() {
            m.set_origin(r);
        }
    }

    fn card_shown(&self) -> f32 {
        if self.morph.as_ref().is_some_and(Morph::is_closing) {
            morph::source_alpha(self.progress)
        } else {
            0.0
        }
    }

    fn step(&mut self, by: i64) {
        let Some(next) = usize::try_from(self.index as i64 + by).ok().filter(|i| *i < self.list.len()) else { return };
        self.previous = self.last.clone();
        self.index = next;
        self.nav = (self.nav.0 + 1, by as f32);
    }

    /// Draws it over the page in `order`; true once it closed back into its card.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx, pictures: &mut Pictures, order: Order) -> bool {
        let ctx = ui.ctx().clone();
        let page = ui.max_rect();
        let Some(p) = self.list.get(self.index).cloned() else { return true };
        let area = Rect::from_min_max(
            pos2(page.left() + WINDOW_GUTTER, page.top() + cx.top + 12.0),
            pos2(page.right() - WINDOW_GUTTER, page.bottom() - WINDOW_GUTTER),
        );
        let development = cx.snap.settings.development;
        // The photo editor, over the whole window under its title bar; the window waits under it.
        if let Some(e) = self.editor.as_mut() {
            let window = ctx.content_rect();
            let rect = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
            let mut open = true;
            egui::Area::new(Id::unique("community-editor")).order(Order::Foreground).fixed_pos(rect.min).show(&ctx, |ui| {
                ui.set_min_size(rect.size());
                open = e.show(ui, rect);
            });
            if !open {
                self.editor = None;
            }
            return false;
        }
        let thumb = source(&p, false, development, &self.stand_ins).and_then(|s| pictures.get(&ctx, &s));
        let large = source(&p, true, development, &self.stand_ins).and_then(|s| pictures.get(&ctx, &s));
        let aspect = large.as_ref().or(thumb.as_ref()).map(|t| t.size_vec2().x / t.size_vec2().y.max(1.0)).unwrap_or(if p.height > 0 {
            p.width as f32 / p.height as f32
        } else {
            16.0 / 9.0
        });
        let measured = &self.measured;
        // Details taller than what the picture leaves of the page (many replies) scroll under it: they take what the
        // picture at its own height leaves, and at least two fifths of the page (the picture is cropped a little).
        let layout = viewer::picture_layout(area, aspect, |w| {
            let natural = measured.get(&(w.round() as u32)).copied().unwrap_or(if w <= viewer::SIDE_W + 1.0 { 300.0 } else { 170.0 });
            let room = if w <= viewer::SIDE_W + 1.0 { area.height() } else { (area.height() - w / aspect.max(0.01)).max(area.height() * 0.4) };
            natural.min(room)
        });
        let origin = self.origin;
        let morph = self.morph.get_or_insert_with(|| Morph::new(Id::unique(("community-window", OPENED.fetch_add(1, Ordering::Relaxed))), origin, 20.0));
        let f = morph.frame(&ctx, layout.window);
        self.progress = f.t;
        let (wid, closing) = (morph.id(), morph.is_closing());
        let picture = morph.part(&ctx, "picture", layout.picture, &f);
        let nav = if self.nav.0 == 0 { 1.0 } else { anim::spring_from(&ctx, wid.with(("nav", self.nav.0)), [0.0], [1.0], Spring::SOFT)[0] };
        if nav >= 0.999 {
            self.previous = None;
        }
        let k = nav.clamp(0.0, 1.0);
        let window = ctx.content_rect();
        let catcher = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
        let mut close = false;
        let mut reopen = false;
        let mut edit = false;
        let mut step = 0i64;
        let mut open_url: Option<String> = None;
        let gallery = self.galleries.get(&p.gallery_id).cloned();
        let event =
            p.event.clone().or(self.event.clone()).map(|brief| gallery.as_ref().and_then(|g| g.event.clone()).filter(|e| e.id == brief.id).unwrap_or(brief));
        let development = cx.snap.settings.development;
        egui::Area::new(wid.with("area")).order(order).fixed_pos(catcher.min).fade_in(false).show(&ctx, |ui| {
            ui.set_min_size(catcher.size());
            let backdrop = ui.interact(catcher, wid.with("backdrop"), Sense::click());
            viewer::paint_backdrop(&ui.painter().with_clip_rect(window), window, &f);
            if widgets::pressed(&backdrop) && !f.rect.contains(backdrop.interact_pointer_pos().unwrap_or_default()) && !self.covered {
                close = true;
            }
            if widgets::pressed(&ui.interact(f.rect, wid.with("surface"), Sense::click())) && closing {
                reopen = true;
            }
            let pa = ui.painter().clone();
            viewer::paint_surface(&pa, &f);
            ui.multiply_opacity(f.shape);
            let pa = ui.painter().clone();
            let r = f.radius;
            let radii = match layout.arrangement {
                Arrangement::Under => [r, r, 0.0, 0.0],
                Arrangement::Beside => [r, 0.0, 0.0, r],
            };
            if let Some(prev) = self.previous.as_ref().filter(|_| k < 0.999) {
                feedback::cover(&pa, picture, radii, prev, 1.0, 1.0);
            }
            // Dragged out of the window (its page allows it): a press on the picture that moves past the slop hands
            // it to Windows' drag and drop; the controls over it (arrows, Edit, close) are above it and keep their
            // presses. Lifted, it is a little smaller and faded until the drop, as the grid's cards.
            let lift = anim::toggle(&ctx, wid.with("picture-lift"), self.lifted, Spring::BOUNCY).clamp(-0.2, 1.2);
            if self.drag_out && !closing {
                let resp = ui.interact(picture, wid.with("picture"), Sense::drag());
                if self.drag.update(&resp) {
                    let name = self.about.get(self.index).map(|a| a.title.clone()).or_else(|| gallery.as_ref().map(|g| g.name.clone()));
                    self.dragged = Some(picture_entry(self.key(), name.unwrap_or_else(|| tr("Community gallery").into()), &p, development, &self.stand_ins));
                }
                resp.on_hover_cursor(if self.lifted || self.drag.dragging() { CursorIcon::Grabbing } else { CursorIcon::Grab });
            }
            let mut np = pa.with_clip_rect(picture.intersect(pa.clip_rect()));
            if lift > 0.001 {
                feedback::fill_corners(&np, picture, radii, theme::alpha(theme::INK_750, lift.clamp(0.0, 1.0)));
                np.multiply_opacity(1.0 - 0.35 * lift.clamp(0.0, 1.0));
            }
            let at = picture.translate(vec2(28.0 * self.nav.1 * (1.0 - k), 0.0));
            let at = Rect::from_center_size(at.center(), at.size() * (1.0 - 0.04 * lift));
            let shown_large = feedback::loaded(&ctx, wid.with(("large", self.nav.0)), large.is_some());
            match (&thumb, &large) {
                (Some(t), _) if shown_large < 0.99 => feedback::cover(&np, at, radii, t, 1.0, k),
                (None, None) => {
                    feedback::fill_corners(&np, at, radii, theme::alpha(theme::INK_750, k));
                    widgets::icon(&np, at.center(), icons::IMAGE, 40.0, theme::alpha(theme::FG_4, k));
                }
                _ => {}
            }
            if let Some(l) = &large {
                feedback::cover(&np, at, radii, l, 1.0, shown_large * k);
            }
            self.last = large.clone().filter(|_| shown_large > 0.99).or_else(|| thumb.clone());
            let mut cu = ui.new_child(UiBuilder::new().max_rect(f.rect).id_salt(wid.with("chrome")));
            cu.set_clip_rect(f.rect.intersect(ui.clip_rect()));
            cu.multiply_opacity(f.content);
            if closing || f.content < 0.05 {
                cu.disable();
            }
            // What it is, over the picture.
            let about = self.about.get(self.index);
            let (vg, vl, vc) = if about.is_some() { (icons::TAG, "", theme::ACCENT) } else { visibility(gallery.as_ref()) };
            let label = match about {
                Some(a) => a.title.clone(),
                None => gallery.as_ref().map(|g| g.name.clone()).unwrap_or_else(|| tr("Community gallery").into()),
            };
            let t = Txt::new(&label, 11.5).weight(weight::SEMIBOLD).color(theme::FG);
            let tw = widgets::text_size(&cu, &t).x.min(picture.width() - 120.0);
            let badge = Rect::from_min_size(picture.min + vec2(16.0, 16.0), vec2(tw + 38.0, 28.0));
            let cp = cu.painter().clone();
            cp.rect_filled(badge, theme::radius(14.0), theme::black(0.55));
            widgets::icon(&cp, pos2(badge.left() + 17.0, badge.center().y), vg, 14.0, vc);
            widgets::text_truncated(&cp, pos2(badge.left() + 29.0, badge.center().y), Align2::LEFT_CENTER, &t, tw);
            let _ = vl;
            if viewer::close_button(&mut cu, f.rect, tr("Close")) {
                close = true;
            }
            // Edit a copy of a gallery's picture, or of an asset's by its link.
            let linked = [&p.full_url, &p.src].into_iter().flatten().any(|u| u.starts_with("https://"));
            if p.id != 0 || linked {
                let b = widgets::Button::new(tr("Edit")).icon(icons::SLIDERS_HORIZONTAL).variant(widgets::Variant::Glass).id_salt(wid.with("edit"));
                let br = viewer::beside_close(f.rect, b.desired_width(&cu));
                if sharing::pressed(b.show_at(&mut cu, br)) {
                    edit = true;
                }
            }
            for (dir, glyph, label, x, there) in [
                (-1i64, icons::CARET_LEFT, tr("Previous"), picture.left() + 16.0 + 20.0, self.index > 0),
                (1, icons::CARET_RIGHT, tr("Next"), picture.right() - 16.0 - 20.0, self.index + 1 < self.list.len()),
            ] {
                if !there {
                    continue;
                }
                let br = Rect::from_center_size(pos2(x, picture.center().y), vec2(40.0, 40.0));
                if sharing::pressed(IconButton::new(glyph, label).size(40.0, 18.0).backdrop(theme::black(0.5)).show_at(&mut cu, br)) {
                    step = dir;
                }
            }
            // Details, laid out at the window's final width and height, scrolling when they are taller (the bar in the
            // window's right gutter, shown once the window landed).
            let d = match layout.arrangement {
                Arrangement::Under => Rect::from_min_size(pos2(f.rect.left(), picture.bottom()), layout.details.size()),
                Arrangement::Beside => Rect::from_min_size(pos2(picture.right(), f.rect.top()), layout.details.size()),
            };
            let d = d.translate(vec2(14.0 * self.nav.1 * (1.0 - k), 0.0));
            let mut du = ui.new_child(UiBuilder::new().max_rect(d).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(wid.with("details")));
            du.set_clip_rect(f.rect.intersect(ui.clip_rect()).intersect(d));
            let alpha = f.content * k;
            du.multiply_opacity(alpha);
            if closing || alpha < 0.5 {
                du.disable();
            }
            let pad_top = if layout.arrangement == Arrangement::Beside { 64.0 } else { 18.0 };
            let landed = !closing && f.content > 0.99 && k > 0.99;
            let mut natural = 0.0;
            egui::ScrollArea::vertical()
                .id_salt(wid.with(("details-scroll", p.id)))
                .max_height(d.height())
                .auto_shrink([false, true])
                .scroll_bar_visibility(if landed {
                    egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                } else {
                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                })
                .show(&mut du, |su| {
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(WINDOW_PAD as i8, 0)).show(su, |su| {
                        su.set_width(d.width() - 2.0 * WINDOW_PAD);
                        su.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        let dy = 10.0 * (1.0 - alpha);
                        su.add_space(pad_top + dy);
                        let top = su.cursor().top();
                        let about = self.about.get(self.index).cloned();
                        details(su, wid, &p, gallery.as_ref(), event.as_ref(), about.as_ref(), pictures, &self.stand_ins, development, &mut open_url, &mut self.event_pressed, &mut self.talk);
                        talk_part(su, wid, &p, about.is_some(), &mut self.talk, pictures, development, &mut open_url);
                        su.add_space(22.0);
                        natural = su.cursor().top() - top + pad_top;
                    });
                });
            self.measured.insert(layout.details.width().round() as u32, natural.ceil());
        });
        if let Some(url) = open_url {
            let r = crate::controller::open_url(&url);
            cx.run(&ctx, r);
        }
        if edit && !closing {
            let about = self.about.get(self.index);
            let title = about.map(|a| a.title.clone()).or_else(|| event.as_ref().map(|e| e.title.clone())).or_else(|| gallery.as_ref().map(|g| g.name.clone())).unwrap_or_default();
            let mut source = super::editor::gallery_source(&p, event.as_ref(), development);
            if let (Some(a), Some(r)) = (about, source.remote.as_mut()) {
                // An asset's picture: named after the asset.
                r.name = a.title.clone();
            }
            // Screenshots: the stand-in picture in place of the download.
            if let Some(Picture::Shot { path, .. }) = self.stand_ins.get((p.id as usize) % self.stand_ins.len().max(1)) {
                if !source.picture.exists() {
                    let _ = std::fs::create_dir_all(source.picture.parent().unwrap_or(std::path::Path::new(".")));
                    let _ = std::fs::copy(path, &source.picture);
                }
            }
            self.editor = Some(super::editor::Editor::open(&ctx, source, title));
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        if !closing && !typing && !self.covered && self.event_pressed.is_none() {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowLeft) {
                    step = -1;
                } else if i.key_pressed(egui::Key::ArrowRight) {
                    step = 1;
                }
            });
        }
        if step != 0 && !closing {
            self.step(step);
        }
        if !closing && !self.covered && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if let Some(m) = self.morph.as_mut() {
            if close {
                m.close();
            }
            if reopen && !f.closed {
                m.reopen();
            }
        }
        f.closed
    }
}

/// Under the details of a community's picture: the replies to its Discord message ([`super::community_talk`]); an
/// asset's picture has none.
#[allow(clippy::too_many_arguments)]
fn talk_part(ui: &mut Ui, wid: Id, p: &community::Picture, asset: bool, talk: &mut super::community_talk::Talk, pictures: &mut Pictures, development: bool, open_url: &mut Option<String>) {
    if !asset {
        super::community_talk::replies(ui, wid, p, talk, pictures, development, open_url);
    }
}

/// The picture's details: who posted it and when (with Open on Orbiters and the full picture), its gallery, its words,
/// its reactions (who reacted on hover), and the event it was taken at. An asset's picture (`about`): the asset, what it is to the
/// member, and its page.
#[allow(clippy::too_many_arguments)]
fn details(
    ui: &mut Ui,
    wid: Id,
    p: &community::Picture,
    gallery: Option<&Gallery>,
    event: Option<&Card>,
    about: Option<&About>,
    pictures: &mut Pictures,
    stand_ins: &[Picture],
    development: bool,
    open_url: &mut Option<String>,
    event_pressed: &mut Option<(Card, Rect)>,
    talk: &mut super::community_talk::Talk,
) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let server = if development { Server::Development } else { Server::Production };
    // Who and when, the actions on the right.
    let (row, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
    let pa = ui.painter().clone();
    let name = match about {
        Some(a) => a.title.clone(),
        None => p.author.as_ref().map(|a| a.username.clone()).unwrap_or_else(|| tr("Someone").into()),
    };
    let face = if stand_ins.is_empty() { avatar_of(p.author.as_ref()).and_then(|a| pictures.get(&ctx, &a)) } else { None };
    let ar = Rect::from_min_size(pos2(row.left(), row.top() + 4.0), vec2(36.0, 36.0));
    if about.is_some() {
        pa.rect_filled(ar, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.13));
        widgets::icon(&pa, ar.center(), icons::TAG, 17.0, theme::ACCENT);
    } else {
        widgets::avatar(&pa, ar, face.as_ref(), &name);
    }
    let site = match about {
        Some(a) => a.site.clone(),
        None => format!("{}gallery?tab=gallery-{}", server.website(), p.gallery_id),
    };
    let mut bx = row.right();
    for (glyph, label, url) in [
        (icons::ARROW_SQUARE_OUT, mark("Open on Orbiters"), Some(site)),
        (icons::ARROWS_OUT, mark("Open the full picture"), p.full_url.clone().filter(|u| u.starts_with("https://"))),
    ] {
        let Some(url) = url else { continue };
        let br = Rect::from_min_size(pos2(bx - 36.0, row.top() + 4.0), vec2(36.0, 36.0));
        if sharing::pressed(IconButton::new(glyph, tr(label)).size(36.0, 16.0).backdrop(theme::control()).id_salt(wid.with(label)).show_at(ui, br)) {
            *open_url = Some(url);
        }
        bx -= 44.0;
    }
    let room = bx - ar.right() - 16.0;
    widgets::text_truncated(&pa, pos2(ar.right() + 12.0, ar.top() + 9.0), Align2::LEFT_CENTER, &Txt::new(&name, 15.0).weight(weight::SEMIBOLD), room);
    let posted = match about {
        Some(a) => a.line.clone(),
        None => p.posted_at.as_deref().map(super::local_time).map(|t| tr!("Posted {time}", time = t)).unwrap_or_default(),
    };
    widgets::text_truncated(&pa, pos2(ar.right() + 12.0, ar.top() + 28.0), Align2::LEFT_CENTER, &Txt::new(posted, 12.0).color(theme::FG_3), room);
    if about.is_some() {
        return;
    }
    // The gallery.
    ui.add_space(12.0);
    let (grow, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let (vg, vl, vc) = visibility(gallery);
    widgets::icon(&pa, pos2(grow.left() + 7.0, grow.center().y), vg, 13.0, vc);
    let gname = gallery.map(|g| g.name.clone()).unwrap_or_else(|| tr("Community gallery").into());
    widgets::text_truncated(
        &pa,
        pos2(grow.left() + 21.0, grow.center().y),
        Align2::LEFT_CENTER,
        &Txt::new(format!("{gname} · {vl}"), 12.5).color(theme::FG_2),
        w - 21.0,
    );
    // Its words.
    if let Some(text) = p.message.as_ref().map(|m| m.text()).filter(|t| !t.trim().is_empty()) {
        ui.add_space(12.0);
        let g = ui.painter().layout_job({
            let mut job = Txt::new(text.trim(), 14.0).color(theme::FG).line_height(21.0).job(w);
            job.wrap.max_rows = 6;
            job
        });
        let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
        ui.painter().galley(r.min, g, theme::FG);
    }
    // Its reactions, who reacted on hover ([`super::community_talk`]; its replies come after the event).
    super::community_talk::reactions(ui, wid, p, talk, pictures, development);
    // The event it was taken at.
    if let Some(e) = event {
        ui.add_space(14.0);
        let (r, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
        let id = wid.with("event");
        let resp = ui.interact(r, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
        let pa = ui.painter().clone();
        widgets::frame(&pa, r, 16.0, theme::mix(theme::white(0.04), theme::white(0.08), hover));
        let tile = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + 10.0), vec2(40.0, 40.0));
        pa.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.13));
        widgets::icon(&pa, tile.center(), icons::CALENDAR_DOTS, 18.0, theme::ACCENT);
        let tx = tile.right() + 12.0;
        let room = r.right() - tx - 34.0;
        widgets::text(&pa, pos2(tx, r.top() + 15.0), Align2::LEFT_CENTER, &Txt::caps(tr("Taken at")));
        widgets::text_truncated(&pa, pos2(tx, r.top() + 31.0), Align2::LEFT_CENTER, &Txt::new(&e.title, 13.5).weight(weight::SEMIBOLD), room);
        widgets::text_truncated(
            &pa,
            pos2(tx, r.top() + 47.0),
            Align2::LEFT_CENTER,
            &Txt::new(events::when(e.starts_at.as_deref(), e.ends_at.as_deref()), 11.5).color(theme::FG_3),
            room,
        );
        widgets::icon(&pa, pos2(r.right() - 18.0 + 3.0 * hover, r.center().y), icons::CARET_RIGHT, 15.0, theme::FG_3);
        if sharing::pressed(resp) {
            *event_pressed = Some((e.clone(), tile));
        }
    }
    let _ = Tone::Ok;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_gallery_heads_its_pictures_once() {
        let mut state = State::default();
        let event = Card { id: "e1".into(), ..Default::default() };
        let picture = |id, gallery_id, event: Option<Card>| community::Picture { id, gallery_id, event, ..Default::default() };
        state.feed.items = vec![picture(1, 31, Some(event.clone())), picture(2, 12, None), picture(3, 31, Some(event.clone())), picture(4, 12, None)];
        let mut entries = state.entries("");
        let order = |e: &[Entry]| e.iter().map(|e| e.key.clone()).collect::<Vec<_>>();
        state.gather(&mut entries);
        assert_eq!(order(&entries), ["community:1", "community:3", "community:2", "community:4"]);
        assert!(state.whole(&entries[0]) && !state.whole(&entries[2]));
        assert_eq!(state.header_height(&entries[0]), Some(116.0));
        assert_eq!(state.header_height(&entries[2]), Some(52.0));
    }
}
