//! Looks at the game's window on a Mac: ScreenCaptureKit's picture of that one window, alone, taken only while there is something to look for ([`Looker::ask`]). The frames
//! stay in memory the GPU and the processor share: a look reads the few pixels the detectors ask for ([`detect::parts`]), and nothing is kept.
//!
//! The looks run on ScreenCaptureKit's own queue, and starting and stopping on a thread of this module's, which sleeps while nothing is asked. The loop never waits for either.

use orbiters_core::locks::*;
use super::super::detect::{self, CameraLook, Part};
use super::super::zones::Around;
use super::game::Shot;
use super::wheel::{self, Fit, Scores};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{class, define_class, msg_send, AnyThread, DefinedClass, Encode, Encoding};
use objc2_foundation::{NSArray, NSError};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A time of CoreMedia's.
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: the same fields in the same order as CoreMedia's.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct("?", &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING]);
}

/// A sample buffer of CoreMedia's, as ScreenCaptureKit hands it over.
#[repr(C)]
struct SampleBuffer {
    _opaque: [u8; 0],
}

// SAFETY: an opaque type, only ever behind a pointer.
unsafe impl objc2::RefEncode for SampleBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// A queue of Grand Central Dispatch's is an Objective-C object.
type Queue = *mut AnyObject;

extern "C" {
    fn dispatch_queue_create(label: *const std::ffi::c_char, attributes: *const c_void) -> Queue;
}

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(buffer: *mut SampleBuffer) -> *mut c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceSRGB: &'static objc2_core_foundation::CFString;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
}

/// `kCVPixelFormatType_32BGRA`.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `kCVPixelBufferLock_ReadOnly`.
const READ_ONLY: u64 = 1;
/// How long macOS is given to list its windows, and to start or stop a picture.
const ANSWERS_WITHIN: Duration = Duration::from_secs(3);
/// After a failure, nothing is started again for this long.
const AGAIN_AFTER: Duration = Duration::from_secs(1);

/// What the loop asks of the looks now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ask {
    /// The game's window and its process.
    pub window: u32,
    pub owner: u32,
    pub shot: Shot,
    /// VRChat's menus are looked for, and how long from a look to the next.
    pub menus: bool,
    pub every: Duration,
    /// Its camera's panel is looked for.
    pub camera: Option<CameraAsk>,
    /// Where the wheel is looked for.
    pub search: Search,
    /// The look this is of, and when it began: one that finds nothing for [`TELLS_AFTER`] says once where the disc most like a wheel is, if any.
    pub look: u32,
    pub began: Instant,
    /// The frame it says that of is kept as a picture in the logs' folder (a setting, off by default).
    pub keep: bool,
}

/// How VRChat's camera panel is looked for ([`detect::camera_look`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraAsk {
    /// The game's picture as its places are counted ([`super::game::Game::client`]).
    pub client: egui::Vec2,
    /// Where it shows now, there.
    pub shown: Option<egui::Rect>,
    /// VRChat says its camera is open.
    pub open: bool,
}

/// Where the wheel is looked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Search {
    InPlace,
    /// Around its place too (R was pressed).
    Around,
    /// Where it was found.
    At(Fit),
}

/// How long a look for the wheel goes on before it says what it saw.
const TELLS_AFTER: Duration = Duration::from_millis(900);

/// What a look measured: what its log says when it found nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Measured {
    pub menu: Option<Around>,
    /// The wheel's rim is in place.
    pub coming: bool,
    pub wheel: Scores,
    /// The best around its place, when it was looked for there.
    pub fit: Option<Fit>,
    /// The Quick Menu's icons found.
    pub icons: usize,
}

/// What a look found, and when its frame was read.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    pub at: Instant,
    /// What it saw of the menus, when they were looked for; and of the camera's panel, when that was.
    pub saw: Option<Measured>,
    pub camera: Option<CameraLook>,
    /// The look it is of.
    pub look: u32,
    /// The disc most like a wheel anywhere, when this look was asked for it.
    pub anywhere: Option<Option<Fit>>,
}

/// Why no picture of the window could be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// macOS does not let this app record the screen (it was never allowed, or no longer is).
    NotAllowed,
    /// The person stopped it from macOS's own menu.
    Stopped,
    /// Anything else, as macOS said it.
    Failed(String),
}

