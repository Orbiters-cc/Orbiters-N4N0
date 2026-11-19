//! Package icons for the Unity projects page, best first: the logo a creator set on Orbiters for a package of a listing
//! Orbiters hosts ([`crate::network::package_logo`]), else the icon of its author's website (`author.url`), else of its
//! own (`siteUrl`). A website's icon is the largest its page declares (`<link rel="icon">`, `apple-touch-icon`, the web
//! manifest's icons), else its `/favicon.ico`; a GitHub address gives its owner's picture rather than GitHub's mark.
//!
//! Workers only. Each site costs at most a page, a manifest and a few images, every response is size-limited, and the
//! answer (the icon at most 128 px, or that there is none) is kept under `Orbiters/package-icons` for a week (a day
//! when none was found) so pages reopen without the network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

/// The longest side an icon is kept at.
pub const SIDE: u32 = 128;
const FOUND_FOR: u64 = 7 * 86400;
const MISSING_FOR: u64 = 86400;
const PAGE_LIMIT: usize = 512 * 1024;
const MANIFEST_LIMIT: usize = 128 * 1024;
const IMAGE_LIMIT: usize = 2 * 1024 * 1024;
/// Icons tried from one page (largest first) before its /favicon.ico.
const CANDIDATES: usize = 4;

/// Where a package's icon can come from, best first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sources {
    /// The logo set on Orbiters (an absolute URL).
    pub logo: Option<String>,
    /// Websites whose icon stands for it: its author's, then its own.
    pub sites: Vec<String>,
}

/// An icon, straight (not premultiplied) RGBA.
#[derive(Clone, Debug)]
pub struct Image {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}
impl Image {
    /// Solid nearly everywhere (an avatar, an app icon with its own rounded corners): a picture that fills its tile
    /// rather than a mark to frame.
    pub fn fills(&self) -> bool {
        let solid = self.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 200).count();
        solid * 100 >= self.rgba.len() / 4 * 86
    }
    /// The mean lightness (0 to 1) of what it draws: a dark mark needs a light tile behind it.
    pub fn lightness(&self) -> f32 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for p in self.rgba.as_chunks::<4>().0 {
            let a = p[3] as f32 / 255.0;
            sum += a * (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
            weight += a;
        }
        if weight > 0.0 {
            sum / weight
        } else {
            1.0
        }
    }
}

pub fn cache_dir() -> PathBuf {
    crate::data_dir().join("package-icons")
}

/// Where `package`'s icon can come from (blocking: the logos of the Orbiters listing it is in, kept for an hour).
pub fn sources(package: &crate::vpm::Package) -> Sources {
    Sources {
        logo: package.repository.as_deref().and_then(|listing| crate::network::package_logo(listing, &package.name)),
        sites: [&package.author_url, &package.site_url].into_iter().flatten().cloned().collect(),
    }
}

/// Where a package repository's icon can come from (blocking, as [`sources`]): the logo Orbiters keeps for a listing
/// it hosts, else the website the listing is on (its host, then the domain that host belongs to: packages.vrchat.com,
/// then vrchat.com), else `authors`, the websites of its packages' authors.
pub fn repository_sources(listing: &str, authors: &[String]) -> Sources {
    let mut sites = vec![];
    if let Some(url) = site_url(listing) {
        if let Some(host) = url.host_str() {
            let scheme = url.scheme();
            sites.push(format!("{scheme}://{host}/"));
            let labels: Vec<&str> = host.split('.').collect();
            if labels.len() > 2 && !host.ends_with(".github.io") && labels.iter().all(|l| !l.is_empty() && !l.bytes().all(|b| b.is_ascii_digit())) {
                sites.push(format!("{scheme}://{}/", labels[labels.len() - 2..].join(".")));
            }
        }
    }
    for a in authors {
        if !sites.contains(a) {
            sites.push(a.clone());
        }
    }
    Sources { logo: crate::network::listing_logo(listing), sites }
}

/// The icon of each entry of `list` (in its order): answers on disk first, the network for those out of date, four at
/// a time. Entries with the same sources are looked up once.
pub fn resolve_all(list: &[Sources]) -> Vec<Option<Image>> {
    resolve_all_in(&cache_dir(), list)
}

