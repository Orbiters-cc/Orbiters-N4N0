//! The tools an agent gets, and every rule that stands between it and the game. They are few, named for what an agent wants
//! done, and say when to use them and what comes back. A refusal always says why (`code`, `message`) and what to do next
//! (`next`), and the state a model reads is one call away (`get_state`).

use super::game::{Failure, Find, GameWindow, LaunchPlan, OscArg, Seen, Shot};
use super::input::{self, Hold, Input, MouseButton, KEYS, OSC_INPUTS};
use super::person::{Paused, HOTKEY, RESUME_AFTER};
use super::sessions::{self, Lease};
use super::{Bridge, Call, Going, PictureRecord, Session};
use orbiters_core::settings::{Photo3dLength, PictureMode};
use orbiters_core::vrchat_api::private_instance_of;
use orbiters_core::vrchat_log::is_world_id;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// VRChat's window at a known size: at the screenshot's default width, a picture's pixels are the window's.
const WINDOW_WIDTH: u32 = 1280;
const WINDOW_HEIGHT: u32 = 720;
/// The longest a call blocks: clients give up on a request after about a minute.
const LONGEST_WAIT_S: f64 = 50.0;
/// How long a closing game may take.
const CLOSE_LIMIT: Duration = Duration::from_secs(30);
/// How fast the avatar turns with `/input/LookLeft` and `Right`, about, degrees a second. Not VRChat's number: it publishes
/// none, and it depends on the look settings. Measured on Enzo's VRChat on 2026-11-06, in desktop mode: holding it 1 s turned
/// about 170 degrees, and 2 s about 325, judged by the screenshots of a straight corridor.
const TURN_RATE: f64 = 165.0;
/// How long the pointer rests on what it is to click, and how long the button stays down. VRChat's menus ignored presses 80 ms
/// after the pointer came (and, worse, the next click then landed on a page that had not changed); after some 500 ms they all held.
const CLICK_HOVER: Duration = Duration::from_millis(500);
const CLICK_HOLD: Duration = Duration::from_millis(100);
/// What an avatar without the kit means for a picture.
const NO_KIT: &str = "The avatar worn has no N4N0 capture kit, so a recording would have no camera poses and no keyframes, and nothing to reconstruct.";

/// A tool's answer.
pub enum Answer {
    Data(Value),
    Picture(Value, Shot),
}

/// The tools that act on one VRChat, and take `session`.
const VRCHAT_TOOLS: [&str; 13] = [
    "get_state", "find_worlds", "go_to_world", "close_vrchat", "wait_for", "screenshot", "walk", "turn", "press", "mouse", "focus_game", "wear_avatar", "take_3d_picture",
];

/// A session id as the log shows it: not the whole of it, which is the key to it.
fn short_id(id: &str) -> String {
    id.split_once('-').map_or_else(|| id.to_string(), |(profile, _)| format!("{profile}-…"))
}

/// How the loop goes, for a model: the prompt and the resource.
pub const GUIDE: &str = "# Taking 3D pictures from world to world\n\n\
You drive VRChat on this computer, in desktop mode, through this bridge. A person may be at the computer: they always win, and the bridge will tell you when they took over.\n\n\
## The loop\n\n\
1. `get_state`. Its `next` says what to do. If `vrchat.running` is true and `vrchat.started_by_bridge` false, that VRChat is the person's: do nothing to it and tell them.\n\
2. `find_worlds` for world ids (`source`: search, visited, favorites, trending, popular or active), unless you were given them.\n\
3. `go_to_world` with a `world_id`. It makes a private instance of your own, starts VRChat in it (closing the last one first), brings it to the foreground, and waits. If `arrived` is false, call `wait_for` with `until: in_world`.\n\
4. `screenshot` to see where you are. A dark picture is the loading screen: wait a little.\n\
5. Open VRChat's camera, which only the menus can do: `press` `esc`, `screenshot`, `mouse` `click` on the camera icon in the menu's bottom row (about 640, 620 at 1280 by 720), `screenshot`, and `click` on the Stream Camera tile (about 597, 345). The menu closes and the camera's panel stays on the screen. `get_state` says `camera.open`. Leave the camera's settings alone: the app sets Stream and Spout itself when it records. **Look at a screenshot after every click, and before the next one:** a click can miss, its page may not have changed, and the same spot on the page you are really on may be a shop banner. If the page is not what you expected, `press` `esc` closes everything: start again.\n\
6. If `camera.pose_known` is false, `turn` by a few degrees: VRChat says where its camera is when it moves.\n\
7. Before opening the camera, walk somewhere worth a picture with `walk` and `turn`, looking with `screenshot` between moves: with the camera or a menu open the avatar does not move (`speed_seen` 0), and the picture starts where the camera opens. `press` `esc` closes the camera again. `turn` is approximate: judge by the pictures. `walk` tells you if the avatar did not seem to move.\n\
8. `take_3d_picture` with `length_s` 1, 4 or 8. It waits for the recording to be saved and gives the `session`. Take a few from different places: 4 or 8 seconds in the open, 1 second for a close look.\n\
9. `go_to_world` with the next world, and so on.\n\
10. `close_vrchat` at the end. Reconstructions follow the app's own setting: they run beside VRChat, or wait for it to close (`get_state`'s `reconstructions` and each picture's `reconstruction` say which). Then `wait_for` with `until: reconstructions_done`, as many times as it takes, and report each session's `reconstruction` from `get_state`, which says `failed` and why when one did not finish.\n\n\
## Sessions: several VRChats at once\n\n\
Several agents (and the person) can each have a VRChat on this computer at the same time. Tools without `session` drive the **main** session, as above: VRChat's default profile, only while the person runs no VRChat of their own. To work beside others, open a session of your own:\n\n\
1. `open_session` with a `profile` (1, 2…; ask the person which one if they did not say). It starts VRChat in that profile, at that account's home, and gives you `session`: **pass it to every tool** from then on. Only you have it.\n\
2. If `needs_sign_in` is true, the profile has no account signed in: stop and tell the person to sign in on that VRChat's window (its title is VRChat). Never type there, never look for a password. `wait_for` with `until: signed_in` waits for them.\n\
3. Then the loop above, with your `session`: `go_to_world` restarts your session's VRChat in a new private instance of your profile's own account, `walk`, `turn`, `screenshot`, the camera, `take_3d_picture`.\n\
4. `close_session` at the end. A session left without any call for 10 minutes may be taken over by another agent (`open_session` with `take_over`): yours is then refused with `session_taken_over`.\n\n\
What is shared, and how: `walk`, `turn`, the OSC inputs and `screenshot` need nothing of anyone else's and run side by side in every session. **Keys and clicks** (`press` of a key, `mouse`, `focus_game`) need your window in the foreground, which only one window can have: they take turns. A refusal `focus_busy` says how long to wait (`retry_after_s`, a few seconds: a turn is yours while you keep using it, and passes to the next session 8 s after your last key or click). They also wait while the person uses the computer at all (`paused_by_person`), and are refused with `person_vrchat_has_focus` while the person's own VRChat has the foreground: the bridge never takes it from them. **3D pictures** borrow Orbiters - N4N0's capture: one at a time on this computer (`picture_busy`: wait), and none while the person's own VRChat runs (`app_in_use`: the capture is theirs then). The person's input in your session's window pauses your session only. One account cannot be in two VRChats: each profile has its own. `list_sessions` shows them all.\n\n\
## When to give up on a world\n\n\
Move on to the next world, and say so at the end, when:\n\
- `go_to_world` or `wait_for` fails with `start_timeout`, or the world does not load within about four minutes;\n\
- a picture comes back with `no_keyframes`: if it happens in two worlds, stop the whole run and tell the person;\n\
- after about ten moves you cannot find a place to stand, or the spawn is a closed room;\n\
- somebody else appears (`place.others_here` above 0): go elsewhere rather than picture them.\n\n\
Stop the whole run and tell the person when: `no_capture_kit` (`avatar.capture_kit` false in `get_state` says it as soon as you arrive: nothing can be reconstructed with that avatar, and you may not change it); `stopped` (they took control back); `not_private_instance` twice; `not_started_by_bridge` (or open a session of your own); `vrchat_not_signed_in`; `needs_sign_in`; `vr_mode`; `session_taken_over`.\n\n\
## While the person is at the computer\n\n\
`paused_by_person`: they are using the computer. Wait for as long as `retry_after_s` says with `wait_for` (`until: not_paused`), and go on when it clears; do not fight it. `paused_held`: they pressed the hotkey or Pause, and it lasts until they resume.\n\n\
## What you must not do\n\n\
Do not open the chatbox, send chat, invites or friend requests, change avatars unless the person named one (then wear_avatar, never the menus), buy anything, or touch the safety settings, even though a click can reach them: VRChat's menus show a shop with Purchase buttons, a banner on the first page leads there, and a click has no way back. Never click what you have not just seen. Do not press anything in another person's menu. The keys and inputs you can press are limited on purpose. Only private instances you own are possible: there is no way to join others, and none to try.\n";

fn number(args: &Value, key: &str, default: f64) -> f64 {
    args[key].as_f64().unwrap_or(default)
}

