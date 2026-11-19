//! Explorer points and titles (REQUEST13, `night-1119/explorer-contract.md`): whether a world already has a public 3D
//! picture on Orbiters, and the signed-in member's points and title.
//!
//! The rule is the server's (Enzo, 2026-11-19): the first public 3D picture of a world that had none earns 1 point, the
//! second and third 0.5 each, any later one nothing; an administrator removing a picture costs its uploader 1 point. So
//! points come in halves, a total can go down and below zero, and a title can be lost. Thresholds are never written here:
//! they come with every answer (`titles`).
//!
//! The world question is asked by world id only, without the Authorization header: the answer is the same for everyone
//! and says nothing of who asks or where they are (no instance, no account).

use crate::account::Api;
use serde::{Deserialize, Serialize};

/// One title: its stable key (what the app translates by), the points it takes, the server's name in the account's
/// language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Title {
    pub key: String,
    pub points: f64,
    pub name: String,
}

/// The next title, and how many points are still missing for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Next {
    pub key: String,
    pub points: f64,
    pub name: String,
    pub remaining: f64,
}

/// `GET /captures/explorer`: where the member stands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Explorer {
    pub points: f64,
    /// The points times two (the server's exact integer): preferred over `points` when present ([`Explorer::exact`]).
    pub half_points: Option<i64>,
    /// The highest title reached, none below the first one (or below zero).
    pub title: Option<Title>,
    /// The next title, none past the last one.
    pub next: Option<Next>,
    pub titles: Vec<Title>,
}

impl Explorer {
    /// How far from the title before the next one (or from zero) towards the next one, 0 to 1: 1 past the last title,
    /// 0 at or below zero.
    pub fn progress(&self) -> f32 {
        let Some(next) = self.next.as_ref() else { return 1.0 };
        let from = self.title.as_ref().map_or(0.0, |t| t.points);
        let span = next.points - from;
        if span <= 0.0 {
            return 0.0;
        }
        (((self.points - from) / span) as f32).clamp(0.0, 1.0)
    }

    /// The next title as the answer's list says it when `next` was left out (an older or partial answer): the first
    /// title above the points.
    pub fn next_title(&self) -> Option<Next> {
        if self.next.is_some() {
            return self.next.clone();
        }
        let mut titles = self.titles.clone();
        titles.sort_by(|a, b| a.points.total_cmp(&b.points));
        titles.into_iter().find(|t| t.points > self.points).map(|t| Next { remaining: t.points - self.points, key: t.key, points: t.points, name: t.name })
    }
}

/// How many public 3D pictures a world has, as far as earning goes: none, one, two, three or more.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldPictures {
    None,
    One,
    Two,
    ThreeOrMore,
}

impl WorldPictures {
    pub fn of(count: u64) -> Self {
        match count {
            0 => Self::None,
            1 => Self::One,
            2 => Self::Two,
            _ => Self::ThreeOrMore,
        }
    }

    /// What one more public picture of this world earns by the owner's rule (for the button's hint only: what a share
    /// earned is what the server says after it).
    pub fn would_earn(self) -> f64 {
        match self {
            Self::None => 1.0,
            Self::One | Self::Two => 0.5,
            Self::ThreeOrMore => 0.0,
        }
    }
}

/// `GET /maps/worlds/:worldId/public-capture`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct WorldAnswer {
    has_public_capture: Option<bool>,
    /// How many public 3D pictures it has (the server says 0, 1, 2, or 3 for three and more). Read leniently under the
    /// names the contract may use.
    #[serde(rename = "publicCaptures")]
    public_count: Option<u64>,
    /// What a new public 3D picture of this world would earn now: 1, 0.5 or 0 (from the ranks already used).
    next_earns: Option<f64>,
}

/// What a world lookup says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldLookup {
    pub has_public: bool,
    /// None when the server does not say how many.
    pub pictures: Option<WorldPictures>,
    /// What a new public 3D picture here would earn, when the server says (it knows ranks used by pictures unshared
    /// since, which the count does not show).
    pub next_earns: Option<f64>,
}

