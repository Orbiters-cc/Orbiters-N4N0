//! What a capture's tracking said of the person who made it, as far as a view of it stands on it: how high the avatar's eyes
//! were and where its hips were, at the moments it can be looked at from (its keyframes). Facts about the recording, so they
//! stay with it ([`FILE`], in the session folder) when freeing space takes the tracking away ([`keep`], which
//! crate::cleanup calls before it deletes any of it): a view or a share of it starts where it did. The tracking itself
//! answers while it is there.

use crate::contracts::performance::{self, parameters, skeleton_frames};
use anyhow::Context as _;
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The file of a session that keeps what was measured.
pub const FILE: &str = "measured.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Measured {
    /// When (the session's clock), and the avatar's eye height in metres.
    eye_heights: Vec<(u64, f32)>,
    /// When, and the avatar's hips (Unity world).
    hips: Vec<(u64, [f32; 3])>,
}

/// The avatar's eye height at `time_ns` (None: the start): the last value VRChat sent before it (else the first one after),
/// in metres.
pub fn eye_height(session: &Path, time_ns: Option<u64>) -> Option<f32> {
    let values = tracked_eye_heights(session).ok().flatten().unwrap_or_else(|| stored(session).eye_heights);
    Some(values[eye_at(&values, time_ns.unwrap_or(0))?].1)
}

/// The avatar's hips (Unity world) in the capture kit's frame nearest `time_ns` (None: the start).
pub fn hips(session: &Path, time_ns: Option<u64>) -> Option<[f32; 3]> {
    let frames = tracked_hips(session).ok().flatten().unwrap_or_else(|| stored(session).hips);
    Some(frames[hips_at(&frames, time_ns.unwrap_or(0))?].1)
}

/// Keeps what the tracking of `session` says now in [`FILE`], on the disk when it returns. What it no longer says (a
/// stream gone or damaged) stays as it was kept before; a session that never had tracking gets no file. An error is a
/// stream that is there and could not be read, or a file that could not be written: the tracking must then stay.
pub fn keep(session: &Path) -> anyhow::Result<()> {
    let before = stored(session);
    let mut times = vec![0];
    times.extend(keyframe_times(session)?);
    let eye_heights = match tracked_eye_heights(session)? {
        Some(v) if !v.is_empty() => chosen(&v, &times, eye_at),
        _ => before.eye_heights.clone(),
    };
    let hips = match tracked_hips(session)? {
        Some(v) if !v.is_empty() => chosen(&v, &times, hips_at),
        _ => before.hips.clone(),
    };
    let now = Measured { eye_heights, hips };
    if now == before {
        return Ok(());
    }
    let path = session.join(FILE);
    crate::write_atomic(&path, &serde_json::to_vec(&now)?).with_context(|| tr!("writing {path}", path = path.display()))
}

