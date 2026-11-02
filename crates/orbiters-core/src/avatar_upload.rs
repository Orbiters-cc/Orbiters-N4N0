//! Avatar uploads (contracts' `Docs/avatar-upload.md`): Orbiters Toolkit notes each successful upload from Unity in
//! `<local data>/Orbiters/Avatars/uploaded.json`, and the desktop asks VRChat to wear the uploaded avatar over OSC
//! (`/avatar/change`), so the person sees the new version without selecting it again. What is here decides, without
//! threads, sockets or clocks of its own: the record, which records count ([`UploadWatch`]), whether and how to switch
//! ([`decide`]) and the switch's steps as VRChat answers ([`Switch`]). The desktop's controller reads the file, sends and
//! shows the notices.
//!
//! Whether VRChat reloads the avatar it already wears when `/avatar/change` names it is unverified (2026-11-02):
//! [`SameAvatarReload`] chooses between asking for it as for any other and switching away and back.

use crate::settings::SameAvatarReload;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The file's name in [`crate::paths::avatars_dir`].
pub const FILE_NAME: &str = "uploaded.json";
/// The record's `schema` this reads.
pub const SCHEMA: u32 = 1;
/// The platform VRChat on this computer wears (the record's `platform`).
pub const PC_PLATFORM: &str = "windows";
/// Between the upload and the switch: VRChat's servers may not serve the new version at once.
pub const SWITCH_DELAY: Duration = Duration::from_secs(3);
/// How long VRChat has to say it wears the avatar asked for (its own `/avatar/change`): a large avatar downloads first.
pub const CONFIRM_WITHIN: Duration = Duration::from_secs(45);
/// Away and back: once VRChat wears the other avatar, this long before the uploaded one is asked for again.
pub const AWAY_FOR: Duration = Duration::from_secs(2);
/// A record counts when its `uploadedAt` is at most this far from now: one left from another day does nothing when the
/// app starts, one written just before it started still counts.
pub const FRESH: Duration = Duration::from_secs(120);

/// `<local data>/Orbiters/Avatars/uploaded.json`.
pub fn file() -> PathBuf {
    crate::paths::avatars_dir().join(FILE_NAME)
}

/// What Toolkit wrote of one upload.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadRecord {
    pub schema: u32,
    pub avatar_id: String,
    pub platform: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub project: String,
    pub uploaded_at: String,
    #[serde(default)]
    pub unity_pid: Option<u32>,
    /// `uploaded_at` in milliseconds since 1970 (UTC).
    #[serde(skip)]
    pub uploaded_ms: i64,
}

impl UploadRecord {
    /// The file's content, refused when its schema is not [`SCHEMA`], its id not an uploaded avatar's (`avtr_`) or its
    /// time not RFC 3339.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut record: Self = serde_json::from_slice(bytes).map_err(|e| format!("not an upload record: {e}"))?;
        if record.schema != SCHEMA {
            return Err(format!("schema {} (this reads {SCHEMA})", record.schema));
        }
        if !is_uploaded_avatar(&record.avatar_id) {
            return Err(format!("{:?} is not an uploaded avatar's id", record.avatar_id));
        }
        let at = time::OffsetDateTime::parse(&record.uploaded_at, &time::format_description::well_known::Rfc3339)
            .map_err(|e| format!("uploadedAt {:?}: {e}", record.uploaded_at))?;
        record.uploaded_ms = (at.unix_timestamp_nanos() / 1_000_000) as i64;
        Ok(record)
    }

    /// The avatar's name, else its id.
    pub fn label(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() {
            &self.avatar_id
        } else {
            name
        }
    }
}

