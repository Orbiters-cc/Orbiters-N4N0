//! What a window asks of Orbiters as the member it shows (`crate::member`): sent here, with that member's credential, under the
//! identity gate, or refused.

use super::Shared;
use crate::member::{MemberAnswer, MemberRequest};
use orbiters_core::account::{Api, Server};
use orbiters_i18n::tr;
use std::time::Duration;

/// The largest answer that goes to a window over the link; a larger one (a picture's file) goes in a file.
const ANSWER_LINKED: u64 = 1 << 20;
/// The largest answer at all.
const ANSWER_MOST: u64 = 2 << 30;
/// How long a file may take to come.
const FILE_WITHIN: Duration = orbiters_core::account::TRANSFER_TIMEOUT;

/// Where large answers wait for the window that asked: this user's own, emptied as the service starts.
fn spool() -> std::path::PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("orbiters-test-answers-{}", std::process::id()))
    } else {
        orbiters_core::paths::cache_dir().join("answers")
    }
}

/// Answers no window read (it closed meanwhile): gone.
pub fn empty_spool() {
    let _ = std::fs::remove_dir_all(spool());
}

/// `body`, read: in memory, or, when it is large, in a file.
fn taken(body: ureq::Body) -> anyhow::Result<(Vec<u8>, Option<std::path::PathBuf>)> {
    use std::io::Read;
    let mut reader = body.into_with_config().limit(ANSWER_MOST).reader();
    let mut bytes = Vec::new();
    (&mut reader).take(ANSWER_LINKED + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 <= ANSWER_LINKED {
        return Ok((bytes, None));
    }
    let dir = spool();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(uuid::Uuid::new_v4().to_string());
    let mut write = || -> anyhow::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&path)?;
        file.write_all(&bytes)?;
        std::io::copy(&mut reader, &mut file)?;
        Ok(file.flush()?)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok((Vec::new(), Some(path)))
}

/// Whether `path` names something under the API and nothing else.
fn within(path: &str) -> bool {
    let plain = path.split(['?', '#']).next().unwrap_or_default();
    !path.is_empty() && !path.starts_with('/') && !plain.contains("..") && !path.contains(['\\', '\r', '\n', ' ']) && !plain.contains("://") && !plain.contains(':')
}

impl super::Controller {
    /// Sends `request` as the member of `epoch`, whom the window showed. What changes something is sent while they are known
    /// to be signed in (a handover waits for it); a reading takes their credential then, and its answer is for them only
    /// (the window drops it when it shows another by then).
    pub fn member_request(self: &Shared, epoch: u64, request: MemberRequest) -> Result<MemberAnswer, String> {
        let acting = self.n4n0.acting(epoch).ok_or_else(|| tr("That was for the Orbiters account signed in before: nothing was done.").to_string())?;
        let (api, token) = acting.account().ok_or_else(|| tr("Sign in to Orbiters in Settings first.").to_string())?;
        let server = if request.development { Server::Development } else { Server::Production };
        if api.url("") != Api::new(server).url("") || !within(&request.path) {
            return Err(tr("This Orbiters account belongs to another environment: nothing was sent.").into());
        }
        let method = request.method.parse::<ureq::http::Method>().ok().filter(|m| ["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&m.as_str())).ok_or("method")?; // i18n: not shown
        let reads = method == ureq::http::Method::GET;
        let mut held = Some(acting);
        if reads {
            held = None;
        }
        let mut sent = ureq::http::Request::builder().method(method).uri(api.url(&request.path)).header("Authorization", format!("Bearer {token}"));
        if let Some(t) = &request.content_type {
            sent = sent.header("Content-Type", t);
        }
        if let Some(t) = &request.accept {
            sent = sent.header("Accept", t);
        }
        let unreachable = |e: ureq::Error| {
            log_warn!("member request: {e}", e = e);
            tr("Orbiters could not be reached").to_string()
        };
        let response = if reads {
            let sent = api
                .agent()
                .configure_request(sent.body(()).map_err(|e| e.to_string())?)
                .timeout_global(Some(FILE_WITHIN))
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(60)))
                .build();
            api.agent().run(sent)
        } else {
            // (A handover waits for these: not for long.)
            let sent = api
                .agent()
                .configure_request(sent.body(request.body).map_err(|e| e.to_string())?)
                .timeout_global(Some(Duration::from_secs(60)))
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_send_request(Some(Duration::from_secs(5)))
                .timeout_send_body(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(20)))
                .build();
            api.agent().run(sent)
        };
        // Sent as them, or not at all: what is left is reading the answer.
        drop(held);
        let response = response.map_err(unreachable)?;
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (status, content_type, retry_after) = (response.status().as_u16(), header("content-type"), header("retry-after"));
        let (body, file) = taken(response.into_body()).map_err(|e| {
            log_warn!("member request: {e:#}", e = e);
            tr("Orbiters could not be reached").to_string()
        })?;
        Ok(MemberAnswer { status, content_type, retry_after, body, file })
    }

    /// The member signed in, as the windows key what is theirs: the environment and the id.
    pub(super) fn member_key(&self) -> Option<String> {
        let rt = self.account_runtime.locked();
        rt.identity()?.key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify5-r21 m5: a small answer goes over the link, a large one in a file, whole.
    #[test]
    fn a_large_answer_waits_in_a_file() {
        let body = |n: usize| ureq::Body::builder().reader(std::io::Cursor::new((0..n).map(|i| (i % 251) as u8).collect::<Vec<u8>>()));
        let (bytes, file) = taken(body(ANSWER_LINKED as usize)).unwrap();
        assert!(bytes.len() as u64 == ANSWER_LINKED && file.is_none());
        let n = 3 << 20;
        let (bytes, file) = taken(body(n)).unwrap();
        let path = file.expect("in a file");
        let read = std::fs::read(&path).unwrap();
        assert!(bytes.is_empty() && read.len() == n && read.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
        std::fs::remove_file(path).unwrap();
    }
}
