//! A world tour's tools beyond VRChat itself: which Orbiters environment the app talks to (and switching it), publishing a
//! session's 3D picture to it the way Photos & captures' Share publicly does, the pictures the bridge took (kept across
//! restarts, journal.rs) and where each stands, and freeing their files on this computer once they are safe on Orbiters.

use super::game::{EnvironmentSeen, Failure, Publish, PublishSeen};
use super::journal;
use super::person::Paused;
use super::tools::{paused_failure, Answer};
use super::{Bridge, Call};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const NAMES: [&str; 4] = ["orbiters_environment", "publish_3d_picture", "pictures", "free_picture_files"];

/// What a switch of the environment changes, for the agent to say and the person to know.
pub const SWITCH_CHANGES: &str = "This is Orbiters - N4N0's own Development environment switch (Settings), for the whole app and for the person too, until it is switched back: the Orbiters account it uses (each environment has its own saved sign-in), where world labels, likes, comments and 3D picture shares go from then on, the update feed it follows (in production it does not see development builds), and the website its links open. Nothing already sent moves. The person sees it in Settings, and the bridge's log in Settings says who switched it.";

/// What one picture takes on disk, measured on this computer (2026-11-15): 4 s pictures 118 to 251 MB on arrival (74 to 185 MB
/// of it keyframes), 8 s ones about twice that; reconstructed and reduced to their 3D view 40 to 200 MB.
const ONE_PICTURE_GB: f64 = 0.5;
/// Below this much free space on a drive the pictures use, get_state says to free files.
const LOW_GB: f64 = 8.0;

pub fn reads(name: &str) -> bool {
    name == "pictures"
}

