//! Materials as the preview draws them, read from `.mat` files (Unity YAML) without Unity: the main colour and
//! texture, how transparent, which faces, and the look by shader family, the same rules Orbiters Toolkit's
//! `PreviewLook` applies inside Unity (contracts' Docs/project-preview.md, Materials): lilToon, Poiyomi, MToon, Unity
//! Toon Shader, Standard and its look-alikes, unlit shaders. A material keeps the properties of every shader it once
//! had, so the shader's name decides the family first, its properties only when the shader is missing (a project
//! never opened has its shaders' packages, but a locked Poiyomi shader may be gone).
//!
//! Colours in a `.mat` are as the Inspector shows them (sRGB); the look's shade, emission and metallic are what the
//! shader computes with, linear in a linear project (every VRChat project).

use super::yaml::{self, Value};
use crate::preview::{Alpha, Cull, Material, Shading};
use std::collections::HashMap;

/// A material's look and the textures it needs (by GUID), its `texture`/`emission_texture` still unset.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub material: Material,
    pub main_texture: Option<String>,
    pub emission_texture: Option<String>,
    /// Its normal map, read for what is not an avatar (Toolkit's rules for a material's pictures stay its own).
    pub normal_texture: Option<String>,
    pub shader: String,
}

/// A `.mat` file's saved properties.
pub struct Props {
    floats: HashMap<String, f32>,
    colors: HashMap<String, [f32; 4]>,
    textures: HashMap<String, (Option<String>, [f32; 2], [f32; 2])>,
    keywords: Vec<String>,
    queue: i64,
    tags: HashMap<String, String>,
}

