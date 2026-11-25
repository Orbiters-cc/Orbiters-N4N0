//! The capture's controls on a bar of their own under N4N0's menu, outside the 3D worlds panel: the capture (a press
//! opens its page), See VRChat, Recenter, how sharp, Hide. It shows while a capture shows or loads and the menu or the
//! panel shows its front, whatever page is open: the controls used to be on the capture's page only, and only once
//! it showed (not while it loaded, not on the results, not on another capture's page), so they came and went.

use super::super::super::orbit::panels::look as orbit_look;
use super::*;
use orbiters_i18n::tr;

/// The bar's picture, as wide as N4N0's menu.
pub const BAR_PX: [u32; 2] = [1024, 88];

const TOP: f32 = 12.0;
const TALL: f32 = 64.0;

/// Where each control is.
fn targets() -> [(Target, Rect); 7] {
    let at = |x0: f32, x1: f32| Rect::from_min_max(pos2(x0, TOP), pos2(x1, TOP + TALL));
    [
        (Target::Open, at(12.0, 292.0)),
        (Target::SeeThrough, at(300.0, 476.0)),
        (Target::Recenter, at(484.0, 636.0)),
        (Target::Quality(Quality::Performance), at(644.0, 732.0)),
        (Target::Quality(Quality::Balanced), at(732.0, 820.0)),
        (Target::Quality(Quality::High), at(820.0, 908.0)),
        (Target::Hide, at(916.0, 1012.0)),
    ]
}

/// What is under the laser (from the top left).
pub fn bar_hit(x: f32, y: f32) -> Option<Target> {
    targets().into_iter().find(|(_, r)| r.expand(3.0).contains(pos2(x, y))).map(|(t, _)| t)
}

/// What a press on the bar asks. Recenter waits for the capture to show (its start view is placed then anyway).
pub fn bar_press(target: Target, status: &Status) -> Vec<Action> {
    match target {
        Target::Open => vec![Action::Open],
        Target::SeeThrough => vec![Action::SeeThrough(!status.see_through)],
        Target::Recenter if status.shown => vec![Action::Recenter],
        Target::Quality(q) => vec![Action::Quality(q)],
        Target::Hide => vec![Action::Hide],
        _ => Vec::new(),
    }
}

