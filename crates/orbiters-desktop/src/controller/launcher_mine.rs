//! The account's own profile on Home (REQUEST15 7b): read once after each signed-in read, changed only on the
//! person's own press, one change at a time, with VRChat's limits held here before anything is sent.
//!
//! Paths and bodies from the community OpenAPI spec (vrchatapi/specification, 2026-11-20): `getPublicProfile` with
//! `asSelf=true`, `updateUser` (`status`, `statusDescription`), `updateProfile` (`bio`, `userIcon`, `bannerType`,
//! `bannerColor`, `bannerCustomUrl`), `getFiles` (`tag=icon`, `tag=gallery`). How a picture of the account's files is
//! named for `userIcon` / `bannerCustomUrl` (its latest version's `file.url`) is VRCX's (EditProfileDialog.vue,
//! GallerySelectDialog.vue), read 2026-11-20. Every call goes through the client's pacing and its 429 back-off.
use super::*;
use orbiters_core::launcher::{BannerChoice, MyProfile, OwnPicture, PictureKind, ProfileEdit, ProfilePart, Saving, Status, BIO_MAX, STATUS_TEXT_MAX};
use serde_json::Value;

/// VRChat refuses profile changes in quick succession (it does not say how quick): one every few seconds from here.
pub(super) const CHANGE_EVERY: Duration = Duration::from_secs(5);

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn https(v: &Value, key: &str) -> String {
    Some(text(v, key)).filter(|u| u.starts_with("https://")).unwrap_or_default()
}

/// The account's own profile from VRChat's `PublicProfile` (asked as itself), keeping the lists already read.
pub fn my_profile(v: &Value, kept: Option<&MyProfile>) -> MyProfile {
    let banner = match text(v, "bannerType").as_str() {
        "avatarBanner" => BannerChoice::Avatar,
        "customImage" => BannerChoice::Custom,
        _ => BannerChoice::Colour,
    };
    let kept = kept.cloned().unwrap_or_default();
    MyProfile {
        bio: text(v, "bio"),
        banner,
        banner_colour: launcher_feed::colour(&text(v, "bannerColor")),
        banner_custom: https(v, "bannerCustomUrl"),
        icon: https(v, "userIcon"),
        avatar_picture: Some(https(v, "currentAvatarThumbnailImageUrl")).filter(|u| !u.is_empty()).unwrap_or_else(|| https(v, "currentAvatarImageUrl")),
        supporter: v.get("hasVrcPlus").and_then(Value::as_bool).unwrap_or(kept.supporter),
        ..kept
    }
}

/// A file of the account's (`getFiles`): its id and its latest version's address.
pub fn own_picture(v: &Value) -> Option<OwnPicture> {
    let id = text(v, "id");
    let url = v.get("versions")?.as_array()?.iter().rev().find_map(|version| version.get("file").and_then(|f| f.get("url")).and_then(Value::as_str).map(str::to_string))?;
    (id.starts_with("file_") && url.starts_with("https://api.vrchat.cloud/api/1/file/")).then_some(OwnPicture { id, url })
}

pub fn status_value(status: Status) -> Option<&'static str> {
    match status {
        Status::JoinMe => Some("join me"), // i18n: not shown (VRChat's API value)
        Status::Active => Some("active"), // i18n: not shown (VRChat's API value)
        Status::AskMe => Some("ask me"), // i18n: not shown (VRChat's API value)
        Status::Busy => Some("busy"), // i18n: not shown (VRChat's API value)
        Status::Offline => None,
    }
}

/// What the page's limits are in words, when `edit` is past them.
pub fn over_limit(edit: &ProfileEdit) -> Option<String> {
    match edit {
        ProfileEdit::StatusText(t) if t.chars().count() > STATUS_TEXT_MAX => Some(tr!("A status text is {n} characters at most.", n = STATUS_TEXT_MAX)),
        ProfileEdit::Bio(t) if t.chars().count() > BIO_MAX => Some(tr!("A bio is {n} characters at most.", n = BIO_MAX)),
        ProfileEdit::Status(s) if status_value(*s).is_none() => Some(tr("Choose Join me, Online, Ask me or Busy.").to_string()),
        ProfileEdit::Banner { choice: BannerChoice::Custom, custom, .. } if custom.is_empty() => Some(tr("Choose a picture of your gallery first.").to_string()),
        ProfileEdit::Banner { choice: BannerChoice::Colour, colour: None, .. } => Some(tr("Choose a colour first.").to_string()),
        _ => None,
    }
}

