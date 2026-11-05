//! N4N0's full pictures, the desktop's side (contracts `Docs/n4n0-picture.md`): each picture encoded once at both sizes
//! ([`Full`]), what each of the avatar's picture slots should hold, and the lanes that carry the streams there in chunks:
//! everyone's through the text channel ([`Synced`]: a chunk in the channel's three steps, in turn with the panel, the
//! screen and the card), the owner's through the 64 local values ([`Lane::local`], a chunk every local step).
//!
//! A chunk sets bytes, so nothing is lost by sending one again: each lane sends every chunk of what its slots should hold
//! once (headers first, then layer 1 of every picture, then layer 2, then layer 3: the pictures sharpen together), then
//! again in rounds, for whoever loaded the avatar since and to repair what VRChat dropped (everyone's lane always; the
//! owner's twice, then a round now and then).

use orbiters_contracts::n4n0::picture::{self, chunk, Encoded, Size};
use orbiters_contracts::n4n0::{self, params};
use rosc::OscType;
use std::sync::Arc;

/// A picture at both full sizes, encoded once (about 12 ms): the results' thumbnails use the small one, the music's art
/// and the link card's picture the large one.
#[derive(Clone, Debug, PartialEq)]
pub struct Full {
    pub small: Arc<Encoded>,
    pub large: Arc<Encoded>,
}

impl Full {
    /// From a picture already 16:9 (a video's thumbnail; the music's art fitted on the page's colour): each size a cover
    /// fit of it, encoded at its budget.
    pub fn from_rgb(rgb: &crate::picture::Rgb) -> Self {
        let encode = |size: Size| Arc::new(picture::encode(&crate::picture::fit(rgb, size.width(), size.height()), size, size.budget()));
        Self { small: encode(Size::Small), large: encode(Size::Large) }
    }

    pub fn encoded(&self, size: Size) -> &Encoded {
        match size {
            Size::Small => &self.small,
            Size::Large => &self.large,
        }
    }

    /// The stream for slot `slot` (its size) under the placeholder thumbnail `thumbnail` (the bytes the screen's memory
    /// holds: their first 8 are the signature).
    pub fn stream(&self, slot: usize, thumbnail: &[u8]) -> Option<Arc<Vec<u8>>> {
        let size = picture::slot_size(slot)?;
        let mut signature = [0u8; picture::SIGNATURE];
        signature.copy_from_slice(thumbnail.get(..picture::SIGNATURE)?);
        Some(Arc::new(self.encoded(size).stream(signature)))
    }
}

/// One chunk of a slot's stream, as a lane sends it: the slot, and its values' bytes (with the synced pointer).
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub slot: usize,
    pub pointer: Option<i32>,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Synced(usize),
    Local,
}

/// What a slot should hold and how often each of its chunks went.
#[derive(Clone, Debug)]
struct Slot {
    stream: Arc<Vec<u8>>,
    /// Each chunk's layer (0 the header), and how many times it went.
    layers: Vec<u8>,
    sent: Vec<u32>,
    /// Chunks that can't be sent (no salt keeps their values from 128: never with the encoder's streams).
    unsendable: Vec<bool>,
}

/// The owner's lane: every chunk twice, then one round every [`LOCAL_REPAIR`] (Orbiters - N4N0 asks with
/// [`Lane::again`]).
pub const LOCAL_ROUNDS: u32 = 2;
pub const LOCAL_REPAIR: std::time::Duration = std::time::Duration::from_secs(10);

/// One lane's slots: what each should hold, and what went of it.
#[derive(Clone, Debug)]
pub struct Lane {
    kind: Kind,
    slots: [Option<Slot>; chunk::WRITABLE_SLOTS],
    /// Slots wanted now, in their order (the page's pictures first, then the card's).
    order: Vec<usize>,
    /// The owner's lane stops after this many rounds until asked for another.
    rounds: u32,
}

impl Lane {
    /// Everyone's lane with `sync` text values (8 or 16).
    pub fn synced(sync: usize) -> Self {
        assert!(chunk::SYNCED_VALUES.contains(&sync));
        Self { kind: Kind::Synced(sync), slots: Default::default(), order: Vec::new(), rounds: u32::MAX }
    }

    /// The owner's lane (`N4N0/Picture/Value0` … `Value63`).
    pub fn local() -> Self {
        Self { kind: Kind::Local, slots: Default::default(), order: Vec::new(), rounds: LOCAL_ROUNDS }
    }

