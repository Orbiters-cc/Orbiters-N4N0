//! A theme of N4N0's menu (contracts `Docs/n4n0.md`, Menu themes and translations): which drawing (Orbiters' own look,
//! "modern", or the phosphor terminal, "retro"), the palette both drawings take their colours from, and the colours of the
//! avatar's screen itself (its projected plate and edge, its glow), which Orbiters Toolkit puts on the screen's shader:
//!
//! `{ "schema": 1, "style": "modern", "colors": { "accent": "#00DA6D", … }, "screen": { "tint": "#020805D1", "edge":
//! "#00DA6D", "glow": 1 } }`
//!
//! The base themes are [`Theme::MODERN`] and [`Theme::RETRO`] (today's looks; files beside this crate,
//! `menu-themes/modern.json` and `retro.json`). A colour a theme leaves out is its style's base one.

use orbiters_core::settings::MenuLook;
use orbiters_i18n::tr;
use serde_json::Value;
use std::path::Path;

/// The theme files' schema this build reads and writes.
pub const SCHEMA: u64 = 1;

/// An sRGB colour.
pub type Rgb = [u8; 3];
/// An sRGB colour with its alpha (255 opaque).
pub type Rgba = [u8; 4];

/// The palette both drawings use. Modern: `background` the page, `surface` tiles, fields and chips (keys and buttons a
/// step towards `text`), `text` and `muted` the two text colours, `accent` the main action, what is on and the accent
/// text; `green` to `amber` the home tiles' colours (`red` also hang up). Retro: `background` the ground (its dot grid
/// and dim lines between it and `accent`), `surface` panels, `accent` lines, `text` and `muted` its texts (hot, what is
/// pressed, a step from `text` to white), `red` alerts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
    pub accent: Rgb,
    pub background: Rgb,
    pub surface: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub green: Rgb,
    pub red: Rgb,
    pub blue: Rgb,
    pub violet: Rgb,
    pub cyan: Rgb,
    pub amber: Rgb,
}

/// The palette's names, in the theme files' order.
pub const COLOR_NAMES: [&str; 11] = ["accent", "background", "surface", "text", "muted", "green", "red", "blue", "violet", "cyan", "amber"];

impl Colors {
    fn get_mut(&mut self, name: &str) -> Option<&mut Rgb> {
        Some(match name {
            "accent" => &mut self.accent,
            "background" => &mut self.background,
            "surface" => &mut self.surface,
            "text" => &mut self.text,
            "muted" => &mut self.muted,
            "green" => &mut self.green,
            "red" => &mut self.red,
            "blue" => &mut self.blue,
            "violet" => &mut self.violet,
            "cyan" => &mut self.cyan,
            "amber" => &mut self.amber,
            _ => return None,
        })
    }

    /// The colours in [`COLOR_NAMES`]'s order.
    pub fn all(&self) -> [Rgb; 11] {
        [self.accent, self.background, self.surface, self.text, self.muted, self.green, self.red, self.blue, self.violet, self.cyan, self.amber]
    }
}

/// The avatar's screen itself: what Orbiters Toolkit puts on its shader (the menu's pictures do not use these).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    /// The projected plate behind the page (`_Plate`), alpha its opacity.
    pub tint: Rgba,
    /// The projected screen's edge (`_Edge`).
    pub edge: Rgba,
    /// How much the screen glows, 0 to 2 (1 as the base themes).
    pub glow: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub style: MenuLook,
    pub colors: Colors,
    pub screen: Screen,
}

const BASE_SCREEN: Screen = Screen { tint: [0x02, 0x08, 0x05, 0xd1], edge: [0x00, 0xda, 0x6d, 0xff], glow: 1.0 };

impl Theme {
    /// Orbiters' own look, as it is (Orbiters UI's palette: `orbiters_ui::theme`).
    pub const MODERN: Theme = Theme {
        style: MenuLook::Modern,
        colors: Colors {
            accent: [0x00, 0xda, 0x6d],
            background: [0x10, 0x10, 0x12],
            surface: [0x1f, 0x1f, 0x23],
            text: [0xec, 0xed, 0xee],
            muted: [0x8b, 0x8b, 0x94],
            green: [0x00, 0xda, 0x6d],
            red: [0xff, 0x4d, 0x6d],
            blue: [0x4f, 0x89, 0xff],
            violet: [0xbd, 0x65, 0xff],
            cyan: [0x48, 0xe6, 0xff],
            amber: [0xf5, 0xb7, 0x4f],
        },
        screen: BASE_SCREEN,
    };

