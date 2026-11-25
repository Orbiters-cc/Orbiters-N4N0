//! What the engine's window server says of the mouse (obelisk-cli 0.1.15, voile-lkd's `wsd/wsd_pointer.h`): whether the game holds the pointer or lets it go, where its
//! window lies, and the raw moves while it holds it. A socket of the host's folder of that very session (`orbiters_core::obelisk::pointer_channel`), which says and takes
//! nothing: nothing is ever written to it, and nothing asked.
//!
//! Read without waiting, once a turn, on the loop's own thread. Its absence (an older engine, a launch that could not make it, one that ended) is the ordinary case of before.

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often a channel that is not there is tried.
const AGAIN_EVERY: Duration = Duration::from_secs(2);
/// A message longer than this is not one of the protocol's: the channel is left.
const LONGEST: usize = 4096;

const HELLO: u32 = 1;
const STATE: u32 = 2;
const MOVE: u32 = 3;

/// Who has the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// The game shows its own cursor, or has no window: the mouse is the person's.
    Plain,
    /// The game hides its cursor and does not confine the mouse (VRChat's menus): it is the person's, over the game and over what lies on it.
    Free,
    /// The game holds it: macOS's pointer stands still.
    Held,
    /// The game would hold it but its window has not the keyboard, or shows no frames.
    Away,
}

/// A state of the channel's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub pointer: Pointer,
    /// The game's window has the keyboard.
    pub key: bool,
    /// The game's window at macOS's window server (0: none).
    pub window: u32,
    /// Its picture, in points of macOS's global coordinates.
    pub frame: [f64; 4],
}

/// A move the game was sent while it holds the mouse, in points (y down).
pub type Move = (f32, f32);

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?)).filter(|v| v.is_finite())
}

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?)).filter(|v| v.is_finite())
}

fn state(b: &[u8]) -> Option<State> {
    let pointer = match u32_at(b, 0)? {
        0 => Pointer::Plain,
        1 => Pointer::Free,
        2 => Pointer::Held,
        3 => Pointer::Away,
        // A later engine's, which this does not know: as no word of it at all.
        _ => return None,
    };
    Some(State { pointer, key: u32_at(b, 4)? & 1 != 0, window: u32_at(b, 8)?, frame: [f64_at(b, 16)?, f64_at(b, 24)?, f64_at(b, 32)?, f64_at(b, 40)?] })
}

/// What was read and not yet taken apart.
#[derive(Default)]
struct Words {
    buffer: Vec<u8>,
    /// The window server's process, as it said at the start.
    server: Option<u32>,
    state: Option<State>,
    moves: Vec<Move>,
}

impl Words {
    /// Takes the whole messages apart; `false` when what comes is not the protocol.
    fn take(&mut self) -> bool {
        let mut at = 0;
        while let (Some(kind), Some(len)) = (u32_at(&self.buffer, at), u32_at(&self.buffer, at + 4)) {
            let len = len as usize;
            if len > LONGEST || self.server.is_none() && kind != HELLO {
                return false;
            }
            let Some(body) = self.buffer.get(at + 8..at + 8 + len) else { break };
            match kind {
                HELLO => match (u32_at(body, 0), u32_at(body, 4)) {
                    (Some(1..), Some(server)) => self.server = Some(server),
                    _ => return false,
                },
                STATE => self.state = state(body),
                MOVE => {
                    if let (Some(x), Some(y)) = (f32_at(body, 0), f32_at(body, 4)) {
                        self.moves.push((x, y));
                    }
                }
                _ => {}
            }
            at += 8 + len;
        }
        self.buffer.drain(..at);
        true
    }
}

/// The folder and the socket are this user's alone, no link, and who answers is this user's.
fn own(path: &Path) -> bool {
    // SAFETY: a plain query.
    let me = unsafe { libc::getuid() };
    let is = |p: &Path| std::fs::symlink_metadata(p).ok().filter(|m| m.uid() == me);
    path.parent().and_then(is).is_some_and(|m| m.is_dir() && m.permissions().mode() & 0o077 == 0) && is(path).is_some_and(|m| !m.is_symlink())
}

