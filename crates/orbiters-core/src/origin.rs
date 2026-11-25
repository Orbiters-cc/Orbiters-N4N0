//! A credential goes to the origin it was issued by and to no other (verify3-r21 B2).
//!
//! Every Orbiters credential this process learns (read from an account file, handed out by a sign-in, confirmed by a
//! check) is noted here with the origin it belongs to ([`issued`]), and every HTTP client that may carry one refuses, in
//! the client itself and before anything is sent, a request whose bearer credential is not its destination's
//! ([`guard`]): whatever the callers paired. A credential nobody noted reaches no Orbiters environment at all.

use crate::account::Server;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// The origins each credential was issued by (a digest of it).
static ISSUED: Mutex<Option<HashMap<[u8; 32], HashSet<String>>>> = Mutex::new(None);

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.trim().as_bytes()).into()
}

/// `https://host:port` of `url` (its default port written out), `None` for what is not an http(s) address.
pub fn of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.to_ascii_lowercase();
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(':') && port.bytes().all(|c| c.is_ascii_digit()) && !port.is_empty() => (host.to_string(), port.parse::<u16>().ok()?),
        _ => (authority, default),
    };
    (!host.is_empty()).then(|| format!("{scheme}://{host}:{port}"))
}

/// `token` was issued by the server at `base`: it goes there only.
pub fn issued(token: &str, base: &str) {
    let (Some(origin), false) = (of(base), token.trim().is_empty()) else { return };
    ISSUED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().entry(digest(token)).or_default().insert(origin);
}

/// The server at `base` gave or confirmed `token`: it is that environment's. (A stand-in's made-up ones belong to no
/// environment, and reach none.)
pub fn answered(token: &str, base: &str) {
    if of(base).is_some_and(|origin| is_environment(&origin)) {
        issued(token, base);
    }
}

/// Whether `origin` is one of Orbiters' environments.
fn is_environment(origin: &str) -> bool {
    [Server::Production, Server::Development].iter().any(|s| of(s.api()).as_deref() == Some(origin))
}

/// Whether a request to `url` may carry `token`.
pub fn allowed(token: &str, url: &str) -> bool {
    let Some(origin) = of(url) else { return false };
    match ISSUED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|all| all.get(&digest(token))) {
        Some(origins) => origins.contains(&origin),
        // Of unknown origin: never to an environment of Orbiters (a stand-in's own address takes its made-up ones).
        None => !is_environment(&origin),
    }
}

/// Why a request was not sent.
#[derive(Debug)]
pub struct OtherOrigin {
    /// Where it would have gone.
    pub origin: String,
}

impl std::fmt::Display for OtherOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(orbiters_i18n::tr("This Orbiters account belongs to another environment: nothing was sent."))
    }
}

impl std::error::Error for OtherOrigin {}

