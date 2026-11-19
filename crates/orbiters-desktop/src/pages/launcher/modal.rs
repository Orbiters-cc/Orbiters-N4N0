//! A person's profile over the page. It grows out of the row that was pressed, its banner and picture settle, its
//! sections come in one after the other; Escape, a press outside or × close it, back into the row.

use super::row::{boop_button, burst, emoji_icon, reach_pill};
use super::states::{self, Reach};
use super::{access_icon, access_name, cover_uv, person_picture, rank_color, rank_name, region_tag, status_color, status_name, Cx, State};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Key, LayerId, Order, Pos2, Rect, Sense, Ui, UiBuilder};
use orbiters_core::launcher::{Action, Friend, InstanceAccess, Platform, Profile, Whereabouts};
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};

const W: f32 = 580.0;
const H: f32 = 700.0;
const BANNER_H: f32 = 156.0;
const PICTURE: f32 = 104.0;

fn layer() -> LayerId {
    LayerId::new(Order::Foreground, Id::unique("launcher-profile"))
}

/// Whether the modal is up (rows under it do not light up).
pub(super) fn covers(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::unique("launcher-profile-up")).unwrap_or(false))
}

/// The profile shown: the contract's, once it is this person's; their row's meanwhile.
fn profile(state: &State, id: &str) -> Option<Profile> {
    match &state.data.profile {
        Some(p) if p.friend.id == id => Some(p.clone()),
        _ => state.data.friend(id).map(|f| Profile { friend: f.clone(), loading: true, ..Default::default() }),
    }
}

pub(super) fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State, page: Rect, clock: f64) {
    let ctx = ui.ctx().clone();
    let Some(modal) = state.modal.as_ref() else {
        ctx.data_mut(|d| d.insert_temp(Id::unique("launcher-profile-up"), false));
        return;
    };
    let (id, from, opened, closing) = (modal.id.clone(), modal.from, modal.opened, modal.closing);
    let Some(profile) = profile(state, &id) else {
        state.modal = None;
        return;
    };
    let key = Id::unique(("launcher-profile-t", opened.to_bits()));
    let t = anim::spring_from(&ctx, key, [0.0], [if closing.is_some() { 0.0 } else { 1.0 }], Spring::EXPAND)[0];
    if closing.is_some() && t < 0.03 {
        state.modal = None;
        ctx.data_mut(|d| d.insert_temp(Id::unique("launcher-profile-up"), false));
        return;
    }
    ctx.data_mut(|d| d.insert_temp(Id::unique("launcher-profile-up"), closing.is_none()));
    let area = Rect::from_min_max(pos2(page.left(), page.top() + cx.top), page.max);
    let w = W.min(area.width() - 40.0);
    let h = H.min(area.height() - 40.0);
    let target = Rect::from_center_size(area.center(), vec2(w, h));
    // From the row to the middle: the rect follows the spring, the content comes once it has room.
    let k = t.clamp(0.0, 1.2);
    let rect = Rect::from_min_max(from.min.lerp(target.min, k), from.max.lerp(target.max, k));
    let content = ((t - 0.35) / 0.65).clamp(0.0, 1.0);

    // The whole window dims (the sidebar too): one thing at a time.
    let screen = ctx.content_rect();
    let mut over = ui.new_child(UiBuilder::new().max_rect(screen).layer_id(layer()));
    over.set_clip_rect(screen);
    let scrim = over.interact(screen, Id::unique("launcher-profile-scrim"), Sense::click());
    over.painter().rect_filled(screen, 0.0, theme::black(0.55 * t.clamp(0.0, 1.0)));
    let panel_resp = over.interact(rect, Id::unique("launcher-profile-panel"), Sense::click());
    paint::shadow(over.painter(), rect, 24.0, vec2(0.0, 24.0), 60.0, -10.0, theme::black(0.55 * t.clamp(0.0, 1.0)));
    widgets::frame(over.painter(), rect, 24.0, theme::surface_strong());
    paint::stroke_squircle(over.painter(), rect, theme::squircle(24.0), theme::hairline());

    let mut close = false;
    if closing.is_none() {
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        if scrim.clicked() && pointer.is_some_and(|p| !rect.contains(p)) {
            close = true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            close = true;
        }
    }
    let _ = panel_resp;
    if content > 0.0 {
        // Laid out in the final rect, moved with the panel's centre and clipped by it.
        let shift = rect.center() - target.center();
        let f = target.translate(shift);
        let mut inner = over.new_child(UiBuilder::new().max_rect(f).id_salt(("launcher-profile", &id)));
        inner.set_clip_rect(rect.shrink(1.0).intersect(page));
        inner.multiply_opacity(content);
        if body(&mut inner, cx, state, f, &profile, opened, clock) {
            close = true;
        }
    }
    if close {
        if let Some(m) = state.modal.as_mut() {
            m.closing = Some(clock);
        }
        state.act(cx, clock, Action::CloseProfile);
    }
}