impl Trouble {
    /// From an error of ScreenCaptureKit's: its domain and code.
    fn of(domain: &str, code: isize, words: &str) -> Self {
        if domain.contains("ScreenCaptureKit") {
            match code {
                // userDeclined, missingEntitlements
                -3801 | -3803 => return Self::NotAllowed,
                // userStopped, and the system's own stop button
                -3817 | -3821 => return Self::Stopped,
                _ => {}
            }
        }
        Self::Failed(format!("{domain} {code}: {words}"))
    }
}

/// What the looks found, read and wrote between the loop and ScreenCaptureKit's queue.
struct Shared {
    asked: Mutex<Option<Ask>>,
    wake: Condvar,
    stop: AtomicBool,
    trouble: Mutex<Option<Trouble>>,
    /// Pictures running now, as told to macOS (what a test of the cost at rest asks).
    running: AtomicBool,
    /// Frames that came, ever.
    frames: AtomicU64,
}

/// The menu's look at a whole frame of `shot` (top-down BGRA, `stride` bytes a row): `None` where the frame is not of that size (the window's older one).
pub fn measure(bgra: &[u8], width: usize, height: usize, stride: usize, shot: &Shot, search: &Search) -> Option<Measured> {
    let whole = whole(bgra, width, height, stride, shot)?;
    let client = shot.client();
    let scores = wheel::in_place(&whole, client);
    let mut out = Measured { coming: scores.edge >= detect::WHEEL_EDGE, wheel: scores, ..Measured::default() };
    // The wheel first: it is cheaper, and the Quick Menu's row is not under it when it shows.
    let seen = match search {
        _ if scores.has(&wheel::IN_PLACE) && !matches!(search, Search::At(_)) => true,
        Search::InPlace => false,
        Search::Around => {
            out.fit = wheel::near(&whole, client);
            out.fit.is_some_and(|f| f.scores.has(&wheel::FIT))
        }
        Search::At(fit) => {
            out.fit = Some(*fit);
            wheel::still(&whole, client, fit)
        }
    };
    if seen {
        out.menu = Some(Around::Wheel);
    } else {
        out.fit = out.fit.filter(|_| matches!(search, Search::Around));
        out.icons = detect::icons_found(&whole, client);
        out.menu = (out.icons >= detect::ICONS_NEEDED).then_some(Around::QuickMenu);
    }
    Some(out)
}

/// The camera's look at a whole frame of `shot`: its panel's place, counted in the game's own picture.
pub fn camera(bgra: &[u8], width: usize, height: usize, stride: usize, shot: &Shot, ask: &CameraAsk) -> Option<CameraLook> {
    let whole = whole(bgra, width, height, stride, shot)?;
    let client = shot.client();
    let parts: Vec<Part> = detect::camera_parts(client)
        .iter()
        .map(|r| {
            let (x0, y0) = (r.min.x as u32, r.min.y as u32);
            let start = (y0 * whole.stride + x0 * 4) as usize;
            Part { x0, y0, width: r.width().ceil() as u32, height: r.height().ceil() as u32, stride: whole.stride, bgra: &whole.bgra[start.min(whole.bgra.len())..] }
        })
        .collect();
    let k = ask.client.y / client.y.max(1.0);
    let scaled = |r: egui::Rect, k: f32| egui::Rect::from_min_max((r.min.to_vec2() * k).to_pos2(), (r.max.to_vec2() * k).to_pos2());
    let look = detect::camera_look(&parts, client, ask.shown.map(|r| scaled(r, 1.0 / k)), ask.open);
    Some(CameraLook { found: look.found.map(|r| scaled(r, k)), at_shown: look.at_shown })
}

/// The game's own picture in a frame, as a part of itself.
fn whole<'a>(bgra: &'a [u8], width: usize, height: usize, stride: usize, shot: &Shot) -> Option<Part<'a>> {
    if (width, height) != (shot.width as usize, shot.height as usize) || stride < width * 4 || bgra.len() < stride * height || shot.client().y < 2.0 || width < 2 {
        return None;
    }
    Some(Part { x0: 0, y0: 0, width: shot.width, height: shot.height - shot.title, stride: stride as u32, bgra: &bgra[shot.title as usize * stride..] })
}

#[cfg(test)]
pub fn look(bgra: &[u8], width: usize, height: usize, stride: usize, shot: &Shot) -> Option<(Option<Around>, bool)> {
    measure(bgra, width, height, stride, shot, &Search::InPlace).map(|m| (m.menu, m.coming))
}

/// How many looked-at frames the logs' folder keeps.
const KEPT: usize = 12;

