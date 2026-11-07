//! What people said about a community gallery's picture, in its picture window ([`super::community::PictureWindow`]),
//! as the website's picture window shows it (`GalleryImageDetails`, `galleryReplies/GalleryReplies.jsx`): a pill for
//! each emoji people reacted with (its count; hovered, who reacted, Discord's "Alex, Mika and Robin reacted with
//! :orbit:"), then the replies to the Discord message the picture was posted with (who, their picture, when, their
//! words in a bubble, their stickers and files), "N more on Discord" past the 200 listed and the message on Discord.
//!
//! Both are read from Orbiters (`orbiters_core::gallery_talk`) once per picture shown, on a worker, only when the
//! listing counts any; while they come the pills are the counts and the replies placeholder rows (as many as the count
//! says, at most three, so the window grows once); a failure says so with Try again. An asset's showcase picture has
//! neither (its placements are not the galleries').
use super::Job;
use crate::worlds::{Picture, Pictures};
use egui::{pos2, vec2, Align2, CornerRadius, CursorIcon, Id, Order, Rect, Sense, Ui};
use orbiters_core::account::{Api, Server};
use orbiters_core::community;
use orbiters_core::gallery_talk::{Reaction, Reactions, Replies, Reply};
use orbiters_i18n::{mark, tr, tr_n};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};
use orbiters_ui::{feedback, format, icons, paint};
use std::collections::HashMap;

/// Names a reaction's hover lists before "and N others".
pub const NAMES_SHOWN: usize = 60;
/// Discord's colour, for its logo.
const DISCORD: egui::Color32 = egui::Color32::from_rgb(0x58, 0x65, 0xf2);
const PILL_H: f32 = 30.0;
const AVATAR: f32 = 28.0;

/// One of the two reads.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase<T> {
    /// Nothing to read (the listing counts none).
    None,
    Waiting,
    Ready(T),
    Failed(String),
}

/// What was read for a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub replies: Phase<Replies>,
    pub reactions: Phase<Reactions>,
}

/// The reads of the pictures a window showed, by placement id.
#[derive(Default)]
pub struct Talk {
    said: HashMap<u64, Said>,
    jobs: HashMap<u64, Job<Said>>,
    /// Screenshots: what each picture says, never read from Orbiters; and a reaction shown hovered.
    staged: bool,
    pub hover: Option<usize>,
}

/// Whether a picture has replies and reactions to read: a community gallery's, listed with some.
pub fn readable(p: &community::Picture) -> bool {
    p.id != 0 && p.asset_id == 0 && (p.reply_count > 0 || p.reaction_count > 0)
}

impl Talk {
    /// Screenshots: every picture says this (`replies` and `reactions` as Orbiters would answer).
    pub fn stage(&mut self, said: Said) {
        self.staged = true;
        self.jobs.clear();
        self.said.clear();
        self.said.insert(0, said);
    }

    /// What `p` says, asked for the first time it is shown.
    fn of(&mut self, ctx: &egui::Context, p: &community::Picture, development: bool) -> Option<Said> {
        if !readable(p) {
            return None;
        }
        if self.staged {
            return self.said.get(&0).cloned();
        }
        if let Some(job) = self.jobs.get_mut(&p.id) {
            if let Some(said) = job.take() {
                self.jobs.remove(&p.id);
                self.said.insert(p.id, said);
            }
        }
        if let Some(said) = self.said.get(&p.id) {
            return Some(said.clone());
        }
        if !self.jobs.contains_key(&p.id) {
            let (id, replies, reactions) = (p.id, p.reply_count > 0, p.reaction_count > 0);
            let server = if development { Server::Development } else { Server::Production };
            self.jobs.insert(id, Job::spawn(ctx, move || read(server, id, replies, reactions)));
        }
        Some(Said { replies: if p.reply_count > 0 { Phase::Waiting } else { Phase::None }, reactions: if p.reaction_count > 0 { Phase::Waiting } else { Phase::None } })
    }

    /// Reads `p` again (Try again).
    fn retry(&mut self, p: &community::Picture) {
        if !self.staged {
            self.said.remove(&p.id);
        }
    }
}

