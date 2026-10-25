//! The avatar's projected screen (contracts `Docs/orb.md`, Screen): the menu as bytes in the screen's memory, and the
//! sender that writes them. The screen shows a picture of the page (rendered from [`super::layout`], part of the prop)
//! and draws over it what changes, read from its memory: [`bank_bytes`] bytes (744, or 1024 with 128-byte thumbnails),
//! two to an animator value.
//!
//! The owner's screen is written through local parameters (`N4N0/Screen/Pointer` and 16 values a step, as fast as
//! VRChat takes them); everyone else's through the text panel's synced channel (`N4N0/Text/Pointer` past the panel's
//! blocks, `N4N0/Text/Sync*`), a block every step: the header first, then the texts, then the thumbnails a layer at a time
//! (contracts `orb::thumbnail`: a blur first, sharper with each layer), so others see the page early and the pictures
//! sharpen.
//!
//! Only the bytes a page reads are ever sent for it, and resent in turn; a new page clears the memory first, so none of
//! the last page's text shows in the new one's slots. Everyone else's frame ([`Frame::public`]) carries no hover nor
//! press and names none of the owner's friends.

use super::layout::{self, Layout, ScreenPage, MAX_ELEMENTS, PICTURES};
use super::Menu;
use crate::text::Message;
use orbiters_contracts::orb::{self, params, thumbnail};
use rosc::OscType;

/// The largest screen memory (128-byte thumbnails): every memory here is this long, its bytes past the avatar's
/// [`bank_bytes`] never read nor sent.
pub const BANK: usize = bank_bytes(thumbnail::MAX_BYTES);
/// The header: page, flags, hovered and pressed elements, the "on", "hidden" and "unavailable" masks, the thumbnails'
/// layer counts.
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
/// How many layers of each thumbnail the screen holds (0 to 3, or 4 with 128-byte thumbnails).
pub const LAYER_COUNTS: usize = 16;
/// Elements greyed out (not available now: fast walk in desktop mode), 7 to a byte, elements 0 to 20.
pub const UNAVAILABLE: usize = 21;
pub const UNAVAILABLE_BYTES: usize = 3;
/// Where the texts start (slot offsets count from here).
pub const TEXT: usize = HEADER;
/// Where the thumbnails start: [`PICTURES`] of the avatar's thumbnail size (contracts `orb::thumbnail::SIZES`).
pub const PICTURES_AT: usize = TEXT + layout::TEXT_BYTES as usize;

/// The screen's memory with thumbnails of `size` bytes: 744 with 72, 1024 with 128.
pub const fn bank_bytes(size: usize) -> usize {
    PICTURES_AT + PICTURES * size
}

/// `N4N0/Screen/Pointer`: write nothing.
pub const LOCAL_NONE: i32 = 0;
/// `N4N0/Screen/Pointer`: clear the owner's screen memory.
pub const LOCAL_CLEAR: i32 = 255;
/// Values per local step (`N4N0/Screen/Value0` …).
pub const LOCAL_VALUES: usize = 16;
/// `N4N0/Text/Pointer`: the screen closed (everyone else's screen hides and clears).
pub const POINTER_SCREEN_CLOSE: i32 = orb::POINTER_SCREEN_CLOSE;
/// `N4N0/Text/Pointer`: everyone else's screen is a dropped link card, frozen where it is until the screen closes.
pub const POINTER_SCREEN_DROP: i32 = orb::POINTER_SCREEN_DROP;

const _: () = assert!(bank_bytes(72) == 744 && BANK == 1024);
const _: () = assert!(HIDDEN_BYTES * 7 >= MAX_ELEMENTS);
const _: () = assert!(LAYER_COUNTS + PICTURES <= HEADER);

/// The screen's flags (header byte [`FLAGS`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub listening: bool,
    pub loading: bool,
    pub thinking: bool,
}

/// What the screen should hold for `menu`: [`bank_bytes`] of its thumbnails' size (the rest stays 0).
pub struct Frame {
    pub bank: [u8; BANK],
}

/// A thumbnail as the screen shows it (contracts `orb::thumbnail`), in both sizes a prop may keep.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    small: Vec<u8>,
    large: Vec<u8>,
}

impl Picture {
    /// From a 64 × 36 picture (sRGB values 0 to 1, rows top first).
    pub fn new(picture: &[[f32; 3]]) -> Self {
        Self { small: thumbnail::encode(picture, 72), large: thumbnail::encode(picture, 128) }
    }

    /// The memory of a thumbnail of `size` bytes (72 or 128).
    pub fn bytes(&self, size: usize) -> &[u8] {
        if size >= thumbnail::MAX_BYTES {
            &self.large
        } else {
            &self.small
        }
    }
}

