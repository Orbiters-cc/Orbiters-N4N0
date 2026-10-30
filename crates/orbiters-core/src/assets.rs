//! The Orbiters assets the member has, with their pictures, as Photos & captures shows them (My assets): those they
//! made (`GET /creator/assets`, every asset they own, with its gallery) and the avatars they hold a licence to (`GET
//! /mcb/assets`), whose pictures come from their public page (`GET /assets/:id`). Both lists are tool routes the
//! member's app credential signs in to; an asset's page is read as a visitor (the server does not read the app's
//! credential there), so a bought avatar that is not published shows its thumbnail alone.
//!
//! An asset's pictures are what its page shows: its product gallery (`product.galleryFileIds`, each
//! `/creator-tools/media/<id>`), else its preview links (store-synced listings), else its thumbnail.
//!
//! Not reachable with the app's credential (server, 2026-10-30): the website's library of everything the member bought
//! or can use (`GET /assets`, its JWT only), the pictures of an unpublished asset (`/creator-tools/media/:id` reads
//! no app credential), and an asset's community pictures (`/assets/:id/showcase-images`).

use crate::account::Api;
use serde::{Deserialize, Serialize};

/// An asset as Orbiters lists it (the fields the gallery uses).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    /// AVATAR, ACCESSORY, CLOTHING, TEXTURE, PRODUCT, COMMISSION, YCH, STICKERS, PHYSICAL or OTHER.
    #[serde(rename = "type")]
    pub kind: String,
    pub thumbnail: Option<String>,
    /// Links (absolute, or a path on the API): the gallery's mirror, or a store's pictures.
    pub previews: Vec<serde_json::Value>,
    pub product: Option<Product>,
    /// Set for an art commission listing (its page is another one).
    pub commission_listing: Option<serde_json::Value>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Product {
    /// The gallery's files, in its order (numbers; text tolerated).
    pub gallery_file_ids: Vec<serde_json::Value>,
}

/// An avatar the member holds a licence to (`/mcb/assets`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Licensed {
    pub id: u64,
    pub name: String,
    pub owner_id: Option<u64>,
    pub thumbnail_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum How {
    /// The member made it (its owner on Orbiters).
    #[default]
    Made,
    /// The member holds a licence to it (bought, or given).
    Licensed,
}

/// An asset the member has and its pictures (absolute links).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Owned {
    pub asset: Asset,
    pub how: How,
    pub pictures: Vec<String>,
}

impl Owned {
    /// Its page on the website, relative to it (`assets/12`, `commissions/assets/12`).
    pub fn page(&self) -> String {
        let listing = self.asset.commission_listing.as_ref().is_some_and(|c| !c.is_null());
        format!("{}assets/{}", if listing { "commissions/" } else { "" }, self.asset.id)
    }

