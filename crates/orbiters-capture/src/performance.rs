//! Performance streams of a session (contracts `Docs/performance.md`): what the avatar did besides the frames.
//! Files are created when their first value arrives, so a session without them stays as it was.

use orbiters_contracts::performance::{self as perf, parameters};
use orbiters_core::osc::{OscValue, AVATAR_CHANGE, PARAMETERS_PREFIX};
use orbiters_core::protocol::OscSample;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// `parameters.bin` + `parameters.json`: every OSC value VRChat sent, in arrival order.
pub struct ParameterStream {
    dir: PathBuf,
    file: Option<BufWriter<File>>,
    table: parameters::Table,
    ids: HashMap<(String, Option<u8>), u16>,
    pub changes: u64,
    /// Last value of each parameter that changed while the session was paused, written on resume.
    held: BTreeMap<u16, parameters::Value>,
}

impl ParameterStream {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf(), file: None, table: parameters::Table::new(), ids: HashMap::new(), changes: 0, held: BTreeMap::new() }
    }

    /// Name in the table: the animator's parameter name for `/avatar/parameters/<name>`, else the address, with
    /// `[i]` for the arguments of messages that have several.
    fn name_of(address: &str, index: Option<u8>) -> String {
        match (address.strip_prefix(PARAMETERS_PREFIX), index) {
            (Some(name), None) => name.to_string(),
            (_, Some(i)) => format!("{address}[{i}]"),
            (None, None) => address.to_string(),
        }
    }

    fn id_of(&mut self, address: &str, index: Option<u8>, kind: parameters::Kind) -> Option<u16> {
        if let Some(id) = self.ids.get(&(address.to_string(), index)) {
            return Some(*id);
        }
        let id = u16::try_from(self.table.parameters.len()).ok()?;
        self.table.parameters.push(parameters::ParameterInfo { id, name: Self::name_of(address, index), address: address.to_string(), kind });
        self.ids.insert((address.to_string(), index), id);
        Some(id)
    }

    fn write(&mut self, change: parameters::Change) -> anyhow::Result<()> {
        if self.file.is_none() {
            let mut f = BufWriter::with_capacity(1 << 16, File::create(self.dir.join(perf::PARAMETERS_FILE))?);
            f.write_all(&parameters::header())?;
            self.file = Some(f);
        }
        self.file.as_mut().expect("open").write_all(&change.to_bytes())?;
        self.changes += 1;
        Ok(())
    }

    /// Records one sample. `epoch_ns` is the session start on [`orbiters_core::clock`]; initial values are written
    /// at time 0. While `paused`, only the last value of each parameter is kept, for [`Self::resume`].
    pub fn add(&mut self, epoch_ns: u64, sample: &OscSample, paused: bool) -> anyhow::Result<()> {
        let time_ns = if sample.initial { 0 } else { sample.at_ns.saturating_sub(epoch_ns) };
        let value = match &sample.value {
            OscValue::Float(v) => parameters::Value::Float(*v),
            OscValue::Int(v) => parameters::Value::Int(*v),
            OscValue::Bool(v) => parameters::Value::Bool(*v),
            OscValue::Text(id) => {
                if sample.address == AVATAR_CHANGE {
                    self.table.avatars.push(parameters::AvatarChange { time_ns, id: id.clone() });
                }
                return Ok(());
            }
        };
        let Some(id) = self.id_of(&sample.address, sample.index, value.kind()) else { return Ok(()) };
        if paused && !sample.initial {
            self.held.insert(id, value);
            return Ok(());
        }
        self.write(parameters::Change { time_ns, id, flags: if sample.initial { parameters::flags::INITIAL } else { 0 }, value })
    }

    /// Writes what changed during a pause, at `time_ns`.
    pub fn resume(&mut self, time_ns: u64) -> anyhow::Result<()> {
        for (id, value) in std::mem::take(&mut self.held) {
            self.write(parameters::Change { time_ns, id, flags: 0, value })?;
        }
        Ok(())
    }

    /// Flushes `parameters.bin` (readers can follow a running session).
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
        }
        Ok(())
    }

    /// Closes the stream and writes `parameters.json`; returns the number of changes and the bytes written.
    pub fn finish(mut self) -> anyhow::Result<(u64, u64)> {
        let mut bytes = 0;
        if let Some(mut f) = self.file.take() {
            f.flush()?;
            bytes += (perf::HEADER_SIZE + self.changes as usize * parameters::RECORD_SIZE) as u64;
        }
        if bytes == 0 && self.table.avatars.is_empty() {
            return Ok((0, 0));
        }
        let json = serde_json::to_vec_pretty(&self.table)?;
        std::fs::write(self.dir.join(perf::PARAMETERS_TABLE), &json)?;
        Ok((self.changes, bytes + json.len() as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at_ns: u64, address: &str, index: Option<u8>, value: OscValue, initial: bool) -> OscSample {
        OscSample { at_ns, address: address.into(), index, value, initial }
    }

    #[test]
    fn parameters_round_trip_with_initial_values_pause_and_poses() {
        let dir = std::env::temp_dir().join(format!("orbiters-perf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let epoch = 1_000_000_000;
        let mut s = ParameterStream::new(&dir);
        s.add(epoch, &sample(900, AVATAR_CHANGE, None, OscValue::Text("avtr_a".into()), true), false).unwrap();
        s.add(epoch, &sample(900, "/avatar/parameters/Viseme", None, OscValue::Int(4), true), false).unwrap();
        s.add(epoch, &sample(epoch + 10, "/avatar/parameters/v2/JawOpen", None, OscValue::Float(0.5), false), false).unwrap();
        s.add(epoch, &sample(epoch + 20, "/tracking/vrsystem/head/pose", Some(1), OscValue::Float(1.6), false), false).unwrap();
        // Paused: only the last value survives, written on resume.
        s.add(epoch, &sample(epoch + 30, "/avatar/parameters/v2/JawOpen", None, OscValue::Float(0.6), false), true).unwrap();
        s.add(epoch, &sample(epoch + 40, "/avatar/parameters/v2/JawOpen", None, OscValue::Float(0.7), false), true).unwrap();
        s.resume(50).unwrap();
        s.add(epoch, &sample(epoch + 60, "/avatar/parameters/MuteSelf", None, OscValue::Bool(true), false), false).unwrap();
        let (changes, bytes) = s.finish().unwrap();
        assert_eq!(changes, 5);

        let bin = std::fs::read(dir.join(perf::PARAMETERS_FILE)).unwrap();
        let table: parameters::Table = serde_json::from_slice(&std::fs::read(dir.join(perf::PARAMETERS_TABLE)).unwrap()).unwrap();
        assert_eq!(bytes, (bin.len() + std::fs::read(dir.join(perf::PARAMETERS_TABLE)).unwrap().len()) as u64);
        let read = parameters::read(&bin).unwrap();
        let named: Vec<(u64, &str, u8, f32)> =
            read.iter().map(|c| (c.time_ns, table.parameters[c.id as usize].name.as_str(), c.flags, c.value.as_f32())).collect();
        assert_eq!(
            named,
            vec![
                (0, "Viseme", parameters::flags::INITIAL, 4.0),
                (10, "v2/JawOpen", 0, 0.5),
                (20, "/tracking/vrsystem/head/pose[1]", 0, 1.6),
                (50, "v2/JawOpen", 0, 0.7),
                (60, "MuteSelf", 0, 1.0),
            ]
        );
        assert_eq!(table.parameters[2].address, "/tracking/vrsystem/head/pose");
        assert_eq!(table.parameters[0].kind, parameters::Kind::Int);
        assert_eq!(table.avatars, vec![parameters::AvatarChange { time_ns: 0, id: "avtr_a".into() }]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nothing_recorded_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("orbiters-perf-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(ParameterStream::new(&dir).finish().unwrap(), (0, 0));
        assert!(!dir.join(perf::PARAMETERS_FILE).exists() && !dir.join(perf::PARAMETERS_TABLE).exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
