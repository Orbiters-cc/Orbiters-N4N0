//! The text reader's model files, downloaded the first time Translation is turned on into
//! `%LOCALAPPDATA%\Orbiters\Models\translate` (as the voice models are): PaddleOCR's PP-OCR models (Apache-2.0), from
//! PaddlePaddle's own Hugging Face repositories at a fixed revision, each checked against its size and SHA-256. A file
//! that is not exactly the one expected is refused. Partial downloads resume.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// Its name in the models folder.
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

macro_rules! hf {
    ($repo:literal, $rev:literal, $file:literal) => {
        concat!("https://huggingface.co/PaddlePaddle/", $repo, "/resolve/", $rev, "/", $file)
    };
}

/// PP-OCRv6 small text detector (any script), 9.4 MiB. Measured on VRChat eye pictures (`tests/bench.rs`, 2026-11-13):
/// faster than PP-OCRv5 mobile's (143 against 212 ms for the middle of an eye at 960 pixels, two threads) and it finds
/// more lines.
pub const DETECTOR: ModelFile = ModelFile {
    name: "ppocr-v6-small-det.onnx",
    url: hf!("PP-OCRv6_small_det_onnx", "28fe5895c24fd108c19eb3e8479f4ab385fbfc62", "inference.onnx"),
    size: 9880512,
    sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
};

/// PP-OCRv6 small recogniser: Chinese (both forms), Japanese, English and 46 Latin-script languages.
pub const MAIN: [ModelFile; 2] = [
    ModelFile {
        name: "ppocr-v6-small-rec.onnx",
        url: hf!("PP-OCRv6_small_rec_onnx", "b8f84f0b80c529de40b4fbb3544b84fa7233a513", "inference.onnx"),
        size: 21159378,
        sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
    },
    ModelFile {
        name: "ppocr-v6-small-rec.yml",
        url: hf!("PP-OCRv6_small_rec_onnx", "b8f84f0b80c529de40b4fbb3544b84fa7233a513", "inference.yml"),
        size: 150579,
        sha256: "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1",
    },
];

/// PP-OCRv5 Korean recogniser (Korean and English).
pub const KOREAN: [ModelFile; 2] = [
    ModelFile {
        name: "ppocr-v5-korean-rec.onnx",
        url: hf!("korean_PP-OCRv5_mobile_rec_onnx", "5c6f574b8e2230adf4287b33e736d71b9fabd28e", "inference.onnx"),
        size: 13418787,
        sha256: "92f0b7785e64fc9090106a241cf4c1eb97472824558272751b88a2a4476d3a08",
    },
    ModelFile {
        name: "ppocr-v5-korean-rec.yml",
        url: hf!("korean_PP-OCRv5_mobile_rec_onnx", "5c6f574b8e2230adf4287b33e736d71b9fabd28e", "inference.yml"),
        size: 96039,
        sha256: "f757fa1c40e99edcf27e9cce879b93eb2a51fa46f5ef39095689b8c37dd75998",
    },
];

/// PP-OCRv5 East Slavic recogniser (Russian, Ukrainian, Belarusian and English).
pub const CYRILLIC: [ModelFile; 2] = [
    ModelFile {
        name: "ppocr-v5-eslav-rec.onnx",
        url: hf!("eslav_PP-OCRv5_mobile_rec_onnx", "9a32171fc5718746875e1a261818884517975013", "inference.onnx"),
        size: 7887627,
        sha256: "b3018ef2b09a0250b6e0c8e871c927098363e5fd4df890cc68e8358eb0aaf1bd",
    },
    ModelFile {
        name: "ppocr-v5-eslav-rec.yml",
        url: hf!("eslav_PP-OCRv5_mobile_rec_onnx", "9a32171fc5718746875e1a261818884517975013", "inference.yml"),
        size: 4538,
        sha256: "025039bac23eb4a308efcefa4d58eab3af440767815c6ba6938468bf6353ee5a",
    },
];

/// Every file the text reader needs.
pub fn all() -> Vec<ModelFile> {
    let mut v = vec![DETECTOR];
    v.extend(MAIN);
    v.extend(KOREAN);
    v.extend(CYRILLIC);
    v
}

