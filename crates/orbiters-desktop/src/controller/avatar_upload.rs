//! An avatar uploaded from Unity, worn in VRChat (contracts' `Docs/avatar-upload.md`): Orbiters Toolkit replaces
//! `<local data>/Orbiters/Avatars/uploaded.json` after each upload; the watchers look at it every second, and a new
//! Windows upload makes VRChat wear the avatar 3 s later (`/avatar/change`), unless Settings say not to, VRChat is not
//! there or a recording runs (a notice then). What decides is `orbiters_core::avatar_upload`; here the file is read,
//! the messages are sent through the OSC service and VRChat's own `/avatar/change` (on the OSC receiving thread) moves
//! the switch on. Every step is noted in `logs/avatar-upload.log`, which tells whether VRChat reloads the avatar it
//! already wears (unverified).

use super::{NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_core::avatar_upload::{self as upload, Decision, Gate, Outcome, Progress, Situation, Switch, Tone, UploadRecord, UploadWatch, Worn};
use orbiters_core::osc::{OscValue, AVATAR_CHANGE};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The log is set aside as `avatar-upload.old.log` past this.
const MAX_LOG: u64 = 256 * 1024;

#[derive(Debug, Default)]
pub(super) struct AvatarUploadRuntime {
    watch: UploadWatch,
    worn: Worn,
    switch: Option<Switch>,
}

fn now_ms() -> i64 {
    orbiters_n4n0::unix_ms() as i64
}

fn log_file() -> PathBuf {
    orbiters_core::paths::logs_dir().join("avatar-upload.log")
}

/// One line in the log (and on stderr), set aside first when it grew past [`MAX_LOG`].
fn note(line: &str) {
    eprintln!("avatar upload: {line}");
    if cfg!(test) {
        return;
    }
    let path = log_file();
    let _ = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
            std::fs::rename(&path, path.with_file_name("avatar-upload.old.log"))?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(format!("{} {line}\n", orbiters_core::now_utc_iso()).as_bytes())
    })();
}

fn kind(tone: Tone) -> NoticeKind {
    match tone {
        Tone::Info => NoticeKind::Info,
        Tone::Success => NoticeKind::Success,
        Tone::Warning => NoticeKind::Warning,
    }
}

impl super::Controller {
    /// Every second (the watchers): a new upload, and the switch under way.
    pub(super) fn avatar_upload_tick(self: &Shared) {
        self.avatar_upload_tick_in(&upload::file(), now_ms());
    }

    fn avatar_upload_tick_in(self: &Shared, file: &Path, now_ms: i64) {
        let stamp = std::fs::metadata(file).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
        let fresh = {
            let mut rt = self.avatar_upload.lock().unwrap();
            if rt.watch.changed(stamp) {
                match std::fs::read(file).map_err(|e| e.to_string()).and_then(|b| UploadRecord::parse(&b)) {
                    Ok(record) => rt.watch.take(record, now_ms),
                    Err(e) => {
                        note(&format!("{} not read: {e}", file.display()));
                        None
                    }
                }
            } else {
                None
            }
        };
        if let Some(record) = fresh {
            self.avatar_uploaded(&record);
        }
        self.avatar_switch_step();
    }

    /// What holds for sending to VRChat now.
    fn avatar_upload_gate(&self) -> Gate {
        let (running, found, recording) = {
            let s = self.snapshot.lock().unwrap();
            (s.vrchat.running, s.osc.vrchat_found, matches!(s.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused))
        };
        let target = self.with_osc(|o| o.vrchat_target().is_some()).unwrap_or(false);
        Gate { vrchat: (running || found) && target, recording }
    }

    /// The avatar VRChat wears: as it said since the app started, else as OSCQuery or N4N0 know it.
    fn worn_avatar(&self, worn: &Worn) -> Option<String> {
        worn.current.clone().or_else(|| match self.osc_values.lock().unwrap().latest.first(AVATAR_CHANGE) {
            Some(OscValue::Text(id)) => Some(id),
            _ => self.snapshot.lock().unwrap().n4n0.avatar_id.clone(),
        })
    }

