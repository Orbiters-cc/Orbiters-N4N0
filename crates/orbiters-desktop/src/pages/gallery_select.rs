//! Photos & captures' selection, dragging out and copying. Ctrl-click, Shift-click, the check on a card's corner or a
//! rubber band select cards (a green ring and check, the badges stepping aside); while some are selected a click adds
//! or removes one, Ctrl+A selects every card shown, Ctrl+C copies them and Escape lets go. A bar at the bottom says how
//! many, shows them, and offers Select all and Clear.
//!
//! Dragging a card, or any card of the selection, out of the window hands their files to the app it is dropped on
//! ([`crate::drag_out`], as Explorer would): a VRChat picture is its PNG, a 3D capture or recording the picture of its 3D
//! view (`capture_picture`, its thumbnail before it is reconstructed). A picture only on Orbiters (shared with the
//! member or by them, a community's, an asset's) is fetched first, whole, into `<cache>/dragged/orbiters` ([`Fetches`]):
//! from the press on its card, as it is selected (the first [`PREFETCH`]), or once the drag starts; the lifted cards
//! wait for what is still coming (the bar counts it) and Windows gets the drag once everything is there, if the button
//! is still down. A copy (Ctrl+C, a card's menu) puts the same files on the clipboard, after the same wait. The cards
//! lift (smaller, faded) as the drag starts and settle back once it ended; the bar says what was copied.
//!
//! A picture open in its window (the item window, a community's or an asset's picture window) is dragged out the same
//! way ([`Selection::carry`]): the window's picture is lifted alone, whatever is selected, and hands out the file its
//! card would (the chosen layer's while the window shows one of a multi-layer picture's layers, [`window_payload`]).
//! The gesture ([`WindowDrag`]): a press on the picture that moves [`DRAG_SLOP`] points with the button held is a drag
//! out; a press released before it moved that far is the picture's click (See it whole), so a click never starts a drag
//! and a drag never toggles the view. Presses on the controls over the picture (arrows, layers, Edit, close) stay
//! theirs: they sit above the picture's own response. While a window is open the bar shows only what a drag out of it
//! says.
use crate::controller::shares::{Entry, Local};
use crate::drag_out::{self, Outcome};
use egui::{pos2, vec2, Align2, Id, Order, Pos2, Rect, Response, Sense, Stroke, Ui};
use orbiters_core::account::{Api, Server};
use orbiters_core::shares::Kind;
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::{
    anim::{self, ease, Spring},
    feedback, icons, paint,
    theme::{self, weight, Txt},
    widgets::{self, Button, Size, Variant},
};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How long the cards lift before Windows' drag starts (its loop is modal: the window shows what it drew last).
const LIFT: f64 = 0.09;
/// How long the bar says what a drag copied.
const SAID_FOR: f64 = 2.6;
/// The check on a card's corner.
pub const CHECK: f32 = 24.0;
/// Pictures from Orbiters fetched as they are selected (more wait for a drag or a copy of them).
pub const PREFETCH: usize = 30;
/// How far a press on a window's picture moves before it is a drag out of the window (egui's click distance).
pub const DRAG_SLOP: f32 = 6.0;

#[derive(Default)]
pub struct Selection {
    pub keys: BTreeSet<String>,
    anchor: Option<String>,
    /// Each card drawn this frame and the last (screen rects): the rubber band selects what it crosses.
    rects: Vec<(String, Rect)>,
    last_rects: Vec<(String, Rect)>,
    /// The rubber band: where it started, and what was selected before it (Ctrl adds to it).
    band: Option<(Pos2, BTreeSet<String>)>,
    /// What is being dragged out (drawn lifted) and since when; `started` once Windows has it.
    carried: Vec<String>,
    lifted_at: f64,
    started: bool,
    /// What the drag handed out (the keys whose files went).
    handed: Vec<String>,
    /// A picture carried out of its window that the page shows no card of (a layer, an item opened from Requests).
    loose: Option<Entry>,
    /// The drag came from a window: the bar says how it went over the window.
    from_window: bool,
    /// The lifted drag waits for this many pictures still coming from Orbiters (of so many).
    waiting: Option<(usize, usize)>,
    /// A copy asked for, waiting for its pictures from Orbiters.
    copying: Option<Vec<String>>,
    /// What the last drag copied, and when.
    said: Option<(String, f64)>,
    /// Pictures from Orbiters fetched to hand out, and those already asked for as they were selected.
    fetches: Fetches,
    prefetched: BTreeSet<String>,
    prefetching: usize,
    /// The page made its list again since the last [`Selection::update`]: what left it is no longer selected.
    list_changed: bool,
    /// The development environment (where pictures from Orbiters come from).
    development: bool,
}

/// What a press on a card asked for.
pub enum CardAct {
    None,
    Open,
}

/// How a card shows its part in the selection (the cards of other pages, community and asset pictures, paint it with
/// [`paint_card`] as the gallery's own).
#[derive(Clone, Copy, Debug, Default)]
pub struct Marks {
    pub selected: bool,
    /// Any card is selected: every card shows its check.
    pub selecting: bool,
    /// Lifted by a drag out of the window.
    pub carried: bool,
}

