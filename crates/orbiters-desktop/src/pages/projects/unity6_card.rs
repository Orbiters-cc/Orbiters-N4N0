//! The panel's Unity 6 card. For a VRChat project: where it stands (its editor, its SDK, VRChat's release list), the
//! editors it needs and how to get one, what can be said of its packages and content before a try, and Try on Unity 6.
//! For a trial: what it shares with its original, what to do in Creator Companion while it waits, then Open in Unity 6
//! and what each opening showed. Calm and factual: VRChat's Unity 6 SDK is a beta with no announced end for Unity 2022.

use orbiters_core::app_log::OrLog as _;
use orbiters_core::locks::*;
use orbiters_i18n::tr;
use super::art;
use super::state::State;
use crate::pages::Cx;
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Ui};
use orbiters_projects::discovery::Project;
use orbiters_projects::unity6::{compat, editors, matrix, outcome, standing, trial};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{format, icons};
use std::path::{Path, PathBuf};

/// VRChat's migration guide (beta documentation).
pub(super) const GUIDE: &str = "https://vrc-beta-docs.netlify.app/sdk/upgrade/unity-6000/";

/// A wrapped paragraph.
pub(super) fn para(ui: &mut Ui, text: &str, size: f32, color: Color32) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(text, size).color(color).line_height(size * 1.5).job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// A line with a glyph, a title and a wrapped detail under it.
pub(super) fn item(ui: &mut Ui, glyph: &str, tint: Color32, title: &str, detail: Option<&str>) {
    let w = ui.available_width();
    let tg = ui.painter().layout_job(Txt::new(title, 13.0).weight(weight::MEDIUM).line_height(19.0).job(w - 26.0));
    let dg = detail.map(|d| ui.painter().layout_job(Txt::new(d, 12.0).color(theme::fg_3()).line_height(18.0).job(w - 26.0)));
    let h = tg.size().y + dg.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    widgets::icon(p, pos2(r.left() + 8.0, r.top() + 10.0), glyph, 14.0, tint);
    let th = tg.size().y;
    p.galley(pos2(r.left() + 26.0, r.top()), tg, theme::fg());
    if let Some(g) = dg {
        p.galley(pos2(r.left() + 26.0, r.top() + th + 2.0), g, theme::fg_3());
    }
}

/// A step of a list: its number in a disc, its text wrapped beside it.
fn numbered(ui: &mut Ui, n: usize, text: &str) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::fg_2()).line_height(19.0).job(w - 30.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y.max(20.0)), Sense::hover());
    let p = ui.painter();
    let c = pos2(r.left() + 10.0, r.top() + 10.0);
    p.circle_filled(c, 10.0, theme::white(0.07));
    widgets::text(p, c, Align2::CENTER_CENTER, &Txt::new(n.to_string(), 11.0).weight(weight::SEMIBOLD).color(theme::fg_2()));
    p.galley(pos2(r.left() + 30.0, r.top()), g, theme::fg_2());
}

/// A tinted box holding a heading and a paragraph (what cannot be undone).
pub(super) fn notice(ui: &mut Ui, tone: Tone, title: &str, text: &str) {
    let w = ui.available_width();
    let tg = ui.painter().layout_job(Txt::new(title, 13.0).weight(weight::SEMIBOLD).line_height(19.0).job(w - 52.0));
    let dg = ui.painter().layout_job(Txt::new(text, 12.5).color(theme::fg_2()).line_height(19.0).job(w - 52.0));
    let h = 14.0 + tg.size().y + 4.0 + dg.size().y + 14.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, r, 14.0, theme::alpha(tone.color(), 0.08));
    widgets::icon(p, pos2(r.left() + 22.0, r.top() + 24.0), icons::WARNING, 15.0, tone.color());
    let th = tg.size().y;
    p.galley(pos2(r.left() + 40.0, r.top() + 14.0), tg, theme::fg());
    p.galley(pos2(r.left() + 40.0, r.top() + 18.0 + th), dg, theme::fg_2());
}