    fn avatar_uploaded(&self, record: &UploadRecord) {
        let (enabled, same_avatar) = self.with_settings(|s| (s.osc.switch_after_upload, s.osc.same_avatar_reload));
        let gate = self.avatar_upload_gate();
        let worn = self.avatar_upload.lock().unwrap().worn.clone();
        let current = self.worn_avatar(&worn);
        let situation = Situation { enabled, same_avatar, gate, worn: current.as_deref(), previous: worn.previous.as_deref() };
        let what = format!(
            "{} ({}) uploaded for {} from {} at {}; VRChat wears {}, before it {}",
            record.label(),
            record.avatar_id,
            record.platform,
            record.project,
            record.uploaded_at,
            current.as_deref().unwrap_or("nothing known"),
            worn.previous.as_deref().unwrap_or("nothing known"),
        );
        match upload::decide(record, Instant::now(), &situation) {
            Decision::Nothing(why) => note(&format!("{what}: nothing done, {why}")),
            Decision::Notice(text) => {
                note(&format!("{what}: {text}"));
                self.notify(NoticeKind::Info, text);
            }
            Decision::Switch(switch) => {
                note(&format!("{what}: switching ({same_avatar:?}), asking for {}", switch.steps().join(" then ")));
                let replaced = self.avatar_upload.lock().unwrap().switch.replace(switch);
                if let Some(old) = replaced {
                    note(&format!("the switch to {} gave way to the newer upload", old.avatar_id));
                }
            }
        }
    }

    /// The switch's next message when it is due (sent outside the runtime's lock: VRChat's answer comes in on the OSC
    /// receiving thread, which takes it), or its end.
    fn avatar_switch_step(&self) {
        let gate = self.avatar_upload_gate();
        let progress = match self.avatar_upload.lock().unwrap().switch.as_mut() {
            Some(switch) => switch.step(Instant::now(), gate),
            None => return,
        };
        match progress {
            Progress::Wait => {}
            Progress::Send(id) => {
                let sent = self.with_osc(|o| o.send(AVATAR_CHANGE, vec![rosc::OscType::String(id.clone())]));
                match sent {
                    Some(Ok(())) => note(&format!("sent {AVATAR_CHANGE} {id}")),
                    Some(Err(e)) => self.avatar_switch_done(Outcome::SendFailed(e.to_string())),
                    None => self.avatar_switch_done(Outcome::SendFailed("OSC is off".into())),
                }
            }
            Progress::Done(outcome) => self.avatar_switch_done(outcome),
        }
    }

    /// VRChat said it wears `id` (OSC receiving thread): what the next upload knows, and the switch's next step.
    pub(super) fn avatar_upload_echo(&self, id: &str) {
        let outcome = {
            let mut rt = self.avatar_upload.lock().unwrap();
            rt.worn.changed(id);
            let Some(switch) = rt.switch.as_mut() else { return };
            note(&format!("VRChat says it wears {id}"));
            switch.echo(id, Instant::now())
        };
        if let Some(outcome) = outcome {
            self.avatar_switch_done(outcome);
        }
    }

