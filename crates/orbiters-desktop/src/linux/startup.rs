//! Starting with the person's session: not yet (L2: `~/.config/autostart/cc.orbiters.n4n0.desktop`).
use orbiters_i18n::tr;

pub fn is_set() -> bool {
    false
}

pub fn set(_on: bool) -> anyhow::Result<()> {
    anyhow::bail!("{}", tr("Starting with your session is not there yet on Linux."))
}