/// The HTTP clients' middleware: a request whose bearer credential is not its destination's is refused here, before a
/// connection is made.
pub fn guard(request: ureq::http::Request<ureq::SendBody>, next: ureq::middleware::MiddlewareNext) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let url = request.uri().to_string();
    for value in request.headers().get_all(ureq::http::header::AUTHORIZATION) {
        let Ok(value) = value.to_str() else { continue };
        let Some((scheme, token)) = value.split_once(' ') else { continue };
        if scheme.eq_ignore_ascii_case("bearer") && !allowed(token, &url) {
            return Err(ureq::Error::Other(Box::new(OtherOrigin { origin: of(&url).unwrap_or_default() })));
        }
    }
    next.handle(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Api;
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// A server that counts the bytes it gets and answers nothing.
    fn counting() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let got = Arc::new(AtomicUsize::new(0));
        let seen = got.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let seen = seen.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = stream.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.fetch_add(n, Ordering::SeqCst);
                    }
                });
            }
        });
        (base, got)
    }

    #[test]
    fn origins_are_compared_whole() {
        assert_eq!(of("https://api.orbiters.cc/x?y"), Some("https://api.orbiters.cc:443".into()));
        assert_eq!(of("HTTPS://API.Orbiters.cc:443"), Some("https://api.orbiters.cc:443".into()));
        assert_eq!(of("http://127.0.0.1:4100/a"), Some("http://127.0.0.1:4100".into()));
        assert_eq!(of("https://orbiters.cc@evil.example/"), Some("https://evil.example:443".into()));
        assert_eq!(of("http://[::1]:80/"), Some("http://[::1]:80".into()));
        for no in ["file:///x", "orbiters.cc", "ftp://a/", "https:///x"] {
            assert_eq!(of(no), None, "{no}");
        }
        assert_ne!(of("http://127.0.0.1:4100"), of("http://127.0.0.1:4101"));
        assert_ne!(of("http://api.orbiters.cc:443"), of("https://api.orbiters.cc:443"));
    }

    /// Every client that may carry a credential: not one byte leaves for another origin, whatever the request.
    #[test]
    fn not_a_byte_of_a_credential_leaves_for_another_origin() {
        let (first, at_first) = counting();
        let (second, at_second) = counting();
        issued("orbit-of-first", &first);
        let api = Api::at(&second);
        assert!(api.check("orbit-of-first").is_err());
        assert!(api.member_feature("orbit-of-first", "x").is_err());
        assert!(api.set_language("orbit-of-first", Some("fr")).is_err());
        let short = Some(Duration::from_millis(400));
        let auth = "Bearer orbit-of-first";
        for agent in [api.agent().clone(), crate::http_agent(short, short)] {
            assert!(agent.get(&second).header("Authorization", auth).call().is_err());
            assert!(agent.get(&second).header("authorization", "bearer  orbit-of-first ").call().is_err());
            assert!(agent.post(&second).header("Authorization", auth).send_json(serde_json::json!({ "a": 1 })).is_err());
            // A request with timeouts of its own is the same client's.
            assert!(agent.delete(&second).config().timeout_global(short).build().header("Authorization", auth).call().is_err());
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(at_second.load(Ordering::SeqCst), 0, "bytes left for another origin");
        // Its own origin gets it.
        let _ = Api::at(&first).agent().get(&first).config().timeout_global(short).build().header("Authorization", auth).call();
        assert!(at_first.load(Ordering::SeqCst) > 0);
        // The refusal says so.
        let refused = api.agent().get(&second).header("Authorization", auth).call().unwrap_err();
        assert!(matches!(&refused, ureq::Error::Other(e) if e.downcast_ref::<OtherOrigin>().is_some()), "{refused:?}");
    }

    /// A credential nobody noted goes to no environment; noted for one, not to the other.
    #[test]
    fn an_environment_gets_its_own_credentials_only() {
        for server in [Server::Production, Server::Development] {
            assert!(!allowed("orbit-nobody-knows", &format!("{}n4n0/assistant", server.api())));
        }
        issued("orbit-prod-only", Server::Production.api());
        assert!(allowed("orbit-prod-only", "https://api.orbiters.cc/unity-wizard/token"));
        assert!(!allowed("orbit-prod-only", "http://127.0.0.1:4100/unity-wizard/token"));
        assert!(!allowed("orbit-prod-only", "https://api.orbiters.cc.evil.example/"));
        assert!(!allowed("orbit-prod-only", "http://api.orbiters.cc/"));
    }

    /// Every HTTP client of the app that could carry a credential is guarded: a client is configured in these files only,
    /// and each that sends an Authorization header has the guard.
    #[test]
    fn every_client_that_may_carry_a_credential_is_guarded() {
        // Clients of other servers, which send no Orbiters credential (the speech models', updates, the translator's,
        // VRChat's own API, which has its own sign-in and hosts).
        const OTHERS: &[&str] = &["orbiters-voice/src/main.rs", "orbiters-update/src/net.rs", "orbiters-translate/src/provider.rs", "orbiters-core/src/vrchat_api.rs", "orbiters-core/src/origin.rs"];
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut wrong = Vec::new();
        let mut agents = 0;
        let folders: Vec<_> = std::fs::read_dir(&crates).unwrap().flatten().map(|c| c.path().join("src")).collect();
        for f in folders.iter().flat_map(|src| crate::source::files(src)) {
            let name = f.path.to_string_lossy().replace('\\', "/");
            // All of the app's code in it: tests' own clients, wherever they are, are not the app's.
            let app = f.text.as_str();
            if OTHERS.iter().any(|o| name.ends_with(o)) {
                assert!(!app.contains("\"Authorization\", &format!(\"Bearer") || name.ends_with("vrchat_api.rs"), "{name}");
                continue;
            }
            for (n, _) in app.match_indices("Agent::config_builder()") {
                let statement = app[n..].split(';').next().unwrap_or_default();
                agents += 1;
                if !statement.contains("origin::guard") {
                    wrong.push(format!("{name}: {}", statement.chars().take(120).collect::<String>()));
                }
            }
            for other in ["Agent::new_with_defaults", "Agent::new_with_config", "ureq::get(", "ureq::post(", "ureq::put(", "ureq::delete(", "ureq::patch(", "ureq::head(", "ureq::agent(", "ureq::run("] {
                if app.contains(other) {
                    wrong.push(format!("{name}: {other}"));
                }
            }
        }
        assert!(agents >= 3, "the app's clients are found: {agents}");
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}
