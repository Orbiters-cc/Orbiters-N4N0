//! The sender of Orb's text panel (contracts `Docs/orb.md`, Text): turns text into pages and, step by step, into
//! the pointer and sync values VRChat syncs to everyone, resending blocks while a page is up so that people who
//! arrive late see it whole.

use orbiters_contracts::orb::{self, params, Page, POINTER_CLEAR, TEXT_LENGTH};
use rosc::OscType;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One OSC message to VRChat: an avatar parameter (full address) and its value.
pub type Message = (String, OscType);

/// Time between the phases of a block (contracts `Docs/orb.md`, Text). VRChat may apply sync values a frame or more
/// before a pointer sent with them, which wrote each block with the next one's letters; so no message batch changes
/// both: the pointer goes to 0 (nothing written), then the values change, then the pointer moves to the block.
pub const PHASE_GAP: Duration = Duration::from_millis(80);

/// What a block still has to send.
enum Phase {
    /// The pointer is at 0: the block's values go next.
    Values(usize),
    /// Its values were sent (these cells): the pointer goes to the block next.
    Pointer(usize, Vec<u8>),
}

fn parameter(name: &str) -> String {
    format!("{}{name}", orbiters_core::osc::PARAMETERS_PREFIX)
}

/// How long a page stays up once it was fully sent: time to read its words, at least three seconds.
pub fn reading_time(page: &Page) -> Duration {
    let text = orb::page_text(page);
    let words = text.split_whitespace().count() as u64;
    Duration::from_millis((1000 + words * 300).max(3000))
}

pub struct TextSender {
    sync: usize,
    step: Duration,
    /// What the panel holds, as far as the steps sent it.
    sent: Page,
    /// Whether `sent` is known: after an avatar (re)load the panel's cells are not, so it is cleared first.
    known: bool,
    current: Option<Page>,
    queue: VecDeque<Page>,
    /// When the current page was fully sent.
    complete_at: Option<Instant>,
    hold: Duration,
    /// The next block resent while a page is up.
    resync: usize,
    /// What `Orb/Text/Visible` was last set to.
    visible: bool,
    next_step: Option<Instant>,
    clear: bool,
    /// The last page stays up until replaced or hidden (the AFK message).
    sticky: bool,
    /// The block being sent and when its next phase is due.
    phase: Option<(Instant, Phase)>,
    /// `Orb/Text/Pointer` as this sender last set it; unknown after a reload or when something else used the channel.
    pointer: Option<i32>,
}

impl TextSender {
    /// A sender for a prop with `sync` text sync values (4, 8 or 16), one step every `step`.
    pub fn new(sync: usize, step: Duration) -> Self {
        Self {
            sync: if orb::SYNC_COUNTS.contains(&sync) { sync } else { 8 },
            step: step.max(Duration::from_millis(orb::MIN_STEP_MS)),
            sent: [0; TEXT_LENGTH],
            known: false,
            current: None,
            queue: VecDeque::new(),
            complete_at: None,
            hold: Duration::ZERO,
            resync: 0,
            visible: false,
            next_step: None,
            clear: false,
            sticky: false,
            phase: None,
            pointer: None,
        }
    }

    pub fn sync(&self) -> usize {
        self.sync
    }

    pub fn set_step(&mut self, step: Duration) {
        self.step = step.max(Duration::from_millis(orb::MIN_STEP_MS));
    }

    /// Shows `text` from the next step on, replacing what was shown or waiting; blank text hides the panel.
    pub fn show(&mut self, text: &str) {
        self.sticky = false;
        self.load(text);
    }

    /// Shows `text` and keeps its last page up until something replaces or hides it.
    pub fn show_sticky(&mut self, text: &str) {
        self.sticky = true;
        self.load(text);
    }

    /// Whether what is up stays up (`show_sticky`).
    pub fn sticky(&self) -> bool {
        self.sticky && self.busy()
    }

    fn load(&mut self, text: &str) {
        let mut pages: VecDeque<Page> = orb::pages(text).into();
        self.complete_at = None;
        self.resync = 0;
        self.current = pages.pop_front();
        self.queue = pages;
        if let Some(page) = &self.current {
            self.hold = reading_time(page);
        } else {
            self.clear = true;
        }
    }

