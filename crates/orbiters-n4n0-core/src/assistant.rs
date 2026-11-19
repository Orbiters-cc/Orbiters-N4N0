//! The client of N4N0's assistant on the Orbiters server (contracts `Docs/n4n0-assistant.md`): one request of a turn,
//! read as server-sent events. The conversation lives here, on the desktop; the server keeps nothing.

use anyhow::Context as _;
use orbiters_core::account::Api;
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};

/// Limits of a turn request, as the server counts them (UTF-16 units, like JavaScript's `length`).
pub const SUMMARY_UNITS: usize = 6000;
pub const INPUT_UNITS: usize = 4000;
pub const RESULT_UNITS: usize = 16000;

/// The longest start of `text` that is at most `max` UTF-16 units, cut between characters.
pub fn utf16_prefix(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &text[..i];
        }
    }
    text
}

/// What the desktop knows about the moment, for the assistant.
#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub summary: String,
    pub locale: String,
    pub time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<Display>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Display {
    pub columns: u32,
    pub rows: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Input {
    pub text: String,
    /// `voice` or `typed`.
    pub via: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Request<'a> {
    pub client: &'static str,
    pub messages: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Input>,
    pub tool_results: Vec<ToolResult>,
    pub tools: &'a [Value],
    pub context: Context,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    #[serde(default)]
    pub used: u32,
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub own_key: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Done {
    /// The conversation as it now stands. A `done` without it (an error answered with a success status) is no answer:
    /// it never replaces the conversation.
    #[serde(default)]
    pub messages: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub quota: Option<Quota>,
    /// The whole answer's text (the JSON answer only).
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Confirmation {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub approve_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Text(String),
    Tool { name: String, phase: String },
    Confirmation(Confirmation),
}

/// A refused or failed request: the server's code and its message for the person.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantError {
    pub code: String,
    pub message: String,
    /// The same for the avatar's panel, which has Latin letters only: the server's own words as they came, the app's own in
    /// a Latin language.
    pub on_panel: String,
}

impl std::fmt::Display for AssistantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AssistantError {}

fn error_from(status: u16, body: &str) -> AssistantError {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
    let code = text("code").unwrap_or_else(|| {
        match status {
            401 | 403 => "unauthorized",
            429 => "rate_limited",
            400 | 413 | 422 => "bad_request",
            _ => "unavailable",
        }
        .to_string()
    });
    // The app's own words when the server sent none, in the person's language (TXT-22).
    let own = |code: &str| match code {
        "unauthorized" => orbiters_i18n::tr("Sign in to Orbiters again to talk to N4N0.").to_string(),
        "rate_limited" | "quota_exhausted" => orbiters_i18n::tr("N4N0 needs a break: try again in a moment.").to_string(),
        _ => orbiters_i18n::tr!("N4N0's assistant is not available right now ({status}).", status = status),
    };
    match text("message").or_else(|| text("error")) {
        Some(message) => AssistantError { code, on_panel: message.clone(), message },
        None => AssistantError { message: own(&code), on_panel: crate::text::latin(|| own(&code)), code },
    }
}

/// What a person reads when Orbiters cannot be reached at all (no connection, refused, no name, timed out): one plain
/// sentence, never the operating system's words; those go to the log.
pub const UNREACHABLE: &str = orbiters_i18n::mark("N4N0 can't reach Orbiters right now. Try again in a moment.");

/// Whether `e` says Orbiters could not be reached at all, rather than that it answered with a refusal or something
/// unreadable.
pub fn unreachable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<ureq::Error>(),
            Some(ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound | ureq::Error::Timeout(_) | ureq::Error::ConnectProxyFailed(_))
        ) || c.downcast_ref::<std::io::Error>().is_some()
    })
}

/// Sends one request of a turn and reads its answer: `on_event` gets the text and progress as they come; the result
/// is the `done` payload. `cancel` stops reading at the next event.
pub fn send(api: &Api, token: &str, request: &Request, cancel: &AtomicBool, mut on_event: impl FnMut(Event)) -> anyhow::Result<Done> {
    let response = api
        .agent()
        .post(&api.url("n4n0/assistant/turns"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "text/event-stream")
        .send_json(request)
        .with_context(|| tr("Orbiters could not be reached"))?;
    let status = response.status().as_u16();
    let json = response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("application/json"));
    let mut body = response.into_body();
    if !(200..300).contains(&status) {
        let text = body.with_config().limit(1 << 20).read_to_string().unwrap_or_default();
        return Err(error_from(status, &text).into());
    }
    if json {
        let text = body.with_config().limit(8 << 20).read_to_string()?;
        let done: Done = serde_json::from_str(&text).with_context(|| tr("unexpected answer from Orbiters"))?;
        if done.messages.is_none() {
            // An error object answered as a success: its words when it has some.
            return Err(error_from(status, &text).into());
        }
        if let Some(t) = &done.text {
            on_event(Event::Text(t.clone()));
        }
        return Ok(done);
    }
    read_events(BufReader::new(body.with_config().limit(8 << 20).reader()), cancel, &mut on_event)
}

