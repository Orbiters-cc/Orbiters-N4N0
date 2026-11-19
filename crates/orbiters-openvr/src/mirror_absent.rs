//! The headset's picture where SteamVR's compositor gives none (anything but Windows: it is read through D3D11). The
//! same names as `mirror.rs`, and a reader that cannot be made, so that what reads an eye compiles everywhere and finds
//! nothing to read.
use crate::compositor::{Compositor, Eye, MirrorSource};

const ABSENT: &str = "the headset's picture is read on Windows only"; // i18n: not shown (a log's line)

/// An eye's picture: RGBA, gamma-encoded, top row first.
#[derive(Default)]
pub struct EyePicture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Where this lies in the whole eye's picture, and how large that is.
    pub from: [u32; 2],
    pub whole: [u32; 2],
}

pub struct EyeReader(());

impl EyeReader {
    pub fn new(_adapter: Option<u32>) -> Result<Self, String> {
        Err(ABSENT.into())
    }

    pub fn read(&self, _compositor: &Compositor, _eye: Eye) -> Result<EyePicture, String> {
        Err(ABSENT.into())
    }

    pub fn read_part(&self, _source: MirrorSource, _eye: Eye, _area: Option<[u32; 4]>) -> Result<EyePicture, String> {
        Err(ABSENT.into())
    }

    pub fn read_pair(&self, _source: MirrorSource, _areas: [Option<[u32; 4]>; 2]) -> Result<([EyePicture; 2], Option<u32>), String> {
        Err(ABSENT.into())
    }

    pub fn read_pair_into(&self, _source: MirrorSource, _areas: [Option<[u32; 4]>; 2], _out: &mut [EyePicture; 2]) -> Result<Option<u32>, String> {
        Err(ABSENT.into())
    }
}