/// Reads a picture's replies and reactions (those it has) on a worker.
fn read(server: Server, id: u64, replies: bool, reactions: bool) -> Said {
    let Some(account) = orbiters_core::account::read(server) else {
        let signed_out = || mark("Sign in to Orbiters in Settings first.").to_string();
        return Said {
            replies: if replies { Phase::Failed(signed_out()) } else { Phase::None },
            reactions: if reactions { Phase::Failed(signed_out()) } else { Phase::None },
        };
    };
    let api = Api::new(server);
    fn phase<T>(asked: bool, read: impl FnOnce() -> anyhow::Result<T>) -> Phase<T> {
        match asked.then(read) {
            None => Phase::None,
            Some(Ok(v)) => Phase::Ready(v),
            Some(Err(e)) => Phase::Failed(why(&e)),
        }
    }
    Said {
        replies: phase(replies, || api.picture_replies(&account.token, id)),
        reactions: phase(reactions, || api.picture_reactions(&account.token, id)),
    }
}

/// Why a read failed, in words: a server without these routes (or one that does not let this app read them yet)
/// answers 401 or 404 without the picture's own words.
fn why(e: &anyhow::Error) -> String {
    match community::Refused::of(e) {
        Some(r) if r.status == 401 && r.code.is_none() => mark("Orbiters does not let this app read them yet.").into(),
        Some(r) if r.status == 404 && r.message != mark("Image no longer available.") => mark("Orbiters does not have them for this picture yet.").into(),
        Some(r) => r.message.clone(),
        None => e.to_string(),
    }
}

/// A reply as the window lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub avatar: Option<String>,
    pub text: String,
    /// "3 h ago", and the full local time on hover.
    pub when: String,
    pub when_full: String,
    pub edited: bool,
    /// Stickers' and files' names, each a chip under the words.
    pub extras: Vec<(&'static str, String)>,
    /// Their Orbiters profile on the website.
    pub profile: Option<String>,
}

/// The replies as rows, oldest first, at `now` (unix seconds) on `server`'s website.
pub fn rows(replies: &Replies, now: u64, server: Server) -> Vec<Row> {
    replies.items.iter().map(|r| row(r, now, server)).collect()
}

fn row(r: &Reply, now: u64, server: Server) -> Row {
    let posted = r.posted_at.as_deref().unwrap_or_default();
    let at = crate::controller::shares::unix(posted);
    let stickers = r.stickers.iter().filter(|s| !s.name.trim().is_empty()).map(|s| (icons::SMILEY, s.name.clone()));
    let files = r.attachments.iter().filter(|f| !f.name.trim().is_empty()).map(|f| {
        let picture = f.content_type.as_deref().is_some_and(|t| t.starts_with("image/"));
        (if picture { icons::IMAGE } else { icons::LINK_SIMPLE }, f.name.clone())
    });
    Row {
        name: if r.author.name.trim().is_empty() { tr("Discord member").into() } else { r.author.name.trim().to_string() },
        avatar: r.author.avatar().map(str::to_owned),
        text: r.text(),
        when: if at > 0 { format::relative_time(at, now.max(at)) } else { String::new() },
        when_full: if posted.is_empty() { String::new() } else { super::local_time(posted) },
        edited: r.edited_at.is_some(),
        extras: stickers.chain(files).collect(),
        profile: r.author.profile_id.map(|id| format!("{}user/{id}", server.website())),
    }
}

/// "1 reply", "5 replies".
pub fn heading(total: u32) -> String {
    tr_n("{count} reply", "{count} replies", total as u64)
}