fn connect(path: &Path) -> Option<UnixStream> {
    if !own(path) {
        return None;
    }
    let socket = UnixStream::connect(path).ok()?;
    socket.set_nonblocking(true).ok()?;
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: plain queries into this call's own variables.
    let mine = unsafe { libc::getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == libc::getuid() };
    mine.then_some(socket)
}

#[derive(Default)]
pub struct Channel {
    path: Option<PathBuf>,
    socket: Option<UnixStream>,
    words: Words,
    tried: Option<Instant>,
}

impl Channel {
    /// Reads what came. `path`: the session's channel, when its game runs; `server`: the window server of that game's, which must be the one that speaks (the folder is named
    /// after a process, and a number may come again).
    pub fn turn(&mut self, path: Option<&Path>, server: Option<u32>, now: Instant) -> Vec<Move> {
        if self.path.as_deref() != path {
            *self = Self { path: path.map(Path::to_path_buf), ..Self::default() };
        }
        let Some(path) = path else { return Vec::new() };
        if self.socket.is_none() {
            if server.is_none() || self.tried.is_some_and(|t| now.duration_since(t) < AGAIN_EVERY) {
                return Vec::new();
            }
            self.tried = Some(now);
            self.words = Words::default();
            self.socket = connect(path);
        }
        if !self.read() || self.words.server.is_some_and(|s| server.is_some_and(|mine| mine != s)) {
            self.socket = None;
            self.words = Words::default();
        }
        std::mem::take(&mut self.words.moves)
    }

