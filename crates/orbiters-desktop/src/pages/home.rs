//! Home: N4N0's menu as a page of the window. Its seven features (fast walk, YouTube, calls, a 3D picture, music, the
//! light, the avatar's saved states) are cards like the rest of the app's, and each opens a page of its own drawn the
//! same way; what N4N0 says fills the grid's last row. What they show and what a press does are the menu's
//! (`orbiters_orb::menu`, run by [`crate::menu_host`] in step with the tray service like the menu in VR); only the
//! drawing is the window's (the States page is [`super::states`]). Cards and choices act on the pointer's press, buttons
//! on its release with the press shown at once, and the menu shows a change before the service answers.

use super::{block, section, Cx};
use crate::ipc::OrbCommand;
use crate::menu_host::MenuHost;
use crate::overlay::menu_ui;
use crate::state::StateAction;
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align, Align2, Color32, CursorIcon, Id, Key, Layout, Painter, Pos2, Rect, Sense, Stroke, TextureId, Ui, UiBuilder, Vec2};
use orbiters_contracts::orb::{LIGHT_COLORS, LIGHT_LEVELS};
use orbiters_core::settings::Settings;
use orbiters_orb::menu::layout::{self, Content, Tint, HOME_TILES, LENGTH_LABELS, TIMER_LABELS};
use orbiters_orb::menu::{Call, Effect, MediaCommand, Menu, Page, PhotoLength, Target, Volume, TIMERS};
use orbiters_orb::youtube::Video;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Size, Tone, Variant};
use std::time::Duration;

/// The page's width at most, as the Settings cards'.
const MAX_W: f32 = 980.0;
/// Home's feature cards, the room between them, and their insets (the Settings cards').
const CARD_H: f32 = 128.0;
const GAP: f32 = 16.0;
const INSET: f32 = 24.0;
/// A result's thumbnail.
const THUMB: Vec2 = vec2(160.0, 90.0);
/// How long a result's button says its link was copied.
const COPIED_FOR: f64 = 2.0;

/// What a press asks of the menu, done once the page is drawn.
enum Act {
    Press(Target),
    /// The search field's new text.
    Query(String),
    /// Calls the friend at this place in the list.
    Call(usize),
    /// The speakers' volume, in percent.
    Volume(u8),
    /// What the States page asks of the service.
    States(StateAction),
    /// Back to Home itself.
    Home,
}

#[derive(Default)]
pub struct State {
    /// Made with the window's context on the first visit (thumbnails become its textures), opened on Home.
    host: Option<MenuHost>,
    /// The page shown (YouTube's search and results are one), since when, and how many were opened: a feature's page
    /// enters like a page of its own.
    shown: Option<Page>,
    since: f64,
    opened: u64,
    /// The search field takes the keyboard when YouTube opens.
    focus: bool,
    /// The result whose link was copied last, and when.
    copied: Option<(String, f64)>,
    /// The States page's own state (what was asked and not yet seen, a name being typed).
    states: super::states::StatesPage,
}

impl State {
    /// Back to Home itself (the sidebar's Home pressed on a feature's page).
    pub fn back_home(&mut self) {
        if let Some(host) = self.host.as_mut() {
            go_home(host);
        }
    }

    /// The menu Home runs, once shown (the screenshot review opens its pages).
    pub fn menu_host(&mut self) -> Option<&mut MenuHost> {
        self.host.as_mut()
    }
}

/// YouTube's search and its results are one page here.
fn shown_page(page: Page) -> Page {
    match page {
        Page::Results => Page::Search,
        page => page,
    }
}

fn go_home(host: &mut MenuHost) {
    // Results go back to the search, the search to Home.
    for _ in 0..2 {
        if host.menu.page != Page::Home {
            host.press(Target::Back);
        }
    }
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let State { host, shown, since, opened, focus, copied, states } = state;
    let host = host.get_or_insert_with(|| {
        let mut host = MenuHost::new(ctx.clone());
        host.open(snap);
        host
    });
    host.follow(snap);
    // The music card names what plays, and the music page shows it: read while one of them shows.
    if matches!(host.menu.page, Page::Home | Page::Music) {
        host.follow_media();
    }
    let mut effects = host.poll();

    // A page opened enters as a page of its own: its blocks count from then.
    let page = shown_page(host.menu.page);
    let now = anim::time(&ctx);
    if *shown != Some(page) {
        *shown = Some(page);
        *since = now;
        *opened += 1;
        *focus = page == Page::Search;
    }
    let (visit, entered) = (cx.visit, cx.entered);
    cx.visit = (visit << 20) | *opened;
    cx.entered = entered.max(*since);

    let mut acts = Vec::new();
    if page != Page::Home && ui.input(|i| i.key_pressed(Key::Escape)) {
        acts.push(Act::Home);
    }
    // The 3D picture as the Capture page allows it.
    let take_blocked = if !snap.settings.consent_acknowledged {
        Some("Accept the capture notice on the Capture page first")
    } else if !snap.vrchat.running {
        Some("Start VRChat first: its camera takes the picture")
    } else {
        None
    };
    let idle = idle_line(&snap.settings);
    let menu = &host.menu;
    let picture = |key: &str| host.picture(key, false);
    let copied_id = copied.as_ref().filter(|(_, at)| now - at < COPIED_FOR).map(|(id, _)| id.as_str());
    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt(("home", page)).auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            match page {
                // The window never shows a dropped card itself (the avatar's screen does).
                Page::Home | Page::Card => home(ui, cx, menu, &idle, &mut acts),
                Page::Search | Page::Results => youtube(ui, cx, menu, &picture, focus, copied_id, &mut acts),
                Page::Call => call(ui, cx, menu, &mut acts),
                Page::Photo => photo(ui, cx, menu, take_blocked, &mut acts),
                Page::Music => music(ui, cx, menu, &picture, &mut acts),
                Page::Light => light(ui, cx, menu, &mut acts),
                Page::States => {
                    let mut out = Vec::new();
                    super::states::show(ui, cx, states, &mut out);
                    acts.extend(out.into_iter().map(|o| match o {
                        super::states::Out::Home => Act::Home,
                        super::states::Out::Action(action) => Act::States(action),
                    }));
                }
            }
        });
    });
    // Down to where the content starts (its top margin).
    super::under_top(ui, cx, "home", from, ui.max_rect().top() + cx.top + 8.0);
    cx.visit = visit;
    cx.entered = entered;

    for act in acts {
        match act {
            Act::Press(target) => {
                if let Target::Copy(i) = target {
                    if let Some(video) = host.menu.page_results().get(i as usize) {
                        *copied = Some((video.id.clone(), now));
                    }
                }
                effects.extend(host.press(target));
            }
            Act::Query(text) => {
                // Typing after a search: back to the suggestions.
                if host.menu.page == Page::Results {
                    host.press(Target::Back);
                }
                effects.extend(host.menu.set_query(&text));
            }
            Act::Call(i) => effects.extend(host.menu.call_contact(i)),
            Act::Volume(percent) => {
                host.menu.volume = Some(Volume { percent, muted: false });
                effects.push(Effect::Media(MediaCommand::Volume(percent)));
            }
            Act::States(action) => cx.ctl.orb(OrbCommand::States { action }),
            Act::Home => go_home(host),
        }
    }
    // It stays open here (nothing closes it): a close goes back to Home. A dropped card is on the avatar's screen: the
    // results stay here.
    if host.effects(effects, snap, cx.ctl) {
        host.open(snap);
    }
    host.dropped = false;

    // What arrives on its own: YouTube's answers and thumbnails, the music, a call's clock; a copied link's note ends.
    let menu = &host.menu;
    let wait = match menu.page {
        Page::Search | Page::Results => Some(Duration::from_millis(100)),
        Page::Home | Page::Music => Some(Duration::from_millis(500)),
        _ if matches!(menu.call, Some(Call::Connected { .. })) => Some(Duration::from_secs(1)),
        _ => None,
    };
    if let Some(wait) = wait {
        ctx.request_repaint_after(wait);
    }
    if let Some((_, at)) = copied {
        let left = COPIED_FOR - (now - *at);
        if left > 0.0 {
            ctx.request_repaint_after(Duration::from_secs_f64(left));
        }
    }
}

