//! Screenshots of the Unity 6 transition (`projects-u6-*`, `screenshot/projects_panel.rs`). Stand-in projects are
//! generated in the run's sandbox (app data of the harness) and read by the same code as real ones: their standing,
//! their packages' declarations and the checklist; what they show is then put under the gallery's stand-in names.
//! Trials, editors, Creator Companions and the openings' outcomes are set as each scene needs: nothing is copied,
//! launched, downloaded or asked of anyone.

use super::state::{Modal, State, STAND_IN_PROJECTS};
use super::unity6::{CheckedPlace, Fixture, Run, Sheet, Step};
use orbiters_i18n::tr;
use orbiters_projects::discovery::{self, Kind, Project};
use orbiters_projects::unity6::{self, budget, compat, editors, matrix, outcome, standing, trial, vcc};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const GB: u64 = 1_000_000_000;

/// A project's files: its editor, its SDK (content package, version, line), other packages, files under Assets.
fn generate(at: &Path, editor: &str, content: &str, sdk: &str, line: &str, packages: &[(&str, &str, serde_json::Value)], assets: &[(&str, &str)], base_installed: bool) {
    let _ = std::fs::remove_dir_all(at);
    let write = |rel: &str, body: &str| {
        let p = at.join(rel);
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, body);
    };
    write("ProjectSettings/ProjectVersion.txt", &format!("m_EditorVersion: {editor}\n"));
    write("ProjectSettings/EditorSettings.asset", "EditorSettings:\n  m_SerializationMode: 2\n");
    let mut deps = serde_json::Map::new();
    let mut locked = serde_json::Map::new();
    if !sdk.is_empty() {
        deps.insert(content.into(), json!({ "version": sdk }));
        for n in ["com.vrchat.base", content] {
            locked.insert(n.into(), json!({ "version": sdk, "dependencies": {} }));
        }
    }
    for (name, version, _) in packages {
        deps.insert((*name).into(), json!({ "version": version }));
        locked.insert((*name).into(), json!({ "version": version, "dependencies": {} }));
    }
    write("Packages/vpm-manifest.json", &json!({ "dependencies": deps, "locked": locked }).to_string());
    write("Packages/manifest.json", r#"{"dependencies":{"com.unity.timeline":"1.7.5","com.unity.ugui":"1.0.0"}}"#);
    if !sdk.is_empty() && base_installed {
        for n in ["com.vrchat.base", content] {
            write(&format!("Packages/{n}/package.json"), &json!({ "name": n, "version": sdk, "unity": line }).to_string());
        }
    }
    for (name, version, extra) in packages {
        let mut m = json!({ "name": name, "version": version, "displayName": extra["display"].as_str().unwrap_or(name) });
        for k in ["unity", "unityRelease", "vpmDependencies"] {
            if !extra[k].is_null() {
                m[k] = extra[k].clone();
            }
        }
        write(&format!("Packages/{name}/package.json"), &m.to_string());
    }
    for (rel, body) in assets {
        write(rel, body);
    }
}

fn candidate(program: &str, check: editors::Check, version: &str, modules: editors::Modules) -> editors::Candidate {
    let identity = matches!(check, editors::Check::Verified).then(|| editors::Identity { version: version.into(), changeset: None, arch: vec![editors::Arch::here()] });
    editors::Candidate { program: PathBuf::from(program), hint: editors::Hint::HubFolder, check, identity, modules }
}

fn place(name: &str, at: &str) -> trial::Place {
    trial::Place { path: PathBuf::from(format!("{at}/{name}")), id: None, name: name.into() }
}

