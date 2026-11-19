//! Bounded CPU rendering for pages and shell parts. No platform handles or effects run here.
use super::{panels, Canvas, Facts, Item, Sheet, Zone};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::time::Instant;

type Shared = (Mutex<Mailbox>, Condvar);
/// Immutable worker pixels. The last input-side release only transfers ownership;
/// large allocations are reclaimed by their raster owner, outside the mailbox lock.
pub(super) struct PixelBuffer { bytes: Vec<u8>, owner: std::thread::ThreadId, shared: std::sync::Weak<Shared> }
pub(super) type Pixels = Arc<PixelBuffer>;
impl PixelBuffer {
    fn new(bytes: Vec<u8>, shared: std::sync::Weak<Shared>) -> Pixels { Arc::new(Self { bytes, owner: std::thread::current().id(), shared }) }
    pub fn as_slice(&self) -> &[u8] { &self.bytes }
}
impl std::ops::Deref for PixelBuffer { type Target = [u8]; fn deref(&self) -> &[u8] { &self.bytes } }
impl std::fmt::Debug for PixelBuffer { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("Pixels").field("len", &self.bytes.len()).finish() } }
impl PartialEq for PixelBuffer { fn eq(&self, other: &Self) -> bool { self.bytes == other.bytes } }
impl Drop for PixelBuffer {
    fn drop(&mut self) {
        if self.owner == std::thread::current().id() || self.bytes.is_empty() { return; }
        if let Some(shared) = self.shared.upgrade() {
            let mut state = shared.0.lock().unwrap_or_else(|e| e.into_inner());
            if !state.stop {
                state.garbage.push(std::mem::take(&mut self.bytes));
                // At most 64 cached + 64 retiring frames, finite shell zones and
                // two producer slots can release buffers without another worker turn.
                debug_assert!(state.garbage.len() <= 1024);
                drop(state); shared.1.notify_one();
            }
        }
    }
}

#[derive(Clone)]
pub(super) enum Scene {
    Native,
    Menu { menu: Arc<orbiters_n4n0_core::menu::Menu>, skin: super::Skin, retro: bool, cover: bool, pictures: HashMap<String, egui::TextureId> },
    Worlds(super::super::worlds::PaintSnapshot),
}
impl Scene {
    pub fn menu_key(menu: &orbiters_n4n0_core::menu::Menu) -> String { format!("menu {:?} {:?} {:?} {} {} {} {} {} {} {}", menu.page, menu.typing, menu.layout, menu.state_page, menu.result_page, menu.subject, menu.comment_page, menu.instance_page, menu.capture_page, menu.friend_page) }
    pub fn key(&self, sheet: Sheet) -> String {
        match self { Self::Native => format!("{sheet:?}"), Self::Menu { menu, .. } => Self::menu_key(menu), Self::Worlds(scene) => scene.key() }
    }
    pub fn menu(&self) -> Option<&orbiters_n4n0_core::menu::Menu> { if let Self::Menu { menu, .. } = self { Some(menu) } else { None } }
}
#[derive(Clone)]
pub(super) struct PartSpec {
    pub zone: Zone,
    pub token: u64,
    pub canvas_id: u64,
    pub ctx: egui::Context,
    pub px: [u32; 2],
    pub logical: egui::Vec2,
    pub pointer: Option<egui::Pos2>,
    pub pressed: Option<panels::Action>,
}
#[derive(Clone)]
pub(super) struct PartPicture {
    pub zone: Zone,
    pub token: u64,
    pub pixels: Pixels,
    pub mask: Option<Arc<Vec<u8>>>,
    pub items: Vec<Item>,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    pub facts: Facts,
    #[cfg(test)] pub finish_pixels: usize,
}
pub(super) struct PartRequest { pub spec: PartSpec, pub facts: Facts, pub time: f64, pub language: orbiters_i18n::Language }
pub(super) type CompleteSheet = (Pixels, [u32; 2], u32);
#[derive(Clone)]
pub(super) struct Request {
    pub px: [u32; 2], pub epoch: u64, pub sheet: Sheet, pub facts: Facts,
    pub hover: Option<usize>, pub pressed: Option<usize>,
    pub time: f64, pub language: orbiters_i18n::Language,
    pub scene: Scene, pub header: Option<PartSpec>, pub top: Option<u32>, pub pointer: Option<egui::Pos2>,
}
#[derive(Clone)]
pub(super) struct Frame {
    pub px: [u32; 2], pub epoch: u64, pub sheet: Sheet, pub facts: Facts, pub items: Vec<Item>, pub pixels: Pixels,
    pub phases: crate::overlay::PaintPhases, pub time: f64, pub hover: Option<usize>, pub pressed: Option<usize>,
    pub scene: Scene, pub header: Option<PartPicture>,
    pub header_spec: Option<PartSpec>, pub top: Option<u32>, pub composed: Option<CompleteSheet>, pub pointer: Option<egui::Pos2>, pub language: orbiters_i18n::Language,
}
impl Frame {
    pub fn matches(&self, epoch: u64, sheet: Option<Sheet>, gallery: &panels::Gallery) -> bool {
        self.epoch == epoch && Some(self.sheet) == sheet && self.facts.recent == *gallery
    }
    fn request(&self) -> Request { Request { px: self.px, epoch: self.epoch, sheet: self.sheet, facts: self.facts.clone(), hover: self.hover, pressed: self.pressed, time: self.time, language: self.language, scene: self.scene.clone(), header: self.header_spec.clone(), top: self.top, pointer: self.pointer } }
    pub fn bytes(&self) -> usize { self.pixels.len() + self.header.as_ref().map_or(0, |h| h.pixels.len()) + self.composed.as_ref().map_or(0, |c| c.0.len()) }
    pub fn key(&self) -> String { self.scene.key(self.sheet) }
}
#[derive(Default)]
struct Mailbox {
    garbage: Vec<Vec<u8>>,
    request: Option<Request>, frame: Option<Frame>, epoch: Option<u64>,
    parts: HashMap<Zone, PartRequest>, part_frames: HashMap<Zone, PartPicture>, order: VecDeque<Zone>,
    audit: Option<(Frame, std::sync::mpsc::Sender<Audit>)>,
    stop: bool, working: bool, was_page: bool,
    #[cfg(test)] publish_gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    #[cfg(test)] gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    /// Tests: the next page job panics.
    #[cfg(test)] panic_next: bool,
}

