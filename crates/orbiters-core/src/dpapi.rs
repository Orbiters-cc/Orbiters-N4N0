//! Current-user protection shared by the two independent account stores: Windows' DPAPI, or on the Mac a key the login
//! keychain keeps (see [`crypt`]'s there). Elsewhere there is none.
#[cfg(windows)]
pub(crate) fn crypt(data: &[u8], entropy: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    anyhow::ensure!(data.len() <= u32::MAX as usize && entropy.len() <= u32::MAX as usize, "{}", orbiters_i18n::tr("Secret is too large"));
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

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn crypt(_: &[u8], _: &[u8], _: bool) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("{}", orbiters_i18n::tr("VRChat session storage requires Windows DPAPI"))
}

/// What DPAPI does on Windows, on the Mac: AES-256-GCM under a key that stays in the person's login keychain and is made
/// the first time it is needed, `entropy` counted in the seal as DPAPI's is (another `entropy` does not open it). The
/// file is useless away from this account, as a DPAPI one is.
#[cfg(target_os = "macos")]
pub(crate) fn crypt(data: &[u8], entropy: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
    let key = mac::keychain_key()?;
    if encrypt {
        mac::seal(&key, data, entropy)
    } else {
        mac::open(&key, data, entropy)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use orbiters_i18n::tr;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
    use ring::rand::{SecureRandom, SystemRandom};
    use security_framework::passwords::{get_generic_password, set_generic_password};

    const SERVICE: &str = "Orbiters - N4N0";
    const ACCOUNT: &str = "session key"; // i18n: not shown (the name of a Keychain item)
    const KEY_LEN: usize = 32;
    /// The Keychain's "item not found" (`errSecItemNotFound`).
    const NOT_FOUND: i32 = -25300;

    /// The key from the login keychain, made and kept there when there is none yet.
    pub(super) fn keychain_key() -> anyhow::Result<[u8; KEY_LEN]> {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => <[u8; KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| anyhow::anyhow!("{}", tr("the keychain holds a key of another length"))),
            Err(e) if e.code() == NOT_FOUND => {
                let mut key = [0u8; KEY_LEN];
                SystemRandom::new().fill(&mut key).map_err(|_| anyhow::anyhow!("{}", tr("no random numbers")))?;
                set_generic_password(SERVICE, ACCOUNT, &key).map_err(|e| anyhow::anyhow!("{}", tr!("the keychain did not keep the key: {error}", error = e)))?;
                Ok(key)
            }
            Err(e) => anyhow::bail!("{}", tr!("the keychain did not give the key: {error}", error = e)),
        }
    }

    fn cipher(key: &[u8; KEY_LEN]) -> anyhow::Result<LessSafeKey> {
        Ok(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow::anyhow!("{}", tr("not a key")))?))
    }

    /// A fresh nonce, then the sealed text and its tag.
    pub(super) fn seal(key: &[u8; KEY_LEN], data: &[u8], entropy: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow::anyhow!("{}", tr("no random numbers")))?;
        let mut sealed = data.to_vec();
        cipher(key)?
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(entropy), &mut sealed)
            .map_err(|_| anyhow::anyhow!("{}", tr("sealing failed")))?;
        let mut out = nonce.to_vec();
        out.append(&mut sealed);
        Ok(out)
    }

    pub(super) fn open(key: &[u8; KEY_LEN], sealed: &[u8], entropy: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(sealed.len() >= NONCE_LEN, "{}", tr("too short to be sealed"));
        let (nonce, text) = sealed.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow::anyhow!("{}", tr("not a nonce")))?;
        let mut text = text.to_vec();
        let plain = cipher(key)?.open_in_place(nonce, Aad::from(entropy), &mut text).map_err(|_| anyhow::anyhow!("{}", tr("not sealed for this user")))?.len();
        text.truncate(plain);
        Ok(text)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_seal_opens_only_with_its_key_and_its_entropy() {
            let key = [7u8; KEY_LEN];
            let plain = br#"{"auth":"test-cookie"}"#;
            let sealed = seal(&key, plain, b"entropy").unwrap();
            assert!(!sealed.windows(4).any(|w| w == b"auth"), "the text is not readable");
            assert_eq!(open(&key, &sealed, b"entropy").unwrap(), plain);
            assert!(open(&key, &sealed, b"other").is_err());
            assert!(open(&[8u8; KEY_LEN], &sealed, b"entropy").is_err());
            let mut changed = sealed.clone();
            *changed.last_mut().unwrap() ^= 1;
            assert!(open(&key, &changed, b"entropy").is_err(), "tampered with");
            assert_ne!(seal(&key, plain, b"entropy").unwrap(), sealed, "a new nonce each time");
        }

        /// The real login keychain (`cargo test -p orbiters-core keychain -- --ignored`): macOS may ask the first time.
        #[test]
        #[ignore]
        fn the_keychain_keeps_the_key() {
            let first = keychain_key().unwrap();
            assert_eq!(keychain_key().unwrap(), first);
            let sealed = super::super::crypt(b"secret", b"e", true).unwrap();
            assert_eq!(super::super::crypt(&sealed, b"e", false).unwrap(), b"secret");
        }
    }
}
