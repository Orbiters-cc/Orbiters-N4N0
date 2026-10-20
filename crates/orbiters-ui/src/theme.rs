//! The Orbiters look: the website's near-black with purple and blue glows, the logo's cyan and violet, the Unity
//! tools' flat cards (radius 16-20, 1 px hairlines) and their status colours.

use crate::fonts;
use egui::epaint::text::{LayoutJob, TextFormat, VariationCoords};
use egui::{Align, Color32, CornerRadius, FontFamily, FontId, Stroke, Style, TextStyle, Vec2};

pub const INK_950: Color32 = Color32::from_rgb(0x07, 0x07, 0x0b);
pub const INK_900: Color32 = Color32::from_rgb(0x0c, 0x0c, 0x12);
pub const INK_850: Color32 = Color32::from_rgb(0x11, 0x11, 0x18);
pub const INK_800: Color32 = Color32::from_rgb(0x16, 0x16, 0x1e);
pub const INK_750: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x26);
pub const INK_700: Color32 = Color32::from_rgb(0x24, 0x24, 0x2f);
pub const INK_600: Color32 = Color32::from_rgb(0x30, 0x30, 0x3d);

pub const FG: Color32 = Color32::from_rgb(0xf3, 0xf3, 0xf8);
pub const FG_2: Color32 = Color32::from_rgb(0xb4, 0xb4, 0xc6);
pub const FG_3: Color32 = Color32::from_rgb(0x7a, 0x7a, 0x8f);
pub const FG_4: Color32 = Color32::from_rgb(0x50, 0x50, 0x5f);

pub const CYAN: Color32 = Color32::from_rgb(0x48, 0xe6, 0xff);
pub const VIOLET: Color32 = Color32::from_rgb(0xbd, 0x65, 0xff);
pub const INDIGO: Color32 = Color32::from_rgb(0x74, 0x66, 0xff);
pub const BLUE: Color32 = Color32::from_rgb(0x4f, 0x89, 0xff);
pub const PURPLE: Color32 = Color32::from_rgb(0xa2, 0x59, 0xf7);

pub const OK: Color32 = Color32::from_rgb(0x3c, 0xf2, 0x9a);
pub const WARN: Color32 = Color32::from_rgb(0xf5, 0xb7, 0x4f);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x6b, 0x7a);
pub const REC: Color32 = Color32::from_rgb(0xff, 0x4d, 0x6d);

/// The logo's mark colour.
pub const MARK: Color32 = Color32::from_rgb(0xe9, 0xe9, 0xf2);

pub const CARD_RADIUS: f32 = 20.0;
pub const TILE_RADIUS: f32 = 16.0;

/// The brand gradient (cyan → indigo → violet).
pub const BRAND: [(f32, Color32); 3] = [(0.0, CYAN), (0.5, INDIGO), (1.0, VIOLET)];
/// The primary button gradient (blue → indigo → violet).
pub const PRIMARY: [(f32, Color32); 3] = [(0.0, BLUE), (0.5, INDIGO), (1.0, VIOLET)];

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

/// Glass card fill (the web's `surface`).
pub fn surface() -> Color32 {
    alpha(INK_800, 0.78)
}

/// Stronger glass for popovers, toasts and the session detail (the web's `surface-strong`).
pub fn surface_strong() -> Color32 {
    alpha(INK_750, 0.9)
}

pub fn hairline() -> Stroke {
    Stroke::new(1.0, white(0.06))
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
        Self { text: text.into(), size, weight: weight::REGULAR, color: FG, tracking: 0.0, mono: false, line_height: None }
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
        Self::new(text.as_ref().to_uppercase(), 10.5).weight(650.0).tracking(0.08).color(FG_3)
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

/// Gradient text (the web's `text-gradient`): one colour per character, sampled along the word.
pub fn gradient_job(text: &str, base: &Txt, stops: &[(f32, Color32)]) -> LayoutJob {
    let chars: Vec<char> = text.chars().collect();
    let mut job = LayoutJob::default();
    let n = chars.len().max(2) - 1;
    for (i, ch) in chars.iter().enumerate() {
        let mut format = base.format();
        format.color = crate::paint::sample(stops, i as f32 / n as f32);
        job.append(&ch.to_string(), 0.0, format);
    }
    job
}

/// Dark style for egui's own widgets (text edits, scroll bars, tooltips) in the Orbiters palette.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(style);
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
    v.hyperlink_color = CYAN;
    v.selection.bg_fill = alpha(VIOLET, 0.45);
    v.selection.stroke = Stroke::new(1.0, FG);
    v.text_cursor.stroke = Stroke::new(1.5, CYAN);
    v.popup_shadow = egui::Shadow { offset: [0, 18], blur: 40, spread: 0, color: black(0.6) };
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
        w.fg_stroke = Stroke::new(1.0, FG_2);
        w.corner_radius = radius(10.0);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG_2);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, FG);
    v.widgets.active.fg_stroke = Stroke::new(1.0, FG);

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
        dormant_handle_opacity: 0.6,
        active_handle_opacity: 0.9,
        interact_handle_opacity: 1.0,
        ..ScrollStyle::floating()
    };
    style.interaction.tooltip_delay = 0.35;
    style.animation_time = 0.12;
}
