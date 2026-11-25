//! VRChat's stream camera on a Mac, where VRChat runs on Obelisk's engine (obelisk-cli 0.1.15): the same [`Receiver`] as on Windows.
//!
//! Spout itself is Windows'. Under the engine, each texture the game shares for it is **published** to programs of the same user by the engine's render server, outside the game:
//! a socket in a folder of that session's alone, which says each texture's number, size and format and hands over a read-only descriptor of memory its finished pictures are
//! copied into (Neptune's `virglrenderer/docs/shared-textures.md` and `npt_publish_wire.h`, which this file follows). Nothing is ever written to it, and nothing of the game is
//! opened.
//!
//! - **Which session** is told ([`set_session`]): the game Home started, named by the engine's own record. Nothing is looked for elsewhere.
//! - **The senders' names** are in Spout's own named memory, plain files of the host while a sender lives. They say what is there without connecting, and so cost the game
//!   nothing: while nobody is connected, the render server copies no picture. An internal of the engine's Wine, though: without them, the textures are named as VRChat names
//!   its streams, in the order it made them.
//! - **A frame** is always a whole one: the rows asked for are copied, and thrown away if the slot they came from was written meanwhile (its number says so).

use crate::pixels::{add_rows, half_to_srgb8_table, missing_rows};
use crate::{formats, ChannelOrder, Frame, Result, SenderInfo, SpoutError};
use orbiters_i18n::tr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where the textures of a session are published, and where Spout's own memory of it lies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// `<host folder>/shared`.
    pub shared: PathBuf,
    /// The folder the engine's guests have for `/tmp`.
    pub guest_tmp: PathBuf,
}

impl Session {
    /// That of the game whose engine's launcher has the host folder `host`.
    pub fn of(host: &Path) -> Self {
        // SAFETY: a plain query.
        let uid = unsafe { libc::getuid() };
        let guest_tmp = std::env::var_os("LKD_HOST_TMP").map_or_else(|| host.parent().unwrap_or(host).join(format!("lkd-{uid}/tmp")), PathBuf::from);
        Self { shared: host.join("shared"), guest_tmp }
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// The session whose senders are read from now on: the running game of Home's, or none. A receiver connected to another one's loses its sender.
pub fn set_session(session: Option<Session>) {
    *SESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = session;
}

pub fn session() -> Option<Session> {
    SESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The largest picture received, as on Windows: 8K.
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 7680 * 4320;
const SLOTS: u32 = 4;
const HEADER: usize = 16384;
const MAGIC: u32 = 0x4654_504e;
/// A message longer than this is not one of the protocol's.
const LONGEST: usize = 4096;
/// How long the render server is given to say what it publishes (it says it as soon as it accepts).
const ANSWERS_WITHIN: Duration = Duration::from_millis(700);
/// Without Spout's own memory to say what is there, it is asked by connecting, this often at most.
const ASK_EVERY: Duration = Duration::from_secs(10);
/// How often the rows of a slot written meanwhile are taken anew before this frame is given up.
const TRIES: u32 = 4;

fn platform(context: &str, e: impl std::fmt::Display) -> SpoutError {
    SpoutError::Platform(format!("{context}: {e}"))
}

// ---------------------------------------------------------------------------------------------- the socket

/// This user's, and nobody else may go in or write there.
fn own(path: &Path, dir: bool) -> bool {
    // SAFETY: a plain query.
    let me = unsafe { libc::getuid() };
    std::fs::symlink_metadata(path).is_ok_and(|m| m.uid() == me && !m.is_symlink() && m.is_dir() == dir && (!dir || m.permissions().mode() & 0o077 == 0))
}

/// The render server's workers that publish: their process and socket. A killed one leaves its file behind.
fn workers(session: &Session) -> Vec<(u32, PathBuf)> {
    if !own(&session.shared, true) {
        return Vec::new();
    }
    let Ok(list) = std::fs::read_dir(&session.shared) else { return Vec::new() };
    let mut out: Vec<(u32, PathBuf)> = list
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.strip_suffix(".sock")?.parse().ok()?;
            let path = e.path();
            // SAFETY: signal 0 only asks whether the process is there.
            let lives = pid > 1 && unsafe { libc::kill(pid as i32, 0) } == 0;
            (lives && own(&path, false)).then_some((pid, path))
        })
        .collect();
    out.sort_unstable();
    out
}

/// A texture as the render server tells of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Texture {
    token: u32,
    order: u32,
    width: u32,
    height: u32,
    format: u32,
    stride: u32,
    map_size: u64,
}

