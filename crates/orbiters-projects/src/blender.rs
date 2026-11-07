//! The Blender files a project's avatars and worlds were linked to with Orbiters' Blender tools, and the Blender to
//! open them with.
//!
//! Mixer links a scene to a .blend: once "Open scene in Blender" was used, `Assets/Orbiters/Mixer/Link/<scene
//! GUID>.asset` keeps its `blendPath` (absolute), and the .blend Mixer made is in
//! `Assets/Orbiters/Mixer/Blender/<scene GUID>/blend~/`. MCB, before it began going through Mixer (1.12.11), linked an
//! avatar's custom base: each Magic Sync session left `Library/MCB/BlenderSync/<session>/session.json` (its
//! `customBaseGlobalId`, a GlobalObjectId naming the scene, and `blenderProjectAbsolutePath` or
//! `blenderProjectUnityPath`) and, for a .blend picked by hand, Blender's `blender_heartbeat.json` (`blendFile`); the
//! .blend files MCB made are `Assets/MCB/blenderProjects/<avatar root>_<hash>/(blend~/)<avatar root>.blend`. Neither
//! records a blueprint ID: a link goes to the avatar whose root it names, else to the avatars and worlds of the scene
//! it was made from. Only files that exist are listed; nothing is written.
use orbiters_i18n::tr;
use crate::avatars::Avatar;
use crate::discovery;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Mixer,
    Mcb,
}
impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mixer => "Mixer",
            Self::Mcb => "MCB",
        }
    }
}

/// A .blend file linked to a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub blend: PathBuf,
    /// The scene it was linked from, as [`Avatar::scene`] (project-relative, forward slashes), when known.
    pub scene: Option<String>,
    /// The name of the avatar root it was made for (MCB's), when known.
    pub root: Option<String>,
    pub tool: Tool,
    /// Unix seconds of the .blend's last change.
    pub modified: u64,
    scene_guid: Option<String>,
}

impl Link {
    /// A link as [`links`] finds them (stand-ins for screenshots).
    pub fn new(blend: PathBuf, tool: Tool, scene: Option<String>, root: Option<String>, modified: u64) -> Self {
        Self { blend, scene, root, tool, modified, scene_guid: None }
    }
}

/// The .blend files linked in `project`, newest first, each once; `scenes` (project-relative, as [`Avatar::scene`])
/// are the scenes the links' GUIDs are looked for in. Reads small files only; call on a worker.
pub fn links(project: &Path, scenes: &[&str]) -> Vec<Link> {
    let guids: HashMap<String, String> = scenes.iter().filter_map(|s| Some((scene_guid(project, s)?, s.to_string()))).collect();
    let mut found = vec![];
    mixer(project, &mut found);
    mcb(project, &mut found);
    let mut out: Vec<Link> = vec![];
    for mut l in found {
        if l.scene.is_none() {
            l.scene = l.scene_guid.as_ref().and_then(|g| guids.get(g)).cloned();
        }
        let key = discovery::key(&l.blend);
        match out.iter_mut().find(|o| discovery::key(&o.blend) == key) {
            Some(o) => {
                o.scene = o.scene.take().or(l.scene);
                o.root = o.root.take().or(l.root);
                o.scene_guid = o.scene_guid.take().or(l.scene_guid);
            }
            None if l.blend.is_file() => {
                l.modified = discovery::stamp(&l.blend);
                out.push(l);
            }
            None => {}
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.blend.cmp(&b.blend)));
    out
}

/// The links of `target` (an avatar or a world): those made for its root, and those of its scene that name no root.
pub fn linked<'a>(links: &'a [Link], target: &Avatar) -> Vec<&'a Link> {
    links.iter().filter(|l| matches(l, target)).collect()
}

