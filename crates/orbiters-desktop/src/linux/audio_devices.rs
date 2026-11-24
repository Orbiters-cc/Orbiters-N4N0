//! The sound devices of a Linux computer. Nothing yet (the port's sound stream, L3).

use super::{AudioEndpoint, Flow, Role};

pub fn list(_: Flow) -> Result<Vec<AudioEndpoint>, &'static str> {
    Ok(Vec::new())
}

pub fn defaults(_: Flow) -> [Option<AudioEndpoint>; 3] {
    Default::default()
}

pub struct Meter;

impl Meter {
    pub fn open(_: &str) -> Option<Self> {
        None
    }

    pub fn peak(&self) -> Option<f32> {
        None
    }
}

pub fn set_default(_: &str, _: Role) -> Result<(), &'static str> {
    Err(orbiters_i18n::tr("Sound devices are not used on Linux yet."))
}

pub fn play_test(_: &str) -> Result<(), &'static str> {
    Err(orbiters_i18n::tr("Sound devices are not used on Linux yet."))
}