/// The looked-at frame in the logs' folder, the oldest of them beyond [`KEPT`] gone. (Only with the setting on.)
fn keep(bgra: &[u8], width: usize, height: usize, stride: usize) {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in bgra.chunks(stride).take(height) {
        rgba.extend(row[..width * 4].chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]));
    }
    let _ = std::thread::Builder::new().name("orbit-keeps".into()).spawn(move || {
        let Some(image) = image::RgbaImage::from_raw(width as u32, height as u32, rgba) else { return };
        let folder = orbiters_core::paths::logs_dir().join(KEPT_FOLDER);
        keep_in(&folder, &image);
    });
}

/// The folder of the logs' folder the kept frames are in.
pub const KEPT_FOLDER: &str = "overlay-looks";

fn keep_in(folder: &std::path::Path, image: &image::RgbaImage) {
    if std::fs::create_dir_all(folder).is_err() {
        return;
    }
    let name = format!("look-{}.png", orbiters_core::now_utc_iso().replace(':', "-"));
    let _ = image.save(folder.join(name));
    let mut old: Vec<_> = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "png")).collect();
    old.sort();
    for path in old.iter().take(old.len().saturating_sub(KEPT)) {
        let _ = std::fs::remove_file(path);
    }
}

struct OutputIvars {
    shared: Arc<Shared>,
    answers: mpsc::Sender<Answer>,
    /// When the menus were last looked for, and the camera's panel; and whether this stream was ended by macOS.
    last: Mutex<(Option<Instant>, Option<Instant>)>,
    /// The look that last said what is anywhere on its frame.
    told: AtomicU64,
    ended: Arc<AtomicBool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "OrbitersOverlayLooks"]
    #[ivars = OutputIvars]
    struct Output;

    impl Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn output(&self, _stream: *mut AnyObject, buffer: *mut SampleBuffer, kind: isize) {
            if kind == 0 {
                self.frame(buffer);
            }
        }

        #[unsafe(method(stream:didStopWithError:))]
        fn stopped(&self, _stream: *mut AnyObject, error: *mut NSError) {
            // SAFETY: the error ScreenCaptureKit passes, for the call.
            let why = unsafe { trouble(error) };
            *self.ivars().shared.trouble.locked() = Some(why);
            self.ivars().ended.store(true, Ordering::Release);
            self.ivars().shared.wake.notify_all();
        }
    }
);

impl Output {
    fn frame(&self, buffer: *mut SampleBuffer) {
        let this = self.ivars();
        // SAFETY: a plain query of the buffer ScreenCaptureKit passes; a frame with nothing new has no picture.
        let image = unsafe { CMSampleBufferGetImageBuffer(buffer) };
        if image.is_null() {
            return;
        }
        this.shared.frames.fetch_add(1, Ordering::Relaxed);
        let Some(ask) = *this.shared.asked.locked() else { return };
        let now = Instant::now();
        let (menus, camera) = {
            let mut last = this.last.locked();
            let menus = ask.menus && last.0.is_none_or(|t| now.duration_since(t) >= ask.every);
            let camera = ask.camera.filter(|_| last.1.is_none_or(|t| now.duration_since(t) >= detect::CAMERA_LOOK_EVERY));
            if !menus && camera.is_none() {
                return;
            }
            if menus {
                last.0 = Some(now);
            }
            if camera.is_some() {
                last.1 = Some(now);
            }
            (menus, camera)
        };
        // SAFETY: the picture is locked for reading while its rows are read, within its own size, and unlocked after.
        let (found, anywhere, camera) = unsafe {
            if CVPixelBufferLockBaseAddress(image, READ_ONLY) != 0 {
                return;
            }
            let (base, stride) = (CVPixelBufferGetBaseAddress(image), CVPixelBufferGetBytesPerRow(image));
            let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
            let bgra = if base.is_null() { &[][..] } else { std::slice::from_raw_parts(base, stride * height) };
            let camera = camera.and_then(|asked| self::camera(bgra, width, height, stride, &ask.shot, &asked));
            let found = menus.then(|| measure(bgra, width, height, stride, &ask.shot, &ask.search)).flatten();
            // Once a look that found nothing, late in it: by then VRChat's menu shows, if it was opened.
            let tells = !matches!(ask.search, Search::At(_)) && found.is_some_and(|f| f.menu.is_none()) && now.duration_since(ask.began) >= TELLS_AFTER && this.told.swap(u64::from(ask.look), Ordering::Relaxed) != u64::from(ask.look);
            let mut anywhere = None;
            if tells {
                if ask.search == Search::Around {
                    anywhere = Some(whole(bgra, width, height, stride, &ask.shot).and_then(|w| wheel::anywhere(&w, ask.shot.client())));
                }
                if ask.keep {
                    keep(bgra, width, height, stride);
                }
            }
            CVPixelBufferUnlockBaseAddress(image, READ_ONLY);
            (found, anywhere, camera)
        };
        crate::perf::elsewhere(crate::perf::Part::Capture, now.elapsed());
        if found.is_some() || camera.is_some() {
            let _ = this.answers.send(Answer { at: now, saw: found, camera, look: ask.look, anywhere });
        }
    }
}