/// A JPEG thumbnail made into a screen picture: fitted to 64 × 36 (a cover, cropped at its middle) and encoded.
pub fn picture_of(jpeg: &[u8]) -> anyhow::Result<Picture> {
    Ok(picture_from_rgb(&crate::picture::rgb_jpeg(jpeg)?))
}

pub fn picture_from_rgb(rgb: &crate::picture::Rgb) -> Picture {
    Picture::new(&crate::picture::fit(rgb, thumbnail::WIDTH, thumbnail::HEIGHT))
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

impl Frame {
    /// The owner's screen memory for `menu` on its page: `hover` and `pressed` are element indices of that page's
    /// layout, `size` the bytes of each thumbnail the prop keeps, `pictures` the thumbnails known by key (a video id,
    /// the music's art key: [`layout::picture_key`]).
    pub fn new(menu: &Menu, hover: Option<usize>, pressed: Option<usize>, flags: Flags, size: usize, pictures: &dyn Fn(&str) -> Option<Picture>) -> Self {
        Self::build(menu, hover, pressed, flags, size, pictures, false)
    }

    /// Everyone else's screen memory for `menu`: no hover nor press (they changed with every move of the owner's finger
    /// and held the rest back, one block a step), and the owner's friends never named ([`layout::text_for`]).
    pub fn public(menu: &Menu, flags: Flags, size: usize, pictures: &dyn Fn(&str) -> Option<Picture>) -> Self {
        Self::build(menu, None, None, flags, size, pictures, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        menu: &Menu,
        hover: Option<usize>,
        pressed: Option<usize>,
        flags: Flags,
        size: usize,
        pictures: &dyn Fn(&str) -> Option<Picture>,
        public: bool,
    ) -> Self {
        let mut bank = [0u8; BANK];
        if !menu.open {
            return Self { bank };
        }
        let page = ScreenPage::of(menu);
        let layout = layout::cached(page);
        bank[PAGE] = page as u8;
        bank[FLAGS] = flags.listening as u8 | (flags.loading as u8) << 1 | (flags.thinking as u8) << 2;
        bank[HOVER] = hover.filter(|&h| h < layout.elements.len()).map_or(0, |h| h as u8 + 1);
        bank[PRESSED] = pressed.filter(|&p| p < layout.elements.len()).map_or(0, |p| p as u8 + 1);
        for (i, element) in layout.elements.iter().enumerate() {
            let state = layout::state_for(menu, element, public);
            if state.on && i < ON_BYTES * 7 {
                bank[ON + i / 7] |= 1 << (i % 7);
            }
            if state.hidden {
                bank[HIDDEN + i / 7] |= 1 << (i % 7);
            }
            if state.unavailable && i < UNAVAILABLE_BYTES * 7 {
                bank[UNAVAILABLE + i / 7] |= 1 << (i % 7);
            }
        }
        for slot in &layout.slots {
            let text = layout::slot_text_for(menu, slot, public);
            let at = TEXT + slot.offset as usize;
            bank[at..at + slot.cols as usize].copy_from_slice(&cells(&text, slot.cols as usize, slot.align));
        }
        // The results' thumbnails, the music's art.
        for picture in &layout.pictures {
            let Some(thumb) = layout::picture_key(menu, picture).and_then(pictures) else { continue };
            let at = PICTURES_AT + picture.result as usize * size;
            bank[at..at + size].copy_from_slice(thumb.bytes(size));
            bank[LAYER_COUNTS + picture.result as usize] = thumbnail::layers_in(size) as u8;
        }
        Self { bank }
    }

    /// The layout of the page this frame shows.
    pub fn layout(menu: &Menu) -> &'static Layout {
        layout::cached(ScreenPage::of(menu))
    }
}

/// The byte ranges in the order an audience gets them again: header, texts, then each layer of every thumbnail.
fn priority(size: usize) -> Vec<std::ops::Range<usize>> {
    let mut out = vec![0..LAYER_COUNTS, LAYER_COUNTS..HEADER, TEXT..PICTURES_AT];
    for layer in 1..=thumbnail::layers_in(size) {
        out.extend(layer_bytes(size, layer));
    }
    out
}

/// Where layer `layer` (from 1) of each thumbnail of `size` bytes ends, in bytes from its first.
fn layer_end(layer: usize) -> usize {
    thumbnail::LAYERS[layer - 1]
}

/// The bytes of layer `layer` (from 1) of every thumbnail of `size` bytes.
fn layer_bytes(size: usize, layer: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
    let start = if layer > 1 { layer_end(layer - 1) } else { 0 };
    (0..PICTURES).map(move |p| {
        let at = PICTURES_AT + p * size;
        at + start..at + layer_end(layer)
    })
}

/// The bytes page `page` reads (contracts `Docs/orb.md`, Memory) with thumbnails of `size` bytes: the header, the
/// page's text slots and, on the results page, the thumbnails. The rest of the memory is never sent for that page (its
/// bytes count as equal whatever they hold), nor resent. Page 0 (nothing shown) reads the header only.
fn visible(page: u8, size: usize) -> &'static [bool; BANK] {
    type Masks = Vec<[bool; BANK]>;
    static MASKS: std::sync::OnceLock<[Masks; 2]> = std::sync::OnceLock::new();
    let masks = MASKS.get_or_init(|| {
        thumbnail::SIZES.map(|size| {
            let mut header = [false; BANK];
            header[..HEADER].fill(true);
            let mut masks = vec![header; ScreenPage::ALL.len() + 1];
            for page in ScreenPage::ALL {
                let l = layout::cached(page);
                let mask = &mut masks[page as usize];
                for s in &l.slots {
                    let at = TEXT + s.offset as usize;
                    mask[at..at + s.cols as usize].fill(true);
                }
                // The thumbnails the page has: the results' five, the music's art, a card's picture.
                for p in &l.pictures {
                    let at = PICTURES_AT + p.result as usize * size;
                    mask[at..at + size].fill(true);
                }
            }
            masks
        })
    });
    let masks = &masks[usize::from(size >= thumbnail::MAX_BYTES)];
    masks.get(page as usize).unwrap_or(&masks[0])
}

