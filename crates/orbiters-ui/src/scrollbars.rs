//! Scroll bars that show only while something scrolls (Request16 3, Enzo: "Make the scrollbar disappear when we're not actively scrolling").
//!
//! Every list of the app is an [`egui::ScrollArea`] with the floating bars of [`crate::theme`]: they take no room (nothing moves when one shows or goes) and
//! egui draws each one's handle at one of three opacities from the style, chosen per area: `interact` while the pointer is on that bar's track or drags it,
//! `active` while the pointer is over the area, `dormant` otherwise. At rest all of them are 0 but `interact`, so a bar is hidden and still found where it
//! is. Once a frame, before anything is drawn ([`install`], a hook of the context), [`follow`] looks at what scrolls: the wheel (or a touchpad, or the wheel's
//! button scrolling, [`crate::autoscroll`], which turns into wheel steps), and a bar's track pointed at or dragged. From the last of these the `active` opacity
//! is full for [`HOLD`] and then fades out over [`FADE`]: the bar of the area being scrolled shows and goes away about a second after the scrolling stops.
//! A frame is asked for only while it fades (and one at the hold's end); at rest nothing here asks for any.
//!
//! egui 0.37's areas take no key to scroll (no Page Down, no arrows), so the keyboard has nothing to show here.

use egui::{Context, Id, Rect, Sense, Vec2};
use std::time::Duration;

/// How long a bar stays fully shown after the last scrolling, in seconds.
pub const HOLD: f64 = 0.7;
/// How long it then takes to fade out, in seconds.
pub const FADE: f64 = 0.35;
/// The handle's opacity while it shows, over the area under the pointer (as it was always drawn before).
pub const SHOWN: f32 = 0.9;

/// The `active` opacity `since` seconds after the last scrolling (`None`: none yet).
pub fn opacity(since: Option<f64>) -> f32 {
    match since {
        None => 0.0,
        Some(t) if t < HOLD => SHOWN,
        Some(t) if t < HOLD + FADE => {
            let k = ((t - HOLD) / FADE) as f32;
            // Smooth at both ends.
            SHOWN * (1.0 - k * k * (3.0 - 2.0 * k))
        }
        Some(_) => 0.0,
    }
}

/// When the next frame is needed for the bars, `since` seconds after the last scrolling: every frame while fading, the hold's end while held, never at rest.
pub fn next_frame(since: Option<f64>) -> Option<Duration> {
    match since {
        Some(t) if t < HOLD => Some(Duration::from_secs_f64(HOLD - t)),
        Some(t) if t < HOLD + FADE => Some(Duration::ZERO),
        _ => None,
    }
}

/// Whether a widget of this rect and sense is one of egui's scroll bars: a strip as wide as the style's bars, along a scroll area's edge, that takes presses and
/// drags (`ScrollArea` senses the bar's whole track so).
pub fn is_bar(rect: Rect, sense: Sense, bar_width: f32) -> bool {
    let (thin, long) = (rect.width().min(rect.height()), rect.width().max(rect.height()));
    sense.senses_drag() && sense.senses_click() && (thin - bar_width).abs() < 0.01 && long >= 2.0 * bar_width
}

fn last_id() -> Id {
    Id::unique("orbiters-scrollbars-last")
}

/// What scrolls this frame: the wheel's steps (and a touchpad's, and the wheel's button's), fingers on a touch screen, or a bar's track pointed at or dragged.
fn scrolling(ctx: &Context) -> bool {
    // Fingers on a touch screen drag an area's content (egui's `DragScroll::OnTouch`).
    let wheel = ctx.input(|i| i.smooth_scroll_delta != Vec2::ZERO || i.raw.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. })) || i.is_scrolling() || i.any_touches());
    if wheel {
        return true;
    }
    let bar_width = ctx.global_style().spacing.scroll.bar_width;
    let ids: Vec<Id> = ctx.interaction_snapshot(|s| s.hovered.iter().copied().chain(s.dragged).chain(s.drag_stopped).collect());
    ids.into_iter().any(|id| ctx.read_response(id).is_some_and(|r| is_bar(r.rect, r.sense, bar_width)))
}

/// Sets this frame's bar opacities in `ui` (the pass's root, which every page's `Ui` takes its style from) and in the context (areas, popups).
pub fn follow(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let mut last = ctx.data(|d| d.get_temp::<f64>(last_id()));
    if scrolling(&ctx) {
        last = Some(now);
        ctx.data_mut(|d| d.insert_temp(last_id(), now));
    }
    let since = last.map(|l| (now - l).max(0.0));
    let active = opacity(since);
    if ctx.global_style().spacing.scroll.active_handle_opacity != active {
        ctx.all_styles_mut(|s| s.spacing.scroll.active_handle_opacity = active);
    }
    if ui.style().spacing.scroll.active_handle_opacity != active {
        ui.style_mut().spacing.scroll.active_handle_opacity = active;
    }
    match next_frame(since) {
        Some(Duration::ZERO) => ctx.request_repaint(),
        Some(after) => ctx.request_repaint_after(after),
        None => {}
    }
}

