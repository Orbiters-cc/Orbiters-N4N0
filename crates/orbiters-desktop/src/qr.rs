//! N4N0's QR code scanner on this computer (`orbiters_n4n0::Host::qr_scan`), also the one beside VRChat's camera in VR: a
//! thread of its own, below normal priority, reads VRChat's stream camera through Spout (`VRCSender1`, or Settings' Spout
//! sender) about three times a second and reads the codes in it with [`orbiters_qr`] (the whole picture one look, a closer
//! one where a code was glimpsed or the middle the next, the middle the third: styled codes too, in negative, coloured,
//! with a logo). A code seen but not read is said ([`QrSeen::Glimpse`]): cut by the picture's edge, or not clear yet.
//! N4N0 decides what opens. What it saw is noted in `vr-test.log` every ten seconds while it looks.

pub use orbiters_qr::{grey, Pixels, View, MOST_WIDTH};
use orbiters_n4n0::{QrEvents, QrGlimpse, QrScan, QrSeen, QrSide};
use orbiters_i18n::tr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The time between two looks at the camera.
pub const EVERY: Duration = Duration::from_millis(330);
/// VRChat's camera's Spout sender.
#[cfg(any(windows, target_os = "macos"))]
const VRCHAT_SENDER: &str = "VRCSender1";
/// No frame for this long (from the start, or since the last): the camera is said to send none.
#[cfg(any(windows, target_os = "macos"))]
const NO_PICTURE_AFTER: Duration = Duration::from_millis(2500);
/// ...and only after as many looks without one as fit in that time. The scanner runs below normal priority, so a busy
/// computer (VRChat itself, or the whole test suite) can leave it without a turn for seconds: the time alone then said the
/// camera sent nothing when the scanner had not looked.
#[cfg(any(windows, target_os = "macos"))]
const NO_PICTURE_LOOKS: u32 = (NO_PICTURE_AFTER.as_millis() / EVERY.as_millis()) as u32;
/// How often what the scanner saw is noted.
#[cfg(any(windows, target_os = "macos"))]
const NOTE_EVERY: Duration = Duration::from_secs(10);

/// The texts of the QR codes in a grey picture (every way [`orbiters_qr::read`] has).
pub fn decode(width: usize, height: usize, grey: &[u8]) -> Vec<String> {
    orbiters_qr::read(width, height, grey, false).codes
}

/// What is said of a code glimpsed but not read: it must be seen in two of the last three looks at the whole picture
/// (one look's glimpse may be a passing likeness), and is unsaid once two looks in a row saw none.
#[derive(Default)]
pub struct Glimpses {
    recent: [Option<QrGlimpse>; 3],
    said: Option<QrGlimpse>,
}

impl Glimpses {
    /// A look at the whole picture saw `seen`: what is to be said now, when it changed.
    pub fn look(&mut self, seen: Option<orbiters_qr::Glimpse>) -> Option<Option<QrGlimpse>> {
        let seen = seen.map(|g| match g {
            orbiters_qr::Glimpse::Cut(side) => QrGlimpse::Cut(side.map(|s| match s {
                orbiters_qr::Side::Left => QrSide::Left,
                orbiters_qr::Side::Right => QrSide::Right,
                orbiters_qr::Side::Top => QrSide::Top,
                orbiters_qr::Side::Bottom => QrSide::Bottom,
                orbiters_qr::Side::Around => QrSide::Around,
            })),
            orbiters_qr::Glimpse::Unclear => QrGlimpse::Unclear,
        });
        self.recent.rotate_right(1);
        self.recent[0] = seen;
        let cuts = self.recent.iter().filter(|r| matches!(r, Some(QrGlimpse::Cut(_)))).count();
        // The side said stays while a recent look still saw it there (a code at a corner is not said to swap sides); else the
        // latest look's.
        let cut = || self.said.filter(|s| matches!(s, QrGlimpse::Cut(_)) && self.recent.contains(&Some(*s))).or_else(|| self.recent.iter().flatten().find(|r| matches!(r, QrGlimpse::Cut(_))).copied());
        let now = if self.recent[0].is_none() && self.recent[1].is_none() {
            None
        } else if cuts >= 2 {
            cut()
        } else if self.recent.iter().filter(|r| r.is_some()).count() >= 2 {
            // Cut once, unclear once: seen, whichever it is.
            if cuts > 0 && matches!(self.said, Some(QrGlimpse::Cut(_))) { cut() } else { Some(QrGlimpse::Unclear) }
        } else {
            self.said
        };
        (now != self.said).then(|| {
            self.said = now;
            now
        })
    }

