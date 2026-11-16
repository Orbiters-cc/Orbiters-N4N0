//! Sending the kept reports to Orbiters (Settings › Beta feedback › Send reports): `POST /n4n0/menu-reports`, one report a request,
//! with its picture when it has one. Unauthenticated, as the beta's usage statistics are: a report carries no account. A report Orbiters
//! took gets a `sent` file in its folder and stays on this computer until deleted.

use super::{Kept, Report, PICTURE, REPORT, SENT};
use base64::Engine as _;
use std::time::Duration;

/// The largest request sent: the server takes JSON bodies up to 1 MB. A bigger report loses its oldest lines first, then its picture.
pub const MOST_BYTES: usize = 900 * 1024;

/// What a round of sending did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub sent: usize,
    pub failed: usize,
    /// Why the last one failed, in English for the log (the page says it in its own words).
    pub why: Option<String>,
    /// Orbiters answered that it does not take reports (an older server).
    pub refused: bool,
}

/// The Orbiters reports go to: the app's server; a debug build may be pointed at a server under test (`ORBITERS_MENU_REPORTS_API`).
pub fn api() -> orbiters_core::account::Api {
    if cfg!(debug_assertions) {
        if let Some(base) = std::env::var("ORBITERS_MENU_REPORTS_API").ok().filter(|b| !b.is_empty()) {
            return orbiters_core::account::Api::at(&base);
        }
    }
    orbiters_core::account::Api::new(orbiters_core::account::Server::current())
}

/// The request's body for a report: the report, and its picture as base64 PNG, made to fit [`MOST_BYTES`].
pub fn body(mut report: Report, picture: Option<Vec<u8>>) -> serde_json::Value {
    let mut picture = picture.map(|p| base64::engine::general_purpose::STANDARD.encode(p));
    loop {
        let value = serde_json::json!({ "report": report, "picture": picture });
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |v| v.len());
        if size <= MOST_BYTES {
            return value;
        }
        // The oldest lines first: what came just before the press matters most.
        let before = report.timeline.iter().filter(|e| e.t < -5_000).count();
        if before > 0 {
            let drop = before.div_ceil(4).max(1);
            report.timeline.drain(..drop);
        } else if picture.is_some() {
            picture = None;
            report.picture = None;
        } else if !report.timeline.is_empty() {
            let half = report.timeline.len() / 2;
            report.timeline.drain(..half.max(1));
        } else {
            return value;
        }
    }
}

/// Sends every kept report not yet sent, one after another. Blocking: run it on a thread of its own.
pub fn unsent(api: &orbiters_core::account::Api, kept: &[Kept]) -> Outcome {
    let mut out = Outcome::default();
    for k in kept.iter().filter(|k| !k.sent) {
        let report: Report = match std::fs::read(k.dir.join(REPORT)).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string())) {
            Ok(r) => r,
            Err(e) => {
                out.failed += 1;
                out.why = Some(format!("{}: {e}", k.dir.display()));
                continue;
            }
        };
        let picture = report.picture.as_ref().and_then(|_| std::fs::read(k.dir.join(PICTURE)).ok());
        let answer = api
            .agent()
            .post(&api.url("n4n0/menu-reports"))
            .config()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .send_json(&body(report, picture));
        match answer {
            Ok(r) if r.status().is_success() => {
                let _ = std::fs::write(k.dir.join(SENT), super::iso(super::now_ms()));
                out.sent += 1;
            }
            Ok(r) => {
                let status = r.status().as_u16();
                out.failed += 1;
                out.refused |= status == 404 || status == 405;
                out.why = Some(format!("Orbiters answered {status}"));
                if out.refused {
                    break;
                }
            }
            Err(e) => {
                out.failed += 1;
                out.why = Some(format!("Orbiters not reached: {e}"));
                break;
            }
        }
    }
    if let Some(why) = &out.why {
        eprintln!("menu mismatch reports: {} sent, {} not: {why}", out.sent, out.failed); // i18n: not shown (a log's line)
    }
    out
}