impl Texture {
    fn read(b: &[u8]) -> Option<Self> {
        let w = |i: usize| Some(u32::from_le_bytes(b.get(i * 4..i * 4 + 4)?.try_into().ok()?));
        let it = Self { token: w(0)?, order: w(1)?, width: w(2)?, height: w(3)?, format: w(4)?, stride: w(5)?, map_size: u64::from_le_bytes(b.get(32..40)?.try_into().ok()?) };
        (w(6)? == SLOTS).then_some(it)
    }
}

/// The process of the render server's worker a token was made by (its low 17 bits).
fn worker_of(token: u32) -> Option<u32> {
    (token & 0x4000_0003 == 0x4000_0002).then_some((token >> 13) & 0x1_ffff)
}

/// A connection to one worker: while it is there, the game's finished pictures are copied for it.
struct Link {
    socket: UnixStream,
    worker: u32,
    buffer: Vec<u8>,
    passed: std::collections::VecDeque<OwnedFd>,
    greeted: bool,
    textures: Vec<(Texture, OwnedFd)>,
    over: bool,
}

impl Link {
    fn open(worker: u32, path: &Path) -> std::io::Result<Self> {
        let socket = UnixStream::connect(path)?;
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: plain queries into this call's own variables.
        if unsafe { libc::getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) != 0 || uid != libc::getuid() } {
            return Err(std::io::Error::other("another user's")); // i18n: not shown (an error detail)
        }
        socket.set_nonblocking(true)?;
        Ok(Self { socket, worker, buffer: Vec::new(), passed: Default::default(), greeted: false, textures: Vec::new(), over: false })
    }

    /// Waits for what the render server says first: its textures. (It writes them as it accepts; a texture made at that very moment comes a little later.)
    fn greet(&mut self) {
        let until = Instant::now() + ANSWERS_WITHIN;
        loop {
            self.hear();
            let left = until.saturating_duration_since(Instant::now());
            if self.over || left.is_zero() {
                return;
            }
            let mut p = libc::pollfd { fd: self.socket.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // Once greeted, the textures follow in the same breath: until it says no more. (The first one's memory may have to be made first.)
            let wait = match (self.greeted, self.textures.is_empty()) {
                (false, _) => left,
                (true, true) => left.min(Duration::from_millis(60)),
                (true, false) => left.min(Duration::from_millis(15)),
            };
            // SAFETY: one descriptor of this link's own.
            let n = unsafe { libc::poll(&mut p, 1, wait.as_millis().max(1) as i32) };
            if n == 0 && self.greeted {
                return;
            }
        }
    }

    /// Takes what was said since, without waiting.
    fn hear(&mut self) {
        while !self.over {
            let mut chunk = [0u8; 8192];
            let mut control = [0u64; 32];
            let mut part = libc::iovec { iov_base: chunk.as_mut_ptr().cast(), iov_len: chunk.len() };
            // SAFETY: a header of zeros is a valid one; the buffers are this call's own and their sizes are said.
            let n = unsafe {
                let mut message: libc::msghdr = std::mem::zeroed();
                message.msg_iov = &mut part;
                message.msg_iovlen = 1;
                message.msg_control = control.as_mut_ptr().cast();
                message.msg_controllen = std::mem::size_of_val(&control) as _;
                let n = libc::recvmsg(self.socket.as_raw_fd(), &mut message, 0);
                if n > 0 {
                    let mut c = libc::CMSG_FIRSTHDR(&message);
                    while !c.is_null() {
                        if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                            let data = libc::CMSG_DATA(c);
                            let bytes = ((*c).cmsg_len as usize).saturating_sub(data as usize - c as usize);
                            for i in 0..bytes / std::mem::size_of::<i32>() {
                                let fd = std::ptr::read_unaligned(data.cast::<i32>().add(i));
                                if fd >= 0 {
                                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                                    self.passed.push_back(OwnedFd::from_raw_fd(fd));
                                }
                            }
                        }
                        c = libc::CMSG_NXTHDR(&message, c);
                    }
                }
                n
            };
            if n < 0 {
                match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::Interrupted => continue,
                    std::io::ErrorKind::WouldBlock => return,
                    _ => self.over = true,
                }
            } else if n == 0 {
                self.over = true;
            } else {
                self.buffer.extend_from_slice(&chunk[..n as usize]);
                if !self.take() {
                    self.over = true;
                }
            }
        }
    }

    /// Takes the whole messages apart; `false` when what comes is not the protocol.
    fn take(&mut self) -> bool {
        let mut at = 0;
        let word = |b: &[u8], i: usize| b.get(i..i + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
        while let (Some(kind), Some(len)) = (word(&self.buffer, at), word(&self.buffer, at + 4)) {
            if len as usize > LONGEST || !self.greeted && kind != 1 {
                return false;
            }
            let Some(body) = self.buffer.get(at + 8..at + 8 + len as usize) else { break };
            match kind {
                // HELLO: of the version this follows (a later one keeps what it knows), and of the worker the file is named after.
                1 => match (word(body, 0), word(body, 4)) {
                    (Some(1..), Some(worker)) if worker == self.worker => self.greeted = true,
                    _ => return false,
                },
                2 => {
                    let (Some(texture), Some(fd)) = (Texture::read(body), self.passed.pop_front()) else { return false };
                    self.textures.retain(|(t, _)| t.token != texture.token);
                    self.textures.push((texture, fd));
                }
                3 => {
                    let gone = word(body, 0);
                    self.textures.retain(|(t, _)| Some(t.token) != gone);
                }
                _ => {}
            }
            at += 8 + len as usize;
        }
        self.buffer.drain(..at);
        true
    }
}