/// The modal's content in `f`; returns whether × was pressed.
#[allow(clippy::too_many_arguments)]
fn body(ui: &mut Ui, cx: &mut Cx, state: &mut State, f: Rect, profile: &Profile, opened: f64, clock: f64) -> bool {
    let ctx = ui.ctx().clone();
    let friend = &profile.friend;
    let since = (clock - opened) as f32;
    let p = ui.painter().clone();

    // The banner settles from a little larger, the picture rises into place.
    let settle = anim::spring_from(&ctx, Id::unique(("launcher-profile-settle", opened.to_bits())), [0.0], [1.0], Spring::SOFT)[0];
    let band = Rect::from_min_size(f.min, vec2(f.width(), BANNER_H));
    match state.picture(&friend.look.banner) {
        Some(texture) => {
            let zoom = 1.0 + 0.06 * (1.0 - settle);
            let uv = cover_uv(texture.size_vec2(), band.size());
            let uv = Rect::from_center_size(uv.center(), uv.size() / zoom);
            paint::squircle_texture(&p, band, [24.0, 24.0, 0.0, 0.0], texture.id(), uv, Color32::WHITE);
        }
        None => {
            // Their banner's colour, else their own colour, as a soft wash.
            let c = friend.look.banner_colour.map_or_else(|| widgets::name_color(&friend.name), |[r, g, b]| Color32::from_rgb(r, g, b));
            paint::fill_convex(&p, &paint::rounded_rect_points_corners(band, [24.0, 24.0, 0.0, 0.0]), |at| {
                let x = (at.x - band.left()) / band.width();
                let y = (at.y - band.top()) / band.height();
                theme::mix(theme::alpha(c, 0.55), theme::alpha(theme::mix(c, theme::VIOLET, 0.5), 0.25), (x * 0.7 + y * 0.3).clamp(0.0, 1.0))
            });
        }
    }
    // The banner melts into the panel at its foot.
    let foot = Rect::from_min_max(pos2(band.left(), band.bottom() - 60.0), band.max);
    paint::fill_convex(&p, &[foot.left_top(), foot.right_top(), foot.right_bottom(), foot.left_bottom()], |at| {
        theme::alpha(theme::surface_strong(), ((at.y - foot.top()) / foot.height()).clamp(0.0, 1.0))
    });
    let x_rect = Rect::from_center_size(pos2(f.right() - 28.0, f.top() + 28.0), vec2(34.0, 34.0));
    let closed = widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Close")).size(34.0, 16.0).backdrop(theme::black(0.45)).id_salt("launcher-profile-close").show_at(ui, x_rect));

    let rise = 14.0 * (1.0 - settle);
    let pic = Rect::from_min_size(pos2(f.left() + 28.0, band.bottom() - PICTURE * 0.62 + rise), vec2(PICTURE, PICTURE));
    p.circle_filled(pic.center(), PICTURE * 0.5 + 5.0, theme::surface_strong());
    let picture = state.picture(&friend.look.icon).or_else(|| state.picture(&profile.picture));
    let frame = state.picture(&friend.look.icon_frame);
    person_picture(&p, pic, picture.as_ref(), frame.as_ref(), friend, theme::surface_strong());

    // Name, pronouns, VRC+, rank and platform.
    let x = pic.right() + 18.0;
    let name_y = band.bottom() + 10.0;
    let n = widgets::text_truncated(&p, pos2(x, name_y), Align2::LEFT_CENTER, &Txt::new(&friend.name, 24.0).weight(weight::SEMIBOLD).tracking(-0.02).color(rank_color(friend.look.rank)), f.right() - x - 120.0);
    let mut cx_ = n.right() + 10.0;
    if !profile.pronouns.is_empty() {
        cx_ = chip(&p, ui, pos2(cx_, name_y), &profile.pronouns, theme::FG_2, theme::white(0.07)) + 6.0;
    }
    if friend.look.supporter {
        chip(&p, ui, pos2(cx_, name_y), "VRC+", theme::WARN, theme::alpha(theme::WARN, 0.14));
    }
    let line_y = name_y + 26.0;
    let mut lx = x;
    lx = chip(&p, ui, pos2(lx, line_y), rank_name(friend.look.rank), rank_color(friend.look.rank), theme::alpha(rank_color(friend.look.rank), 0.14)) + 8.0;
    if let Some((glyph, name)) = platform(friend.platform) {
        widgets::icon(&p, pos2(lx + 7.0, line_y), glyph, 13.0, theme::FG_3);
        widgets::text(&p, pos2(lx + 18.0, line_y), Align2::LEFT_CENTER, &Txt::new(name, 12.0).color(theme::FG_3));
    }
    // Their status.
    let status_y = pic.bottom() + 22.0;
    let dot = pos2(f.left() + 34.0, status_y);
    p.circle_filled(dot, 5.0, status_color(friend.status));
    let s = widgets::text(&p, pos2(dot.x + 12.0, status_y), Align2::LEFT_CENTER, &Txt::new(status_name(friend.status), 13.5).weight(weight::SEMIBOLD));
    if !friend.status_text.is_empty() {
        widgets::text_truncated(&p, pos2(s.right() + 8.0, status_y), Align2::LEFT_CENTER, &Txt::new(format!("· {}", friend.status_text), 13.5).color(theme::FG_2), f.right() - s.right() - 36.0);
    }

    // What follows scrolls, its sections coming in one after the other.
    let body = Rect::from_min_max(pos2(f.left(), status_y + 20.0), f.max);
    let mut child = ui.new_child(UiBuilder::new().max_rect(body).id_salt("launcher-profile-body"));
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt(("launcher-profile-scroll", &friend.id)).auto_shrink([false, false]).show(&mut child, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 24, top: 8, bottom: 24 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 14.0);
            let mut n = 0;
            let mut section = |ui: &mut Ui, height: f32, f: &mut dyn FnMut(&mut Ui, Rect)| {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
                let delay = 0.12 + n as f32 * 0.06;
                n += 1;
                let k = anim::spring_from(&ctx, Id::unique(("launcher-profile-section", opened.to_bits(), n)), [0.0], [if since > delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
                if since <= delay {
                    super::super::frame_in(&ctx, (delay - since) as f64);
                }
                let r = rect.translate(vec2(0.0, 12.0 * (1.0 - k)));
                let mut c = ui.new_child(UiBuilder::new().max_rect(r));
                c.multiply_opacity(k.clamp(0.0, 1.0));
                f(&mut c, r);
            };
            section(ui, 104.0, &mut |ui, r| place_card(ui, cx, state, r, friend, clock));
            section(ui, 44.0, &mut |ui, r| boop_row(ui, cx, state, r, friend, clock));
            if profile.loading {
                section(ui, 58.0, &mut |ui, r| shimmer(ui, r, clock));
            }
            if !profile.bio.trim().is_empty() {
                let txt = Txt::new(profile.bio.trim(), 13.5).color(theme::FG_2).line_height(20.0);
                let galley = ui.painter().layout_job(txt.job(ui.available_width()));
                let h = galley.size().y + 24.0;
                section(ui, h, &mut |ui, r| {
                    widgets::text(ui.painter(), r.left_top() + vec2(0.0, 6.0), Align2::LEFT_CENTER, &Txt::caps(tr("Bio")));
                    ui.painter().galley(r.left_top() + vec2(0.0, 18.0), galley.clone(), theme::FG_2);
                });
            }
            if !profile.links.is_empty() {
                section(ui, 34.0, &mut |ui, r| links(ui, r, &profile.links));
            }
            let facts = facts(state, profile);
            if !facts.is_empty() {
                let rows = facts.len().div_ceil(2);
                section(ui, rows as f32 * 50.0 + 4.0, &mut |ui, r| fact_grid(ui, r, &facts));
            }
            if !profile.badges.is_empty() {
                section(ui, 58.0, &mut |ui, r| badges(ui, state, r, profile));
            }
            if !friend.note.trim().is_empty() {
                section(ui, 52.0, &mut |ui, r| {
                    let p = ui.painter();
                    widgets::frame(p, r, 12.0, theme::alpha(theme::WARN, 0.08));
                    widgets::icon(p, pos2(r.left() + 20.0, r.center().y), icons::NOTE, 15.0, theme::WARN);
                    widgets::text(p, pos2(r.left() + 38.0, r.top() + 15.0), Align2::LEFT_CENTER, &Txt::caps(tr("Your note")));
                    widgets::text_truncated(p, pos2(r.left() + 38.0, r.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(friend.note.trim(), 13.0).color(theme::FG), r.width() - 52.0);
                });
            }
        });
    });
    // What scrolls on fades out at the panel's foot instead of being cut.
    let fade = Rect::from_min_max(pos2(body.left(), body.bottom() - 28.0), pos2(body.right() - 10.0, body.bottom() - 1.0));
    paint::fill_convex(&p, &[fade.left_top(), fade.right_top(), fade.right_bottom(), fade.left_bottom()], |at| {
        theme::alpha(theme::surface_strong(), ((at.y - fade.top()) / fade.height()).clamp(0.0, 1.0))
    });
    closed
}