pub fn resolve_all_in(cache: &Path, list: &[Sources]) -> Vec<Option<Image>> {
    let mut unique: Vec<Sources> = vec![];
    for s in list {
        if !unique.contains(s) {
            unique.push(s.clone());
        }
    }
    let Ok(rt) = crate::runtime() else { return list.iter().map(|_| None).collect() };
    let found: HashMap<Sources, Option<Image>> = rt.block_on(async {
        let client = client();
        let public = public_client();
        let mut found = HashMap::new();
        for chunk in unique.chunks(4) {
            let tasks: Vec<_> = chunk
                .iter()
                .map(|s| {
                    let (s, cache, client, public) = (s.clone(), cache.to_path_buf(), client.clone(), public.clone());
                    tokio::spawn(async move {
                        let image = resolve(&cache, &s, client.as_ref(), public.as_ref()).await;
                        (s, image)
                    })
                })
                .collect();
            for t in tasks {
                if let Ok((s, image)) = t.await {
                    found.insert(s, image);
                }
            }
        }
        found
    });
    list.iter().map(|s| found.get(s).cloned().flatten()).collect()
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("Orbiters-N4N0/{} (package icons)", orbiters_core::app_version())) // i18n: not shown (a user agent)
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .ok()
}

/// The client for websites a repository's listing names (PROJECTS-13): any listing can name any address, so these
/// requests never reach this computer or its network (by address, by name resolving to one, through a redirect or a
/// proxy). An icon there falls back to the next source.
fn public_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("Orbiters-N4N0/{} (package icons)", orbiters_core::app_version())) // i18n: not shown (a user agent)
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(12))
        .no_proxy()
        .dns_resolver(std::sync::Arc::new(PublicResolver))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !public_target(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .ok()
}

/// Names resolved to public addresses only.
struct PublicResolver;
impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| public_ip(a.ip())).collect();
            if found.is_empty() {
                return Err(std::io::Error::other(format!("{host} has no public address")).into()); // i18n: not shown (a request error)
            }
            Ok(Box::new(found.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Whether `ip` is on the internet: not this computer, its network, link-local, shared (CGNAT), unique-local,
/// multicast, documentation or unspecified.
pub fn public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (18..20).contains(&b))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || first == 0x2001 && v6.segments()[1] == 0x0db8)
        }
    }
}

/// Whether a request to `url` may leave for the internet: an address that is public, or a name not kept for local
/// networks (its addresses are checked again when it is resolved).
pub fn public_target(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => public_ip(ip.into()),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            let local = ["localhost", "local", "lan", "internal", "home.arpa", "intranet", "corp"];
            name.contains('.') && !local.iter().any(|l| name == *l || name.ends_with(&format!(".{l}")))
        }
        None => false,
    }
}

/// Where requests go: the Orbiters server's own addresses (a logo; the development server is on this computer), or a
/// website a listing names, public only.
struct Web<'a> {
    client: &'a reqwest::Client,
    public: bool,
}

async fn resolve(cache: &Path, s: &Sources, client: Option<&reqwest::Client>, public: Option<&reqwest::Client>) -> Option<Image> {
    if let Some(logo) = &s.logo {
        if let Some(image) = cached_or(cache, logo, client, |c| async move { image_at(&Web { client: c, public: false }, &Url::parse(logo).ok()?).await }).await {
            return Some(image);
        }
    }
    for site in &s.sites {
        let Some(url) = site_url(site) else { continue };
        let at = url.clone();
        if let Some(image) = cached_or(cache, url.as_str(), public, |c| async move { site_icon(&Web { client: c, public: true }, at).await }).await {
            return Some(image);
        }
    }
    None
}

