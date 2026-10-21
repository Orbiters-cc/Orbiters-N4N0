//! The sender of Orb's text panel (contracts `Docs/orb.md`, Text): turns text into pages and, step by step, into
//! the pointer and sync values VRChat syncs to everyone, resending blocks while a page is up so that people who
//! arrive late see it whole.

use orbiters_contracts::orb::{self, params, Page, POINTER_CLEAR, TEXT_LENGTH};
use rosc::OscType;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One OSC message to VRChat: an avatar parameter (full address) and its value.
pub type Message = (String, OscType);

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
    }

    /// The avatar was (re)loaded: its panel's cells are unknown until cleared; what was being shown starts again.
    pub fn reset(&mut self) {
        self.known = false;
        self.visible = false;
        self.complete_at = None;
        self.next_step = None;
    }

    /// Whether a step is due at `now` and would send something (a page, a clear).
    pub fn wants_step(&self, now: Instant) -> bool {
        self.next_step.is_none_or(|t| now >= t) && (self.busy() || !self.known || self.clear)
    }

    /// Something else uses the text channel until `at` (Orb's screen takes its turns): no step before then.
    pub fn hold_until(&mut self, at: Instant) {
        self.next_step = Some(self.next_step.map_or(at, |t| t.max(at)));
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
        if self.next_step.is_some_and(|t| now < t) {
            return Vec::new();
        }
        let out = self.step_messages(now);
        if !out.is_empty() {
            self.next_step = Some(now + self.step);
        }
        out
    }

    fn step_messages(&mut self, now: Instant) -> Vec<Message> {
        let mut out = Vec::new();
        if !self.known || self.clear {
            out.push((parameter(params::TEXT_POINTER), OscType::Int(POINTER_CLEAR)));
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
            out.extend(self.block(&page, block));
            // Complete once its last block went: its reading time starts now.
            if !(0..pointers).any(|b| differs(&self.sent, b)) {
                self.complete_at = Some(now);
            }
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
                    out.push((parameter(params::TEXT_POINTER), OscType::Int(POINTER_CLEAR)));
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
        out.extend(self.block(&page, block));
        out
    }

    fn block(&mut self, page: &Page, block: usize) -> Vec<Message> {
        let n = self.sync;
        let mut out = vec![(parameter(params::TEXT_POINTER), OscType::Int(block as i32 + 1))];
        for i in 0..n {
            let cell = block * n + i;
            out.push((parameter(&params::text_sync(i)), OscType::Float(orb::sync_value(page[cell]))));
            self.sent[cell] = page[cell];
        }
        out
    }
}

