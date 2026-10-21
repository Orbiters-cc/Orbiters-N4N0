//! The avatar's projected screen (contracts `Docs/orb.md`, Screen): the menu as bytes in the screen's memory, and the
//! sender that writes them. The screen shows a picture of the page (rendered from [`super::layout`], part of the prop)
//! and draws over it what changes, read from its memory: [`BANK`] bytes, two to an animator value.
//!
//! The owner's screen is written through local parameters (`Orb/Screen/Pointer` and 16 values a step, as fast as
//! VRChat takes them); everyone else's through the text panel's synced channel (`Orb/Text/Pointer` past the panel's
//! blocks, `Orb/Text/Sync*`), a block every step: the header first, then the texts, then the thumbnails a pass at a time
//! (every eighth row, then every fourth...), so others see the page early and the pictures sharpen.

use super::layout::{self, Layout, ScreenPage, MAX_ELEMENTS, PICTURES};
use super::Menu;
use crate::text::Message;
use orbiters_contracts::orb::{self, params};
use rosc::OscType;

/// Bytes of the screen's memory.
pub const BANK: usize = 744;
/// The header: page, flags, hovered and pressed elements, the "on" and "hidden" masks, the thumbnails' passes.
pub const HEADER: usize = 24;
pub const PAGE: usize = 0;
pub const FLAGS: usize = 1;
/// Hovered element + 1 (0: none).
pub const HOVER: usize = 2;
/// Pressed element + 1 (0: none).
pub const PRESSED: usize = 3;
/// Elements switched on, 7 to a byte (bit `i mod 7` of byte `ON + i / 7`), elements 0 to 27.
pub const ON: usize = 4;
pub const ON_BYTES: usize = 4;
/// Elements not shown, 7 to a byte, elements 0 to 55.
pub const HIDDEN: usize = 8;
pub const HIDDEN_BYTES: usize = 8;
/// How many passes of each thumbnail the screen holds (0 to [`PASSES`]).
pub const PASS: usize = 16;
/// Where the texts start (slot offsets count from here).
pub const TEXT: usize = HEADER;
/// Where the thumbnails start: [`PICTURES`] of [`PICTURE_BYTES`].
pub const PICTURES_AT: usize = TEXT + layout::TEXT_BYTES as usize;
/// A thumbnail's size in pixels, one bit each (white 1), packed 8 to a byte, the leftmost in the highest bit.
pub const PICTURE_W: usize = 32;
pub const PICTURE_H: usize = 18;
pub const ROW_BYTES: usize = PICTURE_W / 8;
pub const PICTURE_BYTES: usize = ROW_BYTES * PICTURE_H;
/// The order rows are kept in, so each pass is a run of bytes: every eighth row, every fourth, every second, the rest.
pub const ROW_ORDER: [usize; PICTURE_H] = [0, 8, 16, 4, 12, 2, 6, 10, 14, 1, 3, 5, 7, 9, 11, 13, 15, 17];
/// How many kept rows each pass ends after.
pub const PASS_ENDS: [usize; 4] = [3, 5, 9, 18];
pub const PASSES: u8 = 4;

/// `Orb/Screen/Pointer`: write nothing.
pub const LOCAL_NONE: i32 = 0;
/// `Orb/Screen/Pointer`: clear the owner's screen memory.
pub const LOCAL_CLEAR: i32 = 255;
/// Values per local step (`Orb/Screen/Value0` …).
pub const LOCAL_VALUES: usize = 16;
/// `Orb/Text/Pointer`: the screen closed (everyone else's screen hides and clears).
pub const POINTER_SCREEN_CLOSE: i32 = 254;

const _: () = assert!(PICTURES_AT + PICTURES * PICTURE_BYTES == BANK);
const _: () = assert!(HIDDEN_BYTES * 7 >= MAX_ELEMENTS);
const _: () = assert!(PASS + PICTURES <= HEADER);

/// The screen's flags (header byte [`FLAGS`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub listening: bool,
    pub loading: bool,
    pub thinking: bool,
}

/// What the screen should hold for `menu`.
pub struct Frame {
    pub bank: [u8; BANK],
}

/// A thumbnail as the screen shows it: rows top first, [`ROW_BYTES`] each.
pub type Picture = [u8; PICTURE_BYTES];

