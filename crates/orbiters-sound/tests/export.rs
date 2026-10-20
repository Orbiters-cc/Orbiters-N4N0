//! Writes the rendered sounds as 32-bit float WAVs to compare by ear or with React Bits' exports (run by hand):
//! ORBITERS_SOUND_OUT=<folder> cargo test -p orbiters-sound --release --test export -- --ignored

use orbiters_sound::{render, sounds, RATE};
use std::io::Write;

#[test]
#[ignore]
fn export_wavs() {
    let dir = std::path::PathBuf::from(std::env::var("ORBITERS_SOUND_OUT").expect("ORBITERS_SOUND_OUT"));
    for (name, recipe) in [("reveal", &sounds::REVEAL), ("rocker", &sounds::ROCKER)] {
        let s = render(recipe, RATE);
        let data: Vec<u8> = s.frames.iter().flat_map(|f| f.iter().flat_map(|v| v.to_le_bytes())).collect();
        let mut w = std::fs::File::create(dir.join(format!("{name}.wav"))).expect("file");
        let header = [
            b"RIFF".to_vec(),
            (36 + data.len() as u32).to_le_bytes().to_vec(),
            b"WAVEfmt ".to_vec(),
            16u32.to_le_bytes().to_vec(),
            3u16.to_le_bytes().to_vec(),
            2u16.to_le_bytes().to_vec(),
            RATE.to_le_bytes().to_vec(),
            (RATE * 8).to_le_bytes().to_vec(),
            8u16.to_le_bytes().to_vec(),
            32u16.to_le_bytes().to_vec(),
            b"data".to_vec(),
            (data.len() as u32).to_le_bytes().to_vec(),
        ]
        .concat();
        w.write_all(&header).and_then(|_| w.write_all(&data)).expect("write");
    }
}
