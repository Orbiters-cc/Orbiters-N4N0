//! One call: a peer connection with one two-way Opus track. Sound comes in through [`Call::mic`] (mono, 48 kHz, any
//! chunk size) and goes out through [`Call::speaker`]; whoever owns the call plugs a microphone and a speaker there (the
//! worker: `orbiters_sound::Duplex`; the loopback: a tone and a check).

use crate::opus::{FrameDecoder, FrameEncoder, FRAME, RATE};
use crate::protocol::EndReason;
use bytes::Bytes;
use rtc::media::Sample;
use rtc::peer_connection::configuration::media_engine::MIME_TYPE_OPUS;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind};
use std::collections::VecDeque;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::media_stream::{MediaStreamTrack, Track};
use webrtc::peer_connection::{
    register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription, Registry,
};
use webrtc::rtp_transceiver::RtpSender;

/// Opus's usual payload type in WebRTC (browsers use it too).
const OPUS_PAYLOAD_TYPE: u8 = 111;
/// What the speaker queue may hold when nothing plays it (the worker's speaker keeps 200 ms itself).
const SPEAKER_MOST: usize = RATE;

/// What a call tells its owner, tagged with the call's id.
#[derive(Debug)]
pub enum CallEvent {
    Candidate(RTCIceCandidateInit),
    Connected,
    Ended(EndReason),
}

/// The peaks since the last look (0..1): what was sent and what was heard.
#[derive(Default)]
pub struct Levels {
    mic: AtomicU32,
    remote: AtomicU32,
}

impl Levels {
    fn note(cell: &AtomicU32, peak: f32) {
        // Non-negative floats order like their bits.
        cell.fetch_max(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn take(&self) -> (f32, f32) {
        (f32::from_bits(self.mic.swap(0, Ordering::Relaxed)), f32::from_bits(self.remote.swap(0, Ordering::Relaxed)))
    }
}

pub struct Call {
    pub id: u64,
    pub started: Instant,
    /// Sound to send: mono, 48 kHz, any chunk size. Ignored until the call connects.
    pub mic: UnboundedSender<Vec<f32>>,
    /// Sound heard: mono, 48 kHz.
    pub speaker: Arc<Mutex<VecDeque<f32>>>,
    pub levels: Arc<Levels>,
    /// Silence goes out instead of the microphone while set.
    pub muted: Arc<AtomicBool>,
    pc: Arc<dyn PeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    mic_rx: Option<UnboundedReceiver<Vec<f32>>>,
    remote_set: bool,
    /// The other side's candidates that came before its description.
    early: Vec<RTCIceCandidateInit>,
    tasks: Vec<JoinHandle<()>>,
}

#[derive(Clone)]
struct Handler {
    id: u64,
    events: UnboundedSender<(u64, CallEvent)>,
    speaker: Arc<Mutex<VecDeque<f32>>>,
    levels: Arc<Levels>,
    connected: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(mut candidate) => {
                // The one m-line (BUNDLE): its mid is "0" as this side writes the offer and as browsers do.
                candidate.sdp_mid = Some("0".into());
                candidate.sdp_mline_index = Some(0);
                let _ = self.events.send((self.id, CallEvent::Candidate(candidate)));
            }
            Err(e) => eprintln!("call: unusable local candidate: {e}"),
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let event = match state {
            RTCPeerConnectionState::Connected if !self.connected.swap(true, Ordering::Relaxed) => CallEvent::Connected,
            RTCPeerConnectionState::Failed if !self.ended.swap(true, Ordering::Relaxed) => CallEvent::Ended(EndReason::Failed),
            RTCPeerConnectionState::Closed if !self.ended.swap(true, Ordering::Relaxed) => CallEvent::Ended(EndReason::Closed),
            _ => return,
        };
        let _ = self.events.send((self.id, event));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if track.kind().await != RtpCodecKind::Audio {
            return;
        }
        let (speaker, levels) = (self.speaker.clone(), self.levels.clone());
        tokio::spawn(async move {
            let mut decoder = match FrameDecoder::new() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("call: opus decoder: {e:#}");
                    return;
                }
            };
            let mut sound = Vec::with_capacity(FRAME * 2);
            while let Some(event) = track.poll().await {
                let TrackRemoteEvent::OnRtpPacket(packet) = event else { continue };
                sound.clear();
                decoder.packet(packet.header.sequence_number, &packet.payload, &mut sound);
                if sound.is_empty() {
                    continue;
                }
                Levels::note(&levels.remote, sound.iter().fold(0.0f32, |m, v| m.max(v.abs())));
                let mut queue = speaker.lock().unwrap();
                queue.extend(sound.iter().copied());
                let excess = queue.len().saturating_sub(SPEAKER_MOST);
                queue.drain(..excess);
            }
        });
    }
}

fn random_u32() -> u32 {
    std::collections::hash_map::RandomState::new().build_hasher().finish() as u32
}

