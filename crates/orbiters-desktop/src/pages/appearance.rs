//! Settings, Appearance: a miniature of the app as it looks now, then its colours, its font and its background, each choice one
//! press and drawn as what it gives. Everything applies as it is chosen; one button puts Orbiters' own look back.

use super::{section_up_to, Cx, Job};
use crate::appearance::{self, Palette, ACCENTS, BACKGROUNDS, PALETTES, TINTS};
use egui::{pos2, vec2, Align2, Color32, ColorImage, Id, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use orbiters_core::appearance::{AppearanceSettings, Background, BackgroundColors, InterfaceSize, Tint};
use orbiters_core::settings::Settings;
use orbiters_i18n::{mark, tr, tr_ctx};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::background::{self, Backdrop, Laid, Scene, Time};
use orbiters_ui::faces::{self, BUNDLED, INSTALLED};
use orbiters_ui::look::{self, Look, Oklch};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Variant};
use orbiters_ui::{icons, n4n0_logo, paint};
use std::collections::HashMap;
use std::sync::OnceLock;

// The cards.
pub const PREVIEW: &str = mark("Preview");
pub const COLOURS: &str = mark("Colors");
pub const FONT: &str = mark("Font");
pub const BACKGROUND: &str = mark("Background");

// What the registry calls the controls that have no label of their own.
pub const RESET: &str = mark("Back to Orbiters' look");
pub const OWN_FONT: &str = mark("A font on this computer");

/// The website's specimen: the creator fonts draw Latin letters, so it is the same in every language.
const SPECIMEN: &str = "Your world, in your words.";

/// How many of this computer's fonts show before more are asked for.
const FONTS_FIRST: usize = 12;

#[derive(Default)]
pub struct State {
    /// The accent as typed, until it is a colour or left.
    hex: Option<String>,
    /// The hue held on the strip: greys have none of their own, so it stays where it was put.
    hue: Option<f32>,
    /// This computer's fonts, once asked for.
    fonts: Option<Job<Vec<(String, String)>>>,
    /// Their names, and where each one's specimen is read from.
    families: Option<Vec<(String, String)>>,
    fonts_open: bool,
    search: String,
    shown: usize,
    /// Specimens: a font's key to its picture, or that it has none.
    specimens: HashMap<String, Specimen>,
}

enum Specimen {
    Coming(Job<Option<ColorImage>>),
    Ready(egui::TextureHandle),
    None,
}

impl State {
    /// Screenshots: this computer's fonts shown as a list of these.
    pub(crate) fn stage_fonts(&mut self, names: &[&str]) {
        self.families = Some(names.iter().map(|n| (n.to_string(), String::new())).collect());
        self.fonts_open = true;
    }
}

/// The sub-tab's cards; where each went is noted in `cards`. Whether the settings changed.
pub fn cards(ui: &mut Ui, cx: &mut Cx, state: &mut State, draft: &mut Settings, cards: &mut Vec<(&'static str, Rect)>) -> bool {
    let mut changed = false;
    let width = ui.available_width().min(1180.0);
    let mut card = |ui: &mut Ui, index: usize, title: &'static str, glyph: &str, about: &str, body: &mut dyn FnMut(&mut Ui)| {
        let r = ui.scope(|ui| section_up_to(ui, cx, width, index, glyph, tr(title), Some(about), |ui| body(ui)));
        cards.push((title, r.response.rect));
    };
    let mut asked = false;
    card(ui, 1, PREVIEW, icons::EYE, tr("Everything here applies at once, in the window and in the headset"), &mut |ui| asked |= preview(ui, draft));
    if asked {
        draft.appearance.reset();
        draft.orbit.accent_own = false;
        draft.orbit.accent = orbiters_core::settings::OrbitSettings::FIRST_ACCENT;
        (state.hex, state.hue) = (None, None);
        changed = true;
    }
    let reduced = draft.reduce_motion;
    let orbit = &mut draft.orbit;
    let a = &mut draft.appearance;
    card(ui, 2, COLOURS, icons::PALETTE, tr("Text stays readable whatever you pick, and green, amber and red keep their meaning"), &mut |ui| changed |= colours(ui, state, a, orbit));
    card(ui, 3, FONT, icons::TEXT_T, tr("Letters a font does not have, Japanese for one, come from the default"), &mut |ui| changed |= font(ui, state, a));
    card(ui, 4, BACKGROUND, icons::IMAGE, tr("Behind the window's pages: the backgrounds of profiles on orbiters.cc"), &mut |ui| changed |= backgrounds(ui, a, reduced));
    changed
}

// --- What every choice here is made of ---