impl Controller {
    /// The account's own profile, after a signed-in read of `account`: into the state only while that account's list
    /// is on show.
    pub(super) fn launcher_read_mine(&self, client: &Arc<Mutex<Client>>, account: &str) {
        let Ok(v) = send_as(client, account, |c| c.own_profile()) else { return };
        self.update_if(|s| {
            if !matches!(&s.launcher.source, Source::SignedIn { user_id, .. } if user_id == account) {
                return false;
            }
            let mine = my_profile(&v, s.launcher.mine.as_ref());
            let changed = s.launcher.mine.as_ref() != Some(&mine);
            if changed {
                s.launcher.mine = Some(mine);
            }
            changed
        });
    }

    /// Changes the own profile on show, only while the list on show is `account`'s (`None`: no signed-in list): an answer
    /// that comes back after the person switched accounts lands on nobody's card (REQUEST15 fix B3). Whether it landed.
    pub(super) fn launcher_mine_publish(&self, account: Option<&str>, f: impl FnOnce(&mut MyProfile)) -> bool {
        self.update_if(|s| {
            let shown = match &s.launcher.source {
                Source::SignedIn { user_id, .. } => Some(user_id.as_str()),
                _ => None,
            };
            if shown != account {
                return false;
            }
            f(s.launcher.mine.get_or_insert_with(MyProfile::default));
            true
        })
    }

    pub(super) fn launcher_mine_saving(&self, account: Option<&str>, saving: Saving, next: Option<i64>) {
        self.launcher_mine_publish(account, |mine| {
            // Saving is no answer yet; Saved and Failed are.
            if !matches!(saving, Saving::Saving(_)) {
                mine.answered += 1;
            }
            mine.saving = Some(saving);
            if next.is_some() {
                mine.next_change_at = next;
            }
        });
    }

    /// A change of the account's own profile, pressed by the person on the list of `account`: checked against VRChat's
    /// limits and our own pace, then sent with that account's session only. The card shows Saving, then Saved or why not.
    pub(super) fn launcher_edit_mine(self: &Shared, edit: ProfileEdit, account: Option<&str>) -> Option<Notice> {
        let part = edit.part();
        if self.launcher_client().is_none() {
            self.launcher_mine_saving(account, Saving::Failed(part, tr("Sign in to VRChat in Settings to change your profile here.").into()), None);
            return None;
        }
        let (client, account) = match (self.launcher_sender(account), account) {
            (Ok(client), Some(account)) => (client, account.to_string()),
            (sender, _) => {
                let text = match notice_or_changed(sender.err()) {
                    Notice::Error(t) | Notice::Failed { text: t, .. } => t,
                    _ => tr("Sign in to VRChat in Settings to change your profile here.").to_string(),
                };
                self.launcher_mine_saving(account, Saving::Failed(part, text), None);
                return None;
            }
        };
        if let Some(why) = over_limit(&edit) {
            self.launcher_mine_saving(Some(&account), Saving::Failed(part, why), None);
            return None;
        }
        // One change at a time, a few seconds apart.
        {
            let mut rt = self.launcher.lock().unwrap();
            if let Some(at) = rt.mine_next.filter(|at| Instant::now() < *at) {
                let left = at.saturating_duration_since(Instant::now()).as_secs().max(1);
                drop(rt);
                self.launcher_mine_saving(Some(&account), Saving::Failed(part, tr!("VRChat takes one change every few seconds: try again in {n} s.", n = left)), None);
                return None;
            }
            rt.mine_next = Some(Instant::now() + CHANGE_EVERY);
        }
        self.launcher_mine_saving(Some(&account), Saving::Saving(part), Some(now() + CHANGE_EVERY.as_secs() as i64));
        // A picture goes only when it is one of this account's own files, as VRChat lists them now: never a URL that
        // merely looks like one (another person's file on the same host, a list read for another account).
        let own = match &edit {
            ProfileEdit::Picture(url) if !url.is_empty() => Some(("icon", url.clone())),
            ProfileEdit::Banner { choice: BannerChoice::Custom, custom, .. } => Some(("gallery", custom.clone())),
            _ => None,
        };
        if let Some((tag, url)) = own {
            let listed = send_as(&client, &account, |c| c.own_files(tag)).map(|files| files.iter().filter_map(own_picture).any(|p| p.url == url));
            let why = match listed {
                Ok(true) => None,
                Ok(false) => Some(tr("This picture is not one of your account's: open the list again and choose one.").to_string()),
                Err(Notice::Error(t) | Notice::Failed { text: t, .. }) => Some(t),
                Err(_) => Some(tr("VRChat did not take the change. Try again in a moment.").to_string()),
            };
            if let Some(why) = why {
                self.launcher_mine_saving(Some(&account), Saving::Failed(part, why), None);
                return None;
            }
        }
        let sent = match &edit {
            ProfileEdit::Status(status) => send_as(&client, &account, |c| c.update_own_status(status_value(*status), None)),
            ProfileEdit::StatusText(t) => send_as(&client, &account, |c| c.update_own_status(None, Some(t))),
            other => match orbiters_core::vrchat_api::own_profile_body(other) {
                Ok(body) => send_as(&client, &account, |c| c.update_own_profile(&body)),
                Err(e) => Err(Notice::Error(error_text(&e))),
            },
        };
        match sent {
            Ok(v) => self.update(|s| {
                if !matches!(&s.launcher.source, Source::SignedIn { user_id, .. } if *user_id == account) {
                    return;
                }
                let avatar = s.launcher.mine.as_ref().map(|m| m.avatar_picture.clone()).unwrap_or_default();
                if let Some(me) = s.launcher.me.as_mut() {
                    // What VRChat answered is what shows; what it did not say, what was asked for.
                    match &edit {
                        // VRChat answers with the whole account: its status and its text, both as they now are.
                        ProfileEdit::Status(st) => {
                            me.status = v.get("status").and_then(Value::as_str).map(launcher_feed::status).unwrap_or(*st);
                            if let Some(t) = v.get("statusDescription").and_then(Value::as_str) {
                                me.status_text = t.to_string();
                            }
                        }
                        ProfileEdit::StatusText(t) => {
                            me.status_text = v.get("statusDescription").and_then(Value::as_str).map_or_else(|| t.clone(), str::to_string);
                            if let Some(st) = v.get("status").and_then(Value::as_str) {
                                me.status = launcher_feed::status(st);
                            }
                        }
                        ProfileEdit::Picture(url) => {
                            let icon = Some(https(&v, "userIcon")).filter(|u| !u.is_empty()).unwrap_or_else(|| url.clone());
                            me.look.icon = if icon.is_empty() { avatar } else { icon };
                        }
                        ProfileEdit::Banner { choice, colour, custom } => {
                            me.look.banner = match choice {
                                BannerChoice::Custom => custom.clone(),
                                BannerChoice::Avatar => avatar,
                                BannerChoice::Colour => String::new(),
                            };
                            me.look.banner_colour = if *choice == BannerChoice::Colour { *colour } else { None };
                        }
                        ProfileEdit::Bio(_) => {}
                    }
                }
                let mine = s.launcher.mine.get_or_insert_with(MyProfile::default);
                match &edit {
                    ProfileEdit::Bio(t) => mine.bio = v.get("bio").and_then(Value::as_str).map_or_else(|| t.clone(), str::to_string),
                    ProfileEdit::Picture(url) => mine.icon = url.clone(),
                    ProfileEdit::Banner { choice, colour, custom } => {
                        mine.banner = choice.clone();
                        if let Some(c) = colour {
                            mine.banner_colour = Some(*c);
                        }
                        if *choice == BannerChoice::Custom {
                            mine.banner_custom = custom.clone();
                        }
                    }
                    ProfileEdit::Status(_) | ProfileEdit::StatusText(_) => {}
                }
                mine.saving = Some(Saving::Saved(part));
                mine.answered += 1;
            }),
            Err(notice) => {
                let text = match notice {
                    Notice::Error(t) | Notice::Failed { text: t, .. } => t,
                    _ => tr("VRChat did not take the change. Try again in a moment.").to_string(),
                };
                self.launcher_mine_saving(Some(&account), Saving::Failed(part, text), None);
            }
        }
        None
    }

