//! Orb's calls: the call worker (`orbiters-call`) and its protocol, JSON lines with commands on its stdin
//! (`{"cmd": …}`) and events on its stdout (`{"event": …}`); the call itself is in contracts `Docs/orb-calls.md`.
//!
//! The service numbers its calls: every command for a call and every event of one carries that number, so a late
//! event of a call that ended (or of one being replaced) never acts on the next. Candidates are WebRTC's
//! `RTCIceCandidateInit` JSON, relay servers WebRTC's `RTCIceServer` JSON, as the server passes them on.

use serde::{Deserialize, Deserializer, Serialize};

/// A STUN or TURN server as WebRTC writes it: `urls` is one URL or a list. Missing or null fields read as empty, so
/// an answer of the server that leaves one out never stops a call.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    #[serde(default, deserialize_with = "one_or_many")]
    pub urls: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default", skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, deserialize_with = "null_as_default", skip_serializing_if = "String::is_empty")]
    pub credential: String,
}

// The credential is a TURN password: never in a log.
impl std::fmt::Debug for IceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IceServer")
            .field("urls", &self.urls)
            .field("username", &self.username)
            .field("credential", &if self.credential.is_empty() { "" } else { "…" })
            .finish()
    }
}

/// The relay servers in a server's answer (`ice_servers`): the entries that read as servers, none when the field is
/// missing, null or not a list.
pub fn ice_servers(value: &serde_json::Value) -> Vec<IceServer> {
    value.as_array().map(|list| list.iter().filter_map(|s| serde_json::from_value(s.clone()).ok()).collect()).unwrap_or_default()
}

/// One ICE candidate, as WebRTC's `RTCIceCandidateInit` writes it. An empty `candidate` ends the candidates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceCandidate {
    #[serde(default, deserialize_with = "null_as_default")]
    pub candidate: String,
    #[serde(default)]
    pub sdp_mid: Option<String>,
    #[serde(default, rename = "sdpMLineIndex")]
    pub sdp_mline_index: Option<u16>,
    #[serde(default)]
    pub username_fragment: Option<String>,
}

impl IceCandidate {
    /// A candidate as the server passes it on: WebRTC's object, or only its `candidate` line.
    pub fn from_value(value: &serde_json::Value) -> Option<Self> {
        match value {
            serde_json::Value::String(line) => Some(Self { candidate: line.clone(), ..Default::default() }),
            other => serde_json::from_value(other.clone()).ok(),
        }
    }
}

/// Service → call worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum CallCommand {
    /// Starts call `call` as the caller (replacing any other): answered with [`CallEvent::Offer`].
    Offer {
        call: u64,
        #[serde(default, deserialize_with = "null_as_default")]
        ice_servers: Vec<IceServer>,
        #[serde(default)]
        mic: Option<String>,
        /// The call starts muted (Mute was pressed while it was being set up).
        #[serde(default)]
        muted: bool,
        /// Through the relay only: the other side never learns this computer's addresses.
        #[serde(default)]
        relay_only: bool,
    },
    /// Takes call `call` (replacing any other): answered with [`CallEvent::Answer`].
    Answer {
        call: u64,
        offer: String,
        #[serde(default, deserialize_with = "null_as_default")]
        ice_servers: Vec<IceServer>,
        #[serde(default)]
        mic: Option<String>,
        #[serde(default)]
        muted: bool,
        #[serde(default)]
        relay_only: bool,
    },
    /// The callee's answer, on the caller's side.
    RemoteAnswer {
        call: u64,
        sdp: String,
    },
    /// The other side's ICE candidates. Those for a call the worker has not started yet wait for it.
    Candidates {
        call: u64,
        #[serde(default, deserialize_with = "null_as_default")]
        candidates: Vec<IceCandidate>,
    },
    /// Stops (or restarts) sending the microphone: silence goes out instead.
    Mute {
        call: u64,
        on: bool,
    },
    Hangup {
        call: u64,
    },
    Quit,
}

impl CallCommand {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("serializable")
    }

    /// The call it is for (`None` for [`CallCommand::Quit`]).
    pub fn call(&self) -> Option<u64> {
        match self {
            Self::Offer { call, .. }
            | Self::Answer { call, .. }
            | Self::RemoteAnswer { call, .. }
            | Self::Candidates { call, .. }
            | Self::Mute { call, .. }
            | Self::Hangup { call } => Some(*call),
            Self::Quit => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// This side hung up.
    Hangup,
    /// The connection could not be made, or was lost.
    Failed,
    /// The other side closed it.
    Closed,
    /// Nothing reached the relay, nor the other side directly: this network seems to block calls (UDP).
    Unreachable,
}

/// Call worker → service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CallEvent {
    Ready,
    /// The caller's offer. It carries no candidate: they follow as [`CallEvent::Candidate`], which the service holds
    /// until the other side answered.
    Offer {
        call: u64,
        sdp: String,
    },
    /// The callee's answer (no candidate in it either).
    Answer {
        call: u64,
        sdp: String,
    },
    Candidate {
        call: u64,
        candidate: IceCandidate,
    },
    /// ICE and DTLS are up: audio flows.
    Connected {
        call: u64,
    },
    /// Peaks (0..1) of the microphone as sent and of the other side as heard, about five times a second.
    Levels {
        call: u64,
        mic: f32,
        remote: f32,
    },
    Ended {
        call: u64,
        reason: EndReason,
    },
    /// Something to tell the person (`call` 0: not about one call).
    Error {
        #[serde(default)]
        call: u64,
        message: String,
    },
}

impl CallEvent {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("serializable")
    }
}

