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
        return mac::seal(&key, data, entropy);
    }
    // What an unsigned build of this computer sealed with its key file opens too (`mac::older_key`).
    mac::open(&key, data, entropy).or_else(|e| match mac::older_key().filter(|older| *older != key) {
        Some(older) => mac::open(&older, data, entropy).map_err(|_| e),
        None => Err(e),
    })
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

    /// The keychain's "interaction not allowed": it is locked (the Mac's screen locked, or before the person's login),
    /// and no question could be asked. Not a refusal: asked again at the next need.
    const LOCKED: i32 = -25308;

    /// Why there is no key: `again` when asking once more later may give it (a locked keychain).
    #[derive(Debug)]
    pub(super) struct NoKey {
        pub message: String,
        pub again: bool,
    }

    /// The key of this run: found once, and a refusal is kept as well (the keychain is not asked again and again),
    /// except a locked keychain's, which is not one.
    pub(super) fn key() -> anyhow::Result<[u8; KEY_LEN]> {
        static KEY: std::sync::Mutex<Option<Result<[u8; KEY_LEN], String>>> = std::sync::Mutex::new(None);
        let mut kept = KEY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(found) = kept.as_ref() {
            return found.clone().map_err(|e| anyhow::anyhow!(e));
        }
        let file = crate::paths::config_dir().join(KEY_FILE);
        let found = if signed() { keychain_key_after(&file) } else { file_key(&file).map_err(|e| NoKey { message: format!("{e:#}"), again: false }) };
        match found {
            Ok(key) => {
                *kept = Some(Ok(key));
                Ok(key)
            }
            Err(NoKey { message, again }) => {
                if !again {
                    *kept = Some(Err(message.clone()));
                }
                Err(anyhow::anyhow!(message))
            }
        }
    }

    /// For a signed build, the key file an unsigned build of this computer made (a developer's run from `target`): what it
    /// sealed opens with it. Never the signed build's own key, which is the keychain's.
    pub(super) fn older_key() -> Option<[u8; KEY_LEN]> {
        if !signed() {
            return None;
        }
        std::fs::read(crate::paths::config_dir().join(KEY_FILE)).ok().and_then(|bytes| <[u8; KEY_LEN]>::try_from(bytes.as_slice()).ok())
    }

    /// [`signed_by_a_team`], asked once per run.
    fn signed() -> bool {
        static SIGNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SIGNED.get_or_init(signed_by_a_team)
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

    /// What a signed build does with the keychain's answer and a key file an unsigned build may have left.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum Plan {
        /// The keychain's key: it is this app's, whatever file lies beside the settings (2026-11-22 21:59: a developer's
        /// unsigned run had made a file key, and the signed app put it in the keychain in place of its own key, which
        /// opened the saved VRChat session; that session could never be opened again).
        Keychain,
        /// No key in the keychain yet, and the key file of the unsigned build that came before: it goes in, so that what it
        /// sealed still opens.
        FileIntoKeychain,
        /// None anywhere: a new one, in the keychain.
        New,
    }

    /// `item` is the keychain's answer (`Ok(true)` it has the key, `Ok(false)` it has none, `Err` it would not say).
    pub(super) fn plan(item: Result<bool, i32>, file: bool) -> Result<Plan, i32> {
        match item {
            Ok(true) => Ok(Plan::Keychain),
            Ok(false) if file => Ok(Plan::FileIntoKeychain),
            Ok(false) => Ok(Plan::New),
            Err(code) => Err(code),
        }
    }

    /// A signed build's key: the keychain's ([`plan`]).
    fn keychain_key_after(file: &std::path::Path) -> Result<[u8; KEY_LEN], NoKey> {
        let refused = |e: security_framework::base::Error| NoKey { again: e.code() == LOCKED, message: tr!("the keychain did not give the key: {error}", error = e) };
        let item = match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.code() == NOT_FOUND => None,
            Err(e) => return Err(refused(e)),
        };
        let file_key = std::fs::read(file).ok().and_then(|bytes| <[u8; KEY_LEN]>::try_from(bytes.as_slice()).ok());
        match plan(Ok(item.is_some()), file_key.is_some()) {
            Ok(Plan::Keychain) => <[u8; KEY_LEN]>::try_from(item.unwrap_or_default().as_slice())
                .map_err(|_| NoKey { message: tr("the keychain holds a key of another length").to_string(), again: false }),
            Ok(Plan::FileIntoKeychain) => {
                let key = file_key.unwrap_or_default();
                let _ = delete_generic_password(SERVICE, FIRST_ACCOUNT);
                if set_generic_password(SERVICE, ACCOUNT, &key).is_ok() {
                    let _ = std::fs::remove_file(file);
                }
                Ok(key)
            }
            Ok(Plan::New) | Err(_) => keychain_key().map_err(|e| NoKey { message: format!("{e:#}"), again: false }),
        }
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

        #[test]
        fn the_keychain_s_key_is_never_replaced_by_an_unsigned_build_s_file() {
            assert_eq!(plan(Ok(true), true), Ok(Plan::Keychain), "the keychain's key stays the app's, a file beside it or not");
            assert_eq!(plan(Ok(true), false), Ok(Plan::Keychain));
            assert_eq!(plan(Ok(false), true), Ok(Plan::FileIntoKeychain), "an unsigned build's key moves in only where there is none");
            assert_eq!(plan(Ok(false), false), Ok(Plan::New));
            assert_eq!(plan(Err(LOCKED), true), Err(LOCKED), "a locked keychain changes nothing");
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
