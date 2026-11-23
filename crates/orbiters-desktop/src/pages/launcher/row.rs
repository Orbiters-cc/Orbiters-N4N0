//! The column's parts: the account's head, the line about the sign-in, what was received, the list's rows (only those
//! in view), and the rail of pictures a narrow window has instead.

use super::list::{self, Row, Section};
use super::states::{self, Reach};
use super::{column_fill, person_picture, rank_color, status_name, where_line, Cx, State};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_core::launcher::{Action, Friend, InviteKind, Presence, Whereabouts};
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};

/// The account itself at the top: picture, name, status; Read again (and Close over the page).
pub(super) fn me(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, drawer: bool, clock: f64) {
    let me = state.data.me.clone();
    if let Some(me) = &me {
        super::backdrop(ui.painter(), rect, 14.0, state, &me.look, column_fill(), 0.15, 0.85);
    }
    let pic = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 21.0), vec2(42.0, 42.0));
    let text_x = pic.right() + 12.0;
    let right_room = if drawer { 76.0 } else { 42.0 };
    let max_w = rect.right() - text_x - right_room;
    let p = ui.painter().clone();
    match &me {
        Some(me) => {
            let picture = state.picture(&me.look.icon);
            let frame = state.picture(&me.look.icon_frame);
            person_picture(&p, pic, picture.as_ref(), frame.as_ref(), me, column_fill());
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
            // The own profile and its lists are read again too.
            state.mine.read_again();
            state.mine_listed.clear();
        }
    }
}

/// What needs the VRChat sign-in, said once at the head of the list, with the way to sign in. Returns where the next
/// part starts.
pub(super) fn sign_in_note(ui: &mut Ui, cx: &mut Cx, state: &mut State, inner: Rect, y: f32) -> f32 {
    // r21: a saved session being restored, or signed in with its list on the way: said quietly, no sign-in offered.
    let vrchat = cx.snap.vrchat_login.presence();
    if vrchat != crate::controller::vrchat_login::Presence::SignedOut && states::live(&state.data) != states::Live::SignedIn {
        let name = cx.snap.vrchat_login.shown_user().map(|u| u.display_name.clone()).filter(|n| !n.trim().is_empty());
        let text = match (vrchat, name) {
            (crate::controller::vrchat_login::Presence::SignedIn, _) => tr("Reading your friends from VRChat…").to_string(),
            (_, Some(name)) => orbiters_i18n::tr!("Connecting to VRChat as {name}…", name = name),
            (_, None) => tr("Connecting to VRChat…").to_string(),
        };
        let row = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 36.0));
        let p = ui.painter().clone();
        widgets::frame(&p, row, 12.0, theme::white(0.04));
        orbiters_ui::feedback::spinner(&p, pos2(row.left() + 18.0, row.center().y), 6.0, 2.0, theme::FG_3);
        widgets::text_truncated(&p, pos2(row.left() + 34.0, row.center().y), Align2::LEFT_CENTER, &Txt::new(&text, 12.5).color(theme::FG_2), row.width() - 44.0);
        anim::request_frame(ui.ctx());
        return y + row.height() + 10.0;
    }
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

/// What was received, as its row draws it.
enum Got {
    Boop { id: String, from: String, name: String, icon: String, at: i64 },
    Request { id: String, name: String, icon: String, at: i64 },
}

/// The boop or invite request `id` received.
fn got(state: &State, id: &str) -> Option<Got> {
    if let Some(b) = state.data.boops.iter().find(|b| b.id == id) {
        return Some(Got::Boop { id: b.id.clone(), from: b.from_id.clone(), name: b.from_name.clone(), icon: b.from_icon.clone(), at: b.received_at });
    }
    state.data.invites.iter().find(|i| i.id == id && i.kind == InviteKind::Request).map(|i| Got::Request { id: i.id.clone(), name: i.from_name.clone(), icon: i.from_icon.clone(), at: i.received_at })
}

