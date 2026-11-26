//! The avatar gun on N4N0's page (contracts `Docs/avatar-gun.md`): a card with the avatar the gun is loaded with, what
//! the gun can do now, who may change this player's avatar with one, and how it works; its Choose avatar opens the picker,
//! a page that grows out of the button (as Photos & captures' details grow out of their cards): the public avatars of the
//! VRChat account's favourite lists, a section for each list (its name, how many) over a grid of the avatars, the loaded
//! one ringed. A press loads the gun and the page stays. The lists come group by group (`crate::avatar_gun_picker`), a
//! list not read yet keeping its row of grey cards.

use super::{gallery_layout, section, Cx, Go};
use crate::avatar_gun_picker::{self as picker, Stage, Store};
use crate::controller::vrchat_data::Scope;
use egui::{pos2, vec2, Align2, Context, Id, Order, Rect, Sense, TextureId, Ui, UiBuilder};
use orbiters_core::settings::AvatarGunAllow;
use orbiters_i18n::{mark, tr};
use orbiters_ui::anim::Spring;
use orbiters_ui::morph;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Size, Variant};
use orbiters_ui::icons;

pub const TITLE: &str = mark("Avatar gun");
pub const SUB: &str = mark("Shoot a friend into another avatar");
pub const HOW: &str = mark("Wear an avatar with Orbiters' avatar gun, load it with a public avatar from your favorites, point at someone and pull the trigger: their avatar changes to it. They need Orbiters - N4N0 running and a receiver on their avatar (N4N0's prop has one), and their own setting decides: asked first, at once, or never.");
pub const WHO: &str = mark("Who can change my avatar");
pub const WHO_WHY: &str = mark("When someone's avatar gun hits you. Friends: VRChat friends whose account Orbiters verified change it at once, everyone else asks.");
pub const CHOOSE: &str = mark("Choose avatar");
/// Settings' search finds the setting by these too.
pub const KEYWORDS: &str = mark("avatar gun shoot change avatar hit friends ask");

/// The picker's page while it shows, and where it grew from.
struct Detail {
    origin: Rect,
    closing: bool,
    serial: u64,
}

#[derive(Default)]
pub struct State {
    store: Store,
    pictures: Option<crate::worlds::Pictures>,
    detail: Option<Detail>,
    serials: u64,
    /// The avatar pressed here, and when (ringed until the settings say it back).
    picked: Option<(String, f64)>,
    /// Made-up pictures by avatar (the screenshot review): shown in place of VRChat's.
    staged: std::collections::HashMap<String, egui::TextureHandle>,
}

impl State {
    /// The screenshot review's made-up lists and pictures; the picker open (grown from `origin`) when `open`.
    pub fn stage(&mut self, ctx: &Context, stage: Stage, groups: Vec<picker::Group>, pictures: &dyn Fn(usize) -> egui::ColorImage, open: Option<Rect>) {
        self.staged.clear();
        for (i, pick) in groups.iter().filter_map(|g| g.avatars.as_ref()).flatten().enumerate() {
            if i % 5 != 4 {
                self.staged.insert(pick.id.clone(), ctx.load_texture("avatar-gun-stand-in", pictures(i), egui::TextureOptions::LINEAR));
            }
        }
        self.store.stand_in(stage, groups);
        if let Some(origin) = open {
            self.serials += 1;
            self.detail = Some(Detail { origin, closing: false, serial: self.serials });
        }
    }
}

/// A segmented choice of who may change the avatar.
pub fn allow_options() -> [SegOption<'static, AvatarGunAllow>; 4] {
    [
        SegOption::new(AvatarGunAllow::Nobody, tr("Nobody")),
        SegOption::new(AvatarGunAllow::AskFirst, tr("Ask first")),
        SegOption::new(AvatarGunAllow::Friends, tr("Friends")),
        SegOption::new(AvatarGunAllow::Anyone, tr("Anyone")),
    ]
}

/// What the gun can do now, in a line.
fn status(cx: &Cx) -> (&'static str, egui::Color32) {
    let snap = cx.snap;
    let gun = &snap.avatar_gun;
    if snap.settings.avatar_gun.loaded.is_none() {
        (tr("Not loaded: choose an avatar."), theme::fg_3())
    } else if !gun.gun {
        (tr("Your avatar has no avatar gun. Add it in Orbiters' My Avatar."), theme::fg_3())
    } else if snap.account.state.username().is_none() {
        (tr("Sign in to Orbiters to fire it."), theme::WARN)
    } else if gun.loaded {
        (tr("Loaded. Point at someone and pull the trigger."), theme::OK)
    } else {
        (tr("Loading… the gun shows it when it's ready."), theme::fg_3())
    }
}