/// What an error of macOS's says.
///
/// # Safety
/// `error` is null or a live error.
unsafe fn trouble(error: *mut NSError) -> Trouble {
    match error.as_ref() {
        Some(e) => Trouble::of(&e.domain().to_string(), e.code() as isize, &e.localizedDescription().to_string()),
        None => Trouble::Failed("no reason given".into()), // i18n: not shown (a log's line)
    }
}

/// Objects macOS's own queues hand to this module's thread: only moved, and each used by one thread at a time.
struct Sent<T>(T);
// SAFETY: Objective-C objects of ScreenCaptureKit, which may be used from any thread.
unsafe impl<T> Send for Sent<T> {}

/// The game's window as ScreenCaptureKit knows it: asked of macOS once for each size of it.
fn filter_of(window: u32, owner: u32) -> Result<Retained<AnyObject>, Trouble> {
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move |content: *mut AnyObject, error: *mut NSError| {
        // SAFETY: what ScreenCaptureKit passes lives for the call; the content is kept (retained) beyond it.
        let answer = unsafe {
            match Retained::retain(content) {
                Some(content) => Ok(Sent(content)),
                None => Err(trouble(error)),
            }
        };
        let _ = tx.send(answer);
    });
    // SAFETY: ScreenCaptureKit's own method and block (macOS 12.3 and later).
    unsafe {
        let () = msg_send![class!(SCShareableContent), getShareableContentExcludingDesktopWindows: true, onScreenWindowsOnly: true, completionHandler: &*done];
    }
    let content = rx.recv_timeout(ANSWERS_WITHIN).map_err(|_| Trouble::Failed("macOS did not list its windows".into()))??.0; // i18n: not shown (a log's line)
    // SAFETY: properties of ScreenCaptureKit's objects, of the types its headers give; the filter is made for a window of the content.
    unsafe {
        let windows: Retained<NSArray<AnyObject>> = msg_send![&content, windows];
        for w in windows.iter() {
            let id: u32 = msg_send![&w, windowID];
            if id != window {
                continue;
            }
            // Asked again whose it is: a window's number may be another program's by now.
            let app: Option<Retained<AnyObject>> = msg_send![&w, owningApplication];
            let pid: i32 = app.map_or(0, |a| msg_send![&a, processID]);
            if pid != owner as i32 {
                break;
            }
            let made: Allocated<AnyObject> = msg_send![class!(SCContentFilter), alloc];
            return Ok(msg_send![made, initWithDesktopIndependentWindow: &*w]);
        }
    }
    Err(Trouble::Failed("the window is not among those macOS shares".into())) // i18n: not shown (a log's line)
}

/// A running picture of the window.
struct Stream {
    stream: Retained<AnyObject>,
    config: Retained<AnyObject>,
    /// The pace its frames are asked at.
    pace: i32,
    _output: Retained<Output>,
    /// macOS ended it.
    ended: Arc<AtomicBool>,
}