    /// A code was read: nothing glimpsed is said any more.
    pub fn read(&mut self) -> Option<Option<QrGlimpse>> {
        self.recent = [None; 3];
        self.said.take().map(|_| None)
    }
}

/// One decoding of a picture any world can show, its panic contained: `None` for that picture (the scan goes on with the
/// next), noted in the log the first time and every hundredth after (`panics` counts them).
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn contained<T>(panics: &mut u32, decode: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(decode)) {
        Ok(v) => Some(v),
        Err(why) => {
            *panics += 1;
            if *panics % 100 == 1 {
                let why = why.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| why.downcast_ref::<String>().cloned()).unwrap_or_default();
                crate::overlay::vr_log::line(&format!("qr scan: a picture could not be decoded (the decoder panicked: {why}); skipped, {} so far", *panics)); // i18n: not shown (a log's line)
            }
            None
        }
    }
}

/// The scan's handle: dropped, its thread stops at its next look.
struct Scan(Arc<AtomicBool>);

impl QrScan for Scan {}

impl Drop for Scan {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Starts reading `wanted` (Settings' Spout sender; empty: VRChat's) for QR codes, telling `events` what it sees.
pub fn start(wanted: String, events: QrEvents) -> Result<Box<dyn QrScan>, String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("qr-scanner".into())
            .spawn(move || {
                lower_priority();
                // Ended otherwise than by its stop (a panic past the decoder's guard): said, never a scan shown running that
                // reads nothing.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&wanted, &events, &flag))).is_err() && !flag.load(Ordering::Relaxed) {
                    events(QrSeen::Failed(tr("The QR scanner stopped after an error: start it again.").into()));
                }
            })
            .map_err(|e| tr!("The scanner could not start: {error}", error = e))?;
        Ok(Box::new(Scan(stop)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (wanted, events);
        Err(tr("Reading VRChat's camera needs Spout, on Windows").into())
    }
}

/// Below normal priority: VRChat's frames come first. (A Mac's scheduler is left as it is.)
#[cfg(target_os = "macos")]
fn lower_priority() {}

/// Below normal priority: VRChat's frames come first.
#[cfg(windows)]
fn lower_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    // SAFETY: the pseudo-handle of the calling thread.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

/// The sender to read: `wanted` when it sends, else VRChat's camera's.
#[cfg(any(windows, target_os = "macos"))]
fn sender(wanted: &str) -> Option<String> {
    let senders = orbiters_spout::list_senders();
    [wanted, VRCHAT_SENDER].into_iter().filter(|n| !n.is_empty()).find(|n| senders.iter().any(|s| s == n)).map(str::to_string)
}

