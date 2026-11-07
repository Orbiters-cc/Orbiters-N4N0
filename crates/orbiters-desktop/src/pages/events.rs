//! Events: what Orbiters lists (the website's homepage events, and the ones under way), soonest first. The next one is
//! a hero with its cover's light behind it; the rest are cards grouped by day, each with its cover, its status (a
//! countdown, or Happening now with a pulse), when in this computer's time zone, who hosts it and where. Join opens the
//! event's window, grown out of its card ([`Window`], also opened from Photos & captures' event galleries): its cover,
//! status and time, the host, what it is, its program step by step with each world, its pictures, and Invite me.
//!
//! Invite me is the website's step sign-up (`orbiters_core::community`): the community's VRChat account invites the
//! member when the step starts and its instance is ready, as long as VRChat is linked on Orbiters, the member is in
//! the group with its role, and that account is their friend (each missing piece is said, with its way out). For an
//! instance under way that takes no sign-ups, Orbiters' own VRChat account invites them there, as Home's Previous
//! instances do (`orbiters_n4n0_core::invites`).

use super::{sharing, Cx, Job};
use crate::worlds::{Picture, Pictures};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Order, Painter, Pos2, Rect, Sense, TextureHandle, Ui, UiBuilder};
use orbiters_core::account::{Api, Server};
use orbiters_core::community::{self, Card, Cards, Friend, Invites, Overview, Refused, Signup};
use orbiters_i18n::{mark, tr, tr_n};
use orbiters_n4n0_core::menu::InviteAnswer;
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::viewer::{self, Morph};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{feedback, icons, logo, morph};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use time::{OffsetDateTime, UtcOffset};

// ---------------------------------------------------------------------------------------------------------------------
// When: this computer's time zone, countdowns.

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn parse(iso: Option<&str>) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(iso?, &time::format_description::well_known::Rfc3339).ok()
}

fn unix(iso: Option<&str>) -> Option<i64> {
    parse(iso).map(|t| t.unix_timestamp())
}

/// This computer's offset from UTC (asked again once a minute: summer time).
fn offset() -> UtcOffset {
    use std::sync::Mutex;
    static KEPT: Mutex<Option<(i64, UtcOffset)>> = Mutex::new(None);
    let t = now();
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    match *kept {
        Some((at, o)) if t - at < 60 => o,
        _ => {
            let o = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
            *kept = Some((t, o));
            o
        }
    }
}

fn local(t: OffsetDateTime) -> OffsetDateTime {
    t.to_offset(offset())
}

const MONTHS: [&str; 12] = [mark("Jan"), mark("Feb"), mark("Mar"), mark("Apr"), mark("May"), mark("Jun"), mark("Jul"), mark("Aug"), mark("Sep"), mark("Oct"), mark("Nov"), mark("Dec")];
const DAYS: [&str; 7] = [mark("Mon"), mark("Tue"), mark("Wed"), mark("Thu"), mark("Fri"), mark("Sat"), mark("Sun")];
const DAYS_LONG: [&str; 7] = [mark("Monday"), mark("Tuesday"), mark("Wednesday"), mark("Thursday"), mark("Friday"), mark("Saturday"), mark("Sunday")];

fn clock(t: OffsetDateTime) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// "Sat 1 Nov" (and its year when it is not this one).
fn date_short(t: OffsetDateTime, today: OffsetDateTime) -> String {
    let date = tr!("{weekday} {day} {month}", weekday = tr(DAYS[t.weekday().number_days_from_monday() as usize]), day = t.day(), month = tr(MONTHS[t.month() as usize - 1]));
    if t.year() != today.year() {
        tr!("{date} {year}", date = date, year = t.year())
    } else {
        date
    }
}

/// The day of `t` as people say it from `today`: Today, Tomorrow, Yesterday, the weekday within a week, else the date.
fn day_name(t: OffsetDateTime, today: OffsetDateTime) -> String {
    let days = (t.date() - today.date()).whole_days();
    match days {
        0 => tr("Today").into(),
        1 => tr("Tomorrow").into(),
        -1 => tr("Yesterday").into(),
        2..=6 => tr(DAYS_LONG[t.weekday().number_days_from_monday() as usize]).into(),
        _ => date_short(t, today),
    }
}

/// When an event takes place, in this computer's time zone: "Today · 21:00 – 23:00", "Sat 1 Nov, 22:00 – Sun 2 Nov,
/// 01:00", "Time to be announced".
pub fn when(starts: Option<&str>, ends: Option<&str>) -> String {
    when_at(starts, ends, now())
}

fn when_at(starts: Option<&str>, ends: Option<&str>, now: i64) -> String {
    let Some(s) = parse(starts).map(local) else { return tr("Time to be announced").into() };
    let today = local(OffsetDateTime::from_unix_timestamp(now).unwrap_or(OffsetDateTime::UNIX_EPOCH));
    match parse(ends).map(local).filter(|e| *e > s) {
        Some(e) if e.date() == s.date() => format!("{} · {} – {}", day_name(s, today), clock(s), clock(e)),
        // Over midnight, ending early in the morning: still that evening's event.
        Some(e) if (e.date() - s.date()).whole_days() == 1 && e.hour() < 8 => format!("{} · {} – {}", day_name(s, today), clock(s), clock(e)),
        Some(e) => format!("{}, {} – {}, {}", day_name(s, today), clock(s), day_name(e, today), clock(e)),
        None => format!("{} · {}", day_name(s, today), clock(s)),
    }
}

/// This computer's time zone as its offset ("UTC+1", "UTC−3:30").
pub fn zone() -> String {
    let o = offset();
    let (h, m) = (o.whole_hours(), o.minutes_past_hour().abs());
    match (h, m) {
        (0, 0) => "UTC".into(),
        (h, 0) => format!("UTC{}{}", if h < 0 { "−" } else { "+" }, h.abs()),
        (h, m) => format!("UTC{}{}:{m:02}", if h < 0 || o.is_negative() { "−" } else { "+" }, h.abs()),
    }
}

/// Where an event stands now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Live,
    /// Starts in this many seconds.
    Soon(i64),
    Ended,
    Cancelled,
    Planning,
    Tba,
}

/// The event's phase at `now` (the website's rule): cancelled and ended first, then under way (started, or a step's
/// instance open), else a countdown.
pub fn phase(status: Option<&str>, starts: Option<&str>, ends: Option<&str>, step_live: bool, now: i64) -> Phase {
    match status {
        Some("cancelled" | "cancelling") => return Phase::Cancelled,
        Some("ended") => return Phase::Ended,
        Some("planning") => return Phase::Planning,
        _ => {}
    }
    if unix(ends).is_some_and(|e| e <= now) {
        return Phase::Ended;
    }
    let start = unix(starts);
    if step_live || start.is_some_and(|s| s <= now) {
        return Phase::Live;
    }
    start.map_or(Phase::Tba, |s| Phase::Soon(s - now))
}

fn countdown(secs: i64) -> String {
    let minutes = ((secs as f64) / 60.0).round().max(1.0) as i64;
    let (days, hours) = (minutes / 1440, minutes % 1440 / 60);
    if days > 0 {
        tr!("{days} d {hours} h", days = days, hours = hours)
    } else if hours > 0 {
        tr!("{hours} h {minutes} min", hours = hours, minutes = minutes % 60)
    } else {
        tr!("{minutes} min", minutes = minutes)
    }
}

pub fn phase_label(p: Phase) -> String {
    match p {
        Phase::Live => tr("Happening now").into(),
        Phase::Soon(s) => tr!("Starts in {time}", time = countdown(s)),
        Phase::Ended => tr("Ended").into(),
        Phase::Cancelled => tr("Cancelled").into(),
        Phase::Planning => tr("Voting open").into(),
        Phase::Tba => tr("Time to be announced").into(),
    }
}

fn card_phase(c: &Card, now: i64) -> Phase {
    phase(c.status.as_deref(), c.starts_at.as_deref(), c.ends_at.as_deref(), false, now)
}

