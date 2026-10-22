//! YouTube for Orb's menu, without an account or an API key: suggestions while the person types (Google's suggest
//! endpoint, the one YouTube's own search box uses), search results from YouTube's web client API (InnerTube
//! `/search`, as the website itself calls it) and each result's thumbnail. Read only: the menu copies a result's link
//! so it can be pasted into a world's video player.

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

/// A search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    /// `1:04:42`, as YouTube writes it; `None` for live streams and premieres.
    pub duration: Option<String>,
    pub live: bool,
}

impl Video {
    /// What a world's video player takes.
    pub fn link(&self) -> String {
        link(&self.id)
    }
}

/// One page of results as YouTube sends it (about 20), and the token for the next.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub videos: Vec<Video>,
    pub continuation: Option<String>,
}

const API: &str = "https://www.youtube.com/youtubei/v1";
/// The website's client, as the server's own YouTube reader sends it (`agents/orb/orb_sync/youtube.py`): sent when a
/// recent one ([`client_version`]) is refused.
const CLIENT_VERSION: &str = "2.20251015.01.00";
/// Search filter: videos only (no channels, playlists or shorts shelves).
const ONLY_VIDEOS: &str = "EgIQAQ%3D%3D";
/// The most a query may hold; YouTube's box takes about this much.
pub const MAX_QUERY: usize = 200;

pub fn link(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// The 320 × 180 JPEG every video has (live streams included).
pub fn thumbnail_url(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
}

/// The suggestions endpoint for `query` in `language` (an ISO 639-1 code such as `en` or `fr`). Asked in UTF-8 both
/// ways: otherwise Google answers in the language's old charset (ISO-8859-1 for French, windows-1251 for Russian).
pub fn suggestions_url(query: &str, language: &str) -> String {
    format!(
        "https://suggestqueries.google.com/complete/search?client=firefox&ds=yt&ie=utf-8&oe=utf-8&hl={}&q={}",
        crate::videos::encode(language),
        crate::videos::encode(query)
    )
}

/// The website's client version: a build of a month ago (YouTube takes those for months; a fixed one ages out).
fn client_version() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64 - 30;
    let (year, month, day) = civil(days);
    if year < 2025 {
        return CLIENT_VERSION.to_string();
    }
    format!("2.{year:04}{month:02}{day:02}.01.00")
}

/// The date of `days` since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (month <= 2) as i64, month, day)
}