/// A JPEG thumbnail made into a screen picture: grey, fitted, dithered to black and white.
pub fn picture_of(jpeg: &[u8]) -> anyhow::Result<Picture> {
    let grey = crate::picture::grey_jpeg(jpeg)?;
    let values = crate::picture::fit(&grey, PICTURE_W, PICTURE_H);
    let bits = crate::picture::dither(&values, PICTURE_W, PICTURE_H, 2);
    let packed = crate::picture::pack(&bits, 1);
    let mut picture = [0u8; PICTURE_BYTES];
    picture.copy_from_slice(&packed);
    Ok(picture)
}

/// Glyph indices for `text` in `cols` cells (aligned as the slot says).
fn cells(text: &str, cols: usize, align: layout::Align) -> Vec<u8> {
    let folded = orb::panel_text(text).replace('\n', " ");
    let mut glyphs: Vec<u8> = folded.chars().filter_map(orb::glyph_index).take(cols).collect();
    let pad = cols - glyphs.len();
    if align == layout::Align::Center && pad > 0 {
        let mut centered = vec![0u8; pad / 2];
        centered.append(&mut glyphs);
        glyphs = centered;
    }
    glyphs.resize(cols, 0);
    glyphs
}

/// The bytes a glyph index or a picture byte travels as: 128 has no float, so a picture byte of 128 becomes 129 (its
/// last pixel turns white).
fn sendable(byte: u8) -> u8 {
    if byte == orb::UNSENDABLE {
        orb::UNSENDABLE + 1
    } else {
        byte
    }
}

impl Frame {
    /// The screen's memory for `menu` on its page: `hover` and `pressed` are element indices of that page's layout,
    /// `pictures` the thumbnails known by video id.
    pub fn new(menu: &Menu, hover: Option<usize>, pressed: Option<usize>, flags: Flags, pictures: &dyn Fn(&str) -> Option<Picture>) -> Self {
        let mut bank = [0u8; BANK];
        if !menu.open {
            return Self { bank };
        }
        let page = ScreenPage::of(menu);
        let layout = layout::layout(page);
        bank[PAGE] = page as u8;
        bank[FLAGS] = flags.listening as u8 | (flags.loading as u8) << 1 | (flags.thinking as u8) << 2;
        bank[HOVER] = hover.filter(|&h| h < layout.elements.len()).map_or(0, |h| h as u8 + 1);
        bank[PRESSED] = pressed.filter(|&p| p < layout.elements.len()).map_or(0, |p| p as u8 + 1);
        for (i, element) in layout.elements.iter().enumerate() {
            let state = layout::state(menu, element);
            if state.on && i < ON_BYTES * 7 {
                bank[ON + i / 7] |= 1 << (i % 7);
            }
            if state.hidden {
                bank[HIDDEN + i / 7] |= 1 << (i % 7);
            }
        }
        for slot in &layout.slots {
            let text = layout::slot_text(menu, slot);
            let at = TEXT + slot.offset as usize;
            bank[at..at + slot.cols as usize].copy_from_slice(&cells(&text, slot.cols as usize, slot.align));
        }
        if page == ScreenPage::Results {
            for picture in &layout.pictures {
                let Some(video) = menu.page_results().get(picture.result as usize) else { continue };
                let Some(pixels) = pictures(&video.id) else { continue };
                let at = PICTURES_AT + picture.result as usize * PICTURE_BYTES;
                for (kept, &row) in ROW_ORDER.iter().enumerate() {
                    let from = row * ROW_BYTES;
                    let to = at + kept * ROW_BYTES;
                    for b in 0..ROW_BYTES {
                        bank[to + b] = sendable(pixels[from + b]);
                    }
                }
                bank[PASS + picture.result as usize] = PASSES;
            }
        }
        Self { bank }
    }

    /// The layout of the page this frame shows.
    pub fn layout(menu: &Menu) -> Layout {
        layout::layout(ScreenPage::of(menu))
    }
}

