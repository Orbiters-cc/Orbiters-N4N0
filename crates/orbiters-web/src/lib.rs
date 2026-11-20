//! Private, windowless WebView2 pages for VR overlays. Absent on macOS.
#![cfg(windows)]
mod engine;
mod lifetime;
mod network;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, SyncSender},
    Arc, Mutex,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    Loaded,
    Failed(String),
    Closed,
}
#[derive(Clone, Debug)]
pub struct State {
    pub load: Load,
    pub url: String,
    pub title: String,
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// The decoded picture itself, shared (CORE-11): an `Arc<[u8]>` made from it copied every changed picture (3 MB at the
    /// camera page's 820 x 940, up to 40 a second).
    pub rgba: Arc<Vec<u8>>,
    pub sequence: u64,
}
#[derive(Clone, Debug)]
pub enum Input {
    Move {
        x: f64,
        y: f64,
    },
    Press {
        x: f64,
        y: f64,
    },
    Release {
        x: f64,
        y: f64,
    },
    /// Positive deltas scroll right/down, in page pixels (not wheel notches).
    Wheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
}
pub(crate) enum Command {
    #[cfg(test)]
    Fixture(String),
    #[cfg(test)]
    FailCdp,
    Navigate(String),
    Input(Input),
    Resize(u32, u32),
}
pub(crate) struct Shared {
    state: State,
    frame: Option<Frame>,
    /// The engine is up (its private profile, browser and page made) and takes a navigation at once: from then on a link
    /// opens in the time its site takes, not the engine's start too (Request 18).
    ready: bool,
    /// What happened, in order, for the overlay's log (a few lines a page; never what the page holds).
    notes: Vec<String>,
}

impl Shared {
    fn note(&mut self, line: String) {
        // Bounded: a page that navigates without end must not grow it.
        if self.notes.len() < 64 {
            self.notes.push(line);
        }
    }
}
/// All methods are nonblocking: bounded command mailbox and a latest-frame slot.
/// `poll` returns None on brief contention; retain the previous snapshot then.
pub struct WebPage {
    tx: SyncSender<Command>,
    shared: Arc<Mutex<Shared>>,
    closed: Arc<AtomicBool>,
    seen: u64,
}
impl WebPage {
    pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
        Self::start(width, height, false)
    }
    /// An engine whose pages may reach this computer's own address: this crate's tests only (a slow page they serve).
    #[cfg(test)]
    pub(crate) fn new_reaching_loopback(width: u32, height: u32) -> anyhow::Result<Self> {
        Self::start(width, height, true)
    }
    fn start(width: u32, height: u32, loopback: bool) -> anyhow::Result<Self> {
        size(width, height)?;
        let (tx, rx) = mpsc::sync_channel(64);
        let shared = Arc::new(Mutex::new(Shared { state: State { load: Load::Loading, url: String::new(), title: String::new() }, frame: None, ready: false, notes: Vec::new() }));
        let s = shared.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let closing = closed.clone();
        std::thread::Builder::new().name("camera-web".into()).spawn(move || {
            if let Err(e) = engine::run(width, height, rx, s.clone(), closing, loopback) {
                let mut s = s.lock().unwrap();
                s.note(format!("engine stopped: {e:#}")); // i18n: not shown (a log's line)
                s.state.load = Load::Failed(display_text(&format!("{e:#}"), 1024));
            }
        })?;
        Ok(Self { tx, shared, closed, seen: 0 })
    }
    pub fn navigate(&self, url: &str) -> anyhow::Result<()> {
        validate(url)?;
        self.send(Command::Navigate(url.into()))
    }
    pub fn input(&self, input: Input) -> anyhow::Result<()> {
        let values = match &input {
            Input::Move { x, y } | Input::Press { x, y } | Input::Release { x, y } => vec![*x, *y],
            Input::Wheel { x, y, dx, dy } => vec![*x, *y, *dx, *dy],
        };
        anyhow::ensure!(values.iter().all(|v| v.is_finite() && v.abs() <= 100_000.), "invalid page input coordinates");
        self.send(Command::Input(input))
    }
    pub fn resize(&self, width: u32, height: u32) -> anyhow::Result<()> {
        size(width, height)?;
        self.send(Command::Resize(width, height))
    }
    fn send(&self, c: Command) -> anyhow::Result<()> {
        anyhow::ensure!(!self.closed.load(Ordering::Relaxed), "browser is closing");
        self.tx.try_send(c).map_err(|e| anyhow::anyhow!("browser mailbox: {e}"))
    }
    /// `(state, latest frame, changed)`. Upload only when changed is true.
    pub fn poll(&mut self) -> Option<(State, Option<Frame>, bool)> {
        let s = self.shared.try_lock().ok()?;
        let seq = s.frame.as_ref().map_or(0, |f| f.sequence);
        let changed = seq != self.seen;
        self.seen = seq;
        Some((s.state.clone(), s.frame.clone(), changed))
    }
    /// Whether the engine is up and a navigation starts at once (started ahead of a link: Request 18). False while it
    /// starts, and once it failed or closed.
    pub fn ready(&self) -> bool {
        self.shared.try_lock().is_ok_and(|s| s.ready && !matches!(s.state.load, Load::Failed(_) | Load::Closed))
    }
    /// What the engine noted since the last call (when it started, loaded, first drew; what failed), for a log.
    pub fn notes(&self) -> Vec<String> {
        self.shared.try_lock().map(|mut s| std::mem::take(&mut s.notes)).unwrap_or_default()
    }
    /// Signals shutdown without joining on the overlay thread. Poll until Closed for cleanup completion.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}