/// What Home's heading says while N4N0 does not listen: how to call it from here.
fn idle_line(settings: &Settings) -> String {
    match settings.orb.wake_words.first().filter(|_| settings.orb.wake_word) {
        Some(word) => {
            let mut letters = word.chars();
            let word: String = letters.next().map(|c| c.to_uppercase().chain(letters).collect()).unwrap_or_default();
            format!("Pick a feature, or say “{word}” to talk to N4N0")
        }
        None => "Pick a feature, or hold Talk on N4N0's page to talk to it".into(),
    }
}

fn home(ui: &mut Ui, cx: &Cx, menu: &Menu, idle: &str, acts: &mut Vec<Act>) {
    block(ui, cx, 0, 72.0, |ui, rect| {
        let p = ui.painter();
        widgets::text(p, pos2(rect.left(), rect.top() + 7.0), Align2::LEFT_CENTER, &Txt::caps("N4N0 menu"));
        widgets::text(p, pos2(rect.left(), rect.top() + 34.0), Align2::LEFT_CENTER, &Txt::new("Home", 30.0).weight(weight::SEMIBOLD).tracking(-0.015));
        let y = rect.top() + 62.0;
        if menu.listening {
            // N4N0 listens: its dot breathes.
            widgets::status_dot(p, pos2(rect.left() + 5.0, y), Tone::Ok, true);
            let line = Txt::new(layout::text(menu, Content::Status), 13.0).color(theme::ACCENT);
            widgets::text_truncated(p, pos2(rect.left() + 18.0, y), Align2::LEFT_CENTER, &line, rect.width() * 0.6);
        } else {
            widgets::text_truncated(p, pos2(rect.left(), y), Align2::LEFT_CENTER, &Txt::new(idle, 13.0).color(theme::FG_3), rect.width() * 0.6);
        }
    });

    let w = ui.available_width().min(MAX_W);
    let cols = home_columns(w);
    let card_w = (w - GAP * (cols - 1) as f32) / cols as f32;
    // The features, then what N4N0 says in the rest of the last row (one place at least): the grid stays whole.
    let cells = HOME_TILES.len() + 1;
    let rows = cells.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (CARD_H + GAP) - GAP), Sense::hover());
    let cell = |i: usize| area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (CARD_H + GAP));
    for i in 0..HOME_TILES.len() {
        feature(ui, cx, i, Rect::from_min_size(cell(i), vec2(card_w, CARD_H)), menu, acts);
    }
    let span = cols - HOME_TILES.len() % cols;
    let at = cell(HOME_TILES.len());
    said(ui, cx, 1 + HOME_TILES.len(), Rect::from_min_size(at, vec2(span as f32 * (card_w + GAP) - GAP, CARD_H)), menu);
}

/// Home's columns at `width`: four on a wide window, three, two on a narrow one.
fn home_columns(width: f32) -> usize {
    if width >= 900.0 {
        4
    } else if width >= 700.0 {
        3
    } else {
        2
    }
}

