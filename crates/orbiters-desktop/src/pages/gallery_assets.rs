//! Photos & captures' My assets: the Orbiters assets the member made or holds a licence to ([`orbiters_core::assets`])
//! with their pictures, one group per asset (its name, what it is to the member, how many, Open on Orbiters); All
//! shows them among the member's own. Cards are the gallery's own (selection, dragging out, their menu); a picture
//! opens in the community picture window ([`super::super::community::PictureWindow`], the capture window's look and motion), ←/→
//! through the assets' pictures shown.
//!
//! Entries are keyed `asset:<asset id>:<n>` (the `n`th of its pictures), their group `asset:<asset id>`.

use super::super::community::About;
use super::super::{sharing, Cx, Job};
use crate::controller::shares::{self as controller, Entry};
use crate::worlds::Picture;
use egui::{pos2, vec2, Align2, Rect, Ui};
use orbiters_core::account::{Api, Server};
use orbiters_core::assets::{self, How, Owned};
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

/// "Made by you", "Bought on Orbiters".
fn how(o: &Owned) -> &'static str {
    match o.how {
        How::Made => "Made by you",
        How::Licensed => "Yours on Orbiters",
    }
}

#[derive(Default)]
pub struct State {
    assets: Vec<Owned>,
    job: Job<anyhow::Result<Vec<Owned>>>,
    /// What was asked: the environment and the account.
    asked: Option<(bool, String)>,
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

