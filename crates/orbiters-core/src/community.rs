//! Orbiters' events and the member's community galleries, as Orbiters - N4N0's Events page and Photos & captures read
//! them: the events the website's homepage lists, and with them the events under way (`/community-events/upcoming
//! ?live=true`), an event's own page (`/community-events/public/:id`: its steps, worlds, instance and invites), the
//! member's invites and the website's Invite me (`/community-events/:id/steps/:step/invite`), and the galleries of the
//! communities the member is in with their pictures (`/galleries`, `/galleries/images`). The member's app credential
//! signs these in (the server's tool routes); signed out, only the public reads answer.

use crate::account::Api;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A community hosting an event (its VRChat group or Discord server).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Host {
    /// The VRChat group's id (`grp_…`) on an event's page; absent on cards.
    pub id: Option<String>,
    pub name: String,
    pub icon_url: Option<String>,
    /// The VRChat account that sends the group's invites (`usr_…`).
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct World {
    pub id: Option<String>,
    pub name: Option<String>,
    pub image_url: Option<String>,
}

/// An event as lists show it: the homepage's upcoming events, the card above an event's gallery, the event a gallery
/// picture was taken at (only id, title, times and status there).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub description: String,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub category: Option<String>,
    /// `published`, `ended`, `cancelled`… (cards of galleries; listed events are all published).
    pub status: Option<String>,
    pub adult_only: bool,
    pub has_banner: bool,
    pub community: Option<Host>,
    pub discord: Option<Host>,
    pub world: Option<World>,
    pub steps: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cards {
    pub items: Vec<Card>,
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Movie {
    pub title: String,
    pub year: Option<serde_json::Value>,
}

/// One step of an event's evening: when, where, and whether its instance is open and its invites taken.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    /// The organiser starts it (no time of its own).
    pub organizer_starts: bool,
    pub world: Option<World>,
    pub movie: Option<Movie>,
    /// Its instance is open now.
    pub live: bool,
    /// The instance's launch link (`https://vrchat.com/home/launch?worldId=…&instanceId=…`) while it is open.
    pub instance_url: Option<String>,
    /// Invite me takes sign-ups for this step.
    pub invite_open: bool,
    pub invite_mode: String,
}

/// An event's own page (what the website's event page shows).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Overview {
    pub id: String,
    pub title: String,
    pub description: String,
    pub long_description: String,
    pub status: String,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub category: Option<String>,
    pub adult_only: bool,
    /// `public`, `members` or `plus`.
    pub access: Option<String>,
    pub has_banner: bool,
    pub planning: bool,
    pub organizer: bool,
    pub instance_url: Option<String>,
    pub community: Option<Host>,
    pub discord: Option<Host>,
    pub steps: Vec<Step>,
    /// VRChat could not name every world yet: ask again shortly.
    pub worlds_pending: bool,
}

/// The member's invite status for each step (`pending`, `sending`, `sent`, `uncertain`, `failed`, `withdrawn`), and why
/// a failed or waiting one did not go (`not_friends`, `not_member`, …).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Invites {
    pub items: BTreeMap<String, String>,
    pub reasons: BTreeMap<String, String>,
}

/// The VRChat account sending a group's invites, which the member befriends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InviteAccount {
    pub name: Option<String>,
    pub vrchat_id: Option<String>,
}

/// Whether the community's account can invite the member (VRChat invites need a friendship).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Friend {
    pub is_friend: Option<bool>,
    /// The member's friend request is being accepted.
    pub accepting: bool,
    pub account: Option<InviteAccount>,
}

/// Whether the member may sign up for a step's invites: VRChat linked, in the group with its roles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Eligibility {
    pub linked: bool,
    pub allowed: bool,
    pub member: Option<bool>,
    pub friend: Option<Friend>,
    pub group: Option<Host>,
}

/// Invite me's answer: the status now (`pending` until the event's invites go out) and the friendship it needs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Signup {
    pub status: String,
    pub friend: Option<Friend>,
}

