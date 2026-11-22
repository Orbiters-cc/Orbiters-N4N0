//! "Orbiters - N4N0 will minimize to save memory" (LIGHT-DESKTOP-8, Enzo's decision of 2026-11-19): a window left
//! minimised for long while VRChat runs closes to the tray, as its close button does (the service, the tray, the SteamVR
//! overlay and whatever runs keep running), and gives back what it holds (measured on Enzo's PC: 146 MB of graphics
//! memory and 387 MB of commit). It never does so unseen: an 8-second countdown with Keep open comes first, where the
//! person is: in VR, the band before the eyes that "Talk with N4N0?" uses (`overlay::floating::prompt`), pressed with a
//! laser; on the desktop, a card of its own in the corner of the screen, over the game, that never takes the focus (a
//! message inside the minimised window would be seen by nobody). Keep open keeps the window for the rest of the session;
//! Settings has a switch to turn it off for good. Opened again from the tray, the window comes back on its page.
//!
//! When it offers ([`Rest::step`]), all of these at once:
//! - minimised for [`MINIMIZED_FOR`] without a break (restored in between: the time starts again, and an offer goes);
//! - VRChat runs on this computer;
//! - nothing in the window is at work ([`Facts::working`], the app's [`crate::app::App`] says: an upload, a reconstruction,
//!   a recording, a clean-up, a share, a call or Talk, a page still reading, any window, dialog, editor or viewer open on
//!   a page, a menu open);
//! - the person can see the offer: in VR (SteamVR with its overlay, the headset worn), or on the desktop when no game
//!   holds the screen in exclusive full screen;
//! - the switch is on and Keep open was not pressed since the window opened.
//! In VR the countdown starts only once the band says it shows (else the offer goes, and comes again later).

use crate::state::RestOffer;
use egui::{pos2, vec2, Align2, Painter, Rect, Vec2};
use orbiters_i18n::{mark, tr};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{n4n0_logo, widgets};

/// How long the window stays minimised, while VRChat runs, before it offers to close: long enough that a window put
/// down for a moment (a quick look at the game, a call) never goes, short enough to give the memory back within a
/// session.
pub const MINIMIZED_FOR: f64 = 15.0 * 60.0;
/// The countdown, Enzo's eight seconds.
pub const COUNTDOWN: f64 = 8.0;
/// The VR band says it shows the offer within this long, or the offer goes (it was not seen).
pub const SEEN_WITHIN: f64 = 4.0;
/// An offer that went for a reason other than Keep open comes again no sooner than this.
pub const AGAIN_AFTER: f64 = 120.0;
/// The most one look credits to the countdown, seen or not: the window looks ten times a second while it runs, and a
/// window stalled for longer (a busy machine, a frozen frame) never counts the gap as seen (verify2, SWEEP-7).
pub const MAX_CREDIT: f64 = 0.5;
/// While a desktop countdown is paused because the card cannot show (a game in exclusive full screen), nothing tells
/// the window when it can again: it looks this often.
pub const PAUSED_LOOK: f64 = 1.0;

/// Where the person sees the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Vr,
    Desktop,
}

/// Where the person would see the offer now: in VR when the headset is worn and N4N0's SteamVR overlay is up (its band);
/// on the desktop when no headset can be in use (SteamVR not running, or the overlay says the headset is not worn) and a
/// card can be drawn over the screen; else nowhere. A worn headset with no overlay, or SteamVR running with nobody to
/// say whether the headset is worn, is nowhere: the desktop card would be on a monitor nobody looks at (SWEEP-7).
pub fn where_seen(steamvr: bool, overlay: bool, headset_in_use: Option<bool>, card_can_show: bool) -> Option<Where> {
    if steamvr {
        return match headset_in_use {
            Some(true) => overlay.then_some(Where::Vr),
            Some(false) => card_can_show.then_some(Where::Desktop),
            None => None,
        };
    }
    card_can_show.then_some(Where::Desktop)
}

/// What the app tells the countdown at each look.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    pub minimized: bool,
    pub vrchat: bool,
    /// Where the person would see it now (`None`: nowhere: the countdown pauses).
    pub seen_at: Option<Where>,
    /// Something of the window is at work or open (`App::idle_facts`): no offer, and one up goes.
    pub working: bool,
    /// Settings' switch.
    pub on: bool,
    /// What the VR band says now: the offer it shows this moment (a live fact, not a latch: hidden by SteamVR's
    /// dashboard or by "Talk with N4N0?", it shows none), and the one Keep open was pressed on.
    pub shown_in_vr: Option<u64>,
    pub kept_in_vr: Option<u64>,
    /// The desktop card confirms the offer up is on the screen now (made, its picture taken by Windows, visible): the
    /// only ground for counting on the desktop; a card that fails or cannot be confirmed is no countdown (verify2).
    pub card_shows: bool,
    /// Keep open, pressed on the desktop card and read just before this look: it wins over a close of this same look.
    pub kept_on_desktop: bool,
}

/// What to do after a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Put up this offer there, or again with its countdown running or paused.
    Show(RestOffer, Where),
    /// Take the offer down.
    Withdraw,
    /// The countdown is over, every second of it seen: close the window to the tray.
    Close,
}

struct Up {
    id: u64,
    at: Where,
    offered: f64,
    /// It was seen at least once (the VR band or the desktop card confirmed it on show).
    seen: bool,
    /// Seconds of the countdown left, counted only while it is seen.
    left: f64,
    /// While it is seen: when the last look was (the time since counts). `None`: paused.
    ticking: Option<f64>,
}

