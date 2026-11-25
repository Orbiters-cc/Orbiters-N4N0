//! The game's window on a Mac: the engine's window server (`wsd`) of the person's own VRChat, which of its windows is the game's, and where its picture lies on the
//! screen. Only macOS's own lists are read (processes, windows): nothing of the game's process is opened.

use crate::bridge::mac::{Frame, MacWindow, TITLE_BAR_POINTS};
use egui::{pos2, vec2, Pos2, Vec2};

/// The game's window among the window server `owner`'s: the largest on screen at least 320 x 200 points, in a window (layer 0, or 3 while the game asked to be topmost)
/// or over the whole screen with the menu bar and the Dock under it (layer 25 while the game is in front). Never the engine's 400 x 225 splash. (The VRChat bridge's
/// own rule, `bridge::mac::game_window`, knows the bridge's windows alone: layer 0.)
pub fn pick(windows: &[MacWindow], owner: u32) -> Option<MacWindow> {
    windows
        .iter()
        .filter(|w| w.owner == owner && w.on_screen && (0..=25).contains(&w.layer))
        .filter(|w| w.width >= 320.0 && w.height >= 200.0 + TITLE_BAR_POINTS && !(w.width == 400.0 && w.height == 225.0))
        .max_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height)))
        .copied()
}

/// The game's window as it lies now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Game {
    pub window: MacWindow,
    /// Backing pixels per point of its display.
    pub scale: f64,
    /// Its title bar, in points.
    pub title: f64,
}

impl Game {
    pub fn of(window: MacWindow, scale: f64, display: Option<Frame>) -> Self {
        Self { window, scale: scale.max(0.5), title: crate::bridge::mac::title_points(&window, display) }
    }

    /// The same, its picture where the engine says it is (`frame`, in points of macOS's global coordinates): in the window, as wide as it, at its foot. Anything else it says
    /// is of another moment than the window's.
    pub fn told(self, frame: [f64; 4]) -> Self {
        let [x, y, width, height] = frame;
        let title = self.window.height - height;
        let off = [x - self.window.x, y - self.window.y - title, width - self.window.width];
        if (0.0..=80.0).contains(&title) && height >= 1.0 && off.iter().all(|d| d.abs() <= 1.0) {
            Self { title, ..self }
        } else {
            self
        }
    }

    /// The size of its picture in the display's pixels: what the parts' places are in, as VRChat's client area is on Windows. (What the game itself draws is smaller
    /// below a render percent of 100: macOS stretches it over this.)
    pub fn client(&self) -> Vec2 {
        vec2((self.window.width * self.scale).round() as f32, ((self.window.height - self.title) * self.scale).round() as f32)
    }

    /// The top left corner of its picture, in points of macOS's global coordinates (from the main display's top left, y down).
    pub fn origin(&self) -> (f64, f64) {
        (self.window.x, self.window.y + self.title)
    }

    /// Where a point of macOS's global coordinates is in its picture, in pixels.
    pub fn pixel(&self, at: (f64, f64)) -> Pos2 {
        let (x, y) = self.origin();
        pos2(((at.0 - x) * self.scale) as f32, ((at.1 - y) * self.scale) as f32)
    }

    /// Where a pixel of its picture is, in macOS's global coordinates.
    pub fn point(&self, at: Pos2) -> (f64, f64) {
        let (x, y) = self.origin();
        (x + f64::from(at.x) / self.scale, y + f64::from(at.y) / self.scale)
    }
}

/// How large a picture of the window is asked for, and what of it is the game's: the whole frame, title bar included, at most [`LOOKED_AT_HIGH`] pixels high. The detectors
/// were measured on frames 540 to 1080 high, and what they look for scales with the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub width: u32,
    pub height: u32,
    /// The title bar's rows, at its top.
    pub title: u32,
}

/// The height a picture is asked at, at most.
pub const LOOKED_AT_HIGH: f64 = 1080.0;

