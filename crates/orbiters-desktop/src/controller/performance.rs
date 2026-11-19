//! World performance (contracts `Docs/world-performance.md`): the SteamVR overlay hands over VRChat's frame intervals,
//! this matches them with the world, the people there and the avatars worn, a summary a minute, and sends the
//! summaries to Orbiters every few minutes while the setting is on. Other people are only counted: their avatars go by
//! name, never who wears them. Unsent summaries wait on disk (a day at most).

use super::Shared;
use crate::state::FrameChunk;
use orbiters_core::account::Api;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A minute counts with at least this much of it measured.
const MIN_SECONDS: f32 = 20.0;
const SEND_EVERY: Duration = Duration::from_secs(5 * 60);
const MAX_PER_REQUEST: usize = 120;
/// Summaries kept while Orbiters cannot be reached (a day of minutes).
const MAX_PENDING: usize = 24 * 60;
const MAX_OTHERS: usize = 50;

/// One minute in one world, as sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub world_id: String,
    pub started_utc: String,
    pub seconds: u32,
    pub fps_mean: f32,
    pub fps_p05: f32,
    pub frame_ms_p95: f32,
    pub players: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<OwnAvatar>,
    pub others: Vec<OtherAvatar>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnAvatar {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OtherAvatar {
    pub name: String,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hardware {
    pub gpu: Option<String>,
    pub cpu: Option<String>,
    pub ram_gb: Option<u32>,
    pub headset: Option<String>,
    pub refresh_hz: Option<f32>,
}

/// The minute being gathered.
#[derive(Debug)]
struct Minute {
    /// Unix seconds of its start.
    start: u64,
    /// World and instance; a minute that saw two is dropped.
    place: String,
    world_id: String,
    mixed: bool,
    intervals: Vec<f32>,
    /// Who wore what during the minute (display names stay here, in memory).
    worn: BTreeMap<String, String>,
    players: u32,
    avatar: Option<OwnAvatar>,
}

#[derive(Default)]
pub struct PerformanceRuntime {
    minute: Option<Minute>,
    /// What waits to be sent, read from (and kept in) the file of the account it was gathered as.
    pending: Option<(PathBuf, Vec<Sample>)>,
    hardware: Hardware,
    last_sent: Option<Instant>,
}

/// Where the summaries gathered before this version waited, for every account and environment at once (CTL2-9).
fn legacy_pending_file() -> PathBuf {
    orbiters_core::paths::config_dir().join("world-performance-pending.json")
}

/// Where the summaries gathered as `user` on `server` wait, in `dir`: one file per account and environment, as the
/// upload records are, so that what one member gathered is never sent as another, nor development's to production.
fn pending_file_in(dir: &std::path::Path, server: orbiters_core::account::Server, user: &str) -> PathBuf {
    let env = if server == orbiters_core::account::Server::Development { "dev" } else { "prod" };
    dir.join(format!("world-performance-pending-{env}-{user}.json"))
}

/// What waits to be sent, as kept; a file that cannot be read is kept beside before the next save replaces it.
fn read_pending(path: &std::path::Path) -> Vec<Sample> {
    let Ok(Some(text)) = orbiters_core::read_text(path) else { return Vec::new() };
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).unwrap_or_else(|e| {
        eprintln!("world performance: {}: {e}; kept as {:?}", path.display(), orbiters_core::keep_aside(&path));
        Vec::new()
    })
}

/// Writes what waits to be sent, whole or not at all (a crash mid-write would lose it all). Under the runtime's lock,
/// so that an older list never lands after a newer one.
fn keep_pending(path: &std::path::Path, pending: &[Sample]) {
    if let Err(e) = orbiters_core::write_atomic(path, serde_json::to_string(pending).unwrap_or_default().as_bytes()) {
        eprintln!("world performance: could not keep the summaries waiting: {e}");
    }
}

/// The runtime's waiting list for the file `path`, read when it was another's (an account switch). The first time an
/// account's file is opened, a list of the version before (one file for all) becomes this account's when it is younger
/// than a day, and goes either way.
fn pending_for<'a>(rt: &'a mut PerformanceRuntime, path: &std::path::Path) -> &'a mut Vec<Sample> {
    if rt.pending.as_ref().is_none_or(|(p, _)| p != path) {
        let mut list = read_pending(path);
        let legacy = legacy_pending_file();
        if list.is_empty() {
            let young = std::fs::metadata(&legacy).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(24 * 3600));
            if young {
                list = read_pending(&legacy);
                keep_pending(path, &list);
            }
        }
        let _ = std::fs::remove_file(&legacy);
        rt.pending = Some((path.to_path_buf(), list));
    }
    &mut rt.pending.as_mut().expect("just set").1
}