    fn chunk_count(&self, len: usize) -> usize {
        match self.kind {
            Kind::Synced(sync) => chunk::synced_chunks(len, sync).min(512),
            Kind::Local => len.div_ceil(chunk::LOCAL_PAYLOAD),
        }
    }

    fn chunk_range(&self, c: usize) -> (usize, usize) {
        let n = match self.kind {
            Kind::Synced(sync) => chunk::synced_payload(sync),
            Kind::Local => chunk::LOCAL_PAYLOAD,
        };
        (c * n, (c + 1) * n)
    }

    fn build(&self, slot: usize, c: usize, stream: &[u8]) -> Option<Chunk> {
        match self.kind {
            Kind::Synced(sync) => chunk::synced(slot, c, stream, sync).map(|(pointer, bytes)| Chunk { slot, pointer: Some(pointer), bytes }),
            Kind::Local => chunk::local(slot, c * chunk::LOCAL_PAYLOAD, stream).map(|bytes| Chunk { slot, pointer: None, bytes }),
        }
    }

    /// What the slots should hold now (`(slot, stream)`, in priority order): a slot whose stream changed starts over, one
    /// left out keeps what it has (and is not sent).
    pub fn want(&mut self, wanted: &[(usize, Arc<Vec<u8>>)]) {
        self.order.clear();
        for (slot, stream) in wanted {
            let slot = *slot;
            if slot >= chunk::WRITABLE_SLOTS || self.order.contains(&slot) {
                continue;
            }
            self.order.push(slot);
            if self.slots[slot].as_ref().is_some_and(|s| s.stream == *stream) {
                continue;
            }
            let header = picture::Header::parse(stream);
            let count = self.chunk_count(stream.len());
            let layers = (0..count)
                .map(|c| {
                    let (_, end) = self.chunk_range(c);
                    let last = end.min(stream.len()) - 1;
                    if last < picture::HEADER {
                        return 0;
                    }
                    header.map_or(3, |h| (0..picture::LAYERS).find(|&j| h.layer(j).is_some_and(|(_, e)| last < e)).map_or(3, |j| j as u8 + 1))
                })
                .collect();
            let unsendable = (0..count).map(|c| self.build(slot, c, stream).is_none()).collect();
            self.slots[slot] = Some(Slot { stream: stream.clone(), layers, sent: vec![0; count], unsendable });
        }
    }

    /// The next chunk and marks it sent: the least sent first, then the coarsest layer, then the slots' order.
    pub fn next_chunk(&mut self) -> Option<Chunk> {
        let mut best: Option<(u32, u8, usize, usize, usize)> = None;
        for (rank, &slot) in self.order.iter().enumerate() {
            let Some(s) = self.slots[slot].as_ref() else { continue };
            for c in 0..s.sent.len() {
                if s.unsendable[c] || s.sent[c] >= self.rounds {
                    continue;
                }
                let key = (s.sent[c], s.layers[c], rank, c, slot);
                if best.is_none_or(|b| key < b) {
                    best = Some(key);
                }
            }
        }
        let (_, _, _, c, slot) = best?;
        let s = self.slots[slot].as_mut().unwrap();
        s.sent[c] += 1;
        let stream = s.stream.clone();
        self.build(slot, c, &stream)
    }

    /// Whether a chunk would go now.
    pub fn wants(&self) -> bool {
        self.order.iter().filter_map(|&slot| self.slots[slot].as_ref()).any(|s| s.sent.iter().zip(&s.unsendable).any(|(&n, &u)| !u && n < self.rounds))
    }

    /// Whether every chunk of what the slots should hold went at least once.
    pub fn sent_once(&self) -> bool {
        self.order.iter().filter_map(|&slot| self.slots[slot].as_ref()).all(|s| s.sent.iter().zip(&s.unsendable).all(|(&n, &u)| u || n >= 1))
    }

    /// The avatar's textures are not known (an avatar loaded, messages lost): everything goes again from the start.
    pub fn forget(&mut self) {
        self.slots = Default::default();
        if self.kind == Kind::Local {
            self.rounds = LOCAL_ROUNDS;
        }
    }

    /// The owner's lane: one more round of everything (repairing what VRChat dropped).
    pub fn again(&mut self) {
        if self.kind == Kind::Local {
            let least = self.order.iter().filter_map(|&slot| self.slots[slot].as_ref()).flat_map(|s| s.sent.iter().copied()).min().unwrap_or(0);
            self.rounds = self.rounds.max(least + 1);
        }
    }
}

fn parameter(name: &str) -> String {
    format!("{}{name}", orbiters_core::osc::PARAMETERS_PREFIX)
}

