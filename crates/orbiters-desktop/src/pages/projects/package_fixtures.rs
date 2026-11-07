//! Screenshots of the packages' updates, the missing packages' card, the conflict dialog and a project open in Unity
//! (`screenshot/projects_panel.rs`): stand-ins; no check, lookup, editor or request runs.
use orbiters_i18n::tr;
use super::state::{Modal, State};
use orbiters_projects::updates::{Check, Held, Plan, Step, Update, Updates};
use orbiters_projects::{coherent, discovery, editor::Session, missing, vpm};

// The names and versions of the screenshot scene's conflict, the packages and SDK it is about.
const SDK: &str = "VRChat SDK - Avatars"; // i18n: not shown (a sample package name)
const NEW_SDK: &str = "3.9.0"; // i18n: not shown (a sample version)
const HOLDERS: &str = "Gesture Manager, Av3Manager"; // i18n: not shown (sample package names)

/// What the plan of a conflict says of a package that has to move with the SDK (`coherent::solve`'s own words).
fn accepts(current: &str, range: &str, v: &str) -> String {
    tr!("{current} accepts {package} {range} only; {v} works with {wanted}", current = current, package = SDK, range = range, v = v, wanted = NEW_SDK)
}

fn step(n: &str, d: &str, from: Option<&str>, to: Option<&str>, why: Option<&str>) -> Step {
    Step { name: n.into(), display_name: d.into(), from: from.map(Into::into), to: to.map(Into::into), why: why.map(Into::into) }
}

