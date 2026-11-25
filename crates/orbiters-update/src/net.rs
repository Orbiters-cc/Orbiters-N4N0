//! Fetching a channel's release and downloading its setup. Everything goes over HTTPS with the system's own TLS and
//! certificate store (ureq's native TLS, as everywhere in the app; on Linux and Android, rustls with Mozilla's roots, as
//! `orbiters_core::http_tls` there); plain HTTP is allowed to this computer only (a test feed). The release is trusted
//! for its signature, the setup for the size and SHA-256 the release gives.

use crate::{feed, release_url, Artifact, Release, SignedRelease};
use anyhow::Context as _;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The largest release file read: a few kilobytes are expected.
const MAX_RELEASE: u64 = 256 * 1024;

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = if cfg!(any(target_os = "android", target_os = "linux")) {
        TlsConfig::builder().provider(TlsProvider::Rustls).root_certs(RootCerts::WebPki).build()
    } else {
        TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build()
    };
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        // Long enough for a slow connection, never forever: a download cut short goes on from there next time.
        .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
        .user_agent(format!("Orbiters-N4N0/{}", orbiters_version::get()))
        .build()
        .new_agent()
}

/// Whether an address may be fetched: `https://`, or `http://` to this computer.
fn allowed(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| rest.strip_prefix(host).is_some_and(|after| after.is_empty() || after.starts_with([':', '/'])))
}

/// The current release of `channel` on [`feed`], once its signature checks out against `key`.
pub fn fetch_release(channel: &str, key: &[u8; 32]) -> anyhow::Result<Release> {
    let url = release_url(&feed(), channel);
    anyhow::ensure!(allowed(&url), "{}", orbiters_i18n::tr!("{url}: not an HTTPS address", url = url));
    let response = agent().get(&url).header("Cache-Control", "no-cache").call().with_context(|| url.clone())?;
    let bytes = response.into_body().with_config().limit(MAX_RELEASE).read_to_vec().with_context(|| url.clone())?;
    SignedRelease::open(&bytes, key, channel)
}

/// Whether asking for a release failed only because the channel has none here yet (`fetch_release`'s error), which is no
/// fault: a platform's first release is not out, say.
pub fn none_published(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| matches!(cause.downcast_ref::<ureq::Error>(), Some(ureq::Error::StatusCode(404))))
}