    /// Clears and hides the panel at the next step.
    pub fn hide(&mut self) {
        self.sticky = false;
        self.current = None;
        self.queue.clear();
        self.clear = true;
        self.phase = None;
    }

    /// The avatar was (re)loaded: its panel's cells are unknown until cleared; what was being shown starts again.
    pub fn reset(&mut self) {
        self.known = false;
        self.visible = false;
        self.complete_at = None;
        self.next_step = None;
        self.phase = None;
        self.pointer = None;
    }

    /// Whether a step is due at `now` and would send something (a page, a clear), or a block is between its phases.
    pub fn wants_step(&self, now: Instant) -> bool {
        self.phase.is_some() || self.next_step.is_none_or(|t| now >= t) && (self.busy() || !self.known || self.clear)
    }

    /// Whether a block is between its phases: nothing else may use the text channel until it is done.
    pub fn mid_block(&self) -> bool {
        self.phase.is_some()
    }

    /// Something else uses the text channel until `at` (Orb's screen takes its turns): no step before then, and the
    /// pointer is no longer known (the next block moves it to 0 first). A block between its phases starts again later.
    pub fn hold_until(&mut self, at: Instant) {
        self.next_step = Some(self.next_step.map_or(at, |t| t.max(at)));
        self.phase = None;
        self.pointer = None;
    }

    /// The time between two steps.
    pub fn step(&self) -> Duration {
        self.step
    }

    /// Whether a page is up or waiting.
    pub fn busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    /// Pages left after the one on display.
    pub fn pages_waiting(&self) -> usize {
        self.queue.len()
    }

    /// The page on display, as text.
    pub fn current_text(&self) -> Option<String> {
        self.current.as_ref().map(|p| orb::page_text(p).lines().map(str::trim_end).collect::<Vec<_>>().join("\n"))
    }

    /// The messages of the step due at `now` (none between steps or with nothing to do).
    pub fn tick(&mut self, now: Instant) -> Vec<Message> {
        if let Some((at, phase)) = self.phase.take() {
            if now < at {
                self.phase = Some((at, phase));
                return Vec::new();
            }
            return self.phase_messages(phase, now);
        }
        if self.next_step.is_some_and(|t| now < t) {
            return Vec::new();
        }
        let out = self.step_messages(now);
        if !out.is_empty() {
            self.next_step = Some(now + self.step);
        }
        out
    }

    fn set_pointer(&mut self, pointer: i32) -> Message {
        self.pointer = Some(pointer);
        (parameter(params::TEXT_POINTER), OscType::Int(pointer))
    }

    /// A block's next phase: its values (the pointer is at 0), or the pointer moving to it, after which the step's time
    /// starts (the pointer stays on the block a whole step, for people who receive it late).
    fn phase_messages(&mut self, phase: Phase, now: Instant) -> Vec<Message> {
        match phase {
            Phase::Values(block) => {
                let page = self.current.unwrap_or([0; TEXT_LENGTH]);
                let n = self.sync;
                let cells = page[block * n..(block + 1) * n].to_vec();
                let out = cells.iter().enumerate().map(|(i, &g)| (parameter(&params::text_sync(i)), OscType::Float(orb::sync_value(g)))).collect();
                self.phase = Some((now + PHASE_GAP, Phase::Pointer(block, cells)));
                out
            }
            Phase::Pointer(block, cells) => {
                let n = self.sync;
                self.sent[block * n..(block + 1) * n].copy_from_slice(&cells);
                self.next_step = Some(now + self.step);
                // Complete once its last block went: its reading time starts now.
                if let Some(page) = self.current {
                    if self.complete_at.is_none() && (0..orb::pointer_count(n)).all(|b| page[b * n..(b + 1) * n] == self.sent[b * n..(b + 1) * n]) {
                        self.complete_at = Some(now);
                    }
                }
                vec![self.set_pointer(block as i32 + 1)]
            }
        }
    }