/// Reads server-sent events until `done` or `error`.
pub fn read_events(reader: impl BufRead, cancel: &AtomicBool, on_event: &mut impl FnMut(Event)) -> anyhow::Result<Done> {
    let mut event = String::new();
    let mut data = String::new();
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled");
        }
        let line = line.with_context(|| tr("the answer stopped"))?;
        if line.is_empty() {
            if !data.is_empty() || !event.is_empty() {
                if let Some(done) = dispatch(&event, &data, on_event)? {
                    return Ok(done);
                }
            }
            event.clear();
            data.clear();
        } else if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
        // Comments (": heartbeat") and other fields are ignored.
    }
    if !data.is_empty() {
        if let Some(done) = dispatch(&event, &data, on_event)? {
            return Ok(done);
        }
    }
    anyhow::bail!("{}", tr("the answer ended before it was complete"))
}

fn dispatch(event: &str, data: &str, on_event: &mut impl FnMut(Event)) -> anyhow::Result<Option<Done>> {
    let value: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    match event {
        "text" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                on_event(Event::Text(delta.to_string()));
            }
        }
        "tool" => on_event(Event::Tool {
            name: value.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            phase: value.get("phase").and_then(Value::as_str).unwrap_or_default().to_string(),
        }),
        "confirmation" => {
            if let Ok(c) = serde_json::from_value::<Confirmation>(value) {
                on_event(Event::Confirmation(c));
            }
        }
        "done" => {
            let done: Done = serde_json::from_value(value).with_context(|| tr("unexpected end of answer"))?;
            anyhow::ensure!(done.messages.is_some(), "{}", tr("the answer ended without the conversation"));
            return Ok(Some(done));
        }
        "error" => return Err(error_from(200, data).into()),
        _ => {}
    }
    Ok(None)
}

