//! What makes a scene a place rather than objects in the dark: its baked light, its sky and its fog, as the builder reads
//! them from the project's files. Lightmaps come from Bakery's storage when the scene has one (Bakery sets them when
//! the scene loads, so its list counts), else from the renderers' `m_LightmapIndex` and the scene's `LightingData`
//! asset. Each is an HDR picture of several megabytes: it is averaged down, its brightest pixels kept out of its scale,
//! and stored as a JPEG whose colours read as sRGB are the light over that scale, so the viewer needs no HDR decoder
//! and samples it like any picture, with its mipmaps.
//!
//! The sky is the skybox material's: a latitude-longitude picture (Panoramic, a Cubemap imported from one, a custom
//! shader's main picture of that shape), six pictures made into one (6 Sided), or colours (Procedural and any other
//! shader). A picture whose sky is the scene's ambient light gives that ambient light too, which only Unity otherwise
//! computes.

use super::prefab::{self, Graph, Loader};
use super::texture::{self, Encoded, Linear};
use super::{material, yaml};
use crate::preview as file;
use anyhow::Context as _;
use orbiters_i18n::tr;
use std::path::PathBuf;

/// The most lightmaps a preview keeps, and the most pixels on a side of each by how many there are.
pub const MAX_LIGHTMAPS: usize = 16;
pub fn lightmap_side(count: usize) -> u32 {
    if count <= 4 {
        2048
    } else {
        1024
    }
}
/// A sky's picture: a whole sphere stays soft at this size, and costs less than a megabyte.
const SKY_WIDTH: u32 = 2048;
/// How far back a world opens at least, in its framed part's radius.
const MIN_VIEW_DISTANCE: f32 = 0.38;
const BUILTIN_EXTRA: &str = "0000000000000000f000000000000000";
const UNITY_COLOR_SPACE_DOUBLE: f32 = 4.594_794;

/// The scene's list of lightmaps as Unity keeps it (`LightingData`): each lightmap's picture, by index.
pub fn lighting_data_maps(loader: &mut Loader, graph: &Graph) -> Vec<Option<String>> {
    let Some(r) = &graph.settings.lighting_data else { return Vec::new() };
    let Some(path) = r.guid.as_deref().and_then(|g| loader.assets.path(g)) else { return Vec::new() };
    let v = match super::serialized::read_file(&path, r.file_id, 1120, loader.limits.max_yaml_bytes) {
        Ok(v) => v,
        // A project saving its assets as text.
        Err(_) => {
            let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
            let docs = yaml::documents(&text);
            let Some(d) = docs.iter().find(|d| d.class == 1120) else { return Vec::new() };
            d.parse()
        }
    };
    loader.files.push(path);
    v.get("m_Lightmaps").items().iter().map(|m| m.get("m_Lightmap").as_ref().and_then(|r| r.guid)).collect()
}

/// A lightmap read and made small.
pub fn lightmap(path: &std::path::Path, side: u32, limits: &super::Limits) -> anyhow::Result<(Encoded, f32)> {
    limits.check(tr("reading the lightmaps"))?;
    let picture = texture::decode_linear(path, limits.max_texture_bytes)?;
    limits.check(tr("reading the lightmaps"))?;
    texture::encode_light(&picture.shrink(side, true), 0.999, 92)
}

/// The sky a scene's `RenderSettings` name, and the ambient light its picture gives.
pub struct BuiltSky {
    pub sky: file::Sky,
    pub picture: Option<Encoded>,
    /// Facing up, round the horizon, facing down (linear): for a scene whose ambient light comes from its skybox.
    pub ambient: Option<[[f32; 3]; 3]>,
    pub files: Vec<PathBuf>,
}

/// Unity's default sky, as a gradient.
fn default_sky() -> file::Sky {
    file::Sky { texture: -1, scale: 1.0, rotation: 0.0, top: [0.19, 0.37, 0.70], horizon: [0.75, 0.80, 0.86], bottom: [0.32, 0.30, 0.28] }
}