impl Stream {
    fn start(filter: &AnyObject, ask: &Ask, shared: &Arc<Shared>, answers: &mpsc::Sender<Answer>, queue: Queue) -> Result<Self, Trouble> {
        let ended = Arc::new(AtomicBool::new(false));
        let output = Output::alloc().set_ivars(OutputIvars { shared: shared.clone(), answers: answers.clone(), last: Mutex::new((None, None)), told: AtomicU64::new(0), ended: ended.clone() });
        // SAFETY: NSObject's init on a freshly allocated object; then ScreenCaptureKit's own classes, methods and argument types.
        let pace = pace(ask);
        let (stream, config, output) = unsafe {
            let output: Retained<Output> = msg_send![super(output), init];
            let config: Retained<AnyObject> = msg_send![class!(SCStreamConfiguration), new];
            let () = msg_send![&config, setWidth: ask.shot.width as usize];
            let () = msg_send![&config, setHeight: ask.shot.height as usize];
            let () = msg_send![&config, setPixelFormat: BGRA];
            // The game alone: no pointer in it, no shadow round it (the picture is the window's frame, to the pixel).
            let () = msg_send![&config, setShowsCursor: false];
            let () = msg_send![&config, setQueueDepth: 3isize];
            let () = msg_send![&config, setMinimumFrameInterval: CMTime { value: 1, timescale: pace, flags: 1, epoch: 0 }];
            let () = msg_send![&config, setColorSpaceName: kCGColorSpaceSRGB];
            for (has, off) in [(objc2::sel!(setIgnoreShadowsSingleWindow:), true), (objc2::sel!(setScalesToFit:), true)] {
                let can: bool = msg_send![&config, respondsToSelector: has];
                if can {
                    if has == objc2::sel!(setScalesToFit:) {
                        let () = msg_send![&config, setScalesToFit: off];
                    } else {
                        let () = msg_send![&config, setIgnoreShadowsSingleWindow: off];
                    }
                }
            }
            let made: Allocated<AnyObject> = msg_send![class!(SCStream), alloc];
            let stream: Retained<AnyObject> = msg_send![made, initWithFilter: filter, configuration: &*config, delegate: &*output];
            let mut error: *mut NSError = std::ptr::null_mut();
            let added: bool = msg_send![&stream, addStreamOutput: &*output, type: 0isize, sampleHandlerQueue: queue, error: &mut error];
            if !added {
                return Err(trouble(error));
            }
            (stream, config, output)
        };
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: what ScreenCaptureKit passes lives for the call.
            let _ = tx.send(if error.is_null() { Ok(()) } else { Err(unsafe { trouble(error) }) });
        });
        // SAFETY: ScreenCaptureKit's own method and block.
        unsafe {
            let () = msg_send![&stream, startCaptureWithCompletionHandler: &*done];
        }
        let this = Self { stream, config, pace, _output: output, ended };
        match rx.recv_timeout(ANSWERS_WITHIN) {
            Ok(Ok(())) => Ok(this),
            Ok(Err(why)) => Err(why),
            Err(_) => Err(Trouble::Failed("macOS did not start the picture".into())), // i18n: not shown (a log's line)
        }
    }
}

/// How many frames a second are worth taking: every one of a game's just after a key, which is when a menu opens or closes, and while one shows what looks every 60 ms need.
fn pace(ask: &Ask) -> i32 {
    if ask.menus && ask.every.is_zero() {
        60
    } else if ask.camera.is_some() {
        // (a look each 30 ms: what goes is gone within a few frames)
        40
    } else {
        20
    }
}

impl Stream {
    /// Its frames at another pace from now on.
    fn follow(&mut self, ask: &Ask) {
        let pace = pace(ask);
        if pace == self.pace {
            return;
        }
        self.pace = pace;
        let done = RcBlock::new(|_: *mut NSError| {});
        // SAFETY: ScreenCaptureKit's own classes, methods, block and argument types; a stream that ended only says so.
        unsafe {
            let () = msg_send![&self.config, setMinimumFrameInterval: CMTime { value: 1, timescale: pace, flags: 1, epoch: 0 }];
            let () = msg_send![&self.stream, updateConfiguration: &*self.config, completionHandler: &*done];
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |_: *mut NSError| {
            let _ = tx.send(());
        });
        // SAFETY: ScreenCaptureKit's own method and block; stopping one macOS ended only says so.
        unsafe {
            let () = msg_send![&self.stream, stopCaptureWithCompletionHandler: &*done];
        }
        let _ = rx.recv_timeout(ANSWERS_WITHIN);
    }
}

