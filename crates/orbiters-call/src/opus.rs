//! Opus both ways: the microphone in 20 ms frames to packets, RTP packets back to sound, with loss covered by the next
//! packet's inband FEC and by packet loss concealment.

use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Bitrate, Channels, SampleRate, Signal};

/// Samples a second (mono).
pub const RATE: usize = 48_000;
/// One 20 ms frame.
pub const FRAME: usize = RATE / 50;
/// The voice's bitrate.
const BITRATE: i32 = 32_000;
/// The loss Opus prepares its FEC for (percent).
const EXPECTED_LOSS: u8 = 10;
/// Gaps up to this many frames are concealed; longer ones are left silent.
const MAX_CONCEALED: u16 = 10;
/// The largest packet a frame can make.
const MAX_PACKET: usize = 1500;
/// The encoder's effort (0 to 10): 6 sounds as good for a voice at this bitrate for about half of 10's CPU.
const COMPLEXITY: u8 = 6;

/// Cuts any sound into 20 ms frames and encodes them.
pub struct FrameEncoder {
    encoder: Encoder,
    pending: Vec<f32>,
    packet: Vec<u8>,
    /// Discontinuous transmission, on while muted: silence then costs a byte or two a frame.
    dtx: bool,
}

impl FrameEncoder {
    pub fn new() -> anyhow::Result<Self> {
        let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::BitsPerSecond(BITRATE))?;
        encoder.set_complexity(COMPLEXITY)?;
        // Always a voice: Opus need not guess (and never switches to its music mode on a laugh).
        encoder.set_signal(Signal::Voice)?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(EXPECTED_LOSS)?;
        Ok(Self { encoder, pending: Vec::with_capacity(FRAME * 2), packet: vec![0; MAX_PACKET], dtx: false })
    }

    /// Adds `samples`; `packet` gets each whole frame encoded, with the frame's peak (0 when `muted`: silence is sent).
    pub fn push(&mut self, samples: &[f32], muted: bool, mut packet: impl FnMut(&[u8], f32)) {
        self.pending.extend_from_slice(samples);
        if muted != self.dtx && self.encoder.set_dtx(muted).is_ok() {
            self.dtx = muted;
        }
        let mut start = 0;
        while self.pending.len() - start >= FRAME {
            let frame = &mut self.pending[start..start + FRAME];
            start += FRAME;
            let peak = if muted {
                frame.fill(0.0);
                0.0
            } else {
                frame.iter().fold(0.0f32, |m, v| m.max(v.abs())).min(1.0)
            };
            match self.encoder.encode_float(frame, &mut self.packet) {
                Ok(n) => packet(&self.packet[..n], peak),
                Err(e) => eprintln!("call: opus encode: {e}"),
            }
        }
        self.pending.drain(..start);
    }
}

/// Decodes the other side's packets in RTP order: late or repeated packets are dropped, a short gap is filled from
/// the next packet's FEC (its last frame) and by concealment (the frames before).
/// Packets this many numbers behind the next one are late (dropped); farther behind, the sender started again.
const LATE: i16 = 32;

pub struct FrameDecoder {
    decoder: Decoder,
    next: Option<u16>,
    frame: Vec<f32>,
    /// Frames rebuilt from FEC and concealed since the start (a check of the loopback).
    pub recovered: u32,
    pub concealed: u32,
}