/// A row of buttons from the left, each `(label, glyph, enabled, salt)`; the index pressed.
pub(super) fn buttons(ui: &mut Ui, list: &[(&str, &str, bool, &str)], primary: bool) -> Option<usize> {
    let w = ui.available_width();
    let widths: Vec<f32> = list.iter().map(|(l, g, _, _)| Button::new(l).icon(g).size(Size::Sm).desired_width(ui)).collect();
    let (spots, h) = widgets::flow(&widths, w, 32.0, 8.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut pressed = None;
    for (i, ((label, glyph, enabled, salt), at)) in list.iter().zip(spots).enumerate() {
        let variant = if primary && i == 0 { Variant::Primary } else { Variant::Ghost };
        let b = Button::new(label).icon(glyph).size(Size::Sm).variant(variant).enabled(*enabled).id_salt(("u6-button", *salt));
        let rect = Rect::from_min_size(r.min + at, vec2(widths[i], 32.0));
        // A press of the person's own: several of these are hard to take back.
        if b.confirm_at(ui, rect) {
            pressed = Some(i);
        }
    }
    pressed
}

pub(super) fn open_url(ctx: &egui::Context, cx: &mut Cx, url: &str) {
    if let Err(e) = crate::controller::open_url(url) {
        cx.toasts.push(ctx, orbiters_ui::toast::Kind::Error, tr!("The page could not open: {error}", error = e));
    }
}

fn gb(bytes: u64) -> String {
    format::bytes(bytes as f64)
}

/// The day of a time, as the person's language writes it.
pub(super) fn day(epoch: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(epoch as i64).map(|t| format!("{:04}-{:02}-{:02}", t.year(), t.month() as u8, t.day())).unwrap_or_default()
}

/// The short label of a standing, for a gallery card's pill; `None`: the card shows the editor as before.
pub(super) fn pill(s: &standing::Standing) -> Option<String> {
    let sdk = s.sdk.version().unwrap_or_default();
    Some(match s.state {
        standing::State::On2022 => tr!("2022 · SDK {sdk}", sdk = sdk),
        standing::State::OnUnity6 => tr!("Unity 6 · SDK {sdk}", sdk = sdk),
        standing::State::Disagree => tr("Editor and SDK differ").to_string(),
        standing::State::SdkMissing => tr("SDK not installed").to_string(),
        standing::State::SdkIncomplete => tr("SDK incomplete").to_string(),
        standing::State::EditorUnknown => tr("Unity version unknown").to_string(),
        standing::State::LineUnknown => tr!("Unity {editor} · SDK {sdk}", editor = s.editor.text, sdk = sdk),
        standing::State::NotVrchat | standing::State::LegacySdk | standing::State::OtherLine => return None,
    })
}

/// What a standing means, in a sentence or two.
fn sentence(s: &standing::Standing, m: &matrix::Matrix) -> String {
    let sdk = s.sdk.version().unwrap_or_default().to_string();
    let needs = s.sdk.needs.clone().unwrap_or_default();
    match s.state {
        standing::State::On2022 => {
            let supported = m.stable_2022().map(|r| r.unity.clone()).unwrap_or_default();
            if !supported.is_empty() && s.editor.text != supported {
                tr!("On the Unity 2022 line with SDK {sdk}. VRChat's supported 2022 editor is {supported}; this project uses {editor}.", sdk = sdk, supported = supported, editor = s.editor.text)
            } else {
                tr!("On Unity 2022 with SDK {sdk}, the editor VRChat supports for it.", sdk = sdk)
            }
        }
        standing::State::OnUnity6 => {
            if s.beta == Some(true) {
                tr!("On Unity 6 with SDK {sdk}, a beta. Orbiters has no record of how it got here.", sdk = sdk)
            } else {
                tr!("On Unity 6 with SDK {sdk}. Orbiters has no record of how it got here.", sdk = sdk)
            }
        }
        standing::State::Disagree => tr!(
            "Its editor is Unity {editor}, and its installed SDK {sdk} needs the {line} line. Unity opens it with {editor}; the SDK may not work there.",
            editor = s.editor.text,
            sdk = sdk,
            line = needs
        ),
        standing::State::SdkMissing => tr!("Its lock names SDK {sdk}, but the SDK is not in its Packages folder. Restore its packages (Packages card) before anything else.", sdk = sdk),
        standing::State::SdkIncomplete => tr!(
            "Part of its SDK is missing or not at the version its lock names: {packages}. Restore its packages first.",
            packages = s.sdk.missing.iter().chain(&s.sdk.mismatched).cloned().collect::<Vec<_>>().join(", ")
        ),
        standing::State::EditorUnknown => tr("Its ProjectSettings/ProjectVersion.txt names no Unity version.").to_string(),
        standing::State::LineUnknown => tr!("SDK {sdk} declares no Unity line, and Orbiters' release list does not know it.", sdk = sdk),
        _ => String::new(),
    }
}

/// The release list's line: built in or served, when it was checked, and newer rows it names but the app cannot act on.
fn release_list(ui: &mut Ui, m: &matrix::Matrix) {
    let now = orbiters_projects::discovery::now();
    let text = match m.age(now) {
        None => tr!("VRChat's release list in Orbiters: built into this version, checked {day}.", day = m.data.verified),
        Some(age) => tr!("VRChat's release list in Orbiters: from orbiters.cc, verified {when}.", when = crate::pages::ago(ui.ctx(), now.saturating_sub(age), now)),
    };
    let mut text = text;
    if m.stale(now) {
        text.push(' ');
        text.push_str(tr("It may be out of date."));
    }
    for r in m.display_only_rows() {
        text.push(' ');
        text.push_str(&tr!("orbiters.cc also lists SDK {sdk} on Unity {unity}: this version of Orbiters does not act on it.", sdk = r.sdk, unity = r.unity));
    }
    para(ui, &text, 11.5, theme::fg_4());
}

/// One editor: found (verified or not), another version found, or missing with Unity Hub's link, its sizes and modules.
fn editor_row(ui: &mut Ui, cx: &mut Cx, state: &mut State, version: &str, salt: &str) {
    let ctx = ui.ctx().clone();
    let m = state.u6.matrix();
    let row = m.row_of_editor(version).cloned();
    let found = state.u6.editors.get(version).cloned();
    match &found {
        None => item(ui, icons::CIRCLE_NOTCH, theme::fg_4(), &tr!("Unity {version}", version = version), Some(tr("Looking for it…"))),
        Some(editors::Found::Exact(c)) => {
            let detail = tr!("Installed and verified: {path}", path = c.program.display());
            item(ui, icons::CHECK_CIRCLE, theme::OK, &tr!("Unity {version}", version = version), Some(&detail));
            modules_line(ui, c.modules);
        }
        Some(editors::Found::Unverified(c)) => {
            let detail = tr!("Found at {path}, but its version could not be read from the program. Locate it to use it for a trial.", path = c.program.display());
            item(ui, icons::WARNING_CIRCLE, theme::WARN, &tr!("Unity {version}, not verified", version = version), Some(&detail));
        }
        Some(editors::Found::Missing { wrong }) => {
            let mut detail = match row.as_ref().and_then(|r| r.size_here()) {
                Some(s) => tr!(
                    "Not installed. Unity Hub installs it: about {download} to download and {installed} on disk, before modules.",
                    download = gb(s.download),
                    installed = gb(s.installed)
                ),
                None => tr("Not installed. Unity Hub installs it.").to_string(),
            };
            if let Some(s) = row.as_ref().and_then(|r| r.size_here()) {
                if let Some([d, i]) = s.android {
                    detail.push(' ');
                    detail.push_str(&tr!("Android Build Support with its SDK, NDK and JDK adds about {download} and {installed}: only for Android and Quest uploads.", download = gb(d), installed = gb(i)));
                }
                if let Some([d, i]) = s.ios {
                    detail.push(' ');
                    detail.push_str(&tr!("iOS Build Support adds about {download} and {installed}: only for iOS uploads.", download = gb(d), installed = gb(i)));
                }
            }
            for w in wrong {
                if let editors::Check::Wrong(why) = &w.check {
                    detail.push(' ');
                    detail.push_str(&tr!("{path} was found, and not used: {why}.", path = w.program.display(), why = why));
                }
            }
            item(ui, icons::DOWNLOAD_SIMPLE, theme::fg_3(), &tr!("Unity {version}", version = version), Some(&detail));
        }
    }
    let usable = found.as_ref().is_some_and(|f| matches!(f, editors::Found::Exact(_)));
    let mut list: Vec<(&str, &str, bool, &str)> = vec![];
    let link = row.as_ref().filter(|r| m.actionable(r)).map(|r| r.hub_link());
    if !usable && found.is_some() {
        if link.is_some() {
            list.push((tr("Install with Unity Hub"), icons::ARROW_SQUARE_OUT, true, "hub"));
        }
        list.push((tr("Locate…"), icons::FOLDER_OPEN, state.u6.busy().is_none(), "locate"));
    }
    if list.is_empty() {
        return;
    }
    let salted: Vec<(&str, &str, bool, String)> = list.iter().map(|(l, g, e, s)| (*l, *g, *e, format!("{salt}-{s}"))).collect();
    let refs: Vec<(&str, &str, bool, &str)> = salted.iter().map(|(l, g, e, s)| (*l, *g, *e, s.as_str())).collect();
    match buttons(ui, &refs, false).map(|i| list[i].3) {
        Some("hub") => open_url(&ctx, cx, &link.unwrap_or_default()),
        Some("locate") => {
            let version = version.to_string();
            state.u6.act(&ctx, tr("Locating the editor…"), move || {
                let Some(file) = rfd::FileDialog::new().set_title(tr!("Locate Unity {version}", version = version)).pick_file() else { return Ok(String::new()) };
                let c = editors::locate(&orbiters_projects::data_dir(), &file, &version, &editors::probe)?;
                Ok(match c.check {
                    editors::Check::Verified => tr!("Unity {version} located and verified.", version = version),
                    _ => tr!("Unity {version} located: its version could not be read, Orbiters uses it as you chose.", version = version),
                })
            });
        }
        _ => {}
    }
}

fn modules_line(ui: &mut Ui, m: editors::Modules) {
    let android = if m.android_tools {
        tr("Android: with SDK, NDK and JDK")
    } else if m.android {
        tr("Android: without its SDK, NDK or JDK")
    } else {
        tr("Android: not installed")
    };
    let ios = if m.ios { tr("iOS: installed") } else { tr("iOS: not installed") };
    para(ui, &format!("{android} · {ios}"), 11.5, theme::fg_4());
}

/// What a package's manifest and the release list say, for its row.
fn package_rows(ui: &mut Ui, state: &State, rows: &[compat::Row]) {
    let m = state.u6.matrix();
    let target = m.unity6_target().map(|r| r.sdk.clone()).unwrap_or_default();
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            widgets::separator(ui);
        }
        let source = match r.source {
            compat::Source::Vpm => tr("From a VPM repository"),
            compat::Source::Embedded => tr("Embedded in Packages"),
            compat::Source::UpmRegistry => tr("From a Unity package registry"),
            compat::Source::UpmGit => tr("From Git"),
            compat::Source::UpmLocal => tr("A local package"),
        };
        let mut lines = vec![source.to_string()];
        lines.push(match &r.unity_min {
            Some(u) => tr!("Declares Unity {unity} or later (a minimum, not a test).", unity = u),
            None => tr("Declares no Unity version.").to_string(),
        });
        if let Some(asked) = &r.requested {
            lines.push(tr!("Its manifest asks for {asked}.", asked = asked));
        }
        for range in &r.sdk_ranges {
            lines.push(match range.admits_target {
                Some(compat::Admits::Yes) => tr!("Its SDK range {range} takes SDK {sdk}.", range = range.range, sdk = target),
                Some(compat::Admits::No) => tr!("Its SDK range {range} leaves out SDK {sdk}.", range = range.range, sdk = target),
                Some(compat::Admits::DependsOnPrerelease) => tr!("Its SDK range {range} takes SDK {sdk} only when pre-releases are allowed.", range = range.range, sdk = target),
                None => tr!("Its SDK range is {range}.", range = range.range),
            });
        }
        for issue in &r.issues {
            let who = match issue.kind {
                matrix::IssueKind::Publisher => tr("Publisher"),
                matrix::IssueKind::Report => tr("Reported"),
            };
            lines.push(format!("{who} ({}): {} {}", issue.date, issue.text(), issue.link));
        }
        lines.push(tr("Tested on Unity 6 by Orbiters: no evidence.").to_string());
        let title = match &r.version {
            Some(v) => format!("{} {v}", r.display),
            None => r.display.clone(),
        };
        let tint = if r.sdk_ranges.iter().any(|s| s.admits_target == Some(compat::Admits::No)) { theme::WARN } else { theme::fg_3() };
        item(ui, art::glyph::PACKAGE, tint, &title, Some(&lines.join("\n")));
    }
}