/// Panics of the paint thread within [`PANICS_WITHIN`] after which the overlay ends, for the service to start it again.
const PANICS_AT_MOST: usize = 3;
const PANICS_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

/// A job of the paint thread, a panic in it contained: logged in `vr-test.log`, `None` returned (OVL2-11: the thread died and every
/// VR page stayed as it was for the session, with nothing said). The caller makes its canvases anew.
fn contained<T>(what: &str, panics: &mut VecDeque<Instant>, job: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
        Ok(out) => Some(out),
        Err(payload) => {
            let why = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default();
            crate::overlay::vr_log::line(&format!("page painter: painting {what} panicked ({why}); its canvases are made anew")); // i18n: not shown (a log's line)
            let now = Instant::now();
            panics.push_back(now);
            while panics.front().is_some_and(|t| now.duration_since(*t) > PANICS_WITHIN) {
                panics.pop_front();
            }
            if panics.len() >= PANICS_AT_MOST && !cfg!(test) {
                crate::overlay::vr_log::line("page painter: three panics in ten seconds; the overlay ends and is started again"); // i18n: not shown (a log's line)
                std::process::exit(1);
            }
            None
        }
    }
}
pub(super) struct Painter { shared: Arc<(Mutex<Mailbox>, Condvar)>, pub ctx: egui::Context }
struct PartCanvas { id: u64, canvas: Canvas, mask: Option<Arc<Vec<u8>>>, pixels: Vec<u8>, published: Option<Pixels>, style: Option<orbiters_core::settings::OrbitSettings> }
struct Engine { page: Canvas, parts: HashMap<Zone, PartCanvas>, parallel: bool, shared: std::sync::Weak<Shared> }
impl Engine {
    fn part(&mut self, spec: &PartSpec, facts: &Facts, time: f64) -> PartPicture {
        let slot = self.parts.entry(spec.zone).or_insert_with(|| PartCanvas { id: 0, canvas: Canvas::new(1, 1), mask: None, pixels: Vec::new(), published: None, style: None });
        if slot.id != spec.canvas_id {
            let mut canvas = Canvas::zoomed(spec.px[0], spec.px[1], spec.px[0] as f32 / spec.logical.x).in_orbit_look();
            canvas.textures = crate::overlay::PaintTextures::of(&spec.ctx); canvas.ctx = spec.ctx.clone();
            canvas.painter.set_parallel(self.parallel);
            *slot = PartCanvas { id: spec.canvas_id, canvas, mask: spec.zone.ring().map(|ring| Arc::new(panels::cell_mask(ring, spec.px, spec.px[0] as f32 / spec.logical.x))), pixels: vec![0; spec.px[0] as usize * spec.px[1] as usize * 4], published: None, style: None };
        }
        let items = panels::items(spec.zone, spec.logical, facts);
        let hover = spec.pointer.and_then(|p| panels::hit(&items, p));
        let pressed = spec.pressed.and_then(|a| items.iter().position(|i| i.action == a));
        slot.canvas.paint(time, |p| panels::paint(p, spec.zone, spec.logical, facts, hover, pressed));
        let began = Instant::now();
        let all = [0, 0, spec.px[0], spec.px[1]];
        let changed = if slot.style != Some(facts.orbit) { all } else { slot.canvas.damage().unwrap_or([0, 0, 0, 0]) };
        let from = slot.canvas.pixels();
        for y in changed[1]..changed[3] {
            let (a, b) = (((y * spec.px[0] + changed[0]) * 4) as usize, ((y * spec.px[0] + changed[2]) * 4) as usize);
            slot.pixels[a..b].copy_from_slice(&from[a..b]);
        }
        // Also consumes this draw's preserved-picture rectangles when there is no damage.
        panels::finish_picture(&mut slot.pixels, from, spec.px, spec.px[0] as f32 / spec.logical.x, changed);
        if let Some(mask) = &slot.mask { panels::apply_mask(&mut slot.pixels, mask, spec.px, changed); }
        slot.style = Some(facts.orbit);
        if slot.published.is_none() || changed[0] != changed[2] && changed[1] != changed[3] { slot.published = Some(PixelBuffer::new(slot.pixels.clone(), self.shared.clone())); }
        let pixels = slot.published.as_ref().unwrap().clone();
        super::super::record_phase(3, began.elapsed());
        PartPicture { zone: spec.zone, token: spec.token, pixels, mask: slot.mask.clone(), items, hover, pressed, facts: facts.clone(), #[cfg(test)] finish_pixels: (changed[2] - changed[0]) as usize * (changed[3] - changed[1]) as usize }
    }

    fn page(&mut self, mut request: Request) -> Frame {
        if self.page.pixel_size() != request.px {
            let ctx = self.page.ctx().clone();
            self.page = Canvas::zoomed(request.px[0], request.px[1], request.px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32).in_orbit_look();
            self.page.textures = crate::overlay::PaintTextures::of(&ctx); self.page.ctx = ctx; self.page.painter.set_parallel(self.parallel);
        }
        let px = self.page.pixel_size();
        crate::overlay::paint_phases(true);
        let at = Instant::now();
        let items = panels::sheet_items(request.sheet, &request.facts);
        super::super::record_phase(4, at.elapsed());
        let hover = match &mut request.scene {
            Scene::Native => request.pointer.and_then(|p| panels::hit(&items, p)).or(request.hover),
            Scene::Menu { menu, .. } => request.pointer.and_then(|p| {
                let page = orbiters_n4n0_core::menu::layout::cached(orbiters_n4n0_core::menu::layout::ScreenPage::of(menu));
                orbiters_n4n0_core::menu::layout::hit_pointable(page, menu, p.x, p.y).map(|(n, _)| n)
            }).or(request.hover),
            Scene::Worlds(scene) => { scene.point(request.pointer); None }
        };
        self.page.set_restyle_off(matches!(request.scene, Scene::Menu { .. }));
        self.page.paint(request.time, |p| match &request.scene {
            Scene::Native => panels::paint_sheet(p, request.sheet, &request.facts, hover, request.pressed),
            Scene::Menu { menu, skin, retro, cover, pictures } => {
                let picture = |id: &str| pictures.get(id).copied();
                let view = super::View { menu, skin, picture: &picture, time: request.time, hover, pressed: request.pressed };
                if *retro { super::menu_retro::draw(p, egui::Pos2::ZERO, &view); }
                else { super::menu_ui::draw(p, egui::Pos2::ZERO, &view); }
                if *cover { let title = menu.world_video.as_ref().map(|v| v.title.as_deref().map_or_else(|| v.url.clone(), panels::clean_title)); super::type_hint(p, title.as_deref()); }
            }
            Scene::Worlds(scene) => scene.draw(p),
        });
        let at = Instant::now();
        let mut pixels = self.page.pixels().to_vec();
        panels::finish_picture(&mut pixels, self.page.pixels(), px, px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32, [0, 0, px[0], px[1]]);
        panels::cut_page_corners(&mut pixels, px, [0, 0, px[0], px[1]]);
        super::super::record_phase(3, at.elapsed());
        let header = request.header.as_ref().map(|spec| self.part(spec, &request.facts, request.time));
        let at = Instant::now();
        let composed = header.as_ref().zip(request.header.as_ref()).zip(request.top).map(|((header, spec), top)| (PixelBuffer::new(super::vr::compose_sheet(&header.pixels, spec.px, &pixels, px, top), self.shared.clone()), spec.px, top));
        super::super::record_phase(3, at.elapsed());
        Frame { px: request.px, epoch: request.epoch, sheet: request.sheet, facts: request.facts, items, pixels: PixelBuffer::new(pixels, self.shared.clone()), phases: crate::overlay::paint_phases(false), time: request.time, hover, pressed: request.pressed, scene: request.scene, header, header_spec: request.header, top: request.top, composed, pointer: request.pointer, language: request.language }
    }
}

#[derive(Debug)]
pub(super) struct Audit { pub pixels: usize, pub header_pixels: usize, pub parallel_pixels: usize, pub parallel_header_pixels: usize, pub items: bool, pub sheet_pixels: usize, pub parallel_sheet_pixels: usize, pub hash: u64 }
impl Audit {
    fn compare(expected: &Frame, serial: &Frame, parallel: &Frame) -> Self {
        let diff = |a: &[u8], b: &[u8]| a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(a,b)| a != b).count() + a.len().abs_diff(b.len()) / 4;
        fn header(f: &Frame) -> &[u8] { f.header.as_ref().map(|h| h.pixels.as_slice()).unwrap_or(&[]) }
        fn sheet(f: &Frame) -> &[u8] { f.composed.as_ref().map(|c| c.0.as_slice()).unwrap_or(&[]) }
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for b in serial.pixels.iter() { hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3); }
        Self { sheet_pixels: diff(sheet(expected), sheet(serial)), parallel_sheet_pixels: diff(sheet(serial), sheet(parallel)), pixels: diff(&expected.pixels, &serial.pixels), header_pixels: diff(header(expected), header(serial)), parallel_pixels: diff(&serial.pixels, &parallel.pixels), parallel_header_pixels: diff(header(serial), header(parallel)), items: expected.items == serial.items && serial.items == parallel.items && expected.header.as_ref().map(|h| &h.items) == serial.header.as_ref().map(|h| &h.items), hash }
    }
}
impl Engine {
    fn mode(&mut self, parallel: bool) {
        self.parallel = parallel; self.page.painter.set_parallel(parallel); self.page.painter.invalidate();
        for slot in self.parts.values_mut() { slot.canvas.painter.set_parallel(parallel); slot.canvas.painter.invalidate(); }
    }
}
impl Painter {
    #[cfg(test)]
    pub(super) fn hold_next(&self) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (ready, at) = std::sync::mpsc::channel(); let (go, wait) = std::sync::mpsc::channel();
        self.shared.0.lock().unwrap().gate = Some((ready, wait)); (at, go)
    }
    pub fn new(px: [u32; 2]) -> Self { Self::with_context(px, Canvas::new(1, 1).ctx().clone()) }
    pub fn with_context(px: [u32; 2], ctx: egui::Context) -> Self {
        let mut page = Canvas::zoomed(px[0], px[1], px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32).in_orbit_look();
        page.textures = crate::overlay::PaintTextures::of(&ctx); page.ctx = ctx.clone(); page.painter.set_parallel(true);
        let shared = Arc::new((Mutex::new(Mailbox { garbage: Vec::with_capacity(1024), ..Default::default() }), Condvar::new()));
        let thread = shared.clone();
        let fresh_ctx = ctx.clone();
        std::thread::Builder::new().name("orbit-page-paint".into()).spawn(move || {
            let mut engine = Engine { page, parts: HashMap::new(), parallel: true, shared: Arc::downgrade(&thread) };
            // After a contained panic: canvases made as at the start (one left mid-paint is not used again).
            let fresh = |thread: &Arc<(Mutex<Mailbox>, Condvar)>| {
                let mut page = Canvas::zoomed(px[0], px[1], px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32).in_orbit_look();
                page.textures = crate::overlay::PaintTextures::of(&fresh_ctx); page.ctx = fresh_ctx.clone(); page.painter.set_parallel(true);
                Engine { page, parts: HashMap::new(), parallel: true, shared: Arc::downgrade(thread) }
            };
            let mut panics = VecDeque::new();
            let mut cleanup = std::time::Duration::ZERO;
            let mut garbage = Vec::with_capacity(1024);
            loop {
                let (page, part, audit) = {
                    let mut state = thread.0.lock().unwrap_or_else(|e| e.into_inner());
                    while state.request.is_none() && state.parts.is_empty() && state.audit.is_none() && state.garbage.is_empty() && !state.stop { state = thread.1.wait(state).unwrap_or_else(|e| e.into_inner()); }
                    if state.stop { return; }
                    state.working = true;
                    std::mem::swap(&mut garbage, &mut state.garbage);
                    if state.request.is_some() && (!state.was_page || state.parts.is_empty()) {
                        state.was_page = true; (state.request.take(), None, None)
                    } else if !state.parts.is_empty() {
                        state.was_page = false;
                        let part = loop { if let Some(zone) = state.order.pop_front() { if let Some(part) = state.parts.remove(&zone) { break part; } } };
                        (None, Some(part), None)
                    } else { (None, None, state.audit.take()) }
                };
                let at = Instant::now(); garbage.clear(); cleanup += at.elapsed();
                #[cfg(test)] {
                    let gate = thread.0.lock().unwrap().gate.take();
                    if let Some((ready, go)) = gate { ready.send(()).unwrap(); go.recv().unwrap(); }
                }
                if let Some(request) = page {
                    let current = thread.0.lock().unwrap_or_else(|e| e.into_inner()).epoch == Some(request.epoch);
                    if current {
                        let epoch = request.epoch;
                        let style = request.facts.orbit;
                        let language = request.language;
                        #[cfg(test)]
                        let panic_now = std::mem::take(&mut thread.0.lock().unwrap().panic_next);
                        #[cfg(not(test))]
                        let panic_now = false;
                        let painted = contained("a page", &mut panics, || {
                            if panic_now {
                                panic!("test: the page job panics");
                            }
                            orbiters_i18n::in_language(language, || panels::with_render_style(style, || engine.page(request)))
                        });
                        let Some(mut frame) = painted else {
                            engine = fresh(&thread);
                            thread.0.lock().unwrap_or_else(|e| e.into_inner()).working = false;
                            continue;
                        };
                        frame.phases.finish += std::mem::take(&mut cleanup);
                        #[cfg(test)] {
                            let gate = thread.0.lock().unwrap().publish_gate.take();
                            if let Some((ready, go)) = gate { ready.send(()).unwrap(); go.recv().unwrap(); }
                        }
                        let retired = {
                            let mut state = thread.0.lock().unwrap_or_else(|e| e.into_inner());
                            if state.epoch == Some(epoch) && !state.stop { state.frame.replace(frame) } else { Some(frame) }
                        };
                        drop(retired);
                    }
                }
                if let Some(request) = part {
                    match contained("a part", &mut panics, || orbiters_i18n::in_language(request.language, || panels::with_render_style(request.facts.orbit, || engine.part(&request.spec, &request.facts, request.time)))) {
                        Some(picture) => {
                            let retired = thread.0.lock().unwrap_or_else(|e| e.into_inner()).part_frames.insert(picture.zone, picture);
                            drop(retired);
                        }
                        None => engine = fresh(&thread),
                    }
                }
                if let Some((expected, reply)) = audit {
                    let style = expected.facts.orbit;
                    let result = contained("an audit", &mut panics, || orbiters_i18n::in_language(expected.language, || panels::with_render_style(style, || {
                        engine.mode(false);
                        let serial = engine.page(expected.request());
                        engine.mode(true);
                        let parallel = engine.page(expected.request());
                        Audit::compare(&expected, &serial, &parallel)
                    })));
                    match result {
                        Some(result) => {
                            let _ = reply.send(result);
                        }
                        None => engine = fresh(&thread),
                    }
                }
                thread.0.lock().unwrap_or_else(|e| e.into_inner()).working = false;
            }
        }).expect("CPU page painter thread");
        Self { shared, ctx }
    }
    pub fn submit(&self, request: Request) {
        let retired = {
            let mut s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
            if s.epoch.is_none() { s.epoch = Some(request.epoch); }
            if s.epoch != Some(request.epoch) { return; }
            s.request.replace(request)
        };
        self.shared.1.notify_one(); drop(retired);
    }
    pub fn submit_part(&self, request: PartRequest) {
        let retired = {
            let mut s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
            if !s.parts.contains_key(&request.spec.zone) { s.order.push_back(request.spec.zone); }
            s.parts.insert(request.spec.zone, request)
        };
        self.shared.1.notify_one(); drop(retired);
    }
    pub fn cancel_page(&self, epoch: u64) {
        let retired = {
            let mut s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
            s.epoch = Some(epoch); (s.request.take(), s.frame.take())
        };
        drop(retired);
    }
    pub fn retain_parts(&self, live: &[(Zone, u64)]) {
        let retired = {
            let mut s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
            let gone: Vec<_> = s.parts.keys().filter(|z| !live.iter().any(|(zone, _)| zone == *z)).copied().collect();
            let requests: Vec<_> = gone.into_iter().filter_map(|z| s.parts.remove(&z)).collect();
            let stale: Vec<_> = s.part_frames.iter().filter(|(zone, frame)| !live.contains(&(**zone, frame.token))).map(|(z, _)| *z).collect();
            let frames: Vec<_> = stale.into_iter().filter_map(|z| s.part_frames.remove(&z)).collect();
            // Order contains only live pending keys: it cannot accumulate closed-menu history.
            let keys: Vec<_> = s.parts.keys().copied().collect(); s.order.retain(|z| keys.contains(z));
            (requests, frames)
        }; drop(retired);
    }
    /// Outside measured turns only: replay frozen inputs with the same font and texture history.
    pub fn audit(&self, frame: Frame) -> std::sync::mpsc::Receiver<Audit> {
        let (reply, rx) = std::sync::mpsc::channel();
        let retired = self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).audit.replace((frame, reply));
        self.shared.1.notify_one(); drop(retired); rx
    }
    pub fn take(&self) -> Option<Frame> { self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).frame.take() }
    pub fn take_parts(&self) -> Vec<PartPicture> { self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).part_frames.drain().map(|(_, f)| f).collect() }
    pub fn busy(&self) -> bool { let s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner()); s.working || s.audit.is_some() || s.request.is_some() || s.frame.is_some() || !s.parts.is_empty() || !s.part_frames.is_empty() || !s.garbage.is_empty() }
}
impl Drop for Painter {
    fn drop(&mut self) { self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).stop = true; self.shared.1.notify_one(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn review_last_input_pixel_release_transfers_the_allocation_to_its_owner() {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let weak = Arc::downgrade(&shared);
        let pixels = std::thread::spawn(move || PixelBuffer::new(vec![91; 1024 * 1024], weak)).join().unwrap();
        let pointer = pixels.as_ptr();
        drop(pixels);
        let mut state = shared.0.lock().unwrap();
        assert_eq!(state.garbage.len(), 1, "input must not reclaim the allocation");
        assert_eq!(state.garbage[0].as_ptr(), pointer, "ownership moves without copying pixels");
        assert_eq!(state.garbage[0].len(), 1024 * 1024);
        assert_eq!(state.garbage.pop().unwrap()[0], 91);
    }

    fn request(epoch: u64, sheet: Sheet) -> Request {
        Request { px: [512,288], epoch, sheet, facts: Facts::default(), hover: None, pressed: None, time: 1.0, language: orbiters_i18n::Language::En, scene: Scene::Native, header: None, top: None, pointer: None }
    }
    #[test]
    fn mailbox_replaces_pending_work_instead_of_queuing_scroll_frames() {
        let mut mailbox = Mailbox::default();
        for epoch in 0..6000 { mailbox.request = Some(request(epoch, Sheet::Pictures)); }
        assert_eq!(mailbox.request.take().unwrap().epoch, 5999);
        assert!(mailbox.request.is_none());
    }
    #[test]
    fn frame_carries_the_geometry_of_its_pixels() {
        let painter = Painter::new([512, 288]);
        let request = request(7, Sheet::Settings);
        let items = panels::sheet_items(request.sheet, &request.facts);
        painter.submit(request);
        let began = Instant::now();
        let frame = loop {
            if let Some(frame) = painter.take() { break frame; }
            assert!(began.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(frame.epoch, 7);
        assert!(frame.matches(7, Some(Sheet::Settings), &frame.facts.recent));
        assert!(!frame.matches(8, Some(Sheet::Settings), &frame.facts.recent), "late A after A-B-A");
        assert!(!frame.matches(7, Some(Sheet::Tracking), &frame.facts.recent), "old page");
        assert!(!frame.matches(7, Some(Sheet::Settings), &panels::Gallery::default()), "reordered gallery");
        assert_eq!(frame.items, items);
        assert_eq!(frame.pixels.len(), 512 * 288 * 4);
        assert!(frame.pixels.iter().any(|p| *p != 0));
    }
    #[test]
    fn settled_worker_frames_match_the_serial_canvas() {
        let _one = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        panels::set_style(&orbiters_core::settings::OrbitSettings::default());
        for language in orbiters_i18n::Language::ALL {
        orbiters_i18n::in_language(language, || {
        let painter = Painter::new([512, 288]);
        let mut serial = Canvas::zoomed(512, 288, 512.0 / orbiters_n4n0_core::menu::layout::WIDTH as f32).in_orbit_look();
        for sheet in [Sheet::Settings, Sheet::Tracking, Sheet::Settings] {
            for _ in 0..2 {
                let mut request = request(1, sheet);
                request.language = language;
                let facts = request.facts.clone();
                painter.submit(request);
                let began = Instant::now();
                let frame = loop {
                    if let Some(frame) = painter.take() { break frame; }
                    assert!(began.elapsed() < Duration::from_secs(10));
                    std::thread::sleep(Duration::from_millis(2));
                };
                orbiters_i18n::in_language(language, || {
                    serial.paint(1.0, |p| panels::paint_sheet(p, sheet, &facts, None, None));
                    let mut pixels = serial.pixels().to_vec();
                    panels::finish_picture(&mut pixels, serial.pixels(), [512, 288], 512.0 / orbiters_n4n0_core::menu::layout::WIDTH as f32, [0, 0, 512, 288]);
                    panels::cut_page_corners(&mut pixels, [512, 288], [0, 0, 512, 288]);
                    assert_eq!(frame.pixels.as_slice(), pixels);
                });
            }
        }
        });
        }
    }

    /// OVL2-20: a part handed back with the very pixels it showed is not shown (uploaded) again; new pixels, or a first picture, are.
    #[test]
    fn an_unchanged_part_is_not_uploaded_again() {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let a = PixelBuffer::new(vec![1; 16], Arc::downgrade(&shared));
        let b = PixelBuffer::new(vec![1; 16], Arc::downgrade(&shared));
        assert!(super::super::same_picture(true, Some(&a), &a.clone()));
        assert!(!super::super::same_picture(true, Some(&a), &b), "a new picture, even with the same bytes");
        assert!(!super::super::same_picture(false, Some(&a), &a), "a part not painted yet (woken, or new)");
        assert!(!super::super::same_picture(true, None, &a));
    }

    /// OVL2-11: a panic while a page is painted costs that page once: the next one comes, and the painter is not busy for ever.
    #[test]
    fn a_panic_in_a_page_job_is_contained() {
        let painter = Painter::new([512, 288]);
        painter.shared.0.lock().unwrap().panic_next = true;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        painter.submit(request(3, Sheet::Settings));
        let began = Instant::now();
        while painter.shared.0.lock().unwrap().request.is_some() || painter.shared.0.lock().unwrap().working {
            assert!(began.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }
        std::panic::set_hook(hook);
        assert!(painter.take().is_none(), "the page that panicked gives no frame");
        assert!(!painter.busy(), "and the painter is not left working");
        painter.submit(request(3, Sheet::Settings));
        let frame = loop {
            if let Some(frame) = painter.take() { break frame; }
            assert!(began.elapsed() < Duration::from_secs(20), "the next page still comes");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(frame.pixels.len(), 512 * 288 * 4);
    }

    #[test]
    fn a_closed_page_does_not_stay_busy_for_an_unused_result() {
        let mut panels = super::super::Panels::new(Arc::new(crate::screenshot::Still), 2.0, true).with_page_worker();
        panels.stage.host.dirty = false;
        panels.stage.painter.submit(request(0, Sheet::Settings));
        assert!(panels.takeover.is_none());
        assert!(!panels.busy());
    }

}

#[cfg(test)]
mod review_scheduler {
    use super::*;
    use std::time::Duration;
    fn request(epoch: u64) -> Request { Request { px: [512,288], epoch, sheet: Sheet::Settings, facts: Facts::default(), hover: None, pressed: None, time: 1.0, language: orbiters_i18n::Language::En, scene: Scene::Native, header: None, top: None, pointer: None } }
    #[test]
    fn review_cancellation_discards_pending_work_and_inflight_publication() {
        let painter = Painter::new([512, 288]);
        let (ready, began) = std::sync::mpsc::channel(); let (go, wait) = std::sync::mpsc::channel();
        painter.shared.0.lock().unwrap().gate = Some((ready, wait));
        painter.submit(request(1)); began.recv_timeout(Duration::from_secs(10)).unwrap();
        painter.submit(request(1)); painter.cancel_page(2);
        let pending_cleared = painter.shared.0.lock().unwrap().request.is_none();
        go.send(()).unwrap();
        let start = Instant::now();
        loop { let s = painter.shared.0.lock().unwrap(); if !s.working && s.request.is_none() { break; } drop(s); assert!(start.elapsed() < Duration::from_secs(10)); std::thread::sleep(Duration::from_millis(1)); }
        assert!(pending_cleared, "obsolete pending page survived the generation change");
        assert!(painter.take().is_none(), "obsolete page was published after cancellation");
    }
    #[test]
    fn review_finished_a_cannot_publish_after_a_b_a_or_a_language_change() {
        let painter = Painter::new([512, 288]);
        let (ready, finished) = std::sync::mpsc::channel(); let (go, wait) = std::sync::mpsc::channel();
        painter.shared.0.lock().unwrap().publish_gate = Some((ready, wait));
        painter.submit(request(1)); finished.recv_timeout(Duration::from_secs(10)).unwrap();
        painter.cancel_page(2); painter.cancel_page(3);
        let mut latest = request(3); latest.language = orbiters_i18n::Language::Ja;
        latest.facts.orbit.theme = orbiters_core::settings::MenuLook::Retro;
        latest.facts.orbit.retro_color = [170, 30, 210];
        painter.submit(latest); go.send(()).unwrap();
        let start = Instant::now();
        let frame = loop { if let Some(frame) = painter.take() { break frame; } assert!(start.elapsed() < Duration::from_secs(10)); std::thread::sleep(Duration::from_millis(1)); };
        assert_eq!(frame.epoch, 3); assert_eq!(frame.language, orbiters_i18n::Language::Ja);
        assert_eq!(frame.facts.orbit.retro_color, [170, 30, 210]);
        let audit = painter.audit(frame).recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((audit.pixels, audit.header_pixels, audit.parallel_pixels, audit.parallel_header_pixels), (0, 0, 0, 0));
        assert!(audit.items);
    }
}

#[cfg(test)]
mod review_stage_lifetime {
    use super::*;
    use super::super::zones::{self, Around};
    use std::time::Duration;
    #[test]
    fn review_opening_the_first_page_keeps_one_worker_while_input_continues() {
        let _one = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let mut panels = super::super::Panels::new(Arc::new(crate::screenshot::Still), 2.0, false).with_page_worker();
        let snap = crate::screenshot::idle_snapshot(&orbiters_core::settings::Settings::default(), &[]);
        let shared = panels.stage.painter.shared.clone();
        let (ready, began) = std::sync::mpsc::channel(); let (go, wait) = std::sync::mpsc::channel();
        shared.0.lock().unwrap().gate = Some((ready, wait));
        panels.set_open(Some(Around::QuickMenu), Zone::around(Around::QuickMenu).iter().map(|z| zones::in_units(*z)).collect(), &snap);
        panels.paint(&snap, |_,_| {}); began.recv_timeout(Duration::from_secs(10)).unwrap();
        panels.takeover_place = Some(zones::in_units(Zone::Takeover)); panels.back_place = Some(zones::in_units(Zone::SheetBack));
        panels.open_sheet(Sheet::Settings, "", &snap);
        let same = Arc::ptr_eq(&shared, &panels.stage.painter.shared);
        // A blocked paint must not prevent the remaining input/state work of a turn.
        panels.hover(None); panels.follow(&snap, 0); panels.menu_turn(&snap); panels.step_springs(0.011); panels.paint(&snap, |_,_| {});
        go.send(()).unwrap();
        assert!(same, "a second painter can race the first through shared shell contexts");
    }
}

#[cfg(test)]
mod review_header_finish {
    use super::*;
    #[test]
    fn review_an_unchanged_retro_header_does_no_pixel_finishing_and_a_new_look_does() {
        let source = super::super::Panel::new(super::super::zones::in_units(Zone::SheetBack), 2.0, 0, false);
        let spec = super::super::Panels::paint_spec(&source);
        let mut engine = Engine { page: Canvas::new(1, 1), parts: HashMap::new(), parallel: true, shared: std::sync::Weak::new() };
        let mut facts = Facts { sheet: Some(Sheet::Pictures), ..Default::default() }; facts.orbit.theme = orbiters_core::settings::MenuLook::Retro;
        let first = panels::with_render_style(facts.orbit, || engine.part(&spec, &facts, 1.0));
        assert!(first.finish_pixels > 0);
        facts.gallery_scroll = 120.0;
        let second = panels::with_render_style(facts.orbit, || engine.part(&spec, &facts, 1.0));
        assert_eq!(second.finish_pixels, 0, "the page's scrolling did not change its header");
        assert_eq!(first.pixels, second.pixels);
        facts.orbit.retro_color = [170, 30, 210];
        let third = panels::with_render_style(facts.orbit, || engine.part(&spec, &facts, 1.0));
        assert_eq!(third.finish_pixels, spec.px[0] as usize * spec.px[1] as usize);
        assert_ne!(second.pixels, third.pixels);
    }
}