/// Whether `id` is a VRChat world id (`wrld_` and a UUID), the only thing ever sent about a world.
pub fn is_world_id(id: &str) -> bool {
    let Some(uuid) = id.strip_prefix("wrld_") else { return false };
    uuid.len() == 36
        && uuid.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

impl Api {
    /// Whether world `world_id` has a public 3D picture on Orbiters (and how many, when the server says): asked without
    /// signing in. `Ok(None)` when the answer is not known (a refusal, a rate limit, a server error): nothing is said
    /// then.
    pub fn world_public_capture(&self, world_id: &str) -> anyhow::Result<Option<WorldLookup>> {
        anyhow::ensure!(is_world_id(world_id), "not a VRChat world id"); // i18n: not shown (a programming error's text)
        let response = self.agent().get(&self.url(&format!("maps/worlds/{world_id}/public-capture"))).call()?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let answer: WorldAnswer = Api::json(response)?;
        let pictures = answer.public_count.map(WorldPictures::of);
        let has_public = match (answer.has_public_capture, answer.public_count) {
            (Some(has), _) => has,
            (None, Some(n)) => n > 0,
            (None, None) => return Ok(None),
        };
        // A count that contradicts the yes or no is not believed.
        let pictures = pictures.filter(|p| (*p != WorldPictures::None) == has_public);
        let next_earns = answer.next_earns.filter(|e| [0.0, 0.5, 1.0].contains(e)).or_else(|| pictures.map(WorldPictures::would_earn));
        Ok(Some(WorldLookup { has_public, pictures, next_earns }))
    }

    /// The member's Explorer points and title.
    pub fn explorer(&self, token: &str) -> anyhow::Result<Explorer> {
        let response = self.agent().get(&self.url("captures/explorer")).header("Authorization", &format!("Bearer {token}")).call()?;
        let mut explorer: Explorer = crate::shares::json(response)?;
        if let Some(halves) = explorer.half_points {
            explorer.points = halves as f64 / 2.0;
        }
        anyhow::ensure!(explorer.points.is_finite(), "{}", orbiters_i18n::tr("unexpected answer from Orbiters"));
        Ok(explorer)
    }
}

/// What a public share, upload or unshare answered about points (`explorer` in its answer, contract section 3): the
/// member's standing right after it, the points just before, what THIS request earned, a title it reached.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Outcome {
    pub points: f64,
    pub half_points: Option<i64>,
    pub title: Option<Title>,
    pub next: Option<Next>,
    pub titles: Vec<Title>,
    pub before: Option<f64>,
    pub earned: Option<f64>,
    pub title_reached: Option<Title>,
}

impl Outcome {
    /// Where the member stands after it.
    pub fn standing(&self) -> Explorer {
        let points = self.half_points.map_or(self.points, |h| h as f64 / 2.0);
        Explorer { points, half_points: self.half_points, title: self.title.clone(), next: self.next.clone(), titles: self.titles.clone() }
    }

    /// Two answers of one share in their order (the upload's, then the share's): what both earned, the second's standing,
    /// the first's points before, a title either reached (the later one first).
    pub fn then(self, later: Outcome) -> Outcome {
        let earned = match (self.earned, later.earned) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
        Outcome { before: self.before.or(later.before), earned, title_reached: later.title_reached.clone().or(self.title_reached), ..later }
    }
}

/// What one public share earned, read from where the member stood before and after it: `None` when the points before
/// could not be read.
pub fn earned(before: Option<&Explorer>, after: &Explorer) -> Option<f64> {
    before.map(|b| ((after.points - b.points) * 2.0).round() / 2.0)
}