/// The byte ranges in the order an audience gets them: header, texts, then each pass of every thumbnail.
fn priority() -> Vec<std::ops::Range<usize>> {
    let mut out = vec![0..PASS, PASS..HEADER, TEXT..PICTURES_AT];
    let mut start = 0;
    for &end in &PASS_ENDS {
        for p in 0..PICTURES {
            let at = PICTURES_AT + p * PICTURE_BYTES;
            out.push(at + start * ROW_BYTES..at + end * ROW_BYTES);
        }
        start = end;
    }
    out
}

/// What one step of a screen channel sends (contracts `Docs/orb.md`, Screen): the pointer or the values, never both.
/// VRChat may apply values a frame or more before a pointer sent with them, which wrote a block with the next one's
/// bytes; so a block goes in three steps: the pointer to 0 (nothing written), its values, the pointer to it.
#[derive(Clone, Debug, PartialEq)]
pub enum Send {
    /// The pointer to 0: the next block's values follow.
    Rest,
    /// The values of a block (from 0), written once the pointer moves to it.
    Values(usize, Vec<u8>),
    /// The pointer to a block (from 0).
    Pointer(usize),
}

impl Send {
    /// Whether this step moves the pointer onto a block (which then stays there a whole step).
    pub fn moves_to_block(&self) -> bool {
        matches!(self, Send::Pointer(_))
    }
}

/// One audience's copy of the screen's memory, as the steps sent it: the owner (local values) or everyone else (the
/// synced text channel). Each block sent is the most urgent one that differs from what it should hold.
pub struct Audience {
    /// Bytes a step writes.
    block: usize,
    sent: [u8; BANK],
    /// Whether `sent` is known (cleared, then written); after a reload the screen's memory is not.
    known: bool,
    /// The next block resent when nothing differs.
    resync: usize,
    order: Vec<std::ops::Range<usize>>,
    /// The block whose values go next (the pointer rests at 0), and the block the pointer moves to next.
    queued: Option<usize>,
    pending: Option<usize>,
    /// The pointer rests at 0.
    resting: bool,
}

impl Audience {
    pub fn new(block: usize) -> Self {
        Self { block: block.max(2), sent: [0; BANK], known: false, resync: 0, order: priority(), queued: None, pending: None, resting: false }
    }

    pub fn blocks(&self) -> usize {
        BANK.div_ceil(self.block)
    }

    /// The screen's memory is unknown again (an avatar load, the screen reopened): it is cleared first.
    pub fn forget(&mut self) {
        self.known = false;
        self.queued = None;
        self.pending = None;
        self.resting = false;
    }

    /// Whether a block is between its steps: nothing else may use the channel until it is done.
    pub fn mid_block(&self) -> bool {
        self.queued.is_some() || self.pending.is_some()
    }

    /// The next step for `want`: the next block (a changed one by urgency, else one in turn) goes in three steps, the
    /// pointer to 0, its values, the pointer to it.
    pub fn step(&mut self, want: &[u8; BANK]) -> Send {
        if let Some(block) = self.pending.take() {
            self.resting = false;
            return Send::Pointer(block);
        }
        let block = match self.queued.take() {
            Some(block) => block,
            None => {
                let block = self.next(want).unwrap_or_else(|| self.resend());
                if !self.resting {
                    self.resting = true;
                    self.queued = Some(block);
                    return Send::Rest;
                }
                block
            }
        };
        self.pending = Some(block);
        Send::Values(block, self.bytes(block))
    }

    /// Whether the memory must be cleared before writing (and is then taken as clear).
    pub fn take_clear(&mut self) -> bool {
        if self.known {
            return false;
        }
        self.known = true;
        self.sent = [0; BANK];
        true
    }

    /// What `want` holds once the thumbnails' passes say only what this audience has whole: a pass is counted once
    /// every byte of it was sent.
    fn presentable(&self, want: &[u8; BANK]) -> [u8; BANK] {
        let mut out = *want;
        for p in 0..PICTURES {
            let at = PICTURES_AT + p * PICTURE_BYTES;
            let mut passes = 0;
            for (i, &end) in PASS_ENDS.iter().enumerate() {
                if want[PASS + p] as usize > i && self.sent[at..at + end * ROW_BYTES] == want[at..at + end * ROW_BYTES] {
                    passes = i as u8 + 1;
                } else {
                    break;
                }
            }
            out[PASS + p] = passes;
        }
        out
    }

