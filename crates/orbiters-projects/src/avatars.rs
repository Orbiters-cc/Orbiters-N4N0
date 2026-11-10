//! Unity text serialization reader: the uploaded avatars and worlds of a project's scenes (their PipelineManager
//! blueprint IDs), and the world thumbnail My World keeps. Supports direct components and prefab-instance overrides
//! (the Canine scene stores its uploaded blueprint in an override). Never writes scenes or opens them in Unity.
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Avatar {
    pub id: String,
    pub name: String,
    pub scene: String,
    pub path: String,
    /// A world's own picture on disk: the thumbnail My World keeps for the scene (`MyWorld.thumbnail`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<PathBuf>,
}
impl Avatar {
    /// An uploaded world (`wrld_…`) rather than an avatar.
    pub fn is_world(&self) -> bool {
        self.id.starts_with("wrld_")
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CachedScene {
    pub modified: u64,
    pub len: u64,
    pub avatars: Vec<Avatar>,
    pub dependencies: Vec<(PathBuf, u64)>,
}
pub type SceneCache = HashMap<PathBuf, CachedScene>;

/// One document of a scene or a prefab: of the kinds read here, with the lines read of it ([`Wanted`]).
#[derive(Default, Clone, Debug, PartialEq)]
struct Doc {
    id: String,
    kind: u32,
    lines: Vec<String>,
}

/// What finding uploaded avatars and worlds reads of a scene or a prefab, picked out a line at a time: the documents of
/// the five kinds it looks at (GameObjects 1, Transforms 4 and 224, MonoBehaviours 114, prefab instances 1001), and of
/// each only the lines it looks at. A scene used to be read whole and then kept a `String` a line, every document of
/// it: two to three times its size in memory, on a background thread of the app's window, and Unity writes meshes
/// and terrains into scenes as text. What is kept here is a few lines an object, whatever the scene weighs.
///
/// Every reader below ([`value`], [`overrides`], the prefab instances' own loop) sees the lines it would have seen
/// among all of the document's, in their order: a line is kept when it starts with a key some reader asks that kind
/// of document for, and a prefab instance's modification (`- target:`, `propertyPath:`, `value:`) when its property is
/// one that is read, `m_Name` or `blueprintId`, behind the target it applies to. A test holds this against every
/// document read whole (`what_is_kept_of_a_scene_reads_as_the_whole_of_it`).
#[derive(Default)]
struct Wanted {
    docs: Vec<Doc>,
    /// The document being read, if it is of a kind that is kept.
    current: Option<Doc>,
    /// In a prefab instance: the last `- target:` line, whether it is among the kept lines yet, and whether the
    /// values that follow belong to a property that is read.
    target: Option<String>,
    target_kept: bool,
    keep_value: bool,
    /// Whether `blueprintId` is written anywhere in the file (a prefab without it has nothing uploaded).
    blueprint: bool,
}

impl Wanted {
    fn of_text(text: &str) -> Self {
        let mut wanted = Self::default();
        for l in text.lines() {
            wanted.line(l);
        }
        wanted.end();
        wanted
    }

    /// The next line of the file, without its end.
    fn line(&mut self, l: &str) {
        if !self.blueprint && l.contains("blueprintId") {
            self.blueprint = true;
        }
        if let Some(h) = l.strip_prefix("--- !u!") {
            self.end();
            self.current = h.split_once(" &").and_then(|(kind, id)| {
                let kind = kind.parse().unwrap_or(0);
                matches!(kind, 1 | 4 | 224 | 114 | 1001).then(|| Doc { id: id.split_whitespace().next().unwrap_or_default().into(), kind, lines: vec![] })
            });
            return;
        }
        let Some(d) = self.current.as_mut() else { return };
        let t = l.trim();
        let keys: &[&str] = match d.kind {
            1 => &["m_Name:", "m_CorrespondingSourceObject:", "m_PrefabInstance:"],
            4 | 224 => &["m_GameObject:", "m_Father:"],
            114 => &["blueprintId:", "m_GameObject:", "thumbnailCamera:", "thumbnail:"],
            _ => &["m_SourcePrefab:", "m_TransformParent:"],
        };
        if keys.iter().any(|k| t.starts_with(k)) {
            d.lines.push(l.into());
        } else if d.kind == 1001 {
            if t.starts_with("- target:") {
                self.target = Some(l.into());
                self.target_kept = false;
            } else if let Some(property) = t.strip_prefix("propertyPath:") {
                self.keep_value = matches!(property.trim(), "m_Name" | "blueprintId");
                if self.keep_value {
                    self.keep_target();
                    self.current.as_mut().expect("being read").lines.push(l.into());
                }
            } else if self.keep_value && t.starts_with("value:") {
                self.keep_target();
                self.current.as_mut().expect("being read").lines.push(l.into());
            }
        }
    }

    /// Puts the last `- target:` line among the kept ones, once: what follows applies to it.
    fn keep_target(&mut self) {
        if let (false, Some(target), Some(d)) = (self.target_kept, &self.target, self.current.as_mut()) {
            d.lines.push(target.clone());
            self.target_kept = true;
        }
    }

    /// The end of a document, or of the file.
    fn end(&mut self) {
        if let Some(d) = self.current.take() {
            self.docs.push(d);
        }
        (self.target, self.target_kept, self.keep_value) = (None, false, false);
    }
}

/// What a scene's or a prefab's file turned out to be.
enum ReadFile {
    /// Unity's text: what is read of it, and whether it says `blueprintId` anywhere.
    Yaml(Model, bool),
    /// Other text: nothing to find, and nothing wrong.
    OtherText,
    /// Not text (Asset Serialization is not Force Text, or the file is damaged): it cannot be read here.
    Binary,
}

/// How much of a line is looked at: the lines read are keys and short values, and a line longer than this is a mesh's or
/// a terrain's data, megabytes without a line end.
const LINE_HEAD: usize = 64 * 1024;

/// Reads a scene or a prefab through, a line at a time, keeping what [`Wanted`] keeps: the memory it takes does not
/// grow with the file (a 64 KiB read buffer, the head of the line being read, and the kept lines).
fn read_file(path: &Path) -> std::io::Result<ReadFile> {
    use std::io::{BufRead, Read};
    let mut reader = std::io::BufReader::with_capacity(LINE_HEAD, std::fs::File::open(path)?);
    let mut line = Vec::with_capacity(256);
    let mut wanted = Wanted::default();
    let mut first = true;
    loop {
        line.clear();
        let read = (&mut reader).take(LINE_HEAD as u64).read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        } else if read == LINE_HEAD {
            // The rest of a long line: passed over, never held.
            reader.skip_until(b'\n')?;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if std::mem::take(&mut first) && !line.starts_with(b"%YAML") {
            // No text holds a zero byte; a cut at the head's end may split a character, which is no fault of the text.
            let text = !line.contains(&0) && std::str::from_utf8(&line).map_or_else(|e| e.error_len().is_none(), |_| true);
            return Ok(if text { ReadFile::OtherText } else { ReadFile::Binary });
        }
        match std::str::from_utf8(&line) {
            Ok(l) => wanted.line(l),
            Err(_) => wanted.line(&String::from_utf8_lossy(&line)),
        }
    }
    if first {
        return Ok(ReadFile::OtherText);
    }
    wanted.end();
    let blueprint = wanted.blueprint;
    Ok(ReadFile::Yaml(Model::of(wanted.docs), blueprint))
}
fn value(d: &Doc, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    d.lines.iter().find_map(|l| l.trim().strip_prefix(&prefix).map(|s| unquote(s.trim())))
}
fn unquote(s: &str) -> String {
    if s.starts_with('"') {
        serde_json::from_str(s).unwrap_or_else(|_| s.trim_matches('"').into())
    } else {
        s.trim_matches('\'').replace("''", "'")
    }
}
fn fileid(s: &str) -> String {
    s.split("fileID:").nth(1).unwrap_or_default().trim().split([',', '}']).next().unwrap_or_default().trim().into()
}
fn guid(s: &str) -> Option<String> {
    Some(s.split("guid:").nth(1)?.trim().split([',', '}']).next()?.trim().into())
}
/// An avatar's or a world's blueprint ID.
fn valid_id(s: &str) -> bool {
    s.strip_prefix("avtr_").or_else(|| s.strip_prefix("wrld_")).is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}
fn overrides(d: &Doc, property_name: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut target = String::new();
    let mut property = String::new();
    for l in &d.lines {
        let l = l.trim();
        if let Some(v) = l.strip_prefix("- target:") {
            target = fileid(v);
        }
        if let Some(v) = l.strip_prefix("propertyPath:") {
            property = v.trim().into();
        }
        if let Some(v) = l.strip_prefix("value:") {
            if property == property_name {
                out.insert(target.clone(), unquote(v.trim()));
            }
        }
    }
    out
}

struct Model {
    docs: Vec<Doc>,
    names: HashMap<String, String>,
    transforms: HashMap<String, (String, String)>,
}
impl Model {
    fn new(text: &str) -> Self {
        Self::of(Wanted::of_text(text).docs)
    }

    fn of(docs: Vec<Doc>) -> Self {
        let mut names = HashMap::new();
        let mut transforms = HashMap::new();
        for d in &docs {
            if d.kind == 1 {
                names.insert(d.id.clone(), value(d, "m_Name").unwrap_or_default());
            }
            if matches!(d.kind, 4 | 224) {
                transforms.insert(d.id.clone(), (fileid(&value(d, "m_GameObject").unwrap_or_default()), fileid(&value(d, "m_Father").unwrap_or_default())));
            }
        }
        // A prefab variant's root GameObject is stripped; its name is an override on the source GameObject.
        for d in docs.iter().filter(|d| d.kind == 1 && value(d, "m_Name").is_none()) {
            let source = fileid(&value(d, "m_CorrespondingSourceObject").unwrap_or_default());
            let instance = fileid(&value(d, "m_PrefabInstance").unwrap_or_default());
            if let Some(name) = docs.iter().find(|d| d.id == instance && d.kind == 1001).and_then(|d| overrides(d, "m_Name").remove(&source)) {
                names.insert(d.id.clone(), name);
            }
        }
        Self { docs, names, transforms }
    }
    fn path(&self, go: &str) -> String {
        let mut parts = vec![self.names.get(go).cloned().unwrap_or_else(|| tr("Avatar").into())];
        let mut parent = self.transforms.values().find(|(g, _)| g == go).map(|(_, p)| p.as_str()).unwrap_or("0");
        let mut seen = HashSet::new();
        while parent != "0" && seen.insert(parent.to_string()) {
            let Some((g, p)) = self.transforms.get(parent) else { break };
            if let Some(n) = self.names.get(g) {
                parts.push(n.clone());
            }
            parent = p;
        }
        parts.reverse();
        parts.join("/")
    }
    fn direct(&self, scene: &str) -> Vec<Avatar> {
        self.docs
            .iter()
            .filter(|d| d.kind == 114)
            .filter_map(|d| {
                let id = value(d, "blueprintId")?;
                if !valid_id(&id) {
                    return None;
                }
                let go = fileid(&value(d, "m_GameObject")?);
                Some(Avatar {
                    id,
                    name: self.names.get(&go).cloned().unwrap_or_else(|| tr("Avatar").into()),
                    scene: scene.into(),
                    path: self.path(&go),
                    ..Default::default()
                })
            })
            .collect()
    }
    /// The GUID of the picture My World's component keeps as the world's thumbnail (its `thumbnail` and
    /// `thumbnailCamera` fields), if the scene has one.
    fn world_thumbnail(&self) -> Option<String> {
        self.docs
            .iter()
            .filter(|d| d.kind == 114 && d.lines.iter().any(|l| l.trim_start().starts_with("thumbnailCamera:")))
            .find_map(|d| value(d, "thumbnail").and_then(|t| guid(&t)).filter(|g| g.len() == 32 && g.bytes().any(|b| b != b'0')))
    }
}
pub fn extract(text: &str, scene: &str) -> Vec<Avatar> {
    if !text.starts_with("%YAML") {
        return vec![];
    }
    Model::new(text).direct(scene)
}

pub fn in_project(project: &Path, cache: &mut SceneCache) -> Vec<Avatar> {
    in_project_checked(project, cache).0
}

/// Also reports incomplete reads, so discovery can retain the previous list during transient I/O failures. A scene or
/// a prefab of any size is read ([`read_file`]): a project's uploaded avatars and worlds show whatever its scenes weigh.
pub(crate) fn in_project_checked(project: &Path, cache: &mut SceneCache) -> (Vec<Avatar>, bool) {
    let mut complete = true;
    let assets = project.join("Assets");
    let mut scenes = vec![];
    let mut prefabs = HashMap::new();
    // Pictures' .meta files, read only when a scene names a My World thumbnail.
    let mut pictures = vec![];
    for entry in walkdir::WalkDir::new(&assets).follow_links(false).into_iter().filter_entry(|e| {
        !crate::discovery::skip_dir(&e.file_name().to_string_lossy())
            && (!e.file_type().is_dir() || !e.metadata().ok().is_some_and(|m| crate::discovery::is_link(&m)))
    }) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let p = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if p.extension().is_some_and(|e| e == "unity") {
            scenes.push(p.to_path_buf());
        }
        let lower = p.to_string_lossy().to_ascii_lowercase();
        if [".png.meta", ".jpg.meta", ".jpeg.meta"].iter().any(|e| lower.ends_with(e)) {
            pictures.push(p.to_path_buf());
        }
        if p.to_string_lossy().ends_with(".prefab.meta") {
            if let Ok(s) = std::fs::read_to_string(p) {
                if let Some(g) = s.lines().find_map(|l| l.strip_prefix("guid: ")) {
                    prefabs.insert(g.to_string(), p.with_extension(""));
                }
            } else {
                complete = false;
            }
        }
    }
    let mut out = vec![];
    let mut prefab_models: HashMap<PathBuf, Option<Model>> = HashMap::new();
    let mut picture_guids: Option<HashMap<String, PathBuf>> = None;
    for p in scenes {
        let Ok(meta) = std::fs::metadata(&p) else {
            complete = false;
            continue;
        };
        let modified = crate::discovery::fingerprint(&p);
        if let Some(c) = cache
            .get(&p)
            .filter(|c| c.modified == modified && c.len == meta.len() && c.dependencies.iter().all(|(p, m)| crate::discovery::fingerprint(p) == *m))
        {
            out.extend(c.avatars.clone());
            continue;
        }
        let model = match read_file(&p) {
            Ok(ReadFile::Yaml(model, _)) => model,
            Ok(ReadFile::OtherText) => continue,
            // What cannot be read is not known to hold nothing: the project keeps what it showed.
            Ok(ReadFile::Binary) | Err(_) => {
                complete = false;
                continue;
            }
        };
        let scene = p.strip_prefix(project).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let mut avatars = model.direct(&scene);
        let mut dependencies = vec![];
        let thumbnail = model.world_thumbnail().and_then(|g| {
            let guids = picture_guids.get_or_insert_with(|| {
                pictures
                    .iter()
                    .filter_map(|m| {
                        let s = std::fs::read_to_string(m).ok()?;
                        Some((s.lines().find_map(|l| l.strip_prefix("guid: "))?.trim().to_string(), m.with_extension("")))
                    })
                    .collect()
            });
            guids.get(&g).cloned()
        });
        if let Some(t) = &thumbnail {
            dependencies.push((t.clone(), crate::discovery::fingerprint(t)));
        }
        for d in model.docs.iter().filter(|d| d.kind == 1001) {
            let Some(source) = value(d, "m_SourcePrefab").and_then(|s| guid(&s)).and_then(|g| prefabs.get(&g)) else { continue };
            dependencies.push((source.clone(), crate::discovery::fingerprint(source)));
            let prefab = prefab_models.entry(source.clone()).or_insert_with(|| match read_file(source) {
                Ok(ReadFile::Yaml(model, blueprint)) => blueprint.then_some(model),
                Ok(ReadFile::OtherText) => None,
                Ok(ReadFile::Binary) | Err(_) => {
                    complete = false;
                    None
                }
            });
            let Some(prefab) = prefab.as_ref() else { continue };
            let mut names = prefab.names.clone();
            let mut ids: HashMap<String, String> = HashMap::new();
            let mut target = String::new();
            let mut property = String::new();
            for l in &d.lines {
                let l = l.trim();
                if let Some(v) = l.strip_prefix("- target:") {
                    target = fileid(v);
                }
                if let Some(v) = l.strip_prefix("propertyPath:") {
                    property = v.trim().into();
                }
                if let Some(v) = l.strip_prefix("value:") {
                    let v = unquote(v.trim());
                    if property == "m_Name" {
                        names.insert(target.clone(), v.clone());
                    }
                    if property == "blueprintId" {
                        ids.insert(target.clone(), v);
                    }
                }
            }
            for component in prefab.docs.iter().filter(|d| d.kind == 114) {
                let id = ids.get(&component.id).cloned().or_else(|| value(component, "blueprintId")).unwrap_or_default();
                if !valid_id(&id) {
                    continue;
                }
                let go = fileid(&value(component, "m_GameObject").unwrap_or_default());
                let name =
                    names.get(&go).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| source.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                let mut hierarchy = Model { docs: vec![], names: names.clone(), transforms: prefab.transforms.clone() }.path(&go);
                if let Some(parent) = value(d, "m_TransformParent").map(|s| fileid(&s)).and_then(|t| model.transforms.get(&t)) {
                    hierarchy = format!("{}/{}", model.path(&parent.0), hierarchy);
                }
                avatars.push(Avatar { id, name, scene: scene.clone(), path: hierarchy, ..Default::default() });
            }
        }
        for a in avatars.iter_mut().filter(|a| a.is_world()) {
            a.thumbnail = thumbnail.clone();
        }
        avatars.sort_by(|a, b| (&a.id, &a.path).cmp(&(&b.id, &b.path)));
        avatars.dedup();
        if complete {
            cache.insert(p, CachedScene { modified, len: meta.len(), avatars: avatars.clone(), dependencies });
        }
        out.extend(avatars);
    }
    (out, complete)
}

/// Shared manifest/thumbnail filename contract, including Build & Test names.
pub fn filename(id: &str) -> String {
    let mut changed = false;
    let stem: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                changed = true;
                '_'
            }
        })
        .collect();
    if changed {
        let hash = id.as_bytes().iter().fold(0x811c9dc5u32, |h, b| (h ^ *b as u32).wrapping_mul(0x01000193));
        format!("{stem}-{hash:08x}")
    } else {
        stem
    }
}
pub fn local_thumbnail(id: &str) -> Option<PathBuf> {
    let p = crate::data_dir().join("Avatars").join(format!("{}.png", filename(id)));
    p.is_file().then_some(p)
}

