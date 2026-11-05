//! Core Audio (macOS): the audio queues this app plays and records through, the sound devices they name, the default
//! output's volume, and whether macOS lets this app use the microphone.
//!
//! An audio queue converts between its own format and the device's (another sample rate, other channels), as WASAPI's
//! autoconvert does, and calls back on a thread of its own rather than the device's real-time one. So [`Render`] and
//! [`Capture`] only note in the callback that a buffer came back, and whoever waits on their [`Turn`] does the work:
//! the same shape as a WASAPI client waiting on its event.
//!
//! A queue is opened on a device by its UID, never left to follow the default on its own: what follows the default
//! looks at [`default_uid`] and opens a new queue, so the moment it moves is known.
//!
//! The few calls used are declared by hand. A process's use of the microphone is attributed by macOS to the app it
//! belongs to (the process responsible for it), so the workers inside the app's bundle share the app's answer.

use block2::RcBlock;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

type OsStatus = i32;
type CfString = *const c_void;
type QueueRef = *mut c_void;
type BufferRef = *mut QueueBuffer;
type ObjectId = u32;

const fn four_cc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const LINEAR_PCM: u32 = four_cc(b"lpcm");
const FORMAT_FLOAT_PACKED: u32 = 1 | (1 << 3);
const SYSTEM_OBJECT: ObjectId = 1;
const ELEMENT_MAIN: u32 = 0;
const SCOPE_GLOBAL: u32 = four_cc(b"glob");
const SCOPE_INPUT: u32 = four_cc(b"inpt");
const SCOPE_OUTPUT: u32 = four_cc(b"outp");
const HARDWARE_DEVICES: u32 = four_cc(b"dev#");
const HARDWARE_DEFAULT_INPUT: u32 = four_cc(b"dIn ");
const HARDWARE_DEFAULT_OUTPUT: u32 = four_cc(b"dOut");
const HARDWARE_UID_TO_DEVICE: u32 = four_cc(b"uidd");
const DEVICE_UID: u32 = four_cc(b"uid ");
const OBJECT_NAME: u32 = four_cc(b"lnam");
const DEVICE_STREAMS: u32 = four_cc(b"slay");
const DEVICE_RATE: u32 = four_cc(b"nsrt");
const DEVICE_ALIVE: u32 = four_cc(b"livn");
const DEVICE_HIDDEN: u32 = four_cc(b"hidn");
const DEVICE_RUNNING: u32 = four_cc(b"gone");
const DEVICE_MUTE: u32 = four_cc(b"mute");
/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`: the one volume of a device with a level per channel.
const DEVICE_VOLUME: u32 = four_cc(b"vmvc");
const QUEUE_DEVICE: u32 = four_cc(b"aqcd");
const TIME_HOST_VALID: u32 = 1 << 1;
const UTF8: u32 = 0x0800_0100;

/// `AudioStreamBasicDescription`.
#[repr(C)]
struct StreamFormat {
    rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

/// `AudioQueueBuffer`.
#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    size: u32,
    user_data: *mut c_void,
    description_capacity: u32,
    descriptions: *mut c_void,
    description_count: u32,
}

/// `AudioTimeStamp`.
#[repr(C)]
struct TimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: [u8; 24],
    flags: u32,
    reserved: u32,
}

/// `AudioObjectPropertyAddress`.
#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

type OutputCallback = unsafe extern "C" fn(user: *mut c_void, queue: QueueRef, buffer: BufferRef);
type InputCallback =
    unsafe extern "C" fn(user: *mut c_void, queue: QueueRef, buffer: BufferRef, start: *const TimeStamp, packets: u32, descriptions: *const c_void);

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioQueueNewOutput(
        format: *const StreamFormat,
        callback: OutputCallback,
        user: *mut c_void,
        run_loop: *const c_void,
        mode: *const c_void,
        flags: u32,
        queue: *mut QueueRef,
    ) -> OsStatus;
    fn AudioQueueNewInput(
        format: *const StreamFormat,
        callback: InputCallback,
        user: *mut c_void,
        run_loop: *const c_void,
        mode: *const c_void,
        flags: u32,
        queue: *mut QueueRef,
    ) -> OsStatus;
    fn AudioQueueAllocateBuffer(queue: QueueRef, bytes: u32, buffer: *mut BufferRef) -> OsStatus;
    fn AudioQueueEnqueueBuffer(queue: QueueRef, buffer: BufferRef, descriptions: u32, description: *const c_void) -> OsStatus;
    fn AudioQueueStart(queue: QueueRef, at: *const TimeStamp) -> OsStatus;
    fn AudioQueueStop(queue: QueueRef, immediate: u8) -> OsStatus;
    fn AudioQueueDispose(queue: QueueRef, immediate: u8) -> OsStatus;
    fn AudioQueueSetProperty(queue: QueueRef, property: u32, data: *const c_void, size: u32) -> OsStatus;
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectHasProperty(object: ObjectId, address: *const Address) -> u8;
    fn AudioObjectGetPropertyDataSize(object: ObjectId, address: *const Address, qualifier_size: u32, qualifier: *const c_void, size: *mut u32) -> OsStatus;
    fn AudioObjectGetPropertyData(
        object: ObjectId,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        object: ObjectId,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> OsStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> CfString;
    fn CFStringGetLength(text: CfString) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(text: CfString, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(object: *const c_void);
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    /// The `AVMediaType` of sound, an `NSString`.
    static AVMediaTypeAudio: *const AnyObject;
}

extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
    fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
}

/// A status of Core Audio as an error's words.
fn check(what: &str, status: OsStatus) -> Result<(), String> {
    if status == 0 {
        return Ok(());
    }
    let code = status.to_be_bytes();
    if code.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        Err(format!("{what}: Core Audio said '{}'", String::from_utf8_lossy(&code)))
    } else {
        Err(format!("{what}: Core Audio said {status}"))
    }
}

/// A `CFString` to text.
fn text_of(string: CfString) -> Option<String> {
    if string.is_null() {
        return None;
    }
    // SAFETY: a valid string; the buffer is as long as the size it is given.
    unsafe {
        let most = CFStringGetMaximumSizeForEncoding(CFStringGetLength(string), UTF8) + 1;
        let mut buffer = vec![0 as c_char; most.max(1) as usize];
        if CFStringGetCString(string, buffer.as_mut_ptr(), most, UTF8) == 0 {
            return None;
        }
        Some(CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned())
    }
}

/// `text` as a `CFString` for the time it lives.
struct Cf(CfString);

impl Cf {
    fn new(text: &str) -> Option<Self> {
        let text = CString::new(text).ok()?;
        // SAFETY: a valid C string; the copy is released on drop.
        let string = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
        (!string.is_null()).then_some(Self(string))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: created above and owned here.
        unsafe { CFRelease(self.0) };
    }
}

fn address(selector: u32, scope: u32) -> Address {
    Address { selector, scope, element: ELEMENT_MAIN }
}

/// A property of plain data (a number, a device's id), `None` when the object does not have it.
fn read<T: Copy + Default>(object: ObjectId, selector: u32, scope: u32) -> Option<T> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `size` is the size of `value`, and the property is a plain value of at most that size.
    let status = unsafe { AudioObjectGetPropertyData(object, &address(selector, scope), 0, std::ptr::null(), &mut size, (&raw mut value).cast()) };
    (status == 0).then_some(value)
}

/// A property that is a `CFString` handed over to the caller.
fn read_text(object: ObjectId, selector: u32, scope: u32) -> Option<String> {
    let mut string: CfString = std::ptr::null();
    let mut size = std::mem::size_of::<CfString>() as u32;
    // SAFETY: the property is a retained `CFString`, which is released here.
    unsafe {
        let status = AudioObjectGetPropertyData(object, &address(selector, scope), 0, std::ptr::null(), &mut size, (&raw mut string).cast());
        if status != 0 {
            return None;
        }
        let text = text_of(string);
        if !string.is_null() {
            CFRelease(string);
        }
        text
    }
}

/// Which way the sound goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Output,
    Input,
}

impl Flow {
    fn scope(self) -> u32 {
        match self {
            Flow::Output => SCOPE_OUTPUT,
            Flow::Input => SCOPE_INPUT,
        }
    }

    fn default_selector(self) -> u32 {
        match self {
            Flow::Output => HARDWARE_DEFAULT_OUTPUT,
            Flow::Input => HARDWARE_DEFAULT_INPUT,
        }
    }
}

/// A sound device: its UID (which stays with the device between runs) and the name macOS shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub uid: String,
    pub name: String,
}

/// The channels `object` has in `flow`'s direction, from its `AudioBufferList`.
fn channels(object: ObjectId, flow: Flow) -> u32 {
    let address = address(DEVICE_STREAMS, flow.scope());
    let mut size = 0u32;
    // SAFETY: the list is written within the size asked for, in a buffer aligned for it; it holds a count, then that
    // many buffers of 16 bytes each from offset 8.
    unsafe {
        if AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) != 0 || size < 8 {
            return 0;
        }
        let mut list = vec![0u64; (size as usize).div_ceil(8)];
        if AudioObjectGetPropertyData(object, &address, 0, std::ptr::null(), &mut size, list.as_mut_ptr().cast()) != 0 {
            return 0;
        }
        let bytes = std::slice::from_raw_parts(list.as_ptr().cast::<u8>(), size as usize);
        let count = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        bytes[8..].chunks_exact(16).take(count).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])).sum()
    }
}

/// The devices that take or give sound in `flow`'s direction, without the ones macOS keeps to itself (hidden, and the
/// aggregates it makes for other apps' calls).
pub fn devices(flow: Flow) -> Vec<DeviceInfo> {
    let address = address(HARDWARE_DEVICES, SCOPE_GLOBAL);
    let mut size = 0u32;
    // SAFETY: the ids are written within the size asked for.
    let ids = unsafe {
        if AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &address, 0, std::ptr::null(), &mut size) != 0 {
            return Vec::new();
        }
        let mut ids = vec![0 as ObjectId; size as usize / std::mem::size_of::<ObjectId>()];
        if AudioObjectGetPropertyData(SYSTEM_OBJECT, &address, 0, std::ptr::null(), &mut size, ids.as_mut_ptr().cast()) != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / std::mem::size_of::<ObjectId>());
        ids
    };
    ids.into_iter()
        .filter(|id| channels(*id, flow) > 0 && read::<u32>(*id, DEVICE_HIDDEN, SCOPE_GLOBAL).unwrap_or(0) == 0)
        .filter_map(|id| {
            let uid = read_text(id, DEVICE_UID, SCOPE_GLOBAL)?;
            let name = read_text(id, OBJECT_NAME, SCOPE_GLOBAL).map_or_else(|| uid.clone(), |n| n.trim().to_string());
            (!uid.starts_with("CADefaultDeviceAggregate") && !name.starts_with("CADefaultDeviceAggregate")).then_some(DeviceInfo { uid, name })
        })
        .collect()
}

fn default_device(flow: Flow) -> Option<ObjectId> {
    read::<ObjectId>(SYSTEM_OBJECT, flow.default_selector(), SCOPE_GLOBAL).filter(|id| *id != 0)
}

/// The UID of the default device of `flow`, `None` when there is none.
pub fn default_uid(flow: Flow) -> Option<String> {
    read_text(default_device(flow)?, DEVICE_UID, SCOPE_GLOBAL)
}

/// The device a UID names now, `None` when it is not there.
fn device_of(uid: &str) -> Option<ObjectId> {
    let uid = Cf::new(uid)?;
    let mut device: ObjectId = 0;
    let mut size = std::mem::size_of::<ObjectId>() as u32;
    let qualifier_size = std::mem::size_of::<CfString>() as u32;
    // SAFETY: the qualifier is the `CFString` this property takes, alive for the call; the device is a plain id.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &address(HARDWARE_UID_TO_DEVICE, SCOPE_GLOBAL),
            qualifier_size,
            (&raw const uid.0).cast(),
            &mut size,
            (&raw mut device).cast(),
        )
    };
    (status == 0 && device != 0).then_some(device)
}

/// The device a UID names, as it is now, when it is there.
pub fn device(uid: &str) -> Option<DeviceInfo> {
    let id = device_of(uid)?;
    Some(DeviceInfo { uid: uid.to_string(), name: read_text(id, OBJECT_NAME, SCOPE_GLOBAL).map_or_else(|| uid.to_string(), |n| n.trim().to_string()) })
}

/// Whether the device a UID names is there and working (false once unplugged).
pub fn alive(uid: &str) -> bool {
    device_of(uid).is_some_and(|id| read::<u32>(id, DEVICE_ALIVE, SCOPE_GLOBAL).unwrap_or(1) != 0)
}

/// The sample rate the device runs at.
pub fn rate_of(uid: &str) -> Option<f64> {
    read::<f64>(device_of(uid)?, DEVICE_RATE, SCOPE_GLOBAL).filter(|r| *r > 0.0)
}

/// Whether some program has the device running now.
pub fn running(uid: &str) -> Option<bool> {
    read::<u32>(device_of(uid)?, DEVICE_RUNNING, SCOPE_GLOBAL).map(|r| r != 0)
}

/// The default output's volume (0..1) and whether it is muted, `None` when it has no volume of its own (a screen over
/// HDMI, some virtual devices).
pub fn output_volume() -> Option<(f32, bool)> {
    let device = default_device(Flow::Output)?;
    let volume = read::<f32>(device, DEVICE_VOLUME, SCOPE_OUTPUT)?;
    let muted = read::<u32>(device, DEVICE_MUTE, SCOPE_OUTPUT).unwrap_or(0) != 0;
    Some((volume.clamp(0.0, 1.0), muted))
}

/// Sets the default output's volume (0..1).
pub fn set_output_volume(volume: f32) -> Result<(), String> {
    let device = default_device(Flow::Output).ok_or("this computer has no sound output")?;
    let volume = volume.clamp(0.0, 1.0);
    // SAFETY: the property is one `f32`, alive for the call.
    let status = unsafe { AudioObjectSetPropertyData(device, &address(DEVICE_VOLUME, SCOPE_OUTPUT), 0, std::ptr::null(), 4, (&raw const volume).cast()) };
    check("setting the volume", status)
}

/// Mutes the default output or lets it sound again. A device without a mute of its own has nothing to do when unmuted.
pub fn set_output_mute(muted: bool) -> Result<(), String> {
    let device = default_device(Flow::Output).ok_or("this computer has no sound output")?;
    let mute = address(DEVICE_MUTE, SCOPE_OUTPUT);
    // SAFETY: a plain question about a property, which is one `u32` when it is there, alive for the call.
    unsafe {
        if AudioObjectHasProperty(device, &mute) == 0 {
            return if muted { Err("this sound output cannot be muted".into()) } else { Ok(()) };
        }
        let value = u32::from(muted);
        check("muting", AudioObjectSetPropertyData(device, &mute, 0, std::ptr::null(), 4, (&raw const value).cast()))
    }
}

/// Nanoseconds since a host time (`mach_absolute_time`'s ticks), as audio packets are timed.
fn host_age_ns(host_time: u64) -> u64 {
    let mut timebase = [0u32; 2];
    // SAFETY: plain reads of the clock; the timebase is two `u32`.
    let (now, _) = unsafe { (mach_absolute_time(), mach_timebase_info(&mut timebase)) };
    let [numerator, denominator] = timebase;
    (now.saturating_sub(host_time) as u128 * numerator as u128 / denominator.max(1) as u128) as u64
}

/// This thread does the sound: macOS runs it ahead of the busy ones, as Windows' multimedia class "Audio" does.
pub fn audio_priority() {
    const USER_INTERACTIVE: u32 = 0x21;
    // SAFETY: changes this thread's class only; a refusal leaves it as it was.
    let _ = unsafe { pthread_set_qos_class_self_np(USER_INTERACTIVE, 0) };
}

/// What macOS last answered about this app and the microphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicAccess {
    /// Nobody was asked yet: macOS asks when [`microphone_access`] is called.
    Undetermined,
    /// Turned off for this app, or by a policy of the computer's.
    Denied,
    Allowed,
}

/// What macOS says now, asking nobody. In a worker of the app's it is the app's answer (macOS attributes the process
/// to the one responsible for it).
pub fn mic_access() -> MicAccess {
    // SAFETY: a class method taking the `NSString` constant and returning `AVAuthorizationStatus`, an `NSInteger`.
    let status: isize = unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio] };
    match status {
        0 => MicAccess::Undetermined,
        3 => MicAccess::Allowed,
        _ => MicAccess::Denied,
    }
}

const MIC_DENIED: &str = "macOS keeps the microphone from Orbiters - N4N0: turn it on in System Settings › Privacy & Security › Microphone";
const MIC_ASKING: &str = "macOS is asking whether Orbiters - N4N0 may use the microphone: answer it, then try again";

/// Whether this app may use the microphone. The first time, macOS asks the person, once per process: that answer takes
/// a while, so this says it is being asked rather than waiting, and whoever opens the microphone again finds the answer.
/// Asking first matters: while the person has not answered, starting an input queue blocks for as long as they take, up
/// to five minutes (it then fails).
pub fn microphone_access() -> Result<(), String> {
    static ASKED: AtomicBool = AtomicBool::new(false);
    match mic_access() {
        MicAccess::Allowed => Ok(()),
        MicAccess::Denied => Err(MIC_DENIED.into()),
        MicAccess::Undetermined => {
            if !ASKED.swap(true, Ordering::Relaxed) {
                let answered = RcBlock::new(|_granted: Bool| {});
                // SAFETY: a class method taking the `NSString` constant and a block, which it copies.
                let _: () = unsafe { msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*answered] };
            }
            Err(MIC_ASKING.into())
        }
    }
}

/// A device's turn came. One [`Turn`] may serve a microphone and an output: whichever comes first wakes the thread.
#[derive(Default)]
pub struct Turn {
    came: Mutex<bool>,
    wake: Condvar,
}

impl Turn {
    fn tell(&self) {
        if let Ok(mut came) = self.came.lock() {
            *came = true;
            self.wake.notify_all();
        }
    }

    /// Waits for a turn, at most `timeout`.
    pub fn wait(&self, timeout: Duration) {
        let Ok(came) = self.came.lock() else { return };
        if let Ok((mut came, _)) = self.wake.wait_timeout_while(came, timeout, |came| !*came) {
            *came = false;
        }
    }
}

/// What a queue's callback hands back, with whom to tell. Lives in a box: the queue holds its address until disposed.
struct Back<T> {
    items: Mutex<VecDeque<T>>,
    turn: Arc<Turn>,
}

impl<T> Back<T> {
    fn new(turn: &Arc<Turn>) -> Box<Self> {
        Box::new(Self { items: Mutex::new(VecDeque::new()), turn: turn.clone() })
    }

    fn pop(&self) -> Option<T> {
        self.items.lock().ok()?.pop_front()
    }

    fn push(&self, item: T) {
        if let Ok(mut items) = self.items.lock() {
            items.push_back(item);
        }
    }
}

fn float_format(rate: u32, channels: u32) -> StreamFormat {
    let bytes = channels * 4;
    StreamFormat {
        rate: rate as f64,
        format_id: LINEAR_PCM,
        format_flags: FORMAT_FLOAT_PACKED,
        bytes_per_packet: bytes,
        frames_per_packet: 1,
        bytes_per_frame: bytes,
        channels,
        bits: 32,
        reserved: 0,
    }
}

/// Stops and frees a queue, which ends its callbacks before it returns.
fn dispose(queue: QueueRef) {
    // SAFETY: a queue this module made, disposed once.
    unsafe {
        let _ = AudioQueueStop(queue, 1);
        let _ = AudioQueueDispose(queue, 1);
    }
}

fn set_device(queue: QueueRef, uid: &str) -> Result<(), String> {
    let string = Cf::new(uid).ok_or("a device's name that cannot be read")?;
    // SAFETY: the property is a `CFString`, which the queue copies.
    let status = unsafe { AudioQueueSetProperty(queue, QUEUE_DEVICE, (&raw const string.0).cast(), std::mem::size_of::<CfString>() as u32) };
    check("choosing the device", status)
}

unsafe extern "C" fn rendered(user: *mut c_void, _queue: QueueRef, buffer: BufferRef) {
    // SAFETY: `user` is the box the queue was made with, which outlives the queue; the buffer goes back to the queue's
    // owner, the only one to touch it while the queue does not have it.
    let back = unsafe { &*user.cast::<Back<usize>>() };
    back.push(buffer as usize);
    back.turn.tell();
}

/// What a [`Render`] plays: 48 kHz stereo floats, in buffers of 10 ms. Three of them keep the wait short (clicks follow
/// the press closely) and survive a thread that is 20 ms late.
const RENDER_BUFFERS: usize = 3;
const RENDER_FRAMES: u32 = crate::device::RATE / 100;

/// An output device, open. It starts at its first feed.
pub struct Render {
    queue: QueueRef,
    back: Box<Back<usize>>,
    started: bool,
    pub uid: String,
}

impl Render {
    /// Opens the output `uid`: its turn comes on `turn` each time a buffer has played.
    pub fn open(uid: &str, turn: &Arc<Turn>) -> Result<Self, String> {
        if !alive(uid) {
            return Err(format!("the sound output {uid} is not there"));
        }
        let format = float_format(crate::device::RATE, 2);
        let back = Back::new(turn);
        let mut queue = std::ptr::null_mut();
        // SAFETY: the box outlives the queue, disposed in `drop` before the box goes; the callback and the run loop
        // (none: the queue's own thread) are as the call wants them.
        let status = unsafe {
            AudioQueueNewOutput(
                &format,
                rendered,
                std::ptr::from_ref::<Back<usize>>(&*back).cast_mut().cast(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut queue,
            )
        };
        check("opening the sound output", status)?;
        let render = Self { queue, back, started: false, uid: uid.to_string() };
        set_device(queue, uid)?;
        for _ in 0..RENDER_BUFFERS {
            let mut buffer = std::ptr::null_mut();
            // SAFETY: the queue is open and owns what it allocates.
            check("opening the sound output", unsafe { AudioQueueAllocateBuffer(queue, RENDER_FRAMES * 8, &mut buffer) })?;
            render.back.push(buffer as usize);
        }
        Ok(render)
    }

    /// Fills the buffers the device gave back: `fill` gets interleaved stereo to fill. Starts it the first time.
    pub fn feed(&mut self, fill: &mut dyn FnMut(&mut [f32])) -> Result<(), String> {
        while let Some(buffer) = self.back.pop() {
            let buffer = buffer as BufferRef;
            // SAFETY: the queue gave this buffer back and has none of it until it is enqueued again; its capacity is
            // `RENDER_FRAMES` stereo frames of floats.
            unsafe {
                fill(std::slice::from_raw_parts_mut((*buffer).data.cast::<f32>(), RENDER_FRAMES as usize * 2));
                (*buffer).size = RENDER_FRAMES * 8;
                check("playing", AudioQueueEnqueueBuffer(self.queue, buffer, 0, std::ptr::null()))?;
            }
        }
        if !self.started {
            // SAFETY: the queue is open; no time means as soon as it can.
            check("starting the sound output", unsafe { AudioQueueStart(self.queue, std::ptr::null()) })?;
            self.started = true;
        }
        Ok(())
    }
}

impl Drop for Render {
    fn drop(&mut self) {
        dispose(self.queue);
    }
}

/// When a packet of sound was recorded.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stamp {
    /// Nanoseconds between its first frame and now, `None` when the device did not say.
    pub age_ns: Option<u64>,
}

/// A buffer back from an input queue, and when its first frame was recorded (host time).
struct Recorded {
    buffer: usize,
    host_time: Option<u64>,
}

unsafe extern "C" fn recorded(user: *mut c_void, _queue: QueueRef, buffer: BufferRef, start: *const TimeStamp, _packets: u32, _descriptions: *const c_void) {
    // SAFETY: as `rendered`; the time stamp is valid during the call when it is there at all.
    let (back, start) = unsafe { (&*user.cast::<Back<Recorded>>(), start.as_ref()) };
    let host_time = start.filter(|t| t.flags & TIME_HOST_VALID != 0).map(|t| t.host_time);
    back.push(Recorded { buffer: buffer as usize, host_time });
    back.turn.tell();
}

/// A buffer's length in a [`Capture`].
const CAPTURE_BUFFER: Duration = Duration::from_millis(10);

/// A microphone, open, as mono floats at the rate asked for (the queue converts from the device's own).
pub struct Capture {
    queue: QueueRef,
    back: Box<Back<Recorded>>,
    pub uid: String,
}

impl Capture {
    /// Opens the microphone `uid` at `rate`: its turn comes on `turn` each time a buffer is full. `hold` is how much
    /// sound it keeps for a reader that is late. Fails with what to do when macOS has not let this app use it
    /// ([`microphone_access`]).
    pub fn open(uid: &str, rate: u32, hold: Duration, turn: &Arc<Turn>) -> Result<Self, String> {
        if !alive(uid) {
            return Err(format!("the microphone {uid} is not there"));
        }
        microphone_access()?;
        let format = float_format(rate, 1);
        let back = Back::new(turn);
        let mut queue = std::ptr::null_mut();
        // SAFETY: as in `Render::open`.
        let status = unsafe {
            AudioQueueNewInput(
                &format,
                recorded,
                std::ptr::from_ref::<Back<Recorded>>(&*back).cast_mut().cast(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut queue,
            )
        };
        check("opening the microphone", status)?;
        let capture = Self { queue, back, uid: uid.to_string() };
        set_device(queue, uid)?;
        let frames = (rate as u64 * CAPTURE_BUFFER.as_millis() as u64 / 1000).max(1) as u32;
        let buffers = (hold.as_millis() / CAPTURE_BUFFER.as_millis()).clamp(3, 200);
        for _ in 0..buffers {
            let mut buffer = std::ptr::null_mut();
            // SAFETY: the queue is open and owns what it allocates; it fills the buffer before giving it back.
            unsafe {
                check("opening the microphone", AudioQueueAllocateBuffer(queue, frames * 4, &mut buffer))?;
                check("opening the microphone", AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null()))?;
            }
        }
        Ok(capture)
    }

    pub fn start(&mut self) -> Result<(), String> {
        // SAFETY: the queue is open; no time means as soon as it can.
        check("starting the microphone", unsafe { AudioQueueStart(self.queue, std::ptr::null()) })
    }

    /// Hands what was recorded to `each` (samples and their time), the oldest first, and gives the buffers back. An
    /// error from `each` stops there and is returned inside; one from the device is returned outside.
    pub fn drain<E>(&mut self, mut each: impl FnMut(&[f32], Stamp) -> Result<(), E>) -> Result<Result<(), E>, String> {
        while let Some(item) = self.back.pop() {
            let buffer = item.buffer as BufferRef;
            let stamp = Stamp { age_ns: item.host_time.map(host_age_ns) };
            // SAFETY: the queue gave this buffer back and has none of it until it is enqueued again; it filled `size`
            // bytes of floats.
            let result = unsafe {
                let samples = std::slice::from_raw_parts((*buffer).data.cast::<f32>().cast_const(), (*buffer).size as usize / 4);
                let result = if samples.is_empty() { Ok(()) } else { each(samples, stamp) };
                (*buffer).size = 0;
                check("reading the microphone", AudioQueueEnqueueBuffer(self.queue, buffer, 0, std::ptr::null()))?;
                result
            };
            if result.is_err() {
                return Ok(result);
            }
        }
        Ok(Ok(()))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        dispose(self.queue);
    }
}

/// Plays `sound` on the output `uid`, whatever the default is, returning once it has played.
pub fn play_on(uid: &str, sound: &crate::synth::Sound) -> Result<(), String> {
    let turn = Arc::new(Turn::default());
    let mut render = Render::open(uid, &turn)?;
    // What the queue holds at the end plays out too.
    let end = sound.frames.len() + RENDER_BUFFERS * RENDER_FRAMES as usize;
    let deadline = Instant::now() + Duration::from_secs_f64(sound.seconds() + 2.0);
    let mut at = 0usize;
    while at < end && Instant::now() < deadline {
        render.feed(&mut |out| {
            for frame in out.chunks_exact_mut(2) {
                frame.copy_from_slice(&sound.frames.get(at).copied().unwrap_or_default());
                at += 1;
            }
        })?;
        turn.wait(Duration::from_millis(100));
    }
    Ok(())
}
