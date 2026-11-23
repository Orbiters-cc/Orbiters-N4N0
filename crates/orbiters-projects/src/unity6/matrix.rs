//! VRChat's release matrix: which SDK goes with which exact Unity editor and changeset, on which channel, from which
//! sources, checked when; and a short list of publisher statements and reported issues about packages on Unity 6.
//!
//! The app ships a reviewed copy (`data/unity6-data.json`, the one actions use). Orbiters' server may serve a newer one,
//! which is **display only**: it is accepted when it is signed with Orbiters' release key, at most [`MAX_BYTES`] long,
//! well formed and not older than the copy kept (no rollback). The last copy that passed is kept with the time it was
//! verified, and its age is shown. A tuple only a served copy names can be shown, never acted on ([`Matrix::actionable`]):
//! install links, the trial's target and the editor chosen come from the rows built into the app.

use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use vrc_get_vpm::version::{PrereleaseAcceptance, UnityVersion, Version, VersionRange};

/// The copy reviewed and shipped with this version of the app.
pub const BUILT_IN: &str = include_str!("../../data/unity6-data.json");
/// The largest served copy read; anything longer is refused unread.
pub const MAX_BYTES: usize = 64 * 1024;
/// Where on Orbiters' API the served copy is (`GET`), signed as [`Envelope`].
pub const SERVED_PATH: &str = "desktop/unity6-data.json";
/// After this long without a verified copy, the list says it may be out of date.
pub const STALE_SECS: u64 = 14 * 24 * 3600;
/// How often the window asks the server again.
pub const REFRESH_SECS: u64 = 12 * 3600;
/// The file in app data holding the last served copy that passed every check.
pub const KEPT_FILE: &str = "unity6-data.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
}

/// What installing an editor takes, from Unity's release record: the base editor, then Android (with its SDK, NDK and
/// JDK) and iOS, each `[download, installed]` in bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSize {
    pub download: u64,
    pub installed: u64,
    #[serde(default)]
    pub android: Option<[u64; 2]>,
    #[serde(default)]
    pub ios: Option<[u64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// One SDK version (`3.11.0-beta.1`) or a VPM range of them (`>=3.5.0 <3.11.0-0`).
    pub sdk: String,
    pub channel: Channel,
    /// The exact editor (`6000.0.67f1`) and its changeset.
    pub unity: String,
    pub changeset: String,
    /// The Creator Companion that migrates to it, when one is needed.
    #[serde(default)]
    pub vcc: Option<String>,
    /// The day the row was checked against its sources (`YYYY-MM-DD`).
    pub checked: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub notes: String,
    /// By platform: `windows-x86_64`, `windows-arm64`, `macos-arm64`, `macos-x86_64`.
    #[serde(default)]
    pub editor: BTreeMap<String, EditorSize>,
}

impl Row {
    /// The editor's line, `6000.0`.
    pub fn line(&self) -> String {
        UnityVersion::parse(&self.unity).map(|v| format!("{}.{}", v.major(), v.minor())).unwrap_or_default()
    }

    fn admits(&self, version: &Version) -> bool {
        match self.sdk.parse::<Version>() {
            Ok(exact) => &exact == version,
            Err(_) => self.sdk.parse::<VersionRange>().is_ok_and(|r| r.match_pre(version, PrereleaseAcceptance::Allow)),
        }
    }

    /// Unity Hub's link installing exactly this editor.
    pub fn hub_link(&self) -> String {
        format!("unityhub://{}/{}", self.unity, self.changeset)
    }

    /// What installing the editor takes on this computer, when the row knows.
    pub fn size_here(&self) -> Option<&EditorSize> {
        self.editor.get(platform())
    }
}