/// Where they go (`ORBITERS_TRANSLATE_MODELS` names another folder, for tests and the lab).
pub fn folder() -> PathBuf {
    std::env::var_os("ORBITERS_TRANSLATE_MODELS").map(PathBuf::from).unwrap_or_else(|| orbiters_core::paths::models_dir().join("translate"))
}

pub fn path(file: &ModelFile) -> PathBuf {
    folder().join(file.name)
}

/// Whether a file is there whole (by its size: the digest was checked when it arrived).
pub fn present(file: &ModelFile) -> bool {
    std::fs::metadata(path(file)).is_ok_and(|m| m.len() == file.size)
}

pub fn missing() -> Vec<ModelFile> {
    all().into_iter().filter(|f| !present(f)).collect()
}

/// Bytes still to download.
pub fn missing_bytes() -> u64 {
    missing().iter().map(|f| f.size).sum()
}

/// Downloads what is missing, saying how far it got (bytes done, bytes in all) as it goes. Stops early, keeping what
/// arrived for next time, when `stop` is set.
pub fn install(stop: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<()> {
    install_files(&all(), stop, progress)
}

/// [`install`] for some files (the built-in translator's: `crate::builtin::FILES`).
pub fn install_files(files: &[ModelFile], stop: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<()> {
    let todo: Vec<ModelFile> = files.iter().filter(|f| !present(f)).copied().collect();
    let total: u64 = todo.iter().map(|f| f.size).sum();
    let mut done = 0u64;
    std::fs::create_dir_all(folder())?;
    let agent = orbiters_core::http_agent(Some(std::time::Duration::from_secs(15)), None);
    for file in todo {
        download(&agent, &file, &path(&file), stop, &mut |n| progress(done + n, total))?;
        done += file.size;
        progress(done, total);
    }
    Ok(())
}

/// One file into `to`, through `to.part`, resumed from what that holds; checked before it takes its name.
pub fn download(agent: &ureq::Agent, file: &ModelFile, to: &Path, stop: &AtomicBool, progress: &mut dyn FnMut(u64)) -> anyhow::Result<()> {
    let part = to.with_extension("part");
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > file.size {
        std::fs::remove_file(&part)?;
        have = 0;
    }
    if have < file.size {
        let mut request = agent.get(file.url);
        if have > 0 {
            request = request.header("Range", &format!("bytes={have}-"));
        }
        let mut response = request.call()?;
        let status = response.status().as_u16();
        if status == 200 && have > 0 {
            // The server sends it whole: start over.
            have = 0;
        } else if status != 200 && status != 206 {
            anyhow::bail!("HTTP {status} for {}", file.url); // i18n: not shown (log detail)
        }
        let mut out = std::fs::OpenOptions::new().create(true).append(have > 0).write(true).truncate(have == 0).open(&part)?;
        let mut body = response.body_mut().with_config().limit(file.size + 1).reader();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            if stop.load(Ordering::Relaxed) {
                anyhow::bail!("stopped"); // i18n: not shown (log detail)
            }
            let n = body.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            have += n as u64;
            progress(have);
            anyhow::ensure!(have <= file.size, "{} is larger than expected", file.name); // i18n: not shown (log detail)
        }
        out.flush()?;
    }
    anyhow::ensure!(have == file.size, "{} arrived incomplete ({have} of {} bytes)", file.name, file.size); // i18n: not shown (log detail)
    let digest = sha256(&part)?;
    if digest != file.sha256 {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("{} is not the file expected (SHA-256 {digest})", file.name); // i18n: not shown (log detail)
    }
    std::fs::rename(&part, to)?;
    Ok(())
}

fn sha256(path: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_pinned() {
        for f in all() {
            assert!(f.url.starts_with("https://huggingface.co/PaddlePaddle/") && f.url.contains("/resolve/"), "{}", f.url);
            assert!(!f.url.contains("/resolve/main/"), "a fixed revision, not main: {}", f.url);
            assert_eq!(f.sha256.len(), 64);
            assert!(f.size > 1000);
        }
        let names: std::collections::HashSet<_> = all().iter().map(|f| f.name).collect();
        assert_eq!(names.len(), all().len());
    }
}