impl Call {
    /// A peer connection with one sendrecv Opus track, its UDP sockets bound on `udp_addr` (`0.0.0.0:0`: every
    /// interface).
    pub async fn new(id: u64, ice_servers: Vec<RTCIceServer>, udp_addr: &str, events: UnboundedSender<(u64, CallEvent)>) -> anyhow::Result<Self> {
        let opus = RTCRtpCodec {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: RATE as u32,
            // Opus is always written opus/48000/2 in SDP; what is sent is mono.
            channels: 2,
            sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
            rtcp_feedback: vec![],
        };
        let mut media = MediaEngine::default();
        media.register_codec(RTCRtpCodecParameters { rtp_codec: opus.clone(), payload_type: OPUS_PAYLOAD_TYPE }, RtpCodecKind::Audio)?;
        let registry = register_default_interceptors(Registry::new(), &mut media)?;
        let speaker = Arc::new(Mutex::new(VecDeque::new()));
        let levels = Arc::new(Levels::default());
        let handler = Handler {
            id,
            events,
            speaker: speaker.clone(),
            levels: levels.clone(),
            connected: Arc::new(AtomicBool::new(false)),
            ended: Arc::new(AtomicBool::new(false)),
        };
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(RTCConfigurationBuilder::new().with_ice_servers(ice_servers).build())
                .with_media_engine(media)
                .with_interceptor_registry(registry)
                .with_handler(Arc::new(handler))
                .with_udp_addrs(vec![udp_addr.to_owned()])
                .build()
                .await?,
        );
        let track = Arc::new(TrackLocalStaticSample::new(
            Instant::now(),
            MediaStreamTrack::new(
                "orbiters-call".to_owned(),
                "orbiters-call-voice".to_owned(),
                "voice".to_owned(),
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(random_u32()), ..Default::default() },
                    active: true,
                    codec: opus,
                    ..Default::default()
                }],
            ),
        )?);
        let sender = match pc.add_track(track.clone() as Arc<dyn TrackLocal>).await {
            Ok(s) => s,
            Err(e) => {
                let _ = pc.close().await;
                return Err(e.into());
            }
        };
        // The track's own events (RTCP reports) are of no use here: read them so none pile up.
        let drain = {
            let track = track.clone();
            tokio::spawn(async move { while track.poll().await.is_some() {} })
        };
        let (mic, mic_rx) = unbounded_channel();
        Ok(Self {
            id,
            started: Instant::now(),
            mic,
            speaker,
            levels,
            muted: Arc::new(AtomicBool::new(false)),
            pc,
            track,
            sender,
            mic_rx: Some(mic_rx),
            remote_set: false,
            early: Vec::new(),
            tasks: vec![drain],
        })
    }

    /// The caller's offer (candidates follow as events).
    pub async fn offer(&mut self) -> anyhow::Result<String> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        self.local_sdp().await
    }

    /// The callee's answer to `offer`.
    pub async fn answer(&mut self, offer: String) -> anyhow::Result<String> {
        self.pc.set_remote_description(RTCSessionDescription::offer(offer)?).await?;
        self.remote_ready().await;
        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer).await?;
        self.local_sdp().await
    }

    /// The callee's answer, on the caller's side.
    pub async fn remote_answer(&mut self, sdp: String) -> anyhow::Result<()> {
        self.pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await?;
        self.remote_ready().await;
        Ok(())
    }

    pub async fn add_candidates(&mut self, candidates: Vec<RTCIceCandidateInit>) {
        if !self.remote_set {
            self.early.extend(candidates);
            return;
        }
        for candidate in candidates {
            if candidate.candidate.is_empty() {
                // End of candidates.
                continue;
            }
            if let Err(e) = self.pc.add_ice_candidate(candidate.clone()).await {
                eprintln!("call: candidate refused ({e}): {}", candidate.candidate);
            }
        }
    }

    async fn remote_ready(&mut self) {
        self.remote_set = true;
        let early = std::mem::take(&mut self.early);
        self.add_candidates(early).await;
    }

    async fn local_sdp(&self) -> anyhow::Result<String> {
        Ok(self.pc.local_description().await.ok_or_else(|| anyhow::anyhow!("no local description"))?.sdp)
    }

    /// Starts sending [`Call::mic`] (once connected): 20 ms Opus frames on the track.
    pub async fn start_sending(&mut self) -> anyhow::Result<()> {
        let Some(mut mic) = self.mic_rx.take() else { return Ok(()) };
        let ssrc = *self.track.ssrcs().await.first().ok_or_else(|| anyhow::anyhow!("the track has no SSRC"))?;
        let parameters = self.sender.get_parameters().await?;
        let payload_type = parameters.rtp_parameters.codecs.first().map(|c| c.payload_type).ok_or_else(|| anyhow::anyhow!("no codec was negotiated"))?;
        let mut encoder = FrameEncoder::new()?;
        let (track, muted, levels) = (self.track.clone(), self.muted.clone(), self.levels.clone());
        self.tasks.push(tokio::spawn(async move {
            let mut packets = Vec::new();
            let mut failed = false;
            while let Some(sound) = mic.recv().await {
                encoder.push(&sound, muted.load(Ordering::Relaxed), |packet, peak| {
                    Levels::note(&levels.mic, peak);
                    packets.push(Bytes::copy_from_slice(packet));
                });
                for data in packets.drain(..) {
                    let sample = Sample { data, duration: Duration::from_millis(20), ..Sample::new(Instant::now()) };
                    if let Err(e) = track.sample_writer(ssrc, payload_type).write_sample(&sample).await {
                        // Once is enough (a closing connection refuses every packet).
                        if !std::mem::replace(&mut failed, true) {
                            eprintln!("call: send: {e}");
                        }
                    }
                }
            }
        }));
        Ok(())
    }

    pub async fn close(self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Err(e) = self.pc.close().await {
            eprintln!("call: close: {e}");
        }
    }
}
