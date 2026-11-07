//! One call: a peer connection with one two-way Opus track. Sound comes in through [`Call::mic`] (mono, 48 kHz, any
//! chunk size, from any thread) and goes out through [`Call::speaker`]; whoever owns the call plugs a microphone and a
//! speaker there (the worker: `orbiters_sound::Duplex`; the loopback: a tone and a check).
//!
//! What the other side learns of this computer: with `relay_only`, the relay's address only (no other candidate is
//! gathered); otherwise also the address of the socket this side binds (the worker binds the interface of the
//! default route only, never VPNs or virtual adapters) and the public address it maps to. Never the address behind a
//! reflexive or relayed candidate, and nothing in the descriptions: their candidates go one by one as events, when
//! the service chooses.

use crate::opus::{FrameDecoder, FrameEncoder, FRAME, RATE};
use crate::protocol;
use bytes::Bytes;
use orbiters_core::call::{EndReason, IceCandidate};
use rtc::media::Sample;
use rtc::peer_connection::configuration::media_engine::MIME_TYPE_OPUS;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind};
use std::collections::VecDeque;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use webrtc::data_channel::DataChannel;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::media_stream::{MediaStreamTrack, Track};
use webrtc::peer_connection::{
    register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceCandidateType, RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy, RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription,
    Registry,
};
use webrtc::rtp_transceiver::RtpSender;

/// Opus's usual payload type in WebRTC (browsers use it too).
const OPUS_PAYLOAD_TYPE: u8 = 111;
/// What the speaker queue may hold when nothing plays it (the worker's speaker keeps less itself).
const SPEAKER_MOST: usize = RATE;
/// What the microphone queue may hold when sending falls behind.
const MIC_MOST: usize = RATE / 2;
/// Without a sound from the microphone for this long (it failed, or went away), silence goes out in its place: the
/// other side keeps hearing a live call.
const MIC_SILENT_AFTER: Duration = Duration::from_millis(200);

