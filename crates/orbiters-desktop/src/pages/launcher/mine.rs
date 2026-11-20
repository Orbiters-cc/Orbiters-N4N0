//! Home's card of the account's own VRChat profile (REQUEST15 7b), edited where it shows: the text is pressed to
//! change it, the status to choose another, the picture and the banner to choose among what VRChat allows for the
//! account. Every change goes to VRChat only on the person's own press (Enter, Save, a choice), one at a time; the
//! card says Saving, Saved or why not, and VRChat's limits (lengths, VRC+, its pace) before they are hit.

use super::{cover_uv, status_color, status_name, Cx, State};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Key, Order, Rect, Sense, Ui, UiBuilder};
use orbiters_core::launcher::{Action, BannerChoice, MyProfile, OwnPicture, PictureKind, ProfileEdit, ProfilePart, Saving, Source, Status, BIO_MAX, SETTABLE_STATUSES, STATUS_TEXT_MAX};
use orbiters_i18n::tr;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{feedback, paint, popup, widgets};

const BANNER_H: f32 = 86.0;
/// From the banner's foot: the name's line, the status text's, the bio's top.
const NAME_DY: f32 = 18.0;
const TEXT_DY: f32 = 46.0;
const BIO_DY: f32 = 70.0;
/// The line that says why a change did not go, under the status text.
const FAILED_H: f32 = 30.0;
const ICON: f32 = 78.0;
const PAD: f32 = 18.0;
const BIO_LINE: f32 = 19.0;
const BIO_ROWS_IDLE: usize = 3;
const BIO_EDIT_H: f32 = 104.0;
const BUTTONS_H: f32 = 34.0;
/// How long "Saved" stays after VRChat took a change.
const SAVED_FOR: f64 = 3.0;
/// Colours offered for a colour banner.
pub const BANNER_COLOURS: [[u8; 3]; 10] = [
    [59, 130, 246],
    [99, 102, 241],
    [139, 92, 246],
    [236, 72, 153],
    [239, 68, 68],
    [249, 115, 22],
    [234, 179, 8],
    [34, 197, 94],
    [20, 184, 166],
    [71, 85, 105],
];

