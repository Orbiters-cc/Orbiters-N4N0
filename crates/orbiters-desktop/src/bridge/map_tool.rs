//! The map tools: a world's finished 3D pictures as a plan for the next one (`map_plan`), a check before moving (`map_check`),
//! and the notes `aim_camera` and `walk` add from the same plan. The maps themselves are map.rs; here is where the pictures,
//! the camera and the avatar come from, and what an agent reads.
//!
//! The avatar's position is known exactly when a picture recorded it (`skeleton.bin`, the capture kit's band): from then on
//! the bridge adds up its own `walk`s and `turn`s, stopped where the plan shows a wall, and says how many it added.

use super::game::Failure;
use super::map::{self, Capture, Map, Marks};
use super::tools::Answer;
use super::{Bridge, Session};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const NAMES: [&str; 2] = ["map_plan", "map_check"];

/// The walking speed assumed before a walk measured one: VRChat's desktop default, measured by the tour (`speed_seen` 4).
const WALK_SPEED: f32 = 4.0;
/// Moves kept per session.
const MOVES_KEPT: usize = 400;
/// How far a picture of the plan is, at most, pixels.
const PICTURE_PX: u32 = 360;

/// A walk or turn of a session's avatar, as the bridge sent it.
#[derive(Clone, Debug)]
enum Move {
    /// Metres (0 when VRChat reported no movement), at an angle from where the avatar faced (0 forward, 180 back, -90 left).
    Walk { off: f32, metres: f32 },
    Turn { degrees: f32 },
}

/// A map built: its world, the pictures it was built of, and it.
type Built = (String, Vec<String>, Arc<Map>);
/// A session's world and its moves there: (unix seconds, move).
type Moves = (String, Vec<(u64, Move)>);

/// The maps and what they need, kept by the bridge.
#[derive(Default)]
pub struct Maps {
    /// Where each picture's share of its world's map is kept (`None`: in memory only).
    dir: Mutex<Option<PathBuf>>,
    captures: Mutex<HashMap<String, Arc<Capture>>>,
    /// The last map built: its world, the pictures it was built of, and it.
    built: Mutex<Option<Built>>,
    /// Each session's world and its moves there: (unix seconds, move).
    moves: Mutex<HashMap<String, Moves>>,
    /// The last walking speed VRChat reported per session, metres a second.
    speeds: Mutex<HashMap<String, f32>>,
}

impl Maps {
    /// Keeps each picture's share in `dir` from now on.
    pub fn keep_in(&self, dir: PathBuf) {
        *self.dir.lock().unwrap() = Some(dir);
    }

    fn file(&self, session: &str) -> Option<PathBuf> {
        self.dir.lock().unwrap().as_ref().map(|d| d.join(format!("{session}.json")))
    }

    /// A picture's share: kept, or read from its folder (and kept).
    fn capture(&self, sessions_dir: &std::path::Path, session: &str) -> Result<Arc<Capture>, String> {
        if let Some(c) = self.captures.lock().unwrap().get(session) {
            return Ok(c.clone());
        }
        if let Some(c) = self.file(session).and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<Capture>(&b).ok()) {
            let c = Arc::new(c);
            self.captures.lock().unwrap().insert(session.to_string(), c.clone());
            return Ok(c);
        }
        let dir = sessions_dir.join(session);
        if !dir.is_dir() {
            return Err("its files were deleted before a map was made of it".into());
        }
        let c = Arc::new(Capture::read(&dir)?);
        if let Some(file) = self.file(session) {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(bytes) = serde_json::to_vec(&*c) {
                if let Err(e) = orbiters_core::write_atomic(&file, &bytes) {
                    eprintln!("bridge map: a picture's map could not be kept: {e}");
                }
            }
        }
        self.captures.lock().unwrap().insert(session.to_string(), c.clone());
        Ok(c)
    }

    /// Takes a picture's share before its folder is deleted, so the world's map keeps it.
    pub fn keep_before_delete(&self, sessions_dir: &std::path::Path, session: &str) {
        let _ = self.capture(sessions_dir, session);
    }

    fn record(&self, session: &str, world: Option<&str>, m: Move) {
        let Some(world) = world else { return };
        let mut moves = self.moves.lock().unwrap();
        let entry = moves.entry(session.to_string()).or_insert_with(|| (world.to_ascii_lowercase(), Vec::new()));
        if !entry.0.eq_ignore_ascii_case(world) {
            *entry = (world.to_ascii_lowercase(), Vec::new());
        }
        entry.1.push((orbiters_core::visited::now(), m));
        let over = entry.1.len().saturating_sub(MOVES_KEPT);
        entry.1.drain(..over);
    }
}

