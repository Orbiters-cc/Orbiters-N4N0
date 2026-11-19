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
    /// Archives: redirects followed here, one hop at a time, so that each hop's headers are chosen again.
    archives: reqwest::Client,
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
        Ok(Self { archives: archive_client()?, scoped })
    }
}

fn archive_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("Orbiters-N4N0/{}", orbiters_core::app_version()))
        .timeout(std::time::Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// The headers that may go to `url`: only a pair the person's settings configure on a repository whose origin is
/// `url`'s (verifier, point 5). Anything else is dropped: a pair kept in an older cached listing after the settings
/// changed, a listing's own per-package header, a credential on its way to another host through a redirect.
fn allowed<'a>(scoped: &[Scoped], url: &Url, headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<(&'a str, &'a str)> {
    let origin = url.origin();
    headers
        .into_iter()
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
        let response = fetch(&self.archives, &self.scoped, url, &headers, None).await?.error_for_status().map_err(io::Error::other)?;
        Ok(response.bytes_stream().map_err(io::Error::other).into_async_read())
    }

    /// A listing: the same rule as an archive, hop by hop (reqwest would carry a custom header across hosts).
    async fn get_with_etag(
        &self,
        url: &Url,
        headers: &IndexMap<Box<str>, Box<str>>,
        current_etag: Option<&str>,
    ) -> io::Result<Option<(impl futures::AsyncRead + Send, Option<Box<str>>)>> {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_ref(), v.as_ref())).collect();
        let response = fetch(&self.archives, &self.scoped, url, &headers, current_etag).await?;
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
        // A header name matched whatever its case.
        let lower = [("authorization", "Bearer secret")];
        assert_eq!(allowed(&scoped, &Url::parse("https://vpm.example.com/a.zip").unwrap(), lower).len(), 1);
    }
}
