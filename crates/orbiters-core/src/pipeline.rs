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
    pub fn open(mut stream: S, host: &str, path: &str, user_agent: &str) -> Result<Self, Option<u16>> {
        let key = STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
        stream.write_all(handshake(host, path, &key, user_agent).as_bytes()).map_err(|_| None)?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 16 * 1024 || stream.read(&mut byte).map_err(|_| None)? == 0 {
                return Err(None);
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head);
        if !switched(&head) {
            return Err(status_of(&head));
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

/// Keeps one pipeline connection open until `stop`: `token` gives a fresh token for each connection (`None`: no session,
/// stop), `message` gets every text message, `connected` is told each time it is up. Reconnects after 5 s, doubling to
/// 5 minutes while it keeps failing.
#[cfg(not(target_os = "android"))]
pub fn run(stop: &AtomicBool, mut token: impl FnMut() -> Option<String>, mut message: impl FnMut(&str), mut connected: impl FnMut(bool)) -> Ended {
    let mut wait = Duration::from_secs(5);
    while !stop.load(Ordering::Acquire) {
        let Some(t) = token() else { return Ended::Refused };
        match connect(&t) {
            Ok(mut c) => {
                connected(true);
                wait = Duration::from_secs(5);
                while !stop.load(Ordering::Acquire) {
                    match c.next_message() {
                        Ok(Some(text)) => message(&text),
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
                connected(false);
            }
            Err(Some(401 | 403)) => return Ended::Refused,
            Err(_) => {}
        }
        let until = Instant::now() + wait;
        while Instant::now() < until && !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(200));
        }
        wait = (wait * 2).min(Duration::from_secs(300));
    }
    Ended::Stopped
}

#[cfg(not(target_os = "android"))]
fn connect(token: &str) -> Result<Connection<native_tls::TlsStream<std::net::TcpStream>>, Option<u16>> {
    use std::net::ToSocketAddrs;
    let address = (HOST, 443).to_socket_addrs().map_err(|_| None)?.next().ok_or(None)?;
    let tcp = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(|_| None)?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|_| None)?;
    tcp.set_write_timeout(Some(Duration::from_secs(10))).map_err(|_| None)?;
    let tls = native_tls::TlsConnector::new().map_err(|_| None)?.connect(HOST, tcp).map_err(|_| None)?;
    let path = format!("/?authToken={}", crate::vrchat_api::encode(token));
    Connection::open(tls, HOST, &path, &crate::vrchat_api::user_agent())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(refused.err(), Some(Some(401)));
    }
}