fn check_text(c: &compat::Check) -> (&'static str, &'static str) {
    match c.item {
        compat::Item::UdonSharp => (tr("UdonSharp scripts"), tr("Unity's API updater renames velocity to linearVelocity in them.")),
        compat::Item::UdonGraph => (tr("Udon Graph programs"), tr("The Upgrade Wizard migrates them.")),
        compat::Item::CustomUasm => (tr("Udon Assembly written by hand or a tool"), tr("Its velocity externs need a manual change to linearVelocity.")),
        compat::Item::Velocity => (tr("Scripts using a Rigidbody's velocity"), tr("Renamed linearVelocity in Unity 6.")),
        compat::Item::ForceModes => (tr("Forces with Acceleration or VelocityChange"), tr("They act differently in Unity 6: use ForceMode.Force with the force multiplied by the mass.")),
        compat::Item::RingBuffer => (tr("Particles set to Loop Until Replaced"), tr("They now stop at Max Particles: look at them, raise the limit if they stop emitting.")),
        compat::Item::TextMeshPro => (tr("TextMeshPro"), tr("Its package is updated, and each scene needs its colour conversion (the Upgrade Wizard offers it when the scene opens).")),
        compat::Item::NavMesh => (tr("A NavMeshObstacle on a NavMeshAgent"), tr("Works differently in Unity 6: the Upgrade Wizard and the SDK Builder offer to remove the obstacle.")),
        compat::Item::ParameterNames => (tr("Synced avatar parameters"), tr("Avatars uploaded with SDK 3.11 or later match them by name again; names a tool makes at build time are only known after a build.")),
        compat::Item::PackageMaker => (tr("Package Maker"), tr("Now part of the base SDK, no longer of the VPM Resolver.")),
        compat::Item::Shaders => (tr("Shaders"), tr("Look at the materials on each platform you upload for.")),
    }
}

fn checklist(ui: &mut Ui, list: &compat::Checklist) {
    for c in &list.checks {
        let (title, what) = check_text(c);
        let (glyph, tint, seen) = match c.seen {
            compat::Seen::Found => (icons::WARNING_CIRCLE, theme::WARN, tr("Found in this project")),
            compat::Seen::NotFound => (icons::CHECK, theme::fg_4(), tr("Not found")),
            compat::Seen::CannotTell => (icons::INFO, theme::fg_3(), tr("Cannot tell from its files")),
        };
        let mut detail = format!("{seen}. {what}");
        if !c.at.is_empty() {
            detail.push('\n');
            detail.push_str(&c.at.join(", "));
        }
        item(ui, glyph, tint, title, Some(&detail));
    }
    let mut foot = tr!("Orbiters opened and built none of this project's {count} scenes: Build & Test each scene, on each platform you upload for.", count = list.scenes);
    if !list.text_assets {
        foot.push(' ');
        foot.push_str(tr("Its assets are not saved as text, so scenes and prefabs could not be read here."));
    }
    if !list.skipped.is_empty() {
        foot.push(' ');
        foot.push_str(&tr!("Not read (too large, a link or unreadable): {files}.", files = list.skipped.join(", ")));
    }
    if list.cut {
        foot.push(' ');
        foot.push_str(tr("The look stopped before the end of its files."));
    }
    para(ui, &foot, 11.5, theme::fg_4());
}

/// The warning shown before a trial exists, and on a trial: the content IDs it shares with its original.
pub(super) fn ids_warning(ui: &mut Ui, worlds: bool) {
    let text = if worlds {
        tr("The trial keeps the original's world IDs. Once a world is uploaded from Unity 6, VRChat no longer lets it be uploaded from Unity 2022. Test with Build & Test, or give the trial a test world ID of its own before uploading from it.")
    } else {
        tr("The trial keeps the original's avatar IDs. Once an avatar is uploaded from Unity 6, VRChat no longer lets it be uploaded from Unity 2022. Test with Build & Test, or give the trial a test avatar ID of its own before uploading from it.")
    };
    notice(ui, Tone::Warn, tr("Uploading from Unity 6 cannot be undone"), text);
}

