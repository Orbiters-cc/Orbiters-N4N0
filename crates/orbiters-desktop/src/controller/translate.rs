//! Orbiters' translation service for live translation in VR: the overlay's batches of lines go to
//! `POST n4n0/assistant/translate` (contracts `Docs/n4n0-translate.md`) with the account's credential, which only this
//! service holds. Only text goes, never pictures; and only after the person chose Orbiters and accepted that the text
//! seen in VR is sent to Orbiters (`TranslateSettings::orbiters_agreed`, checked here too).

use super::Shared;
use orbiters_translate::provider::{Batch, Failure};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Why Orbiters did not translate a batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refused {
    Unreachable(String),
    BadAnswer(String),
    /// Today's allowance is used up.
    Quota,
    /// Too many requests this minute.
    Busy,
    SignedOut,
    /// The person has not accepted that the text goes to Orbiters.
    NotAgreed,
}

pub type Answer = Result<Vec<String>, Refused>;

impl From<Refused> for Failure {
    fn from(r: Refused) -> Self {
        match r {
            Refused::Unreachable(d) => Failure::Unreachable(d),
            Refused::BadAnswer(d) => Failure::BadAnswer(d),
            Refused::Quota => Failure::Quota,
            Refused::Busy => Failure::Busy,
            Refused::SignedOut | Refused::NotAgreed => Failure::SignedOut,
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
struct Translated {
    translations: Vec<String>,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: String,
}

/// What the server's answer means, from its status and body.
pub fn read_answer(status: u16, body: &str, count: usize) -> Answer {
    match status {
        200..=299 => {
            let t: Translated = serde_json::from_str(body).map_err(|e| Refused::BadAnswer(e.to_string()))?;
            if t.translations.len() != count {
                return Err(Refused::BadAnswer(format!("{} translations for {count} lines", t.translations.len()))); // i18n: not shown (log detail)
            }
            Ok(t.translations)
        }
        401 | 403 => Err(Refused::SignedOut),
        429 => {
            let code = serde_json::from_str::<ErrorBody>(body).map(|b| b.code).unwrap_or_default();
            Err(if code == "quota_exhausted" { Refused::Quota } else { Refused::Busy })
        }
        _ => Err(Refused::BadAnswer(format!("HTTP {status}"))),
    }
}

impl super::Controller {
    pub fn set_translate_status(self: &Shared, status: crate::state::TranslateStatus) {
        self.update(|s| {
            s.translate.set(status);
        });
    }

    /// A batch translated by Orbiters (blocking: called on a thread of its own).
    pub fn translate_remote(self: &Shared, batch: &Batch) -> Answer {
        let settings = self.view().settings.translate.clone();
        if !settings.orbiters_agreed || settings.provider != orbiters_core::settings::TranslateProvider::Orbiters {
            return Err(Refused::NotAgreed);
        }
        let Some((api, token)) = self.account_api() else { return Err(Refused::SignedOut) };
        call(&api, &token, batch)
    }
}

/// One batch to Orbiters' route with `token` (the account's credential).
pub fn call(api: &orbiters_core::account::Api, token: &str, batch: &Batch) -> Answer {
    {
        let body = serde_json::json!({ "target": batch.target, "lines": batch.lines });
        let response = api
            .agent()
            .post(&api.url("n4n0/assistant/translate"))
            .config()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(&body);
        let mut response = response.map_err(|e| Refused::Unreachable(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.body_mut().with_config().limit(1 << 20).read_to_string().unwrap_or_default();
        read_answer(status, &text, batch.lines.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End to end against the development server on this computer (127.0.0.1:4100), with the development account the
    /// Unity tools and the app share (`MCB/account_dev.dat`, read, never written). Run by hand:
    /// `cargo test -p orbiters-desktop --bin orbiters-desktop -- controller::translate --ignored --nocapture`.
    #[test]
    #[ignore]
    fn orbiters_translates_a_batch_on_the_development_server() {
        use orbiters_core::account::{read, Api, Server};
        use orbiters_translate::provider::Item;
        let account = read(Server::Development).expect("a development account signed in on this computer");
        let batch = Batch { target: "en".into(), lines: vec![Item { text: "西口".into(), source: Some("ja".into()) }, Item { text: "北口".into(), source: Some("ja".into()) }, Item { text: "関係者以外立入禁止".into(), source: Some("ja".into()) }] };
        let t = std::time::Instant::now();
        let answer = call(&Api::new(Server::Development), &account.token, &batch);
        println!("{:.0} ms: {answer:?}", t.elapsed().as_secs_f64() * 1000.0);
        let translations = answer.expect("translated");
        assert_eq!(translations.len(), 3);
        assert!(translations[0].to_lowercase().contains("west"), "{translations:?}");
    }

    #[test]
    fn the_servers_answers_are_told_apart() {
        assert_eq!(read_answer(200, r#"{"translations":["Exit","Welcome"]}"#, 2), Ok(vec!["Exit".to_string(), "Welcome".to_string()]));
        assert!(matches!(read_answer(200, r#"{"translations":["Exit"]}"#, 2), Err(Refused::BadAnswer(_))), "a line left out");
        assert_eq!(read_answer(429, r#"{"code":"quota_exhausted"}"#, 1), Err(Refused::Quota));
        assert_eq!(read_answer(429, r#"{"code":"rate_limited"}"#, 1), Err(Refused::Busy));
        assert_eq!(read_answer(401, "", 1), Err(Refused::SignedOut));
    }
}
