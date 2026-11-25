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

    /// The next line of the file, without its end. Whether it is a line some reader looks at: one that is kept, or one
    /// that decides what is (a modification's target, the property of one that is read).
    fn line(&mut self, l: &str) -> bool {
        if let Some(h) = l.strip_prefix("--- !u!") {
            self.end();
            self.current = h.split_once(" &").and_then(|(kind, id)| {
                let kind = kind.parse().unwrap_or(0);
                matches!(kind, 1 | 4 | 224 | 114 | 1001).then(|| Doc { id: id.split_whitespace().next().unwrap_or_default().into(), kind, lines: vec![] })
            });
            return false;
        }
        let Some(d) = self.current.as_mut() else { return false };
        let t = l.trim();
        let keys: &[&str] = match d.kind {
            1 => &["m_Name:", "m_CorrespondingSourceObject:", "m_PrefabInstance:"],
            4 | 224 => &["m_GameObject:", "m_Father:"],
            114 => &["blueprintId:", "m_GameObject:", "thumbnailCamera:", "thumbnail:"],
            _ => &["m_SourcePrefab:", "m_TransformParent:"],
        };
        if keys.iter().any(|k| t.starts_with(k)) {
            d.lines.push(l.into());
            return true;
        }
        if d.kind != 1001 {
            return false;
        }
        if t.starts_with("- target:") {
            self.target = Some(l.into());
            self.target_kept = false;
            true
        } else if let Some(property) = t.strip_prefix("propertyPath:") {
            self.keep_value = matches!(property.trim(), "m_Name" | "blueprintId");
            if self.keep_value {
                self.keep_target();
                self.current.as_mut().expect("being read").lines.push(l.into());
            }
            self.keep_value
        } else if self.keep_value && t.starts_with("value:") {
            self.keep_target();
            self.current.as_mut().expect("being read").lines.push(l.into());
            true
        } else {
            false
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
    /// It cannot be read here, and so is not known to hold nothing: it is not text (Asset Serialization is not Force
    /// Text), it holds bytes that are no text (a file damaged, or caught half written), or a line some reader looks at
    /// is longer than is looked at of a line. The scan it is part of is incomplete, and the project keeps what it showed.
    Unreadable,
}

/// How much of a line is looked at: the lines read are keys and short values, and a line longer than this is a mesh's or
/// a terrain's data, megabytes without a line end.
const LINE_HEAD: usize = 64 * 1024;

/// Whether bytes handed over a piece at a time are text (UTF-8) as a whole: a character a piece ends in the middle of is
/// finished with the next piece, so where the pieces are cut makes no difference.
#[derive(Default)]
struct Utf8 {
    /// The start of a character the last piece ended inside.
    partial: [u8; 4],
    held: usize,
}

impl Utf8 {
    /// Takes the next piece; `false` once the bytes so far cannot be text.
    fn take(&mut self, mut piece: &[u8]) -> bool {
        while self.held > 0 {
            let Some((next, rest)) = piece.split_first() else { return true };
            self.partial[self.held] = *next;
            self.held += 1;
            piece = rest;
            match std::str::from_utf8(&self.partial[..self.held]) {
                Ok(_) => self.held = 0,
                Err(e) if e.error_len().is_some() || self.held == self.partial.len() => return false,
                Err(_) => {}
            }
        }
        match std::str::from_utf8(piece) {
            Ok(_) => true,
            Err(e) if e.error_len().is_some() => false,
            Err(e) => {
                let start = &piece[e.valid_up_to()..];
                self.partial[..start.len()].copy_from_slice(start);
                self.held = start.len();
                true
            }
        }
    }

    /// Whether the bytes ended on a whole character.
    fn ended_whole(&self) -> bool {
        self.held == 0
    }
}

/// Whether `blueprintId` is written anywhere in bytes handed over a piece at a time, across their cuts too.
#[derive(Default)]
struct Mention {
    found: bool,
    /// The last bytes of what came before: one fewer than the word is long.
    before: Vec<u8>,
}

impl Mention {
    const WORD: &'static [u8] = b"blueprintId";

    fn take(&mut self, piece: &[u8]) {
        if self.found {
            return;
        }
        let most = Self::WORD.len() - 1;
        // Across the cut: the end of what came before, with as much of the start of this piece.
        let reach = piece.len().min(most);
        self.before.extend_from_slice(&piece[..reach]);
        self.found = self.before.windows(Self::WORD.len()).any(|w| w == Self::WORD) || piece.windows(Self::WORD.len()).any(|w| w == Self::WORD);
        // The end of everything so far, for the next cut.
        if piece.len() >= most {
            self.before.clear();
            self.before.extend_from_slice(&piece[piece.len() - most..]);
        } else {
            let more = self.before.len().saturating_sub(most);
            self.before.drain(..more);
        }
    }
}

/// A file being read, every byte of which is looked at on its way: whether the whole of it is text ([`Utf8`]), whatever
/// of it the reading keeps or passes over, and, for a prefab, whether it says `blueprintId` ([`Mention`]).
struct Looked<R> {
    inner: R,
    utf8: Utf8,
    text: bool,
    mention: Option<Mention>,
}

impl<R: std::io::Read> std::io::Read for Looked<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?;
        self.text = self.text && self.utf8.take(&buffer[..n]);
        if let Some(mention) = &mut self.mention {
            mention.take(&buffer[..n]);
        }
        Ok(n)
    }
}

