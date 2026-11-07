//! Two calls in one process, connected over 127.0.0.1 with host candidates only: each side sends a tone (440 Hz one
//! way, 660 Hz the other) in place of a microphone, and what each side hears is measured: length, level, pitch and how
//! much of it is the tone.

use crate::call::{Call, PeerEvent};
use crate::opus::{FRAME, RATE};
use std::fmt;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::JoinHandle;

const AMPLITUDE: f32 = 0.5;
const TONES: [f32; 2] = [440.0, 660.0];
/// What the decoder needs to settle, left out of the measure.
const SETTLE: usize = RATE * 3 / 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What one side heard of the other's tone.
pub struct Heard {
    pub tone: f32,
    pub seconds: f32,
    pub rms: f32,
    pub pitch: f32,
    /// The tone's share of the energy (1: nothing else).
    pub purity: f32,
}

impl Heard {
    fn measure(tone: f32, sound: &[f32]) -> Self {
        let seconds = sound.len() as f32 / RATE as f32;
        let window = sound.get(SETTLE..).unwrap_or_default();
        if window.len() < RATE / 10 {
            return Self { tone, seconds, rms: 0.0, pitch: 0.0, purity: 0.0 };
        }
        let n = window.len() as f32;
        let mean_square = window.iter().map(|v| v * v).sum::<f32>() / n;
        let rises = window.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        // Goertzel at the tone: a pure sine of amplitude A gives |X| = A N / 2.
        let w = std::f32::consts::TAU * tone / RATE as f32;
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        let coefficient = 2.0 * w.cos();
        for &x in window {
            let s = x + coefficient * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let magnitude = (s1 * s1 + s2 * s2 - coefficient * s1 * s2).max(0.0).sqrt();
        let amplitude = 2.0 * magnitude / n;
        let purity = if mean_square > 0.0 { (amplitude * amplitude / 2.0 / mean_square).min(1.0) } else { 0.0 };
        Self { tone, seconds, rms: mean_square.sqrt(), pitch: rises as f32 / (n / RATE as f32), purity }
    }

    fn passed(&self, listened: Duration) -> bool {
        let expected_rms = AMPLITUDE / std::f32::consts::SQRT_2;
        self.seconds >= listened.as_secs_f32() * 0.7 && (self.pitch - self.tone).abs() < 3.0 && (self.rms - expected_rms).abs() < 0.1 && self.purity > 0.9
    }
}

pub struct Report {
    pub connect_ms: u128,
    pub listened: Duration,
    /// What the callee heard of the caller (440 Hz), then the other way (660 Hz).
    pub heard: [Heard; 2],
}

impl Report {
    pub fn passed(&self) -> bool {
        self.heard.iter().all(|h| h.passed(self.listened))
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "connected in {} ms, listened {:.1} s", self.connect_ms, self.listened.as_secs_f32())?;
        for (who, h) in ["callee heard the caller", "caller heard the callee"].iter().zip(&self.heard) { // i18n: not shown (the loopback test tool prints for developers)
            writeln!(
                f,
                "{who}: {:.2} s of sound, {:.0} Hz sent, {:.1} Hz heard, rms {:.3} (sent {:.3}), tone {:.1}% of the energy",
                h.seconds,
                h.tone,
                h.pitch,
                h.rms,
                AMPLITUDE / std::f32::consts::SQRT_2,
                h.purity * 100.0
            )?;
        }
        write!(f, "{}", if self.passed() { "PASS" } else { "FAIL" })
    }
}

/// A tone into `call`'s microphone, 20 ms at a time, in real time.
fn play_tone(call: &Call, hz: f32) -> JoinHandle<()> {
    let mic = call.mic.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        let mut n = 0usize;
        loop {
            ticker.tick().await;
            let frame: Vec<f32> = (n..n + FRAME).map(|i| AMPLITUDE * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin()).collect();
            n += FRAME;
            mic.push(&frame);
        }
    })
}

pub async fn run(listen: Duration) -> anyhow::Result<Report> {
    let (events, mut event_rx) = unbounded_channel();
    let mut calls = [Call::new(1, vec![], false, "127.0.0.1:0", events.clone()).await?, Call::new(2, vec![], false, "127.0.0.1:0", events).await?];
    let started = Instant::now();
    let offer = calls[0].offer().await?;
    let answer = calls[1].answer(offer).await?;
    calls[0].remote_answer(answer).await?;
    let mut tones = Vec::new();
    let mut heard = [Vec::new(), Vec::new()];
    let mut connected = 0;
    let mut connect_ms = 0;
    let mut listening = None::<Instant>;
    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    let result = loop {
        if listening.is_none() && started.elapsed() > CONNECT_TIMEOUT {
            break Err(anyhow::anyhow!("the calls did not connect in {} s", CONNECT_TIMEOUT.as_secs()));
        }
        if listening.is_some_and(|t| t.elapsed() >= listen) {
            break Ok(());
        }
        tokio::select! {
            Some((id, event)) = event_rx.recv() => {
                let (me, other) = if id == 1 { (0, 1) } else { (1, 0) };
                match event {
                    PeerEvent::Candidate(candidate) => calls[other].add_candidates(vec![candidate]).await,
                    PeerEvent::Gathered => {}
                    PeerEvent::Connected => {
                        if let Err(e) = calls[me].start_sending().await {
                            break Err(e);
                        }
                        tones.push(play_tone(&calls[me], TONES[me]));
                        connected += 1;
                        if connected == 2 {
                            connect_ms = started.elapsed().as_millis();
                            listening = Some(Instant::now());
                        }
                    }
                    PeerEvent::Ended(reason) => break Err(anyhow::anyhow!("call {id} ended: {reason:?}")),
                }
            }
            _ = ticker.tick() => {
                // Each side's speaker: what the other sent.
                for (call, sound) in calls.iter().zip(heard.iter_mut()) {
                    if listening.is_some() {
                        sound.extend(call.speaker.lock().unwrap().drain(..));
                    } else {
                        call.speaker.lock().unwrap().clear();
                    }
                }
            }
        }
    };
    for tone in tones {
        tone.abort();
    }
    for call in calls {
        call.close().await;
    }
    result?;
    let [caller_heard, callee_heard] = heard;
    Ok(Report { connect_ms, listened: listen, heard: [Heard::measure(TONES[0], &callee_heard), Heard::measure(TONES[1], &caller_heard)] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_measure_knows_a_tone() {
        let sound: Vec<f32> = (0..RATE).map(|i| AMPLITUDE * (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin()).collect();
        let h = Heard::measure(440.0, &sound);
        assert!((h.pitch - 440.0).abs() < 1.5, "{}", h.pitch);
        assert!(h.purity > 0.99, "{}", h.purity);
        assert!(h.passed(Duration::from_secs(1)));
        assert!(Heard::measure(660.0, &sound).purity < 0.05);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tones_cross_a_loopback_call() {
        let report = run(Duration::from_secs(2)).await.unwrap();
        println!("{report}");
        assert!(report.passed(), "{report}");
    }
}