/// A choice at `rect`: pressed or reached with the keyboard, its ring while it is the one, a fainter one under the pointer, a
/// clear one while it has the keys. `key` is what the settings' registry knows it by.
fn choice(ui: &mut Ui, key: &'static str, n: usize, rect: Rect, radius: f32, chosen: bool, name: &str) -> (Response, bool) {
    let id = ui.scope_id().with((key, n));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    widgets::mark_control(key, "tile", rect);
    let label = name.to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, chosen, &label));
    let ctx = ui.ctx().clone();
    let on = anim::toggle(&ctx, id.with("on"), chosen, Spring::SNAPPY);
    let over = anim::toggle(&ctx, id.with("over"), resp.hovered(), Spring::SNAPPY);
    let p = ui.painter();
    if over > 0.01 && on < 0.99 {
        p.rect_stroke(rect.expand(2.0), theme::radius(radius + 2.0), Stroke::new(1.5, theme::white(0.22 * over * (1.0 - on))), StrokeKind::Middle);
    }
    if on > 0.01 {
        p.rect_stroke(rect.expand(3.0), theme::radius(radius + 3.0), Stroke::new(2.0, theme::alpha(theme::accent(), on)), StrokeKind::Middle);
    }
    if resp.has_focus() {
        p.rect_stroke(rect.expand(6.0), theme::radius(radius + 6.0), Stroke::new(2.0, theme::fg()), StrokeKind::Middle);
    }
    let pressed = widgets::activated(&resp);
    (resp, pressed && !chosen)
}

/// A grid of `count` tiles (the choices of `key`) at least `min` wide and `h` high, over the rest of the width.
fn grid(ui: &mut Ui, key: &str, count: usize, min: f32, h: f32, gap: f32, mut tile: impl FnMut(&mut Ui, usize, Rect)) {
    let w = ui.available_width();
    let n = (((w + gap) / (min + gap)).floor() as usize).clamp(1, count.max(1));
    let each = (w - gap * (n - 1) as f32) / n as f32;
    let rows = count.div_ceil(n);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * h + rows.saturating_sub(1) as f32 * gap), Sense::hover());
    for i in 0..count {
        let r = Rect::from_min_size(area.min + vec2((i % n) as f32 * (each + gap), (i / n) as f32 * (h + gap)), vec2(each, h));
        if ui.is_rect_visible(r) {
            tile(ui, i, r);
        } else {
            widgets::mark_control(key, "tile", r);
        }
    }
}

/// A row's name and what it says, above what fills the card's width under them.
fn heading(ui: &mut Ui, label: &str, about: Option<&str>) {
    widgets::field(ui, label, about, 0.0, |_| {});
}

// --- The preview ---

/// The app in miniature, over the background. Whether Orbiters' look was asked back.
fn preview(ui: &mut Ui, draft: &Settings) -> bool {
    let a = &draft.appearance;
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 228.0), Sense::hover());
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    p.rect_filled(r, theme::radius(16.0), theme::ink_950());
    let b = appearance::backdrop(a);
    if b.scene == Scene::None {
        glow(&p, r);
    } else {
        background::paint(&p, Id::unique("appearance-preview"), r, &b, Time::STILL, Laid { veil: b.under(&theme::tokens()).0, radius: 16.0, ..Laid::PAGE });
    }
    let over = b.scene != Scene::None;
    theme::with_background(over, || miniature(&p, r));
    ui.add_space(10.0);
    let mut reset = false;
    widgets::field(ui, tr(RESET), Some(tr("The colors, the font, the size and the background, as they were.")), 120.0, |ui| {
        let can = !a.is_orbiters() || draft.orbit.has_own_accent();
        reset = Button::new(tr("Reset")).icon(icons::ARROW_COUNTER_CLOCKWISE).size(Size::Sm).enabled(can).show(ui).clicked();
    });
    reset
}

/// Orbiters' own glow, still.
fn glow(p: &Painter, r: Rect) {
    let blob = |at: egui::Pos2, size: f32, c: Color32| paint::shadow(&p.with_clip_rect(r.intersect(p.clip_rect())), Rect::from_center_size(at, vec2(size, size * 0.6)), size * 0.3, vec2(0.0, 0.0), size * 0.55, 0.0, c);
    // Clear of the corners, which are round.
    blob(pos2(r.left() + r.width() * 0.3, r.center().y - r.height() * 0.1), r.height() * 0.9, theme::alpha(theme::PURPLE, 0.2));
    blob(pos2(r.right() - r.width() * 0.26, r.center().y + r.height() * 0.06), r.height() * 0.8, theme::alpha(theme::BLUE, 0.15));
}