/// One of Home's features: its icon chip, its name and what it says now, with a switch for what turns on and off (fast
/// walk, the light) or an arrow for what opens a page.
fn feature(ui: &mut Ui, cx: &Cx, i: usize, rect: Rect, menu: &Menu, acts: &mut Vec<Act>) {
    let (target, icon, title, tint) = HOME_TILES[i];
    let state = layout::target_state(menu, target);
    let (rect, alpha) = super::entering(ui, cx, 1 + i, rect);
    let ctx = ui.ctx().clone();
    let id = Id::unique(("orbiters-home-feature", i));
    let enabled = !state.unavailable;
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && widgets::pressed(&resp) {
        acts.push(Act::Press(target));
    }
    let hover = anim::toggle(&ctx, id.with("hover"), enabled && resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand);
    }
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.025 * press));
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha * if enabled { 1.0 } else { 0.5 });
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::INK_800, hover));

    // The chip has the feature's colour: grey while what it switches is off, the light's own colour while it is on.
    let switched = matches!(target, Target::FastWalk | Target::Light);
    let color = match target {
        Target::Light => menu.light.filter(|l| l.on).map(|l| menu_ui::tint(Tint::Light(l.color))),
        _ if switched && !state.on => None,
        _ => Some(menu_ui::tint(tint)),
    };
    let (bg, fg) = match color {
        Some(c) => (theme::alpha(c, 0.12), c),
        None => (theme::white(0.05), theme::FG_3),
    };
    let bg = anim::color(&ctx, id.with("chip-bg"), bg, Spring::SOFT);
    let fg = anim::color(&ctx, id.with("chip-fg"), fg, Spring::SOFT);
    let chip = Rect::from_min_size(r.min + vec2(INSET, 22.0), vec2(36.0, 36.0));
    p.rect_filled(chip, theme::radius(11.0), bg);
    widgets::icon(&p, chip.center(), menu_ui::glyph(icon), 17.0, fg);

    let x = r.left() + INSET;
    widgets::text(&p, pos2(x, r.bottom() - 46.0), Align2::LEFT_CENTER, &Txt::new(title, 15.5).weight(weight::SEMIBOLD));
    let active = state.on
        || match target {
            Target::Call => menu.call.is_some(),
            Target::Photo => menu.countdown.is_some(),
            Target::Music => menu.media.as_ref().is_some_and(|m| m.playing && !m.title.is_empty()),
            Target::States => menu.states.iter().any(|s| s.default),
            _ => false,
        };
    let note = Txt::new(layout::text(menu, Content::TileNote(i as u8)), 12.5).color(if active { theme::ACCENT } else { theme::FG_3 });
    widgets::text_truncated(&p, pos2(x, r.bottom() - 24.0), Align2::LEFT_CENTER, &note, r.width() - 2.0 * INSET);

    // On the right: a switch over the card (pressed apart from it), or the way in.
    let switch = match target {
        Target::FastWalk => Some(Target::FastWalk),
        Target::Light if menu.light.is_some() => Some(Target::LightSwitch),
        _ => None,
    };
    match switch {
        Some(switch) => {
            let sr = Rect::from_min_size(pos2(r.right() - INSET - 48.0, chip.center().y - 14.0), vec2(48.0, 28.0));
            let mut su = ui.new_child(UiBuilder::new().max_rect(sr).id_salt(("orbiters-home-switch", i)));
            su.multiply_opacity(alpha);
            if !enabled {
                su.disable();
            }
            let mut on = state.on;
            if widgets::toggle(&mut su, "switch", &mut on) {
                acts.push(Act::Press(switch));
            }
        }
        None => widgets::icon(&p, pos2(r.right() - INSET - 8.0, chip.center().y), icons::CARET_RIGHT, 16.0, theme::mix(theme::FG_4, theme::FG_2, hover)),
    }
}

/// What N4N0 said last (or heard: the owner's own words), in the last row of the features: N4N0 beside its caption, the
/// words under them.
fn said(ui: &mut Ui, cx: &Cx, index: usize, rect: Rect, menu: &Menu) {
    let (r, alpha) = super::entering(ui, cx, index, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::surface());
    let chip = Rect::from_min_size(r.min + vec2(INSET, 22.0), vec2(36.0, 36.0));
    p.rect_filled(chip, theme::radius(11.0), theme::alpha(theme::ACCENT, 0.12));
    orb_face(&p, chip.center(), 10.0);
    let caption = match (&menu.heard, menu.heard_private) {
        (None, _) => "N4N0",
        (Some(_), true) => "You said",
        (Some(_), false) => "N4N0 says",
    };
    widgets::text(&p, pos2(chip.right() + 12.0, chip.center().y), Align2::LEFT_CENTER, &Txt::caps(caption));
    let (text, color) = match menu.heard.as_deref() {
        Some(text) => (format!("“{text}”"), theme::FG_2),
        None => ("Nothing said yet: N4N0's answers show here.".to_string(), theme::FG_3),
    };
    let mut job = Txt::new(text, 13.0).color(color).line_height(18.0).job(r.width() - 2.0 * INSET);
    job.wrap.max_rows = 2;
    let lines = p.layout_job(job);
    p.galley(pos2(r.left() + INSET, chip.bottom() + 10.0), lines, color);
}

/// N4N0 itself, `radius` large: its green dot with its eyes, as the menu's back shows it.
fn orb_face(p: &Painter, center: Pos2, radius: f32) {
    let s = radius / 26.0;
    p.circle_filled(center, radius, theme::ACCENT);
    for side in [-1.0, 1.0] {
        let eye = Rect::from_center_size(center + vec2(side * 9.0 * s, -2.0 * s), vec2(7.0 * s, 13.0 * s));
        p.rect_filled(eye, theme::radius(3.5 * s), menu_ui::CARD);
    }
}

/// The top of a feature's page: back to Home, over its heading.
fn top(ui: &mut Ui, cx: &Cx, title: &str, sub: &str, acts: &mut Vec<Act>) {
    block(ui, cx, 0, 116.0, |ui, rect| {
        let back = Button::new("Home").icon(icons::ARROW_LEFT).variant(Variant::Ghost).size(Size::Sm);
        let w = back.desired_width(ui);
        if back.show_at(ui, Rect::from_min_size(rect.min, vec2(w, 32.0))).clicked() {
            acts.push(Act::Home);
        }
        super::heading(ui, Rect::from_min_max(pos2(rect.left(), rect.top() + 44.0), rect.max), "N4N0 menu", title, Some(sub));
    });
}

/// A card that only explains (nothing to show, nothing to call yet): its heading and a few lines under it.
fn note_card(ui: &mut Ui, cx: &Cx, index: usize, glyph: &str, title: &str, text: &str) {
    section(ui, cx, index, glyph, title, None, |ui| {
        let w = ui.available_width() - 48.0;
        let lines = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::FG_3).line_height(19.0).job(w));
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), lines.size().y + 8.0), Sense::hover());
        ui.painter().galley(pos2(r.left() + 48.0, r.top()), lines, theme::FG_3);
    });
}

