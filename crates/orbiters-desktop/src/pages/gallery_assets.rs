//! Photos & captures' My assets: the showcase galleries of the Orbiters assets the member has ([`orbiters_core::assets`]:
//! made, bought, licensed), the pictures each asset's page shows from the Discord rooms its creator chose, one group per
//! asset (its name, what it is to the member, how many, Open on Orbiters). Not the assets' own pictures (Enzo,
//! 2026-10-30). All shows them among the member's own. Cards are the gallery's own (selection, dragging out, their
//! menu, Edit); a picture opens in the community picture window ([`super::super::community::PictureWindow`]: who posted
//! it, its words), ←/→ through the showcase pictures shown. Discord's links expire: they are renewed at the asset's
//! `/showcase-images/:id/source` (the picture knows its asset).
//!
//! Entries are keyed `asset:<asset id>:<n>` (the `n`th of its showcase), their group `asset:<asset id>`.

use super::super::community::{self as community_page, About};
use super::super::{sharing, Cx, Job};
use crate::controller::shares::{self as controller, Entry};
use crate::worlds::Picture;
use egui::{pos2, vec2, Align2, Rect, Ui};
use orbiters_core::account::{Api, Server};
use orbiters_core::assets::{self, How, Owned};
use orbiters_core::community;
use orbiters_i18n::{mark, tr, tr_n};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, IconButton};

const KEY: &str = "asset:";

pub fn owns(key: &str) -> bool {
    key.starts_with(KEY)
}

/// The asset and the picture of a key (`asset:7:2`), or of a group (`asset:7`).
fn parts(key: &str) -> Option<(u64, Option<usize>)> {
    let rest = key.strip_prefix(KEY)?;
    match rest.split_once(':') {
        Some((id, n)) => Some((id.parse().ok()?, Some(n.parse().ok()?))),
        None => Some((rest.parse().ok()?, None)),
    }
}

/// "Made by you", "Yours on Orbiters".
fn how(o: &Owned) -> &'static str {
    match o.asset.how {
        How::Made => tr("Made by you"),
        How::Licensed => tr("Yours on Orbiters"),
    }
}

fn ask(ctx: &egui::Context, server: Server) -> Job<anyhow::Result<Vec<Owned>>> {
    Job::spawn(ctx, move || {
        let Some(account) = orbiters_core::account::read(server) else { return Ok(Vec::new()) };
        Api::new(server).owned_assets(&account.token)
    })
}

#[derive(Default)]
pub struct State {
    assets: Vec<Owned>,
    job: Job<anyhow::Result<Vec<Owned>>>,
    /// What was asked: the environment and the account.
    asked: Option<(bool, String)>,
    account: crate::pages::AccountFollow,
    /// Orbiters came to answer for the account since the last look: what failed is asked again.
    answered: bool,
    error: Option<String>,
    changed: bool,
    stand_ins: Vec<Picture>,
    fixture: bool,
}

impl State {
    pub fn busy(&self) -> bool {
        self.job.running()
    }

    pub fn loading(&self) -> bool {
        self.job.running()
    }

