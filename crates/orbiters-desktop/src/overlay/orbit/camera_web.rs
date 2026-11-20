//! The page a QR code opened, over VRChat's camera (Enzo, F17): its page engine ([`orbiters_web`], a private off-screen WebView2 on Windows), what it shows, and
//! the laser and the stick on it. The camera's own scan finds the code (`controller::camera_tools`); this opens it there, never in the computer's browser unless
//! asked with Open in browser.
//!
//! A laser pressed and let go where it was is a click on the page; pressed and moved, the page scrolls with it, as a finger does on a phone, and glides on a
//! little when let go while moving. The stick scrolls it while the laser is on it (and only then: VRChat keeps the stick otherwise).
//!
//! Never without an end (K-qr-page, 2026-11-15: Enzo's first page stayed on "Opening the page..." for good): the page shows as soon as there is a picture of
//! it; with none after [`SLOW_AFTER`] it says so, with Try again beside Open in browser and the close; a failure says so with the same ways out. What the
//! engine did is noted in `vr-test.log` ("camera page: ...").
//!
//! Fast (Request 18, Enzo: "the page linked by the QR code often takes a while to load, drastically optimize that"). His log of 2026-11-20 had 9 to 12 s
//! between a code read and its page's navigation, one page 30 s and then "Operation aborted": the engine (a private profile with its cleaning watcher, a
//! browser, a page) was started only once a code was read, and that start was not even in the times noted. Now:
//! - an engine is started ahead ([`Spare`]) while the camera looks for a code, and a code's link goes to it the turn it is read;
//! - the page shows from its first paint (its document committed), not from its content read or its load;
//! - the same link read again while its page is open keeps that page, and one closed lately shows its last picture at once (not pressable) while its
//!   page opens anew;
//! - the time from the code read to the first picture is noted, and whether the engine was ready, started ahead or started then.

use super::panels::{web_host, WebPage, WebState};
use egui::{ColorImage, Pos2, Vec2};

/// The page's pixels: as wide as its place over the camera is in units, twice over, so that it reads as a phone's page does.
pub const PAGE_PX: [u32; 2] = [820, 940];
/// How far a laser moves on the page, in its pixels, before a press is a drag that scrolls rather than a click.
const DRAG_FROM_PX: f32 = 10.0;
/// No picture of the page this long after it was opened: it is said to be slow, with Try again.
pub const SLOW_AFTER: std::time::Duration = std::time::Duration::from_secs(12);
/// A slide let go faster than this (page pixels a second) glides on, slowing down by [`GLIDE_EASE`] each second.
const GLIDE_FROM: f32 = 400.0;
const GLIDE_EASE: f32 = 5.0;

/// A page's picture is remembered this long after its page closed: the same code read again shows it at once.
pub const REMEMBERED_FOR: std::time::Duration = std::time::Duration::from_secs(120);

/// An engine started ahead of a link while the camera looks for a code (Request 18): a code read goes to it at once. One at a time; one that failed is
/// started again at most every [`SPARE_AGAIN_AFTER`].
pub struct Spare {
    #[cfg(all(windows, not(test)))]
    page: orbiters_web::WebPage,
    started: std::time::Instant,
}

/// How long after a spare engine was started another may be (one that failed, or was used).
pub const SPARE_AGAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

impl Spare {
    /// An engine started now, with no page; none where there is no engine (macOS).
    pub fn start() -> Option<Self> {
        #[cfg(all(windows, not(test)))]
        {
            match orbiters_web::WebPage::new(PAGE_PX[0], PAGE_PX[1]) {
                Ok(page) => {
                    super::super::vr_log::line("camera page: an engine is started ahead, for the code the camera looks for"); // i18n: not shown (a log's line)
                    Some(Self { page, started: std::time::Instant::now() })
                }
                Err(e) => {
                    super::super::vr_log::line(&format!("camera page: no engine could be started ahead: {e:#}")); // i18n: not shown (a log's line)
                    None
                }
            }
        }
        // Tests drive what shows with no browser of their own.
        #[cfg(test)]
        {
            Some(Self { started: std::time::Instant::now() })
        }
        #[cfg(all(not(windows), not(test)))]
        {
            None
        }
    }