/// The owner's chunk as messages: the 64 local values.
pub fn local_messages(chunk: &Chunk) -> Vec<crate::text::Message> {
    chunk.bytes.iter().enumerate().map(|(i, &b)| (parameter(&params::picture_value(i)), OscType::Float(n4n0::sync_value(b)))).collect()
}

/// `N4N0/Picture/Send`: the owner's bridge on while chunks flow through the local values.
pub fn local_send(on: bool) -> crate::text::Message {
    (parameter(params::PICTURE_SEND), OscType::Bool(on))
}

/// One step of everyone's lane on the text channel: the pointer to 0, a chunk's values, the chunk's pointer (never the
/// pointer and values together, as every block of the channel).
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Rest,
    Values(Vec<u8>),
    Pointer(i32),
}

impl Step {
    /// Whether this step moves the pointer onto a chunk (which then stays there a whole step).
    pub fn moves_to_block(&self) -> bool {
        matches!(self, Step::Pointer(_))
    }

    /// The step's messages with `sync` text values.
    pub fn messages(&self) -> Vec<crate::text::Message> {
        match self {
            Step::Rest => vec![(parameter(params::TEXT_POINTER), OscType::Int(0))],
            Step::Values(bytes) => bytes.iter().enumerate().map(|(i, &b)| (parameter(&params::text_sync(i)), OscType::Float(n4n0::sync_value(b)))).collect(),
            Step::Pointer(p) => vec![(parameter(params::TEXT_POINTER), OscType::Int(*p))],
        }
    }
}

/// Everyone's lane in the channel's steps.
#[derive(Clone, Debug)]
pub struct Synced {
    pub lane: Lane,
    /// The chunk whose values went (its pointer next), and whether the pointer rests at 0.
    pending: Option<i32>,
    queued: Option<Chunk>,
    resting: bool,
}

impl Synced {
    pub fn new(sync: usize) -> Self {
        Self { lane: Lane::synced(sync), pending: None, queued: None, resting: false }
    }

    /// Whether a chunk is between its steps: nothing else may use the channel until it is done.
    pub fn mid_block(&self) -> bool {
        self.pending.is_some() || self.queued.is_some()
    }

    /// Whether it has a step to send.
    pub fn wants(&self) -> bool {
        self.mid_block() || self.lane.wants()
    }

    /// Something else moved the pointer onto a block of its own: the next chunk rests first.
    pub fn moved_elsewhere(&mut self) {
        if !self.mid_block() {
            self.resting = false;
        }
    }

    /// Something else left the pointer where it writes nothing (0, a close).
    pub fn parked(&mut self) {
        if !self.mid_block() {
            self.resting = true;
        }
    }

    /// The avatar's textures are not known: everything goes again; where the pointer is, is not known either.
    pub fn forget(&mut self) {
        self.lane.forget();
        self.pending = None;
        self.queued = None;
        self.resting = false;
    }

