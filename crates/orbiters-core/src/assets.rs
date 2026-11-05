//! The showcase galleries of the Orbiters assets the member has, as Photos & captures shows them (My assets): each
//! asset's page has a gallery of pictures from the Discord rooms its creator chose (`/assets/:id/showcase-images`, the
//! website's `AssetShowcaseGallery`), and those are what the app shows, not the assets' own pictures (Enzo,
//! 2026-10-30).
//!
//! Which assets: `GET /assets/library` (a tool route, read only), everything the member made, holds a licence to
//! (bought, claimed, given) or reaches through a supporter tier or a Discord role. A server without it (answering
//! anything but a list) falls back on what the member made (`GET /creator/assets`) and the avatars they hold a licence
//! to (`GET /mcb/assets`). Each asset's showcase is read with the app's credential (a tool route too), as its page
//! shows it to the member: its owner sees every picture, anyone else the pictures of rooms open to their whole server.

use crate::account::Api;
use crate::community::{answer, Picture, Pictures, Refused};
use serde::{Deserialize, Serialize};

/// An asset's showcase pictures read at most (two of the server's pages).
pub const MOST: usize = 120;
/// The server's largest page.
const PAGE: usize = 60;

/// An asset the member has (the fields the gallery uses).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    /// AVATAR, ACCESSORY, CLOTHING, TEXTURE, PRODUCT, COMMISSION, YCH, STICKERS, PHYSICAL or OTHER.
    #[serde(rename = "type")]
    pub kind: String,
    pub how: How,
    /// Set for an art commission listing (its page is another one).
    pub commission_listing: Option<serde_json::Value>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    /// The member made it (its owner on Orbiters).
    #[default]
    Made,
    /// The member holds a licence to it (bought, or given) or reaches it through a tier or a role.
    Licensed,
}

/// An asset the member has and the pictures of its showcase gallery, in its page's order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Owned {
    pub asset: Asset,
    pub pictures: Vec<Picture>,
}

impl Owned {
    /// Its page on the website, relative to it (`assets/12`, `commissions/assets/12`).
    pub fn page(&self) -> String {
        let listing = self.asset.commission_listing.as_ref().is_some_and(|c| !c.is_null());
        format!("{}assets/{}", if listing { "commissions/" } else { "" }, self.asset.id)
    }

    /// When its showcase last got a picture (else when the asset changed): where its group goes among the others.
    pub fn date(&self) -> &str {
        self.pictures
            .iter()
            .filter_map(|p| p.posted_at.as_deref())
            .max()
            .or(self.asset.updated_at.as_deref())
            .or(self.asset.created_at.as_deref())
            .unwrap_or_default()
    }
}

/// "Avatar", "Clothing"… for an asset's type.
pub fn kind_label(kind: &str) -> &'static str {
    match kind.to_ascii_uppercase().as_str() {
        "AVATAR" => "Avatar",
        "ACCESSORY" => "Accessory",
        "CLOTHING" => "Clothing",
        "TEXTURE" => "Texture",
        "PRODUCT" => "Product",
        "COMMISSION" | "YCH" => "Commission",
        "STICKERS" => "Stickers",
        "PHYSICAL" => "Physical item",
        _ => "Asset",
    }
}

/// An avatar the member holds a licence to (`/mcb/assets`, the fallback list).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Licensed {
    pub id: u64,
    pub name: String,
}

/// What the member made, then the avatars they hold a licence to and did not make (a server without
/// `/assets/library`).
pub fn merge(made: Vec<Asset>, licensed: Vec<Licensed>) -> Vec<Asset> {
    let mut out: Vec<Asset> = made.into_iter().map(|a| Asset { how: How::Made, ..a }).collect();
    for l in licensed {
        if !out.iter().any(|a| a.id == l.id) {
            out.push(Asset { id: l.id, name: l.name, kind: "AVATAR".into(), how: How::Licensed, ..Default::default() });
        }
    }
    out
}