/// The answer kept for `key` while it is fresh, else `fetch`'s (kept for next time). A fetch that fails keeps an icon
/// found before.
async fn cached_or<'a, F, Fut>(cache: &Path, key: &str, client: Option<&'a reqwest::Client>, fetch: F) -> Option<Image>
where
    F: FnOnce(&'a reqwest::Client) -> Fut,
    Fut: std::future::Future<Output = Option<Image>>,
{
    let stem = cache.join(format!("{:016x}", fnv(key)));
    let png = stem.with_extension("png");
    let checked = std::fs::read(stem.with_extension("json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let kept = || load_png(&png);
    if let Some(v) = &checked {
        let age = crate::discovery::now().saturating_sub(v["checked"].as_u64().unwrap_or(0));
        let found = v["found"].as_bool().unwrap_or(false);
        if age < if found { FOUND_FOR } else { MISSING_FOR } {
            return if found { kept() } else { None };
        }
    }
    let Some(client) = client else { return kept() };
    let image = fetch(client).await;
    let _ = std::fs::create_dir_all(cache);
    match &image {
        Some(i) => {
            let tmp = stem.with_extension("png.tmp");
            if image::save_buffer_with_format(&tmp, &i.rgba, i.size[0], i.size[1], image::ExtendedColorType::Rgba8, image::ImageFormat::Png).is_ok() {
                let _ = std::fs::rename(&tmp, &png);
            }
        }
        // Nothing found where an icon was before (the site down for a moment): keep showing it, ask again tomorrow.
        None if png.is_file() => {
            let _ = crate::write_json(
                &stem.with_extension("json"),
                &serde_json::json!({ "key": key, "checked": crate::discovery::now() - FOUND_FOR + MISSING_FOR, "found": true }),
            );
            return kept();
        }
        None => {}
    }
    let _ = crate::write_json(&stem.with_extension("json"), &serde_json::json!({ "key": key, "checked": crate::discovery::now(), "found": image.is_some() }));
    image
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn load_png(path: &Path) -> Option<Image> {
    let i = decode_bounded(&std::fs::read(path).ok()?)?.to_rgba8();
    Some(Image { size: [i.width(), i.height()], rgba: i.into_raw() })
}

/// A website as a package manifest gives it (often without its scheme: "orbiters.cc"), as an HTTP(S) URL.
pub fn site_url(site: &str) -> Option<Url> {
    let site = site.trim();
    let url = match Url::parse(site) {
        Err(url::ParseError::RelativeUrlWithoutBase) => Url::parse(&format!("https://{site}")).ok()?,
        parsed => parsed.ok()?,
    };
    let plain = url.username().is_empty() && url.password().is_none();
    (plain && matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| h.contains('.'))).then_some(url)
}

/// The GitHub account a page belongs to: github.com/<owner>/… or <owner>.github.io.
pub fn github_owner(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let owner = if host == "github.com" || host == "www.github.com" {
        let mut segments = url.path_segments()?.filter(|s| !s.is_empty());
        match segments.next()? {
            "orgs" | "users" => segments.next()?,
            s => s,
        }
        .to_string()
    } else {
        host.strip_suffix(".github.io")?.to_string()
    };
    let reserved = ["features", "sponsors", "about", "pricing", "topics", "marketplace", "settings", "login", "explore", "apps", "site"];
    (owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !reserved.contains(&owner.as_str())).then_some(owner)
}

async fn site_icon(client: &Web<'_>, site: Url) -> Option<Image> {
    let avatar = |owner: String| Url::parse(&format!("https://github.com/{owner}.png?size={SIDE}")).ok();
    let owner = github_owner(&site);
    if site.host_str().is_some_and(|h| h.trim_start_matches("www.").eq_ignore_ascii_case("github.com")) {
        return image_at(client, &avatar(owner?)?).await;
    }
    if let Some((page, base)) = get(client, &site, PAGE_LIMIT).await.filter(|(b, _)| !looks_like_image(b)) {
        let html = String::from_utf8_lossy(&page);
        let (mut candidates, manifest) = page_icons(&html, &base);
        if let Some(m) = manifest {
            if let Some((json, at)) = get(client, &m, MANIFEST_LIMIT).await {
                candidates.extend(manifest_icons(&json, &at));
            }
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c.1));
        candidates.dedup_by(|a, b| a.0 == b.0);
        for (url, _) in candidates.into_iter().take(CANDIDATES) {
            if let Some(image) = image_at(client, &url).await {
                return Some(image);
            }
        }
    }
    if let Some(image) = image_at(client, &site.join("/favicon.ico").ok()?).await {
        return Some(image);
    }
    // A GitHub Pages site without an icon of its own: its owner's picture.
    image_at(client, &avatar(owner?)?).await
}

async fn image_at(client: &Web<'_>, url: &Url) -> Option<Image> {
    let (bytes, _) = get(client, url, IMAGE_LIMIT).await?;
    decode(&bytes)
}

/// The body of a successful GET (at most `limit` bytes, else none) and the address it came from after redirects.
async fn get(web: &Web<'_>, url: &Url, limit: usize) -> Option<(Vec<u8>, Url)> {
    if !matches!(url.scheme(), "http" | "https") || (web.public && !public_target(url)) {
        return None;
    }
    let mut response = web.client.get(url.clone()).send().await.ok()?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n as usize > limit) {
        return None;
    }
    let at = response.url().clone();
    let mut body = vec![];
    while let Some(chunk) = response.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return None;
        }
    }
    Some((body, at))
}