/// The countdown of the window ([`Rest::step`], with a clock given in seconds).
pub struct Rest {
    since: Option<f64>,
    up: Option<Up>,
    kept: bool,
    /// Offers are numbered from this window's start time (ms), so that a later window's never repeats one the SteamVR
    /// overlay (which outlives windows) already took or saw pressed (night3, SWEEP-8).
    base: u64,
    last_id: u64,
    wait_until: f64,
    /// Settings' switch at the last look.
    on: bool,
}

impl Default for Rest {
    fn default() -> Self {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        Self { since: None, up: None, kept: false, base: (ms & ((1 << 47) - 1)) << 8, last_id: 0, wait_until: 0.0, on: true }
    }
}

impl Rest {
    /// Keep open, pressed (on the desktop card; the VR band's comes as [`Facts::kept_in_vr`]): the window stays for the
    /// rest of the session.
    pub fn keep(&mut self) {
        self.kept = true;
    }

    pub fn kept(&self) -> bool {
        self.kept
    }

    /// The offer up and the clock it runs on (`now`, `now_ms`: the same moment in seconds and in Unix ms). Its
    /// `until_ms` is 0 while it is paused (not seen); `left_ms` then says what is left.
    pub fn offer(&self, now: f64, now_ms: u64) -> Option<(RestOffer, Where)> {
        let up = self.up.as_ref()?;
        let for_ms = (COUNTDOWN * 1000.0) as u64;
        let left = (up.left - up.ticking.map_or(0.0, |t| (now - t).clamp(0.0, MAX_CREDIT))).max(0.0);
        let left_ms = (left * 1000.0) as u64;
        let until_ms = if up.ticking.is_some() { now_ms + left_ms } else { 0 };
        Some((RestOffer { id: up.id, until_ms, for_ms, left_ms }, up.at))
    }

    /// Whether the countdown runs now (its message seen).
    pub fn running(&self) -> bool {
        self.up.as_ref().is_some_and(|u| u.ticking.is_some())
    }

    /// When the next look is needed, in seconds from `now` (None: only when something changes: a new snapshot, a job
    /// ending, the window restored all wake the window by themselves). Never twice a second for ever (SWEEP-9).
    pub fn next_look(&self, now: f64) -> Option<f64> {
        if self.kept || !self.on {
            return None;
        }
        let t = match &self.up {
            // The countdown moves (its line, its seconds).
            Some(up) if up.ticking.is_some() => 0.1,
            // Offered, the band not showing it yet: until it is too late for it.
            Some(up) if !up.seen => (up.offered + SEEN_WITHIN - now).max(0.1),
            // Paused on the desktop (a game in exclusive full screen, a card Windows refused): nothing says when it
            // can show again, so a look now and then.
            Some(up) if up.at == Where::Desktop => PAUSED_LOOK,
            // Paused in VR: whatever shows it again comes with a snapshot.
            Some(_) => return None,
            None => {
                let at = (self.since? + MINIMIZED_FOR).max(self.wait_until) - now;
                if at <= 0.0 {
                    return None;
                }
                at
            }
        };
        Some(t.min(3600.0))
    }

    fn withdraw(&mut self) -> Step {
        if self.up.take().is_some() {
            Step::Withdraw
        } else {
            Step::Nothing
        }
    }

    fn shown(&self, now: f64, now_ms: u64) -> Step {
        self.offer(now, now_ms).map_or(Step::Nothing, |(o, w)| Step::Show(o, w))
    }

    /// One look at `now` (seconds, any clock that does not go back) with the app's `facts`.
    pub fn step(&mut self, now: f64, now_ms: u64, facts: &Facts) -> Step {
        self.on = facts.on;
        if !facts.minimized {
            // Restored: the time starts again.
            self.since = None;
            return self.withdraw();
        }
        let since = *self.since.get_or_insert(now);
        if let (Some(kept), Some(up)) = (facts.kept_in_vr, self.up.as_ref()) {
            if kept == up.id {
                self.kept = true;
            }
        }
        // Read before anything below can close: a press arriving before the close happens wins (verify2).
        if facts.kept_on_desktop && self.up.is_some() {
            self.kept = true;
        }
        if self.kept || !facts.on {
            return self.withdraw();
        }
        let eligible = facts.vrchat && !facts.working;
        let Some(up) = self.up.as_mut() else {
            let Some(at) = facts.seen_at.filter(|_| eligible) else { return Step::Nothing };
            if now - since < MINIMIZED_FOR || now < self.wait_until {
                return Step::Nothing;
            }
            self.last_id += 1;
            // Seen once what shows it says so: the VR band, or the desktop card confirmed on the screen (verify2: being
            // able to draw a card is not having drawn it).
            self.up = Some(Up { id: self.base | (self.last_id & 0xff), at, offered: now, seen: false, left: COUNTDOWN, ticking: None });
            return self.shown(now, now_ms);
        };
        if !eligible || facts.seen_at.is_some_and(|w| w != up.at) {
            // Something started, VRChat ended, the person went elsewhere: not now.
            self.wait_until = now + AGAIN_AFTER;
            return self.withdraw();
        }
        // Seen now? Only while what shows it confirms it shows this offer.
        let seen_now = match up.at {
            Where::Desktop => facts.seen_at == Some(Where::Desktop) && facts.card_shows,
            Where::Vr => facts.seen_at == Some(Where::Vr) && facts.shown_in_vr == Some(up.id),
        };
        if !up.seen && !seen_now {
            if now - up.offered > SEEN_WITHIN {
                // Never seen: it goes, unseen, and nothing closes.
                self.wait_until = now + AGAIN_AFTER;
                return self.withdraw();
            }
            return Step::Nothing;
        }
        match (seen_now, up.ticking) {
            // Seen and running: count.
            (true, Some(t)) => {
                up.left -= (now - t).clamp(0.0, MAX_CREDIT);
                up.ticking = Some(now);
                if up.left <= 0.0 {
                    self.up = None;
                    // Closing: nothing more is offered by this window.
                    self.wait_until = f64::INFINITY;
                    return Step::Close;
                }
                Step::Nothing
            }
            // Seen again (or for the first time): it runs from where it stood.
            (true, None) => {
                up.seen = true;
                up.ticking = Some(now);
                self.shown(now, now_ms)
            }
            // Hidden (SteamVR's dashboard, "Talk with N4N0?" on the band, a game in exclusive full screen): it pauses.
            (false, Some(t)) => {
                up.left -= (now - t).clamp(0.0, MAX_CREDIT);
                up.ticking = None;
                self.shown(now, now_ms)
            }
            (false, None) => Step::Nothing,
        }
    }
}