pub fn sky(loader: &mut Loader, graph: &Graph) -> Option<BuiltSky> {
    let r = graph.settings.skybox.as_ref()?;
    let guid = r.guid.as_deref()?;
    if guid == BUILTIN_EXTRA {
        return Some(BuiltSky { sky: default_sky(), picture: None, ambient: None, files: Vec::new() });
    }
    let path = loader.assets.path(guid)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let (_, shader, props) = material::props(&text)?;
    let mut files = vec![path];
    let name = match shader {
        Some(s) if s.guid.as_deref() == Some(BUILTIN_EXTRA) => material::builtin_shader(s.file_id).to_string(),
        Some(s) => s.guid.as_deref().and_then(|g| loader.assets.path(g)).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| material::shader_name(&t)).unwrap_or_default(),
        None => String::new(),
    };
    let limits = loader.limits;
    let exposure = props.f("_Exposure", 1.0).clamp(0.0, 16.0);
    let tint = props.linear("_Tint", [0.5, 0.5, 0.5, 1.0]);
    let gain = [tint[0], tint[1], tint[2]].map(|c| c * UNITY_COLOR_SPACE_DOUBLE * exposure);
    let rotation = props.f("_Rotation", 0.0);
    let mut sides = Vec::new();
    for k in ["_FrontTex", "_BackTex", "_LeftTex", "_RightTex", "_UpTex", "_DownTex"] {
        sides.push(props.texture(k).and_then(|g| loader.assets.path(g)));
    }
    let picture: Option<Linear> = if sides.iter().all(Option::is_some) && !name.contains("panoramic") {
        let mut faces = Vec::new();
        for p in sides.iter().flatten() {
            files.push(p.clone());
            faces.push(texture::decode_linear(p, limits.max_texture_bytes).ok()?.shrink(1024, false));
        }
        Some(texture::sphere_from_cube(&faces))
    } else {
        let main = ["_MainTex", "_Tex", "_Panorama", "_SkyTexture", "_Cubemap"].iter().find_map(|k| props.texture(k)).and_then(|g| loader.assets.path(g));
        match main {
            Some(p) => {
                files.push(p.clone());
                // The whole sphere only (a 1:1 or a cross-shaped cubemap would be drawn wrong): else the colours below.
                texture::decode_linear(&p, limits.max_texture_bytes).ok().filter(|l| l.width >= l.height * 3 / 2)
            }
            None => None,
        }
    };
    if let Some(p) = picture {
        let mut small = p.shrink(SKY_WIDTH, false);
        for px in &mut small.pixels {
            for k in 0..3 {
                px[k] *= gain[k];
            }
        }
        let ambient = small.sphere_light();
        let (encoded, scale) = texture::encode_light(&small, 0.995, 88).ok()?;
        let [top, horizon, bottom] = ambient;
        return Some(BuiltSky { sky: file::Sky { texture: 0, scale, rotation, top, horizon, bottom }, picture: Some(encoded), ambient: Some(ambient), files });
    }
    if name.contains("procedural") || props.has("_SkyTint") {
        // Procedural: its tint over Unity's blue, its ground colour below (their defaults give about the default sky).
        let d = default_sky();
        let sky = props.linear("_SkyTint", [0.5, 0.5, 0.5, 1.0]);
        let ground = props.linear("_GroundColor", [0.369, 0.349, 0.341, 1.0]);
        let top = [0, 1, 2].map(|k| d.top[k] * sky[k] * UNITY_COLOR_SPACE_DOUBLE * exposure);
        let horizon = [0, 1, 2].map(|k| d.horizon[k] * exposure);
        return Some(BuiltSky { sky: file::Sky { top, horizon, bottom: [ground[0] * exposure, ground[1] * exposure, ground[2] * exposure], ..d }, picture: None, ambient: None, files });
    }
    // A shader of its own whose picture is not a whole sphere: the colours it names, if any, over Unity's sky.
    let d = default_sky();
    let top = props.color("_Color").map(|c| [0, 1, 2].map(|k| material::srgb_to_linear(c[k]))).unwrap_or(d.top);
    Some(BuiltSky { sky: file::Sky { top, ..d }, picture: None, ambient: None, files })
}

