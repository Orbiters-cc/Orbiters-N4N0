//! What VRChat's own limits on avatars are set to, read from the registry, **read only**: this file opens the key for reading
//! and has no call that could write. The booster writes nothing of VRChat's at all.
//!
//! Unity stores floats as an 8-byte double under the type `REG_DWORD`, which ordinary registry reads truncate to 0, so the
//! raw bytes are read. Keys also exist per account (`usr_<id>_…`) beside global ones, and which of them wins is not known. So
//! only the keys of the account VRChat's log says is playing are read, and if that cannot be said, nothing is shown and the
//! person is told to look in VRChat's menu. No account's ID goes further than this file's arguments.

use crate::guided::Known;

/// What this account has set, as far as the registry says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub max_shown: Option<Known>,
    pub hide_beyond: Option<Known>,
}

/// A registry value, as it is stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    /// `name` without Unity's `_h<hash>` end.
    pub name: String,
    pub kind: u32,
    pub bytes: Vec<u8>,
}

const REG_DWORD: u32 = 4;

/// A value holding a whole number: 4 bytes of `REG_DWORD`.
fn as_count(raw: &Raw) -> Option<u32> {
    (raw.kind == REG_DWORD && raw.bytes.len() == 4).then(|| u32::from_le_bytes([raw.bytes[0], raw.bytes[1], raw.bytes[2], raw.bytes[3]]))
}

/// A value holding a float: Unity's 8-byte double under `REG_DWORD`.
fn as_double(raw: &Raw) -> Option<f64> {
    (raw.bytes.len() == 8).then(|| f64::from_le_bytes(raw.bytes[..8].try_into().unwrap_or([0; 8])))
}

/// The limits from one account's values. Each limit has its value, and a separate switch for whether it is on.
pub fn limits_from(values: &[Raw]) -> Limits {
    let find = |name: &str| values.iter().find(|v| v.name == name);
    let max_shown = match (find("currentShowMaxNumberOfAvatarsEnabled").and_then(as_count), find("avatarProxyShowMaxNumber").and_then(as_count)) {
        (Some(0), _) => Some(Known::Off),
        (Some(_), Some(count)) if count < 1000 => Some(Known::Count(count)),
        _ => None,
    };
    let hide_beyond = match (find("avatarProxyShowAtRangeToggle").and_then(as_count), find("avatarProxyShowAtRange").and_then(as_double)) {
        (Some(0), _) => Some(Known::Off),
        (Some(_), Some(meters)) if meters > 0.0 => Some(Known::Meters(meters as f32)),
        _ => None,
    };
    Limits { max_shown, hide_beyond }
}

/// A value's name without Unity's hash, if it is one of this account's.
fn account_field<'a>(value_name: &'a str, account: &str) -> Option<&'a str> {
    let rest = value_name.strip_prefix(account)?.strip_prefix('_')?;
    Some(rest.rsplit_once("_h").map_or(rest, |(name, _)| name))
}

/// The active account's limits. `account` is what VRChat's log named this person (`usr_…`); without it there is nothing to
/// read, as the global keys and the other accounts' are not known to apply.
#[cfg(windows)]
pub fn read(account: Option<&str>) -> Limits {
    let Some(account) = account.filter(|a| a.starts_with("usr_")) else { return Limits::default() };
    limits_from(&account_values(account))
}

#[cfg(not(windows))]
pub fn read(_account: Option<&str>) -> Limits {
    Limits::default()
}

#[cfg(windows)]
fn account_values(account: &str) -> Vec<Raw> {
    use windows::core::{w, PWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
    let mut key = HKEY::default();
    // SAFETY: a valid out parameter; read access only.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\VRChat\\VRChat"), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut name = vec![0u16; 1024];
    let mut bytes = vec![0u8; 64];
    for index in 0.. {
        let mut name_len = name.len() as u32;
        let mut size = bytes.len() as u32;
        let mut kind = 0u32;
        // SAFETY: both buffers' sizes are what is passed.
        let status = unsafe { RegEnumValueW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut name_len, None, Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut size)) };
        if status == windows::Win32::Foundation::ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            // A value larger than this buffer is none of the ones looked for.
            continue;
        }
        let value_name = String::from_utf16_lossy(&name[..name_len as usize]);
        if let Some(field) = account_field(&value_name, account) {
            out.push(Raw { name: field.to_string(), kind, bytes: bytes[..size as usize].to_vec() });
        }
    }
    // SAFETY: opened above, closed once.
    unsafe {
        let _ = RegCloseKey(key);
    }
    out
}