    /// Whether its engine failed or closed: no use for a link.
    pub fn failed(&mut self) -> bool {
        #[cfg(all(windows, not(test)))]
        {
            return self.page.poll().is_some_and(|(state, _, _)| matches!(state.load, orbiters_web::Load::Failed(_) | orbiters_web::Load::Closed));
        }
        #[allow(unreachable_code)]
        false
    }

    /// How it stands, for the log: ready, or started this long ago.
    fn standing(&self) -> String {
        #[cfg(all(windows, not(test)))]
        if self.page.ready() {
            return format!("an engine started ahead and ready ({} ms before)", self.started.elapsed().as_millis()); // i18n: not shown (a log's line)
        }
        format!("an engine started ahead {} ms before, still starting", self.started.elapsed().as_millis()) // i18n: not shown (a log's line)
    }
}

/// One page, opened from one code.
pub struct CameraWeb {
    #[cfg(windows)]
    page: Option<orbiters_web::WebPage>,
    /// The link the code held, as read (what a code read again is compared with; [`CameraWeb::url`] follows redirects).
    pub asked: String,
    /// What opened it, for the log: an engine started ahead, or one started for it.
    engine: String,
    /// A live picture of the page came; and whether what shows is a picture remembered from the last time it was open (not pressable).
    live: bool,
    remembered: bool,
    pub url: String,
    pub title: String,
    pub state: WebState,
    /// The page as drawn last, and whether it changed since it was painted.
    pub frame: Option<ColorImage>,
    pub fresh: bool,
    /// Where the laser is on the page, in its pixels; where it was pressed and last moved to, and whether that press became a drag.
    at: Option<Pos2>,
    held: Option<(Pos2, Pos2, bool)>,
    /// Input the engine's mailbox had no room for: sent again next turn (a release lost would leave the page pressed).
    waiting: Vec<Input>,
    /// When it was opened (or tried again), and the engine is still loading it.
    opened: std::time::Instant,
    pub busy: bool,
    /// A slide's speed (page pixels a second, down positive) while held, and its glide once let go; the last turn.
    speed: f32,
    glide: f32,
    last_turn: Option<std::time::Instant>,
    /// The last click or scroll sent and when, until a new picture of the page came: how long a press takes to show is
    /// noted in `vr-test.log` (K-qr-page: "pressing the site's buttons was very slow").
    awaiting: Option<(&'static str, std::time::Instant)>,
}

/// What goes to the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Move(Pos2),
    Press(Pos2),
    Release(Pos2),
    Scroll(Pos2, f32),
}

impl CameraWeb {
    /// Opens `url` in a page of its own, with an engine started for it.
    pub fn open(url: &str) -> Self {
        Self::open_with(url, None, None)
    }

    /// Opens `url` in `spare`'s engine when there is one (started ahead: Request 18), else in one started now; showing `remembered` (the last
    /// picture of the same link's page, closed lately) until a live picture comes.
    pub fn open_with(url: &str, spare: Option<Spare>, remembered: Option<ColorImage>) -> Self {
        // Changed only where an engine opens (Windows, outside tests).
        #[allow(unused_mut)]
        let mut web = Self::idle(url);
        web.engine = spare.as_ref().map_or_else(|| "an engine started for it".to_string(), Spare::standing); // i18n: not shown (a log's line)
        if let Some(picture) = remembered {
            web.frame = Some(picture);
            web.fresh = true;
            web.remembered = true;
        }
        super::super::vr_log::line(&format!("camera page: {} read, opening it in {}{}", web_host(url), web.engine, if web.frame.is_some() { ", its last picture shown meanwhile" } else { "" })); // i18n: not shown (a log's line)
        // Tests drive what shows around it, with no browser of their own (camera-web proves the engine).
        #[cfg(all(windows, not(test)))]
        {
            let page = match spare {
                Some(spare) => Ok(spare.page),
                None => orbiters_web::WebPage::new(PAGE_PX[0], PAGE_PX[1]),
            };
            match page.and_then(|page| page.navigate(url).map(|_| page)) {
                Ok(page) => web.page = Some(page),
                Err(e) => {
                    super::super::vr_log::line(&format!("camera page: could not open: {e:#}")); // i18n: not shown (a log's line)
                    web.state = WebState::Failed(e.to_string());
                }
            }
        }
        #[cfg(test)]
        let _ = spare;
        #[cfg(not(windows))]
        {
            web.state = WebState::Failed(String::new());
        }
        web
    }

