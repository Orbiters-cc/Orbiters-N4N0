//! The column's parts: the account's head, the line about the sign-in, what was received, the list's rows (only those
//! in view), and the rail of pictures a narrow window has instead.

use super::list::{self, Row};
use super::states::{self, Reach};
use super::{banner, column_fill, person_picture, rank_color, status_name, where_line, Cx, State};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_core::launcher::{Action, Friend, GroupKind, InviteKind, Presence, Whereabouts};
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};

/// The account itself at the top: picture, name, status; Read again (and Close over the page).
pub(super) fn me(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, drawer: bool, clock: f64) {
    let me = state.data.me.clone();
    if let Some(texture) = me.as_ref().and_then(|m| state.picture(&m.look.banner)) {
        banner(ui.painter(), rect, 14.0, &texture, column_fill(), 0.15, 0.85);
    }
    let pic = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 21.0), vec2(42.0, 42.0));
    let text_x = pic.right() + 12.0;
    let right_room = if drawer { 76.0 } else { 42.0 };
    let max_w = rect.right() - text_x - right_room;
    let p = ui.painter().clone();
    match &me {
        Some(me) => {
            let picture = state.picture(&me.look.icon);
            person_picture(&p, pic, picture.as_ref(), me, column_fill());
            widgets::text_truncated(&p, pos2(text_x, rect.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(&me.name, 15.0).weight(weight::SEMIBOLD), max_w);
            let line = if me.status_text.is_empty() { status_name(me.status).to_string() } else { me.status_text.clone() };
            widgets::text_truncated(&p, pos2(text_x, rect.center().y + 11.0), Align2::LEFT_CENTER, &Txt::new(line, 12.0).color(theme::FG_2), max_w);
        }
        None => {
            p.circle_filled(pic.center(), 21.0, theme::control());
            widgets::icon(&p, pic.center(), icons::USER, 19.0, theme::FG_3);
            widgets::text(&p, pos2(text_x, rect.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(tr("Friends"), 15.0).weight(weight::SEMIBOLD));
            let line = if state.data.loading { tr("Reading your friends…") } else { "VRChat" };
            widgets::text(&p, pos2(text_x, rect.center().y + 11.0), Align2::LEFT_CENTER, &Txt::new(line, 12.0).color(theme::FG_3));
        }
    }
    let mut x = rect.right() - 20.0;
    if drawer {
        let close = Rect::from_center_size(pos2(x, rect.center().y), vec2(32.0, 32.0));
        if widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Close")).size(32.0, 15.0).id_salt("launcher-drawer-close").show_at(ui, close)) {
            state.drawer = None;
        }
        x -= 36.0;
    }
    if matches!(states::live(&state.data), states::Live::SignedIn) {
        let refresh = Rect::from_center_size(pos2(x, rect.center().y), vec2(32.0, 32.0));
        if widgets::activated(&widgets::IconButton::new(icons::ARROW_CLOCKWISE, orbiters_i18n::mark("Read again")).size(32.0, 15.0).id_salt("launcher-refresh").show_at(ui, refresh)) {
            state.act(cx, clock, Action::Refresh);
        }
    }
}

