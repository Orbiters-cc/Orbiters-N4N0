//! The edit of a picture, as it is saved next to its copy: how it is cropped and turned, the layers of a VRChat
//! multi-layer picture each with its own effects, and the layers over the whole picture. Places and sizes are shares
//! of the picture (its width and height from the top-left corner, sizes of its height), so the same edit looks the same
//! on the preview and on the full picture. Every field has a default, so an edit saved by an older version opens.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    pub geometry: Geometry,
    /// A VRChat multi-layer picture's layers (the world, the people, the interface), back to front, each with its own
    /// effects: composited in that order before [`Edit::layers`]. Empty for a plain picture.
    pub sources: Vec<SourceStack>,
    /// Over the whole picture, the first applied first (shown at the bottom of the list).
    pub layers: Vec<Layer>,
    /// The id of the next layer made.
    pub next_id: u32,
}

impl Default for Edit {
    fn default() -> Self {
        Self { geometry: Geometry::default(), sources: Vec::new(), layers: Vec::new(), next_id: 1 }
    }
}

impl Edit {
    /// A layer of `effect` with a new id.
    pub fn layer(&mut self, effect: Effect) -> Layer {
        let id = self.next_id;
        self.next_id += 1;
        Layer { id, visible: true, opacity: 1.0, mask: Mask::default(), effect }
    }

    /// The stack `source` names: a source layer's (`Some(i)`), or the whole picture's.
    pub fn stack(&self, source: Option<usize>) -> &[Layer] {
        match source.and_then(|i| self.sources.get(i)) {
            Some(s) => &s.layers,
            None => &self.layers,
        }
    }

    pub fn stack_mut(&mut self, source: Option<usize>) -> &mut Vec<Layer> {
        match source {
            Some(i) if i < self.sources.len() => &mut self.sources[i].layers,
            _ => &mut self.layers,
        }
    }

    /// Whether it changes anything (a picture opened and saved untouched is a plain copy).
    pub fn is_identity(&self) -> bool {
        self.geometry.is_identity()
            && self.layers.iter().all(|l| !l.visible)
            && self.sources.iter().all(|s| s.visible && s.opacity >= 1.0 && s.layers.iter().all(|l| !l.visible))
    }
}

/// One of a multi-layer picture's layers and its effects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStack {
    /// VRChat's name for it ("Environment", "Player", "UI").
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    /// Its effects, the first applied first.
    pub layers: Vec<Layer>,
}

impl Default for SourceStack {
    fn default() -> Self {
        Self { name: String::new(), visible: true, opacity: 1.0, layers: Vec::new() }
    }
}

/// A layer of the stack: an effect, how much of it shows and where.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub id: u32,
    pub visible: bool,
    /// 0 to 1: the picture before it and after it mixed.
    pub opacity: f32,
    pub mask: Mask,
    pub effect: Effect,
}

impl Default for Layer {
    fn default() -> Self {
        Self { id: 0, visible: true, opacity: 1.0, mask: Mask::default(), effect: Effect::Light(Light::default()) }
    }
}

/// Where a layer acts. Distances are shares of the picture's height.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "shape", rename_all = "kebab-case")]
pub enum Mask {
    #[default]
    Everywhere,
    /// Inside a circle around `center`, fading out over `feather` past `radius` (outside it with `invert`).
    Radial { center: [f32; 2], radius: f32, feather: f32, invert: bool },
    /// Along a band through `center` at `angle` degrees, `width` across, fading over `feather` (outside it with
    /// `invert`): a tilt-shift.
    Band { center: [f32; 2], angle: f32, width: f32, feather: f32, invert: bool },
}

impl Mask {
    pub fn radial() -> Self {
        Mask::Radial { center: [0.5, 0.5], radius: 0.25, feather: 0.2, invert: false }
    }

    pub fn band() -> Self {
        Mask::Band { center: [0.5, 0.5], angle: 0.0, width: 0.2, feather: 0.15, invert: false }
    }

    pub fn center(&self) -> Option<[f32; 2]> {
        match self {
            Mask::Everywhere => None,
            Mask::Radial { center, .. } | Mask::Band { center, .. } => Some(*center),
        }
    }

    pub fn set_center(&mut self, c: [f32; 2]) {
        if let Mask::Radial { center, .. } | Mask::Band { center, .. } = self {
            *center = c;
        }
    }