/// What a call tells its owner, tagged with the call's number.
#[derive(Debug)]
pub enum PeerEvent {
    Candidate(IceCandidate),
    Connected,
    /// This side found all the candidates it will find.
    Gathered,
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

/// The candidates this side gathered, by kind (what a call that cannot connect says about the network).
#[derive(Default)]
pub struct Found {
    pub host: AtomicU32,
    pub reflexive: AtomicU32,
    pub relay: AtomicU32,
}

/// Sound to send: the microphone's thread pushes, the sending task takes whole frames. No allocation per chunk.
#[derive(Default)]
pub struct MicQueue {
    samples: Mutex<VecDeque<f32>>,
    ready: Notify,
}

impl MicQueue {
    pub fn push(&self, samples: &[f32]) {
        {
            let mut queue = self.samples.lock().unwrap();
            queue.extend(samples.iter().copied());
            let excess = queue.len().saturating_sub(MIC_MOST);
            queue.drain(..excess);
        }
        self.ready.notify_one();
    }
}

pub struct Call {
    pub id: u64,
    pub started: Instant,
    /// Only relay candidates are gathered and given.
    pub relay_only: bool,
    /// A TURN server this WebRTC can use was given.
    pub relay_given: bool,
    /// Sound to send. Ignored until the call connects.
    pub mic: Arc<MicQueue>,
    /// Sound heard: mono, 48 kHz.
    pub speaker: Arc<Mutex<VecDeque<f32>>>,
    pub levels: Arc<Levels>,
    /// Silence goes out instead of the microphone while set.
    pub muted: Arc<AtomicBool>,
    pub found: Arc<Found>,
    /// When the other side was last heard (an RTP packet), in ms after `started`.
    heard_at: Arc<AtomicU64>,
    pc: Arc<dyn PeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    sending: bool,
    remote_set: bool,
    /// The other side's candidates that came before its description.
    early: Vec<IceCandidate>,
    tasks: Vec<JoinHandle<()>>,
}

#[derive(Clone)]
struct Handler {
    id: u64,
    events: UnboundedSender<(u64, PeerEvent)>,
    started: Instant,
    relay_only: bool,
    speaker: Arc<Mutex<VecDeque<f32>>>,
    levels: Arc<Levels>,
    found: Arc<Found>,
    heard_at: Arc<AtomicU64>,
    connected: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let mut candidate = event.candidate;
        let count = match candidate.typ {
            RTCIceCandidateType::Relay => &self.found.relay,
            // Only the relay's address leaves this computer (WebRTC gathers nothing else then: this is a guard).
            _ if self.relay_only => return,
            RTCIceCandidateType::Host => &self.found.host,
            _ => &self.found.reflexive,
        };
        count.fetch_add(1, Ordering::Relaxed);
        // The address behind a reflexive or relayed candidate is this computer's own, and of no use to the other
        // side (RFC 8445 keeps it for diagnostics): left out, as browsers do.
        if !candidate.related_address.is_empty() {
            candidate.related_address = "0.0.0.0".into(); // network: not a socket (what a candidate says it was related to)
            candidate.related_port = 0;
        }
        match candidate.to_json() {
            Ok(mut candidate) => {
                // The one m-line (BUNDLE): its mid is "0" as this side writes the offer and as browsers do.
                candidate.sdp_mid = Some("0".into());
                candidate.sdp_mline_index = Some(0);
                let _ = self.events.send((self.id, PeerEvent::Candidate(protocol::candidate(candidate))));
            }
            Err(e) => eprintln!("call: unusable local candidate: {e}"),
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.events.send((self.id, PeerEvent::Gathered));
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let event = match state {
            RTCPeerConnectionState::Connected if !self.connected.swap(true, Ordering::Relaxed) => PeerEvent::Connected,
            RTCPeerConnectionState::Failed if !self.ended.swap(true, Ordering::Relaxed) => PeerEvent::Ended(EndReason::Failed),
            RTCPeerConnectionState::Closed if !self.ended.swap(true, Ordering::Relaxed) => PeerEvent::Ended(EndReason::Closed),
            _ => return,
        };
        let _ = self.events.send((self.id, event));
    }

    /// Calls carry sound only: a data channel the other side opens is closed at once, so nothing it sends piles up.
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        tokio::spawn(async move {
            let _ = channel.close().await;
        });
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if track.kind().await != RtpCodecKind::Audio {
            return;
        }
        let (speaker, levels, heard_at, started) = (self.speaker.clone(), self.levels.clone(), self.heard_at.clone(), self.started);
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
                heard_at.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
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
    /// interface; an address: that interface only). With `relay_only`, only relay candidates are gathered.
    pub async fn new(
        id: u64,
        ice_servers: Vec<RTCIceServer>,
        relay_only: bool,
        udp_addr: &str,
        events: UnboundedSender<(u64, PeerEvent)>,
    ) -> anyhow::Result<Self> {
        let relay_given = ice_servers.iter().any(|s| s.urls.iter().any(|u| u.starts_with("turn:")));
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
        let started = Instant::now();
        let speaker = Arc::new(Mutex::new(VecDeque::new()));
        let levels = Arc::new(Levels::default());
        let found = Arc::new(Found::default());
        let heard_at = Arc::new(AtomicU64::new(0));
        let handler = Handler {
            id,
            events,
            started,
            relay_only,
            speaker: speaker.clone(),
            levels: levels.clone(),
            found: found.clone(),
            heard_at: heard_at.clone(),
            connected: Arc::new(AtomicBool::new(false)),
            ended: Arc::new(AtomicBool::new(false)),
        };
        let policy = if relay_only { RTCIceTransportPolicy::Relay } else { RTCIceTransportPolicy::All };
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(RTCConfigurationBuilder::new().with_ice_servers(ice_servers).with_ice_transport_policy(policy).build())
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
        Ok(Self {
            id,
            started,
            relay_only,
            relay_given,
            mic: Arc::new(MicQueue::default()),
            speaker,
            levels,
            muted: Arc::new(AtomicBool::new(false)),
            found,
            heard_at,
            pc,
            track,
            sender,
            sending: false,
            remote_set: false,
            early: Vec::new(),
            tasks: vec![drain],
        })
    }