fn looks_like_image(bytes: &[u8]) -> bool {
    image::guess_format(bytes).is_ok()
}

/// The icons a page's `<link>` tags declare, each with the size it says it has, and its web manifest.
pub fn page_icons(html: &str, base: &Url) -> (Vec<(Url, u32)>, Option<Url>) {
    let lower = html.to_ascii_lowercase();
    let mut icons = vec![];
    let mut manifest = None;
    let mut from = 0;
    while let Some(at) = lower[from..].find("<link").map(|i| i + from) {
        let end = lower[at..].find('>').map_or(lower.len(), |i| i + at);
        from = end;
        let attrs = attributes(&html[at + 5..end]);
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let (Some(rel), Some(href)) = (get("rel"), get("href")) else { continue };
        let Ok(url) = base.join(href.trim()) else { continue };
        let rel = rel.to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
        if rels.contains(&"manifest") {
            manifest.get_or_insert(url);
            continue;
        }
        // Safari's pinned-tab mask is a one-colour silhouette, not the icon.
        if rels.contains(&"mask-icon") || !rels.iter().any(|r| matches!(*r, "icon" | "apple-touch-icon" | "apple-touch-icon-precomposed")) {
            continue;
        }
        let svg = get("type").is_some_and(|t| t.contains("svg")) || url.path().to_ascii_lowercase().ends_with(".svg");
        let size = get("sizes").and_then(declared_size).unwrap_or(if rels.iter().any(|r| r.starts_with("apple-touch-icon")) {
            180
        } else if svg {
            256
        } else {
            32
        });
        icons.push((url, size));
    }
    (icons, manifest)
}

/// The icons of a web manifest (JSON), without the one-colour ones.
pub fn manifest_icons(json: &[u8], base: &Url) -> Vec<(Url, u32)> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else { return vec![] };
    v["icons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["purpose"].as_str().is_none_or(|p| p.split_ascii_whitespace().any(|p| p != "monochrome")))
        .filter_map(|i| {
            let url = base.join(i["src"].as_str()?).ok()?;
            Some((url, i["sizes"].as_str().and_then(declared_size).unwrap_or(64)))
        })
        .collect()
}

/// The largest of "16x16 32x32" ("any": a vector icon, as large as wanted).
fn declared_size(sizes: &str) -> Option<u32> {
    sizes
        .split_ascii_whitespace()
        .filter_map(|s| if s.eq_ignore_ascii_case("any") { Some(512) } else { s.to_ascii_lowercase().split_once('x')?.0.parse().ok() })
        .max()
}

/// A tag's attributes (lower-cased names, values unquoted).
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut chars = tag.trim_end_matches('/').chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace() || *c == '/').is_some() {}
        let name: String = std::iter::from_fn(|| chars.next_if(|c| !c.is_whitespace() && *c != '=')).collect();
        if name.is_empty() {
            return out;
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if chars.next_if_eq(&'=').is_some() {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            match chars.next_if(|c| *c == '"' || *c == '\'') {
                Some(q) => value = std::iter::from_fn(|| chars.next_if(|c| *c != q)).collect::<String>(),
                None => value = std::iter::from_fn(|| chars.next_if(|c| !c.is_whitespace())).collect(),
            }
            chars.next();
        }
        out.push((name.to_ascii_lowercase(), value.replace("&amp;", "&")));
    }
}

/// The longest side of a picture an icon is made from: a 4096 px logo still is one, a small file declaring a huge
/// picture is not decoded at all (PROJECTS-14).
const DECODE_SIDE: u32 = 4096;
/// What decoding one may allocate (the default is 512 MiB).
const DECODE_ALLOC: u64 = 128 << 20;

/// `bytes` decoded when the picture they declare is within [`DECODE_SIDE`] and [`DECODE_ALLOC`]; checked before the
/// pixels are allocated.
fn decode_bounded(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(DECODE_SIDE);
    limits.max_image_height = Some(DECODE_SIDE);
    limits.max_alloc = Some(DECODE_ALLOC);
    reader.limits(limits);
    reader.decode().ok()
}

