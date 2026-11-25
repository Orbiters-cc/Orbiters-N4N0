//! Looks at the game's window on a Mac: ScreenCaptureKit's picture of that one window, alone, taken only while there is something to look for ([`Looker::ask`]). The frames
//! stay in memory the GPU and the processor share: a look reads the few pixels the detectors ask for ([`detect::parts`]), and nothing is kept.
//!
//! The looks run on ScreenCaptureKit's own queue, and starting and stopping on a thread of this module's, which sleeps while nothing is asked. The loop never waits for either.

use super::super::detect::{self, Part};
use super::super::zones::Around;
use super::game::Shot;
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
    /// How long from a look to the next.
    pub every: Duration,
}

/// What a look found, and when its frame was read.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    pub at: Instant,
    pub menu: Option<Around>,
    /// The wheel's rim is in place.
    pub coming: bool,
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
pub fn look(bgra: &[u8], width: usize, height: usize, stride: usize, shot: &Shot) -> Option<(Option<Around>, bool)> {
    if (width, height) != (shot.width as usize, shot.height as usize) || stride < width * 4 || bgra.len() < stride * height {
        return None;
    }
    let client = shot.client();
    let parts = detect::parts(client).map(|r| {
        let (x0, y0) = (r.min.x.max(0.0) as u32, r.min.y.max(0.0) as u32);
        let (w, h) = ((r.width().ceil() as u32).min(shot.width - x0.min(shot.width)), (r.height().ceil() as u32).min(client.y as u32 - y0.min(client.y as u32)));
        let start = (y0 + shot.title) as usize * stride + x0 as usize * 4;
        (w > 1 && h > 1).then(|| Part { x0, y0, width: w, height: h, stride: stride as u32, bgra: &bgra[start.min(bgra.len())..] })
    });
    let [Some(wheel), Some(row)] = parts else { return None };
    Some((detect::look(&wheel, &row, client), detect::wheel_coming(&wheel, client)))
}

struct OutputIvars {
    shared: Arc<Shared>,
    answers: mpsc::Sender<Answer>,
    /// When the last look was, and whether this stream was ended by macOS.
    last: Mutex<Option<Instant>>,
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
            *self.ivars().shared.trouble.lock().unwrap() = Some(why);
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
        let Some(ask) = *this.shared.asked.lock().unwrap() else { return };
        let now = Instant::now();
        {
            let mut last = this.last.lock().unwrap();
            if last.is_some_and(|t| now.duration_since(t) < ask.every) {
                return;
            }
            *last = Some(now);
        }
        // SAFETY: the picture is locked for reading while its rows are read, within its own size, and unlocked after.
        let found = unsafe {
            if CVPixelBufferLockBaseAddress(image, READ_ONLY) != 0 {
                return;
            }
            let (base, stride) = (CVPixelBufferGetBaseAddress(image), CVPixelBufferGetBytesPerRow(image));
            let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
            let found = if base.is_null() { None } else { look(std::slice::from_raw_parts(base, stride * height), width, height, stride, &ask.shot) };
            CVPixelBufferUnlockBaseAddress(image, READ_ONLY);
            found
        };
        crate::perf::elsewhere(crate::perf::Part::Capture, now.elapsed());
        if let Some((menu, coming)) = found {
            let _ = this.answers.send(Answer { at: now, menu, coming });
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
    _output: Retained<Output>,
    /// macOS ended it.
    ended: Arc<AtomicBool>,
}

impl Stream {
    fn start(filter: &AnyObject, ask: &Ask, shared: &Arc<Shared>, answers: &mpsc::Sender<Answer>, queue: Queue) -> Result<Self, Trouble> {
        let ended = Arc::new(AtomicBool::new(false));
        let output = Output::alloc().set_ivars(OutputIvars { shared: shared.clone(), answers: answers.clone(), last: Mutex::new(None), ended: ended.clone() });
        // SAFETY: NSObject's init on a freshly allocated object; then ScreenCaptureKit's own classes, methods and argument types.
        let (stream, output) = unsafe {
            let output: Retained<Output> = msg_send![super(output), init];
            let config: Retained<AnyObject> = msg_send![class!(SCStreamConfiguration), new];
            let () = msg_send![&config, setWidth: ask.shot.width as usize];
            let () = msg_send![&config, setHeight: ask.shot.height as usize];
            let () = msg_send![&config, setPixelFormat: BGRA];
            // The game alone: no pointer in it, no shadow round it (the picture is the window's frame, to the pixel).
            let () = msg_send![&config, setShowsCursor: false];
            let () = msg_send![&config, setQueueDepth: 3isize];
            let () = msg_send![&config, setMinimumFrameInterval: CMTime { value: 1, timescale: 60, flags: 1, epoch: 0 }];
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
            (stream, output)
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
        let this = Self { stream, _output: output, ended };
        match rx.recv_timeout(ANSWERS_WITHIN) {
            Ok(Ok(())) => Ok(this),
            Ok(Err(why)) => Err(why),
            Err(_) => Err(Trouble::Failed("macOS did not start the picture".into())), // i18n: not shown (a log's line)
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
        let mut asked = self.shared.asked.lock().unwrap();
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
        self.shared.trouble.lock().unwrap().take()
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
            let mut asked = shared.asked.lock().unwrap();
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    break;
                }
                let ended = stream.as_ref().is_some_and(|(_, s)| s.ended.load(Ordering::Acquire));
                let changed = match (&*asked, &stream) {
                    (None, None) => false,
                    (Some(a), Some((now, _))) => !same(a, now),
                    _ => true,
                };
                if ended || changed {
                    break;
                }
                asked = shared.wake.wait_timeout(asked, Duration::from_secs(60)).unwrap().0;
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
            let Some(ask) = ask.filter(|a| shared.asked.lock().unwrap().is_some_and(|now| same(&now, a))) else { return false };
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
                    *shared.trouble.lock().unwrap() = Some(why);
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
