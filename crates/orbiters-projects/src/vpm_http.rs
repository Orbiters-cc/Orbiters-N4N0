//! The HTTP client package installs use: a private repository's headers (its credentials) go to that repository's own
//! origin only (AS-11). vrc-get sends a repository's headers with every archive of its listing, and an archive is often
//! on another host (a CDN, a storage bucket, GitHub's release assets), which must never receive them; nor must a host a
//! redirect leads to.
use futures::TryStreamExt as _;
use indexmap::IndexMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use vrc_get_vpm::HttpClient;

/// How many redirects an archive download follows, as reqwest's default does.
const REDIRECTS: usize = 10;

/// How long a host may take to answer at all, and how long an archive may stop arriving. An archive as a whole takes what its line
/// needs (PRJ-22: 90 s for all of it was never enough for a large package on a slow line, however well it came).
const CONNECT: Duration = Duration::from_secs(20);
const IDLE: Duration = Duration::from_secs(60);
/// A listing is small, and every look at the repositories waits for the slowest: all of it within this.
const LISTING: Duration = Duration::from_secs(90);
/// How many archives come at once: they share one line, and vrc-get asks for every package of a change together.
const AT_ONCE: usize = 4;

/// A header a person configured on a repository, and the origin it is meant for.
#[derive(Clone, Debug)]
struct Scoped {
    name: String,
    value: String,
    origin: Option<url::Origin>,
}

pub(crate) struct Client {
    /// Redirects are followed here, one hop at a time, so that each hop's headers are chosen again.
    archives: reqwest::Client,
    listings: reqwest::Client,
    lines: Arc<tokio::sync::Semaphore>,
    scoped: Vec<Scoped>,
}

impl Client {
    /// Also allows `headers` to go to `url`'s origin: a repository being added, not in the settings yet.
    pub(crate) fn with_new_repository(mut self, url: &Url, headers: &std::collections::BTreeMap<String, String>) -> Self {
        self.scoped.extend(headers.iter().map(|(k, v)| Scoped { name: k.clone(), value: v.clone(), origin: Some(url.origin()) }));
        self
    }

    pub(crate) fn new(plain: reqwest::Client, settings: &vrc_get_vpm::environment::Settings) -> anyhow::Result<Self> {
        let scoped = settings
            .get_user_repos()
            .iter()
            .flat_map(|r| {
                let origin = r.url().map(Url::origin);
                r.headers().iter().map(move |(k, v)| Scoped { name: k.to_string(), value: v.to_string(), origin: origin.clone() })
            })
            .collect();
        let _ = plain;
        Ok(Self { archives: archives(IDLE)?, listings: client().timeout(LISTING).build()?, lines: Arc::new(tokio::sync::Semaphore::new(AT_ONCE)), scoped })
    }
}

fn client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(format!("Orbiters-N4N0/{}", orbiters_core::app_version()))
        .connect_timeout(CONNECT)
        .redirect(reqwest::redirect::Policy::none())
}

/// Given up once nothing came for `idle`, however long the whole takes.
fn archives(idle: Duration) -> reqwest::Result<reqwest::Client> {
    client().read_timeout(idle).build()
}

/// The headers that may go to `url`: only a pair the person's settings configure on a repository whose origin is
/// `url`'s (verifier, point 5). Anything else is dropped: a pair kept in an older cached listing after the settings
/// changed, a listing's own per-package header, a credential on its way to another host through a redirect.
fn allowed<'a>(scoped: &[Scoped], url: &Url, headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<(&'a str, &'a str)> {
    let origin = url.origin();
    headers
        .into_iter()
        // Every request names Orbiters: a repository's configured headers never name another program in its place.
        .filter(|(name, _)| !name.eq_ignore_ascii_case("User-Agent"))
        .filter(|(name, value)| scoped.iter().any(|s| s.name.eq_ignore_ascii_case(name) && s.value == *value && s.origin.as_ref().is_some_and(|o| *o == origin && o.is_tuple())))
        .collect()
}