/// Each asset with its showcase (`showcase`'s answer), newest picture first. An asset whose showcase Orbiters does
/// not show the member (404: hidden, gone) has none. Every showcase refused to the app's credential (401: a server that
/// does not let the app read them yet) is an error.
pub fn gather(assets: Vec<Asset>, showcase: impl Fn(u64) -> anyhow::Result<Vec<Picture>> + Sync) -> anyhow::Result<Vec<Owned>> {
    let answers: Vec<anyhow::Result<Vec<Picture>>> = std::thread::scope(|s| {
        // Six at once: a member with many assets waits for a few rounds, not one after another.
        let mut out = Vec::with_capacity(assets.len());
        for chunk in assets.chunks(6) {
            let handles: Vec<_> = chunk.iter().map(|a| s.spawn(|| showcase(a.id))).collect();
            out.extend(handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("reading a showcase failed")))));
        }
        out
    });
    let refused = |e: &anyhow::Error| Refused::of(e).is_some_and(|r| matches!(r.status, 401 | 403));
    if !answers.is_empty() && answers.iter().all(|a| a.as_ref().is_err_and(refused)) {
        anyhow::bail!("Orbiters does not let this app read your assets' showcases yet.");
    }
    let mut first_error = None;
    let mut out = Vec::new();
    for (asset, answer) in assets.into_iter().zip(answers) {
        match answer {
            Ok(mut pictures) => {
                for p in &mut pictures {
                    p.asset_id = asset.id;
                }
                out.push(Owned { asset, pictures });
            }
            Err(e) if Refused::of(&e).is_some_and(|r| r.status == 404) => out.push(Owned { asset, pictures: Vec::new() }),
            Err(e) => {
                first_error.get_or_insert(e);
                out.push(Owned { asset, pictures: Vec::new() });
            }
        }
    }
    // Nothing read at all (Orbiters unreachable): say so rather than show no pictures.
    if let Some(e) = first_error {
        if out.iter().all(|o| o.pictures.is_empty()) {
            return Err(e);
        }
    }
    out.sort_by(|a, b| b.date().cmp(a.date()));
    Ok(out)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct List<T> {
    assets: Vec<T>,
}

impl Api {
    /// The assets the member has (`/assets/library`).
    pub fn library(&self, token: &str) -> anyhow::Result<Vec<Asset>> {
        let r = self.agent().get(&self.url("assets/library")).header("Authorization", &format!("Bearer {token}")).call()?;
        Ok(answer::<List<Asset>>(r)?.assets)
    }

    /// The assets the member made (owns on Orbiters).
    pub fn made_assets(&self, token: &str) -> anyhow::Result<Vec<Asset>> {
        let r = self.agent().get(&self.url("creator/assets")).header("Authorization", &format!("Bearer {token}")).call()?;
        Ok(Self::json::<List<Asset>>(r)?.assets)
    }

    /// The avatars the member holds a licence to.
    pub fn licensed_avatars(&self, token: &str) -> anyhow::Result<Vec<Licensed>> {
        let r = self.agent().get(&self.url("mcb/assets")).header("Authorization", &format!("Bearer {token}")).call()?;
        Ok(Self::json::<List<Licensed>>(r)?.assets)
    }

    /// A page of an asset's showcase gallery, in its page's order (the most relevant first).
    pub fn showcase_page(&self, token: &str, asset: u64, cursor: Option<&str>) -> anyhow::Result<Pictures> {
        let mut r = self.agent().get(&self.url(&format!("assets/{asset}/showcase-images"))).query("sort", "relevant").query("limit", PAGE.to_string());
        if let Some(c) = cursor {
            r = r.query("cursor", c);
        }
        answer(r.header("Authorization", &format!("Bearer {token}")).call()?)
    }

    /// An asset's showcase pictures, at most [`MOST`].
    pub fn showcase(&self, token: &str, asset: u64) -> anyhow::Result<Vec<Picture>> {
        let mut all = Pictures::default();
        let mut cursor: Option<String> = None;
        loop {
            all.append(self.showcase_page(token, asset, cursor.as_deref())?);
            match all.next() {
                Some(next) if all.items.len() < MOST && cursor.as_ref() != Some(&next) => cursor = Some(next),
                _ => break,
            }
        }
        all.items.truncate(MOST);
        Ok(all.items)
    }