/// What needs the VRChat sign-in, said once at the head of the list, with the way to sign in. Returns where the next
/// part starts.
pub(super) fn sign_in_note(ui: &mut Ui, cx: &mut Cx, state: &mut State, inner: Rect, y: f32) -> f32 {
    let text = match (states::live(&state.data), &state.data.source) {
        (states::Live::SignedIn, _) => return y,
        (states::Live::Copy, orbiters_core::launcher::Source::Database { app, read_at }) => {
            let app = match app {
                orbiters_core::launcher::DatabaseApp::Vrcx => "VRCX",
                orbiters_core::launcher::DatabaseApp::VrcNext => "VRCNext",
            };
            let when = super::super::ago(ui.ctx(), (*read_at).max(0) as u64, state.now().max(0) as u64);
            (
                orbiters_i18n::tr!("From {app}'s copy, {when}", app = app, when = when),
                tr("Live status, invites and boops need your VRChat sign-in.").to_string(),
            )
        }
        _ => (tr("Your friends will show here").to_string(), tr("Sign in to VRChat in N4N0, or keep VRCX or VRCNext on this computer: N4N0 reads their copy.").to_string()),
    };
    let w = inner.width() - 4.0;
    let body = Txt::new(&text.1, 12.0).color(theme::FG_2).line_height(16.0);
    let galley = ui.painter().layout_job(body.job(w - 28.0));
    let h = 14.0 + 18.0 + galley.size().y + 12.0 + 32.0 + 12.0;
    let card = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(w, h));
    let p = ui.painter().clone();
    widgets::frame(&p, card, 14.0, theme::alpha(theme::BLUE, 0.1));
    paint::stroke_squircle(&p, card, theme::squircle(14.0), egui::Stroke::new(1.0, theme::alpha(theme::BLUE, 0.22)));
    widgets::icon(&p, pos2(card.left() + 22.0, card.top() + 23.0), icons::INFO, 15.0, theme::BLUE);
    widgets::text_truncated(&p, pos2(card.left() + 36.0, card.top() + 23.0), Align2::LEFT_CENTER, &Txt::new(&text.0, 13.0).weight(weight::SEMIBOLD), w - 48.0);
    p.galley(pos2(card.left() + 14.0, card.top() + 14.0 + 18.0 + 4.0), galley, theme::FG_2);
    let button = Rect::from_min_size(pos2(card.left() + 14.0, card.bottom() - 12.0 - 32.0), vec2(0.0, 32.0));
    let b = widgets::Button::new(tr("Sign in to VRChat")).icon(icons::SIGN_IN).variant(widgets::Variant::Primary).size(widgets::Size::Sm).id_salt("launcher-sign-in");
    let bw = b.desired_width(ui);
    if b.show_at(ui, Rect::from_min_size(button.min, vec2(bw, 32.0))).clicked() {
        cx.go = Some(super::super::Go::VrchatAccount);
    }
    y + h + 10.0
}

/// Boops and invite requests received, newest first: two at most, then how many more.
pub(super) fn received(ui: &mut Ui, cx: &mut Cx, state: &mut State, inner: Rect, mut y: f32, clock: f64) -> f32 {
    enum Got {
        Boop { id: String, from: String, name: String, icon: String, at: i64 },
        Request { id: String, name: String, icon: String, at: i64 },
    }
    let mut got: Vec<Got> = Vec::new();
    for b in state.data.boops.iter().filter(|b| !state.dismissed.contains(&b.id)) {
        got.push(Got::Boop { id: b.id.clone(), from: b.from_id.clone(), name: b.from_name.clone(), icon: b.from_icon.clone(), at: b.received_at });
    }
    for i in state.data.invites.iter().filter(|i| i.kind == InviteKind::Request && !state.dismissed.contains(&i.id)) {
        got.push(Got::Request { id: i.id.clone(), name: i.from_name.clone(), icon: i.from_icon.clone(), at: i.received_at });
    }
    got.sort_by_key(|g| std::cmp::Reverse(match g {
        Got::Boop { at, .. } | Got::Request { at, .. } => *at,
    }));
    let more = got.len().saturating_sub(2);
    let now = state.now();
    for (n, g) in got.into_iter().take(2).enumerate() {
        let rect = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 48.0));
        let (enter, dy) = widgets::entrance(ui, Id::unique(("launcher-got", n, match &g {
            Got::Boop { id, .. } | Got::Request { id, .. } => id.clone(),
        })), 0.0, 0);
        let rect = rect.translate(vec2(0.0, dy * 0.4));
        let mut p = ui.painter().clone();
        p.multiply_opacity(enter);
        let accent = matches!(g, Got::Boop { .. });
        widgets::frame(&p, rect, 14.0, if accent { theme::alpha(theme::ACCENT, 0.09) } else { theme::surface_soft() });
        let (name, icon, at, id) = match &g {
            Got::Boop { name, icon, at, id, .. } | Got::Request { name, icon, at, id } => (name.clone(), icon.clone(), *at, id.clone()),
        };
        let pic = Rect::from_center_size(pos2(rect.left() + 24.0, rect.center().y), vec2(30.0, 30.0));
        let texture = state.picture(&icon);
        widgets::avatar(&p, pic, texture.as_ref(), &name);
        if accent {
            // A little hand on the picture's corner says what this is at a glance.
            let c = pic.right_bottom() - vec2(3.0, 3.0);
            p.circle_filled(c, 9.0, theme::INK_900);
            widgets::icon(&p, c, icons::HAND_WAVING, 11.0, theme::ACCENT);
        }
        let what = if accent { tr("booped you") } else { tr("asks for an invite") };
        let x = pic.right() + 10.0;
        let room = rect.right() - x - if accent { 142.0 } else { 40.0 };
        widgets::text_truncated(&p, pos2(x, rect.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(&name, 13.0).weight(weight::SEMIBOLD), room);
        let when = super::super::ago(ui.ctx(), at.max(0) as u64, now.max(0) as u64);
        widgets::text_truncated(&p, pos2(x, rect.center().y + 9.0), Align2::LEFT_CENTER, &Txt::new(format!("{what} · {when}"), 11.5).color(theme::FG_3), room);
        let close = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(28.0, 28.0));
        if widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Dismiss")).size(28.0, 13.0).id_salt(("launcher-got-x", &id)).show_at(ui, close)) {
            state.act(cx, clock, Action::Dismiss { id: id.clone() });
        }
        if let Got::Boop { from, .. } = &g {
            let friend = state.data.friend(from).cloned();
            let reach = match &friend {
                Some(f) => states::boop(&state.data, f, state.booped.get(from).map(|b| b.0), clock, now),
                None if state.data.possible.boop => Reach::Ready,
                None => Reach::SignIn,
            };
            let at = Rect::from_min_size(pos2(close.left() - 4.0 - 96.0, rect.center().y - 15.0), vec2(96.0, 30.0));
            if reach_pill(ui, at, ("launcher-boop-back", from.as_str()), reach, icons::HAND_WAVING, tr("Boop back"), state.booped.get(from).map(|b| b.0), clock) {
                state.act(cx, clock, Action::Boop { to: from.clone(), emoji: None });
                state.act(cx, clock, Action::Dismiss { id: id.clone() });
            }
        }
        y += 48.0 + 6.0;
    }
    if more > 0 {
        let t = orbiters_i18n::tr!("{n} more", n = more);
        widgets::text(ui.painter(), pos2(inner.left() + 10.0, y + 6.0), Align2::LEFT_CENTER, &Txt::new(t, 11.5).color(theme::FG_4));
        y += 18.0;
    }
    if y > inner.top() {
        y + 4.0
    } else {
        y
    }
}