/// The desktop card's size, in points.
pub const CARD: Vec2 = vec2(420.0, 136.0);

/// Its words, in English (the catalog area `windowrest` has them in every language).
const TITLE: &str = mark("Orbiters - N4N0 will minimize to save memory");
const KEEP: &str = mark("Keep open");
pub(crate) const SETTING: &str = mark("Close the minimized window while playing");
pub(crate) const SETTING_HINT: &str = mark("After 15 minutes minimized while VRChat runs, Orbiters - N4N0 asks for 8 seconds, then closes its window to the tray to give its memory back. Open it again from the tray: it comes back on its page.");

fn title() -> &'static str {
    tr(TITLE)
}

fn line(seconds: u64) -> String {
    orbiters_i18n::tr!("In {seconds} s, to the tray: everything keeps running there.", seconds = seconds)
}

fn keep() -> &'static str {
    tr(KEEP)
}

/// Settings' switch, its line and its hint.
pub fn setting_label() -> &'static str {
    tr(SETTING)
}

pub fn setting_hint() -> &'static str {
    tr(SETTING_HINT)
}

/// Where the desktop card's Keep open is, in points of the card.
pub fn card_button() -> Rect {
    Rect::from_min_size(pos2(70.0, 82.0), vec2(124.0, 34.0))
}

/// The desktop card (`scale`: pixels a point): N4N0's mark, the words, Keep open under them, the line of time left.
/// `hover`: the pointer on Keep open.
pub fn paint_card(p: &Painter, scale: f32, hover: bool, left: f32, seconds: u64) {
    let s = scale;
    let card = Rect::from_min_size(pos2(0.0, 0.0), CARD * s).shrink(2.0 * s);
    let radius = 18.0 * s;
    p.rect_filled(card, radius, theme::alpha(theme::INK_950, 0.96));
    p.rect_stroke(card, radius, egui::Stroke::new(1.5 * s, theme::white(0.14)), egui::StrokeKind::Inside);
    let mark = Rect::from_center_size(pos2(card.left() + 38.0 * s, card.top() + 38.0 * s), n4n0_logo::size_for_width(32.0 * s));
    n4n0_logo::paint(p, mark, theme::MARK);
    let words = card.left() + 68.0 * s;
    let room = card.right() - 18.0 * s - words;
    let t = title();
    let size = crate::mismatch::fitting(p, t, &[15.5 * s, 14.5 * s, 13.5 * s, 12.5 * s], room);
    widgets::text_truncated(p, pos2(words, card.top() + 30.0 * s), Align2::LEFT_CENTER, &Txt::new(t, size).weight(weight::SEMIBOLD), room + 2.0 * s);
    let l = line(seconds);
    let size = crate::mismatch::fitting(p, &l, &[12.5 * s, 12.0 * s, 11.5 * s, 11.0 * s], room);
    widgets::text_truncated(p, pos2(words, card.top() + 52.0 * s), Align2::LEFT_CENTER, &Txt::new(&l, size).color(theme::FG_3), room + 2.0 * s);
    // Keep open, under the words: a secondary pill, lit under the pointer.
    let b = card_button();
    let pill = Rect::from_min_size(card.min + b.min.to_vec2() * s - vec2(2.0, 2.0) * s, b.size() * s);
    let k = keep();
    let size = crate::mismatch::fitting(p, k, &[13.0 * s, 12.0 * s, 11.0 * s], pill.width() - 24.0 * s);
    p.rect_filled(pill, pill.height() * 0.5, if hover { theme::white(0.16) } else { theme::white(0.09) });
    p.rect_stroke(pill, pill.height() * 0.5, egui::Stroke::new(1.0 * s, theme::white(if hover { 0.28 } else { 0.14 })), egui::StrokeKind::Inside);
    widgets::text_truncated(p, pill.center(), Align2::CENTER_CENTER, &Txt::new(k, size).weight(weight::SEMIBOLD), pill.width() - 20.0 * s);
    // Time left, along its bottom edge, as the window's toasts.
    let track = Rect::from_min_max(pos2(card.left() + radius, card.bottom() - 7.0 * s), pos2(card.right() - radius, card.bottom() - 4.0 * s));
    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), 2.0 * s, theme::white(0.35));
}