/// Downloads `artifact` (resolved against [`feed`]) to `to`, through `to` with `.part` added: a download that stopped
/// goes on from where it was, and only a whole file of the right size and SHA-256 takes `to`'s name. `progress` hears
/// the bytes there are of the size; `stop` ends it early (an error).
pub fn download(artifact: &Artifact, to: &Path, mut progress: impl FnMut(u64, u64), stop: &AtomicBool) -> anyhow::Result<()> {
    let url = artifact.resolve(&feed());
    anyhow::ensure!(allowed(&url), "{}", orbiters_i18n::tr!("{url}: not an HTTPS address", url = url));
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = to.with_file_name(format!("{}.part", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > artifact.size {
        std::fs::remove_file(&part)?;
        have = 0;
    }
    let going = || {
        anyhow::ensure!(!stop.load(Ordering::Relaxed), "stopped");
        Ok(())
    };
    let mut hasher = Sha256::new();
    if have > 0 {
        // What came before counts, hashed again: the hash covers the whole file.
        let mut file = std::fs::File::open(&part)?.take(have);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            going()?;
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }
    progress(have, artifact.size);
    going()?;
    if have < artifact.size {
        let mut request = agent().get(&url);
        if have > 0 {
            request = request.header("Range", &format!("bytes={have}-"));
        }
        let response = request.call().with_context(|| url.clone())?;
        let status = response.status().as_u16();
        anyhow::ensure!(status == 200 || status == 206, "{url}: HTTP {status}");
        let resumed = have > 0 && status == 206;
        if have > 0 && !resumed {
            // The server sends it whole: from the start.
            hasher = Sha256::new();
            have = 0;
        }
        let file = std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part)?;
        let mut file = std::io::BufWriter::with_capacity(1 << 20, file);
        // At most what is left, and a byte more to see a larger file.
        let mut reader = response.into_body().into_with_config().limit(artifact.size - have + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            going()?;
            let n = reader.read(&mut buf).with_context(|| url.clone())?;
            if n == 0 {
                break;
            }
            have += n as u64;
            if have > artifact.size {
                drop(file);
                let _ = std::fs::remove_file(&part);
                anyhow::bail!("{}", orbiters_i18n::tr!("{url}: larger than the release says", url = url));
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            progress(have, artifact.size);
        }
        file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }
    anyhow::ensure!(have == artifact.size, "{}", orbiters_i18n::tr!("{url}: the download stopped at {have} of {size} bytes", url = url, have = have, size = artifact.size));
    if crate::hex(&hasher.finalize()) != artifact.sha256 {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("{}", orbiters_i18n::tr!("{url}: not the file the release names (its SHA-256 differs)", url = url));
    }
    // Stopped: nothing is published; the whole part stays for the next time.
    going()?;
    // In one step: an earlier file there is replaced by this one or stays.
    std::fs::rename(&part, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate_key, hex, STABLE};
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};

    /// A small HTTP server on this computer: each path's body, a part of it for a `Range` request.
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || answer(stream, &routes));
            }
        });
        format!("http://{address}")
    }

    fn answer(mut stream: TcpStream, routes: &[(&str, Vec<u8>)]) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let path = first.split_whitespace().nth(1).unwrap_or_default().to_string();
        let mut from = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                from = range.trim().trim_end_matches('-').parse::<usize>().ok();
            }
        }
        let Some((_, body)) = routes.iter().find(|(p, _)| *p == path) else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        };
        let head = match from {
            Some(start) => format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                body.len() - start,
                body.len() - 1,
                body.len()
            ),
            None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
        };
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body[from.unwrap_or(0)..]);
    }

    #[test]
    fn a_channel_without_a_release_is_no_failure() {
        let not_there = anyhow::Error::from(ureq::Error::StatusCode(404)).context("https://orbiters.cc/downloads/desktop/macos/stable.json");
        assert!(none_published(&not_there));
        assert!(!none_published(&anyhow::Error::from(ureq::Error::StatusCode(500))));
        assert!(!none_published(&anyhow::anyhow!("signature")));
    }

    #[test]
    fn a_release_and_its_setup_come_from_the_feed() {
        let setup: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        let (private, public) = generate_key().unwrap();
        let release = Release {
            product: crate::PRODUCT.into(),
            channel: STABLE.into(),
            version: "1.2.3".into(),
            released: "2026-10-23T12:00:00Z".into(),
            commit: String::new(),
            notes: "Faster.".into(),
            installer: Artifact { url: "1.2.3/OrbitersSetup-1.2.3.exe".into(), sha256: hex(&Sha256::digest(&setup)), size: setup.len() as u64 },
        };
        let signed = serde_json::to_vec(&SignedRelease::sign(&release, &private).unwrap()).unwrap();
        let feed_file: &'static str = format!("/desktop/{}stable.json", crate::PLATFORM_FOLDER).leak();
        let base = serve(vec![(feed_file, signed), ("/desktop/1.2.3/OrbitersSetup-1.2.3.exe", setup.clone())]);
        // The only test here that reads the feed.
        std::env::set_var("ORBITERS_UPDATE_FEED", format!("{base}/desktop"));

        assert_eq!(fetch_release(STABLE, &public).unwrap(), release);
        let (_, stranger) = generate_key().unwrap();
        assert!(fetch_release(STABLE, &stranger).is_err(), "signed with another key");

        // A download cut short goes on from where it stopped.
        let dir = std::env::temp_dir().join(format!("orbiters-update-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("OrbitersSetup-1.2.3.exe");
        std::fs::write(dir.join("OrbitersSetup-1.2.3.exe.part"), &setup[..100_000]).unwrap();
        let mut seen = Vec::new();
        download(&release.installer, &to, |have, _| seen.push(have), &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), setup);
        assert_eq!(seen.first(), Some(&100_000), "went on from the part");
        assert_eq!(seen.last(), Some(&(setup.len() as u64)));
        assert!(!dir.join("OrbitersSetup-1.2.3.exe.part").exists());

        // Not the file the release names: refused, and nothing takes the name.
        let wrong = Artifact { sha256: "0".repeat(64), ..release.installer.clone() };
        let other = dir.join("other.exe");
        assert!(download(&wrong, &other, |_, _| {}, &AtomicBool::new(false)).is_err());
        assert!(!other.exists());
        // COR-13: stopped when everything is already here: nothing takes the name, and the part is kept for later.
        let whole = dir.join("whole.exe");
        std::fs::write(dir.join("whole.exe.part"), &setup).unwrap();
        let told = std::cell::Cell::new(false);
        assert!(download(&release.installer, &whole, |_, _| told.set(true), &AtomicBool::new(true)).is_err());
        assert!(!told.get() && !whole.exists() && dir.join("whole.exe.part").exists());
        // Stopped while the last of it comes.
        let stop = AtomicBool::new(false);
        assert!(download(&release.installer, &whole, |have, size| stop.store(have == size, Ordering::Relaxed), &stop).is_err());
        assert!(!whole.exists() && dir.join("whole.exe.part").exists());
        // COR-26: what was there stays while the new one cannot take its place (its part held by another program).
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::write(&whole, b"the one before").unwrap();
            let held = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(dir.join("whole.exe.part")).unwrap();
            assert!(download(&release.installer, &whole, |_, _| {}, &AtomicBool::new(false)).is_err());
            assert_eq!(std::fs::read(&whole).unwrap(), b"the one before");
            drop(held);
        }
        download(&release.installer, &whole, |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&whole).unwrap(), setup);
        // Stopped: an error, the part kept for later.
        assert!(download(&release.installer, &dir.join("stopped.exe"), |_, _| {}, &AtomicBool::new(true)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_https_or_this_computer() {
        assert!(allowed("https://orbiters.cc/downloads/desktop/stable.json"));
        assert!(allowed("http://127.0.0.1:8123/stable.json"));
        assert!(allowed("http://localhost/stable.json"));
        assert!(allowed("http://[::1]:80/stable.json"));
        assert!(!allowed("http://orbiters.cc/stable.json"));
        assert!(!allowed("http://127.0.0.1.example.com/stable.json"));
        assert!(!allowed("ftp://orbiters.cc/stable.json"));
    }
}
