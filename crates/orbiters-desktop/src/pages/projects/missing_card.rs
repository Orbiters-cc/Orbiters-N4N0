//! The error card of a project needing packages none of the person's repositories list ("Following dependencies are
//! not found: …"): **Add their repositories** looks them up (`orbiters_projects::missing`: Orbiters' Known VPM
//! catalog, the person's repositories turned off, Orbiters listings), shows which repository will be added for which
//! package (and the packages nobody provides, or not at the version needed) and, confirmed, adds them and runs the
//! change that failed again; the card goes once it succeeds.
use orbiters_i18n::{mark, tr};
use super::state::{Modal, Outcome, State};
use crate::pages::Job;
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Shape, Ui, UiBuilder};
use orbiters_projects::{missing, vpm};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, icons};
use std::path::PathBuf;

pub(crate) struct Card {
    pub path: PathBuf,
    /// The change that failed, run again once the repositories are added.
    pub change: vpm::Change,
    pub packages: Vec<missing::Missing>,
    pub lookup: Option<Result<missing::Lookup, String>>,
    pub job: Job<Result<missing::Lookup, String>>,
    /// Screenshots: looking, without a lookup running.
    pub fixture_looking: bool,
}
impl Card {
    /// The card for `error` when it is the resolver's missing packages, for the change that failed.
    pub fn from_error(error: &str, change: Option<(PathBuf, vpm::Change)>) -> Option<Card> {
        let packages = missing::parse(error);
        let (path, change) = change?;
        (!packages.is_empty()).then(|| Card { path, change, packages, lookup: None, job: Job::default(), fixture_looking: false })
    }
    pub fn looking(&self) -> bool {
        self.job.running()
    }
}

fn label(m: &missing::Missing) -> String {
    match m.exact() {
        Some(v) => format!("{} {v}", m.name),
        None => format!("{} {}", m.name, m.range),
    }
}
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest).to_string()
}