/// `null` reads as the type's default (serde's `default` covers only a missing field).
fn null_as_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One URL or a list of them (`null`: none).
fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Urls {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<Urls>::deserialize(d)? {
        Some(Urls::One(url)) => vec![url],
        Some(Urls::Many(urls)) => urls,
        None => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(line: &str) -> CallCommand {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn commands_and_events_round_trip() {
        let offer = CallCommand::Offer {
            call: 3,
            ice_servers: vec![IceServer { urls: vec!["turn:turn.example:3478".into()], username: "u".into(), credential: "p".into() }],
            mic: Some("{0.0.1.00000000}.{abc}".into()),
            muted: true,
            relay_only: true,
        };
        assert_eq!(command(&offer.to_line()), offer);
        assert_eq!(offer.call(), Some(3));
        assert_eq!(CallCommand::Quit.call(), None);
        let events = [
            CallEvent::Ready,
            CallEvent::Offer { call: 1, sdp: "v=0".into() },
            CallEvent::Candidate {
                call: 1,
                candidate: IceCandidate { candidate: "candidate:1".into(), sdp_mid: Some("0".into()), sdp_mline_index: Some(0), ..Default::default() },
            },
            CallEvent::Connected { call: 1 },
            CallEvent::Levels { call: 1, mic: 0.5, remote: 0.25 },
            CallEvent::Ended { call: 1, reason: EndReason::Unreachable },
            CallEvent::Error { call: 0, message: "no".into() },
        ];
        for e in events {
            assert_eq!(serde_json::from_str::<CallEvent>(&e.to_line()).unwrap(), e);
        }
        assert_eq!(CallEvent::Ended { call: 2, reason: EndReason::Failed }.to_line(), r#"{"event":"ended","call":2,"reason":"failed"}"#);
    }

    #[test]
    fn relay_servers_as_the_server_writes_them() {
        // `null` where a list belongs reads as no server: the call still starts (it used to stop the worker cold).
        assert_eq!(
            command(r#"{"cmd":"answer","call":2,"offer":"v=0","ice_servers":null,"mic":null}"#),
            CallCommand::Answer { call: 2, offer: "v=0".into(), ice_servers: vec![], mic: None, muted: false, relay_only: false }
        );
        let CallCommand::Offer { ice_servers, .. } = command(
            r#"{"cmd":"offer","call":1,"ice_servers":[
                {"urls":"stun:stun.cloudflare.com:3478","username":null},
                {"urls":["turn:turn.cloudflare.com:3478?transport=udp","turns:turn.cloudflare.com:5349?transport=tcp"],"username":"u","credential":"p"}]}"#,
        ) else {
            panic!()
        };
        assert_eq!(ice_servers[0].urls, vec!["stun:stun.cloudflare.com:3478"]);
        assert_eq!(ice_servers[0].username, "");
        assert_eq!(ice_servers[1].urls.len(), 2);
        assert_eq!((ice_servers[1].username.as_str(), ice_servers[1].credential.as_str()), ("u", "p"));
        // From a server's answer: what is not a server is left out.
        let servers = ice_servers_of(json!({ "ice_servers": [{ "urls": "stun:a" }, 3, { "urls": 4 }, { "urls": ["turn:b"], "credential": null }] }));
        assert_eq!(servers.len(), 2);
        assert!(ice_servers_of(json!({ "ice_servers": null })).is_empty());
        assert!(ice_servers_of(json!({})).is_empty());
        // The TURN password stays out of logs.
        let debug = format!("{:?}", IceServer { urls: vec!["turn:b".into()], username: "u".into(), credential: "secret".into() });
        assert!(!debug.contains("secret"), "{debug}");
    }

    fn ice_servers_of(v: serde_json::Value) -> Vec<IceServer> {
        ice_servers(&v["ice_servers"])
    }

    #[test]
    fn candidates_as_browsers_and_the_server_write_them() {
        let CallCommand::Candidates { candidates, .. } = command(
            r#"{"cmd":"candidates","call":4,"candidates":[
                {"candidate":"candidate:1 1 udp 2130706431 192.168.1.2 50000 typ host","sdpMid":"0","sdpMLineIndex":0},
                {"candidate":"candidate:2 1 udp 1694498815 1.2.3.4 50001 typ srflx raddr 0.0.0.0 rport 0","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"abcd"}]}"#,
        ) else {
            panic!()
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].sdp_mid.as_deref(), Some("0"));
        assert_eq!(candidates[1].username_fragment.as_deref(), Some("abcd"));
        assert_eq!(IceCandidate::from_value(&json!("candidate:3 1 udp 1 1.2.3.4 9 typ relay")).unwrap().candidate, "candidate:3 1 udp 1 1.2.3.4 9 typ relay");
        assert_eq!(IceCandidate::from_value(&json!({ "candidate": "candidate:4", "sdpMid": "0" })).unwrap().sdp_mid.as_deref(), Some("0"));
        assert!(IceCandidate::from_value(&json!(7)).is_none());
        // Written as WebRTC writes them, for the other side's browser-shaped parser.
        let line =
            serde_json::to_string(&IceCandidate { candidate: "candidate:1".into(), sdp_mid: Some("0".into()), sdp_mline_index: Some(0), ..Default::default() })
                .unwrap();
        assert_eq!(line, r#"{"candidate":"candidate:1","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":null}"#);
    }

    #[test]
    fn unknown_or_incomplete_commands_are_refused() {
        assert!(serde_json::from_str::<CallCommand>(r#"{"cmd":"dial"}"#).is_err());
        assert!(serde_json::from_str::<CallCommand>(r#"{"cmd":"answer","call":1}"#).is_err());
        assert!(serde_json::from_str::<CallCommand>(r#"{"cmd":"mute","call":1}"#).is_err());
        assert!(serde_json::from_str::<CallCommand>(r#"{"cmd":"hangup"}"#).is_err(), "a hang-up names its call");
    }
}