    /// How much of the layer shows at `(u, v)` (shares of the width and height) of a picture of `aspect` (width over
    /// height): 0 to 1.
    pub fn weight(&self, u: f32, v: f32, aspect: f32) -> f32 {
        match *self {
            Mask::Everywhere => 1.0,
            Mask::Radial { center, radius, feather, invert } => {
                let d = (((u - center[0]) * aspect).powi(2) + (v - center[1]).powi(2)).sqrt();
                let w = 1.0 - super::smoothstep(radius, radius + feather.max(1e-4), d);
                if invert {
                    1.0 - w
                } else {
                    w
                }
            }
            Mask::Band { center, angle, width, feather, invert } => {
                let (s, c) = angle.to_radians().sin_cos();
                // Across the band: the distance from its middle line.
                let d = (-(u - center[0]) * aspect * s + (v - center[1]) * c).abs();
                let w = 1.0 - super::smoothstep(width * 0.5, width * 0.5 + feather.max(1e-4), d);
                if invert {
                    1.0 - w
                } else {
                    w
                }
            }
        }
    }
}

/// What a layer does (`type` in its JSON: a blur has a `kind` of its own).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Effect {
    Light(Light),
    Color(Color),
    Curves(Curves),
    Vignette(Vignette),
    Aberration(Aberration),
    Comic(Comic),
    Blur(Blur),
    Text(Text),
}

/// The kinds of layer, in the order the add menu lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Light,
    Color,
    Curves,
    Vignette,
    Aberration,
    Comic,
    Blur,
    Text,
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Light, Kind::Color, Kind::Curves, Kind::Vignette, Kind::Aberration, Kind::Comic, Kind::Blur, Kind::Text];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Light => "Light",
            Kind::Color => "Color",
            Kind::Curves => "Curves",
            Kind::Vignette => "Vignette",
            Kind::Aberration => "Chromatic aberration",
            Kind::Comic => "Comic",
            Kind::Blur => "Blur",
            Kind::Text => "Text",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Kind::Light => "Exposure, brightness, contrast, highlights, shadows and black point.",
            Kind::Color => "Temperature, tint, saturation, vibrance and hue.",
            Kind::Curves => "Tones remapped by a curve, for all channels or red, green and blue.",
            Kind::Vignette => "Darker or lighter edges around a centre you can move.",
            Kind::Aberration => "Colour fringes toward the edges, like a cheap lens.",
            Kind::Comic => "Flat colours in a few tones and ink outlines, like a comic page. Dots add a print screen.",
            Kind::Blur => "Gaussian, lens (round bokeh) or directional (motion).",
            Kind::Text => "A line of text in a font and a style, placed on the picture.",
        }
    }

    /// A new layer's effect, at the usual strength.
    pub fn effect(self) -> Effect {
        match self {
            Kind::Light => Effect::Light(Light::default()),
            Kind::Color => Effect::Color(Color::default()),
            Kind::Curves => Effect::Curves(Curves::default()),
            Kind::Vignette => Effect::Vignette(Vignette::default()),
            Kind::Aberration => Effect::Aberration(Aberration::default()),
            Kind::Comic => Effect::Comic(Comic::default()),
            Kind::Blur => Effect::Blur(Blur::default()),
            Kind::Text => Effect::Text(Text::default()),
        }
    }
}

impl Effect {
    pub fn kind(&self) -> Kind {
        match self {
            Effect::Light(_) => Kind::Light,
            Effect::Color(_) => Kind::Color,
            Effect::Curves(_) => Kind::Curves,
            Effect::Vignette(_) => Kind::Vignette,
            Effect::Aberration(_) => Kind::Aberration,
            Effect::Comic(_) => Kind::Comic,
            Effect::Blur(_) => Kind::Blur,
            Effect::Text(_) => Kind::Text,
        }
    }

    /// What its row in the list says: its kind, or a text's words.
    pub fn title(&self) -> String {
        match self {
            Effect::Text(t) if !t.value.trim().is_empty() => format!("“{}”", t.value.lines().next().unwrap_or_default().trim()),
            Effect::Blur(b) => format!("{} blur", b.kind.name()),
            e => e.kind().name().to_string(),
        }
    }
}

/// Exposure in linear light, then the tones as seen: black point, brightness, contrast, shadows and highlights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Light {
    /// Stops: -4 to 4.
    pub exposure: f32,
    /// -1 to 1: the middle tones lifted or lowered.
    pub brightness: f32,
    /// -1 to 1.
    pub contrast: f32,
    /// -1 to 1: the light tones brightened, or brought back below zero.
    pub highlights: f32,
    /// -1 to 1: the dark tones lifted, or deepened below zero.
    pub shadows: f32,
    /// -0.25 to 0.5: the tone that becomes black (below zero, black is lifted to a faded grey).
    pub black_point: f32,
}