    /// The next step, `None` when nothing is to send.
    pub fn step(&mut self) -> Option<Step> {
        if let Some(p) = self.pending.take() {
            self.resting = false;
            return Some(Step::Pointer(p));
        }
        let chunk = match self.queued.take() {
            Some(c) => c,
            None => {
                let c = self.lane.next_chunk()?;
                if !self.resting {
                    self.resting = true;
                    self.queued = Some(c);
                    return Some(Step::Rest);
                }
                c
            }
        };
        self.pending = chunk.pointer;
        Some(Step::Values(chunk.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(seed: u32) -> crate::picture::Rgb {
        let (width, height) = (320, 180);
        let pixels = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [((x * 3 + seed as usize * 40) % 256) as u8, ((y * 5) % 256) as u8, (((x + y) * 2 + seed as usize) % 256) as u8]
            })
            .collect();
        crate::picture::Rgb { width, height, pixels }
    }

    fn stream(seed: u32, slot: usize) -> Arc<Vec<u8>> {
        Full::from_rgb(&rgb(seed)).stream(slot, &[seed as u8; 72]).unwrap()
    }

    /// The streams the chunks write (as CRT A keeps them).
    fn apply(memory: &mut [Vec<u8>; 7], c: &Chunk) {
        let (slot, start, payload) = match c.pointer {
            Some(p) => chunk::parse_synced(p, &c.bytes).unwrap(),
            None => chunk::parse_local(&c.bytes).unwrap(),
        };
        assert_eq!(slot, c.slot);
        let m = &mut memory[slot];
        if m.len() < start + payload.len() {
            m.resize(start + payload.len(), 0);
        }
        m[start..start + payload.len()].copy_from_slice(&payload);
    }

    #[test]
    fn both_lanes_carry_every_stream_headers_and_coarse_layers_first() {
        let wanted: Vec<(usize, Arc<Vec<u8>>)> = (0..5).map(|s| (s, stream(s as u32, s))).chain([(picture::SLOT_CARD, stream(9, picture::SLOT_CARD))]).collect();
        for mut lane in [Lane::synced(8), Lane::synced(16), Lane::local()] {
            lane.want(&wanted);
            let mut memory: [Vec<u8>; 7] = Default::default();
            let mut layers = Vec::new();
            while !lane.sent_once() {
                let c = lane.next_chunk().unwrap();
                apply(&mut memory, &c);
                let start = match c.pointer {
                    Some(p) => chunk::parse_synced(p, &c.bytes).unwrap().1,
                    None => chunk::parse_local(&c.bytes).unwrap().1,
                };
                let end = start + c.bytes.len().saturating_sub(if c.pointer.is_some() { 2 } else { 5 });
                let s = &wanted.iter().find(|(slot, _)| *slot == c.slot).unwrap().1;
                let h = picture::Header::parse(s).unwrap();
                let last = end.min(s.len()) - 1;
                layers.push(if last < picture::HEADER { 0 } else { (0..3).find(|&j| last < h.layer(j).unwrap().1).unwrap() + 1 });
            }
            for (slot, s) in &wanted {
                assert_eq!(&memory[*slot][..s.len()], &s[..], "slot {slot}");
                assert_eq!(picture::decode(&memory[*slot], None).unwrap().layers, 3);
            }
            // Every picture's header, then layer 1 of every picture, then layer 2, then layer 3.
            assert!(layers.windows(2).all(|w| w[0] <= w[1]), "{layers:?}");
        }
        // The local lane stops after two rounds, until asked for another.
        let mut local = Lane::local();
        local.want(&wanted[..1]);
        let mut n = 0;
        while local.next_chunk().is_some() {
            n += 1;
        }
        assert_eq!(n, 2 * wanted[0].1.len().div_ceil(chunk::LOCAL_PAYLOAD));
        assert!(!local.wants());
        local.again();
        assert!(local.wants());
        // Everyone's lane never stops.
        let mut synced = Lane::synced(8);
        synced.want(&wanted[..1]);
        for _ in 0..1000 {
            assert!(synced.next_chunk().is_some());
        }
    }

    #[test]
    fn a_new_picture_starts_over_and_a_slot_left_out_is_not_sent() {
        let mut lane = Lane::synced(8);
        lane.want(&[(0, stream(1, 0)), (1, stream(2, 1))]);
        while !lane.sent_once() {
            lane.next_chunk();
        }
        // Slot 1 gets another picture: its header goes before anything resent.
        lane.want(&[(0, stream(1, 0)), (1, stream(3, 1))]);
        let c = lane.next_chunk().unwrap();
        assert_eq!((c.slot, chunk::parse_synced(c.pointer.unwrap(), &c.bytes).unwrap().1), (1, 0));
        // Only slot 1 wanted: slot 0 is never sent again.
        lane.want(&[(1, stream(3, 1))]);
        for _ in 0..500 {
            assert_eq!(lane.next_chunk().unwrap().slot, 1);
        }
    }

    #[test]
    fn everyone_s_chunks_go_in_three_steps() {
        let mut s = Synced::new(8);
        s.lane.want(&[(2, stream(4, 2))]);
        assert_eq!(s.step(), Some(Step::Rest));
        let Some(Step::Values(v)) = s.step() else { panic!() };
        assert_eq!(v.len(), 8);
        let Some(Step::Pointer(p)) = s.step() else { panic!() };
        assert_eq!(p, chunk::POINTER_FIRST + 16);
        assert!(!s.mid_block());
        assert_eq!(s.step(), Some(Step::Rest), "the next chunk rests first");
        s.step();
        s.step();
        // Another party left the pointer at 0: no rest needed.
        s.parked();
        assert!(matches!(s.step(), Some(Step::Values(_))));
        assert!(s.mid_block());
        let messages = Step::Values(vec![0, 1, 255, 129, 3, 4, 5, 6]).messages();
        assert!(messages[2].0.ends_with("N4N0/Text/Sync2"));
        let local = local_messages(&Chunk { slot: 0, pointer: None, bytes: vec![1; 64] });
        assert_eq!(local.len(), 64);
        assert!(local[63].0.ends_with("N4N0/Picture/Value63"));
    }
}