    /// The next block to write for `want`, by urgency, and marks it sent; `None` when the screen holds it all.
    pub fn next(&mut self, want: &[u8; BANK]) -> Option<usize> {
        let want = self.presentable(want);
        let block = self.order.iter().find_map(|range| {
            let mut b = range.start / self.block;
            while b * self.block < range.end {
                let (from, to) = (b * self.block, ((b + 1) * self.block).min(BANK));
                if self.sent[from..to] != want[from..to] {
                    return Some(b);
                }
                b += 1;
            }
            None
        })?;
        let (from, to) = (block * self.block, ((block + 1) * self.block).min(BANK));
        self.sent[from..to].copy_from_slice(&want[from..to]);
        Some(block)
    }

    /// A block to write again while nothing differs, in turn (for whoever loaded the avatar since).
    pub fn resend(&mut self) -> usize {
        let block = self.resync % self.blocks();
        self.resync = (block + 1) % self.blocks();
        block
    }

    /// The bytes of `block` as this audience holds them.
    pub fn bytes(&self, block: usize) -> Vec<u8> {
        let from = block * self.block;
        (from..from + self.block).map(|i| self.sent.get(i).copied().unwrap_or(0)).collect()
    }
}

fn parameter(name: &str) -> String {
    format!("{}{name}", orbiters_core::osc::PARAMETERS_PREFIX)
}

/// The screen's local parameters (contracts `Docs/orb.md`, Screen).
pub mod local {
    use orbiters_contracts::orb::params;

    pub const OPEN: &str = params::SCREEN_OPEN;
    pub const POINTER: &str = params::SCREEN_POINTER;
    pub const VALUE_PREFIX: &str = params::SCREEN_VALUE_PREFIX;
    /// `Orb/Screen/Touch/L0` …: how near the left (`L`) or right (`R`) index finger is to each touch point (0 to 1).
    pub const TOUCH_PREFIX: &str = params::SCREEN_TOUCH_PREFIX;

    pub fn value(i: usize) -> String {
        format!("{VALUE_PREFIX}{i}")
    }
}

/// The owner's step through the local pointer (block `b` is pointer `b + 1`) or values.
pub fn local_messages(send: &Send) -> Vec<Message> {
    match send {
        Send::Rest => vec![(parameter(local::POINTER), OscType::Int(0))],
        Send::Values(_, bytes) => bytes.iter().enumerate().map(|(i, &b)| (parameter(&local::value(i)), OscType::Float(orb::sync_value(b)))).collect(),
        Send::Pointer(block) => vec![(parameter(local::POINTER), OscType::Int(*block as i32 + 1))],
    }
}

/// The owner's screen memory cleared.
pub fn local_clear() -> Vec<Message> {
    vec![(parameter(local::POINTER), OscType::Int(LOCAL_CLEAR))]
}

pub fn local_open(open: bool) -> Message {
    (parameter(local::OPEN), OscType::Bool(open))
}

/// The first `Orb/Text/Pointer` of the screen for a prop with `sync` values: right after the panel's blocks.
pub fn first_pointer(sync: usize) -> usize {
    orb::pointer_count(sync) + 1
}

/// Whether a prop with `sync` values can address the whole screen through `Orb/Text/Pointer` (pointers up to 253).
pub fn fits(sync: usize) -> bool {
    first_pointer(sync) + BANK.div_ceil(sync) - 1 < POINTER_SCREEN_CLOSE as usize
}

/// Everyone else's step through the text panel's synced pointer (block `b` is pointer `first_pointer(sync) + b`) or
/// values.
pub fn synced_messages(sync: usize, send: &Send) -> Vec<Message> {
    match send {
        Send::Rest => vec![(parameter(params::TEXT_POINTER), OscType::Int(0))],
        Send::Values(_, bytes) => {
            bytes.iter().take(sync).enumerate().map(|(i, &b)| (parameter(&params::text_sync(i)), OscType::Float(orb::sync_value(b)))).collect()
        }
        Send::Pointer(block) => vec![(parameter(params::TEXT_POINTER), OscType::Int((first_pointer(sync) + block) as i32))],
    }
}