/// A small rounded tag at `left_center`; returns its right edge.
fn chip(p: &egui::Painter, ui: &Ui, left_center: Pos2, text: &str, fg: Color32, bg: Color32) -> f32 {
    let txt = Txt::new(text, 11.5).weight(weight::SEMIBOLD).color(fg);
    let w = widgets::text_size(ui, &txt).x + 16.0;
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - 10.0), vec2(w, 20.0));
    p.rect_filled(r, theme::radius(10.0), bg);
    widgets::text(p, r.center(), Align2::CENTER_CENTER, &txt);
    r.right()
}

fn platform(platform: Platform) -> Option<(&'static str, &'static str)> {
    match platform {
        Platform::Pc => Some((icons::DESKTOP, tr_ctx("platform", "PC"))),
        Platform::Android => Some((icons::VIRTUAL_REALITY, tr_ctx("platform", "Quest / Android"))),
        Platform::Ios => Some((icons::DEVICE_MOBILE, tr_ctx("platform", "iPhone / iPad"))),
        Platform::Web => Some((icons::GLOBE_HEMISPHERE_WEST, tr_ctx("platform", "Website"))),
        Platform::Unknown => None,
    }
}

/// Where they are, with the world's picture, and Request invite beside it.
fn place_card(ui: &mut Ui, cx: &mut Cx, state: &mut State, r: Rect, friend: &Friend, clock: f64) {
    let p = ui.painter().clone();
    widgets::frame(&p, r, 16.0, theme::surface_soft());
    let pic = Rect::from_min_size(r.min + vec2(12.0, 12.0), vec2((r.height() - 24.0) * 16.0 / 9.0, r.height() - 24.0));
    let (title, sub, world_pic, glyph) = match &friend.whereabouts {
        Whereabouts::World(place) => {
            let mut sub = access_name(place.access).to_string();
            if !place.region.is_empty() {
                sub = format!("{sub} · {}", region_tag(&place.region));
            }
            let name = if place.world_name.is_empty() { tr("A world").to_string() } else { place.world_name.clone() };
            (name, sub, state.picture(&place.world_picture), access_icon(place.access))
        }
        Whereabouts::Private => (tr("In a private instance").to_string(), tr("Ask them for an invite to join").to_string(), None, icons::LOCK_SIMPLE),
        Whereabouts::Traveling => (tr("Traveling…").to_string(), tr("Between two worlds").to_string(), None, icons::PAPER_PLANE_TILT),
        Whereabouts::Website => (tr("On the website").to_string(), tr("Not in the game right now").to_string(), None, icons::GLOBE_HEMISPHERE_WEST),
        Whereabouts::Offline | Whereabouts::Unknown => {
            let sub = match friend.last_seen {
                Some(at) => orbiters_i18n::tr!("Last seen {when}", when = super::super::ago(ui.ctx(), at.max(0) as u64, state.now().max(0) as u64)),
                None => String::new(),
            };
            (tr_ctx("vrchat status", "Offline").to_string(), sub, None, icons::MOON)
        }
    };
    match &world_pic {
        Some(t) => paint::squircle_texture(&p, pic, [10.0; 4], t.id(), cover_uv(t.size_vec2(), pic.size()), Color32::WHITE),
        None => {
            widgets::frame(&p, pic, 10.0, theme::white(0.05));
            widgets::icon(&p, pic.center(), glyph, 22.0, theme::FG_4);
        }
    }
    let x = pic.right() + 14.0;
    let now = state.now();
    let reach = states::request(&state.data, friend, state.requested.get(&friend.id).copied(), clock, now);
    let label = tr("Request invite");
    let pill_w = match reach {
        Reach::Ready | Reach::Sent => widgets::text_size(ui, &Txt::new(label, 12.5).weight(weight::SEMIBOLD)).x + 46.0,
        Reach::Cooling(_) => 80.0,
        _ => 0.0,
    };
    // Join where the instance takes anyone it shows to the account (public, friends+, friends, a group's public or
    // plus): the Play path with that instance (the running VRChat's own way when one runs). Private ones: ask.
    let join = match &friend.whereabouts {
        Whereabouts::World(place)
            if state.data.possible.play
                && !matches!(state.data.game, orbiters_core::launcher::Game::NotInstalled | orbiters_core::launcher::Game::NoEngine)
                && matches!(place.access, InstanceAccess::Public | InstanceAccess::FriendsPlus | InstanceAccess::Friends | InstanceAccess::GroupPublic | InstanceAccess::GroupPlus) =>
        {
            Some(place.clone())
        }
        _ => None,
    };
    let join_label = tr("Join");
    let join_w = if join.is_some() { widgets::text_size(ui, &Txt::new(join_label, 12.5).weight(weight::SEMIBOLD)).x + 46.0 } else { 0.0 };
    let room = r.right() - x - 14.0 - if pill_w > 0.0 { pill_w + 10.0 } else { 0.0 } - if join_w > 0.0 { join_w + 8.0 } else { 0.0 };
    widgets::text(&p, pos2(x, r.top() + 22.0), Align2::LEFT_CENTER, &Txt::caps(tr("Where they are")));
    widgets::text_truncated(&p, pos2(x, r.center().y + 1.0), Align2::LEFT_CENTER, &Txt::new(title, 16.0).weight(weight::SEMIBOLD), room);
    widgets::text_truncated(&p, pos2(x, r.bottom() - 22.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::FG_3), room);
    if pill_w > 0.0 {
        let pr = Rect::from_min_size(pos2(r.right() - 14.0 - pill_w, r.center().y - 18.0), vec2(pill_w, 36.0));
        if reach_pill(ui, pr, ("launcher-modal-request", friend.id.as_str()), reach, icons::PAPER_PLANE_TILT, label, state.requested.get(&friend.id).copied(), clock) {
            state.act(cx, clock, Action::RequestInvite { to: friend.id.clone() });
        }
    }
    if let Some(place) = join {
        let right = r.right() - 14.0 - if pill_w > 0.0 { pill_w + 8.0 } else { 0.0 };
        let jr = Rect::from_min_size(pos2(right - join_w, r.center().y - 18.0), vec2(join_w, 36.0));
        if reach_pill(ui, jr, ("launcher-modal-join", friend.id.as_str()), Reach::Ready, icons::PLAY, join_label, None, clock) {
            state.act(cx, clock, Action::Play { mode: cx.snap.settings.launch.last_mode, destination: Some(place) });
        }
    }
}