fn matches(l: &Link, a: &Avatar) -> bool {
    let same_scene = l.scene.as_deref().map(|s| s.replace('\\', "/").eq_ignore_ascii_case(&a.scene.replace('\\', "/")));
    match (&l.root, same_scene) {
        (_, Some(false)) => false,
        (Some(root), _) => {
            let leaf = a.path.rsplit('/').next().unwrap_or_default();
            [a.name.as_str(), leaf].iter().any(|n| !n.is_empty() && (n.eq_ignore_ascii_case(root) || file_safe(n).eq_ignore_ascii_case(root)))
        }
        (None, Some(true)) => true,
        (None, None) => false,
    }
}

/// A name as MCB puts it in a folder's name: what Windows refuses in file names replaced by `_`.
fn file_safe(name: &str) -> String {
    name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect()
}

fn is_guid(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A scene's GUID, from its `.meta` file.
fn scene_guid(project: &Path, scene: &str) -> Option<String> {
    let text = std::fs::read_to_string(project.join(format!("{scene}.meta"))).ok()?;
    text.lines().find_map(|l| l.trim().strip_prefix("guid:")).map(|g| g.trim().to_ascii_lowercase()).filter(|g| is_guid(g))
}

/// The scene GUID of a scene object's GlobalObjectId (`GlobalObjectId_V1-2-<scene GUID>-<file ID>-<prefab ID>`).
fn global_id_scene(id: &str) -> Option<String> {
    let parts: Vec<&str> = id.split('-').collect();
    (parts.len() >= 3 && parts[1] == "2" && is_guid(parts[2])).then(|| parts[2].to_ascii_lowercase())
}

/// A YAML field's value as Unity writes it (plain, 'single' or "double" quoted).
fn yaml_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let v = text.lines().find_map(|l| l.trim().strip_prefix(prefix.as_str()))?.trim();
    Some(if v.starts_with('"') {
        serde_json::from_str(v).unwrap_or_else(|_| v.trim_matches('"').into())
    } else if v.starts_with('\'') {
        v.trim_matches('\'').replace("''", "'")
    } else {
        v.to_string()
    })
}

/// A path a tool wrote: absolute, or relative to the project.
fn resolve(project: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        project.join(p)
    }
}

/// The .blend files in `dir` (Blender's .blend1 backups left out).
fn blends(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("blend")) && p.is_file()).collect()
}

fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn text(v: &serde_json::Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn mixer(project: &Path, out: &mut Vec<Link>) {
    let base = project.join("Assets/Orbiters/Mixer");
    let link = |blend: PathBuf, guid: Option<String>| Link { blend, scene: None, root: None, tool: Tool::Mixer, modified: 0, scene_guid: guid };
    if let Ok(entries) = std::fs::read_dir(base.join("Link")) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "asset") {
                continue;
            }
            let Some(blend) = std::fs::read_to_string(&p).ok().and_then(|t| yaml_value(&t, "blendPath")).filter(|b| !b.is_empty()) else { continue };
            let guid = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).filter(|g| is_guid(g));
            out.push(link(resolve(project, &blend), guid));
        }
    }
    // The .blend files Mixer made, in their place, linked or not yet.
    if let Ok(entries) = std::fs::read_dir(base.join("Blender")) {
        for e in entries.flatten() {
            let guid = e.file_name().to_string_lossy().to_ascii_lowercase();
            if !is_guid(&guid) {
                continue;
            }
            for blend in blends(&e.path().join("blend~")).into_iter().chain(blends(&e.path())) {
                out.push(link(blend, Some(guid.clone())));
            }
        }
    }
}

/// The avatar root an MCB .blend was made for: its folder under `blenderProjects` is `<root>_<12 hex>` (with `_2`,
/// `_3`… when the meshes differed), else the file is named after it.
fn mcb_root(blend: &Path) -> Option<String> {
    let parts: Vec<String> = blend.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let at = parts.iter().position(|p| p.eq_ignore_ascii_case("blenderProjects"))?;
    let folder = parts.get(at + 1)?;
    let name = match folder.rsplit_once('_') {
        Some((head, n)) if !n.is_empty() && n.len() < 4 && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => folder.as_str(),
    };
    match name.rsplit_once('_') {
        Some((root, hash)) if hash.len() == 12 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !root.is_empty() => Some(root.to_string()),
        _ => blend.file_stem().map(|s| s.to_string_lossy().into_owned()),
    }
}