fn job(id: &str, original: &str, name: &str, at: &str, phase: trial::Phase, error: Option<&str>) -> trial::Job {
    trial::Job {
        id: id.into(),
        created: discovery::now() - 3 * 86400,
        source: place(original, STAND_IN_PROJECTS),
        trial: place(name, at),
        phase,
        target: trial::Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() },
        error: error.map(Into::into),
        differences: vec![],
        attempts: vec![],
        migrated: None,
        beside: None,
        localized: vec![],
        linked: vec![],
        content: vec![],
        content_unsure: vec![],
        guid: String::new(),
        manifest: None,
        import_high: 0,
    }
}

fn diag(file: &str, line: u32, code: &str, message: &str) -> outcome::Diagnostic {
    outcome::Diagnostic { file: file.into(), line, code: code.into(), message: message.into() }
}

impl State {
    pub(super) fn stage_unity6(&mut self, name: &str) {
        if !name.starts_with("projects-u6") {
            return;
        }
        let m = matrix::Matrix::built_in();
        let root = orbiters_projects::data_dir().join("u6-stand-ins");
        let _ = std::fs::create_dir_all(&root);
        let poiyomi = ("com.poiyomi.toon", "10.0.20", json!({ "display": "Poiyomi Toon Shader", "unity": "2022.3" })); // i18n: not shown (sample data of the screenshot scenes)
        let liltoon = ("jp.lilxyzw.liltoon", "1.10.3", json!({ "display": "lilToon", "unity": "2019.4" }));
        let gesture = ("vrchat.blackstartx.gesture-manager", "3.9.8", json!({ "display": "Gesture Manager", "unity": "2019.4", "vpmDependencies": { "com.vrchat.avatars": ">=3.5.0 <3.11.0" } })); // i18n: not shown (sample data of the screenshot scenes)
        let av3 = ("com.vrlabs.av3manager", "2.1.2", json!({ "display": "Avatars 3.0 Manager", "unity": "2019.4", "vpmDependencies": { "com.vrchat.avatars": ">=3.2.0 <3.10.0" } })); // i18n: not shown (sample data of the screenshot scenes)
        let fury = ("com.vrcfury.vrcfury", "1.1475.0", json!({ "display": "VRCFury", "unity": "2019.4" }));
        let usharp = ("com.vrchat.udonsharp", "1.1.9", json!({ "display": "UdonSharp", "unity": "2022.3" }));
        let tmp_scene = "%YAML 1.1\n--- !u!114 &1\nMonoBehaviour:\n  m_fontAsset: {fileID: 1}\n  m_fontColor: {r: 1, g: 1, b: 1, a: 1}\n";
        let world_scene = "%YAML 1.1\n--- !u!195 &1\nNavMeshAgent:\n--- !u!208 &2\nNavMeshObstacle:\n--- !u!198 &3\nParticleSystem:\n  ringBufferMode: 2\n";
        let door = "public class Door : UdonSharpBehaviour { public Rigidbody body; void Interact() { body.velocity = Vector3.up; body.AddForce(Vector3.up, ForceMode.VelocityChange); } }"; // i18n: not shown (sample data of the screenshot scenes)
        let make = |dir: &str, editor: &str, content: &str, sdk: &str, line: &str, pkgs: &[(&str, &str, serde_json::Value)], assets: &[(&str, &str)], installed: bool| {
            let at = root.join(dir);
            generate(&at, editor, content, sdk, line, pkgs, assets, installed);
            at
        };
        let avatar = make(
            "avatar",
            "2022.3.22f1",
            "com.vrchat.avatars",
            "3.10.6",
            "2022.3",
            &[poiyomi.clone(), liltoon, gesture, av3, fury],
            &[("Assets/MasculineCanine/Avatar.unity", tmp_scene), ("Assets/MasculineCanine/Hoodie.prefab", "%YAML 1.1\n")],
            true,
        );
        let world = make("world", "2022.3.22f1", "com.vrchat.worlds", "3.10.6", "2022.3", &[usharp], &[("Assets/Scripts/Door.cs", door), ("Assets/Scenes/Moonlight.unity", world_scene)], true);
        let trial6 = make("trial6", "6000.0.67f1", "com.vrchat.avatars", "3.11.0-beta.1", "6000.0", &[poiyomi], &[("Assets/MasculineCanine/Avatar.unity", tmp_scene)], true);
        let differ = make("differ", "6000.0.67f1", "com.vrchat.avatars", "3.10.6", "2022.3", &[], &[], true);
        let missing = make("missing", "2022.3.22f1", "com.vrchat.avatars", "3.10.6", "2022.3", &[], &[], false);
        let incomplete = make("incomplete", "2022.3.22f1", "com.vrchat.worlds", "3.10.6", "2022.3", &[], &[], true);
        let _ = std::fs::remove_dir_all(incomplete.join("Packages/com.vrchat.worlds"));
        let old = make("old", "2019.4.31f1", "com.vrchat.avatars", "3.1.13", "2019.4", &[], &[], true);
        let read = |p: &Path| standing::read(p, &m);

        // The gallery: every project listed, a trial beside its original.
        let first = self.scan.projects[0].clone();
        let trial_path = PathBuf::from(format!("{STAND_IN_PROJECTS}/My avatars (Unity 6)"));
        let mut trial_project = Project { path: trial_path.clone(), name: "My avatars (Unity 6)".into(), unity: "6000.0.67f1".into(), registered: true, modified: first.modified + 60, ..first.clone() }; // i18n: not shown (sample data of the screenshot scenes)
        trial_project.kind = Kind::Avatars;
        self.scan.projects.insert(1, trial_project);
        for p in self.scan.projects.iter_mut().take(8) {
            p.registered = true;
        }
        let waiting = matches!(name, "projects-u6-trial-waiting" | "projects-u6-trial-vcc-copy");
        let by_name = |n: &str| -> Option<standing::Standing> {
            Some(match n {
                "My avatars" => read(&avatar),
                "My avatars (Unity 6)" => if waiting { read(&avatar) } else { read(&trial6) },
                "Moonlight world" => read(&world),
                "Club outfits" => read(&differ),
                "Avatar backup" => read(&missing),
                "Old avatar project" => read(&old),
                "Test world" => read(&incomplete),
                "Weekend avatar" => read(&avatar),
                _ => return None,
            })
        };
        let standings: Vec<(PathBuf, standing::Standing)> = self.scan.projects.iter().filter_map(|p| by_name(&p.name).map(|s| (p.path.clone(), s))).collect();
        let mut mine = job("trial-1", "My avatars", "My avatars (Unity 6)", STAND_IN_PROJECTS, trial::Phase::Ready, None); // i18n: not shown (sample data of the screenshot scenes)
        let other_disk = if cfg!(target_os = "macos") { "/Volumes/Archive/Unity projects" } else { "E:/Unity projects" }; // i18n: not shown (sample data of the screenshot scenes)
        let mut trials = vec![];
        let status = match name {
            "projects-u6-trial-waiting" => trial::Status::Waiting { vcc_copy: None },
            "projects-u6-trial-vcc-copy" => trial::Status::Waiting { vcc_copy: Some(PathBuf::from(format!("{STAND_IN_PROJECTS}/My avatars (Unity 6)-Migrated"))) },
            _ => trial::Status::OnUnity6,
        };
        let outcomes: Vec<outcome::Outcome> = match name {
            "projects-u6-outcomes-a" => vec![
                outcome::Outcome::DidNotStart("The system cannot find the file specified. (os error 2)".into()), // i18n: not shown (sample data of the screenshot scenes)
                outcome::Outcome::ExitedAtOnce(None),
                outcome::Outcome::SafeMode { errors: vec![diag("Assets/Scripts/Toggle.cs", 14, "CS0246", "The type or namespace name 'VRCExpressionsMenu' could not be found")], running: true }, // i18n: not shown (sample data of the screenshot scenes)
                outcome::Outcome::Importing,
            ],
            "projects-u6-outcomes-b" => vec![
                outcome::Outcome::Undetermined { why: outcome::Unknown::EndedEarly, running: false },
                outcome::Outcome::CompilerErrors {
                    errors: vec![
                        diag("Packages/com.vrlabs.av3manager/Editor/Manager.cs", 212, "CS0246", "The type or namespace name 'AnimatorControllerLayer' could not be found"), // i18n: not shown (sample data of the screenshot scenes)
                        diag("Assets/Scripts/Spin.cs", 8, "CS1061", "'Rigidbody' does not contain a definition for 'angularDrag'"), // i18n: not shown (sample data of the screenshot scenes)
                    ],
                    running: false,
                },
                outcome::Outcome::Imported { running: false, cut: false },
                outcome::Outcome::Imported { running: true, cut: false },
            ],
            _ => vec![],
        };
        for (i, o) in outcomes.iter().enumerate() {
            let n = i as u32 + 1;
            let start = matches!(o, outcome::Outcome::DidNotStart(_)).then(|| o.clone());
            mine.attempts.push(trial::Attempt { n, started: discovery::now() - (4 - i as u64) * 3600, log: PathBuf::from(format!("attempt-{n}.log")), editor: PathBuf::from("Unity.exe"), unity: "6000.0.67f1".into(), start, process: None, reserved: false });
            self.u6.outcomes.insert((mine.id.clone(), n), o.clone());
        }
        trials.push((mine, status));
        if matches!(name, "projects-u6-gallery" | "projects-u6-jobs" | "projects-u6-damaged") {
            trials.push((job("trial-2", "Club outfits", "Club outfits (Unity 6)", STAND_IN_PROJECTS, trial::Phase::Copying, None), trial::Status::Interrupted)); // i18n: not shown (sample data of the screenshot scenes)
            trials.push((
                job("trial-3", "Test world", "Test world (Unity 6)", STAND_IN_PROJECTS, trial::Phase::Verified, Some("The trial is at H:/Unity projects/Test world (Unity 6), but it could not be finished: a file of it is in use by another program.")), // i18n: not shown (sample data of the screenshot scenes)
                trial::Status::Unfinished,
            ));
            trials.push((job("trial-4", "Avatar backup", "Avatar backup (Unity 6)", other_disk, trial::Phase::Ready, None), trial::Status::Missing)); // i18n: not shown (sample data of the screenshot scenes)
            let mut failed = job("trial-5", "Weekend avatar", "Weekend avatar (Unity 6)", STAND_IN_PROJECTS, trial::Phase::Failed, Some("The copy at H:/Unity projects/Weekend avatar (Unity 6) is not what the original holds (2 differences):")); // i18n: not shown (sample data of the screenshot scenes)
            failed.differences = vec!["Assets/Weekend/Body.png: not the same contents".into(), "Assets/Weekend/Notes.txt: missing".into()]; // i18n: not shown (sample data of the screenshot scenes)
            trials.push((failed, trial::Status::Failed { kept: true }));
        }
        let pairs = vec![unity6::Pair { original: first.path.clone(), trial: trial_path.clone(), job: "trial-1".into() }];
        let standings: Vec<(PathBuf, standing::Standing)> = standings;
        let index = standings.iter().enumerate().map(|(i, (p, _))| (discovery::key(p), i)).collect();
        let present = trials.iter().filter(|(_, s)| !matches!(s, trial::Status::Missing)).map(|(j, _)| discovery::key(&j.trial.path)).collect();
        let health = if name == "projects-u6-damaged" {
            trial::Health::Damaged { lines: 1, why: "its checksum does not match what it says".into(), newer: false } // i18n: not shown (sample data of the screenshot scenes)
        } else {
            trial::Health::Fine
        };
        let copies = match name {
            "projects-u6-jobs" => [
                ("c1", "Club outfits", orbiters_projects::copies::State::Whole, "The copy is at H:/Unity projects/Club outfits copy, but registration failed: settings.json of Creator Companion or ALCOM is in use: close them, then try again. Nothing was changed."), // i18n: not shown (sample data of the screenshot scenes)
                ("c2", "Test world", orbiters_projects::copies::State::Copying, ""), // i18n: not shown (sample data of the screenshot scenes)
            ]
            .map(|(id, name, state, error)| {
                let copy = orbiters_projects::copies::Copy { id: id.into(), source: PathBuf::from(format!("{STAND_IN_PROJECTS}/{name}")), destination: PathBuf::from(format!("{STAND_IN_PROJECTS}/{name} copy")), folder: None, started: discovery::now(), state, error: (!error.is_empty()).then(|| error.to_string()) };
                let status = if state == orbiters_projects::copies::State::Whole { orbiters_projects::copies::Status::Unlisted } else { orbiters_projects::copies::Status::Incomplete };
                (copy, status)
            })
            .into(),
            _ => vec![],
        };
        self.u6.look = Some(unity6::Look { matrix: m.clone(), projects: vec![], standings, index, trials, pairs, health, copies, present });
        self.u6.fixture = Some(Fixture { mac: name == "projects-u6-mac" });

        // Editors and Creator Companions.
        let hub = if cfg!(target_os = "macos") { "/Applications/Unity/Hub/Editor" } else { "C:/Program Files/Unity/Hub/Editor" };
        let exe = |v: &str| if cfg!(target_os = "macos") { format!("{hub}/{v}/Unity.app/Contents/MacOS/Unity") } else { format!("{hub}/{v}/Editor/Unity.exe") };
        self.u6.editors.insert("2022.3.22f1".into(), editors::Found::Exact(candidate(&exe("2022.3.22f1"), editors::Check::Verified, "2022.3.22f1", editors::Modules { android: true, android_tools: true, ios: false })));
        let six = match name {
            "projects-u6-editor-unverified" => editors::Found::Unverified(candidate("D:/Unity/6000.0.67f1/Editor/Unity.exe", editors::Check::Unverified, "6000.0.67f1", Default::default())),
            "projects-u6-editor-found" | "projects-u6-trial-ready" | "projects-u6-outcomes-a" | "projects-u6-outcomes-b" => {
                editors::Found::Exact(candidate(&exe("6000.0.67f1"), editors::Check::Verified, "6000.0.67f1", editors::Modules { android: true, android_tools: true, ios: false }))
            }
            _ => editors::Found::Missing { wrong: vec![] },
        };
        self.u6.editors.insert("6000.0.67f1".into(), six);
        self.u6.vccs = Some(vec![vcc::Vcc { folder: PathBuf::from("D:/Tools/VCC 2.5.0-beta.3"), version: Some("2.5.0-beta.3".into()), portable: true }]); // i18n: not shown (sample data of the screenshot scenes)

        // What the panel's card reads of the project shown.
        let (shown, generated) = match name {
            "projects-u6-panel-world" => (self.scan.projects.iter().find(|p| p.kind == Kind::Worlds).map(|p| p.path.clone()), world.clone()),
            n if n.starts_with("projects-u6-trial") || n.starts_with("projects-u6-outcomes") => (Some(trial_path.clone()), if waiting { avatar.clone() } else { trial6.clone() }),
            _ => (Some(first.path.clone()), avatar.clone()),
        };
        if let Some(shown) = shown.clone() {
            let content = read(&generated).sdk.content.unwrap_or(standing::Content::Base);
            let rows = compat::packages(&generated, &m, "3.11.0-beta.1");
            let list = compat::checklist(&generated, content, &Default::default());
            self.u6.compat = Some((shown, rows, list));
        }
        self.u6.show_packages = name == "projects-u6-packages";
        match name {
            "projects-u6-gallery" | "projects-u6-jobs" | "projects-u6-damaged" => {
                self.selected = Some(first.path.clone());
                if name != "projects-u6-gallery" {
                    self.scroll_gallery = Some(620.0);
                }
            }
            n if n.starts_with("projects-u6-try") => {
                self.selected = Some(first.path.clone());
                self.u6.scroll_to = true;
                self.modal = Some(Modal::Unity6(first.path.clone()));
                let location = STAND_IN_PROJECTS.to_string();
                let trial_name = "My avatars (Unity 6)".to_string(); // i18n: not shown (sample data of the screenshot scenes)
                let step = match n {
                    "projects-u6-try-ids" => Step::Ids,
                    "projects-u6-try-baseline" => Step::Baseline,
                    "projects-u6-try-copying" => Step::Copying,
                    "projects-u6-try-done" | "projects-u6-try-failed" => Step::Done,
                    _ => Step::Place,
                };
                let short = n == "projects-u6-try-budget-short";
                let trial_at = PathBuf::from(format!("{location}/{trial_name}"));
                let plan = budget::Budget {
                    volumes: vec![
                        budget::Volume {
                            at: trial_at.clone(),
                            free: Some(if short { 17_270_000_000 } else { 412 * GB }),
                            needs: vec![
                                budget::Demand { need: budget::Need::Copy, at: trial_at.clone(), bytes: 7_350_000_000, low: None },
                                budget::Demand { need: budget::Need::Import, at: trial_at.clone(), bytes: 22_080_000_000, low: Some(8_830_000_000) },
                                budget::Demand { need: budget::Need::Margin, at: trial_at.clone(), bytes: budget::MARGIN, low: None },
                            ],
                        },
                        budget::Volume {
                            at: PathBuf::from(hub),
                            free: Some(35 * GB),
                            needs: vec![budget::Demand { need: budget::Need::Editor, at: PathBuf::from(hub), bytes: 4_078_656_432 + 7_771_104_225, low: None }],
                        },
                    ],
                    import_guessed: false,
                };
                let blockers = if n == "projects-u6-try-blocked" { vec![trial::Blocker::Open, trial::Blocker::OnlyInLibrary(vec!["com.example.private-tools".into()])] } else { vec![] };
                let outside = if n == "projects-u6-try-budget" { vec![tr!("{link} is a link to {folder}: the trial gets its files, not the link.", link = "Assets/Shared", folder = "D:/Shared assets")] } else { vec![] };
                let mut sheet = Sheet { step, name: trial_name.clone(), location: location.clone(), checked: Some(((location.clone(), trial_name.clone()), Ok(CheckedPlace { plan, blockers, outside }))), ..Default::default() };
                sheet.done = match n {
                    "projects-u6-try-done" => Some(Ok(job("trial-1", "My avatars", "My avatars (Unity 6)", STAND_IN_PROJECTS, trial::Phase::Ready, None))), // i18n: not shown (sample data of the screenshot scenes)
                    "projects-u6-try-failed" => Some(Err("Could not copy Assets/MasculineCanine/Textures/Body_Albedo.png: There is not enough space on the disk. (os error 112)".into())), // i18n: not shown (sample data of the screenshot scenes)
                    _ => None,
                };
                self.u6.sheet = sheet;
                if n == "projects-u6-try-copying" {
                    let progress: orbiters_projects::templates::SharedProgress = Arc::new(Mutex::new(orbiters_projects::templates::Progress {
                        copied: 1840,
                        total: 4310,
                        file: "Assets/MasculineCanine/Textures/Body_Albedo.png".into(),
                        phase: String::from(orbiters_i18n::mark("Copying project")),
                    }));
                    self.u6.run = Some(Run { shown: true, recorded: Arc::new(Mutex::new(Some("trial-1".into()))), job: Default::default(), cancel: Default::default(), progress });
                }
            }
            _ => {
                self.selected = shown;
                self.u6.scroll_to = true;
            }
        }
    }
}
