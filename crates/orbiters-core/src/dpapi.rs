//! Current-user protection shared by VRChat's session store and [`crate::sealed`]'s files (what was said to N4N0, the
//! bridge's keys): Windows' DPAPI, on the Mac a key the login keychain keeps, on Linux a key in a file of the person's (see
//! each [`crypt`]). Elsewhere there is none.
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

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
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
        return sealing::seal(&key, data, entropy);
    }
    // What an unsigned build of this computer sealed with its key file opens too (`mac::older_key`).
    sealing::open(&key, data, entropy).or_else(|e| match mac::older_key().filter(|older| *older != key) {
        Some(older) => sealing::open(&older, data, entropy).map_err(|_| e),
        None => Err(e),
    })
}

/// What DPAPI does on Windows, on Linux: AES-256-GCM under a random key made the first time it is needed, `entropy` counted in the
/// seal as DPAPI's is. The key is a file only the person can read, beside the settings, as a DPAPI secret is any program of the
/// person's to open: the Secret Service (GNOME's keyring, KWallet) would give it to every program of theirs as well, is not on
/// every desktop, and one that is there one day and not the next would take the saved session with it.
#[cfg(target_os = "linux")]
pub(crate) fn crypt(data: &[u8], entropy: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
    let key = sealing::file_key(&crate::paths::config_dir().join(sealing::KEY_FILE))?;
    if encrypt {
        sealing::seal(&key, data, entropy)
    } else {
        sealing::open(&key, data, entropy)
    }
}