    /// How many pictures the assets have.
    pub fn count(&self) -> usize {
        self.assets.iter().map(|a| a.pictures.len()).sum()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn retry(&mut self) {
        self.asked = None;
    }

    /// Screenshots: two avatars (one the member made, one they hold a licence to), this computer's pictures as theirs.
    pub fn stage(&mut self, sessions_dir: &std::path::Path) {
        self.fixture = true;
        self.job = Job::default();
        let api = Api::new(Server::Production);
        let made = assets::Asset {
            id: 41,
            name: "Lumen Fox".into(),
            kind: "AVATAR".into(),
            product: Some(assets::Product { gallery_file_ids: vec![501.into(), 502.into(), 503.into()] }),
            updated_at: Some("2026-10-29T18:00:00Z".into()),
            ..Default::default()
        };
        let bought = assets::Asset {
            id: 77,
            name: "Neon Racer".into(),
            kind: "AVATAR".into(),
            product: Some(assets::Product { gallery_file_ids: vec![601.into(), 602.into()] }),
            updated_at: Some("2026-10-27T12:00:00Z".into()),
            ..Default::default()
        };
        let licensed = vec![assets::Licensed { id: 77, name: bought.name.clone(), ..Default::default() }];
        self.assets = assets::gather(&api, vec![made], licensed, |_| Some(bought.clone()));
        self.stand_ins = super::super::events::Covers::stand_ins(sessions_dir);
        self.changed = true;
    }

    /// Reads the member's assets when Photos & captures shows them (`wanted`), again when the account or the
    /// environment change. True when what it shows changed.
    pub fn poll(&mut self, ctx: &egui::Context, cx: &Cx, wanted: bool) -> bool {
        let development = cx.snap.settings.development;
        let server = if development { Server::Development } else { Server::Production };
        if !self.fixture && wanted {
            let key = (development, format!("{:?}", cx.snap.account.state));
            if self.asked.as_ref() != Some(&key) {
                self.asked = Some(key);
                self.error = None;
                self.assets.clear();
                self.changed = true;
                self.job = Job::spawn(ctx, move || {
                    let Some(account) = orbiters_core::account::read(server) else { return Ok(Vec::new()) };
                    Api::new(server).owned_assets(&account.token)
                });
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

    /// The assets' pictures as the gallery's entries, those `q` (lowercase) finds by the asset's name or kind.
    pub fn entries(&self, q: &str, development: bool) -> Vec<Entry> {
        let mut out = Vec::new();
        for a in &self.assets {
            let kind = assets::kind_label(&a.asset.kind);
            if !q.is_empty() && ![a.asset.name.to_lowercase(), kind.to_lowercase()].iter().any(|s| s.contains(q)) {
                continue;
            }
            for (n, url) in a.pictures.iter().enumerate() {
                let picture = if self.stand_ins.is_empty() {
                    Picture::Gallery { development, id: 0, url: url.clone(), large: false }
                } else {
                    self.stand_ins[(a.asset.id as usize + n) % self.stand_ins.len()].clone()
                };
                out.push(Entry {
                    key: format!("{KEY}{}:{n}", a.asset.id),
                    world_id: format!("{KEY}{}", a.asset.id),
                    world_name: a.asset.name.clone(),
                    taken: controller::unix(a.date()),
                    instance: None,
                    source_key: None,
                    people: Vec::new(),
                    local: None,
                    remote: None,
                    content: Default::default(),
                    ready: true,
                    picture: Some(picture),
                    size: None,
                    layers: Vec::new(),
                });
            }
        }
        out
    }

    /// What a card of it says: its kind ("Avatar picture") and what it is to the member.
    pub fn labels(&self, e: &Entry) -> Option<(String, String)> {
        let a = self.of(&e.key)?;
        Some((format!("{} picture", assets::kind_label(&a.asset.kind)), how(a).to_string()))
    }

    /// Its page on the website.
    pub fn page(&self, key: &str, server: Server) -> Option<String> {
        Some(format!("{}{}", server.website(), self.of(key)?.page()))
    }

    /// The picture's link, whole.
    pub fn link(&self, key: &str) -> Option<String> {
        let (id, n) = parts(key)?;
        self.get(id)?.pictures.get(n?).cloned()
    }

    /// The asset group's heading: its name, what it is to the member, when it last changed and how many pictures, and
    /// Open on Orbiters.
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
        let t = Txt::new(format!("{} · {}", assets::kind_label(&a.asset.kind), how(a)), 11.0).weight(weight::SEMIBOLD).color(theme::FG_2);
        let tw = widgets::text_size(ui, &t).x;
        let pill = Rect::from_min_size(pos2(r.left(), y - 10.0), vec2(tw + 30.0, 20.0));
        p.rect_filled(pill, theme::radius(10.0), theme::white(0.06));
        widgets::icon(&p, pos2(pill.left() + 11.0, pill.center().y), icons::TAG, 11.0, theme::ACCENT);
        widgets::text(&p, pos2(pill.left() + 21.0, pill.center().y), Align2::LEFT_CENTER, &t);
        let when = super::super::local_time(a.date());
        let items = format!("{count} picture{}", if count == 1 { "" } else { "s" });
        let b = IconButton::new(icons::ARROW_SQUARE_OUT, "Open on Orbiters").size(32.0, 15.0).backdrop(theme::control()).id_salt(("asset-site", a.asset.id));
        let br = Rect::from_min_size(pos2(r.right() - 32.0, r.top() + 6.0), vec2(32.0, 32.0));
        let line = if when.is_empty() { items } else { format!("{items} · updated {when}") };
        widgets::text_truncated(&p, pos2(pill.right() + 10.0, y), Align2::LEFT_CENTER, &Txt::new(line, 12.0).color(theme::FG_3), br.left() - pill.right() - 26.0);
        if sharing::pressed(b.show_at(ui, br)) {
            let link = format!("{}{}", server.website(), a.page());
            let result = crate::controller::open_url(&link);
            cx.run(ui.ctx(), result);
        }
    }

    /// The pictures of `keys` (the assets' cards shown, in the page's order) for the picture window, whose they are,
    /// and where `key` is among them. Screenshots: the stand-ins, numbered so each shows its own.
    pub fn window(&self, keys: &[String], key: &str, development: bool) -> Option<(Vec<orbiters_core::community::Picture>, Vec<About>, usize, Vec<Picture>)> {
        let server = if development { Server::Development } else { Server::Production };
        let mut list = Vec::new();
        let mut about = Vec::new();
        for k in keys {
            let (Some(a), Some(url)) = (self.of(k), self.link(k)) else { continue };
            let n = list.len() as u64;
            list.push(orbiters_core::community::Picture {
                id: if self.stand_ins.is_empty() { 0 } else { n },
                src: Some(url.clone()),
                full_url: Some(url),
                posted_at: a.asset.updated_at.clone(),
                ..Default::default()
            });
            about.push(About {
                key: k.clone(),
                title: a.asset.name.clone(),
                line: format!("{} · {}", assets::kind_label(&a.asset.kind), how(a)),
                site: format!("{}{}", server.website(), a.page()),
            });
        }
        let index = about.iter().position(|a| a.key == key)?;
        Some((list, about, index, self.stand_ins.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_asset_is_a_group_of_its_pictures() {
        let mut state = State::default();
        state.stage(std::path::Path::new("no such folder"));
        // The real links, not this computer's pictures standing in for them.
        state.stand_ins.clear();
        let entries = state.entries("", false);
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["asset:41:0", "asset:41:1", "asset:41:2", "asset:77:0", "asset:77:1"]);
        assert!(entries.iter().all(|e| e.local.is_none() && e.remote.is_none() && owns(&e.key)));
        assert_eq!(entries[0].world_id, "asset:41");
        assert!(matches!(&entries[3].picture, Some(Picture::Gallery { id: 0, url, .. }) if url == "https://api.orbiters.cc/creator-tools/media/601"));
        assert_eq!(state.labels(&entries[0]), Some(("Avatar picture".into(), "Made by you".into())));
        assert_eq!(state.labels(&entries[4]).unwrap().1, "Yours on Orbiters");
        assert_eq!(state.page("asset:77:1", Server::Production).as_deref(), Some("https://orbiters.cc/assets/77"));
        assert_eq!(state.entries("racer", false).len(), 2, "found by name");
        assert_eq!(state.entries("avatar", false).len(), 5, "and by kind");
        let shown: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
        let (list, about, index, _) = state.window(&shown, "asset:77:0", false).unwrap();
        assert_eq!((list.len(), index, about[index].title.as_str()), (5, 3, "Neon Racer"));
        assert_eq!(list[index].full_url.as_deref(), Some("https://api.orbiters.cc/creator-tools/media/601"));
    }
}