/// An error answer of the events routes: the server's message and its code (`SIGN_IN_REQUIRED`, `ADULTS_ONLY`,
/// `VRCHAT_LINK_REQUIRED`, `VRCHAT_GROUP_REQUIRED`).
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

impl Refused {
    /// The [`Refused`] inside an error of these calls, if Orbiters answered.
    pub fn of(e: &anyhow::Error) -> Option<&Refused> {
        e.downcast_ref()
    }
}

/// A person on Orbiters (a gallery's creator, a picture's author): their Discord picture when they have one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Member {
    pub id: u64,
    pub username: String,
    pub discord_id: Option<String>,
    pub avatar: Option<String>,
}

impl Member {
    /// Their Discord picture, 64 pixels.
    pub fn avatar_url(&self) -> Option<String> {
        let (id, hash) = (self.discord_id.as_deref()?, self.avatar.as_deref()?);
        (!id.is_empty() && !hash.is_empty() && !hash.contains('/')).then(|| format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=64"))
    }
}

/// A gallery the member can see: their own, one of a community they are in (members), or a public one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Gallery {
    pub id: u64,
    pub name: String,
    pub is_public: bool,
    /// `private`, `members` or `public`.
    pub visibility: String,
    pub owned: bool,
    pub creator: Option<Member>,
    /// The event the gallery belongs to, when the member may see it.
    pub event: Option<Card>,
    /// Its latest picture.
    pub cover_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Galleries {
    pub private_galleries: Vec<Gallery>,
    pub member_galleries: Vec<Gallery>,
    pub public_galleries: Vec<Gallery>,
}

impl Galleries {
    /// Every gallery, the member's own first, then their communities', then the public ones (the website's order).
    pub fn all(&self) -> impl Iterator<Item = &Gallery> {
        self.private_galleries.iter().chain(&self.member_galleries).chain(&self.public_galleries)
    }

    pub fn get(&self, id: u64) -> Option<&Gallery> {
        self.all().find(|g| g.id == id)
    }
}

/// The Discord message a picture was posted with, its mentions' names by id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    pub content: String,
    pub mentions: Mentions,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mentions {
    pub users: BTreeMap<String, String>,
    /// Orbiters keeps a role as `{ name, color }` (`replyContent.mentionsOf`); an older shape had its name alone.
    #[serde(deserialize_with = "names")]
    pub roles: BTreeMap<String, String>,
    pub channels: BTreeMap<String, String>,
}

/// Mentions' names by id, each a name or an object with its `name` (a role's colour is left out). Anything else reads
/// as no name (the mention then shows as Discord wrote it), never as an error that would lose the whole page.
fn names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    let raw: BTreeMap<String, serde_json::Value> = Deserialize::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(id, v)| match v {
            serde_json::Value::String(s) => Some((id, s)),
            serde_json::Value::Object(o) => o.get("name").and_then(|n| n.as_str()).map(|n| (id, n.to_string())),
            _ => None,
        })
        .collect())
}

impl Message {
    /// The text as people read it: Discord's mentions (`<@id>`, `<@&id>`, `<#id>`) as names.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.content.len());
        let mut rest = self.content.as_str();
        while let Some(start) = rest.find('<') {
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let Some(end) = tail.find('>') else {
                out.push_str(tail);
                return out;
            };
            let inner = &tail[1..end];
            let named = if let Some(name) = custom_emoji(inner) {
                Some(format!(":{name}:"))
            } else if let Some(id) = inner.strip_prefix("@&") {
                self.mentions.roles.get(id).map(|n| format!("@{n}"))
            } else if let Some(id) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@')) {
                self.mentions.users.get(id).map(|n| format!("@{n}"))
            } else if let Some(id) = inner.strip_prefix('#') {
                self.mentions.channels.get(id).map(|n| format!("#{n}"))
            } else {
                None
            };
            match named {
                Some(n) => out.push_str(&n),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        out
    }
}