/// Reads a scene or a prefab through, a line at a time, keeping what [`Wanted`] keeps. The memory it takes is a 64 KiB
/// read buffer, the head of the line being read, and the kept lines: a few an object, whatever the file weighs.
///
/// What it answers is what reading the whole file as text answered, or [`ReadFile::Unreadable`], never something else:
/// - Every byte is checked to be text, those of the lines passed over too. Read whole, a file with a byte that is no
///   text failed to be read, the scan was incomplete and the project kept its list. Converted leniently instead, a
///   damaged blueprint ID was no ID, and the scene answered, complete, that nothing was uploaded.
/// - A line is looked at up to [`LINE_HEAD`]. The rest of a mesh's data is passed over; a line some reader looks at
///   that is longer (a name, a blueprint ID behind 64 KiB of spaces) cannot be read short: the file is unreadable.
///
/// `mention`: also whether the file says `blueprintId` anywhere (asked of prefabs: one that never says it is not read
/// further into).
fn read_file_looking(path: &Path, mention: bool) -> std::io::Result<ReadFile> {
    use std::io::{BufRead, Read};
    let looked = Looked { inner: std::fs::File::open(path)?, utf8: Utf8::default(), text: true, mention: mention.then(Mention::default) };
    let mut reader = std::io::BufReader::with_capacity(LINE_HEAD, looked);
    let mut line = Vec::with_capacity(256);
    let mut wanted = Wanted::default();
    let (mut first, mut yaml, mut cut_short) = (true, false, false);
    loop {
        line.clear();
        let read = (&mut reader).take(LINE_HEAD as u64).read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        let mut cut = false;
        if line.last() == Some(&b'\n') {
            line.pop();
        } else if read == LINE_HEAD {
            // A line of just that length ends here; the rest of a longer one is passed over, never held (and looked
            // at on its way, as every byte is).
            match reader.fill_buf()?.first().copied() {
                None => {}
                Some(b'\n') => reader.consume(1),
                Some(_) => {
                    cut = true;
                    reader.skip_until(b'\n')?;
                }
            }
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if std::mem::take(&mut first) {
            yaml = line.starts_with(b"%YAML");
        }
        if !yaml {
            // Nothing is read of it: only whether all of it is text.
            continue;
        }
        // The head of a line cut inside a character is text up to that character; bytes that are no text at all are
        // the file's fault, found on their way in.
        let head = match std::str::from_utf8(&line) {
            Ok(head) => head,
            Err(e) => std::str::from_utf8(&line[..e.valid_up_to()]).unwrap_or_default(),
        };
        let read_by_some = wanted.line(head);
        // A cut line that is read, or whose head is all blank (what it starts with is past what was looked at).
        cut_short |= cut && (read_by_some || head.trim().is_empty());
    }
    let looked = reader.into_inner();
    if !looked.text || !looked.utf8.ended_whole() || cut_short {
        return Ok(ReadFile::Unreadable);
    }
    if !yaml {
        return Ok(ReadFile::OtherText);
    }
    wanted.end();
    Ok(ReadFile::Yaml(Model::of(wanted.docs), looked.mention.is_some_and(|m| m.found)))
}

/// A scene's file ([`read_file_looking`]).
fn read_file(path: &Path) -> std::io::Result<ReadFile> {
    read_file_looking(path, false)
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
    // What could not be read: of the walk, of the prefabs' and pictures' `.meta` files (any scene may need them), and of a scene.
    let (mut complete, mut metas) = (true, true);
    let assets = project.join("Assets");
    let mut scenes = vec![];
    // Prefabs' .meta files, read only when a scene that is read again places a prefab (PROJECTS-8): every scan read all of them,
    // thousands in an avatar project, though the scenes' cache answered for every scene.
    let mut prefab_metas = vec![];
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
            prefab_metas.push(p.to_path_buf());
        }
    }
    // Walked whole: a scene that is not there any more has no answer to keep (PRJ-12).
    if complete {
        let there: HashSet<&Path> = scenes.iter().map(PathBuf::as_path).collect();
        cache.retain(|p, _| !p.starts_with(&assets) || there.contains(p.as_path()));
    }
    let mut prefabs: Option<HashMap<String, PathBuf>> = None;
    let mut out = vec![];
    // (`None`: it cannot be read; `Some(None)`: it holds nothing that is looked for.)
    let mut prefab_models: HashMap<PathBuf, Option<Option<Model>>> = HashMap::new();
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
            Ok(ReadFile::Unreadable) | Err(_) => {
                complete = false;
                continue;
            }
        };
        let scene = p.strip_prefix(project).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        // This scene's answer is whole: another one that is not costs it nothing (PRJ-13).
        let mut whole = true;
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
            let Some(g) = value(d, "m_SourcePrefab").and_then(|s| guid(&s)) else { continue };
            let prefabs = prefabs.get_or_insert_with(|| {
                let mut found = HashMap::new();
                for meta in &prefab_metas {
                    if let Ok(s) = std::fs::read_to_string(meta) {
                        if let Some(g) = s.lines().find_map(|l| l.strip_prefix("guid: ")) {
                            found.insert(g.to_string(), meta.with_extension(""));
                        }
                    } else {
                        metas = false;
                    }
                }
                found
            });
            let Some(source) = prefabs.get(&g) else {
                // Perhaps one whose `.meta` could not be read.
                whole &= metas;
                continue;
            };
            dependencies.push((source.clone(), crate::discovery::fingerprint(source)));
            let prefab = prefab_models.entry(source.clone()).or_insert_with(|| match read_file_looking(source, true) {
                Ok(ReadFile::Yaml(model, blueprint)) => Some(blueprint.then_some(model)),
                Ok(ReadFile::OtherText) => Some(None),
                Ok(ReadFile::Unreadable) | Err(_) => None,
            });
            let Some(prefab) = prefab.as_ref() else {
                whole = false;
                continue;
            };
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
        if whole {
            cache.insert(p, CachedScene { modified, len: meta.len(), avatars: avatars.clone(), dependencies });
        }
        complete &= whole;
        out.extend(avatars);
    }
    (out, complete && metas)
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
            let ReadFile::Yaml(read, blueprint) = read_file_looking(&file, true).unwrap() else { panic!("not read as Unity's text") };
            let of_text = Model::new(&text);
            assert!(read.docs == of_text.docs && read.names == of_text.names && read.transforms == of_text.transforms, "seed {seed}");
            assert_eq!(blueprint, text.contains("blueprintId"));
            assert!(!read.docs.is_empty());
        }
        // Other text holds nothing and is no failure, zero bytes and all (they are text to whoever reads text). Bytes
        // that are no text make the file unreadable wherever they are, as reading it whole failed on them.
        let files: [(&[u8], bool); 6] = [
            (b"", true),
            (b"just a note\nabout the scene", true),
            (b"\x00\x00\x00\x16zero bytes are text", true),
            (b"\xff\xfe\x00binary", false),
            (b"a first line that reads as text\nand then \x9a\x3c, which is none", false),
            (b"text that ends inside a character: \xc3", false),
        ];
        for (bytes, text) in files {
            std::fs::write(&file, bytes).unwrap();
            assert_eq!(matches!(read_file(&file).unwrap(), ReadFile::OtherText), text, "{bytes:?}");
            assert_eq!(matches!(read_file(&file).unwrap(), ReadFile::Unreadable), !text, "{bytes:?}");
            assert_eq!(in_project_checked(&dir, &mut SceneCache::default()), (vec![], text), "{bytes:?}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A scene with bytes that are no text is not read as a scene with nothing uploaded: the scan is incomplete, and the
    /// project keeps what it showed. Read whole, such a file failed to be read; converted leniently, as the
    /// line-at-a-time reader first did, a damaged blueprint ID was no ID and the scan answered, complete, with an empty
    /// list, which discovery put in place of the project's own. Wherever the bytes are: in what is read, in a document
    /// that is not, past what is looked at of a mesh's line, and at the file's end inside a character (a scene caught
    /// half written).
    /// PROJECTS-8: prefabs' .meta files are read only for a scene that places a prefab and is read again: one that cannot be read
    /// leaves a scan of scenes without prefabs complete, and the scene's cache answering, nothing of them is read.
    #[test]
    fn prefabs_are_looked_up_only_for_a_scene_that_places_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Assets")).unwrap();
        std::fs::write(dir.path().join("Assets").join("Avatar.unity"), scene_with("Fox", AVATAR)).unwrap();
        // Not text: it cannot be read as a .meta file.
        std::fs::write(dir.path().join("Assets").join("Broken.prefab.meta"), b"guid: \xff\xfe\n").unwrap();
        let mut cache = SceneCache::default();
        let (known, complete) = in_project_checked(dir.path(), &mut cache);
        assert!(complete, "no scene places a prefab: the prefab's .meta is not read");
        assert_eq!(known.len(), 1);
        // A scene that places a prefab: the .meta files are read, and the one that cannot be leaves the scan incomplete.
        std::fs::write(dir.path().join("Assets").join("Placed.unity"), "%YAML 1.1\n--- !u!1001 &100\nPrefabInstance:\n  m_SourcePrefab: {fileID: 100100000, guid: 0123456789abcdef0123456789abcdef, type: 3}\n").unwrap();
        let (_, complete) = in_project_checked(dir.path(), &mut cache);
        assert!(!complete);
    }

    /// PRJ-12: a scene that is gone leaves the cache when its project was walked whole; another project's stays.
    #[test]
    fn a_scene_that_is_gone_leaves_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let (p, other) = (dir.path().join("P"), dir.path().join("Pq"));
        let mut cache = SceneCache::default();
        for (project, scene) in [(&p, "A.unity"), (&p, "B.unity"), (&other, "A.unity")] {
            std::fs::create_dir_all(project.join("Assets")).unwrap();
            std::fs::write(project.join("Assets").join(scene), scene_with("Fox", AVATAR)).unwrap();
            in_project_checked(project, &mut cache);
        }
        assert_eq!(cache.len(), 3);
        std::fs::remove_file(p.join("Assets/A.unity")).unwrap();
        in_project_checked(&p, &mut cache);
        assert!(cache.contains_key(&p.join("Assets").join("B.unity")) && cache.contains_key(&other.join("Assets").join("A.unity")));
        assert_eq!(cache.len(), 2, "what is not there any more is not kept");
        // A project that cannot be walked keeps what it had.
        std::fs::rename(&p, dir.path().join("Away")).unwrap();
        in_project_checked(&p, &mut cache);
        assert_eq!(cache.len(), 2);
    }

    /// PRJ-13: a scene that cannot be read costs its own answer, not those of the scenes after it.
    #[test]
    fn one_unreadable_scene_costs_no_other_its_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Assets")).unwrap();
        for name in ["a", "m", "z"] {
            std::fs::write(dir.path().join(format!("Assets/{name}.unity")), scene_with("Fox", AVATAR)).unwrap();
        }
        std::fs::write(dir.path().join("Assets/b.unity"), b"%YAML 1.1\n--- !u!114 &1\n  blueprintId: \xff\n").unwrap();
        let mut cache = SceneCache::default();
        let (found, complete) = in_project_checked(dir.path(), &mut cache);
        assert_eq!((found.len(), complete, cache.len()), (3, false, 3));
        // A scene that places a prefab whose `.meta` cannot be read is not known whole either.
        std::fs::write(dir.path().join("Assets/Broken.prefab.meta"), b"guid: \xff\xfe\n").unwrap();
        let placed = "%YAML 1.1\n--- !u!1001 &100\nPrefabInstance:\n  m_SourcePrefab: {fileID: 100100000, guid: 0123456789abcdef0123456789abcdef, type: 3}\n";
        std::fs::write(dir.path().join("Assets/c.unity"), placed).unwrap();
        assert!(!in_project_checked(dir.path(), &mut cache).1);
        assert!(cache.len() == 3 && !cache.contains_key(&dir.path().join("Assets").join("c.unity")));
    }

    #[test]
    fn a_damaged_scene_leaves_the_scan_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Assets")).unwrap();
        let scene = dir.path().join("Assets").join("Avatar.unity");
        let text = scene_with("Fox", AVATAR);
        std::fs::write(&scene, &text).unwrap();
        let mut cache = SceneCache::default();
        let (known, complete) = in_project_checked(dir.path(), &mut cache);
        assert!(complete && known.len() == 1);
        let kept = cache.get(&scene).cloned().expect("the scene's answer is kept");

        let id = text.find(AVATAR).unwrap();
        let mesh = format!("--- !u!43 &7\nMesh:\n  _typelessdata: {}\n", "0f".repeat(LINE_HEAD));
        let mut in_the_id = text.clone().into_bytes();
        in_the_id[id + 5] = 0xff;
        in_the_id.push(b'\n');
        let mut in_the_mesh = [text.as_bytes(), mesh.as_bytes()].concat();
        let near_its_end = in_the_mesh.len() - 10;
        in_the_mesh[near_its_end] = 0xff;
        let damaged: [(&str, Vec<u8>); 4] = [
            ("in its blueprint ID", in_the_id),
            ("in a document that is not read", [text.as_bytes(), b"--- !u!23 &9\nMeshRenderer:\n  m_Note: \xff\n"].concat()),
            ("past what is looked at of a mesh's line", in_the_mesh),
            ("at its end, inside a character", [text.as_bytes(), b"# caf\xc3"].concat()),
        ];
        for (place, bytes) in &damaged {
            std::fs::write(&scene, bytes).unwrap();
            assert!(matches!(read_file(&scene).unwrap(), ReadFile::Unreadable), "damaged {place}: read as text");
            let (_, complete) = in_project_checked(dir.path(), &mut cache);
            assert!(!complete, "damaged {place}: the project must keep its previous list");
            let now = cache.get(&scene).expect("the answer kept before");
            assert_eq!((now.len, &now.avatars), (kept.len, &kept.avatars), "damaged {place}: nothing is kept of an unreadable scene");
        }

        // Not damage: a character cut in two by how much of a line is looked at, in a line nobody reads.
        let note = format!("--- !u!43 &7\nMesh:\n  _note: {}\u{e9}{}\n--- !u!1 &200\nGameObject:\n  m_Name: After\n", "x".repeat(LINE_HEAD - 10), "y".repeat(500));
        assert_eq!(note.lines().nth(2).map(|l| (l.len() > LINE_HEAD, l.is_char_boundary(LINE_HEAD))), Some((true, false)), "the character must lie across the cut");
        let whole_characters = format!("{text}{note}");
        std::fs::write(&scene, &whole_characters).unwrap();
        let ReadFile::Yaml(read, _) = read_file(&scene).unwrap() else { panic!("a character across the cut taken for damage") };
        let of_text = Model::new(&whole_characters);
        assert!(read.docs == of_text.docs && read.names == of_text.names && read.names.values().any(|n| n == "After"));
        let (found, complete) = in_project_checked(dir.path(), &mut cache);
        assert!(complete && found.len() == 1 && found[0].id == AVATAR);
    }

    /// A line some reader looks at that is longer than is looked at of a line is never read short: the file is
    /// unreadable, the scan incomplete, and the project keeps its list. Cut and accepted, a long name came out
    /// shortened, and a blueprint ID behind 64 KiB of spaces came out as no avatar at all. A line of just the length
    /// looked at is whole. Long lines nobody reads change nothing: a mesh's data, the megabytes of a modification of
    /// some other property.
    #[test]
    fn a_line_that_is_read_is_never_read_short() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Assets")).unwrap();
        let scene = dir.path().join("Assets").join("Avatar.unity");
        let name_line = |name: &str| format!("  m_Name: {name}");
        let just = "N".repeat(LINE_HEAD - name_line("").len());
        assert_eq!(name_line(&just).len(), LINE_HEAD);
        // Read short: each of these is unreadable.
        let instance = |property: &str, value: &str| {
            format!("--- !u!1001 &300\nPrefabInstance:\n  m_Modification:\n    m_Modifications:\n    - target: {{fileID: 100, guid: 0123456789abcdef0123456789abcdef, type: 3}}\n      propertyPath: {property}\n      value: {value}\n      objectReference: {{fileID: 0}}\n")
        };
        let cut: [(&str, String); 6] = [
            ("a name a byte longer", scene_with(&format!("{just}N"), AVATAR)),
            ("a name far longer", scene_with(&"N".repeat(3 * LINE_HEAD), AVATAR)),
            ("a blueprint ID behind spaces", scene_with("Fox", AVATAR).replace("blueprintId: ", &format!("blueprintId:{}", " ".repeat(LINE_HEAD)))),
            ("a name with a character across the cut", scene_with(&format!("{}\u{e9}tail", &just[1..]), AVATAR)),
            ("a line whose start is past what is looked at", scene_with("Fox", AVATAR).replace("  m_Name: Fox", &format!("{}m_Name: Fox", " ".repeat(LINE_HEAD + 2)))),
            ("an overridden name longer than is looked at", format!("{}{}", scene_with("Fox", AVATAR), instance("m_Name", &"V".repeat(LINE_HEAD)))),
        ];
        for (what, text) in &cut {
            std::fs::write(&scene, text).unwrap();
            assert!(matches!(read_file(&scene).unwrap(), ReadFile::Unreadable), "{what}: read short");
            let (_, complete) = in_project_checked(dir.path(), &mut SceneCache::default());
            assert!(!complete, "{what}: the scan must be incomplete");
            // Read whole, it had the avatar: read short it must not answer anything else.
            assert_eq!(Model::new(text).direct("scene").len(), 1, "{what}");
        }
        // Whole: a line of just the length looked at, at a line's end and at the file's; and long lines nobody reads.
        let whole: [(&str, String); 4] = [
            ("a name of just the length looked at", scene_with(&just, AVATAR)),
            ("the same as the file's last line", format!("{}--- !u!1 &200\nGameObject:\n{}", scene_with("Fox", AVATAR), name_line(&just))),
            ("a mesh's data", format!("{}--- !u!43 &7\nMesh:\n  _typelessdata: {}\n  m_Name: A mesh\n", scene_with("Fox", AVATAR), "0f".repeat(2 * LINE_HEAD))),
            ("megabytes in a modification of another property", format!("{}{}{}", scene_with("Fox", AVATAR), instance("m_Text", &"T".repeat(1 << 20)), instance("m_Name", "Renamed"))),
        ];
        for (what, text) in &whole {
            std::fs::write(&scene, text).unwrap();
            let ReadFile::Yaml(read, _) = read_file(&scene).unwrap() else { panic!("{what}: not read") };
            let of_text = Model::new(text);
            assert!(read.docs == of_text.docs && read.names == of_text.names && read.transforms == of_text.transforms, "{what}");
            assert_eq!(read.direct("scene"), of_text.direct("scene"), "{what}");
            let (found, complete) = in_project_checked(dir.path(), &mut SceneCache::default());
            assert!(complete && found.len() == 1, "{what}");
        }
    }

    /// Text is told from what is not, and `blueprintId` found, wherever the bytes are cut into pieces: between any two
    /// bytes of a character of two, three or four bytes, and of the word. A file that ends inside a character is not
    /// text. And a prefab's file says the word when it is only written past what is looked at of a line, as its text
    /// read whole did: a prefab that never says it is not read further into.
    #[test]
    fn text_and_the_word_looked_for_are_found_across_any_cut() {
        let text = "h\u{e9}llo w\u{f6}rld \u{2713} \u{1f98a} m_blueprintId \u{1f98a}\u{2713}\u{e9}".as_bytes();
        for first in 0..=text.len() {
            for second in first..=text.len() {
                let pieces = [&text[..first], &text[first..second], &text[second..]];
                let (mut utf8, mut mention) = (Utf8::default(), Mention::default());
                assert!(pieces.iter().all(|p| utf8.take(p)) && utf8.ended_whole(), "cut at {first} and {second}");
                pieces.iter().for_each(|p| mention.take(p));
                assert!(mention.found, "the word cut at {first} and {second}");
                // The same bytes with one that is no text, and without the word's last letter.
                let mut bad = text.to_vec();
                bad[second.min(text.len() - 1)] = 0xff;
                let mut utf8 = Utf8::default();
                assert!(![&bad[..first], &bad[first..second], &bad[second..]].iter().all(|p| utf8.take(p)), "a byte that is no text, cut at {first} and {second}");
                let (without, mut mention) = (String::from_utf8_lossy(text).replace("blueprintId", "blueprintI_"), Mention::default());
                let without = without.as_bytes();
                [&without[..first], &without[first..second], &without[second..]].iter().for_each(|p| mention.take(p));
                assert!(!mention.found);
            }
        }
        let mut utf8 = Utf8::default();
        assert!(utf8.take(&text[..text.len() - 1]) && !utf8.ended_whole(), "ended inside its last character");

        // In a file: the word only past what is looked at of a line, across every cut of the 64 KiB the file is read by.
        let dir = tempfile::tempdir().unwrap();
        let prefab = dir.path().join("Thing.prefab");
        let head = "%YAML 1.1\n--- !u!43 &1\nMesh:\n  _typelessdata: ";
        for before_the_cut in 0..=Mention::WORD.len() {
            let filler = 2 * LINE_HEAD - head.len() - before_the_cut;
            let text = format!("{head}{}blueprintId{}\n--- !u!1 &2\nGameObject:\n  m_Name: Thing\n", "0".repeat(filler), "f".repeat(300));
            assert_eq!(text.find("blueprintId"), Some(2 * LINE_HEAD - before_the_cut));
            std::fs::write(&prefab, &text).unwrap();
            assert!(matches!(read_file_looking(&prefab, true).unwrap(), ReadFile::Yaml(_, true)), "the word {before_the_cut} bytes before a cut");
            assert!(matches!(read_file_looking(&prefab, false).unwrap(), ReadFile::Yaml(_, false)), "not looked for unless asked");
            std::fs::write(&prefab, text.replace("blueprintId", "blueprintI_")).unwrap();
            assert!(matches!(read_file_looking(&prefab, true).unwrap(), ReadFile::Yaml(_, false)));
        }
    }

    /// What reading one real file both ways came to.
    #[derive(Default)]
    struct RealFile {
        bytes: u64,
        /// Its longest line, bytes, and its longest line among those some reader looks at.
        longest_line: usize,
        longest_read_line: usize,
        /// Documents of the kinds that are read, uploaded avatars and worlds found in it, prefab instances whose
        /// name or blueprint ID is overridden, objects named by their prefab instance (a variant's root), a My World
        /// thumbnail.
        documents: usize,
        uploaded: usize,
        overridden: usize,
        named_by_instance: usize,
        thumbnail: bool,
        crlf: bool,
        /// Unity's text, other text, or not text.
        kind: &'static str,
        /// How the two readings differ, if they do.
        differs: Option<String>,
        /// Not opened or not read (another program holds it, or it went away).
        skipped: Option<String>,
    }

    /// Reads one real scene or prefab a line at a time ([`read_file`]) and whole, as it used to be read, and compares
    /// everything discovery takes from it.
    fn compare_real_file(path: &Path) -> RealFile {
        let mut out = RealFile::default();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                out.skipped = Some(e.to_string());
                return out;
            }
        };
        out.bytes = bytes.len() as u64;
        let read = match read_file_looking(path, true) {
            Ok(read) => read,
            Err(e) => {
                out.skipped = Some(e.to_string());
                return out;
            }
        };
        // As it was read: the whole file as text, or not at all (the scan was then incomplete).
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                out.kind = "not text";
                if !matches!(read, ReadFile::Unreadable) {
                    out.differs = Some("bytes that are no text: could not be read whole, read now".into());
                }
                return out;
            }
        };
        out.crlf = text.contains("\r\n");
        out.longest_line = text.lines().map(str::len).max().unwrap_or(0);
        if !text.starts_with("%YAML") {
            out.kind = "other text";
            if !matches!(read, ReadFile::OtherText) {
                out.differs = Some("text that is not Unity's, not taken for it".into());
            }
            return out;
        }
        out.kind = "Unity's text";
        // The lines some reader looks at, from the text itself.
        out.longest_read_line = Wanted::of_text(&text).docs.iter().flat_map(|d| &d.lines).map(String::len).max().unwrap_or(0);
        let (model, blueprint) = match read {
            ReadFile::Yaml(model, blueprint) => (model, blueprint),
            ReadFile::Unreadable => {
                out.differs = Some("unreadable a line at a time: a line that is read, or its start, is past what is looked at of a line".into());
                return out;
            }
            ReadFile::OtherText => {
                out.differs = Some("Unity's text, not read as it".into());
                return out;
            }
        };
        let all: Vec<Doc> = whole(&text).into_iter().filter(|d| matches!(d.kind, 1 | 4 | 224 | 114 | 1001)).collect();
        out.documents = all.len();
        out.overridden = all.iter().filter(|d| d.kind == 1001 && (!overrides(d, "m_Name").is_empty() || !overrides(d, "blueprintId").is_empty())).count();
        let mut differs = Vec::new();
        if model.docs.iter().map(|d| (&d.id, d.kind)).ne(all.iter().map(|d| (&d.id, d.kind))) {
            differs.push(format!("{} documents kept of {}", model.docs.len(), all.len()));
        } else {
            for (kept, all) in model.docs.iter().zip(&all) {
                if read_of(kept) != read_of(all) {
                    differs.push(format!("document {} of kind {}: {:?} kept, {:?} whole", all.id, all.kind, read_of(kept), read_of(all)));
                    break;
                }
            }
        }
        if blueprint != text.contains("blueprintId") {
            differs.push(format!("blueprintId written in it: {blueprint} a line at a time, {} whole", !blueprint));
        }
        let of_all = Model::of(all);
        out.named_by_instance = of_all.docs.iter().filter(|d| d.kind == 1 && value(d, "m_Name").is_none() && of_all.names.contains_key(&d.id)).count();
        if model.names != of_all.names {
            differs.push("the objects' names".into());
        }
        if model.transforms != of_all.transforms {
            differs.push("the objects' parents".into());
        }
        let (found, found_whole) = (model.direct("scene"), of_all.direct("scene"));
        if found != found_whole {
            differs.push(format!("uploaded avatars and worlds: {found:?} a line at a time, {found_whole:?} whole"));
        }
        if model.world_thumbnail() != of_all.world_thumbnail() {
            differs.push("the world's thumbnail".into());
        }
        out.uploaded = found_whole.len();
        out.thumbnail = of_all.world_thumbnail().is_some();
        out.differs = (!differs.is_empty()).then(|| differs.join("; "));
        out
    }

    /// Lab: the line-at-a-time reader held against the whole-file reading on real Unity files. Every `.unity` and
    /// `.prefab` under `ORBITERS_UNITY_FOLDER` (a folder of projects, or one project), outside `Library`, `Temp`, `Logs`
    /// and `obj`, is read both ways, and everything discovery takes from it compared: per document what each reader
    /// returns, then the names, parents, uploaded avatars and worlds with their blueprint IDs, and the world's
    /// thumbnail. Read-only: nothing is written, and a file another program holds is passed over and counted. Fails
    /// when a file reads differently, after saying which and how.
    /// `ORBITERS_UNITY_FOLDER=<folder> cargo test --release -p orbiters-projects --lib -- --ignored --nocapture real_scenes_and_prefabs`.
    #[test]
    #[ignore]
    fn real_scenes_and_prefabs_read_as_the_whole_of_them() {
        let Some(folder) = std::env::var_os("ORBITERS_UNITY_FOLDER").map(PathBuf::from) else {
            println!("ORBITERS_UNITY_FOLDER names no folder: nothing read");
            return;
        };
        let start = std::time::Instant::now();
        let files: Vec<PathBuf> = walkdir::WalkDir::new(&folder)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !(e.file_type().is_dir() && ["library", "temp", "logs", "obj"].contains(&e.file_name().to_string_lossy().to_ascii_lowercase().as_str())))
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "unity" || x == "prefab"))
            .map(|e| e.into_path())
            .collect();
        println!("{} scenes and prefabs under {} (listed in {:.0} s)", files.len(), folder.display(), start.elapsed().as_secs_f32());
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results = std::sync::Mutex::new(Vec::with_capacity(files.len()));
        std::thread::scope(|scope| {
            for _ in 0..6 {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let result = compare_real_file(path);
                    results.lock().unwrap().push((i, result));
                });
            }
        });
        let mut results = results.into_inner().unwrap();
        results.sort_by_key(|(i, _)| *i);
        let of = |i: &usize| files[*i].strip_prefix(&folder).unwrap_or(&files[*i]).display().to_string();
        let read: Vec<&(usize, RealFile)> = results.iter().filter(|(_, r)| r.skipped.is_none()).collect();
        let count = |kind: &str| read.iter().filter(|(_, r)| r.kind == kind).count();
        let sum = |f: fn(&RealFile) -> usize| read.iter().map(|(_, r)| f(r)).sum::<usize>();
        println!("read both ways: {} files, {} MB, in {:.0} s", read.len(), read.iter().map(|(_, r)| r.bytes).sum::<u64>() >> 20, start.elapsed().as_secs_f32());
        println!("  Unity's text {}, other text {}, not text {}; with Windows line ends {}", count("Unity's text"), count("other text"), count("not text"), read.iter().filter(|(_, r)| r.crlf).count());
        println!(
            "  documents of the kinds read {}, uploaded avatars and worlds {}, prefab instances with a name or blueprint ID overridden {}, objects named by their prefab instance {}, My World thumbnails {}",
            sum(|r| r.documents),
            sum(|r| r.uploaded),
            sum(|r| r.overridden),
            sum(|r| r.named_by_instance),
            read.iter().filter(|(_, r)| r.thumbnail).count()
        );
        if let Some((i, r)) = read.iter().max_by_key(|(_, r)| r.bytes) {
            println!("  largest file: {} MB, {}", r.bytes >> 20, of(i));
        }
        if let Some((i, r)) = read.iter().max_by_key(|(_, r)| r.longest_line) {
            println!("  longest line: {} bytes, {} ({} files have a line longer than the {LINE_HEAD} bytes looked at)", r.longest_line, of(i), read.iter().filter(|(_, r)| r.longest_line > LINE_HEAD).count());
        }
        if let Some((i, r)) = read.iter().max_by_key(|(_, r)| r.longest_read_line) {
            let near = read.iter().filter(|(_, r)| r.longest_read_line > LINE_HEAD / 10).count();
            println!(
                "  longest line that is read: {} bytes of the {LINE_HEAD} looked at, {} ({near} files have one longer than a tenth of that; a longer one than all of it makes its file unreadable)",
                r.longest_read_line,
                of(i)
            );
        }
        // By project: the folder's own subfolders.
        let mut projects: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for (i, r) in &read {
            let project = files[*i].strip_prefix(&folder).ok().and_then(|p| p.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            let entry = projects.entry(project).or_default();
            entry.0 += 1;
            entry.1 += r.uploaded;
        }
        println!("  {} projects: {}", projects.len(), projects.iter().map(|(name, (files, uploaded))| format!("{name} ({files} files, {uploaded} uploaded)")).collect::<Vec<_>>().join(", "));
        let skipped: Vec<&(usize, RealFile)> = results.iter().filter(|(_, r)| r.skipped.is_some()).collect();
        println!("passed over (held by another program, or gone): {}", skipped.len());
        for (i, r) in skipped.iter().take(20) {
            println!("  {}: {}", of(i), r.skipped.as_deref().unwrap_or_default());
        }
        let differing: Vec<&&(usize, RealFile)> = read.iter().filter(|(_, r)| r.differs.is_some()).collect();
        println!("read differently: {}", differing.len());
        for (i, r) in differing.iter().take(100) {
            println!("  {} ({} bytes): {}", of(i), r.bytes, r.differs.as_deref().unwrap_or_default());
        }
        assert!(differing.is_empty(), "{} of {} real files read differently a line at a time", differing.len(), read.len());
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