/// The suggestions in the endpoint's answer: `["query", ["suggestion", …], …]`.
pub fn parse_suggestions(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    value
        .get(1)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// The request body of a first search.
pub fn search_body(query: &str, language: &str) -> Value {
    json!({ "context": context(language), "query": cut(query.trim(), MAX_QUERY), "params": ONLY_VIDEOS })
}

/// The request body of the next page of a search.
pub fn continuation_body(token: &str, language: &str) -> Value {
    json!({ "context": context(language), "continuation": token })
}

fn context(language: &str) -> Value {
    json!({ "client": { "clientName": "WEB", "clientVersion": client_version(), "hl": if language.is_empty() { "en" } else { language } } })
}

fn cut(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// The videos of a search answer, first page or continuation alike (they sit at different depths, so the tree is
/// walked for `videoRenderer`s, and for the `lockupViewModel`s YouTube's newer pages use), and the token of the next
/// page: the one ending the results' own list (whose items are `itemSectionRenderer`s), else any list's last item's,
/// else the first token anywhere. Objects are walked in key order, which says nothing about the page's order, but
/// lists keep theirs.
pub fn parse_results(value: &Value) -> Results {
    let mut results = Results::default();
    let mut tokens = Tokens::default();
    walk(value, &mut results, &mut tokens);
    results.continuation = tokens.results.or(tokens.list_end).or(tokens.any);
    results
}

/// Continuation tokens found, from the most to the least telling.
#[derive(Default)]
struct Tokens {
    results: Option<String>,
    list_end: Option<String>,
    any: Option<String>,
}

fn walk(node: &Value, results: &mut Results, tokens: &mut Tokens) {
    match node {
        Value::Object(map) => {
            let video = map.get("videoRenderer").and_then(video_of).or_else(|| map.get("lockupViewModel").and_then(lockup_of));
            if let Some(video) = video {
                if !results.videos.iter().any(|v| v.id == video.id) {
                    results.videos.push(video);
                }
            }
            if let Some(token) = map.get("continuationCommand").and_then(|c| c.get("token")).and_then(Value::as_str) {
                tokens.any.get_or_insert_with(|| token.to_string());
            }
            for (key, value) in map {
                if key != "videoRenderer" && key != "lockupViewModel" {
                    walk(value, results, tokens);
                }
            }
        }
        Value::Array(list) => {
            if let Some(token) = list.last().and_then(|last| last.get("continuationItemRenderer")).and_then(token_in) {
                if list.iter().any(|item| item.get("itemSectionRenderer").is_some()) {
                    tokens.results.get_or_insert(token);
                } else {
                    tokens.list_end.get_or_insert(token);
                }
            }
            list.iter().for_each(|v| walk(v, results, tokens));
        }
        _ => {}
    }
}

/// The first continuation token under `node`.
fn token_in(node: &Value) -> Option<String> {
    match node {
        Value::Object(map) => {
            map.get("continuationCommand").and_then(|c| c.get("token")).and_then(Value::as_str).map(str::to_string).or_else(|| map.values().find_map(token_in))
        }
        Value::Array(list) => list.iter().find_map(token_in),
        _ => None,
    }
}

/// Every value under `node` at key `key`, in the order met.
fn under<'a>(node: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                if k == key {
                    out.push(v);
                }
                under(v, key, out);
            }
        }
        Value::Array(list) => list.iter().for_each(|v| under(v, key, out)),
        _ => {}
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A video in the "lockup" shape: `contentId`, a title and metadata rows (the channel first), the length or LIVE as
/// a badge on the thumbnail.
fn lockup_of(item: &Value) -> Option<Video> {
    let kind = item.get("contentType").and_then(Value::as_str).unwrap_or("LOCKUP_CONTENT_TYPE_VIDEO");
    if kind != "LOCKUP_CONTENT_TYPE_VIDEO" {
        return None;
    }
    let id = item.get("contentId")?.as_str()?;
    if !valid_id(id) {
        return None;
    }
    let meta = item.pointer("/metadata/lockupMetadataViewModel")?;
    let title = meta.pointer("/title/content").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return None;
    }
    let channel = meta
        .pointer("/metadata/contentMetadataViewModel/metadataRows/0/metadataParts/0/text/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut badges = Vec::new();
    if let Some(image) = item.get("contentImage") {
        under(image, "thumbnailBadgeViewModel", &mut badges);
    }
    let texts: Vec<&str> = badges.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).map(str::trim).collect();
    let duration = texts.iter().find(|t| t.contains(':') && t.chars().all(|c| c.is_ascii_digit() || c == ':')).map(|t| t.to_string());
    let live = duration.is_none() && texts.iter().any(|t| t.eq_ignore_ascii_case("live"));
    Some(Video { id: id.to_string(), title, channel, duration, live })
}

fn text_of(node: Option<&Value>) -> String {
    let Some(node) = node else { return String::new() };
    if let Some(simple) = node.get("simpleText").and_then(Value::as_str) {
        return simple.trim().to_string();
    }
    node.get("runs")
        .and_then(Value::as_array)
        .map(|runs| runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn video_of(item: &Value) -> Option<Video> {
    let id = item.get("videoId")?.as_str()?;
    if !valid_id(id) {
        return None;
    }
    let title = text_of(item.get("title"));
    if title.is_empty() {
        return None;
    }
    let channel = text_of(item.get("ownerText").or_else(|| item.get("longBylineText")).or_else(|| item.get("shortBylineText")));
    let duration = Some(text_of(item.get("lengthText"))).filter(|d| !d.is_empty());
    let badge = |name: &str| {
        item.get("badges").and_then(Value::as_array).is_some_and(|badges| {
            badges.iter().any(|b| {
                let badge = b.get("metadataBadgeRenderer");
                let label = badge.and_then(|r| r.get("label")).and_then(Value::as_str).unwrap_or_default();
                let style = badge.and_then(|r| r.get("style")).and_then(Value::as_str).unwrap_or_default();
                label.eq_ignore_ascii_case(name) || style.ends_with("_LIVE_NOW")
            })
        })
    };
    let live = duration.is_none() && badge("LIVE");
    Some(Video { id: id.to_string(), title, channel, duration, live })
}

/// One agent for every request (the menu's suggestions, searches and thumbnails): its connections stay open from one
/// keystroke or thumbnail to the next, where an agent each paid a TCP and TLS handshake every time. Each request sets
/// its own time limit.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| orbiters_core::http_agent(Some(Duration::from_secs(4)), None))
}

/// A response's text: UTF-8, else ISO-8859-1 when its charset says so (or one close to it), else as well as it can be
/// read. ureq reads UTF-8 only.
fn decode(bytes: Vec<u8>, charset: &str) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            let bytes = e.into_bytes();
            if ["iso-8859-1", "latin1", "latin-1", "windows-1252", "cp1252", "iso-8859-15"].contains(&charset) {
                bytes.iter().map(|&b| b as char).collect()
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            }
        }
    }
}