/// This computer's platform as the rows name it.
pub fn platform() -> &'static str {
    match (cfg!(target_os = "macos"), cfg!(target_arch = "aarch64")) {
        (true, true) => "macos-arm64",
        (true, false) => "macos-x86_64",
        (false, true) => "windows-arm64",
        (false, false) => "windows-x86_64",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueKind {
    /// What the package's publisher says about it.
    Publisher,
    /// What someone reported, not confirmed by the publisher.
    Report,
}

/// A dated statement about a package on Unity 6, shown beside it. It never selects a package, version, repository or
/// action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub package: String,
    /// The versions it is about, a VPM range (`*` for all).
    pub versions: String,
    pub kind: IssueKind,
    /// By language tag; `en` always.
    pub text: BTreeMap<String, String>,
    /// An `https://` page that says it.
    pub link: String,
    pub date: String,
}

impl Issue {
    /// Its text in the language shown, else in English.
    pub fn text(&self) -> &str {
        let tag = orbiters_i18n::language().tag();
        self.text.get(tag).or_else(|| self.text.get("en")).map(String::as_str).unwrap_or_default()
    }

    pub fn applies(&self, version: Option<&str>) -> bool {
        if self.versions.trim() == "*" {
            return true;
        }
        let (Ok(range), Some(Ok(v))) = (self.versions.parse::<VersionRange>(), version.map(str::parse::<Version>)) else { return false };
        range.match_pre(&v, PrereleaseAcceptance::Allow)
    }
}

/// Dated facts kept apart from version numbers: neither is ever inferred from a release.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Events {
    /// When VRChat made its Unity 6 SDK a release (`YYYY-MM-DD`); none announced.
    #[serde(default)]
    pub unity6_stable: Option<String>,
    /// When VRChat stops accepting uploads from Unity 2022; none announced.
    #[serde(default)]
    pub unity2022_retirement: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Data {
    pub schema: u32,
    /// Grows with each copy published: a copy with a smaller one is an older one served again.
    pub serial: u64,
    pub verified: String,
    pub rows: Vec<Row>,
    #[serde(default)]
    pub events: Events,
    #[serde(default)]
    pub issues: Vec<Issue>,
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

impl Data {
    /// Every field the app relies on, checked; the first problem said.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.schema == 1, "{}", tr!("the release list is of format {schema}, which this version of Orbiters does not read", schema = self.schema));
        anyhow::ensure!(self.serial > 0 && is_date(&self.verified) && !self.rows.is_empty() && self.rows.len() <= 64 && self.issues.len() <= 256, "{}", tr("the release list is incomplete"));
        for r in &self.rows {
            let sdk_ok = r.sdk.parse::<Version>().is_ok() || r.sdk.parse::<VersionRange>().is_ok();
            let changeset_ok = r.changeset.len() == 12 && r.changeset.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
            anyhow::ensure!(
                sdk_ok && UnityVersion::parse(&r.unity).is_some() && changeset_ok && is_date(&r.checked) && r.vcc.as_deref().is_none_or(|v| v.parse::<Version>().is_ok()),
                "{}",
                tr!("the release list's row for SDK {sdk} is not well formed", sdk = r.sdk)
            );
        }
        for i in &self.issues {
            let link_ok = url::Url::parse(&i.link).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some() && u.username().is_empty());
            let range_ok = i.versions.trim() == "*" || i.versions.parse::<VersionRange>().is_ok();
            let text_ok = i.text.get("en").is_some_and(|t| !t.trim().is_empty()) && i.text.values().all(|t| t.chars().count() <= 400);
            anyhow::ensure!(
                link_ok && range_ok && text_ok && is_date(&i.date) && !i.package.is_empty() && i.package.len() <= 214,
                "{}",
                tr!("the release list's note about {package} is not well formed", package = i.package)
            );
        }
        Ok(())
    }
}

/// Where the list shown came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The copy built into this version of the app.
    BuiltIn,
    /// A copy served by Orbiters and verified at `verified` (seconds since 1970).
    Served { verified: u64 },
}

#[derive(Clone, Debug)]
pub struct Matrix {
    pub data: Data,
    pub origin: Origin,
    built_in: Data,
}

