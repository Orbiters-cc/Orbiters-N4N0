//! What one eye sees, read for VRChat's camera panel ([`super::camera_track`]), on a thread of its own and only while its camera is open.
//!
//! The compositor's picture of the eye comes first: what it covers is known from SteamVR, so nothing has to be guessed to place the tiles. Only around the last place while it is seen, ten
//! times a second; all of it four times a second while it is not. Once measured, a 16 ms producer reads the guided glyphs and our composited timing marker. A separate CPU worker fits native geometry, accepting only a fresh job when it is ready; it cannot queue old frames or stall the producer. If it cannot be read, or `camera_use_window` asks for it, VRChat's own window instead, by Windows Graphics Capture, which
//! `Tune` says the share of. Closing signals the reader without waiting or logging on the overlay turn. OrbitVr retains it until it finishes, and joins any remainder at teardown before
//! the borrowed SteamVR client dies. The picture goes no further than the reading of it.

use super::camera_track::{self, Found};
use orbiters_i18n::mark;
use orbiters_openvr::compositor::MirrorSource;
use orbiters_openvr::mirror::EyeReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Keep metric refinement and the latest glyph observation independently.
/// A slow fit must neither erase a newer pickup observation nor be starved by it.
#[derive(Default)]
struct Looks { metric: Option<Seen>, glyph: Option<Seen> }
impl Looks {
    fn publish(&mut self, look: Seen) {
        let slot=if look.found.perspective.is_some() { &mut self.metric } else { &mut self.glyph };
        if slot.as_ref().is_none_or(|old|old.at<=look.at) { *slot=Some(look); }
    }
    fn take(&mut self) -> Option<Seen> { self.metric.take().or_else(||self.glyph.take()) }
}

/// Which picture a look read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Eye,
    Window,
}

/// A look's result: what was found, on a picture of `size`, of what, and when it was read.
#[derive(Clone, Copy, Debug)]
pub struct Seen {
    pub found: Found,
    pub size: [f32; 2],
    pub source: Source,
    pub at: Instant,
    pub right: Option<RightSeen>,
    pub record: Option<[f32;2]>,
}

#[derive(Clone, Copy, Debug)]
pub struct RightSeen {
    pub found: Found,
    pub size: [f32;2],
    pub at: Instant,
    pub frame_index: u32,
}

/// What it reads from, and how.
#[derive(Clone, Copy, Debug)]
pub struct From {
    pub mirror: Option<MirrorSource>,
    pub adapter: Option<u32>,
    /// Also keeps a frame a second in the logs' folder, for measuring what a headset really shows.
    pub keep_frames: bool,
}

/// How the reading goes.
#[derive(Clone, Copy, Debug, Default)]
struct Health {
    /// Opening the window's capture fails.
    cannot_open: bool,
    /// Which picture is being read: VRChat's window has its own troubles.
    source: Option<Source>,
    /// When a frame came, and when the camera's panel was last on one.
    framed: Option<Instant>,
    found: Option<Instant>,
    /// Looks, and how many found it.
    looks: u32,
    finds: u32,
}

/// How long something has to go on before it is said: a panel out of sight for a moment says nothing.
const QUIET_FOR: Duration = Duration::from_secs(5);

fn trouble(health: &Health, began: Instant, now: Instant) -> Option<&'static str> {
    let long = |since: Option<Instant>| now.duration_since(since.unwrap_or(began)) >= QUIET_FOR;
    if health.cannot_open {
        return Some(mark("Orbiters cannot read VRChat's window, so it cannot find the camera there."));
    }
    if long(health.framed) {
        return Some(mark("VRChat's window shows no picture, so the camera cannot be found on it. It must not be minimized."));
    }
    if long(health.found) {
        return Some(mark("VRChat's camera is open, but its panel is not in view now. The tiles come when it is."));
    }
    None
}