/// The VR band's face ([`crate::wake_offer::VR_PX`], the band "Talk with N4N0?" uses): N4N0's mark, the words across
/// the band, under them the seconds left and a Keep open pill (the whole band presses it), the line of time left.
/// `hover`: a laser is on it.
pub fn paint_band(p: &Painter, hover: bool, left: f32, seconds: u64) {
    use crate::wake_offer::VR_PX;
    let all = Rect::from_min_size(pos2(0.0, 0.0), vec2(VR_PX[0] as f32, VR_PX[1] as f32));
    let band = all.shrink(6.0);
    let radius = 36.0;
    let fill = if hover { theme::mix(theme::INK_950, theme::FG, 0.14) } else { theme::alpha(theme::INK_950, 0.94) };
    p.rect_filled(band, radius, fill);
    p.rect_stroke(band, radius, egui::Stroke::new(3.0, if hover { theme::ACCENT } else { theme::white(0.16) }), egui::StrokeKind::Inside);
    let mark = Rect::from_center_size(pos2(band.left() + 30.0 + 26.0, band.center().y - 2.0), n4n0_logo::size_for_width(52.0));
    n4n0_logo::paint(p, mark, theme::MARK);
    let words = mark.right() + 22.0;
    // The words, whole, across the band (the size comes down until they fit).
    let room = band.right() - 30.0 - words;
    let t = title();
    let size = crate::mismatch::fitting(p, t, &[28.0, 26.0, 24.0, 22.0, 20.0, 18.0], room);
    widgets::text_truncated(p, pos2(words, band.top() + 44.0), Align2::LEFT_CENTER, &Txt::new(t, size).weight(weight::SEMIBOLD), room + 4.0);
    // Under them: the seconds left, and Keep open on the right, light (Talk's is green: this one only keeps what is).
    let pill = Rect::from_min_size(pos2(band.right() - 26.0 - 184.0, band.top() + 80.0), vec2(184.0, 54.0));
    p.rect_filled(pill, 27.0, if hover { theme::FG } else { theme::mix(theme::INK_950, theme::FG, 0.86) });
    let k = keep();
    let size = crate::mismatch::fitting(p, k, &[24.0, 22.0, 20.0, 18.0], pill.width() - 28.0);
    widgets::text_truncated(p, pill.center(), Align2::CENTER_CENTER, &Txt::new(k, size).weight(weight::SEMIBOLD).color(theme::INK_950), pill.width() - 24.0);
    let soon = orbiters_i18n::tr!("In {seconds} s", seconds = seconds);
    widgets::text_truncated(p, pos2(words, pill.center().y), Align2::LEFT_CENTER, &Txt::new(&soon, 24.0).color(theme::FG_3), pill.left() - 18.0 - words);
    let track = Rect::from_min_max(pos2(band.left() + radius, band.bottom() - 9.0), pos2(band.right() - radius, band.bottom() - 5.0));
    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), 2.0, theme::white(0.35));
}

/// The page the window was on when it closed to the tray, for the next window to open on: a small file of its own,
/// read once by the next window (opened from the tray, a second launch, a menu) within a day.
pub mod resume {
    use std::path::PathBuf;

    const KEEP_FOR_S: u64 = 24 * 3600;

    fn file() -> PathBuf {
        orbiters_core::paths::cache_dir().join("window-resume.txt")
    }

    fn now_s() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    pub fn save(page: &str) {
        let f = file();
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, format!("{} {page}", now_s()));
    }

    /// The page saved, once (the file goes), when it is recent.
    pub fn take() -> Option<String> {
        let f = file();
        let text = std::fs::read_to_string(&f).ok()?;
        let _ = std::fs::remove_file(&f);
        parse(&text, now_s())
    }

    pub(super) fn parse(text: &str, now: u64) -> Option<String> {
        let (at, page) = text.trim().split_once(' ')?;
        let at: u64 = at.parse().ok()?;
        (now.saturating_sub(at) <= KEEP_FOR_S && !page.is_empty()).then(|| page.to_string())
    }
}

/// The desktop card: a window of its own over everything (the game too), in the corner of the screen the game is on,
/// that never takes the focus; only its Keep open takes a click. Painted on the CPU as the panels around VRChat are.
#[cfg(windows)]
pub mod card {
    use super::{paint_card, CARD};
    use crate::overlay::orbit::desktop::{take_presses, PanelWindow};
    use crate::overlay::Canvas;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

