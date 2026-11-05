//! A project already open in Unity: where "Open in Unity" was (the panel's hero card, its compact bar, the right-click
//! menus), Show (its window to the front, restored when minimised, or the dialog it waits on), Save & Exit (Unity asked
//! to quit as its close button does: it asks whether to save modified scenes, that question brought to the front, and
//! the panel says when it quit) and Kill (that project's editor ended after a confirmation; other editors untouched,
//! nothing deleted). The editors are looked at on a worker every second and a half while the page is shown
//! (`orbiters_projects::editor`).
use super::state::State;
use crate::pages::{Cx, Job};
use egui::{pos2, vec2, Align2, Rect, Ui};
use orbiters_projects::discovery::{self, Project};
use orbiters_projects::editor::{self, Event, Session};
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};
use orbiters_ui::{feedback, icons};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the editors are looked at.
const EVERY: Duration = Duration::from_millis(1500);
/// After Unity's save question was answered, how long it may take to quit before it counts as staying open (Cancel).
const QUITTING: Duration = Duration::from_secs(20);

impl State {
    /// The session of `project`'s editor.
    pub(super) fn session(&self, path: &std::path::Path) -> Session {
        self.sessions.get(&discovery::key(path)).map(|s| s.0).unwrap_or_default()
    }
    pub(super) fn poll_editors(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let Some(r) = self.editor_action.take() {
            if let Err(e) = r {
                self.error = Some(e);
                // What was asked did not happen: the next look says what Unity is doing.
                for s in self.sessions.values_mut() {
                    if let Session::Closing { pid, .. } | Session::Killing { pid } = s.0 {
                        s.0 = Session::Open { pid };
                    }
                }
            }
        }
        if self.fixture {
            return;
        }
        if let Some(seen) = self.editors_job.take() {
            self.apply_sessions(ctx, cx, seen);
        }
        if !self.editors_job.running() && self.editors_at.is_none_or(|t| t.elapsed() >= EVERY) {
            self.editors_at = Some(Instant::now());
            let projects: Vec<PathBuf> = self.scan.projects.iter().map(|p| p.path.clone()).collect();
            self.editors_job = Job::spawn(ctx, move || {
                let editors = editor::running();
                let unread = editors.iter().any(|e| e.project.as_os_str().is_empty());
                projects
                    .iter()
                    .map(|p| {
                        // The lock file is read only for an editor whose command line could not be.
                        let locked = unread && orbiters_projects::unity_open(p);
                        (discovery::key(p), editor::of(&editors, p, locked).map(|e| (e.pid, e.asking)))
                    })
                    .collect()
            });
        }
        ctx.request_repaint_after(EVERY);
    }
    fn apply_sessions(&mut self, ctx: &egui::Context, cx: &mut Cx, seen: HashMap<String, Option<(u32, bool)>>) {
        let now = Instant::now();
        for (key, seen) in seen {
            let (old, answered) = self.sessions.get(&key).copied().unwrap_or_default();
            let mut new = old.next(Event::Seen(seen));
            let project = self.scan.projects.iter().find(|p| discovery::key(&p.path) == key).cloned();
            let name = project.as_ref().map(|p| p.name.clone()).unwrap_or_default();
            // Unity asks whether to save: its question to the front.
            if let (Session::Closing { asking: false, .. }, Session::Closing { pid, asking: true, .. }) = (old, new) {
                if let Some(p) = &project {
                    let path = p.path.clone();
                    std::thread::spawn(move || editor::show(pid, &path));
                }
            }
            let mut at = None;
            if let Session::Closing { answered: true, asking: false, .. } = new {
                let since = *at.insert(answered.unwrap_or(now));
                if now.duration_since(since) > QUITTING {
                    new = new.next(Event::Stayed);
                    at = None;
                    cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, format!("Unity kept {name} open"));
                }
            }
            if old.ended(new) {
                let said = if matches!(old, Session::Killing { .. }) { format!("Unity was ended: {name} can be opened again") } else { format!("Unity saved and closed {name}") };
                cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, said);
                if self.kill_confirm.as_ref().is_some_and(|p| discovery::key(p) == key) {
                    self.kill_confirm = None;
                }
            }
            if new == Session::Closed {
                self.sessions.remove(&key);
            } else {
                self.sessions.insert(key, (new, at));
            }
        }
    }
    fn editor_act(&mut self, ctx: &egui::Context, project: &Project, event: Event) {
        let key = discovery::key(&project.path);
        let Some((session, _)) = self.sessions.get(&key).copied() else { return };
        let Some(pid) = session.pid() else { return };
        let path = project.path.clone();
        if event != Event::Seen(None) {
            let next = session.next(event);
            self.sessions.insert(key, (next, None));
        }
        if self.fixture {
            return;
        }
        self.editor_action = Job::spawn(ctx, move || {
            match event {
                Event::Close => editor::close(pid, &path),
                Event::Kill => editor::kill(pid, &path),
                _ => editor::show(pid, &path),
            }
            .map_err(|e| format!("{e:#}"))
        });
    }
    pub(super) fn show_editor(&mut self, ctx: &egui::Context, project: &Project) {
        self.editor_act(ctx, project, Event::Seen(None));
    }
    pub(super) fn save_and_exit(&mut self, ctx: &egui::Context, project: &Project) {
        self.editor_act(ctx, project, Event::Close);
    }
    pub(super) fn kill_editor(&mut self, ctx: &egui::Context, project: &Project) {
        self.kill_confirm = None;
        self.editor_act(ctx, project, Event::Kill);
    }
}

