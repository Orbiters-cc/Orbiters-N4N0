//! `IVRChaperoneSetup_006`: the play space's working copy (where the standing and seated universes sit in the raw
//! tracking space, and the chaperone's bounds), shown live to every application while previewed (layouts from Valve's
//! public `openvr_capi.h`).

use crate::{entry, Client, Error, Matrix34};

pub const CHAPERONE_SETUP_VERSION: &str = "IVRChaperoneSetup_006";

/// `HmdQuad_t`: one wall of the chaperone, four corners in the standing universe.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Quad {
    pub corners: [[f32; 3]; 4],
}

pub struct ChaperoneSetup<'a> {
    table: *const usize,
    _client: &'a Client,
}

impl Client {
    pub fn chaperone_setup(&self) -> Result<ChaperoneSetup<'_>, Error> {
        Ok(ChaperoneSetup { table: self.interface(CHAPERONE_SETUP_VERSION)?, _client: self })
    }
}

impl ChaperoneSetup<'_> {
    /// Drops the working copy's changes: it is the saved (live) setup again.
    pub fn revert_working_copy(&self) {
        type F = unsafe extern "system" fn();
        // SAFETY: IVRChaperoneSetup_006 entry 1 is RevertWorkingCopy.
        unsafe { entry::<F>(self.table, 1)() }
    }

    pub fn working_collision_bounds(&self) -> Option<Vec<Quad>> {
        type F = unsafe extern "system" fn(*mut Quad, *mut u32) -> bool;
        let mut count = 0u32;
        // SAFETY: entry 4 is GetWorkingCollisionBoundsInfo; a null buffer asks for the count.
        unsafe { entry::<F>(self.table, 4)(std::ptr::null_mut(), &mut count) };
        let mut quads = vec![Quad::default(); count as usize];
        if count == 0 {
            return Some(quads);
        }
        // SAFETY: as above, with a buffer of `count` quads.
        let ok = unsafe { entry::<F>(self.table, 4)(quads.as_mut_ptr(), &mut count) };
        quads.truncate(count as usize);
        ok.then_some(quads)
    }

    pub fn working_seated_zero(&self) -> Option<Matrix34> {
        type F = unsafe extern "system" fn(*mut Matrix34) -> bool;
        let mut m = Matrix34::IDENTITY;
        // SAFETY: entry 6 is GetWorkingSeatedZeroPoseToRawTrackingPose.
        unsafe { entry::<F>(self.table, 6)(&mut m) }.then_some(m)
    }

    pub fn working_standing_zero(&self) -> Option<Matrix34> {
        type F = unsafe extern "system" fn(*mut Matrix34) -> bool;
        let mut m = Matrix34::IDENTITY;
        // SAFETY: entry 7 is GetWorkingStandingZeroPoseToRawTrackingPose.
        unsafe { entry::<F>(self.table, 7)(&mut m) }.then_some(m)
    }

    pub fn set_working_collision_bounds(&self, quads: &[Quad]) {
        type F = unsafe extern "system" fn(*const Quad, u32);
        // SAFETY: entry 9 is SetWorkingCollisionBoundsInfo.
        unsafe { entry::<F>(self.table, 9)(quads.as_ptr(), quads.len() as u32) }
    }

    pub fn set_working_seated_zero(&self, m: &Matrix34) {
        type F = unsafe extern "system" fn(*const Matrix34);
        // SAFETY: entry 11 is SetWorkingSeatedZeroPoseToRawTrackingPose.
        unsafe { entry::<F>(self.table, 11)(m) }
    }

    pub fn set_working_standing_zero(&self, m: &Matrix34) {
        type F = unsafe extern "system" fn(*const Matrix34);
        // SAFETY: entry 12 is SetWorkingStandingZeroPoseToRawTrackingPose.
        unsafe { entry::<F>(self.table, 12)(m) }
    }

    /// Every application sees the working copy (until hidden or reverted).
    pub fn show_working_set_preview(&self) {
        type F = unsafe extern "system" fn();
        // SAFETY: entry 17 is ShowWorkingSetPreview.
        unsafe { entry::<F>(self.table, 17)() }
    }

    pub fn hide_working_set_preview(&self) {
        type F = unsafe extern "system" fn();
        // SAFETY: entry 18 is HideWorkingSetPreview.
        unsafe { entry::<F>(self.table, 18)() }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quad_matches_openvr() {
        assert_eq!(std::mem::size_of::<super::Quad>(), 48);
    }
}
