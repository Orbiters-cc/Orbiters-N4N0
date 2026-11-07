//! Where text no longer fits: when turned on (`--screenshot --text-report`), every frame's shapes are read for text past the box it lies in, past
//! its clip, cut with an ellipsis, or lying over other text. French runs about 30% longer than English and these menus are tight: a render in each language
//! shows what broke, and comparing with English's report (`scripts/i18n/overflow-diff.py`) leaves what the translation did.

use egui::epaint::{ClippedShape, TextShape};
use egui::{Rect, Shape};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ON: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<BTreeSet<Finding>> = Mutex::new(BTreeSet::new());
static REPORT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// How far past its box text may lie before it counts, in points.
const SLACK: f32 = 1.5;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    /// `box`, `clip`, `cut` or `over`.
    pub kind: &'static str,
    /// How far, in whole points.
    pub by: u32,
    pub text: String,
}

pub fn turn_on() {
    ON.store(true, Ordering::Relaxed);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Reads a frame's shapes, when turned on.
pub fn record(shapes: &[ClippedShape]) {
    if on() {
        let found = audit(shapes);
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).extend(found);
    }
}

/// What was read since the last scene belongs to `scene`.
pub fn end_scene(scene: &str) {
    if on() {
        let pending = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
        let mut report = REPORT.lock().unwrap_or_else(|e| e.into_inner());
        for f in pending {
            report.push(format!("{scene}\t{}\t{}\t{}", f.kind, f.by, f.text.replace(['\n', '\t'], " ")));
        }
    }
}

/// The report: scene, kind, how far, text, tab separated.
pub fn report() -> String {
    let report = REPORT.lock().unwrap_or_else(|e| e.into_inner());
    report.iter().map(|l| format!("{l}\n")).collect()
}

fn flatten<'a>(shape: &'a Shape, clip: Rect, boxes: &mut Vec<Rect>, texts: &mut Vec<(&'a TextShape, Rect)>) {
    match shape {
        Shape::Vec(list) => {
            for s in list {
                flatten(s, clip, boxes, texts);
            }
        }
        Shape::Rect(r) if r.rect.width() >= 8.0 && r.rect.height() >= 8.0 => boxes.push(r.rect),
        Shape::Path(p) if p.closed && p.points.len() >= 3 => {
            let bounds = Rect::from_points(&p.points);
            if bounds.width() >= 8.0 && bounds.height() >= 8.0 {
                boxes.push(bounds);
            }
        }
        Shape::Text(t) => texts.push((t, clip)),
        _ => {}
    }
}

fn is_icon(t: &TextShape) -> bool {
    t.galley.job.text.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c))
}

/// What is wrong with one frame's text.
pub fn audit(shapes: &[ClippedShape]) -> Vec<Finding> {
    let mut boxes = Vec::new();
    let mut texts = Vec::new();
    let mut screen: Option<Rect> = None;
    for clipped in shapes {
        screen = Some(screen.map_or(clipped.clip_rect, |s| s.union(clipped.clip_rect)));
        flatten(&clipped.shape, clipped.clip_rect, &mut boxes, &mut texts);
    }
    let screen = screen.unwrap_or(Rect::NOTHING);
    let mut out = Vec::new();
    let placed: Vec<(&TextShape, Rect, Rect)> = texts
        .iter()
        // One letter at a time is text along a curve, which touches itself by design.
        .filter(|(t, _)| !is_icon(t) && t.galley.job.text.trim().chars().count() >= 2)
        .map(|(t, clip)| (*t, t.galley.rect.translate(t.pos.to_vec2()), *clip))
        .collect();
    for (t, rect, clip) in &placed {
        let text = t.galley.job.text.clone();
        // A line box is a little taller than its letters.
        let ink = rect.shrink2(egui::vec2(0.0, (rect.height() * 0.12).min(3.0)));
        if t.galley.elided || text.trim_end().ends_with('\u{2026}') {
            out.push(Finding { kind: "cut", by: 0, text: text.clone() });
        }
        // The smallest box round where it starts, its middle or where it ends (it may be aligned to any), which is not the whole frame: what it is in.
        let anchors = [ink.center(), ink.left_center() + egui::vec2(2.0, 0.0), ink.right_center() - egui::vec2(2.0, 0.0)];
        let home = boxes
            .iter()
            .filter(|b| anchors.iter().any(|a| b.contains(*a)) && b.area() < screen.area() * 0.95 && b.height() >= ink.height() * 0.6)
            .min_by(|a, b| a.area().total_cmp(&b.area()));
        if let Some(home) = home {
            let by = past(ink, *home);
            if by > SLACK {
                out.push(Finding { kind: "box", by: by as u32, text: text.clone() });
            }
        }
        let by = past(ink, *clip);
        if by > SLACK {
            out.push(Finding { kind: "clip", by: by as u32, text: text.clone() });
        }
    }
    for (i, (a, ra, _)) in placed.iter().enumerate() {
        for (b, rb, _) in &placed[i + 1..] {
            let (ra, rb) = (ra.shrink2(egui::vec2(1.0, ra.height() * 0.2)), rb.shrink2(egui::vec2(1.0, rb.height() * 0.2)));
            let both = ra.intersect(rb);
            if both.is_positive() && both.area() >= 6.0 && a.galley.job.text != b.galley.job.text {
                out.push(Finding { kind: "over", by: both.width().min(both.height()) as u32, text: format!("{} | {}", a.galley.job.text, b.galley.job.text) });
            }
        }
    }
    out
}