fn stored(session: &Path) -> Measured {
    std::fs::read(session.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Which of `values` a moment reads: of the eye heights, the last sent by then, else the first.
fn eye_at(values: &[(u64, f32)], t: u64) -> Option<usize> {
    values.iter().rposition(|(at, _)| *at <= t).or((!values.is_empty()).then_some(0))
}

/// Of the hips, the nearest.
fn hips_at(values: &[(u64, [f32; 3])], t: u64) -> Option<usize> {
    (0..values.len()).min_by_key(|&i| values[i].0.abs_diff(t))
}

/// Those of `values` that some of `times` reads, in their order: every one of `times` reads the same from them.
fn chosen<T: Copy>(values: &[(u64, T)], times: &[u64], at: fn(&[(u64, T)], u64) -> Option<usize>) -> Vec<(u64, T)> {
    let read: std::collections::BTreeSet<usize> = times.iter().filter_map(|&t| at(values, t)).collect();
    read.into_iter().map(|i| values[i]).collect()
}

/// When the keyframes of `session` were taken.
fn keyframe_times(session: &Path) -> anyhow::Result<Vec<u64>> {
    use crate::contracts::frame_record::record_flags;
    let Some(bytes) = read(session, crate::contracts::manifest::FRAMES_FILE)? else { return Ok(Vec::new()) };
    let records = bytes.as_chunks::<{ crate::contracts::frame_record::RECORD_SIZE }>().0;
    Ok(records.iter().filter_map(|c| crate::contracts::frame_record::FrameRecord::from_bytes(c).ok()).filter(|r| r.has(record_flags::KEYFRAME)).map(|r| r.capture_time_ns).collect())
}

/// A file of `session`, none when it is not there.
fn read(session: &Path, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let path = session.join(name);
    match std::fs::read(&path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| tr!("reading {path}", path = path.display())),
    }
}

/// The eye heights VRChat sent, none without the streams or with ones that do not read as such.
fn tracked_eye_heights(session: &Path) -> anyhow::Result<Option<Vec<(u64, f32)>>> {
    let (Some(table), Some(changes)) = (read(session, performance::PARAMETERS_TABLE)?, read(session, performance::PARAMETERS_FILE)?) else { return Ok(None) };
    let (Ok(table), Ok(changes)) = (serde_json::from_slice::<parameters::Table>(&table), parameters::read(&changes)) else { return Ok(None) };
    // VRChat's built-in parameter first, then the OSC address it also sends.
    let of = |name: &str| -> Vec<(u64, f32)> {
        let Some(id) = table.parameters.iter().find(|p| p.name == name).map(|p| p.id) else { return Vec::new() };
        changes.iter().filter(|c| c.id == id).map(|c| (c.time_ns, c.value.as_f32())).filter(|(_, v)| v.is_finite() && (0.1..=50.0).contains(v)).collect()
    };
    Ok(Some(["EyeHeightAsMeters", "/avatar/eyeheight"].into_iter().map(of).find(|v| !v.is_empty()).unwrap_or_default()))
}

/// The hips of every frame of the capture kit's band, none without the stream or with one that does not read as such.
fn tracked_hips(session: &Path) -> anyhow::Result<Option<Vec<(u64, [f32; 3])>>> {
    let Some(bytes) = read(session, performance::SKELETON_FILE)? else { return Ok(None) };
    let Ok(frames) = skeleton_frames::read(&bytes) else { return Ok(None) };
    Ok(Some(frames.iter().filter(|f| f.hips.iter().all(|v| v.is_finite())).map(|f| (f.time_ns, f.hips)).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::frame_record::{record_flags, FrameRecord};

    fn session(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-measured-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn track(dir: &Path, eyes: &[(u64, f32)], hips: &[(u64, f32)]) {
        let mut table = parameters::Table::new();
        table.parameters.push(parameters::ParameterInfo { id: 3, name: "EyeHeightAsMeters".into(), address: "/avatar/parameters/EyeHeightAsMeters".into(), kind: parameters::Kind::Float });
        std::fs::write(dir.join(performance::PARAMETERS_TABLE), serde_json::to_vec(&table).unwrap()).unwrap();
        let mut bytes = parameters::header().to_vec();
        for &(time_ns, v) in eyes {
            bytes.extend(parameters::Change { time_ns, id: 3, flags: 0, value: parameters::Value::Float(v) }.to_bytes());
        }
        std::fs::write(dir.join(performance::PARAMETERS_FILE), bytes).unwrap();
        let mut bytes = skeleton_frames::header().to_vec();
        for &(time_ns, y) in hips {
            bytes.extend(skeleton_frames::Frame { time_ns, game_time: 0.0, avatar: 1, hips: [0.1, y, 0.3], rotations: [None; crate::contracts::skeleton::BONES] }.to_bytes());
        }
        std::fs::write(dir.join(performance::SKELETON_FILE), bytes).unwrap();
    }

    fn untrack(dir: &Path) {
        for f in [performance::PARAMETERS_TABLE, performance::PARAMETERS_FILE, performance::SKELETON_FILE] {
            std::fs::remove_file(dir.join(f)).unwrap();
        }
    }

    /// Kept, every keyframe reads to the bit what the tracking said, from far fewer values than it had.
    #[test]
    fn what_was_measured_answers_as_the_tracking_did() {
        let dir = session("answers");
        // Values all over, the same every run.
        let mut state = 8u64;
        let mut next = || {
            state = state.wrapping_mul(0x5851_F42D_4C95_7F2D).wrapping_add(0x1405_7B7E_F767_814F);
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        let eyes: Vec<(u64, f32)> = (1..40u64).map(|i| (i * 1000, 0.3 + next())).collect();
        let hips: Vec<(u64, f32)> = (0..4000u64).map(|i| (i * 10, next() * 3.0 - 4.0)).collect();
        track(&dir, &eyes, &hips);
        let times: Vec<u64> = (0..30).map(|_| (next() * 45_000.0) as u64).collect();
        let records: Vec<u8> = times.iter().flat_map(|&t| FrameRecord { flags: record_flags::KEYFRAME | record_flags::STRIP_OK, capture_time_ns: t, ..Default::default() }.to_bytes()).collect();
        std::fs::write(dir.join(crate::contracts::manifest::FRAMES_FILE), records).unwrap();
        let moments: Vec<Option<u64>> = times.iter().map(|&t| Some(t)).chain([None]).collect();
        let read = || moments.iter().map(|&t| (eye_height(&dir, t).map(f32::to_bits), hips_bits(hips_of(&dir, t)))).collect::<Vec<_>>();
        fn hips_of(dir: &Path, t: Option<u64>) -> Option<[f32; 3]> {
            super::hips(dir, t)
        }
        fn hips_bits(h: Option<[f32; 3]>) -> Option<[u32; 3]> {
            h.map(|h| h.map(f32::to_bits))
        }
        let before = read();
        assert!(before.iter().all(|(e, h)| e.is_some() && h.is_some()));
        keep(&dir).unwrap();
        assert_eq!(read(), before, "the tracking still answers");
        untrack(&dir);
        assert_eq!(read(), before, "and what it said after it is gone");
        assert!(stored(&dir).hips.len() <= times.len() + 1);
        // Nothing left to say: what was kept stays.
        let kept = std::fs::read(dir.join(FILE)).unwrap();
        keep(&dir).unwrap();
        assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), kept);
        // Tracking put back answers again, and takes its place when it is kept; one of the two streams keeps the other's.
        track(&dir, &[(0, 2.5)], &[(0, -1.0)]);
        assert_eq!(eye_height(&dir, None), Some(2.5));
        std::fs::remove_file(dir.join(performance::SKELETON_FILE)).unwrap();
        keep(&dir).unwrap();
        untrack_parameters(&dir);
        assert_eq!((eye_height(&dir, None), hips_bits(hips_of(&dir, None))), (Some(2.5), before.last().unwrap().1));
        fn untrack_parameters(dir: &Path) {
            for f in [performance::PARAMETERS_TABLE, performance::PARAMETERS_FILE] {
                std::fs::remove_file(dir.join(f)).unwrap();
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A capture that never had tracking measured nothing and gets no file; a stream that is there and cannot be read is an
    /// error (freeing space then keeps it).
    #[test]
    fn nothing_is_kept_of_no_tracking_and_an_unreadable_stream_is_an_error() {
        let dir = session("none");
        keep(&dir).unwrap();
        assert!(!dir.join(FILE).exists() && eye_height(&dir, None).is_none() && hips(&dir, None).is_none());
        // Not a skeleton stream: nothing to measure.
        std::fs::write(dir.join(performance::SKELETON_FILE), [7; 100]).unwrap();
        keep(&dir).unwrap();
        assert!(!dir.join(FILE).exists());
        std::fs::remove_file(dir.join(performance::SKELETON_FILE)).unwrap();
        // A folder of its name is no file to read.
        std::fs::create_dir(dir.join(performance::SKELETON_FILE)).unwrap();
        assert!(keep(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
