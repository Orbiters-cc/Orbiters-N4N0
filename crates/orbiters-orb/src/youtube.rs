//! YouTube for Orb's menu, without an account or an API key: suggestions while the person types (Google's suggest
//! endpoint, the one YouTube's own search box uses), search results from YouTube's web client API (InnerTube
//! `/search`, as the website itself calls it) and each result's thumbnail. Read only: the menu copies a result's link
//! so it can be pasted into a world's video player.

use serde_json::{json, Value};
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
/// The website's client, as the server's own YouTube reader sends it (`agents/orb/orb_sync/youtube.py`).
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

/// The suggestions endpoint for `query` in `language` (an ISO 639-1 code such as `en` or `fr`).
pub fn suggestions_url(query: &str, language: &str) -> String {
    format!(
        "https://suggestqueries.google.com/complete/search?client=firefox&ds=yt&hl={}&q={}",
        crate::videos::encode(language),
        crate::videos::encode(query)
    )
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
    json!({ "client": { "clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": if language.is_empty() { "en" } else { language } } })
}

fn cut(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// The videos of a search answer, first page or continuation alike (they sit at different depths, so the tree is
/// walked for `videoRenderer`s), and the last continuation token in it.
pub fn parse_results(value: &Value) -> Results {
    let mut results = Results::default();
    walk(value, &mut results);
    results
}

fn walk(node: &Value, results: &mut Results) {
    match node {
        Value::Object(map) => {
            if let Some(video) = map.get("videoRenderer").and_then(video_of) {
                if !results.videos.iter().any(|v| v.id == video.id) {
                    results.videos.push(video);
                }
            }
            if let Some(token) = map.get("continuationCommand").and_then(|c| c.get("token")).and_then(Value::as_str) {
                results.continuation = Some(token.to_string());
            }
            for (key, value) in map {
                if key != "videoRenderer" {
                    walk(value, results);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|v| walk(v, results)),
        _ => {}
    }
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
    if id.len() != 11 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
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

fn agent(timeout: Duration) -> ureq::Agent {
    orbiters_core::http_agent(None, Some(timeout))
}

fn read(mut response: ureq::http::Response<ureq::Body>, limit: u64) -> anyhow::Result<String> {
    let status = response.status().as_u16();
    if status != 200 {
        anyhow::bail!("YouTube answered {status}");
    }
    Ok(response.body_mut().with_config().limit(limit).read_to_string()?)
}

/// Suggestions for what the person has typed so far (at most 10). Blocking, 4 s at most.
pub fn suggestions(query: &str, language: &str) -> anyhow::Result<Vec<String>> {
    let query = cut(query.trim_start(), MAX_QUERY);
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let response = agent(Duration::from_secs(4)).get(&suggestions_url(query, language)).call()?;
    Ok(parse_suggestions(&read(response, 64 * 1024)?))
}

fn post(body: &Value) -> anyhow::Result<Results> {
    let response = agent(Duration::from_secs(8))
        .post(&format!("{API}/search?prettyPrint=false"))
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .send_json(body)?;
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
    let mut response = agent(Duration::from_secs(6)).get(&thumbnail_url(id)).call()?;
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
            "https://suggestqueries.google.com/complete/search?client=firefox&ds=yt&hl=fr&q=lofi%20hip%20hop%20%26%20caf%C3%A9"
        );
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
            Video { id: "n61ULEU7CO0".into(), title: "Best of lofi hip hop 2021 ✨".into(), channel: "Lofi Girl".into(), duration: Some("6:10:58".into()), live: false }
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
}
