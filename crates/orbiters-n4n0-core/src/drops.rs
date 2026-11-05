//! N4N0's link cards on Orbiters (contracts `Docs/n4n0.md`, Link card): a card dropped in a VRChat instance is told to
//! Orbiters, which knows the instance only by the SHA-256 its players compute from VRChat's log, so that whoever grabs it
//! there fetches its link (for twenty seconds, without an account). Only links a browser opens are ever copied.

use orbiters_core::account::{Api, ApiError};
use serde::Deserialize;
use std::time::Duration;

/// How long a request waits for Orbiters: a grab should copy at once, or not at all.
const TIMEOUT: Duration = Duration::from_secs(6);
/// The longest link copied (Orbiters keeps no longer ones).
pub const LINK_MAX: usize = 2000;

/// The newest card dropped in an instance, as Orbiters gives it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Dropped {
    pub link: String,
    #[serde(default)]
    pub title: String,
    /// When it was dropped (ISO 8601).
    #[serde(rename = "droppedAt", default)]
    pub dropped_at: String,
}

#[derive(Deserialize)]
struct Posted {
    #[serde(rename = "expiresAt", default)]
    expires_at: String,
}

/// Whether `link` may be put in the clipboard: an `http` or `https` link with a host, on one line, not too long.
pub fn copyable(link: &str) -> bool {
    let lower = link.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")) else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    link.len() <= LINK_MAX && !host.is_empty() && !host.starts_with('@') && !link.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// An error answer in the person's words: the server's own message, or its status.
fn plain(e: anyhow::Error) -> anyhow::Error {
    match e.downcast::<ApiError>() {
        Ok(api) if api.message.is_empty() => anyhow::anyhow!("Orbiters answered {}", api.status),
        Ok(api) => api.into(),
        Err(e) => e,
    }
}

/// Tells Orbiters a card with `link` and `title` was dropped in `instance` (its hash,
/// `orbiters_core::vrchat_log::WorldState::instance_hash`), as the account of `token`. Returns when the card expires
/// there (ISO 8601).
pub fn post(api: &Api, token: &str, instance: &str, link: &str, title: &str) -> anyhow::Result<String> {
    anyhow::ensure!(copyable(link), "This card has no link that can be opened.");
    let response = api
        .agent()
        .post(&api.url("n4n0/drops"))
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "instance": instance, "link": link, "title": title }))?;
    let posted: Posted = Api::json(response).map_err(plain)?;
    Ok(posted.expires_at)
}

/// The newest card dropped in `instance` within the last twenty seconds (`None`: none there), when its link may be
/// copied ([`copyable`]; another one is answered as none).
pub fn latest(api: &Api, instance: &str) -> anyhow::Result<Option<Dropped>> {
    let response = api.agent().get(&api.url(&format!("n4n0/drops/latest?instance={instance}"))).config().timeout_global(Some(TIMEOUT)).build().call()?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    let card: Dropped = Api::json(response).map_err(plain)?;
    Ok(Some(card).filter(|c| copyable(&c.link)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A request the stand-in Orbiters got: its first line, its Authorization header and its body.
    type Seen = Arc<Mutex<Vec<(String, Option<String>, String)>>>;

    /// A stand-in Orbiters answering each request with the next of `answers` (status, body).
    fn orbiters(answers: Vec<(u16, &'static str)>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let kept = seen.clone();
        std::thread::spawn(move || {
            let mut answers = answers.into_iter();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut first, mut auth, mut length) = (String::new(), None, 0usize);
                reader.read_line(&mut first).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        auth = Some(line["authorization:".len()..].trim().to_string());
                    }
                    line.clear();
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                kept.lock().unwrap().push((first.trim().to_string(), auth, String::from_utf8_lossy(&body).into_owned()));
                let (status, text) = answers.next().unwrap_or((500, "{}"));
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        (base, seen)
    }

    #[test]
    fn drops_a_card_with_the_account_and_fetches_the_newest_without_one() {
        let hash = "a".repeat(64);
        let (base, seen) = orbiters(vec![
            (201, r#"{ "expiresAt": "2026-10-25T10:00:20.000Z" }"#),
            (200, r#"{ "link": "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "title": "Never Gonna", "droppedAt": "2026-10-25T10:00:00.000Z" }"#),
            (404, r#"{ "error": "No card was dropped here in the last twenty seconds." }"#),
            (200, r#"{ "link": "javascript:alert(1)", "title": "x", "droppedAt": "t" }"#),
            (429, r#"{ "error": "Too many cards at once. Wait a moment and try again." }"#),
        ]);
        let api = Api::at(&base);
        let link = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        assert_eq!(post(&api, "orbit-test", &hash, link, "Never Gonna").unwrap(), "2026-10-25T10:00:20.000Z");
        let card = latest(&api, &hash).unwrap().expect("a card");
        assert_eq!((card.link.as_str(), card.title.as_str()), (link, "Never Gonna"));
        assert_eq!(latest(&api, &hash).unwrap(), None, "404: none there");
        assert_eq!(latest(&api, &hash).unwrap(), None, "a link no browser opens is never copied");
        assert_eq!(latest(&api, &hash).unwrap_err().to_string(), "Too many cards at once. Wait a moment and try again.");
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0, "POST /n4n0/drops HTTP/1.1");
        assert_eq!(seen[0].1.as_deref(), Some("Bearer orbit-test"));
        let body: serde_json::Value = serde_json::from_str(&seen[0].2).unwrap();
        assert_eq!(body, serde_json::json!({ "instance": hash, "link": link, "title": "Never Gonna" }));
        assert_eq!(seen[1].0, format!("GET /n4n0/drops/latest?instance={hash} HTTP/1.1"));
        assert_eq!(seen[1].1, None, "fetching needs no account");
    }

    #[test]
    fn only_web_links_are_copied_or_dropped() {
        for good in ["https://www.youtube.com/watch?v=dQw4w9WgXcQ", "http://example.com", "HTTPS://Example.com/a?b#c"] {
            assert!(copyable(good), "{good}");
        }
        for bad in ["javascript:alert(1)", "file:///C:/x", "https://", "https:///path", "https://a b.com", "ftp://x.com", "https://@x", "https://x.com/\n"] {
            assert!(!copyable(bad), "{bad}");
        }
        assert!(!copyable(&format!("https://x.com/{}", "a".repeat(LINK_MAX))));
        let api = Api::at("http://127.0.0.1:9/");
        assert!(post(&api, "t", &"a".repeat(64), "file:///etc/passwd", "x").is_err(), "refused before any request");
    }
}
