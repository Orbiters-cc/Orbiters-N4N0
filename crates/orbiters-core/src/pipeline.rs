//! VRChat's pipeline: the one websocket (`wss://pipeline.vrchat.cloud/?authToken=…`) through which VRChat tells the
//! signed-in account what changes (friend-online, friend-location, notification, …; the `Websocket*` schemas of the
//! community spec). The launcher keeps its friends live with it instead of asking again and again.
//!
//! A small client of our own (RFC 6455, text messages only), since nothing else here speaks websockets: the framing is
//! pure and tested; the connection is TLS over a plain socket. The server's `Sec-WebSocket-Accept` is not checked (it
//! needs SHA-1, which nothing else here uses): TLS already says who answers.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const HOST: &str = "pipeline.vrchat.cloud";
/// The largest message kept (a friend's whole `User` is a few kB).
const MAX_MESSAGE: usize = 4 << 20;

/// The opening request, for `path` (`/?authToken=…`) with the client's `key`.
pub fn handshake(host: &str, path: &str, key: &str, user_agent: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: {user_agent}\r\n\r\n"
    )
}

/// The server's answer head says it switched to the websocket.
pub fn switched(head: &str) -> bool {
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let upgraded = lines.any(|l| l.split_once(':').is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("upgrade") && v.trim().eq_ignore_ascii_case("websocket")));
    status.starts_with("HTTP/1.1 101") && upgraded
}

/// Why the opening did not switch: the HTTP status when there was one, and the server's `Retry-After` (seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
}

impl From<Option<u16>> for Refusal {
    fn from(status: Option<u16>) -> Self {
        Self { status, retry_after: None }
    }
}

/// The `Retry-After` of an answer head: seconds, or an HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) counted from now.
pub fn retry_after_of(head: &str) -> Option<Duration> {
    let value = head.split("\r\n").find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("retry-after")).map(|(_, v)| v.trim().to_string()))?;
    retry_after_value(&value, time::OffsetDateTime::now_utc())
}

/// One `Retry-After` value at `now`: seconds, or an HTTP date; a date already past is no wait.
pub fn retry_after_value(value: &str, now: time::OffsetDateTime) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    use time::format_description::well_known::Rfc2822;
    let date = time::OffsetDateTime::parse(value, &Rfc2822).or_else(|_| time::OffsetDateTime::parse(&value.replace(" GMT", " +0000").replace(" UTC", " +0000"), &Rfc2822)).ok()?;
    Some(Duration::from_secs((date - now).whole_seconds().max(0) as u64))
}

/// The longest a server's wait is honoured as given (a week): beyond it the value is not a wait anyone meant.
const SERVER_WAIT_MOST: Duration = Duration::from_secs(7 * 24 * 3600);

/// The HTTP status of an answer head that did not switch (401: the token is no longer good).
pub fn status_of(head: &str) -> Option<u16> {
    head.split(' ').nth(1)?.parse().ok()
}

/// A client frame (always masked, as RFC 6455 wants from clients).
pub fn frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x80 | (opcode & 0x0F)];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= u16::MAX as usize => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Text(String),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// Reads the server's frames from the bytes as they come, joining fragmented messages.
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    message: Vec<u8>,
    in_text: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Broken;

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole event, `Ok(None)` when more bytes are needed, `Err` when the stream is not a websocket's.
    pub fn next_event(&mut self) -> Result<Option<Event>, Broken> {
        loop {
            let b = &self.buffer;
            if b.len() < 2 {
                return Ok(None);
            }
            let (fin, opcode, masked) = (b[0] & 0x80 != 0, b[0] & 0x0F, b[1] & 0x80 != 0);
            let (len, mut at) = match b[1] & 0x7F {
                126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
                127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()).min(usize::MAX as u64) as usize, 10),
                126 | 127 => return Ok(None),
                n => (n as usize, 2),
            };
            if len > MAX_MESSAGE {
                return Err(Broken);
            }
            let mask = if masked {
                if b.len() < at + 4 {
                    return Ok(None);
                }
                at += 4;
                Some([b[at - 4], b[at - 3], b[at - 2], b[at - 1]])
            } else {
                None
            };
            if b.len() < at + len {
                return Ok(None);
            }
            let mut payload: Vec<u8> = self.buffer[at..at + len].to_vec();
            if let Some(m) = mask {
                payload.iter_mut().enumerate().for_each(|(i, x)| *x ^= m[i % 4]);
            }
            self.buffer.drain(..at + len);
            match opcode {
                0x1 | 0x0 => {
                    if opcode == 0x1 {
                        self.message.clear();
                        self.in_text = true;
                    } else if !self.in_text {
                        return Err(Broken);
                    }
                    self.message.extend_from_slice(&payload);
                    if self.message.len() > MAX_MESSAGE {
                        return Err(Broken);
                    }
                    if fin {
                        self.in_text = false;
                        let text = String::from_utf8(std::mem::take(&mut self.message)).map_err(|_| Broken)?;
                        return Ok(Some(Event::Text(text)));
                    }
                }
                0x2 => {} // binary: VRChat sends none; skipped
                0x8 => return Ok(Some(Event::Close)),
                0x9 => return Ok(Some(Event::Ping(payload))),
                0xA => return Ok(Some(Event::Pong)),
                _ => return Err(Broken),
            }
        }
    }
}