/// The scanner's thread, until `stop`.
#[cfg(any(windows, target_os = "macos"))]
fn run(wanted: &str, events: &QrEvents, stop: &AtomicBool) {
    use std::time::Instant;
    let mut receiver = match orbiters_spout::Receiver::new() {
        Ok(r) => r,
        Err(e) => {
            events(QrSeen::Failed(tr!("VRChat's camera cannot be read here: {error}", error = e)));
            return;
        }
    };
    let started = Instant::now();
    let mut connected = false;
    let mut last_frame: Option<Instant> = None;
    // What was last said of the camera (nothing yet).
    let mut said: Option<bool> = None;
    let mut looks = orbiters_qr::Looks::default();
    let mut glimpses = Glimpses::default();
    // Looks since the last frame (or the start) that found none.
    let mut looks_without = 0u32;
    let mut note = Note::default();
    let mut panics = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let begun = Instant::now();
        if !connected {
            if let Some(name) = sender(wanted) {
                match receiver.connect(&name) {
                    Ok(info) => {
                        connected = true;
                        // A few one-pixel rows tell a new frame from the last (VRChat's sender counts none).
                        let (w, h) = (info.width, info.height);
                        receiver.set_probe((1..=4).map(|k| h * k / 5).filter(|&y| y < h).map(|y| [0, y, w, y + 1]).collect());
                        crate::overlay::vr_log::line(&format!("qr scan: reading \"{name}\", {w} x {h}")); // i18n: not shown (a log's line)
                    }
                    Err(e) => log_warn!("qr: \"{name}\" could not be opened: {e}", name = name, e = e),
                }
            }
        }
        let mut found = Vec::new();
        let mut framed = false;
        if connected {
            let read = receiver.receive(Duration::from_millis(250), |f| {
                let pixels = Pixels {
                    width: f.width as usize,
                    height: f.height as usize,
                    stride: f.stride,
                    data: f.data,
                    bgra: f.order == orbiters_spout::ChannelOrder::Bgra,
                };
                (looks.copy(&pixels), (pixels.width, pixels.height))
            });
            match read {
                Ok(Some((copy, size))) => {
                    last_frame = Some(Instant::now());
                    framed = true;
                    let at = Instant::now();
                    // The picture is whatever the world in front of the camera shows (SWEEP-14): a decoder that panics on it
                    // costs that frame, never the scan.
                    let Some(reading) = contained(&mut panics, || {
                        #[cfg(test)]
                        tests::maybe_panic(wanted);
                        looks.read(&copy)
                    }) else {
                        looks = orbiters_qr::Looks::default();
                        std::thread::sleep(EVERY.saturating_sub(begun.elapsed()));
                        continue;
                    };
                    note.looked(copy.look, size, at.elapsed(), &reading);
                    found = reading.codes;
                    let change = if !found.is_empty() {
                        glimpses.read()
                    } else if copy.look == orbiters_qr::Look::Whole {
                        glimpses.look(reading.glimpse)
                    } else {
                        None
                    };
                    if let Some(glimpse) = change {
                        events(QrSeen::Glimpse(glimpse));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    if !matches!(e, orbiters_spout::SpoutError::NotFound(_)) {
                        log_warn!("qr: reading VRChat's camera failed: {e}", e = e);
                    }
                    receiver.disconnect();
                    connected = false;
                }
            }
        }
        looks_without = if framed { 0 } else { looks_without.saturating_add(1) };
        let frames = last_frame.unwrap_or(started).elapsed() < NO_PICTURE_AFTER && last_frame.is_some();
        // Frames come, or none came for a while (from the start too) over enough looks.
        let camera = if frames {
            Some(true)
        } else {
            (last_frame.unwrap_or(started).elapsed() >= NO_PICTURE_AFTER && looks_without >= NO_PICTURE_LOOKS).then_some(false)
        };
        if let Some(camera) = camera.filter(|c| said != Some(*c)) {
            said = Some(camera);
            events(QrSeen::Camera(camera));
        }
        for text in found {
            events(QrSeen::Code(text));
        }
        note.maybe(connected);
        std::thread::sleep(EVERY.saturating_sub(begun.elapsed()));
    }
}

