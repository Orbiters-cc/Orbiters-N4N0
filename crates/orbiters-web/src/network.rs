//! Mandatory HTTP CONNECT proxy. Resolve, check every answer, then connect to that exact IP.
use std::{
    io::{Read, Write},
    net::{IpAddr, Shutdown, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
pub struct Proxy {
    pub port: u16,
    stop: Arc<AtomicBool>,
}
impl Proxy {
    pub fn new() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let count = Arc::new(AtomicUsize::new(0));
            while !flag.load(Ordering::Relaxed) {
                if let Ok((s, _)) = listener.accept() {
                    if count.load(Ordering::Relaxed) >= 32 {
                        continue;
                    }
                    count.fetch_add(1, Ordering::Relaxed);
                    let count = count.clone();
                    let flag = flag.clone();
                    std::thread::spawn(move || {
                        let _ = serve(s, flag);
                        count.fetch_sub(1, Ordering::Relaxed);
                    });
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        Ok(Self { port, stop })
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000 && s[0] != 0x2002 && !(s[0] == 0x2001 && s[1] < 0x200) && !(s[0] == 0x2001 && s[1] == 0xdb8) && (s[0] & 0xfff0 != 0x3ff0)
        }
    }
}
fn connect(host: &str, port: u16) -> anyhow::Result<TcpStream> {
    let addresses: Vec<_> = (host, port).to_socket_addrs()?.collect();
    anyhow::ensure!(!addresses.is_empty() && addresses.iter().all(|a| public(a.ip())), "private network destination denied");
    for a in addresses {
        if let Ok(s) = TcpStream::connect_timeout(&a, Duration::from_secs(3)) {
            return Ok(s);
        }
    }
    anyhow::bail!("destination unavailable")
}
fn serve(mut client: TcpStream, stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(3)))?;
    client.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        anyhow::ensure!(header.len() < 16384, "proxy header too large");
        let mut b = [0];
        client.read_exact(&mut b)?;
        header.push(b[0]);
    }
    let text = std::str::from_utf8(&header)?;
    let first = text.lines().next().unwrap_or("");
    let parts: Vec<_> = first.split_whitespace().collect();
    anyhow::ensure!(parts.len() == 3, "invalid proxy request");
    let tunnel = parts[0] == "CONNECT";
    let u = url::Url::parse(&if tunnel { format!("https://{}/", parts[1]) } else { parts[1].to_owned() })?;
    anyhow::ensure!(matches!(u.scheme(), "http" | "https" | "ws" | "wss") && u.username().is_empty() && u.password().is_none(), "proxy scheme denied");
    let mut upstream =
        match connect(u.host_str().ok_or_else(|| anyhow::anyhow!("missing host"))?.trim_matches(['[', ']']), u.port_or_known_default().unwrap_or(443)) {
            Ok(s) => s,
            Err(e) => {
                client.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")?;
                return Err(e);
            }
        };
    upstream.set_read_timeout(Some(Duration::from_millis(250)))?;
    upstream.set_write_timeout(Some(Duration::from_secs(3)))?;
    client.set_read_timeout(Some(Duration::from_millis(250)))?;
    if tunnel {
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
    } else {
        // Close each plaintext HTTP connection: a second absolute request must be revalidated.
        // WebSockets use CONNECT (wss); plaintext upgrades are deliberately refused.
        anyhow::ensure!(!text.to_ascii_lowercase().contains("upgrade:"), "plaintext upgrades denied");
        let path = &u[url::Position::BeforePath..url::Position::AfterQuery];
        write!(upstream, "{} {} HTTP/1.0\r\n", parts[0], path)?;
        for line in text.lines().skip(1).filter(|l| !l.is_empty()) {
            let lower = line.to_ascii_lowercase();
            if !lower.starts_with("connection:") && !lower.starts_with("proxy-") {
                write!(upstream, "{line}\r\n")?;
            }
        }
        upstream.write_all(b"Connection: close\r\n\r\n")?;
    }
    let mut client_read = client.try_clone()?;
    let mut upstream_write = upstream.try_clone()?;
    let flag = stop.clone();
    let copier = std::thread::spawn(move || {
        relay(&mut client_read, &mut upstream_write, &flag);
    });
    relay(&mut upstream, &mut client, &stop);
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = copier.join();
    Ok(())
}
fn relay(from: &mut TcpStream, to: &mut TcpStream, stop: &AtomicBool) {
    let mut b = [0; 16384];
    while !stop.load(Ordering::Relaxed) {
        match from.read(&mut b) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&b[..n]).is_err() {
                    break;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
            Err(_) => break,
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_all_nonpublic_families_and_mapped_addresses() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "172.31.0.1",
            "0.0.0.0",
            "224.1.1.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2002:7f00:1::",
            "64:ff9b::7f00:1",
        ] {
            assert!(!public(s.parse().unwrap()), "{s}");
        }
        assert!(public("8.8.8.8".parse().unwrap()));
        assert!(public("192.0.78.24".parse().unwrap()));
        assert!(public("192.2.1.1".parse().unwrap()));
    }
    #[test]
    fn blocked_request_never_arrives_at_local_service() {
        let fixture = TcpListener::bind("127.0.0.1:0").unwrap();
        fixture.set_nonblocking(true).unwrap();
        let proxy = Proxy::new().unwrap();
        for method in ["GET", "CONNECT"] {
            let mut s = TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let authority = fixture.local_addr().unwrap();
            let target = if method == "CONNECT" { authority.to_string() } else { format!("http://{authority}/side-effect") };
            write!(s, "{method} {target} HTTP/1.1\r\nHost: {authority}\r\n\r\n").unwrap();
            let mut b = [0; 128];
            let n = s.read(&mut b).unwrap();
            assert!(String::from_utf8_lossy(&b[..n]).contains("403"));
            assert!(fixture.accept().is_err());
        }
    }
}