/// White balance and saturation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Color {
    /// -1 (cooler, bluer) to 1 (warmer).
    pub temperature: f32,
    /// -1 (greener) to 1 (more magenta).
    pub tint: f32,
    /// -1 (grey) to 1.
    pub saturation: f32,
    /// -1 to 1: like saturation, mostly on the dull colours.
    pub vibrance: f32,
    /// Degrees, -180 to 180.
    pub hue: f32,
}

/// Curves for all channels and each channel, as points (input, output) from 0 to 1, sorted by input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    pub rgb: Vec<[f32; 2]>,
    pub red: Vec<[f32; 2]>,
    pub green: Vec<[f32; 2]>,
    pub blue: Vec<[f32; 2]>,
}

impl Default for Curves {
    fn default() -> Self {
        let line = vec![[0.0, 0.0], [1.0, 1.0]];
        Self { rgb: line.clone(), red: line.clone(), green: line.clone(), blue: line }
    }
}

/// Which curve of [`Curves`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Rgb,
    Red,
    Green,
    Blue,
}

impl Curves {
    pub fn points(&self, c: Channel) -> &Vec<[f32; 2]> {
        match c {
            Channel::Rgb => &self.rgb,
            Channel::Red => &self.red,
            Channel::Green => &self.green,
            Channel::Blue => &self.blue,
        }
    }

    pub fn points_mut(&mut self, c: Channel) -> &mut Vec<[f32; 2]> {
        match c {
            Channel::Rgb => &mut self.rgb,
            Channel::Red => &mut self.red,
            Channel::Green => &mut self.green,
            Channel::Blue => &mut self.blue,
        }
    }
}

/// The Toolkit photoshoot's vignette, around a centre that moves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// -1 (lighter edges) to 1 (darker edges).
    pub amount: f32,
    /// 0.3 to 2: how far the light reaches.
    pub size: f32,
    /// 0 to 1: how softly it falls off.
    pub feather: f32,
    pub center: [f32; 2],
}

impl Default for Vignette {
    fn default() -> Self {
        Self { amount: 0.6, size: 1.0, feather: 0.5, center: [0.5, 0.5] }
    }
}

/// The Toolkit photoshoot's chromatic aberration: red and blue pulled apart away from a centre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Aberration {
    /// 0 to 3 (the Toolkit's range).
    pub amount: f32,
    pub center: [f32; 2],
}

impl Default for Aberration {
    fn default() -> Self {
        Self { amount: 1.0, center: [0.5, 0.5] }
    }
}

/// The Toolkit photoshoot's comic look (its halftone pass): flat colours in a few tones, ink lines where the picture
/// changes sharply (the Toolkit reads them from the scene's depth; a picture has its own edges and, for a layer of
/// VRChat's, the outline of what it holds), and a print screen of ink dots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Comic {
    /// 2 to 12 tones (the Toolkit's 4 by default).
    pub tones: u32,
    pub dots: bool,
    /// 0 to 1: the dots' spacing, from a 260th of the height to a 36th.
    pub dot_size: f32,
    /// 0 to 1: how many edges get ink lines (none at 0).
    pub ink: f32,
}

impl Default for Comic {
    fn default() -> Self {
        Self { tones: 4, dots: false, dot_size: 0.3, ink: 0.6 }
    }
}

impl Comic {
    pub const MIN_TONES: u32 = 2;
    pub const MAX_TONES: u32 = 12;

    /// The distance between dots, in pixels of a picture `height` high (the Toolkit's `ComicDotCell`, at least 3).
    pub fn dot_cell(&self, height: f32) -> f32 {
        (height * super::lerp(1.0 / 260.0, 1.0 / 36.0, self.dot_size.clamp(0.0, 1.0))).max(3.0)
    }