impl FrameDecoder {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self { decoder: Decoder::new(SampleRate::Hz48000, Channels::Mono)?, next: None, frame: vec![0.0; FRAME * 6], recovered: 0, concealed: 0 })
    }

    /// Decodes the packet numbered `sequence` into `out`.
    pub fn packet(&mut self, sequence: u16, payload: &[u8], out: &mut Vec<f32>) {
        let gap = match self.next {
            Some(next) => {
                let ahead = sequence.wrapping_sub(next) as i16;
                if (-LATE..0).contains(&ahead) {
                    // A packet that came late: its frame was concealed already.
                    return;
                }
                // Far behind: the other side started its numbers again (a restarted stream). Taken as a new start,
                // or every packet would be "late" until the numbers climbed back, up to 11 minutes (SWEEP-17).
                if ahead < 0 {
                    0
                } else {
                    ahead as u16
                }
            }
            None => 0,
        };
        self.next = Some(sequence.wrapping_add(1));
        if payload.is_empty() {
            // Padding only.
            return;
        }
        if (1..=MAX_CONCEALED).contains(&gap) {
            for _ in 1..gap {
                self.decode(None, false, FRAME, out);
                self.concealed += 1;
            }
            self.decode(Some(payload), true, FRAME, out);
            self.recovered += 1;
        }
        let len = self.frame.len();
        self.decode(Some(payload), false, len, out);
    }

    fn decode(&mut self, payload: Option<&[u8]>, fec: bool, samples: usize, out: &mut Vec<f32>) {
        let packet = match payload.map(audiopus::packet::Packet::try_from) {
            Some(Ok(p)) => Some(p),
            Some(Err(_)) => return,
            None => None,
        };
        let Ok(signals) = audiopus::MutSignals::try_from(&mut self.frame[..samples]) else { return };
        match self.decoder.decode_float(packet, signals, fec) {
            Ok(n) => out.extend_from_slice(&self.frame[..n]),
            Err(e) => eprintln!("call: opus decode: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, offset: usize) -> Vec<f32> {
        (offset..offset + n).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin()).collect()
    }

    fn packets(frames: usize) -> Vec<Vec<u8>> {
        let mut encoder = FrameEncoder::new().unwrap();
        let mut out = Vec::new();
        // Odd chunk sizes, as a microphone gives them.
        let sound = tone(frames * FRAME, 0);
        for chunk in sound.chunks(441) {
            encoder.push(chunk, false, |p, peak| {
                assert!(peak <= 0.5 + 1e-6);
                out.push(p.to_vec());
            });
        }
        out
    }

    /// SWEEP-17: after the other side's numbers jump back (a restarted stream), what it sends is heard again at once; a
    /// packet a few numbers late is still dropped.
    #[test]
    fn a_stream_that_starts_its_numbers_again_is_heard() {
        let packets = packets(30);
        let mut decoder = FrameDecoder::new().unwrap();
        let mut out = Vec::new();
        for (i, p) in packets[..10].iter().enumerate() {
            decoder.packet(1000 + i as u16, p, &mut out);
        }
        let heard = out.len();
        decoder.packet(1005, &packets[5], &mut out);
        assert_eq!(out.len(), heard, "a late packet is dropped");
        for (i, p) in packets[10..20].iter().enumerate() {
            decoder.packet(5 + i as u16, p, &mut out);
        }
        assert!(out.len() >= heard + 10 * FRAME, "the restarted stream is heard: {} after {heard}", out.len());
    }

    #[test]
    fn frames_are_20_ms_at_about_32_kbit() {
        let packets = packets(50);
        assert_eq!(packets.len(), 50);
        let bits: usize = packets.iter().map(|p| p.len() * 8).sum();
        assert!((16_000..48_000).contains(&bits), "{bits} bit/s");
    }

    #[test]
    fn muted_frames_are_silent_and_small() {
        let mut encoder = FrameEncoder::new().unwrap();
        let mut sizes = Vec::new();
        encoder.push(&tone(FRAME * 50, 0), true, |p, peak| {
            assert_eq!(peak, 0.0);
            sizes.push(p.len());
        });
        assert_eq!(sizes.len(), 50);
        // Discontinuous transmission starts after 200 ms of silence: a byte a frame, a refresh every 400 ms.
        let after: usize = sizes[10..].iter().sum();
        assert!(after <= 40 * 3, "{sizes:?}");
    }

    #[test]
    fn losses_keep_the_timeline() {
        let packets = packets(30);
        let mut decoder = FrameDecoder::new().unwrap();
        let mut out = Vec::new();
        for (i, p) in packets.iter().enumerate() {
            // Lose 10, 20 and 21; deliver 5 twice and 25 late (after 26).
            if [10, 20, 21, 25].contains(&i) {
                continue;
            }
            // Numbered across the wrap.
            let number = |i: usize| (i as u16).wrapping_add(65_530);
            decoder.packet(number(i), p, &mut out);
            if i == 5 {
                decoder.packet(number(i), p, &mut out);
            }
            if i == 26 {
                decoder.packet(number(25), &packets[25], &mut out);
            }
        }
        // 30 frames sent, the late one played as lost: the timeline is whole.
        assert_eq!(out.len(), 30 * FRAME);
        assert_eq!((decoder.recovered, decoder.concealed), (3, 1));
        let rms = (out[FRAME * 2..].iter().map(|v| v * v).sum::<f32>() / (out.len() - FRAME * 2) as f32).sqrt();
        assert!((0.25..0.45).contains(&rms), "rms {rms}");
    }
}