/// A pill for something that reaches a person: its label while it can be pressed, "Sent" a moment after, the
/// seconds left while it cools down; nothing for what is not possible now. Returns whether it was pressed.
#[allow(clippy::too_many_arguments)]
pub(super) fn reach_pill(ui: &mut Ui, rect: Rect, salt: (&str, &str), reach: Reach, glyph: &str, label: &str, sent_at: Option<f64>, clock: f64) -> bool {
    let id = Id::unique(salt);
    match reach {
        Reach::NotNow | Reach::SignIn => false,
        Reach::Ready => {
            let resp = ui.interact(rect, id, Sense::click());
            let pressed = widgets::activated(&resp);
            widgets::note_control(ui.ctx(), label, rect, true);
            let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
            let down = anim::toggle(ui.ctx(), id.with("down"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.05 * down));
            let p = ui.painter();
            p.rect_filled(r, theme::radius(r.height() * 0.5), theme::mix(theme::ACCENT, theme::ACCENT_HOVER, hover));
            pill_content(p, r, glyph, label, theme::ON_ACCENT);
            resp.on_hover_cursor(CursorIcon::PointingHand);
            pressed
        }
        Reach::Sent => {
            // The press lands: the pill fills with a light green and its check pops in.
            let t = sent_at.map_or(1.0, |at| ((clock - at) / 0.35).clamp(0.0, 1.0) as f32);
            let p = ui.painter();
            p.rect_filled(rect, theme::radius(rect.height() * 0.5), theme::alpha(theme::ACCENT, 0.16));
            let pop = 1.0 + 0.25 * (1.0 - t) * (t * std::f32::consts::PI).sin();
            let r = Rect::from_center_size(rect.center(), rect.size() * pop.min(1.08));
            pill_content(p, r, icons::CHECK, tr_ctx("sent", "Sent"), theme::ACCENT);
            if t < 1.0 {
                ui.ctx().request_repaint();
            }
            super::super::frame_in(ui.ctx(), sent_at.map_or(0.0, |at| states::SENT_FOR - (clock - at)));
            false
        }
        Reach::Cooling(left) => {
            let p = ui.painter();
            p.rect_filled(rect, theme::radius(rect.height() * 0.5), theme::white(0.05));
            pill_content(p, rect, icons::HOURGLASS_MEDIUM, &orbiters_i18n::tr!("{n} s", n = left), theme::FG_3);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(1000));
            let resp = ui.interact(rect, id.with("cool"), Sense::hover());
            resp.on_hover_text(orbiters_i18n::tr!("Again in {n} s: one at a time keeps it fun", n = left));
            false
        }
    }
}