    fn step_messages(&mut self, now: Instant) -> Vec<Message> {
        let mut out = Vec::new();
        if !self.known || self.clear {
            out.push(self.set_pointer(POINTER_CLEAR));
            self.sent = [0; TEXT_LENGTH];
            self.known = true;
            self.clear = false;
            if self.current.is_none() || !self.visible {
                let show = self.current.is_some();
                out.push((parameter(params::TEXT_VISIBLE), OscType::Bool(show)));
                self.visible = show;
            }
            return out;
        }
        let Some(page) = self.current else { return out };
        if !self.visible {
            out.push((parameter(params::TEXT_VISIBLE), OscType::Bool(true)));
            self.visible = true;
        }
        let n = self.sync;
        let pointers = orb::pointer_count(n);
        let differs = |sent: &Page, b: usize| page[b * n..(b + 1) * n] != sent[b * n..(b + 1) * n];
        if let Some(block) = (0..pointers).find(|&b| differs(&self.sent, b)) {
            out.extend(self.block(block, now));
            return out;
        }
        let complete = *self.complete_at.get_or_insert(now);
        let last = self.queue.is_empty();
        if now.duration_since(complete) >= self.hold && !(last && self.sticky) {
            self.complete_at = None;
            self.resync = 0;
            match self.queue.pop_front() {
                Some(next) => {
                    self.hold = reading_time(&next);
                    self.current = Some(next);
                    // The next page's first changed block goes at once.
                    out.extend(self.step_messages(now));
                }
                None => {
                    self.current = None;
                    out.push(self.set_pointer(POINTER_CLEAR));
                    out.push((parameter(params::TEXT_VISIBLE), OscType::Bool(false)));
                    self.sent = [0; TEXT_LENGTH];
                    self.visible = false;
                }
            }
            return out;
        }
        // Up and whole: resend a block in turn for whoever loaded the avatar after it was sent.
        let block = self.resync % pointers;
        self.resync = (block + 1) % pointers;
        out.extend(self.block(block, now));
        out
    }

    /// Starts sending `block`: its values at once when the pointer writes nothing (0, or 255 that clears), else the
    /// pointer to 0 first.
    fn block(&mut self, block: usize, now: Instant) -> Vec<Message> {
        if matches!(self.pointer, Some(0) | Some(POINTER_CLEAR)) {
            return self.phase_messages(Phase::Values(block), now);
        }
        self.phase = Some((now + PHASE_GAP, Phase::Values(block)));
        vec![self.set_pointer(0)]
    }
}

/// The prop's animator as far as the panel goes (for tests and previews): the pointer and values it last received,
/// and its cells. A cell takes value i while the pointer is at its block, whichever of the two arrived last.
#[derive(Clone, Debug)]
pub struct PanelModel {
    pub cells: Page,
    pointer: i32,
    values: [Option<u8>; 16],
}

impl PanelModel {
    pub fn new(cells: Page) -> Self {
        Self { cells, pointer: 0, values: [None; 16] }
    }

    /// Applies `messages` in order; returns the last visibility they set.
    pub fn apply(&mut self, sync: usize, messages: &[Message]) -> Option<bool> {
        let mut visible = None;
        let prefix = orbiters_core::osc::PARAMETERS_PREFIX;
        for (address, value) in messages {
            let Some(name) = address.strip_prefix(prefix) else { continue };
            match (name, value) {
                (n, OscType::Bool(v)) if n == params::TEXT_VISIBLE => visible = Some(*v),
                (n, OscType::Int(p)) if n == params::TEXT_POINTER => {
                    self.pointer = *p;
                    if self.pointer == POINTER_CLEAR {
                        self.cells = [0; TEXT_LENGTH];
                    }
                    for i in 0..sync.min(16) {
                        self.write(sync, i);
                    }
                }
                (n, OscType::Float(v)) => {
                    if let Some(i) = n.strip_prefix(params::TEXT_SYNC_PREFIX).and_then(|i| i.parse::<usize>().ok()).filter(|&i| i < sync.min(16)) {
                        self.values[i] = Some(orb::glyph_of_value(*v));
                        self.write(sync, i);
                    }
                }
                _ => {}
            }
        }
        visible
    }

    fn write(&mut self, sync: usize, i: usize) {
        if let (true, Some(g)) = ((1..=orb::pointer_count(sync) as i32).contains(&self.pointer), self.values[i]) {
            self.cells[(self.pointer as usize - 1) * sync + i] = g;
        }
    }
}