/// The name of a Discord custom emoji as Discord writes it in a message (`:orbit:123…`, animated `a:orbit:123…`).
fn custom_emoji(inner: &str) -> Option<&str> {
    let rest = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':'))?;
    let (name, id) = rest.split_once(':')?;
    (!name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        .then_some(name)
}

/// A picture of a gallery.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Picture {
    /// Its placement in its gallery (the id the website's picture links use).
    pub id: u64,
    pub gallery_id: u64,
    /// Set for a picture of an asset's showcase gallery (`/assets/:id/showcase-images`) instead of a community's: its
    /// links are renewed at `/assets/<asset_id>/showcase-images/<id>/source`.
    pub asset_id: u64,
    /// A preview (at most 640 pixels for Discord's pictures) and the full picture.
    pub src: Option<String>,
    pub full_url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub posted_at: Option<String>,
    pub reaction_count: u32,
    pub reply_count: u32,
    pub message: Option<Message>,
    pub author: Option<Member>,
    /// The event its gallery belongs to.
    pub event: Option<Card>,
}

/// A picture's links renewed by Orbiters (`/galleries/images/:id/source`): Discord's links expire, and a picture whose
/// link did lists without one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Source {
    pub src: Option<String>,
    pub full_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pictures {
    pub items: Vec<Picture>,
    pub next_cursor: Option<serde_json::Value>,
}

