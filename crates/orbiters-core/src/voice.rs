//! N4N0's ears: the voice worker (`orbiters-voice`) and its protocol, JSON lines with commands on its stdin and events on
//! its stdout. The worker opens the microphone only while N4N0 listens (or while the wake word is on), finds sentences
//! with Silero VAD and transcribes them on this computer with NVIDIA Parakeet (through sherpa-onnx, loaded when a
//! sentence needs it and let go after ten idle minutes); nothing it hears is kept or sent anywhere by the worker itself.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Silero VAD's model in the models folder ([`crate::paths::models_dir`]).
pub const VAD_FILE: &str = "silero_vad_v5.onnx";
/// The speech recogniser's folder in the models folder.
pub const ASR_DIR: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
/// The recogniser's files in its folder.
pub const ASR_FILES: [&str; 4] = ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"];
/// Where the models come from: k2-fsa's releases of sherpa-onnx models (Silero VAD: MIT; Parakeet TDT 0.6B v3:
/// CC-BY-4.0, NVIDIA).
pub const VAD_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad_v5.onnx";
pub const ASR_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";

/// What a model file must be: its size and SHA-256 (lowercase hex), those of the release assets the installer was
/// written against (a release tag can be changed: a file that differs is refused, and the installer says so). The
/// SHA-256 of each download is computed while it comes and logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    pub size: u64,
    pub sha256: Option<&'static str>,
}

/// Silero VAD's model (one file, [`VAD_URL`]). GitHub keeps no digest of assets uploaded before mid-2025: this is the
/// SHA-256 of the asset as downloaded on 2026-10-22 (the copy the installer had put on this computer matched).
pub const VAD: Expected = Expected { size: 2_313_101, sha256: Some("6b99cbfd39246b6706f98ec13c7c50c6b299181f2474fa05cbc8046acc274396") };
/// Parakeet's archive ([`ASR_URL`], bzip2 tar): the SHA-256 is GitHub's own digest of the release asset.
pub const ASR_ARCHIVE: Expected = Expected { size: 487_170_055, sha256: Some("5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf") };
/// The recogniser's files ([`ASR_FILES`], in that order) as the archive holds them.
pub const ASR_SIZES: [u64; 4] = [652_184_281, 11_845_275, 6_355_277, 93_939];
/// Bytes to download, for the progress shown before the server says.
pub const DOWNLOAD_BYTES: u64 = VAD.size + ASR_ARCHIVE.size;
/// Audio the worker works on.
pub const SAMPLE_RATE: u32 = 16_000;

/// Whether the models are complete in `models`: every file there, of its size (a cut download or a truncated copy
/// is not installed, and the installer repairs it).
pub fn installed(models: &Path) -> bool {
    damaged(models).is_empty()
}

/// The model files missing from `models`, or not of their size.
pub fn damaged(models: &Path) -> Vec<String> {
    let size = |p: PathBuf| std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len());
    let mut out = Vec::new();
    if size(models.join(VAD_FILE)) != Some(VAD.size) {
        out.push(VAD_FILE.to_string());
    }
    for (name, expected) in ASR_FILES.iter().zip(ASR_SIZES) {
        if size(models.join(ASR_DIR).join(name)) != Some(expected) {
            out.push(format!("{ASR_DIR}/{name}"));
        }
    }
    out
}

/// Service → voice worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceCommand {
    /// Gets ready with the models of `models` (Silero VAD loads now, the recogniser when a sentence needs it); `mic`
    /// is a capture endpoint id (`None`: the Windows default). `wake_words` are the words that wake N4N0: a long
    /// sentence heard for the wake word is transcribed whole only when its start holds one.
    Start {
        models: PathBuf,
        mic: Option<String>,
        wake_word: bool,
        #[serde(default)]
        wake_words: Vec<String>,
    },
    /// Another microphone (`None`: the Windows default), without loading anything again.
    Mic {
        mic: Option<String>,
    },
    /// Listens for a sentence for N4N0: until [`VoiceCommand::Finish`] when `hold`, else until the sentence ends. The
    /// worker answers every listen with one [`VoiceEvent::Heard`] carrying its `id`, unless it is cancelled.
    Listen {
        hold: bool,
        #[serde(default)]
        id: u64,
    },
    /// The held sentence is complete: transcribe it.
    Finish,
    /// Forget the sentence being listened to: no [`VoiceEvent::Heard`] answers it.
    Cancel,
    /// Transcribe every sentence heard (the service looks for the wake word, `words`), or stop.
    WakeWord {
        on: bool,
        #[serde(default)]
        words: Vec<String>,
    },
    /// Downloads the models into `models`, then quits: what is missing or not of its size, or with `repair` all of it
    /// (models that are whole in size but fail to load). [`VoiceCommand::Cancel`] or [`VoiceCommand::Quit`] stop it;
    /// what was downloaded is kept and the next install goes on from there.
    Install {
        models: PathBuf,
        #[serde(default)]
        repair: bool,
    },
    Quit,
}

