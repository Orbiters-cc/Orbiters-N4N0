//! The worker's protocol: one JSON object a line, commands on stdin (`{"cmd": …}`), events on stdout
//! (`{"event": …}`). Candidates are WebRTC's `RTCIceCandidateInit` JSON, ICE servers WebRTC's `RTCIceServer` JSON.

use serde::{Deserialize, Serialize};
use webrtc::peer_connection::{RTCIceCandidateInit, RTCIceServer};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Starts a call as the caller: answers with `offer`.
    Offer {
        #[serde(default)]
        ice_servers: Vec<IceServer>,
        #[serde(default)]
        mic: Option<String>,
    },
    /// Takes a call: answers with `answer`.
    Answer {
        offer: String,
        #[serde(default)]
        ice_servers: Vec<IceServer>,
        #[serde(default)]
        mic: Option<String>,
    },
    /// The callee's answer, on the caller's side.
    RemoteAnswer { sdp: String },
    /// The other side's ICE candidates.
    Candidates { candidates: Vec<RTCIceCandidateInit> },
    /// Stops (or restarts) sending the microphone: silence goes out instead.
    Mute { on: bool },
    Hangup,
    Quit,
}

/// A STUN or TURN server as WebRTC writes it: `urls` is one URL or a list.
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct IceServer {
    pub urls: Urls,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub credential: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
#[serde(untagged)]
pub enum Urls {
    One(String),
    Many(Vec<String>),
}

impl From<IceServer> for RTCIceServer {
    fn from(server: IceServer) -> Self {
        RTCIceServer {
            urls: match server.urls {
                Urls::One(url) => vec![url],
                Urls::Many(urls) => urls,
            },
            username: server.username.unwrap_or_default(),
            credential: server.credential.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// This side hung up.
    Hangup,
    /// The connection could not be made, or was lost.
    Failed,
    /// The other side closed it.
    Closed,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready,
    Offer { sdp: String },
    Answer { sdp: String },
    Candidate { candidate: RTCIceCandidateInit },
    /// ICE and DTLS are up: audio flows.
    Connected,
    /// Peaks (0..1) of the microphone as sent and of the other side as heard, about five times a second.
    Levels { mic: f32, remote: f32 },
    Ended { reason: EndReason },
    Error { message: String },
}

impl Event {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("events always serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Command {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn offer_with_ice_servers_in_both_shapes() {
        let c = parse(
            r#"{"cmd":"offer","mic":null,"ice_servers":[
                {"urls":"stun:stun.cloudflare.com:3478"},
                {"urls":["turn:turn.cloudflare.com:3478?transport=udp","turns:turn.cloudflare.com:5349?transport=tcp"],"username":"u","credential":"p"}]}"#,
        );
        let Command::Offer { ice_servers, mic } = c else { panic!("{c:?}") };
        assert_eq!(mic, None);
        let servers: Vec<RTCIceServer> = ice_servers.into_iter().map(Into::into).collect();
        assert_eq!(servers[0].urls, vec!["stun:stun.cloudflare.com:3478"]);
        assert_eq!(servers[0].username, "");
        assert_eq!(servers[1].urls.len(), 2);
        assert_eq!((servers[1].username.as_str(), servers[1].credential.as_str()), ("u", "p"));
    }

    #[test]
    fn offer_without_optional_fields() {
        assert_eq!(parse(r#"{"cmd":"offer"}"#), Command::Offer { ice_servers: vec![], mic: None });
    }

    #[test]
    fn answer_with_mic() {
        let c = parse(r#"{"cmd":"answer","offer":"v=0\r\n","ice_servers":[],"mic":"{0.0.1.00000000}.{abc}"}"#);
        assert_eq!(c, Command::Answer { offer: "v=0\r\n".into(), ice_servers: vec![], mic: Some("{0.0.1.00000000}.{abc}".into()) });
    }

    #[test]
    fn remote_answer_mute_hangup_quit() {
        assert_eq!(parse(r#"{"cmd":"remote_answer","sdp":"x"}"#), Command::RemoteAnswer { sdp: "x".into() });
        assert_eq!(parse(r#"{"cmd":"mute","on":true}"#), Command::Mute { on: true });
        assert_eq!(parse(r#"{"cmd":"hangup"}"#), Command::Hangup);
        assert_eq!(parse(r#"{"cmd":"quit"}"#), Command::Quit);
    }

    #[test]
    fn candidates_as_browsers_write_them() {
        let c = parse(
            r#"{"cmd":"candidates","candidates":[
                {"candidate":"candidate:1 1 udp 2130706431 192.168.1.2 50000 typ host","sdpMid":"0","sdpMLineIndex":0},
                {"candidate":"candidate:2 1 udp 1694498815 1.2.3.4 50001 typ srflx raddr 0.0.0.0 rport 0","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"abcd"}]}"#,
        );
        let Command::Candidates { candidates } = c else { panic!("{c:?}") };
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].sdp_mid.as_deref(), Some("0"));
        assert_eq!(candidates[0].sdp_mline_index, Some(0));
        assert_eq!(candidates[1].username_fragment.as_deref(), Some("abcd"));
    }

    #[test]
    fn unknown_or_incomplete_commands_are_refused() {
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"dial"}"#).is_err());
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"answer"}"#).is_err());
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"mute"}"#).is_err());
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"offer","ice_servers":[{"urls":3}]}"#).is_err());
    }

    #[test]
    fn events_are_tagged_lines() {
        assert_eq!(Event::Ready.to_line(), r#"{"event":"ready"}"#);
        assert_eq!(Event::Connected.to_line(), r#"{"event":"connected"}"#);
        assert_eq!(Event::Offer { sdp: "v=0".into() }.to_line(), r#"{"event":"offer","sdp":"v=0"}"#);
        assert_eq!(Event::Ended { reason: EndReason::Failed }.to_line(), r#"{"event":"ended","reason":"failed"}"#);
        assert_eq!(Event::Levels { mic: 0.5, remote: 0.25 }.to_line(), r#"{"event":"levels","mic":0.5,"remote":0.25}"#);
        assert_eq!(Event::Error { message: "no".into() }.to_line(), r#"{"event":"error","message":"no"}"#);
        let candidate = RTCIceCandidateInit { candidate: "candidate:1".into(), sdp_mid: Some("0".into()), sdp_mline_index: Some(0), ..Default::default() };
        assert_eq!(
            Event::Candidate { candidate }.to_line(),
            r#"{"event":"candidate","candidate":{"candidate":"candidate:1","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":null}}"#
        );
    }
}