    /// `false` once the channel is over.
    fn read(&mut self) -> bool {
        let Some(socket) = self.socket.as_mut() else { return true };
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match socket.read(&mut chunk) {
                Ok(0) => return false,
                Ok(n) => {
                    self.words.buffer.extend_from_slice(&chunk[..n]);
                    if !self.words.take() {
                        return false;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
    }

    /// What the engine says of the mouse now; `None` without the channel.
    pub fn state(&self) -> Option<State> {
        self.socket.as_ref().and(self.words.state)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    fn message(kind: u32, body: &[u8]) -> Vec<u8> {
        [&kind.to_le_bytes()[..], &(body.len() as u32).to_le_bytes(), body].concat()
    }

    fn hello(server: u32) -> Vec<u8> {
        message(HELLO, &[1u32.to_le_bytes(), server.to_le_bytes()].concat())
    }

    /// `struct wsd_pointer_state` as the engine lays it.
    fn state_of(pointer: u32, flags: u32, window: u32, frame: [f64; 4]) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [pointer, flags, window, 4242] {
            b.extend(v.to_le_bytes());
        }
        for v in frame.into_iter().chain([0.9, 0.9]) {
            b.extend(v.to_le_bytes());
        }
        b.extend([0u8; 16]);
        b.extend(7u64.to_le_bytes());
        assert_eq!(b.len(), 88, "wsd_pointer.h");
        message(STATE, &b)
    }

    fn moved(x: f32, y: f32) -> Vec<u8> {
        message(MOVE, &[&x.to_le_bytes()[..], &y.to_le_bytes(), &9u64.to_le_bytes()].concat())
    }

    /// A window server of a test's, in a folder of this user's alone.
    pub struct Server {
        pub dir: PathBuf,
        listener: UnixListener,
        peer: Option<UnixStream>,
    }

    impl Server {
        pub fn new(name: &str) -> Self {
            // (a socket's path has 104 bytes)
            let dir = std::env::temp_dir().join(format!("orbc{}{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            let listener = UnixListener::bind(dir.join("pointer.sock")).unwrap();
            Self { dir, listener, peer: None }
        }

        pub fn path(&self) -> PathBuf {
            self.dir.join("pointer.sock")
        }

        fn accept(&mut self) {
            self.peer = Some(self.listener.accept().unwrap().0);
        }

        fn say(&mut self, bytes: &[u8]) {
            self.peer.as_mut().unwrap().write_all(bytes).unwrap();
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Who has the mouse, where the window is, and the moves; a message cut anywhere is waited for, and one of a later engine's is passed over.
    #[test]
    fn the_engines_word_is_read_whole_however_it_comes() {
        let mut server = Server::new("a");
        let (mut channel, t) = (Channel::default(), Instant::now());
        assert!(channel.turn(Some(&server.path()), Some(77), t).is_empty());
        server.accept();
        assert_eq!(channel.state(), None, "nothing said yet");
        let all = [hello(77), state_of(2, 1 | 2, 91, [10.0, 20.0, 1470.0, 956.0]), message(9, &[1, 2, 3]), moved(3.0, -4.5), moved(1.0, 0.0)].concat();
        let mut moves = Vec::new();
        for piece in all.chunks(11) {
            server.say(piece);
            moves.extend(channel.turn(Some(&server.path()), Some(77), t));
        }
        assert_eq!(moves, [(3.0, -4.5), (1.0, 0.0)]);
        assert_eq!(channel.state(), Some(State { pointer: Pointer::Held, key: true, window: 91, frame: [10.0, 20.0, 1470.0, 956.0] }));
        assert!(channel.turn(Some(&server.path()), Some(77), t).is_empty(), "each move once");
        for (code, pointer) in [(0, Pointer::Plain), (1, Pointer::Free), (3, Pointer::Away)] {
            server.say(&state_of(code, 0, 91, [0.0; 4]));
            channel.turn(Some(&server.path()), Some(77), t);
            assert_eq!(channel.state().map(|s| (s.pointer, s.key)), Some((pointer, false)));
        }
        server.say(&state_of(12, 0, 91, [0.0; 4]));
        channel.turn(Some(&server.path()), Some(77), t);
        assert_eq!(channel.state(), None, "a word this does not know is none at all");
    }

    /// The session ended, or another's took its number, or what speaks is not the protocol: no word of the mouse, and it is tried again in a while.
    #[test]
    fn a_channel_that_goes_says_nothing_and_is_tried_again() {
        let mut server = Server::new("b");
        let (mut channel, t) = (Channel::default(), Instant::now());
        channel.turn(Some(&server.path()), Some(77), t);
        server.accept();
        server.say(&[hello(77), state_of(1, 1, 5, [0.0; 4])].concat());
        channel.turn(Some(&server.path()), Some(77), t);
        assert!(channel.state().is_some());
        server.peer = None;
        channel.turn(Some(&server.path()), Some(77), t);
        assert_eq!(channel.state(), None, "its end");
        channel.turn(Some(&server.path()), Some(77), t + Duration::from_millis(500));
        assert!(channel.socket.is_none(), "not at every turn");
        channel.turn(Some(&server.path()), Some(77), t + AGAIN_EVERY);
        server.accept();
        server.say(&[hello(78), state_of(1, 1, 5, [0.0; 4])].concat());
        channel.turn(Some(&server.path()), Some(77), t + AGAIN_EVERY);
        assert_eq!(channel.state(), None, "another window server's");
        for (n, wrong) in [state_of(1, 1, 5, [0.0; 4]), message(HELLO, &[0; 8]), [&HELLO.to_le_bytes()[..], &(1u32 << 30).to_le_bytes()].concat()].into_iter().enumerate() {
            let at = t + AGAIN_EVERY * (n as u32 + 2);
            channel.turn(Some(&server.path()), Some(77), at);
            server.accept();
            server.say(&wrong);
            channel.turn(Some(&server.path()), Some(77), at);
            assert!(channel.state().is_none() && channel.socket.is_none(), "{n}");
        }
    }

    /// No folder (an older engine, a game that is not Home's), one others may enter, no window server known: none.
    #[test]
    fn a_channel_that_is_not_this_sessions_own_is_not_taken() {
        let t = Instant::now();
        let mut channel = Channel::default();
        channel.turn(Some(Path::new("/nowhere/pointer.sock")), Some(1), t);
        assert!(channel.socket.is_none());
        let server = Server::new("c");
        channel.turn(Some(&server.path()), None, t);
        assert!(channel.socket.is_none(), "whose it is cannot be told yet");
        channel.turn(Some(&server.path()), Some(1), t);
        assert!(channel.socket.is_some());
        channel.turn(None, Some(1), t);
        assert!(channel.socket.is_none(), "the game ended");
        std::fs::set_permissions(&server.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        channel.turn(Some(&server.path()), Some(1), t);
        assert!(channel.socket.is_none());
    }
}