impl VoiceCommand {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("serializable")
    }
}

/// Voice worker → service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    /// Replaceable words for the current listen; never a committed sentence.
    Provisional { listen: u64, text: String },
    /// The models are loaded.
    Ready {
        engine: String,
        load_ms: u32,
    },
    /// The microphone opened (its name) or closed (`None`).
    Mic {
        name: Option<String>,
    },
    /// Someone started or stopped speaking.
    Speech {
        active: bool,
    },
    /// The microphone's level while N4N0 listens (0 to 1), about ten times a second.
    Level {
        level: f32,
    },
    /// A transcribed sentence: the one N4N0 listened for (`for_n4n0`, answering listen `listen`), or one heard while the
    /// wake word is on (`listen` 0). An empty text for N4N0 means nothing was said.
    Heard {
        text: String,
        for_n4n0: bool,
        seconds: f32,
        decode_ms: u32,
        #[serde(default)]
        listen: u64,
    },
    /// The model download's progress (0 to 1).
    Installing {
        fraction: f32,
    },
    Installed,
    Error {
        message: String,
    },
}

impl VoiceEvent {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let c = VoiceCommand::Start { models: PathBuf::from("m"), mic: None, wake_word: true, wake_words: vec!["nano".into()] };
        assert_eq!(serde_json::from_str::<VoiceCommand>(&c.to_line()).unwrap(), c);
        // Lines from before the wake words, the microphone switch and repairs still read.
        assert_eq!(
            serde_json::from_str::<VoiceCommand>(r#"{"type":"start","models":"m","mic":null,"wake_word":false}"#).unwrap(),
            VoiceCommand::Start { models: PathBuf::from("m"), mic: None, wake_word: false, wake_words: vec![] }
        );
        assert_eq!(
            serde_json::from_str::<VoiceCommand>(r#"{"type":"install","models":"m"}"#).unwrap(),
            VoiceCommand::Install { models: PathBuf::from("m"), repair: false }
        );
        assert_eq!(serde_json::from_str::<VoiceCommand>(r#"{"type":"wake_word","on":true}"#).unwrap(), VoiceCommand::WakeWord { on: true, words: vec![] });
        let m = VoiceCommand::Mic { mic: Some("{0.0.1.00000000}.{abc}".into()) };
        assert_eq!(serde_json::from_str::<VoiceCommand>(&m.to_line()).unwrap(), m);
        let e = VoiceEvent::Heard { text: "Nano, mets mon sweat".into(), for_n4n0: false, seconds: 1.5, decode_ms: 120, listen: 0 };
        assert_eq!(serde_json::from_str::<VoiceEvent>(&e.to_line()).unwrap(), e);
        let l = VoiceCommand::Listen { hold: true, id: 7 };
        assert_eq!(serde_json::from_str::<VoiceCommand>(&l.to_line()).unwrap(), l);
        // Lines from before listens were numbered still read.
        assert_eq!(serde_json::from_str::<VoiceCommand>(r#"{"type":"listen","hold":false}"#).unwrap(), VoiceCommand::Listen { hold: false, id: 0 });
        assert!(!installed(Path::new("does-not-exist")));
    }

    #[test]
    fn a_model_of_the_wrong_size_is_not_installed() {
        let dir = std::env::temp_dir().join(format!("orbiters-voice-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(ASR_DIR)).unwrap();
        assert_eq!(damaged(&dir).len(), 5);
        // A cut download of the VAD (the recogniser's files are too big to make here).
        let vad = std::fs::File::create(dir.join(VAD_FILE)).unwrap();
        vad.set_len(VAD.size - 1).unwrap();
        assert!(damaged(&dir).contains(&VAD_FILE.to_string()));
        vad.set_len(VAD.size).unwrap();
        drop(vad);
        let left = damaged(&dir);
        assert_eq!(left.len(), 4);
        assert!(left.iter().all(|f| f.starts_with(ASR_DIR)), "{left:?}");
        // A folder where a file belongs is not one.
        std::fs::create_dir_all(dir.join(ASR_DIR).join("tokens.txt")).unwrap();
        assert_eq!(damaged(&dir).len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
