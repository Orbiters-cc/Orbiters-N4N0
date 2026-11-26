//! What a Settings page costs for what listens to its controls, when nothing does. A process of its own: it looks at what the process read.

use orbiters_ui::widgets;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

struct Counting;
thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
}
// SAFETY: the system's allocator, counted.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        MADE.with(|m| m.set(m.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        System.dealloc(at, layout)
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

static PRESSED: AtomicU32 = AtomicU32::new(0);

/// VRR-3: in English, every row of every frame looked its label up in the French catalogue, which the first one read (half a megabyte of JSON, on the interface's thread).
/// VRR-11: every control made its name for a recorder that was not there.
#[test]
fn a_settings_page_reads_no_catalogue_and_names_no_control_for_nobody() {
    widgets::on_press(|_| {
        PRESSED.fetch_add(1, Ordering::Relaxed);
    });
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let mut on = [false; 40];
    let mut frame = |n: u32, rows: &mut dyn FnMut(&mut egui::Ui)| {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 4000.0))), time: Some(n as f64 / 60.0), ..Default::default() };
        let mut full = ctx.run_ui(input, |ui| rows(ui));
        full.textures_delta.clear();
    };
    let mut page = |ui: &mut egui::Ui| {
        for (i, on) in on.iter_mut().enumerate() {
            let label = format!("Row {i}");
            widgets::field(ui, &label, None, 60.0, |ui| {
                widgets::toggle(ui, ("toggle", i), on);
            });
        }
    };
    for n in 0..120 {
        frame(n, &mut page);
    }
    assert!(!orbiters_i18n::is_read(orbiters_i18n::Language::Fr), "an English page read the French catalogue");
    assert_eq!(PRESSED.load(Ordering::Relaxed), 0);
    // What naming forty switches for a recorder costs, against a frame with none.
    let cost = |page: &mut dyn FnMut(&mut egui::Ui), frame: &mut dyn FnMut(u32, &mut dyn FnMut(&mut egui::Ui)), n: u32| {
        let before = MADE.with(Cell::get);
        frame(n, page);
        MADE.with(Cell::get) - before
    };
    let quiet = cost(&mut page, &mut frame, 121);
    let mut recorded = |ui: &mut egui::Ui| {
        let (_, marks) = widgets::record_controls(|| page(ui));
        assert_eq!(marks.len(), 40);
        assert_eq!(marks[3].key, "(\"toggle\", 3)");
    };
    let heard = cost(&mut recorded, &mut frame, 122);
    println!("allocations of a frame of forty rows: {quiet} with nothing recording, {heard} recorded");
    assert!(quiet + 80 <= heard, "{quiet} {heard}: a name is made for each control, recorded or not");
}

/// VRR-15: after the first dialog that landed on its button, every button of every frame copied the list of landings to look itself up in it, for the rest of the session.
#[test]
fn a_button_asks_about_landings_without_copying_them() {
    let ctx = egui::Context::default();
    let place = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(80.0, 30.0));
    let mut made = 0;
    for n in 0..20 {
        let mut full = ctx.run_ui(egui::RawInput { time: Some(n as f64 / 60.0), ..Default::default() }, |ui| {
            if n == 2 {
                orbiters_ui::morph::land(ui.ctx(), place, 0.9);
            }
            if n == 3 {
                assert_eq!(orbiters_ui::morph::bounce(ui.ctx(), place), 0.9);
            }
            let before = MADE.with(Cell::get);
            for k in 0..100 {
                std::hint::black_box(orbiters_ui::morph::bounce(ui.ctx(), place.translate(egui::vec2(0.0, 40.0 * k as f32 + 40.0))));
            }
            if n >= 2 {
                made += MADE.with(Cell::get) - before;
            }
        });
        full.textures_delta.clear();
    }
    println!("allocations of 100 buttons over 18 frames after a landing: {made}");
    assert_eq!(made, 0);
}
