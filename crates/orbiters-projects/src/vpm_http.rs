//! The HTTP client package installs use: a private repository's headers (its credentials) go to that repository's own
//! origin only (AS-11). vrc-get sends a repository's headers with every archive of its listing, and an archive is often
//! on another host (a CDN, a storage bucket, GitHub's release assets), which must never receive them; nor must a host a
//! redirect leads to.
use futures::TryStreamExt as _;
use indexmap::IndexMap;
use std::io;
use url::Url;
use vrc_get_vpm::HttpClient;

/// How many redirects an archive download follows, as reqwest's default does.
const REDIRECTS: usize = 10;

/// A header a person configured on a repository, and the origin it is meant for.
#[derive(Clone, Debug)]
struct Scoped {
    name: String,
    value: String,
    origin: Option<url::Origin>,
}

pub(crate) struct Client {
    /// Listings: the repository's own address, with its own headers (vrc-get's behaviour, unchanged).
    plain: reqwest::Client,
    /// Archives: redirects followed here, one hop at a time, so that each hop's headers are chosen again.
    archives: reqwest::Client,
    scoped: Vec<Scoped>,
}

impl Client {
    pub(crate) fn new(plain: reqwest::Client, settings: &vrc_get_vpm::environment::Settings) -> anyhow::Result<Self> {
        let scoped = settings
            .get_user_repos()
            .iter()
            .flat_map(|r| {
                let origin = r.url().map(Url::origin);
                r.headers().iter().map(move |(k, v)| Scoped { name: k.to_string(), value: v.to_string(), origin: origin.clone() })
            })
            .collect();
        Ok(Self { plain, archives: archive_client()?, scoped })
    }
}

fn archive_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("Orbiters-N4N0/{}", orbiters_core::app_version()))
        .timeout(std::time::Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// The headers that may go to `url`: a header configured on a repository only when `url` is on that repository's origin
/// (a header no repository configured, a listing's own per-package header, passes).
fn allowed<'a>(scoped: &[Scoped], url: &Url, headers: &IndexMap<&'a str, &'a str>) -> Vec<(&'a str, &'a str)> {
    let origin = url.origin();
    headers
        .iter()
        .filter(|(name, value)| {
            let configured: Vec<&Scoped> = scoped.iter().filter(|s| s.name.eq_ignore_ascii_case(name) && s.value == **value).collect();
            configured.is_empty() || configured.iter().any(|s| s.origin.as_ref().is_some_and(|o| *o == origin && o.is_tuple()))
        })
        .map(|(n, v)| (*n, *v))
        .collect()
}

impl HttpClient for Client {
    async fn get(&self, url: &Url, headers: &IndexMap<&str, &str>) -> io::Result<impl futures::AsyncRead + Send> {
        let mut url = url.clone();
        for _ in 0..=REDIRECTS {
            let mut request = self.archives.get(url.clone());
            for (name, value) in allowed(&self.scoped, &url, headers) {
                request = request.header(name, value);
            }
            let response = request.send().await.map_err(io::Error::other)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .ok_or_else(|| io::Error::other(format!("{url} redirects nowhere")))?; // i18n: not shown (vrc-get's error chain)
                url = url.join(location).map_err(io::Error::other)?;
                continue;
            }
            let response = response.error_for_status().map_err(io::Error::other)?;
            return Ok(response.bytes_stream().map_err(io::Error::other).into_async_read());
        }
        Err(io::Error::other(format!("{url}: too many redirects"))) // i18n: not shown (vrc-get's error chain)
    }

    async fn get_with_etag(
        &self,
        url: &Url,
        headers: &IndexMap<Box<str>, Box<str>>,
        current_etag: Option<&str>,
    ) -> io::Result<Option<(impl futures::AsyncRead + Send, Option<Box<str>>)>> {
        self.plain.get_with_etag(url, headers, current_etag).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scoped(name: &str, value: &str, repository: &str) -> Scoped {
        Scoped { name: name.into(), value: value.into(), origin: Some(Url::parse(repository).unwrap().origin()) }
    }

    #[test]
    fn a_repositorys_credential_goes_to_its_own_origin_only() {
        let scoped = vec![scoped("Authorization", "Bearer secret", "https://vpm.example.com/index.json")];
        let headers = IndexMap::from_iter([("Authorization", "Bearer secret"), ("X-Listing", "per-package")]);
        let names = |url: &str| allowed(&scoped, &Url::parse(url).unwrap(), &headers).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names("https://vpm.example.com/pkg/1.0.0.zip"), ["Authorization", "X-Listing"]);
        assert_eq!(names("https://cdn.example.net/pkg/1.0.0.zip"), ["X-Listing"], "another host");
        assert_eq!(names("http://vpm.example.com/pkg/1.0.0.zip"), ["X-Listing"], "another scheme");
        assert_eq!(names("https://vpm.example.com:8443/pkg.zip"), ["X-Listing"], "another port");
        // The same name with another value is not the credential: a listing's own header.
        let other = IndexMap::from_iter([("authorization", "Bearer listing")]);
        assert_eq!(allowed(&scoped, &Url::parse("https://cdn.example.net/a.zip").unwrap(), &other).len(), 1);
        // A header name matched whatever its case.
        let lower = IndexMap::from_iter([("authorization", "Bearer secret")]);
        assert!(allowed(&scoped, &Url::parse("https://cdn.example.net/a.zip").unwrap(), &lower).is_empty());
    }
}