/// Whether `message` starts a turn: the member's own message, not a tool's result (`tool_result` blocks, a `tool`
/// role, a call id), whatever the provider's shape the server keeps the conversation in.
pub fn starts_turn(message: &Value) -> bool {
    let role = message.get("role").and_then(Value::as_str);
    let result_block =
        |content: &Value| content.as_array().is_some_and(|blocks| blocks.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")));
    role == Some("user") && message.get("tool_call_id").is_none() && message.get("tool_use_id").is_none() && !message.get("content").is_some_and(result_block)
}

/// The conversation's last `max` messages or fewer, starting where a turn starts: never with a tool's result, whose
/// call was cut away (providers refuse a conversation that starts so). A turn longer than `max` is kept whole.
pub fn trim(mut messages: Vec<Value>, max: usize) -> Vec<Value> {
    if messages.len() <= max {
        return messages;
    }
    let earliest = messages.len() - max;
    let start = (earliest..messages.len()).find(|&i| starts_turn(&messages[i])).or_else(|| (0..earliest).rev().find(|&i| starts_turn(&messages[i])));
    match start {
        Some(start) => messages.split_off(start),
        // No turn starts anywhere: nothing can be cut safely.
        None => messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_stream() {
        let stream = "event: text\ndata: {\"delta\":\"Ça \"}\n\n: heartbeat\n\nevent: tool\ndata: {\"name\":\"create_event_draft\",\"phase\":\"running\"}\n\nevent: text\ndata: {\"delta\":\"marche\"}\n\nevent: confirmation\ndata: {\"id\":\"c1\",\"title\":\"Movie night\",\"summary\":\"Sat 21:00\",\"approve_url\":\"https://orbiters.cc/x\"}\n\nevent: done\ndata: {\"messages\":[{\"role\":\"user\"}],\"tool_calls\":[{\"id\":\"call_1\",\"name\":\"set_avatar_control\",\"input\":{\"control\":\"Hoodie\",\"on\":true}}],\"quota\":{\"used\":3,\"limit\":80,\"own_key\":false}}\n\n";
        let mut events = Vec::new();
        let done = read_events(stream.as_bytes(), &AtomicBool::new(false), &mut |e| events.push(e)).unwrap();
        assert_eq!(events[0], Event::Text("Ça ".into()));
        assert_eq!(events[1], Event::Tool { name: "create_event_draft".into(), phase: "running".into() });
        assert!(matches!(&events[3], Event::Confirmation(c) if c.approve_url == "https://orbiters.cc/x"));
        assert_eq!(done.tool_calls[0].name, "set_avatar_control");
        assert_eq!(done.quota.unwrap().used, 3);
        assert_eq!(done.messages.map(|m| m.len()), Some(1));
        // A done without the conversation is not an answer: the conversation it would replace stays.
        let empty = "event: done\ndata: {\"tool_calls\":[]}\n\n";
        assert!(read_events(empty.as_bytes(), &AtomicBool::new(false), &mut |_| {}).is_err());
        assert!(read_events(stream.as_bytes(), &AtomicBool::new(true), &mut |_| {}).is_err(), "cancelled");
    }

    #[test]
    fn trims_where_a_turn_starts() {
        let user = |t: &str| serde_json::json!({ "role": "user", "content": t });
        let call = serde_json::json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "c1", "name": "n4n0_mood", "input": {} }] });
        let result = serde_json::json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "c1", "content": "{}" }] });
        let answer = serde_json::json!({ "role": "assistant", "content": "ok" });
        assert!(starts_turn(&user("hi")) && !starts_turn(&result) && !starts_turn(&answer));
        assert!(!starts_turn(&serde_json::json!({ "role": "tool", "tool_call_id": "c1", "content": "{}" })));
        // Two turns of four messages: cut to five, the second turn starts the conversation, not its tool result.
        let conversation = vec![user("one"), call.clone(), result.clone(), answer.clone(), user("two"), call.clone(), result.clone(), answer.clone()];
        let kept = trim(conversation.clone(), 5);
        assert_eq!(kept.len(), 4);
        assert_eq!(kept[0], user("two"));
        assert_eq!(trim(conversation.clone(), 40).len(), 8, "short enough: as it is");
        // One turn longer than the limit stays whole rather than starting with a result.
        let long = vec![user("one"), call.clone(), result.clone(), call, result, answer];
        assert_eq!(trim(long, 3)[0], user("one"));
    }

    #[test]
    fn cuts_where_the_server_counts() {
        assert_eq!(utf16_prefix("abc", 2), "ab");
        // An emoji is two UTF-16 units: it does not fit in one.
        assert_eq!(utf16_prefix("a😀b", 2), "a");
        assert_eq!(utf16_prefix("a😀b", 3), "a😀");
        assert_eq!(utf16_prefix("éé", 10), "éé");
    }

    #[test]
    fn a_refused_connection_is_unreachable() {
        // Port 9 (discard) is closed here: the connection is refused, as with the dev server stopped (logs6/shot4.png).
        let api = Api::at("http://127.0.0.1:9/");
        let context = Context { summary: String::new(), locale: "en".into(), time: String::new(), display: None };
        let request = Request { client: "test", messages: &[], input: None, tool_results: Vec::new(), tools: &[], context };
        let err = send(&api, "token", &request, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(unreachable(&err), "{err:#}");
        assert!(!unreachable(&anyhow::Error::new(error_from(503, "{}"))));
        assert!(!unreachable(&anyhow::anyhow!("unexpected answer from Orbiters")));
        assert!(!UNREACHABLE.contains("os error") && !UNREACHABLE.contains("io:"));
    }

    #[test]
    fn errors_carry_the_servers_words() {
        let stream = "event: error\ndata: {\"code\":\"quota_exhausted\",\"message\":\"Free quota used up until midnight.\"}\n\n";
        let err = read_events(stream.as_bytes(), &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        let err = err.downcast::<AssistantError>().unwrap();
        assert_eq!(err.code, "quota_exhausted");
        assert_eq!(err.message, "Free quota used up until midnight.");
        assert_eq!(error_from(401, "").code, "unauthorized");
        assert_eq!(error_from(500, r#"{"error":"Boom"}"#).message, "Boom");
        assert!(read_events("event: text\ndata: {}\n\n".as_bytes(), &AtomicBool::new(false), &mut |_| {}).is_err(), "no done");
    }
}