/// What the scanner saw over [`NOTE_EVERY`], for `vr-test.log`: Enzo's session must say what the reader was given and
/// what it made of it (his two codes of 2026-11-14 left no trace).
#[cfg(any(windows, target_os = "macos"))]
#[derive(Default)]
struct Note {
    since: Option<std::time::Instant>,
    looks: [u32; 3],
    size: (usize, usize),
    took: Duration,
    slowest: Duration,
    read: u32,
    cut: u32,
    unclear: u32,
}

#[cfg(any(windows, target_os = "macos"))]
impl Note {
    fn looked(&mut self, look: orbiters_qr::Look, size: (usize, usize), took: Duration, reading: &orbiters_qr::Reading) {
        let k = match look {
            orbiters_qr::Look::Whole => 0,
            orbiters_qr::Look::Closer(_) => 1,
            orbiters_qr::Look::Middle => 2,
        };
        self.looks[k] += 1;
        self.size = size;
        self.took += took;
        self.slowest = self.slowest.max(took);
        self.read += !reading.codes.is_empty() as u32;
        match reading.glimpse {
            Some(orbiters_qr::Glimpse::Cut(_)) => self.cut += 1,
            Some(orbiters_qr::Glimpse::Unclear) => self.unclear += 1,
            None => {}
        }
    }

