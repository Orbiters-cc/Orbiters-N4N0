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
        "description":"Exclusive real SteamVR session with an emulated head and two oculus_touch hands. start refuses existing VR sessions and journals configuration; stop restores it. restore works after a crash. pose uses Standing metres and quaternion w,x,y,z, duration_s makes a smooth move. input values remain held until released with zero. capture saves left/right compositor PNGs under output_prefix. overlays reads SteamVR transforms and visibility for overlay_keys. Read docs/vr-sandbox.md before use.",
        "inputSchema":{"type":"object","properties":{
            "action":{"type":"string","enum":["start","stop","restore","status","pose","input","capture","overlays"]},
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