/// Where the avatar is, as far as the bridge knows.
#[derive(Clone, Debug)]
pub struct Whereabouts {
    pub at: [f32; 2],
    pub heading: f32,
    /// The picture whose recording placed it.
    pub picture: String,
    pub walks: u32,
    pub turns: u32,
}

impl Whereabouts {
    fn json(&self) -> Value {
        let mut v = json!({ "at": [r1(self.at[0]), r1(self.at[1])], "heading": self.heading.round(), "as_of_picture": self.picture });
        if self.walks + self.turns > 0 {
            v["added_since"] = json!(format!(
                "{} walks and {} turns since that picture, added up by the bridge: about ±{:.0} m, ±{:.0}°",
                self.walks,
                self.turns,
                (self.walks as f32 * 0.8).max(0.5),
                (self.turns as f32 * 15.0).max(5.0)
            ));
        }
        v
    }
}

fn r1(v: f32) -> f64 {
    map::tenth(v)
}

pub fn list() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    vec![
        json!({
            "name": "map_plan",
            "title": "Plan the next picture from the map",
            "description": format!("Turns this world's finished 3D pictures (those this bridge took here whose reconstruction is done; they merge as the world gets more) into a plan seen from above, and answers where to go next: the space's size and a verdict on how many pictures it deserves (a very simple world: one), where the earlier pictures stood (their centres), VRChat's camera and the avatar on it, and up to three suggested viewpoints at least {apart} m from every earlier centre (when the world is that large), in open space clear of walls, each with why (another area, an opening into the unseen, a high point), its camera pose, the aim_camera calls that take the open camera there, and a walk plan (turn and walk steps through floor the pictures saw) when the avatar must go. With a picture of the plan (PNG). Unknown means no picture saw it: never take it for open. Frame: aim_camera's (metres, heading 0 along +z, 90 along +x).", apart = map::APART_M),
            "inputSchema": { "type": "object", "properties": {
                "session": { "type": "string" },
                "pictures": { "type": "array", "items": { "type": "string" }, "description": "Only these pictures (session names). Default: every picture of this world." },
                "world_id": { "type": "string", "description": "Another world this bridge pictured. Default: the one this session is in." },
                "image": { "type": "boolean", "description": "The picture of the plan. Default true." },
                "detail": { "type": "boolean", "description": "Also the openings, areas and what each picture added. Default false." },
            }, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "map_check",
            "title": "Check a spot on the map",
            "description": "Before moving: what this world's map (its finished 3D pictures) says of a camera pose or a walk. point [x, y, z]: free, near_solid, solid (inside a wall or an object), unknown (no picture saw it) or outside (the void, the sky, unpictured parts), with the ground below, the ceiling above, the nearest solid thing and whether it is behind something from where the pictures were taken. aim: the same for where aim_camera with these forward_m, right_m and up_m would put the open camera. walk {direction, seconds}: how far the plan shows the way clear from where the avatar is, and what stops it. walk_to [x, z]: a walk plan there through floor the pictures saw. Without any of them: the camera's spot and the avatar's. aim_camera and walk add the same note to their answers by themselves.",
            "inputSchema": { "type": "object", "properties": {
                "session": { "type": "string" },
                "point": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 },
                "aim": { "type": "object", "properties": {
                    "forward_m": { "type": "number" }, "right_m": { "type": "number" }, "up_m": { "type": "number" },
                }, "additionalProperties": false },
                "walk": { "type": "object", "properties": {
                    "direction": { "type": "string", "enum": ["forward", "backward", "left", "right"] },
                    "seconds": { "type": "number", "exclusiveMinimum": 0, "maximum": 10 },
                    "run": { "type": "boolean" },
                }, "required": ["direction", "seconds"], "additionalProperties": false },
                "walk_to": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
            }, "additionalProperties": false },
            "annotations": read_only,
        }),
    ]
}

