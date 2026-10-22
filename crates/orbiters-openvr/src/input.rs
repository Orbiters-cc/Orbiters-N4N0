//! `IVRInput_011`: actions an overlay application reads alongside the scene application (layouts from Valve's public
//! `openvr_capi.h`). The actions and their default bindings come from an action manifest (a JSON file).

use crate::{entry, Client, Error};
use std::ffi::CString;
use std::path::Path;

pub const INPUT_VERSION: &str = "IVRInput_011";

pub type ActionSetHandle = u64;
pub type ActionHandle = u64;

/// `VRActiveActionSet_t` (32 bytes).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
struct ActiveActionSet {
    action_set: ActionSetHandle,
    restricted_to_device: u64,
    secondary_action_set: ActionSetHandle,
    padding: u32,
    priority: i32,
}

/// `InputAnalogActionData_t` (48 bytes).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct AnalogData {
    pub active: bool,
    pub active_origin: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub delta_x: f32,
    pub delta_y: f32,
    pub delta_z: f32,
    pub update_time: f32,
}

/// `InputDigitalActionData_t` (24 bytes).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct DigitalData {
    pub active: bool,
    pub active_origin: u64,
    /// Pressed.
    pub state: bool,
    /// Pressed or let go since the last update.
    pub changed: bool,
    pub update_time: f32,
}

pub struct Input<'a> {
    table: *const usize,
    _client: &'a Client,
}

impl Client {
    pub fn input(&self) -> Result<Input<'_>, Error> {
        Ok(Input { table: self.table(&self.input_table, INPUT_VERSION)?, _client: self })
    }
}

/// An `EVRInputError` (0 is none).
fn result(code: i32) -> Result<(), i32> {
    if code == 0 {
        Ok(())
    } else {
        Err(code)
    }
}

fn c(text: &str) -> Result<CString, i32> {
    CString::new(text).map_err(|_| 1)
}

impl Input<'_> {
    /// Loads the action manifest at `path` (absolute). Once per process.
    pub fn set_action_manifest(&self, path: &Path) -> Result<(), i32> {
        type F = unsafe extern "system" fn(*const i8) -> i32;
        let path = c(&path.to_string_lossy())?;
        // SAFETY: IVRInput_011 entry 0 is SetActionManifestPath; NUL-terminated path.
        result(unsafe { entry::<F>(self.table, 0)(path.as_ptr()) })
    }

    pub fn action_set(&self, name: &str) -> Result<ActionSetHandle, i32> {
        type F = unsafe extern "system" fn(*const i8, *mut u64) -> i32;
        let name = c(name)?;
        let mut handle = 0;
        // SAFETY: entry 1 is GetActionSetHandle.
        result(unsafe { entry::<F>(self.table, 1)(name.as_ptr(), &mut handle) })?;
        Ok(handle)
    }

    pub fn action(&self, name: &str) -> Result<ActionHandle, i32> {
        type F = unsafe extern "system" fn(*const i8, *mut u64) -> i32;
        let name = c(name)?;
        let mut handle = 0;
        // SAFETY: entry 2 is GetActionHandle.
        result(unsafe { entry::<F>(self.table, 2)(name.as_ptr(), &mut handle) })?;
        Ok(handle)
    }

    /// Reads the actions of `set` for this frame (call before reading them).
    pub fn update(&self, set: ActionSetHandle) -> Result<(), i32> {
        self.update_sets(&[set])
    }

    /// Reads the actions of every set in `sets` for this frame (once a frame, all of them together).
    pub fn update_sets(&self, sets: &[ActionSetHandle]) -> Result<(), i32> {
        type F = unsafe extern "system" fn(*mut ActiveActionSet, u32, u32) -> i32;
        if sets.is_empty() {
            return Ok(());
        }
        // A few sets each frame: on the stack.
        let mut few = [ActiveActionSet::default(); 8];
        let mut many = Vec::new();
        let active: &mut [ActiveActionSet] = if sets.len() <= few.len() {
            &mut few[..sets.len()]
        } else {
            many.resize(sets.len(), ActiveActionSet::default());
            &mut many
        };
        for (a, &s) in active.iter_mut().zip(sets) {
            a.action_set = s;
        }
        // SAFETY: entry 4 is UpdateActionState; `active.len()` sets of the given size.
        result(unsafe { entry::<F>(self.table, 4)(active.as_mut_ptr(), std::mem::size_of::<ActiveActionSet>() as u32, active.len() as u32) })
    }

    pub fn digital(&self, action: ActionHandle) -> Result<DigitalData, i32> {
        type F = unsafe extern "system" fn(u64, *mut DigitalData, u32, u64) -> i32;
        let mut data = DigitalData::default();
        // SAFETY: entry 5 is GetDigitalActionData; no device restriction.
        result(unsafe { entry::<F>(self.table, 5)(action, &mut data, std::mem::size_of::<DigitalData>() as u32, 0) })?;
        Ok(data)
    }

    pub fn analog(&self, action: ActionHandle) -> Result<AnalogData, i32> {
        type F = unsafe extern "system" fn(u64, *mut AnalogData, u32, u64) -> i32;
        let mut data = AnalogData::default();
        // SAFETY: entry 6 is GetAnalogActionData; no device restriction.
        result(unsafe { entry::<F>(self.table, 6)(action, &mut data, std::mem::size_of::<AnalogData>() as u32, 0) })?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn layouts_match_openvr() {
        assert_eq!(std::mem::size_of::<super::ActiveActionSet>(), 32);
        assert_eq!(std::mem::size_of::<super::AnalogData>(), 48);
        assert_eq!(std::mem::offset_of!(super::AnalogData, x), 16);
        assert_eq!(std::mem::size_of::<super::DigitalData>(), 24);
        assert_eq!(std::mem::offset_of!(super::DigitalData, state), 16);
        assert_eq!(std::mem::offset_of!(super::DigitalData, changed), 17);
        assert_eq!(std::mem::offset_of!(super::DigitalData, update_time), 20);
    }
}
