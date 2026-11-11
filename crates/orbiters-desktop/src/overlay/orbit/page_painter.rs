//! Latest-only CPU page painter. No platform handles or GPU work live here.
use super::{panels, Canvas, Facts, Item, Sheet};
use std::sync::{Arc, Mutex, Condvar};
use std::time::Instant;

pub(super) struct Request {
    pub epoch: u64,
    pub sheet: Sheet,
    pub facts: Facts,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    pub time: f64,
    pub language: orbiters_i18n::Language,
}
pub(super) struct Frame {
    pub epoch: u64,
    pub sheet: Sheet,
    pub facts: Facts,
    pub items: Vec<Item>,
    pub pixels: Vec<u8>,
    pub phases: crate::overlay::PaintPhases,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
}
impl Frame {
    pub fn matches(&self, epoch: u64, sheet: Option<Sheet>, gallery: &panels::Gallery) -> bool {
        self.epoch == epoch && Some(self.sheet) == sheet && self.facts.recent == *gallery
    }
}
#[derive(Default)]
struct Mailbox { request: Option<Request>, frame: Option<Frame>, stop: bool, working: bool }
pub(super) struct Painter {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    pub ctx: egui::Context,
}
impl Painter {
    pub fn new(px: [u32; 2]) -> Self {
        let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32).in_orbit_look();
        canvas.painter.set_parallel(true);
        let ctx = canvas.ctx().clone();
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let thread = shared.clone();
        std::thread::Builder::new().name("orbit-page-paint".into()).spawn(move || {
            loop {
                let request = {
                    let mut state = thread.0.lock().unwrap_or_else(|e| e.into_inner());
                    while state.request.is_none() && !state.stop { state = thread.1.wait(state).unwrap_or_else(|e| e.into_inner()); }
                    if state.stop { return; }
                    state.working = true;
                    state.request.take().unwrap()
                };
                let frame = orbiters_i18n::in_language(request.language, || {
                    crate::overlay::paint_phases(true);
                    let items = panels::sheet_items(request.sheet, &request.facts);
                    canvas.paint(request.time, |p| panels::paint_sheet(p, request.sheet, &request.facts, request.hover, request.pressed));
                    let start = Instant::now();
                    let mut pixels = canvas.pixels().to_vec();
                    panels::finish_picture(&mut pixels, canvas.pixels(), px, px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32, [0, 0, px[0], px[1]]);
                    super::super::record_phase(3, start.elapsed());
                    Frame { epoch: request.epoch, sheet: request.sheet, facts: request.facts, items, pixels, hover: request.hover, pressed: request.pressed, phases: crate::overlay::paint_phases(false) }
                });
                let mut state = thread.0.lock().unwrap_or_else(|e| e.into_inner());
                state.working = false;
                state.frame = Some(frame);
            }
        }).expect("CPU page painter thread");
        Self { shared, ctx }
    }
    pub fn submit(&self, request: Request) {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).request = Some(request);
        self.shared.1.notify_one();
    }
    pub fn take(&self) -> Option<Frame> { self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).frame.take() }
    pub fn busy(&self) -> bool { let s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner()); s.working || s.request.is_some() || s.frame.is_some() }
}
impl Drop for Painter {
    fn drop(&mut self) { self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).stop = true; self.shared.1.notify_one(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn request(epoch: u64, sheet: Sheet) -> Request {
        Request { epoch, sheet, facts: Facts::default(), hover: None, pressed: None, time: 1.0, language: orbiters_i18n::Language::En }
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
        panels::set_style(&orbiters_core::settings::OrbitSettings::default());
        let painter = Painter::new([512, 288]);
        let mut serial = Canvas::zoomed(512, 288, 512.0 / orbiters_n4n0_core::menu::layout::WIDTH as f32).in_orbit_look();
        for sheet in [Sheet::Settings, Sheet::Tracking, Sheet::Settings] {
            for _ in 0..2 {
                let request = request(1, sheet);
                let facts = request.facts.clone();
                painter.submit(request);
                let began = Instant::now();
                let frame = loop {
                    if let Some(frame) = painter.take() { break frame; }
                    assert!(began.elapsed() < Duration::from_secs(10));
                    std::thread::sleep(Duration::from_millis(2));
                };
                orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
                    serial.paint(1.0, |p| panels::paint_sheet(p, sheet, &facts, None, None));
                    let mut pixels = serial.pixels().to_vec();
                    panels::finish_picture(&mut pixels, serial.pixels(), [512, 288], 512.0 / orbiters_n4n0_core::menu::layout::WIDTH as f32, [0, 0, 512, 288]);
                    assert_eq!(frame.pixels, pixels);
                });
            }
        }
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