/// The scene's fog in the format's terms.
pub fn fog(graph: &Graph) -> Option<file::Fog> {
    let f = graph.settings.fog?;
    let mode = match f.mode {
        1 => file::FogMode::Linear,
        2 => file::FogMode::Exponential,
        _ => file::FogMode::Squared,
    };
    Some(file::Fog { mode, color: [0, 1, 2].map(|k| material::srgb_to_linear(f.color[k])), start: f.start, end: f.end, density: f.density })
}

/// The Scene view's camera as Unity kept it, when the project's window layout has one (`UserSettings/Layouts`): what
/// the person last looked at in that editor, for the scene it had open. Its pivot, rotation and size become a camera
/// as the Scene view makes them: the camera a size's distance away along its rotation.
pub fn scene_view_camera(project: &std::path::Path) -> Option<file::SceneCamera> {
    let dir = project.join("UserSettings/Layouts");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.ends_with(".dwlt") || name.contains("maximize") {
            continue;
        }
        let t = e.metadata().and_then(|m| m.modified()).ok()?;
        if newest.as_ref().is_none_or(|(b, _)| t > *b) {
            newest = Some((t, e.path()));
        }
    }
    let text = std::fs::read_to_string(newest?.1).ok()?;
    camera_in_layout(&text)
}

fn camera_in_layout(text: &str) -> Option<file::SceneCamera> {
    for d in yaml::documents(text).iter().filter(|d| d.class == 114 && d.mentions("UnityEditor.SceneView")) {
        let v = d.parse();
        let (Some(pivot), Some(rotation), Some(size)) = (v.at(&["m_Position", "m_Target"]).vec3(), v.at(&["m_Rotation", "m_Target"]).quat(), v.at(&["m_Size", "m_Target"]).as_f32()) else {
            continue;
        };
        // An orthographic or 2D view is not a place to look from.
        if v.at(&["m_Ortho", "m_Target"]).as_i64().unwrap_or(0) != 0 || v.get("m_2DMode").as_i64().unwrap_or(0) != 0 {
            continue;
        }
        let fov = v.at(&["m_CameraSettings", "m_FieldOfView"]).as_f32().filter(|f| *f > 1.0 && *f < 170.0).unwrap_or(60.0);
        // The Scene view's `GetPerspectiveCameraDistance`.
        let distance = size.abs().max(0.01) / (fov.to_radians() * 0.5).sin();
        let [x, y, z, w] = rotation;
        let f = [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)];
        let position = [0, 1, 2].map(|k| pivot[k] - f[k] * distance);
        return Some(file::SceneCamera { position, rotation, pivot, fov_y: fov });
    }
    None
}

/// A Scene view left close up on one wall says little of a world: from the same side, looking at the same point, far
/// enough back to see around it. A view already that far is left as it is.
pub fn far_enough(mut c: file::SceneCamera, focus_radius: f32) -> file::SceneCamera {
    let d = (0..3).map(|k| (c.pivot[k] - c.position[k]).powi(2)).sum::<f32>().sqrt().max(1e-3);
    let wanted = focus_radius * MIN_VIEW_DISTANCE;
    if d < wanted {
        for k in 0..3 {
            c.position[k] = c.pivot[k] + (c.position[k] - c.pivot[k]) / d * wanted;
        }
    }
    c
}

/// Whether a scene is the one Unity had open last (the layout's Scene view looked at it).
pub fn is_last_open(project: &std::path::Path, scene: &std::path::Path, assets: &super::assets::Assets) -> bool {
    let rel = prefab::rel(assets, scene);
    super::last_open_scene(project).is_some_and(|s| s.eq_ignore_ascii_case(&rel))
}

