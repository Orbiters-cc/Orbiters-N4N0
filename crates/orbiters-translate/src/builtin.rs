//! The translator built into the app, for a computer with no AI app: Meta's M2M100 418M (MIT), one model for 100
//! languages, int8 ONNX (Xenova's export), through the ONNX Runtime the app already ships, on the processor. Downloaded the
//! first time it is needed (632 MB). Measured on this computer (i7-12700K, two threads, 2026-11-13): 0.2 to 1 s a line,
//! greedy; good on simple signs and chat, weak on dense notices ("関係者以外立入禁止" came out "Prohibited entrance outside
//! of the entity"). An AI app on the computer or the network translates better and is preferred when one answers.
//!
//! The tokenizer is SentencePiece's BPE, read from its model file: pieces merged pair by pair, the highest-scoring merge
//! first, as SentencePiece does; their ids are M2M100's `vocab.json`'s. Generation is greedy with the decoder's cache.

use crate::models::ModelFile;
use crate::provider::{Batch, Failure, Translator};
use std::collections::HashMap;

macro_rules! xenova {
    ($file:literal) => {
        concat!("https://huggingface.co/Xenova/m2m100_418M/resolve/", "9c374f0b7aca709787cea97b047bfbbd1559d177", "/", $file)
    };
}

pub const FILES: [ModelFile; 4] = [
    ModelFile { name: "m2m100-418m-encoder-int8.onnx", url: xenova!("onnx/encoder_model_quantized.onnx"), size: 287856370, sha256: "13a94e354a9140764eb81102d77d3ec6952d796e6f113c651eeb3c3443da0386" },
    ModelFile { name: "m2m100-418m-decoder-int8.onnx", url: xenova!("onnx/decoder_model_merged_quantized.onnx"), size: 344128178, sha256: "007654bcabb6cea6fd3bde34ce933137b431330b3755781145d7b6906270b45a" },
    ModelFile { name: "m2m100-418m-sentencepiece.model", url: xenova!("sentencepiece.bpe.model"), size: 2423393, sha256: "d8f7c76ed2a5e0822be39f0a4f95a55eb19c78f4593ce609e2edbc2aea4d380a" },
    ModelFile { name: "m2m100-418m-vocab.json", url: xenova!("vocab.json"), size: 3708092, sha256: "b6e77e474aeea8f441363aca7614317c06381f3eacfe10fb9856d5081d1074cc" },
];

/// M2M100's languages, in the order their tokens follow the vocabulary.
const LANGUAGES: &[&str] = &[
    "af", "am", "ar", "ast", "az", "ba", "be", "bg", "bn", "br", "bs", "ca", "ceb", "cs", "cy", "da", "de", "el", "en", "es", "et", "fa", "ff", "fi", "fr", "fy", "ga", "gd", "gl", "gu", "ha", "he", "hi", "hr", "ht", "hu", "hy", "id", "ig", "ilo", "is", "it", "ja", "jv", "ka", "kk", "km", "kn", "ko", "lb", "lg", "ln", "lo", "lt", "lv", "mg", "mk", "ml", "mn", "mr", "ms", "my", "ne", "nl", "no", "ns", "oc", "or", "pa", "pl", "ps", "pt", "ro", "ru", "sd", "si", "sk", "sl", "sq", "sr", "ss", "su", "sv", "sw", "ta", "th", "tl", "tn", "tr", "uk", "ur", "uz", "vi", "wo", "xh", "yi", "yo", "zh", "zu",
];
const EOS: i64 = 2;
const UNK: i64 = 3;
/// Tokens a translation may take at most: a line on a sign.
const MOST_TOKENS: usize = 64;

/// SentencePiece's BPE pieces and their scores, from a `.model` file (a protobuf: field 1 holds each piece, whose field 1 is
/// its text and field 2 its score).
pub struct Pieces {
    score: HashMap<String, f32>,
}

fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