fn mcb(project: &Path, out: &mut Vec<Link>) {
    let link = |blend: PathBuf, guid: Option<String>| {
        let root = mcb_root(&blend);
        Link { blend, scene: None, root, tool: Tool::Mcb, modified: 0, scene_guid: guid }
    };
    if let Ok(sessions) = std::fs::read_dir(project.join("Library/MCB/BlenderSync")) {
        for s in sessions.flatten() {
            let dir = s.path();
            let session = read_json(&dir.join("session.json"));
            let guid = text(&session["customBaseGlobalId"]).and_then(global_id_scene);
            let absolute = text(&session["blenderProjectAbsolutePath"]).map(PathBuf::from).filter(|p| p.is_file());
            if let Some(blend) = absolute.or_else(|| text(&session["blenderProjectUnityPath"]).map(|p| resolve(project, p))) {
                out.push(link(blend, guid.clone()));
            }
            // A .blend picked by hand: only Blender's heartbeat names it.
            if let Some(blend) = text(&read_json(&dir.join("blender_heartbeat.json"))["blendFile"]) {
                out.push(link(resolve(project, blend), guid.clone()));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(project.join("Assets/MCB/blenderProjects")) {
        for e in entries.flatten() {
            for blend in blends(&e.path().join("blend~")).into_iter().chain(blends(&e.path())) {
                out.push(link(blend, None));
            }
        }
    }
}

/// A Blender found on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blender {
    pub exe: PathBuf,
    /// Its version (major, minor): the newest version folder its installation keeps beside the program.
    pub version: Option<(u32, u32)>,
}

impl Blender {
    pub fn label(&self) -> String {
        match self.version {
            Some((a, b)) => format!("Blender {a}.{b}"),
            None => "Blender".into(),
        }
    }
}

/// "4.2" → (4, 2).
fn parse_version(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// The version of the Blender whose program is `exe`: Blender keeps its scripts and data in a folder named after its
/// version beside the program (Steam's leaves older ones behind: the newest is the one installed), on the Mac in its
/// bundle's `Contents/Resources`.
fn version_of(exe: &Path) -> Option<(u32, u32)> {
    let beside = exe.parent()?;
    let resources = beside.parent().map(|contents| contents.join("Resources")).filter(|_| cfg!(target_os = "macos") && beside.ends_with("Contents/MacOS"));
    let entries = std::fs::read_dir(resources.as_deref().unwrap_or(beside)).ok()?;
    entries.flatten().filter(|e| e.path().is_dir()).filter_map(|e| parse_version(&e.file_name().to_string_lossy())).max()
}

/// The program of a shell command (`"C:\…\blender.exe" "%1"`, or unquoted up to `.exe`).
#[cfg_attr(not(windows), allow(dead_code))]
fn command_program(command: &str) -> Option<PathBuf> {
    let c = command.trim();
    let program = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        let lower = c.to_ascii_lowercase();
        let end = lower.find(".exe").map(|i| i + 4).unwrap_or_else(|| c.find(' ').unwrap_or(c.len()));
        &c[..end]
    };
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// The program .blend files open with (`HKEY_CLASSES_ROOT\.blend`'s ProgID, its `shell\open\command`).
fn associated() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CLASSES_ROOT;
        let root = winreg::RegKey::predef(HKEY_CLASSES_ROOT);
        let prog: String = root.open_subkey(".blend").ok()?.get_value("").ok()?;
        let command: String = root.open_subkey(format!(r"{}\shell\open\command", prog.trim())).ok()?.get_value("").ok()?;
        command_program(&command)
    }
    // Launch Services answers about a file that exists: an empty one of that kind, in its own temporary folder.
    #[cfg(target_os = "macos")]
    {
        let folder = std::env::temp_dir().join(format!("orbiters-blend-{}", std::process::id()));
        let file = folder.join("association.blend");
        std::fs::create_dir_all(&folder).ok()?;
        std::fs::write(&file, b"").ok()?;
        let app = crate::mac::default_app(&file);
        let _ = std::fs::remove_dir_all(&folder);
        app.map(|app| crate::mac::bundle_program(&app))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    None
}

/// Steam's libraries: its own folder and those `libraryfolders.vdf` lists.
fn steam_libraries() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let Some(steam) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Valve\Steam")
            .ok()
            .and_then(|k| k.get_value::<String, _>("SteamPath").ok())
            .map(PathBuf::from)
        else {
            return vec![];
        };
        let mut out = vec![steam.clone()];
        if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) {
            out.extend(vdf_paths(&vdf));
        }
        out
    }
    #[cfg(target_os = "macos")]
    {
        let Some(steam) = dirs::config_dir().map(|d| d.join("Steam")) else { return vec![] };
        let mut out = vec![steam.clone()];
        if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) {
            out.extend(vdf_paths(&vdf));
        }
        out
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    vec![]
}