/// Over two or more received rows, as a section's header says its name and count: what they are, how many.
fn got_head(ui: &Ui, rect: Rect, count: usize, boops: usize) {
    let title = match boops {
        b if b == count => tr("Boops"),
        0 => tr("Invite requests"),
        _ => tr("Boops and invite requests"),
    };
    let p = ui.painter();
    let cy = rect.center().y + 1.0;
    widgets::icon(p, pos2(rect.left() + 16.0, cy), icons::HAND_WAVING, 13.0, theme::ACCENT);
    let t = widgets::text_truncated(p, pos2(rect.left() + 29.0, cy), Align2::LEFT_CENTER, &Txt::new(title, 12.5).weight(weight::SEMIBOLD), rect.width() - 80.0);
    widgets::text(p, pos2(t.right() + 7.0, cy), Align2::LEFT_CENTER, &Txt::new(count.to_string(), 11.5).weight(weight::MEDIUM).color(theme::FG_4));
}

/// One received row at `rect`: who, what and when; Boop back on a boop; its put-away cross.
fn got_row(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, g: &Got, clock: f64, now: i64) {
    let p = ui.painter().clone();
    let accent = matches!(g, Got::Boop { .. });
    widgets::frame(&p, rect, 14.0, if accent { theme::alpha(theme::ACCENT, 0.09) } else { theme::surface_soft() });
    let (name, icon, at, id) = match g {
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
    // Boop back as wide as its words in the language.
    let back_w = pill_width(ui, tr("Boop back")).max(92.0);
    let room = rect.right() - x - if accent { back_w + 46.0 } else { 40.0 };
    widgets::text_truncated(&p, pos2(x, rect.center().y - 8.0), Align2::LEFT_CENTER, &Txt::new(&name, 13.0).weight(weight::SEMIBOLD), room);
    let when = super::super::ago(ui.ctx(), at.max(0) as u64, now.max(0) as u64);
    widgets::text_truncated(&p, pos2(x, rect.center().y + 9.0), Align2::LEFT_CENTER, &Txt::new(format!("{what} · {when}"), 11.5).color(theme::FG_3), room);
    let close = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(28.0, 28.0));
    if widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Dismiss")).size(28.0, 13.0).id_salt(("launcher-got-x", &id)).show_at(ui, close)) {
        state.act(cx, clock, Action::Dismiss { id: id.clone() });
    }
    if let Got::Boop { from, .. } = g {
        let friend = state.data.friend(from).cloned();
        let reach = match &friend {
            Some(f) => states::boop(&state.data, f, state.booped.get(from).map(|b| b.0), clock, now),
            None if state.data.possible.boop => Reach::Ready,
            None => Reach::SignIn,
        };
        let at = Rect::from_min_size(pos2(close.left() - 4.0 - back_w, rect.center().y - 15.0), vec2(back_w, 30.0));
        if reach_pill(ui, at, ("launcher-boop-back", from.as_str()), reach, icons::HAND_WAVING, tr("Boop back"), state.booped.get(from).map(|b| b.0), clock) {
            state.act(cx, clock, Action::Boop { to: from.clone(), emoji: None });
            state.act(cx, clock, Action::Dismiss { id: id.clone() });
        }
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
            resp.on_hover_text(orbiters_i18n::tr!("Again in {n} s", n = left));
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
        // VRChat's default emoji ids (`launcher_feed::boop_emojis`) and the sample's.
        "👋" | "sample_0" | "wave" | "default_hand_wave" => icons::HAND_WAVING,
        "💖" | "❤️" | "sample_1" | "heart" | "love" | "default_heart" => icons::HEART,
        "😊" | "default_smile" => icons::SMILEY,
        "😺" | "🐱" | "😂" | "sample_2" | "cat" | "default_laugh" => icons::SMILEY_WINK,
        "🎉" | "sample_3" | "confetti" | "party" | "default_confetti" => icons::CONFETTI,
        "🎵" | "default_music_note" => icons::MUSIC_NOTES,
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
                Row::Header { group, section, name, count, folded } => header(ui, state, r, &group, section, &name, count, folded),
                Row::Person { group, friend } => person(ui, cx, state, r, &group, friend, clock),
                Row::GotHead { count, boops } => got_head(ui, r, count, boops),
                // As wide as a person's card, the gap under it.
                Row::Got { id } => {
                    if let Some(g) = got(state, &id) {
                        let now = state.now();
                        got_row(ui, cx, state, Rect::from_min_size(r.min + vec2(4.0, 2.0), vec2(r.width() - 8.0, list::GOT_H - 6.0)), &g, clock, now);
                    }
                }
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

/// A section's header: what it is (the mark the counts line gives it), its name and count; a press folds or opens it
/// (remembered). A favourite group's, inside the favourites, sits indented with its own name.
fn header(ui: &mut Ui, state: &mut State, rect: Rect, group: &str, section: Section, name: &str, count: usize, folded: bool) {
    let id = Id::unique(("launcher-header", group));
    let nested = section.nested();
    let rect = if nested { Rect::from_min_max(pos2(rect.left() + 14.0, rect.top()), rect.max) } else { rect };
    let resp = ui.interact(rect.shrink2(vec2(4.0, 2.0)), id, Sense::click());
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let open = anim::toggle(ui.ctx(), id.with("open"), !folded, Spring::SNAPPY);
    let p = ui.painter();
    if hover > 0.01 {
        p.rect_filled(rect.shrink2(vec2(4.0, 3.0)), theme::radius(10.0), theme::white(0.035 * hover));
    }
    let y = rect.center().y + 2.0;
    let x = rect.left() + 12.0;
    // The same marks as the counts line over the list: filled in the game, a ring on the website.
    let mark = pos2(x + 5.0, y);
    match section {
        Section::Favourites | Section::FavouriteGroup | Section::OtherFavourites => {
            widgets::icon_fill(p, mark, icons::STAR, if nested { 10.0 } else { 11.0 }, theme::alpha(theme::WARN, if nested { 0.75 } else { 1.0 }))
        }
        Section::InGame => {
            p.circle_filled(mark, 4.0, theme::ACCENT);
        }
        Section::Website => {
            p.circle_stroke(mark, 3.3, egui::Stroke::new(1.6, theme::ACCENT));
        }
        Section::Offline => {
            p.circle_filled(mark, 4.0, theme::FG_4);
        }
    }
    let x = x + 16.0;
    let title = match section {
        Section::Favourites => Txt::caps(tr_ctx("friends section", "Favourites")).color(theme::FG_2),
        Section::FavouriteGroup => Txt::new(name, 12.5).weight(weight::SEMIBOLD).color(theme::FG),
        Section::OtherFavourites => Txt::new(tr_ctx("favourite group", "Other"), 12.5).weight(weight::SEMIBOLD).color(theme::FG),
        Section::InGame => Txt::caps(tr_ctx("friends section", "In game")).color(theme::FG_2),
        Section::Website => Txt::caps(tr_ctx("friends section", "On the website")).color(theme::FG_2),
        Section::Offline => Txt::caps(tr_ctx("friends section", "Offline")).color(theme::FG_2),
    };
    let t = widgets::text_truncated(p, pos2(x, y), Align2::LEFT_CENTER, &title, rect.width() - 90.0);
    widgets::text(p, pos2(t.right() + 7.0, y), Align2::LEFT_CENTER, &Txt::new(count.to_string(), 11.5).weight(weight::MEDIUM).color(theme::FG_4));
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
    super::backdrop(&p, card, 14.0, state, &friend.look, fill, 0.32, 0.9);
    let offline = friend.presence == Presence::Offline;
    let pic = Rect::from_center_size(pos2(card.left() + 26.0, card.center().y), vec2(40.0, 40.0));
    let picture = state.picture(&friend.look.icon);
    let mut faded = p.clone();
    if offline {
        faded.multiply_opacity(0.55);
    }
    let frame = state.picture(&friend.look.icon_frame);
    person_picture(&faded, pic, picture.as_ref(), frame.as_ref(), &friend, fill);

    // The buttons at the end, on hover: their room comes off the text's.
    let now = state.now();
    let request = states::request(&state.data, &friend, state.requested.get(&friend.id).copied(), clock, now);
    let booped = state.booped.get(&friend.id).cloned();
    let boop = states::boop(&state.data, &friend, booped.as_ref().map(|b| b.0), clock, now);
    // An instance that takes the account without asking: Select (into Where) in Request's place (REQUEST23); both words
    // get the wider one's room, so a row never jumps between them.
    let select = (request == Reach::Ready).then(|| super::previous::friend_selectable(state, &friend)).flatten();
    let request_w = match request {
        Reach::Ready => pill_width(ui, tr_ctx("request invite, short", "Request")).max(pill_width(ui, tr_ctx("choose an instance", "Select"))),
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
    let seen = friend.last_seen.filter(|_| friend.presence == Presence::Offline).map(|at| super::super::ago(&ctx, at.max(0) as u64, now.max(0) as u64));
    second_line(&p, &friend, pos2(x, card.center().y + 10.0), room, booped.as_ref().filter(|b| clock - b.0 < states::SENT_FOR).is_some(), seen);

    if reveal > 0.01 {
        let mut right = card.right() - 10.0 + 8.0 * (1.0 - reveal);
        let mut q = ui.new_child(UiBuilder::new().max_rect(card).id_salt(("launcher-row-actions", group, &friend.id)));
        // A person is in a favourite group and in their section: one button each, told apart by the section.
        let salt = format!("{group}/{}", friend.id);
        q.multiply_opacity(reveal);
        if let (true, Some(place)) = (request_w > 0.0, select.as_ref()) {
            let r = Rect::from_min_max(pos2(right - request_w, card.center().y - 15.0), pos2(right, card.center().y + 15.0));
            let label = tr_ctx("choose an instance", "Select");
            let hint = orbiters_i18n::tr!("Choose {name}'s instance in Where: Play starts VRChat there", name = friend.name);
            if reach_pill(&mut q, r, ("launcher-select", salt.as_str()), Reach::Ready, icons::CROSSHAIR, label, None, clock) {
                super::previous::select_friend(state, cx, &friend.id, &friend.name, place);
            }
            q.interact(r, Id::unique(("launcher-select-hint", salt.as_str())), Sense::hover()).on_hover_text(hint);
            right = r.left() - 6.0;
        } else if request_w > 0.0 {
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
fn second_line(p: &egui::Painter, friend: &Friend, at: egui::Pos2, room: f32, booped: bool, seen: Option<String>) {
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
            // Their status line; else when they were last seen; else their status.
            let line = match (friend.status_text.is_empty(), seen) {
                (false, _) => friend.status_text.clone(),
                (true, Some(when)) => orbiters_i18n::tr!("Last seen {when}", when = when),
                (true, None) => status_name(friend.status).to_string(),
            };
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
        let frame = state.picture(&friend.look.icon_frame);
        person_picture(&p, r, picture.as_ref(), frame.as_ref(), &friend, column_fill());
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

/// In place of a list whose VRChat account is no longer signed in, until the service's next look replaces it: what
/// happened, and nothing of the old list (REQUEST15 third pass, blocker 2).
pub(super) fn put_away(ui: &mut Ui, state: &State, rect: Rect, side: super::Side) {
    let p = ui.painter().clone();
    widgets::frame(&p, rect, theme::CARD_RADIUS, column_fill());
    paint::stroke_squircle(&p, rect, theme::squircle(theme::CARD_RADIUS), theme::hairline());
    let signed_out = matches!(&state.login, Some(None));
    let (glyph, title, body) = if signed_out {
        (icons::SIGN_OUT, tr("Signed out of VRChat"), tr("The friends list was that account's: it is put away. Sign in again to see yours."))
    } else {
        (icons::USERS, tr("Another VRChat account"), tr("The friends list was the previous account's: it is put away until this account's is read."))
    };
    let c = pos2(rect.center().x, rect.top() + 64.0);
    p.circle_filled(c, 24.0, theme::white(0.06));
    widgets::icon(&p, c, glyph, 22.0, theme::FG_3);
    if matches!(side, super::Side::Rail) {
        // The rail: the sign alone, its words on hover.
        ui.interact(Rect::from_center_size(c, vec2(48.0, 48.0)), Id::unique("launcher-put-away"), Sense::hover()).on_hover_text(format!("{title}\n{body}"));
        return;
    }
    widgets::text(&p, pos2(rect.center().x, c.y + 44.0), Align2::CENTER_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD));
    let w = rect.width() - 48.0;
    let mut job = Txt::new(body, 12.5).color(theme::FG_3).line_height(17.0).job(w);
    // Each line centred under the title.
    job.halign = egui::Align::Center;
    let galley = p.layout_job(job);
    p.galley(pos2(rect.center().x, c.y + 64.0), galley, theme::FG_3);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Ctl};
    use crate::state::Snapshot;
    use orbiters_core::launcher::Boop;
    use orbiters_core::settings::Settings;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Nothing is sent from these pictures of the list.
    struct Still;
    impl Backend for Still {
        fn n4n0(&self, _command: crate::ipc::N4n0Command) {}
        fn view(&self) -> Arc<Snapshot> {
            Arc::new(Snapshot::new(Settings::default()))
        }
        fn take_notices(&self) -> Vec<crate::controller::Notice> {
            Vec::new()
        }
        fn library_version(&self) -> u64 {
            0
        }
        fn attach(&self, _ctx: &egui::Context) {}
        fn start_capture(&self) {}
        fn stop_capture(&self) {}
        fn pause_capture(&self, _paused: bool) {}
        fn start_test_capture(&self) {}
        fn import_raw_capture(&self, _dir: PathBuf) {}
        fn patch_settings(&self, _patch: serde_json::Value) {}
        fn trash_session(&self, _dir: PathBuf) {}
        fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
        fn reconstruct(&self, _dir: PathBuf) {}
        fn stop_reconstruction(&self) {}
        fn unqueue_reconstruction(&self, _dir: PathBuf) {}
        fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
        fn sign_in(&self) {}
        fn reopen_sign_in(&self) {}
        fn cancel_sign_in(&self) {}
        fn sign_out(&self) {}
        fn request_worker_log(&self) {}
        fn worker_log(&self) -> Option<Vec<String>> {
            None
        }
        fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
        fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
        fn request_maps(&self) {}
        fn maps(&self) -> Option<crate::state::MapsMine> {
            None
        }
        fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
        fn frames(&self, _chunk: crate::state::FrameChunk) {}
        fn headset(&self, _in_use: bool) {}
        fn walk_boost(&self, _strength: f32) {}
        fn osc_reconnect(&self) {}
        fn updates(&self, _action: crate::ipc::UpdateAction) {}
        fn quit(&self) {}
    }

    /// r21: the friends column drawn from a launch with a session saved here: while it is connecting (and once signed
    /// in, its list not read yet) no "Sign in to VRChat" is painted, the account's name is; signed out for certain, it is.
    #[test]
    fn the_friends_column_offers_the_sign_in_only_when_signed_out_for_certain() {
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let painted = |login: crate::controller::vrchat_login::Status| {
            let mut data = orbiters_core::launcher::sample();
            data.source = orbiters_core::launcher::Source::Database { app: orbiters_core::launcher::DatabaseApp::VrcNext, read_at: 0 };
            let mut state = State { data: Arc::new(data), ..Default::default() };
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let ctl: Ctl = Arc::new(Still);
            let mut snap = Snapshot::new(Settings::default());
            snap.vrchat_login = login;
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let column = Rect::from_min_size(pos2(0.0, 0.0), vec2(380.0, 780.0));
            let mut out = Vec::new();
            // Two seconds of frames: the column fades in, and nothing is painted while it is transparent.
            for _ in 0..120 {
                let input = egui::RawInput { screen_rect: Some(column), time: Some(ctx.input(|x| x.time) + 1.0 / 60.0), ..Default::default() };
                let mut output = ctx.run_ui(input, |ui| {
                    let mut cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                    super::super::side_panel(ui, &mut cx, &mut state, column, super::super::Side::Column(column.width()));
                });
                // Nothing paints these frames.
                output.textures_delta.clear();
                out.clear();
                output.shapes.iter().for_each(|c| texts(&c.shape, &mut out));
            }
            out.join(" | ")
        };
        let tester = || serde_json::from_value::<orbiters_core::vrchat_api::User>(serde_json::json!({"id": "usr_test", "displayName": "Tester"})).unwrap();
        let connecting = crate::controller::vrchat_login::Status { restoring: true, known: Some(tester()), ..Default::default() };
        let drawn = painted(connecting);
        assert!(drawn.contains("Search friends"), "the column is painted: {drawn}");
        assert!(!drawn.contains("Sign in to VRChat"), "{drawn}");
        assert!(drawn.contains("Connecting to VRChat as Tester"), "{drawn}");
        let waiting = crate::controller::vrchat_login::Status { waiting: true, restoring: true, known: Some(tester()), ..Default::default() };
        assert!(!painted(waiting).contains("Sign in to VRChat"));
        let signed_in = crate::controller::vrchat_login::Status { allowed: true, user: Some(tester()), ..Default::default() };
        let drawn = painted(signed_in);
        assert!(!drawn.contains("Sign in to VRChat") && drawn.contains("Reading your friends from VRChat"), "{drawn}");
        let drawn = painted(crate::controller::vrchat_login::Status::default());
        assert!(drawn.contains("Sign in to VRChat"), "signed out for certain: {drawn}");
    }

    /// REQUEST18 1, Enzo: "the whole thing is scrollable … intricated scrollable frames are terrible experience". Thirty
    /// boops (made up, from the sample's made-up people) in the friends column of a 780 high window: the column has one
    /// scroll area, its list; the newest boops' rows are its first rows, and one turn of the wheel over the list takes them
    /// out of view with the rest of it, the friends following.
    #[test]
    fn boops_are_rows_of_the_one_scrolling_list() {
        let mut data = orbiters_core::launcher::sample();
        data.possible.boop = true;
        let from = data.friends[3].clone();
        data.boops = (0..30)
            .map(|i| Boop { id: format!("not_boop_{i:02}"), from_id: format!("usr_sample_boop_{i:02}"), from_name: from.name.clone(), from_icon: String::new(), emoji: None, received_at: 1_000 - i as i64 })
            .collect();
        // More people in the game, so that the friends under the boops fill more than the view (Offline starts folded).
        for n in 0..12 {
            let mut f = from.clone();
            f.id = format!("usr_sample_more_{n:02}");
            data.groups.iter_mut().find(|g| g.id == list::IN_GAME).unwrap().members.push(f.id.clone());
            data.friends.push(f);
        }
        let mut state = State { data: Arc::new(data), ..Default::default() };
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: Ctl = Arc::new(Still);
        let snap = Snapshot::new(Settings::default());
        let mut toasts = orbiters_ui::toast::Toasts::default();
        let column = Rect::from_min_size(pos2(0.0, 0.0), vec2(380.0, 780.0));
        let mut frame = |state: &mut State, n: u32, events: Vec<egui::Event>| {
            for i in 0..n {
                let input = egui::RawInput {
                    screen_rect: Some(column),
                    time: Some(ctx.input(|x| x.time) + 1.0 / 60.0),
                    events: if i == 0 { events.clone() } else { Vec::new() },
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    let mut cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                    super::super::side_panel(ui, &mut cx, state, column, super::super::Side::Column(column.width()));
                });
                // Nothing paints these frames.
                output.textures_delta.clear();
            }
        };
        let drawn = |state: &State, i: usize| ctx.read_response(Id::unique(("launcher-boop-back", state.data.boops[i].from_id.as_str()))).is_some();
        frame(&mut state, 30, Vec::new());
        assert_eq!(ctx.data(|d| d.count::<egui::scroll_area::State>()), 1, "one scroll area in the column: the list");
        assert!(drawn(&state, 0) && drawn(&state, 1), "the newest boops are the list's first rows, each with its Boop back");
        assert!(!drawn(&state, 29), "the oldest further down the same list, out of view");
        assert_eq!(state.rows[0], Row::GotHead { count: 31, boops: 30 }, "every boop, and the sample's invite request");
        let list_at = pos2(190.0, 500.0);
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -(list::GOT_HEAD_H + 31.0 * list::GOT_H)), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE, source: egui::MouseWheelSource::Wheel };
        frame(&mut state, 90, vec![egui::Event::PointerMoved(list_at), wheel]);
        assert!((0..30).all(|i| !drawn(&state, i)), "scrolled past the boops: none left in view");
        assert_eq!(ctx.data(|d| d.count::<egui::scroll_area::State>()), 1, "still one scroll area");
    }

    /// REQUEST20: the pictures of the rows on screen are the ones asked (then asked first by the fetcher, which serves what
    /// a page asked in the last two seconds before the background), top row first, in the sectioned list's order; none of a
    /// folded section or of the rows below the view. The background then walks the people in that same order.
    #[test]
    fn the_rows_on_screen_ask_for_their_pictures_in_the_sectioned_lists_order() {
        let mut data = orbiters_core::launcher::sample();
        data.boops.clear();
        data.invites.clear();
        let in_game = data.friends.iter().find(|f| f.presence == Presence::InGame && f.favourite_groups.is_empty()).unwrap().clone();
        for n in 0..30 {
            let mut f = in_game.clone();
            f.id = format!("usr_sample_more_{n:02}");
            data.groups.iter_mut().find(|g| g.id == list::IN_GAME).unwrap().members.push(f.id.clone());
            data.friends.push(f);
        }
        for (i, f) in data.friends.iter_mut().enumerate() {
            f.look.icon = format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-{i:012}/1/file");
        }
        let folded: std::collections::HashSet<String> = [list::OFFLINE.to_string()].into();
        let order: Vec<String> = list::rows(&data, "", &folded)
            .into_iter()
            .filter_map(|r| match r {
                Row::Person { friend, .. } => Some(data.friends[friend].look.icon.clone()),
                _ => None,
            })
            .collect();
        let icons: Vec<String> = data.friends.iter().map(|f| f.look.icon.clone()).collect();
        let offline: Vec<String> = data.friends.iter().filter(|f| f.presence == Presence::Offline).map(|f| f.look.icon.clone()).collect();
        assert!(!offline.is_empty());
        let mut state = State { data: Arc::new(data), ..Default::default() };
        (state.folded, state.folded_read) = (folded, true);
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: Ctl = Arc::new(Still);
        let snap = Snapshot::new(Settings::default());
        let mut toasts = orbiters_ui::toast::Toasts::default();
        let column = Rect::from_min_size(pos2(0.0, 0.0), vec2(380.0, 780.0));
        for _ in 0..30 {
            let input = egui::RawInput { screen_rect: Some(column), time: Some(ctx.input(|x| x.time) + 1.0 / 60.0), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                super::super::side_panel(ui, &mut cx, &mut state, column, super::super::Side::Column(column.width()));
            });
            output.textures_delta.clear();
        }
        // The people's pictures asked (the list also asks its decorations; none here).
        let asked: Vec<String> = state.wanted.iter().filter(|w| icons.contains(w)).cloned().collect();
        assert!(asked.len() >= 4, "the rows on screen asked for theirs: {}", asked.len());
        assert_eq!(asked, order[..asked.len()], "top row first, in the list's order (favourites in the game, then in the game)");
        assert!(asked.len() < order.len(), "the rows below the view are not asked yet");
        assert!(asked.iter().all(|a| !offline.contains(a)), "nothing of the folded Offline section");
        // The background walks everyone in the order the list shows them (every section open), after the rows on screen.
        let background: Vec<String> = State::people_pictures(&state.data).into_iter().map(|(url, _)| url).collect();
        assert_eq!(background[..order.len()], order[..], "the open sections first, in their order");
        assert!(offline.iter().all(|o| background.contains(o)), "then Offline's (folded is not forgotten)");
        assert_eq!(background.len(), icons.len(), "each person once");
    }
}
