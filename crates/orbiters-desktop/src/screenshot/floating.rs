//! `--only floating`: the floating Orbiters panel and the watch as SteamVR gets them (overlay/floating), painted on the
//! CPU: `floating-panel*.png` the tab's interface on the floating panel (no Float button) with a laser on one of its
//! buttons and the bar under it with a laser on its handle, as they hang in the play space; `floating-bar*.png` the bar
//! alone; `floating-watch.png` the watch closed, and open under a laser; `floating-laser.png` the beam and its dot.
//! `settings-watch` (a window scenario): the watch's switches in Settings.

use super::{idle_snapshot, n4n0_snapshot, over_ground, with_account, Page, Scenario, StandInBody, Still};
use crate::backend::Ctl;
use crate::overlay::floating::{laser, panel, watch};
use crate::overlay::ui::{OverlayUi, Page as TabPage, Surface, SIZE};
use crate::overlay::Canvas;
use crate::state::Snapshot;
use egui::{pos2, Event, Pos2, RawInput, Rect, ViewportId, ViewportInfo};
use orbiters_core::settings::{FloatingSettings, Settings};
use orbiters_ui::offscreen::save_png;
use std::path::Path;

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    // Settings' switch for closing the minimised window while playing (Interface), at three places of the page (its
    // cards' heights differ by language).
    for (name, scroll) in [("settings-close-minimized", 5800.0), ("settings-close-minimized-2", 6600.0), ("settings-close-minimized-3", 7400.0)] {
        scenarios.push(Scenario {
            name,
            page: Page::Settings,
            snapshot: scenarios[0].snapshot.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll,
            world: None,
            home: None,
            library: None,
        });
    }
    let mut snapshot = scenarios[0].snapshot.clone();
    snapshot.steamvr.running = true;
    snapshot.steamvr.overlay = true;
    snapshot.settings.floating.watch_hand = orbiters_core::settings::Hand::Right;
    scenarios.push(Scenario {
        name: "settings-watch",
        page: Page::Settings,
        snapshot,
        frames: 120,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll: 1850.0,
        world: None,
        home: None,
        library: None,
    });
}