/// The `"path"` values of Steam's `libraryfolders.vdf`.
fn vdf_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("\"path\"")?.trim();
            Some(PathBuf::from(rest.trim_matches('"').replace("\\\\", "\\")))
        })
        .collect()
}

/// The Blender applications in a folder of applications: `Blender.app`, `Blender 4.2.app`.
#[cfg(target_os = "macos")]
fn apps_in(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return vec![] };
    let mut apps: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("blender") && name.ends_with(".app")
        })
        .map(|e| e.path())
        .collect();
    apps.sort();
    apps
}

/// The Blenders on this computer, the one .blend files open with first: then Steam's (in each library) and those under
/// Program Files\Blender Foundation; on the Mac those of the Applications folders, then any other Spotlight knows. Each
/// once.
pub fn installs() -> Vec<Blender> {
    let mut exes: Vec<PathBuf> = associated().into_iter().collect();
    #[cfg(not(target_os = "macos"))]
    {
        exes.extend(steam_libraries().into_iter().map(|l| l.join("steamapps/common/Blender/blender.exe")));
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            if let Ok(entries) = std::fs::read_dir(PathBuf::from(pf).join("Blender Foundation")) {
                exes.extend(entries.flatten().map(|e| e.path().join("blender.exe")));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut apps: Vec<PathBuf> = steam_libraries().into_iter().map(|l| l.join("steamapps/common/Blender/Blender.app")).collect();
        apps.extend(apps_in(Path::new("/Applications")));
        apps.extend(dirs::home_dir().map(|h| apps_in(&h.join("Applications"))).unwrap_or_default());
        apps.extend(crate::mac::spotlight("kMDItemCFBundleIdentifier == 'org.blenderfoundation.blender'")); // i18n: not shown (a macOS query and a folder name)
        exes.extend(apps.iter().map(|app| crate::mac::bundle_program(app)));
    }
    let mut out: Vec<Blender> = vec![];
    for exe in exes {
        if exe.is_file() && !out.iter().any(|b| discovery::key(&b.exe) == discovery::key(&exe)) {
            out.push(Blender { version: version_of(&exe), exe });
        }
    }
    out
}

/// The Blender versions whose user settings (`config`, Blender's `%APPDATA%\Blender Foundation\Blender`, on the Mac
/// `~/Library/Application Support/Blender`) have Mixer's or MCB's add-on, as an add-on or an extension.
fn with_addons(config: &Path) -> Vec<(u32, u32)> {
    let Ok(entries) = std::fs::read_dir(config) else { return vec![] };
    let has = |dir: &Path| ["orbiters_mixer", "mcb_blender"].iter().any(|a| dir.join(a).is_dir());
    entries
        .flatten()
        .filter_map(|e| {
            let v = parse_version(&e.file_name().to_string_lossy())?;
            let dir = e.path();
            let extensions = std::fs::read_dir(dir.join("extensions")).map(|r| r.flatten().any(|x| has(&x.path()))).unwrap_or(false);
            (has(&dir.join("scripts/addons")) || extensions).then_some(v)
        })
        .collect()
}

/// The Blender to open Orbiters' links with: the newest that has Mixer's or MCB's add-on, else the newest, else the
/// one .blend files open with (the first). The association alone can be an old Blender that would not open a file
/// saved by a newer one.
pub fn choose<'a>(installs: &'a [Blender], with_addons: &[(u32, u32)]) -> Option<&'a Blender> {
    let newest = |it: &mut dyn Iterator<Item = &'a Blender>| it.filter(|b| b.version.is_some()).max_by_key(|b| b.version);
    newest(&mut installs.iter().filter(|b| b.version.is_some_and(|v| with_addons.contains(&v))))
        .or_else(|| newest(&mut installs.iter()))
        .or_else(|| installs.first())
}