/// What a panel shows after `messages` (all of them, in order) were applied to `cells`.
pub fn apply(cells: &mut Page, sync: usize, messages: &[Message]) -> Option<bool> {
    let mut panel = PanelModel::new(*cells);
    let visible = panel.apply(sync, messages);
    *cells = panel.cells;
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: Duration = Duration::from_millis(300);
    // The desktop's loop: the sender is asked every 50 ms.
    const TICK: Duration = Duration::from_millis(50);
    // A block at most: the pointer to 0, the values, the pointer to it, then the step.
    const BLOCK: Duration = Duration::from_millis(300 + 2 * 80 + 100);

    /// Ticks from `start` for `time`; returns the time after it and the last visibility set. Each tick's messages reach
    /// the panel in reverse order, as VRChat may apply them: the phases make it harmless.
    fn run(sender: &mut TextSender, panel: &mut PanelModel, start: Instant, time: Duration) -> (Instant, Option<bool>) {
        let mut t = start;
        let mut visible = None;
        while t < start + time {
            let mut out = sender.tick(t);
            out.reverse();
            if let Some(v) = panel.apply(sender.sync(), &out) {
                visible = Some(v);
            }
            t += TICK;
        }
        (t, visible)
    }

    fn first_row(cells: &Page) -> String {
        orb::page_text(cells).lines().next().unwrap().trim_end().to_string()
    }

    #[test]
    fn a_page_arrives_block_by_block_then_hides() {
        for sync in orb::SYNC_COUNTS {
            let mut sender = TextSender::new(sync, STEP);
            let mut panel = PanelModel::new([7; TEXT_LENGTH]);
            sender.show("Ça marche ! Je mets ton sweat.");
            // The first step clears what the panel held before and shows it.
            let t0 = Instant::now();
            assert_eq!(panel.apply(sync, &sender.tick(t0)), Some(true));
            assert_eq!(panel.cells, [0; TEXT_LENGTH]);
            // A block at a time: 30 characters take 32 / sync blocks.
            let (t, _) = run(&mut sender, &mut panel, t0 + STEP, BLOCK * (32 / sync) as u32);
            assert_eq!(first_row(&panel.cells), "Ça marche ! Je mets ton sweat.", "{sync} sync values");
            assert!(sender.busy());
            // Up long enough to read, then cleared and hidden.
            let (_, visible) = run(&mut sender, &mut panel, t + Duration::from_secs(4), BLOCK);
            assert_eq!(visible, Some(false));
            assert!(!sender.busy());
            assert_eq!(panel.cells, [0; TEXT_LENGTH]);
        }
    }

    #[test]
    fn no_batch_changes_the_pointer_and_the_values_together() {
        let mut sender = TextSender::new(8, STEP);
        sender.show("one two three four five six seven eight nine ten eleven twelve thirteen fourteen");
        let mut t = Instant::now();
        let mut pointers = 0;
        for _ in 0..400 {
            let out = sender.tick(t);
            let pointer = out.iter().any(|(a, _)| a.ends_with(params::TEXT_POINTER));
            let values = out.iter().any(|(a, _)| a.contains(params::TEXT_SYNC_PREFIX));
            assert!(!(pointer && values), "{out:?}");
            pointers += pointer as usize;
            t += TICK;
        }
        assert!(pointers > 10);
    }

    #[test]
    fn only_changed_blocks_are_sent_and_pages_follow() {
        let mut sender = TextSender::new(8, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        let text = (1..=6).map(|i| format!("ligne {i}")).collect::<Vec<_>>().join("\n");
        sender.show(&text);
        assert_eq!(sender.pages_waiting(), 1);
        let t0 = Instant::now();
        panel.apply(8, &sender.tick(t0));
        // Rows hold "ligne n" in their first block only: four blocks for four rows.
        let (t, _) = run(&mut sender, &mut panel, t0 + STEP, BLOCK * 4);
        assert_eq!(orb::page_text(&panel.cells).lines().filter(|l| l.starts_with("ligne")).count(), 4);
        // After the first page's reading time the second replaces it, again one changed block at a time.
        let (_, _) = run(&mut sender, &mut panel, t + Duration::from_secs(5), BLOCK * 4);
        let shown: Vec<String> = orb::page_text(&panel.cells).lines().map(|l| l.trim_end().to_string()).collect();
        assert_eq!(shown, vec!["ligne 5", "ligne 6", "", ""]);
    }

    #[test]
    fn a_whole_page_is_resent_while_it_is_up() {
        let mut sender = TextSender::new(16, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        let text = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
        // Kept up (sticky) for as long as the round takes.
        sender.show_sticky(text);
        let (t, _) = run(&mut sender, &mut panel, Instant::now(), BLOCK * (1 + orb::pointer_count(16)) as u32);
        // Someone who loads the avatar now starts from an empty panel and gets every block within one round.
        let mut late = PanelModel::new([0; TEXT_LENGTH]);
        let mut pointers = std::collections::HashSet::new();
        let mut now = t;
        while now < t + BLOCK * orb::pointer_count(16) as u32 {
            let out = sender.tick(now);
            for (a, v) in &out {
                if let (true, OscType::Int(p)) = (a.ends_with(params::TEXT_POINTER), v) {
                    pointers.insert(*p);
                }
            }
            late.apply(16, &out);
            now += TICK;
        }
        assert!(pointers.len() >= orb::pointer_count(16));
        assert_eq!(late.cells, panel.cells);
    }

    #[test]
    fn steps_wait_their_turn_and_a_reload_clears_first() {
        let mut sender = TextSender::new(8, STEP);
        sender.show("Hello");
        let t0 = Instant::now();
        assert!(!sender.tick(t0).is_empty());
        assert!(sender.tick(t0 + Duration::from_millis(100)).is_empty(), "too early");
        assert!(!sender.tick(t0 + STEP).is_empty());
        sender.reset();
        let mut panel = PanelModel::new([9; TEXT_LENGTH]);
        let out = sender.tick(t0 + Duration::from_millis(310));
        assert_eq!(out[0], (parameter(params::TEXT_POINTER), OscType::Int(POINTER_CLEAR)));
        assert_eq!(panel.apply(8, &out), Some(true));
        // What was shown comes back after the clear.
        run(&mut sender, &mut panel, t0 + Duration::from_millis(620), BLOCK * 2);
        assert_eq!(first_row(&panel.cells), "Hello");
    }

    #[test]
    fn a_block_cut_short_by_the_screen_starts_again() {
        let mut sender = TextSender::new(8, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        sender.show("Hello there, how are you today?");
        let t0 = Instant::now();
        let (t, _) = run(&mut sender, &mut panel, t0, BLOCK + STEP);
        // Mid block, something else takes the channel: the pointer is unknown, the block is sent again from 0.
        sender.hold_until(t + STEP);
        assert!(!sender.mid_block());
        panel.apply(8, &[(parameter(params::TEXT_POINTER), OscType::Int(20)), (parameter(&params::text_sync(0)), OscType::Float(0.5))]);
        let out = sender.tick(t + STEP);
        assert_eq!(out, vec![(parameter(params::TEXT_POINTER), OscType::Int(0))], "the pointer goes to 0 before any value");
        run(&mut sender, &mut panel, t + STEP, BLOCK * 4);
        assert_eq!(first_row(&panel.cells), "Hello there, how are you today?");
    }

    #[test]
    fn a_sticky_page_stays_up() {
        let mut sender = TextSender::new(8, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        sender.show_sticky("AFK: back soon");
        let (t, _) = run(&mut sender, &mut panel, Instant::now(), BLOCK * 3);
        let (t, visible) = run(&mut sender, &mut panel, t + Duration::from_secs(600), BLOCK * 3);
        assert_eq!(visible, None, "never hidden");
        assert!(sender.sticky());
        assert_eq!(first_row(&panel.cells), "AFK: back soon");
        sender.hide();
        let (_, visible) = run(&mut sender, &mut panel, t, BLOCK);
        assert_eq!(visible, Some(false));
        assert!(!sender.sticky());
    }

    #[test]
    fn blank_text_hides() {
        let mut sender = TextSender::new(8, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        sender.show("Yo");
        let (t, _) = run(&mut sender, &mut panel, Instant::now(), BLOCK * 3);
        sender.show("   ");
        let (_, visible) = run(&mut sender, &mut panel, t, BLOCK);
        assert_eq!(visible, Some(false));
        assert!(!sender.busy());
    }
}