/// What is being typed.
#[derive(Clone, Debug, PartialEq)]
pub enum Editing {
    StatusText(String),
    Bio(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Picker {
    Picture,
    Banner,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum BannerTab {
    #[default]
    Colour,
    Avatar,
    Gallery,
}

/// The card's own state, kept by the launcher's page.
#[derive(Default)]
pub struct Mine {
    pub editing: Option<Editing>,
    picker: Option<(Picker, Rect, f64)>,
    banner_tab: Option<BannerTab>,
    /// The account the profile was asked for (once per account and run).
    asked_for: Option<String>,
    /// The account everything below was typed or chosen on: a switch to another drops all of it (REQUEST15 fix B3).
    account: Option<String>,
    /// The last change pressed, the answers counted when it was pressed, and when: shown at once while VRChat answers,
    /// given back to its field when VRChat refuses it.
    pending: Option<(ProfileEdit, u64, f64)>,
    last: Option<ProfileEdit>,
    /// When the page first saw the last "Saved".
    saved_seen: Option<(u64, f64)>,
}

impl Mine {
    /// Pictures of the page: a text being typed.
    pub fn stand_in_editing(&mut self, editing: Editing) {
        self.editing = Some(editing);
    }

    /// Pictures of the page: the last change pressed (its Try again).
    pub fn stand_in_last(&mut self, edit: ProfileEdit) {
        self.last = Some(edit);
    }

    /// Pictures of the page: the banner's chooser (on its gallery) or the picture's, opened from `anchor` at `clock`.
    pub fn stand_in_picker(&mut self, banner: bool, gallery: bool, anchor: Rect, clock: f64) {
        self.picker = Some((if banner { Picker::Banner } else { Picker::Picture }, anchor, clock));
        self.banner_tab = gallery.then_some(BannerTab::Gallery);
    }
}

fn shown_mine(state: &State) -> Option<MyProfile> {
    state.data.mine.clone()
}

fn signed_in(state: &State) -> Option<String> {
    match &state.data.source {
        Source::SignedIn { user_id, .. } if !user_id.is_empty() => Some(user_id.clone()),
        _ => None,
    }
}

/// The card's height at `width` for what it shows now.
fn height(ui: &Ui, state: &State, width: f32) -> f32 {
    if signed_in(state).is_none() {
        return 92.0;
    }
    let bio_h = match &state.mine.editing {
        Some(Editing::Bio(_)) => BIO_EDIT_H + 8.0 + BUTTONS_H,
        _ => {
            let bio = shown_mine(state).map(|m| m.bio).unwrap_or_default();
            if bio.trim().is_empty() {
                BIO_LINE
            } else {
                let mut job = Txt::new(bio.trim(), 13.0).line_height(BIO_LINE).job(width - 2.0 * PAD);
                job.wrap.max_rows = BIO_ROWS_IDLE;
                ui.painter().layout_job(job).size().y.max(BIO_LINE)
            }
        }
    };
    BANNER_H + BIO_DY + failed_h(state) + bio_h + PAD
}

/// Why the last change did not go, while it is said (none while another is on its way).
fn failed(state: &State) -> Option<String> {
    match state.data.mine.as_ref().and_then(|m| m.saving.as_ref()) {
        Some(Saving::Failed(_, why)) if state.mine.pending.is_none() => Some(why.clone()),
        _ => None,
    }
}

fn failed_h(state: &State) -> f32 {
    if failed(state).is_some() {
        FAILED_H
    } else {
        0.0
    }
}

/// The card, as Home's block `index`.
pub fn card(ui: &mut Ui, cx: &mut Cx, state: &mut State, index: usize) {
    sync_account(state);
    if !state.shown() {
        return;
    }
    // The account's own profile is read once its list shows (a read; nothing is sent from it).
    if let Some(account) = signed_in(state) {
        if state.data.mine.is_none() && state.mine.asked_for.as_deref() != Some(account.as_str()) {
            state.mine.asked_for = Some(account);
            state.act(cx, anim::time(ui.ctx()), Action::ReadMine);
        }
    }
    settle(state, anim::time(ui.ctx()));
    let h = height(ui, state, ui.available_width());
    let mut go = false;
    super::super::block(ui, cx, index, h, |ui, rect| {
        if signed_in(state).is_none() {
            go = sign_in(ui, rect);
        } else {
            draw(ui, cx, state, rect);
        }
    });
    if go {
        cx.go = Some(super::super::Go::VrchatAccount);
    }
    picker(ui, cx, state);
}

/// Another account on show (or none): what was typed, chosen or sent for the previous one is dropped, never saved to
/// this one.
pub(super) fn sync_account(state: &mut State) {
    let now = signed_in(state);
    if state.mine.account != now {
        let asked_for = state.mine.asked_for.take().filter(|a| Some(a) == now.as_ref());
        state.mine = Mine { account: now, asked_for, ..Mine::default() };
        state.mine_listed.clear();
    }
}

/// The change as it goes out: bound to the account it was made on, and only while that account's list is on show.
fn bound(state: &State, edit: ProfileEdit) -> Option<Action> {
    let account = state.mine.account.clone()?;
    (signed_in(state).as_ref() == Some(&account)).then(|| Action::As { account, action: Box::new(Action::EditProfile(edit)) })
}

/// A press answered: the text goes back to its field when VRChat refused it.
fn settle(state: &mut State, clock: f64) {
    let Some(mine) = state.data.mine.as_ref() else { return };
    if let Some((edit, at, _)) = &state.mine.pending {
        if mine.answered > *at {
            if let Some(Saving::Failed(..)) = &mine.saving {
                state.mine.editing = match edit {
                    ProfileEdit::StatusText(t) => Some(Editing::StatusText(t.clone())),
                    ProfileEdit::Bio(t) => Some(Editing::Bio(t.clone())),
                    _ => state.mine.editing.take(),
                };
            }
            state.mine.pending = None;
        }
    }
    if matches!(mine.saving, Some(Saving::Saved(_))) && state.mine.saved_seen.is_none_or(|(n, _)| n != mine.answered) {
        state.mine.saved_seen = Some((mine.answered, clock));
    }
}

/// Sends a change the person pressed, shown at once.
fn send(state: &mut State, cx: &Cx, clock: f64, edit: ProfileEdit) {
    let Some(action) = bound(state, edit.clone()) else {
        // Typed on an account no longer on show: dropped.
        state.mine = Mine { account: signed_in(state), ..Mine::default() };
        return;
    };
    let answered = state.data.mine.as_ref().map_or(0, |m| m.answered);
    state.mine.pending = Some((edit.clone(), answered, clock));
    state.mine.last = Some(edit);
    if !cx.ctl.launcher(action) {
        // Never reached the service: nothing says it is on its way.
        state.mine.pending = None;
    }
}

/// The card without a VRChat sign-in: what it would do, and the way to sign in (pressed: true).
fn sign_in(ui: &mut Ui, rect: Rect) -> bool {
    let p = ui.painter().clone();
    widgets::frame(&p, rect, theme::CARD_RADIUS, theme::surface());
    paint::stroke_squircle(&p, rect, theme::squircle(theme::CARD_RADIUS), theme::hairline());
    let c = pos2(rect.left() + PAD + 24.0, rect.center().y);
    p.circle_filled(c, 24.0, theme::control());
    widgets::icon(&p, c, icons::USER_CIRCLE, 24.0, theme::FG_3);
    let x = c.x + 40.0;
    let b = widgets::Button::new(tr("Sign in to VRChat")).icon(icons::SIGN_IN).variant(widgets::Variant::Primary).size(widgets::Size::Sm).id_salt("mine-sign-in");
    let bw = b.desired_width(ui);
    let room = rect.right() - PAD - bw - 16.0 - x;
    widgets::text_truncated(&p, pos2(x, rect.center().y - 11.0), Align2::LEFT_CENTER, &Txt::new(tr("Your VRChat profile"), 15.0).weight(weight::SEMIBOLD), room);
    widgets::text_truncated(&p, pos2(x, rect.center().y + 11.0), Align2::LEFT_CENTER, &Txt::new(tr("Sign in to change your status, bio, picture and banner from here."), 12.5).color(theme::FG_3), room);
    b.show_at(ui, Rect::from_min_size(pos2(rect.right() - PAD - bw, rect.center().y - 16.0), vec2(bw, 32.0))).clicked()
}

/// What a part shows while its change is on the way: the change, at once.
fn pending_of<'a>(state: &'a State, part: ProfilePart) -> Option<&'a ProfileEdit> {
    state.mine.pending.as_ref().map(|(e, ..)| e).filter(|e| e.part() == part)
}

fn draw(ui: &mut Ui, cx: &Cx, state: &mut State, rect: Rect) {
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let mine = shown_mine(state).unwrap_or_default();
    let Some(me) = state.data.me.clone() else {
        let p = ui.painter();
        widgets::frame(p, rect, theme::CARD_RADIUS, theme::surface());
        feedback::shimmer(p, rect, theme::CARD_RADIUS);
        return;
    };
    let p = ui.painter().clone();
    paint::shadow(&p, rect, theme::CARD_RADIUS, vec2(0.0, 8.0), 24.0, -8.0, theme::black(0.25));
    widgets::frame(&p, rect, theme::CARD_RADIUS, theme::surface());
    paint::stroke_squircle(&p, rect, theme::squircle(theme::CARD_RADIUS), theme::hairline());

    // The banner, pressed to choose another.
    let banner = Rect::from_min_size(rect.min, vec2(rect.width(), BANNER_H));
    let banner_id = Id::unique("mine-banner");
    let banner_resp = ui.interact(banner, banner_id, Sense::click());
    let banner_hover = anim::toggle(&ctx, banner_id.with("hover"), banner_resp.hovered(), Spring::SNAPPY);
    let (choice, colour, custom) = match pending_of(state, ProfilePart::Banner) {
        Some(ProfileEdit::Banner { choice, colour, custom }) => (choice.clone(), colour.or(mine.banner_colour), custom.clone()),
        _ => (mine.banner.clone(), mine.banner_colour.or(me.look.banner_colour), mine.banner_custom.clone()),
    };
    let banner_url = match choice {
        BannerChoice::Custom if !custom.is_empty() => custom,
        BannerChoice::Avatar => mine.avatar_picture.clone(),
        // Before the profile is read: what the list shows.
        _ if state.data.mine.is_none() => me.look.banner.clone(),
        _ => String::new(),
    };
    let radii = [theme::CARD_RADIUS, theme::CARD_RADIUS, 0.0, 0.0];
    match (choice == BannerChoice::Colour).then_some(()).map_or_else(|| state.picture(&banner_url), |_| None) {
        Some(t) => paint::squircle_texture(&p, banner, radii, t.id(), cover_uv(t.size_vec2(), banner.size()), Color32::WHITE),
        None => {
            let [r, g, b] = colour.unwrap_or([60, 70, 110]);
            let c = Color32::from_rgb(r, g, b);
            paint::fill_convex(&p, &paint::squircle_points(banner, theme::squircle(theme::CARD_RADIUS)), |at| {
                let t = ((at.x - banner.left()) / banner.width() * 0.7 + (at.y - banner.top()) / banner.height() * 0.3).clamp(0.0, 1.0);
                theme::mix(c, theme::mix(c, theme::INK_900, 0.55), t)
            });
        }
    }
    // A veil at its foot, so the picture and the name read over any banner.
    paint::fill_convex(&p, &[banner.left_top(), banner.right_top(), banner.right_bottom(), banner.left_bottom()], |at| theme::black(0.32 * ((at.y - banner.top()) / banner.height()).powi(2) + 0.12 * banner_hover));
    let change = Txt::new(tr("Change banner"), 12.0).weight(weight::MEDIUM).color(theme::FG);
    let cw = widgets::text_size(ui, &change).x + 36.0;
    let change_rect = Rect::from_min_size(pos2(banner.right() - 12.0 - cw, banner.top() + 12.0), vec2(cw, 26.0));
    p.rect_filled(change_rect, theme::radius(13.0), theme::black(0.45 + 0.2 * banner_hover));
    widgets::icon(&p, pos2(change_rect.left() + 14.0, change_rect.center().y), icons::IMAGE, 13.0, theme::FG);
    widgets::text(&p, pos2(change_rect.left() + 26.0, change_rect.center().y), Align2::LEFT_CENTER, &change);
    if widgets::activated(&banner_resp) {
        state.mine.picker = Some((Picker::Banner, change_rect, clock));
    }
    // An open chooser stays under what opened it (the page scrolls).
    match &mut state.mine.picker {
        Some((Picker::Banner, anchor, _)) => *anchor = change_rect,
        Some((Picker::Picture, anchor, _)) => *anchor = Rect::from_min_size(pos2(rect.left() + PAD, banner.bottom() - ICON * 0.5), vec2(ICON, ICON)),
        None => {}
    }
    banner_resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Choose your banner: a colour, your avatar's picture or one of your gallery"));

    // The picture, over the banner's edge, pressed to choose another.
    let pic = Rect::from_min_size(pos2(rect.left() + PAD, banner.bottom() - ICON * 0.5), vec2(ICON, ICON));
    let pic_id = Id::unique("mine-picture");
    let pic_resp = ui.interact(pic.expand(3.0), pic_id, Sense::click());
    let pic_hover = anim::toggle(&ctx, pic_id.with("hover"), pic_resp.hovered(), Spring::SNAPPY);
    let icon_url = match pending_of(state, ProfilePart::Picture) {
        Some(ProfileEdit::Picture(url)) if url.is_empty() => mine.avatar_picture.clone(),
        Some(ProfileEdit::Picture(url)) => url.clone(),
        _ => me.look.icon.clone(),
    };
    p.circle_filled(pic.center(), ICON * 0.5 + 4.0, theme::surface());
    let texture = state.picture(&icon_url);
    widgets::avatar(&p, pic, texture.as_ref(), &me.name);
    if pic_hover > 0.01 {
        p.circle_filled(pic.center(), ICON * 0.5, theme::black(0.45 * pic_hover));
        let mut hp = p.clone();
        hp.multiply_opacity(pic_hover);
        widgets::icon(&hp, pic.center(), icons::PENCIL_SIMPLE, 20.0, theme::FG);
    }
    // Its status dot at its foot.
    let status = match pending_of(state, ProfilePart::Status) {
        Some(ProfileEdit::Status(s)) => *s,
        _ => me.status,
    };
    let dot = pic.right_bottom() - vec2(11.0, 11.0);
    p.circle_filled(dot, 11.0, theme::surface());
    p.circle_filled(dot, 7.5, status_color(status));
    if widgets::activated(&pic_resp) {
        state.mine.picker = Some((Picker::Picture, pic, clock));
    }
    pic_resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Choose your profile picture"));

    // The name, the status (pressed to choose), what is being saved.
    let x = pic.right() + 16.0;
    let right = rect.right() - PAD;
    let name_y = banner.bottom() + NAME_DY;
    let name = Txt::new(&me.name, 18.0).weight(weight::SEMIBOLD).tracking(-0.01);
    let name_r = widgets::text_truncated(&p, pos2(x, name_y), Align2::LEFT_CENTER, &name, (right - x) * 0.45);
    let status_label = super::status_name(status);
    let st = Txt::new(status_label, 12.5).weight(weight::MEDIUM);
    let sw = widgets::text_size(ui, &st).x + 48.0;
    let status_rect = Rect::from_min_size(pos2(name_r.right() + 12.0, name_y - 14.0), vec2(sw, 28.0));
    let status_id = Id::unique("mine-status");
    let status_resp = ui.interact(status_rect, status_id, Sense::click());
    let sh = anim::toggle(&ctx, status_id.with("hover"), status_resp.hovered() || popup::is_open(&ctx, status_id.with("list")), Spring::SNAPPY);
    p.rect_filled(status_rect, theme::radius(14.0), theme::mix(theme::alpha(status_color(status), 0.14), theme::alpha(status_color(status), 0.26), sh));
    p.circle_filled(pos2(status_rect.left() + 15.0, status_rect.center().y), 4.5, status_color(status));
    widgets::text(&p, pos2(status_rect.left() + 26.0, status_rect.center().y), Align2::LEFT_CENTER, &st);
    widgets::icon(&p, pos2(status_rect.right() - 13.0, status_rect.center().y), icons::CARET_DOWN, 11.0, theme::FG_2);
    widgets::note_control(&ctx, status_label, status_rect, true);
    let details = [tr("Friends join you without asking"), tr("Friends can join you"), tr("Friends ask before they join"), tr("No invites and no requests")];
    let items: Vec<popup::Item> = SETTABLE_STATUSES.iter().zip(details).map(|(s, d)| popup::Item::new(status_name(*s)).detail(d).icon(icons::CIRCLE)).collect();
    let selected = SETTABLE_STATUSES.iter().position(|s| *s == status);
    let pressed = widgets::activated(&status_resp);
    if let Some(i) = popup::list_from(&ctx, status_id.with("list"), status_rect, pressed, &items, selected, 300.0) {
        let chosen = SETTABLE_STATUSES[i];
        if chosen != me.status {
            send(state, cx, clock, ProfileEdit::Status(chosen));
        }
    }
    status_resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Choose your status"));
    saving_line(ui, state, &mine, Rect::from_min_max(pos2(status_rect.right() + 12.0, name_y - 14.0), pos2(right, name_y + 14.0)), cx, clock);

    // The status text, pressed to type another.
    let text_y = banner.bottom() + TEXT_DY;
    let text_rect = Rect::from_min_max(pos2(x - 8.0, text_y - 15.0), pos2(right, text_y + 15.0));
    let shown_text = match pending_of(state, ProfilePart::StatusText) {
        Some(ProfileEdit::StatusText(t)) => t.clone(),
        _ => me.status_text.clone(),
    };
    match state.mine.editing.clone() {
        Some(Editing::StatusText(mut draft)) => {
            let done = text_editor(ui, Id::unique("mine-status-text"), text_rect, &mut draft, tr("What you are up to"), STATUS_TEXT_MAX);
            match done {
                Some(true) => {
                    state.mine.editing = None;
                    if draft != me.status_text {
                        send(state, cx, clock, ProfileEdit::StatusText(draft));
                    }
                }
                Some(false) => state.mine.editing = None,
                None => state.mine.editing = Some(Editing::StatusText(draft)),
            }
        }
        _ => {
            let placeholder = shown_text.trim().is_empty();
            let line = if placeholder { tr("Add a status text").to_string() } else { shown_text.clone() };
            if editable(ui, Id::unique("mine-status-text-idle"), text_rect, &Txt::new(line, 13.5).color(if placeholder { theme::FG_4 } else { theme::FG_2 }), tr("Change your status text")) {
                state.mine.editing = Some(Editing::StatusText(shown_text));
            }
        }
    }

    // The bio, pressed to write another.
    // Why the last change did not go, in full, with Try again.
    if let Some(why) = failed(state) {
        failed_line(ui, cx, state, Rect::from_min_max(pos2(x - 8.0, text_y + 17.0), pos2(right, text_y + 17.0 + FAILED_H - 4.0)), &why, clock);
    }
    let bio_top = banner.bottom() + BIO_DY + failed_h(state);
    let bio_rect = Rect::from_min_max(pos2(rect.left() + PAD - 8.0, bio_top - 6.0), pos2(right + 8.0, rect.bottom() - PAD + 6.0));
    let shown_bio = match pending_of(state, ProfilePart::Bio) {
        Some(ProfileEdit::Bio(t)) => t.clone(),
        _ => mine.bio.clone(),
    };
    match state.mine.editing.clone() {
        Some(Editing::Bio(mut draft)) => {
            let field = Rect::from_min_size(bio_rect.min, vec2(bio_rect.width(), BIO_EDIT_H));
            let done = bio_editor(ui, field, &mut draft);
            let by = field.bottom() + 8.0;
            let count = Txt::new(format!("{} / {BIO_MAX}", draft.chars().count()), 12.0).color(theme::FG_4);
            widgets::text(ui.painter(), pos2(bio_rect.left() + 4.0, by + BUTTONS_H * 0.5), Align2::LEFT_CENTER, &count);
            let save = widgets::Button::new(tr("Save")).icon(icons::CHECK).variant(widgets::Variant::Primary).size(widgets::Size::Sm).id_salt("mine-bio-save");
            let sw = save.desired_width(ui);
            let cancel = widgets::Button::new(tr("Cancel")).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("mine-bio-cancel");
            let cw = cancel.desired_width(ui);
            let saved = save.show_at(ui, Rect::from_min_size(pos2(bio_rect.right() - sw, by), vec2(sw, BUTTONS_H))).clicked();
            let cancelled = cancel.show_at(ui, Rect::from_min_size(pos2(bio_rect.right() - sw - 8.0 - cw, by), vec2(cw, BUTTONS_H))).clicked();
            match (done, saved, cancelled) {
                (Some(true), ..) | (_, true, _) => {
                    state.mine.editing = None;
                    if draft != mine.bio {
                        send(state, cx, clock, ProfileEdit::Bio(draft));
                    }
                }
                (Some(false), ..) | (_, _, true) => state.mine.editing = None,
                _ => state.mine.editing = Some(Editing::Bio(draft)),
            }
        }
        _ => {
            let empty = shown_bio.trim().is_empty();
            let txt = if empty { Txt::new(tr("Write a few words about you"), 13.0).color(theme::FG_4) } else { Txt::new(shown_bio.trim(), 13.0).color(theme::FG_2).line_height(BIO_LINE) };
            let id = Id::unique("mine-bio");
            let resp = ui.interact(bio_rect, id, Sense::click());
            let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
            p.rect_filled(bio_rect, theme::radius(10.0), theme::white(0.04 * hover));
            let mut job = txt.job(bio_rect.width() - 16.0 - 24.0);
            job.wrap.max_rows = BIO_ROWS_IDLE;
            let galley = ui.painter().layout_job(job);
            p.galley(bio_rect.min + vec2(8.0, 6.0), galley, theme::FG_2);
            let mut hp = p.clone();
            hp.multiply_opacity(0.35 + 0.65 * hover);
            widgets::icon(&hp, pos2(bio_rect.right() - 14.0, bio_rect.top() + 16.0), icons::PENCIL_SIMPLE, 14.0, theme::FG_3);
            if widgets::activated(&resp) {
                state.mine.editing = Some(Editing::Bio(shown_bio));
            }
            resp.on_hover_cursor(CursorIcon::Text).on_hover_text(tr("Change your bio"));
        }
    }
}

/// A line of text that turns into its field when pressed: a soft well on hover and a pencil.
fn editable(ui: &mut Ui, id: Id, rect: Rect, txt: &Txt, hint: &str) -> bool {
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, id, Sense::click());
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(10.0), theme::white(0.045 * hover));
    let r = widgets::text_truncated(p, pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, txt, rect.width() - 40.0);
    let mut hp = p.clone();
    hp.multiply_opacity(0.35 + 0.65 * hover);
    widgets::icon(&hp, pos2((r.right() + 16.0).min(rect.right() - 14.0), rect.center().y), icons::PENCIL_SIMPLE, 13.0, theme::FG_3);
    let pressed = widgets::activated(&resp);
    resp.on_hover_cursor(CursorIcon::Text).on_hover_text(hint);
    pressed
}