/// Everyone else's screen closes (and clears).
pub fn synced_close() -> Vec<Message> {
    vec![(parameter(params::TEXT_POINTER), OscType::Int(POINTER_SCREEN_CLOSE))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Target, RESULTS_PER_PAGE};
    use crate::youtube::{Results, Video};

    fn results_menu() -> Menu {
        let mut menu = Menu::default();
        menu.open();
        menu.search_for("lofi");
        let videos = (0..7).map(|i| Video {
            id: format!("video{i:06}"),
            title: format!("Song {i}"),
            channel: "Lofi Girl".into(),
            duration: Some("1:00".into()),
            live: false,
        });
        menu.results_arrived("lofi", Results { videos: videos.collect(), continuation: None }, false);
        menu
    }

    fn picture(seed: u8) -> Picture {
        let mut p = [0u8; PICTURE_BYTES];
        for (i, b) in p.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(seed);
        }
        p
    }

    /// What the screen's shader reads back from a bank: a cell's glyph, a picture's pixel in a display row.
    fn pixel(bank: &[u8; BANK], picture: usize, x: usize, y: usize) -> bool {
        let passes = bank[PASS + picture] as usize;
        if passes == 0 {
            return false;
        }
        // The display row a pass shows for `y`: rounded down to the pass's step.
        let step = [8, 4, 2, 1][passes - 1];
        let shown = y / step * step;
        let kept = ROW_ORDER.iter().position(|&r| r == shown).unwrap();
        let byte = bank[PICTURES_AT + picture * PICTURE_BYTES + kept * ROW_BYTES + x / 8];
        byte >> (7 - x % 8) & 1 == 1
    }

    #[test]
    fn a_results_page_holds_its_texts_states_and_pictures() {
        let menu = results_menu();
        let frame = Frame::new(&menu, Some(1), None, Flags { listening: true, ..Default::default() }, &|id| (id == "video000001").then(|| picture(9)));
        let bank = frame.bank;
        assert_eq!(bank[PAGE], ScreenPage::Results as u8);
        assert_eq!(bank[FLAGS], 1);
        assert_eq!((bank[HOVER], bank[PRESSED]), (2, 0));
        let layout = Frame::layout(&menu);
        let title = layout.slots.iter().find(|s| s.content == layout::Content::ResultTitle(1)).unwrap();
        let at = TEXT + title.offset as usize;
        let glyphs: String = bank[at..at + 6].iter().map(|&g| orb::glyphs()[g as usize]).collect();
        assert_eq!(glyphs, "Song 1");
        // Five results on page one: nothing hidden among rows; "previous" hidden.
        let previous = layout.elements.iter().position(|e| e.target == Target::Previous).unwrap();
        assert_eq!(bank[HIDDEN + previous / 7] >> (previous % 7) & 1, 1);
        assert_eq!(bank[PASS + 1], PASSES);
        assert_eq!(bank[PASS], 0, "no thumbnail for the first result yet");
        let source = picture(9);
        for y in 0..PICTURE_H {
            for x in 0..PICTURE_W {
                let expected = source[y * ROW_BYTES + x / 8] >> (7 - x % 8) & 1 == 1;
                // 128 travels as 129: the byte's last pixel may turn white.
                let byte = source[y * ROW_BYTES + x / 8];
                if byte == 128 && x % 8 == 7 {
                    continue;
                }
                assert_eq!(pixel(&bank, 1, x, y), expected, "({x}, {y})");
            }
        }
        assert!(bank.iter().all(|&b| b != orb::UNSENDABLE), "every byte can be sent");
        assert_eq!(Frame::new(&Menu::default(), None, None, Flags::default(), &|_| None).bank, [0; BANK], "closed: empty");
        assert_eq!(RESULTS_PER_PAGE, PICTURES);
    }

    #[test]
    fn everyone_gets_header_texts_then_pictures_by_pass() {
        let menu = results_menu();
        let want = Frame::new(&menu, None, None, Flags::default(), &|_| Some(picture(3))).bank;
        let mut everyone = Audience::new(8);
        assert!(everyone.take_clear());
        assert!(!everyone.take_clear(), "once");
        let mut order = Vec::new();
        while let Some(block) = everyone.next(&want) {
            order.push(block);
            assert!(order.len() < 400, "it ends");
        }
        assert_eq!(everyone.sent, want, "everything arrives");
        assert_eq!(order[0], 0, "the header first");
        let text_blocks = (TEXT / 8)..PICTURES_AT.div_ceil(8);
        let first_picture = order.iter().position(|&b| b * 8 >= PICTURES_AT).unwrap();
        assert!(order[..first_picture].iter().filter(|&&b| text_blocks.contains(&b)).count() > 20, "texts before pictures");
        // After the first pass of every picture, the header says one pass each before any second-pass row goes.
        let pass_header = order.iter().position(|&b| b == PASS / 8).unwrap();
        assert!(pass_header > first_picture);
        // Owner: 16 bytes a step, the same content.
        let mut owner = Audience::new(LOCAL_VALUES);
        owner.take_clear();
        let mut steps = 0;
        while owner.next(&want).is_some() {
            steps += 1;
        }
        assert_eq!(owner.sent, want);
        // Every block once, and the header again as each picture's passes complete.
        assert!(steps <= BANK.div_ceil(LOCAL_VALUES) + PICTURES * PASSES as usize, "{steps} steps");
        // Nothing left: resend in turn.
        assert_eq!(owner.next(&want), None);
        assert_eq!((owner.resend(), owner.resend()), (0, 1));
    }

    #[test]
    fn messages_and_pointers() {
        assert!(fits(4) && fits(8) && fits(16));
        assert_eq!(first_pointer(8), 17);
        let pointer = format!("{}Orb/Text/Pointer", orbiters_core::osc::PARAMETERS_PREFIX);
        assert_eq!(synced_messages(8, &Send::Pointer(2)), vec![(pointer.clone(), OscType::Int(19))]);
        assert_eq!(synced_messages(8, &Send::Rest), vec![(pointer, OscType::Int(0))]);
        let m = synced_messages(8, &Send::Values(2, vec![0, 1, 255, 129, 4, 5, 6, 7]));
        assert_eq!(m.len(), 8);
        assert_eq!(m[2].1, OscType::Float(orb::sync_value(255)));
        let l = local_messages(&Send::Values(0, vec![1; LOCAL_VALUES]));
        assert_eq!(l.len(), 16);
        assert!(l[15].0.ends_with("Orb/Screen/Value15"));
        assert_eq!(local_messages(&Send::Pointer(0))[0].1, OscType::Int(1));
    }

    /// The owner's screen memory as the animator writes it: a block takes the values while the pointer is on it,
    /// whichever arrived last; pointer 0 writes nothing.
    fn write(memory: &mut [u8; BANK], pointer: &mut Option<usize>, values: &mut Vec<u8>, size: usize, send: &Send) {
        match send {
            Send::Rest => *pointer = None,
            Send::Pointer(b) => *pointer = Some(*b),
            Send::Values(_, bytes) => *values = bytes.clone(),
        }
        if let Some(b) = *pointer {
            for (i, &v) in values.iter().enumerate() {
                if b * size + i < BANK {
                    memory[b * size + i] = v;
                }
            }
        }
    }

    #[test]
    fn blocks_go_in_three_steps_that_never_mix_pointer_and_values() {
        let menu = results_menu();
        let want = Frame::new(&menu, None, None, Flags::default(), &|_| Some(picture(3))).bank;
        for size in [8, LOCAL_VALUES] {
            let mut audience = Audience::new(size);
            audience.take_clear();
            let (mut memory, mut pointer, mut values) = ([0u8; BANK], None, Vec::new());
            let mut kinds = Vec::new();
            for _ in 0..3 * 400 {
                let send = audience.step(&want);
                kinds.push(std::mem::discriminant(&send));
                write(&mut memory, &mut pointer, &mut values, size, &send);
            }
            assert_eq!(memory, want, "{size} bytes a block");
            assert!(!audience.mid_block() || kinds.len() % 3 != 0);
            // Rest, values, pointer, in turn.
            assert_eq!(kinds[0], std::mem::discriminant(&Send::Rest));
            assert_eq!(kinds[1], std::mem::discriminant(&Send::Values(0, Vec::new())));
            assert_eq!(kinds[2], std::mem::discriminant(&Send::Pointer(0)));
        }
    }
}