fn picture(state: &mut State, ctx: &Context, cx: &Cx, id: &str, url: &str) -> Option<TextureId> {
    if let Some(t) = state.staged.get(id) {
        return Some(t.id());
    }
    if url.is_empty() {
        return None;
    }
    let pictures = state.pictures.get_or_insert_with(|| crate::worlds::Pictures::with_backend(ctx, Some(cx.ctl.clone())));
    pictures.get(ctx, &crate::worlds::Picture::Avatar { scope: Scope::of(cx.snap), avatar_id: id.to_string(), url: url.to_string() }).map(|t| t.id())
}

/// An avatar's picture in `rect`, its corners rounded; its place when it has none yet.
fn thumb(p: &egui::Painter, rect: Rect, radius: f32, texture: Option<TextureId>) {
    let radii = [radius; 4];
    match texture {
        Some(id) => orbiters_ui::paint::squircle_texture(p, rect, radii, id, crate::avatar_gun_ui::cover_uv(rect.size(), 4.0 / 3.0), egui::Color32::WHITE),
        None => {
            orbiters_ui::paint::fill_convex(p, &orbiters_ui::paint::squircle_points_corners(rect, radii), |_| theme::ink_750());
            widgets::icon(p, rect.center(), icons::USER, (rect.height() * 0.32).min(34.0), theme::fg_4());
        }
    }
}

/// The card on N4N0's page.
pub fn card(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State) {
    let snap = cx.snap;
    let ctx = ui.ctx().clone();
    let loaded = snap.settings.avatar_gun.loaded.clone();
    let sub = loaded.as_ref().map_or_else(|| tr(SUB).to_string(), |l| tr!("Loaded with {name}", name = l.name));
    let mut open_at: Option<Rect> = None;
    let mut allow = snap.settings.avatar_gun.allow;
    section(ui, cx, index, icons::CROSSHAIR, tr(TITLE), Some(&sub), |ui| {
        // The avatar loaded, and what the gun can do now; Choose avatar beside them.
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), Sense::hover());
        let pic = Rect::from_min_size(row.min + vec2(0.0, 2.0), vec2(80.0 * 4.0 / 3.0, 80.0));
        let texture = loaded.as_ref().and_then(|l| picture(state, &ctx, cx, &l.id, &l.thumbnail));
        thumb(ui.painter(), pic, 12.0, texture);
        if loaded.is_some() {
            ui.painter().rect_stroke(pic.expand(2.0), theme::radius(14.0), egui::Stroke::new(2.0, if snap.avatar_gun.loaded { theme::accent() } else { theme::white(0.25) }), egui::StrokeKind::Middle);
        }
        let choose = Button::new(tr(CHOOSE)).icon(icons::CROSSHAIR).variant(Variant::Primary).size(Size::Sm);
        let bw = choose.desired_width(ui);
        let at = Rect::from_min_size(pos2(row.right() - bw, row.center().y - 16.0), vec2(bw, 32.0));
        let x = pic.right() + 16.0;
        let room = at.left() - 16.0 - x;
        let name = loaded.as_ref().map_or_else(|| tr("No avatar loaded").to_string(), |l| l.name.clone());
        widgets::text_truncated(ui.painter(), pos2(x, row.center().y - 11.0), Align2::LEFT_CENTER, &Txt::new(name, 16.0).weight(weight::SEMIBOLD), room);
        let (line, colour) = status(cx);
        widgets::text_truncated(ui.painter(), pos2(x, row.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(line, 12.5).color(colour), room);
        if choose.show_at(ui, at).clicked() {
            open_at = Some(at);
        }
        ui.add_space(10.0);
        widgets::separator(ui);
        let options = allow_options();
        let width = widgets::segmented_width(ui, &options, true);
        widgets::field(ui, tr(WHO), Some(tr(WHO_WHY)), width, |ui| {
            widgets::segmented(ui, "n4n0-avatar-gun-allow", &mut allow, &options, true);
        });
        widgets::separator(ui);
        let g = ui.painter().layout_job(Txt::new(tr(HOW), 12.0).color(theme::fg_3()).line_height(16.0).job(ui.available_width()));
        let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
        ui.painter().galley(r.min, g, theme::fg_3());
    });
    if allow != snap.settings.avatar_gun.allow {
        let mut changed = snap.settings.clone();
        changed.avatar_gun.allow = allow;
        cx.ctl.save_changes(&snap.settings, &changed);
    }
    if let Some(origin) = open_at {
        state.serials += 1;
        state.detail = Some(Detail { origin, closing: false, serial: state.serials });
        state.store.again();
    }
}