/// While nothing differs, the header goes again every this many blocks (for whoever just loaded the avatar: the
/// page's picture shows at once, its texts follow).
const HEADER_EVERY: usize = 6;

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
    /// The memory cleared (the owner's pointer 255; everyone else's 254, which also closes their screen until the next
    /// block): a new page starts from nothing, so none of the last page's text shows in its slots.
    Clear,
}

impl Send {
    /// Whether this step moves the pointer onto a block (which then stays there a whole step).
    pub fn moves_to_block(&self) -> bool {
        matches!(self, Send::Pointer(_))
    }

    /// Whether the pointer stays as this step set it a whole step: on a block, or clearing.
    pub fn holds(&self) -> bool {
        matches!(self, Send::Pointer(_) | Send::Clear)
    }
}

/// One audience's copy of the screen's memory, as the steps sent it: the owner (local values) or everyone else (the
/// synced text channel). Each block sent is the most urgent one that differs from what it should hold, among the
/// bytes its page reads; when none does, the page's blocks go again in turn.
pub struct Audience {
    /// Bytes a step writes.
    block: usize,
    /// The bytes of each thumbnail the prop keeps (72 or 128): its memory is [`bank_bytes`] of it.
    size: usize,
    sent: [u8; BANK],
    /// Whether `sent` is known (cleared, then written); after a reload the screen's memory is not.
    known: bool,
    /// The next of the page's blocks resent when nothing differs, and how many went since the header did.
    resync: usize,
    since_header: usize,
    /// The blocks the page reads, in the order they are resent, and the page they are for.
    page_blocks: (u8, Vec<usize>),
    order: Vec<std::ops::Range<usize>>,
    /// The block whose values go next (the pointer rests at 0), and the block the pointer moves to next.
    queued: Option<usize>,
    pending: Option<usize>,
    /// The pointer rests at 0.
    resting: bool,
    /// The block in its steps (or the last one) is a resend: nothing differed when it was picked.
    resending: bool,
}

impl Audience {
    pub fn new(block: usize, size: usize) -> Self {
        Self {
            block: block.max(2),
            size,
            sent: [0; BANK],
            known: false,
            resync: 0,
            since_header: 0,
            page_blocks: (0, Vec::new()),
            order: priority(size),
            queued: None,
            pending: None,
            resting: false,
            resending: false,
        }
    }

    pub fn blocks(&self) -> usize {
        bank_bytes(self.size).div_ceil(self.block)
    }

    /// The screen's memory is unknown again (an avatar load, the screen reopened): it is cleared first.
    pub fn forget(&mut self) {
        self.known = false;
        self.queued = None;
        self.pending = None;
        self.resting = false;
        self.resending = false;
    }