/// A one-line field over `rect`, focused, at most `max` characters, its count and ✓ / ×: `Some(true)` saved (Enter
/// or ✓), `Some(false)` put away (Escape or ×), `None` still typing.
fn text_editor(ui: &mut Ui, id: Id, rect: Rect, draft: &mut String, hint: &str, max: usize) -> Option<bool> {
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::radius(10.0), theme::INK_750);
    p.rect_stroke(rect, theme::radius(10.0), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.8)), egui::StrokeKind::Outside);
    let count = Txt::new(format!("{} / {max}", draft.chars().count()), 11.5).color(theme::FG_4);
    let count_w = widgets::text_size(ui, &count).x;
    let ok = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(26.0, 26.0));
    let no = ok.translate(vec2(-30.0, 0.0));
    widgets::text(&p, pos2(no.left() - 8.0, rect.center().y), Align2::RIGHT_CENTER, &count);
    let field = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 4.0), pos2(no.left() - 16.0 - count_w, rect.bottom() - 4.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(field));
    let font = theme::font(13.5, weight::REGULAR);
    let resp = child.add(
        egui::TextEdit::singleline(draft)
            .id(id)
            .frame(egui::Frame::NONE)
            .char_limit(max)
            .font(font.clone())
            .text_color(theme::FG)
            .hint_text(egui::RichText::new(hint).color(theme::FG_4).font(font))
            .desired_width(field.width())
            .min_size(field.size()),
    );
    if !resp.has_focus() && !resp.lost_focus() && ui.ctx().memory(|m| m.focused().is_none()) {
        resp.request_focus();
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    let saved = widgets::activated(&widgets::IconButton::new(icons::CHECK, orbiters_i18n::mark("Save")).size(26.0, 13.0).id_salt(id.with("ok")).show_at(ui, ok));
    let dropped = widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Cancel")).size(26.0, 13.0).id_salt(id.with("no")).show_at(ui, no));
    if enter || saved {
        Some(true)
    } else if escape || dropped {
        Some(false)
    } else {
        None
    }
}