/// The tools as `tools/list` gives them, in a fixed order.
pub fn list() -> Value {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    let acts = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    json!([
        {
            "name": "get_state",
            "title": "Get the state",
            "description": "Everything readable at once: whether VRChat runs and was started by this bridge, which world and instance, whether that instance is private, the window's size and focus, the camera, the recording, the 3D pictures taken and their reconstructions, whether the person paused you. Call it first, and whenever unsure. Its `next` says what to do now.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "find_worlds",
            "title": "Find worlds",
            "description": "Lists worlds to go to, with their ids for go_to_world. Use it when you were not given world ids. VRChat's API is rate limited: search for what you need rather than paging through everything.",
            "inputSchema": { "type": "object", "properties": {
                "source": { "type": "string", "enum": ["search", "visited", "favorites", "trending", "popular", "active"], "description": "search: by name, needs query. visited: where this account has been, latest first. favorites: this account's favorite worlds. trending: what is hot right now. popular: most played of all time. active: most people in them now." },
                "query": { "type": "string", "description": "Words in the world's name, for source search." },
                "page": { "type": "integer", "minimum": 0, "description": "From 0." },
            }, "required": ["source"], "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "go_to_world",
            "title": "Go to a world",
            "description": "Goes to a new private instance of a world, owned by the signed-in VRChat account: makes the instance through VRChat's API, closes the VRChat this bridge started if it runs, starts VRChat in desktop mode in a window, and waits up to about 40 s. Going to a world takes as long as a start of the game, about a minute or more. Returns `arrived`; if false, call wait_for with until in_world. Only private instances exist here: there is no way to name an instance, or to join anyone. At most one every 30 s.",
            "inputSchema": { "type": "object", "properties": {
                "world_id": { "type": "string", "description": "wrld_… from find_worlds." },
                "region": { "type": "string", "enum": ["eu", "us", "use", "jp"], "description": "Where the instance runs. Default eu." },
                "profile": { "type": "integer", "minimum": 0, "description": "VRChat's --profile, when the signed-in account is on another profile than the default." },
            }, "required": ["world_id"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "close_vrchat",
            "title": "Close VRChat",
            "description": "Closes the VRChat this bridge started, as its window's close button does, and waits for it to go. Call it at the end of a run: reconstructions that wait for VRChat start once it is closed. Refused while a picture records.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "wait_for",
            "title": "Wait for something",
            "description": "Waits, up to timeout_s, for something that takes long, and returns `done` with the state. Call it again when `done` is false and it still makes sense.",
            "inputSchema": { "type": "object", "properties": {
                "until": { "type": "string", "enum": ["in_world", "closed", "picture_saved", "reconstructions_done", "not_paused"], "description": "in_world: go_to_world arrived, or its failure. closed: VRChat is gone. picture_saved: the last recording is saved. reconstructions_done: none running or waiting (with the app set to wait for VRChat, only true once it is closed). not_paused: the person left the computer alone." },
                "timeout_s": { "type": "number", "minimum": 1, "maximum": 50, "description": "Default 45." },
            }, "required": ["until"], "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "screenshot",
            "title": "Look at VRChat's window",
            "description": "A picture of VRChat's window and nothing else, even when something covers it. Returns the JPEG, and timestamp, window_width, window_height, image_width, image_height, and scale. mouse takes window pixels: window pixel = image pixel / scale. At the default width the two are the same. all_black true means the loading screen, or that the capture does not work here. Refused while the person is at the computer, or in an instance that is not private.",
            "inputSchema": { "type": "object", "properties": {
                "max_width": { "type": "integer", "minimum": 320, "maximum": 2560, "description": "Default 1280." },
            }, "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "walk",
            "title": "Walk",
            "description": "Walks for a time, over OSC, which needs no window focus. Blocks until done. `speed_seen` is the avatar's speed as VRChat reported it, when it did: 0 or nothing while walking means blocked, seated, or a menu or the camera open (press esc closes them). How far that is depends on the world; judge by screenshot. At most 10 s a call, so that a lost connection never leaves the avatar walking.",
            "inputSchema": { "type": "object", "properties": {
                "direction": { "type": "string", "enum": ["forward", "backward", "left", "right"], "description": "Relative to where the avatar faces. Left and right sidestep." },
                "seconds": { "type": "number", "exclusiveMinimum": 0, "maximum": 10 },
                "run": { "type": "boolean", "description": "Faster, if the world allows it." },
            }, "required": ["direction", "seconds"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "turn",
            "title": "Turn",
            "description": "Turns the avatar by about so many degrees, over OSC, which needs no window focus. Approximate: it holds VRChat's look input for degrees / rate seconds, and the real speed depends on VRChat's settings, so compare screenshots, and pass rate_deg_per_s once you know better. With a menu or the camera open it does nothing. Positive turns right, negative left.",
            "inputSchema": { "type": "object", "properties": {
                "degrees": { "type": "number", "minimum": -360, "maximum": 360 },
                "rate_deg_per_s": { "type": "number", "exclusiveMinimum": 0, "description": "Default 165." },
            }, "required": ["degrees"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "press",
            "title": "Press an input",
            "description": format!("Presses one input for a short time, then lets go. OSC inputs need no focus: jump; quick_menu opens or closes the menu; look_horizontal, move_vertical and move_horizontal are axes taking value from -1 to 1. Keys need VRChat to be the foreground window: esc is the Quick Menu, r the Action Menu, c crouch, z prone. Only these names exist: {}. Nothing that types or speaks is among them.", input::input_names().join(", ")),
            "inputSchema": { "type": "object", "properties": {
                "input": { "type": "string", "enum": input::input_names() },
                "seconds": { "type": "number", "exclusiveMinimum": 0, "maximum": 10, "description": "Default 0.1." },
                "value": { "type": "number", "minimum": -1, "maximum": 1, "description": "For the axes. Default 1." },
            }, "required": ["input"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "mouse",
            "title": "Use the mouse",
            "description": "Mouse input, needing VRChat to be the foreground window. move puts the pointer over a point of the window (x, y in window pixels); click does that and clicks (with a menu open: while the game has the mouse, a click only uses what the avatar faces). look moves the mouse by dx, dy counts, which is how to look up and down with no menu open. Click only what you see in the screenshot you just took, and only the camera and menus you need: not chat, invites, friends, avatars, the shop or safety. It takes about 0.6 s: the pointer rests first, or VRChat's menus ignore the press.",
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string", "enum": ["move", "click", "look"] },
                "x": { "type": "integer", "minimum": 0 },
                "y": { "type": "integer", "minimum": 0 },
                "dx": { "type": "integer", "minimum": -2000, "maximum": 2000 },
                "dy": { "type": "integer", "minimum": -2000, "maximum": 2000 },
                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Default left." },
            }, "required": ["action"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "wear_avatar",
            "title": "Wear an avatar",
            "description": "Asks VRChat over OSC to change to an avatar, by its avtr_ id, and waits until it has changed. Only when the person asked for it, or named the avatar: what someone wears is theirs. VRChat takes only an avatar this account may wear and says nothing when it may not: `changed` is false then. `avatar.capture_kit` says whether 3D pictures work with it.",
            "inputSchema": { "type": "object", "properties": {
                "avatar_id": { "type": "string", "pattern": "^avtr_[0-9a-fA-F-]{36}$" },
                "timeout_s": { "type": "integer", "minimum": 1, "maximum": 60, "description": "Default 20." },
            }, "required": ["avatar_id"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "focus_game",
            "title": "Bring VRChat forward",
            "description": "Asks Windows to make VRChat the foreground window, for press's keys and mouse, when something else took the focus. Windows may refuse: `foreground` says what happened, and if it stays false only the person can click VRChat. Do not call it in a loop: if the focus went away, the person may be using the computer.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "take_3d_picture",
            "title": "Take a 3D picture",
            "description": "Takes a 3D picture with VRChat's camera where it stands: the camera flies by itself for the time asked while the app records it, then the recording is saved and queued for reconstruction. Needs the camera open (see the guide). Blocks up to wait_s, then returns the `session` and its state; if it is still recording, call wait_for with until picture_saved. `no_keyframes` means the worn avatar has no capture kit: nothing can be reconstructed from it. Reconstructions follow the app's setting, and may wait until VRChat is closed: `reconstruction` says.",
            "inputSchema": { "type": "object", "properties": {
                "length_s": { "type": "integer", "enum": [1, 4, 8], "description": "1: a gentle sweep around what the camera aims at. 4 and 8: flights around the place. Default 4." },
                "mode": { "type": "string", "enum": ["people", "world"], "description": "What the picture is of. world (the default): through the place, your avatar left out when it can be. people: around the people in it, for an avatar or a friend." },
                "wait_s": { "type": "number", "minimum": 0, "maximum": 50, "description": "Default 30." },
            }, "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "open_session",
            "title": "Open a VRChat session of your own",
            "description": "Starts VRChat in a profile of its own (VRChat's --profile, each with its own signed-in account), in desktop mode, beside any other VRChat on this computer, the person's included, and gives you its `session` id: pass it to every other tool from then on, and nobody else drives it. Use it to work at the same time as other agents or the person. It starts at that account's home; go_to_world then takes it to a private instance. If `needs_sign_in` is true, stop and tell the person to sign in on that window: never type there. A session left without any call for 10 minutes may be taken over by another agent with take_over.",
            "inputSchema": { "type": "object", "properties": {
                "profile": { "type": "integer", "minimum": 1, "maximum": 99, "description": "VRChat's profile number. 0 is the default profile, the person's and the main session's." },
                "take_over": { "type": "boolean", "description": "Take over this profile's session when its agent has left it alone for 10 minutes or more. Default false." },
            }, "required": ["profile"], "additionalProperties": false },
            "annotations": acts,
        },
        {
            "name": "list_sessions",
            "title": "List the VRChat sessions",
            "description": "The VRChat sessions the bridge runs: main (tools without session) and the ones open_session opened, each with its profile, account, world, who drives it and since when it is idle, and how many VRChats of the person's run. Ids are shortened: use the one open_session gave you.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "close_session",
            "title": "Close a VRChat session",
            "description": "Closes your session's VRChat, as its window's close button does, and ends the session: its id is refused afterwards. Call it when you are done; reconstructions waiting for VRChat start once no VRChat runs. Refused while a picture records, and for 90 s after the start of a VRChat that never signed in (Easy Anti-Cheat would stay loaded and fail the next start).",
            "inputSchema": { "type": "object", "properties": {
                "session": { "type": "string", "description": "The id open_session gave you." },
            }, "required": ["session"], "additionalProperties": false },
            "annotations": acts,
        },
    ])
    .as_array()
    .cloned()
    .unwrap_or_default()
    .into_iter()
    .map(|mut tool| {
        // Every tool that acts on one VRChat takes the session it acts on.
        if VRCHAT_TOOLS.iter().any(|n| tool["name"] == *n) {
            tool["inputSchema"]["properties"]["session"] = json!({
                "type": "string",
                "description": "Your session's id from open_session. Without it: the main session, as before.",
            });
        }
        tool
    })
    .chain(super::projects::list())
    .chain(super::sandbox::list())
    .collect::<Vec<Value>>()
    .into()
}

/// The failure for a pause.
pub fn paused_failure(paused: &Paused) -> Failure {
    match paused {
        Paused::Person { resumes_in_s } => Failure::new(
            "paused_by_person",
            format!(
                "The person is using the computer, so nothing was sent and input was released. It goes on {} s after their last input.",
                RESUME_AFTER.as_secs()
            ),
            "Call wait_for with until not_paused, then go on from where you were.",
        )
        .retry_after(*resumes_in_s),
        Paused::Held => Failure::new(
            "paused_held",
            format!("The person paused you (the hotkey {HOTKEY}, or Settings). It lasts until they resume."),
            "Call wait_for with until not_paused now and then, or stop and tell them what you were doing.",
        ),
    }
}

/// Whether two instances are one: VRChat's log may leave out what the API's name of it has at its end.
fn same_instance(a: &str, b: &str) -> bool {
    a.split('~').next() == b.split('~').next()
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Bridge {
    /// One tool call, whose outcome goes to the log (all but what only reads: those fill it with nothing).
    pub fn call(&self, name: &str, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let acts = !matches!(name, "get_state" | "wait_for" | "find_worlds" | "list_sessions") && !super::projects::reads(name);
        let result = match name {
            "vr_sandbox" => {
                if !matches!(args["action"].as_str(), Some("stop" | "restore" | "status")) {
                    if let Some(paused) = self.paused() { return Err(paused_failure(&paused)); }
                }
                let main = self.main.clone();
                self.exclusively(&main, || self.sandbox_call(args))
            }
            unity if super::projects::NAMES.contains(&unity) => self.projects.call(unity, args, call, self.paused()),
            "open_session" => self.open_session(args, call),
            "list_sessions" => Ok(Answer::Data(self.list_sessions())),
            "close_session" => self.session_of(args, true).and_then(|s| self.exclusively(&s, || self.close_session(&s, call))),
            vrchat if VRCHAT_TOOLS.contains(&vrchat) => {
                let s = self.session_of(args, true)?;
                let s = &*s;
                if s.is_main() && s.game.lent_away() && !matches!(vrchat, "get_state" | "find_worlds" | "wait_for") {
                    return Err(Failure::new(
                        "app_lent",
                        "Another session's 3D picture borrows Orbiters - N4N0 for its recording: for these seconds, nothing of the main session's can be sent or read.",
                        "Try again in 15 s.",
                    )
                    .retry_after(15));
                }
                match vrchat {
                    "get_state" => Ok(Answer::Data(self.state(s))),
                    "find_worlds" => self.find_worlds(s, args),
                    "go_to_world" => self.exclusively(s, || self.go_to_world(s, args, call)),
                    "close_vrchat" => self.exclusively(s, || self.close_vrchat(s, call)),
                    "wait_for" => self.wait_for(s, args, call),
                    "screenshot" => self.screenshot(s, args),
                    "walk" => self.exclusively(s, || self.walk(s, args, call)),
                    "turn" => self.exclusively(s, || self.turn(s, args, call)),
                    "press" => self.exclusively(s, || self.press(s, args, call)),
                    "mouse" => self.exclusively(s, || self.mouse(s, args)),
                    "focus_game" => self.exclusively(s, || self.focus_game(s)),
                    "wear_avatar" => self.exclusively(s, || self.wear_avatar(s, args)),
                    _ => self.exclusively(s, || self.take_3d_picture(s, args, call)),
                }
            }
            other => Err(Failure::new("unknown_tool", format!("There is no tool called {other}."), "Call tools/list.")),
        };
        let session = args["session"].as_str().map(|id| format!("[{}] ", short_id(id))).unwrap_or_default();
        match &result {
            Ok(_) if acts => self.log(true, format!("{session}{}", Self::describe(name, args))),
            Err(failure) => self.log(false, format!("{session}{}: {}", Self::describe(name, args), failure.code)),
            Ok(_) => {}
        }
        result
    }

    /// What a call asked for, for the log.
    fn describe(name: &str, args: &Value) -> String {
        match name {
            "go_to_world" => format!("go to {}", args["world_id"].as_str().unwrap_or("?")),
            "close_vrchat" => "close VRChat".into(),
            "walk" => format!("walk {} {:.1} s", args["direction"].as_str().unwrap_or("?"), number(args, "seconds", 0.0)),
            "turn" => format!("turn about {:.0}°", number(args, "degrees", 0.0)),
            "press" => format!("press {}", args["input"].as_str().unwrap_or("?")),
            "mouse" => match args["action"].as_str() {
                Some("look") => format!("look {}, {}", args["dx"].as_i64().unwrap_or(0), args["dy"].as_i64().unwrap_or(0)),
                action => format!("mouse {} at {}, {}", action.unwrap_or("?"), args["x"].as_i64().unwrap_or(0), args["y"].as_i64().unwrap_or(0)),
            },
            "focus_game" => "bring VRChat forward".into(),
            "open_session" => format!("open a session in profile {}", args["profile"].as_u64().map_or("?".into(), |p| p.to_string())),
            "close_session" => "close the session".into(),
            "wear_avatar" => format!("wear {}", args["avatar_id"].as_str().unwrap_or("?")),
            "take_3d_picture" => format!("3D picture of {} s", args["length_s"].as_u64().unwrap_or(4)),
            "screenshot" => "screenshot".into(),
            unity if unity.starts_with("unity_") => {
                let words: Vec<&str> = ["action", "what", "package", "path", "url", "id", "name", "template", "job"]
                    .iter()
                    .filter_map(|k| args[*k].as_str())
                    .collect();
                let job = args["job"].as_u64().map(|j| format!(" job {j}")).unwrap_or_default();
                format!("{} {}{job}", unity.trim_start_matches("unity_").replace('_', " "), words.join(" ")).trim().to_string()
            }
            other => other.to_string(),
        }
    }

    /// One action at a time in a session (sessions act side by side).
    fn exclusively<T>(&self, s: &Session, action: impl FnOnce() -> Result<T, Failure>) -> Result<T, Failure> {
        let Ok(_one) = s.action.try_lock() else {
            return Err(Failure::new("busy", "Another action is running in this session.", "Wait for its answer before starting the next: one at a time."));
        };
        action()
    }

    /// The game this bridge started, or why not.
    fn ours(&self, s: &Session, seen: &Seen) -> Result<u32, Failure> {
        let ours = s.run.lock().unwrap().pid;
        match (ours, seen.pid) {
            (Some(ours), Some(pid)) if ours == pid => Ok(pid),
            (_, Some(_)) => Err(Failure::new(
                "not_started_by_bridge",
                "This VRChat was not started by this bridge: it is the person's, so it is left alone.",
                "Leave it alone. open_session with a profile of your own (1, 2…) starts a VRChat of yours beside it; or the person closes theirs.",
            )),
            (_, None) => Err(Failure::new("vrchat_not_running", "VRChat is not running in this session.", "Call go_to_world.")),
        }
    }

    /// Whether the agent may act on the game at all now: not paused, the bridge's own game, in desktop mode, in a strict
    /// private instance of the signed-in account.
    fn may_act(&self, s: &Session) -> Result<(Seen, u32), Failure> {
        if let Some(paused) = self.paused_in(s) {
            return Err(paused_failure(&paused));
        }
        let seen = s.game.seen();
        if seen.needs_sign_in {
            return Err(sign_in_failure(s));
        }
        let pid = self.ours(s, &seen)?;
        Self::in_private_instance(&seen)?;
        Ok((seen, pid))
    }

    /// The rule of instances, on what is seen now.
    fn in_private_instance(seen: &Seen) -> Result<(), Failure> {
        if seen.desktop_mode == Some(false) {
            return Err(Failure::new(
                "vr_mode",
                "VRChat runs in VR: the bridge only drives desktop mode.",
                "Call close_vrchat and tell the person; a Steam launch option may override --no-vr.",
            ));
        }
        let me = Self::my_id(seen)?;
        if !seen.in_world {
            return Err(Failure::new("not_in_world", "VRChat is not in a world yet.", "Call wait_for with until in_world."));
        }
        let instance = seen.instance.as_deref().unwrap_or_default();
        // A side session's own account may own it too (its profile signs in with another account than the app's).
        let own = seen.session_account.as_ref().is_some_and(|(id, _)| private_instance_of(instance, id));
        if !private_instance_of(instance, me) && !own {
            return Err(Failure::new(
                "not_private_instance",
                format!("This is not a private instance of your own ({}), so movement, input and pictures are refused.", Self::kind_of_instance(instance)),
                "Call go_to_world with a world id, which makes one, or close_vrchat.",
            ));
        }
        Ok(())
    }

    fn my_id(seen: &Seen) -> Result<&str, Failure> {
        seen.vrchat_user.as_ref().map(|(id, _)| id.as_str()).ok_or_else(|| {
            Failure::new(
                "vrchat_not_signed_in",
                "No VRChat account is signed in in Orbiters - N4N0, so no private instance can be made or recognised.",
                "Tell the person to sign in to VRChat in Orbiters - N4N0's Settings.",
            )
        })
    }

    /// What kind of instance this is, in words.
    fn kind_of_instance(instance: &str) -> &'static str {
        if instance.contains("~canRequestInvite") {
            "invite+"
        } else if instance.contains("~private(") {
            "someone else's private one"
        } else if instance.contains("~friends(") {
            "friends"
        } else if instance.contains("~hidden(") {
            "friends+"
        } else if instance.contains("~group(") {
            "a group's"
        } else if instance.is_empty() {
            "unknown"
        } else {
            "public"
        }
    }

    fn window(&self, s: &Session, pid: u32) -> Result<GameWindow, Failure> {
        s.game.window(pid).ok_or_else(|| Failure::new("no_window", "VRChat has no window yet.", "Wait a moment and call get_state."))
    }

    /// Follows the game: the start's arrival or its end, and the game going away.
    pub(super) fn follow_game(&self, s: &Session) {
        // Lent to a side session: what the app sees is that VRChat's, which says nothing of this one's.
        if s.is_main() && s.game.lent_away() {
            return;
        }
        let seen = s.game.seen();
        let mut run = s.run.lock().unwrap();
        let mut settled = false;
        if let Some(going) = run.going.clone() {
            if going.started {
                if run.pid.is_none() && seen.pid.is_some() {
                    run.pid = seen.pid;
                }
                // Not at "Joining": the world is still loading for 20 s and more then (22 s for a world already in the cache, on
                // the first real run), and the player only exists once it loaded.
                let arrived = seen.in_world
                    && seen.local_player.is_some()
                    && seen.pid.is_some()
                    && seen.world_id.as_deref() == Some(&going.world_id)
                    && seen.instance.as_deref().is_some_and(|i| same_instance(i, &going.instance));
                if arrived {
                    run.going = None;
                    settled = true;
                } else if going.since.elapsed() > self.timing.start_limit {
                    run.going = None;
                    run.event = Some(self.why_no_arrival(&seen, &going));
                    settled = true;
                }
            }
        }
        // The game this bridge started went away by itself.
        if !run.closing && run.going.as_ref().is_none_or(|g| g.started) && run.pid.is_some() && run.pid != seen.pid {
            let running = run.going.is_some();
            run.pid = None;
            if running {
                run.going = None;
                settled = true;
            }
            run.event = Some("VRChat closed or crashed by itself.".into());
            drop(run);
            self.release_session(s);
            self.log(false, "VRChat closed or crashed by itself.");
        } else {
            drop(run);
        }
        if settled {
            s.game.hold_reconstructions(false);
        }
    }

    /// Why a start did not arrive, in a sentence a model can act on.
    fn why_no_arrival(&self, seen: &Seen, going: &Going) -> String {
        let limit = self.timing.start_limit.as_secs();
        if !seen.running {
            format!("VRChat did not start within {limit} s.")
        } else if !seen.log_found {
            "VRChat runs, but its log folder was not found, so its joining cannot be seen.".into()
        } else if seen.in_world && seen.world_id.as_deref() != Some(&going.world_id) {
            format!("VRChat went to another world ({}) instead of {}: the launch link was not taken.", seen.world_id.as_deref().unwrap_or("?"), going.world_id)
        } else if seen.in_world {
            format!("VRChat is in {}, but in another instance than the private one that was made for it.", going.world_id)
        } else {
            format!("VRChat runs but did not reach {} within {limit} s: the world did not load.", going.world_id)
        }
    }

    /// `get_state`.
    pub fn state(&self, s: &Session) -> Value {
        if s.is_main() && s.game.lent_away() {
            return json!({
                "session": { "id": sessions::MAIN },
                "app_lent_to_another_session": true,
                "next": "Another session's 3D picture borrows Orbiters - N4N0 for a few seconds: call get_state again in 15 s.",
            });
        }
        self.follow_game(s);
        let seen = s.game.seen();
        let (ours, going, event, pictures) = {
            let run = s.run.lock().unwrap();
            (run.pid, run.going.clone(), run.event.clone(), run.pictures.clone())
        };
        let started_by_bridge = ours.is_some() && ours == seen.pid;
        let window = seen.pid.filter(|_| started_by_bridge).and_then(|pid| s.game.window(pid));
        let camera_open = if started_by_bridge { s.game.camera_open() } else { None };
        let capture_kit = if started_by_bridge && seen.local_player.is_some() { s.game.capture_kit() } else { None };
        let private = seen.in_world && seen.instance.as_deref().is_some_and(|i| {
            seen.vrchat_user.iter().chain(seen.session_account.iter()).any(|(me, _)| private_instance_of(i, me))
        });
        let paused = self.paused_in(s);
        let picture_list: Vec<Value> = pictures.iter().map(|p| self.picture_json(s, p, &seen)).collect();
        let mut next = Self::next_step(&seen, &paused, started_by_bridge, private, going.is_some(), camera_open, capture_kit);
        if seen.needs_sign_in {
            next = sign_in_failure(s).next;
        } else if s.is_main() && seen.running && !started_by_bridge {
            next = "This VRChat is the person's: leave it alone. To work beside it, open_session with a profile of your own (1, 2…) and pass its session to every tool.".into();
        }
        // Once saved the file knows: the live count stops at its last report (296 of 700 frames in the first real picture, which
        // the agent noticed and could not explain).
        let mut recording = seen.capture.clone();
        if matches!(recording.state.as_str(), "idle" | "") {
            if let Some(session) = &recording.session {
                let files = s.game.picture_files(session);
                recording.frames = files.frames;
                recording.keyframes = files.keyframes;
            }
        }
        json!({
            "session": self.session_json(s, &seen),
            "bridge": {
                "paused": paused,
                "hotkey": HOTKEY,
                "last_event": event,
                "sessions": self.sessions().len(),
            },
            "vrchat": {
                "running": seen.running,
                "started_by_bridge": started_by_bridge,
                "starting": going.as_ref().map(|g| json!({ "world_id": g.world_id, "seconds": g.since.elapsed().as_secs() })),
                "desktop_mode": seen.desktop_mode,
                "signed_in_as": seen.vrchat_user.as_ref().map(|u| u.1.clone()),
                "in_game_as": seen.local_player,
                "osc_answers": seen.osc_found,
                "window": window.map(|w| json!({ "width": w.width, "height": w.height, "foreground": s.game.is_foreground(&w) })),
                "steamvr_running": seen.steamvr,
            },
            "place": {
                "in_world": seen.in_world,
                "world_id": seen.world_id.as_ref().filter(|_| seen.in_world),
                "world_name": seen.world_name.as_ref().filter(|_| seen.in_world),
                "instance": seen.instance.as_ref().filter(|_| seen.in_world),
                "private": private,
                "others_here": seen.others,
            },
            "avatar": { "name": seen.avatar_name, "capture_kit": capture_kit },
            "camera": { "open": camera_open, "pose_known": seen.camera_pose_known },
            "recording": recording,
            "reconstructions": {
                "running": seen.reconstruction,
                "waiting": seen.queue.iter().map(|p| file_name(p)).collect::<Vec<_>>(),
                "waiting_starts": seen.queue_waits_for,
            },
            "pictures": picture_list,
            "next": next,
        })
    }

    /// What to do now, for a model that just read the state.
    fn next_step(seen: &Seen, paused: &Option<Paused>, started_by_bridge: bool, private: bool, going: bool, camera_open: Option<bool>, capture_kit: Option<bool>) -> String {
        if paused.is_some() {
            return "The person took over: call wait_for with until not_paused.".into();
        }
        if going {
            return "VRChat is starting: call wait_for with until in_world.".into();
        }
        if seen.running && !started_by_bridge {
            return "This VRChat is the person's: leave it alone and tell them. It has to be closed before you can start one.".into();
        }
        if seen.vrchat_user.is_none() {
            return "No VRChat account is signed in in Orbiters - N4N0: tell the person to sign in in its Settings.".into();
        }
        if !seen.running {
            if !seen.queue.is_empty() || seen.reconstruction.is_some() {
                return "VRChat is closed and reconstructions are running: call wait_for with until reconstructions_done, or go_to_world for more pictures."
                    .into();
            }
            return "Call find_worlds if you need a world, then go_to_world.".into();
        }
        if !seen.in_world {
            return "VRChat is between worlds: call wait_for with until in_world.".into();
        }
        if !private {
            return "This instance is not private: call go_to_world or close_vrchat. Nothing else is allowed here.".into();
        }
        if seen.capture.state != "idle" {
            return "A recording is under way: call wait_for with until picture_saved.".into();
        }
        if capture_kit == Some(false) {
            return format!("{NO_KIT} Tell the person, who can change the avatar or name one for wear_avatar: you may not choose one. Look around if that helps them, then close_vrchat.");
        }
        if camera_open != Some(true) {
            return "Open VRChat's camera: press esc, screenshot, click the camera icon in the menu's bottom row, screenshot, click Stream Camera.".into();
        }
        if !seen.camera_pose_known {
            return "turn by a few degrees: VRChat says where its camera is when it moves.".into();
        }
        "Look around with screenshot, walk and turn, then take_3d_picture; go_to_world when this world is done, close_vrchat at the end.".into()
    }

    fn picture_json(&self, s: &Session, picture: &PictureRecord, seen: &Seen) -> Value {
        json!({
            "session": file_name(&picture.session),
            "world_id": picture.world_id,
            "world_name": picture.world_name,
            "length_s": picture.length_s,
            "recording": seen.capture.session.as_ref() == Some(&picture.session) && seen.capture.state != "idle",
            "frames": s.game.picture_files(&picture.session).frames,
            "keyframes": s.game.picture_files(&picture.session).keyframes,
            "reconstruction": self.reconstruction_of(s, &picture.session, seen),
        })
    }

    /// Where a session's reconstruction stands.
    fn reconstruction_of(&self, s: &Session, session: &std::path::Path, seen: &Seen) -> Value {
        if let Some(job) = seen.reconstruction.as_ref().filter(|j| j.session == session) {
            return json!({ "state": "running", "stage": job.stage, "progress": job.progress });
        }
        if let Some(position) = seen.queue.iter().position(|d| d == session) {
            return json!({ "state": "waiting", "position": position, "starts": seen.queue_waits_for });
        }
        let files = s.game.picture_files(session);
        if files.reconstructed {
            return json!({ "state": "done" });
        }
        if let Some(message) = files.failed {
            return json!({ "state": "failed", "message": message, "next": "Tell the person: this picture has no result, and the app has already told them why." });
        }
        json!({ "state": "none" })
    }

    fn find_worlds(&self, s: &Session, args: &Value) -> Result<Answer, Failure> {
        let page = args["page"].as_u64().unwrap_or(0) as usize;
        let find = match args["source"].as_str() {
            Some("search") => {
                let query = args["query"].as_str().unwrap_or_default().trim();
                if query.is_empty() {
                    return Err(Failure::new("bad_arguments", "source search needs query.", "Pass words from the world's name in query."));
                }
                Find::Search(query.to_string())
            }
            Some("visited") => Find::Visited,
            Some("favorites") => Find::Favorites,
            Some("popular") => Find::Popular,
            Some("active") => Find::Active,
            Some("trending") => Find::Trending,
            _ => return Err(Failure::new("bad_arguments", "source must be search, visited, favorites, trending, popular or active.", "Pass one of those.")),
        };
        let worlds = s.game.find_worlds(&find, page)?;
        Ok(Answer::Data(json!({ "worlds": worlds, "page": page })))
    }

    fn go_to_world(&self, s: &Session, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let world_id = args["world_id"].as_str().unwrap_or_default();
        if !is_world_id(world_id) {
            return Err(Failure::new("bad_world_id", format!("{world_id:?} is not a world id."), "Pass a wrld_… id from find_worlds."));
        }
        let region = args["region"].as_str().unwrap_or("eu");
        if !["eu", "us", "use", "jp"].contains(&region) {
            return Err(Failure::new("bad_arguments", "region must be eu, us, use or jp.", "Pass one of those, or none."));
        }
        if let Some(paused) = self.paused_in(s) {
            return Err(paused_failure(&paused));
        }
        let seen = s.game.seen();
        Self::my_id(&seen)?;
        if seen.needs_sign_in {
            return Err(sign_in_failure(s));
        }
        let pid = match seen.pid {
            Some(_) => Some(self.ours(s, &seen)?),
            None => None,
        };
        if !matches!(seen.capture.state.as_str(), "idle" | "") {
            return Err(Failure::new("picture_under_way", "A recording is under way.", "Call wait_for with until picture_saved first."));
        }
        let every = self.timing.instance_every;
        if let Some(wait) = s.run.lock().unwrap().last_instance.map(|at| at.elapsed()).filter(|e| *e < every).map(|e| every - e) {
            let seconds = wait.as_secs() + 1;
            return Err(Failure::new(
                "too_soon",
                format!("Worlds are changed at most once every {} s: VRChat's API and the game both limit it.", every.as_secs()),
                format!("Try again in {seconds} s."),
            )
            .retry_after(seconds));
        }
        // A side session's account cannot travel to a private instance of the app's account ("You are not allowed to
        // travel to that location", measured 2026-11-15): it makes its own, as VRChat's client does, strict private and owned
        // by its own account (`<n>~private(<its usr>)~region(<r>)~nonce(<uuid>)`: joined at once, 2026-11-15 03:11).
        let (instance, owner) = if s.is_main() {
            (s.game.create_private_instance(world_id, region)?, seen.vrchat_user.as_ref().map(|u| u.0.clone()))
        } else {
            let Some((account, _)) = seen.session_account.clone() else { return Err(sign_in_failure(s)) };
            (sessions::own_private_instance(&account, region), Some(account))
        };
        // What came from the outside is checked again here: only a private instance of this account gets started.
        if !owner.is_some_and(|me| private_instance_of(&instance, &me)) {
            return Err(Failure::new(
                "not_private_instance",
                "VRChat's API did not make a private instance, so it is not joined.",
                "Tell the person: this should not happen.",
            ));
        }
        {
            let mut run = s.run.lock().unwrap();
            run.last_instance = Some(Instant::now());
            run.event = None;
            run.going = Some(Going { world_id: world_id.to_string(), instance: instance.clone(), since: Instant::now(), started: false });
        }
        // From here on, ending early must hand the reconstructions back.
        s.game.hold_reconstructions(true);
        let start = (|| {
            if let Some(pid) = pid {
                self.release_session(s);
                self.close_and_wait(s, pid, call)?;
            }
            if self.paused_in(s).is_some() || self.stopped_since(call) {
                return Err(self.paused_in(s).map_or_else(
                    || Failure::new("stopped", "The person took control back in Orbiters - N4N0.", "Stop here: the person has to turn the bridge on again."),
                    |p| paused_failure(&p),
                ));
            }
            if !s.is_main() {
                let plan = LaunchPlan { world_id: world_id.to_string(), instance: instance.clone(), width: WINDOW_WIDTH, height: WINDOW_HEIGHT, profile: s.profile, osc: s.osc };
                return self.launch_side(s, plan, call);
            }
            let profile = args["profile"].as_u64().map(|p| p as u32);
            let plan = LaunchPlan { world_id: world_id.to_string(), instance: instance.clone(), width: WINDOW_WIDTH, height: WINDOW_HEIGHT, profile, osc: None };
            // Beside side sessions, one launch at a time: a new VRChat is told apart by appearing after its launch.
            let _one = self.launching.lock().unwrap();
            s.game.launch(&plan)
        })();
        if let Err(failure) = start {
            s.run.lock().unwrap().going = None;
            s.game.hold_reconstructions(false);
            return Err(failure);
        }
        {
            let mut run = s.run.lock().unwrap();
            if let Some(going) = run.going.as_mut() {
                going.started = true;
                going.since = Instant::now();
            }
        }
        let arrived = self.wait_until(Duration::from_secs(40), call, || !self.is_going(s))?;
        Ok(Answer::Data(self.world_outcome(s, arrived, world_id)?))
    }

    fn is_going(&self, s: &Session) -> bool {
        self.follow_game(s);
        s.run.lock().unwrap().going.is_some()
    }

    /// What became of a start: where it arrived, still on its way, or the failure.
    fn world_outcome(&self, s: &Session, settled: bool, world_id: &str) -> Result<Value, Failure> {
        if !settled {
            return Ok(json!({ "arrived": false, "world_id": world_id, "next": "VRChat is still on its way: call wait_for with until in_world." }));
        }
        let event = s.run.lock().unwrap().event.clone();
        let seen = s.game.seen();
        if let Some(event) = event {
            return Err(Failure::new("start_timeout", event, "Call close_vrchat and go on with the next world. If this is the first world, tell the person."));
        }
        Self::in_private_instance(&seen)?;
        let foreground = self.forward_on_arrival(s, &seen);
        let capture_kit = s.game.capture_kit();
        Ok(json!({
            "arrived": true,
            "avatar": { "name": seen.avatar_name, "capture_kit": capture_kit },
            "world_id": seen.world_id,
            "world_name": seen.world_name,
            "instance": seen.instance,
            "private": true,
            "foreground": foreground,
            "next": if capture_kit == Some(false) {
                "The avatar worn has no capture kit, so a 3D picture would have nothing to reconstruct: stop and tell the person. They can change the avatar, or name one for wear_avatar."
            } else if foreground {
                "Look with screenshot. The camera must be opened in each new start of VRChat."
            } else {
                "Look with screenshot. Windows did not give VRChat the focus, so keys and the mouse are refused until focus_game works or the person clicks it; walk and turn work without it. The camera must be opened in each new start of VRChat, which takes the mouse."
            },
        }))
    }

    /// A game just arrived in its world: it gets the foreground, or an unattended run could not open its camera. The person
    /// is not at the computer, or the agent would be paused and this not reached. Whether it has it now.
    fn forward_on_arrival(&self, s: &Session, seen: &Seen) -> bool {
        let Some(window) = seen.pid.and_then(|pid| s.game.window(pid)) else { return false };
        if !s.is_main() {
            // Beside other sessions and the person: only on its turn, and never from the person's VRChat.
            return self.focus_for(s, &window).is_ok();
        }
        if !s.game.is_foreground(&window) && self.paused_in(s).is_none() {
            s.game.bring_forward(&window);
            std::thread::sleep(Duration::from_millis(250));
        }
        s.game.is_foreground(&window)
    }

    /// Waits until `done`, the time is up (`false`), or the call has to end.
    fn wait_until(&self, limit: Duration, call: &Call, done: impl Fn() -> bool) -> Result<bool, Failure> {
        let begin = Instant::now();
        loop {
            if done() {
                return Ok(true);
            }
            if call.client_gone() {
                return Err(Failure::new("client_gone", "The client disconnected.", "Call get_state when back."));
            }
            if self.stopped_since(call) {
                return Err(Failure::new(
                    "stopped",
                    "The person took control back in Orbiters - N4N0.",
                    "Stop here: the person has to turn the bridge on again.",
                ));
            }
            if begin.elapsed() >= limit {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn close_and_wait(&self, s: &Session, pid: u32, call: &Call) -> Result<(), Failure> {
        let window = self.window(s, pid)?;
        s.run.lock().unwrap().closing = true;
        let closed = s.game.close(&window).and_then(|()| self.wait_until(CLOSE_LIMIT, call, || !s.game.seen().running));
        {
            // In one step with what it says: the listener's next look finds either the game ours or nothing to tell.
            let mut run = s.run.lock().unwrap();
            run.closing = false;
            if matches!(closed, Ok(true)) {
                run.pid = None;
            }
        }
        if !closed? {
            return Err(Failure::new(
                "close_timeout",
                format!("VRChat did not close within {} s.", CLOSE_LIMIT.as_secs()),
                "It may show a dialog: look with screenshot. It is not forced, so tell the person if it stays.",
            ));
        }
        Ok(())
    }

    fn close_vrchat(&self, s: &Session, call: &Call) -> Result<Answer, Failure> {
        if let Some(paused) = self.paused_in(s) {
            return Err(paused_failure(&paused));
        }
        let seen = s.game.seen();
        let pid = self.ours(s, &seen)?;
        if !matches!(seen.capture.state.as_str(), "idle" | "") {
            return Err(Failure::new("picture_under_way", "A recording is under way.", "Call wait_for with until picture_saved first."));
        }
        self.release_session(s);
        self.close_and_wait(s, pid, call)?;
        let seen = s.game.seen();
        Ok(Answer::Data(json!({
            "closed": true,
            "reconstructions_waiting": seen.queue.len(),
            "next": "Reconstructions waiting for VRChat start now: call wait_for with until reconstructions_done, as many times as it takes.",
        })))
    }

    fn wait_for(&self, s: &Session, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let until = args["until"].as_str().unwrap_or_default();
        let limit = Duration::from_secs_f64(number(args, "timeout_s", 45.0).clamp(1.0, LONGEST_WAIT_S));
        let done = match until {
            "in_world" => self.wait_until(limit, call, || !self.is_going(s))?,
            "closed" => self.wait_until(limit, call, || !s.game.seen().running)?,
            "picture_saved" => self.wait_until(limit, call, || matches!(s.game.seen().capture.state.as_str(), "idle" | ""))?,
            "reconstructions_done" => self.wait_until(limit, call, || {
                let seen = s.game.seen();
                seen.reconstruction.is_none() && seen.queue.is_empty()
            })?,
            "not_paused" => self.wait_until(limit, call, || self.paused_in(s).is_none())?,
            "signed_in" => self.wait_until(limit, call, || s.game.seen().session_account.is_some() && !s.game.seen().needs_sign_in)?,
            _ => {
                return Err(Failure::new(
                    "bad_arguments",
                    "until must be in_world, closed, picture_saved, reconstructions_done, not_paused or signed_in.",
                    "Pass one of those.",
                ))
            }
        };
        if until == "in_world" && done {
            let seen = s.game.seen();
            if !seen.running {
                let event = s.run.lock().unwrap().event.clone();
                return Err(event.map_or_else(
                    || Failure::new("vrchat_not_running", "VRChat is not running, and nothing is on its way.", "Call go_to_world."),
                    |event| Failure::new("start_timeout", event, "Go on with the next world. If this is the first world, tell the person."),
                ));
            }
            let world_id = seen.world_id.unwrap_or_default();
            return Ok(Answer::Data(self.world_outcome(s, true, &world_id)?));
        }
        Ok(Answer::Data(json!({ "done": done, "state": self.state(s) })))
    }

    fn screenshot(&self, s: &Session, args: &Value) -> Result<Answer, Failure> {
        if let Some(paused) = self.paused_in(s) {
            return Err(paused_failure(&paused));
        }
        let seen = s.game.seen();
        let pid = self.ours(s, &seen)?;
        // Between worlds there is nobody to see; in a world, only in a private one.
        if seen.in_world {
            Self::in_private_instance(&seen)?;
        }
        let window = self.window(s, pid)?;
        let max_width = args["max_width"].as_u64().unwrap_or(WINDOW_WIDTH as u64).clamp(320, 2560) as u32;
        let shot = s.game.capture(&window, max_width)?;
        let scale = if window.width == 0 { 1.0 } else { shot.width as f64 / window.width as f64 };
        let info = json!({
            "timestamp": orbiters_core::visited::now(),
            "window_width": window.width,
            "window_height": window.height,
            "image_width": shot.width,
            "image_height": shot.height,
            "scale": scale,
            "foreground": s.game.is_foreground(&window),
            "all_black": shot.all_black,
        });
        Ok(Answer::Picture(info, shot))
    }

    /// A press's `still_ok`: the instance stays what allowed it.
    fn still_private<'a>(&'a self, s: &'a Session) -> impl Fn() -> Result<(), Failure> + 'a {
        move || Self::in_private_instance(&s.game.seen())
    }

    fn walk(&self, s: &Session, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let address = match args["direction"].as_str() {
            Some("forward") => "/input/MoveForward",
            Some("backward") => "/input/MoveBackward",
            Some("left") => "/input/MoveLeft",
            Some("right") => "/input/MoveRight",
            _ => return Err(Failure::new("bad_arguments", "direction must be forward, backward, left or right.", "Pass one of those.")),
        };
        let seconds = input::bounded_seconds(number(args, "seconds", 0.0))?;
        self.may_act(s)?;
        let mut holds = vec![(Hold::OscButton(address), Some(OscArg::Int(1)), None)];
        if args["run"].as_bool().unwrap_or(false) {
            holds.push((Hold::OscButton("/input/Run"), Some(OscArg::Int(1)), None));
        }
        let speed = std::sync::Mutex::new(None::<f32>);
        let still_ok = || {
            let seen = s.game.seen();
            if let Some(s) = seen.speed {
                let mut most = speed.lock().unwrap();
                *most = Some(most.map_or(s, |m: f32| m.max(s)));
            }
            Self::in_private_instance(&seen)
        };
        let held = self.press_for(s, call, &holds, seconds, None, &still_ok)?;
        let speed_seen = speed.lock().unwrap().or(s.game.seen().speed);
        let note = match speed_seen {
            Some(s) if s < 0.1 => Some("VRChat reported no movement: blocked, seated, or a menu open. Look with screenshot."),
            _ => None,
        };
        Ok(Answer::Data(json!({ "walked_s": held.as_secs_f64(), "speed_seen": speed_seen, "note": note })))
    }

    fn turn(&self, s: &Session, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let degrees = number(args, "degrees", 0.0);
        if !degrees.is_finite() || degrees == 0.0 || degrees.abs() > 360.0 {
            return Err(Failure::new("bad_arguments", "degrees must be between -360 and 360, and not 0.", "Positive turns right, negative left."));
        }
        let rate = number(args, "rate_deg_per_s", TURN_RATE).clamp(1.0, 720.0);
        let seconds = input::bounded_seconds(input::turn_seconds(degrees, rate))?;
        self.may_act(s)?;
        let address = if degrees > 0.0 { "/input/LookRight" } else { "/input/LookLeft" };
        let holds = [(Hold::OscButton(address), Some(OscArg::Int(1)), None)];
        let held = self.press_for(s, call, &holds, seconds, None, &self.still_private(s))?;
        Ok(Answer::Data(json!({
            "turned_about_degrees": degrees,
            "held_s": held.as_secs_f64(),
            "rate_assumed_deg_per_s": rate,
            "note": "Approximate. Compare screenshots; pass rate_deg_per_s when you know better.",
        })))
    }

    fn press(&self, s: &Session, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let name = args["input"].as_str().unwrap_or_default();
        let seconds = input::bounded_seconds(number(args, "seconds", 0.1))?;
        if let Some((_, address, axis)) = OSC_INPUTS.iter().copied().find(|i| i.0 == name) {
            self.may_act(s)?;
            let (hold, value) = if axis {
                let value = number(args, "value", 1.0).clamp(-1.0, 1.0) as f32;
                (Hold::OscAxis(address), OscArg::Float(value))
            } else {
                (Hold::OscButton(address), OscArg::Int(1))
            };
            let held = self.press_for(s, call, &[(hold, Some(value), None)], seconds, None, &self.still_private(s))?;
            return Ok(Answer::Data(json!({ "pressed": name, "held_s": held.as_secs_f64(), "over": "osc" })));
        }
        let Some((_, scan, extended)) = KEYS.iter().copied().find(|k| k.0 == name) else {
            return Err(Failure::new(
                "unknown_input",
                format!("{name:?} is not an input: only {} exist. Nothing that types or speaks is available, on purpose.", input::input_names().join(", ")),
                "Pick one of those.",
            ));
        };
        let (_, pid) = self.may_act(s)?;
        let window = self.window(s, pid)?;
        self.focus_for(s, &window)?;
        let holds = [(Hold::Key { scan, extended }, None, Some(Input::Key { scan, extended, down: true }))];
        let still_ok = || {
            Self::in_private_instance(&s.game.seen())?;
            if s.game.is_foreground(&window) {
                Ok(())
            } else {
                Err(Self::not_foreground())
            }
        };
        let held = self.press_for(s, call, &holds, seconds, Some(&window), &still_ok)?;
        Ok(Answer::Data(json!({ "pressed": name, "held_s": held.as_secs_f64(), "over": "keyboard" })))
    }

    fn not_foreground() -> Failure {
        Failure::new(
            "not_foreground",
            "VRChat is not the foreground window, so no keyboard or mouse input was sent.",
            "walk, turn and the OSC inputs work without the focus. For menus call focus_game; if that does not help, the person must click VRChat.",
        )
    }

    fn mouse(&self, s: &Session, args: &Value) -> Result<Answer, Failure> {
        let action = args["action"].as_str().unwrap_or_default();
        let (_, pid) = self.may_act(s)?;
        let window = self.window(s, pid)?;
        self.focus_for(s, &window)?;
        let inside = || -> Result<(u32, u32), Failure> {
            let (x, y) = (args["x"].as_u64(), args["y"].as_u64());
            let (Some(x), Some(y)) = (x, y) else {
                return Err(Failure::new(
                    "bad_arguments",
                    format!("{action} needs x and y, in the window's pixels."),
                    "See screenshot's window_width and window_height.",
                ));
            };
            if x >= window.width as u64 || y >= window.height as u64 {
                return Err(Failure::new(
                    "outside_window",
                    format!("({x}, {y}) is outside VRChat's window, which is {} by {}.", window.width, window.height),
                    "Take a screenshot: window pixel = image pixel / scale.",
                ));
            }
            Ok((x as u32, y as u32))
        };
        match action {
            "move" => {
                let (x, y) = inside()?;
                self.send_own(s, &window, &[Input::MoveTo { x, y }])?;
                Ok(Answer::Data(json!({ "moved_to": [x, y] })))
            }
            "click" => {
                let (x, y) = inside()?;
                let button = match args["button"].as_str() {
                    None | Some("left") => MouseButton::Left,
                    Some("right") => MouseButton::Right,
                    Some("middle") => MouseButton::Middle,
                    _ => return Err(Failure::new("bad_arguments", "button must be left, right or middle.", "Pass one of those, or none.")),
                };
                // The pointer first, and time for the game to see it there: VRChat's menus only take a press on what the pointer has
                // been over for a moment, so that with both in one go the first click on anything only highlighted it.
                self.send_own(s, &window, &[Input::MoveTo { x, y }])?;
                std::thread::sleep(CLICK_HOVER);
                let hold = Hold::Button(button);
                s.held.press(hold.clone(), Instant::now() + CLICK_HOLD * 5);
                self.tell_guard(format!("down {}", hold.guard_line()));
                let sent = self.send_own(s, &window, &[Input::Button { button, down: true }]);
                if sent.is_ok() {
                    // Long enough for a game that samples its input each frame, at the frame rates it has with a capture running.
                    std::thread::sleep(CLICK_HOLD);
                }
                if s.held.lift(&hold) {
                    self.lift_now(s, &hold);
                }
                sent?;
                Ok(Answer::Data(
                    json!({ "clicked": [x, y], "button": args["button"].as_str().unwrap_or("left"), "next": "Look with screenshot to see what it did." }),
                ))
            }
            "look" => {
                let (dx, dy) = (args["dx"].as_i64().unwrap_or(0).clamp(-2000, 2000) as i32, args["dy"].as_i64().unwrap_or(0).clamp(-2000, 2000) as i32);
                if dx == 0 && dy == 0 {
                    return Err(Failure::new(
                        "bad_arguments",
                        "look needs dx or dy.",
                        "Pass mouse counts: how far that turns depends on VRChat's sensitivity.",
                    ));
                }
                self.send_own(s, &window, &[Input::MoveBy { dx, dy }])?;
                Ok(Answer::Data(json!({ "looked": [dx, dy] })))
            }
            _ => Err(Failure::new("bad_arguments", "action must be move, click or look.", "Pass one of those.")),
        }
    }

    fn focus_game(&self, s: &Session) -> Result<Answer, Failure> {
        let (_, pid) = self.may_act(s)?;
        let window = self.window(s, pid)?;
        if !s.is_main() {
            self.focus_for(s, &window)?;
            return Ok(Answer::Data(json!({ "foreground": true, "next": "Keys and the mouse reach this session's VRChat now, for as long as you keep using them (another session's turn comes after 8 s without)." })));
        }
        s.game.bring_forward(&window);
        std::thread::sleep(Duration::from_millis(250));
        let foreground = s.game.is_foreground(&window);
        Ok(Answer::Data(json!({
            "foreground": foreground,
            "next": if foreground { "Keys and the mouse reach VRChat now." } else { "Windows said no. Only the person can click VRChat. walk, turn and the OSC inputs work without it." },
        })))
    }

    fn wear_avatar(&self, s: &Session, args: &Value) -> Result<Answer, Failure> {
        let id = args["avatar_id"].as_str().unwrap_or_default();
        if !is_avatar_id(id) {
            return Err(Failure::new("not_an_avatar", format!("{id:?} is not an avatar's id."), "An id is avtr_ and 36 characters: the person can give it, and VRChat's site shows it."));
        }
        let (seen, _) = self.may_act(s)?;
        let before = seen.avatar_name;
        s.game.wear(id)?;
        let timeout = Duration::from_secs(args["timeout_s"].as_u64().unwrap_or(20).clamp(1, 60));
        let start = Instant::now();
        let mut name = before.clone();
        while name == before && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(250));
            // The person took over, or this is no place to act any more: only the wait ends, what was asked stays asked.
            let Ok((seen, _)) = self.may_act(s) else { break };
            name = seen.avatar_name;
        }
        let changed = name != before;
        let kit = s.game.capture_kit();
        Ok(Answer::Data(json!({
            "changed": changed,
            "avatar": { "id": id, "name": name, "capture_kit": kit },
            "next": if !changed {
                "VRChat is still on the avatar it wore: it may be downloading the new one, the id may be the one already worn, or this account may not wear it. Look with screenshot and get_state, and tell the person."
            } else if kit == Some(false) {
                "It has no capture kit: 3D pictures will not work with it."
            } else {
                "Worn. Look with screenshot."
            },
        })))
    }

    fn take_3d_picture(&self, s: &Session, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let length_s = args["length_s"].as_u64().unwrap_or(4) as u32;
        let length = match length_s {
            1 => Photo3dLength::Quick,
            4 => Photo3dLength::Standard,
            8 => Photo3dLength::Full,
            other => return Err(Failure::new("bad_arguments", format!("length_s {other} is not a length."), "Pass 1, 4 or 8.")),
        };
        let mode = match args["mode"].as_str().unwrap_or("world") {
            "world" => PictureMode::World,
            "people" => PictureMode::People,
            other => return Err(Failure::new("bad_arguments", format!("mode {other} is not a kind of picture."), "Pass world or people.")),
        };
        let (seen, _) = self.may_act(s)?;
        if !matches!(seen.capture.state.as_str(), "idle" | "") {
            return Err(Failure::new("picture_under_way", "A recording is under way.", "Call wait_for with until picture_saved first."));
        }
        // Before any recording: without the kit there are no camera poses, and all of it would be for nothing.
        if s.game.capture_kit() == Some(false) {
            return Err(Failure::new(
                "no_capture_kit",
                format!("{NO_KIT} ({})", seen.avatar_name.as_deref().unwrap_or("its name is not known")),
                "Stop the run and tell the person to wear an avatar with N4N0's capture kit. Do not change the avatar yourself, unless the person named one: then wear_avatar.",
            ));
        }
        if s.game.camera_open() != Some(true) {
            return Err(Failure::new(
                "camera_closed",
                "VRChat's camera is not open, and nothing but its menu opens it.",
                "press esc, screenshot, click the camera icon in the menu's bottom row, screenshot, click Stream Camera. If get_state still says camera.open is not true, OSC may be off: in VRChat's Action Menu (r), Options, OSC, turn it on.",
            ));
        }
        if !seen.camera_pose_known {
            return Err(Failure::new(
                "camera_pose_unknown",
                "VRChat has not said where its camera is.",
                "turn by a few degrees, then try again: it says so when the camera moves.",
            ));
        }
        let before = seen.capture.session.clone();
        s.game.take_picture(length, mode)?;
        // The session is known once the recording started: a new one, as the last recording's stays named meanwhile.
        let began = self.wait_until(Duration::from_secs(10), call, || {
            let capture = s.game.seen().capture;
            capture.session.is_some() && capture.session != before && capture.state != "starting"
        })?;
        let seen = s.game.seen();
        if !began || seen.capture.session.is_none() {
            return Err(Failure::new(
                "picture_did_not_start",
                seen.capture.error.unwrap_or_else(|| "The recording did not start.".into()),
                "Check that VRChat's Stream camera sends: look with screenshot, and tell the person if it stays.",
            ));
        }
        let session = seen.capture.session.clone().unwrap_or_default();
        s.run.lock().unwrap().pictures.push(PictureRecord {
            session: session.clone(),
            world_id: seen.world_id.clone(),
            world_name: seen.world_name.clone(),
            length_s,
        });
        let wait = Duration::from_secs_f64(number(args, "wait_s", 30.0).clamp(0.0, LONGEST_WAIT_S));
        let saved = self.wait_until(wait, call, || {
            let capture = s.game.seen().capture;
            matches!(capture.state.as_str(), "idle" | "") || capture.session.as_ref() != Some(&session)
        })?;
        let seen = s.game.seen();
        let files = s.game.picture_files(&session);
        if saved && files.frames > 0 && files.keyframes == 0 {
            return Err(Failure::new(
                "no_keyframes",
                format!("The recording {} has no keyframes: {}", file_name(&session), seen.capture.no_strip.unwrap_or_else(|| "the avatar worn has no N4N0 capture kit, or the camera's Local User is off.".into())),
                "Nothing can be reconstructed from it. Try one more world; if this happens again, stop and tell the person to wear an avatar with the capture kit.",
            ));
        }
        Ok(Answer::Data(json!({
            "session": file_name(&session),
            "mode": if mode == PictureMode::World { "world" } else { "people" },
            "saved": saved,
            "frames": files.frames,
            "keyframes": files.keyframes,
            "reconstruction": self.reconstruction_of(s, &session, &seen),
            "next": if saved { "Take another, or go on. Its reconstruction follows the app's setting: `reconstruction` says where it stands." } else { "Still recording: call wait_for with until picture_saved." },
        })))
    }
}

/// What a session needing a sign-in says, everywhere.
fn sign_in_failure(s: &Session) -> Failure {
    Failure::new(
        "needs_sign_in",
        format!(
            "This session's VRChat (profile {}) waits at its sign-in page: no account is signed in on that profile.",
            s.profile.map_or("0".into(), |p| p.to_string())
        ),
        "Stop and tell the person: they sign in themselves, on that VRChat's window (the bridge never types there). Then call get_state, or wait_for until signed_in.",
    )
}

impl Bridge {
    /// The session a call names (`session`), or main without one. A side session renews its lease with every call.
    pub(crate) fn session_of(&self, args: &Value, renew: bool) -> Result<Arc<Session>, Failure> {
        let Some(id) = args["session"].as_str().filter(|id| *id != sessions::MAIN) else { return Ok(self.main.clone()) };
        let sides = self.sides.lock().unwrap();
        let Some(s) = sides.iter().find(|s| s.id == id).cloned() else {
            let profile = id.split_once('-').and_then(|(p, _)| p.strip_prefix('p')).and_then(|p| p.parse::<u32>().ok());
            let taken = profile.is_some_and(|p| sides.iter().any(|s| s.profile == Some(p)));
            return Err(if taken {
                Failure::new(
                    "session_taken_over",
                    "This session was taken over by another agent after ten minutes without a call: it is not yours any more.",
                    "Stop driving it. list_sessions shows who has it; open_session another profile if you need one.",
                )
            } else {
                Failure::new("unknown_session", format!("There is no session {id}: it was closed, or never opened."), "Call list_sessions, or open_session.")
            });
        };
        if renew {
            if let Some(lease) = s.lease.lock().unwrap().as_mut() {
                lease.last_call = Instant::now();
            }
        }
        Ok(s)
    }

    /// The keyboard and the mouse for `s`'s window: on its turn, with the person away from the computer, and never by
    /// taking the foreground from a VRChat the bridge did not start (the person's own, on the desktop or in VR).
    pub(crate) fn focus_for(&self, s: &Session, window: &GameWindow) -> Result<(), Failure> {
        if s.is_main() {
            return if s.game.is_foreground(window) { Ok(()) } else { Err(Self::not_foreground()) };
        }
        if let Some(paused) = self.paused() {
            return Err(paused_failure(&paused));
        }
        if let Err(wait) = self.focus.lock().unwrap().ask(&s.id, Instant::now()) {
            return Err(Failure::new(
                "focus_busy",
                "Another session is using the keyboard and the mouse: only one VRChat window can have them at a time.",
                format!("Try again in {wait} s; walk, turn, the OSC inputs and screenshot need no turn."),
            )
            .retry_after(wait));
        }
        if s.game.is_foreground(window) {
            return Ok(());
        }
        let opener = self.opener.lock().unwrap().clone();
        let ours: Vec<u32> = self.sessions().iter().filter_map(|x| x.pid()).collect();
        if let Some(fg) = opener.as_ref().and_then(|o| o.foreground_pid()) {
            if opener.as_ref().is_some_and(|o| o.vrchat_pids().contains(&fg)) && !ours.contains(&fg) {
                return Err(Failure::new(
                    "person_vrchat_has_focus",
                    "The person's own VRChat has the foreground: the bridge never takes it away from them.",
                    "walk, turn, the OSC inputs and screenshot work without the focus. For keys and clicks, wait until the person leaves their VRChat (or ask them).",
                )
                .retry_after(60));
            }
        }
        s.game.bring_forward(window);
        std::thread::sleep(Duration::from_millis(250));
        if s.game.is_foreground(window) {
            Ok(())
        } else {
            Err(Self::not_foreground())
        }
    }

    /// The VRChats running that no session of the bridge's started: the person's.
    fn persons_vrchats(&self, opener: &dyn super::game::Opener) -> Vec<u32> {
        let ours: Vec<u32> = self.sessions().iter().filter_map(|s| s.pid()).collect();
        opener.vrchat_pids().into_iter().filter(|p| !ours.contains(p)).collect()
    }

    /// `open_session`: starts VRChat in a profile of its own beside everything else, at that account's home.
    fn open_session(&self, args: &Value, call: &Call) -> Result<Answer, Failure> {
        let Some(profile) = args["profile"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
            return Err(Failure::new("bad_arguments", "open_session needs profile, a number from 1.", "Pass the VRChat profile to use: 1, 2…"));
        };
        let Some(osc) = sessions::ports_of(profile) else {
            return Err(Failure::new(
                "bad_profile",
                format!("Profile {profile} cannot be a session of its own: profile 0 is the default one, the person's and the main session's."),
                "Use profile 1 or more. Tools without session drive the main session (profile 0) as before.",
            ));
        };
        if let Some(paused) = self.paused() {
            return Err(paused_failure(&paused));
        }
        let Some(opener) = self.opener.lock().unwrap().clone() else {
            return Err(Failure::new("sessions_unavailable", "This bridge cannot open sessions here.", "Use the main session: tools without session."));
        };
        let owner = self.client_seen().map_or_else(|| "an MCP client".to_string(), |c| c.name);
        // The profile's session already: its own agent's, or one left alone long enough to take over.
        let existing = self.sides.lock().unwrap().iter().find(|s| s.profile == Some(profile)).cloned();
        if let Some(existing) = existing {
            let lease = existing.lease.lock().unwrap().clone();
            let stale = lease.as_ref().is_none_or(|l| l.stale(Instant::now()));
            if !stale || !args["take_over"].as_bool().unwrap_or(false) {
                let idle = lease.as_ref().map_or(0, |l| l.last_call.elapsed().as_secs());
                return Err(Failure::new(
                    "session_in_use",
                    format!("Profile {profile} already has a session, driven by {} (last call {idle} s ago).", lease.map_or_else(|| "nobody".into(), |l| l.owner)),
                    if stale {
                        "It has been left alone for more than ten minutes: call open_session again with take_over true to take it over (its VRChat stays as it is).".to_string()
                    } else {
                        "Use another profile, or wait: a session left alone for ten minutes can be taken over.".to_string()
                    },
                ));
            }
            // Taken over: the same VRChat, a new key; the old one is refused from now on.
            self.release_session(&existing);
            let renamed = Session::new(&sessions::new_id(profile), existing.profile, existing.osc, existing.game.clone());
            *renamed.run.lock().unwrap() = std::mem::take(&mut *existing.run.lock().unwrap());
            *renamed.lease.lock().unwrap() = Some(Lease { owner: owner.clone(), last_call: Instant::now() });
            {
                let mut sides = self.sides.lock().unwrap();
                sides.retain(|s| s.id != existing.id);
                sides.push(renamed.clone());
            }
            self.focus.lock().unwrap().leave(&existing.id);
            opener.rename(&existing.id, &renamed.id);
            self.log(true, format!("session of profile {profile} taken over by {owner}"));
            return Ok(Answer::Data(json!({ "session": renamed.id, "profile": profile, "taken_over": true, "state": self.state(&renamed) })));
        }
        // Not a profile the person's VRChat runs: one account cannot be in two VRChats, and theirs is never touched.
        if opener.others_profiles().contains(&profile) {
            return Err(Failure::new(
                "profile_in_use",
                format!("A VRChat the bridge did not start runs with profile {profile}: it is the person's."),
                "Use another profile.",
            ));
        }
        let running = self.sessions().iter().filter(|s| !s.is_main() || s.pid().is_some()).count();
        let limit = opener.limit();
        if running >= limit {
            return Err(Failure::new(
                "too_many_sessions",
                format!("The bridge runs {running} VRChat sessions, the most this computer is set to carry ({limit}: Settings, Experimental, VRChat sessions at once). Each VRChat in a world takes 2 to 6 GB of graphics memory and about 2 GB of memory."),
                "Close a session you no longer need (close_session), or use one you have.",
            ));
        }
        if let Some(free) = opener.memory_free().filter(|f| *f < sessions::MEMORY_NEEDED) {
            return Err(Failure::new(
                "low_memory",
                format!("Only {:.1} GB of memory is free; a VRChat needs about 4.", free as f64 / f64::from(1u32 << 30)),
                "Close a session, or tell the person.",
            ));
        }
        let id = sessions::new_id(profile);
        let game = opener.side(&id, profile, osc)?;
        let s = Session::new(&id, Some(profile), Some(osc), game);
        *s.lease.lock().unwrap() = Some(Lease { owner: owner.clone(), last_call: Instant::now() });
        self.launch_side(&s, LaunchPlan { world_id: String::new(), instance: String::new(), width: WINDOW_WIDTH, height: WINDOW_HEIGHT, profile: Some(profile), osc: Some(osc) }, call)?;
        self.sides.lock().unwrap().push(s.clone());
        // Its log says who signed in, or that it waits for one.
        let known = self.wait_until(Duration::from_secs(40), call, || {
            let seen = s.game.seen();
            seen.needs_sign_in || seen.session_account.is_some()
        })?;
        self.log(true, format!("session of profile {profile} opened for {owner}"));
        let seen = s.game.seen();
        Ok(Answer::Data(json!({
            "session": id,
            "profile": profile,
            "signed_in_as": seen.session_account.as_ref().map(|a| a.1.clone()),
            "needs_sign_in": seen.needs_sign_in,
            "osc_ports": { "in": osc.0, "out": osc.1 },
            "next": if seen.needs_sign_in {
                "This profile has no account signed in: stop and tell the person to sign in on its VRChat window. Never type there yourself. Then wait_for until signed_in.".to_string()
            } else if known {
                format!("Pass session \"{id}\" to every tool from now on. Then go_to_world: it starts this session's VRChat again in a private instance.")
            } else {
                format!("VRChat is still starting: call get_state with session \"{id}\" in a moment.")
            },
        })))
    }

    /// Starts a side session's VRChat: one launch at a time on this computer, once Easy Anti-Cheat allows it, and known by
    /// its process before the next launch may start.
    fn launch_side(&self, s: &Session, plan: LaunchPlan, call: &Call) -> Result<(), Failure> {
        let opener = self.opener.lock().unwrap().clone().ok_or_else(|| Failure::new("sessions_unavailable", "This bridge cannot open sessions here.", "Use the main session."))?;
        let _one = self.launching.lock().unwrap();
        // Easy Anti-Cheat of a VRChat that just closed: wait for it, within the call.
        if !self.wait_until(Duration::from_secs(30).min(self.timing.start_limit), call, || opener.eac().may_launch())? {
            return Err(opener.eac().failure());
        }
        s.game.launch(&plan)?;
        let started = self.wait_until(Duration::from_secs(30), call, || s.game.seen().pid.is_some())?;
        let pid = s.game.seen().pid;
        if !started || pid.is_none() {
            return Err(Failure::new(
                "start_failed",
                "VRChat did not start within 30 s: Easy Anti-Cheat may show a launch error on the screen.",
                "Call get_state; if a session cannot start twice, stop and tell the person.",
            ));
        }
        s.run.lock().unwrap().pid = pid;
        Ok(())
    }

    /// `list_sessions`.
    fn list_sessions(&self) -> Value {
        let opener = self.opener.lock().unwrap().clone();
        let sessions: Vec<Value> = self
            .sessions()
            .iter()
            .map(|s| {
                let seen = s.game.seen();
                let lease = s.lease.lock().unwrap().clone();
                json!({
                    "session": if s.is_main() { sessions::MAIN.to_string() } else { short_id(&s.id) },
                    "profile": s.profile.unwrap_or(0),
                    "running": s.pid().is_some() && seen.pid == s.pid(),
                    "signed_in_as": seen.session_account.as_ref().map(|a| a.1.clone()),
                    "needs_sign_in": seen.needs_sign_in,
                    "world_name": seen.world_name.filter(|_| seen.in_world),
                    "driven_by": lease.as_ref().map(|l| l.owner.clone()),
                    "idle_s": lease.as_ref().map(|l| l.last_call.elapsed().as_secs()),
                    "may_take_over": lease.as_ref().is_some_and(|l| l.stale(Instant::now())),
                })
            })
            .collect();
        let persons = opener.as_ref().map(|o| self.persons_vrchats(o.as_ref()).len()).unwrap_or(0);
        json!({
            "sessions": sessions,
            "persons_vrchats_running": persons,
            "limit": opener.as_ref().map(|o| o.limit()),
            "keyboard_and_mouse_turn": self.focus.lock().unwrap().holder().map(short_id),
            "next": "Your own session's full id is the one open_session gave you: pass it as session. Ids are not shown here, so that nobody drives another agent's session by accident.",
        })
    }

    /// `close_session`: closes the session's VRChat (as its own close button does) and forgets the session.
    fn close_session(&self, s: &Session, call: &Call) -> Result<Answer, Failure> {
        if s.is_main() {
            return Err(Failure::new("bad_arguments", "The main session is closed with close_vrchat.", "Pass the id open_session gave you."));
        }
        if let Some(paused) = self.paused_in(s) {
            return Err(paused_failure(&paused));
        }
        let seen = s.game.seen();
        if !matches!(seen.capture.state.as_str(), "idle" | "") {
            return Err(Failure::new("picture_under_way", "A recording is under way.", "Call wait_for with until picture_saved first."));
        }
        // Easy Anti-Cheat: a VRChat closed before its anti-cheat session began leaves the driver loaded for a minute
        // (2026-11-13). Signed in, it began two seconds after; never signed in, it is given 90 s.
        let young = s.opened.elapsed() < Duration::from_secs(90) && seen.session_account.is_none();
        if young {
            let wait = 90 - s.opened.elapsed().as_secs();
            return Err(Failure::new(
                "too_young_to_close",
                "This VRChat started moments ago and never signed in: closing it now would leave Easy Anti-Cheat loaded and fail the next launch on this computer.",
                format!("Try again in {wait} s."),
            )
            .retry_after(wait));
        }
        self.release_session(s);
        if let Some(pid) = s.pid().filter(|p| seen.pid == Some(*p)) {
            self.close_and_wait(s, pid, call)?;
        }
        self.sides.lock().unwrap().retain(|x| x.id != s.id);
        self.focus.lock().unwrap().leave(&s.id);
        if let Some(opener) = self.opener.lock().unwrap().clone() {
            opener.forget(&s.id);
        }
        Ok(Answer::Data(json!({ "closed": true, "session": s.id, "next": "This session is gone: its id is refused from now on." })))
    }

    /// What get_state says of the session itself.
    fn session_json(&self, s: &Session, seen: &Seen) -> Value {
        let lease = s.lease.lock().unwrap().clone();
        json!({
            "id": if s.is_main() { sessions::MAIN.to_string() } else { s.id.clone() },
            "profile": s.profile.unwrap_or(0),
            "account": seen.session_account.as_ref().map(|a| json!({ "id": a.0, "name": a.1 })),
            "needs_sign_in": seen.needs_sign_in,
            "osc_ports": s.osc.map(|(i, o)| json!({ "in": i, "out": o })),
            "driven_by": lease.map(|l| l.owner),
            "paused_here": self.paused_in(s),
            "keyboard_and_mouse_turn": self.focus.lock().unwrap().holder().map(|h| h == s.id),
        })
    }
}

/// A tool's error as MCP gives it: to the model, as a result it can read.
pub fn failure_json(failure: &Failure) -> Value {
    serde_json::to_value(failure).unwrap_or_else(|_| json!({ "code": failure.code }))
}

/// An avatar's id, as VRChat writes them.
fn is_avatar_id(id: &str) -> bool {
    id.strip_prefix("avtr_").is_some_and(|rest| rest.len() == 36 && rest.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_avatars_own_id_goes_to_vrchat() {
        assert!(is_avatar_id("avtr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf"));
        for no in ["", "avtr_", "usr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf", "avtr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf/x", "avtr_zzzzzzzz-87f0-4872-bc7e-22c5c071fdcf"] {
            assert!(!is_avatar_id(no), "{no}");
        }
    }

    #[test]
    fn the_tools_are_few_and_say_when_to_use_them() {
        let tools = list();
        let tools = tools.as_array().unwrap();
        let vrchat = tools.iter().filter(|t| !t["name"].as_str().unwrap().starts_with("unity_")).count();
        // The thirteen of a game, and three for the sessions (and the sandbox's, in its builds).
        assert!(vrchat <= 17, "few: {vrchat}");
        assert_eq!(tools.len(), vrchat + super::super::projects::NAMES.len(), "the Unity projects tools after them");
        for tool in tools {
            assert!(tool["description"].as_str().unwrap().len() > 60, "{tool}");
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names[0], "get_state");
        // The person may name an avatar to wear, nothing more: no way to look for one, or to change what it is.
        assert_eq!(names.iter().copied().filter(|n| n.contains("avatar")).collect::<Vec<&str>>(), vec!["wear_avatar"]);
        // No way to name an instance, or to do what is ruled out.
        let go = tools.iter().find(|t| t["name"] == "go_to_world").unwrap();
        let properties = go["inputSchema"]["properties"].as_object().unwrap();
        assert!(properties.keys().all(|k| !k.contains("instance") && !k.contains("location") && !k.contains("type")));
        for name in names {
            for banned in ["chat", "invite", "friend"] {
                assert!(!name.replace("vrchat", "").contains(banned), "{name}");
            }
        }
    }

    #[test]
    fn instances_are_compared_by_their_number_only() {
        assert!(same_instance("123~private(usr_1)~region(eu)~nonce(x)", "123~private(usr_1)~region(eu)"));
        assert!(!same_instance("123~private(usr_1)~region(eu)", "124~private(usr_1)~region(eu)"));
    }

    #[test]
    fn a_kind_of_instance_is_told_in_words() {
        for (instance, kind) in [
            ("1~private(usr_1)~canRequestInvite~region(eu)", "invite+"),
            ("1~private(usr_2)~region(eu)", "someone else's private one"),
            ("1~friends(usr_1)~region(eu)", "friends"),
            ("1~hidden(usr_1)~region(eu)", "friends+"),
            ("1~group(grp_1)~region(eu)", "a group's"),
            ("1~region(eu)", "public"),
            ("", "unknown"),
        ] {
            assert_eq!(Bridge::kind_of_instance(instance), kind);
        }
    }

    #[test]
    fn the_guide_explains_the_loop_and_when_to_give_up() {
        for words in ["go_to_world", "take_3d_picture", "close_vrchat", "give up", "paused_by_person", "no_keyframes", "open_session", "needs_sign_in", "focus_busy", "close_session", "take_over"] {
            assert!(GUIDE.contains(words), "{words}");
        }
    }
}