/// What a preview frames: the part of the scene things are in, without the far pieces that would make them tiny (a sky
/// dome, a floor reaching for miles, one object far off). A few points of each drawn object count alike, so what is
/// large but single weighs as little as it should, and the box holds all but the outer few percent of them on each
/// axis.
pub fn framing(points: &mut [Vec<f32>; 3], all: file::Bounds) -> file::Bounds {
    let n = points[0].len();
    if n < 40 {
        return all;
    }
    let mut b = all;
    for k in 0..3 {
        let v = &mut points[k];
        v.sort_by(f32::total_cmp);
        b.min[k] = v[n * 3 / 100];
        b.max[k] = v[n - 1 - n * 3 / 100];
        // Never a flat box: a town on flat ground still has height.
        if b.max[k] - b.min[k] < 1e-3 {
            b.min[k] = all.min[k];
            b.max[k] = all.max[k];
        }
    }
    b
}

/// Reads a scene's Bakery or Unity lightmap for a renderer: which picture, and where.
pub fn lightmap_of(graph: &Graph, unity_maps: &[Option<String>], id: i64, index: i64, st: [f32; 4]) -> Option<(String, [f32; 4])> {
    if let Some(b) = &graph.bakery {
        if let Some(&(k, st)) = b.renderers.get(&id) {
            return Some((b.maps.get(k)?.clone(), st));
        }
    }
    if index >= prefab::NO_LIGHTMAP || index < 0 {
        return None;
    }
    let guid = unity_maps.get(index as usize)?.clone()?;
    Some((guid, st))
}

/// Which file holds a lightmap (for messages).
pub fn path_of(loader: &Loader, guid: &str) -> anyhow::Result<PathBuf> {
    loader.assets.path(guid).with_context(|| tr!("a lightmap that is not in the project ({guid})", guid = guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_views_camera_comes_from_the_layout() {
        let text = "%YAML 1.1
--- !u!114 &1
MonoBehaviour:
  m_EditorClassIdentifier: UnityEditor.dll::UnityEditor.GameView
--- !u!114 &2
MonoBehaviour:
  m_EditorClassIdentifier: UnityEditor.dll::UnityEditor.SceneView
  m_Position:
    m_Target: {x: 1, y: 2, z: 3}
    speed: 2
  m_Rotation:
    m_Target: {x: 0, y: 0, z: 0, w: 1}
  m_Size:
    m_Target: 5
  m_Ortho:
    m_Target: 0
";
        let c = camera_in_layout(text).expect("a camera");
        assert_eq!(c.pivot, [1.0, 2.0, 3.0]);
        // Looking along +Z from 10 m back: 5 / sin(30°).
        assert!((c.position[2] - (3.0 - 10.0)).abs() < 1e-3, "{:?}", c.position);
        assert!(camera_in_layout("%YAML 1.1\n--- !u!114 &1\nMonoBehaviour:\n  m_EditorClassIdentifier: UnityEditor.dll::UnityEditor.SceneView\n").is_none());
    }

    #[test]
    fn the_framing_leaves_out_the_far_pieces() {
        // Fifty objects in a town 20 m across, and one point 5 km off.
        let mut pts: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for i in 0..100 {
            pts[0].push(i as f32 * 0.2);
            pts[1].push((i % 5) as f32);
            pts[2].push(i as f32 * 0.2);
        }
        pts[0].push(5000.0);
        pts[1].push(0.0);
        pts[2].push(5000.0);
        let all = file::Bounds { min: [0.0; 3], max: [5000.0, 4.0, 5000.0] };
        let b = framing(&mut pts, all);
        assert!(b.max[0] < 25.0, "{b:?}");
        assert!(b.max[2] < 25.0, "{b:?}");
        // Too few things to judge: everything.
        let mut few: [Vec<f32>; 3] = [vec![0.0, 1000.0], vec![0.0, 1.0], vec![0.0, 1.0]];
        assert_eq!(framing(&mut few, all), all);
    }
}