/// The bio's field: several lines, at most [`BIO_MAX`] characters; Ctrl+Enter saves, Escape puts it away.
fn bio_editor(ui: &mut Ui, rect: Rect, draft: &mut String) -> Option<bool> {
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::radius(12.0), theme::INK_750);
    p.rect_stroke(rect, theme::radius(12.0), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.8)), egui::StrokeKind::Outside);
    let inner = rect.shrink2(vec2(10.0, 8.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
    let font = theme::font(13.0, weight::REGULAR);
    let id = Id::unique("mine-bio-field");
    let resp = egui::ScrollArea::vertical()
        .id_salt("mine-bio-scroll")
        .max_height(inner.height())
        .show(&mut child, |ui| {
            ui.add(
                egui::TextEdit::multiline(draft)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .char_limit(BIO_MAX)
                    .font(font.clone())
                    .text_color(theme::FG)
                    .hint_text(egui::RichText::new(tr("Write a few words about you")).color(theme::FG_4).font(font))
                    .desired_width(inner.width())
                    .desired_rows(4),
            )
        })
        .inner;
    if !resp.has_focus() && !resp.lost_focus() && ui.ctx().memory(|m| m.focused().is_none()) {
        resp.request_focus();
    }
    let save = ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    if save {
        Some(true)
    } else if escape {
        Some(false)
    } else {
        None
    }
}

/// Saving…, Saved, or why a change did not go, with Try again.
fn saving_line(ui: &mut Ui, state: &mut State, mine: &MyProfile, rect: Rect, cx: &Cx, clock: f64) {
    let p = ui.painter().clone();
    let at = pos2(rect.right(), rect.center().y);
    let waiting = state.mine.pending.is_some();
    let _ = cx;
    match &mine.saving {
        _ if waiting => {
            let t = Txt::new(tr("Saving…"), 12.5).color(theme::FG_3);
            let r = widgets::text(&p, at, Align2::RIGHT_CENTER, &t);
            feedback::spinner(&p, pos2(r.left() - 12.0, at.y), 6.0, 2.0, theme::FG_3);
            ui.ctx().request_repaint();
        }
        Some(Saving::Saved(_)) => {
            let Some((_, seen)) = state.mine.saved_seen else { return };
            let left = SAVED_FOR - (clock - seen);
            if left <= 0.0 {
                return;
            }
            let k = anim::toggle(ui.ctx(), Id::unique("mine-saved"), left > 0.4, Spring::SOFT).clamp(0.0, 1.0);
            let mut fp = p.clone();
            fp.multiply_opacity(k);
            let r = widgets::text(&fp, at, Align2::RIGHT_CENTER, &Txt::new(tr("Saved"), 12.5).weight(weight::MEDIUM).color(theme::ACCENT));
            widgets::icon(&fp, pos2(r.left() - 11.0, at.y), icons::CHECK_CIRCLE, 14.0, theme::ACCENT);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        _ => {}
    }
}

/// Why a change did not go, in a soft red band, and Try again (the same change, sent again on the press).
fn failed_line(ui: &mut Ui, cx: &Cx, state: &mut State, rect: Rect, why: &str, clock: f64) {
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::radius(8.0), theme::alpha(theme::DANGER, 0.1));
    widgets::icon(&p, pos2(rect.left() + 16.0, rect.center().y), icons::WARNING_CIRCLE, 14.0, theme::DANGER);
    let again = widgets::Button::new(tr("Try again")).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("mine-again");
    let aw = again.desired_width(ui);
    let mut right = rect.right() - 4.0;
    if state.mine.last.is_some() {
        let br = Rect::from_min_size(pos2(right - aw, rect.center().y - 13.0), vec2(aw, 26.0));
        if again.show_at(ui, br).clicked() {
            if let Some(edit) = state.mine.last.clone() {
                state.mine.editing = None;
                send(state, cx, clock, edit);
            }
        }
        right -= aw + 6.0;
    }
    let t = Txt::new(why, 12.5).color(theme::mix(theme::DANGER, theme::FG, 0.35));
    let r = widgets::text_truncated(&p, pos2(rect.left() + 30.0, rect.center().y), Align2::LEFT_CENTER, &t, right - rect.left() - 34.0);
    ui.interact(r.expand(4.0), Id::unique("mine-failed"), Sense::hover()).on_hover_text(why);
}

/// What a chooser says of what VRChat allows, in plain words.
fn picker_note(kind: Picker, tab: BannerTab, mine: &MyProfile, list: Option<&Vec<OwnPicture>>) -> &'static str {
    match (kind, tab) {
        (Picker::Picture, _) if !mine.supporter => tr("Your own icons are a VRC+ feature: with VRC+, upload them in VRChat and choose them here. Without it, your avatar's picture shows."),
        (Picker::Picture, _) if list.is_some_and(Vec::is_empty) => tr("No icons yet: upload one in VRChat (Gallery, Icons), then open this again."),
        (Picker::Picture, _) => tr("Your icons uploaded in VRChat."),
        (Picker::Banner, BannerTab::Colour) => tr("A colour behind your name on your profile."),
        (Picker::Banner, BannerTab::Avatar) => tr("The picture of the avatar you wear, behind your name."),
        (Picker::Banner, BannerTab::Gallery) if !mine.supporter => tr("Gallery pictures are uploaded with VRC+ in VRChat: without it, choose a colour or your avatar's picture."),
        (Picker::Banner, BannerTab::Gallery) if list.is_some_and(Vec::is_empty) => tr("Your gallery is empty: upload a picture in VRChat (Gallery), then open this again."),
        (Picker::Banner, BannerTab::Gallery) => tr("Pictures of your VRChat gallery."),
    }
}