pub(super) fn card(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    let trial = state.u6.trial_at(&project.path).cloned();
    let Some(s) = state.u6.standing(&project.path).cloned() else { return };
    if !s.in_transition() && trial.is_none() {
        return;
    }
    let mut guide = false;
    art::card(
        ui,
        "unity6",
        icons::CUBE,
        tr("Unity 6"),
        None,
        |ui, slot| {
            let b = Button::new(tr("VRChat's guide")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Ghost).size(Size::Sm).id_salt("u6-guide");
            let bw = b.desired_width(ui).min(slot.width());
            guide = widgets::activated(&b.show_at(ui, Rect::from_min_size(pos2(slot.right() - bw, slot.center().y - 16.0), vec2(bw, 32.0))).on_hover_text(GUIDE));
        },
        |ui| {
            if let Some(e) = state.u6.error.clone() {
                if orbiters_ui::feedback::banner(ui, egui::Id::unique(("u6-error", &e)), Tone::Danger, &e, true) {
                    state.u6.error = None;
                }
            }
            if let Some(label) = state.u6.busy() {
                para(ui, label, 12.0, theme::fg_3());
            }
            match &trial {
                Some((job, status)) => trial_body(ui, cx, state, project, &s, job, status),
                None => project_body(ui, cx, state, project, &s),
            }
        },
    );
    if guide {
        open_url(&ctx, cx, GUIDE);
    }
}

fn project_body(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project, s: &standing::Standing) {
    let ctx = ui.ctx().clone();
    let m = state.u6.matrix();
    let target = m.unity6_target().cloned();
    let title = match s.sdk.version() {
        Some(sdk) => tr!("Unity {editor} · SDK {sdk}", editor = s.editor.text, sdk = sdk),
        None => tr!("Unity {version}", version = s.editor.text),
    };
    item(ui, icons::CUBE, theme::fg_3(), &title, Some(&sentence(s, &m)));
    if let Some(t) = &target {
        let events = &state.u6.matrix().data.events;
        let mut text = match (&events.unity6_stable, t.channel) {
            (Some(day), _) => tr!("VRChat released its Unity 6 SDK on {day}. Orbiters' trial target is SDK {sdk} on Unity {unity}.", day = day, sdk = t.sdk, unity = t.unity),
            (None, matrix::Channel::Beta) => tr!("VRChat's Unity 6 SDK is in open beta: SDK {sdk} on Unity {unity}. VRChat has announced no date for its release.", sdk = t.sdk, unity = t.unity),
            (None, matrix::Channel::Stable) => tr!("Orbiters' trial target is SDK {sdk} on Unity {unity}.", sdk = t.sdk, unity = t.unity),
        };
        text.push(' ');
        text.push_str(&match &events.unity2022_retirement {
            Some(day) => tr!("VRChat stops accepting uploads from Unity 2022 on {day}.", day = day),
            None => tr("VRChat has announced no end date for Unity 2022.").to_string(),
        });
        para(ui, &text, 12.0, theme::fg_3());
    }
    release_list(ui, &m);
    // Its trials.
    let pairs: Vec<(PathBuf, String)> = state.u6.trials_of(&project.path).into_iter().map(|p| (p.trial.clone(), p.job.clone())).collect();
    for (trial, _) in &pairs {
        let name = trial.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        item(ui, icons::COPY, theme::accent(), &tr!("Unity 6 trial: {name}", name = name), Some(&trial.display().to_string()));
        if buttons(ui, &[(tr("Show the trial"), icons::ARROW_RIGHT, true, "show-trial")], false).is_some() {
            state.open(&ctx, trial.clone());
        }
    }
    if state.u6.mac() {
        para(
            ui,
            tr("On a Mac, Creator Companion's migration is not available: its window is for Windows. Orbiters shows where the project stands and what the guide's changes involve; the migration itself is done on Windows."),
            12.0,
            theme::fg_3(),
        );
    } else if s.state == standing::State::On2022 && target.as_ref().is_some_and(|t| m.actionable(t)) && project.registered && !project.missing {
        let anchor = ui.cursor().min;
        if buttons(ui, &[(tr("Try on Unity 6…"), icons::COPY, state.u6.run.is_none() && !state.running(), "try")], true).is_some() {
            super::unity6_sheet::open(&ctx, state, project, Rect::from_min_size(anchor, vec2(160.0, 32.0)));
        }
        state.anchors.insert("u6-try".into(), Rect::from_min_size(anchor, vec2(160.0, 32.0)));
    } else {
        let why = match s.state {
            standing::State::OnUnity6 => tr("This project is on Unity 6 already."),
            _ if !project.registered => tr("Add the project to your list first."),
            _ => tr("A trial starts from a VRChat project on Unity 2022 with its SDK installed."),
        };
        para(ui, why, 12.0, theme::fg_3());
    }
    // The editors: its own, and Unity 6's for a project on 2022.
    let mut versions = vec![s.editor.text.clone()];
    if let Some(t) = target.as_ref().filter(|_| matches!(s.state, standing::State::On2022 | standing::State::Disagree)) {
        versions.push(t.unity.clone());
    }
    state.u6.want_editors(&ctx, &versions);
    art::caps(ui, tr("Editors"));
    for (i, v) in versions.iter().enumerate() {
        editor_row(ui, cx, state, v, &format!("editor-{i}"));
    }
    // What can be said before a try.
    let content = s.sdk.content.unwrap_or(standing::Content::Base);
    state.u6.want_compat(&ctx, &project.path, content);
    let compat = state.u6.compat.clone().filter(|(p, _, _)| *p == project.path);
    art::caps(ui, tr("Before a try"));
    match &compat {
        None => para(ui, tr("Reading the project's packages and files…"), 12.0, theme::fg_3()),
        Some((_, rows, list)) => {
            let label = if state.u6.show_packages { tr("Hide the packages").to_string() } else { tr!("Show what its {count} packages declare", count = rows.len()) };
            if !rows.is_empty() && buttons(ui, &[(&label, art::glyph::PACKAGE, true, "packages")], false).is_some() {
                state.u6.show_packages = !state.u6.show_packages;
            }
            if state.u6.show_packages {
                package_rows(ui, state, rows);
            }
            checklist(ui, list);
        }
    }
}