impl Selection {
    pub fn any(&self) -> bool {
        !self.keys.is_empty()
    }
    pub fn has(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
    pub fn carried(&self, key: &str) -> bool {
        self.carried.iter().any(|k| k == key)
    }
    pub fn marks(&self, key: &str) -> Marks {
        Marks { selected: self.has(key), selecting: self.any(), carried: self.carried(key) }
    }
    pub fn clear(&mut self) {
        self.keys.clear();
        self.anchor = None;
    }

    /// Another account (night4, verify2): nothing of the old one stays to be handed out. The selection, a drag lifted
    /// but not yet handed to Windows (and a picture carried out of its window), a copy waiting, and every picture
    /// fetched or on its way: the fetching threads write into the old store, which no one reads again. A drag Windows
    /// already has is Windows' own.
    pub fn forget_account(&mut self) {
        self.clear();
        self.band = None;
        if !self.started {
            self.carried.clear();
        }
        self.loose = None;
        self.waiting = None;
        self.copying = None;
        self.fetches = Fetches::default();
        self.prefetched.clear();
        self.prefetching = 0;
    }

    /// Tests: whether anything of a drag, a copy or a fetch is kept.
    #[cfg(test)]
    pub fn keeps_anything(&self) -> bool {
        self.any() || !self.carried.is_empty() || self.loose.is_some() || self.copying.is_some() || !self.fetches.lock().states.is_empty() || !self.prefetched.is_empty()
    }

    /// Tests: a picture carried out of its window (`e`), its file fetched to `path`.
    #[cfg(test)]
    pub fn stage_carried(&mut self, e: &Entry, path: PathBuf) {
        self.fetches.stage(&e.key, path);
        self.carried = vec![e.key.clone()];
        self.loose = Some(e.clone());
        self.from_window = true;
        self.lifted_at = 0.0;
    }

    /// Adds a card to the selection, or takes it out (a card's menu).
    pub fn toggle(&mut self, key: &str) {
        if !self.keys.remove(key) {
            self.keys.insert(key.to_owned());
        }
        self.anchor = Some(key.to_owned());
    }

    /// The file a picture from Orbiters was fetched to, once it was (for the photo editor's copy of an Orbiters picture).
    pub fn fetched(&self, key: &str) -> Option<PathBuf> {
        match self.fetches.state(key) {
            Some(Fetch::Ready(p)) => Some(p),
            _ => None,
        }
    }

    /// Before the grid draws: last frame's cards become the band's targets.
    pub fn begin(&mut self, development: bool) {
        self.last_rects = std::mem::take(&mut self.rects);
        self.development = development;
    }

    /// A card as drawn this frame.
    pub fn place(&mut self, key: &str, rect: Rect) {
        self.rects.push((key.to_owned(), rect));
    }

    /// A card's response this frame: opens it, changes the selection, or starts dragging it (with the selection when
    /// it is part of it). `order` is every card shown, in order (Shift selects the run between two). A press on a
    /// picture from Orbiters starts fetching it (a drag may follow).
    pub fn card(&mut self, resp: &Response, e: &Entry, check: Rect, order: &[Entry]) -> CardAct {
        let ctx = resp.ctx.clone();
        if resp.is_pointer_button_down_on() && e.local.is_none() && self.fetches.state(&e.key).is_none() {
            self.want(&ctx, e, false);
        }
        if resp.drag_started_by(egui::PointerButton::Primary) && self.carried.is_empty() {
            self.lift(&ctx, e, order);
            return CardAct::None;
        }
        if !resp.clicked() {
            return CardAct::None;
        }
        let (m, on_check) = ctx.input(|i| (i.modifiers, i.pointer.interact_pos().is_some_and(|p| check.expand(4.0).contains(p))));
        if m.shift {
            let from = self.anchor.as_ref().and_then(|a| order.iter().position(|o| &o.key == a)).unwrap_or(0);
            let to = order.iter().position(|o| o.key == e.key).unwrap_or(from);
            if !m.command {
                self.keys.clear();
            }
            self.keys.extend(order[from.min(to)..=from.max(to)].iter().map(|o| o.key.clone()));
            return CardAct::None;
        }
        if m.command || on_check || self.any() {
            self.toggle(&e.key);
            return CardAct::None;
        }
        self.anchor = Some(e.key.clone());
        CardAct::Open
    }

    /// Screenshots: these cards selected.
    pub fn stage(&mut self, keys: Vec<String>) {
        self.keys = keys.into_iter().collect();
    }

    /// Screenshots: these cards lifted as if Windows had their drag, their drag image written to `image`.
    pub fn stage_carry(&mut self, entries: &[Entry], image: Option<&std::path::Path>) {
        self.carried = entries.iter().map(|e| e.key.clone()).collect();
        self.started = true;
        if let Some(path) = image {
            let refs: Vec<&Entry> = entries.iter().collect();
            if let Some(i) = drag_image(&refs, &self.fetches) {
                let _ = image::save_buffer(path, &i.rgba, i.width, i.height, image::ExtendedColorType::Rgba8);
            }
        }
    }

    /// Screenshots: a window's picture lifted by a drag out of it, as if Windows had it, or still `waiting` for it from
    /// Orbiters.
    pub fn stage_window_carry(&mut self, e: &Entry, waiting: bool) {
        self.carried = vec![e.key.clone()];
        self.loose = Some(e.clone());
        self.from_window = true;
        self.started = !waiting;
        self.waiting = waiting.then_some((1, 1));
        self.lifted_at = if waiting { f64::INFINITY } else { 0.0 };
    }

    /// Screenshots: these cards lifted, the drag waiting for `waiting` of them still coming from Orbiters.
    pub fn stage_wait(&mut self, keys: Vec<String>, waiting: usize) {
        self.waiting = Some((waiting, keys.len()));
        self.carried = keys;
        self.started = false;
        self.lifted_at = f64::INFINITY;
    }

    /// Screenshots: the bar says this, as after a drag.
    pub fn stage_said(&mut self, ctx: &egui::Context, text: &str) {
        self.carried.clear();
        self.say(ctx, text);
    }

    /// The bar says this for a moment (what a card's menu did).
    pub fn say(&mut self, ctx: &egui::Context, text: &str) {
        self.said = Some((text.into(), anim::time(ctx)));
    }

    /// Fetches a picture from Orbiters to hand out (again after it failed when `retry`).
    fn want(&self, ctx: &egui::Context, e: &Entry, retry: bool) {
        if let Some(from) = remote(e, self.development) {
            self.fetches.want(ctx, &e.key, from, stem(e), retry);
        }
    }

    /// The drag picks up the card, or the whole selection when the card is in it; what is only on Orbiters starts
    /// coming.
    fn lift(&mut self, ctx: &egui::Context, e: &Entry, order: &[Entry]) {
        // Nothing can be handed to another program by a drag: the press stays a press.
        if !orbiters_core::here::HERE.drag_out {
            return;
        }
        let keys: Vec<String> =
            if self.keys.contains(&e.key) { order.iter().filter(|o| self.keys.contains(&o.key)).map(|o| o.key.clone()).collect() } else { vec![e.key.clone()] };
        let entries = by_keys(order, &keys);
        if !entries.iter().any(|e| e.local.is_some() || remote(e, self.development).is_some()) {
            self.said = Some((tr("These can't be dragged out").into(), anim::time(ctx)));
            return;
        }
        for e in entries.iter().filter(|e| e.local.is_none()) {
            self.want(ctx, e, true);
        }
        self.carried = keys;
        self.lifted_at = anim::time(ctx);
        self.started = false;
        self.waiting = None;
        self.loose = None;
        self.from_window = false;
    }

    /// A drag out of a picture's window: that picture alone, whatever is selected (`e` is what the window hands out,
    /// [`window_payload`]); its file goes as its card's would, once what comes from Orbiters is there.
    pub fn carry(&mut self, ctx: &egui::Context, e: &Entry) {
        if !self.carried.is_empty() {
            return;
        }
        self.from_window = true;
        if !self.can_hand_out(e) {
            self.said = Some((tr("This can't be dragged out").into(), anim::time(ctx)));
            return;
        }
        if e.local.is_none() {
            self.want(ctx, e, true);
        }
        self.carried = vec![e.key.clone()];
        self.loose = Some(e.clone());
        self.lifted_at = anim::time(ctx);
        self.started = false;
        self.waiting = None;
    }

    /// The window's picture is lifted by a drag out of it (drawn smaller and faded until the drop).
    pub fn window_lifted(&self, key: &str) -> bool {
        self.from_window && self.carried(key)
    }

    /// Copies these cards' files to the clipboard (those from Orbiters once fetched): Ctrl+C, a card's Copy.
    pub fn copy(&mut self, ctx: &egui::Context, keys: Vec<String>, order: &[Entry]) {
        for e in order.iter().filter(|e| e.local.is_none() && keys.contains(&e.key)) {
            self.want(ctx, e, true);
        }
        self.copying = Some(keys);
    }

    /// Copies these files to the clipboard now (a multi-layer picture with its layers), saying so on the bar.
    pub fn copy_paths(&mut self, ctx: &egui::Context, files: &[PathBuf]) {
        let said = match drag_out::copy_files(files) {
            Ok(()) => tr("Copied the picture and its layers to the clipboard").to_string(),
            Err(e) => e,
        };
        self.say(ctx, &said);
    }

    /// Whether a card has a file to hand out (on this computer, or fetched from Orbiters).
    pub fn can_hand_out(&self, e: &Entry) -> bool {
        e.local.is_some() || remote(e, self.development).is_some()
    }

    /// What saving a picture from Orbiters runs on a worker ([`download`]), when it can be fetched.
    pub fn download_job(&self, e: &Entry) -> Option<impl FnOnce() -> anyhow::Result<PathBuf> + Send + 'static> {
        let from = remote(e, self.development)?;
        let stem = stem(e);
        Some(move || download(&from, &stem))
    }

    /// The page made its list again (another filter, a search, what came): [`Selection::update`] drops what left it.
    pub fn list_changed(&mut self) {
        self.list_changed = true;
    }

