//! Orbiters' event streams (`text/event-stream`), read the same way for every feature that has one (N4N0 calls, the
//! avatar gun): a line at a time, a line or an event too long dropped whole, the connection given up when it stays
//! silent (Orbiters sends a comment line every 20 s) so that one that died without a word is opened again.

use std::io::{BufRead, BufReader};
use std::time::Duration;

/// Server-sent events, a line at a time.
pub(super) struct Sse {
    event: String,
    data: String,
    too_big: bool,
    /// The most an event's data may hold.
    most: usize,
}

impl Sse {
    pub(super) fn new(most: usize) -> Self {
        Self { event: String::new(), data: String::new(), too_big: false, most }
    }

    /// The event a blank line completes: its name and data. Comments (`: ping`) and other fields give nothing; an
    /// event whose data passes the most it may hold is dropped.
    pub(super) fn line(&mut self, line: &str) -> Option<(String, String)> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            let done = (!self.event.is_empty() && !self.too_big).then(|| (std::mem::take(&mut self.event), std::mem::take(&mut self.data)));
            self.event.clear();
            self.data.clear();
            self.too_big = false;
            return done;
        }
        if let Some(v) = line.strip_prefix("event:") {
            self.event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(v.strip_prefix(' ').unwrap_or(v));
            if self.data.len() > self.most {
                self.data.clear();
                self.too_big = true;
            }
        }
        None
    }
}

/// One line of `input` into `line` (without its end), at most `most` bytes: a longer one is skipped whole (false).
/// `None` at the end.
pub(super) fn read_line(input: &mut impl BufRead, line: &mut Vec<u8>, most: usize) -> std::io::Result<Option<bool>> {
    line.clear();
    let (mut any, mut skipped) = (false, false);
    loop {
        let buf = match input.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            return Ok(any.then_some(!skipped));
        }
        any = true;
        let newline = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if !skipped && line.len() + chunk.len() <= most {
            line.extend_from_slice(chunk);
        } else {
            skipped = true;
            line.clear();
        }
        let used = newline.map_or(buf.len(), |i| i + 1);
        input.consume(used);
        if newline.is_some() {
            return Ok(Some(!skipped));
        }
    }
}

/// An HTTP client whose reads give up after `idle` without a byte: a connection that died without a word ends
/// instead of waiting forever. (ureq has only overall timeouts; this wraps its transport.)
pub(super) fn idle_agent(connect: Duration, idle: Duration) -> ureq::Agent {
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport};

    #[derive(Debug)]
    struct Idle(Duration);

    #[derive(Debug)]
    struct IdleTransport<T> {
        inner: T,
        idle: Duration,
    }

    impl<In: Transport> Connector<In> for Idle {
        type Out = IdleTransport<In>;

        fn connect(&self, _: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
            Ok(chained.map(|inner| IdleTransport { inner, idle: self.0 }))
        }
    }

    impl<T: Transport> Transport for IdleTransport<T> {
        fn buffers(&mut self) -> &mut dyn Buffers {
            self.inner.buffers()
        }

        fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
            self.inner.transmit_output(amount, timeout)
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
            let idle = NextTimeout { after: self.idle.into(), reason: ureq::Timeout::RecvBody };
            self.inner.await_input(if idle.after < timeout.after { idle } else { timeout })
        }

        fn is_open(&mut self) -> bool {
            self.inner.is_open()
        }

        fn is_tls(&self) -> bool {
            self.inner.is_tls()
        }
    }

    let config = ureq::Agent::config_builder().middleware(orbiters_core::origin::guard).tls_config(orbiters_core::http_tls()).timeout_connect(Some(connect)).build();
    ureq::Agent::with_parts(config, DefaultConnector::new().chain(Idle(idle)), DefaultResolver::default())
}