pub fn list() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    let acts = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": true });
    let deletes = json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    vec![
        json!({
            "name": "orbiters_environment",
            "title": "Orbiters environment",
            "description": format!("Reads (action read) or switches (action switch, with to) the Orbiters environment Orbiters - N4N0 talks to: production (orbiters.cc, the real site everyone sees) or development (the server on this computer). It decides where label_world's labels and publish_3d_picture's pictures go, and under which account: get_state's `orbiters` says it at any time. {SWITCH_CHANGES} Switch only when the person asked for that environment, say so in your report, and switch back only if they asked you to. Refused while labels are still waiting to be sent (they would go to the other one) or a picture is being published."),
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string", "enum": ["read", "switch"] },
                "to": { "type": "string", "enum": ["production", "development"], "description": "For switch." },
            }, "required": ["action"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "publish_3d_picture",
            "title": "Publish a 3D picture",
            "description": "Publishes a 3D picture this bridge took (its `session` from take_3d_picture or pictures) to Orbiters, in the environment get_state's `orbiters` names, under that account, attached to the world it was taken in: exactly Photos & captures' Share publicly (visibility public), or an upload kept on the account and listed nowhere (private: only the account sees it, in its gallery). Pictures are published one at a time in the order asked; when Orbiters asks to wait (it allows only so many uploads an hour), the answer says `rate_limited` with `retry_after_s` and the picture stays queued and goes by itself. Its reconstruction must be done. It is published as it is: other people see a public one on the world's page, orbiters.cc's Map explorer and N4N0's Worlds in 3D. Orbiters lists one public 3D picture per account and world: a newer public one takes the older one's place in the public listing (the older stays on Orbiters, the account's own). So publish a world's other pictures private and the best one public, last. Waits up to wait_s and answers the item's id and link, or where it stands (pictures reads it later). Publish only pictures that show the world well: a black, blurred or empty one is not worth publishing.",
            "inputSchema": { "type": "object", "properties": {
                "picture": { "type": "string", "description": "The picture's session name." },
                "visibility": { "type": "string", "enum": ["public", "private"], "description": "Default public." },
                "wait_s": { "type": "number", "minimum": 0, "maximum": 50, "description": "Default 45." },
            }, "required": ["picture"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "pictures",
            "title": "List the pictures taken",
            "description": "The 3D pictures this bridge's sessions took, newest first, kept across restarts of the app (and those taken before the app kept them, found in the sessions folder by their instance: a strict private instance of a side session's own VRChat account, never the person's): each one's world, length, size on this computer, reconstruction (waiting, running, done, failed) and publishing (none, running, done with id and link, failed) in the active environment. With picture: that one. unpublished: only those not published yet.",
            "inputSchema": { "type": "object", "properties": {
                "picture": { "type": "string" },
                "unpublished": { "type": "boolean" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Default 30." },
            }, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "free_picture_files",
            "title": "Free a picture's files",
            "description": "Deletes a picture's files on this computer for good, to keep room for the next ones (get_state's `disks`). what capture_files: what only its reconstruction read (keyframes, depth frames, leftover runs: about 190 of an 8 s picture's 230 MB), once its reconstruction is done; its 3D view and everything publishing reads stay, so it can still be published and seen here, but not reconstructed again. what everything: the whole picture, only once it is published (publish_3d_picture done in the active environment), or when its reconstruction failed or it has no keyframes (nothing to publish). Refused while it records, waits for or runs its reconstruction, or is being published.",
            "inputSchema": { "type": "object", "properties": {
                "picture": { "type": "string" },
                "what": { "type": "string", "enum": ["capture_files", "everything"] },
            }, "required": ["picture", "what"], "additionalProperties": false },
            "annotations": deletes,
        }),
    ]
}

/// The environment as the tools answer it.
pub fn environment_json(env: &EnvironmentSeen) -> Value {
    json!({
        "environment": if env.development { "development" } else { "production" },
        "server": env.server,
        "website": env.website,
        "signed_in": env.account.is_some(),
        "account": env.account.as_ref().map(|(name, id)| json!({ "name": name, "id": id })),
        "update_feed": if env.development { "development (the server on this computer)" } else { "production (orbiters.cc)" },
        "other_environment_signed_in": env.other_signed_in,
        "labels_waiting": env.labels_pending,
    })
}

fn publish_json(seen: &PublishSeen) -> Value {
    serde_json::to_value(seen).unwrap_or(Value::Null)
}

fn megabytes(bytes: u64) -> f64 {
    (bytes as f64 / 1e5).round() / 10.0
}

impl Bridge {
    fn publish_seam(&self) -> Result<Arc<dyn Publish>, Failure> {
        self.publish.lock().unwrap().clone().ok_or_else(|| Failure::new("publishing_unavailable", "Publishing cannot be reached from this bridge.", "Go on without it and tell the person."))
    }

    pub(super) fn tour_call(&self, name: &str, args: &Value, call: &Call) -> Result<Answer, Failure> {
        if name != "pictures" && !(name == "orbiters_environment" && args["action"] == "read") {
            if let Some(Paused::Held) = self.paused() {
                return Err(paused_failure(&Paused::Held));
            }
        }
        match name {
            "orbiters_environment" => self.orbiters_environment(args, call),
            "publish_3d_picture" => self.publish_3d_picture(args, call),
            "pictures" => self.pictures(args),
            _ => self.free_picture_files(args),
        }
    }

    /// What get_state says of Orbiters and the disks.
    pub(super) fn orbiters_state(&self) -> (Value, Value, Option<String>) {
        let Ok(publish) = self.publish_seam() else { return (Value::Null, Value::Null, None) };
        let env = publish.environment();
        let disks = publish.disks();
        let low = disks.iter().filter(|d| d.free_gb < LOW_GB).map(|d| format!("{} has {:.1} GB free", d.drive, d.free_gb)).collect::<Vec<_>>();
        let warning = (!low.is_empty()).then(|| {
            format!(
                "{}: free the files of pictures already published (free_picture_files everything) or reconstructed (capture_files) before taking more; a picture takes up to {ONE_PICTURE_GB} GB until then.",
                low.join(", ")
            )
        });
        (environment_json(&env), serde_json::to_value(&disks).unwrap_or_default(), warning)
    }

    /// A picture's publishing, for get_state's pictures (none without the seam).
    pub(super) fn publish_of(&self, session: &std::path::Path) -> Value {
        self.publish_seam().map(|p| publish_json(&p.published(session))).unwrap_or(Value::Null)
    }

    fn orbiters_environment(&self, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let publish = self.publish_seam()?;
        let before = publish.environment();
        let action = args["action"].as_str().unwrap_or("read");
        if action == "read" {
            return Ok(Answer::Data(json!({ "orbiters": environment_json(&before), "switching_changes": SWITCH_CHANGES })));
        }
        let development = match args["to"].as_str() {
            Some("production") => false,
            Some("development") => true,
            _ => return Err(Failure::new("bad_arguments", "switch needs to: production or development.", "Pass to.")),
        };
        if development == before.development {
            return Ok(Answer::Data(json!({ "switched": false, "orbiters": environment_json(&before), "next": "It is that environment already: nothing changed." })));
        }
        if before.labels_pending > 0 {
            return Err(Failure::new(
                "labels_waiting",
                format!("{} world label changes are still waiting to be sent: switching now would send them to the other environment.", before.labels_pending),
                "Wait until world_labels says pending 0 (Orbiters - N4N0 sends them by itself), then switch.",
            )
            .retry_after(30));
        }
        let journal: Vec<String> = self.journal.lock().unwrap().pictures.iter().rev().take(200).map(|p| p.session.clone()).collect();
        let dir = publish.sessions_dir();
        if journal.iter().any(|s| matches!(publish.published(&dir.join(s)), PublishSeen::Running { .. } | PublishSeen::RateLimited { .. })) {
            return Err(Failure::new("publish_under_way", "A picture is being published to the active environment.", "Wait for it (pictures), then switch.").retry_after(30));
        }
        publish.set_environment(development)?;
        // The account of the other environment is read within half a second.
        let after_wanted = |e: &EnvironmentSeen| e.development == development && (e.account.is_some() || !e.other_signed_in);
        self.wait_until(Duration::from_secs(5), call, || after_wanted(&publish.environment()))?;
        let after = publish.environment();
        let to = if development { "development" } else { "production" };
        let by = self.client_seen().map_or_else(|| "an MCP client".to_string(), |c| c.name);
        self.log(true, format!("Orbiters environment switched to {to} by {by}")); // i18n: not shown (the bridge's log, as its other lines)
        Ok(Answer::Data(json!({
            "switched": true,
            "before": environment_json(&before),
            "orbiters": environment_json(&after),
            "what_changed": SWITCH_CHANGES,
            "next": if after.account.is_none() {
                format!("No Orbiters account is saved for {to}: labels and publishing are refused until the person signs in there (Orbiters - N4N0's Settings, Orbiters account, Sign in). Tell them.")
            } else {
                format!("Labels and pictures go to {to} now, under {}. Say in your report that you switched it; switch back only if the person asked you to.", after.account.as_ref().and_then(|a| a.0.clone()).unwrap_or_else(|| "its account".into()))
            },
        })))
    }

    /// Takes in the pictures bridge sessions took before the journal knew them (journal.rs `adopt`).
    fn adopt(&self, publish: &dyn Publish) -> usize {
        let persons = self.game.seen().vrchat_user.map(|u| u.0);
        let dir = publish.sessions_dir();
        if dir.as_os_str().is_empty() {
            return 0;
        }
        self.journal.lock().unwrap().adopt(&dir, persons.as_deref())
    }

    /// The picture a call names: one this bridge took, its folder, and its journal entry.
    fn picture_named(&self, publish: &dyn Publish, args: &Value) -> Result<(PathBuf, journal::Picture), Failure> {
        let name = args["picture"].as_str().unwrap_or_default();
        if self.journal.lock().unwrap().picture(name).is_none() {
            self.adopt(publish);
        }
        let Some(picture) = self.journal.lock().unwrap().picture(name).cloned() else {
            return Err(Failure::new(
                "unknown_picture",
                format!("{name:?} is not a picture this bridge took."),
                "Pass a session name from take_3d_picture's answer, or from pictures.",
            ));
        };
        Ok((publish.sessions_dir().join(&picture.session), picture))
    }

    fn picture_entry(&self, publish: &dyn Publish, picture: &journal::Picture) -> Value {
        let dir = publish.sessions_dir().join(&picture.session);
        let here = dir.is_dir();
        let seen = self.game.seen();
        json!({
            "session": picture.session,
            "world_id": picture.world_id,
            "world_name": picture.world_name,
            "length_s": picture.length_s,
            "taken_at": picture.at,
            "on_disk_mb": if here { megabytes(publish.size(&dir)) } else { 0.0 },
            "freed": picture.freed,
            "reconstruction": if here { self.reconstruction_of(&self.main, &dir, &seen) } else { json!({ "state": "deleted" }) },
            "publish": publish_json(&publish.published(&dir)),
        })
    }

    fn pictures(&self, args: &Value) -> Result<Answer, Failure> {
        let publish = self.publish_seam()?;
        if args["picture"].is_string() {
            let (_, picture) = self.picture_named(&*publish, args)?;
            return Ok(Answer::Data(self.picture_entry(&*publish, &picture)));
        }
        let adopted = self.adopt(&*publish);
        let limit = args["limit"].as_u64().unwrap_or(30).clamp(1, 200) as usize;
        let unpublished = args["unpublished"].as_bool().unwrap_or(false);
        let all: Vec<journal::Picture> = self.journal.lock().unwrap().pictures.iter().rev().cloned().collect();
        let dir = publish.sessions_dir();
        let listed: Vec<Value> = all
            .iter()
            .filter(|p| !unpublished || !matches!(publish.published(&dir.join(&p.session)), PublishSeen::Done { .. }))
            .take(limit)
            .map(|p| self.picture_entry(&*publish, p))
            .collect();
        Ok(Answer::Data(json!({
            "pictures": listed,
            "taken_in_all": all.len(),
            "found_on_disk_now": adopted,
            "orbiters": environment_json(&publish.environment()),
            "disks": publish.disks(),
        })))
    }

    fn publish_3d_picture(&self, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let publish = self.publish_seam()?;
        let (dir, picture) = self.picture_named(&*publish, args)?;
        let public = match args["visibility"].as_str().unwrap_or("public") {
            "public" => true,
            "private" => false,
            other => return Err(Failure::new("bad_arguments", format!("visibility {other} is not one."), "Pass public or private.")),
        };
        if !dir.is_dir() {
            return Err(Failure::new("picture_deleted", "This picture's files were deleted from this computer.", "Nothing left to publish: go on."));
        }
        let env = publish.environment();
        if env.account.is_none() {
            return Err(Failure::new(
                "orbiters_not_signed_in",
                format!("No Orbiters account is saved for {} in Orbiters - N4N0, so nothing was sent.", if env.development { "development" } else { "production" }),
                "Tell the person to sign in to Orbiters in Orbiters - N4N0's Settings (or orbiters_environment says which environment has one).",
            ));
        }
        let world_id = picture.world_id.clone().filter(|w| orbiters_core::vrchat_log::is_world_id(w)).ok_or_else(|| {
            Failure::new("no_world", "This picture's world is not known, so it cannot be attached to one.", "Leave it unpublished.")
        })?;
        let state = self.reconstruction_of(&self.main, &dir, &self.game.seen());
        if state["state"] != "done" {
            return Err(Failure::new(
                "not_reconstructed",
                format!("Its reconstruction is {}: only a finished 3D picture can be published.", state["state"].as_str().unwrap_or("not there")),
                "Publish it later: pictures (or get_state) says when its reconstruction is done. A failed one cannot be published.",
            )
            .retry_after(60));
        }
        let wanted = |seen: &PublishSeen| matches!(seen, PublishSeen::Done { public: p, .. } if *p || !public);
        let now = publish.published(&dir);
        if !matches!(now, PublishSeen::Running { .. } | PublishSeen::RateLimited { .. }) && !wanted(&now) {
            publish.publish(&dir, &world_id, picture.world_name.as_deref().unwrap_or_default(), public)?;
            self.log(true, format!("publish {} ({})", picture.session, if public { "public" } else { "private" })); // i18n: not shown (the bridge's log)
        }
        let wait = Duration::from_secs_f64(args["wait_s"].as_f64().unwrap_or(45.0).clamp(0.0, 50.0));
        self.wait_until(wait, call, || !matches!(publish.published(&dir), PublishSeen::Running { .. }))?;
        let seen = publish.published(&dir);
        let retry_after_s = match &seen {
            PublishSeen::RateLimited { retry_after_s, .. } => Some(*retry_after_s),
            _ => None,
        };
        let next = match &seen {
            PublishSeen::RateLimited { .. } => "Orbiters asked to wait before more uploads (it allows only so many an hour): this picture is queued and goes by itself after retry_after_s, with the others in the order asked. Do not ask again: go on touring and taking pictures, and queue more with publish_3d_picture; pictures says when each is done.",
            PublishSeen::Done { .. } => "Published. Its files here can go now: free_picture_files with what everything, when you no longer need it here.",
            PublishSeen::Running { .. } => "Still under way: call pictures with this picture in a minute.",
            PublishSeen::Failed { explicit: true, .. } => "Orbiters asks for a confirmation that only the person gives for explicit content in public: publish it private, and tell the person.",
            PublishSeen::Failed { .. } => "Not published. Try once more later; if it fails again, leave it and tell the person with its message.",
            PublishSeen::None => "It did not start: try again.",
        };
        let mut answer = json!({
            "picture": picture.session,
            "world_id": world_id,
            "orbiters": environment_json(&env),
            "publish": publish_json(&seen),
            "next": next,
        });
        if let Some(wait) = retry_after_s {
            answer["code"] = json!("rate_limited");
            answer["retry_after_s"] = json!(wait);
        }
        Ok(Answer::Data(answer))
    }

    fn free_picture_files(&self, args: &Value) -> Result<Answer, Failure> {
        let publish = self.publish_seam()?;
        let (dir, picture) = self.picture_named(&*publish, args)?;
        let everything = match args["what"].as_str() {
            Some("capture_files") => false,
            Some("everything") => true,
            _ => return Err(Failure::new("bad_arguments", "what is capture_files or everything.", "Pass one.")),
        };
        if !dir.is_dir() {
            return Ok(Answer::Data(json!({ "freed_mb": 0.0, "next": "Its files are gone already.", "disks": publish.disks() })));
        }
        for s in self.sessions() {
            let capture = s.game.seen().capture;
            if capture.session.as_deref() == Some(dir.as_path()) && !matches!(capture.state.as_str(), "idle" | "") {
                return Err(Failure::new("picture_under_way", "This picture is still recording.", "Wait for it: wait_for with until picture_saved."));
            }
        }
        let reconstruction = self.reconstruction_of(&self.main, &dir, &self.game.seen());
        let state = reconstruction["state"].as_str().unwrap_or("none");
        if matches!(state, "running" | "waiting") {
            return Err(Failure::new("reconstruction_waiting", format!("Its reconstruction is {state}: it needs these files."), "Free it once its reconstruction is done.").retry_after(60));
        }
        let published = publish.published(&dir);
        if matches!(published, PublishSeen::Running { .. } | PublishSeen::RateLimited { .. }) {
            return Err(Failure::new("publish_under_way", "It is being published.", "Wait for it (pictures), then free it.").retry_after(30));
        }
        let files = self.game.picture_files(&dir);
        let nothing_to_publish = state == "failed" || (files.frames > 0 && files.keyframes == 0);
        if !everything && state != "done" {
            return Err(Failure::new(
                "not_reconstructed",
                "Its 3D view is not made (its reconstruction is not done), so its recording is all it has.",
                if nothing_to_publish { "Its reconstruction failed: what everything deletes it whole." } else { "Free it once its reconstruction is done." },
            ));
        }
        if everything && !nothing_to_publish && !matches!(published, PublishSeen::Done { .. }) {
            return Err(Failure::new(
                "not_published",
                "It is not published in the active environment: deleting it whole would lose it.",
                "publish_3d_picture first (or what capture_files, which keeps its 3D view).",
            ));
        }
        if everything {
            // The world's map keeps what this picture saw (map_tool.rs).
            self.map_keep(&publish.sessions_dir(), &picture.session);
        }
        let freed = publish.delete(&dir, everything)?;
        let what = if everything { "everything" } else { "capture_files" };
        self.journal.lock().unwrap().freed(&picture.session, what);
        Ok(Answer::Data(json!({
            "picture": picture.session,
            "freed": what,
            "freed_mb": megabytes(freed),
            "disks": publish.disks(),
            "next": "Done.",
        })))
    }
}