    /// The retro look, as it is (contracts `Docs/n4n0.md`, Retro look): ground, panels, lines, text, secondary text and
    /// alerts; its home tiles' colours are its lines' (it draws icons in its lines' colour, YouTube's in red).
    pub const RETRO: Theme = Theme {
        style: MenuLook::Retro,
        colors: Colors {
            accent: [0x2b, 0xe3, 0x6a],
            background: [0x02, 0x0a, 0x04],
            surface: [0x05, 0x18, 0x0b],
            text: [0x7d, 0xff, 0xa6],
            muted: [0x2f, 0xb8, 0x5e],
            green: [0x2b, 0xe3, 0x6a],
            red: [0xff, 0x4d, 0x3d],
            blue: [0x2b, 0xe3, 0x6a],
            violet: [0x2b, 0xe3, 0x6a],
            cyan: [0x2b, 0xe3, 0x6a],
            amber: [0x2b, 0xe3, 0x6a],
        },
        screen: BASE_SCREEN,
    };

    /// The base theme of a style.
    pub fn base(style: MenuLook) -> Theme {
        match style {
            MenuLook::Modern => Theme::MODERN,
            MenuLook::Retro => Theme::RETRO,
        }
    }

    /// Reads a theme file's JSON: colours it leaves out are its style's base ones; names it does not know are warnings;
    /// anything malformed refuses the whole file.
    pub fn parse(json: &str) -> Result<(Theme, Vec<String>), String> {
        let value: Value = serde_json::from_str(json).map_err(|e| tr!("not JSON: {error}", error = e))?;
        let Value::Object(top) = value else { return Err("not a theme file: its JSON is not an object".into()) };
        match top.get("schema") {
            Some(Value::Number(n)) if n.as_u64() == Some(SCHEMA) => {}
            Some(Value::Number(n)) => return Err(format!("\"schema\" {n} is not one this Orbiters - N4N0 reads (it reads {SCHEMA})")),
            Some(_) => return Err(format!("\"schema\" must be the number {SCHEMA}")),
            None => return Err(format!("no \"schema\" (it must be {SCHEMA})")),
        }
        let style = match top.get("style").and_then(Value::as_str) {
            Some("modern") => MenuLook::Modern,
            Some("retro") => MenuLook::Retro,
            _ => return Err("\"style\" must be \"modern\" or \"retro\"".into()),
        };
        let mut theme = Theme::base(style);
        let mut warnings = Vec::new();
        for field in top.keys().filter(|k| !matches!(k.as_str(), "schema" | "style" | "colors" | "screen" | "name")) {
            warnings.push(tr!("unknown field {field} ignored", field = format!("{field:?}")));
        }
        if let Some(colors) = top.get("colors") {
            let Value::Object(colors) = colors else { return Err("\"colors\" must be an object of colours (\"#RRGGBB\")".into()) };
            for (name, value) in colors {
                let Some(slot) = theme.colors.get_mut(name) else {
                    warnings.push(tr!("unknown colour \"colors\".{name} ignored", name = format!("{name:?}")));
                    continue;
                };
                let [r, g, b, a] = color(value).ok_or_else(|| tr!("\"colors\".{name} must be a colour \"#RRGGBB\"", name = format!("{name:?}")))?;
                if a != 255 {
                    warnings.push(tr!("\"colors\".{name}: its alpha is ignored (the menu's colours are opaque)", name = format!("{name:?}")));
                }
                *slot = [r, g, b];
            }
        }
        if let Some(screen) = top.get("screen") {
            let Value::Object(screen) = screen else { return Err("\"screen\" must be an object (\"tint\", \"edge\", \"glow\")".into()) };
            for (name, value) in screen {
                match name.as_str() {
                    "tint" => theme.screen.tint = color(value).ok_or(tr("\"screen\".\"tint\" must be a colour \"#RRGGBB\" or \"#RRGGBBAA\""))?,
                    "edge" => theme.screen.edge = color(value).ok_or(tr("\"screen\".\"edge\" must be a colour \"#RRGGBB\" or \"#RRGGBBAA\""))?,
                    "glow" => {
                        theme.screen.glow = value.as_f64().filter(|g| (0.0..=2.0).contains(g)).ok_or(tr("\"screen\".\"glow\" must be a number from 0 to 2"))? as f32
                    }
                    _ => warnings.push(tr!("unknown field \"screen\".{name} ignored", name = format!("{name:?}"))),
                }
            }
        }
        Ok((theme, warnings))
    }

