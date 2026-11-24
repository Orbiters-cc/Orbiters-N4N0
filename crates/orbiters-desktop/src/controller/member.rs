//! What a window asks of Orbiters as the member it shows (`crate::member`): sent here, with that member's credential, under the
//! identity gate, or refused.

use super::Shared;
use crate::member::{MemberAnswer, MemberRequest};
use orbiters_core::account::{Api, Server};
use orbiters_i18n::tr;
use std::time::Duration;

/// The largest answer handed to a window (a picture's file).
const ANSWER_MOST: u64 = 48 << 20;

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
            eprintln!("member request: {e}");
            tr("Orbiters could not be reached").to_string()
        };
        let response = if reads {
            let sent = api.agent().configure_request(sent.body(()).map_err(|e| e.to_string())?).timeout_global(Some(Duration::from_secs(90))).build();
            api.agent().run(sent)
        } else {
            // (A handover waits for these: not for long.)
            let sent = api
                .agent()
                .configure_request(sent.body(request.body).map_err(|e| e.to_string())?)
                .timeout_global(Some(Duration::from_secs(60)))
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(20)))
                .build();
            api.agent().run(sent)
        };
        // Sent as them, or not at all: what is left is reading the answer.
        drop(held);
        let response = response.map_err(unreachable)?;
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (status, content_type, retry_after) = (response.status().as_u16(), header("content-type"), header("retry-after"));
        let body = response.into_body().into_with_config().limit(ANSWER_MOST).read_to_vec().map_err(unreachable)?;
        Ok(MemberAnswer { status, content_type, retry_after, body })
    }

    /// The member signed in, as the windows key what is theirs: the environment and the id.
    pub(super) fn member_key(&self) -> Option<String> {
        let rt = self.account_runtime.lock().unwrap();
        rt.identity()?.key()
    }
}