    /// Its last live picture and the link it was opened for, to show at once if the same code is read again ([`REMEMBERED_FOR`]).
    pub fn last_picture(&self) -> Option<(String, ColorImage)> {
        self.frame.as_ref().filter(|_| self.live).map(|f| (self.asked.clone(), f.clone()))
    }

    /// Whether it failed (a link read again opens it anew then).
    pub fn failed(&self) -> bool {
        matches!(self.state, WebState::Failed(_))
    }

    /// A page with no engine yet: what [`CameraWeb::open`] starts from (and tests drive).
    pub(super) fn idle(url: &str) -> Self {
        Self {
            #[cfg(windows)]
            page: None,
            asked: url.to_string(),
            engine: String::new(),
            live: false,
            remembered: false,
            url: url.to_string(),
            title: String::new(),
            state: WebState::Loading,
            frame: None,
            fresh: false,
            at: None,
            held: None,
            waiting: Vec::new(),
            opened: std::time::Instant::now(),
            busy: true,
            speed: 0.0,
            glide: 0.0,
            last_turn: None,
            awaiting: None,
        }
    }

    /// Opens the same address again in a new engine (Try again): the old one is closed.
    pub fn retry(&mut self) {
        let url = self.url.clone();
        super::super::vr_log::line(&format!("camera page: tried again ({})", web_host(&url))); // i18n: not shown (a log's line)
        *self = Self::open(&url);
    }