// ---------------------------------------------------------------------------------------------- Spout's own memory

/// A sender's info block, as Spout lays it: which texture it is of.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Info {
    token: u32,
    width: u32,
    height: u32,
    format: u32,
    description: String,
    /// When its file was made: blocks of several senders are told apart by which came first.
    born: u64,
}

fn text(b: &[u8]) -> Option<String> {
    let end = b.iter().position(|c| *c == 0)?;
    // (a name is the game's own ASCII; anything else is another mapping of another program's)
    (end > 0 && b[..end].iter().all(|c| (0x20..0x7f).contains(c))).then(|| String::from_utf8_lossy(&b[..end]).into_owned())
}

/// The names of Spout's list (256 bytes each), when that is what this memory is: every one a name or nothing, whole.
fn names_in(b: &[u8]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for slot in b.chunks_exact(256) {
        if slot[0] == 0 {
            continue;
        }
        out.push(text(slot)?);
    }
    (!out.is_empty()).then_some(out)
}

fn info_in(b: &[u8], born: u64) -> Option<Info> {
    let w = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().expect("four bytes"));
    if b.len() < 280 {
        return None;
    }
    let (token, width, height, format) = (w(0), w(1), w(2), w(3));
    worker_of(token)?;
    ((1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height)).then(|| Info { token, width, height, format, description: text(&b[20..276]).unwrap_or_default(), born })
}