/// The angle a walk direction makes with where the avatar faces.
fn off_of(direction: &str) -> f32 {
    match direction {
        "backward" => 180.0,
        "left" => -90.0,
        "right" => 90.0,
        _ => 0.0,
    }
}

const NO_MAP_NEXT: &str = "Take a 3D picture here (take_3d_picture); once its reconstruction is done (pictures says, usually within a minute or two), map_plan uses it.";

impl Bridge {
    pub(super) fn map_call(&self, name: &str, s: &Session, args: &Value) -> Result<Answer, Failure> {
        match name {
            "map_plan" => self.map_plan(s, args),
            _ => self.map_check(s, args),
        }
    }

    /// The world a call means: `world_id`, else the session's.
    fn map_world(&self, s: &Session, args: &Value) -> Result<String, Failure> {
        args["world_id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| s.game.seen().world_id)
            .map(|w| w.to_ascii_lowercase())
            .ok_or_else(|| Failure::new("no_world", "This session is in no world.", "go_to_world first, or pass world_id."))
    }

    /// The map of a world from its finished pictures (or `only` of them), built once for the same pictures. Also what was
    /// left out and why.
    fn world_map(&self, world: &str, only: Option<&[String]>) -> Result<(Arc<Map>, Vec<Value>), Failure> {
        let sessions_dir = self.publish.lock().unwrap().as_ref().map(|p| p.sessions_dir()).unwrap_or_default();
        let mut pictures: Vec<super::journal::Picture> =
            self.journal.lock().unwrap().pictures.iter().filter(|p| p.world_id.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(world))).cloned().collect();
        if let Some(only) = only {
            pictures.retain(|p| only.contains(&p.session));
        }
        pictures.sort_by_key(|p| p.at);
        let mut used = Vec::new();
        let mut left_out = Vec::new();
        for p in &pictures {
            let state = if sessions_dir.join(&p.session).is_dir() { self.reconstruction_of(&self.main, &sessions_dir.join(&p.session), &self.game.seen())["state"].as_str().unwrap_or("none").to_string() } else { "done".into() };
            if matches!(state.as_str(), "running" | "waiting" | "recording") {
                left_out.push(json!({ "picture": p.session, "why": format!("its reconstruction is {state}") }));
                continue;
            }
            match self.maps.capture(&sessions_dir, &p.session) {
                Ok(c) => used.push(c),
                Err(why) => left_out.push(json!({ "picture": p.session, "why": why })),
            }
        }
        if used.is_empty() {
            let message = if pictures.is_empty() { "This bridge has no 3D picture of this world yet.".to_string() } else { "None of this world's pictures has a finished reconstruction yet.".to_string() };
            return Err(Failure::new("no_map", message, NO_MAP_NEXT).retry_after(if pictures.is_empty() { 0 } else { 60 }));
        }
        // Pictures far from the newest are of another part of the world (a teleport): the plan is of where the newest is.
        let (used, elsewhere) = map::one_place(used);
        for c in &elsewhere {
            left_out.push(json!({ "picture": c.session, "why": format!("taken {:.0} m away, in another part of the world", r1(map::dist3_of(c.centre, used.last().map_or(c.centre, |u| u.centre)))) }));
        }
        let key: Vec<String> = used.iter().map(|c| c.session.clone()).collect();
        if let Some((w, k, m)) = &*self.maps.built.lock().unwrap() {
            if w == world && *k == key {
                return Ok((m.clone(), left_out));
            }
        }
        let map = Arc::new(Map::build(used));
        *self.maps.built.lock().unwrap() = Some((world.to_string(), key, map.clone()));
        Ok((map, left_out))
    }

