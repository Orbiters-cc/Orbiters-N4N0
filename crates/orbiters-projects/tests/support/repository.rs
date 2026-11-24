//! A disposable package repository on the loopback, for the tests of the resolver and installer.
#![allow(dead_code)]
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// The SHA-256 of `bytes`, in hex, as a package's cache has it beside it.
pub fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn archive(name: &str, version: &str) -> Vec<u8> {
    // A single stored ZIP member. Keep the fixture independent of the production unzip implementation.
    let data = serde_json::to_vec(&json!({"name":name,"version":version,"displayName":name})).unwrap();
    let file = b"package.json";
    let mut crc = 0xffff_ffffu32;
    for b in &data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    let crc = !crc;
    let mut out = vec![];
    let u16s = |out: &mut Vec<u8>, v: u16| out.extend(v.to_le_bytes());
    let u32s = |out: &mut Vec<u8>, v: u32| out.extend(v.to_le_bytes());
    u32s(&mut out, 0x04034b50);
    for n in [20, 0, 0, 0, 0] {
        u16s(&mut out, n);
    }
    for n in [crc, data.len() as u32, data.len() as u32] {
        u32s(&mut out, n);
    }
    u16s(&mut out, file.len() as u16);
    u16s(&mut out, 0);
    out.extend(file);
    out.extend(&data);
    let start = out.len();
    u32s(&mut out, 0x02014b50);
    for n in [20, 20, 0, 0, 0, 0] {
        u16s(&mut out, n);
    }
    for n in [crc, data.len() as u32, data.len() as u32] {
        u32s(&mut out, n);
    }
    for n in [file.len() as u16, 0, 0, 0, 0] {
        u16s(&mut out, n);
    }
    u32s(&mut out, 0);
    u32s(&mut out, 0);
    out.extend(file);
    let len = out.len() - start;
    u32s(&mut out, 0x06054b50);
    for n in [0, 0, 1, 1] {
        u16s(&mut out, n);
    }
    u32s(&mut out, len as u32);
    u32s(&mut out, start as u32);
    u16s(&mut out, 0);
    out
}
pub struct Repository {
    pub url: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Repository {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}
/// A loopback HTTP server answering each request with `answer(request)`, a body sent with 200 OK.
pub fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Repository) {
    serve_raw(move |request| {
        let body = answer(request);
        let mut out = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n", body.len()).into_bytes();
        out.extend(body);
        out
    })
}
/// A loopback HTTP server answering each request with the whole response `answer(request)` gives.
pub fn serve_raw(answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Repository) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let worker = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            // On Windows a connection taken from a listener that does not wait does not wait either: a read before the
            // request had come ended at once, and the answer was an empty body ("EOF while parsing a value").
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut data = vec![];
            let mut buf = [0; 2048];
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let _ = stream.write_all(&answer(&String::from_utf8_lossy(&data)));
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    (base.clone(), Repository { url: format!("{base}/index.json"), stop, worker: Some(worker) })
}
/// A repository whose archives are at `archives` (its own address when `None`).
pub fn repository_with_archives(archives: Option<&str>) -> Repository {
    let manifest = Arc::new(Mutex::new(Vec::new()));
    let listing = manifest.clone();
    let (base, repository) = serve(move |request| {
        let path = request.split_whitespace().nth(1).unwrap_or_default();
        let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if path == "/index.json" {
            assert!(request.to_ascii_lowercase().contains("x-fixture: enabled"));
            listing.lock().unwrap().clone()
        } else if parts.len() == 2 {
            archive(parts[0], parts[1].trim_end_matches(".zip"))
        } else {
            vec![]
        }
    });
    let at = archives.unwrap_or(&base).to_string();
    let package = |name: &str, version: &str, dep: bool| json!({"name":name,"version":version,"displayName":name,"url":format!("{at}/{name}/{version}.zip"),"vpmDependencies":if dep{json!({"test.base":">=1.0.0"})}else{json!({})}});
    *manifest.lock().unwrap() = serde_json::to_vec(&json!({"id":"test.repo","name":"Disposable repository","url":format!("{base}/index.json"),"packages":{"test.avatar":{"versions":{"1.0.0":package("test.avatar","1.0.0",true),"2.0.0":package("test.avatar","2.0.0",true)}},"test.base":{"versions":{"1.0.0":package("test.base","1.0.0",false)}}}})).unwrap();
    repository
}
pub fn repository() -> Repository {
    repository_with_archives(None)
}