/// What to do in Creator Companion while a trial waits for it.
pub(super) fn vcc_steps(ui: &mut Ui, state: &mut State, trial: &Path, target: &trial::Target) {
    let ctx = ui.ctx().clone();
    state.u6.want_vccs(&ctx);
    let needed = state.u6.matrix().unity6_target().and_then(|r| r.vcc.clone()).unwrap_or_default();
    let vccs = state.u6.vccs.clone().unwrap_or_default();
    let able = vccs.iter().find(|v| v.can_migrate(&needed) == Some(true));
    let first = match (able, vccs.first()) {
        (Some(v), _) => tr!("Open Creator Companion {version} ({folder}).", version = v.version.clone().unwrap_or_default(), folder = v.folder.display()),
        (None, Some(v)) => tr!(
            "The Creator Companion found here is {version}: migrating to Unity 6 needs {needed} or later, from VRChat's guide.",
            version = v.version.clone().unwrap_or_else(|| tr("of a version it does not say").into()),
            needed = needed
        ),
        (None, None) => tr!("Get Creator Companion {needed} or later from VRChat's guide (its beta comes as a portable folder), then open it.", needed = needed),
    };
    let name = trial.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let steps = [
        first,
        tr!("Add this folder in Creator Companion, with its Add Existing Project: {path}. Orbiters adds nothing to Creator Companion's list.", path = trial.display()),
        tr!("Click the Unity column beside {name}, then Migrate to Unity 6000.", name = name),
        tr("Choose to migrate this project itself: it is already a copy. If Creator Companion makes a migrated copy beside it instead, Orbiters can take that one.").to_string(),
        tr!("Let it finish: it opens the project in Unity {unity}, where VRChat's Upgrade Wizard asks you about the changes.", unity = target.unity),
    ];
    for (i, step) in steps.iter().enumerate() {
        numbered(ui, i + 1, step);
    }
    let list = [
        (tr("Copy the trial's path"), icons::COPY, true, "copy-path"),
        (tr("Show the trial's folder"), icons::FOLDER_OPEN, true, "show-folder"),
        (tr("Locate Creator Companion…"), icons::FOLDER_OPEN, state.u6.busy().is_none(), "locate-vcc"),
        (tr("Look again"), icons::ARROW_COUNTER_CLOCKWISE, true, "look"),
    ];
    match buttons(ui, &list, false) {
        Some(0) => ctx.copy_text(trial.display().to_string()),
        Some(1) => {
            orbiters_projects::reveal(trial).or_log("a folder could not be shown");
        }
        Some(2) => {
            state.u6.vccs = None;
            state.u6.act(&ctx, tr("Locating Creator Companion…"), move || {
                let Some(file) = rfd::FileDialog::new().set_title(tr("Locate Creator Companion")).pick_file() else { return Ok(String::new()) };
                let v = orbiters_projects::unity6::vcc::choose(&orbiters_projects::data_dir(), &file)?;
                Ok(tr!("Creator Companion {version} located.", version = v.version.unwrap_or_default()))
            });
        }
        Some(3) => {
            state.u6.act(&ctx, "", || Ok(String::new()));
        }
        _ => {}
    }
}

fn outcome_text(o: &outcome::Outcome) -> (Tone, String) {
    let list = |errors: &[outcome::Diagnostic]| {
        errors.iter().take(4).map(|d| format!("{}:{} {} {}", d.file, d.line, d.code, d.message)).collect::<Vec<_>>().join("\n")
    };
    match o {
        outcome::Outcome::DidNotStart(why) => (Tone::Danger, tr!("Unity did not start: {why}", why = why)),
        outcome::Outcome::ExitedAtOnce(_) => (Tone::Danger, tr("Unity quit right after starting.").to_string()),
        outcome::Outcome::Importing => (Tone::Info, tr("Unity is open, and its log shows the first import begun.").to_string()),
        outcome::Outcome::SafeMode { errors, .. } => (Tone::Warn, format!("{}\n{}", tr("Unity opened in Safe Mode: it holds back the project's scripts until their errors are fixed."), list(errors))),
        outcome::Outcome::CompilerErrors { errors, .. } => (
            Tone::Warn,
            format!("{}\n{}", tr!("The log lists {count} compiler errors, by where they are (the folder holding a file is not necessarily its cause):", count = errors.len()), list(errors)),
        ),
        // Only the log's end was read: an error before it would not be seen.
        outcome::Outcome::Imported { cut: true, .. } => (Tone::Info, tr("The import finished. The log is too long to be read whole: the part read lists no compiler error. Check with the SDK's Build & Test.").to_string()),
        outcome::Outcome::Imported { running: true, .. } => (Tone::Ok, tr("The import finished with no compiler error in this log. Unity is open: VRChat's Upgrade Wizard may be waiting for you there. Then check with the SDK's Build & Test.").to_string()),
        outcome::Outcome::Imported { running: false, .. } => (Tone::Ok, tr("The import finished with no compiler error in this log, and Unity is closed. Check with the SDK's Build & Test.").to_string()),
        outcome::Outcome::Undetermined { why, running } => {
            let mut text = match why {
                outcome::Unknown::NoLog => tr("Cannot tell: Unity wrote no log for this opening."),
                outcome::Unknown::Empty => tr("Cannot tell: the log is empty."),
                outcome::Unknown::EndedEarly => tr("Cannot tell: Unity closed before its log said the import finished."),
                outcome::Unknown::Unreadable => tr("Cannot tell: the log could not be read."),
                outcome::Unknown::NotYet => tr("Cannot tell yet: its log says nothing Orbiters reads so far."),
            }
            .to_string();
            if *running {
                text.push(' ');
                text.push_str(tr("Unity is still running."));
            }
            (Tone::Off, text)
        }
    }
}