/// A row of a card that acts on the pointer's press (a suggestion), a little lighter under the pointer; returns its rect,
/// how much it is hovered, and whether it was pressed.
fn press_row(ui: &mut Ui, id: Id, height: f32) -> (Rect, f32, bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let pressed = widgets::pressed(&resp);
    let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let down = anim::toggle(ui.ctx(), id.with("down"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    if hover + down > 0.0 {
        ui.painter().rect_filled(rect.expand2(vec2(12.0, 0.0)), theme::radius(12.0), theme::white(0.04 * hover + 0.05 * down));
    }
    resp.on_hover_cursor(CursorIcon::PointingHand);
    (rect, hover, pressed)
}

/// A round icon button, or its icon greyed out while it does nothing; true when clicked.
fn icon_button(ui: &mut Ui, rect: Rect, glyph: &str, label: &str, enabled: bool) -> bool {
    if enabled {
        return IconButton::new(glyph, label).size(rect.width(), 18.0).show_at(ui, rect).clicked();
    }
    widgets::icon(ui.painter(), rect.center(), glyph, 18.0, theme::alpha(theme::FG_4, 0.6));
    false
}

/// A picture with rounded corners, or its place while it loads.
fn thumbnail(p: &Painter, r: Rect, texture: Option<TextureId>, radius: f32) {
    match texture {
        Some(id) => {
            p.add(RectShape::filled(r, theme::radius(radius), Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        None => {
            p.rect_filled(r, theme::radius(radius), theme::surface_soft());
        }
    }
}

fn youtube(ui: &mut Ui, cx: &Cx, menu: &Menu, picture: &dyn Fn(&str) -> Option<TextureId>, focus: &mut bool, copied: Option<&str>, acts: &mut Vec<Act>) {
    top(ui, cx, "YouTube", "Find a video, then copy its link for the world's video player", acts);
    let w = ui.available_width().min(MAX_W);
    block(ui, cx, 1, 40.0, |ui, rect| {
        let button_w = 104.0;
        let field = Rect::from_min_size(rect.min, vec2(w - button_w - 12.0, 40.0));
        let mut fu = ui.new_child(UiBuilder::new().max_rect(field).layout(Layout::left_to_right(Align::Center)));
        let mut text = menu.query.clone();
        let resp = widgets::search_field(&mut fu, "orbiters-home-search", &mut text, "Search YouTube", field.width());
        if std::mem::take(focus) {
            resp.request_focus();
        }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let search = Button::new("Search").variant(Variant::Primary).enabled(!text.trim().is_empty()).width(button_w);
        let clicked = search.show_at(ui, Rect::from_min_size(pos2(field.right() + 12.0, rect.top()), vec2(button_w, 40.0))).clicked();
        if text != menu.query {
            acts.push(Act::Query(text.clone()));
        }
        if (enter || clicked) && !text.trim().is_empty() {
            acts.push(Act::Press(Target::Search));
        }
    });
    match menu.page {
        Page::Results => results(ui, cx, menu, picture, copied, acts),
        _ => suggestions(ui, cx, menu, acts),
    }
}

fn suggestions(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let shown: Vec<&str> = menu.shown_suggestions().collect();
    if shown.is_empty() {
        let text = "Type what you want to watch and press Enter. A result's Copy link puts it in the clipboard, ready to paste in the world's video player.";
        return note_card(ui, cx, 2, icons::YOUTUBE_LOGO, "Search YouTube", text);
    }
    section(ui, cx, 2, icons::MAGNIFYING_GLASS, "Suggestions", None, |ui| {
        ui.add_space(4.0);
        for (i, text) in shown.iter().enumerate() {
            let (r, hover, pressed) = press_row(ui, Id::unique(("orbiters-home-suggestion", i)), 44.0);
            if pressed {
                acts.push(Act::Press(Target::Suggestion(i as u8)));
            }
            let p = ui.painter();
            widgets::icon(p, pos2(r.left() + 8.0, r.center().y), icons::MAGNIFYING_GLASS, 14.0, theme::FG_4);
            let line = Txt::new(*text, 13.5).color(theme::mix(theme::FG_2, theme::FG, hover));
            widgets::text_truncated(p, pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, &line, r.width() - 40.0);
        }
    });
}

fn results(ui: &mut Ui, cx: &Cx, menu: &Menu, picture: &dyn Fn(&str) -> Option<TextureId>, copied: Option<&str>, acts: &mut Vec<Act>) {
    let sub = layout::text(menu, Content::ResultsTitle);
    section(ui, cx, 2, icons::YOUTUBE_LOGO, "Results", Some(&sub), |ui| {
        ui.add_space(4.0);
        if menu.loading() {
            for i in 0..3 {
                if i > 0 {
                    widgets::separator(ui);
                }
                placeholder_row(ui);
            }
            return;
        }
        for (i, video) in menu.page_results().iter().enumerate() {
            if i > 0 {
                widgets::separator(ui);
            }
            // After its Copy link, the row offers Drop (the avatar's screen shows the card in the world), and says it was
            // copied as long as it does.
            let drop = !layout::target_state(menu, Target::Drop(i as u8)).hidden;
            if let Some(target) = result_row(ui, i, video, picture(&video.id), drop || copied == Some(video.id.as_str()), drop) {
                acts.push(Act::Press(target));
            }
        }
        if menu.has_previous() || menu.has_next() {
            widgets::separator(ui);
            if let Some(target) = pager(ui, menu) {
                acts.push(Act::Press(target));
            }
        }
    });
}

/// A result: its thumbnail and length, its title and channel, its Copy link button and, once copied where a card can be
/// dropped, its Drop; the one clicked.
fn result_row(ui: &mut Ui, i: usize, video: &Video, texture: Option<TextureId>, copied: bool, drop: bool) -> Option<Target> {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), THUMB.y + 28.0), Sense::hover());
    let thumb = Rect::from_min_size(pos2(row.left(), row.center().y - THUMB.y / 2.0), THUMB);
    let button_w = 124.0;
    let drop_w = if drop { 96.0 + 8.0 } else { 0.0 };
    let x = thumb.right() + 16.0;
    let text_w = (row.right() - button_w - drop_w - 20.0 - x).max(80.0);
    {
        let p = ui.painter();
        thumbnail(p, thumb, texture, 10.0);
        let length = if video.live { Some(("LIVE", theme::REC, Color32::WHITE)) } else { video.duration.as_deref().map(|d| (d, theme::black(0.7), theme::FG)) };
        if let Some((label, bg, fg)) = length {
            super::sessions::badge(p, ui, pos2(thumb.right() - 6.0, thumb.bottom() - 28.0), label, bg, fg, true);
        }
        let mut job = Txt::new(&video.title, 14.0).weight(weight::MEDIUM).line_height(19.0).job(text_w);
        job.wrap.max_rows = 2;
        let title = p.layout_job(job);
        let title_h = title.size().y;
        let top = row.center().y - (title_h + 22.0) / 2.0;
        p.galley(pos2(x, top), title, theme::FG);
        widgets::text_truncated(p, pos2(x, top + title_h + 14.0), Align2::LEFT_CENTER, &Txt::new(&video.channel, 12.5).color(theme::FG_3), text_w);
    }
    let (label, glyph) = if copied { ("Copied", icons::CHECK) } else { ("Copy link", icons::COPY) };
    let copy = Button::new(label).icon(glyph).size(Size::Sm).width(button_w).id_salt(("orbiters-home-copy", i));
    let mut clicked = None;
    if copy.show_at(ui, Rect::from_min_size(pos2(row.right() - button_w, row.center().y - 16.0), vec2(button_w, 32.0))).clicked() {
        clicked = Some(Target::Copy(i as u8));
    }
    if drop {
        let hint = "Drop it in the world as a card: whoever grabs it copies the link";
        let button = Button::new("Drop").icon(icons::HAND_DEPOSIT).size(Size::Sm).variant(Variant::Primary).width(96.0).hint(hint);
        let at = Rect::from_min_size(pos2(row.right() - button_w - 8.0 - 96.0, row.center().y - 16.0), vec2(96.0, 32.0));
        if button.id_salt(("orbiters-home-drop", i)).show_at(ui, at).clicked() {
            clicked = Some(Target::Drop(i as u8));
        }
    }
    clicked
}

/// A result's place while the search runs.
fn placeholder_row(ui: &mut Ui) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), THUMB.y + 28.0), Sense::hover());
    let p = ui.painter();
    let thumb = Rect::from_min_size(pos2(row.left(), row.center().y - THUMB.y / 2.0), THUMB);
    p.rect_filled(thumb, theme::radius(10.0), theme::surface_soft());
    let x = thumb.right() + 16.0;
    for (dy, w) in [(-14.0, ((row.right() - x) * 0.55).max(60.0)), (10.0, 120.0)] {
        p.rect_filled(Rect::from_min_size(pos2(x, row.center().y + dy - 5.0), vec2(w, 10.0)), theme::radius(5.0), theme::white(0.06));
    }
}

