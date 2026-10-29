//! Windows current-user protection shared by the two independent account stores.
#[cfg(windows)]
pub(crate) fn crypt(data: &[u8], entropy: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    anyhow::ensure!(data.len() <= u32::MAX as usize && entropy.len() <= u32::MAX as usize, "Secret is too large");
    let blob = |bytes: &[u8]| CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: both input slices live for the call. DPAPI allocates the output with LocalAlloc; it is copied and freed once.
    unsafe {
        if encrypt {
            CryptProtectData(&blob(data), None, Some(&blob(entropy)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?;
        } else {
            CryptUnprotectData(&blob(data), None, Some(&blob(entropy)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)?;
        }
        if out.pbData.is_null() {
            return Ok(Vec::new());
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        Ok(bytes)
    }
}

#[cfg(not(windows))]
pub(crate) fn crypt(_: &[u8], _: &[u8], _: bool) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("VRChat session storage requires Windows DPAPI")
}
