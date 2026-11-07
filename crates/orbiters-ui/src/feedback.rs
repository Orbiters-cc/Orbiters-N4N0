//! What a page shows while it waits or when something went wrong: skeleton shimmers in place of what is loading,
//! a spinner, pictures that fade in once loaded, inline banners, and the activity capsule (what runs in the background,
//! how far it got).

use crate::anim::{self, ease, Spring};
use crate::icons;
use crate::paint::{self, Gradient};
use crate::theme::{self, weight, Txt};
use crate::widgets::{self, Button, Size, Tone, Variant};
use egui::{pos2, vec2, Align2, Color32, Id, Painter, Pos2, Rect, Sense, TextureHandle, Ui};
use std::f32::consts::TAU;

/// A skeleton in `rect` (squircle corners as for a circular `radius`) that a soft light sweeps across, for content
/// still loading. The sweep is a looping decoration: it rests with the others.
pub fn shimmer(painter: &Painter, rect: Rect, radius: f32) {
    let pts = paint::squircle_points(rect, theme::squircle(radius));
    paint::fill_convex(painter, &pts, |_| theme::white(0.035));
    let t = anim::clock(painter.ctx(), 30.0);
    let k = ease::in_out(((t % 1.8) / 1.8) as f32);
    // The band crosses the rect and leaves it before coming back; long rects get a wider one.
    let band_w = (rect.width() * 0.6).clamp(80.0, 360.0);
    let x = rect.left() - band_w + k * (rect.width() + band_w * 2.0);
    let band = Rect::from_min_size(pos2(x, rect.top()), vec2(band_w, rect.height()));
    let g = Gradient::horizontal(band, &[(0.0, Color32::TRANSPARENT), (0.5, theme::white(0.045)), (1.0, Color32::TRANSPARENT)]);
    paint::fill_gradient(painter, &pts, &g);
}

/// A spinner: an arc that runs around a faint ring (a looping decoration, so it rests with the others).
pub fn spinner(painter: &Painter, center: Pos2, radius: f32, width: f32, color: Color32) {
    let t = anim::clock(painter.ctx(), 30.0) as f32;
    paint::ring(painter, center, radius, width, theme::alpha(color, 0.16));
    let sweep = TAU * (0.18 + 0.14 * (0.5 + 0.5 * (t * 2.4).sin()));
    paint::arc(painter, center, radius, t * TAU * 0.85, sweep, width, color);
}

/// How much of a picture shows (0 to 1): it fades in over its placeholder the first time it is there, and its
/// placeholder takes the rest. Key `id` by what the picture is (an avatar id), not where it is drawn.
pub fn loaded(ctx: &egui::Context, id: Id, ready: bool) -> f32 {
    // A picture there from the first sight shows at once; one that arrives later fades in.
    anim::toggle(ctx, id, ready, Spring::SOFT).clamp(0.0, 1.0)
}

/// A picture covering `rect` (cropped to fill it, squircle corners `radii` as circular radii: top-left, top-right,
/// bottom-right, bottom-left), `zoom` above 1 to crop closer (a hover), faded by `alpha`.
pub fn cover(painter: &Painter, rect: Rect, radii: [f32; 4], tex: &TextureHandle, zoom: f32, alpha: f32) {
    let size = tex.size_vec2();
    let ta = rect.width() / rect.height().max(1.0);
    let ia = size.x / size.y.max(1.0);
    let (mut w, mut h) = if ta > ia { (1.0, ia / ta) } else { (ta / ia, 1.0) };
    w /= zoom.max(0.01);
    h /= zoom.max(0.01);
    let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(w, h));
    paint::squircle_texture(painter, rect, radii.map(theme::squircle), tex.id(), uv, Color32::WHITE.gamma_multiply(alpha.clamp(0.0, 1.0)));
}

/// A filled shape with squircle corners `radii` (circular radii: top-left, top-right, bottom-right, bottom-left).
pub fn fill_corners(painter: &Painter, rect: Rect, radii: [f32; 4], color: Color32) {
    let pts = paint::squircle_points_corners(rect, radii.map(theme::squircle));
    paint::fill_convex(painter, &pts, |_| color);
}

/// An inline message across the width of `ui`: a tinted frame, the tone's icon, the text wrapped; with `dismiss` an
/// × at its end. It grows in when it first shows. Returns true when dismissed.
pub fn banner(ui: &mut Ui, id: Id, tone: Tone, text: &str, dismiss: bool) -> bool {
    let w = ui.available_width();
    let text_w = w - 16.0 - 28.0 - if dismiss { 40.0 } else { 16.0 };
    let galley = ui.painter().layout_job(Txt::new(text, 13.0).color(theme::FG).line_height(19.0).job(text_w));
    let full_h = galley.size().y.max(19.0) + 24.0;
    let k = anim::spring_from(ui.ctx(), id.with("in"), [0.0], [1.0], Spring::SOFT)[0];
    let (rect, _) = ui.allocate_exact_size(vec2(w, full_h * k.clamp(0.0, 1.0)), Sense::hover());
    let r = Rect::from_min_size(rect.min, vec2(w, full_h));
    let mut p = ui.painter().with_clip_rect(rect.expand(1.0).intersect(ui.clip_rect()));
    p.multiply_opacity(k.clamp(0.0, 1.0));
    let c = tone.color();
    widgets::frame(&p, r, 14.0, theme::alpha(c, 0.1));
    let glyph = match tone {
        Tone::Danger | Tone::Rec => icons::WARNING,
        Tone::Warn => icons::WARNING,
        Tone::Ok => icons::CHECK_CIRCLE,
        Tone::Info | Tone::Off => icons::INFO,
    };
    widgets::icon(&p, pos2(r.left() + 24.0, r.top() + 21.5), glyph, 16.0, c);
    p.galley(pos2(r.left() + 44.0, r.top() + 12.0), galley, theme::FG);
    if dismiss {
        let b = Rect::from_center_size(pos2(r.right() - 22.0, r.top() + 21.5), vec2(28.0, 28.0));
        let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(b).id_salt(id.with("dismiss")));
        bu.multiply_opacity(k.clamp(0.0, 1.0));
        return widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Dismiss")).size(28.0, 14.0).id_salt(id).show_at(&mut bu, b));
    }
    false
}

