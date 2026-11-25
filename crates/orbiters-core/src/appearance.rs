//! How Orbiters - N4N0 looks (Settings, Appearance): colours, font, size and background. Kept with the other settings, per
//! computer. Read tolerantly, as every setting is: a value a newer version wrote that this one does not know gets its
//! default, the others stay.

use serde::{Deserialize, Serialize};

/// What [`AppearanceSettings::version`] says now: raised when a field changes meaning, never when one is added.
pub const VERSION: u32 = 1;

/// Orbiters green.
pub const ACCENT: [u8; 3] = [0x00, 0xda, 0x6d];

/// What the surfaces' greys lean to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tint {
    #[default]
    Zinc,
    Graphite,
    Slate,
    Moss,
    Stone,
    Plum,
    Black,
}

/// The window's size of everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceSize {
    Small,
    #[default]
    Default,
    Large,
}

impl InterfaceSize {
    pub const ALL: [InterfaceSize; 3] = [InterfaceSize::Small, InterfaceSize::Default, InterfaceSize::Large];

    pub fn zoom(self) -> f32 {
        match self {
            InterfaceSize::Small => 0.9,
            InterfaceSize::Default => 1.0,
            InterfaceSize::Large => 1.15,
        }
    }
}

/// What lies behind the window's pages: the backgrounds of orbiters.cc's profiles, by the website's ids.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Background {
    #[default]
    None,
    AeroShards,
    Grainient,
    GradientWaves,
    PatternWaves,
    GradientAscii,
    LeafShadow,
}

/// The colours a background is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundColors {
    /// The website's own for it.
    #[default]
    Own,
    /// From the app's accent.
    Accent,
    // The website's palettes.
    Aurora,
    Ember,
    Lagoon,
    Daydream,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub version: u32,
    pub tint: Tint,
    pub accent: [u8; 3],
    /// The interface's font: empty, the default; a bundled font's id; `installed:` and a family on this computer.
    pub font: String,
    pub size: InterfaceSize,
    pub background: Background,
    pub background_colors: BackgroundColors,
    /// An animated background moves (it stands still otherwise, as with reduced motion).
    pub animate: bool,
    /// It moves while VRChat or SteamVR runs too.
    pub animate_while_playing: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            version: VERSION,
            tint: Tint::Zinc,
            accent: ACCENT,
            font: String::new(),
            size: InterfaceSize::Default,
            background: Background::None,
            background_colors: BackgroundColors::Own,
            animate: true,
            animate_while_playing: false,
        }
    }
}

impl AppearanceSettings {
    /// Orbiters' own look, whatever the switches about motion say.
    pub fn is_orbiters(&self) -> bool {
        let d = Self::default();
        self.tint == d.tint && self.accent == d.accent && self.font.is_empty() && self.size == d.size && self.background == d.background
    }

    /// Back to Orbiters' look; the switches about motion stay.
    pub fn reset(&mut self) {
        *self = Self { animate: self.animate, animate_while_playing: self.animate_while_playing, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_newer_version_wrote_is_read_for_what_it_is_worth() {
        let (read, dropped) = crate::json_lenient::<crate::settings::Settings>(
            r##"{ "appearance": { "version": 9, "tint": "opal", "accent": [183, 138, 255], "font": "nunito", "background": "aero-shards", "stars": 3 } }"##,
        );
        let a = &read.appearance;
        assert_eq!((a.tint, a.accent, a.font.as_str(), a.background), (Tint::Zinc, [183, 138, 255], "nunito", Background::AeroShards));
        assert_eq!(dropped.len(), 1, "only the tint it does not know: {dropped:?}");
        // Settings of before: Orbiters' own look.
        let (old, dropped) = crate::json_lenient::<crate::settings::Settings>("{}");
        assert!(dropped.is_empty() && old.appearance == AppearanceSettings::default() && old.appearance.is_orbiters());
    }

    #[test]
    fn it_is_saved_and_read_back_and_reset_at_once() {
        let mut a = AppearanceSettings { tint: Tint::Plum, accent: [1, 2, 3], font: "installed:Comic Sans MS".into(), size: InterfaceSize::Large, background: Background::LeafShadow, background_colors: BackgroundColors::Ember, animate: false, animate_while_playing: true, ..Default::default() };
        let text = serde_json::to_string(&a).unwrap();
        assert!(text.contains("\"leaf-shadow\""), "the website's id: {text}");
        assert_eq!(serde_json::from_str::<AppearanceSettings>(&text).unwrap(), a);
        a.reset();
        assert!(a.is_orbiters() && !a.animate && a.animate_while_playing);
    }
}