/// What a reaction's hover says: who, then "reacted with :orbit:".
pub fn hover_lines(r: &Reaction) -> (String, String) {
    (r.who(NAMES_SHOWN), tr!("reacted with {emoji}", emoji = r.emoji.label()))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn emoji_picture(r: &Reaction) -> Option<Picture> {
    r.emoji.image_url().map(|url| Picture::Gallery { development: false, asset: 0, id: 0, url, large: false })
}

/// The reactions of `p` under its words, at the details' width (`ui` lays them out top down).
pub fn reactions(ui: &mut Ui, wid: Id, p: &community::Picture, talk: &mut Talk, pictures: &mut Pictures, development: bool) {
    let ctx = ui.ctx().clone();
    let said = talk.of(&ctx, p, development);
    let phase = said.as_ref().map(|s| &s.reactions);
    if p.reaction_count > 0 || matches!(phase, Some(Phase::Ready(r)) if !r.items.is_empty()) {
        ui.add_space(14.0);
        if reaction_pills(ui, wid, p, phase, talk.hover, pictures) {
            talk.retry(p);
        }
    }
}

/// The replies of `p`, at the end of its details. Links go to `open_url`.
pub fn replies(ui: &mut Ui, wid: Id, p: &community::Picture, talk: &mut Talk, pictures: &mut Pictures, development: bool, open_url: &mut Option<String>) {
    let ctx = ui.ctx().clone();
    let said = talk.of(&ctx, p, development);
    let server = if development { Server::Development } else { Server::Production };
    let phase = said.as_ref().map(|s| &s.replies);
    if p.reply_count > 0 || matches!(phase, Some(Phase::Ready(r)) if !r.items.is_empty()) {
        ui.add_space(20.0);
        if replies_list(ui, wid, p, phase, pictures, server, open_url) {
            talk.retry(p);
        }
    }
}

/// The pills, one per emoji (or the count alone before the reactions came); true when Try again was pressed.
fn reaction_pills(ui: &mut Ui, wid: Id, p: &community::Picture, phase: Option<&Phase<Reactions>>, forced: Option<usize>, pictures: &mut Pictures) -> bool {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let list: Vec<Reaction> = match phase {
        Some(Phase::Ready(r)) => r.items.clone(),
        _ => Vec::new(),
    };
    let failed = matches!(phase, Some(Phase::Failed(_)));
    // Before they came (or when they could not): the count alone, as the listing says it.
    let summary = list.is_empty();
    let label = |r: &Reaction| r.count.to_string();
    let widths: Vec<f32> = if summary {
        vec![widgets::text_size(ui, &Txt::new(p.reaction_count.to_string(), 12.5).weight(weight::SEMIBOLD)).x + 48.0]
    } else {
        list.iter().map(|r| widgets::text_size(ui, &Txt::new(label(r), 12.5).weight(weight::SEMIBOLD)).x + 50.0).collect()
    };
    let (places, h) = widgets::flow(&widths, w, PILL_H, 8.0);
    let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut retry = false;
    for (i, at) in places.iter().enumerate() {
        let r = Rect::from_min_size(area.min + *at, vec2(widths[i].min(w), PILL_H));
        let id = wid.with(("reaction", i));
        let resp = ui.interact(r, id, Sense::click());
        let hovered = resp.hovered() || forced == Some(i);
        let hover = anim::toggle(&ctx, id.with("hover"), hovered, Spring::SNAPPY);
        let pa = ui.painter().clone();
        pa.rect_filled(r, theme::radius(PILL_H * 0.5), theme::mix(theme::white(0.06), theme::white(0.11), hover));
        let ec = pos2(r.left() + 20.0, r.center().y);
        if summary {
            widgets::icon_fill(&pa, ec, icons::HEART, 14.0, theme::REC);
            widgets::text(&pa, pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(p.reaction_count.to_string(), 12.5).weight(weight::SEMIBOLD));
            if failed {
                let tip = Rect::from_min_size(r.min, r.size());
                if hover > 0.01 {
                    tooltip(&ctx, id, tip, hover, None, tr("Who reacted could not be read"), tr("Click to try again"), pictures);
                }
                if widgets::activated(&resp.on_hover_cursor(CursorIcon::PointingHand)) {
                    retry = true;
                }
            }
            continue;
        }
        let reaction = &list[i];
        match emoji_picture(reaction).and_then(|e| pictures.get(&ctx, &e)) {
            Some(tex) => {
                let er = Rect::from_center_size(ec, vec2(18.0, 18.0));
                pa.image(tex.id(), er, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None if reaction.emoji.id.is_some() => {
                widgets::icon(&pa, ec, icons::SMILEY, 15.0, theme::FG_2);
            }
            None => {
                widgets::text(&pa, ec, Align2::CENTER_CENTER, &Txt::new(reaction.emoji.label(), 15.0));
            }
        }
        widgets::text(&pa, pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(label(reaction), 12.5).weight(weight::SEMIBOLD));
        if hover > 0.01 {
            let (who, with) = hover_lines(reaction);
            tooltip(&ctx, id, r, hover, Some(reaction), &who, &with, pictures);
        }
    }
    retry
}

/// Who reacted, over the pill (under it when there is no room above): the emoji large, the names, "reacted with".
#[allow(clippy::too_many_arguments)]
fn tooltip(ctx: &egui::Context, id: Id, pill: Rect, k: f32, reaction: Option<&Reaction>, who: &str, with: &str, pictures: &mut Pictures) {
    let max_w = 300.0;
    let lead = if reaction.is_some() { 44.0 } else { 0.0 };
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, id.with("tip")));
    let who_g = painter.layout_job(Txt::new(who, 13.0).weight(weight::SEMIBOLD).line_height(18.0).job(max_w - 28.0 - lead));
    let with_g = painter.layout_job(Txt::new(with, 12.0).color(theme::FG_3).job(max_w - 28.0 - lead));
    let text_w = who_g.size().x.max(with_g.size().x);
    let size = vec2(text_w + 28.0 + lead, (who_g.size().y + 4.0 + with_g.size().y).max(if reaction.is_some() { 32.0 } else { 0.0 }) + 22.0);
    let screen = ctx.content_rect().shrink(8.0);
    let above = pill.top() - 8.0 - size.y >= screen.top();
    let y = if above { pill.top() - 8.0 - size.y } else { pill.bottom() + 8.0 };
    let x = (pill.center().x - size.x * 0.5).clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    let target = Rect::from_min_size(pos2(x, y), size);
    let a = k.clamp(0.0, 1.0);
    let rect = Rect::from_center_size(target.center() + vec2(0.0, if above { 4.0 } else { -4.0 } * (1.0 - a)), size * (0.96 + 0.04 * a));
    let mut p = painter.clone();
    p.multiply_opacity(a);
    paint::shadow(&p, rect, 14.0, vec2(0.0, 10.0), 26.0, -4.0, theme::black(0.45));
    p.rect_filled(rect, theme::radius(14.0), theme::surface_strong());
    p.rect_stroke(rect, theme::radius(14.0), theme::hairline(), egui::StrokeKind::Inside);
    let mut x = rect.left() + 14.0;
    if let Some(r) = reaction {
        let c = pos2(x + 16.0, rect.top() + 11.0 + 16.0);
        match emoji_picture(r).and_then(|e| pictures.get(ctx, &e)) {
            Some(tex) => {
                p.image(tex.id(), Rect::from_center_size(c, vec2(32.0, 32.0)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None if r.emoji.id.is_some() => widgets::icon(&p, c, icons::SMILEY, 26.0, theme::FG_2),
            None => {
                widgets::text(&p, c, Align2::CENTER_CENTER, &Txt::new(r.emoji.label(), 26.0));
            }
        }
        x += lead;
    }
    let top = rect.top() + 11.0;
    let wh = who_g.size().y;
    p.galley(pos2(x, top), who_g, theme::FG);
    p.galley(pos2(x, top + wh + 4.0), with_g, theme::FG_3);
    if a < 1.0 {
        ctx.request_repaint();
    }
}

/// The replies, under the reactions; true when Try again was pressed.
fn replies_list(ui: &mut Ui, wid: Id, p: &community::Picture, phase: Option<&Phase<Replies>>, pictures: &mut Pictures, server: Server, open_url: &mut Option<String>) -> bool {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let ready = match phase {
        Some(Phase::Ready(r)) => Some(r),
        _ => None,
    };
    let total = ready.map(|r| r.total).unwrap_or(p.reply_count);
    // The heading: how many, and the message on Discord.
    let (head, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
    let pa = ui.painter().clone();
    widgets::text(&pa, pos2(head.left(), head.center().y), Align2::LEFT_CENTER, &Txt::new(heading(total), 12.0).weight(weight::SEMIBOLD).color(theme::FG_3));
    if let Some(link) = ready.and_then(|r| r.discord_link()) {
        let br = Rect::from_min_size(pos2(head.right() - 32.0, head.top()), vec2(32.0, 32.0));
        let id = wid.with("replies-discord");
        let resp = ui.interact(br, id, Sense::click()).on_hover_text(tr("Open the message on Discord")).on_hover_cursor(CursorIcon::PointingHand);
        let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
        pa.circle_filled(br.center(), 16.0, theme::white(0.04 + 0.08 * hover));
        widgets::icon_fill(&pa, br.center(), icons::DISCORD_LOGO, 17.0, DISCORD);
        if widgets::activated(&resp) {
            *open_url = Some(link.to_string());
        }
    }
    ui.add_space(8.0);
    match phase {
        Some(Phase::Failed(why)) => {
            let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            let said = Txt::new(tr!("The replies could not be read: {why}", why = tr(why)), 12.5).color(theme::FG_2);
            let b = Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(Size::Sm).variant(Variant::Secondary).id_salt(wid.with("replies-retry"));
            let bw = b.desired_width(ui);
            widgets::text_truncated(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &said, w - bw - 12.0);
            return super::sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(r.right() - bw, r.top()), vec2(bw, 32.0))));
        }
        Some(Phase::Ready(replies)) => {
            for (i, row) in rows(replies, now(), server).iter().enumerate() {
                if i > 0 {
                    ui.add_space(12.0);
                }
                reply(ui, wid.with(("reply", i)), row, pictures, open_url);
            }
            if replies.more() > 0 {
                if let Some(link) = replies.discord_link() {
                    ui.add_space(10.0);
                    let t = Txt::new(tr!("{count} more on Discord", count = replies.more()), 12.5).weight(weight::SEMIBOLD).color(theme::ACCENT);
                    let tw = widgets::text_size(ui, &t).x;
                    let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
                    let lr = Rect::from_min_size(pos2(r.left() + AVATAR + 10.0, r.top()), vec2(tw, 20.0));
                    let resp = ui.interact(lr, wid.with("replies-more"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                    widgets::text(ui.painter(), pos2(lr.left(), lr.center().y), Align2::LEFT_CENTER, &t);
                    if resp.hovered() {
                        ui.painter().hline(lr.x_range(), lr.bottom() - 1.0, egui::Stroke::new(1.0, theme::ACCENT));
                    }
                    if widgets::activated(&resp) {
                        *open_url = Some(link.to_string());
                    }
                }
            }
        }
        // Coming: placeholder rows, as many as the listing counts (at most three), so the window grows once.
        _ => {
            for i in 0..p.reply_count.clamp(1, 3) as usize {
                if i > 0 {
                    ui.add_space(12.0);
                }
                let (r, _) = ui.allocate_exact_size(vec2(w, 62.0), Sense::hover());
                let pa = ui.painter();
                feedback::shimmer(pa, Rect::from_min_size(r.min, vec2(AVATAR, AVATAR)), AVATAR * 0.5);
                let x = r.left() + AVATAR + 10.0;
                feedback::shimmer(pa, Rect::from_min_size(pos2(x, r.top() + 2.0), vec2(72.0, 10.0)), 5.0);
                feedback::shimmer(pa, Rect::from_min_size(pos2(x, r.top() + 20.0), vec2((w - AVATAR - 10.0) * (0.62 - i as f32 * 0.14), 36.0)), 14.0);
            }
        }
    }
    false
}

/// One reply: their picture, their name and when, their words in a bubble, their stickers and files.
fn reply(ui: &mut Ui, id: Id, row: &Row, pictures: &mut Pictures, open_url: &mut Option<String>) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let x = AVATAR + 10.0;
    let bubble_w = w - x;
    let text = (!row.text.is_empty()).then(|| ui.painter().layout_job(Txt::new(&row.text, 13.5).line_height(19.0).job(bubble_w - 24.0)));
    let chips_h = if row.extras.is_empty() { 0.0 } else { 6.0 + 24.0 };
    let bubble_h = text.as_ref().map(|g| g.size().y + 15.0).unwrap_or(0.0);
    let h = 20.0 + bubble_h + chips_h;
    let (r, _) = ui.allocate_exact_size(vec2(w, h.max(AVATAR)), Sense::hover());
    let pa = ui.painter().clone();
    // Their picture and name open their Orbiters profile when they have one.
    let face = row.avatar.clone().map(|url| Picture::Gallery { development: false, asset: 0, id: 0, url, large: false }).and_then(|pic| pictures.get(&ctx, &pic));
    let ar = Rect::from_min_size(r.min, vec2(AVATAR, AVATAR));
    widgets::avatar(&pa, ar, face.as_ref(), &row.name);
    let name = Txt::new(&row.name, 12.5).weight(weight::SEMIBOLD);
    let nr = widgets::text_truncated(&pa, pos2(r.left() + x, r.top() + 8.0), Align2::LEFT_CENTER, &name, bubble_w * 0.6);
    if let Some(profile) = &row.profile {
        for (part, rect) in [("avatar", ar), ("name", nr)] {
            let resp = ui.interact(rect, id.with(part), Sense::click()).on_hover_text(tr!("{name} on Orbiters", name = row.name)).on_hover_cursor(CursorIcon::PointingHand);
            if part == "name" && resp.hovered() {
                pa.hline(nr.x_range(), nr.bottom(), egui::Stroke::new(1.0, theme::FG));
            }
            if widgets::activated(&resp) {
                *open_url = Some(profile.clone());
            }
        }
    }
    let when = if row.edited { tr!("{when} · edited", when = row.when) } else { row.when.clone() };
    if !when.trim().is_empty() {
        let tr = widgets::text(&pa, pos2(nr.right() + 8.0, r.top() + 8.0), Align2::LEFT_CENTER, &Txt::new(when, 11.5).color(theme::FG_3));
        if !row.when_full.is_empty() {
            ui.interact(tr, id.with("when"), Sense::hover()).on_hover_text(&row.when_full);
        }
    }
    let mut y = r.top() + 20.0;
    if let Some(g) = text {
        let br = Rect::from_min_size(pos2(r.left() + x, y), vec2((g.size().x + 24.0).min(bubble_w), bubble_h));
        // The website's chat bubble: filled, its corner by their picture tighter.
        pa.rect_filled(br, CornerRadius { nw: 6, ne: 16, sw: 16, se: 16 }, theme::white(0.06));
        pa.galley(pos2(br.left() + 12.0, br.top() + 7.0), g, theme::FG);
        y = br.bottom();
    }
    if !row.extras.is_empty() {
        let mut cx = r.left() + x;
        let cy = y + 6.0;
        for (glyph, label) in &row.extras {
            let t = Txt::new(label, 11.5).color(theme::FG_2);
            let tw = widgets::text_size(ui, &t).x.min(180.0);
            let cw = tw + 34.0;
            if cx + cw > r.right() {
                break;
            }
            let cr = Rect::from_min_size(pos2(cx, cy), vec2(cw, 24.0));
            pa.rect_filled(cr, theme::radius(12.0), theme::alpha(theme::FG_3, 0.14));
            widgets::icon(&pa, pos2(cr.left() + 13.0, cr.center().y), glyph, 12.0, theme::FG_2);
            widgets::text_truncated(&pa, pos2(cr.left() + 24.0, cr.center().y), Align2::LEFT_CENTER, &t, tw);
            cx += cw + 6.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::gallery_talk::{Emoji, Person};

    fn replies() -> Replies {
        serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/gallery-replies.json")).unwrap()
    }

    #[test]
    fn replies_become_the_rows_the_window_lists() {
        let now = crate::controller::shares::unix("2026-10-29T11:02:00Z");
        let rows = rows(&replies(), now, Server::Development);
        assert_eq!(rows.len(), 3);
        let [mika, alex, robin] = &rows[..] else { panic!() };
        assert_eq!((mika.name.as_str(), mika.when.as_str(), mika.edited), ("Mika", "14 h ago", false));
        assert_eq!(mika.text, "Lovely shot @Alex! :orbit: Photographers, look: @Photographers");
        assert_eq!(mika.profile.as_deref(), Some("https://dev.orbiters.cc/user/7"));
        assert!(!mika.when_full.is_empty());
        assert!(alex.edited && alex.profile.is_none());
        assert_eq!(alex.extras, [(icons::SMILEY, "Wave".to_string()), (icons::IMAGE, "behind-the-scenes.png".to_string())]);
        assert_eq!(robin.avatar, None, "a picture that is not Discord's is not loaded");
        assert_eq!(heading(1), "1 reply");
        assert_eq!(heading(5), "5 replies");
        // A reply posted after this computer's clock (a skewed clock) is "just now", never in the future.
        let late = Reply { posted_at: Some("2026-10-29T12:00:00Z".into()), ..Default::default() };
        assert_eq!(row(&late, now, Server::Production).when, "just now");
        assert_eq!(row(&Reply::default(), now, Server::Production).name, "Discord member");
    }

    #[test]
    fn a_server_without_these_routes_says_so() {
        let refused = |status, code: Option<&str>, message: &str| {
            anyhow::Error::new(community::Refused { status, code: code.map(str::to_owned), message: message.into() })
        };
        assert_eq!(why(&refused(401, None, "Sign in to Orbiters in Settings first.")), "Orbiters does not let this app read them yet.");
        assert_eq!(why(&refused(404, None, "This event is no longer on Orbiters.")), "Orbiters does not have them for this picture yet.");
        assert_eq!(why(&refused(404, None, "Image no longer available.")), "Image no longer available.");
        assert_eq!(why(&refused(403, Some("GALLERY_MEMBERS_ONLY"), "Only members of this server can see it.")), "Only members of this server can see it.");
        assert_eq!(why(&anyhow::anyhow!("timed out")), "timed out");
    }

    #[test]
    fn a_reaction_s_hover_names_who_reacted() {
        let r = Reaction {
            emoji: Emoji { key: "orbit:1".into(), id: Some("1".into()), name: "orbit".into(), animated: false },
            count: 4,
            users: ["Alex", "Mika", "Robin"].iter().map(|n| Person { name: n.to_string(), ..Default::default() }).collect(),
        };
        assert_eq!(hover_lines(&r), ("Alex, Mika, Robin and 1 other".into(), "reacted with :orbit:".into()));
        let many = Reaction { count: 100, users: (0..100).map(|i| Person { name: format!("m{i}"), ..Default::default() }).collect(), ..r };
        assert!(hover_lines(&many).0.ends_with("m59 and 40 others"), "the first {NAMES_SHOWN} by name");
    }

    /// Counts take each language's plural and what a reaction says is a phrase of its own, in the language of the moment.
    #[test]
    fn replies_and_reactions_speak_the_language() {
        use orbiters_i18n::{in_language, Language};
        in_language(Language::Fr, || {
            assert_eq!((heading(0), heading(1), heading(2)), ("0 réponse".to_string(), "1 réponse".to_string(), "2 réponses".to_string()));
        });
        in_language(Language::Ja, || assert_eq!(heading(5), "返信5件"));
        in_language(Language::Es, || assert_eq!((heading(0), heading(1)), ("0 respuestas".to_string(), "1 respuesta".to_string())));
    }

    #[test]
    fn only_community_pictures_with_talk_are_read() {
        let p = |id, asset_id, reply_count, reaction_count| community::Picture { id, asset_id, reply_count, reaction_count, ..Default::default() };
        assert!(readable(&p(4, 0, 2, 0)) && readable(&p(4, 0, 0, 1)));
        assert!(!readable(&p(4, 0, 0, 0)), "nothing to read");
        assert!(!readable(&p(4, 7, 3, 3)), "an asset's showcase picture");
        assert!(!readable(&p(0, 0, 3, 3)), "a picture without its placement");
        // Staged (screenshots): what was staged, never a read.
        let ctx = egui::Context::default();
        let mut talk = Talk::default();
        talk.stage(Said { replies: Phase::Ready(replies()), reactions: Phase::Waiting });
        let said = talk.of(&ctx, &p(4, 0, 3, 3), false).unwrap();
        assert!(matches!(said.replies, Phase::Ready(ref r) if r.items.len() == 3));
        assert!(talk.jobs.is_empty());
    }
}