/// `avtr_` and at most 64 characters VRChat ids are made of (`/avatar/change` takes no other).
pub fn is_uploaded_avatar(id: &str) -> bool {
    id.len() > 5 && id.len() <= 64 && id.starts_with("avtr_") && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Which records count: a file looked at again only when its modification time or size changed, and each upload once.
#[derive(Debug, Default)]
pub struct UploadWatch {
    stamp: Option<(std::time::SystemTime, u64)>,
    last_ms: Option<i64>,
}

impl UploadWatch {
    /// The file's modification time and size now (`None`: no file): true when it is to be read again. A file that went
    /// away is no change.
    pub fn changed(&mut self, stamp: Option<(std::time::SystemTime, u64)>) -> bool {
        match stamp {
            Some(stamp) if self.stamp != Some(stamp) => {
                self.stamp = Some(stamp);
                true
            }
            _ => false,
        }
    }

    /// `record`, read from the file, when it is a new upload: newer than the last one seen and within [`FRESH`] of
    /// `now_ms` (milliseconds since 1970). A stale record is still seen: an older one never counts after it.
    pub fn take(&mut self, record: UploadRecord, now_ms: i64) -> Option<UploadRecord> {
        if self.last_ms.is_some_and(|last| record.uploaded_ms <= last) {
            return None;
        }
        self.last_ms = Some(record.uploaded_ms);
        ((now_ms - record.uploaded_ms).unsigned_abs() <= FRESH.as_millis() as u64).then_some(record)
    }
}

/// The avatar VRChat wears and the other one it wore before it, from VRChat's `/avatar/change` since the app started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worn {
    pub current: Option<String>,
    pub previous: Option<String>,
}

impl Worn {
    /// VRChat says it wears `id` (again, in a new world, changes nothing).
    pub fn changed(&mut self, id: &str) {
        if self.current.as_deref() != Some(id) {
            self.previous = self.current.replace(id.to_string());
        }
    }
}

/// What the desktop knows when an upload is noted.
#[derive(Clone, Copy, Debug)]
pub struct Situation<'a> {
    /// Settings › VRChat and OSC › Switch to the avatar after an upload.
    pub enabled: bool,
    pub same_avatar: SameAvatarReload,
    pub gate: Gate,
    /// The avatar VRChat wears, when known.
    pub worn: Option<&'a str>,
    /// The other avatar it wore before, when one was seen.
    pub previous: Option<&'a str>,
}

/// What must hold for a message to be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    /// VRChat runs (or OSCQuery found it) and OSC has somewhere to send.
    pub vrchat: bool,
    /// A recording runs (3D pictures and scans are recordings): an avatar change would reset the capture kit.
    pub recording: bool,
}

/// How a notice reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Warning,
}

/// What to do about a new upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing, not even a notice; the reason for the log.
    Nothing(&'static str),
    Notice(String),
    Switch(Switch),
}

pub fn decide(record: &UploadRecord, now: Instant, s: &Situation) -> Decision {
    if !s.enabled {
        return Decision::Nothing("Switch to the avatar after an upload is off");
    }
    if record.platform != PC_PLATFORM {
        return Decision::Nothing("built for another platform than this PC's");
    }
    if let Some(blocked) = blocked(s.gate, record.label()) {
        return Decision::Notice(blocked);
    }
    let same = s.worn == Some(record.avatar_id.as_str());
    let steps = match (same, s.same_avatar, s.previous.filter(|p| *p != record.avatar_id)) {
        (true, SameAvatarReload::AwayAndBack, Some(previous)) => vec![previous.to_string(), record.avatar_id.clone()],
        (true, SameAvatarReload::AwayAndBack, None) => {
            return Decision::Notice(format!("Uploaded {}: VRChat keeps the old version until you re-select it.", record.label()));
        }
        _ => vec![record.avatar_id.clone()],
    };
    Decision::Switch(Switch {
        avatar_id: record.avatar_id.clone(),
        name: record.label().to_string(),
        same,
        was_worn: s.worn.map(str::to_string),
        steps,
        step: 0,
        phase: Phase::Waiting { until: now + SWITCH_DELAY },
    })
}