    /// Reads a theme file (see [`Theme::parse`]).
    pub fn load(path: &Path) -> Result<(Theme, Vec<String>), String> {
        let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Theme::parse(json.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The theme's file, every field given.
    pub fn to_json(&self) -> String {
        let style = match self.style {
            MenuLook::Modern => "modern",
            MenuLook::Retro => "retro",
        };
        let mut out = format!("{{\n  \"schema\": {SCHEMA},\n  \"style\": \"{style}\",\n  \"colors\": {{\n");
        for (i, (name, rgb)) in COLOR_NAMES.iter().zip(self.colors.all()).enumerate() {
            out += &format!("    \"{name}\": \"{}\"{}\n", hex(&rgb), if i + 1 < COLOR_NAMES.len() { "," } else { "" });
        }
        let glow = if self.screen.glow.fract() == 0.0 { format!("{}", self.screen.glow as i32) } else { format!("{}", self.screen.glow) };
        out += &format!(
            "  }},\n  \"screen\": {{\n    \"tint\": \"{}\",\n    \"edge\": \"{}\",\n    \"glow\": {glow}\n  }}\n}}\n", // i18n: not shown
            hex(&self.screen.tint),
            hex(&self.screen.edge)
        );
        out
    }
}

/// A colour of a theme file: `#RRGGBB` (opaque) or `#RRGGBBAA`, either case.
fn color(value: &Value) -> Option<Rgba> {
    let digits = value.as_str()?.strip_prefix('#')?;
    if !(digits.len() == 6 || digits.len() == 8) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?, if digits.len() == 8 { byte(6)? } else { 255 }])
}

/// `#RRGGBB`, or `#RRGGBBAA` when not opaque.
fn hex(c: &[u8]) -> String {
    let opaque = c.len() == 3 || c[3] == 255;
    c[..if opaque { 3 } else { 4 }].iter().fold("#".to_string(), |s, b| s + &format!("{b:02X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_theme_files_are_the_base_themes() {
        for (json, base) in [(include_str!("../../menu-themes/modern.json"), Theme::MODERN), (include_str!("../../menu-themes/retro.json"), Theme::RETRO)] {
            let (theme, warnings) = Theme::parse(json).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(theme, base);
            assert_eq!(json.replace("\r\n", "\n"), base.to_json(), "the file is as the theme writes it");
        }
    }

    #[test]
    fn a_theme_leaves_the_rest_to_its_style() {
        let (theme, warnings) =
            Theme::parse(r##"{ "schema": 1, "style": "retro", "colors": { "accent": "#ffc247", "gold": "#ffd700" }, "screen": { "glow": 1.5 } }"##).unwrap();
        assert_eq!(theme.style, MenuLook::Retro);
        assert_eq!(theme.colors.accent, [0xff, 0xc2, 0x47]);
        assert_eq!(theme.colors.background, Theme::RETRO.colors.background);
        assert_eq!(theme.screen.glow, 1.5);
        assert_eq!(theme.screen.edge, Theme::RETRO.screen.edge);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (again, _) = Theme::parse(&theme.to_json()).unwrap();
        assert_eq!(again, theme);
    }

    #[test]
    fn malformed_themes_are_refused_with_what_is_wrong() {
        for (json, says) in [
            ("{", "not JSON"),
            (r#"{ "style": "modern" }"#, "schema"),
            (r#"{ "schema": 3, "style": "modern" }"#, "schema"),
            (r#"{ "schema": 1 }"#, "style"),
            (r#"{ "schema": 1, "style": "vapor" }"#, "style"),
            (r#"{ "schema": 1, "style": "modern", "colors": [] }"#, "colors"),
            (r#"{ "schema": 1, "style": "modern", "colors": { "accent": "green" } }"#, "accent"),
            (r##"{ "schema": 1, "style": "modern", "colors": { "accent": "#12345" } }"##, "accent"),
            (r##"{ "schema": 1, "style": "modern", "colors": { "text": "#GGGGGG" } }"##, "text"),
            (r##"{ "schema": 1, "style": "modern", "screen": { "tint": 4 } }"##, "tint"),
            (r##"{ "schema": 1, "style": "modern", "screen": { "glow": 3 } }"##, "glow"),
            (r##"{ "schema": 1, "style": "modern", "screen": { "glow": "1" } }"##, "glow"),
        ] {
            let error = Theme::parse(json).expect_err(json);
            assert!(error.contains(says), "{json}: {error}");
        }
    }

    #[test]
    fn colours_read_both_ways() {
        assert_eq!(color(&Value::String("#00DA6D".into())), Some([0, 0xda, 0x6d, 255]));
        assert_eq!(color(&Value::String("#020805d1".into())), Some([2, 8, 5, 0xd1]));
        assert_eq!(color(&Value::String("00DA6D".into())), None);
        assert_eq!(hex(&[2, 8, 5, 0xd1]), "#020805D1");
        assert_eq!(hex(&[0, 0xda, 0x6d, 255]), "#00DA6D");
    }
}