    /// The ink lines' width in pixels (the Toolkit's: a 520th of the height, at least one).
    pub fn line_width(height: f32) -> f32 {
        (height / 520.0).max(1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BlurKind {
    #[default]
    Gaussian,
    /// A round lens's: every point spread over a disc, bright ones the most (bokeh).
    Lens,
    /// Along a line (motion).
    Directional,
}

impl BlurKind {
    pub fn name(self) -> &'static str {
        match self {
            BlurKind::Gaussian => "Gaussian",
            BlurKind::Lens => "Lens",
            BlurKind::Directional => "Directional",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Blur {
    pub kind: BlurKind,
    /// A share of the picture's height: the gaussian's deviation, the lens's disc radius, the line's length.
    pub radius: f32,
    /// Degrees, for the directional blur (0: along the width).
    pub angle: f32,
    /// 0 to 1: how much bright points outweigh the rest in a lens blur.
    pub bokeh: f32,
}

impl Default for Blur {
    fn default() -> Self {
        Self { kind: BlurKind::Gaussian, radius: 0.01, angle: 0.0, bokeh: 0.5 }
    }
}

/// The Toolkit's text styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TextStyle {
    Clean,
    #[default]
    Outline,
    Shadow,
    Glow,
}

impl TextStyle {
    pub const ALL: [TextStyle; 4] = [TextStyle::Clean, TextStyle::Outline, TextStyle::Shadow, TextStyle::Glow];

    pub fn name(self) -> &'static str {
        match self {
            TextStyle::Clean => "Clean",
            TextStyle::Outline => "Outline",
            TextStyle::Shadow => "Shadow",
            TextStyle::Glow => "Glow",
        }
    }
}

/// The Toolkit photoshoot's text: lines placed and sized as shares of the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Text {
    pub value: String,
    /// A font of [`super::text::fonts`] by its key; empty for the default.
    pub font: String,
    /// Where the text's middle is.
    pub center: [f32; 2],
    /// The font size as a share of the picture's height.
    pub size: f32,
    /// Encoded sRGB.
    pub color: [u8; 3],
    pub style: TextStyle,
}

impl Default for Text {
    fn default() -> Self {
        Self { value: "Orbiters".into(), font: String::new(), center: [0.5, 0.8], size: 0.13, color: [255, 255, 255], style: TextStyle::Outline }
    }
}

impl Text {
    pub const MIN_SIZE: f32 = 0.03;
    pub const MAX_SIZE: f32 = 0.45;
}

//// How the picture is cropped and turned: quarter turns and flips first, then tilted in perspective about its middle
/// lines (vertical and horizontal keystone), then straightened by a small angle about its middle, then cropped (the crop
/// is kept inside the tilted and straightened picture, [`super::geometry`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Geometry {
    /// Quarter turns clockwise, 0 to 3.
    pub turns: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Degrees clockwise, -45 to 45.
    pub straighten: f32,
    /// Degrees, -45 to 45: the picture turned in depth about its horizontal middle line (positive: its top nearer and
    /// wider), straightening lines that lean together upwards, as a building shot from below.
    pub vertical: f32,
    /// Degrees, -45 to 45: turned about its vertical middle line (positive: its right side nearer and taller).
    pub horizontal: f32,
    /// In shares of the turned picture, as asked: the crop drawn is this, shrunk to fit the tilted and straightened
    /// picture.
    pub crop: Crop,
    /// The shape the crop keeps while it is dragged.
    pub aspect: Aspect,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { turns: 0, flip_h: false, flip_v: false, straighten: 0.0, vertical: 0.0, horizontal: 0.0, crop: Crop::FULL, aspect: Aspect::Free }
    }
}

impl Geometry {
    /// The most straightening and perspective tilt, in degrees either way.
    pub const MAX_ANGLE: f32 = 45.0;

    pub fn is_identity(&self) -> bool {
        self.turns.is_multiple_of(4) && !self.flip_h && !self.flip_v && self.straighten == 0.0 && self.vertical == 0.0 && self.horizontal == 0.0 && self.crop == Crop::FULL
    }
}

/// A rectangle in shares of a picture's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for Crop {
    fn default() -> Self {
        Crop::FULL
    }
}

impl Crop {
    pub const FULL: Crop = Crop { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    pub fn center(&self) -> [f32; 2] {
        [self.x + self.w * 0.5, self.y + self.h * 0.5]
    }
}

/// Crop shapes: free, the picture's own, square, and the presets of the iPhone's Photos, standing (portrait) or lying
/// (landscape). Saved by name: those of the first version as they were ("portrait" is 4:5, "classic" 3:2, "wide" 16:9,
/// "story" 9:16), the others as their ratio ("5:7", "7:5"…).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Aspect {
    #[default]
    Free,
    /// The picture's own.
    Original,
    Square,
    /// 4:5, portrait.
    Portrait,
    /// 3:2.
    Classic,
    /// 16:9.
    Wide,
    /// 9:16, a phone's story.
    Story,
    /// The picture's own shape turned a quarter (a lying picture's, standing).
    OriginalTurned,
    #[serde(rename = "5:7")]
    R5x7,
    #[serde(rename = "3:4")]
    R3x4,
    #[serde(rename = "3:5")]
    R3x5,
    #[serde(rename = "2:3")]
    R2x3,
    #[serde(rename = "5:4")]
    R5x4,
    #[serde(rename = "7:5")]
    R7x5,
    #[serde(rename = "4:3")]
    R4x3,
    #[serde(rename = "5:3")]
    R5x3,
}