/// Why nothing can be sent now, as the notice says it.
fn blocked(gate: Gate, name: &str) -> Option<String> {
    if !gate.vrchat {
        Some(format!("Uploaded {name}: open VRChat to wear it."))
    } else if gate.recording {
        Some(format!("Uploaded {name}: VRChat keeps your avatar while you record. Select it in VRChat afterwards to wear the new version."))
    } else {
        None
    }
}

/// A switch under way: the avatars to ask VRChat for, in order (the last is the uploaded one), each once VRChat said it
/// wears the one before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    pub avatar_id: String,
    pub name: String,
    /// The uploaded avatar was the one worn.
    pub same: bool,
    /// What VRChat wore when the upload was noted: its late `/avatar/change` (a world just joined) ends nothing.
    was_worn: Option<String>,
    steps: Vec<String>,
    step: usize,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The next step is sent then.
    Waiting { until: Instant },
    /// The step was sent then; VRChat has not said it wears that avatar yet.
    Sent { at: Instant },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Wait,
    /// Send `/avatar/change` with this id.
    Send(String),
    Done(Outcome),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// VRChat said it wears the uploaded avatar, this long after it was asked for.
    Switched { after: Duration },
    /// VRChat said nothing in [`CONFIRM_WITHIN`] after it was asked for this step's avatar (`away`: the other one).
    NoEcho { away: bool },
    /// The person changed avatar meanwhile (to this one).
    Interrupted { by: String },
    /// Just before a send: VRChat is gone, or a recording started (the notice says which).
    Blocked(String),
    /// The message could not be sent.
    SendFailed(String),
}

impl Switch {
    /// The avatars asked for, in order.
    pub fn steps(&self) -> &[String] {
        &self.steps
    }

    /// Switching away to the previous avatar first.
    pub fn away_and_back(&self) -> bool {
        self.steps.len() > 1
    }

    /// Time passed (every second or so): the next message when it is due and `gate` lets it go, the end when VRChat
    /// stayed silent too long.
    pub fn step(&mut self, now: Instant, gate: Gate) -> Progress {
        match self.phase {
            Phase::Waiting { until } if now >= until => match blocked(gate, &self.name) {
                Some(why) => Progress::Done(Outcome::Blocked(why)),
                None => {
                    self.phase = Phase::Sent { at: now };
                    Progress::Send(self.steps[self.step].clone())
                }
            },
            Phase::Sent { at } if now.saturating_duration_since(at) > CONFIRM_WITHIN => {
                Progress::Done(Outcome::NoEcho { away: self.step + 1 < self.steps.len() })
            }
            _ => Progress::Wait,
        }
    }

    /// VRChat said it wears `id` (`/avatar/change`): the next step after a moment, or the end.
    pub fn echo(&mut self, id: &str, now: Instant) -> Option<Outcome> {
        let before = self.step.checked_sub(1).map(|i| self.steps[i].as_str());
        match self.phase {
            Phase::Sent { at } if id == self.steps[self.step] => {
                if self.step + 1 == self.steps.len() {
                    return Some(Outcome::Switched { after: now.saturating_duration_since(at) });
                }
                self.step += 1;
                self.phase = Phase::Waiting { until: now + AWAY_FOR };
                None
            }
            // The step before said again, or the avatar worn before anything was sent (a world joined).
            _ if Some(id) == before => None,
            _ if self.step == 0 && Some(id) == self.was_worn.as_deref() => None,
            _ => Some(Outcome::Interrupted { by: id.to_string() }),
        }
    }