    fn maybe(&mut self, connected: bool) {
        let since = *self.since.get_or_insert_with(std::time::Instant::now);
        if since.elapsed() < NOTE_EVERY {
            return;
        }
        let n: u32 = self.looks.iter().sum();
        let mean = if n > 0 { self.took.as_secs_f64() * 1000.0 / n as f64 } else { 0.0 };
        crate::overlay::vr_log::line(&format!(
            "qr scan: {n} looks in {:.0} s (whole {}, closer {}, middle {}) of {} x {} frames, reading {mean:.1} ms each (slowest {:.1}); codes read {}, glimpsed cut {}, unclear {}; connected {connected}", // i18n: not shown (a log's line)
            since.elapsed().as_secs_f64(),
            self.looks[0],
            self.looks[1],
            self.looks[2],
            self.size.0,
            self.size.1,
            self.slowest.as_secs_f64() * 1000.0,
            self.read,
            self.cut,
            self.unclear
        ));
        *self = Self { since: Some(std::time::Instant::now()), ..Default::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scans (by their sender's name) whose decoder panics on its next pictures, and how many times.
    #[cfg(any(windows, target_os = "macos"))]
    static PANICS_FOR: std::sync::Mutex<Vec<(String, u32)>> = std::sync::Mutex::new(Vec::new());

    /// The decoder of the scan reading `wanted` panics here when a test asked for it (SWEEP-14: as a crafted grid could).
    #[cfg(any(windows, target_os = "macos"))]
    pub(super) fn maybe_panic(wanted: &str) {
        let mut all = PANICS_FOR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(left) = all.iter_mut().find(|(w, n)| w == wanted && *n > 0) {
            left.1 -= 1;
            drop(all);
            panic!("index out of bounds: a crafted grid (test)");
        }
    }

    /// SWEEP-14: a picture that makes the decoder panic costs that picture: the scan goes on and reads the code it shows next,
    /// and the counter says what was skipped. Without the guard the scanner's thread ended there and the code was never read.
    #[cfg(windows)]
    #[test]
    fn a_picture_that_makes_the_decoder_panic_costs_that_picture_not_the_scan() {
        let mut panics = 0;
        assert_eq!(contained(&mut panics, || -> u8 { panic!("a crafted grid") }), None);
        assert_eq!((panics, contained(&mut panics, || 7)), (1, Some(7)));
        let name = format!("OrbitersQrPanic{}", std::process::id());
        PANICS_FOR.lock().unwrap().push((name.clone(), 3));
        let seen = scan_a_code(&name, "https://orbiters.cc/after-a-panic");
        assert!(seen.contains(&QrSeen::Code("https://orbiters.cc/after-a-panic".into())), "{seen:?}");
        assert!(!seen.iter().any(|s| matches!(s, QrSeen::Failed(_))), "{seen:?}");
        assert_eq!(PANICS_FOR.lock().unwrap().iter().find(|(w, _)| *w == name).map(|p| p.1), Some(0), "the decoder panicked three times");
    }

    /// A test's own Spout sender showing `link`'s code, scanned until the code is read (60 s at most): what was said.
    #[cfg(windows)]
    fn scan_a_code(name: &str, link: &str) -> Vec<QrSeen> {
        use std::sync::mpsc;
        let (w, h) = (1280usize, 720usize);
        let (bgra, stride) = frame_with_code(link, w, h, 500, 200, 8);
        let rgba: Vec<u8> = (0..h).flat_map(|y| bgra[y * stride..y * stride + w * 4].to_vec()).collect();
        let stop_sending = Arc::new(AtomicBool::new(false));
        let sending = {
            let (name, stop) = (name.to_string(), stop_sending.clone());
            std::thread::spawn(move || {
                let mut sender = orbiters_spout::Sender::new(&name, w as u32, h as u32).unwrap();
                let mut frame = rgba;
                let mut k = 0u8;
                while !stop.load(Ordering::Relaxed) {
                    k = k.wrapping_add(1);
                    frame[(h / 5 * w) * 4] = k;
                    sender.send_rgba(&frame).unwrap();
                    std::thread::sleep(Duration::from_millis(16));
                }
            })
        };
        let patience = Duration::from_secs(60);
        let deadline = std::time::Instant::now() + patience;
        while !orbiters_spout::list_senders().contains(&name.to_string()) {
            assert!(std::time::Instant::now() < deadline, "the test's Spout sender never showed");
            std::thread::sleep(Duration::from_millis(20));
        }
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let scan = start(name.to_string(), Box::new(move |seen| tx.lock().unwrap().send(seen).unwrap())).unwrap();
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + patience;
        while !seen.contains(&QrSeen::Code(link.into())) && std::time::Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.push(s);
            }
        }
        drop(scan);
        stop_sending.store(true, Ordering::Relaxed);
        sending.join().unwrap();
        seen
    }

    /// A frame `width` × `height` (BGRA, rows padded as a mapped texture's) of a grey world with `text`'s QR code drawn
    /// `module` pixels a module with its top left at (`x`, `y`), and the stride.
    fn frame_with_code(text: &str, width: usize, height: usize, x: usize, y: usize, module: usize) -> (Vec<u8>, usize) {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let n = code.width();
        let colors = code.to_colors();
        let stride = width * 4 + 64;
        let mut data = vec![0u8; stride * height];
        for py in 0..height {
            for px in 0..width {
                // A dim, slightly uneven world around a white card with its quiet zone.
                let mut v = (90 + (px * 7 + py * 3) % 40) as u8;
                let (cx, cy) = (px as isize - x as isize, py as isize - y as isize);
                let quiet = 4 * module as isize;
                let side = (n * module) as isize;
                if cx >= -quiet && cy >= -quiet && cx < side + quiet && cy < side + quiet {
                    v = 235;
                    if cx >= 0 && cy >= 0 && cx < side && cy < side {
                        let (mx, my) = (cx as usize / module, cy as usize / module);
                        if colors[my * n + mx] == qrcode::Color::Dark {
                            v = 20;
                        }
                    }
                }
                let i = py * stride + px * 4;
                data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        (data, stride)
    }

    #[test]
    fn a_code_in_a_stream_frame_is_read_from_its_grey_copy() {
        let link = "https://orbiters.cc/maps/wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
        let (w, h) = (1920, 1080);
        let (data, stride) = frame_with_code(link, w, h, 1180, 300, 8);
        let frame = Pixels { width: w, height: h, stride, data: &data, bgra: true };
        let (gw, gh, picture) = grey(&frame, View::Whole);
        assert_eq!((gw, gh), (960, 540), "shrunk to at most {MOST_WIDTH} wide, its shape kept");
        assert_eq!(decode(gw, gh, &picture), [link]);
        // A frame without a code reads nothing.
        let (empty, stride) = frame_with_code(link, 64, 64, 1000, 1000, 1);
        let blank = Pixels { width: 64, height: 64, stride, data: &empty, bgra: false };
        let (gw, gh, picture) = grey(&blank, View::Whole);
        assert!(decode(gw, gh, &picture).is_empty());
    }

    #[test]
    fn a_small_code_in_the_middle_is_read_at_full_detail() {
        let link = "https://example.com/a-rather-long-path/that-makes-a-denser-code?with=query&and=more";
        let (w, h) = (1920, 1080);
        // Two pixels a module: too fine once the whole picture is halved, read in the middle at full size.
        let (data, stride) = frame_with_code(link, w, h, 900, 480, 2);
        let frame = Pixels { width: w, height: h, stride, data: &data, bgra: false };
        let (gw, gh, picture) = grey(&frame, View::Middle);
        assert_eq!((gw, gh), (960, 540));
        assert_eq!(decode(gw, gh, &picture), [link]);
    }

    /// The scanner's thread on a real Spout sender (a test's own name, as Settings' Spout sender): frames come, the code
    /// is read, and once the sender goes the camera is said to send none.
    #[cfg(windows)]
    #[test]
    fn the_scanner_reads_a_spout_sender_and_says_when_it_stops() {
        use std::sync::mpsc;
        let link = "https://orbiters.cc/n4n0";
        let name = format!("OrbitersQrTest{}", std::process::id());
        let (w, h) = (1280usize, 720usize);
        let (bgra, stride) = frame_with_code(link, w, h, 500, 200, 8);
        // Spout wants tightly packed RGBA rows.
        let rgba: Vec<u8> = (0..h).flat_map(|y| bgra[y * stride..y * stride + w * 4].to_vec()).collect();
        let stop_sending = Arc::new(AtomicBool::new(false));
        let sending = {
            let (name, stop) = (name.clone(), stop_sending.clone());
            std::thread::spawn(move || {
                let mut sender = orbiters_spout::Sender::new(&name, w as u32, h as u32).unwrap();
                let mut frame = rgba;
                let mut k = 0u8;
                while !stop.load(Ordering::Relaxed) {
                    // A pixel in a probed row changes, as a live camera's would.
                    k = k.wrapping_add(1);
                    frame[(h / 5 * w) * 4] = k;
                    sender.send_rgba(&frame).unwrap();
                    std::thread::sleep(Duration::from_millis(16));
                }
            })
        };
        // A busy machine (the whole test suite) leaves the below-normal scanner without a turn for seconds: what must come is
        // waited for this long, which only a failing test waits in full.
        let patience = Duration::from_secs(60);
        // The scan starts once the sender is there: one that came later than the scanner's 2.5 s would rightly be said to
        // send nothing first.
        let deadline = std::time::Instant::now() + patience;
        while !orbiters_spout::list_senders().contains(&name) {
            assert!(std::time::Instant::now() < deadline, "the test's Spout sender never showed");
            std::thread::sleep(Duration::from_millis(20));
        }
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let scan = start(name, Box::new(move |seen| tx.lock().unwrap().send(seen).unwrap())).unwrap();
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + patience;
        while !seen.contains(&QrSeen::Code(link.into())) && std::time::Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.push(s);
            }
        }
        assert_eq!(seen.first(), Some(&QrSeen::Camera(true)), "{seen:?}");
        assert!(seen.contains(&QrSeen::Code(link.into())), "{seen:?}");
        stop_sending.store(true, Ordering::Relaxed);
        sending.join().unwrap();
        let deadline = std::time::Instant::now() + patience;
        let mut gone = None;
        while gone.is_none() && std::time::Instant::now() < deadline {
            gone = rx.recv_timeout(Duration::from_millis(200)).ok().filter(|s| matches!(s, QrSeen::Camera(_)));
        }
        assert_eq!(gone, Some(QrSeen::Camera(false)), "the camera was never said to have stopped: {seen:?}");
        drop(scan);
    }

    /// K-qr-detect: Enzo's shot1 was a light code on a purple poster: never read by 0.3.5-dev.56 (rqrr reads dark modules on
    /// light only). The scanner's looks read it now, and say a code cut by the frame's edge.
    #[test]
    fn a_code_in_negative_is_read_and_a_cut_one_is_said() {
        let link = "https://qrco.de/bdeKjG";
        let (w, h) = (1920, 1080);
        let (mut data, stride) = frame_with_code(link, w, h, 760, 300, 9);
        // In negative, on purple: light modules, a dark ground.
        for px in data.chunks_mut(4) {
            let v = px[0];
            px.copy_from_slice(&[(255 - v) / 2 + 60, (255 - v) / 3 + 20, (255 - v) / 2 + 70, 255]);
        }
        let frame = Pixels { width: w, height: h, stride, data: &data, bgra: true };
        let (gw, gh, picture) = grey(&frame, View::Whole);
        assert!(orbiters_qr::read_with(gw, gh, &picture, true, &[orbiters_qr::Pass::Rows]).codes.is_empty(), "rqrr alone, as before");
        let mut looks = orbiters_qr::Looks::default();
        let copy = looks.copy(&frame);
        assert_eq!(looks.read(&copy).codes, [link]);
        // The same code with a third of it past the right edge.
        let (data, stride) = frame_with_code(link, w, h, w - 200, 400, 9);
        let frame = Pixels { width: w, height: h, stride, data: &data, bgra: true };
        let mut looks = orbiters_qr::Looks::default();
        let copy = looks.copy(&frame);
        let reading = looks.read(&copy);
        assert!(reading.codes.is_empty());
        assert_eq!(reading.glimpse, Some(orbiters_qr::Glimpse::Cut(Some(orbiters_qr::Side::Right))), "cut off the right side, said so");
    }

    /// What is said of a glimpsed code: only once two of three looks saw it, unsaid after two looks without, at once when a
    /// code is read.
    #[test]
    fn a_glimpse_is_said_when_it_holds() {
        use orbiters_qr::Glimpse;
        let mut said = Glimpses::default();
        let left = Glimpse::Cut(Some(orbiters_qr::Side::Left));
        assert_eq!(said.look(Some(left)), None, "one look is not enough");
        assert_eq!(said.look(Some(left)), Some(Some(QrGlimpse::Cut(Some(QrSide::Left)))));
        assert_eq!(said.look(Some(left)), None, "said once");
        assert_eq!(said.look(Some(Glimpse::Cut(Some(orbiters_qr::Side::Top)))), None, "the side said stays while a recent look saw it there");
        assert_eq!(said.look(None), None, "one look without: still there");
        assert_eq!(said.look(None), Some(None), "two: gone");
        assert_eq!(said.look(Some(Glimpse::Unclear)), None);
        assert_eq!(said.look(None), None);
        assert_eq!(said.look(Some(Glimpse::Unclear)), Some(Some(QrGlimpse::Unclear)), "two of three");
        assert_eq!(said.read(), Some(None), "read: unsaid at once");
        assert_eq!(said.read(), None);
    }

    #[test]
    fn a_4k_frame_shrinks_by_whole_steps() {
        let data = vec![200u8; 3840 * 4 * 2160];
        let frame = Pixels { width: 3840, height: 2160, stride: 3840 * 4, data: &data, bgra: true };
        let (gw, gh, picture) = grey(&frame, View::Whole);
        assert_eq!((gw, gh), (960, 540));
        assert!(picture.iter().all(|&v| v == 200));
        let (mw, mh, _) = grey(&frame, View::Middle);
        assert_eq!((mw, mh), (960, 540));
    }
}