impl Shot {
    pub fn of(game: &Game) -> Self {
        let content = (game.window.height - game.title).max(1.0);
        let k = game.scale.min(LOOKED_AT_HIGH / content);
        let (width, height) = ((game.window.width * k).round().max(2.0) as u32, (game.window.height * k).round().max(2.0) as u32);
        Self { width, height, title: crate::bridge::mac::title_rows(height, game.window.height, game.title) }
    }

    /// The size of the game's own picture in it.
    pub fn client(&self) -> Vec2 {
        vec2(self.width as f32, self.height.saturating_sub(self.title) as f32)
    }
}

/// The children of process `pid`.
fn children(pid: u32) -> Vec<u32> {
    let mut list = vec![0i32; 256];
    // SAFETY: the buffer and its size in bytes are ours; the kernel writes at most that many.
    let n = unsafe { libc::proc_listchildpids(pid as i32, list.as_mut_ptr().cast(), (list.len() * std::mem::size_of::<i32>()) as i32) };
    list.truncate(n.max(0) as usize);
    list.into_iter().filter_map(|p| u32::try_from(p).ok()).filter(|p| *p > 1).collect()
}

fn is_window_server(pid: u32) -> bool {
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is ours and its size is said.
    let n = unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    n > 0 && path[..n as usize].ends_with(b"/wsd")
}

/// The engine's window server of the game whose process the service named (`launcher`): it, or a child of it, at most three down.
pub fn window_server(launcher: u32) -> Option<u32> {
    let mut level = vec![launcher];
    for _ in 0..4 {
        if let Some(found) = level.iter().copied().find(|p| is_window_server(*p)) {
            return Some(found);
        }
        level = level.into_iter().flat_map(children).collect();
    }
    None
}

/// The window server followed, and its game's window: looked for at most this often while there is none (the game starting, hidden).
const LOOK_FOR: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Default)]
pub struct Finder {
    launcher: Option<u32>,
    server: Option<u32>,
    looked: Option<std::time::Instant>,
    last: Option<Game>,
}

impl Finder {
    /// The window server of the game the service names, if it is there.
    pub fn server(&self) -> Option<u32> {
        self.server
    }

    /// The game's window now, as macOS lists it; `launcher` is the service's word, `told` the engine's of its window and where its picture lies, when it gives it. With no
    /// window, asked anew at most twice a second.
    pub fn find(&mut self, launcher: Option<u32>, told: Option<(u32, [f64; 4])>) -> Option<Game> {
        let found = self.listed(launcher, told.map(|(window, _)| window));
        match told {
            Some((window, frame)) => found.map(|g| if g.window.id == window { g.told(frame) } else { g }),
            None => found,
        }
    }

    fn listed(&mut self, launcher: Option<u32>, told: Option<u32>) -> Option<Game> {
        if self.launcher != launcher {
            *self = Self { launcher, ..Self::default() };
        }
        let launcher = launcher?;
        let now = std::time::Instant::now();
        // The engine's word of which window is its game's: that one, no other of its server's.
        if told.is_some_and(|id| id != 0 && self.last.is_some_and(|l| l.window.id != id)) {
            (self.last, self.looked) = (None, None);
        }
        if let Some(last) = self.last {
            // The one window, asked alone: far less than the whole list.
            match crate::bridge::mac::window(last.window.id).filter(|w| Some(w.owner) == self.server && w.on_screen) {
                Some(w) if pick(&[w], w.owner).is_some() => {
                    let same = (w.x, w.y, w.width, w.height) == (last.window.x, last.window.y, last.window.width, last.window.height);
                    // (a display's scale may change under a window that stays: it is asked again a few times a second)
                    if same && self.looked.is_some_and(|t| now.duration_since(t) < LOOK_FOR) {
                        self.last = Some(Game { window: w, ..last });
                    } else {
                        self.looked = Some(now);
                        self.last = Some(measured(w));
                    }
                    return self.last;
                }
                _ => self.last = None,
            }
        }
        if self.looked.is_some_and(|t| now.duration_since(t) < LOOK_FOR) {
            return None;
        }
        self.looked = Some(now);
        if self.server.is_none_or(|s| !is_window_server(s)) {
            self.server = window_server(launcher);
        }
        let server = self.server?;
        let windows = crate::bridge::mac::windows();
        let named = told.filter(|id| *id != 0).and_then(|id| windows.iter().find(|w| w.id == id)).and_then(|w| pick(&[*w], server));
        self.last = named.or_else(|| pick(&windows, server)).map(measured);
        self.last
    }
}