fn mask() -> [u8; 4] {
    let b = *uuid::Uuid::new_v4().as_bytes();
    [b[0], b[1], b[2], b[3]]
}

/// One open pipeline over any stream (TLS for real, a buffer in tests).
pub struct Connection<S: Read + Write> {
    stream: S,
    decoder: Decoder,
    last_heard: Instant,
    last_ping: Instant,
}

/// How long nothing may come before a ping; and before the connection counts as dead.
const PING_AFTER: Duration = Duration::from_secs(30);
const DEAD_AFTER: Duration = Duration::from_secs(95);

impl<S: Read + Write> Connection<S> {
    /// Opens the websocket over `stream` (already connected, TLS done). `Err(Some(status))` when the server refused.
    pub fn open(mut stream: S, host: &str, path: &str, user_agent: &str) -> Result<Self, Refusal> {
        let key = STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
        stream.write_all(handshake(host, path, &key, user_agent).as_bytes()).map_err(|_| Refusal::from(None))?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 16 * 1024 || stream.read(&mut byte).map_err(|_| Refusal::from(None))? == 0 {
                return Err(None.into());
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head);
        if !switched(&head) {
            return Err(Refusal { status: status_of(&head), retry_after: retry_after_of(&head) });
        }
        Ok(Self { stream, decoder: Decoder::default(), last_heard: Instant::now(), last_ping: Instant::now() })
    }

