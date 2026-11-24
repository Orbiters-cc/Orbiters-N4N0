//! VRChat on Linux, under Steam's Proton, for the launcher (`launcher::RealMachine`) and Where (`travel::Real`): the VRChats that run
//! and where VRChat is are known; starting one is not there yet (the port's VRChat stream, L1: `steam -applaunch`, or Flatpak's
//! Steam, with `orbiters_core::proton` saying which).

use orbiters_core::vrchat_launch::SavedWindow;
use orbiters_core::processes::Process;
use orbiters_i18n::{mark, tr};

/// Why Home starts no VRChat (English; said with `tr`).
pub const NOT_YET: &str = mark("Orbiters - N4N0 cannot start VRChat on Linux yet: start it from Steam.");

/// The VRChats that run now.
pub fn running() -> Vec<Process> {
    orbiters_core::processes::all().into_iter().filter(Process::is_vrchat).collect()
}

/// Whether Home can start the VRChat this computer has.
pub fn installed() -> bool {
    orbiters_core::here::HERE.starts_vrchat && orbiters_core::proton::vrchat().is_some_and(|v| v.installed())
}

/// Starts VRChat through Steam, as a person does.
pub fn start(_args: &[String]) -> Result<(), String> {
    Err(tr(NOT_YET).into())
}

/// Starts one more VRChat, on a profile of its own ([`orbiters_core::here::Here::several_games`]).
pub fn start_beside(_args: &[String]) -> Result<(), String> {
    Err(tr(NOT_YET).into())
}

/// VRChat's saved window, from its prefix's registry (read only; it may lag a running VRChat).
pub fn saved_window() -> Option<SavedWindow> {
    let registry = orbiters_core::proton::vrchat()?.prefix.vrchat_registry();
    Some(SavedWindow {
        width: registry.u32("Screenmanager Resolution Width_h182942802")?, // i18n: not shown (a registry value name)
        height: registry.u32("Screenmanager Resolution Height_h2627697771")?, // i18n: not shown (a registry value name)
        mode: registry.u32("Screenmanager Fullscreen mode_h3630240806").unwrap_or(3), // i18n: not shown (a registry value name)
    })
}
