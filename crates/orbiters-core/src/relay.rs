//! An [`Api`] whose requests another process sends (verify4-r21 P3): a window has no credential. What it asks of Orbiters goes,
//! as it would over HTTP, to the service, which sends it as the member the window shows, or refuses.

use crate::account::Api;
use std::sync::Arc;

/// A request of Orbiters' API, its address relative to it, without any credential.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub method: String,
    /// The path and query, with no leading slash.
    pub path: String,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub body: Vec<u8>,
}

/// Orbiters' answer to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answered {
    pub status: u16,
    pub content_type: Option<String>,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
    /// Instead of `body`, when it is large: the file it was put in, removed once it is read.
    pub file: Option<std::path::PathBuf>,
}

/// A large answer, read from the file it was put in, which goes with it.
struct Spooled {
    file: std::fs::File,
    path: std::path::PathBuf,
}

impl std::io::Read for Spooled {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Drop for Spooled {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Where the requests go; an error says why nothing was sent.
pub trait Relay: Send + Sync + 'static {
    fn send(&self, asked: Asked) -> Result<Answered, String>;
}

/// Why a relayed request was not sent.
#[derive(Debug)]
struct NotSent(String);

impl std::fmt::Display for NotSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotSent {}

/// The largest request body relayed (nothing a page asks is near it; the link carries it in base64, 4 MB at most).
const BODY_MOST: u64 = 2 << 20;

/// The client's middleware: no credential leaves this process, and the requests under `root` are the relay's.
fn relaying(root: String, relay: Arc<dyn Relay>) -> impl ureq::middleware::Middleware {
    move |mut request: ureq::http::Request<ureq::SendBody>, next: ureq::middleware::MiddlewareNext| {
        let url = request.uri().to_string();
        request.headers_mut().remove(ureq::http::header::AUTHORIZATION);
        let Some(path) = url.strip_prefix(root.as_str()) else { return next.handle(request) };
        let header = |name: ureq::http::HeaderName| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (content_type, accept) = (header(ureq::http::header::CONTENT_TYPE), header(ureq::http::header::ACCEPT));
        let (method, path) = (request.method().to_string(), path.to_string());
        let mut body = Vec::new();
        {
            use std::io::Read;
            request.into_body().into_reader().take(BODY_MOST + 1).read_to_end(&mut body).map_err(ureq::Error::Io)?;
        }
        if body.len() as u64 > BODY_MOST {
            return Err(ureq::Error::BodyExceedsLimit(BODY_MOST));
        }
        let answered = relay.send(Asked { method, path, content_type, accept, body }).map_err(|e| ureq::Error::Other(Box::new(NotSent(e))))?;
        let mut response = ureq::http::Response::builder().status(answered.status);
        if let Some(t) = &answered.content_type {
            response = response.header(ureq::http::header::CONTENT_TYPE, t);
        }
        if let Some(after) = &answered.retry_after {
            response = response.header(ureq::http::header::RETRY_AFTER, after);
        }
        let mime = answered.content_type.unwrap_or_else(|| "application/octet-stream".into());
        let body = ureq::Body::builder().mime_type(mime);
        let body = match answered.file {
            Some(path) => {
                let file = std::fs::File::open(&path).map_err(ureq::Error::Io)?;
                body.reader(Spooled { file, path })
            }
            None => body.data(answered.body),
        };
        response.body(body).map_err(ureq::Error::Http)
    }
}

impl Api {
    /// The API at `base`, its own requests sent by `relay`; what is elsewhere (a picture's link) is asked directly, with no
    /// credential whatever the caller put.
    pub fn relayed(base: &str, relay: Arc<dyn Relay>) -> Self {
        let base = format!("{}/", base.trim_end_matches('/'));
        let root = base.clone();
        let agent = ureq::Agent::config_builder()
            .middleware(relaying(root, relay))
            .middleware(crate::origin::guard)
            .tls_config(crate::http_tls())
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(120)))
            .user_agent(format!("Orbiters-N4N0/{}", crate::app_version()))
            .build()
            .new_agent();
        Self::with_agent(agent, base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// A relay that keeps what it was asked and answers `answer`.
    struct Kept(Mutex<Vec<Asked>>, Result<Answered, String>);

    impl Relay for Kept {
        fn send(&self, asked: Asked) -> Result<Answered, String> {
            self.0.lock().unwrap().push(asked);
            self.1.clone()
        }
    }

    /// The API's own functions work as they are; no credential is in what is relayed, whatever the caller gave.
    #[test]
    fn the_apis_functions_go_through_the_relay_without_a_credential() {
        let kept = Arc::new(Kept(Mutex::default(), Ok(Answered { status: 200, content_type: Some("application/json".into()), retry_after: None, body: br#"{"items":{"s1":"sent"},"reasons":{}}"#.to_vec(), file: None })));
        let api = Api::relayed("https://api.invalid/v1", kept.clone());
        let invites = api.my_invites("whatever", "ev1").unwrap();
        assert_eq!(invites.items.get("s1").map(String::as_str), Some("sent"));
        let _ = api.agent().post(&api.url("a/b?c=d")).header("Authorization", "Bearer secret").send_json(serde_json::json!({ "x": 1 }));
        let asked = kept.0.lock().unwrap().clone();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].method, "GET");
        assert!(!asked[0].path.starts_with('/') && asked[0].path.contains("ev1"), "{asked:?}");
        assert_eq!((asked[1].method.as_str(), asked[1].path.as_str()), ("POST", "a/b?c=d"));
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&asked[1].body).unwrap()["x"], 1);
        assert!(!format!("{asked:?}").contains("secret") && !format!("{asked:?}").contains("whatever"));
    }

    /// Verify5-r21 m5: a large answer comes in a file, read as any answer is, and gone once it was; a body too large for the link
    /// is refused here.
    #[test]
    fn a_large_answer_comes_in_a_file() {
        let path = std::env::temp_dir().join(format!("orbiters-relay-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; 5 << 20]).unwrap();
        let api = Api::relayed("https://api.invalid/v1", Arc::new(Kept(Mutex::default(), Ok(Answered { status: 200, content_type: Some("image/png".into()), file: Some(path.clone()), ..Default::default() }))));
        let mut response = api.agent().get(&api.url("a")).call().unwrap();
        let read = response.body_mut().with_config().limit(64 << 20).read_to_vec().unwrap();
        assert!(read.len() == 5 << 20 && read.iter().all(|b| *b == 7));
        drop(response);
        assert!(!path.exists());
        assert!(api.agent().post(&api.url("a")).send(&vec![0u8; (BODY_MOST + 1) as usize][..]).is_err());
    }

    /// A refusal is the request's error, in its words; nothing is asked of anyone.
    #[test]
    fn a_refusal_says_why() {
        let api = Api::relayed("https://api.invalid/v1", Arc::new(Kept(Mutex::default(), Err("Another member is signed in.".into()))));
        let e = api.my_invites("t", "ev1").unwrap_err();
        assert!(format!("{e:#}").contains("Another member is signed in."), "{e:#}");
    }

    /// What is elsewhere is asked directly, with no credential.
    #[test]
    fn another_origin_is_asked_directly_and_given_nothing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
            String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase()
        });
        let kept = Arc::new(Kept(Mutex::default(), Err("no".into())));
        let api = Api::relayed("https://api.invalid/v1", kept.clone());
        api.agent().get(&base).header("Authorization", "Bearer secret").call().unwrap();
        assert!(!server.join().unwrap().contains("secret"));
        assert!(kept.0.lock().unwrap().is_empty());
    }
}