/// Follows the scrolling before every pass of `ctx` ([`crate::install`] does it for every context of the app).
pub fn install(ctx: &Context) {
    ctx.on_begin_pass("orbiters-scrollbars", std::sync::Arc::new(follow));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Event, RawInput};

    #[test]
    fn hidden_at_rest_shown_while_scrolling_then_faded_after_a_second() {
        assert_eq!(opacity(None), 0.0);
        assert_eq!(opacity(Some(0.0)), SHOWN);
        assert_eq!(opacity(Some(HOLD - 0.01)), SHOWN);
        let mid = opacity(Some(HOLD + FADE / 2.0));
        assert!(mid > 0.0 && mid < SHOWN, "{mid}");
        assert_eq!(opacity(Some(HOLD + FADE)), 0.0);
        assert!(HOLD + FADE <= 1.1, "gone about a second after");
        // Frames: none at rest, the hold's end while held, every frame while fading.
        assert_eq!(next_frame(None), None);
        assert_eq!(next_frame(Some(HOLD + FADE + 5.0)), None);
        assert_eq!(next_frame(Some(0.2)), Some(Duration::from_secs_f64(HOLD - 0.2)));
        assert_eq!(next_frame(Some(HOLD + 0.1)), Some(Duration::ZERO));
    }

    #[test]
    fn a_bar_is_a_thin_strip_that_takes_drags() {
        let bar = Rect::from_min_size(pos2(500.0, 40.0), vec2(6.0, 400.0));
        assert!(is_bar(bar, Sense::click_and_drag(), 6.0));
        assert!(is_bar(Rect::from_min_size(pos2(0.0, 300.0), vec2(400.0, 6.0)), Sense::click_and_drag(), 6.0));
        assert!(!is_bar(bar, Sense::click(), 6.0));
        assert!(!is_bar(Rect::from_min_size(pos2(0.0, 0.0), vec2(6.0, 6.0)), Sense::click_and_drag(), 6.0));
        assert!(!is_bar(Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 400.0)), Sense::click_and_drag(), 6.0));
    }

    /// A long list in an area, drawn frame after frame as the app draws it (the style of [`crate::install`]): what egui paints for its bar's handle.
    struct Page {
        ctx: Context,
        t: f64,
        repaint: Duration,
    }

    impl Page {
        fn new() -> Self {
            let ctx = Context::default();
            crate::install(&ctx);
            Self { ctx, t: 0.0, repaint: Duration::MAX }
        }

        /// One frame `dt` after the last with these events; the bar's handle colour's alpha (0 when none is painted).
        fn frame(&mut self, dt: f64, events: Vec<Event>) -> u8 {
            self.t += dt;
            let input = RawInput { time: Some(self.t), screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))), events, ..Default::default() };
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::ScrollArea::vertical().id_salt("list").auto_shrink([false, false]).show(ui, |ui| {
                    for i in 0..80 {
                        ui.label(format!("row {i}"));
                    }
                });
            });
            out.textures_delta.clear();
            self.repaint = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
            // The handle: the one filled rect along the right edge, thinner than the bar's track.
            let mut alpha = 0;
            for s in &out.shapes {
                if let egui::Shape::Rect(r) = &s.shape {
                    if r.rect.left() > 380.0 && r.rect.width() <= 6.01 && r.rect.height() > 6.0 {
                        alpha = alpha.max(r.fill.a());
                    }
                }
            }
            alpha
        }
    }

    fn wheel(at: egui::Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(at),
            Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -120.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE, source: egui::MouseWheelSource::Wheel },
        ]
    }

    #[test]
    fn a_lists_bar_shows_while_it_scrolls_and_goes_a_second_after() {
        let mut page = Page::new();
        let over = pos2(200.0, 150.0);
        // At rest, even with the pointer over the list: no bar, and no frame asked for by it.
        page.frame(0.0, vec![Event::PointerMoved(over)]);
        for _ in 0..30 {
            assert_eq!(page.frame(1.0 / 60.0, vec![]), 0, "a bar at rest");
        }
        assert!(page.repaint > Duration::from_secs(1), "frames asked for at rest: {:?}", page.repaint);
        // A turn of the wheel: the bar shows while it scrolls and just after.
        page.frame(1.0 / 60.0, wheel(over));
        let mut shown = 0;
        for _ in 0..20 {
            shown = shown.max(page.frame(1.0 / 60.0, vec![]));
        }
        assert!(shown > 100, "the bar shows while scrolling ({shown})");
        // About a second after, gone, and no more frames for it.
        for _ in 0..80 {
            page.frame(1.0 / 60.0, vec![]);
        }
        assert_eq!(page.frame(1.0 / 60.0, vec![]), 0, "the bar is gone a second after the scrolling");
        page.frame(1.0 / 60.0, vec![]);
        assert!(page.repaint > Duration::from_secs(1), "frames still asked for after the fade: {:?}", page.repaint);
    }

    #[test]
    fn a_bar_shows_under_the_pointer_on_its_track_and_fades_once_it_leaves() {
        let mut page = Page::new();
        page.frame(0.0, vec![Event::PointerMoved(pos2(200.0, 150.0))]);
        for _ in 0..30 {
            page.frame(1.0 / 60.0, vec![]);
        }
        // On the track (the window's right edge, where the bar is): it shows, to be grabbed.
        let mut on_track = 0;
        for _ in 0..10 {
            on_track = page.frame(1.0 / 60.0, vec![Event::PointerMoved(pos2(396.0, 150.0))]);
        }
        assert!(on_track > 100, "the bar on its track ({on_track})");
        // Leaving it for the list: still there just after, gone a second later.
        assert!(page.frame(1.0 / 60.0, vec![Event::PointerMoved(pos2(200.0, 150.0))]) > 100, "it stays a moment");
        for _ in 0..80 {
            page.frame(1.0 / 60.0, vec![]);
        }
        assert_eq!(page.frame(1.0 / 60.0, vec![]), 0, "it fades once the pointer has left");
    }
}
