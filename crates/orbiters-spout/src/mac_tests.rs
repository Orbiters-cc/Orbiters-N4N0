//! The receiver against a render server of the tests' own, which speaks as Neptune's does (`npt_publish_wire.h`): socket, descriptors and memory are real, the pictures are
//! made here. (The engine's own render server with an owned Spout sender: `tools/mac-spout/check.py`.)

use super::*;
use std::io::Write;
use std::os::fd::IntoRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixListener;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, MutexGuard};

/// The session is one for the process: one test at a time.
static ALONE: Mutex<()> = Mutex::new(());

struct Texture2 {
    told: Texture,
    map: *mut u8,
    path: PathBuf,
    frame: u64,
}

// SAFETY: plain memory of the tests'.
unsafe impl Send for Texture2 {}

impl Texture2 {
    fn word(&self, at: usize) -> &AtomicU32 {
        // SAFETY: within the mapping's header.
        unsafe { &*self.map.add(at).cast::<AtomicU32>() }
    }

    fn wide(&self, at: usize) -> &AtomicU64 {
        // SAFETY: within the mapping's header.
        unsafe { &*self.map.add(at).cast::<AtomicU64>() }
    }

    fn slot(&self, i: u64) -> &mut [u8] {
        let size = self.wide(40).load(Ordering::Relaxed) as usize;
        // SAFETY: within the mapping; one writer.
        unsafe { std::slice::from_raw_parts_mut(self.map.add(HEADER + i as usize * size), size) }
    }

    /// The next picture, filled by `fill`, as the render server publishes one.
    fn publish(&mut self, fill: impl Fn(usize) -> u8) {
        self.frame += 1;
        let slot = self.frame % 4;
        self.wide(80 + slot as usize * 16).store(0, Ordering::Release);
        for (i, b) in self.slot(slot).iter_mut().enumerate() {
            *b = fill(i);
        }
        self.wide(80 + slot as usize * 16).store(self.frame, Ordering::Release);
        self.wide(48).store(self.frame << 8 | slot, Ordering::Release);
    }
}

struct Server {
    dir: PathBuf,
    session: Session,
    textures: Arc<Mutex<Vec<Texture2>>>,
    peers: Arc<Mutex<Vec<UnixStream>>>,
    accepted: Arc<AtomicUsize>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    _alone: MutexGuard<'static, ()>,
}

fn pid() -> u32 {
    std::process::id()
}

fn token(n: u32) -> u32 {
    0x4000_0000 | (pid() & 0x1_ffff) << 13 | n << 2 | 2
}

fn send(peer: &UnixStream, kind: u32, body: &[u8], fd: Option<i32>) {
    let message = [&kind.to_le_bytes()[..], &(body.len() as u32).to_le_bytes(), body].concat();
    // SAFETY: buffers of this call's own; the control message is laid by libc's own macros.
    unsafe {
        let mut part = libc::iovec { iov_base: message.as_ptr() as *mut _, iov_len: message.len() };
        let mut control = [0u64; 8];
        let mut head: libc::msghdr = std::mem::zeroed();
        head.msg_iov = &mut part;
        head.msg_iovlen = 1;
        if let Some(fd) = fd {
            head.msg_control = control.as_mut_ptr().cast();
            head.msg_controllen = libc::CMSG_SPACE(4) as _;
            let c = libc::CMSG_FIRSTHDR(&head);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(4) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast(), fd);
        }
        // (a reader that left meanwhile hears nothing more)
        let _ = libc::sendmsg(peer.as_raw_fd(), &head, 0);
    }
}

fn told(t: &Texture) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [t.token, t.order, t.width, t.height, t.format, t.stride, SLOTS, 0] {
        b.extend(v.to_le_bytes());
    }
    b.extend(t.map_size.to_le_bytes());
    b.extend(7u64.to_le_bytes());
    b
}

fn tell(peer: &UnixStream, t: &Texture2) {
    let fd = std::fs::File::open(&t.path).unwrap().into_raw_fd();
    send(peer, 2, &told(&t.told), Some(fd));
    // SAFETY: sent; this end's is this test's own.
    unsafe { libc::close(fd) };
}