    /// The account's own icons or gallery pictures, to choose among (read on the person's press only).
    pub(super) fn launcher_list_pictures(&self, kind: PictureKind, account: Option<&str>) {
        let (client, account) = match (self.launcher_sender(account), account) {
            (Ok(client), Some(account)) => (client, account.to_string()),
            _ => return,
        };
        self.launcher_mine_publish(Some(&account), |mine| mine.listing = Some(kind));
        let tag = match kind {
            PictureKind::Icons => "icon",
            PictureKind::Gallery => "gallery",
        };
        let read = send_as(&client, &account, |c| c.own_files(tag));
        // Only onto the card of the account it was read for (the person may have switched meanwhile).
        self.launcher_mine_publish(Some(&account), |mine| {
            mine.listing = None;
            match read {
                Ok(files) => {
                    mine.list_failed = None;
                    let pictures: Vec<OwnPicture> = files.iter().filter_map(own_picture).collect();
                    match kind {
                        PictureKind::Icons => mine.icons = Some(pictures),
                        PictureKind::Gallery => mine.gallery = Some(pictures),
                    }
                }
                Err(notice) => {
                    // A list that did not come is no change that failed: said in its chooser, whose Try again reads
                    // the list again (never a profile change replayed).
                    let text = match notice {
                        Notice::Error(t) | Notice::Failed { text: t, .. } => t,
                        _ => tr("VRChat could not be reached. Try again shortly.").to_string(),
                    };
                    mine.list_failed = Some((kind, text));
                }
            }
        });
    }
}