/// Boop: the hand boops at once; the emojis beside it boop with that emoji. Without the sign-in, one line says so.
fn boop_row(ui: &mut Ui, cx: &mut Cx, state: &mut State, r: Rect, friend: &Friend, clock: f64) {
    let now = state.now();
    let booped = state.booped.get(&friend.id).cloned();
    let reach = states::boop(&state.data, friend, booped.as_ref().map(|b| b.0), clock, now);
    let p = ui.painter().clone();
    if reach == Reach::SignIn {
        widgets::icon(&p, pos2(r.left() + 10.0, r.center().y), icons::INFO, 15.0, theme::BLUE);
        let t = tr("Sign in to VRChat to boop and ask for invites.");
        widgets::text_truncated(&p, pos2(r.left() + 26.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(t, 13.0).color(theme::FG_2), r.width() - 180.0);
        let b = widgets::Button::new(tr("Sign in to VRChat")).variant(widgets::Variant::Primary).size(widgets::Size::Sm).id_salt("launcher-modal-sign-in");
        let w = b.desired_width(ui);
        if b.show_at(ui, Rect::from_min_size(pos2(r.right() - w, r.center().y - 16.0), vec2(w, 32.0))).clicked() {
            cx.go = Some(super::super::Go::VrchatAccount);
        }
        return;
    }
    // The hand, bigger here, and its word.
    let c = pos2(r.left() + 22.0, r.center().y);
    if boop_button(ui, c, 44.0, ("launcher-modal-boop", friend.id.as_str()), reach, booped.clone(), clock) {
        state.act(cx, clock, Action::Boop { to: friend.id.clone(), emoji: None });
    }
    let word = match reach {
        Reach::Sent => tr("Booped!").to_string(),
        Reach::Cooling(left) => orbiters_i18n::tr!("Boop again in {n} s", n = left),
        _ => tr("Boop").to_string(),
    };
    let color = if reach == Reach::Sent { theme::ACCENT } else { theme::FG };
    let wr = widgets::text(&p, pos2(c.x + 32.0, c.y), Align2::LEFT_CENTER, &Txt::new(word, 14.0).weight(weight::SEMIBOLD).color(color));
    // The emojis: one press, the same boop with it.
    if reach.pressable() && !state.data.boop_emojis.is_empty() {
        let mut x = wr.right().max(c.x + 110.0) + 12.0;
        widgets::text(&p, pos2(x, c.y), Align2::LEFT_CENTER, &Txt::new(tr("or with"), 12.0).color(theme::FG_4));
        x += 52.0;
        let emojis = state.data.boop_emojis.clone();
        for e in emojis {
            if x + 36.0 > r.right() {
                break;
            }
            let er = Rect::from_center_size(pos2(x + 17.0, c.y), vec2(34.0, 34.0));
            let id = Id::unique(("launcher-emoji", &friend.id, &e.id));
            let resp = ui.interact(er, id, Sense::click());
            let h = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::BOUNCY);
            let er2 = Rect::from_center_size(er.center() - vec2(0.0, 3.0 * h), er.size() * (1.0 + 0.12 * h));
            p.circle_filled(er2.center(), er2.width() * 0.5, theme::white(0.06 + 0.06 * h));
            match state.picture(&e.picture) {
                Some(t) => paint::circle_texture(&p, er2.shrink(5.0), t.id(), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE),
                None => widgets::icon_fill(&p, er2.center(), emoji_icon(if e.glyph.is_empty() { &e.id } else { &e.glyph }), 17.0, emoji_tint(&e.id)),
            }
            if widgets::activated(&resp.on_hover_cursor(CursorIcon::PointingHand)) {
                state.act(cx, clock, Action::Boop { to: friend.id.clone(), emoji: Some(e.id.clone()) });
            }
            x += 40.0;
        }
    } else if let Some((at, emoji)) = booped.filter(|b| clock - b.0 < states::BOOP_FOR) {
        burst(&p, c, ((clock - at) / states::BOOP_FOR) as f32, emoji.as_deref());
    }
}

