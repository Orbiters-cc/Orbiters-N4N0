//! What people said about a community gallery's picture on Discord, as the website's picture window shows it
//! (`server/frontend/src/components/elements/galleryReplies`): the replies to the Discord message the picture was
//! posted with (`GET /galleries/images/:id/replies`, oldest first, at most 200, the total and the message's link), and
//! who reacted with each emoji (`GET /galleries/images/:id/reactions`, the most used first, at most 100 names each
//! with the full count). Both follow the picture's access: a picture the member cannot open answers 403 or 404.

use crate::account::Api;
use crate::community::{answer, Mentions, Message};
use orbiters_i18n::{fill, tr, tr_n};
use serde::{Deserialize, Serialize};

/// Who wrote a reply, as Discord showed them in the server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Person {
    pub discord_id: Option<String>,
    pub name: String,
    /// Their Discord picture (Orbiters keeps only Discord's links).
    pub avatar_url: Option<String>,
    /// Their Orbiters profile, when they have one that may be shown.
    pub profile_id: Option<u64>,
}

impl Person {
    /// Their picture, when it is a Discord link (nothing else is loaded).
    pub fn avatar(&self) -> Option<&str> {
        self.avatar_url.as_deref().filter(|u| u.starts_with("https://cdn.discordapp.com/") || u.starts_with("https://media.discordapp.net/"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct File {
    pub name: String,
    pub content_type: Option<String>,
}

/// A sticker; format 3 (Lottie) has no picture and shows as its name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sticker {
    pub id: String,
    pub name: String,
    pub format: u8,
}

/// A message replying to the picture's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reply {
    pub id: serde_json::Value,
    pub author: Person,
    pub content: String,
    pub mentions: Mentions,
    pub attachments: Vec<File>,
    pub stickers: Vec<Sticker>,
    pub posted_at: Option<String>,
    pub edited_at: Option<String>,
}

impl Reply {
    /// Its words as people read them (mentions as names, custom emoji as `:name:`).
    pub fn text(&self) -> String {
        Message { content: self.content.clone(), mentions: self.mentions.clone() }.text().trim().to_string()
    }

    /// What it carries besides its words: its stickers' and files' names.
    pub fn extras(&self) -> Vec<String> {
        self.stickers.iter().map(|s| s.name.clone()).chain(self.attachments.iter().map(|f| f.name.clone())).filter(|n| !n.trim().is_empty()).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Replies {
    pub items: Vec<Reply>,
    pub total: u32,
    /// The picture's Discord message.
    pub message_url: Option<String>,
}

impl Replies {
    /// How many replies are not listed (the list stops at 200): they are on Discord.
    pub fn more(&self) -> u32 {
        self.total.saturating_sub(self.items.len() as u32)
    }

    /// The Discord link of the picture's message, when it is one.
    pub fn discord_link(&self) -> Option<&str> {
        self.message_url.as_deref().filter(|u| u.starts_with("https://discord.com/") || u.starts_with("https://discordapp.com/"))
    }
}

/// A reaction's emoji: one of Discord's (`id` none, `name` the emoji itself) or a server's own (its picture on
/// Discord's CDN).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Emoji {
    pub key: String,
    pub id: Option<String>,
    pub name: String,
    pub animated: bool,
}

impl Emoji {
    /// A server's own emoji's picture (its first frame when it is animated).
    pub fn image_url(&self) -> Option<String> {
        let id = self.id.as_deref().filter(|id| !id.is_empty() && id.len() <= 30 && id.chars().all(|c| c.is_ascii_digit()))?;
        Some(format!("https://cdn.discordapp.com/emojis/{id}.png?size=48"))
    }

    /// How it reads in words: the emoji itself, or `:name:` for a server's own.
    pub fn label(&self) -> String {
        if self.id.is_some() {
            format!(":{}:", self.name)
        } else if self.name.is_empty() {
            self.key.clone()
        } else {
            self.name.clone()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reaction {
    pub emoji: Emoji,
    pub count: u32,
    pub users: Vec<Person>,
}

impl Reaction {
    /// Who reacted, in words: "Alex", "Alex and Mika", "Alex, Mika and Robin", the first `max` names and "and 3 others"
    /// for the rest (those Orbiters does not name, a closed account, count among them).
    pub fn who(&self, max: usize) -> String {
        let names: Vec<&str> = self.users.iter().map(|u| u.name.trim()).filter(|n| !n.is_empty()).take(max.max(1)).collect();
        let others = (self.count as usize).saturating_sub(names.len());
        let comma = tr(", ");
        // The names, in the language's way of listing: "Alex, Mika and Robin"; what is left over is a count that ends the sentence.
        let listed = |names: &[&str]| match names {
            [] => String::new(),
            [only] => only.to_string(),
            [first @ .., last] => tr!("{names} and {last}", names = first.join(comma), last = last),
        };
        match (names.is_empty(), others) {
            (true, 0) => String::new(),
            (true, n) => tr_n("{count} person", "{count} people", n as u64),
            (false, 0) => listed(&names),
            (false, n) => fill(&tr_n("{names} and {count} other", "{names} and {count} others", n as u64), &[("names", &names.join(comma))]),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reactions {
    pub items: Vec<Reaction>,
    pub total: u32,
}

impl Api {
    /// The replies to the Discord message a gallery picture (its placement `id`) was posted with.
    pub fn picture_replies(&self, token: &str, id: u64) -> anyhow::Result<Replies> {
        let r = self.agent().get(&self.url(&format!("galleries/images/{id}/replies"))).header("Authorization", &format!("Bearer {token}"));
        answer(r.call()?)
    }

    /// Who reacted to a gallery picture (its placement `id`), by emoji.
    pub fn picture_reactions(&self, token: &str, id: u64) -> anyhow::Result<Reactions> {
        let r = self.agent().get(&self.url(&format!("galleries/images/{id}/reactions"))).header("Authorization", &format!("Bearer {token}"));
        answer(r.call()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_read_as_the_website_gets_them() {
        let page: Replies = serde_json::from_str(include_str!("../tests/fixtures/gallery-replies.json")).unwrap();
        assert_eq!((page.items.len(), page.total, page.more()), (3, 5, 2));
        assert_eq!(page.discord_link(), Some("https://discord.com/channels/223456789012345678/323456789012345678/423456789012345601"));
        let first = &page.items[0];
        assert_eq!(first.author.name, "Mika");
        assert_eq!(first.author.profile_id, Some(7));
        assert_eq!(first.text(), "Lovely shot @Alex! :orbit: Photographers, look: @Photographers");
        assert_eq!(page.items[1].extras(), ["Wave", "behind-the-scenes.png"]);
        assert_eq!(page.items[2].author.avatar(), None, "only Discord's pictures are loaded");
        assert!(Replies { message_url: Some("https://evil.example/x".into()), ..Default::default() }.discord_link().is_none());
    }

    #[test]
    fn reactions_say_who_reacted() {
        let page: Reactions = serde_json::from_str(include_str!("../tests/fixtures/gallery-reactions.json")).unwrap();
        assert_eq!(page.total, 7);
        let [orbit, heart, fire] = &page.items[..] else { panic!("three emoji") };
        assert_eq!(orbit.emoji.label(), ":orbit:");
        assert_eq!(orbit.emoji.image_url().as_deref(), Some("https://cdn.discordapp.com/emojis/923456789012345678.png?size=48"));
        assert_eq!((heart.emoji.label(), heart.emoji.image_url()), ("\u{2764}\u{fe0f}".to_string(), None));
        assert_eq!(orbit.who(40), "Alex, Mika and Robin");
        assert_eq!(orbit.who(2), "Alex, Mika and 1 other");
        assert_eq!(heart.who(40), "Alex and 2 others", "a closed account counts without its name");
        assert_eq!(fire.who(40), "1 person");
        let two = Reaction { count: 2, users: vec![Person { name: "Alex".into(), ..Default::default() }, Person { name: "Mika".into(), ..Default::default() }], ..Default::default() };
        assert_eq!(two.who(40), "Alex and Mika");
        assert_eq!(Reaction { count: 3, ..Default::default() }.who(40), "3 people");
        assert_eq!(Reaction::default().who(40), "");
        // The list and the count of those left over are each language's own.
        use orbiters_i18n::{in_language, Language};
        in_language(Language::Fr, || {
            assert_eq!(orbit.who(40), "Alex, Mika et Robin");
            assert_eq!(orbit.who(2), "Alex, Mika et 1 autre personne");
            assert_eq!(heart.who(40), "Alex et 2 autres personnes");
            assert_eq!(fire.who(40), "1 personne");
        });
        in_language(Language::Ja, || {
            assert_eq!(orbit.who(2), "Alex、Mikaほか1人");
            assert_eq!(two.who(40), "AlexとMika");
        });
        let bad = Emoji { id: Some("../x".into()), name: "x".into(), ..Default::default() };
        assert_eq!(bad.image_url(), None);
    }
}