    /// When it last changed (its pictures carry no time of their own).
    pub fn date(&self) -> &str {
        self.asset.updated_at.as_deref().or(self.asset.created_at.as_deref()).unwrap_or_default()
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

/// A link of the API absolute (`/creator-tools/media/3` on `api`), https links kept, anything else left out.
fn absolute(api: &Api, link: &str) -> Option<String> {
    let link = link.trim();
    if link.starts_with("https://") {
        Some(link.to_string())
    } else if link.starts_with('/') && !link.starts_with("//") {
        Some(api.url(link))
    } else {
        None
    }
}

/// The pictures its page shows, as absolute links: its gallery, else its previews, else its thumbnail; each once.
pub fn pictures(api: &Api, asset: &Asset) -> Vec<String> {
    let gallery: Vec<String> = asset
        .product
        .iter()
        .flat_map(|p| &p.gallery_file_ids)
        .filter_map(|id| id.as_u64().or_else(|| id.as_str()?.parse().ok()))
        .map(|id| api.url(&format!("creator-tools/media/{id}")))
        .collect();
    let mut out = if gallery.is_empty() { asset.previews.iter().filter_map(|p| absolute(api, p.as_str()?)).collect() } else { gallery };
    if out.is_empty() {
        out.extend(asset.thumbnail.as_deref().and_then(|t| absolute(api, t)));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|u| seen.insert(u.clone()));
    out
}

/// What the member made, then the avatars they hold a licence to and did not make (with `page`'s answer for each,
/// None when its page could not be read: its thumbnail alone), newest change first.
pub fn gather(api: &Api, made: Vec<Asset>, licensed: Vec<Licensed>, mut page: impl FnMut(u64) -> Option<Asset>) -> Vec<Owned> {
    let mut out: Vec<Owned> = made.into_iter().map(|asset| Owned { pictures: pictures(api, &asset), asset, how: How::Made }).collect();
    for l in licensed {
        if out.iter().any(|o| o.asset.id == l.id) {
            continue;
        }
        let asset = page(l.id).unwrap_or_else(|| Asset { id: l.id, name: l.name.clone(), kind: "AVATAR".into(), ..Default::default() });
        let mut pictures = pictures(api, &asset);
        if pictures.is_empty() {
            pictures.extend(l.thumbnail_url.as_deref().and_then(|t| absolute(api, t)));
        }
        let name = if asset.name.is_empty() { l.name } else { asset.name.clone() };
        out.push(Owned { asset: Asset { name, ..asset }, how: How::Licensed, pictures });
    }
    out.sort_by(|a, b| b.date().cmp(a.date()));
    out
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct List<T> {
    assets: Vec<T>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Page {
    asset: Asset,
}

impl Api {
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

    /// An asset's public page (as a visitor: published listings only).
    pub fn public_asset(&self, id: u64) -> anyhow::Result<Asset> {
        Ok(Self::json::<Page>(self.agent().get(&self.url(&format!("assets/{id}"))).call()?)?.asset)
    }

    /// Every asset the member has, with its pictures ([`gather`]). What they made must answer; a licence list or a
    /// page that does not leaves those out.
    pub fn owned_assets(&self, token: &str) -> anyhow::Result<Vec<Owned>> {
        let made = self.made_assets(token)?;
        let licensed = self.licensed_avatars(token).unwrap_or_default();
        Ok(gather(self, made, licensed, |id| self.public_asset(id).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_asset_shows_its_gallery_else_its_previews_else_its_thumbnail() {
        let api = Api::at("https://api.orbiters.cc/");
        let list: List<Asset> = serde_json::from_str(
            r#"{"assets": [
                {"id": 7, "name": "Fox", "type": "AVATAR", "thumbnail": "/creator-tools/media/3", "previews": ["/creator-tools/media/9"],
                 "product": {"galleryFileIds": [9, "11", 9]}, "commissionListing": null, "updatedAt": "2026-10-20T10:00:00.000Z"},
                {"id": 8, "name": "Hat", "type": "ACCESSORY", "thumbnail": "https://cdn.example/hat.png",
                 "previews": ["https://public-files.gumroad.com/a.jpg", null, "http://plain/b.jpg", "//cdn/c.jpg"], "product": null},
                {"id": 9, "name": "Sketch", "type": "YCH", "thumbnail": "/files/serve/4", "previews": [], "commissionListing": {"published": true}}
            ]}"#,
        )
        .unwrap();
        let [fox, hat, sketch] = &list.assets[..] else { panic!() };
        assert_eq!(pictures(&api, fox), ["https://api.orbiters.cc/creator-tools/media/9", "https://api.orbiters.cc/creator-tools/media/11"]);
        assert_eq!(pictures(&api, hat), ["https://public-files.gumroad.com/a.jpg"], "store pictures, only over https");
        assert_eq!(pictures(&api, sketch), ["https://api.orbiters.cc/files/serve/4"], "its thumbnail when it has nothing else");
        assert_eq!(kind_label(&fox.kind), "Avatar");
        assert_eq!(Owned { asset: sketch.clone(), ..Default::default() }.page(), "commissions/assets/9");
        assert_eq!(Owned { asset: fox.clone(), ..Default::default() }.page(), "assets/7");
    }

    #[test]
    fn bought_avatars_join_what_the_member_made_once() {
        let api = Api::at("https://api.orbiters.cc/");
        let made = vec![Asset { id: 1, name: "Mine".into(), updated_at: Some("2026-10-01T00:00:00Z".into()), ..Default::default() }];
        let licensed: List<Licensed> = serde_json::from_str(
            r#"{"assets": [
                {"id": 1, "name": "Mine", "ownerId": 5, "thumbnailUrl": "https://api.orbiters.cc/a.png"},
                {"id": 2, "name": "Bought", "ownerId": 6, "thumbnailUrl": "https://api.orbiters.cc/b.png"},
                {"id": 3, "name": "Unpublished", "ownerId": 6, "thumbnailUrl": "https://api.orbiters.cc/c.png"}
            ]}"#,
        )
        .unwrap();
        let mut asked = Vec::new();
        let owned = gather(&api, made, licensed.assets, |id| {
            asked.push(id);
            (id == 2).then(|| Asset { id: 2, name: "Bought".into(), kind: "AVATAR".into(), product: Some(Product { gallery_file_ids: vec![42.into()] }), updated_at: Some("2026-10-25T00:00:00Z".into()), ..Default::default() })
        });
        assert_eq!(asked, [2, 3], "what the member made is not read again");
        let names: Vec<(&str, How)> = owned.iter().map(|o| (o.asset.name.as_str(), o.how)).collect();
        assert_eq!(names, [("Bought", How::Licensed), ("Mine", How::Made), ("Unpublished", How::Licensed)]);
        assert_eq!(owned[0].pictures, ["https://api.orbiters.cc/creator-tools/media/42"]);
        assert_eq!(owned[2].pictures, ["https://api.orbiters.cc/c.png"], "an unreadable page: its thumbnail");
        assert_eq!(owned[2].asset.kind, "AVATAR");
    }
}