fn category_label(id: &str) -> String {
    match id {
        "film_media" => tr("Film & media").into(),
        "roleplaying" => tr("Roleplay").into(),
        "education" => tr("Education").into(),
        "avatars" => tr("Avatars").into(),
        "hangout" => tr("Hangout").into(),
        "music" => tr("Music").into(),
        "exploration" => tr("Exploration").into(),
        other => {
            let mut s = other.replace('_', " ");
            if let Some(first) = s.get(..1) {
                s.replace_range(..1, &first.to_uppercase());
            }
            s
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Covers.

/// How large a cover is wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverSize {
    Card,
    Large,
    /// A few pixels: drawn large and soft as the event's light.
    Glow,
}

/// Where events' covers come from: Orbiters (`Picture::EventCover`), the main world's picture for events without one,
/// or, for screenshots, this computer's VRChat pictures.
#[derive(Clone, Debug, Default)]
pub struct Covers {
    pub development: bool,
    /// Screenshots: stand-in covers, by the event's place in `order`.
    pub fixture: Vec<Picture>,
    pub order: Vec<String>,
}

impl Covers {
    pub fn get(&self, id: &str, has_banner: bool, world_image: Option<&str>, size: CoverSize) -> Option<Picture> {
        let picture = if !self.fixture.is_empty() {
            if !has_banner {
                return None;
            }
            let i = self.order.iter().position(|o| o == id).unwrap_or(0);
            self.fixture[i % self.fixture.len()].clone()
        } else if has_banner {
            Picture::EventCover { development: self.development, id: id.to_string() }
        } else {
            let url = world_image.filter(|u| u.starts_with("https://"))?;
            Picture::Remote { world_id: format!("event-world-{id}"), url: url.to_string(), large: size == CoverSize::Large }
        };
        Some(if size == CoverSize::Glow { Picture::Glow(Box::new(picture)) } else { picture })
    }

    pub fn card(&self, c: &Card, size: CoverSize) -> Option<Picture> {
        self.get(&c.id, c.has_banner, c.world.as_ref().and_then(|w| w.image_url.as_deref()), size)
    }

    /// This computer's VRChat pictures and captures as stand-in covers (screenshots).
    pub fn stand_ins(sessions_dir: &std::path::Path) -> Vec<Picture> {
        let mut found: Vec<Picture> = Vec::new();
        for w in crate::worlds::load(sessions_dir) {
            found.extend(w.shots.iter().take(2).map(|s| Picture::Shot { path: s.path.clone(), size: s.size, width: 1280 }));
            if found.len() >= 12 {
                return found;
            }
        }
        let sessions = crate::controller::list_sessions(sessions_dir);
        found.extend(sessions.iter().filter(|s| s.has_thumbnail).take(12).map(|s| Picture::Session(s.dir.clone())));
        found
    }
}

/// A cover filling `rect` (corners `radii`): the picture, else the event's own colours with a calendar.
pub fn paint_cover(p: &Painter, rect: Rect, radii: [f32; 4], tex: Option<&TextureHandle>, seed: &str, zoom: f32, alpha: f32) {
    paint_cover_at(p, rect, radii, tex, seed, zoom, alpha, 0.5);
}

/// [`paint_cover`] whose calendar, without a picture, sits at `icon_x` of its width (clear of words over the cover).
#[allow(clippy::too_many_arguments)]
pub fn paint_cover_at(p: &Painter, rect: Rect, radii: [f32; 4], tex: Option<&TextureHandle>, seed: &str, zoom: f32, alpha: f32, icon_x: f32) {
    match tex {
        Some(t) => feedback::cover(p, rect, radii, t, zoom, alpha),
        None => {
            let pts = paint::squircle_points_corners(rect, radii.map(theme::squircle));
            let a = widgets::name_color(seed);
            let g = Gradient::new(
                rect.left_top(),
                rect.right_bottom(),
                &[
                    (0.0, theme::mix(theme::INK_800, a, 0.42).gamma_multiply(alpha)),
                    (1.0, theme::mix(theme::INK_850, theme::VIOLET, 0.26).gamma_multiply(alpha)),
                ],
            );
            paint::fill_gradient(p, &pts, &g);
            let s = (rect.height() * 0.22).clamp(18.0, 44.0);
            widgets::icon(p, pos2(rect.left() + rect.width() * icon_x, rect.center().y), icons::CALENDAR_DOTS, s, theme::white(0.5 * alpha));
        }
    }
}

/// The event's light: its cover a few pixels wide (so already soft), spread around `rect` (corners `radius`) and fading
/// out over `spread` (a ring of the picture's own colours, as the website's blurred cover behind an event).
pub fn paint_glow(p: &Painter, rect: Rect, radius: f32, spread: egui::Vec2, tex: Option<&TextureHandle>, strength: f32) {
    let Some(t) = tex else { return };
    if strength <= 0.001 {
        return;
    }
    let outer = rect.expand2(spread);
    let uv = |q: Pos2| pos2((q.x - outer.left()) / outer.width(), (q.y - outer.top()) / outer.height());
    // Three rounded outlines with the same points: the shape, halfway out, the outer edge; the light fades along them.
    let grow = spread.x.max(spread.y);
    let rings = [(0.0, 0.62), (0.4, 0.22), (1.0, 0.0)].map(|(k, a)| (glow_outline(rect.expand2(spread * k), radius + grow * k, 10), a * strength));
    let mut mesh = egui::Mesh::with_texture(t.id());
    for (points, a) in &rings {
        for q in points {
            mesh.vertices.push(egui::epaint::Vertex { pos: *q, uv: uv(*q), color: Color32::WHITE.gamma_multiply(*a) });
        }
    }
    let n = rings[0].0.len() as u32;
    for ring in 0..2u32 {
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b, c, d) = (ring * n + i, ring * n + j, (ring + 1) * n + j, (ring + 1) * n + i);
            mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// A rounded rect's outline, `per_corner` + 1 points a corner (the same count whatever its size, so outlines of
/// several sizes pair up).
fn glow_outline(rect: Rect, radius: f32, per_corner: usize) -> Vec<Pos2> {
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -std::f32::consts::FRAC_PI_2),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), std::f32::consts::FRAC_PI_2),
        (pos2(rect.left() + r, rect.top() + r), std::f32::consts::PI),
    ];
    let mut out = Vec::with_capacity(4 * (per_corner + 1));
    for (c, start) in corners {
        for k in 0..=per_corner {
            let a = start + std::f32::consts::FRAC_PI_2 * k as f32 / per_corner as f32;
            out.push(pos2(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    out
}

/// A status over a cover: a countdown with a green dot, Happening now in red with a pulse, the rest quiet.
pub fn status_pill(p: &Painter, ui: &Ui, at: Pos2, ph: Phase, small: bool) -> Rect {
    let label = phase_label(ph);
    let size = if small { 11.0 } else { 12.0 };
    let t = Txt::new(&label, size).weight(weight::SEMIBOLD).color(Color32::WHITE);
    let tw = widgets::text_size(ui, &t).x;
    let h = if small { 24.0 } else { 28.0 };
    let dot = !matches!(ph, Phase::Ended | Phase::Cancelled | Phase::Tba);
    let w = tw + if dot { 34.0 } else { 22.0 };
    let r = Rect::from_min_size(at, vec2(w, h));
    let bg = match ph {
        Phase::Live => theme::alpha(theme::REC, 0.9),
        Phase::Cancelled => theme::alpha(theme::DANGER, 0.7),
        _ => theme::black(0.55),
    };
    p.rect_filled(r, theme::radius(h * 0.5), bg);
    let mut x = r.left() + 11.0;
    if dot {
        let c = pos2(x + 3.5, r.center().y);
        let color = if ph == Phase::Live { Color32::WHITE } else { theme::ACCENT };
        if ph == Phase::Live && anim::is_live(ui.ctx()) {
            let k = (anim::clock(ui.ctx(), 30.0) % 1.4 / 1.4) as f32;
            let e = ease::out_cubic(k);
            p.circle_filled(c, 3.5 * (1.0 + 1.4 * e), theme::white(0.6 * (1.0 - e)));
        }
        p.circle_filled(c, 3.5, color);
        x += 13.0;
    }
    widgets::text(p, pos2(x, r.center().y), Align2::LEFT_CENTER, &t);
    r
}

/// The homepage's date tile: the month over the day, in the accent's tint.
fn date_tile(p: &Painter, rect: Rect, starts: Option<&str>) {
    p.rect_filled(rect, theme::radius(13.0), theme::alpha(theme::ACCENT, 0.13));
    let Some(t) = parse(starts).map(local) else {
        widgets::icon(p, rect.center(), icons::CALENDAR_DOTS, 18.0, theme::ACCENT);
        return;
    };
    widgets::text(
        p,
        pos2(rect.center().x, rect.top() + 14.0),
        Align2::CENTER_CENTER,
        &Txt::new(tr(MONTHS[t.month() as usize - 1]).to_uppercase(), 9.5).weight(weight::BOLD).tracking(0.06).color(theme::ACCENT),
    );
    widgets::text(
        p,
        pos2(rect.center().x, rect.top() + 33.0),
        Align2::CENTER_CENTER,
        &Txt::new(t.day().to_string(), 21.0).weight(weight::BOLD).color(theme::ACCENT),
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Invite me.

/// Where a step's Invite me stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Ask {
    /// Not known yet (the member's invites are being read).
    #[default]
    Unknown,
    /// Nothing asked, or taken back.
    Idle,
    /// Orbiters is answering: what is under way.
    Busy(&'static str),
    /// On the step's invite list: the community's account invites them when it starts.
    Listed {
        friend: Option<Friend>,
    },
    /// Sent, or on its way.
    Sent(&'static str),
    /// The last invite could not go, and why (`not_friends`, `not_member`, …).
    Failed(Option<String>),
    /// VRChat is not linked on Orbiters.
    Link,
    /// Not in the group, or without the role the event asks for.
    Group,
    /// Orbiters' own account sent an invite into the open instance.
    Invited,
    /// Orbiters' own account must be their friend first.
    Befriend {
        name: String,
        link: String,
    },
    /// Not signed in to Orbiters.
    SignIn,
    Error(String),
}

impl Ask {
    /// What the server's status of a step's invite means here.
    fn of_status(status: &str, reason: Option<&String>) -> Self {
        match status {
            "pending" => Ask::Listed { friend: None },
            "sent" => Ask::Sent(mark("Invite sent")),
            "sending" | "uncertain" => Ask::Sent(mark("Invite on its way")),
            "failed" => Ask::Failed(reason.cloned()),
            _ => Ask::Idle,
        }
    }

    fn of_signup(s: Signup) -> Self {
        match s.status.as_str() {
            "pending" => Ask::Listed { friend: s.friend },
            other => Ask::of_status(other, None),
        }
    }

    fn of_refusal(r: &Refused) -> Self {
        match (r.code.as_deref(), r.status) {
            (Some("VRCHAT_LINK_REQUIRED"), _) => Ask::Link,
            (Some("VRCHAT_GROUP_REQUIRED"), _) => Ask::Group,
            (_, 401) => Ask::SignIn,
            _ => Ask::Error(r.message.clone()),
        }
    }

    fn of_error(e: &anyhow::Error) -> Self {
        match Refused::of(e) {
            Some(r) => Ask::of_refusal(r),
            None => Ask::Error(tr!("Orbiters could not be reached: {e}", e = e)),
        }
    }

    fn busy(&self) -> bool {
        matches!(self, Ask::Busy(_))
    }

    /// A step the member is on the list for, or was invited to.
    pub fn signed_up(&self) -> bool {
        matches!(self, Ask::Listed { .. } | Ask::Sent(_) | Ask::Invited)
    }
}

/// What was pressed on an Invite me control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Ask,
    Cancel,
}

/// How an Invite me control draws its face in a rect (and what was pressed on it).
type Face = Box<dyn FnOnce(&mut Ui, Rect) -> Option<Press>>;

/// The Invite me control at `at` (its left edge, vertically centred): a button while nothing is asked, a spinner pill
/// while Orbiters answers, a chip once on the list or sent (with Cancel beside "Will invite you"). It morphs between
/// them: the width on a spring, the new face fading and growing in. Returns its width and what was pressed.
fn invite_control(ui: &mut Ui, id: Id, at: Pos2, ask: &Ask, primary: bool, small: bool, enabled: bool) -> (f32, Option<Press>) {
    let ctx = ui.ctx().clone();
    let h = if small { 32.0 } else { 40.0 };
    let size = if small { 12.5 } else { 13.5 };
    let face = std::mem::discriminant(ask);
    let key = format!("{face:?}");
    let k = anim::spring_from(&ctx, id.with(("face", &key)), [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
    let mut pressed = None;
    let (target_w, draw): (f32, Face) = match ask {
        Ask::Busy(label) => {
            let t = Txt::new(tr(label), size).weight(weight::MEDIUM).color(theme::FG_2);
            let w = widgets::text_size(ui, &t).x + 50.0;
            (
                w,
                Box::new(move |ui: &mut Ui, r: Rect| {
                    let p = ui.painter();
                    p.rect_filled(r, theme::radius(r.height() * 0.5), theme::control());
                    feedback::spinner(p, pos2(r.left() + 20.0, r.center().y), 6.0, 2.0, theme::ACCENT);
                    widgets::text_truncated(p, pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, &t, r.width() - 44.0);
                    None
                }),
            )
        }
        Ask::Listed { .. } | Ask::Sent(_) | Ask::Invited => {
            let (glyph, label, cancel) = match ask {
                Ask::Listed { .. } => (icons::CHECK, tr("Will invite you"), true),
                Ask::Sent(l) => (icons::CHECK_CIRCLE, tr(l), false),
                _ => (icons::PAPER_PLANE_TILT, tr("Invite sent"), false),
            };
            let t = Txt::new(label, size).weight(weight::SEMIBOLD).color(theme::ACCENT);
            let chip_w = widgets::text_size(ui, &t).x + 46.0;
            let c = Txt::new(tr("Cancel"), size).weight(weight::MEDIUM).color(theme::FG_3);
            let cancel_w = if cancel { widgets::text_size(ui, &c).x + 24.0 } else { 0.0 };
            let id2 = id.with("cancel");
            (
                chip_w + cancel_w,
                Box::new(move |ui: &mut Ui, r: Rect| {
                    let chip = Rect::from_min_size(r.min, vec2((r.width() - cancel_w).max(h), r.height()));
                    let p = ui.painter().clone();
                    p.rect_filled(chip, theme::radius(r.height() * 0.5), theme::alpha(theme::ACCENT, 0.14));
                    widgets::icon(&p, pos2(chip.left() + 20.0, chip.center().y), glyph, 15.0, theme::ACCENT);
                    widgets::text_truncated(&p, pos2(chip.left() + 34.0, chip.center().y), Align2::LEFT_CENTER, &t, chip.width() - 42.0);
                    if cancel {
                        let cr = Rect::from_min_max(pos2(chip.right(), r.top()), r.max);
                        let resp =
                            ui.interact(cr, id2, Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Leave this step's invite list"));
                        let hover = anim::toggle(ui.ctx(), id2.with("hover"), resp.hovered(), Spring::SNAPPY);
                        widgets::text(
                            &p,
                            pos2(cr.center().x + 2.0, cr.center().y),
                            Align2::CENTER_CENTER,
                            &Txt::new(tr("Cancel"), size).weight(weight::MEDIUM).color(theme::mix(theme::FG_3, theme::FG, hover)),
                        );
                        if sharing::pressed(resp) {
                            return Some(Press::Cancel);
                        }
                    }
                    None
                }),
            )
        }
        _ => {
            let b = Button::new(tr("Invite me"))
                .icon(icons::PAPER_PLANE_TILT)
                .variant(if primary { Variant::Primary } else { Variant::Secondary })
                .size(if small { Size::Sm } else { Size::Md })
                .enabled(enabled)
                .id_salt(id.with("button"));
            let w = b.desired_width(ui);
            (
                w,
                Box::new(move |ui: &mut Ui, r: Rect| {
                    let b = Button::new(tr("Invite me"))
                        .icon(icons::PAPER_PLANE_TILT)
                        .variant(if primary { Variant::Primary } else { Variant::Secondary })
                        .size(if small { Size::Sm } else { Size::Md })
                        .enabled(enabled)
                        .width(r.width())
                        .id_salt(id.with("button"));
                    sharing::pressed(b.show_at(ui, r)).then_some(Press::Ask)
                }),
            )
        }
    };
    let w = anim::value(&ctx, id.with("w"), target_w, Spring::MORPH).max(h);
    let rect = Rect::from_min_size(pos2(at.x, at.y - h * 0.5), vec2(w, h));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(rect.expand(4.0)).id_salt(id.with("control")));
    cu.multiply_opacity(k);
    let s = 0.94 + 0.06 * k;
    let shown = Rect::from_center_size(rect.center(), rect.size() * vec2(1.0, s));
    if let Some(p) = draw(&mut cu, shown) {
        pressed = Some(p);
    }
    (w, pressed)
}

/// The line under an Invite me saying what happens next or what is missing, and the buttons that help.
fn ask_note(ask: &Ask, page: Option<&Overview>) -> Option<(Tone, String, Vec<NoteAction>)> {
    let account = page.and_then(|o| o.community.as_ref()).and_then(|c| c.account_id.clone());
    let group = page.and_then(|o| o.community.as_ref());
    Some(match ask {
        Ask::Listed { friend } => match friend {
            Some(Friend { is_friend: Some(false), accepting: true, .. }) => {
                (Tone::Info, tr("Orbiters is accepting your friend request: the invite follows.").into(), vec![])
            }
            Some(Friend { is_friend: Some(false), account: a, .. }) => {
                let name = a.as_ref().and_then(|a| a.name.clone());
                let id = a.as_ref().and_then(|a| a.vrchat_id.clone()).or(account);
                (
                    Tone::Warn,
                    match name {
                        Some(n) => tr!("Add {name} as a friend on VRChat to receive the invite.", name = n),
                        None => tr("Add the community's VRChat account as a friend to receive the invite.").into(),
                    },
                    id.map(|id| vec![NoteAction::Url(mark("Open the account"), format!("https://vrchat.com/home/user/{id}"))]).unwrap_or_default(),
                )
            }
            _ => (Tone::Ok, tr("You're on the invite list: the invite comes in VRChat when this step starts and its instance is ready.").into(), vec![]),
        },
        Ask::Sent(_) => (Tone::Ok, tr("Your invite was sent: open it from VRChat's notifications.").into(), vec![]),
        Ask::Invited => (Tone::Ok, tr("Orbiters sent you an invite into the instance: open it from VRChat's notifications.").into(), vec![]),
        Ask::Failed(reason) => {
            let (text, profile) = match reason.as_deref() {
                Some("not_friends") => (tr("Not sent: add the community's VRChat account as a friend, then ask again."), true),
                Some("not_member") => (tr("Not sent: join the group and get its role, then ask again."), false),
                _ => (tr("Your invite could not be sent. Ask again to retry."), false),
            };
            let actions = if profile {
                account.map(|id| vec![NoteAction::Url(mark("Open the account"), format!("https://vrchat.com/home/user/{id}"))]).unwrap_or_default()
            } else {
                vec![]
            };
            (Tone::Warn, text.into(), actions)
        }
        Ask::Link => (Tone::Warn, tr("Connect your VRChat account on Orbiters to get the invite.").into(), vec![NoteAction::Connect, NoteAction::Check]),
        Ask::Group => {
            let name = group.map(|g| g.name.clone()).filter(|n| !n.is_empty());
            let mut actions = vec![];
            if let Some(id) = group.and_then(|g| g.id.clone()) {
                actions.push(NoteAction::Url(mark("Open the group"), format!("https://vrchat.com/home/group/{id}")));
            }
            actions.push(NoteAction::Check);
            let said = match name {
                Some(name) => tr!("Join {name} on VRChat and get the role the event asks for, then check again.", name = name),
                None => tr("Join the group on VRChat and get the role the event asks for, then check again.").to_string(),
            };
            (Tone::Warn, said, actions)
        }
        Ask::Befriend { name, link } => (
            Tone::Warn,
            tr!("VRChat lets Orbiters invite only its friends: add {name} as a friend on VRChat, then try again.", name = name),
            vec![NoteAction::Url(mark("Open the account"), link.clone())],
        ),
        Ask::SignIn => (Tone::Info, tr("Sign in to Orbiters to get invites to events.").into(), vec![NoteAction::SignIn]),
        Ask::Error(m) => (Tone::Danger, m.clone(), vec![]),
        Ask::Unknown | Ask::Idle | Ask::Busy(_) => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
enum NoteAction {
    Url(&'static str, String),
    /// Opens the website's invite page, which links VRChat.
    Connect,
    /// Reads VRChat again, then signs up.
    Check,
    SignIn,
}

// ---------------------------------------------------------------------------------------------------------------------
// The event's window.

/// What an action of the window brought back.
enum Outcome {
    Signed(anyhow::Result<Signup>),
    Withdrawn(anyhow::Result<Signup>),
    /// Checked again: not allowed yet, or signed up.
    Checked(Ask),
    N4n0(InviteAnswer),
}

/// Every opening gets springs of its own.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// An event's window: it grows out of the card's cover into the middle of the page (the cover becomes its hero) and
/// shrinks back into it. What the card knew shows at once; the event's page, the member's invites and its pictures
/// arrive behind it.
pub struct Window {
    pub id: String,
    card: Card,
    morph: Morph,
    server: Server,
    page: Option<Overview>,
    page_job: Job<anyhow::Result<Overview>>,
    error: Option<Refused>,
    invites_job: Job<anyhow::Result<Invites>>,
    asks: HashMap<String, Ask>,
    action: Job<(String, Outcome)>,
    /// The step whose note shows under the actions (the one last asked for).
    focus: Option<String>,
    /// The event's gallery, when known (opened from it), and its first pictures.
    pub gallery: Option<u64>,
    gallery_job: Job<anyhow::Result<(Option<u64>, Vec<community::Picture>)>>,
    pictures: Vec<community::Picture>,
    pictures_asked: bool,
    /// The body's height as last laid out.
    body_h: f32,
    /// Screenshots: made-up answers, nothing asked of Orbiters.
    fixture: bool,
    /// A picture of the strip was pressed: its index and rect.
    pub picture_pressed: Option<(usize, Rect)>,
    /// Shows Pictures from this event (off when opened from that very gallery).
    pub show_pictures: bool,
    /// The window's progress as last drawn (0 the card, 1 open).
    pub progress: f32,
    /// Another window is over it (a picture of the strip): Escape and presses are that window's.
    pub covered: bool,
    /// Screenshots: this computer's VRChat pictures stand in for the gallery's.
    stand_ins: Vec<Picture>,
}

/// Space between the window and the page's edges.
const GUTTER: f32 = 28.0;
const PAD: f32 = 28.0;
const MAX_W: f32 = 880.0;

impl Window {
    /// The window of `card`, growing out of `origin` (its cover, corners `radius`).
    pub fn open(ctx: &egui::Context, card: Card, origin: Rect, radius: f32, server: Server) -> Self {
        let serial = OPENED.fetch_add(1, Ordering::Relaxed);
        let id = card.id.clone();
        let mut w = Self {
            id: id.clone(),
            card,
            morph: Morph::new(Id::unique(("event-window", serial)), origin, radius),
            server,
            page: None,
            page_job: Job::default(),
            error: None,
            invites_job: Job::default(),
            asks: HashMap::new(),
            action: Job::default(),
            focus: None,
            gallery: None,
            gallery_job: Job::default(),
            pictures: Vec::new(),
            pictures_asked: false,
            body_h: 320.0,
            fixture: false,
            picture_pressed: None,
            show_pictures: true,
            progress: 0.0,
            covered: false,
            stand_ins: Vec::new(),
        };
        w.load(ctx);
        w
    }

    fn load(&mut self, ctx: &egui::Context) {
        let (server, id) = (self.server, self.id.clone());
        self.error = None;
        self.page_job = Job::spawn(ctx, move || {
            let token = orbiters_core::account::read(server).map(|a| a.token);
            Api::new(server).event(token.as_deref(), &id)
        });
        if let Some(account) = orbiters_core::account::read(server) {
            let id = self.id.clone();
            self.invites_job = Job::spawn(ctx, move || Api::new(server).my_invites(&account.token, &id));
        }
    }

    /// Screenshots: the event's page from the fixture (its times moved as the card's), the member's invites as `asks`.
    pub fn stage(&mut self, page: Overview, asks: &[(&str, Ask)], pictures: Vec<community::Picture>) {
        self.fixture = true;
        self.page_job = Job::default();
        self.invites_job = Job::default();
        self.page = Some(page);
        for (step, ask) in asks {
            self.asks.insert((*step).to_string(), ask.clone());
            self.focus = Some((*step).to_string());
        }
        self.pictures = pictures;
        self.pictures_asked = true;
    }

    /// Screenshots: the event's page still on its way (its outline shows).
    pub fn stage_loading(&mut self) {
        self.fixture = true;
        self.page = None;
        self.page_job = Job::default();
        self.invites_job = Job::default();
    }

    pub fn close(&mut self) {
        self.morph.close();
    }

    /// Follows its card (it moved: a scroll, the grid reflowed).
    pub fn set_origin(&mut self, origin: Rect) {
        self.morph.set_origin(origin);
    }

    pub fn loading(&self) -> bool {
        self.page_job.running() || self.invites_job.running() || self.gallery_job.running()
    }

    /// How much of its card shows while the window is out: none, then all of it as it lands back.
    pub fn card_shown(&self) -> f32 {
        if self.morph.is_closing() {
            morph::source_alpha(self.progress)
        } else {
            0.0
        }
    }

    pub fn ask_of(&self, step: &str) -> Ask {
        self.asks.get(step).cloned().unwrap_or(if self.invites_job.running() { Ask::Unknown } else { Ask::Idle })
    }

    /// Whether the member signed up for any of the event's steps.
    pub fn signed_up(&self) -> bool {
        self.asks.values().any(Ask::signed_up)
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(r) = self.page_job.take() {
            match r {
                Ok(p) => {
                    self.page = Some(p);
                    self.error = None;
                }
                Err(e) => {
                    let unreachable = tr!("Orbiters could not be reached: {e}", e = e);
                    self.error = Some(Refused::of(&e).cloned().unwrap_or(Refused { status: 0, code: None, message: unreachable }))
                }
            }
        }
        if let Some(Ok(invites)) = self.invites_job.take() {
            for (step, status) in &invites.items {
                if !self.asks.get(step).is_some_and(Ask::busy) {
                    self.asks.insert(step.clone(), Ask::of_status(status, invites.reasons.get(step)));
                }
            }
        }
        if let Some((step, outcome)) = self.action.take() {
            let ask = match outcome {
                Outcome::Signed(Ok(s)) => Ask::of_signup(s),
                Outcome::Signed(Err(e)) => Ask::of_error(&e),
                Outcome::Withdrawn(Ok(s)) => match s.status.as_str() {
                    "sent" | "sending" | "uncertain" => Ask::of_status(&s.status, None),
                    _ => Ask::Idle,
                },
                Outcome::Withdrawn(Err(e)) => Ask::of_error(&e),
                Outcome::Checked(a) => a,
                Outcome::N4n0(InviteAnswer::Sent) => Ask::Invited,
                Outcome::N4n0(InviteAnswer::NeedsFriend { account, .. }) => Ask::Befriend { name: account.name, link: account.link },
                Outcome::N4n0(InviteAnswer::Failed { message }) => Ask::Error(message),
            };
            self.asks.insert(step, ask);
        }
        // The event's pictures: its gallery among the member's, then the gallery's newest.
        if !self.fixture && self.show_pictures && !self.pictures_asked && self.page.is_some() {
            if let Some(account) = orbiters_core::account::read(self.server) {
                self.pictures_asked = true;
                let (server, id, known) = (self.server, self.id.clone(), self.gallery);
                self.gallery_job = Job::spawn(ctx, move || {
                    let api = Api::new(server);
                    let gallery = match known {
                        Some(g) => Some(g),
                        None => api.galleries(&account.token)?.all().find(|g| g.event.as_ref().is_some_and(|e| e.id == id)).map(|g| g.id),
                    };
                    let pictures = match gallery {
                        Some(g) => api.gallery_pictures(&account.token, Some(g), None, 12)?.items,
                        None => Vec::new(),
                    };
                    Ok((gallery, pictures))
                });
            }
        }
        if let Some(Ok((gallery, pictures))) = self.gallery_job.take() {
            self.gallery = gallery.or(self.gallery);
            self.pictures = pictures;
        }
    }

    fn act(&mut self, ctx: &egui::Context, cx: &mut Cx, act: Act) {
        let server = self.server;
        let token = || orbiters_core::account::read(server).map(|a| a.token);
        match act {
            Act::Url(url) => {
                let r = crate::controller::open_url(&url);
                cx.run(ctx, r);
            }
            Act::SignIn => cx.ctl.sign_in(),
            Act::Retry => self.load(ctx),
            Act::Ask(step) | Act::Check(step) | Act::Withdraw(step) | Act::N4n0(step, _) if self.fixture => {
                self.focus = Some(step);
            }
            Act::Ask(step) => {
                self.focus = Some(step.clone());
                let Some(token) = token() else {
                    self.asks.insert(step, Ask::SignIn);
                    return;
                };
                self.asks.insert(step.clone(), Ask::Busy(mark("Signing you up…")));
                let id = self.id.clone();
                self.action = Job::spawn(ctx, move || {
                    let r = Api::new(server).invite_me(&token, &id, &step);
                    (step, Outcome::Signed(r))
                });
            }
            Act::Check(step) => {
                self.focus = Some(step.clone());
                let Some(token) = token() else {
                    self.asks.insert(step, Ask::SignIn);
                    return;
                };
                self.asks.insert(step.clone(), Ask::Busy(mark("Checking with VRChat…")));
                let id = self.id.clone();
                self.action = Job::spawn(ctx, move || {
                    let api = Api::new(server);
                    let ask = match api.invite_eligibility(&token, &id, &step, true) {
                        Ok(e) if !e.linked => Ask::Link,
                        Ok(e) if !e.allowed => Ask::Group,
                        Ok(_) => match api.invite_me(&token, &id, &step) {
                            Ok(s) => Ask::of_signup(s),
                            Err(e) => Ask::of_error(&e),
                        },
                        Err(e) => Ask::of_error(&e),
                    };
                    (step, Outcome::Checked(ask))
                });
            }
            Act::Withdraw(step) => {
                self.focus = Some(step.clone());
                let Some(token) = token() else { return };
                self.asks.insert(step.clone(), Ask::Busy(mark("Cancelling…")));
                let id = self.id.clone();
                self.action = Job::spawn(ctx, move || {
                    let r = Api::new(server).withdraw_invite(&token, &id, &step);
                    (step, Outcome::Withdrawn(r))
                });
            }
            Act::N4n0(step, location) => {
                self.focus = Some(step.clone());
                let Some(token) = token() else {
                    self.asks.insert(step, Ask::SignIn);
                    return;
                };
                self.asks.insert(step.clone(), Ask::Busy(mark("Asking VRChat…")));
                self.action = Job::spawn(ctx, move || {
                    let answer = orbiters_n4n0_core::invites::send(&Api::new(server), &token, &location);
                    (step, Outcome::N4n0(answer))
                });
            }
        }
    }

    /// Draws the window over the page (`page`: the page's rect); true once it closed back into its card.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx, pictures: &mut Pictures, covers: &Covers) -> bool {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let page = ui.max_rect();
        let area = Rect::from_min_max(pos2(page.left() + GUTTER, page.top() + cx.top + 14.0), pos2(page.right() - GUTTER, page.bottom() - GUTTER));
        let width = area.width().min(MAX_W);
        let hero_h = (width * 7.0 / 16.0).clamp(200.0, (area.height() * 0.46).max(200.0)).min(380.0);
        let height = (hero_h + self.body_h).min(area.height());
        let target = Rect::from_center_size(area.center(), vec2(width, height));
        let f = self.morph.frame(&ctx, target);
        self.progress = f.t;
        let wid = self.morph.id();
        let closing = self.morph.is_closing();
        let hero_target = Rect::from_min_size(target.min, vec2(width, hero_h));
        let hero = self.morph.part(&ctx, "hero", hero_target, &f);
        let now = now();
        let large = covers.card(&self.card, CoverSize::Large).and_then(|p| pictures.get(&ctx, &p));
        let small = covers.card(&self.card, CoverSize::Card).and_then(|p| pictures.get(&ctx, &p));
        let glow = covers.card(&self.card, CoverSize::Glow).and_then(|p| pictures.get(&ctx, &p));
        let window = ctx.content_rect();
        let catcher = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
        let mut acts: Vec<Act> = Vec::new();
        let mut close = false;
        let mut reopen = false;
        let width_now = f.rect.width();
        egui::Area::new(wid.with("area")).order(Order::Foreground).fixed_pos(catcher.min).fade_in(false).show(&ctx, |ui| {
            ui.set_min_size(catcher.size());
            let backdrop = ui.interact(catcher, wid.with("backdrop"), Sense::click());
            viewer::paint_backdrop(&ui.painter().with_clip_rect(window), window, &f);
            if widgets::pressed(&backdrop) && !f.rect.contains(backdrop.interact_pointer_pos().unwrap_or_default()) && !self.covered {
                close = true;
            }
            if widgets::pressed(&ui.interact(f.rect, wid.with("surface"), Sense::click())) && closing {
                reopen = true;
            }
            // The event's light around the window, its cover's colours on the dimmed page.
            paint_glow(&ui.painter().with_clip_rect(window), f.rect, f.radius, vec2(90.0, 80.0), glow.as_ref(), 0.85 * f.content * f.shape);
            let p = ui.painter().clone();
            viewer::paint_surface(&p, &f);
            ui.multiply_opacity(f.shape);
            let p = ui.painter().clone();
            let r = f.radius;
            // The hero: the card's cover at once, the large one fading in over it once decoded.
            let shown_large = feedback::loaded(&ctx, wid.with("large"), large.is_some());
            paint_cover(&p, hero, [r, r, 0.0, 0.0], small.as_ref().or(large.as_ref()), &self.id, 1.0, 1.0);
            if let (Some(l), true) = (&large, small.is_some()) {
                feedback::cover(&p, hero, [r, r, 0.0, 0.0], l, 1.0, shown_large);
            }
            let shade = paint::rounded_rect_points_corners(hero, [theme::squircle(r), theme::squircle(r), 0.0, 0.0]);
            paint::fill_gradient(
                &p,
                &shade,
                &Gradient::vertical(hero, &[(0.0, theme::black(0.18)), (0.3, Color32::TRANSPARENT), (0.55, theme::black(0.25)), (1.0, theme::black(0.82))]),
            );
            // Over the hero: status, title, when (laid out at the window's final width, faded in as it grows).
            let mut hu = ui.new_child(UiBuilder::new().max_rect(hero).id_salt(wid.with("hero")));
            hu.set_clip_rect(hero.intersect(ui.clip_rect()));
            hu.multiply_opacity(f.content);
            self.hero_text(&mut hu, hero, width, now);
            let mut cu = ui.new_child(UiBuilder::new().max_rect(f.rect).id_salt(wid.with("chrome")));
            cu.set_clip_rect(f.rect.intersect(ui.clip_rect()));
            cu.multiply_opacity(f.content);
            if closing || f.content < 0.05 {
                cu.disable();
            }
            if viewer::close_button(&mut cu, f.rect, tr("Close")) {
                close = true;
            }
            // The body, laid out at the window's final width, under the hero; it scrolls when taller than the room.
            let body_rect = Rect::from_min_max(pos2(f.rect.left(), hero.bottom()), pos2(f.rect.left() + width.max(width_now), f.rect.bottom()));
            let mut bu = ui.new_child(UiBuilder::new().max_rect(body_rect).id_salt(wid.with("body")));
            bu.set_clip_rect(Rect::from_min_max(pos2(f.rect.left(), hero.bottom()), f.rect.max).intersect(ui.clip_rect()));
            let alpha = viewer::smoothstep(0.55, 0.95, f.t);
            bu.multiply_opacity(alpha);
            if closing || alpha < 0.5 {
                bu.disable();
            }
            let landed = f.t > 0.98 && !closing;
            let dy = 12.0 * (1.0 - alpha);
            let mut measured = 0.0;
            egui::ScrollArea::vertical()
                .id_salt(wid.with("scroll"))
                .auto_shrink([false, false])
                .scroll_bar_visibility(if landed {
                    egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                } else {
                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                })
                .show(&mut bu, |ui| {
                    egui::Frame::NONE.inner_margin(egui::Margin { left: PAD as i8, right: PAD as i8, top: 0, bottom: 0 }).show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        ui.set_width(width - PAD * 2.0);
                        let top = ui.cursor().top();
                        ui.add_space(20.0 + dy);
                        self.body(ui, cx, pictures, covers, width - PAD * 2.0, now, &mut acts);
                        ui.add_space(26.0);
                        measured = ui.cursor().top() - top - dy;
                    });
                });
            if measured > 0.0 && (measured - self.body_h).abs() > 0.5 {
                self.body_h = measured;
            }
        });
        for a in acts {
            self.act(&ctx, cx, a);
        }
        if !closing && !self.covered && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close {
            self.morph.close();
        }
        if reopen && !f.closed {
            self.morph.reopen();
        }
        if anim::is_live(&ctx) {
            ctx.request_repaint_after(std::time::Duration::from_secs(15));
        }
        f.closed
    }

    fn phase_now(&self, now: i64) -> Phase {
        match &self.page {
            Some(o) => phase(Some(&o.status), o.starts_at.as_deref(), o.ends_at.as_deref(), o.steps.iter().any(|s| s.live), now),
            None => card_phase(&self.card, now),
        }
    }

    fn hero_text(&self, ui: &mut Ui, hero: Rect, width: f32, now: i64) {
        let p = ui.painter().clone();
        let title = self.page.as_ref().map_or(self.card.title.as_str(), |o| o.title.as_str());
        let (starts, ends) = match &self.page {
            Some(o) => (o.starts_at.as_deref(), o.ends_at.as_deref()),
            None => (self.card.starts_at.as_deref(), self.card.ends_at.as_deref()),
        };
        let x = hero.left() + PAD;
        let galley = p.layout_job({
            let mut job = Txt::new(title, if width < 600.0 { 26.0 } else { 32.0 })
                .weight(weight::BOLD)
                .tracking(-0.02)
                .color(Color32::WHITE)
                .line_height(if width < 600.0 { 30.0 } else { 37.0 })
                .job(width - PAD * 2.0 - 40.0);
            job.wrap.max_rows = 2;
            job
        });
        let when_y = hero.bottom() - 26.0;
        let title_top = when_y - 18.0 - galley.size().y;
        p.galley(pos2(x, title_top), galley, Color32::WHITE);
        let when = tr!("{time} · your time ({zone})", time = when(starts, ends), zone = zone());
        widgets::text_truncated(
            &p,
            pos2(x, when_y),
            Align2::LEFT_CENTER,
            &Txt::new(when, 14.0).weight(weight::MEDIUM).color(theme::white(0.86)),
            width - PAD * 2.0,
        );
        status_pill(&p, ui, pos2(x, title_top - 40.0), self.phase_now(now), false);
        if let Some(c) = self.card.category.as_deref().or(self.page.as_ref().and_then(|o| o.category.as_deref())) {
            let t = Txt::new(category_label(c), 11.5).weight(weight::SEMIBOLD).color(theme::white(0.9));
            let tw = widgets::text_size(ui, &t).x;
            let r = Rect::from_min_size(pos2(hero.left() + PAD, hero.top() + 22.0), vec2(tw + 22.0, 26.0));
            p.rect_filled(r, theme::radius(13.0), theme::black(0.45));
            widgets::text(&p, r.center(), Align2::CENTER_CENTER, &t);
        }
    }

    /// The step Invite me on the hero acts for: the one under way with sign-ups open, else the next.
    fn hero_step(&self, now: i64) -> Option<&community::Step> {
        let o = self.page.as_ref()?;
        let open: Vec<&community::Step> = o.steps.iter().filter(|s| s.invite_open || self.ask_of(&s.id).signed_up()).collect();
        open.iter()
            .find(|s| s.live)
            .copied()
            .or_else(|| open.iter().find(|s| unix(s.ends_at.as_deref()).is_none_or(|e| e > now)).copied())
            .or(open.first().copied())
    }

    #[allow(clippy::too_many_arguments)]
    fn body(&mut self, ui: &mut Ui, cx: &mut Cx, pictures: &mut Pictures, _covers: &Covers, width: f32, now: i64, acts: &mut Vec<Act>) {
        let ctx = ui.ctx().clone();
        let wid = self.morph.id();
        // Host and actions.
        let host = self.page.as_ref().and_then(|o| o.community.clone()).or_else(|| self.card.community.clone()).or_else(|| self.card.discord.clone());
        let narrow = width < 600.0;
        let (row, _) = ui.allocate_exact_size(vec2(width, if narrow { 104.0 } else { 52.0 }), Sense::hover());
        let p = ui.painter().clone();
        if let Some(h) = &host {
            let ar = Rect::from_min_size(pos2(row.left(), row.top() + 6.0), vec2(40.0, 40.0));
            let icon = h
                .icon_url
                .as_ref()
                .filter(|u| u.starts_with("https://"))
                .and_then(|u| pictures.get(&ctx, &Picture::Gallery { development: false, asset: 0, id: 0, url: u.clone(), large: false }));
            widgets::avatar(&p, ar, icon.as_ref(), &h.name);
            widgets::text(&p, pos2(ar.right() + 12.0, ar.top() + 11.0), Align2::LEFT_CENTER, &Txt::caps(tr("Hosted by")));
            widgets::text_truncated(
                &p,
                pos2(ar.right() + 12.0, ar.top() + 29.0),
                Align2::LEFT_CENTER,
                &Txt::new(&h.name, 15.0).weight(weight::SEMIBOLD),
                if narrow { width - 52.0 } else { width * 0.4 },
            );
        }
        let actions_y = if narrow { row.top() + 52.0 + 26.0 } else { row.top() + 26.0 };
        let instance = self.page.as_ref().and_then(|o| o.instance_url.clone()).filter(|u| u.starts_with("https://"));
        let hero_step = self.hero_step(now).map(|s| s.id.clone());
        // An instance under way that takes no sign-ups: Orbiters' own account invites them into it.
        let n4n0_step = self.page.as_ref().and_then(|o| o.steps.iter().find(|s| s.live && !s.invite_open && s.instance_url.is_some())).map(|s| s.id.clone());
        let ph = self.phase_now(now);
        let mut x = if narrow { row.left() } else { row.right() };
        let mut place = |w: f32| -> Pos2 {
            if narrow {
                let at = pos2(x, actions_y);
                x += w + 8.0;
                at
            } else {
                x -= w;
                let at = pos2(x, actions_y);
                x -= 8.0;
                at
            }
        };
        // Right to left on wide windows: Open on Orbiters, Invite me, Join in VRChat.
        let website = format!("{}events/{}", self.server.website(), self.id);
        let order: Vec<u8> = if narrow { vec![0, 1, 2] } else { vec![2, 1, 0] };
        for which in order {
            match which {
                0 => {
                    if let Some(url) = instance.clone() {
                        let b = Button::new(tr("Join in VRChat")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).id_salt(wid.with("join"));
                        let w = b.desired_width(ui);
                        let at = place(w);
                        if sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(at.x, at.y - 20.0), vec2(w, 40.0)))) {
                            acts.push(Act::Url(url));
                        }
                    }
                }
                1 => {
                    let (step, n4n0) = match (&hero_step, &n4n0_step) {
                        (Some(s), _) => (Some(s.clone()), false),
                        (None, Some(s)) => (Some(s.clone()), true),
                        _ => (None, false),
                    };
                    let ask = step.as_ref().map(|s| self.ask_of(s)).unwrap_or(Ask::Idle);
                    let enabled = step.is_some() && !matches!(ask, Ask::Unknown) && !matches!(ph, Phase::Ended | Phase::Cancelled);
                    // Width first (right to left), then drawn where it lands.
                    let probe = invite_width_now(ui, wid.with("hero-invite"), &ask, false);
                    let at = place(probe);
                    // Green only while it can be asked for (no Join beside it): a dead main action stays quiet.
                    let (_, press) = invite_control(ui, wid.with("hero-invite"), at, &ask, instance.is_none() && enabled, false, enabled);
                    if step.is_none() && self.page.is_some() {
                        let r = Rect::from_min_size(pos2(at.x, at.y - 20.0), vec2(probe, 40.0));
                        ui.interact(r, wid.with("no-invite"), Sense::hover()).on_hover_text(if matches!(ph, Phase::Ended | Phase::Cancelled) {
                            tr("This event is over")
                        } else {
                            tr("This event takes no invite sign-ups: join it in VRChat once it starts")
                        });
                    }
                    if let (Some(step), Some(press)) = (step, press) {
                        acts.push(match (press, n4n0) {
                            (Press::Cancel, _) => Act::Withdraw(step),
                            (Press::Ask, true) => {
                                let location = self
                                    .page
                                    .as_ref()
                                    .and_then(|o| o.steps.iter().find(|s| s.id == step))
                                    .and_then(|s| s.instance_url.as_deref())
                                    .and_then(community::launch_location)
                                    .unwrap_or_default();
                                Act::N4n0(step, location)
                            }
                            (Press::Ask, false) if matches!(ask, Ask::Group | Ask::Link) => Act::Check(step),
                            (Press::Ask, false) => Act::Ask(step),
                        });
                    }
                }
                _ => {
                    let b = widgets::IconButton::new(icons::ARROW_SQUARE_OUT, tr("Open on Orbiters"))
                        .size(40.0, 17.0)
                        .backdrop(theme::control())
                        .id_salt(wid.with("website"));
                    let at = place(40.0);
                    if sharing::pressed(b.show_at(ui, Rect::from_min_size(pos2(at.x, at.y - 20.0), vec2(40.0, 40.0)))) {
                        acts.push(Act::Url(website.clone()));
                    }
                }
            }
        }
        // What happens next, or what is missing, for the step last asked for.
        let focus = self.focus.clone().or(hero_step.clone()).or(n4n0_step.clone());
        if let Some(step) = focus {
            let ask = self.ask_of(&step);
            if let Some((tone, text, actions)) = ask_note(&ask, self.page.as_ref()) {
                ui.add_space(14.0);
                let step_title = self.page.as_ref().filter(|o| o.steps.len() > 1).and_then(|o| o.steps.iter().find(|s| s.id == step)).map(|s| s.title.clone());
                let text = match step_title {
                    Some(t) if !t.is_empty() => format!("{t}: {text}"),
                    _ => text,
                };
                if let Some(a) = note(ui, wid.with(("note", &step, format!("{:?}", std::mem::discriminant(&ask)))), tone, &text, &actions) {
                    acts.push(match a {
                        NoteAction::Url(_, url) => Act::Url(url),
                        NoteAction::Connect => Act::Url(format!("{}events/{}/invite/{}", self.server.website(), self.id, step)),
                        NoteAction::Check => Act::Check(step.clone()),
                        NoteAction::SignIn => Act::SignIn,
                    });
                }
            }
        }

        // What the page could not show.
        if let Some(err) = self.error.clone() {
            ui.add_space(18.0);
            let (text, action) = match err.code.as_deref() {
                Some("SIGN_IN_REQUIRED") => {
                    (tr("This event is shared with its community's members: sign in to Orbiters to see it.").to_string(), Some(NoteAction::SignIn))
                }
                _ if err.status == 403 => (tr!("This event isn't available to you: {message}", message = err.message), None),
                _ => (tr!("The event's page could not load: {message}", message = err.message), None),
            };
            let mut actions: Vec<NoteAction> = action.into_iter().collect();
            if err.status != 403 {
                actions.push(NoteAction::Check);
            }
            if let Some(a) = note(ui, wid.with("error"), if err.status == 403 { Tone::Warn } else { Tone::Danger }, &text, &actions) {
                acts.push(match a {
                    NoteAction::SignIn => Act::SignIn,
                    _ => Act::Retry,
                });
            }
        }

        let Some(o) = self.page.clone() else {
            if self.error.is_none() {
                // The event's page on its way: its outline.
                ui.add_space(22.0);
                let (r, _) = ui.allocate_exact_size(vec2(width, 150.0), Sense::hover());
                let p = ui.painter();
                for (i, w) in [0.92f32, 0.8, 0.86, 0.5].iter().enumerate() {
                    feedback::shimmer(p, Rect::from_min_size(pos2(r.left(), r.top() + i as f32 * 24.0), vec2(width * w, 13.0)), 6.0);
                }
                feedback::shimmer(p, Rect::from_min_size(pos2(r.left(), r.top() + 112.0), vec2(width, 38.0)), 14.0);
            }
            return;
        };

        // Facts.
        ui.add_space(20.0);
        let mut facts: Vec<(Option<&str>, String)> = Vec::new();
        if let Some(c) = o.category.as_deref() {
            facts.push((None, category_label(c)));
        }
        facts.push((
            Some(icons::DOOR_OPEN),
            match o.access.as_deref() {
                Some("public") => tr("Open to everyone"),
                Some("plus") => tr("Group+"),
                _ => tr("Group members"),
            }
            .to_string(),
        ));
        if o.steps.len() > 1 {
            facts.push((Some(icons::LIST_CHECKS), tr_n("{count} step", "{count} steps", o.steps.len() as u64)));
        }
        if o.adult_only {
            facts.push((Some(icons::SHIELD_CHECK), "18+".into()));
        }
        if let Some(d) = &o.discord {
            facts.push((Some(icons::DISCORD_LOGO), tr!("Also on {name}", name = d.name)));
        }
        let widths: Vec<f32> = facts.iter().map(|(g, l)| widgets::text_size(ui, &Txt::new(l, 12.5)).x + 26.0 + if g.is_some() { 20.0 } else { 0.0 }).collect();
        let (at, h) = widgets::flow(&widths, width, 28.0, 8.0);
        let (r, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
        let p = ui.painter().clone();
        for (i, (glyph, label)) in facts.iter().enumerate() {
            let fr = Rect::from_min_size(r.min + at[i], vec2(widths[i], 28.0));
            p.rect_filled(fr, theme::radius(14.0), theme::white(0.06));
            let mut tx = fr.left() + 13.0;
            if let Some(g) = glyph {
                widgets::icon(&p, pos2(tx + 7.0, fr.center().y), g, 13.0, theme::FG_3);
                tx += 20.0;
            }
            widgets::text(&p, pos2(tx, fr.center().y), Align2::LEFT_CENTER, &Txt::new(label, 12.5).color(theme::FG_2));
        }

        // What it is.
        let text = [o.description.trim(), plain(&o.long_description).trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
        if !text.is_empty() {
            ui.add_space(18.0);
            let g = ui.painter().layout_job(Txt::new(&text, 14.5).color(theme::FG_2).line_height(23.0).job(width.min(720.0)));
            let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, theme::FG_2);
        }

        // Program.
        ui.add_space(26.0);
        let single = o.steps.len() == 1;
        let (r, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
        widgets::text(
            ui.painter(),
            pos2(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            &Txt::new(if single { tr("Where we meet") } else { tr("Program") }, 17.0).weight(weight::SEMIBOLD),
        );
        ui.add_space(10.0);
        for (i, s) in o.steps.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.0);
            }
            if let Some(a) = self.step_row(ui, cx, pictures, s, single, width, now) {
                acts.push(a);
            }
        }

        // Its pictures.
        if self.show_pictures && !self.pictures.is_empty() {
            ui.add_space(26.0);
            let (r, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
            let p = ui.painter().clone();
            widgets::text(&p, pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Pictures from this event"), 17.0).weight(weight::SEMIBOLD));
            widgets::text(&p, pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, &Txt::new(tr("Also in Photos & captures"), 12.0).color(theme::FG_3));
            ui.add_space(10.0);
            let n = self.pictures.len().min(if width < 600.0 { 3 } else { 4 });
            let gap = 10.0;
            let tw = (width - gap * (n as f32 - 1.0)) / n as f32;
            let th = tw * 9.0 / 16.0;
            let (row, _) = ui.allocate_exact_size(vec2(width, th), Sense::hover());
            for i in 0..n {
                let pic = &self.pictures[i];
                let tr = Rect::from_min_size(pos2(row.left() + i as f32 * (tw + gap), row.top()), vec2(tw, th));
                let tex = super::community::source(pic, false, cx.snap.settings.development, &self.stand_ins).and_then(|p| pictures.get(&ctx, &p));
                let resp = ui.interact(tr, wid.with(("picture", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                let hover = anim::toggle(&ctx, wid.with(("picture-hover", i)), resp.hovered(), Spring::SOFT);
                let p = ui.painter();
                match &tex {
                    Some(t) => feedback::cover(p, tr, [14.0; 4], t, 1.0 + 0.05 * hover, 1.0),
                    None => feedback::shimmer(p, tr, 14.0),
                }
                if i + 1 == n && self.pictures.len() > n {
                    feedback::fill_corners(p, tr, [14.0; 4], theme::black(0.55));
                    widgets::text(p, tr.center(), Align2::CENTER_CENTER, &Txt::new(format!("+{}", self.pictures.len() - n), 18.0).weight(weight::SEMIBOLD));
                }
                if sharing::pressed(resp) {
                    self.picture_pressed = Some((i, tr));
                }
            }
        }
    }

    /// One step: its time, its world's picture, what and where, live, and its own Join and Invite me.
    #[allow(clippy::too_many_arguments)]
    fn step_row(&mut self, ui: &mut Ui, _cx: &mut Cx, pictures: &mut Pictures, s: &community::Step, single: bool, width: f32, now: i64) -> Option<Act> {
        let ctx = ui.ctx().clone();
        let wid = self.morph.id();
        let narrow = width < 600.0;
        let h = if narrow { 136.0 } else { 96.0 };
        let (r, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
        let p = ui.painter().clone();
        let live = s.live || phase(None, s.starts_at.as_deref(), s.ends_at.as_deref(), false, now) == Phase::Live && s.instance_url.is_some();
        widgets::frame(&p, r, 20.0, if live { theme::alpha(theme::ACCENT, 0.1) } else { theme::white(0.035) });
        // Time.
        let time = if s.organizer_starts { tr("Later").to_string() } else { parse(s.starts_at.as_deref()).map(local).map(clock).unwrap_or_else(|| "—".into()) };
        let time_box = Rect::from_min_size(pos2(r.left() + 14.0, r.top() + 14.0), vec2(58.0, 28.0));
        p.rect_filled(time_box, theme::radius(14.0), theme::white(0.06));
        widgets::text(&p, time_box.center(), Align2::CENTER_CENTER, &Txt::new(time, 12.0).weight(weight::BOLD).color(theme::ACCENT));
        // World.
        let wr = Rect::from_min_size(pos2(time_box.right() + 12.0, r.top() + 12.0), vec2(96.0, 72.0));
        let world_tex = s
            .world
            .as_ref()
            .and_then(|w| Some((w.id.clone()?, w.image_url.clone().filter(|u| u.starts_with("https://"))?)))
            .and_then(|(id, url)| pictures.get(&ctx, &Picture::Remote { world_id: id, url, large: false }));
        match &world_tex {
            Some(t) => feedback::cover(&p, wr, [12.0; 4], t, 1.0, 1.0),
            None => {
                p.rect_filled(wr, theme::radius(12.0), theme::white(0.05));
                widgets::icon(&p, wr.center(), if s.world.is_some() { icons::GLOBE } else { icons::LIST_CHECKS }, 20.0, theme::FG_4);
            }
        }
        // What and where.
        let tx = wr.right() + 14.0;
        let world_name = s.world.as_ref().and_then(|w| w.name.clone());
        let title = if single { world_name.clone().unwrap_or_else(|| s.title.clone()) } else { s.title.clone() };
        let text_w = if narrow { r.right() - tx - 14.0 } else { (r.right() - tx - 14.0 - 220.0).max(120.0) };
        let tt = Txt::new(&title, 15.0).weight(weight::SEMIBOLD);
        let title_r = widgets::text_truncated(&p, pos2(tx, r.top() + 26.0), Align2::LEFT_CENTER, &tt, text_w);
        if live {
            // As wide as its word needs: "Live" fits 48 points, "En direct" does not.
            let live_text = Txt::new(tr("Live"), 11.0).weight(weight::BOLD).color(Color32::WHITE);
            let live_w = (widgets::text_size(ui, &live_text).x + 24.0).max(48.0);
            let lr = Rect::from_min_size(pos2(title_r.right() + 10.0, r.top() + 15.0), vec2(live_w, 22.0));
            if lr.right() < tx + text_w + 60.0 {
                p.rect_filled(lr, theme::radius(11.0), theme::alpha(theme::REC, 0.9));
                p.circle_filled(pos2(lr.left() + 12.0, lr.center().y), 3.0, Color32::WHITE);
                widgets::text(&p, pos2(lr.left() + 20.0, lr.center().y), Align2::LEFT_CENTER, &live_text);
            }
        }
        let sub = match (&world_name, single) {
            (Some(w), false) => w.clone(),
            (Some(_), true) => s.title.clone(),
            (None, _) => tr("World chosen by vote").into(),
        };
        widgets::text_truncated(&p, pos2(tx, r.top() + 48.0), Align2::LEFT_CENTER, &Txt::new(&sub, 12.5).color(theme::FG_3), text_w);
        let ask = self.ask_of(&s.id);
        let line = if s.organizer_starts {
            tr("Starts when the organizer is ready").to_string()
        } else {
            match (parse(s.starts_at.as_deref()).map(local), parse(s.ends_at.as_deref()).map(local)) {
                (Some(a), Some(b)) => format!("{} – {}", clock(a), clock(b)),
                (Some(a), None) => clock(a),
                _ => String::new(),
            }
        };
        if let Some(m) = &s.movie {
            widgets::icon(&p, pos2(tx + 6.0, r.top() + 70.0), icons::FILM_STRIP, 12.0, theme::FG_3);
            widgets::text_truncated(&p, pos2(tx + 18.0, r.top() + 70.0), Align2::LEFT_CENTER, &Txt::new(&m.title, 12.0).color(theme::FG_2), text_w - 18.0);
        } else if !line.is_empty() {
            widgets::text_truncated(&p, pos2(tx, r.top() + 70.0), Align2::LEFT_CENTER, &Txt::new(line, 12.0).color(theme::FG_4), text_w);
        }
        // Its actions.
        let mut act = None;
        let (ay, mut ax) = if narrow { (r.bottom() - 28.0, tx) } else { (r.center().y, r.right() - 14.0) };
        let join = s.instance_url.clone().filter(|u| u.starts_with("https://"));
        let invite = s.invite_open || ask.signed_up() || !matches!(ask, Ask::Idle | Ask::Unknown);
        if invite {
            let w = invite_width_now(ui, wid.with(("step-invite", &s.id)), &ask, true);
            let at = if narrow {
                pos2(ax, ay)
            } else {
                ax -= w;
                pos2(ax, ay)
            };
            let (w_now, press) = invite_control(ui, wid.with(("step-invite", &s.id)), at, &ask, false, true, !matches!(ask, Ask::Unknown));
            if narrow {
                ax += w_now + 8.0;
            } else {
                ax -= 8.0;
            }
            if let Some(press) = press {
                act = Some(match press {
                    Press::Cancel => Act::Withdraw(s.id.clone()),
                    Press::Ask if matches!(ask, Ask::Group | Ask::Link) => Act::Check(s.id.clone()),
                    Press::Ask => Act::Ask(s.id.clone()),
                });
            }
        }
        if let Some(url) = join {
            let b = Button::new(tr("Join")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).size(Size::Sm).id_salt(wid.with(("step-join", &s.id)));
            let w = b.desired_width(ui);
            let br = if narrow { Rect::from_min_size(pos2(ax, ay - 16.0), vec2(w, 32.0)) } else { Rect::from_min_size(pos2(ax - w, ay - 16.0), vec2(w, 32.0)) };
            if sharing::pressed(b.show_at(ui, br)) {
                act = Some(Act::Url(url));
            }
        }
        act
    }
}

/// What the person asked of the window.
enum Act {
    Url(String),
    SignIn,
    Retry,
    Ask(String),
    Check(String),
    Withdraw(String),
    /// Orbiters' own account invites them into the open instance of a step (`wrld_…:<instance>`).
    N4n0(String, String),
}

/// The width the Invite me control `id` has this frame (on its way to [`invite_width`]), to place it before drawing it.
fn invite_width_now(ui: &Ui, id: Id, ask: &Ask, small: bool) -> f32 {
    anim::value(ui.ctx(), id.with("w"), invite_width(ui, ask, small), Spring::MORPH).max(if small { 32.0 } else { 40.0 })
}

/// The width an Invite me control settles at for `ask`.
fn invite_width(ui: &Ui, ask: &Ask, small: bool) -> f32 {
    let size = if small { 12.5 } else { 13.5 };
    match ask {
        Ask::Busy(l) => widgets::text_size(ui, &Txt::new(tr(l), size).weight(weight::MEDIUM)).x + 50.0,
        Ask::Listed { .. } => {
            widgets::text_size(ui, &Txt::new(tr("Will invite you"), size).weight(weight::SEMIBOLD)).x
                + 46.0
                + widgets::text_size(ui, &Txt::new(tr("Cancel"), size).weight(weight::MEDIUM)).x
                + 24.0
        }
        Ask::Sent(l) => widgets::text_size(ui, &Txt::new(tr(l), size).weight(weight::SEMIBOLD)).x + 46.0,
        Ask::Invited => widgets::text_size(ui, &Txt::new(tr("Invite sent"), size).weight(weight::SEMIBOLD)).x + 46.0,
        _ => Button::new(tr("Invite me")).icon(icons::PAPER_PLANE_TILT).size(if small { Size::Sm } else { Size::Md }).desired_width(ui),
    }
}

/// A tinted line across the body with its icon, the text wrapped, and buttons at its end (or under it when narrow).
/// Returns the action pressed.
fn note(ui: &mut Ui, id: Id, tone: Tone, text: &str, actions: &[NoteAction]) -> Option<NoteAction> {
    let w = ui.available_width();
    let labels: Vec<String> = actions
        .iter()
        .map(|a| match a {
            NoteAction::Url(l, _) => tr(l).to_string(),
            NoteAction::Connect => tr("Connect on Orbiters").into(),
            NoteAction::Check => tr("Check again").into(),
            NoteAction::SignIn => tr("Sign in").into(),
        })
        .collect();
    let bws: Vec<f32> = labels.iter().map(|l| Button::new(l).size(Size::Sm).desired_width(ui)).collect();
    let buttons_w = bws.iter().sum::<f32>() + 8.0 * bws.len().saturating_sub(1) as f32;
    let beside = !actions.is_empty() && w - buttons_w > 360.0;
    let text_w = if beside { w - 56.0 - buttons_w - 16.0 } else { w - 56.0 };
    let galley = ui.painter().layout_job(Txt::new(text, 13.0).color(theme::FG).line_height(19.0).job(text_w));
    let text_h = galley.size().y.max(19.0);
    let h = if beside || actions.is_empty() { (text_h + 26.0).max(52.0) } else { text_h + 26.0 + 42.0 };
    let k = anim::spring_from(ui.ctx(), id.with("in"), [0.0], [1.0], Spring::SOFT)[0];
    let (rect, _) = ui.allocate_exact_size(vec2(w, h * k.clamp(0.0, 1.0)), Sense::hover());
    let r = Rect::from_min_size(rect.min, vec2(w, h));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(id));
    cu.set_clip_rect(rect.expand(1.0).intersect(ui.clip_rect()));
    cu.multiply_opacity(k.clamp(0.0, 1.0));
    let p = cu.painter().clone();
    let c = tone.color();
    widgets::frame(&p, r, 16.0, theme::alpha(c, 0.1));
    let glyph = match tone {
        Tone::Ok => icons::CHECK_CIRCLE,
        Tone::Danger | Tone::Rec | Tone::Warn => icons::WARNING_CIRCLE,
        Tone::Info | Tone::Off => icons::INFO,
    };
    widgets::icon(&p, pos2(r.left() + 26.0, r.top() + 13.0 + 9.5), glyph, 17.0, c);
    p.galley(pos2(r.left() + 46.0, r.top() + 13.0), galley, theme::FG);
    let mut pressed = None;
    let (mut x, y) = if beside { (r.right() - 12.0 - buttons_w, r.center().y - 16.0) } else { (r.left() + 46.0, r.top() + 13.0 + text_h + 8.0) };
    for (i, a) in actions.iter().enumerate() {
        let b = Button::new(&labels[i]).size(Size::Sm).variant(if i == 0 { Variant::Secondary } else { Variant::Ghost }).id_salt(id.with(("action", i)));
        if sharing::pressed(b.show_at(&mut cu, Rect::from_min_size(pos2(x, y), vec2(bws[i], 32.0)))) {
            pressed = Some(a.clone());
        }
        x += bws[i] + 8.0;
    }
    pressed
}

/// Markdown as plain text: headings' hashes, emphasis marks and links' addresses gone.
fn plain(md: &str) -> String {
    let mut out = String::new();
    for line in md.lines() {
        let l = line.trim_start_matches('#').trim_start();
        let l = l.strip_prefix("- ").map(|r| format!("• {r}")).unwrap_or_else(|| l.to_string());
        let mut s = String::new();
        let mut rest = l.as_str();
        // [text](url) → text
        while let Some(open) = rest.find('[') {
            let Some(mid) = rest[open..].find("](") else { break };
            let Some(close) = rest[open + mid..].find(')') else { break };
            s.push_str(&rest[..open]);
            s.push_str(&rest[open + 1..open + mid]);
            rest = &rest[open + mid + close + 1..];
        }
        s.push_str(rest);
        out.push_str(&s.replace("**", "").replace("__", "").replace('`', ""));
        out.push('\n');
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------------------------------------------------
// The page.

#[derive(Default)]
pub struct State {
    list: Job<anyhow::Result<Cards>>,
    cards: Vec<Card>,
    next: Option<u64>,
    /// The environment and visit the list was read for, and when (egui's clock).
    loaded: Option<(bool, u64, f64)>,
    error: Option<String>,
    query: String,
    pictures: Option<Pictures>,
    covers: Covers,
    window: Option<Window>,
    /// A picture of the event's gallery shown large (from the window's strip).
    picture: Option<super::community::PictureWindow>,
    /// Each card's cover as last drawn (by event id): windows open out of it and close back into it.
    cover_rects: HashMap<String, Rect>,
    /// Steps signed up for from a window (by event id): their card says so.
    signed_up: HashMap<String, bool>,
    fixture: Option<String>,
}

/// Cards are at least this wide (as Visited worlds').
const CARD_MIN: f32 = 280.0;
/// What a card takes under its cover.
const CARD_BODY: f32 = 124.0;

impl State {
    pub fn busy(&self) -> bool {
        self.list.running() || self.pictures.as_ref().is_some_and(|p| p.loading()) || self.window.as_ref().is_some_and(Window::loading)
    }

    /// Screenshots: the fixture's events, their times moved so the first is under way and the second starts soon,
    /// this computer's VRChat pictures as their covers.
    pub fn stage(&mut self, name: &str, sessions_dir: &std::path::Path) {
        self.fixture = Some(name.to_string());
        let mut page: Cards = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/events.json")).unwrap();
        retime(&mut page.items);
        if name == "events-empty" {
            page.items.clear();
        }
        let refused = "connection refused"; // i18n: not shown (the system's own English words, as the network layer reports them)
        self.error = (name == "events-error").then(|| tr!("Orbiters could not be reached: {e}", e = refused));
        if name == "events-error" {
            page.items.clear();
        }
        if name == "events-search" {
            self.query = "night".into();
        }
        self.covers = Covers { development: false, fixture: Covers::stand_ins(sessions_dir), order: page.items.iter().map(|c| c.id.clone()).collect() };
        if self.covers.fixture.is_empty() {
            // No VRChat pictures here: the events' own colours.
            for c in &mut page.items {
                c.has_banner = false;
            }
        }
        self.cards = page.items;
        self.list = Job::default();
        self.loaded = Some((false, u64::MAX, 0.0));
    }

    /// Opens the window of the `n`th card (0: the hero), as if Join was pressed (screenshots), on the fixture's page
    /// with the member's invites as `asks`.
    pub fn open_card(&mut self, ctx: &egui::Context, n: usize, asks: &[(&str, Ask)], loading: bool, with_pictures: bool) {
        let Some(card) = self.shown().get(n).cloned() else { return };
        let origin = self.cover_rects.get(&card.id).copied().unwrap_or(Rect::from_min_size(pos2(300.0, 200.0), vec2(400.0, 225.0)));
        let mut w = Window::open(ctx, card.clone(), origin, if n == 0 { 26.0 } else { 20.0 }, Server::Production);
        if loading {
            w.stage_loading();
        } else {
            let pictures = if with_pictures {
                let feed: community::Pictures = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/gallery-pictures.json")).unwrap();
                feed.items.into_iter().filter(|p| p.gallery_id == 31).collect()
            } else {
                Vec::new()
            };
            w.stage(staged_overview(&card), asks, pictures);
            w.stand_ins = self.covers.fixture.clone();
        }
        self.window = Some(w);
    }

    pub fn close_window(&mut self) {
        if let Some(w) = self.window.as_mut() {
            w.close();
        }
    }

    /// The events the search shows, soonest first.
    fn shown(&self) -> Vec<Card> {
        let q = self.query.trim().to_lowercase();
        let hit = |c: &Card| {
            q.is_empty()
                || c.title.to_lowercase().contains(&q)
                || c.description.to_lowercase().contains(&q)
                || c.community.as_ref().is_some_and(|h| h.name.to_lowercase().contains(&q))
                || c.world.as_ref().and_then(|w| w.name.as_ref()).is_some_and(|n| n.to_lowercase().contains(&q))
        };
        self.cards.iter().filter(|c| hit(c)).cloned().collect()
    }

    fn fetch(&mut self, ctx: &egui::Context, server: Server, more: bool) {
        let offset = if more { self.next.unwrap_or(0) } else { 0 };
        self.error = None;
        self.list = Job::spawn(ctx, move || Api::new(server).events(offset));
        if !more {
            self.next = None;
        }
    }
}

/// The fixture's times, moved: the first event started 35 minutes ago, the second starts in 2 h 14 min, the others on
/// the next days at their own hour.
pub fn retime(cards: &mut [Card]) {
    let now = now();
    let iso = |t: i64| OffsetDateTime::from_unix_timestamp(t).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok());
    for (i, c) in cards.iter_mut().enumerate() {
        let (Some(s), Some(e)) = (unix(c.starts_at.as_deref()), unix(c.ends_at.as_deref())) else { continue };
        let length = e - s;
        let start = match i {
            0 => now - 35 * 60,
            1 => now + 2 * 3600 + 14 * 60,
            n => (now / 86400 + n as i64 - 1) * 86400 + s % 86400,
        };
        c.starts_at = iso(start);
        c.ends_at = iso(start + length);
    }
}

/// The fixture's event page for `card`: its own words and times, the fixture's program (moved with it).
pub fn staged_overview(card: &Card) -> Overview {
    let mut o: Overview = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/event-overview.json")).unwrap();
    let (Some(s), Some(e)) = (unix(card.starts_at.as_deref()), unix(card.ends_at.as_deref())) else { return o };
    let live = s <= now();
    let iso = |t: i64| OffsetDateTime::from_unix_timestamp(t).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok());
    o.id = card.id.clone();
    o.title = card.title.clone();
    o.description = card.description.clone();
    o.category = card.category.clone();
    o.has_banner = card.has_banner;
    o.starts_at = card.starts_at.clone();
    o.ends_at = card.ends_at.clone();
    if !live {
        o.instance_url = None;
    }
    let half = (e - s) / 2;
    for (i, step) in o.steps.iter_mut().enumerate() {
        step.starts_at = iso(s + i as i64 * half);
        step.ends_at = iso(s + (i as i64 + 1) * half);
        step.live = live && i == 0;
        if !step.live {
            step.instance_url = None;
        }
    }
    o
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let page = ui.max_rect();
    let now_t = anim::time(&ctx);
    let development = cx.snap.settings.development;
    let server = if development { Server::Development } else { Server::Production };
    if state.pictures.is_none() {
        state.pictures = Some(Pictures::new(&ctx));
    }
    if state.fixture.is_none() {
        state.covers.development = development;
        state.covers.fixture.clear();
        // Read again on every visit (after a minute), and when the environment changes.
        let stale = state.loaded.is_none_or(|(dev, visit, at)| dev != development || (visit != cx.visit && now_t - at > 60.0));
        if stale && !state.list.running() {
            state.loaded = Some((development, cx.visit, now_t));
            state.fetch(&ctx, server, false);
        }
    }
    if let Some(r) = state.list.take() {
        match r {
            Ok(page) => {
                if state.next.is_none() || state.cards.is_empty() {
                    state.cards.clear();
                }
                for c in page.items {
                    if !state.cards.iter().any(|k| k.id == c.id) {
                        state.cards.push(c);
                    }
                }
                state.next = page.next_offset;
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }
    let shown = state.shown();
    let now = now();
    let title = tr("Events");
    let header = super::header::Header {
        caption: "Orbiters",
        title,
        sub: &if state.list.running() && state.cards.is_empty() {
            tr("Finding what's on…").to_string()
        } else {
            let live = state.cards.iter().filter(|c| card_phase(c, now) == Phase::Live).count();
            let mut parts = vec![tr_n("{count} event", "{count} events", state.cards.len() as u64)];
            if live > 0 {
                parts.push(tr!("{count} happening now", count = live));
            }
            parts.push(tr!("times in your time zone ({zone})", zone = zone()));
            parts.join(" · ")
        },
        busy: state.list.running(),
        search: Some(("events-search", tr("Search events"))),
        actions: vec![super::header::Action::new("refresh", tr("Refresh"), icons::ARROW_CLOCKWISE)
            .icon_only()
            .enabled(!state.list.running())
            .spinning(state.list.running())],
    };
    let placed = header.place(ui, page, cx);
    let top = placed.content_top;

    // The open window's card sits under it (hidden while it is out, back as it lands).
    let open_id = state.window.as_ref().map(|w| (w.id.clone(), w.card_shown()));
    let modal = state.window.is_some() || state.picture.is_some();
    let from = orbiters_ui::blur::next_shape(ui);
    let mut gu = ui.new_child(UiBuilder::new().max_rect(page).id_salt("events-grid"));
    let mut clicked: Option<(Card, Rect, f32)> = None;
    let mut more = false;
    let mut retry = false;
    let pictures = state.pictures.as_mut().unwrap();
    egui::ScrollArea::vertical().id_salt("events").auto_shrink([false, false]).scroll_bar_rect(Rect::from_min_max(pos2(page.left(), top), page.max)).show(
        &mut gu,
        |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 40 }).show(ui, |ui| {
                ui.add_space(top - page.top());
                let width = ui.available_width();
                if shown.is_empty() {
                    if state.list.running() {
                        let (area, _) = ui.allocate_exact_size(vec2(width, 320.0 + 40.0 + 300.0), Sense::hover());
                        feedback::shimmer(ui.painter(), Rect::from_min_size(area.min, vec2(width, 320.0)), 28.0);
                        let (cols, w) = orbiters_ui::layout::columns(width, CARD_MIN, 16.0);
                        for c in 0..cols {
                            super::sessions::shimmer_card(
                                ui,
                                Rect::from_min_size(area.min + vec2(c as f32 * (w + 16.0), 360.0), vec2(w, w * 9.0 / 16.0 + CARD_BODY)),
                            );
                        }
                    } else if let Some(e) = state.error.clone() {
                        ui.add_space(12.0);
                        feedback::banner(ui, Id::unique("events-error"), Tone::Danger, &tr!("Orbiters' events could not be loaded: {e}", e = e), false);
                        ui.add_space(10.0);
                        let b = Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(Size::Sm);
                        let bw = b.desired_width(ui);
                        let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                        if sharing::pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                            retry = true;
                        }
                    } else {
                        empty_state(ui, cx, !state.query.trim().is_empty(), (page.bottom() - top) * 0.6);
                    }
                    return;
                }
                // The next event, large.
                let hero = &shown[0];
                let hero_h = (width * 0.36).clamp(260.0, 360.0);
                let (hr, _) = ui.allocate_exact_size(vec2(width, hero_h), Sense::hover());
                let (hr, ha) = super::entering(ui, cx, 1, hr);
                let card_alpha = open_id.as_ref().filter(|(id, _)| *id == hero.id).map_or(1.0, |(_, a)| *a);
                let cover = state.covers.card(hero, CoverSize::Large).and_then(|p| pictures.get(&ctx, &p));
                let small = state.covers.card(hero, CoverSize::Card).and_then(|p| pictures.get(&ctx, &p));
                let glow = state.covers.card(hero, CoverSize::Glow).and_then(|p| pictures.get(&ctx, &p));
                state.cover_rects.insert(hero.id.clone(), hr);
                let mut hu = ui.new_child(UiBuilder::new().max_rect(hr.expand(90.0)).id_salt("events-hero"));
                hu.set_clip_rect(ui.clip_rect());
                hu.multiply_opacity(ha * card_alpha);
                let signed = state.signed_up.get(&hero.id).copied().unwrap_or(false);
                if hero_card(&mut hu, cx, hero, hr, cover.as_ref().or(small.as_ref()), glow.as_ref(), !modal, signed, now) {
                    clicked = Some((hero.clone(), hr, 26.0));
                }
                ui.add_space(36.0);
                // The rest, by day.
                let rest: Vec<&Card> = shown.iter().skip(1).collect();
                let mut index = 2usize;
                for (period, group) in period_groups(&rest, now) {
                    let label = tr(period.label());
                    let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                    let (r, a) = super::entering(ui, cx, index.min(8), r);
                    let mut p = ui.painter().clone();
                    p.multiply_opacity(a);
                    widgets::text(&p, pos2(r.left(), r.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(label, 18.0).weight(weight::SEMIBOLD).tracking(-0.01));
                    let count = tr_n("{count} event", "{count} events", group.len() as u64);
                    widgets::text(&p, pos2(r.left(), r.top() + 35.0), Align2::LEFT_CENTER, &Txt::new(count, 12.0).color(theme::FG_3));
                    ui.add_space(14.0);
                    let (cols, w) = orbiters_ui::layout::columns(width, CARD_MIN, 16.0);
                    let h = w * 9.0 / 16.0 + CARD_BODY;
                    let rows = group.len().div_ceil(cols);
                    let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * (h + 16.0) - 16.0), Sense::hover());
                    for (n, i) in group.iter().enumerate() {
                        let c = rest[*i];
                        let r =
                            Rect::from_min_size(pos2(area.left() + (n % cols) as f32 * (w + 16.0), area.top() + (n / cols) as f32 * (h + 16.0)), vec2(w, h));
                        index += 1;
                        let thumb = Rect::from_min_size(r.min, vec2(w, w * 9.0 / 16.0));
                        state.cover_rects.insert(c.id.clone(), thumb);
                        if !ui.is_rect_visible(r) {
                            continue;
                        }
                        let tex = state.covers.card(c, CoverSize::Card).and_then(|p| pictures.get(&ctx, &p));
                        let shown = open_id.as_ref().filter(|(id, _)| *id == c.id).map_or(1.0, |(_, a)| *a);
                        let signed = state.signed_up.get(&c.id).copied().unwrap_or(false);
                        if let Some(thumb) = event_card(ui, cx, index, c, r, tex.as_ref(), shown, !modal, signed, now) {
                            clicked = Some((c.clone(), thumb, 20.0));
                        }
                    }
                    ui.add_space(32.0);
                }
                if state.next.is_some() {
                    let b = Button::new(if state.list.running() { tr("Loading…") } else { tr("Show more") }).variant(Variant::Ghost).enabled(!state.list.running());
                    let bw = b.desired_width(ui);
                    let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                    if sharing::pressed(b.show_at(ui, Rect::from_center_size(r.center(), vec2(bw, 40.0)))) {
                        more = true;
                    }
                }
            });
        },
    );
    super::under_top(&gu, cx, "events", from, placed.band_bottom);
    if header.show(ui, cx, &placed, Some(&mut state.query)) == Some("refresh") && state.fixture.is_none() {
        state.fetch(&ctx, server, false);
    }
    if retry && state.fixture.is_none() {
        state.fetch(&ctx, server, false);
    }
    if more && state.fixture.is_none() {
        state.fetch(&ctx, server, true);
    }
    if let Some((card, origin, radius)) = clicked {
        state.window = Some(Window::open(&ctx, card, origin, radius, server));
    }
    let covers = state.covers.clone();
    if let Some(w) = state.window.as_mut() {
        if let Some(r) = state.cover_rects.get(&w.id).copied().filter(|r| r.intersects(page)) {
            w.set_origin(r);
        }
        let pictures = state.pictures.as_mut().unwrap();
        // A picture over the window: the window waits under it.
        w.covered = state.picture.is_some();
        let closed = w.show(ui, cx, pictures, &covers);
        if w.signed_up() {
            state.signed_up.insert(w.id.clone(), true);
        }
        if let Some((i, origin)) = w.picture_pressed.take() {
            let list = w.pictures.clone();
            let stand_ins = state.covers.fixture.clone();
            state.picture = Some(super::community::PictureWindow::open(&ctx, list, i, origin, None, Some(w.card.clone())).with_stand_ins(stand_ins));
        }
        if closed {
            state.window = None;
        }
    }
    if let Some(pw) = state.picture.as_mut() {
        let pictures = state.pictures.as_mut().unwrap();
        if pw.show(ui, cx, pictures, Order::Tooltip) {
            state.picture = None;
        }
    }
    if anim::is_live(&ctx) {
        ctx.request_repaint_after(std::time::Duration::from_secs(15));
    }
}

/// When the events of a section take place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Period {
    Now,
    /// Within the next seven days.
    Week,
    Later,
}

impl Period {
    fn label(self) -> &'static str {
        match self {
            Period::Now => mark("Happening now"),
            Period::Week => mark("This week"),
            Period::Later => mark("Later"),
        }
    }
}

/// The cards in sections (under way, this week, later), each a grid of its own: the cards' date tiles carry their
/// day, so wide windows fill their rows instead of one short row a day.
fn period_groups(cards: &[&Card], now: i64) -> Vec<(Period, Vec<usize>)> {
    let mut groups: Vec<(Period, Vec<usize>)> = Vec::new();
    for (i, c) in cards.iter().enumerate() {
        let period = match card_phase(c, now) {
            Phase::Live => Period::Now,
            Phase::Soon(s) if s <= 7 * 86400 => Period::Week,
            _ => Period::Later,
        };
        match groups.iter_mut().find(|(p, _)| *p == period) {
            Some((_, g)) => g.push(i),
            None => groups.push((period, vec![i])),
        }
    }
    groups.sort_by_key(|(p, _)| *p);
    groups
}

/// The next event, across the page: its cover with its light behind it, its status, title, when, who hosts it and
/// Join. True when pressed.
#[allow(clippy::too_many_arguments)]
fn hero_card(
    ui: &mut Ui,
    cx: &Cx,
    c: &Card,
    rect: Rect,
    tex: Option<&TextureHandle>,
    glow: Option<&TextureHandle>,
    interactive: bool,
    signed: bool,
    now: i64,
) -> bool {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("event-hero", &c.id));
    let resp = ui.interact(rect, id, if interactive { Sense::click() } else { Sense::hover() });
    let resp = if interactive { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() && interactive, Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on() && interactive, Spring::SNAPPY);
    let landing = morph::bounce(&ctx, rect);
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.012 * press) * landing);
    let p = ui.painter().clone();
    paint_glow(&p, r, 28.0, vec2(26.0, 30.0), glow, 0.75 + 0.25 * hover);
    paint::shadow(&p, r, 28.0, vec2(0.0, 20.0), 50.0, -10.0, theme::black(0.45));
    paint_cover_at(&p, r, [28.0; 4], tex, &c.id, 1.0 + 0.035 * ease::out_cubic(hover), 1.0, 0.8);
    // A dark side for the words, the picture free on the other.
    let pts = paint::squircle_points(r, theme::squircle(28.0));
    paint::fill_gradient(&p, &pts, &Gradient::horizontal(r, &[(0.0, theme::black(0.78)), (0.45, theme::black(0.45)), (0.8, Color32::TRANSPARENT)]));
    paint::fill_gradient(&p, &pts, &Gradient::vertical(r, &[(0.5, Color32::TRANSPARENT), (1.0, theme::black(0.4))]));
    let x = r.left() + 36.0;
    let text_w = (r.width() * 0.6).max(320.0).min(r.width() - 72.0);
    let ph = card_phase(c, now);
    let eyebrow = if ph == Phase::Live { tr("Happening now on Orbiters") } else { tr("Next on Orbiters") };
    widgets::text(&p, pos2(x, r.top() + 36.0), Align2::LEFT_CENTER, &Txt::caps(eyebrow).color(theme::white(0.7)));
    let g = p.layout_job({
        let mut job = Txt::new(&c.title, if r.width() < 700.0 { 28.0 } else { 36.0 })
            .weight(weight::BOLD)
            .tracking(-0.025)
            .color(Color32::WHITE)
            .line_height(if r.width() < 700.0 { 32.0 } else { 41.0 })
            .job(text_w);
        job.wrap.max_rows = 2;
        job
    });
    let title_top = r.top() + 56.0 + 36.0;
    status_pill(&p, ui, pos2(x, r.top() + 56.0), ph, false);
    let gh = g.size().y;
    p.galley(pos2(x, title_top), g, Color32::WHITE);
    let mut y = title_top + gh + 18.0;
    widgets::icon(&p, pos2(x + 7.0, y), icons::CALENDAR_BLANK, 14.0, theme::white(0.75));
    widgets::text_truncated(
        &p,
        pos2(x + 22.0, y),
        Align2::LEFT_CENTER,
        &Txt::new(when(c.starts_at.as_deref(), c.ends_at.as_deref()), 13.5).weight(weight::MEDIUM).color(theme::white(0.9)),
        text_w - 22.0,
    );
    y += 24.0;
    let host = c.community.as_ref().or(c.discord.as_ref());
    let mut fx = x;
    if let Some(h) = host {
        let ar = Rect::from_center_size(pos2(fx + 9.0, y), vec2(18.0, 18.0));
        widgets::avatar(&p, ar, None, &h.name);
        let t = Txt::new(&h.name, 13.0).color(theme::white(0.8));
        let by = widgets::text_truncated(&p, pos2(ar.right() + 7.0, y), Align2::LEFT_CENTER, &t, text_w * 0.5);
        fx = by.right() + 16.0;
    }
    if let Some(w) = c.world.as_ref().and_then(|w| w.name.as_ref()) {
        widgets::icon(&p, pos2(fx + 7.0, y), icons::GLOBE, 13.0, theme::white(0.7));
        widgets::text_truncated(&p, pos2(fx + 21.0, y), Align2::LEFT_CENTER, &Txt::new(w, 13.0).color(theme::white(0.8)), (x + text_w - fx - 21.0).max(40.0));
    }
    // Join, bottom left (the whole hero opens the event too).
    let b = Button::new(if signed { tr("On the invite list") } else { tr("Join") })
        .icon(if signed { icons::CHECK } else { icons::ARROW_RIGHT })
        .variant(if signed { Variant::Glass } else { Variant::Primary })
        .size(Size::Md)
        .id_salt(id.with("join"));
    let bw = b.desired_width(ui);
    let br = Rect::from_min_size(pos2(x, r.bottom() - 36.0 - 40.0), vec2(bw.max(120.0), 40.0));
    let mut bu = ui.new_child(UiBuilder::new().max_rect(br.expand(4.0)).id_salt(id.with("join-ui")));
    if !interactive {
        bu.disable();
    }
    let join = sharing::pressed(b.width(bw.max(120.0)).show_at(&mut bu, br));
    let _ = cx;
    interactive && (join || widgets::pressed(&resp))
}

/// One event: its cover with its status and category, the date tile, the title, when, who hosts it and where, and
/// Join. Returns its cover's rect when pressed.
#[allow(clippy::too_many_arguments)]
fn event_card(
    ui: &mut Ui,
    cx: &Cx,
    i: usize,
    c: &Card,
    rect: Rect,
    tex: Option<&TextureHandle>,
    shown: f32,
    interactive: bool,
    signed: bool,
    now: i64,
) -> Option<Rect> {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("event-card", &c.id));
    let resp = ui.interact(rect, id, if interactive { Sense::click() } else { Sense::hover() });
    let resp = if interactive { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let delay = 0.08 + i.min(12) as f64 * 0.035;
    let elapsed = anim::time(&ctx) - cx.entered;
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
    }
    let enter = anim::spring_from(&ctx, id.with(("enter", cx.visit)), [0.0], [if elapsed >= delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() && interactive, Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on() && interactive, Spring::SNAPPY);
    let thumb0 = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 9.0 / 16.0));
    let landing = morph::bounce(&ctx, thumb0);
    let scale = (0.97 + 0.03 * enter) * (1.0 - 0.03 * press) * landing;
    let r = Rect::from_center_size(rect.center() + vec2(0.0, 20.0 * (1.0 - enter) - 4.0 * hover), rect.size() * scale);
    let alpha = enter.clamp(0.0, 1.0) * shown;
    let mut cu = ui.new_child(UiBuilder::new().max_rect(r.expand(4.0)).id_salt(id.with("ui")));
    cu.multiply_opacity(alpha);
    if !interactive {
        cu.disable();
    }
    let p = cu.painter().clone();
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover));
    let thumb = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0));
    paint_cover(&p, thumb, [20.0, 20.0, 0.0, 0.0], tex, &c.id, 1.0 + 0.06 * ease::out_cubic(hover), 1.0);
    let shade = paint::rounded_rect_points_corners(thumb, [theme::squircle(20.0), theme::squircle(20.0), 0.0, 0.0]);
    paint::fill_gradient(
        &p,
        &shade,
        &Gradient::vertical(thumb, &[(0.0, theme::black(0.3)), (0.35, Color32::TRANSPARENT), (0.7, Color32::TRANSPARENT), (1.0, theme::black(0.45))]),
    );
    status_pill(&p, &cu, pos2(thumb.left() + 12.0, thumb.top() + 12.0), card_phase(c, now), true);
    if let Some(cat) = c.category.as_deref() {
        super::sessions::badge(&p, &cu, pos2(thumb.right() - 12.0, thumb.top() + 13.0), &category_label(cat), theme::black(0.55), theme::FG_2, true);
    }
    // Date tile, title, when.
    let tile = Rect::from_min_size(pos2(r.left() + 14.0, thumb.bottom() + 14.0), vec2(46.0, 50.0));
    date_tile(&p, tile, c.starts_at.as_deref());
    let tx = tile.right() + 12.0;
    let tw = r.right() - 14.0 - tx;
    let g = p.layout_job({
        let mut job = Txt::new(&c.title, 14.5).weight(weight::SEMIBOLD).line_height(19.0).job(tw);
        job.wrap.max_rows = 2;
        job
    });
    let gh = g.size().y;
    p.galley(pos2(tx, tile.top() + 1.0), g, theme::FG);
    let times = match (parse(c.starts_at.as_deref()).map(local), parse(c.ends_at.as_deref()).map(local)) {
        (Some(a), Some(b)) => format!("{} – {}", clock(a), clock(b)),
        (Some(a), None) => clock(a),
        _ => tr("Time to be announced").into(),
    };
    widgets::text_truncated(&p, pos2(tx, tile.top() + gh + 12.0), Align2::LEFT_CENTER, &Txt::new(times, 12.0).color(theme::FG_3), tw);
    // Host and world, then Join.
    let line_y = r.bottom() - 26.0;
    let b = Button::new(if signed { tr("On the list") } else { tr("Join") })
        .icon(if signed { icons::CHECK } else { icons::ARROW_RIGHT })
        .variant(if signed { Variant::Secondary } else { Variant::Primary })
        .size(Size::Sm)
        .id_salt(id.with("join"));
    let bw = b.desired_width(&cu);
    let br = Rect::from_min_size(pos2(r.right() - 14.0 - bw, line_y - 16.0), vec2(bw, 32.0));
    let join = sharing::pressed(b.show_at(&mut cu, br));
    let mut fx = r.left() + 16.0;
    let room = br.left() - 12.0;
    if let Some(h) = c.community.as_ref().or(c.discord.as_ref()) {
        let ar = Rect::from_center_size(pos2(fx + 9.0, line_y), vec2(18.0, 18.0));
        widgets::avatar(&p, ar, None, &h.name);
        let has_world = c.world.as_ref().and_then(|w| w.name.as_ref()).is_some();
        let by = widgets::text_truncated(
            &p,
            pos2(ar.right() + 7.0, line_y),
            Align2::LEFT_CENTER,
            &Txt::new(&h.name, 12.0).weight(weight::MEDIUM).color(theme::FG_2),
            if has_world { (room - ar.right() - 7.0) * 0.5 } else { room - ar.right() - 7.0 },
        );
        fx = by.right() + 12.0;
    }
    if let Some(w) = c.world.as_ref().and_then(|w| w.name.as_ref()) {
        if room - fx > 40.0 {
            widgets::icon(&p, pos2(fx + 6.0, line_y), icons::GLOBE, 12.0, theme::FG_4);
            widgets::text_truncated(&p, pos2(fx + 18.0, line_y), Align2::LEFT_CENTER, &Txt::new(w, 12.0).color(theme::FG_3), room - fx - 18.0);
        }
    }
    (interactive && (join || widgets::pressed(&resp))).then_some(thumb)
}

