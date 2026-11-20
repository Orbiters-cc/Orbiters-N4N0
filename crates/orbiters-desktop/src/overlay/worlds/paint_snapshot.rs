//! Immutable draw and interaction data for the CPU page worker.
use super::*;

/// What a picture of the page is kept and matched under: the page, and the account generation it was painted for. The worker's frames and the live page
/// compute it the same way, here (verify-r15 B1).
pub(crate) fn page_key(panel: &panel::Panel, accounts: u64) -> String {
    format!("{} account {accounts}", panel.paint_key()) // i18n: not shown (render cache identity)
}

#[derive(Clone)]
pub(crate) struct PaintSnapshot {
    pub(crate) panel: panel::Panel,
    entries: Arc<[Entry]>, picked: Option<Entry>, status: panel::Status,
    feedback: orbiters_n4n0_core::feedback::FeedbackView,
    facts: Arc<HashMap<String, panel::Facts>>,
    thumbs: HashMap<u64, TextureId>, pictures: HashMap<(u64, bool), TextureId>,
    pub(crate) time: f64, now: u64,
    /// The account generation it was made for ([`Worlds::paint_key`]).
    accounts: u64,
}
impl PaintSnapshot {
    fn with_data<T>(&self, f: impl FnOnce(&panel::Panel, &panel::Data<'_>) -> T) -> T {
        let thumb = |e: &Entry| self.thumbs.get(&e.id).copied();
        let picture = |id: u64, large: bool| self.pictures.get(&(id, large)).copied();
        let d = panel::Data { entries: &self.entries, picked: self.picked.as_ref(), status: &self.status, feedback: &self.feedback, facts: &self.facts, thumb: &thumb, picture: &picture, now: self.now };
        f(&self.panel, &d)
    }
    pub(crate) fn draw(&self, p: &egui::Painter) { self.with_data(|panel, d| panel.draw(p, d, self.time)); }
    pub(crate) fn hit(&self, at: Option<(f32, f32)>) -> Option<panel::Target> { at.and_then(|(x, y)| self.with_data(|panel, d| panel.hit(d, x, y))) }
    pub(crate) fn point(&mut self, at: Option<egui::Pos2>) { self.panel.hover = self.hit(at.map(|p| (p.x, p.y))); }
    pub(crate) fn key(&self) -> String { page_key(&self.panel, self.accounts) }
    pub(crate) fn hole(&self) -> Option<egui::Rect> { self.panel.hole(&self.status) }
    #[cfg(test)]
    pub(super) fn target_for_test(&self, target: panel::Target) -> Option<egui::Pos2> { self.with_data(|panel, d| panel.targets(d).into_iter().find(|(t, _)| *t == target).map(|(_, r)| r.center())) }
    pub(super) fn compatible(&self, live: &Worlds, entries: &[Entry], status: &panel::Status, picked: Option<&Entry>, target: panel::Target) -> bool {
        // Painted for the account before: nothing on it is pressed for this one (verify-r15 B1).
        if self.accounts != live.accounts || !self.panel.same_input(&live.panel, target) || self.picked.as_ref().map(|e| (e.id, &e.world_id)) != picked.map(|e| (e.id, &e.world_id)) { return false; }
        // Opening the composer needs no feedback read (F6), but its shown sign-in
        // meaning must still match the live account. Sending/deleting stays guarded.
        if target == panel::Target::WriteComment { return self.feedback.signed_in == live.feedback.signed_in; }
        if matches!(target, panel::Target::Key(_) | panel::Target::Space | panel::Target::Backspace | panel::Target::Comments | panel::Target::Join | panel::Target::Back | panel::Target::Labels | panel::Target::Pictures) { return true; }
        self.entries.len() == entries.len()
            && self.entries.iter().zip(entries).all(|(a, b)| (a.id, &a.world_id, a.capture) == (b.id, &b.world_id, b.capture))
            && self.picked.as_ref().map(|e| (e.id, &e.world_id)) == picked.map(|e| (e.id, &e.world_id))
            && self.status.list.vrchat == status.list.vrchat && self.status.go == status.go
            && self.feedback == live.feedback && *self.facts == live.facts
    }
}

#[cfg(test)]
mod review_navigation {
    use super::*;
    #[test]
    fn review_world_navigation_survives_arrivals_but_never_changes_its_world() {
        let ctx = egui::Context::default(); orbiters_ui::install(&ctx);
        let mut worlds = Worlds::alone(ctx.clone());
        let entry = |id: &str| {
            let found = orbiters_core::maps::FoundWorld { world: orbiters_core::maps::World { world_id: id.into(), name: id.into(), ..Default::default() }, ..Default::default() };
            Entry::world(&found, None, Default::default())
        };
        worlds.pick_for_test(entry("wrld_shown"));
        let shown = worlds.prepare_paint(&ctx);
        // The comments/description may arrive while a finger is pressing the still
        // shown navigation button. Data-changing controls must still reject this.
        worlds.feedback.signed_in = !shown.feedback.signed_in;
        worlds.panel.scroll_by(17.0);
        let accepts = |worlds: &Worlds, target| shown.compatible(worlds, &worlds.entries(), &worlds.status(), worlds.picked(), target);
        assert!(accepts(&worlds, panel::Target::Comments));
        assert!(accepts(&worlds, panel::Target::Join));
        assert!(accepts(&worlds, panel::Target::Back));
        assert!(accepts(&worlds, panel::Target::Labels));
        assert!(accepts(&worlds, panel::Target::Pictures));
        assert!(!accepts(&worlds, panel::Target::WriteComment));
        worlds.feedback.signed_in = shown.feedback.signed_in;
        assert!(accepts(&worlds, panel::Target::WriteComment), "the same shown sign-in/composer action survives unrelated arrivals and scroll");
        worlds.pick_for_test(entry("wrld_other"));
        assert!(!accepts(&worlds, panel::Target::Comments));
        assert!(!accepts(&worlds, panel::Target::Join));
        assert!(!accepts(&worlds, panel::Target::Back));
        assert!(!accepts(&worlds, panel::Target::Labels));
        assert!(!accepts(&worlds, panel::Target::Pictures));
        assert!(!accepts(&worlds, panel::Target::WriteComment));
        worlds.pick_for_test(entry("wrld_shown"));
        worlds.panel.restore(panel::Page::Go, worlds.panel.selected, 1.0);
        let chooser = worlds.prepare_paint(&ctx);
        worlds.feedback.signed_in = !chooser.feedback.signed_in;
        worlds.panel.scroll_by(17.0);
        assert!(chooser.compatible(&worlds, &worlds.entries(), &worlds.status(), worlds.picked(), panel::Target::Back));
        worlds.panel.restore(panel::Page::Comments, worlds.panel.selected, 1.0);
        assert!(!chooser.compatible(&worlds, &worlds.entries(), &worlds.status(), worlds.picked(), panel::Target::Back), "a different live page still drops the old Back");
    }
}
impl Worlds {
    pub(crate) fn snapshot_mode(&mut self, enabled: bool) { self.snapshot_mode = enabled; }
    pub(crate) fn present_paint(&mut self, picture: PaintSnapshot) {
        self.panel.adopt_metrics(&picture.panel);
        self.presented_paint = Some(picture);
    }
    pub(crate) fn prepare_paint(&mut self, _ctx: &egui::Context) -> PaintSnapshot {
        self.panel.vr_coast(self.time());
        // How soon a flip of the carousel shows the next one's picture (`vr-test.log`).
        if let Some((id, at)) = self.flipped_at.take_if(|(id, _)| self.panel.selected == Some(*id)) {
            super::super::vr_log::line(&format!("carousel: {id}'s picture painted {} ms after the flip", at.elapsed().as_millis())); // i18n: not shown (a log's line)
        }
        self.panel.dirty = false;
        self.hole_changed = false;
        if let Some(frame) = self.live_frame.take() {
            let image = egui::ColorImage::from_rgba_premultiplied(frame.size, &frame.rgba);
            match &mut self.live_texture {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => self.live_texture = Some(self.textures.load_texture("side-panel-view", image, egui::TextureOptions::LINEAR)),
            }
            if let Some(live) = self.panel.live.as_mut() { live.picture = self.live_texture.as_ref().map(|t| t.id()); live.plate = frame.plate; }
        }
        self.last_paint = Some(Instant::now());
        let entries = self.entries(); let mut status = self.status();
        self.facts_pending = false;
        if let Some(fixture) = &self.paint_fixture {
            status.fetching = false; status.listing_error = None; status.list.loading = false; status.list.more = false; status.list.error = None;
            let mut panel = self.panel.clone(); panel.moves = false;
            return PaintSnapshot { panel, entries: entries.into(), picked: self.picked().cloned(), status, feedback: self.feedback.clone(), facts: Arc::new(self.facts.clone()), thumbs: fixture.thumbs.iter().map(|(id, tex)| (*id, tex.id())).collect(), pictures: HashMap::new(), time: 1.0, now: fixture.now, accounts: self.accounts };
        }
        if self.panel.sort == panel::Sort::Smooth { for e in entries.iter().filter(|e| e.capture) { self.facts_for(&e.world_id); } }
        let visible = self.visible(&entries, &status);
        for v in &visible { if v.page { self.facts_for(&v.world); } }
        let thumbs = self.thumbnails(&visible);
        let pictures = self.world_pictures(&entries);
        PaintSnapshot { panel: self.panel.clone(), entries: entries.into(), picked: self.picked().cloned(), status, feedback: self.feedback.clone(), facts: Arc::new(self.facts.clone()), thumbs, pictures, time: self.time(), now: unix_now(), accounts: self.accounts }
    }
}

pub(super) struct PaintFixture { pub(super) entries: Vec<Entry>, thumbs: HashMap<u64, egui::TextureHandle>, now: u64 }
impl Worlds {
    /// CPU replay fixture. All image sources are local and acknowledged before the first turn.
    pub(crate) fn freeze_paint_fixture(&mut self, entries: Vec<Entry>, images: Vec<egui::ColorImage>) -> bool {
        let ready = entries.len() == images.len();
        let thumbs = entries.iter().zip(images).map(|(entry, image)| (entry.id, self.ctx.load_texture(format!("fixture {}", entry.id), image, egui::TextureOptions::LINEAR))).collect(); // i18n: not shown (harness texture key)
        self.paint_fixture = Some(PaintFixture { entries, thumbs, now: 1_800_000_000 });
        ready
    }
    pub(crate) fn paint_fixture_ready(&self) -> bool { self.paint_fixture.as_ref().is_some_and(|f| f.entries.len() == f.thumbs.len()) }
    /// Selects a local replay page without effects or external data requests.
    pub(crate) fn paint_fixture_page(&mut self, page: panel::Page) -> bool {
        let Some(entry) = self.paint_fixture.as_ref().and_then(|f| f.entries.first().cloned()) else { return false };
        self.open = true;
        self.picked = Some(entry.clone());
        self.panel.restore(page, Some(entry.id), 1.0);
        true
    }
}
