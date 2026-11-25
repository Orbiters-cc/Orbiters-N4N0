//! The Orbiters look, flat like the website (zinc surfaces on black, radii 10-28, no shadows on controls) and the
//! Unity tools (My Avatar's photoshoot, Logger, Mixer: one step lighter per level, Orbiters green for the main
//! action and for switches that are on). No gradients or glows on controls; colour means status.

use crate::fonts;
use crate::look::{Look, Tokens};
use egui::epaint::text::{LayoutJob, TextFormat, VariationCoords};
use egui::{Align, Color32, CornerRadius, FontFamily, FontId, Stroke, Style, TextStyle, Vec2};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// Orbiters' own colours, as constants: the look everything starts from ([`crate::look`] derives the others), what is drawn
/// outside the app's look (N4N0's pictures on avatars, whose colours are the contracts' themes), and the references of
/// tests.
pub mod base {
    use egui::Color32;

    // Zinc, as the website (Tailwind/HeroUI dark): page, surfaces one step lighter each.
    pub const INK_950: Color32 = Color32::from_rgb(0x09, 0x09, 0x0b);
    pub const INK_900: Color32 = Color32::from_rgb(0x10, 0x10, 0x12);
    pub const INK_850: Color32 = Color32::from_rgb(0x18, 0x18, 0x1b);
    pub const INK_800: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x23);
    pub const INK_750: Color32 = Color32::from_rgb(0x27, 0x27, 0x2a);
    pub const INK_700: Color32 = Color32::from_rgb(0x30, 0x30, 0x35);
    pub const INK_600: Color32 = Color32::from_rgb(0x3f, 0x3f, 0x46);

    pub const FG: Color32 = Color32::from_rgb(0xec, 0xed, 0xee);
    pub const FG_2: Color32 = Color32::from_rgb(0xb4, 0xb4, 0xbb);
    pub const FG_3: Color32 = Color32::from_rgb(0x8b, 0x8b, 0x94);
    pub const FG_4: Color32 = Color32::from_rgb(0x5a, 0x5a, 0x63);

    /// Orbiters green (My Avatar, Logger, Mixer): the main action, switches that are on, selected things.
    pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0xda, 0x6d);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x1e, 0xe6, 0x84);
    /// Text and icons on [`ACCENT`].
    pub const ON_ACCENT: Color32 = Color32::from_rgb(0x05, 0x16, 0x0c);

    /// The logo's mark colour.
    pub const MARK: Color32 = Color32::from_rgb(0xe9, 0xe9, 0xf2);

    pub use super::{CYAN, DANGER, OK, REC, VIOLET, WARN};
}