    /// Every frame after the grid: keyboard, the rubber band, the lifted drag's start and end, a copy waiting for its
    /// pictures. The band starts on the grid's empty space (`background`).
    pub fn update(&mut self, ui: &Ui, order: &[Entry], background: &Response, modal: bool) {
        let ctx = ui.ctx().clone();
        let now = anim::time(&ctx);
        let typing = ctx.memory(|m| m.focused().is_some());
        if !modal && !typing {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
                self.keys = order.iter().map(|e| e.key.clone()).collect();
            }
            if self.any() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.clear();
            }
            // Ctrl+C reaches egui as a copy (egui-winit's clipboard shortcut), not as a key.
            if self.any() && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                let keys = order.iter().filter(|e| self.keys.contains(&e.key)).map(|e| e.key.clone()).collect();
                self.copy(&ctx, keys, order);
            }
        }
        // Gone from the page (another filter, a search): no longer selected. Looked at when the list was made again
        // (a key per card, looked up in a set: Select all on a big library compared every key with every card).
        if std::mem::take(&mut self.list_changed) && self.any() {
            let shown: HashSet<&str> = order.iter().map(|e| e.key.as_str()).collect();
            self.keys.retain(|k| shown.contains(k.as_str()));
        }
        // What is selected from Orbiters starts coming (the first few: a drag or a copy asks for the rest).
        if self.keys.iter().any(|k| !self.prefetched.contains(k)) {
            let new: Vec<&Entry> = order.iter().filter(|e| self.keys.contains(&e.key) && !self.prefetched.contains(&e.key)).collect();
            for e in new {
                if e.local.is_none() && self.prefetching < PREFETCH {
                    self.want(&ctx, e, false);
                    self.prefetching += 1;
                }
                self.prefetched.insert(e.key.clone());
            }
        }
        if !self.any() {
            self.prefetched.clear();
            self.prefetching = 0;
        }

        // The rubber band, from the grid's empty space.
        if !modal && background.drag_started_by(egui::PointerButton::Primary) {
            if let Some(at) = ctx.input(|i| i.pointer.press_origin()) {
                let base = if ctx.input(|i| i.modifiers.command || i.modifiers.shift) { self.keys.clone() } else { BTreeSet::new() };
                self.band = Some((at, base));
            }
        }
        if let Some((start, base)) = self.band.clone() {
            let down = ctx.input(|i| i.pointer.primary_down());
            let at = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(start);
            let band = Rect::from_two_pos(start, at);
            self.keys = base;
            for (k, r) in &self.last_rects {
                if r.intersects(band) {
                    self.keys.insert(k.clone());
                }
            }
            let p = ui.ctx().layer_painter(egui::LayerId::new(Order::Middle, Id::unique("gallery-band")));
            p.rect_filled(band, theme::radius(6.0), theme::alpha(theme::ACCENT, 0.1));
            p.rect_stroke(band, theme::radius(6.0), Stroke::new(1.0, theme::alpha(theme::ACCENT, 0.7)), egui::StrokeKind::Inside);
            if !down {
                self.band = None;
            }
        }

        // A picture carried out of its window without a card on the page is looked for with the cards.
        let with_loose;
        let order = match self.loose.as_ref().filter(|l| !order.iter().any(|o| o.key == l.key)) {
            Some(l) => {
                with_loose = order.iter().cloned().chain(std::iter::once(l.clone())).collect::<Vec<_>>();
                &with_loose[..]
            }
            None => order,
        };
        // The lifted cards go to Windows once they have lifted on screen and what comes from Orbiters is there, if the
        // button is still down.
        if !self.carried.is_empty() && !self.started && self.lifted_at.is_finite() {
            if !ctx.input(|i| i.pointer.primary_down()) {
                if self.waiting.is_some_and(|(n, _)| n > 0) {
                    self.said = Some((tr("Still getting them from Orbiters: drag them again in a moment").into(), now));
                }
                self.carried.clear();
                self.waiting = None;
            } else if now - self.lifted_at >= LIFT {
                let entries = by_keys(order, &self.carried);
                let gathered = self.gather(&entries);
                if gathered.waiting > 0 {
                    self.waiting = Some((gathered.waiting, entries.len()));
                    ctx.request_repaint_after(std::time::Duration::from_millis(120));
                } else if gathered.files.is_empty() {
                    self.said = Some((nothing_came(&gathered), now));
                    self.carried.clear();
                    self.waiting = None;
                } else {
                    let with_files: Vec<&Entry> = entries.into_iter().filter(|e| gathered.keys.contains(&e.key)).collect();
                    drag_out::start(drag_out::Drag { image: drag_image(&with_files, &self.fetches), files: gathered.files });
                    self.handed = gathered.keys;
                    self.started = true;
                    self.waiting = None;
                }
            } else {
                ctx.request_repaint();
            }
        }
        if let Some(outcome) = drag_out::take_outcome() {
            if outcome == Outcome::Copied {
                self.said = Some((copied(order, &self.handed), now));
            } else if outcome == Outcome::Failed {
                self.said = Some((tr!("{system} could not start the drag", system = crate::words::SYSTEM), now));
            }
            self.carried.clear();
            self.started = false;
        }
        if self.carried.is_empty() {
            self.loose = None;
        }

        // A copy, once what it waits for came.
        if let Some(keys) = self.copying.clone() {
            let entries = by_keys(order, &keys);
            let gathered = self.gather(&entries);
            if gathered.waiting > 0 {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            } else {
                self.copying = None;
                let said = if gathered.files.is_empty() {
                    nothing_came(&gathered)
                } else {
                    match drag_out::copy_files(&gathered.files) {
                        Ok(()) => tr!("Copied {what} to the clipboard", what = copied_what(order, &gathered.keys)),
                        Err(e) => e,
                    }
                };
                self.said = Some((said, now));
            }
        }
    }

    /// The files of these cards that can be handed out now, and how many are still coming or failed.
    fn gather(&self, entries: &[&Entry]) -> Gathered {
        let mut g = Gathered::default();
        for e in entries {
            if e.local.is_some() {
                if let Some(f) = files(&[e]).pop() {
                    g.files.push(f);
                    g.keys.push(e.key.clone());
                }
                continue;
            }
            match self.fetches.state(&e.key) {
                Some(Fetch::Ready(p)) => {
                    g.files.push(p);
                    g.keys.push(e.key.clone());
                }
                Some(Fetch::Waiting) => g.waiting += 1,
                Some(Fetch::Failed(why)) => g.failed = Some(why),
                None if remote(e, self.development).is_some() => g.waiting += 1,
                None => {}
            }
        }
        g
    }

    /// The bar at the bottom: how many are selected, them in a little stack, the hint, Select all and Clear; while a
    /// drag or a copy waits for pictures from Orbiters, how many are coming; after it, what it copied.
    pub fn bar(&mut self, ui: &mut Ui, page: Rect, order: &[Entry], pictures: &mut Option<crate::worlds::Pictures>, hidden: bool) {
        let ctx = ui.ctx().clone();
        let now = anim::time(&ctx);
        let said = self.said.as_ref().filter(|(_, at)| now - at < SAID_FOR).map(|(s, _)| s.clone());
        if let Some((_, at)) = &self.said {
            if now - at < SAID_FOR {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAID_FOR - (now - at)));
            }
        }
        // A drag or a copy waiting for pictures from Orbiters says so, with a spinner.
        let coming = match (self.waiting, &self.copying) {
            (Some((1, 1)), _) => Some(tr("Getting it from Orbiters…").to_string()),
            (Some((n, total)), _) if n > 0 => Some(tr!("Getting {n} of {total} from Orbiters…", n = n, total = total)),
            (_, Some(_)) => Some(tr("Getting them from Orbiters to copy…").to_string()),
            _ => None,
        };
        let said = coming.clone().or(said);
        // Over a window it says only how a drag out of that window goes (the selection waits under the window).
        let lone = hidden;
        let shown = if lone { self.from_window && said.is_some() } else { self.any() || said.is_some() };
        let k = anim::toggle(&ctx, Id::unique("gallery-select-bar"), shown, Spring::BOUNCY);
        if k < 0.01 {
            if !lone {
                self.from_window = false;
            }
            return;
        }
        let any = self.any() && !lone;
        let n = self.keys.len();
        let count = Txt::new(if n == 1 { tr("1 selected").to_string() } else { tr!("{count} selected", count = n) }, 14.0).weight(weight::SEMIBOLD);
        let hint_text = said.clone().unwrap_or_else(|| tr("Drag them into Discord, a folder or any app").into());
        let hint = Txt::new(&hint_text, 12.5).color(if said.is_some() { theme::ACCENT } else { theme::FG_3 });
        let all = Button::new(tr("Select all")).variant(Variant::Ghost).size(Size::Sm).id_salt("gallery-select-all");
        let thumbs = if any { self.keys.len().min(3) } else { 0 };
        let count_w = if any { widgets::text_size(ui, &count).x } else { 0.0 };
        let narrow = page.width() < 760.0;
        let hint_w = if narrow && said.is_none() { 0.0 } else { widgets::text_size(ui, &hint).x };
        let all_w = if any && n < order.len() { all.desired_width(ui) } else { 0.0 };
        let stack_w = if thumbs > 0 { 34.0 + 10.0 * (thumbs - 1) as f32 + 12.0 } else { 0.0 };
        let icon_w = if coming.is_some() || said.is_some() && !any { 26.0 } else { 0.0 };
        let inner = stack_w
            + icon_w
            + count_w
            + if count_w > 0.0 && hint_w > 0.0 { 16.0 } else { 0.0 }
            + hint_w
            + if all_w > 0.0 { 12.0 + all_w } else { 0.0 }
            + if any { 8.0 + 32.0 } else { 0.0 };
        let w = (inner + 28.0).min(page.width() - 48.0);
        let h = 52.0;
        let target = Rect::from_center_size(pos2(page.center().x, page.bottom() - 28.0 - h * 0.5), vec2(w, h));
        // Rises out of the bottom edge, growing from a pill.
        let rise = 1.0 - k.clamp(0.0, 1.2);
        let rect = Rect::from_center_size(target.center() + vec2(0.0, 40.0 * rise), vec2(w * (0.86 + 0.14 * k.min(1.0)), h));
        let id = Id::unique("gallery-select-bar-area");
        let mut clear = false;
        let mut select_all = false;
        orbiters_ui::front::area(&ctx, id, if lone { Order::Tooltip } else { Order::Foreground }, false).fixed_pos(rect.min).fade_in(false).show(&ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.multiply_opacity(k.clamp(0.0, 1.0));
            let p = ui.painter().clone();
            paint::shadow(&p, rect, h * 0.5, vec2(0.0, 14.0), 34.0, -6.0, theme::black(0.5));
            p.rect_filled(rect, theme::radius(h * 0.5), theme::surface_strong());
            p.rect_stroke(rect, theme::radius(h * 0.5), theme::hairline(), egui::StrokeKind::Inside);
            // Takes the clicks that land on it (not the cards under it).
            ui.interact(rect, id.with("body"), Sense::click());
            let mut x = rect.left() + 14.0;
            let cy = rect.center().y;
            if thumbs > 0 {
                let picked: Vec<&Entry> = order.iter().filter(|e| self.keys.contains(&e.key)).take(3).collect();
                for (i, e) in picked.iter().enumerate().rev() {
                    let r = Rect::from_center_size(pos2(x + 17.0 + 10.0 * i as f32, cy), vec2(34.0, 34.0));
                    let tex = e.picture.as_ref().and_then(|pic| pictures.as_mut()?.get(&ctx, pic));
                    p.rect_filled(r.expand(2.0), theme::radius(10.0), theme::surface_strong());
                    super::sessions::thumb_shape(&p, r, egui::CornerRadius::same(9), tex.as_ref(), 1.0);
                }
                x += stack_w;
            }
            if icon_w > 0.0 && coming.is_some() {
                feedback::spinner(&p, pos2(x + 9.0, cy), 6.5, 2.0, theme::ACCENT);
                x += icon_w;
            } else if icon_w > 0.0 {
                widgets::icon(&p, pos2(x + 9.0, cy), icons::CHECK, 16.0, theme::ACCENT);
                x += icon_w;
            }
            if count_w > 0.0 {
                widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &count);
                x += count_w + 16.0;
            }
            if hint_w > 0.0 {
                let room = rect.right() - x - 14.0 - if all_w > 0.0 { 12.0 + all_w } else { 0.0 } - if any { 40.0 } else { 0.0 };
                widgets::text_truncated(&p, pos2(x, cy), Align2::LEFT_CENTER, &hint, room.max(0.0));
            }
            let mut right = rect.right() - 10.0;
            if any {
                let close = Rect::from_center_size(pos2(right - 16.0, cy), vec2(32.0, 32.0));
                let resp = ui.interact(close, id.with("clear"), Sense::click()).on_hover_text(tr("Clear the selection (Esc)"));
                let hover = anim::toggle(&ctx, id.with("clear-hover"), resp.hovered(), Spring::SNAPPY);
                p.circle_filled(close.center(), 16.0, theme::white(0.04 + 0.08 * hover));
                widgets::icon(&p, close.center(), icons::X, 15.0, theme::mix(theme::FG_3, theme::FG, hover));
                if widgets::activated(&resp) {
                    clear = true;
                }
                right = close.left() - 8.0;
            }
            if all_w > 0.0 && crate::pages::sharing::pressed(all.show_at(ui, Rect::from_min_size(pos2(right - all_w, cy - 16.0), vec2(all_w, 32.0)))) {
                select_all = true;
            }
        });
        if clear {
            self.clear();
        }
        if select_all {
            self.keys = order.iter().map(|e| e.key.clone()).collect();
        }
    }
}