/// Orbiters refused the batch for good (a 4xx that a retry does not change): it goes, or it would block every newer
/// summary for a day. Not a sign-in, a rate or a time limit (401, 403, 408, 429), nor a failure to reach Orbiters, nor a
/// 5xx: those wait for the next try.
fn refused_for_good(error: &anyhow::Error) -> bool {
    error.downcast_ref::<orbiters_core::account::ApiError>().is_some_and(|e| (400..500).contains(&e.status) && !matches!(e.status, 401 | 403 | 408 | 429))
}

fn utc(unix: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix as i64).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_default()
}

/// A minute's summary from its frame intervals: the frame rate over the minute, and that of its slowest 5 % of frames.
pub fn summarize(intervals: &[f32]) -> Option<(u32, f32, f32, f32)> {
    let total_ms: f32 = intervals.iter().sum();
    if intervals.is_empty() || total_ms / 1000.0 < MIN_SECONDS {
        return None;
    }
    let mut sorted = intervals.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p95 = sorted[((sorted.len() as f32 * 0.95).ceil() as usize).clamp(1, sorted.len()) - 1];
    let fps_mean = intervals.len() as f32 / (total_ms / 1000.0);
    let seconds = (total_ms / 1000.0).round().clamp(1.0, 60.0) as u32;
    Some((seconds, (fps_mean * 10.0).round() / 10.0, (1000.0 / p95 * 10.0).round() / 10.0, (p95 * 10.0).round() / 10.0))
}

impl Minute {
    fn finish(self) -> Option<Sample> {
        if self.mixed {
            return None;
        }
        let (seconds, fps_mean, fps_p05, frame_ms_p95) = summarize(&self.intervals)?;
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for avatar in self.worn.values() {
            *counts.entry(avatar.clone()).or_default() += 1;
        }
        let mut others: Vec<OtherAvatar> = counts.into_iter().map(|(name, count)| OtherAvatar { name, count }).collect();
        others.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        others.truncate(MAX_OTHERS);
        Some(Sample {
            world_id: self.world_id,
            started_utc: utc(self.start),
            seconds,
            fps_mean,
            fps_p05,
            frame_ms_p95,
            players: self.players.clamp(1, 80),
            avatar: self.avatar,
            others,
        })
    }
}

impl super::Controller {
    /// VRChat's frames from the overlay: added to this minute in this world (the setting on).
    pub fn frames(self: &Shared, chunk: FrameChunk) {
        // They pace N4N0's projected screen whether or not they are gathered for Orbiters.
        self.n4n0.frames(&chunk.intervals_ms);
        if !self.with_settings(|s| s.world_performance) {
            return;
        }
        let (world, avatar_id) = {
            let snap = self.snapshot.lock().unwrap();
            (snap.vrchat.world.clone(), snap.n4n0.avatar_id.clone())
        };
        let (Some(world_id), Some(instance), true) = (world.world_id.clone(), world.instance.clone(), world.in_world) else { return };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let start = now - now % 60;
        let place = format!("{world_id}:{instance}");
        let local = world.local_player.clone();
        let finished = {
            let mut rt = self.performance.lock().unwrap();
            rt.hardware.gpu = chunk.gpu.clone().or(rt.hardware.gpu.take());
            rt.hardware.headset = chunk.headset.clone().or(rt.hardware.headset.take());
            rt.hardware.refresh_hz = chunk.refresh_hz.or(rt.hardware.refresh_hz);
            let done = match &rt.minute {
                Some(m) if m.start != start => rt.minute.take(),
                _ => None,
            };
            let m = rt.minute.get_or_insert_with(|| Minute {
                start,
                place: place.clone(),
                world_id: world_id.clone(),
                mixed: false,
                intervals: Vec::new(),
                worn: BTreeMap::new(),
                players: 0,
                avatar: None,
            });
            if m.place != place {
                m.mixed = true;
            }
            m.intervals.extend(chunk.intervals_ms.iter().copied());
            for (player, avatar) in &world.avatars {
                if Some(player) != local.as_ref() {
                    m.worn.insert(player.clone(), avatar.clone());
                }
            }
            m.players = world.players.len() as u32;
            m.avatar = (avatar_id.is_some() || world.avatar_name.is_some()).then(|| OwnAvatar { id: avatar_id.clone(), name: world.avatar_name.clone() });
            done
        };
        if let Some(sample) = finished.and_then(Minute::finish) {
            self.performance_keep(sample);
        }
        self.performance_send_if_due(false);
    }