/// The results' pages: the one before, where it is, the next; the one pressed.
fn pager(ui: &mut Ui, menu: &Menu) -> Option<Target> {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
    let label = Txt::new(format!("Page {}", layout::text(menu, Content::PageLabel)), 12.5).color(theme::FG_3);
    widgets::text(ui.painter(), row.center(), Align2::CENTER_CENTER, &label);
    let mut pressed = None;
    for (target, glyph, name, dx, on) in [
        (Target::Previous, icons::CARET_LEFT, "Previous page", -70.0, menu.has_previous()),
        (Target::Next, icons::CARET_RIGHT, "Next page", 70.0, menu.has_next()),
    ] {
        if icon_button(ui, Rect::from_center_size(row.center() + vec2(dx, 0.0), Vec2::splat(36.0)), glyph, name, on) {
            pressed = Some(target);
        }
    }
    pressed
}

fn call(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let sub = if menu.calls_ready { layout::text(menu, Content::CallsTitle) } else { "Friends who are online, a press away".to_string() };
    top(ui, cx, "Call", &sub, acts);
    if let Some(call) = &menu.call {
        let ringing = matches!(call, Call::Ringing { .. });
        let name = layout::text(menu, Content::CallName);
        let state = layout::text(menu, Content::CallState);
        section(ui, cx, 1, icons::PHONE, &name, Some(&state), |ui| {
            ui.add_space(14.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
            let mut x = row.left();
            if ringing {
                let answer = Button::new("Answer").icon(icons::PHONE).variant(Variant::Primary);
                let w = answer.desired_width(ui);
                if answer.show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0))).clicked() {
                    acts.push(Act::Press(Target::Answer));
                }
                x += w + 8.0;
            }
            let end = Button::new(if ringing { "Decline" } else { "Hang up" }).icon(icons::PHONE_X).variant(Variant::Danger).id_salt("orbiters-home-hang-up");
            let w = end.desired_width(ui);
            if end.show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(w, 40.0))).clicked() {
                acts.push(Act::Press(Target::HangUp));
            }
            ui.add_space(8.0);
        });
        return;
    }
    if !menu.calls_ready {
        return note_card(ui, cx, 1, icons::PHONE, "Calls are not available", &menu.call_note);
    }
    if menu.contacts.is_empty() {
        return note_card(ui, cx, 1, icons::USERS, "No friend online", "Friends who can take a call show here as soon as they are online.");
    }
    section(ui, cx, 1, icons::USERS, "Friends", Some("Online, and able to take a call"), |ui| {
        ui.add_space(4.0);
        for (i, contact) in menu.contacts.iter().enumerate() {
            if i > 0 {
                widgets::separator(ui);
            }
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
            let call = Button::new("Call").icon(icons::PHONE).size(Size::Sm).id_salt(("orbiters-home-call", i));
            let call_w = call.desired_width(ui);
            {
                let p = ui.painter();
                let face = pos2(row.left() + 18.0, row.center().y);
                p.circle_filled(face, 18.0, theme::INK_750);
                widgets::icon(p, face, icons::USER, 17.0, theme::FG_3);
                let name = Txt::new(&contact.name, 13.5).weight(weight::MEDIUM);
                widgets::text_truncated(p, pos2(row.left() + 48.0, row.center().y), Align2::LEFT_CENTER, &name, row.width() - 48.0 - call_w - 16.0);
            }
            if call.show_at(ui, Rect::from_min_size(pos2(row.right() - call_w, row.center().y - 16.0), vec2(call_w, 32.0))).clicked() {
                acts.push(Act::Call(i));
            }
        }
    });
}