/// What Spout's memory says in each of the servers' folders of `guest_tmp`: the senders' names, and the info blocks. Small files alone are looked into.
fn blocks(guest_tmp: &Path) -> Vec<(Vec<String>, Vec<Info>)> {
    use std::io::Read;
    let mut out = Vec::new();
    for wine in std::fs::read_dir(guest_tmp).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".wine-")) {
        for server in std::fs::read_dir(wine.path()).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("server-")) {
            let (mut names, mut infos) = (Vec::new(), Vec::new());
            for file in std::fs::read_dir(server.path()).into_iter().flatten().flatten() {
                if !file.file_name().to_string_lossy().starts_with("tmpmap-") {
                    continue;
                }
                let Ok(meta) = file.metadata() else { continue };
                if !meta.is_file() || meta.len() != 4096 && meta.len() != 16384 {
                    continue;
                }
                let mut bytes = Vec::new();
                let Ok(open) = std::fs::File::open(file.path()) else { continue };
                if open.take(16384).read_to_end(&mut bytes).is_err() {
                    continue;
                }
                let born = meta.created().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
                if let Some(info) = info_in(&bytes, born) {
                    infos.push(info);
                } else if let Some(list) = names_in(&bytes) {
                    for name in list.into_iter().filter(|n| is_a_senders(n)) {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
            if !infos.is_empty() {
                out.push((names, infos));
            }
        }
    }
    out
}

/// What VRChat calls its streams ([`orbiters_core` knows the same]: `VRCSender1` to `4`); a sender of another name is another program's of that session.
fn is_a_senders(name: &str) -> bool {
    !name.is_empty() && name.len() < 256 && !name.contains(['\\', '/', '{'])
}

/// A sender there is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Listed {
    name: String,
    token: u32,
    info: SenderInfo,
}

fn listed(name: String, info: &Info) -> Listed {
    Listed { token: info.token, info: SenderInfo { name: name.clone(), width: info.width, height: info.height, format: info.format, share_handle: info.token, description: info.description.clone() }, name }
}

/// VRChat's own names, in the order it made them, for textures whose names cannot be read.
fn by_order(mut textures: Vec<Texture>) -> Vec<Listed> {
    textures.sort_unstable_by_key(|t| t.order);
    textures
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let name = format!("VRCSender{}", i + 1);
            Listed { token: t.token, info: SenderInfo { name: name.clone(), width: t.width, height: t.height, format: t.format, share_handle: t.token, description: String::new() }, name }
        })
        .collect()
}

/// The senders of `workers` as Spout's own memory says them, `known` being the textures published (when that was asked): each block with the name that goes with it. With one
/// name and one block that is certain; with several, they go in the order the senders were made (a block's file is made with its sender, and stays as its texture changes).
fn named(found: Vec<(Vec<String>, Vec<Info>)>, workers: &[u32], known: Option<&[Texture]>) -> Vec<Listed> {
    let mut out = Vec::new();
    for (mut names, mut infos) in found {
        infos.retain(|i| match known {
            Some(known) => known.iter().any(|t| t.token == i.token),
            None => worker_of(i.token).is_some_and(|w| workers.iter().any(|pid| pid & 0x1_ffff == w)),
        });
        // (several blocks of one texture: one sender's, named twice by a program that opened it)
        infos.sort_unstable_by_key(|i| (i.born, i.token));
        let mut seen = Vec::new();
        infos.retain(|i| !seen.contains(&i.token) && {
            seen.push(i.token);
            true
        });
        names.sort_unstable();
        // VRChat's own first: with them there, another name is no stream of its.
        if names.iter().filter(|n| n.starts_with("VRCSender")).count() >= infos.len() {
            names.retain(|n| n.starts_with("VRCSender"));
        }
        if names.len() < infos.len() {
            names = (1..=infos.len()).map(|i| format!("VRCSender{i}")).collect();
        }
        out.extend(infos.iter().zip(names).map(|(i, name)| listed(name, i)));
    }
    out
}

/// What was last learned by connecting, when Spout's own memory said nothing: for which workers, when, and what.
type Asked = (Session, Vec<u32>, Instant, Vec<Listed>);
static ASKED: Mutex<Option<Asked>> = Mutex::new(None);

fn senders() -> Vec<Listed> {
    let Some(session) = session() else { return Vec::new() };
    let workers = workers(&session);
    if workers.is_empty() {
        return Vec::new();
    }
    let pids: Vec<u32> = workers.iter().map(|w| w.0).collect();
    let found = named(blocks(&session.guest_tmp), &pids, None);
    if !found.is_empty() {
        return found;
    }
    // A worker publishes and no block of Spout's is there: no sender, or an engine whose Wine keeps them elsewhere. Asked of the worker itself, seldom.
    let mut asked = ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((of, on, at, list)) = asked.as_ref() {
        if *of == session && *on == pids && at.elapsed() < ASK_EVERY {
            return list.clone();
        }
    }
    let list = by_order(published(&workers));
    *asked = Some((session, pids, Instant::now(), list.clone()));
    list
}

