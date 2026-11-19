//! The QR code scanner (N4N0's menu, More; contracts `Docs/n4n0.md`, Menu): what it shows, which codes open a page and how
//! often. The desktop reads VRChat's stream camera and decodes the codes; what a code may open, the same link not opened
//! again, and when scanning stops are decided here, the same for every menu.

use orbiters_i18n::mark;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Scanning stops after this long without a link opened (Scan again starts it again).
pub const SCAN_FOR: Duration = Duration::from_secs(120);
/// A link is not opened again while it was seen this recently (a code held before the camera opens one page).
pub const REOPEN_AFTER: Duration = Duration::from_secs(30);
/// The longest link opened.
pub const MAX_LINK: usize = 2048;
/// Pages a scan opens at once (two codes side by side), then one every [`OPEN_EVERY`], and [`MOST_PER_SCAN`] in all: a
/// slideshow or a video of codes never floods the browser with tabs (N4N0-13).
pub const OPEN_BURST: u32 = 2;
pub const OPEN_EVERY: Duration = Duration::from_secs(5);
pub const MOST_PER_SCAN: u32 = 6;
/// What the scanner says once a scan opened [`MOST_PER_SCAN`] pages: it stops.
pub const ENOUGH_NOTE: &str = mark("Stopped after six pages opened: press Scan again to read the camera");

/// What the scanner does now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QrPhase {
    /// Not scanning.
    #[default]
    Off,
    /// Asked for, VRChat's camera not read yet.
    Starting,
    /// No picture comes from VRChat's camera: the note says how to turn its stream on.
    NoCamera,
    /// Frames come: looking for a QR code.
    Looking,
    /// Stopped on its own (after [`SCAN_FOR`], or it could not run): the note says why.
    Stopped,
}

/// The scanner as the menus show it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrView {
    pub phase: QrPhase,
    /// How to turn VRChat's stream camera on, why it stopped, or a code that holds no web link.
    #[serde(default)]
    pub note: String,
    /// The last link opened, and how many were (each opening counts).
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub opened: u32,
}

impl QrView {
    /// It scans (or is starting to).
    pub fn scanning(&self) -> bool {
        matches!(self.phase, QrPhase::Starting | QrPhase::NoCamera | QrPhase::Looking)
    }
}

/// What the scanner tells the menus while VRChat's camera sends no picture: where it stands and what to do.
pub fn no_camera_note(camera_open: Option<bool>) -> String {
    match camera_open {
        Some(false) => mark("Open VRChat's camera, choose Stream, then turn Spout Stream on in its settings").into(),
        _ => mark("No picture from VRChat's camera: choose Stream and turn Spout Stream on in its settings").into(),
    }
}

/// The page a QR code's text opens: an `http` or `https` link with a host and no user name (`https://bank.com@evil.example`
/// is refused), without spaces or control characters, at most [`MAX_LINK`] long; the scheme as lower case. Else why not.
pub fn web_link(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if text.len() > MAX_LINK {
        return Err(mark("The link is too long"));
    }
    let (scheme, rest) = text.split_once("://").ok_or(mark("Not a web link"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(mark("Not a web link"));
    }
    if rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(mark("Not a web link"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(mark("The link hides where it goes"));
    }
    // The host, without its port (digits only); an IPv6 address in brackets.
    let digits = |port: &str| port.bytes().all(|b| b.is_ascii_digit());
    let host = match authority.strip_prefix('[') {
        Some(inner) => match inner.split_once(']') {
            Some((host, "")) => host,
            Some((host, port)) if port.strip_prefix(':').is_some_and(digits) => host,
            _ => return Err(mark("Not a web link")),
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) if digits(port) => host,
            Some(_) => return Err(mark("Not a web link")),
            None => authority,
        },
    };
    let name = |c: char| c.is_alphanumeric() || matches!(c, '-' | '.' | '_') || (authority.starts_with('[') && c == ':');
    if host.is_empty() || host.starts_with('.') || host.starts_with('-') || !host.chars().all(name) {
        return Err(mark("Not a web link"));
    }
    Ok(format!("{scheme}://{rest}"))
}

/// The links seen lately: one is opened again only once [`REOPEN_AFTER`] passed since it was last seen (each sighting
/// counts, so a code held before the camera opens its page once).
#[derive(Clone, Debug, Default)]
pub struct Recent {
    seen: Vec<(String, Instant)>,
}

/// Whether a scan opens one more page now ([`OPEN_BURST`], [`OPEN_EVERY`], [`MOST_PER_SCAN`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    Open,
    /// Not yet: the code opens if it is still in view a moment later.
    Later,
    /// This scan opened enough: it stops ([`ENOUGH_NOTE`]).
    Enough,
}

/// The pages a scan opened, and when it may open the next ones.
#[derive(Clone, Debug, Default)]
pub struct Opening {
    opened: u32,
    /// Openings allowed now beyond those used ([`OPEN_BURST`] at most), as of `at`.
    allowed: f32,
    at: Option<Instant>,
}