/// What a card shows of the selection: the ring around it, how far it shrank, the check on its corner (visible on
/// hover, while selecting, and filled when chosen). `radius` is the card's corners'; a picture that is the whole card
/// (the tile layouts) is tinted to all its corners.
#[allow(clippy::too_many_arguments)]
pub fn paint_card(p: &egui::Painter, ctx: &egui::Context, key: &str, card: Rect, thumb: Rect, radius: f32, selected: bool, show_check: bool, hover: f32) -> f32 {
    let id = Id::unique(("gallery-select", key));
    let s = anim::toggle(ctx, id.with("on"), selected, Spring::SNAPPY);
    let pop = anim::toggle(ctx, id.with("pop"), selected, Spring::BOUNCY);
    let vis = anim::toggle(ctx, id.with("check"), show_check || selected || hover > 0.5, Spring::SOFT);
    if s > 0.01 {
        let ring = card.expand(3.0);
        paint::stroke_squircle(p, ring, theme::squircle(radius + 3.0), Stroke::new(2.5, theme::alpha(theme::ACCENT, s)));
        let whole = thumb.bottom() >= card.bottom() - 0.5;
        let corners = [radius, radius, if whole { radius } else { 0.0 }, if whole { radius } else { 0.0 }];
        orbiters_ui::feedback::fill_corners(p, thumb, corners, theme::alpha(theme::ACCENT, 0.1 * s));
    }
    if vis > 0.01 {
        let c = pos2(thumb.left() + 12.0 + CHECK * 0.5, thumb.top() + 12.0 + CHECK * 0.5);
        let r = CHECK * 0.5 * (0.7 + 0.3 * vis) * (1.0 + 0.12 * (pop - s).abs().min(0.3));
        p.circle_filled(c, r + 1.0, theme::black(0.35 * vis * (1.0 - s)));
        p.circle_stroke(c, r - 1.0, Stroke::new(2.0, theme::white(0.85 * vis * (1.0 - s))));
        if s > 0.01 {
            p.circle_filled(c, r * (0.6 + 0.4 * ease::out_cubic(s)), theme::alpha(theme::ACCENT, s.min(1.0)));
            widgets::icon(p, c, icons::CHECK, 14.0 * pop.clamp(0.0, 1.2), theme::alpha(theme::ON_ACCENT, s.min(1.0)));
        }
    }
    vis
}

/// The check's rect on a card's picture (presses there toggle the card).
pub fn check_rect(thumb: Rect) -> Rect {
    Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(CHECK, CHECK))
}

/// What a picture's window hands out when it is dragged out: the file the window is about. While one of a
/// multi-layer picture's layers is chosen, that layer's file (`shown`, the window's entry); otherwise the card's own
/// entry as the page lists it (`card_key` in `order`), so the window and its card hand out the same full file; an
/// item without a card on the page (opened from Requests) is the window's entry as it is.
pub fn window_payload(order: &[Entry], shown: &Entry, card_key: &str) -> Entry {
    if shown.key != card_key {
        return shown.clone();
    }
    order.iter().find(|e| e.key == card_key).cloned().unwrap_or_else(|| shown.clone())
}

/// The gesture that drags a window's picture out ([`DRAG_SLOP`]): pressed on the picture, then moved that far with the
/// primary button held. Once per press: it re-arms when the button is up.
#[derive(Default)]
pub struct WindowDrag {
    fired: bool,
}

impl WindowDrag {
    /// The press became a drag out (until the button is up).
    pub fn dragging(&self) -> bool {
        self.fired
    }

    /// True on the frame the press on `resp` (sensing drags) became a drag out.
    pub fn update(&mut self, resp: &Response) -> bool {
        let (down, origin, at) = resp.ctx.input(|i| (i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.latest_pos()));
        if !down {
            self.fired = false;
            return false;
        }
        if self.fired || !resp.dragged_by(egui::PointerButton::Primary) {
            return false;
        }
        self.fired = moved_past(origin, at, DRAG_SLOP);
        self.fired
    }
}

/// Whether the pointer went `slop` points or more from where it was pressed.
fn moved_past(origin: Option<Pos2>, at: Option<Pos2>, slop: f32) -> bool {
    matches!((origin, at), (Some(o), Some(a)) if o.distance(a) >= slop)
}

/// What a drag or a copy could hand out of some cards now.
#[derive(Default)]
struct Gathered {
    files: Vec<PathBuf>,
    keys: Vec<String>,
    /// Still coming from Orbiters.
    waiting: usize,
    /// Why the last one that could not be fetched was not.
    failed: Option<String>,
}

/// What the bar says when nothing could be handed out.
fn nothing_came(g: &Gathered) -> String {
    match &g.failed {
        Some(why) => tr!("These could not be fetched from Orbiters: {why}", why = why),
        None => tr("These can't be dragged out").into(),
    }
}

/// The entries of `keys`, in their order, each the first card of `order` with that key (one look-up per key in a map
/// made once, where a search through every card per key cost selected x shown).
fn by_keys<'a>(order: &'a [Entry], keys: &[String]) -> Vec<&'a Entry> {
    let mut first: HashMap<&str, &Entry> = HashMap::with_capacity(order.len());
    for e in order {
        first.entry(e.key.as_str()).or_insert(e);
    }
    keys.iter().filter_map(|k| first.get(k.as_str()).copied()).collect()
}

/// "Copied 3 pictures", "Copied a 3D capture", "Copied 2 pictures and a 3D capture": what was handed out (`handed`).
fn copied(order: &[Entry], handed: &[String]) -> String {
    tr!("Copied {what}", what = copied_what(order, handed))
}