#[cfg(test)]
mod read_completeness_tests {
    use super::*;

    #[test]
    fn missing_assets_is_not_a_successful_empty_scan() {
        let dir = std::env::temp_dir().join(format!("orbiters-read-check-{}", std::process::id()));
        let mut cache = SceneCache::default();
        assert!(!in_project_checked(&dir, &mut cache).1);
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        assert!(in_project_checked(&dir, &mut cache).1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A scene with one uploaded avatar or world, as Unity writes it.
    fn scene_with(name: &str, id: &str) -> String {
        let lines = [
            "%YAML 1.1",
            "%TAG !u! tag:unity3d.com,2011:",
            "--- !u!1 &100",
            "GameObject:",
            &format!("  m_Name: {name}"),
            "--- !u!4 &101",
            "Transform:",
            "  m_GameObject: {fileID: 100}",
            "  m_Father: {fileID: 0}",
            "--- !u!114 &102",
            "MonoBehaviour:",
            "  m_GameObject: {fileID: 100}",
            &format!("  blueprintId: {id}"),
        ];
        lines.join("\n") + "\n"
    }

    const AVATAR: &str = "avtr_11111111-2222-3333-4444-555555555555";
    const WORLD: &str = "wrld_99999999-2222-3333-4444-555555555555";

    /// A project that shows an uploaded avatar and an uploaded world goes on showing them when their scenes grow past
    /// what a preview reads (256 MB), and past any size: a scene's weight is in the meshes and terrains Unity writes
    /// into it, megabytes without a line end, and none of that is held. Refusing such a scene took the avatar off its
    /// project ("No uploaded avatar yet"), and with it the world's card, N4N0's project matching and the Blender links.
    #[test]
    fn a_project_goes_on_showing_its_avatar_and_world_when_their_scenes_grow_large() {
        let dir = std::env::temp_dir().join(format!("orbiters-large-scenes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        let (avatar, world) = (dir.join("Assets").join("Avatar.unity"), dir.join("Assets").join("World.unity"));
        std::fs::write(&avatar, scene_with("Fox", AVATAR)).unwrap();
        std::fs::write(&world, scene_with("VRCWorld", WORLD)).unwrap();
        let mut cache = SceneCache::default();
        let (known, complete) = in_project_checked(&dir, &mut cache);
        assert!(complete);
        let shown = |list: &[Avatar]| {
            let mut shown = list.iter().map(|a| (a.id.clone(), a.name.clone(), a.path.clone(), a.is_world())).collect::<Vec<_>>();
            shown.sort();
            shown
        };
        assert_eq!(shown(&known), [(AVATAR.to_string(), "Fox".to_string(), "Fox".to_string(), false), (WORLD.to_string(), "VRCWorld".to_string(), "VRCWorld".to_string(), true)]);

        // The avatar's scene past the preview's limit: a megabyte more than it, in one line with no end.
        let large = crate::scene::MAX_YAML_BYTES + (1 << 20);
        std::fs::OpenOptions::new().write(true).open(&avatar).unwrap().set_len(large).unwrap();
        // The world's: a mesh's megabytes in one line in the middle, and its world written after it.
        let mut grown = String::from("%YAML 1.1\n--- !u!43 &7\nMesh:\n  _typelessdata: ");
        grown.push_str(&"00ff3c".repeat(700_000));
        grown.push('\n');
        grown.push_str(scene_with("VRCWorld", WORLD).split_once("\n").unwrap().1);
        std::fs::write(&world, &grown).unwrap();
        assert!(std::fs::metadata(&avatar).unwrap().len() > crate::scene::MAX_YAML_BYTES && grown.len() > 4 << 20);

        let (still, complete) = in_project_checked(&dir, &mut cache);
        assert!(complete, "a large scene is read, not refused");
        assert_eq!(shown(&still), shown(&known), "what the project showed before its scenes grew");
        // And what discovery keeps for the project is what it found, both times.
        assert_eq!(cache.get(&avatar).map(|c| (c.len, c.avatars.len())), Some((large, 1)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Every document of a file, with every line of it: how scenes were read before [`Wanted`], kept as what it is
    /// checked against.
    fn whole(text: &str) -> Vec<Doc> {
        let mut out = vec![];
        let mut current = None;
        for l in text.lines() {
            if let Some(h) = l.strip_prefix("--- !u!") {
                out.extend(current.take());
                if let Some((kind, id)) = h.split_once(" &") {
                    current = Some(Doc { id: id.split_whitespace().next().unwrap_or_default().into(), kind: kind.parse().unwrap_or(0), lines: vec![] });
                }
            } else if let Some(d) = current.as_mut() {
                d.lines.push(l.into());
            }
        }
        out.extend(current);
        out
    }

    /// A scene made up at random of everything the readers meet: the kinds they read and others, the keys they read at
    /// several depths and under other keys' names, prefab instances' modifications of read and unread properties, and
    /// broken ones (a value before its property, a target that changes without one, a header without an ID).
    fn made_up_scene(seed: u64, end: &str) -> String {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        let mut lines: Vec<String> = vec!["%YAML 1.1".into(), "%TAG !u! tag:unity3d.com,2011:".into()];
        for doc in 0..60 {
            let kind = [1, 4, 224, 114, 1001, 23, 33, 43, 1001, 114, 1][next(11) as usize];
            lines.push(if next(25) == 0 { format!("--- !u!{kind}") } else { format!("--- !u!{kind} &{}{}", 100 + doc, if next(6) == 0 { " stripped" } else { "" }) });
            lines.push("Thing:".into());
            for _ in 0..next(14) {
                let indent = " ".repeat(2 * (1 + next(3) as usize));
                let reference = format!("{{fileID: {}, guid: {:032x}, type: 3}}", 100 + next(60), next(1 << 40));
                lines.push(match next(16) {
                    0 => format!("{indent}m_Name: Thing {}", next(9)),
                    1 => format!("{indent}m_GameObject: {{fileID: {}}}", 100 + next(60)),
                    2 => format!("{indent}m_Father: {{fileID: {}}}", if next(3) == 0 { 0 } else { 100 + next(60) }),
                    3 => format!("{indent}blueprintId: avtr_{:08x}-2222-3333-4444-555555555555", next(4)),
                    4 => format!("{indent}thumbnail: {reference}"),
                    5 => format!("{indent}thumbnailCamera: {{fileID: {}}}", next(3)),
                    6 => format!("{indent}m_SourcePrefab: {reference}"),
                    7 => format!("{indent}m_TransformParent: {{fileID: {}}}", 100 + next(60)),
                    8 => format!("{indent}m_CorrespondingSourceObject: {reference}"),
                    9 => format!("{indent}m_PrefabInstance: {{fileID: {}}}", 100 + next(60)),
                    10 => format!("{indent}- target: {reference}"),
                    11 => format!("{indent}propertyPath: {}", ["m_Name", "blueprintId", "m_LocalPosition.x", "m_IsActive", "m_Name "][next(5) as usize]),
                    12 => format!("{indent}value: {}", ["Renamed", "'It''s'", "\"Quoted\"", "avtr_00000001-2222-3333-4444-555555555555", ""][next(5) as usize]),
                    13 => format!("{indent}objectReference: {{fileID: 0}}"),
                    14 => format!("{indent}m_NameOfSomethingElse: {}", next(9)),
                    _ => format!("{indent}_typelessdata: {}", "ab".repeat(next(40) as usize)),
                });
            }
        }
        lines.join(end) + end
    }

    /// Everything the readers ask of a document of its kind (Model::of, direct, world_thumbnail and in_project_checked),
    /// whatever lines it holds.
    fn read_of(d: &Doc) -> Vec<Option<String>> {
        let keys: &[&str] = match d.kind {
            1 => &["m_Name", "m_CorrespondingSourceObject", "m_PrefabInstance"],
            4 | 224 => &["m_GameObject", "m_Father"],
            114 => &["blueprintId", "m_GameObject", "thumbnail"],
            _ => &["m_SourcePrefab", "m_TransformParent"],
        };
        let mut read: Vec<Option<String>> = keys.iter().map(|key| value(d, key)).collect();
        if d.kind == 114 {
            read.push(Some(d.lines.iter().any(|l| l.trim_start().starts_with("thumbnailCamera:")).to_string()));
        }
        if d.kind == 1001 {
            for property in ["m_Name", "blueprintId"] {
                let mut overridden: Vec<(String, String)> = overrides(d, property).into_iter().collect();
                overridden.sort();
                read.push(Some(format!("{overridden:?}")));
            }
        }
        read
    }

    /// The few lines kept of a scene read exactly as the whole of it did: the same documents of the kinds that are
    /// read, each reader's answer the same for every one of them, and so the same names, parents, avatars and world
    /// thumbnail. On made-up scenes of every shape the readers meet, and on the avatar scene of the contract tests.
    #[test]
    fn what_is_kept_of_a_scene_reads_as_the_whole_of_it() {
        let mut texts: Vec<String> = (1..=300).map(|seed| made_up_scene(seed, if seed % 3 == 0 { "\r\n" } else { "\n" })).collect();
        texts.push(include_str!("../tests/fixtures/avatar.unity").to_string());
        texts.push(scene_with("Fox", AVATAR));
        let (mut documents, mut kept_lines, mut all_lines, mut avatars) = (0, 0, 0, 0);
        for text in &texts {
            let all: Vec<Doc> = whole(text).into_iter().filter(|d| matches!(d.kind, 1 | 4 | 224 | 114 | 1001)).collect();
            let kept = Wanted::of_text(text);
            assert_eq!(kept.docs.iter().map(|d| (&d.id, d.kind)).collect::<Vec<_>>(), all.iter().map(|d| (&d.id, d.kind)).collect::<Vec<_>>());
            for (k, a) in kept.docs.iter().zip(&all) {
                assert_eq!(read_of(k), read_of(a), "document {} of kind {}:\n{}", a.id, a.kind, a.lines.join("\n"));
                assert!(k.lines.len() <= a.lines.len());
            }
            assert_eq!(kept.blueprint, text.contains("blueprintId"));
            documents += all.len();
            kept_lines += kept.docs.iter().map(|d| d.lines.len()).sum::<usize>();
            all_lines += text.lines().count();
            let (kept, all) = (Model::of(kept.docs), Model::of(all));
            assert_eq!((&kept.names, &kept.transforms), (&all.names, &all.transforms));
            assert_eq!(kept.direct("scene"), all.direct("scene"));
            assert_eq!(kept.world_thumbnail(), all.world_thumbnail());
            avatars += kept.direct("scene").len();
        }
        // The made-up scenes do reach what is read: documents, uploaded avatars, and lines that are not kept.
        assert!(documents > 5_000 && avatars > 100 && kept_lines * 2 < all_lines, "{documents} documents, {avatars} avatars, {kept_lines} of {all_lines} lines kept");
    }

    /// A file is read as its text is, whatever its line ends, with a line far longer than is looked at in its middle
    /// and no line end at its end; other text holds nothing and is no failure; what is not text cannot be read, and
    /// leaves the scan incomplete, so that the project keeps what it showed.
    #[test]
    fn a_file_is_read_a_line_at_a_time_as_its_text_is() {
        let dir = std::env::temp_dir().join(format!("orbiters-read-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        let file = dir.join("Assets").join("Scene.unity");
        for (seed, end) in [(7, "\n"), (8, "\r\n"), (9, "\n")] {
            let mut text = made_up_scene(seed, end);
            // A mesh in the middle, in one line of three times what is looked at of a line, then more of the scene.
            let middle = text.len() / 2;
            let at = middle + text[middle..].find("--- !u!").unwrap();
            text.insert_str(at, &format!("--- !u!43 &9{end}Mesh:{end}  _typelessdata: {}{end}", "0f".repeat(LINE_HEAD * 3 / 2)));
            let text = text.trim_end().to_string();
            std::fs::write(&file, &text).unwrap();
            let ReadFile::Yaml(read, blueprint) = read_file(&file).unwrap() else { panic!("not read as Unity's text") };
            let of_text = Model::new(&text);
            assert!(read.docs == of_text.docs && read.names == of_text.names && read.transforms == of_text.transforms, "seed {seed}");
            assert_eq!(blueprint, text.contains("blueprintId"));
            assert!(!read.docs.is_empty());
        }
        let files: [(&[u8], bool); 4] = [(b"", true), (b"just a note\nabout the scene", true), (b"\x00\x00\x00\x16binary scene", false), (b"\xff\xfe\x00binary", false)];
        for (bytes, text) in files {
            std::fs::write(&file, bytes).unwrap();
            assert_eq!(matches!(read_file(&file).unwrap(), ReadFile::OtherText), text, "{bytes:?}");
            assert_eq!(matches!(read_file(&file).unwrap(), ReadFile::Binary), !text, "{bytes:?}");
            assert_eq!(in_project_checked(&dir, &mut SceneCache::default()), (vec![], text), "{bytes:?}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The memory this process holds now, bytes.
    fn memory_now(system: &mut sysinfo::System) -> u64 {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        let pid = sysinfo::get_current_pid().unwrap();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::nothing().with_memory());
        system.process(pid).map_or(0, |p| p.memory())
    }

    /// Runs `f` and says how long it took and how much more memory the process held at its most while it ran
    /// (sampled every two milliseconds).
    fn measured<T>(f: impl FnOnce() -> T) -> (T, f64, u64) {
        let mut system = sysinfo::System::new();
        let before = memory_now(&mut system);
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = {
            let done = done.clone();
            std::thread::spawn(move || {
                let mut system = sysinfo::System::new();
                let mut most = 0;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    most = most.max(memory_now(&mut system));
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                most
            })
        };
        let start = std::time::Instant::now();
        let out = f();
        let seconds = start.elapsed().as_secs_f64();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        (out, seconds, watch.join().unwrap().saturating_sub(before))
    }

    /// Lab: the time and the memory reading a scene larger than a preview reads takes, a line at a time and, for
    /// contrast, whole as it used to be read. `ORBITERS_SCENE_MB` megabytes (300) of a world as Unity writes one: an
    /// uploaded world, objects with their transforms and components, and meshes of 2 MB of data in one line each.
    /// `cargo test --release -p orbiters-projects --lib -- --ignored --nocapture reading_a_large_scene`.
    #[test]
    #[ignore]
    fn reading_a_large_scene() {
        use std::io::Write;
        let megabytes: usize = std::env::var("ORBITERS_SCENE_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
        let dir = std::env::temp_dir().join(format!("orbiters-large-scene-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        let file = dir.join("Assets").join("World.unity");
        let (mut objects, mut meshes) = (0, 0);
        {
            let mut out = std::io::BufWriter::new(std::fs::File::create(&file).unwrap());
            out.write_all(scene_with("VRCWorld", WORLD).as_bytes()).unwrap();
            let mesh = "3fa0c1d2".repeat(250_000);
            let mut id = 1000u64;
            let mut written = 0usize;
            while written < megabytes << 20 {
                // A mesh, then two hundred objects that could each be the one that is looked for.
                let mut part = format!("--- !u!43 &{id}\nMesh:\n  m_Name: Mesh {id}\n  _typelessdata: {mesh}\n");
                meshes += 1;
                id += 1;
                for _ in 0..200 {
                    part.push_str(&format!(
                        "--- !u!1 &{id}\nGameObject:\n  m_Component:\n  - component: {{fileID: {}}}\n  m_Name: Object {id}\n  m_IsActive: 1\n--- !u!4 &{}\nTransform:\n  m_GameObject: {{fileID: {id}}}\n  m_LocalPosition: {{x: 0, y: 1.5, z: 0}}\n  m_Father: {{fileID: 101}}\n--- !u!23 &{}\nMeshRenderer:\n  m_GameObject: {{fileID: {id}}}\n  m_Materials:\n  - {{fileID: 2100000, guid: 0123456789abcdef0123456789abcdef, type: 2}}\n",
                        id + 1,
                        id + 1,
                        id + 2
                    ));
                    id += 3;
                    objects += 1;
                }
                out.write_all(part.as_bytes()).unwrap();
                written += part.len();
            }
        }
        let bytes = std::fs::metadata(&file).unwrap().len();
        println!("{} MB scene: {objects} objects, {meshes} meshes of 2 MB in one line", bytes >> 20);
        assert!(megabytes < 257 || bytes > crate::scene::MAX_YAML_BYTES);
        for run in 0..3 {
            let ((found, complete), seconds, memory) = measured(|| in_project_checked(&dir, &mut SceneCache::default()));
            assert!(complete && found.len() == 1 && found[0].id == WORLD, "{found:?}");
            println!("a line at a time, run {run}: {seconds:.2} s, {} MB more memory at most", memory >> 20);
        }
        let (found, seconds, memory) = measured(|| {
            let text = std::fs::read_to_string(&file).unwrap();
            Model::of(whole(&text)).direct("Assets/World.unity")
        });
        assert_eq!(found.len(), 1);
        println!("whole, as it was read: {seconds:.2} s, {} MB more memory at most", memory >> 20);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