impl Server {
    fn new(name: &str) -> Self {
        let alone = ALONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("orbs{}{name}", pid()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = Session { shared: dir.join("s"), guest_tmp: dir.join("t") };
        std::fs::create_dir_all(&session.shared).unwrap();
        std::fs::set_permissions(&session.shared, std::fs::Permissions::from_mode(0o700)).unwrap();
        let this = Self { dir, session: session.clone(), textures: Arc::default(), peers: Arc::default(), accepted: Arc::default(), stop: Arc::default(), _alone: alone };
        let listener = UnixListener::bind(session.shared.join(format!("{}.sock", pid()))).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (textures, peers, accepted, stop) = (this.textures.clone(), this.peers.clone(), this.accepted.clone(), this.stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Ok((peer, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                accepted.fetch_add(1, Ordering::Relaxed);
                send(&peer, 1, &[1u32.to_le_bytes(), pid().to_le_bytes()].concat(), None);
                let textures = textures.lock().unwrap();
                for t in textures.iter() {
                    tell(&peer, t);
                }
                peers.lock().unwrap().push(peer);
            }
        });
        *ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        set_session(Some(session));
        this
    }

    fn add(&self, n: u32, width: u32, height: u32, format: u32, bpp: u32) -> u32 {
        // (rows padded, as Metal's are)
        let stride = (width * bpp).next_multiple_of(256);
        let slot = (stride * height).next_multiple_of(16384) as usize;
        let size = HEADER + 4 * slot;
        let path = self.dir.join(format!("memory{n}"));
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).mode(0o600).open(&path).unwrap();
        file.set_len(size as u64).unwrap();
        // SAFETY: this test's own file, of that size.
        let map: *mut u8 = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0) }.cast();
        let texture = Texture2 { told: Texture { token: token(n), order: n, width, height, format, stride, map_size: size as u64 }, map, path, frame: 0 };
        for (i, v) in [MAGIC, 1, token(n), width, height, format, stride, SLOTS].into_iter().enumerate() {
            texture.word(i * 4).store(v, Ordering::Relaxed);
        }
        texture.wide(32).store(HEADER as u64, Ordering::Relaxed);
        texture.wide(40).store(slot as u64, Ordering::Relaxed);
        for peer in self.peers.lock().unwrap().iter() {
            tell(peer, &texture);
        }
        self.textures.lock().unwrap().push(texture);
        token(n)
    }

    /// Spout's own memory, as Wine's server keeps it on the host.
    fn spout(&self, names: &[&str], infos: &[(u32, u32, u32, u32)]) {
        let dir = self.session.guest_tmp.join(".wine-1000/server-1-2a");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |n: usize, size: usize, bytes: &[u8]| {
            let mut f = std::fs::File::create(dir.join(format!("tmpmap-{n:08x}"))).unwrap();
            f.write_all(bytes).unwrap();
            f.set_len(size as u64).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        };
        let list: Vec<u8> = names.iter().flat_map(|n| [n.as_bytes(), &vec![0; 256 - n.len()]].concat()).collect();
        // Other programs' mappings, and what is none of Spout's.
        write(1, 4096, &[0xff; 900]);
        write(2, 65536, b"VRCSender9");
        if let Some(first) = names.first() {
            write(3, 4096, first.as_bytes());
        }
        for (i, (token, w, h, format)) in infos.iter().enumerate() {
            let mut b: Vec<u8> = [*token, *w, *h, *format, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
            b.extend(b"C:\\VRChat\\VRChat.exe");
            write(10 + i, 4096, &b);
        }
        write(4, 16384, &list);
    }

    fn with<R>(&self, n: u32, f: impl FnOnce(&mut Texture2) -> R) -> R {
        f(self.textures.lock().unwrap().iter_mut().find(|t| t.told.token == token(n)).unwrap())
    }

    fn gone(&self, n: u32) {
        self.with(n, |t| t.word(72).store(1, Ordering::Release));
        self.textures.lock().unwrap().retain(|t| t.told.token != token(n));
        for peer in self.peers.lock().unwrap().iter() {
            send(peer, 3, &[token(n).to_le_bytes(), [0; 4]].concat(), None);
        }
    }

    fn connections(&self) -> usize {
        self.accepted.load(Ordering::Relaxed)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        set_session(None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const BGRA: u32 = formats::B8G8R8A8_UNORM;
const WAIT: Duration = Duration::from_millis(500);

/// What is there is known from Spout's own memory, with no connection (which would have the engine copy every picture), by its own name.
#[test]
fn senders_are_listed_by_their_names_without_connecting() {
    let server = Server::new("a");
    for _ in 0..5 {
        assert!(list_senders().is_empty());
    }
    assert_eq!(server.connections(), 1, "a worker with no block of Spout's is asked once");
    let t = server.add(1, 1280, 720, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 1280, 720, BGRA)]);
    assert_eq!(list_senders(), ["VRCSender1"]);
    assert_eq!(active_sender().as_deref(), Some("VRCSender1"));
    let info = sender_info("VRCSender1").unwrap();
    assert_eq!((info.width, info.height, info.format, info.share_handle, info.description.as_str()), (1280, 720, BGRA, t, "C:\\VRChat\\VRChat.exe"));
    assert!(sender_info("VRCSender2").is_none());
    assert_eq!(server.connections(), 1, "nor once there is one");
    // A block left by a session that ended is of no worker of this one.
    server.spout(&["VRCSender1"], &[(t ^ 1 << 14, 1280, 720, BGRA)]);
    assert!(list_senders().iter().all(|n| n == "VRCSender1"));
    set_session(None);
    assert!(list_senders().is_empty(), "no game of Home's runs");
}

/// Several at once (the camera dolly's): each block with its own name, in the order the senders were made, also once the first one's texture changed.
#[test]
fn several_senders_keep_their_own_names() {
    let server = Server::new("b");
    let (a, b, c) = (server.add(5, 1920, 1080, BGRA, 4), server.add(2, 1280, 720, BGRA, 4), server.add(3, 640, 360, BGRA, 4));
    server.spout(&["VRCSender1", "VRCSender2", "VRCSender3"], &[(a, 1920, 1080, BGRA), (b, 1280, 720, BGRA), (c, 640, 360, BGRA)]);
    assert_eq!(list_senders(), ["VRCSender1", "VRCSender2", "VRCSender3"]);
    for (name, size) in [("VRCSender1", (1920, 1080)), ("VRCSender2", (1280, 720)), ("VRCSender3", (640, 360))] {
        let mut receiver = Receiver::new().unwrap();
        let info = receiver.connect(name).unwrap();
        assert_eq!((info.width, info.height), size, "{name}");
    }
}

/// With Spout's memory unreadable (another Wine's), the textures published are named as VRChat names its streams; asked of the engine seldom.
#[test]
fn without_spout_s_memory_the_textures_are_taken_in_their_order() {
    let server = Server::new("c");
    server.add(4, 640, 360, BGRA, 4);
    server.add(2, 1280, 720, BGRA, 4);
    assert_eq!(list_senders(), ["VRCSender1", "VRCSender2"]);
    assert_eq!(sender_info("VRCSender1").unwrap().width, 1280, "the first made");
    let asked = server.connections();
    for _ in 0..20 {
        list_senders();
    }
    assert_eq!(server.connections(), asked, "not at each look");
    assert_eq!(Receiver::new().unwrap().connect("VRCSender2").unwrap().width, 640);
}

/// Every picture whole, its number growing, each one once, padded rows and all; only the rows asked for are read.
#[test]
fn every_frame_is_whole_new_and_numbered() {
    let server = Server::new("d");
    let t = server.add(1, 300, 200, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 300, 200, BGRA)]);
    let mut receiver = Receiver::new().unwrap();
    assert!(matches!(receiver.connect("VRCSender2"), Err(SpoutError::NotFound(_))));
    receiver.connect("VRCSender1").unwrap();
    assert_eq!(receiver.receive_with(Duration::from_millis(20), |_| ()).unwrap(), None, "no picture yet");
    for n in 1..=20u64 {
        server.with(1, |t| t.publish(|_| n as u8 * 7));
        let got = receiver
            .receive_with(WAIT, |r| {
                assert_eq!((r.width(), r.height(), r.frame_count), (300, 200, Some(n as i64)));
                let some = r.rows(&[(10, 20), (150, 400)]).unwrap();
                assert_eq!((some.stride, some.order), (1280, ChannelOrder::Bgra));
                assert!(some.data[10 * 1280..20 * 1280].iter().all(|b| *b == n as u8 * 7));
                let full = r.full().unwrap();
                assert_eq!(full.data.len(), 1280 * 199 + 1200);
                full.data.iter().all(|b| *b == n as u8 * 7)
            })
            .unwrap();
        assert_eq!(got, Some(true), "{n}");
        assert_eq!(receiver.receive_with(Duration::ZERO, |_| ()).unwrap(), None, "once");
    }
    // Slower than the sender: the newest.
    for n in 21..=23u8 {
        server.with(1, |t| t.publish(|_| n));
    }
    assert_eq!(receiver.receive(WAIT, |f| (f.data[0], f.frame_count)).unwrap(), Some((23, Some(23))));
}

