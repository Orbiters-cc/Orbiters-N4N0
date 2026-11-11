//! The page a QR code opened, over VRChat's camera (Enzo, F17): its page engine ([`orbiters_web`], a private off-screen WebView2 on Windows), what it shows, and
//! the laser and the stick on it. The camera's own scan finds the code (`controller::camera_tools`); this opens it there, never in the computer's browser unless
//! asked with Open in browser.
//!
//! A laser pressed and let go where it was is a click on the page; pressed and moved, the page scrolls with it, as a finger does on a phone. The stick scrolls it
//! while the laser is on it.

use super::panels::{WebPage, WebState};
use egui::{ColorImage, Pos2, Vec2};

/// The page's pixels: as wide as its place over the camera is in units, twice over, so that it reads as a phone's page does.
pub const PAGE_PX: [u32; 2] = [820, 940];
/// How far a laser moves on the page, in its pixels, before a press is a drag that scrolls rather than a click.
const DRAG_FROM_PX: f32 = 10.0;

/// One page, opened from one code.
pub struct CameraWeb {
    #[cfg(windows)]
    page: Option<orbiters_web::WebPage>,
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
    /// Opens `url` in a page of its own.
    pub fn open(url: &str) -> Self {
        let mut web = Self { #[cfg(windows)] page: None, url: url.to_string(), title: String::new(), state: WebState::Loading, frame: None, fresh: false, at: None, held: None, waiting: Vec::new() };
        #[cfg(windows)]
        match orbiters_web::WebPage::new(PAGE_PX[0], PAGE_PX[1]).and_then(|page| page.navigate(url).map(|_| page)) {
            Ok(page) => web.page = Some(page),
            Err(e) => {
                super::super::vr_log::line(&format!("camera page: could not open: {e:#}")); // i18n: not shown (a log's line)
                web.state = WebState::Failed(e.to_string());
            }
        }
        #[cfg(not(windows))]
        {
            web.state = WebState::Failed(String::new());
        }
        web
    }

    /// What the page did since the last turn. Whether anything shown changed.
    pub fn turn(&mut self) -> bool {
        let mut changed = false;
        for input in std::mem::take(&mut self.waiting) {
            self.send(input);
        }
        #[cfg(windows)]
        if let Some((state, frame, fresh)) = self.page.as_mut().and_then(|page| page.poll()) {
            let load = match state.load {
                orbiters_web::Load::Loading => WebState::Loading,
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
                    self.frame = Some(ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba));
                    self.fresh = true;
                    changed = true;
                }
                // A navigation cleared it.
                None if self.frame.is_some() && fresh => {
                    self.frame = None;
                    changed = true;
                }
                _ => {}
            }
        }
        // Shown only once there is a picture of it: Loaded with nothing drawn yet still says it is opening.
        if self.state == WebState::Shown && self.frame.is_none() {
            self.state = WebState::Loading;
        }
        changed
    }

    /// As the panels show it.
    pub fn facts(&self) -> WebPage {
        WebPage { url: self.url.clone(), title: self.title.clone(), state: self.state.clone(), picture: None }
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
                    self.send(Input::Scroll(p, dy));
                }
            }
            None => self.send(Input::Move(p)),
        }
    }

    /// Pressed where the laser is.
    pub fn press(&mut self) {
        if let Some(p) = self.at {
            self.held = Some((p, p, false));
        }
    }

    /// Let go: a click where it was pressed, unless it became a drag.
    pub fn release(&mut self) {
        if let Some((from, _, dragging)) = self.held.take() {
            if !dragging {
                self.send(Input::Press(from));
                self.send(Input::Release(from));
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
            self.send(Input::Scroll(p, pixels));
        }
    }

    /// Where `p` of a place of `size` units lies on the page, in its pixels.
    fn pixel(&self, p: Pos2, size: Vec2) -> Pos2 {
        let (w, h) = self.frame.as_ref().map_or((PAGE_PX[0] as f32, PAGE_PX[1] as f32), |f| (f.size[0] as f32, f.size[1] as f32));
        Pos2::new((p.x / size.x.max(1.0)).clamp(0.0, 1.0) * w, (p.y / size.y.max(1.0)).clamp(0.0, 1.0) * h)
    }

    fn send(&mut self, input: Input) {
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
        let mut web = CameraWeb { #[cfg(windows)] page: None, url: "https://orbiters.cc".into(), title: String::new(), state: WebState::Shown, frame: None, fresh: false, at: None, held: None, waiting: Vec::new() };
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
}
