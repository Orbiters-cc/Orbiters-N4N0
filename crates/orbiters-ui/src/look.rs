//! The look a person chose (Settings, Appearance): the surfaces' tint and the accent. Everything else the theme draws with
//! is derived from those two by rule, so that a choice can never make a page unreadable:
//!
//! - the ink ramp and the text ramp keep the lightness of Orbiters' own steps and take the tint's hue, so every step
//!   between two surfaces stays the one the pages were drawn for;
//! - text steps that carry words (`fg`, `fg_2`, `fg_3`) are at WCAG AA (4.5:1) or better on every surface up to
//!   [`Tokens::ink_750`];
//! - the accent is at AA as text on those surfaces too ([`passing`] lightens one that is not, as little as it can, keeping
//!   its hue), and what is written on it ([`Tokens::on_accent`]) is at AA on both the accent and its hover;
//! - colours that mean something (online, warnings, errors, recording, VRChat's own statuses) are not part of it.
//!
//! Orbiters' own look ([`Look::ORBITERS`]) is not derived: it is the constants of [`crate::theme::base`], exactly.

use crate::theme::base;
use egui::Color32;

/// The surfaces' family: the hue their greys lean to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tint {
    /// The website's zinc, Orbiters' own.
    #[default]
    Zinc,
    /// Pure greys.
    Graphite,
    Slate,
    Moss,
    Stone,
    Plum,
    /// Zinc over true black.
    Black,
}

impl Tint {
    pub const ALL: [Tint; 7] = [Tint::Zinc, Tint::Graphite, Tint::Slate, Tint::Moss, Tint::Stone, Tint::Plum, Tint::Black];

    /// Hue (degrees) and chroma of its inks in OKLCH, and how light they are against zinc's.
    fn family(self) -> (f32, f32, f32) {
        match self {
            Tint::Zinc => (286.0, 0.005, 1.0),
            Tint::Graphite => (0.0, 0.0, 1.0),
            Tint::Slate => (257.0, 0.021, 1.0),
            Tint::Moss => (158.0, 0.016, 1.0),
            Tint::Stone => (62.0, 0.011, 1.0),
            Tint::Plum => (318.0, 0.022, 1.0),
            Tint::Black => (286.0, 0.005, 0.8),
        }
    }
}

/// What was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    pub tint: Tint,
    pub accent: Color32,
}

impl Look {
    /// Orbiters' own look.
    pub const ORBITERS: Look = Look { tint: Tint::Zinc, accent: base::ACCENT };
}

impl Default for Look {
    fn default() -> Self {
        Self::ORBITERS
    }
}

/// Every colour the theme draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub ink_950: Color32,
    pub ink_900: Color32,
    pub ink_850: Color32,
    pub ink_800: Color32,
    pub ink_750: Color32,
    pub ink_700: Color32,
    pub ink_600: Color32,
    pub fg: Color32,
    pub fg_2: Color32,
    pub fg_3: Color32,
    pub fg_4: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub mark: Color32,
    pub selection: Color32,
    pub caret: Color32,
}

/// AA for text.
pub const AA: f32 = 4.5;

impl Tokens {
    /// Orbiters' own.
    pub const ORBITERS: Tokens = Tokens {
        ink_950: base::INK_950,
        ink_900: base::INK_900,
        ink_850: base::INK_850,
        ink_800: base::INK_800,
        ink_750: base::INK_750,
        ink_700: base::INK_700,
        ink_600: base::INK_600,
        fg: base::FG,
        fg_2: base::FG_2,
        fg_3: base::FG_3,
        fg_4: base::FG_4,
        accent: base::ACCENT,
        accent_hover: base::ACCENT_HOVER,
        on_accent: base::ON_ACCENT,
        mark: base::MARK,
        selection: base::VIOLET,
        caret: base::CYAN,
    };

    pub fn of(look: &Look) -> Tokens {
        let mut t = Tokens::ORBITERS;
        if look.tint != Tint::Zinc {
            let (hue, chroma, light) = look.tint.family();
            let ink = |c: Color32| {
                let l = Oklch::of(c).l * light;
                // The page itself goes to true black; the steps above it stay apart.
                let l = if look.tint == Tint::Black && c == base::INK_950 { 0.0 } else { l };
                Oklch { l, c: chroma, h: hue }.color()
            };
            let text = |c: Color32| Oklch { l: Oklch::of(c).l, c: chroma * 0.45, h: hue }.color();
            t.ink_950 = ink(base::INK_950);
            t.ink_900 = ink(base::INK_900);
            t.ink_850 = ink(base::INK_850);
            t.ink_800 = ink(base::INK_800);
            t.ink_750 = ink(base::INK_750);
            t.ink_700 = ink(base::INK_700);
            t.ink_600 = ink(base::INK_600);
            t.fg = lightened(text(base::FG), t.ink_700, AA);
            t.fg_2 = lightened(text(base::FG_2), t.ink_700, AA);
            t.fg_3 = lightened(text(base::FG_3), t.ink_750, AA);
            t.fg_4 = lightened(text(base::FG_4), t.ink_850, contrast(base::FG_4, base::INK_850));
            t.mark = text(base::MARK);
        }
        if look.accent.to_opaque() != base::ACCENT {
            let a = passing(look.accent.to_opaque(), look.tint);
            let k = Oklch::of(a);
            t.accent = a;
            t.accent_hover = Oklch { l: (k.l + 0.04).min(0.985), ..k }.color();
            t.on_accent = Oklch { l: 0.185, c: (k.c * 0.2).min(0.03), h: k.h }.color();
            t.selection = Oklch { l: 0.62, c: k.c.min(0.19), h: k.h }.color();
            t.caret = a;
        }
        t
    }
}