    /// The notice an outcome makes (none when the person changed avatar).
    pub fn notice(&self, outcome: &Outcome) -> Option<(Tone, String)> {
        let name = &self.name;
        Some(match outcome {
            Outcome::Switched { .. } => (Tone::Success, format!("Switched VRChat to {name} (uploaded just now).")),
            Outcome::NoEcho { away: true } => {
                (Tone::Warning, format!("VRChat did not switch away from {name}: it keeps the old version until you re-select it."))
            }
            Outcome::NoEcho { away: false } if self.same && !self.away_and_back() => {
                (Tone::Warning, format!("VRChat did not reload {name}: it keeps the old version until you re-select it."))
            }
            Outcome::NoEcho { away: false } => (Tone::Warning, format!("VRChat did not switch to {name}. Select it in VRChat to wear the new version.")),
            Outcome::Blocked(why) => (Tone::Info, why.clone()),
            Outcome::SendFailed(e) => (Tone::Warning, format!("Could not ask VRChat to wear {name}: {e}")),
            Outcome::Interrupted { .. } => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "avtr_2bd4c3b1-6a8e-4d1f-9e0a-1c2d3e4f5a6b";
    const OTHER: &str = "avtr_00000000-0000-0000-0000-000000000001";
    const THIRD: &str = "avtr_00000000-0000-0000-0000-000000000002";
    const OPEN: Gate = Gate { vrchat: true, recording: false };

    fn json(platform: &str, at: &str) -> String {
        format!(
            r#"{{"schema":1,"avatarId":"{ID}","platform":"{platform}","name":"Rexouium","project":"H:\\Unity\\Rex","uploadedAt":"{at}","unityPid":23140,"later":true}}"#
        )
    }

    fn record(at: &str) -> UploadRecord {
        UploadRecord::parse(json("windows", at).as_bytes()).unwrap()
    }

    fn situation<'a>(worn: Option<&'a str>, previous: Option<&'a str>, same_avatar: SameAvatarReload) -> Situation<'a> {
        Situation { enabled: true, same_avatar, gate: OPEN, worn, previous }
    }

    fn switch(d: Decision) -> Switch {
        match d {
            Decision::Switch(s) => s,
            other => panic!("not a switch: {other:?}"),
        }
    }

    #[test]
    fn the_contracts_record_is_read() {
        let r = record("2026-11-02T14:03:27.512Z");
        assert_eq!(r.avatar_id, ID);
        assert_eq!(r.platform, "windows");
        assert_eq!(r.label(), "Rexouium");
        assert_eq!(r.project, "H:\\Unity\\Rex");
        assert_eq!(r.unity_pid, Some(23140));
        assert_eq!(r.uploaded_ms, 1_793_628_207_512);
        // An offset is the same instant; no name shows the id.
        let r = UploadRecord::parse(br#"{"schema":1,"avatarId":"avtr_x","platform":"android","uploadedAt":"2026-11-02T15:03:27.512+01:00"}"#).unwrap();
        assert_eq!((r.uploaded_ms, r.label(), r.unity_pid), (1_793_628_207_512, "avtr_x", None));
    }

    #[test]
    fn other_records_are_refused() {
        for bad in [
            r#"{"schema":2,"avatarId":"avtr_x","platform":"windows","uploadedAt":"2026-11-02T14:03:27Z"}"#,
            r#"{"schema":1,"avatarId":"local:sdk_Rex","platform":"windows","uploadedAt":"2026-11-02T14:03:27Z"}"#,
            r#"{"schema":1,"avatarId":"wrld_x","platform":"windows","uploadedAt":"2026-11-02T14:03:27Z"}"#,
            r#"{"schema":1,"avatarId":"avtr_","platform":"windows","uploadedAt":"2026-11-02T14:03:27Z"}"#,
            r#"{"schema":1,"avatarId":"avtr_x","platform":"windows","uploadedAt":"yesterday"}"#,
            r#"{"schema":1,"avatarId":"avtr_x","platform":"windows"}"#,
            r#"{"schema":1,"avatarId":"avtr_x","platform":"windows","uploadedAt":"2026-11-02T14:03:27Z""#,
            "",
        ] {
            assert!(UploadRecord::parse(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_upload_counts_once_and_only_while_fresh() {
        let mut w = UploadWatch::default();
        let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        assert!(!w.changed(None));
        assert!(w.changed(Some((t, 200))));
        assert!(!w.changed(Some((t, 200))), "the same file is not read again");
        assert!(w.changed(Some((t, 201))), "nor is a file of another size missed");
        assert!(!w.changed(None), "a file that went away is no change");

        let first = record("2026-11-02T14:03:27.512Z");
        let now = first.uploaded_ms + 1_000;
        assert_eq!(w.take(first.clone(), now), Some(first.clone()));
        assert_eq!(w.take(first.clone(), now), None, "the same upload again");
        let older = record("2026-11-02T14:03:20.000Z");
        assert_eq!(w.take(older, now), None, "an older one");
        let next = record("2026-11-02T14:05:00.000Z");
        assert_eq!(w.take(next.clone(), next.uploaded_ms + 500), Some(next));

        // The app starts the next day: yesterday's record does nothing, nor does it after.
        let mut w = UploadWatch::default();
        let old = record("2026-11-01T09:00:00Z");
        assert_eq!(w.take(old.clone(), old.uploaded_ms + 86_400_000), None);
        assert_eq!(w.take(old.clone(), old.uploaded_ms + 1), None);
        // One written a minute before the app started counts; a clock a little behind Unity's too.
        let mut w = UploadWatch::default();
        let r = record("2026-11-02T14:03:27Z");
        assert!(w.take(r.clone(), r.uploaded_ms + 60_000).is_some());
        let mut w = UploadWatch::default();
        assert!(w.take(r.clone(), r.uploaded_ms - 5_000).is_some());
    }

    #[test]
    fn what_gates_a_switch() {
        let now = Instant::now();
        let r = record("2026-11-02T14:03:27Z");
        let s = situation(Some(OTHER), None, SameAvatarReload::Reload);
        assert!(matches!(decide(&r, now, &Situation { enabled: false, ..s }), Decision::Nothing(_)));
        for platform in ["android", "ios", "Windows", ""] {
            let other = UploadRecord { platform: platform.into(), ..r.clone() };
            assert!(matches!(decide(&other, now, &s), Decision::Nothing(_)), "{platform}");
        }
        assert_eq!(
            decide(&r, now, &Situation { gate: Gate { vrchat: false, recording: false }, ..s }),
            Decision::Notice("Uploaded Rexouium: open VRChat to wear it.".into())
        );
        match decide(&r, now, &Situation { gate: Gate { vrchat: true, recording: true }, ..s }) {
            Decision::Notice(n) => assert!(n.starts_with("Uploaded Rexouium: VRChat keeps your avatar while you record."), "{n}"),
            other => panic!("{other:?}"),
        }
        let sw = switch(decide(&r, now, &s));
        assert_eq!((sw.steps(), sw.same, sw.avatar_id.as_str()), (&[ID.to_string()][..], false, ID));
        // Nothing worn known yet: asked for as any other.
        assert_eq!(switch(decide(&r, now, &situation(None, None, SameAvatarReload::AwayAndBack))).steps(), [ID]);
    }

    #[test]
    fn a_switch_waits_then_sends_once_and_ends_with_vrchats_echo() {
        let t0 = Instant::now();
        let r = record("2026-11-02T14:03:27Z");
        let mut sw = switch(decide(&r, t0, &situation(Some(OTHER), None, SameAvatarReload::Reload)));
        assert_eq!(sw.step(t0 + Duration::from_secs(2), OPEN), Progress::Wait);
        // VRChat says the avatar worn again (a world joined) before anything was sent: the switch still goes.
        assert_eq!(sw.echo(OTHER, t0 + Duration::from_secs(2)), None);
        assert_eq!(sw.step(t0 + SWITCH_DELAY, OPEN), Progress::Send(ID.into()));
        assert_eq!(sw.step(t0 + SWITCH_DELAY + Duration::from_secs(1), OPEN), Progress::Wait, "sent once");
        // A late echo of the avatar worn before is not the person's change.
        assert_eq!(sw.echo(OTHER, t0 + Duration::from_secs(4)), None);
        let done = sw.echo(ID, t0 + Duration::from_secs(9)).unwrap();
        assert_eq!(done, Outcome::Switched { after: Duration::from_secs(6) });
        assert_eq!(sw.notice(&done), Some((Tone::Success, "Switched VRChat to Rexouium (uploaded just now).".into())));
    }

    #[test]
    fn a_recording_or_vrchat_gone_before_the_send_stops_it() {
        let t0 = Instant::now();
        let r = record("2026-11-02T14:03:27Z");
        let s = situation(Some(OTHER), None, SameAvatarReload::Reload);
        let mut sw = switch(decide(&r, t0, &s));
        let done = sw.step(t0 + SWITCH_DELAY, Gate { vrchat: true, recording: true });
        let Progress::Done(outcome) = done else { panic!("{done:?}") };
        assert_eq!(sw.notice(&outcome).unwrap().0, Tone::Info);
        let mut sw = switch(decide(&r, t0, &s));
        let done = sw.step(t0 + SWITCH_DELAY, Gate { vrchat: false, recording: false });
        assert_eq!(done, Progress::Done(Outcome::Blocked("Uploaded Rexouium: open VRChat to wear it.".into())));
    }

    #[test]
    fn silence_and_the_persons_own_change_end_it() {
        let t0 = Instant::now();
        let r = record("2026-11-02T14:03:27Z");
        let mut sw = switch(decide(&r, t0, &situation(Some(OTHER), None, SameAvatarReload::Reload)));
        let sent = t0 + SWITCH_DELAY;
        assert_eq!(sw.step(sent, OPEN), Progress::Send(ID.into()));
        assert_eq!(sw.step(sent + CONFIRM_WITHIN, OPEN), Progress::Wait);
        let done = sw.step(sent + CONFIRM_WITHIN + Duration::from_millis(1), OPEN);
        assert_eq!(done, Progress::Done(Outcome::NoEcho { away: false }));
        let (tone, text) = sw.notice(&Outcome::NoEcho { away: false }).unwrap();
        assert_eq!((tone, text.as_str()), (Tone::Warning, "VRChat did not switch to Rexouium. Select it in VRChat to wear the new version."));

        let mut sw = switch(decide(&r, t0, &situation(Some(OTHER), None, SameAvatarReload::Reload)));
        assert_eq!(sw.step(sent, OPEN), Progress::Send(ID.into()));
        let done = sw.echo(THIRD, sent + Duration::from_secs(1)).unwrap();
        assert_eq!(done, Outcome::Interrupted { by: THIRD.into() });
        assert_eq!(sw.notice(&done), None);
        // Also before anything was sent.
        let mut sw = switch(decide(&r, t0, &situation(Some(OTHER), None, SameAvatarReload::Reload)));
        assert_eq!(sw.echo(THIRD, t0 + Duration::from_secs(1)), Some(Outcome::Interrupted { by: THIRD.into() }));
    }

    #[test]
    fn the_avatar_worn_is_asked_for_again_when_vrchat_reloads_it() {
        let t0 = Instant::now();
        let r = record("2026-11-02T14:03:27Z");
        let mut sw = switch(decide(&r, t0, &situation(Some(ID), Some(OTHER), SameAvatarReload::Reload)));
        assert!(sw.same && !sw.away_and_back());
        assert_eq!(sw.step(t0 + SWITCH_DELAY, OPEN), Progress::Send(ID.into()));
        // Its echo is the reload.
        assert!(matches!(sw.echo(ID, t0 + Duration::from_secs(5)), Some(Outcome::Switched { .. })));
        // Without one, VRChat ignored it.
        let mut sw = switch(decide(&r, t0, &situation(Some(ID), None, SameAvatarReload::Reload)));
        sw.step(t0 + SWITCH_DELAY, OPEN);
        let done = sw.step(t0 + SWITCH_DELAY + CONFIRM_WITHIN + Duration::from_secs(1), OPEN);
        let Progress::Done(outcome) = done else { panic!("{done:?}") };
        assert_eq!(sw.notice(&outcome).unwrap().1, "VRChat did not reload Rexouium: it keeps the old version until you re-select it.");
    }

    #[test]
    fn away_and_back_goes_through_the_previous_avatar() {
        let t0 = Instant::now();
        let r = record("2026-11-02T14:03:27Z");
        let mut sw = switch(decide(&r, t0, &situation(Some(ID), Some(OTHER), SameAvatarReload::AwayAndBack)));
        assert_eq!(sw.steps(), [OTHER, ID]);
        let sent = t0 + SWITCH_DELAY;
        assert_eq!(sw.step(sent, OPEN), Progress::Send(OTHER.into()));
        // The avatar worn, said again before VRChat changed: nothing.
        assert_eq!(sw.echo(ID, sent + Duration::from_millis(500)), None);
        assert_eq!(sw.echo(OTHER, sent + Duration::from_secs(4)), None);
        // Back once the other avatar had a moment.
        let away = sent + Duration::from_secs(4);
        assert_eq!(sw.step(away + Duration::from_secs(1), OPEN), Progress::Wait);
        assert_eq!(sw.echo(OTHER, away + Duration::from_secs(1)), None, "the other avatar said again");
        assert_eq!(sw.step(away + AWAY_FOR, OPEN), Progress::Send(ID.into()));
        assert_eq!(sw.echo(ID, away + AWAY_FOR + Duration::from_secs(3)), Some(Outcome::Switched { after: Duration::from_secs(3) }));

        // A recording started while away: the uploaded one is not asked for.
        let mut sw = switch(decide(&r, t0, &situation(Some(ID), Some(OTHER), SameAvatarReload::AwayAndBack)));
        sw.step(sent, OPEN);
        sw.echo(OTHER, sent + Duration::from_secs(1));
        assert!(matches!(sw.step(sent + Duration::from_secs(5), Gate { vrchat: true, recording: true }), Progress::Done(Outcome::Blocked(_))));
        // The person picked a third avatar while away.
        let mut sw = switch(decide(&r, t0, &situation(Some(ID), Some(OTHER), SameAvatarReload::AwayAndBack)));
        sw.step(sent, OPEN);
        sw.echo(OTHER, sent + Duration::from_secs(1));
        assert_eq!(sw.echo(THIRD, sent + Duration::from_secs(2)), Some(Outcome::Interrupted { by: THIRD.into() }));
        // VRChat never left the worn one.
        let mut sw = switch(decide(&r, t0, &situation(Some(ID), Some(OTHER), SameAvatarReload::AwayAndBack)));
        sw.step(sent, OPEN);
        let done = sw.step(sent + CONFIRM_WITHIN + Duration::from_secs(1), OPEN);
        assert_eq!(done, Progress::Done(Outcome::NoEcho { away: true }));
        assert_eq!(
            sw.notice(&Outcome::NoEcho { away: true }).unwrap().1,
            "VRChat did not switch away from Rexouium: it keeps the old version until you re-select it."
        );
    }

    #[test]
    fn away_and_back_without_another_avatar_says_so() {
        let r = record("2026-11-02T14:03:27Z");
        let notice = Decision::Notice("Uploaded Rexouium: VRChat keeps the old version until you re-select it.".into());
        assert_eq!(decide(&r, Instant::now(), &situation(Some(ID), None, SameAvatarReload::AwayAndBack)), notice);
        // The "previous" being the uploaded one is none.
        assert_eq!(decide(&r, Instant::now(), &situation(Some(ID), Some(ID), SameAvatarReload::AwayAndBack)), notice);
    }

    #[test]
    fn the_worn_avatars_follow_vrchat() {
        let mut w = Worn::default();
        w.changed(ID);
        assert_eq!((w.current.as_deref(), w.previous.as_deref()), (Some(ID), None));
        w.changed(ID);
        assert_eq!(w.previous, None, "the same avatar in a new world");
        w.changed(OTHER);
        assert_eq!((w.current.as_deref(), w.previous.as_deref()), (Some(OTHER), Some(ID)));
        w.changed(ID);
        assert_eq!((w.current.as_deref(), w.previous.as_deref()), (Some(ID), Some(OTHER)));
    }
}
