//! `--only orbit`: the panels around VRChat's menus (overlay/orbit) as they are painted, placed as in a window of each size around a stand-in of the wheel or the Quick
//! Menu: `orbit-wheel-*.png`, `orbit-qm-*.png`, one per state; `orbit-takeover*.png` N4N0's menu in front of the Quick Menu. What the real game looks like with them is in
//! reports/orbit-overlays (captured over the bridge).

use super::over_ground;
use crate::overlay::menu_ui::{self, Skin, View};
use crate::overlay::orbit::panels::{self, Facts, Recent};
use crate::overlay::orbit::zones::{self, Around, Placed, Zone};
use crate::overlay::Canvas;
use egui::{pos2, vec2, Color32, ColorImage, TextureOptions, Vec2};
use orbiters_n4n0_core::menu::{KeyboardLayout, Menu, PhotoLength};
use orbiters_ui::offscreen::save_png;
use std::path::{Path, PathBuf};

/// Four recent captures, with stand-in pictures (dusk colours) that `paint` makes textures of.
fn recent() -> Vec<Recent> {
    [("Sky Dream", "3D picture, ready"), ("Rainy Skyline", "Reconstructing 42%"), ("Pacific Dreams (Night)", "Recording, waiting"), ("After The Rain", "3D picture, ready")]
        .iter()
        .map(|(title, state)| Recent { dir: PathBuf::from(title), title: title.to_string(), state: state.to_string(), picture: None })
        .collect()
}

fn stand_in(seed: u8) -> ColorImage {
    let (w, h) = (160usize, 90usize);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let t = y as f32 / h as f32;
            let glow = 1.0 - ((x as f32 / w as f32 - 0.3 - seed as f32 * 0.12).abs() * 2.0).min(1.0);
            pixels.push(Color32::from_rgb((40.0 + 180.0 * (1.0 - t) * glow) as u8, (30.0 + 80.0 * (1.0 - t)) as u8, (60.0 + 90.0 * t) as u8));
        }
    }
    ColorImage::new([w, h], pixels)
}

/// A panel's picture, as the platform gets it: painted twice (what loads on the first frame shows on the second), and cut where the wheel bites.
fn picture(placed: &Placed, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) -> (Vec<u8>, [u32; 2]) {
    let px = [placed.rect.width().round() as u32, placed.rect.height().round() as u32];
    let logical = placed.logical();
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / logical.x);
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    for (recent, texture) in facts.recent.iter_mut().zip(&textures) {
        recent.picture = Some(texture.id());
    }
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint(p, placed.zone, logical, &facts, hover, pressed));
    }
    let mut out = canvas.pixels().to_vec();
    if let Some(b) = placed.zone.bite() {
        let center = ((placed.center - placed.rect.min)).to_pos2();
        panels::bite(&mut out, px, center, b * placed.unit, [0, 0, px[0], px[1]]);
    }
    (out, px)
}