fn pill_content(p: &egui::Painter, r: Rect, glyph: &str, label: &str, color: Color32) {
    let txt = Txt::new(label, 12.5).weight(weight::SEMIBOLD).color(color);
    let w = p.layout_job(txt.job(f32::INFINITY)).size().x + 20.0;
    let x = r.center().x - w * 0.5;
    widgets::icon(p, pos2(x + 7.0, r.center().y), glyph, 14.0, color);
    widgets::text(p, pos2(x + 20.0, r.center().y), Align2::LEFT_CENTER, &txt);
}

/// How wide a pill with this label is.
fn pill_width(ui: &Ui, label: &str) -> f32 {
    widgets::text_size(ui, &Txt::new(label, 12.5).weight(weight::SEMIBOLD)).x + 20.0 + 26.0
}

/// The round boop button: one press boops; its hand waves and little hearts burst from it.
pub(super) fn boop_button(ui: &mut Ui, center: egui::Pos2, size: f32, salt: (&str, &str), reach: Reach, booped: Option<(f64, Option<String>)>, clock: f64) -> bool {
    let id = Id::unique(salt);
    let rect = Rect::from_center_size(center, vec2(size, size));
    let ctx = ui.ctx().clone();
    let mut pressed = false;
    let (fill, fg) = match reach {
        Reach::Ready => {
            let resp = ui.interact(rect, id, Sense::click());
            pressed = widgets::activated(&resp);
            widgets::note_control(&ctx, orbiters_i18n::mark("Boop"), rect, true);
            let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
            resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Boop"));
            (theme::mix(theme::white(0.1), theme::alpha(theme::ACCENT, 0.3), hover), theme::mix(theme::FG, theme::ACCENT, hover))
        }
        Reach::Sent => (theme::alpha(theme::ACCENT, 0.2), theme::ACCENT),
        Reach::Cooling(left) => {
            let resp = ui.interact(rect, id.with("cool"), Sense::hover());
            resp.on_hover_text(orbiters_i18n::tr!("Boop again in {n} s", n = left));
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
            (theme::white(0.05), theme::FG_4)
        }
        Reach::NotNow | Reach::SignIn => return false,
    };
    let p = ui.painter().clone();
    let since = booped.as_ref().map(|b| clock - b.0).filter(|t| *t < states::BOOP_FOR);
    let squash = since.map_or(0.0, |t| (1.0 - (t / 0.3).min(1.0)) as f32);
    p.circle_filled(center, size * 0.5 * (1.0 - 0.12 * squash), fill);
    let wave = since.map_or(0.0, |t| ((t * 28.0).sin() * (1.0 - t / states::BOOP_FOR)) as f32);
    widgets::icon(&p, center + vec2(2.5 * wave, -1.0 * wave.abs()), icons::HAND_WAVING, size * 0.5, fg);
    if let (Some(t), Some((_, emoji))) = (since, booped.as_ref()) {
        burst(&p, center, (t / states::BOOP_FOR) as f32, emoji.as_deref());
        ctx.request_repaint();
    }
    pressed
}

/// Little hearts (or the emoji sent) rising from a boop and fading.
pub(super) fn burst(p: &egui::Painter, center: egui::Pos2, t: f32, emoji: Option<&str>) {
    let glyph = emoji.map_or(icons::HEART, emoji_icon);
    let e = anim::ease::out_cubic(t);
    for k in 0..7 {
        let a = -std::f32::consts::FRAC_PI_2 + (k as f32 - 3.0) * 0.42;
        let reach = 18.0 + 22.0 * e + (k % 2) as f32 * 6.0 * e;
        let at = center + egui::Vec2::angled(a) * reach - vec2(0.0, 6.0 * e);
        let fade = (1.0 - t).powf(1.4);
        let color = if k % 3 == 0 { theme::alpha(theme::REC, fade) } else { theme::alpha(theme::ACCENT, fade) };
        let mut q = p.clone();
        q.set_opacity(fade);
        orbiters_ui::widgets::icon_fill(&q, at, glyph, 9.0 + 3.0 * (k % 3) as f32 * (1.0 - t), color);
    }
}