    /// How many showcase pictures the assets have.
    pub fn count(&self) -> usize {
        self.assets.iter().map(|a| a.pictures.len()).sum()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn retry(&mut self) {
        self.asked = None;
    }

    /// Screenshots: two avatars' showcases (one the member made, one they hold a licence to), this computer's pictures
    /// standing in for the Discord rooms' pictures, and an asset without showcase pictures (no group).
    pub fn stage(&mut self, sessions_dir: &std::path::Path) {
        self.stage_assets();
        self.stand_ins = super::super::events::Covers::stand_ins(sessions_dir);
    }

    /// [`State::stage`]'s assets alone, with the real links: no folder of this computer is read (the stand-ins index
    /// every picture folder of the settings, an archive of thousands of pictures on an external drive among them,
    /// which kept the test below reading for over half an hour).
    fn stage_assets(&mut self) {
        self.fixture = true;
        self.job = Job::default();
        let made = assets::Asset { id: 41, name: "Lumen Fox".into(), kind: "AVATAR".into(), how: How::Made, ..Default::default() };
        let bought = assets::Asset { id: 77, name: "Neon Racer".into(), kind: "AVATAR".into(), how: How::Licensed, ..Default::default() };
        let quiet = assets::Asset { id: 90, name: "Plain Hat".into(), kind: "ACCESSORY".into(), how: How::Licensed, ..Default::default() };
        let shot = |id: u64, at: &str, author: &str, words: &str| community::Picture {
            id,
            src: Some(format!("https://media.discordapp.net/attachments/1/{id}/showcase.png")),
            full_url: Some(format!("https://cdn.discordapp.com/attachments/1/{id}/showcase.png")),
            width: 1920,
            height: 1080,
            posted_at: Some(at.into()),
            reaction_count: 4,
            author: Some(community::Member { id: 1, username: author.into(), ..Default::default() }),
            message: (!words.is_empty()).then(|| community::Message { content: words.into(), ..Default::default() }),
            ..Default::default()
        };
        let showcase = |id: u64| -> anyhow::Result<Vec<community::Picture>> {
            Ok(match id {
                41 => vec![
                    shot(5101, "2026-10-29T18:00:00Z", "Mika", "Fox at the lake"),
                    shot(5102, "2026-10-28T21:30:00Z", "Ren", ""),
                    shot(5103, "2026-10-27T12:00:00Z", "Mika", "Night ride"),
                ],
                77 => vec![shot(7701, "2026-10-26T20:00:00Z", "Sol", "Racing"), shot(7702, "2026-10-25T19:00:00Z", "Ivy", "")],
                _ => Vec::new(),
            })
        };
        self.assets = assets::gather(vec![made, bought, quiet], showcase).unwrap_or_default();
        self.stand_ins.clear();
        self.changed = true;
    }

    /// Reads the member's assets' showcases when Photos & captures shows them (`wanted`), again when the account or
    /// the environment change. True when what it shows changed.
    pub fn poll(&mut self, ctx: &egui::Context, cx: &Cx, wanted: bool) -> bool {
        let development = cx.snap.settings.development;
        let server = if development { Server::Development } else { Server::Production };
        // Who the account is, not how its check goes: offline, the check comes and goes every minute.
        let (account, answered) = self.account.see(&cx.snap.account.state);
        self.answered |= answered;
        if !self.fixture && wanted {
            let key = (development, account);
            if self.asked.as_ref() != Some(&key) {
                self.asked = Some(key);
                self.answered = false;
                self.error = None;
                self.assets.clear();
                self.changed = true;
                self.job = ask(ctx, server);
            } else if std::mem::take(&mut self.answered) && (self.assets.is_empty() || self.error.is_some()) && !self.job.running() {
                // Orbiters answers again: what could not be read is asked again; what shows stays.
                self.error = None;
                self.job = ask(ctx, server);
            }
        }
        if let Some(r) = self.job.take() {
            match r {
                Ok(a) => self.assets = a,
                Err(e) => self.error = Some(e.to_string()),
            }
            self.changed = true;
        }
        std::mem::take(&mut self.changed)
    }

    fn get(&self, id: u64) -> Option<&Owned> {
        self.assets.iter().find(|a| a.asset.id == id)
    }

    fn of(&self, key: &str) -> Option<&Owned> {
        self.get(parts(key)?.0)
    }

    /// The showcase picture of a key.
    fn picture(&self, key: &str) -> Option<&community::Picture> {
        let (id, n) = parts(key)?;
        self.get(id)?.pictures.get(n?)
    }

    /// The assets' showcase pictures as the gallery's entries, those `q` finds by the asset's name or
    /// kind, who posted it or its words. Each asset's group sits at its newest picture's time, its pictures in its
    /// page's order.
    pub fn entries(&self, q: &orbiters_core::search::Query, development: bool) -> Vec<Entry> {
        let mut out = Vec::new();
        for a in &self.assets {
            let kind = assets::kind_label(&a.asset.kind);
            let asset_found = q.matches_any([a.asset.name.as_str(), kind, tr(kind)]);
            let taken = controller::unix(a.date());
            for (n, p) in a.pictures.iter().enumerate() {
                let author = p.author.as_ref().map(|m| m.username.clone()).unwrap_or_default();
                if !asset_found {
                    let words = p.message.as_ref().map(|m| m.content.as_str()).unwrap_or_default();
                    if !(q.matches(&author) || q.in_text(words)) {
                        continue;
                    }
                }
                let picture = if self.stand_ins.is_empty() {
                    community_page::source(p, false, development, &[])
                } else {
                    Some(self.stand_ins[(a.asset.id as usize + n) % self.stand_ins.len()].clone())
                };
                out.push(Entry {
                    key: format!("{KEY}{}:{n}", a.asset.id),
                    world_id: format!("{KEY}{}", a.asset.id),
                    world_name: a.asset.name.clone(),
                    taken,
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
                    picture,
                    size: (p.width > 0 && p.height > 0).then_some([p.width, p.height]),
                    layers: Vec::new(),
                    edits: None,
                    uploaded_edits: None,
                });
            }
        }
        out
    }

    /// What a card of it says: who posted it in the asset's showcase, and what the asset is to the member.
    pub fn labels(&self, e: &Entry) -> Option<(String, String)> {
        let a = self.of(&e.key)?;
        let by = self.picture(&e.key).and_then(|p| p.author.as_ref()).map(|m| m.username.clone()).filter(|n| !n.is_empty());
        Some((by.map_or_else(|| tr("Showcase picture").to_string(), |b| tr!("by {name}", name = b)), how(a).to_string()))
    }

    /// Its page on the website.
    pub fn page(&self, key: &str, server: Server) -> Option<String> {
        Some(format!("{}{}", server.website(), self.of(key)?.page()))
    }

    /// The asset group's heading: its name, what it is to the member, how many showcase pictures and when the latest
    /// was posted, and Open on Orbiters (its page, at its showcase).
    pub fn group_header(&self, ui: &mut Ui, cx: &mut Cx, first: &Entry, count: usize) {
        let Some(a) = self.of(&first.key) else { return };
        let r = ui.max_rect();
        let p = ui.painter().clone();
        let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
        widgets::text_truncated(
            &p,
            pos2(r.left(), r.top() + 12.0),
            Align2::LEFT_CENTER,
            &Txt::new(&a.asset.name, 18.0).weight(weight::SEMIBOLD).tracking(-0.01),
            r.width() - 56.0,
        );
        let y = r.top() + 36.0;
        let t = Txt::new(format!("{} · {}", tr(assets::kind_label(&a.asset.kind)), how(a)), 11.0).weight(weight::SEMIBOLD).color(theme::FG_2);
        let tw = widgets::text_size(ui, &t).x;
        let pill = Rect::from_min_size(pos2(r.left(), y - 10.0), vec2(tw + 30.0, 20.0));
        p.rect_filled(pill, theme::radius(10.0), theme::white(0.06));
        widgets::icon(&p, pos2(pill.left() + 11.0, pill.center().y), icons::TAG, 11.0, theme::ACCENT);
        widgets::text(&p, pos2(pill.left() + 21.0, pill.center().y), Align2::LEFT_CENTER, &t);
        let when = super::super::local_time(a.date());
        let items = tr_n("{count} showcase picture", "{count} showcase pictures", count as u64);
        let b = IconButton::new(icons::ARROW_SQUARE_OUT, mark("Open on Orbiters")).size(32.0, 15.0).backdrop(theme::control()).id_salt(("asset-site", a.asset.id));
        let br = Rect::from_min_size(pos2(r.right() - 32.0, r.top() + 6.0), vec2(32.0, 32.0));
        let line = if when.is_empty() { items } else { tr!("{items} · latest {when}", items = items, when = when) };
        widgets::text_truncated(&p, pos2(pill.right() + 10.0, y), Align2::LEFT_CENTER, &Txt::new(line, 12.0).color(theme::FG_3), br.left() - pill.right() - 26.0);
        if sharing::pressed(b.show_at(ui, br)) {
            let link = format!("{}{}#gallery", server.website(), a.page());
            let result = crate::controller::open_url(&link);
            cx.run(ui.ctx(), result);
        }
    }

    /// The pictures of `keys` (the assets' cards shown, in the page's order) for the picture window, whose they are,
    /// and where `key` is among them. Screenshots: the stand-ins.
    pub fn window(&self, keys: &[String], key: &str, development: bool) -> Option<(Vec<community::Picture>, Vec<About>, usize, Vec<Picture>)> {
        let server = if development { Server::Development } else { Server::Production };
        let mut list = Vec::new();
        let mut about = Vec::new();
        for k in keys {
            let (Some(a), Some(p)) = (self.of(k), self.picture(k)) else { continue };
            list.push(p.clone());
            let line = tr!("{kind} showcase · {how}", kind = tr(assets::kind_label(&a.asset.kind)), how = how(a));
            about.push(About {
                key: k.clone(),
                title: a.asset.name.clone(),
                line,
                site: format!("{}{}#gallery", server.website(), a.page()),
            });
        }
        let index = about.iter().position(|a| a.key == key)?;
        Some((list, about, index, self.stand_ins.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::gallery_select::{remote, Remote};
    use super::*;

    #[test]
    fn each_asset_is_a_group_of_its_showcase_pictures() {
        let mut state = State::default();
        // The real links, not this computer's pictures standing in for them.
        state.stage_assets();
        let entries = state.entries(&orbiters_core::search::Query::new(""), false);
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["asset:41:0", "asset:41:1", "asset:41:2", "asset:77:0", "asset:77:1"], "an asset without showcase pictures has no group");
        assert!(entries.iter().all(|e| e.local.is_none() && e.remote.is_none() && owns(&e.key)));
        assert_eq!(entries[0].world_id, "asset:41");
        assert!(entries[..3].iter().all(|e| e.taken == entries[0].taken), "one group, at its newest picture's time");
        // A showcase picture, its link renewed at its asset's route: never the asset's own pictures.
        assert!(matches!(&entries[3].picture, Some(Picture::Gallery { asset: 77, id: 7701, url, .. }) if url.contains("/7701/")));
        assert!(entries.iter().all(|e| !matches!(&e.picture, Some(Picture::Gallery { url, .. }) if url.contains("creator-tools/media"))));
        assert_eq!(entries[1].people, ["Ren"]);
        assert_eq!(state.labels(&entries[0]), Some(("by Mika".into(), "Made by you".into())));
        assert_eq!(state.labels(&entries[4]).unwrap().1, "Yours on Orbiters");
        assert_eq!(state.page("asset:77:1", Server::Production).as_deref(), Some("https://orbiters.cc/assets/77"));
        assert_eq!(state.entries(&orbiters_core::search::Query::new("racer"), false).len(), 2, "found by the asset's name");
        assert_eq!(state.entries(&orbiters_core::search::Query::new("avatar"), false).len(), 5, "and by its kind");
        assert_eq!(state.entries(&orbiters_core::search::Query::new("mika"), false).len(), 2, "or by who posted it");
        assert_eq!(state.entries(&orbiters_core::search::Query::new("night ride"), false).len(), 1, "or by its words");
        let shown: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
        let (list, about, index, _) = state.window(&shown, "asset:77:0", false).unwrap();
        assert_eq!((list.len(), index, about[index].title.as_str()), (5, 3, "Neon Racer"));
        assert_eq!((list[index].id, list[index].asset_id), (7701, 77));
        assert_eq!(about[index].site, "https://orbiters.cc/assets/77#gallery");
        // Dragged out or edited, its file comes by its link renewed at the asset's showcase.
        let url = "https://media.discordapp.net/attachments/1/7701/showcase.png".to_string();
        assert_eq!(remote(&entries[3], false), Some(Remote::Gallery { server: Server::Production, asset: 77, id: 7701, url }));
    }
}
