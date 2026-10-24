//! Home: N4N0's menu as the avatar's screen and the SteamVR overlay show it, the same pages drawn by the same code
//! ([`crate::overlay::menu_ui`], or its retro look): fast walk, YouTube, calls, a 3D picture, music and the light,
//! side by side on its Home and each a page of its own. It runs here ([`crate::menu_host`]), in step with the tray
//! service like the one in VR; a press shows at once, on the pointer's press. On the search page the keyboard types.

use super::{Cx, PAD_X};
use crate::menu_host::MenuHost;
use crate::overlay::{menu_retro, menu_ui};
use egui::emath::TSTransform;
use egui::{pos2, vec2, CursorIcon, Event, Id, Key, Pos2, Rect, Sense, Ui};
use orbiters_core::settings::MenuLook;
use orbiters_orb::menu::layout::{self, ScreenPage};
use orbiters_orb::menu::{Page, Target};
use std::time::Duration;

/// The menu at its own size at most: the overlay's pixels are its points.
const MAX_SCALE: f32 = 1.0;
/// Space kept under the menu.
const BOTTOM: f32 = 28.0;

#[derive(Default)]
pub struct State {
    /// Made with the window's context on the first visit (thumbnails become its textures), opened on Home.
    host: Option<MenuHost>,
    hover: Option<usize>,
    /// The element pressed, until the pointer is let go.
    pressed: Option<usize>,
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let host = state.host.get_or_insert_with(|| {
        let mut host = MenuHost::new(ctx.clone());
        host.open(snap);
        host
    });
    host.follow(snap);
    // Home's music tile names what plays, and the music page shows it: read while one of them shows.
    if matches!(host.menu.page, Page::Home | Page::Music) {
        host.follow_media();
    }
    let mut effects = host.poll();

    // The menu, as large as the page lets it be, under the title bar.
    let page = ui.max_rect();
    let room = Rect::from_min_max(pos2(page.left() + PAD_X, page.top() + cx.top + 8.0), pos2(page.right() - PAD_X, page.bottom() - BOTTOM));
    let size = vec2(layout::WIDTH as f32, layout::HEIGHT as f32);
    let scale = (room.width() / size.x).min(room.height() / size.y).clamp(0.2, MAX_SCALE);
    let card = Rect::from_center_size(pos2(room.center().x, room.top() + (room.height() - size.y * scale).max(0.0) * 0.4 + size.y * scale / 2.0), size * scale);
    let to_menu = |p: Pos2| ((p - card.min) / scale).to_pos2();

    // The pointer: hover, and a press on the pointer's press (what it does follows).
    let response = ui.interact(card, Id::unique("orbiters-home-menu"), Sense::click_and_drag());
    let shown = layout::cached(ScreenPage::of(&host.menu));
    // The menu stays here: its close button is left out.
    let hit = |p: Pos2| {
        let m = to_menu(p);
        layout::hit(shown, m.x, m.y).filter(|(i, target)| *target != Target::Close && layout::state(&host.menu, &shown.elements[*i]).pointable())
    };
    let hover = response.hover_pos().and_then(hit).map(|(i, _)| i);
    if hover.is_some() {
        response.clone().on_hover_cursor(CursorIcon::PointingHand);
    }
    let press = ui.input(|i| i.pointer.primary_pressed().then(|| i.pointer.interact_pos()).flatten());
    if let Some((i, target)) = press.filter(|p| card.contains(*p)).and_then(hit) {
        state.pressed = Some(i);
        effects.extend(host.press(target));
    }
    if !ui.input(|i| i.pointer.primary_down()) {
        state.pressed = None;
    }
    if hover != state.hover {
        state.hover = hover;
        host.dirty = true;
    }

    // The keyboard: typing on the search page, Enter searches, Escape goes back a page.
    let keys: Vec<Target> = ui.input(|i| {
        let typing = host.menu.page == Page::Search;
        let mut keys = Vec::new();
        for event in &i.events {
            match event {
                Event::Text(text) if typing => keys.extend(text.chars().map(|c| if c == ' ' { Target::Space } else { Target::Key(c) })),
                Event::Key { key: Key::Backspace, pressed: true, .. } if typing => keys.push(Target::Backspace),
                Event::Key { key: Key::Enter, pressed: true, .. } if typing => keys.push(Target::Search),
                Event::Key { key: Key::Escape, pressed: true, .. } if host.menu.page != Page::Home => keys.push(Target::Back),
                _ => {}
            }
        }
        keys
    });
    for key in keys {
        effects.extend(host.press(key));
    }

    // It stays open here (no close button; going idle does not close it either): a close goes back to Home.
    if host.effects(effects, snap, cx.ctl) {
        host.open(snap);
    }

    let retro = snap.settings.orb.menu.look == MenuLook::Retro;
    let time = ui.input(|i| i.time);
    let view =
        menu_ui::View { menu: &host.menu, hover: state.hover, pressed: state.pressed, picture: &|key: &str| host.picture(key, retro), time, stays: true };
    // Drawn at the menu's own size, then scaled into place (shapes, text and their clip with them): the clip is the
    // page's, brought into the menu's own points, and the menu's card.
    let page_clip = ui.clip_rect();
    let clip = Rect::from_min_max(to_menu(page_clip.min), to_menu(page_clip.max)).intersect(Rect::from_min_size(Pos2::ZERO, size));
    ui.with_visual_transform(TSTransform::new(card.min.to_vec2(), scale), |ui| {
        let mut painter = ui.painter().clone();
        painter.set_clip_rect(clip);
        if retro {
            menu_retro::draw(&painter, Pos2::ZERO, &view);
        } else {
            menu_ui::draw(&painter, Pos2::ZERO, &view);
        }
    });
    host.dirty = false;

    // What arrives on its own: N4N0 listening (the microphone breathes), YouTube's answers and thumbnails, the music.
    let wait = if host.menu.listening {
        Some(Duration::from_millis(50))
    } else {
        match host.menu.page {
            Page::Search | Page::Results => Some(Duration::from_millis(100)),
            Page::Home | Page::Music => Some(Duration::from_millis(500)),
            _ => None,
        }
    };
    if let Some(wait) = wait {
        ctx.request_repaint_after(wait);
    }
}