fn miniature(p: &Painter, r: Rect) {
    // The sidebar.
    let side = Rect::from_min_size(r.min, vec2((r.width() * 0.24).clamp(120.0, 190.0), r.height()));
    let mark_r = Rect::from_min_size(side.min + vec2(18.0, 18.0), n4n0_logo::size_for_width(34.0));
    n4n0_logo::paint(p, mark_r, theme::mark());
    let nav = [(icons::HOUSE, tr("Home")), (icons::IMAGES, tr("Photos & captures")), (icons::CUBE, tr("Unity projects")), (icons::SLIDERS_HORIZONTAL, tr("Settings"))];
    for (i, (glyph, label)) in nav.into_iter().enumerate() {
        let row = Rect::from_min_size(pos2(side.left() + 10.0, side.top() + 72.0 + i as f32 * 36.0), vec2(side.width() - 14.0, 32.0));
        let on = i == 0;
        if on {
            p.rect_filled(row, theme::radius(10.0), theme::white(0.07));
            p.rect_filled(Rect::from_center_size(pos2(row.left() + 1.5, row.center().y), vec2(3.0, 16.0)), theme::radius(1.5), theme::accent());
        }
        let c = if on { theme::fg() } else { theme::fg_3() };
        widgets::icon(p, pos2(row.left() + 18.0, row.center().y), glyph, 14.0, if on { theme::accent() } else { c });
        widgets::text_truncated(p, pos2(row.left() + 34.0, row.center().y), Align2::LEFT_CENTER, &Txt::new(label, 12.0).weight(weight::MEDIUM).color(c), row.width() - 40.0);
    }
    // A page.
    let x = side.right() + 14.0;
    let right = r.right() - 20.0;
    let top = r.top() + 20.0;
    widgets::text(p, pos2(x, top + 6.0), Align2::LEFT_CENTER, &Txt::caps(tr("Preview")));
    widgets::text_truncated(p, pos2(x, top + 30.0), Align2::LEFT_CENTER, &Txt::new("Orbiters - N4N0", 22.0).weight(weight::SEMIBOLD).tracking(-0.01), right - x - 150.0);
    let play = Rect::from_min_size(pos2(right - 130.0, top + 10.0), vec2(130.0, 34.0));
    p.rect_filled(play, theme::radius(17.0), theme::accent());
    widgets::text_truncated(p, play.center(), Align2::CENTER_CENTER, &Txt::new(tr("Main action"), 12.5).weight(weight::SEMIBOLD).color(theme::on_accent()), play.width() - 20.0);
    // A card, with a row of a list, a switch and a second button.
    let card = Rect::from_min_max(pos2(x, top + 62.0), pos2(right, r.bottom() - 18.0));
    paint::fill_squircle(p, card, theme::squircle(14.0), theme::surface());
    let c = card.shrink2(vec2(18.0, 14.0));
    widgets::text_truncated(p, pos2(c.left(), c.top() + 8.0), Align2::LEFT_CENTER, &Txt::new(tr("A card, as on every page"), 14.0).weight(weight::SEMIBOLD), c.width() - 70.0);
    widgets::text_truncated(p, pos2(c.left(), c.top() + 29.0), Align2::LEFT_CENTER, &Txt::new(tr("The text you read, and quieter text beside it."), 12.5).color(theme::fg_2()), c.width() - 60.0);
    let track = Rect::from_min_size(pos2(c.right() - 38.0, c.top() + 2.0), vec2(38.0, 22.0));
    p.rect_filled(track, theme::radius(11.0), theme::accent());
    p.circle_filled(pos2(track.right() - 11.0, track.center().y), 8.0, Color32::WHITE);
    p.hline(c.x_range(), c.top() + 50.0, Stroke::new(1.0, theme::white(0.06)));
    let y = c.top() + 80.0;
    let face = Rect::from_center_size(pos2(c.left() + 18.0, y), vec2(36.0, 36.0));
    widgets::avatar(p, face, None, "Aya");
    p.circle_filled(face.center() + vec2(13.0, 13.0), 6.5, theme::ink_850());
    p.circle_filled(face.center() + vec2(13.0, 13.0), 4.5, theme::OK);
    widgets::text_truncated(p, pos2(face.right() + 12.0, y - 9.0), Align2::LEFT_CENTER, &Txt::new("Aya", 13.5).weight(weight::MEDIUM), c.width() * 0.4);
    widgets::text_truncated(p, pos2(face.right() + 12.0, y + 9.0), Align2::LEFT_CENTER, &Txt::new(tr("Online, whatever the accent"), 11.5).color(theme::fg_3()), c.width() * 0.5);
    let second = Rect::from_center_size(pos2(c.right() - 52.0, y), vec2(104.0, 30.0));
    p.rect_filled(second, theme::radius(15.0), theme::control());
    widgets::text_truncated(p, second.center(), Align2::CENTER_CENTER, &Txt::new(tr("Another"), 12.0).weight(weight::MEDIUM), second.width() - 16.0);
}

// --- Colours ---