impl Aspect {
    pub const ALL: [Aspect; 16] = [
        Aspect::Free,
        Aspect::Original,
        Aspect::Square,
        Aspect::Portrait,
        Aspect::Classic,
        Aspect::Wide,
        Aspect::Story,
        Aspect::OriginalTurned,
        Aspect::R5x7,
        Aspect::R3x4,
        Aspect::R3x5,
        Aspect::R2x3,
        Aspect::R5x4,
        Aspect::R7x5,
        Aspect::R4x3,
        Aspect::R5x3,
    ];

    /// The presets as the iPhone's Photos lists them (each in its standing form; [`Aspect::oriented`] lays it down).
    pub const PRESETS: [Aspect; 9] =
        [Aspect::Original, Aspect::Free, Aspect::Square, Aspect::Story, Aspect::Portrait, Aspect::R5x7, Aspect::R3x4, Aspect::R3x5, Aspect::R2x3];

    pub fn name(self) -> &'static str {
        match self {
            Aspect::Free => "Freeform",
            Aspect::Original | Aspect::OriginalTurned => "Original",
            Aspect::Square => "Square",
            Aspect::Portrait => "4:5",
            Aspect::Classic => "3:2",
            Aspect::Wide => "16:9",
            Aspect::Story => "9:16",
            Aspect::R5x7 => "5:7",
            Aspect::R3x4 => "3:4",
            Aspect::R3x5 => "3:5",
            Aspect::R2x3 => "2:3",
            Aspect::R5x4 => "5:4",
            Aspect::R7x5 => "7:5",
            Aspect::R4x3 => "4:3",
            Aspect::R5x3 => "5:3",
        }
    }

    /// Width over height, for a picture whose own is `original`; None when free.
    pub fn ratio(self, original: f32) -> Option<f32> {
        Some(match self {
            Aspect::Free => return None,
            Aspect::Original => original,
            Aspect::OriginalTurned => 1.0 / original.max(1e-6),
            Aspect::Square => 1.0,
            Aspect::Portrait => 4.0 / 5.0,
            Aspect::Classic => 3.0 / 2.0,
            Aspect::Wide => 16.0 / 9.0,
            Aspect::Story => 9.0 / 16.0,
            Aspect::R5x7 => 5.0 / 7.0,
            Aspect::R3x4 => 3.0 / 4.0,
            Aspect::R3x5 => 3.0 / 5.0,
            Aspect::R2x3 => 2.0 / 3.0,
            Aspect::R5x4 => 5.0 / 4.0,
            Aspect::R7x5 => 7.0 / 5.0,
            Aspect::R4x3 => 4.0 / 3.0,
            Aspect::R5x3 => 5.0 / 3.0,
        })
    }

    /// The same shape turned a quarter (4:5 and 5:4, the original and the original turned); free and square stay.
    pub fn turned(self) -> Aspect {
        match self {
            Aspect::Free | Aspect::Square => self,
            Aspect::Original => Aspect::OriginalTurned,
            Aspect::OriginalTurned => Aspect::Original,
            Aspect::Portrait => Aspect::R5x4,
            Aspect::R5x4 => Aspect::Portrait,
            Aspect::Classic => Aspect::R2x3,
            Aspect::R2x3 => Aspect::Classic,
            Aspect::Wide => Aspect::Story,
            Aspect::Story => Aspect::Wide,
            Aspect::R5x7 => Aspect::R7x5,
            Aspect::R7x5 => Aspect::R5x7,
            Aspect::R3x4 => Aspect::R4x3,
            Aspect::R4x3 => Aspect::R3x4,
            Aspect::R3x5 => Aspect::R5x3,
            Aspect::R5x3 => Aspect::R3x5,
        }
    }

    /// Whether it lies (wider than tall) for a picture whose own shape is `original`; None for free and square.
    pub fn landscape(self, original: f32) -> Option<bool> {
        let r = self.ratio(original)?;
        (self != Aspect::Square && (r - 1.0).abs() > 1e-4).then_some(r > 1.0)
    }

    /// This preset standing or lying, for a picture of shape `original` (free and square stay as they are).
    pub fn oriented(self, landscape: bool, original: f32) -> Aspect {
        match self.landscape(original) {
            Some(l) if l != landscape => self.turned(),
            _ => self,
        }
    }

    /// The preset of [`Aspect::PRESETS`] it is a form of.
    pub fn preset(self) -> Aspect {
        if Aspect::PRESETS.contains(&self) {
            self
        } else {
            self.turned()
        }
    }
}