    /// The screen's memory is clear without a step saying so (closing cleared everyone else's; a loaded avatar starts
    /// clear). `pointer_writes_nothing`: the pointer was left where it writes nothing (closed), so a block's values
    /// may go first.
    pub fn cleared(&mut self, pointer_writes_nothing: bool) {
        self.known = true;
        self.sent = [0; BANK];
        self.queued = None;
        self.pending = None;
        self.resting = pointer_writes_nothing;
        self.resending = false;
    }

    /// The pointer was moved where it writes nothing (a dropped card's freeze, 253), a block's steps not under way: the
    /// next block's values may go first.
    pub fn park(&mut self) {
        if !self.mid_block() {
            self.resting = true;
        }
    }

    /// Whether a block is between its steps: nothing else may use the channel until it is done.
    pub fn mid_block(&self) -> bool {
        self.queued.is_some() || self.pending.is_some()
    }

    /// Whether the block in its steps, or the last one, was sent again only because nothing differed.
    pub fn resending(&self) -> bool {
        self.resending
    }

    /// Whether the memory holds all of `want` its page reads, no block being in its steps: what goes next would only
    /// be sent again.
    pub fn settled(&self, want: &[u8; BANK]) -> bool {
        self.known && !self.mid_block() && self.differing(&self.presentable(want)).is_none()
    }