fn read(mut response: ureq::http::Response<ureq::Body>, limit: u64) -> anyhow::Result<String> {
    let status = response.status().as_u16();
    if status != 200 {
        anyhow::bail!("YouTube answered {status}");
    }
    let charset = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|t| t.split(';').find_map(|p| p.trim().strip_prefix("charset=")))
        .map(|c| c.trim_matches('"').to_ascii_lowercase())
        .unwrap_or_default();
    let bytes = response.body_mut().with_config().limit(limit).read_to_vec()?;
    Ok(decode(bytes, &charset))
}

/// Suggestions for what the person has typed so far (at most 10). Blocking, 4 s at most.
pub fn suggestions(query: &str, language: &str) -> anyhow::Result<Vec<String>> {
    let query = cut(query.trim_start(), MAX_QUERY);
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let response = agent().get(&suggestions_url(query, language)).config().timeout_global(Some(Duration::from_secs(4))).build().call()?;
    Ok(parse_suggestions(&read(response, 64 * 1024)?))
}

fn post(body: &Value) -> anyhow::Result<Results> {
    let send = |body: &Value| {
        agent()
            .post(&format!("{API}/search?prettyPrint=false"))
            .config()
            .timeout_global(Some(Duration::from_secs(8)))
            .http_status_as_error(false)
            .build()
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .send_json(body)
    };
    let mut response = send(body)?;
    if (400..500).contains(&response.status().as_u16()) && body["context"]["client"]["clientVersion"] != CLIENT_VERSION {
        // A client version YouTube does not take: the one known to work.
        let mut body = body.clone();
        body["context"]["client"]["clientVersion"] = json!(CLIENT_VERSION);
        response = send(&body)?;
    }
    let text = read(response, 8 * 1024 * 1024)?;
    Ok(parse_results(&serde_json::from_str(&text)?))
}

/// Videos for `query`. Blocking, 8 s at most.
pub fn search(query: &str, language: &str) -> anyhow::Result<Results> {
    if query.trim().is_empty() {
        anyhow::bail!("nothing to search for");
    }
    post(&search_body(query, language))
}

/// The page after `token` (from [`Results::continuation`]).
pub fn more(token: &str, language: &str) -> anyhow::Result<Results> {
    post(&continuation_body(token, language))
}