    fn avatar_switch_done(&self, outcome: Outcome) {
        let Some(switch) = self.avatar_upload.lock().unwrap().switch.take() else { return };
        note(&format!("switch to {} ended: {outcome:?}", switch.avatar_id));
        if let Some((tone, text)) = switch.notice(&outcome) {
            self.notify(kind(tone), text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Controller, Notice};
    use super::*;
    use orbiters_core::settings::{SameAvatarReload, Settings};
    use std::time::Duration;

    const ID: &str = "avtr_2bd4c3b1-6a8e-4d1f-9e0a-1c2d3e4f5a6b";
    const OTHER: &str = "avtr_00000000-0000-0000-0000-000000000001";

    /// A folder of its own for one test's record, removed after it.
    struct Folder(PathBuf);

    impl Folder {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A controller sending to a stand-in VRChat, which runs.
    fn setup(same: SameAvatarReload) -> (Shared, std::net::UdpSocket, Folder) {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        settings.osc.same_avatar_reload = same;
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.update(|s| s.vrchat.running = true);
        let port = vrchat.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("orbiters-avatar-upload-{}-{port}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (ctl, vrchat, Folder(dir))
    }

    fn write(dir: &Path, platform: &str, at_ms: i64) -> PathBuf {
        let at = time::OffsetDateTime::from_unix_timestamp_nanos(at_ms as i128 * 1_000_000).unwrap();
        let at = at.format(&time::format_description::well_known::Rfc3339).unwrap();
        let path = dir.join(upload::FILE_NAME);
        let json = format!(r#"{{"schema":1,"avatarId":"{ID}","platform":"{platform}","name":"Rexouium","project":"P","uploadedAt":"{at}","unityPid":1}}"#);
        std::fs::write(&path, json).unwrap();
        path
    }

    fn changes(socket: &std::net::UdpSocket, wait: Duration) -> Vec<String> {
        let mut out = Vec::new();
        let until = Instant::now() + wait;
        let mut buf = [0u8; 1024];
        while Instant::now() < until {
            if let Ok(n) = socket.recv(&mut buf) {
                if let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                    if let (AVATAR_CHANGE, [rosc::OscType::String(id)]) = (m.addr.as_str(), m.args.as_slice()) {
                        out.push(id.clone());
                    }
                }
            }
        }
        out
    }

    fn toasts(ctl: &Shared) -> Vec<(NoticeKind, String)> {
        ctl.take_notices()
            .into_iter()
            .filter_map(|n| match n {
                Notice::Toast { kind, message } => Some((kind, message)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_upload_makes_vrchat_wear_the_avatar_once() {
        let (ctl, vrchat, dir) = setup(SameAvatarReload::Reload);
        ctl.avatar_upload_echo(OTHER);
        let now = now_ms();
        let file = write(dir.path(), "windows", now - 1_000);
        ctl.avatar_upload_tick_in(&file, now);
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_some());
        assert!(changes(&vrchat, Duration::from_millis(200)).is_empty(), "nothing before the delay");
        // Due: the tick sends it once.
        std::thread::sleep(upload::SWITCH_DELAY);
        ctl.avatar_upload_tick_in(&file, now);
        ctl.avatar_upload_tick_in(&file, now);
        assert_eq!(changes(&vrchat, Duration::from_millis(300)), vec![ID.to_string()]);
        // VRChat's echo ends it with the notice.
        ctl.avatar_upload_echo(ID);
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_none());
        assert_eq!(toasts(&ctl), vec![(NoticeKind::Success, "Switched VRChat to Rexouium (uploaded just now).".to_string())]);
        // The same file again, and the same upload rewritten: nothing.
        ctl.avatar_upload_tick_in(&file, now);
        let file = write(dir.path(), "windows", now - 1_000);
        std::fs::write(&file, std::fs::read(&file).unwrap().into_iter().chain(*b" ").collect::<Vec<u8>>()).unwrap();
        ctl.avatar_upload_tick_in(&file, now);
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_none());
        ctl.shutdown();
    }

    #[test]
    fn uploads_that_must_not_switch() {
        // A recording runs: a notice, nothing sent.
        let (ctl, vrchat, dir) = setup(SameAvatarReload::Reload);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        let now = now_ms();
        ctl.avatar_upload_tick_in(&write(dir.path(), "windows", now), now);
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_none());
        let t = toasts(&ctl);
        assert_eq!(t.len(), 1);
        assert!(t[0].1.contains("while you record"), "{t:?}");
        // An Android upload: nothing at all.
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.avatar_upload_tick_in(&write(dir.path(), "android", now + 1), now);
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_none() && toasts(&ctl).is_empty());
        // VRChat closed.
        ctl.update(|s| s.vrchat.running = false);
        ctl.avatar_upload_tick_in(&write(dir.path(), "windows", now + 2), now);
        assert_eq!(toasts(&ctl), vec![(NoticeKind::Info, "Uploaded Rexouium: open VRChat to wear it.".to_string())]);
        // The setting off.
        ctl.update(|s| {
            s.vrchat.running = true;
            s.settings.osc.switch_after_upload = false;
        });
        ctl.avatar_upload_tick_in(&write(dir.path(), "windows", now + 3), now);
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_none() && toasts(&ctl).is_empty());
        assert!(changes(&vrchat, Duration::from_millis(200)).is_empty());
        ctl.shutdown();
    }

    #[test]
    fn a_recording_started_during_the_delay_keeps_the_avatar() {
        let (ctl, vrchat, dir) = setup(SameAvatarReload::Reload);
        let now = now_ms();
        let file = write(dir.path(), "windows", now);
        ctl.avatar_upload_tick_in(&file, now);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        std::thread::sleep(upload::SWITCH_DELAY);
        ctl.avatar_upload_tick_in(&file, now);
        assert!(changes(&vrchat, Duration::from_millis(200)).is_empty());
        assert!(ctl.avatar_upload.lock().unwrap().switch.is_none());
        assert!(toasts(&ctl)[0].1.contains("while you record"));
        ctl.shutdown();
    }

    #[test]
    fn away_and_back_switches_to_the_previous_avatar_first() {
        let (ctl, vrchat, dir) = setup(SameAvatarReload::AwayAndBack);
        ctl.avatar_upload_echo(OTHER);
        ctl.avatar_upload_echo(ID);
        let now = now_ms();
        let file = write(dir.path(), "windows", now);
        ctl.avatar_upload_tick_in(&file, now);
        std::thread::sleep(upload::SWITCH_DELAY);
        ctl.avatar_upload_tick_in(&file, now);
        assert_eq!(changes(&vrchat, Duration::from_millis(300)), vec![OTHER.to_string()]);
        ctl.avatar_upload_echo(OTHER);
        std::thread::sleep(upload::AWAY_FOR);
        ctl.avatar_upload_tick_in(&file, now);
        assert_eq!(changes(&vrchat, Duration::from_millis(300)), vec![ID.to_string()]);
        ctl.avatar_upload_echo(ID);
        assert_eq!(toasts(&ctl), vec![(NoticeKind::Success, "Switched VRChat to Rexouium (uploaded just now).".to_string())]);
        ctl.shutdown();
    }
}