/// "3 pictures", "a 3D capture", "2 pictures and a 3D capture": what was handed out, to say it in a sentence of its own.
fn copied_what(order: &[Entry], handed: &[String]) -> String {
    let entries = by_keys(order, handed);
    let pictures = entries.iter().filter(|e| e.is_picture()).count();
    let captures = entries.len().saturating_sub(pictures);
    let pictures = (pictures > 0).then(|| tr_n("a picture", "{count} pictures", pictures as u64));
    let captures = (captures > 0).then(|| tr_n("a 3D capture", "{count} 3D captures", captures as u64));
    match (pictures, captures) {
        (Some(first), Some(second)) => tr!("{first} and {second}", first = first, second = second),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => String::new(),
    }
}

/// The files to hand out: each picture as it is, each capture as the picture of its 3D view (made once, kept in the
/// cache under the capture folder's name).
fn files(entries: &[&Entry]) -> Vec<PathBuf> {
    let dir = crate::pages::cache_root().join("dragged");
    entries
        .iter()
        .filter_map(|e| match e.local.as_ref()? {
            Local::Picture(p) => p.is_file().then(|| p.clone()),
            Local::Session(dir_of) => {
                let name = dir_of.file_name()?.to_string_lossy().into_owned();
                crate::capture_picture::png_file(dir_of, &dir, &name)
            }
        })
        .collect()
}

/// The drag image: the first three as a fanned stack of rounded pictures with a soft shadow, the count on a green
/// badge when there are several. A picture from Orbiters is the small copy made as it was fetched.
fn drag_image(entries: &[&Entry], fetches: &Fetches) -> Option<drag_out::Image> {
    let pictures: Vec<image::RgbaImage> = entries
        .iter()
        .take(3)
        .filter_map(|e| {
            let local = e
                .picture
                .as_ref()
                .and_then(crate::worlds::decode_local)
                .and_then(|c| image::RgbaImage::from_raw(c.size[0] as u32, c.size[1] as u32, c.as_raw().to_vec()));
            local.or_else(|| fetches.small(&e.key))
        })
        .collect();
    let front = pictures.first()?;
    let n = entries.len();
    let (cw, ch) = card_size(front.width(), front.height());
    let pad = 22u32;
    let fan = 10u32 * (pictures.len() as u32 - 1);
    let (w, h) = (cw + pad * 2 + fan + 14, ch + pad * 2 + fan + 14);
    let mut canvas = Canvas::new(w, h);
    // The cards behind first, each further up and to the right, darker.
    for (i, picture) in pictures.iter().enumerate().rev() {
        let off = 10 * i as u32;
        let (x, y) = (pad + off, pad + 14 + fan - off);
        let (pw, ph) = if i == 0 { (cw, ch) } else { (cw, ch) };
        let scaled = image::imageops::resize(picture, pw, ph, image::imageops::FilterType::Triangle);
        canvas.shadow(x as f32, y as f32 + 6.0, pw as f32, ph as f32, 12.0, 10.0, 0.42);
        let shade = 1.0 - 0.18 * i as f32;
        canvas.picture(&scaled, x as f32, y as f32, 12.0, shade, 0.94);
        canvas.outline(x as f32, y as f32, pw as f32, ph as f32, 12.0, [255, 255, 255], 0.22);
    }
    if n > 1 {
        let (bx, by) = ((pad + cw) as f32 - 2.0, (pad + 14 + fan) as f32 + 2.0);
        canvas.badge(bx, by, &n.to_string());
    }
    // The pointer holds the front card a little below its middle.
    Some(drag_out::Image { width: w, height: h, rgba: canvas.rgba, cursor: [(pad + cw / 2) as i32, (pad + 14 + fan + ch / 2 + 10) as i32] })
}

/// A card of the stack, 168 points wide at most and as tall as its picture allows (90 to 168).
fn card_size(w: u32, h: u32) -> (u32, u32) {
    let aspect = w as f32 / h.max(1) as f32;
    let cw = 168.0f32;
    let ch = (cw / aspect).clamp(90.0, 168.0);
    (cw as u32, ch as u32)
}

/// A straight-alpha RGBA canvas painted with coverage (antialiased rounded corners), over-blended.
struct Canvas {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self { w, h, rgba: vec![0; (w * h * 4) as usize] }
    }

    fn blend(&mut self, x: u32, y: u32, c: [f32; 3], a: f32) {
        if x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let d = &mut self.rgba[i..i + 4];
        let da = d[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        for k in 0..3 {
            let v = (c[k] * a + d[k] as f32 / 255.0 * da * (1.0 - a)) / oa;
            d[k] = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        d[3] = (oa * 255.0).round() as u8;
    }

    /// Coverage of a rounded rectangle at a pixel's centre.
    fn inside(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let qx = (px - cx).abs() - (w * 0.5 - r);
        let qy = (py - cy).abs() - (h * 0.5 - r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
        (0.5 - outside).clamp(0.0, 1.0)
    }

    fn picture(&mut self, img: &image::RgbaImage, x: f32, y: f32, r: f32, shade: f32, alpha: f32) {
        for (ix, iy, p) in img.enumerate_pixels() {
            let (px, py) = (x + ix as f32 + 0.5, y + iy as f32 + 0.5);
            let cov = Self::inside(px, py, x, y, img.width() as f32, img.height() as f32, r);
            let c = [p[0] as f32 / 255.0 * shade, p[1] as f32 / 255.0 * shade, p[2] as f32 / 255.0 * shade];
            self.blend((x as u32) + ix, (y as u32) + iy, c, cov * alpha * p[3] as f32 / 255.0);
        }
    }

    fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: [u8; 3], alpha: f32) {
        let c = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
        for py in (y as u32)..(y + h).ceil() as u32 {
            for px in (x as u32)..(x + w).ceil() as u32 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let ring = Self::inside(fx, fy, x, y, w, h, r) - Self::inside(fx, fy, x + 1.0, y + 1.0, w - 2.0, h - 2.0, r - 1.0);
                self.blend(px, py, c, ring.max(0.0) * alpha);
            }
        }
    }

    /// A soft shadow: the rounded rectangle's coverage, feathered over `blur` pixels.
    fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, blur: f32, alpha: f32) {
        let (x0, y0) = ((x - blur).max(0.0) as u32, (y - blur).max(0.0) as u32);
        let (x1, y1) = (((x + w + blur) as u32).min(self.w), ((y + h + blur) as u32).min(self.h));
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        for py in y0..y1 {
            for px in x0..x1 {
                let qx = (px as f32 + 0.5 - cx).abs() - (w * 0.5 - r);
                let qy = (py as f32 + 0.5 - cy).abs() - (h * 0.5 - r);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                let t = (1.0 - (d + blur * 0.3) / blur).clamp(0.0, 1.0);
                self.blend(px, py, [0.0, 0.0, 0.0], alpha * t * t * (3.0 - 2.0 * t));
            }
        }
    }

    /// The count on a green disc with a dark rim, centred on (`x`, `y`).
    fn badge(&mut self, x: f32, y: f32, text: &str) {
        let digits: Vec<&[&str; 7]> = text.chars().filter_map(|c| c.to_digit(10)).map(|d| &DIGITS[d as usize]).collect();
        let tw = digits.len() as f32 * 6.0 - 1.0;
        let r = (tw * 0.5 + 8.0).max(13.0);
        let (hw, hh) = (r.max(tw * 0.5 + 8.0), 13.0);
        let accent = [0.0, 218.0 / 255.0, 109.0 / 255.0];
        let ink = [5.0 / 255.0, 22.0 / 255.0, 12.0 / 255.0];
        for py in (y - hh - 3.0) as u32..(y + hh + 3.0) as u32 {
            for px in (x - hw - 3.0) as u32..(x + hw + 3.0) as u32 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let rim = Self::inside(fx, fy, x - hw - 2.0, y - hh - 2.0, hw * 2.0 + 4.0, hh * 2.0 + 4.0, hh + 2.0);
                self.blend(px, py, [0.035, 0.035, 0.043], rim);
                let disc = Self::inside(fx, fy, x - hw, y - hh, hw * 2.0, hh * 2.0, hh);
                self.blend(px, py, accent, disc);
            }
        }
        // Digits drawn at twice their 5x7 grid, in the badge's dark ink.
        let (sx, sy) = (x - tw, y - 7.0);
        for (i, rows) in digits.iter().enumerate() {
            for (ry, row) in rows.iter().enumerate() {
                for (rx, cell) in row.chars().enumerate() {
                    if cell != '#' {
                        continue;
                    }
                    let (cx, cy) = (sx + (i as f32 * 6.0 + rx as f32) * 2.0, sy + ry as f32 * 2.0);
                    for py in cy as u32..(cy + 2.0) as u32 {
                        for px in cx as u32..(cx + 2.0) as u32 {
                            self.blend(px, py, ink, 1.0);
                        }
                    }
                }
            }
        }
    }
}