/// An icon for a boop's emoji when it has no picture: the sample's glyphs and VRChat's usual ones, a sparkle for the rest.
pub(super) fn emoji_icon(id_or_glyph: &str) -> &'static str {
    match id_or_glyph {
        "👋" | "sample_0" | "wave" => icons::HAND_WAVING,
        "💖" | "❤️" | "sample_1" | "heart" | "love" => icons::HEART,
        "😺" | "🐱" | "sample_2" | "cat" => icons::SMILEY_WINK,
        "🎉" | "sample_3" | "confetti" | "party" => icons::CONFETTI,
        _ => icons::SPARKLE,
    }
}

/// The list's sections and people, only the rows in view drawn.
pub(super) fn list(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, clock: f64) {
    let jump = state.refresh_rows();
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("launcher-list"));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    if state.rows.is_empty() {
        empty(&mut child, state, rect);
        return;
    }
    let mut area = egui::ScrollArea::vertical().id_salt("launcher-list-scroll").auto_shrink([false, false]);
    if let Some(y) = jump {
        area = area.vertical_scroll_offset(y);
    }
    let output = area.show_viewport(&mut child, |ui, viewport| {
        let total = *state.tops.last().unwrap_or(&0.0) + 8.0;
        let width = ui.available_width();
        ui.set_min_size(vec2(width, total));
        let origin = ui.max_rect().min;
        let range = list::visible(&state.tops, viewport.min.y, viewport.max.y);
        let shown: Vec<(usize, Row)> = range.map(|i| (i, state.rows[i].clone())).collect();
        for (i, row) in shown {
            let r = Rect::from_min_size(origin + vec2(0.0, state.tops[i]), vec2(width, row.height()));
            match row {
                Row::Header { group, kind, name, count, online, folded } => header(ui, state, r, &group, kind, &name, count, online, folded),
                Row::Person { group, friend } => person(ui, cx, state, r, &group, friend, clock),
            }
        }
    });
    state.anchor = list::anchor_at(&state.keys, &state.tops, output.state.clamped_offset().y);
}

fn empty(ui: &mut Ui, state: &State, rect: Rect) {
    let p = ui.painter();
    let c = pos2(rect.center().x, rect.top() + 64.0);
    let (glyph, line) = if state.data.loading {
        (icons::CIRCLE_NOTCH, tr("Reading your friends…").to_string())
    } else if !state.query.trim().is_empty() {
        (icons::MAGNIFYING_GLASS, orbiters_i18n::tr!("Nobody matches “{text}”", text = state.query.trim()))
    } else if state.data.friends.is_empty() && !matches!(state.data.source, orbiters_core::launcher::Source::Nothing) {
        (icons::USERS, tr("No friends yet").to_string())
    } else {
        return;
    };
    widgets::icon(p, c, glyph, 22.0, theme::FG_4);
    widgets::text_truncated(p, pos2(c.x, c.y + 28.0), Align2::CENTER_CENTER, &Txt::new(line, 12.5).color(theme::FG_3), rect.width() - 32.0);
}

/// A section's header: its name and count; a press folds or opens it (remembered).
#[allow(clippy::too_many_arguments)]
fn header(ui: &mut Ui, state: &mut State, rect: Rect, group: &str, kind: GroupKind, name: &str, count: usize, online: usize, folded: bool) {
    let id = Id::unique(("launcher-header", group));
    let resp = ui.interact(rect.shrink2(vec2(4.0, 2.0)), id, Sense::click());
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let open = anim::toggle(ui.ctx(), id.with("open"), !folded, Spring::SNAPPY);
    let p = ui.painter();
    if hover > 0.01 {
        p.rect_filled(rect.shrink2(vec2(4.0, 3.0)), theme::radius(10.0), theme::white(0.035 * hover));
    }
    let y = rect.center().y + 2.0;
    let mut x = rect.left() + 12.0;
    let title = match kind {
        GroupKind::Favourite => {
            widgets::icon_fill(p, pos2(x + 5.0, y), icons::STAR, 11.0, theme::WARN);
            x += 16.0;
            Txt::new(if name.is_empty() { tr("Favourites") } else { name }, 12.5).weight(weight::SEMIBOLD).color(theme::FG)
        }
        GroupKind::InGame => Txt::caps(tr_ctx("friends section", "In game")).color(theme::FG_2),
        GroupKind::Website => Txt::caps(tr_ctx("friends section", "On the website")).color(theme::FG_2),
        GroupKind::Offline => Txt::caps(tr_ctx("friends section", "Offline")).color(theme::FG_2),
    };
    let t = widgets::text_truncated(p, pos2(x, y), Align2::LEFT_CENTER, &title, rect.width() - 110.0);
    widgets::text(p, pos2(t.right() + 7.0, y), Align2::LEFT_CENTER, &Txt::new(count.to_string(), 11.5).weight(weight::MEDIUM).color(theme::FG_4));
    if kind == GroupKind::Favourite && count > 0 && states::live(&state.data) == states::Live::SignedIn {
        let on = orbiters_i18n::tr!("{n} online", n = online);
        widgets::text(p, pos2(rect.right() - 36.0, y), Align2::RIGHT_CENTER, &Txt::new(on, 11.0).color(if online > 0 { theme::alpha(theme::ACCENT, 0.85) } else { theme::FG_4 }));
    }
    // The caret turns as the section folds.
    let c = pos2(rect.right() - 20.0, y);
    let glyph = if open > 0.5 { icons::CARET_DOWN } else { icons::CARET_RIGHT };
    widgets::icon(p, c, glyph, 12.0, theme::mix(theme::FG_4, theme::FG_2, hover));
    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        state.fold(group);
    }
}