    /// What the page did since the last turn. Whether anything shown changed.
    pub fn turn(&mut self) -> bool {
        let mut changed = false;
        for input in std::mem::take(&mut self.waiting) {
            self.send(input);
        }
        self.glide_on();
        #[cfg(windows)]
        if let Some(page) = self.page.as_ref() {
            for note in page.notes() {
                super::super::vr_log::line(&format!("camera page: {note}")); // i18n: not shown (a log's line)
            }
        }
        #[cfg(windows)]
        if let Some((state, frame, fresh)) = self.page.as_mut().and_then(|page| page.poll()) {
            self.busy = state.load == orbiters_web::Load::Loading;
            let load = match state.load {
                // Still loading: what `settle` made of it stands (Slow said once, not undone and said again each turn: his log
                // of 2026-11-20 had "no picture ... after 12 s" 1,589 times, and every turn repainted the page's parts).
                orbiters_web::Load::Loading => loading(&self.state),
                orbiters_web::Load::Loaded => WebState::Shown,
                orbiters_web::Load::Failed(why) => {
                    if !matches!(self.state, WebState::Failed(_)) {
                        super::super::vr_log::line(&format!("camera page: failed: {why}")); // i18n: not shown (a log's line)
                    }
                    WebState::Failed(why)
                }
                orbiters_web::Load::Closed => WebState::Failed(String::new()),
            };
            // A page that names itself; the address it went to (a link on it was followed).
            let (title, url) = (state.title.chars().take(200).collect::<String>(), if state.url.is_empty() { self.url.clone() } else { state.url });
            if load != self.state || title != self.title || url != self.url {
                (self.state, self.title, self.url) = (load, title, url);
                changed = true;
            }
            match frame {
                Some(frame) if fresh => {
                    self.remembered = false;
                    if !self.live {
                        self.live = true;
                        super::super::vr_log::line(&format!("camera page: first picture of {} {} ms after its code was read ({})", web_host(&self.url), self.opened.elapsed().as_millis(), self.engine)); // i18n: not shown (a log's line)
                    }
                    if let Some((what, at)) = self.awaiting.take() {
                        super::super::vr_log::line(&format!("camera page: a {what} showed on the page {} ms after it was sent", at.elapsed().as_millis())); // i18n: not shown (a log's line)
                    }
                    self.frame = Some(ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba));
                    self.fresh = true;
                    changed = true;
                }
                // A navigation cleared it (a remembered picture stays until the live one comes).
                None if self.frame.is_some() && fresh && !self.remembered => {
                    self.frame = None;
                    changed = true;
                }
                _ => {}
            }
        }
        changed |= self.settle();
        changed
    }

    /// What shows follows what there is: the page as soon as there is a picture of it (still loading or not); none yet, it is
    /// opening, then slow after [`SLOW_AFTER`]. A failure stays. Whether that changed.
    fn settle(&mut self) -> bool {
        let state = match &self.state {
            WebState::Failed(_) => return false,
            _ if self.frame.is_some() => WebState::Shown,
            _ if self.opened.elapsed() >= SLOW_AFTER => WebState::Slow,
            _ => WebState::Loading,
        };
        if state == WebState::Slow && self.state != WebState::Slow {
            super::super::vr_log::line(&format!("camera page: no picture of {} after {} s", web_host(&self.url), SLOW_AFTER.as_secs())); // i18n: not shown (a log's line)
        }
        let changed = state != self.state;
        self.state = state;
        changed
    }

    /// A slide let go while moving goes on a little, slowing down, as a phone's page does.
    fn glide_on(&mut self) {
        let now = std::time::Instant::now();
        let dt = self.last_turn.map_or(0.0, |t| (now - t).as_secs_f32()).min(0.1);
        self.last_turn = Some(now);
        if self.held.is_some() || self.glide == 0.0 {
            return;
        }
        let Some(p) = self.at else {
            self.glide = 0.0;
            return;
        };
        let step = self.glide * dt;
        self.glide *= (-GLIDE_EASE * dt).exp();
        if self.glide.abs() < 60.0 {
            self.glide = 0.0;
        }
        if step != 0.0 {
            self.send(Input::Scroll(p, step));
        }
    }

    /// As the panels show it.
    pub fn facts(&self) -> WebPage {
        WebPage { url: self.url.clone(), title: self.title.clone(), state: self.state.clone(), picture: None, busy: self.busy }
    }

    /// The laser on the page at `at` of its place of `size` units, or off it.
    pub fn point(&mut self, at: Option<(Pos2, Vec2)>) {
        let px = at.map(|(p, size)| self.pixel(p, size));
        if px == self.at {
            return;
        }
        self.at = px;
        let Some(p) = px else { return };
        match self.held.as_mut() {
            Some((from, last, dragging)) => {
                if !*dragging && (p - *from).length() >= DRAG_FROM_PX {
                    *dragging = true;
                }
                if *dragging {
                    // Moved up, the page goes up with it: what was lower comes into view.
                    let dy = last.y - p.y;
                    *last = p;
                    // Its speed, smoothed over a few turns: what a release lets glide on.
                    let now = std::time::Instant::now();
                    let dt = self.last_turn.map_or(1.0 / 90.0, |t| (now - t).as_secs_f32()).max(1.0 / 240.0);
                    self.speed = self.speed * 0.6 + dy / dt * 0.4;
                    self.send(Input::Scroll(p, dy));
                }
            }
            None => self.send(Input::Move(p)),
        }
    }

    /// Pressed where the laser is. Not on a remembered picture: the page under it is not there yet.
    pub fn press(&mut self) {
        if self.remembered {
            return;
        }
        if let Some(p) = self.at {
            self.held = Some((p, p, false));
            self.speed = 0.0;
            self.glide = 0.0;
        }
    }

    /// Let go: a click where it was pressed, unless it became a drag.
    /// The page's hold ends with no let-go (its menu went): no click.
    pub fn cancel(&mut self) {
        self.held = None;
    }

    pub fn release(&mut self) {
        if let Some((from, _, dragging)) = self.held.take() {
            if !dragging {
                self.send(Input::Press(from));
                self.send(Input::Release(from));
            } else if self.speed.abs() >= GLIDE_FROM {
                self.glide = self.speed;
            }
        }
    }

    /// Whether the laser is on the page.
    pub fn pointed(&self) -> bool {
        self.at.is_some()
    }

    /// The stick on the page: `pixels` down, positive.
    pub fn scroll(&mut self, pixels: f32) {
        if let Some(p) = self.at {
            self.glide = 0.0;
            self.send(Input::Scroll(p, pixels));
        }
    }

    /// Where `p` of a place of `size` units lies on the page, in its pixels: the page's own (its view is [`PAGE_PX`] whatever
    /// size its pictures come at), which is what the engine's input is in.
    fn pixel(&self, p: Pos2, size: Vec2) -> Pos2 {
        let (w, h) = (PAGE_PX[0] as f32, PAGE_PX[1] as f32);
        Pos2::new((p.x / size.x.max(1.0)).clamp(0.0, 1.0) * w, (p.y / size.y.max(1.0)).clamp(0.0, 1.0) * h)
    }

    fn send(&mut self, input: Input) {
        let what = match input {
            Input::Release(_) => Some("click"),
            Input::Scroll(..) => Some("scroll"),
            _ => None,
        };
        if let Some(what) = what.filter(|_| self.awaiting.is_none_or(|(_, at)| at.elapsed() > std::time::Duration::from_secs(2))) {
            self.awaiting = Some((what, std::time::Instant::now()));
        }
        #[cfg(windows)]
        if let Some(page) = self.page.as_ref() {
            let to = match input {
                Input::Move(p) => orbiters_web::Input::Move { x: p.x as f64, y: p.y as f64 },
                Input::Press(p) => orbiters_web::Input::Press { x: p.x as f64, y: p.y as f64 },
                Input::Release(p) => orbiters_web::Input::Release { x: p.x as f64, y: p.y as f64 },
                Input::Scroll(p, dy) => orbiters_web::Input::Wheel { x: p.x as f64, y: p.y as f64, dx: 0.0, dy: dy as f64 },
            };
            // A full mailbox: a move or a scroll is soon replaced; a press or a release must arrive.
            if page.input(to).is_err() && matches!(input, Input::Press(_) | Input::Release(_)) {
                self.waiting.push(input);
            }
        }
        #[cfg(not(windows))]
        let _ = input;
    }
}