    /// The next step for `want`: a new page's clear, else the next block (a changed one by urgency, else one in turn)
    /// in three steps, the pointer to 0, its values, the pointer to it.
    pub fn step(&mut self, want: &[u8; BANK]) -> Send {
        // A new page: the memory is cleared first (a block between its steps with it), then the header goes.
        if self.known && self.sent[PAGE] != 0 && want[PAGE] != self.sent[PAGE] {
            self.known = false;
        }
        if !self.known {
            // Clearing writes nothing: the next block's values may go at once.
            self.cleared(true);
            return Send::Clear;
        }
        if let Some(block) = self.pending.take() {
            self.resting = false;
            return Send::Pointer(block);
        }
        let block = match self.queued.take() {
            Some(block) => block,
            None => {
                let next = self.next(want);
                self.resending = next.is_none();
                let block = next.unwrap_or_else(|| self.resend(want[PAGE]));
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

    /// What `want` holds once the thumbnails' layer counts say only what this audience has whole: a layer is counted
    /// once every byte of it was sent.
    fn presentable(&self, want: &[u8; BANK]) -> [u8; BANK] {
        let mut out = *want;
        for p in 0..PICTURES {
            let at = PICTURES_AT + p * self.size;
            let mut layers = 0;
            for layer in 1..=thumbnail::layers_in(self.size) {
                let end = at + layer_end(layer);
                if want[LAYER_COUNTS + p] as usize >= layer && self.sent[at..end] == want[at..end] {
                    layers = layer as u8;
                } else {
                    break;
                }
            }
            out[LAYER_COUNTS + p] = layers;
        }
        out
    }

    /// The most urgent block whose bytes the page reads differ from `want` (already [`Self::presentable`]): the header,
    /// the texts, then the thumbnails a layer at a time, the layers' count once a layer of them all went (one block
    /// saying so for the five, rather than one per thumbnail).
    fn differing(&self, want: &[u8; BANK]) -> Option<usize> {
        let mask = visible(want[PAGE], self.size);
        // The first block holding a byte of `range` (only those count) that differs.
        let first = |range: std::ops::Range<usize>| {
            let mut b = range.start / self.block;
            while b * self.block < range.end {
                let (from, to) = ((b * self.block).max(range.start), ((b + 1) * self.block).min(range.end));
                if (from..to).any(|i| mask[i] && self.sent[i] != want[i]) {
                    return Some(b);
                }
                b += 1;
            }
            None
        };
        let counted = |level: u8| (0..PICTURES).any(|p| mask[LAYER_COUNTS + p] && self.sent[LAYER_COUNTS + p] < want[LAYER_COUNTS + p].min(level));
        // A thumbnail that changed hides first (its layers' count down), so its old layers never show with the new.
        if (0..PICTURES).any(|p| mask[LAYER_COUNTS + p] && want[LAYER_COUNTS + p] < self.sent[LAYER_COUNTS + p]) {
            return Some(LAYER_COUNTS / self.block);
        }
        if let Some(b) = first(0..LAYER_COUNTS).or_else(|| first(TEXT..PICTURES_AT)) {
            return Some(b);
        }
        for layer in 1..=thumbnail::layers_in(self.size) {
            if let Some(b) = layer_bytes(self.size, layer).find_map(&first) {
                return Some(b);
            }
            if counted(layer as u8) {
                return Some(LAYER_COUNTS / self.block);
            }
        }
        first(LAYER_COUNTS..HEADER)
    }

    /// The next block to write for `want`, by urgency, and marks it sent; `None` when the screen holds all its page
    /// reads. The block's other bytes are sent as the memory holds them.
    pub fn next(&mut self, want: &[u8; BANK]) -> Option<usize> {
        let want = self.presentable(want);
        let block = self.differing(&want)?;
        let mask = visible(want[PAGE], self.size);
        for i in block * self.block..((block + 1) * self.block).min(BANK) {
            if mask[i] {
                self.sent[i] = want[i];
            }
        }
        Some(block)
    }

    /// A block to write again while nothing differs (for whoever loaded the avatar since, and to repair a block
    /// VRChat dropped): page `page`'s blocks in turn, its texts before its pictures, the header every [`HEADER_EVERY`].
    pub fn resend(&mut self, page: u8) -> usize {
        if self.page_blocks.0 != page || self.page_blocks.1.is_empty() {
            let mask = visible(page, self.size);
            let mut blocks: Vec<usize> = Vec::new();
            for range in &self.order {
                for b in range.start / self.block..range.end.div_ceil(self.block) {
                    let read = (b * self.block..((b + 1) * self.block).min(BANK)).any(|i| mask[i]);
                    if read && !blocks.contains(&b) {
                        blocks.push(b);
                    }
                }
            }
            self.page_blocks = (page, blocks);
            self.resync = 0;
        }
        self.since_header += 1;
        let blocks = &self.page_blocks.1;
        let block = if self.since_header >= HEADER_EVERY {
            0
        } else {
            let block = blocks[self.resync % blocks.len()];
            self.resync = (self.resync + 1) % blocks.len();
            block
        };
        if block == 0 {
            self.since_header = 0;
        }
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
    /// The owner's screen is a dropped link card (from version 5): it shows, frozen, whatever else says.
    pub const DROP: &str = params::SCREEN_DROP;
    pub const POINTER: &str = params::SCREEN_POINTER;
    pub const VALUE_PREFIX: &str = params::SCREEN_VALUE_PREFIX;
    /// `N4N0/Screen/Touch/L0` …: how near the left (`L`) or right (`R`) index finger is to each touch point (0 to 1).
    pub const TOUCH_PREFIX: &str = params::SCREEN_TOUCH_PREFIX;
    /// `N4N0/Screen/Eye0` … and `N4N0/Screen/Gaze0` …: how near the eyes and the gaze point are to each touch point
    /// (desktop mode's looking).
    pub const EYE_PREFIX: &str = params::SCREEN_EYE_PREFIX;
    pub const GAZE_PREFIX: &str = params::SCREEN_GAZE_PREFIX;
    /// `N4N0/Screen/Ray/L_Hit`, `…_Ratio` and the right hand's: the pointing rays (screen version 4).
    pub const RAY_PREFIX: &str = params::SCREEN_RAY_PREFIX;

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
        Send::Clear => local_clear(),
    }
}

/// The owner's screen memory cleared.
pub fn local_clear() -> Vec<Message> {
    vec![(parameter(local::POINTER), OscType::Int(LOCAL_CLEAR))]
}

pub fn local_open(open: bool) -> Message {
    (parameter(local::OPEN), OscType::Bool(open))
}

/// The owner's screen is a dropped link card (`N4N0/Screen/Drop`), or no longer.
pub fn local_drop(dropped: bool) -> Message {
    (parameter(local::DROP), OscType::Bool(dropped))
}

/// The first `N4N0/Text/Pointer` of the screen for a prop with `sync` values: right after the panel's blocks.
pub fn first_pointer(sync: usize) -> usize {
    orb::pointer_count(sync) + 1
}

/// Whether a prop with `sync` values and thumbnails of `size` bytes can address the whole screen through
/// `N4N0/Text/Pointer` (pointers up to 252: 253 freezes a dropped card, 254 closes the screen): 128-byte thumbnails need
/// 8 values or more.
pub fn fits(sync: usize, size: usize) -> bool {
    thumbnail::SIZES.contains(&size) && first_pointer(sync) + bank_bytes(size).div_ceil(sync) - 1 < POINTER_SCREEN_DROP as usize
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
        Send::Clear => synced_close(),
    }
}

/// Everyone else's screen closes (and clears).
pub fn synced_close() -> Vec<Message> {
    vec![(parameter(params::TEXT_POINTER), OscType::Int(POINTER_SCREEN_CLOSE))]
}

/// Everyone else's screen is a dropped link card: it freezes where it is (until it closes) and can be grabbed.
pub fn synced_drop() -> Vec<Message> {
    vec![(parameter(params::TEXT_POINTER), OscType::Int(POINTER_SCREEN_DROP))]
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
        menu.results_arrived(1, Results { videos: videos.collect(), continuation: None }, false);
        menu
    }

    fn picture(seed: u8) -> Picture {
        let pixels: Vec<[f32; 3]> = (0..thumbnail::WIDTH * thumbnail::HEIGHT)
            .map(|i| {
                let (x, y) = ((i % thumbnail::WIDTH) as f32 / 64.0, (i / thumbnail::WIDTH) as f32 / 36.0);
                [(x + seed as f32 * 0.1).fract(), y, ((x * 3.0 + y) * 0.5 + seed as f32 * 0.07).fract()]
            })
            .collect();
        Picture::new(&pixels)
    }

    #[test]
    fn a_results_page_holds_its_texts_states_and_pictures() {
        let menu = results_menu();
        let thumb = picture(9);
        let frame = Frame::new(&menu, Some(1), None, Flags { listening: true, ..Default::default() }, 72, &|id| (id == "video000001").then(|| thumb.clone()));
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
        assert_eq!(bank[LAYER_COUNTS + 1], 3, "72 bytes: three layers");
        assert_eq!(bank[LAYER_COUNTS], 0, "no thumbnail for the first result yet");
        assert_eq!(&bank[PICTURES_AT + 72..PICTURES_AT + 144], thumb.bytes(72), "result 1's thumbnail, as encoded");
        assert!(bank[bank_bytes(72)..].iter().all(|&b| b == 0), "nothing past 744 bytes");
        assert!(bank.iter().all(|&b| b != orb::UNSENDABLE), "every byte can be sent");
        // 128-byte thumbnails: four layers, result 1 from byte 384 + 128.
        let large = Frame::new(&menu, None, None, Flags::default(), 128, &|id| (id == "video000001").then(|| thumb.clone())).bank;
        assert_eq!(large[LAYER_COUNTS + 1], 4);
        assert_eq!(&large[PICTURES_AT + 128..PICTURES_AT + 256], thumb.bytes(128));
        assert_eq!(Frame::new(&Menu::default(), None, None, Flags::default(), 72, &|_| None).bank, [0; BANK], "closed: empty");
        assert_eq!(RESULTS_PER_PAGE, PICTURES);
    }

    #[test]
    fn everyone_gets_header_texts_then_pictures_by_layer() {
        let menu = results_menu();
        let thumb = picture(3);
        let want = Frame::new(&menu, None, None, Flags::default(), 72, &|_| Some(thumb.clone())).bank;
        let mut everyone = Audience::new(8, 72);
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
        // After the first layer of every picture, the header says one layer each before any second-layer byte goes.
        let layer_header = order.iter().position(|&b| b == LAYER_COUNTS / 8).unwrap();
        assert!(layer_header > first_picture);
        let first_layer: Vec<usize> = layer_bytes(72, 1).map(|r| r.start / 8).collect();
        assert!(first_layer.iter().all(|b| order[..layer_header].contains(b)), "every first layer before the count");
        // Owner: 16 bytes a step, the same content.
        let mut owner = Audience::new(LOCAL_VALUES, 72);
        owner.take_clear();
        let mut steps = 0;
        while owner.next(&want).is_some() {
            steps += 1;
        }
        assert_eq!(owner.sent, want);
        // Every block once (some twice: a 16-byte block may hold two thumbnails' layers), and the layers' count again once
        // per layer of them all (not once per thumbnail).
        assert!(steps <= bank_bytes(72).div_ceil(LOCAL_VALUES) + 2 * PICTURES + 3, "{steps} steps");
        // Nothing left: resend in turn.
        assert_eq!(owner.next(&want), None);
        assert!(owner.settled(&want));
        assert_eq!((owner.resend(want[PAGE]), owner.resend(want[PAGE])), (0, 1));
    }

    #[test]
    fn a_page_s_unread_bytes_are_never_sent_and_the_header_comes_back_often() {
        // The results page sent whole, then Home: its slots and header only.
        let mut menu = results_menu();
        let mut everyone = Audience::new(8, 72);
        everyone.cleared(false);
        let thumb = picture(3);
        let results = Frame::public(&menu, Flags::default(), 72, &|_| Some(thumb.clone())).bank;
        while everyone.next(&results).is_some() {}
        menu.press(Target::Back);
        menu.press(Target::Back);
        let home = Frame::public(&menu, Flags::default(), 72, &|_| None).bank;
        assert_eq!(home[PAGE], ScreenPage::Home as u8);
        assert_eq!(everyone.step(&home), Send::Clear, "a new page clears first");
        let mut blocks = 0;
        while everyone.next(&home).is_some() {
            blocks += 1;
        }
        let mask = visible(home[PAGE], 72);
        let read_blocks = (0..BANK.div_ceil(8)).filter(|b| (b * 8..((b + 1) * 8).min(BANK)).any(|i| mask[i])).count();
        assert!(blocks <= read_blocks && read_blocks < 30, "{blocks} blocks for Home's {read_blocks}");
        assert!(everyone.sent[PICTURES_AT..].iter().all(|&b| b == 0), "no thumbnail byte for Home");
        // Nothing differs: the page's blocks in turn, the header among them every few.
        let resent: Vec<usize> = (0..60).map(|_| everyone.resend(home[PAGE])).collect();
        assert!(resent.iter().all(|&b| (b * 8..((b + 1) * 8).min(BANK)).any(|i| mask[i])), "only blocks Home reads: {resent:?}");
        assert!(resent.windows(HEADER_EVERY).all(|w| w.contains(&0)), "{resent:?}");
    }

    #[test]
    fn everyone_else_never_gets_the_owner_s_finger_or_friends() {
        let mut menu = Menu::default();
        menu.open();
        menu.set_contacts(vec![crate::menu::Contact { id: "u1".into(), name: "Lyra".into() }], true, "");
        menu.press(Target::Call);
        let owner = Frame::new(&menu, Some(3), Some(4), Flags::default(), 72, &|_| None).bank;
        let public = Frame::public(&menu, Flags::default(), 72, &|_| None).bank;
        assert_eq!((owner[HOVER], owner[PRESSED]), (4, 5));
        assert_eq!((public[HOVER], public[PRESSED]), (0, 0), "no hover: block 0 stays put while the finger moves");
        let lyra: Vec<u8> = "Lyra".chars().map(|c| orb::glyph_index(c).unwrap()).collect();
        assert!(owner.windows(4).any(|w| w == lyra));
        assert!(!public.windows(4).any(|w| w == lyra), "no friend's name for everyone else");
    }

    #[test]
    fn a_changed_thumbnail_hides_before_its_layers_change() {
        let menu = results_menu();
        let (a, b) = (picture(3), picture(4));
        let first = Frame::new(&menu, None, None, Flags::default(), 128, &|_| Some(a.clone())).bank;
        let mut owner = Audience::new(LOCAL_VALUES, 128);
        owner.take_clear();
        while owner.next(&first).is_some() {}
        let second = Frame::new(&menu, None, None, Flags::default(), 128, &|_| Some(b.clone())).bank;
        assert_eq!(owner.next(&second), Some(LAYER_COUNTS / LOCAL_VALUES), "the layers' count first");
        assert!((0..PICTURES).all(|p| owner.sent[LAYER_COUNTS + p] == 0));
        while owner.next(&second).is_some() {}
        assert_eq!(owner.sent, second, "then the new pictures, layer by layer");
    }

    #[test]
    fn messages_and_pointers() {
        assert!(fits(4, 72) && fits(8, 72) && fits(16, 72));
        assert!(!fits(4, 128) && fits(8, 128) && fits(16, 128), "1024 bytes in blocks of 4 would pass pointer 253");
        assert!(!fits(8, 100));
        assert_eq!(first_pointer(8), 17);
        let pointer = format!("{}N4N0/Text/Pointer", orbiters_core::osc::PARAMETERS_PREFIX);
        assert_eq!(synced_messages(8, &Send::Pointer(2)), vec![(pointer.clone(), OscType::Int(19))]);
        assert_eq!(synced_messages(8, &Send::Rest), vec![(pointer, OscType::Int(0))]);
        let m = synced_messages(8, &Send::Values(2, vec![0, 1, 255, 129, 4, 5, 6, 7]));
        assert_eq!(m.len(), 8);
        assert_eq!(m[2].1, OscType::Float(orb::sync_value(255)));
        let l = local_messages(&Send::Values(0, vec![1; LOCAL_VALUES]));
        assert_eq!(l.len(), 16);
        assert!(l[15].0.ends_with("N4N0/Screen/Value15"));
        assert_eq!(local_messages(&Send::Pointer(0))[0].1, OscType::Int(1));
    }

    /// The owner's screen memory as the animator writes it: a block takes the values while the pointer is on it,
    /// whichever arrived last; pointer 0 writes nothing.
    fn write(memory: &mut [u8; BANK], pointer: &mut Option<usize>, values: &mut Vec<u8>, size: usize, send: &Send) {
        match send {
            Send::Rest => *pointer = None,
            Send::Pointer(b) => *pointer = Some(*b),
            Send::Values(_, bytes) => *values = bytes.clone(),
            Send::Clear => {
                *pointer = None;
                *memory = [0; BANK];
            }
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
        let thumb = picture(3);
        for (size, thumbnails) in [(8, 72), (LOCAL_VALUES, 72), (8, 128), (LOCAL_VALUES, 128)] {
            let want = Frame::new(&menu, None, None, Flags::default(), thumbnails, &|_| Some(thumb.clone())).bank;
            let mut audience = Audience::new(size, thumbnails);
            audience.take_clear();
            let (mut memory, mut pointer, mut values) = ([0u8; BANK], None, Vec::new());
            let mut kinds = Vec::new();
            for _ in 0..3 * 400 {
                let send = audience.step(&want);
                kinds.push(std::mem::discriminant(&send));
                write(&mut memory, &mut pointer, &mut values, size, &send);
            }
            assert_eq!(memory, want, "{size} bytes a block, {thumbnails}-byte thumbnails");
            assert!(!audience.mid_block() || kinds.len() % 3 != 0);
            // Rest, values, pointer, in turn.
            assert_eq!(kinds[0], std::mem::discriminant(&Send::Rest));
            assert_eq!(kinds[1], std::mem::discriminant(&Send::Values(0, Vec::new())));
            assert_eq!(kinds[2], std::mem::discriminant(&Send::Pointer(0)));
        }
    }

    #[test]
    fn a_dropped_card_s_countdown_moves_in_steps_for_everyone_else() {
        use crate::menu::{Card, Pages, CARD_SEGMENTS};
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.show_card(Card { link: "https://www.youtube.com/watch?v=video000001".into(), title: "Song".into(), video: Some("video000001".into()), left: 13 });
        let thumb = picture(5);
        let owner = Frame::new(&menu, None, None, Flags::default(), 72, &|_| Some(thumb.clone())).bank;
        let public = Frame::public(&menu, Flags::default(), 72, &|_| Some(thumb.clone())).bank;
        assert_eq!(owner[PAGE], ScreenPage::Card as u8);
        let hidden = |bank: &[u8; BANK]| (0..CARD_SEGMENTS).filter(|&i| bank[HIDDEN + i / 7] >> (i % 7) & 1 == 1).count();
        assert_eq!((hidden(&owner), hidden(&public)), (2, 0), "13 s left: the owner's bar shows 13, everyone else's 15");
        assert_eq!(owner[LAYER_COUNTS], 3);
        assert_eq!(&owner[PICTURES_AT..PICTURES_AT + 72], thumb.bytes(72));
        // Only the page's own picture is read: the other thumbnails' places are never sent for it, nor resent.
        let mask = visible(owner[PAGE], 72);
        assert!(mask[PICTURES_AT] && !mask[PICTURES_AT + 72] && !mask[bank_bytes(72) - 1]);
        let music = visible(ScreenPage::Music as u8, 72);
        assert!(music[PICTURES_AT] && !music[PICTURES_AT + 72]);
        let results = visible(ScreenPage::Results as u8, 128);
        assert!(results[PICTURES_AT] && results[bank_bytes(128) - 1], "the results read all five");
        let prefix = orbiters_core::osc::PARAMETERS_PREFIX;
        assert_eq!(synced_drop(), vec![(format!("{prefix}N4N0/Text/Pointer"), OscType::Int(253))]);
        assert_eq!(local_drop(true), (format!("{prefix}N4N0/Screen/Drop"), OscType::Bool(true)));
    }

    #[test]
    fn a_parked_pointer_lets_the_next_block_s_values_go_first() {
        let menu = results_menu();
        let want = Frame::public(&menu, Flags::default(), 72, &|_| None).bank;
        let mut everyone = Audience::new(8, 72);
        everyone.cleared(true);
        assert!(matches!(everyone.step(&want), Send::Values(0, _)), "cleared with the pointer where it writes nothing");
        assert!(matches!(everyone.step(&want), Send::Pointer(0)));
        // The pointer on a block: the next block rests first...
        assert_eq!(everyone.step(&want), Send::Rest);
        assert!(matches!(everyone.step(&want), Send::Values(_, _)));
        assert!(matches!(everyone.step(&want), Send::Pointer(_)));
        // ...unless something moved it where it writes nothing (253) meanwhile.
        everyone.park();
        assert!(matches!(everyone.step(&want), Send::Values(_, _)), "no rest needed");
        // Parking mid-block changes nothing: the block's pointer still goes.
        everyone.park();
        assert!(matches!(everyone.step(&want), Send::Pointer(_)));
    }
}