fn photo(ui: &mut Ui, cx: &Cx, menu: &Menu, blocked: Option<&str>, acts: &mut Vec<Act>) {
    top(ui, cx, "3D picture", &layout::text(menu, Content::PhotoStatus), acts);
    section(ui, cx, 1, icons::CUBE_FOCUS, "Camera", Some("The timer and the flight stay as chosen"), |ui| {
        let mut timer = menu.photo_timer;
        let timers: Vec<SegOption<u32>> = TIMERS.iter().zip(TIMER_LABELS).map(|(&t, label)| SegOption::new(t, label)).collect();
        widgets::field(ui, "Timer", Some("Time to get ready after Take."), 240.0, |ui| {
            if widgets::segmented(ui, "orbiters-home-timer", &mut timer, &timers, true) {
                if let Some(i) = TIMERS.iter().position(|&t| t == timer) {
                    acts.push(Act::Press(Target::Timer(i as u8)));
                }
            }
        });
        widgets::separator(ui);
        let mut length = menu.photo_length;
        let lengths: Vec<SegOption<PhotoLength>> = PhotoLength::ALL
            .iter()
            .zip(LENGTH_LABELS)
            .zip(super::capture::FLIGHT_HINTS)
            .map(|((&l, label), hint)| SegOption::new(l, label).hint(hint))
            .collect();
        widgets::field(ui, "Camera flight", Some("Longer flights see more of the place."), 200.0, |ui| {
            if widgets::segmented(ui, "orbiters-home-length", &mut length, &lengths, true) {
                if let Some(i) = PhotoLength::ALL.iter().position(|&l| l == length) {
                    acts.push(Act::Press(Target::Length(i as u8)));
                }
            }
        });
        widgets::separator(ui);
        ui.add_space(20.0);
        let label = match menu.countdown {
            Some(n) => format!("Starting in {n} s…"),
            None => "Take 3D picture".to_string(),
        };
        let mut take = Button::new(&label)
            .icon(icons::CUBE_FOCUS)
            .variant(Variant::Primary)
            .enabled(menu.countdown.is_none() && blocked.is_none())
            .id_salt("orbiters-home-take");
        if let Some(why) = blocked {
            take = take.hint(why);
        }
        let w = take.desired_width(ui).max(200.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        if take.width(w).show_at(ui, Rect::from_min_size(row.min, vec2(w, 40.0))).clicked() {
            acts.push(Act::Press(Target::Take));
        }
        ui.add_space(8.0);
    });
}

fn music(ui: &mut Ui, cx: &Cx, menu: &Menu, picture: &dyn Fn(&str) -> Option<TextureId>, acts: &mut Vec<Act>) {
    let media = menu.media.as_ref().filter(|m| !m.title.is_empty());
    let sub = match media {
        Some(_) => layout::text(menu, Content::MediaState),
        None => "What this computer plays, and its volume".to_string(),
    };
    top(ui, cx, "Music", &sub, acts);
    let from = media.map(|m| m.app.as_str()).filter(|app| !app.is_empty()).map(|app| format!("From {app}"));
    section(ui, cx, 1, icons::MUSIC_NOTES, "Now playing", from.as_deref(), |ui| {
        ui.add_space(14.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 108.0), Sense::hover());
        let art = Rect::from_min_size(row.min, vec2(192.0, 108.0));
        let controls_w = 40.0 + 10.0 + 48.0 + 10.0 + 40.0;
        {
            let p = ui.painter();
            match media.and_then(|m| m.art.as_deref()).and_then(picture) {
                Some(texture) => thumbnail(p, art, Some(texture), 12.0),
                None => {
                    thumbnail(p, art, None, 12.0);
                    widgets::icon(p, art.center(), icons::MUSIC_NOTES, 30.0, theme::FG_4);
                }
            }
            let x = art.right() + 20.0;
            let text_w = (row.right() - x - if media.is_some() { controls_w + 24.0 } else { 0.0 }).max(80.0);
            let lines = [
                (Content::MediaTitle, 26.0, Txt::new("", 17.0).weight(weight::SEMIBOLD)),
                (Content::MediaArtist, 52.0, Txt::new("", 13.0).color(theme::FG_2)),
                (Content::MediaAlbum, 74.0, Txt::new("", 12.5).color(theme::FG_3)),
            ];
            for (content, dy, style) in lines {
                let line = Txt { text: layout::text(menu, content), ..style };
                widgets::text_truncated(p, pos2(x, row.top() + dy), Align2::LEFT_CENTER, &line, text_w);
            }
        }
        if let Some(m) = media {
            let (cy, mut x) = (row.center().y, row.right() - controls_w);
            if icon_button(ui, Rect::from_min_size(pos2(x, cy - 20.0), Vec2::splat(40.0)), icons::SKIP_BACK, "Previous", m.can_previous) {
                acts.push(Act::Press(Target::MediaPrevious));
            }
            x += 50.0;
            let play = Button::new("")
                .icon(if m.playing { icons::PAUSE } else { icons::PLAY })
                .variant(Variant::Primary)
                .size(Size::Lg)
                .width(48.0)
                .enabled(m.can_play_pause)
                .hint(if m.playing { "Pause" } else { "Play" })
                .id_salt("orbiters-home-play");
            if play.show_at(ui, Rect::from_min_size(pos2(x, cy - 24.0), Vec2::splat(48.0))).clicked() {
                acts.push(Act::Press(Target::MediaPlayPause));
            }
            x += 58.0;
            if icon_button(ui, Rect::from_min_size(pos2(x, cy - 20.0), Vec2::splat(40.0)), icons::SKIP_FORWARD, "Next", m.can_next) {
                acts.push(Act::Press(Target::MediaNext));
            }
        }
        ui.add_space(14.0);
        if let Some(volume) = menu.volume {
            widgets::separator(ui);
            widgets::field(ui, "Volume", Some("This computer's speakers."), 340.0, |ui| {
                let mut percent = volume.percent as f32;
                let format = |v: f32| if volume.muted { "Muted".to_string() } else { format!("{v:.0} %") };
                if widgets::slider(ui, "orbiters-home-volume", &mut percent, 0.0..=100.0, 1.0, format) {
                    acts.push(Act::Volume(percent.round() as u8));
                }
                ui.add_space(6.0);
                let (glyph, label) = if volume.muted { (icons::SPEAKER_X, "Unmute") } else { (icons::SPEAKER_HIGH, "Mute") };
                if IconButton::new(glyph, label).show(ui).clicked() {
                    acts.push(Act::Press(Target::Mute));
                }
            });
        }
    });
}