    /// The caller's offer, without candidates (they follow as events).
    pub async fn offer(&mut self) -> anyhow::Result<String> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        self.local_sdp().await
    }

    /// The callee's answer to `offer`, without candidates (they follow as events).
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

    pub async fn add_candidates(&mut self, candidates: Vec<IceCandidate>) {
        if !self.remote_set {
            self.early.extend(candidates);
            return;
        }
        let mut refused = 0;
        for candidate in candidates {
            if candidate.candidate.is_empty() {
                // End of candidates.
                continue;
            }
            if self.pc.add_ice_candidate(protocol::rtc_candidate(candidate)).await.is_err() {
                refused += 1;
            }
        }
        if refused > 0 {
            // Not the candidates themselves: they hold the other side's addresses.
            eprintln!("call: {refused} candidate(s) of the other side refused");
        }
    }

    async fn remote_ready(&mut self) {
        self.remote_set = true;
        let early = std::mem::take(&mut self.early);
        self.add_candidates(early).await;
    }

    async fn local_sdp(&self) -> anyhow::Result<String> {
        let sdp = self.pc.local_description().await.ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr("no local description")))?.sdp;
        Ok(protocol::without_candidates(&sdp))
    }

    /// How long nothing was heard from the other side (since `since`, when it connected, at the earliest).
    pub fn quiet_for(&self, since: Instant) -> Duration {
        let heard = self.started + Duration::from_millis(self.heard_at.load(Ordering::Relaxed));
        since.max(heard).elapsed()
    }

    /// Why a call that cannot connect fails: nothing found beyond this computer means the network blocks it.
    pub fn failure(&self) -> EndReason {
        let relays = self.found.relay.load(Ordering::Relaxed);
        let public = self.found.reflexive.load(Ordering::Relaxed);
        if (self.relay_only || self.relay_given) && relays == 0 && public == 0 {
            EndReason::Unreachable
        } else {
            EndReason::Failed
        }
    }

    /// Starts sending [`Call::mic`] (once connected): 20 ms Opus frames on the track, silence while the
    /// microphone gives nothing.
    pub async fn start_sending(&mut self) -> anyhow::Result<()> {
        if std::mem::replace(&mut self.sending, true) {
            return Ok(());
        }
        let ssrc = *self.track.ssrcs().await.first().ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr("the track has no SSRC")))?;
        let parameters = self.sender.get_parameters().await?;
        let payload_type = parameters.rtp_parameters.codecs.first().map(|c| c.payload_type).ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr("no codec was negotiated")))?;
        let mut encoder = FrameEncoder::new()?;
        let (track, mic, muted, levels) = (self.track.clone(), self.mic.clone(), self.muted.clone(), self.levels.clone());
        self.tasks.push(tokio::spawn(async move {
            let mut packets = Vec::new();
            let mut sound = Vec::with_capacity(FRAME * 4);
            let mut failed = false;
            let mut last_sound = Instant::now();
            loop {
                let woken = tokio::time::timeout(MIC_SILENT_AFTER, mic.ready.notified()).await.is_ok();
                sound.clear();
                sound.extend(mic.samples.lock().unwrap().drain(..));
                if !sound.is_empty() {
                    last_sound = Instant::now();
                } else if !woken {
                    // Nothing from the microphone: the time it was silent goes out as silence.
                    let gap = last_sound.elapsed().as_secs_f64();
                    sound.resize((gap * RATE as f64) as usize / FRAME * FRAME, 0.0);
                    last_sound += Duration::from_secs_f64(sound.len() as f64 / RATE as f64);
                }
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