fn published(workers: &[(u32, PathBuf)]) -> Vec<Texture> {
    workers
        .iter()
        .filter_map(|(pid, path)| Link::open(*pid, path).ok())
        .flat_map(|mut link| {
            link.greet();
            link.textures.iter().map(|(t, _)| *t).collect::<Vec<_>>()
        })
        .collect()
}

/// Every sender of the session, by name.
pub fn list_senders() -> Vec<String> {
    senders().into_iter().map(|s| s.name).collect()
}

/// The one that counts as chosen: VRChat's camera's.
pub fn active_sender() -> Option<String> {
    list_senders().into_iter().next()
}

/// What a sender says of itself.
pub fn sender_info(name: &str) -> Option<SenderInfo> {
    senders().into_iter().find(|s| s.name == name).map(|s| s.info)
}

// ---------------------------------------------------------------------------------------------- a texture's frames

/// The published memory of a texture, mapped for reading alone.
struct Memory {
    base: *const u8,
    size: usize,
    texture: Texture,
    offset: usize,
    slot_size: usize,
}

// SAFETY: plain memory, read through atomics and copies; unmapped when dropped.
unsafe impl Send for Memory {}

impl Memory {
    fn map(texture: Texture, fd: &OwnedFd) -> Result<Self> {
        let size = texture.map_size as usize;
        if texture.map_size < HEADER as u64 || texture.map_size > 2 << 30 {
            return Err(platform("the engine's frames", "a size that is none")); // i18n: not shown (an error detail)
        }
        // SAFETY: a zeroed stat is valid, and the descriptor is this process's own.
        let held = unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            libc::fstat(fd.as_raw_fd(), &mut st) == 0 && st.st_size as u64 >= texture.map_size
        };
        if !held {
            return Err(platform("the engine's frames", "less memory than it said")); // i18n: not shown (an error detail)
        }
        // SAFETY: the descriptor's memory is at least that long; it is only read.
        let base = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if base == libc::MAP_FAILED {
            return Err(platform("the engine's frames", std::io::Error::last_os_error())); // i18n: not shown (an error detail)
        }
        let mut this = Self { base: base.cast(), size, texture, offset: 0, slot_size: 0 };
        // Copied out and checked once: these do not change, and the memory is another program's.
        let (offset, slot_size) = (this.wide(32), this.wide(40));
        let head = [this.word(0), this.word(4), this.word(8), this.word(12), this.word(16), this.word(20), this.word(24), this.word(28)];
        let told = [MAGIC, head[1], texture.token, texture.width, texture.height, texture.format, texture.stride, SLOTS];
        let picture = u64::from(texture.stride) * u64::from(texture.height);
        let end = slot_size.checked_mul(u64::from(SLOTS)).and_then(|n| n.checked_add(offset));
        if head != told || head[1] == 0 || offset < HEADER as u64 || slot_size < picture || end.is_none_or(|end| end > texture.map_size) {
            return Err(platform("the engine's frames", "a header that is not the texture's")); // i18n: not shown (an error detail)
        }
        (this.offset, this.slot_size) = (offset as usize, slot_size as usize);
        Ok(this)
    }

    fn word(&self, at: usize) -> u32 {
        // SAFETY: within the header, whose fields are naturally aligned, and which the mapping holds.
        unsafe { (*self.base.add(at).cast::<AtomicU32>()).load(Ordering::Acquire) }
    }

    fn wide(&self, at: usize) -> u64 {
        // SAFETY: as [`Memory::word`].
        unsafe { (*self.base.add(at).cast::<AtomicU64>()).load(Ordering::Acquire) }
    }

    /// The newest whole frame: its number and the slot it is in.
    fn latest(&self) -> Option<(u64, u32)> {
        let latest = self.wide(48);
        let (n, slot) = (latest >> 8, (latest & 0xff) as u32);
        (n != 0 && slot < SLOTS && self.holds(n, slot)).then_some((n, slot))
    }

    /// The texture is no more.
    fn closed(&self) -> bool {
        self.word(72) != 0
    }

    fn holds(&self, frame: u64, slot: u32) -> bool {
        self.wide(80 + slot as usize * 16) == frame
    }

    /// Copies `out.len()` bytes from `at` of a frame. `false` when the slot was written meanwhile: what was copied is not that frame's.
    fn copy(&self, frame: u64, slot: u32, at: usize, out: &mut [u8]) -> bool {
        debug_assert!(at + out.len() <= self.slot_size);
        if !self.holds(frame, slot) || at + out.len() > self.slot_size {
            return false;
        }
        // SAFETY: within the slot, which the mapping holds. Another program may write it meanwhile: the bytes are only ever used once its number says it did not.
        unsafe { std::ptr::copy_nonoverlapping(self.base.add(self.offset + slot as usize * self.slot_size + at), out.as_mut_ptr(), out.len()) };
        fence(Ordering::Acquire);
        self.holds(frame, slot)
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: mapped once, with that size.
        unsafe { libc::munmap(self.base.cast_mut().cast(), self.size) };
    }
}