/// The looks, off the loop.
pub struct Looker {
    shared: Arc<Shared>,
    answers: mpsc::Receiver<Answer>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Looker {
    pub fn start() -> Self {
        let shared = Arc::new(Shared { asked: Mutex::new(None), wake: Condvar::new(), stop: AtomicBool::new(false), trouble: Mutex::new(None), running: AtomicBool::new(false), frames: AtomicU64::new(0) });
        let (tx, answers) = mpsc::channel();
        let mine = shared.clone();
        let thread = std::thread::Builder::new().name("orbit-looks".into()).spawn(move || run(&mine, &tx)).ok();
        Self { shared, answers, thread }
    }

    /// What to look for from now on; `None`: nothing, and the picture ends.
    pub fn ask(&self, ask: Option<Ask>) {
        let mut asked = self.shared.asked.locked();
        if *asked != ask {
            *asked = ask;
            self.shared.wake.notify_all();
        }
    }

    /// What was found since the last call, in order.
    pub fn answers(&self) -> Vec<Answer> {
        self.answers.try_iter().collect()
    }

    /// Why there was no picture, since the last call.
    pub fn trouble(&self) -> Option<Trouble> {
        self.shared.trouble.locked().take()
    }

    /// Whether a picture runs now, and how many frames came ever (what the lab measures).
    pub fn running(&self) -> (bool, u64) {
        (self.shared.running.load(Ordering::Relaxed), self.shared.frames.load(Ordering::Relaxed))
    }
}

impl Drop for Looker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread that starts and stops the picture; it sleeps while what is asked stays the same.
fn run(shared: &Arc<Shared>, answers: &mpsc::Sender<Answer>) {
    // SAFETY: a plain call; the queue lives as long as the process.
    let queue = unsafe { dispatch_queue_create(c"cc.orbiters.overlay-looks".as_ptr(), std::ptr::null()) };
    let mut filter: Option<(Ask, Retained<AnyObject>)> = None;
    let mut stream: Option<(Ask, Stream)> = None;
    // What differs for macOS: another window or size of it (the pace is the looks' own).
    let same = |a: &Ask, b: &Ask| (a.window, a.owner, a.shot) == (b.window, b.owner, b.shot);
    loop {
        let ask = {
            let mut asked = shared.asked.locked();
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    break;
                }
                let ended = stream.as_ref().is_some_and(|(_, s)| s.ended.load(Ordering::Acquire));
                let changed = match (&*asked, &mut stream) {
                    (None, None) => false,
                    (Some(a), Some((now, running))) if same(a, now) => {
                        running.follow(a);
                        false
                    }
                    _ => true,
                };
                if ended || changed {
                    break;
                }
                asked = shared.wake.wait_timeout_locked(asked, Duration::from_secs(60)).0;
            }
            *asked
        };
        // What ScreenCaptureKit makes meanwhile goes with each turn.
        let over = objc2::rc::autoreleasepool(|_| {
            // The old picture ends before another starts, and with nothing asked.
            if let Some((_, old)) = stream.take() {
                let ended = old.ended.load(Ordering::Acquire);
                drop(old);
                shared.running.store(false, Ordering::Relaxed);
                if ended {
                    filter = None;
                    pause(shared, AGAIN_AFTER);
                }
            }
            if shared.stop.load(Ordering::Acquire) {
                return true;
            }
            // (macOS may have ended it: what is asked is read again)
            let Some(ask) = ask.filter(|a| shared.asked.locked().is_some_and(|now| same(&now, a))) else { return false };
            let made = match filter.take().filter(|(of, _)| same(of, &ask)) {
                Some((_, kept)) => Ok(kept),
                None => filter_of(ask.window, ask.owner),
            };
            let started = made.and_then(|f| {
                let s = Stream::start(&f, &ask, shared, answers, queue)?;
                filter = Some((ask, f));
                Ok(s)
            });
            match started {
                Ok(s) => {
                    shared.running.store(true, Ordering::Relaxed);
                    stream = Some((ask, s));
                }
                Err(why) => {
                    *shared.trouble.locked() = Some(why);
                    pause(shared, AGAIN_AFTER);
                }
            }
            false
        });
        if over {
            return;
        }
    }
}

/// Waits `long`, or until the looks end.
fn pause(shared: &Shared, long: Duration) {
    let until = Instant::now() + long;
    while !shared.stop.load(Ordering::Acquire) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_kept_frames_stay() {
        let root = std::env::temp_dir().join(format!("orbiters-kept-looks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..KEPT + 3 {
            std::fs::write(root.as_path().join(format!("look-2000-01-01T00-00-{i:02}Z.png")), b"").unwrap();
        }
        std::fs::write(root.as_path().join("notes.txt"), b"").unwrap();
        keep_in(&root, &image::RgbaImage::new(2, 2));
        let names: Vec<String> = std::fs::read_dir(root.as_path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        let pictures: Vec<&String> = names.iter().filter(|n| n.ends_with(".png")).collect();
        assert_eq!(pictures.len(), KEPT, "{names:?}");
        assert!(names.contains(&"notes.txt".to_string()), "what is not its own stays");
        assert!(pictures.iter().any(|n| !n.contains("2000-01-01")), "the new one stays");
        assert!(!names.iter().any(|n| n.contains("00-00Z") || n.contains("00-01Z")), "the oldest went: {names:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
