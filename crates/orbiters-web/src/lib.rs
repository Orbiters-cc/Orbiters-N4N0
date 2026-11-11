//! Private, windowless WebView2 pages for VR overlays. Absent on macOS.
#![cfg(windows)]
mod engine;
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
    pub rgba: Arc<[u8]>,
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
    Navigate(String),
    Input(Input),
    Resize(u32, u32),
}
pub(crate) struct Shared {
    state: State,
    frame: Option<Frame>,
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
        size(width, height)?;
        let (tx, rx) = mpsc::sync_channel(64);
        let shared = Arc::new(Mutex::new(Shared { state: State { load: Load::Loading, url: String::new(), title: String::new() }, frame: None }));
        let s = shared.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let closing = closed.clone();
        std::thread::Builder::new().name("camera-web".into()).spawn(move || {
            if let Err(e) = engine::run(width, height, rx, s.clone(), closing) {
                s.lock().unwrap().state.load = Load::Failed(format!("{e:#}"));
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
    /// Signals shutdown without joining on the overlay thread. Poll until Closed for cleanup completion.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
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