/// Reads until dropped.
pub struct CameraWatch {
    health: Arc<Mutex<Health>>,
    began: Instant,
    seen: Arc<Mutex<Looks>>,
    stop: Arc<AtomicBool>,
    hint:Arc<Mutex<Option<(Instant,Found,Option<([f32;2],f32)>)>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CameraWatch {
    pub fn start(from: From) -> Self {
        let (seen, stop) = (Arc::new(Mutex::new(Looks::default())), Arc::new(AtomicBool::new(false)));
        let health = Arc::new(Mutex::new(Health::default()));
        let (s, st, h) = (seen.clone(), stop.clone(), health.clone());
        let hint=Arc::new(Mutex::new(None));let hints=hint.clone();
        let began = Instant::now();
        let thread = std::thread::Builder::new().name("camera panel".into()).spawn(move || { // i18n: not shown (thread name)
            run(&s, &st, &h, &hints, from);
            let health = *h.lock().unwrap_or_else(|e| e.into_inner());
            note(&format!("{:?}: {} looks, {} found the panel, over {} s", health.source, health.looks, health.finds, began.elapsed().as_secs())); // i18n: not shown (diagnostics)
        }).ok();
        Self { health, began, seen, stop, hint,thread }
    }

    /// A current pose can guide pixel search; it is never published as a look.
    pub fn guide(&self,hint:Option<Found>,record:Option<([f32;2],f32)>) { *self.hint.lock().unwrap_or_else(|e|e.into_inner())=hint.map(|h|(Instant::now(),h,record)); }

    pub fn stop(&self) { self.stop.store(true, Ordering::Relaxed); }
    pub fn finished(&self) -> bool { self.thread.as_ref().is_none_or(|t| t.is_finished()) }
    /// Only during teardown (before the borrowed SteamVR client dies), or after finished().
    pub fn finish(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }

    /// Why no tile can show, as far as reading goes: for Settings, which translates it.
    pub fn problem(&self) -> Option<&'static str> {
        let health = *self.health.lock().unwrap_or_else(|e| e.into_inner());
        trouble(&health, self.began, Instant::now())
    }

    /// The last look's result, once.
    pub fn take(&self) -> Option<Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Drop for CameraWatch {
    fn drop(&mut self) {
        // OrbitVr retains unfinished readers and calls finish before its borrowed Client can die.
        self.stop();
    }
}

/// A line in `camera-tiles.log`, which is how a run in a headset can be read afterwards.
fn note(line: &str) {
    let dir = orbiters_core::paths::logs_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(dir.join("camera-tiles.log")) {
            use std::io::Write;
            let _ = writeln!(file, "{} {line}", orbiters_core::now_utc_iso());
        }
    }
}

/// How far apart looks are, while it is seen and while it is not.
const LOOK_EVERY: Duration = Duration::from_millis(100);
const LOOK_WHOLE_EVERY: Duration = Duration::from_millis(250);
/// How many errors in a row before the eye is given up, until the camera opens again.
const EYE_TRIES: u32 = 8;

/// One look on a picture read: `area` is where it is in the whole.
fn looked(pixels: &[u8], width: usize, height: usize, stride: usize, offset: [f32; 2], whole: [usize; 2], last: Option<Found>, seen: &Mutex<Looks>, health: &Mutex<Health>, source: Source, at: Instant, right: Option<RightSeen>) -> Option<Found> {
    let found = found_in(pixels, width, height, stride, offset, whole, last);
    told(found, whole, seen, health, source, at, right, &mut None);
    found
}

/// What a picture read shows of the panel, in the whole picture's pixels, without saying it anywhere yet.
fn found_in(pixels: &[u8], width: usize, height: usize, stride: usize, offset: [f32; 2], whole: [usize; 2], last: Option<Found>) -> Option<Found> {
    let in_part = pixels.len() < whole[0] * whole[1] * 4 && stride == width * 4 && width < whole[0];
    let found = match last {
        // Around the last place in what was read, unless what was read is that already.
        Some(last) if !in_part => camera_track::find_in(pixels, width, height, stride, camera_track::around(&last, width, height), false).or_else(|| camera_track::find(pixels, width, height, stride)),
        _ => camera_track::find_in(pixels, width, height, stride, [0, 0, width, height], width <= 1400),
    };
    found.map(|f| f.shifted(offset))
}

/// Eyes wider than this are searched at half size: the detector's sizes and thresholds were measured on 1280 to 1620 px eyes, and Enzo's
/// 3168 px Virtual Desktop eyes made each fit take a quarter of a second or more in a release build (about one look a second on his dev.44).
const HALVED_FROM: u32 = 2400;