// The look's colours ([`crate::look`]): Orbiters' own unless the person chose others in Settings, Appearance.
macro_rules! tokens {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
        #[allow(non_camel_case_types, clippy::enum_variant_names)]
        #[derive(Clone, Copy)]
        enum Token { $($name),+ }

        const TOKENS: usize = [$(Token::$name),+].len();

        /// The look of the process, a colour each (as [`Color32::to_array`] has it): read for everything drawn, set when
        /// the look changes.
        static NOW: [AtomicU32; TOKENS] = [$(AtomicU32::new(u32::from_ne_bytes(Tokens::ORBITERS.$name.to_array()))),+];

        $($(#[$doc])* pub fn $name() -> Color32 { token(Token::$name) })+

        fn of(tokens: &Tokens) -> [u32; TOKENS] {
            [$(u32::from_ne_bytes(tokens.$name.to_array())),+]
        }

        /// Every colour of the look, together.
        pub fn tokens() -> Tokens {
            Tokens { $($name: $name()),+ }
        }
    };
}

tokens! {
    /// The page.
    ink_950, ink_900,
    /// Cards.
    ink_850,
    /// Raised: popovers, toasts.
    ink_800, ink_750, ink_700, ink_600,
    fg, fg_2, fg_3,
    /// Decoration and what is disabled: no words anyone must read.
    fg_4,
    /// The main action, switches that are on, selected things. What *means* good is [`OK`], whatever the accent.
    accent, accent_hover,
    /// Text and icons on [`accent`].
    on_accent,
    /// The logo's mark.
    mark,
    /// What is selected in a text (at 45 %).
    selection,
    /// The text cursor, links.
    caret,
}

thread_local! {
    /// [`in_look`]'s, over [`NOW`].
    static HERE: Cell<Option<&'static [u32; TOKENS]>> = const { Cell::new(None) };
    /// [`in_look`] with a background behind the page.
    static HERE_OVER: Cell<Option<bool>> = const { Cell::new(None) };
}

fn token(t: Token) -> Color32 {
    let v = HERE.with(Cell::get).map_or_else(|| NOW[t as usize].load(Ordering::Relaxed), |here| here[t as usize]);
    let [r, g, b, a] = v.to_ne_bytes();
    Color32::from_rgba_premultiplied(r, g, b, a)
}

/// Counts the changes of the look: what keeps something drawn compares it.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// A background lies behind the window's pages ([`crate::background`]): cards let a little of it through.
static OVER_BACKGROUND: AtomicBool = AtomicBool::new(false);

pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// The process draws in `look` from now on. Contexts follow at their next frame ([`crate::follow`]).
pub fn set_look(look: &Look) {
    set_look_over(look, None);
}

/// The same, its pages over a background whose brightest is the colour given ([`Tokens::over`]).
pub fn set_look_over(look: &Look, ground: Option<Color32>) {
    // Asked each frame: the colours are worked out when they change.
    static LAST: Mutex<Option<(Look, Option<Color32>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.replace((*look, ground)) == Some((*look, ground)) {
        return;
    }
    let mut changed = false;
    let tokens = Tokens::of(look);
    for (now, new) in NOW.iter().zip(of(&ground.map_or(tokens, |g| tokens.over(g)))) {
        changed |= now.swap(new, Ordering::Relaxed) != new;
    }
    if changed {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether a background lies behind the pages (the window's process alone).
pub fn set_over_background(over: bool) {
    OVER_BACKGROUND.store(over, Ordering::Relaxed);
}

fn over_background() -> bool {
    HERE_OVER.with(Cell::get).unwrap_or_else(|| OVER_BACKGROUND.load(Ordering::Relaxed))
}

/// Runs `f` in `look` on this thread alone (tests run side by side in one process, and the live preview draws another
/// look than the page around it).
pub fn in_look<R>(look: &Look, f: impl FnOnce() -> R) -> R {
    in_tokens(&Tokens::of(look), f)
}

pub fn in_tokens<R>(tokens: &Tokens, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<&'static [u32; TOKENS]>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HERE.with(|h| h.set(self.0));
        }
    }
    let here = of(tokens);
    // SAFETY: the reference is read through HERE alone, on this thread, and HERE holds what it held before again when
    // `_restore` drops, before `here` does (locals drop in the reverse of their order), whether `f` returns or unwinds.
    let here: &'static [u32; TOKENS] = unsafe { &*(&raw const here) };
    let _restore = Restore(HERE.with(|h| h.replace(Some(here))));
    f()
}

/// Runs `f` as if a background lay behind the page, or none did, on this thread alone.
pub fn with_background<R>(over: bool, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HERE_OVER.with(|h| h.set(self.0));
        }
    }
    let _restore = Restore(HERE_OVER.with(|h| h.replace(Some(over))));
    f()
}

// Colours that mean something, or tell kinds apart: the same in every look.
pub const CYAN: Color32 = Color32::from_rgb(0x48, 0xe6, 0xff);
pub const VIOLET: Color32 = Color32::from_rgb(0xbd, 0x65, 0xff);
pub const INDIGO: Color32 = Color32::from_rgb(0x74, 0x66, 0xff);
pub const BLUE: Color32 = Color32::from_rgb(0x4f, 0x89, 0xff);
pub const PURPLE: Color32 = Color32::from_rgb(0xa2, 0x59, 0xf7);

/// Good, online, done: green whatever the accent.
pub const OK: Color32 = Color32::from_rgb(0x00, 0xda, 0x6d);
pub const WARN: Color32 = Color32::from_rgb(0xf5, 0xb7, 0x4f);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x6b, 0x7a);
pub const REC: Color32 = Color32::from_rgb(0xff, 0x4d, 0x6d);