impl Matrix {
    /// The list built into the app.
    pub fn built_in() -> Self {
        let data: Data = serde_json::from_str(BUILT_IN).expect("data/unity6-data.json is well formed"); // i18n: not shown (a test of the shipped file catches it)
        Self { built_in: data.clone(), data, origin: Origin::BuiltIn }
    }

    /// The row of an SDK version: its own row, else the row whose range holds it.
    pub fn row_for(&self, sdk: &str) -> Option<&Row> {
        let v = sdk.parse::<Version>().ok()?;
        self.data.rows.iter().find(|r| r.sdk.parse::<Version>().is_ok_and(|e| e == v)).or_else(|| self.data.rows.iter().find(|r| r.admits(&v)))
    }

    /// Whether `row` may drive an action: it is one of the rows built into the app, exactly.
    pub fn actionable(&self, row: &Row) -> bool {
        self.built_in.rows.iter().any(|b| b.sdk == row.sdk && b.unity == row.unity && b.changeset == row.changeset && b.channel == row.channel)
    }

    /// The built-in row with this exact editor, if one has it.
    pub fn row_of_editor(&self, unity: &str) -> Option<&Row> {
        self.built_in.rows.iter().find(|r| r.unity == unity)
    }

    /// The Unity 6 row a trial targets: the newest built-in row on the 6000 line.
    pub fn unity6_target(&self) -> Option<&Row> {
        self.built_in.rows.iter().filter(|r| r.line().starts_with("6000.")).max_by_key(|r| UnityVersion::parse(&r.unity))
    }

    /// The supported 2022 editor, from the built-in rows.
    pub fn stable_2022(&self) -> Option<&Row> {
        self.built_in.rows.iter().find(|r| r.line() == "2022.3" && r.channel == Channel::Stable)
    }

    /// Rows a served copy names that the app does not: shown as news, never acted on.
    pub fn display_only_rows(&self) -> Vec<&Row> {
        self.data.rows.iter().filter(|r| !self.actionable(r)).collect()
    }

    pub fn issues_for(&self, package: &str, version: Option<&str>) -> Vec<&Issue> {
        self.data.issues.iter().filter(|i| i.package == package && i.applies(version)).collect()
    }

    /// How long ago the list shown was verified (`None`: the built-in copy, dated by its `verified` day).
    pub fn age(&self, now: u64) -> Option<u64> {
        match self.origin {
            Origin::BuiltIn => None,
            Origin::Served { verified } => Some(now.saturating_sub(verified)),
        }
    }

    /// Whether the list shown may be out of date: a served copy not verified for [`STALE_SECS`], or the built-in copy
    /// once the app is that much older than its `verified` day.
    pub fn stale(&self, now: u64) -> bool {
        match self.origin {
            Origin::Served { verified } => now.saturating_sub(verified) > STALE_SECS,
            Origin::BuiltIn => day_seconds(&self.data.verified).is_some_and(|d| now.saturating_sub(d) > STALE_SECS),
        }
    }
}

/// Seconds since 1970 at the start of a `YYYY-MM-DD` day.
pub fn day_seconds(day: &str) -> Option<u64> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    let date = time::Date::parse(day, &format).ok()?;
    u64::try_from(date.midnight().assume_utc().unix_timestamp()).ok()
}

/// A served copy: the list's JSON, exactly the bytes signed, and the Ed25519 signature of them (Base64).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub data: String,
    pub signature: String,
}

/// The list in a served file, once every rule passes: no longer than [`MAX_BYTES`], signed with `key`, well formed.
pub fn open(bytes: &[u8], key: &[u8; 32]) -> anyhow::Result<Data> {
    use base64::Engine as _;
    anyhow::ensure!(bytes.len() <= MAX_BYTES, "{}", tr("the release list served is too long"));
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("{}", tr("the release list served is not a signed list")))?;
    let signature =
        base64::engine::general_purpose::STANDARD.decode(envelope.signature.trim()).map_err(|_| anyhow::anyhow!("{}", tr("the release list's signature is not Base64")))?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(envelope.data.as_bytes(), &signature)
        .map_err(|_| anyhow::anyhow!("{}", tr("the release list is not signed with Orbiters' key")))?;
    let data: Data = serde_json::from_str(&envelope.data).map_err(|_| anyhow::anyhow!("{}", tr("the release list cannot be read")))?;
    data.validate()?;
    Ok(data)
}