/// Every 2 x 2 pixels as one, averaged.
fn halved(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let (w, h) = (width / 2, height / 2);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let (a, b) = (&rgba[(2 * y) * width * 4..(2 * y + 1) * width * 4], &rgba[(2 * y + 1) * width * 4..(2 * y + 2) * width * 4]);
        for x in 0..w {
            for c in 0..4 {
                let sum = a[8 * x + c] as u16 + a[8 * x + 4 + c] as u16 + b[8 * x + c] as u16 + b[8 * x + 4 + c] as u16;
                out[(y * w + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

/// The panel on one eye's picture as the reader read it, at half size when the eye is large; in the whole eye's pixels.
fn found_on(picture: &orbiters_openvr::mirror::EyePicture) -> Option<Found> {
    let offset = [picture.from[0] as f32, picture.from[1] as f32];
    let (width, height, whole) = (picture.width as usize, picture.height as usize, [picture.whole[0] as usize, picture.whole[1] as usize]);
    if picture.whole[0] >= HALVED_FROM && width >= 64 && height >= 64 {
        let half = halved(&picture.rgba, width, height);
        return found_in(&half, width / 2, height / 2, width / 2 * 4, [0.0, 0.0], [whole[0] / 2, whole[1] / 2], None).map(|f| f.enlarged(2.0).shifted(offset));
    }
    found_in(&picture.rgba, width, height, width * 4, offset, whole, None)
}

/// A look's result, counted and handed to the overlay; its landmarks into `camera-tiles.log` at most twice a second (`logged`), as a replay needs them.
#[allow(clippy::too_many_arguments)]
fn told(found: Option<Found>, whole: [usize; 2], seen: &Mutex<Looks>, health: &Mutex<Health>, source: Source, at: Instant, right: Option<RightSeen>, logged: &mut Option<Instant>) {
    if let Some(found) = found {
        if logged.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
            *logged = Some(Instant::now());
            note(&format!("landmarks: ns={}, captured={:?}, source={:?}, fit={:?}", orbiters_core::clock::now_ns(), at, source, found)); // i18n: not shown (diagnostics)
            if let Some(right) = right {
                note(&format!("right landmarks: frame {}, fit={:?}", right.frame_index, right.found)); // i18n: not shown (diagnostics)
            }
        }
    }
    let mut h = health.lock().unwrap_or_else(|e| e.into_inner());
    h.framed = Some(at);
    h.looks = h.looks.saturating_add(1);
    if let Some(found) = found {
        h.found = Some(at);
        h.finds = h.finds.saturating_add(1);
        drop(h);
        seen.lock().unwrap_or_else(|e| e.into_inner()).publish(Seen { found, size: [whole[0] as f32, whole[1] as f32], source, at, right, record:None });
    }
}

/// How the CPU worker's fits went, said every five seconds: how fast looks are, and whether the right eye is ever found (Enzo's dev.44 session made
/// about one look a second and never a stereo pose, with nothing in the logs saying why).
#[derive(Default)]
struct Pace { since: Option<Instant>, jobs: u32, left_ms: f32, right_ms: f32, right_searched: u32, right_found: u32, paired: u32, left_found: u32 }
impl Pace {
    fn job(&mut self, left_ms: f32, right_ms: Option<f32>, left: bool, right: bool, paired: bool) {
        let since = *self.since.get_or_insert_with(Instant::now);
        self.jobs += 1; self.left_ms += left_ms; self.left_found += u32::from(left);
        if let Some(ms) = right_ms { self.right_searched += 1; self.right_ms += ms; self.right_found += u32::from(right); self.paired += u32::from(right && paired); }
        if since.elapsed() >= Duration::from_secs(5) {
            let n = self.jobs.max(1) as f32;
            note(&format!("fits: {} in {:.1} s, left {:.0} ms each (found {}), right searched {} at {:.0} ms each, found {}, same compositor frame {}", // i18n: not shown (diagnostics)
                self.jobs, since.elapsed().as_secs_f32(), self.left_ms / n, self.left_found, self.right_searched, self.right_ms / self.right_searched.max(1) as f32, self.right_found, self.paired));
            *self = Self::default();
        }
    }
}

/// A frame a second kept for measuring, if asked: only what is in `pixels`, and only on this computer.
fn keep(pixels: &[u8], width: usize, height: usize, stride: usize, source: Source, kept: &mut Option<Instant>) {
    if kept.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
        return;
    }
    *kept = Some(Instant::now());
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        if let Some(row) = pixels.get(y * stride..y * stride + width * 4) {
            rgba.extend_from_slice(row);
        }
    }
    if source == Source::Window {
        rgba.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    }
    rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    let dir = orbiters_core::paths::logs_dir().join("camera-frames");
    let _ = std::fs::create_dir_all(&dir);
    if let Some(image) = image::RgbaImage::from_raw(width as u32, height as u32, rgba) {
        let _ = image.save(dir.join(format!("{source:?}-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()))));
    }
}

type Guide = (Instant,Found,Option<([f32;2],f32)>);
struct Refinement {
    left: orbiters_openvr::mirror::EyePicture,
    right: orbiters_openvr::mirror::EyePicture,
    paired: Option<u32>,
    at: Instant,
    record_hint: Option<([f32;2],f32)>,
    record: Option<[f32;2]>,
}

/// Where to look for the panel in the right eye when it has not been found there yet: the rows the left eye has it in, all across (both eyes look along
/// the same rows; only the distance shifts it sideways). One pixel, never searched, while the left has not found it either: the whole right eye
/// (44 MB on Enzo's headset) is not read for a look that cannot use it.
fn right_band(left: Option<[u32; 4]>, whole: Option<[u32; 2]>) -> [u32; 4] {
    left.zip(whole).map_or([0, 0, 1, 1], |([_, y, _, h], [w, _])| [0, y, w, h])
}

/// After this many looks in a row without the right eye, it is looked for only one look in `RIGHT_RETRY_EVERY`: one eye places the tools, and a
/// right eye that never shows the panel (as on Enzo's dev.44 session) must not halve how often the left is read.
const RIGHT_MISSES: u32 = 3;
const RIGHT_RETRY_EVERY: u32 = 4;

fn run(seen:&Mutex<Looks>,stop:&AtomicBool,health:&Mutex<Health>,hints:&Mutex<Option<Guide>>,from:From) {
    let mut kept=None;
    if let Some(source)=from.mirror {
        match EyeReader::new(from.adapter) {
            Ok(reader) => {
                health.lock().unwrap_or_else(|e|e.into_inner()).source=Some(Source::Eye);
                // One persistent pair of mirror streams. Only CPU-owned pixels
                // cross threads; slow geometry never blocks the frame producer.
                let (tx,rx)=std::sync::mpsc::sync_channel::<Refinement>(0);
                let fitted=Mutex::new([None::<(Found,[u32;2])>;2]);
                let busy=AtomicBool::new(false);
                std::thread::scope(|scope| {
                    let fitted_ref=&fitted;let busy_ref=&busy;
                    scope.spawn(move || {
                        let mut kept=None;let mut debug_kept=false;let mut logged=None;
                        let (mut right_misses,mut jobs,mut pace)=(0u32,0u32,Pace::default());
                        while let Ok(job)=rx.recv() {
                            if stop.load(Ordering::Relaxed){break;}
                            jobs=jobs.wrapping_add(1);
                            let picture=job.left;
                            if from.keep_frames {keep(&picture.rgba,picture.width as usize,picture.height as usize,picture.width as usize*4,Source::Eye,&mut kept);}
                            // Sandbox-only source evidence, encoded on this CPU
                            // worker so PNG writing cannot stall glyph sampling.
                            if !debug_kept && job.record_hint.is_some() && cfg!(debug_assertions) && std::env::var_os("ORBITERS_VR_SANDBOX_ROOT").is_some() {
                                if let Some(home)=std::env::var_os("USERPROFILE") {
                                    let root=std::path::PathBuf::from(home).join(".orbiters-build/vr-sandbox");
                                    let mut rgba=picture.rgba.clone();rgba.chunks_exact_mut(4).for_each(|p|p[3]=255);
                                    if let Some(image)=image::RgbaImage::from_raw(picture.width,picture.height,rgba) {
                                        debug_kept=image.save(root.join("camera-reader.png")).is_ok();
                                        let info=serde_json::json!({"from":picture.from,"whole":picture.whole,"record_hint":job.record_hint,"record_found":job.record});
                                        let _=std::fs::write(root.join("camera-reader.json"),info.to_string());
                                    }
                                }
                            }
                            let whole=picture.whole;
                            let began=Instant::now();
                            let found=found_on(&picture);
                            let left_ms=began.elapsed().as_secs_f32()*1000.;
                            fitted_ref.lock().unwrap_or_else(|e|e.into_inner())[0]=found.map(|f|(f,whole));
                            // The right eye only where the left has it, and not on every look while it keeps missing.
                            let search_right=found.is_some() && job.right.width>1 && (right_misses<RIGHT_MISSES || jobs%RIGHT_RETRY_EVERY==0);
                            let began=Instant::now();
                            let right_found=search_right.then(||found_on(&job.right)).flatten();
                            let right_ms=search_right.then(||began.elapsed().as_secs_f32()*1000.);
                            if search_right { right_misses=if right_found.is_some() {0} else {right_misses.saturating_add(1)}; }
                            fitted_ref.lock().unwrap_or_else(|e|e.into_inner())[1]=right_found.map(|f|(f,job.right.whole));
                            let right=right_found.zip(job.paired).map(|(found,frame_index)|RightSeen{found,size:[job.right.whole[0] as f32,job.right.whole[1] as f32],at:job.at,frame_index});
                            pace.job(left_ms,right_ms,found.is_some(),right_found.is_some(),job.paired.is_some());
                            told(found,[whole[0] as usize,whole[1] as usize],seen,health,Source::Eye,job.at,right,&mut logged);
                            busy_ref.store(false,Ordering::Relaxed);
                        }
                    });
                    let(mut errors,mut last,mut said)=(0,None,false);
                    let mut requested=Instant::now().checked_sub(LOOK_WHOLE_EVERY).unwrap_or_else(Instant::now);
                    while !stop.load(Ordering::Relaxed) && errors<EYE_TRIES {
                        let began=Instant::now();
                        let guide=hints.lock().unwrap_or_else(|e|e.into_inner()).filter(|(at,_,_)|at.elapsed()<Duration::from_millis(100));
                        let fitted=*fitted.lock().unwrap_or_else(|e|e.into_inner());
                        let hint=guide.map(|(_,f,_)|f);let record_hint=guide.and_then(|(_,_,r)|r);
                        let predicted=hint.zip(last.or(fitted[0]).map(|(_,size)|size));
                        let area=predicted.or(last).or(fitted[0]).map(|(f,size)|camera_track::around(&f,size[0] as usize,size[1] as usize).map(|v|v as u32));
                        let right_area=fitted[1].map(|(f,size)|camera_track::around(&f,size[0] as usize,size[1] as usize).map(|v|v as u32))
                            .or_else(||Some(right_band(area,last.or(fitted[0]).map(|(_,size)|size))));
                        // A fit as soon as the worker is free, ten a second at most while the panel is in view: it is what places the tools.
                        let job_every=if fitted[0].is_some() || last.is_some() {LOOK_EVERY} else {LOOK_WHOLE_EVERY};
                        let job_due=requested.elapsed()>=job_every && !busy.load(Ordering::Relaxed);
                        // Between fits only the left eye is read, and only to follow glyphs near where the overlay expects them.
                        let read=if job_due {
                            reader.read_pair(source,[area,right_area]).map(|([left,right],paired)|(left,Some((right,paired))))
                        } else if hint.is_some() {
                            reader.read_part(source,orbiters_openvr::compositor::Eye::Left,area).map(|left|(left,None))
                        } else { Err(String::new()) };
                        match read {
                            Ok((left,pair)) => {
                                errors=0;
                                if !std::mem::replace(&mut said,true){note(&format!("reading persistent eye pair, {} x {}",left.whole[0],left.whole[1]));} // i18n: not shown (diagnostics)
                                let offset=[left.from[0] as f32,left.from[1] as f32];
                                let record=record_hint.and_then(|(center,radius)|camera_track::record_near(&left.rgba,left.width as usize,left.height as usize,left.width as usize*4,offset,center,radius));
                                let guided=hint.and_then(|hint|camera_track::glyphs_near(&left.rgba,left.width as usize,left.height as usize,left.width as usize*4,offset,hint));
                                if let Some(found)=guided {
                                    seen.lock().unwrap_or_else(|e|e.into_inner()).publish(Seen{found,size:[left.whole[0] as f32,left.whole[1] as f32],source:Source::Eye,at:began,right:None,record});
                                    let mut h=health.lock().unwrap_or_else(|e|e.into_inner());h.framed=Some(began);h.found=Some(began);h.looks=h.looks.saturating_add(1);h.finds=h.finds.saturating_add(1);
                                    last=Some((found,left.whole));
                                }
                                if let Some((right,paired))=pair {
                                    busy.store(true,Ordering::Relaxed);
                                    match tx.try_send(Refinement{left,right,paired,at:began,record_hint,record}) {
                                        Ok(())=>requested=Instant::now(),
                                        Err(std::sync::mpsc::TrySendError::Full(_))=>busy.store(false,Ordering::Relaxed),
                                        Err(std::sync::mpsc::TrySendError::Disconnected(_))=>break,
                                    }
                                }
                            }
                            Err(error) if error.is_empty() => {}
                            Err(error) => {errors+=1;last=None;if errors==EYE_TRIES{note(&format!("the eye cannot be read: {error}"));}} // i18n: not shown (diagnostics)
                        }
                        let every=if guide.is_some() {Duration::from_millis(16)} else if last.is_some() || fitted[0].is_some() {Duration::from_millis(10)} else {LOOK_WHOLE_EVERY};
                        std::thread::sleep(every.saturating_sub(began.elapsed()));
                    }
                    drop(tx); // CPU worker exits after its current bounded job.
                });
            }
            Err(error)=>note(&format!("no reader for the eye: {error}")), // i18n: not shown (diagnostics)
        }
    }
    if !stop.load(Ordering::Relaxed){run_on_window(seen,stop,health,from.keep_frames,&mut kept);}
}

#[cfg(not(windows))]
fn run_on_window(_: &Mutex<Looks>, _: &AtomicBool, _: &Mutex<Health>, _: bool, _: &mut Option<Instant>) {}

#[cfg(windows)]
fn run_on_window(seen: &Mutex<Looks>, stop: &AtomicBool, health: &Mutex<Health>, keep_frames: bool, kept: &mut Option<Instant>) {
    let told = |what: &dyn Fn(&mut Health)| what(&mut health.lock().unwrap_or_else(|e| e.into_inner()));
    told(&|h| h.source = Some(Source::Window));
    let mut said = false;
    while !stop.load(Ordering::Relaxed) {
        // VRChat may not be there, or its window may go: looked for again.
        match imp::Frames::open() {
            Ok(mut frames) => {
                said = false;
                told(&|h| h.cannot_open = false);
                let mut last = None;
                while !stop.load(Ordering::Relaxed) && frames.alive() {
                    let began = Instant::now();
                    if let Some((bgra, _, _, stride, left, top, client)) = frames.next() {
                        // Only what is the game's: the frame's title bar and borders are not.
                        let start = top as usize * stride + left as usize * 4;
                        if let Some(game) = bgra.get(start..) {
                            if keep_frames {
                                keep(game, client[0] as usize, client[1] as usize, stride, Source::Window, kept);
                            }
                            last = looked(game, client[0] as usize, client[1] as usize, stride, [0.0, 0.0], [client[0] as usize, client[1] as usize], last, seen, health, Source::Window, began, None);
                        }
                    }
                    std::thread::sleep(LOOK_EVERY.saturating_sub(began.elapsed()));
                }
            }
            Err(e) => {
                if !std::mem::replace(&mut said, true) {
                    note(&format!("VRChat's window cannot be read: {e}")); // i18n: not shown (a log's line)
                }
                told(&|h| h.cannot_open = true);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn slow_refinement_cannot_erase_or_starve_pickup_pixels() {
        use super::*;
        let base=Instant::now();
        let found=camera_track::Found {perspective:None,lines:None,landmarks:None,residual_px:0.,x:[0.;2],chevron:[0.;2],middle:[0.;2],right:[1.,0.],down:[0.,1.],across:1.,tall:1.};
        let look=|ms,metric| {let mut found=found;if metric {found.perspective=Some([[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]]);} Seen{found,size:[1620.,1800.],source:Source::Eye,at:base+Duration::from_millis(ms),right:None,record:None}};
        let mut looks=Looks::default();
        // The worker finishes a 600ms border fit after fresh glyph pixels.
        looks.publish(look(600,false));looks.publish(look(0,true));
        looks.publish(look(616,false));looks.publish(look(584,false));
        assert_eq!(looks.take().unwrap().at,base);
        assert_eq!(looks.take().unwrap().at,base+Duration::from_millis(616));
        assert!(looks.take().is_none());
    }
    #[test]
    fn retired_readers_are_joined_before_the_owner_finishes() {
        use super::*;
        let (release,wait)=std::sync::mpsc::channel();
        let thread=std::thread::spawn(move || { let _=wait.recv_timeout(Duration::from_secs(1)); });
        let mut watch=CameraWatch { health:Arc::new(Mutex::new(Health::default())),began:Instant::now(),seen:Arc::new(Mutex::new(Looks::default())),stop:Arc::new(AtomicBool::new(false)),hint:Arc::new(Mutex::new(None)),thread:Some(thread) };
        watch.stop();
        assert!(!watch.finished());
        release.send(()).unwrap();
        watch.finish();
        assert!(watch.finished() && watch.thread.is_none());
    }
    #[test]
    fn closing_does_not_wait_for_a_reader() {
        use super::*;
        let (release, wait) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || { let _ = wait.recv_timeout(Duration::from_secs(1)); });
        let watch = CameraWatch { health: Arc::new(Mutex::new(Health::default())), began: Instant::now(), seen: Arc::new(Mutex::new(Looks::default())), stop: Arc::new(AtomicBool::new(false)), hint:Arc::new(Mutex::new(None)),thread: Some(thread) };
        let began = Instant::now();
        drop(watch);
        assert!(began.elapsed() < Duration::from_millis(150));
        release.send(()).unwrap();
    }
    use super::*;
    use std::time::Duration;

    /// A large eye is searched at half size, and what is found comes back in the whole eye's pixels.
    #[test]
    fn a_large_eye_is_read_at_half_size_in_its_own_pixels() {
        let picture = orbiters_openvr::mirror::EyePicture { width: 4, height: 2, rgba: vec![10, 20, 30, 255, 30, 40, 50, 255, 0, 0, 0, 0, 0, 0, 0, 0, 50, 60, 70, 255, 70, 80, 90, 255, 0, 0, 0, 0, 0, 0, 0, 0], from: [0, 0], whole: [4, 2] };
        assert_eq!(halved(&picture.rgba, 4, 2), vec![40, 50, 60, 255, 0, 0, 0, 0]);
        let found = camera_track::Found { perspective: Some([[2., 0., 100.], [0., 2., 50.], [0., 0., 1.]]), lines: None, landmarks: Some((1, [([1., 1.], [10., 20.]); 10])), residual_px: 1., x: [10., 20.], chevron: [30., 40.], middle: [20., 30.], right: [1., 0.], down: [0., 1.], across: 2., tall: 2. };
        let big = found.enlarged(2.0).shifted([1000., 500.]);
        assert_eq!((big.x, big.chevron, big.across, big.residual_px), ([1020., 540.], [1060., 580.], 4., 2.));
        assert_eq!(big.landmarks.unwrap().1[0].1, [1020., 540.]);
        let at = big.at([3., 4.]);
        assert!((at[0] - (1000. + 2. * (2. * 3. + 100.))).abs() < 1e-3 && (at[1] - (500. + 2. * (2. * 4. + 50.))).abs() < 1e-3, "{at:?}");
    }

    /// Settings says why, and a panel out of sight for a moment says nothing.
    #[test]
    fn what_keeps_the_tiles_away_is_said() {
        let began = Instant::now();
        let at = |s: u64| began + Duration::from_secs(s);
        let mut health = Health::default();
        assert_eq!(trouble(&health, began, at(1)), None, "it has only just begun");
        assert!(trouble(&health, began, at(6)).is_some_and(|said| said.contains("minimized")), "no frame at all");
        health.framed = Some(at(6));
        assert!(trouble(&health, began, at(7)).is_some_and(|said| said.contains("not in view")));
        health.found = Some(at(7));
        assert_eq!(trouble(&health, began, at(8)), None, "seen");
        health.cannot_open = true;
        assert!(trouble(&health, began, at(8)).is_some_and(|said| said.contains("cannot read")));
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::{w, Interface};
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect, GetWindowRect, IsWindow};

    pub struct Frames {
        window: HWND,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        winrt: IDirect3DDevice,
        pool: Direct3D11CaptureFramePool,
        _session: GraphicsCaptureSession,
        _item: GraphicsCaptureItem,
        size: (i32, i32),
        staging: Option<(ID3D11Texture2D, u32, u32)>,
        pixels: Vec<u8>,
    }

    impl Frames {
        pub fn open() -> Result<Self, String> {
            let text = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
            // SAFETY: a plain query by name.
            let window = unsafe { FindWindowW(w!("UnityWndClass"), w!("VRChat")) }.map_err(|e| text("no VRChat window", e))?; // i18n: not shown (only printed to the log, with `camera panel:`)
            let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
            // SAFETY: the outputs are valid.
            unsafe { D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)) }.map_err(|e| text("Direct3D", e))?;
            let (device, context) = (device.ok_or("no Direct3D device")?, context.ok_or("no Direct3D context")?); // i18n: not shown (only printed to the log, with `camera panel:`)
            let dxgi: IDXGIDevice = device.cast().map_err(|e| text("DXGI", e))?;
            // SAFETY: the DXGI device is alive for the call.
            let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.and_then(|d| d.cast()).map_err(|e| text("device", e))?;
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| text("capture item", e))?; // i18n: not shown (only printed to the log, with `camera panel:`)
            // SAFETY: a window handle; one that is gone only fails.
            let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }.map_err(|e| text("capture item", e))?; // i18n: not shown (only printed to the log, with `camera panel:`)
            let size = item.Size().map_err(|e| text("size", e))?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(|e| text("frames", e))?;
            let session = pool.CreateCaptureSession(&item).map_err(|e| text("session", e))?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture().map_err(|e| text("start", e))?;
            Ok(Self { window, device, context, winrt, pool, _session: session, _item: item, size: (size.Width, size.Height), staging: None, pixels: Vec::new() })
        }

        pub fn alive(&self) -> bool {
            // SAFETY: a plain query.
            unsafe { IsWindow(Some(self.window)) }.as_bool()
        }

        /// The newest frame's pixels: them, its size, their stride, where the game's area starts in them, and how large that is.
        pub fn next(&mut self) -> Option<(&[u8], u32, u32, usize, u32, u32, [u32; 2])> {
            let mut newest = None;
            while let Ok(frame) = self.pool.TryGetNextFrame() {
                newest = Some(frame);
            }
            let frame = newest?;
            let content = frame.ContentSize().ok()?;
            if (content.Width, content.Height) != self.size {
                self.size = (content.Width, content.Height);
                let _ = self.pool.Recreate(&self.winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content);
                return None;
            }
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface().ok()?.cast().ok()?;
            // SAFETY: the surface is a Direct3D 11 texture, as the pool was made for.
            let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.ok()?;
            let (width, height) = (content.Width as u32, content.Height as u32);
            let staging = self.staging_for(width, height)?;
            // SAFETY: both textures are this device's and of one format; the box is the frame's; it is unmapped before anything else happens to it.
            let stride = unsafe {
                let region = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
                self.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&region));
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
                let pitch = mapped.RowPitch;
                self.pixels.clear();
                self.pixels.extend_from_slice(std::slice::from_raw_parts(mapped.pData as *const u8, (pitch * height) as usize));
                self.context.Unmap(&staging, 0);
                pitch as usize
            };
            let (mut outer, mut client, mut corner) = (RECT::default(), RECT::default(), POINT::default());
            // SAFETY: plain queries with valid outputs.
            unsafe {
                GetWindowRect(self.window, &mut outer).ok()?;
                GetClientRect(self.window, &mut client).ok()?;
                let _ = ClientToScreen(self.window, &mut corner);
            }
            let (left, top) = ((corner.x - outer.left).max(0) as u32, (corner.y - outer.top).max(0) as u32);
            let area = [(client.right - client.left).max(0) as u32, (client.bottom - client.top).max(0) as u32];
            let area = [area[0].min(width.saturating_sub(left)), area[1].min(height.saturating_sub(top))];
            Some((&self.pixels, width, height, stride, left, top, area))
        }

        fn staging_for(&mut self, width: u32, height: u32) -> Option<ID3D11Texture2D> {
            if let Some((t, w, h)) = &self.staging {
                if *w == width && *h == height {
                    return Some(t.clone());
                }
            }
            let description = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            // SAFETY: the description and the output are valid.
            unsafe { self.device.CreateTexture2D(&description, None, Some(&mut staging)) }.ok()?;
            let staging = staging?;
            self.staging = Some((staging.clone(), width, height));
            Some(staging)
        }
    }
}