fn trial_body(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Project, s: &standing::Standing, job: &trial::Job, status: &trial::Status) {
    let ctx = ui.ctx().clone();
    let worlds = s.sdk.content == Some(standing::Content::Worlds) || project.kind == orbiters_projects::discovery::Kind::Worlds;
    item(
        ui,
        icons::COPY,
        theme::accent(),
        &tr!("Unity 6 trial of {name}", name = job.source.name),
        Some(&tr!("Made {day}. Fixes made to the original after that day are not in the trial: before moving for good, make a fresh trial.", day = day(job.forked()))),
    );
    // The first copy, once Creator Companion's own migrated copy was taken: it stays on 2022, with nothing to open.
    if job.migrated.is_some() && orbiters_projects::discovery::key(&job.trial.path) == orbiters_projects::discovery::key(&project.path) {
        para(ui, tr("Creator Companion's migrated copy is this trial's Unity 6 copy now; this first copy stays on Unity 2022 and is not changed from here."), 12.0, theme::fg_3());
        let list = [(tr("Show the migrated copy"), icons::ARROW_RIGHT, true, "migrated"), (tr("Open the folder"), icons::FOLDER_OPEN, true, "reveal-first")];
        match buttons(ui, &list, false) {
            Some(0) => state.open(&ctx, job.unity6().path.clone()),
            Some(1) => {
                orbiters_projects::reveal(&job.trial.path).or_log("a folder could not be shown");
            }
            _ => {}
        }
        return;
    }
    ids_warning(ui, worlds);
    if !job.linked.is_empty() {
        para(ui, &tr!("Links of the original whose files were copied: {links}.", links = job.linked.join(", ")), 12.0, theme::fg_3());
    }
    if !job.localized.is_empty() {
        para(ui, &tr!("Local packages made part of the trial, named so in its Packages/manifest.json: {packages}.", packages = job.localized.join(", ")), 12.0, theme::fg_3());
    }
    if state.u6.mac() {
        para(ui, tr("On a Mac, Creator Companion's migration and opening a trial in Unity 6 from Orbiters are not available: this trial is shown for what its files say. Its migration is done on Windows."), 12.0, theme::fg_3());
    }
    match status {
        _ if state.u6.mac() => {}
        trial::Status::Waiting { vcc_copy } => {
            art::caps(ui, tr("Next, in Creator Companion"));
            para(ui, tr("Orbiters does not change the trial's packages or editor version: Creator Companion migrates it, as VRChat's guide describes. Until then the trial stays on Unity 2022 and is not opened from here."), 12.0, theme::fg_3());
            if let Some(beside) = vcc_copy {
                item(ui, icons::CHECK_CIRCLE, theme::OK, tr("Creator Companion made a migrated copy"), Some(&beside.display().to_string()));
                if buttons(ui, &[(tr("Use the migrated copy"), icons::CHECK, state.u6.busy().is_none(), "adopt")], true).is_some() {
                    let (id, beside) = (job.id.clone(), beside.clone());
                    state.u6.act(&ctx, tr("Taking the migrated copy…"), move || {
                        trial::adopt_vcc_copy(&orbiters_projects::data_dir(), &id, &beside)?;
                        Ok(tr("The migrated copy is the trial now.").to_string())
                    });
                }
            } else if buttons(ui, &[(tr("Locate the migrated copy…"), icons::FOLDER_OPEN, state.u6.busy().is_none(), "adopt-locate")], false).is_some() {
                let (id, start) = (job.id.clone(), job.trial.path.parent().map(Path::to_path_buf));
                state.u6.act(&ctx, tr("Taking the migrated copy…"), move || {
                    let mut d = rfd::FileDialog::new().set_title(tr("The copy Creator Companion migrated"));
                    if let Some(start) = start {
                        d = d.set_directory(start);
                    }
                    let Some(folder) = d.pick_folder() else { return Ok(String::new()) };
                    trial::adopt_vcc_copy(&orbiters_projects::data_dir(), &id, &folder)?;
                    Ok(tr("The migrated copy is the trial now.").to_string())
                });
            }
            vcc_steps(ui, state, &job.trial.path, &job.target);
        }
        trial::Status::OnUnity6 => {
            art::caps(ui, tr("On Unity 6"));
            para(ui, &sentence(s, &state.u6.matrix()), 12.0, theme::fg_3());
            let version = s.editor.text.clone();
            state.u6.want_editors(&ctx, std::slice::from_ref(&version));
            editor_row(ui, cx, state, &version, "trial-editor");
            let found = state.u6.editors.get(&version).cloned();
            let candidate = found.as_ref().and_then(|f| match f {
                editors::Found::Exact(c) => Some(c.clone()),
                _ => None,
            });
            let open = state.session(&job.unity6().path).open();
            if !open {
                let enabled = candidate.is_some() && state.u6.busy().is_none();
                if buttons(ui, &[(tr("Open in Unity 6"), icons::PLAY, enabled, "open-u6")], true).is_some() {
                    if candidate.is_some() {
                        let (id, unity) = (job.id.clone(), version.clone());
                        state.u6.act(&ctx, tr("Opening the trial in Unity 6…"), move || {
                            let start = |program: &Path, project: &Path, log: &Path| orbiters_projects::start_logged(program, project, log, &unity);
                            // Found and probed again now: what was found earlier is not trusted at the launch.
                            let a = trial::launch(&orbiters_projects::data_dir(), &id, &|v, build| editors::find_build(v, Some(build)), &trial::is_open, &trial::PROCESSES, &start)?;
                            Ok(tr!("Unity is starting. This opening's log: {log}", log = a.log.display()))
                        });
                    }
                }
            } else {
                para(ui, tr("Unity has the trial open."), 12.0, theme::fg_3());
            }
            state.u6.want_outcomes(&ctx, job);
            for a in job.attempts.iter().rev().take(4) {
                let o = state.u6.outcomes.get(&(job.id.clone(), a.n)).cloned().or(a.start.clone());
                let (tone, text) = match &o {
                    Some(o) => outcome_text(o),
                    None => (Tone::Off, tr("Reading this opening's log…").to_string()),
                };
                let glyph = match tone {
                    Tone::Ok => icons::CHECK_CIRCLE,
                    Tone::Danger | Tone::Warn => icons::WARNING_CIRCLE,
                    _ => icons::INFO,
                };
                item(ui, glyph, tone.color(), &tr!("Opening {n} · {day}", n = a.n, day = day(a.started)), Some(&text));
            }
            if let Some(last) = job.attempts.last() {
                let log = last.log.clone();
                let written = !matches!(state.u6.outcomes.get(&(job.id.clone(), last.n)), None | Some(outcome::Outcome::Undetermined { why: outcome::Unknown::NoLog, .. }));
                if buttons(ui, &[(tr("Show the last log"), icons::FOLDER_OPEN, written, "log")], false).is_some() {
                    orbiters_projects::reveal(&log).or_log("a folder could not be shown");
                }
            }
        }
        trial::Status::Unexpected(state_now) => {
            let now = standing::Standing { state: *state_now, ..s.clone() };
            para(ui, &tr!("The trial's files show this now: {what}", what = sentence(&now, &state.u6.matrix())), 12.0, theme::fg_3());
        }
        _ => {}
    }
    // What can always be done with a trial: nothing here deletes it.
    let original = state.u6.look.as_ref().and_then(|l| l.pairs.iter().find(|p| p.job == job.id)).map(|p| p.original.clone());
    let list = [
        (tr("Show the original"), icons::ARROW_LEFT, original.is_some(), "original"),
        (tr("Open the folder"), icons::FOLDER_OPEN, true, "reveal"),
        (tr("Forget the trial"), icons::X, state.u6.busy().is_none(), "forget-trial"),
    ];
    // Where trials are only shown, nothing is changed of them.
    let list = if state.u6.mac() { &list[..2] } else { &list[..] };
    match buttons(ui, list, false) {
        Some(0) => {
            if let Some(original) = original {
                state.open(&ctx, original);
            }
        }
        Some(1) => {
            orbiters_projects::reveal(&job.unity6().path).or_log("a folder could not be shown");
        }
        Some(2) => {
            let id = job.id.clone();
            state.u6.act(&ctx, tr("Forgetting the trial…"), move || {
                let left = trial::abandon(&orbiters_projects::data_dir(), &id)?;
                Ok(forgotten(&left))
            });
        }
        _ => {}
    }
}