/// A slot written while it is read: with nothing lent yet the newest picture is, and once rows were, the frame is over rather than made of two pictures.
#[test]
fn a_picture_written_meanwhile_is_never_lent() {
    let server = Server::new("e");
    let t = server.add(1, 256, 64, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 256, 64, BGRA)]);
    let mut receiver = Receiver::new().unwrap();
    receiver.connect("VRCSender1").unwrap();
    server.with(1, |t| t.publish(|_| 1));
    let seen = receiver
        .receive_with(WAIT, |r| {
            for n in 2..=6u8 {
                server.with(1, |t| t.publish(|_| n));
            }
            r.full().map(|f| (f.data.iter().all(|b| *b == 6), f.frame_count)).unwrap()
        })
        .unwrap();
    assert_eq!(seen, Some((true, Some(6))), "nothing was lent of the first");
    server.with(1, |t| t.publish(|_| 7));
    let over = receiver
        .receive_with(WAIT, |r| {
            assert!(r.rows(&[(0, 8)]).unwrap().data[..8 * 1024].iter().all(|b| *b == 7));
            for n in 8..=12u8 {
                server.with(1, |t| t.publish(|_| n));
            }
            r.full().is_err()
        })
        .unwrap();
    assert_eq!(over, Some(true), "its other rows are another picture's");
    assert_eq!(receiver.receive(WAIT, |f| f.data[0]).unwrap(), Some(12), "and the next is read as ever");
}

