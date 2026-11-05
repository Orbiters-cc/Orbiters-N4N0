//! Signing in to Orbiters the way the desktop does (`orbiters_core::account`): the app listens on the phone's loopback
//! ([`Callback`]), asks Orbiters for a link naming where it listens, as the app's tool, and opens it in the browser;
//! the person signs in on the website and clicks Connect, and the browser comes back to
//! `http://127.0.0.1:<port>/<path>?code=…`, which the link's secret trades for the credential. The app keeps the listener
//! alive while the browser is in front: Android's foreground service does, on iOS the browser is a sheet inside the app.

use anyhow::bail;
use orbiters_core::account::{Api, Link, Server};

/// How long a sign-in link waits for Connect (the server's own lifetime).
pub const LINK_LIFETIME: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Asks Orbiters for a sign-in link whose Connect brings its code back to `port` and `path` on this phone, naming the
/// phone `device` on the website. `client` is the tool name Orbiters knows the app by (the server's editor-link clients).
pub fn begin_link(api: &Api, client: &str, device: &str, port: u16, path: &str) -> anyhow::Result<Link> {
    let body = serde_json::json!({ "client": client, "device": device, "port": port, "path": path });
    let link: Link = Api::json(api.agent().post(&api.url("editor-links")).send_json(body)?)?;
    if link.link_url.is_empty() || link.connected_url.is_empty() || link.exchange_token.is_empty() {
        bail!("Orbiters did not return a sign-in link.");
    }
    Ok(link)
}

/// The link and its connected page are pages of `server`'s website (the browser is sent nowhere else).
pub fn check_link(server: Server, link: &Link) -> anyhow::Result<()> {
    if !(server.is_website_page(&link.link_url) && server.is_website_page(&link.connected_url)) {
        bail!("Orbiters answered with a sign-in link that is not on its website.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A stand-in for Orbiters that answers one request with `answer` and hands back what it was sent.
    fn stand_in(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let response =
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (base, handle)
    }

    #[test]
    fn asks_as_the_app_s_tool() {
        let (base, server) =
            stand_in(r#"{"linkUrl":"https://orbiters.cc/link-editor?token=t","connectedUrl":"https://orbiters.cc/editor-connected","exchangeToken":"x"}"#);
        let link = begin_link(&Api::at(&base), "android", "Pixel 9", 40123, "abcdefabcdefabcdef").unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /editor-links "), "{request}");
        let body: serde_json::Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body, serde_json::json!({ "client": "android", "device": "Pixel 9", "port": 40123, "path": "abcdefabcdefabcdef" }));
        assert_eq!(link.exchange_token, "x");
        check_link(Server::Production, &link).unwrap();
    }

    #[test]
    fn a_link_elsewhere_is_refused() {
        let (base, server) = stand_in(r#"{"linkUrl":"https://evil.example/x","connectedUrl":"https://orbiters.cc/c","exchangeToken":"x"}"#);
        let link = begin_link(&Api::at(&base), "ios", "Phone", 40000, "abcdefabcdefabcdef").unwrap();
        server.join().unwrap();
        assert!(check_link(Server::Production, &link).is_err());
        let (base, server) = stand_in(r#"{"linkUrl":"","connectedUrl":"","exchangeToken":""}"#);
        assert!(begin_link(&Api::at(&base), "ios", "Phone", 40000, "abcdefabcdefabcdef").is_err());
        server.join().unwrap();
    }
}