impl Tokens {
    /// The same over a background as bright as `ground`: the quieter words and the accent a step lighter where AA needs it, never
    /// past the second text step. What is drawn on the accent stays as readable (it only gets lighter).
    pub fn over(self, ground: Color32) -> Tokens {
        let lifted = lightened(self.fg_3, ground, AA);
        let fg_3 = if luminance(lifted) > luminance(self.fg_2) { self.fg_2 } else { lifted };
        Tokens { fg_3, accent: lightened(self.accent, ground, AA), ..self }
    }
}

/// The accent itself when it can be read on the tint's surfaces, else the nearest of its hue that can.
pub fn passing(accent: Color32, tint: Tint) -> Color32 {
    if accent == base::ACCENT {
        return accent;
    }
    let ground = Tokens::of(&Look { tint, accent: base::ACCENT }).ink_750;
    lightened(accent.to_opaque(), ground, AA)
}

/// `color` as it is when it is at `least` on `ground`, else lightened (its hue kept) just until it is.
fn lightened(color: Color32, ground: Color32, least: f32) -> Color32 {
    if contrast(color, ground) >= least {
        return color;
    }
    let k = Oklch::of(color);
    let (mut lo, mut hi) = (k.l, 1.0f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        if contrast(Oklch { l: mid, ..k }.color(), ground) >= least {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Oklch { l: hi, ..k }.color()
}

/// Relative luminance (WCAG).
pub fn luminance(c: Color32) -> f32 {
    let [r, g, b, _] = c.to_array();
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG contrast of two opaque colours (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `top` (premultiplied, as painted) over an opaque `ground`.
pub fn over(top: Color32, ground: Color32) -> Color32 {
    let k = 1.0 - top.a() as f32 / 255.0;
    let c = |t: u8, g: u8| (t as f32 + g as f32 * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(c(top.r(), ground.r()), c(top.g(), ground.g()), c(top.b(), ground.b()))
}

/// How dark a veil (black at this opacity) must lie on `ground` for `text` to be at `least` over it: none when it is.
pub fn veil_for(text: Color32, ground: Color32, least: f32) -> f32 {
    let veiled = |a: f32| over(crate::theme::black(a), ground);
    if contrast(text, veiled(0.0)) >= least && luminance(text) > luminance(ground) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        let g = veiled(mid);
        if contrast(text, g) >= least && luminance(text) > luminance(g) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

fn linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// A colour by lightness, chroma and hue as the eye has them (OKLCH, Björn Ottosson): steps of lightness keep their hue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    /// Degrees.
    pub h: f32,
}

impl Oklch {
    pub fn of(color: Color32) -> Oklch {
        let [r, g, b] = [linear(color.r()), linear(color.g()), linear(color.b())];
        let l = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_993 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b).cbrt();
        let lab = [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ];
        Oklch { l: lab[0], c: lab[1].hypot(lab[2]), h: lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0) }
    }

    fn rgb(self) -> [f32; 3] {
        let (a, b) = (self.c * self.h.to_radians().cos(), self.c * self.h.to_radians().sin());
        let l = (self.l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m = (self.l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s = (self.l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        [4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s, -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s, -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s]
    }

    /// The colour: one the screen cannot show keeps its lightness and hue and gives up chroma.
    pub fn color(self) -> Color32 {
        let shown = |k: Oklch| k.rgb().iter().all(|v| (-0.000_5..=1.000_5).contains(v));
        let mut k = Oklch { l: self.l.clamp(0.0, 1.0), ..self };
        if !shown(k) {
            let (mut lo, mut hi) = (0.0f32, k.c);
            for _ in 0..18 {
                let mid = (lo + hi) / 2.0;
                if shown(Oklch { c: mid, ..k }) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            k.c = lo;
        }
        let [r, g, b] = k.rgb().map(|v| (encoded(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
        Color32::from_rgb(r, g, b)
    }
}

/// `#rrggbb` (or `rrggbb`, or the short `#rgb`), as typed.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let t = text.trim().trim_start_matches('#');
    if !t.is_ascii() || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let of = |s: &str| u8::from_str_radix(s, 16).ok();
    match t.len() {
        6 => Some(Color32::from_rgb(of(&t[0..2])?, of(&t[2..4])?, of(&t[4..6])?)),
        3 => {
            let d = |i: usize| of(&t[i..i + 1]).map(|v| v * 17);
            Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
        }
        _ => None,
    }
}

pub fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCENTS: [&str; 14] = ["#00da6d", "#338ef7", "#b78aff", "#ffab76", "#64dec3", "#ed91be", "#ff0000", "#0000ff", "#00ff00", "#ffff00", "#ffffff", "#000000", "#123456", "#7f7f7f"];

    #[test]
    fn orbiters_own_look_is_exactly_as_it_was() {
        assert_eq!(Tokens::of(&Look::ORBITERS), Tokens::ORBITERS);
        // Its accent under another tint is untouched too.
        assert_eq!(Tokens::of(&Look { tint: Tint::Plum, accent: base::ACCENT }).accent, base::ACCENT);
    }

    #[test]
    fn words_are_readable_whatever_is_chosen() {
        for tint in Tint::ALL {
            for accent in ACCENTS {
                let look = Look { tint, accent: parse_hex(accent).unwrap() };
                let t = Tokens::of(&look);
                let own = look.tint == Tint::Zinc;
                for (name, ground) in [("950", t.ink_950), ("900", t.ink_900), ("850", t.ink_850), ("800", t.ink_800), ("750", t.ink_750)] {
                    for (step, text) in [("fg", t.fg), ("fg_2", t.fg_2), ("fg_3", t.fg_3), ("accent", t.accent)] {
                        // Orbiters' own third text step is 4.4:1 on its lightest surface: it stays exactly as it is.
                        let least = if own && step == "fg_3" && name == "750" { 4.4 } else { AA };
                        assert!(contrast(text, ground) >= least, "{tint:?} {accent}: {step} on ink_{name} is {:.2}", contrast(text, ground));
                    }
                }
                for text in [t.fg, t.fg_2] {
                    assert!(contrast(text, t.ink_700) >= AA, "{tint:?}");
                }
                assert!(contrast(t.on_accent, t.accent) >= AA && contrast(t.on_accent, t.accent_hover) >= AA, "{tint:?} {accent}");
                // What is selected in a text keeps the words on it readable (it lies at 45 % on a surface).
                let selected = over(crate::theme::alpha(t.selection, 0.45), t.ink_800);
                assert!(contrast(t.fg, selected) >= AA, "{tint:?} {accent}: selection {:.2}", contrast(t.fg, selected));
                // What means something stays readable on every tint.
                for status in [base::OK, base::WARN, base::DANGER, base::REC] {
                    assert!(contrast(status, t.ink_800) >= AA, "{tint:?}: {status:?}");
                }
            }
        }
    }

    #[test]
    fn surfaces_stay_apart_in_every_tint() {
        for tint in Tint::ALL {
            let t = Tokens::of(&Look { tint, accent: base::ACCENT });
            let inks = [t.ink_950, t.ink_900, t.ink_850, t.ink_800, t.ink_750, t.ink_700, t.ink_600];
            for pair in inks.windows(2) {
                assert!(luminance(pair[0]) < luminance(pair[1]), "{tint:?} {pair:?}");
            }
        }
    }

    #[test]
    fn an_accent_that_passes_is_kept_and_another_moves_no_further_than_it_must() {
        let violet = parse_hex("#b78aff").unwrap();
        assert_eq!(passing(violet, Tint::Zinc), violet);
        let navy = parse_hex("#123456").unwrap();
        let lit = passing(navy, Tint::Zinc);
        assert_ne!(lit, navy);
        let c = contrast(lit, base::INK_750);
        assert!((AA..AA + 0.15).contains(&c), "{c}");
        let (a, b) = (Oklch::of(navy).h, Oklch::of(lit).h);
        assert!((a - b).abs() < 4.0, "its hue is kept: {a} {b}");
    }

    #[test]
    fn a_veil_is_as_dark_as_reading_needs_and_no_darker() {
        assert_eq!(veil_for(base::FG_3, base::INK_950, AA), 0.0);
        let bright = Color32::from_rgb(120, 110, 140);
        let a = veil_for(base::FG_3, bright, AA);
        let c = contrast(base::FG_3, over(crate::theme::black(a), bright));
        assert!((AA..AA + 0.2).contains(&c), "{a} {c}");
    }

    #[test]
    fn colours_go_round_through_oklch_and_hex() {
        for c in ["#00da6d", "#896abd", "#ffffff", "#000000", "#120f17", "#f5b74f"] {
            let color = parse_hex(c).unwrap();
            let back = Oklch::of(color).color();
            let d = |a: u8, b: u8| a.abs_diff(b);
            assert!(d(color.r(), back.r()) <= 1 && d(color.g(), back.g()) <= 1 && d(color.b(), back.b()) <= 1, "{c} {back:?}");
            assert_eq!(hex(color), c);
        }
        assert_eq!(parse_hex(" #abc "), Some(Color32::from_rgb(0xaa, 0xbb, 0xcc)));
        for no in ["", "#12", "#12345", "red", "#gggggg", "#1234567", "é12"] {
            assert_eq!(parse_hex(no), None, "{no}");
        }
    }
}