fn colours(ui: &mut Ui, state: &mut State, a: &mut AppearanceSettings, orbit: &mut orbiters_core::settings::OrbitSettings) -> bool {
    let mut changed = false;
    heading(ui, tr("Palette"), Some(tr("The surfaces, the accent and the background's colors together.")));
    let mut picked: Option<&Palette> = None;
    grid(ui, "appearance-palette", PALETTES.len(), 150.0, 104.0, 14.0, |ui, i, r| {
        let palette = &PALETTES[i];
        let chosen = palette.is(a);
        let art = Rect::from_min_size(r.min, vec2(r.width(), 74.0));
        let (_, pressed) = choice(ui, "appearance-palette", i, art, 14.0, chosen, tr(palette.name));
        theme::in_tokens(&palette_tokens()[i], || palette_art(ui.painter(), art));
        named(ui.painter(), r, art, tr(palette.name), chosen);
        if pressed {
            picked = Some(palette);
        }
    });
    if let Some(palette) = picked {
        palette.apply(a);
        (state.hex, state.hue) = (None, None);
        changed = true;
    }
    ui.add_space(14.0);
    widgets::separator(ui);

    // The accent: some to press, then any.
    let wide = ACCENTS.len() as f32 * 38.0;
    widgets::field(ui, tr("Accent"), Some(tr("The main action, what is selected, and switches that are on.")), wide, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(wide, 36.0), Sense::hover());
        for (i, c) in ACCENTS.into_iter().enumerate() {
            let dot = Rect::from_center_size(pos2(r.left() + 19.0 + i as f32 * 38.0, r.center().y), vec2(26.0, 26.0));
            let color = Color32::from_rgb(c[0], c[1], c[2]);
            let name = look::hex(color);
            let (resp, pressed) = choice(ui, "appearance-accent", i, dot, 13.0, a.accent == c, &name);
            resp.on_hover_text(name);
            ui.painter().circle_filled(dot.center(), 13.0, color);
            if a.accent == c {
                widgets::icon(ui.painter(), dot.center(), icons::CHECK, 13.0, ink_on(color));
            }
            if pressed {
                a.accent = c;
                (state.hex, state.hue) = (None, None);
                changed = true;
            }
        }
    });
    changed |= own_accent(ui, state, a);
    widgets::separator(ui);

    // The surfaces.
    let wide = TINTS.len() as f32 * 46.0;
    let about = tr!("What the greys lean to. {name} now.", name = tint_name(a.tint));
    widgets::field(ui, tr("Surfaces"), Some(&about), wide, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(wide, 40.0), Sense::hover());
        for (i, t) in TINTS.into_iter().enumerate() {
            let tile = Rect::from_center_size(pos2(r.left() + 23.0 + i as f32 * 46.0, r.center().y), vec2(34.0, 34.0));
            let (resp, pressed) = choice(ui, "appearance-tint", i, tile, 10.0, a.tint == t, tint_name(t));
            resp.on_hover_text(tint_name(t));
            let tokens = &tint_tokens()[i];
            let p = ui.painter();
            p.rect_filled(tile, theme::radius(10.0), tokens.ink_950);
            p.rect_filled(tile.shrink(6.0), theme::radius(6.0), tokens.ink_800);
            p.rect_filled(tile.shrink(12.0), theme::radius(3.0), tokens.fg_3);
            p.rect_stroke(tile, theme::radius(10.0), Stroke::new(1.0, theme::white(0.14)), StrokeKind::Inside);
            if pressed {
                a.tint = t;
                changed = true;
            }
        }
    });

    if orbiters_core::here::HERE.around() {
        widgets::separator(ui);
        let mut own = orbit.has_own_accent();
        let options = [SegOption::new(false, tr("The app's accent")), SegOption::new(true, tr("Their own"))];
        widgets::field(ui, tr("Menus in VRChat"), Some(tr("The color of Orbiters' panels beside VRChat's menus. Their own is picked in the headset, on the panels' settings.")), 290.0, |ui| {
            if widgets::segmented(ui, "appearance-menus", &mut own, &options, true) {
                orbit.accent_own = own;
                if !own {
                    orbit.accent = orbiters_core::settings::OrbitSettings::FIRST_ACCENT;
                }
                changed = true;
            }
        });
    }
    changed
}

/// The palettes' colours, and the surfaces', worked out once.
fn palette_tokens() -> &'static [look::Tokens] {
    static TOKENS: OnceLock<Vec<look::Tokens>> = OnceLock::new();
    TOKENS.get_or_init(|| PALETTES.iter().map(|p| look::Tokens::of(&p.look())).collect())
}

fn tint_tokens() -> &'static [look::Tokens] {
    static TOKENS: OnceLock<Vec<look::Tokens>> = OnceLock::new();
    TOKENS.get_or_init(|| TINTS.iter().map(|t| look::Tokens::of(&Look { tint: appearance::tint(*t), accent: theme::base::ACCENT })).collect())
}

/// The hue strip's colour at `h` degrees, one of those that can be read on every surface.
fn strip_color(h: f32) -> Color32 {
    Oklch { l: 0.8, c: 0.145, h }.color()
}

const STRIP: usize = 48;

fn strip() -> &'static [Color32; STRIP] {
    static STRIP_COLORS: OnceLock<[Color32; STRIP]> = OnceLock::new();
    STRIP_COLORS.get_or_init(|| std::array::from_fn(|i| strip_color(i as f32 / STRIP as f32 * 360.0)))
}

/// What is drawn on a swatch of `c`.
fn ink_on(c: Color32) -> Color32 {
    if look::luminance(c) > 0.2 {
        theme::base::ON_ACCENT
    } else {
        Color32::WHITE
    }
}

/// A tile's name under its picture, the chosen one's brighter.
fn named(p: &Painter, r: Rect, art: Rect, name: &str, chosen: bool) {
    let c = if chosen { theme::fg() } else { theme::fg_2() };
    widgets::text_truncated(p, pos2(r.left() + 4.0, art.bottom() + 17.0), Align2::LEFT_CENTER, &Txt::new(name, 12.5).weight(weight::MEDIUM).color(c), r.width() - 8.0);
}