/// The writer as fast as it can beside a reader: not one torn picture.
#[test]
fn no_picture_is_torn_however_fast_the_sender() {
    let server = Server::new("f");
    let t = server.add(1, 640, 360, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 640, 360, BGRA)]);
    let mut receiver = Receiver::new().unwrap();
    receiver.connect("VRCSender1").unwrap();
    let mut texture = server.textures.lock().unwrap().pop().unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    let writer = std::thread::spawn(move || {
        let mut n = 0u8;
        while !flag.load(Ordering::Relaxed) {
            n = n.wrapping_add(1);
            texture.publish(|_| n);
        }
        texture
    });
    let (mut whole, mut gone, mut last) = (0, 0, 0);
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        match receiver.receive(Duration::from_millis(50), |f| (f.data.iter().all(|b| *b == f.data[0]), f.frame_count.unwrap())) {
            Ok(Some((one, n))) => {
                assert!(one, "torn");
                assert!(n > last, "{n} after {last}");
                (whole, last) = (whole + 1, n);
            }
            Ok(None) => {}
            Err(SpoutError::Platform(_)) => gone += 1,
            Err(e) => panic!("{e}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    server.textures.lock().unwrap().push(writer.join().unwrap());
    assert!(whole > 20, "{whole} whole, {gone} gone before they were read");
}

/// The camera closed and opened again, at another size: read on, at its new size; gone for good, said so; and the game's end.
#[test]
fn a_sender_that_goes_or_comes_back_is_followed() {
    let server = Server::new("g");
    let t = server.add(1, 320, 200, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 320, 200, BGRA)]);
    let mut receiver = Receiver::new().unwrap();
    receiver.connect("VRCSender1").unwrap();
    server.with(1, |t| t.publish(|_| 9));
    assert_eq!(receiver.receive(WAIT, |f| (f.width, f.height, f.data[0])).unwrap(), Some((320, 200, 9)));
    server.gone(1);
    let t = server.add(2, 640, 360, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 640, 360, BGRA)]);
    server.with(2, |t| t.publish(|_| 11));
    assert_eq!(receiver.receive(WAIT, |f| (f.width, f.height, f.data[0])).unwrap(), Some((640, 360, 11)), "made anew");
    assert_eq!(receiver.info().unwrap().share_handle, t);
    server.gone(2);
    server.spout(&[], &[]);
    assert!(matches!(receiver.receive(WAIT, |_| ()), Err(SpoutError::NotFound(name)) if name == "VRCSender1"));
    assert!(matches!(receiver.receive(WAIT, |_| ()), Err(SpoutError::Platform(_))), "not connected");
    // The worker's end (the game's): the same.
    let t = server.add(3, 320, 200, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 320, 200, BGRA)]);
    receiver.connect("VRCSender1").unwrap();
    server.peers.lock().unwrap().clear();
    server.textures.lock().unwrap().clear();
    server.spout(&[], &[]);
    assert!(matches!(receiver.receive(WAIT, |_| ()), Err(SpoutError::NotFound(_))));
    // Another session's: never read on into it by the old one's connection.
    let t = server.add(4, 320, 200, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 320, 200, BGRA)]);
    receiver.connect("VRCSender1").unwrap();
    set_session(None);
    assert!(matches!(receiver.receive(WAIT, |_| ()), Err(SpoutError::NotFound(_))), "the game ended");
}