/// An icon file (PNG, JPEG, WebP, GIF, ICO or SVG) at most [`SIDE`] pixels on its longest side; none for what is not
/// one, a speck or nothing visible.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    let image = if head.trim_start().starts_with('<') && head.contains("<svg") {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size();
        let scale = SIDE as f32 / size.width().max(size.height());
        let (w, h) = ((size.width() * scale).round().max(1.0) as u32, (size.height() * scale).round().max(1.0) as u32);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        image::RgbaImage::from_raw(w, h, rgba)?
    } else {
        let i = decode_bounded(bytes)?.to_rgba8();
        let (w, h) = i.dimensions();
        let scale = SIDE as f32 / w.max(h) as f32;
        if scale < 1.0 {
            image::imageops::resize(
                &i,
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            i
        }
    };
    let (w, h) = image.dimensions();
    if w.min(h) < 12 || image.pixels().all(|p| p[3] < 8) {
        return None;
    }
    Some(Image { size: [w, h], rgba: image.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_look_at_their_sites() {
        let s = repository_sources("https://packages.vrchat.com/official?download", &["https://vrchat.com/".into(), "https://github.com/vrchat".into()]);
        assert_eq!(s.sites, ["https://packages.vrchat.com/", "https://vrchat.com/", "https://github.com/vrchat"]);
        assert_eq!(repository_sources("https://poiyomi.github.io/vpm/index.json", &[]).sites, ["https://poiyomi.github.io/"]);
        assert_eq!(repository_sources("http://192.168.1.4:8080/index.json", &[]).sites.len(), 1);
    }

    #[test]
    fn page_links_give_their_declared_sizes_and_the_manifest() {
        let base = Url::parse("https://example.com/tools/").unwrap();
        let html = r##"<html><head>
            <link rel="stylesheet" href="/a.css">
            <LINK REL="icon" HREF="/favicon-32.png" sizes="16x16 32x32">
            <link rel='apple-touch-icon' href='touch.png'>
            <link rel="icon" type="image/svg+xml" href="/logo.svg">
            <link rel="mask-icon" href="/pinned.svg" color="#000">
            <link rel=manifest href=/site.webmanifest>
            <link rel="shortcut icon" href="https://cdn.example.com/x.ico?v=1&amp;b=2" />
        </head></html>"##;
        let (icons, manifest) = page_icons(html, &base);
        let found: Vec<(&str, u32)> = icons.iter().map(|(u, s)| (u.as_str(), *s)).collect();
        assert_eq!(
            found,
            [
                ("https://example.com/favicon-32.png", 32),
                ("https://example.com/tools/touch.png", 180),
                ("https://example.com/logo.svg", 256),
                ("https://cdn.example.com/x.ico?v=1&b=2", 32),
            ]
        );
        assert_eq!(manifest.unwrap().as_str(), "https://example.com/site.webmanifest");
    }

    #[test]
    fn manifest_icons_skip_one_colour_ones() {
        let base = Url::parse("https://example.com/app/site.webmanifest").unwrap();
        let json = br#"{"icons":[{"src":"icon-512.png","sizes":"512x512"},{"src":"mono.png","sizes":"1024x1024","purpose":"monochrome"},{"src":"/m.png","sizes":"192x192","purpose":"maskable any"}]}"#;
        let found: Vec<(String, u32)> = manifest_icons(json, &base).into_iter().map(|(u, s)| (u.to_string(), s)).collect();
        assert_eq!(found, [("https://example.com/app/icon-512.png".to_string(), 512), ("https://example.com/m.png".to_string(), 192)]);
    }

    #[test]
    fn websites_as_manifests_write_them() {
        assert_eq!(site_url("orbiters.cc").unwrap().as_str(), "https://orbiters.cc/");
        assert_eq!(site_url(" https://vrlabs.dev ").unwrap().as_str(), "https://vrlabs.dev/");
        assert!(site_url("mailto:someone@example.com").is_none());
        assert!(site_url("not a site").is_none());
        let owner = |s: &str| github_owner(&site_url(s).unwrap());
        assert_eq!(owner("https://github.com/bdunderscore").as_deref(), Some("bdunderscore"));
        assert_eq!(owner("https://github.com/orgs/vrchat-community/repositories").as_deref(), Some("vrchat-community"));
        assert_eq!(owner("https://pimaker.github.io/").as_deref(), Some("pimaker"));
        assert_eq!(owner("https://github.com/"), None);
        assert_eq!(owner("https://orbiters.cc"), None);
    }

    /// PROJECTS-13: a listing's website on this computer or its network is never asked for anything.
    #[test]
    fn websites_inside_the_network_are_never_requested() {
        for blocked in [
            "http://127.0.0.1:47615/x",
            "http://192.168.1.1/",
            "http://10.0.0.8/",
            "http://172.16.4.4/",
            "http://169.254.169.254/latest",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:192.168.0.1]/",
            "http://localhost/",
            "http://router.local/",
            "http://nas.lan/",
            "http://printer.home.arpa/",
            "ftp://example.com/",
        ] {
            assert!(!public_target(&Url::parse(blocked).unwrap()), "{blocked}");
        }
        for open in ["https://orbiters.cc/", "https://8.8.8.8/", "https://[2606:4700::1111]/", "https://vrlabs.dev/x"] {
            assert!(public_target(&Url::parse(open).unwrap()), "{open}");
        }
        assert!(site_url("localhost").is_none());
        let rt = crate::runtime().unwrap();
        let web = Web { client: &public_client().unwrap(), public: true };
        assert!(rt.block_on(get(&web, &Url::parse("http://127.0.0.1:9/").unwrap(), 10)).is_none());
    }

    #[test]
    fn icons_decode_to_at_most_their_side() {
        // A 300x150 PNG: scaled to 128x64.
        let mut png = vec![];
        image::RgbaImage::from_pixel(300, 150, image::Rgba([10, 20, 30, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let i = decode(&png).unwrap();
        assert_eq!(i.size, [128, 64]);
        assert!(i.fills());
        assert!(i.lightness() < 0.1);
        // An SVG mark on nothing: framed, light.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8" fill="#fff"/></svg>"##;
        let i = decode(svg).unwrap();
        assert_eq!(i.size, [128, 128]);
        assert!(!i.fills());
        assert!(i.lightness() > 0.9);
        // An ICO holding a 48x48 picture.
        let mut ico = vec![];
        image::RgbaImage::from_pixel(48, 48, image::Rgba([200, 0, 0, 255])).write_to(&mut std::io::Cursor::new(&mut ico), image::ImageFormat::Ico).unwrap();
        assert_eq!(decode(&ico).unwrap().size, [48, 48]);
        // A small file declaring a picture far larger than an icon: not decoded.
        let mut big = vec![];
        let side = DECODE_SIDE + 1;
        image::RgbaImage::from_pixel(side, side, image::Rgba([1, 2, 3, 255])).write_to(&mut std::io::Cursor::new(&mut big), image::ImageFormat::Png).unwrap();
        assert!(big.len() < IMAGE_LIMIT, "{} bytes", big.len());
        assert!(decode(&big).is_none());
        // Not an image, a speck, nothing visible.
        assert!(decode(b"<html>404</html>").is_none());
        let mut speck = vec![];
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255])).write_to(&mut std::io::Cursor::new(&mut speck), image::ImageFormat::Png).unwrap();
        assert!(decode(&speck).is_none());
    }

    #[test]
    fn answers_are_kept_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let rt = crate::runtime().unwrap();
        let fetched = std::sync::atomic::AtomicUsize::new(0);
        let fetch = |found: bool| {
            let fetched = &fetched;
            move |_: &reqwest::Client| async move {
                fetched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                found.then(|| Image { size: [16, 16], rgba: vec![255; 16 * 16 * 4] })
            }
        };
        let client = client().unwrap();
        // Found, then kept: the second look needs no fetch.
        assert!(rt.block_on(cached_or(dir.path(), "a", Some(&client), fetch(true))).is_some());
        assert!(rt.block_on(cached_or(dir.path(), "a", Some(&client), fetch(false))).is_some());
        assert_eq!(fetched.load(std::sync::atomic::Ordering::Relaxed), 1);
        // Nothing found is remembered too.
        assert!(rt.block_on(cached_or(dir.path(), "b", Some(&client), fetch(false))).is_none());
        assert!(rt.block_on(cached_or(dir.path(), "b", Some(&client), fetch(true))).is_none());
        assert_eq!(fetched.load(std::sync::atomic::Ordering::Relaxed), 2);
        // Out of date and the site answers nothing this time: the icon found before stays.
        let stem = dir.path().join(format!("{:016x}", fnv("a")));
        crate::write_json(&stem.with_extension("json"), &serde_json::json!({ "checked": 0, "found": true })).unwrap();
        assert!(rt.block_on(cached_or(dir.path(), "a", Some(&client), fetch(false))).is_some());
        assert_eq!(fetched.load(std::sync::atomic::Ordering::Relaxed), 3);
        // Offline (no client): whatever is kept.
        assert!(rt.block_on(cached_or(dir.path(), "a", None, fetch(false))).is_some());
    }
}
