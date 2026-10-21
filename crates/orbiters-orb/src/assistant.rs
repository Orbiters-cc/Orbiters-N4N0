//! The client of Orb's assistant on the Orbiters server (contracts `Docs/orb-assistant.md`): one request of a turn,
//! read as server-sent events. The conversation lives here, on the desktop; the server keeps nothing.

use anyhow::Context as _;
use orbiters_core::account::Api;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};

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
    #[serde(default)]
    pub messages: Vec<Value>,
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
    let message = text("message").or_else(|| text("error")).unwrap_or_else(|| match code.as_str() {
        "unauthorized" => "Sign in to Orbiters again to talk to Orb.".to_string(),
        "rate_limited" | "quota_exhausted" => "Orb needs a break: try again in a moment.".to_string(),
        _ => format!("Orb's assistant is not available right now ({status})."),
    });
    AssistantError { code, message }
}

/// Sends one request of a turn and reads its answer: `on_event` gets the text and progress as they come; the result
/// is the `done` payload. `cancel` stops reading at the next event.
pub fn send(api: &Api, token: &str, request: &Request, cancel: &AtomicBool, mut on_event: impl FnMut(Event)) -> anyhow::Result<Done> {
    let response = api
        .agent()
        .post(&api.url("orb/assistant/turns"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "text/event-stream")
        .send_json(request)
        .context("Orbiters could not be reached")?;
    let status = response.status().as_u16();
    let json = response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("application/json"));
    let mut body = response.into_body();
    if !(200..300).contains(&status) {
        let text = body.with_config().limit(1 << 20).read_to_string().unwrap_or_default();
        return Err(error_from(status, &text).into());
    }
    if json {
        let text = body.with_config().limit(8 << 20).read_to_string()?;
        let done: Done = serde_json::from_str(&text).context("unexpected answer from Orbiters")?;
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
        let line = line.context("the answer stopped")?;
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
    anyhow::bail!("the answer ended before it was complete")
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
        "done" => return Ok(Some(serde_json::from_value(value).context("unexpected end of answer")?)),
        "error" => return Err(error_from(200, data).into()),
        _ => {}
    }
    Ok(None)
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
        assert_eq!(done.messages.len(), 1);
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