/// 16-bit frames become the 8-bit codes every decoder expects, as on Windows; what cannot be read is said, not guessed.
#[test]
fn formats_are_read_as_on_windows() {
    let server = Server::new("h");
    let t = server.add(1, 64, 64, formats::R16G16B16A16_FLOAT, 8);
    server.spout(&["VRCSender1"], &[(t, 64, 64, formats::R16G16B16A16_FLOAT)]);
    let mut receiver = Receiver::new().unwrap();
    receiver.connect("VRCSender1").unwrap();
    // 1.0, 0.5, 0.0, 1.0 in half floats.
    server.with(1, |t| t.publish(|i| [0x00, 0x3c, 0x00, 0x38, 0x00, 0x00, 0x00, 0x3c][i % 8]));
    let got = receiver.receive(WAIT, |f| (f.order, f.stride, f.data[..4].to_vec(), f.data[63 * 256 + 4..63 * 256 + 8].to_vec())).unwrap().unwrap();
    assert_eq!(got, (ChannelOrder::Rgba, 256, vec![255, 188, 0, 255], vec![255, 188, 0, 255]));
    let t = server.add(2, 64, 64, formats::R8G8B8A8_UNORM_SRGB, 4);
    server.spout(&["VRCSender1"], &[(t, 64, 64, formats::R8G8B8A8_UNORM_SRGB)]);
    receiver.connect("VRCSender1").unwrap();
    server.with(2, |t| t.publish(|_| 3));
    assert_eq!(receiver.receive(WAIT, |f| f.order).unwrap(), Some(ChannelOrder::Rgba));
    let t = server.add(3, 64, 64, 24, 4);
    server.spout(&["VRCSender1"], &[(t, 64, 64, 24)]);
    assert!(matches!(receiver.connect("VRCSender1"), Err(SpoutError::UnsupportedFormat(24))));
}

/// Memory that is not what was told, a folder others may enter, a protocol that is another: nothing is read of them.
#[test]
fn nothing_is_read_that_is_not_as_it_was_told() {
    let server = Server::new("i");
    let t = server.add(1, 64, 64, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 64, 64, BGRA)]);
    let mut receiver = Receiver::new().unwrap();
    for (at, wrong) in [(0, 1), (8, 5), (12, 65), (16, 63), (24, 64), (28, 3), (32, 100), (36, 1), (40, 64), (44, 7)] {
        let was = server.with(1, |t| t.word(at).swap(wrong, Ordering::Relaxed));
        assert!(matches!(receiver.connect("VRCSender1"), Err(SpoutError::Platform(_))), "at {at}");
        server.with(1, |t| t.word(at).store(was, Ordering::Relaxed));
    }
    receiver.connect("VRCSender1").unwrap();
    // A slot that is none, a number that is not the slot's: no frame.
    for latest in [1 << 8 | 4, 1 << 8 | 0xff, 5 << 8 | 1] {
        server.with(1, |t| t.wide(48).store(latest, Ordering::Relaxed));
        assert_eq!(receiver.receive_with(Duration::ZERO, |_| ()).unwrap(), None);
    }
    // Memory shorter than told.
    server.with(1, |t| std::fs::OpenOptions::new().write(true).open(&t.path).unwrap().set_len(HEADER as u64).unwrap());
    assert!(receiver.connect("VRCSender1").is_err());
    std::fs::set_permissions(&server.session.shared, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(list_senders().is_empty(), "a folder others may enter");
    assert!(matches!(receiver.connect("VRCSender1"), Err(SpoutError::NotFound(_))));
    std::fs::set_permissions(&server.session.shared, std::fs::Permissions::from_mode(0o700)).unwrap();
    // A worker that was killed leaves its file behind.
    std::fs::rename(server.session.shared.join(format!("{}.sock", pid())), server.session.shared.join("99999999.sock")).unwrap();
    assert!(list_senders().is_empty());
}

/// Waiting for a frame that does not come costs next to nothing, and not being connected costs nothing at all.
#[test]
fn waiting_costs_next_to_nothing() {
    let server = Server::new("j");
    let t = server.add(1, 1920, 1080, BGRA, 4);
    server.spout(&["VRCSender1"], &[(t, 1920, 1080, BGRA)]);
    let mut receiver = Receiver::new().unwrap();
    receiver.connect("VRCSender1").unwrap();
    let cpu = || {
        // SAFETY: a plain query into this call's own structure.
        unsafe {
            let mut t: libc::timespec = std::mem::zeroed();
            libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t);
            t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
        }
    };
    let began = cpu();
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        assert_eq!(receiver.receive_with(Duration::from_millis(100), |_| ()).unwrap(), None);
    }
    let used = cpu() - began;
    assert!(used < 0.05, "{used} s of a second");
    receiver.disconnect();
    assert!(receiver.info().is_none());
}
