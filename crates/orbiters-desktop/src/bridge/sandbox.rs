//! Development-only adapter to the standalone, journaled SteamVR sandbox.
// i18n: not shown (developer MCP diagnostics and schema)
use super::{game::Failure, tools::Answer};
use serde_json::{json, Value};

fn root() -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) { return None; }
    std::env::var_os("ORBITERS_VR_SANDBOX_ROOT").map(std::path::PathBuf::from)
}

pub fn list() -> Vec<Value> {
    if root().is_none() { return vec![]; }
    vec![json!({
        "name":"vr_sandbox", "title":"Developer SteamVR sandbox",
        "description":"Shared real SteamVR + VRChat session with an emulated head and two oculus_touch hands; agents take turns on it (docs/vr-sandbox.md). start takes a turn (starting the session if none runs; it refuses the owner's own VR); stop ends this program's turn and puts the session back to its neutral state, it does not close VRChat; the session closes itself after 10 idle minutes and restores everything. restore works after a crash. vrchat launches VRChat once, or sends the new private instance to the running VRChat through its launch pipe. home rejoins the session's home instance; neutral releases every input and resets poses and the camera. pose uses Standing metres and quaternion w,x,y,z, duration_s makes a smooth move. input values remain held until released with zero. menu opens quick/wheel or releases it. flick moves the two stick axes together then returns to centre. camera opens through the right-hand Action Menu Tools route; inspect an eye capture to verify. camera_fly sends the documented OSC Flying control. capture saves left/right compositor PNGs under output_prefix. overlays reads SteamVR transforms and visibility for overlay_keys, or all known Orbiters menu/camera keys when omitted. Read docs/vr-sandbox.md before use.",
        "inputSchema":{"type":"object","properties":{
            "action":{"type":"string","enum":["start","stop","restore","status","pose","input","capture","motion_capture","overlays","vrchat","home","neutral","menu","flick","camera","camera_fly","camera_mode","view"]},
            "frames":{"type":"integer","minimum":3,"maximum":80,"description":"motion_capture: per-eye samples while pose moves, with QPC timestamps, controller poses and overlay readback. Default 24."},
            "mode":{"type":"integer","minimum":0,"maximum":6,"description":"Documented OSC camera mode: 0 Off, 1 Photo, 2 Stream, 3 Emoji, 4 Multilayer, 5 Print, 6 Drone. Open through camera first; inspect the actual mode after writing."},
            "menu":{"type":"string","enum":["quick","wheel","close"]},
            "stick":{"type":"array","items":{"type":"number","minimum":-1,"maximum":1},"minItems":2,"maxItems":2},
            "enabled":{"type":"boolean"},
            "world_id":{"type":"string"},
            "device":{"type":"string","enum":["head","left","right"]},
            "position":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},
            "rotation":{"type":"array","items":{"type":"number"},"minItems":4,"maxItems":4},
            "duration_s":{"type":"number","minimum":0,"maximum":30},
            "input":{"type":"string"},"value":{"type":"number","minimum":-1,"maximum":1},
            "output_prefix":{"type":"string"},"overlay_keys":{"type":"array","items":{"type":"string"}}
        },"required":["action"],"additionalProperties":false},
        "annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}
    })]
}

impl super::Bridge {
    pub fn sandbox_call(&self, args: &Value) -> Result<Answer, Failure> {
        if !matches!(args["action"].as_str(), Some("stop" | "restore" | "status")) {
            let seen=self.game.seen();
            if seen.running && !seen.vrchat_user.as_ref().zip(seen.instance.as_deref()).is_some_and(|((user,_),instance)|orbiters_core::vrchat_api::private_instance_of(instance,user)) {
                return Err(Failure::new("not_private_instance", "The running VRChat is not in this account's strict private instance.", "Stop the sandbox; do not send more input."));
            }
        }
        if args["action"] != "vrchat" { return call(args); }
        // A running sandbox VRChat gets the new instance through its own launch pipe (sandbox.py): no second
        // anti-cheat start. sandbox.py refuses a VRChat that its session did not start.
        let seen=self.game.seen();
        let (user,_) = seen.vrchat_user.ok_or_else(|| Failure::new("vrchat_not_signed_in", "No signed-in VRChat account.", "Use the existing signed-in bridge session."))?;
        let world=args["world_id"].as_str().unwrap_or_default();
        if !orbiters_core::vrchat_log::is_world_id(world) { return Err(Failure::new("bad_world_id", "Expected wrld_ id.", "Use find_worlds.")); }
        // Require the live sandbox BEFORE making an instance or launching anything.
        call(&json!({"action":"status"}))?;
        let instance=self.game.create_private_instance(world,"eu")?;
        if !orbiters_core::vrchat_api::private_instance_of(&instance,&user) { return Err(Failure::new("not_private_instance", "API returned an unexpected instance.", "Inspect the instance response.")); }
        call(&json!({"action":"vrchat","world_id":world,"instance":instance,"user_id":user}))
    }
}

pub fn call(args: &Value) -> Result<Answer, Failure> {
    let fail = |message: String| Failure::new("sandbox_error", message, "Inspect the recovery journal and docs/vr-sandbox.md; restore before starting another session.");
    let root = root().ok_or_else(|| fail("Developer sandbox is not enabled in this build/process.".into()))?;
    let mut command = std::process::Command::new("python");
    command.arg(root.join("sandbox.py")).arg("request").arg(args.to_string());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let result = command.output().map_err(|e| fail(e.to_string()))?;
    if !result.status.success() { return Err(fail(String::from_utf8_lossy(&result.stderr).into_owned())); }
    serde_json::from_slice(&result.stdout).map(Answer::Data).map_err(|e| fail(format!("Invalid sandbox response: {e}")))
}