/// The controls of a project open in Unity, in `rect` (40 points high in the hero card, 32 in the compact bar): Show,
/// Save & Exit and Kill; Kill's confirmation; Unity closing or being ended. Nothing drawn (false) when it is not open.
pub(super) fn controls(ui: &mut Ui, state: &mut State, project: &Project, rect: Rect, salt: &str) -> bool {
    let ctx = ui.ctx().clone();
    let session = state.session(&project.path);
    if !session.open() {
        return false;
    }
    let small = rect.height() < 36.0;
    let size = if small { Size::Sm } else { Size::Md };
    let gap = 8.0;
    let confirming = state.kill_confirm.as_ref().is_some_and(|p| p == &project.path);
    let split = |weights: &[f32]| -> Vec<Rect> {
        let total: f32 = weights.iter().sum();
        let free = rect.width() - gap * (weights.len() - 1) as f32;
        let mut x = rect.left();
        weights
            .iter()
            .map(|w| {
                let r = Rect::from_min_size(pos2(x, rect.top()), vec2(free * w / total, rect.height()));
                x = r.right() + gap;
                r
            })
            .collect()
    };
    match session {
        Session::Killing { .. } => {
            let p = ui.painter();
            widgets::frame(p, rect, rect.height() * 0.5, theme::alpha(theme::DANGER, 0.12));
            feedback::spinner(p, pos2(rect.left() + 20.0, rect.center().y), 7.0, 2.0, theme::DANGER);
            widgets::text(p, pos2(rect.left() + 36.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new("Ending Unity…", 12.5).color(theme::FG_2));
        }
        _ if confirming => {
            // The longer words when they fit.
            let long = Button::new("Kill, losing unsaved changes").icon(icons::X_CIRCLE).size(size).desired_width(ui);
            let cancel_w = Button::new("Cancel").size(size).desired_width(ui);
            let words = if long + gap + cancel_w <= rect.width() { "Kill, losing unsaved changes" } else { "Kill, losing changes" };
            let r = split(&[1.7, 1.0]);
            let kill = Button::new(words)
                .icon(icons::X_CIRCLE)
                .variant(Variant::Danger)
                .size(size)
                .width(r[0].width())
                .id_salt((salt, "kill-confirm"));
            if widgets::activated(&kill.show_at(ui, r[0]).on_hover_text("Ends this project's Unity at once: what was not saved is lost. Other Unity windows stay.")) {
                state.kill_editor(&ctx, project);
            }
            if widgets::activated(&Button::new("Cancel").variant(Variant::Secondary).size(size).width(r[1].width()).id_salt((salt, "kill-cancel")).show_at(ui, r[1])) {
                state.kill_confirm = None;
            }
        }
        _ => {
            let (label, hint, enabled) = match session {
                Session::Closing { asking: true, .. } => ("Answer in Unity", "Unity asks whether to save: answer it there (Show brings it to the front)", true),
                Session::Closing { .. } => ("Closing…", "Unity is saving and quitting", false),
                _ => ("Save & Exit", "Asks Unity to quit as its close button does: it offers to save modified scenes first", true),
            };
            // Each button as wide as its words, what is left shared by Show and Save & Exit; Kill becomes its icon
            // alone when the words do not fit.
            let ws = Button::new("Show").icon(icons::ARROW_SQUARE_OUT).size(size).desired_width(ui);
            let wv = Button::new(label).icon(icons::FLOPPY_DISK).size(size).desired_width(ui);
            let wk = Button::new("Kill").icon(icons::X_CIRCLE).size(size).desired_width(ui);
            let icon_kill = ws + wv + wk + 2.0 * gap > rect.width();
            let kw = if icon_kill { rect.height() } else { wk };
            let extra = (rect.width() - ws - wv - kw - 2.0 * gap).max(0.0) * 0.5;
            let r0 = Rect::from_min_size(rect.min, vec2(ws + extra, rect.height()));
            let r1 = Rect::from_min_size(pos2(r0.right() + gap, rect.top()), vec2((rect.right() - kw - gap - r0.right() - gap).max(wv.min(rect.width() * 0.4)), rect.height()));
            let r2 = Rect::from_min_max(pos2(rect.right() - kw, rect.top()), rect.max);
            let r = [r0, r1, r2];
            state.anchors.insert(format!("{salt}-show"), r[0]);
            state.anchors.insert(format!("{salt}-kill"), r[2]);
            let show = Button::new("Show").icon(icons::ARROW_SQUARE_OUT).variant(Variant::Primary).size(size).width(r[0].width()).id_salt((salt, "show"));
            if widgets::activated(&show.show_at(ui, r[0]).on_hover_text("Brings this project's Unity window to the front")) {
                state.show_editor(&ctx, project);
            }
            let save = Button::new(label).icon(icons::FLOPPY_DISK).variant(if label == "Answer in Unity" { Variant::Warn } else { Variant::Secondary }).size(size).width(r[1].width()).enabled(enabled).id_salt((salt, "save"));
            if widgets::activated(&save.show_at(ui, r[1]).on_hover_text(hint)) {
                if matches!(session, Session::Closing { .. }) {
                    state.show_editor(&ctx, project);
                } else {
                    state.save_and_exit(&ctx, project);
                }
            }
            let kill_hint = "Ends this project's Unity without saving (asks first)";
            let pressed = if icon_kill {
                let b = widgets::IconButton::new(icons::X_CIRCLE, kill_hint).size(rect.height(), if small { 15.0 } else { 17.0 }).danger().backdrop(theme::control()).id_salt((salt, "kill"));
                widgets::activated(&b.show_at(ui, r[2]))
            } else {
                let b = Button::new("Kill").icon(icons::X_CIRCLE).variant(Variant::Ghost).size(size).width(r[2].width()).id_salt((salt, "kill"));
                widgets::activated(&b.show_at(ui, r[2]).on_hover_text(kill_hint))
            };
            if pressed {
                state.kill_confirm = Some(project.path.clone());
            }
        }
    }
    true
}