pub(super) fn shots(out: &Path, settings: &Settings) -> anyhow::Result<()> {
    let sessions = crate::controller::list_sessions(&settings.sessions_dir);
    let mut idle = idle_snapshot(settings, &sessions);
    idle.steamvr.running = true;
    idle.steamvr.overlay = true;
    idle.vrchat.running = true;
    idle.osc.vrchat_found = true;
    let mut talking = n4n0_snapshot(&with_account(&idle, crate::state::AccountState::SignedIn { username: "Desktop Tester".into() }, None));
    talking.vrchat.running = true;
    // (name, snapshot, the tab's page, where the laser points on the panel).
    let shots: [(&str, &Snapshot, TabPage, Pos2); 2] =
        [("floating-panel", &idle, TabPage::Capture, pos2(250.0, 210.0)), ("floating-panel-n4n0", &talking, TabPage::N4n0, pos2(250.0, 330.0))];
    for (name, snap, page, laser_at) in shots {
        let panel_px = paint_panel(snap, page, laser_at);
        let bar_px = paint_bar(Some(panel::BarButton::Handle), None, FloatingSettings::default().width_m);
        let (w, h, image) = hung(&panel_px, &bar_px, FloatingSettings::default().width_m, Some(laser_at));
        let path = out.join(format!("{name}.png"));
        save_png(&path, &over_ground(&image, w, h))?;
        println!("{}", path.display());
    }
    // The bar alone: at rest, a laser on larger, close pressed, at the smallest width (smaller dim).
    let widths = FloatingSettings::WIDTHS;
    for (name, hover, pressed, width) in [
        ("floating-bar", None, None, 0.8),
        ("floating-bar-larger", Some(panel::BarButton::Larger), None, 0.8),
        ("floating-bar-close", Some(panel::BarButton::Close), Some(panel::BarButton::Close), 0.8),
        ("floating-bar-smallest", Some(panel::BarButton::Smaller), None, widths[0]),
    ] {
        let px = paint_bar(hover, pressed, width);
        let path = out.join(format!("{name}.png"));
        save_png(&path, &over_ground(&px, panel::BAR_PX[0], panel::BAR_PX[1]))?;
        println!("{}", path.display());
    }
    // "Talk with N4N0?" before the eyes (N4N0-11): in every language, at rest and under a laser, half its time left.
    let [w, h] = crate::wake_offer::VR_PX;
    for language in orbiters_i18n::Language::ALL {
        for (state, hover) in [("rest", false), ("laser", true)] {
            let mut canvas = Canvas::new(w, h);
            orbiters_i18n::in_language(language, || {
                for _ in 0..2 {
                    canvas.paint(0.0, |p| crate::wake_offer::paint_vr(p, hover, 0.5));
                }
            });
            let path = out.join(format!("wake-offer-vr-{}-{state}.png", language.tag()));
            save_png(&path, &over_ground(canvas.pixels(), w, h))?;
            println!("{}", path.display());
        }
    }
    // "Orbiters - N4N0 will minimize to save memory" (LIGHT-DESKTOP-8): the VR band in every language, at rest and under
    // a laser, 4 of its 8 seconds left; the desktop card at 100 % and 150 %, at rest and under the pointer.
    for language in orbiters_i18n::Language::ALL {
        for (state, hover) in [("rest", false), ("laser", true)] {
            let mut canvas = Canvas::new(w, h);
            orbiters_i18n::in_language(language, || {
                for _ in 0..2 {
                    canvas.paint(0.0, |p| crate::window_rest::paint_band(p, hover, 0.5, 4));
                }
            });
            let path = out.join(format!("window-rest-vr-{}-{state}.png", language.tag()));
            save_png(&path, &over_ground(canvas.pixels(), w, h))?;
            println!("{}", path.display());
        }
        for (scale, hover) in [(1.0f32, false), (1.5, true)] {
            let (cw, ch) = ((crate::window_rest::CARD.x * scale).round() as u32, (crate::window_rest::CARD.y * scale).round() as u32);
            let mut canvas = Canvas::new(cw, ch);
            orbiters_i18n::in_language(language, || {
                for _ in 0..2 {
                    canvas.paint(0.0, |p| crate::window_rest::paint_card(p, scale, hover, 0.62, 5));
                }
            });
            let state = if hover { "hover" } else { "rest" };
            let path = out.join(format!("window-rest-card-{}-{}x-{state}.png", language.tag(), scale));
            save_png(&path, &over_ground(canvas.pixels(), cw, ch))?;
            println!("{}", path.display());
        }
    }
    // The watch: closed, then open under a laser, side by side.
    let n = watch::PX;
    let mut both = vec![0u8; (n * 2 * n * 4) as usize];
    for (i, (hover, open)) in [(false, false), (true, true)].into_iter().enumerate() {
        let mut canvas = Canvas::new(n, n);
        for _ in 0..2 {
            canvas.paint(0.0, |p| watch::paint(p, hover, open));
        }
        blit(&mut both, n * 2, canvas.pixels(), n, n, i as u32 * n, 0);
    }
    let path = out.join("floating-watch.png");
    save_png(&path, &over_ground(&both, n * 2, n))?;
    println!("{}", path.display());
    // The laser: its beam (shown 64 px wide here, its length as painted) and its dot.
    let mut beam = Canvas::new(8, 256);
    let beam_px = beam.paint(0.0, laser::paint_beam).to_vec();
    let mut dot = Canvas::new(64, 64);
    let dot_px = dot.paint(0.0, laser::paint_dot).to_vec();
    let (lw, lh) = (160u32, 256u32);
    let mut laser_img = vec![0u8; (lw * lh * 4) as usize];
    let wide = resize(&beam_px, 8, 256, 64, 256);
    blit(&mut laser_img, lw, &wide, 64, 256, 0, 0);
    blit(&mut laser_img, lw, &dot_px, 64, 64, 88, 96);
    let path = out.join("floating-laser.png");
    save_png(&path, &over_ground(&laser_img, lw, lh))?;
    println!("{}", path.display());
    Ok(())
}