/// A paragraph wrapped to the width left.
fn para(ui: &mut Ui, txt: Txt) {
    let w = ui.available_width();
    let galley = ui.painter().layout_job(txt.job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(r.min, galley, theme::FG);
}

/// Draws the card in place of the error banner; returns whether it did (the banner is then left out).
pub(super) fn show(ui: &mut Ui, state: &mut State, salt: &str) -> bool {
    let ctx = ui.ctx().clone();
    let Some(card) = state.missing.as_mut() else { return false };
    if let Some(r) = card.job.take() {
        card.lookup = Some(r);
    }
    let (packages, lookup, looking) = (card.packages.clone(), card.lookup.clone(), card.looking() || card.fixture_looking);
    let retrying = state.adding_repositories && state.running();
    let w = ui.available_width();
    let top_left = ui.cursor().min;
    let bg = ui.painter().add(Shape::Noop);
    let inner = Rect::from_min_size(top_left + vec2(44.0, 12.0), vec2(w - 44.0 - 44.0, 100_000.0));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("missing-card", salt)));
    cu.spacing_mut().item_spacing = vec2(8.0, 6.0);
    let n = packages.len();
    para(&mut cu, Txt::new(if n == 1 { tr("A package this project needs is in none of your repositories") } else { tr("Packages this project needs are in none of your repositories") }, 13.0).weight(weight::SEMIBOLD).line_height(19.0));
    let names: Vec<String> = packages.iter().map(label).collect();
    para(&mut cu, Txt::new(names.join(" · "), 12.0).color(theme::FG_2).line_height(17.0));
    let mut act: Option<Act> = None;
    let busy = state.acting().is_some() && !retrying;
    if retrying {
        status(&mut cu, state.acting().unwrap_or(tr("Adding the repositories…")));
    } else if looking {
        status(&mut cu, tr("Looking for their repositories…"));
    } else {
        match &lookup {
            None => {
                cu.add_space(2.0);
                let b = Button::new(tr("Add their repositories")).icon(icons::PLUS).variant(Variant::Primary).size(Size::Sm).enabled(!busy).id_salt(("missing-find", salt));
                let resp = b.show(&mut cu).on_hover_text(tr("Finds which repositories have these packages (Orbiters' Known VPM first) and shows them before adding"));
                state.anchors.insert("missing-find".into(), resp.rect);
                if widgets::activated(&resp) {
                    act = Some(Act::Find);
                }
            }
            Some(Err(e)) => {
                para(&mut cu, Txt::new(tr!("Their repositories could not be looked up: {error}", error = e), 12.0).color(theme::FG_2).line_height(17.0));
                if widgets::activated(&Button::new(tr("Try again")).variant(Variant::Secondary).size(Size::Sm).id_salt(("missing-again", salt)).show(&mut cu)) {
                    act = Some(Act::Find);
                }
            }
            Some(Ok(lookup)) => {
                let additions = lookup.additions();
                cu.add_space(4.0);
                for a in &additions {
                    addition(&mut cu, a);
                }
                for l in &lookup.not_listed {
                    let has = if l.versions.is_empty() { tr("no version").to_string() } else { l.versions.join(", ") };
                    line(&mut cu, icons::WARNING, theme::WARN, &tr!("{package}: {repository} lists {has}, not this version", package = label(&l.package), repository = l.repository, has = has));
                }
                for m in &lookup.not_provided {
                    line(&mut cu, icons::X_CIRCLE, theme::DANGER, &tr!("{package}: no repository Orbiters knows provides it", package = label(m)));
                }
                cu.add_space(4.0);
                cu.horizontal(|ui| {
                    if additions.is_empty() {
                        let resp = Button::new(tr("Package repos")).icon(icons::PLUS).variant(Variant::Secondary).size(Size::Sm).id_salt(("missing-repos", salt)).show(ui);
                        if widgets::activated(&resp) {
                            act = Some(Act::Repos(resp.rect));
                        }
                    } else {
                        let n = additions.len();
                        let text = if n == 1 { tr("Add it and retry").to_string() } else { tr!("Add {n} repositories and retry", n = n) };
                        let resp = Button::new(&text).icon(icons::PLUS).variant(Variant::Primary).size(Size::Sm).enabled(!busy).id_salt(("missing-add", salt)).show(ui);
                        state.anchors.insert("missing-add".into(), resp.rect);
                        if widgets::activated(&resp) {
                            act = Some(Act::Add(additions.clone()));
                        }
                        if widgets::activated(&Button::new(tr("Cancel")).variant(Variant::Ghost).size(Size::Sm).id_salt(("missing-cancel", salt)).show(ui)) {
                            act = Some(Act::Cancel);
                        }
                    }
                });
            }
        }
    }
    let h = cu.min_rect().height() + 12.0 + 14.0;
    let rect = Rect::from_min_size(top_left, vec2(w, h));
    let p = ui.painter();
    p.set(bg, Shape::rect_filled(rect, theme::radius(14.0), theme::alpha(theme::DANGER, 0.1)));
    widgets::icon(p, pos2(rect.left() + 24.0, rect.top() + 21.5), icons::WARNING, 16.0, theme::DANGER);
    let x = Rect::from_center_size(pos2(rect.right() - 22.0, rect.top() + 21.5), vec2(28.0, 28.0));
    let mut xu = ui.new_child(UiBuilder::new().max_rect(x).id_salt(("missing-dismiss", salt)));
    if !retrying && widgets::activated(&IconButton::new(icons::X, mark("Dismiss")).size(28.0, 14.0).id_salt(("missing-x", salt)).show_at(&mut xu, x)) {
        act = Some(Act::Dismiss);
    }
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.add_space(12.0);
    match act {
        Some(Act::Find) => {
            let card = state.missing.as_mut().unwrap();
            let packages = card.packages.clone();
            let server = state.server_now.unwrap_or(orbiters_core::account::Server::Production);
            card.lookup = None;
            card.job = Job::spawn(&ctx, move || {
                orbiters_projects::runtime()
                    .and_then(|rt| rt.block_on(missing::lookup(&orbiters_projects::vcc_dir(), server, &packages)))
                    .map_err(|e| format!("{e:#}"))
            });
        }
        Some(Act::Add(additions)) => add_and_retry(&ctx, state, additions),
        Some(Act::Cancel) => {
            if let Some(c) = state.missing.as_mut() {
                c.lookup = None;
            }
        }
        Some(Act::Repos(r)) => state.open_modal(&ctx, Modal::Repos, r),
        Some(Act::Dismiss) => {
            state.missing = None;
            state.error = None;
        }
        None => {}
    }
    true
}