    /// "Share my frame rate per world" was turned off (PRIV-12): what waited to be sent is forgotten, here and on
    /// disk, and the minute being gathered too, so that turning it on later never sends what was gathered before.
    pub(super) fn performance_forget(&self) {
        let mut rt = self.performance.lock().unwrap();
        rt.pending = None;
        rt.minute = None;
        // Every account's waiting list on this computer, and the one of the version before.
        let dir = orbiters_core::paths::config_dir();
        let lists = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("world-performance-pending") && n.ends_with(".json"))
        });
        for path in lists {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("world performance: the summaries waiting could not be removed: {e}");
            }
        }
    }

    /// The waiting list's file of the account signed in now: `None` signed out (nothing is kept for nobody).
    fn performance_file(&self) -> Option<PathBuf> {
        let (server, _, user) = self.feedback_account()?;
        let user = user.filter(|u| !u.is_empty() && u.chars().all(|c| c.is_ascii_digit()))?;
        Some(pending_file_in(&orbiters_core::paths::config_dir(), server, &user))
    }

    fn performance_keep(&self, sample: Sample) {
        // Gathered signed out: sent as nobody's, so kept for nobody (it was sent later as whoever signed in, CTL2-9).
        let Some(path) = self.performance_file() else { return };
        let mut rt = self.performance.lock().unwrap();
        // Turned off while this minute was being finished: it is not kept (the switch is read under the lock that
        // forgetting takes, so it is either forgotten after this or never kept).
        if !self.with_settings(|s| s.world_performance) {
            return;
        }
        let pending = pending_for(&mut rt, &path);
        pending.push(sample);
        if pending.len() > MAX_PENDING {
            let extra = pending.len() - MAX_PENDING;
            pending.drain(..extra);
        }
        let _ = std::fs::create_dir_all(orbiters_core::paths::config_dir());
        keep_pending(&path, pending);
    }

    /// Sends what was gathered when it is time (or `now`), on a thread of its own.
    pub(super) fn performance_send_if_due(self: &Shared, now: bool) {
        let Some(path) = self.performance_file() else { return };
        let batch = {
            let mut rt = self.performance.lock().unwrap();
            pending_for(&mut rt, &path);
            // Asked with every chunk of frames: nothing is copied until it is time.
            let due = now || rt.last_sent.is_none_or(|t| t.elapsed() >= SEND_EVERY);
            if !self.with_settings(|s| s.world_performance) {
                return;
            }
            // Orbiters refuses summaries without the headset's refresh rate: they wait until the overlay reported it.
            if !due || rt.pending.as_ref().is_none_or(|(_, p)| p.is_empty()) || rt.hardware.refresh_hz.is_none() {
                return;
            }
            rt.last_sent = Some(Instant::now());
            let mut hardware = rt.hardware.clone();
            fill_host(&mut hardware);
            rt.hardware = hardware.clone();
            let samples: Vec<Sample> = rt.pending.iter().flat_map(|(_, p)| p.iter()).take(MAX_PER_REQUEST).cloned().collect();
            (samples, hardware)
        };
        let Some((api, token)) = self.account_api() else { return };
        let ctl = self.clone();
        std::thread::spawn(move || {
            let sent = send(&api, &token, &batch.0, &batch.1);
            match &sent {
                Ok(()) => {}
                Err(e) if refused_for_good(e) => eprintln!("world performance: {} summaries refused for good, dropped: {e:#}", batch.0.len()),
                Err(e) => {
                    eprintln!("world performance: {e:#}");
                    return;
                }
            }
            let mut rt = ctl.performance.lock().unwrap();
            // Turned off meanwhile: forgotten already, nothing to write back.
            if !ctl.with_settings(|s| s.world_performance) {
                return;
            }
            // Only from the list they were taken from (the account may have changed meanwhile).
            if let Some((file, pending)) = rt.pending.as_mut().filter(|(file, _)| *file == path) {
                pending.retain(|s| !batch.0.contains(s));
                keep_pending(file, pending);
            }
        });
    }
}

/// The processor's name and the memory, from Windows (the graphics card and headset come from SteamVR).
fn fill_host(hardware: &mut Hardware) {
    if hardware.cpu.is_some() && hardware.ram_gb.is_some() {
        return;
    }
    let mut system = sysinfo::System::new();
    system.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
    system.refresh_memory();
    hardware.cpu = system.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty());
    hardware.ram_gb = Some(((system.total_memory() as f64) / 1024f64.powi(3)).round() as u32);
}