/// The picker's page over the window while it shows (after the page under it is drawn).
pub fn detail(ctx: &Context, cx: &mut Cx, state: &mut State) {
    let Some(d) = state.detail.as_ref() else { return };
    let (origin, closing, serial) = (d.origin, d.closing, d.serial);
    state.store.follow(Some(cx.ctl), cx.snap);
    let id = Id::unique(("avatar-gun-picker", serial));
    let travel = morph::travel(ctx, id.with("morph"), !closing, Spring::MORPH);
    let t = travel.t;
    if travel.done {
        state.detail = None;
        return;
    }
    if travel.landed {
        morph::land(ctx, origin, travel.landing);
    }
    let window = ctx.content_rect();
    let page = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
    let target = Rect::from_center_size(page.center(), vec2((page.width() - 64.0).min(1040.0), (page.height() - 48.0).min(760.0)));
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let rect = Rect::from_min_max(pos2(lerp(origin.left(), target.left()), lerp(origin.top(), target.top())), pos2(lerp(origin.right(), target.right()), lerp(origin.bottom(), target.bottom())));
    let content = ((t - 0.55) / 0.45).clamp(0.0, 1.0);
    let mut close = false;
    let k = t.clamp(0.0, 1.0);
    orbiters_ui::front::area(ctx, id, Order::Foreground, !closing).fixed_pos(page.min).interactable(!closing).show(ctx, |ui| {
        let (_, backdrop) = ui.allocate_exact_size(page.size(), Sense::click());
        if backdrop.clicked() && !rect.contains(backdrop.interact_pointer_pos().unwrap_or_default()) {
            close = true;
        }
        ui.painter().with_clip_rect(window).rect_filled(window, 0.0, theme::alpha(theme::ink_950(), 0.7 * morph::smoothstep(0.1, 0.6, k)));
        ui.multiply_opacity(morph::shape_alpha_by(t, 0.2));
        widgets::panel(ui.painter(), rect, lerp(10.0, 24.0));
        if content <= 0.0 {
            return;
        }
        let mut cu = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(28.0, 22.0))).id_salt(("avatar-gun-body", serial)));
        cu.multiply_opacity(content);
        let inner = cu.max_rect();
        // Its head: the avatar loaded (ringed), what the gun can do, and its close.
        let close_r = Rect::from_min_size(pos2(inner.right() - 36.0, inner.top()), vec2(36.0, 36.0));
        if IconButton::new(icons::X, mark("Close")).size(36.0, 18.0).show_at(&mut cu, close_r).clicked() {
            close = true;
        }
        let snap = cx.snap;
        let chosen = state.picked.as_ref().map(|(id, _)| id.clone()).or_else(|| snap.settings.avatar_gun.loaded.as_ref().map(|l| l.id.clone()));
        let head_pic = Rect::from_min_size(inner.min + vec2(0.0, 2.0), vec2(56.0 * 4.0 / 3.0, 56.0));
        let loaded_pick = chosen.as_deref().and_then(|c| state.store.find(c)).cloned();
        let shown = loaded_pick.as_ref().map(|p| (p.id.clone(), p.name.clone(), p.thumbnail.clone())).or_else(|| snap.settings.avatar_gun.loaded.as_ref().map(|l| (l.id.clone(), l.name.clone(), l.thumbnail.clone())));
        let texture = shown.as_ref().and_then(|(id, _, url)| picture(state, ctx, cx, id, url));
        thumb(cu.painter(), head_pic, 10.0, texture);
        if shown.is_some() {
            cu.painter().rect_stroke(head_pic.expand(2.0), theme::radius(12.0), egui::Stroke::new(2.0, if snap.avatar_gun.loaded { theme::accent() } else { theme::white(0.25) }), egui::StrokeKind::Middle);
        }
        let x = head_pic.right() + 16.0;
        let title = shown.as_ref().map_or_else(|| tr(TITLE).to_string(), |(_, name, _)| tr!("Loaded: {name}", name = name));
        widgets::text_truncated(cu.painter(), pos2(x, head_pic.center().y - 12.0), Align2::LEFT_CENTER, &Txt::new(title, 22.0).weight(weight::SEMIBOLD).tracking(-0.01), close_r.left() - 16.0 - x);
        let (line, colour) = status(cx);
        widgets::text_truncated(cu.painter(), pos2(x, head_pic.center().y + 14.0), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(colour), close_r.left() - 16.0 - x);
        let list = Rect::from_min_max(pos2(inner.left(), head_pic.bottom() + 20.0), inner.max);
        cu.painter().line_segment([pos2(inner.left(), list.top() - 10.0), pos2(inner.right(), list.top() - 10.0)], egui::Stroke::new(1.0, theme::white(0.08)));
        let mut lu = cu.new_child(UiBuilder::new().max_rect(list).id_salt(("avatar-gun-list", serial)));
        let picked = body(&mut lu, cx, state, chosen.as_deref());
        if let Some(pick) = picked {
            let mut changed = snap.settings.clone();
            changed.avatar_gun.loaded = Some(pick.loaded());
            cx.ctl.save_changes(&snap.settings, &changed);
            state.picked = Some((pick.id, anim_time(ctx)));
        }
    });
    // Said back by the settings (or long ago): the settings ring it.
    if state.picked.as_ref().is_some_and(|(id, at)| anim_time(ctx) - at > 5.0 || cx.snap.settings.avatar_gun.loaded.as_ref().is_some_and(|l| l.id == *id)) {
        state.picked = None;
    }
    if close || (!closing && ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        if let Some(d) = state.detail.as_mut() {
            d.closing = true;
        }
    }
}