/// A video's thumbnail as JPEG bytes. Blocking, 6 s at most.
pub fn thumbnail(id: &str) -> anyhow::Result<Vec<u8>> {
    let mut response = agent().get(&thumbnail_url(id)).config().timeout_global(Some(Duration::from_secs(6))).build().call()?;
    if response.status().as_u16() != 200 {
        anyhow::bail!("no thumbnail ({})", response.status().as_u16());
    }
    Ok(response.body_mut().with_config().limit(1024 * 1024).read_to_vec()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_suggestions() {
        let text = r#"["café",["café gourmand","café"," café del mar ",""],[],{"google:suggestsubtypes":[[512]]}]"#;
        assert_eq!(parse_suggestions(text), vec!["café gourmand", "café", "café del mar"]);
        assert!(parse_suggestions("not json").is_empty());
        assert!(parse_suggestions(r#"["q"]"#).is_empty());
        assert_eq!(
            suggestions_url("lofi hip hop & café", "fr"),
            "https://suggestqueries.google.com/complete/search?client=firefox&ds=yt&ie=utf-8&oe=utf-8&hl=fr&q=lofi%20hip%20hop%20%26%20caf%C3%A9"
        );
        // An answer in the language's old charset still reads.
        assert_eq!(decode(b"[\"caf\xe9\"]".to_vec(), "iso-8859-1"), "[\"café\"]");
        assert_eq!(decode("[\"café\"]".as_bytes().to_vec(), ""), "[\"café\"]");
        assert_eq!(decode(b"caf\xe9".to_vec(), "windows-1251"), "caf\u{fffd}");
    }

    #[test]
    fn a_recent_client_version() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_382), (2025, 10, 21));
        assert_eq!(civil(19_782), (2024, 2, 29));
        let version = client_version();
        assert!(version.starts_with("2.20") && version.ends_with(".01.00") && version.len() == CLIENT_VERSION.len(), "{version}");
    }

    fn renderer(id: &str, title: &str, channel: &str, length: Option<&str>, badge: Option<&str>) -> Value {
        let mut video = json!({
            "videoId": id,
            "title": { "runs": [{ "text": title }] },
            "ownerText": { "runs": [{ "text": channel, "navigationEndpoint": {} }] },
            "thumbnail": { "thumbnails": [{ "url": format!("https://i.ytimg.com/vi/{id}/hq720.jpg"), "width": 360, "height": 202 }] }
        });
        if let Some(length) = length {
            video["lengthText"] = json!({ "simpleText": length });
        }
        if let Some(label) = badge {
            video["badges"] = json!([{ "metadataBadgeRenderer": { "label": label, "style": "BADGE_STYLE_TYPE_LIVE_NOW" } }]);
        }
        json!({ "videoRenderer": video })
    }

    #[test]
    fn reads_a_first_page_and_a_continuation() {
        let first = json!({
            "contents": { "twoColumnSearchResultsRenderer": { "primaryContents": { "sectionListRenderer": { "contents": [
                { "itemSectionRenderer": { "contents": [
                    renderer("n61ULEU7CO0", "Best of lofi hip hop 2021 ✨", "Lofi Girl", Some("6:10:58"), None),
                    { "shelfRenderer": { "content": { "verticalListRenderer": { "items": [
                        renderer("uWzSqdcfPZw", "lofi hip hop radio", "STEEZYASFUCK", None, Some("LIVE")),
                        renderer("n61ULEU7CO0", "Best of lofi hip hop 2021 ✨", "Lofi Girl", Some("6:10:58"), None)
                    ] } } } },
                    { "videoRenderer": { "videoId": "bad", "title": { "simpleText": "Not a video id" } } },
                    { "radioRenderer": { "playlistId": "RD123" } }
                ] } },
                { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "EqEDEgxsb2Zp", "request": "CONTINUATION_REQUEST_TYPE_SEARCH" } } } }
            ] } } } }
        });
        let results = parse_results(&first);
        assert_eq!(results.continuation.as_deref(), Some("EqEDEgxsb2Zp"));
        assert_eq!(results.videos.len(), 2, "a video listed twice (top result and shelf) counts once; a malformed id is left out");
        assert_eq!(
            results.videos[0],
            Video {
                id: "n61ULEU7CO0".into(),
                title: "Best of lofi hip hop 2021 ✨".into(),
                channel: "Lofi Girl".into(),
                duration: Some("6:10:58".into()),
                live: false
            }
        );
        assert!(results.videos[1].live && results.videos[1].duration.is_none());
        assert_eq!(results.videos[0].link(), "https://www.youtube.com/watch?v=n61ULEU7CO0");

        let next = json!({ "onResponseReceivedCommands": [{ "appendContinuationItemsAction": { "continuationItems": [
            { "itemSectionRenderer": { "contents": [renderer("_jH2oTNRe7g", "cozy autumn music", "Lofi Girl", Some("1:04:42"), None)] } },
            { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "page3" } } } }
        ] } }] });
        let more = parse_results(&next);
        assert_eq!(more.videos.len(), 1);
        assert_eq!(more.continuation.as_deref(), Some("page3"));
        assert_eq!(parse_results(&json!({})), Results::default());
    }

    #[test]
    fn the_results_list_s_own_token_wins() {
        // A header's token sits under a key that sorts after the results ("zHeader"), and a shelf's list ends with its own.
        let page = json!({
            "contents": { "sectionListRenderer": { "contents": [
                { "itemSectionRenderer": { "contents": [
                    renderer("n61ULEU7CO0", "Best of lofi", "Lofi Girl", Some("6:10:58"), None),
                    { "shelfRenderer": { "content": { "verticalListRenderer": { "items": [
                        renderer("uWzSqdcfPZw", "radio", "Lofi Girl", None, Some("LIVE")),
                        { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "shelf" } } } }
                    ] } } } }
                ] } },
                { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "results" } } } }
            ] } },
            "zHeader": { "chipCloudRenderer": { "chips": [ { "continuationCommand": { "token": "chip" } } ] } }
        });
        assert_eq!(parse_results(&page).continuation.as_deref(), Some("results"));
    }

    #[test]
    fn reads_lockup_videos() {
        let lockup = |id: &str, badge: &str| {
            json!({ "lockupViewModel": {
                "contentId": id, "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
                "contentImage": { "thumbnailViewModel": { "overlays": [ { "thumbnailOverlayBadgeViewModel": { "thumbnailBadges": [
                    { "thumbnailBadgeViewModel": { "text": badge } } ] } } ] } },
                "metadata": { "lockupMetadataViewModel": {
                    "title": { "content": "cozy autumn music" },
                    "metadata": { "contentMetadataViewModel": { "metadataRows": [ { "metadataParts": [ { "text": { "content": "Lofi Girl" } } ] } ] } }
                } }
            } })
        };
        let page = json!({ "items": [lockup("_jH2oTNRe7g", "1:04:42"), lockup("uWzSqdcfPZw", "LIVE"), lockup("short", "1:00"),
            { "lockupViewModel": { "contentId": "PLabcdefghi", "contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST" } }] });
        let results = parse_results(&page);
        assert_eq!(results.videos.len(), 2, "a bad id and a playlist are left out");
        assert_eq!(
            results.videos[0],
            Video { id: "_jH2oTNRe7g".into(), title: "cozy autumn music".into(), channel: "Lofi Girl".into(), duration: Some("1:04:42".into()), live: false }
        );
        assert!(results.videos[1].live && results.videos[1].duration.is_none());
    }

    #[test]
    fn builds_requests() {
        let body = search_body("  lofi  ", "fr");
        assert_eq!(body["query"], "lofi");
        assert_eq!(body["context"]["client"]["hl"], "fr");
        assert_eq!(body["params"], ONLY_VIDEOS);
        assert_eq!(continuation_body("tok", "")["context"]["client"]["hl"], "en");
        assert_eq!(cut("été", 2), "ét");
        assert_eq!(thumbnail_url("n61ULEU7CO0"), "https://i.ytimg.com/vi/n61ULEU7CO0/mqdefault.jpg");
    }

    /// Against YouTube itself: `cargo test -p orbiters-orb youtube_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn youtube_live() {
        let suggestions = suggestions("lofi hip", "en").unwrap();
        println!("suggestions: {suggestions:?}");
        assert!(!suggestions.is_empty());
        let results = search("lofi hip hop", "en").unwrap();
        for v in &results.videos {
            println!("{} | {} | {} | {:?} | live {}", v.id, v.title, v.channel, v.duration, v.live);
        }
        assert!(results.videos.len() >= 10 && results.continuation.is_some());
        let page = more(results.continuation.as_deref().unwrap(), "en").unwrap();
        assert!(!page.videos.is_empty());
        let jpeg = thumbnail(&results.videos[0].id).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }

    /// Accented suggestions against Google itself: `cargo test -p orbiters-orb accented_suggestions_live -- --ignored`.
    #[test]
    #[ignore]
    fn accented_suggestions_live() {
        let found = suggestions("caf", "fr").unwrap();
        println!("suggestions: {found:?}");
        assert!(found.iter().any(|s| s.contains('é')), "{found:?}");
        assert!(!suggestions("моло", "ru").unwrap().is_empty());
    }
}