impl Pictures {
    /// The cursor of the next page as the query takes it.
    pub fn next(&self) -> Option<String> {
        match self.next_cursor.as_ref()? {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// Adds the next page, without the pictures already shown (pages can overlap while new pictures arrive).
    pub fn append(&mut self, page: Pictures) {
        for p in page.items {
            if !self.items.iter().any(|q| q.id == p.id) {
                self.items.push(p);
            }
        }
        self.next_cursor = page.next_cursor;
    }
}

/// The answer as `T`, or what the events routes said ([`Refused`], with the server's code).
pub(crate) fn answer<T: serde::de::DeserializeOwned>(response: ureq::http::Response<ureq::Body>) -> anyhow::Result<T> {
    let status = response.status().as_u16();
    let text = response.into_body().with_config().limit(8 << 20).read_to_string().unwrap_or_default();
    if (200..300).contains(&status) {
        return Ok(serde_json::from_str(&text)?);
    }
    Err(refused(status, &text).into())
}

fn refused(status: u16, text: &str) -> Refused {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Body {
        error: String,
        message: String,
        code: Option<String>,
    }
    let body: Body = serde_json::from_str(text).unwrap_or_default();
    let said = if body.error.trim().is_empty() { body.message.trim().to_string() } else { body.error.trim().to_string() };
    let message = match (said.is_empty(), status) {
        (false, _) => said,
        (true, 401) => "Sign in to Orbiters in Settings first.".into(),
        (true, 404) => "This event is no longer on Orbiters.".into(),
        (true, 429) => "Orbiters is busy with many requests: try again in a moment.".into(),
        (true, _) => format!("Orbiters answered {status}."),
    };
    Refused { status, code: body.code.filter(|c| !c.is_empty()), message }
}

/// Only the path pieces Orbiters makes (uuids, `main`, numbers): nothing that would leave the route.
fn piece(s: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(!s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'), "not an Orbiters id");
    Ok(s)
}

impl Api {
    fn signed(&self, request: ureq::RequestBuilder<ureq::typestate::WithoutBody>, token: Option<&str>) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        match token {
            Some(t) => request.header("Authorization", &format!("Bearer {t}")),
            None => request,
        }
    }

    /// The events listed on Orbiters (the website's homepage) and the ones under way, soonest first.
    pub fn events(&self, offset: u64) -> anyhow::Result<Cards> {
        answer(self.agent().get(&self.url("community-events/upcoming")).query("live", "true").query("limit", "48").query("offset", offset.to_string()).call()?)
    }

    /// An event's page, as the member sees it (members-only events need `token`).
    pub fn event(&self, token: Option<&str>, id: &str) -> anyhow::Result<Overview> {
        let r = self.agent().get(&self.url(&format!("community-events/public/{}", piece(id)?)));
        answer(self.signed(r, token).call()?)
    }

    /// The url of an event's cover (read it with the member's credential: members-only covers need it).
    pub fn event_banner_url(&self, id: &str) -> anyhow::Result<String> {
        Ok(self.url(&format!("community-events/public/{}/banner", piece(id)?)))
    }

    /// The member's invite status for each of the event's steps.
    pub fn my_invites(&self, token: &str, id: &str) -> anyhow::Result<Invites> {
        let r = self.agent().get(&self.url(&format!("community-events/{}/invites/me", piece(id)?)));
        answer(self.signed(r, Some(token)).call()?)
    }

    /// Whether the member may sign up for a step's invites; `refresh` reads VRChat again (after joining the group).
    pub fn invite_eligibility(&self, token: &str, id: &str, step: &str, refresh: bool) -> anyhow::Result<Eligibility> {
        let r =
            self.agent().get(&self.url(&format!("community-events/{}/steps/{}/eligibility", piece(id)?, piece(step)?))).query("refresh", refresh.to_string());
        answer(self.signed(r, Some(token)).call()?)
    }

    /// Invite me: the community's VRChat account invites the member when the step's invites go out.
    pub fn invite_me(&self, token: &str, id: &str, step: &str) -> anyhow::Result<Signup> {
        let url = self.url(&format!("community-events/{}/steps/{}/invite", piece(id)?, piece(step)?));
        answer(self.agent().post(&url).header("Authorization", &format!("Bearer {token}")).send_empty()?)
    }

    /// Takes the member off a step's invites (one already sent stays sent).
    pub fn withdraw_invite(&self, token: &str, id: &str, step: &str) -> anyhow::Result<Signup> {
        let url = self.url(&format!("community-events/{}/steps/{}/invite", piece(id)?, piece(step)?));
        answer(self.agent().delete(&url).header("Authorization", &format!("Bearer {token}")).call()?)
    }

    /// The galleries the member can see.
    pub fn galleries(&self, token: &str) -> anyhow::Result<Galleries> {
        answer(self.signed(self.agent().get(&self.url("galleries")), Some(token)).call()?)
    }

    /// Pictures of every gallery the member can see (`gallery` None) or of one, newest first.
    pub fn gallery_pictures(&self, token: &str, gallery: Option<u64>, cursor: Option<&str>, limit: u32) -> anyhow::Result<Pictures> {
        let path = match gallery {
            Some(id) => format!("galleries/{id}/images"),
            None => "galleries/images".into(),
        };
        let mut r = self.agent().get(&self.url(&path)).query("sort", "date").query("limit", limit.clamp(1, 48).to_string());
        if let Some(c) = cursor {
            r = r.query("cursor", c);
        }
        answer(self.signed(r, Some(token)).call()?)
    }

    /// A picture's links, renewed from Discord; `failed`: the link given before did not load (Orbiters then asks
    /// Discord again even when its link looks fresh). `asset`: the asset whose showcase it is in (0: a community
    /// gallery's).
    pub fn picture_source(&self, token: &str, asset: u64, id: u64, failed: bool) -> anyhow::Result<Source> {
        let path = if asset == 0 { format!("galleries/images/{id}/source") } else { format!("assets/{asset}/showcase-images/{id}/source") };
        let mut r = self.agent().get(&self.url(&path));
        if failed {
            r = r.query("failed", "1");
        }
        answer(self.signed(r, Some(token)).call()?)
    }
}

/// The world and instance of an instance's launch link (`https://vrchat.com/home/launch?worldId=…&instanceId=…`), as
/// VRChat's logs write a location (`wrld_…:<instance>`).
pub fn launch_location(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let mut world = None;
    let mut instance = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        let v = decode(v)?;
        match k {
            "worldId" => world = Some(v),
            "instanceId" => instance = Some(v),
            _ => {}
        }
    }
    let (world, instance) = (world?, instance?);
    (world.starts_with("wrld_") && !instance.is_empty()).then(|| format!("{world}:{instance}"))
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_events_carry_their_cards() {
        let page: Cards = serde_json::from_str(include_str!("../tests/fixtures/events.json")).unwrap();
        assert!(page.items.len() >= 4);
        let first = &page.items[0];
        assert!(first.has_banner && first.steps >= 1);
        assert_eq!(first.community.as_ref().map(|h| h.name.as_str()), Some("Orbit Photo Club"));
        // A card without a cover, host or world (an older server's small shape) still reads.
        let bare: Card =
            serde_json::from_str(r#"{"id":"e","title":"T","description":"","startsAt":"2026-10-30T20:00:00.000Z","endsAt":null,"category":null}"#).unwrap();
        assert!(!bare.has_banner && bare.community.is_none() && bare.ends_at.is_none());
    }

    #[test]
    fn an_event_page_reads_with_its_steps() {
        let o: Overview = serde_json::from_str(include_str!("../tests/fixtures/event-overview.json")).unwrap();
        assert_eq!(o.steps.len(), 2);
        assert!(o.steps[0].invite_open && o.steps[0].world.as_ref().and_then(|w| w.name.as_deref()) == Some("Sunset Studio"));
        assert_eq!(o.community.as_ref().and_then(|c| c.account_id.as_deref()), Some("usr_c0ffee00-0000-4000-8000-000000000001"));
    }

    #[test]
    fn refusals_keep_the_server_s_words_and_code() {
        let r = refused(409, r#"{"error":"Connect VRChat to receive your invite.","code":"VRCHAT_LINK_REQUIRED"}"#);
        assert_eq!((r.status, r.code.as_deref(), r.message.as_str()), (409, Some("VRCHAT_LINK_REQUIRED"), "Connect VRChat to receive your invite."));
        assert_eq!(refused(404, "<html>").message, "This event is no longer on Orbiters.");
        assert_eq!(refused(503, "").message, "Orbiters answered 503.");
        assert_eq!(refused(401, r#"{"error":"Sign in to see this event.","code":"SIGN_IN_REQUIRED"}"#).code.as_deref(), Some("SIGN_IN_REQUIRED"));
    }

    #[test]
    fn only_orbiters_ids_go_into_paths() {
        assert!(piece("11111111-1111-4111-8111-111111111111").is_ok() && piece("main").is_ok());
        for bad in ["", "../galleries", "a/b", "x?y", "e%2F"] {
            assert!(piece(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn gallery_pictures_page_and_name_their_mentions() {
        let mut page: Pictures = serde_json::from_str(include_str!("../tests/fixtures/gallery-pictures.json")).unwrap();
        assert_eq!(page.next().as_deref(), Some("24"));
        let n = page.items.len();
        let again = page.clone();
        page.append(again);
        assert_eq!(page.items.len(), n, "pages overlapping keep each picture once");
        let m = Message {
            content: "Thanks <@1> and <@!2> in <#3> <@&4> <@9> <oops".into(),
            mentions: Mentions {
                users: [("1".into(), "Alex".into()), ("2".into(), "Mika".into())].into(),
                roles: [("4".into(), "Staff".into())].into(),
                channels: [("3".into(), "photos".into())].into(),
            },
        };
        assert_eq!(m.text(), "Thanks @Alex and @Mika in #photos @Staff <@9> <oops");
        // Orbiters' roles are objects (name and colour); custom emoji read as their names.
        let m: Message = serde_json::from_str(
            r##"{"content":"<@&4> <:orbit:923456789012345678> <a:wave_2:1> <:bad name:1>","mentions":{"users":{},"roles":{"4":{"name":"Photographers","color":"#ff8800"},"5":7},"channels":{}}}"##,
        )
        .unwrap();
        assert_eq!(m.text(), "@Photographers :orbit: :wave_2: <:bad name:1>");
        let author = Member { id: 1, username: "a".into(), discord_id: Some("42".into()), avatar: Some("abc".into()) };
        assert_eq!(author.avatar_url().as_deref(), Some("https://cdn.discordapp.com/avatars/42/abc.png?size=64"));
        assert_eq!(Member { avatar: Some("../x".into()), ..author }.avatar_url(), None);
        let all: Galleries = serde_json::from_str(include_str!("../tests/fixtures/galleries.json")).unwrap();
        assert!(all.all().count() >= 3 && all.all().any(|g| g.event.is_some()));
    }

    /// Against a disposable Orbiters: `ORBITERS_TEST_API` (its address), `ORBITERS_TEST_TOKEN` (a member's app
    /// credential, VRChat not linked) and `ORBITERS_TEST_EVENT` (a public event under way with a gallery and pictures).
    /// `cargo test -p orbiters-core community -- --ignored`
    #[test]
    #[ignore]
    fn reads_events_and_galleries_and_asks_for_an_invite_on_a_disposable_server() {
        let (Ok(base), Ok(token), Ok(event)) = (std::env::var("ORBITERS_TEST_API"), std::env::var("ORBITERS_TEST_TOKEN"), std::env::var("ORBITERS_TEST_EVENT"))
        else {
            return;
        };
        let api = Api::at(&base);
        let listed = api.events(0).unwrap();
        let card = listed.items.iter().find(|c| c.id == event).expect("the event under way is listed");
        assert!(card.community.is_some() && card.steps >= 1, "{card:?}");
        let page = api.event(Some(&token), &event).unwrap();
        assert!(!page.steps.is_empty() && page.steps[0].invite_open, "{page:?}");
        assert!(page.instance_url.as_deref().and_then(launch_location).is_some());
        api.my_invites(&token, &event).unwrap();
        let e = api.invite_eligibility(&token, &event, "main", true).unwrap();
        assert!(!e.linked && !e.allowed && e.group.is_some(), "{e:?}");
        let refused = api.invite_me(&token, &event, "main").unwrap_err();
        let r = Refused::of(&refused).expect("Orbiters' refusal");
        assert_eq!((r.status, r.code.as_deref()), (409, Some("VRCHAT_LINK_REQUIRED")), "{r:?}");
        let signed_out = api.invite_me("orbit-00000000-00000000-00000000-01-01-2026", &event, "main").unwrap_err();
        assert_eq!(Refused::of(&signed_out).map(|r| r.status), Some(401));
        assert_eq!(api.withdraw_invite(&token, &event, "main").unwrap().status, "withdrawn");
        let galleries = api.galleries(&token).unwrap();
        let gallery = galleries.all().find(|g| g.event.as_ref().is_some_and(|e| e.id == event)).expect("the event's gallery");
        let pictures = api.gallery_pictures(&token, Some(gallery.id), None, 12).unwrap();
        assert!(!pictures.items.is_empty());
        assert!(pictures.items.iter().all(|p| p.gallery_id == gallery.id && p.event.as_ref().is_some_and(|e| e.id == event)));
        let all = api.gallery_pictures(&token, None, None, 24).unwrap();
        assert!(all.items.len() >= pictures.items.len());
    }

    #[test]
    fn launch_links_become_vrchat_locations() {
        assert_eq!(
            launch_location("https://vrchat.com/home/launch?worldId=wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b&instanceId=12345~group(grp_1)~groupAccessType(public)~region(eu)")
                .as_deref(),
            Some("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b:12345~group(grp_1)~groupAccessType(public)~region(eu)")
        );
        assert_eq!(launch_location("https://vrchat.com/home/launch?worldId=wrld_1&instanceId=1%7Eregion%28us%29").as_deref(), Some("wrld_1:1~region(us)"));
        assert_eq!(launch_location("https://vrchat.com/home/group/grp_1"), None);
        assert_eq!(launch_location("https://vrchat.com/home/launch?worldId=x&instanceId=1"), None);
    }
}