/// 5x7 digits for the count badge.
const DIGITS: [[&str; 7]; 10] = [
    [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    [".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
    ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    [".###.", "#....", "#....", "####.", "#...#", "#...#", ".###."],
    ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    [".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."],
];

// ---------------------------------------------------------------------------------------------------------------------
// Pictures from Orbiters, fetched to hand out.

/// Where a picture that is only on Orbiters comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Remote {
    /// A picture shared with the member or by them: the sharing contract's file, whole.
    Shared { server: Server, id: u64 },
    /// A community gallery's picture or an asset showcase's (`asset`, else 0): its full picture, its link renewed by
    /// Orbiters (Discord's links expire).
    Gallery { server: Server, asset: u64, id: u64, url: String },
    /// A public picture (an asset's, a shared 3D capture's picture).
    Link(String),
}

/// Where a card that is not on this computer is fetched from, if it can be.
pub fn remote(e: &Entry, development: bool) -> Option<Remote> {
    if e.local.is_some() {
        return None;
    }
    let server = if development { Server::Development } else { Server::Production };
    if let Some(item) = &e.remote {
        return match item.kind {
            Kind::Picture => Some(Remote::Shared { server, id: item.id }),
            Kind::Capture => {
                let p = item.picture.as_ref()?;
                [&p.url, &p.thumbnail_url].into_iter().find(|u| u.starts_with("https://")).map(|u| Remote::Link(u.clone()))
            }
        };
    }
    match e.picture.as_ref()? {
        crate::worlds::Picture::Gallery { asset, id, url, .. } if *id != 0 => Some(Remote::Gallery { server, asset: *asset, id: *id, url: url.clone() }),
        crate::worlds::Picture::Gallery { url, .. } | crate::worlds::Picture::Remote { url, .. } if url.starts_with("https://") => Some(Remote::Link(url.clone())),
        _ => None,
    }
}

/// The file a fetched picture is kept as, without its extension (the picture's own decides it):
/// `<cache>/dragged/orbiters/Midnight Rooftop 2026-10-28 21-30-00 picture-12`, what the app it lands in shows.
fn stem(e: &Entry) -> PathBuf {
    crate::pages::cache_root().join("dragged").join("orbiters").join(stem_name(e))
}

/// [`stem`]'s file name. The picture's identity always whole at its end (night4, verify2: a long world name cut it at
/// 120 characters, two pictures shared one file, and the editor was seeded from the wrong one): the world's name is
/// what gets shortened, and a long identity is hashed.
fn stem_name(e: &Entry) -> String {
    let when = crate::controller::shares::iso(e.taken).replace('T', " ").replace(':', "-").trim_end_matches('Z').to_string();
    let key = e.key.replace(':', "-");
    let key: String = if key.chars().count() <= 40 && file_name(&key) == key {
        key
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        e.key.hash(&mut h);
        format!("k{:016x}", h.finish())
    };
    let world: String = file_name(e.world_name.trim()).chars().take(50).collect();
    file_name(&format!("{} {when} {key}", world.trim_end()))
}

/// A name Windows takes for a file: no reserved characters, no trailing dots or spaces, at most 120 characters.
fn file_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).take(120).collect();
    let clean = clean.trim_end_matches(['.', ' ']).trim_start().to_string();
    if clean.is_empty() {
        "Orbiters picture".into() // i18n: not shown
    } else {
        clean
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fetch {
    Waiting,
    Ready(PathBuf),
    Failed(String),
}

/// Pictures from Orbiters fetched by up to [`WORKERS`] threads, oldest asked first; each kept with a small copy for the
/// drag image.
#[derive(Default)]
pub struct Fetches {
    inner: Arc<Mutex<FetchInner>>,
}

#[derive(Default)]
struct FetchInner {
    states: HashMap<String, Fetch>,
    small: HashMap<String, image::RgbaImage>,
    queue: VecDeque<(String, Remote, PathBuf)>,
    workers: usize,
}

const WORKERS: usize = 3;
const EXTENSIONS: [&str; 5] = ["png", "jpg", "webp", "gif", "avif"];

impl Fetches {
    fn lock(&self) -> std::sync::MutexGuard<'_, FetchInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn state(&self, key: &str) -> Option<Fetch> {
        self.lock().states.get(key).cloned()
    }

    fn small(&self, key: &str) -> Option<image::RgbaImage> {
        self.lock().small.get(key).cloned()
    }

    /// Fetches `from` into `stem` (a file kept from before is taken as it is); a failed one again with `retry`.
    fn want(&self, ctx: &egui::Context, key: &str, from: Remote, stem: PathBuf, retry: bool) {
        let mut inner = self.lock();
        match inner.states.get(key) {
            Some(Fetch::Failed(_)) if retry => {}
            Some(_) => return,
            None => {}
        }
        inner.states.insert(key.to_owned(), Fetch::Waiting);
        inner.queue.push_back((key.to_owned(), from, stem));
        if inner.workers < WORKERS {
            inner.workers += 1;
            let (shared, ctx) = (self.inner.clone(), ctx.clone());
            std::thread::spawn(move || work(&shared, &ctx));
        }
    }

    /// Tests: this picture as if fetched to `path`.
    #[cfg(test)]
    fn stage(&self, key: &str, path: PathBuf) {
        self.lock().states.insert(key.to_owned(), Fetch::Ready(path));
    }
}

/// A fetching thread: takes what is asked until nothing is left.
fn work(shared: &Arc<Mutex<FetchInner>>, ctx: &egui::Context) {
    loop {
        let job = {
            let mut inner = shared.lock().unwrap_or_else(|e| e.into_inner());
            match inner.queue.pop_front() {
                Some(job) => job,
                None => {
                    inner.workers -= 1;
                    return;
                }
            }
        };
        let (key, from, stem) = job;
        let (state, small) = match kept(&stem).map(Ok).unwrap_or_else(|| fetch(&from, &stem)) {
            Ok(path) => {
                let small = image::open(&path).ok().map(|i| i.thumbnail(336, 336).to_rgba8());
                (Fetch::Ready(path), small)
            }
            Err(e) => (Fetch::Failed(e.to_string()), None),
        };
        let mut inner = shared.lock().unwrap_or_else(|e| e.into_inner());
        inner.states.insert(key.clone(), state);
        if let Some(s) = small {
            inner.small.insert(key, s);
        }
        drop(inner);
        ctx.request_repaint();
    }
}

/// The picture fetched before (from another drag, another run), if it is there whole.
fn kept(stem: &Path) -> Option<PathBuf> {
    EXTENSIONS.iter().map(|x| with_ext(stem, x)).find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0))
}

/// Fetches a picture into `stem` with the extension its bytes say (written whole, then renamed).
fn fetch(from: &Remote, stem: &Path) -> anyhow::Result<PathBuf> {
    // Tests never ask Orbiters (night4).
    if cfg!(test) {
        anyhow::bail!("tests never download"); // i18n: not shown (tests only)
    }
    let dir = stem.parent().ok_or_else(|| anyhow::anyhow!("{}", tr("no folder")))?;
    std::fs::create_dir_all(dir)?;
    let part = with_ext(stem, "part");
    let bytes = match from {
        Remote::Shared { server, id } => {
            let (api, token) = crate::member::reading(*server).ok_or_else(|| anyhow::anyhow!("{}", tr("Sign in to Orbiters in Settings first.")))?.api();
            let result = api.capture_file(&token, Kind::Picture, *id, &part);
            let bytes = result.and_then(|_| Ok(std::fs::read(&part)?));
            let _ = std::fs::remove_file(&part);
            bytes?
        }
        Remote::Gallery { server, asset, id, url } => {
            let (api, token) = crate::member::reading(*server).ok_or_else(|| anyhow::anyhow!("{}", tr("Sign in to Orbiters in Settings first.")))?.api();
            let renewed = api.picture_source(&token, *asset, *id, false).ok().unwrap_or_default();
            let links: Vec<String> = [renewed.full_url, renewed.src, Some(url.clone())].into_iter().flatten().filter(|u| u.starts_with("https://")).collect();
            let mut last = anyhow::anyhow!("{}", tr("Orbiters has no link for this picture"));
            let mut got = None;
            for link in links {
                match api.image(&link) {
                    Ok(b) => {
                        got = Some(b);
                        break;
                    }
                    Err(e) => last = e,
                }
            }
            got.ok_or(last)?
        }
        Remote::Link(url) => Api::new(Server::Production).image(url)?,
    };
    let format = image::guess_format(&bytes).map_err(|_| anyhow::anyhow!("{}", tr("Orbiters did not send a picture")))?;
    let ext = format.extensions_str().first().copied().unwrap_or("png");
    let ext = if ext == "jpeg" { "jpg" } else { ext };
    let out = with_ext(stem, ext);
    orbiters_core::write_atomic(&out, &bytes)?;
    Ok(out)
}