    /// The map of the session's world when one can be had, for the notes of aim_camera and walk (never an error).
    fn session_map(&self, s: &Session) -> Option<Arc<Map>> {
        let world = s.game.seen().world_id?.to_ascii_lowercase();
        self.world_map(&world, None).ok().map(|m| m.0)
    }

    /// Where the avatar is: the newest picture that recorded it, plus this session's walks and turns since.
    fn whereabouts(&self, s: &Session, map: &Map) -> Option<Whereabouts> {
        let taken: HashMap<String, u64> = self.journal.lock().unwrap().pictures.iter().map(|p| (p.session.clone(), p.at)).collect();
        let anchor = map.captures.iter().filter(|c| c.avatar.is_some()).max_by_key(|c| taken.get(&c.session).copied().unwrap_or(c.taken))?;
        let since = taken.get(&anchor.session).copied().unwrap_or(anchor.taken);
        let avatar = anchor.avatar?;
        let mut w = Whereabouts { at: [avatar.hips[0], avatar.hips[2]], heading: avatar.heading, picture: anchor.session.clone(), walks: 0, turns: 0 };
        let world = s.game.seen().world_id;
        let moves = self.maps.moves.lock().unwrap();
        if let Some((mw, list)) = moves.get(&s.id) {
            if world.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(mw)) {
                for (_, m) in list.iter().filter(|(at, _)| *at >= since) {
                    match m {
                        Move::Turn { degrees } => {
                            w.heading = (w.heading + degrees).rem_euclid(360.0);
                            w.turns += 1;
                        }
                        Move::Walk { off, metres } => {
                            let h = w.heading + off;
                            let check = map.check_walk(w.at, h, *metres);
                            // A wall stops the avatar; past the plan's edge it went on as far as it walked.
                            let gone = if check.stops == Some("blocked") { (check.clear_m - 0.3).max(0.0) } else { *metres };
                            let f = map::forward(h);
                            w.at = [w.at[0] + f[0] * gone, w.at[1] + f[1] * gone];
                            w.walks += 1;
                        }
                    }
                }
            }
        }
        Some(w)
    }

    /// VRChat's camera, when it is open and has said where it is: (x, y, z, pitch, heading).
    fn camera_now(s: &Session) -> Option<[f32; 5]> {
        if s.game.camera_open() != Some(true) {
            return None;
        }
        let p = s.game.camera_pose()?;
        let pitch = if p[3] > 180.0 { p[3] - 360.0 } else { p[3] };
        Some([p[0], p[1], p[2], pitch, p[4].rem_euclid(360.0)])
    }

    fn speed_of(&self, s: &Session) -> f32 {
        self.maps.speeds.lock().unwrap().get(&s.id).copied().unwrap_or(WALK_SPEED)
    }

    fn map_plan(&self, s: &Session, args: &Value) -> Result<Answer, Failure> {
        let world = self.map_world(s, args)?;
        let only: Option<Vec<String>> = args["pictures"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
        let (map, left_out) = self.world_map(&world, only.as_deref())?;
        let here = s.game.seen().world_id.is_some_and(|w| w.eq_ignore_ascii_case(&world));
        let size = map.size();
        let verdict = map.verdict(&size);
        let suggestions = map.suggestions(&size);
        let camera = if here { Self::camera_now(s) } else { None };
        let avatar = if here { self.whereabouts(s, &map) } else { None };
        let speed = self.speed_of(s);
        let mut route = Vec::new();
        let suggested: Vec<Value> = suggestions
            .iter()
            .map(|g| {
                let mut v = json!({
                    "id": g.id.to_string(),
                    "why": g.why,
                    "pose": g.pose,
                    "from_centres_m": g.from_centres_m,
                });
                if let Some(c) = camera {
                    v["aim_camera"] = json!(map::aim_steps(c, g.pose, super::tools::AIM_REACH_M as f32 - 1.0));
                }
                if let Some(a) = &avatar {
                    match map.route(a.at, g.stand) {
                        Some(corners) => {
                            let steps = map::walk_steps(&corners, a.heading, speed);
                            if route.is_empty() {
                                route = corners.clone();
                            }
                            v["walk"] = json!(steps);
                            if g.kind == "high_point" {
                                v["walk_note"] = json!("Walk there, open the camera, then aim_camera up to the pose's height.");
                            }
                        }
                        None => v["walk"] = json!("No way the pictures saw: aim_camera can take the camera there through walls; the avatar would have to find a way."),
                    }
                }
                v
            })
            .collect();
        let centres: Vec<Value> = map.captures.iter().enumerate().map(|(k, c)| json!({ "n": k + 1, "picture": c.session, "at": c.centre.map(r1) })).collect();
        let mut answer = json!({
            "world": { "id": world, "name": map.captures.last().and_then(|c| c.world_name.clone()) },
            "pictures_used": map.captures.len(),
            "size": size,
            "verdict": verdict,
            "centres": centres,
            "suggestions": suggested,
            "limits": "Made from what the pictures' flights saw: plain or dark surfaces leave holes, glass and mirrors mislead, and unknown (black on the picture) may be open or solid. Check with a screenshot after each move.",
        });
        if let Some(c) = camera {
            let check = map.check_point([c[0], c[1], c[2]]);
            answer["camera"] = json!({ "pose": c.map(r1), "at": check.at });
        }
        answer["avatar"] = avatar.as_ref().map_or_else(|| json!(if here { "Not known: no picture of this world recorded it (the capture kit's band)." } else { "In another world." }), Whereabouts::json);
        if !left_out.is_empty() {
            answer["left_out"] = json!(left_out);
        }
        answer["next"] = json!(if verdict.simple && !map.captures.is_empty() {
            "The rule: a very simple world needs one picture, and it has one. Go on to the next world.".to_string()
        } else if map.captures.len() >= verdict.pictures as usize {
            "This world has its pictures; take more only for an area or an opening not yet pictured (suggestions).".to_string()
        } else if suggestions.is_empty() {
            "No spot in seen open space is far enough from the earlier centres: walk through an opening (black on the picture) and look, or use aim_camera with a screenshot.".to_string()
        } else {
            format!("Go to suggestion {}: aim_camera's calls (the camera open), or the walk then the camera; screenshot; take_3d_picture there. Then map_plan again once its reconstruction is done.", suggestions[0].id)
        });
        if args["detail"].as_bool().unwrap_or(false) {
            let openings: Vec<Value> = map
                .openings()
                .iter()
                .map(|o| json!({ "at": map.cell_centre(o[o.len() / 2]).map(r1), "width_m": r1(o.len() as f32 * map.cell) }))
                .collect();
            let (_, areas) = map.areas();
            answer["detail"] = json!({
                "openings": openings,
                "areas_m2": areas.iter().map(|n| (*n as f32 * map.cell * map.cell).round()).filter(|a| *a >= 6.0).collect::<Vec<_>>(),
                "floor_y": r1(map.floor),
                "voxel_m": map.voxel,
                "cell_m": map.cell,
                "pictures": map.captures.iter().map(|c| json!({ "picture": c.session, "centre": c.centre.map(r1), "avatar": c.avatar.map(|a| json!({ "hips": a.hips.map(r1), "heading": a.heading.round() })) })).collect::<Vec<_>>(),
                "legend": map::LEGEND,
                "walk_speed_assumed_m_s": speed,
            });
        } else {
            answer["legend"] = json!("Picture: grey floor, blue-grey open (floor unseen), red blocked, black unknown, yellow numbers earlier centres, blue letters suggestions, cyan the camera, pink the avatar; up is +z. detail: true says more.");
        }
        if args["image"].as_bool() == Some(false) {
            return Ok(Answer::Data(answer));
        }
        let marks = Marks {
            centres: map.captures.iter().map(|c| c.centre).collect(),
            camera: camera.map(|c| ([c[0], c[1], c[2]], c[4])),
            avatar: avatar.as_ref().map(|a| (a.at, a.heading)),
            suggestions,
            route,
        };
        let png = map.picture(&marks, PICTURE_PX);
        Ok(Answer::Image(answer, png, "image/png"))
    }

    fn map_check(&self, s: &Session, args: &Value) -> Result<Answer, Failure> {
        let world = self.map_world(s, args)?;
        let (map, _) = self.world_map(&world, None)?;
        let camera = Self::camera_now(s);
        let avatar = self.whereabouts(s, &map);
        let mut answer = json!({});
        if let Some(p) = args["point"].as_array() {
            let p: Vec<f32> = p.iter().filter_map(Value::as_f64).map(|v| v as f32).collect();
            if p.len() != 3 {
                return Err(Failure::new("bad_arguments", "point is [x, y, z].", "In aim_camera's frame, metres."));
            }
            answer["point"] = json!(map.check_point([p[0], p[1], p[2]]));
        }
        if args["aim"].is_object() {
            let Some(c) = camera else {
                return Err(Failure::new("camera_pose_unknown", "VRChat's camera is not open, or has not said where it is.", "Open the camera (see the guide), or pass point."));
            };
            let get = |k: &str| args["aim"][k].as_f64().unwrap_or(0.0) as f32;
            let f = map::forward(c[4]);
            let to = [c[0] + get("forward_m") * f[0] + get("right_m") * f[1], c[1] + get("up_m"), c[2] + get("forward_m") * f[1] - get("right_m") * f[0]];
            answer["aim"] = json!({ "to": to.map(r1), "check": map.check_point(to) });
        }
        if args["walk"].is_object() || args["walk_to"].is_array() {
            let Some(a) = &avatar else {
                return Err(Failure::new("avatar_unknown", "No picture of this world recorded where the avatar is.", "Take a 3D picture where it stands (its recording places it), then ask again."));
            };
            if args["walk"].is_object() {
                let seconds = args["walk"]["seconds"].as_f64().unwrap_or(0.0) as f32;
                let speed = self.speed_of(s) * if args["walk"]["run"].as_bool() == Some(true) { 1.5 } else { 1.0 };
                let h = a.heading + off_of(args["walk"]["direction"].as_str().unwrap_or("forward"));
                answer["walk"] = json!({ "heading": h.rem_euclid(360.0).round(), "metres_asked": r1(seconds * speed), "check": map.check_walk(a.at, h, seconds * speed) });
            }
            if let Some(t) = args["walk_to"].as_array() {
                let t: Vec<f32> = t.iter().filter_map(Value::as_f64).map(|v| v as f32).collect();
                if t.len() != 2 {
                    return Err(Failure::new("bad_arguments", "walk_to is [x, z].", "In aim_camera's frame, metres."));
                }
                answer["walk_to"] = match map.route(a.at, [t[0], t[1]]) {
                    Some(corners) => json!({ "steps": map::walk_steps(&corners, a.heading, self.speed_of(s)) }),
                    None => json!({ "steps": null, "why": "No way through floor the pictures saw (the target may be behind a wall or in unknown space)." }),
                };
            }
        }
        if answer.as_object().is_some_and(|o| o.is_empty()) {
            if let Some(c) = camera {
                answer["camera"] = json!({ "pose": c.map(r1), "check": map.check_point([c[0], c[1], c[2]]) });
            }
            if let Some(a) = &avatar {
                let ahead = map.check_walk(a.at, a.heading, 10.0);
                answer["avatar"] = json!({ "where": a.json(), "ahead_10_m": ahead });
            }
        }
        answer["avatar_known"] = json!(avatar.as_ref().map(Whereabouts::json));
        Ok(Answer::Data(answer))
    }

    /// aim_camera's note: what the map says of where the camera went (none without a map of this world).
    pub(super) fn map_note_for_camera(&self, s: &Session, to: [f32; 3]) -> Option<Value> {
        let map = self.session_map(s)?;
        let check = map.check_point(to);
        let warn = check.at != "free";
        let mut v = json!({ "at": check.at, "note": check.note });
        if let Some(d) = check.ground_below_m {
            v["ground_below_m"] = json!(d);
        }
        if check.behind_something {
            v["behind_something"] = json!(true);
        }
        if warn {
            v["warning"] = json!("The map does not show this spot as open: look with a screenshot before a picture (map_check says more).");
        }
        Some(v)
    }

    /// walk's note before it goes: how far the plan shows the way clear (none without a map or a known avatar).
    pub(super) fn map_note_for_walk(&self, s: &Session, direction: &str, seconds: f32, run: bool) -> Option<Value> {
        let map = self.session_map(s)?;
        let a = self.whereabouts(s, &map)?;
        let metres = seconds * self.speed_of(s) * if run { 1.5 } else { 1.0 };
        let check = map.check_walk(a.at, a.heading + off_of(direction), metres);
        let mut v = json!({ "clear_m": check.clear_m, "of_m": r1(metres) });
        if let Some(stop) = check.stops {
            v["then"] = json!(match stop {
                "blocked" => "a wall or something standing (the avatar stops there)",
                "unknown" => "space no picture saw",
                _ => "beyond everything pictured",
            });
        }
        Some(v)
    }

    /// Notes a walk the bridge sent (for the avatar's whereabouts).
    pub(super) fn map_walked(&self, s: &Session, direction: &str, held_s: f64, speed_seen: Option<f32>, run: bool) {
        let seen = s.game.seen();
        if let Some(v) = speed_seen.filter(|v| *v > 0.5 && !run) {
            self.maps.speeds.lock().unwrap().insert(s.id.clone(), v);
        }
        let metres = match speed_seen {
            Some(v) if v < 0.1 => 0.0,
            Some(v) => v * held_s as f32,
            None => self.speed_of(s) * held_s as f32 * if run { 1.5 } else { 1.0 },
        };
        self.maps.record(&s.id, seen.world_id.as_deref(), Move::Walk { off: off_of(direction), metres });
    }

    /// Notes a turn the bridge sent. On the desktop none while the camera or a menu is open (then it turns nothing); a VR
    /// session's turns are counted always (whether VRChat in VR turns the avatar with the camera open is not measured). VR sessions are otherwise the same to the map: `/usercamera/Pose`,
    /// the recording's poses and its skeleton band are the same world frame there.
    pub(super) fn map_turned(&self, s: &Session, degrees: f64) {
        if s.vr().is_none() && s.game.camera_open() == Some(true) {
            return;
        }
        self.maps.record(&s.id, s.game.seen().world_id.as_deref(), Move::Turn { degrees: degrees as f32 });
    }

    /// Keeps a picture's share of its world's map before its files go.
    pub(super) fn map_keep(&self, sessions_dir: &std::path::Path, session: &str) {
        self.maps.keep_before_delete(sessions_dir, session);
    }

    /// Keeps the maps' pictures in `dir` from now on.
    pub fn keep_maps(&self, dir: PathBuf) {
        self.maps.keep_in(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SWEEP-19 a: every picture's share stayed in the service's memory for the whole session (a long tour: hundreds of MB).
    /// Only the last ones stay; any other comes back from its file when a map needs it.
    #[test]
    fn the_pictures_kept_in_memory_are_bounded_and_the_others_come_back_from_their_files() {
        let dir = std::env::temp_dir().join(format!("orbiters-map-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let maps = Maps::default();
        maps.keep_in(dir.clone());
        for k in 0..200 {
            let c = Capture { session: format!("s{k}"), centre: [k as f32, 0.0, 0.0], path: vec![[k as f32, 0.0, 0.0]], voxels: vec![[k, 0, 0, 2]; 50], ..Default::default() };
            std::fs::write(dir.join(format!("s{k}.json")), serde_json::to_vec(&c).unwrap()).unwrap();
        }
        for k in 0..200 {
            assert_eq!(maps.capture(&dir, &format!("s{k}")).unwrap().centre[0], k as f32);
        }
        let kept = maps.captures.lock().unwrap().len();
        assert!(kept <= 64, "{kept} pictures kept in memory");
        // The first, long gone from memory, is read again from its file.
        assert_eq!(maps.capture(&dir, "s0").unwrap().voxels.len(), 50);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