/// The picture or banner chooser, under what opened it; a press outside or Escape puts it away.
fn picker(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let Some((kind, anchor, opened)) = state.mine.picker else { return };
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let mine = shown_mine(state).unwrap_or_default();
    let w: f32 = 384.0;
    let screen = ctx.content_rect();
    // Towards the card: from what opened it, on its side of the page.
    let x = if anchor.center().x > screen.center().x { anchor.right() - w } else { anchor.left() };
    let x = x.min(screen.right() - w - 16.0).max(screen.left() + 16.0);
    let list = match kind {
        Picker::Picture => mine.icons.clone(),
        Picker::Banner => mine.gallery.clone(),
    };
    let tab = state.mine.banner_tab.unwrap_or(match mine.banner {
        BannerChoice::Colour => BannerTab::Colour,
        BannerChoice::Avatar => BannerTab::Avatar,
        BannerChoice::Custom => BannerTab::Gallery,
    });
    // What the person may choose among is read when the chooser opens (once per run).
    let wanted = match (kind, tab) {
        (Picker::Picture, _) => Some(PictureKind::Icons),
        (Picker::Banner, BannerTab::Gallery) => Some(PictureKind::Gallery),
        _ => None,
    };
    if let Some(want) = wanted {
        if list.is_none() && mine.listing.is_none() && !state.mine_listed.contains(&want) {
            state.mine_listed.push(want);
            state.act(cx, clock, Action::ListPictures(want));
        }
    }
    let thumbs = match (kind, tab) {
        (Picker::Picture, _) => 1 + list.as_ref().map_or(0, Vec::len),
        (Picker::Banner, BannerTab::Gallery) => list.as_ref().map_or(0, Vec::len),
        _ => 0,
    };
    let (gap, per_row) = match kind {
        Picker::Picture => (10.0, 5usize),
        Picker::Banner => (10.0, 3usize),
    };
    let cell = ((w - 32.0 - gap * (per_row - 1) as f32) / per_row as f32).floor();
    let cell_h = if kind == Picker::Picture { cell } else { cell * 9.0 / 16.0 };
    let rows = thumbs.div_ceil(per_row).clamp(1, 3);
    let body_h = match (kind, tab) {
        (Picker::Banner, BannerTab::Colour) => 2.0 * 40.0 + 10.0,
        (Picker::Banner, BannerTab::Avatar) => 96.0,
        _ => rows as f32 * (cell_h + gap) - gap,
    };
    let tabs_h = if kind == Picker::Banner { 44.0 } else { 0.0 };
    let note = picker_note(kind, tab, &mine, list.as_ref());
    let note_galley = ui.painter().layout_job(Txt::new(note, 12.0).color(theme::FG_3).line_height(16.0).job(w - 32.0));
    let note_h = note_galley.size().y.max(16.0);
    let h = 16.0 + 26.0 + 10.0 + tabs_h + body_h + 12.0 + note_h + 16.0;
    let below = anchor.bottom() + 8.0;
    let y = if below + h > screen.bottom() - 12.0 { (anchor.top() - 8.0 - h).max(screen.top() + 12.0) } else { below };
    let base = Rect::from_min_size(pos2(x, y), vec2(w, h));
    let id = Id::unique("mine-picker");
    let pop = popup::popover(&ctx, id.with(opened.to_bits()), anchor, base, true);
    let layer = egui::LayerId::new(Order::Foreground, id);
    let mut over = ui.new_child(UiBuilder::new().max_rect(screen).layer_id(layer));
    over.multiply_opacity(pop.opacity);
    let rect = pop.rect;
    let p = over.painter().clone();
    paint::shadow(&p, rect, 18.0, vec2(0.0, 14.0), 36.0, -6.0, theme::black(0.5));
    widgets::frame(&p, rect, 18.0, theme::surface_strong());
    paint::stroke_squircle(&p, rect, theme::squircle(18.0), theme::hairline());
    let panel = over.interact(rect, id.with("panel"), Sense::click());
    let inner = rect.shrink(16.0);
    let title = match kind {
        Picker::Picture => tr("Profile picture"),
        Picker::Banner => tr("Banner"),
    };
    widgets::text(&p, pos2(inner.left(), inner.top() + 13.0), Align2::LEFT_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD));
    let close = Rect::from_center_size(pos2(inner.right() - 12.0, inner.top() + 13.0), vec2(28.0, 28.0));
    let mut closed = widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Close")).size(28.0, 13.0).id_salt("mine-picker-close").show_at(&mut over, close));
    let mut y = inner.top() + 26.0 + 10.0;
    let mut chosen: Option<ProfileEdit> = None;
    if kind == Picker::Banner {
        let options = [
            widgets::SegOption::new(BannerTab::Colour, tr("Colour")),
            widgets::SegOption::new(BannerTab::Avatar, tr("Avatar's picture")),
            widgets::SegOption::new(BannerTab::Gallery, tr("Gallery")),
        ];
        let mut t = tab;
        let mut seg = over.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 34.0))));
        if widgets::segmented(&mut seg, "mine-banner-tab", &mut t, &options, true) {
            state.mine.banner_tab = Some(t);
        }
        y += tabs_h;
    }
    let body = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), body_h));
    match (kind, tab) {
        (Picker::Banner, BannerTab::Colour) => {
            let n = BANNER_COLOURS.len();
            let per = n.div_ceil(2);
            let sz = (body.width() - (per - 1) as f32 * 10.0) / per as f32;
            for (i, [r, g, b]) in BANNER_COLOURS.iter().enumerate() {
                let at = pos2(body.left() + (i % per) as f32 * (sz + 10.0), body.top() + (i / per) as f32 * (40.0 + 10.0));
                let sw = Rect::from_min_size(at, vec2(sz, 40.0));
                let sid = id.with(("swatch", i));
                let resp = over.interact(sw, sid, Sense::click());
                let hv = anim::toggle(&ctx, sid.with("h"), resp.hovered(), Spring::SNAPPY);
                let current = mine.banner == BannerChoice::Colour && mine.banner_colour == Some([*r, *g, *b]);
                p.rect_filled(sw.shrink(2.0 * (1.0 - hv)), theme::radius(10.0), Color32::from_rgb(*r, *g, *b));
                if current {
                    p.rect_stroke(sw.expand(2.0), theme::radius(12.0), egui::Stroke::new(2.0, theme::FG), egui::StrokeKind::Outside);
                    widgets::icon(&p, sw.center(), icons::CHECK, 16.0, Color32::WHITE);
                }
                if widgets::activated(&resp) {
                    chosen = Some(ProfileEdit::Banner { choice: BannerChoice::Colour, colour: Some([*r, *g, *b]), custom: String::new() });
                }
                resp.on_hover_cursor(CursorIcon::PointingHand);
            }
        }
        (Picker::Banner, BannerTab::Avatar) => {
            let prev = Rect::from_min_size(body.min, vec2(body.height() * 17.0 / 6.0, body.height()).min(vec2(body.width() * 0.55, body.height())));
            match state.picture(&mine.avatar_picture) {
                Some(t) => paint::squircle_texture(&p, prev, [10.0; 4], t.id(), cover_uv(t.size_vec2(), prev.size()), Color32::WHITE),
                None => widgets::frame(&p, prev, 10.0, theme::control()),
            }
            let b = widgets::Button::new(tr("Use it")).icon(icons::CHECK).variant(widgets::Variant::Primary).size(widgets::Size::Sm).id_salt("mine-banner-avatar").enabled(mine.banner != BannerChoice::Avatar);
            let bw = b.desired_width(&over);
            if b.show_at(&mut over, Rect::from_min_size(pos2(body.right() - bw, body.center().y - 16.0), vec2(bw, 32.0))).clicked() {
                chosen = Some(ProfileEdit::Banner { choice: BannerChoice::Avatar, colour: None, custom: String::new() });
            }
        }
        _ => {
            // A grid of the account's own pictures (the picture's chooser begins with the avatar's).
            let mut items: Vec<(String, bool)> = Vec::new();
            if kind == Picker::Picture {
                items.push((String::new(), mine.icon.is_empty()));
            }
            for OwnPicture { url, .. } in list.iter().flatten() {
                let current = match kind {
                    Picker::Picture => *url == mine.icon,
                    Picker::Banner => mine.banner == BannerChoice::Custom && *url == mine.banner_custom,
                };
                items.push((url.clone(), current));
            }
            let listing = mine.listing.is_some() || (wanted.is_some() && list.is_none());
            for (i, (url, current)) in items.iter().enumerate().take(per_row * 3) {
                let at = pos2(body.left() + (i % per_row) as f32 * (cell + gap), body.top() + (i / per_row) as f32 * (cell_h + gap));
                let r = Rect::from_min_size(at, vec2(cell, cell_h));
                let tid = id.with(("thumb", i));
                let resp = over.interact(r, tid, Sense::click());
                let hv = anim::toggle(&ctx, tid.with("h"), resp.hovered(), Spring::SNAPPY);
                let shown_url = if url.is_empty() { mine.avatar_picture.clone() } else { url.clone() };
                let texture = state.picture(&shown_url);
                let rr = r.shrink(2.0 * (1.0 - hv));
                if kind == Picker::Picture {
                    widgets::avatar(&p, rr, texture.as_ref(), "");
                    if *current {
                        p.circle_stroke(r.center(), r.width() * 0.5 + 2.0, egui::Stroke::new(2.0, theme::ACCENT));
                    }
                } else {
                    match texture {
                        Some(t) => paint::squircle_texture(&p, rr, [8.0; 4], t.id(), cover_uv(t.size_vec2(), rr.size()), Color32::WHITE),
                        None => widgets::frame(&p, rr, 8.0, theme::control()),
                    }
                    if *current {
                        p.rect_stroke(r.expand(1.0), theme::radius(9.0), egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Outside);
                    }
                }
                if url.is_empty() {
                    let tag = Txt::new(tr("Avatar"), 10.5).weight(weight::SEMIBOLD).color(theme::FG);
                    let tw = widgets::text_size(&over, &tag).x + 12.0;
                    let tr_ = Rect::from_center_size(pos2(r.center().x, r.bottom() - 8.0), vec2(tw, 16.0));
                    p.rect_filled(tr_, theme::radius(8.0), theme::black(0.65));
                    widgets::text(&p, tr_.center(), Align2::CENTER_CENTER, &tag);
                }
                if widgets::activated(&resp) && !current {
                    chosen = Some(match kind {
                        Picker::Picture => ProfileEdit::Picture(url.clone()),
                        Picker::Banner => ProfileEdit::Banner { choice: BannerChoice::Custom, colour: None, custom: url.clone() },
                    });
                }
                resp.on_hover_cursor(CursorIcon::PointingHand);
            }
            if listing {
                let c = if items.is_empty() { body.center() } else { pos2(body.left() + (items.len() % per_row) as f32 * (cell + gap) + cell * 0.5, body.top() + cell_h * 0.5) };
                feedback::spinner(&p, c, 9.0, 2.5, theme::FG_3);
                ctx.request_repaint();
            }
        }
    }
    // What VRChat allows, in plain words.
    p.galley(pos2(inner.left(), body.bottom() + 12.0), note_galley, theme::FG_3);
    if let Some(edit) = chosen {
        send(state, cx, clock, edit);
        closed = true;
    }
    // Away: a press outside (not on what opened it), Escape.
    let outside = ctx.input(|i| i.pointer.any_pressed()) && !panel.hovered() && !ctx.pointer_interact_pos().is_some_and(|p| rect.contains(p) || anchor.contains(p)) && !popup::any_open(&ctx);
    if closed || outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
        state.mine.picker = None;
        state.mine.banner_tab = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::launcher::sample;
    use std::sync::Arc;

    #[test]
    fn a_refused_text_goes_back_to_its_field_and_a_saved_one_closes_it() {
        let mut state = State { data: Arc::new(sample()), ..Default::default() };
        state.mine.pending = Some((ProfileEdit::StatusText("afk".into()), 0, 1.0));
        // VRChat has not answered yet: nothing changes.
        settle(&mut state, 2.0);
        assert!(state.mine.pending.is_some() && state.mine.editing.is_none());
        let mut data = sample();
        let mine = data.mine.as_mut().unwrap();
        mine.saving = Some(Saving::Failed(ProfilePart::StatusText, "no".into()));
        mine.answered = 1;
        state.data = Arc::new(data.clone());
        settle(&mut state, 3.0);
        assert_eq!(state.mine.editing, Some(Editing::StatusText("afk".into())), "the words typed are not lost");
        assert!(state.mine.pending.is_none());
        // Typed again and saved.
        state.mine.editing = None;
        state.mine.pending = Some((ProfileEdit::StatusText("afk".into()), 1, 4.0));
        let mine = data.mine.as_mut().unwrap();
        mine.saving = Some(Saving::Saved(ProfilePart::StatusText));
        mine.answered = 2;
        state.data = Arc::new(data);
        settle(&mut state, 5.0);
        assert!(state.mine.editing.is_none() && state.mine.pending.is_none());
        assert_eq!(state.mine.saved_seen, Some((2, 5.0)), "Saved shows from now");
    }

    #[test]
    fn a_draft_typed_on_one_account_is_dropped_when_another_shows() {
        let mut data = sample();
        data.source = Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
        let mut state = State { data: Arc::new(data.clone()), ..Default::default() };
        sync_account(&mut state);
        state.mine.editing = Some(Editing::Bio("written as A".into()));
        state.mine.pending = Some((ProfileEdit::StatusText("A's".into()), 0, 1.0));
        state.mine.last = Some(ProfileEdit::Picture("https://api.vrchat.cloud/api/1/file/file_a/1/file".into()));
        state.mine.picker = Some((Picker::Picture, Rect::NOTHING, 1.0));
        assert!(bound(&state, ProfileEdit::Bio("x".into())).is_some());
        // The person signs in to B: the same frame, before the card can save anything.
        data.source = Source::SignedIn { user_id: "usr_b".into(), name: String::new() };
        state.data = Arc::new(data.clone());
        assert_eq!(bound(&state, ProfileEdit::Bio("written as A".into())), None, "never labelled with B's account");
        sync_account(&mut state);
        assert!(state.mine.editing.is_none() && state.mine.pending.is_none() && state.mine.last.is_none() && state.mine.picker.is_none());
        assert_eq!(state.mine.account.as_deref(), Some("usr_b"));
        // Signed out: nothing is bound.
        data.source = Source::Nothing;
        state.data = Arc::new(data);
        sync_account(&mut state);
        assert_eq!(bound(&state, ProfileEdit::Bio("x".into())), None);
    }

    #[test]
    fn a_change_is_sent_only_as_the_lists_account_and_shown_at_once() {
        let mut data = sample();
        data.source = Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
        let mut state = State { data: Arc::new(data), ..Default::default() };
        let mut sent = Vec::new();
        state.press(1.0, Action::EditProfile(ProfileEdit::Status(Status::Busy)), |a| {
            sent.push(a);
            true
        });
        assert_eq!(sent, [Action::As { account: "usr_a".into(), action: Box::new(Action::EditProfile(ProfileEdit::Status(Status::Busy))) }]);
        state.mine.pending = Some((ProfileEdit::Status(Status::Busy), 0, 1.0));
        assert_eq!(pending_of(&state, ProfilePart::Status), Some(&ProfileEdit::Status(Status::Busy)));
        assert_eq!(pending_of(&state, ProfilePart::Bio), None);
    }
}