fn emoji_tint(id: &str) -> Color32 {
    let palette = [theme::WARN, theme::REC, theme::CYAN, theme::VIOLET, theme::ACCENT];
    palette[id.bytes().map(usize::from).sum::<usize>() % palette.len()]
}

fn shimmer(ui: &Ui, r: Rect, clock: f64) {
    let p = ui.painter();
    let t = ((clock * 0.8) % 1.0) as f32;
    for (i, w) in [0.9f32, 0.75, 0.5].iter().enumerate() {
        let bar = Rect::from_min_size(r.left_top() + vec2(0.0, 6.0 + i as f32 * 18.0), vec2(r.width() * w, 10.0));
        let a = 0.05 + 0.04 * ((t * std::f32::consts::TAU + i as f32).sin() * 0.5 + 0.5);
        p.rect_filled(bar, theme::radius(5.0), theme::white(a));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

fn links(ui: &mut Ui, r: Rect, links: &[String]) {
    let mut x = r.left();
    for link in links.iter().take(4) {
        let host = link.split("://").nth(1).unwrap_or(link).split('/').next().unwrap_or(link).trim_start_matches("www.").to_string();
        let txt = Txt::new(&host, 12.5).weight(weight::MEDIUM).color(theme::FG);
        let w = widgets::text_size(ui, &txt).x + 38.0;
        if x + w > r.right() {
            break;
        }
        let chip = Rect::from_min_size(pos2(x, r.top() + 2.0), vec2(w, 30.0));
        let id = Id::unique(("launcher-link", link));
        let resp = ui.interact(chip, id, Sense::click());
        let h = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let p = ui.painter();
        p.rect_filled(chip, theme::radius(15.0), theme::white(0.06 + 0.06 * h));
        widgets::icon(p, pos2(chip.left() + 15.0, chip.center().y), icons::LINK_SIMPLE, 13.0, theme::CYAN);
        widgets::text(p, pos2(chip.left() + 27.0, chip.center().y), Align2::LEFT_CENTER, &txt);
        if resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(link).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(link));
        }
        x += w + 8.0;
    }
}