fn anim_time(ctx: &Context) -> f64 {
    orbiters_ui::anim::time(ctx)
}

/// The lists, or why there are none. The avatar pressed, if one was.
fn body(ui: &mut Ui, cx: &mut Cx, state: &mut State, chosen: Option<&str>) -> Option<picker::Pick> {
    let ctx = ui.ctx().clone();
    let area = ui.max_rect();
    let middle = |ui: &mut Ui, glyph: &str, title: &str, words: &str| {
        let c = area.center() - vec2(0.0, 40.0);
        widgets::icon(ui.painter(), c - vec2(0.0, 34.0), glyph, 44.0, theme::fg_4());
        widgets::text(ui.painter(), c + vec2(0.0, 12.0), Align2::CENTER_CENTER, &Txt::new(title, 18.0).weight(weight::SEMIBOLD));
        let mut job = Txt::new(words, 13.0).color(theme::fg_3()).line_height(18.0).job(520.0);
        job.halign = egui::Align::Center;
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(c.x, c.y + 34.0), g, theme::fg_3());
        Rect::from_center_size(pos2(c.x, c.y + 34.0 + 70.0), vec2(220.0, 34.0))
    };
    let listed = match state.store.stage().clone() {
        Stage::SignedOut { gated: true } => {
            middle(ui, icons::LOCK_SIMPLE, tr("VRChat sign-in needed"), tr(picker::GATED));
            false
        }
        Stage::SignedOut { gated: false } => {
            let at = middle(ui, icons::SIGN_IN, tr("Sign in to VRChat"), tr(picker::SIGN_IN));
            if Button::new(tr("Sign in to VRChat")).icon(icons::SIGN_IN).variant(Variant::Primary).size(Size::Sm).show_at(ui, at).clicked() {
                cx.go = Some(Go::VrchatAccount);
            }
            false
        }
        Stage::Failed(e) => {
            let at = middle(ui, icons::WARNING_CIRCLE, tr("Your favorites could not be read"), &picker::failure(&e));
            if Button::new(tr(picker::RETRY)).icon(icons::ARROW_CLOCKWISE).variant(Variant::Secondary).size(Size::Sm).show_at(ui, at).clicked() {
                state.store.again();
            }
            false
        }
        Stage::Ready if state.store.groups().iter().all(|g| g.avatars.as_ref().is_some_and(Vec::is_empty)) => {
            middle(ui, icons::STAR, tr("No public avatar yet"), tr(picker::EMPTY));
            false
        }
        _ => true,
    };
    if !listed {
        return None;
    }
    let mut pressed = None;
    let groups: Vec<picker::Group> = state.store.groups().to_vec();
    let skeletons = groups.is_empty();
    let retry = state.store.failed().is_some();
    egui::ScrollArea::vertical().id_salt("avatar-gun-picker-list").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 10.0);
        let width = ui.available_width() - 12.0;
        let grey = |ui: &mut Ui, n: usize| {
            let (rects, h) = gallery_layout::grid_shaped(n, width, 150.0, 4.0 / 3.0, 30.0);
            let (area, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
            for r in rects {
                let r = r.translate(area.min.to_vec2());
                let pic = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 3.0 / 4.0));
                orbiters_ui::paint::fill_convex(ui.painter(), &orbiters_ui::paint::squircle_points_corners(pic, [12.0; 4]), |_| theme::white(0.05));
                ui.painter().rect_filled(Rect::from_min_size(pos2(r.left(), pic.bottom() + 10.0), vec2(r.width() * 0.6, 9.0)), theme::radius(4.5), theme::white(0.05));
            }
        };
        let columns = orbiters_ui::layout::columns(width, 150.0, gallery_layout::GAP).0;
        if skeletons {
            for _ in 0..2 {
                widgets::section_header(ui, icons::STAR, tr("Loading…"), None);
                grey(ui, columns);
            }
        }
        if retry {
            let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
            let words = state.store.failed().map(picker::failure).unwrap_or_default();
            let b = Button::new(tr(picker::RETRY)).icon(icons::ARROW_CLOCKWISE).variant(Variant::Secondary).size(Size::Sm);
            let bw = b.desired_width(ui);
            widgets::text_truncated(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &Txt::new(words, 12.5).color(theme::WARN), r.width() - bw - 16.0);
            if b.show_at(ui, Rect::from_min_size(pos2(r.right() - bw, r.top()), vec2(bw, 32.0))).clicked() {
                state.store.again();
            }
        }
        for (g, group) in groups.iter().enumerate() {
            let count = group.avatars.as_ref().map(|a| orbiters_i18n::fill(&orbiters_i18n::tr_n("{count} avatar", "{count} avatars", a.len() as u64), &[("count", &a.len())]));
            let sub = count.unwrap_or_else(|| if group.error.is_some() { String::new() } else { tr("Loading…").to_string() });
            widgets::section_header(ui, icons::STAR, &group.name, (!sub.is_empty()).then_some(sub.as_str()));
            match (&group.avatars, &group.error) {
                (Some(avatars), _) if !avatars.is_empty() => {
                    let (rects, h) = gallery_layout::grid_shaped(avatars.len(), width, 150.0, 4.0 / 3.0, 30.0);
                    let (area, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    for (i, (avatar, r)) in avatars.iter().zip(rects).enumerate() {
                        let r = r.translate(area.min.to_vec2());
                        // Only what is in view asks for its picture.
                        if !ui.is_rect_visible(r) {
                            continue;
                        }
                        let response = ui.interact(r, Id::unique(("avatar-gun-tile", g, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        let [lift] = orbiters_ui::anim::spring(&ctx, Id::unique(("avatar-gun-tile-lift", g, i)), [if response.hovered() { 1.0 } else { 0.0 }], Spring::SNAPPY);
                        let pic = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 3.0 / 4.0)).translate(vec2(0.0, -2.0 * lift));
                        let texture = picture(state, &ctx, cx, &avatar.id, &avatar.thumbnail);
                        thumb(ui.painter(), pic, 12.0, texture);
                        let on = chosen == Some(avatar.id.as_str());
                        if on {
                            ui.painter().rect_stroke(pic.expand(1.5), theme::radius(13.5), egui::Stroke::new(2.5, theme::accent()), egui::StrokeKind::Middle);
                            let badge = pos2(pic.right() - 14.0, pic.top() + 14.0);
                            ui.painter().circle_filled(badge, 10.0, theme::accent());
                            widgets::icon(ui.painter(), badge, icons::CHECK, 12.0, theme::ink_950());
                        } else if lift > 0.01 {
                            ui.painter().rect_stroke(pic, theme::radius(12.0), egui::Stroke::new(1.5, theme::alpha(theme::accent(), 0.7 * lift)), egui::StrokeKind::Inside);
                        }
                        let colour = if on { theme::accent() } else { theme::fg() };
                        widgets::text_truncated(ui.painter(), pos2(r.left() + 2.0, pic.bottom() + 16.0), Align2::LEFT_CENTER, &Txt::new(&avatar.name, 13.0).weight(weight::MEDIUM).color(colour), r.width() - 4.0);
                        if response.clicked() && !on {
                            pressed = Some(avatar.clone());
                        }
                    }
                }
                (Some(_), _) => {
                    widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 10.0), Align2::LEFT_CENTER, &Txt::new(tr(picker::EMPTY_GROUP), 12.5).color(theme::fg_3()));
                    ui.add_space(20.0);
                }
                (None, Some(e)) => {
                    let words = picker::failure(e);
                    widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 10.0), Align2::LEFT_CENTER, &Txt::new(&words, 12.5).color(theme::fg_3()));
                    ui.add_space(20.0);
                }
                (None, None) => grey(ui, columns),
            }
            ui.add_space(8.0);
        }
    });
    // (Its pictures wake the page; the lists are read by a thread of a store the headset's panel has too.)
    if !state.store.settled() {
        orbiters_ui::anim::working_read(&ctx);
    }
    pressed
}