/// The floating panel's picture after a second of frames with a laser at `laser_at`, as the overlay paints it.
fn paint_panel(snap: &Snapshot, page: TabPage, laser_at: Pos2) -> Vec<u8> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let backend: Ctl = std::sync::Arc::new(Still);
    let mut state = OverlayUi::default();
    state.page = page;
    let mut body = StandInBody { t_pose: false };
    let mut sw = crate::overlay::painter();
    let mut image = Vec::new();
    for f in 0..60 {
        let t = f as f64 / 60.0;
        let events = if f == 1 { vec![Event::PointerMoved(laser_at)] } else { Vec::new() };
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), time: Some(t), events, ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
        let mut full = ctx.run_ui(input, |ui| state.show_in(ui, snap, &backend, &mut body, Surface::Floating));
        sw.set_textures(&mut full.textures_delta);
        let prims = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        image = sw.paint(&prims, full.pixels_per_point, orbiters_ui::theme::INK_950).to_vec();
        sw.free_textures(&mut full.textures_delta);
    }
    image
}

fn paint_bar(hover: Option<panel::BarButton>, pressed: Option<panel::BarButton>, width: f32) -> Vec<u8> {
    let mut canvas = Canvas::new(panel::BAR_PX[0], panel::BAR_PX[1]);
    for _ in 0..2 {
        canvas.paint(0.0, |p| panel::paint_bar(p, hover, pressed, width));
    }
    canvas.pixels().to_vec()
}

/// The panel and its bar as they hang in the play space, at the panel's pixels per metre (`width` metres wide), with
/// the laser's dot where it points: (width, height, premultiplied RGBA).
fn hung(panel_px: &[u8], bar_px: &[u8], width: f32, laser_at: Option<Pos2>) -> (u32, u32, Vec<u8>) {
    let per_m = panel::PX[0] as f32 / width;
    let bar_w = (panel::BAR_WIDTH_M * per_m).round() as u32;
    let bar_h = (bar_w as f32 * panel::BAR_PX[1] as f32 / panel::BAR_PX[0] as f32).round() as u32;
    let gap = (0.012 * per_m).round() as u32;
    let (w, h) = (panel::PX[0], panel::PX[1] + gap + bar_h);
    let mut image = vec![0u8; (w * h * 4) as usize];
    blit(&mut image, w, panel_px, panel::PX[0], panel::PX[1], 0, 0);
    let bar = resize(bar_px, panel::BAR_PX[0], panel::BAR_PX[1], bar_w, bar_h);
    blit(&mut image, w, &bar, bar_w, bar_h, (w - bar_w) / 2, panel::PX[1] + gap);
    if let Some(at) = laser_at {
        let dot_px = (laser::DOT_WIDTH_M * per_m).round().max(4.0) as u32;
        let mut dot = Canvas::new(64, 64);
        let pixels = resize(dot.paint(0.0, laser::paint_dot), 64, 64, dot_px, dot_px);
        blit(&mut image, w, &pixels, dot_px, dot_px, (at.x as u32).saturating_sub(dot_px / 2), (at.y as u32).saturating_sub(dot_px / 2));
    }
    (w, h, image)
}

/// `src` (premultiplied, `sw` × `sh`) over `dst` (`dw` wide) at (`x`, `y`).
fn blit(dst: &mut [u8], dw: u32, src: &[u8], sw: u32, sh: u32, x: u32, y: u32) {
    let dh = dst.len() as u32 / 4 / dw;
    for j in 0..sh.min(dh.saturating_sub(y)) {
        for i in 0..sw.min(dw.saturating_sub(x)) {
            let s = ((j * sw + i) * 4) as usize;
            let d = (((y + j) * dw + x + i) * 4) as usize;
            let a = src[s + 3] as u32;
            for c in 0..4 {
                dst[d + c] = (src[s + c] as u32 + dst[d + c] as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

/// A premultiplied picture resized by averaging (smaller) or nearest (larger) samples.
fn resize(src: &[u8], sw: u32, sh: u32, nw: u32, nh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let (x0, x1) = (x * sw / nw, ((x + 1) * sw / nw).max(x * sw / nw + 1).min(sw));
            let (y0, y1) = (y * sh / nh, ((y + 1) * sh / nh).max(y * sh / nh + 1).min(sh));
            let mut sum = [0u32; 4];
            for j in y0..y1 {
                for i in x0..x1 {
                    let s = ((j * sw + i) * 4) as usize;
                    (0..4).for_each(|c| sum[c] += src[s + c] as u32);
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            let d = ((y * nw + x) * 4) as usize;
            (0..4).for_each(|c| out[d + c] = (sum[c] / n.max(1)) as u8);
        }
    }
    out
}