/// A palette as the app looks in it: its page, a card, two lines, a button.
fn palette_art(p: &Painter, r: Rect) {
    p.rect_filled(r, theme::radius(14.0), theme::ink_950());
    p.rect_stroke(r, theme::radius(14.0), Stroke::new(1.0, theme::white(0.1)), StrokeKind::Inside);
    let card = Rect::from_min_max(r.min + vec2(r.width() * 0.2, 14.0), r.max - vec2(12.0, 12.0));
    for (i, c) in [theme::accent(), theme::fg_4(), theme::fg_4()].into_iter().enumerate() {
        p.rect_filled(Rect::from_min_size(pos2(r.left() + 12.0, r.top() + 16.0 + i as f32 * 14.0), vec2(r.width() * 0.2 - 22.0, 5.0)), theme::radius(2.5), c);
    }
    p.rect_filled(card, theme::radius(9.0), theme::ink_800());
    let inner = card.shrink(10.0);
    p.rect_filled(Rect::from_min_size(inner.min, vec2(inner.width() * 0.62, 6.0)), theme::radius(3.0), theme::fg());
    p.rect_filled(Rect::from_min_size(inner.min + vec2(0.0, 12.0), vec2(inner.width() * 0.44, 5.0)), theme::radius(2.5), theme::fg_3());
    let pill = Rect::from_min_size(pos2(inner.right() - 40.0, inner.bottom() - 13.0), vec2(40.0, 13.0));
    p.rect_filled(pill, theme::radius(6.5), theme::accent());
    p.rect_filled(Rect::from_center_size(pill.center(), vec2(18.0, 3.0)), theme::radius(1.5), theme::on_accent());
}

/// Any accent: a hue, how vivid, or its code. The hue strip only has colours that can be read; a code that cannot is used
/// lightened, and the row says so.
fn own_accent(ui: &mut Ui, state: &mut State, a: &mut AppearanceSettings) -> bool {
    let mut changed = false;
    let set = Color32::from_rgb(a.accent[0], a.accent[1], a.accent[2]);
    let used = look::passing(set, appearance::tint(a.tint));
    let note = if used == set {
        tr("Drag along the colors, or type a code.").to_string()
    } else {
        tr!("{code} is too dark to read here: {used} is used, the nearest that can be.", code = look::hex(set), used = look::hex(used))
    };
    let preset = ACCENTS.contains(&a.accent);
    widgets::field(ui, tr("Your own accent"), Some(&note), 380.0, |ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
        let mut typed = state.hex.clone().unwrap_or_else(|| look::hex(set));
        let resp = widgets::text_field(ui, "appearance-accent-code", &mut typed, "#00da6d", 104.0, true);
        if resp.changed() {
            if let Some(c) = look::parse_hex(&typed) {
                a.accent = [c.r(), c.g(), c.b()];
                state.hue = None;
                changed = true;
            }
            state.hex = Some(typed);
        } else if !resp.has_focus() {
            state.hex = None;
        }
        // What it gives, beside the code.
        let (chip, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
        ui.painter().circle_filled(chip.center(), 13.0, used);
        if !preset {
            ui.painter().circle_stroke(chip.center(), 16.0, Stroke::new(2.0, theme::accent()));
        }
        let (bar, _) = ui.allocate_exact_size(vec2(216.0, 36.0), Sense::hover());
        let id = ui.scope_id().with("appearance-hue");
        let resp = ui.interact(bar, id, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
        widgets::mark_control("appearance-hue", "slider", bar);
        resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(Oklch::of(set).h), tr("Your own accent")));
        let track = Rect::from_center_size(bar.center(), vec2(bar.width(), 16.0));
        let colors = strip();
        for (x, i) in [(track.left() + 8.0, 0), (track.right() - 8.0, 0)] {
            ui.painter().circle_filled(pos2(x, track.center().y), 8.0, colors[i]);
        }
        // One mesh, the colours running into each other.
        let mut mesh = egui::Mesh::default();
        for i in 0..=STRIP {
            let x = track.left() + 8.0 + i as f32 / STRIP as f32 * (track.width() - 16.0);
            let c = colors[i % STRIP];
            mesh.colored_vertex(pos2(x, track.top()), c);
            mesh.colored_vertex(pos2(x, track.bottom()), c);
            if i > 0 {
                let n = i as u32 * 2;
                mesh.add_triangle(n - 2, n - 1, n);
                mesh.add_triangle(n - 1, n + 1, n);
            }
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        let mut hue = state.hue.unwrap_or_else(|| Oklch::of(set).h);
        let mut moved = false;
        if let (true, Some(pos)) = (resp.is_pointer_button_down_on() || resp.dragged(), resp.interact_pointer_pos()) {
            hue = ((pos.x - track.left() - 8.0) / (track.width() - 16.0)).clamp(0.0, 1.0) * 359.0;
            moved = true;
        }
        if resp.has_focus() {
            let step = ui.input_mut(|i| {
                let by = |i: &mut egui::InputState, k| i.count_and_consume_key(egui::Modifiers::NONE, k) as f32;
                (by(i, egui::Key::ArrowRight) - by(i, egui::Key::ArrowLeft)) * 4.0
            });
            if step != 0.0 {
                hue = (hue + step).rem_euclid(360.0);
                moved = true;
            }
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { horizontal_arrows: true, ..Default::default() }));
            ui.painter().rect_stroke(track.expand(4.0), theme::radius(12.0), Stroke::new(2.0, theme::fg()), StrokeKind::Middle);
        }
        if moved {
            let c = strip_color(hue);
            state.hue = Some(hue);
            state.hex = None;
            let c = [c.r(), c.g(), c.b()];
            changed |= a.accent != c;
            a.accent = c;
        }
        let knob = pos2(track.left() + 8.0 + hue / 359.0 * (track.width() - 16.0), track.center().y);
        ui.painter().circle_filled(knob, 11.0, theme::ink_950());
        ui.painter().circle_filled(knob, 8.5, used);
    });
    changed
}