/// Signs a list for serving, with the release key's private half (PKCS#8): what `cargo xtask` would run before
/// publishing a new copy. The JSON is signed exactly as given.
pub fn sign(json: &str, private_key: &[u8]) -> anyhow::Result<String> {
    use base64::Engine as _;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(private_key).map_err(|e| anyhow::anyhow!("not an Ed25519 private key: {e}"))?; // i18n: not shown (release signing)
    let data: Data = serde_json::from_str(json)?;
    data.validate()?;
    let signature = base64::engine::general_purpose::STANDARD.encode(pair.sign(json.as_bytes()).as_ref());
    Ok(serde_json::to_string_pretty(&Envelope { data: json.to_string(), signature })?)
}

/// What is kept in app data: the served file as it came, and when it last passed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    verified: u64,
    envelope: String,
    /// The highest serial ever accepted here: a copy below it is a rollback, whatever the file holds now.
    #[serde(default)]
    high: u64,
}

/// The kept file, read only when it is no longer than a served copy can be (with its few fields around it).
fn read_kept(dir: &Path) -> Option<Kept> {
    let path = kept_path(dir);
    let len = std::fs::metadata(&path).ok()?.len();
    if len > 2 * MAX_BYTES as u64 + 1024 {
        return None;
    }
    serde_json::from_slice::<Kept>(&std::fs::read(path).ok()?).ok()
}

fn kept_path(dir: &Path) -> PathBuf {
    dir.join(KEPT_FILE)
}

/// The list to show: the kept served copy when it still passes every rule and is not older than the built-in one,
/// else the built-in one.
pub fn load(dir: &Path, key: Option<&[u8; 32]>) -> Matrix {
    let built = Matrix::built_in();
    let Some(key) = key else { return built };
    let Some(kept) = read_kept(dir) else { return built };
    match open(kept.envelope.as_bytes(), key) {
        Ok(data) if data.serial >= built.data.serial => Matrix { data, origin: Origin::Served { verified: kept.verified }, built_in: built.built_in },
        _ => built,
    }
}

/// How fetching the served copy went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// A newer copy, verified and kept.
    Newer(u64),
    /// The same copy again, verified again (its age starts over).
    Same,
}

/// What reads the served copy: Orbiters' API for real, a stand-in in tests. `cap` is the most it may return; a longer
/// answer is an error.
pub trait Fetch {
    fn get(&self, url: &str, cap: usize) -> anyhow::Result<Vec<u8>>;
}

/// Asks for the served copy and keeps it when it passes every rule: within [`MAX_BYTES`], signed with `key`, well
/// formed, and with a serial at least the kept copy's and the built-in one's. Anything else leaves the kept copy as it
/// was and says why.
pub fn refresh(dir: &Path, key: &[u8; 32], fetch: &dyn Fetch, url: &str, now: u64) -> anyhow::Result<Refresh> {
    let bytes = fetch.get(url, MAX_BYTES)?;
    let data = open(&bytes, key)?;
    let incoming: Envelope = serde_json::from_slice(&bytes)?;
    // Compared and kept under one lock across processes: two refreshes never pass each other (verification's major 9).
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("unity6-data.lock"))?;
    lock.lock()?;
    let kept = read_kept(dir);
    let current = load(dir, Some(key));
    let high = kept.as_ref().map_or(0, |k| k.high).max(current.data.serial);
    anyhow::ensure!(
        data.serial >= high,
        "{}",
        tr!("the release list served (number {served}) is older than the one kept (number {kept})", served = data.serial, kept = high)
    );
    let same = data.serial == current.data.serial;
    if same && current.origin != Origin::BuiltIn {
        let kept_data = kept.as_ref().and_then(|k| serde_json::from_str::<Envelope>(&k.envelope).ok()).map(|e| e.data);
        anyhow::ensure!(kept_data.as_deref() == Some(incoming.data.as_str()), "{}", tr("the release list served has the number of the one kept, with other contents"));
    }
    let envelope = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{}", tr("the release list cannot be read")))?;
    crate::write_json(&kept_path(dir), &Kept { verified: now, envelope, high: high.max(data.serial) })?;
    Ok(if same { Refresh::Same } else { Refresh::Newer(data.serial) })
}