/// A person's row: picture and status, name in their rank's colour, where they are or their status line, their
/// profile's banner behind; on hover, Boop and Request invite at its end. A press opens their profile.
fn person(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, group: &str, index: usize, clock: f64) {
    let Some(friend) = state.data.friends.get(index).cloned() else { return };
    let ctx = ui.ctx().clone();
    let card = rect.shrink2(vec2(4.0, 2.0));
    let id = Id::unique(("launcher-person", group, &friend.id));
    let resp = ui.interact(card, id, Sense::click());
    let inside = ui.rect_contains_pointer(card) && !super::modal::covers(&ctx);
    let hover = anim::toggle(&ctx, id.with("hover"), inside, Spring::SNAPPY);
    let open = state.modal.as_ref().is_some_and(|m| m.id == friend.id && m.closing.is_none());
    let p = ui.painter().clone();
    let fill = theme::mix(column_fill(), theme::INK_800, hover.max(if open { 1.0 } else { 0.0 }));
    widgets::frame(&p, card, 14.0, fill);
    if let Some(texture) = state.picture(&friend.look.banner) {
        banner(&p, card, 14.0, &texture, fill, 0.32, 0.9);
    }
    let offline = friend.presence == Presence::Offline;
    let pic = Rect::from_center_size(pos2(card.left() + 26.0, card.center().y), vec2(40.0, 40.0));
    let picture = state.picture(&friend.look.icon);
    let mut faded = p.clone();
    if offline {
        faded.multiply_opacity(0.55);
    }
    person_picture(&faded, pic, picture.as_ref(), &friend, fill);

    // The buttons at the end, on hover: their room comes off the text's.
    let now = state.now();
    let request = states::request(&state.data, &friend, state.requested.get(&friend.id).copied(), clock, now);
    let booped = state.booped.get(&friend.id).cloned();
    let boop = states::boop(&state.data, &friend, booped.as_ref().map(|b| b.0), clock, now);
    let request_w = match request {
        Reach::Ready => pill_width(ui, tr_ctx("request invite, short", "Request")),
        Reach::Sent => pill_width(ui, tr_ctx("sent", "Sent")),
        Reach::Cooling(_) => 74.0,
        _ => 0.0,
    };
    let boop_shows = !matches!(boop, Reach::SignIn);
    let busy = booped.as_ref().is_some_and(|b| clock - b.0 < states::SENT_FOR) || state.requested.get(&friend.id).is_some_and(|at| clock - at < states::SENT_FOR);
    let reveal = hover.max(if busy { 1.0 } else { 0.0 });
    let actions_w = if boop_shows { 36.0 } else { 0.0 } + if request_w > 0.0 { request_w + 6.0 } else { 0.0 };
    let x = pic.right() + 12.0;
    let room = card.right() - x - 10.0 - actions_w * reveal;

    let name_color = if offline { theme::mix(rank_color(friend.look.rank), theme::FG_3, 0.45) } else { rank_color(friend.look.rank) };
    widgets::text_truncated(&p, pos2(x, card.center().y - 9.0), Align2::LEFT_CENTER, &Txt::new(&friend.name, 13.5).weight(weight::SEMIBOLD).color(name_color), room);
    second_line(&p, &friend, pos2(x, card.center().y + 10.0), room, booped.as_ref().filter(|b| clock - b.0 < states::SENT_FOR).is_some());

    if reveal > 0.01 {
        let mut right = card.right() - 10.0 + 8.0 * (1.0 - reveal);
        let mut q = ui.new_child(UiBuilder::new().max_rect(card).id_salt(("launcher-row-actions", group, &friend.id)));
        // A person is in a favourite group and in their section: one button each, told apart by the section.
        let salt = format!("{group}/{}", friend.id);
        q.multiply_opacity(reveal);
        if request_w > 0.0 {
            let r = Rect::from_min_max(pos2(right - request_w, card.center().y - 15.0), pos2(right, card.center().y + 15.0));
            // Short on the row, so the name keeps its room; the whole words in its tooltip and the profile.
            let label = tr_ctx("request invite, short", "Request");
            let hint = orbiters_i18n::tr!("Request an invite from {name}", name = friend.name);
            let pressed = reach_pill(&mut q, r, ("launcher-request", salt.as_str()), request, icons::PAPER_PLANE_TILT, label, state.requested.get(&friend.id).copied(), clock);
            if request == Reach::Ready {
                q.interact(r, Id::unique(("launcher-request-hint", salt.as_str())), Sense::hover()).on_hover_text(hint);
            }
            if pressed {
                state.act(cx, clock, Action::RequestInvite { to: friend.id.clone() });
            }
            right = r.left() - 6.0;
        }
        if boop_shows && boop_button(&mut q, pos2(right - 16.0, card.center().y), 32.0, ("launcher-boop", salt.as_str()), boop, booped, clock) {
            state.act(cx, clock, Action::Boop { to: friend.id.clone(), emoji: None });
        }
    }
    if resp.clicked() {
        state.open_profile(&ctx, &friend.id, card);
        state.act(cx, clock, Action::OpenProfile { id: friend.id.clone() });
    }
    resp.on_hover_cursor(CursorIcon::PointingHand);
}