fn tint_name(t: Tint) -> &'static str {
    tr_ctx(
        "surfaces",
        match t {
            Tint::Zinc => mark("Zinc"),
            Tint::Graphite => mark("Graphite"),
            Tint::Slate => mark("Slate"),
            Tint::Moss => mark("Moss"),
            Tint::Stone => mark("Stone"),
            Tint::Plum => mark("Plum"),
            Tint::Black => mark("Black"),
        },
    )
}

// --- Font ---

fn font(ui: &mut Ui, state: &mut State, a: &mut AppearanceSettings) -> bool {
    let mut changed = false;
    ui.add_space(8.0);
    let mut picked: Option<String> = None;
    grid(ui, "appearance-font", BUNDLED.len() + 1, 220.0, 78.0, 12.0, |ui, i, r| {
        let (key, name) = match i.checked_sub(1).map(|k| &BUNDLED[k]) {
            None => (String::new(), tr("Orbiters' own").to_string()),
            Some(b) => (b.id.to_string(), b.name.to_string()),
        };
        if specimen_tile(ui, state, "appearance-font", i, r, &key, &name, a.font == key) {
            picked = Some(key);
        }
    });
    ui.add_space(12.0);
    widgets::separator(ui);

    // This computer's.
    let installed = a.font.strip_prefix(INSTALLED).map(str::to_string);
    let about = match &installed {
        Some(family) => tr!("{font} now. It is drawn at Orbiters' size, so pages keep their layout.", font = family),
        None => tr("Any font installed here, drawn at Orbiters' size so pages keep their layout.").to_string(),
    };
    widgets::field(ui, tr(OWN_FONT), Some(&about), 160.0, |ui| {
        let label = if state.fonts_open { tr("Hide") } else { tr("Choose a font") };
        if Button::new(label).icon(if state.fonts_open { icons::CARET_UP } else { icons::CARET_DOWN }).size(Size::Sm).show(ui).clicked() {
            state.fonts_open = !state.fonts_open;
        }
    });
    if state.fonts_open {
        if let Some(family) = own_fonts(ui, state, installed.as_deref()) {
            picked = Some(format!("{INSTALLED}{family}"));
        }
    }
    if let Some(key) = picked {
        a.font = key;
        changed = true;
    }
    widgets::separator(ui);
    let sizes = [SegOption::new(InterfaceSize::Small, tr_ctx("size", "Small")), SegOption::new(InterfaceSize::Default, tr_ctx("size", "Default")), SegOption::new(InterfaceSize::Large, tr_ctx("size", "Large"))];
    widgets::field(ui, tr("Size"), Some(tr("Of the text and everything in the window.")), 260.0, |ui| {
        changed |= widgets::segmented(ui, "appearance-size", &mut a.size, &sizes, true);
    });
    changed
}

/// The computer's fonts, found by a thread, as rows of the page: a search, then the first of them. The family pressed.
fn own_fonts(ui: &mut Ui, state: &mut State, chosen: Option<&str>) -> Option<String> {
    if state.families.is_none() {
        let job = state.fonts.get_or_insert_with(|| {
            Job::spawn(ui.ctx(), || {
                let path = |f: &faces::Installed| f.regular().map(|(p, i)| format!("{i}:{}", p.display())).unwrap_or_default();
                faces::installed().iter().map(|f| (f.family.clone(), path(f))).collect()
            })
        });
        match job.take() {
            Some(found) => state.families = Some(found),
            None => {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                widgets::text(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &Txt::new(tr("Reading this computer's fonts…"), 12.5).color(theme::fg_3()));
                return None;
            }
        }
    }
    let width = ui.available_width();
    if widgets::search_field_clearable(ui, "appearance-font-search", &mut state.search, tr("Search this computer's fonts…"), width).changed() {
        state.shown = FONTS_FIRST;
    }
    ui.add_space(12.0);
    let wanted = orbiters_core::search::fold(&state.search);
    let families = state.families.take().unwrap_or_default();
    let mut found: Vec<&(String, String)> = families.iter().filter(|(f, _)| orbiters_core::search::fold(f).contains(&wanted)).collect();
    // The one chosen comes first, until something is searched.
    if wanted.is_empty() {
        found.sort_by_key(|(f, _)| Some(f.as_str()) != chosen);
    }
    let shown = found.len().min(state.shown.max(FONTS_FIRST));
    let mut picked = None;
    if found.is_empty() {
        widgets::text(ui.painter(), ui.cursor().min + vec2(0.0, 12.0), Align2::LEFT_CENTER, &Txt::new(tr("No font of that name here."), 12.5).color(theme::fg_3()));
        ui.add_space(28.0);
    }
    grid(ui, "appearance-installed", shown, 220.0, 78.0, 12.0, |ui, i, r| {
        let (family, path) = found[i];
        if !path.is_empty() && !state.specimens.contains_key(family) {
            let path = path.clone();
            let job = Job::spawn(ui.ctx(), move || {
                let (index, file) = path.split_once(':')?;
                faces::specimen(&std::fs::read(file).ok()?, index.parse().ok()?, SPECIMEN, 40.0)
            });
            state.specimens.insert(family.clone(), Specimen::Coming(job));
        }
        if specimen_tile(ui, state, "appearance-installed", i, r, family, family, Some(family.as_str()) == chosen) {
            picked = Some(family.clone());
        }
    });
    let more = found.len() - shown;
    if more > 0 {
        ui.add_space(8.0);
        let label = tr!("Show more ({count})", count = more);
        if Button::new(&label).variant(Variant::Ghost).size(Size::Sm).show(ui).clicked() {
            state.shown = shown + FONTS_FIRST * 4;
        }
    }
    ui.add_space(8.0);
    state.families = Some(families);
    picked
}