    /// Every asset the member has, with its showcase ([`gather`]).
    pub fn owned_assets(&self, token: &str) -> anyhow::Result<Vec<Owned>> {
        let assets = match self.library(token) {
            Ok(a) => a,
            // A server without the library (an older one) answers with an error status: what the member made and
            // their avatars' licences instead.
            Err(e) if Refused::of(&e).is_some() => merge(self.made_assets(token)?, self.licensed_avatars(token).unwrap_or_default()),
            Err(e) => return Err(e),
        };
        gather(assets, |id| self.showcase(token, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(id: u64, at: &str) -> Picture {
        Picture { id, posted_at: Some(at.into()), src: Some(format!("https://media.discordapp.net/attachments/1/{id}/p.png")), ..Default::default() }
    }

    fn refused(status: u16) -> anyhow::Error {
        Refused { status, code: None, message: String::new() }.into()
    }

    #[test]
    fn the_library_reads_as_the_server_sends_it() {
        let list: List<Asset> = serde_json::from_str(
            r#"{"assets": [
                {"id": 7, "name": "Fox", "type": "AVATAR", "how": "made", "commissionListing": null, "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-20T10:00:00.000Z"},
                {"id": 9, "name": "Sketch", "type": "YCH", "how": "licensed", "commissionListing": {"published": true}}
            ]}"#,
        )
        .unwrap();
        let [fox, sketch] = &list.assets[..] else { panic!() };
        assert_eq!((fox.how, sketch.how), (How::Made, How::Licensed));
        assert_eq!(Owned { asset: sketch.clone(), ..Default::default() }.page(), "commissions/assets/9");
        assert_eq!(Owned { asset: fox.clone(), ..Default::default() }.page(), "assets/7");
        assert_eq!(kind_label(&fox.kind), "Avatar");
        // A showcase picture as `/assets/:id/showcase-images` lists it.
        let page: Pictures = serde_json::from_str(
            r#"{"items": [{"id": 31, "assetId": 7, "discordImageId": 5, "src": null, "fullUrl": null, "sourceEndpoint": "/assets/7/showcase-images/31/source",
                "width": 1200, "height": 800, "postedAt": "2026-10-28T20:00:00.000Z", "reactionCount": 3, "author": {"id": 2, "username": "Mika"}}],
                "nextCursor": "60", "sort": "relevant", "canManage": false}"#,
        )
        .unwrap();
        assert_eq!((page.items[0].id, page.items[0].asset_id, page.next().as_deref()), (31, 7, Some("60")));
    }

    #[test]
    fn each_asset_gets_its_showcase_newest_first_and_hidden_ones_none() {
        let assets = vec![
            Asset { id: 1, name: "Old".into(), updated_at: Some("2026-10-29T00:00:00Z".into()), ..Default::default() },
            Asset { id: 2, name: "Busy".into(), how: How::Licensed, ..Default::default() },
            Asset { id: 3, name: "Hidden".into(), ..Default::default() },
        ];
        let owned = gather(assets, |id| match id {
            1 => Ok(vec![picture(10, "2026-09-01T00:00:00Z")]),
            2 => Ok(vec![picture(20, "2026-10-28T00:00:00Z"), picture(21, "2026-10-30T00:00:00Z")]),
            _ => Err(refused(404)),
        })
        .unwrap();
        let order: Vec<(&str, usize)> = owned.iter().map(|o| (o.asset.name.as_str(), o.pictures.len())).collect();
        assert_eq!(order, [("Busy", 2), ("Old", 1), ("Hidden", 0)], "by their newest showcase picture, not the asset's own date");
        assert!(owned[0].pictures.iter().all(|p| p.asset_id == 2), "each picture knows its asset (its link is renewed there)");
        assert_eq!(owned[0].pictures[0].id, 20, "in its page's order");
    }

    #[test]
    fn a_server_that_refuses_the_showcases_says_so() {
        let assets = vec![Asset { id: 1, ..Default::default() }, Asset { id: 2, ..Default::default() }];
        let e = gather(assets.clone(), |_| Err(refused(401))).unwrap_err();
        assert!(e.to_string().contains("showcases"), "{e}");
        assert!(gather(Vec::new(), |_| Err(refused(401))).unwrap().is_empty(), "no assets: nothing to say");
        // One unreachable among answered ones: those are shown.
        let owned = gather(assets, |id| if id == 1 { Ok(vec![picture(5, "2026-10-01T00:00:00Z")]) } else { Err(anyhow::anyhow!("timed out")) }).unwrap();
        assert_eq!(owned.iter().map(|o| o.pictures.len()).sum::<usize>(), 1);
    }

    #[test]
    fn without_the_library_what_they_made_and_licences_join_once() {
        let made = vec![Asset { id: 1, name: "Mine".into(), kind: "ACCESSORY".into(), ..Default::default() }];
        let licensed = vec![Licensed { id: 1, name: "Mine".into() }, Licensed { id: 2, name: "Bought".into() }];
        let all = merge(made, licensed);
        let names: Vec<(&str, How, &str)> = all.iter().map(|a| (a.name.as_str(), a.how, a.kind.as_str())).collect();
        assert_eq!(names, [("Mine", How::Made, "ACCESSORY"), ("Bought", How::Licensed, "AVATAR")]);
    }
}