/// A key kept in a file of the person's, and what is sealed with a key: shared by the Mac (for builds its keychain cannot know)
/// and Linux.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod sealing {
    use orbiters_i18n::tr;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
    use ring::rand::{SecureRandom, SystemRandom};

    pub(super) const KEY_LEN: usize = 32;

    /// The key's file, in the settings' folder: on Linux, and on a Mac for builds the keychain cannot know again.
    pub(super) const KEY_FILE: &str = "session-key";

    /// The key in a file only the person reads, made when there is none yet. A new key is written whole into a file of its
    /// own, then linked to the key file's name, which fails when another process put its key there first: then theirs is
    /// the key. A key file is never removed while it may be another process's being made; one of another length that has
    /// not changed for a minute is damage, set aside (never deleted) and replaced.
    pub(super) fn file_key(file: &std::path::Path) -> anyhow::Result<[u8; KEY_LEN]> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Some(Ok(key)) = read_whole(file) {
            return Ok(key);
        }
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        // Repair and first creation one process at a time (a lock beside it), every look made again under it: another
        // process may have repaired or made it meanwhile.
        let _held = lock(file)?;
        if let Some(key) = read_key_file(file)? {
            return Ok(key);
        }
        let mut key = [0u8; KEY_LEN];
        SystemRandom::new().fill(&mut key).map_err(|_| anyhow::anyhow!("{}", tr("no random numbers")))?;
        let made = file.with_file_name(format!(".{KEY_FILE}.{}.{}", std::process::id(), next_part()));
        let written = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&made).and_then(|mut f| {
            f.write_all(&key)?;
            f.sync_all()
        });
        let published = written.and_then(|()| std::fs::hard_link(&made, file));
        let _ = std::fs::remove_file(&made);
        match published {
            Ok(()) => Ok(key),
            // Someone else's key got there first: it is the one.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read_key_file(file)?.ok_or_else(|| anyhow::anyhow!("{}", tr("the key file is being written: try again"))),
            Err(e) => Err(e.into()),
        }
    }

    /// Reads the key file whole: `None` when there is none, `Some(Err)` for one of another length (or unreadable).
    pub(super) fn read_whole(file: &std::path::Path) -> Option<Result<[u8; KEY_LEN], ()>> {
        match std::fs::read(file) {
            Ok(bytes) => Some(<[u8; KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| ())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(Err(())),
        }
    }

    /// The lock of the key file's repair and creation, held while the returned file is.
    fn lock(file: &std::path::Path) -> anyhow::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(file.with_file_name(format!(".{KEY_FILE}.lock")))?;
        lock.lock()?;
        Ok(lock)
    }

    /// A number for each key file this process makes.
    fn next_part() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// The key file's key; `None` when there is none (or only damage, now set aside). A file of another length that changed
    /// in the last minute may be another process's being written: an error, asked again later.
    /// Called with the repair lock held: the file set aside is the very one read (its identity checked again right before).
    fn read_key_file(file: &std::path::Path) -> anyhow::Result<Option<[u8; KEY_LEN]>> {
        use std::os::unix::fs::MetadataExt as _;
        let identity = |m: &std::fs::Metadata| (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec());
        let before = match std::fs::metadata(file) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let bytes = std::fs::read(file)?;
        if let Ok(key) = <[u8; KEY_LEN]>::try_from(bytes.as_slice()) {
            return Ok(Some(key));
        }
        let age = before.modified().ok().and_then(|t| t.elapsed().ok()).unwrap_or_default();
        anyhow::ensure!(age >= std::time::Duration::from_secs(60), "{}", tr("the key file is being written: try again"));
        // Still the same file as read: a whole key put there since is never set aside.
        anyhow::ensure!(std::fs::metadata(file).is_ok_and(|now| identity(&now) == identity(&before)), "{}", tr("the key file is being written: try again"));
        std::fs::rename(file, file.with_file_name(format!("{KEY_FILE}.damaged")))?;
        Ok(None)
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
        fn the_key_s_file_is_one_only_the_person_reads() {
            use std::os::unix::fs::PermissionsExt as _;
            let folder = std::env::temp_dir().join(format!("orbiters-key-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            let file = folder.join("settings").join(KEY_FILE);
            let first = file_key(&file).unwrap();
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(file_key(&file).unwrap(), first, "the same key at the next start: the session still opens");
            // A file of another length may be another process's being written: left alone, asked again later.
            std::fs::write(&file, b"cut short").unwrap();
            assert!(file_key(&file).is_err(), "not replaced while it may be someone's being written");
            assert_eq!(std::fs::read(&file).unwrap(), b"cut short");
            // One that has not changed for a minute is damage: set aside, never deleted, and replaced.
            let old = std::fs::File::options().write(true).open(&file).unwrap();
            old.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120)).unwrap();
            assert_ne!(file_key(&file).unwrap(), first);
            assert_eq!(std::fs::read(&file).unwrap().len(), KEY_LEN);
            assert_eq!(std::fs::read(file.with_file_name(format!("{KEY_FILE}.damaged"))).unwrap(), b"cut short");
            let _ = std::fs::remove_dir_all(&folder);
        }

        /// The verifier's case: two first uses at once (two processes, here threads) each make a key; one key file results,
        /// and both use the key in it.
        #[test]
        fn two_first_uses_at_once_end_with_one_key_for_both() {
            // M10: two processes both find an old short file; one repairs it; the other never sets the repaired key aside.
            for round in 0..10 {
                let folder = std::env::temp_dir().join(format!("orbiters-key-repair-{}-{round}", std::process::id()));
                let _ = std::fs::remove_dir_all(&folder);
                std::fs::create_dir_all(&folder).unwrap();
                let file = folder.join(KEY_FILE);
                std::fs::write(&file, b"short").unwrap();
                std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120)).unwrap();
                let keys: Vec<[u8; KEY_LEN]> = std::thread::scope(|scope| {
                    let racers: Vec<_> = (0..8).map(|_| scope.spawn(|| loop {
                        if let Ok(key) = file_key(&file) {
                            break key;
                        }
                    })).collect();
                    racers.into_iter().map(|r| r.join().unwrap()).collect()
                });
                let on_disk = std::fs::read(&file).unwrap();
                assert!(keys.iter().all(|k| k.as_slice() == on_disk.as_slice()), "round {round}: one repaired key for all");
                assert_eq!(std::fs::read(file.with_file_name(format!("{KEY_FILE}.damaged"))).unwrap(), b"short", "only the damage set aside");
                let _ = std::fs::remove_dir_all(&folder);
            }
            for round in 0..20 {
                let folder = std::env::temp_dir().join(format!("orbiters-key-race-{}-{round}", std::process::id()));
                let _ = std::fs::remove_dir_all(&folder);
                let file = folder.join(KEY_FILE);
                let keys: Vec<[u8; KEY_LEN]> = std::thread::scope(|scope| {
                    let racers: Vec<_> = (0..8).map(|_| scope.spawn(|| loop {
                        if let Ok(key) = file_key(&file) {
                            break key;
                        }
                    })).collect();
                    racers.into_iter().map(|r| r.join().unwrap()).collect()
                });
                let on_disk = std::fs::read(&file).unwrap();
                assert!(keys.iter().all(|k| k.as_slice() == on_disk.as_slice()), "round {round}: one key for all");
                assert!(std::fs::read_dir(&folder).unwrap().flatten().all(|e| { let n = e.file_name(); let n = n.to_string_lossy(); n == KEY_FILE || n == ".session-key.lock" }), "nothing left beside it but its lock");
                let _ = std::fs::remove_dir_all(&folder);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::sealing::{file_key, read_whole, KEY_FILE, KEY_LEN};
    use crate::locks::*;
    use orbiters_i18n::tr;
    use ring::rand::{SecureRandom, SystemRandom};
    use security_framework::passwords::{delete_generic_password, get_generic_password};

    const SERVICE: &str = "Orbiters - N4N0";
    /// The second of its name: the first ("session key") was made by builds signed ad hoc, whose item the keychain
    /// gives to no other build without asking for the login password. It is removed, never read.
    const ACCOUNT: &str = "session key 2"; // i18n: not shown (the name of a Keychain item)
    const FIRST_ACCOUNT: &str = "session key"; // i18n: not shown (the name of a Keychain item)
    /// The Keychain's "item not found" (`errSecItemNotFound`).
    const NOT_FOUND: i32 = -25300;
    /// The Keychain's "already there" (`errSecDuplicateItem`): another process made the key first.
    const DUPLICATE: i32 = -25299;

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
        /// A refusal is kept this long, then the keychain is asked again (a person who said no may say yes later).
        const REFUSAL_KEPT: std::time::Duration = std::time::Duration::from_secs(300);
        type Kept = Option<(Result<[u8; KEY_LEN], String>, std::time::Instant)>;
        static KEY: std::sync::Mutex<Kept> = std::sync::Mutex::new(None);
        let mut kept = KEY.locked();
        match kept.as_ref() {
            Some((Ok(key), _)) => return Ok(*key),
            Some((Err(e), at)) if at.elapsed() < REFUSAL_KEPT => return Err(anyhow::anyhow!(e.clone())),
            _ => {}
        }
        let file = crate::paths::config_dir().join(KEY_FILE);
        // A key file that could not be read or made now (another process writing it) is asked again at the next need.
        let found = if signed() { keychain_key_after(&file) } else { file_key(&file).map_err(|e| NoKey { message: format!("{e:#}"), again: true }) };
        match found {
            Ok(key) => {
                *kept = Some((Ok(key), std::time::Instant::now()));
                Ok(key)
            }
            Err(NoKey { message, again }) => {
                if !again {
                    *kept = Some((Err(message.clone()), std::time::Instant::now()));
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
        let file_key = migration_file(file)?;
        match plan(Ok(item.is_some()), file_key.is_some()) {
            Ok(Plan::Keychain) => <[u8; KEY_LEN]>::try_from(item.unwrap_or_default().as_slice())
                .map_err(|_| NoKey { message: tr("the keychain holds a key of another length").to_string(), again: false }),
            Ok(Plan::FileIntoKeychain) => {
                let key = file_key.unwrap_or_default();
                let kept = add_or_adopt(key, add_item, || get_generic_password(SERVICE, ACCOUNT).map_err(|e| e.code())).map_err(keychain_refused)?;
                if kept == key {
                    let _ = delete_generic_password(SERVICE, FIRST_ACCOUNT);
                }
                // The file stays: an unsigned build run again keeps using it (removed, it would make another key and lose
                // what it sealed), and this build opens what it sealed with it (`older_key`).
                Ok(kept)
            }
            Ok(Plan::New) | Err(_) => keychain_key().map_err(keychain_refused),
        }
    }

    /// The key file an unsigned build left, for a signed build's migration: none when there is none (or only damage); a
    /// file that is there and cannot be read is not "none" (what it sealed may be needed): asked again later.
    pub(super) fn migration_file(file: &std::path::Path) -> Result<Option<[u8; KEY_LEN]>, NoKey> {
        match read_whole(file) {
            None => Ok(None),
            Some(Ok(key)) => Ok(Some(key)),
            Some(Err(())) if std::fs::read(file).is_ok() => Ok(None),
            Some(Err(())) => Err(NoKey { message: tr("the key file could not be read: try again").to_string(), again: true }),
        }
    }

    /// Why the keychain gave no key: asked again later when it was locked (the person's screen), else not this run.
    fn keychain_refused(code: i32) -> NoKey {
        NoKey { again: code == LOCKED, message: tr!("the keychain did not give the key: {error}", error = security_framework::base::Error::from_code(code)) }
    }

    /// Adds `key` to the keychain only where there is none (`add`: `Err(DUPLICATE)` when another process's is there); then
    /// that one, the winner's, read back by `get`, is the key, whichever process made it.
    pub(super) fn add_or_adopt(key: [u8; KEY_LEN], add: impl Fn(&[u8; KEY_LEN]) -> Result<(), i32>, get: impl Fn() -> Result<Vec<u8>, i32>) -> Result<[u8; KEY_LEN], i32> {
        match add(&key) {
            Ok(()) => Ok(key),
            Err(DUPLICATE) => <[u8; KEY_LEN]>::try_from(get()?.as_slice()).map_err(|_| DUPLICATE),
            Err(code) => Err(code),
        }
    }

    /// The key as a new generic password of this app's, never over an existing one.
    fn add_item(key: &[u8; KEY_LEN]) -> Result<(), i32> {
        use security_framework::item::{ItemAddOptions, ItemAddValue, ItemClass};
        ItemAddOptions::new(ItemAddValue::Data { class: ItemClass::generic_password(), data: core_foundation::data::CFData::from_buffer(key) })
            .set_service(SERVICE)
            .set_account_name(ACCOUNT)
            .add()
            .map_err(|e| e.code())
    }

    /// The key from the login keychain, made and kept there when there is none yet (`Err` the keychain's code).
    pub(super) fn keychain_key() -> Result<[u8; KEY_LEN], i32> {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => <[u8; KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| DUPLICATE),
            Err(e) if e.code() == NOT_FOUND => {
                let mut key = [0u8; KEY_LEN];
                SystemRandom::new().fill(&mut key).map_err(|_| NOT_FOUND)?;
                let kept = add_or_adopt(key, add_item, || get_generic_password(SERVICE, ACCOUNT).map_err(|e| e.code()))?;
                let _ = delete_generic_password(SERVICE, FIRST_ACCOUNT);
                Ok(kept)
            }
            Err(e) => Err(e.code()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_build_without_a_team_is_known() {
            assert!(names_a_team("Identifier=cc.orbiters.n4n0\nTeamIdentifier=V83Y447ML9\nSealed Resources version=2"));
            assert!(!names_a_team("Identifier=cc.orbiters.n4n0\nSignature=adhoc\nTeamIdentifier=not set"));
            assert!(!names_a_team("orbiters-desktop: code object is not signed at all"));
        }

        /// Two signed processes at their first use: the keychain item is added, never set over another's; the one that
        /// finds it there takes the winner's key. A locked keychain stays a reason to ask again.
        #[test]
        fn the_keychain_s_first_key_is_the_one_every_process_keeps() {
            let keychain = std::sync::Mutex::new(None::<[u8; KEY_LEN]>);
            let add = |key: &[u8; KEY_LEN]| {
                let mut item = keychain.lock().unwrap();
                if item.is_some() {
                    return Err(DUPLICATE);
                }
                *item = Some(*key);
                Ok(())
            };
            let get = || keychain.lock().unwrap().map(|k| k.to_vec()).ok_or(NOT_FOUND);
            let first = add_or_adopt([1; KEY_LEN], add, get).unwrap();
            let second = add_or_adopt([2; KEY_LEN], add, get).unwrap();
            assert_eq!((first, second), ([1; KEY_LEN], [1; KEY_LEN]), "the second process uses the first one's key");
            assert_eq!(*keychain.lock().unwrap(), Some([1; KEY_LEN]), "never replaced");
            assert_eq!(add_or_adopt([3; KEY_LEN], |_| Err(LOCKED), get), Err(LOCKED));
            assert!(keychain_refused(LOCKED).again, "locked while making it: asked again later");
            assert!(!keychain_refused(-25293).again);
        }

        /// M10: a key file that cannot be read now is no "none" for a migration: asked again, never replaced.
        #[test]
        fn an_unreadable_key_file_is_not_taken_for_none() {
            use std::os::unix::fs::PermissionsExt as _;
            let folder = std::env::temp_dir().join(format!("orbiters-key-migrate-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            let file = folder.join(KEY_FILE);
            assert!(matches!(migration_file(&file), Ok(None)));
            std::fs::write(&file, [5u8; KEY_LEN]).unwrap();
            assert!(matches!(migration_file(&file), Ok(Some(k)) if k == [5u8; KEY_LEN]));
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = migration_file(&file);
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(matches!(unreadable, Err(NoKey { again: true, .. })), "{unreadable:?}");
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