impl Pieces {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut score = HashMap::new();
        let mut at = 0;
        while at < bytes.len() {
            let key = varint(bytes, &mut at)?;
            let (field, wire) = (key >> 3, key & 7);
            match wire {
                0 => {
                    varint(bytes, &mut at)?;
                }
                1 => at += 8,
                5 => at += 4,
                2 => {
                    let len = varint(bytes, &mut at)? as usize;
                    let body = bytes.get(at..at + len)?;
                    at += len;
                    if field == 1 {
                        let (mut text, mut s, mut i) = (None, 0f32, 0);
                        while i < body.len() {
                            let k = varint(body, &mut i)?;
                            match (k >> 3, k & 7) {
                                (1, 2) => {
                                    let l = varint(body, &mut i)? as usize;
                                    text = Some(String::from_utf8_lossy(body.get(i..i + l)?).into_owned());
                                    i += l;
                                }
                                (2, 5) => {
                                    s = f32::from_le_bytes(body.get(i..i + 4)?.try_into().ok()?);
                                    i += 4;
                                }
                                (_, 0) => {
                                    varint(body, &mut i)?;
                                }
                                (_, 2) => {
                                    let l = varint(body, &mut i)? as usize;
                                    i += l;
                                }
                                (_, 5) => i += 4,
                                (_, 1) => i += 8,
                                _ => return None,
                            }
                        }
                        if let Some(t) = text {
                            score.insert(t, s);
                        }
                    }
                }
                _ => return None,
            }
        }
        (!score.is_empty()).then_some(Self { score })
    }

    /// A text as pieces: spaces as "▁", one before the text, then the best-scoring merges first.
    pub fn encode(&self, text: &str) -> Vec<String> {
        let normal = format!("▁{}", text.split_whitespace().collect::<Vec<_>>().join("▁"));
        let mut parts: Vec<String> = normal.chars().map(|c| c.to_string()).collect();
        loop {
            let mut best: Option<(usize, f32)> = None;
            for i in 0..parts.len().saturating_sub(1) {
                let merged = format!("{}{}", parts[i], parts[i + 1]);
                if let Some(&s) = self.score.get(&merged) {
                    if best.is_none_or(|(_, b)| s > b) {
                        best = Some((i, s));
                    }
                }
            }
            let Some((i, _)) = best else { break };
            let next = parts.remove(i + 1);
            parts[i].push_str(&next);
        }
        parts
    }
}

pub struct Builtin {
    encoder: ort::session::Session,
    decoder: ort::session::Session,
    pieces: Pieces,
    ids: HashMap<String, i64>,
    words: HashMap<i64, String>,
    /// The decoder's cache inputs (`past_key_values.N.decoder.key` …) and its heads and head size.
    past: Vec<String>,
    heads: usize,
    head_size: usize,
}

impl Builtin {
    pub fn installed() -> bool {
        FILES.iter().all(crate::models::present)
    }

    pub fn load(threads: usize) -> anyhow::Result<Self> {
        let path = |f: &ModelFile| crate::models::path(f);
        anyhow::ensure!(Self::installed(), "the built-in translator is not downloaded"); // i18n: not shown (log detail)
        let encoder = crate::ocr::session(&path(&FILES[0]), threads)?;
        let decoder = crate::ocr::session(&path(&FILES[1]), threads)?;
        let pieces = Pieces::parse(&std::fs::read(path(&FILES[2]))?).ok_or_else(|| anyhow::anyhow!("unreadable SentencePiece model"))?; // i18n: not shown (log detail)
        let ids: HashMap<String, i64> = serde_json::from_str(&std::fs::read_to_string(path(&FILES[3]))?)?;
        let words = ids.iter().map(|(k, v)| (*v, k.clone())).collect();
        let past: Vec<String> = decoder.inputs.iter().map(|i| i.name.clone()).filter(|n| n.starts_with("past_key_values")).collect();
        Ok(Self { encoder, decoder, pieces, ids, words, past, heads: 16, head_size: 64 })
    }

    fn language(&self, tag: &str) -> Option<i64> {
        let primary = crate::language::primary(tag);
        LANGUAGES.iter().position(|l| *l == primary).map(|i| self.ids.len() as i64 + i as i64)
    }