/// The row's second line: where they are (the world, its instance and region) or their status line; "Booped!" a
/// moment after a boop.
fn second_line(p: &egui::Painter, friend: &Friend, at: egui::Pos2, room: f32, booped: bool) {
    if booped {
        widgets::icon(p, pos2(at.x + 6.0, at.y), icons::HAND_WAVING, 12.0, theme::ACCENT);
        widgets::text_truncated(p, pos2(at.x + 17.0, at.y), Align2::LEFT_CENTER, &Txt::new(tr("Booped!"), 12.0).weight(weight::SEMIBOLD).color(theme::ACCENT), room - 17.0);
        return;
    }
    let small = |t: &str| Txt::new(t, 12.0).color(theme::FG_3);
    let in_world = matches!(friend.whereabouts, Whereabouts::World(_) | Whereabouts::Private | Whereabouts::Traveling);
    if !in_world && !friend.status_text.is_empty() {
        widgets::text_truncated(p, at, Align2::LEFT_CENTER, &small(&friend.status_text), room);
        return;
    }
    let mut x = at.x;
    if let Whereabouts::World(place) = &friend.whereabouts {
        widgets::icon(p, pos2(x + 6.0, at.y), super::access_icon(place.access), 12.0, theme::FG_3);
        x += 16.0;
        if !place.region.is_empty() {
            let tag = Txt::new(super::region_tag(&place.region), 9.5).weight(weight::BOLD).tracking(0.04).color(theme::FG_2);
            let w = p.layout_job(tag.job(f32::INFINITY)).size().x + 8.0;
            let chip = Rect::from_min_size(pos2(x, at.y - 7.5), vec2(w, 15.0));
            p.rect_filled(chip, theme::radius(4.0), theme::white(0.08));
            widgets::text(p, chip.center(), Align2::CENTER_CENTER, &tag);
            x += w + 6.0;
        }
        let world = if place.world_name.is_empty() { tr("A world").to_string() } else { place.world_name.clone() };
        widgets::text_truncated(p, pos2(x, at.y), Align2::LEFT_CENTER, &small(&world).color(theme::FG_2), room - (x - at.x));
        return;
    }
    match where_line(friend) {
        Some((glyph, line)) => {
            widgets::icon(p, pos2(x + 6.0, at.y), glyph, 12.0, theme::FG_4);
            widgets::text_truncated(p, pos2(x + 16.0, at.y), Align2::LEFT_CENTER, &small(&line), room - 16.0);
        }
        None => {
            let line = if friend.status_text.is_empty() { status_name(friend.status).to_string() } else { friend.status_text.clone() };
            widgets::text_truncated(p, at, Align2::LEFT_CENTER, &small(&line).color(theme::FG_4), room);
        }
    }
}