/// A title reached by the change from `before` to `after`, none when it stayed or went down.
pub fn title_reached(before: Option<&Explorer>, after: &Explorer) -> Option<Title> {
    let before = before?;
    let (was, now) = (before.title.as_ref().map(|t| t.points), after.title.as_ref().map(|t| t.points));
    match (was, now) {
        (_, None) => None,
        (Some(was), Some(now)) if now <= was => None,
        _ => after.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A server on this computer answering each request with the next of `answers`, and what was asked.
    fn serve(answers: Vec<(u16, &'static str)>) -> (Api, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                asked.push(String::from_utf8_lossy(&request).into_owned());
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(reply.as_bytes()).unwrap();
            }
            asked
        });
        (api, server)
    }

    const WORLD: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";

    #[test]
    fn a_world_is_asked_by_its_id_alone_and_without_signing_in() {
        let (api, server) = serve(vec![
            (200, r#"{"worldId":"wrld_x","hasPublicCapture":false}"#),
            (200, r#"{"worldId":"wrld_x","hasPublicCapture":true,"publicCaptures":2,"nextEarns":0.5}"#),
            (429, r#"{"error":"Too many"}"#),
            (503, "Bad gateway"),
            (200, r#"{"hasPublicCapture":false,"publicCaptures":3}"#),
        ]);
        assert_eq!(api.world_public_capture(WORLD).unwrap(), Some(WorldLookup { has_public: false, pictures: None, next_earns: None }));
        assert_eq!(api.world_public_capture(WORLD).unwrap(), Some(WorldLookup { has_public: true, pictures: Some(WorldPictures::Two), next_earns: Some(0.5) }));
        assert_eq!(api.world_public_capture(WORLD).unwrap(), None, "a rate limit says nothing");
        assert_eq!(api.world_public_capture(WORLD).unwrap(), None, "a server error says nothing");
        assert_eq!(api.world_public_capture(WORLD).unwrap(), Some(WorldLookup { has_public: false, pictures: None, next_earns: None }), "a contradicting count is not believed");
        let asked = server.join().unwrap();
        for request in &asked {
            assert!(request.starts_with(&format!("GET /maps/worlds/{WORLD}/public-capture ")), "{request}");
            assert!(!request.to_ascii_lowercase().contains("authorization"), "no sign-in: {request}");
            assert!(!request.contains('~') && !request.contains("instance"), "nothing of the instance: {request}");
        }
        assert!(api.world_public_capture("wrld_x:1234~private(usr_a)").is_err(), "an instance is never sent");
    }

    #[test]
    fn points_read_as_halves_and_below_zero() {
        let body = r#"{"points":12.5,"title":{"key":"explorer","points":10,"name":"Explorer"},"next":{"key":"navigator","points":50,"name":"Navigator","remaining":37.5},"titles":[]}"#;
        let (api, server) = serve(vec![(200, body), (200, r#"{"points":-1,"title":null,"next":{"key":"explorer","points":10,"name":"Explorer","remaining":11},"titles":[]}"#), (401, r#"{"error":"Sign in"}"#)]);
        let e = api.explorer("t").unwrap();
        assert_eq!((e.points, e.title.as_ref().unwrap().key.as_str(), e.next.as_ref().unwrap().remaining), (12.5, "explorer", 37.5));
        assert!((e.progress() - 2.5 / 40.0).abs() < 1e-6);
        let below = api.explorer("t").unwrap();
        assert_eq!((below.points, below.title.is_none(), below.progress()), (-1.0, true, 0.0));
        assert!(api.explorer("t").is_err());
        assert!(server.join().unwrap()[0].to_ascii_lowercase().contains("authorization: bearer t"));
    }

    fn at(points: f64, title: Option<f64>) -> Explorer {
        Explorer { points, title: title.map(|p| Title { key: format!("t{p}"), points: p, name: String::new() }), ..Default::default() }
    }

    #[test]
    fn what_a_share_earned_is_the_difference_and_a_title_only_counts_going_up() {
        assert_eq!(earned(Some(&at(9.5, None)), &at(10.5, Some(10.0))), Some(1.0));
        assert_eq!(earned(Some(&at(3.0, None)), &at(3.5, None)), Some(0.5));
        assert_eq!(earned(Some(&at(3.0, None)), &at(3.0, None)), Some(0.0));
        assert_eq!(earned(None, &at(3.0, None)), None);
        assert_eq!(title_reached(Some(&at(9.5, None)), &at(10.5, Some(10.0))).map(|t| t.points), Some(10.0));
        assert_eq!(title_reached(Some(&at(10.5, Some(10.0))), &at(11.0, Some(10.0))), None);
        assert_eq!(title_reached(Some(&at(10.0, Some(10.0))), &at(9.0, None)), None, "a title lost is no title reached");
    }

    #[test]
    fn what_one_more_picture_would_earn_by_the_world_s_count() {
        assert_eq!([0, 1, 2, 3, 40].map(|n| WorldPictures::of(n).would_earn()), [1.0, 0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn a_missing_next_comes_from_the_list() {
        let titles = [(10.0, "explorer"), (50.0, "navigator"), (1000.0, "multiverseTraveler")]
            .map(|(p, k)| Title { key: k.into(), points: p, name: k.into() })
            .to_vec();
        let e = Explorer { points: 12.5, titles: titles.clone(), ..Default::default() };
        assert_eq!(e.next_title().map(|n| (n.key, n.remaining)), Some(("navigator".into(), 37.5)));
        let top = Explorer { points: 1000.0, titles, ..Default::default() };
        assert_eq!(top.next_title(), None);
    }

    /// A picture uploaded publicly by its share: the upload earned, the share after it earned nothing more; what the share
    /// earned is both, its standing the later answer's.
    #[test]
    fn an_upload_and_its_share_say_together_what_the_share_earned() {
        let up: Outcome = serde_json::from_str(r#"{"points":10.5,"halfPoints":21,"title":{"key":"explorer","points":10,"name":"Explorer"},"next":null,"titles":[],"before":9.5,"earned":1,"titleReached":{"key":"explorer","points":10,"name":"Explorer"}}"#).unwrap();
        let shared: Outcome = serde_json::from_str(r#"{"points":10.5,"halfPoints":21,"title":{"key":"explorer","points":10,"name":"Explorer"},"next":null,"titles":[],"before":10.5,"earned":0,"titleReached":null}"#).unwrap();
        let both = up.then(shared);
        assert_eq!((both.earned, both.before, both.standing().points, both.title_reached.map(|t| t.key)), (Some(1.0), Some(9.5), 10.5, Some("explorer".into())));
        let shares: crate::shares::Shared = serde_json::from_str(r#"{"item":{"id":5,"public":true},"explorer":null}"#).unwrap();
        assert!(matches!(shares, crate::shares::Shared::Item { explorer: None, .. }), "null: the share stands, without points");
        let request: crate::shares::Shared = serde_json::from_str(r#"{"request":{"id":3}}"#).unwrap();
        assert!(matches!(request, crate::shares::Shared::Request { .. }), "a requester's share earns nothing");
    }

    /// A first public share refused for the day (429 `explorer-daily-limit`) is not the address's rate limit (a 429
    /// without that code), and a 503 is the server's own (not ready).
    #[test]
    fn the_daily_limit_and_not_ready_are_told_apart() {
        let (api, server) = serve(vec![
            (429, r#"{"error":"Limite du jour atteinte","code":"explorer-daily-limit"}"#),
            (429, r#"{"error":"Too many requests"}"#),
            (503, r#"{"error":"Not ready"}"#),
        ]);
        let share = crate::shares::Share { public: true, recipients: Vec::new(), acknowledge_explicit: false, message: String::new() };
        let call = || api.share_capture("t", crate::shares::Kind::Capture, 5, &share).unwrap_err();
        let limit = call();
        assert_eq!(limit.downcast_ref::<crate::shares::ExplorerDailyLimit>().map(|e| e.0.as_str()), Some("Limite du jour atteinte"));
        let busy = call();
        assert!(busy.downcast_ref::<crate::shares::RateLimited>().is_some() && busy.downcast_ref::<crate::shares::ExplorerDailyLimit>().is_none());
        let not_ready = call();
        assert_eq!(not_ready.downcast_ref::<crate::account::ApiError>().map(|e| e.status), Some(503));
        server.join().unwrap();
    }

    #[test]
    fn only_a_world_id_is_a_world_id() {
        assert!(is_world_id(WORLD));
        assert!(!is_world_id("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b:123~private(usr_x)"));
        assert!(!is_world_id("usr_4cf554b4-430c-4f8f-b53e-1f294eed230b"));
        assert!(!is_world_id("wrld_../../admin"));
    }
}