fn send(api: &Api, token: &str, samples: &[Sample], hardware: &Hardware) -> anyhow::Result<()> {
    let body = json!({ "client": concat!("Orbiters - N4N0 ", env!("ORBITERS_VERSION")), "hardware": hardware, "samples": samples });
    let response = api.agent().post(&api.url("maps/performance")).header("Authorization", &format!("Bearer {token}")).send_json(&body)?;
    let _: serde_json::Value = Api::json(response)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CTL2-9: a batch Orbiters refuses for good goes; a sign-in, rate or time limit, a 5xx or no answer waits; and each
    /// account and environment has its own waiting list.
    #[test]
    fn a_refused_batch_goes_and_each_account_waits_apart() {
        let api = |status: u16| anyhow::Error::from(orbiters_core::account::ApiError { status, message: "no".into(), code: None });
        for status in [400, 404, 409, 413, 422] {
            assert!(refused_for_good(&api(status)), "{status}");
        }
        for status in [401, 403, 408, 429, 500, 502, 503] {
            assert!(!refused_for_good(&api(status)), "{status}");
        }
        assert!(!refused_for_good(&anyhow::anyhow!("connection refused")));
        let dir = std::path::Path::new("cfg");
        use orbiters_core::account::Server;
        assert_eq!(pending_file_in(dir, Server::Production, "12"), dir.join("world-performance-pending-prod-12.json"));
        assert_ne!(pending_file_in(dir, Server::Production, "12"), pending_file_in(dir, Server::Production, "13"));
        assert_ne!(pending_file_in(dir, Server::Production, "12"), pending_file_in(dir, Server::Development, "12"));
    }

    #[test]
    fn a_minute_sums_up_its_frames() {
        // 40 s at 90 fps, with 5 % of frames at 50 ms.
        let mut intervals = vec![11.1; 3420];
        intervals.extend(vec![50.0; 180]);
        let (seconds, mean, p05, p95) = summarize(&intervals).unwrap();
        assert_eq!(seconds, 47);
        assert!((mean - 3600.0 / 46.962).abs() < 0.2, "{mean}");
        assert_eq!(p95, 11.1);
        assert!((p05 - 90.1).abs() < 0.1);
        assert!(summarize(&vec![11.1; 1000]).is_none(), "11 s measured is too little");
    }

    /// Verifier (PRIV-12c): a minute finished while the share was being turned off is never kept nor sent.
    #[test]
    fn a_minute_finished_after_off_is_not_kept() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings { world_performance: false, ..Default::default() });
        // Signed in with a numeric id, so the minute reaches the switch check (signed out it is never kept anyway, CTL2-9).
        ctl.test_account_with_user("424242");
        assert!(ctl.performance_file().is_some(), "an account the waiting list is kept for");
        let m = Minute {
            start: 1_761_073_440,
            place: "wrld_a:1".into(),
            world_id: "wrld_a".into(),
            mixed: false,
            intervals: vec![11.1; 3000],
            worn: Default::default(),
            players: 1,
            avatar: None,
        };
        ctl.performance_keep(m.finish().expect("a minute measured"));
        let rt = ctl.performance.lock().unwrap();
        assert!(rt.pending.as_ref().is_none_or(|(_, p)| p.is_empty()), "not kept after Off");
    }

    #[test]
    fn other_people_are_counted_by_avatar_name_only() {
        let m = Minute {
            start: 1_761_073_440,
            place: "wrld_a:1".into(),
            world_id: "wrld_a".into(),
            mixed: false,
            intervals: vec![11.1; 3000],
            worn: [("Alice".to_string(), "Fox".to_string()), ("Bob".to_string(), "Fox".to_string()), ("Carol".to_string(), "Avali".to_string())].into(),
            players: 4,
            avatar: Some(OwnAvatar { id: Some("avtr_x".into()), name: Some("Mine".into()) }),
        };
        let sample = m.finish().unwrap();
        assert_eq!(sample.started_utc, "2025-10-21T19:04:00Z");
        assert_eq!(sample.others, vec![OtherAvatar { name: "Fox".into(), count: 2 }, OtherAvatar { name: "Avali".into(), count: 1 }]);
        let text = serde_json::to_string(&sample).unwrap();
        assert!(!text.contains("Alice") && !text.contains("Bob") && !text.contains("Carol"), "{text}");
        let mixed = Minute {
            mixed: true,
            ..Minute {
                start: 0,
                place: String::new(),
                world_id: "wrld_a".into(),
                mixed: false,
                intervals: vec![11.1; 3000],
                worn: BTreeMap::new(),
                players: 1,
                avatar: None,
            }
        };
        assert!(mixed.finish().is_none(), "a minute in two worlds is dropped");
    }
}