pub const CARD_RADIUS: f32 = 18.0;
pub const TILE_RADIUS: f32 = 14.0;
/// How opaque cards are over a background.
pub const OVER_BACKGROUND_ALPHA: f32 = 0.82;

/// `c` at opacity `a` (0..1), blended like CSS (in gamma space).
pub fn alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

pub fn white(a: f32) -> Color32 {
    alpha(Color32::WHITE, a)
}

pub fn black(a: f32) -> Color32 {
    alpha(Color32::BLACK, a)
}

/// Mix of two colours in gamma space (`t` = 0 gives `a`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Card fill (the website's `--n4n0-surface`: content1 at 92 %), a little more open over a background (the website's
/// glass over its creators' backgrounds, kept dark enough for text at AA whatever lies under it).
pub fn surface() -> Color32 {
    alpha(ink_850(), if over_background() { OVER_BACKGROUND_ALPHA } else { 0.92 })
}

/// Popovers, toasts and the session detail (the website's raised surface).
pub fn surface_strong() -> Color32 {
    ink_800()
}

/// A surface inside a card (the website's `--n4n0-surface-soft`).
pub fn surface_soft() -> Color32 {
    alpha(ink_750(), 0.45)
}

/// Neutral control fill (secondary buttons, the selected segment, switches that are off).
pub fn control() -> Color32 {
    alpha(ink_600(), 0.55)
}

pub fn hairline() -> Stroke {
    Stroke::new(1.0, white(0.05))
}

/// The squircle radius that looks like a circular corner of radius `r` (the website doubles its radii under
/// `corner-shape: squircle`: 18 → 36, 24 → 48).
pub fn squircle(r: f32) -> f32 {
    2.0 * r
}

pub fn radius(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Font weights available through [`Txt`]; exact on variable fonts, nearest available otherwise.
pub mod weight {
    pub const REGULAR: f32 = 400.0;
    pub const MEDIUM: f32 = 500.0;
    pub const SEMIBOLD: f32 = 600.0;
    pub const BOLD: f32 = 700.0;
}

pub fn font(size: f32, weight: f32) -> FontId {
    FontId::new(size, fonts::family(weight))
}

/// A run of styled text: size, weight, colour, tracking, all caps, monospace.
#[derive(Clone, Debug)]
pub struct Txt {
    pub text: String,
    pub size: f32,
    pub weight: f32,
    pub color: Color32,
    /// Letter spacing in em.
    pub tracking: f32,
    pub mono: bool,
    pub line_height: Option<f32>,
}

impl Txt {
    pub fn new(text: impl Into<String>, size: f32) -> Self {
        Self { text: text.into(), size, weight: weight::REGULAR, color: fg(), tracking: 0.0, mono: false, line_height: None }
    }

    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }

    pub fn color(mut self, c: Color32) -> Self {
        self.color = c;
        self
    }

    pub fn tracking(mut self, em: f32) -> Self {
        self.tracking = em;
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn line_height(mut self, h: f32) -> Self {
        self.line_height = Some(h);
        self
    }

    /// The web's `label-caps`: 10.5 px, weight 650, 0.08 em tracking, upper case, `fg-3`.
    pub fn caps(text: impl AsRef<str>) -> Self {
        Self::new(text.as_ref().to_uppercase(), 10.5).weight(650.0).tracking(0.08).color(fg_3())
    }

    pub fn format(&self) -> TextFormat {
        let font_id = if self.mono { FontId::new(self.size * 0.95, FontFamily::Monospace) } else { font(self.size, self.weight) };
        TextFormat {
            font_id,
            extra_letter_spacing: self.tracking * self.size,
            line_height: self.line_height,
            color: self.color,
            coords: if self.mono { VariationCoords::default() } else { fonts::coords(self.size, self.weight) },
            valign: Align::Center,
            ..Default::default()
        }
    }

    pub fn job(&self, wrap_width: f32) -> LayoutJob {
        let mut job = LayoutJob::single_section(self.text.clone(), self.format());
        job.wrap.max_width = wrap_width;
        job
    }

    /// One line, cut with an ellipsis when wider than `max_width`.
    pub fn job_truncated(&self, max_width: f32) -> LayoutJob {
        let mut job = self.job(max_width);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job
    }
}

/// Dark style for egui's own widgets (text edits, scroll bars, tooltips) in the Orbiters palette.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(style);
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(STYLED), tokens()));
}