/// Orbiters' API, through the shared agent; reads at most `cap + 1` bytes.
pub struct Api(pub orbiters_core::account::Server);

impl Fetch for Api {
    fn get(&self, url: &str, cap: usize) -> anyhow::Result<Vec<u8>> {
        use std::io::Read as _;
        let api = orbiters_core::account::Api::at(self.0.api());
        let response = api.agent().get(url).call()?;
        anyhow::ensure!(response.status().is_success(), "HTTP {}", response.status()); // i18n: not shown (a status code)
        let mut body = vec![];
        response.into_body().into_reader().take(cap as u64 + 1).read_to_end(&mut body)?;
        anyhow::ensure!(body.len() <= cap, "{}", tr("the release list served is too long"));
        Ok(body)
    }
}

/// The served copy's address on `server`.
pub fn served_url(server: orbiters_core::account::Server) -> String {
    orbiters_core::account::Api::at(server.api()).url(SERVED_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn key_pair() -> (ring::signature::Ed25519KeyPair, Vec<u8>, [u8; 32]) {
        use ring::signature::KeyPair as _;
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public: [u8; 32] = pair.public_key().as_ref().try_into().unwrap();
        (pair, pkcs8.as_ref().to_vec(), public)
    }

    /// The built-in list with another serial and a row a served copy adds.
    fn served(serial: u64, extra: bool) -> String {
        let mut v: serde_json::Value = serde_json::from_str(BUILT_IN).unwrap();
        v["serial"] = serial.into();
        if extra {
            let mut row = v["rows"][0].clone();
            row["sdk"] = "3.11.0-beta.2".into();
            row["unity"] = "6000.0.70f1".into();
            row["changeset"] = "0123456789ab".into();
            v["rows"].as_array_mut().unwrap().insert(0, row);
        }
        serde_json::to_string(&v).unwrap()
    }

    /// A stand-in for Orbiters' API: what it answers, and the addresses it was asked for.
    struct Fake {
        answer: RefCell<anyhow::Result<Vec<u8>>>,
        asked: RefCell<Vec<String>>,
    }
    impl Fake {
        fn answering(bytes: Vec<u8>) -> Self {
            Self { answer: RefCell::new(Ok(bytes)), asked: RefCell::default() }
        }
    }
    impl Fetch for Fake {
        fn get(&self, url: &str, cap: usize) -> anyhow::Result<Vec<u8>> {
            self.asked.borrow_mut().push(url.into());
            let answer = std::mem::replace(&mut *self.answer.borrow_mut(), Err(anyhow::anyhow!("asked twice")));
            let bytes = answer?;
            anyhow::ensure!(bytes.len() <= cap, "too long");
            Ok(bytes)
        }
    }

    #[test]
    fn the_shipped_list_is_well_formed_and_has_the_verified_beta_row() {
        let m = Matrix::built_in();
        m.data.validate().unwrap();
        let beta = m.row_for("3.11.0-beta.1").unwrap();
        assert_eq!((beta.unity.as_str(), beta.changeset.as_str(), beta.channel), ("6000.0.67f1", "78a1c2bbeb6a", Channel::Beta));
        assert_eq!(beta.hub_link(), "unityhub://6000.0.67f1/78a1c2bbeb6a");
        assert_eq!(m.unity6_target().map(|r| r.unity.as_str()), Some("6000.0.67f1"));
        // Every 2022 SDK from 3.5 on goes with 2022.3.22f1; a later beta than the one known is not guessed.
        for sdk in ["3.5.0", "3.10.6", "3.10.6-beta.4"] {
            assert_eq!(m.row_for(sdk).map(|r| r.unity.as_str()), Some("2022.3.22f1"), "{sdk}");
        }
        assert_eq!(m.row_for("3.11.0-beta.2"), None);
        assert_eq!(m.row_for("3.1.13"), None);
        assert!(m.data.events.unity6_stable.is_none() && m.data.events.unity2022_retirement.is_none(), "neither date is announced");
        let sizes = beta.editor.get("windows-x86_64").unwrap();
        assert_eq!((sizes.download, sizes.installed), (4_078_656_432, 7_771_104_225));
    }

    #[test]
    fn issues_are_display_text_in_every_language_and_match_their_versions() {
        let m = Matrix::built_in();
        assert_eq!(m.issues_for("com.poiyomi.toon", Some("10.0.20")).len(), 1);
        assert!(m.issues_for("com.poiyomi.toon", Some("9.2.67")).is_empty());
        assert!(m.issues_for("com.poiyomi.toon", None).is_empty(), "a range needs a version to apply");
        assert_eq!(m.issues_for("jp.lilxyzw.liltoon", None).len(), 1, "* applies to every version");
        for issue in &m.data.issues {
            for tag in ["en", "fr", "es", "ja", "pt-BR"] {
                assert!(issue.text.contains_key(tag), "{} has no {tag}", issue.package);
            }
        }
        let french = orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || m.data.issues[0].text().to_string());
        assert!(french.contains("prise en charge"), "{french}");
    }

    #[test]
    fn a_served_copy_is_kept_only_when_signed_capped_and_not_older() {
        let dir = tempfile::tempdir().unwrap();
        let (_, private, public) = key_pair();
        let url = "https://example.invalid/desktop/unity6-data.json";
        // Signed, newer: kept; shown with its age; its extra row is display only.
        let newer = sign(&served(2, true), &private).unwrap();
        assert_eq!(refresh(dir.path(), &public, &Fake::answering(newer.clone().into_bytes()), url, 1_000).unwrap(), Refresh::Newer(2));
        let m = load(dir.path(), Some(&public));
        assert_eq!((m.data.serial, m.origin.clone(), m.age(1_600)), (2, Origin::Served { verified: 1_000 }, Some(600)));
        let extra = m.row_for("3.11.0-beta.2").unwrap();
        assert!(!m.actionable(extra), "a tuple only the server names never drives an action");
        assert!(m.actionable(m.row_for("3.11.0-beta.1").unwrap()));
        assert_eq!(m.display_only_rows().len(), 1);
        assert_eq!(m.unity6_target().map(|r| r.unity.as_str()), Some("6000.0.67f1"), "the target stays the built-in row");

        // Each bad answer leaves the kept copy and its age as they were.
        let kept_before = std::fs::read(dir.path().join(KEPT_FILE)).unwrap();
        let (_, other_private, _) = key_pair();
        let mut tampered: Envelope = serde_json::from_str(&sign(&served(3, false), &private).unwrap()).unwrap();
        tampered.data = tampered.data.replace("78a1c2bbeb6a", "61dfb374e36f");
        let mut huge: serde_json::Value = serde_json::from_str(&served(4, false)).unwrap();
        huge["issues"][0]["text"]["en"] = "x".repeat(MAX_BYTES).into();
        let mut malformed: serde_json::Value = serde_json::from_str(&served(5, false)).unwrap();
        malformed["rows"][0]["changeset"] = "not-a-changeset".into();
        let mut http: serde_json::Value = serde_json::from_str(&served(6, false)).unwrap();
        http["issues"][0]["link"] = "http://example.com/".into();
        for (label, bytes) in [
            ("signed with another key", sign(&served(3, false), &other_private).unwrap().into_bytes()),
            ("tampered after signing", serde_json::to_vec(&tampered).unwrap()),
            ("over the cap", huge.to_string().into_bytes()),
            ("unsigned", served(3, false).into_bytes()),
            ("a rollback", sign(&served(1, false), &private).unwrap().into_bytes()),
        ] {
            assert!(refresh(dir.path(), &public, &Fake::answering(bytes), url, 2_000).is_err(), "{label} was accepted");
            assert_eq!(std::fs::read(dir.path().join(KEPT_FILE)).unwrap(), kept_before, "{label} changed the kept copy");
        }
        // Not signable either: the signer checks the list the same way.
        assert!(sign(&malformed.to_string(), &private).is_err());
        assert!(sign(&http.to_string(), &private).is_err());
        let m = load(dir.path(), Some(&public));
        assert_eq!((m.data.serial, m.age(3_000)), (2, Some(2_000)), "the last verified copy, with its true age");
        assert!(!m.stale(3_000) && m.stale(1_000 + STALE_SECS + 1), "older than two weeks: said to be maybe out of date");

        // The same copy again: verified again, its age starts over.
        assert_eq!(refresh(dir.path(), &public, &Fake::answering(newer.into_bytes()), url, 5_000).unwrap(), Refresh::Same);
        assert_eq!(load(dir.path(), Some(&public)).age(5_000), Some(0));

        // Verification major 9: the same number with other contents is refused.
        let mut other: serde_json::Value = serde_json::from_str(&served(2, true)).unwrap();
        other["verified"] = "2026-11-30".into();
        assert!(refresh(dir.path(), &public, &Fake::answering(sign(&other.to_string(), &private).unwrap().into_bytes()), url, 5_500).is_err());
        // A newer one, then the kept file put back to the older one: the older one is still a rollback.
        let older_kept = std::fs::read(dir.path().join(KEPT_FILE)).unwrap();
        refresh(dir.path(), &public, &Fake::answering(sign(&served(7, false), &private).unwrap().into_bytes()), url, 5_600).unwrap();
        let mut back: serde_json::Value = serde_json::from_slice(&older_kept).unwrap();
        back["high"] = 7.into();
        std::fs::write(dir.path().join(KEPT_FILE), back.to_string()).unwrap();
        assert!(refresh(dir.path(), &public, &Fake::answering(sign(&served(3, false), &private).unwrap().into_bytes()), url, 5_700).is_err());
        std::fs::write(dir.path().join(KEPT_FILE), &older_kept).unwrap();

        // A network failure: the kept copy stays.
        let failing = Fake { answer: RefCell::new(Err(anyhow::anyhow!("offline"))), asked: RefCell::default() };
        assert!(refresh(dir.path(), &public, &failing, url, 6_000).is_err());
        assert_eq!(failing.asked.borrow().as_slice(), [url]);
        assert_eq!(load(dir.path(), Some(&public)).data.serial, 2);
    }

    #[test]
    fn a_kept_copy_that_no_longer_passes_falls_back_to_the_built_in_list() {
        let dir = tempfile::tempdir().unwrap();
        let (_, private, public) = key_pair();
        let (_, _, other_public) = key_pair();
        refresh(dir.path(), &public, &Fake::answering(sign(&served(2, false), &private).unwrap().into_bytes()), "u", 10).unwrap();
        // Checked against another key (a build with a new release key), or no key at all: the built-in list.
        assert_eq!(load(dir.path(), Some(&other_public)).origin, Origin::BuiltIn);
        assert_eq!(load(dir.path(), None).origin, Origin::BuiltIn);
        // A damaged file: the built-in list.
        std::fs::write(dir.path().join(KEPT_FILE), b"{").unwrap();
        assert_eq!(load(dir.path(), Some(&public)).origin, Origin::BuiltIn);
    }

    #[test]
    fn the_built_in_list_says_when_it_was_checked() {
        let m = Matrix::built_in();
        let day = day_seconds("2026-11-22").unwrap();
        assert_eq!(m.age(day), None);
        assert!(!m.stale(day + 3600) && m.stale(day + STALE_SECS + 3600));
    }
}