/// What the activity capsule shows.
pub struct Activity<'a> {
    pub label: &'a str,
    /// A second, quieter line (a file being copied).
    pub detail: Option<&'a str>,
    /// How far it got, when that is known.
    pub fraction: Option<f32>,
    /// A Cancel button.
    pub cancel: bool,
}

/// A capsule over the bottom of `area` saying what runs in the background (a spinner, a label, a progress bar), springing
/// up while `activity` is given and back down once it ends. Returns true when Cancel was pressed.
pub fn activity(ui: &mut Ui, id: Id, area: Rect, activity: Option<&Activity<'_>>) -> bool {
    let ctx = ui.ctx().clone();
    let k = anim::toggle(&ctx, id.with("shown"), activity.is_some(), Spring::BOUNCY);
    // What it said last, so it can leave with it.
    let kept = id.with("kept");
    let (label, detail, fraction, cancel) = match activity {
        Some(a) => {
            let v = (a.label.to_string(), a.detail.map(str::to_string), a.fraction, a.cancel);
            ctx.data_mut(|d| d.insert_temp(kept, v.clone()));
            v
        }
        None if k > 0.01 => match ctx.data(|d| d.get_temp::<(String, Option<String>, Option<f32>, bool)>(kept)) {
            Some(v) => v,
            None => return false,
        },
        None => return false,
    };
    let label_t = Txt::new(&label, 13.0).weight(weight::MEDIUM);
    let detail_t = detail.as_ref().map(|d| Txt::new(d, 11.5).color(theme::FG_3));
    let text_w = widgets::text_size(ui, &label_t).x.max(detail_t.as_ref().map(|t| widgets::text_size(ui, t).x.min(320.0)).unwrap_or(0.0));
    let bar_w = if fraction.is_some() { 140.0 } else { 0.0 };
    let cancel_b = Button::new(orbiters_i18n::tr("Cancel")).variant(Variant::Ghost).size(Size::Sm);
    let cancel_w = if cancel { cancel_b.desired_width(ui) + 8.0 } else { 0.0 };
    let w = (16.0 + 22.0 + 12.0 + text_w + if bar_w > 0.0 { 16.0 + bar_w } else { 0.0 } + 16.0 + cancel_w).min(area.width() - 32.0);
    let h = if detail.is_some() { 58.0 } else { 48.0 };
    let base = Rect::from_center_size(pos2(area.center().x, area.bottom() - 24.0 - h * 0.5), vec2(w, h));
    let kk = k.clamp(0.0, 1.2);
    let r = Rect::from_center_size(base.center() + vec2(0.0, 28.0 * (1.0 - kk)), vec2(w * (0.86 + 0.14 * kk), h));
    let mut p = ui.painter().clone();
    p.multiply_opacity(k.clamp(0.0, 1.0));
    paint::shadow(&p, r, h * 0.5, vec2(0.0, 10.0), 26.0, -6.0, theme::black(0.45));
    p.rect_filled(r, theme::radius(h * 0.5), theme::surface_strong());
    spinner(&p, pos2(r.left() + 16.0 + 11.0, r.center().y), 8.0, 2.2, theme::ACCENT);
    let x = r.left() + 16.0 + 22.0 + 12.0;
    let text_max = r.right() - 16.0 - cancel_w - if bar_w > 0.0 { bar_w + 16.0 } else { 0.0 } - x;
    match &detail_t {
        Some(d) => {
            widgets::text_truncated(&p, pos2(x, r.center().y - 9.0), Align2::LEFT_CENTER, &label_t, text_max);
            widgets::text_truncated(&p, pos2(x, r.center().y + 10.0), Align2::LEFT_CENTER, d, text_max);
        }
        None => {
            widgets::text_truncated(&p, pos2(x, r.center().y), Align2::LEFT_CENTER, &label_t, text_max);
        }
    }
    if let Some(f) = fraction {
        let bar = Rect::from_min_size(pos2(r.right() - 16.0 - cancel_w - bar_w, r.center().y - 3.0), vec2(bar_w, 6.0));
        p.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
        let f = anim::value(&ctx, id.with("fraction"), f.clamp(0.0, 1.0), Spring::SOFT).clamp(0.0, 1.0);
        if f > 0.0 {
            p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), bar.height())), theme::radius(3.0), theme::ACCENT);
        }
    }
    // Clicks on the capsule stop there.
    let _ = ui.interact(r, id.with("capsule"), Sense::click());
    if cancel && activity.is_some() {
        let b = Rect::from_min_size(pos2(r.right() - 8.0 - (cancel_w - 8.0), r.center().y - 16.0), vec2(cancel_w - 8.0, 32.0));
        return widgets::activated(&cancel_b.id_salt(id.with("cancel")).show_at(ui, b));
    }
    false
}