impl Props {
    pub fn has(&self, k: &str) -> bool {
        self.floats.contains_key(k) || self.colors.contains_key(k) || self.textures.contains_key(k)
    }
    pub fn f(&self, k: &str, otherwise: f32) -> f32 {
        self.floats.get(k).copied().unwrap_or(otherwise)
    }
    fn on(&self, k: &str) -> bool {
        self.f(k, 0.0) > 0.5
    }
    pub fn color(&self, k: &str) -> Option<[f32; 4]> {
        self.colors.get(k).copied()
    }
    /// A colour as the shader computes with it (linear, HDR kept).
    pub fn linear(&self, k: &str, otherwise: [f32; 4]) -> [f32; 4] {
        let c = self.color(k).unwrap_or(otherwise);
        [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), c[3]]
    }
    pub fn texture(&self, k: &str) -> Option<&String> {
        self.textures.get(k).and_then(|t| t.0.as_ref())
    }
    fn keyword(&self, k: &str) -> bool {
        self.keywords.iter().any(|w| w == k)
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Reads a `.mat` file's text.
pub fn props(text: &str) -> Option<(String, Option<yaml::Ref>, Props)> {
    let doc = yaml::documents(text).into_iter().find(|d| d.class == 21)?;
    Some(props_of(&doc.parse()))
}

/// A material's fields (from a `.mat` file, or from Unity's binary form, which names them alike).
pub fn props_of(m: &Value) -> (String, Option<yaml::Ref>, Props) {
    let saved = m.get("m_SavedProperties");
    let mut floats = HashMap::new();
    let mut colors = HashMap::new();
    let mut textures = HashMap::new();
    // Older materials store each as `- first: {name: _Color} second: …`; newer as `- _Color: …`.
    let each = |list: &Value, f: &mut dyn FnMut(&str, &Value)| {
        for item in list.items() {
            match (item.get("first").get("name").as_str(), item.entries()) {
                (Some(name), _) => f(name, item.get("second")),
                (None, [(k, v)]) => f(k, v),
                _ => {}
            }
        }
    };
    each(saved.get("m_Floats"), &mut |k, v| {
        if let Some(x) = v.as_f32() {
            floats.insert(k.to_string(), x);
        }
    });
    each(saved.get("m_Ints"), &mut |k, v| {
        if let Some(x) = v.as_f32() {
            floats.entry(k.to_string()).or_insert(x);
        }
    });
    each(saved.get("m_Colors"), &mut |k, v| {
        if let Some(c) = v.color() {
            colors.insert(k.to_string(), c);
        }
    });
    each(saved.get("m_TexEnvs"), &mut |k, v| {
        let guid = v.get("m_Texture").as_ref().and_then(|r| r.guid);
        textures.insert(k.to_string(), (guid, v.get("m_Scale").vec2().unwrap_or([1.0, 1.0]), v.get("m_Offset").vec2().unwrap_or([0.0, 0.0])));
    });
    let mut keywords: Vec<String> = m.get("m_ValidKeywords").items().iter().filter_map(|k| k.as_str().map(str::to_string)).collect();
    if let Some(s) = m.get("m_ShaderKeywords").as_str() {
        keywords.extend(s.split_whitespace().map(str::to_string));
    }
    let tags = m.get("stringTagMap").entries().iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect();
    let props = Props { floats, colors, textures, keywords, queue: m.get("m_CustomRenderQueue").as_i64().unwrap_or(-1), tags };
    (m.get("m_Name").str_or("").to_string(), m.get("m_Shader").as_ref(), props)
}

/// A material's look from its properties and its shader's name (lowercase; empty when unknown).
pub fn look(name: &str, shader: &str, p: &Props) -> Look {
    let mut m = Material { name: name.to_string(), ..Default::default() };
    m.color = p.color("_Color").or_else(|| p.color("_BaseColor")).unwrap_or([1.0; 4]);
    let main =
        ["_MainTex", "_BaseMap", "_BaseColorMap", "_MainTexture"].iter().find_map(|k| p.textures.get(*k).filter(|t| t.0.is_some()).map(|t| (*k, t.clone())));
    let mut look = Look { shader: shader.to_string(), ..Default::default() };
    if let Some((_, (guid, scale, offset))) = main {
        look.main_texture = guid;
        m.tiling = scale;
        m.offset = offset;
    }
    m.alpha = alpha(shader, p);
    m.cutoff = p.f("_Cutoff", 0.5).clamp(0.0, 1.0);
    m.cull = match p.f("_Cull", 2.0).round() as i64 {
        0 => Cull::Off,
        1 => Cull::Front,
        _ => Cull::Back,
    };
    m.shading = Shading::Lit;
    m.smoothness = 0.2;
    let lil = p.has("_lilToonVersion") || shader.contains("liltoon");
    let poi = shader.contains("poiyomi") || (shader.is_empty() && p.has("_LightingMode") && p.has("_LightingShadowColor"));
    if lil {
        lil_toon(&mut m, &mut look, p);
    } else if poi {
        poiyomi(&mut m, &mut look, p);
    } else if p.has("_ShadeToony") && p.has("_ShadeColor") {
        mtoon(&mut m, &mut look, p);
    } else if p.has("_1st_ShadeColor") {
        unity_toon(&mut m, &mut look, p);
    } else if shader.contains("unlit") || shader.contains("particle") || shader.contains("skybox") {
        m.shading = Shading::Unlit;
        m.smoothness = 0.0;
    } else if shader.contains("toon") {
        m.shading = if shader.contains("toon lit") { Shading::Flat } else { Shading::Toon };
        m.smoothness = 0.0;
        emission(&mut m, &mut look, p, p.keyword("_EMISSION"), 1.0, "_EmissionColor", "_EmissionMap");
    } else {
        standard(&mut m, &mut look, shader, p);
    }
    if shader.contains("water") {
        water(&mut m, &mut look, p);
    } else if let Some((guid, scale, offset)) = p.textures.get("_BumpMap").filter(|t| t.0.is_some()).map(|t| (t.0.clone(), t.1, t.2)) {
        look.normal_texture = guid;
        m.normal_scale = p.f("_BumpScale", 1.0);
        // Without a main picture its tiling is the only one the material has.
        if look.main_texture.is_none() {
            m.tiling = scale;
            m.offset = offset;
        }
    }
    look.material = m;
    look
}

/// A water shader (Mochie's, and those like it) draws no picture: its colour comes from what it reflects and what
/// lies under it. A preview draws a see-through surface of its emission's tint with its ripples, smooth enough for the
/// sun to glint on them, which reads as water from the distances a preview looks from.
fn water(m: &mut Material, look: &mut Look, p: &Props) {
    let tint = p.linear("_EmissionColor", [0.35, 0.65, 0.75, 1.0]);
    let base = p.color("_Color").unwrap_or([1.0; 4]);
    // The tint lifted towards white: a shallow sea over sand.
    let lift = |t: f32, c: f32| (0.55 + t * 1.1).min(1.0) * c;
    m.color = [lift(tint[0], base[0]), lift(tint[1], base[1]), lift(tint[2], base[2]), p.f("_Opacity", 0.75).clamp(0.45, 0.85)];
    m.alpha = Alpha::Blend;
    m.shading = Shading::Lit;
    m.smoothness = 0.9;
    m.metallic = 0.0;
    m.emission = [0.0; 3];
    look.emission_texture = None;
    let scale = p.textures.get("_NormalMap0").map_or([1.0, 1.0], |t| t.1);
    let tile = p.f("_NormalMapScale0", 0.0);
    if let Some(guid) = p.texture("_NormalMap0").or_else(|| p.texture("_BumpMap")).or_else(|| p.texture("_NormalMap")) {
        look.normal_texture = Some(guid.clone());
        m.normal_scale = p.f("_NormalStr0", 0.4).clamp(0.0, 1.0).max(0.2);
        if look.main_texture.is_none() {
            m.tiling = if tile > 0.0 { [tile, tile] } else { scale };
        }
    }
}

/// From the render queue (as Unity sorts it), else the shader's own blend mode.
fn alpha(shader: &str, p: &Props) -> Alpha {
    let by_queue = |q: i64| {
        if q >= 3000 {
            Alpha::Blend
        } else if q >= 2450 {
            Alpha::Cutout
        } else {
            Alpha::Opaque
        }
    };
    if p.queue > 0 {
        return by_queue(p.queue);
    }
    if let Some(t) = p.tags.get("RenderType") {
        match t.as_str() {
            "TransparentCutout" => return Alpha::Cutout,
            "Transparent" | "Fade" => return Alpha::Blend,
            "Opaque" => return Alpha::Opaque,
            _ => {}
        }
    }
    if shader.contains("cutout") || shader.contains("transparentcutout") {
        return Alpha::Cutout;
    }
    if shader.contains("transparent") || shader.contains("fade") || shader.contains("/trans") {
        return Alpha::Blend;
    }
    // Standard's and Poiyomi's Rendering Mode.
    match p.f("_Mode", 0.0).round() as i64 {
        1 => Alpha::Cutout,
        2..=6 if p.has("_Mode") => Alpha::Blend,
        _ => Alpha::Opaque,
    }
}

fn standard(m: &mut Material, look: &mut Look, shader: &str, p: &Props) {
    let urp = p.has("_Smoothness") && !p.has("_Glossiness");
    let metallic = p.f("_Metallic", 0.0);
    let smoothness = if p.has("_Glossiness") {
        p.f("_Glossiness", 0.5)
    } else if urp {
        p.f("_Smoothness", 0.5)
    } else if shader.contains("diffuse") || shader.contains("lightmapped") || shader.contains("matcap") {
        0.0
    } else if shader.contains("specular") && p.has("_Shininess") {
        p.f("_Shininess", 0.0).clamp(0.0, 1.0) * 0.6
    } else {
        0.2
    };
    // A [Gamma] slider (Standard's Metallic) is converted in a linear project; no metallic map read here.
    m.metallic = if p.has("_Glossiness") { srgb_to_linear(metallic) } else { metallic };
    m.smoothness = smoothness;
    if p.texture("_MetallicGlossMap").is_some() {
        // The map would scale them; without reading it, keep them modest.
        m.metallic *= 0.5;
        m.smoothness = p.f("_GlossMapScale", smoothness) * 0.6;
    }
    emission(m, look, p, p.keyword("_EMISSION"), 1.0, "_EmissionColor", "_EmissionMap");
}

fn lil_toon(m: &mut Material, look: &mut Look, p: &Props) {
    if p.f("_AsUnlit", 0.0) >= 0.99 {
        m.shading = Shading::Unlit;
    } else if p.on("_UseShadow") {
        m.shading = Shading::Toon;
        let c = p.linear("_ShadowColor", [0.82, 0.76, 0.85, 1.0]);
        let strength = p.f("_ShadowStrength", 1.0) * p.color("_ShadowColor").map_or(1.0, |c| c[3]);
        m.shade = [0, 1, 2].map(|k| 1.0 + (c[k] - 1.0) * strength);
        m.shade_border = p.f("_ShadowBorder", 0.5);
        m.shade_blur = p.f("_ShadowBlur", 0.1);
    } else {
        m.shading = Shading::Flat;
    }
    m.smoothness = 0.0;
    if p.on("_UseReflection") {
        m.metallic = p.f("_Metallic", 0.0);
        m.smoothness = p.f("_Smoothness", 1.0);
    }
    emission(m, look, p, p.on("_UseEmission"), p.f("_EmissionBlend", 1.0), "_EmissionColor", "_EmissionMap");
}

fn poiyomi(m: &mut Material, look: &mut Look, p: &Props) {
    let mode = p.f("_LightingMode", 5.0).round() as i64;
    let strength = p.f("_ShadowStrength", 1.0);
    let tint = |prop: &str, scale: f32| {
        let c = p.linear(prop, [1.0; 4]);
        [0, 1, 2].map(|k| 1.0 + (c[k] * scale - 1.0) * strength)
    };
    match mode {
        0 => toon(m, tint("_LightingShadowColor", 0.55), 0.5, 0.4),
        1 => toon(m, tint("_ShadowColor", 1.0), p.f("_ShadowBorder", 0.5), p.f("_ShadowBlur", 0.1)),
        2 => toon(m, tint("_LightingShadowColor", 0.4), 0.45, 0.9),
        4 => toon(m, tint("_1st_ShadeColor", 1.0), p.f("_BaseColor_Step", 0.5), p.f("_BaseShade_Feather", 0.0001)),
        3 | 6 | 7 => m.shading = Shading::Lit,
        _ => m.shading = Shading::Flat,
    }
    m.metallic = 0.0;
    m.smoothness = 0.0;
    if p.on("_MochieBRDF") {
        m.metallic = p.f("_MochieMetallicMultiplier", 0.0);
        m.smoothness = p.f("_MochieRoughnessMultiplier", 1.0);
    }
    emission(m, look, p, p.on("_EnableEmission"), p.f("_EmissionStrength", 1.0), "_EmissionColor", "_EmissionMap");
}

fn mtoon(m: &mut Material, look: &mut Look, p: &Props) {
    let lit = p.linear("_Color", [1.0; 4]);
    let shade = p.linear("_ShadeColor", [0.97, 0.81, 0.86, 1.0]);
    let tint = [0, 1, 2].map(|k| ratio(shade[k], lit[k]));
    let toony = p.f("_ShadeToony", 0.9);
    let shift = p.f("_ShadeShift", 0.0);
    toon(m, tint, (0.5 - shift * 0.5).clamp(0.0, 1.0), (1.0 - toony).clamp(0.0, 1.0));
    m.smoothness = 0.0;
    emission(m, look, p, p.has("_EmissionColor"), 1.0, "_EmissionColor", "_EmissionMap");
}

fn unity_toon(m: &mut Material, look: &mut Look, p: &Props) {
    let lit = p.linear("_BaseColor", [1.0; 4]);
    let shade = p.linear("_1st_ShadeColor", [1.0; 4]);
    toon(m, [0, 1, 2].map(|k| ratio(shade[k], lit[k])), p.f("_BaseColor_Step", 0.5), p.f("_BaseShade_Feather", 0.0001));
    m.smoothness = 0.0;
    emission(m, look, p, p.has("_Emissive_Color"), 1.0, "_Emissive_Color", "_Emissive_Tex");
}

fn toon(m: &mut Material, shade: [f32; 3], border: f32, blur: f32) {
    m.shading = Shading::Toon;
    m.shade = shade.map(|c| c.clamp(0.0, 1.0));
    m.shade_border = border.clamp(0.0, 1.0);
    m.shade_blur = blur.clamp(0.0, 1.0);
}

fn ratio(shade: f32, lit: f32) -> f32 {
    if lit > 0.001 {
        (shade / lit).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn emission(m: &mut Material, look: &mut Look, p: &Props, on: bool, strength: f32, color: &str, texture: &str) {
    if !on || !p.colors.contains_key(color) {
        return;
    }
    let c = p.linear(color, [0.0; 4]);
    let c = [c[0] * strength, c[1] * strength, c[2] * strength];
    if c.iter().all(|v| *v <= 0.001) {
        return;
    }
    m.emission = c;
    look.emission_texture = p.texture(texture).cloned();
}

/// A shader's name from its file (`Shader "lilToon"`), lowercase.
pub fn shader_name(text: &str) -> Option<String> {
    let at = text.find("Shader \"")?;
    let rest = &text[at + 8..];
    Some(rest[..rest.find('"')?].to_ascii_lowercase())
}

/// Unity's built-in shaders by file ID (`guid: 0000000000000000f000000000000000`).
pub fn builtin_shader(file_id: i64) -> &'static str {
    match file_id {
        46 => "standard",
        45 => "standard (specular setup)",
        10750 => "unlit/texture",
        10751 => "unlit/transparent",
        10752 => "unlit/transparent cutout",
        10753 => "unlit/color",
        10755 => "unlit/texture",
        200 | 7 => "legacy shaders/diffuse",
        103 => "skybox/cubemap",
        104 => "skybox/6 sided",
        106 => "skybox/procedural",
        108 => "skybox/panoramic",
        _ => "standard",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mat(shader_floats: &str, colors: &str) -> String {
        format!(
            "%YAML 1.1\n--- !u!21 &2100000\nMaterial:\n  m_Name: Body\n  m_Shader: {{fileID: 4800000, guid: 11112222333344445555666677778888, type: 3}}\n  m_ValidKeywords: []\n  m_CustomRenderQueue: -1\n  m_SavedProperties:\n    serializedVersion: 3\n    m_TexEnvs:\n    - _MainTex:\n        m_Texture: {{fileID: 2800000, guid: 99990000000000000000000000000000, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n    - _EmissionMap:\n        m_Texture: {{fileID: 2800000, guid: 88880000000000000000000000000000, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n    m_Floats:\n{shader_floats}    m_Colors:\n{colors}"
        )
    }

    #[test]
    fn liltoon_draws_toon_with_its_shadow_colour() {
        let text = mat(
            "    - _lilToonVersion: 44\n    - _UseShadow: 1\n    - _ShadowStrength: 1\n    - _Cull: 0\n",
            "    - _Color: {r: 0.5, g: 0.5, b: 1, a: 1}\n    - _ShadowColor: {r: 0.5, g: 0.5, b: 0.5, a: 1}\n",
        );
        let (name, shader, p) = props(&text).unwrap();
        assert_eq!(name, "Body");
        assert_eq!(shader.unwrap().guid.as_deref(), Some("11112222333344445555666677778888"));
        let l = look(&name, "liltoon", &p);
        assert_eq!(l.material.shading, Shading::Toon);
        assert!((l.material.shade[0] - srgb_to_linear(0.5)).abs() < 1e-4);
        assert_eq!(l.material.color, [0.5, 0.5, 1.0, 1.0]);
        assert_eq!(l.material.cull, Cull::Off);
        assert_eq!(l.main_texture.as_deref(), Some("99990000000000000000000000000000"));
        assert_eq!(l.emission_texture, None, "lilToon's emission is off");
    }

    #[test]
    fn poiyomi_emission_brings_its_mask_and_the_queue_sets_alpha() {
        let text = mat(
            "    - _LightingMode: 6\n    - _EnableEmission: 1\n    - _EmissionStrength: 2\n",
            "    - _Color: {r: 1, g: 1, b: 1, a: 1}\n    - _EmissionColor: {r: 1, g: 0, b: 0, a: 1}\n    - _LightingShadowColor: {r: 1, g: 1, b: 1, a: 1}\n",
        )
        .replace("m_CustomRenderQueue: -1", "m_CustomRenderQueue: 2460");
        let (name, _, p) = props(&text).unwrap();
        let l = look(&name, "hidden/locked/.poiyomi/poiyomi toon/body", &p);
        assert_eq!(l.material.shading, Shading::Lit);
        assert_eq!(l.material.emission, [2.0, 0.0, 0.0]);
        assert_eq!(l.emission_texture.as_deref(), Some("88880000000000000000000000000000"));
        assert_eq!(l.material.alpha, Alpha::Cutout);
    }

    #[test]
    fn a_missing_shader_is_guessed_from_its_properties_and_unknown_ones_draw_lit() {
        let text = mat("    - _LightingMode: 5\n", "    - _LightingShadowColor: {r: 1, g: 1, b: 1, a: 1}\n");
        let (name, _, p) = props(&text).unwrap();
        assert_eq!(look(&name, "", &p).material.shading, Shading::Flat, "Poiyomi's Flat lighting");
        let text = mat("    - _Glossiness: 0.8\n    - _Metallic: 1\n    - _Mode: 3\n", "    - _Color: {r: 1, g: 1, b: 1, a: 0.5}\n");
        let (name, _, p) = props(&text).unwrap();
        let l = look(&name, "standard", &p);
        assert_eq!((l.material.shading, l.material.alpha), (Shading::Lit, Alpha::Blend));
        assert!((l.material.smoothness - 0.8).abs() < 1e-6 && (l.material.metallic - 1.0).abs() < 1e-6);
    }

    #[test]
    fn shader_names_come_from_the_shader_file() {
        assert_eq!(shader_name("// comment\nShader \"lilToon/Cutout\" {\n").as_deref(), Some("liltoon/cutout"));
        assert_eq!(shader_name("nothing"), None);
    }
}