/// The facts in two columns: when last seen, joined, friends since, in common, languages, the groups they are in.
fn facts(state: &State, profile: &Profile) -> Vec<(&'static str, &'static str, String)> {
    let mut out = Vec::new();
    let now = state.now().max(0) as u64;
    let f = &profile.friend;
    if let (Some(at), orbiters_core::launcher::Presence::Offline) = (f.last_seen, f.presence) {
        out.push((icons::CLOCK, tr("Last seen"), orbiters_ui::format::relative_time(at.max(0) as u64, now)));
    }
    if !profile.date_joined.is_empty() {
        if let Some(day) = day_of(&profile.date_joined) {
            out.push((icons::CALENDAR_BLANK, tr("Joined VRChat"), orbiters_ui::format::relative_time(day, now)));
        }
    }
    if let Some(at) = profile.friends_since {
        out.push((icons::HEART, tr("Friends since"), orbiters_ui::format::relative_time(at.max(0) as u64, now)));
    }
    if let Some(n) = profile.mutual_friends {
        out.push((icons::USERS, tr("Friends in common"), n.to_string()));
    }
    if let Some(n) = profile.mutual_groups {
        out.push((icons::USERS_THREE, tr("Groups in common"), n.to_string()));
    }
    if !profile.languages.is_empty() {
        out.push((icons::TRANSLATE, tr("Speaks"), profile.languages.iter().map(|l| language_name(l)).collect::<Vec<_>>().join(", ")));
    }
    let groups = super::list::favourite_names(&state.data, f);
    if !groups.is_empty() {
        out.push((icons::STAR, tr("In your favourites"), groups.join(", ")));
    }
    out
}