/// The account the log names, from who is in the instance: the local player's user id, never kept beyond this call.
pub fn account_of<'a>(local_player: Option<&str>, players: impl IntoIterator<Item = (&'a String, &'a String)>) -> Option<String> {
    let local = local_player?;
    let mut found = players.into_iter().filter(|(id, name)| name.as_str() == local && id.starts_with("usr_")).map(|(id, _)| id.clone());
    let first = found.next()?;
    // Two accounts of one name in an instance: not to be guessed between.
    found.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dword(name: &str, value: u32) -> Raw {
        Raw { name: name.into(), kind: 4, bytes: value.to_le_bytes().to_vec() }
    }

    fn double(name: &str, value: f64) -> Raw {
        Raw { name: name.into(), kind: 4, bytes: value.to_le_bytes().to_vec() }
    }

    /// The raw form read on the real computer: -1.0 stored as `000000000000f0bf` under `REG_DWORD`.
    #[test]
    fn a_double_under_reg_dword_is_read_from_its_bytes() {
        let raw = Raw { name: "x".into(), kind: 4, bytes: vec![0, 0, 0, 0, 0, 0, 0xf0, 0xbf] };
        assert_eq!(as_double(&raw), Some(-1.0));
        assert_eq!(as_count(&raw), None, "it is no 4-byte number");
    }

    #[test]
    fn the_limits_are_their_values_with_their_switches() {
        let got = limits_from(&[dword("currentShowMaxNumberOfAvatarsEnabled", 1), dword("avatarProxyShowMaxNumber", 30), dword("avatarProxyShowAtRangeToggle", 1), double("avatarProxyShowAtRange", 12.7)]);
        assert_eq!(got.max_shown, Some(Known::Count(30)));
        assert!(matches!(got.hide_beyond, Some(Known::Meters(m)) if (m - 12.7).abs() < 0.01));
        // Switched off, whatever value stays stored.
        let got = limits_from(&[dword("currentShowMaxNumberOfAvatarsEnabled", 0), dword("avatarProxyShowMaxNumber", 30), dword("avatarProxyShowAtRangeToggle", 0), double("avatarProxyShowAtRange", 12.7)]);
        assert_eq!((got.max_shown, got.hide_beyond), (Some(Known::Off), Some(Known::Off)));
        // A value without its switch, or the ordinary read's truncated 0: not known, so not shown.
        let got = limits_from(&[dword("avatarProxyShowMaxNumber", 30), dword("avatarProxyShowAtRangeToggle", 1), dword("avatarProxyShowAtRange", 0)]);
        assert_eq!((got.max_shown, got.hide_beyond), (None, None));
    }

    #[test]
    fn only_this_accounts_values_are_its_own() {
        assert_eq!(account_field("usr_abc_avatarProxyShowMaxNumber_h123", "usr_abc"), Some("avatarProxyShowMaxNumber"));
        assert_eq!(account_field("usr_other_avatarProxyShowMaxNumber_h123", "usr_abc"), None);
        assert_eq!(account_field("avatarProxyShowMaxNumber_h171091884", "usr_abc"), None, "not the global ones");
        assert_eq!(account_field("usr_abcd_x_h1", "usr_abc"), None, "not an account with a longer ID");
    }

    #[test]
    fn the_account_is_the_one_the_log_names_or_none() {
        let players: std::collections::BTreeMap<String, String> = [("usr_a".to_string(), "Me".to_string()), ("usr_b".to_string(), "Them".to_string()), ("name:Odd".to_string(), "Odd".to_string())].into();
        assert_eq!(account_of(Some("Me"), &players), Some("usr_a".to_string()));
        assert_eq!(account_of(Some("Odd"), &players), None, "no real id");
        assert_eq!(account_of(None, &players), None);
        let twice: std::collections::BTreeMap<String, String> = [("usr_a".to_string(), "Me".to_string()), ("usr_b".to_string(), "Me".to_string())].into();
        assert_eq!(account_of(Some("Me"), &twice), None, "not guessed between");
    }

    #[test]
    fn without_an_account_nothing_is_read() {
        assert_eq!(read(None), Limits::default());
        assert_eq!(read(Some("not an account")), Limits::default());
    }
}
