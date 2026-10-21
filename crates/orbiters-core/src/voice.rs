//! Orb's ears: the voice worker (`orbiters-voice`) and its protocol, JSON lines with commands on its stdin and events on
//! its stdout. The worker opens the microphone only while Orb listens (or while the wake word is on), finds sentences
//! with Silero VAD and transcribes them on this computer with NVIDIA Parakeet (through sherpa-onnx); nothing it hears
//! is kept or sent anywhere by the worker itself.

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
/// Bytes to download, for the progress shown before the server says.
pub const DOWNLOAD_BYTES: u64 = 2_313_101 + 487_170_055;
/// Audio the worker works on.
pub const SAMPLE_RATE: u32 = 16_000;

/// Whether the models are complete in `models`.
pub fn installed(models: &Path) -> bool {
    models.join(VAD_FILE).is_file() && ASR_FILES.iter().all(|f| models.join(ASR_DIR).join(f).is_file())
}

/// Service → voice worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceCommand {
    /// Loads the models from `models`; `mic` is a capture endpoint id (`None`: the Windows default).
    Start {
        models: PathBuf,
        mic: Option<String>,
        wake_word: bool,
    },
    /// Listens for a sentence for Orb: until [`VoiceCommand::Finish`] when `hold`, else until the sentence ends.
    Listen {
        hold: bool,
    },
    /// The held sentence is complete: transcribe it.
    Finish,
    /// Forget the sentence being listened to.
    Cancel,
    /// Transcribe every sentence heard (the service looks for the wake word), or stop.
    WakeWord {
        on: bool,
    },
    /// Downloads the models into `models`, then quits.
    Install {
        models: PathBuf,
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
    /// The microphone's level while Orb listens (0 to 1), about ten times a second.
    Level {
        level: f32,
    },
    /// A transcribed sentence: the one Orb listened for (`for_orb`), or one heard while the wake word is on. An empty
    /// text for Orb means nothing was said.
    Heard {
        text: String,
        for_orb: bool,
        seconds: f32,
        decode_ms: u32,
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
        let c = VoiceCommand::Start { models: PathBuf::from("m"), mic: None, wake_word: true };
        assert_eq!(serde_json::from_str::<VoiceCommand>(&c.to_line()).unwrap(), c);
        let e = VoiceEvent::Heard { text: "Orb, mets mon sweat".into(), for_orb: false, seconds: 1.5, decode_ms: 120 };
        assert_eq!(serde_json::from_str::<VoiceEvent>(&e.to_line()).unwrap(), e);
        assert!(!installed(Path::new("does-not-exist")));
    }
}