/// The answer to `url` with the headers each hop may have, redirects followed one at a time ([`allowed`] again at each).
async fn fetch(client: &reqwest::Client, scoped: &[Scoped], url: &Url, headers: &[(&str, &str)], etag: Option<&str>) -> io::Result<reqwest::Response> {
    let first = url.origin();
    let mut url = url.clone();
    for _ in 0..=REDIRECTS {
        let mut request = client.get(url.clone());
        for (name, value) in allowed(scoped, &url, headers.iter().copied()) {
            request = request.header(name, value);
        }
        // A listing's validator is that host's: never sent to another one a redirect leads to.
        if let Some(etag) = etag.filter(|_| url.origin() == first) {
            request = request.header("If-None-Match", etag);
        }
        let response = request.send().await.map_err(io::Error::other)?;
        if response.status().is_redirection() && response.status() != reqwest::StatusCode::NOT_MODIFIED {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .ok_or_else(|| io::Error::other(format!("{url} redirects nowhere")))?; // i18n: not shown (vrc-get's error chain)
            url = url.join(location).map_err(io::Error::other)?;
            continue;
        }
        return Ok(response);
    }
    Err(io::Error::other(format!("{url}: too many redirects"))) // i18n: not shown (vrc-get's error chain)
}

impl HttpClient for Client {
    async fn get(&self, url: &Url, headers: &IndexMap<&str, &str>) -> io::Result<impl futures::AsyncRead + Send> {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, *v)).collect();
        let line = self.lines.clone().acquire_owned().await.map_err(io::Error::other)?;
        let response = fetch(&self.archives, &self.scoped, url, &headers, None).await?.error_for_status().map_err(io::Error::other)?;
        // The line is held as long as the archive comes.
        let held = move |bytes| {
            let _ = &line;
            bytes
        };
        Ok(response.bytes_stream().map_err(io::Error::other).map_ok(held).into_async_read())
    }

    /// A listing: the same rule as an archive, hop by hop (reqwest would carry a custom header across hosts).
    async fn get_with_etag(
        &self,
        url: &Url,
        headers: &IndexMap<Box<str>, Box<str>>,
        current_etag: Option<&str>,
    ) -> io::Result<Option<(impl futures::AsyncRead + Send, Option<Box<str>>)>> {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_ref(), v.as_ref())).collect();
        let response = fetch(&self.listings, &self.scoped, url, &headers, current_etag).await?;
        if current_etag.is_some() && response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let response = response.error_for_status().map_err(io::Error::other)?;
        let etag = response.headers().get("Etag").and_then(|x| x.to_str().ok()).map(Into::into);
        Ok(Some((response.bytes_stream().map_err(io::Error::other).into_async_read(), etag)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scoped(name: &str, value: &str, repository: &str) -> Scoped {
        Scoped { name: name.into(), value: value.into(), origin: Some(Url::parse(repository).unwrap().origin()) }
    }

    /// A server of one archive: what it sends of it and how, and how many it sent at once at most.
    fn server(answer: impl Fn(&mut std::net::TcpStream) + Send + Sync + 'static) -> (Url, Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Read as _;
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/a.zip", listener.local_addr().unwrap())).unwrap();
        let (answer, now, most) = (Arc::new(answer), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let seen = most.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let (answer, now, most) = (answer.clone(), now.clone(), most.clone());
                std::thread::spawn(move || {
                    let mut asked = [0u8; 4096];
                    if !stream.read(&mut asked).is_ok_and(|n| n > 0) {
                        return;
                    }
                    most.fetch_max(now.fetch_add(1, SeqCst) + 1, SeqCst);
                    answer(&mut stream);
                    now.fetch_sub(1, SeqCst);
                });
            }
        });
        (url, seen)
    }
    /// `parts` of 10 bytes, one after each `pause`; whole when they are what was promised.
    fn parts(stream: &mut std::net::TcpStream, promised: usize, parts: usize, pause: Duration) {
        use std::io::Write as _;
        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", promised * 10);
        for _ in 0..parts {
            std::thread::sleep(pause);
            if stream.write_all(&[7u8; 10]).and_then(|()| stream.flush()).is_err() {
                return;
            }
        }
        if parts < promised {
            // It stops there, its line open.
            std::thread::sleep(Duration::from_secs(20));
        }
    }
    fn client_of(idle: Duration) -> Client {
        Client { archives: archives(idle).unwrap(), listings: client().build().unwrap(), lines: Arc::new(tokio::sync::Semaphore::new(AT_ONCE)), scoped: vec![] }
    }
    async fn download(client: &Client, url: &Url) -> io::Result<usize> {
        use futures::AsyncReadExt as _;
        let mut bytes = vec![];
        let none = IndexMap::new();
        let body = client.get(url, &none).await?;
        futures::pin_mut!(body);
        body.read_to_end(&mut bytes).await?;
        Ok(bytes.len())
    }

    /// PRJ-22: an archive that keeps coming is never cut, however long it takes; one that stops coming is given up.
    #[test]
    fn an_archive_is_given_up_when_it_stops_and_not_when_it_is_long() {
        let client = client_of(Duration::from_millis(1500));
        crate::runtime().unwrap().block_on(async {
            let (slow, _) = server(|s| parts(s, 20, 20, Duration::from_millis(200)));
            assert_eq!(download(&client, &slow).await.unwrap(), 200, "four seconds, never one and a half without a byte");
            let started = std::time::Instant::now();
            let (stuck, _) = server(|s| parts(s, 20, 3, Duration::from_millis(50)));
            assert!(download(&client, &stuck).await.is_err());
            assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        });
    }

    /// The same with the times the app has: an archive of 95 s.
    #[test]
    #[ignore = "takes 95 seconds"]
    fn an_archive_may_take_longer_than_ninety_seconds() {
        let temp = tempfile::tempdir().unwrap();
        crate::runtime().unwrap().block_on(async {
            let io = vrc_get_vpm::io::DefaultEnvironmentIo::new(temp.path().into());
            let settings = vrc_get_vpm::environment::Settings::load(&io).await.unwrap();
            let client = Client::new(reqwest::Client::new(), &settings).unwrap();
            let (slow, _) = server(|s| parts(s, 95, 95, Duration::from_secs(1)));
            assert_eq!(download(&client, &slow).await.unwrap(), 950);
        });
    }

    /// A change of many packages downloads a few at a time.
    #[test]
    fn a_few_archives_come_at_once() {
        let client = client_of(Duration::from_secs(5));
        let (url, most) = server(|s| parts(s, 4, 4, Duration::from_millis(50)));
        let all = crate::runtime().unwrap().block_on(futures::future::join_all((0..15).map(|_| download(&client, &url))));
        assert!(all.into_iter().all(|n| n.unwrap() == 40));
        assert_eq!(most.load(std::sync::atomic::Ordering::SeqCst), AT_ONCE);
    }

    #[test]
    fn a_repositorys_credential_goes_to_its_own_origin_only() {
        let scoped = vec![scoped("Authorization", "Bearer secret", "https://vpm.example.com/index.json")];
        let headers = [("Authorization", "Bearer secret"), ("X-Listing", "per-package")];
        let names = |url: &str| allowed(&scoped, &Url::parse(url).unwrap(), headers).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names("https://vpm.example.com/pkg/1.0.0.zip"), ["Authorization"], "only what the settings configure");
        assert!(names("https://cdn.example.net/pkg/1.0.0.zip").is_empty(), "another host");
        assert!(names("http://vpm.example.com/pkg/1.0.0.zip").is_empty(), "another scheme");
        assert!(names("https://vpm.example.com:8443/pkg.zip").is_empty(), "another port");
        // A pair the settings no longer have (an older cached listing's) goes nowhere, not even to its own host.
        let stale = [("Authorization", "Bearer old")];
        assert!(allowed(&scoped, &Url::parse("https://vpm.example.com/a.zip").unwrap(), stale).is_empty());
        // What names the program asking is Orbiters' own, whatever a repository's settings say.
        let other = vec![super::tests::scoped("User-Agent", "VCC/1.0", "https://vpm.example.com/index.json")]
            .into_iter()
            .chain([Scoped { name: "user-agent".into(), value: "VCC/1.0".into(), origin: Some(Url::parse("https://vpm.example.com/").unwrap().origin()) }])
            .collect::<Vec<_>>();
        assert!(allowed(&other, &Url::parse("https://vpm.example.com/a.zip").unwrap(), [("User-Agent", "VCC/1.0"), ("user-agent", "VCC/1.0")]).is_empty());
        // A header name matched whatever its case.
        let lower = [("authorization", "Bearer secret")];
        assert_eq!(allowed(&scoped, &Url::parse("https://vpm.example.com/a.zip").unwrap(), lower).len(), 1);
    }
}