fn light(ui: &mut Ui, cx: &Cx, menu: &Menu, acts: &mut Vec<Act>) {
    let Some(light) = menu.light else {
        top(ui, cx, "Light", "N4N0 as a light everyone around sees", acts);
        return note_card(ui, cx, 1, icons::LIGHTBULB, "No light on this N4N0", &layout::text(menu, Content::LightNote));
    };
    top(ui, cx, "Light", &layout::text(menu, Content::LightState), acts);
    section(ui, cx, 1, icons::LIGHTBULB, "N4N0's light", Some(&layout::text(menu, Content::LightNote)), |ui| {
        widgets::field(ui, "Light", None, 48.0, |ui| {
            let mut on = light.on;
            if widgets::toggle(ui, "orbiters-home-light", &mut on) {
                acts.push(Act::Press(Target::LightSwitch));
            }
        });
        widgets::separator(ui);
        let name = LIGHT_COLORS.get(light.color as usize).map_or("", |c| c.0);
        widgets::field(ui, "Colour", Some(name), swatches_width(), |ui| {
            if let Some(i) = swatches(ui, light.color) {
                acts.push(Act::Press(Target::LightColor(i)));
            }
        });
        widgets::separator(ui);
        let mut level = light.level_choice();
        let levels: Vec<SegOption<usize>> = LIGHT_LEVELS.iter().enumerate().map(|(i, (name, _))| SegOption::new(i, name)).collect();
        widgets::field(ui, "Brightness", None, 260.0, |ui| {
            if widgets::segmented(ui, "orbiters-home-light-level", &mut level, &levels, true) {
                acts.push(Act::Press(Target::LightLevel(level as u8)));
            }
        });
    });
}

const SWATCH: f32 = 26.0;
const SWATCH_GAP: f32 = 10.0;

fn swatches_width() -> f32 {
    LIGHT_COLORS.len() as f32 * (SWATCH + SWATCH_GAP) - SWATCH_GAP + 8.0
}