/// Saves a picture from Orbiters (the one fetched before, else fetched now) in `Documents/Orbiters/Downloaded
/// pictures`, under its own name (another number when that one is taken). The saved file.
pub fn download(from: &Remote, stem: &Path) -> anyhow::Result<PathBuf> {
    let got = match kept(stem) {
        Some(p) => p,
        None => fetch(from, stem)?,
    };
    let dir = orbiters_core::paths::documents_root().join("Downloaded pictures");
    std::fs::create_dir_all(&dir)?;
    let name = got.file_name().ok_or_else(|| anyhow::anyhow!("{}", tr("no name")))?.to_string_lossy().into_owned();
    let (base, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), "png"));
    let out = (1..1000)
        .map(|n| dir.join(if n == 1 { format!("{base}.{ext}") } else { format!("{base} ({n}).{ext}") }))
        .find(|p| !p.exists())
        .ok_or_else(|| anyhow::anyhow!("{}", tr("too many copies already")))?;
    std::fs::copy(&got, &out)?;
    Ok(out)
}

/// `stem` with `.ext` after it (a world's name may hold dots of its own).
fn with_ext(stem: &Path, ext: &str) -> PathBuf {
    let mut s = stem.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    /// Night4, verify2: two pictures of a world with a very long name never share a fetched file.
    #[test]
    fn a_long_world_name_never_cuts_the_pictures_identity() {
        let long = "A".repeat(200);
        let mut a = crate::controller::shares::remote_entry(orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Picture, id: 1, ..Default::default() });
        let mut b = crate::controller::shares::remote_entry(orbiters_core::shares::Item { kind: orbiters_core::shares::Kind::Picture, id: 2, ..Default::default() });
        a.world_name = long.clone();
        b.world_name = long;
        assert_ne!(stem_name(&a), stem_name(&b));
        assert!(stem_name(&a).chars().count() <= 120);
    }

    use super::*;

    fn entry(key: &str, picture: bool) -> Entry {
        let mut e = crate::controller::shares::remote_entry(orbiters_core::shares::Item::default());
        e.key = key.into();
        e.local = Some(if picture { Local::Picture(PathBuf::from(format!("{key}.png"))) } else { Local::Session(PathBuf::from(key)) });
        e
    }

    /// Night PAGES-UI-2: Select all on a big library, then frames: each frame compared every selected key with every
    /// card (20,000 x 20,000 / 2 string comparisons a frame). Now what left the list is looked for when it changed,
    /// in a set, and a lift finds its cards in a map.
    #[test]
    fn select_all_on_a_big_library_keeps_the_frames_cheap() {
        let order: Vec<Entry> = (0..20_000).map(|i| entry(&format!("C:/Users/someone/Pictures/VRChat/2026-11/picture-{i:06}"), true)).collect();
        let ctx = egui::Context::default();
        let mut sel = Selection::default();
        let started = std::time::Instant::now();
        let frame = |sel: &mut Selection, order: &[Entry], events: Vec<egui::Event>| {
            let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                let background = ui.interact(ui.max_rect(), Id::unique("band"), Sense::drag());
                sel.update(ui, order, &background, false);
            });
            out.textures_delta.clear();
        };
        sel.list_changed();
        let ctrl_a = egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        frame(&mut sel, &order, vec![ctrl_a]);
        assert_eq!(sel.keys.len(), 20_000, "all selected");
        for _ in 0..10 {
            frame(&mut sel, &order, Vec::new());
        }
        let first = order[0].clone();
        sel.lift(&ctx, &first, &order);
        assert_eq!(sel.carried.len(), 20_000, "the whole selection is lifted");
        let spent = started.elapsed();
        assert!(spent < std::time::Duration::from_secs(5), "eleven frames and a lift took {spent:?}");
        // A list made again without some cards: those are no longer selected, the others stay.
        let fewer = &order[..15_000];
        sel.list_changed();
        frame(&mut sel, fewer, Vec::new());
        assert_eq!(sel.keys.len(), 15_000);
        assert!(sel.has(&order[0].key) && !sel.has(&order[19_999].key));
    }

    #[test]
    fn what_a_drag_copied_reads_naturally() {
        let order = vec![entry("a", true), entry("b", true), entry("c", false)];
        let keys = |ks: &[&str]| ks.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(copied(&order, &keys(&["a"])), "Copied a picture");
        assert_eq!(copied(&order, &keys(&["a", "b"])), "Copied 2 pictures");
        assert_eq!(copied(&order, &keys(&["c"])), "Copied a 3D capture");
        assert_eq!(copied(&order, &keys(&["a", "b", "c"])), "Copied 2 pictures and a 3D capture");
        // A picture from Orbiters counts as a picture.
        let mut shared = entry("picture:4", true);
        shared.local = None;
        shared.remote = Some(orbiters_core::shares::Item { kind: Kind::Picture, id: 4, ..Default::default() });
        assert_eq!(copied(&[shared], &keys(&["picture:4"])), "Copied a picture");
    }

    /// What was copied is said in whole sentences of the language, its pictures and captures counted in its plural.
    #[test]
    fn what_a_drag_copied_follows_the_language() {
        use orbiters_i18n::{in_language, Language};
        let order = vec![entry("a", true), entry("b", true), entry("c", false)];
        let keys = |ks: &[&str]| ks.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        in_language(Language::Fr, || {
            assert_eq!(copied(&order, &keys(&["a", "b"])), "Copié\u{a0}: 2 photos");
            assert_eq!(copied(&order, &keys(&["a", "b", "c"])), "Copié\u{a0}: 2 photos et une capture 3D");
        });
        in_language(Language::Ja, || assert_eq!(copied(&order, &keys(&["a", "b", "c"])), "写真2枚と3Dキャプチャ1件をコピーしました"));
        in_language(Language::Es, || assert_eq!(copied(&order, &keys(&["c"])), "Copiado: una captura 3D"));
    }

    /// A card that is only on Orbiters: a community's picture, an asset's, one shared with the member, a shared 3D
    /// capture.
    fn remote_entry(key: &str, picture: Option<crate::worlds::Picture>, item: Option<orbiters_core::shares::Item>) -> Entry {
        let mut e = entry(key, true);
        e.local = None;
        e.picture = picture;
        e.remote = item;
        e.world_name = "Midnight Rooftop".into();
        e.taken = crate::controller::shares::unix("2026-10-28T21:30:00Z");
        e
    }

    #[test]
    fn pictures_from_orbiters_are_fetched_from_where_they_are() {
        use crate::worlds::Picture;
        let gallery = remote_entry("community:345", Some(Picture::Gallery { development: false, asset: 0, id: 345, url: String::new(), large: false }), None);
        assert_eq!(remote(&gallery, false), Some(Remote::Gallery { server: Server::Production, asset: 0, id: 345, url: String::new() }));
        let asset = Picture::Gallery { development: false, asset: 0, id: 0, url: "https://api.orbiters.cc/creator-tools/media/9".into(), large: false };
        let asset = remote_entry("asset:7:0", Some(asset), None);
        assert_eq!(remote(&asset, true), Some(Remote::Link("https://api.orbiters.cc/creator-tools/media/9".into())));
        let item = orbiters_core::shares::Item { kind: Kind::Picture, id: 12, ..Default::default() };
        assert_eq!(remote(&remote_entry("picture:12", None, Some(item)), true), Some(Remote::Shared { server: Server::Development, id: 12 }));
        let capture = orbiters_core::shares::Item {
            kind: Kind::Capture,
            id: 3,
            picture: Some(orbiters_core::shares::Picture { url: String::new(), thumbnail_url: "https://cdn.orbiters.cc/t.webp".into(), width: 0, height: 0 }),
            ..Default::default()
        };
        assert_eq!(remote(&remote_entry("capture:3", None, Some(capture)), false), Some(Remote::Link("https://cdn.orbiters.cc/t.webp".into())));
        let nothing = remote_entry("community:0", Some(Picture::Gallery { development: false, asset: 0, id: 0, url: String::new(), large: false }), None);
        assert_eq!(remote(&nothing, false), None);
        assert_eq!(remote(&entry("a", true), false), None, "what is on this computer is handed out as it is");
        // Kept under a name the app it lands in shows.
        let name = stem(&gallery).file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, "Midnight Rooftop 2026-10-28 21-30-00 community-345");
        assert_eq!(file_name("A/B: \"C\"? . "), "A_B_ _C__");
        assert_eq!(with_ext(Path::new("dir/Mr. Smith picture-1"), "webp"), PathBuf::from("dir/Mr. Smith picture-1.webp"));
    }

    #[test]
    fn remote_cards_are_selected_and_dragged_with_the_local_ones() {
        use crate::worlds::Picture;
        let dir = std::env::temp_dir().join(format!("orbiters-select-remote-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("VRChat_2026-10-28_21-30-00.000_1920x1080.png");
        let fetched = dir.join("Midnight Rooftop 2026-10-28 21-30-00 community-345.webp");
        std::fs::write(&local, b"png").unwrap();
        std::fs::write(&fetched, b"webp").unwrap();
        let mut here = entry("here", true);
        here.local = Some(Local::Picture(local.clone()));
        let gallery = remote_entry("community:345", Some(Picture::Gallery { development: false, asset: 0, id: 345, url: String::new(), large: false }), None);
        let asset = Picture::Gallery { development: false, asset: 0, id: 0, url: "https://api.orbiters.cc/creator-tools/media/9".into(), large: false };
        let asset = remote_entry("asset:7:0", Some(asset), None);
        let order = vec![here.clone(), gallery.clone(), asset.clone()];
        let ctx = egui::Context::default();
        let mut sel = Selection::default();
        // The community's picture is already fetched; the asset's is still coming.
        sel.fetches.stage(&gallery.key, fetched.clone());
        sel.fetches.lock().states.insert(asset.key.clone(), Fetch::Waiting);
        sel.stage(vec![here.key.clone(), gallery.key.clone(), asset.key.clone()]);
        sel.lift(&ctx, &gallery, &order);
        assert_eq!(sel.carried, ["here", "community:345", "asset:7:0"], "the whole selection, in the page's order");
        let entries: Vec<&Entry> = order.iter().collect();
        let g = sel.gather(&entries);
        assert_eq!((g.files.clone(), g.waiting), (vec![local.clone(), fetched.clone()], 1), "the drag waits for the asset's picture");
        // It came: everything goes.
        sel.fetches.stage(&asset.key, fetched.clone());
        let g = sel.gather(&entries);
        assert_eq!((g.files.len(), g.waiting, g.keys.len()), (3, 0, 3));
        // One that failed is left out, and the bar says why when nothing else is there.
        sel.fetches.lock().states.insert(asset.key.clone(), Fetch::Failed("Orbiters answered 404.".into()));
        let g = sel.gather(&[&asset]);
        assert!(g.files.is_empty());
        assert_eq!(nothing_came(&g), "These could not be fetched from Orbiters: Orbiters answered 404.");
        // A card alone, outside the selection, goes alone.
        sel.carried.clear();
        sel.lift(&ctx, &remote_entry("community:9", None, None), &order);
        assert!(sel.carried.is_empty(), "nothing to hand out of a card without a picture");
        assert_eq!(sel.fetched(&gallery.key), Some(fetched));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stack_cards_keep_their_shape_within_bounds() {
        assert_eq!(card_size(1920, 1080), (168, 94));
        assert_eq!(card_size(1080, 1920), (168, 168));
        assert_eq!(card_size(4000, 400), (168, 90));
    }

    #[test]
    fn the_badge_and_pictures_paint_inside_the_canvas() {
        let mut c = Canvas::new(80, 60);
        c.badge(40.0, 30.0, "12");
        let img = image::RgbaImage::from_pixel(30, 20, image::Rgba([200, 100, 50, 255]));
        c.picture(&img, 5.0, 5.0, 6.0, 1.0, 1.0);
        c.shadow(5.0, 5.0, 30.0, 20.0, 6.0, 8.0, 0.4);
        // The badge's green middle and its dark digits are there; the corner of the rounded picture stays clear.
        assert!(c.rgba.chunks_exact(4).any(|p| p[1] > 200 && p[0] < 20));
        assert!(c.rgba.chunks_exact(4).any(|p| p[3] == 255 && p[1] < 40 && p[0] < 20));
        assert_eq!(c.rgba[((5 * 80 + 5) * 4 + 3) as usize] < 160, true);
    }

    #[test]
    fn a_window_hands_out_its_card_s_file_or_its_chosen_layer() {
        let mut card = entry("VRChat_1", true);
        card.world_name = "Midnight Rooftop".into();
        let order = vec![entry("other", true), card.clone()];
        // The window's own copy of its card (a remote capture's window sets its picture): the card's entry goes.
        let mut shown = card.clone();
        shown.picture = Some(crate::worlds::Picture::Capture { development: false, id: 3, large: false });
        let out = window_payload(&order, &shown, &card.key);
        assert_eq!((out.key.as_str(), out.picture.is_none()), ("VRChat_1", true));
        // A layer chosen: that layer's file, a key the page has no card of.
        let mut layer = entry("VRChat_1#Player", true);
        layer.local = Some(Local::Picture(PathBuf::from("VRChat_1_Player.png")));
        assert_eq!(window_payload(&order, &layer, &card.key).key, "VRChat_1#Player");
        // Opened from Requests: no card, the window's entry as it is.
        let alone = remote_entry("picture:4", None, Some(orbiters_core::shares::Item { kind: Kind::Picture, id: 4, ..Default::default() }));
        assert_eq!(window_payload(&order, &alone, "picture:4").key, "picture:4");
    }

    #[test]
    fn a_window_s_picture_is_carried_alone_whatever_is_selected() {
        let ctx = egui::Context::default();
        let mut sel = Selection::default();
        let order = vec![entry("a", true), entry("b", true)];
        sel.stage(vec!["a".into(), "b".into()]);
        let mut layer = entry("a#UI", true);
        layer.local = Some(Local::Picture(PathBuf::from("a_UI.png")));
        sel.carry(&ctx, &layer);
        assert_eq!(sel.carried, ["a#UI"], "the window's picture alone, not the selection");
        assert!(sel.window_lifted("a#UI") && !sel.window_lifted("a"));
        assert!(sel.any(), "the selection is kept for after the window");
        // Its file is found though the page has no card of it, and only one drag at a time.
        assert_eq!(files(&[sel.loose.as_ref().unwrap()]), Vec::<PathBuf>::new(), "a missing file is not handed out");
        sel.carry(&ctx, &order[1]);
        assert_eq!(sel.carried, ["a#UI"]);
        // Nothing to hand out: the bar says so over the window, nothing is lifted.
        let mut sel = Selection::default();
        sel.carry(&ctx, &remote_entry("community:9", None, None));
        assert!(sel.carried.is_empty() && sel.from_window);
        assert_eq!(sel.said.as_ref().map(|s| s.0.as_str()), Some("This can't be dragged out"));
        // A card's own drag is the grid's again.
        sel.lift(&ctx, &order[0], &order);
        assert!(!sel.from_window && sel.loose.is_none());
    }

    #[test]
    fn a_press_on_a_window_s_picture_drags_only_once_it_moved() {
        assert!(!moved_past(Some(pos2(10.0, 10.0)), Some(pos2(14.0, 13.0)), DRAG_SLOP), "5 points: still a click");
        assert!(moved_past(Some(pos2(10.0, 10.0)), Some(pos2(16.0, 10.0)), DRAG_SLOP));
        assert!(!moved_past(None, Some(pos2(0.0, 0.0)), DRAG_SLOP));
        // In egui: a press, a small move (a click), then a move past the slop with the button held (one drag out).
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 120.0));
        let mut gesture = WindowDrag::default();
        let mut fired = Vec::new();
        let mut clicked = Vec::new();
        let at = pos2(150.0, 150.0);
        let steps: Vec<Vec<egui::Event>> = vec![
            vec![egui::Event::PointerMoved(at)],
            vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
            vec![egui::Event::PointerMoved(at + vec2(3.0, 0.0))],
            vec![egui::Event::PointerButton { pos: at + vec2(3.0, 0.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }],
            vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
            vec![egui::Event::PointerMoved(at + vec2(4.0, 0.0))],
            vec![egui::Event::PointerMoved(at + vec2(20.0, 0.0))],
            vec![egui::Event::PointerMoved(at + vec2(40.0, 0.0))],
            vec![egui::Event::PointerButton { pos: at + vec2(40.0, 0.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }],
        ];
        for (i, events) in steps.into_iter().enumerate() {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, time: Some(i as f64 * 0.05), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                let resp = ui.interact(rect, Id::unique("window-picture"), Sense::click_and_drag());
                if gesture.update(&resp) {
                    fired.push(i);
                }
                if resp.clicked() {
                    clicked.push(i);
                }
            });
            output.textures_delta.clear();
        }
        assert_eq!(clicked, [3], "the small move stays the picture's click");
        assert_eq!(fired, [6], "the drag out starts once, as the press moved past the slop");
    }
}
