//! Feature suggestions sent to Orbiters from N4N0's menu (Suggest features): typed on the menu's keyboard or said to
//! N4N0, they reach the Orbiters team in the admin's N4N0 tab. Sent as the signed-in account.

use orbiters_core::account::{Api, ApiError};
use orbiters_i18n::tr;
use std::time::Duration;

/// How long a request waits for Orbiters.
const TIMEOUT: Duration = Duration::from_secs(15);
/// The longest suggestion Orbiters takes, in characters.
pub const TEXT_MAX: usize = 2000;

/// What is sent with a suggestion besides its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    /// Where it was written: `menu` (N4N0's menu, on the avatar's screen or in the overlay), `desktop` (the window),
    /// `android`.
    pub source: &'static str,
    /// The app's version, and the avatar's N4N0's (its release in My Avatar's gallery), when known.
    pub app_version: String,
    pub n4n0_version: Option<String>,
    /// Some of it was said to N4N0 rather than typed.
    pub voice: bool,
}

/// Sends `text` to Orbiters as the account of `token`.
pub fn send(api: &Api, token: &str, text: &str, about: &About) -> anyhow::Result<()> {
    let text = text.trim();
    anyhow::ensure!(!text.is_empty(), "{}", tr("Write or say your idea first."));
    let text: String = text.chars().take(TEXT_MAX).collect();
    let body = serde_json::json!({
        "text": text,
        "source": about.source,
        "app_version": about.app_version,
        "n4n0_version": about.n4n0_version,
        "voice": about.voice,
    });
    let response = api
        .agent()
        .post(&api.url("n4n0/suggestions"))
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(body)?;
    let _: serde_json::Value = Api::json(response).map_err(plain)?;
    Ok(())
}

/// An error answer in the person's words: the server's own message, or its status.
fn plain(e: anyhow::Error) -> anyhow::Error {
    match e.downcast::<ApiError>() {
        Ok(api) if api.message.is_empty() => anyhow::anyhow!("{}", tr!("Orbiters answered {status}", status = api.status)),
        Ok(api) => api.into(),
        Err(e) => e,
    }
}
