//! The sender of N4N0's text panel (contracts `Docs/n4n0.md`, Text): turns text into pages and, step by step, into
//! the pointer and sync values VRChat syncs to everyone, resending blocks while a page is up so that people who
//! arrive late see it whole.

use orbiters_contracts::n4n0::{self, params, Page, POINTER_CLEAR, TEXT_LENGTH};
use rosc::OscType;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One OSC message to VRChat: an avatar parameter (full address) and its value.
pub type Message = (String, OscType);

/// Time between the phases of a block (contracts `Docs/n4n0.md`, Text). VRChat may apply sync values a frame or more
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

/// A letter with an accent written as two characters (the letter, then a combining accent: how some keyboards and
/// speech engines write them) as the one the panel has a glyph for.
fn compose(base: char, mark: char) -> Option<char> {
    const MARKS: [(char, &str, &str); 7] = [
        ('\u{0300}', "aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        ('\u{0301}', "aeiouyAEIOUY", "áéíóúýÁÉÍÓÚÝ"),
        ('\u{0302}', "aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        ('\u{0303}', "anoANO", "ãñõÃÑÕ"),
        ('\u{0308}', "aeiouyAEIOUY", "äëïöüÿÄËÏÖÜŸ"),
        ('\u{030A}', "aA", "åÅ"),
        ('\u{0327}', "cC", "çÇ"),
    ];
    let (_, from, to) = MARKS.iter().find(|(m, _, _)| *m == mark)?;
    from.chars().position(|c| c == base).and_then(|i| to.chars().nth(i))
}

/// A Vietnamese letter (Latin Extended Additional, U+1EA0 to U+1EF9) as the nearest the panel shows: its circumflex
/// kept when it has one (`ệ` → `ê`), its other marks dropped (`ặ` → `a`, `ở` → `o`).
fn vietnamese(c: char) -> Option<char> {
    let code = c as u32;
    if !(0x1EA0..=0x1EF9).contains(&code) {
        return match c {
            'Ơ' => Some('O'),
            'ơ' => Some('o'),
            'Ư' => Some('U'),
            'ư' => Some('u'),
            _ => None,
        };
    }
    let pair = (code - 0x1EA0) / 2;
    let lower = (code - 0x1EA0) % 2 == 1;
    let base = match pair {
        0..=1 | 7..=11 => 'A',
        2..=6 => 'Â',
        12..=14 => 'E',
        15..=19 => 'Ê',
        20..=21 => 'I',
        22..=23 | 29..=33 => 'O',
        24..=28 => 'Ô',
        34..=40 => 'U',
        _ => 'Y',
    };
    Some(if lower { base.to_lowercase().next().unwrap_or(base) } else { base })
}

/// A Greek letter in Latin letters (ELOT 743, accents dropped).
fn greek(c: char) -> Option<&'static str> {
    Some(match c {
        'α' | 'ά' => "a",
        'β' => "v",
        'γ' => "g",
        'δ' => "d",
        'ε' | 'έ' => "e",
        'ζ' => "z",
        'η' | 'ή' => "i",
        'θ' => "th",
        'ι' | 'ί' | 'ϊ' | 'ΐ' => "i",
        'κ' => "k",
        'λ' => "l",
        'μ' => "m",
        'ν' => "n",
        'ξ' => "x",
        'ο' | 'ό' => "o",
        'π' => "p",
        'ρ' => "r",
        'σ' | 'ς' => "s",
        'τ' => "t",
        'υ' | 'ύ' | 'ϋ' | 'ΰ' => "y",
        'φ' => "f",
        'χ' => "ch",
        'ψ' => "ps",
        'ω' | 'ώ' => "o",
        'Α' | 'Ά' => "A",
        'Β' => "V",
        'Γ' => "G",
        'Δ' => "D",
        'Ε' | 'Έ' => "E",
        'Ζ' => "Z",
        'Η' | 'Ή' => "I",
        'Θ' => "Th",
        'Ι' | 'Ί' | 'Ϊ' => "I",
        'Κ' => "K",
        'Λ' => "L",
        'Μ' => "M",
        'Ν' => "N",
        'Ξ' => "X",
        'Ο' | 'Ό' => "O",
        'Π' => "P",
        'Ρ' => "R",
        'Σ' => "S",
        'Τ' => "T",
        'Υ' | 'Ύ' | 'Ϋ' => "Y",
        'Φ' => "F",
        'Χ' => "Ch",
        'Ψ' => "Ps",
        'Ω' | 'Ώ' => "O",
        _ => return None,
    })
}

/// `text` with what the contract's folding (`n4n0::fold`) cannot place made placeable first: accents written as a
/// letter and a combining mark joined into one letter, Vietnamese letters as their nearest, Greek in Latin letters.
/// The contract would show them as `?`, or drop the accent.
pub fn fold_more(text: &str) -> std::borrow::Cow<'_, str> {
    let placeable = |c: char| c.is_ascii() || !(('\u{0300}'..='\u{036F}').contains(&c) || vietnamese(c).is_some() || greek(c).is_some());
    if text.chars().all(placeable) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut last: Option<char> = None;
    for c in text.chars() {
        if ('\u{0300}'..='\u{036F}').contains(&c) {
            // A combining mark: onto the letter before it when they make one the panel has; dropped otherwise.
            if let Some(joined) = last.and_then(|b| compose(b, c)) {
                out.pop();
                out.push(joined);
                last = Some(joined);
            }
            continue;
        }
        if let Some(v) = vietnamese(c) {
            out.push(v);
            last = Some(v);
        } else if let Some(latin) = greek(c) {
            out.push_str(latin);
            last = latin.chars().last();
        } else {
            out.push(c);
            last = Some(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// How long a page stays up once it was fully sent: time to read its words, at least three seconds.
pub fn reading_time(page: &Page) -> Duration {
    let text = n4n0::page_text(page);
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
    /// What `N4N0/Text/Visible` was last set to.
    visible: bool,
    next_step: Option<Instant>,
    clear: bool,
    /// The last page stays up until replaced or hidden (the AFK message).
    sticky: bool,
    /// The block being sent and when its next phase is due.
    phase: Option<(Instant, Phase)>,
    /// `N4N0/Text/Pointer` as this sender last set it; unknown after a reload or when something else used the channel.
    pointer: Option<i32>,
    /// Blocks the page on display sends whatever `sent` says (bit `b`: block `b`), and whether it has not had its first
    /// step yet: a page starting sends every block that shows text or that `sent` holds text in. `sent` is only what
    /// the steps believe: VRChat may reload the avatar without saying so (its cells wiped), and messages may be lost
    /// while OSC is down. Sending those blocks whole at every page puts such a panel right again at the next page.
    force: u64,
    fresh: bool,
    /// The page on display goes out at the phases' pace until it is whole (a list the owner asked for and picks from:
    /// they see it at once, everyone else as the resends that follow reach them), then at the steps' again.
    quick: bool,
    /// Until when something else (N4N0's screen) last took the channel.
    held_until: Option<Instant>,
}

/// Between a block's pointer and the next block of a quick page ([`TextSender::show_quick`]).
pub const QUICK_STEP: Duration = PHASE_GAP;

impl TextSender {
    /// A sender for a prop with `sync` text sync values (4, 8 or 16), one step every `step`.
    pub fn new(sync: usize, step: Duration) -> Self {
        Self {
            sync: if n4n0::SYNC_COUNTS.contains(&sync) { sync } else { 8 },
            step: step.max(Duration::from_millis(n4n0::MIN_STEP_MS)),
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
            force: 0,
            fresh: false,
            quick: false,
            held_until: None,
        }
    }

    pub fn sync(&self) -> usize {
        self.sync
    }

    pub fn set_step(&mut self, step: Duration) {
        self.step = step.max(Duration::from_millis(n4n0::MIN_STEP_MS));
    }

    /// Shows `text` from the next step on, replacing what was shown or waiting; blank text hides the panel.
    pub fn show(&mut self, text: &str) {
        self.sticky = false;
        self.quick = false;
        self.load(text);
    }

    /// Shows `text` as [`TextSender::show`] does, its blocks [`QUICK_STEP`] apart until it is whole: a short list the
    /// owner opened and picks from (N4N0's Call menu), seen at once by them (their avatar's parameters apply as they
    /// come). Remote viewers may miss blocks that go faster than [`n4n0::MIN_STEP_MS`]; the resends while it is up
    /// bring them, a step at a time.
    pub fn show_quick(&mut self, text: &str) {
        self.sticky = false;
        self.load(text);
        self.quick = true;
        // The first block goes now, not at the end of the step under way (unless N4N0's screen has the channel).
        if self.phase.is_none() {
            self.next_step = self.held_until.filter(|t| *t > Instant::now());
        }
    }

    /// Shows `text` and keeps its last page up until something replaces or hides it.
    pub fn show_sticky(&mut self, text: &str) {
        self.sticky = true;
        self.quick = false;
        self.load(text);
    }

    /// Whether what is up stays up (`show_sticky`).
    pub fn sticky(&self) -> bool {
        self.sticky && self.busy()
    }

    fn load(&mut self, text: &str) {
        let mut pages: VecDeque<Page> = n4n0::pages(&fold_more(text)).into();
        self.complete_at = None;
        self.resync = 0;
        self.current = pages.pop_front();
        self.queue = pages;
        self.fresh = self.current.is_some();
        self.force = 0;
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

    /// The text up and waiting (every page left, one line each), taken off the panel: it hides at the next step. N4N0's
    /// screen shows it instead while it is open.
    pub fn take_text(&mut self) -> Option<String> {
        let text: Vec<String> = self
            .current
            .iter()
            .chain(&self.queue)
            .map(|page| n4n0::page_text(page).split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .collect();
        self.hide();
        (!text.is_empty()).then(|| text.join(" "))
    }

    /// When the sender next has something to do (a block's next phase, a step), `None` when it has nothing: the
    /// N4N0 thread sleeps until then.
    pub fn next_due(&self) -> Option<Instant> {
        if let Some((at, _)) = &self.phase {
            return Some(*at);
        }
        (self.busy() || !self.known || self.clear).then(|| self.next_step.unwrap_or_else(Instant::now))
    }

    /// The avatar was (re)loaded: its panel's cells are unknown until cleared; what was being shown starts again.
    pub fn reset(&mut self) {
        self.forget();
        self.next_step = None;
    }

    /// Messages to VRChat were lost (OSC stopped, a send failed): what the panel holds is no longer known. It is
    /// cleared and what is up sent whole again, from the next step on (at the steps' pace: nothing is hurried while
    /// OSC stays down).
    pub fn forget(&mut self) {
        self.known = false;
        self.visible = false;
        self.complete_at = None;
        self.phase = None;
        self.pointer = None;
        self.fresh = self.current.is_some();
        self.force = 0;
    }

    /// Whether a step is due at `now` and would send something (a page, a clear), or a block is between its phases.
    pub fn wants_step(&self, now: Instant) -> bool {
        self.phase.is_some() || self.next_step.is_none_or(|t| now >= t) && (self.busy() || !self.known || self.clear)
    }

    /// Whether a block is between its phases: nothing else may use the text channel until it is done.
    pub fn mid_block(&self) -> bool {
        self.phase.is_some()
    }

    /// Something else uses the text channel until `at` (N4N0's screen takes its turns): no step before then, and the
    /// pointer is no longer known (the next block moves it to 0 first). A block between its phases starts again later.
    pub fn hold_until(&mut self, at: Instant) {
        self.held_until = Some(at);
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
        self.current.as_ref().map(|p| n4n0::page_text(p).lines().map(str::trim_end).collect::<Vec<_>>().join("\n"))
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
            self.next_step = Some(now + if self.quick { QUICK_STEP } else { self.step });
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
                let out = cells.iter().enumerate().map(|(i, &g)| (parameter(&params::text_sync(i)), OscType::Float(n4n0::sync_value(g)))).collect();
                self.phase = Some((now + PHASE_GAP, Phase::Pointer(block, cells)));
                out
            }
            Phase::Pointer(block, cells) => {
                let n = self.sync;
                self.sent[block * n..(block + 1) * n].copy_from_slice(&cells);
                self.force &= !(1u64 << block);
                self.next_step = Some(now + if self.quick { QUICK_STEP } else { self.step });
                // Complete once its last block went: its reading time starts now (and a quick page goes on at the
                // steps' pace, its resends for whoever missed a block).
                if let Some(page) = self.current {
                    let whole = (0..n4n0::pointer_count(n)).all(|b| page[b * n..(b + 1) * n] == self.sent[b * n..(b + 1) * n]);
                    if self.complete_at.is_none() && self.force == 0 && whole {
                        self.complete_at = Some(now);
                        self.quick = false;
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
        let pointers = n4n0::pointer_count(n);
        if std::mem::take(&mut self.fresh) {
            // A new page: every block that shows text, or that the panel should hold text in, goes whatever `sent` says.
            let blank = |cells: &[u8]| cells.iter().all(|&g| g == 0);
            self.force = (0..pointers).filter(|&b| !blank(&page[b * n..(b + 1) * n]) || !blank(&self.sent[b * n..(b + 1) * n])).fold(0, |m, b| m | 1 << b);
        }
        let differs = |sent: &Page, b: usize| page[b * n..(b + 1) * n] != sent[b * n..(b + 1) * n];
        if let Some(block) = (0..pointers).find(|&b| self.force & (1 << b) != 0 || differs(&self.sent, b)) {
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
                    self.fresh = true;
                    // The next page's first block goes at once.
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
                        self.values[i] = Some(n4n0::glyph_of_value(*v));
                        self.write(sync, i);
                    }
                }
                _ => {}
            }
        }
        visible
    }

    fn write(&mut self, sync: usize, i: usize) {
        if let (true, Some(g)) = ((1..=n4n0::pointer_count(sync) as i32).contains(&self.pointer), self.values[i]) {
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

    #[test]
    fn a_quick_page_is_whole_at_once_then_goes_on_at_the_steps_pace() {
        // N4N0's Call menu: what it says with nobody to call, and four friends with the second one picked.
        let list = "  Kasumi
> Lyra Nightfall
  Bob the Builder
  Ana";
        for text in ["No friend online", list] {
            let whole = |quick: bool| {
                let mut sender = TextSender::new(8, STEP);
                let mut panel = PanelModel::new([0; TEXT_LENGTH]);
                let start = Instant::now();
                if quick {
                    sender.show_quick(text);
                } else {
                    sender.show(text);
                }
                let want = n4n0::pages(text)[0];
                // Asked every 10 ms, as the N4N0 thread wakes for the sender's next phase.
                let mut t = start;
                while panel.cells != want {
                    let mut out = sender.tick(t);
                    out.reverse();
                    panel.apply(8, &out);
                    t += Duration::from_millis(10);
                    assert!(t < start + Duration::from_secs(20), "{text:?} never whole");
                }
                (t - start, sender)
            };
            let (slow, _) = whole(false);
            let (fast, sender) = whole(true);
            eprintln!("{text:?}: whole after {} ms, quick {} ms", slow.as_millis(), fast.as_millis());
            let blocks = n4n0::pages(text)[0].chunks(8).filter(|b| b.iter().any(|&g| g != 0)).count() as u32;
            // The clear, the first block's values and pointer, then for each other block: the step, the pointer to 0,
            // its values, its pointer.
            let most = QUICK_STEP + PHASE_GAP + (QUICK_STEP + 2 * PHASE_GAP) * (blocks - 1) + Duration::from_millis(20);
            assert!(fast <= most, "{text:?}: {fast:?} for {blocks} blocks");
            assert!(fast * 5 < slow * 3, "{text:?}: {fast:?} against {slow:?}");
            // Whole, it resends at the steps' pace again (for whoever missed a block).
            assert!(!sender.quick);
        }
    }

    fn first_row(cells: &Page) -> String {
        n4n0::page_text(cells).lines().next().unwrap().trim_end().to_string()
    }

    #[test]
    fn a_page_arrives_block_by_block_then_hides() {
        for sync in n4n0::SYNC_COUNTS {
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
        assert_eq!(n4n0::page_text(&panel.cells).lines().filter(|l| l.starts_with("ligne")).count(), 4);
        // After the first page's reading time the second replaces it, again one changed block at a time.
        let (_, _) = run(&mut sender, &mut panel, t + Duration::from_secs(5), BLOCK * 4);
        let shown: Vec<String> = n4n0::page_text(&panel.cells).lines().map(|l| l.trim_end().to_string()).collect();
        assert_eq!(shown, vec!["ligne 5", "ligne 6", "", ""]);
    }

    #[test]
    fn a_whole_page_is_resent_while_it_is_up() {
        let mut sender = TextSender::new(16, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        let text = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen";
        // Kept up (sticky) for as long as the round takes.
        sender.show_sticky(text);
        let (t, _) = run(&mut sender, &mut panel, Instant::now(), BLOCK * (1 + n4n0::pointer_count(16)) as u32);
        // Someone who loads the avatar now starts from an empty panel and gets every block within one round.
        let mut late = PanelModel::new([0; TEXT_LENGTH]);
        let mut pointers = std::collections::HashSet::new();
        let mut now = t;
        while now < t + BLOCK * n4n0::pointer_count(16) as u32 {
            let out = sender.tick(now);
            for (a, v) in &out {
                if let (true, OscType::Int(p)) = (a.ends_with(params::TEXT_POINTER), v) {
                    pointers.insert(*p);
                }
            }
            late.apply(16, &out);
            now += TICK;
        }
        assert!(pointers.len() >= n4n0::pointer_count(16));
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

    /// The blocks a sender sends from `start` until it sends one a second time (it then only resends in turn).
    fn blocks_until_a_repeat(sender: &mut TextSender, start: Instant) -> Vec<i32> {
        let mut blocks = Vec::new();
        let mut t = start;
        for _ in 0..2000 {
            for (address, value) in sender.tick(t) {
                if let (true, OscType::Int(p)) = (address.ends_with(params::TEXT_POINTER), value) {
                    if p > 0 && p != POINTER_CLEAR {
                        if blocks.contains(&p) {
                            return blocks;
                        }
                        blocks.push(p);
                    }
                }
            }
            t += TICK;
        }
        blocks
    }

    #[test]
    fn a_new_page_repairs_a_panel_that_lost_what_it_held() {
        let mut sender = TextSender::new(8, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        sender.show("Hello there, how are you today?");
        let (t, _) = run(&mut sender, &mut panel, Instant::now(), BLOCK * 5);
        assert_eq!(first_row(&panel.cells), "Hello there, how are you today?");
        // VRChat reloads the avatar without a word to Orbiters - N4N0: its panel is blank, while the sender believes it
        // still holds the text. The next text shares three of its four blocks, and still arrives whole.
        let mut wiped = PanelModel::new([0; TEXT_LENGTH]);
        sender.show("Hello there, how are you now?");
        run(&mut sender, &mut wiped, t, BLOCK * 6);
        assert_eq!(first_row(&wiped.cells), "Hello there, how are you now?");
        // A whole page that differs from the last in one block: every block with text goes before the page counts as
        // shown (and before any block is resent in turn).
        let lines = "aaaa bbbb cccc dddd eeee ffff gg\nhhhh iiii jjjj kkkk llll mmmm nn\noooo pppp qqqq rrrr ssss tttt uu\nvvvv wwww xxxx yyyy zzzz 1111 22";
        let mut sender = TextSender::new(8, STEP);
        sender.show_sticky(lines);
        let shown = blocks_until_a_repeat(&mut sender, Instant::now());
        assert_eq!(shown.len(), 16);
        sender.show_sticky(&lines.replacen("aaaa", "AAAA", 1));
        let resent = blocks_until_a_repeat(&mut sender, Instant::now() + Duration::from_secs(30));
        assert_eq!(resent, (1..=16).collect::<Vec<i32>>(), "every block of the new page, in order");
    }

    #[test]
    fn a_sender_that_lost_its_messages_sends_the_text_whole_again() {
        let mut sender = TextSender::new(8, STEP);
        let mut panel = PanelModel::new([0; TEXT_LENGTH]);
        sender.show_sticky("Hi, I'm N4N0. Yes, the one in the menu.");
        let (mut t, _) = run(&mut sender, &mut panel, Instant::now(), BLOCK * 6);
        while sender.mid_block() {
            sender.tick(t);
            t += TICK;
        }
        // OSC goes down as the next text starts: its first messages are lost, and the host says so.
        sender.show_sticky("Hi, I'm N4N0. Yes, the one in the menu!");
        let t = sender.next_due().unwrap().max(t);
        assert!(!sender.tick(t).is_empty());
        sender.forget();
        assert!(sender.tick(t + TICK).is_empty(), "the next step at its pace, not at once");
        // Back: the panel (holding who knows what) is cleared, then the whole text goes.
        let mut stale = PanelModel::new([9; TEXT_LENGTH]);
        let out = sender.tick(t + STEP);
        assert_eq!(out[0], (parameter(params::TEXT_POINTER), OscType::Int(POINTER_CLEAR)));
        stale.apply(8, &out);
        run(&mut sender, &mut stale, t + STEP + TICK, BLOCK * 6);
        assert_eq!(first_row(&stale.cells), "Hi, I'm N4N0. Yes, the one in");
    }

    #[test]
    fn letters_the_contract_cannot_place_are_placed_first() {
        assert_eq!(fold_more("Tiếng Việt, xin chào"), "Tiêng Viêt, xin chào");
        assert_eq!(fold_more("Đặng Ước"), "Đang Uoc", "Đ is the contract's to fold");
        assert_eq!(fold_more("Καλημέρα"), "Kalimera");
        // Accents written as a letter and a combining mark.
        assert_eq!(fold_more("cafe\u{0301} nai\u{0308}ve Vie\u{0323}\u{0302}t"), "café naïve Viêt");
        assert!(matches!(fold_more("Ça va ? Привет"), std::borrow::Cow::Borrowed(_)), "nothing to do: no copy");
        let page = &n4n0::pages(&fold_more("Việt"))[0];
        assert_eq!(n4n0::page_text(page).lines().next().unwrap().trim_end(), "Viêt", "every letter has a glyph");
    }

    #[test]
    fn the_screen_takes_every_page_left() {
        let mut sender = TextSender::new(8, STEP);
        assert_eq!(sender.take_text(), None);
        let text = (1..=6).map(|i| format!("ligne {i}")).collect::<Vec<_>>().join("\n");
        sender.show(&text);
        assert_eq!(sender.next_due().map(|t| t <= Instant::now()), Some(true), "a step is due");
        assert_eq!(sender.take_text().as_deref(), Some("ligne 1 ligne 2 ligne 3 ligne 4 ligne 5 ligne 6"));
        assert!(!sender.busy());
        let mut panel = PanelModel::new([5; TEXT_LENGTH]);
        let (_, visible) = run(&mut sender, &mut panel, Instant::now(), BLOCK);
        assert_eq!(visible, Some(false), "taken off the panel");
        assert_eq!(sender.next_due(), None, "then nothing to do");
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