enum Act {
    Find,
    Add(Vec<missing::Addition>),
    Cancel,
    Repos(Rect),
    Dismiss,
}

fn status(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    feedback::spinner(ui.painter(), pos2(r.left() + 8.0, r.center().y), 7.0, 2.0, theme::ACCENT);
    widgets::text(ui.painter(), pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 12.5).color(theme::FG_2));
}

/// A repository to add and the packages it brings.
fn addition(ui: &mut Ui, a: &missing::Addition) {
    let w = ui.available_width();
    let pkgs: Vec<String> = a.packages.iter().map(|(n, v)| format!("{n} {v}")).collect();
    let body = Txt::new(tr!("for {packages}", packages = pkgs.join(", ")), 11.5).color(theme::FG_3).line_height(16.0);
    let galley = ui.painter().layout_job(body.job(w - 40.0));
    let h = 22.0 + galley.size().y + 10.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, r, 10.0, theme::black(0.22));
    widgets::icon(p, pos2(r.left() + 16.0, r.top() + 16.0), if a.turn_on.is_some() { icons::EYE } else { icons::PLUS }, 13.0, theme::ACCENT);
    let verb = if a.turn_on.is_some() { tr("Turn on") } else { tr("Add") };
    let name = widgets::text_truncated(
        p,
        pos2(r.left() + 30.0, r.top() + 16.0),
        Align2::LEFT_CENTER,
        &Txt::new(format!("{verb} {}", a.name), 12.5).weight(weight::MEDIUM),
        (w - 40.0) * 0.6,
    );
    widgets::text_truncated(p, pos2(name.right() + 8.0, r.top() + 16.0), Align2::LEFT_CENTER, &Txt::new(host(&a.url), 11.0).color(theme::FG_4), r.right() - name.right() - 18.0);
    p.galley(pos2(r.left() + 30.0, r.top() + 27.0), galley, theme::FG_3);
    let _ = ui.interact(r, Id::unique(("missing-addition", &a.url)), Sense::hover()).on_hover_text(&a.url);
}

fn line(ui: &mut Ui, glyph: &str, color: egui::Color32, text: &str) {
    let w = ui.available_width();
    let galley = ui.painter().layout_job(Txt::new(text, 11.5).color(theme::FG_2).line_height(16.0).job(w - 22.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y.max(16.0)), Sense::hover());
    widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.top() + 8.0), glyph, 12.0, color);
    ui.painter().galley(pos2(r.left() + 20.0, r.top()), galley, theme::FG_2);
}

/// Adds the repositories (turning the person's own back on) and runs the change that failed again.
fn add_and_retry(ctx: &egui::Context, state: &mut State, additions: Vec<missing::Addition>) {
    let Some(card) = state.missing.as_ref() else { return };
    let (path, change) = (card.path.clone(), card.change.clone());
    let n = additions.len();
    let label = if n == 1 { tr!("Adding {name}…", name = additions[0].name) } else { tr!("Adding {n} repositories…", n = n) };
    state.last_change = Some((path.clone(), change.clone()));
    state.adding_repositories = true;
    state.task(ctx, &label, move || {
        let root = orbiters_projects::vcc_dir();
        orbiters_projects::runtime()?.block_on(async {
            missing::add(&root, &additions).await?;
            vpm::change(&root, &path, change).await
        })?;
        Ok(Outcome::Changed(if n == 1 { tr("Repository added: the packages are installed").into() } else { tr!("{n} repositories added: the packages are installed", n = n) }))
    });
}