fn blit(onto: &mut [u8], width: u32, pixels: &[u8], px: [u32; 2], at: egui::Pos2) {
    for y in 0..px[1] {
        for x in 0..px[0] {
            let (ox, oy) = (at.x as u32 + x, at.y as u32 + y);
            let i = ((oy * width + ox) * 4) as usize;
            if i + 3 >= onto.len() || ox >= width {
                continue;
            }
            let j = ((y * px[0] + x) * 4) as usize;
            let a = pixels[j + 3] as u32;
            for c in 0..4 {
                onto[i + c] = (pixels[j + c] as u32 + onto[i + c] as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

/// A stand-in of the menu the panels surround, to judge the gaps.
fn stand_in_menu(around: Around, client: Vec2, canvas: &mut Canvas) {
    let (center, unit) = zones::frame(around, client);
    canvas.paint(0.0, |p| match around {
        Around::Wheel => {
            p.circle_filled(center, 100.0 * unit, Color32::from_rgb(7, 55, 58));
            p.circle_filled(center, 48.0 * unit, Color32::from_rgb(22, 34, 43));
        }
        Around::QuickMenu => {
            let r = |x0: f32, y0: f32, x1: f32, y1: f32| egui::Rect::from_min_max(center + vec2(x0, y0) * unit, center + vec2(x1, y1) * unit);
            p.rect_filled(r(-351.0, -150.0, 351.0, 186.0), 8.0, Color32::from_rgb(25, 34, 42));
            p.rect_filled(r(-188.0, -204.0, 188.0, 204.0), 8.0, Color32::from_rgb(22, 34, 43));
            for x in zones::QM_ICONS_X {
                p.circle_filled(center + vec2(x, zones::QM_ICONS_Y) * unit, 13.0 * unit, Color32::from_rgb(30, 90, 95));
            }
        }
    });
}

fn scene(name: &str, out: &Path, around: Around, client: Vec2, facts: &Facts, hover: Option<(Zone, usize, bool)>) -> anyhow::Result<()> {
    let (w, h) = (client.x as u32, client.y as u32);
    let mut ground = Canvas::new(w, h);
    stand_in_menu(around, client, &mut ground);
    let mut image = ground.pixels().to_vec();
    for placed in zones::layout(around, client) {
        let (hovered, pressed) = match hover {
            Some((zone, i, down)) if zone == placed.zone => (Some(i), down.then_some(i)),
            _ => (None, None),
        };
        let (pixels, px) = picture(&placed, facts, hovered, pressed);
        blit(&mut image, w, &pixels, px, placed.rect.min);
    }
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

pub(super) fn shots(out: &Path) -> anyhow::Result<()> {
    let hd = vec2(1280.0, 720.0);
    let idle = Facts { recent: recent(), timer: 3, n4n0: true, desktop: true, ..Default::default() };
    let empty = Facts { timer: 3, n4n0: true, ..Default::default() };
    let busy = Facts {
        recording: true,
        recording_seconds: Some(83),
        capture_busy: true,
        reconstructing: Some(0.42),
        waiting: 2,
        fast_walk: true,
        music: Some(("Midnight City".into(), true)),
        ..idle.clone()
    };
    let counting = Facts { countdown: Some(2), ..idle.clone() };
    for (around, tag) in [(Around::Wheel, "wheel"), (Around::QuickMenu, "qm")] {
        scene(&format!("orbit-{tag}-idle"), out, around, hd, &idle, None)?;
        scene(&format!("orbit-{tag}-empty"), out, around, hd, &empty, None)?;
        scene(&format!("orbit-{tag}-busy"), out, around, hd, &busy, None)?;
        scene(&format!("orbit-{tag}-countdown"), out, around, hd, &counting, None)?;
        scene(&format!("orbit-{tag}-1080"), out, around, vec2(1920.0, 1080.0), &busy, None)?;
        scene(&format!("orbit-{tag}-540"), out, around, vec2(960.0, 540.0), &idle, None)?;
        scene(&format!("orbit-{tag}-narrow"), out, around, vec2(800.0, 800.0), &idle, None)?;
    }
    scene("orbit-wheel-hover", out, Around::Wheel, hd, &idle, Some((Zone::WheelTop, 0, false)))?;
    scene("orbit-wheel-pressed", out, Around::Wheel, hd, &idle, Some((Zone::WheelTop, 1, true)))?;
    scene("orbit-qm-hover", out, Around::QuickMenu, hd, &idle, Some((Zone::QmRight, 2, false)))?;
    // N4N0's menu in front of the Quick Menu.
    for (tag, client) in [("orbit-takeover", hd), ("orbit-takeover-1080", vec2(1920.0, 1080.0))] {
        let (w, h) = (client.x as u32, client.y as u32);
        let mut ground = Canvas::new(w, h);
        stand_in_menu(Around::QuickMenu, client, &mut ground);
        let mut image = ground.pixels().to_vec();
        for placed in zones::layout(Around::QuickMenu, client) {
            let (pixels, px) = picture(&placed, &idle, None, None);
            blit(&mut image, w, &pixels, px, placed.rect.min);
        }
        let placed = zones::place(Zone::Takeover, client).expect("it fits");
        let px = [placed.rect.width() as u32, placed.rect.height() as u32];
        let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32);
        let mut menu = Menu::new(KeyboardLayout::Qwerty, 5, PhotoLength::Standard);
        menu.open();
        let none = |_: &str| -> Option<egui::TextureId> { None };
        let view = View { menu: &menu, hover: Some(2), pressed: None, picture: &none, time: 0.0, skin: Skin::english() };
        for _ in 0..2 {
            canvas.paint(0.0, |p| menu_ui::draw(p, pos2(0.0, 0.0), &view));
        }
        blit(&mut image, w, canvas.pixels(), px, placed.rect.min);
        let path = out.join(format!("{tag}.png"));
        save_png(&path, &over_ground(&image, w, h))?;
        println!("{}", path.display());
    }
    Ok(())
}
