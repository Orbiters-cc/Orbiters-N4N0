//! Photos & captures: local pictures, captures and the authenticated shared feed in one date-ordered gallery.
use super::{
    sharing::{self, Dialog},
    Cx, Job,
};
use crate::controller::shares::{self as controller, Entry};
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_core::{
    account::Server,
    shares::{Feed, Query},
    vrchat_log::WorldState,
};
use orbiters_ui::{
    anim::{self, Spring},
    icons,
    theme::{self, weight, Txt},
    widgets::{self, Button, SegOption, Variant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    Mine,
    #[default]
    All,
    Shared,
}
impl Filter {
    fn value(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::All => "all",
            Self::Shared => "shared",
        }
    }
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
    instance: Option<Entry>,
    fixture: bool,
    roster_job: Job<Vec<String>>,
}
impl State {
    pub fn busy(&self) -> bool {
        self.requests.busy()
            || self.hash_job.running()
            || self.local_job.running()
            || self.feed_job.running()
            || self.pictures.as_ref().is_some_and(|p| p.loading())
            || self.dialog.as_ref().is_some_and(Dialog::loading)
    }
    pub fn stage(&mut self, ctx: &egui::Context, name: &str) {
        self.requests.stage(name);
        self.shown_key = None;
        self.fixture = true;
        self.feed_job = Job::default();
        self.feed = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/captures.json")).unwrap();
        if name.starts_with("gallery-") {
            self.locals.truncate(2);
            for (n, e) in self.locals.iter_mut().enumerate() {
                e.world_id = self.feed.items[0].world.id.clone();
                e.world_name = self.feed.items[0].world.name.clone();
                e.instance = self.feed.items[0].instance_id.clone();
                e.taken = controller::unix("2026-10-28T21:30:00Z") - n as u64 * 300;
            }
        }
        if name == "gallery-public" {
            self.feed.items[0].public = true;
        }
        if name == "gallery-instance" {
            let mut entry = controller::remote_entry(self.feed.items[0].clone());
            entry.instance = Some("12345~friends(usr_11111111-1111-1111-1111-111111111111)~region(eu)".into());
            entry.people = vec!["Alex".into(), "Mika".into(), "Robin".into()];
            self.instance = Some(entry);
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
            "gallery-explicit",
            "gallery-error",
            "gallery-picture",
            "gallery-progress",
            "gallery-request",
        ]
        .contains(&name)
        {
            let item = self.feed.items[usize::from(matches!(name, "gallery-grantee" | "gallery-picture" | "gallery-request"))].clone();
            let mut entry = controller::remote_entry(item);
            if let Some(p) = self.locals.iter().find(|e| e.picture.is_some()).and_then(|e| e.picture.clone()) {
                entry.picture = Some(p);
            }
            let mut d = Dialog::new(ctx, entry, false);
            d.fixture(if name == "gallery-request" { "friends" } else { name.trim_start_matches("gallery-") });
            self.dialog = Some(d);
        }
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
    let requested = (state.filter, state.query.trim().to_string());
    if !state.fixture && state.asked.as_ref() != Some(&requested) && now - state.query_changed > 0.3 {
        state.feed = Feed::default();
        state.shown_key = None;
        state.asked = Some(requested);
        state.fetch(&ctx, server, false);
    }
    if state.asked.as_ref().is_some_and(|(_, q)| q != state.query.trim()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(320));
    }
    let head = Rect::from_min_size(pos2(page.left() + 32.0, page.top() + cx.top + 8.0), vec2(page.width() - 64.0, 72.0));
    let narrow = head.width() < 690.0;
    let controls = Rect::from_min_size(pos2(head.left(), head.bottom() + 12.0), vec2(head.width(), if narrow { 88.0 } else { 40.0 }));
    let top = controls.bottom() + 24.0;
    let q = state.query.trim().to_lowercase();
    let shown_key = (state.filter, q.clone());
    let entries = if state.shown_key.as_ref() == Some(&shown_key) {
        std::mem::take(&mut state.shown)
    } else {
        state.shown_key = Some(shown_key);
        let mut entries: Vec<Entry> =
            state.locals.iter().filter(|_| state.filter != Filter::Shared).take(if state.fixture { 1 } else { usize::MAX }).cloned().collect();
        for item in &state.feed.items {
            if state.filter == Filter::Mine && !item.mine || state.filter == Filter::Shared && item.mine {
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
        entries.sort_by_key(|e| std::cmp::Reverse(e.taken));
        entries
    };
    let from = orbiters_ui::blur::next_shape(ui);
    let mut gu = ui.new_child(UiBuilder::new().max_rect(page).id_salt("photos-captures-grid"));
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("photos-captures")
        .auto_shrink([false, false])
        .scroll_bar_rect(Rect::from_min_max(pos2(page.left(), top), page.max))
        .show(&mut gu, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 32 }).show(ui, |ui| {
                ui.add_space(top - page.top());
                let width = ui.available_width();
                if entries.is_empty() {
                    ui.add_space(70.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(if state.local_job.running() || state.feed_job.running() {
                                "Gathering your pictures…"
                            } else if !q.is_empty() {
                                "No worlds match this search"
                            } else if state.filter == Filter::Shared {
                                "Nothing shared with you yet"
                            } else {
                                "Your moments belong here"
                            })
                            .size(21.0)
                            .strong(),
                        );
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("VRChat pictures and N4N0 captures appear together here.").color(theme::FG_3));
                    });
                }
                let groups = groups(&entries);
                for indices in groups {
                    let first = &entries[indices[0]];
                    let last = &entries[*indices.last().unwrap()];
                    let (r, _) = ui.allocate_exact_size(vec2(width, 48.0), Sense::hover());
                    widgets::text(
                        ui.painter(),
                        pos2(r.left(), r.top() + 11.0),
                        Align2::LEFT_CENTER,
                        &Txt::new(&first.world_name, 16.0).weight(weight::SEMIBOLD),
                    );
                    let date = super::local_time(&controller::iso(first.taken));
                    let end = super::local_time(&controller::iso(last.taken));
                    let span = if first.taken == last.taken { date } else { format!("{end} – {date}") };
                    widgets::text_truncated(
                        ui.painter(),
                        pos2(r.left(), r.top() + 33.0),
                        Align2::LEFT_CENTER,
                        &Txt::new(span, 11.5).color(theme::FG_3),
                        width - 160.0,
                    );
                    let b = Rect::from_min_size(pos2(r.right() - 132.0, r.top() + 2.0), vec2(132.0, 36.0));
                    if sharing::pressed(
                        Button::new("Instance info")
                            .icon(icons::INFO)
                            .variant(Variant::Ghost)
                            .id_salt((&first.key, "instance"))
                            .width(b.width())
                            .show_at(ui, b),
                    ) {
                        state.instance = Some(first.clone());
                        let location = first.instance.as_ref().map(|i| format!("{}:{i}", first.world_id));
                        let taken = first.taken;
                        state.roster_job =
                            Job::spawn(&ctx, move || location.map(|l| orbiters_core::vrchat_accounts::instance_people(&l, taken)).unwrap_or_default());
                    }
                    ui.add_space(8.0);
                    let cols = (((width + 16.0) / 286.0).floor() as usize).max(1);
                    let w = (width - 16.0 * (cols - 1) as f32) / cols as f32;
                    let h = w * 9.0 / 16.0 + 96.0;
                    let rows = indices.len().div_ceil(cols);
                    let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * (h + 16.0) - 16.0), Sense::hover());
                    for (n, i) in indices.iter().enumerate() {
                        let e = &entries[*i];
                        let r =
                            Rect::from_min_size(pos2(area.left() + (n % cols) as f32 * (w + 16.0), area.top() + (n / cols) as f32 * (h + 16.0)), vec2(w, h));
                        if !ui.is_rect_visible(r) {
                            continue;
                        }
                        let response = ui.interact(r, ui.scope_id().with(&e.key), Sense::click());
                        let k = anim::spring(&ctx, response.id.with("hover"), [if response.hovered() { 1.0 } else { 0.0 }], Spring::SNAPPY)[0];
                        let p = ui.painter();
                        widgets::frame(p, r, 18.0, theme::mix(theme::surface(), theme::INK_800, k));
                        let cover = Rect::from_min_size(r.min, vec2(w, w * 9.0 / 16.0));
                        let tex = e.picture.as_ref().and_then(|p| state.pictures.as_mut()?.get(&ctx, p));
                        super::sessions::thumb_shape(p, cover, theme::radius(16.0), tex.as_ref(), 1.0);
                        if tex.is_none() {
                            widgets::icon(p, cover.center(), if e.is_picture() { icons::IMAGES } else { icons::CUBE }, 34.0, theme::FG_4);
                        }
                        let title = if e.is_picture() {
                            "VRChat picture"
                        } else if e.ready {
                            "3D capture"
                        } else {
                            "Recording · reconstruct to share"
                        };
                        widgets::text_truncated(
                            p,
                            pos2(r.left() + 16.0, cover.bottom() + 22.0),
                            Align2::LEFT_CENTER,
                            &Txt::new(title, 14.0).weight(weight::SEMIBOLD),
                            w - 32.0,
                        );
                        let visibility = e
                            .remote
                            .as_ref()
                            .map(|i| {
                                if !i.mine {
                                    "Shared with you"
                                } else if i.public {
                                    "Public"
                                } else if !i.grants.is_empty() {
                                    "Shared privately"
                                } else {
                                    "Private"
                                }
                            })
                            .unwrap_or("On this computer");
                        widgets::text(p, pos2(r.left() + 16.0, cover.bottom() + 43.0), Align2::LEFT_CENTER, &Txt::new(visibility, 11.5).color(theme::FG_3));
                        if let Some(from) = e.remote.as_ref().and_then(|i| i.from.as_ref()) {
                            let ar = Rect::from_min_size(pos2(r.left() + 16.0, r.bottom() - 30.0), vec2(20.0, 20.0));
                            let tex = if state.fixture {
                                e.picture.as_ref().and_then(|p| state.pictures.as_mut()?.get(&ctx, p))
                            } else {
                                from.avatar_url.as_ref().and_then(|url| {
                                    state
                                        .pictures
                                        .as_mut()?
                                        .get(&ctx, &crate::worlds::Picture::Remote { world_id: format!("person-{}", from.id), url: url.clone(), large: false })
                                })
                            };
                            sharing::avatar(ui, ar, tex.as_ref(), &from.name);
                            widgets::text(
                                p,
                                pos2(ar.right() + 7.0, ar.center().y),
                                Align2::LEFT_CENTER,
                                &Txt::new(format!("from {}", from.name), 11.5).color(theme::FG_2),
                            );
                        }
                        if sharing::pressed(response) {
                            clicked = Some(e.clone());
                        }
                    }
                    ui.add_space(28.0);
                }
                if state.feed.next.is_some() {
                    if sharing::pressed(
                        Button::new(if state.feed_job.running() { "Loading…" } else { "Load more" }).enabled(!state.feed_job.running()).show(ui),
                    ) {
                        state.fetch(&ctx, server, true);
                    }
                }
                if let Some(error) = state.error.clone() {
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(error).color(theme::DANGER));
                    if sharing::pressed(Button::new("Retry").enabled(!state.feed_job.running()).show(ui)) {
                        state.fetch(&ctx, server, false);
                    }
                }
            });
        });
    super::under_top(&gu, cx, "photos-captures", from, top);
    super::heading(
        ui,
        head,
        "Library",
        "Photos & captures",
        Some(if narrow { "Pictures, captures and shared moments" } else { "Your pictures, 3D captures and moments shared by friends" }),
    );
    let mut manage = false;
    let b = Rect::from_min_size(pos2(head.right() - 172.0, head.top() + 25.0), vec2(172.0, 40.0));
    if sharing::pressed(Button::new("Manage captures").icon(icons::FOLDER_OPEN).width(b.width()).show_at(ui, b)) {
        manage = true;
    }
    let mut cu = ui.new_child(UiBuilder::new().max_rect(controls).id_salt("gallery-controls"));
    widgets::segmented(
        &mut cu,
        "gallery-filter",
        &mut state.filter,
        &[SegOption::new(Filter::Mine, "Mine"), SegOption::new(Filter::All, "All"), SegOption::new(Filter::Shared, "Shared with me")],
        true,
    );
    let search = if narrow {
        Rect::from_min_size(pos2(controls.left(), controls.top() + 48.0), vec2(controls.width(), 40.0))
    } else {
        Rect::from_min_size(pos2(controls.right() - 280.0, controls.top()), vec2(280.0, 40.0))
    };
    let request_button = Rect::from_min_size(pos2(if narrow { controls.right() - 150.0 } else { search.left() - 162.0 }, controls.top()), vec2(150.0, 40.0));
    if sharing::pressed(
        Button::new(&format!("Requests ({})", state.requests.pending())).icon(icons::CLOCK).width(request_button.width()).show_at(ui, request_button),
    ) {
        state.requests.open = true;
    }
    let mut su = ui.new_child(UiBuilder::new().max_rect(search).id_salt("gallery-search"));
    let before = state.query.clone();
    widgets::search_field(&mut su, "gallery-search", &mut state.query, "Search worlds", search.width());
    if state.query != before {
        state.query_changed = now;
    }
    if let Some(item) = state.requests.show(ui, cx) {
        state.dialog = Some(Dialog::new(&ctx, controller::remote_entry(item), false));
    }
    if let Some(mut e) = clicked {
        if let Some(item) = e.remote.as_ref() {
            if item.picture.is_some() && !state.fixture {
                e.picture = Some(crate::worlds::Picture::Capture { development, id: item.id, large: true });
            }
        }
        state.dialog = Some(Dialog::new(&ctx, e, false));
    }
    if let Some(d) = state.dialog.as_mut() {
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
    if let Some(people) = state.roster_job.take() {
        if let Some(e) = state.instance.as_mut() {
            e.people = people;
        }
    }
    if let Some(e) = state.instance.clone() {
        let response = egui::Modal::new(Id::unique("capture-instance-info"))
            .frame(egui::Frame::NONE.fill(theme::INK_900).corner_radius(20).inner_margin(24))
            .show(&ctx, |ui| {
                ui.set_width(460.0);
                ui.heading(&e.world_name);
                ui.add_space(12.0);
                ui.label(e.instance.as_deref().unwrap_or("Instance was not recorded for this item"));
                if let Some(instance) = &e.instance {
                    ui.label(instance_type(instance));
                }
                ui.add_space(12.0);
                let current = cx.snap.vrchat.running && is_current(&cx.snap.vrchat.world, &e);
                let people = if current { cx.snap.vrchat.world.players.values().cloned().collect::<Vec<_>>() } else { e.people.clone() };
                ui.label(if people.is_empty() { "Who was there was not recorded".into() } else { people.join(", ") });
                ui.add_space(16.0);
                if current {
                    let instance = e.instance.as_deref().unwrap();
                    let link = format!("https://vrchat.com/home/launch?worldId={}&instanceId={}", e.world_id, encode(instance));
                    if sharing::pressed(Button::new("Open in VRChat").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).show(ui)) {
                        let _ = crate::controller::open_url(&link);
                    }
                } else {
                    ui.label(egui::RichText::new("Open in VRChat is available only for your current instance.").color(theme::FG_3));
                }
                if Button::new("Done").show(ui).clicked() {
                    state.instance = None;
                }
            });
        if response.should_close() {
            state.instance = None;
        }
    }
    state.shown = entries;
    manage
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
fn instance_type(instance: &str) -> &str {
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
fn encode(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