/// Reads the event stream at `url` (with the bearer credential `token`) until it ends, fails, stays silent past `idle`,
/// or `still` says it is no longer wanted (asked once the server answered, then at every line, pings included); each
/// whole event goes to `each`
/// (its name and data), which says whether to go on. A line or an event's data longer than `most` is dropped. True
/// when it delivered something.
pub(super) fn follow(url: &str, token: &str, idle: Duration, most: usize, mut still: impl FnMut() -> bool, mut each: impl FnMut(&str, &str) -> bool) -> anyhow::Result<bool> {
    let agent = idle_agent(Duration::from_secs(15), idle);
    let response = agent.get(url).header("Authorization", &format!("Bearer {token}")).header("Accept", "text/event-stream").call()?;
    // Asked once the server took it too (before its first line, which may be its first ping): who waits for the stream
    // to be open knows it is.
    if !still() {
        return Ok(false);
    }
    let mut reader = BufReader::new(response.into_body().into_with_config().limit(u64::MAX).reader());
    let mut sse = Sse::new(most);
    let mut line = Vec::new();
    let mut delivered = false;
    while let Some(whole) = read_line(&mut reader, &mut line, most)? {
        delivered = true;
        if !still() {
            break;
        }
        if !whole {
            continue;
        }
        if let Some((event, data)) = sse.line(&String::from_utf8_lossy(&line)) {
            if !each(&event, &data) {
                break;
            }
        }
    }
    Ok(delivered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Instant;

    const MOST: usize = 256 * 1024;

    #[test]
    fn server_sent_events_are_read_line_by_line() {
        let mut sse = Sse::new(MOST);
        let mut events = Vec::new();
        for line in [": ping", "", "event: incoming", "data: {\"call\":", "data: \"k_1\"}", "", "event: ended\r", "data: {}\r", "\r"] {
            events.extend(sse.line(line));
        }
        assert_eq!(events, vec![("incoming".to_string(), "{\"call\":\n\"k_1\"}".to_string()), ("ended".to_string(), "{}".to_string())]);
        // An event too big to be one is dropped whole.
        let big = format!("data: {}", "x".repeat(MOST + 1));
        for line in ["event: candidates", big.as_str()] {
            assert!(sse.line(line).is_none());
        }
        assert!(sse.line("").is_none());
        assert_eq!(sse.line("event: declined").or(sse.line("")), Some(("declined".to_string(), String::new())));
    }

    #[test]
    fn a_silent_event_stream_gives_up() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // The server stays silent until the test is done, so that only the idle timeout can end the read.
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: ping\n\n").unwrap();
            // Then nothing, as from a server behind a connection that died.
            let _ = wait.recv_timeout(Duration::from_secs(30));
        });
        let agent = idle_agent(Duration::from_secs(2), Duration::from_millis(300));
        let response = agent.get(&format!("http://127.0.0.1:{port}/n4n0/calls/events")).call().unwrap();
        let mut reader = BufReader::new(response.into_body().into_with_config().limit(u64::MAX).reader());
        let mut line = Vec::new();
        assert_eq!(read_line(&mut reader, &mut line, MOST).unwrap(), Some(true));
        assert_eq!(line, b": ping");
        let started = Instant::now();
        let mut ended = false;
        for _ in 0..3 {
            match read_line(&mut reader, &mut line, MOST) {
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => {
                    ended = true;
                    break;
                }
            }
        }
        // 300 ms of silence ends it; the bound leaves room for a busy computer.
        let waited = started.elapsed();
        let _ = done.send(());
        server.join().unwrap();
        assert!(ended && waited < Duration::from_secs(10), "the read gave up after {waited:?}");
    }

    #[test]
    fn long_lines_are_skipped_whole() {
        let text = format!("a\n{}\nb", "x".repeat(40));
        let mut reader = std::io::BufReader::with_capacity(8, text.as_bytes());
        let mut line = Vec::new();
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), Some(true));
        assert_eq!(line, b"a");
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), Some(false));
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), Some(true));
        assert_eq!(line, b"b");
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), None);
    }

    /// A whole stream: its credential and its `Accept` sent, its events handed over in order, its pings delivering
    /// nothing but keeping it, and `each` ending it.
    #[test]
    fn a_stream_is_followed_event_by_event() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let n = socket.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer orbit-test") && request.contains("accept: text/event-stream"), "{request}");
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: ping\n\nevent: landed\ndata: {\"shot\":\"a\"}\n\nevent: crossed\ndata: {}\n\nevent: never\ndata: {}\n\n").unwrap();
        });
        let mut seen = Vec::new();
        let delivered = follow(&format!("http://127.0.0.1:{port}/n4n0/avatar-gun/events"), "orbit-test", Duration::from_secs(5), MOST, || true, |event, data| {
            seen.push((event.to_string(), data.to_string()));
            event != "crossed"
        })
        .unwrap();
        server.join().unwrap();
        assert!(delivered);
        assert_eq!(seen, [("landed".to_string(), "{\"shot\":\"a\"}".to_string()), ("crossed".to_string(), "{}".to_string())]);
    }
}