    /// The next text message; `Ok(None)` when the read timed out with nothing (the caller checks its stop flag and comes
    /// back). Pings are answered and sent here. `Err` when the connection is over.
    pub fn next_message(&mut self) -> std::io::Result<Option<String>> {
        use std::io::{Error, ErrorKind};
        loop {
            match self.decoder.next_event().map_err(|_| Error::from(ErrorKind::InvalidData))? {
                Some(Event::Text(t)) => return Ok(Some(t)),
                Some(Event::Ping(p)) => self.stream.write_all(&frame(0xA, &p, mask()))?,
                Some(Event::Pong) => {}
                Some(Event::Close) => {
                    let _ = self.stream.write_all(&frame(0x8, &[], mask()));
                    return Err(Error::from(ErrorKind::ConnectionAborted));
                }
                None => {
                    let mut chunk = [0u8; 16 * 1024];
                    match self.stream.read(&mut chunk) {
                        Ok(0) => return Err(Error::from(ErrorKind::UnexpectedEof)),
                        Ok(n) => {
                            self.last_heard = Instant::now();
                            self.decoder.push(&chunk[..n]);
                        }
                        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                            if self.last_heard.elapsed() > DEAD_AFTER {
                                return Err(Error::from(ErrorKind::TimedOut));
                            }
                            if self.last_ping.elapsed() > PING_AFTER {
                                self.last_ping = Instant::now();
                                self.stream.write_all(&frame(0x9, b"n4n0", mask()))?;
                            }
                            return Ok(None);
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
}

/// Why the pipeline stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Stopped,
    /// The server refused the token (401/403): sign in again.
    Refused,
}

/// Why no token came for a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoToken {
    /// The session is over (signed out, or VRChat said 401): stop.
    Refused,
    /// Not now (no network, a time-out, a 429, a 5xx): try again after the usual wait.
    Later,
}

/// What `run` reads messages from: an open connection (a [`Connection`] for real, a script in tests).
pub trait Messages {
    /// As [`Connection::next_message`].
    fn next_message(&mut self) -> std::io::Result<Option<String>>;
}

impl<S: Read + Write> Messages for Connection<S> {
    fn next_message(&mut self) -> std::io::Result<Option<String>> {
        Connection::next_message(self)
    }
}

/// The first wait before a reconnection, and the longest.
const FIRST_WAIT: Duration = Duration::from_secs(5);
const LONGEST_WAIT: Duration = Duration::from_secs(300);
/// A connection that lived this long was a good one: the waits start again from the first.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// After VRChat answered 429 to the opening, at least this long.
const RATE_LIMITED_WAIT: Duration = Duration::from_secs(60);

/// `wait` spread over half to one and a half of itself by `random` (32768 is `wait` itself), so that every N4N0 does not reconnect
/// on the same beat after one of VRChat's restarts.
pub fn jittered(wait: Duration, random: u16) -> Duration {
    wait.mul_f64(0.5 + f64::from(random) / 65536.0)
}

/// Keeps one pipeline connection open until `stop`: `token` gives a token for each connection (`Err(NoToken::Refused)`:
/// no session, stop; `Err(NoToken::Later)`: try again later), `message` gets every text message, `connected` is told each
/// time it is up. Reconnects after about 5 s (with jitter), doubling to 5 minutes while it keeps failing; a connection
/// counts as good (and the wait starts again from 5 s) only once it lived a minute, so a server that accepts and closes at
/// once is not asked every 5 s.
#[cfg(not(target_os = "android"))]
pub fn run(stop: &AtomicBool, token: impl FnMut() -> Result<String, NoToken>, message: impl FnMut(&str), connected: impl FnMut(bool)) -> Ended {
    run_with(
        stop,
        token,
        connect,
        message,
        connected,
        |d| {
            let until = Instant::now() + d;
            while Instant::now() < until && !stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(200));
            }
        },
        Instant::now,
        || {
            let b = *uuid::Uuid::new_v4().as_bytes();
            u16::from_be_bytes([b[0], b[1]])
        },
    )
}

/// [`run`] with the connection, the waiting, the clock and the randomness given (tests use made-up ones).
#[allow(clippy::too_many_arguments)]
pub fn run_with<C: Messages>(
    stop: &AtomicBool,
    mut token: impl FnMut() -> Result<String, NoToken>,
    mut connect: impl FnMut(&str) -> Result<C, Refusal>,
    mut message: impl FnMut(&str),
    mut connected: impl FnMut(bool),
    mut pause: impl FnMut(Duration),
    clock: impl Fn() -> Instant,
    mut random: impl FnMut() -> u16,
) -> Ended {
    let mut wait = FIRST_WAIT;
    // What VRChat asked for after a 429: jitter only ever adds to it, never takes from it.
    let mut at_least = Duration::ZERO;
    while !stop.load(Ordering::Acquire) {
        match token() {
            Err(NoToken::Refused) => return Ended::Refused,
            Err(NoToken::Later) => {}
            Ok(t) => match connect(&t) {
                Ok(mut c) => {
                    let since = clock();
                    connected(true);
                    while !stop.load(Ordering::Acquire) {
                        match c.next_message() {
                            // Checked again: a message read while the account changed is not handed on.
                            Ok(Some(text)) if !stop.load(Ordering::Acquire) => message(&text),
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    connected(false);
                    if clock().saturating_duration_since(since) >= STABLE_AFTER {
                        wait = FIRST_WAIT;
                    }
                }
                Err(Refusal { status: Some(401 | 403), .. }) => return Ended::Refused,
                Err(Refusal { status: Some(429), retry_after }) => {
                    // Never below the server's own wait, in either form, and never below our minute.
                    at_least = retry_after.unwrap_or(RATE_LIMITED_WAIT).max(RATE_LIMITED_WAIT).min(SERVER_WAIT_MOST);
                    wait = wait.max(at_least);
                }
                Err(_) => {}
            },
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        pause(jittered(wait, random()).max(std::mem::take(&mut at_least)));
        wait = (wait * 2).min(LONGEST_WAIT);
    }
    Ended::Stopped
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
type Tls = native_tls::TlsStream<std::net::TcpStream>;
#[cfg(target_os = "linux")]
type Tls = rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>;

/// TLS to `host` over `tcp`, the certificate checked: the system's own (`crate::TLS_STACK`).
#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn secure(tcp: std::net::TcpStream, host: &str) -> Option<Tls> {
    native_tls::TlsConnector::new().ok()?.connect(host, tcp).ok()
}

/// TLS to `host` over `tcp`, the certificate checked against Mozilla's roots, as every other address there (`crate::tls_config`).
#[cfg(target_os = "linux")]
fn secure(mut tcp: std::net::TcpStream, host: &str) -> Option<Tls> {
    let mut connection = rustls::ClientConnection::new(linux_tls()?, rustls::pki_types::ServerName::try_from(host.to_string()).ok()?).ok()?;
    // The whole handshake here, as native TLS does it: a refused certificate is a refused connection, not a read error later.
    while connection.is_handshaking() {
        connection.complete_io(&mut tcp).ok()?;
    }
    Some(rustls::StreamOwned::new(connection, tcp))
}

/// The client's TLS: ring by name, for another crate of the same program may bring rustls another provider.
#[cfg(target_os = "linux")]
fn linux_tls() -> Option<std::sync::Arc<rustls::ClientConfig>> {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .ok()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Some(std::sync::Arc::new(config))
}

#[cfg(not(target_os = "android"))]
fn connect(token: &str) -> Result<Connection<Tls>, Refusal> {
    use std::net::ToSocketAddrs;
    // Every address the name gives (an IPv6 one first may have no route here), within one overall deadline.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut tcp = None;
    for address in (HOST, 443).to_socket_addrs().map_err(|_| Refusal::from(None))? {
        let left = deadline.saturating_duration_since(Instant::now());
        if left < Duration::from_secs(1) {
            break;
        }
        if let Ok(t) = std::net::TcpStream::connect_timeout(&address, left.min(Duration::from_secs(10))) {
            tcp = Some(t);
            break;
        }
    }
    let tcp = tcp.ok_or(Refusal::from(None))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|_| Refusal::from(None))?;
    tcp.set_write_timeout(Some(Duration::from_secs(10))).map_err(|_| Refusal::from(None))?;
    let tls = secure(tcp, HOST).ok_or(Refusal::from(None))?;
    let path = format!("/?authToken={}", crate::vrchat_api::encode(token));
    Connection::open(tls, HOST, &path, &crate::vrchat_api::user_agent())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pipeline's TLS against a real server: its certificate is taken for its own name and for no other
    /// (`cargo test -p orbiters-core this_system_s_tls -- --ignored`, the network needed).
    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore]
    fn this_system_s_tls_takes_a_certificate_for_its_own_name_only() {
        let dial = || std::net::TcpStream::connect(("orbiters.cc", 443)).expect("the network");
        assert!(secure(dial(), "orbiters.cc").is_some());
        assert!(secure(dial(), "not-orbiters.example").is_none(), "another name's certificate");
    }

    #[test]
    fn the_opening_asks_for_a_websocket_and_reads_the_switch() {
        let h = handshake(HOST, "/?authToken=t", "a2V5", "N4N0");
        assert!(h.starts_with("GET /?authToken=t HTTP/1.1\r\nHost: pipeline.vrchat.cloud\r\n"));
        assert!(h.contains("Upgrade: websocket\r\n") && h.contains("Sec-WebSocket-Version: 13\r\n") && h.ends_with("\r\n\r\n"));
        assert!(switched("HTTP/1.1 101 Switching Protocols\r\nupgrade: WebSocket\r\nConnection: Upgrade\r\n\r\n"));
        assert!(!switched("HTTP/1.1 401 Unauthorized\r\n\r\n"));
        assert_eq!(status_of("HTTP/1.1 401 Unauthorized\r\n\r\n"), Some(401));
    }

    fn server_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            n if n < 126 => out.push(n as u8),
            n if n <= 65535 => {
                out.push(126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn frames_are_read_whole_whatever_the_cut() {
        let message = r#"{"type":"friend-offline","content":"{\"userId\":\"usr_1\",\"platform\":\"\"}"}"#;
        let long = "x".repeat(70_000);
        let mut bytes = server_frame(true, 0x1, message.as_bytes());
        bytes.extend(server_frame(true, 0x9, b"hi"));
        bytes.extend(server_frame(false, 0x1, b"{\"a\":"));
        bytes.extend(server_frame(true, 0x0, b"1}"));
        bytes.extend(server_frame(true, 0x1, long.as_bytes()));
        bytes.extend(server_frame(true, 0x8, &[]));
        for cut in [1, 3, 7, 1000, bytes.len()] {
            let mut d = Decoder::default();
            let mut got = vec![];
            for chunk in bytes.chunks(cut) {
                d.push(chunk);
                while let Some(e) = d.next_event().unwrap() {
                    got.push(e);
                }
            }
            assert_eq!(got, [Event::Text(message.into()), Event::Ping(b"hi".to_vec()), Event::Text("{\"a\":1}".into()), Event::Text(long.clone()), Event::Close], "cut {cut}");
        }
        let mut d = Decoder::default();
        d.push(&server_frame(true, 0x0, b"x"));
        assert_eq!(d.next_event(), Err(Broken), "a continuation with no start");
    }

    #[test]
    fn client_frames_are_masked() {
        let f = frame(0x1, b"abc", [1, 2, 3, 4]);
        assert_eq!(f, [0x81, 0x83, 1, 2, 3, 4, b'a' ^ 1, b'b' ^ 2, b'c' ^ 3]);
        let mut d = Decoder::default();
        d.push(&frame(0x1, "x".repeat(300).as_bytes(), [9, 8, 7, 6]));
        assert_eq!(d.next_event().unwrap(), Some(Event::Text("x".repeat(300))), "a masked frame reads back");
    }

    /// A recorded exchange: the server's switch, then a message, a ping (answered with a pong), and its close.
    struct Recorded {
        incoming: std::io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }
    impl Read for Recorded {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.incoming.read(buf)
        }
    }
    impl Write for Recorded {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_connection_hands_out_messages_and_answers_pings() {
        let mut incoming = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n".to_vec();
        incoming.extend(server_frame(true, 0x9, b"p"));
        incoming.extend(server_frame(true, 0x1, b"{\"type\":\"friend-add\"}"));
        incoming.extend(server_frame(true, 0x8, &[]));
        let mut c = Connection::open(Recorded { incoming: std::io::Cursor::new(incoming), written: vec![] }, HOST, "/?authToken=t", "N4N0").unwrap();
        assert_eq!(c.next_message().unwrap().as_deref(), Some("{\"type\":\"friend-add\"}"));
        assert!(c.next_message().is_err(), "closed");
        let written = &c.stream.written;
        let opening = written.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert!(std::str::from_utf8(&written[..opening]).unwrap().starts_with("GET /?authToken=t HTTP/1.1"));
        assert_eq!(written[opening], 0x8A, "the ping is answered with a pong");
        let refused = Connection::open(Recorded { incoming: std::io::Cursor::new(b"HTTP/1.1 401 Unauthorized\r\n\r\n".to_vec()), written: vec![] }, HOST, "/", "N4N0");
        assert_eq!(refused.err().map(|r| r.status), Some(Some(401)));
    }
    /// A connection that gives its script, then closes.
    struct Script(Vec<&'static str>);
    impl Messages for Script {
        fn next_message(&mut self) -> std::io::Result<Option<String>> {
            if self.0.is_empty() {
                Err(std::io::ErrorKind::ConnectionAborted.into())
            } else {
                Ok(Some(self.0.remove(0).to_string()))
            }
        }
    }

    /// Runs the loop with a made-up clock: `connect` answers per attempt; it stops after `attempts` tries.
    fn simulate(attempts: usize, token: impl FnMut(usize) -> Result<String, NoToken>, mut connect: impl FnMut(usize) -> Result<Script, Option<u16>>, lives: Duration) -> (Ended, Vec<Duration>, usize, usize) {
        simulate_with(attempts, token, move |n| connect(n).map_err(Refusal::from), lives, 32768)
    }

    fn simulate_with(attempts: usize, mut token: impl FnMut(usize) -> Result<String, NoToken>, mut connect: impl FnMut(usize) -> Result<Script, Refusal>, lives: Duration, random: u16) -> (Ended, Vec<Duration>, usize, usize) {
        let stop = AtomicBool::new(false);
        let now = std::cell::Cell::new(Instant::now());
        let (mut tokens, mut tries, mut ups) = (0, 0, 0);
        let mut pauses = Vec::new();
        let ended = run_with(
            &stop,
            || {
                tokens += 1;
                token(tokens)
            },
            |_| {
                tries += 1;
                if tries >= attempts {
                    stop.store(true, Ordering::Release);
                }
                connect(tries)
            },
            |_| {},
            |up| {
                if up {
                    ups += 1;
                } else {
                    now.set(now.get() + lives);
                }
            },
            |d| {
                pauses.push(d);
                now.set(now.get() + d);
            },
            || now.get(),
            || random,
        );
        (ended, pauses, tries, ups)
    }

    #[test]
    fn a_server_that_accepts_and_closes_at_once_is_asked_less_and_less() {
        let (ended, pauses, tries, ups) = simulate(8, |_| Ok("t".into()), |_| Ok(Script(vec![])), Duration::from_secs(1));
        assert_eq!((ended, tries, ups), (Ended::Stopped, 8, 8));
        let secs: Vec<u64> = pauses.iter().map(|d| d.as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 80, 160, 300], "doubling, never back to 5 s for a connection that did not last");
    }

    #[test]
    fn a_connection_that_lasted_starts_the_waits_again() {
        let (_, pauses, ..) = simulate(4, |_| Ok("t".into()), |_| Ok(Script(vec!["{}"])), Duration::from_secs(120));
        let secs: Vec<u64> = pauses.iter().map(|d| d.as_secs()).collect();
        assert_eq!(secs, [5, 5, 5]);
    }

    #[test]
    fn a_token_that_cannot_be_read_now_is_tried_again_and_a_refused_one_stops() {
        // No network for three tries, then a connection: the pipeline does not end on the way.
        let (ended, pauses, tries, _) = simulate(1, |n| if n <= 3 { Err(NoToken::Later) } else { Ok("t".into()) }, |_| Ok(Script(vec![])), Duration::from_secs(1));
        assert_eq!((ended, tries), (Ended::Stopped, 1));
        assert_eq!(pauses.iter().map(|d| d.as_secs()).collect::<Vec<_>>(), [5, 10, 20]);
        let (ended, pauses, tries, _) = simulate(9, |_| Err(NoToken::Refused), |_| Ok(Script(vec![])), Duration::ZERO);
        assert_eq!((ended, pauses.len(), tries), (Ended::Refused, 0, 0));
        let (ended, ..) = simulate(9, |_| Ok("t".into()), |_| Err(Some(401)), Duration::ZERO);
        assert_eq!(ended, Ended::Refused);
    }

    #[test]
    fn a_429_on_the_opening_waits_at_least_a_minute() {
        let (_, pauses, ..) = simulate(2, |_| Ok("t".into()), |_| Err(Some(429)), Duration::ZERO);
        assert_eq!(pauses[0].as_secs(), 60);
    }

    #[test]
    fn jitter_never_shortens_what_vrchat_asked_after_a_429() {
        for random in [0, 32768, u16::MAX] {
            let (_, pauses, ..) = simulate_with(2, |_| Ok("t".into()), |_| Err(Refusal { status: Some(429), retry_after: None }), Duration::ZERO, random);
            assert!(pauses[0] >= Duration::from_secs(60), "{random}: {:?}", pauses[0]);
            let (_, pauses, ..) = simulate_with(2, |_| Ok("t".into()), |_| Err(Refusal { status: Some(429), retry_after: Some(Duration::from_secs(900)) }), Duration::ZERO, random);
            assert!(pauses[0] >= Duration::from_secs(900), "the server's own wait, longer than the usual longest: {random}: {:?}", pauses[0]);
        }
        assert_eq!(retry_after_of("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\n\r\n"), Some(Duration::from_secs(120)));
        // Two hours, and an HTTP date ten minutes ahead: never shortened, at either jitter extreme.
        for random in [0, u16::MAX] {
            let (_, pauses, ..) = simulate_with(2, |_| Ok("t".into()), |_| Err(Refusal { status: Some(429), retry_after: Some(Duration::from_secs(7200)) }), Duration::ZERO, random);
            assert!(pauses[0] >= Duration::from_secs(7200), "{random}: {:?}", pauses[0]);
        }
        let now = time::OffsetDateTime::from_unix_timestamp(1_795_089_600).unwrap();
        assert_eq!(retry_after_value("Thu, 19 Nov 2026 12:10:00 GMT", now), Some(Duration::from_secs(600)));
        assert_eq!(retry_after_value("Thu, 19 Nov 2026 11:00:00 GMT", now), Some(Duration::ZERO), "a date gone by");
        assert_eq!(retry_after_value("soon", now), None);
        assert_eq!(retry_after_of("HTTP/1.1 429 Too Many Requests\r\n\r\n"), None);
    }

    #[test]
    fn waits_are_spread_around_their_length() {
        let w = Duration::from_secs(10);
        assert_eq!(jittered(w, 0), Duration::from_secs(5));
        assert!(jittered(w, u16::MAX) < Duration::from_secs(15) && jittered(w, u16::MAX) > Duration::from_millis(14_990));
        assert_eq!(jittered(w, 32768), w);
    }
}