/// The light's colours as round swatches, the chosen one ringed; the one pressed (pressing turns the light on, the
/// chosen colour too).
fn swatches(ui: &mut Ui, chosen: u8) -> Option<u8> {
    let (rect, _) = ui.allocate_exact_size(vec2(swatches_width(), 36.0), Sense::hover());
    let ctx = ui.ctx().clone();
    let mut pressed = None;
    for (i, (name, [r, g, b])) in LIGHT_COLORS.iter().enumerate() {
        let center = pos2(rect.left() + 4.0 + SWATCH / 2.0 + i as f32 * (SWATCH + SWATCH_GAP), rect.center().y);
        let id = Id::unique(("orbiters-home-colour", i));
        let resp = ui.interact(Rect::from_center_size(center, Vec2::splat(SWATCH + 8.0)), id, Sense::click());
        if widgets::pressed(&resp) {
            pressed = Some(i as u8);
        }
        let on = anim::toggle(&ctx, id.with("on"), i as u8 == chosen, Spring::SNAPPY);
        let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let down = anim::toggle(&ctx, id.with("down"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let p = ui.painter();
        let ring = theme::mix(theme::white(0.18 * hover), theme::FG, on);
        if ring.a() > 0 {
            p.circle_stroke(center, SWATCH / 2.0 + 4.0, Stroke::new(2.0, ring));
        }
        p.circle_filled(center, SWATCH / 2.0 * (1.0 - 0.1 * down), Color32::from_rgb(*r, *g, *b));
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(*name);
    }
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Ctl};
    use crate::ipc::OrbCommand;
    use crate::state::{OrbLight, Snapshot};
    use egui::{Event, Modifiers, PointerButton, RawInput};
    use orbiters_ui::toast::Toasts;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// What Home asked of the app: settings changes and N4N0 commands.
    #[derive(Default)]
    struct Recorder {
        patches: Mutex<Vec<serde_json::Value>>,
        orb: Mutex<Vec<OrbCommand>>,
    }

    impl Backend for Recorder {
        fn orb(&self, command: OrbCommand) {
            self.orb.lock().unwrap().push(command);
        }
        fn patch_settings(&self, patch: serde_json::Value) {
            self.patches.lock().unwrap().push(patch);
        }
        fn view(&self) -> Arc<Snapshot> {
            Arc::new(Snapshot::new(Settings::default()))
        }
        fn take_notices(&self) -> Vec<crate::controller::Notice> {
            Vec::new()
        }
        fn library_version(&self) -> u64 {
            0
        }
        fn attach(&self, _ctx: &egui::Context) {}
        fn start_capture(&self) {}
        fn stop_capture(&self) {}
        fn pause_capture(&self, _paused: bool) {}
        fn start_test_capture(&self) {}
        fn import_raw_capture(&self, _dir: PathBuf) {}
        fn trash_session(&self, _dir: PathBuf) {}
        fn reconstruct(&self, _dir: PathBuf) {}
        fn stop_reconstruction(&self) {}
        fn unqueue_reconstruction(&self, _dir: PathBuf) {}
        fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
        fn sign_in(&self, _provider: &str) {}
        fn reopen_sign_in(&self) {}
        fn cancel_sign_in(&self) {}
        fn sign_out(&self) {}
        fn request_worker_log(&self) {}
        fn worker_log(&self) -> Option<Vec<String>> {
            None
        }
        fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
        fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
        fn request_maps(&self) {}
        fn maps(&self) -> Option<crate::state::MapsMine> {
            None
        }
        fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
        fn frames(&self, _chunk: crate::state::FrameChunk) {}
        fn headset(&self, _in_use: bool) {}
        fn walk_boost(&self, _strength: f32) {}
        fn osc_reconnect(&self) {}
        fn updates(&self, _action: crate::ipc::UpdateAction) {}
        fn quit(&self) {}
    }

    /// Home in a 1048 × 780 page, frame by frame: its cards are four to a row, 233 wide from x 32, 128 high from y 140
    /// (16 apart): fast walk, YouTube, Call, 3D picture, then music, light, States and what N4N0 says.
    struct Harness {
        ctx: egui::Context,
        recorder: Arc<Recorder>,
        ctl: Ctl,
        snap: Snapshot,
        toasts: Toasts,
        state: State,
        time: f64,
    }

    impl Harness {
        /// N4N0's light on, fast walk off.
        fn new() -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let recorder = Arc::new(Recorder::default());
            let ctl: Ctl = recorder.clone();
            let mut snap = Snapshot::new(Settings::default());
            snap.orb.light = Some(OrbLight { on: true, color: 1, level: 0.6 });
            Self { ctx, recorder, ctl, snap, toasts: Toasts::default(), state: State::default(), time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input =
                RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1048.0, 780.0))), time: Some(self.time), events, ..Default::default() };
            let (ctl, snap, toasts, state) = (&self.ctl, &self.snap, &mut self.toasts, &mut self.state);
            let mut output = self.ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl, snap, toasts: &mut *toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0 };
                show(ui, &mut cx, state);
            });
            // Nothing paints these frames.
            output.textures_delta.clear();
        }

        /// Frames until what entered stands still.
        fn settle(&mut self) {
            for _ in 0..90 {
                self.frame(Vec::new());
            }
        }

        fn press(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerMoved(at), button(at, true)]);
        }

        fn release(&mut self, at: Pos2) {
            self.frame(vec![button(at, false)]);
        }

        fn page(&self) -> Page {
            self.state.host.as_ref().map_or(Page::Home, |host| host.menu.page)
        }

        fn menu(&self) -> &Menu {
            &self.state.host.as_ref().expect("Home shown").menu
        }
    }

    fn button(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn key(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    #[test]
    fn a_card_opens_its_page_on_the_press_and_home_comes_back() {
        let mut h = Harness::new();
        h.settle();
        // YouTube's card, the second of the first row: its page on the press, and the search field takes the keys.
        let youtube = pos2(397.0, 204.0);
        h.press(youtube);
        assert_eq!(h.page(), Page::Search, "open on the press, before the release");
        h.release(youtube);
        h.settle();
        h.frame(vec![Event::Text("lofi".into())]);
        assert_eq!(h.menu().query, "lofi", "typed into the menu's field");
        h.frame(vec![key(Key::Escape)]);
        assert_eq!(h.page(), Page::Home);
        h.settle();

        // The light's card (away from its switch) opens the light's page; its Home button comes back on the release.
        h.press(pos2(400.0, 380.0));
        h.release(pos2(400.0, 380.0));
        assert_eq!(h.page(), Page::Light);
        h.settle();
        let back = pos2(60.0, 64.0);
        h.press(back);
        assert_eq!(h.page(), Page::Light, "a button acts on its release");
        h.release(back);
        assert_eq!(h.page(), Page::Home);
    }

    #[test]
    fn a_switch_acts_alone_and_shows_its_change_at_once() {
        let mut h = Harness::new();
        h.settle();
        // Fast walk's switch: one change, saved as the setting; the card under it does nothing more.
        let walk = pos2(217.0, 180.0);
        h.press(walk);
        assert!(h.menu().fast_walk, "on at once");
        h.release(walk);
        let patches = h.recorder.patches.lock().unwrap().clone();
        assert_eq!(patches.len(), 1, "{patches:?}");
        assert_eq!(patches[0]["walk_boost"]["enabled"], serde_json::json!(true));
        assert_eq!(h.page(), Page::Home);

        // The light's switch: off at once, and it stays off while the service has not said so yet.
        let light = pos2(466.0, 324.0);
        h.press(light);
        h.release(light);
        h.frame(Vec::new());
        assert!(matches!(h.recorder.orb.lock().unwrap().as_slice(), [OrbCommand::Light { on: false, .. }]));
        assert!(!h.menu().light.expect("a light").on, "as switched, though the service still says on");
        assert_eq!(h.page(), Page::Home, "its card did not open the light's page");
    }

    #[test]
    fn the_states_page_saves_and_its_default_acts_on_the_press() {
        use crate::state::{StateView, StatesView};
        widgets::watch_controls();
        let mut h = Harness::new();
        h.snap.orb.states = StatesView {
            avatar_id: Some("avtr_a".into()),
            avatar_name: Some("Rexouium".into()),
            states: vec![StateView {
                id: "s1".into(),
                name: "Hoodie on".into(),
                parameters: 10,
                pose_parameters: 2,
                eye_height: Some(1.6),
                ..Default::default()
            }],
            ..Default::default()
        };
        h.settle();
        // The States card, the third of the second row: its page on the press.
        let card = pos2(646.0, 348.0);
        h.press(card);
        assert_eq!(h.page(), Page::States, "open on the press");
        h.release(card);
        h.settle();
        // Set as default is a choice: asked on the press, before the release.
        let (default, _) = widgets::control(&h.ctx, "Set as default").expect("drawn");
        h.press(default.center());
        assert!(
            matches!(h.recorder.orb.lock().unwrap().last(), Some(OrbCommand::States { action: StateAction::Default { id: Some(id) } }) if id == "s1"),
            "{:?}",
            h.recorder.orb.lock().unwrap()
        );
        h.release(default.center());
        h.frame(Vec::new());
        assert!(widgets::control(&h.ctx, "Default").is_some(), "marked at once, before the service says so");
        // Save with no name: the next "State n".
        let (save, _) = widgets::control(&h.ctx, "Save").expect("drawn");
        h.press(save.center());
        h.release(save.center());
        assert!(matches!(
            h.recorder.orb.lock().unwrap().last(),
            Some(OrbCommand::States { action: StateAction::Save { name: Some(name) } }) if name == "State 2"
        ));
    }
}
