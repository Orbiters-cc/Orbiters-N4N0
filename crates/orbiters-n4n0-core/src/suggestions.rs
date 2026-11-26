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
    /// Its own id, the same each time it is sent ([`Unconfirmed`]).
    pub client_id: String,
}

/// The ids of what was composed and is not confirmed sent: the same words sent again (the answer was lost, the network was
/// gone) go with the id of the first time, and Orbiters answers as it did then instead of keeping a second suggestion
/// (EXT-21). In this process only, as the text itself.
#[derive(Debug, Default)]
pub struct Unconfirmed(Vec<(u64, String, String)>);

impl Unconfirmed {
    /// How many are remembered: what was given up on long ago is another suggestion.
    const MOST: usize = 16;

    /// The id `text` goes with, of the member of `epoch`.
    pub fn id(&mut self, epoch: u64, text: &str) -> String {
        let text = said(text);
        if let Some((.., id)) = self.0.iter().find(|(e, t, _)| *e == epoch && *t == text) {
            return id.clone();
        }
        let id = uuid::Uuid::new_v4().to_string();
        if self.0.len() >= Self::MOST {
            self.0.remove(0);
        }
        self.0.push((epoch, text, id.clone()));
        id
    }

    /// Orbiters has it: the same words said again are another suggestion.
    pub fn sent(&mut self, id: &str) {
        self.0.retain(|(.., i)| i != id);
    }
}

/// A suggestion as it goes out.
fn said(text: &str) -> String {
    text.trim().chars().take(TEXT_MAX).collect()
}

/// Sends `text` to Orbiters as the account of `token`.
pub fn send(api: &Api, token: &str, text: &str, about: &About) -> anyhow::Result<()> {
    let text = said(text);
    anyhow::ensure!(!text.is_empty(), "{}", tr("Write or say your idea first."));
    let body = serde_json::json!({
        "text": text,
        "source": about.source,
        "app_version": about.app_version,
        "n4n0_version": about.n4n0_version,
        "voice": about.voice,
        "client_id": about.client_id,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    /// EXT-21: what is sent again after an answer was lost says it is the same one; edited, or said again once it was
    /// received, it is another.
    #[test]
    fn a_suggestion_sent_again_has_the_id_of_the_first_time() {
        let mut ids = Unconfirmed::default();
        let first = ids.id(1, "A bigger menu");
        assert!((8..=64).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "{first}: what Orbiters takes");
        assert_eq!(ids.id(1, "  A bigger menu "), first, "not confirmed: the same one");
        assert_ne!(ids.id(1, "A bigger menu!"), first, "edited: another");
        assert_ne!(ids.id(2, "A bigger menu"), first, "another member's: another");
        ids.sent(&first);
        assert_ne!(ids.id(1, "A bigger menu"), first, "received, then said again: another");
        for n in 0..100 {
            ids.id(1, &n.to_string());
        }
        assert!(ids.0.len() <= Unconfirmed::MOST);
    }

    #[test]
    fn the_id_goes_with_the_suggestion() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}/", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (mut got, mut part) = (Vec::new(), [0; 4096]);
            while !String::from_utf8_lossy(&got).trim_end().ends_with('}') {
                let n = stream.read(&mut part).unwrap();
                assert!(n > 0, "{}", String::from_utf8_lossy(&got));
                got.extend_from_slice(&part[..n]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        let about = About { source: "menu", client_id: "11111111-2222-4333-8444-555555555555".into(), ..Default::default() };
        // (A test's address is no Orbiters server: without a credential of one.)
        send(&api, "t", "A bigger menu", &about).unwrap();
        let body = server.join().unwrap();
        let sent: serde_json::Value = serde_json::from_str(&body[body.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(sent["client_id"], "11111111-2222-4333-8444-555555555555", "{body}");
    }
}