/// What a page still loading shows: what it showed (the page itself, or slow), else that it opens.
fn loading(now: &WebState) -> WebState {
    match now {
        WebState::Slow | WebState::Shown => now.clone(),
        _ => WebState::Loading,
    }
}

impl Drop for CameraWeb {
    fn drop(&mut self) {
        // Signals the engine and returns at once: it cleans its private profile up on its own thread.
        #[cfg(windows)]
        if let Some(page) = self.page.as_ref() {
            page.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enzo (F17): the page scrolls with the laser and a short press is a click: a press held and moved scrolls and clicks nothing; one let go where it was clicks.
    #[test]
    fn a_laser_drag_scrolls_the_page_and_a_press_clicks_it() {
        let mut web = CameraWeb::idle("https://orbiters.cc");
        web.state = WebState::Shown;
        let size = Vec2::new(410.0, 470.0);
        web.point(Some((Pos2::new(205.0, 300.0), size)));
        assert!(web.pointed());
        web.press();
        web.point(Some((Pos2::new(205.0, 200.0), size)));
        assert!(web.held.is_some_and(|h| h.2), "moved, it is a drag");
        web.release();
        assert!(web.held.is_none());
        // Where a unit lands on the page.
        let p = web.pixel(Pos2::new(410.0, 0.0), size);
        assert_eq!((p.x, p.y), (PAGE_PX[0] as f32, 0.0));
        web.point(None);
        assert!(!web.pointed());
    }

    /// K-qr-page: never "Opening the page..." for good. A picture shows the page at once, loaded or not; none after
    /// [`SLOW_AFTER`] says it is slow (Try again, Open in browser); a failure stays said; Try again starts it over.
    #[test]
    fn a_page_always_ends_in_a_picture_or_a_message() {
        let mut web = CameraWeb::idle("https://vrcidlinker.com/");
        assert!(!web.settle());
        assert_eq!(web.state, WebState::Loading);
        web.opened = std::time::Instant::now() - SLOW_AFTER;
        assert!(web.settle(), "slow is said");
        assert_eq!(web.state, WebState::Slow);
        assert!(web.facts().busy);
        web.frame = Some(ColorImage::filled([4, 4], egui::Color32::WHITE));
        assert!(web.settle());
        assert_eq!(web.state, WebState::Shown, "a picture shows, though the page has not finished loading");
        web.state = WebState::Failed("navigation error 5".into());
        assert!(!web.settle());
        assert!(matches!(web.state, WebState::Failed(_)), "a failure stays said");
        web.retry();
        assert_eq!(web.state, WebState::Loading);
        assert!(web.frame.is_none());
        assert_eq!(web.url, "https://vrcidlinker.com/");
    }

    /// Request 18: a page slow to give a picture is said slow once and stays so while it loads (his log had the line 1,589 times: the engine's
    /// "loading" undid "slow" every turn, and `settle` said it again).
    #[test]
    fn a_slow_page_is_said_slow_once() {
        assert_eq!(loading(&WebState::Slow), WebState::Slow);
        assert_eq!(loading(&WebState::Loading), WebState::Loading);
        assert_eq!(loading(&WebState::Shown), WebState::Shown);
        let mut web = CameraWeb::idle("https://store.steampowered.com/");
        web.opened = std::time::Instant::now() - SLOW_AFTER;
        assert!(web.settle());
        web.state = loading(&web.state);
        assert!(!web.settle(), "nothing changed, nothing said again");
        assert_eq!(web.state, WebState::Slow);
    }

    /// Request 18: a link read again within [`REMEMBERED_FOR`] of its page closing shows that page's last picture at once, not pressable, until the live
    /// page's first picture replaces it.
    #[test]
    fn a_page_closed_lately_shows_its_last_picture_at_once() {
        let mut shown = CameraWeb::idle("https://linktr.ee/someone");
        shown.frame = Some(ColorImage::filled([4, 4], egui::Color32::RED));
        assert_eq!(shown.last_picture(), None, "only a live picture is remembered");
        shown.live = true;
        let (link, picture) = shown.last_picture().unwrap();
        assert_eq!(link, "https://linktr.ee/someone");
        let mut again = CameraWeb::open_with(&link, Spare::start(), Some(picture));
        assert!(again.frame.is_some() && again.fresh, "shown at once");
        assert!(again.settle());
        assert_eq!(again.state, WebState::Shown);
        again.point(Some((Pos2::new(10.0, 10.0), Vec2::new(410.0, 470.0))));
        again.press();
        assert!(again.held.is_none(), "a remembered picture is not the page: no press");
    }

    /// K-qr-page: a slide moves the page as the laser moves (one scroll a turn), and let go while moving it glides on, slowing
    /// down; a press stops a glide. Laser positions map to the page's own pixels whatever size its pictures come at.
    #[test]
    fn a_slide_scrolls_with_the_laser_and_glides_when_let_go() {
        let mut web = CameraWeb::idle("https://en.wikipedia.org/wiki/Virtual_reality");
        web.state = WebState::Shown;
        web.frame = Some(ColorImage::filled([410, 470], egui::Color32::WHITE));
        let size = Vec2::new(410.0, 470.0);
        assert_eq!(web.pixel(Pos2::new(410.0, 470.0), size), Pos2::new(PAGE_PX[0] as f32, PAGE_PX[1] as f32), "the page's pixels, not its picture's");
        web.point(Some((Pos2::new(200.0, 400.0), size)));
        web.press();
        for k in 1..=10 {
            web.last_turn = Some(std::time::Instant::now() - std::time::Duration::from_millis(11));
            web.point(Some((Pos2::new(200.0, 400.0 - 12.0 * k as f32), size)));
        }
        assert!(web.held.is_some_and(|h| h.2), "a drag");
        assert!(web.speed > GLIDE_FROM, "moving up fast: {}", web.speed);
        web.release();
        assert!(web.glide > 0.0, "it glides on");
        let mut turns = 0;
        while web.glide != 0.0 && turns < 1000 {
            web.last_turn = Some(std::time::Instant::now() - std::time::Duration::from_millis(11));
            web.glide_on();
            turns += 1;
        }
        assert!(turns > 5 && turns < 200, "the glide eases out in a moment: {turns} turns");
        web.glide = 900.0;
        web.press();
        assert_eq!(web.glide, 0.0, "a press stops it");
    }
}