fn measured(window: MacWindow) -> Game {
    Game::of(window, crate::bridge::mac::scale_of(&window), crate::bridge::mac::display_frame(&window))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u32, layer: i64, x: f64, y: f64, width: f64, height: f64) -> MacWindow {
        MacWindow { id, owner: 7, x, y, width, height, layer, on_screen: true, alpha: Some(1.0) }
    }

    /// Home's VRChat is at layer 25 while it is in front and 0 behind another app; the splash, strips, the crash box and what macOS does not show are not it.
    #[test]
    fn the_game_s_window_is_found_at_both_of_its_layers() {
        let full = window(1, 25, 0.0, 0.0, 1470.0, 956.0);
        assert_eq!(pick(&[full], 7).map(|w| w.id), Some(1));
        assert_eq!(pick(&[MacWindow { layer: 0, ..full }], 7).map(|w| w.id), Some(1), "behind another app");
        assert_eq!(pick(&[window(2, 0, 535.0, 365.0, 400.0, 225.0)], 7), None, "the splash");
        assert_eq!(pick(&[window(3, 0, 0.0, 0.0, 1470.0, 33.0)], 7), None, "a strip");
        assert_eq!(pick(&[window(4, 0, 0.0, 0.0, 195.0, 138.0)], 7), None, "the crash box");
        assert_eq!(pick(&[MacWindow { on_screen: false, ..full }], 7), None, "minimised, hidden or on another Space");
        assert_eq!(pick(&[MacWindow { owner: 8, ..full }], 7), None, "another program's");
        assert_eq!(pick(&[MacWindow { layer: 1000, ..full }], 7), None);
        assert_eq!(pick(&[window(2, 0, 535.0, 365.0, 400.0, 225.0), full], 7).map(|w| w.id), Some(1));
    }

    /// Points and pixels, full screen (no title bar) and in a window (32 points of it), on a display that is not the main one.
    #[test]
    fn a_pixel_of_the_game_is_where_it_shows_on_the_screen() {
        let display = Some((0.0, 0.0, 1470.0, 956.0));
        let full = Game::of(window(1, 25, 0.0, 0.0, 1470.0, 956.0), 2.0, display);
        assert_eq!((full.title, full.client()), (0.0, vec2(2940.0, 1912.0)));
        assert_eq!(full.point(pos2(2940.0, 0.0)), (1470.0, 0.0));
        assert_eq!(full.pixel((735.0, 478.0)), pos2(1470.0, 956.0));
        let titled = Game::of(window(1, 0, -1800.0, 100.0, 640.0, 392.0), 1.0, Some((-1920.0, 0.0, 1920.0, 1080.0)));
        assert_eq!((titled.title, titled.client()), (32.0, vec2(640.0, 360.0)));
        assert_eq!(titled.origin(), (-1800.0, 132.0));
        assert_eq!(titled.point(pos2(10.0, 20.0)), (-1790.0, 152.0));
        assert_eq!(titled.pixel((-1790.0, 152.0)), pos2(10.0, 20.0));
        assert!(Game::of(window(1, 0, 0.0, 0.0, 640.0, 392.0), 0.0, None).scale > 0.0, "a scale that could not be read divides nothing by zero");
    }

    /// The engine says where its picture is: taken where it fits the window macOS lists, and left where it is another moment's (the window moved since, or is being resized).
    #[test]
    fn the_engine_s_word_of_where_its_picture_is_goes_over_the_title_bar_s_guess() {
        let display = Some((0.0, 0.0, 1470.0, 956.0));
        let game = Game::of(window(1, 0, 50.0, 60.0, 640.0, 388.0), 2.0, display);
        assert_eq!(game.title, 32.0, "a title bar's usual height");
        let told = game.told([50.0, 88.0, 640.0, 360.0]);
        assert_eq!((told.title, told.client(), told.origin()), (28.0, vec2(1280.0, 720.0), (50.0, 88.0)));
        for other in [[50.0, 60.0, 640.0, 0.0], [80.0, 88.0, 640.0, 360.0], [50.0, 88.0, 800.0, 360.0], [50.0, 200.0, 640.0, 248.0], [0.0; 4]] {
            assert_eq!(game.told(other), game, "{other:?}");
        }
        let full = Game::of(window(1, 25, 0.0, 0.0, 1470.0, 956.0), 2.0, display);
        assert_eq!(full.told([0.0, 0.0, 1470.0, 956.0]), full);
    }

    /// The picture asked for: the window's own pixels up to 1080 of the game's rows, its shape kept, its title bar cut off.
    #[test]
    fn the_picture_looked_at_keeps_the_window_s_shape() {
        let display = Some((0.0, 0.0, 1470.0, 956.0));
        for (game, high) in [
            (Game::of(window(1, 25, 0.0, 0.0, 1470.0, 956.0), 2.0, display), 1080.0),
            (Game::of(window(1, 25, 0.0, 0.0, 1920.0, 1080.0), 1.0, Some((0.0, 0.0, 1920.0, 1080.0))), 1080.0),
            (Game::of(window(1, 0, 50.0, 50.0, 640.0, 392.0), 2.0, display), 720.0),
            (Game::of(window(1, 25, 0.0, 0.0, 2560.0, 1440.0), 2.0, Some((0.0, 0.0, 2560.0, 1440.0))), 1080.0),
        ] {
            let shot = Shot::of(&game);
            assert!((shot.client().y - high).abs() <= 1.5, "{shot:?}");
            let (a, b) = (shot.client().x / shot.client().y, game.client().x / game.client().y);
            assert!((a - b).abs() < 0.004, "{shot:?}: {a} {b}");
        }
        assert_eq!(Shot::of(&Game::of(window(1, 0, 50.0, 50.0, 640.0, 392.0), 2.0, display)).title, 64);
    }

    /// The window server is the launcher's child, or the launcher's child's; a program of another name is not it.
    #[test]
    fn the_window_server_is_the_launchers_own() {
        let dir = std::env::temp_dir().join(format!("orbiters-overlay-wsd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("waits.c");
        std::fs::write(&file, "#include <unistd.h>\nint main(void){sleep(30);return 0;}\n").unwrap();
        let server = dir.join("wsd");
        assert!(std::process::Command::new("/usr/bin/cc").arg("-o").arg(&server).arg(&file).status().unwrap().success());
        let mut launcher = std::process::Command::new("/bin/sh").arg("-c").arg(format!("{} & wait", server.display())).spawn().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut found = None;
        while found.is_none() && std::time::Instant::now() < until {
            found = window_server(launcher.id());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let found = found.expect("its child");
        assert_ne!(found, launcher.id());
        assert_eq!(window_server(found), Some(found), "named itself");
        let mut other = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        assert_eq!(window_server(other.id()), None);
        // SAFETY: a signal to this test's own child.
        unsafe { libc::kill(found as i32, libc::SIGKILL) };
        let _ = launcher.kill();
        let _ = launcher.wait();
        let _ = other.kill();
        let _ = other.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