fn empty_state(ui: &mut Ui, cx: &Cx, searching: bool, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(260.0)), Sense::hover());
    let (rect, alpha) = super::entering(ui, cx, 1, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let t = anim::clock(ui.ctx(), 30.0);
    let bob = -8.0 * ease::in_out(ease::mirror(t, 2.0));
    let lr = Rect::from_center_size(rect.center() + vec2(0.0, -44.0 + bob), logo::size_for_width(70.0));
    logo::paint(&p, lr, logo::Look::default());
    let (title, text) = if searching {
        (tr("No event matches"), tr("Try another name, community or world."))
    } else {
        (tr("Nothing on right now"), tr("Events communities publish on Orbiters show up here, soonest first, with the ones under way."))
    };
    widgets::text(&p, rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 13.0).color(theme::FG_3).line_height(20.0).job(400.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y + 34.0), g, theme::FG_3);
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_761_768_000; // 2025-10-29T20:00:00Z

    fn iso(t: i64) -> String {
        OffsetDateTime::from_unix_timestamp(t).unwrap().format(&time::format_description::well_known::Rfc3339).unwrap()
    }

    #[test]
    fn phases_follow_the_website() {
        let (s, e) = (iso(T), iso(T + 7200));
        assert_eq!(phase(Some("published"), Some(&s), Some(&e), false, T - 8100), Phase::Soon(8100));
        assert_eq!(phase_label(Phase::Soon(8100)), "Starts in 2 h 15 min");
        assert_eq!(phase_label(Phase::Soon(3 * 86400 + 4 * 3600)), "Starts in 3 d 4 h");
        assert_eq!(phase_label(Phase::Soon(20)), "Starts in 1 min");
        assert_eq!(phase(Some("published"), Some(&s), Some(&e), false, T + 60), Phase::Live);
        assert_eq!(phase(Some("published"), Some(&s), Some(&e), true, T - 600), Phase::Live, "a step's instance open early");
        assert_eq!(phase(Some("published"), Some(&s), Some(&e), false, T + 7200), Phase::Ended);
        assert_eq!(phase(Some("cancelled"), Some(&s), Some(&e), true, T), Phase::Cancelled);
        assert_eq!(phase(Some("ended"), Some(&s), Some(&e), false, T - 10), Phase::Ended);
        assert_eq!(phase(None, None, None, false, T), Phase::Tba);
    }

    #[test]
    fn invite_answers_become_what_the_window_says() {
        assert_eq!(Ask::of_status("pending", None), Ask::Listed { friend: None });
        assert_eq!(Ask::of_status("sent", None), Ask::Sent("Invite sent"));
        assert_eq!(Ask::of_status("uncertain", None), Ask::Sent("Invite on its way"));
        assert_eq!(Ask::of_status("failed", Some(&"not_friends".to_string())), Ask::Failed(Some("not_friends".into())));
        assert_eq!(Ask::of_status("withdrawn", None), Ask::Idle);
        let link = Refused { status: 409, code: Some("VRCHAT_LINK_REQUIRED".into()), message: "Connect VRChat".into() };
        assert_eq!(Ask::of_refusal(&link), Ask::Link);
        let group = Refused { status: 409, code: Some("VRCHAT_GROUP_REQUIRED".into()), message: "Join".into() };
        assert_eq!(Ask::of_refusal(&group), Ask::Group);
        assert_eq!(Ask::of_refusal(&Refused { status: 401, code: None, message: "x".into() }), Ask::SignIn);
        let closed = Refused { status: 409, code: None, message: "Invite sign-ups for this step have closed.".into() };
        assert_eq!(Ask::of_refusal(&closed), Ask::Error("Invite sign-ups for this step have closed.".into()));
        let befriend = Signup { status: "pending".into(), friend: Some(Friend { is_friend: Some(false), accepting: false, account: None }) };
        let ask = Ask::of_signup(befriend);
        let (tone, text, _) = ask_note(&ask, None).unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(text.starts_with("Add the community"), "{text}");
        assert!(ask.signed_up());
    }

    #[test]
    fn times_read_in_words() {
        assert_eq!(countdown(59 * 60 + 40), "1 h 0 min");
        assert_eq!(when_at(None, None, T), "Time to be announced");
        // The same day, then over midnight into the early morning: one evening.
        let s = iso(T);
        let local_start = local(OffsetDateTime::from_unix_timestamp(T).unwrap());
        let same = when_at(Some(&s), Some(&iso(T + 3600)), T - 86400 * 10);
        assert!(
            same.ends_with(&format!("{} – {}", clock(local_start), clock(local(OffsetDateTime::from_unix_timestamp(T + 3600).unwrap()))))
                || same.contains(" – "),
            "{same}"
        );
        assert!(when_at(Some(&s), Some(&iso(T + 3600)), T).starts_with("Today"));
        assert!(when_at(Some(&iso(T + 86400)), None, T).starts_with("Tomorrow"));
        assert_eq!(category_label("film_media"), "Film & media");
        assert_eq!(category_label("hangout"), "Hangout");
    }

    /// Dates, countdowns and where an event stands are said in the language of the moment (no English day or month in a Japanese
    /// date), and what the code reads, the phase and the answers of the server, is the same in every language.
    #[test]
    fn dates_and_phases_speak_the_language() {
        use orbiters_i18n::{in_language, Language};
        let s = iso(T);
        in_language(Language::Ja, || {
            assert_eq!(phase_label(Phase::Soon(3 * 86400 + 4 * 3600)), "3日4時間後に開始");
            assert_eq!(phase_label(Phase::Live), "開催中");
            assert_eq!(when_at(None, None, T), "時間は未定");
            let t = local(OffsetDateTime::from_unix_timestamp(T).unwrap());
            let date = date_short(t, t);
            assert!(date.contains('月') && date.ends_with('）'), "{date}");
        });
        in_language(Language::Fr, || {
            assert_eq!(countdown(8100), "2 h 15 min");
            assert_eq!(phase_label(Phase::Cancelled), "Annulé");
            assert!(when_at(Some(&s), None, T).starts_with("Aujourd’hui"), "{}", when_at(Some(&s), None, T));
            assert_eq!(category_label("music"), "Musique");
        });
        in_language(Language::Es, || {
            assert_eq!(phase(Some("cancelled"), None, None, false, T), Phase::Cancelled);
            // The state keeps its English; what draws it says it in Spanish.
            let Ask::Sent(label) = Ask::of_status("sent", None) else { panic!("sent") };
            assert_eq!(label, "Invite sent");
            assert_eq!(tr(label), "Invitación enviada");
        });
        in_language(Language::PtBr, || assert_eq!(phase_label(Phase::Ended), "Encerrado"));
    }

    #[test]
    fn markdown_reads_as_plain_text() {
        assert_eq!(plain("## Rules\n- Be **kind**\n- See [the page](https://orbiters.cc/x)"), "Rules\n• Be kind\n• See the page");
    }

    #[test]
    fn events_group_into_now_this_week_and_later() {
        let card = |s: i64| Card { id: s.to_string(), starts_at: Some(iso(s)), ends_at: Some(iso(s + 3600)), ..Default::default() };
        let cards = [card(T - 600), card(T - 300), card(T + 86400), card(T + 86400 + 60), card(T + 3 * 86400)];
        let refs: Vec<&Card> = cards.iter().collect();
        let groups = period_groups(&refs, T);
        assert_eq!(groups, vec![(Period::Now, vec![0, 1]), (Period::Week, vec![2, 3, 4])]);
        let far = [card(T + 9 * 86400), card(T + 3600)];
        let refs: Vec<&Card> = far.iter().collect();
        assert_eq!(period_groups(&refs, T), vec![(Period::Week, vec![1]), (Period::Later, vec![0])]);
    }
}