/// A font as a tile: the specimen in it, its name under. Whether it was pressed.
#[allow(clippy::too_many_arguments)]
fn specimen_tile(ui: &mut Ui, state: &mut State, control: &'static str, n: usize, r: Rect, key: &str, name: &str, chosen: bool) -> bool {
    let (_, pressed) = choice(ui, control, n, r, 12.0, chosen, name);
    let p = ui.painter().clone();
    p.rect_filled(r, theme::radius(12.0), theme::surface_soft());
    if !state.specimens.contains_key(key) {
        let made = if key.is_empty() { None } else { BUNDLED.iter().find(|b| b.id == key).and_then(|b| faces::specimen(b.bytes(), 0, SPECIMEN, 40.0)) };
        let made = made.map_or(Specimen::None, |image| Specimen::Ready(ui.ctx().load_texture(format!("specimen-{key}"), image, egui::TextureOptions::LINEAR)));
        state.specimens.insert(key.to_string(), made);
    }
    if let Some(Specimen::Coming(job)) = state.specimens.get_mut(key) {
        if let Some(image) = job.take() {
            let made = image.map_or(Specimen::None, |image| Specimen::Ready(ui.ctx().load_texture(format!("specimen-{key}"), image, egui::TextureOptions::LINEAR)));
            state.specimens.insert(key.to_string(), made);
        }
    }
    let words = pos2(r.left() + 16.0, r.top() + 28.0);
    let room = r.width() - 32.0;
    match state.specimens.get(key) {
        Some(Specimen::Ready(t)) => {
            // Drawn at twice its size, so it is crisp at 200 %.
            let size = t.size_vec2() / 2.0;
            let k = (room / size.x).min(1.0);
            let at = Rect::from_min_size(pos2(words.x, words.y - size.y * k / 2.0), size * k);
            p.image(t.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), theme::fg());
        }
        Some(Specimen::Coming(_)) => {
            p.rect_filled(Rect::from_min_size(pos2(words.x, words.y - 5.0), vec2(room * 0.7, 10.0)), theme::radius(5.0), theme::white(0.06));
        }
        _ if key.is_empty() => {
            // The default, whichever is chosen now.
            let p = &p;
            widgets::text_truncated(p, words, Align2::LEFT_CENTER, &Txt::new(SPECIMEN, 18.0), room);
        }
        _ => {
            widgets::text_truncated(&p, words, Align2::LEFT_CENTER, &Txt::new(name, 18.0), room);
        }
    }
    let c = if chosen { theme::fg() } else { theme::fg_3() };
    widgets::text_truncated(&p, pos2(words.x, r.bottom() - 18.0), Align2::LEFT_CENTER, &Txt::new(name, 11.5).color(c), room - 18.0);
    if chosen {
        widgets::icon(&p, pos2(r.right() - 20.0, r.bottom() - 18.0), icons::CHECK, 13.0, theme::accent());
    }
    pressed
}

// --- Background ---

/// The scenes' names are the website's, the same in every language; their captions are its too.
fn scene_words(b: Background) -> (&'static str, &'static str) {
    match b {
        Background::None => ("Orbiters", tr("Keep it understated")),
        Background::AeroShards => ("Aero Shards", tr("Sculpted light")),
        Background::Grainient => ("Grainient", tr("Soft, flowing color")),
        Background::GradientWaves => ("Gradient Waves", tr("A luminous horizon")),
        Background::PatternWaves => ("Pattern Waves", tr("Dots rolling like silk")),
        Background::GradientAscii => ("Gradient ASCII", tr("A horizon drawn in characters")),
        Background::LeafShadow => ("Window Light", tr("Leaves swaying in the light")),
    }
}

