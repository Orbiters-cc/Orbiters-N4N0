//! The coverage panel over VRChat's window on Linux. Nothing yet (the port's panels stream, L7).

pub struct Panel;

impl Panel {
    pub fn open(_: u32, _: u32, _: i32) -> anyhow::Result<Self> {
        anyhow::bail!("{}", orbiters_i18n::tr("Nothing shows over VRChat on Linux yet."))
    }
    pub fn show(&mut self, _: &[u8]) {}
    pub fn pump(&self) {}
    pub fn close(self) {}
}