/// A narrow window's rail: a button with how many are online that opens the list over the page, then their pictures
/// (a press opens a profile).
pub(super) fn rail(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect) {
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let p = ui.painter().clone();
    widgets::frame(&p, rect, theme::CARD_RADIUS, column_fill());
    paint::stroke_squircle(&p, rect, theme::squircle(theme::CARD_RADIUS), theme::hairline());
    let (in_game, web, _) = list::counts(&state.data);
    let top = Rect::from_center_size(pos2(rect.center().x, rect.top() + 34.0), vec2(44.0, 44.0));
    let id = Id::unique("launcher-rail-open");
    let resp = ui.interact(top, id, Sense::click());
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    p.circle_filled(top.center(), 22.0, theme::mix(theme::white(0.07), theme::white(0.14), hover));
    widgets::icon(&p, top.center(), icons::USERS, 20.0, theme::FG);
    if in_game + web > 0 {
        widgets::count_badge(&p, top.right_top() + vec2(-4.0, 6.0), in_game + web);
    }
    // Something received (a boop, an invite request): a little green hand on the button's foot.
    let got = state.data.boops.iter().any(|b| !state.dismissed.contains(&b.id))
        || state.data.invites.iter().any(|i| i.kind == InviteKind::Request && !state.dismissed.contains(&i.id));
    if got {
        let c = top.left_bottom() + vec2(6.0, -6.0);
        p.circle_filled(c, 10.0, column_fill());
        p.circle_filled(c, 8.0, theme::ACCENT);
        widgets::icon(&p, c, icons::HAND_WAVING, 10.0, theme::ON_ACCENT);
    }
    widgets::note_control(&ctx, orbiters_i18n::mark("Friends"), top, true);
    if widgets::activated(&resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Friends"))) {
        state.open_drawer(&ctx);
    }
    p.hline(rect.shrink(14.0).x_range(), top.bottom() + 12.0, egui::Stroke::new(1.0, theme::white(0.06)));
    // Online first: in the game, then on the website.
    let mut order: Vec<usize> = (0..state.data.friends.len()).filter(|&i| state.data.friends[i].presence == Presence::InGame).collect();
    order.extend((0..state.data.friends.len()).filter(|&i| state.data.friends[i].presence == Presence::Website));
    let step = 50.0;
    let first = top.bottom() + 24.0 + 20.0;
    let fits = (((rect.bottom() - 16.0 - first) / step).floor().max(0.0) as usize + 1).min(order.len());
    let more = order.len() - fits;
    let shown = if more > 0 { fits.saturating_sub(1) } else { fits };
    for (n, &i) in order.iter().take(shown).enumerate() {
        let friend = state.data.friends[i].clone();
        let c = pos2(rect.center().x, first + n as f32 * step);
        let r = Rect::from_center_size(c, vec2(40.0, 40.0));
        let rid = Id::unique(("launcher-rail", &friend.id));
        let resp = ui.interact(r, rid, Sense::click());
        let h = anim::toggle(&ctx, rid.with("hover"), resp.hovered(), Spring::SNAPPY);
        let r = Rect::from_center_size(c, vec2(40.0, 40.0) * (1.0 + 0.08 * h));
        let picture = state.picture(&friend.look.icon);
        person_picture(&p, r, picture.as_ref(), &friend, column_fill());
        if resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(&friend.name).clicked() {
            state.open_profile(&ctx, &friend.id, r);
            state.act(cx, clock, Action::OpenProfile { id: friend.id.clone() });
        }
    }
    if more > 0 {
        let c = pos2(rect.center().x, first + shown as f32 * step);
        let r = Rect::from_center_size(c, vec2(40.0, 40.0));
        let resp = ui.interact(r, Id::unique("launcher-rail-more"), Sense::click());
        p.circle_filled(c, 20.0, theme::white(if resp.hovered() { 0.12 } else { 0.06 }));
        widgets::text(&p, c, Align2::CENTER_CENTER, &Txt::new(format!("+{more}"), 12.0).weight(weight::SEMIBOLD).color(theme::FG_2));
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            state.open_drawer(&ctx);
        }
    }
}