/// The Blender [`open`] uses, if one is found.
pub fn find() -> Option<Blender> {
    let settings = if cfg!(target_os = "macos") { "Blender" } else { "Blender Foundation/Blender" }; // i18n: not shown (a macOS query and a folder name)
    let config = dirs::config_dir().map(|d| d.join(settings)).map(|d| with_addons(&d)).unwrap_or_default();
    let all = installs();
    choose(&all, &config).cloned()
}

/// Opens `blend` in Blender ([`find`]), else with whatever the shell opens .blend files with. What opened it, to
/// tell the person.
pub fn open(blend: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(blend.is_file(), "{} is not there anymore.", blend.display());
    #[cfg(target_os = "macos")]
    {
        if let Some(b) = find() {
            if open_on_mac(&b.exe, blend).status().is_ok_and(|s| s.success()) {
                return Ok(b.label());
            }
        }
        let answer = crate::mac::open_command().arg(blend).output()?;
        anyhow::ensure!(answer.status.success(), "No program here opens .blend files. Install Blender from blender.org.");
        Ok(tr("the program .blend files open with").into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(b) = find() {
            let mut c = crate::hidden_command(&b.exe);
            c.arg(blend);
            if let Some(dir) = blend.parent() {
                c.current_dir(dir);
            }
            if c.spawn().is_ok() {
                return Ok(b.label());
            }
        }
        crate::reveal(blend)?;
        Ok(tr("the program .blend files open with").into())
    }
}

/// What opens `blend` in the Blender whose program is `exe`: a new one through the system when it is in a bundle (a Blender
/// already running would otherwise drop what it has open for this file), else that program with the file.
#[cfg(target_os = "macos")]
fn open_on_mac(exe: &Path, blend: &Path) -> std::process::Command {
    match crate::mac::enclosing_app(exe) {
        Some(app) => {
            let mut c = crate::mac::start_app(&app);
            c.arg(blend);
            c
        }
        None => {
            let mut c = crate::hidden_command(exe);
            c.arg(blend);
            if let Some(dir) = blend.parent() {
                c.current_dir(dir);
            }
            c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, text: &str) {
        let p = root.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const CANINE: &str = "af5575049eafbc04fa0023ba7cd78bd7";
    const WORLD: &str = "0b4e0f0e2c8d4a51b6f0a4c7d1e2f3a4";

    fn avatar(id: &str, name: &str, scene: &str, path: &str) -> Avatar {
        Avatar { id: id.into(), name: name.into(), scene: scene.into(), path: path.into(), ..Default::default() }
    }

    #[test]
    fn links_come_from_mixer_and_mcb_records_and_go_to_their_avatars() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("MCB Test");
        let outside = temp.path().join("Blender/UltiZear.blend");
        write(temp.path(), "Blender/UltiZear.blend", "BLENDER");
        write(&p, "Assets/MasculineCanine/Canine.unity.meta", &format!("fileFormatVersion: 2\nguid: {CANINE}\nDefaultImporter:\n"));
        write(&p, "Assets/Scenes/World.unity.meta", &format!("fileFormatVersion: 2\nguid: {WORLD}\n"));
        // Mixer: the scene's link asset and the .blend in its place.
        let mixer_blend = p.join(format!("Assets/Orbiters/Mixer/Blender/{WORLD}/blend~/World.blend"));
        write(&p, &format!("Assets/Orbiters/Mixer/Blender/{WORLD}/blend~/World.blend"), "BLENDER");
        write(&p, &format!("Assets/Orbiters/Mixer/Blender/{WORLD}/blend~/World.blend1"), "backup");
        write(
            &p,
            &format!("Assets/Orbiters/Mixer/Link/{WORLD}.asset"),
            &format!("%YAML 1.1\n--- !u!114 &11400000\nMonoBehaviour:\n  m_Name: World link\n  excludedIds: []\n  blendPath: {}\n", mixer_blend.display()),
        );
        // MCB: a session with the .blend it made, another whose .blend was picked by hand, and a made one no session names.
        let made = p.join("Assets/MCB/blenderProjects/MasculineCanine_a4d47dec0b66/blend~/MasculineCanine.blend");
        write(&p, "Assets/MCB/blenderProjects/MasculineCanine_a4d47dec0b66/blend~/MasculineCanine.blend", "BLENDER");
        write(&p, "Assets/MCB/blenderProjects/Alphazear_e5d5433dd397_2/Alphazear.blend", "BLENDER");
        let global = format!("GlobalObjectId_V1-2-{CANINE}-4477832160588500237-0");
        write(
            &p,
            "Library/MCB/BlenderSync/one/session.json",
            &serde_json::json!({"customBaseGlobalId": global, "customBaseName": "UltiPaw", "blenderProjectAbsolutePath": made, "blenderProjectUnityPath": "Assets/MCB/blenderProjects/MasculineCanine_a4d47dec0b66/blend~/MasculineCanine.blend"}).to_string(),
        );
        write(&p, "Library/MCB/BlenderSync/two/session.json", &serde_json::json!({"customBaseGlobalId": global, "blenderProjectAbsolutePath": null}).to_string());
        write(&p, "Library/MCB/BlenderSync/two/blender_heartbeat.json", &serde_json::json!({"kind": "orbiters.mcb.blenderHeartbeat", "blendFile": outside}).to_string());
        // A session whose .blend went away.
        write(&p, "Library/MCB/BlenderSync/three/session.json", &serde_json::json!({"blenderProjectAbsolutePath": p.join("gone.blend")}).to_string());

        let all = links(&p, &["Assets/MasculineCanine/Canine.unity", "Assets/Scenes/World.unity"]);
        assert_eq!(all.len(), 4, "{all:#?}");
        let find = |f: &Path| all.iter().find(|l| discovery::key(&l.blend) == discovery::key(f)).unwrap();
        assert_eq!(find(&mixer_blend).scene.as_deref(), Some("Assets/Scenes/World.unity"));
        assert_eq!(find(&mixer_blend).tool, Tool::Mixer);
        let m = find(&made);
        assert_eq!((m.scene.as_deref(), m.root.as_deref(), m.tool), (Some("Assets/MasculineCanine/Canine.unity"), Some("MasculineCanine"), Tool::Mcb));
        assert_eq!(find(&outside).root, None);
        assert_eq!(find(&p.join("Assets/MCB/blenderProjects/Alphazear_e5d5433dd397_2/Alphazear.blend")).root.as_deref(), Some("Alphazear"));

        let canine = avatar("avtr_1", "MasculineCanine", "Assets/MasculineCanine/Canine.unity", "MasculineCanine");
        let other = avatar("avtr_2", "Ultirex", "Assets/MasculineCanine/Canine.unity", "Ultirex");
        let alpha = avatar("avtr_3", "Alphazear", "Assets/Alpha/Alpha.unity", "Avatars/Alphazear");
        let world = avatar("wrld_1", "Moonlight", "Assets/Scenes/World.unity", "VRCWorld");
        let names = |a: &Avatar| linked(&all, a).iter().map(|l| l.blend.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let mut c = names(&canine);
        c.sort();
        assert_eq!(c, ["MasculineCanine.blend", "UltiZear.blend"]);
        // The hand-picked .blend names no root: every avatar of its scene has it; the made one is MasculineCanine's.
        assert_eq!(names(&other), ["UltiZear.blend"]);
        assert_eq!(names(&alpha), ["Alphazear.blend"]);
        assert_eq!(names(&world), ["World.blend"]);
    }

    /// A real project's links and the Blender that opens them, printed (reads only):
    /// `ORBITERS_TEST_PROJECT="H:\…\MCB Test" cargo test -p orbiters-projects real_links -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_links() {
        let project = PathBuf::from(std::env::var("ORBITERS_TEST_PROJECT").expect("ORBITERS_TEST_PROJECT"));
        let scan = crate::discovery::cached();
        let known = scan.projects.iter().find(|p| discovery::key(&p.path) == discovery::key(&project));
        let targets: Vec<Avatar> = known.map(|p| p.avatars.iter().chain(&p.worlds).cloned().collect()).unwrap_or_default();
        let scenes: Vec<&str> = targets.iter().map(|a| a.scene.as_str()).collect();
        let all = links(&project, &scenes);
        for l in &all {
            println!("{:?} {} scene={:?} root={:?}", l.tool, l.blend.display(), l.scene, l.root);
        }
        for a in &targets {
            println!("{} ({}): {:?}", a.name, a.scene, linked(&all, a).iter().map(|l| l.blend.display().to_string()).collect::<Vec<_>>());
        }
        println!("Blenders: {:?}", installs());
        println!("Opens with: {:?}", find());
    }

    #[test]
    fn a_project_without_links_has_none() {
        let temp = tempfile::tempdir().unwrap();
        assert!(links(temp.path(), &["Assets/Scene.unity"]).is_empty());
    }

    #[test]
    fn mcb_folders_name_the_avatar_root() {
        let root = |p: &str| mcb_root(Path::new(p));
        assert_eq!(root("H:/P/Assets/MCB/blenderProjects/Alphazear_e5d5433dd397/Alphazear.blend").as_deref(), Some("Alphazear"));
        assert_eq!(root("H:/P/Assets/MCB/blenderProjects/My_Avatar_e5d5433dd397_3/blend~/My_Avatar.blend").as_deref(), Some("My_Avatar"));
        assert_eq!(root("H:/P/Assets/MCB/blenderProjects/Odd/Thing.blend").as_deref(), Some("Thing"));
        assert_eq!(root("H:/Elsewhere/UltiZear.blend"), None);
        assert_eq!(global_id_scene(&format!("GlobalObjectId_V1-2-{CANINE}-1-0")).as_deref(), Some(CANINE));
        assert_eq!(global_id_scene(&format!("GlobalObjectId_V1-1-{CANINE}-1-0")), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn blender_is_found_in_its_bundles_with_their_versions() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = |at: &str, versions: &[&str]| {
            let app = temp.path().join(at);
            std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
            std::fs::write(app.join("Contents/MacOS/Blender"), "#!/bin/sh\n").unwrap();
            for v in versions {
                std::fs::create_dir_all(app.join("Contents/Resources").join(v)).unwrap();
            }
            std::fs::create_dir_all(app.join("Contents/Resources/lib")).unwrap();
            let mut info = plist::Dictionary::new();
            info.insert("CFBundleExecutable".into(), plist::Value::String("Blender".into()));
            plist::Value::Dictionary(info).to_file_xml(app.join("Contents/Info.plist")).unwrap();
            app
        };
        let current = bundle("Applications/Blender.app", &["4.2"]);
        let older = bundle("Applications/Blender 3.6.app", &["3.6"]);
        std::fs::create_dir_all(temp.path().join("Applications/Safari.app")).unwrap();
        let steam = bundle("Steam/steamapps/common/Blender/Blender.app", &["4.5", "5.2"]);
        assert_eq!(apps_in(&temp.path().join("Applications")), [older.clone(), current.clone()]);
        assert!(apps_in(&temp.path().join("not there")).is_empty());
        // The version is in the bundle's Resources, where Blender keeps its scripts.
        let program = |app: &Path| crate::mac::bundle_program(app);
        assert_eq!(program(&current), current.join("Contents/MacOS/Blender"));
        assert_eq!(version_of(&program(&current)), Some((4, 2)));
        assert_eq!(version_of(&program(&older)), Some((3, 6)));
        assert_eq!(version_of(&program(&steam)), Some((5, 2)), "Steam's leaves older ones behind");

        // Opened through the system, as a new Blender, with the file.
        let blend = temp.path().join("My Avatar.blend");
        let c = open_on_mac(&program(&current), &blend);
        let args: Vec<_> = c.get_args().map(|a| a.to_os_string()).collect();
        assert_eq!(c.get_program(), "/usr/bin/open");
        assert_eq!(args, vec!["-n".into(), "-a".into(), current.clone().into_os_string(), blend.clone().into_os_string()]);
        // A Blender outside any bundle (built by hand) as on other systems.
        let c = open_on_mac(Path::new("/usr/local/bin/blender"), &blend);
        assert_eq!(c.get_program(), "/usr/local/bin/blender");
        assert_eq!(c.get_args().collect::<Vec<_>>(), [blend.as_os_str()]);
    }

    #[test]
    fn blender_is_found_from_its_association_steam_and_its_version_folders() {
        assert_eq!(
            command_program(r#""C:\Program Files\Blender Foundation\Blender 2.93\blender.exe" "%1""#),
            Some(PathBuf::from(r"C:\Program Files\Blender Foundation\Blender 2.93\blender.exe"))
        );
        assert_eq!(command_program(r"C:\Blender\blender-launcher.exe %1"), Some(PathBuf::from(r"C:\Blender\blender-launcher.exe")));
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"F:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(vdf_paths(vdf), [PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"F:\SteamLibrary")]);

        let temp = tempfile::tempdir().unwrap();
        for d in ["Steam/Blender/2.79", "Steam/Blender/4.5", "Steam/Blender/5.2", "Steam/Blender/blender.shared", "Old/2.93"] {
            std::fs::create_dir_all(temp.path().join(d)).unwrap();
        }
        assert_eq!(version_of(&temp.path().join("Steam/Blender/blender.exe")), Some((5, 2)));
        let config = temp.path().join("config");
        for d in ["5.0/scripts/addons/mcb_blender", "5.2/extensions/user_default/mcb_blender", "4.4/scripts/addons/other"] {
            std::fs::create_dir_all(config.join(d)).unwrap();
        }
        let mut with = with_addons(&config);
        with.sort();
        assert_eq!(with, [(5, 0), (5, 2)]);

        let b = |exe: &str, v: Option<(u32, u32)>| Blender { exe: PathBuf::from(exe), version: v };
        // The association is an old Blender: the newest with Orbiters' add-on opens the links.
        let all = [b("assoc", Some((2, 93))), b("steam", Some((5, 2))), b("pf", Some((3, 3)))];
        assert_eq!(choose(&all, &with).unwrap().exe, PathBuf::from("steam"));
        assert_eq!(choose(&all, &[(3, 3)]).unwrap().exe, PathBuf::from("pf"));
        assert_eq!(choose(&all, &[]).unwrap().exe, PathBuf::from("steam"));
        assert_eq!(choose(&[b("assoc", None)], &[]).unwrap().exe, PathBuf::from("assoc"));
        assert!(choose(&[], &with).is_none());
        assert_eq!(b("x", Some((4, 2))).label(), "Blender 4.2");
    }
}