impl Opening {
    /// Whether a page opens at `now`; one that does is counted.
    pub fn take(&mut self, now: Instant) -> Pace {
        if self.opened >= MOST_PER_SCAN {
            return Pace::Enough;
        }
        let allowed = match self.at {
            None => OPEN_BURST as f32,
            Some(at) => (self.allowed + now.saturating_duration_since(at).as_secs_f32() / OPEN_EVERY.as_secs_f32()).min(OPEN_BURST as f32),
        };
        self.at = Some(now);
        if allowed < 1.0 {
            self.allowed = allowed;
            return Pace::Later;
        }
        self.allowed = allowed - 1.0;
        self.opened += 1;
        Pace::Open
    }
}

impl Recent {
    /// Whether `link` was seen within [`REOPEN_AFTER`] of `now`; seen, it is noted as seen again (it waits longer).
    pub fn lately(&mut self, link: &str, now: Instant) -> bool {
        self.seen.retain(|(_, at)| now.saturating_duration_since(*at) < REOPEN_AFTER);
        match self.seen.iter_mut().find(|(l, _)| l == link) {
            Some((_, at)) => {
                *at = now;
                true
            }
            None => false,
        }
    }

    /// `link` is being opened at `now`.
    pub fn opening(&mut self, link: &str, now: Instant) {
        if !self.lately(link, now) {
            self.seen.push((link.to_string(), now));
        }
    }

    /// Whether `link`, seen at `now`, is to be opened; it is noted as seen either way.
    pub fn fresh(&mut self, link: &str, now: Instant) -> bool {
        self.seen.retain(|(_, at)| now.saturating_duration_since(*at) < REOPEN_AFTER);
        match self.seen.iter_mut().find(|(l, _)| l == link) {
            Some((_, at)) => {
                *at = now;
                false
            }
            None => {
                self.seen.push((link.to_string(), now));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_open() {
        assert_eq!(web_link("https://orbiters.cc/maps?w=1#top"), Ok("https://orbiters.cc/maps?w=1#top".into()));
        assert_eq!(web_link("  HTTP://Example.com/A  \n"), Ok("http://Example.com/A".into()), "the scheme as lower case, the rest as it was");
        assert_eq!(web_link("http://127.0.0.1:4100/x"), Ok("http://127.0.0.1:4100/x".into()));
        assert_eq!(web_link("https://[::1]:8080/"), Ok("https://[::1]:8080/".into()));
        for text in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "ms-settings:privacy",
            "steam://run/438100",
            "WIFI:S:home;T:WPA;P:secret;;",
            "orbiters.cc",
            "https://",
            "https:///path",
            "https://bad host/",
            "https://a.com/\u{7}",
            "https://example.com:http/",
            "https://<b>.com/",
        ] {
            assert_eq!(web_link(text), Err("Not a web link"), "{text}");
        }
        assert_eq!(web_link("https://bank.com@evil.example/login"), Err("The link hides where it goes"));
        assert_eq!(web_link(&format!("https://a.com/{}", "x".repeat(MAX_LINK))), Err("The link is too long"));
    }

    #[test]
    fn a_scan_opens_two_pages_at_once_then_one_every_few_seconds_and_six_in_all() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut opening = Opening::default();
        // A slideshow of codes, three a second: two open, then one every five seconds.
        let paces: Vec<Pace> = (0..30).map(|i| opening.take(at(i * 333))).collect();
        assert_eq!(paces.iter().filter(|p| **p == Pace::Open).count(), 3, "{paces:?}");
        assert_eq!(&paces[..3], &[Pace::Open, Pace::Open, Pace::Later]);
        let mut opening = Opening::default();
        let opened = (0..200).map(|i| opening.take(at(i * 1000))).filter(|p| *p == Pace::Open).count();
        assert_eq!(opened, MOST_PER_SCAN as usize);
        assert_eq!(opening.take(at(1_000_000)), Pace::Enough, "then it is enough");
    }

    #[test]
    fn the_same_link_waits_before_it_opens_again() {
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut recent = Recent::default();
        assert!(recent.fresh("https://a.com", at(0)));
        assert!(!recent.fresh("https://a.com", at(1)), "seen a second ago");
        assert!(recent.fresh("https://b.com", at(2)), "another link opens at once");
        // Held before the camera, it keeps being seen: never opened again meanwhile.
        for s in (5..100).step_by(5) {
            assert!(!recent.fresh("https://a.com", at(s)), "{s} s");
        }
        // Out of sight for long enough, it opens again.
        assert!(recent.fresh("https://a.com", at(95 + REOPEN_AFTER.as_secs())));
        assert!(recent.fresh("https://b.com", at(200)));
    }

    #[test]
    fn the_view_says_whether_it_scans() {
        assert!(!QrView::default().scanning());
        for phase in [QrPhase::Starting, QrPhase::NoCamera, QrPhase::Looking] {
            assert!(QrView { phase, ..Default::default() }.scanning());
        }
        assert!(!QrView { phase: QrPhase::Stopped, ..Default::default() }.scanning());
        assert!(no_camera_note(Some(false)).starts_with("Open VRChat's camera"));
        assert!(no_camera_note(None).contains("Spout Stream"));
        let json = serde_json::to_string(&QrView { phase: QrPhase::Looking, link: Some("https://a.com".into()), opened: 1, note: String::new() }).unwrap();
        assert_eq!(json, r#"{"phase":"looking","note":"","link":"https://a.com","opened":1}"#);
    }
}