struct Connection {
    link: Link,
    session: Session,
    info: SenderInfo,
    memory: Memory,
    bytes_per_pixel: usize,
    /// The last frame lent.
    last: u64,
}

/// Reads frames from one sender into CPU memory.
pub struct Receiver {
    conn: Option<Connection>,
    last_new: Option<Instant>,
    connected_at: Instant,
    interval: Duration,
    last_look: Option<Instant>,
    pixels: Vec<u8>,
    convert: Vec<u8>,
}

impl Receiver {
    pub fn new() -> Result<Self> {
        Ok(Self { conn: None, last_new: None, connected_at: Instant::now(), interval: Duration::from_millis(8), last_look: None, pixels: Vec::new(), convert: Vec::new() })
    }

    pub fn info(&self) -> Option<&SenderInfo> {
        self.conn.as_ref().map(|c| &c.info)
    }

    /// Leaves the sender: the engine copies no more pictures for it.
    pub fn disconnect(&mut self) {
        self.conn = None;
        self.last_new = None;
    }

    /// What tells a new picture from the last where a sender counts none: the engine counts every one, so nothing is compared here.
    pub fn set_probe(&mut self, _rects: Vec<[u32; 4]>) {}

    /// Opens a sender's frames.
    pub fn connect(&mut self, name: &str) -> Result<SenderInfo> {
        self.disconnect();
        let gone = || SpoutError::NotFound(name.to_string());
        let session = session().ok_or_else(gone)?;
        let workers = workers(&session);
        let pids: Vec<u32> = workers.iter().map(|w| w.0).collect();
        let spout = blocks(&session.guest_tmp);
        for (pid, path) in &workers {
            let Ok(mut link) = Link::open(*pid, path) else { continue };
            link.greet();
            let known: Vec<Texture> = link.textures.iter().map(|(t, _)| *t).collect();
            let mut names = named(spout.clone(), &pids, Some(&known));
            if names.is_empty() {
                names = by_order(known);
            }
            let Some(found) = names.into_iter().find(|s| s.name == name) else { continue };
            let Some((texture, fd)) = link.textures.iter().find(|(t, _)| t.token == found.token) else { continue };
            let bytes_per_pixel = match formats::eight_bit_order(texture.format) {
                Some(_) => 4,
                None if texture.format == formats::R16G16B16A16_FLOAT => 8,
                None => return Err(SpoutError::UnsupportedFormat(texture.format)),
            };
            let (w, h) = (texture.width, texture.height);
            if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
                return Err(SpoutError::Platform(tr!("the sender's picture is {width}×{height}: Orbiters reads pictures up to 8K (7680×4320)", width = w, height = h)));
            }
            if (texture.stride as usize) < w as usize * bytes_per_pixel {
                return Err(SpoutError::Platform(orbiters_i18n::tr("the mapped frame is smaller than the picture").into()));
            }
            let memory = Memory::map(*texture, fd)?;
            let info = SenderInfo { width: w, height: h, format: texture.format, share_handle: texture.token, ..found.info };
            self.conn = Some(Connection { link, session, info: info.clone(), memory, bytes_per_pixel, last: 0 });
            self.connected_at = Instant::now();
            return Ok(info);
        }
        Err(gone())
    }

    /// Makes sure the sender is there with the texture it had; opened again when it was made anew (another size).
    fn ensure_connected(&mut self) -> Result<()> {
        let Some(conn) = self.conn.as_mut() else { return Err(SpoutError::Platform(orbiters_i18n::tr("not connected").into())) };
        conn.link.hear();
        let there = !conn.link.over && !conn.memory.closed() && conn.link.textures.iter().any(|(t, _)| t.token == conn.memory.texture.token);
        if there && session().as_ref() == Some(&conn.session) {
            return Ok(());
        }
        let name = conn.info.name.clone();
        self.connect(&name).map(|_| ())
    }

    fn not_due(&self) -> Option<Duration> {
        let since = self.last_new?.elapsed();
        let quiet = self.interval / 2;
        (since < quiet).then(|| quiet - since)
    }

    /// Whether looking for a new frame is worth it now, as on Windows.
    pub fn due(&self) -> bool {
        self.not_due().is_none() && self.last_look.is_none_or(|t| t.elapsed() >= self.overdue_pause())
    }

    /// The pause between two looks once a frame is due: 1 ms, growing to 8 ms while none comes. (A look is one read of memory.)
    fn overdue_pause(&self) -> Duration {
        let since = self.last_new.unwrap_or(self.connected_at).elapsed().saturating_sub(self.interval / 2);
        (since / 8).clamp(Duration::from_millis(1), Duration::from_millis(8))
    }

    fn fresh(&self) -> Option<(u64, u32)> {
        self.conn.as_ref().and_then(|c| c.memory.latest().filter(|(n, _)| *n != c.last))
    }

    /// Waits up to `timeout` for a new frame and lends it to `f`, which takes the rows it needs ([`FrameReader::rows`]). `Ok(None)` when no new frame came.
    pub fn receive_with<R>(&mut self, timeout: Duration, f: impl FnOnce(&mut FrameReader<'_>) -> R) -> Result<Option<R>> {
        self.ensure_connected()?;
        let deadline = Instant::now() + timeout;
        if let Some(wait) = self.not_due() {
            std::thread::sleep(wait.min(deadline.saturating_duration_since(Instant::now())));
        }
        let (frame, slot) = loop {
            self.last_look = Some(Instant::now());
            if let Some(found) = self.fresh() {
                break found;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep(self.overdue_pause().min(deadline - now));
            // A sender that goes meanwhile: said at once, not at the next call.
            if self.conn.as_ref().is_some_and(|c| c.memory.closed()) {
                self.ensure_connected()?;
            }
        };
        let seen = Instant::now();
        if let Some(last) = self.last_new {
            let dt = seen.saturating_duration_since(last);
            if dt < Duration::from_millis(250) {
                self.interval = ((self.interval * 3 + dt) / 4).clamp(Duration::from_millis(4), Duration::from_millis(100));
            }
        }
        self.last_new = Some(seen);
        if let Some(conn) = self.conn.as_mut() {
            conn.last = frame;
        }
        let mut reader = FrameReader { receiver: self, seen, frame_count: Some(frame as i64), frame, slot, fresh: Vec::new(), converted: Vec::new() };
        Ok(Some(f(&mut reader)))
    }

    /// Waits up to `timeout` for a new frame and lends the whole of it to `f`. `Ok(None)` when no new frame came.
    pub fn receive<R>(&mut self, timeout: Duration, f: impl FnOnce(Frame<'_>) -> R) -> Result<Option<R>> {
        match self.receive_with(timeout, |r| r.full().map(f))? {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

/// A frame lent by [`Receiver::receive_with`]: its rows are copied as they are asked for, and every one of them is of one whole picture.
pub struct FrameReader<'r> {
    receiver: &'r mut Receiver,
    /// When the frame was seen.
    pub seen: Instant,
    /// The engine's count of the sender's pictures.
    pub frame_count: Option<i64>,
    frame: u64,
    slot: u32,
    /// Rows of [`Receiver::pixels`] that hold this frame, and of [`Receiver::convert`].
    fresh: Vec<(u32, u32)>,
    converted: Vec<(u32, u32)>,
}

impl FrameReader<'_> {
    pub fn width(&self) -> u32 {
        self.receiver.conn.as_ref().map_or(0, |c| c.info.width)
    }

    pub fn height(&self) -> u32 {
        self.receiver.conn.as_ref().map_or(0, |c| c.info.height)
    }

    /// The frame with at least these ranges of rows (top-down as stored, ends excluded). Only those rows are this frame's.
    pub fn rows(&mut self, ranges: &[(u32, u32)]) -> Result<Frame<'_>> {
        let rx = &mut *self.receiver;
        let Some(conn) = rx.conn.as_mut() else { return Err(SpoutError::Platform(orbiters_i18n::tr("not connected").into())) };
        let (w, h, format, bpp, stride) = (conn.info.width, conn.info.height, conn.info.format, conn.bytes_per_pixel, conn.memory.texture.stride as usize);
        let wanted: Vec<(u32, u32)> = ranges.iter().map(|&(a, b)| (a.min(h), b.min(h))).filter(|(a, b)| a < b).collect();
        rx.pixels.resize(stride * h as usize, 0);
        let mut tries = 0;
        loop {
            let missing: Vec<(u32, u32)> = wanted.iter().flat_map(|&r| missing_rows(r, &self.fresh)).collect();
            let copied = missing.iter().all(|&(a, b)| conn.memory.copy(self.frame, self.slot, a as usize * stride, &mut rx.pixels[a as usize * stride..b as usize * stride]));
            if copied {
                for r in missing {
                    add_rows(&mut self.fresh, r);
                }
                break;
            }
            // Written meanwhile. With no row of it lent yet, the newest frame is as good; else this one is over: rows of two pictures are never one.
            tries += 1;
            let next = if self.fresh.is_empty() && tries < TRIES { conn.memory.latest() } else { None };
            let Some((frame, slot)) = next else {
                return Err(SpoutError::Platform(orbiters_i18n::tr("the frame was gone before it was read").into()));
            };
            (self.frame, self.slot, self.frame_count, conn.last) = (frame, slot, Some(frame as i64), frame);
        }
        // `height` rows `stride` apart, the last one only as long as the picture is wide.
        let len = stride * (h as usize - 1) + w as usize * bpp;
        let raw = &rx.pixels[..len];
        if let Some(order) = formats::eight_bit_order(format) {
            return Ok(Frame { width: w, height: h, stride, data: raw, order, frame_count: self.frame_count, seen: self.seen });
        }
        // 16-bit float (linear): the rows asked for become 8-bit sRGB codes, the form every decoder expects.
        let table = half_to_srgb8_table();
        let row_out = w as usize * 4;
        rx.convert.resize(row_out * h as usize, 0);
        let todo: Vec<(u32, u32)> = wanted.iter().flat_map(|&r| missing_rows(r, &self.converted)).collect();
        for (a, b) in todo {
            for y in a as usize..b as usize {
                let row = &raw[y * stride..y * stride + w as usize * 8];
                let out = &mut rx.convert[y * row_out..(y + 1) * row_out];
                for (px, o) in row.as_chunks::<8>().0.iter().zip(out.as_chunks_mut::<4>().0) {
                    o[0] = table[u16::from_le_bytes([px[0], px[1]]) as usize];
                    o[1] = table[u16::from_le_bytes([px[2], px[3]]) as usize];
                    o[2] = table[u16::from_le_bytes([px[4], px[5]]) as usize];
                    o[3] = 255;
                }
            }
            add_rows(&mut self.converted, (a, b));
        }
        Ok(Frame { width: w, height: h, stride: row_out, data: &rx.convert, order: ChannelOrder::Rgba, frame_count: self.frame_count, seen: self.seen })
    }

    /// The whole frame.
    pub fn full(&mut self) -> Result<Frame<'_>> {
        let h = self.height();
        self.rows(&[(0, h)])
    }
}

#[cfg(test)]
#[path = "mac_tests.rs"]
mod tests;