fn backgrounds(ui: &mut Ui, a: &mut AppearanceSettings, reduced: bool) -> bool {
    let mut changed = false;
    ui.add_space(8.0);
    let mut picked = None;
    grid(ui, "appearance-background", BACKGROUNDS.len(), 150.0, 146.0, 14.0, |ui, i, r| {
        let b = BACKGROUNDS[i];
        let art = Rect::from_min_size(r.min, vec2(r.width(), 100.0));
        let (name, caption) = scene_words(b);
        let chosen = a.background == b;
        let (_, pressed) = choice(ui, "appearance-background", i, art, 14.0, chosen, name);
        let p = ui.painter();
        p.rect_filled(art, theme::radius(14.0), theme::ink_950());
        if b == Background::None {
            glow(p, art);
            p.rect_stroke(art, theme::radius(14.0), Stroke::new(1.0, theme::white(0.1)), StrokeKind::Inside);
        } else {
            let drawn = appearance::backdrop_in(appearance::scene(b), a.background_colors, a.accent);
            // A page-wide scene, small.
            let laid = Laid { picture: true, radius: 14.0, size: Some(art.size() * 2.4), ..Laid::PAGE };
            background::paint(p, Id::unique(("appearance-scene", i)), art, &drawn, Time::STILL, laid);
        }
        if chosen {
            let c = art.right_top() + vec2(-18.0, 18.0);
            p.circle_filled(c, 11.0, theme::accent());
            widgets::icon(p, c, icons::CHECK, 13.0, theme::on_accent());
        }
        named(p, r, art, name, chosen);
        widgets::text_truncated(p, pos2(r.left() + 4.0, art.bottom() + 35.0), Align2::LEFT_CENTER, &Txt::new(caption, 11.5).color(theme::fg_3()), r.width() - 8.0);
        if pressed {
            picked = Some(b);
        }
    });
    if let Some(b) = picked {
        a.background = b;
        changed = true;
    }
    if a.background != Background::None {
        ui.add_space(10.0);
        widgets::separator(ui);
        let sets = [BackgroundColors::Own, BackgroundColors::Accent, BackgroundColors::Aurora, BackgroundColors::Ember, BackgroundColors::Lagoon, BackgroundColors::Daydream];
        let wide = sets.len() as f32 * 58.0;
        let about = tr!("{name} now.", name = colors_name(a.background_colors));
        widgets::field(ui, tr("Background's colors"), Some(&about), wide, |ui| {
            let (r, _) = ui.allocate_exact_size(vec2(wide, 40.0), Sense::hover());
            for (i, set) in sets.into_iter().enumerate() {
                let tile = Rect::from_center_size(pos2(r.left() + 29.0 + i as f32 * 58.0, r.center().y), vec2(48.0, 32.0));
                let (resp, pressed) = choice(ui, "appearance-background-colors", i, tile, 10.0, a.background_colors == set, colors_name(set));
                resp.on_hover_text(colors_name(set));
                swatches(ui.painter(), tile, &appearance::backdrop_in(appearance::scene(a.background), set, a.accent));
                if pressed {
                    a.background_colors = set;
                    changed = true;
                }
            }
        });
        widgets::separator(ui);
        let note = if reduced { tr("Reduce motion is on in App & updates: it stands still.") } else { tr(MOTION_WHY) };
        let mut now = Moves::of(a);
        let options = [SegOption::new(Moves::Never, tr("Still")), SegOption::new(Moves::AtRest, tr("Not while playing")), SegOption::new(Moves::Always, tr_ctx("motion", "Always"))];
        widgets::field(ui, tr("Motion"), Some(note), 320.0, |ui| {
            if widgets::segmented(ui, "appearance-motion", &mut now, &options, true) {
                (a.animate, a.animate_while_playing) = (now != Moves::Never, now == Moves::Always);
                changed = true;
            }
        });
    }
    changed
}

pub const MOTION_WHY: &str = mark("It moves while this window is in front and in use, and rests after half a minute. Not while playing: while VRChat or SteamVR runs it costs a game nothing.");

/// The settings' two switches about motion, as the one choice they are.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Moves {
    Never,
    AtRest,
    Always,
}

impl Moves {
    fn of(a: &AppearanceSettings) -> Moves {
        match (a.animate, a.animate_while_playing) {
            (false, _) => Moves::Never,
            (true, false) => Moves::AtRest,
            (true, true) => Moves::Always,
        }
    }
}

fn colors_name(c: BackgroundColors) -> &'static str {
    match c {
        BackgroundColors::Own => tr("Its own colors"),
        BackgroundColors::Accent => tr("The app's accent"),
        BackgroundColors::Aurora => tr("Aurora"),
        BackgroundColors::Ember => tr("Ember"),
        BackgroundColors::Lagoon => tr("Lagoon"),
        BackgroundColors::Daydream => tr("Daydream"),
    }
}

/// Three colours in one tile: the darkest wide, the others on it.
fn swatches(p: &Painter, r: Rect, b: &Backdrop) {
    p.rect_filled(r, theme::radius(10.0), b.colors[0]);
    p.rect_stroke(r, theme::radius(10.0), Stroke::new(1.0, theme::white(0.14)), StrokeKind::Inside);
    p.circle_filled(r.center() - vec2(8.0, 0.0), 8.0, b.colors[1]);
    p.circle_filled(r.center() + vec2(8.0, 0.0), 8.0, b.colors[2]);
}

/// What the settings' search says of those that need something else.
pub fn needs_background(s: &crate::state::Snapshot) -> Option<&'static str> {
    (s.settings.appearance.background == Background::None).then_some(mark("Shown with a background"))
}