/// How far `inner` lies past `outer`, on its worst side.
fn past(inner: Rect, outer: Rect) -> f32 {
    (outer.left() - inner.left()).max(inner.right() - outer.right()).max(outer.top() - inner.top()).max(inner.bottom() - outer.bottom()).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Color32, FontId};

    fn frame(draw: impl Fn(&egui::Painter)) -> Vec<Finding> {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), ..Default::default() };
        let mut full = ctx.run_ui(raw.clone(), |ui| draw(ui.painter()));
        full.textures_delta.clear();
        let mut full = ctx.run_ui(raw, |ui| draw(ui.painter()));
        full.textures_delta.clear();
        audit(&full.shapes)
    }

    fn kinds(found: &[Finding]) -> Vec<&'static str> {
        found.iter().map(|f| f.kind).collect()
    }

    #[test]
    fn text_in_its_box_is_fine_and_past_it_is_found() {
        let font = FontId::proportional(16.0);
        let fits = frame(|p| {
            let b = Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 40.0));
            p.rect_filled(b, 4.0, Color32::DARK_GRAY);
            p.text(b.center(), egui::Align2::CENTER_CENTER, "Fits", font.clone(), Color32::WHITE);
        });
        assert!(fits.is_empty(), "{fits:?}");
        let past = frame(|p| {
            let b = Rect::from_min_size(pos2(10.0, 10.0), vec2(90.0, 40.0));
            p.rect_filled(b, 4.0, Color32::DARK_GRAY);
            p.text(pos2(14.0, 30.0), egui::Align2::LEFT_CENTER, "Much too long to fit in here at all", font.clone(), Color32::WHITE);
        });
        assert_eq!(kinds(&past), ["box"], "{past:?}");
        assert!(past[0].by > 50);
    }

    #[test]
    fn a_cut_clip_and_overlap_are_each_found() {
        let font = FontId::proportional(16.0);
        let cut = frame(|p| {
            p.text(pos2(10.0, 30.0), egui::Align2::LEFT_CENTER, "Cut short\u{2026}", font.clone(), Color32::WHITE);
        });
        assert_eq!(kinds(&cut), ["cut"]);
        let clip = frame(|p| {
            let clipped = p.with_clip_rect(Rect::from_min_size(pos2(10.0, 10.0), vec2(40.0, 40.0)));
            clipped.text(pos2(10.0, 30.0), egui::Align2::LEFT_CENTER, "Past the clip by a lot", font.clone(), Color32::WHITE);
        });
        assert!(kinds(&clip).contains(&"clip"), "{clip:?}");
        let over = frame(|p| {
            p.text(pos2(10.0, 30.0), egui::Align2::LEFT_CENTER, "One long text here", font.clone(), Color32::WHITE);
            p.text(pos2(60.0, 30.0), egui::Align2::LEFT_CENTER, "Another", font.clone(), Color32::WHITE);
        });
        assert_eq!(kinds(&over), ["over"], "{over:?}");
    }
}