/// Draws the bar: `name` is the capture's world.
pub fn draw_bar(p: &Painter, status: &Status, name: Option<&str>, hover: Option<Target>, pressed: Option<Target>, time: f64) {
    let whole = Rect::from_min_size(Pos2::ZERO, vec2(BAR_PX[0] as f32, BAR_PX[1] as f32)).shrink(2.0);
    p.rect_filled(whole, CornerRadius::same((whole.height() / 2.0) as u8), theme::ink_850());
    p.rect_stroke(whole, CornerRadius::same((whole.height() / 2.0) as u8), Stroke::new(1.0, theme::white(0.06)), StrokeKind::Inside);
    let state = |t: Target| (hover == Some(t), pressed == Some(t));
    let all = targets();
    for (t, rect) in &all {
        let (h, pr) = state(*t);
        match *t {
            Target::Open => {
                let r = if pr { rect.shrink(2.0) } else { *rect };
                p.rect_filled(r, CornerRadius::same(32), if h { RAISED } else { SURFACE });
                let c = pos2(r.min.x + 32.0, r.center().y);
                if status.current.is_some() && !status.shown && status.error.is_none() {
                    loading_ring(p, c, time);
                } else {
                    widgets::icon(p, c, icons::CUBE_FOCUS, 26.0, if status.error.is_some() { theme::DANGER } else { theme::accent() });
                }
                let x = r.min.x + 60.0;
                let width = r.max.x - x - 34.0;
                text_cut(p, pos2(x, r.min.y + 9.0), width, orbit_look::txt(name.unwrap_or_else(|| tr("3D world")).to_string(), 18.0).weight(weight::SEMIBOLD).color(theme::fg()));
                let line = if let Some(e) = &status.error {
                    e.clone()
                } else if status.shown && status.stage.is_none() {
                    tr("Around you · its page").to_string()
                } else if let Some(f) = status.download {
                    tr!("Downloading {percent}%", percent = format!("{:.0}", f * 100.0))
                } else {
                    status.stage.clone().unwrap_or_else(|| tr("Getting ready").into())
                };
                let tint = if status.error.is_some() { theme::DANGER } else { theme::fg_3() };
                text_cut(p, pos2(x, r.min.y + 35.0), width, orbit_look::txt(line, 14.0).color(tint));
                widgets::icon(p, pos2(r.max.x - 20.0, r.center().y), icons::CARET_RIGHT, 18.0, theme::fg_3());
                if let Some(f) = status.download {
                    let track = Rect::from_min_size(pos2(x, r.max.y - 8.0), vec2(width, 3.0));
                    p.rect_filled(track, CornerRadius::same(1), theme::white(0.1));
                    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * f, 3.0)), CornerRadius::same(1), theme::accent());
                }
            }
            Target::SeeThrough => {
                let (bg, fg) = if status.see_through { (theme::accent(), theme::on_accent()) } else { (RAISED, theme::fg()) };
                bar_button(p, *rect, icons::VIRTUAL_REALITY, tr("See VRChat"), bg, fg, h, pr, 1.0);
            }
            Target::Recenter => bar_button(p, *rect, icons::CROSSHAIR, tr("Recenter"), RAISED, theme::fg(), h, pr, if status.shown { 1.0 } else { 0.45 }),
            Target::Quality(q) => {
                if q == Quality::Performance {
                    let track = Rect::from_min_max(rect.min, all[5].1.max);
                    p.rect_filled(track, CornerRadius::same(32), SURFACE);
                }
                let r = Rect::from_center_size(rect.center(), vec2(rect.width() - 6.0, rect.height() - 10.0));
                segment(p, r, q.shown_label(), status.quality == q, h, pr, 1.0);
            }
            Target::Hide => {
                let label = if status.shown {
                    tr("Hide")
                } else if status.error.is_some() {
                    tr("Close")
                } else {
                    tr("Cancel")
                };
                bar_button(p, *rect, icons::EYE_SLASH, label, theme::mix(RAISED, theme::DANGER, 0.22), theme::fg(), h, pr, 1.0);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn bar_button(p: &Painter, rect: Rect, glyph: &str, label: &str, bg: Color32, fg: Color32, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    let bg = if hover { theme::mix(bg, Color32::WHITE, 0.08) } else { bg };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fade(bg, alpha));
    let (galley, with_icon) = button_label(p, orbit_look::txt(label, 17.0).weight(weight::SEMIBOLD).color(fade(fg, alpha)), rect.width() - 46.0, rect.width() - 20.0);
    let total = if with_icon { 28.0 } else { 0.0 } + galley.size().x;
    let left = rect.center().x - total / 2.0;
    if with_icon {
        widgets::icon(p, pos2(left + 10.0, rect.center().y), glyph, 21.0, fade(fg, alpha));
    }
    p.galley(pos2(left + if with_icon { 28.0 } else { 0.0 }, rect.center().y - galley.size().y / 2.0), galley, fade(fg, alpha));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_holds_every_control_and_recenter_waits_for_the_capture() {
        let loading = Status { current: Some(1), download: Some(0.3), ..Default::default() };
        let shown = Status { current: Some(1), shown: true, ..Default::default() };
        // Every control is there whether the capture loads or shows.
        let at = |t: Target| targets().into_iter().find(|(x, _)| *x == t).map(|(_, r)| r.center()).unwrap();
        for t in [Target::Open, Target::SeeThrough, Target::Recenter, Target::Quality(Quality::High), Target::Hide] {
            assert_eq!(bar_hit(at(t).x, at(t).y), Some(t));
        }
        assert_eq!(bar_press(Target::SeeThrough, &loading), [Action::SeeThrough(true)]);
        assert_eq!(bar_press(Target::SeeThrough, &Status { see_through: true, ..shown.clone() }), [Action::SeeThrough(false)]);
        assert!(bar_press(Target::Recenter, &loading).is_empty());
        assert_eq!(bar_press(Target::Recenter, &shown), [Action::Recenter]);
        assert_eq!(bar_press(Target::Quality(Quality::Performance), &loading), [Action::Quality(Quality::Performance)]);
        assert_eq!(bar_press(Target::Hide, &loading), [Action::Hide]);
        assert_eq!(bar_press(Target::Open, &shown), [Action::Open]);
        // The controls do not overlap and fit the bar.
        let all = targets();
        for (i, (_, a)) in all.iter().enumerate() {
            assert!(a.max.x <= BAR_PX[0] as f32 && a.max.y <= BAR_PX[1] as f32);
            for (_, b) in &all[i + 1..] {
                assert!(a.intersect(*b).area() <= 0.0, "{a:?} {b:?}");
            }
        }
    }
}