/// Where a context notes the colours its style was made in.
const STYLED: &str = "orbiters-theme-styled";

/// To call each frame of a context that can live through a change of look: egui's own widgets take its colours.
pub fn follow(ctx: &egui::Context) {
    // (A few loads and one comparison: no more than asking what changed.)
    let now = tokens();
    if ctx.data(|d| d.get_temp::<Tokens>(egui::Id::unique(STYLED))).is_some_and(|was| was != now) {
        apply(ctx);
    }
}

fn style(style: &mut Style) {
    use egui::style::ScrollStyle;
    style.text_styles = [
        (TextStyle::Small, font(11.5, weight::REGULAR)),
        (TextStyle::Body, font(13.5, weight::REGULAR)),
        (TextStyle::Button, font(13.5, weight::MEDIUM)),
        (TextStyle::Heading, font(22.0, weight::SEMIBOLD)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = None;
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = surface_strong();
    v.window_stroke = hairline();
    v.window_corner_radius = radius(16.0);
    v.menu_corner_radius = radius(14.0);
    v.extreme_bg_color = Color32::TRANSPARENT;
    v.faint_bg_color = white(0.03);
    v.code_bg_color = black(0.4);
    v.hyperlink_color = caret();
    v.selection.bg_fill = alpha(selection(), 0.45);
    v.selection.stroke = Stroke::new(1.0, fg());
    v.text_cursor.stroke = Stroke::new(1.5, caret());
    // Tooltips are about 28 points high: egui's blurred rectangles crease and streak under the corners once the blur
    // passes half the shape's height (its feathering meets in the middle), so popups keep a short shadow.
    v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 16, spread: 0, color: black(0.5) };
    v.window_shadow = v.popup_shadow;
    for (w, bg) in [
        (&mut v.widgets.noninteractive, Color32::TRANSPARENT),
        (&mut v.widgets.inactive, white(0.08)),
        (&mut v.widgets.hovered, white(0.14)),
        (&mut v.widgets.active, white(0.2)),
        (&mut v.widgets.open, white(0.1)),
    ] {
        w.bg_fill = bg;
        w.weak_bg_fill = bg;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, fg_2());
        w.corner_radius = radius(10.0);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, fg_2());
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, fg());
    v.widgets.active.fg_stroke = Stroke::new(1.0, fg());

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 8.0);
    s.button_padding = Vec2::new(12.0, 6.0);
    s.interact_size = Vec2::new(24.0, 24.0);
    s.scroll = ScrollStyle {
        bar_width: 6.0,
        floating_width: 3.0,
        floating_allocated_width: 0.0,
        bar_inner_margin: 3.0,
        bar_outer_margin: 3.0,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        // Hidden at rest (Request16 3): over its area while something scrolls (`crate::scrollbars` sets `active` each frame) and on its own track.
        dormant_handle_opacity: 0.0,
        active_handle_opacity: 0.0,
        interact_handle_opacity: 1.0,
        ..ScrollStyle::floating()
    };
    style.interaction.tooltip_delay = 0.35;
    style.animation_time = 0.12;
}