/// `YYYY-MM-DD` as Unix seconds (that day's midnight, UTC).
fn day_of(date: &str) -> Option<u64> {
    let mut parts = date.get(..10)?.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400).ok()
}

fn language_name(code: &str) -> String {
    match code {
        "eng" => tr_ctx("language", "English").into(),
        "fra" => tr_ctx("language", "French").into(),
        "spa" => tr_ctx("language", "Spanish").into(),
        "por" => tr_ctx("language", "Portuguese").into(),
        "jpn" => tr_ctx("language", "Japanese").into(),
        "deu" => tr_ctx("language", "German").into(),
        "kor" => tr_ctx("language", "Korean").into(),
        "zho" => tr_ctx("language", "Chinese").into(),
        "rus" => tr_ctx("language", "Russian").into(),
        "ase" => tr_ctx("language", "American Sign Language").into(),
        other => other.to_uppercase(),
    }
}

fn fact_grid(ui: &mut Ui, r: Rect, facts: &[(&'static str, &'static str, String)]) {
    let p = ui.painter();
    let gap = 12.0;
    let w = (r.width() - gap) * 0.5;
    for (i, (glyph, label, value)) in facts.iter().enumerate() {
        let cell = Rect::from_min_size(r.min + vec2((i % 2) as f32 * (w + gap), (i / 2) as f32 * 50.0), vec2(w, 44.0));
        widgets::frame(p, cell, 12.0, theme::white(0.035));
        widgets::icon(p, pos2(cell.left() + 18.0, cell.center().y), glyph, 15.0, theme::FG_3);
        widgets::text_truncated(p, pos2(cell.left() + 36.0, cell.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(*label, 11.0).color(theme::FG_4), w - 44.0);
        widgets::text_truncated(p, pos2(cell.left() + 36.0, cell.top() + 30.0), Align2::LEFT_CENTER, &Txt::new(value, 13.0).weight(weight::MEDIUM), w - 44.0);
    }
}

fn badges(ui: &mut Ui, state: &mut State, r: Rect, profile: &Profile) {
    widgets::text(ui.painter(), pos2(r.left(), r.top() + 6.0), Align2::LEFT_CENTER, &Txt::caps(tr("Badges")));
    let mut x = r.left();
    for b in &profile.badges {
        let picture = state.picture(&b.picture);
        let p = ui.painter();
        let txt = Txt::new(&b.name, 12.0).weight(weight::MEDIUM);
        let w = widgets::text_size(ui, &txt).x + 40.0;
        if x + w > r.right() {
            break;
        }
        let chip = Rect::from_min_size(pos2(x, r.top() + 22.0), vec2(w, 30.0));
        p.rect_filled(chip, theme::radius(15.0), theme::white(0.06));
        let c = pos2(chip.left() + 16.0, chip.center().y);
        match picture {
            Some(t) => paint::circle_texture(p, Rect::from_center_size(c, vec2(20.0, 20.0)), t.id(), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE),
            None => widgets::icon_fill(p, c, icons::SEAL_CHECK, 15.0, theme::CYAN),
        }
        widgets::text(p, pos2(chip.left() + 30.0, chip.center().y), Align2::LEFT_CENTER, &txt);
        ui.interact(chip, Id::unique(("launcher-badge", &b.id)), Sense::hover()).on_hover_text(&b.description);
        x += w + 8.0;
    }
}