    /// Whether a card over the screen can be seen now: not while a game holds it in exclusive full screen (VRChat runs
    /// in a borderless window unless told otherwise), and not when Windows cannot say (verify2: a failed query is no
    /// proof it can be seen).
    pub fn can_show() -> bool {
        use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
        // SAFETY: a plain query.
        unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s != QUNS_RUNNING_D3D_FULL_SCREEN)
    }

    #[derive(Default)]
    pub struct Card {
        shown: Option<(PanelWindow, Canvas, f32, POINT)>,
        drawn: Option<(bool, u64, u32, orbiters_i18n::Language)>,
        /// The offer whose picture Windows took on the card (`None`: none yet, or refused).
        presented: Option<u64>,
        /// Keep open's place on the screen, in pixels, while the card is up.
        pill: Option<egui::Rect>,
    }

    impl Card {
        /// The offer the card is confirmed to show this moment: its picture taken by Windows and the window visible
        /// (verify2: the countdown runs on nothing less).
        pub fn shows(&self) -> Option<u64> {
            let (window, ..) = self.shown.as_ref()?;
            self.presented.filter(|_| window.presented())
        }

        /// Keep open pressed (down and up on the card, released over the button) since the last call: read just before
        /// the countdown's look, so that a press arriving before the close wins.
        pub fn take_keep(&mut self) -> bool {
            let presses = take_presses();
            let (Some((window, ..)), Some(pill)) = (self.shown.as_ref(), self.pill) else { return false };
            let mut cursor = POINT::default();
            // SAFETY: the output is valid.
            let _ = unsafe { GetCursorPos(&mut cursor) };
            let over = pill.contains(egui::pos2(cursor.x as f32, cursor.y as f32));
            let mine = window.id();
            presses.into_iter().any(|(down, on)| on == mine && !down && over)
        }

        /// Puts the offer `id` on the screen (`scale`: pixels a point), its countdown at `left` with `seconds` to go.
        /// Whether Windows took it is what [`Card::shows`] says next.
        pub fn present(&mut self, id: u64, scale: f32, left: f32, seconds: u64) {
            let scale = scale.clamp(1.0, 3.0);
            if self.shown.as_ref().is_some_and(|s| s.2 != scale) || self.presented.is_some_and(|p| p != id) {
                self.hide();
            }
            if self.shown.is_none() {
                let (w, h) = ((CARD.x * scale).round() as u32, (CARD.y * scale).round() as u32);
                let Ok(window) = PanelWindow::new(w, h, true) else { return };
                // The corner of the screen the game is on (the one in front), above its task bar.
                // SAFETY: plain queries.
                let area = unsafe {
                    let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
                    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                    let _ = GetMonitorInfoW(monitor, &mut info);
                    info.rcWork
                };
                let margin = (20.0 * scale) as i32;
                let at = POINT { x: area.right - margin - w as i32, y: area.bottom - margin - h as i32 };
                self.shown = Some((window, Canvas::new(w, h), scale, at));
                self.drawn = None;
                let _ = take_presses();
            }
            let Some((window, canvas, scale, at)) = self.shown.as_mut() else { return };
            let (scale, at) = (*scale, *at);
            let button = super::card_button();
            let pill = egui::Rect::from_min_size(egui::pos2(at.x as f32, at.y as f32) + (button.min.to_vec2() + egui::vec2(0.0, 0.0)) * scale, button.size() * scale);
            let mut cursor = POINT::default();
            // SAFETY: the output is valid.
            let _ = unsafe { GetCursorPos(&mut cursor) };
            let hover = pill.contains(egui::pos2(cursor.x as f32, cursor.y as f32));
            self.pill = Some(pill);
            let look = (hover, seconds, (left.clamp(0.0, 1.0) * 120.0).ceil() as u32, orbiters_i18n::language());
            if self.drawn != Some(look) {
                let first = self.drawn.is_none();
                for _ in 0..if first { 2 } else { 1 } {
                    canvas.paint(0.0, |p| paint_card(p, scale, hover, left, seconds));
                }
                let (w, h) = ((CARD.x * scale).round() as u32, (CARD.y * scale).round() as u32);
                window.picture(canvas.pixels(), [0, 0, w, h]);
                window.place(at, 255);
                if window.presented() {
                    self.drawn = Some(look);
                    self.presented = Some(id);
                } else {
                    // Refused: not on the screen; painted whole again next turn.
                    self.drawn = None;
                    self.presented = None;
                }
            }
        }

        pub fn hide(&mut self) {
            self.shown = None;
            self.drawn = None;
            self.presented = None;
            self.pill = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts { minimized: true, vrchat: true, seen_at: Some(Where::Desktop), working: false, on: true, shown_in_vr: None, kept_in_vr: None, card_shows: true, kept_on_desktop: false }
    }

    /// Steps the countdown from `from` to `to` seconds, a look every 100 ms; what it did, in order (Nothing left out).
    fn run(rest: &mut Rest, from: f64, to: f64, f: &Facts) -> Vec<Step> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to + 1e-9 {
            let s = rest.step(t, (t * 1000.0) as u64, f);
            if s != Step::Nothing {
                out.push(s);
            }
            t += 0.1;
        }
        out
    }

    fn kinds(steps: &[Step]) -> Vec<&'static str> {
        steps.iter().map(|s| match s {
            Step::Nothing => "nothing",
            Step::Show(..) => "show",
            Step::Withdraw => "withdraw",
            Step::Close => "close",
        }).collect()
    }

    /// SWEEP-7: in VR the countdown runs only while the band shows the offer. Hidden mid-way (SteamVR's dashboard,
    /// "Talk with N4N0?" on the band) it pauses, for as long as it stays hidden, and goes on from where it stood once
    /// shown again: the window closes only after eight seconds SEEN, never unseen.
    #[test]
    fn hidden_mid_way_the_countdown_pauses_and_never_closes_unseen() {
        let mut rest = Rest::default();
        let vr = Facts { seen_at: Some(Where::Vr), ..facts() };
        let steps = run(&mut rest, 0.0, MINIMIZED_FOR + 0.05, &vr);
        let Some(Step::Show(offer, Where::Vr)) = steps.first().copied() else { panic!("{steps:?}") };
        let shown = Facts { shown_in_vr: Some(offer.id), ..vr };
        let hidden = Facts { shown_in_vr: None, ..vr };
        let t0 = MINIMIZED_FOR + 1.0;
        run(&mut rest, t0, t0 + 3.0, &shown);
        // Hidden for a minute: no Close, a paused offer saying what is left.
        let steps = run(&mut rest, t0 + 3.1, t0 + 63.0, &hidden);
        assert_eq!(kinds(&steps), ["show"], "paused once, nothing else");
        let Step::Show(paused, _) = steps[0] else { unreachable!() };
        assert_eq!(paused.until_ms, 0, "paused");
        assert!((4_500..=5_100).contains(&paused.left_ms), "about five seconds left: {}", paused.left_ms);
        assert!(!rest.running());
        // Shown again: it goes on from there and closes after the rest of its eight seconds.
        let back = t0 + 63.1;
        let steps = run(&mut rest, back, back + 4.5, &shown);
        assert_eq!(kinds(&steps), ["show"], "running again, not closed yet");
        assert_eq!(kinds(&run(&mut rest, back + 4.6, back + 5.5, &shown)), ["close"], "about eight seconds seen in all");
    }

    /// verify2, SWEEP-7: on the desktop the countdown runs only on a card confirmed on the screen. A card Windows never
    /// made or refused is no countdown and no close; refused mid-way, it pauses; it never closes unseen.
    #[test]
    fn the_desktop_countdown_runs_only_on_a_card_confirmed_on_screen() {
        let mut rest = Rest::default();
        let refused = Facts { card_shows: false, ..facts() };
        let steps = run(&mut rest, 0.0, MINIMIZED_FOR + 30.0, &refused);
        assert_eq!(kinds(&steps), ["show", "withdraw"], "offered, never confirmed, gone: never closed");
        let later = MINIMIZED_FOR + 30.0 + AGAIN_AFTER + 1.0;
        let steps = run(&mut rest, later, later + 3.0, &facts());
        assert_eq!(kinds(&steps), ["show", "show"], "offered, then running once the card confirms");
        let steps = run(&mut rest, later + 3.1, later + 60.0, &refused);
        assert_eq!(kinds(&steps), ["show"], "refused mid-way: paused, not closed");
        assert_eq!(rest.next_look(later + 60.0), Some(PAUSED_LOOK), "paused on the desktop: looked at again by itself");
        assert_eq!(kinds(&run(&mut rest, later + 60.1, later + 64.0, &facts())), ["show"], "about five seconds left");
        assert_eq!(kinds(&run(&mut rest, later + 64.1, later + 66.0, &facts())), ["close"]);
    }

    /// verify2: Keep open pressed just before the close happens wins: it is read before the look that would close.
    #[test]
    fn keep_open_pressed_at_the_last_moment_wins_over_the_close() {
        let mut rest = Rest::default();
        run(&mut rest, 0.0, MINIMIZED_FOR + COUNTDOWN - 0.05, &facts());
        assert!(rest.running(), "a few hundredths of a second left");
        let pressed = Facts { kept_on_desktop: true, ..facts() };
        assert_eq!(rest.step(MINIMIZED_FOR + COUNTDOWN + 1.0, 0, &pressed), Step::Withdraw, "the press, not the close");
        assert!(rest.kept());
    }

    /// verify2: a window stalled between two looks never counts the gap: neither as seen, nor taken off when hidden.
    #[test]
    fn a_stalled_window_never_counts_the_gap_as_seen() {
        let mut rest = Rest::default();
        run(&mut rest, 0.0, MINIMIZED_FOR + 1.0, &facts());
        assert!(rest.running());
        let t = MINIMIZED_FOR + 1.0;
        assert_eq!(rest.step(t + 30.0, 0, &facts()), Step::Nothing, "thirty seconds without a look: not closed");
        let Step::Show(paused, _) = rest.step(t + 60.0, 0, &Facts { card_shows: false, ..facts() }) else { panic!() };
        assert!(paused.left_ms >= 6_000, "at most a look's credit each time: {}", paused.left_ms);
    }

    /// SWEEP-7 (desktop side): with SteamVR running, the desktop card is used only when the overlay says the headset is
    /// not worn; a worn headset without the overlay, or nobody to say, is nowhere (no offer, the window stays).
    #[test]
    fn the_desktop_card_is_never_for_someone_in_the_headset() {
        assert_eq!(where_seen(true, true, Some(true), true), Some(Where::Vr));
        assert_eq!(where_seen(true, false, Some(true), true), None, "in the headset, no overlay: nowhere");
        assert_eq!(where_seen(true, false, None, true), None, "SteamVR runs, nobody says: nowhere");
        assert_eq!(where_seen(true, true, Some(false), true), Some(Where::Desktop));
        assert_eq!(where_seen(false, false, None, true), Some(Where::Desktop));
        assert_eq!(where_seen(false, false, None, false), None, "a game in exclusive full screen");
    }

    /// SWEEP-8: offers of a later window never repeat the numbers of an earlier one (the SteamVR overlay outlives
    /// windows and remembers numbers it showed or saw pressed).
    #[test]
    fn offers_are_numbered_apart_from_every_earlier_window() {
        let first = Rest::default();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let later = Rest::default();
        let id = |mut r: Rest| {
            let steps = run(&mut r, 0.0, MINIMIZED_FOR + 0.05, &facts());
            match steps.first() {
                Some(Step::Show(o, _)) => o.id,
                other => panic!("{other:?}"),
            }
        };
        let (a, b) = (id(first), id(later));
        assert_ne!(a, b, "two windows' first offers");
        assert!(a > 255 && b > 255, "not 1, 2, 3 in every window");
    }

    /// SWEEP-9: once its time is past, a window that cannot offer (switch off, no VRChat, something at work) does not
    /// wake on a clock: whatever changes that wakes it by itself.
    #[test]
    fn a_window_that_cannot_offer_sleeps() {
        for (why, f) in [("switched off", Facts { on: false, ..facts() }), ("no VRChat", Facts { vrchat: false, ..facts() }), ("working", Facts { working: true, ..facts() })] {
            let mut rest = Rest::default();
            run(&mut rest, 0.0, MINIMIZED_FOR + 5.0, &f);
            assert_eq!(rest.next_look(MINIMIZED_FOR + 5.0), None, "{why}");
        }
        let mut rest = Rest::default();
        run(&mut rest, 0.0, 10.0, &facts());
        assert!(rest.next_look(10.0).is_some_and(|t| t > MINIMIZED_FOR - 20.0), "before its time: one wake at it");
        run(&mut rest, 10.1, MINIMIZED_FOR + 1.0, &facts());
        assert_eq!(rest.next_look(MINIMIZED_FOR + 1.0), Some(0.1), "while it counts down");
    }

    /// SWEEP-18: every job of the window counts as work: a page `Job` that runs blocks the close until it ended.
    #[test]
    fn every_job_of_the_window_counts_as_work() {
        let ctx = egui::Context::default();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let mut job = crate::pages::Job::spawn(&ctx, move || {
            let _ = held.recv();
        });
        assert!(crate::pages::working_now() >= 1, "a job running is work of the window");
        drop(release);
        while job.poll().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// SWEEP-18, safe by construction: every thread the window process starts outside a page `Job` is either counted as
    /// work (it holds a `pages::Working`) or listed here as idle-safe (it only reads, keeps a cache that is made again,
    /// or draws in memory), with why. A thread added later and not classified fails this test.
    #[test]
    fn every_thread_the_window_starts_is_counted_or_classified() {
        const IDLE: [(&str, &str); 15] = [
            ("pages/editor/preview.rs", "draws the editor's previews in memory"),
            ("pages/gallery_select.rs", "fetches pictures to hand out by a drag (a cache)"),
            ("pages/projects/builds.rs", "builds project previews (a cache)"),
            ("pages/projects/thumb_cache.rs", "makes project pictures (a cache)"),
            ("pages/projects/unity_session.rs", "brings a Unity window to the front"),
            ("pages/splats/edit3d.rs", "draws photo layers in memory"),
            ("pages/splat_worker.rs", "draws splats in memory"),
            ("pages/videos.rs", "fetches video thumbnails into memory"),
            ("pages/watch.rs", "a page's watch: looks, writes nothing"),
            ("worlds.rs:world-infos", "reads worlds' names into memory"),
            ("worlds.rs:pictures-", "decodes pictures (a cache)"),
            ("media.rs", "asks the music players what plays"),
            ("pages/launcher/destinations.rs:where-{name}", "reads Where's places (the history, events, an instance open) into memory"),
            ("pages/launcher/previous.rs:where-previous-open", "asks whether a previous instance is still open, into memory"),
            ("pages/launcher/previous.rs:where-previous-history", "reads the previous instances (this computer's history) into memory"),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root.join("app.rs"), root.join("window.rs"), root.join("views.rs"), root.join("worlds.rs"), root.join("media.rs"), root.join("window_rest.rs"), root.join("drag_out.rs")];
        let mut dirs = vec![root.join("pages")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        let mut unclassified = Vec::new();
        for file in files.iter().filter(|f| f.exists()) {
            let name = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            let text = std::fs::read_to_string(file).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            // Only the module's own code: its tests start no thread of the window.
            let end = lines.iter().position(|l| l.trim_start().starts_with("mod tests")).unwrap_or(lines.len());
            for (i, line) in lines[..end].iter().enumerate() {
                if !(line.contains("thread::spawn(") || line.contains("thread::Builder::new()")) || line.trim_start().starts_with("//") {
                    continue;
                }
                let around = lines[i.saturating_sub(4)..(i + 4).min(end)].join("\n");
                let counted = around.contains("Working::begin") || around.contains("let _working");
                let idle = IDLE.iter().any(|(at, _)| match at.split_once(':') {
                    Some((f, thread)) => name == f && lines[i..(i + 3).min(end)].join("\n").contains(thread),
                    None => name == *at,
                });
                let job = name == "pages/mod.rs";
                if !(counted || idle || job) {
                    unclassified.push(format!("{name}:{}: {}", i + 1, line.trim()));
                }
            }
        }
        assert!(unclassified.is_empty(), "threads of the window neither counted as work nor classified idle: {unclassified:#?}");
    }

    #[test]
    fn minimized_for_long_while_vrchat_runs_it_counts_down_eight_seconds_then_closes() {
        let mut rest = Rest::default();
        let f = facts();
        assert!(run(&mut rest, 0.0, MINIMIZED_FOR - 0.2, &f).is_empty(), "not before its time");
        let steps = run(&mut rest, MINIMIZED_FOR - 0.1, MINIMIZED_FOR + COUNTDOWN - 0.15, &f);
        // Offered at its time, then running once the card confirms it on the screen (verify2).
        assert_eq!(kinds(&steps), ["show", "show"], "offered once, at its time, then started");
        let Step::Show(offer, at) = steps[0] else { unreachable!() };
        assert_eq!(at, Where::Desktop);
        assert_eq!(offer.for_ms, 8_000);
        assert_eq!(offer.seconds((MINIMIZED_FOR * 1000.0) as u64), 8);
        assert_eq!(kinds(&run(&mut rest, MINIMIZED_FOR + COUNTDOWN + 0.05, MINIMIZED_FOR + COUNTDOWN + 0.35, &f)), ["close"], "after eight seconds seen");
    }

    #[test]
    fn keep_open_keeps_it_for_the_session() {
        let mut rest = Rest::default();
        let f = facts();
        run(&mut rest, 0.0, MINIMIZED_FOR + 1.0, &f);
        rest.keep();
        assert_eq!(kinds(&run(&mut rest, MINIMIZED_FOR + 1.1, MINIMIZED_FOR + 1.2, &f)), ["withdraw"]);
        assert!(run(&mut rest, MINIMIZED_FOR + 1.3, 4.0 * MINIMIZED_FOR, &f).is_empty(), "never again, however long");
        // Restored and minimised again: still kept.
        rest.step(5.0 * MINIMIZED_FOR, 0, &Facts { minimized: false, ..f });
        assert!(run(&mut rest, 5.0 * MINIMIZED_FOR, 7.0 * MINIMIZED_FOR, &f).is_empty());
    }

    #[test]
    fn restored_in_between_the_time_starts_again_and_an_offer_goes() {
        let mut rest = Rest::default();
        let f = facts();
        run(&mut rest, 0.0, MINIMIZED_FOR - 60.0, &f);
        rest.step(MINIMIZED_FOR - 59.0, 0, &Facts { minimized: false, ..f });
        assert!(run(&mut rest, MINIMIZED_FOR - 58.0, 2.0 * MINIMIZED_FOR - 60.0, &f).is_empty(), "the time started again");
        let steps = run(&mut rest, 2.0 * MINIMIZED_FOR - 57.0, 2.0 * MINIMIZED_FOR - 55.0, &f);
        assert_eq!(kinds(&steps), ["show", "show"], "offered, then started");
        assert_eq!(rest.step(2.0 * MINIMIZED_FOR - 54.0, 0, &Facts { minimized: false, ..f }), Step::Withdraw, "restored during the countdown");
    }

    #[test]
    fn it_never_offers_while_something_works_vrchat_is_off_it_cannot_be_seen_or_the_switch_is_off() {
        for (why, f) in [
            ("working", Facts { working: true, ..facts() }),
            ("no VRChat", Facts { vrchat: false, ..facts() }),
            ("not seen", Facts { seen_at: None, ..facts() }),
            ("switched off", Facts { on: false, ..facts() }),
        ] {
            let mut rest = Rest::default();
            assert!(run(&mut rest, 0.0, 3.0 * MINIMIZED_FOR, &f).is_empty(), "{why}");
        }
        // Work that starts during the countdown takes the offer down; it comes again later, once all is quiet.
        let mut rest = Rest::default();
        let f = facts();
        run(&mut rest, 0.0, MINIMIZED_FOR + 2.0, &f);
        assert_eq!(rest.step(MINIMIZED_FOR + 3.0, 0, &Facts { working: true, ..f }), Step::Withdraw);
        assert!(run(&mut rest, MINIMIZED_FOR + 3.1, MINIMIZED_FOR + 3.0 + AGAIN_AFTER - 0.2, &f).is_empty(), "not again at once");
        assert_eq!(kinds(&run(&mut rest, MINIMIZED_FOR + 3.0 + AGAIN_AFTER, MINIMIZED_FOR + 3.2 + AGAIN_AFTER, &f)), ["show", "show"]);
    }

    #[test]
    fn in_vr_the_countdown_starts_when_the_band_shows_and_its_keep_open_counts() {
        let mut rest = Rest::default();
        let vr = Facts { seen_at: Some(Where::Vr), ..facts() };
        let steps = run(&mut rest, 0.0, MINIMIZED_FOR + 0.05, &vr);
        let Some(Step::Show(offer, Where::Vr)) = steps.first().copied() else { panic!("{steps:?}") };
        assert_eq!(offer.until_ms, 0, "no countdown before the band shows");
        // The band said nothing: the offer goes, unseen, and nothing closes.
        let steps = run(&mut rest, MINIMIZED_FOR + 0.15, MINIMIZED_FOR + 20.0, &vr);
        assert_eq!(kinds(&steps), ["withdraw"]);
        // Later the band shows the next offer: eight seconds from then.
        let later = MINIMIZED_FOR + AGAIN_AFTER + 10.0;
        let steps = run(&mut rest, later, later + 0.05, &vr);
        let Some(Step::Show(offer, _)) = steps.first().copied() else { panic!("{steps:?}") };
        let shown = Facts { shown_in_vr: Some(offer.id), ..vr };
        let steps = run(&mut rest, later + 1.0, later + 1.05, &shown);
        let Some(Step::Show(started, _)) = steps.first().copied() else { panic!("{steps:?}") };
        assert_eq!(started.until_ms, ((later + 1.0) * 1000.0) as u64 + 8_000);
        assert_eq!(kinds(&run(&mut rest, later + 1.1, later + 8.9, &shown)), Vec::<&str>::new());
        // Keep open, pressed in VR.
        let kept = Facts { kept_in_vr: Some(offer.id), ..shown };
        assert_eq!(rest.step(later + 8.95, 0, &kept), Step::Withdraw);
        assert!(rest.kept());
        assert!(run(&mut rest, later + 9.0, later + 2.0 * MINIMIZED_FOR, &shown).is_empty());
    }

    #[test]
    fn the_countdown_says_its_seconds_and_its_line() {
        let offer = RestOffer { id: 1, until_ms: 18_000, for_ms: 8_000, left_ms: 8_000 };
        assert_eq!((offer.seconds(10_000), offer.seconds(10_001), offer.seconds(17_001), offer.seconds(18_000)), (8, 8, 1, 0));
        assert!((offer.left(14_000) - 0.5).abs() < 1e-6);
        assert_eq!(RestOffer { until_ms: 0, ..offer }.left(99), 1.0, "not started: full");
    }

    #[test]
    fn the_page_to_come_back_on_is_read_once_and_only_when_recent() {
        assert_eq!(resume::parse("1000 worlds", 1000 + 60).as_deref(), Some("worlds"));
        assert_eq!(resume::parse("1000 worlds", 1000 + 2 * 24 * 3600), None, "a day old: forgotten");
        assert_eq!(resume::parse("garbage", 0), None);
    }

    #[test]
    fn its_words_are_in_every_language_and_fit() {
        let english = line(8);
        for language in orbiters_i18n::Language::TRANSLATED {
            orbiters_i18n::in_language(language, || {
                for words in [TITLE, KEEP, SETTING, SETTING_HINT] {
                    assert!(tr(words) != words, "{language:?}: {words}");
                }
                let said = line(8);
                assert!(said != english && said.contains('8') && !said.contains("{seconds}"), "{language:?}: {said}");
            });
        }
    }
}