    /// One line, greedy.
    pub fn line(&mut self, text: &str, source: &str, target: &str) -> anyhow::Result<String> {
        use ort::value::Tensor;
        let e = |e: ort::Error| anyhow::anyhow!("{e}");
        let (Some(src), Some(tgt)) = (self.language(source), self.language(target)) else { anyhow::bail!("language not known to the model") }; // i18n: not shown (log detail)
        let mut input: Vec<i64> = vec![src];
        input.extend(self.pieces.encode(text).iter().map(|p| *self.ids.get(p).unwrap_or(&UNK)));
        input.push(EOS);
        let n = input.len();
        let mask = vec![1i64; n];
        let hidden = {
            let outputs = self
                .encoder
                .run(ort::inputs!["input_ids" => Tensor::from_array(([1usize, n], input)).map_err(e)?, "attention_mask" => Tensor::from_array(([1usize, n], mask.clone())).map_err(e)?])
                .map_err(e)?;
            let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(e)?;
            (shape.iter().map(|d| *d as usize).collect::<Vec<_>>(), values.to_vec())
        };
        let mut past: HashMap<String, ort::value::DynValue> = HashMap::new();
        // The first step reads no cache (use_cache_branch false), but its inputs need a shape: ONNX Runtime takes no
        // zero-length tensor from data, so one step of zeros stands in.
        for name in &self.past {
            past.insert(name.clone(), Tensor::<f32>::from_array(([1usize, self.heads, 1, self.head_size], vec![0f32; self.heads * self.head_size])).map_err(e)?.into_dyn());
        }
        let mut out: Vec<i64> = Vec::new();
        let mut feed: Vec<i64> = vec![EOS, tgt];
        for step in 0..MOST_TOKENS {
            let mut inputs: Vec<(String, ort::session::SessionInputValue)> = vec![
                ("input_ids".into(), Tensor::from_array(([1usize, feed.len()], feed.clone())).map_err(e)?.into()),
                ("encoder_attention_mask".into(), Tensor::from_array(([1usize, n], mask.clone())).map_err(e)?.into()),
                ("encoder_hidden_states".into(), Tensor::from_array((hidden.0.clone(), hidden.1.clone())).map_err(e)?.into()),
                ("use_cache_branch".into(), Tensor::from_array(([1usize], vec![step > 0])).map_err(e)?.into()),
            ];
            // Borrowed: the encoder's part of the cache stays for every later step.
            for name in &self.past {
                inputs.push((name.clone(), (&past[name]).into()));
            }
            let mut outputs = self.decoder.run(inputs).map_err(e)?;
            let next = {
                let (shape, logits) = outputs[0].try_extract_tensor::<f32>().map_err(e)?;
                let vocab = shape[shape.len() - 1] as usize;
                let last = &logits[logits.len() - vocab..];
                last.iter().enumerate().fold((0usize, f32::MIN), |b, (i, &v)| if v > b.1 { (i, v) } else { b }).0 as i64
            };
            for name in &self.past {
                let present = name.replace("past_key_values", "present");
                // The encoder's part of the cache is made once, on the first step; the decoder's grows each step.
                if step == 0 || name.contains(".decoder.") {
                    past.insert(name.clone(), outputs.remove(&present).ok_or_else(|| anyhow::anyhow!("no {present}"))?); // i18n: not shown (log detail)
                }
            }
            drop(outputs);
            if next == EOS {
                break;
            }
            out.push(next);
            feed = vec![next];
        }
        let text: String = out.iter().filter_map(|i| self.words.get(i)).cloned().collect();
        Ok(text.replace('▁', " ").trim().to_string())
    }
}

impl Translator for Builtin {
    fn warm_up(&mut self) {
        let _ = self.line("OK", "en", "fr");
    }

    fn translate(&mut self, batch: &Batch) -> Result<Vec<String>, Failure> {
        batch
            .lines
            .iter()
            .map(|l| {
                let source = l.source.clone().or_else(|| crate::language::guess(&l.text).map(str::to_string)).unwrap_or_else(|| "en".into());
                self.line(&l.text, &source, &batch.target).map_err(|e| Failure::BadAnswer(e.to_string()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_merge_best_first() {
        let mut score = HashMap::new();
        for (p, s) in [("▁h", -1.0), ("el", -2.0), ("▁hel", -0.5), ("lo", -3.0), ("▁hello", -0.1), ("▁hell", -0.3)] {
            score.insert(p.to_string(), s);
        }
        let pieces = Pieces { score };
        assert_eq!(pieces.encode("hello"), vec!["▁hello"]);
        // "▁h" (-1) before "lo" (-3); "▁he" and "e▁" are no pieces.
        assert_eq!(pieces.encode("he  lo"), vec!["▁h", "e", "▁", "lo"]);
    }
}
