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

/// What DPAPI does on Windows, on the Mac: AES-256-GCM under a key made the first time it is needed, `entropy` counted
/// in the seal as DPAPI's is (another `entropy` does not open it). The key stays in the person's login keychain when the
/// app carries a signature the keychain can know again after an update (Developer ID, Apple Development): it then gives
/// the key without a question. A build signed ad hoc has no such identity, and the keychain would ask for the login
/// password at every reading, after every update: there the key is a file only the person can read, beside the
/// settings, as a DPAPI secret is any program of the person's to open. Read once per run, whichever it is.
#[cfg(target_os = "macos")]
pub(crate) fn crypt(data: &[u8], entropy: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
    let key = mac::key()?;
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
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

    const SERVICE: &str = "Orbiters - N4N0";
    /// The second of its name: the first ("session key") was made by builds signed ad hoc, whose item the keychain
    /// gives to no other build without asking for the login password. It is removed, never read.
    const ACCOUNT: &str = "session key 2"; // i18n: not shown (the name of a Keychain item)
    const FIRST_ACCOUNT: &str = "session key"; // i18n: not shown (the name of a Keychain item)
    const KEY_LEN: usize = 32;
    /// The Keychain's "item not found" (`errSecItemNotFound`).
    const NOT_FOUND: i32 = -25300;

    /// The key file of builds the keychain cannot know again, in the settings' folder.
    const KEY_FILE: &str = "session-key";

    /// The key of this run: found once, and a refusal is kept as well (the keychain is not asked again and again).
    pub(super) fn key() -> anyhow::Result<[u8; KEY_LEN]> {
        static KEY: std::sync::OnceLock<Result<[u8; KEY_LEN], String>> = std::sync::OnceLock::new();
        KEY.get_or_init(|| {
            let file = crate::paths::config_dir().join(KEY_FILE);
            let found = if signed_by_a_team() { keychain_key_after(&file) } else { file_key(&file) };
            found.map_err(|e| format!("{e:#}"))
        })
        .clone()
        .map_err(|e| anyhow::anyhow!(e))
    }

    /// Whether this program's signature names a team (`codesign -dv` on itself): one the keychain knows again after an
    /// update. Ad hoc and unsigned builds do not.
    fn signed_by_a_team() -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        let Ok(out) = std::process::Command::new("/usr/bin/codesign").arg("-dv").arg(exe).stdin(std::process::Stdio::null()).output() else { return false };
        names_a_team(&String::from_utf8_lossy(&out.stderr))
    }

    /// What `codesign -dv` says of a team: `TeamIdentifier=V83Y447ML9`, or `TeamIdentifier=not set` for ad hoc.
    pub(super) fn names_a_team(described: &str) -> bool {
        described.lines().filter_map(|l| l.strip_prefix("TeamIdentifier=")).any(|team| team.len() == 10 && team.bytes().all(|b| b.is_ascii_alphanumeric()))
    }

    /// The key in a file only the person reads, made when there is none yet.
    pub(super) fn file_key(file: &std::path::Path) -> anyhow::Result<[u8; KEY_LEN]> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Ok(bytes) = std::fs::read(file) {
            if let Ok(key) = <[u8; KEY_LEN]>::try_from(bytes.as_slice()) {
                return Ok(key);
            }
        }
        let mut key = [0u8; KEY_LEN];
        SystemRandom::new().fill(&mut key).map_err(|_| anyhow::anyhow!("{}", tr("no random numbers")))?;
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let _ = std::fs::remove_file(file);
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(file)?.write_all(&key)?;
        Ok(key)
    }

    /// A signed build's key: the keychain's. A key file left by an ad hoc build before it goes into the keychain first
    /// (in place of an item that build may have left, which would ask), so what was sealed with it still opens.
    fn keychain_key_after(file: &std::path::Path) -> anyhow::Result<[u8; KEY_LEN]> {
        if let Some(key) = std::fs::read(file).ok().and_then(|bytes| <[u8; KEY_LEN]>::try_from(bytes.as_slice()).ok()) {
            let _ = delete_generic_password(SERVICE, ACCOUNT);
            let _ = delete_generic_password(SERVICE, FIRST_ACCOUNT);
            if set_generic_password(SERVICE, ACCOUNT, &key).is_ok() {
                let _ = std::fs::remove_file(file);
            }
            return Ok(key);
        }
        keychain_key()
    }

    /// The key from the login keychain, made and kept there when there is none yet.
    pub(super) fn keychain_key() -> anyhow::Result<[u8; KEY_LEN]> {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => <[u8; KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| anyhow::anyhow!("{}", tr("the keychain holds a key of another length"))),
            Err(e) if e.code() == NOT_FOUND => {
                let mut key = [0u8; KEY_LEN];
                SystemRandom::new().fill(&mut key).map_err(|_| anyhow::anyhow!("{}", tr("no random numbers")))?;
                set_generic_password(SERVICE, ACCOUNT, &key).map_err(|e| anyhow::anyhow!("{}", tr!("the keychain did not keep the key: {error}", error = e)))?;
                let _ = delete_generic_password(SERVICE, FIRST_ACCOUNT);
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

        #[test]
        fn a_build_without_a_team_keeps_its_key_in_a_file_only_the_person_reads() {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(names_a_team("Identifier=cc.orbiters.n4n0\nTeamIdentifier=V83Y447ML9\nSealed Resources version=2"));
            assert!(!names_a_team("Identifier=cc.orbiters.n4n0\nSignature=adhoc\nTeamIdentifier=not set"));
            assert!(!names_a_team("orbiters-desktop: code object is not signed at all"));

            let folder = std::env::temp_dir().join(format!("orbiters-key-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            let file = folder.join("settings").join(KEY_FILE);
            let first = file_key(&file).unwrap();
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(file_key(&file).unwrap(), first, "the same key at the next start: the session still opens");
            std::fs::write(&file, b"cut short").unwrap();
            assert_ne!(file_key(&file).unwrap(), first, "a damaged file is replaced, not an error for ever");
            assert_eq!(std::fs::read(&file).unwrap().len(), KEY_LEN);
            let _ = std::fs::remove_dir_all(&folder);
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
