//! The worker's side of `orbiters_core::call` (one JSON object a line, commands on stdin, events on stdout): the
//! protocol's relay servers and candidates to WebRTC's and back, and what this side keeps out of what it hands the
//! other one.

use orbiters_core::call::{IceCandidate, IceServer};
use rtc::ice::url::{ProtoType, SchemeType, Url};
use webrtc::peer_connection::{RTCIceCandidateInit, RTCIceServer};

/// The relay servers this WebRTC can use.
#[derive(Debug, Default)]
pub struct Servers {
    pub servers: Vec<RTCIceServer>,
    /// A TURN server over UDP is among them.
    pub relay: bool,
    /// STUN or TURN URLs left out: TURN over TCP or TLS and STUN over TLS (webrtc-rs 0.21 allocates over UDP only,
    /// and one URL it cannot read would cost every relay of the call), TURN without credentials, malformed ones.
    pub skipped: usize,
}

/// The servers of `list` WebRTC can use.
pub fn servers(list: &[IceServer]) -> Servers {
    let mut out = Servers::default();
    for server in list {
        let mut urls = Vec::new();
        for raw in &server.urls {
            // A `?transport=udp` on a STUN URL is common and means nothing; WebRTC refuses it.
            let url = if raw.starts_with("stun:") { raw.split('?').next().unwrap_or(raw) } else { raw.as_str() };
            match Url::parse_url(url) {
                Ok(u) if u.scheme == SchemeType::Stun => urls.push(url.to_string()),
                Ok(u) if u.scheme == SchemeType::Turn && u.proto == ProtoType::Udp && !server.username.is_empty() && !server.credential.is_empty() => {
                    out.relay = true;
                    urls.push(url.to_string());
                }
                _ => out.skipped += 1,
            }
        }
        if !urls.is_empty() {
            out.servers.push(RTCIceServer { urls, username: server.username.clone(), credential: server.credential.clone() });
        }
    }
    out
}

pub fn rtc_candidate(c: IceCandidate) -> RTCIceCandidateInit {
    RTCIceCandidateInit { candidate: c.candidate, sdp_mid: c.sdp_mid, sdp_mline_index: c.sdp_mline_index, username_fragment: c.username_fragment, url: None }
}

pub fn candidate(c: RTCIceCandidateInit) -> IceCandidate {
    IceCandidate { candidate: c.candidate, sdp_mid: c.sdp_mid, sdp_mline_index: c.sdp_mline_index, username_fragment: c.username_fragment }
}

/// `sdp` without its candidates: they go to the other side one by one, when the service chooses (never with the
/// offer of a call that only rings).
pub fn without_candidates(sdp: &str) -> String {
    sdp.split_inclusive('\n')
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("a=candidate:") && !line.starts_with("a=end-of-candidates")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_webrtc_here_can_use() {
        let list = vec![
            IceServer { urls: vec!["stun:stun.cloudflare.com:3478".into(), "stun:stun.cloudflare.com:53?transport=udp".into()], ..Default::default() },
            IceServer {
                urls: vec![
                    "turn:turn.cloudflare.com:3478?transport=udp".into(),
                    "turn:turn.cloudflare.com:3478?transport=tcp".into(),
                    "turns:turn.cloudflare.com:5349?transport=tcp".into(),
                    "turn:turn.cloudflare.com:53".into(),
                    "nonsense".into(),
                ],
                username: "u".into(),
                credential: "p".into(),
            },
            // TURN without credentials would refuse the whole configuration.
            IceServer { urls: vec!["turn:turn.example:3478".into()], ..Default::default() },
        ];
        let s = servers(&list);
        assert!(s.relay);
        assert_eq!(s.skipped, 4);
        assert_eq!(s.servers.len(), 2);
        assert_eq!(s.servers[0].urls, vec!["stun:stun.cloudflare.com:3478", "stun:stun.cloudflare.com:53"]);
        assert_eq!(s.servers[1].urls, vec!["turn:turn.cloudflare.com:3478?transport=udp", "turn:turn.cloudflare.com:53"]);
        assert_eq!((s.servers[1].username.as_str(), s.servers[1].credential.as_str()), ("u", "p"));
        // Every URL WebRTC is given parses, credentials included.
        for server in &s.servers {
            assert!(server.urls().is_ok(), "{server:?}");
        }
        let stun_only = servers(&list[..1]);
        assert!(!stun_only.relay);
        assert!(servers(&[]).servers.is_empty());
    }

    #[test]
    fn descriptions_go_without_candidates() {
        let sdp = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=candidate:1 1 udp 2130706431 192.168.1.2 50000 typ host\r\n\
                   a=candidate:2 1 udp 1694498815 1.2.3.4 50001 typ srflx raddr 192.168.1.2 rport 50000\r\na=end-of-candidates\r\na=rtpmap:111 opus/48000/2\r\n";
        let out = without_candidates(sdp);
        assert_eq!(out, "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n");
        assert!(!out.contains("192.168"));
    }

    #[test]
    fn candidates_cross_unchanged() {
        let c = IceCandidate {
            candidate: "candidate:1 1 udp 1 1.2.3.4 9 typ relay".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
            username_fragment: Some("ab".into()),
        };
        assert_eq!(candidate(rtc_candidate(c.clone())), c);
    }
}