/// What taking a trial off did, and what it did not.
fn forgotten(left: &trial::Forgotten) -> String {
    let kept = tr("If you added it in Creator Companion, it is still on its list: remove it there yourself.");
    if left.there {
        format!("{} {kept}", tr!("Orbiters does not show it as a trial any more. Its folder stays at {path}.", path = left.at.display()))
    } else {
        format!("{} {kept}", tr("Orbiters does not show it as a trial any more."))
    }
}

/// The gallery's rows for trials that are not projects (yet): being made, stopped, not where they were.
pub(super) fn job_rows(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    let jobs: Vec<(trial::Job, trial::Status)> = state.u6.jobs().into_iter().cloned().collect();
    let health = state.u6.health();
    if jobs.is_empty() && health == trial::Health::Fine {
        return;
    }
    match health {
        trial::Health::Fine => {}
        trial::Health::Unreadable(why) => {
            ui.add_space(28.0);
            notice(ui, Tone::Warn, tr("Unity 6 trials record damaged"), &tr!("The Unity 6 trials record cannot be read ({why}): Orbiters opens and changes no project until it can, since one of them may be a trial.", why = why));
        }
        trial::Health::Damaged { why, newer, .. } => {
            ui.add_space(28.0);
            let text = if newer {
                tr!("A newer version of Orbiters wrote part of the Unity 6 trials record: this version does not act on the trials it is about ({why}).", why = why)
            } else {
                tr!("Part of the Unity 6 trials record cannot be read ({why}). Every other trial is shown; Orbiters does not act on those it may be about until it is set aside.", why = why)
            };
            notice(ui, Tone::Warn, tr("Unity 6 trials record damaged"), &text);
            if !newer && buttons(ui, &[(tr("Set the damaged record aside"), icons::ARROW_COUNTER_CLOCKWISE, state.u6.busy().is_none(), "set-aside")], false).is_some() {
                state.u6.act(&ctx, tr("Setting the damaged record aside…"), || {
                    let aside = trial::set_aside(&orbiters_projects::data_dir())?;
                    Ok(tr!("The damaged record is kept at {path}.", path = aside.display()))
                });
            }
        }
    }
    if jobs.is_empty() {
        return;
    }
    ui.add_space(28.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    widgets::text(ui.painter(), pos2(r.left(), r.top() + 12.0), Align2::LEFT_CENTER, &Txt::new(tr("Unity 6 trials"), 16.0).weight(weight::SEMIBOLD));
    ui.add_space(6.0);
    for (job, status) in jobs {
        let w = ui.available_width();
        let top = ui.cursor().min;
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(top + vec2(16.0, 14.0), vec2(w - 32.0, 10_000.0))).id_salt(("u6-job", &job.id)));
        inner.spacing_mut().item_spacing = vec2(0.0, 8.0);
        let bg = ui.painter().add(egui::Shape::Noop);
        // What this window does with it now, if it does: its Stop and its progress.
        let run = state.u6.run.as_ref().filter(|r| r.recorded.locked().as_deref() == Some(job.id.as_str())).map(|r| (r.cancel.clone(), r.progress.clone()));
        let finishing = state.u6.finishing.as_ref().filter(|f| f.0 == job.id && state.u6.busy().is_some()).map(|f| (f.1.clone(), f.2.clone()));
        let mine = run.or(finishing);
        let (glyph, tint, text) = match &status {
            trial::Status::Running => {
                match mine.as_ref().map(|(_, p)| p.locked().clone()) {
                    Some(p) => {
                        let pct = if p.total > 0 { format!(" · {:.0}%", 100.0 * p.copied as f32 / p.total as f32) } else { String::new() };
                        (icons::CIRCLE_NOTCH, theme::accent(), format!("{}{pct}", super::unity6::phase_label(&p.phase)))
                    }
                    None => (icons::CIRCLE_NOTCH, theme::accent(), tr("Orbiters is working on it in another window.").to_string()),
                }
            }
            trial::Status::Interrupted => (icons::WARNING_CIRCLE, theme::WARN, tr("The copy was interrupted. This folder is not a project; nothing was deleted. Start a new trial, and remove this folder yourself when you no longer need it.").to_string()),
            trial::Status::Failed { kept } => {
                let mut text = format!("{} {}", job.error.clone().unwrap_or_default(), if *kept { tr("Part of the copy is still in this folder.") } else { tr("Nothing of it is left.") });
                for d in job.differences.iter().take(5) {
                    text.push_str(&format!("\n{d}"));
                }
                if job.differences.len() > 5 {
                    text.push_str(&format!("\n{}", tr!("And {count} more differences.", count = job.differences.len() - 5)));
                }
                (icons::X_CIRCLE, theme::DANGER, text)
            }
            trial::Status::Unfinished => (icons::WARNING_CIRCLE, theme::WARN, job.error.clone().unwrap_or_else(|| tr("The trial is copied and compared; its last step is left.").to_string())),
            trial::Status::Uncertain(why) => (icons::PROHIBIT, theme::WARN, orbiters_projects::unity6::trial::uncertain_text(why)),
            trial::Status::Missing => (icons::HARD_DRIVE, theme::fg_3(), tr("Not found at this path: its disk may not be connected. It shows again once it is there.").to_string()),
            trial::Status::Replaced => (icons::PROHIBIT, theme::fg_3(), tr("Another folder is at this path now: Orbiters does not act on it.").to_string()),
            _ => continue,
        };
        item(&mut inner, glyph, tint, &tr!("{name}, a Unity 6 trial of {original}", name = job.trial.name, original = job.source.name), Some(&format!("{}\n{text}", job.trial.path.display())));
        let mut list: Vec<(&str, &str, bool, &str)> = vec![];
        let mac = state.u6.mac();
        if matches!(status, trial::Status::Unfinished) && !mac {
            list.push((tr("Finish it"), icons::CHECK, state.u6.busy().is_none(), "list"));
        }
        if matches!(status, trial::Status::Running) {
            if mine.is_some() {
                list.push((tr("Stop"), icons::X, true, "stop"));
            }
        } else {
            if state.u6.present(&job.trial.path) {
                list.push((tr("Open the folder"), icons::FOLDER_OPEN, true, "reveal"));
            }
            if !matches!(status, trial::Status::Uncertain(_)) && !mac {
                list.push((tr("Take it off"), icons::X, state.u6.busy().is_none(), "forget"));
            }
        }
        let salted: Vec<(&str, &str, bool, String)> = list.iter().map(|(l, g, e, s)| (*l, *g, *e, format!("{}-{s}", job.id))).collect();
        let refs: Vec<(&str, &str, bool, &str)> = salted.iter().map(|(l, g, e, s)| (*l, *g, *e, s.as_str())).collect();
        let pressed = buttons(&mut inner, &refs, false).map(|i| list[i].3);
        let h = inner.min_rect().height() + 28.0;
        let rect = Rect::from_min_size(top, vec2(w, h));
        ui.painter().set(bg, egui::Shape::mesh(orbiters_ui::paint::squircle_mesh(rect, theme::squircle(theme::CARD_RADIUS), theme::surface(), 1.0 / ctx.pixels_per_point())));
        ui.allocate_exact_size(vec2(w, h), Sense::hover());
        ui.add_space(10.0);
        let id = job.id.clone();
        match pressed {
            Some("list") => {
                let (stop, progress): (std::sync::Arc<std::sync::atomic::AtomicBool>, orbiters_projects::templates::SharedProgress) = Default::default();
                state.u6.finishing = Some((id.clone(), stop.clone(), progress.clone()));
                state.u6.act(&ctx, tr("Finishing the trial…"), move || {
                    trial::finish(&orbiters_projects::data_dir(), &id, &stop, &progress)?;
                    Ok(tr("The trial is ready for Creator Companion.").to_string())
                });
            }
            Some("stop") => {
                if let Some((stop, _)) = &mine {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Some("reveal") => {
                orbiters_projects::reveal(&job.trial.path).or_log("a folder could not be shown");
            }
            // Its folder stays, and stays known.
            Some("forget") => state.u6.act(&ctx, tr("Taking it off…"), move || {
                let left = trial::abandon(&orbiters_projects::data_dir(), &id)?;
                Ok(forgotten(&left))
            }),
            _ => {}
        }
    }
}

/// The gallery's rows for copies of projects (Duplicate, a new project) that are not on the list: cut short, or whole and not
/// listed (AGENTS.md: a retained copy is visible and actionable, with where it is and what is wrong).
pub(super) fn copy_rows(ui: &mut Ui, state: &mut State) {
    use orbiters_projects::copies;
    let ctx = ui.ctx().clone();
    let rows: Vec<(copies::Copy, copies::Status)> = state.u6.look.as_ref().map(|l| l.copies.clone()).unwrap_or_default();
    // The one this window is making shows its progress in its dialog.
    let rows: Vec<_> = rows.into_iter().filter(|(_, s)| *s != copies::Status::Running).collect();
    if rows.is_empty() {
        return;
    }
    ui.add_space(28.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    widgets::text(ui.painter(), pos2(r.left(), r.top() + 12.0), Align2::LEFT_CENTER, &Txt::new(tr("Copies to look at"), 16.0).weight(weight::SEMIBOLD));
    ui.add_space(6.0);
    for (copy, status) in rows {
        let w = ui.available_width();
        let top = ui.cursor().min;
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(top + vec2(16.0, 14.0), vec2(w - 32.0, 10_000.0))).id_salt(("copy-row", &copy.id)));
        inner.spacing_mut().item_spacing = vec2(0.0, 8.0);
        let bg = ui.painter().add(egui::Shape::Noop);
        let said = copy.error.clone().unwrap_or_default();
        let (glyph, tint, text) = match status {
            copies::Status::Unlisted => (icons::WARNING_CIRCLE, theme::WARN, format!("{} {said}", tr("Every file of the copy is there, but it is not on the project list."))),
            copies::Status::Incomplete => (icons::X_CIRCLE, theme::DANGER, format!("{} {said}", tr("Incomplete copy: it was cut short, so this folder is not a project. Delete it and make the copy again, or look into its folder."))),
            copies::Status::Missing => (icons::HARD_DRIVE, theme::fg_3(), format!("{} {said}", tr("Not found at this path: its disk may not be connected, or another folder is there."))),
            copies::Status::Running => continue,
        };
        // (One found without a record does not say what it is a copy of.)
        let name = copy.source.file_name().or(copy.destination.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        item(&mut inner, glyph, tint, &tr!("A copy of {name}", name = name), Some(&format!("{}\n{}", copy.destination.display(), text.trim())));
        let free = state.u6.busy().is_none() && !state.running();
        let mut list: Vec<(&str, &str, bool, &str)> = vec![];
        if status == copies::Status::Unlisted {
            list.push((tr("List it"), icons::PLUS, free, "list"));
        }
        if status != copies::Status::Missing {
            list.push((tr("Open the folder"), icons::FOLDER_OPEN, true, "reveal"));
        }
        // An incomplete copy is deleted, after a second press, or stays shown.
        match status {
            copies::Status::Incomplete if state.u6.deleting.as_ref() == Some(&copy.id) => {
                list.push((tr("Delete its files"), super::art::glyph::TRASH_SIMPLE, free, "delete"));
                list.push((tr("Cancel"), icons::X, true, "keep"));
            }
            copies::Status::Incomplete => list.push((tr("Delete the incomplete copy…"), super::art::glyph::TRASH_SIMPLE, free, "ask")),
            _ => list.push((tr("Dismiss"), icons::X, free, "dismiss")),
        }
        let salted: Vec<(&str, &str, bool, String)> = list.iter().map(|(l, g, e, s)| (*l, *g, *e, format!("copy-{}-{s}", copy.id))).collect();
        let refs: Vec<(&str, &str, bool, &str)> = salted.iter().map(|(l, g, e, s)| (*l, *g, *e, s.as_str())).collect();
        let pressed = buttons(&mut inner, &refs, false).map(|i| list[i].3);
        let h = inner.min_rect().height() + 28.0;
        let rect = Rect::from_min_size(top, vec2(w, h));
        ui.painter().set(bg, egui::Shape::mesh(orbiters_ui::paint::squircle_mesh(rect, theme::squircle(theme::CARD_RADIUS), theme::surface(), 1.0 / ctx.pixels_per_point())));
        ui.allocate_exact_size(vec2(w, h), Sense::hover());
        ui.add_space(10.0);
        let id = copy.id.clone();
        match pressed {
            Some("list") => state.u6.act(&ctx, tr("Listing the copy…"), move || {
                let at = copies::list(&orbiters_projects::data_dir(), &orbiters_projects::vcc_dir(), &id)?;
                Ok(tr!("The copy at {path} is listed.", path = at.display()))
            }),
            Some("reveal") => {
                orbiters_projects::reveal(&copy.destination).or_log("a folder could not be shown");
            }
            Some("ask") => state.u6.deleting = Some(id),
            Some("keep") => state.u6.deleting = None,
            Some("delete") => {
                state.u6.deleting = None;
                state.u6.act(&ctx, tr("Deleting the incomplete copy…"), move || {
                    let at = copies::delete(&orbiters_projects::data_dir(), &id)?;
                    Ok(tr!("The incomplete copy at {path} was deleted.", path = at.display()))
                });
            }
            Some("dismiss") => state.u6.act(&ctx, tr("Dismissing the copy…"), move || {
                let at = copies::dismiss(&orbiters_projects::data_dir(), &id)?;
                Ok(tr!("Dismissed. The folder stays at {path}.", path = at.display()))
            }),
            _ => {}
        }
    }
}