/// Page-controlled display text has a UTF-8 byte budget and no control/bidi formatting.
fn display_text(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(max.min(s.len()));
    for c in s.chars() {
        if c.is_control() || matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}
impl Drop for WebPage {
    fn drop(&mut self) {
        self.close();
    }
}
fn size(w: u32, h: u32) -> anyhow::Result<()> {
    anyhow::ensure!((64..=1920).contains(&w) && (64..=1920).contains(&h), "page size must be 64..1920 pixels");
    Ok(())
}
pub(crate) fn validate(s: &str) -> anyhow::Result<()> {
    anyhow::ensure!(s.len() <= 8192, "page URL exceeds 8192 bytes");
    let u = url::Url::parse(s)?;
    anyhow::ensure!(
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some() && u.username().is_empty() && u.password().is_none(),
        "only http/https without credentials is allowed"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_local_and_external_protocols_and_credentials() {
        for u in ["file:///C:/secret", "javascript:alert(1)", "vrchat://launch", "https://user:pw@example.org", "data:text/html,hi"] {
            assert!(validate(u).is_err(), "{u}");
        }
        assert!(validate("http://127.0.0.1:1234/long").is_ok());
    }
    #[test]
    fn bounds_memory() {
        assert!(size(960, 720).is_ok());
        assert!(size(u32::MAX, 720).is_err());
        assert!(size(0, 0).is_err());
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn hostile_text_is_bounded_before_repeated_polls() {
        let state = State {
            title: display_text(&"\u{754c}".repeat(2_000_000), 512),
            url: display_text(&"x".repeat(20000), 8192),
            load: Load::Failed(display_text(&"x".repeat(2000000), 1024)),
        };
        for _ in 0..10000 {
            let s = state.clone();
            assert!(s.title.len() <= 512 && s.url.len() <= 8192);
        }
        assert_eq!(display_text("a\n\u{202e}\u{754c}", 4), "a\u{754c}");
    }
}

#[cfg(test)]
mod live_review_tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn wait(page: &mut WebPage, predicate: impl Fn(&State, &Option<Frame>) -> bool) {
        let t = Instant::now();
        loop {
            if let Some((s, f, _)) = page.poll() {
                if predicate(&s, &f) {
                    return;
                }
                if let Load::Failed(e) = s.load {
                    panic!("browser: {e}");
                }
            }
            assert!(t.elapsed() < Duration::from_secs(30), "browser timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// A page of this computer's own (served by the test) whose words show at once and whose last picture takes `slow`:
    /// its load event waits for that picture. Returns its address.
    fn slow_page(slow: Duration) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = Vec::new();
                    let mut b = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut b) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&b[..n]),
                        }
                    }
                    let first = String::from_utf8_lossy(&request).lines().next().unwrap_or_default().to_string();
                    let (kind, body): (&str, Vec<u8>) = if first.contains("/slow.png") {
                        std::thread::sleep(slow);
                        ("image/png", Vec::new())
                    } else {
                        ("text/html", b"<!doctype html><body style='margin:0;background:#2a6'><h1 style='font:72px sans-serif;color:#fff;margin:40px'>First paint</h1><img src='/slow.png' width=10 height=10></body>".to_vec())
                    };
                    let _ = write!(stream, "HTTP/1.0 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(&body);
                });
            }
        });
        format!("http://{address}/")
    }

    /// From a navigation asked at `from`: when its first picture showed and when it was loaded (ms).
    fn timeline(page: &mut WebPage, from: Instant) -> (u128, u128) {
        let (mut first, mut loaded) = (None, None);
        while first.is_none() || loaded.is_none() {
            if let Some((s, f, _)) = page.poll() {
                if let Load::Failed(e) = &s.load {
                    panic!("browser: {e}");
                }
                if f.is_some() && first.is_none() {
                    first = Some(from.elapsed().as_millis());
                }
                if s.load == Load::Loaded && loaded.is_none() {
                    loaded = Some(from.elapsed().as_millis());
                }
            }
            assert!(from.elapsed() < Duration::from_secs(40), "no picture or no load: {first:?} {loaded:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
        (first.unwrap(), loaded.unwrap())
    }

    /// Request 18 (Enzo: "the page linked by the QR code often takes a while to load"): a page shows at its first paint, not
    /// at its load event (here 4 s later, a slow picture); an engine started ahead (while the camera looks for a code) shows
    /// a link's page without the engine's own start, which a cold one pays after the code is read.
    #[test]
    fn a_slow_page_shows_at_its_first_paint_and_a_warm_engine_skips_its_start() {
        let url = slow_page(Duration::from_secs(4));
        // Cold, as 0.3.8-dev.1 opened every link: the engine starts when the code is read.
        let read = Instant::now();
        let mut cold = WebPage::new_reaching_loopback(820, 940).unwrap();
        cold.navigate(&url).unwrap();
        let (cold_first, cold_loaded) = timeline(&mut cold, read);
        // Warm: started ahead; the code is read once it is ready.
        let mut warm = WebPage::new_reaching_loopback(820, 940).unwrap();
        let started = Instant::now();
        while !warm.ready() {
            assert!(started.elapsed() < Duration::from_secs(40), "the engine never got ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        let ready_after = started.elapsed().as_millis();
        let read = Instant::now();
        warm.navigate(&url).unwrap();
        let (warm_first, warm_loaded) = timeline(&mut warm, read);
        let notes = warm.notes();
        println!("cold engine: first picture {cold_first} ms after the code was read, loaded {cold_loaded} ms");
        println!("warm engine (ready {ready_after} ms after it was started): first picture {warm_first} ms after the code was read, loaded {warm_loaded} ms");
        println!("warm engine's notes: {notes:?}");
        assert!(warm_loaded >= 3900, "the slow picture holds the load event: {warm_loaded} ms");
        assert!(warm_first + 2500 < warm_loaded, "the page shows at its first paint, seconds before its load event: {warm_first} vs {warm_loaded} ms");
        assert!(warm_first + 300 < cold_first, "a warm engine saves its start: {warm_first} vs {cold_first} ms");
        cold.close();
        warm.close();
    }

    #[test]
    fn runtime_hostile_title_subresources_resize_and_async_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let mut page = WebPage::new(640, 480).unwrap();
        page.send(Command::Fixture(format!(r#"<body style='margin:0;background:#135'><script>document.title='x'.repeat(4000000);fetch('http://{address}/fetch').catch(()=>{{}});new WebSocket('ws://{address}/ws');new WebSocket('wss://{address}/wss');let worker=new Worker(URL.createObjectURL(new Blob(["fetch('http://{address}/worker')"],{{type:'text/javascript'}})));</script><img src='http://{address}/image'><iframe src='http://{address}/frame'></iframe>"#))).unwrap();
        // Pictures come as JPEG: a flat colour within a few levels.
        let near = |a: &[u8], b: [u8; 3]| a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 4);
        wait(&mut page, |s, f| s.title.len() == 512 && f.as_ref().is_some_and(|f| f.width == 640 && near(&f.rgba[400 * 640 * 4..400 * 640 * 4 + 3], [17, 51, 85])));
        for _ in 0..10000 {
            if let Some((s, _, _)) = page.poll() {
                assert_eq!(s.title.len(), 512);
            }
        }
        page.resize(320, 240).unwrap();
        wait(&mut page, |_, f| f.as_ref().is_some_and(|f| f.width == 320 && f.height == 240));
        std::thread::sleep(Duration::from_millis(500));
        assert!(listener.accept().is_err(), "private subresource reached listener");
        page.send(Command::FailCdp).unwrap();
        let t = Instant::now();
        loop {
            if let Some((s, _, _)) = page.poll() {
                if matches!(s.load, Load::Failed(_)) {
                    break;
                }
            }
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