/// What a panel shows after `messages` were applied to cells `cells` (the prop's animator, for tests and previews).
pub fn apply(cells: &mut Page, sync: usize, messages: &[Message]) -> Option<bool> {
    let mut visible = None;
    let mut pointer = 0i32;
    let prefix = orbiters_core::osc::PARAMETERS_PREFIX;
    for (address, value) in messages {
        let Some(name) = address.strip_prefix(prefix) else { continue };
        match (name, value) {
            (n, OscType::Bool(v)) if n == params::TEXT_VISIBLE => visible = Some(*v),
            (n, OscType::Int(p)) if n == params::TEXT_POINTER => {
                pointer = *p;
                if pointer == POINTER_CLEAR {
                    *cells = [0; TEXT_LENGTH];
                }
            }
            (n, OscType::Float(v)) => {
                if let Some(i) = n.strip_prefix(params::TEXT_SYNC_PREFIX).and_then(|i| i.parse::<usize>().ok()) {
                    if (1..=orb::pointer_count(sync) as i32).contains(&pointer) && i < sync {
                        cells[(pointer as usize - 1) * sync + i] = orb::glyph_of_value(*v);
                    }
                }
            }
            _ => {}
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: Duration = Duration::from_millis(300);

    /// Runs `steps` steps from `start`; returns the time after them and the last visibility a step set.
    fn run(sender: &mut TextSender, cells: &mut Page, start: Instant, steps: usize) -> (Instant, Option<bool>) {
        let mut t = start;
        let mut visible = None;
        for _ in 0..steps {
            let out = sender.tick(t);
            if let Some(v) = apply(cells, sender.sync(), &out) {
                visible = Some(v);
            }
            t += STEP;
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
            let mut cells: Page = [7; TEXT_LENGTH];
            sender.show("Ça marche ! Je mets ton sweat.");
            // The first step clears what the panel held before and shows it.
            let t0 = Instant::now();
            assert_eq!(apply(&mut cells, sync, &sender.tick(t0)), Some(true));
            assert_eq!(cells, [0; TEXT_LENGTH]);
            // One block per step: 30 characters take 32 / sync steps.
            let (t, _) = run(&mut sender, &mut cells, t0 + STEP, 32 / sync);
            assert_eq!(first_row(&cells), "Ça marche ! Je mets ton sweat.", "{sync} sync values");
            assert!(sender.busy());
            // Up long enough to read, then cleared and hidden.
            let (_, visible) = run(&mut sender, &mut cells, t + Duration::from_secs(4), 1);
            assert_eq!(visible, Some(false));
            assert!(!sender.busy());
            assert_eq!(cells, [0; TEXT_LENGTH]);
        }
    }

    #[test]
    fn only_changed_blocks_are_sent_and_pages_follow() {
        let mut sender = TextSender::new(8, STEP);
        let mut cells: Page = [0; TEXT_LENGTH];
        let text = (1..=6).map(|i| format!("ligne {i}")).collect::<Vec<_>>().join("\n");
        sender.show(&text);
        assert_eq!(sender.pages_waiting(), 1);
        let t0 = Instant::now();
        apply(&mut cells, 8, &sender.tick(t0));
        // Rows hold "ligne n" in their first block only: four steps for four rows.
        let (t, _) = run(&mut sender, &mut cells, t0 + STEP, 4);
        assert_eq!(orb::page_text(&cells).lines().filter(|l| l.starts_with("ligne")).count(), 4);
        // After the first page's reading time the second replaces it, again one changed block per step.
        let (_, _) = run(&mut sender, &mut cells, t + Duration::from_secs(5), 4);
        let shown: Vec<String> = orb::page_text(&cells).lines().map(|l| l.trim_end().to_string()).collect();
        assert_eq!(shown, vec!["ligne 5", "ligne 6", "", ""]);
    }

    #[test]
    fn a_whole_page_is_resent_while_it_is_up() {
        let mut sender = TextSender::new(16, STEP);
        let mut cells: Page = [0; TEXT_LENGTH];
        let text = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
        sender.show(text);
        let (t, _) = run(&mut sender, &mut cells, Instant::now(), 1 + orb::pointer_count(16));
        // Someone who loads the avatar now starts from an empty panel and gets every block within one round.
        let mut late: Page = [0; TEXT_LENGTH];
        let mut pointers = std::collections::HashSet::new();
        let mut now = t;
        for _ in 0..orb::pointer_count(16) {
            let out = sender.tick(now);
            for (a, v) in &out {
                if let (true, OscType::Int(p)) = (a.ends_with(params::TEXT_POINTER), v) {
                    pointers.insert(*p);
                }
            }
            apply(&mut late, 16, &out);
            now += STEP;
        }
        assert_eq!(pointers.len(), orb::pointer_count(16));
        assert_eq!(late, cells);
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
        let mut cells: Page = [9; TEXT_LENGTH];
        let out = sender.tick(t0 + Duration::from_millis(310));
        assert_eq!(out[0], (parameter(params::TEXT_POINTER), OscType::Int(POINTER_CLEAR)));
        assert_eq!(apply(&mut cells, 8, &out), Some(true));
        // What was shown comes back after the clear.
        run(&mut sender, &mut cells, t0 + Duration::from_millis(620), 2);
        assert_eq!(first_row(&cells), "Hello");
    }

    #[test]
    fn a_sticky_page_stays_up() {
        let mut sender = TextSender::new(8, STEP);
        let mut cells: Page = [0; TEXT_LENGTH];
        sender.show_sticky("AFK: back soon");
        let (t, _) = run(&mut sender, &mut cells, Instant::now(), 3);
        let (t, visible) = run(&mut sender, &mut cells, t + Duration::from_secs(600), 5);
        assert_eq!(visible, None, "never hidden");
        assert!(sender.sticky());
        assert_eq!(first_row(&cells), "AFK: back soon");
        sender.hide();
        let (_, visible) = run(&mut sender, &mut cells, t, 1);
        assert_eq!(visible, Some(false));
        assert!(!sender.sticky());
    }

    #[test]
    fn blank_text_hides() {
        let mut sender = TextSender::new(8, STEP);
        let mut cells: Page = [0; TEXT_LENGTH];
        sender.show("Yo");
        let (t, _) = run(&mut sender, &mut cells, Instant::now(), 3);
        sender.show("   ");
        let out = sender.tick(t + STEP);
        assert_eq!(apply(&mut cells, 8, &out), Some(false));
        assert!(!sender.busy());
    }
}