impl State {
    pub(super) fn stage_packages(&mut self, name: &str) {
        let Some(first) = self.scan.projects.first().map(|p| p.path.clone()) else { return };
        if name.starts_with("projects-updates") || name.starts_with("projects-missing") || name.starts_with("projects-conflict") {
            // The package set of Enzo's screenshot: Orbiters' tools a patch behind, the SDK held by the tools pinning it.
            for (n, installed, versions) in [
                ("com.vrchat.avatars", "3.7.5", vec!["3.9.0", "3.8.2", "3.7.5"]),
                ("com.vrchat.base", "3.7.5", vec!["3.9.0", "3.8.2", "3.7.5"]),
                ("orbiters.toolkit", "0.4.10", vec!["0.4.11", "0.4.10", "0.4.9"]),
                ("orbiters.myavatar", "0.9.18", vec!["0.9.19", "0.9.18", "0.9.17"]),
                ("orbiters.mixer", "0.2.3", vec!["0.2.4", "0.2.3"]),
            ] {
                if let Some(p) = self.view.packages.iter_mut().find(|p| p.name == n) {
                    p.installed = Some(installed.into());
                    p.versions = versions.into_iter().map(Into::into).collect();
                }
            }
            let up = |n: &str, d: &str, from: &str, to: &str, also: Vec<Step>| {
                let mut steps = vec![step(n, d, Some(from), Some(to), None)];
                steps.extend(also);
                Update { name: n.into(), display_name: d.into(), from: from.into(), to: to.into(), sdk: false, steps }
            };
            let updates = Updates {
                updates: vec![
                    up("orbiters.mixer", "Mixer", "0.2.3", "0.2.4", vec![]),
                    up("orbiters.myavatar", "My Avatar", "0.9.18", "0.9.19", vec![step("orbiters.toolkit", "Orbiters Toolkit", Some("0.4.10"), Some("0.4.11"), None)]),
                    up("orbiters.toolkit", "Orbiters Toolkit", "0.4.10", "0.4.11", vec![]),
                ],
                held: vec![Held {
                    name: "com.vrchat.avatars".into(),
                    display_name: "VRChat SDK - Avatars".into(), // i18n: not shown (sample package and repository names of a screenshot scene)
                    version: "3.9.0".into(),
                    why: orbiters_i18n::tr!("conflicts with {packages}", packages = HOLDERS),
                }],
                all: Plan {
                    targets: vec![("orbiters.mixer".into(), "0.2.4".into()), ("orbiters.myavatar".into(), "0.9.19".into()), ("orbiters.toolkit".into(), "0.4.11".into())],
                    steps: vec![
                        step("orbiters.mixer", "Mixer", Some("0.2.3"), Some("0.2.4"), None),
                        step("orbiters.myavatar", "My Avatar", Some("0.9.18"), Some("0.9.19"), None),
                        step("orbiters.toolkit", "Orbiters Toolkit", Some("0.4.10"), Some("0.4.11"), None),
                    ],
                    left_out: vec![],
                },
            };
            self.updates = Some((first.clone(), Check { stable: updates.clone(), prerelease: updates, skipped: None }));
            self.selected = Some(first.clone());
            self.scroll_to_packages = name.starts_with("projects-updates");
            self.plan_open = name == "projects-updates-plan";
        }
        if name.starts_with("projects-missing") && name != "projects-missing-done" {
            let message = "Following dependencies are not found: dev.hai-vr.fuck-osc-config@=1.1.5, d4rkpl4y3r.d4rkavataroptimizer@=3.8.0, \
                           de.thryrallo.vrc.avatar-performance-tools@=1.3.7";
            self.error = Some(message.into());
            let mut card = super::missing_card::Card::from_error(message, Some((first.clone(), vpm::Change::Resolve))).expect("a missing packages error");
            if name == "projects-missing-found" || name == "projects-missing-adding" {
                let m = card.packages.clone();
                let found = |p: &missing::Missing, v: &str, r: &str, u: &str| missing::Found {
                    package: p.clone(),
                    version: v.into(),
                    repository: r.into(),
                    url: u.into(),
                    kind: missing::Kind::Known,
                    turn_on: None,
                };
                card.lookup = Some(Ok(missing::Lookup {
                    found: vec![
                        found(&m[0], "1.1.5", "Hai~ packages", "https://hai-vr.github.io/vpm-listing/index.json"), // i18n: not shown (sample package and repository names of a screenshot scene)
                        found(&m[1], "3.8.0", "d4rkpl4y3r packages", "https://d4rkc0d3r.github.io/vpm-repos/main.json"), // i18n: not shown (sample package and repository names of a screenshot scene)
                    ],
                    not_listed: vec![],
                    not_provided: vec![m[2].clone()],
                }));
            }
            card.fixture_looking = name == "projects-missing-looking";
            if name == "projects-missing-adding" {
                self.fixture_action = Some(tr!("Adding {n} repositories…", n = 2));
                self.adding_repositories = true;
            }
            self.missing = Some(card);
        }
        if name == "projects-missing-done" {
            for (n, d, v) in [
                ("dev.hai-vr.fuck-osc-config", "Fuck OSC Config", "1.1.5"), // i18n: not shown (sample package and repository names of a screenshot scene)
                ("d4rkpl4y3r.d4rkavataroptimizer", "d4rkAvatarOptimizer", "3.8.0"), // i18n: not shown (sample package and repository names of a screenshot scene)
                ("de.thryrallo.vrc.avatar-performance-tools", "Avatar Performance Tools", "1.3.7"), // i18n: not shown (sample package and repository names of a screenshot scene)
            ] {
                self.view.packages.push(vpm::Package { name: n.into(), display_name: d.into(), installed: Some(v.into()), versions: vec![v.into()], ..Default::default() });
            }
            self.view.packages.sort_by(|a, b| a.name.cmp(&b.name));
            self.scroll_to_packages = true;
        }
        if name.starts_with("projects-conflict") {
            let need = |n: &str, d: &str, v: &str, r: &str| coherent::Need { name: n.into(), display_name: d.into(), version: v.into(), range: r.into() };
            let mut plan = coherent::Plan {
                requested: vec![("com.vrchat.avatars".into(), "3.9.0".into())],
                conflicts: vec![coherent::Conflict {
                    name: "com.vrchat.avatars".into(),
                    display_name: "VRChat SDK - Avatars".into(), // i18n: not shown (sample package and repository names of a screenshot scene)
                    version: "3.9.0".into(),
                    needed_by: vec![
                        need("dev.hai-vr.fuck-osc-config", "Fuck OSC Config", "1.1.5", ">=3.1.0 <3.8.0-a"), // i18n: not shown (sample package and repository names of a screenshot scene)
                        need("dev.vrlabs.av3manager", "Av3Manager", "2.0.34", "3.2 - 3.7"), // i18n: not shown (sample package and repository names of a screenshot scene)
                        need("vrchat.blackstartx.gesture-manager", "Gesture Manager", "3.9.3", ">=3.5.2 <3.8.X"), // i18n: not shown (sample package and repository names of a screenshot scene)
                    ],
                }],
                ..Default::default()
            };
            if name == "projects-conflict-blocked" {
                plan.blockers = vec![coherent::Blocker {
                    name: "dev.hai-vr.fuck-osc-config".into(),
                    display_name: "Fuck OSC Config".into(), // i18n: not shown (sample package and repository names of a screenshot scene)
                    version: "1.1.5".into(),
                    dependency: "VRChat SDK - Avatars".into(), // i18n: not shown (sample package and repository names of a screenshot scene)
                    range: ">=3.1.0 <3.8.0-a".into(),
                    wanted: "3.9.0".into(),
                    removable: true,
                }];
            } else {
                plan.steps = vec![
                    step("com.vrchat.avatars", "VRChat SDK - Avatars", Some("3.7.5"), Some("3.9.0"), Some(tr("asked for"))), // i18n: not shown (sample package names of a screenshot scene)
                    step("com.vrchat.base", "VRChat SDK - Base", Some("3.7.5"), Some("3.9.0"), Some(&tr!("needed by {package} {version}", package = SDK, version = NEW_SDK))), // i18n: not shown (sample package names of a screenshot scene)
                    step("dev.hai-vr.fuck-osc-config", "Fuck OSC Config", Some("1.1.5"), Some("1.2.0"), Some(&accepts("1.1.5", ">=3.1.0 <3.8.0-a", "1.2.0"))), // i18n: not shown (sample package names of a screenshot scene)
                    step("dev.vrlabs.av3manager", "Av3Manager", Some("2.0.34"), Some("2.1.0"), Some(&accepts("2.0.34", "3.2 - 3.7", "2.1.0"))), // i18n: not shown (sample package names of a screenshot scene)
                    step("vrchat.blackstartx.gesture-manager", "Gesture Manager", Some("3.9.3"), Some("3.9.4"), Some(&accepts("3.9.3", ">=3.5.2 <3.8.X", "3.9.4"))), // i18n: not shown (sample package names of a screenshot scene)
                ];
                plan.install = plan.steps.iter().filter(|s| s.name != "com.vrchat.base").map(|s| (s.name.clone(), s.to.clone().unwrap_or_default())).collect();
            }
            self.conflict = Some(super::conflict::View { path: first.clone(), plan, solving: Default::default() });
            self.modal = Some(Modal::Conflict);
        }
        if name.starts_with("projects-unity") {
            self.selected = Some(first.clone());
            let session = match name {
                "projects-unity-closing" => Session::Closing { pid: 4242, asking: true, answered: false },
                "projects-unity-killing" => Session::Killing { pid: 4242 },
                _ => Session::Open { pid: 4242 },
            };
            self.sessions.insert(discovery::key(&first), (session, None));
            if name == "projects-unity-kill" {
                self.kill_confirm = Some(first.clone());
            }
            self.scroll_to_packages = name == "projects-unity-bar";
        }
    }
}
